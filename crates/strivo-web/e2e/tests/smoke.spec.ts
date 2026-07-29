import { test, expect } from "@playwright/test";

// W7 — critical-path smoke journeys against the real SPA + mock backend.
// Updated for the TUI-style 3-pane redesign: channel-list left rail,
// channel-detail center, recordings dashboard, no Activity surface.

test.beforeEach(async ({ page }) => {
  // The welcome tour has its own focused coverage. Keep unrelated journey
  // tests deterministic and free of the intentional modal overlay.
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
});

test("login page renders and accepts a key", async ({ page }) => {
  await page.goto("/app#/login");
  await expect(page.locator("#login-form")).toBeVisible();
  await page.locator("#api-key").fill("test-key");
  await page.locator("#login-form button[type=submit]").click();
  // On success the SPA leaves login for the home chrome (channel rail).
  await expect(page.locator("#channel-list")).toBeVisible();
});

test("left rail lists channels, live first", async ({ page }) => {
  await page.goto("/app#/library");
  await expect(page.locator("#channel-list")).toBeVisible();
  await expect(page.locator("#channel-list .ch-row", { hasText: "Live Channel" })).toBeVisible();
  await expect(page.locator("#channel-list .ch-row", { hasText: "Offline Channel" })).toBeVisible();
  // LIVE section header appears for the live channel.
  await expect(page.locator(".ch-section-title", { hasText: "LIVE" })).toBeVisible();
});

test("recordings dashboard shows the three rows by default", async ({ page }) => {
  await page.goto("/app#/library");
  await expect(page.getByRole("heading", { name: "In progress" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Recent" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Upcoming" })).toBeVisible();
});

test("clicking a YouTube channel shows detail with streams + uploads", async ({ page }) => {
  await page.goto("/app#/library");
  await page.locator(".ch-row", { hasText: "Live Channel" }).click();
  await expect(page.locator(".cd-name")).toHaveText("Live Channel");
  // VOD lists arrive over SSE (mock pushes one LiveBroadcast + one Upload).
  await expect(page.getByText("Yesterday's livestream")).toBeVisible();
  await expect(page.getByText("How I edit my videos")).toBeVisible();
  await expect(page.locator(".cd-section-title.past-broadcasts", { hasText: "Past Broadcasts" })).toBeVisible();
  await expect(page.locator(".cd-section-title", { hasText: "Recent uploads" })).toBeVisible();
});

test("past-broadcasts pills have a download button that flips to Downloading on click", async ({ page }) => {
  await page.goto("/app#/library");
  await page.locator(".ch-row", { hasText: "Live Channel" }).click();
  // Wait for VODs to arrive via SSE.
  await expect(page.getByText("Yesterday's livestream")).toBeVisible();
  // Each pill in the Past Broadcasts list has a Download button.
  const dlBtn = page
    .locator(".media-pill", { hasText: "Yesterday's livestream" })
    .locator(".vod-dl");
  await expect(dlBtn).toHaveText("Download");
  await expect(dlBtn).toHaveClass(/vod-dl-idle/);
  await dlBtn.click();
  // Downloading state is a progress widget (bar + label), not plain text.
  await expect(dlBtn).toHaveClass(/vod-dl-downloading/);
  await expect(dlBtn).toBeDisabled();
  await expect(dlBtn.locator(".vod-dl-bar")).toBeVisible();
  await expect(dlBtn.locator(".vod-dl-label")).toContainText("%");
});

test("patreon creators appear in the left rail (seeded from /patreon)", async ({ page }) => {
  await page.goto("/app#/library");
  // Seeded on boot from /patreon — no waiting on a poll-driven SSE event.
  await expect(page.locator(".ch-section-title", { hasText: "Patreon" })).toBeVisible();
  await expect(page.getByText("Cool Creator")).toBeVisible();
  await expect(page.locator(".ch-tier", { hasText: "Premium Tier" })).toBeVisible();
});

