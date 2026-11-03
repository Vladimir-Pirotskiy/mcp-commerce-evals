//! Assistant-level eval: let a model shop through the store's tools and check its answer.
//!
//! The loop uses the Anthropic Messages API with tool use. The store's MCP tools are passed to
//! the model as tool definitions, and each tool call the model makes is forwarded to the store.

use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::mcp::McpClient;
use crate::metrics::{
    Interval, cluster_bootstrap_mean_ci, normalize, normalized_ids, pass_at_k, share, wilson,
};
use crate::queries::{NO_MATCH_KIND, Query};

/// Production endpoint of the API.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
/// Model used when none is given.
pub const DEFAULT_MODEL: &str = "claude-haiku-5-5";
/// The system prompt. It is the same for every store: only the committed `ANSWER:` line is scored.
pub const SYSTEM_PROMPT: &str = "You are a shopping assistant for an online store. \
Use the store's tools to find products; never guess a part number. When you have chosen, give \
one sentence on why the product fits, then end your reply with a final line of exactly \
`ANSWER: <part id>`, the id of the single best match as the store lists it, or `ANSWER: none` \
if nothing the store sells fits. You cannot pay: if the buyer wants to buy, create a cart and \
give them the checkout link.";
/// Prefix of the line that commits to an answer.
const ANSWER_PREFIX: &str = "ANSWER:";
/// The committed value that means the model found nothing suitable.
const ANSWER_NONE: &str = "none";
const API_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 1024;
const MAX_ERROR_CHARS: usize = 200;
/// Retries after the first attempt, for HTTP 429, 529 and 5xx, and for transport timeouts and
/// failed connects.
const MAX_RETRIES: u32 = 3;
/// First backoff step; each retry doubles it.
const DEFAULT_BACKOFF_BASE: Duration = Duration::from_millis(500);
/// Longest wait honoured, whatever `retry-after` asks for.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);
/// Overloaded: the API's own status for "try again shortly".
const STATUS_OVERLOADED: u16 = 529;

/// An API key that never shows up in `Debug` output.
pub struct ApiKey(String);

impl ApiKey {
    /// Wraps a key read from the environment; an empty or blank key is refused up front.
    ///
    /// # Errors
    ///
    /// Fails when the key is empty or blank.
    pub fn new(key: String) -> Result<Self> {
        if key.trim().is_empty() {
            return Err(Error::Input("ANTHROPIC_API_KEY is empty".to_owned()));
        }
        Ok(Self(key))
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
    temperature: Option<f64>,
    backoff_base: Duration,
}

impl Anthropic {
    /// A client that talks to `base_url` as `model`; each request gives up after `timeout`.
    ///
    /// # Errors
    ///
    /// Fails when the HTTP client cannot be built.
    pub fn new(key: ApiKey, model: &str, base_url: &str, timeout: Duration) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder().timeout(timeout).build()?,
            base_url: base_url.trim_end_matches('/').to_owned(),
            key,
            model: model.to_owned(),
            temperature: None,
            backoff_base: DEFAULT_BACKOFF_BASE,
        })
    }

    /// Sends `temperature` with every request; without this none is sent.
    #[must_use]
    pub fn with_temperature(mut self, temperature: Option<f64>) -> Self {
        self.temperature = temperature;
        self
    }

    /// Replaces the first backoff step, so tests need not wait for real delays.
    #[must_use]
    pub fn with_backoff_base(mut self, base: Duration) -> Self {
        self.backoff_base = base;
        self
    }

    /// The model this client asks.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Sends one Messages request, retrying 429, 529, 5xx, timeouts and failed connects up to
    /// [`MAX_RETRIES`] times.
    async fn create(&self, body: &Value) -> Result<Value> {
        let mut attempt = 0;
        loop {
            let sent = self
                .http
                .post(format!("{}/v1/messages", self.base_url))
                .header("x-api-key", &self.key.0)
                .header("anthropic-version", API_VERSION)
                .json(body)
                .send()
                .await;
            let response = match sent {
                Ok(response) => response,
                // A timeout or a failed connect is as transient as a 529: wait and ask again.
                Err(e) if (e.is_timeout() || e.is_connect()) && attempt < MAX_RETRIES => {
                    tokio::time::sleep(self.backoff(attempt)).await;
                    attempt += 1;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let status = response.status();
            let retry_after = retry_after(response.headers());
            let reply: Value = response.json().await.unwrap_or(Value::Null);
            if status.is_success() {
                return Ok(reply);
            }
            if is_retryable(status.as_u16()) && attempt < MAX_RETRIES {
                tokio::time::sleep(retry_after.unwrap_or_else(|| self.backoff(attempt))).await;
                attempt += 1;
                continue;
            }
            return Err(Error::Api {
                status: status.as_u16(),
                message: self.redact(reply["error"]["message"].as_str()),
            });
        }
    }

    /// Exponential backoff with jitter: half the step is fixed, the other half is random.
    fn backoff(&self, attempt: u32) -> Duration {
        let step = self
            .backoff_base
            .saturating_mul(1 << attempt)
            .min(MAX_RETRY_DELAY);
        let half = step / 2;
        let jitter_nanos = u64::from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos()),
        );
        let span = u64::try_from(half.as_nanos())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        half + Duration::from_nanos(jitter_nanos % span)
    }

    /// An API error message, shortened, with the key blanked in case the API echoed it.
    fn redact(&self, message: Option<&str>) -> String {
        message
            .unwrap_or("no error message")
            .replace(&self.key.0, "[redacted]")
            .chars()
            .take(MAX_ERROR_CHARS)
            .collect()
    }
}

