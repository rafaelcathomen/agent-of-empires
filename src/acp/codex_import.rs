//! Discovery of existing Codex CLI sessions (rollouts) on disk, for resuming
//! them in a structured-view session via `session/load`.
//!
//! Codex stores one rollout per session at
//! `<CODEX_HOME>/sessions/<YYYY>/<MM>/<DD>/rollout-<ts>-<UUID>.jsonl`, where
//! `<CODEX_HOME>` is `$CODEX_HOME` or `~/.codex`. The `<UUID>` is the session
//! id `codex-acp` resumes via ACP `session/load` (verified: a `session/load`
//! against this id replays the transcript). So resuming is: find the rollout
//! for a cwd, create a structured session whose `acp_session_id` is that UUID.
//!
//! Each rollout line is `{timestamp, type, payload}`. The `cwd` lives in the
//! `session_meta` record's payload; user turns are `response_item` records with
//! `payload = {type:"message", role:"user", content:[{type:"input_text",
//! text}]}`. The first user message is Codex's injected `AGENTS.md` /
//! `<INSTRUCTIONS>` preamble, so it is skipped for the display title (the
//! analog of Claude's `<command-*>` wrappers).
//!
//! AoE-managed sessions (scratch dirs, worktree/workspace dirs) are excluded
//! using the same filters as `claude_import`.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::acp::claude_import::{cwd_is_aoe_scratch, cwd_under_worktree, worktree_dir_markers};

/// Cap how many lines we read per rollout when extracting metadata. The `cwd`
/// (session_meta, first record) and the first real user message live at the
/// head; a few hundred lines is plenty without reading a multi-MB rollout.
const MAX_SCAN_LINES: usize = 400;

/// A discovered Codex session, summarized for resume/matching.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CodexSessionSummary {
    /// The rollout UUID (filename stem). Fed to `session/load`.
    pub session_id: String,
    /// The working directory recorded in the rollout's `session_meta`.
    pub cwd: String,
    /// First human-authored prompt, truncated, for display. `None` when the
    /// rollout has no readable user message past the injected preamble.
    pub title: Option<String>,
    /// File modification time as a unix epoch millisecond stamp, for
    /// recent-first sorting and "last used" display.
    pub last_modified_ms: u64,
    /// Whether `cwd` still exists.
    pub cwd_exists: bool,
}

/// Base directory Codex stores sessions under: `$CODEX_HOME/sessions` when
/// `CODEX_HOME` is set, else `~/.codex/sessions`. `None` when neither resolves.
fn codex_sessions_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("CODEX_HOME") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir).join("sessions"));
        }
    }
    dirs::home_dir().map(|h| h.join(".codex").join("sessions"))
}

/// Scan all discoverable Codex rollouts, newest first, with AoE-managed
/// sessions (scratch / worktree / workspace) filtered out. Empty when the
/// sessions directory is absent. Unreadable rollouts are skipped, not fatal.
pub fn scan_sessions() -> Vec<CodexSessionSummary> {
    match codex_sessions_dir() {
        Some(root) => scan_sessions_in(&root),
        None => Vec::new(),
    }
}

/// The best rollout to resume for `cwd`: the most-recently-modified rollout
/// whose recorded cwd equals `cwd` (after AoE-managed filtering). `None` when
/// no rollout matches — callers fall back to a fresh session.
pub fn find_rollout_for_cwd(cwd: &str) -> Option<CodexSessionSummary> {
    let root = codex_sessions_dir()?;
    find_rollout_for_cwd_in(&root, cwd)
}

