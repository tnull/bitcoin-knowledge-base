use std::collections::{HashMap, VecDeque};
use std::fmt::Write as FmtWrite;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::repo_cache::dir_size;

/// Statistics for a single job's most recent run.
#[derive(Debug, Clone)]
pub struct JobRunStats {
	pub last_duration: Duration,
	pub last_docs: u32,
	pub base_interval: Duration,
	pub last_completed: Instant,
	pub last_error: Option<String>,
}

/// Shared metrics collector for the BKB server.
///
/// Tracks request counts (sliding window of per-minute buckets), job run
/// statistics, and provides lazy-cached cache-directory size computation.
pub struct Metrics {
	request_minutes: Mutex<VecDeque<(u64, u64)>>,
	job_stats: Mutex<HashMap<String, JobRunStats>>,
	db_path: PathBuf,
	cache_dir: Option<PathBuf>,
	max_cache_bytes: Option<u64>,
	cache_size_cached: Mutex<(Instant, u64)>,
}

impl Metrics {
	pub fn new(db_path: PathBuf, cache_dir: Option<PathBuf>, max_cache_bytes: Option<u64>) -> Self {
		Self {
			request_minutes: Mutex::new(VecDeque::new()),
			job_stats: Mutex::new(HashMap::new()),
			db_path,
			cache_dir,
			max_cache_bytes,
			cache_size_cached: Mutex::new((Instant::now() - Duration::from_secs(600), 0)),
		}
	}

	/// Increment the request counter for the current minute.
	pub fn record_request(&self) {
		let minute_epoch = current_minute_epoch();
		let mut minutes = self.request_minutes.lock().unwrap();
		if let Some(last) = minutes.back_mut() {
			if last.0 == minute_epoch {
				last.1 += 1;
				return;
			}
		}
		minutes.push_back((minute_epoch, 1));
		// Prune entries older than 7 days (10080 minutes)
		let cutoff = minute_epoch.saturating_sub(10080);
		while minutes.front().is_some_and(|(m, _)| *m < cutoff) {
			minutes.pop_front();
		}
	}

	/// Total requests in the last 24 hours.
	pub fn requests_last_day(&self) -> u64 {
		let cutoff = current_minute_epoch().saturating_sub(1440);
		let minutes = self.request_minutes.lock().unwrap();
		minutes.iter().filter(|(m, _)| *m >= cutoff).map(|(_, c)| c).sum()
	}

	/// Total requests in the last 7 days.
	pub fn requests_last_week(&self) -> u64 {
		let cutoff = current_minute_epoch().saturating_sub(10080);
		let minutes = self.request_minutes.lock().unwrap();
		minutes.iter().filter(|(m, _)| *m >= cutoff).map(|(_, c)| c).sum()
	}

	/// Record the result of a job run.
	pub fn record_job_run(
		&self, source_id: &str, duration: Duration, docs: u32, base_interval: Duration,
		error: Option<String>,
	) {
		let mut stats = self.job_stats.lock().unwrap();
		stats.insert(
			source_id.to_string(),
			JobRunStats {
				last_duration: duration,
				last_docs: docs,
				base_interval,
				last_completed: Instant::now(),
				last_error: error,
			},
		);
	}

	/// Snapshot of all job stats (sorted by source_id).
	pub fn job_stats_snapshot(&self) -> Vec<(String, JobRunStats)> {
		let stats = self.job_stats.lock().unwrap();
		let mut entries: Vec<_> = stats.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
		entries.sort_by(|a, b| a.0.cmp(&b.0));
		entries
	}

	/// Size of the SQLite database file in bytes.
	pub fn db_size_bytes(&self) -> u64 {
		std::fs::metadata(&self.db_path).map(|m| m.len()).unwrap_or(0)
	}

	/// Cache usage: `(used_bytes, max_bytes, ratio)`.
	///
	/// The directory walk is cached with a 5-minute TTL.
	pub fn cache_size_bytes(&self) -> (u64, Option<u64>, Option<f64>) {
		let cache_dir = match self.cache_dir {
			Some(ref d) => d,
			None => return (0, None, None),
		};

		let mut cached = self.cache_size_cached.lock().unwrap();
		if cached.0.elapsed() > Duration::from_secs(300) {
			cached.1 = dir_size(cache_dir);
			cached.0 = Instant::now();
		}

		let used = cached.1;
		let max = self.max_cache_bytes;
		let ratio = max.map(|m| if m == 0 { 0.0 } else { used as f64 / m as f64 });
		(used, max, ratio)
	}

