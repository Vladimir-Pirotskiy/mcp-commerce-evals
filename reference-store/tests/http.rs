//! The store over real HTTP: the four tools, their schemas, the rate limit and the host check.

use std::sync::Arc;

use catalog::demo_catalog;
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

#[tokio::test]
async fn tool_errors_are_reported_to_the_assistant_not_as_http_errors() {
    let store = start(StoreConfig::default()).await;
    let missing = store
        .call("get_product", json!({ "id": "NOPE-0000" }))
        .await;
    assert_eq!(missing["isError"], true);
    let cart = store.call("create_cart", json!({ "items": [] })).await;
    assert_eq!(cart["isError"], true);
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
