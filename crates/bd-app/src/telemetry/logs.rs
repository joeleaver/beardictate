//! A tracing layer that records warnings and errors, and what llama.cpp and
//! the model loader say (where a GPU that isn't used shows why), as `log`
//! events. Warnings and errors get [`MAX_WARNINGS`] a session and the
//! loaders' chatter [`MAX_CHATTER`] of its own, so a model load never uses
//! up what's left for later warnings. llama.cpp's line per layer and per
//! tensor is left out: the `load` event says how many layers went to the GPU.

use std::fmt::Write;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::json;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;

const MAX_WARNINGS: u32 = 200;
const MAX_CHATTER: u32 = 100;

static WARNINGS: AtomicU32 = AtomicU32::new(0);
static CHATTER: AtomicU32 = AtomicU32::new(0);

/// The same message (numbers aside) is recorded this often, then only as it
/// reaches 10, 100, 1000… times, with the count: wgpu's "Suboptimal present
/// of frame N", 195 times in 30 seconds, once used up a session's warnings.
const SAME_MESSAGE: u32 = 3;

static SEEN: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);

/// How many times a message like this one has come, this one included.
fn times_seen(target: &str, message: &str) -> u32 {
    let key = format!("{target} {}", message.chars().filter(|c| !c.is_ascii_digit()).collect::<String>());
    let mut seen = SEEN.lock().unwrap_or_else(|p| p.into_inner());
    let n = seen.get_or_insert_with(HashMap::new).entry(key).or_insert(0);
    *n += 1;
    *n
}

/// Whether the `n`th sighting of a message is recorded.
fn recorded(n: u32) -> bool {
    n <= SAME_MESSAGE || (n >= 10 && power_of_ten(n))
}

fn power_of_ten(mut n: u32) -> bool {
    while n >= 10 && n.is_multiple_of(10) {
        n /= 10;
    }
    n == 1
}

/// Which budget a message counts against, if it's recorded at all.
#[derive(Debug, PartialEq)]
enum Budget {
    Warnings,
    Chatter,
}

fn budget(level: &Level, target: &str, message: &str) -> Option<Budget> {
    // Never this module's own messages: no loops.
    if target.contains("telemetry") {
        return None;
    }
    if *level <= Level::WARN {
        return Some(Budget::Warnings);
    }
    let loader = target.starts_with("llama-cpp-2") || target.starts_with("bd_cleanup") || target.starts_with("beardictate::engine::load");
    (loader && !per_layer(message)).then_some(Budget::Chatter)
}

/// llama.cpp's lines about each layer and tensor ("layer 12 assigned to
/// device Vulkan0", "loading tensor blk.3.attn_q.weight", "- tensor …",
/// "- kv …"): a model's worth of them is hundreds of lines.
fn per_layer(message: &str) -> bool {
    let m = message.trim_start();
    let layer = m.strip_prefix("layer ").is_some_and(|rest| rest.trim_start().starts_with(|c: char| c.is_ascii_digit()) && rest.contains(" assigned to device "));
    layer || m.starts_with("loading tensor ") || m.starts_with("- tensor ") || m.starts_with("- kv ")
}

/// Install with the app's other logging layers.
pub struct Layer;

impl<S: Subscriber> tracing_subscriber::Layer<S> for Layer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let target = meta.target();
        let mut message = Message(String::new());
        event.record(&mut message);
        let (count, max) = match budget(meta.level(), target, &message.0) {
            Some(Budget::Warnings) => (&WARNINGS, MAX_WARNINGS),
            Some(Budget::Chatter) => (&CHATTER, MAX_CHATTER),
            None => return,
        };
        let n = times_seen(target, &message.0);
        if !recorded(n) {
            return;
        }
        if n > SAME_MESSAGE {
            let _ = write!(message.0, " (seen {n} times)");
        }
        if count.fetch_add(1, Ordering::Relaxed) >= max {
            return;
        }
        let level = meta.level().to_string();
        let target = target.to_string();
        super::record("log", None, move || json!({"level": level, "target": target, "message": message.0}));
    }
}

struct Message(String);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.0, "{value:?}");
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        } else {
            let _ = write!(self.0, " {}={value}", field.name());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warnings_and_loader_chatter_count_separately() {
        let llama = "llama-cpp-2";
        assert_eq!(budget(&Level::WARN, llama, "control-looking token"), Some(Budget::Warnings));
        assert_eq!(budget(&Level::ERROR, "beardictate::engine", "x"), Some(Budget::Warnings));
        assert_eq!(budget(&Level::INFO, llama, "n_layer = 35 module=llama.cpp::print_info"), Some(Budget::Chatter));
        assert_eq!(budget(&Level::DEBUG, llama, "Found 2 Vulkan devices: module=ggml::ggml_vulkan"), Some(Budget::Chatter));
        assert_eq!(budget(&Level::INFO, "beardictate::engine::load", "speech: Nemotron"), Some(Budget::Chatter));
        assert_eq!(budget(&Level::INFO, "beardictate::update", "updates"), None);
        assert_eq!(budget(&Level::WARN, "beardictate::telemetry::upload", "x"), None);
    }

    #[test]
    fn a_repeated_message_is_recorded_three_times_then_at_tens() {
        let kept: Vec<u32> = (1..=1000).filter(|&n| recorded(n)).collect();
        assert_eq!(kept, [1, 2, 3, 10, 100, 1000]);
        let t = "wgpu_hal::vulkan::test";
        assert_eq!(times_seen(t, "Suboptimal present of frame 1"), 1);
        assert_eq!(times_seen(t, "Suboptimal present of frame 2"), 2, "numbers aside, the same message");
        assert_eq!(times_seen(t, "Another message"), 1);
    }

    #[test]
    fn per_layer_lines_are_left_out() {
        let llama = "llama-cpp-2";
        for m in [
            "layer  7 assigned to device Vulkan0, is_swa = 1 module=llama.cpp::load_tensors",
            "layer 34 assigned to device CPU, is_swa = 0 module=llama.cpp::load_tensors",
            "loading tensor token_embd.weight module=llama.cpp::create_tensor",
            "- tensor   12:  blk.1.attn_q.weight q4_0 [2048, 2048, 1, 1]",
            "- kv   3: general.name str = Merged Adapter",
        ] {
            assert_eq!(budget(&Level::DEBUG, llama, m), None, "{m}");
        }
        // A warning about a layer still counts.
        assert_eq!(budget(&Level::WARN, llama, "layer 3 assigned to device CPU"), Some(Budget::Warnings));
        assert!(!per_layer("layers offloaded: 36/36"));
    }
}
