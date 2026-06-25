# Automations: scheduled (and future event-triggered) agent runs

Status: design, grilled. Vocabulary in [`CONTEXT.md`](../../../CONTEXT.md); foundational
decisions in [`docs/adr/0001`](../../adr/0001-scheduler-runs-in-os-registered-daemon.md),
[`0002`](../../adr/0002-scheduled-runs-auto-approve-and-sandbox.md),
[`0003`](../../adr/0003-run-lifecycle-and-firing-semantics.md),
[`0004`](../../adr/0004-scheduler-host-vs-execution-location.md).

## Goal

Let AoE start agent runs on a timer (and, later, on an event) so it can replace the
Codex / Claude Code desktop apps for unattended recurring work. Headline use case:
"summarize my Slack every 30 minutes." On Linux this is a genuine gap — Claude Code's
durable-local tier (Desktop scheduled tasks) does not exist on Linux at all, and Codex
local cron is fragile ("app must be open"). AoE already runs a persistent daemon, which
makes it the natural durable, local, Linux-friendly host.

## Prior art (what we are matching / beating)

- **Codex Automations**: schedule-only (cron + presets), task = prompt + repo + model +
  exec mode (dir or worktree); standalone (fresh run, results in a Triage inbox) vs
  thread (wake-ups on an existing conversation); local cron is fragile; worktrees
  accumulate. No general event/webhook triggers.
- **Claude Code**: three tiers — `/loop` + cron tools (session-scoped, fires only while
  open+idle, 7-day expiry, no catch-up); Desktop scheduled tasks (OS scheduler, fresh
  session, **macOS/Windows only**); cloud Routines (Anthropic infra, 1h min, can trigger
  on GitHub events). "Events" are push (Channels) or polling (`/loop`), not a standing
  local event engine.

## Scope (v1)

In: durable daemon-hosted **cron** automations across CLI + TUI + dashboard, with the
trigger layer architected so event sources plug in later.
Out (YAGNI, seam only): event triggers (webhook/poller), missed-fire catch-up.

## Domain model

An **Automation** = `{ trigger, launch spec, session mode, retention, state }`. Stored
in a new `automations.json` via the same dual-lock `Storage` pattern as `sessions.json`
(new file, no migration).

```
Automation {
  id, name, enabled,
  trigger: Trigger,                 // enum; v1 only Cron { expr, tz=local }
  spec: LaunchSpec {                // everything `aoe add` already needs
    project_path, group_path, tool/command, extra_args,
    view (terminal|structured), worktree opts, sandbox opts (default on, ADR-0002),
    initial_prompt: String,         // the new primitive (see below)
    agent_name, agent_model,
    auto_approve: bool = true,      // ADR-0002; per-automation opt-out
    max_runtime: Duration = 30m,    // ADR-0003
  },
  session_mode: Fresh | Persistent,
  persistent_session_id: Option<String>,
  retention: { keep_last: u32 = 5 },        // Fresh mode
  state: { last_run, next_fire, consecutive_failures, pending_fire: bool },
}

Trigger = Cron { expr, tz }               // v1
        | /* reserved: Event { source, filter } — not built in v1 */
```

The `Trigger` enum is the seam: a future webhook/poller source plugs in without touching
dispatch, lifecycle, or UI.

**Two deliberate seams, treated symmetrically:**
- `Trigger` (enum) is the **event** seam — v1 builds only `Cron`.
- `LaunchSpec`'s **execution location** is the **remote** seam — v1 inherits whatever the
  `aoe add` launch path supports (local host or local Docker sandbox). When that path
  learns SSH-remote execution, automations get it for free; remote is a third value on an
  axis that already exists (`build_host_command` vs container), not a new concept (ADR-0004).
  No dead field is added in v1 (no-dead-code rule); the data model just documents this as
  the landing spot. The Scheduler Daemon stays a single **local** orchestrator regardless
  of where a run executes.

## Components

