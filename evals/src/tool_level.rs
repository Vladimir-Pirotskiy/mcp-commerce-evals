//! Tool-level eval: call the store's search tool for every query and score the ranking.

use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::error::{Error, Result, ShapeProblem};
use crate::mcp::{McpClient, ToolOutput};
use crate::metrics::{Summary, normalize, rank_of, share, summarize};
use crate::queries::{NO_MATCH_KIND, Query};

/// Keys under which a search result list is looked up, in order.
const LIST_KEYS: [&str; 4] = ["results", "products", "items", "hits"];
/// Keys that hold a product's identifier, in order.
const ID_KEYS: [&str; 4] = ["id", "sku", "part_number", "product_id"];

/// The query kind whose expected sets are defined by the structured arguments.
pub const FILTER_KIND: &str = "filter";

/// How the search tool is called.
#[derive(Debug, Clone)]
pub struct Options {
    /// Name of the store's search tool.
    pub search_tool: String,
    /// How many results to ask for (sent as `limit` unless the query sets its own).
    pub top_k: usize,
    /// How many timed calls to make per query; one extra, untimed call warms the connection up
    /// before the first query.
    pub repeat: usize,
}

/// The outcome of one query.
#[derive(Debug, Clone, Serialize)]
pub struct QueryResult {
    /// Query id.
    pub id: String,
    /// Query kind.
    pub kind: String,
    /// Query text.
    pub text: String,
    /// Part ids that count as right.
    pub expected: Vec<String>,
    /// Part ids the store returned, best first, at most `top_k`; `None` marks an entry without
    /// a recognisable id.
    pub returned: Vec<Option<String>>,
    /// 1-based rank of the best expected id among the results, if any.
    pub rank: Option<usize>,
    /// Round-trip time in milliseconds of each timed call; empty when the query errored.
    pub latencies_ms: Vec<f64>,
    /// Why the call failed, when it did.
    pub error: Option<String>,
    /// True when `--repeat` made several calls and their ranked ids were not all the same, so
    /// `returned` (the first call) is one of several answers the store gives.
    #[serde(default)]
    pub nondeterministic: bool,
}

/// What the search tool did with the queries it has no answer for. Descriptive only: whether a
/// store should return nothing or a weak guess is a product choice, so nothing here is scored.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoMatchSummary {
    /// Number of `no_match` queries.
    pub n: usize,
    /// Queries whose call failed.
    pub errors: usize,
    /// Queries answered with an empty result list.
    pub empty_replies: usize,
    /// Mean number of results returned, over the queries that did not error.
    pub mean_results_returned: f64,
}

/// Scores for all queries, per kind and overall, with the per-query detail. The `no_match`
/// queries are in `queries` and in `no_match`, and in none of the other aggregates.
#[derive(Debug, Clone, Serialize)]
pub struct ToolLevelReport {
    /// Scores over every query.
    pub overall: Summary,
    /// Scores over every query except `filter` ones, which pass structured arguments that
    /// define their own expected sets and so test filtering rather than ranking.
    pub overall_without_filter: Summary,
    /// Scores per query kind.
    pub by_kind: BTreeMap<String, Summary>,
    /// The abstention queries, which have no right answer to rank; absent when there are none.
    pub no_match: Option<NoMatchSummary>,
    /// How many queries (including `no_match`) were answered differently across `--repeat`
    /// calls; always 0 with one call.
    pub nondeterministic_queries: usize,
    /// One entry per query.
    pub queries: Vec<QueryResult>,
}

impl ToolLevelReport {
    /// Queries whose search call failed, `no_match` ones included: they are in none of the
    /// ranked aggregates, but a broken call is a broken call.
    #[must_use]
    pub fn error_count(&self) -> usize {
        self.overall.errors + self.no_match.as_ref().map_or(0, |m| m.errors)
    }
}

fn no_match_summary(results: &[&QueryResult]) -> Option<NoMatchSummary> {
    if results.is_empty() {
        return None;
    }
    let answered: Vec<_> = results.iter().filter(|r| r.error.is_none()).collect();
    let total: usize = answered.iter().map(|r| r.returned.len()).sum();
    Some(NoMatchSummary {
        n: results.len(),
        errors: results.len() - answered.len(),
        empty_replies: answered.iter().filter(|r| r.returned.is_empty()).count(),
        mean_results_returned: share(total, answered.len()),
    })
}

/// The list of products in a search reply: the first array under `results`, `products`, `items`
/// or `hits`, read from `structuredContent` or else from the JSON text block.
fn product_list(output: &ToolOutput) -> Result<Vec<Value>> {
    let parsed;
    let body = if let Some(value) = &output.structured {
        value
    } else {
        parsed = serde_json::from_str::<Value>(&output.text)
            .map_err(|_| Error::Shape(ShapeProblem::NotJson))?;
        &parsed
    };
    LIST_KEYS
        .iter()
        .find_map(|key| body.get(key)?.as_array())
        .cloned()
        .ok_or(Error::Shape(ShapeProblem::NoResultList))
}

