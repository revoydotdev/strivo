import { test, expect } from "@playwright/test";

// Studio pane coverage for the two previously-orphaned tool crates
// (ab-render, submix) — the nav entries at #/studio/ab and
// #/studio/submix must render a real, working surface, not the
// generic "reached from inside the Editor" placeholder.

const RECORDING_ID = "11111111-1111-1111-1111-111111111111";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
});

test("A/B render pane: save both slots, diff, then compare via the real endpoint", async ({ page }) => {
  await page.goto("/app#/studio/ab");
  await expect(page.locator(".pro-tab.is-active", { hasText: "A/B render compare" })).toBeVisible();

  await page.locator("#abr-rec").selectOption(RECORDING_ID);
  await expect(page.locator(".abr-slot[data-slot='a']")).toBeVisible();
  await expect(page.locator(".abr-slot[data-slot='b']")).toBeVisible();

  // Slot A: quieter loudness target.
  const slotA = page.locator(".abr-slot[data-slot='a']");
  await slotA.locator(".abr-f-label").fill("quiet-master");
  await slotA.locator(".abr-f-lufs").fill("-18");
  const saveAResp = page.waitForResponse((r) => r.url().includes(`/api/v1/plugins/ab-render/${RECORDING_ID}/a`) && r.request().method() === "POST");
  await slotA.locator(".abr-save").click();
  await saveAResp;

  // Slot B: louder target + faster tempo, so the diff has real rows.
  const slotB = page.locator(".abr-slot[data-slot='b']");
  await slotB.locator(".abr-f-label").fill("loud-master");
  await slotB.locator(".abr-f-lufs").fill("-14");
  await slotB.locator(".abr-f-tempo").fill("1.1");
  const saveBResp = page.waitForResponse((r) => r.url().includes(`/api/v1/plugins/ab-render/${RECORDING_ID}/b`) && r.request().method() === "POST");
  await slotB.locator(".abr-save").click();
  await saveBResp;

  // Diff table reflects the label/loudness/tempo differences.
  const diffTable = page.locator(".abr-diff");
  await expect(diffTable).toContainText("label");
  await expect(diffTable).toContainText("loudness_lufs");
  await expect(diffTable).toContainText("tempo");

  // Compare button is now enabled — both slots filled.
  const compareBtn = page.locator("#abr-compare");
  await expect(compareBtn).toBeEnabled();
  const compareResp = page.waitForResponse((r) => r.url().includes(`/api/v1/plugins/ab-render/${RECORDING_ID}/compare`) && r.request().method() === "POST");
  await compareBtn.click();
  await compareResp;

  await expect(page.locator(".abr-quality")).toContainText("VMAF mean");
  await expect(page.locator(".abr-quality")).toContainText("SSIM all");
});

test("Sub-mix pane: add tracks, save, and see the composed filter_complex from the real endpoint", async ({ page }) => {
  await page.goto("/app#/studio/submix");
  await expect(page.locator(".pro-tab.is-active", { hasText: "Sub-mix bus" })).toBeVisible();

  await page.locator("#smx-rec").selectOption(RECORDING_ID);
  await expect(page.locator(".smx-tracks-card")).toBeVisible();

  await page.locator("#smx-add-track").click();
  await page.locator("#smx-add-track").click();

  const rows = page.locator(".smx-track");
  await expect(rows).toHaveCount(2);

  await rows.nth(0).locator(".smx-t-label").fill("voice");
  await rows.nth(0).locator(".smx-t-input").fill("0");
  await rows.nth(0).locator(".smx-t-gain").fill("0");

  await rows.nth(1).locator(".smx-t-label").fill("game");
  await rows.nth(1).locator(".smx-t-input").fill("1");
  await rows.nth(1).locator(".smx-t-gain").fill("-3");

  await page.locator("#smx-master-gain").fill("-1");

  const saveResp = page.waitForResponse((r) => r.url().includes(`/api/v1/plugins/submix/${RECORDING_ID}`) && r.request().method() === "POST");
  await page.locator("#smx-save").click();
  await saveResp;

  const fc = page.locator(".abr-filter");
  await expect(fc).toContainText("[0:a]anull[bus0]");
  await expect(fc).toContainText("volume=-3.00dB");
  await expect(fc).toContainText("amix=inputs=2:normalize=0[mix]");
  await expect(fc).toContainText("[mix]volume=-1.00dB[out]");
});