/// Testable core of [`find_rollout_for_cwd`]: scan `root` for the newest
/// rollout whose `cwd` matches. Deliberately does NOT apply the AoE-managed
/// (scratch / worktree) filter: the caller is resolving the rollout of a
/// *known* aoe session by its own cwd, and an aoe codex session legitimately
/// lives in a scratch or worktree dir. Filtering here would make those
/// sessions un-convertible (resume nothing). The AoE-managed filter is only
/// for the external-import picker (`scan_sessions`).
fn find_rollout_for_cwd_in(root: &Path, cwd: &str) -> Option<CodexSessionSummary> {
    collect_summaries_in(root)
        .into_iter()
        .find(|s| s.cwd == cwd)
}

/// Testable core of [`scan_sessions`]: all rollouts under `root`, with
/// AoE-managed (scratch / worktree) sessions filtered out, newest-first.
fn scan_sessions_in(root: &Path) -> Vec<CodexSessionSummary> {
    let markers = worktree_dir_markers();
    collect_summaries_in(root)
        .into_iter()
        .filter(|s| !cwd_is_aoe_scratch(&s.cwd) && !cwd_under_worktree(&s.cwd, &markers))
        .collect()
}

/// Walk `root` recursively for `rollout-*.jsonl`, summarize each, sort
/// newest-first. No filtering; callers decide what to exclude.
fn collect_summaries_in(root: &Path) -> Vec<CodexSessionSummary> {
    let mut out = Vec::new();
    collect_rollouts(root, &mut |path| {
        if let Some(summary) = summarize_rollout(path) {
            out.push(summary);
        }
    });
    out.sort_by_key(|s| std::cmp::Reverse(s.last_modified_ms));
    out
}

/// Recurse `dir`, invoking `f` on every `rollout-*.jsonl` file. Codex nests
/// rollouts under `<YYYY>/<MM>/<DD>/`, but we recurse generically so a layout
/// change does not silently hide sessions. Unreadable dirs are skipped.
fn collect_rollouts(dir: &Path, f: &mut impl FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rollouts(&path, f);
        } else if is_rollout_file(&path) {
            f(&path);
        }
    }
}

fn is_rollout_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("jsonl")
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("rollout-"))
}

/// Build a summary for one rollout. Returns `None` when the file has no
/// recoverable session id or cwd (a session we could not safely resume).
fn summarize_rollout(path: &Path) -> Option<CodexSessionSummary> {
    let session_id = session_id_from_filename(path)?;

    let last_modified_ms = fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);

    let mut cwd: Option<String> = None;
    let mut title: Option<String> = None;

    for line in reader.lines().take(MAX_SCAN_LINES).map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // `cwd` lives in the payload (session_meta / turn_context records).
        if cwd.is_none() {
            if let Some(c) = record
                .get("payload")
                .and_then(|p| p.get("cwd"))
                .or_else(|| record.get("cwd"))
                .and_then(|v| v.as_str())
            {
                if !c.is_empty() {
                    cwd = Some(c.to_string());
                }
            }
        }
        if title.is_none() {
            title = extract_user_title(&record);
        }
        if cwd.is_some() && title.is_some() {
            break;
        }
    }

    let cwd = cwd?;
    let cwd_exists = Path::new(&cwd).is_dir();
    Some(CodexSessionSummary {
        session_id,
        cwd,
        title,
        last_modified_ms,
        cwd_exists,
    })
}

/// Extract the rollout UUID from a `rollout-<ts>-<UUID>.jsonl` filename. The
/// timestamp segment also contains dashes, but a UUID is the last five
/// dash-separated groups (8-4-4-4-12), so we take those and validate the shape.
fn session_id_from_filename(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let parts: Vec<&str> = stem.split('-').collect();
    if parts.len() < 5 {
        return None;
    }
    let candidate = parts[parts.len() - 5..].join("-");
    is_uuid(&candidate).then_some(candidate)
}

