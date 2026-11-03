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

Menu names change between releases; check each vendor's current connector documentation.

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
`--requests-per-minute`, which must be at least 1), `/health` is not limited. The window is
fixed, not sliding, so a client can spend its budget at the end of one window and again at the
start of the next: up to twice the limit within a window's length. The rate limit runs before the
`Host` and `Origin` checks, so a request refused for either still counts against its client.

At most 10,000 clients are tracked one by one. Further clients share a single overflow bucket
with its own budget, ten times one client's limit per window. The trade-off: evicting tracked
clients to make room would let someone rotating addresses reset real clients' counters, and
admitting untracked clients uncounted would fail open under exactly that flood. A shared bucket
fails closed, bounding all untracked traffic, at the price that during an address flood newcomers
share one budget and some get 429 undeservedly; tracked clients are unaffected. The reasoning is
in the `rate_limit` module docs.

`/mcp` also sheds load: at most 256 requests are handled at once (`--max-in-flight`; more get 503
with `Retry-After`) and a request that runs past 30 seconds (`--request-timeout-secs`) gets 504.
The store stops on ctrl-c or SIGTERM, finishing requests already in flight.
`--allowed-host` takes a bare host with an optional port (`shop.example.org:8443`); a scheme, a
path or other junk is refused at startup. Inputs: a query
is at most 200 characters and 20 words, `limit` is 1 to 25 and anything else is refused, `brand`,
each `fits` entry and every part id are at most 100 characters, `fits` takes at most 10 non-empty
entries, a request body is at most 64 KiB, and carts take at most 20 lines. A part sent on several
cart lines counts once with the lines added up: the total must fit the shelf and 99 units.

## How the evals work

`evals/queries.json` holds 44 buyer queries. Each has an id, a kind, the text a buyer would type
and the part ids that count as a correct answer (any one of them is enough). The four `no_match`
queries are the exception: they ask for things the catalog does not sell, `expected` is `[]`, and
validation allows an empty list for that kind only.

| Kind | n | Example |
|---|---:|---|
| exact_part_number | 5 | a part number typed as printed on the box |
| plain_description | 8 | "long-cage clutch rear derailleur for 12 speed" |
| compatibility | 8 | "rear hub for a 12x142 thru axle frame with an 11-speed disc brake setup" |
| typo | 6 | misspelled words in an otherwise ordinary query |
| synonym | 6 | the buyer's word is not the catalog's word |
| filter | 6 | "11-speed chain, in stock, under $30" |
| adversarial | 1 | q06 types the letter O for a zero in a part number |
| no_match | 4 | "a bicycle helmet in size medium": the catalog sells no such thing, so the right answer is none |

The expected ids come from catalog attributes (every chain with `speed = 11-speed`, in stock, under
$30), never from running the search, so the search cannot mark its own homework. That claim is
checkable: `evals/tests/expected_sets.rs` holds one plain predicate per query (attributes, brand,
price, stock) and asserts that the ids it selects from the seeded catalog equal the ones in
`queries.json`, and that each `no_match` rule selects no part at all. It never calls the search. Run it with
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
- capped recall@10: expected ids found in the top 10 divided by min(number expected, 10), averaged
  over queries, so a query with 17 expected ids can still reach 1.0 (plain recall would cap it at
  10/17); with a smaller `--top-k` the ceiling is lower;
- nDCG@10 with binary relevance;
- 95% bootstrap intervals for MRR, recall@10 and nDCG@10: 2000 resamples of the per-query scores
  with a fixed seed (a local SplitMix64, recorded in `meta.bootstrap_seed`), so the same results
  always give the same interval;
- latency: p50 and p95 of the round trip, nearest-rank in integer arithmetic, errors excluded.
  `--repeat N` times each query N times after one untimed warm-up call.

A failed call counts as a miss, so a broken server cannot score well. A reply that has no list
under `results`, `products`, `items` or `hits`, or that is neither structured content nor JSON
text, is an error for that query, not an empty result. Use `--search-tool` for a store whose tool
has another name; each entry's id is read from `id`, `sku`, `part_number` or `product_id`.

