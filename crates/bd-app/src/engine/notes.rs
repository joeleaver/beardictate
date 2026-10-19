//! The pipeline's events as alpha telemetry (docs/telemetry.md).

use bd_pipeline::Event;
use serde_json::json;

use crate::telemetry::{ms, record};

/// Records what `event` says, before the engine acts on it.
/// `live_typing`: text outside the pad is typed as it's spoken.
pub(super) fn note(event: &Event, live_typing: bool) {
    let heard = matches!(event, Event::Transcribed { .. } | Event::Partial { .. } | Event::Commanded { .. });
    if heard && !crate::telemetry::dictating() {
        return;
    }
    match event {
        Event::Transcribed { id, text, audio, level, gain, pause, elapsed } => record("transcribed", Some(*id), || {
            json!({
                "text": text,
                "audio_ms": ms(*audio),
                "level_dbfs": 20.0 * level.max(1e-9).log10(),
                "gain": gain,
                "pause_ms": pause.map(ms),
                "asr_ms": ms(*elapsed),
                "recognizer": "nemotron",
                "audio": cfg!(feature = "alpha"),
            })
        }),
        Event::Partial { id, text, since_speech } => {
            record("partial", Some(*id), || json!({"text": text, "since_speech_ms": ms(*since_speech)}));
        }
        Event::Commanded { id, heard, vocabulary, commanded, actions, pad_command } => record("commanded", Some(*id), || {
            json!({"heard": heard, "vocabulary": vocabulary, "commanded": commanded, "actions": actions, "pad_command": pad_command})
        }),
        Event::Cleaned { segments, context, raw, output, elapsed, report, pad } => {
            let r = report.clone().unwrap_or_default();
            record("cleaned", segments.last().copied(), || {
                json!({
                    "segments": segments,
                    "context": context,
                    "raw": raw,
                    "model_output": r.model_output,
                    "final": output,
                    "verdict": r.verdict,
                    "reason": r.reason,
                    "respelled": r.respelled,
                    "removed": r.removed,
                    "prompt_tokens": r.prompt_tokens,
                    "reused_tokens": r.reused_tokens,
                    "generated_tokens": r.generated_tokens,
                    "prefill_ms": ms(r.prefill),
                    "decode_ms": ms(r.decode),
                    "total_ms": ms(*elapsed),
                    "pad": pad,
                })
            });
        }
        Event::Text { id, take_back, text, cleanup, .. } => record("typed", Some(*id), || {
            json!({"text": text, "take_back": take_back, "live": cleanup.is_none() && live_typing})
        }),
        Event::Erase { id, text } => record("scratch", Some(*id), || json!({"what": "erased", "text": text})),
        Event::Scratched { id } => record("scratch", Some(*id), || json!({"what": "queued"})),
        Event::ScratchTooLate => record("scratch", None, || json!({"what": "too_late"})),
        Event::PadSend { text } => record("pad", None, || json!({"action": "send", "sent_chars": text.chars().count(), "text": text})),
        Event::PadNotFound { words } => record("pad", None, || json!({"action": "edit", "words": words, "found": false})),
        Event::Error { stage, message } => record("error", None, || json!({"stage": format!("{stage:?}"), "message": message})),
        Event::Woke => record("state", None, || json!({"state": "awake"})),
        Event::Slept => record("state", None, || json!({"state": "asleep"})),
        Event::ListMode(on) => record("state", None, || json!({"list_mode": on})),
        Event::SessionEnded => record("state", None, || json!({"state": "session_ended"})),
        // The pad's view changes on every word; the pad events above say what matters.
        Event::Pad(_) => {}
    }
}