	/// Render all metrics in Prometheus text exposition format.
	pub fn render_prometheus(&self, doc_stats: &[(String, i64)]) -> String {
		let mut out = String::with_capacity(2048);

		// Request gauges
		let _ =
			writeln!(out, "# HELP bkb_requests_last_24h Total HTTP requests in the last 24 hours.");
		let _ = writeln!(out, "# TYPE bkb_requests_last_24h gauge");
		let _ = writeln!(out, "bkb_requests_last_24h {}", self.requests_last_day());
		let _ =
			writeln!(out, "# HELP bkb_requests_last_7d Total HTTP requests in the last 7 days.");
		let _ = writeln!(out, "# TYPE bkb_requests_last_7d gauge");
		let _ = writeln!(out, "bkb_requests_last_7d {}", self.requests_last_week());

		// Health
		let _ = writeln!(out, "# HELP bkb_health_status Whether the server is healthy (1=ok).");
		let _ = writeln!(out, "# TYPE bkb_health_status gauge");
		let _ = writeln!(out, "bkb_health_status 1");

		// Documents by source type
		let _ =
			writeln!(out, "# HELP bkb_documents_total Number of indexed documents by source type.");
		let _ = writeln!(out, "# TYPE bkb_documents_total gauge");
		for (source_type, count) in doc_stats {
			let _ = writeln!(
				out,
				"bkb_documents_total{{source_type=\"{}\"}} {}",
				prom_escape(source_type),
				count
			);
		}

		// DB size
		let _ = writeln!(out, "# HELP bkb_db_size_bytes SQLite database file size in bytes.");
		let _ = writeln!(out, "# TYPE bkb_db_size_bytes gauge");
		let _ = writeln!(out, "bkb_db_size_bytes {}", self.db_size_bytes());

		// Cache size
		let (cache_used, cache_max, cache_ratio) = self.cache_size_bytes();
		let _ =
			writeln!(out, "# HELP bkb_cache_used_bytes Bytes used by the repo cache directory.");
		let _ = writeln!(out, "# TYPE bkb_cache_used_bytes gauge");
		let _ = writeln!(out, "bkb_cache_used_bytes {}", cache_used);

		if let Some(max) = cache_max {
			let _ = writeln!(
				out,
				"# HELP bkb_cache_max_bytes Maximum configured repo cache size in bytes."
			);
			let _ = writeln!(out, "# TYPE bkb_cache_max_bytes gauge");
			let _ = writeln!(out, "bkb_cache_max_bytes {}", max);
		}

		if let Some(ratio) = cache_ratio {
			let _ = writeln!(
				out,
				"# HELP bkb_cache_used_ratio Fraction of cache capacity used (0.0-1.0)."
			);
			let _ = writeln!(out, "# TYPE bkb_cache_used_ratio gauge");
			let _ = writeln!(out, "bkb_cache_used_ratio {:.4}", ratio);
		}

		// Job stats
		let jobs = self.job_stats_snapshot();
		if !jobs.is_empty() {
			let _ = writeln!(
				out,
				"# HELP bkb_job_last_duration_seconds Duration of the last run for each source."
			);
			let _ = writeln!(out, "# TYPE bkb_job_last_duration_seconds gauge");
			let mut total_duration = 0.0_f64;
			for (source_id, stats) in &jobs {
				let secs = stats.last_duration.as_secs_f64();
				let _ = writeln!(
					out,
					"bkb_job_last_duration_seconds{{source=\"{}\"}} {:.3}",
					prom_escape(source_id),
					secs
				);
				total_duration += secs;
			}

			let _ = writeln!(
				out,
				"# HELP bkb_job_backlog_ratio Ratio of last duration to base interval (>1 means backlog)."
			);
			let _ = writeln!(out, "# TYPE bkb_job_backlog_ratio gauge");
			for (source_id, stats) in &jobs {
				let ratio = if stats.base_interval.as_secs_f64() > 0.0 {
					stats.last_duration.as_secs_f64() / stats.base_interval.as_secs_f64()
				} else {
					0.0
				};
				let _ = writeln!(
					out,
					"bkb_job_backlog_ratio{{source=\"{}\"}} {:.4}",
					prom_escape(source_id),
					ratio
				);
			}

			let _ = writeln!(
				out,
				"# HELP bkb_job_total_duration_seconds Sum of all last job durations."
			);
			let _ = writeln!(out, "# TYPE bkb_job_total_duration_seconds gauge");
			let _ = writeln!(out, "bkb_job_total_duration_seconds {:.3}", total_duration);
		}

		out
	}

