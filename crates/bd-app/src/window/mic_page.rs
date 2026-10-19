//! The Microphone page: which microphone, how loud it's coming in, its gain
//! (learned from your speech or set by hand), and a test that shows what
//! was heard without typing anything.

use rinch::prelude::*;
use rinch::reactive::Effect;

use crate::engine::Command;
use crate::ui::{Section, Shared, change};

/// The Select value standing for "no particular microphone".
const DEFAULT_MIC: &str = "(default)";

/// dBFS for an RMS level.
fn db(level: f32) -> f32 {
    20.0 * level.max(1e-6).log10()
}

/// Where a level sits on the meter, 0 to 100 (-60 dBFS to 0).
fn meter_pct(level: f32) -> f32 {
    ((db(level) + 60.0) / 60.0 * 100.0).clamp(0.0, 100.0)
}

pub fn microphone(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let mut data = vec![SelectOption::new(DEFAULT_MIC, "System default")];
    match bd_audio::list_input_devices() {
        Ok(devices) => data.extend(devices.into_iter().map(|d| SelectOption::new(d.name.clone(), d.name))),
        Err(e) => tracing::warn!("listing microphones: {e}"),
    }
    let mic = s.mic;
    let settings = s.settings;
    // Leaving the page ends a test, so dictation types again.
    let section = s.section;
    Effect::new(move || {
        if section.get() != Section::Microphone && rinch::reactive::untracked(|| mic.testing.get()) {
            mic.testing.set(false);
            crate::engine_send(Command::MicTest(false));
        }
    });
    let manual_db = Signal::new(f64::from(s.settings.with(|st| st.mic_gain_db)));
    let (s1, s2, s3, s4, s5, s6) = (s.clone(), s.clone(), s.clone(), s.clone(), s.clone(), s.clone());
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 18px;",
            Title { order: 3, "Microphone" }
            Select {
                label: "Microphone",
                data: data,
                value_fn: move || s1.settings.with(|st| if st.microphone.is_empty() { DEFAULT_MIC.to_string() } else { st.microphone.clone() }),
                onchange: move |v: String| change(&s2, |st| st.microphone = if v == DEFAULT_MIC { String::new() } else { v }),
            }
            div { style: "display: flex; flex-direction: column; gap: 6px;",
                div { style: "display: flex; justify-content: space-between; align-items: baseline;",
                    Text { style: "font-weight: 600;", "Input level" }
                    Text { size: "sm", color: "dimmed", {move || format!("{:.0} dBFS", db(mic.level.get()))} }
                }
                // Too quiet below -40 dBFS, good to -12, too loud past -6.
                div { style: "position: relative; height: 12px; border-radius: 6px; overflow: hidden; background: linear-gradient(to right, #f1c4c4 0%, #f1c4c4 33%, #c9ecd2 33%, #c9ecd2 80%, #f6e1b5 80%, #f6e1b5 90%, #f1c4c4 90%);",
                    div { style: {move || format!("height: 100%; width: {:.0}%; background: rgba(36, 89, 184, 0.75); transition: width 80ms linear;", meter_pct(mic.level.get()))} }
                }
                div { style: "display: flex; justify-content: space-between; font-size: 11px; color: var(--rinch-color-dimmed);",
                    span { "too quiet" } span { "good" } span { "too loud" }
                }
                Text { size: "xs", color: "dimmed", "Speak normally: the bar should sit in the green. It moves while the microphone is open (dictating, always listening, or during a test)." }
            }
            div { style: "display: flex; flex-direction: column; gap: 8px;",
                Switch {
                    label: "Adjust the gain automatically",
                    description: "Learns from what you say, like a call app, and keeps it there; silence and background noise don't change it.",
                    checked_fn: move || s3.settings.with(|st| st.auto_gain),
                    onchange: move || change(&s4, |st| st.auto_gain = !st.auto_gain),
                }
                if settings.with(|st| st.auto_gain) {
                    Text { size: "sm", color: "dimmed", {move || format!("Gain now: +{:.0} dB", db(mic.gain.get()))} }
                } else {
                    div { style: "display: flex; flex-direction: column; gap: 4px;",
                        div { style: "display: flex; justify-content: space-between;",
                            Text { size: "sm", "Gain" }
                            Text { size: "sm", color: "dimmed", {move || format!("+{:.0} dB", manual_db.get())} }
                        }
                        Slider {
                            min: Some(0.0),
                            max: Some(30.0),
                            step: Some(1.0),
                            value_signal: Some(manual_db),
                            onchange: {let s = s5.clone(); move |v: f64| {
                                manual_db.set(v);
                                change(&s, |st| st.mic_gain_db = v as f32);
                            }},
                        }
                    }
                }
            }
            div { style: "display: flex; flex-direction: column; gap: 8px; padding-top: 14px; border-top: 1px solid var(--rinch-color-default-border);",
                Group {
                    Button {
                        variant: {move || if mic.testing.get() { "filled" } else { "light" }},
                        onclick: move || {
                            let on = !mic.testing.get();
                            mic.testing.set(on);
                            if on {
                                mic.heard.set(Vec::new());
                            }
                            crate::engine_send(Command::MicTest(on));
                        },
                        {move || if mic.testing.get() { "Stop the test" } else { "Test the microphone" }}
                    }
                    Text { size: "sm", color: "dimmed", {move || if mic.testing.get() { "Say a sentence or two. Nothing is typed while testing." } else { "" }} }
                }
                for (i, (text, level)) in mic.heard.get().into_iter().enumerate() {
                    let shown = if text.trim().is_empty() { "(nothing recognized)".to_string() } else { text.clone() };
                    div { key: format!("{i}:{text}"), style: "display: flex; gap: 10px; align-items: baseline;",
                        Text { size: "xs", color: "dimmed", style: "width: 64px; flex: none;", {format!("{:.0} dBFS", db(level))} }
                        Text { size: "sm", {shown} }
                    }
                }
            }
            Switch {
                label: "Keep the microphone open between dictations",
                description: "When Always listening is off: your first word is never clipped, but the desktop shows the microphone as in use all the time.",
                checked_fn: move || s6.settings.with(|st| st.keep_mic_open),
                onchange: {let s = s6.clone(); move || change(&s, |st| st.keep_mic_open = !st.keep_mic_open)},
            }
        }
    }
}
