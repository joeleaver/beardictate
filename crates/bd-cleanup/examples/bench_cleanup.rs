//! Runs eval cases through a cleanup backend and reports latency and
//! quality.
//!
//! ```text
//! cargo run -p bd-cleanup --release --example bench_cleanup -- \
//!     --model /mnt/data/models/Qwen3-4B/Qwen3-4B-Q4_K_M.gguf \
//!     --template generic-v1 --template generic-fewshot-v1 \
//!     --cases eval/cases.jsonl
//! ```
//!
//! Each case's `raw` goes through the bd-commands pass first, as in the app.
//! Models load one at a time and are freed before the next. Cases are JSONL
//! with `context`, `raw`, and optional `id`, `expected`, `style` (`us`/`uk`);
//! without `--cases`, the four seed cases from the 2026-10-14 bench run.
//!
//! Scores per (model, template):
//! - `exact`: final text equals `expected` (whitespace-normalized);
//! - `words+marks`: same words, and the same quote marks and breaks between
//!   the same words, ignoring commas and periods; the dialog score;
//! - `accepted`: the guardrail let the model's output through.
//!
//! `--guardrail-only` just measures the guardrail's false rejects on the
//! expected outputs, with no model.

use std::path::PathBuf;
use std::time::Duration;

use bd_cleanup::guardrail::{self, Tokens};
use bd_cleanup::{ChatBackend, GuardedCleanup, HttpChat, HttpConfig};
use bd_commands::CommandPass;
use bd_core::{CleanupRequest, PunctuationPlacement, QuoteStyle, Style};
use clap::Parser;

#[derive(Parser)]
struct Args {
    /// GGUF model to load (repeatable; loaded one at a time).
    #[arg(long)]
    model: Vec<PathBuf>,
    /// OpenAI-compatible server base URL instead of / as well as models.
    #[arg(long)]
    http: Option<String>,
    #[arg(long, default_value = "local")]
    http_model: String,
    /// Prompt template id (repeatable).
    #[arg(long, default_value = "generic-v1")]
    template: Vec<String>,
    /// JSONL eval cases; default: the four seed cases.
    #[arg(long)]
    cases: Vec<PathBuf>,
    /// Only the first N cases of each file.
    #[arg(long)]
    limit: Option<usize>,
    /// Keep N cases spread evenly over all loaded cases (every category).
    #[arg(long)]
    sample: Option<usize>,
    /// GPU layers: "auto", "none" or a count.
    #[arg(long, default_value = "auto")]
    gpu_layers: String,
    #[arg(long)]
    threads: Option<u32>,
    /// Print every case, not only misses.
    #[arg(long)]
    verbose: bool,
    /// Only check the guardrail against the expected outputs.
    #[arg(long)]
    guardrail_only: bool,
}

struct Case {
    id: String,
    context: String,
    raw: String,
    expected: Option<String>,
    style: Style,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn,llama-cpp-2=error".into()))
        .with_writer(std::io::stderr)
        .init();
    let args = Args::parse();
    let cases = load_cases(&args)?;
    println!("{} cases", cases.len());
    guardrail_false_rejects(&cases);
    if args.guardrail_only {
        return Ok(());
    }
    if let Some(url) = &args.http {
        let chat = HttpChat::new(HttpConfig::new(url.clone(), args.http_model.clone()));
        bench_backend(&format!("http {url}"), chat, &args, &cases)?;
    }
    for path in &args.model {
        bench_model(path, &args, &cases)?;
    }
    Ok(())
}

#[cfg(feature = "llama")]
fn bench_model(path: &std::path::Path, args: &Args, cases: &[Case]) -> Result<(), Box<dyn std::error::Error>> {
    use bd_cleanup::{GpuLayers, LlamaChat, LlamaConfig};
    let mut config = LlamaConfig::new(path);
    config.gpu_layers = match args.gpu_layers.as_str() {
        "auto" => GpuLayers::Auto,
        "none" => GpuLayers::None,
        n => GpuLayers::Count(n.parse()?),
    };
    config.threads = args.threads;
    let started = std::time::Instant::now();
    let chat = LlamaChat::load(config)?;
    let info = chat.info().clone();
    println!(
        "\n## {} — loaded in {:.1}s ({:.1} GB, {} of {} layers on GPU, {} threads, thinking suppressed: {})",
        path.file_name().unwrap_or_default().to_string_lossy(),
        started.elapsed().as_secs_f32(),
        info.size_bytes as f64 / 1e9,
        info.gpu_layers,
        info.n_layer,
        info.threads,
        info.thinking_suppressed,
    );
    bench_backend(&path.display().to_string(), chat, args, cases)
    // `chat` was moved into bench_backend and dropped there, freeing the
    // model before the next one loads.
}

