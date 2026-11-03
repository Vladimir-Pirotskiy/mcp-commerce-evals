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
}

/// Allows `limit` requests per client in each `window`.
pub struct RateLimiter {
    limit: u32,
    window: Duration,
    max_clients: usize,
    table: Mutex<Table>,
}

impl RateLimiter {
    /// A limiter that allows `limit` requests per client per `window`.
    pub fn new(limit: u32, window: Duration) -> Self {
        Self::with_capacity(limit, window, MAX_CLIENTS)
    }

    fn with_capacity(limit: u32, window: Duration, max_clients: usize) -> Self {
        let now = Instant::now();
        Self {
            limit,
            window,
            max_clients,
            table: Mutex::new(Table {
                clients: HashMap::new(),
                overflow: Window::new(now),
                last_prune: now,
            }),
        }
    }

    /// Counts one request from `client` at `now`. `Err` carries how long until the next window.
    pub fn check(&self, client: IpAddr, now: Instant) -> Result<(), Duration> {
        let mut table = self.table.lock().unwrap_or_else(PoisonError::into_inner);
        if now.duration_since(table.last_prune) >= self.window {
            // At most one sweep per window keeps the cost per request constant on average.
            table
                .clients
                .retain(|_, w| now.duration_since(w.start) < self.window);
            table.last_prune = now;
        }
        let Table {
            clients, overflow, ..
        } = &mut *table;
        let slot = if clients.contains_key(&client) || clients.len() < self.max_clients {
            clients.entry(client).or_insert_with(|| Window::new(now))
        } else {
            overflow
        };
        self.count(slot, now)
    }

    fn count(&self, slot: &mut Window, now: Instant) -> Result<(), Duration> {
        if now.duration_since(slot.start) >= self.window {
            *slot = Window::new(now);
        }
        if slot.used >= self.limit {
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
/// proxy wrote. Entries further left are client-supplied and never read. `None` when the header
/// is missing, short, not UTF-8 or the entry is not an IP address.
fn forwarded_address(headers: &HeaderMap, hops: NonZeroUsize) -> Option<IpAddr> {
    let mut entries = Vec::new();
    for value in headers.get_all("x-forwarded-for") {
        entries.extend(value.to_str().ok()?.split(','));
    }
    let index = entries.len().checked_sub(hops.get())?;
    entries.get(index)?.trim().parse().ok()
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
        assert!(
            limiter.check(ip("10.0.0.3"), t0).is_ok(),
            "first overflow client"
        );
        assert!(
            limiter.check(ip("10.0.0.4"), t0).is_err(),
            "shares the overflow budget"
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
    fn the_rightmost_forwarded_entry_names_the_client() {
        let map = headers(&["1.1.1.1, 2.2.2.2", "203.0.113.9"]);
        assert_eq!(forwarded_address(&map, hops(1)), Some(ip("203.0.113.9")));
        assert_eq!(forwarded_address(&map, hops(2)), Some(ip("2.2.2.2")));
        assert_eq!(forwarded_address(&map, hops(4)), None);
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
