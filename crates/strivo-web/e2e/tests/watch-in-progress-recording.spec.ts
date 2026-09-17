import { test, expect } from "@playwright/test";

// B-04 (SPA half) — the recording player (#/play?recording=<id>, 019b-pvr.js)
// must not autoplay or source /download for a job that is still being
// written. The mock fixture "22222222-2222-2222-2222-222222222222"
// (mock-server.mjs RECORDINGS) has state: "Recording", which isInProgress()
// treats as in-flight.
//
// Every recording-open path used to land on #/watch (the live multiview
// wall); it now opens this dedicated route instead, and the legacy
// `#/watch?recording=…&fresh=1` shape redirects to it (renderWatch,
// 018e-pvr.js) for old bookmarks/external links.

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
});

test("watching an in-progress recording shows the still-recording affordance, not a player", async ({ page }) => {
  const downloadRequests: string[] = [];
  page.on("request", (req) => {
    if (req.url().includes("/download")) downloadRequests.push(req.url());
  });

  await page.goto("/app#/play?recording=22222222-2222-2222-2222-222222222222");

  const tile = page.locator('.ms-leaf[data-recording-id="22222222-2222-2222-2222-222222222222"]');
  await expect(tile).toBeVisible();
  await expect(tile.locator(".ms-empty-pill")).toHaveText(/still recording/i);

  // No mount was ever created for this tile — no video/iframe, and no
  // controller ever requested the (unstable-length) /download bytes.
  await expect(tile.locator(".ms-mount")).toHaveCount(0);
  await expect(tile.locator("video")).toHaveCount(0);
  expect(downloadRequests).toEqual([]);
});

test("watching a finished recording still autoplays a real player", async ({ page }) => {
  await page.goto("/app#/play?recording=11111111-1111-1111-1111-111111111111");

  const tile = page.locator('.ms-leaf-rec[data-recording-id="11111111-1111-1111-1111-111111111111"]');
  await expect(tile).toBeVisible();
  await expect(tile.locator(".ms-mount")).toHaveAttribute("data-src", /\/download$/);
  await expect(tile.locator(".ms-mount")).toHaveAttribute("data-playing", "1");
});

test("the legacy #/watch?recording=…&fresh=1 shape redirects to the player route", async ({ page }) => {
  await page.goto("/app#/watch?recording=11111111-1111-1111-1111-111111111111&fresh=1");
  await page.waitForFunction(() => /#\/play\?recording=/.test(window.location.hash));
  expect(page.url()).toContain(
    "#/play?recording=11111111-1111-1111-1111-111111111111",
  );
  await expect(page.locator("#play")).toBeVisible();
});
