//! `bd eval`: run eval cases through the command pass and the cleanup model
//! exactly as the pipeline does, writing `outputs.jsonl` for `eval/score.py`.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use bd_commands::{CommandConfig, CommandPass};
use bd_core::{CleanupRequest, Style};
use bd_models::Store;
use clap::Args;
use serde::Deserialize;

use crate::setup::{self, CleanupArgs, StyleArg};

#[derive(Debug, Args)]
pub struct EvalArgs {
    /// Cases JSONL files (`id`, `raw`, `context`, optional `style`).
    #[arg(long = "cases", required = true, num_args = 1..)]
    pub cases: Vec<PathBuf>,
    /// Directory for outputs.jsonl.
    #[arg(long)]
    pub out: PathBuf,
    /// Run only the first N cases.
    #[arg(long)]
    pub limit: Option<usize>,
    #[command(flatten)]
    pub cleanup: CleanupArgs,
    /// Print each case as it finishes.
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    raw: String,
    #[serde(default)]
    context: String,
    #[serde(default)]
    style: Option<String>,
}

pub fn run(args: &EvalArgs) -> Result<()> {
    let mut cases = Vec::new();
    for path in &args.cases {
        let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        for (n, line) in BufReader::new(file).lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let case: Case = serde_json::from_str(&line).with_context(|| format!("{}:{}", path.display(), n + 1))?;
            cases.push(case);
        }
    }
    if let Some(limit) = args.limit {
        cases.truncate(limit);
    }
    let store = Store::from_env()?;
    let started = Instant::now();
    let Some(loaded) = setup::load_cleanup(&args.cleanup, &store)? else {
        bail!("bd eval needs a cleanup model: pass --cleanup ID|PATH or --cleanup-url URL");
    };
    eprintln!("cleanup: {} (loaded in {:.1} s)", loaded.description, started.elapsed().as_secs_f64());
    let mut engine = loaded.engine;
    let warm = Style::default();
    // The first request also decodes the system prompt, which later ones
    // find in the cache: the app's first sentence after a load.
    let first = engine.clean_detailed(&CleanupRequest { context: "", raw: "Um, warm up the the model.", style: &warm })?;
    eprintln!("first request {:.0} ms", first.elapsed.as_secs_f64() * 1e3);

    fs::create_dir_all(&args.out)?;
    let out_path = args.out.join("outputs.jsonl");
    let mut out = BufWriter::new(File::create(&out_path)?);
    let (mut rejected, mut repaired, mut total_ms) = (0, 0, 0.0);
    for (k, case) in cases.iter().enumerate() {
        let style = match case.style.as_deref() {
            Some("uk") => StyleArg::Uk.style(),
            Some("us") | None => StyleArg::Us.style(),
            Some(other) => bail!("case {}: unknown style {other:?}", case.id),
        };
        let pass = CommandPass::new(CommandConfig::default(), style.clone());
        let commanded = pass.apply(&case.raw, ends_in_open_quote(&case.context, &style)).text;
        let request = CleanupRequest { context: &case.context, raw: &commanded, style: &style };
        let outcome = engine.clean_detailed(&request).with_context(|| format!("case {}", case.id))?;
        let ms = outcome.elapsed.as_secs_f64() * 1e3;
        total_ms += ms;
        rejected += usize::from(outcome.fell_back());
        repaired += usize::from(outcome.repair.is_some());
        let record = serde_json::json!({
            "id": case.id,
            "output": outcome.text,
            "latency_ms": (ms * 10.0).round() / 10.0,
            "model_input": commanded,
            "model_output": outcome.model_output,
            "guardrail_rejected": outcome.fell_back(),
            "repaired": outcome.repair.as_ref().map(|r| &r.restored),
        });
        writeln!(out, "{record}")?;
        out.flush()?;
        if args.verbose {
            eprintln!("[{}/{}] {} {ms:.0} ms: {:?}", k + 1, cases.len(), case.id, outcome.text);
        } else if (k + 1) % 20 == 0 {
            eprintln!("  {}/{}", k + 1, cases.len());
        }
    }
    let n = cases.len().max(1);
    eprintln!(
        "{} cases, mean {:.0} ms, {repaired} repaired, fell back on {rejected}. Wrote {}",
        cases.len(),
        total_ms / n as f64,
        out_path.display()
    );
    eprintln!("score it: python eval/score.py {} {}", join_paths(&args.cases), out_path.display());
    Ok(())
}

fn join_paths(paths: &[PathBuf]) -> String {
    paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(" ")
}

/// Whether the context leaves a quotation open, as `eval/score.py` decides
/// it: the last opening mark comes after the last closing mark.
fn ends_in_open_quote(context: &str, style: &Style) -> bool {
    let (open, close) = style.quotes.marks();
    match (context.rfind(open), context.rfind(close)) {
        (Some(o), Some(c)) => o > c,
        (Some(_), None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_quote_at_the_end_of_the_context() {
        let us = Style::default();
        assert!(ends_in_open_quote("He said, \u{201C}Wait", &us));
        assert!(!ends_in_open_quote("\u{201C}Wait,\u{201D} he said.", &us));
        assert!(!ends_in_open_quote("No quotes here.", &us));
        let uk = StyleArg::Uk.style();
        assert!(ends_in_open_quote("\u{2018}Wait", &uk));
    }
}
