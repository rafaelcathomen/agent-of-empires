# Web Session View Conversion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add safe web-dashboard actions to convert eligible terminal sessions to structured view and structured sessions back to terminal view.

**Architecture:** Reuse the existing Axum enable and disable endpoints. Add typed frontend request helpers, context-menu gates in `WorkspaceSidebar`, and a focused confirmation dialog for the destructive structured-to-terminal path.

**Tech Stack:** React 19, TypeScript, Vitest with React Testing Library, Axum ACP endpoints.

## Global Constraints

- Use `POST /api/sessions/{id}/acp/enable` and `POST /api/sessions/{id}/acp/disable`; do not add server routes.
- Show terminal-to-structured conversion only for `view === "terminal"` and `acp_capable === true`.
- Hide both actions in read-only mode.
- Require confirmation before disable because the existing server endpoint deletes structured history and starts a fresh tmux session.
- Update the web coverage matrix.

---

## File Structure

- `web/src/lib/api.ts`: response and failure types plus enable and disable helpers.
- `web/src/lib/api.acp.test.ts`: request and error behavior for the helpers.
- `web/src/components/SessionViewConversionDialog.tsx`: destructive confirmation dialog.
- `web/src/components/__tests__/SessionViewConversionDialog.test.tsx`: dialog behavior tests.
- `web/src/components/WorkspaceSidebar.tsx`: menu items, conversion handlers, and dialog state.
- `web/src/components/__tests__/SessionRowTriage.test.tsx`: sidebar visibility and request behavior tests.
- `web/tests/coverage-matrix.json`: lifecycle coverage entry.

### Task 1: Typed view-switch API helpers

**Files:**

- Modify: `web/src/lib/api.ts:1266-1330`
- Modify: `web/src/lib/api.acp.test.ts:1-130`

**Interfaces:**

- Produces `ViewSwitchResponse`, `ViewSwitchResult`, `enableStructuredView(sessionId)`, and `disableStructuredView(sessionId)`.
- Sidebar handlers consume `ViewSwitchResult` and report the `message` on failure.

- [ ] **Step 1: Write failing helper tests**

```ts
it("enables structured view with an encoded session id", async () => {
  (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
    ok({ session_id: "weird/id", view: "structured" }),
  );

  await expect(enableStructuredView("weird/id")).resolves.toEqual({
    ok: true,
    response: { session_id: "weird/id", view: "structured" },
  });
  const [url, init] = (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls[0];
  expect(String(url)).toContain("/api/sessions/weird%2Fid/acp/enable");
  expect((init as RequestInit).method).toBe("POST");
});

it("returns a server message when view conversion is rejected", async () => {
  (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
    new Response("no structured view agent registered", { status: 400 }),
  );
  await expect(enableStructuredView("s-1")).resolves.toEqual({
    ok: false,
    message: "no structured view agent registered",
  });
});
```

Add an analogous disable test for `/acp/disable` and `view: "terminal"`.

- [ ] **Step 2: Run the focused test and confirm RED**

Run: `cd web && npm run test:unit -- src/lib/api.acp.test.ts --run`

Expected: failure because the two helpers are not exported.

- [ ] **Step 3: Implement the minimal helper contract**

```ts
export interface ViewSwitchResponse {
  session_id: string;
  view: "structured" | "terminal";
}

export type ViewSwitchResult =
  | { ok: true; response: ViewSwitchResponse }
  | { ok: false; message: string };

async function switchSessionView(sessionId: string, target: "enable" | "disable"): Promise<ViewSwitchResult> {
  try {
    const res = await fetch(`/api/sessions/${encodeURIComponent(sessionId)}/acp/${target}`, { method: "POST" });
    if (!res.ok) return { ok: false, message: (await res.text()) || `Server error (${res.status})` };
    return { ok: true, response: (await res.json()) as ViewSwitchResponse };
  } catch {
    return { ok: false, message: "Could not change this session view. Please try again." };
  }
}

export const enableStructuredView = (sessionId: string) => switchSessionView(sessionId, "enable");
export const disableStructuredView = (sessionId: string) => switchSessionView(sessionId, "disable");
```

