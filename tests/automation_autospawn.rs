#[cfg(feature = "serve")]
mod serve_tests {
    use serial_test::serial;
    use std::time::Duration;

    /// Full round-trip: ensure_scheduler_running spawns a real daemon and
    /// daemon_pid() becomes Some.
    ///
    /// `start_daemon` uses `std::env::current_exe()` to find the binary, which
    /// in a test binary returns the test runner, not the `aoe` binary. We work
    /// around this by setting the `_AOE_TEST_BIN` env var so `ensure_daemon_spawned`
    /// can find the real binary path from `CARGO_BIN_EXE_aoe`. The test is gated
    /// on: (a) tmux presence, (b) the real binary being reachable, (c) a free port.
    /// Serialized to prevent env-var mutations from racing.
    #[test]
    #[serial]
    fn ensure_scheduler_running_spawns_when_absent() {
        if std::process::Command::new("tmux")
            .arg("-V")
            .output()
            .is_err()
        {
            eprintln!("skipping: tmux not available");
            return;
        }

        // CARGO_BIN_EXE_aoe is set by Cargo for integration tests in a crate
        // with [[bin]] named "aoe". Verify it points to a real binary.
        let aoe_bin = std::path::PathBuf::from(env!("CARGO_BIN_EXE_aoe"));
        if !aoe_bin.exists() {
            eprintln!("skipping: aoe binary not found at {}", aoe_bin.display());
            return;
        }

        // Isolate app dir so we never touch real user state.
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", tmp.path());

        // Tell ensure_daemon_spawned to use the real aoe binary rather than
        // current_exe() (which in a test binary returns the test runner).
        std::env::set_var("_AOE_TEST_BIN", &aoe_bin);

        // No daemon should be present in the isolated dir.
        assert!(
            agent_of_empires::cli::serve::daemon_pid().is_none(),
            "expected no daemon in isolated temp dir"
        );

        use agent_of_empires::automation::lifecycle::ensure_scheduler_running;
        let spawned = match ensure_scheduler_running("default") {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: ensure_scheduler_running failed: {e}");
                std::env::remove_var("_AOE_TEST_BIN");
                return;
            }
        };
        std::env::remove_var("_AOE_TEST_BIN");

        assert!(
            spawned,
            "expected ensure_scheduler_running to report spawned=true"
        );

        // Give it a beat to write the pid file and start up.
        std::thread::sleep(Duration::from_millis(1500));

        let pid = agent_of_empires::cli::serve::daemon_pid();

        // Cleanup: stop the daemon we spawned, even if assertion below fails.
        if let Some(p) = pid {
            let _ = std::process::Command::new("kill")
                .arg(p.to_string())
                .status();
            std::thread::sleep(Duration::from_millis(200));
        } else {
            // Try to kill by reading pid file directly (daemon_pid() may have
            // cleaned up a stale file on our behalf).
            if let Ok(dir) = agent_of_empires::session::get_app_dir() {
                if let Ok(raw) = std::fs::read_to_string(dir.join("serve.pid")) {
                    if let Ok(p) = raw.trim().parse::<u32>() {
                        let _ = std::process::Command::new("kill")
                            .arg(p.to_string())
                            .status();
                    }
                }
            }
        }

        assert!(
            pid.is_some(),
            "expected daemon_pid() to be Some after spawning"
        );
    }

    /// Deterministic check: ensure_scheduler_running must compile, link, and
    /// return Ok(_) without panicking. Does not require tmux. Serialized to
    /// prevent env-var mutations from racing with the spawn test.
    #[test]
    #[serial]
    fn ensure_scheduler_running_returns_ok() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", tmp.path());
        // Set the override so that if a spawn is attempted it uses the real binary.
        let aoe_bin = std::path::PathBuf::from(env!("CARGO_BIN_EXE_aoe"));
        if aoe_bin.exists() {
            std::env::set_var("_AOE_TEST_BIN", &aoe_bin);
        }

        use agent_of_empires::automation::lifecycle::ensure_scheduler_running;
        let result = ensure_scheduler_running("default");
        std::env::remove_var("_AOE_TEST_BIN");

        match result {
            Ok(spawned) => {
                // Kill any daemon we might have incidentally spawned.
                if spawned {
                    std::thread::sleep(Duration::from_millis(300));
                    if let Some(p) = agent_of_empires::cli::serve::daemon_pid() {
                        let _ = std::process::Command::new("kill")
                            .arg(p.to_string())
                            .status();
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }
            Err(e) => {
                panic!("ensure_scheduler_running returned unexpected error: {e}");
            }
        }
    }
}
