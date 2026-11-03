//! A small MCP client for Streamable HTTP servers: initialize, list tools, call a tool.
//!
//! It accepts both reply styles the transport allows (a JSON body or a server-sent-event
//! stream) and carries a session id when the server hands one out, so it also works against
//! stateful servers.

use std::time::Duration;

use reqwest::header::ACCEPT;
use serde::Serialize;
use serde_json::{Value, json};

use crate::error::{Error, Result};

/// Protocol revisions this client speaks over Streamable HTTP, newest first. A server may
/// negotiate any of them, and `mcp-protocol-version` follows its choice.
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];
/// The revision requested in `initialize`: the newest one listed. The client uses only the part
/// of the protocol every listed revision shares (`initialize`, `tools/list`, `tools/call`,
/// `ping`, JSON and event-stream replies, session ids); its tests run it at this revision against
/// the in-process reference store, which grants it. Optional features added since (tasks,
/// elicitation, sampling) are not used and not offered in `capabilities`.
const REQUESTED_PROTOCOL_VERSION: &str = SUPPORTED_PROTOCOL_VERSIONS[0];

/// What a tool call returned.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    /// The server flagged the call as failed (`isError`).
    pub is_error: bool,
    /// The `structuredContent` object, when the tool returns one.
    pub structured: Option<Value>,
    /// The text content blocks joined together.
    pub text: String,
}

/// What the server said about itself in its `initialize` reply.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ServerHandshake {
    /// The server's `serverInfo` object (name and version), `null` when it sent none.
    pub server_info: Value,
    /// The protocol revision the server negotiated, empty when it named none.
    pub protocol_version: String,
}

/// A connected MCP session.
pub struct McpClient {
    http: reqwest::Client,
    url: String,
    session: Option<String>,
    next_id: u64,
    handshake: ServerHandshake,
    /// Sent as `mcp-protocol-version`: the requested revision until the server picks one.
    protocol_header: &'static str,
}

/// Most `tools/list` pages followed before the client gives up on a server that never stops
/// paginating.
pub const MAX_TOOL_PAGES: usize = 20;

impl McpClient {
    /// Opens a session: `initialize` followed by `notifications/initialized`. Every request,
    /// including the ones made later, gives up after `timeout`.
    ///
    /// # Errors
    ///
    /// Fails when the server is unreachable or does not answer `initialize` as MCP requires.
    pub async fn connect(url: &str, timeout: Duration) -> Result<Self> {
        let http = reqwest::Client::builder().timeout(timeout).build()?;
        let mut client = Self {
            http,
            url: url.to_owned(),
            session: None,
            next_id: 0,
            handshake: ServerHandshake {
                server_info: Value::Null,
                protocol_version: String::new(),
            },
            protocol_header: REQUESTED_PROTOCOL_VERSION,
        };
        client.initialize().await?;
        Ok(client)
    }

