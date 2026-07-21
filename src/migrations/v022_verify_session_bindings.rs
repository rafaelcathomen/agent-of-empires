//! Migration v022: verify durable session bindings.
//!
//! Introduces the durable `session_id_verified` flag (see
//! `Instance::session_id_verified`). A verified row resumes its own
//! `agent_session_id` directly on reboot and is protected from a same-cwd peer
//! or a post-crash stub silently rebinding it. This migration classifies every
//! existing resumable row so healthy sessions gain the protection and provably
//! poisoned ones are flagged (via `resume_probe_failed_sid`) for an explicit
//! re-pin rather than being auto-resumed into the wrong conversation.
//!
//! Per row (tools in {claude,codex,pi,cursor}, `resume_intent` Default/Use,
//! `agent_session_id` present):
//!
//! 1. Collision guard first: a sid claimed by two or more rows is a same-cwd
//!    cross-assignment; verify neither and set `resume_probe_failed_sid` on
//!    both (fail-safe, needs a manual re-pin).
//! 2. `ResumeIntent::Use` is user-authoritative: verify, skip the heal.
//! 3. Verify by content: `capture::sid_cwd_matches` (claude transcript in-file
//!    cwd, codex/pi rollout header cwd) proves the sid belongs to the row's
//!    project. Pass sets `session_id_verified`.
//! 4. Bounded self-heal for a failed row (claude only): a foreign-cwd capture
//!    (transcript lives under a different cwd) flags for re-pin; otherwise, if
//!    the project's own folder holds exactly one transcript unclaimed by any
//!    other instance, adopt and verify it; else leave the sid, flag, and warn.
//!
//! Idempotent: verified rows are skipped and no field is rewritten to a value
//! it already holds, so a re-run is a no-op. Operates on the raw
//! `sessions.json` JSON (like the other data migrations) so it is robust to
//! unrelated schema drift and hand-craft testable.
//!
//! ## Failure policy
//!
//! Per `AGENTS.md > Data Migrations`, a returned `Err` aborts boot. A
//! `sessions.json` that fails to parse is logged and skipped. Only
//! `get_app_dir` and directory-read failures propagate.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::Result;
use tracing::{debug, info, warn};

pub fn run() -> Result<()> {
    let app_dir = crate::session::get_app_dir()?;
    run_in(&app_dir)
}

pub(crate) fn run_in(app_dir: &Path) -> Result<()> {
    let profiles_dir = app_dir.join("profiles");
    if profiles_dir.exists() {
        for entry in fs::read_dir(&profiles_dir)? {
            let entry = entry?;
            if entry.path().is_dir() {
                verify_bindings(&entry.path().join("sessions.json"))?;
            }
        }
    }
    // Legacy top-level sessions.json (pre-profiles layout).
    verify_bindings(&app_dir.join("sessions.json"))?;
    Ok(())
}

