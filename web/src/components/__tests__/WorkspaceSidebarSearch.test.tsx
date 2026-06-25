// @vitest-environment jsdom
//
// RTL coverage for the inline sidebar session search (Ctrl/Cmd+F). The pure
// matcher/highlighter math is locked in `lib/__tests__/sessionSearch.test.ts`;
// this exercises the controlled component wiring: live in-place filtering
// (including a group_path match), auto-expanding a collapsed matching group,
// the matched-run <mark>, Up/Down/Enter keyboard nav through the filtered
// rows, the clear (x) button, and the empty state. The search is driven
// controlled (searchOpen/searchQuery/onSearchQueryChange) exactly as App.tsx
// drives it, with a wrapper holding the query state.

import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";

import { WorkspaceSidebar } from "../WorkspaceSidebar";
import type { SidebarGroup, SidebarWorkspaceView } from "../../lib/sidebarGroups";
import type { SessionResponse, Workspace } from "../../lib/types";

function session(over: Partial<SessionResponse> = {}): SessionResponse {
  return {
    id: "s1",
    title: "My Session",
    project_path: "/work/orion",
    group_path: "",
    tool: "claude",
    status: "Idle",
    yolo_mode: false,
    created_at: "2025-01-01T00:00:00Z",
    last_accessed_at: null,
    idle_entered_at: null,
    last_error: null,
    branch: null,
    main_repo_path: null,
    is_sandboxed: false,
    favorited: false,
    has_managed_worktree: false,
    has_terminal: true,
    profile: "default",
    cleanup_defaults: { delete_worktree: false, delete_branch: false, delete_sandbox: false },
    remote_owner: null,
    notify_on_waiting: null,
    notify_on_idle: null,
    notify_on_error: null,
    claude_fullscreen: false,
    workspace_repos: [],
    scratch: false,
    ...over,
  };
}

function workspace(id: string, over: Partial<Workspace> = {}): Workspace {
  // Each workspace gets a project path unique to its id so the derived
  // repo-name match does not bleed one fixture's id into another's path.
  const projectPath = `/work/${id}`;
  return {
    id,
    branch: null,
    projectPath,
    displayName: id,
    agents: ["claude"],
    primaryAgent: "claude",
    status: "idle",
    sessions: [session({ id: `${id}-s`, title: id, project_path: projectPath })],
    ...over,
  };
}

function view(ws: Workspace): SidebarWorkspaceView {
  return { key: ws.id, workspace: ws };
}

function group(over: Partial<SidebarGroup> = {}): SidebarGroup {
  return {
    id: "g1",
    kind: "sessionGroup",
    displayName: "Group One",
    defaultDisplayName: "Group One",
    alias: null,
    color: null,
    remoteOwner: null,
    workspaces: [],
    status: "idle",
    collapsed: false,
    capabilities: { appearance: false, reorder: false, create: "generic" },
    groupPath: "g1",
    registeredProjects: [],
    pinned: false,
    pinnedEmpty: false,
    ...over,
  };
}

// A wrapper that owns the search query, mirroring how App.tsx controls the
// sidebar search, so typing updates state and re-renders the filtered list.
function Harness({ groups, onSelect }: { groups: SidebarGroup[]; onSelect: (id: string) => void }) {
  const [searchQuery, setSearchQuery] = useState("");
  return (
    <WorkspaceSidebar
      groups={groups}
      nestedGroups={[]}
      onToggleSubgroup={vi.fn()}
      onReorderWorkspaces={vi.fn()}
      onReorderGroups={vi.fn()}
      activeId={null}
      open
      onToggle={vi.fn()}
      onSelect={onSelect}
      onToggleGroup={vi.fn()}
      onUpdateRepoAppearance={vi.fn()}
      onNew={vi.fn()}
      onCreateSession={vi.fn()}
      onPinProject={vi.fn()}
      onUnpinProject={vi.fn()}
      savedProjects={[]}
      onAddProject={vi.fn()}
      onEditProject={vi.fn()}
      onRemoveProject={vi.fn()}
      onSettings={vi.fn()}
      onDeleteSession={vi.fn()}
      onStopSession={vi.fn()}
      onStartSession={vi.fn()}
      sortMode="manual"
      onSortModeChange={vi.fn()}
      axis="group"
      onAxisChange={vi.fn()}
      searchOpen
      searchQuery={searchQuery}
      onSearchQueryChange={setSearchQuery}
      onSearchClose={vi.fn()}
      onSearchOpen={vi.fn()}
    />
  );
}

function input() {
  return screen.getByTestId("sidebar-filter-input") as HTMLInputElement;
}

afterEach(() => {
  cleanup();
});

