//! A fixed-window request limiter, one counter per client.
//!
//! Clients are named by IP address: the socket peer, or, behind a trusted proxy, the address
//! that proxy appended to `X-Forwarded-For`. The table is bounded, so a flood of distinct
//! addresses cannot grow memory without limit.
//!
//! Windows are fixed, not sliding: a client that spends its whole budget at the end of one window
//! and again at the start of the next gets up to twice the budget within one window's length.
//! That burst is bounded by 2x, which is fine for protecting a demo server; a sliding window or
//! token bucket would remove it at the cost of per-client state.
//!
//! # When the table is full
//!
//! Once [`MAX_CLIENTS`] distinct clients are tracked, every further client is counted in one
//! shared overflow bucket until a sweep frees slots. The bucket has its own budget,
//! [`OVERFLOW_FACTOR`] (10) times a single client's limit per window, because it stands in for
//! many clients, not one. The alternatives and what each costs:
//!
//! - Evicting a tracked client to make room lets an attacker who rotates addresses push real
//!   clients out and reset their counters, which defeats the limit exactly when it matters.
//! - Admitting untracked clients without counting fails open under the very flood the table
//!   bound exists for.
//! - A shared bucket fails closed: total traffic from untracked clients is bounded, at the price
//!   that during an address flood newcomers compete for one budget and some get 429 although they
//!   did nothing wrong. Tracked clients are unaffected. For a demo server that trade is the right
//!   one; a production limiter would keep this state in an external store.
//!
//! Before a newcomer goes to the overflow bucket, windows that have already ended are evicted, so
//! an idle table never pushes anyone into it. Who pays when the table is truly full of live
//! clients: the newcomers, who share one budget and may get 429; tracked clients keep their own.
//!
//! # Where it runs
//!
//! The limiter wraps the MCP service, and the `Host` and `Origin` checks run inside that service
//! (rmcp does not expose them as a separate layer), so a request refused for its `Host` or
//! `Origin` still spends one unit of its client's budget.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Most distinct clients tracked at once. Further clients share one overflow budget until an
/// expired window frees a slot.
pub const MAX_CLIENTS: usize = 10_000;

/// The overflow budget is this many times a single client's limit per window. See the module
/// documentation for why it is larger, and what it costs.
pub const OVERFLOW_FACTOR: u32 = 10;

/// Least time between two sweeps forced by a full table. Shorter than any sensible window, so a
/// freed slot is found within a second, and long enough that a flood of new addresses sweeps once
/// a second at most instead of once per request.
const FULL_TABLE_SWEEP_GAP: Duration = Duration::from_secs(1);

/// Bits of an IPv6 address that name a client: one subscriber usually owns a whole /64.
const IPV6_PREFIX_BITS: u32 = 64;

/// One client's window: when it started and how many requests it has used.
#[derive(Clone, Copy)]
struct Window {
    start: Instant,
    used: u32,
}

impl Window {
    fn new(now: Instant) -> Self {
        Self {
            start: now,
            used: 0,
        }
    }
}

struct Table {
    clients: HashMap<IpAddr, Window>,
    overflow: Window,
    last_prune: Instant,
    /// How many times the whole table has been swept; read by the tests.
    sweeps: u64,
}

/// Allows `limit` requests per client in each `window`.
pub struct RateLimiter {
    limit: u32,
    overflow_limit: u32,
    window: Duration,
    max_clients: usize,
    table: Mutex<Table>,
}

impl RateLimiter {
    /// A limiter that allows `limit` requests per client per `window`.
    #[cfg(test)]
    pub fn new(limit: u32, window: Duration) -> Self {
        Self::with_capacity(limit, window, MAX_CLIENTS)
    }

    /// Like [`RateLimiter::new`], tracking at most `max_clients` clients individually.
    pub fn with_capacity(limit: u32, window: Duration, max_clients: usize) -> Self {
        let now = Instant::now();
        Self {
            limit,
            overflow_limit: limit.saturating_mul(OVERFLOW_FACTOR),
            window,
            max_clients,
            table: Mutex::new(Table {
                clients: HashMap::new(),
                overflow: Window::new(now),
                last_prune: now,
                sweeps: 0,
            }),
        }
    }