/// Loose UUID shape check: groups of 8-4-4-4-12 lowercase hex. Avoids pulling
/// in a uuid-parsing dep just to validate a filename suffix.
fn is_uuid(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    let lens = [8usize, 4, 4, 4, 12];
    groups.len() == 5
        && groups
            .iter()
            .zip(lens)
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Pull a human-readable title from a `response_item` user message, skipping
/// Codex's injected preamble (`# AGENTS.md`, `<INSTRUCTIONS>`,
/// `<environment_context>`, `<user_instructions>`).
fn extract_user_title(record: &serde_json::Value) -> Option<String> {
    let payload = record.get("payload").unwrap_or(record);
    if payload.get("role").and_then(|v| v.as_str()) != Some("user") {
        return None;
    }
    let content = payload.get("content")?;
    let text = match content {
        serde_json::Value::String(s) => displayable_user_text(s).map(str::to_owned),
        serde_json::Value::Array(parts) => parts.iter().find_map(|p| {
            let kind = p.get("type").and_then(|v| v.as_str());
            if kind != Some("input_text") && kind != Some("text") {
                return None;
            }
            let text = p.get("text").and_then(|v| v.as_str())?;
            displayable_user_text(text).map(str::to_owned)
        }),
        _ => None,
    }?;
    Some(truncate(&text, 120))
}

/// A user message's displayable text, or `None` for Codex's injected preamble
/// blocks. Only those specific system injections are dropped; a real prompt is
/// kept even if it happens to start with `<`.
fn displayable_user_text(text: &str) -> Option<&str> {
    let t = text.trim();
    if t.is_empty()
        || t.starts_with("# AGENTS.md")
        || t.starts_with("<INSTRUCTIONS>")
        || t.starts_with("<user_instructions>")
        || t.starts_with("<environment_context>")
    {
        None
    } else {
        Some(t)
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    let trimmed: String = s.chars().take(max_chars).collect();
    if trimmed.chars().count() < s.chars().count() {
        format!("{trimmed}…")
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Write a rollout under `root/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl`.
    fn write_rollout(root: &Path, uuid: &str, ts: &str, lines: &[String]) -> PathBuf {
        let dir = root.join("2026").join("06").join("28");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("rollout-{ts}-{uuid}.jsonl"));
        let mut f = fs::File::create(&path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        path
    }

    fn meta(cwd: &str) -> String {
        format!(r#"{{"timestamp":"t","type":"session_meta","payload":{{"cwd":"{cwd}"}}}}"#)
    }

    fn user_msg(text: &str) -> String {
        let esc = text.replace('\\', "\\\\").replace('"', "\\\"");
        format!(
            r#"{{"timestamp":"t","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{esc}"}}]}}}}"#
        )
    }

    #[test]
    fn session_id_extracted_from_filename() {
        let p = Path::new("rollout-2026-06-26T08-28-14-019f029d-5a71-71b0-ac3b-09e8d3e068a3.jsonl");
        assert_eq!(
            session_id_from_filename(p).as_deref(),
            Some("019f029d-5a71-71b0-ac3b-09e8d3e068a3")
        );
    }

    #[test]
    fn non_uuid_suffix_rejected() {
        assert!(session_id_from_filename(Path::new("rollout-2026-06-26-notauuid.jsonl")).is_none());
    }

    #[test]
    fn summarize_reads_cwd_and_skips_preamble_for_title() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        fs::create_dir(&work).unwrap();
        let cwd = work.to_str().unwrap();
        let path = write_rollout(
            tmp.path(),
            "019f029d-5a71-71b0-ac3b-09e8d3e068a3",
            "2026-06-28T10-00-00",
            &[
                meta(cwd),
                user_msg("# AGENTS.md instructions for /x\n<INSTRUCTIONS>noise"),
                user_msg("Fix the rollout parser please"),
            ],
        );
        let s = summarize_rollout(&path).unwrap();
        assert_eq!(s.session_id, "019f029d-5a71-71b0-ac3b-09e8d3e068a3");
        assert_eq!(s.cwd, cwd);
        assert_eq!(s.title.as_deref(), Some("Fix the rollout parser please"));
        assert!(s.cwd_exists);
    }

    #[test]
    fn rollout_without_cwd_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_rollout(
            tmp.path(),
            "019f029d-5a71-71b0-ac3b-09e8d3e068a3",
            "2026-06-28T10-00-00",
            &[user_msg("hi, no meta record so no cwd")],
        );
        assert!(summarize_rollout(&path).is_none());
    }

    #[test]
    fn find_rollout_for_cwd_picks_newest_match() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("proj");
        fs::create_dir(&work).unwrap();
        let cwd = work.to_str().unwrap().to_string();

        let older = write_rollout(
            tmp.path(),
            "aaaaaaaa-1111-2222-3333-444444444444",
            "2026-06-28T09-00-00",
            &[meta(&cwd), user_msg("older convo")],
        );
        let newer = write_rollout(
            tmp.path(),
            "bbbbbbbb-1111-2222-3333-444444444444",
            "2026-06-28T11-00-00",
            &[meta(&cwd), user_msg("newer convo")],
        );
        // Force mtimes: newer file modified after older.
        let now = std::time::SystemTime::now();
        filetime_set(&older, now - std::time::Duration::from_secs(600));
        filetime_set(&newer, now);

        let got = find_rollout_for_cwd_in(tmp.path(), &cwd).unwrap();
        assert_eq!(got.session_id, "bbbbbbbb-1111-2222-3333-444444444444");
        // Unknown cwd → None.
        assert!(find_rollout_for_cwd_in(tmp.path(), "/nope").is_none());
    }

    #[test]
    #[serial_test::serial]
    fn find_rollout_for_cwd_honors_codex_home_env() {
        // Exercises the exact public path acp_enable calls: CODEX_HOME ->
        // sessions dir -> scan -> cwd match.
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("repo");
        fs::create_dir(&work).unwrap();
        let cwd = work.to_str().unwrap().to_string();
        // write_rollout nests under <root>/2026/06/28; CODEX_HOME/sessions is root.
        write_rollout(
            &tmp.path().join("sessions"),
            "dddddddd-1111-2222-3333-444444444444",
            "2026-06-28T10-00-00",
            &[meta(&cwd), user_msg("env-routed convo")],
        );

        let prev = std::env::var("CODEX_HOME").ok();
        std::env::set_var("CODEX_HOME", tmp.path());
        let got = find_rollout_for_cwd(&cwd);
        match prev {
            Some(v) => std::env::set_var("CODEX_HOME", v),
            None => std::env::remove_var("CODEX_HOME"),
        }

        let got = got.expect("rollout found via CODEX_HOME");
        assert_eq!(got.session_id, "dddddddd-1111-2222-3333-444444444444");
        assert_eq!(got.title.as_deref(), Some("env-routed convo"));
    }

    #[test]
    fn aoe_managed_cwds_excluded_from_picker_but_not_convert() {
        let tmp = tempfile::tempdir().unwrap();
        let scratch = "/home/me/.config/agent-of-empires/scratch/abcd";
        write_rollout(
            tmp.path(),
            "cccccccc-1111-2222-3333-444444444444",
            "2026-06-28T10-00-00",
            &[meta(scratch), user_msg("scratch run")],
        );
        // The external-import picker filters AoE-managed (scratch) cwds out.
        assert!(scan_sessions_in(tmp.path()).is_empty());
        // But the convert path resolves a known aoe session's own rollout by
        // cwd regardless: an aoe codex session can live in a scratch dir and
        // still must be convertible (resume its real conversation).
        let got = find_rollout_for_cwd_in(tmp.path(), scratch).unwrap();
        assert_eq!(got.session_id, "cccccccc-1111-2222-3333-444444444444");
    }

    /// Set a file's mtime deterministically via std's `File::set_modified`
    /// (stable, no extra dep), so the newest-match assertion doesn't race.
    fn filetime_set(path: &Path, t: std::time::SystemTime) {
        let f = fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(t).unwrap();
    }
}
