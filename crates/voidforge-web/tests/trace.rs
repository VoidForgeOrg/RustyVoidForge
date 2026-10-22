//! P5.1 (D-064; P4.2 T5's carry): a page's API reads carry the web handler's trace in their
//! `traceparent`, so a browser action's trace starts in the web and continues in the API.

mod common;

use common::World;
use voidforge_telemetry::testing::capture;

#[tokio::test]
async fn a_page_s_api_reads_carry_its_trace() {
    let captured = capture();
    let w = World::start().await;
    let mut b = w.browser();
    assert_eq!(b.login().await.status, 303);
    let from = w.api.requests.lock().unwrap().len();
    assert_eq!(b.get("/").await.status, 200);
    let sent: Vec<_> = w.api.requests.lock().unwrap()[from..].to_vec();
    let me = sent
        .iter()
        .find(|r| r.uri == "/players/me")
        .expect("Home reads /players/me");
    let traceparent = me
        .traceparent
        .as_deref()
        .expect("the read carries a traceparent");
    let parts: Vec<&str> = traceparent.split('-').collect();
    assert_eq!(parts.len(), 4, "{traceparent}");
    let (trace, parent) = (parts[1], parts[2]);
    let spans = captured.trace(trace);
    let home = spans
        .iter()
        .find(|s| s.name == "web.home")
        .expect("the trace is the web handler's");
    assert_eq!(
        home.span_context.span_id().to_string(),
        parent,
        "the API read's parent is the handler"
    );
    // Every read the page made carries the same trace.
    for r in &sent {
        assert!(
            r.traceparent.as_deref().is_some_and(|t| t.contains(trace)),
            "{}: {:?}",
            r.uri,
            r.traceparent
        );
    }
}