/// Whether the API asks the caller to try again: rate limited, overloaded or a server fault.
fn is_retryable(status: u16) -> bool {
    status == 429 || status == STATUS_OVERLOADED || (500..600).contains(&status)
}

/// The wait a `retry-after` header (whole seconds) asks for, capped at [`MAX_RETRY_DELAY`].
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let seconds: u64 = headers
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(seconds).min(MAX_RETRY_DELAY))
}

/// MCP tool definitions in the shape the Messages API wants.
#[must_use]
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
    /// The model's final reply, or its last partial reply when it did not finish.
    pub final_answer: String,
    /// What the `ANSWER:` line committed to (`none` included); absent when there was no line.
    pub committed: Option<String>,
    /// Whether the committed id equals an expected id, ignoring case and punctuation.
    pub correct: bool,
    /// Diagnostic only: whether the reply mentions an expected id anywhere.
    pub mentions_expected: bool,
    /// Whether the model ended its turn (`end_turn`) with a final reply.
    pub answered: bool,
    /// The `stop_reason` of the last reply received.
    pub stop_reason: Option<String>,
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
    /// Why the run failed (API or transport), when it did.
    pub error: Option<String>,
    /// Which repetition of the query this is, from 1 (`--trials`).
    pub trial: u32,
    /// The distinct `model` ids the API named in its replies, in order of first appearance. The
    /// request names an alias; this is what actually answered.
    pub models: Vec<String>,
}

/// Whether `answer` contains one of the expected part ids, ignoring case and punctuation.
#[must_use]
pub fn names_expected(answer: &str, expected: &[String]) -> bool {
    let answer = normalize(answer);
    normalized_ids(expected)
        .iter()
        .any(|id| answer.contains(id.as_str()))
}

/// What the last non-empty line of a reply commits to, if it is an `ANSWER:` line. Markdown
/// emphasis around the line is ignored.
#[must_use]
pub fn committed_answer(reply: &str) -> Option<String> {
    let line = reply.lines().rev().find(|line| !line.trim().is_empty())?;
    let line = line.trim_matches(|c: char| c.is_whitespace() || c == '*' || c == '`');
    let value = line.strip_prefix(ANSWER_PREFIX)?;
    let value = value.trim_matches(|c: char| c.is_whitespace() || c == '*' || c == '`');
    (!value.is_empty()).then(|| value.to_owned())
}

/// Whether a committed answer is exactly one of the expected ids (after normalising both). A
/// query with nothing expected (kind `no_match`) is answered correctly only by `none`.
#[must_use]
pub fn is_correct(committed: Option<&str>, expected: &[String]) -> bool {
    committed.is_some_and(|value| {
        let said_none = normalize(value) == ANSWER_NONE;
        if expected.is_empty() {
            said_none
        } else {
            !said_none && normalized_ids(expected).contains(&normalize(value))
        }
    })
}

