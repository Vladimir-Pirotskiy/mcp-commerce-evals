//! The MCP client against a hand-rolled HTTP server, so each test controls exactly what is
//! written to the socket: negotiated versions, error bodies and event streams that stay open.

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mcp_commerce_evals::mcp::McpClient;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

const TIMEOUT: Duration = Duration::from_secs(10);

/// What the raw server answers to one request.
enum Reply {
    /// 200 with a JSON body.
    Json(Value),
    /// A bare status with a text body.
    Status(u16, String),
    /// 202 with no body.
    Accepted,
    /// 200 `text/event-stream` that writes these events and then stays open.
    OpenStream(Vec<String>),
}

/// One request as the server saw it.
#[derive(Debug, Clone)]
struct Seen {
    /// `mcp-protocol-version` header.
    version: Option<String>,
    body: Value,
}

type Handler = dyn Fn(&Value) -> Reply + Send + Sync;

struct Raw {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    task: JoinHandle<()>,
}

impl Drop for Raw {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Raw {
    fn url(&self) -> String {
        format!("{}/mcp", self.base)
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().expect("lock").clone()
    }

    fn methods(&self, method: &str) -> Vec<Seen> {
        self.seen()
            .into_iter()
            .filter(|s| s.body["method"] == method)
            .collect()
    }
}

async fn spawn_raw(handler: impl Fn(&Value) -> Reply + Send + Sync + 'static) -> Raw {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
    let base = format!("http://{}", listener.local_addr().expect("local address"));
    let handler: Arc<Handler> = Arc::new(handler);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(connection(stream, handler.clone(), log.clone()));
        }
    });
    Raw { base, seen, task }
}

