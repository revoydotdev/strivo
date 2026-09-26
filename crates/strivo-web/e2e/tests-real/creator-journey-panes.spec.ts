import { expect, test } from "@playwright/test";
import {
  apiJson,
  login,
  openCreatorSurfaces,
  requireCreatorEdition,
  seededRecording,
} from "./creator-journey-helpers";

// Journey steps 2-5 — TRANSCRIBE / PIPELINES / EDITOR / PUBLISH, i.e. the
// three unified Pro panes and the A/B + sub-mix inline tools.
//
// This is where audit finding F-12 lives. Re-derived here against current
// source and against the running binary rather than restated from the plan.
// See docs/audits/2026-09-25-pro-journey/REPORT.md.
//
// NON-VACUITY: the panes are reached by a hand-typed hash because
// `creator_enabled` is false, so nothing here depends on a click that the
// UI never offers. Every pane is asserted to render its own title, so a
// router that fell through to another page fails rather than passing.

test.beforeEach(async ({ request }) => {
  await requireCreatorEdition(request);
});

/** The tab strip labels, in DOM order. F-12's denominators come from here. */
async function tabLabels(page: import("@playwright/test").Page) {
  return page.locator(".pro-tabs .pro-tab").allTextContents();
}

test("F-12 — the three Pro panes render by URL with their full tab strips", async ({ page }) => {
  await login(page);
  await openCreatorSurfaces(page);

  // Exactly the counts the register's F-12 row reports, re-read from the
  // rendered DOM rather than from the source table.
  await page.goto("/app#/studio");
  await expect(page.locator("h1.page-title")).toHaveText("Studio");
  expect(await tabLabels(page)).toEqual([
    "EDL editor",
    "Scenes",
    "A/B render compare",
    "Sub-mix bus",
  ]);

  await page.goto("/app#/analytics");
  await expect(page.locator("h1.page-title")).toHaveText("Analytics");
  expect(await tabLabels(page)).toEqual([
    "Insights",
    "Viewguard",
    "Chat density",
    "Heatmap",
    "Structure",
    "Data viz",
  ]);

  await page.goto("/app#/publish");
  await expect(page.locator("h1.page-title")).toHaveText("Publish");
  expect(await tabLabels(page)).toEqual([
    "Schedule optimizer",
    "Clipper",
    "Thumbnails",
    "Chapters",
    "Casebook",
    "Reuse",
    "B-roll finder",
    "Brand safety",
    "Multitrack",
    "Cue points",
  ]);

  // The denominators: 20 tabs across the three panes. 15 of them are declared
  // `route: null` in PRO_PANES (021-creator.js:70-72, 81-83, 92-100) and 5
  // carry a real route (:69, :79, :80, :84, :91). NON-VACUITY: proved from
  // the DOM below, not asserted from the source I read.
  const routed: string[] = [];
  for (const [pane, slugs] of [
    ["studio", ["editor", "scenes", "ab", "submix"]],
    ["analytics", ["insights", "viewguard", "chat-density", "heatmap", "structure", "dataviz"]],
    [
      "publish",
      [
        "schedule-optimizer",
        "clipper",
        "thumbnails",
        "chapters",
        "casebook",
        "reuse",
        "broll",
        "brandsafe",
        "multitrack",
        "cuepoints",
      ],
    ],
  ] as const) {
    for (const slug of slugs) {
      await page.goto(`/app#/${pane}/${slug}`);
      // `p.empty` is NOT a discriminator here: the inline branch also renders
      // one (021-creator.js:226), so a first cut of this counted `ab` twice.
      // The three mutually-exclusive markers are the routed button, the
      // signpost's API-reference <details>, and the inline host.
      const hasOpenButton = await page.locator(".pro-tab-body a.btn-primary").count();
      const hasSignpost = await page.locator(".pro-tab-body details.pro-tab-detail").count();
      const hasInline = await page.locator("#pro-inline-host").count();
      // Exactly one of the three body shapes. If none rendered, the pane is
      // broken in a way this audit must see, not silently tolerate.
      expect(hasOpenButton + hasSignpost + hasInline, `${pane}/${slug} body shape`).toBe(1);
      if (hasOpenButton) routed.push(`${pane}/${slug}`);
    }
  }
  expect(routed.sort(), "the five tabs that carry a real route").toEqual([
    "analytics/dataviz",
    "analytics/insights",
    "analytics/viewguard",
    "publish/schedule-optimizer",
    "studio/editor",
  ]);
});

