//! End-to-end: two verified Claude sessions sharing one project cwd each
//! resume their OWN session id on restart, with zero cross-assignment, even
//! when the peer's transcript is the newest in the shared folder.
//!
//! This is the marquee guarantee of the durable session-resume feature (plan
//! §5 "C3 shared-cwd" / "C1 reboot"): a `session_id_verified` row resumes its
//! stored id directly and never re-derives it via the shared-cwd freshest-mtime
//! scan, so distinct sessions in one directory can never swap conversations.
//!
//! Modelled on `resume_fallback.rs`: a fake `claude` shim records the flags it
//! is launched with, and the CLI `session restart` drives the real resume path.
//! "Reboot" is implicit: each restart is a fresh subprocess with no live daemon
//! or `/tmp` sidecar, so the decision rests entirely on the persisted verified
//! pin.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::{Map, Value};
use serial_test::serial;

use crate::harness::{require_tmux, TuiTestHarness};

const TITLE_A: &str = "SharedCwdA";
const TITLE_B: &str = "SharedCwdB";
const FAKE_AGENT: &str = "aoe-shared-cwd-agent";
const SID_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const SID_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

fn new_harness(test_name: &str) -> TuiTestHarness {
    #[cfg(unix)]
    {
        TuiTestHarness::new_in_tmp(test_name)
    }
    #[cfg(not(unix))]
    {
        TuiTestHarness::new(test_name)
    }
}

fn sessions_path(h: &TuiTestHarness) -> PathBuf {
    crate::harness::app_dir_in(h.home_path()).join("profiles/default/sessions.json")
}

fn read_sessions(h: &TuiTestHarness) -> Value {
    let path = sessions_path(h);
    let content = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));
    serde_json::from_str(&content).expect("invalid sessions JSON")
}

fn session_by_title<'a>(sessions: &'a Value, title: &str) -> &'a Value {
    sessions
        .as_array()
        .and_then(|arr| arr.iter().find(|s| s["title"].as_str() == Some(title)))
        .unwrap_or_else(|| panic!("no session titled '{title}' in sessions.json"))
}

fn patch_session<F>(h: &TuiTestHarness, title: &str, patch: F)
where
    F: FnOnce(&mut Map<String, Value>),
{
    let path = sessions_path(h);
    let mut sessions = read_sessions(h);
    let row = sessions
        .as_array_mut()
        .and_then(|arr| arr.iter_mut().find(|s| s["title"].as_str() == Some(title)))
        .unwrap_or_else(|| panic!("no session titled '{title}' in sessions.json"));
    let row = row.as_object_mut().expect("session row must be an object");
    patch(row);
    fs::write(&path, serde_json::to_string_pretty(&sessions).unwrap())
        .unwrap_or_else(|e| panic!("failed to write {}: {}", path.display(), e));
}

/// Pin a verified Claude session onto `sid`, swapping its launcher for the fake
/// agent while keeping the `claude` tool (so resume-flag construction matches).
fn pin_verified(h: &TuiTestHarness, title: &str, sid: &str) {
    patch_session(h, title, |row| {
        row.insert("command".to_string(), Value::String(FAKE_AGENT.to_string()));
        row.insert("tool".to_string(), Value::String("claude".to_string()));
        row.insert("status".to_string(), Value::String("idle".to_string()));
        row.insert(
            "agent_session_id".to_string(),
            Value::String(sid.to_string()),
        );
        row.insert("session_id_verified".to_string(), Value::Bool(true));
        row.remove("resume_probe_failed_sid");
        row.remove("resume_intent");
    });
}

fn install_fake_agent(h: &mut TuiTestHarness) -> PathBuf {
    let bin = h.install_path_command(FAKE_AGENT);
    let log = h.home_path().join("shared-cwd-agent.log");
    // Log the launch flags, then stay alive so the resume settle-probe passes.
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexec sleep 30\n",
        sh_quote(&log),
    );
    let script_path = bin.join(FAKE_AGENT);
    fs::write(&script_path, script).expect("write fake agent");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755))
            .expect("chmod fake agent");
    }
    log
}

fn sh_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn disable_restart_wake_message(h: &TuiTestHarness) {
    let config_path = crate::harness::app_dir_in(h.home_path()).join("config.toml");
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&config_path)
        .unwrap_or_else(|e| panic!("failed to open {}: {}", config_path.display(), e));
    file.write_all(b"\n[session]\nrestart_wake_message = \"\"\n")
        .expect("disable restart wake message");
}

fn read_log_lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Seed a Claude transcript for `sid` under the shared project's encoded folder,
/// exactly where the host resume path looks. Same encoding as `resume_fallback`:
/// every non `[A-Za-z0-9-]` char maps to `-`. Returns the transcript path so the
/// caller can age it.
fn seed_claude_transcript(h: &TuiTestHarness, project_path: &Path, sid: &str) -> PathBuf {
    let canonical = fs::canonicalize(project_path).unwrap_or_else(|_| project_path.to_path_buf());
    let encoded: String = canonical
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let dir = h.home_path().join(".claude").join("projects").join(encoded);
    fs::create_dir_all(&dir).expect("create claude projects dir");
    let path = dir.join(format!("{sid}.jsonl"));
    fs::write(&path, "{}\n").expect("write claude transcript");
    path
}

