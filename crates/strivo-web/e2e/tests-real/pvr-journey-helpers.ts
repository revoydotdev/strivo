import { expect, type APIRequestContext, type Page } from "@playwright/test";

// Shared fixtures for the pvr-journey-*.spec.ts audit (todo 15).
//
// Not a spec file: Playwright's default testMatch is `*.@(spec|test).*`, so
// this module is never collected as a test.
//
// TWO TRAPS this module exists to make impossible to forget.
//
// 1. The product tour overlays the whole page and intercepts pointer events,
//    so every click in this lane silently no-ops without it. 20+ existing
//    uses in ../tests/; the audit would have reported "the wizard's Search
//    button does nothing" and been wrong.
//
// 2. `API._fetch` collapses GETs behind `_cacheTtlMs: 2000`
//    (assets/spa/002-pvr.js:17). A DOM read taken within 2 s of an earlier
//    read of the same path is a CACHE HIT, not a live server answer. Every
//    helper here therefore reads authoritative state through `request` (a
//    separate HTTP context with an `x-api-key` header, which never touches
//    the page's cache) rather than through the SPA. See PJ-08 in
//    docs/audits/2026-09-25-pvr-journey/REPORT.md for the finding this
//    discipline exists to prevent.

export const API_KEY = "strivo-e2e-real-server-test-key";
export const TOUR_DONE = () => localStorage.setItem("strivo-tour-done", "1");

/** Sign in through the real login form. Fails bounded, on a named selector. */
export async function login(page: Page) {
  await page.addInitScript(TOUR_DONE);
  await page.goto("/app#/login");
  await expect(page.locator("#login-form")).toBeVisible({ timeout: 15_000 });
  await page.locator("#api-key").fill(API_KEY);
  await page.locator("#login-form button[type=submit]").click();
  // The app chrome's channel rail is the first thing painted after login.
  await expect(page.locator("#channel-list")).toBeVisible({ timeout: 15_000 });
}

/** Authoritative read: bypasses the SPA entirely, so it can never be a cache hit. */
export async function apiGet(request: APIRequestContext, path: string) {
  const res = await request.get(path, { headers: { "x-api-key": API_KEY } });
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
 * boot, so a literal would make the whole audit silently skip on the next
 * run. Matched on the literal the seeding script writes
 * (`channel_name: "e2eseed"`, real-server.sh:115), and asserted to exist.
 */
export async function seededRecording(request: APIRequestContext): Promise<SeededRecording> {
  const { status, body } = await apiGet(request, "/api/v1/recordings");
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
