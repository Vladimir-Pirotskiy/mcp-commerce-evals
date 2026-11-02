//! A small MCP client for Streamable HTTP servers: initialize, list tools, call a tool.
//!
//! It accepts both reply styles the transport allows (a JSON body or a server-sent-event
//! stream) and carries a session id when the server hands one out, so it also works against
//! stateful servers.

use std::time::Duration;

use reqwest::header::ACCEPT;
use serde_json::{Value, json};

use crate::error::{Error, Result};

/// Protocol revision the client asks for. Servers may answer with another one.
const PROTOCOL_VERSION: &str = "2025-06-18";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

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

/// A connected MCP session.
pub struct McpClient {
    http: reqwest::Client,
    url: String,
    session: Option<String>,
    next_id: u64,
}

impl McpClient {
    /// Opens a session: `initialize` followed by `notifications/initialized`.
    pub async fn connect(url: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        let mut client = Self {
            http,
            url: url.to_owned(),
            session: None,
            next_id: 0,
        };
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "mcp-commerce-evals", "version": env!("CARGO_PKG_VERSION") },
        });
        client.request("initialize", params).await?;
        client.notify("notifications/initialized").await?;
        Ok(client)
    }

    /// The server's tool definitions, as MCP sends them (`name`, `description`, `inputSchema`).
    pub async fn list_tools(&mut self) -> Result<Vec<Value>> {
        let result = self.request("tools/list", json!({})).await?;
        Ok(result["tools"].as_array().cloned().unwrap_or_default())
    }

    /// Calls a tool and flattens the result.
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

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let response = self.post(&body).send().await?;
        let status = response.status();
        if let Some(session) = response.headers().get("mcp-session-id") {
            self.session = session.to_str().ok().map(str::to_owned);
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
        let message = if is_stream {
            find_in_stream(&text, id)?
        } else {
            serde_json::from_str(&text)?
        };
        if let Some(error) = message.get("error") {
            let reason = error["message"].as_str().unwrap_or("unknown error");
            return Err(Error::Protocol(format!("{method}: {reason}")));
        }
        Ok(message["result"].clone())
    }
}

/// The JSON-RPC reply with `id` inside a server-sent-event body.
fn find_in_stream(body: &str, id: u64) -> Result<Value> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
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
}
