//! End-to-end tests: the eval against the reference store running in this process, and the
//! assistant loop against a mock of the Messages API. No network and no API key needed.

use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, post};
use axum::{Json, Router};
use catalog::demo_catalog;
use mcp_commerce_evals::assistant::{self, Anthropic, ApiKey, api_tools};
use mcp_commerce_evals::mcp::McpClient;
use mcp_commerce_evals::queries::{self, Query};
use mcp_commerce_evals::tool_level::{self, Options};
use reference_store::{StoreConfig, app, serve};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const TIMEOUT: Duration = Duration::from_secs(10);
const QUERIES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/queries.json");

/// Stops its server task when dropped.
struct Server {
    base: String,
    task: JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn spawn_store() -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
    let base = format!("http://{}", listener.local_addr().expect("local address"));
    let router = app(Arc::new(demo_catalog()), &StoreConfig::default());
    let task = tokio::spawn(async move {
        let _ = serve(listener, router).await;
    });
    Server { base, task }
}

async fn connect(base: &str) -> McpClient {
    McpClient::connect(&format!("{base}/mcp"), TIMEOUT)
        .await
        .expect("connect")
}

fn options() -> Options {
    Options {
        search_tool: "search_products".to_owned(),
        top_k: 10,
        repeat: 1,
    }
}

#[test]
fn the_queries_file_is_valid_and_every_expected_part_exists() {
    let queries = queries::load(Path::new(QUERIES)).unwrap();
    assert_eq!(queries.len(), 40);
    let catalog = demo_catalog();
    for query in &queries {
        for id in &query.expected {
            assert!(
                catalog.get(id).is_some(),
                "{}: {id} is not in the demo catalog",
                query.id
            );
        }
    }
    let kinds: HashSet<_> = queries.iter().map(|q| q.kind.as_str()).collect();
    for kind in [
        "exact_part_number",
        "plain_description",
        "compatibility",
        "typo",
        "synonym",
        "filter",
        "adversarial",
    ] {
        assert!(kinds.contains(kind), "no {kind} queries");
    }
}

#[tokio::test]
async fn tool_level_eval_against_the_in_process_store() {
    let store = spawn_store().await;
    let queries = queries::load(Path::new(QUERIES)).unwrap();
    let mut mcp = connect(&store.base).await;

    let report = tool_level::run(&mut mcp, &queries, &options()).await;

    assert_eq!(report.overall.n, 40);
    assert_eq!(report.overall.errors, 0);
    assert!(
        report.overall.hit_at_5 >= 0.85,
        "hit@5 fell to {}",
        report.overall.hit_at_5
    );
    assert!(
        report.overall.mrr >= 0.85,
        "MRR fell to {}",
        report.overall.mrr
    );
    // The shipped seed is deterministic, so the counts are exact: 36 and 37 of 40.
    assert_eq!(report.overall.hit_at_1, 0.9);
    assert_eq!(report.overall.hit_at_5, 0.925);
    let ci = report.overall.hit_at_5_ci;
    assert!(
        ci.low < 0.925 && 0.925 < ci.high && ci.high <= 1.0,
        "{ci:?}"
    );
    assert!(report.overall.recall_at_10 > 0.0 && report.overall.ndcg_at_10 > 0.0);
    assert_eq!(report.overall_without_filter.n, 34);
    assert_eq!(report.overall.latency_samples, 40);
    let overall = &report.overall;
    assert!(0.0 < overall.latency_p50_ms && overall.latency_p50_ms <= overall.latency_p95_ms);
    // Spelled-out part numbers are the easy case and must always rank first.
    for id in ["q01", "q02", "q03", "q04", "q05"] {
        let result = report.queries.iter().find(|q| q.id == id).unwrap();
        assert_eq!(result.rank, Some(1), "{id} {}", result.text);
    }
    assert_eq!(report.by_kind.len(), 7);
}

#[tokio::test]
async fn a_missing_search_tool_is_reported_per_query_not_as_a_crash() {
    let store = spawn_store().await;
    let queries = queries::load(Path::new(QUERIES)).unwrap();
    let mut mcp = connect(&store.base).await;
    let wrong = Options {
        search_tool: "no_such_tool".to_owned(),
        top_k: 10,
        repeat: 1,
    };

    let report = tool_level::run(&mut mcp, &queries[..2], &wrong).await;

    assert_eq!(report.overall.errors, 2);
    assert_eq!(report.overall.hit_at_5, 0.0);
}

