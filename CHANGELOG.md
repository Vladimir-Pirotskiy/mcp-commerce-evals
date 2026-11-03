# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.1] - 2026-11-03

### Fixed

- The release binary read its default queries from a build-time absolute path, so it failed
  outside a checkout. `queries.json` is now compiled in and used when `--queries` is not given;
  `--queries <path>` still overrides, and `meta.queries_sha256` hashes the bytes the run used.
- A 200 reply from the Messages API that is not JSON (or whose body cannot be read) was read as
  an empty reply. The query now records an error and the run exits 2.
- `MCP_EVALS_GIT_COMMIT` went stale after a commit on the same branch; the build script now
  watches HEAD, the index, `packed-refs` and the checked-out ref, in linked worktrees too.
- One `X-Forwarded-For` line that is not UTF-8 made the rate limiter name every client by the
  proxy's address. Entries are counted on raw bytes and only the trusted entry must be UTF-8 and
  an address; otherwise the client is named by the socket peer.
- The client asked for protocol revision 2025-06-18 although it lists 2025-11-25 as supported;
  it now asks for the newest revision it lists (the in-process store grants it).
- The assistant table's count column is headed `runs`, because it counts runs, not queries.

### Changed

- The README is 163 lines; the store, evals and development reference and the result tables are
  in `docs/`.
- The README states the real dependency rule: a 14-day cooldown, with an exception for security
  fixes (RUSTSEC-2026-0382 and -0383). Dependabot has `cooldown: default-days: 14` for both
  ecosystems.
- The release workflow checks out without persisting credentials, runs the tests before building,
  smoke-tests the tarball from an empty directory, and publishes the CHANGELOG section of the tag's
  version as the release notes.
- The bootstrap interval test is pinned tightly enough to fail on another seed, and the 504 test
  proves the handler future is dropped.

### Security

- `reference-store` closes a connection that has not sent its request line and headers within 10
  seconds (`--header-read-timeout-secs`) and refuses connections beyond 1024 open
  (`--max-connections`); before, a partial request stayed open indefinitely and idle connections
  were unbounded. It now serves through hyper directly, with graceful shutdown kept.
  `hyper` and `hyper-util` were already in the lockfile through axum.

## [0.1.0] - 2026-11-03

First release.

### Added

- `reference-store`: an MCP server over Streamable HTTP with four tools (`search_products`,
  `get_product`, `check_availability`, `create_cart`) on a seeded catalog of 500 fictional bicycle
  parts, with money in dollars and whole cents. `--seed` and `--parts` choose the catalog; `--parts`
  is capped at 2000 and a request the generator cannot fill is an error, never a shorter catalog.
  The catalog refuses prices that are not whole cents from $0.01 to $10,000, and cart totals
  saturate instead of wrapping.
- Rate limiting per client (fixed window, IPv6 per /64, `X-Forwarded-For` behind a trusted proxy),
  a bounded client table that evicts expired windows before a shared overflow bucket of ten times a
  client's limit takes newcomers, `Host` and `Origin` checks (case-insensitive), a cap of 256
  requests in flight, a 30-second request deadline, and shutdown on ctrl-c or SIGTERM that lets
  requests in flight finish. `StoreConfig` takes non-zero request and in-flight limits.
- `--allowed-host` is validated at startup (a bare host with an optional port).
- `mcp-commerce-evals`: tool-level metrics (hit@1, hit@5, MRR, capped recall@10, nDCG@10, latency
  percentiles) with Wilson and seeded bootstrap 95% intervals, and an optional assistant-level run
  through the Anthropic Messages API with `--trials`, `--max-tokens` and pass@k.
- Bootstrap intervals read `n/a` under 10 queries or for a constant sample; with `--trials` the
  accuracy interval resamples whole queries. MRR is capped at the scored depth. Queries whose
  ranking changes between `--repeat` calls are flagged. `tools/list` follows `nextCursor`. Ids
  that normalize to nothing match nothing. Errors of `no_match` queries count toward exit status 2.
- Each assistant tool call is stored with its arguments; the `meta` block records temperature and
  `max_tokens`. `--temperature` and `--max-tokens` require `--assistant`; met gates are printed.
- 44 queries, including four `no_match` abstention queries, whose expected sets are re-derived from
  catalog attributes by `evals/tests/expected_sets.rs`.
- Gates `--min-hit5`, `--min-mrr`, `--min-accuracy`, exit codes 0, 1, 2 and 3, and a report `meta`
  block (versions, queries hash, server info, negotiated protocol, bootstrap seed, model).
- The MCP client follows the protocol version the server negotiates, reads event streams as they
  arrive, answers `ping`, and surfaces JSON-RPC errors from non-2xx bodies. The assistant-level run
  retries request timeouts and failed connects like HTTP 429.
- A README threat model, and measured results: tool-level (`results/tool-level-2026-11-03.json`)
  and assistant-level runs of `claude-haiku-5-5` (3 trials) and `claude-sonnet-5-5` (1 trial).
- CI: fmt, clippy with `-D warnings`, tests, docs, `cargo deny` (licenses, bans, sources and
  advisories, cargo-deny pinned), a tool-level eval job with score floors and an uploaded report,
  on pushes to `main` and `release/**` and on pull requests; Dependabot for GitHub Actions and
  Cargo. A test keeps the floors in step with the committed results.
- `mcp-commerce-evals render <results.json>` prints the Markdown report for a saved results file,
  scored again by the current report code, with no network call. Older files still parse.
- The assistant report counts runs that finished without an `ANSWER:` line (`no_answer_line`)
  apart from runs that committed a wrong id (`wrong_id`), in the JSON (both default to 0 in older
  files) and as table columns. The `unanswered` column is now headed `unfinished`.
- A test checks that every committed `results/*.json` renders and that the README tables are the
  rows the current code renders from them.
- A weekly scheduled CI run, and a `release` workflow: pushing a `v*` tag that matches the
  workspace version builds Linux x86_64 binaries and creates the GitHub release with generated
  notes and a checksum.

### Changed

- Recall@10 divides by min(expected ids, 10) instead of the number of expected ids, so a query
  with more than ten right answers can reach 1.0.
- `temperature` is sent only with `--temperature`, because some models reject it.
- `rustls-webpki` and `webpki-root-certs` are updated for RUSTSEC-2026-0382 and -0383.
- The advisory scan is a job of its own that does not fail a push or pull request run, only the
  weekly scheduled one, so an upstream advisory cannot block an unrelated change.
- The README assistant tables were re-rendered from the saved files with `render`: Haiku's
  intervals now resample whole queries (three trials each); Sonnet's stay Wilson. The tool-level
  results were rerun on the new build.
- A `--top-k` above the search tool's advertised `limit` maximum (the reference store's is 25)
  failed every query; it now stops before any query with an error naming both numbers.
- A rate-limit table full of live clients was swept on every request from a new address; the
  sweep forced by a full table now runs at most once a second.

[Unreleased]: https://github.com/Vladimir-Pirotskiy/mcp-commerce-evals/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/Vladimir-Pirotskiy/mcp-commerce-evals/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Vladimir-Pirotskiy/mcp-commerce-evals/releases/tag/v0.1.0
