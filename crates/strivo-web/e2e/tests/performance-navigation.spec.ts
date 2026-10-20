import { test, expect } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("strivo-tour-done", "1");
  });
});

test("a late recordings response cannot replace a newer Settings route", async ({ page }) => {
  // Hold every recordings response behind a gate.  A timed delay raced the
  // `load` event that `goto` waits for: under load the app's loading window
  // could close before the first assertion ran.  The gate keeps the Recordings
  // route genuinely in flight until the test decides it is late.
  let releaseRecordings!: () => void;
  const recordingsHeld = new Promise<void>((resolve) => { releaseRecordings = resolve; });
  await page.route("**/api/v1/recordings**", async (route) => {
    await recordingsHeld;
    await route.continue();
  });
  const isRecordings = (r: { url(): string }) => r.url().includes("/api/v1/recordings");
  const lateResponses = [
    page.waitForResponse(isRecordings),
    page.waitForResponse(isRecordings),
  ];

  await page.goto("/app#/recordings");
  await expect(page.locator("#route-status")).toContainText(/Loading recordings/i);
  await page.locator('a[href="#/settings"]').click();

  await expect(page).toHaveURL(/#\/settings/);
  await expect(page.locator("h1.page-title")).toHaveText("Settings");
  // Now let the abandoned requests complete.  Their renderer must not reclaim
  // the shared content node after the Settings route owns it.  Waiting for
  // each response to finish is the real signal, not a timer.
  releaseRecordings();
  await Promise.all((await Promise.all(lateResponses)).map((r) => r.finished()));
  await expect(page.locator("h1.page-title")).toHaveText("Settings");
  await expect(page.locator(".recordings-table")).toHaveCount(0);
});

test("Home paints while health, Patreon, and schedule are still loading", async ({ page }) => {
  let releaseAncillary!: () => void;
  const ancillaryHeld = new Promise<void>((resolve) => { releaseAncillary = resolve; });
  for (const path of ["**/api/v1/patreon", "**/api/v1/schedule", "**/api/v1/health"]) {
    await page.route(path, async (route) => {
      await ancillaryHeld;
      await route.continue();
    });
  }

  await page.goto("/app#/library");
  // Channels and recordings are the dashboard core.  Ancillary calls remain
  // held, so this proves the visible destination has no dependency waterfall.
  await expect(page.locator("h1.page-title")).toHaveText("Home");
  await expect(page.locator("#route-status")).toBeHidden();

  releaseAncillary();
});

test("navigation keeps the chrome node and direct recordings entry hydrates its rail", async ({ page }) => {
  await page.goto("/app#/recordings");
  await expect(page.locator(".recordings-table")).toBeVisible();
  await expect(page.locator("#channel-list")).toContainText("Live Channel");
  const shell = await page.locator(".chrome").elementHandle();

  await page.locator('a[href="#/settings"]').click();
  await expect(page.locator("h1.page-title")).toHaveText("Settings");
  expect(await page.evaluate((node) => document.querySelector(".chrome") === node, shell)).toBe(true);
});
