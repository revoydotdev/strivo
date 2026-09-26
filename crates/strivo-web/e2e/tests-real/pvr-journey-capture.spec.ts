import { test, expect } from "@playwright/test";
import { apiGet, login, API_KEY } from "./pvr-journey-helpers";

// Journey steps 3 (add channel), 4 (monitor) and 5 (record), driven against
// the real release binary. Findings: PJ-06. See
// docs/audits/2026-09-25-pvr-journey/REPORT.md.
//
// NON-VACUITY: mock-server.mjs has NO /api/v1/channels/resolve route and NO
// /api/v1/channels/:key/auto_record route (only :992, a read-only vods
// match), so the "Search failed" transition, the real daemon error string,
// and the config-vs-snapshot divergence asserted below are all impossible
// against the mock. mock-server.mjs:516 also returns a non-empty channel
// fixture, so every "no channels" assertion here fails against it.

test("step 3 — add-channel wizard surfaces the real daemon error, and phase 2 never arrives", async ({ page, request }) => {
  await login(page);
  await page.goto("/app#/library");
  await expect(page.locator("#add-channel")).toBeVisible();

  // --- state: closed. The modal is created lazily on first open
  // (012d-pvr.js:7-21), so its mere absence is the closed state.
  await expect(page.locator("#add-channel-modal")).toHaveCount(0);

  await page.locator("#add-channel").click();
  await expect(page.locator("#add-channel-modal.open")).toBeVisible();
  await expect(page.locator(".wizard-step")).toHaveText("Step 1 of 2 — find the channel");
  expect(await page.locator("#aw-platform option").allTextContents()).toEqual([
    "Twitch",
    "YouTube",
    "Patreon",
  ]);
  // --- state: loading. `#aw-result` is replaced synchronously with
  // "Searching…" (012d-pvr.js:72) before the request is even issued.
  expect((await page.locator("#aw-result").innerText()).trim()).toBe("");

  // --- state: partial input. doSearch returns early on an empty query
  // (012d-pvr.js:70), so nothing at all happens — no error, no progress.
  await page.locator("#aw-search").click();
  await page.waitForTimeout(400);
  expect((await page.locator("#aw-result").innerText()).trim()).toBe("");

  // --- state: loading -> failure, driven by a REAL daemon error frame.
  // POST /channels/resolve returns 202 (api.rs:216-240) and the verdict
  // arrives as a `ChannelResolved` SSE frame; paintAddWizardConfirm
  // (012d-pvr.js:87-96) renders `ev.error` as "Not found: …".
  //
  // The resolve is delayed on purpose. Undelayed, the local daemon answers
  // faster than a round trip can observe, so "Searching…" would be a
  // coin-flip assertion rather than a state — the exact kind of assertion
  // that looks green and proves nothing. The delay is on the RESPONSE, so
  // the loading state is real product behaviour, not an injected fixture.
  await page.route("**/api/v1/channels/resolve", async (route) => {
    await new Promise((r) => setTimeout(r, 1500));
    await route.continue();
  });
  await page.locator("#aw-query").fill("definitely-not-a-real-channel-xyz");
  const requested = page.waitForResponse(
    (r) => r.url().endsWith("/api/v1/channels/resolve"),
    { timeout: 15_000 },
  );
  await page.locator("#aw-search").click();
  // The loading state, now deterministically observable.
  await expect(page.locator("#aw-result")).toContainText("Searching…");
  expect((await requested).status(), "resolve is accepted, verdict is async").toBe(202);
  // The real string. It names the daemon's own missing config section, so it
  // cannot be produced by any stub.
  await expect(page.locator("#aw-result")).toContainText("twitch section missing in config", {
    timeout: 20_000,
  });
  await expect(page.locator("#aw-result")).toContainText("Not found:");
  await page.unroute("**/api/v1/channels/resolve");

  // --- state: phase 2 is UNREACHABLE in this build/sandbox, and that is
  // itself the finding. paintAddWizardConfirm only renders `#aw-confirm`
  // when a resolve succeeds; the daemon cannot resolve without platform
  // credentials, and the only way to obtain those is the wizard in
  // pvr-journey-firstrun.spec.ts step 2, which accepts but does not
  // validate them (PJ-01). The button therefore never exists here.
  await expect(
    page.locator("#aw-confirm"),
    "phase 2 cannot be reached without a working platform resolve",
  ).toHaveCount(0);

  // --- state: ESC does close this modal (the control for PJ-02).
  await page.keyboard.press("Escape");
  await expect(page.locator("#add-channel-modal.open")).toHaveCount(0);
});

