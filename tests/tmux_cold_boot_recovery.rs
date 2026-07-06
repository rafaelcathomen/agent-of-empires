//! Regression coverage for the cold-boot startup-recovery bug: a machine
//! reboot leaves no tmux server at all (not even a stale socket), and
//! `batch_pane_metadata()` must report "zero panes" (`Ok`) rather than the
//! ambiguous-failure `Err` its callers (daemon and TUI startup recovery)
//! treat as "skip this pass, don't touch anything" -- which previously
//! meant a cold boot silently skipped relaunching every session.
//!
//! This lives in its own top-level `tests/*.rs` file (its own process,
//! auto-discovered by Cargo as a separate test binary) rather than inside
//! `tests/integration/main.rs`'s consolidated binary. `tmux_socket_path()`
//! caches its resolution in a process-wide `OnceLock` on first call, so
//! sharing a process with any other tmux-touching test would make the
//! `AOE_TMUX_SOCKET` override below a race against whichever test happens
//! to call into `tmux::` first.

use std::path::PathBuf;

/// A path guaranteed to have no tmux server listening on it: this test
/// process never starts tmux against it, and the pid-suffixed name means no
/// other process on the machine has used it either.
fn fresh_nonexistent_socket_path() -> PathBuf {
    std::env::temp_dir().join(format!("aoe-cold-boot-test-{}.sock", std::process::id()))
}

#[test]
fn batch_pane_metadata_returns_empty_ok_when_no_server_is_reachable() {
    std::env::set_var("AOE_TMUX_SOCKET", fresh_nonexistent_socket_path());

    let result = agent_of_empires::tmux::batch_pane_metadata();

    assert!(
        result.is_ok(),
        "a genuinely absent tmux server must be Ok(empty), not Err (which callers \
         treat as an ambiguous glitch and skip startup recovery entirely): {result:?}"
    );
    assert!(
        result.unwrap().is_empty(),
        "no server reachable means zero panes exist"
    );
}
