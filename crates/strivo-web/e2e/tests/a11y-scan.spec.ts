// WCAG 2.2 AA scan over both editions (todo 18).
//
// `@axe-core/playwright` is a test-only devDependency of this directory,
// which never enters the release pipeline — the served bundle is
// `assets/spa*` and nothing in the Rust build reads node_modules.
//
// Two things this file is deliberately paranoid about, because both failure
// modes report success:
//
//   1. A scan that NAVIGATED NOWHERE reports zero violations. Every route is
//      therefore checked three ways: the SPA is asserted to have settled on
//      the requested hash, the route's page is asserted non-empty, and the
//      set of routes actually visited is asserted at the end to equal the
//      enumerated list (see `visited` + the `all enumerated routes were
//      scanned` test).
//   2. A scan that MATCHED NOTHING is indistinguishable from a clean page,
//      so `scan()` asserts axe evaluated a non-trivial number of rules, and
//      the `scan detects an injected violation` test proves the result
//      object is read for real by breaking a control on purpose.
//
// No rule is disabled, nothing is excluded, and no route is skipped. If a
// violation cannot be fixed it is reported, not suppressed.
import { test, expect, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

/** The nine PVR routes named in `docs/RESEARCH-PLATFORM-ROADMAP.md:73-83`
 *  scope and in todo 18's acceptance criteria, in the ROUTES table order
 *  (008-pvr.js:318). `#/play` is here because it is a real route; the
 *  ux-budget lane's own list omitted it until this pass added it. */
const PVR_ROUTES = [
  "library",
  "recordings",
  "schedule",
  "watch",
  "play",
  "chat",
  "settings",
  "system",
  "logs",
] as const;

/** The Creator-gated routes (012-pvr.js:32 `CREATOR_ROUTES`). The plan names
 *  `settings`, `recordings` and `library` for the Creator edition, but those
 *  three are shared PVR routes already scanned above; the routes that are
 *  actually Creator-only are these seven, so the scan covers them too rather
 *  than claiming a second edition's coverage it did not measure. */
const CREATOR_ROUTES = [
  "timeline",
  "studio",
  "analytics",
  "publish",
  "pipelines",
  "plugins",
  "dataviz",
  "archive",
] as const;

const ALL_ROUTES = [...PVR_ROUTES, ...CREATOR_ROUTES];

const TAGS = ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"];

/** Routes actually opened, accumulated across the run. Module scope because
 *  the assertion that the enumerated list was covered can only be made
 *  after the per-route tests have run. */
const visited = new Set<string>();

type Impact = "critical" | "serious" | "moderate" | "minor";
const BLOCKING: Impact[] = ["critical", "serious"];

async function open(page: Page, route: string) {
  await page.addInitScript(() => {
    localStorage.setItem("strivo-tour-done", "1");
    localStorage.setItem("strivo-player-autoplay", "0");
  });
  await page.goto(`/app#/${route}`);
  await page.locator(".chrome").waitFor();
  // Hydration (channels, recordings, tiles, settings) is async; an early
  // scan would grade a loading skeleton instead of the route.
  await page.waitForTimeout(1200);
}

/** Navigate, prove we arrived, then scan. `proveArrival` throws rather than
 *  returning a flag so a mis-navigated route can never be scanned and
 *  counted as clean. */
async function scan(page: Page, route: string) {
  await open(page, route);

  const arrived = await page.evaluate(() => ({
    hash: window.location.hash,
    // Non-empty page: the router bounced (unknown route) or a paint threw,
    // and a blank <main> is trivially violation-free.
    text: (document.getElementById("content")?.textContent || "").trim().length,
  }));
  expect(arrived.hash, `#/${route} did not stay on its route`).toBe(`#/${route}`);
  expect(arrived.text, `#/${route} rendered an empty #content`).toBeGreaterThan(20);
  visited.add(route);

  const results = await new AxeBuilder({ page }).withTags(TAGS).analyze();

  // Non-vacuity: axe really ran, and it really evaluated the document.
  expect(
    results.violations.length + results.passes.length + results.incomplete.length,
    `axe evaluated almost nothing on #/${route} — the scan is not looking at the page`,
  ).toBeGreaterThan(20);

  const blocking = results.violations.filter((v) => BLOCKING.includes(v.impact as Impact));
  if (blocking.length) {
    void test.info().attach(`axe-critical-serious-${route}`, {
      body: JSON.stringify(
        blocking.map((v) => ({
          id: v.id,
          impact: v.impact,
          help: v.help,
          nodes: v.nodes.map((n) => ({ target: n.target, failureSummary: n.failureSummary })),
        })),
        null,
        2,
      ),
      contentType: "application/json",
    });
  }
  expect(
    blocking.map((v) => `${v.impact} ${v.id} (${v.nodes.length} nodes)`),
    `#/${route} has critical/serious WCAG 2.2 AA violations`,
  ).toEqual([]);

  // Non-blocking findings are reported, not asserted — the plan's bar is
  // critical/serious, and inventing a moderate-or-clean bar would be a
  // threshold move in the opposite direction.
  const advisory = results.violations
    .filter((v) => !BLOCKING.includes(v.impact as Impact))
    .map((v) => `${v.impact} ${v.id} (${v.nodes.length} nodes)`);
  test.info().annotations.push({
    type: "axe-advisory",
    description: advisory.length ? advisory.join("; ") : "none",
  });
  return { blocking: blocking.length, advisory, passes: results.passes.length };
}

// Serial so the coverage assertion below sees one worker's `visited` set —
// under the lane's `fullyParallel: true` the per-route tests are spread
// across workers, each with its own module instance, and the aggregate would
// report near-total non-coverage.
test.describe.configure({ mode: "serial" });

for (const route of PVR_ROUTES) {
  test(`PVR #/${route}: zero critical or serious WCAG 2.2 AA violations`, async ({ page }) => {
    await scan(page, route);
  });
}

for (const route of CREATOR_ROUTES) {
  test(`Creator #/${route}: zero critical or serious WCAG 2.2 AA violations`, async ({ page }) => {
    await scan(page, route);
  });
}

test("all enumerated routes were actually scanned", async () => {
  const missing = ALL_ROUTES.filter((r) => !visited.has(r));
  expect(missing, "routes never scanned — the report would overstate coverage").toEqual([]);
  expect(visited.size).toBe(ALL_ROUTES.length);
});

test("scan detects an injected violation", async ({ page }) => {
  // The probe the plan asks for: a scan that runs but matches nothing looks
  // identical to a clean page. Break an icon-only control's accessible name
  // in the live DOM and prove the scan reports it, then put it back.
  await open(page, "library");

  const before = await new AxeBuilder({ page }).withTags(TAGS).analyze();
  expect(
    before.violations.filter((v) => BLOCKING.includes(v.impact as Impact)).length,
    "precondition: #/library is clean before the injection",
  ).toBe(0);

  // Both the name AND the title have to go: `#health-pill` ships
  // `title="System health — click for details"` (012-pvr.js:148) and `title`
  // is a fallback accessible name, so removing only aria-label leaves the
  // control correctly named. That is a real fact about the template, and the
  // first version of this probe got it wrong.
  const injected = await page.evaluate(() => {
    const pill = document.getElementById("health-pill") as HTMLElement | null;
    if (!pill) return null;
    pill.hidden = false;
    pill.removeAttribute("aria-label");
    pill.removeAttribute("title");
    // A second, independent rule family so the probe is not tied to one rule.
    const img = document.createElement("img");
    img.src = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";
    document.querySelector("#content")?.prepend(img);
    return { text: (pill.textContent || "").trim() };
  });
  expect(injected, "the injection target #health-pill was not present").not.toBeNull();

  const after = await new AxeBuilder({ page }).withTags(TAGS).analyze();
  const ids = after.violations.map((v) => v.id);
  expect(
    ids,
    "axe did not report the injected violations — the scan cannot fail",
  ).toEqual(expect.arrayContaining(["image-alt"]));
  expect(
    ids,
    "axe did not report the injected missing-accessible-name violation — the scan cannot fail",
  ).toEqual(expect.arrayContaining(["link-name"]));

  // Revert, and prove the revert is what makes it clean again rather than
  // the violation never having applied.
  await page.evaluate(() => {
    const pill = document.getElementById("health-pill") as HTMLElement | null;
    pill?.setAttribute("aria-label", "System health — click for details");
    pill?.setAttribute("title", "System health — click for details");
    pill && (pill.hidden = true);
    document.querySelector("#content > img")?.remove();
  });
  const reverted = await new AxeBuilder({ page }).withTags(TAGS).analyze();
  expect(
    reverted.violations.filter((v) => BLOCKING.includes(v.impact as Impact)).length,
    "restoring the aria-label did not clear the violation",
  ).toBe(0);
});