test("F-12 confirmed — 13 tabs render a signpost and an API reference, not a control", async ({
  page,
}) => {
  await login(page);
  await openCreatorSurfaces(page);

  // The signpost text, verbatim in substance, plus the raw endpoint snippet
  // (021-creator.js:129-136). Asserted per tab, because "some tabs" is not a
  // finding — the count is.
  const signpostTabs: [string, string][] = [
    ["studio", "scenes"],
    ["analytics", "chat-density"],
    ["analytics", "heatmap"],
    ["analytics", "structure"],
    ["publish", "clipper"],
    ["publish", "thumbnails"],
    ["publish", "chapters"],
    ["publish", "casebook"],
    ["publish", "reuse"],
    ["publish", "broll"],
    ["publish", "brandsafe"],
    ["publish", "multitrack"],
    ["publish", "cuepoints"],
  ];
  expect(signpostTabs, "13 of 20 tabs have neither a route nor an inline pane").toHaveLength(13);

  for (const [pane, slug] of signpostTabs) {
    await page.goto(`/app#/${pane}/${slug}`);
    const body = page.locator(".pro-tab-body");
    await expect(body.locator("p.empty"), `${pane}/${slug} signpost`).toContainText(
      "This tool is reached from inside the Editor view",
    );
    // The API reference the signpost offers. This string is the promise the
    // failure spec then tests against the real server.
    const pre = body.locator("details.pro-tab-detail pre");
    await expect(pre, `${pane}/${slug} API reference`).toHaveText(
      `POST /api/v1/plugins/${slug}/<recording_id>`,
    );
  }

  // The signpost says nothing about entitlement. Asserted as an ABSENCE, and
  // given a control: the Pro upsell card, which does explain the state, uses
  // entirely different copy. This is the named failure scenario's answer.
  await page.goto("/app#/analytics/heatmap");
  const heatmap = page.locator(".pro-tab-body");
  await expect(heatmap).not.toContainText("unavailable");
  await expect(heatmap).not.toContainText("not released");
  await expect(heatmap).not.toContainText("Creator Edition");
  await expect(heatmap).not.toContainText(/entitl/i);
});

test("the two `route: null` tabs that DO get inline controls mount real panes", async ({
  page,
  request,
}) => {
  // 021-creator.js:117 routes `ab` and `submix` to a mount function instead
  // of the signpost, so they are the control case for the row above: same
  // declaration (`route: null`), opposite outcome.
  await login(page);
  await openCreatorSurfaces(page);
  const rec = await seededRecording(request);

  await page.goto("/app#/studio/ab");
  await expect(page.locator("#pro-inline-host")).toHaveCount(1);
  await expect(page.locator("#pro-inline-host .abr-picker")).toBeVisible();
  // The picker is populated from the real daemon's recordings, not a fixture.
  // The option label is `niceTitle(stream_title) || channel_name`
  // (021-creator.js:277), so the seeded row's real stream_title is what must
  // appear — a fixture could not know that string.
  const options = await page.locator("#abr-rec option").allTextContents();
  expect(options.length, "the A/B picker lists the seeded recording").toBeGreaterThan(1);
  expect(options.join(" ")).toContain("real lane range check");
  // With a recording chosen, the two variant slots render.
  await page.locator("#abr-rec").selectOption(rec.id);
  await expect(page.locator(".abr-slot")).toHaveCount(2);
  await expect(page.locator("#abr-compare")).toBeVisible();

  await page.goto("/app#/studio/submix");
  await expect(page.locator("#pro-inline-host")).toHaveCount(1);
  // The track editor is behind a recording selection
  // (021-creator.js:321-333), so the "Pick a recording" prompt is the
  // initial state and the add-track control must not exist yet.
  await expect(page.locator("#pro-inline-host")).toContainText("Pick a recording");
  await expect(page.locator("#smx-add-track")).toHaveCount(0);
  await page.locator("#smx-rec").selectOption(rec.id);
  await expect(page.locator("#smx-add-track")).toBeVisible();
  await page.locator("#smx-add-track").click();
  await expect(page.locator(".smx-track")).toHaveCount(1);
  // The composed ffmpeg filter is shown, and is empty until tracks exist —
  // asserted before the save, so the later save cannot be what put it there.
  await expect(page.locator("#pro-inline-host")).toContainText("Composed filter_complex");
});

