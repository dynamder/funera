use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default)]
pub struct TestMetrics {
    pub test_name: String,
    pub duration: Duration,
    pub assertions_passed: usize,
    pub assertions_failed: usize,
    pub extra: Vec<(String, String)>,
}

impl TestMetrics {
    pub fn new(test_name: impl Into<String>) -> Self {
        Self {
            test_name: test_name.into(),
            ..Default::default()
        }
    }

    pub fn record_assertion(&mut self, passed: bool) {
        if passed {
            self.assertions_passed += 1;
        } else {
            self.assertions_failed += 1;
        }
    }

    pub fn record(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.extra.push((key.into(), value.into()));
    }
}

pub struct TestTimer {
    start: Instant,
}

impl TestTimer {
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}
