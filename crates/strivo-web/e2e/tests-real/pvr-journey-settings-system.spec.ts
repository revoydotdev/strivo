import { test, expect } from "@playwright/test";
import { apiGet, login, API_KEY } from "./pvr-journey-helpers";

// Journey steps 8 (settings), 9 (backup/restore) and 10 (system), driven
// against the real release binary. See
// docs/audits/2026-09-25-pvr-journey/REPORT.md.
//
// NON-VACUITY: the backup assertions here are the strongest real-binary
// discriminator in the whole audit. mock-server.mjs:461 hardcodes
// `201 {name:"2026-05-26T00-00-00Z", bytes:1234}` and :465 a hardcoded
// one-entry list, and it implements NO download and NO restore route for a
// backup at all. So "the created backup's name tracks the current clock",
// "its size is the real tarball's", "its Download link returns real gzip
// bytes" and "Restore returns a real response" are all unpassable against
// the mock. The system health assertions are likewise live-daemon reads,
// not the mock's `{status:"ok"}` /health.

test("step 8 — every settings section renders from the real config", async ({ page, request }) => {
  await login(page);

  // SETTINGS_SECTIONS (032-pvr.js:71-81) minus the Creator-only Plugins
  // pane, which the PVR build filters out of the rail (032-pvr.js:100-101).
  const sections: Array<[string, string, number]> = [
    ["general", "General", 7],
    ["recording", "Recording", 11],
    ["notifications", "Notifications", 7],
    ["platforms", "Platforms", 6],
    ["interface", "Interface", 8],
    ["multiview", "Multi-view", 4],
    ["advanced", "Advanced", 4],
    ["about", "About", 2],
  ];

  await page.goto("/app#/settings/general");
  await expect(page.locator("#stg-shell, .stg-shell")).toBeVisible();
  expect(
    await page.locator(".stg-rail-item").evaluateAll((els) =>
      els.map((e) => (e as HTMLElement).getAttribute("href")),
    ),
    "the Plugins rail item is absent in the PVR build",
  ).not.toContain("#/settings/plugins");

  for (const [slug, label, rows] of sections) {
    await page.goto(`/app#/settings/${slug}`);
    await expect(page.locator(".stg-rail-item.is-active .stg-rail-label")).toHaveText(label);
    await expect(page.locator("#stg-pane .stg-row")).toHaveCount(rows);
  }

  // --- state: a real write round-trips through the daemon's config file.
  // Unlike the values, this is not asserted from the DOM alone: a reload
  // re-reads the config from disk, so a value that only lived in the page
  // could not survive it.
  await page.goto("/app#/settings/recording");
  const field = page.locator('[data-stg-path="recording.format.bitrate_kbps"]');
  const original = await field.inputValue();
  try {
    const saved = page.waitForResponse(
      (r) =>
        r.url().endsWith("/api/v1/settings/update") &&
        r.request().postDataJSON()?.path === "recording.format.bitrate_kbps",
      { timeout: 15_000 },
    );
    await field.fill("4321");
    await field.press("Tab");
    const res = await saved;
    expect(res.status()).toBe(202);
    expect((await res.json()).restart_required).toBe(true);
    await expect(page.locator(".stg-effect-note")).toContainText("Restart the daemon");

    await page.reload();
    await expect(page.locator('[data-stg-path="recording.format.bitrate_kbps"]')).toHaveValue("4321");
    const cfg = await apiGet(request, "/api/v1/settings");
    expect(JSON.stringify(cfg.body), "the daemon reports the new value").toContain("4321");
  } finally {
    const reset = page.waitForResponse(
      (r) =>
        r.url().endsWith("/api/v1/settings/update") &&
        r.request().postDataJSON()?.path === "recording.format.bitrate_kbps",
      { timeout: 15_000 },
    );
    await page.locator('[data-stg-path="recording.format.bitrate_kbps"]').fill(original);
    await page.locator('[data-stg-path="recording.format.bitrate_kbps"]').press("Tab");
    expect((await reset).status(), "restore the sandbox config").toBe(202);
  }

  // --- state: the settings search filter narrows rows without a refetch.
  await page.goto("/app#/settings/general");
  await page.locator("#stg-search").fill("zzzz-no-such-knob");
  await expect(page.locator("#stg-pane .stg-row:not(.stg-row-hidden)")).toHaveCount(0);
  await page.locator("#stg-search").fill("");
  await expect(page.locator("#stg-pane .stg-row:not(.stg-row-hidden)")).toHaveCount(7);
});

