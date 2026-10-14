//! Manual test tool: waits, then injects text into whatever has focus.
//!
//! ```text
//! cargo run -p bd-platform --example inject -- [--paste|--type] [--shift-enter]
//!     [--chord ctrl-v|ctrl-shift-v|shift-insert] [--delay SECS] [--hold SECS]
//!     [--prepare] TEXT
//! ```
//!
//! `TEXT` may contain `\n` and `\t` escapes. During the delay, focus the
//! target app. Prints the outcome and the backend used. `--hold` keeps the
//! process alive afterwards: on X11 and Wayland the clipboard content we
//! own (the restored one, or text left on the clipboard) disappears when
//! the process exits unless a clipboard manager takes it over.

use std::time::Duration;

use bd_platform::{InjectorConfig, NewlineKey, PasteChord, PlatformInjector, Strategy};

fn main() {
    let mut config = InjectorConfig::default();
    let mut delay = 3.0f64;
    let mut hold = 0.0f64;
    let mut prepare = false;
    let mut text = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--paste" => config.strategy = Strategy::PasteOnly,
            "--type" => config.strategy = Strategy::TypeOnly,
            "--shift-enter" => config.newline = NewlineKey::ShiftEnter,
            "--prepare" => prepare = true,
            "--chord" => {
                config.paste_chord = match args.next().as_deref() {
                    Some("ctrl-v") => PasteChord::CtrlV,
                    Some("ctrl-shift-v") => PasteChord::CtrlShiftV,
                    Some("shift-insert") => PasteChord::ShiftInsert,
                    other => exit(&format!("unknown chord {other:?}")),
                }
            }
            "--delay" => delay = seconds(args.next()),
            "--hold" => hold = seconds(args.next()),
            _ => text = Some(arg.replace("\\n", "\n").replace("\\t", "\t")),
        }
    }
    let text = text.unwrap_or_else(|| exit("no text given"));
    let mut injector = PlatformInjector::new(config).unwrap_or_else(|e| exit(&e.to_string()));
    println!("backend: {}", injector.backend_name());
    if prepare {
        injector.prepare().unwrap_or_else(|e| exit(&e.to_string()));
    }
    std::thread::sleep(Duration::from_secs_f64(delay));
    println!("foreground: {:?}", injector.foreground_app());
    match injector.inject_text(&text) {
        Ok(outcome) => println!("outcome: {outcome:?}"),
        Err(e) => exit(&e.to_string()),
    }
    std::thread::sleep(Duration::from_secs_f64(hold));
}

fn seconds(arg: Option<String>) -> f64 {
    arg.and_then(|d| d.parse().ok())
        .unwrap_or_else(|| exit("--delay/--hold need seconds"))
}

fn exit(msg: &str) -> ! {
    eprintln!("inject: {msg}");
    std::process::exit(1)
}
