//! Markdown tables for the terminal and the JSON document written to disk.

use std::fmt::Write;

use serde::Serialize;

use crate::assistant::{AssistantReport, AssistantSummary};
use crate::metrics::{Interval, Summary};
use crate::tool_level::ToolLevelReport;

/// Everything one run produced; this is what `--out` writes.
#[derive(Debug, Serialize)]
pub struct EvalReport {
    /// MCP endpoint that was evaluated.
    pub store_url: String,
    /// Queries file that was used.
    pub queries_file: String,
    /// Seconds since the Unix epoch when the run finished.
    pub unix_time: u64,
    /// Retrieval scores of the search tool.
    pub tool_level: ToolLevelReport,
    /// Scores of a model shopping through the tools; absent unless requested.
    pub assistant_level: Option<AssistantReport>,
}

fn interval(value: f64, ci: Interval) -> String {
    format!("{value:.2} ({:.2}-{:.2})", ci.low, ci.high)
}

fn tool_row(label: &str, s: &Summary) -> String {
    format!(
        "| {label} | {} | {} | {} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} |\n",
        s.n,
        interval(s.hit_at_1, s.hit_at_1_ci),
        interval(s.hit_at_5, s.hit_at_5_ci),
        s.recall_at_10,
        s.ndcg_at_10,
        s.mrr,
        s.latency_p50_ms,
        s.latency_p95_ms
    )
}

/// The tool-level result as Markdown: a table per kind, the headline rows with and without the
/// filter queries, and a list of the queries that missed at rank 1. Intervals are 95% Wilson.
pub fn tool_level_markdown(report: &ToolLevelReport) -> String {
    let mut out = String::from(
        "| kind | queries | hit@1 (95% CI) | hit@5 (95% CI) | recall@10 | nDCG@10 | MRR | p50 ms | p95 ms |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|\n",
    );
    for (kind, summary) in &report.by_kind {
        out.push_str(&tool_row(kind, summary));
    }
    out.push_str(&tool_row("**all**", &report.overall));
    out.push_str(&tool_row(
        "**all except filter**",
        &report.overall_without_filter,
    ));
    let misses: Vec<_> = report
        .queries
        .iter()
        .filter(|q| q.rank != Some(1))
        .collect();
    if !misses.is_empty() {
        out.push_str("\nNot ranked first:\n\n| id | query | rank | expected | top results |\n|---|---|---:|---|---|\n");
        for q in misses {
            let rank = q.rank.map_or_else(|| "none".to_owned(), |r| r.to_string());
            let expected = q
                .expected
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            let top = match &q.error {
                Some(error) => format!("error: {error}"),
                None => q
                    .returned
                    .iter()
                    .take(3)
                    .map(|id| id.as_deref().unwrap_or("(no id)"))
                    .collect::<Vec<_>>()
                    .join(", "),
            };
            let _ = writeln!(
                out,
                "| {} | {} | {rank} | {expected} | {top} |",
                q.id, q.text
            );
        }
    }
    out
}

fn assistant_row(label: &str, s: &AssistantSummary) -> String {
    format!(
        "| {label} | {} | {:.2} | {:.2} | {:.2} | {} | {} | {:.1} | {} | {} |\n",
        s.n,
        s.answer_accuracy,
        s.answer_accuracy_without_errors,
        s.mentions_expected,
        s.unanswered,
        s.errors,
        s.mean_tool_calls,
        s.input_tokens,
        s.output_tokens
    )
}

/// The assistant-level result as a Markdown table.
pub fn assistant_markdown(report: &AssistantReport) -> String {
    let mut out = format!(
        "Model: {} (temperature {})\n\n| kind | queries | committed answer correct | correct, API errors excluded | mentions expected | unanswered | API errors | tool calls per query | input tokens | output tokens |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n",
        report.model, report.temperature
    );
    for (kind, summary) in &report.by_kind {
        out.push_str(&assistant_row(kind, summary));
    }
    out.push_str(&assistant_row("**all**", &report.overall));
    out
}
