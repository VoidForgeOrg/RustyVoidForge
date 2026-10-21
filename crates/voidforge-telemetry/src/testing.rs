//! In-memory capture of spans and metrics, for other crates' tests (feature `testing`; P5.1,
//! D-064). Never in a binary.
//!
//! [`capture`] installs, once per process, a global subscriber whose OpenTelemetry layer exports to
//! memory and a global meter provider that reads into memory. Global, because the host runs its
//! own thread, and once, so it works under nextest (a process per test) and plain `cargo test`
//! (threads in one process) alike. Tests sharing a process see each other's spans: find yours by
//! its trace id.

use std::sync::OnceLock;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use tracing_subscriber::layer::{Layer as _, SubscriberExt as _};
use tracing_subscriber::util::SubscriberInitExt as _;

pub use opentelemetry::trace::{SpanId, Status, TraceId};
pub use opentelemetry_sdk::trace::SpanData;

/// What the process has recorded.
#[derive(Debug)]
pub struct Captured {
    spans: InMemorySpanExporter,
    tracer: SdkTracerProvider,
    metrics: InMemoryMetricExporter,
    meters: SdkMeterProvider,
}

/// The process's capture, installed on first use.
///
/// # Panics
/// If another global subscriber was installed first.
pub fn capture() -> &'static Captured {
    static CAPTURED: OnceLock<Captured> = OnceLock::new();
    CAPTURED.get_or_init(|| {
        let spans = InMemorySpanExporter::default();
        let tracer = SdkTracerProvider::builder()
            .with_simple_exporter(spans.clone())
            .build();
        let metrics = InMemoryMetricExporter::default();
        let meters = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(metrics.clone()).build())
            .build();
        tracing_subscriber::registry()
            .with(
                tracing_opentelemetry::layer()
                    .with_tracer(tracer.tracer("test"))
                    .with_filter(tracing_subscriber::filter::LevelFilter::INFO),
            )
            .try_init()
            .expect("no other global subscriber in a test that captures");
        opentelemetry::global::set_meter_provider(meters.clone());
        Captured {
            spans,
            tracer,
            metrics,
            meters,
        }
    })
}

impl Captured {
    /// Every finished span so far.
    pub fn spans(&self) -> Vec<SpanData> {
        let _ = self.tracer.force_flush();
        self.spans.get_finished_spans().unwrap_or_default()
    }

    /// The finished spans of one trace, by its hex id ([`crate::trace_id`]).
    pub fn trace(&self, id: &str) -> Vec<SpanData> {
        self.spans()
            .into_iter()
            .filter(|s| s.span_context.trace_id().to_string() == id)
            .collect()
    }

    /// Collects metrics now, and returns the latest reading of each.
    fn read(&self) -> Vec<ResourceMetrics> {
        self.metrics.reset();
        let _ = self.meters.force_flush();
        self.metrics.get_finished_metrics().unwrap_or_default()
    }

    /// The data points of the metric `name` now, as (value, attributes as `key=value` text): a
    /// gauge's or a sum's value, a histogram's count.
    pub fn points(&self, name: &str) -> Vec<(f64, Vec<String>)> {
        let mut out = Vec::new();
        for rm in self.read() {
            for scope in rm.scope_metrics() {
                for m in scope.metrics().filter(|m| m.name() == name) {
                    push_points(m.data(), &mut out);
                }
            }
        }
        out
    }
}

fn attrs<'a>(kv: impl Iterator<Item = &'a opentelemetry::KeyValue>) -> Vec<String> {
    kv.map(|kv| format!("{}={}", kv.key, kv.value)).collect()
}

#[allow(clippy::cast_precision_loss)] // test readings, far below 2^52
fn push_points(data: &AggregatedMetrics, out: &mut Vec<(f64, Vec<String>)>) {
    macro_rules! points {
        ($d:expr, $conv:expr) => {
            match $d {
                MetricData::Gauge(g) => {
                    for p in g.data_points() {
                        out.push(($conv(p.value()), attrs(p.attributes())));
                    }
                }
                MetricData::Sum(s) => {
                    for p in s.data_points() {
                        out.push(($conv(p.value()), attrs(p.attributes())));
                    }
                }
                MetricData::Histogram(h) => {
                    for p in h.data_points() {
                        out.push((p.count() as f64, attrs(p.attributes())));
                    }
                }
                MetricData::ExponentialHistogram(h) => {
                    for p in h.data_points() {
                        out.push((p.count() as f64, attrs(p.attributes())));
                    }
                }
            }
        };
    }
    match data {
        AggregatedMetrics::F64(d) => points!(d, |v: f64| v),
        AggregatedMetrics::U64(d) => points!(d, |v: u64| v as f64),
        AggregatedMetrics::I64(d) => points!(d, |v: i64| v as f64),
    }
}
