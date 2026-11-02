//! Assistant-level eval: let a model shop through the store's tools and check its answer.
//!
//! The loop uses the Anthropic Messages API with tool use. The store's MCP tools are passed to
//! the model as tool definitions, and each tool call the model makes is forwarded to the store.

use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::mcp::McpClient;
use crate::metrics::normalize;
use crate::queries::Query;

/// Production endpoint of the API.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
/// Model used when none is given.
pub const DEFAULT_MODEL: &str = "claude-haiku-5-5";
const API_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 1024;
const MAX_ERROR_CHARS: usize = 200;
const SYSTEM_PROMPT: &str = "You are a shopping assistant for an online bicycle-parts store. \
Use the store's tools to find parts; never guess a part number. When you have chosen, reply with \
the part number of the single best match and one sentence on why it fits. You cannot pay: if the \
buyer wants to buy, create a cart and give them the checkout link.";

/// An API key that never shows up in `Debug` output.
pub struct ApiKey(String);

impl ApiKey {
    /// Wraps a key read from the environment.
    pub fn new(key: String) -> Self {
        Self(key)
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

/// A client for the Messages API.
#[derive(Debug)]
pub struct Anthropic {
    http: reqwest::Client,
    base_url: String,
    key: ApiKey,
    model: String,
}

impl Anthropic {
    /// A client that talks to `base_url` as `model`.
    pub fn new(key: ApiKey, model: &str, base_url: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_owned(),
            key,
            model: model.to_owned(),
        }
    }

    async fn create(&self, body: &Value) -> Result<Value> {
        let response = self
            .http
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.key.0)
            .header("anthropic-version", API_VERSION)
            .json(body)
            .send()
            .await?;
        let status = response.status();
        let reply: Value = response.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            return Ok(reply);
        }
        let reason = reply["error"]["message"]
            .as_str()
            .unwrap_or("no error message");
        let reason: String = reason.chars().take(MAX_ERROR_CHARS).collect();
        Err(Error::Protocol(format!(
            "the API answered HTTP {status}: {reason}"
        )))
    }
}

/// MCP tool definitions in the shape the Messages API wants.
pub fn api_tools(mcp_tools: &[Value]) -> Vec<Value> {
    mcp_tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool["name"],
                "description": tool["description"],
                "input_schema": tool["inputSchema"],
            })
        })
        .collect()
}

/// The outcome of one query.
#[derive(Debug, Clone, Serialize)]
pub struct AssistantResult {
    /// Query id.
    pub id: String,
    /// Query kind.
    pub kind: String,
    /// What the buyer said.
    pub text: String,
    /// Part ids that count as right.
    pub expected: Vec<String>,
    /// The model's final answer.
    pub final_answer: String,
    /// Whether the final answer names an expected part.
    pub named_expected: bool,
    /// Tool calls the model made.
    pub tool_calls: u32,
    /// Which tools, in order.
    pub tools_used: Vec<String>,
    /// Model turns taken.
    pub turns: u32,
    /// Input tokens over all turns.
    pub input_tokens: u64,
    /// Output tokens over all turns.
    pub output_tokens: u64,
    /// Why the run failed, when it did.
    pub error: Option<String>,
}

/// Whether `answer` contains one of the expected part ids, ignoring case and punctuation.
pub fn names_expected(answer: &str, expected: &[String]) -> bool {
    let answer = normalize(answer);
    expected.iter().any(|id| answer.contains(&normalize(id)))
}

