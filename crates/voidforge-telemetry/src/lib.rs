//! Tracing and OTLP setup: the one telemetry initialization every binary calls (Technical Plan
//! P0.6, decision log D-016).
//!
//! `init` installs a global `tracing` subscriber that writes INFO and above to stderr. When
//! `OTEL_EXPORTER_OTLP_ENDPOINT` is set (for the local stack, `http://localhost:4318`), spans are
//! also exported over OTLP/HTTP to `<endpoint>/v1/traces`, and metrics recorded through [`meter`]
//! to `<endpoint>/v1/metrics` every 10 s (P5.1, D-064). Without it, the binary still runs and
//! logs locally, metrics are no-ops, and nothing waits on a collector. Export never blocks the
//! caller: spans queue in a bounded batch, and both exporters run on their own threads.
//!
//! [`traceparent`] and [`set_remote_parent`] carry a trace across HTTP: the web and the bots send
//! the header, and the API joins the caller's trace from it, reading nothing else.
//!
//! Call [`Telemetry::shutdown`] before the process exits: it flushes buffered spans and reports
//! spans that could not be delivered, instead of dropping them silently.

use std::collections::HashMap;
use std::time::Duration;

use opentelemetry::propagation::{Extractor, TextMapPropagator as _};
use opentelemetry::trace::{TraceContextExt as _, TracerProvider as _};
use opentelemetry_otlp::WithExportConfig as _;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::{Layer as _, SubscriberExt as _};
use tracing_subscriber::util::SubscriberInitExt as _;

/// The standard OpenTelemetry variable naming the OTLP base URL; `/v1/traces` is appended.
pub const ENDPOINT_VAR: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";

/// How long one export may take before it counts as failed.
const EXPORT_TIMEOUT: Duration = Duration::from_secs(5);

/// Why telemetry could not start or could not deliver its spans.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The OTLP exporter could not be built for the configured endpoint.
    #[error("cannot build the OTLP exporter for {endpoint}: {source}")]
    Exporter {
        /// The traces URL the exporter was built for.
        endpoint: String,
        /// The exporter's own error.
        source: opentelemetry_otlp::ExporterBuildError,
    },
    /// A global `tracing` subscriber is already installed in this process.
    #[error("telemetry is already initialized in this process: {0}")]
    AlreadyInitialized(#[from] tracing_subscriber::util::TryInitError),
    /// Buffered spans could not be delivered when flushing or shutting down.
    #[error("spans could not be delivered to {endpoint}: {source}")]
    Delivery {
        /// The traces URL spans were sent to.
        endpoint: String,
        /// The SDK's error.
        source: opentelemetry_sdk::error::OTelSdkError,
    },
}

/// Keeps the exporters alive; [`Telemetry::shutdown`] flushes them. Dropping it without calling
/// `shutdown` still flushes, and reports a failure on stderr.
#[derive(Debug)]
#[must_use = "call Telemetry::shutdown before exit, or buffered spans may be lost"]
pub struct Telemetry {
    exporter: Option<Exporter>,
}

#[derive(Debug)]
struct Exporter {
    endpoint: String,
    provider: SdkTracerProvider,
    meters: SdkMeterProvider,
}

/// Initializes telemetry for the binary named `service_name`, which becomes the OpenTelemetry
/// `service.name`. Reads [`ENDPOINT_VAR`] to decide whether spans and metrics are exported.
pub fn init(service_name: &'static str) -> Result<Telemetry, Error> {
    let endpoint = std::env::var(ENDPOINT_VAR)
        .ok()
        .filter(|value| !value.trim().is_empty());
    let (subscriber, telemetry) = build(service_name, endpoint.as_deref())?;
    subscriber.try_init()?;
    if let Some(exporter) = &telemetry.exporter {
        opentelemetry::global::set_meter_provider(exporter.meters.clone());
    }
    Ok(telemetry)
}

impl Telemetry {
    /// Whether spans and metrics are exported over OTLP (an endpoint was configured).
    pub fn exports(&self) -> bool {
        self.exporter.is_some()
    }

