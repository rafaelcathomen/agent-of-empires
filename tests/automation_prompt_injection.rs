// Skips when tmux is unavailable, matching the repo's other tmux-gated tests.
#[test]
fn terminal_session_receives_initial_prompt() {
    if std::process::Command::new("tmux")
        .arg("-V")
        .output()
        .is_err()
    {
        eprintln!("skipping: tmux not available");
        return;
    }
    use agent_of_empires::session::Instance;

    let mut inst = Instance::new("aoe_test_inject", "/tmp");
    inst.tool = "bash".into();
    inst.command = "bash".into();
    inst.initial_prompt = "echo AOE_INJECTED_MARKER".into();
    inst.start_with_size(Some((120, 40))).unwrap();
    inst.inject_initial_prompt().unwrap();

    // Give the pane a moment, then capture and assert the marker echoed.
    std::thread::sleep(std::time::Duration::from_millis(800));
    let session = agent_of_empires::tmux::Session::new(&inst.id, &inst.title).unwrap();
    let dump = session.capture_pane(50).unwrap_or_default();
    let _ = inst.stop();
    assert!(dump.contains("AOE_INJECTED_MARKER"), "pane was:\n{dump}");
}
