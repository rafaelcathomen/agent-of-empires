---
name: talk-to-agent
description: Ask another agent in the Agent-of-Empires (AoE) fleet a question, or broadcast to a whole folder/group, and get replies back. Use when asked to "ask/tell/talk to <session>", consult another agent, poll a group of agents, or coordinate/relay work across AoE sessions. You drive (own) the discussion.
---

# Talk to another AoE agent: `agent-chat`

Other agents on this machine run as independent, long-lived AoE sessions.
`agent-chat` lets you ask one (or a whole folder) a question and get a
**structured reply back through a shared SQLite store**, using `aoe send` as the
"doorbell" that wakes the recipient so it actually answers. (No screen-scraping;
the recipient writes its reply via `agent-chat reply`.)

## Ask one agent

```bash
agent-chat ask "<addr>" "<question>" [--timeout 120] [--json]
```

Blocks until the recipient replies or times out. `<addr>` is a session **id**, a
**full folder path** like `general/skills/skill-pi`, a path **suffix**, or a
**unique title**. A title that exists in more than one folder errors and lists
the candidates; pass the full folder path or the id to disambiguate.

## Broadcast to a whole group (concurrent fan-out)

```bash
agent-chat broadcast "<folder-path>" "<question>" [--timeout 120] [--json]
```

Asks every session under that folder subtree **in parallel** and collects all
replies (wall-clock = slowest agent, not the sum). Example:
`agent-chat broadcast work/agile "what's your current blocker?"`.

## When YOU are the recipient

If you receive a turn beginning with `[agent-chat]`, another agent is asking you
a question and the message shows the exact command to answer with. Reply by
running it:

```bash
agent-chat reply <msg_id> "<your answer>"
```

Anytime: `agent-chat inbox` (open questions to you) · `agent-chat replies`
(replies to questions you asked) · `agent-chat thread <id>` · `agent-chat whoami`.

## You own the discussion

The asking agent holds the thread. A multi-turn or multi-party discussion is a
loop you drive: **ask → read reply → decide → ask again**, or relay one agent's
answer to another. Threads and history persist in the store
(`agent-chat thread <id>`), so nested relays are reconstructable. Keep a short
running summary so the discussion stays coherent and goal-directed; end it when
you have what you need.

## Notes & limits

- **Profile** defaults to `main`. **Identity** is auto-detected via
  `aoe session current`; headless/non-aoe callers set `AGENT_CHAT_ID='id:title'`
  or pass `--from 'id:title'`.
- `--json` gives machine-parseable output. **Exit codes:** `0` answered ·
  `3` no answer (pending/skipped) · `1` error. Branch on the code, not stdout.
- A timed-out `ask`/`broadcast` isn't lost: the reply is delivered later via a
  doorbell and is retrievable with `agent-chat replies`.
- Recipients must be live AoE agents that can run `agent-chat reply` (the
  doorbell text is self-describing, so no prior setup is required). For an agent
  that genuinely can't cooperate (paneless/non-Claude), the legacy scrape tools
  `aoe-ask` / `aoe-a2a` remain as a last-resort fallback.
