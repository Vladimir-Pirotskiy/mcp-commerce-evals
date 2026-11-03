//! A reference MCP store: four shopping tools over a seeded catalog of fictional bicycle
//! parts, served over stateless Streamable HTTP with JSON responses.
//!
//! [`app`] builds the axum router and [`serve`] runs it; the `reference-store` binary is a thin
//! wrapper with command-line options.

mod api;
mod cart;
mod error;
mod guard;
mod hosts;
mod input;
mod rate_limit;
mod tools;

use std::num::{NonZeroU32, NonZeroUsize};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{ConnectInfo, Request};
use axum::middleware::{Next, from_fn, from_fn_with_state};
use axum::response::Json;
use axum::routing::get;
use catalog::Catalog;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use hyper_util::service::TowerToHyperService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

pub use hosts::{AllowedHost, InvalidHost};
pub use tools::ReferenceStore;

use guard::GuardState;
use rate_limit::{LimitState, RateLimiter};

/// How the store is deployed.
#[derive(Debug, Clone)]
pub struct StoreConfig {
    /// Base of the fake checkout links that `create_cart` returns.
    pub checkout_url: String,
    /// `Host` header values accepted besides loopback; set the public host for a remote
    /// deployment, otherwise the MCP endpoint answers 403. Browser `Origin` headers are accepted
    /// from the same hosts (any scheme, any port unless the entry names one) and refused with 403
    /// otherwise; requests without an `Origin` are not affected.
    pub allowed_hosts: Vec<AllowedHost>,
    /// Requests per client per minute on `/mcp`.
    pub requests_per_minute: NonZeroU32,
    /// Name clients by `X-Forwarded-For` instead of the socket address (behind a proxy only).
    pub trust_forwarded_for: bool,
    /// With `trust_forwarded_for`: how many proxies you run in front of the store. The client is
    /// the entry that many places from the right of `X-Forwarded-For`; the client controls
    /// everything to its left.
    pub trusted_proxy_hops: NonZeroUsize,
    /// Most clients the rate limiter tracks one by one. Beyond that, new clients share one
    /// overflow budget of ten times a client's limit (see the `rate_limit` module notes).
    pub max_tracked_clients: usize,
    /// Most `/mcp` requests handled at once; a request over the cap gets 503 with `Retry-After`.
    pub max_in_flight: NonZeroUsize,
    /// How long one `/mcp` request may take before it gets 504.
    pub request_timeout: Duration,
}

const DEFAULT_REQUESTS_PER_MINUTE: NonZeroU32 = NonZeroU32::new(120).expect("120 is not zero");
const DEFAULT_MAX_IN_FLIGHT: NonZeroUsize = NonZeroUsize::new(256).expect("256 is not zero");

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            checkout_url: "https://demo-shop.example/checkout".to_owned(),
            allowed_hosts: Vec::new(),
            requests_per_minute: DEFAULT_REQUESTS_PER_MINUTE,
            trust_forwarded_for: false,
            trusted_proxy_hops: NonZeroUsize::MIN,
            max_tracked_clients: rate_limit::MAX_CLIENTS,
            max_in_flight: DEFAULT_MAX_IN_FLIGHT,
            request_timeout: Duration::from_secs(30),
        }
    }
}

/// Largest request body on `/mcp`. A full call (twenty lines, a 200-character query) is a few KiB.
/// rmcp reads the body itself, so this is its own limit: axum's `DefaultBodyLimit` only guards
/// axum extractors and would not apply to the nested service.
const MAX_BODY_BYTES: usize = 64 * 1024;

fn mcp_config(config: &StoreConfig) -> StreamableHttpServerConfig {
    let hosts = hosts::allowed_hosts(&config.allowed_hosts);
    let origins = hosts::allowed_origins(&hosts);
    StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_max_request_body_bytes(MAX_BODY_BYTES)
        .with_allowed_hosts(hosts)
        .with_allowed_origins(origins)
}

