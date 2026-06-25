# Automations TUI view (Plan 2)

## Context

Plan 1 (`add-new-aoe-feature`) delivered the automations engine and CLI: an
`Automation` (trigger + launch spec) persisted to `automations.json` via
`AutomationStore`, a daemon `automation_poll_loop` that fires cron triggers and
dispatches runs, and the CLI backbone `aoe automation add | list | rm | enable |
disable | run-now`.

This plan adds the **TUI Automations view** the design spec promised in
"Management surfaces": *"an Automations view to browse/add/edit/delete, showing
`last_run` outcome and `next_fire`."* The dashboard panel + REST API remain
Plan 3.

Glossary terms (Automation, Trigger, Cron Trigger, Fire, Run, Launch Spec,
Session Mode, Retention) are defined in `CONTEXT.md` and used here as-is.

## Goal

A full-screen TUI view to manage automations: browse the list with `next_fire`
and `last_run`, enable/disable, delete, run-now (manual fire), and add/edit
automations by reusing the existing new-session wizard to collect the launch
spec plus a small schedule dialog for the automation-only fields.

## Non-goals

- Dashboard panel / REST API (Plan 3).
- Live per-second `next_fire` countdown; relative time computed at render is
  enough.
- Editing daemon-owned state (`consecutive_failures`, `pending_fire`,
  `persistent_session_id`) — shown read-only.
- Any change to the Plan 1 engine, store schema, or CLI semantics.

## Architecture

The view follows the established **full-screen takeover** pattern used by
Settings, Diff, and Serve: an `Option<AutomationsView>` field on `HomeView`,
checked first in the render and input dispatch, rendered full-screen, closed by
setting the option back to `None`.

New module `src/tui/automations/` mirroring `src/tui/settings/`:

- `mod.rs` — `AutomationsView` state struct, `new()` (loads the store), mode
  enum, and the public `render` / `handle_key` entry points.
- `render.rs` — list + detail panes and the schedule dialog rendering.
- `input.rs` — key handling per mode.

Data flows through the **existing `AutomationStore`** (Plan 1). The view loads
the full `Vec<Automation>` in `new()`, caches it in state, and reloads after
every mutation (toggle, delete, run-now, add, edit). No engine code changes.

### State

```rust
pub struct AutomationsView {
    automations: Vec<Automation>,   // cached snapshot from the store
    selected: usize,                // cursor in the list
    mode: Mode,
    error: Option<String>,          // inline error banner
}

enum Mode {
    List,
    ConfirmDelete,                  // confirm dialog for the selected automation
    Schedule(ScheduleForm),         // add/edit the automation-only fields
}
```

The `Schedule(ScheduleForm)` mode carries the schedule dialog's own state:
collected `NewSessionData` (the launch spec, when adding), the editing target
(`None` = add, `Some(id)` = edit), and the four automation-only inputs.

### Opening the view

A new `ActionId::Automations` bound to `a` (non-strict) / `A` (strict),
`Context::Always`, with a command-palette entry ("Open automations",
keywords: schedule, cron, automation). `a`/`A` are currently unused on the home
screen (Settings is `s`/`S`, Serve `R`, Profiles `P`).

`HomeView` gains `open_automations_view()`: construct `AutomationsView::new()`;
on store-load error, surface it through the existing info-dialog path rather
than opening a broken view.

## Components and flows

### 1. Browse (List mode)

Two-pane layout (left list, right detail):

- **List row:** enabled glyph (`●` enabled / `○` disabled) · name · cron expr ·
  `next_fire` as relative time ("in 4h", "—" if none/disabled) · last-run
  outcome glyph (✓ success / ✗ failure / · never run).
- **Detail pane** (selected automation): launch spec (project path, group,
  tool/command, view, worktree, sandbox, yolo/auto-approve, initial prompt),
  session mode, retention keep-last, `next_fire`, `last_run` (timestamp +
  outcome + session id), consecutive failures.

Keys: `↑/↓` or `j/k` move selection; `Space`/`Enter` toggle enable/disable;
`a` add; `e` edit; `d` delete; `r` run-now; `Esc` close the view. Empty-list
state shows a hint to press `a`.

### 2. Enable / disable

Toggle `enabled` on the selected automation, persist via the store, reload.
This mirrors `aoe automation enable|disable`; it reuses the same store mutation
the CLI calls so behavior (including `next_fire` recomputation on enable) stays
identical.

### 3. Delete

`d` → `ConfirmDelete` mode (a centered confirm dialog naming the automation).
Confirm removes it via the store (same path as `aoe automation rm`) and reloads;
cancel returns to List.

### 4. Run-now