fn text_of(content: &[Value]) -> String {
    content
        .iter()
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

async fn converse(
    api: &Anthropic,
    mcp: &mut McpClient,
    tools: &[Value],
    query: &Query,
    max_turns: u32,
    result: &mut AssistantResult,
) -> Result<()> {
    let mut messages = vec![json!({ "role": "user", "content": query.text })];
    for _ in 0..max_turns {
        let body = json!({
            "model": api.model, "max_tokens": MAX_TOKENS, "system": SYSTEM_PROMPT,
            "tools": tools, "messages": messages,
        });
        let reply = api.create(&body).await?;
        result.turns += 1;
        result.input_tokens += reply["usage"]["input_tokens"].as_u64().unwrap_or(0);
        result.output_tokens += reply["usage"]["output_tokens"].as_u64().unwrap_or(0);
        let content = reply["content"].as_array().cloned().unwrap_or_default();
        messages.push(json!({ "role": "assistant", "content": content }));
        let calls: Vec<&Value> = content.iter().filter(|b| b["type"] == "tool_use").collect();
        if calls.is_empty() {
            result.final_answer = text_of(&content);
            return Ok(());
        }
        let mut answers = Vec::with_capacity(calls.len());
        for call in calls {
            let name = call["name"].as_str().unwrap_or_default();
            result.tool_calls += 1;
            result.tools_used.push(name.to_owned());
            let (text, is_error) = match mcp.call_tool(name, call["input"].clone()).await {
                Ok(output) => (output.text, output.is_error),
                Err(e) => (e.to_string(), true),
            };
            answers.push(json!({
                "type": "tool_result", "tool_use_id": call["id"], "content": text,
                "is_error": is_error,
            }));
        }
        messages.push(json!({ "role": "user", "content": answers }));
    }
    Err(Error::Protocol(format!(
        "no final answer after {max_turns} turns"
    )))
}

/// Lets the model answer one query, then scores the answer.
pub async fn answer(
    api: &Anthropic,
    mcp: &mut McpClient,
    tools: &[Value],
    query: &Query,
    max_turns: u32,
) -> AssistantResult {
    let mut result = AssistantResult {
        id: query.id.clone(),
        kind: query.kind.clone(),
        text: query.text.clone(),
        expected: query.expected.clone(),
        final_answer: String::new(),
        named_expected: false,
        tool_calls: 0,
        tools_used: Vec::new(),
        turns: 0,
        input_tokens: 0,
        output_tokens: 0,
        error: None,
    };
    if let Err(e) = converse(api, mcp, tools, query, max_turns, &mut result).await {
        result.error = Some(e.to_string());
    }
    result.named_expected = names_expected(&result.final_answer, &query.expected);
    result
}

/// Scores over a set of assistant runs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AssistantSummary {
    /// Number of queries.
    pub n: usize,
    /// Share of queries whose final answer names an expected part.
    pub answer_accuracy: f64,
    /// Mean tool calls per query.
    pub mean_tool_calls: f64,
    /// Input tokens over all queries.
    pub input_tokens: u64,
    /// Output tokens over all queries.
    pub output_tokens: u64,
    /// Queries that ended in an error.
    pub errors: usize,
}

/// Assistant-level results, per kind and overall, with the per-query detail.
#[derive(Debug, Clone, Serialize)]
pub struct AssistantReport {
    /// Model that answered.
    pub model: String,
    /// Scores over every query.
    pub overall: AssistantSummary,
    /// Scores per query kind.
    pub by_kind: BTreeMap<String, AssistantSummary>,
    /// One entry per query.
    pub queries: Vec<AssistantResult>,
}

fn summarize(results: &[&AssistantResult]) -> AssistantSummary {
    let n = results.len();
    let per = |count: f64| if n == 0 { 0.0 } else { count / n as f64 };
    AssistantSummary {
        n,
        answer_accuracy: per(results.iter().filter(|r| r.named_expected).count() as f64),
        mean_tool_calls: per(results.iter().map(|r| f64::from(r.tool_calls)).sum()),
        input_tokens: results.iter().map(|r| r.input_tokens).sum(),
        output_tokens: results.iter().map(|r| r.output_tokens).sum(),
        errors: results.iter().filter(|r| r.error.is_some()).count(),
    }
}

/// Runs every query through the model, in file order.
pub async fn run(
    api: &Anthropic,
    mcp: &mut McpClient,
    queries: &[Query],
    max_turns: u32,
) -> Result<AssistantReport> {
    let tools = api_tools(&mcp.list_tools().await?);
    let mut results = Vec::with_capacity(queries.len());
    for query in queries {
        results.push(answer(api, mcp, &tools, query, max_turns).await);
    }
    let mut kinds: BTreeMap<String, Vec<&AssistantResult>> = BTreeMap::new();
    for result in &results {
        kinds.entry(result.kind.clone()).or_default().push(result);
    }
    Ok(AssistantReport {
        model: api.model.clone(),
        overall: summarize(&results.iter().collect::<Vec<_>>()),
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

    #[test]
    fn the_key_is_redacted_in_debug_output() {
        let api = Anthropic::new(ApiKey::new("sk-secret-value".to_owned()), "m", "http://x");
        assert!(!format!("{api:?}").contains("sk-secret-value"));
    }

    #[test]
    fn an_answer_names_a_part_in_any_spelling() {
        let expected = ["ALD-CH-1002".to_owned()];
        assert!(names_expected(
            "Get the chain ald ch 1002, it fits.",
            &expected
        ));
        assert!(names_expected("**ALD-CH-1002**", &expected));
        assert!(!names_expected("Get ALD-CH-1003 instead.", &expected));
        assert!(!names_expected("", &expected));
    }

    #[test]
    fn mcp_tools_become_api_tools() {
        let tools = api_tools(&[json!({
            "name": "search_products", "description": "d", "inputSchema": { "type": "object" },
            "outputSchema": { "type": "object" }
        })]);
        assert_eq!(
            tools,
            [json!({ "name": "search_products", "description": "d",
                     "input_schema": { "type": "object" } })]
        );
    }
}
