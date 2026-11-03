//! Markdown tables for the terminal and the JSON document written to disk.

use std::fmt::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::assistant::{AssistantReport, AssistantSummary};
use crate::error::{Error, Result};
use crate::meta::Meta;
use crate::metrics::{Interval, Summary};
use crate::queries::NO_MATCH_KIND;
use crate::tool_level::ToolLevelReport;

/// Everything one run produced; this is what `--out` writes.
#[derive(Debug, Serialize)]
pub struct EvalReport {
    /// What produced the numbers: versions, inputs, server and model settings.
    pub meta: Meta,
    /// MCP endpoint that was evaluated.
    pub store_url: String,
    /// Queries file that was used.
    pub queries_file: String,
    /// Seconds since the Unix epoch when the report was last written.
    pub unix_time: u64,
    /// False while the run is still going, so a report found on disk after a crash says so.
    pub complete: bool,
    /// Retrieval scores of the search tool.
    pub tool_level: ToolLevelReport,
    /// Scores of a model shopping through the tools; absent unless requested.
    pub assistant_level: Option<AssistantReport>,
}

impl EvalReport {
    /// Writes the report as pretty JSON, stamping the time of the write.
    ///
    /// # Errors
    ///
    /// Fails when the file cannot be written.
    pub fn write(&mut self, path: &Path) -> Result<()> {
        self.unix_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json + "\n")
            .map_err(|e| Error::Input(format!("cannot write {}: {e}", path.display())))
    }
}

/// `value` to two places; a zero that is negative (or rounds to one) prints as `0.00`.
fn two_places(value: f64) -> String {
    let text = format!("{value:.2}");
    if text == "-0.00" {
        "0.00".to_owned()
    } else {
        text
    }
}

fn interval(value: f64, ci: Interval) -> String {
    format!(
        "{} ({}-{})",
        two_places(value),
        two_places(ci.low),
        two_places(ci.high)
    )
}

fn tool_row(label: &str, s: &Summary) -> String {
    format!(
        "| {label} | {} | {} | {} | {} | {} | {} | {:.2} | {:.2} |\n",
        s.n,
        interval(s.hit_at_1, s.hit_at_1_ci),
        interval(s.hit_at_5, s.hit_at_5_ci),
        interval(s.recall_at_10, s.recall_at_10_ci),
        interval(s.ndcg_at_10, s.ndcg_at_10_ci),
        interval(s.mrr, s.mrr_ci),
        s.latency_p50_ms,
        s.latency_p95_ms
    )
}

/// The tool-level result as Markdown: a table per kind, the headline rows with and without the
/// filter queries, and a list of the queries that missed at rank 1. Hit intervals are 95% Wilson; recall, nDCG and MRR intervals are seeded 95% bootstrap.
#[must_use]
pub fn tool_level_markdown(report: &ToolLevelReport) -> String {
    let mut out = String::from(
        "| kind | queries | hit@1 (95% CI) | hit@5 (95% CI) | recall@10 (95% CI) | nDCG@10 (95% CI) | MRR (95% CI) | p50 ms | p95 ms |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|\n",
    );
    for (kind, summary) in &report.by_kind {
        out.push_str(&tool_row(kind, summary));
    }
    out.push_str(&tool_row("**all**", &report.overall));
    out.push_str(&tool_row(
        "**all except filter**",
        &report.overall_without_filter,
    ));
    if let Some(no_match) = &report.no_match {
        let _ = write!(
            out,
            "\n**no_match** ({} queries with no right answer, left out of every row above): {} \
             returned no results, {:.1} results on average, {} errors.\n",
            no_match.n, no_match.empty_replies, no_match.mean_results_returned, no_match.errors
        );
    }
    let misses: Vec<_> = report
        .queries
        .iter()
        .filter(|q| q.rank != Some(1) && q.kind != NO_MATCH_KIND)
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
        "| {label} | {} | {} | {:.2} | {:.2} | {} | {} | {:.1} | {} | {} |\n",
        s.n,
        interval(s.answer_accuracy, s.answer_accuracy_ci),
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
#[must_use]
pub fn assistant_markdown(report: &AssistantReport) -> String {
    let mut out = format!(
        "Model: {}{} ({})\n\n| kind | queries | committed answer correct (95% CI) | correct, API errors excluded | mentions expected | unanswered | API errors | tool calls per query | input tokens | output tokens |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n",
        report.model,
        if report.response_models.is_empty() {
            String::new()
        } else {
            format!(", answered by {}", report.response_models.join(", "))
        },
        report.temperature.map_or_else(
            || "no temperature sent".to_owned(),
            |t| format!("temperature {t}")
        )
    );
    for (kind, summary) in &report.by_kind {
        out.push_str(&assistant_row(kind, summary));
    }
    out.push_str(&assistant_row("**all**", &report.overall));
    if report.trials > 1 {
        let curve: Vec<String> = report
            .pass_at_k
            .iter()
            .map(|p| format!("pass@{} {}", p.k, two_places(p.value)))
            .collect();
        let _ = write!(
            out,
            "\n{} trials per query; the interval above treats runs as independent. {}\n",
            report.trials,
            curve.join(", ")
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_negative_zero_prints_as_plain_zero() {
        assert_eq!(two_places(-0.0), "0.00");
        assert_eq!(two_places(-0.001), "0.00");
        assert_eq!(two_places(-0.5), "-0.50");
        assert_eq!(two_places(0.456), "0.46");
    }
}