`no_match` queries are reported in their own line (how many returned nothing, how many results
on average) and are excluded from every hit, MRR, nDCG and recall aggregate, including the "all"
rows; they cannot be ranked. Whether a store should return nothing or a weak guess is a product
choice, so the tool level only describes it. At the assistant level they count: see below.

`--kind <kind>` (repeatable) runs only queries of those kinds; the report's `meta` records them.

The process exits 0 on success, 1 when the run could not complete, 2 when any query errored (in
the tool-level or the assistant-level run) and 3 when a gate was not met: `--min-hit5 <f64>` and
`--min-mrr <f64>` compare against the "all" row, `--min-accuracy <f64>` against the
assistant-level share of correct committed answers (it needs `--assistant`), and the failed gate
is printed. A run whose report is incomplete exits 1. `--top-k` takes 1 to 100, `--max-turns` 1 to
50 and `--repeat` at least 1; other values are refused before any request. The JSON report is written first, and `--out` refuses to replace an
existing file without `--force`. The report starts with a `meta` block: crate version, git commit of
the build (`+dirty` if the tree had changes), SHA-256 of the queries file, `top_k`, the search tool,
the server's `serverInfo` and negotiated protocol version, and for assistant runs the model,
temperature (null when none was sent), `max_turns` and a hash of the system prompt.
`--timeout-secs` (default 60) bounds every HTTP request, to the store and to the model API. The MCP
client reads JSON and event-stream replies (an event stream is read as it arrives, and the call
returns when the reply to its request does, even if the server keeps the stream open), answers a
server `ping` seen in the stream, carries a session id when the server issues one, opens a new
session once if the server answers 404 to it, and ends the session with a DELETE (a 405 is fine)
when the run completes. It sends the `mcp-protocol-version` the server negotiated (2025-11-25,
2025-06-18 or 2025-03-26) and fails with a clear message on any other. On a non-2xx reply the error
carries the JSON-RPC message from the body when there is one. Server-initiated requests other
than `ping` (sampling, roots) are not supported.

**Assistant-level (optional, costs tokens).** `--assistant anthropic` gives a model the store's
tools through the Anthropic Messages API and lets it work until it answers, at most `--max-turns`
(default 8). No `temperature` is sent unless `--temperature <0-1>` is given, because some models
reject the parameter; the report records `null` or the value. A tool that returns only structured
content reaches the model as that JSON. The system prompt does not name a kind of store and asks
the model to end its final reply with a line `ANSWER: <part id>` (or `ANSWER: none`). The answer is correct
only when that committed id equals an expected id after normalising case and punctuation; whether
the reply merely mentions an expected id is kept as a separate diagnostic (`mentions_expected`; not applicable, `n/a` in the table and `null` in the JSON, for a `no_match` query, and left out of the overall figure).
Only a `stop_reason` of `end_turn` is a final answer: `max_tokens`, `refusal`, `pause_turn` or
anything else is recorded per query and counted as unanswered. The client retries HTTP 429, 529
and 5xx up to three times with exponential backoff and jitter, honouring `retry-after`, and
refuses an empty `ANTHROPIC_API_KEY` before any request. Accuracy is reported over all queries
and over the queries without API errors, next to the error count. The report adds tool calls per
query and input and output tokens, which is where a store with vague tool descriptions shows up:
the model needs more calls to get to the same answer. The report file is rewritten after every
run, with `"complete": false` until the end, so a paid run that fails midway leaves its partial
results.

For a `no_match` query the only correct committed answer is `ANSWER: none`; for every other query
`none` is wrong. Accuracy carries a 95% Wilson interval. `--trials N` runs each query N times and
reports the mean accuracy and pass@k for k = 1..N with the unbiased estimator (the chance that at
least one of k runs drawn from the N is right). The Wilson interval treats the runs as
independent, which repeated runs of the same 44 queries are not, so with several trials it is
narrower than the truth; read pass@k and the per-query rows instead. The report records the
`model` id the API returned in its replies next to the alias that was requested. The client also
retries a request timeout or a failed connect, with the same backoff and limit as an HTTP 429.