fn verify_bindings(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(path)?;
    let mut value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            debug!("v022: failed to parse {}: {e}, skipping", path.display());
            return Ok(());
        }
    };
    let Some(array) = value.as_array_mut() else {
        return Ok(());
    };

    // Frozen pre-mutation snapshots: every row's agent_session_id (the set a
    // heal-adopt must avoid stealing from) and per-sid collision counts over
    // eligible rows only.
    let mut claimed: HashSet<String> = HashSet::new();
    let mut sid_counts: HashMap<String, usize> = HashMap::new();
    for inst in array.iter() {
        if let Some(sid) = inst.get("agent_session_id").and_then(|v| v.as_str()) {
            claimed.insert(sid.to_string());
        }
        if let Some((_, sid)) = eligible(inst) {
            *sid_counts.entry(sid).or_insert(0) += 1;
        }
    }

    let mut verified = 0usize;
    let mut collided = 0usize;
    let mut healed = 0usize;
    let mut flagged = 0usize;
    let mut changed = false;

    for inst in array.iter_mut() {
        let Some((tool, sid)) = eligible(inst) else {
            continue;
        };
        // Idempotency: an already-verified row is settled.
        if inst
            .get("session_id_verified")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        let id = inst
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let project_path = inst
            .get("project_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let intent_kind = inst
            .get("resume_intent")
            .and_then(|ri| ri.get("kind"))
            .and_then(|k| k.as_str())
            .unwrap_or("Default");

        // 1. Collision guard: a sid claimed by two or more rows is a same-cwd
        //    cross-assignment. Verify neither; flag both for a manual re-pin.
        if sid_counts.get(&sid).copied().unwrap_or(0) >= 2 {
            changed |= set_probe_failed(inst, &sid);
            collided += 1;
            warn!(
                "v022: sid {sid} claimed by multiple instances in {}; left unverified, flagged for re-pin",
                path.display()
            );
            continue;
        }

        // 2. Explicit user pin is authoritative.
        if intent_kind == "Use" {
            changed |= set_verified(inst);
            verified += 1;
            continue;
        }

        // 3. Verify by content.
        if crate::session::capture::sid_cwd_matches(&id, &tool, &project_path, &sid) {
            changed |= set_verified(inst);
            verified += 1;
            continue;
        }

        // 4. Bounded self-heal (claude only).
        if tool == "claude" {
            // `sid_cwd_matches` already returned false, so any transcript that
            // exists at all for this sid lives under a foreign cwd (a cross-cwd
            // capture or an encode collision). Flag it; recovery must not resume
            // a conversation that belongs to a different project.
            if let Some(found) = crate::session::capture::claude_transcript_cwd(&sid) {
                changed |= set_probe_failed(inst, &sid);
                flagged += 1;
                warn!(
                    "v022: claude sid {sid} transcript cwd {} != project {}; flagged for re-pin",
                    found.display(),
                    project_path
                );
                continue;
            }
            if let Some(adopt) =
                crate::session::capture::claude_unclaimed_transcript(&project_path, &claimed)
            {
                changed |= set_str(inst, "agent_session_id", &adopt);
                changed |= set_verified(inst);
                changed |= remove_probe_failed(inst);
                healed += 1;
                info!(
                    "v022: adopted unclaimed local claude transcript {adopt} for instance {id} in {}",
                    path.display()
                );
                // Claim the adopted transcript within this run so a second lost
                // row sharing the same project folder cannot adopt it too. Two
                // rows verified onto one conversation would be a collision the
                // pre-frozen guard (built from the distinct stub sids) never
                // sees and the runtime protect block cannot later split.
                claimed.insert(adopt);
                continue;
            }
        }

        // 5. Cannot verify or heal: leave the sid, flag for a manual re-pin.
        changed |= set_probe_failed(inst, &sid);
        flagged += 1;
        warn!(
            "v022: could not verify sid {sid} for instance {id} ({tool}) in {}; left unverified, flagged for re-pin",
            path.display()
        );
    }

    if changed {
        crate::session::atomic_write(path, serde_json::to_string_pretty(&value)?.as_bytes())?;
        info!(
            "v022: verified {verified}, collided {collided}, healed {healed}, flagged {flagged} in {}",
            path.display()
        );
    }
    Ok(())
}

/// Eligible row: tool in {claude,codex,pi,cursor}, `resume_intent` Default or
/// Use, non-empty `agent_session_id`. Returns `(tool, sid)`.
fn eligible(inst: &serde_json::Value) -> Option<(String, String)> {
    let tool = inst.get("tool")?.as_str()?;
    if !matches!(tool, "claude" | "codex" | "pi" | "cursor") {
        return None;
    }
    let kind = inst
        .get("resume_intent")
        .and_then(|ri| ri.get("kind"))
        .and_then(|k| k.as_str())
        .unwrap_or("Default");
    if !matches!(kind, "Default" | "Use") {
        return None;
    }
    let sid = inst.get("agent_session_id")?.as_str()?;
    if sid.trim().is_empty() {
        return None;
    }
    Some((tool.to_string(), sid.to_string()))
}

/// Set `session_id_verified = true`, returning whether the value changed.
fn set_verified(inst: &mut serde_json::Value) -> bool {
    let already = inst
        .get("session_id_verified")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if already {
        return false;
    }
    if let Some(obj) = inst.as_object_mut() {
        obj.insert(
            "session_id_verified".to_string(),
            serde_json::Value::Bool(true),
        );
        return true;
    }
    false
}

