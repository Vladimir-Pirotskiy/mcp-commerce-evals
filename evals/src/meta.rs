//! The `meta` block of a report: what produced the numbers, so a rerun can be compared.

use serde::Serialize;
use serde_json::Value;

use crate::assistant::{SYSTEM_PROMPT, TEMPERATURE};
use crate::hash::sha256_hex;
use crate::mcp::ServerHandshake;

/// Git commit of the build, set by `build.rs`; absent when git could not tell.
const GIT_COMMIT: Option<&str> = option_env!("MCP_EVALS_GIT_COMMIT");

/// Settings of the assistant-level run.
#[derive(Debug, Clone, Serialize)]
pub struct AssistantMeta {
    /// Model that answered.
    pub model: String,
    /// Sampling temperature that was sent.
    pub temperature: f64,
    /// Most model turns per query.
    pub max_turns: u32,
    /// SHA-256 of the system prompt.
    pub system_prompt_sha256: String,
}

impl AssistantMeta {
    /// The settings for a run of `model` with at most `max_turns` turns per query.
    #[must_use]
    pub fn new(model: &str, max_turns: u32) -> Self {
        Self {
            model: model.to_owned(),
            temperature: TEMPERATURE,
            max_turns,
            system_prompt_sha256: sha256_hex(SYSTEM_PROMPT.as_bytes()),
        }
    }
}

/// What produced a report.
#[derive(Debug, Clone, Serialize)]
pub struct Meta {
    /// Version of this crate.
    pub crate_version: &'static str,
    /// Git commit the binary was built from (`+dirty` when the tree had changes).
    pub git_commit: Option<&'static str>,
    /// SHA-256 of the queries file, byte for byte.
    pub queries_sha256: String,
    /// How many results were asked for and scored.
    pub top_k: usize,
    /// Timed search calls per query.
    pub repeat: usize,
    /// Name of the search tool.
    pub search_tool: String,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// The server's `serverInfo` from `initialize`.
    pub server_info: Value,
    /// The protocol revision the server negotiated.
    pub protocol_version: String,
    /// Settings of the assistant-level run, when there was one.
    pub assistant: Option<AssistantMeta>,
}

/// The run settings that are not read from the server.
#[derive(Debug, Clone)]
pub struct RunSettings<'a> {
    /// Raw bytes of the queries file.
    pub queries_file: &'a [u8],
    /// Results asked for and scored.
    pub top_k: usize,
    /// Timed search calls per query.
    pub repeat: usize,
    /// Name of the search tool.
    pub search_tool: &'a str,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
}

impl Meta {
    /// Collects the settings of a run and what the server said about itself.
    #[must_use]
    pub fn new(
        settings: &RunSettings<'_>,
        handshake: &ServerHandshake,
        assistant: Option<AssistantMeta>,
    ) -> Self {
        Self {
            crate_version: env!("CARGO_PKG_VERSION"),
            git_commit: GIT_COMMIT,
            queries_sha256: sha256_hex(settings.queries_file),
            top_k: settings.top_k,
            repeat: settings.repeat,
            search_tool: settings.search_tool.to_owned(),
            timeout_secs: settings.timeout_secs,
            server_info: handshake.server_info.clone(),
            protocol_version: handshake.protocol_version.clone(),
            assistant,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn records_the_inputs_the_server_and_the_prompt_hash() {
        let handshake = ServerHandshake {
            server_info: json!({ "name": "s", "version": "1" }),
            protocol_version: "2025-06-18".to_owned(),
        };
        let settings = RunSettings {
            queries_file: b"abc",
            top_k: 10,
            repeat: 3,
            search_tool: "search_products",
            timeout_secs: 60,
        };

        let meta = Meta::new(&settings, &handshake, Some(AssistantMeta::new("m", 8)));

        assert_eq!(meta.crate_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(
            meta.queries_sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(meta.server_info["name"], "s");
        assert_eq!(meta.protocol_version, "2025-06-18");
        let assistant = meta.assistant.expect("assistant meta");
        assert_eq!(assistant.system_prompt_sha256.len(), 64);
        assert_eq!((assistant.temperature, assistant.max_turns), (0.0, 8));
    }
}
