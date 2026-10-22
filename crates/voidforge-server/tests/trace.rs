//! P5.1 (D-064): each matched route's `api.request` span, captured in memory. A caller's
//! `traceparent` makes the request a child in the caller's trace; nothing else from outside is
//! read (T3: no `tracestate`, no baggage); the span names the route as declared, never the path
//! or its query.

use axum::Router;
use axum::routing::get;
use voidforge_server::api::layered;
use voidforge_telemetry::testing::capture;

const CALLER_TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const CALLER_SPAN: &str = "00f067aa0ba902b7";

/// Answers the trace id its handler runs in.
async fn whose() -> String {
    voidforge_telemetry::trace_id(&tracing::Span::current()).unwrap_or_default()
}

async fn serve() -> String {
    let app: Router = layered(Router::new().route("/planets/{id}", get(whose)), &[]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test]
async fn a_caller_s_traceparent_makes_the_request_its_child_and_nothing_else_is_read() {
    let captured = capture();
    let base = serve().await;
    let body = reqwest::Client::new()
        .get(format!("{base}/planets/7?secret=q"))
        .header("traceparent", format!("00-{CALLER_TRACE}-{CALLER_SPAN}-01"))
        .header("tracestate", "outside=injected")
        .header("baggage", "user=injected")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, CALLER_TRACE, "the handler runs in the caller's trace");
    let spans = captured.trace(CALLER_TRACE);
    let request = spans
        .iter()
        .find(|s| s.name == "api.request")
        .expect("the request span");
    assert_eq!(request.parent_span_id.to_string(), CALLER_SPAN);
    assert!(
        request.span_context.trace_state().header().is_empty(),
        "no tracestate from outside"
    );
    let attrs: Vec<String> = request
        .attributes
        .iter()
        .map(|kv| format!("{}={}", kv.key, kv.value))
        .collect();
    assert!(
        attrs.contains(&"http.route=/planets/{id}".to_owned()),
        "{attrs:?}"
    );
    assert!(
        attrs.contains(&"http.request.method=GET".to_owned()),
        "{attrs:?}"
    );
    assert!(
        attrs.contains(&"http.response.status_code=200".to_owned()),
        "{attrs:?}"
    );
    for a in &attrs {
        assert!(
            !a.contains("secret") && !a.contains("injected") && !a.contains("/7"),
            "{a}"
        );
    }
}

#[tokio::test]
async fn without_a_traceparent_or_with_a_malformed_one_the_request_is_a_root() {
    let captured = capture();
    let base = serve().await;
    let client = reqwest::Client::new();
    for traceparent in [
        None,
        Some("not-a-traceparent"),
        Some("00-00000000000000000000000000000000-00f067aa0ba902b7-01"),
    ] {
        let mut req = client.get(format!("{base}/planets/1"));
        if let Some(t) = traceparent {
            req = req.header("traceparent", t);
        }
        let trace = req.send().await.unwrap().text().await.unwrap();
        assert_eq!(trace.len(), 32, "{traceparent:?}: a trace of its own");
        let spans = captured.trace(&trace);
        let request = spans
            .iter()
            .find(|s| s.name == "api.request")
            .expect("the request span");
        assert_eq!(
            request.parent_span_id,
            voidforge_telemetry::testing::SpanId::INVALID,
            "{traceparent:?}"
        );
    }
}