#[cfg(not(feature = "llama"))]
fn bench_model(_: &std::path::Path, _: &Args, _: &[Case]) -> Result<(), Box<dyn std::error::Error>> {
    Err("built without the `llama` feature".into())
}

#[derive(Default)]
struct Tally {
    n: usize,
    with_expected: usize,
    exact: usize,
    structure: usize,
    model_structure: usize,
    accepted: usize,
    latencies: Vec<Duration>,
}

fn bench_backend<B: ChatBackend>(label: &str, backend: B, args: &Args, cases: &[Case]) -> Result<(), Box<dyn std::error::Error>> {
    let mut cleanup = GuardedCleanup::new(backend, &args.template[0])?;
    for template in &args.template {
        cleanup.set_template(template)?;
        // Warm-up: first call pays for the system prompt's prefill.
        let style = Style::default();
        cleanup.clean_detailed(&CleanupRequest { context: "", raw: "Warm up.", style: &style })?;
        let mut tally = Tally::default();
        for case in cases {
            let commands = CommandPass::new(Default::default(), case.style.clone()).apply(&case.raw, false);
            let request = CleanupRequest { context: &case.context, raw: &commands.text, style: &case.style };
            let outcome = cleanup.clean_detailed(&request)?;
            tally.n += 1;
            tally.latencies.push(outcome.elapsed);
            if outcome.verdict.as_ref().is_some_and(|v| v.is_accepted()) {
                tally.accepted += 1;
            }
            let model_text = outcome.model_output.clone().unwrap_or_default();
            let mut miss = false;
            if let Some(expected) = &case.expected {
                tally.with_expected += 1;
                let exact = normalize_ws(&outcome.text) == normalize_ws(expected);
                let structure = same_structure(&outcome.text, expected);
                tally.exact += usize::from(exact);
                tally.structure += usize::from(structure);
                tally.model_structure += usize::from(same_structure(&model_text, expected));
                miss = !structure;
            }
            if args.verbose || miss || outcome.fell_back() {
                let stats = outcome.stats.as_ref().map_or(String::new(), |s| {
                    format!(
                        " prompt {} (reused {}), gen {} | prefill {:.2}s decode {:.2}s",
                        s.prompt_tokens,
                        s.reused_prompt_tokens,
                        s.generated_tokens,
                        s.prefill.as_secs_f32(),
                        s.decode.as_secs_f32()
                    )
                });
                println!("--- {} [{}] {:.2}s{stats}", case.id, template, outcome.elapsed.as_secs_f32());
                println!("  in:    {:?}", commands.text);
                println!("  model: {model_text:?}");
                if let Some(guardrail::GuardrailVerdict::Rejected(r)) = &outcome.verdict {
                    println!("  REJECTED: {r}");
                }
                if let Some(expected) = &case.expected {
                    println!("  want:  {expected:?}");
                }
            }
        }
        report(label, template, &mut tally);
    }
    Ok(())
}

fn report(label: &str, template: &str, t: &mut Tally) {
    t.latencies.sort();
    let pct = |p: f64| {
        let i = ((t.latencies.len() as f64 - 1.0) * p).round() as usize;
        t.latencies.get(i).map_or(0.0, Duration::as_secs_f64)
    };
    let mean = t.latencies.iter().map(Duration::as_secs_f64).sum::<f64>() / t.latencies.len().max(1) as f64;
    println!(
        "=== {label} [{template}] n={} | latency mean {mean:.2}s p50 {:.2}s p95 {:.2}s max {:.2}s | accepted {}/{} | exact {}/{} | words+marks {}/{} (model before guardrail {}/{})",
        t.n,
        pct(0.5),
        pct(0.95),
        pct(1.0),
        t.accepted,
        t.n,
        t.exact,
        t.with_expected,
        t.structure,
        t.with_expected,
        t.model_structure,
        t.with_expected,
    );
}

