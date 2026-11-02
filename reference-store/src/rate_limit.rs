//! A fixed-window request limiter, one counter per client.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Above this many tracked clients, expired windows are dropped on the next request.
const PRUNE_ABOVE: usize = 10_000;

/// Allows `limit` requests per client in each `window`.
pub struct RateLimiter {
    limit: u32,
    window: Duration,
    clients: Mutex<HashMap<String, (Instant, u32)>>,
}

impl RateLimiter {
    /// A limiter that allows `limit` requests per client per `window`.
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window,
            clients: Mutex::new(HashMap::new()),
        }
    }

    /// Counts one request from `client` at `now`. `Err` carries how long until the next window.
    pub fn check(&self, client: &str, now: Instant) -> Result<(), Duration> {
        let mut clients = self.clients.lock().unwrap_or_else(PoisonError::into_inner);
        if clients.len() > PRUNE_ABOVE {
            clients.retain(|_, (start, _)| now.duration_since(*start) < self.window);
        }
        let entry = clients.entry(client.to_owned()).or_insert((now, 0));
        if now.duration_since(entry.0) >= self.window {
            *entry = (now, 0);
        }
        if entry.1 >= self.limit {
            return Err(self.window.saturating_sub(now.duration_since(entry.0)));
        }
        entry.1 += 1;
        Ok(())
    }
}

/// What the middleware needs: the limiter and how to name a client.
#[derive(Clone)]
pub struct LimitState {
    /// The shared limiter.
    pub limiter: Arc<RateLimiter>,
    /// Name clients by the first `X-Forwarded-For` address (only behind a proxy you control).
    pub trust_forwarded_for: bool,
}

fn client_key(state: &LimitState, peer: SocketAddr, request: &Request) -> String {
    let forwarded = state
        .trust_forwarded_for
        .then(|| request.headers().get("x-forwarded-for")?.to_str().ok())
        .flatten()
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|ip| !ip.is_empty());
    forwarded.map_or_else(|| peer.ip().to_string(), str::to_owned)
}

/// Axum middleware: answers 429 with `Retry-After` once a client is over its limit.
pub async fn limit(
    State(state): State<LimitState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    let key = client_key(&state, peer, &request);
    match state.limiter.check(&key, Instant::now()) {
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
    use super::*;

    #[test]
    fn allows_up_to_the_limit_then_refuses_with_a_wait() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(limiter.check("a", t0).is_ok());
        assert!(limiter.check("a", t0).is_ok());
        let wait = limiter
            .check("a", t0 + Duration::from_secs(10))
            .unwrap_err();
        assert_eq!(wait, Duration::from_secs(50));
    }

    #[test]
    fn clients_are_counted_separately_and_windows_reset() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(limiter.check("a", t0).is_ok());
        assert!(limiter.check("b", t0).is_ok());
        assert!(limiter.check("a", t0).is_err());
        assert!(limiter.check("a", t0 + Duration::from_secs(60)).is_ok());
    }
}
