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
//! `session_meta` record's payload.
//!
//! This module is for terminal-to-structured conversion. It resolves the
//! rollout for a known AoE session cwd, including scratch or worktree dirs.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::session::capture::parse_codex_rollout_metadata;
use crate::session::claude_import::normalize_cwd;

/// Cap how many lines we read per rollout when extracting metadata. The `cwd`
/// usually lives in the first record; a few hundred lines is plenty without
/// reading a multi-MB rollout.
const MAX_SCAN_LINES: usize = 400;

/// A discovered Codex session, summarized for resume/matching.
pub(crate) struct CodexSessionSummary {
    /// The rollout UUID (filename stem). Fed to `session/load`.
    pub(crate) session_id: String,
    /// The working directory recorded in the rollout's `session_meta`.
    cwd: String,
    /// File modification time as a unix epoch millisecond stamp, for
    /// recent-first sorting.
    last_modified_ms: u64,
    is_child: bool,
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

/// The best rollout to resume for `cwd`: the most-recently-modified rollout
/// whose recorded cwd equals `cwd`. No AoE-managed cwd filter is applied
/// because conversion resolves a known AoE session's own rollout. `None` when
/// no rollout matches; callers fall back to a fresh session.
pub(crate) fn find_rollout_for_cwd(cwd: &str) -> Option<CodexSessionSummary> {
    let root = codex_sessions_dir()?;
    find_rollout_for_cwd_in(&root, cwd)
}

/// Testable core of [`find_rollout_for_cwd`]: scan `root` for the newest
/// rollout whose `cwd` matches. It intentionally accepts AoE-managed scratch
/// and worktree dirs because terminal conversion resolves a known AoE session.
fn find_rollout_for_cwd_in(root: &Path, cwd: &str) -> Option<CodexSessionSummary> {
    let target = normalize_cwd(cwd);
    collect_summaries_in(root)
        .into_iter()
        .find(|s| !s.is_child && normalize_cwd(&s.cwd) == target)
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
    let mut is_child = false;

    for line in reader.lines().take(MAX_SCAN_LINES).map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(metadata) = parse_codex_rollout_metadata(line) {
            cwd = Some(metadata.cwd);
            is_child |= metadata.is_child;
            break;
        }
    }

    let cwd = cwd?;
    Some(CodexSessionSummary {
        session_id,
        cwd,
        last_modified_ms,
        is_child,
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

    fn subagent_meta(cwd: &str) -> String {
        format!(
            r#"{{"timestamp":"t","type":"session_meta","payload":{{"cwd":"{cwd}","thread_source":"subagent","source":{{"subagent":{{"thread_spawn":{{}}}}}},"parent_thread_id":"aaaaaaaa-1111-2222-3333-444444444444"}}}}"#
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
    fn summarize_reads_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        fs::create_dir(&work).unwrap();
        let cwd = work.to_str().unwrap();
        let path = write_rollout(
            tmp.path(),
            "019f029d-5a71-71b0-ac3b-09e8d3e068a3",
            "2026-06-28T10-00-00",
            &[meta(cwd)],
        );
        let s = summarize_rollout(&path).unwrap();
        assert_eq!(s.session_id, "019f029d-5a71-71b0-ac3b-09e8d3e068a3");
        assert_eq!(s.cwd, cwd);
    }

    #[test]
    fn rollout_without_cwd_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_rollout(
            tmp.path(),
            "019f029d-5a71-71b0-ac3b-09e8d3e068a3",
            "2026-06-28T10-00-00",
            &[],
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
            &[meta(&cwd)],
        );
        let newer = write_rollout(
            tmp.path(),
            "bbbbbbbb-1111-2222-3333-444444444444",
            "2026-06-28T11-00-00",
            &[meta(&cwd)],
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
    fn find_rollout_for_cwd_skips_newer_subagent() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("proj");
        fs::create_dir(&work).unwrap();
        let cwd = work.to_str().unwrap().to_string();

        let top_level = write_rollout(
            tmp.path(),
            "aaaaaaaa-1111-2222-3333-444444444444",
            "2026-06-28T09-00-00",
            &[meta(&cwd)],
        );
        let child = write_rollout(
            tmp.path(),
            "bbbbbbbb-1111-2222-3333-444444444444",
            "2026-06-28T11-00-00",
            &[subagent_meta(&cwd)],
        );
        let now = std::time::SystemTime::now();
        filetime_set(&top_level, now - std::time::Duration::from_secs(600));
        filetime_set(&child, now);

        let got = find_rollout_for_cwd_in(tmp.path(), &cwd).unwrap();
        assert_eq!(got.session_id, "aaaaaaaa-1111-2222-3333-444444444444");
    }

    #[test]
    fn find_rollout_matches_unnormalized_cwd() {
        // aoe stores worktree paths as `.../repo/../repo-worktrees/branch`,
        // while codex records the resolved cwd. The lookup must still match.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let work = tmp.path().join("repo-worktrees").join("branch");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&work).unwrap();
        let resolved = work.canonicalize().unwrap();
        let recorded = resolved.to_str().unwrap().to_string();
        write_rollout(
            tmp.path(),
            "eeeeeeee-1111-2222-3333-444444444444",
            "2026-06-28T10-00-00",
            &[meta(&recorded)],
        );

        // Query with the `..`-laden path aoe would pass.
        let unnormalized = format!("{}/../repo-worktrees/branch", repo.to_str().unwrap());
        let got = find_rollout_for_cwd_in(tmp.path(), &unnormalized).unwrap();
        assert_eq!(got.session_id, "eeeeeeee-1111-2222-3333-444444444444");
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
            &[meta(&cwd)],
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
    }

    #[test]
    fn find_rollout_for_cwd_matches_scratch_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let scratch = "/home/me/.config/agent-of-empires/scratch/abcd";
        write_rollout(
            tmp.path(),
            "cccccccc-1111-2222-3333-444444444444",
            "2026-06-28T10-00-00",
            &[meta(scratch)],
        );

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