/// One scripted reply of the mock Messages API.
#[derive(Clone)]
struct Reply {
    status: u16,
    body: Value,
    retry_after: Option<&'static str>,
}

impl Reply {
    fn new(status: u16, body: Value) -> Self {
        Self {
            status,
            body,
            retry_after: None,
        }
    }
}

/// A scripted stand-in for `POST /v1/messages` that records what it was sent.
#[derive(Default)]
struct MockApi {
    script: Mutex<VecDeque<Reply>>,
    seen: Mutex<Vec<(String, Value)>>,
}

async fn messages(
    State(mock): State<Arc<MockApi>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, HeaderMap, Json<Value>) {
    let key = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    mock.seen.lock().expect("lock").push((key, body));
    let reply = mock
        .script
        .lock()
        .expect("lock")
        .pop_front()
        .unwrap_or_else(|| Reply::new(400, json!({ "error": { "message": "script exhausted" } })));
    let mut out_headers = HeaderMap::new();
    if let Some(seconds) = reply.retry_after {
        out_headers.insert("retry-after", seconds.parse().expect("header value"));
    }
    (
        StatusCode::from_u16(reply.status).expect("status"),
        out_headers,
        Json(reply.body),
    )
}

async fn spawn_mock(script: Vec<Reply>) -> (Server, Arc<MockApi>) {
    let mock = Arc::new(MockApi {
        script: Mutex::new(script.into()),
        ..MockApi::default()
    });
    let router = Router::new()
        .route("/v1/messages", post(messages))
        .with_state(mock.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
    let base = format!("http://{}", listener.local_addr().expect("local address"));
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (Server { base, task }, mock)
}

fn api_reply(stop_reason: &str, content: &Value) -> Reply {
    Reply::new(
        200,
        json!({
            "stop_reason": stop_reason,
            "content": content,
            "usage": { "input_tokens": 100, "output_tokens": 20 }
        }),
    )
}

fn tool_use(query: &str) -> Reply {
    api_reply(
        "tool_use",
        &json!([
            { "type": "text", "text": "Let me look." },
            { "type": "tool_use", "id": "toolu_1", "name": "search_products",
              "input": { "query": query, "limit": 3 } }
        ]),
    )
}

fn reply_with(stop_reason: &str, text: &str) -> Reply {
    api_reply(stop_reason, &json!([{ "type": "text", "text": text }]))
}

fn final_answer(text: &str) -> Reply {
    reply_with("end_turn", text)
}

fn client(api_server: &Server, key: &str) -> Anthropic {
    Anthropic::new(
        ApiKey::new(key.to_owned()).expect("key"),
        "test-model",
        &api_server.base,
        TIMEOUT,
    )
    .expect("client")
    .with_backoff_base(Duration::ZERO)
}

fn query(id: &str, text: &str, expected: &str) -> Query {
    serde_json::from_value(json!({ "id": id, "kind": "k", "text": text, "expected": [expected] }))
        .expect("query")
}

/// Asks the model one query against a store with no tools exposed to it.
async fn ask(api: &Anthropic, store: &Server, expected: &str) -> assistant::AssistantResult {
    let mut mcp = connect(&store.base).await;
    assistant::answer(api, &mut mcp, &[], &query("a", "a chain", expected), 8).await
}

#[tokio::test]
async fn the_model_searches_through_the_store_and_commits_to_the_right_part() {
    let store = spawn_store().await;
    let (api_server, mock) = spawn_mock(vec![
        tool_use("FER-CH-4296"),
        final_answer("An 11-speed chain.\nANSWER: FER-CH-4296"),
    ])
    .await;
    let api = client(&api_server, "test-key");
    let mut mcp = connect(&store.base).await;
    let tools = api_tools(&mcp.list_tools().await.unwrap());

    let result = assistant::answer(
        &api,
        &mut mcp,
        &tools,
        &query("a", "an 11-speed chain", "FER-CH-4296"),
        8,
    )
    .await;

    assert_eq!(result.error, None);
    assert!(result.correct && result.answered);
    assert_eq!(result.committed.as_deref(), Some("FER-CH-4296"));
    assert_eq!(result.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!((result.tool_calls, result.turns), (1, 2));
    assert_eq!(result.tools_used, ["search_products"]);
    assert_eq!((result.input_tokens, result.output_tokens), (200, 40));

    let seen = mock.seen.lock().unwrap();
    assert_eq!(seen[0].0, "test-key");
    assert_eq!(seen[0].1["model"], "test-model");
    assert!(
        seen[0].1.get("temperature").is_none(),
        "no temperature is sent by default: {}",
        seen[0].1
    );
    assert!(
        seen[0].1["system"]
            .as_str()
            .unwrap()
            .contains("ANSWER: <part id>")
    );
    assert_eq!(seen[0].1["tools"].as_array().unwrap().len(), 4);
    assert_eq!(seen[0].1["tools"][0]["input_schema"]["type"], "object");
    let reply = &seen[1].1["messages"][2]["content"][0];
    assert_eq!(reply["type"], "tool_result");
    assert_eq!(reply["tool_use_id"], "toolu_1");
    assert!(reply["content"].as_str().unwrap().contains("FER-CH-4296"));
}

#[tokio::test]
async fn a_mention_without_the_committed_id_is_not_correct() {
    let store = spawn_store().await;
    let text = "FER-CH-4296 is close but too dear.\nANSWER: FER-CH-1581";
    let (api_server, _) = spawn_mock(vec![final_answer(text)]).await;

    let result = ask(&client(&api_server, "k"), &store, "FER-CH-4296").await;

    assert!(!result.correct);
    assert!(result.mentions_expected);
}

#[tokio::test]
async fn answering_none_is_not_correct() {
    let store = spawn_store().await;
    let (api_server, _) = spawn_mock(vec![final_answer("Nothing fits.\nANSWER: none")]).await;

    let result = ask(&client(&api_server, "k"), &store, "FER-CH-4296").await;

    assert!(!result.correct && result.answered);
}

#[tokio::test]
async fn a_stop_reason_other_than_end_turn_is_recorded_and_not_answered() {
    let store = spawn_store().await;
    for stop_reason in ["max_tokens", "refusal", "pause_turn"] {
        let text = "ANSWER: FER-CH-4296";
        let (api_server, _) = spawn_mock(vec![reply_with(stop_reason, text)]).await;

        let result = ask(&client(&api_server, "k"), &store, "FER-CH-4296").await;

        assert_eq!(result.stop_reason.as_deref(), Some(stop_reason));
        assert!(!result.answered && !result.correct, "{stop_reason}");
        assert_eq!(result.committed, None, "{stop_reason}");
        assert_eq!(result.error, None, "{stop_reason}");
    }
}

#[tokio::test]
async fn a_429_with_retry_after_is_retried_and_then_succeeds() {
    let store = spawn_store().await;
    let limited = Reply {
        retry_after: Some("0"),
        ..Reply::new(429, json!({ "error": { "message": "slow down" } }))
    };
    let (api_server, mock) = spawn_mock(vec![limited, final_answer("ANSWER: X-1")]).await;

    let result = ask(&client(&api_server, "k"), &store, "X-1").await;

    assert_eq!(result.error, None);
    assert!(result.correct);
    assert_eq!(mock.seen.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn overload_and_server_faults_are_retried_up_to_three_times() {
    let store = spawn_store().await;
    let fault = |status| Reply::new(status, json!({ "error": { "message": "try later" } }));
    let (api_server, mock) = spawn_mock(vec![fault(529), fault(500), fault(503), fault(529)]).await;

    let result = ask(&client(&api_server, "k"), &store, "X-1").await;

    let error = result.error.expect("gives up");
    assert!(error.contains("529"), "{error}");
    assert_eq!(
        mock.seen.lock().unwrap().len(),
        4,
        "one attempt plus three retries"
    );
}

#[tokio::test]
async fn a_401_is_not_retried_and_its_message_never_contains_the_key() {
    let store = spawn_store().await;
    let denied = json!({ "error": { "type": "authentication_error",
                                    "message": "invalid x-api-key sk-do-not-print" } });
    let (api_server, mock) = spawn_mock(vec![Reply::new(401, denied)]).await;
    let api = client(&api_server, "sk-do-not-print");

    let result = ask(&api, &store, "X-1").await;

    let error = result.error.expect("the run fails");
    assert!(error.contains("401"), "{error}");
    assert!(!error.contains("sk-do-not-print") && !format!("{api:?}").contains("sk-do-not-print"));
    assert_eq!(mock.seen.lock().unwrap().len(), 1);
    assert!(!result.correct);
}

#[tokio::test]
async fn a_slow_api_hits_the_request_timeout() {
    let store = spawn_store().await;
    let router = Router::new().route(
        "/v1/messages",
        post(|| async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Json(json!({}))
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
    let base = format!("http://{}", listener.local_addr().expect("local address"));
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let slow = Server { base, task };
    let api = Anthropic::new(
        ApiKey::new("k".to_owned()).expect("key"),
        "m",
        &slow.base,
        Duration::from_millis(100),
    )
    .expect("client");

    let result = ask(&api, &store, "X-1").await;

    assert!(result.error.expect("times out").contains("HTTP error"));
}

#[tokio::test]
async fn the_report_scores_each_query_and_counts_unanswered_apart_from_errors() {
    let store = spawn_store().await;
    let script = vec![
        tool_use("FER-CH-4296"),
        final_answer("ANSWER: FER-CH-4296"),
        final_answer("I would pick ALD-CH-0000.\nANSWER: ALD-CH-0000"),
        tool_use("chain"),
        tool_use("chain"),
        Reply::new(400, json!({ "error": { "message": "bad request" } })),
    ];
    let (api_server, _) = spawn_mock(script).await;
    let api = client(&api_server, "test-key");
    let mut mcp = connect(&store.base).await;
    let queries = [
        query("a", "chain one", "FER-CH-4296"),
        query("b", "chain two", "FER-CH-4296"),
        query("c", "chain three", "FER-CH-4296"),
        query("d", "chain four", "FER-CH-4296"),
    ];

    let report = assistant::run(&api, &mut mcp, &queries, 2, |_| Ok(()))
        .await
        .unwrap();

    assert_eq!(report.model, "test-model");
    assert_eq!(report.temperature, None);
    let overall = &report.overall;
    assert_eq!((overall.n, overall.errors, overall.unanswered), (4, 1, 1));
    assert_eq!(overall.answer_accuracy, 0.25);
    assert!((overall.answer_accuracy_without_errors - 1.0 / 3.0).abs() < 1e-9);
    assert_eq!(report.queries[2].stop_reason.as_deref(), Some("tool_use"));
    assert!(!report.queries[2].answered && report.queries[2].error.is_none());
}

#[test]
fn an_empty_api_key_is_rejected_before_any_request() {
    assert!(ApiKey::new(String::new()).is_err());
}

/// A stand-in MCP server whose `tools/call` always returns `call_result` and counts its calls.
struct MockMcp {
    call_result: Value,
    calls: AtomicUsize,
}

async fn mock_rpc(
    State(mock): State<Arc<MockMcp>>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let reply = match body["method"].as_str() {
        Some("initialize") => json!({
            "protocolVersion": "2025-03-26",
            "serverInfo": { "name": "mock-store", "version": "9.9" }
        }),
        Some("tools/call") => {
            mock.calls.fetch_add(1, Ordering::SeqCst);
            mock.call_result.clone()
        }
        _ => return (StatusCode::ACCEPTED, Json(Value::Null)),
    };
    (
        StatusCode::OK,
        Json(json!({ "jsonrpc": "2.0", "id": body["id"], "result": reply })),
    )
}

async fn spawn_mock_mcp(call_result: Value) -> (Server, Arc<MockMcp>) {
    let mock = Arc::new(MockMcp {
        call_result,
        calls: AtomicUsize::new(0),
    });
    let router = Router::new()
        .route("/mcp", post(mock_rpc))
        .with_state(mock.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
    let base = format!("http://{}", listener.local_addr().expect("local address"));
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (Server { base, task }, mock)
}

fn structured(body: &Value) -> Value {
    json!({ "content": [], "structuredContent": body })
}

#[tokio::test]
async fn a_store_that_ignores_limit_is_scored_on_the_top_k_only() {
    let hits: Vec<_> = (1..=9)
        .map(|i| json!({ "id": format!("P-{i}") }))
        .chain([json!({ "id": "WANTED" })])
        .collect();
    let (server, _) = spawn_mock_mcp(structured(&json!({ "results": hits }))).await;
    let mut mcp = connect(&server.base).await;
    let queries = [query("a", "thing", "WANTED")];
    let options = Options {
        top_k: 5,
        ..options()
    };

    let report = tool_level::run(&mut mcp, &queries, &options).await;

    assert_eq!(report.queries[0].returned.len(), 5);
    assert_eq!(report.queries[0].rank, None);
    assert_eq!(report.overall.hit_at_5, 0.0);
}

#[tokio::test]
async fn an_unrecognised_result_shape_counts_as_an_error() {
    let (server, _) = spawn_mock_mcp(structured(&json!({ "matches": [{ "id": "WANTED" }] }))).await;
    let mut mcp = connect(&server.base).await;
    let queries = [query("a", "thing", "WANTED")];

    let report = tool_level::run(&mut mcp, &queries, &options()).await;

    assert_eq!(report.overall.errors, 1);
    assert!(report.queries[0].error.is_some());
}

#[tokio::test]
async fn the_handshake_keeps_the_server_info_and_negotiated_protocol() {
    let (server, _) = spawn_mock_mcp(json!({})).await;
    let mcp = connect(&server.base).await;

    let handshake = mcp.handshake();

    assert_eq!(handshake.server_info["name"], "mock-store");
    assert_eq!(handshake.protocol_version, "2025-03-26");
}

/// Runs the CLI binary against `base` and returns its exit code and stderr.
async fn run_cli(base: &str, extra: &[&str], out: &Path) -> (i32, String) {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_mcp-commerce-evals"));
    command
        .args([
            "--url",
            &format!("{base}/mcp"),
            "--queries",
            QUERIES,
            "--out",
        ])
        .arg(out)
        .args(extra);
    let output = tokio::task::spawn_blocking(move || command.output())
        .await
        .expect("join")
        .expect("run the binary");
    (
        output.status.code().expect("exit code"),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn scratch_file(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mcp-evals-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create dir");
    dir.join("report.json")
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_exits_zero_and_writes_the_report_when_gates_hold() {
    let store = spawn_store().await;
    let out = scratch_file("ok");

    let (code, stderr) = run_cli(
        &store.base,
        &["--min-hit5", "0.5", "--min-mrr", "0.5"],
        &out,
    )
    .await;

    assert_eq!(code, 0, "{stderr}");
    let report: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(report["complete"], true);
    let meta = &report["meta"];
    assert_eq!(meta["crate_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(meta["queries_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(meta["top_k"], 10);
    assert_eq!(meta["search_tool"], "search_products");
    assert!(meta["server_info"]["name"].is_string(), "{meta}");
    assert!(meta["protocol_version"].as_str().unwrap().starts_with("20"));
    assert!(meta["assistant"].is_null());
    assert!(report["tool_level"]["overall_without_filter"]["hit_at_1_ci"]["low"].is_number());
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_exits_3_after_writing_the_json_when_a_gate_is_not_met() {
    let store = spawn_store().await;
    let out = scratch_file("gate");

    let (code, stderr) = run_cli(&store.base, &["--min-hit5", "1.0"], &out).await;

    assert_eq!(code, 3);
    assert!(stderr.contains("gate failed: hit5"), "{stderr}");
    assert!(
        out.exists(),
        "the report is written before the gate decides"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_exits_2_when_a_query_errors() {
    let store = spawn_store().await;
    let out = scratch_file("errors");

    let (code, stderr) = run_cli(&store.base, &["--search-tool", "no_such_tool"], &out).await;

    assert_eq!(code, 2, "{stderr}");
    assert!(out.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_refuses_to_overwrite_a_report_unless_forced() {
    let store = spawn_store().await;
    let out = scratch_file("force");
    std::fs::write(&out, "keep me").expect("seed the file");

    let (code, stderr) = run_cli(&store.base, &[], &out).await;
    assert_eq!(code, 1);
    assert!(stderr.contains("--force"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&out).expect("read"), "keep me");

    let (code, stderr) = run_cli(&store.base, &["--force"], &out).await;
    assert_eq!(code, 0, "{stderr}");
    assert_ne!(std::fs::read_to_string(&out).expect("read"), "keep me");
}

#[tokio::test]
async fn repeat_times_each_query_that_many_times_after_one_untimed_warm_up() {
    let body = structured(&json!({ "results": [{ "id": "WANTED" }] }));
    let (server, mock) = spawn_mock_mcp(body).await;
    let mut mcp = connect(&server.base).await;
    let queries = [query("a", "one", "WANTED"), query("b", "two", "WANTED")];
    let options = Options {
        repeat: 3,
        ..options()
    };

    let report = tool_level::run(&mut mcp, &queries, &options).await;

    assert_eq!(mock.calls.load(Ordering::SeqCst), 1 + 2 * 3);
    assert_eq!(report.overall.latency_samples, 6);
    assert_eq!(report.queries[0].latencies_ms.len(), 3);
}

#[tokio::test]
async fn assistant_progress_reports_the_partial_results_after_every_query() {
    let store = spawn_store().await;
    let script = vec![final_answer("ANSWER: A-1"), final_answer("ANSWER: B-2")];
    let (api_server, _) = spawn_mock(script).await;
    let api = client(&api_server, "k");
    let mut mcp = connect(&store.base).await;
    let queries = [query("a", "one", "A-1"), query("b", "two", "A-1")];
    let mut seen = Vec::new();

    assistant::run(&api, &mut mcp, &queries, 2, |partial| {
        seen.push((partial.queries.len(), partial.overall.answer_accuracy));
        Ok(())
    })
    .await
    .unwrap();

    assert_eq!(seen, [(1, 1.0), (2, 0.5)]);
}

#[tokio::test]
async fn a_requested_temperature_is_sent_and_recorded() {
    let store = spawn_store().await;
    let (api_server, mock) = spawn_mock(vec![final_answer("ANSWER: none")]).await;
    let api = client(&api_server, "k").with_temperature(Some(0.3));
    let mut mcp = connect(&store.base).await;

    let report = assistant::run(&api, &mut mcp, &[query("a", "one", "A-1")], 2, |_| Ok(()))
        .await
        .unwrap();

    assert_eq!(report.temperature, Some(0.3));
    assert_eq!(mock.seen.lock().unwrap()[0].1["temperature"], 0.3);
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_exits_2_when_every_assistant_query_fails_with_an_api_error() {
    let store = spawn_store().await;
    let script = vec![Reply::new(400, json!({ "error": { "message": "nope" } })); 64];
    let (api_server, _) = spawn_mock(script).await;
    let out = scratch_file("assistant-errors");
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_mcp-commerce-evals"));
    command
        .args([
            "--url",
            &format!("{}/mcp", store.base),
            "--queries",
            QUERIES,
        ])
        .args(["--assistant", "anthropic", "--out"])
        .arg(&out)
        .env("ANTHROPIC_API_KEY", "test-key")
        .env("ANTHROPIC_BASE_URL", &api_server.base);
    let output = tokio::task::spawn_blocking(move || command.output())
        .await
        .expect("join")
        .expect("run the binary");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    let report: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(report["tool_level"]["overall"]["errors"], 0);
    assert!(
        report["assistant_level"]["overall"]["errors"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(report["assistant_level"]["temperature"].is_null());
}

/// What the scripted server saw of one request: HTTP method, JSON-RPC method (if any) and the
/// session id the client sent.
type Seen = (String, Option<String>, Option<String>);

type ScriptedReply =
    Box<dyn Fn(&Method, Option<&str>, &Value, Option<&str>) -> Response + Send + Sync>;

/// A server whose replies come from a closure, and which records every request.
struct Scripted {
    reply: ScriptedReply,
    seen: Mutex<Vec<Seen>>,
}

async fn scripted_rpc(
    State(server): State<Arc<Scripted>>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let session = headers
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let rpc = json["method"].as_str().map(str::to_owned);
    server
        .seen
        .lock()
        .expect("lock")
        .push((method.to_string(), rpc.clone(), session.clone()));
    (server.reply)(&method, rpc.as_deref(), &json, session.as_deref())
}

async fn spawn_scripted(
    reply: impl Fn(&Method, Option<&str>, &Value, Option<&str>) -> Response + Send + Sync + 'static,
) -> (Server, Arc<Scripted>) {
    let server = Arc::new(Scripted {
        reply: Box::new(reply),
        seen: Mutex::new(Vec::new()),
    });
    let router = Router::new()
        .route("/mcp", any(scripted_rpc))
        .with_state(server.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
    let base = format!("http://{}", listener.local_addr().expect("local address"));
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (Server { base, task }, server)
}

fn rpc_ok(id: &Value, result: &Value, session: Option<&str>) -> Response {
    let mut response =
        Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response();
    if let Some(session) = session {
        response.headers_mut().insert(
            "mcp-session-id",
            HeaderValue::from_str(session).expect("header"),
        );
    }
    response
}

/// An `initialize` result that names a protocol version the client supports.
fn initialized() -> Value {
    json!({ "protocolVersion": "2025-06-18" })
}

fn sessions_sent(server: &Scripted, rpc: &str) -> Vec<Option<String>> {
    let seen = server.seen.lock().expect("lock");
    seen.iter()
        .filter(|(_, m, _)| m.as_deref() == Some(rpc))
        .map(|(_, _, s)| s.clone())
        .collect()
}

#[tokio::test]
async fn a_session_id_that_is_not_text_keeps_the_previous_one() {
    let (server, scripted) = spawn_scripted(|_, rpc, body, _| match rpc {
        Some("initialize") => rpc_ok(&body["id"], &initialized(), Some("s1")),
        Some("tools/list") => {
            let mut response = rpc_ok(&body["id"], &json!({ "tools": [] }), None);
            response.headers_mut().insert(
                "mcp-session-id",
                HeaderValue::from_bytes(&[0xff, 0xfe]).expect("header"),
            );
            response
        }
        Some("tools/call") => rpc_ok(&body["id"], &json!({ "content": [] }), None),
        _ => StatusCode::ACCEPTED.into_response(),
    })
    .await;
    let mut mcp = connect(&server.base).await;

    mcp.list_tools().await.unwrap();
    mcp.call_tool("t", json!({})).await.unwrap();

    assert_eq!(
        sessions_sent(&scripted, "tools/call"),
        [Some("s1".to_owned())]
    );
}

#[tokio::test]
async fn a_forgotten_session_is_reopened_once_and_the_request_retried() {
    let (server, scripted) = spawn_scripted(|_, rpc, body, session| match (rpc, session) {
        (Some("initialize"), _) => {
            let fresh = if body["id"] == 1 { "old" } else { "new" };
            rpc_ok(&body["id"], &initialized(), Some(fresh))
        }
        (Some("tools/list"), Some("old")) => StatusCode::NOT_FOUND.into_response(),
        (Some("tools/list"), Some("new")) => {
            rpc_ok(&body["id"], &json!({ "tools": [{ "name": "t" }] }), None)
        }
        _ => StatusCode::ACCEPTED.into_response(),
    })
    .await;
    let mut mcp = connect(&server.base).await;

    let tools = mcp.list_tools().await.unwrap();

    assert_eq!(tools.len(), 1);
    assert_eq!(
        sessions_sent(&scripted, "tools/list"),
        [Some("old".to_owned()), Some("new".to_owned())]
    );
    assert_eq!(sessions_sent(&scripted, "initialize").len(), 2);
}

#[tokio::test]
async fn closing_deletes_the_session_and_tolerates_a_405() {
    for refuse in [false, true] {
        let (server, scripted) = spawn_scripted(move |method, rpc, body, _| {
            if method == Method::DELETE {
                return if refuse {
                    StatusCode::METHOD_NOT_ALLOWED
                } else {
                    StatusCode::OK
                }
                .into_response();
            }
            match rpc {
                Some("initialize") => rpc_ok(&body["id"], &initialized(), Some("s9")),
                _ => StatusCode::ACCEPTED.into_response(),
            }
        })
        .await;
        let mcp = connect(&server.base).await;

        mcp.close().await;

        let seen = scripted.seen.lock().unwrap();
        let last = seen.last().unwrap();
        assert_eq!((last.0.as_str(), last.2.as_deref()), ("DELETE", Some("s9")));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_runs_only_the_requested_kinds() {
    let store = spawn_store().await;
    let out = scratch_file("kinds");

    let (code, stderr) = run_cli(&store.base, &["--kind", "typo", "--kind", "synonym"], &out).await;

    assert_eq!(code, 0, "{stderr}");
    let report: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(report["tool_level"]["overall"]["n"], 12);
    assert_eq!(report["meta"]["kinds"], json!(["typo", "synonym"]));

    let (code, stderr) = run_cli(&store.base, &["--kind", "nope", "--force"], &out).await;
    assert_eq!(code, 1);
    assert!(stderr.contains("no query has kind"), "{stderr}");
}
