//! `reference-store`: serve the demo bicycle-parts catalog over MCP.

use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use reference_store::{AllowedHost, StoreConfig, app, serve_until};
use tokio::net::TcpListener;

/// Serve the demo bicycle-parts catalog to AI assistants over MCP (Streamable HTTP).
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:8787")]
    bind: SocketAddr,
    /// Base URL of the fake checkout links returned by `create_cart`.
    #[arg(long, default_value = "https://demo-shop.example/checkout")]
    checkout_url: String,
    /// Extra Host header value to accept (repeatable); required behind a public hostname.
    #[arg(long = "allowed-host")]
    allowed_hosts: Vec<AllowedHost>,
    /// Requests per client per minute on /mcp.
    #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u32).range(1..))]
    requests_per_minute: u32,
    /// Identify clients by X-Forwarded-For (only behind a proxy that sets it).
    #[arg(long)]
    trust_forwarded_for: bool,
    /// Proxies in front of the store when --trust-forwarded-for is set: the client is the
    /// X-Forwarded-For entry that many places from the right.
    #[arg(long, default_value = "1", requires = "trust_forwarded_for")]
    trusted_proxy_hops: NonZeroUsize,
    /// Most /mcp requests handled at once; more get 503 with Retry-After.
    #[arg(long, default_value_t = 256, value_parser = clap::value_parser!(u32).range(1..))]
    max_in_flight: u32,
    /// Seconds one /mcp request may take before it gets 504.
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..))]
    request_timeout_secs: u64,
    /// Seed of the generated catalog.
    #[arg(long, default_value_t = catalog::DEMO_SEED)]
    seed: u64,
    /// Number of generated parts.
    #[arg(long, default_value_t = catalog::DEMO_SIZE)]
    parts: usize,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    match run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("reference-store: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> std::io::Result<()> {
    let listener = TcpListener::bind(args.bind).await?;
    let local = listener.local_addr()?;
    let config = StoreConfig {
        checkout_url: args.checkout_url,
        allowed_hosts: args.allowed_hosts,
        requests_per_minute: args.requests_per_minute,
        trust_forwarded_for: args.trust_forwarded_for,
        trusted_proxy_hops: args.trusted_proxy_hops,
        max_in_flight: args.max_in_flight as usize,
        request_timeout: std::time::Duration::from_secs(args.request_timeout_secs),
        ..StoreConfig::default()
    };
    let catalog = Arc::new(
        catalog::Catalog::new(
            catalog::generate(args.seed, args.parts).map_err(std::io::Error::other)?,
        )
        .map_err(std::io::Error::other)?,
    );
    println!(
        "reference-store: {} parts, MCP endpoint http://{local}/mcp",
        catalog.parts().len()
    );
    serve_until(listener, app(catalog, &config), shutdown_signal()).await?;
    println!("reference-store: shut down");
    Ok(())
}

/// Completes on ctrl-c, or on SIGTERM where there is one, which is what a container runtime or
/// a process manager sends to stop a service.
async fn shutdown_signal() {
    let ctrl_c = async {
        // If the handler cannot be installed, never complete on this branch.
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sigterm) => {
                sigterm.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_request_budget_is_rejected() {
        let refused = Args::try_parse_from(["reference-store", "--requests-per-minute", "0"]);
        assert!(refused.is_err());
    }

    #[test]
    fn an_allowed_host_with_a_scheme_or_path_fails_at_startup() {
        for bad in ["https://shop.example.org", "shop.example.org/mcp", ""] {
            let refused = Args::try_parse_from(["reference-store", "--allowed-host", bad]);
            assert!(refused.is_err(), "{bad:?}");
        }
        let ok =
            Args::try_parse_from(["reference-store", "--allowed-host", "shop.example.org:8443"]);
        assert!(ok.is_ok());
    }

    #[test]
    fn proxy_hops_need_trust_in_the_forwarded_header() {
        let alone = Args::try_parse_from(["reference-store", "--trusted-proxy-hops", "2"]);
        assert!(alone.is_err());
        let with = Args::try_parse_from([
            "reference-store",
            "--trust-forwarded-for",
            "--trusted-proxy-hops",
            "2",
        ]);
        assert!(with.is_ok());
    }
}
