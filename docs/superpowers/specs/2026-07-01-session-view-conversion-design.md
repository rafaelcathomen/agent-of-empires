# Web session view conversion

## Goal

Make the existing structured-view conversion endpoints discoverable in the web
dashboard. A user can convert an eligible terminal session to the structured
view, and can return a structured session to a terminal session with an
explicit warning about history loss.

## Scope

The session-row context menu gains two actions:

- A terminal session with `acp_capable: true` shows **Convert to structured
  view**.
- A structured session shows **Convert to terminal**.

The terminal-to-structured action calls the existing
`POST /api/sessions/{id}/acp/enable` endpoint. It is not shown for agents that
cannot speak ACP. The server retains its existing behavior: Codex and Claude
resume and import their terminal conversation when identity is available; other
supported agents begin a fresh structured conversation.

The structured-to-terminal action first opens a confirmation modal. Its copy
makes the destructive behavior clear: the structured transcript is deleted and
the replacement terminal session starts fresh. Confirmation calls the existing
`POST /api/sessions/{id}/acp/disable` endpoint; cancellation makes no request.

## UI behavior

Actions appear beside the existing session actions in `WorkspaceSidebar` and
are hidden in read-only mode. They operate on the row's concrete session rather
than an arbitrary sibling in a multi-session workspace.

During either request, the menu closes. A successful response triggers the
existing session refresh path so the main pane follows the server-provided
`view`. Failed responses surface the server message through the established
error reporting path and leave the current session unchanged.

## API boundary

The UI uses small typed helpers in `web/src/lib/api.ts` for enable and disable,
returning the existing `{ session_id, view }` payload. The frontend does not
duplicate eligibility validation. The server remains authoritative for agent
resolution, writable state, terminal teardown, transcript preservation, and
history deletion.

## Tests

Add component tests around the session-row context menu:

- Eligible terminal sessions expose the structured conversion action and it
  sends the enable request.
- Non-ACP terminal sessions do not expose it.
- Structured sessions expose the terminal conversion action.
- The terminal conversion confirmation sends disable only after confirmation;
  cancelling sends no request.
- Conversion errors are reported and do not optimistically change the row.

Update `web/tests/coverage-matrix.json` for the dashboard session conversion
flow.

## Non-goals

- No new server endpoint or session storage format.
- No conversion control in the native TUI or CLI.
- No attempt to preserve structured history when returning to terminal view;
  the existing server contract deletes it.
