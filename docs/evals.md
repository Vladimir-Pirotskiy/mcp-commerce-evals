# How the evals work

The queries file, the metrics, the exit codes and the report format of `mcp-commerce-evals`.
`queries.json` is compiled into the binary and used when `--queries <path>` is not given, so the
release binary runs from any directory; the report's `queries_file` then reads
`built-in evals/queries.json`, and `meta.queries_sha256` hashes the bytes the run used.

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
- MRR: mean of 1 / rank of the best expected part, 0 when it is not in the top 10 (the rank is
  capped at the same depth as recall and nDCG, so `--top-k 100` cannot raise it);
- capped recall@10: expected ids found in the top 10 divided by min(number expected, 10), averaged
  over queries, so a query with 17 expected ids can still reach 1.0 (plain recall would cap it at
  10/17); with a smaller `--top-k` the ceiling is lower;
- nDCG@10 with binary relevance;
- 95% bootstrap intervals for MRR, recall@10 and nDCG@10: 2000 resamples of the per-query scores
  with a fixed seed (a local SplitMix64, recorded in `meta.bootstrap_seed`), so the same results
  always give the same interval. A percentile bootstrap of a tiny or constant sample is a
  zero-width interval that claims certainty, so it is shown as `n/a` (null in the JSON) when there
  are fewer than 10 queries or every query scored the same. Wilson intervals, which stay valid at
  small n, are always shown;
- latency: p50 and p95 of the round trip, nearest-rank in integer arithmetic, errors excluded.
  `--repeat N` times each query N times after one untimed warm-up call; a query whose ranked ids
  differ between those calls is flagged `nondeterministic` and counted in the report.

A failed call counts as a miss, so a broken server cannot score well. A reply that has no list
under `results`, `products`, `items` or `hits`, or that is neither structured content nor JSON
text, is an error for that query, not an empty result. Use `--search-tool` for a store whose tool
has another name; each entry's id is read from `id`, `sku`, `part_number` or `product_id`.

`no_match` queries are reported in their own line (how many returned nothing, how many results
on average) and are excluded from every hit, MRR, nDCG and recall aggregate, including the "all"
rows; they cannot be ranked. Whether a store should return nothing or a weak guess is a product
choice, so the tool level only describes it. At the assistant level they count: see below.

`--kind <kind>` (repeatable) runs only queries of those kinds; the report's `meta` records them.

`mcp-commerce-evals render <results.json>` prints the Markdown tables for a saved report without
any network call. It reads only the per-query entries and scores them again, so the intervals and
columns follow the current report code, and a file written by an older version still reads
(fields added later default to an empty or zero value).

The process exits 0 on success, 1 when the run could not complete, 2 when any query errored (in
the tool-level or the assistant-level run) and 3 when a gate was not met: `--min-hit5 <f64>` and
`--min-mrr <f64>` compare against the "all" row, `--min-accuracy <f64>` against the
assistant-level share of correct committed answers (it needs `--assistant`), the failed gate
is printed, and a run that meets its gates prints each threshold with the measured score. Query errors of the `no_match` kind count toward exit 2. A run whose report is incomplete exits 1. `--top-k` takes 1 to 100, `--max-turns` 1 to
50 and `--repeat` at least 1; other values are refused before any request. After connecting, the
CLI reads the search tool's `inputSchema.properties.limit.maximum` from `tools/list`; a `--top-k`
above it (the reference store accepts 25) stops the run with exit status 1 and an error naming both
numbers, before any query, rather than clamping silently into a score that is not the one asked for.
A store that lists no such maximum is not checked. The JSON report is written first, and `--out` refuses to replace an
existing file without `--force`. The report starts with a `meta` block: crate version, git commit of
the build (`+dirty` if the tree had changes), SHA-256 of the queries file, `top_k`, the search tool,
the server's `serverInfo` and negotiated protocol version, and for assistant runs the model,
temperature (null when none was sent), `max_turns` and a hash of the system prompt.
`--timeout-secs` (default 60) bounds every HTTP request, to the store and to the model API. The MCP
client reads JSON and event-stream replies (an event stream is read as it arrives, and the call
returns when the reply to its request does, even if the server keeps the stream open), answers a
server `ping` seen in the stream, carries a session id when the server issues one, opens a new
session once if the server answers 404 to it, and ends the session with a DELETE (a 405 is fine)
when the run completes. It asks for protocol revision 2025-11-25 in `initialize`, sends the
`mcp-protocol-version` the server negotiates back (2025-11-25, 2025-06-18 or 2025-03-26) and fails with a clear message on any other. On a non-2xx reply the error
carries the JSON-RPC message from the body when there is one. Server-initiated requests other
than `ping` (sampling, roots) are not supported.

