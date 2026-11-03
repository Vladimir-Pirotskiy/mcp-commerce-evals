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

It prints a Markdown table and writes `eval-results.json` (change it with `--out`).

## Connecting a hosted assistant

Hosted assistants call your server from their cloud, so `127.0.0.1` is not reachable and the URL
must be public HTTPS. Expose the store through a tunnel or a host you control, and tell the store
which hostname to accept, because it rejects unknown `Host` headers (DNS rebinding protection):

```sh
reference-store --allowed-host shop.example.org --trust-forwarded-for
```

`--trust-forwarded-for` makes the rate limiter key on `X-Forwarded-For`; use it only behind a proxy
that sets that header. The client is the entry that many proxies from the right (default 1, the
address your proxy appended; change it with `--trusted-proxy-hops`), because everything to its left
is written by the client. An entry that is not an IP address falls back to the socket peer, and
IPv6 clients are counted per /64. Make sure your proxy appends to the header rather than passing
the client's value through unchanged.

The same host list guards the `Origin` header, as the MCP transport spec requires: a browser
request whose `Origin` is not a page on a listed host (or on loopback) gets 403, while requests
without an `Origin`, which is every non-browser client, are served.

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
| `check_availability` | `items[]` of `{id, quantity}` | Per part: available, partial, backorder, unavailable or unknown_part. A part sent on several lines is added up first, as `create_cart` does. Changes nothing. |
| `create_cart` | `items[]` of `{id, quantity}` | Returns a fake `checkout_url`, the lines and the subtotal. The assistant never pays; the buyer opens the link. Idempotent: the same items give the same cart. |

Money is sent twice: dollars for display (`price_usd`, `unit_price_usd`, `line_total_usd`,
`subtotal_usd`) and whole cents (`price_cents`, `unit_price_cents`, `line_total_cents`,
`subtotal_cents`). The cents are authoritative; do arithmetic on them.

The descriptions are written for the assistant that reads them: when to call the tool, what the
result does not contain, and what to do next. Errors come back as tool errors (`isError: true`)
with a message that says how to recover, for example "Copy ids from search_products results".

Limits: 120 requests per client per minute on `/mcp` (429 with `Retry-After`; change it with
`--requests-per-minute`, which must be at least 1), at most 10,000 clients tracked (more share one
budget until a window expires), `/health` is not limited. The window is fixed, not sliding, so a client can spend its budget at
the end of one window and again at the start of the next: up to twice the limit within a window's
length. The rate limit runs before the `Host` and
`Origin` checks, so a request refused for either still counts against its client. Inputs: a query
is at most 200 characters and 20 words, `limit` is 1 to 25 and anything else is refused, `brand`,
each `fits` entry and every part id are at most 100 characters, `fits` takes at most 10 non-empty
entries, a request body is at most 64 KiB, and carts take at most 20 lines. A part sent on several
cart lines counts once with the lines added up: the total must fit the shelf and 99 units.

## How the evals work

`evals/queries.json` holds 40 buyer queries. Each has an id, a kind, the text a buyer would type
and the part ids that count as a correct answer (any one of them is enough).

| Kind | n | Example |
|---|---:|---|
| exact_part_number | 5 | a part number typed as printed on the box |
| plain_description | 8 | "long-cage clutch rear derailleur for 12 speed" |
| compatibility | 8 | "rear hub for a 12x142 thru axle frame with an 11-speed disc brake setup" |
| typo | 6 | misspelled words in an otherwise ordinary query |
| synonym | 6 | the buyer's word is not the catalog's word |
| filter | 6 | "11-speed chain, in stock, under $30" |
| adversarial | 1 | q06 types the letter O for a zero in a part number |

The expected ids come from catalog attributes (every chain with `speed = 11-speed`, in stock, under
$30), never from running the search, so the search cannot mark its own homework. That claim is
checkable: `evals/tests/expected_sets.rs` holds one plain predicate per query (attributes, brand,
price, stock) and asserts that the ids it selects from the seeded catalog equal the ones in
`queries.json`. It never calls the search. Run it with
`cargo test -p mcp-commerce-evals --test expected_sets`.

A filter query also carries a `search` object, the arguments an assistant would build from the
sentence. The tool-level run sends that object as is; plain queries send `{"query": text}`. Those
arguments define the expected set themselves (price, stock), so filter queries test filtering,
not ranking, and the headline is shown twice: over all queries and over all except `filter`. The
`search` object of q35 has no `category`, although the tool accepts one: the other filter queries
(q36, q40) leave it out the same way, an assistant often sends only the words of the sentence, and
the failure it causes (chain lubes outrank chains) is a fair finding about the store's ranking. I
left it rather than tune the query to the store.

