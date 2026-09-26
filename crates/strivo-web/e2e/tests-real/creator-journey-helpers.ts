import { expect, test, type APIRequestContext, type Page } from "@playwright/test";

// Shared fixtures for the creator-journey-*.spec.ts audit (todo 16).
//
// Not a spec file: Playwright's default testMatch is `*.@(spec|test).*`, so
// this module is never collected as a test.
//
// THE EDITION GUARD, and why it cannot hide a failure.
//
// Both playwright.real.config.ts and playwright.creator.config.ts set
// `testDir: ./tests-real`, and Playwright's default testMatch is recursive —
// so these Creator specs are ALSO collected by `npm run test:real`, which
// boots a PVR binary on which every Creator route is 404. Left alone they
// would fail the PVR lane; "fixed" by deleting them, the lane would not
// exist. So `requireCreatorEdition` reads the served descriptor and skips
// when the edition is not `creator`.
//
// That skip is a *true statement about the binary under test*, and it cannot
// make this lane vacuous, because the very first thing every spec does is
// assert `edition === "creator"` with a non-empty, Pro-bearing capability
// set. A binary that is not Creator fails the lane rather than skipping it.
// The guard is therefore an edition assertion, not an error swallower.
export const API_KEY = "strivo-e2e-real-server-test-key";
export const TOUR_DONE = () => localStorage.setItem("strivo-tour-done", "1");

export type Descriptor = {
  version: number;
  edition: string;
  capabilities: {
    key: string;
    edition: string;
    entitlement: "included" | "pro";
    entry_point: "available" | "roadmap";
    installed: boolean;
    requires: string[];
    capability_tags: string[];
  }[];
};

/**
 * Read the feature-capability descriptor — the one fact that decides which
 * edition this binary is.
 *
 * Read through `request`, never through the page: `API._fetch` collapses
 * duplicate GETs behind `_cacheTtlMs: 2000` (assets/spa/002-pvr.js:17), so a
 * DOM-initiated read inside that window is a cache hit, not a server answer.
 */
export async function descriptor(request: APIRequestContext): Promise<Descriptor> {
  const res = await request.get("/api/v1/plugins/descriptor", {
    headers: { "x-api-key": API_KEY },
  });
  expect(res.status(), "GET /api/v1/plugins/descriptor").toBe(200);
  return (await res.json()) as Descriptor;
}

/** Skip unless this binary actually is a Creator build. See the note above. */
export async function requireCreatorEdition(request: APIRequestContext) {
  const d = await descriptor(request).catch(() => null);
  test.skip(d?.edition !== "creator", `binary reports edition "${d?.edition}", not "creator"`);
  return d as Descriptor;
}

/** Sign in through the real login form. Fails bounded, on a named selector. */
export async function login(page: Page) {
  await page.addInitScript(TOUR_DONE);
  await page.goto("/app#/login");
  await expect(page.locator("#login-form")).toBeVisible({ timeout: 15_000 });
  await page.locator("#api-key").fill(API_KEY);
  await page.locator("#login-form button[type=submit]").click();
  // The channel rail is the first thing painted after login.
  await expect(page.locator("#channel-list")).toBeVisible({ timeout: 15_000 });
}

/** Authoritative read: bypasses the SPA entirely, so it can never be a cache hit. */
export async function apiJson(
  request: APIRequestContext,
  method: "get" | "post",
  path: string,
  body?: unknown,
) {
  const res = await request[method](path, {
    headers: { "x-api-key": API_KEY, "content-type": "application/json" },
    data: body ?? {},
  });
  return { status: res.status(), body: await res.json().catch(() => null) };
}

export type SeededRecording = {
  id: string;
  channel_name: string;
  state: string;
  bytes_written: number;
  output_path: string;
  file_exists: boolean;
};

/**
 * The one recording real-server.sh seeds, located by its real provenance.
 *
 * Deliberately NOT hardcoded by id: real-server.sh mints a fresh uuid per
 * boot, so a literal would make the whole audit silently skip on a later run.
 * Matched on the literal the seeding script writes (`channel_name:
 * "e2eseed"`, real-server.sh:115) and asserted to exist.
 */