describe("WorkspaceSidebar inline session search", () => {
  it("filters rows in place as the query changes", () => {
    const groups = [group({ workspaces: [view(workspace("Orion")), view(workspace("Zeta"))] })];
    render(<Harness groups={groups} onSelect={vi.fn()} />);
    expect(screen.getAllByTestId("sidebar-session-row")).toHaveLength(2);

    fireEvent.change(input(), { target: { value: "orion" } });
    const rows = screen.getAllByTestId("sidebar-session-row");
    expect(rows).toHaveLength(1);
    expect(rows[0]!.textContent).toContain("Orion");
  });

  it("matches on a session group_path", () => {
    const ws = workspace("alpha", { sessions: [session({ id: "a-s", title: "alpha", group_path: "refactor" })] });
    const groups = [group({ workspaces: [view(ws), view(workspace("beta"))] })];
    render(<Harness groups={groups} onSelect={vi.fn()} />);

    fireEvent.change(input(), { target: { value: "refactor" } });
    const rows = screen.getAllByTestId("sidebar-session-row");
    expect(rows).toHaveLength(1);
    expect(rows[0]!.textContent).toContain("alpha");
  });

  it("auto-expands a collapsed matching group and hides non-matching ones", () => {
    const groups = [
      group({ id: "g1", displayName: "Group One", collapsed: true, workspaces: [view(workspace("Orion"))] }),
      group({ id: "g2", displayName: "Group Two", collapsed: false, workspaces: [view(workspace("Zeta"))] }),
    ];
    render(<Harness groups={groups} onSelect={vi.fn()} />);

    // Collapsed group hides its row until a query forces it open.
    expect(screen.queryByText("Orion")).toBeNull();

    fireEvent.change(input(), { target: { value: "orion" } });
    const rows = screen.getAllByTestId("sidebar-session-row");
    expect(rows).toHaveLength(1);
    expect(rows[0]!.textContent).toContain("Orion");
    expect(screen.queryByText("Zeta")).toBeNull();
  });

  it("wraps the matched run of the title in a <mark>", () => {
    const groups = [group({ workspaces: [view(workspace("Orion"))] })];
    render(<Harness groups={groups} onSelect={vi.fn()} />);

    fireEvent.change(input(), { target: { value: "rio" } });
    const mark = screen.getByTestId("session-search-match");
    expect(mark.tagName).toBe("MARK");
    expect(mark.textContent).toBe("rio");
  });

  it("navigates with ArrowDown then Enter, opening the highlighted row", () => {
    const onSelect = vi.fn();
    const groups = [group({ workspaces: [view(workspace("task-one")), view(workspace("task-two"))] })];
    render(<Harness groups={groups} onSelect={onSelect} />);

    fireEvent.change(input(), { target: { value: "task" } });
    const rows = () => screen.getAllByTestId("sidebar-session-row");
    expect(rows()).toHaveLength(2);

    // First row starts highlighted; ArrowDown moves to the second.
    fireEvent.keyDown(input(), { key: "ArrowDown" });
    fireEvent.keyDown(input(), { key: "Enter" });
    expect(onSelect).toHaveBeenCalledWith("task-two");

    // ArrowUp back to the first, then Enter.
    onSelect.mockClear();
    fireEvent.keyDown(input(), { key: "ArrowUp" });
    fireEvent.keyDown(input(), { key: "Enter" });
    expect(onSelect).toHaveBeenCalledWith("task-one");
  });

  it("ignores Enter while an IME composition is in progress", () => {
    const onSelect = vi.fn();
    const groups = [group({ workspaces: [view(workspace("task-one")), view(workspace("task-two"))] })];
    render(<Harness groups={groups} onSelect={onSelect} />);

    fireEvent.change(input(), { target: { value: "task" } });
    expect(screen.getAllByTestId("sidebar-session-row")).toHaveLength(2);

    // Enter during composition commits the IME candidate; it must not open
    // the highlighted session.
    fireEvent.keyDown(input(), { key: "Enter", isComposing: true });
    expect(onSelect).not.toHaveBeenCalled();

    // A committed Enter (no composition) opens the highlighted row.
    fireEvent.keyDown(input(), { key: "Enter" });
    expect(onSelect).toHaveBeenCalledWith("task-one");
  });

  it("marks the keyboard-highlighted row via aria-activedescendant", () => {
    const groups = [group({ workspaces: [view(workspace("task-one")), view(workspace("task-two"))] })];
    render(<Harness groups={groups} onSelect={vi.fn()} />);

    fireEvent.change(input(), { target: { value: "task" } });
    expect(input().getAttribute("aria-activedescendant")).toBe("sidebar-row-task-one");

    fireEvent.keyDown(input(), { key: "ArrowDown" });
    expect(input().getAttribute("aria-activedescendant")).toBe("sidebar-row-task-two");
    expect(document.getElementById("sidebar-row-task-two")?.getAttribute("aria-selected")).toBe("true");
  });

  it("clears the query with the x button but keeps the input visible", () => {
    const groups = [group({ workspaces: [view(workspace("Orion")), view(workspace("Zeta"))] })];
    render(<Harness groups={groups} onSelect={vi.fn()} />);

    fireEvent.change(input(), { target: { value: "orion" } });
    expect(screen.getAllByTestId("sidebar-session-row")).toHaveLength(1);

    fireEvent.click(screen.getByTestId("sidebar-search-clear"));
    expect(input().value).toBe("");
    expect(screen.queryByTestId("sidebar-filter-input")).not.toBeNull();
    expect(screen.getAllByTestId("sidebar-session-row")).toHaveLength(2);
  });

  it("shows the empty state when nothing matches", () => {
    const groups = [group({ workspaces: [view(workspace("Orion"))] })];
    render(<Harness groups={groups} onSelect={vi.fn()} />);

    fireEvent.change(input(), { target: { value: "zzzzzz" } });
    const empty = screen.getByTestId("sidebar-no-matches");
    expect(empty.textContent).toContain("No matching sessions");
    expect(screen.queryByTestId("sidebar-session-row")).toBeNull();
  });
});
