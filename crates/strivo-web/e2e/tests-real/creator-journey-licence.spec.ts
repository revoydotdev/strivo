import { expect, test } from "@playwright/test";
import {
  API_KEY,
  apiJson,
  descriptor,
  login,
  openCreatorSurfaces,
  requireCreatorEdition,
  servedBundle,
} from "./creator-journey-helpers";

// Journey step 1 — LICENCE, and the gate every later step passes through.
// Driven against a real **Creator-featured** binary. See
// docs/audits/2026-09-25-pro-journey/REPORT.md (findings PJ-C01..PJ-C04).
//
// NON-VACUITY, stated before the first assertion:
//  * `descriptor()` reads through Playwright's `request` context, not the
//    page, so `API._fetch`'s 2 s GET cache (_cacheTtlMs, 002-pvr.js:17) can
//    never turn a broken endpoint into a green one.
//  * A PVR binary answers /api/v1/plugins/descriptor with `edition:"pvr"` and
//    an EMPTY capability list, so the first test fails on a PVR build rather
//    than skipping. These assertions cannot pass against a binary that does
//    not contain the Creator surface.

test.beforeEach(async ({ request }) => {
  await requireCreatorEdition(request);
});

test("the binary under test is Creator-featured, by served content", async ({ request }) => {
  const d = await descriptor(request);

  // The edition, as the server states it.
  expect(d.edition).toBe("creator");
  expect(d.version).toBe(1);

  // Non-emptiness, so the rest of the register is about real rows. A PVR
  // descriptor is 0 rows by construction (capabilities.rs:604), which is why
  // this is an edition check and not a length check on its own.
  expect(d.capabilities.length).toBeGreaterThan(30);
  expect(d.capabilities.length, "one row per CapabilityKey variant").toBe(39);

  // Every served row belongs to the edition that serves it
  // (capabilities.rs:902-906). If a Creator row leaked into a PVR build this
  // would be the assertion that catches it.
  expect(d.capabilities.filter((c) => c.edition === "pvr")).toHaveLength(0);

  // The Pro set is DERIVED (capabilities.rs:228-234), not hand-kept, and
  // todo 27 landed it at four. Asserting the exact set is the point: if a
  // future lane flips a row to Pro, this fails and says the pricing decision
  // was made.
  expect(
    d.capabilities.filter((c) => c.entitlement === "pro").map((c) => c.key).sort(),
  ).toEqual(["archiver", "crunchr", "insights", "viewguard"]);

  // `yt-publish` is the reserved roadmap slot — the row that proves the
  // descriptor is a superset of the gates rather than equal to them
  // (capabilities.rs:446-457).
  const yt = d.capabilities.find((c) => c.key === "yt-publish");
  expect(yt, "yt-publish must be represented, not absent").toBeTruthy();
  expect(yt!.entry_point).toBe("roadmap");
  expect(yt!.installed, "a roadmap row claims no executor").toBe(false);

  // CONTROL: the Creator SPA modules really are in the served bundle, so the
  // edition is not merely a Rust cfg. `renderProApp` is the identifier that
  // survives minification.
  const bundle = await servedBundle(request);
  expect(bundle, "creator SPA modules assembled into the bundle").toContain("renderProApp");
  expect(bundle).toContain("PRO_PANES");
});

test("a Creator build is unlicensed by default and offers no way to become licensed", async ({
  request,
}) => {
  // The licence cache reconnection todo 27 landed is OFF by default, so a
  // fresh sandbox is genuinely unlicensed — which is the state the rest of
  // the Pro journey is observed in.
  const { status, body } = await apiJson(request, "get", "/api/v1/licence/status");
  expect(status).toBe(200);
  expect(body.entitled).toBe(false);
  expect(body.tier).toBe("free");
  expect(body.trial).toBeNull();
  // `implemented` is the backend's own "there is a licence service to talk
  // to" flag. It is what PHASE0/F-11 says the SPA never reads; see the
  // failure spec for that cross-check.
  expect(body.implemented).toBe(false);

  // The release hold is real, not cosmetic: neither entry point can succeed.
  const trial = await apiJson(request, "post", "/api/v1/licence/trial");
  expect([400, 501, 503]).toContain(trial.status);

  const activate = await apiJson(request, "post", "/api/v1/licence/activate", {
    key: "definitely-not-a-real-key",
  });
  expect([400, 401, 403, 501, 503]).toContain(activate.status);
  // And a bad key does not grant anything.
  const after = await apiJson(request, "get", "/api/v1/licence/status");
  expect(after.body.entitled, "a rejected key must not grant entitlement").toBe(false);
});

test("the Pro data endpoints refuse an unentitled machine with 402, not 404 and not 200", async ({
  request,
}) => {
  // The gate is real and it is the Pro set. Each of the four Pro plugins has
  // at least one data route; all must refuse.
  const gated = [
    "/api/v1/plugins/crunchr/recordings",
    "/api/v1/plugins/insights/words",
    "/api/v1/plugins/viewguard/verdicts",
    "/api/v1/plugins/archiver/channels",
  ];
  for (const p of gated) {
    const { status, body } = await apiJson(request, "get", p);
    expect(status, `${p} must be gated`).toBe(402);
    // The 402 body names the capability, so the SPA can say which one.
    expect(JSON.stringify(body), `${p} names the gated capability`).toMatch(/Pro plugin/);
  }

  // CONTROL 1: a non-Pro Creator route on the same server is NOT gated —
  // proves the 402 is a gate, not a blanket refusal.
  const dag = await apiJson(request, "get", "/api/v1/pipelines/dag");
  expect(dag.status, "the DAG is public by design (api.rs:392-396)").toBe(200);

  // CONTROL 2: and the matrix advertises the very plugins the gate refuses,
  // ungated. If the matrix were gated, the honest signal would be a 402 too.
  const matrix = await apiJson(request, "get", "/api/v1/plugins/capabilities");
  expect(matrix.status).toBe(200);
  expect(matrix.body.length, "the capability matrix is populated").toBeGreaterThan(20);
});

