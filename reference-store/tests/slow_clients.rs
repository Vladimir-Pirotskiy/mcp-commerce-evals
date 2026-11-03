//! Slow and idle clients: a connection that does not finish its request line and headers is
//! closed, and connections over the cap are refused instead of queued.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use catalog::demo_catalog;
use reference_store::{ConnectionLimits, StoreConfig, app, serve_with_limits};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const HEALTH: &[u8] = b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n";

struct Running {
    address: std::net::SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<std::io::Result<()>>,
}

async fn start(header_read_timeout: Duration, max_connections: usize) -> Running {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
    let address = listener.local_addr().expect("local address");
    let router = app(Arc::new(demo_catalog()), &StoreConfig::default());
    let limits = ConnectionLimits {
        header_read_timeout,
        max_connections: NonZeroUsize::new(max_connections).expect("a positive cap"),
    };
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(serve_with_limits(listener, router, limits, async move {
        let _ = stopped.await;
    }));
    Running {
        address,
        stop: Some(stop),
        task,
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Reads until the peer closes the connection; returns what arrived, or `None` when the peer
/// kept it open past `within`.
async fn read_until_closed(stream: &mut TcpStream, within: Duration) -> Option<Vec<u8>> {
    let mut received = Vec::new();
    let read = tokio::time::timeout(within, async {
        let mut buffer = [0_u8; 1024];
        loop {
            match stream.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(n) => received.extend_from_slice(&buffer[..n]),
            }
        }
    })
    .await;
    read.ok().map(|()| received)
}

async fn connect(running: &Running) -> TcpStream {
    TcpStream::connect(running.address).await.expect("connect")
}

#[tokio::test]
async fn a_partial_request_line_is_closed_within_the_header_timeout() {
    let running = start(Duration::from_millis(300), 64).await;
    let mut stream = connect(&running).await;
    stream.write_all(b"GET /hea").await.expect("write");

    let started = Instant::now();
    let closed = read_until_closed(&mut stream, Duration::from_secs(3)).await;

    assert!(closed.is_some(), "the server kept a half request line open");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn partial_headers_are_closed_within_the_header_timeout() {
    let running = start(Duration::from_millis(300), 64).await;
    let mut stream = connect(&running).await;
    stream
        .write_all(b"POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Length: 10\r\n")
        .await
        .expect("write");

    let closed = read_until_closed(&mut stream, Duration::from_secs(3)).await;

    assert!(closed.is_some(), "the server kept half a header block open");
}

#[tokio::test]
async fn a_connection_that_sends_nothing_is_closed_too() {
    let running = start(Duration::from_millis(300), 64).await;
    let mut stream = connect(&running).await;

    let closed = read_until_closed(&mut stream, Duration::from_secs(3)).await;

    assert!(closed.is_some(), "an idle connection stayed open");
}

#[tokio::test]
async fn a_client_that_finishes_in_time_is_served() {
    let running = start(Duration::from_millis(300), 64).await;
    let mut stream = connect(&running).await;
    stream.write_all(HEALTH).await.expect("write");

    let mut buffer = [0_u8; 512];
    let n = stream.read(&mut buffer).await.expect("read");

    assert!(
        buffer[..n].starts_with(b"HTTP/1.1 200"),
        "{:?}",
        &buffer[..n]
    );
}

#[tokio::test]
async fn connections_over_the_cap_are_not_served_and_a_freed_slot_is_reused() {
    let running = start(Duration::from_secs(30), 2).await;
    let mut held = vec![connect(&running).await, connect(&running).await];
    for stream in &mut held {
        stream.write_all(b"GET /hea").await.expect("write");
    }
    // Let the server accept both before the third arrives.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut over = connect(&running).await;
    let _ = over.write_all(HEALTH).await;
    let answer = read_until_closed(&mut over, Duration::from_secs(2)).await;
    assert_eq!(
        answer,
        Some(Vec::new()),
        "a connection over the cap must get no answer"
    );

    drop(held.pop());
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let mut again = connect(&running).await;
        let _ = again.write_all(HEALTH).await;
        let mut buffer = [0_u8; 512];
        let n = tokio::time::timeout(Duration::from_secs(1), again.read(&mut buffer))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(0);
        if buffer[..n].starts_with(b"HTTP/1.1 200") {
            break;
        }
        assert!(Instant::now() < deadline, "the freed slot was never reused");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn shutdown_does_not_wait_for_an_idle_connection() {
    let mut running = start(Duration::from_secs(30), 64).await;
    let _idle = connect(&running).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    running
        .stop
        .take()
        .expect("stop once")
        .send(())
        .expect("server listening");
    let task = &mut running.task;
    let finished = tokio::time::timeout(Duration::from_secs(2), task).await;

    assert!(finished.is_ok(), "shutdown hung on an idle connection");
}