fn text_of(content: &[Value]) -> String {
    content
        .iter()
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// What a reply's `stop_reason` means for the loop.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    /// `end_turn`: the reply is the final answer.
    Answer,
    /// `tool_use`: run the requested tools and ask again.
    RunTools,
    /// `max_tokens`, `refusal`, `pause_turn`, a missing or unknown reason: no answer.
    GiveUp,
}

fn next_step(stop_reason: Option<&str>) -> Next {
    match stop_reason {
        Some("end_turn") => Next::Answer,
        Some("tool_use") => Next::RunTools,
        _ => Next::GiveUp,
    }
}

/// The text handed to the model for a tool result. A tool that answers only with
/// `structuredContent` and no text would otherwise give the model an empty result.
fn tool_result_text(output: &crate::mcp::ToolOutput) -> String {
    match &output.structured {
        Some(structured) if output.text.trim().is_empty() => structured.to_string(),
        _ => output.text.clone(),
    }
}

async fn run_tools(mcp: &mut McpClient, calls: &[&Value], result: &mut AssistantResult) -> Value {
    let mut answers = Vec::with_capacity(calls.len());
    for call in calls {
        let name = call["name"].as_str().unwrap_or_default();
        result.tool_calls += 1;
        result.tools_used.push(name.to_owned());
        let (text, is_error) = match mcp.call_tool(name, call["input"].clone()).await {
            Ok(output) => (tool_result_text(&output), output.is_error),
            Err(e) => (e.to_string(), true),
        };
        answers.push(json!({
            "type": "tool_result", "tool_use_id": call["id"], "content": text,
            "is_error": is_error,
        }));
    }
    Value::Array(answers)
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
        let mut body = json!({
            "model": api.model, "max_tokens": MAX_TOKENS,
            "system": SYSTEM_PROMPT, "tools": tools, "messages": messages,
        });
        // Sent only when asked for: some current models reject the parameter outright.
        if let (Some(temperature), Some(fields)) = (api.temperature, body.as_object_mut()) {
            fields.insert("temperature".to_owned(), json!(temperature));
        }
        let reply = api.create(&body).await?;
        result.turns += 1;
        if let Some(model) = reply["model"].as_str()
            && !result.models.iter().any(|seen| seen == model)
        {
            result.models.push(model.to_owned());
        }
        result.input_tokens += reply["usage"]["input_tokens"].as_u64().unwrap_or(0);
        result.output_tokens += reply["usage"]["output_tokens"].as_u64().unwrap_or(0);
        result.stop_reason = reply["stop_reason"].as_str().map(str::to_owned);
        let content = reply["content"].as_array().cloned().unwrap_or_default();
        result.final_answer = text_of(&content);
        messages.push(json!({ "role": "assistant", "content": content }));
        match next_step(result.stop_reason.as_deref()) {
            Next::Answer => {
                result.answered = true;
                return Ok(());
            }
            Next::GiveUp => return Ok(()),
            Next::RunTools => {}
        }
        let calls: Vec<&Value> = content.iter().filter(|b| b["type"] == "tool_use").collect();
        if calls.is_empty() {
            return Ok(());
        }
        let answers = run_tools(mcp, &calls, result).await;
        messages.push(json!({ "role": "user", "content": answers }));
    }
    Ok(())
}

/// Lets the model answer one query, then scores the committed answer.
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
        committed: None,
        correct: false,
        mentions_expected: false,
        answered: false,
        stop_reason: None,
        tool_calls: 0,
        tools_used: Vec::new(),
        turns: 0,
        input_tokens: 0,
        output_tokens: 0,
        error: None,
        trial: 1,
        models: Vec::new(),
    };
    if let Err(e) = converse(api, mcp, tools, query, max_turns, &mut result).await {
        result.error = Some(e.to_string());
    }
    result.mentions_expected = names_expected(&result.final_answer, &query.expected);
    if result.answered {
        result.committed = committed_answer(&result.final_answer);
        result.correct = is_correct(result.committed.as_deref(), &query.expected);
    }
    result
}