    /// Counts one request from `client` at `now`. `Err` carries how long until the next window.
    pub fn check(&self, client: IpAddr, now: Instant) -> Result<(), Duration> {
        let mut table = self.table.lock().unwrap_or_else(PoisonError::into_inner);
        if now.duration_since(table.last_prune) >= self.window {
            // At most one sweep per window keeps the cost per request constant on average.
            self.sweep(&mut table, now);
        }
        if !table.clients.contains_key(&client)
            && table.clients.len() >= self.max_clients
            && now.duration_since(table.last_prune) >= FULL_TABLE_SWEEP_GAP
        {
            // Full: windows that ended since the last sweep are dead weight, so free them before
            // pushing a newcomer into the shared overflow bucket. Rate-limited through
            // `last_prune`, so a flood of new addresses cannot make every request walk the map.
            self.sweep(&mut table, now);
        }
        let Table {
            clients, overflow, ..
        } = &mut *table;
        if clients.contains_key(&client) || clients.len() < self.max_clients {
            let slot = clients.entry(client).or_insert_with(|| Window::new(now));
            self.count(slot, self.limit, now)
        } else {
            self.count(overflow, self.overflow_limit, now)
        }
    }

    /// Drops every client whose window has ended and records when it happened.
    fn sweep(&self, table: &mut Table, now: Instant) {
        table
            .clients
            .retain(|_, w| now.duration_since(w.start) < self.window);
        table.last_prune = now;
        table.sweeps += 1;
    }

    fn count(&self, slot: &mut Window, limit: u32, now: Instant) -> Result<(), Duration> {
        if now.duration_since(slot.start) >= self.window {
            *slot = Window::new(now);
        }
        if slot.used >= limit {
            return Err(self.window.saturating_sub(now.duration_since(slot.start)));
        }
        slot.used += 1;
        Ok(())
    }
}

/// What the middleware needs: the limiter and how to name a client.
#[derive(Clone)]
pub struct LimitState {
    /// The shared limiter.
    pub limiter: Arc<RateLimiter>,
    /// When set, name a client by the `X-Forwarded-For` entry this many places from the right
    /// (1 is the address the nearest trusted proxy appended). Behind a proxy you control only.
    pub trusted_proxy_hops: Option<NonZeroUsize>,
}

/// The part of an address that identifies a client: the full IPv4 address, or the /64 of an
/// IPv6 one, with IPv4-mapped IPv6 addresses folded into IPv4.
fn client_network(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => {
            let mask = !0_u128 << (128 - IPV6_PREFIX_BITS);
            IpAddr::V6(Ipv6Addr::from(u128::from(v6) & mask))
        }
        v4 @ IpAddr::V4(_) => v4,
    }
}