/// The guardrail must accept every expected output as an edit of its input.
fn guardrail_false_rejects(cases: &[Case]) {
    let mut checked = 0;
    let mut rejected = 0;
    for case in cases {
        let Some(expected) = &case.expected else { continue };
        let commands = CommandPass::new(Default::default(), case.style.clone()).apply(&case.raw, false);
        let output = bd_cleanup::finalize(&commands.text, expected);
        checked += 1;
        if let guardrail::GuardrailVerdict::Rejected(r) = guardrail::review_in_context(&case.context, &commands.text, &output) {
            rejected += 1;
            println!("FALSE REJECT {}: {r}\n  in:   {:?}\n  want: {:?}", case.id, commands.text, expected);
        }
    }
    println!("guardrail false rejects on expected outputs: {rejected}/{checked}");
}

/// Collapses runs of spaces, drops spaces around line breaks and trailing
/// whitespace; leading breaks count.
fn normalize_ws(s: &str) -> String {
    let lines: Vec<String> = s
        .trim_end()
        .split('\n')
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    lines.join("\n")
}

/// Same words, and per gap the same number of quote marks and the same
/// break level.
fn same_structure(a: &str, b: &str) -> bool {
    let (a, b): (Tokens, Tokens) = (guardrail::tokenize(a), guardrail::tokenize(b));
    a.words.len() == b.words.len()
        && a.words.iter().zip(&b.words).all(|(x, y)| x.norm == y.norm)
        && a.gaps.iter().zip(&b.gaps).all(|(x, y)| x.quotes() == y.quotes() && x.break_level() == y.break_level())
}

fn load_cases(args: &Args) -> Result<Vec<Case>, Box<dyn std::error::Error>> {
    if args.cases.is_empty() {
        return Ok(seed_cases());
    }
    let mut out = Vec::new();
    for path in &args.cases {
        let text = std::fs::read_to_string(path)?;
        let lines = text.lines().filter(|l| !l.trim().is_empty());
        for (n, line) in lines.take(args.limit.unwrap_or(usize::MAX)).enumerate() {
            let v: serde_json::Value = serde_json::from_str(line)?;
            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
            let style = match s("style").as_deref() {
                Some("uk") => Style { quotes: QuoteStyle::CurlySingle, punctuation: PunctuationPlacement::Logical },
                _ => Style::default(),
            };
            out.push(Case {
                id: s("id").unwrap_or_else(|| format!("{}:{}", path.display(), n + 1)),
                context: s("context").unwrap_or_default(),
                raw: s("raw").ok_or("case without raw")?,
                expected: s("expected"),
                style,
            });
        }
    }
    if let Some(n) = args.sample.filter(|&n| n > 0 && n < out.len()) {
        let step = out.len() as f64 / n as f64;
        let picks: Vec<usize> = (0..n).map(|k| (k as f64 * step) as usize).collect();
        out = out
            .into_iter()
            .enumerate()
            .filter(|(i, _)| picks.contains(i))
            .map(|(_, c)| c)
            .collect();
    }
    Ok(out)
}

/// The four cases of the 2026-10-14 seed bench, with reference answers.
fn seed_cases() -> Vec<Case> {
    let case = |id: &str, context: &str, raw: &str, expected: &str| Case {
        id: id.into(),
        context: context.into(),
        raw: raw.into(),
        expected: Some(expected.into()),
        style: Style::default(),
    };
    vec![
        case(
            "seed-1",
            "",
            "the rain had not stopped for three days and um Mara was was starting to think it never would",
            "The rain had not stopped for three days, and Mara was starting to think it never would.",
        ),
        case(
            "seed-2",
            "The rain had not stopped for three days, and Mara was starting to think it never would.",
            "where are you going she asked uh he didn't look up from the the map I told you he said to the river",
            "\n\n“Where are you going?” she asked.\n\nHe didn't look up from the map. “I told you,” he said. “To the river.”",
        ),
        case(
            "seed-3",
            "“Where are you going?” she asked.\n\nHe didn't look up from the map. “I told you,” he said. “To the river.”",
            "the river's flooded you know that it's been flooded since Tuesday so what I'll swim he folded the map and and stood up",
            "\n\n“The river's flooded. You know that. It's been flooded since Tuesday.”\n\n“So what? I'll swim.” He folded the map and stood up.",
        ),
        case(
            "seed-4",
            "",
            "open quote get down close quote Tom hissed new paragraph she dropped to her knees um just as the [cough] the headlights swept across the field",
            "“Get down,” Tom hissed.\n\nShe dropped to her knees just as the headlights swept across the field.",
        ),
    ]
}
