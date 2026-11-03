# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [0.1.0] - 2026-11-03

First release.

### Added

- `reference-store`: an MCP server over Streamable HTTP with four tools (`search_products`,
  `get_product`, `check_availability`, `create_cart`) on a seeded catalog of 500 fictional bicycle
  parts, with money in dollars and whole cents.
- Rate limiting per client (fixed window, IPv6 per /64, `X-Forwarded-For` behind a trusted proxy),
  a bounded client table with a shared overflow bucket of ten times a client's limit, `Host` and
  `Origin` checks, a cap of 256 requests in flight, a 30-second request deadline, and shutdown on
  ctrl-c or SIGTERM that lets requests in flight finish.
- `--allowed-host` is validated at startup (a bare host with an optional port).
- `mcp-commerce-evals`: tool-level metrics (hit@1, hit@5, MRR, capped recall@10, nDCG@10, latency
  percentiles) with Wilson and seeded bootstrap 95% intervals, and an optional assistant-level run
  through the Anthropic Messages API with `--trials` and pass@k.
- 44 queries, including four `no_match` abstention queries, whose expected sets are re-derived from
  catalog attributes by `evals/tests/expected_sets.rs`.
- Gates `--min-hit5`, `--min-mrr`, `--min-accuracy`, exit codes 0, 1, 2 and 3, and a report `meta`
  block (versions, queries hash, server info, negotiated protocol, bootstrap seed, model).
- The MCP client follows the protocol version the server negotiates, reads event streams as they
  arrive, answers `ping`, and surfaces JSON-RPC errors from non-2xx bodies.
- CI: fmt, clippy with `-D warnings`, tests, docs, `cargo deny`, a tool-level eval job with score
  floors and an uploaded report; Dependabot for GitHub Actions.

### Changed

- Recall@10 divides by min(expected ids, 10) instead of the number of expected ids, so a query
  with more than ten right answers can reach 1.0.
- The assistant-level run retries request timeouts and failed connects like it retries HTTP 429.

### Added (results)

- Measured results: tool-level (`results/tool-level-2026-11-03.json`) and assistant-level runs of
  `claude-haiku-5-5` (3 trials) and `claude-sonnet-5-5` (1 trial), all from commit `e0f398e`, with
  the findings in the README.

### Fixed

- The assistant report no longer scores `mentions_expected` for `no_match` queries, which expect
  nothing and dragged the overall figure down; it is `n/a` in the table and `null` in the JSON.
- A short live run against `claude-haiku-5-5` showed that the API answers HTTP 400 "`temperature` is
  deprecated for this model", so `temperature` is sent only with `--temperature`.
- A run of the store with Claude Code (`claude -p`, Haiku, the store as its only server) found
  that the client warned on every `uint32` and `double` schema format schemars writes for Rust
  numbers; the store drops them (`schemas_use_only_standard_formats`). In that run the assistant
  searched, checked stock and created a cart for an expected chain in 5 turns.
- Tool results that carry only `structuredContent` reach the model as that JSON.
- `check_availability` merges duplicate lines before checking the shelf, as `create_cart` does.
