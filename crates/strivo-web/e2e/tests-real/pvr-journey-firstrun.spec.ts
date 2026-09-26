import { test, expect } from "@playwright/test";
import { apiGet, login, API_KEY } from "./pvr-journey-helpers";

// Journey steps 1 (first run) and 2 (platform wizard), driven against the
// real release binary. Findings: PJ-01, PJ-02, PJ-05. See
// docs/audits/2026-09-25-pvr-journey/REPORT.md.
//
// NON-VACUITY: real-server.sh boots with no platform configured and no
// channel in the daemon snapshot, so this lane starts in the genuine
// first-run state. mock-server.mjs:516 returns a NON-EMPTY channel
// fixture ("Live Channel", is_live:true) and has no /channels/resolve or
// /settings/platform route at all, so the empty-rail and first-run-gate
// assertions below cannot pass against the mock.

test("step 1 — first run shows the setup gate and hides real recordings", async ({ page, request }) => {
  await login(page);

  // --- state: the first-run gate itself.
  // renderFirstRun (012b-pvr.js:8-63) is reached from renderHome's
  // `!anyPlatform && !firstRunDismissed` branch (012b-pvr.js:92-98).
  await expect(page).toHaveURL(/#\/library/);
  await expect(page.locator("h1.page-title")).toHaveText("Welcome to StriVo");
  await expect(page.locator(".fr-step")).toHaveCount(3);
  await expect(page.locator(".fr-label")).toHaveText([
    "1 · Connect a platform",
    "2 · Recording directory",
    "3 · Pick channels to record",
  ]);
  // Exactly ONE step is already complete, and it is not an accident:
  // real-server.sh's sandbox has a recording_dir, so step 2 reads `done`
  // while steps 1 and 3 read `todo`. Asserting "all three un-ticked" would
  // be wrong about this build, and asserting nothing here would let a gate
  // that marks a step done unconditionally pass unnoticed.
  await expect(page.locator(".fr-step")).toHaveCount(3);
  await expect(page.locator(".fr-step .fr-label")).toHaveText([
    "1 · Connect a platform",
    "2 · Recording directory",
    "3 · Pick channels to record",
  ]);
  await expect(page.locator(".fr-step.todo")).toHaveCount(2);
  await expect(page.locator(".fr-step.done")).toHaveCount(1);
  await expect(page.locator(".fr-step.done .fr-label")).toHaveText("2 · Recording directory");
  // Array form: a single-string toHaveText on a multi-element locator is a
  // strict-mode violation, and "expect 2 elements to have one text" is not a
  // meaningful assertion anyway.
  await expect(page.locator(".fr-step .fr-mark")).toHaveText(["\u25cb", "\u2713", "\u25cb"]);
  await expect(page.locator(".fr-pill.ok"), "no platform pill is ticked").toHaveCount(0);
  await expect(page.locator("#fr-continue")).toHaveText("Continue anyway");

  // --- state: empty rail. Cross-checked against a direct HTTP read so this
  // cannot be a `_cacheTtlMs` hit. Genuinely empty, not a failed fetch.
  await expect(page.locator("#channel-list .ch-empty")).toContainText("No channels yet.");
  const channels = await apiGet(request, "/api/v1/channels");
  expect(channels.status).toBe(200);
  expect(channels.body.channels, "real daemon snapshot is empty").toEqual([]);

  // --- PJ-05: the recordings dashboard is unreachable behind the gate even
  // though renderHome had ALREADY fetched recordings before returning
  // (012b-pvr.js:86-88) and discards them at the early return (`:95-98`,
  // before the recRes handling at `:114-118`).
  //
  // The proof that a real recording exists is an authoritative read, not the
  // DOM: the row is real ffmpeg output on disk.
  const recs = await apiGet(request, "/api/v1/recordings");
  expect(recs.status).toBe(200);
  expect(recs.body.recordings.length, "a real recording IS on disk").toBeGreaterThan(0);
  await expect(page.locator("[data-dashboard-recording]"), "hidden behind the first-run gate").toHaveCount(0);
  await expect(page.locator("#dash"), "dashboard subtree is never mounted").toHaveCount(0);

  // --- state: the gate's own escape hatch reveals the same recording.
  // This is the reproduction half of PJ-05: nothing about the data changed,
  // only the gate was dismissed.
  await page.locator("#fr-continue").click();
  await expect(page.locator("h1.page-title")).toHaveText("Home");
  await expect(page.locator("[data-dashboard-recording]")).toHaveCount(
    recs.body.recordings.length,
  );
  await expect(page.locator("#dash")).toContainText("In progress");
  await expect(page.locator("#dash")).toContainText("Nothing recording"); // empty in-flight section
  await expect(page.locator('[data-action="play"]')).toHaveCount(1);

  // --- state: re-check is a live refetch, not a cached repaint.
  // "Re-check" (012b-pvr.js:58) re-enters renderHome; the gate is gone for
  // the session because firstRunDismissed is a module-scope flag.
  await page.locator(".topnav-link[data-route='library']").click();
  await page.locator(".topnav-link[data-route='recordings']").click();
  await page.locator(".topnav-link[data-route='library']").click();
  await expect(page.locator("h1.page-title")).toHaveText("Home");
  await expect(page.locator(".fr-card")).toHaveCount(0);
});

test("step 2 — platform wizard: validation, false success, and dismissal", async ({ page, request }) => {
  await login(page);
  await page.goto("/app#/settings/platforms");

  // --- state: the pane lists all three platforms with Configure buttons.
  // PLATFORM_SPECS + the pane loop live in 034d-pvr.js:7-40 / :527.
  await expect(page.locator(".stg-cfg-btn")).toHaveCount(3);
  const platforms = await page.locator(".stg-cfg-btn").evaluateAll((els) =>
    els.map((e) => (e as HTMLElement).dataset.platform),
  );
  expect(platforms.sort()).toEqual(["patreon", "twitch", "youtube"]);

  // --- PJ-02: Escape does NOT dismiss this modal.
  // openPlatformWizard (034d-pvr.js:44-84) wires only `.modal-close`,
  // `.modal-cancel` and a backdrop click. The document-level Escape
  // handler (012d-pvr.js:41-43) calls closeAllAppModals(), which queries
  // `.app-modal.open` — and this dialog is `.modal-backdrop`, so the
  // handler can never reach it. The control assertion below is what makes
  // this a finding rather than "Escape is broken in this browser".
  await page.locator('.stg-cfg-btn[data-platform="twitch"]').click();
  await expect(page.locator(".modal")).toBeVisible();
  await expect(page.locator("#pf-title")).toHaveText("Configure Twitch");
  await page.keyboard.press("Escape");
  await page.waitForTimeout(500);
  await expect(page.locator(".modal"), "platform wizard survives Escape").toHaveCount(1);

  // CONTROL: the Add-channel modal DOES close on Escape, in the same page,
  // in the same session. Without this the assertion above is unfalsifiable.
  await page.locator(".modal-close").click();
  await expect(page.locator(".modal")).toHaveCount(0);
  await page.goto("/app#/library");
  await expect(page.locator("#add-channel")).toBeVisible();
  await page.locator("#add-channel").click();
  await expect(page.locator("#add-channel-modal.open")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(
    page.locator("#add-channel-modal.open"),
    "control: .app-modal modals DO close on Escape",
  ).toHaveCount(0);

  // Back to the platforms pane, which the leg above navigated away from.
  await page.goto("/app#/settings/platforms");
  await expect(page.locator(".stg-cfg-btn")).toHaveCount(3);

  // --- state: partial input is blocked by native validation, not by the app.
  // The inputs carry `required` (034d-pvr.js:56-59) and the form is never
  // submitted, so the only feedback is the browser's own bubble.
  await page.locator('.stg-cfg-btn[data-platform="twitch"]').click();
  await expect(page.locator(".modal")).toBeVisible();
  const submit = page.locator(".modal button[type=submit]");
  await submit.click();
  await page.waitForTimeout(800);
  await expect(page.locator(".modal"), "empty submit is blocked").toHaveCount(1);
  const validity = await page
    .locator('input[name="client_id"]')
    .evaluate((e) => ({ missing: (e as HTMLInputElement).validity.valueMissing, msg: (e as HTMLInputElement).validationMessage }));
  expect(validity.missing, "the app relies on native required-ness").toBe(true);
  expect(validity.msg).toBe("Please fill out this field.");

  // --- PJ-01 (false success): the real daemon accepts junk credentials.
  // This is the headline finding. openPlatformWizard branches ONLY on
  // `res.applied === false` (034d-pvr.js:96-104) and treats everything
  // else as success, so a 202 with `applied:true` from a daemon that never
  // contacted Twitch produces a green "Twitch saved" toast.
  const junk = { client_id: "not-a-real-client-id", client_secret: "not-a-real-secret" };
  const saved = page.waitForResponse(
    (r) => r.url().endsWith("/api/v1/settings/platform/twitch"),
    { timeout: 15_000 },
  );
  await page.locator('input[name="client_id"]').fill(junk.client_id);
  await page.locator('input[name="client_secret"]').fill(junk.client_secret);
  await submit.click();
  const res = await saved;
  expect(res.status()).toBe(202);
  const body = await res.json();
  // The server's own claim: configured, applied, ok — for credentials that
  // cannot possibly work. This assertion is the defect.
  expect(body).toMatchObject({ ok: true, applied: true, configured: true, platform: "twitch" });
  await expect(page.locator(".modal")).toHaveCount(0);
  await expect(page.locator(".toast-msg")).toContainText("Twitch saved");

  // The lie is persisted: /settings now reports the platform as configured,
  // which is also what un-gates the first-run checklist (012b-pvr.js:92-95).
  const cfg = await apiGet(request, "/api/v1/settings");
  expect(cfg.body.twitch_configured, "junk creds flipped the configured flag").toBe(true);

  // --- the false success has a reachable consequence: the first-run gate
  // now passes on a setup that does not work. Repro of PJ-01's blast radius.
  await page.goto("/app#/library");
  await expect(page.locator(".fr-card"), "gate no longer fires").toHaveCount(0);
  await expect(page.locator("h1.page-title")).toHaveText("Home");

  // --- PJ-01b: the false success is NOT reversible through the product.
  // Two candidate un-configure paths exist and the real daemon refuses both,
  // so once a user has saved credentials that do not work, the only way back
  // is editing config.toml by hand. That turns PJ-01 from a wrong message
  // into a wrong state with no in-app remedy.
  const viaUpdate = await request.post("/api/v1/settings/update", {
    headers: { "x-api-key": API_KEY, "content-type": "application/json" },
    data: { path: "twitch.client_id", value: "" },
  });
  expect(
    viaUpdate.status(),
    "settings/update cannot clear a platform credential",
  ).toBe(400);

  const viaPlatform = await request.post("/api/v1/settings/platform/twitch", {
    headers: { "x-api-key": API_KEY, "content-type": "application/json" },
    data: { client_id: "", client_secret: "" },
  });
  expect(
    viaPlatform.status(),
    "the platform endpoint rejects empty credentials rather than clearing them",
  ).toBe(400);
  expect(await viaPlatform.text()).toContain("client_id and client_secret are required");

  const after = await apiGet(request, "/api/v1/settings");
  expect(
    after.body.twitch_configured,
    "the lie survives both attempted repairs",
  ).toBe(true);
});