**Assistant-level (optional, costs tokens).** `--assistant anthropic` gives a model the store's
tools through the Anthropic Messages API and lets it work until it answers, at most `--max-turns`
(default 8). No `temperature` is sent unless `--temperature <0-1>` is given, because some models
reject the parameter; the report records `null` or the value. `--max-tokens` (default 1024) caps
each model reply and is recorded in `meta.assistant` with the temperature; both flags need
`--assistant`. Each tool call is stored in `tools_used` with the arguments the model sent (older
files hold bare tool names and still parse). The `tools/list` call follows `nextCursor` pages (at most 20). A tool that returns only structured
content reaches the model as that JSON. The system prompt does not name a kind of store and asks
the model to end its final reply with a line `ANSWER: <part id>` (or `ANSWER: none`). The answer is correct
only when that committed id equals an expected id after normalising case and punctuation; whether
the reply merely mentions an expected id is kept as a separate diagnostic (`mentions_expected`; not applicable, `n/a` in the table and `null` in the JSON, for a `no_match` query, and left out of the overall figure).
Only a `stop_reason` of `end_turn` is a final answer: `max_tokens`, `refusal`, `pause_turn` or
anything else is recorded per query and counted as unanswered (`unfinished` in the table). Of the
runs that did finish, the table counts a committed but wrong id (`wrong id`) apart from a reply
with no `ANSWER:` line at all (`no ANSWER line`), because the first is a mistake and the second is
a refusal to commit, often a question put back to the buyer. The client retries HTTP 429, 529
and 5xx up to three times with exponential backoff and jitter, honouring `retry-after`, and
refuses an empty `ANTHROPIC_API_KEY` before any request. Accuracy is reported over all queries
and over the queries without API errors, next to the error count. The report adds tool calls per
query and input and output tokens, which is where a store with vague tool descriptions shows up:
the model needs more calls to get to the same answer. The report file is rewritten after every
run, with `"complete": false` until the end, so a paid run that fails midway leaves its partial
results.

For a `no_match` query the only correct committed answer is `ANSWER: none`; for every other query
`none` is wrong. With one trial, accuracy carries a 95% Wilson interval. `--trials N` runs each query N times and
reports the mean accuracy and pass@k for k = 1..N with the unbiased estimator (the chance that at
least one of k runs drawn from the N is right). The runs of one query are not independent, so with
several trials the interval is a seeded bootstrap that resamples whole queries and keeps each
query's trials together (`n/a` under 10 queries or when every query scored the same); pass@k
and the per-query rows remain the better read. The report records the
`model` id the API returned in its replies next to the alias that was requested. The client also
retries a request timeout or a failed connect, with the same backoff and limit as an HTTP 429.

```sh
export ANTHROPIC_API_KEY=...        # read from the environment only, never from argv
cargo run --release -p mcp-commerce-evals -- --assistant anthropic --model claude-haiku-5-5
```

The loop is tested against a mock of the API: tool call, tool result, the committed answer, each
stop reason, retries, a 401 whose message must not contain the key, a request timeout, token
counts and trials.
