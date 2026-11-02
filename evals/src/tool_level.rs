//! Tool-level eval: call the store's search tool for every query and score the ranking.

use std::collections::BTreeMap;
use std::time::Instant;

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::error::Result;
use crate::mcp::{McpClient, ToolOutput};
use crate::metrics::{Summary, rank_of, summarize};
use crate::queries::Query;

/// Keys under which a search result list is looked up, in order.
const LIST_KEYS: [&str; 4] = ["results", "products", "items", "hits"];
/// Keys that hold a product's identifier, in order.
const ID_KEYS: [&str; 4] = ["id", "sku", "part_number", "product_id"];

/// How the search tool is called.
#[derive(Debug, Clone)]
pub struct Options {
    /// Name of the store's search tool.
    pub search_tool: String,
    /// How many results to ask for (sent as `limit` unless the query sets its own).
    pub top_k: usize,
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
    /// Part ids the store returned, best first.
    pub returned: Vec<String>,
    /// 1-based rank of the best expected id among the results, if any.
    pub rank: Option<usize>,
    /// Round-trip time of the search call in milliseconds.
    pub latency_ms: f64,
    /// Why the call failed, when it did.
    pub error: Option<String>,
}

/// Scores for all queries, per kind and overall, with the per-query detail.
#[derive(Debug, Clone, Serialize)]
pub struct ToolLevelReport {
    /// Scores over every query.
    pub overall: Summary,
    /// Scores per query kind.
    pub by_kind: BTreeMap<String, Summary>,
    /// One entry per query.
    pub queries: Vec<QueryResult>,
}

/// Ids of the products in a search reply: the first list under `results`, `products`, `items` or
/// `hits`, read from `structuredContent` or else from the text block, taking each entry's `id`,
/// `sku`, `part_number` or `product_id`.
pub fn result_ids(output: &ToolOutput) -> Vec<String> {
    let parsed;
    let body = match &output.structured {
        Some(value) => value,
        None => {
            parsed = serde_json::from_str::<Value>(&output.text).unwrap_or(Value::Null);
            &parsed
        }
    };
    let Some(list) = LIST_KEYS.iter().find_map(|key| body.get(key)?.as_array()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|entry| ID_KEYS.iter().find_map(|key| entry.get(key)?.as_str()))
        .map(str::to_owned)
        .collect()
}

fn arguments(query: &Query, top_k: usize) -> Value {
    let mut args = match &query.search {
        Some(Value::Object(map)) => map.clone(),
        _ => Map::from_iter([("query".to_owned(), json!(query.text))]),
    };
    args.entry("limit").or_insert(json!(top_k));
    Value::Object(args)
}

async fn run_one(mcp: &mut McpClient, query: &Query, options: &Options) -> QueryResult {
    let started = Instant::now();
    let outcome = mcp
        .call_tool(&options.search_tool, arguments(query, options.top_k))
        .await;
    let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
    let (returned, error) = match outcome {
        Ok(output) if output.is_error => (Vec::new(), Some(output.text)),
        Ok(output) => (result_ids(&output), None),
        Err(e) => (Vec::new(), Some(e.to_string())),
    };
    QueryResult {
        id: query.id.clone(),
        kind: query.kind.clone(),
        text: query.text.clone(),
        expected: query.expected.clone(),
        rank: rank_of(&returned, &query.expected),
        returned,
        latency_ms,
        error,
    }
}

/// Runs every query once, in file order.
pub async fn run(
    mcp: &mut McpClient,
    queries: &[Query],
    options: &Options,
) -> Result<ToolLevelReport> {
    let mut results = Vec::with_capacity(queries.len());
    for query in queries {
        results.push(run_one(mcp, query, options).await);
    }
    let mut kinds: BTreeMap<String, Vec<QueryResult>> = BTreeMap::new();
    for result in &results {
        kinds
            .entry(result.kind.clone())
            .or_default()
            .push(result.clone());
    }
    Ok(ToolLevelReport {
        overall: summarize(&results),
        by_kind: kinds
            .iter()
            .map(|(kind, rs)| (kind.clone(), summarize(rs)))
            .collect(),
        queries: results,
    })
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

    #[test]
    fn ids_come_from_structured_content_or_the_text_block() {
        let structured = json!({ "results": [{ "id": "A-1" }, { "id": "B-2" }] });
        assert_eq!(result_ids(&output(Some(structured), "")), ["A-1", "B-2"]);
        let text = r#"{"products":[{"sku":"X9"},{"id":"Y8"},{"name":"no id"}]}"#;
        assert_eq!(result_ids(&output(None, text)), ["X9", "Y8"]);
        assert!(result_ids(&output(None, "not json")).is_empty());
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
