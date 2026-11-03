//! The store over real HTTP: the four tools, their schemas, the rate limit and the host check.

use std::sync::Arc;

use catalog::{StockStatus, demo_catalog};
use reference_store::{StoreConfig, app, serve};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

struct Running {
    base: String,
    client: reqwest::Client,
    task: JoinHandle<()>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn start(config: StoreConfig) -> Running {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
    let base = format!("http://{}", listener.local_addr().expect("local address"));
    let router = app(Arc::new(demo_catalog()), &config);
    let task = tokio::spawn(async move {
        let _ = serve(listener, router).await;
    });
    Running {
        base,
        client: reqwest::Client::new(),
        task,
    }
}

impl Running {
    async fn rpc(&self, method: &str, params: Value) -> reqwest::Response {
        self.client
            .post(format!("{}/mcp", self.base))
            .header("accept", "application/json, text/event-stream")
            .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
            .send()
            .await
            .expect("request reaches the store")
    }

    async fn rpc_with_header(&self, name: &str, value: &str) -> StatusCode {
        self.client
            .post(format!("{}/mcp", self.base))
            .header("accept", "application/json, text/event-stream")
            .header(name, value)
            .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
            .send()
            .await
            .expect("request reaches the store")
            .status()
    }

    async fn call(&self, tool: &str, arguments: Value) -> Value {
        let response = self
            .rpc(
                "tools/call",
                json!({ "name": tool, "arguments": arguments }),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        response.json::<Value>().await.expect("JSON body")["result"].clone()
    }
}

#[tokio::test]
async fn health_answers_ok() {
    let store = start(StoreConfig::default()).await;
    let health = store
        .client
        .get(format!("{}/health", store.base))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
}

#[tokio::test]
async fn every_tool_has_schemas_and_a_description() {
    let store = start(StoreConfig::default()).await;
    let listed: Value = store
        .rpc("tools/list", json!({}))
        .await
        .json()
        .await
        .unwrap();
    let tools = listed["result"]["tools"].as_array().unwrap();
    let mut names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "check_availability",
            "create_cart",
            "get_product",
            "search_products"
        ]
    );
    for tool in tools {
        assert!(tool["description"].as_str().unwrap().len() > 80, "{tool}");
        assert_eq!(tool["inputSchema"]["type"], "object");
        assert_eq!(tool["outputSchema"]["type"], "object");
    }
}

/// Formats a JSON Schema validator knows (draft 2020-12). Schemars adds its own (`uint32`,
/// `double`) for Rust number types, and an MCP client that validates the schema warns on each one.
const STANDARD_FORMATS: &[&str] = &[
    "date-time",
    "date",
    "time",
    "duration",
    "email",
    "idn-email",
    "hostname",
    "idn-hostname",
    "ipv4",
    "ipv6",
    "uri",
    "uri-reference",
    "iri",
    "iri-reference",
    "uuid",
    "uri-template",
    "json-pointer",
    "relative-json-pointer",
    "regex",
];

fn non_standard_formats(schema: &Value, path: &str, found: &mut Vec<String>) {
    match schema {
        Value::Object(map) => {
            if let Some(Value::String(format)) = map.get("format")
                && !STANDARD_FORMATS.contains(&format.as_str())
            {
                found.push(format!("{path}: {format}"));
            }
            for (key, value) in map {
                non_standard_formats(value, &format!("{path}/{key}"), found);
            }
        }
        Value::Array(items) => {
            for (i, value) in items.iter().enumerate() {
                non_standard_formats(value, &format!("{path}/{i}"), found);
            }
        }
        _ => {}
    }
}

#[tokio::test]
async fn schemas_use_only_standard_formats() {
    let store = start(StoreConfig::default()).await;
    let listed: Value = store
        .rpc("tools/list", json!({}))
        .await
        .json()
        .await
        .unwrap();
    let mut found = Vec::new();
    for tool in listed["result"]["tools"].as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        non_standard_formats(&tool["inputSchema"], &format!("{name}/input"), &mut found);
        non_standard_formats(&tool["outputSchema"], &format!("{name}/output"), &mut found);
    }
    assert!(found.is_empty(), "non-standard formats: {found:?}");
    let search = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "search_products")
        .unwrap();
    let limit = &search["inputSchema"]["properties"]["limit"];
    assert_eq!(
        (&limit["minimum"], &limit["maximum"]),
        (&json!(1), &json!(25))
    );
}

#[tokio::test]
async fn search_returns_structured_content_and_a_matching_text_block() {
    let store = start(StoreConfig::default()).await;
    let result = store
        .call(
            "search_products",
            json!({ "query": "11-speed chain", "limit": 3 }),
        )
        .await;
    assert_eq!(result["isError"], false);
    let structured = &result["structuredContent"];
    assert_eq!(structured["results"].as_array().unwrap().len(), 3);
    assert!(structured["total_matches"].as_u64().unwrap() >= 3);
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(&text, structured);
}

