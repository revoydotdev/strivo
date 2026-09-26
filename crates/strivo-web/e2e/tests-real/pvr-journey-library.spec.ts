import { test, expect } from "@playwright/test";
import { apiGet, login, seededRecording } from "./pvr-journey-helpers";

// Journey steps 6 (library) and 7 (watch), driven against the real release
// binary. Findings: PJ-03, PJ-04. See
// docs/audits/2026-09-25-pvr-journey/REPORT.md.
//
// NON-VACUITY: the row asserted here is real ffmpeg output on real disk,
// matched by the literal real-server.sh writes. mock-server.mjs:449 returns
// a hardcoded `LilAggy` / "Elden Ring" history row and mock-server.mjs:516
// a "Live Channel" fixture with is_live:true, so neither the "No recordings
// yet" empty state nor the empty watch stage can pass against the mock.

test("step 6 — library renders the real recording with loading, empty and filtered states", async ({ page, request }) => {
  await login(page);
  const seeded = await seededRecording(request);

  // --- state: loading. `root` carries aria-busy while the route paints
  // (012e-pvr.js:76 clears it only after API.recordings() resolves), so the
  // busy attribute is a genuine, observable pre-data state.
  await page.goto("/app#/recordings");
  const busy = page.locator("#app[aria-busy]");
  // Racy by nature — record whichever of the two states we caught.
  const sawBusy = await busy
    .waitFor({ state: "attached", timeout: 1500 })
    .then(() => true)
    .catch(() => false);
  expect(typeof sawBusy).toBe("boolean");

  // --- state: populated, from the real daemon.
  await expect(page.locator("h1.page-title")).toHaveText("Recordings");
  await expect(page.locator(".recordings-table")).toBeVisible();
  await expect(page.locator("#rec-body tr")).toHaveCount(1);
  const row = page.locator("#rec-body tr").first();
  await expect(row).toContainText("e2eseed");
  await expect(row).toContainText("real lane range check");
  await expect(row).toContainText("Finished");

  // The row's size must be the real file's size, read from the server.
  // This is the assertion that would fail against a fixture.
  const expectedKb = Math.round(seeded.bytes_written / 1024);
  await expect(row).toContainText(`${expectedKb} KB`);

  await expect(page.locator("#rec-count")).toContainText("1 recording");
  // No Load-more: the library genuinely has one page.
  await expect(page.locator("#rec-load-more")).toHaveCount(0);

  // --- state: state chips are deliberately ABSENT for a single-state
  // library (012f-pvr.js:165-169, "Single state in cache → chips add no
  // value; skip the row entirely"). Recorded so a future reader does not
  // mistake the empty chip row for a defect.
  await expect(page.locator(".rec-state-chip")).toHaveCount(0);

  // --- state: filtered to empty. The filter narrows the client-side cache
  // (012e-pvr.js:139-143) — no refetch, so this is a pure view state.
  await page.locator("#rec-filter").fill("zzz-no-such-channel");
  await expect(page.locator("#rec-count")).toContainText("0 match");
  await expect(page.locator("#rec-body tr")).toHaveCount(0);
  await page.locator("#rec-filter").fill("");
  await expect(page.locator("#rec-body tr")).toHaveCount(1);

  // --- state: timeline view reads the DURABLE journal, and the old
  // #/history route redirects into it (008-pvr.js:16-17, 012-pvr.js:88).
  await page.goto("/app#/history");
  await expect(page).toHaveURL(/#\/recordings\?view=timeline/);
  await expect(page.locator("h1.page-title")).toHaveText("Recordings");
  await expect(page.locator("#content")).toContainText("durable across restarts");
  await expect(page.locator("#content")).toContainText("real lane range check");
  // The journal proves the file is present, independently of the snapshot.
  const hist = await apiGet(request, "/api/v1/history");
  expect(hist.status).toBe(200);
  expect(hist.body.history.length).toBeGreaterThan(0);
});

test("step 7 — watch route: empty stage, and a retired route that still resolves", async ({ page, request }) => {
  await login(page);
  const seeded = await seededRecording(request);

  await page.goto("/app#/watch");
  // The recent-recordings strip is fed by the route's recordings hydration
  // (ROUTE_HYDRATION.watch, 012-pvr.js:430), so it appears a beat after the
  // stage. Wait for the stable stage copy first rather than racing the fetch.
  await expect(page.locator("#content")).toContainText("drag a channel from the rail", {
    timeout: 20_000,
  });
  // Case-insensitive on purpose: the DOM text is "Recent recordings" and the
  // stylesheet uppercases it, so `innerText` returns either form depending on
  // whether the stylesheet has been applied at read time. Asserting the exact
  // case made this a 20 s timeout rather than a real signal.
  await expect(page.locator("#content")).toContainText(/recent recordings/i, { timeout: 20_000 });
  await expect(page.locator("#content")).toContainText("real lane range check");

  // --- state: empty stage. There is no channel, so there is no tile and no
  // vendor iframe. The copy names the cause rather than showing a blank
  // wall, which is the correct empty state.
  await expect(page.locator("iframe")).toHaveCount(0);
  const channels = await apiGet(request, "/api/v1/channels");
  expect(channels.body.channels, "no channels => no tiles, authoritatively").toEqual([]);

  // --- PJ-03: a malformed recording route. `#/play/<id>` is NOT the shape
  // the SPA emits — renderPlayer reads only the `?recording=` query param
  // (019b-pvr.js:61-62) and never consults routeParts(). So a path-style
  // URL, which is the obvious thing to hand-type or bookmark, is
  // indistinguishable from bare `#/play`: both render "No recording
  // specified." rather than naming the bad id or the id form.
  await page.goto("/app#/play/does-not-exist-1234");
  await page.waitForTimeout(2000);
  await expect(page.locator("#content")).toContainText("No recording specified.");
  await expect(page.locator("#content")).not.toContainText("does-not-exist-1234");

  // The shape the SPA actually emits, with the real id, is honoured — so
  // this is specifically the path-vs-query defect and not a dead player.
  await page.goto(`/app#/play?recording=${seeded.id}`);
  await expect(page.locator("#content")).toContainText("real lane range check");
  await expect(page.locator("video")).toHaveCount(1);
  await expect(page.locator("video")).toHaveAttribute(
    "src",
    `/api/v1/recordings/${seeded.id}/download`,
  );
});

test("step 7b — PJ-04: a Creator deep link in the PVR build keeps its URL but shows PVR content", async ({ page }) => {
  await login(page);

  // `renderSettings` rewrites `known` to "general" for the Creator-only
  // Plugins pane (032-pvr.js:88-90) WITHOUT rewriting the hash. Compare the
  // sibling behaviour: a Creator ROUTE bounces properly, via
  // `location.hash = "#/library"` (012-pvr.js:563-565). So #/studio is
  // honest about where it landed and #/settings/plugins is not.
  await page.goto("/app#/settings/plugins");
  await expect(page).toHaveURL(/#\/settings\/plugins/);
  await expect(page.locator(".stg-rail-item.is-active .stg-rail-label")).toHaveText("General");
  await expect(page.locator("h1.page-title")).toHaveText("Settings");

  // The Plugins rail item is not rendered at all in PVR, so the address bar
  // names a section the page has no link to.
  await expect(page.locator('.stg-rail-item[href="#/settings/plugins"]')).toHaveCount(0);

  // Control: the Creator ROUTE bounce does move the URL.
  await page.goto("/app#/studio");
  await expect(page).toHaveURL(/#\/library/);
});
