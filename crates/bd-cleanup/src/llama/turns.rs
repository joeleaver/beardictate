//! Chat formats llama.cpp's built-in template engine doesn't know. It
//! matches known templates by their text and can't run arbitrary Jinja, so
//! a newer model's template fails with "ffi error -1". These render what
//! the model's own template would for plain text turns (no tools, no
//! thinking), checked against the template shipped in the GGUF.

use crate::{ChatMessage, prompt::Role};

/// A chat format rendered here rather than by llama.cpp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Turns {
    /// Gemma 4: `<|turn>role\n…<turn|>\n`, the assistant as `model`, every
    /// message trimmed; thinking is off unless the system turn starts with
    /// `<|think|>`, so nothing is added to keep it off.
    Gemma4,
}

impl Turns {
    /// The format a model's own template uses, if it's one of these.
    pub fn detect(template: &str) -> Option<Turns> {
        (template.contains("<|turn>") && template.contains("<turn|>")).then_some(Turns::Gemma4)
    }

    /// The prompt, ending where the model starts its answer. No BOS: the
    /// tokenizer adds it.
    pub fn render(self, messages: &[ChatMessage]) -> String {
        match self {
            Turns::Gemma4 => {
                let mut out = String::new();
                for m in messages {
                    let role = match m.role {
                        Role::System => "system",
                        Role::User => "user",
                        Role::Assistant => "model",
                    };
                    out.push_str(&format!("<|turn>{role}\n{}<turn|>\n", m.content.trim()));
                }
                out.push_str("<|turn>model\n");
                out
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: Role, content: &str) -> ChatMessage {
        ChatMessage { role, content: content.into() }
    }

    #[test]
    fn gemma4_turns_match_its_template() {
        assert_eq!(Turns::detect("{{- '<|turn>system\\n' -}} … {{- '<turn|>\\n' -}}"), Some(Turns::Gemma4));
        assert_eq!(Turns::detect("<|im_start|>user"), None);
        let prompt = Turns::Gemma4.render(&[
            msg(Role::System, "Clean it.\n"),
            msg(Role::User, "Dictation:\nhi"),
            msg(Role::Assistant, "\n\n\u{201C}Hi.\u{201D}"),
            msg(Role::User, "Dictation:\nbye"),
        ]);
        assert_eq!(
            prompt,
            "<|turn>system\nClean it.<turn|>\n<|turn>user\nDictation:\nhi<turn|>\n\
             <|turn>model\n\u{201C}Hi.\u{201D}<turn|>\n<|turn>user\nDictation:\nbye<turn|>\n<|turn>model\n"
        );
    }
}
