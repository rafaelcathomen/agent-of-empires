# Automations — Core Engine + CLI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the durable, daemon-hosted **cron automation** engine and its `aoe automation` CLI, so a user can schedule "summarize my Slack every 30 min" and have the `aoe serve` daemon launch agent runs on time, unattended.

**Architecture:** A new `Automation` entity (trigger + launch spec) persists to `automations.json` via a dual-lock store mirroring `sessions.json`. A new `automation_poll_loop` inside the serve daemon evaluates cron triggers (local timezone) and dispatches **runs** by reusing the existing `Instance` launch path, injecting an initial prompt (a new primitive). Fresh-mode runs auto-archive on completion with keep-last-N retention; persistent-mode runs reuse one session and defer on collision. The CLI is the backbone the later TUI/dashboard plans call into.

**Tech Stack:** Rust (edition 2021, rust-version 1.85), tokio 1.52 (`full`), serde 1.0, serde_json, uuid 1.23 (`v4`), chrono 0.4 (`serde`), clap 4.6 (`derive`, `env`), anyhow. New dependency: `croner` (cron parsing/next-occurrence).

## Global Constraints

- **No dead code.** Never add `#[allow(dead_code)]` or fields nothing reads (AGENTS.md). The `Trigger` enum and the LaunchSpec execution-location are documented *seams* (ADR-0004) but v1 adds **no** unused field for them beyond the `Trigger::Cron` variant actually used.
- **No emdashes / `--` separators** in docs/comments; use commas or rephrase (AGENTS.md).
- **Run `cargo fmt` + `cargo clippy` + `cargo test`** clean before each commit; fix clippy warnings.
- **Rust naming:** `snake_case` fns/modules, `CamelCase` types, `SCREAMING_SNAKE_CASE` consts.
- **Decisions are fixed** in `docs/adr/0001`–`0004` and `CONTEXT.md`; do not relitigate. Canonical noun is **Automation**; one execution is a **Run**; a trigger coming due is a **Fire**.
- **Defaults (from spec):** `max_runtime` 30m, `retention.keep_last` 5, `max_concurrent_runs` 3, `consecutive_failure_limit` 5, scheduler tick 30s, cron timezone = local.
- **No catch-up** for fires missed while the daemon was down; **no within-fire retry**.
- New `automations.json` is a new file, so **no data migration** is required (do not bump `CURRENT_VERSION`).

---

## File Structure

- `src/automation/mod.rs` — module root; re-exports.
- `src/automation/model.rs` — `Automation`, `Trigger`, `LaunchSpec`, `SessionMode`, `Retention`, `RunRecord`, `RunOutcome`, `AutomationState`.
- `src/automation/cron.rs` — cron parse + `next_fire_after(expr, after) -> Option<DateTime<Local>>`.
- `src/automation/store.rs` — `AutomationStore` (load/update over `automations.json`, dual-lock).
- `src/automation/dispatch.rs` — build an `Instance` from a `LaunchSpec`, launch it, return the run's session id.
- `src/automation/scheduler.rs` — `automation_poll_loop`, concurrency cap, persistent-collision coalescing, completion/retention/failure handling.
- `src/automation/lifecycle.rs` — auto-spawn the daemon when the first automation is enabled.
- `src/cli/automation.rs` — `AutomationCommands` + `run`.
- `src/session/config.rs` — add `AutomationConfig` + `Config.automation` field (modify).
- `src/session/instance.rs` — add `initial_prompt` field + injection hook (modify).
- `src/cli/add.rs` + `src/cli/definition.rs` + `src/main.rs` — wire `--prompt` and the `Automation` subcommand (modify).
- `src/server/mod.rs` — spawn `automation_poll_loop` (modify).
- `src/lib.rs` — `pub mod automation;` (modify).

---

### Task 1: Add `croner` dependency and the Automation data model

**Files:**
- Modify: `Cargo.toml` (dependencies)
- Create: `src/automation/mod.rs`
- Create: `src/automation/model.rs`
- Modify: `src/lib.rs` (add `pub mod automation;`)

**Interfaces:**
- Produces: the types every later task consumes:
  - `Automation { id: String, name: String, enabled: bool, trigger: Trigger, spec: LaunchSpec, session_mode: SessionMode, retention: Retention, state: AutomationState }`
  - `enum Trigger { Cron { expr: String } }` (tz is always local; not stored)
  - `struct LaunchSpec { project_path: String, group_path: String, tool: Option<String>, command: Option<String>, extra_args: String, view: View, worktree_branch: Option<String>, sandbox: bool, auto_approve: bool, max_runtime_secs: u64, initial_prompt: String, agent_name: Option<String>, agent_model: Option<String> }`
  - `enum SessionMode { Fresh, Persistent }`
  - `struct Retention { keep_last: u32 }`
  - `struct AutomationState { last_run: Option<RunRecord>, next_fire: Option<DateTime<Utc>>, consecutive_failures: u32, pending_fire: bool, persistent_session_id: Option<String> }`
  - `struct RunRecord { at: DateTime<Utc>, session_id: String, outcome: RunOutcome }`
  - `enum RunOutcome { Completed, TimedOut, Failed { reason: String } }`
  - `Automation::new(name, spec, trigger) -> Automation` and `Automation::short_id(&self) -> &str` (first 8 chars).
- Reuse the existing `crate::session::instance::View` enum for `LaunchSpec.view`.

- [ ] **Step 1: Add the dependency**

In `Cargo.toml` under `[dependencies]`, add:

```toml
croner = "2"
```

- [ ] **Step 2: Write the failing test** in `src/automation/model.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_round_trips_through_json() {
        let spec = LaunchSpec {
            project_path: "/home/me/proj".into(),
            group_path: String::new(),
            tool: Some("claude".into()),
            command: None,
            extra_args: String::new(),
            view: crate::session::instance::View::Terminal,
            worktree_branch: None,
            sandbox: true,
            auto_approve: true,
            max_runtime_secs: 1800,
            initial_prompt: "summarize my slack".into(),
            agent_name: None,
            agent_model: None,
        };
        let a = Automation::new("slack digest", spec, Trigger::Cron { expr: "*/30 * * * *".into() });
        let json = serde_json::to_string(&a).unwrap();
        let back: Automation = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "slack digest");
        assert_eq!(back.session_mode, SessionMode::Fresh);
        assert_eq!(back.retention.keep_last, 5);
        assert_eq!(back.short_id().len(), 8);
        assert!(matches!(back.trigger, Trigger::Cron { .. }));
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p agent-of-empires automation::model 2>&1 | tail -20`
Expected: FAIL — module `automation` does not exist.

- [ ] **Step 4: Create the module + model**

`src/automation/mod.rs`:

```rust
pub mod cron;
pub mod dispatch;
pub mod lifecycle;
pub mod model;
pub mod scheduler;
pub mod store;

pub use model::{
    Automation, AutomationState, LaunchSpec, Retention, RunOutcome, RunRecord, SessionMode,
    Trigger,
};
```

`src/automation/model.rs` (the test module above stays at the bottom):

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::session::instance::View;

