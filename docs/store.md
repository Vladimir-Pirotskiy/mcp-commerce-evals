# The reference store

Details of `reference-store` that the README only summarises: the tools, the limits it enforces and
how to expose it to a hosted assistant. The threat model is in the README.

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
is written by the client. An entry that is not an IP address, or is not valid UTF-8, falls back to the socket peer; entries
are counted on raw bytes, so a bad entry to the left of the trusted one changes nothing.
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
share one budget and some get 429 undeservedly; tracked clients are unaffected. Before a newcomer is sent to the overflow bucket, windows that have already ended are evicted, so
only a table full of live clients costs anyone: the newcomers, who share that one budget. The
reasoning is in the `rate_limit` module docs. The counters live in this process: with several
instances each has its own table and budget.

`--seed` (default 2026) and `--parts` (default 500) choose the generated catalog. The queries file
and the committed results assume the defaults; other values give another catalog. `--parts` is
accepted up to 2000, the size the generator is proven to reach for every seed it is tested with;
a larger value, or one the generator cannot fill with distinct parts, is refused at startup with
an error instead of serving a shorter catalog. Prices must be whole cents from $0.01 to $10,000.

`/mcp` also sheds load: at most 256 requests are handled at once (`--max-in-flight`; more get 503
with `Retry-After`) and a request that runs past 30 seconds (`--request-timeout-secs`) gets 504.
The store stops on ctrl-c or SIGTERM, closing idle connections and finishing requests already
in flight. Connections are capped (`--max-connections`, default 1024; a connection over the cap is
closed without an answer; the cap is global, not per IP, and the OS file-descriptor limit
(`ulimit -n`) may need raising to reach it) and read on a clock (`--header-read-timeout-secs`, default 10) so that a
client that does not finish its request line and headers is closed.
`--allowed-host` takes a bare host with an optional port (`shop.example.org:8443`); a scheme, a
path or other junk is refused at startup. Inputs: a query
is at most 200 characters and 20 words, `limit` is 1 to 25 and anything else is refused, `brand`,
each `fits` entry and every part id are at most 100 characters, `fits` takes at most 10 non-empty
entries, a request body is at most 64 KiB, and carts take at most 20 lines. A part sent on several
cart lines counts once with the lines added up: the total must fit the shelf and 99 units.
