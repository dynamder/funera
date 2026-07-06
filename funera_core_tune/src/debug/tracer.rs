use std::sync::OnceLock;

static TRACER_INIT: OnceLock<()> = OnceLock::new();

pub fn init_tracing() {
    TRACER_INIT.get_or_init(|| {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
            )
            .with_test_writer()
            .init();
    });
}

pub fn init_tracing_with_filter(filter: &str) {
    TRACER_INIT.get_or_init(|| {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_test_writer()
            .init();
    });
}
