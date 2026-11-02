//! End-to-end tests: the eval against the reference store running in this process, and the
//! assistant loop against a mock of the Messages API. No network and no API key needed.

use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
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

fn options() -> Options {
    Options {
        search_tool: "search_products".to_owned(),
        top_k: 10,
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
    ] {
        assert!(kinds.contains(kind), "no {kind} queries");
    }
}

#[tokio::test]
async fn tool_level_eval_against_the_in_process_store() {
    let store = spawn_store().await;
    let queries = queries::load(Path::new(QUERIES)).unwrap();
    let mut mcp = McpClient::connect(&format!("{}/mcp", store.base))
        .await
        .unwrap();

    let report = tool_level::run(&mut mcp, &queries, &options())
        .await
        .unwrap();

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
    assert!(report.overall.hit_at_1 <= 1.0 && report.overall.latency_p95_ms > 0.0);
    // Spelled-out part numbers are the easy case and must always rank first.
    for id in ["q01", "q02", "q03", "q04", "q05"] {
        let result = report.queries.iter().find(|q| q.id == id).unwrap();
        assert_eq!(result.rank, Some(1), "{id} {}", result.text);
    }
    assert_eq!(report.by_kind.len(), 6);
}

#[tokio::test]
async fn a_missing_search_tool_is_reported_per_query_not_as_a_crash() {
    let store = spawn_store().await;
    let queries = queries::load(Path::new(QUERIES)).unwrap();
    let mut mcp = McpClient::connect(&format!("{}/mcp", store.base))
        .await
        .unwrap();
    let wrong = Options {
        search_tool: "no_such_tool".to_owned(),
        top_k: 10,
    };

    let report = tool_level::run(&mut mcp, &queries[..2], &wrong)
        .await
        .unwrap();

    assert_eq!(report.overall.errors, 2);
    assert_eq!(report.overall.hit_at_5, 0.0);
}

/// A scripted stand-in for `POST /v1/messages` that records what it was sent.
#[derive(Default)]
struct MockApi {
    script: Mutex<VecDeque<(u16, Value)>>,
    seen: Mutex<Vec<(String, Value)>>,
}

async fn messages(
    State(mock): State<Arc<MockApi>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let key = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    mock.seen.lock().expect("lock").push((key, body));
    let (status, reply) = mock
        .script
        .lock()
        .expect("lock")
        .pop_front()
        .unwrap_or((500, json!({ "error": { "message": "script exhausted" } })));
    (StatusCode::from_u16(status).expect("status"), Json(reply))
}

async fn spawn_mock(script: Vec<(u16, Value)>) -> (Server, Arc<MockApi>) {
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

fn tool_use(query: &str) -> (u16, Value) {
    (
        200,
        json!({
            "stop_reason": "tool_use",
            "content": [
                { "type": "text", "text": "Let me look." },
                { "type": "tool_use", "id": "toolu_1", "name": "search_products",
                  "input": { "query": query, "limit": 3 } }
            ],
            "usage": { "input_tokens": 100, "output_tokens": 20 }
        }),
    )
}

fn final_answer(text: &str) -> (u16, Value) {
    (
        200,
        json!({
            "stop_reason": "end_turn",
            "content": [{ "type": "text", "text": text }],
            "usage": { "input_tokens": 150, "output_tokens": 30 }
        }),
    )
}

fn query(id: &str, text: &str, expected: &str) -> Query {
    serde_json::from_value(json!({ "id": id, "kind": "k", "text": text, "expected": [expected] }))
        .expect("query")
}

#[tokio::test]
async fn the_model_searches_through_the_store_and_names_the_part() {
    let store = spawn_store().await;
    let (api_server, mock) = spawn_mock(vec![
        tool_use("FER-CH-4296"),
        final_answer("Take FER-CH-4296, an 11-speed chain."),
    ])
    .await;
    let api = Anthropic::new(
        ApiKey::new("test-key".to_owned()),
        "test-model",
        &api_server.base,
    );
    let mut mcp = McpClient::connect(&format!("{}/mcp", store.base))
        .await
        .unwrap();
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
    assert!(result.named_expected);
    assert_eq!((result.tool_calls, result.turns), (1, 2));
    assert_eq!(result.tools_used, ["search_products"]);
    assert_eq!((result.input_tokens, result.output_tokens), (250, 50));

    let seen = mock.seen.lock().unwrap();
    assert_eq!(seen[0].0, "test-key");
    assert_eq!(seen[0].1["model"], "test-model");
    let names: Vec<_> = seen[0].1["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].clone())
        .collect();
    assert_eq!(names.len(), 4);
    assert!(seen[0].1["tools"][0]["input_schema"]["type"] == "object");
    let reply = &seen[1].1["messages"][2]["content"][0];
    assert_eq!(reply["type"], "tool_result");
    assert_eq!(reply["tool_use_id"], "toolu_1");
    assert!(reply["content"].as_str().unwrap().contains("FER-CH-4296"));
}

#[tokio::test]
async fn an_api_error_is_recorded_without_leaking_the_key() {
    let store = spawn_store().await;
    let denied =
        json!({ "error": { "type": "authentication_error", "message": "invalid x-api-key" } });
    let (api_server, _) = spawn_mock(vec![(401, denied)]).await;
    let api = Anthropic::new(
        ApiKey::new("sk-do-not-print".to_owned()),
        "test-model",
        &api_server.base,
    );
    let mut mcp = McpClient::connect(&format!("{}/mcp", store.base))
        .await
        .unwrap();

    let result = assistant::answer(&api, &mut mcp, &[], &query("a", "chain", "X-1"), 8).await;

    let error = result.error.expect("the run fails");
    assert!(error.contains("401"), "{error}");
    assert!(!error.contains("sk-do-not-print") && !format!("{api:?}").contains("sk-do-not-print"));
    assert!(!result.named_expected);
}

#[tokio::test]
async fn the_assistant_report_scores_each_query_and_gives_up_after_the_turn_limit() {
    let store = spawn_store().await;
    let script = vec![
        tool_use("FER-CH-4296"),
        final_answer("FER-CH-4296"),
        final_answer("I would pick ALD-CH-0000."),
        tool_use("chain"),
        tool_use("chain"),
    ];
    let (api_server, _) = spawn_mock(script).await;
    let api = Anthropic::new(
        ApiKey::new("test-key".to_owned()),
        "test-model",
        &api_server.base,
    );
    let mut mcp = McpClient::connect(&format!("{}/mcp", store.base))
        .await
        .unwrap();
    let queries = [
        query("a", "chain one", "FER-CH-4296"),
        query("b", "chain two", "FER-CH-4296"),
        query("c", "chain three", "FER-CH-4296"),
    ];

    let report = assistant::run(&api, &mut mcp, &queries, 2).await.unwrap();

    assert_eq!(report.model, "test-model");
    assert_eq!(report.overall.n, 3);
    assert!((report.overall.answer_accuracy - 1.0 / 3.0).abs() < 1e-9);
    assert_eq!(report.overall.errors, 1);
    assert_eq!(report.overall.mean_tool_calls, 1.0);
    assert!(
        report.queries[2]
            .error
            .as_deref()
            .unwrap()
            .contains("no final answer after 2 turns")
    );
}
