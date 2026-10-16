//! Tracing and OTLP setup: the one telemetry initialization every binary calls (Technical Plan
//! P0.6, decision log D-016).
//!
//! `init` installs a global `tracing` subscriber that writes INFO and above to stderr. When
//! `OTEL_EXPORTER_OTLP_ENDPOINT` is set (for the local stack, `http://localhost:4318`), spans are
//! also exported over OTLP/HTTP to `<endpoint>/v1/traces`. Without it, the binary still runs and
//! logs locally; nothing waits on a collector.
//!
//! Call [`Telemetry::shutdown`] before the process exits: it flushes buffered spans and reports
//! spans that could not be delivered, instead of dropping them silently.

use std::time::Duration;

use opentelemetry::trace::{TraceContextExt as _, TracerProvider as _};
use opentelemetry_otlp::WithExportConfig as _;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt as _;
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

/// Keeps the exporter alive; [`Telemetry::shutdown`] flushes it. Dropping it without calling
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
}

/// Initializes telemetry for the binary named `service_name`, which becomes the OpenTelemetry
/// `service.name`. Reads [`ENDPOINT_VAR`] to decide whether spans are exported.
pub fn init(service_name: &'static str) -> Result<Telemetry, Error> {
    let endpoint = std::env::var(ENDPOINT_VAR)
        .ok()
        .filter(|value| !value.trim().is_empty());
    let (subscriber, telemetry) = build(service_name, endpoint.as_deref())?;
    subscriber.try_init()?;
    Ok(telemetry)
}

impl Telemetry {
    /// Whether spans are exported over OTLP (an endpoint was configured).
    pub fn exports(&self) -> bool {
        self.exporter.is_some()
    }

    /// Flushes buffered spans and stops the exporter. Fails if spans could not be delivered.
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
    /// Flushes, then always shuts the provider down, even when the flush failed: the global
    /// subscriber still holds the provider, so skipping shutdown would leave the exporter
    /// running. A flush failure is reported in preference to a shutdown failure.
    fn shutdown(self) -> Result<(), Error> {
        let flushed = self.provider.force_flush();
        let stopped = self.provider.shutdown();
        flushed.and(stopped).map_err(|source| Error::Delivery {
            endpoint: self.endpoint,
            source,
        })
    }
}

/// The OpenTelemetry trace ID of `span`, as 32 hex digits, when spans are exported.
pub fn trace_id(span: &tracing::Span) -> Option<String> {
    let context = span.context();
    let span_context = context.span().span_context().clone();
    span_context
        .is_valid()
        .then(|| span_context.trace_id().to_string())
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
        tracing_opentelemetry::layer().with_tracer(exporter.provider.tracer(service_name))
    });
    let subscriber = tracing_subscriber::registry()
        .with(LevelFilter::INFO)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(otel_layer);
    Ok((subscriber, Telemetry { exporter }))
}

fn exporter(service_name: &'static str, base: &str) -> Result<Exporter, Error> {
    let endpoint = format!("{}/v1/traces", base.trim_end_matches('/'));
    let span_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint.clone())
        .with_timeout(EXPORT_TIMEOUT)
        .build()
        .map_err(|source| Error::Exporter {
            endpoint: endpoint.clone(),
            source,
        })?;
    let provider = SdkTracerProvider::builder()
        .with_resource(Resource::builder().with_service_name(service_name).build())
        .with_batch_exporter(span_exporter)
        .build();
    Ok(Exporter { endpoint, provider })
}

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