    /// Flushes buffered spans and metrics and stops the exporters. Fails if they could not be
    /// delivered.
    pub fn shutdown(mut self) -> Result<(), Error> {
        match self.exporter.take() {
            Some(exporter) => exporter.shutdown(),
            None => Ok(()),
        }
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        if let Some(exporter) = self.exporter.take()
            && let Err(error) = exporter.shutdown()
        {
            eprintln!("voidforge-telemetry: {error}");
        }
    }
}

impl Exporter {
    /// Flushes the spans (a flush, unlike a shutdown, reports a rejected export), then always
    /// shuts the span exporter down, even when the flush failed: the global subscriber still holds
    /// it, so skipping shutdown would leave it running. Beside that, the metrics' shutdown exports
    /// what they hold. Side by side, and with a short shutdown after a failed flush, a collector
    /// that doesn't answer costs about one export timeout, not four (P5.1 M7). A span failure is
    /// reported first.
    fn shutdown(self) -> Result<(), Error> {
        let (spans, metrics) = std::thread::scope(|scope| {
            let metrics = scope.spawn(|| self.meters.shutdown());
            let spans = match self.provider.force_flush() {
                Ok(()) => self.provider.shutdown(),
                Err(e) => {
                    let _ = self.provider.shutdown_with_timeout(AFTER_A_FAILED_FLUSH);
                    Err(e)
                }
            };
            (spans, metrics.join())
        });
        let metrics = metrics.unwrap_or_else(|_| {
            Err(opentelemetry_sdk::error::OTelSdkError::InternalFailure(
                "the metrics shutdown panicked".to_owned(),
            ))
        });
        spans.and(metrics).map_err(|source| Error::Delivery {
            endpoint: self.endpoint,
            source,
        })
    }
}

/// The span exporter's shutdown after its flush already failed: it only has to stop.
const AFTER_A_FAILED_FLUSH: Duration = Duration::from_secs(1);

/// The OpenTelemetry trace ID of `span`, as 32 hex digits, when spans are exported.
pub fn trace_id(span: &tracing::Span) -> Option<String> {
    let context = span.context();
    let span_context = context.span().span_context().clone();
    span_context
        .is_valid()
        .then(|| span_context.trace_id().to_string())
}

/// The W3C `traceparent` header naming `span`, for an outgoing request (P5.1, D-064), when spans
/// are exported. Our own requests carry no `tracestate` or baggage.
pub fn traceparent(span: &tracing::Span) -> Option<String> {
    let mut headers = HashMap::new();
    TraceContextPropagator::new().inject_context(&span.context(), &mut headers);
    headers.remove(TRACEPARENT)
}

/// Makes the remote span an incoming request's `traceparent` names the parent of `span`, so the
/// request joins the caller's trace. Only `traceparent` is read: an outside caller can choose
/// which trace a request joins and nothing else, never `tracestate` or baggage (P5.1 T3). A
/// missing or malformed header leaves `span` a root.
pub fn set_remote_parent(span: &tracing::Span, traceparent: Option<&str>) {
    let Some(value) = traceparent else {
        return;
    };
    let context = TraceContextPropagator::new().extract(&OnlyTraceparent(value));
    if context.span().span_context().is_valid() {
        // Fails only when the span isn't exported (no OpenTelemetry layer): nothing to join.
        let _ = span.set_parent(context);
    }
}

/// Links `span` to `to`'s span, when both are exported: the host's turn names the commands it
/// carried, each in its own trace.
pub fn link(span: &tracing::Span, to: &tracing::Span) {
    let target = to.context().span().span_context().clone();
    if target.is_valid() {
        span.add_link(target);
    }
}

const TRACEPARENT: &str = "traceparent";

/// The one header the API trusts from outside.
struct OnlyTraceparent<'a>(&'a str);

impl Extractor for OnlyTraceparent<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        (key == TRACEPARENT).then_some(self.0)
    }

    fn keys(&self) -> Vec<&str> {
        vec![TRACEPARENT]
    }
}