fn set_mtime(path: &Path, secs_ago: u64) {
    let when = SystemTime::now() - Duration::from_secs(secs_ago);
    let f = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap_or_else(|e| panic!("open {} for mtime: {}", path.display(), e));
    f.set_modified(when)
        .unwrap_or_else(|e| panic!("set mtime on {}: {}", path.display(), e));
}

struct StopSessionsOnDrop<'a> {
    h: &'a TuiTestHarness,
}

impl Drop for StopSessionsOnDrop<'_> {
    fn drop(&mut self) {
        let _ = self.h.run_cli(&["session", "stop", TITLE_A]);
        let _ = self.h.run_cli(&["session", "stop", TITLE_B]);
    }
}

#[test]
#[serial]
fn shared_cwd_verified_sessions_resume_their_own_sid() {
    require_tmux!();

    let mut h = new_harness("shared_cwd_resume");
    disable_restart_wake_message(&h);
    let log_path = install_fake_agent(&mut h);
    let project = h.project_path();

    for title in [TITLE_A, TITLE_B] {
        let add = h.run_cli(&[
            "add",
            project.to_str().unwrap(),
            "--cmd",
            "claude",
            "-t",
            title,
        ]);
        assert!(
            add.status.success(),
            "aoe add {title} failed: {}",
            String::from_utf8_lossy(&add.stderr)
        );
    }
    let _cleanup = StopSessionsOnDrop { h: &h };

    pin_verified(&h, TITLE_A, SID_A);
    pin_verified(&h, TITLE_B, SID_B);

    // Both transcripts live in the SAME encoded folder (shared cwd). Make B's
    // the newest so a naive freshest-mtime scan would wrongly hand it to A; the
    // verified path must skip that scan entirely.
    let a_transcript = seed_claude_transcript(&h, &project, SID_A);
    let b_transcript = seed_claude_transcript(&h, &project, SID_B);
    set_mtime(&a_transcript, 120);
    set_mtime(&b_transcript, 1);

    // --- Restart A: must resume its OWN sid, never the newer peer B. ---
    let before_a = read_log_lines(&log_path).len();
    let restart_a = h.run_cli(&["session", "restart", TITLE_A]);
    assert!(
        restart_a.status.success(),
        "restart A should succeed: {}",
        String::from_utf8_lossy(&restart_a.stderr)
    );

    let a_lines: Vec<String> = read_log_lines(&log_path).split_off(before_a);
    assert!(
        a_lines.iter().any(|l| l.contains(SID_A)),
        "restart A must launch --resume {SID_A}; new log={a_lines:?}"
    );
    assert!(
        a_lines.iter().all(|l| !l.contains(SID_B)),
        "restart A must NOT resume peer sid {SID_B} (shared-cwd cross-assignment); new log={a_lines:?}"
    );

    let sessions = read_sessions(&h);
    let a_row = session_by_title(&sessions, TITLE_A);
    assert_eq!(a_row["agent_session_id"].as_str(), Some(SID_A));
    assert_eq!(a_row["session_id_verified"].as_bool(), Some(true));
    assert!(
        a_row["resume_probe_failed_sid"].is_null(),
        "A must resume cleanly, got marker {:?}",
        a_row["resume_probe_failed_sid"]
    );
    // B untouched by A's restart.
    assert_eq!(
        session_by_title(&sessions, TITLE_B)["agent_session_id"].as_str(),
        Some(SID_B)
    );

    // --- Restart B: must resume its OWN sid, never A. ---
    let before_b = read_log_lines(&log_path).len();
    let restart_b = h.run_cli(&["session", "restart", TITLE_B]);
    assert!(
        restart_b.status.success(),
        "restart B should succeed: {}",
        String::from_utf8_lossy(&restart_b.stderr)
    );

    let b_lines: Vec<String> = read_log_lines(&log_path).split_off(before_b);
    assert!(
        b_lines.iter().any(|l| l.contains(SID_B)),
        "restart B must launch --resume {SID_B}; new log={b_lines:?}"
    );
    assert!(
        b_lines.iter().all(|l| !l.contains(SID_A)),
        "restart B must NOT resume peer sid {SID_A}; new log={b_lines:?}"
    );

    let sessions = read_sessions(&h);
    assert_eq!(
        session_by_title(&sessions, TITLE_B)["agent_session_id"].as_str(),
        Some(SID_B)
    );
    assert_eq!(
        session_by_title(&sessions, TITLE_A)["agent_session_id"].as_str(),
        Some(SID_A),
        "A's pin must be unchanged after B's restart (no cross-assignment)"
    );
}
