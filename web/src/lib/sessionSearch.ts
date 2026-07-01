import type { SessionResponse, Workspace } from "./types";

// A contiguous matched span within a source string, as [startInclusive,
// endExclusive] character offsets. Returned by `highlightRanges` so the
// row renderer can wrap the matched substring without re-running the match.
export interface HighlightRange {
  start: number;
  end: number;
}

// Normalize a raw query the same way every call site must: trim outer
// whitespace and lowercase. Centralized so the matcher and the highlighter
// agree on what "the query" is. An all-whitespace query normalizes to "",
// which callers treat as "no active filter".
export function normalizeQuery(query: string): string {
  return query.trim().toLowerCase();
}

// Case-insensitive substring test. `haystack` is matched against an
// already-normalized (trimmed + lowercased) `needle`; pass the output of
// `normalizeQuery`. A null/undefined haystack never matches.
export function matchesText(haystack: string | null | undefined, needle: string): boolean {
  if (needle === "") return true;
  if (haystack == null) return false;
  return haystack.toLowerCase().includes(needle);
}

// Derive the project/repo display name from a workspace's `projectPath`,
// mirroring useWorkspaces (`projectPath.split("/").pop()`). Kept here so the
// matcher can search the repo name even when `displayName` is a branch or a
// session title rather than the repo name.
export function projectNameOf(projectPath: string): string {
  return projectPath.split("/").pop() ?? projectPath;
}

// Every searchable string for one session: title, branch, tool, group_path.
function sessionSearchFields(session: SessionResponse): (string | null)[] {
  return [session.title, session.branch, session.tool, session.group_path];
}

// True when a workspace matches the normalized query across any of its own
// fields (displayName, projectPath, branch, agents/tools, derived project
// name) or any of its sessions' fields (title, branch, tool, group_path).
// `needle` MUST be pre-normalized via `normalizeQuery`.
export function workspaceMatches(ws: Workspace, needle: string): boolean {
  if (needle === "") return true;
  if (
    matchesText(ws.displayName, needle) ||
    matchesText(ws.projectPath, needle) ||
    matchesText(projectNameOf(ws.projectPath), needle) ||
    matchesText(ws.branch, needle)
  ) {
    return true;
  }
  if (ws.agents.some((a) => matchesText(a, needle))) return true;
  return ws.sessions.some((s) => sessionSearchFields(s).some((f) => matchesText(f, needle)));
}

// Compute the matched spans of `query` within `text` for highlighting. Uses
// case-insensitive substring matching and returns ALL non-overlapping
// occurrences left to right (so a repeated token highlights every hit).
// Offsets index into the ORIGINAL `text` so the renderer slices the
// original-cased characters. Empty query or no match returns []. The
// renderer is expected to wrap each [start,end) span and leave the gaps
// plain; ranges never overlap and are sorted ascending.
export function highlightRanges(text: string, query: string): HighlightRange[] {
  const needle = normalizeQuery(query);
  if (needle === "" || text === "") return [];
  const haystack = text.toLowerCase();
  const ranges: HighlightRange[] = [];
  let from = 0;
  for (;;) {
    const idx = haystack.indexOf(needle, from);
    if (idx === -1) break;
    ranges.push({ start: idx, end: idx + needle.length });
    from = idx + needle.length;
  }
  return ranges;
}
