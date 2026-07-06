use std::fmt::Write;

use crate::analysis::metrics::TestMetrics;

pub fn format_report(metrics: &TestMetrics) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "=== Test Report: {} ===", metrics.test_name);
    let _ = writeln!(out, "  Duration:       {:?}", metrics.duration);
    let _ = writeln!(
        out,
        "  Assertions:     {} passed, {} failed",
        metrics.assertions_passed, metrics.assertions_failed
    );
    for (key, value) in &metrics.extra {
        let _ = writeln!(out, "  {}: {}", key, value);
    }
    out
}