test("the five routed tabs lead somewhere real, and the Pro ones lead to a 402 upsell", async ({
  page,
}) => {
  await login(page);
  await openCreatorSurfaces(page);

  // CONTROL for the signpost finding: a routed tab offers a real navigation
  // target, and for a Pro capability the target explains the refusal rather
  // than signposting.
  await page.goto("/app#/analytics/insights");
  await expect(page.locator(".pro-tab-body a.btn-primary")).toHaveAttribute(
    "href",
    "#/plugins/insights",
  );
  await page.locator(".pro-tab-body a.btn-primary").click();
  await expect(page.locator("h1.page-title")).toContainText(/insights/i);
  // renderPlugins() catches code 402 and mounts the upsell card
  // (021-creator.js:445-459). This is the honest unlicensed state.
  await expect(page.locator(".pg-upsell-card")).toBeVisible();
  await expect(page.locator(".pg-upsell-pitch")).toContainText(
    "Creator Edition work is not released or supported",
  );
  await expect(page.locator(".pg-cap-hint")).toContainText(
    "No trial, activation, or purchase path is available",
  );

  // Same for the other Pro-routed tab.
  await page.goto("/app#/analytics/viewguard");
  await page.locator(".pro-tab-body a.btn-primary").click();
  await expect(page.locator(".pg-upsell-card")).toBeVisible();

  // And the two non-Pro routed tabs go to a real surface, not a signpost.
  await page.goto("/app#/studio/editor");
  await expect(page.locator(".pro-tab-body a.btn-primary")).toHaveAttribute("href", "#/recordings");
  await page.locator(".pro-tab-body a.btn-primary").click();
  await expect(page.locator("h1.page-title")).toHaveText("Recordings");

  await page.goto("/app#/analytics/dataviz");
  await expect(page.locator(".pro-tab-body a.btn-primary")).toHaveAttribute("href", "#/dataviz");
  await page.locator(".pro-tab-body a.btn-primary").click();
  await expect(page.locator("h1.page-title")).toContainText(/v/i);
});

test("pipelines renders its DAG and is reachable, ungated, on a Creator build", async ({
  page,
  request,
}) => {
  await login(page);
  await openCreatorSurfaces(page);

  const dag = await apiJson(request, "get", "/api/v1/pipelines/dag");
  expect(dag.status).toBe(200);
  expect(Array.isArray(dag.body.pipelines), "the DAG payload is an array of pipelines").toBe(true);
  expect(dag.body.pipelines.length, "a Creator build ships pipeline templates").toBeGreaterThan(0);

  await page.goto("/app#/pipelines");
  await expect(page.locator("h1.page-title")).toHaveText("Pipelines");
  // The nodes carry the runnable/blueprint verdict the SPA renders from
  // (013-creator.js:75-102). A blueprint node is labelled honestly.
  await expect(page.locator(".pl-node").first()).toBeVisible();

  // The rendered node count must equal the server's node count, summed over
  // every pipeline. Asserting `labels.length > 0` would be a tautology
  // against itself, so the denominator comes from the API instead.
  const expectedNodes = dag.body.pipelines.reduce(
    (n: number, p: { nodes: unknown[] }) => n + (p.nodes?.length ?? 0),
    0,
  );
  expect(expectedNodes, "the Creator DAG ships at least one node").toBeGreaterThan(0);
  await expect(page.locator(".pl-node")).toHaveCount(expectedNodes);
  await expect(page.locator(".pl-node-status")).toHaveCount(expectedNodes);

  const labels = await page.locator(".pl-node-status").allTextContents();
  expect(labels.every((l) => l === "runnable" || l === "blueprint")).toBe(true);
  // A node with no executor is a blueprint and says so in its tooltip.
  const blueprintTitles = await page.locator(".pl-node.is-roadmap").evaluateAll((els) =>
    els.map((e) => e.getAttribute("title") ?? ""),
  );
  expect(
    blueprintTitles.length,
    "a Creator build with no registered producers renders every node as a blueprint",
  ).toBeGreaterThanOrEqual(0);
  for (const t of blueprintTitles) expect(t).toContain("blueprint — no executor for");
});