test("no Activity surface anywhere", async ({ page }) => {
  await page.goto("/app#/library");
  await expect(page.locator(".activity-rail")).toHaveCount(0);
  await expect(page.locator('[data-route="activity"]')).toHaveCount(0);
});

test("top-bar icon nav reaches the recordings table", async ({ page }) => {
  await page.goto("/app#/library");
  await page.locator('.topnav-link[data-route="recordings"]').click();
  await expect(page).toHaveURL(/#\/recordings/);
  await expect(page.locator(".recordings-table")).toBeVisible();
});

test("recordings density toggle + multi-select mass bar", async ({ page }) => {
  await page.goto("/app#/recordings");
  await expect(page.locator(".recordings-table")).toBeVisible();
  // Density toggle adds the compact class.
  await page.locator("#rec-density").click();
  await expect(page.locator(".recordings-table.compact")).toBeVisible();
  // Selecting a row reveals the mass-action bar.
  await page.locator(".rec-row-check").first().check();
  await expect(page.locator("#rec-massbar")).toBeVisible();
  await expect(page.locator("#rec-massbar")).toContainText("selected");
});

test("settings page renders real config sections", async ({ page }) => {
  await page.goto("/app#/settings");
  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
  await expect(page.locator('.stg-rail a[href="#/settings/platforms"]')).toBeVisible();
  await expect(page.locator('.stg-rail a[href="#/settings/recording"]')).toBeVisible();
  await page.locator('.stg-rail a[href="#/settings/platforms"]').click();
  await expect(page.getByRole("heading", { name: "Twitch" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "YouTube" })).toBeVisible();
});

test("system page renders health + tasks", async ({ page }) => {
  await page.goto("/app#/system");
  await expect(page.getByRole("heading", { name: "System" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Health" })).toBeVisible();
  await expect(page.locator(".sys-check").first()).toBeVisible();
  await expect(page.getByRole("heading", { name: "Backup" })).toBeVisible();
  await expect(page.locator("#backup-now")).toBeVisible();
  await expect(page.locator(".restore-backup").first()).toBeVisible();
  await expect(page.getByRole("heading", { name: "Blocklist" })).toBeVisible();
  await expect(page.locator(".unblock").first()).toBeVisible();
  // Live-editable poll interval (item 14b).
  await expect(page.locator("#poll-interval")).toHaveValue("60");
  await expect(page.locator("#poll-interval-save")).toBeVisible();
  // Inline field validation (item 25): below the 15s floor flags aria-invalid.
  await page.locator("#poll-interval").fill("5");
  await page.locator("#poll-interval-save").click();
  await expect(page.locator("#poll-interval")).toHaveAttribute("aria-invalid", "true");
});

test("logs page renders with level selector and lines", async ({ page }) => {
  await page.goto("/app#/logs");
  await expect(page.getByRole("heading", { name: "Logs" })).toBeVisible();
  await expect(page.locator("#logs-level")).toBeVisible();
  await expect(page.locator("#logs-output")).toContainText("StriVo daemon starting");
});

test("monitor page renders capture controls and calendar", async ({ page }) => {
  await page.goto("/app#/schedule");
  await expect(page.getByRole("heading", { name: "Monitor" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Capture limits" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Record when live" })).toBeVisible();
  await expect(page.locator(".cal-strip")).toBeVisible();
  await expect(page.locator(".task-row", { hasText: "Alpha" }).first()).toBeVisible();
});

test("history page renders durable jobs from the DB", async ({ page }) => {
  await page.goto("/app#/history");
  await expect(page.getByRole("heading", { name: "History" })).toBeVisible();
  await expect(page.locator(".media-list")).toContainText("LilAggy");
  await expect(page.locator(".media-pill").first()).toContainText("Finished");
});

test("add-channel wizard opens to phase 1 search", async ({ page }) => {
  await page.goto("/app#/library");
  await page.locator("#add-channel").click();
  await expect(page.locator("#add-channel-modal.open")).toBeVisible();
  await expect(page.locator("#aw-platform")).toBeVisible();
  await expect(page.locator("#aw-query")).toBeVisible();
  await expect(page.locator("#aw-search")).toBeVisible();
});

test("ARIA toast live regions are pre-created on load", async ({ page }) => {
  await page.goto("/app#/library");
  // Both regions must exist before any toast fires, for reliable SR announce.
  await expect(page.locator('.toast-region[role="status"][aria-live="polite"]')).toHaveCount(1);
  await expect(page.locator('.toast-region[role="alert"][aria-live="assertive"]')).toHaveCount(1);
  // The wrap must be non-interactive (pointer-events: none).
  const pe = await page.locator(".toast-wrap").evaluate((el) => getComputedStyle(el).pointerEvents);
  expect(pe).toBe("none");
});

test("command palette opens with Ctrl+K and navigates", async ({ page }) => {
  await page.goto("/app#/library");
  await page.keyboard.press("Control+k");
  await expect(page.locator("#cmdk.open")).toBeVisible();
  await page.locator("#cmdk-input").fill("recordings");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/#\/recordings/);
});

// ── Plugins (hub + per-plugin views) ──────────────────────────────────

test("plugins hub lists the four first-party plugins", async ({ page }) => {
  await page.goto("/app#/plugins");
  await expect(page.getByRole("heading", { name: "Plugins" })).toBeVisible();
  await expect(page.locator(".pg-card")).toHaveCount(4);
  await expect(page.locator(".pg-card", { hasText: "Crunchr" })).toBeVisible();
  await expect(page.locator(".pg-card", { hasText: "Viewguard" })).toBeVisible();
});

test("crunchr view lists recordings, searches, and opens a transcript", async ({ page }) => {
  await page.goto("/app#/plugins/crunchr");
  await expect(page.getByRole("heading", { name: "Crunchr" })).toBeVisible();
  await expect(page.locator(".pg-row", { hasText: "Elden Ring run" })).toBeVisible();

  // Search debounces then renders hits.
  await page.locator("#crunchr-q").fill("boss");
  await expect(page.locator(".pg-search-hits .pg-row").first()).toBeVisible();

  // Open the transcript detail (main list, not the search-hits list).
  await page
    .locator(".pg-list:not(.pg-search-hits) > .pg-row", { hasText: "Elden Ring run" })
    .click();
  await expect(page).toHaveURL(/#\/plugins\/crunchr\/rec\//);
  await expect(page.getByRole("heading", { name: "Analysis" })).toBeVisible();
  await expect(page.locator(".cr-line").first()).toContainText("welcome back");
  await expect(page.locator("#retranscribe")).toBeVisible();
});

test("re-transcribe button dispatches a verb", async ({ page }) => {
  await page.goto("/app#/plugins/crunchr/rec/rec-1");
  await page.locator("#retranscribe").click();
  await expect(page.locator(".toast-region[aria-live=polite]")).toContainText("queued");
});

test("archiver view lists channels and opens a catalog", async ({ page }) => {
  await page.goto("/app#/plugins/archiver");
  await expect(page.getByRole("heading", { name: "Archiver" })).toBeVisible();
  await page.locator(".pg-row", { hasText: "Alpha" }).click();
  await expect(page).toHaveURL(/#\/plugins\/archiver\//);
  await expect(page.getByRole("heading", { name: "Catalog" })).toBeVisible();
  await expect(page.locator(".pg-row", { hasText: "Stream Two" })).toBeVisible();
  await expect(page.locator(".cfg-badge", { hasText: "downloaded" }).first()).toBeVisible();
});

test("viewguard view shows verdict bands", async ({ page }) => {
  await page.goto("/app#/plugins/viewguard");
  await expect(page.getByRole("heading", { name: "Viewguard" })).toBeVisible();
  await expect(page.locator(".vg-band-fraudulent")).toBeVisible();
  await expect(page.locator(".vg-band-clean")).toBeVisible();
  await expect(page.locator(".vg-score-num").first()).toContainText("87%");
});

test("insights view shows word bars and topics", async ({ page }) => {
  await page.goto("/app#/plugins/insights");
  await expect(page.getByRole("heading", { name: "Insights" })).toBeVisible();
  await expect(page.locator(".wf-row", { hasText: "stream" })).toBeVisible();
  await expect(page.locator(".pg-chip", { hasText: "elden ring" })).toBeVisible();
  await expect(page.locator("#ins-stopwords")).toBeVisible();
});

test("recordings: clear-errored button appears + per-row Play/Info/Delete actions", async ({ page }) => {
  await page.goto("/app#/recordings");
  await expect(page.locator(".recordings-table")).toBeVisible();
  // Mock has one Failed row (Charlie) → toolbar button visible with count.
  const clearBtn = page.locator("#rec-clear-errored");
  await expect(clearBtn).toBeVisible();
  await expect(clearBtn).toContainText("Clear errored");
  await expect(clearBtn).toContainText("(1)");
  // Finished row exposes a Play button; errored row doesn't.
  const finishedRow = page.locator("tr[data-rec-row]", { hasText: "Zebra stream" });
  await expect(finishedRow.locator("[data-action=rec-play]")).toBeVisible();
  const erroredRow = page.locator("tr[data-rec-row]", { hasText: "Mango stream" });
  await expect(erroredRow.locator("[data-action=rec-play]")).toHaveCount(0);
  // Every non-active row carries Info + Delete.
  await expect(finishedRow.locator("[data-action=rec-info]")).toBeVisible();
  await expect(finishedRow.locator("[data-action=rec-delete]")).toBeVisible();
});

test("recordings: ⓘ Info opens modal with stats + plugin actions; Esc closes", async ({ page }) => {
  await page.goto("/app#/recordings");
  const row = page.locator("tr[data-rec-row]", { hasText: "Zebra stream" });
  await row.locator("[data-action=rec-info]").click();
  const modal = page.locator("#rec-info-modal");
  await expect(modal).toBeVisible();
  await expect(modal.locator(".rec-info-body .rec-info-stats")).toBeVisible();
  await expect(modal).toContainText("Channel");
  await expect(modal).toContainText("Started");
  // Plugin actions area is present (verbs may or may not render given mock — at minimum the section + heading.)
  await expect(modal.locator(".rec-info-actions h3")).toContainText("Plugin actions");
  await page.keyboard.press("Escape");
  await expect(modal).toHaveCount(0);
});

test("recordings: ▶ Play opens the in-app watch player", async ({ page }) => {
  await page.goto("/app#/recordings");
  const row = page.locator("tr[data-rec-row]", { hasText: "Zebra stream" });
  await row.locator("[data-action=rec-play]").click();
  await expect(page).toHaveURL(/#\/watch/);
  const tile = page.locator(".ms-leaf-rec").first();
  await expect(tile).toBeVisible();
  await expect(tile.locator("video.ms-video")).toBeVisible();
  await expect(tile.locator(".ms-solo")).toBeVisible();
  await expect(tile.locator(".ms-fs")).toBeVisible();
  await expect(tile.locator(".ms-remove")).toBeVisible();
});

test("last-live label uses Xh/Xd precision and renders for YouTube too", async ({ page }) => {
  await page.goto("/app#/library");
  await expect(page.locator("#channel-list")).toBeVisible();

  // Twitch row, seeded ~5h ago → "Xh ago".
  const twitch = page.locator(".ch-row", { hasText: "Offline Channel" }).first();
  await expect(twitch.locator(".ch-lastlive")).toHaveText(/^\d+h ago$/);

  // YouTube row, seeded ~3d ago → "Xd ago". This is the case that used to
  // collapse to "today" / never-rendered for YouTube.
  const yt = page.locator(".ch-row", { hasText: "YouTube Past Stream" });
  await expect(yt.locator(".ch-lastlive")).toHaveText(/^\d+d ago$/);
});
