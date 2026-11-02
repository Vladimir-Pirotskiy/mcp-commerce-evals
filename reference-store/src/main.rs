//! `reference-store`: serve the demo bicycle-parts catalog over MCP.

use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use reference_store::{StoreConfig, app, serve};
use tokio::net::TcpListener;

/// Serve the demo bicycle-parts catalog to AI assistants over MCP (Streamable HTTP).
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:8787")]
    bind: SocketAddr,
    /// Base URL of the fake checkout links returned by create_cart.
    #[arg(long, default_value = "https://demo-shop.example/checkout")]
    checkout_url: String,
    /// Extra Host header value to accept (repeatable); required behind a public hostname.
    #[arg(long = "allowed-host")]
    allowed_hosts: Vec<String>,
    /// Requests per client per minute on /mcp.
    #[arg(long, default_value_t = 120)]
    requests_per_minute: u32,
    /// Identify clients by X-Forwarded-For (only behind a proxy that sets it).
    #[arg(long)]
    trust_forwarded_for: bool,
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
    };
    let catalog = Arc::new(catalog::Catalog::new(catalog::generate(
        args.seed, args.parts,
    )));
    println!(
        "reference-store: {} parts, MCP endpoint http://{local}/mcp",
        catalog.parts().len()
    );
    let server = serve(listener, app(catalog, &config));
    tokio::select! {
        result = server => result,
        _ = tokio::signal::ctrl_c() => Ok(()),
    }
}