test("the plugin hub is unreachable in a real Creator build, so its empty state is never seen", async ({
  page,
  request,
}) => {
  // Server side: all four listed plugins are Pro, and the handler filters
  // locked ones out entirely (routes/plugins.rs:3782-3792). So an unlicensed
  // machine would get an empty list, not a dimmed list.
  const list = await apiJson(request, "get", "/api/v1/plugins");
  expect(list.status).toBe(200);
  expect(list.body.plugins, "locked Pro plugins are filtered out server-side").toEqual([]);

  // But the page that would explain that is itself unreachable. `plugins` is
  // in CREATOR_ROUTES (012-pvr.js:314-316), and render() redirects any
  // CREATOR_ROUTES entry to #/library while creator_enabled is false
  // (008-pvr.js:562-565). Observed WITHOUT instrumentation, because that is
  // the state the product is in.
  await login(page);
  await page.goto("/app#/plugins");
  await expect(page.locator("h1.page-title")).toHaveText("Welcome to StriVo");
  expect(
    await page.evaluate(() => location.hash),
    "the hash was rewritten, not merely ignored",
  ).toBe("#/library");
  await expect(page.locator(".upgrade-card")).toHaveCount(0);
  await expect(page.locator(".pg-cap-matrix")).toHaveCount(0);

  // The honest empty state DOES exist in the source and renders correctly the
  // moment the pane is reachable — so this is a reachability finding, not a
  // broken card. Reached here under the one-field instrumentation documented
  // in creator-journey-helpers.ts::openCreatorSurfaces.
  await openCreatorSurfaces(page);
  await page.goto("/app#/plugins");
  await expect(page.locator(".upgrade-card")).toBeVisible();
  await expect(page.locator(".upgrade-title")).toContainText("Creator Edition unavailable");
  await expect(page.locator(".upgrade-hint")).toContainText(
    "No activation, trial, or purchase path is available",
  );
  // The grid is empty AND the reason is on screen in the same paint.
  await expect(page.locator(".pg-grid")).toContainText("Activate Strivo Pro above");
  await expect(page.locator(".pg-card")).toHaveCount(0);

  // The tension, recorded as PJ-C02: the same page renders the capability
  // matrix from an UNGATED endpoint, and that matrix names the plugins the
  // grid just hid as "available". So the page simultaneously says "activate
  // to populate this grid" and advertises the populated set below it.
  // The chip carries `title="<plugin> · <status>"` (021-creator.js:601-604).
  await expect(page.locator(".pg-cap-matrix").first()).toBeVisible();
  const chips = await page.locator(".pg-cap-chip").evaluateAll((els) =>
    els.map((e) => e.getAttribute("title") ?? ""),
  );
  expect(chips, "the matrix names plugins the grid omitted").toContain("crunchr · available");
  // And it is the very capability the 402 refuses, on the same page load.
  const refused = await apiJson(request, "get", "/api/v1/plugins/crunchr/recordings");
  expect(refused.status).toBe(402);
});

test("a Creator build reports creator_enabled:false, so every Pro route hard-redirects", async ({
  page,
  request,
}) => {
  // Deliberate, and cited: settings.rs:637-639 hardcodes it, with the reason
  // ("Creator work is not publicly available, including in a feature-enabled
  // build"). This is the release hold, not a bug — the finding is that it
  // closes the Pro journey in BOTH directions at once: no affordance and no
  // deep link.
  const settings = await apiJson(request, "get", "/api/v1/settings");
  expect(settings.status).toBe(200);
  expect(settings.body.creator_enabled).toBe(false);
  // The archiver config section IS present under the same cfg, which proves
  // the feature is compiled in and this is a deliberate false, not a build
  // that missed the feature.
  expect(Object.keys(settings.body)).toContain("archiver");

  await login(page);
  await page.goto("/app#/library");

  // Nav: no Creator entry is offered. The links carry their route in
  // data-route, not in text (012-pvr.js:135-138), so read the attribute.
  const routes = await page
    .locator(".topnav-link")
    .evaluateAll((els) => els.map((e) => e.getAttribute("data-route") ?? ""));
  expect(routes.length, "the nav rendered").toBeGreaterThanOrEqual(5);
  for (const pvr of ["library", "recordings", "schedule", "watch"]) {
    expect(routes, `${pvr} is a PVR nav entry and must be present`).toContain(pvr);
  }
  for (const creator of CREATOR_ROUTES) {
    expect(routes, `nav must not offer ${creator}`).not.toContain(creator);
  }

  // Router: every Creator route is not merely unlisted, it is refused. A deep
  // link lands on Home with the hash rewritten, so even a bookmarked Pro URL
  // cannot reach the pane.
  for (const route of CREATOR_ROUTES) {
    await page.goto(`/app#/${route}`);
    await expect(page.locator("h1.page-title"), `#/${route} must bounce to Home`).toHaveText(
      "Welcome to StriVo",
    );
    expect(await page.evaluate(() => location.hash), `#/${route} hash rewritten`).toBe("#/library");
  }
});

/** The seven routes `CREATOR_ROUTES` names (012-pvr.js:314-316). */
const CREATOR_ROUTES = [
  "studio",
  "analytics",
  "publish",
  "pipelines",
  "plugins",
  "dataviz",
  "archive",
];
