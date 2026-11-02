//! `mcp-commerce-evals`: score an MCP store's search tool, and optionally a model shopping on it.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, ValueEnum};
use mcp_commerce_evals::assistant::{self, Anthropic, ApiKey};
use mcp_commerce_evals::error::{Error, Result};
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
    /// Most model turns per query in the assistant-level run.
    #[arg(long, default_value_t = 8)]
    max_turns: u32,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Args::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mcp-commerce-evals: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<()> {
    let queries = queries::load(&args.queries)?;
    // Read the key before any network traffic so a missing key fails fast.
    let api = match args.assistant {
        Some(Provider::Anthropic) => {
            let key = std::env::var("ANTHROPIC_API_KEY")
                .map_err(|_| Error::Input("ANTHROPIC_API_KEY is not set".to_owned()))?;
            Some(Anthropic::new(
                ApiKey::new(key),
                &args.model,
                assistant::DEFAULT_BASE_URL,
            ))
        }
        None => None,
    };
    let mut mcp = McpClient::connect(&args.url).await?;
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
    Ok(())
}
