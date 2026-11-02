//! A reference MCP store: four shopping tools over a seeded catalog of fictional bicycle
//! parts, served over stateless Streamable HTTP with JSON responses.
//!
//! [`app`] builds the axum router and [`serve`] runs it; the `reference-store` binary is a thin
//! wrapper with command-line options.

mod api;
mod cart;
mod rate_limit;
mod tools;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::middleware::from_fn_with_state;
use axum::response::Json;
use axum::routing::get;
use catalog::Catalog;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use serde_json::json;
use tokio::net::TcpListener;

pub use api::*;
pub use rate_limit::RateLimiter;
pub use tools::ReferenceStore;

use rate_limit::LimitState;

/// How the store is deployed.
#[derive(Debug, Clone)]
pub struct StoreConfig {
    /// Base of the fake checkout links that `create_cart` returns.
    pub checkout_url: String,
    /// `Host` header values accepted besides loopback; set the public host for a remote
    /// deployment, otherwise the MCP endpoint answers 403.
    pub allowed_hosts: Vec<String>,
    /// Requests per client per minute on `/mcp`.
    pub requests_per_minute: u32,
    /// Name clients by `X-Forwarded-For` instead of the socket address (behind a proxy only).
    pub trust_forwarded_for: bool,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            checkout_url: "https://demo-shop.example/checkout".to_owned(),
            allowed_hosts: Vec::new(),
            requests_per_minute: 120,
            trust_forwarded_for: false,
        }
    }
}

fn mcp_config(config: &StoreConfig) -> StreamableHttpServerConfig {
    let hosts = ["localhost", "127.0.0.1", "::1"]
        .into_iter()
        .map(str::to_owned)
        .chain(config.allowed_hosts.iter().cloned());
    StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_allowed_hosts(hosts)
}

/// The store's HTTP application: `/mcp` (rate limited) and `/health`.
pub fn app(catalog: Arc<Catalog>, config: &StoreConfig) -> Router {
    let checkout_base: Arc<str> = Arc::from(config.checkout_url.trim_end_matches('/'));
    let service = StreamableHttpService::new(
        move || Ok(ReferenceStore::new(catalog.clone(), checkout_base.clone())),
        Arc::new(LocalSessionManager::default()),
        mcp_config(config),
    );
    let limit = LimitState {
        limiter: Arc::new(RateLimiter::new(
            config.requests_per_minute,
            Duration::from_secs(60),
        )),
        trust_forwarded_for: config.trust_forwarded_for,
    };
    let mcp = Router::new()
        .nest_service("/mcp", service)
        .layer(from_fn_with_state(limit, rate_limit::limit));
    Router::new()
        .route("/health", get(|| async { Json(json!({ "status": "ok" })) }))
        .merge(mcp)
}

/// Serves `app` on `listener` until the future is dropped or the server fails.
pub async fn serve(listener: TcpListener, app: Router) -> std::io::Result<()> {
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}
