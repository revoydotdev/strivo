import { test, expect } from "@playwright/test";

// Regression coverage for the #/watch side-panel discoverability fix:
// both the left channel rail and the right chat rail default to
// collapsed (space-saving), but shipped with only a tiny toolbar toggle
// button — easy to miss entirely, reported as "stays stuck
// indefinitely." Fix adds a persistent, full-height grab affordance to
// each collapsed panel and (on a genuinely first-ever visit) opens the
// left rail by default so at least one panel is never a dead end.

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("strivo-tour-done", "1");
    // Simulate a genuinely first-ever visit: neither rail key has ever
    // been written.
    localStorage.removeItem("strivo-player-rail-open");
    localStorage.removeItem("strivo-player-chat-rail-open");
  });
});

test("first visit opens the left rail by default (neither panel a dead end)", async ({ page }) => {
  // The chat rail only renders at all when a live, chat-capable (Twitch)
  // tile is on the wall — seed one so this test can still exercise its
  // collapsed-toggle affordance rather than a genuinely empty wall, which
  // now hides the rail entirely (see player-bar.spec.ts's chat-rail
  // visibility coverage).
  await page.addInitScript(() => {
    localStorage.setItem(
      "strivo-player-layout",
      JSON.stringify({ kind: "slot", streamId: "Twitch:twitch-live-1", recordingId: null }),
    );
    localStorage.setItem("strivo-player-preset", "single");
  });
  await page.goto("/app#/watch");
  await expect(page.locator("#watch")).toBeVisible();
  // First-ever visit: left rail defaults open.
  await expect(page.locator("body")).toHaveClass(/watch-rail-open/);
  await expect(page.locator(".watch-rail-toggle")).toHaveAttribute("aria-pressed", "true");
  // Chat rail keeps its own (collapsed) default, but its collapsed
  // toggle must still be a large, obviously-clickable affordance, not a
  // small icon lost in a sliver.
  const chatToggle = page.locator("#player-chat-rail-toggle");
  await expect(chatToggle).toBeVisible();
  const chatBox = await chatToggle.boundingBox();
  expect(chatBox).not.toBeNull();
  expect(chatBox!.height).toBeGreaterThan(200);
});

test("left-rail grab tab appears when collapsed, is large, and toggles the rail", async ({ page }) => {
  // Force the left rail collapsed (as if the user had already closed it
  // once) while leaving the chat rail key unset, so this test exercises
  // the collapsed-state affordance specifically, independent of the
  // first-visit default.
  await page.addInitScript(() => {
    localStorage.setItem("strivo-player-rail-open", "0");
  });
  await page.goto("/app#/watch");
  await expect(page.locator("#watch")).toBeVisible();
  await expect(page.locator("body")).not.toHaveClass(/watch-rail-open/);

  const grab = page.locator(".watch-rail-grab");
  await expect(grab).toBeVisible();
  const box = await grab.boundingBox();
  expect(box).not.toBeNull();
  // A dead-end sliver would be a handful of pixels tall/wide; the fix
  // makes it a full-viewport-height strip.
  expect(box!.height).toBeGreaterThan(400);
  expect(box!.width).toBeGreaterThanOrEqual(18);

  await grab.click();
  await expect(page.locator("body")).toHaveClass(/watch-rail-open/);
  await expect
    .poll(() => page.evaluate(() => localStorage.getItem("strivo-player-rail-open")))
    .toBe("1");
  // Chat rail's own key is untouched by toggling the left rail.
  await expect
    .poll(() => page.evaluate(() => localStorage.getItem("strivo-player-chat-rail-open")))
    .toBeNull();
});

test("chat-rail collapsed toggle stretches full-height and toggles the rail", async ({ page }) => {
  await page.addInitScript(() => {
    // Left rail already opened (its own concern is covered above) so
    // this test isolates the chat rail's collapsed affordance. A live,
    // chat-capable tile must be on the wall — the rail hides entirely
    // otherwise (see player-bar.spec.ts's chat-rail visibility coverage).
    localStorage.setItem("strivo-player-rail-open", "1");
    localStorage.setItem("strivo-player-chat-rail-open", "0");
    localStorage.setItem(
      "strivo-player-layout",
      JSON.stringify({ kind: "slot", streamId: "Twitch:twitch-live-1", recordingId: null }),
    );
    localStorage.setItem("strivo-player-preset", "single");
  });
  await page.goto("/app#/watch");
  await expect(page.locator("#watch")).toBeVisible();

  const rail = page.locator("#player-chat-rail");
  await expect(rail).toHaveAttribute("data-open", "false");
  const toggle = page.locator("#player-chat-rail-toggle");
  const railBox = await rail.boundingBox();
  const toggleBox = await toggle.boundingBox();
  expect(railBox).not.toBeNull();
  expect(toggleBox).not.toBeNull();
  // The collapsed toggle should fill (or nearly fill) the collapsed
  // rail's full height, not sit as a small icon inside it.
  expect(toggleBox!.height).toBeGreaterThan(railBox!.height * 0.85);

  await toggle.click();
  await expect(rail).toHaveAttribute("data-open", "true");
  await expect(page.locator("#watch")).toHaveClass(/has-chat-rail/);
  await expect
    .poll(() => page.evaluate(() => localStorage.getItem("strivo-player-chat-rail-open")))
    .toBe("1");
  // Left rail's own key is untouched by toggling the chat rail.
  await expect
    .poll(() => page.evaluate(() => localStorage.getItem("strivo-player-rail-open")))
    .toBe("1");
});
