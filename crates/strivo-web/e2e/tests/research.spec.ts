import { test, expect } from "@playwright/test";

// Coding Studio surfaces (codebook.js / corpus.js / notebook.js) mounted as
// Archive sub-tabs over the research kernel. Fixtures come from
// mock-server.mjs's RESEARCH_* constants (one project, one source, a
// parent+child code pair, one coding, one case, one signal, one memo, one
// relationship).

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
});

test("archive route shows sub-tabs for the Coding Studio surfaces", async ({ page }) => {
  await page.goto("/app#/archive");
  await expect(page.getByRole("heading", { name: "Archive" })).toBeVisible();
  await expect(page.locator(".pro-tab", { hasText: "Search" })).toBeVisible();
  await expect(page.locator(".pro-tab", { hasText: "Codebook" })).toBeVisible();
  await expect(page.locator(".pro-tab", { hasText: "Corpus" })).toBeVisible();
  await expect(page.locator(".pro-tab", { hasText: "Notebook" })).toBeVisible();
});

test("codebook tab renders the code tree and its codings", async ({ page }) => {
  await page.goto("/app#/archive/codebook");
  await expect(page.locator(".pro-tab.is-active", { hasText: "Codebook" })).toBeVisible();
  await expect(page.locator(".cb-node-btn", { hasText: "Onboarding friction" })).toBeVisible();
  await expect(page.locator(".cb-node-btn", { hasText: "Signup drop-off" })).toBeVisible();
  // The one seeded coding shows under "all codes" by default.
  await expect(page.locator(".arc-row", { hasText: "the boss fight is confusing at first" })).toBeVisible();

  // Selecting the parent code re-filters the codings list to it.
  await page.locator(".cb-node-btn", { hasText: "Onboarding friction" }).click();
  await expect(page.locator(".cfg-title", { hasText: "Codings · Onboarding friction" })).toBeVisible();
});

test("codebook tab creates a new code", async ({ page }) => {
  await page.goto("/app#/archive/codebook");
  await page.locator("#cb-new-code-btn").click();
  await expect(page.locator("#cb-code-form")).toBeVisible();
  await page.locator("#cb-code-name").fill("Retention risk");
  const [req] = await Promise.all([
    page.waitForRequest((r) => r.url().includes("/codes") && r.method() === "POST"),
    page.locator("#cb-code-form button[type=submit]").click(),
  ]);
  expect(JSON.parse(req.postData() || "{}").name).toBe("Retention risk");
  await expect(page.locator(".toast-region[aria-live=polite]")).toContainText("Code created");
});

test("corpus tab lists sources, cases, and the signal browser", async ({ page }) => {
  await page.goto("/app#/archive/corpus");
  await expect(page.locator(".pro-tab.is-active", { hasText: "Corpus" })).toBeVisible();
  await expect(page.locator(".arc-row", { hasText: "Elden Ring run" }).first()).toBeVisible();
  await expect(page.locator(".arc-row", { hasText: "Case One" })).toBeVisible();
  await expect(page.locator(".arc-row", { hasText: "hey everyone welcome back" })).toBeVisible();
});

test("corpus tab creates a case and assigns a source to it", async ({ page }) => {
  await page.goto("/app#/archive/corpus");
  await page.locator("#cp-new-case-btn").click();
  await page.locator("#cp-case-name").fill("New cohort");
  const [createReq] = await Promise.all([
    page.waitForRequest((r) => r.url().includes("/cases") && r.method() === "POST" && !r.url().includes("/sources")),
    page.locator("#cp-case-form button[type=submit]").click(),
  ]);
  expect(JSON.parse(createReq.postData() || "{}").name).toBe("New cohort");

  const assignSelect = page.locator(".cp-assign-source").first();
  const [assignReq] = await Promise.all([
    page.waitForRequest((r) => /\/cases\/[^/]+\/sources$/.test(r.url()) && r.method() === "POST"),
    assignSelect.selectOption({ label: "Elden Ring run" }),
  ]);
  expect(assignReq.method()).toBe("POST");
});

test("corpus tab filters signals by kind", async ({ page }) => {
  await page.goto("/app#/archive/corpus");
  await expect(page.locator(".arc-row", { hasText: "hey everyone welcome back" })).toBeVisible();
  await page.locator("#cp-sig-kind").fill("nonexistent.kind");
  const [req] = await Promise.all([
    page.waitForRequest((r) => r.url().includes("/signals") && r.url().includes("kind=nonexistent")),
    page.locator("#cp-sig-apply").click(),
  ]);
  expect(req.url()).toContain("kind=nonexistent.kind");
  await expect(page.locator("#cp-signals-host")).toContainText("No signals match this filter.");
});

test("notebook tab shows memos and relationships", async ({ page }) => {
  await page.goto("/app#/archive/notebook");
  await expect(page.locator(".pro-tab.is-active", { hasText: "Notebook" })).toBeVisible();
  await expect(page.locator(".arc-row", { hasText: "Pacing memo" })).toBeVisible();
  await expect(page.locator(".arc-row", { hasText: "supports" })).toBeVisible();
});

test("notebook tab computes coder agreement", async ({ page }) => {
  await page.goto("/app#/archive/notebook");
  await page.locator("#nb-agr-a").fill("Ada");
  await page.locator("#nb-agr-b").fill("Grace");
  const [req] = await Promise.all([
    page.waitForRequest((r) => r.url().includes("/agreement")),
    page.locator("#nb-agr-run").click(),
  ]);
  expect(req.url()).toContain("author_a=Ada");
  expect(req.url()).toContain("author_b=Grace");
  await expect(page.locator(".nb-stat-value").first()).toHaveText("0.820");
});

test("notebook tab exports JSON", async ({ page }) => {
  await page.goto("/app#/archive/notebook");
  const [download, req] = await Promise.all([
    page.waitForEvent("download"),
    page.waitForRequest((r) => r.url().includes("/export") && r.url().includes("format=json")),
    page.locator("#nb-export-json").click(),
  ]);
  expect(req.url()).toContain("format=json");
  expect(download.suggestedFilename()).toContain("archive-");
});
