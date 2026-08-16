//! Service broker: multi-provider round-robin routing with plugin lifecycle.
//!
//! ```bash
//! cargo run --example service_broker
//! ```

use std::sync::Arc;

use funera_core::env::FuneraEnv;
use funera_core::loader::{Loader, PluginEntry};
use funera_core::plugin::{BrokerPlugin, BrokerProvider, PluginPhase, ServiceBroker};

#[tokio::main]
async fn main() {
    let (env, _watcher) = FuneraEnv::new(async_openai::Client::new(), "demo");
    let mut loader = Loader::new(env);

    let broker = ServiceBroker::<String>::new();
    let entries = vec![
        PluginEntry::new("broker", Arc::new(BrokerPlugin::new(broker.clone()))),
        PluginEntry::new(
            "provider-a",
            Arc::new(BrokerProvider::new(
                "a",
                Arc::<String>::new("provider-a".into()),
            )),
        ),
        PluginEntry::new(
            "provider-b",
            Arc::new(BrokerProvider::new(
                "b",
                Arc::<String>::new("provider-b".into()),
            )),
        ),
    ];

    let report = loader.reconcile(&entries).await;
    println!("loaded: {:?}", report.loaded);
    assert_eq!(loader.entry_phase("broker"), Some(PluginPhase::Active));
    assert_eq!(loader.entry_phase("provider-a"), Some(PluginPhase::Active));
    assert_eq!(loader.entry_phase("provider-b"), Some(PluginPhase::Active));
    assert_eq!(broker.len(), 2);

    let first = broker.next().unwrap();
    let second = broker.next().unwrap();
    let third = broker.next().unwrap();
    println!("routing: {first} -> {second} -> {third}");
    assert_ne!(first, second);
    assert_eq!(third, first);

    // Remove provider-b; it must unregister from the broker automatically.
    loader.reconcile(&entries[..2]).await;
    assert_eq!(loader.entry_phase("provider-b"), None);
    assert_eq!(broker.len(), 1);
    println!("after removing provider-b, broker.len() = {}", broker.len());

    println!("service_broker: all assertions passed");
}
