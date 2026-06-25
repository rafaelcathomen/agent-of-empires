# Scheduled runs auto-approve and default to the sandbox

A scheduled run has no human present, so it defaults to auto-approve (`yolo_mode`)
to avoid parking forever in `Waiting` on the first tool-use approval. To contain the
blast radius of an autonomous, recurring agent (e.g. a prompt-injected Slack message
steering it), runs default to AoE's Docker sandbox where available. The job
configuration must surface "runs unattended with auto-approve" loudly. A per-job
opt-out to non-auto-approve is allowed for trusted read-only jobs.

We rejected "no auto-approve; park and notify" (defeats unattended operation, the whole
point) and "auto-approve with no containment" (no guardrail against a hostile input
steering an auto-approving agent).

## Remote-execution caveat

This posture assumed a local machine. When a run's execution location is a remote host
(future, see ADR-0004), Docker may be absent so the sandbox default may not apply, and
auto-approving an autonomous agent on someone else's machine is a sharper edge. Remote
automations should still prefer the sandbox where available, and the unattended
auto-approve warning matters more, not less.
