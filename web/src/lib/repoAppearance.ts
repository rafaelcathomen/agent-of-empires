import type { CSSProperties } from "react";
import { safeGetItem, safeRemoveItem, safeSetItem } from "./safeStorage";

const STORAGE_KEY = "aoe-repo-appearance-v1";

export type RepoColor =
  | "red"
  | "orange"
  | "amber"
  | "yellow"
  | "lime"
  | "emerald"
  | "teal"
  | "cyan"
  | "sky"
  | "blue"
  | "indigo"
  | "violet"
  | "fuchsia"
  | "pink"
  | "rose"
  | "slate";

export interface RepoAppearance {
  alias?: string;
  color?: RepoColor;
}

export type RepoAppearanceUpdate = {
  alias?: string | null;
  color?: RepoColor | null;
};

export const REPO_COLOR_OPTIONS: Array<{
  id: RepoColor;
  label: string;
}> = [
  { id: "red", label: "Red" },
  { id: "orange", label: "Orange" },
  { id: "amber", label: "Amber" },
  { id: "yellow", label: "Yellow" },
  { id: "lime", label: "Lime" },
  { id: "emerald", label: "Emerald" },
  { id: "teal", label: "Teal" },
  { id: "cyan", label: "Cyan" },
  { id: "sky", label: "Sky" },
  { id: "blue", label: "Blue" },
  { id: "indigo", label: "Indigo" },
  { id: "violet", label: "Violet" },
  { id: "fuchsia", label: "Fuchsia" },
  { id: "pink", label: "Pink" },
  { id: "rose", label: "Rose" },
  { id: "slate", label: "Slate" },
];

// Resolved CSS color per palette entry. The original six keep their theme
// token (so existing folders look unchanged across themes); the added hues are
// pinned to the same tailwind hexes as the TUI's `FolderColor::rgb` so both
// clients render an identical wheel.
const REPO_COLOR_CSS: Record<RepoColor, string> = {
  red: "#f87171",
  orange: "#fb923c",
  amber: "var(--color-status-waiting)",
  yellow: "#facc15",
  lime: "#a3e635",
  emerald: "#34d399",
  teal: "var(--color-terminal-active)",
  cyan: "#22d3ee",
  sky: "var(--color-sandbox)",
  blue: "#60a5fa",
  indigo: "#818cf8",
  violet: "var(--color-diff-header)",
  fuchsia: "#e879f9",
  pink: "#f472b6",
  rose: "var(--color-status-error)",
  slate: "var(--color-surface-700)",
};

// Faint tinted background for a repo header / project row carrying a color.
export function repoColorStyle(color: RepoColor | null): CSSProperties | undefined {
  if (!color) return undefined;
  return {
    backgroundColor: `color-mix(in srgb, ${REPO_COLOR_CSS[color]} 14%, transparent)`,
  };
}

// Solid swatch for the color picker.
export function repoSwatchStyle(color: RepoColor): CSSProperties {
  return { backgroundColor: REPO_COLOR_CSS[color] };
}

const validColors = new Set(REPO_COLOR_OPTIONS.map((option) => option.id));

function normalizeAppearance(value: unknown): RepoAppearance | null {
  if (!value || typeof value !== "object") return null;
  const raw = value as { alias?: unknown; color?: unknown };
  const alias = typeof raw.alias === "string" ? raw.alias.trim() : "";
  const color =
    typeof raw.color === "string" && validColors.has(raw.color as RepoColor) ? (raw.color as RepoColor) : undefined;
  if (!alias && !color) return null;
  return {
    ...(alias ? { alias } : {}),
    ...(color ? { color } : {}),
  };
}

export function loadRepoAppearances(): Record<string, RepoAppearance> {
  const raw = safeGetItem(STORAGE_KEY);
  if (!raw) return {};
  try {
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return {};
    const entries = Object.entries(parsed)
      .map(([repoId, value]) => [repoId, normalizeAppearance(value)] as const)
      .filter((entry): entry is readonly [string, RepoAppearance] => entry[1] !== null);
    return Object.fromEntries(entries);
  } catch {
    return {};
  }
}

export function persistRepoAppearances(map: Record<string, RepoAppearance>): void {
  if (Object.keys(map).length === 0) {
    safeRemoveItem(STORAGE_KEY);
    return;
  }
  safeSetItem(STORAGE_KEY, JSON.stringify(map));
}

export function applyRepoAppearanceUpdate(
  current: Record<string, RepoAppearance>,
  repoId: string,
  update: RepoAppearanceUpdate,
): Record<string, RepoAppearance> {
  const nextForRepo: RepoAppearance = { ...(current[repoId] ?? {}) };
  if ("alias" in update) {
    const alias = update.alias?.trim() ?? "";
    if (alias) nextForRepo.alias = alias;
    else delete nextForRepo.alias;
  }
  if ("color" in update) {
    if (update.color && validColors.has(update.color)) nextForRepo.color = update.color;
    else delete nextForRepo.color;
  }

  const next = { ...current };
  if (nextForRepo.alias || nextForRepo.color) next[repoId] = nextForRepo;
  else delete next[repoId];
  return next;
}