export async function seededRecording(request: APIRequestContext): Promise<SeededRecording> {
  const { status, body } = await apiJson(request, "get", "/api/v1/recordings");
  expect(status, "GET /api/v1/recordings").toBe(200);
  const found = (body?.recordings ?? []).find(
    (r: SeededRecording) => r.channel_name === "e2eseed",
  );
  expect(
    found,
    `real-server.sh seeds exactly one "e2eseed" recording; got ${JSON.stringify(body)}`,
  ).toBeTruthy();
  expect(found.state, "the seeded row is journalled as Finished").toBe("Finished");
  expect(found.bytes_written, "real ffmpeg output, not a fixture").toBeGreaterThan(0);
  return found;
}

/**
 * Open the Creator surfaces that the shipped product keeps closed.
 *
 * WHAT THIS IS, precisely, because the distinction is the whole audit:
 * `GET /api/v1/settings` hardcodes `creator_enabled: false`
 * (routes/settings.rs:637-639) on every build, Creator-featured or not. The
 * SPA reads that flag twice — `chrome()` filters all seven Creator nav
 * entries (012-pvr.js:128-130) and `render()` hard-redirects all seven
 * Creator routes to `#/library` (008-pvr.js:562-565). So on a real Creator
 * binary the Pro panes never render and the plugin hub never renders, and
 * every observation of their content would otherwise be impossible.
 *
 * This helper rewrites ONE FIELD of ONE response so the real router reaches
 * the real `renderProApp`. It changes:
 *   - nothing in the product source, the served bundle, or the binary;
 *   - nothing about entitlement — `creator_enabled` is a *reachability* flag
 *     and `licence.entitled` stays false, so every run below is still an
 *     UNLICENSED machine, which is the state the audit is about;
 *   - nothing about the backend's answers — the panes still run against real
 *     routes, real 402s and real data.
 *
 * It therefore buys reachability and nothing else. Every spec that uses it
 * says so, and the manifest records the pane-content findings as reached
 * under instrumentation while the reachability finding itself is observed
 * WITHOUT it. Reversible by construction: the interception lives in the
 * browser context and dies with the page.
 *
 * It reloads before returning, and that is load-bearing rather than tidy:
 * `page.goto` to the same path with a different hash is a same-document
 * navigation, so it does NOT re-boot the SPA, and `fetchEdition()` — the only
 * reader of `creator_enabled` — would never run again with the interception
 * in place. Registering the route without reloading left the panes still
 * bouncing to Home, which is the failure this note exists to stop being
 * rediscovered.
 *
 * It then WAITS for the settings response rather than assuming it landed.
 * A reload followed immediately by a hash navigation is a race: `render()`
 * reads `CREATOR_ENABLED`, which is still false until `fetchEdition()`
 * resolves, so a fast hash change bounces the pane to Home. That produced an
 * intermittent failure of exactly this shape before the wait was added.
 */
export async function openCreatorSurfaces(page: Page) {
  await page.route("**/api/v1/settings", async (route) => {
    const res = await route.fetch();
    const body = await res.json().catch(() => null);
    if (body && typeof body === "object") body.creator_enabled = true;
    await route.fulfill({
      status: res.status(),
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body ?? {}),
    });
  });
  const settingsArrived = page.waitForResponse(
    (r) => r.url().includes("/api/v1/settings") && r.request().resourceType() !== "image",
    { timeout: 15_000 },
  );
  await page.reload();
  await settingsArrived;
}

/** Read the SPA's own source out of the served bundle.
 *
 * Deliberately not `page.evaluate(() => window.API)`: assets/spa.html:17
 * loads spa.js with `type="module"`, so the SPA's top-level `const API` is
 * NOT on `window` and that read is `undefined`. Reading the served
 * /assets/spa.js also proves the behaviour is in the emitted artifact rather
 * than only in the source tree.
 */
export async function servedBundle(request: APIRequestContext): Promise<string> {
  const res = await request.get("/assets/spa.js");
  expect(res.status(), "GET /assets/spa.js").toBe(200);
  return res.text();
}
