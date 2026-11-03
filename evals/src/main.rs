//! `mcp-commerce-evals`: score an MCP store's search tool, and optionally a model shopping on it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, ValueEnum};
use mcp_commerce_evals::assistant::{self, Anthropic, ApiKey};
use mcp_commerce_evals::error::{Error, Result};
use mcp_commerce_evals::gate::{self, Gates};
use mcp_commerce_evals::mcp::McpClient;
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
/// a --min-* gate was not met (the report is written first).
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// MCP endpoint of the store (Streamable HTTP).
    #[arg(long, default_value = "http://127.0.0.1:8787/mcp")]
    url: String,
    /// Queries file: a JSON array of {id, kind, text, expected, search?}.
    #[arg(long, default_value = "evals/queries.json")]
    queries: PathBuf,
    /// Where to write the JSON report.
    #[arg(long, default_value = "eval-results.json")]
    out: PathBuf,
    /// Name of the store's search tool.
    #[arg(long, default_value = "search_products")]
    search_tool: String,
    /// How many results to ask the search tool for (sent as `limit`).
    #[arg(long, default_value_t = 10)]
    top_k: usize,
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
    /// Replace the report file if it already exists.
    #[arg(long)]
    force: bool,
    /// Seconds before an HTTP request to the store or the model API gives up.
    #[arg(long, default_value_t = 60)]
    timeout_secs: u64,
    /// Most model turns per query in the assistant-level run.
    #[arg(long, default_value_t = 8)]
    max_turns: u32,
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

async fn run(args: Args) -> Result<u8> {
    refuse_to_overwrite(&args.out, args.force)?;
    let timeout = Duration::from_secs(args.timeout_secs);
    let queries = queries::load(&args.queries)?;
    // Read the key before any network traffic so a missing key fails fast.
    let api = match args.assistant {
        Some(Provider::Anthropic) => {
            let key = std::env::var("ANTHROPIC_API_KEY")
                .map_err(|_| Error::Input("ANTHROPIC_API_KEY is not set".to_owned()))?;
            Some(Anthropic::new(
                ApiKey::new(key)?,
                &args.model,
                assistant::DEFAULT_BASE_URL,
                timeout,
            )?)
        }
        None => None,
    };
    let mut mcp = McpClient::connect(&args.url, timeout).await?;
    let options = tool_level::Options {
        search_tool: args.search_tool,
        top_k: args.top_k,
    };
    let tool_level = tool_level::run(&mut mcp, &queries, &options).await?;
    println!("Tool-level ({} queries, {})\n", queries.len(), args.url);
    println!("{}", tool_level_markdown(&tool_level));

    let assistant_level = match &api {
        Some(api) => {
            let report = assistant::run(api, &mut mcp, &queries, args.max_turns).await?;
            println!("Assistant-level\n\n{}", assistant_markdown(&report));
            Some(report)
        }
        None => None,
    };
    let unix_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let report = EvalReport {
        store_url: args.url,
        queries_file: args.queries.display().to_string(),
        unix_time,
        tool_level,
        assistant_level,
    };
    let json = serde_json::to_string_pretty(&report)?;
    std::fs::write(&args.out, json + "\n")
        .map_err(|e| Error::Input(format!("cannot write {}: {e}", args.out.display())))?;
    println!("\nJSON written to {}", args.out.display());
    let gates = Gates {
        min_hit5: args.min_hit5,
        min_mrr: args.min_mrr,
    };
    let failures = gates.failures(&report.tool_level.overall);
    for failure in &failures {
        eprintln!("mcp-commerce-evals: {failure}");
    }
    let query_errors = report.tool_level.overall.errors;
    if query_errors > 0 {
        eprintln!("mcp-commerce-evals: {query_errors} queries errored");
    }
    Ok(gate::exit_status(query_errors, &failures))
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
