# mcp-commerce-evals

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

It prints a Markdown table and writes `eval-results.json` (change it with `--out`).

## Connecting a hosted assistant

Hosted assistants call your server from their cloud, so `127.0.0.1` is not reachable and the URL
must be public HTTPS. Expose the store through a tunnel or a host you control, and tell the store
which hostname to accept, because it rejects unknown `Host` headers (DNS rebinding protection):

```sh
reference-store --allowed-host shop.example.org --trust-forwarded-for
```

`--trust-forwarded-for` makes the rate limiter key on `X-Forwarded-For`; use it only behind a proxy
that sets that header, otherwise a client can pick its own identity.

- Claude: Settings, Connectors, Add custom connector, paste `https://shop.example.org/mcp`.
- ChatGPT: enable Developer mode, create an app, paste the same URL. ChatGPT accepts remote HTTPS
  servers only.

Menu names change between releases; check each vendor's current connector documentation. I ran the
store and the evals locally. I did not test a connection from Claude Code, Claude or ChatGPT: the
hosted ones need a public hostname, which this repo does not provide.

## The store's tools

The server is stateless and answers with JSON, so any client can call `tools/list` and
`tools/call` without a session. Every tool has an input schema, an output schema, structured
content, and a JSON text block with the same data for clients that read only text.

| Tool | Arguments | What it does |
|---|---|---|
| `search_products` | `query`, `brand`, `category`, `fits[]`, `in_stock_only`, `max_price_usd`, `limit` | Ranked search. Tolerates typos and synonyms, matches part numbers. Returns stock status, not quantity. |
| `get_product` | `id` | Full detail: attributes, compatibility, units on the shelf, restock days. |
| `check_availability` | `items[]` of `{id, quantity}` | Per item: available, partial, backorder, unavailable or unknown_part. Changes nothing. |
| `create_cart` | `items[]` of `{id, quantity}` | Returns a fake `checkout_url`, the lines and the subtotal. The assistant never pays; the buyer opens the link. Idempotent: the same items give the same cart. |

The descriptions are written for the assistant that reads them: when to call the tool, what the
result does not contain, and what to do next. Errors come back as tool errors (`isError: true`)
with a message that says how to recover, for example "Copy ids from search_products results".

Limits: 120 requests per client per minute on `/mcp` (429 with `Retry-After`), `/health` is not
limited, queries over 200 characters are refused, carts take at most 20 lines.

## How the evals work

`evals/queries.json` holds 40 buyer queries. Each has an id, a kind, the text a buyer would type
and the part ids that count as a correct answer (any one of them is enough).

| Kind | n | Example |
|---|---:|---|
| exact_part_number | 6 | a part number typed as printed on the box |
| plain_description | 8 | "long-cage clutch rear derailleur for 12 speed" |
| compatibility | 8 | "rear hub for a 12x142 thru axle frame with an 11-speed disc brake setup" |
| typo | 6 | misspelled words in an otherwise ordinary query |
| synonym | 6 | the buyer's word is not the catalog's word |
| filter | 6 | "11-speed chain, in stock, under $30" |

The expected ids come from catalog attributes (every chain with `speed = 11-speed`, in stock, under
$30), never from running the search, so the search cannot mark its own homework. Some queries are
meant to fail: q06 types the letter O for a zero in a part number.

A filter query also carries a `search` object, the arguments an assistant would build from the
sentence. The tool-level run sends that object as is; plain queries send `{"query": text}`.

**Tool-level (default, no key).** For each query the CLI calls the search tool over HTTP and
compares the returned ids with the expected ones. It reports, per kind and overall:

- hit@1 and hit@5: share of queries with an expected part in the first 1 or 5 results;
- MRR: mean of 1 / rank of the best expected part, 0 when it is not in the top 10;
- latency: p50 and p95 of the round trip, nearest-rank, errors excluded.

A failed call counts as a miss, so a broken server cannot score well. Use `--search-tool` for a
store whose tool has another name; results are read from `results`, `products`, `items` or `hits`,
and each entry's `id`, `sku`, `part_number` or `product_id`.