/// The store's HTTP application: `/mcp` (rate limited, capped in flight, with a deadline) and
/// `/health`.
///
/// ```
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// use std::sync::Arc;
/// use reference_store::{StoreConfig, app, serve};
///
/// let catalog = Arc::new(catalog::demo_catalog());
/// let router = app(catalog, &StoreConfig::default());
/// let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
/// let address = listener.local_addr().unwrap();
/// let server = tokio::spawn(serve(listener, router));
///
/// // `/health` answers without a session; `/mcp` speaks MCP over POST.
/// let health = reqwest::get(format!("http://{address}/health")).await.unwrap();
/// assert!(health.status().is_success());
/// server.abort();
/// # });
/// ```
pub fn app(catalog: Arc<Catalog>, config: &StoreConfig) -> Router {
    let checkout_base: Arc<str> = Arc::from(config.checkout_url.trim_end_matches('/'));
    let service = StreamableHttpService::new(
        move || Ok(ReferenceStore::new(catalog.clone(), checkout_base.clone())),
        Arc::new(LocalSessionManager::default()),
        mcp_config(config),
    );
    let limit = LimitState {
        limiter: Arc::new(RateLimiter::with_capacity(
            config.requests_per_minute.get(),
            Duration::from_secs(60),
            config.max_tracked_clients,
        )),
        trusted_proxy_hops: config
            .trust_forwarded_for
            .then_some(config.trusted_proxy_hops),
    };
    let mcp = Router::new()
        .nest_service("/mcp", service)
        .layer(from_fn_with_state(
            GuardState::new(config.max_in_flight.get(), config.request_timeout),
            guard::guard,
        ))
        .layer(from_fn_with_state(limit, rate_limit::limit));
    Router::new()
        .route("/health", get(|| async { Json(json!({ "status": "ok" })) }))
        .merge(mcp)
}

/// Serves `app` on `listener` until the future is dropped or the server fails.
///
/// # Errors
///
/// Fails when the underlying server stops with an I/O error.
pub async fn serve(listener: TcpListener, app: Router) -> std::io::Result<()> {
    serve_until(listener, app, std::future::pending()).await
}

/// Serves `app` on `listener` until `shutdown` completes, then stops accepting connections and
/// lets requests in flight finish before returning.
///
/// # Errors
///
/// Fails when the underlying server stops with an I/O error.
pub async fn serve_until(
    listener: TcpListener,
    app: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    serve_with_limits(listener, app, ConnectionLimits::default(), shutdown).await
}

/// Limits on the connections the server accepts.
#[derive(Debug, Clone, Copy)]
pub struct ConnectionLimits {
    /// How long a client may take to send the request line and headers before the connection is
    /// closed. Also bounds how long an idle kept-alive connection waits for its next request.
    pub header_read_timeout: Duration,
    /// Most connections open at once; a connection over the cap is closed without being served.
    pub max_connections: NonZeroUsize,
}

impl Default for ConnectionLimits {
    fn default() -> Self {
        Self {
            header_read_timeout: Duration::from_secs(10),
            max_connections: NonZeroUsize::new(1024).expect("1024 is not zero"),
        }
    }
}

/// Like [`serve_until`], with explicit [`ConnectionLimits`].
///
/// Connections are served over HTTP/1.1 by hyper directly, because `axum::serve` has no header
/// timeout and no cap. When `shutdown` completes the server stops accepting, closes idle
/// connections and lets requests in flight finish.
///
/// # Errors
///
/// Never fails today; the `Result` keeps the signature of [`serve_until`].
pub async fn serve_with_limits(
    listener: TcpListener,
    app: Router,
    limits: ConnectionLimits,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let permits = Arc::new(Semaphore::new(limits.max_connections.get()));
    let graceful = GracefulShutdown::new();
    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        let (stream, peer) = tokio::select! {
            () = &mut shutdown => break,
            accepted = listener.accept() => match accepted {
                Ok(connection) => connection,
                Err(error) => {
                    // Out of file descriptors and the like: back off instead of spinning.
                    eprintln!("reference-store: accept failed: {error}");
                    tokio::select! {
                        () = &mut shutdown => break,
                        () = tokio::time::sleep(Duration::from_secs(1)) => continue,
                    }
                }
            },
        };
        // The slot is taken before anything is read, so a flood cannot queue behind the cap.
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            drop(stream);
            continue;
        };
        let service = TowerToHyperService::new(app.clone().layer(from_fn(
            move |mut request: Request, next: Next| async move {
                request.extensions_mut().insert(ConnectInfo(peer));
                next.run(request).await
            },
        )));
        let connection = http1::Builder::new()
            .timer(TokioTimer::new())
            .header_read_timeout(limits.header_read_timeout)
            .serve_connection(TokioIo::new(stream), service);
        let watched = graceful.watch(connection);
        tokio::spawn(async move {
            // A client that disconnects mid-request is not an error worth reporting.
            let _ = watched.await;
            drop(permit);
        });
    }
    graceful.shutdown().await;
    Ok(())
}