**Tool-level (default, no key).** For each query the CLI calls the search tool over HTTP and
compares the returned ids with the expected ones. It reads at most `--top-k` results (default 10)
whatever the store returns, keeps only the first position of a repeated id, and keeps an entry
without a recognisable id as an empty rank so later results do not move up. It reports, per kind
and overall:

- hit@1 and hit@5: share of queries with an expected part in the first 1 or 5 results, each with a
  95% Wilson interval;
- MRR: mean of 1 / rank of the best expected part, 0 when it is not in the top 10;
- recall@10: share of a query's expected ids found in the top 10, averaged over queries (a query
  with 17 expected ids cannot reach 1.0 at `--top-k 5`);
- nDCG@10 with binary relevance;
- latency: p50 and p95 of the round trip, nearest-rank in integer arithmetic, errors excluded.
  `--repeat N` times each query N times after one untimed warm-up call.

A failed call counts as a miss, so a broken server cannot score well. A reply that has no list
under `results`, `products`, `items` or `hits`, or that is neither structured content nor JSON
text, is an error for that query, not an empty result. Use `--search-tool` for a store whose tool
has another name; each entry's id is read from `id`, `sku`, `part_number` or `product_id`.

`--kind <kind>` (repeatable) runs only queries of those kinds; the report's `meta` records them.

The process exits 0 on success, 1 when the run could not complete, 2 when any query errored (in
the tool-level or the assistant-level run) and 3 when a gate was not met: `--min-hit5 <f64>` and `--min-mrr <f64>` compare against the "all" row
and the failed gate is printed. The JSON report is written first, and `--out` refuses to replace an
existing file without `--force`. The report starts with a `meta` block: crate version, git commit of
the build (`+dirty` if the tree had changes), SHA-256 of the queries file, `top_k`, the search tool,
the server's `serverInfo` and negotiated protocol version, and for assistant runs the model,
temperature (null when none was sent), `max_turns` and a hash of the system prompt.
`--timeout-secs` (default 60) bounds every HTTP request, to the store and to the model API. The MCP
client reads JSON and event-stream replies, carries a session id when the server issues one,
opens a new session once if the server answers 404 to it, and ends the session with a DELETE
(a 405 is fine) when the run completes.

**Assistant-level (optional, costs tokens).** `--assistant anthropic` gives a model the store's
tools through the Anthropic Messages API and lets it work until it answers, at most `--max-turns`
(default 8). No `temperature` is sent unless `--temperature <0-1>` is given, because some models
reject the parameter; the report records `null` or the value. A tool that returns only structured
content reaches the model as that JSON. The system prompt does not name a kind of store and asks
the model to end its final reply with a line `ANSWER: <part id>` (or `ANSWER: none`). The answer is correct
only when that committed id equals an expected id after normalising case and punctuation; whether
the reply merely mentions an expected id is kept as a separate diagnostic (`mentions_expected`).
Only a `stop_reason` of `end_turn` is a final answer: `max_tokens`, `refusal`, `pause_turn` or
anything else is recorded per query and counted as unanswered. The client retries HTTP 429, 529
and 5xx up to three times with exponential backoff and jitter, honouring `retry-after`, and
refuses an empty `ANTHROPIC_API_KEY` before any request. Accuracy is reported over all queries
and over the queries without API errors, next to the error count. The report adds tool calls per
query and input and output tokens, which is where a store with vague tool descriptions shows up:
the model needs more calls to get to the same answer. The report file is rewritten after every
query, with `"complete": false` until the end, so a paid run that fails midway leaves its partial
results.

```sh
export ANTHROPIC_API_KEY=...        # read from the environment only, never from argv
cargo run --release -p mcp-commerce-evals -- --assistant anthropic --model claude-haiku-5-5
```

The loop is tested against a mock of the API (tool call, tool result, the committed answer, each
stop reason, a 429 that is retried, a 401 whose message must not contain the key, a request
timeout, token counts, no `temperature` by default). A short live smoke run against
`claude-haiku-5-5` found that the API answers HTTP 400 "`temperature` is deprecated for this
model" when the parameter is sent, which is why it is now opt-in. I have not checked that the API
accepts the `$schema` field the store's input schemas carry, and no assistant-level numbers are
published here.