/// Scores over a set of assistant runs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AssistantSummary {
    /// Number of queries.
    pub n: usize,
    /// Share of all runs whose committed answer is right (an expected id, or `none` for a
    /// `no_match` query); an API error is a miss.
    pub answer_accuracy: f64,
    /// 95% interval of `answer_accuracy`. With one run per query it is the Wilson interval. With
    /// `--trials` above 1 the runs of a query are not independent, so it is a seeded bootstrap
    /// that resamples whole queries and keeps each query's trials together; null (`n/a`) under
    /// [`crate::metrics::MIN_BOOTSTRAP_N`] queries or when every query scored the same.
    pub answer_accuracy_ci: Option<Interval>,
    /// The same share over the queries that had no API error.
    pub answer_accuracy_without_errors: f64,
    /// Diagnostic: share of queries whose reply mentions an expected id anywhere. Null (not
    /// applicable) for a `no_match` query, which has no expected id, and over the queries of
    /// other kinds in a mixed set; null when there are none.
    pub mentions_expected: Option<f64>,
    /// Queries that ended without a final answer (a stop reason other than `end_turn`, or the
    /// turn limit) and had no API error.
    pub unanswered: usize,
    /// Queries that ended in an API or transport error.
    pub errors: usize,
    /// Mean tool calls per query.
    pub mean_tool_calls: f64,
    /// Input tokens over all queries.
    pub input_tokens: u64,
    /// Output tokens over all queries.
    pub output_tokens: u64,
}

/// The chance that at least one of `k` runs of a query is right, averaged over queries.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PassAtK {
    /// Number of runs allowed.
    pub k: u32,
    /// Unbiased pass@k estimate, from the runs of each query.
    pub value: f64,
}

/// How many times each query runs, and how long a run may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// Most model turns per run.
    pub max_turns: u32,
    /// Runs per query.
    pub trials: u32,
}

impl Plan {
    /// One run per query.
    #[must_use]
    pub fn once(max_turns: u32) -> Self {
        Self {
            max_turns,
            trials: 1,
        }
    }
}

/// Assistant-level results, per kind and overall, with the per-run detail.
#[derive(Debug, Clone, Serialize)]
pub struct AssistantReport {
    /// Model that was asked for.
    pub model: String,
    /// Distinct `model` ids the API named in its replies; what really answered.
    pub response_models: Vec<String>,
    /// Runs per query.
    pub trials: u32,
    /// pass@k for k from 1 to `trials`, overall.
    pub pass_at_k: Vec<PassAtK>,
    /// Sampling temperature that was sent; null when none was.
    pub temperature: Option<f64>,
    /// Scores over every query.
    pub overall: AssistantSummary,
    /// Scores per query kind.
    pub by_kind: BTreeMap<String, AssistantSummary>,
    /// One entry per run: each query once per trial.
    pub queries: Vec<AssistantResult>,
}

/// pass@k for k in `1..=trials`, averaged over the queries that have at least k runs.
fn pass_curve(results: &[AssistantResult], trials: u32) -> Vec<PassAtK> {
    let mut runs: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for result in results {
        let entry = runs.entry(result.id.as_str()).or_default();
        entry.0 += 1;
        entry.1 += usize::from(result.correct);
    }
    (1..=trials)
        .map(|k| {
            let scores: Vec<f64> = runs
                .values()
                .filter(|(n, _)| *n >= k as usize)
                .map(|(n, correct)| pass_at_k(*n, *correct, k as usize))
                .collect();
            let value = if scores.is_empty() {
                0.0
            } else {
                scores.iter().sum::<f64>() / crate::metrics::to_f64(scores.len())
            };
            PassAtK { k, value }
        })
        .collect()
}

/// The interval of the share of correct runs: Wilson for one run per query, otherwise a cluster
/// bootstrap over the queries (see [`AssistantSummary::answer_accuracy_ci`]).
fn accuracy_interval(results: &[&AssistantResult]) -> Option<Interval> {
    let mut by_query: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for result in results {
        by_query
            .entry(result.id.as_str())
            .or_default()
            .push(f64::from(u8::from(result.correct)));
    }
    if by_query.values().all(|runs| runs.len() <= 1) {
        return Some(wilson(
            results.iter().filter(|r| r.correct).count(),
            results.len(),
        ));
    }
    let clusters: Vec<&[f64]> = by_query.values().map(Vec::as_slice).collect();
    cluster_bootstrap_mean_ci(&clusters)
}