### 1. Scheduler Daemon (ADR-0001)
The scheduler runs inside the existing `aoe serve` daemon (a scheduler-only mode that
need not open the web port). A new `automation_poll_loop` sits beside `status_poll_loop`,
ticking ~30s (1-min granularity needs no faster). Each tick: for every enabled Cron
automation with `next_fire <= now`, dispatch a run and recompute `next_fire`
(recomputed from scratch on daemon startup). 5-field cron via a small crate, local
timezone. Optional deterministic per-automation jitter (from id) to avoid `:00` herds.

**Lifecycle / activation:** the first enabled automation auto-spawns the daemon
(works this login session). AoE then prompts once — "make this survive reboot?" — and
on explicit yes installs an OS service (systemd user unit / launchd LaunchAgent). Never
installed silently (ADR-0001/0002 install is consent-gated). Honest limit: nothing fires
while the daemon is down; missed fires are skipped, not replayed.

### 2. Dispatch & run lifecycle (ADR-0003)
On fire, build an `Instance` from `spec` and launch via the **existing** session-creation
path (worktree/sandbox/tool resolution all reused), with `auto_approve` set.

- **Fresh:** new session; inject Initial Prompt; run is complete on the first
  `Running -> Idle` *after* injection (the daemon already detects this transition for
  unread markers). `max_runtime` caps wedged/looping runs (stop, mark timed-out, notify).
  On completion: auto-archive the session, prune so only `keep_last` runs survive
  (deleting pruned worktrees).
- **Persistent:** first fire creates the session and stores `persistent_session_id`;
  later fires re-inject into it (context carries across runs — the Slack case). If a fire
  is due while the session is busy, defer and inject on next `Idle`, coalescing multiple
  missed fires into one pending run. Never interrupt an in-flight turn or a human.

**Concurrency:** a global `max_concurrent_runs` (default 3) with a FIFO queue; a queued
fire that can't start before it goes stale (its next scheduled fire) is dropped and
logged — consistent with no-catch-up. Mirrors the existing ACP worker cap.

**Failure:** record the reason on the run and notify; keep the automation enabled so the
next fire proceeds (transient failures self-heal). After N consecutive failures
(default 5) auto-disable the automation and notify loudly. No within-fire retry.

### 3. Initial-prompt injection (the one new primitive)
Today AoE launches a session and the human types. Automations need to launch *with* a
prompt, so we add `initial_prompt` to the launch path:
- **structured/ACP view:** send it as the first ACP `prompt` once the worker is ready
  (clean, programmatic — preferred for unattended runs).
- **terminal/tmux view:** after launch + a readiness check, `tmux send-keys` the prompt + Enter.
Independently useful as `aoe add --prompt "..."`, so it's a real building block.

### 4. Result surfacing
Reuse existing infra: the run's session becomes Idle with an unread marker and (if
configured) a web-push notification. A digest just shows up as a new idle session with an
unread badge — no new notification system.

### 5. Management surfaces (all three)
- **CLI (backbone):** `aoe automation add | list | rm | enable | disable | run-now`
  (`run-now` = manual fire, for testing). Reuses `aoe add`'s arg parsing for the spec.
- **TUI:** an Automations view to browse/add/edit/delete, showing `last_run` outcome and
  `next_fire`.
- **Dashboard:** an Automations panel with `GET/POST/PATCH/DELETE /api/automations`;
  server validates against the schema. Cron/retention/runtime config fields flow through
  the existing `#[setting]`-derived schema where they map to config; the automation
  itself is a dedicated typed resource.

## Testing

- **Unit:** cron next-fire (incl. local tz / DST), retention pruning, no-catch-up skip,
  consecutive-failure auto-disable, busy-collision coalescing.
- **Integration:** near-future cron + fake agent shim → one run → session archived →
  pruned to `keep_last`; persistent mode re-injects into the same session id; collision
  defers to next idle.
- **e2e:** `aoe automation add` → daemon fires → session appears (reuse the fake-ACP
  harness for prompt injection). Per AGENTS.md, add a `web/tests/coverage-matrix.json`
  entry for the dashboard Automations panel.

## Deliberate cuts (v1)

- No event triggers (enum seam only).
- No missed-fire catch-up (matches Codex local + Claude Code session tasks).
- No within-fire retry.