- [ ] **Step 4: Run the focused API tests and confirm GREEN**

Run: `cd web && npm run test:unit -- src/lib/api.acp.test.ts --run`

Expected: all `api.acp` tests pass.

- [ ] **Step 5: Commit the API contract**

```bash
git add web/src/lib/api.ts web/src/lib/api.acp.test.ts
git commit -m "feat: add web session view conversion API helpers"
```

### Task 2: Destructive terminal-conversion dialog

**Files:**

- Create: `web/src/components/SessionViewConversionDialog.tsx`
- Create: `web/src/components/__tests__/SessionViewConversionDialog.test.tsx`

**Interfaces:**

- Consumes `sessionTitle`, `onConfirm: () => Promise<boolean>`, and `onCancel`.
- Produces `role="dialog"` with `data-testid="session-view-conversion-dialog"`.
- `WorkspaceSidebar` shows it only before structured-to-terminal conversion.

- [ ] **Step 1: Write the failing dialog tests**

```tsx
it("does not convert when Cancel is clicked", () => {
  const onConfirm = vi.fn().mockResolvedValue(true);
  render(<SessionViewConversionDialog sessionTitle="codex" onConfirm={onConfirm} onCancel={() => {}} />);
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(onConfirm).not.toHaveBeenCalled();
});

it("warns about transcript deletion and confirms once", async () => {
  const onConfirm = vi.fn().mockResolvedValue(true);
  render(<SessionViewConversionDialog sessionTitle="codex" onConfirm={onConfirm} onCancel={() => {}} />);
  expect(screen.getByText(/structured transcript will be deleted/i)).not.toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Convert to terminal" }));
  await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledTimes(1));
});
```

Cover Escape cancellation and a failed confirmation that keeps the dialog open and re-enables controls.

- [ ] **Step 2: Run the dialog test and confirm RED**

Run: `cd web && npm run test:unit -- src/components/__tests__/SessionViewConversionDialog.test.tsx --run`

Expected: module-not-found failure for `SessionViewConversionDialog`.

- [ ] **Step 3: Implement the accessible confirmation dialog**

Follow `StopSessionDialog` for focus restoration, Enter, Escape, backdrop, and in-flight behavior. Use this signature:

```tsx
interface Props {
  sessionTitle: string;
  onConfirm: () => Promise<boolean>;
  onCancel: () => void;
}
```

The body must say that the structured transcript is deleted, the previous terminal is not restored, and the new terminal starts fresh. Only call `onCancel` after `onConfirm()` resolves `true`.

- [ ] **Step 4: Run the focused dialog tests and confirm GREEN**

Run: `cd web && npm run test:unit -- src/components/__tests__/SessionViewConversionDialog.test.tsx --run`

Expected: all dialog tests pass.

- [ ] **Step 5: Commit the dialog**

```bash
git add web/src/components/SessionViewConversionDialog.tsx web/src/components/__tests__/SessionViewConversionDialog.test.tsx
git commit -m "feat: confirm structured session terminal conversion"
```

### Task 3: Sidebar actions and coverage registration

**Files:**

- Modify: `web/src/components/WorkspaceSidebar.tsx:1-110, 930-1085, 1485-1560, 1700-1725`
- Modify: `web/src/components/__tests__/SessionRowTriage.test.tsx:315-560`
- Modify: `web/tests/coverage-matrix.json:1105-1112`

**Interfaces:**

- Produces menu test IDs `sidebar-context-menu-enable-structured` and `sidebar-context-menu-disable-structured`.
- Consumes the Task 1 helpers and Task 2 dialog.

- [ ] **Step 1: Write failing sidebar tests**