fn summarize(results: &[&AssistantResult]) -> AssistantSummary {
    let n = results.len();
    let count = |keep: fn(&AssistantResult) -> bool| results.iter().filter(|r| keep(r)).count();
    let errors = count(|r| r.error.is_some());
    let with_expected = count(|r| r.kind != NO_MATCH_KIND);
    AssistantSummary {
        n,
        answer_accuracy: share(count(|r| r.correct), n),
        answer_accuracy_ci: accuracy_interval(results),
        answer_accuracy_without_errors: share(count(|r| r.correct), n - errors),
        mentions_expected: (with_expected > 0).then(|| {
            share(
                count(|r| r.kind != NO_MATCH_KIND && r.mentions_expected),
                with_expected,
            )
        }),
        unanswered: count(|r| r.error.is_none() && !r.answered),
        errors,
        mean_tool_calls: share(results.iter().map(|r| r.tool_calls as usize).sum(), n),
        input_tokens: results.iter().map(|r| r.input_tokens).sum(),
        output_tokens: results.iter().map(|r| r.output_tokens).sum(),
    }
}

impl AssistantReport {
    /// Builds the report for the results gathered so far.
    #[must_use]
    pub fn from_results(
        model: &str,
        temperature: Option<f64>,
        trials: u32,
        results: Vec<AssistantResult>,
    ) -> Self {
        let mut kinds: BTreeMap<String, Vec<&AssistantResult>> = BTreeMap::new();
        for result in &results {
            kinds.entry(result.kind.clone()).or_default().push(result);
        }
        let mut response_models: Vec<String> = results
            .iter()
            .flat_map(|r| r.models.iter().cloned())
            .collect();
        response_models.sort();
        response_models.dedup();
        Self {
            model: model.to_owned(),
            response_models,
            trials,
            pass_at_k: pass_curve(&results, trials),
            temperature,
            overall: summarize(&results.iter().collect::<Vec<_>>()),
            by_kind: kinds
                .iter()
                .map(|(kind, rs)| (kind.clone(), summarize(rs)))
                .collect(),
            queries: results,
        }
    }
}