/// Set `resume_probe_failed_sid = sid`, returning whether the value changed.
fn set_probe_failed(inst: &mut serde_json::Value, sid: &str) -> bool {
    if inst.get("resume_probe_failed_sid").and_then(|v| v.as_str()) == Some(sid) {
        return false;
    }
    set_str(inst, "resume_probe_failed_sid", sid)
}

fn remove_probe_failed(inst: &mut serde_json::Value) -> bool {
    match inst.as_object_mut() {
        Some(obj) => obj.remove("resume_probe_failed_sid").is_some(),
        None => false,
    }
}

fn set_str(inst: &mut serde_json::Value, key: &str, val: &str) -> bool {
    match inst.as_object_mut() {
        Some(obj) => {
            let changed = obj.get(key).and_then(|v| v.as_str()) != Some(val);
            obj.insert(key.to_string(), serde_json::Value::String(val.to_string()));
            changed
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::capture::encode_claude_project_path;
    use crate::session::test_support::EnvGuard;
    use serial_test::serial;

    fn write_transcript(claude_home: &Path, project_path: &str, sid: &str, in_file_cwd: &str) {
        let dir = claude_home
            .join("projects")
            .join(encode_claude_project_path(project_path));
        fs::create_dir_all(&dir).unwrap();
        let line = format!(
            r#"{{"type":"user","cwd":"{in_file_cwd}","message":{{"role":"user","content":"hi"}}}}"#
        );
        fs::write(dir.join(format!("{sid}.jsonl")), format!("{line}\n")).unwrap();
    }

    fn read_rows(path: &Path) -> Vec<serde_json::Value> {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    #[serial]
    fn classifies_healthy_stub_and_collision() {
        let home = tempfile::tempdir().unwrap();
        let claude = home.path().join(".claude");
        let _guard = EnvGuard::set(&[("CLAUDE_CONFIG_DIR", claude.clone())]);

        let healthy = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let stub = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
        let collide = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

        // Healthy: transcript under its own project with matching in-file cwd.
        write_transcript(&claude, "/work/proj", healthy, "/work/proj");
        // Stub: no transcript anywhere.
        // Collision: claimed by two rows.

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.json");
        let rows = format!(
            r#"[
                {{"id":"a","tool":"claude","project_path":"/work/proj","agent_session_id":"{healthy}"}},
                {{"id":"b","tool":"claude","project_path":"/work/other","agent_session_id":"{stub}"}},
                {{"id":"c","tool":"claude","project_path":"/work/shared","agent_session_id":"{collide}"}},
                {{"id":"d","tool":"claude","project_path":"/work/shared","agent_session_id":"{collide}"}}
            ]"#
        );
        fs::write(&path, rows).unwrap();

        verify_bindings(&path).unwrap();
        let out = read_rows(&path);

        assert_eq!(out[0]["session_id_verified"], serde_json::json!(true));
        // Stub: no transcript, no adoptable local -> flagged, not verified.
        assert!(out[1].get("session_id_verified").is_none());
        assert_eq!(out[1]["resume_probe_failed_sid"], serde_json::json!(stub));
        // Collision: neither verified, both flagged.
        assert!(out[2].get("session_id_verified").is_none());
        assert!(out[3].get("session_id_verified").is_none());
        assert_eq!(
            out[2]["resume_probe_failed_sid"],
            serde_json::json!(collide)
        );
        assert_eq!(
            out[3]["resume_probe_failed_sid"],
            serde_json::json!(collide)
        );

        // Idempotent: a second run rewrites nothing.
        let before = fs::read_to_string(&path).unwrap();
        verify_bindings(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    #[serial]
    fn use_pin_is_verified_and_foreign_capture_flagged() {
        let home = tempfile::tempdir().unwrap();
        let claude = home.path().join(".claude");
        let _guard = EnvGuard::set(&[("CLAUDE_CONFIG_DIR", claude.clone())]);

        let pinned = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
        let foreign = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";
        // Foreign: transcript exists but under a DIFFERENT cwd than the row.
        write_transcript(&claude, "/elsewhere", foreign, "/elsewhere");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.json");
        let rows = format!(
            r#"[
                {{"id":"p","tool":"claude","project_path":"/work/pin","agent_session_id":"{pinned}","resume_intent":{{"kind":"Use","value":"{pinned}"}}}},
                {{"id":"f","tool":"claude","project_path":"/work/here","agent_session_id":"{foreign}"}}
            ]"#
        );
        fs::write(&path, rows).unwrap();

        verify_bindings(&path).unwrap();
        let out = read_rows(&path);

        // Use pin -> verified without any content check.
        assert_eq!(out[0]["session_id_verified"], serde_json::json!(true));
        // Foreign capture -> flagged, not verified.
        assert!(out[1].get("session_id_verified").is_none());
        assert_eq!(
            out[1]["resume_probe_failed_sid"],
            serde_json::json!(foreign)
        );
    }

    #[test]
    #[serial]
    fn adopts_single_unclaimed_local_transcript() {
        let home = tempfile::tempdir().unwrap();
        let claude = home.path().join(".claude");
        let _guard = EnvGuard::set(&[("CLAUDE_CONFIG_DIR", claude.clone())]);

        // Row's stored sid has no transcript, but its project folder holds
        // exactly one unclaimed local transcript -> adopt it.
        let stored = "11111111-1111-4111-8111-111111111111";
        let local = "22222222-2222-4222-8222-222222222222";
        write_transcript(&claude, "/work/lost", local, "/work/lost");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.json");
        let rows = format!(
            r#"[
                {{"id":"x","tool":"claude","project_path":"/work/lost","agent_session_id":"{stored}"}}
            ]"#
        );
        fs::write(&path, rows).unwrap();

        verify_bindings(&path).unwrap();
        let out = read_rows(&path);

        assert_eq!(out[0]["agent_session_id"], serde_json::json!(local));
        assert_eq!(out[0]["session_id_verified"], serde_json::json!(true));
        assert!(out[0].get("resume_probe_failed_sid").is_none());
    }

    #[test]
    #[serial]
    fn does_not_double_adopt_shared_folder_transcript() {
        let home = tempfile::tempdir().unwrap();
        let claude = home.path().join(".claude");
        let _guard = EnvGuard::set(&[("CLAUDE_CONFIG_DIR", claude.clone())]);

        // Two lost rows in ONE project folder, each a stub sid with no
        // transcript, and exactly one unclaimed local transcript. Only one row
        // may adopt it; the other must stay unverified and flagged. Adopting it
        // for both would forge a verified collision the frozen guard can't see.
        let stub_a = "11111111-1111-4111-8111-aaaaaaaaaaaa";
        let stub_b = "22222222-2222-4222-8222-bbbbbbbbbbbb";
        let orphan = "33333333-3333-4333-8333-cccccccccccc";
        write_transcript(&claude, "/work/shared", orphan, "/work/shared");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.json");
        let rows = format!(
            r#"[
                {{"id":"a","tool":"claude","project_path":"/work/shared","agent_session_id":"{stub_a}"}},
                {{"id":"b","tool":"claude","project_path":"/work/shared","agent_session_id":"{stub_b}"}}
            ]"#
        );
        fs::write(&path, rows).unwrap();

        verify_bindings(&path).unwrap();
        let out = read_rows(&path);

        // Exactly one row adopted the orphan (and is verified); never both.
        let a_adopted = out[0]["agent_session_id"] == serde_json::json!(orphan)
            && out[0]["session_id_verified"] == serde_json::json!(true);
        let b_adopted = out[1]["agent_session_id"] == serde_json::json!(orphan)
            && out[1]["session_id_verified"] == serde_json::json!(true);
        assert!(
            a_adopted ^ b_adopted,
            "exactly one row may adopt the orphan"
        );

        // The loser keeps its stub, stays unverified, and is flagged for re-pin.
        let loser = if a_adopted { &out[1] } else { &out[0] };
        assert!(loser.get("session_id_verified").is_none());
        assert!(loser.get("resume_probe_failed_sid").is_some());

        // Idempotent: a second run rewrites nothing.
        let before = fs::read_to_string(&path).unwrap();
        verify_bindings(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    #[serial]
    fn missing_and_corrupt_files_are_safe() {
        let dir = tempfile::tempdir().unwrap();
        verify_bindings(&dir.path().join("nope.json")).unwrap();
        let corrupt = dir.path().join("sessions.json");
        fs::write(&corrupt, "{ not json").unwrap();
        verify_bindings(&corrupt).unwrap();
        assert_eq!(fs::read_to_string(&corrupt).unwrap(), "{ not json");
    }
}
