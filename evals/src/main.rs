//! `mcp-commerce-evals`: score an MCP store's search tool, and optionally a model shopping on it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, ValueEnum};
use mcp_commerce_evals::assistant::{self, Anthropic, ApiKey, Plan};
use mcp_commerce_evals::error::{Error, Result};
use mcp_commerce_evals::gate::{self, Gates};
use mcp_commerce_evals::mcp::McpClient;
use mcp_commerce_evals::meta::{AssistantMeta, Meta, RunSettings};
use mcp_commerce_evals::report::{EvalReport, assistant_markdown, tool_level_markdown};
use mcp_commerce_evals::{queries, tool_level};

/// Which model API drives the assistant-level run.
#[derive(Clone, Copy, ValueEnum)]
enum Provider {
    /// The Anthropic Messages API; the key is read from `ANTHROPIC_API_KEY`.
    Anthropic,
}

/// Evaluate an MCP store: tool-level retrieval metrics, plus an optional assistant-level run.
///
/// Exit status: 0 on success, 1 when the run could not complete, 2 when any query errored, 3 when
/// a --min-* gate was not met (the report is written first). A run whose report is incomplete
/// is a failure to complete, so it exits 1.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// MCP endpoint of the store (Streamable HTTP).
    #[arg(long, default_value = "http://127.0.0.1:8787/mcp")]
    url: String,
    /// Queries file: a JSON array of {id, kind, text, expected, search?}. Defaults to
    /// evals/queries.json under the current directory, else the copy in the source tree.
    #[arg(long)]
    queries: Option<PathBuf>,
    /// Where to write the JSON report.
    #[arg(long, default_value = "eval-results.json")]
    out: PathBuf,
    /// Name of the store's search tool.
    #[arg(long, default_value = "search_products")]
    search_tool: String,
    /// How many results to ask the search tool for (sent as `limit`).
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=100))]
    top_k: u32,
    /// Also run the assistant-level eval with this provider (costs API tokens).
    #[arg(long, value_enum)]
    assistant: Option<Provider>,
    /// Model for the assistant-level run.
    #[arg(long, default_value = assistant::DEFAULT_MODEL)]
    model: String,
    /// Exit with status 3 when hit@5 over all queries is below this (0 to 1).
    #[arg(long, value_parser = gate::parse_threshold)]
    min_hit5: Option<f64>,
    /// Exit with status 3 when MRR over all queries is below this (0 to 1).
    #[arg(long, value_parser = gate::parse_threshold)]
    min_mrr: Option<f64>,
    /// Exit with status 3 when the assistant-level share of correct committed answers is below
    /// this (0 to 1); needs --assistant.
    #[arg(long, value_parser = gate::parse_threshold, requires = "assistant")]
    min_accuracy: Option<f64>,
    /// Run only queries of this kind (repeatable); all kinds when omitted.
    #[arg(long = "kind")]
    kinds: Vec<String>,
    /// Replace the report file if it already exists.
    #[arg(long)]
    force: bool,
    /// Timed search calls per query, for latency; one untimed warm-up call precedes them.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..))]
    repeat: u64,
    /// Seconds before an HTTP request to the store or the model API gives up.
    #[arg(long, default_value_t = 60)]
    timeout_secs: u64,
    /// Sampling temperature (0 to 1) for the assistant-level run; sent only when given, because
    /// some models reject the parameter.
    #[arg(long, value_parser = parse_temperature)]
    temperature: Option<f64>,
    /// Most model turns per query in the assistant-level run.
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u32).range(1..=50))]
    max_turns: u32,
    /// Runs of each query in the assistant-level run, for the mean accuracy and pass@k.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=50),
          requires = "assistant")]
    trials: u32,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Args::parse()).await {
        Ok(status) => ExitCode::from(status),
        Err(error) => {
            eprintln!("mcp-commerce-evals: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Parses `--temperature`: a number from 0 to 1.
fn parse_temperature(text: &str) -> std::result::Result<f64, String> {
    match text.parse::<f64>() {
        Ok(value) if (0.0..=1.0).contains(&value) => Ok(value),
        _ => Err(format!("{text:?} is not a number from 0 to 1")),
    }
}

/// The Anthropic client for `--assistant`, built before any traffic so a bad key fails fast.
fn anthropic_client(args: &Args, timeout: Duration) -> Result<Option<Anthropic>> {
    match args.assistant {
        Some(Provider::Anthropic) => {
            let key = std::env::var("ANTHROPIC_API_KEY")
                .map_err(|_| Error::Input("ANTHROPIC_API_KEY is not set".to_owned()))?;
            // The override exists so the tests can point the binary at a mock API.
            let base_url = std::env::var("ANTHROPIC_BASE_URL")
                .unwrap_or_else(|_| assistant::DEFAULT_BASE_URL.to_owned());
            Anthropic::new(ApiKey::new(key)?, &args.model, &base_url, timeout)
                .map(|api| Some(api.with_temperature(args.temperature)))
        }
        None => Ok(None),
    }
}

async fn run(args: Args) -> Result<u8> {
    refuse_to_overwrite(&args.out, args.force)?;
    let timeout = Duration::from_secs(args.timeout_secs);
    let queries_path = args.queries.clone().unwrap_or_else(queries::default_path);
    let raw_queries = queries::read_file(&queries_path)?;
    let queries = queries::select_kinds(queries::parse(&raw_queries)?, &args.kinds)?;
    let api = anthropic_client(&args, timeout)?;
    let mut mcp = McpClient::connect(&args.url, timeout).await?;
    let repeat = usize::try_from(args.repeat).unwrap_or(usize::MAX);
    let top_k = args.top_k as usize;
    let options = tool_level::Options {
        search_tool: args.search_tool.clone(),
        top_k,
        repeat,
    };
    let tool_level = tool_level::run(&mut mcp, &queries, &options).await;
    println!("Tool-level ({} queries, {})\n", queries.len(), args.url);
    println!("{}", tool_level_markdown(&tool_level));

    let settings = RunSettings {
        queries_file: raw_queries.as_bytes(),
        top_k,
        repeat,
        search_tool: &args.search_tool,
        timeout_secs: args.timeout_secs,
        kinds: &args.kinds,
    };
    let plan = Plan {
        max_turns: args.max_turns,
        trials: args.trials,
    };
    let assistant_meta = api
        .as_ref()
        .map(|_| AssistantMeta::new(&args.model, args.temperature, plan));
    let mut report = EvalReport {
        meta: Meta::new(&settings, mcp.handshake(), assistant_meta),
        store_url: args.url.clone(),
        queries_file: queries_path.display().to_string(),
        unix_time: 0,
        complete: false,
        tool_level,
        assistant_level: None,
    };
    // Written before the paid part starts, and again after every query, so a failure midway
    // leaves the partial results on disk.
    report.write(&args.out)?;
    if let Some(api) = &api {
        let out = args.out.clone();
        let assistant_level = assistant::run(api, &mut mcp, &queries, plan, |partial| {
            report.assistant_level = Some(partial.clone());
            report.write(&out)
        })
        .await?;
        println!(
            "Assistant-level\n\n{}",
            assistant_markdown(&assistant_level)
        );
        report.assistant_level = Some(assistant_level);
    }
    check_complete(&report, queries.len(), plan)?;
    report.complete = true;
    report.write(&args.out)?;
    mcp.close().await;
    println!("\nJSON written to {}", args.out.display());
    Ok(exit_status_of(&report, &args))
}

/// Refuses to call a report complete when a part of it is missing: one tool-level entry per
/// query, and one assistant-level entry per query and trial.
fn check_complete(report: &EvalReport, queries: usize, plan: Plan) -> Result<()> {
    let assistant_runs = report.assistant_level.as_ref().map(|a| a.queries.len());
    let wanted_runs = queries * plan.trials as usize;
    if report.tool_level.queries.len() != queries
        || assistant_runs.is_some_and(|runs| runs != wanted_runs)
    {
        return Err(Error::Protocol(
            "the report is incomplete: some queries have no result; it is saved with \
             \"complete\": false"
                .to_owned(),
        ));
    }
    Ok(())
}

/// Prints every failed gate and error count, and returns the exit status for them.
fn exit_status_of(report: &EvalReport, args: &Args) -> u8 {
    let gates = Gates {
        min_hit5: args.min_hit5,
        min_mrr: args.min_mrr,
        min_accuracy: args.min_accuracy,
    };
    let accuracy = report
        .assistant_level
        .as_ref()
        .map(|assistant| assistant.overall.answer_accuracy);
    let failures = gates.failures(&report.tool_level.overall, accuracy);
    for failure in &failures {
        eprintln!("mcp-commerce-evals: {failure}");
    }
    let query_errors = report.tool_level.error_count()
        + report
            .assistant_level
            .as_ref()
            .map_or(0, |assistant| assistant.overall.errors);
    if query_errors > 0 {
        eprintln!("mcp-commerce-evals: {query_errors} queries errored");
    }
    gate::exit_status(query_errors, &failures)
}

/// Fails before any traffic when the report file exists and `--force` was not given.
fn refuse_to_overwrite(out: &Path, force: bool) -> Result<()> {
    if out.exists() && !force {
        return Err(Error::Input(format!(
            "{} exists; pass --force to replace it",
            out.display()
        )));
    }
    Ok(())
}
