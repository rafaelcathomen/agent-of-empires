//! Full-stack regression for converting a normal Codex tmux session to the
//! structured view without changing conversations or leaving imported history
//! in the Running state.
#![cfg(feature = "serve")]

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use serial_test::serial;

use crate::harness::{
    app_dir_in, pick_free_port, require_node, require_tmux, wait_for_port, TuiTestHarness,
};

const TARGET_ID: &str = "019f1ddb-343e-7a21-a768-f59622afddc8";
const DECOY_ID: &str = "019f1ddc-1d09-7121-94c8-4ea4a745510e";
const TARGET_USER_1: &str = "TARGET_HISTORY_USER_ONE";
const TARGET_ASSISTANT_1: &str = "TARGET_HISTORY_ASSISTANT_ONE";
const TARGET_USER_2: &str = "TARGET_HISTORY_USER_TWO";
const TARGET_ASSISTANT_2: &str = "TARGET_HISTORY_ASSISTANT_TWO";
const DECOY_MARKER: &str = "DECOY_HISTORY_MARKER";
const LIVE_PROMPT: &str = "LIVE_PROMPT_AFTER_IMPORT";
const LIVE_RESPONSE: &str = "LIVE_RESPONSE_AFTER_IMPORT";

fn parse_session_id(add_stdout: &str) -> String {
    add_stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("ID:"))
        .map(|rest| rest.trim().to_string())
        .unwrap_or_else(|| panic!("could not find session ID in aoe add output:\n{add_stdout}"))
}

fn init_git(project: &Path) {
    for args in [
        vec!["init", "-q"],
        vec!["commit", "--allow-empty", "-q", "-m", "init"],
    ] {
        let out = Command::new("git")
            .args(&args)
            .current_dir(project)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn write_rollout(home: &Path, project: &Path, id: &str, timestamp: &str, modified: SystemTime) {
    let dir = home.join(".codex/sessions/2026/07/01");
    std::fs::create_dir_all(&dir).expect("create Codex sessions dir");
    let path = dir.join(format!("rollout-{timestamp}-{id}.jsonl"));
    let meta = serde_json::json!({
        "timestamp": "2026-07-01T12:00:00Z",
        "type": "session_meta",
        "payload": {
            "id": id,
            "cwd": project,
            "source": "cli",
            "thread_source": "user"
        }
    });
    std::fs::write(&path, format!("{meta}\n")).expect("write Codex rollout");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open Codex rollout")
        .set_modified(modified)
        .expect("set rollout mtime");
}

fn get_json(
    runtime: &tokio::runtime::Runtime,
    client: &reqwest::Client,
    url: &str,
) -> serde_json::Value {
    runtime.block_on(async {
        let response = client.get(url).send().await.expect("GET request");
        let status = response.status();
        let body = response.text().await.expect("GET response body");
        assert!(status.is_success(), "GET {url} failed: {status} {body}");
        serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("invalid JSON from {url}: {e}\n{body}"))
    })
}

fn session_status(sessions: &serde_json::Value, session_id: &str) -> Option<String> {
    sessions["sessions"]
        .as_array()?
        .iter()
        .find(|session| session["id"] == session_id)
        .and_then(|session| session["status"].as_str())
        .map(str::to_string)
}

fn worker_is_live(h: &TuiTestHarness, session_id: &str) -> bool {
    let out = h.run_cli(&["acp", "ps", "--json"]);
    let records: serde_json::Value =
        serde_json::from_slice(&out.stdout).unwrap_or_else(|_| serde_json::json!([]));
    records.as_array().is_some_and(|records| {
        records.iter().any(|record| {
            record["session_id"] == session_id && record["alive"].as_bool() == Some(true)
        })
    })
}

fn stored_acp_session_id(h: &TuiTestHarness, session_id: &str) -> Option<String> {
    let path = app_dir_in(h.home_path()).join("profiles/default/sessions.json");
    let rows: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    rows.as_array()?
        .iter()
        .find(|row| row["id"] == session_id)
        .and_then(|row| row["acp_session_id"].as_str())
        .map(str::to_string)
}

fn stored_import_pending(h: &TuiTestHarness, session_id: &str) -> bool {
    let path = app_dir_in(h.home_path()).join("profiles/default/sessions.json");
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(rows) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    rows.as_array()
        .and_then(|rows| rows.iter().find(|row| row["id"] == session_id))
        .and_then(|row| row["import_pending"].as_bool())
        == Some(true)
}

fn frame_seq_containing(replay: &serde_json::Value, marker: &str) -> u64 {
    replay["frames"]
        .as_array()
        .and_then(|frames| {
            frames
                .iter()
                .find(|frame| frame["event"].to_string().contains(marker))
        })
        .and_then(|frame| frame["seq"].as_u64())
        .unwrap_or_else(|| panic!("replay frame missing marker {marker}: {replay}"))
}

struct TmuxSessionGuard(String);

impl Drop for TmuxSessionGuard {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", &self.0])
            .output();
    }
}

