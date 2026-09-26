import { test, expect } from "@playwright/test";

// Plan todo 11 — the pipeline run view must not refetch the whole page on
// every stage transition.
//
// The SSE loop is closed and correct end to end: `PipelineUpdated`
// (src/events.rs:172) carries the whole `Pipeline`, routes/events.rs:26-48
// relays it as an UNNAMED frame (a named `event:` field would stop
// `onmessage` firing), and the SPA dispatches on the externally tagged body.
// `renderPipelines()` answered every one of those frames with a
// `Promise.all` of three API calls plus a full `root.innerHTML` repaint.
//
// Two things make that expensive, and the first is easy to miss:
//   1. `API._fetch` collapses duplicate GETs behind a 2 s cache
//      (`_cacheTtlMs`, 002-pvr.js:17), so a *burst* of events costs zero
//      extra requests even with the bug present. Measuring a burst would
//      make this spec vacuous. Real `creator_publish` stages take seconds
//      each, so transitions here are spaced past the TTL — the cadence at
//      which the bug really costs three round trips per stage.
//   2. That same cache is why the refetch was not merely slow but wrong:
//      the cached list overwrote the live transition, so the run view sat
//      at "1/10 stages" while the daemon had advanced. Test 3 pins that.

const KNOWN_RUN_ID = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const STAGES = 10;
const API_GET_TTL_MS = 2000;

// Exactly the three endpoints renderPipelines()'s `Promise.all` issues.
const PIPELINE_CALLS = new Set([
  "/api/v1/pipelines/dag",
  "/api/v1/pipelines/runs",
  "/api/v1/recordings",
]);

/** A `Pipeline` snapshot with `done` stages complete — the real wire shape. */
function pipelineWith(done: number, runId = KNOWN_RUN_ID) {
  return {
    id: runId,
    name: "Ultimate creator publish",
    trigger: "recording-finished",
    state: done === STAGES ? "Done" : "Running",
    started_at_secs: 1757000000,
    completed_at_secs: done === STAGES ? 1757000900 : null,
    stages: Array.from({ length: STAGES }, (_, i) => ({
      id: `stage-${i + 1}`,
      name: `Stage ${i + 1}`,
      kind: { Custom: "creator.stage" },
      inputs: i ? [`stage-${i}`] : [],
      state: i < done ? "Done" : "Pending",
      artifacts: i < done ? [{ kind: "chapters", path: `/artifacts/${i + 1}.json` }] : [],
    })),
  };
}

function makeRun(index: number) {
  return { ...pipelineWith(1, `run-${index}`), name: `Ultimate creator publish ${index + 1}` };
}

/**
 * Serve `runs()` from GET /pipelines/runs for this page only. A getter, not
 * a value, so the stub stays in step with the frames the test emits: the
 * real daemon persists the registry BEFORE it emits (runtime.rs:324-330).
 * Freezing the list instead would let the refetch overwrite the live
 * transition, and the specs would then fail on DOM timeouts rather than on
 * a request count.
 */
async function stubRuns(page: import("@playwright/test").Page, runs: () => unknown[]) {
  await page.route("**/api/v1/pipelines/runs", async (route) => {
    if (new URL(route.request().url()).search) return route.fallback();
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ runs: runs() }),
    });
  });
}

/**
 * Give this page a private pipe for `PipelineUpdated` frames.
 *
 * `/__test__/broadcast` writes to the shared mock server's `sseClients` set,
 * so every frame it emits lands on EVERY page in the run — a 10-stage run
 * pushed there repaints any other test sitting on `#/pipelines` and takes
 * it down. Dispatching on the page's own EventSource keeps the frames where
 * they belong. The SPA's real subscription path is still exercised: its
 * `onmessage` fires, the real route guard runs, and the real patch path is
 * what gets measured.
 */
async function openFramePipe(page: import("@playwright/test").Page) {
  await page.addInitScript(() => {
    const Native = window.EventSource;
    const pipes: ((data: string) => void)[] = [];
    // @ts-ignore
    window.EventSource = class extends Native {
      constructor(...args: ConstructorParameters<typeof Native>) {
        super(...args);
        const self = this;
        pipes.push((injected) => self.dispatchEvent(new MessageEvent("message", { data: injected })));
      }
    };
    (window as any).__pushPipelineFrame = (data: string) => {
      for (const pipe of pipes) pipe(data);
    };
  });
}

/** Push one `PipelineUpdated` frame at this page, as the daemon would. */
async function emit(page: import("@playwright/test").Page, pipeline: unknown) {
  await page.evaluate((data) => (window as any).__pushPipelineFrame(data), JSON.stringify({
    PipelineUpdated: { pipeline },
  }));
}

function counter(page: import("@playwright/test").Page) {
  const calls: string[] = [];
  page.on("request", (req) => {
    const path = new URL(req.url()).pathname;
    if (PIPELINE_CALLS.has(path)) calls.push(path);
  });
  return calls;
}

// Same first-run dismissal the rest of the suite uses — the product tour
// overlay otherwise intercepts the click on "Show all".
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
  await openFramePipe(page);
});