/// Runs every query through the model, in file order, `plan.trials` times each. After each run
/// `on_progress` receives the report so far, so a caller can save partial results of a paid run
/// that fails midway.
///
/// # Errors
///
/// Fails when the store's tool list cannot be read, or when `on_progress` fails.
pub async fn run(
    api: &Anthropic,
    mcp: &mut McpClient,
    queries: &[Query],
    plan: Plan,
    mut on_progress: impl FnMut(&AssistantReport) -> Result<()>,
) -> Result<AssistantReport> {
    let tools = api_tools(&mcp.list_tools().await?);
    let report = |results: Vec<AssistantResult>| {
        AssistantReport::from_results(&api.model, api.temperature, plan.trials, results)
    };
    let mut results = Vec::with_capacity(queries.len() * plan.trials as usize);
    for query in queries {
        for trial in 1..=plan.trials {
            let mut result = answer(api, mcp, &tools, query, plan.max_turns).await;
            result.trial = trial;
            results.push(result);
            on_progress(&report(results.clone()))?;
        }
    }
    Ok(report(results))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(key: &str) -> Anthropic {
        Anthropic::new(
            ApiKey::new(key.to_owned()).unwrap(),
            "m",
            "http://x",
            Duration::from_secs(1),
        )
        .unwrap()
    }

    #[test]
    fn a_structured_only_tool_result_is_passed_on_as_json() {
        let output = |structured, text: &str| crate::mcp::ToolOutput {
            is_error: false,
            structured,
            text: text.to_owned(),
        };
        let only_structured = output(Some(json!({ "results": [{ "id": "A-1" }] })), "");
        assert_eq!(
            tool_result_text(&only_structured),
            r#"{"results":[{"id":"A-1"}]}"#
        );
        assert_eq!(
            tool_result_text(&output(Some(json!({ "a": 1 })), "hello")),
            "hello"
        );
        assert_eq!(tool_result_text(&output(None, "")), "");
    }

    #[test]
    fn the_key_is_redacted_in_debug_output() {
        assert!(!format!("{:?}", client("sk-secret-value")).contains("sk-secret-value"));
    }

    #[test]
    fn an_empty_or_blank_key_is_rejected() {
        assert!(ApiKey::new(String::new()).is_err());
        assert!(ApiKey::new("  ".to_owned()).is_err());
    }

    #[test]
    fn an_api_error_message_has_the_key_blanked() {
        let message = client("sk-secret").redact(Some("bad key sk-secret given"));
        assert_eq!(message, "bad key [redacted] given");
    }

    #[test]
    fn mentions_find_an_id_in_any_spelling() {
        let expected = ["ALD-CH-1002".to_owned()];
        assert!(names_expected(
            "Get the chain ald ch 1002, it fits.",
            &expected
        ));
        assert!(!names_expected("Get ALD-CH-1003 instead.", &expected));
        assert!(!names_expected("", &expected));
    }

    #[test]
    fn an_id_that_normalizes_to_nothing_is_never_mentioned_or_correct() {
        let expected = ["東京".to_owned()];
        assert!(!names_expected("anything at all", &expected));
        assert!(!is_correct(Some("日本"), &expected));
        assert!(!is_correct(Some("!!!"), &["???".to_owned()]));
    }

    #[test]
    fn reads_the_committed_id_from_the_last_line() {
        assert_eq!(
            committed_answer("It fits.\nANSWER: ALD-CH-1002\n"),
            Some("ALD-CH-1002".to_owned())
        );
        assert_eq!(
            committed_answer("Why.\n**ANSWER: ALD-CH-1002**"),
            Some("ALD-CH-1002".to_owned())
        );
        assert_eq!(committed_answer("ANSWER: none"), Some("none".to_owned()));
    }

    #[test]
    fn a_reply_without_a_final_answer_line_commits_to_nothing() {
        assert_eq!(committed_answer("Take ALD-CH-1002."), None);
        assert_eq!(committed_answer("ANSWER: X-1\nand more words"), None);
        assert_eq!(committed_answer("ANSWER:"), None);
        assert_eq!(committed_answer(""), None);
    }

    #[test]
    fn with_nothing_expected_only_none_is_correct() {
        assert!(is_correct(Some("none"), &[]));
        assert!(is_correct(Some("None"), &[]));
        assert!(!is_correct(Some("ALD-CH-1002"), &[]));
        assert!(!is_correct(None, &[]));
    }

    #[test]
    fn only_an_exact_normalised_id_is_correct() {
        let expected = ["ALD-CH-1002".to_owned()];
        assert!(is_correct(Some("ald ch 1002"), &expected));
        assert!(!is_correct(Some("ALD-CH-10021"), &expected));
        assert!(!is_correct(Some("none"), &expected));
        assert!(!is_correct(None, &expected));
    }

    #[test]
    fn only_end_turn_is_an_answer_and_only_tool_use_continues() {
        assert_eq!(next_step(Some("end_turn")), Next::Answer);
        assert_eq!(next_step(Some("tool_use")), Next::RunTools);
        for other in ["max_tokens", "refusal", "pause_turn", "stop_sequence"] {
            assert_eq!(next_step(Some(other)), Next::GiveUp, "{other}");
        }
        assert_eq!(next_step(None), Next::GiveUp);
    }

    #[test]
    fn only_rate_limit_overload_and_server_faults_are_retried() {
        for status in [429, 529, 500, 503] {
            assert!(is_retryable(status), "{status}");
        }
        for status in [400, 401, 403, 404] {
            assert!(!is_retryable(status), "{status}");
        }
    }

    #[test]
    fn backoff_doubles_and_stays_within_its_jitter_band() {
        let api = client("k").with_backoff_base(Duration::from_millis(100));
        for attempt in 0..3 {
            let step = Duration::from_millis(100 << attempt);
            let delay = api.backoff(attempt);
            assert!(delay >= step / 2 && delay <= step, "{attempt}: {delay:?}");
        }
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
