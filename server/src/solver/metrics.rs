// Ported from FlareSolverr v3.5.2 (src/metrics.py, src/bottle_plugins/prometheus_plugin.py), MIT.
//! FlareSolverr's Prometheus metrics: a count of `/v1` answers by site and result, and how long they took, served
//! on their own port (`PROMETHEUS_PORT`, 8192) when `PROMETHEUS_ENABLED` is `true`.
//!
//! The text is what Python's `prometheus_client` writes for the same two metrics (its process and Python metrics
//! are left out: Lantern isn't a Python program).

use axum::http::header;
use axum::response::IntoResponse;
use axum::Router;
use serde_json::Value;
use std::sync::Mutex;

/// The histogram's upper bounds, in seconds (FlareSolverr's `[0, 10, 25, 50]`, then `+Inf`).
pub const BUCKETS: [f64; 4] = [0.0, 10.0, 25.0, 50.0];

/// One counted (site, result) pair.
#[derive(Debug, Clone)]
struct Count {
    /// The site's host name (`unknown` or `None` as FlareSolverr writes them).
    domain: String,
    /// `solved`, `not_detected`, `error` or `unknown`.
    result: String,
    /// How many answers.
    value: f64,
    /// When the pair was first counted, in seconds since 1970.
    created: f64,
}

/// One timed site.
#[derive(Debug, Clone)]
struct Timing {
    /// The site's host name.
    domain: String,
    /// How many answers fell at or under each of [`BUCKETS`].
    buckets: [f64; 4],
    /// How many answers in all.
    count: f64,
    /// Their seconds added up.
    sum: f64,
    /// When the site was first timed, in seconds since 1970.
    created: f64,
}

/// The metrics, kept in memory while Lantern runs.
#[derive(Debug, Default)]
pub struct Metrics {
    /// `flaresolverr_request`, in the order the pairs were first seen.
    counts: Mutex<Vec<Count>>,
    /// `flaresolverr_request_duration`, in the order the sites were first seen.
    timings: Mutex<Vec<Timing>>,
}

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// The host name of `url` as Python's `urlparse(url).hostname` gives it (lower case), `None` when it has none.
fn hostname(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.trim_start_matches('[').trim_end_matches(']').to_lowercase()))
        .unwrap_or_else(|| "None".into())
}

/// A number the way `prometheus_client` writes it (Go's style: `1.0`, `+Inf`, `1.7e+09`).
pub fn go_float(d: f64) -> String {
    if d == f64::INFINITY {
        return "+Inf".into();
    }
    if d == f64::NEG_INFINITY {
        return "-Inf".into();
    }
    if d.is_nan() {
        return "NaN".into();
    }
    let s = format!("{d:?}");
    match s.find('.') {
        Some(dot) if d > 0.0 && dot > 6 => {
            let mantissa = format!("{}.{}{}", &s[..1], &s[1..dot], &s[dot + 1..]);
            format!("{}e+0{}", mantissa.trim_end_matches(['0', '.']), dot - 1)
        }
        _ => s,
    }
}

/// A label value with `\`, `"` and line breaks escaped.
fn label(v: &str) -> String {
    v.replace('\\', r"\\").replace('\n', r"\n").replace('"', "\\\"")
}

impl Metrics {
    /// Counts one `/v1` answer (`res`), for the request `req` (FlareSolverr's `export_metrics`). Answers without
    /// timestamps (errors before the command ran) aren't counted.
    pub fn record(&self, req: &Value, res: &Value) {
        let (Some(start), Some(end)) = (res.get("startTimestamp").and_then(Value::as_f64), res.get("endTimestamp").and_then(Value::as_f64))
        else {
            return;
        };
        let solved_url = res.pointer("/solution/url").and_then(Value::as_str).filter(|u| !u.is_empty());
        let domain = match solved_url {
            Some(u) => hostname(u),
            None => match req.get("url") {
                Some(Value::String(u)) if !u.is_empty() => hostname(u),
                _ => "unknown".into(),
            },
        };
        let message = res.get("message").and_then(Value::as_str).unwrap_or("");
        let result = match message {
            "Challenge solved!" => "solved",
            "Challenge not detected!" => "not_detected",
            m if m.starts_with("Error") => "error",
            _ => "unknown",
        };
        self.observe(&domain, result, (end - start) / 1000.0);
    }

    /// Counts one answer for `domain` with `result` that took `seconds`.
    pub fn observe(&self, domain: &str, result: &str, seconds: f64) {
        {
            let mut counts = self.counts.lock().unwrap_or_else(|e| e.into_inner());
            match counts.iter_mut().find(|c| c.domain == domain && c.result == result) {
                Some(c) => c.value += 1.0,
                None => counts.push(Count { domain: domain.into(), result: result.into(), value: 1.0, created: now() }),
            }
        }
        let mut timings = self.timings.lock().unwrap_or_else(|e| e.into_inner());
        let i = match timings.iter().position(|t| t.domain == domain) {
            Some(i) => i,
            None => {
                timings.push(Timing { domain: domain.into(), buckets: [0.0; 4], count: 0.0, sum: 0.0, created: now() });
                timings.len() - 1
            }
        };
        let t = &mut timings[i];
        for (b, bound) in t.buckets.iter_mut().zip(BUCKETS) {
            if seconds <= bound {
                *b += 1.0;
            }
        }
        t.count += 1.0;
        t.sum += seconds;
    }

