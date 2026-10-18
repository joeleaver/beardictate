//! A tracing layer that records warnings and errors, and what llama.cpp and
//! the model loader say (where a GPU that isn't used shows why), as `log`
//! events. At most [`MAX_LOGS`] a session.

use std::fmt::Write;
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::json;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;

const MAX_LOGS: u32 = 200;

static COUNT: AtomicU32 = AtomicU32::new(0);

/// Install with the app's other logging layers.
pub struct Layer;

impl<S: Subscriber> tracing_subscriber::Layer<S> for Layer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let target = meta.target();
        let wanted = *meta.level() <= Level::WARN
            || target.starts_with("llama-cpp-2")
            || target.starts_with("bd_cleanup")
            || target.starts_with("beardictate::engine::load");
        // Never this module's own messages: no loops.
        if !wanted || target.contains("telemetry") || COUNT.fetch_add(1, Ordering::Relaxed) >= MAX_LOGS {
            return;
        }
        let mut message = Message(String::new());
        event.record(&mut message);
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