    /// `initialize` followed by `notifications/initialized`, on a client without a session.
    async fn initialize(&mut self) -> Result<()> {
        self.protocol_header = REQUESTED_PROTOCOL_VERSION;
        let params = json!({
            "protocolVersion": REQUESTED_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "mcp-commerce-evals", "version": env!("CARGO_PKG_VERSION") },
        });
        let Exchange::Done(reply) = self.exchange("initialize", &params).await? else {
            return Err(Error::Protocol("initialize: HTTP 404".to_owned()));
        };
        let negotiated = reply["protocolVersion"].as_str().unwrap_or_default();
        self.protocol_header = SUPPORTED_PROTOCOL_VERSIONS
            .into_iter()
            .find(|supported| *supported == negotiated)
            .ok_or_else(|| {
                Error::Protocol(format!(
                    "the server negotiated protocol version {negotiated:?}; this client supports {}",
                    SUPPORTED_PROTOCOL_VERSIONS.join(", ")
                ))
            })?;
        self.handshake = ServerHandshake {
            server_info: reply["serverInfo"].clone(),
            protocol_version: negotiated.to_owned(),
        };
        self.notify("notifications/initialized").await
    }

    /// Ends the session politely: a DELETE with the session id, which a stateless server may
    /// refuse with 405. Best effort, since the results are already in hand.
    pub async fn close(self) {
        let Some(session) = &self.session else {
            return;
        };
        let _ = self
            .http
            .delete(&self.url)
            .header("mcp-protocol-version", self.protocol_header)
            .header("mcp-session-id", session)
            .send()
            .await;
    }

    /// What the server reported when the session was opened.
    #[must_use]
    pub fn handshake(&self) -> &ServerHandshake {
        &self.handshake
    }

    /// The server's tool definitions, as MCP sends them (`name`, `description`, `inputSchema`),
    /// following `nextCursor` through every page, up to [`MAX_TOOL_PAGES`].
    ///
    /// # Errors
    ///
    /// Fails when a request or the JSON-RPC reply fails, or when the server still offers a next
    /// page after [`MAX_TOOL_PAGES`] pages.
    pub async fn list_tools(&mut self) -> Result<Vec<Value>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_TOOL_PAGES {
            let params = cursor
                .as_ref()
                .map_or_else(|| json!({}), |cursor| json!({ "cursor": cursor }));
            let result = self.request("tools/list", params).await?;
            tools.extend(result["tools"].as_array().cloned().unwrap_or_default());
            match result["nextCursor"].as_str().filter(|c| !c.is_empty()) {
                Some(next) => cursor = Some(next.to_owned()),
                None => return Ok(tools),
            }
        }
        Err(Error::Protocol(format!(
            "tools/list still had a next page after {MAX_TOOL_PAGES} pages; giving up"
        )))
    }

    /// Calls a tool and flattens the result.
    ///
    /// # Errors
    ///
    /// Fails when the request or the JSON-RPC reply fails.
    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<ToolOutput> {
        let result = self
            .request(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
            )
            .await?;
        let text = result["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
            .join("\n");
        Ok(ToolOutput {
            is_error: result["isError"].as_bool().unwrap_or(false),
            structured: result
                .get("structuredContent")
                .filter(|v| !v.is_null())
                .cloned(),
            text,
        })
    }

    fn post(&self, body: &Value) -> reqwest::RequestBuilder {
        let mut request = self
            .http
            .post(&self.url)
            .header(ACCEPT, "application/json, text/event-stream")
            .header("mcp-protocol-version", self.protocol_header)
            .json(body);
        if let Some(session) = &self.session {
            request = request.header("mcp-session-id", session);
        }
        request
    }

    async fn notify(&mut self, method: &str) -> Result<()> {
        let response = self
            .post(&json!({ "jsonrpc": "2.0", "method": method }))
            .send()
            .await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(Error::Protocol(format!(
                "{method}: HTTP {}",
                response.status()
            )))
        }
    }

    /// One request. When the server forgets the session (HTTP 404 on a request that carried
    /// one), the session is opened again once and the request retried.
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let had_session = self.session.is_some();
        match self.exchange(method, &params).await? {
            Exchange::Done(result) => Ok(result),
            Exchange::SessionGone if had_session => {
                self.session = None;
                self.initialize().await?;
                match self.exchange(method, &params).await? {
                    Exchange::Done(result) => Ok(result),
                    Exchange::SessionGone => Err(Error::Protocol(format!(
                        "{method}: HTTP 404 after a new session"
                    ))),
                }
            }
            Exchange::SessionGone => Err(Error::Protocol(format!("{method}: HTTP 404"))),
        }
    }

    async fn exchange(&mut self, method: &str, params: &Value) -> Result<Exchange> {
        self.next_id += 1;
        let id = self.next_id;
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let sent_session = self.session.is_some();
        let response = self.post(&body).send().await?;
        let status = response.status();
        // A header that is not text keeps the id we have rather than clearing the session.
        if let Some(session) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            self.session = Some(session.to_owned());
        }
        if status == reqwest::StatusCode::NOT_FOUND && sent_session {
            return Ok(Exchange::SessionGone);
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Protocol(http_failure(method, status, &body)));
        }
        let is_stream = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        let message = if is_stream {
            self.read_stream(response, id, method).await?
        } else {
            parse_reply(&response.text().await?, false, id)?
        };
        reply_result(method, id, &message).map(Exchange::Done)
    }

    /// Reads an event stream chunk by chunk and returns the reply to `id` as soon as it is
    /// complete, without waiting for the server to close the stream. A `ping` request from the
    /// server on the way is answered; other server requests are not supported and are ignored.
    async fn read_stream(
        &self,
        mut response: reqwest::Response,
        id: u64,
        method: &str,
    ) -> Result<Value> {
        let mut events = EventSplitter::default();
        loop {
            let chunk = response.chunk().await?;
            let data = match &chunk {
                Some(bytes) => events.push(bytes),
                None => events.finish(),
            };
            for message in data
                .iter()
                .filter_map(|data| serde_json::from_str::<Value>(data).ok())
            {
                if message.get("method").is_none() && message["id"] == id {
                    return Ok(message);
                }
                if message["method"] == "ping" && message.get("id").is_some() {
                    // Best effort: a failed pong must not hide the reply we are waiting for.
                    let _ = self
                        .post(&json!({ "jsonrpc": "2.0", "id": message["id"], "result": {} }))
                        .send()
                        .await;
                }
            }
            if chunk.is_none() {
                return Err(Error::Protocol(format!(
                    "{method}: the event stream ended without a reply with id {id}"
                )));
            }
        }
    }
}

