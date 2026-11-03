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

/// Protocol revision the client asks for. Servers may answer with another one.
const PROTOCOL_VERSION: &str = "2025-06-18";

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
}

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
        };
        client.initialize().await?;
        Ok(client)
    }

    /// `initialize` followed by `notifications/initialized`, on a client without a session.
    async fn initialize(&mut self) -> Result<()> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "mcp-commerce-evals", "version": env!("CARGO_PKG_VERSION") },
        });
        let Exchange::Done(reply) = self.exchange("initialize", &params).await? else {
            return Err(Error::Protocol("initialize: HTTP 404".to_owned()));
        };
        self.handshake = ServerHandshake {
            server_info: reply["serverInfo"].clone(),
            protocol_version: reply["protocolVersion"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
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
            .header("mcp-protocol-version", PROTOCOL_VERSION)
            .header("mcp-session-id", session)
            .send()
            .await;
    }

    /// What the server reported when the session was opened.
    #[must_use]
    pub fn handshake(&self) -> &ServerHandshake {
        &self.handshake
    }

    /// The server's tool definitions, as MCP sends them (`name`, `description`, `inputSchema`).
    ///
    /// # Errors
    ///
    /// Fails when the request or the JSON-RPC reply fails.
    pub async fn list_tools(&mut self) -> Result<Vec<Value>> {
        let result = self.request("tools/list", json!({})).await?;
        Ok(result["tools"].as_array().cloned().unwrap_or_default())
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
            .header("mcp-protocol-version", PROTOCOL_VERSION)
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
            return Err(Error::Protocol(format!("{method}: HTTP {status}")));
        }
        let is_stream = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        let text = response.text().await?;
        let message = parse_reply(&text, is_stream, id)?;
        reply_result(method, id, &message).map(Exchange::Done)
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

/// The data of each server-sent event: its `data:` lines joined with newlines, as the SSE
/// format defines, so a JSON message split over several lines parses.
fn event_data(body: &str) -> Vec<String> {
    let mut events = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in body.lines() {
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

/// The JSON-RPC reply with `id` inside a server-sent-event body.
fn find_in_stream(body: &str, id: u64) -> Result<Value> {
    event_data(body)
        .iter()
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