```tsx
it("shows structured conversion only for ACP-capable terminal sessions", () => {
  render(<Wrap><Row ws={workspace("w", [session({ view: "terminal", acp_capable: true })])} /></Wrap>);
  fireEvent.contextMenu(screen.getByTestId("sidebar-session-row"));
  expect(screen.queryByTestId("sidebar-context-menu-enable-structured")).not.toBeNull();
});

it("hides structured conversion for a terminal session without ACP support", () => {
  render(<Wrap><Row ws={workspace("w", [session({ view: "terminal", acp_capable: false })])} /></Wrap>);
  fireEvent.contextMenu(screen.getByTestId("sidebar-session-row"));
  expect(screen.queryByTestId("sidebar-context-menu-enable-structured")).toBeNull();
});

it("opens confirmation before converting structured view to terminal", () => {
  render(<Wrap><Row ws={workspace("w", [session({ view: "structured" })])} /></Wrap>);
  fireEvent.contextMenu(screen.getByTestId("sidebar-session-row"));
  fireEvent.click(screen.getByTestId("sidebar-context-menu-disable-structured"));
  expect(screen.queryByTestId("session-view-conversion-dialog")).not.toBeNull();
  expect(fetchSpy).not.toHaveBeenCalled();
});
```

Add POST assertions for enable, confirmed disable, and read-only hiding of both actions.

- [ ] **Step 2: Run the sidebar test and confirm RED**

Run: `cd web && npm run test:unit -- src/components/__tests__/SessionRowTriage.test.tsx --run`

Expected: missing menu actions and dialog assertions fail.

- [ ] **Step 3: Implement menu gates and handlers**

Import the helpers and dialog. Use state and handlers with this behavior:

```tsx
const [terminalConversion, setTerminalConversion] = useState<{ id: string; title: string } | null>(null);

const handleEnableStructured = async () => {
  setContextMenu(null);
  if (!sessionId) return;
  const result = await enableStructuredView(sessionId);
  if (!result.ok) reportError(result.message);
};

const confirmDisableStructured = async (): Promise<boolean> => {
  if (!terminalConversion) return false;
  const result = await disableStructuredView(terminalConversion.id);
  if (!result.ok) {
    reportError(result.message);
    return false;
  }
  setTerminalConversion(null);
  return true;
};
```

Show enable only for a writable ACP-capable terminal row. Show disable only for a writable structured row; close the menu then set `terminalConversion`, and mount the dialog through the existing portal pattern. Do not optimistically rewrite `session.view`; the normal session refresh observes the server result.

- [ ] **Step 4: Add coverage matrix entry**

Extend `session.lifecycle` with `src/components/__tests__/SessionRowTriage.test.tsx`, `src/components/__tests__/SessionViewConversionDialog.test.tsx`, and `src/lib/api.acp.test.ts`. Add `web/src/components/SessionViewConversionDialog.tsx` to the entry’s component list.

- [ ] **Step 5: Run focused verification and commit**

Run:

```bash
cd web
npm run test:unit -- src/components/__tests__/SessionRowTriage.test.tsx src/components/__tests__/SessionViewConversionDialog.test.tsx src/lib/api.acp.test.ts --run
node tests/validate-coverage-matrix.mjs
```

Expected: all focused tests and matrix validation pass.

```bash
git add web/src/components/WorkspaceSidebar.tsx web/src/components/__tests__/SessionRowTriage.test.tsx web/tests/coverage-matrix.json
git commit -m "feat: add web session view conversion actions"
```

### Task 4: Full verification

**Files:** Verify only.

- [ ] **Step 1: Run all web quality gates**

```bash
cd web
npm run format:check
npm run lint
npx tsc -b
npm run test:unit -- --run
node tests/validate-coverage-matrix.mjs
```

Expected: each command exits 0.

- [ ] **Step 2: Run repository checks for the retained conversion regression**

```bash
cargo fmt --all -- --check
cargo test --features serve --test e2e codex_conversion_e2e -- --nocapture
git diff --check origin/main...HEAD
git status --short
```

Expected: all commands exit 0 and no unintended files are present.