/// Ids of the first `top_k` products in a search reply, best first, so a store that ignores
/// `limit` cannot change the score. An entry with no `id`, `sku`, `part_number` or `product_id`
/// stays in the list as `None`, because it still occupies a rank. A repeated id keeps only its
/// first position. A reply that is not a recognisable product list is an error.
///
/// # Errors
///
/// Fails when the reply is not a recognisable product list.
pub fn result_ids(output: &ToolOutput, top_k: usize) -> Result<Vec<Option<String>>> {
    let mut seen = HashSet::new();
    Ok(product_list(output)?
        .iter()
        .take(top_k)
        .map(|entry| ID_KEYS.iter().find_map(|key| entry.get(key)?.as_str()))
        .filter(|id| {
            id.is_none_or(|id| {
                let key = normalize(id);
                // Ids with no ASCII letters or digits all normalize to "": keep each of them.
                key.is_empty() || seen.insert(key)
            })
        })
        .map(|id| id.map(str::to_owned))
        .collect())
}

/// The `maximum` the named tool's input schema sets on `limit`, when it sets one.
#[must_use]
pub fn limit_maximum(tools: &[Value], tool: &str) -> Option<u64> {
    tools
        .iter()
        .find(|t| t["name"] == tool)?
        .pointer("/inputSchema/properties/limit/maximum")?
        .as_u64()
}

/// Fails when `top_k` is above the `limit` the search tool advertises in `tools/list`, so a
/// run does not turn into one error per query. A store that lists no such bound, does not list
/// the tool or cannot list tools at all is not checked here: its queries report their own errors.
///
/// # Errors
///
/// Fails with a message naming both numbers when `top_k` exceeds the advertised maximum.
pub async fn check_top_k(mcp: &mut McpClient, search_tool: &str, top_k: usize) -> Result<()> {
    let Ok(tools) = mcp.list_tools().await else {
        return Ok(());
    };
    match limit_maximum(&tools, search_tool) {
        Some(max) if top_k as u64 > max => Err(Error::Input(format!(
            "--top-k {top_k} is above the maximum `limit` of {max} that {search_tool} accepts \
             (from its inputSchema); pass --top-k {max} or lower"
        ))),
        _ => Ok(()),
    }
}

fn arguments(query: &Query, top_k: usize) -> Value {
    let mut args = match &query.search {
        Some(Value::Object(map)) => map.clone(),
        _ => Map::from_iter([("query".to_owned(), json!(query.text))]),
    };
    args.entry("limit").or_insert(json!(top_k));
    Value::Object(args)
}

/// One timed search call, with its reply read as a ranking.
async fn search_ids(
    mcp: &mut McpClient,
    query: &Query,
    options: &Options,
) -> (Result<Vec<Option<String>>>, f64) {
    let started = Instant::now();
    let outcome = mcp
        .call_tool(&options.search_tool, arguments(query, options.top_k))
        .await;
    let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
    let ids = match outcome {
        Ok(output) if output.is_error => Err(Error::Protocol(output.text)),
        Ok(output) => result_ids(&output, options.top_k),
        Err(e) => Err(e),
    };
    (ids, latency_ms)
}

/// Calls the search tool `options.repeat` times for one query. The first reply is ranked; any
/// failed call makes the whole query an error, so a flaky store cannot score well.
async fn run_one(mcp: &mut McpClient, query: &Query, options: &Options) -> QueryResult {
    let mut returned = Vec::new();
    let mut nondeterministic = false;
    let mut latencies_ms = Vec::with_capacity(options.repeat);
    let mut error = None;
    for call in 0..options.repeat.max(1) {
        let (outcome, latency_ms) = search_ids(mcp, query, options).await;
        match outcome {
            Ok(ids) => {
                latencies_ms.push(latency_ms);
                if call == 0 {
                    returned = ids;
                } else if ids != returned {
                    nondeterministic = true;
                }
            }
            Err(e) => {
                error = Some(e.to_string());
                returned.clear();
                latencies_ms.clear();
                nondeterministic = false;
                break;
            }
        }
    }
    QueryResult {
        id: query.id.clone(),
        kind: query.kind.clone(),
        text: query.text.clone(),
        expected: query.expected.clone(),
        rank: rank_of(&returned, &query.expected),
        returned,
        latencies_ms,
        error,
        nondeterministic,
    }
}

