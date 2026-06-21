/// Query performance statistics and benchmarking output.
///
/// Collects latency samples from repeated query executions and prints a
/// summary table with min/avg/max/p50/p95/p99 latency, rows/sec throughput,
/// and result-set size in bytes.
use std::time::Duration;

/// Summary statistics over a set of query runs.
#[derive(Debug, Clone)]
pub struct BenchStats {
    /// Total number of runs.
    pub runs: u32,
    /// Total rows returned across all runs.
    pub total_rows: u64,
    /// Total result-set bytes across all runs.
    pub total_bytes: u64,
    /// Per-run durations (nanoseconds, sorted ascending after [`BenchStats::finish`]).
    samples: Vec<u64>,
}

impl BenchStats {
    /// Create a new collector for `runs` total iterations.
    pub fn new(runs: u32) -> Self {
        Self {
            runs,
            total_rows: 0,
            total_bytes: 0,
            samples: Vec::with_capacity(runs as usize),
        }
    }

    /// Record one completed run.
    pub fn record(&mut self, duration: Duration, rows: usize, result_bytes: usize) {
        self.samples.push(duration.as_nanos() as u64);
        self.total_rows += rows as u64;
        self.total_bytes += result_bytes as u64;
    }

    /// Sort samples and compute the printable statistics report.
    pub fn report(&mut self) -> String {
        if self.samples.is_empty() {
            return String::from("No samples collected.");
        }
        self.samples.sort_unstable();
        let n = self.samples.len() as u64;
        let total_ns: u64 = self.samples.iter().sum();
        let total_secs = total_ns as f64 / 1e9;

        let min_ms = self.samples[0] as f64 / 1e6;
        let max_ms = self.samples[n as usize - 1] as f64 / 1e6;
        let avg_ms = total_ns as f64 / 1e6 / n as f64;
        let p50_ms = percentile(&self.samples, 50) as f64 / 1e6;
        let p95_ms = percentile(&self.samples, 95) as f64 / 1e6;
        let p99_ms = percentile(&self.samples, 99) as f64 / 1e6;

        let rows_per_sec = self.total_rows as f64 / total_secs;
        let bytes_per_sec = self.total_bytes as f64 / total_secs;
        let result_kb = self.total_bytes as f64 / 1024.0;
        let mem_rss_kb = resident_set_kb();

        let mut out = String::new();
        out.push_str(&format!(
            "\n-- Benchmark: {} run{} -----------------------------\n",
            self.runs,
            if self.runs == 1 { "" } else { "s" }
        ));
        out.push_str(&format!(
            "  Latency (ms)    min={min_ms:.3}  avg={avg_ms:.3}  max={max_ms:.3}\n"
        ));
        out.push_str(&format!(
            "  Percentiles     p50={p50_ms:.3}  p95={p95_ms:.3}  p99={p99_ms:.3}\n"
        ));
        out.push_str(&format!("  Throughput      {rows_per_sec:.0} rows/sec\n"));
        out.push_str(&format!(
            "  Data xfer       {result_kb:.1} KB total  ({bytes_ps:.0} KB/sec)\n",
            bytes_ps = bytes_per_sec / 1024.0
        ));
        out.push_str(&format!("  Total rows      {}\n", self.total_rows));
        if mem_rss_kb > 0 {
            out.push_str(&format!("  Process RSS     {} KB\n", mem_rss_kb));
        }
        // Latency histogram — only show if there are enough samples.
        if self.samples.len() >= 2 {
            out.push_str(&build_histogram(&self.samples, 10));
        }
        out.push_str("------------------------------------------------\n");
        out
    }
}

/// Estimate result-set size in bytes for quick throughput calculation.
///
/// Counts UTF-8 bytes in all cell string representations.
pub fn estimate_result_bytes(output: &str) -> usize {
    output.len()
}

/// Build an ASCII latency histogram from sorted nanosecond samples.
///
/// Divides the range \[min, max\] into `buckets` equal-width bins and prints
/// a bar for each bin proportional to how many samples fall in it.
fn build_histogram(sorted_ns: &[u64], buckets: usize) -> String {
    let min = sorted_ns[0];
    let max = *sorted_ns.last().unwrap();
    if min == max {
        return String::new(); // All identical — nothing to show.
    }
    let bar_width = 30usize;
    let step = (max - min) / buckets as u64 + 1;
    let mut counts = vec![0usize; buckets];
    for &ns in sorted_ns {
        let idx = ((ns - min) / step) as usize;
        let idx = idx.min(buckets - 1);
        counts[idx] += 1;
    }
    let max_count = *counts.iter().max().unwrap_or(&1).max(&1);
    let mut out = String::from("  Latency histogram (ms):\n");
    for (i, &cnt) in counts.iter().enumerate() {
        let lo_ms = (min + i as u64 * step) as f64 / 1e6;
        let hi_ms = (min + (i + 1) as u64 * step) as f64 / 1e6;
        let filled = cnt * bar_width / max_count;
        let bar: String = "█".repeat(filled);
        out.push_str(&format!(
            "    [{lo_ms:>7.3} – {hi_ms:>7.3}] {bar:<width$} {cnt}\n",
            width = bar_width
        ));
    }
    out
}

/// Return the p-th percentile value from a sorted slice (nearest-rank method).
fn percentile(sorted: &[u64], p: u8) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let n = sorted.len();
    let rank = (p as usize * n).div_ceil(100);
    sorted[(rank.min(n)) - 1]
}

/// Read the process RSS in KB from `/proc/self/status` (Linux only).
///
/// Returns 0 on non-Linux platforms or if the file cannot be read.
fn resident_set_kb() -> u64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = std::fs::read_to_string("/proc/self/status") {
            for line in content.lines() {
                if let Some(rest) = line.strip_prefix("VmRSS:") {
                    let kb: u64 = rest
                        .split_whitespace()
                        .next()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    return kb;
                }
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_single_element() {
        assert_eq!(percentile(&[42], 50), 42);
        assert_eq!(percentile(&[42], 99), 42);
    }

    #[test]
    fn percentile_ordered() {
        let data: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&data, 50), 50);
        assert_eq!(percentile(&data, 95), 95);
        assert_eq!(percentile(&data, 99), 99);
    }

    #[test]
    fn bench_stats_report_contains_key_fields() {
        let mut stats = BenchStats::new(3);
        stats.record(Duration::from_millis(10), 100, 4096);
        stats.record(Duration::from_millis(12), 100, 4096);
        stats.record(Duration::from_millis(11), 100, 4096);
        let report = stats.report();
        assert!(report.contains("min="), "report: {report}");
        assert!(report.contains("p95="), "report: {report}");
        assert!(report.contains("rows/sec"), "report: {report}");
    }

    #[test]
    fn estimate_result_bytes_counts_chars() {
        assert_eq!(estimate_result_bytes("hello"), 5);
        assert_eq!(estimate_result_bytes(""), 0);
    }
}