`r` invokes the **same dispatch entry point the CLI's `run-now` uses**
(`automation::dispatch` / the scheduler's run launch), ensuring the daemon is
spawned if required (Plan 1's `ensure_daemon_spawned`). The call must not block
the UI thread: it is fire-and-forget (spawn/dispatch, then reload to reflect the
new `last_run` once it lands). Any dispatch error surfaces in the error banner.

### 5. Add — reuse the new-session wizard

The new-session wizard (`src/tui/dialogs/new_session/`) is a **pure collector**:
it produces a `NewSessionData` and the *caller* (`home/operations.rs::create_session`)
decides to launch. We exploit that:

1. `a` opens the **existing `NewSessionDialog`**, tagged with an "automation"
   purpose (a small `NewSessionPurpose { Session, Automation }` flag stored on
   `HomeView` alongside the dialog, defaulting to `Session`) so the wizard's
   submit is routed to automation creation instead of `create_session`.
2. On wizard submit, open the **Schedule dialog** (`Mode::Schedule`) carrying
   the returned `NewSessionData`. It collects the automation-only fields:
   - **name** (text; defaults to the session title)
   - **cron expression** (text; validated with Plan 1's `automation::cron`
     next-fire parse — invalid expressions block submit with an inline error)
   - **session mode** (Fresh / Persistent toggle)
   - **retention keep-last** (number; only meaningful for Fresh)
3. On submit, map `NewSessionData` + schedule fields → an `Automation` with its
   `LaunchSpec`, persist via the store, reload, return to List with the new row
   selected.

The `NewSessionData → LaunchSpec` mapping is a straightforward field copy,
analogous to the existing `NewSessionData → InstanceParams` mapping in
`create_session`. It lives in one tested function.

### 6. Edit

`e` on a selected automation opens the **Schedule dialog pre-filled** with the
automation's name/cron/mode/retention and `enabled` — the common edit. Submit
writes the changed fields back through the store and reloads.

Editing the **launch spec** itself re-opens the new-session wizard
**pre-populated** from the stored `LaunchSpec`. This needs a new
`NewSessionDialog::from_launch_spec(spec)` constructor (the wizard currently has
only a defaults-based `new()`); it is the **main integration risk** and is
isolated in its own implementation step with a round-trip test
(`spec → dialog → NewSessionData → spec` is stable for the fields an automation
carries).

## Data freshness

The daemon updates `next_fire` / `last_run` in `automations.json` out of band.
The view reloads the store on open and after every mutation, so user actions
always reflect fresh state. A passive reload on the app's existing redraw tick
keeps `next_fire`/`last_run` reasonably current while the view is open; this is
a cheap file read and is best-effort (a failed reload keeps the last snapshot
and is not fatal).

## Error handling

- Store load failure on open: info dialog, view does not open.
- Store mutation failure (toggle/delete/add/edit): inline error banner, state
  unchanged.
- Invalid cron expression in the Schedule dialog: inline error, submit blocked.
- Run-now dispatch failure: inline error banner.

## Testing

- **Unit (in-module):**
  - List navigation and mode transitions (List ↔ ConfirmDelete ↔ Schedule).
  - `NewSessionData` + schedule fields → `Automation` mapping.
  - Schedule-dialog cron validation (valid accepts, invalid blocks).
  - `NewSessionDialog::from_launch_spec` round-trip stability.
- **e2e (tmux, `TuiTestHarness`):**
  - Launch the TUI with a pre-seeded `automations.json`, press `a` to open the
    view, and assert the list renders the seeded automation (name, cron,
    next_fire). (`a` from the home screen opens the view; `a` *inside* the view
    starts the add flow — same key, different context.)
  - Drive enable/disable toggle and assert the glyph/state flips and the store
    is updated.
  - Walk the add flow far enough to assert the Schedule dialog appears after the
    wizard and that a completed add produces a new store row.

  This e2e is a first-class deliverable: it is the in-tmux verification of the
  view.

## Files touched

New: `src/tui/automations/{mod,render,input}.rs`.

Modified:
- `src/tui/mod.rs` — declare the module.
- `src/tui/home/mod.rs` — `automations_view: Option<AutomationsView>` and the
  `NewSessionPurpose` flag.
- `src/tui/home/bindings.rs` — `ActionId::Automations`, binding, palette, help.
- `src/tui/home/render.rs` — takeover render branch.
- `src/tui/home/input.rs` — takeover input branch + open action + wizard-submit
  routing on the automation purpose.
- `src/tui/home/operations.rs` — automation-create/edit from `NewSessionData`
  (sibling to `create_session`).
- `src/tui/dialogs/new_session/mod.rs` — `from_launch_spec` constructor.
- `tests/e2e/` — new automations TUI e2e + registration in `tests/e2e/main.rs`.

## Scope boundary

Plan 2 of 3. Plan 3 adds the dashboard panel + REST API and the consent-gated
OS-service install. No engine, store, or CLI changes here.