/// Runs every query in file order, after one untimed warm-up call.
pub async fn run(mcp: &mut McpClient, queries: &[Query], options: &Options) -> ToolLevelReport {
    if let Some(first) = queries.first() {
        // The result is deliberately dropped: this call only opens the connection and warms
        // the store's caches, so the first measured query is not penalised.
        let _ = search_ids(mcp, first, options).await;
    }
    let mut results = Vec::with_capacity(queries.len());
    for query in queries {
        results.push(run_one(mcp, query, options).await);
    }
    let (no_match, ranked): (Vec<&QueryResult>, Vec<&QueryResult>) =
        results.iter().partition(|r| r.kind == NO_MATCH_KIND);
    let mut kinds: BTreeMap<&str, Vec<&QueryResult>> = BTreeMap::new();
    for result in &ranked {
        kinds.entry(result.kind.as_str()).or_default().push(result);
    }
    let without_filter: Vec<&QueryResult> = ranked
        .iter()
        .copied()
        .filter(|r| r.kind != FILTER_KIND)
        .collect();
    ToolLevelReport {
        overall: summarize(&ranked),
        overall_without_filter: summarize(&without_filter),
        by_kind: kinds
            .iter()
            .map(|(kind, rs)| ((*kind).to_owned(), summarize(rs)))
            .collect(),
        no_match: no_match_summary(&no_match),
        nondeterministic_queries: results.iter().filter(|r| r.nondeterministic).count(),
        queries: results,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(structured: Option<Value>, text: &str) -> ToolOutput {
        ToolOutput {
            is_error: false,
            structured,
            text: text.to_owned(),
        }
    }

    fn ids(output: &ToolOutput, top_k: usize) -> Vec<Option<String>> {
        result_ids(output, top_k).unwrap()
    }

    fn some(ids: &[&str]) -> Vec<Option<String>> {
        ids.iter().map(|id| Some((*id).to_owned())).collect()
    }

    #[test]
    fn reads_ids_from_structured_content_or_the_text_block() {
        let structured = json!({ "results": [{ "id": "A-1" }, { "id": "B-2" }] });
        assert_eq!(
            ids(&output(Some(structured), ""), 10),
            some(&["A-1", "B-2"])
        );
        let text = r#"{"products":[{"sku":"X9"},{"id":"Y8"}]}"#;
        assert_eq!(ids(&output(None, text), 10), some(&["X9", "Y8"]));
    }

    #[test]
    fn truncates_results_to_top_k() {
        let body = json!({ "results": [{ "id": "A" }, { "id": "B" }, { "id": "C" }] });
        assert_eq!(ids(&output(Some(body), ""), 2), some(&["A", "B"]));
    }

    #[test]
    fn keeps_only_the_first_position_of_a_repeated_id() {
        let body = json!({ "results": [{ "id": "A-1" }, { "id": "a1" }, { "id": "B" }] });
        assert_eq!(ids(&output(Some(body), ""), 10), some(&["A-1", "B"]));
    }

    #[test]
    fn ids_without_ascii_letters_or_digits_are_not_taken_for_repeats_of_each_other() {
        let body = json!({ "results": [{ "id": "日本" }, { "id": "東京" }, { "id": "B" }] });
        assert_eq!(
            ids(&output(Some(body), ""), 10),
            some(&["日本", "東京", "B"])
        );
    }

    #[test]
    fn an_entry_without_an_id_keeps_its_rank_position() {
        let body = json!({ "results": [{ "name": "no id" }, { "id": "B" }] });
        assert_eq!(
            ids(&output(Some(body), ""), 10),
            [None, Some("B".to_owned())]
        );
    }

    #[test]
    fn a_reply_without_a_known_list_key_is_an_error() {
        let body = json!({ "matches": [{ "id": "A" }] });
        let error = result_ids(&output(Some(body), ""), 10).unwrap_err();
        assert!(matches!(error, Error::Shape(ShapeProblem::NoResultList)));
    }

    #[test]
    fn text_that_is_not_json_is_an_error() {
        let error = result_ids(&output(None, "not json"), 10).unwrap_err();
        assert!(matches!(error, Error::Shape(ShapeProblem::NotJson)));
    }

    #[test]
    fn reads_the_limit_maximum_of_the_named_tool() {
        let tools = [
            json!({ "name": "other", "inputSchema": { "properties": { "limit": { "maximum": 3 } } } }),
            json!({ "name": "search", "inputSchema": { "properties": { "limit": { "maximum": 25 } } } }),
            json!({ "name": "bare", "inputSchema": { "properties": {} } }),
        ];
        assert_eq!(limit_maximum(&tools, "search"), Some(25));
        assert_eq!(limit_maximum(&tools, "bare"), None);
        assert_eq!(limit_maximum(&tools, "missing"), None);
    }

    #[test]
    fn arguments_default_to_the_text_and_keep_a_query_specific_limit() {
        let plain: Query = serde_json::from_value(
            json!({ "id": "a", "kind": "k", "text": "11-speed chain", "expected": ["X"] }),
        )
        .unwrap();
        assert_eq!(
            arguments(&plain, 10),
            json!({ "query": "11-speed chain", "limit": 10 })
        );
        let filtered: Query = serde_json::from_value(json!({
            "id": "b", "kind": "k", "text": "t", "expected": ["X"],
            "search": { "query": "chain", "in_stock_only": true, "limit": 3 }
        }))
        .unwrap();
        assert_eq!(
            arguments(&filtered, 10),
            json!({ "query": "chain", "in_stock_only": true, "limit": 3 })
        );
    }
}