/// The failure text for a non-2xx reply: the status, plus the JSON-RPC error message when the
/// body carries one, since that is what says why.
fn http_failure(method: &str, status: reqwest::StatusCode, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_owned));
    match detail {
        Some(message) => format!("{method}: HTTP {status}: {message}"),
        None => format!("{method}: HTTP {status}"),
    }
}

/// What one request came to.
enum Exchange {
    /// The `result` of the reply.
    Done(Value),
    /// The server answered 404 to a request that carried a session id.
    SessionGone,
}

/// The JSON-RPC message answering `id` in a reply body (plain JSON or an event stream).
///
/// A JSON array is a batch, which this client never asks for, so it is an error.
fn parse_reply(body: &str, is_stream: bool, id: u64) -> Result<Value> {
    if is_stream {
        return find_in_stream(body, id);
    }
    let message: Value = serde_json::from_str(body)?;
    if message.is_array() {
        return Err(Error::Protocol(
            "the server replied with a JSON-RPC batch, which this client does not support"
                .to_owned(),
        ));
    }
    Ok(message)
}

/// The `result` of a JSON-RPC reply to request `id`, or the reason it is not usable.
fn reply_result(method: &str, id: u64, message: &Value) -> Result<Value> {
    if message["id"] != id {
        return Err(Error::Protocol(format!(
            "{method}: the reply has id {}, not {id}",
            message["id"]
        )));
    }
    if let Some(error) = message.get("error") {
        return Err(Error::Rpc {
            method: method.to_owned(),
            code: error["code"].as_i64().unwrap_or_default(),
            message: error["message"]
                .as_str()
                .unwrap_or("unknown error")
                .to_owned(),
        });
    }
    message
        .get("result")
        .cloned()
        .ok_or_else(|| Error::Protocol(format!("{method}: the reply has neither result nor error")))
}

/// The data of each server-sent event in `text`: its `data:` lines joined with newlines, as the
/// SSE format defines, so a JSON message split over several lines parses.
fn event_data(text: &str) -> Vec<String> {
    let mut events = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            if !current.is_empty() {
                events.push(current.join("\n"));
                current.clear();
            }
        } else if let Some(data) = line.strip_prefix("data:") {
            current.push(data.strip_prefix(' ').unwrap_or(data));
        }
    }
    if !current.is_empty() {
        events.push(current.join("\n"));
    }
    events
}

/// Cuts a byte stream into server-sent events as chunks arrive. Bytes are buffered until a blank
/// line closes an event, so neither a split UTF-8 character nor a split line is misread.
#[derive(Default)]
struct EventSplitter {
    buffer: Vec<u8>,
}

impl EventSplitter {
    /// Adds a chunk and returns the data of every event it completed.
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some((end, separator)) = event_end(&self.buffer) {
            let block: Vec<u8> = self.buffer.drain(..end + separator).take(end).collect();
            out.extend(event_data(&String::from_utf8_lossy(&block)));
        }
        out
    }

    /// The data of a last event the server did not close with a blank line.
    fn finish(&mut self) -> Vec<String> {
        let rest = std::mem::take(&mut self.buffer);
        event_data(&String::from_utf8_lossy(&rest))
    }
}