/// Builds the subscriber without installing it, so tests can scope it to one thread.
fn build(
    service_name: &'static str,
    endpoint: Option<&str>,
) -> Result<(impl tracing::Subscriber + Send + Sync, Telemetry), Error> {
    let exporter = endpoint
        .map(|base| exporter(service_name, base))
        .transpose()?;
    let otel_layer = exporter.as_ref().map(|exporter| {
        span_layer(exporter.provider.tracer(service_name)).with_filter(LevelFilter::INFO)
    });
    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(LevelFilter::INFO),
        )
        .with(otel_layer);
    Ok((subscriber, Telemetry { exporter }))
}

/// The OpenTelemetry layer every exporting subscriber uses, the tests' capture included. It
/// records no `code.*` location attributes: a dependency's span would carry the build host's
/// absolute path, the user's home directory in it (QA's N1 on P5.1).
fn span_layer<S>(
    tracer: opentelemetry_sdk::trace::SdkTracer,
) -> tracing_opentelemetry::OpenTelemetryLayer<S, opentelemetry_sdk::trace::SdkTracer>
where
    S: tracing::Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    tracing_opentelemetry::layer()
        .with_tracer(tracer)
        .with_location(false)
}

/// How often metrics are sent while the process runs.
const METRICS_EVERY: Duration = Duration::from_secs(10);

fn exporter(service_name: &'static str, base: &str) -> Result<Exporter, Error> {
    let base = base.trim_end_matches('/');
    let endpoint = format!("{base}/v1/traces");
    let span_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint.clone())
        .with_timeout(EXPORT_TIMEOUT)
        .build()
        .map_err(|source| Error::Exporter {
            endpoint: endpoint.clone(),
            source,
        })?;
    let metrics_endpoint = format!("{base}/v1/metrics");
    let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_endpoint(metrics_endpoint.clone())
        .with_timeout(EXPORT_TIMEOUT)
        .build()
        .map_err(|source| Error::Exporter {
            endpoint: metrics_endpoint,
            source,
        })?;
    let resource = Resource::builder().with_service_name(service_name).build();
    let provider = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(span_exporter)
        .build();
    let meters = SdkMeterProvider::builder()
        .with_resource(resource)
        .with_reader(
            PeriodicReader::builder(metric_exporter)
                .with_interval(METRICS_EVERY)
                .build(),
        )
        .build();
    Ok(Exporter {
        endpoint,
        provider,
        meters,
    })
}

/// The instruments' types, for the crates that record metrics.
pub use opentelemetry::KeyValue;
pub use opentelemetry::metrics;

/// The meter a crate records its metrics with; a no-op when nothing is exported.
pub fn meter(name: &'static str) -> metrics::Meter {
    opentelemetry::global::meter(name)
}

#[cfg(feature = "testing")]
pub mod testing;

#[cfg(test)]
mod tests {
    use std::io::{BufRead as _, BufReader, Read as _, Write as _};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Instant;

    use super::*;

    /// A received HTTP request: request line, lower-cased headers and body bytes.
    struct Request {
        line: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl Request {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        }
    }