#[tokio::test]
async fn filters_reach_the_search() {
    let store = start(StoreConfig::default()).await;
    let result = store
        .call(
            "search_products",
            json!({ "category": "tire", "in_stock_only": true, "max_price_usd": 30, "limit": 25 }),
        )
        .await;
    let results = result["structuredContent"]["results"].as_array().unwrap();
    assert!(!results.is_empty());
    for part in results {
        assert_eq!(part["category"], "tire");
        assert!(part["price_usd"].as_f64().unwrap() <= 30.0);
        assert!(["in_stock", "low_stock"].contains(&part["stock_status"].as_str().unwrap()));
    }
}

#[tokio::test]
async fn product_availability_and_cart_flow() {
    let store = start(StoreConfig::default()).await;
    let found = store
        .call(
            "search_products",
            json!({ "query": "chain", "in_stock_only": true }),
        )
        .await;
    let id = found["structuredContent"]["results"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let product = store
        .call("get_product", json!({ "id": id.to_lowercase() }))
        .await;
    assert_eq!(product["structuredContent"]["id"], id);
    assert!(product["structuredContent"]["attributes"].is_object());

    let items = json!({ "items": [{ "id": id, "quantity": 1 }] });
    let availability = store.call("check_availability", items.clone()).await;
    assert_eq!(
        availability["structuredContent"]["items"][0]["availability"],
        "available"
    );

    let cart = store.call("create_cart", items).await;
    let url = cart["structuredContent"]["checkout_url"].as_str().unwrap();
    assert!(
        url.starts_with("https://demo-shop.example/checkout/cart_"),
        "{url}"
    );
}

/// The text of a tool result that must be an error.
fn error_text(result: &Value) -> &str {
    assert_eq!(result["isError"], true, "{result}");
    result["content"][0]["text"].as_str().expect("error text")
}

#[tokio::test]
async fn tool_errors_are_reported_to_the_assistant_with_a_recovery_hint() {
    let store = start(StoreConfig::default()).await;
    let missing = store
        .call("get_product", json!({ "id": "NOPE-0000" }))
        .await;
    assert_eq!(
        error_text(&missing),
        "No part has the id 'NOPE-0000'. Copy ids from search_products results."
    );
    let cart = store.call("create_cart", json!({ "items": [] })).await;
    assert_eq!(error_text(&cart), "Send between 1 and 20 items.");
}

fn first_part(stock: StockStatus, at_least: u32) -> catalog::Part {
    demo_catalog()
        .parts()
        .iter()
        .find(|p| p.stock == stock && p.quantity_available >= at_least)
        .expect("the demo catalog has such a part")
        .clone()
}

#[tokio::test]
async fn repeated_lines_are_checked_against_the_shelf_as_one_quantity() {
    let store = start(StoreConfig::default()).await;
    let part = first_part(StockStatus::InStock, 2);
    let half = part.quantity_available;
    let cart = store
        .call(
            "create_cart",
            json!({ "items": [{ "id": part.id, "quantity": half }, { "id": part.id, "quantity": 1 }] }),
        )
        .await;
    let text = error_text(&cart);
    assert!(text.starts_with("Cart not created."), "{text}");
    assert!(
        text.contains(&format!("Only {} on the shelf", part.quantity_available)),
        "{text}"
    );
}

#[tokio::test]
async fn repeated_lines_over_the_per_part_maximum_are_refused_not_clamped() {
    let store = start(StoreConfig::default()).await;
    let part = first_part(StockStatus::Backorder, 0);
    let cart = store
        .call(
            "create_cart",
            json!({ "items": [{ "id": part.id, "quantity": 60 }, { "id": part.id, "quantity": 60 }] }),
        )
        .await;
    let text = error_text(&cart);
    assert!(
        text.contains("120 times in total") && text.contains("most per part is 99"),
        "{text}"
    );
}

#[tokio::test]
async fn a_client_over_the_limit_gets_429_with_retry_after() {
    let store = start(StoreConfig {
        requests_per_minute: 3,
        ..StoreConfig::default()
    })
    .await;
    for _ in 0..3 {
        assert_eq!(
            store.rpc("tools/list", json!({})).await.status(),
            StatusCode::OK
        );
    }
    let limited = store.rpc("tools/list", json!({})).await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.headers().contains_key("retry-after"));
    let health = store
        .client
        .get(format!("{}/health", store.base))
        .send()
        .await
        .unwrap();
    assert_eq!(
        health.status(),
        StatusCode::OK,
        "health is not rate limited"
    );
}

