# Agent of Empires

Glossary for AoE. Currently scoped to the Automations feature (scheduled and,
later, event-triggered agent runs). Definitions only — no implementation detail.

## Automations

**Automation**:
A saved entity pairing a trigger with a launch spec: "when this fires, launch a
session like this." The unit users create, list, enable, and delete.
_Avoid_: job, schedule, routine, task, cron job.

**Trigger**:
What causes an automation to fire. In v1 the only kind is a Cron Trigger; the
type is left open so event triggers can be added later.
_Avoid_: rule, condition.

**Cron Trigger**:
A trigger defined by a 5-field cron expression evaluated in the user's local
timezone, at one-minute granularity.

**Fire**:
The moment a trigger comes due and an automation is asked to run.
_Avoid_: tick, hit, trigger (as a verb).

**Run**:
One execution of an automation — a single launched-and-executed session.
An automation has many runs over its life.
_Avoid_: job, invocation, execution, instance.

**Launch Spec**:
The parameters a run uses to launch its session: project path, tool/agent, view,
worktree and sandbox options, execution location, and the initial prompt.
_Avoid_: config, template, recipe.

**Execution Location**:
Where a run's session actually executes: the local host, a local Docker sandbox, or
(future) a remote host over SSH. An axis of the Launch Spec, independent of where the
Scheduler Daemon runs. Note: distinct from "remote" elsewhere in AoE, which means
remote *dashboard access* (tunnel/tailscale), not remote *execution*.
_Avoid_: using bare "remote" for execution; say "remote host" / "execution location".

**Initial Prompt**:
The prompt injected into a run's session at the start of the run. The new
primitive that lets a session launch already carrying work.

**Session Mode**:
Per-automation choice of how a run gets its session. **Fresh**: each run spawns a
new session, archived (keep-last-N) when complete. **Persistent**: all runs reuse
one long-lived session so context carries across runs.
_Avoid_: ephemeral/sticky, oneshot/continuous.

**Scheduler Daemon**:
The AoE daemon process that hosts trigger evaluation and dispatches runs. Runs as
an OS-registered service when installed; auto-spawned as a fallback otherwise.
_Avoid_: scheduler, cron daemon, worker.

**Retention**:
The keep-last-N policy that auto-archives and prunes old Fresh-mode run sessions
(and their worktrees) so runs don't accumulate.
