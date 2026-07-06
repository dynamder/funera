use funera_core::chat::session::{FuneraSession, Idle};

fn idle_session() -> FuneraSession<Idle> {
    FuneraSession::new()
}

#[test]
fn session_new_creates_idle_session() {
    let session = idle_session();
    let context = session.session_context();
    assert!(context.is_empty());
}

#[test]
fn session_id_is_unique() {
    let s1 = FuneraSession::<Idle>::new();
    let s2 = FuneraSession::<Idle>::new();
    assert_ne!(s1.id(), s2.id());
}

#[test]
fn idle_to_running_transition() {
    let idle = idle_session();
    let id = idle.id();
    let running = idle.run();
    assert_eq!(running.id(), id);
}

#[test]
fn running_to_idle_transition() {
    let idle = FuneraSession::<Idle>::new();
    let running = idle.run();
    let _idle = running.idle();
}

#[test]
fn session_state_transitions_identity() {
    let idle = FuneraSession::<Idle>::new();
    let id_orig = idle.id();
    let running = idle.run();
    let idle2 = running.idle();
    assert_eq!(idle2.id(), id_orig);
}