test("step 3b — an auto-record channel in the config is invisible to the channel list", async ({ page, request }) => {
  await login(page);

  // A channel CAN be added out-of-band: PUT /channels/{key}/auto_record
  // writes `config.auto_record_channels` directly (blocklist.rs:132-160).
  // That is the only way to create a channel without platform credentials,
  // so this is the state a user reaches by hand-editing config.toml.
  const key = "Twitch:12345";
  const put = await request.put(`/api/v1/channels/${encodeURIComponent(key)}/auto_record`, {
    headers: { "x-api-key": API_KEY, "content-type": "application/json" },
    data: { enabled: true },
  });
  expect(put.status()).toBe(200);

  try {
    // The write landed…
    const cfg = await apiGet(request, "/api/v1/settings");
    expect(
      cfg.body.auto_record_channels.map((c: { platform: string; channel_id: string }) => `${c.platform}:${c.channel_id}`),
      "the auto-record channel is in the config",
    ).toContain(key);

    // …and PJ-06: the channel list still does not contain it.
    // GET /api/v1/channels returns `ServerMessage::StateSnapshot.channels`
    // and nothing else (settings.rs:42-53) — the daemon's in-memory
    // channel model, which the auto_record write never touches. A channel
    // that is genuinely configured for recording is therefore absent from
    // the rail, from Monitor, and from the watch tiles, and there is no
    // `data-action="record"` button to start a capture from.
    const channels = await apiGet(request, "/api/v1/channels");
    expect(channels.status).toBe(200);
    expect(channels.body.channels, "config says yes, the snapshot says nothing").toEqual([]);

    await page.reload();
    await page.waitForTimeout(2500);
    await expect(page.locator("#channel-list .ch-empty")).toContainText("No channels yet.");
    await expect(page.locator('[data-action="record"]'), "no channel, no capture button").toHaveCount(0);
    await expect(page.locator('[data-action="auto-record"]')).toHaveCount(0);

    // Monitor states the opposite of the truth in its own copy: it promises
    // "Channels StriVo is watching" while listing none. Asserted on the
    // daemon's own answer rather than a DOM negative, because a `not.` DOM
    // assertion on a re-rendering page is a race, and the API read above
    // already establishes the fact.
    await page.goto("/app#/schedule");
    await expect(page.locator("h1.page-title")).toHaveText("Monitor");
    await expect(page.locator("#content")).toContainText("Channels StriVo is watching");
    const stillNothing = await apiGet(request, "/api/v1/channels");
    expect(stillNothing.body.channels, "still invisible after a full reload").toEqual([]);
  } finally {
    const off = await request.put(`/api/v1/channels/${encodeURIComponent(key)}/auto_record`, {
      headers: { "x-api-key": API_KEY, "content-type": "application/json" },
      data: { enabled: false },
    });
    expect(off.status(), "restore the sandbox config").toBe(200);
  }
});

test("step 4 — monitor reports live daemon figures and an empty in-flight set", async ({ page, request }) => {
  await login(page);
  await page.goto("/app#/schedule");

  // --- state: populated. The disk figure is read from the live daemon, so
  // it must parse as a real quantity and must match what /health/checks says.
  await expect(page.locator("h1.page-title")).toHaveText("Monitor");
  await expect(page.locator("#content")).toContainText("0 recordings in flight");
  await expect(page.locator("#content")).toContainText("Max concurrent recordings");

  const free = (await page.locator("#content").innerText()).match(/([\d.]+) GB free of ([\d.]+) GB/);
  expect(free, "the disk gauge renders a real parsed quantity").toBeTruthy();
  expect(Number(free![1])).toBeGreaterThan(0);
  expect(Number(free![2])).toBeGreaterThan(Number(free![1]));

  // Cross-check the rendered figure against the daemon, not against the
  // page: this is what rules out a hardcoded or cached read.
  const checks = await apiGet(request, "/api/v1/health/checks");
  expect(checks.status).toBe(200);
  const disk = JSON.stringify(checks.body);
  expect(disk, "the rendered GB figure originates in the daemon's own checks").toContain("free");

  // --- state: the safety knobs are real settings rows, and editable ones
  // round-trip through the real config endpoint.
  const knob = page.locator('[data-stg-path="recording.max_concurrent"]');
  if (await knob.count()) {
    const before = await knob.inputValue();
    const saved = page.waitForResponse(
      (r) => r.url().endsWith("/api/v1/settings/update"),
      { timeout: 15_000 },
    );
    await knob.fill("2");
    await knob.press("Tab");
    expect((await saved).status()).toBe(202);
    await page.reload();
    await expect(page.locator('[data-stg-path="recording.max_concurrent"]')).toHaveValue("2");
    const restore = page.waitForResponse(
      (r) => r.url().endsWith("/api/v1/settings/update"),
      { timeout: 15_000 },
    );
    await page.locator('[data-stg-path="recording.max_concurrent"]').fill(before);
    await page.locator('[data-stg-path="recording.max_concurrent"]').press("Tab");
    expect((await restore).status()).toBe(202);
  }
});

test("step 5 — recording cannot be started from the UI without a channel", async ({ page, request }) => {
  await login(page);

  // There is no channel, therefore no capture affordance. This is a
  // reachability statement with a code citation, not a silent omission:
  // the buttons are rendered per channel in channelDetailHtml
  // (012c-pvr.js:11,17) and the per-card auto-record toggle at :25, all of
  // which live inside a channel detail that only exists for a selected
  // channel. `selectedChannelKey` is null on a fresh session.
  await page.goto("/app#/library");
  await expect(page.locator("h1.page-title")).toHaveText("Welcome to StriVo");
  await page.locator("#fr-continue").click();
  await expect(page.locator("h1.page-title")).toHaveText("Home");
  await expect(page.locator('[data-action="record"]')).toHaveCount(0);
  await expect(page.locator('[data-action="stop"]'), "nothing is recording, so no Stop either").toHaveCount(0);

  // What IS reachable is recording *history*: the real seeded capture, with
  // a real on-disk file behind it.
  await page.goto("/app#/recordings");
  await expect(page.locator(".recordings-table")).toBeVisible();
  const recs = await apiGet(request, "/api/v1/recordings");
  await expect(page.locator("#rec-body tr")).toHaveCount(recs.body.recordings.length);
});
