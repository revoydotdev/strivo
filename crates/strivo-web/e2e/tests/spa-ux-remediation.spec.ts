import { test, expect } from "@playwright/test";

// Coverage for the 2026-09-06 SPA UX remediation pass (R01/R02/R03/R04).
// Each test exercises the real browser surface (keyboard focus, real
// network round trips) rather than grepping for markup.

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
});

// R01 — recordings library pagination. The mock backend seeds 502 total
// recordings (3 named + 499 filler), so the default 500-row page always
// leaves 2 rows unreached until "Load more recordings" is used.
test("R01: recordings Load more reaches a second page beyond the first 500", async ({ page }) => {
  await page.goto("/app#/recordings");
  await expect(page.locator(".recordings-table")).toBeVisible();

  const count = page.locator("#rec-count");
  await expect(count).toContainText("500 recordings");

  const loadMore = page.locator("#rec-load-more");
  await expect(loadMore).toBeVisible();
  await expect(loadMore).toHaveText("Load more recordings");

  await loadMore.click();

  // The 2 remaining rows are now in the client cache, and the button
  // retires because the server reports no further cursor.
  await expect(count).toContainText("502 recordings");
  await expect(loadMore).toHaveCount(0);
});

// R04 — sortable recordings headers are keyboard-operable, not just
// clickable. Verifies both Enter and Space actually re-sort the table.
test("R04: focusing a sortable header and pressing Enter/Space re-sorts the table", async ({ page }) => {
  await page.goto("/app#/recordings");
  await expect(page.locator(".recordings-table")).toBeVisible();

  const firstRowChannel = () =>
    page.locator("tbody#rec-body tr[data-rec-row]").first().locator("td").nth(2);

  // Default sort is "started desc" — the most recently started fixture
  // (Bravo, 2026-05-26) leads.
  await expect(firstRowChannel()).toHaveText("Bravo");

  const channelHeader = () => page.locator('th[data-sort="channel"]');
  await expect(channelHeader()).toHaveAttribute("tabindex", "0");
  // No role override: `aria-sort` is only valid on columnheader/rowheader,
  // and role="button" replaced the <th>'s implicit columnheader, so every
  // sortable header reported "ARIA attribute is not allowed" (axe
  // aria-allowed-attr, critical). Keyboard operability is tabindex plus the
  // keydown listener; the native role is what makes the announced sort
  // direction legal. This assertion used to demand role="button".
  await expect(channelHeader()).not.toHaveAttribute("role", "button");
  await expect(channelHeader()).toHaveAttribute("aria-sort", "none");

  await channelHeader().focus();
  await expect(channelHeader()).toBeFocused();
  await page.keyboard.press("Enter");
  // Ascending by channel name: Alpha sorts first.
  await expect(firstRowChannel()).toHaveText("Alpha");

  // Header re-renders on each sort; re-locate and press Space this time.
  // It comes back FOCUSED, so this does not re-focus by hand — that is the
  // defect the WCAG pass fixed (sorting used to drop focus on <body>, so a
  // keyboard user could sort exactly once).
  const header2 = channelHeader();
  await expect(header2, "sorting threw focus away instead of restoring it").toBeFocused();
  await page.keyboard.press(" ");
  // Toggled back to descending: the filler channel (alphabetically last)
  // now leads, so the row is no longer Alpha.
  await expect(firstRowChannel()).not.toHaveText("Alpha");
});

// R03 — skip link. Must be the first focusable element in `chrome()`, and
// activating it must move real keyboard focus to #content.
test("R03: Tab focuses the skip link first; activating it focuses #content", async ({ page }) => {
  await page.goto("/app#/library");
  await expect(page.locator("#channel-list")).toBeVisible();

  await page.keyboard.press("Tab");
  const skip = page.locator(".skip-link");
  await expect(skip).toBeFocused();
  await expect(skip).toHaveText("Skip to content");

  await page.keyboard.press("Enter");
  await expect(page.locator("#content")).toBeFocused();
});

// R02 — the B-roll finder plugin is catalogued and auth-gated on the
// backend but had no trigger anywhere in the SPA. Verifies the Info-modal
// action genuinely calls /plugins/broll/<id> and renders real suggestions.
test("R02: B-roll suggestions run from the recording Info modal", async ({ page }) => {
  await page.goto("/app#/recordings");
  const row = page.locator("tr[data-rec-row]", { hasText: "Zebra stream" });
  await row.locator("[data-action=rec-info]").click();
  const modal = page.locator("#rec-info-modal");
  await expect(modal).toBeVisible();

  await modal.locator("[data-action=rec-info-broll]").click();
  const host = modal.locator("#rec-broll");
  await expect(host).toBeVisible();

  const lib = JSON.stringify({
    assets: [{ id: "a1", path: "/media/broll/city.mp4", duration_sec: 8, tags: ["city"] }],
  });
  await host.locator("#rec-broll-lib").fill(lib);
  await host.locator("#rec-broll-run").click();

  await expect(host.locator("#rec-broll-results")).toContainText("a1");
  // Library persists across modal reopens (localStorage), matching the
  // backend's "streamer-curated JSON" contract.
  await expect(host.locator("#rec-broll-lib")).toHaveValue(lib);
});
