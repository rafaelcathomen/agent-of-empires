# Codex Structured Conversion Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve the exact terminal Codex conversation and idle state when converting to structured view.

**Architecture:** Resolve the authoritative captured Codex ID before tmux teardown, harden fallback discovery against child rollouts, and emit an explicit history-replay completion boundary after imported events. Keep replay visible while preventing historical activity from behaving like a live turn. Lock the cross-process behavior with an isolated tmux end-to-end test.

**Tech Stack:** Rust, Tokio, Axum, tmux, ACP, React, TypeScript, Vitest, and the Node fake ACP agent.

## Global Constraints

- Follow red, green, refactor for every behavior change.
- Read the live tmux environment before killing the terminal session.
- An exact captured session ID outranks filesystem discovery.
- Filesystem discovery must never select a Codex child or subagent rollout; user-created forks remain eligible.
- Imported transcript rows remain visible, but imported activity must settle silently to Idle.
- Use isolated temporary HOME, tmux, and app-data paths in end-to-end tests.
- Run structured-view verification with `--features serve`.

---

### Task 1: Lock Codex identity selection with failing tests

**Files:**
- Modify: `src/acp/codex_import.rs`
- Modify: `src/session/capture.rs`

- [x] Add a fallback-discovery test where a newer same-directory child rollout must lose to an older top-level rollout.
- [x] Add host and container capture tests for the same child-rollout exclusion.
- [x] Run the focused tests and confirm they fail because the child rollout is currently selected.

### Task 2: Preserve the authoritative terminal Codex identity

**Files:**
- Modify: `src/session/capture.rs`
- Modify: `src/session/instance.rs`
- Modify: `src/server/api/acp.rs`
- Test: `src/acp/codex_import.rs`
- Test: `src/session/capture.rs`

- [x] Parse Codex session metadata once and classify top-level versus child rollouts.
- [x] Exclude child rollouts only from heuristic discovery, while still accepting an exact captured ID.
- [x] Add an instance conversion snapshot that reads `AOE_CAPTURED_SESSION_ID` through a fresh tmux environment query, then falls back to the persisted `agent_session_id`.
- [x] Resolve the conversion ID before tmux teardown and prefer it over cwd discovery.
- [x] Run the focused identity tests until green.

### Task 3: Lock replay completion semantics with failing tests

**Files:**
- Modify: `src/acp/acp_client.rs`
- Modify: `src/server/mod.rs`
- Modify: `src/tui/structured_view/reducer.rs`
- Modify: `web/src/lib/acpTypes.reducer.test.ts`

- [x] Add ACP client tests showing a terminal initial tool call maps to a completed lifecycle rather than an in-flight tool.
- [x] Add reducer tests with multiple imported user turns followed by a history-replay completion boundary.
- [x] Assert the boundary clears active turn, thinking, and open historical tools without producing an empty-output warning.
- [x] Run the focused Rust and Vitest tests and confirm the current implementation fails those assertions.

### Task 4: Make imported history settle silently to Idle

**Files:**
- Modify: `src/acp/acp_client.rs`
- Modify: `src/tui/structured_view/reducer.rs`
- Modify: `web/src/lib/acpTypes.ts`
- Modify: `web/tests/coverage-matrix.json`

- [x] Keep seed history replay visible while suppressing live-turn watchdog and tailer side effects.
- [x] Honor terminal status on initial historical tool-call frames.
- [x] Emit a dedicated stopped reason after successful seed history replay.
- [x] Treat that reason as one silent boundary covering every imported prompt in Rust and web reducers.
- [x] Update the dashboard coverage matrix for the reducer regression test.
- [x] Run focused Rust and web tests until green.

### Task 5: Exercise the real conversion through tmux

**Files:**
- Create: `tests/e2e/codex_conversion_e2e.rs`
- Modify: `tests/e2e/main.rs`
- Modify: `tests/e2e/harness.rs`
- Modify: `web/tests/helpers/fakeAcpAgent.mjs`

- [x] Add a deterministic fake `codex` terminal command and a `codex-acp` shim.
- [x] Seed a target top-level rollout and a newer same-cwd top-level decoy.
- [x] Start the normal Codex session in a real tmux session and publish the target ID into its live tmux environment.
- [x] Switch through the live server endpoint and assert the target transcript is replayed, the decoy transcript is absent, the worker remains alive, and the session is Idle.
- [x] Send a post-import prompt and assert it completes, proving import state cleared.
- [x] Run the new end-to-end test with screen and server diagnostics enabled.

### Task 6: Verify the complete change

**Files:**
- Verify all modified files.

- [x] Run `cargo fmt --check`.
- [x] Run `cargo clippy --features serve --all-targets -- -D warnings`.
- [x] Run `cargo test --features serve`.
- [x] Run `cd web && npm run format:check && npm run lint && npx tsc -b`.
- [x] Re-run the isolated Codex conversion end-to-end test in tmux.
- [x] Inspect the final diff for unrelated changes and confirm the worktree contents.
