//! Evaluate any MCP store from a queries file.
//!
//! [`tool_level`] calls the store's search tool for every query and scores the ranking (hit@1,
//! hit@5, MRR, latency). [`assistant`] lets a model shop through the store's tools and checks
//! whether its final answer names the right part. [`mcp`] is the small client both use.

pub mod assistant;
pub mod error;
pub mod mcp;
pub mod metrics;
pub mod queries;
pub mod report;
pub mod tool_level;
