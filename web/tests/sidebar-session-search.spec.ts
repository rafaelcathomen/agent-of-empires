// Mocked-Playwright story for the inline sidebar session search
// (Ctrl/Cmd+F). Covers the browser-specific behaviors that Vitest cannot
// exercise: the capture-phase shortcut that overrides native find-in-page,
// real focus management, keyboard navigation through the filtered rows,
// Escape restoring the full list, the clear button, and the phone-only
// tap affordance on a coarse-pointer viewport.
//
// This suite is NOT part of the build-verification bar; it exists so the
// coverage-matrix validator finds the spec on disk and so the flow has a
// browser-level regression net. Run with the mocked config:
//   cd web && npx playwright test --config=playwright.config.ts sidebar-session-search

import { test, expect, devices } from "@playwright/test";
import type { Page } from "@playwright/test";
import { installSidebarMocks } from "./helpers/sidebarMocks";

function sessions() {
  return [
    { id: "s-orion", title: "Orion task", project_path: "/work/orion", branch: "feat/orion" },
    { id: "s-zeta", title: "Zeta task", project_path: "/work/zeta", branch: "feat/zeta" },
    { id: "s-nova", title: "Nova task", project_path: "/work/nova", branch: "feat/nova" },
  ];
}

const rows = (page: Page) => page.locator('[data-testid="sidebar-session-row"]');
const searchInput = (page: Page) => page.locator('[data-testid="sidebar-filter-input"]');

test("Ctrl/Cmd+F opens the search input and focuses it", async ({ page }) => {
  await installSidebarMocks(page, { sessions: sessions() });
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.goto("/");

  await expect(rows(page)).toHaveCount(3);
  await expect(searchInput(page)).toBeHidden();

  await page.keyboard.press("ControlOrMeta+f");
  await expect(searchInput(page)).toBeVisible();
  const focused = await page.evaluate(() => document.activeElement?.getAttribute("data-testid") ?? null);
  expect(focused).toBe("sidebar-filter-input");
});

test("typing filters to the matching rows in place", async ({ page }) => {
  await installSidebarMocks(page, { sessions: sessions() });
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.goto("/");

  await page.keyboard.press("ControlOrMeta+f");
  await searchInput(page).fill("orion");
  await expect(rows(page)).toHaveCount(1);
  await expect(rows(page).first()).toContainText("Orion");
  await expect(page.locator('[data-testid="session-search-match"]').first()).toHaveText(/orion/i);
});

test("a collapsed group's member surfaces without manual expand", async ({ page }) => {
  await installSidebarMocks(page, { sessions: sessions() });
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.goto("/");
  await expect(rows(page)).toHaveCount(3);

  // Collapse the first repo group, hiding its row, then prove search reveals
  // it regardless of the persisted collapse state.
  const firstHeader = page.locator('[data-testid="sidebar-group-header"]').first();
  await firstHeader.click();

  await page.keyboard.press("ControlOrMeta+f");
  await searchInput(page).fill("orion");
  await expect(rows(page)).toHaveCount(1);
  await expect(rows(page).first()).toContainText("Orion");
});

test("ArrowDown then Enter navigates to the highlighted session", async ({ page }) => {
  await installSidebarMocks(page, { sessions: sessions() });
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.goto("/");

  await page.keyboard.press("ControlOrMeta+f");
  await searchInput(page).fill("task");
  await expect(rows(page)).toHaveCount(3);

  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/\/session\//);
});

test("Escape closes search and restores the full list", async ({ page }) => {
  await installSidebarMocks(page, { sessions: sessions() });
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.goto("/");

  await page.keyboard.press("ControlOrMeta+f");
  await searchInput(page).fill("orion");
  await expect(rows(page)).toHaveCount(1);

  await page.keyboard.press("Escape");
  await expect(searchInput(page)).toBeHidden();
  await expect(rows(page)).toHaveCount(3);
});

test("the clear button empties the query but keeps search open", async ({ page }) => {
  await installSidebarMocks(page, { sessions: sessions() });
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.goto("/");

  await page.keyboard.press("ControlOrMeta+f");
  await searchInput(page).fill("orion");
  await expect(rows(page)).toHaveCount(1);

  await page.locator('[data-testid="sidebar-search-clear"]').click();
  await expect(searchInput(page)).toBeVisible();
  await expect(searchInput(page)).toHaveValue("");
  await expect(rows(page)).toHaveCount(3);
});

test("a non-matching query shows the empty state", async ({ page }) => {
  await installSidebarMocks(page, { sessions: sessions() });
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.goto("/");

  await page.keyboard.press("ControlOrMeta+f");
  await searchInput(page).fill("zzzzzz");
  await expect(page.locator('[data-testid="sidebar-no-matches"]')).toBeVisible();
  await expect(rows(page)).toHaveCount(0);
});

test.describe("phone", () => {
  test.use({ ...devices["iPhone 13"] });

  test("tapping the search affordance opens the input without a hardware Ctrl+F", async ({ page }) => {
    await installSidebarMocks(page, { sessions: sessions() });
    await page.goto("/");

    await page.locator('[data-testid="sidebar-search-toggle"]').tap();
    await expect(searchInput(page)).toBeVisible();
    await searchInput(page).fill("orion");
    await expect(rows(page)).toHaveCount(1);
  });
});