#[test]
#[serial]
fn codex_terminal_conversion_preserves_history_and_returns_idle() {
    require_tmux!();
    require_node!();

    let mut h = TuiTestHarness::new_in_tmp("codex_conversion");
    h.stop_daemon_on_drop();

    let fake_script = serde_json::json!({
        "loadUpdatesBySession": {
            (TARGET_ID): [
                { "sessionUpdate": "user_message_chunk", "content": { "type": "text", "text": TARGET_USER_1 } },
                { "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": TARGET_ASSISTANT_1 } },
                { "sessionUpdate": "wait_ms", "ms": 1250 },
                {
                    "sessionUpdate": "tool_call",
                    "toolCallId": "target-history-tool",
                    "title": "Historical read",
                    "kind": "read",
                    "status": "completed",
                    "content": [{ "type": "content", "content": { "type": "text", "text": "TARGET_TOOL_OUTPUT" } }]
                },
                { "sessionUpdate": "user_message_chunk", "content": { "type": "text", "text": TARGET_USER_2 } },
                { "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": TARGET_ASSISTANT_2 } }
            ],
            (DECOY_ID): [
                { "sessionUpdate": "user_message_chunk", "content": { "type": "text", "text": DECOY_MARKER } },
                { "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": DECOY_MARKER } }
            ]
        },
        "turns": [{
            "updates": [
                { "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": LIVE_RESPONSE } }
            ],
            "stopReason": "end_turn"
        }]
    });
    let script_path = h.home_path().join("codex-conversion-script.json");
    std::fs::write(
        &script_path,
        serde_json::to_vec_pretty(&fake_script).expect("serialize fake ACP script"),
    )
    .expect("write fake ACP script");
    h.install_acp_shim(&script_path);

    let codex_bin = h.install_path_command("codex");
    std::fs::write(codex_bin.join("codex"), "#!/bin/sh\nexec sleep 600\n")
        .expect("write long-running Codex shim");

    let project = h.project_path();
    init_git(&project);

    let add = h.run_cli(&[
        "add",
        project.to_str().unwrap(),
        "-t",
        "codex-convert",
        "-c",
        "codex",
    ]);
    assert!(
        add.status.success(),
        "aoe add failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&add.stdout),
        String::from_utf8_lossy(&add.stderr)
    );
    let session_id = parse_session_id(&String::from_utf8_lossy(&add.stdout));

    let start = h.run_cli(&["session", "start", &session_id]);
    assert!(
        start.status.success(),
        "aoe session start failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&start.stdout),
        String::from_utf8_lossy(&start.stderr)
    );
    let tmux_name = agent_of_empires::tmux::Session::generate_name(&session_id, "codex-convert");
    let _tmux_guard = TmuxSessionGuard(tmux_name.clone());
    assert!(
        Command::new("tmux")
            .args(["has-session", "-t", &tmux_name])
            .output()
            .is_ok_and(|out| out.status.success()),
        "normal Codex tmux session was not created: {tmux_name}"
    );

    let now = SystemTime::now();
    write_rollout(
        h.home_path(),
        &project,
        TARGET_ID,
        "2026-07-01T12-00-00",
        now - Duration::from_secs(60),
    );
    write_rollout(
        h.home_path(),
        &project,
        DECOY_ID,
        "2026-07-01T12-01-00",
        now,
    );
    agent_of_empires::tmux::test_support::set_hidden_env(
        &tmux_name,
        agent_of_empires::tmux::test_support::AOE_CAPTURED_SESSION_ID_KEY,
        TARGET_ID,
    )
    .expect("publish target Codex ID to tmux");

    let port = pick_free_port();
    let start_daemon = h.run_cli(&[
        "serve",
        "--daemon",
        "--port",
        &port.to_string(),
        "--no-auth",
    ]);
    assert!(
        start_daemon.status.success(),
        "aoe serve failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&start_daemon.stdout),
        String::from_utf8_lossy(&start_daemon.stderr)
    );
    assert!(
        wait_for_port(port, Duration::from_secs(10)),
        "daemon did not bind port {port}"
    );

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let client = reqwest::Client::new();
    runtime.block_on(async {
        let url = format!("http://127.0.0.1:{port}/api/sessions/{session_id}/acp/enable");
        let response = client.post(&url).send().await.expect("POST acp enable");
        let status = response.status();
        let body = response.text().await.expect("enable response body");
        assert!(status.is_success(), "POST {url} failed: {status} {body}");
    });

    let replay_url =
        format!("http://127.0.0.1:{port}/api/sessions/{session_id}/acp/replay?since=0&limit=100");
    let sessions_url = format!("http://127.0.0.1:{port}/api/sessions");
    let deadline = Instant::now() + Duration::from_secs(30);
    let (replay, sessions) = loop {
        let replay = get_json(&runtime, &client, &replay_url);
        let sessions = get_json(&runtime, &client, &sessions_url);
        let replay_text = replay.to_string();
        if replay_text.contains(TARGET_USER_1)
            && replay_text.contains(TARGET_ASSISTANT_1)
            && replay_text.contains(TARGET_USER_2)
            && replay_text.contains(TARGET_ASSISTANT_2)
            && replay_text.contains("history_replay_complete")
            && session_status(&sessions, &session_id).as_deref() == Some("Idle")
            && stored_acp_session_id(&h, &session_id).as_deref() == Some(TARGET_ID)
            && worker_is_live(&h, &session_id)
        {
            break (replay, sessions);
        }
        if Instant::now() >= deadline {
            let ps = h.run_cli(&["acp", "ps", "--json"]);
            panic!(
                "Codex conversion did not settle within 30s.\nreplay: {replay}\nsessions: {sessions}\nacp ps: {}",
                String::from_utf8_lossy(&ps.stdout)
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    };

    let replay_text = replay.to_string();
    for marker in [
        TARGET_USER_1,
        TARGET_ASSISTANT_1,
        TARGET_USER_2,
        TARGET_ASSISTANT_2,
    ] {
        assert_eq!(
            replay_text.matches(marker).count(),
            1,
            "target history marker should be replayed exactly once: {marker}\n{replay}"
        );
    }
    assert!(
        !replay_text.contains(DECOY_MARKER),
        "newer same-cwd decoy history was imported: {replay}"
    );
    assert!(replay_text.contains("target-history-tool"));
    assert!(replay_text.contains("ToolCallCompleted"));
    let replay_boundary_seq = frame_seq_containing(&replay, "history_replay_complete");
    for marker in [
        TARGET_USER_1,
        TARGET_ASSISTANT_1,
        TARGET_USER_2,
        TARGET_ASSISTANT_2,
        "TARGET_TOOL_OUTPUT",
    ] {
        assert!(
            frame_seq_containing(&replay, marker) < replay_boundary_seq,
            "imported history overtook the replay boundary: {marker}\n{replay}"
        );
    }
    assert_eq!(
        session_status(&sessions, &session_id).as_deref(),
        Some("Idle")
    );
    assert_eq!(
        stored_acp_session_id(&h, &session_id).as_deref(),
        Some(TARGET_ID)
    );
    assert!(worker_is_live(&h, &session_id));
    assert!(
        !stored_import_pending(&h, &session_id),
        "import_pending remained set after AcpSessionAssigned"
    );
    assert!(
        !Command::new("tmux")
            .args(["has-session", "-t", &tmux_name])
            .output()
            .is_ok_and(|out| out.status.success()),
        "normal terminal tmux session survived conversion"
    );

    std::thread::sleep(Duration::from_secs(1));
    let settled_sessions = get_json(&runtime, &client, &sessions_url);
    assert_eq!(
        session_status(&settled_sessions, &session_id).as_deref(),
        Some("Idle"),
        "imported history became active again after settling: {settled_sessions}"
    );
    assert!(worker_is_live(&h, &session_id));

    let prompt = h.run_cli(&["acp", "prompt", &session_id, LIVE_PROMPT]);
    assert!(
        prompt.status.success(),
        "post-import prompt was not accepted.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&prompt.stdout),
        String::from_utf8_lossy(&prompt.stderr)
    );
    let live_deadline = Instant::now() + Duration::from_secs(10);
    let live_replay = loop {
        let replay = get_json(&runtime, &client, &replay_url);
        let sessions = get_json(&runtime, &client, &sessions_url);
        let replay_text = replay.to_string();
        if replay_text.contains(LIVE_PROMPT)
            && replay_text.contains(LIVE_RESPONSE)
            && replay_text.contains("prompt_complete")
            && session_status(&sessions, &session_id).as_deref() == Some("Idle")
        {
            break replay;
        }
        if Instant::now() >= live_deadline {
            panic!("post-import prompt did not complete.\nreplay: {replay}\nsessions: {sessions}");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(
        frame_seq_containing(&live_replay, LIVE_PROMPT) > replay_boundary_seq,
        "live prompt was not ordered after imported history: {live_replay}"
    );
}