The store itself was checked with a real MCP client on 2026-11-03: Claude Code (`claude -p`, Haiku,
the store as its only server through `--mcp-config`) was asked for "an 11-speed chain that is in
stock and under 30 dollars" and a cart. It searched, checked stock and returned a cart with
`FER-CH-4296` at $24.99, an expected answer, in 5 turns. That run found that the client warned on
every `uint32` and `double` format schemars writes for Rust numbers; the store now drops them
(`schemas_use_only_standard_formats` in `reference-store/tests/http.rs`).

## Results

Tool-level run on 2026-11-03, release build, store and CLI on the same machine (Apple M4 Max,
64 GiB, macOS 27.2), loopback, one run of 40 queries, `--top-k 10`, `--repeat 1`, build of git
commit `5c5b3f1` (its `meta` block is in the file):

```sh
cargo build --release --workspace
./target/release/reference-store &
./target/release/mcp-commerce-evals --url http://127.0.0.1:8787/mcp \
  --out results/tool-level-2026-11-03.json
```

Intervals are 95% Wilson; p50 and p95 are milliseconds.

| kind | queries | hit@1 (95% CI) | hit@5 (95% CI) | recall@10 | nDCG@10 | MRR | p50 ms | p95 ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| adversarial | 1 | 0.00 (0.00-0.79) | 0.00 (0.00-0.79) | 1.00 | 0.30 | 0.11 | 0.26 | 0.26 |
| compatibility | 8 | 0.88 (0.53-0.98) | 0.88 (0.53-0.98) | 0.87 | 0.89 | 0.89 | 0.33 | 0.52 |
| exact_part_number | 5 | 1.00 (0.57-1.00) | 1.00 (0.57-1.00) | 1.00 | 1.00 | 1.00 | 0.46 | 0.70 |
| filter | 6 | 0.83 (0.44-0.97) | 0.83 (0.44-0.97) | 0.83 | 0.83 | 0.83 | 0.25 | 0.40 |
| plain_description | 8 | 0.88 (0.53-0.98) | 1.00 (0.68-1.00) | 1.00 | 0.94 | 0.92 | 0.31 | 0.44 |
| synonym | 6 | 1.00 (0.61-1.00) | 1.00 (0.61-1.00) | 1.00 | 1.00 | 1.00 | 0.42 | 0.61 |
| typo | 6 | 1.00 (0.61-1.00) | 1.00 (0.61-1.00) | 1.00 | 1.00 | 1.00 | 0.41 | 0.58 |
| **all** | 40 | 0.90 (0.77-0.96) | 0.93 (0.80-0.97) | 0.95 | 0.92 | 0.91 | 0.37 | 0.58 |
| **all except filter** | 34 | 0.91 (0.77-0.97) | 0.94 (0.81-0.98) | 0.97 | 0.94 | 0.93 | 0.38 | 0.61 |

The full per-query output is in `results/tool-level-2026-11-03.json`. The rankings are identical
to the first run, `results/tool-level-2026-11-02.json`, which is kept: that file predates the
`meta` block, the intervals and the changes to how replies are read, and it counts q06 under
`exact_part_number`. Four queries were not ranked first:

| id | query | rank | what happened |
|---|---|---:|---|
| q06 | `STO-HT-1O64` | 9 | The letter O is not read as a zero inside a part number. |
| q14 | long-cage clutch rear derailleur for 12 speed | 3 | Two other derailleurs rank above the expected one. |
| q22 | rear hub for a 12x142 thru axle frame with an 11-speed disc brake setup | 9 | Axle parts outrank hubs. |
| q35 | 11-speed chain, in stock, under $30 | none | The `search` object says `query: "chain"` without a category, and chain lubes rank above chains. With `category: "chain"` the chains come first. |

Read these numbers with care. 40 queries is small: one query moves a rate by 2.5 points, and the
intervals show how wide the uncertainty is, from 0.44 to 0.97 for a kind of six. The one
adversarial query is a single data point, and its interval spans most of the scale. The catalog
and the queries were written together, by the same person, so this shows that the harness
measures and that the store works; it does not rank search engines. Latency is a loopback round
trip to an in-memory index, not a network or database figure, from one run after one warm-up
call. Rankings are deterministic for a given seed; latency varies from run to run.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings   # includes clippy::pedantic
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
that every expected id exists in the demo catalog, and `expected_sets` rederives every expected set
from catalog attributes. The assistant loop, the retries, the exit codes and the report metadata
are tested against mocks and the in-process store.

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
