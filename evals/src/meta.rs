//! The `meta` block of a report: what produced the numbers, so a rerun can be compared.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::assistant::{DEFAULT_MAX_TOKENS, Plan, SYSTEM_PROMPT};
use crate::hash::sha256_hex;
use crate::mcp::ServerHandshake;
use crate::metrics::{BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED};

/// Git commit of the build, set by `build.rs`; absent when git could not tell.
const GIT_COMMIT: Option<&str> = option_env!("MCP_EVALS_GIT_COMMIT");

/// Settings of the assistant-level run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMeta {
    /// Model that answered.
    pub model: String,
    /// Sampling temperature that was sent; null when none was.
    pub temperature: Option<f64>,
    /// Cap on the length of each model reply, in tokens. Reports written before it was recorded
    /// read as the 1024 that was then fixed.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Most model turns per query.
    pub max_turns: u32,
    /// Runs per query.
    pub trials: u32,
    /// SHA-256 of the system prompt.
    pub system_prompt_sha256: String,
}

fn default_max_tokens() -> u32 {
    DEFAULT_MAX_TOKENS
}

impl AssistantMeta {
    /// The settings for a run of `model` under `plan`.
    #[must_use]
    pub fn new(model: &str, temperature: Option<f64>, max_tokens: u32, plan: Plan) -> Self {
        Self {
            model: model.to_owned(),
            temperature,
            max_tokens,
            max_turns: plan.max_turns,
            trials: plan.trials,
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
    /// Query kinds that were run; empty means every kind in the file.
    pub kinds: Vec<String>,
    /// Seed of the bootstrap confidence intervals.
    pub bootstrap_seed: u64,
    /// Resamples behind each bootstrap interval.
    pub bootstrap_resamples: usize,
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
    /// Query kinds that were run; empty means every kind in the file.
    pub kinds: &'a [String],
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
            kinds: settings.kinds.to_vec(),
            bootstrap_seed: BOOTSTRAP_SEED,
            bootstrap_resamples: BOOTSTRAP_RESAMPLES,
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
            kinds: &[],
        };

        let meta = Meta::new(
            &settings,
            &handshake,
            Some(AssistantMeta::new("m", None, 512, Plan::once(8))),
        );

        assert_eq!(meta.crate_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(
            meta.queries_sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(meta.server_info["name"], "s");
        assert_eq!(meta.protocol_version, "2025-06-18");
        let assistant = meta.assistant.expect("assistant meta");
        assert_eq!(assistant.system_prompt_sha256.len(), 64);
        assert_eq!((assistant.temperature, assistant.max_turns), (None, 8));
        assert_eq!(assistant.max_tokens, 512);
        assert_eq!(assistant.trials, 1);
        assert_eq!(meta.bootstrap_seed, BOOTSTRAP_SEED);
        assert_eq!(meta.bootstrap_resamples, 2000);
    }

    #[test]
    fn assistant_meta_round_trips_and_an_old_file_without_max_tokens_reads_as_1024() {
        let meta = AssistantMeta::new("m", Some(0.2), 300, Plan::once(4));
        let text = serde_json::to_string(&meta).unwrap();
        let back: AssistantMeta = serde_json::from_str(&text).unwrap();
        assert_eq!((back.max_tokens, back.temperature), (300, Some(0.2)));

        let old = r#"{"model":"m","temperature":null,"max_turns":8,"trials":3,
                      "system_prompt_sha256":"abc"}"#;
        let old: AssistantMeta = serde_json::from_str(old).unwrap();
        assert_eq!(old.max_tokens, 1024);
    }
}
