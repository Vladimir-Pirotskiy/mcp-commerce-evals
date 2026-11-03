# mcp-commerce-evals

[![ci](https://github.com/Vladimir-Pirotskiy/mcp-commerce-evals/actions/workflows/ci.yml/badge.svg)](https://github.com/Vladimir-Pirotskiy/mcp-commerce-evals/actions/workflows/ci.yml)

Evals for AI shopping assistants over MCP, with a reference store to run them against.

An assistant that shops for a buyer does two things that can go wrong: it finds the wrong part, or
it finds the right part and then reasons badly about it. This repo measures both, against any store
that speaks MCP (Model Context Protocol) over HTTP. It ships a small store of its own so that you
have something to point the evals at, and so that the numbers below can be reproduced in a minute
without an account or a key.

Everything is invented: the catalog is 500 generated bicycle parts from 14 fictional brands, and
checkout links go to a domain that does not exist.

## What is in the repo

| Crate | Kind | Purpose |
|---|---|---|
| `mcp-commerce-evals` (`evals/`) | binary + library | Reads a queries file, scores a store's search tool, optionally lets a model shop and scores its answer. |
| `reference-store` | binary + library | MCP server over Streamable HTTP with four shopping tools. |
| `catalog` | library | Seeded generator of the parts, and the search engine behind the store: BM25 with typo tolerance, synonyms and filters. |

Rust 1.99.0 (pinned in `rust-toolchain.toml`), no unsafe code, no `unwrap()` outside tests. The
store and the catalog set `publish = false`; the eval CLI is the thing you would install.

## Quickstart

```sh
cargo run --release -p reference-store          # listens on 127.0.0.1:8787
curl http://127.0.0.1:8787/health               # {"status":"ok"}
```

Connect an assistant that runs on your machine, for example Claude Code:

```sh
claude mcp add --transport http shop http://127.0.0.1:8787/mcp
```

Then ask it for "a chain for my 11-speed road bike, under $30" and watch it call
`search_products`, `check_availability` and `create_cart`.

Run the evals against the store from the repo root (no key needed):

```sh
cargo run --release -p mcp-commerce-evals -- --url http://127.0.0.1:8787/mcp
```

It prints a Markdown table and writes `eval-results.json` (change it with `--out`). The 44 queries
are compiled into the binary, so the same command works from any directory, with a downloaded
release binary too; `--queries <path>` uses a file of your own.

## Connecting a hosted assistant

Hosted assistants call your server from their cloud, so the URL must be public HTTPS. Expose the
store through a tunnel or a host you control and tell it which hostname to accept, because it
rejects unknown `Host` headers (DNS rebinding protection):

```sh
reference-store --allowed-host shop.example.org --trust-forwarded-for
```

Use `--trust-forwarded-for` only behind a proxy that sets `X-Forwarded-For`. In Claude, add a
custom connector with `https://shop.example.org/mcp`; ChatGPT needs Developer mode and accepts
remote HTTPS servers only. The proxy rules, the `Origin` check and the menu paths are in
[docs/store.md](docs/store.md).

## The store's tools

The server is stateless and answers with JSON, so any client can call `tools/list` and
`tools/call` without a session. Every tool has an input schema, an output schema, structured
content, and a JSON text block with the same data for clients that read only text.

| Tool | Arguments | What it does |
|---|---|---|
| `search_products` | `query`, `brand`, `category`, `fits[]`, `in_stock_only`, `max_price_usd`, `limit` | Ranked search. Tolerates typos and synonyms, matches part numbers. Returns stock status, not quantity. |
| `get_product` | `id` | Full detail: attributes, compatibility, units on the shelf, restock days. |
| `check_availability` | `items[]` of `{id, quantity}` | Per part: available, partial, backorder, unavailable or unknown_part. A part sent on several lines is added up first, as `create_cart` does. Changes nothing. |
| `create_cart` | `items[]` of `{id, quantity}` | Returns a fake `checkout_url`, the lines and the subtotal. The assistant never pays; the buyer opens the link. Idempotent: the same items give the same cart. |

Money is sent in dollars and in whole cents; the cents are authoritative. Errors come back as
tool errors (`isError: true`) with a message that says how to recover. The store sheds load: 120
requests per client per minute (429), 256 requests in flight (503), a 30-second deadline (504), and
inputs are length-checked. Every limit and flag is in [docs/store.md](docs/store.md).

## How the evals work

`evals/queries.json` holds 44 buyer queries of eight kinds (exact part number, plain description,
compatibility, typo, synonym, filter, adversarial, and `no_match`, for things the catalog does not
sell). The expected ids come from catalog attributes, never from running the search, and a test
re-derives them.

- **Tool-level** (default, no key): calls the search tool for every query and reports hit@1, hit@5,
  MRR, capped recall@10, nDCG@10 and latency, with Wilson and seeded bootstrap 95% intervals.
- **Assistant-level** (`--assistant anthropic`, costs tokens): lets a model shop through the store's
  tools and scores the part id it commits to on an `ANSWER:` line. `--trials N` gives pass@k.
- **Gates and exit codes:** `--min-hit5`, `--min-mrr` and `--min-accuracy` turn a score into exit
  status 3; 1 means the run could not complete and 2 that a query errored.
- `mcp-commerce-evals render <results.json>` prints the tables for a saved report with no network
  call.

The flags, metrics, report format and MCP client behaviour are in [docs/evals.md](docs/evals.md).

## Results

Measured on 2026-11-03 against this store: the tables, the queries that missed and the caveats are
in [docs/results.md](docs/results.md).

## Development

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings   # includes clippy::pedantic
cargo test --locked --workspace
RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --no-deps
cargo deny --offline check licenses bans sources
```

`Cargo.lock` is committed; dependencies are taken after a 14-day cooldown, with an exception for
security fixes. The tests need no network and no key. See [docs/development.md](docs/development.md).

## Threat model

The store is a demo meant to be exposed behind a proxy for a hosted assistant. It defends against:

- a spoofed `X-Forwarded-For`: only the entry the trusted proxy appended is read, never the
  client-written ones on its left;
- oversized bodies (64 KiB), long or numerous inputs, and requests that run too long (504);
- slow clients: a client that has not finished the request line and headers within 10 seconds
  (`--header-read-timeout-secs`) has its connection closed, and so does an idle kept-alive
  connection that sends no next request in that time;
- connection floods: at most 1024 connections open at once (`--max-connections`); a connection
  over the cap is closed without an answer, never queued. The cap is global, not per IP, and the
  OS file-descriptor limit (`ulimit -n`) may need raising to reach it;
- request floods: a cap on requests in flight (503) and a per-client rate limit;
- cross-origin browser calls and DNS rebinding: `Host` and `Origin` are checked, case-insensitively;
- exhaustion of the client table: it is bounded, with a shared overflow bucket.

It does not provide: authentication or authorisation (every tool is public and read-only apart
from the fake cart); rate limits shared across instances (the limiter is in memory and per
process, so N instances allow N times the budget); or protection from a distributed or
network-layer denial of service, which belongs to the proxy or the network in front.

## Limitations

- The catalog is synthetic. Descriptions are templated, so a real catalog's mess (inconsistent
  titles, missing attributes, duplicates) is not represented. Some attributes are generated
  independently where real parts tie them together: a cassette's tooth range does not depend on its
  speed.
- The search is a reference implementation, not a claim about how any real store should rank.
- There is no `/.well-known/ucp` profile. The Universal Commerce Protocol (ucp.dev) describes MCP
  stores with its own tool names (`search_catalog`, `lookup_catalog`, `get_product`) and an OpenRPC
  schema in the profile. This store uses different tool names, so publishing a profile for it would
  tell UCP platforms something untrue. Implementing the UCP catalog binding is a separate piece of
  work; I read the overview page of the spec, not its transport section.
- `create_cart` does not take payment, addresses or buyer details. By design the human pays.
- One run, one machine, one seed.
- The store was exercised with a real MCP client (Claude Code) on a local address only; I did not test
  a connection from the hosted assistants, which need a public hostname this repo does not provide.
- The Messages API's acceptance of the `$schema` field in the store's input schemas is unchecked.

## License

MIT, see `LICENSE`.
