//! `bd`: beardictate without the app. Dictate from a WAV or the microphone
//! to stdout (or into the focused window), manage models, run the eval.

mod dictate;
mod eval;
mod models;
mod setup;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "bd", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Dictate from WAV files: text on stdout.
    File(dictate::FileArgs),
    /// Dictate from the microphone, toggled with Enter: text on stdout.
    Mic(dictate::MicArgs),
    /// List, download, verify or import models.
    #[command(subcommand)]
    Models(models::ModelsCommand),
    /// List audio input devices.
    Devices,
    /// Run cleanup eval cases; writes outputs.jsonl for eval/score.py.
    Eval(eval::EvalArgs),
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_env("BD_LOG").unwrap_or_else(|_| EnvFilter::new("warn")))
        .with_writer(std::io::stderr)
        .init();
    match Cli::parse().command {
        Command::File(args) => dictate::file(&args),
        Command::Mic(args) => dictate::mic(&args),
        Command::Models(command) => models::run(&command),
        Command::Devices => models::devices(),
        Command::Eval(args) => eval::run(&args),
    }
}