    /// The metrics in Prometheus's text format.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let counts = self.counts.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let help = "Total requests with result";
        out.push_str(&format!("# HELP flaresolverr_request_total {help}\n# TYPE flaresolverr_request_total counter\n"));
        for c in &counts {
            out.push_str(&format!(
                "flaresolverr_request_total{{domain=\"{}\",result=\"{}\"}} {}\n",
                label(&c.domain),
                label(&c.result),
                go_float(c.value)
            ));
        }
        out.push_str(&format!("# HELP flaresolverr_request_created {help}\n# TYPE flaresolverr_request_created gauge\n"));
        for c in &counts {
            out.push_str(&format!(
                "flaresolverr_request_created{{domain=\"{}\",result=\"{}\"}} {}\n",
                label(&c.domain),
                label(&c.result),
                go_float(c.created)
            ));
        }
        let timings = self.timings.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let help = "Request duration in seconds";
        out.push_str(&format!("# HELP flaresolverr_request_duration {help}\n# TYPE flaresolverr_request_duration histogram\n"));
        for t in &timings {
            let d = label(&t.domain);
            for (b, bound) in t.buckets.iter().zip(BUCKETS) {
                out.push_str(&format!(
                    "flaresolverr_request_duration_bucket{{domain=\"{d}\",le=\"{}\"}} {}\n",
                    go_float(bound),
                    go_float(*b)
                ));
            }
            out.push_str(&format!("flaresolverr_request_duration_bucket{{domain=\"{d}\",le=\"+Inf\"}} {}\n", go_float(t.count)));
            out.push_str(&format!("flaresolverr_request_duration_count{{domain=\"{d}\"}} {}\n", go_float(t.count)));
            out.push_str(&format!("flaresolverr_request_duration_sum{{domain=\"{d}\"}} {}\n", go_float(t.sum)));
        }
        out.push_str(&format!("# HELP flaresolverr_request_duration_created {help}\n# TYPE flaresolverr_request_duration_created gauge\n"));
        for t in &timings {
            out.push_str(&format!("flaresolverr_request_duration_created{{domain=\"{}\"}} {}\n", label(&t.domain), go_float(t.created)));
        }
        out
    }
}

/// The metrics server's routes: the metrics on every path, as `prometheus_client`'s server answers.
pub fn router(metrics: std::sync::Arc<Metrics>) -> Router {
    Router::new().fallback(move || {
        let m = metrics.clone();
        async move { ([(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")], m.render()).into_response() }
    })
}

/// Serves the metrics on `port` (all addresses) in the background, as FlareSolverr does when `PROMETHEUS_ENABLED`.
pub fn serve(metrics: std::sync::Arc<Metrics>, port: u16) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
    rt.spawn(async move {
        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
        match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => {
                tracing::info!("Serving Prometheus exporter on http://0.0.0.0:{port}/metrics");
                let _ = axum::serve(l, router(metrics)).await;
            }
            Err(e) => tracing::error!(
                "The Prometheus metrics can't use port {port}: {e}. Set PROMETHEUS_PORT to a free port, or PROMETHEUS_ENABLED to false."
            ),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_are_written_like_prometheus_client() {
        assert_eq!(go_float(1.0), "1.0");
        assert_eq!(go_float(0.0), "0.0");
        assert_eq!(go_float(2.5), "2.5");
        assert_eq!(go_float(f64::INFINITY), "+Inf");
        assert_eq!(go_float(1730000000.125), "1.730000000125e+09");
        assert_eq!(go_float(1700000000.0), "1.7e+09");
    }

    #[test]
    fn answers_are_counted_by_site_and_result() {
        let m = Metrics::default();
        let ok = json!({ "status": "ok", "message": "Challenge solved!", "solution": { "url": "https://Site.Test/a" },
            "startTimestamp": 1000, "endTimestamp": 3500 });
        m.record(&json!({}), &ok);
        let err = json!({ "status": "error", "message": "Error: Error solving the challenge. Timeout after 0.01 seconds.",
            "startTimestamp": 1000, "endTimestamp": 60000 });
        m.record(&json!({ "url": "https://site.test/b" }), &err);
        m.record(&json!({}), &json!({ "status": "ok", "message": "", "sessions": [], "startTimestamp": 1, "endTimestamp": 1 }));
        m.record(&json!({}), &json!({ "msg": "FlareSolverr is ready!" }));
        let text = m.render();
        assert!(text.contains("# TYPE flaresolverr_request_total counter\n"), "{text}");
        assert!(text.contains("flaresolverr_request_total{domain=\"site.test\",result=\"solved\"} 1.0\n"), "{text}");
        assert!(text.contains("flaresolverr_request_total{domain=\"site.test\",result=\"error\"} 1.0\n"), "{text}");
        assert!(text.contains("flaresolverr_request_total{domain=\"unknown\",result=\"unknown\"} 1.0\n"), "{text}");
        assert!(text.contains("flaresolverr_request_duration_bucket{domain=\"site.test\",le=\"10.0\"} 1.0\n"), "{text}");
        assert!(text.contains("flaresolverr_request_duration_bucket{domain=\"site.test\",le=\"+Inf\"} 2.0\n"), "{text}");
        assert!(text.contains("flaresolverr_request_duration_count{domain=\"site.test\"} 2.0\n"), "{text}");
        assert!(text.contains("flaresolverr_request_duration_sum{domain=\"site.test\"} 61.5\n"), "{text}");
        assert!(text.contains("# TYPE flaresolverr_request_duration_created gauge\n"), "{text}");
        assert_eq!(text.matches("flaresolverr_request_total{").count(), 3, "the index answer isn't counted");
    }
}
