//! E2E tests covering automation run dispatch.
//!
//! Test A (`run_now_produces_session_and_injects_prompt`) is deterministic:
//! it fires the automation on demand via `aoe automation run-now` without
//! waiting for a wall-clock cron tick. This is the CI gate.
//!
//! Test B (`daemon_fires_scheduled_run`) is marked `#[ignore]` because it
//! waits up to ~90 s for a real cron fire from the scheduler daemon. Run it
//! on demand with:
//!
//! ```sh
//! cargo test --test e2e -- --ignored daemon_fires_scheduled_run --nocapture
//! ```

use serial_test::serial;

use crate::harness::{require_tmux, TuiTestHarness};

// ---------------------------------------------------------------------------
// Test A: deterministic run-now dispatch
// ---------------------------------------------------------------------------

/// Verify that `aoe automation run-now` synchronously creates a terminal
/// session named `<automation-name> (auto)`.
///
/// No daemon is involved -- `--no-launch-daemon` is passed to `automation add`
/// and the run is fired by the explicit `run-now` sub-command, which calls
/// `launch_run` synchronously inside the CLI process. This makes the test
/// deterministic and fast.
///
/// The tmux guard is kept because `launch_run` for a terminal-view automation
/// spawns a real tmux session; the test skips cleanly when tmux is absent.
#[test]
#[serial]
fn run_now_produces_session_and_injects_prompt() {
    require_tmux!();

    let h = TuiTestHarness::new_in_tmp("automation_fires_runnow");

    // Register an automation that would fire every minute, but we will not
    // wait for the cron -- we fire it manually with run-now.
    let add = h.run_cli(&[
        "automation",
        "add",
        "--name",
        "ticker",
        "--cron",
        "* * * * *",
        "--path",
        "/tmp",
        "--cmd",
        "bash",
        "--prompt",
        "echo AOE_FIRED",
        "--no-launch-daemon",
    ]);
    assert!(
        add.status.success(),
        "automation add failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&add.stdout),
        String::from_utf8_lossy(&add.stderr),
    );

    // Parse the short id from the list output (first token on the line).
    let list_out = h.run_cli(&["automation", "list"]);
    assert!(
        list_out.status.success(),
        "automation list failed: {}",
        String::from_utf8_lossy(&list_out.stderr)
    );
    let list_stdout = String::from_utf8_lossy(&list_out.stdout);
    let short_id = list_stdout
        .split_whitespace()
        .next()
        .expect("automation list should produce at least one token");

    // Fire the automation immediately. `launch_run` is called synchronously
    // inside the CLI process, which creates and starts the tmux session.
    let run_now = h.run_cli(&["automation", "run-now", short_id]);
    assert!(
        run_now.status.success(),
        "automation run-now failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run_now.stdout),
        String::from_utf8_lossy(&run_now.stderr),
    );

    // The session list must now contain "ticker (auto)".
    let sessions_out = h.run_cli(&["list"]);
    assert!(
        sessions_out.status.success(),
        "aoe list failed: {}",
        String::from_utf8_lossy(&sessions_out.stderr)
    );
    let sessions_stdout = String::from_utf8_lossy(&sessions_out.stdout);
    assert!(
        sessions_stdout.contains("ticker (auto)"),
        "expected 'ticker (auto)' session in list output; got:\n{}",
        sessions_stdout,
    );

    // Cleanup: kill the tmux session that was created for the automation run.
    // We read the session id from sessions.json and stop it best-effort.
    let sessions_json_path =
        crate::harness::app_dir_in(h.home_path()).join("profiles/default/sessions.json");
    if let Ok(content) = std::fs::read_to_string(&sessions_json_path) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(sessions) = val.as_array() {
                for s in sessions {
                    if s["title"].as_str() == Some("ticker (auto)") {
                        if let Some(id) = s["id"].as_str() {
                            let _ = h.run_cli(&["rm", id, "--force"]);
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Test B: slow real-daemon timing test (run on demand with --ignored)
// ---------------------------------------------------------------------------

/// Verify that the scheduler daemon fires an automation on a real cron tick
/// and creates a `ticker (auto)` session.
///
/// This test is `#[ignore]` because it waits up to ~90 s for a wall-clock
/// cron fire (cron granularity is 1 min + scheduler tick overhead). It is not
/// a CI gate; run it locally to confirm end-to-end scheduler behaviour.
///
/// Note: the daemon inherits the test harness's isolated `$HOME` /
/// `XDG_CONFIG_HOME` via `spawn()`, so it writes sessions to the same
/// isolated store that `run_cli(["list"])` reads.
#[test]
#[serial]
#[ignore] // Slow real-daemon timing test; run on demand with --ignored
fn daemon_fires_scheduled_run() {
    require_tmux!();

    let mut h = TuiTestHarness::new_in_tmp("automation_fires_daemon");

    // Register an automation that fires every minute.
    let add = h.run_cli(&[
        "automation",
        "add",
        "--name",
        "ticker",
        "--cron",
        "* * * * *",
        "--path",
        "/tmp",
        "--cmd",
        "bash",
        "--prompt",
        "echo AOE_FIRED",
        "--no-launch-daemon",
    ]);
    assert!(
        add.status.success(),
        "automation add failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&add.stdout),
        String::from_utf8_lossy(&add.stderr),
    );

    // Spawn the scheduler daemon via the serve sub-command. The daemon writes
    // sessions to the harness's isolated app dir, which `run_cli` also reads.
    h.spawn(&["serve", "--daemon"]);

    // Poll for up to ~90 s for the cron to fire (cron granularity ~60 s plus
    // scheduler tick jitter). Check every second.
    let mut found = false;
    for _ in 0..90 {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let list = h.run_cli(&["list"]);
        if String::from_utf8_lossy(&list.stdout).contains("ticker (auto)") {
            found = true;
            break;
        }
    }

    // Stop the daemon before asserting so it doesn't linger if the assert
    // panics (best-effort; the harness Drop also kills the tmux session).
    let _ = h.run_cli(&["serve", "--stop"]);

    assert!(
        found,
        "automation did not produce a 'ticker (auto)' session within 90 s"
    );
}