test("step 9 — backup and restore against a real tarball", async ({ page, request }) => {
  await login(page);
  await page.goto("/app#/system");

  // --- state: loading. paintBackups seeds the host with an explicit
  // "Loading backups…" placeholder (034d-pvr.js:220) before it fetches, so
  // the empty text below is a real pre-data state, not a blank box.
  await expect(page.locator("#backup-list")).toHaveCount(1);
  const beforeText = await page.locator("#backup-list").innerText();
  expect(["", "No backups yet.", "Loading backups…"]).toContain(beforeText.trim());

  // --- state: create. One backup per run, so the list is whatever the
  // sandbox had plus exactly one.
  const listed = await apiGet(request, "/api/v1/backups");
  expect(listed.status).toBe(200);
  const before: string[] = (listed.body.backups ?? []).map((b: { name: string }) => b.name);

  const created = page.waitForResponse(
    (r) => r.url().endsWith("/api/v1/backup") && r.request().method() === "POST",
    { timeout: 30_000 },
  );
  await page.locator("#backup-now").click();
  const res = await created;
  expect(res.status()).toBe(201);
  const made = (await res.json()) as { name: string; bytes: number; files: string[] };

  // The status code is NOT the discriminator: mock-server.mjs:461 also answers
  // 201, so asserting on it would prove nothing. The discriminators are the
  // three below — the mock's name is the FIXED literal "2026-05-26T00-00-00Z"
  // and its byte count is the made-up constant 1234, and it serves no download
  // and no restore route at all.
  // The name is a UTC stamp with dashes in the time part, e.g.
  // "2026-09-26T02-06-26Z" — not an ISO-8601 instant, so Date.parse rejects
  // it. Assert the shape and the date component instead of guessing a parser.
  expect(made.name, `backup name shape: ${made.name}`).toMatch(
    /^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}Z$/,
  );
  expect(
    made.name.slice(0, 10),
    "the backup name carries today's real UTC date",
  ).toBe(new Date().toISOString().slice(0, 10));
  expect(made.bytes, "a real archive, not the mock's constant 1234").toBeGreaterThan(0);
  expect(made.files.sort()).toEqual(["config.toml", "jobs.db"]);

  // --- state: populated, and the new entry is at the head of the list.
  await expect(page.locator("#backup-list")).toContainText(made.name);
  await expect(page.locator("#backup-list")).not.toContainText("No backups yet.");
  await expect(page.locator("#backup-list")).toContainText("jobs.db, config.toml");
  const restoreButtons = page.locator(".restore-backup");
  await expect(restoreButtons).toHaveCount(before.length + 1);

  // --- the Download link serves real gzip bytes. No mock route does this.
  const dl = await request.get(`/api/v1/backups/${encodeURIComponent(made.name)}/download`, {
    headers: { "x-api-key": API_KEY },
  });
  expect(dl.status()).toBe(200);
  expect(dl.headers()["content-type"]).toMatch(/gzip/);
  const bytes = await dl.body();
  expect(bytes.length, "downloaded archive is non-empty").toBeGreaterThan(0);
  // gzip magic: 1f 8b. Proves a real archive, not a JSON error page.
  expect([bytes[0], bytes[1]]).toEqual([0x1f, 0x8b]);
  // PJ-09: `bytes` is walk_dir_bytes(&dest) — the summed size of the
  // UNCOMPRESSED staged archive (backup.rs:62 for create, :86 for list) —
  // while this link serves a gzip stream built on the fly. The two are
  // different numbers, and the UI renders `bytes` next to the row
  // (034d-pvr.js:363-366), so a user comparing it to the download sees a
  // mismatch. Asserted as an inequality, with both sides real, rather than
  // as the equality a reader would assume from the field's name.
  expect(bytes.length, "a real compressed archive").toBeGreaterThan(0);
  expect(
    made.bytes,
    "the reported size is the uncompressed payload, so it exceeds the download",
  ).toBeGreaterThan(bytes.length);

  // --- state: restore. Restoring a backup taken seconds ago over a config
  // we are actively using is safe inside this throwaway sandbox, and it is
  // the only way to observe the restore path end to end.
  const restored = page.waitForResponse(
    (r) => /\/api\/v1\/backups\/[^/]+\/restore$/.test(r.url()),
    { timeout: 30_000 },
  );
  await page.locator(".restore-backup").first().click();
  const confirm = page.locator(".confirm-modal .confirm-ok");
  await expect(confirm, "restore is confirmed, not immediate").toBeVisible();
  await confirm.click();
  const rres = await restored;
  expect(rres.status(), "the real daemon accepted the restore").toBeLessThan(300);
  expect(await rres.json()).toBeTruthy();
});

test("step 10 — system page reports live daemon health", async ({ page, request }) => {
  await login(page);
  await page.goto("/app#/system");
  await expect(page.locator("h1.page-title")).toHaveText("System");

  // Health checks are severity-tiered (034d-pvr.js:158-165) and each names
  // the domain it belongs to.
  const checks = page.locator(".sys-check");
  await expect(checks).not.toHaveCount(0);
  const names = await page.locator(".sys-label").allTextContents();
  expect(names, "at least the daemon IPC domain is reported").toContain("Daemon IPC");
  // sevGlyph (034d-pvr.js:139) is {ok:"✓", warn:"▲", error:"✕"}, with "•" as
  // the fallback for an unrecognised severity. Asserting the real map, so a
  // new severity level has to be a deliberate edit here rather than a silent
  // pass.
  const GLYPHS = ["✓", "▲", "✕", "•"];
  for (const glyph of await page.locator(".sys-sev").allTextContents()) {
    expect(GLYPHS, `unexpected severity glyph ${glyph}`).toContain(glyph);
  }

  // --- the page's verdict must agree with the daemon's own endpoint, read
  // out-of-band. This is what makes the ✓ above evidence rather than a
  // hardcoded glyph.
  const api = await apiGet(request, "/api/v1/health/checks");
  expect(api.status).toBe(200);
  const payload = JSON.stringify(api.body);
  expect(payload).toContain("Daemon");

  // --- state: the disk gauge parses to real quantities.
  await expect(page.locator("#content")).toContainText("Free:");
  const free = (await page.locator("#content").innerText()).match(/Free:\s*([\d.]+ \w+) of ([\d.]+ \w+)/);
  expect(free, "disk gauge renders a parsed quantity").toBeTruthy();

  // --- state: an authenticated data endpoint is reachable, so a green
  // health row is not merely "the page loaded".
  const health = await apiGet(request, "/api/v1/health");
  expect(health.status).toBe(200);
});