#[tokio::test]
async fn an_unlisted_host_header_is_refused() {
    let store = start(StoreConfig::default()).await;
    let response = store
        .client
        .post(format!("{}/mcp", store.base))
        .header("host", "evil.example")
        .header("accept", "application/json, text/event-stream")
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

async fn search_error(store: &Running, arguments: Value) -> String {
    let result = store.call("search_products", arguments).await;
    error_text(&result).to_owned()
}

#[tokio::test]
async fn a_limit_outside_one_to_twenty_five_is_refused() {
    let store = start(StoreConfig::default()).await;
    for limit in [0, 26] {
        let text = search_error(&store, json!({ "limit": limit })).await;
        assert_eq!(text, "limit must be between 1 and 25.");
    }
}

#[tokio::test]
async fn a_query_of_more_than_twenty_words_is_refused_with_advice() {
    let store = start(StoreConfig::default()).await;
    let query = vec!["chain"; 21].join(" ");
    let text = search_error(&store, json!({ "query": query })).await;
    assert_eq!(
        text,
        "The query has more than 20 words. Keep the few that name the part."
    );
}

#[tokio::test]
async fn oversize_and_blank_text_inputs_are_refused() {
    let store = start(StoreConfig::default()).await;
    let long = "x".repeat(101);
    for (arguments, field) in [
        (json!({ "brand": long }), "brand"),
        (json!({ "fits": [long] }), "fits entry"),
    ] {
        let text = search_error(&store, arguments).await;
        assert_eq!(text, format!("{field} is longer than 100 characters."));
    }
    let text = search_error(&store, json!({ "fits": ["road", " "] })).await;
    assert!(text.starts_with("fits has an empty entry"), "{text}");
    let text = search_error(&store, json!({ "query": "x".repeat(201) })).await;
    assert_eq!(text, "query is longer than 200 characters.");
}

#[tokio::test]
async fn a_long_id_is_refused_and_never_echoed_whole() {
    let store = start(StoreConfig::default()).await;
    let id = "9".repeat(5_000);
    let product = store.call("get_product", json!({ "id": id })).await;
    assert_eq!(error_text(&product), "id is longer than 100 characters.");
    let cart = store
        .call("create_cart", json!({ "items": [{ "id": id }] }))
        .await;
    assert!(error_text(&cart).len() < 200);
    let unknown = "A".repeat(100);
    let missing = store.call("get_product", json!({ "id": unknown })).await;
    assert!(error_text(&missing).len() < 200, "{missing}");
}

#[tokio::test]
async fn a_request_body_over_64_kib_is_refused() {
    let store = start(StoreConfig::default()).await;
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "search_products", "arguments": { "query": "x".repeat(70_000) } }
    });
    let response = store
        .client
        .post(format!("{}/mcp", store.base))
        .header("accept", "application/json, text/event-stream")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

fn behind_a_proxy(requests_per_minute: u32) -> StoreConfig {
    StoreConfig {
        requests_per_minute,
        trust_forwarded_for: true,
        ..StoreConfig::default()
    }
}

#[tokio::test]
async fn rotating_the_leftmost_forwarded_address_does_not_escape_the_limit() {
    let store = start(behind_a_proxy(2)).await;
    let mut statuses = Vec::new();
    for client_chosen in 1..=4 {
        let header = format!("198.51.100.{client_chosen}, 203.0.113.9");
        statuses.push(store.rpc_with_header("x-forwarded-for", &header).await);
    }
    assert_eq!(
        statuses,
        [
            StatusCode::OK,
            StatusCode::OK,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::TOO_MANY_REQUESTS
        ]
    );
}

#[tokio::test]
async fn clients_are_told_apart_by_the_address_the_proxy_appended() {
    let store = start(behind_a_proxy(1)).await;
    for proxy_seen in ["203.0.113.1", "203.0.113.2"] {
        let header = format!("198.51.100.7, {proxy_seen}");
        assert_eq!(
            store.rpc_with_header("x-forwarded-for", &header).await,
            StatusCode::OK,
            "{proxy_seen}"
        );
    }
    let again = store
        .rpc_with_header("x-forwarded-for", "198.51.100.7, 203.0.113.1")
        .await;
    assert_eq!(again, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn an_unparseable_forwarded_address_falls_back_to_the_socket_peer() {
    let store = start(behind_a_proxy(1)).await;
    let first = store.rpc_with_header("x-forwarded-for", "not-an-ip").await;
    let second = store.rpc_with_header("x-forwarded-for", "also-bad").await;
    assert_eq!(
        (first, second),
        (StatusCode::OK, StatusCode::TOO_MANY_REQUESTS)
    );
}

#[tokio::test]
async fn forwarded_addresses_are_ignored_unless_trusted() {
    let store = start(StoreConfig {
        requests_per_minute: 1,
        ..StoreConfig::default()
    })
    .await;
    let first = store
        .rpc_with_header("x-forwarded-for", "198.51.100.1")
        .await;
    let second = store
        .rpc_with_header("x-forwarded-for", "198.51.100.2")
        .await;
    assert_eq!(
        (first, second),
        (StatusCode::OK, StatusCode::TOO_MANY_REQUESTS)
    );
}
