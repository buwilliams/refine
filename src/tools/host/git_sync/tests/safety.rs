use super::*;

#[test]
fn durable_state_ignores_transient_lock_temp_and_copy_files() {
    let root = unique_temp_dir("transient-state");
    let sessions = root.join("chat/sessions");
    fs::create_dir_all(&sessions).unwrap();
    fs::write(sessions.join("session.json"), "{}\n").unwrap();
    fs::write(sessions.join(".session.lock"), "").unwrap();
    fs::write(sessions.join("session.json.interrupted.tmp"), "partial\n").unwrap();
    fs::write(sessions.join(".refine-sync-123-0"), "partial\n").unwrap();
    fs::write(root.join("supervisor-agent.lock"), "").unwrap();

    let state = durable_state_map(&root).unwrap();

    assert_eq!(
        state.keys().cloned().collect::<Vec<_>>(),
        vec![PathBuf::from("chat/sessions/session.json")]
    );
    fs::remove_dir_all(root).unwrap();
}
