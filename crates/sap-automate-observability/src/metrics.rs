//! Lightweight Prometheus-compatible metrics registry.
//!
//! Designed for the SAP-Automate hot path: every tool call records a
//! latency observation in the `mcp_tool_latency_seconds` histogram and
//! bumps a counter, and the registry serialises to the Prometheus text
//! exposition format on `/metrics`.  Zero external dependencies — the
//! production install uses the `prometheus` crate; this module is the
//! abstraction we wire callers against so the swap is one-line.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::RwLock;

/// Standard SAP-Automate histogram buckets in seconds.  Chosen to
/// straddle the paper §X-D 80 ms gate at the 1-bucket-per-doubling
/// resolution typical of MCP-server SLOs.
pub const DEFAULT_BUCKETS_SECONDS: &[f64] = &[
    0.0005,   // 0.5 ms — below the typical RAG search
    0.001,    // 1 ms
    0.005,    // 5 ms
    0.010,    // 10 ms
    0.025,    // 25 ms
    0.050,    // 50 ms
    0.080,    // 80 ms — the paper §X-D acceptance gate
    0.100,    // 100 ms
    0.250,    // 250 ms
    0.500,    // 500 ms
    1.0,      // 1 s
    5.0,      // 5 s — long-running ADT scans
    30.0,     // 30 s
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MetricKind {
    Counter,
    Histogram,
    Gauge,
}

#[derive(Debug)]
struct MetricState {
    kind: MetricKind,
    /// Counter / Gauge value.
    scalar: f64,
    /// Histogram buckets `[upper_bound_seconds, ...]` cumulative.
    bucket_bounds: Vec<f64>,
    bucket_counts: Vec<u64>,
    sum: f64,
    count: u64,
}

#[derive(Debug, Default)]
pub struct MetricsRegistry {
    /// Keyed by `(name, sorted_labels_serialized)`.
    series: RwLock<BTreeMap<String, MetricState>>,
    /// `name -> kind + help` so the exposition output emits one TYPE / HELP per metric name.
    names: RwLock<BTreeMap<String, (MetricKind, String)>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Metric {
    pub name: String,
    pub kind: MetricKind,
    pub help: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistogramBucketSet {
    pub bounds: Vec<f64>,
    pub counts: Vec<u64>,
    pub sum: f64,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub metrics: Vec<Metric>,
    pub series: Vec<SeriesPoint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesPoint {
    pub name: String,
    pub labels: BTreeMap<String, String>,
    pub value: f64,
    pub histogram: Option<HistogramBucketSet>,
}

impl MetricsRegistry {
    pub fn new() -> Self { Self::default() }

    /// Idempotent: registering a name twice keeps the first kind/help.
    pub fn register(&self, name: &str, kind: MetricKind, help: &str) {
        let mut names = self.names.write().unwrap();
        names.entry(name.into())
            .or_insert_with(|| (kind, help.into()));
    }

    pub fn inc_counter(&self, name: &str, labels: &[(&str, &str)]) {
        self.add_counter(name, labels, 1.0);
    }

    pub fn add_counter(&self, name: &str, labels: &[(&str, &str)], by: f64) {
        let key = series_key(name, labels);
        let mut series = self.series.write().unwrap();
        let entry = series.entry(key).or_insert_with(|| MetricState {
            kind: MetricKind::Counter,
            scalar: 0.0, bucket_bounds: Vec::new(),
            bucket_counts: Vec::new(), sum: 0.0, count: 0,
        });
        entry.scalar += by;
    }

    pub fn set_gauge(&self, name: &str, labels: &[(&str, &str)], value: f64) {
        let key = series_key(name, labels);
        let mut series = self.series.write().unwrap();
        let entry = series.entry(key).or_insert_with(|| MetricState {
            kind: MetricKind::Gauge,
            scalar: 0.0, bucket_bounds: Vec::new(),
            bucket_counts: Vec::new(), sum: 0.0, count: 0,
        });
        entry.scalar = value;
    }

    /// Observe a duration into a histogram.  Uses the default buckets
    /// (see `DEFAULT_BUCKETS_SECONDS`).
    pub fn observe_histogram(&self, name: &str, labels: &[(&str, &str)], value_seconds: f64) {
        let key = series_key(name, labels);
        let mut series = self.series.write().unwrap();
        let entry = series.entry(key).or_insert_with(|| {
            let bounds: Vec<f64> = DEFAULT_BUCKETS_SECONDS.iter().copied().collect();
            let counts = vec![0u64; bounds.len() + 1]; // +1 for +Inf
            MetricState {
                kind: MetricKind::Histogram,
                scalar: 0.0, bucket_bounds: bounds, bucket_counts: counts,
                sum: 0.0, count: 0,
            }
        });
        entry.sum += value_seconds;
        entry.count += 1;
        let mut placed = false;
        for (i, b) in entry.bucket_bounds.iter().enumerate() {
            if value_seconds <= *b {
                // Histogram buckets in Prometheus are cumulative,
                // i.e. each bucket counts observations <= upper bound.
                for j in i..entry.bucket_counts.len() {
                    entry.bucket_counts[j] += 1;
                }
                placed = true;
                break;
            }
        }
        if !placed {
            // +Inf bucket
            let last = entry.bucket_counts.len() - 1;
            entry.bucket_counts[last] += 1;
        }
    }

    /// Render Prometheus text exposition format.
    pub fn render(&self) -> String {
        let names = self.names.read().unwrap();
        let series = self.series.read().unwrap();
        let mut out = String::new();
        // Group series by name for HELP / TYPE emission.
        let mut by_name: BTreeMap<String, Vec<(&str, &MetricState)>> = BTreeMap::new();
        for (key, state) in series.iter() {
            let name = key.split('{').next().unwrap_or(key);
            by_name.entry(name.into()).or_default().push((key, state));
        }
        for (name, entries) in by_name {
            if let Some((kind, help)) = names.get(&name) {
                out.push_str(&format!("# HELP {name} {help}\n"));
                let kind_str = match kind {
                    MetricKind::Counter => "counter",
                    MetricKind::Histogram => "histogram",
                    MetricKind::Gauge => "gauge",
                };
                out.push_str(&format!("# TYPE {name} {kind_str}\n"));
            }
            for (key, state) in entries {
                match state.kind {
                    MetricKind::Counter | MetricKind::Gauge => {
                        out.push_str(&format!("{key} {}\n", format_float(state.scalar)));
                    }
                    MetricKind::Histogram => {
                        // Emit bucket lines first, then _sum and _count.
                        let labels_prefix = if let Some(brace) = key.find('{') {
                            &key[brace+1..key.len()-1]
                        } else { "" };
                        let base = key.split('{').next().unwrap_or(key);
                        for (i, bound) in state.bucket_bounds.iter().enumerate() {
                            let comma = if labels_prefix.is_empty() { "" } else { "," };
                            out.push_str(&format!("{base}_bucket{{{labels_prefix}{comma}le=\"{}\"}} {}\n",
                                format_float(*bound), state.bucket_counts[i]));
                        }
                        let comma = if labels_prefix.is_empty() { "" } else { "," };
                        out.push_str(&format!("{base}_bucket{{{labels_prefix}{comma}le=\"+Inf\"}} {}\n",
                            state.bucket_counts.last().copied().unwrap_or(0)));
                        out.push_str(&format!("{base}_sum{{{labels_prefix}}} {}\n",
                            format_float(state.sum)));
                        out.push_str(&format!("{base}_count{{{labels_prefix}}} {}\n", state.count));
                    }
                }
            }
        }
        out
    }

    pub fn snapshot(&self) -> Snapshot {
        let names = self.names.read().unwrap();
        let series = self.series.read().unwrap();
        let metrics: Vec<Metric> = names.iter().map(|(n, (k, h))| Metric {
            name: n.clone(), kind: *k, help: h.clone(),
        }).collect();
        let points: Vec<SeriesPoint> = series.iter().map(|(key, state)| {
            let name = key.split('{').next().unwrap_or(key).to_string();
            let labels = parse_labels(key);
            let histogram = if state.kind == MetricKind::Histogram {
                Some(HistogramBucketSet {
                    bounds: state.bucket_bounds.clone(),
                    counts: state.bucket_counts.clone(),
                    sum: state.sum,
                    count: state.count,
                })
            } else { None };
            SeriesPoint { name, labels, value: state.scalar, histogram }
        }).collect();
        Snapshot { metrics, series: points }
    }
}

fn series_key(name: &str, labels: &[(&str, &str)]) -> String {
    if labels.is_empty() { return name.into(); }
    let mut sorted: Vec<(&&str, &&str)> = labels.iter().map(|(k, v)| (k, v)).collect();
    sorted.sort_by_key(|(k, _)| **k);
    let parts: Vec<String> = sorted.iter()
        .map(|(k, v)| format!("{k}=\"{}\"", escape(v)))
        .collect();
    format!("{name}{{{}}}", parts.join(","))
}

fn parse_labels(key: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(brace) = key.find('{') {
        let inner = &key[brace+1..key.len()-1];
        for pair in inner.split(',') {
            if let Some(eq) = pair.find('=') {
                let k = &pair[..eq];
                let v = pair[eq+1..].trim_matches('"');
                out.insert(k.into(), v.into());
            }
        }
    }
    out
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

fn format_float(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_increments_and_renders() {
        let r = MetricsRegistry::new();
        r.register("sap_rfc_calls_total", MetricKind::Counter, "Total RFC calls");
        r.inc_counter("sap_rfc_calls_total", &[("function", "BAPI_MATERIAL_GET_DETAIL")]);
        r.inc_counter("sap_rfc_calls_total", &[("function", "BAPI_MATERIAL_GET_DETAIL")]);
        r.inc_counter("sap_rfc_calls_total", &[("function", "RFC_READ_TABLE")]);
        let out = r.render();
        assert!(out.contains("# TYPE sap_rfc_calls_total counter"));
        assert!(out.contains("sap_rfc_calls_total{function=\"BAPI_MATERIAL_GET_DETAIL\"} 2"));
        assert!(out.contains("sap_rfc_calls_total{function=\"RFC_READ_TABLE\"} 1"));
    }

    #[test]
    fn histogram_records_buckets_correctly() {
        let r = MetricsRegistry::new();
        r.register("mcp_tool_latency_seconds", MetricKind::Histogram, "Tool latency");
        r.observe_histogram("mcp_tool_latency_seconds", &[("tool", "sap.docs.search")], 0.0003);
        r.observe_histogram("mcp_tool_latency_seconds", &[("tool", "sap.docs.search")], 0.07);
        r.observe_histogram("mcp_tool_latency_seconds", &[("tool", "sap.docs.search")], 0.2);
        let out = r.render();
        // 0.0003s ≤ 0.0005 → in 0.0005 bucket
        assert!(out.contains("le=\"0.0005\"} 1"));
        // 0.07s ≤ 0.08 (the paper §X-D gate) → 0.08 bucket gets +1
        assert!(out.contains("le=\"0.08\"} 2"));
        // 0.2s ≤ 0.25 → 0.25 bucket gets all 3
        assert!(out.contains("le=\"0.25\"} 3"));
        // sum should be 0.2703
        assert!(out.contains("_sum"));
        assert!(out.contains("_count"));
    }

    #[test]
    fn gauge_replaces_value() {
        let r = MetricsRegistry::new();
        r.register("sap_pool_in_use", MetricKind::Gauge, "Pool slots in use");
        r.set_gauge("sap_pool_in_use", &[], 5.0);
        r.set_gauge("sap_pool_in_use", &[], 3.0);
        let out = r.render();
        assert!(out.contains("sap_pool_in_use 3"));
        assert!(!out.contains("sap_pool_in_use 5"));
    }

    #[test]
    fn snapshot_returns_structured_data() {
        let r = MetricsRegistry::new();
        r.register("c", MetricKind::Counter, "h");
        r.inc_counter("c", &[("k", "v")]);
        let s = r.snapshot();
        assert_eq!(s.metrics.len(), 1);
        assert_eq!(s.series.len(), 1);
        assert_eq!(s.series[0].name, "c");
        assert_eq!(s.series[0].labels.get("k").map(|s| s.as_str()), Some("v"));
    }
}
