//! `voidforge-server health` (P5.2, D-065): the containers' healthcheck. Each case scripts one
//! answer from a stand-in on a free port: exit 0 only on a 200 whose body is JSON, within 2 s; a
//! hung server, a closed port, another status or a body that isn't JSON exit 1. It starts no
//! telemetry: a hung collector in the environment doesn't slow it.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_voidforge-server");

/// A stand-in that answers one request with `reply` (or holds it open forever when `None`), and
/// returns the request line it saw.
fn stand_in(reply: Option<&'static str>) -> (String, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/openapi.json", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_head(&mut stream);
        match reply {
            Some(reply) => stream.write_all(reply.as_bytes()).unwrap(),
            // Held until the client gives up: the stream drops when the thread ends.
            None => std::thread::sleep(Duration::from_secs(10)),
        }
        request.lines().next().unwrap_or_default().to_owned()
    });
    (url, handle)
}

fn read_head(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut byte = [0; 1];
    while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
        head.push(byte[0]);
    }
    String::from_utf8_lossy(&head).into_owned()
}

fn response(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Runs `health --url url` with `env`; its exit code, stderr and how long it took.
fn health(url: &str, env: &[(&str, &str)]) -> (i32, String, Duration) {
    let started = Instant::now();
    let out = Command::new(BIN)
        .args(["health", "--url", url])
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .envs(env.iter().copied())
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stderr).unwrap(),
        started.elapsed(),
    )
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

#[test]
fn a_200_with_json_is_healthy() {
    let (url, seen) = stand_in(Some(leak(response("200 OK", r#"{"openapi":"3.1.0"}"#))));
    let (code, stderr, _) = health(&url, &[]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(seen.join().unwrap(), "GET /openapi.json HTTP/1.1");
}

#[test]
fn another_status_is_not_healthy() {
    for status in ["503 Service Unavailable", "404 Not Found", "204 No Content"] {
        let (url, _seen) = stand_in(Some(leak(response(status, r#"{"error":"x"}"#))));
        let (code, stderr, _) = health(&url, &[]);
        assert_eq!(code, 1, "{status}: {stderr}");
        assert!(stderr.contains("answered"), "{status}: {stderr}");
    }
}

/// The one thing only the JSON check refuses: a 200 whose body isn't JSON.
#[test]
fn a_200_whose_body_is_not_json_is_not_healthy() {
    let (url, _seen) = stand_in(Some(leak(response("200 OK", "<html>ok</html>"))));
    let (code, stderr, _) = health(&url, &[]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("isn't JSON"), "{stderr}");
}

#[test]
fn a_hung_server_is_not_healthy_within_the_bound() {
    // Once unmeasured: a freshly built binary's first run can wait on macOS's scan (LEARNED).
    let (warm, _seen) = stand_in(Some(leak(response("200 OK", "{}"))));
    assert_eq!(health(&warm, &[]).0, 0);
    let (url, _seen) = stand_in(None);
    let (code, stderr, took) = health(&url, &[]);
    assert_eq!(code, 1, "{stderr}");
    assert!(
        took >= Duration::from_millis(1_900) && took < Duration::from_secs(5),
        "bounded at 2 s (the stand-in holds 10 s), took {took:?}"
    );
}

#[test]
fn a_closed_port_is_not_healthy() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (code, stderr, _) = health(&format!("http://127.0.0.1:{port}/openapi.json"), &[]);
    assert_eq!(code, 1, "{stderr}");
}

/// A collector that accepts and never answers holds any binary that exports for seconds at exit
/// (D-064's hung-collector shutdown); health never starts telemetry, so it isn't held.
#[test]
fn health_starts_no_telemetry() {
    let collector = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", collector.local_addr().unwrap());
    let _hold = std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in collector.incoming().flatten() {
            held.push(stream);
        }
    });
    // Once unmeasured: a freshly built binary's first run can wait on macOS's scan (LEARNED).
    let (warm, _seen) = stand_in(Some(leak(response("200 OK", "{}"))));
    assert_eq!(health(&warm, &[]).0, 0);
    let (url, _seen) = stand_in(Some(leak(response("200 OK", "{}"))));
    let (code, stderr, took) = health(&url, &[("OTEL_EXPORTER_OTLP_ENDPOINT", &endpoint)]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        took < Duration::from_secs(3),
        "no export, so no wait on the collector (6 s when it exports): took {took:?}"
    );
}
