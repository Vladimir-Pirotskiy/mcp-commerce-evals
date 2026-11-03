//! Stopping the store: SIGTERM must end the process cleanly, and in-flight requests must finish.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(unix)]
#[test]
fn sigterm_stops_the_binary_with_status_zero() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_reference-store"))
        .args(["--bind", "127.0.0.1:0", "--parts", "50"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("start the store");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
    let banner = lines.next().expect("a banner line").expect("readable");
    assert!(banner.contains("MCP endpoint"), "{banner}");

    let sent = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("run kill");
    assert!(sent.success());

    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll") {
            break status;
        }
        assert!(Instant::now() < deadline, "the store ignored SIGTERM");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(0), "{status:?}");
}

#[tokio::test]
async fn graceful_shutdown_lets_an_in_flight_request_finish() {
    use std::sync::Arc;

    use axum::Router;
    use axum::routing::get;
    use reference_store::serve_until;
    use tokio::net::TcpListener;
    use tokio::sync::{Notify, oneshot};

    let started = Arc::new(Notify::new());
    let announce = started.clone();
    let router = Router::new().route(
        "/slow",
        get(move || {
            let announce = announce.clone();
            async move {
                announce.notify_one();
                tokio::time::sleep(Duration::from_millis(300)).await;
                "finished"
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
    let url = format!("http://{}/slow", listener.local_addr().expect("address"));
    let (stop, stopped) = oneshot::channel::<()>();
    let server = tokio::spawn(serve_until(listener, router, async move {
        let _ = stopped.await;
    }));
    let request = tokio::spawn(reqwest::get(url));
    started.notified().await;

    stop.send(()).expect("server is listening for the signal");
    let body = request.await.unwrap().unwrap().text().await.unwrap();

    assert_eq!(body, "finished");
    server.await.unwrap().expect("the server returned cleanly");
}