**Assistant-level (optional, costs tokens).** `--assistant anthropic` gives a model the store's
tools through the Anthropic Messages API and lets it work until it answers, at most `--max-turns`
(default 8). The answer counts as correct when it names an expected part number. The report adds
tool calls per query and input and output tokens, which is where a store with vague tool
descriptions shows up: the model needs more calls to get to the same answer.

```sh
export ANTHROPIC_API_KEY=...        # read from the environment only, never from argv
cargo run --release -p mcp-commerce-evals -- --assistant anthropic --model claude-haiku-5-5
```

I did not run this mode against the live API. Its loop is tested against a mock of the API (tool
call, tool result, final answer, token counts, an HTTP 401 whose message must not contain the key),
but the request shape comes from the API documentation, not from a live call, and I have not
checked that the API accepts the `$schema` field the store's input schemas carry. No assistant-level
numbers are published here for that reason.

The store itself was checked with a real MCP client on 2026-11-03: Claude Code (`claude -p`, Haiku,
the store as its only server through `--mcp-config`) was asked for "an 11-speed chain that is in
stock and under 30 dollars" and a cart. It searched, checked stock and returned a cart with
`FER-CH-4296` at $24.99, an expected answer, in 5 turns. That run found that the client warned on
every `uint32` and `double` format schemars writes for Rust numbers; the store now drops them
(`schemas_use_only_standard_formats` in `reference-store/tests/http.rs`).

## Results

Tool-level run on 2026-11-02, release build, store and CLI on the same machine (Apple M4 Max,
64 GiB, macOS 27.2), loopback, one run of 40 queries, `--top-k 10`:

```sh
cargo build --release --workspace
./target/release/reference-store &
./target/release/mcp-commerce-evals --url http://127.0.0.1:8787/mcp \
  --out results/tool-level-2026-11-02.json
```

| kind | queries | hit@1 | hit@5 | MRR | p50 ms | p95 ms |
|---|---:|---:|---:|---:|---:|---:|
| compatibility | 8 | 0.88 | 0.88 | 0.89 | 0.2 | 0.4 |
| exact_part_number | 6 | 0.83 | 0.83 | 0.85 | 0.3 | 0.6 |
| filter | 6 | 0.83 | 0.83 | 0.83 | 0.2 | 0.2 |
| plain_description | 8 | 0.88 | 1.00 | 0.92 | 0.2 | 0.3 |
| synonym | 6 | 1.00 | 1.00 | 1.00 | 0.3 | 0.3 |
| typo | 6 | 1.00 | 1.00 | 1.00 | 0.3 | 0.4 |
| **all** | 40 | 0.90 | 0.93 | 0.91 | 0.2 | 0.4 |

The full per-query output is in `results/tool-level-2026-11-02.json`. Four queries were not ranked
first:

| id | query | rank | what happened |
|---|---|---:|---|
| q06 | `STO-HT-1O64` | 9 | The letter O is not read as a zero inside a part number. |
| q14 | long-cage clutch rear derailleur for 12 speed | 3 | Two other derailleurs rank above the expected one. |
| q22 | rear hub for a 12x142 thru axle frame with an 11-speed disc brake setup | 9 | Axle parts outrank hubs. |
| q35 | 11-speed chain, in stock, under $30 | none | The `search` object says `query: "chain"` without a category, and chain lubes rank above chains. With `category: "chain"` the chains come first. |

Read these numbers with care. 40 queries is small: one query moves a rate by 2.5 points, and per
kind by 12 to 17. The catalog and the queries were written together, by the same person, so this
shows that the harness measures and that the store works; it does not rank search engines. Latency
is a loopback round trip to an in-memory index, not a network or database figure. Rankings are
deterministic for a given seed; latency varies from run to run.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

`Cargo.lock` is committed and holds only crate versions published before 2026-10-19, at least two
weeks before it was generated. Nightly cargo resolved it, for the lockfile only (the build uses
1.99.0):

```sh
cargo +nightly generate-lockfile -Zunstable-options --publish-time 2026-10-19T00:00:00Z
```

The tests need no network and no key. The integration test starts the store on a random port inside
the test process and runs the whole tool-level eval against it, with a floor on hit@5 and MRR and
a requirement that the first five queries (exact part numbers) rank first. Another test checks
that every expected id exists in the demo catalog.

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

## License

MIT, see `LICENSE`.