/// The address `hops` entries from the right of the `X-Forwarded-For` header(s), which a trusted
/// proxy wrote. Entries further left are client-supplied and never read. Entries are counted on
/// the raw bytes, so a header line that is not UTF-8 spoils only the entry it sits in: a bad
/// client-written entry left of the trusted one changes nothing, and a bad trusted entry is
/// unusable. `None` when the header is missing or short, or the entry is not an IP address; the
/// caller then names the client by the socket peer.
fn forwarded_address(headers: &HeaderMap, hops: NonZeroUsize) -> Option<IpAddr> {
    let mut entries: Vec<&[u8]> = Vec::new();
    for value in headers.get_all("x-forwarded-for") {
        entries.extend(value.as_bytes().split(|byte| *byte == b','));
    }
    let index = entries.len().checked_sub(hops.get())?;
    std::str::from_utf8(entries.get(index)?)
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn client_key(state: &LimitState, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
    let forwarded = state
        .trusted_proxy_hops
        .and_then(|hops| forwarded_address(headers, hops));
    client_network(forwarded.unwrap_or_else(|| peer.ip()))
}

/// Axum middleware: answers 429 with `Retry-After` once a client is over its limit.
pub async fn limit(
    State(state): State<LimitState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    let key = client_key(&state, peer, request.headers());
    match state.limiter.check(key, Instant::now()) {
        Ok(()) => next.run(request).await,
        Err(wait) => {
            let seconds = wait.as_secs().max(1);
            let mut response = (
                StatusCode::TOO_MANY_REQUESTS,
                "rate limit exceeded; slow down\n",
            )
                .into_response();
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(seconds));
            response
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    fn ip(text: &str) -> IpAddr {
        text.parse().expect("a test address")
    }

    fn headers(values: &[&str]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for value in values {
            map.append(
                "x-forwarded-for",
                HeaderValue::from_str(value).expect("header"),
            );
        }
        map
    }

    fn hops(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).expect("non-zero")
    }

    #[test]
    fn allows_up_to_the_limit_then_refuses_with_a_wait() {
        let limiter = RateLimiter::new(2, MINUTE);
        let t0 = Instant::now();
        assert!(limiter.check(ip("10.0.0.1"), t0).is_ok());
        assert!(limiter.check(ip("10.0.0.1"), t0).is_ok());
        let wait = limiter
            .check(ip("10.0.0.1"), t0 + Duration::from_secs(10))
            .unwrap_err();
        assert_eq!(wait, Duration::from_secs(50));
    }

    #[test]
    fn clients_are_counted_separately_and_a_new_window_resets_the_count() {
        let limiter = RateLimiter::new(1, MINUTE);
        let t0 = Instant::now();
        assert!(limiter.check(ip("10.0.0.1"), t0).is_ok());
        assert!(limiter.check(ip("10.0.0.2"), t0).is_ok());
        assert!(limiter.check(ip("10.0.0.1"), t0).is_err());
        assert!(limiter.check(ip("10.0.0.1"), t0 + MINUTE).is_ok());
    }

    #[test]
    fn a_full_table_sends_new_clients_to_one_shared_overflow_budget() {
        let limiter = RateLimiter::with_capacity(1, MINUTE, 2);
        let t0 = Instant::now();
        assert!(limiter.check(ip("10.0.0.1"), t0).is_ok());
        assert!(limiter.check(ip("10.0.0.2"), t0).is_ok());
        for n in 0..OVERFLOW_FACTOR {
            let client = ip(&format!("10.0.1.{n}"));
            assert!(limiter.check(client, t0).is_ok(), "overflow request {n}");
        }
        assert!(
            limiter.check(ip("10.0.2.1"), t0).is_err(),
            "the shared budget of {OVERFLOW_FACTOR} is spent"
        );
        assert!(
            limiter.check(ip("10.0.0.1"), t0).is_err(),
            "tracked clients keep their own, smaller budget"
        );
        let table = limiter.table.lock().expect("lock");
        assert_eq!(table.clients.len(), 2, "the table did not grow");
    }

    #[test]
    fn expired_windows_are_swept_once_per_window_and_free_their_slots() {
        let limiter = RateLimiter::with_capacity(1, MINUTE, 2);
        let t0 = Instant::now();
        assert!(limiter.check(ip("10.0.0.1"), t0).is_ok());
        assert!(limiter.check(ip("10.0.0.2"), t0).is_ok());
        let later = t0 + MINUTE;
        assert!(limiter.check(ip("10.0.0.3"), later).is_ok());
        let table = limiter.table.lock().expect("lock");
        assert_eq!(table.clients.len(), 1, "only the new client is tracked");
        assert!(table.clients.contains_key(&ip("10.0.0.3")));
    }

    #[test]
    fn a_full_table_frees_expired_windows_before_using_the_overflow_bucket() {
        let limiter = RateLimiter::with_capacity(1, MINUTE, 2);
        let t0 = Instant::now();
        let secs = Duration::from_secs;
        assert!(limiter.check(ip("10.0.0.1"), t0).is_ok());
        assert!(limiter.check(ip("10.0.0.2"), t0 + secs(10)).is_ok());
        // The sweep at t0+60 drops the first client only, and the next one is due a minute on.
        assert!(limiter.check(ip("10.0.0.3"), t0 + secs(60)).is_ok());

        // The second client's window ended at t0+70, yet no sweep is due before t0+120.
        assert!(limiter.check(ip("10.0.0.4"), t0 + secs(75)).is_ok());

        let table = limiter.table.lock().expect("lock");
        assert!(
            table.clients.contains_key(&ip("10.0.0.4")),
            "tracked, not overflowed"
        );
        assert_eq!(table.overflow.used, 0);
    }

    #[test]
    fn a_flood_of_new_clients_on_a_full_table_does_not_sweep_per_request() {
        let limiter = RateLimiter::with_capacity(1, MINUTE, 2);
        let t0 = Instant::now();
        assert!(limiter.check(ip("10.0.0.1"), t0).is_ok());
        assert!(limiter.check(ip("10.0.0.2"), t0).is_ok());
        let before = limiter.table.lock().expect("lock").sweeps;
        for n in 0..1000_u32 {
            let client = IpAddr::from(std::net::Ipv4Addr::from(0x0a01_0000 + n));
            let _ = limiter.check(client, t0 + Duration::from_millis(u64::from(n % 10)));
        }
        let swept = limiter.table.lock().expect("lock").sweeps - before;
        assert!(swept <= 1, "{swept} sweeps for 1000 newcomers");
    }

    #[test]
    fn the_rightmost_forwarded_entry_names_the_client() {
        let map = headers(&["1.1.1.1, 2.2.2.2", "203.0.113.9"]);
        assert_eq!(forwarded_address(&map, hops(1)), Some(ip("203.0.113.9")));
        assert_eq!(forwarded_address(&map, hops(2)), Some(ip("2.2.2.2")));
        assert_eq!(forwarded_address(&map, hops(4)), None);
    }

    /// A tiny seeded generator, so the property loops below need no extra dependency.
    fn next(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *state >> 33
    }

    #[test]
    fn the_forwarded_hop_is_the_entry_n_from_the_right_for_any_header_split() {
        let mut state = 7;
        for _ in 0..500 {
            let count = usize::try_from(next(&mut state) % 8 + 1).expect("small");
            let addresses: Vec<IpAddr> = (0..count)
                .map(|_| {
                    IpAddr::from(std::net::Ipv4Addr::from(
                        u32::try_from(next(&mut state)).expect("31 bits"),
                    ))
                })
                .collect();
            // The same list split over several header lines at random points.
            let mut lines: Vec<String> = vec![String::new()];
            for (i, address) in addresses.iter().enumerate() {
                if i > 0 && next(&mut state).is_multiple_of(3) {
                    lines.push(String::new());
                }
                let line = lines.last_mut().expect("a line");
                if !line.is_empty() {
                    line.push_str(" , ");
                }
                line.push_str(&address.to_string());
            }
            let map = headers(&lines.iter().map(String::as_str).collect::<Vec<_>>());
            for n in 1..=count + 2 {
                let want = count.checked_sub(n).map(|i| addresses[i]);
                assert_eq!(forwarded_address(&map, hops(n)), want, "{lines:?} hop {n}");
            }
        }
    }

    #[test]
    fn forwarded_parsing_never_panics_on_arbitrary_header_bytes() {
        let mut state = 99;
        for _ in 0..2000 {
            let len = usize::try_from(next(&mut state) % 40).expect("small");
            let bytes: Vec<u8> = (0..len)
                .map(|_| {
                    // Mostly the characters that matter to the parser, some raw bytes.
                    let pick = next(&mut state) % 12;
                    match pick {
                        0 => b',',
                        1 => b' ',
                        2 => b':',
                        3 => b'.',
                        4 => b'[',
                        5 => b']',
                        6..=9 => b'0' + u8::try_from(next(&mut state) % 10).expect("digit"),
                        _ => u8::try_from(next(&mut state) % 256).expect("byte"),
                    }
                })
                .collect();
            let mut map = HeaderMap::new();
            if let Ok(value) = HeaderValue::from_bytes(&bytes) {
                map.append("x-forwarded-for", value);
            }
            for n in 1..=4 {
                let _ = forwarded_address(&map, hops(n));
            }
        }
    }

    #[test]
    fn a_forwarded_entry_that_is_not_an_address_is_not_used() {
        assert_eq!(
            forwarded_address(&headers(&["1.1.1.1, nonsense"]), hops(1)),
            None
        );
        assert_eq!(forwarded_address(&HeaderMap::new(), hops(1)), None);
    }

    #[test]
    fn bytes_that_are_not_utf8_spoil_only_the_entry_they_sit_in() {
        let mut map = HeaderMap::new();
        map.append(
            "x-forwarded-for",
            HeaderValue::from_bytes(b"\xff\xfe, 2.2.2.2").expect("header"),
        );
        map.append("x-forwarded-for", HeaderValue::from_static("203.0.113.9"));
        // The client-written bad line is left of the trusted entries and changes nothing.
        assert_eq!(forwarded_address(&map, hops(1)), Some(ip("203.0.113.9")));
        assert_eq!(forwarded_address(&map, hops(2)), Some(ip("2.2.2.2")));
        // The entry itself is garbage: unusable, so the caller falls back to the socket peer.
        assert_eq!(forwarded_address(&map, hops(3)), None);
        // Hops still count entries across the bad line, not lines.
        let mut trusted_bad = HeaderMap::new();
        trusted_bad.append("x-forwarded-for", HeaderValue::from_static("1.1.1.1"));
        trusted_bad.append(
            "x-forwarded-for",
            HeaderValue::from_bytes(b"\xff").expect("header"),
        );
        assert_eq!(forwarded_address(&trusted_bad, hops(1)), None);
        assert_eq!(
            forwarded_address(&trusted_bad, hops(2)),
            Some(ip("1.1.1.1"))
        );
    }

    #[test]
    fn ipv6_clients_share_a_budget_per_slash_64_and_mapped_ipv4_folds() {
        assert_eq!(
            client_network(ip("2001:db8:1:2:aaaa::1")),
            client_network(ip("2001:db8:1:2:bbbb::2"))
        );
        assert_ne!(
            client_network(ip("2001:db8:1:2::1")),
            client_network(ip("2001:db8:1:3::1"))
        );
        assert_eq!(client_network(ip("::ffff:192.0.2.1")), ip("192.0.2.1"));
    }

    #[test]
    fn without_a_trusted_proxy_the_header_is_ignored() {
        let state = LimitState {
            limiter: Arc::new(RateLimiter::new(1, MINUTE)),
            trusted_proxy_hops: None,
        };
        let peer: SocketAddr = "192.0.2.5:4000".parse().expect("peer");
        let key = client_key(&state, peer, &headers(&["198.51.100.1"]));
        assert_eq!(key, ip("192.0.2.5"));
    }
}
