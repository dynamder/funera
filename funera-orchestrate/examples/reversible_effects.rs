//! Reversible effects — the LIFO teardown primitive.
//!
//! [`FuneraEnv::effect`] registers a side effect together with its inverse (a
//! [`Disposer`](funera_core::env::Disposer)). When the env is torn down via
//! [`FuneraEnv::dispose`], every disposer runs in **reverse registration order**
//! (LIFO), so later effects — which may depend on earlier ones — are undone
//! first. Disposal is idempotent and panic-isolated: one failing disposer never
//! blocks the rest.
//!
//! This is Funera's leak-safety primitive: any registration (a tool, a
//! listener, a connection) can be paired with its inverse, and a single
//! `dispose()` guarantees nothing is left behind. The `EnvActor` calls
//! `dispose()` automatically when the runtime is dropped, so effects registered
//! against a live env are always cleaned up.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use funera_core::env::FuneraEnv;

fn main() {
    // Simulated leak-prone resources owned by the caller.
    let connections: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let listeners = Arc::new(AtomicUsize::new(0));

    let (env, _watcher) = FuneraEnv::new(async_openai::Client::new(), "demo-model");

    // Effect 1: open a "connection"; its inverse closes it.
    env.effect({
        let connections = Arc::clone(&connections);
        move || {
            connections.lock().unwrap().push("db");
            println!("[setup] opened db connection");
            Box::new(move || {
                connections.lock().unwrap().retain(|c| *c != "db");
                println!("[dispose] closed db connection");
            })
        }
    });

    // Effect 2: subscribe a "listener"; its inverse unsubscribes.
    env.effect({
        let listeners = Arc::clone(&listeners);
        move || {
            listeners.fetch_add(1, Ordering::SeqCst);
            println!("[setup] subscribed listener");
            Box::new(move || {
                listeners.fetch_sub(1, Ordering::SeqCst);
                println!("[dispose] unsubscribed listener");
            })
        }
    });

    // Effect 3: depends on effect 2 — must be torn down BEFORE it (LIFO).
    env.effect(|| {
        println!("[setup] started consumer of listener");
        Box::new(|| println!("[dispose] stopped consumer (runs first: LIFO)"))
    });

    println!(
        "live: connections={} listeners={}",
        connections.lock().unwrap().len(),
        listeners.load(Ordering::SeqCst)
    );

    // Tear everything down in reverse order.
    env.dispose();

    assert!(connections.lock().unwrap().is_empty());
    assert_eq!(listeners.load(Ordering::SeqCst), 0);
    println!(
        "after dispose: connections={} listeners={} — no leaks",
        connections.lock().unwrap().len(),
        listeners.load(Ordering::SeqCst)
    );
}
