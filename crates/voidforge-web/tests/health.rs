//! `voidforge-web --health-check <url>` (P5.2, D-065): the web image's healthcheck. Each case
//! scripts one answer from a stand-in on a free port: exit 0 only on a 200 within 2 s; a redirect,
//! another status, a hung server or a closed port exit 1. It reads no other setting and starts no
//! telemetry: a hung collector in the environment doesn't slow it.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_voidforge-web");

/// A stand-in that answers one request with `reply` (or holds it open when `None`), and returns
/// the request line it saw.
fn stand_in(reply: Option<String>) -> (String, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_head(&mut stream);
        match reply {
            Some(reply) => stream.write_all(reply.as_bytes()).unwrap(),
            None => std::thread::sleep(Duration::from_secs(5)),
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

fn response(status: &str, extra: &str) -> String {
    let body = "<html>Voidforge</html>";
    format!(
        "HTTP/1.1 {status}\r\n{extra}content-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Runs `--health-check url` with `env` and no other setting; its exit code, stderr and time.
fn health(url: &str, env: &[(&str, &str)]) -> (i32, String, Duration) {
    let started = Instant::now();
    let out = Command::new(BIN)
        .args(["--health-check", url])
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .env_remove("VOIDFORGE_WEB_CLIENT_SECRET")
        .envs(env.iter().copied())
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stderr).unwrap(),
        started.elapsed(),
    )
}

#[test]
fn a_200_is_healthy_without_any_other_setting() {
    let (url, seen) = stand_in(Some(response("200 OK", "")));
    let (code, stderr, _) = health(&url, &[]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(seen.join().unwrap(), "GET / HTTP/1.1");
}

#[test]
fn another_status_is_not_healthy() {
    for status in [
        "503 Service Unavailable",
        "502 Bad Gateway",
        "404 Not Found",
    ] {
        let (url, _seen) = stand_in(Some(response(status, "")));
        let (code, stderr, _) = health(&url, &[]);
        assert_eq!(code, 1, "{status}: {stderr}");
        assert!(stderr.contains("answered"), "{status}: {stderr}");
    }
}

/// A redirect isn't followed: some other app answering with a 303 to a page that is up must not
/// make this one healthy.
#[test]
fn a_redirect_is_not_healthy() {
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    let location = format!("location: http://{}/\r\n", target.local_addr().unwrap());
    let (url, _seen) = stand_in(Some(response("303 See Other", &location)));
    let (code, stderr, _) = health(&url, &[]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("303"), "{stderr}");
}

#[test]
fn a_hung_server_is_not_healthy_within_the_bound() {
    let (url, _seen) = stand_in(None);
    let (code, stderr, took) = health(&url, &[]);
    assert_eq!(code, 1, "{stderr}");
    assert!(
        took >= Duration::from_millis(1_900) && took < Duration::from_millis(3_500),
        "bounded at 2 s, took {took:?}"
    );
}

#[test]
fn a_closed_port_is_not_healthy() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (code, stderr, _) = health(&format!("http://127.0.0.1:{port}/"), &[]);
    assert_eq!(code, 1, "{stderr}");
}

/// A collector that accepts and never answers holds an exporting binary at exit for seconds
/// (D-064); the health check never starts telemetry, so it isn't held.
#[test]
fn the_health_check_starts_no_telemetry() {
    let collector = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", collector.local_addr().unwrap());
    let _hold = std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in collector.incoming().flatten() {
            held.push(stream);
        }
    });
    let (url, _seen) = stand_in(Some(response("200 OK", "")));
    let (code, stderr, took) = health(&url, &[("OTEL_EXPORTER_OTLP_ENDPOINT", &endpoint)]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        took < Duration::from_secs(1),
        "no export, so no wait on the collector: took {took:?}"
    );
}