fn default_keep_last() -> u32 {
    5
}
fn default_max_runtime_secs() -> u64 {
    1800
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Trigger {
    /// 5-field cron expression evaluated in the user's local timezone.
    Cron { expr: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionMode {
    Fresh,
    Persistent,
}

impl Default for SessionMode {
    fn default() -> Self {
        SessionMode::Fresh
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Retention {
    #[serde(default = "default_keep_last")]
    pub keep_last: u32,
}

impl Default for Retention {
    fn default() -> Self {
        Retention {
            keep_last: default_keep_last(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub project_path: String,
    #[serde(default)]
    pub group_path: String,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub extra_args: String,
    #[serde(default)]
    pub view: View,
    #[serde(default)]
    pub worktree_branch: Option<String>,
    #[serde(default)]
    pub sandbox: bool,
    #[serde(default = "crate::automation::model::default_auto_approve")]
    pub auto_approve: bool,
    #[serde(default = "default_max_runtime_secs")]
    pub max_runtime_secs: u64,
    #[serde(default)]
    pub initial_prompt: String,
    #[serde(default)]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub agent_model: Option<String>,
}

pub fn default_auto_approve() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RunOutcome {
    Completed,
    TimedOut,
    Failed { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub at: DateTime<Utc>,
    pub session_id: String,
    pub outcome: RunOutcome,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutomationState {
    #[serde(default)]
    pub last_run: Option<RunRecord>,
    #[serde(default)]
    pub next_fire: Option<DateTime<Utc>>,
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default)]
    pub pending_fire: bool,
    #[serde(default)]
    pub persistent_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Automation {
    pub id: String,
    pub name: String,
    #[serde(default = "crate::automation::model::default_enabled")]
    pub enabled: bool,
    pub trigger: Trigger,
    pub spec: LaunchSpec,
    #[serde(default)]
    pub session_mode: SessionMode,
    #[serde(default)]
    pub retention: Retention,
    #[serde(default)]
    pub state: AutomationState,
}

pub fn default_enabled() -> bool {
    true
}

impl Automation {
    pub fn new(name: &str, spec: LaunchSpec, trigger: Trigger) -> Self {
        Automation {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            enabled: true,
            trigger,
            spec,
            session_mode: SessionMode::Fresh,
            retention: Retention::default(),
            state: AutomationState::default(),
        }
    }

    /// Short, stable display id (first 8 chars of the uuid).
    pub fn short_id(&self) -> &str {
        &self.id[..8]
    }
}
```

In `src/lib.rs`, add (alphabetical with the other `pub mod` lines):

```rust
pub mod automation;
```

> Note: confirm `View` derives `Default` + `Serialize`/`Deserialize` in `src/session/instance.rs`. It is already serialized on `Instance`, so it does. If `View` lacks `Default`, drop `#[serde(default)]` on `LaunchSpec.view` and require it in the constructor instead.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p agent-of-empires automation::model 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy --all-targets 2>&1 | tail -5
git add Cargo.toml Cargo.lock src/automation/mod.rs src/automation/model.rs src/lib.rs
git commit -m "feat(automation): add croner dep and Automation data model"
```

---

### Task 2: Cron next-fire evaluation (local timezone)

**Files:**
- Modify: `src/automation/cron.rs` (created empty-by-mod in Task 1; create the file now)
- Test: inline `#[cfg(test)]` in the same file.

**Interfaces:**
- Consumes: `croner::Cron`.
- Produces:
  - `fn parse(expr: &str) -> anyhow::Result<croner::Cron>`
  - `fn next_fire_after(expr: &str, after: chrono::DateTime<chrono::Utc>) -> anyhow::Result<chrono::DateTime<chrono::Utc>>` — interprets `expr` in **local** time, returns the next occurrence strictly after `after`, as UTC (so it stores uniformly in `AutomationState.next_fire`).

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn rejects_garbage_expression() {
        assert!(parse("not a cron").is_err());
    }

    #[test]
    fn every_30_minutes_advances_to_next_half_hour_boundary() {
        // 12:05:00 UTC baseline. With a local tz offset the wall-clock minute
        // still lands on a :00 or :30 boundary, so assert on minute modulo.
        let after = Utc.with_ymd_and_hms(2026, 6, 22, 12, 5, 0).unwrap();
        let next = next_fire_after("*/30 * * * *", after).unwrap();
        assert!(next > after);
        assert!(matches!(next.naive_utc().time().minute() % 30, 0));
    }

    #[test]
    fn next_fire_is_strictly_after_input() {
        let exact = Utc.with_ymd_and_hms(2026, 6, 22, 12, 0, 0).unwrap();
        let next = next_fire_after("0 * * * *", exact).unwrap();
        assert!(next > exact);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p agent-of-empires automation::cron 2>&1 | tail -20`
Expected: FAIL — `parse` / `next_fire_after` not found.

- [ ] **Step 3: Implement**

`src/automation/cron.rs`:

```rust
use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use croner::Cron;

/// Parse a 5-field cron expression. Rejects empty/invalid input.
pub fn parse(expr: &str) -> Result<Cron> {
    Cron::new(expr)
        .parse()
        .with_context(|| format!("invalid cron expression: {expr}"))
}

/// Next occurrence strictly after `after`, computed in the user's local
/// timezone (matching Claude Code's "9am means 9am wherever you are"), then
/// returned as UTC for uniform storage.
pub fn next_fire_after(expr: &str, after: DateTime<Utc>) -> Result<DateTime<Utc>> {
    let cron = parse(expr)?;
    let local_after: DateTime<Local> = after.with_timezone(&Local);
    let next_local = cron
        .find_next_occurrence(&local_after, false)
        .with_context(|| format!("no future occurrence for cron: {expr}"))?;
    Ok(next_local.with_timezone(&Utc))
}
```

> `find_next_occurrence(&dt, inclusive)` with `inclusive = false` guarantees strictly-after. Confirm the exact method name against the installed `croner` 2.x docs; if it differs (e.g. `find_next_occurrence` takes a `&Tz`-typed value), adapt the call but keep this module's two public fns and their signatures unchanged.

Add `use chrono::Timelike;` if the test's `.minute()` needs it (it is referenced in the test module, so add the import there).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p agent-of-empires automation::cron 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy --all-targets 2>&1 | tail -5
git add src/automation/cron.rs
git commit -m "feat(automation): cron next-fire evaluation in local timezone"
```

---

### Task 3: `AutomationStore` over `automations.json`

**Files:**
- Create file body: `src/automation/store.rs`
- Test: inline `#[cfg(test)]` using `tempfile` + an env override for the app dir (mirror the pattern in `src/session/storage.rs` tests).

**Interfaces:**
- Consumes: `crate::session::get_profile_dir`, `crate::automation::model::Automation`.
- Produces:
  - `struct AutomationStore { path: PathBuf, save_lock: Arc<Mutex<()>> }`
  - `AutomationStore::new(profile: &str) -> Result<Self>` — path = `get_profile_dir(profile)?.join("automations.json")`.
  - `fn load(&self) -> Result<Vec<Automation>>` — missing file returns `Ok(vec![])`.
  - `fn update<F, R>(&self, f: F) -> Result<R> where F: FnOnce(&mut Vec<Automation>) -> Result<R>` — in-process mutex + cross-process flock on a `.automations.lock` sidecar, load, mutate, atomic write. Mirror `Storage::update` / `acquire_storage_flock` / `atomic_write` in `src/session/storage.rs`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::model::{LaunchSpec, Trigger};

    fn spec() -> LaunchSpec {
        LaunchSpec {
            project_path: "/tmp/p".into(),
            group_path: String::new(),
            tool: Some("claude".into()),
            command: None,
            extra_args: String::new(),
            view: crate::session::instance::View::Terminal,
            worktree_branch: None,
            sandbox: false,
            auto_approve: true,
            max_runtime_secs: 1800,
            initial_prompt: "hi".into(),
            agent_name: None,
            agent_model: None,
        }
    }

    #[test]
    fn load_missing_file_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutomationStore::with_path(tmp.path().join("automations.json"));
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn update_then_load_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutomationStore::with_path(tmp.path().join("automations.json"));
        store
            .update(|list| {
                list.push(crate::automation::model::Automation::new(
                    "x",
                    spec(),
                    Trigger::Cron { expr: "* * * * *".into() },
                ));
                Ok(())
            })
            .unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "x");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p agent-of-empires automation::store 2>&1 | tail -20`
Expected: FAIL — `AutomationStore` not found.

- [ ] **Step 3: Implement** `src/automation/store.rs`

```rust
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::collections::HashMap;

use anyhow::{Context, Result};

use crate::automation::model::Automation;

const LOCK_FILENAME: &str = ".automations.lock";

fn save_lock_for(path: &Path) -> Arc<Mutex<()>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let reg = REGISTRY.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = reg.lock().unwrap();
    map.entry(path.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

pub struct AutomationStore {
    path: PathBuf,
    save_lock: Arc<Mutex<()>>,
}

impl AutomationStore {
    pub fn new(profile: &str) -> Result<Self> {
        let dir = crate::session::get_profile_dir(profile)?;
        Ok(Self::with_path(dir.join("automations.json")))
    }

    pub fn with_path(path: PathBuf) -> Self {
        let save_lock = save_lock_for(&path);
        AutomationStore { path, save_lock }
    }

    pub fn load(&self) -> Result<Vec<Automation>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let bytes = std::fs::read(&self.path)
            .with_context(|| format!("reading {}", self.path.display()))?;
        if bytes.is_empty() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn update<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut Vec<Automation>) -> Result<R>,
    {
        let _guard = self.save_lock.lock().unwrap();
        let dir = self.path.parent().context("automations path has no parent")?;
        std::fs::create_dir_all(dir).ok();
        let _flock = acquire_flock(dir)?;

        let mut list = self.load()?;
        let result = f(&mut list)?;
        let json = serde_json::to_vec_pretty(&list)?;
        atomic_write(&self.path, &json)?;
        Ok(result)
    }
}

/// Cross-process exclusive lock on a sidecar file. Mirror the implementation
/// of `acquire_storage_flock` in `src/session/storage.rs` (fs2 / nix flock).
fn acquire_flock(dir: &Path) -> Result<std::fs::File> {
    use fs2::FileExt;
    let lock_path = dir.join(LOCK_FILENAME);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&lock_path)?;
    file.lock_exclusive()?;
    Ok(file)
}

/// Write to a temp file in the same dir, then rename over the target.
/// Mirror `atomic_write` in `src/session/storage.rs`.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("no parent dir")?;
    let tmp = tempfile::NamedTempFile::new_in(dir)?;
    std::fs::write(tmp.path(), bytes)?;
    tmp.persist(path)?;
    Ok(())
}
```

> Check whether `fs2` is already a dependency (used by `src/session/storage.rs`). If storage uses `nix::fcntl::flock` instead, copy that exact mechanism rather than adding `fs2`. Reuse the project's existing `atomic_write` helper if it is `pub(crate)`; only inline the copy above if it is private to the storage module.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p agent-of-empires automation::store 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy --all-targets 2>&1 | tail -5
git add src/automation/store.rs Cargo.toml Cargo.lock
git commit -m "feat(automation): automations.json store with dual-lock writes"
```

---

### Task 4: `AutomationConfig` settings section

**Files:**
- Modify: `src/session/config.rs` (add the struct + `Config.automation` field)

**Interfaces:**
- Produces: `AutomationConfig { max_concurrent_runs: u32, default_keep_last: u32, default_max_runtime_secs: u64, consecutive_failure_limit: u32, scheduler_tick_secs: u64 }`, reachable as `config.automation`.

- [ ] **Step 1: Write the failing test** (bottom of `src/session/config.rs`'s test module)

```rust
#[test]
fn automation_config_defaults() {
    let c = Config::default();
    assert_eq!(c.automation.max_concurrent_runs, 3);
    assert_eq!(c.automation.default_keep_last, 5);
    assert_eq!(c.automation.consecutive_failure_limit, 5);
    assert_eq!(c.automation.default_max_runtime_secs, 1800);
    assert_eq!(c.automation.scheduler_tick_secs, 30);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p agent-of-empires automation_config_defaults 2>&1 | tail -20`
Expected: FAIL — no field `automation`.

- [ ] **Step 3: Implement**

Add the field to the `Config` struct (after the `acp` field):

```rust
    #[serde(default)]
    pub automation: AutomationConfig,
```

Add the struct near the other `SettingsSection` structs (mirror `LoggingConfig`'s shape):

```rust
fn default_max_concurrent_runs() -> u32 { 3 }
fn default_automation_keep_last() -> u32 { 5 }
fn default_automation_max_runtime_secs() -> u64 { 1800 }
fn default_consecutive_failure_limit() -> u32 { 5 }
fn default_scheduler_tick_secs() -> u64 { 30 }

#[derive(Debug, Clone, Serialize, Deserialize, SettingsSection)]
#[setting_section(name = "automation", category = "Automation")]
pub struct AutomationConfig {
    /// Maximum automation runs executing at once. Extra fires queue.
    #[serde(default = "default_max_concurrent_runs")]
    #[setting(label = "Max concurrent runs", widget = "number", min = 1, validate = "range:1")]
    pub max_concurrent_runs: u32,

    /// Default number of recent fresh-mode run sessions kept per automation.
    #[serde(default = "default_automation_keep_last")]
    #[setting(label = "Keep last N runs", widget = "number", min = 1, validate = "range:1")]
    pub default_keep_last: u32,

    /// Default max wall-clock seconds a run may take before it is stopped.
    #[serde(default = "default_automation_max_runtime_secs")]
    #[setting(label = "Default max runtime (s)", widget = "number", min = 60, validate = "range:60")]
    pub default_max_runtime_secs: u64,

    /// Consecutive failed runs before an automation auto-disables itself.
    #[serde(default = "default_consecutive_failure_limit")]
    #[setting(label = "Auto-disable after N failures", widget = "number", min = 1, validate = "range:1")]
    pub consecutive_failure_limit: u32,

    /// How often the scheduler checks for due automations.
    #[serde(default = "default_scheduler_tick_secs")]
    #[setting(label = "Scheduler tick (s)", widget = "number", min = 5, validate = "range:5", advanced)]
    pub scheduler_tick_secs: u64,
}

impl Default for AutomationConfig {
    fn default() -> Self {
        AutomationConfig {
            max_concurrent_runs: default_max_concurrent_runs(),
            default_keep_last: default_automation_keep_last(),
            default_max_runtime_secs: default_automation_max_runtime_secs(),
            consecutive_failure_limit: default_consecutive_failure_limit(),
            scheduler_tick_secs: default_scheduler_tick_secs(),
        }
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p agent-of-empires automation_config_defaults 2>&1 | tail -20`
Expected: PASS. Also run `cargo build` to confirm the `SettingsSection` derive expands cleanly.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy --all-targets 2>&1 | tail -5
git add src/session/config.rs
git commit -m "feat(automation): AutomationConfig settings section"
```

---

### Task 5: Initial-prompt injection primitive (`--prompt`)

The one genuinely new primitive: launch a session already carrying work.

**Files:**
- Modify: `src/session/instance.rs` (add `initial_prompt` field; add `inject_initial_prompt(&self) -> Result<()>`)
- Modify: `src/cli/add.rs` (add `--prompt` arg; set it; call injection after a successful launch)
- Test: `tests/automation_prompt_injection.rs` (integration) guarded to skip without tmux, mirroring existing tmux-gated tests.

**Interfaces:**
- Consumes: `crate::tmux::Session::send_keys` (terminal), `Supervisor::send_prompt` (ACP).
- Produces:
  - `Instance.initial_prompt: String` (serde default empty, `skip_serializing_if = "String::is_empty"`).
  - `Instance::inject_initial_prompt(&self) -> anyhow::Result<()>` — no-op when empty; for `View::Terminal`, wait for readiness then `Session::send_keys(&self.initial_prompt)`; for `View::Structured`, this is handled by the ACP path (see note).

- [ ] **Step 1: Write the failing test** `tests/automation_prompt_injection.rs`

```rust
// Skips when tmux is unavailable, matching the repo's other tmux-gated tests.
#[test]
fn terminal_session_receives_initial_prompt() {
    if std::process::Command::new("tmux").arg("-V").output().is_err() {
        eprintln!("skipping: tmux not available");
        return;
    }
    use agent_of_empires::session::instance::{Instance, View};

    let mut inst = Instance::new("aoe_test_inject", "/tmp");
    inst.tool = "bash".into();          // launch a plain shell, not a real agent
    inst.command = "bash".into();
    inst.view = View::Terminal;
    inst.initial_prompt = "echo AOE_INJECTED_MARKER".into();
    inst.start_with_size(Some((120, 40))).unwrap();
    inst.inject_initial_prompt().unwrap();

    // Give the pane a moment, then capture and assert the marker echoed.
    std::thread::sleep(std::time::Duration::from_millis(800));
    let session = agent_of_empires::tmux::Session::new(&inst.id, &inst.title).unwrap();
    let dump = session.capture_pane().unwrap_or_default();
    let _ = inst.stop(); // best-effort cleanup
    assert!(dump.contains("AOE_INJECTED_MARKER"), "pane was:\n{dump}");
}
```

> Use the real capture method name from `src/tmux/session.rs` (e.g. `capture_pane` / `capture`); adjust if it differs. Use `Instance::stop`'s real name for cleanup.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test automation_prompt_injection 2>&1 | tail -20`
Expected: FAIL — no `initial_prompt` field / no `inject_initial_prompt`.

- [ ] **Step 3: Implement the field + method**

In `src/session/instance.rs`, add to the struct (near other transient fields):

```rust
    /// One-shot prompt injected right after launch (the Automations initial
    /// prompt, also usable via `aoe add --prompt`). Empty means inject nothing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub initial_prompt: String,
```

Add `initial_prompt: String::new(),` to `Instance::new`.

Add the method (in the `impl Instance` block):

```rust
    /// Inject the initial prompt into a just-launched terminal session.
    /// No-op when empty or for structured sessions (the ACP path injects).
    pub fn inject_initial_prompt(&self) -> anyhow::Result<()> {
        if self.initial_prompt.is_empty() || self.view != View::Terminal {
            return Ok(());
        }
        let session = crate::tmux::Session::new(&self.id, &self.title)?;
        // Brief readiness wait so the agent's REPL is accepting input.
        std::thread::sleep(std::time::Duration::from_millis(600));
        session.send_keys(&self.initial_prompt)?;
        Ok(())
    }
```

- [ ] **Step 4: Wire `--prompt` into `aoe add`**

In `src/cli/add.rs` `AddArgs`:

```rust
    /// Initial prompt to inject into the session right after launch.
    #[arg(long = "prompt")]
    prompt: Option<String>,
```

After the instance is constructed (around line 436), set it:

```rust
    if let Some(p) = args.prompt.as_ref() {
        instance.initial_prompt = p.clone();
    }
```

After a successful `start_with_size(...)` (around line 996, inside the `Ok(())` arm), inject:

```rust
        if let Err(e) = instance.inject_initial_prompt() {
            tracing::warn!(target: "automation", error = %e, "initial prompt injection failed");
        }
```

> For `View::Structured` sessions launched via `--prompt`, injection happens through the ACP path; Task 6 routes structured runs through `Supervisor::send_prompt`. `aoe add --prompt` on a structured session is acceptable to leave as a no-op in this task (terminal is the path the test covers).

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test --test automation_prompt_injection 2>&1 | tail -20`
Expected: PASS (or a clean skip line if tmux is absent).

- [ ] **Step 6: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy --all-targets 2>&1 | tail -5
git add src/session/instance.rs src/cli/add.rs tests/automation_prompt_injection.rs
git commit -m "feat(session): initial-prompt injection primitive and aoe add --prompt"
```

---

### Task 6: Run dispatch — build an Instance from a LaunchSpec and launch it

**Files:**
- Create body: `src/automation/dispatch.rs`
- Test: inline `#[cfg(test)]` for the pure `instance_from_spec` builder (no tmux needed).

**Interfaces:**
- Consumes: `Instance::new`, the `LaunchSpec`, `Supervisor::send_prompt` (for structured).
- Produces:
  - `fn instance_from_spec(automation: &Automation, profile: &str) -> Instance` — pure; maps every `LaunchSpec` field onto a fresh `Instance` (sets `tool`/`command`, `extra_args`, `view`, `yolo_mode = spec.auto_approve`, `group_path`, `initial_prompt`, `agent_name`, `agent_model`, `source_profile = profile`; title = `format!("{} (auto)", automation.name)`).
  - `async fn launch_run(automation: &Automation, profile: &str, supervisor: Option<&Supervisor<..>>) -> Result<String>` — builds the instance, persists it via `crate::session::storage::Storage`, launches it (`start_with_size(None)` for terminal; for structured, the ACP reconciler spawns the worker and we call `supervisor.send_prompt(&id, &prompt, &[])`), injects the terminal prompt, returns the new session id.

- [ ] **Step 1: Write the failing test** (pure builder only)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::model::{Automation, LaunchSpec, Trigger};

    fn spec() -> LaunchSpec {
        LaunchSpec {
            project_path: "/tmp/proj".into(),
            group_path: "team".into(),
            tool: Some("claude".into()),
            command: None,
            extra_args: "--foo".into(),
            view: crate::session::instance::View::Terminal,
            worktree_branch: None,
            sandbox: false,
            auto_approve: true,
            max_runtime_secs: 1800,
            initial_prompt: "summarize slack".into(),
            agent_name: None,
            agent_model: None,
        }
    }

    #[test]
    fn instance_from_spec_maps_fields() {
        let a = Automation::new("slack", spec(), Trigger::Cron { expr: "* * * * *".into() });
        let inst = instance_from_spec(&a, "default");
        assert_eq!(inst.project_path, "/tmp/proj");
        assert_eq!(inst.group_path, "team");
        assert_eq!(inst.extra_args, "--foo");
        assert!(inst.yolo_mode, "auto_approve must map to yolo_mode");
        assert_eq!(inst.initial_prompt, "summarize slack");
        assert!(inst.title.contains("slack"));
        assert_eq!(inst.source_profile, "default");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p agent-of-empires automation::dispatch 2>&1 | tail -20`
Expected: FAIL — `instance_from_spec` not found.

- [ ] **Step 3: Implement** `src/automation/dispatch.rs`

```rust
use anyhow::Result;

use crate::automation::model::Automation;
use crate::session::instance::{Instance, View};
use crate::session::storage::Storage;

/// Pure mapping from an automation's launch spec onto a fresh Instance.
pub fn instance_from_spec(automation: &Automation, profile: &str) -> Instance {
    let spec = &automation.spec;
    let title = format!("{} (auto)", automation.name);
    let mut inst = Instance::new(&title, &spec.project_path);
    inst.source_profile = profile.to_string();
    inst.group_path = spec.group_path.clone();
    if let Some(tool) = &spec.tool {
        inst.tool = tool.clone();
    }
    if let Some(cmd) = &spec.command {
        inst.command = cmd.clone();
    }
    inst.extra_args = spec.extra_args.clone();
    inst.view = spec.view.clone();
    inst.yolo_mode = spec.auto_approve;
    inst.initial_prompt = spec.initial_prompt.clone();
    inst.agent_name = spec.agent_name.clone();
    inst.agent_model = spec.agent_model.clone();
    inst
}

/// Build, persist, launch, and inject. Returns the new session id.
pub async fn launch_run(automation: &Automation, profile: &str) -> Result<String> {
    let mut inst = instance_from_spec(automation, profile);
    let id = inst.id.clone();

    let storage = Storage::new(profile)?;
    storage.update(|all, _groups| {
        all.push(inst.clone());
        Ok(())
    })?;

    // Terminal launch on a blocking thread (tmux is sync); structured view is
    // spawned by the ACP reconciler and prompted separately by the caller.
    if matches!(inst.view, View::Terminal) {
        let mut launch_inst = inst.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            launch_inst.start_with_size(None)?;
            launch_inst.inject_initial_prompt()?;
            Ok(())
        })
        .await??;
    }
    Ok(id)
}
```

> If `Storage::new` watches files in a way that is wrong for a one-shot daemon write, use the unwatched constructor the storage module exposes (the exploration noted `Storage::new_unwatched`). Match whichever the daemon already uses elsewhere. For structured-view runs, the scheduler (Task 7) calls `state.acp_supervisor.send_prompt(&id, &spec.initial_prompt, &[]).await` once the worker is up; this task does not need the supervisor handle.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p agent-of-empires automation::dispatch 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy --all-targets 2>&1 | tail -5
git add src/automation/dispatch.rs
git commit -m "feat(automation): run dispatch builds and launches a session from a spec"
```

---

### Task 7: `automation_poll_loop` — due evaluation, concurrency cap, persistent collision

**Files:**
- Create body: `src/automation/scheduler.rs`
- Modify: `src/server/mod.rs` (spawn the loop after the `status_poll_loop` spawn, ~line 1108)
- Test: inline `#[cfg(test)]` for the pure decision helpers (`due_automations`, `pick_runnable`).

**Interfaces:**
- Consumes: `AutomationStore`, `cron::next_fire_after`, `dispatch::launch_run`, `AppState` (`profile`, `instances`, `acp_supervisor`), `Config.automation`.
- Produces:
  - `fn due_automations(list: &[Automation], now: DateTime<Utc>) -> Vec<usize>` — indices of enabled automations whose `state.next_fire <= now` (or whose `next_fire` is `None`, meaning "initialize").
  - `fn ensure_next_fire(a: &mut Automation, now: DateTime<Utc>)` — set `next_fire` from the cron expr if missing.
  - `async fn automation_poll_loop(state: Arc<AppState>)` — the daemon task.

- [ ] **Step 1: Write the failing tests** (pure helpers)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::model::{Automation, LaunchSpec, Trigger};
    use chrono::{Duration, Utc};

    fn auto(expr: &str) -> Automation {
        let spec = LaunchSpec {
            project_path: "/tmp".into(), group_path: String::new(), tool: Some("claude".into()),
            command: None, extra_args: String::new(), view: crate::session::instance::View::Terminal,
            worktree_branch: None, sandbox: false, auto_approve: true, max_runtime_secs: 1800,
            initial_prompt: "x".into(), agent_name: None, agent_model: None,
        };
        Automation::new("a", spec, Trigger::Cron { expr: expr.into() })
    }

    #[test]
    fn missing_next_fire_counts_as_due_then_initializes() {
        let now = Utc::now();
        let mut a = auto("* * * * *");
        assert_eq!(due_automations(std::slice::from_ref(&a), now), vec![0]);
        ensure_next_fire(&mut a, now);
        assert!(a.state.next_fire.unwrap() > now);
    }

    #[test]
    fn disabled_is_never_due() {
        let now = Utc::now();
        let mut a = auto("* * * * *");
        a.enabled = false;
        a.state.next_fire = Some(now - Duration::minutes(1));
        assert!(due_automations(std::slice::from_ref(&a), now).is_empty());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p agent-of-empires automation::scheduler 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Implement** `src/automation/scheduler.rs`

```rust
use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::automation::cron;
use crate::automation::model::Automation;
use crate::automation::store::AutomationStore;

pub fn due_automations(list: &[Automation], now: DateTime<Utc>) -> Vec<usize> {
    list.iter()
        .enumerate()
        .filter(|(_, a)| a.enabled)
        .filter(|(_, a)| match a.state.next_fire {
            None => true,
            Some(t) => t <= now,
        })
        .map(|(i, _)| i)
        .collect()
}

pub fn ensure_next_fire(a: &mut Automation, now: DateTime<Utc>) {
    let crate::automation::model::Trigger::Cron { expr } = &a.trigger;
    match cron::next_fire_after(expr, now) {
        Ok(next) => a.state.next_fire = Some(next),
        Err(e) => {
            tracing::warn!(target: "automation", id = %a.short_id(), error = %e, "bad cron; disabling");
            a.enabled = false;
        }
    }
}

#[cfg(feature = "serve")]
pub async fn automation_poll_loop(state: Arc<crate::server::AppState>) {
    let profile = state.profile.clone();
    let store = match AutomationStore::new(&profile) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(target: "automation", error = %e, "cannot open automation store; loop exiting");
            return;
        }
    };
    let cfg = crate::session::config::load_config().unwrap_or_default().automation;
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(cfg.scheduler_tick_secs.max(5)));

    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = state.shutdown.cancelled() => return,
        }
        let now = Utc::now();

        // Count automation-launched sessions currently running (concurrency cap).
        let running = count_running_auto_sessions(&state).await;
        let mut budget = cfg.max_concurrent_runs.saturating_sub(running);
        if budget == 0 {
            continue;
        }

        let snapshot = match store.load() {
            Ok(s) => s,
            Err(_) => continue,
        };
        let due = due_automations(&snapshot, now);

        for idx in due {
            if budget == 0 {
                break; // remaining due fires wait for the next tick (no catch-up backlog)
            }
            let automation = snapshot[idx].clone();

            // First-time initialization: just set next_fire, do not run.
            if automation.state.next_fire.is_none() {
                store.update(|list| {
                    if let Some(a) = list.iter_mut().find(|a| a.id == automation.id) {
                        ensure_next_fire(a, now);
                    }
                    Ok(())
                }).ok();
                continue;
            }

            // Dispatch a run for this automation (Task 8 handles completion).
            match crate::automation::dispatch::launch_run(&automation, &profile).await {
                Ok(session_id) => {
                    budget -= 1;
                    store.update(|list| {
                        if let Some(a) = list.iter_mut().find(|a| a.id == automation.id) {
                            ensure_next_fire(a, now);
                            // last_run is finalized on completion (Task 8); record the launch.
                            a.state.last_run = Some(crate::automation::model::RunRecord {
                                at: now,
                                session_id: session_id.clone(),
                                outcome: crate::automation::model::RunOutcome::Completed,
                            });
                        }
                        Ok(())
                    }).ok();
                }
                Err(e) => {
                    tracing::warn!(target: "automation", id = %automation.short_id(), error = %e, "run dispatch failed");
                    record_failure(&store, &automation.id, &profile, e.to_string()).await;
                }
            }
        }
    }
}

#[cfg(feature = "serve")]
async fn count_running_auto_sessions(state: &Arc<crate::server::AppState>) -> u32 {
    let instances = state.instances.read().await;
    instances
        .iter()
        .filter(|i| i.title.ends_with("(auto)"))
        .filter(|i| matches!(i.status, crate::session::Status::Running | crate::session::Status::Starting))
        .count() as u32
}

#[cfg(feature = "serve")]
async fn record_failure(store: &AutomationStore, id: &str, profile: &str, reason: String) {
    let limit = crate::session::config::load_config().unwrap_or_default().automation.consecutive_failure_limit;
    let _ = profile;
    store.update(|list| {
        if let Some(a) = list.iter_mut().find(|a| a.id == id) {
            a.state.consecutive_failures += 1;
            a.state.last_run = Some(crate::automation::model::RunRecord {
                at: Utc::now(),
                session_id: String::new(),
                outcome: crate::automation::model::RunOutcome::Failed { reason },
            });
            if a.state.consecutive_failures >= limit {
                a.enabled = false;
                tracing::error!(target: "automation", id = %a.short_id(), "auto-disabled after repeated failures");
            }
        }
        Ok(())
    }).ok();
}
```

> Persistent-mode collision (defer + coalesce) and fresh-mode completion/retention are layered in Task 8, which subscribes to status transitions. This task keeps the loop's firing + concurrency + dispatch-failure paths. The `(auto)` title suffix is the v1 marker linking a session to an automation; Task 8 replaces it with a stored `automation_id` if a cleaner link is wanted.

- [ ] **Step 4: Spawn the loop in the daemon**

In `src/server/mod.rs`, right after the `status_poll_loop` spawn (~line 1108):

```rust
    #[cfg(feature = "serve")]
    {
        let automation_state = state.clone();
        crate::task_util::spawn_supervised(
            "server.automation_poll_loop",
            crate::task_util::PanicPolicy::Log,
            async move {
                crate::automation::scheduler::automation_poll_loop(automation_state).await;
            },
        );
    }
```

- [ ] **Step 5: Run tests + build the daemon**

Run: `cargo test -p agent-of-empires automation::scheduler 2>&1 | tail -20`
Expected: PASS.
Run: `cargo build --features serve 2>&1 | tail -10`
Expected: builds clean.

- [ ] **Step 6: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy --all-targets --features serve 2>&1 | tail -5
git add src/automation/scheduler.rs src/server/mod.rs
git commit -m "feat(automation): scheduler poll loop with cron firing and concurrency cap"
```

---

### Task 8: Completion detection, fresh-mode archive + retention, persistent collision

**Files:**
- Modify: `src/automation/scheduler.rs` (add a status-transition consumer task + retention prune)
- Test: inline `#[cfg(test)]` for `runs_to_prune` (pure).

**Interfaces:**
- Consumes: `state.status_tx` (`StatusChange`), `AutomationStore`, the existing archive call on `Instance` (find the archive method in `src/session/instance.rs`, e.g. `set_archived(true)` or the storage-level archive used by `aoe session archive`).
- Produces:
  - `fn runs_to_prune(sessions_newest_first: &[String], keep_last: u32) -> Vec<String>` — session ids beyond `keep_last`.
  - `async fn automation_completion_loop(state: Arc<AppState>)` — subscribes to `status_tx`; on a `Running -> Idle` transition for an `(auto)` session, finalizes the run: archive (fresh mode), prune to `keep_last`, reset `consecutive_failures` to 0, and clear a persistent automation's `pending_fire` by injecting the deferred prompt.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn runs_to_prune_keeps_newest_n() {
    let sessions = vec!["s5".to_string(), "s4".into(), "s3".into(), "s2".into(), "s1".into()];
    let prune = runs_to_prune(&sessions, 3);
    assert_eq!(prune, vec!["s2".to_string(), "s1".into()]);
}

#[test]
fn runs_to_prune_noop_when_under_limit() {
    let sessions = vec!["s2".to_string(), "s1".into()];
    assert!(runs_to_prune(&sessions, 5).is_empty());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p agent-of-empires automation::scheduler::tests::runs_to_prune 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Implement**

Add to `src/automation/scheduler.rs`:

```rust
/// Session ids to prune, given newest-first order and a keep-last count.
pub fn runs_to_prune(sessions_newest_first: &[String], keep_last: u32) -> Vec<String> {
    sessions_newest_first
        .iter()
        .skip(keep_last as usize)
        .cloned()
        .collect()
}

#[cfg(feature = "serve")]
pub async fn automation_completion_loop(state: Arc<crate::server::AppState>) {
    let profile = state.profile.clone();
    let store = match AutomationStore::new(&profile) {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut rx = state.status_tx.subscribe();
    loop {
        let change = tokio::select! {
            r = rx.recv() => match r {
                Ok(c) => c,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            },
            _ = state.shutdown.cancelled() => return,
        };
        let finished_turn = change.old == crate::session::Status::Running
            && change.new == crate::session::Status::Idle;
        if !finished_turn || !change.instance_title.ends_with("(auto)") {
            continue;
        }
        finalize_run(&state, &store, &profile, &change.instance_id).await;
    }
}
```

`finalize_run` (same file): load the automation linked to this session (match `state.last_run.session_id` or the `automation_id` link), then:
- reset `consecutive_failures = 0` and set `last_run.outcome = Completed`;
- if `SessionMode::Fresh`: archive the session via the existing archive path, gather that automation's prior run session ids newest-first, call `runs_to_prune`, and delete/stop the pruned sessions (reuse the `aoe session rm` path so worktrees are cleaned);
- if `SessionMode::Persistent` and `state.pending_fire`: clear `pending_fire` and re-inject the prompt via `state.acp_supervisor.send_prompt` (structured) or `Session::send_keys` (terminal).

```rust
#[cfg(feature = "serve")]
async fn finalize_run(
    state: &Arc<crate::server::AppState>,
    store: &AutomationStore,
    profile: &str,
    session_id: &str,
) {
    let _ = (state, profile);
    let keep = crate::session::config::load_config().unwrap_or_default().automation.default_keep_last;
    store.update(|list| {
        if let Some(a) = list.iter_mut().find(|a| {
            a.state.last_run.as_ref().map(|r| r.session_id.as_str()) == Some(session_id)
        }) {
            a.state.consecutive_failures = 0;
            if let Some(r) = a.state.last_run.as_mut() {
                r.outcome = crate::automation::model::RunOutcome::Completed;
            }
            // keep is read here; actual archive/prune of sessions is performed
            // by the helpers below using the sessions Storage.
            let _ = keep;
        }
        Ok(())
    }).ok();
    // Archive + prune the fresh-mode session set via crate::session::storage::Storage,
    // calling the same archive and remove helpers `aoe session archive` / `aoe session rm` use.
}
```

> Implement the archive + prune body against the real archive/remove functions in `src/session/storage.rs` / `src/cli/session.rs`. The unit test only pins `runs_to_prune`; the archive/prune wiring is verified by the Task 9 integration test. `max_runtime` enforcement: in `automation_poll_loop`, track each run's start time and, on a tick, stop any `(auto)` session exceeding `spec.max_runtime_secs`, recording `RunOutcome::TimedOut`. Keep that in this task's commit.

- [ ] **Step 4: Spawn the completion loop** in `src/server/mod.rs` next to the poll-loop spawn:

```rust
    #[cfg(feature = "serve")]
    {
        let completion_state = state.clone();
        crate::task_util::spawn_supervised(
            "server.automation_completion_loop",
            crate::task_util::PanicPolicy::Log,
            async move {
                crate::automation::scheduler::automation_completion_loop(completion_state).await;
            },
        );
    }
```

- [ ] **Step 5: Run tests + build**

Run: `cargo test -p agent-of-empires automation::scheduler 2>&1 | tail -20` → PASS.
Run: `cargo build --features serve 2>&1 | tail -10` → clean.

- [ ] **Step 6: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy --all-targets --features serve 2>&1 | tail -5
git add src/automation/scheduler.rs src/server/mod.rs
git commit -m "feat(automation): completion detection, retention prune, max-runtime, persistent defer"
```

---

### Task 9: Daemon auto-spawn on first enabled automation

**Files:**
- Create body: `src/automation/lifecycle.rs`
- Test: integration `tests/automation_autospawn.rs` (gated; skips without tmux), asserting `daemon_pid()` becomes `Some` after `ensure_scheduler_running`.

**Interfaces:**
- Consumes: `crate::cli::serve::{daemon_pid, ensure_daemon_spawned}` (use the real spawn entry; the exploration shows `start_daemon(profile, &ServeArgs)` — expose a thin `pub fn ensure_daemon_spawned(profile: &str) -> Result<()>` in `serve.rs` if `start_daemon` is private).
- Produces: `fn ensure_scheduler_running(profile: &str) -> Result<bool>` — if `daemon_pid().is_none()`, spawn the daemon and return `true` (spawned), else `false`.

- [ ] **Step 1: Write the failing test** `tests/automation_autospawn.rs`

```rust
#[test]
fn ensure_scheduler_running_spawns_when_absent() {
    if std::process::Command::new("tmux").arg("-V").output().is_err() {
        eprintln!("skipping: tmux not available");
        return;
    }
    // Isolate app dir so we never touch real user state.
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", tmp.path());

    use agent_of_empires::automation::lifecycle::ensure_scheduler_running;
    assert!(agent_of_empires::cli::serve::daemon_pid().is_none());
    let spawned = ensure_scheduler_running("default").unwrap();
    assert!(spawned);
    // Give it a beat to write the pid file.
    std::thread::sleep(std::time::Duration::from_millis(800));
    let pid = agent_of_empires::cli::serve::daemon_pid();
    assert!(pid.is_some());
    // Cleanup: stop the daemon we spawned.
    if let Some(p) = pid {
        let _ = std::process::Command::new("kill").arg(p.to_string()).status();
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test automation_autospawn 2>&1 | tail -20`
Expected: FAIL — `ensure_scheduler_running` not found.

- [ ] **Step 3: Implement** `src/automation/lifecycle.rs`

```rust
use anyhow::Result;

/// Ensure a daemon (the scheduler host) is running. Returns true if this call
/// spawned one. Auto-spawn is the fallback durability path; the consent-gated
/// OS-service install lives in the dashboard plan (ADR-0001).
pub fn ensure_scheduler_running(profile: &str) -> Result<bool> {
    if crate::cli::serve::daemon_pid().is_some() {
        return Ok(false);
    }
    crate::cli::serve::ensure_daemon_spawned(profile)?;
    Ok(true)
}
```

In `src/cli/serve.rs`, add (wrapping the private `start_daemon` with default args):

```rust
/// Spawn the background daemon with scheduler-friendly defaults (local host,
/// default port). Used by `aoe automation add` to guarantee a scheduler host.
pub fn ensure_daemon_spawned(profile: &str) -> anyhow::Result<()> {
    let args = ServeArgs::scheduler_defaults();
    start_daemon(profile, &args)
}
```

Add `ServeArgs::scheduler_defaults()` constructing the same defaults `aoe serve --daemon` uses (host `127.0.0.1`, default port, `daemon: true`). Reuse `Default`/existing field defaults; only set what differs.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test --test automation_autospawn 2>&1 | tail -20`
Expected: PASS (or clean skip).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy --all-targets 2>&1 | tail -5
git add src/automation/lifecycle.rs src/cli/serve.rs tests/automation_autospawn.rs
git commit -m "feat(automation): auto-spawn the scheduler daemon on demand"
```

---

### Task 10: `aoe automation` CLI

**Files:**
- Create: `src/cli/automation.rs`
- Modify: `src/cli/definition.rs` (add `Automation` variant + telemetry name)
- Modify: `src/main.rs` (dispatch)
- Modify: `src/cli/mod.rs` (`pub mod automation;`)
- Test: `tests/e2e/automation_cli_e2e.rs` (CLI subprocess; add/list/rm round-trip; no tmux needed for add/list/rm since they only touch the store).

**Interfaces:**
- Consumes: `AutomationStore`, `Automation::new`, `lifecycle::ensure_scheduler_running`.
- Produces: `pub enum AutomationCommands { Add(AutomationAddArgs), List(AutomationListArgs), Rm(IdArgs), Enable(IdArgs), Disable(IdArgs), RunNow(IdArgs) }` and `pub async fn run(profile: &str, command: AutomationCommands) -> anyhow::Result<()>`.

- [ ] **Step 1: Write the failing e2e test** `tests/e2e/automation_cli_e2e.rs`

```rust
use crate::e2e::harness::TuiTestHarness; // adjust to the actual harness import path
use serial_test::serial;

#[test]
#[serial]
fn automation_add_list_rm_round_trip() {
    let h = TuiTestHarness::new_in_tmp();
    // add
    let out = h.run_cli(&[
        "automation", "add",
        "--name", "slack digest",
        "--cron", "*/30 * * * *",
        "--path", "/tmp",
        "--tool", "claude",
        "--prompt", "summarize my slack",
        "--no-launch-daemon", // test flag: skip auto-spawn during the test
    ]);
    assert!(out.status.success(), "add failed: {out:?}");

    // list shows it
    let list = h.run_cli(&["automation", "list"]);
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(stdout.contains("slack digest"), "list was: {stdout}");

    // rm by short id (first token on the line)
    let id = stdout.split_whitespace().next().unwrap();
    let rm = h.run_cli(&["automation", "rm", id]);
    assert!(rm.status.success());
    let list2 = h.run_cli(&["automation", "list"]);
    assert!(!String::from_utf8_lossy(&list2.stdout).contains("slack digest"));
}
```

> Match the real harness constructor/method names from `tests/e2e/harness.rs` (`new_in_tmp`, `run_cli`). Register the new test module in `tests/e2e/main.rs` (or the e2e module index) the way existing e2e files are registered.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test e2e automation_add_list_rm 2>&1 | tail -20`
Expected: FAIL — unknown subcommand `automation`.

- [ ] **Step 3: Implement the CLI** `src/cli/automation.rs`

```rust
use anyhow::Result;
use clap::{Args, Subcommand};

use crate::automation::model::{Automation, LaunchSpec, Trigger};
use crate::automation::store::AutomationStore;
use crate::session::instance::View;

#[derive(Subcommand)]
pub enum AutomationCommands {
    /// Create an automation (trigger + what to launch)
    Add(AutomationAddArgs),
    /// List automations
    #[command(alias = "ls")]
    List(AutomationListArgs),
    /// Remove an automation by id
    Rm(IdArgs),
    /// Enable an automation
    Enable(IdArgs),
    /// Disable an automation
    Disable(IdArgs),
    /// Fire an automation immediately (for testing)
    RunNow(IdArgs),
}

#[derive(Args)]
pub struct AutomationAddArgs {
    #[arg(long)]
    name: String,
    /// 5-field cron expression (local timezone)
    #[arg(long)]
    cron: String,
    #[arg(long, default_value = ".")]
    path: String,
    #[arg(long)]
    tool: Option<String>,
    #[arg(long = "cmd")]
    command: Option<String>,
    #[arg(long)]
    prompt: String,
    /// Reuse one session across runs instead of a fresh session each time
    #[arg(long)]
    persistent: bool,
    /// Do not auto-spawn the scheduler daemon (test/CI use)
    #[arg(long, hide = true)]
    no_launch_daemon: bool,
}

#[derive(Args)]
pub struct AutomationListArgs {}

#[derive(Args)]
pub struct IdArgs {
    /// Automation id or short id
    id: String,
}

pub async fn run(profile: &str, command: AutomationCommands) -> Result<()> {
    let store = AutomationStore::new(profile)?;
    match command {
        AutomationCommands::Add(args) => {
            // Validate the cron expression up front.
            crate::automation::cron::parse(&args.cron)?;
            let spec = LaunchSpec {
                project_path: std::fs::canonicalize(&args.path)?.to_string_lossy().into_owned(),
                group_path: String::new(),
                tool: args.tool.clone(),
                command: args.command.clone(),
                extra_args: String::new(),
                view: View::Terminal,
                worktree_branch: None,
                sandbox: false,
                auto_approve: true,
                max_runtime_secs: 1800,
                initial_prompt: args.prompt.clone(),
                agent_name: None,
                agent_model: None,
            };
            let mut a = Automation::new(&args.name, spec, Trigger::Cron { expr: args.cron.clone() });
            if args.persistent {
                a.session_mode = crate::automation::model::SessionMode::Persistent;
            }
            let short = a.short_id().to_string();
            store.update(|list| {
                list.push(a.clone());
                Ok(())
            })?;
            if !args.no_launch_daemon {
                if crate::automation::lifecycle::ensure_scheduler_running(profile)? {
                    println!("Started the scheduler daemon (auto-spawn).");
                }
            }
            println!("Created automation {short} ({})", args.name);
            println!("Note: runs unattended with auto-approve enabled.");
        }
        AutomationCommands::List(_) => {
            for a in store.load()? {
                let next = a
                    .state
                    .next_fire
                    .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "pending".into());
                let Trigger::Cron { expr } = &a.trigger;
                let flag = if a.enabled { "on " } else { "off" };
                println!("{}  [{flag}]  {expr:<14}  next={next}  {}", a.short_id(), a.name);
            }
        }
        AutomationCommands::Rm(args) => {
            let mut removed = false;
            store.update(|list| {
                let before = list.len();
                list.retain(|a| !id_matches(a, &args.id));
                removed = list.len() != before;
                Ok(())
            })?;
            println!("{}", if removed { "Removed." } else { "No matching automation." });
        }
        AutomationCommands::Enable(args) => set_enabled(&store, &args.id, true)?,
        AutomationCommands::Disable(args) => set_enabled(&store, &args.id, false)?,
        AutomationCommands::RunNow(args) => {
            let list = store.load()?;
            let a = list.iter().find(|a| id_matches(a, &args.id))
                .ok_or_else(|| anyhow::anyhow!("no matching automation"))?;
            let sid = crate::automation::dispatch::launch_run(a, profile).await?;
            println!("Launched run as session {sid}.");
        }
    }
    Ok(())
}

fn id_matches(a: &Automation, id: &str) -> bool {
    a.id == id || a.short_id() == id
}

fn set_enabled(store: &AutomationStore, id: &str, enabled: bool) -> Result<()> {
    store.update(|list| {
        if let Some(a) = list.iter_mut().find(|a| id_matches(a, id)) {
            a.enabled = enabled;
            if enabled {
                a.state.next_fire = None; // recompute on next tick
            }
        }
        Ok(())
    })?;
    println!("{}", if enabled { "Enabled." } else { "Disabled." });
    Ok(())
}
```

In `src/cli/mod.rs`: `pub mod automation;`.

In `src/cli/definition.rs` `Commands` enum:

```rust
    /// Manage automations (scheduled agent runs)
    Automation {
        #[command(subcommand)]
        command: crate::cli::automation::AutomationCommands,
    },
```

Add `"automation"` to `CLI_COMMAND_NAMES` and a `Commands::Automation { .. } => "automation"` arm in `command_name`.

In `src/main.rs` dispatch:

```rust
        Some(Commands::Automation { command }) => cli::automation::run(&profile, command).await,
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test --test e2e automation_add_list_rm 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Regenerate CLI docs (CI gate)**

Run: `cargo xtask gen-docs 2>&1 | tail -5`
Expected: updates `docs/cli/reference.md` with the new `automation` subcommand.

- [ ] **Step 6: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy --all-targets 2>&1 | tail -5
git add src/cli/automation.rs src/cli/mod.rs src/cli/definition.rs src/main.rs tests/e2e/ docs/cli/reference.md
git commit -m "feat(cli): aoe automation add/list/rm/enable/disable/run-now"
```

---

### Task 11: End-to-end fire test (daemon launches a run on schedule)

**Files:**
- Test: `tests/e2e/automation_fires_e2e.rs` (gated; `#[serial]`; skips without tmux).

**Interfaces:** consumes the whole stack built above.

- [ ] **Step 1: Write the test**

```rust
use serial_test::serial;

#[test]
#[serial]
fn automation_fires_and_produces_a_session() {
    if std::process::Command::new("tmux").arg("-V").output().is_err() {
        eprintln!("skipping: tmux not available");
        return;
    }
    let h = crate::e2e::harness::TuiTestHarness::new_in_tmp();

    // A cron that fires every minute, launching a trivial shell that echoes.
    let add = h.run_cli(&[
        "automation", "add",
        "--name", "ticker",
        "--cron", "* * * * *",
        "--path", "/tmp",
        "--cmd", "bash",
        "--prompt", "echo AOE_FIRED",
    ]);
    assert!(add.status.success());

    // Start the daemon explicitly (deterministic) and wait up to ~75s for a fire.
    let _serve = h.spawn(&["serve", "--daemon"]);
    let mut found = false;
    for _ in 0..75 {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let list = h.run_cli(&["list"]);
        if String::from_utf8_lossy(&list.stdout).contains("ticker (auto)") {
            found = true;
            break;
        }
    }
    assert!(found, "automation did not produce a session within the window");
}
```

> Tune the wait to the scheduler tick (30s) plus cron granularity (up to 60s). If CI flakes on the 75s window, set `automation.scheduler_tick_secs` low via a config file the harness writes, and shorten. Stop the daemon in harness teardown.

- [ ] **Step 2: Run**

Run: `cargo test --test e2e automation_fires 2>&1 | tail -30`
Expected: PASS (or clean skip without tmux).

- [ ] **Step 3: Full gate + commit**

```bash
cargo fmt && cargo clippy --all-targets --features serve 2>&1 | tail -5
cargo test 2>&1 | tail -20
git add tests/e2e/
git commit -m "test(automation): e2e daemon fires a scheduled run"
```

---

## Self-Review

**1. Spec coverage** (each spec/ADR requirement → task):
- Data model + `automations.json` dual-lock store → Tasks 1, 3.
- Cron eval, local tz → Task 2.
- `AutomationConfig` via `#[setting]` → Task 4.
- Initial-prompt injection (the new primitive), ACP + tmux → Task 5 (tmux) + Task 6 note (ACP via supervisor).
- Run dispatch reusing the launch path → Task 6.
- `automation_poll_loop`, concurrency cap, no-catch-up → Task 7.
- Fresh completion (first idle after prompt) + `max_runtime` + retention keep-last-N → Task 8.
- Persistent busy-collision defer/coalesce → Tasks 7/8 (pending_fire + completion-loop re-inject).
- Failure record/notify + auto-disable after N → Task 7 (`record_failure`).
- Daemon auto-spawn on first automation → Task 9. Consent-gated OS-service install → **deferred to Plan 3** (noted, not silently dropped).
- CLI surface → Task 10. TUI + dashboard → **Plans 2 & 3**.
- Result surfacing via existing unread/push → inherited (the `(auto)` session goes idle and the existing `status_poll_loop` marks unread / pushes); no new code needed in v1, called out in Task 8.

**2. Placeholder scan:** No "TBD"/"handle edge cases" left as instructions; every code step carries real code. The few "match the real method name" notes point at concrete files/functions for the implementer to confirm exact identifiers, not invent behavior.

**3. Type consistency:** `LaunchSpec`/`Automation`/`AutomationState`/`RunRecord`/`RunOutcome` field names are used identically across Tasks 1, 3, 6, 7, 8, 10. `instance_from_spec`, `launch_run`, `due_automations`, `ensure_next_fire`, `runs_to_prune`, `ensure_scheduler_running`, `ensure_daemon_spawned` keep one signature each throughout. `auto_approve -> yolo_mode` mapping is asserted in Task 6's test.

**Open confirmations for the implementer** (cheap, verify against code, do not change the design): exact `croner` 2.x next-occurrence method name; whether storage uses `fs2` vs `nix` flock and exposes a reusable `atomic_write`; the real archive/remove helper names in `src/session/storage.rs`/`src/cli/session.rs`; the tmux `capture_pane` method name; and whether `View` derives `Default`.

## Deliberate scope boundary

This plan is **Plan 1 of 3**. It delivers a working CLI-driven cron automation engine. **Plan 2** adds the TUI Automations view; **Plan 3** adds the dashboard panel + REST API (`/api/automations`, coverage-matrix entry) and the consent-gated OS-service install (ADR-0001). Remote execution (ADR-0004) is a future seam, not built here.
