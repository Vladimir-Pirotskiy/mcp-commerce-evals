# Results

Measured results of the tool-level and assistant-level runs, rendered from the files in `results/`.
A test (`evals/tests/saved_results.rs`) fails when a table row rendered from a committed file is
missing from this page.

## Tool-level results

Tool-level run on 2026-11-03, release build, store and CLI on the same machine (Apple M4 Max,
64 GiB, macOS 27.2), loopback, one run of 44 queries (40 ranked, 4 `no_match`), `--top-k 10`, `--repeat 1`, built from commit
`caa2e72` (the `git_commit` in the file's `meta` block), with the gates CI uses:

```sh
cargo build --release --workspace
./target/release/reference-store &
./target/release/mcp-commerce-evals --url http://127.0.0.1:8787/mcp \
  --out results/tool-level-2026-11-03.json --min-hit5 0.92 --min-mrr 0.91
```

Hit intervals are 95% Wilson; recall, nDCG and MRR intervals are seeded 95% bootstrap and read
`n/a` for a kind with fewer than 10 queries or whose scores are all equal (see [How the evals work](evals.md)); p50 and p95 are milliseconds. The file is `results/tool-level-2026-11-03.json`.

| kind | queries | hit@1 (95% CI) | hit@5 (95% CI) | recall@10 (95% CI) | nDCG@10 (95% CI) | MRR (95% CI) | p50 ms | p95 ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| adversarial | 1 | 0.00 (0.00-0.79) | 0.00 (0.00-0.79) | 1.00 (n/a) | 0.30 (n/a) | 0.11 (n/a) | 0.24 | 0.24 |
| compatibility | 8 | 0.88 (0.53-0.98) | 0.88 (0.53-0.98) | 0.90 (n/a) | 0.89 (n/a) | 0.89 (n/a) | 0.22 | 0.33 |
| exact_part_number | 5 | 1.00 (0.57-1.00) | 1.00 (0.57-1.00) | 1.00 (n/a) | 1.00 (n/a) | 1.00 (n/a) | 0.27 | 0.47 |
| filter | 6 | 0.83 (0.44-0.97) | 0.83 (0.44-0.97) | 0.83 (n/a) | 0.83 (n/a) | 0.83 (n/a) | 0.18 | 0.36 |
| plain_description | 8 | 0.88 (0.53-0.98) | 1.00 (0.68-1.00) | 1.00 (n/a) | 0.94 (n/a) | 0.92 (n/a) | 0.24 | 0.31 |
| synonym | 6 | 1.00 (0.61-1.00) | 1.00 (0.61-1.00) | 1.00 (n/a) | 1.00 (n/a) | 1.00 (n/a) | 0.27 | 0.38 |
| typo | 6 | 1.00 (0.61-1.00) | 1.00 (0.61-1.00) | 1.00 (n/a) | 1.00 (n/a) | 1.00 (n/a) | 0.30 | 0.50 |
| **all** | 40 | 0.90 (0.77-0.96) | 0.93 (0.80-0.97) | 0.96 (0.89-1.00) | 0.92 (0.84-0.99) | 0.91 (0.82-0.98) | 0.26 | 0.42 |
| **all except filter** | 34 | 0.91 (0.77-0.97) | 0.94 (0.81-0.98) | 0.98 (0.93-1.00) | 0.94 (0.86-1.00) | 0.93 (0.84-1.00) | 0.27 | 0.47 |

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

## Assistant-level results

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
The tables below are the output of `mcp-commerce-evals render` on the two saved files, made with
the report code of this commit; no API call was made and the model was not asked again. `render`
scores the per-run entries again and ignores the summaries stored in the file, which the older
build `e0f398e` wrote with Wilson intervals only (those stored figures are unchanged). Nothing was
missing: both files keep every run with its `trial` number, which is what the cluster bootstrap
needs. So Haiku's intervals resample whole queries (3 trials each), and a kind with fewer than 10
queries, or one that scored the same in every run, reads `n/a`; the overall Haiku interval moved
from 0.88-0.97 (Wilson, runs counted as independent) to 0.87-0.99. Sonnet ran one trial per query,
so its intervals are Wilson. The columns `wrong id` (committed an id that is not expected, `none`
included) and `no ANSWER line` (finished its turn without committing) are new; they were computed
from the per-run `committed` field. A test (`evals/tests/saved_results.rs`) fails when a row
rendered from a committed file is missing from this page.

**claude-haiku-5-5**, 3 trials per query: pass@1 0.94, pass@2 0.95, pass@3 0.95. Tokens over all 132
runs: 1,196,419 input and 76,890 output.

| kind | runs | committed answer correct (95% CI) | correct, API errors excluded | mentions expected | wrong id | no ANSWER line | unfinished | API errors | tool calls per query | input tokens | output tokens |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| adversarial | 3 | 0.67 (n/a) | 0.67 | 1.00 | 1 | 0 | 0 | 0 | 3.7 | 42451 | 2869 |
| compatibility | 24 | 0.88 (n/a) | 0.88 | 0.96 | 0 | 3 | 0 | 0 | 1.5 | 197436 | 15026 |
| exact_part_number | 15 | 1.00 (n/a) | 1.00 | 1.00 | 0 | 0 | 0 | 0 | 1.1 | 94366 | 4896 |
| filter | 18 | 1.00 (n/a) | 1.00 | 1.00 | 0 | 0 | 0 | 0 | 1.0 | 118650 | 10351 |
| no_match | 12 | 1.00 (n/a) | 1.00 | n/a | 0 | 0 | 0 | 0 | 2.9 | 194862 | 6821 |
| plain_description | 24 | 1.00 (n/a) | 1.00 | 1.00 | 0 | 0 | 0 | 0 | 2.5 | 254538 | 13035 |
| synonym | 18 | 0.94 (n/a) | 0.94 | 1.00 | 0 | 1 | 0 | 0 | 1.5 | 139987 | 10661 |
| typo | 18 | 0.83 (n/a) | 0.83 | 1.00 | 3 | 0 | 0 | 0 | 1.3 | 154129 | 13231 |
| **all** | 132 | 0.94 (0.87-0.99) | 0.94 | 0.99 | 4 | 4 | 0 | 0 | 1.7 | 1196419 | 76890 |

**claude-sonnet-5-5**, 1 trial per query. Tokens over all 44 runs: 473,526 input and 19,703 output.

| kind | runs | committed answer correct (95% CI) | correct, API errors excluded | mentions expected | wrong id | no ANSWER line | unfinished | API errors | tool calls per query | input tokens | output tokens |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| adversarial | 1 | 1.00 (0.21-1.00) | 1.00 | 1.00 | 0 | 0 | 0 | 0 | 4.0 | 15318 | 590 |
| compatibility | 8 | 0.88 (0.53-0.98) | 0.88 | 1.00 | 1 | 0 | 0 | 0 | 2.4 | 94324 | 4390 |
| exact_part_number | 5 | 1.00 (0.57-1.00) | 1.00 | 1.00 | 0 | 0 | 0 | 0 | 1.8 | 35770 | 1140 |
| filter | 6 | 1.00 (0.61-1.00) | 1.00 | 1.00 | 0 | 0 | 0 | 0 | 1.2 | 45297 | 2551 |
| no_match | 4 | 1.00 (0.51-1.00) | 1.00 | n/a | 0 | 0 | 0 | 0 | 3.0 | 58661 | 1552 |
| plain_description | 8 | 1.00 (0.68-1.00) | 1.00 | 1.00 | 0 | 0 | 0 | 0 | 2.5 | 101691 | 3338 |
| synonym | 6 | 1.00 (0.61-1.00) | 1.00 | 1.00 | 0 | 0 | 0 | 0 | 2.2 | 71747 | 2934 |
| typo | 6 | 1.00 (0.61-1.00) | 1.00 | 1.00 | 0 | 0 | 0 | 0 | 1.2 | 50718 | 3208 |
| **all** | 44 | 0.98 (0.88-1.00) | 0.98 | 1.00 | 1 | 0 | 0 | 0 | 2.1 | 473526 | 19703 |

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
Haiku trials the interval resamples whole queries, so it does not treat the three runs of a
query as independent, but it rests on 44 queries and is wide; pass@k and the per-query rows are
the better read. Sonnet has a single trial, so its figure says nothing about
run-to-run spread. The numbers above come from the 2026-11-03 files.

The `mentions_expected` diagnostic in these two files was produced before it was made not
applicable to `no_match` queries: their `by_kind.no_match` and `overall` figures count the
`no_match` runs as zero (overall 0.90 for Haiku, 0.91 for Sonnet). The tables above leave that column out; computed over the
queries that have an expected id, it is 0.99 for Haiku (119 of 120 runs) and 1.00 for Sonnet. The
files are as the run wrote them and were not edited.
