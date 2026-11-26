//! The binary in `--stdio` mode: the handshake, the tool list and a call over a real pipe, and a
//! clean exit when the client closes stdin.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};

const DEADLINE: Duration = Duration::from_secs(20);

#[test]
fn a_client_on_stdio_lists_and_calls_the_tools_and_the_store_exits_on_eof() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_reference-store"))
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the store");
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");

    // Read stdout on a thread so that a silent store fails the test instead of hanging it.
    let (lines, received) = mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    let mut send = |message: Value| {
        writeln!(stdin, "{message}").expect("write to the store");
        stdin.flush().expect("flush");
    };
    let reply = |id: u64| -> Value {
        loop {
            let line = received.recv_timeout(DEADLINE).expect("a reply in time");
            let message: Value = serde_json::from_str(&line).expect("stdout holds only JSON");
            if message["id"] == json!(id) {
                return message;
            }
        }
    };

    send(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "stdio-test", "version": "0" }
        }
    }));
    let initialized = reply(1);
    assert!(
        initialized["result"]["capabilities"]["tools"].is_object(),
        "{initialized}"
    );
    send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));

    send(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    let listed = reply(2);
    let mut names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .map(|tool| tool["name"].as_str().expect("a name"))
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "check_availability",
            "create_cart",
            "get_product",
            "search_products"
        ]
    );

    send(json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": { "name": "search_products", "arguments": { "query": "chain", "limit": 3 } }
    }));
    let called = reply(3);
    assert_eq!(called["result"]["isError"], json!(false), "{called}");
    let results = called["result"]["structuredContent"]["results"]
        .as_array()
        .expect("search results");
    assert!(!results.is_empty(), "{called}");

    drop(send);
    drop(stdin);
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(child.wait());
    });
    let status = finished
        .recv_timeout(DEADLINE)
        .expect("the store exits after stdin closes")
        .expect("wait for the store");
    assert!(status.success(), "{status}");
    reader.join().expect("the reader thread");
}