	/// Render a lightweight HTML admin dashboard.
	pub fn render_dashboard_html(&self, doc_stats: &[(String, i64)]) -> String {
		let total_docs: i64 = doc_stats.iter().map(|(_, c)| c).sum();
		let db_size = self.db_size_bytes();
		let (cache_used, cache_max, cache_ratio) = self.cache_size_bytes();
		let req_day = self.requests_last_day();
		let req_week = self.requests_last_week();
		let jobs = self.job_stats_snapshot();

		let mut doc_rows = String::new();
		for (source_type, count) in doc_stats {
			let _ = write!(
				doc_rows,
				"<tr><td>{}</td><td class=\"num\">{}</td></tr>",
				html_escape(source_type),
				count
			);
		}

		let mut job_rows = String::new();
		for (source_id, stats) in &jobs {
			let backlog = if stats.base_interval.as_secs_f64() > 0.0 {
				stats.last_duration.as_secs_f64() / stats.base_interval.as_secs_f64()
			} else {
				0.0
			};
			let status = match &stats.last_error {
				Some(e) => format!("<span class=\"err\">{}</span>", html_escape(e)),
				None => "ok".to_string(),
			};
			let _ = write!(
				job_rows,
				"<tr><td>{}</td><td class=\"num\">{:.1}s</td><td class=\"num\">{}</td>\
				 <td class=\"num\">{:.3}</td><td>{}</td></tr>",
				html_escape(source_id),
				stats.last_duration.as_secs_f64(),
				stats.last_docs,
				backlog,
				status,
			);
		}

		let cache_info = match (cache_max, cache_ratio) {
			(Some(max), Some(ratio)) => format!(
				"{} / {} ({:.1}%)",
				format_bytes(cache_used),
				format_bytes(max),
				ratio * 100.0
			),
			_ => {
				if cache_used > 0 {
					format_bytes(cache_used)
				} else {
					"N/A".to_string()
				}
			},
		};

		format!(
			r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="refresh" content="30">
<title>BKB Admin Dashboard</title>
<style>
:root {{
	--bg: #fafafa; --fg: #1a1a1a; --muted: #666; --border: #ddd;
	--card-bg: #fff; --accent: #f7931a; --accent2: #4a90d9;
	--table-stripe: #f5f5f5;
}}
@media (prefers-color-scheme: dark) {{
	:root {{
		--bg: #1a1a2e; --fg: #e0e0e0; --muted: #999; --border: #333;
		--card-bg: #16213e; --accent: #f7931a; --accent2: #6db3f2;
		--table-stripe: #1e2a45;
	}}
}}
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{
	font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
	background: var(--bg); color: var(--fg); line-height: 1.6;
	max-width: 960px; margin: 0 auto; padding: 2rem 1rem;
}}
h1 {{ color: var(--accent); margin-bottom: 1.5rem; font-size: 1.5rem; }}
h2 {{ color: var(--accent2); margin: 1.5rem 0 0.5rem; font-size: 1.1rem; }}
.cards {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); gap: 1rem; margin-bottom: 1rem; }}
.card {{
	background: var(--card-bg); border: 1px solid var(--border); border-radius: 8px;
	padding: 1rem; text-align: center;
}}
.card .label {{ font-size: 0.8rem; color: var(--muted); text-transform: uppercase; }}
.card .value {{ font-size: 1.5rem; font-weight: 700; }}
table {{ width: 100%; border-collapse: collapse; background: var(--card-bg); border: 1px solid var(--border); border-radius: 8px; overflow: hidden; }}
th {{ text-align: left; padding: 0.5rem 0.75rem; background: var(--table-stripe); font-size: 0.8rem; color: var(--muted); text-transform: uppercase; }}
td {{ padding: 0.5rem 0.75rem; border-top: 1px solid var(--border); font-size: 0.9rem; }}
.num {{ text-align: right; font-variant-numeric: tabular-nums; }}
.err {{ color: #e74c3c; font-size: 0.8rem; }}
footer {{ margin-top: 2rem; text-align: center; font-size: 0.8rem; color: var(--muted); }}
</style>
</head>
<body>
<h1>BKB Admin Dashboard</h1>

<div class="cards">
<div class="card"><div class="label">Documents</div><div class="value">{total_docs}</div></div>
<div class="card"><div class="label">Requests (24h)</div><div class="value">{req_day}</div></div>
<div class="card"><div class="label">Requests (7d)</div><div class="value">{req_week}</div></div>
<div class="card"><div class="label">DB Size</div><div class="value">{db_size}</div></div>
<div class="card"><div class="label">Cache</div><div class="value">{cache_info}</div></div>
</div>

<h2>Documents by Source</h2>
<table>
<tr><th>Source Type</th><th class="num">Count</th></tr>
{doc_rows}
</table>

<h2>Job Status</h2>
<table>
<tr><th>Source</th><th class="num">Duration</th><th class="num">Docs</th><th class="num">Backlog</th><th>Status</th></tr>
{job_rows}
</table>

<footer>Auto-refreshes every 30 seconds &middot; <a href="/metrics" style="color:var(--accent2)">Prometheus metrics</a></footer>
</body>
</html>
"##,
			total_docs = total_docs,
			req_day = req_day,
			req_week = req_week,
			db_size = format_bytes(db_size),
			cache_info = cache_info,
			doc_rows = doc_rows,
			job_rows = if job_rows.is_empty() {
				"<tr><td colspan=\"5\" style=\"color:var(--muted);text-align:center\">No job data yet</td></tr>"
					.to_string()
			} else {
				job_rows
			},
		)
	}
}

