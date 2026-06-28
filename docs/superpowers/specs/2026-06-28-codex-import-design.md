# Lossless codex → structured conversion

## Context

Converting an existing terminal session to the structured (ACP) view
(`POST /api/sessions/{id}/acp/enable`) preserves the conversation for **claude**
sessions: the status hooks capture the agent session id into
`Instance.acp_session_id`, so on conversion the structured worker calls
`session/load` and the agent replays the transcript (gated on the agent
advertising `agent_capabilities.load_session = true`, which is agent-agnostic in
aoe).

**Codex** sessions don't get this: aoe never captures a codex session id, so
`acp_session_id` is `None` and conversion fresh-spawns an empty structured
session. Verified prerequisites for fixing this:

- `codex-acp` (v0.16.0, `@zed-industries/codex-acp`) advertises
  `loadSession: true` at `initialize`.
- `session/load` against an existing codex rollout **replays the transcript**
  (confirmed: 142 `session/update` events — `tool_call`, `tool_call_update`,
  `agent_message_chunk` — with real historical content).

So aoe's existing replay machinery works for codex unchanged. The only gap is
discovering the codex rollout for a session and wiring its id into the convert
path.

## Goal

When converting an aoe-tracked **terminal codex** session to the structured
view, resume its conversation losslessly by discovering the codex rollout for
the session and feeding its id into the existing `session/load` + history-replay
path.

## Non-goals

- The external-session import picker (#2276) — out of scope (convert-only).
- Capturing the codex session id at launch (a codex `__extract-session-id`
  analog) — discovery-at-convert is enough for v1; capture-at-launch is a future
  robustness improvement.
- Any change to the agent-agnostic replay / `seed_history_replay` / worker
  `session/load` machinery (it already works).
- Claude or other agents (their convert path is unchanged).

## Codex rollout format (pinned from a real rollout)

Files: `~/.codex/sessions/<YYYY>/<MM>/<DD>/rollout-<ts>-<UUID>.jsonl`.
Line-delimited JSON, each record `{timestamp, type, payload}`:

- `type = "session_meta"` (first record): `payload` carries the session `cwd`
  (and the codex session id). This is the authoritative `cwd`.
- `type = "response_item"`, `payload = {type:"message", role:"user",
  content:[{type:"input_text", text:"…"}]}`: user turns. The **first** user
  message is the injected `AGENTS.md`/`<INSTRUCTIONS>` system preamble (noise),
  analogous to claude's `<command-*>` wrappers — skip it for the title.

The **session id** codex-acp's `session/load` resumes by is the rollout's UUID
(the filename stem), matching `codex resume <id>`.

## Architecture

### 1. Discovery module — `src/acp/codex_import.rs`

A codex analog of `claude_import.rs`, sharing its AoE-managed filtering ideas
(scratch dirs, worktree/workspace path markers):

```rust
pub struct CodexSessionSummary {
    pub session_id: String,     // rollout UUID (filename stem)
    pub cwd: String,            // from session_meta payload
    pub title: Option<String>,  // first real user message, truncated
    pub last_modified_ms: u64,
    pub cwd_exists: bool,
}

/// Scan ~/.codex/sessions/**/rollout-*.jsonl, newest first, AoE-managed filtered.
pub fn scan_sessions() -> Vec<CodexSessionSummary>;

/// Best rollout to resume for a given cwd: the most-recently-modified
/// rollout whose recorded cwd == `cwd` (after AoE-managed filtering).
/// `None` when no rollout matches.
pub fn find_rollout_for_cwd(cwd: &str) -> Option<CodexSessionSummary>;
```

Parsing reads only the head of each file (cap ~400 lines, like claude_import)
to pull `cwd` (from `session_meta`) and the first non-preamble user message.
Unreadable / `cwd`-less / id-less files are skipped, not fatal.

### 2. Convert wiring — `acp_enable` (`src/server/api/acp.rs`)

Before persisting the terminal→structured swap, when the session's tool is
**codex** and `acp_session_id` is `None`:

1. `codex_import::find_rollout_for_cwd(&instance.project_path)`.
2. If a rollout is found, set `instance.acp_session_id = Some(rollout.session_id)`
   and `instance.import_pending = Some(true)`.
3. If none found, leave `acp_session_id = None` (today's behavior → fresh
   structured session) and log an info line.

The existing code then persists and spawns the worker, which (because
`acp_session_id` is set and `import_pending`/`seed_history_replay` is true) calls
`session/load` and seeds the transcript from the replay. No replay-path changes.

The resolution is gated to `tool == "codex"` with `acp_session_id.is_none()`, so
claude and every other agent are untouched.

### 3. Matching strategy

Most-recently-modified rollout whose recorded `cwd` equals the session's
`project_path`. Rationale: a terminal codex session is the codex process that
wrote a rollout in that cwd; the most recent rollout for the cwd is almost
always it. A wrong or missing match degrades safely (see Error handling), so a
simple, predictable rule beats a fragile heuristic. No match → fresh convert
(unchanged behavior), logged.

## Error handling

- **No rollout for the cwd:** convert fresh (today's behavior), log an info line.
  Never resume an unrelated conversation.
- **`session/load` rejects the id** (stale/foreign rollout): the existing path
  clears `acp_session_id` and emits `SessionContextReset` (the amber "prior
  turns not in the model's context" callout). The session is a working fresh
  structured session, reversible via `acp_disable`.
- **Unreadable / malformed rollout files:** skipped during discovery.

## Files touched

New: `src/acp/codex_import.rs` (declare `mod codex_import;` in `src/acp/mod.rs`).
Modified: `src/server/api/acp.rs` — codex rollout resolution inside `acp_enable`
before the persist/spawn.

## Testing

- **Unit (`codex_import`):** rollout parsing (id from filename, cwd from
  `session_meta`, title skipping the AGENTS.md preamble), AoE-managed filtering
  (scratch/worktree), `find_rollout_for_cwd` picking the newest cwd match and
  returning `None` for an unknown cwd. Fixtures are hand-written `.jsonl` trees
  in a tempdir (no real `~/.codex` dependency).
- **Integration:** `acp_enable` on a codex terminal session, with a seeded
  fake `~/.codex/sessions` tree, sets `acp_session_id` + `import_pending` from
  the matching rollout; a non-codex session and a codex session with no matching
  rollout are left unchanged.
- A live codex-acp resume e2e is auth-gated; the integration test covers the
  wiring and the unit tests cover parsing. (Manual verification: the spike
  already confirmed codex-acp replays the transcript on `session/load`.)

## Scope boundary

Convert-only, codex-only. The import picker and capture-at-launch are explicit
follow-ups. Reuses the existing, verified replay path end to end.
