//! Load protection for `/mcp`: a cap on requests in flight and a deadline per request.
//!
//! Both fail fast and say so (503 with `Retry-After`, 504), instead of queueing work the store
//! cannot finish: an overloaded demo server should shed load, not grow a backlog.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tokio::sync::Semaphore;

/// Seconds a shed client is told to wait.
const RETRY_AFTER_SECS: u64 = 1;

/// The in-flight cap and the per-request deadline.
#[derive(Clone)]
pub struct GuardState {
    permits: Arc<Semaphore>,
    timeout: Duration,
}

impl GuardState {
    /// At most `max_in_flight` requests at once, each given `timeout` to produce a response.
    pub fn new(max_in_flight: usize, timeout: Duration) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(max_in_flight)),
            timeout,
        }
    }
}

/// Axum middleware: 503 when too many requests are in flight, 504 when one outlives its deadline.
pub async fn guard(State(state): State<GuardState>, request: Request, next: Next) -> Response {
    let Ok(_permit) = state.permits.try_acquire() else {
        let mut response = (
            StatusCode::SERVICE_UNAVAILABLE,
            "too many requests in flight; retry shortly\n",
        )
            .into_response();
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(RETRY_AFTER_SECS));
        return response;
    };
    // Invariant: the permit is held only until `next.run` returns the response head, so a
    // stream that outlives it (an SSE body) never pins a slot; stateless mode refuses GET anyway.
    match tokio::time::timeout(state.timeout, next.run(request)).await {
        Ok(response) => response,
        Err(_) => (StatusCode::GATEWAY_TIMEOUT, "the request took too long\n").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use axum::Router;
    use axum::middleware::from_fn_with_state;
    use axum::routing::get;
    use tokio::net::TcpListener;
    use tokio::sync::Notify;

    use super::*;

    async fn serve(router: Router) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
        let base = format!("http://{}", listener.local_addr().expect("address"));
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        base
    }

    #[tokio::test]
    async fn a_request_over_the_in_flight_cap_is_shed_with_503_and_retry_after() {
        let release = Arc::new(Notify::new());
        let started = Arc::new(Notify::new());
        let (hold, announce) = (release.clone(), started.clone());
        let router = Router::new()
            .route(
                "/slow",
                get(move || {
                    let (hold, announce) = (hold.clone(), announce.clone());
                    async move {
                        announce.notify_one();
                        hold.notified().await;
                        "done"
                    }
                }),
            )
            .layer(from_fn_with_state(
                GuardState::new(1, Duration::from_secs(30)),
                guard,
            ));
        let base = serve(router).await;
        let first = tokio::spawn(reqwest::get(format!("{base}/slow")));
        started.notified().await;

        let shed = reqwest::get(format!("{base}/slow")).await.unwrap();

        assert_eq!(shed.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(shed.headers().contains_key("retry-after"));
        release.notify_one();
        assert_eq!(first.await.unwrap().unwrap().status(), StatusCode::OK);
        let after = reqwest::get(format!("{base}/slow"));
        release.notify_one();
        assert_eq!(
            after.await.unwrap().status(),
            StatusCode::OK,
            "the permit came back"
        );
    }

    /// Sets its flag when dropped: proof that the future holding it was cancelled (or finished).
    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn a_request_that_outlives_its_deadline_gets_504_and_is_dropped() {
        let dropped = Arc::new(AtomicBool::new(false));
        let flag = dropped.clone();
        let router = Router::new()
            .route(
                "/slow",
                get(move || {
                    let flag = flag.clone();
                    async move {
                        // Held across the await: dropped only when the handler future is.
                        let _guard = DropFlag(flag);
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        "late"
                    }
                }),
            )
            .layer(from_fn_with_state(
                GuardState::new(4, Duration::from_millis(50)),
                guard,
            ));
        let base = serve(router).await;

        let response = reqwest::get(format!("{base}/slow")).await.unwrap();

        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        // The sleep has about five seconds left, so only cancellation can have dropped the guard.
        assert!(
            dropped.load(Ordering::SeqCst),
            "the handler future was not dropped when the 504 was sent"
        );
    }
}
