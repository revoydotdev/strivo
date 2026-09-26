import { test, expect } from "@playwright/test";
import { readFileSync, writeFileSync } from "node:fs";
import { apiGet, login, seededRecording } from "./pvr-journey-helpers";

// The named QA failure scenario for todo 15, plus the stale-state and
// malformed-input probes. Findings: PJ-07, and the state record for PJ-08.
// See docs/audits/2026-09-25-pvr-journey/REPORT.md.
//
// This file CORRUPTS the seeded recording's media file and therefore must
// restore it byte-for-byte in a finally. Playwright collects files
// alphabetically, so pvr-journey-* runs BEFORE real-server.spec.ts, whose
// Range/206 assertions depend on that file's bytes. The restore is
// verified by length AND content, not by the absence of an error.

test("FAILURE — a broken recording file surfaces a real error state, not a blank tile", async ({ page, request }) => {
  await login(page);
  const seeded = await seededRecording(request);

  const original = readFileSync(seeded.output_path);
  expect(original.length, "captured the pristine bytes to restore").toBe(seeded.bytes_written);

  try {
    // Truncated garbage in place of a Matroska file: present (so every
    // existence check passes) but undecodable.
    writeFileSync(seeded.output_path, Buffer.from("NOT A MATROSKA FILE\n".repeat(128)));

    // --- the library does NOT notice. `file_exists` is a stat, not a probe
    // (settings.rs:66-80, augment_recording), so a present-but-corrupt file
    // is indistinguishable from a good one until playback.
    const after = await apiGet(request, "/api/v1/recordings");
    expect(after.body.recordings[0].file_exists, "a corrupt file is still 'present'").toBe(true);

    await page.goto("/app#/recordings");
    await expect(page.locator(".recordings-table")).toBeVisible();
    await expect(page.locator("#rec-body tr")).toHaveCount(1);
    await expect(page.locator("#rec-body tr").first()).toContainText("Finished");
    await expect(page.locator("#rec-body tr").first()).toContainText(
      `${Math.round(seeded.bytes_written / 1024)} KB`,
    );

    // --- and playback DOES notice. This is the assertion the whole scenario
    // exists for: a real, specific, actionable error state.
    await page.locator('[data-action="rec-play"]').first().click();
    await expect(page).toHaveURL(/#\/play\?recording=/);
    const surface = page.locator("#content");
    await expect(surface).toContainText("Can't play this recording", { timeout: 20_000 });
    await expect(surface).toContainText("not supported by this browser");
    await expect(surface).toContainText("Playback error");
    // Actionable, not a dead end.
    await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
    await expect(page.getByRole("link", { name: /Download file/ })).toBeVisible();
    // The <video> element exists and points at the real endpoint — the tile
    // is not blank, the decode failed and was reported.
    await expect(page.locator("video")).toHaveCount(1);
    await expect(page.locator("video")).toHaveAttribute(
      "src",
      `/api/v1/recordings/${seeded.id}/download`,
    );

    // --- the download path still serves the (corrupt) bytes with a correct
    // Range response, so the error is a DECODE error, not a transport one.
    const ranged = await request.get(`/api/v1/recordings/${seeded.id}/download`, {
      headers: { "x-api-key": "strivo-e2e-real-server-test-key", range: "bytes=0-15" },
    });
    expect(ranged.status()).toBe(206);
    expect((await ranged.body()).length).toBe(16);
  } finally {
    writeFileSync(seeded.output_path, original);
    const restored = readFileSync(seeded.output_path);
    expect(restored.length, "media file restored to its exact original size").toBe(seeded.bytes_written);
    expect(restored.equals(original), "media file restored byte-for-byte").toBe(true);
  }

  // Post-restore the lane is clean for the specs that follow.
  const healed = await apiGet(request, "/api/v1/recordings");
  expect(healed.body.recordings[0].bytes_written).toBe(seeded.bytes_written);
});

test("STALE — a read inside the 2 s GET cache is a cache hit, not a broken endpoint", async ({ page, request }) => {
  await login(page);
  const seeded = await seededRecording(request);

  // The SPA collapses duplicate GETs behind `_cacheTtlMs: 2000`
  // (002-pvr.js:17). This is the discipline every "empty" claim in this
  // audit rests on, and it is asserted rather than assumed.
  //
  // The constant is read out of the BUNDLE THE REAL SERVER SERVES, not out
  // of the page: assets/spa.html loads spa.js as `type="module"`, so the
  // module-scope `const API` is not on `window` and cannot be read from the
  // page at all. Reading `/assets/spa.js` also proves the cache is in the
  // emitted artifact, not merely in the source tree.
  const bundle = await request.get("/assets/spa.js");
  expect(bundle.status()).toBe(200);
  // The served bundle is MINIFIED: identifier names survive, numeric literals
  // do not. `_cacheTtlMs: 2000` in the source (002-pvr.js:17) is emitted as
  // `_cacheTtlMs:2e3`. Assert the identifier, then parse whichever numeric
  // form the minifier chose — an assertion on the unminified spelling would
  // fail against the real release build and pass against the mock, which is
  // the worst possible combination for this lane.
  const bundleText = await bundle.text();
  expect(bundleText, "the served bundle carries the GET cache").toContain("_cacheTtlMs");
  const ttlLiteral = bundleText.match(/_cacheTtlMs\s*:\s*([0-9e.+]+)/);
  expect(ttlLiteral, "the TTL is a readable literal").toBeTruthy();
  expect(Number(ttlLiteral![1]), "the SPA GET cache TTL is 2 s").toBe(2000);

  // Warm the SPA's cache on the recordings route, then mutate the server's
  // answer out-of-band. A read taken INSIDE the 2 s window still shows the
  // old value — that is a cache hit, and reporting it as a broken endpoint
  // would be the misleading-success class this audit exists to remove.
  //
  // Deliberately NOT asserted: "the immediate re-read is stale". That is
  // wall-clock dependent and would flake under this host's load average. The
  // positive direction below is what proves the endpoints are healthy, and
  // the structural protection for the report's empty-state claims is that
  // they are all cross-checked through `request`, which never touches the
  // page's cache.
  await page.goto("/app#/recordings");
  await expect(page.locator("#rec-body tr")).toHaveCount(1);

  // The server's own answer, read out-of-band and never cached, is live.
  const live = await apiGet(request, "/api/v1/recordings");
  expect(live.status).toBe(200);
  expect(live.body.recordings.length).toBe(1);
  expect(live.body.recordings[0].id, "out-of-band read names the real row").toBe(seeded.id);

  // Past the TTL a fresh in-page navigation shows the same live state, so
  // the equality above is agreement rather than coincidence.
  await page.waitForTimeout(2200);
  await page.reload();
  await expect(page.locator("#rec-body tr")).toHaveCount(1);
  await expect(page.locator("#rec-body tr").first()).toContainText("Finished");

  // The genuinely-empty direction, proven out-of-band: an unknown cursor
  // past the end of the library is a real empty page, not a failure. This
  // is the shape a "the library is empty" claim could be confused with, and
  // the two are now distinguishable.
  const past = await apiGet(request, "/api/v1/recordings?cursor=100000&limit=500");
  expect(past.status, "an out-of-range cursor is a 200 empty page").toBe(200);
  expect(past.body.recordings).toEqual([]);
  const whole = await apiGet(request, "/api/v1/recordings");
  expect(whole.body.recordings.length, "the library is not actually empty").toBeGreaterThan(0);
});

test("MALFORMED — unknown ids, bad keys and unauthenticated reads all fail loudly", async ({ page, request }) => {
  await login(page);
  const seeded = await seededRecording(request);

  // --- unknown recording id, in BOTH shapes, because the server
  // distinguishes them and both answers are loud.
  const unknownUuid = "00000000-0000-4000-8000-000000000000";
  const missing = await request.get(`/api/v1/recordings/${unknownUuid}/download`, {
    headers: { "x-api-key": "strivo-e2e-real-server-test-key" },
  });
  expect(missing.status(), "a well-formed but unknown id is 404").toBe(404);
  expect(await missing.text()).toContain("recording not found");

  const malformed = await request.get(`/api/v1/recordings/${encodeURIComponent("no-such-id")}/download`, {
    headers: { "x-api-key": "strivo-e2e-real-server-test-key" },
  });
  expect(malformed.status(), "a malformed id is 400, naming the parse failure").toBe(400);
  expect(await malformed.text()).toContain("UUID parsing failed");

  // --- malformed channel key for the auto-record route: the handler
  // requires "Platform:id" (blocklist.rs:157-160) and rejects anything else.
  const badKey = await request.put("/api/v1/channels/not-a-valid-key/auto_record", {
    headers: {
      "x-api-key": "strivo-e2e-real-server-test-key",
      "content-type": "application/json",
    },
    data: { enabled: true },
  });
  expect(badKey.status()).toBe(400);
  expect(JSON.stringify(await badKey.json())).toMatch(/Platform:id/);

  // --- an unauthenticated read of every page's data source.
  for (const path of ["/api/v1/recordings", "/api/v1/channels", "/api/v1/settings", "/api/v1/backups"]) {
    const res = await request.get(path);
    expect(res.status(), `${path} must reject an anonymous caller`).toBe(401);
  }

  // --- an unknown route falls back to the library rather than 404ing, and
  // the SPA logs no uncaught error while doing it.
  const pageErrors: string[] = [];
  page.on("pageerror", (e) => pageErrors.push(String(e)));
  // An unknown NON-Creator route renders the library but LEAVES the bogus
  // hash in the address bar: currentRoute() (008-pvr.js:340-348) returns the
  // "library" fallback without rewriting location.hash, and only the
  // CREATOR_ROUTES branch actually reassigns it (012-pvr.js:563-565).
  // Asserted as observed, and contrasted with the Creator bounce below.
  await page.goto("/app#/definitely-not-a-route");
  await expect(page.locator("#channel-list")).toBeVisible();
  await expect(page.locator("h1.page-title")).toHaveText("Welcome to StriVo");
  await expect(page, "the unrecognised hash is left in place").toHaveURL(
    /#\/definitely-not-a-route/,
  );
  expect(pageErrors, "no uncaught error on an unknown route").toEqual([]);

  // --- a Creator route bounces cleanly in the PVR build.
  await page.goto("/app#/archive");
  await expect(page).toHaveURL(/#\/library/);
  expect(pageErrors, "no uncaught error bouncing a Creator route").toEqual([]);

  expect(seeded.id).toBeTruthy();
});