/// Reads one request: the head, then `content-length` bytes of body.
async fn read_request(stream: &mut TcpStream, buf: &mut Vec<u8>) -> Option<Seen> {
    let head_end = loop {
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        let mut chunk = [0_u8; 4096];
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_lowercase();
    let header = |name: &str| {
        head.lines()
            .find_map(|l| l.strip_prefix(&format!("{name}:")))
            .map(|v| v.trim().to_owned())
    };
    let length: usize = header("content-length")?.parse().ok()?;
    while buf.len() < head_end + length {
        let mut chunk = [0_u8; 4096];
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = serde_json::from_slice(&buf[head_end..head_end + length]).unwrap_or(Value::Null);
    buf.drain(..head_end + length);
    Some(Seen {
        version: header("mcp-protocol-version"),
        body,
    })
}

async fn connection(mut stream: TcpStream, handler: Arc<Handler>, log: Arc<Mutex<Vec<Seen>>>) {
    let mut buf = Vec::new();
    while let Some(request) = read_request(&mut stream, &mut buf).await {
        let reply = handler(&request.body);
        log.lock().expect("lock").push(request);
        let written = match reply {
            Reply::Json(body) => write_body(200, "application/json", &body.to_string()),
            Reply::Status(code, text) => write_body(code, "text/plain", &text),
            Reply::Accepted => write_body(202, "text/plain", ""),
            Reply::OpenStream(events) => {
                let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                            transfer-encoding: chunked\r\n\r\n";
                let mut out = head.to_owned();
                for event in events {
                    let _ = write!(out, "{:x}\r\n{event}\r\n", event.len());
                }
                // The terminating chunk is never sent: the stream stays open.
                out
            }
        };
        if stream.write_all(written.as_bytes()).await.is_err() {
            return;
        }
    }
}

fn write_body(code: u16, content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {code} X\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
}

fn rpc(id: &Value, result: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// A server that negotiates `version` and answers `tools/list` with an empty list.
fn negotiating(version: &'static str) -> impl Fn(&Value) -> Reply + Send + Sync {
    move |body| match body["method"].as_str() {
        Some("initialize") => Reply::Json(rpc(
            &body["id"],
            &json!({ "protocolVersion": version, "serverInfo": { "name": "raw" } }),
        )),
        Some("tools/list") => Reply::Json(rpc(&body["id"], &json!({ "tools": [] }))),
        _ => Reply::Accepted,
    }
}

#[tokio::test]
async fn initialize_carries_no_protocol_header_and_later_requests_carry_the_negotiated_one() {
    let server = spawn_raw(negotiating("2025-03-26")).await;
    let mut mcp = McpClient::connect(&server.url(), TIMEOUT).await.unwrap();

    mcp.list_tools().await.unwrap();
    mcp.close().await;

    let seen = |method: &str| server.methods(method)[0].clone();
    let initialize = seen("initialize");
    assert_eq!(initialize.version, None);
    assert_eq!(initialize.body["params"]["protocolVersion"], "2025-06-18");
    assert_eq!(
        seen("notifications/initialized").version.as_deref(),
        Some("2025-03-26")
    );
    assert_eq!(seen("tools/list").version.as_deref(), Some("2025-03-26"));
}

#[tokio::test]
async fn a_protocol_version_the_client_does_not_support_is_an_error_naming_both_sides() {
    for version in ["1999-01-01", ""] {
        let server = spawn_raw(negotiating(version)).await;

        let error = match McpClient::connect(&server.url(), TIMEOUT).await {
            Ok(_) => panic!("connected with {version:?}"),
            Err(error) => error.to_string(),
        };

        assert!(error.contains("2025-06-18"), "{error}");
        assert!(error.contains(&format!("{version:?}")), "{error}");
    }
}

#[tokio::test]
async fn a_non_2xx_reply_carries_the_json_rpc_error_message_from_its_body() {
    let server = spawn_raw(|body| match body["method"].as_str() {
        Some("initialize") => Reply::Json(rpc(
            &body["id"],
            &json!({ "protocolVersion": "2025-06-18" }),
        )),
        Some("tools/list") => Reply::Status(
            400,
            json!({ "jsonrpc": "2.0", "id": null,
                    "error": { "code": -32600, "message": "Session expired upstream" } })
            .to_string(),
        ),
        _ => Reply::Accepted,
    })
    .await;
    let mut mcp = McpClient::connect(&server.url(), TIMEOUT).await.unwrap();

    let error = mcp.list_tools().await.unwrap_err().to_string();

    assert!(error.contains("400"), "{error}");
    assert!(error.contains("Session expired upstream"), "{error}");
}

fn event(message: &Value) -> String {
    format!("event: message\ndata: {message}\n\n")
}

#[tokio::test]
async fn an_event_stream_that_stays_open_is_answered_as_soon_as_the_reply_arrives() {
    let server = spawn_raw(|body| match body["method"].as_str() {
        Some("initialize") => Reply::Json(rpc(
            &body["id"],
            &json!({ "protocolVersion": "2025-06-18" }),
        )),
        Some("tools/list") => Reply::OpenStream(vec![
            event(&json!({ "jsonrpc": "2.0", "method": "notifications/progress" })),
            event(&rpc(&body["id"], &json!({ "tools": [{ "name": "t" }] }))),
        ]),
        _ => Reply::Accepted,
    })
    .await;
    let mut mcp = McpClient::connect(&server.url(), TIMEOUT).await.unwrap();

    let tools = tokio::time::timeout(Duration::from_secs(3), mcp.list_tools())
        .await
        .expect("returned without waiting for the stream to close")
        .unwrap();

    assert_eq!(tools.len(), 1);
}

#[tokio::test]
async fn a_server_ping_seen_in_the_stream_is_answered_with_an_empty_result() {
    let server = spawn_raw(|body| match body["method"].as_str() {
        Some("initialize") => Reply::Json(rpc(
            &body["id"],
            &json!({ "protocolVersion": "2025-06-18" }),
        )),
        Some("tools/list") => Reply::OpenStream(vec![
            event(&json!({ "jsonrpc": "2.0", "id": "ping-1", "method": "ping" })),
            event(&rpc(&body["id"], &json!({ "tools": [] }))),
        ]),
        _ => Reply::Accepted,
    })
    .await;
    let mut mcp = McpClient::connect(&server.url(), TIMEOUT).await.unwrap();

    mcp.list_tools().await.unwrap();

    let pong = server
        .seen()
        .into_iter()
        .find(|s| s.body["id"] == "ping-1")
        .expect("the client answered the ping");
    assert_eq!(pong.body["result"], json!({}));
    assert!(pong.body.get("method").is_none());
}

#[tokio::test]
async fn list_tools_follows_next_cursor_to_the_last_page() {
    let server = spawn_raw(|body| match body["method"].as_str() {
        Some("initialize") => Reply::Json(rpc(
            &body["id"],
            &json!({ "protocolVersion": "2025-06-18", "serverInfo": { "name": "raw" } }),
        )),
        Some("tools/list") => {
            let result = match body["params"]["cursor"].as_str() {
                None => json!({ "tools": [{ "name": "a" }], "nextCursor": "c1" }),
                Some("c1") => json!({ "tools": [{ "name": "b" }], "nextCursor": "c2" }),
                Some(_) => json!({ "tools": [{ "name": "c" }] }),
            };
            Reply::Json(rpc(&body["id"], &result))
        }
        _ => Reply::Accepted,
    })
    .await;
    let mut mcp = McpClient::connect(&server.url(), TIMEOUT).await.unwrap();

    let tools = mcp.list_tools().await.unwrap();

    let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["a", "b", "c"]);
    assert_eq!(server.methods("tools/list").len(), 3);
}

#[tokio::test]
async fn list_tools_gives_up_on_a_server_that_never_stops_paginating() {
    let server = spawn_raw(|body| match body["method"].as_str() {
        Some("initialize") => Reply::Json(rpc(
            &body["id"],
            &json!({ "protocolVersion": "2025-06-18", "serverInfo": { "name": "raw" } }),
        )),
        Some("tools/list") => Reply::Json(rpc(
            &body["id"],
            &json!({ "tools": [{ "name": "a" }], "nextCursor": "again" }),
        )),
        _ => Reply::Accepted,
    })
    .await;
    let mut mcp = McpClient::connect(&server.url(), TIMEOUT).await.unwrap();

    let error = mcp.list_tools().await.unwrap_err().to_string();

    assert!(error.contains("next page"), "{error}");
    assert_eq!(
        server.methods("tools/list").len(),
        mcp_commerce_evals::mcp::MAX_TOOL_PAGES
    );
}