test("a 10-stage run patches the view instead of refetching it per transition", async ({ page }) => {
  test.setTimeout(120_000);
  const calls = counter(page);
  let live = pipelineWith(1, KNOWN_RUN_ID);
  await stubRuns(page, () => [live]);

  await page.goto("/app#/pipelines");
  await expect(page.getByRole("heading", { name: /Runs/ })).toBeVisible();
  const hint = page.locator(".pl-run-row .pg-cap-hint").first();
  await expect(hint).toContainText("1/10 stages");

  // Sync point, not a sleep: the initial `Promise.all` resolved and the
  // first paint is on the DOM, so every request counted from here belongs
  // to a stage transition. `initial > 0` keeps the counter from being
  // vacuous — it proves the counter sees the very calls being avoided.
  const initial = calls.length;
  expect(initial, "counter saw no pipeline requests during the initial load").toBeGreaterThan(0);
  calls.length = 0;
  await page.waitForTimeout(API_GET_TTL_MS + 250);

  for (let done = 1; done <= STAGES; done++) {
    live = pipelineWith(done);
    await emit(page, live);
    await expect(hint).toContainText(`${done}/${STAGES} stages`);
    if (done < STAGES) await page.waitForTimeout(API_GET_TTL_MS + 250);
  }

  expect(
    calls.length,
    `pipeline endpoints hit ${calls.length} times across ${STAGES} stage ` +
      `transitions (${calls.join(", ")}); the acceptance bound is ${STAGES} — ` +
      `one initial coarse load plus one per transition, not three per transition`,
  ).toBeLessThanOrEqual(STAGES);
});

test("a transition for a run the client has never seen falls back to a coarse refetch", async ({
  page,
}) => {
  const calls = counter(page);
  const known = pipelineWith(1, KNOWN_RUN_ID);
  let list = [known];
  await stubRuns(page, () => list);
  await page.goto("/app#/pipelines");
  await expect(page.locator(".pl-run-row")).toHaveCount(1);

  // Started on another client, or evicted by the bounded store: the cache
  // cannot patch it, so the view must reconcile against the server rather
  // than silently drop the event.
  const unknown = pipelineWith(3, "dddddddd-dddd-4ddd-8ddd-dddddddddddd");
  list = [unknown, known];
  await page.waitForTimeout(API_GET_TTL_MS + 250);
  calls.length = 0;
  await emit(page, unknown);

  await expect(page.locator(".pl-run-row")).toHaveCount(2);
  await expect(page.locator('[data-run-id="dddddddd-dddd-4ddd-8ddd-dddddddddddd"]')).toBeVisible();
  // The already-known run survived the coarse load.
  await expect(page.locator('[data-run-id="aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"]')).toBeVisible();
  expect(calls.filter((c) => c === "/api/v1/pipelines/runs").length).toBeGreaterThan(0);
});

test("a transition updates the stage count instead of being overwritten by a cached list", async ({
  page,
}) => {
  let live = pipelineWith(1, KNOWN_RUN_ID);
  await stubRuns(page, () => [live]);
  await page.goto("/app#/pipelines");
  await expect(page.locator(".pl-run-row .pg-cap-hint").first()).toContainText("1/10 stages");

  // Inside the 2 s GET window, where a refetch is served from the SPA's own
  // cache: the run must still advance, or the view is showing a stale run.
  live = pipelineWith(4);
  await emit(page, live);
  await expect(page.locator(".pl-run-row .pg-cap-hint").first()).toContainText("4/10 stages");
});

test("when PipelineUpdated stops, the view shows a stale run rather than going blank", async ({
  page,
}) => {
  await stubRuns(page, () => [pipelineWith(1, KNOWN_RUN_ID)]);
  await page.goto("/app#/pipelines");
  await expect(page.locator(".pl-run-row")).toHaveCount(1);

  await emit(page, pipelineWith(4));
  await expect(page.locator(".pl-run-row .pg-cap-hint").first()).toContainText("4/10 stages");

  // No further events. The last patched state stays on screen: a stale run
  // is an honest, readable failure — an empty list is not.
  await page.waitForTimeout(API_GET_TTL_MS + 250);
  await expect(page.locator(".pl-run-row")).toHaveCount(1);
  await expect(page.getByText("No runs yet", { exact: false })).toHaveCount(0);
});

test("run history past the first page stays reachable", async ({ page }) => {
  const history = Array.from({ length: 25 }, (_, i) => makeRun(i + 1));
  await stubRuns(page, () => history);

  await page.goto("/app#/pipelines");
  // The visible window is capped…
  await expect(page.locator(".pl-run-row")).toHaveCount(20);
  // …the cap is announced rather than silent, and the heading count is the
  // full history length…
  await expect(page.getByRole("heading", { name: /Runs/ })).toContainText("25 durable runs");
  // …and one click reaches the rest.
  await page.locator("#pl-runs-show-all").click();
  await expect(page.locator(".pl-run-row")).toHaveCount(25);
});
