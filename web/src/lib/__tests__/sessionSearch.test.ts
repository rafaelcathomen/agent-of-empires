// @vitest-environment node
//
// Unit tests for the pure inline-sidebar-search helpers (sessionSearch.ts).
// These lock the field coverage the UX spec requires (title, branch, tool,
// group_path, project/repo name), case-insensitivity, the empty-query
// identity contract the sidebar relies on, and the highlight-range math the
// row renderer consumes.

import { describe, expect, it } from "vitest";

import {
  filterNestedSidebarGroups,
  filterSidebarGroups,
  highlightRanges,
  matchesText,
  normalizeQuery,
  projectNameOf,
  workspaceMatches,
} from "../sessionSearch";
import type { SidebarGroup, SidebarWorkspaceView, NestedSidebarGroup } from "../sidebarGroups";
import type { SessionResponse, Workspace } from "../types";

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

function workspace(over: Partial<Workspace> = {}): Workspace {
  return {
    id: "w1",
    branch: null,
    projectPath: "/work/orion",
    displayName: "My Session",
    agents: ["claude"],
    primaryAgent: "claude",
    status: "idle",
    sessions: [session()],
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
    displayName: "Feature Work",
    defaultDisplayName: "Feature Work",
    alias: null,
    color: null,
    remoteOwner: null,
    workspaces: [view(workspace())],
    status: "idle",
    collapsed: false,
    capabilities: { appearance: false, reorder: false, create: "generic" },
    groupPath: "feature",
    registeredProjects: [],
    pinned: false,
    pinnedEmpty: false,
    ...over,
  };
}

describe("normalizeQuery", () => {
  it("trims and lowercases; whitespace-only becomes empty", () => {
    expect(normalizeQuery("  Foo ")).toBe("foo");
    expect(normalizeQuery("   ")).toBe("");
  });
});

describe("matchesText", () => {
  it("is case-insensitive and null-safe; empty needle always matches", () => {
    expect(matchesText("Hello World", "world")).toBe(true);
    expect(matchesText("Hello", "xyz")).toBe(false);
    expect(matchesText(null, "x")).toBe(false);
    expect(matchesText(null, "")).toBe(true);
  });
});

describe("projectNameOf", () => {
  it("returns the trailing path segment", () => {
    expect(projectNameOf("/work/orion")).toBe("orion");
    expect(projectNameOf("orion")).toBe("orion");
  });
});

describe("workspaceMatches", () => {
  it("empty query matches every workspace", () => {
    expect(workspaceMatches(workspace(), "")).toBe(true);
  });
  it("matches on session title (case-insensitive)", () => {
    expect(workspaceMatches(workspace(), "my sess")).toBe(true);
  });
  it("matches on branch", () => {
    const ws = workspace({ branch: "feat/login", sessions: [session({ branch: "feat/login" })] });
    expect(workspaceMatches(ws, "login")).toBe(true);
  });
  it("matches on tool", () => {
    const ws = workspace({ agents: ["codex"], sessions: [session({ tool: "codex" })] });
    expect(workspaceMatches(ws, "codex")).toBe(true);
  });
  it("matches on group_path even when not in displayName", () => {
    const ws = workspace({ sessions: [session({ group_path: "refactor" })] });
    expect(workspaceMatches(ws, "refactor")).toBe(true);
  });
  it("matches on derived project/repo name", () => {
    const ws = workspace({ projectPath: "/work/orion", displayName: "feat/x" });
    expect(workspaceMatches(ws, "orion")).toBe(true);
  });
  it("returns false when nothing matches", () => {
    expect(workspaceMatches(workspace(), "zzz")).toBe(false);
  });
});

describe("filterSidebarGroups", () => {
  it("returns the same array reference for an empty/whitespace query (identity)", () => {
    const groups = [group()];
    expect(filterSidebarGroups(groups, "   ")).toBe(groups);
  });
  it("drops non-matching rows and then empty groups, without mutating input", () => {
    const keep = workspace({ id: "keep", displayName: "Orion task" });
    const drop = workspace({
      id: "drop",
      displayName: "Zeta task",
      projectPath: "/x/zeta",
      sessions: [session({ title: "Zeta task", project_path: "/x/zeta" })],
    });
    const g = group({ workspaces: [view(keep), view(drop)] });
    const out = filterSidebarGroups([g], "orion");
    expect(out).toHaveLength(1);
    expect(out[0]!.workspaces.map((v) => v.workspace.id)).toEqual(["keep"]);
    // purity: original group still has both rows
    expect(g.workspaces).toHaveLength(2);
  });
  it("keeps all rows of a group whose header name matches", () => {
    const g = group({
      displayName: "Feature Work",
      workspaces: [view(workspace({ id: "a", displayName: "nomatch", sessions: [session({ title: "nomatch" })] }))],
    });
    const out = filterSidebarGroups([g], "feature");
    expect(out[0]!.workspaces.map((v) => v.workspace.id)).toEqual(["a"]);
  });
  it("returns [] when nothing matches", () => {
    expect(filterSidebarGroups([group()], "zzzz")).toEqual([]);
  });
});

describe("filterNestedSidebarGroups", () => {
  function nested(): NestedSidebarGroup {
    return {
      repo: group({ id: "repo", kind: "repo", displayName: "orion" }),
      subgroups: [group({ id: "sg", displayName: "Feature Work", workspaces: [view(workspace({ id: "a" }))] })],
    };
  }
  it("identity on empty query", () => {
    const ng = [nested()];
    expect(filterNestedSidebarGroups(ng, "")).toBe(ng);
  });
  it("keeps a row when only the repo header matches; drops empty repos otherwise", () => {
    const out = filterNestedSidebarGroups([nested()], "orion");
    expect(out).toHaveLength(1);
    expect(out[0]!.subgroups[0]!.workspaces.map((v) => v.workspace.id)).toEqual(["a"]);
    expect(filterNestedSidebarGroups([nested()], "zzzz")).toEqual([]);
  });
});

describe("highlightRanges", () => {
  it("returns one range for a single case-insensitive hit, indexing original text", () => {
    expect(highlightRanges("My Orion Task", "orion")).toEqual([{ start: 3, end: 8 }]);
  });
  it("returns every non-overlapping occurrence left to right", () => {
    expect(highlightRanges("aXaXa", "a")).toEqual([
      { start: 0, end: 1 },
      { start: 2, end: 3 },
      { start: 4, end: 5 },
    ]);
  });
  it("returns [] for empty query, empty text, or no match", () => {
    expect(highlightRanges("abc", "")).toEqual([]);
    expect(highlightRanges("", "a")).toEqual([]);
    expect(highlightRanges("abc", "z")).toEqual([]);
  });
});