```sh
export ANTHROPIC_API_KEY=...        # read from the environment only, never from argv
cargo run --release -p mcp-commerce-evals -- --assistant anthropic --model claude-haiku-5-5
```

The loop is tested against a mock of the API: tool call, tool result, the committed answer, each
stop reason, retries, a 401 whose message must not contain the key, a request timeout, token
counts and trials.

## Results

### Tool-level results

Tool-level run on 2026-11-03, release build, store and CLI on the same machine (Apple M4 Max,
64 GiB, macOS 27.2), loopback, one run of 44 queries (40 ranked, 4 `no_match`), `--top-k 10`, `--repeat 1`, built from commit
`e0f398e` (the `git_commit` in the file's `meta` block):

```sh
cargo build --release --workspace
./target/release/reference-store &
./target/release/mcp-commerce-evals --url http://127.0.0.1:8787/mcp \
  --out results/tool-level-2026-11-03.json
```

Hit intervals are 95% Wilson; recall, nDCG and MRR intervals are seeded 95% bootstrap; p50 and
p95 are milliseconds. The file is `results/tool-level-2026-11-03.json`.

| kind | queries | hit@1 (95% CI) | hit@5 (95% CI) | recall@10 (95% CI) | nDCG@10 (95% CI) | MRR (95% CI) | p50 ms | p95 ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| adversarial | 1 | 0.00 (0.00-0.79) | 0.00 (0.00-0.79) | 1.00 (1.00-1.00) | 0.30 (0.30-0.30) | 0.11 (0.11-0.11) | 0.72 | 0.72 |
| compatibility | 8 | 0.88 (0.53-0.98) | 0.88 (0.53-0.98) | 0.90 (0.70-1.00) | 0.89 (0.67-1.00) | 0.89 (0.67-1.00) | 0.54 | 0.74 |
| exact_part_number | 5 | 1.00 (0.57-1.00) | 1.00 (0.57-1.00) | 1.00 (1.00-1.00) | 1.00 (1.00-1.00) | 1.00 (1.00-1.00) | 0.43 | 1.62 |
| filter | 6 | 0.83 (0.44-0.97) | 0.83 (0.44-0.97) | 0.83 (0.50-1.00) | 0.83 (0.50-1.00) | 0.83 (0.50-1.00) | 0.43 | 0.82 |
| plain_description | 8 | 0.88 (0.53-0.98) | 1.00 (0.68-1.00) | 1.00 (1.00-1.00) | 0.94 (0.81-1.00) | 0.92 (0.75-1.00) | 0.55 | 0.72 |
| synonym | 6 | 1.00 (0.61-1.00) | 1.00 (0.61-1.00) | 1.00 (1.00-1.00) | 1.00 (1.00-1.00) | 1.00 (1.00-1.00) | 0.55 | 1.14 |
| typo | 6 | 1.00 (0.61-1.00) | 1.00 (0.61-1.00) | 1.00 (1.00-1.00) | 1.00 (1.00-1.00) | 1.00 (1.00-1.00) | 0.66 | 1.18 |
| **all** | 40 | 0.90 (0.77-0.96) | 0.93 (0.80-0.97) | 0.96 (0.89-1.00) | 0.92 (0.84-0.99) | 0.91 (0.82-0.98) | 0.56 | 1.14 |
| **all except filter** | 34 | 0.91 (0.77-0.97) | 0.94 (0.81-0.98) | 0.98 (0.93-1.00) | 0.94 (0.86-1.00) | 0.93 (0.84-1.00) | 0.56 | 1.18 |

The four `no_match` queries returned 10 results each (the store always returns its best guesses)
and are not in any row above.

The full per-query output is in that file. Four queries were not ranked first:

| id | query | rank | what happened |
|---|---|---:|---|
| q06 | `STO-HT-1O64` | 9 | The letter O is not read as a zero inside a part number. |
| q14 | long-cage clutch rear derailleur for 12 speed | 3 | Two other derailleurs rank above the expected one. |
| q22 | rear hub for a 12x142 thru axle frame with an 11-speed disc brake setup | 9 | Axle parts outrank hubs. |
| q35 | 11-speed chain, in stock, under $30 | none | The `search` object says `query: "chain"` without a category, and chain lubes rank above chains. With `category: "chain"` the chains come first. |

Read these numbers with care. 40 ranked queries is small: one query moves a rate by 2.5 points, and the
intervals show how wide the uncertainty is, from 0.44 to 0.97 for a kind of six. The one
adversarial query is a single data point, and its interval spans most of the scale. The catalog
and the queries were written together, by the same person, so this shows that the harness
measures and that the store works; it does not rank search engines. Latency is a loopback round
trip to an in-memory index, not a network or database figure, from one run after one warm-up
call. Rankings are deterministic for a given seed; latency varies from run to run.

### Assistant-level results

Run on 2026-11-03 from commit `e0f398e` (the `git_commit` in each file's `meta`), release build, store
on loopback, the 44 queries of the tool-level run, `--max-turns 8`, and no `temperature` sent
(`null` in the report). Two models, each answered by the model it asked for (`response_models`):
`claude-haiku-5-5` with `--trials 3` (132 runs) and `claude-sonnet-5-5` with one trial (44 runs).
No run ended in an API error or without a final answer. The files are
`results/assistant-level-2026-11-03-claude-haiku-5-5.json` and
`results/assistant-level-2026-11-03-claude-sonnet-5-5.json`.

```sh
./target/release/mcp-commerce-evals --url http://127.0.0.1:8787/mcp --assistant anthropic \
  --model claude-haiku-5-5 --trials 3 --out results/assistant-level-2026-11-03-claude-haiku-5-5.json
./target/release/mcp-commerce-evals --url http://127.0.0.1:8787/mcp --assistant anthropic \
  --model claude-sonnet-5-5 --out results/assistant-level-2026-11-03-claude-sonnet-5-5.json
```

A run is correct when its `ANSWER:` line commits to an expected id (`none` for a `no_match` query).
Intervals are 95% Wilson and count every run as independent.

**claude-haiku-5-5**, 3 trials per query: pass@1 0.94, pass@2 0.95, pass@3 0.95. Tokens over all 132
runs: 1,196,419 input and 76,890 output.

| kind | runs | committed answer correct (95% CI) | tool calls per query | input tokens | output tokens |
|---|---:|---:|---:|---:|---:|
| adversarial | 3 | 0.67 (0.21-0.94) | 3.7 | 42,451 | 2,869 |
| compatibility | 24 | 0.88 (0.69-0.96) | 1.5 | 197,436 | 15,026 |
| exact_part_number | 15 | 1.00 (0.80-1.00) | 1.1 | 94,366 | 4,896 |
| filter | 18 | 1.00 (0.82-1.00) | 1.0 | 118,650 | 10,351 |
| no_match | 12 | 1.00 (0.76-1.00) | 2.9 | 194,862 | 6,821 |
| plain_description | 24 | 1.00 (0.86-1.00) | 2.5 | 254,538 | 13,035 |
| synonym | 18 | 0.94 (0.74-0.99) | 1.5 | 139,987 | 10,661 |
| typo | 18 | 0.83 (0.61-0.94) | 1.3 | 154,129 | 13,231 |
| **all** | 132 | 0.94 (0.88-0.97) | 1.7 | 1,196,419 | 76,890 |

**claude-sonnet-5-5**, 1 trial per query. Tokens over all 44 runs: 473,526 input and 19,703 output.

| kind | runs | committed answer correct (95% CI) | tool calls per query | input tokens | output tokens |
|---|---:|---:|---:|---:|---:|
| adversarial | 1 | 1.00 (0.21-1.00) | 4.0 | 15,318 | 590 |
| compatibility | 8 | 0.88 (0.53-0.98) | 2.4 | 94,324 | 4,390 |
| exact_part_number | 5 | 1.00 (0.57-1.00) | 1.8 | 35,770 | 1,140 |
| filter | 6 | 1.00 (0.61-1.00) | 1.2 | 45,297 | 2,551 |
| no_match | 4 | 1.00 (0.51-1.00) | 3.0 | 58,661 | 1,552 |
| plain_description | 8 | 1.00 (0.68-1.00) | 2.5 | 101,691 | 3,338 |
| synonym | 6 | 1.00 (0.61-1.00) | 2.2 | 71,747 | 2,934 |
| typo | 6 | 1.00 (0.61-1.00) | 1.2 | 50,718 | 3,208 |
| **all** | 44 | 0.98 (0.88-1.00) | 2.1 | 473,526 | 19,703 |

What the per-query rows show:

- Four queries account for every miss. Haiku missed q20 ("disc rotor that works on my e-bike") and
  q23 ("cassete 11-34T") in all three trials, q33 ("skewer for my road bike") once and q06 once;
  Sonnet missed q20. None of these is a retrieval failure. On q20 and q33 the models listed
  matching parts and asked for the mount type or whether the bike takes quick-release; Haiku ended
  four of those replies with no `ANSWER:` line at all, and Sonnet committed `none`. On q23 Haiku
  answered in Spanish, listed the three 11-34T cassettes and committed `none` until it knew the
  speed count; Sonnet listed the same three and committed to one. The scorer rewards committing, so
  asking back a fair question counts as a miss; the queries do allow several right answers (12 for q20).
- q06 (`STO-HT-1O64`) is ranked 9th by the search, yet the models got there: every run began with
  `get_product` on the typed id, then `search_products`, then `get_product` on the recovered
  `STO-HT-1064` (a second search followed in three of the four runs). Haiku trial 2 used one search,
  so it picked the part out of the ten results. The result files keep tool names, not their
  arguments, so I cannot say what the second searches asked. Haiku's one miss committed to an
  in-stock handlebar tape instead of the out-of-stock expected part.
- All 16 `no_match` runs (12 Haiku, 4 Sonnet) committed `none`, after 2.9 and 3.0 tool calls on
  average, more than any other kind except the adversarial query. The store always returns its best
  guesses, so the models had to read the results and decide none of them fit.
- Sonnet scored 0.98 against Haiku's 0.94 at 2.1 tool calls per query against 1.7; the intervals
  overlap widely, and Sonnet ran each query once.

Caveats: n is 44 queries, and a kind has 1 to 8 of them. The same person wrote the catalog and the
queries. Without a temperature the runs are not deterministic, so a rerun will differ; with three
Haiku trials the interval treats runs of one query as independent, which they are not, so read
pass@k and the per-query rows instead. Sonnet has a single trial, so its figure says nothing about
run-to-run spread. `results/` also holds an earlier tool-level run (2026-11-02, an older file layout);
the numbers above come from the 2026-11-03 files.

The `mentions_expected` diagnostic in these two files was produced before it was made not
applicable to `no_match` queries: their `by_kind.no_match` and `overall` figures count the
`no_match` runs as zero (overall 0.90 for Haiku, 0.91 for Sonnet). The tables above leave that column out; computed over the
queries that have an expected id, it is 0.99 for Haiku (119 of 120 runs) and 1.00 for Sonnet. The
files are as the run wrote them and were not edited.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings   # includes clippy::pedantic
cargo test --workspace
cargo doc --no-deps --workspace                          # with RUSTDOCFLAGS="-D warnings"
cargo deny check licenses bans sources                   # offline; see deny.toml
```

`Cargo.lock` is committed and holds only crate versions published before 2026-10-19, at least two
weeks before it was generated. Nightly cargo resolved it, for the lockfile only (the build uses
1.99.0):

```sh
cargo +nightly generate-lockfile -Zunstable-options --publish-time 2026-10-19T00:00:00Z
```

The tests need no network and no key. The integration test starts the store on a random port inside
the test process and runs the whole tool-level eval against it, with a floor on hit@5 and MRR (CI's eval job gates the same numbers with `--min-hit5 0.92 --min-mrr 0.91`) and
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
- The store was exercised with a real MCP client (Claude Code) on a local address only; I did not test
  a connection from the hosted assistants, which need a public hostname this repo does not provide.
- The Messages API's acceptance of the `$schema` field in the store's input schemas is unchecked.

## License

MIT, see `LICENSE`.