    /// An OTLP/HTTP collector on a free local port that answers every request with `status`
    /// (for example "200 OK") and hands each request to the test.
    fn collector(status: &'static str) -> (String, mpsc::Receiver<Request>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        let base = format!("http://{}", listener.local_addr().expect("local address"));
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let stream = stream.expect("accept the exporter's connection");
                let request = read_request(&stream);
                let response =
                    format!("HTTP/1.1 {status}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
                (&stream)
                    .write_all(response.as_bytes())
                    .expect("answer the exporter");
                if sender.send(request).is_err() {
                    break; // the test has finished
                }
            }
        });
        (base, receiver)
    }

    fn read_request(stream: &TcpStream) -> Request {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).expect("request line");
        let mut headers = Vec::new();
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).expect("header line");
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            let (key, value) = header.split_once(':').expect("header has a colon");
            headers.push((key.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
        let length: usize = headers
            .iter()
            .find(|(key, _)| key == "content-length")
            .map(|(_, value)| value.parse().expect("numeric content-length"))
            .expect("the exporter sends content-length");
        let mut body = vec![0; length];
        reader.read_exact(&mut body).expect("request body");
        Request {
            line: line.trim_end().to_owned(),
            headers,
            body,
        }
    }

    fn contains(haystack: &[u8], needle: &str) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
    }

    /// A port where nothing listens: bind one, then release it.
    fn closed_port_base() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        let base = format!("http://{}", listener.local_addr().expect("local address"));
        drop(listener);
        base
    }

    #[test]
    fn shutdown_delivers_metrics_to_the_configured_endpoint() {
        use opentelemetry::metrics::MeterProvider as _;
        let (base, requests) = collector("200 OK");
        let (_subscriber, telemetry) = build("telemetry-test", Some(&base)).expect("build");
        let meters = telemetry.exporter.as_ref().expect("exports").meters.clone();
        meters
            .meter("test")
            .u64_counter("probe_counter")
            .build()
            .add(3, &[]);
        telemetry.shutdown().expect("the metrics are delivered");
        let metrics = std::iter::from_fn(|| requests.recv_timeout(Duration::from_secs(10)).ok())
            .find(|r| r.line == "POST /v1/metrics HTTP/1.1")
            .expect("a metrics request arrives");
        assert!(
            contains(&metrics.body, "probe_counter"),
            "the metric is in the payload"
        );
        assert!(
            contains(&metrics.body, "telemetry-test"),
            "service.name is in the payload"
        );
    }

    /// M7: a collector that accepts and never answers doesn't slow what records spans: the batch
    /// queue is bounded and drops when full, and export runs on its own thread. Shutdown waits at
    /// most its timeouts, then reports the loss.
    #[test]
    fn a_hung_collector_never_slows_recording_and_bounds_shutdown() {
        use opentelemetry::metrics::MeterProvider as _;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        let base = format!("http://{}", listener.local_addr().expect("local address"));
        let held = thread::spawn(move || {
            let streams: Vec<_> = listener.incoming().take(8).filter_map(Result::ok).collect();
            thread::sleep(Duration::from_secs(60));
            drop(streams);
        });
        let (subscriber, telemetry) = build("telemetry-test", Some(&base)).expect("build");
        let meters = telemetry.exporter.as_ref().expect("exports").meters.clone();
        let counter = meters.meter("test").u64_counter("hung_counter").build();
        let started = Instant::now();
        tracing::subscriber::with_default(subscriber, || {
            for i in 0..10_000u64 {
                tracing::info_span!("hung_probe", i).in_scope(|| counter.add(1, &[]));
            }
        });
        let recording = started.elapsed();
        assert!(
            recording < Duration::from_secs(2),
            "10,000 spans took {recording:?}"
        );
        let stopping = Instant::now();
        assert!(
            telemetry.shutdown().is_err(),
            "undelivered telemetry is reported"
        );
        let shutdown = stopping.elapsed();
        eprintln!("recording {recording:?}, shutdown {shutdown:?}");
        assert!(
            shutdown < Duration::from_secs(8),
            "one export timeout, not four: {shutdown:?}"
        );
        drop(held);
    }

    #[test]
    fn a_traceparent_round_trips_and_nothing_else_is_read() {
        let (base, _requests) = collector("200 OK");
        let (subscriber, telemetry) = build("telemetry-test", Some(&base)).expect("build");
        tracing::subscriber::with_default(subscriber, || {
            let caller = tracing::info_span!("caller");
            let header = traceparent(&caller).expect("an exported span has a traceparent");
            let trace = trace_id(&caller).unwrap();
            assert!(header.starts_with(&format!("00-{trace}-")), "{header}");
            let callee = tracing::info_span!(parent: None, "callee");
            set_remote_parent(&callee, Some(&header));
            assert_eq!(
                trace_id(&callee).as_deref(),
                Some(trace.as_str()),
                "joined the trace"
            );
            // OnlyTraceparent answers nothing but traceparent, so tracestate never arrives.
            let extractor = OnlyTraceparent(&header);
            assert_eq!(extractor.get("tracestate"), None);
            assert_eq!(extractor.get("baggage"), None);
            assert_eq!(extractor.keys(), vec!["traceparent"]);
            for bad in [None, Some(""), Some("00-zz-yy-01")] {
                let root = tracing::info_span!(parent: None, "root");
                set_remote_parent(&root, bad);
                assert_ne!(trace_id(&root).as_deref(), Some(trace.as_str()), "{bad:?}");
            }
        });
        drop(telemetry);
    }

    #[test]
    fn shutdown_delivers_a_span_to_the_configured_endpoint() {
        let (base, requests) = collector("200 OK");
        let (subscriber, telemetry) =
            build("telemetry-test", Some(&format!("{base}/"))).expect("build with an endpoint");
        assert!(telemetry.exports());
        let trace_id = tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("bootstrap_probe");
            let trace_id = trace_id(&span).expect("an exported span has a trace ID");
            span.in_scope(|| tracing::info!("inside the probe span"));
            trace_id
        });
        assert_eq!(trace_id.len(), 32);

        telemetry.shutdown().expect("the span is delivered");

        let request = requests
            .recv_timeout(Duration::from_secs(10))
            .expect("a request arrives");
        assert_eq!(request.line, "POST /v1/traces HTTP/1.1");
        assert_eq!(
            request.header("content-type"),
            Some("application/x-protobuf")
        );
        assert!(
            contains(&request.body, "bootstrap_probe"),
            "the span name is in the payload"
        );
        assert!(
            contains(&request.body, "telemetry-test"),
            "service.name is in the payload"
        );
    }

    #[test]
    fn shutdown_reports_spans_it_could_not_deliver() {
        let base = closed_port_base();
        let (subscriber, telemetry) = build("telemetry-test", Some(&base)).expect("build");
        tracing::subscriber::with_default(subscriber, || {
            tracing::info_span!("undeliverable").in_scope(|| tracing::info!("nobody listens"));
        });

        let started = Instant::now();
        let error = telemetry
            .shutdown()
            .expect_err("delivery to a closed port fails");
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "shutdown gives up, it does not hang"
        );
        let expected = format!("{base}/v1/traces");
        assert!(matches!(&error, Error::Delivery { endpoint, .. } if *endpoint == expected));
    }

    #[test]
    fn a_failed_flush_still_stops_the_exporter() {
        // Regression (P0.6 review): a failed flush used to skip shutdown, leaving the exporter
        // alive in the subscriber, so spans recorded after shutdown were still exported.
        let (base, requests) = collector("400 Bad Request");
        let (subscriber, telemetry) = build("telemetry-test", Some(&base)).expect("build");
        let result = tracing::subscriber::with_default(subscriber, || {
            tracing::info_span!("before_shutdown")
                .in_scope(|| tracing::info!("rejected by the collector"));
            let result = telemetry.shutdown();
            tracing::info_span!("after_shutdown")
                .in_scope(|| tracing::info!("must not be exported"));
            result
        }); // the subscriber, holding the last provider handle, is dropped here

        assert!(
            matches!(result, Err(Error::Delivery { .. })),
            "the rejected flush is reported"
        );
        let first = requests
            .recv_timeout(Duration::from_secs(10))
            .expect("the flush reaches the collector");
        assert!(contains(&first.body, "before_shutdown"));
        // The batch worker exports every 5 seconds; a live exporter would send after_shutdown.
        let later = requests.recv_timeout(Duration::from_secs(7));
        assert!(
            later.is_err(),
            "no export after shutdown, but got {}",
            later.map(|r| r.line).unwrap_or_default()
        );
    }

    #[test]
    fn without_an_endpoint_it_logs_locally_and_exports_nothing() {
        let (subscriber, telemetry) =
            build("telemetry-test", None).expect("build without endpoint");
        assert!(!telemetry.exports());
        let trace_id = tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("offline");
            span.in_scope(|| tracing::info!("logged to stderr only"));
            trace_id(&span)
        });
        assert_eq!(
            trace_id, None,
            "nothing is exported, so there is no trace ID"
        );

        let started = Instant::now();
        telemetry.shutdown().expect("nothing to flush");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_second_init_in_one_process_is_an_error() {
        let first = init("telemetry-test").expect("the first init succeeds");
        let second = init("telemetry-test").expect_err("the second init fails");
        assert!(matches!(second, Error::AlreadyInitialized(_)));
        first
            .shutdown()
            .expect("nothing was recorded, so shutdown succeeds");
    }
}