/// Where the first complete event in `buffer` ends, and how long its blank-line separator is.
fn event_end(buffer: &[u8]) -> Option<(usize, usize)> {
    let find = |separator: &[u8]| {
        buffer
            .windows(separator.len())
            .position(|w| w == separator)
            .map(|at| (at, separator.len()))
    };
    match (find(b"\n\n"), find(b"\r\n\r\n")) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

/// The JSON-RPC reply with `id` inside a complete server-sent-event body.
fn find_in_stream(body: &str, id: u64) -> Result<Value> {
    let mut events = EventSplitter::default();
    let mut data = events.push(body.as_bytes());
    data.extend(events.finish());
    data.iter()
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .find(|message| message["id"] == id)
        .ok_or_else(|| Error::Protocol(format!("no reply with id {id} in the event stream")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_reply_among_other_events() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n\
                    event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"ok\":true}}\n\n";
        let message = find_in_stream(body, 7).unwrap();
        assert_eq!(message["result"]["ok"], true);
        assert!(find_in_stream(body, 8).is_err());
    }

    #[test]
    fn a_message_split_over_several_data_lines_is_joined_per_event() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\ndata: \"id\":4,\ndata: \"result\":{\"ok\":true}}\n\n";
        let message = find_in_stream(body, 4).unwrap();
        assert_eq!(message["result"]["ok"], true);
    }

    #[test]
    fn events_split_across_chunks_and_inside_a_character_are_reassembled() {
        let mut events = EventSplitter::default();
        let bytes =
            "data: {\"id\":1,\"result\":\"caf\u{e9}\"}\r\n\r\ndata: {\"id\":2}\n\n".as_bytes();
        let cut = bytes.iter().position(|b| *b == 0xc3).unwrap() + 1;
        assert_eq!(events.push(&bytes[..cut]), Vec::<String>::new());
        let done = events.push(&bytes[cut..]);
        assert_eq!(done, ["{\"id\":1,\"result\":\"caf\u{e9}\"}", "{\"id\":2}"]);
        assert_eq!(events.push(b"data: {\"id\":3}"), Vec::<String>::new());
        assert_eq!(events.finish(), ["{\"id\":3}"]);
    }

    #[test]
    fn a_non_2xx_failure_names_the_status_and_the_rpc_message_when_there_is_one() {
        let status = reqwest::StatusCode::BAD_REQUEST;
        let rpc = r#"{"error":{"code":-32600,"message":"bad session"}}"#;
        assert_eq!(
            http_failure("m", status, rpc),
            "m: HTTP 400 Bad Request: bad session"
        );
        assert_eq!(
            http_failure("m", status, "<html>"),
            "m: HTTP 400 Bad Request"
        );
    }

    #[test]
    fn a_batch_reply_is_an_error() {
        let error = parse_reply("[{\"id\":1,\"result\":{}}]", false, 1).unwrap_err();
        assert!(error.to_string().contains("batch"), "{error}");
        assert!(parse_reply("{\"id\":1,\"result\":{}}", false, 1).is_ok());
    }

    #[test]
    fn reply_without_result_or_error_is_an_error() {
        let reply = json!({ "jsonrpc": "2.0", "id": 3 });
        let error = reply_result("tools/call", 3, &reply).unwrap_err();
        assert!(error.to_string().contains("neither result nor error"));
    }

    #[test]
    fn rpc_error_keeps_its_code_in_the_message() {
        let reply = json!({ "id": 3, "error": { "code": -32602, "message": "bad params" } });
        let error = reply_result("tools/call", 3, &reply).unwrap_err();
        assert!(error.to_string().contains("-32602"), "{error}");
        assert!(error.to_string().contains("bad params"), "{error}");
    }

    #[test]
    fn reply_with_another_id_is_rejected() {
        let reply = json!({ "id": 9, "result": {} });
        assert!(reply_result("tools/call", 3, &reply).is_err());
        assert!(reply_result("tools/call", 9, &reply).is_ok());
    }
}
