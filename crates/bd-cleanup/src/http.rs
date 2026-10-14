//! Cleanup through any OpenAI-compatible chat-completions server (Ollama,
//! LM Studio, llama-server, vLLM, a hosted API).
//!
//! The text leaves the app, even for localhost, so the UI must make this an
//! explicit opt-in.

use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;

use crate::{ChatBackend, ChatMessage, CleanupError, Completion, GenerationStats};

/// Where and how to reach the server.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpConfig {
    /// Server root, with or without a trailing `/v1`:
    /// `http://127.0.0.1:8080` or `http://localhost:11434/v1`.
    pub base_url: String,
    /// Model name sent in the request (`qwen3:4b`); some servers ignore it.
    pub model: String,
    /// Sent as `Authorization: Bearer …` when set.
    pub api_key: Option<String>,
    pub temperature: f32,
    /// Ask the server to skip thinking with
    /// `chat_template_kwargs.enable_thinking = false` (llama-server, vLLM).
    /// Turn off for servers that reject unknown fields.
    pub disable_thinking: bool,
    /// Whole-request timeout.
    pub timeout: Duration,
}

impl HttpConfig {
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            api_key: None,
            temperature: 0.0,
            disable_thinking: true,
            timeout: Duration::from_secs(60),
        }
    }

    /// The full chat-completions URL.
    pub fn endpoint(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if base.ends_with("/v1") {
            format!("{base}/chat/completions")
        } else {
            format!("{base}/v1/chat/completions")
        }
    }
}

/// An OpenAI-compatible chat-completions client.
pub struct HttpChat {
    config: HttpConfig,
    agent: ureq::Agent,
}

impl HttpChat {
    pub fn new(config: HttpConfig) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(config.timeout))
            .http_status_as_error(false)
            .build()
            .into();
        Self { config, agent }
    }

    pub fn config(&self) -> &HttpConfig {
        &self.config
    }
}

#[derive(Deserialize)]
struct Response {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Deserialize)]
struct Message {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: usize,
    #[serde(default)]
    completion_tokens: usize,
}

impl ChatBackend for HttpChat {
    fn complete(&mut self, messages: &[ChatMessage], max_tokens: usize) -> Result<Completion, CleanupError> {
        let url = self.config.endpoint();
        let messages: Vec<_> = messages
            .iter()
            .map(|m| json!({ "role": m.role.as_str(), "content": m.content }))
            .collect();
        let mut body = json!({
            "model": self.config.model,
            "messages": messages,
            "temperature": self.config.temperature,
            "max_tokens": max_tokens,
            "stream": false,
        });
        if self.config.disable_thinking {
            body["chat_template_kwargs"] = json!({ "enable_thinking": false });
        }
        let mut request = self.agent.post(&url).header("Content-Type", "application/json");
        if let Some(key) = &self.config.api_key {
            request = request.header("Authorization", &format!("Bearer {key}"));
        }
        let started = Instant::now();
        let http_err = |e: ureq::Error| CleanupError::Http { url: url.clone(), message: e.to_string() };
        let mut response = request.send_json(&body).map_err(http_err)?;
        let status = response.status();
        if !status.is_success() {
            let detail = response.body_mut().read_to_string().unwrap_or_default();
            let detail: String = detail.chars().take(300).collect();
            return Err(CleanupError::Http { url, message: format!("HTTP {status}: {detail}") });
        }
        let parsed: Response = response.body_mut().read_json().map_err(|e| CleanupError::Protocol {
            url: url.clone(),
            message: e.to_string(),
        })?;
        let text = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| CleanupError::Protocol { url: url.clone(), message: "no message content".into() })?;
        let stats = parsed.usage.map(|u| GenerationStats {
            prompt_tokens: u.prompt_tokens,
            generated_tokens: u.completion_tokens,
            decode: started.elapsed(),
            ..GenerationStats::default()
        });
        Ok(Completion { text, stats })
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;
    use crate::Role;

    /// Serves one canned response and returns the request it received.
    fn serve_once(status: &str, body: &str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut head = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
                head.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            reader.get_mut().write_all(response.as_bytes()).unwrap();
            head + &String::from_utf8(body).unwrap()
        });
        (format!("http://{addr}"), handle)
    }

    fn messages() -> Vec<ChatMessage> {
        vec![
            ChatMessage { role: Role::System, content: "sys".into() },
            ChatMessage { role: Role::User, content: "hi".into() },
        ]
    }

    #[test]
    fn endpoint_accepts_both_base_forms() {
        assert_eq!(HttpConfig::new("http://h:1", "m").endpoint(), "http://h:1/v1/chat/completions");
        assert_eq!(HttpConfig::new("http://h:1/v1/", "m").endpoint(), "http://h:1/v1/chat/completions");
    }

    #[test]
    fn sends_openai_request_and_reads_reply() {
        let (base, server) = serve_once(
            "200 OK",
            r#"{"choices":[{"message":{"role":"assistant","content":"He left."}}],"usage":{"prompt_tokens":12,"completion_tokens":3}}"#,
        );
        let mut config = HttpConfig::new(base, "qwen3:4b");
        config.api_key = Some("sk-test".into());
        let mut chat = HttpChat::new(config);
        let reply = chat.complete(&messages(), 50).unwrap();
        assert_eq!(reply.text, "He left.");
        assert_eq!(reply.stats.unwrap().generated_tokens, 3);
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /v1/chat/completions"));
        assert!(request.contains("Bearer sk-test"));
        let body: serde_json::Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["model"], "qwen3:4b");
        assert_eq!(body["max_tokens"], 50);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
    }

    #[test]
    fn http_errors_carry_status_and_detail() {
        let (base, server) = serve_once("500 Internal Server Error", r#"{"error":"model not loaded"}"#);
        let err = HttpChat::new(HttpConfig::new(base, "m")).complete(&messages(), 10).unwrap_err();
        server.join().unwrap();
        let text = err.to_string();
        assert!(text.contains("500") && text.contains("model not loaded"), "{text}");
    }

    #[test]
    fn malformed_reply_is_a_protocol_error() {
        let (base, server) = serve_once("200 OK", r#"{"choices":[]}"#);
        let err = HttpChat::new(HttpConfig::new(base, "m")).complete(&messages(), 10).unwrap_err();
        server.join().unwrap();
        assert!(matches!(err, CleanupError::Protocol { .. }), "{err}");
    }
}