/// Current minute since Unix epoch.
fn current_minute_epoch() -> u64 {
	SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() / 60
}

/// Escape a label value for Prometheus text format.
fn prom_escape(s: &str) -> String {
	s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

/// Minimal HTML escaping.
fn html_escape(s: &str) -> String {
	s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Human-readable byte sizes.
fn format_bytes(bytes: u64) -> String {
	const KB: u64 = 1024;
	const MB: u64 = 1024 * KB;
	const GB: u64 = 1024 * MB;
	if bytes >= GB {
		format!("{:.1} GB", bytes as f64 / GB as f64)
	} else if bytes >= MB {
		format!("{:.1} MB", bytes as f64 / MB as f64)
	} else if bytes >= KB {
		format!("{:.1} KB", bytes as f64 / KB as f64)
	} else {
		format!("{} B", bytes)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_record_and_count_requests() {
		let metrics = Metrics::new(PathBuf::from("/tmp/test.db"), None, None);
		assert_eq!(metrics.requests_last_day(), 0);
		metrics.record_request();
		metrics.record_request();
		assert_eq!(metrics.requests_last_day(), 2);
		assert_eq!(metrics.requests_last_week(), 2);
	}

	#[test]
	fn test_record_job_run() {
		let metrics = Metrics::new(PathBuf::from("/tmp/test.db"), None, None);
		metrics.record_job_run(
			"test:source",
			Duration::from_secs(5),
			42,
			Duration::from_secs(3600),
			None,
		);
		let snapshot = metrics.job_stats_snapshot();
		assert_eq!(snapshot.len(), 1);
		assert_eq!(snapshot[0].0, "test:source");
		assert_eq!(snapshot[0].1.last_docs, 42);
		assert!(snapshot[0].1.last_error.is_none());
	}

	#[test]
	fn test_render_prometheus_basic() {
		let metrics = Metrics::new(PathBuf::from("/tmp/test.db"), None, None);
		metrics.record_request();
		let doc_stats = vec![("bip".to_string(), 10i64), ("bolt".to_string(), 5)];
		let output = metrics.render_prometheus(&doc_stats);
		assert!(output.contains("bkb_requests_last_24h 1"));
		assert!(output.contains("bkb_documents_total{source_type=\"bip\"} 10"));
		assert!(output.contains("bkb_documents_total{source_type=\"bolt\"} 5"));
		assert!(output.contains("bkb_health_status 1"));
	}

	#[test]
	fn test_render_dashboard_html() {
		let metrics = Metrics::new(PathBuf::from("/tmp/test.db"), None, None);
		let doc_stats = vec![("bip".to_string(), 10i64)];
		let html = metrics.render_dashboard_html(&doc_stats);
		assert!(html.contains("BKB Admin Dashboard"));
		assert!(html.contains("bip"));
	}

	#[test]
	fn test_format_bytes() {
		assert_eq!(format_bytes(500), "500 B");
		assert_eq!(format_bytes(1536), "1.5 KB");
		assert_eq!(format_bytes(10 * 1024 * 1024), "10.0 MB");
		assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.0 GB");
	}

	#[test]
	fn test_prom_escape() {
		assert_eq!(prom_escape("hello"), "hello");
		assert_eq!(prom_escape("a\"b"), "a\\\"b");
		assert_eq!(prom_escape("a\\b"), "a\\\\b");
	}
}
