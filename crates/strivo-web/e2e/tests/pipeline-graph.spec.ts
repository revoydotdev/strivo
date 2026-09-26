import { test, expect, type Page } from "@playwright/test";
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

// Plan todo 52 — the Pipelines page draws a node graph over the executor's
// own `Pipeline`/`Stage` model, and this lane proves the surface is faithful
// to the payload it is given.
//
// The trap this file is written against: **a graph that renders nodes
// satisfies "the graph shows the pipeline" for free.** So every claim here
// asserts an identity (which stage id, which verdict, which edge) rather
// than a count of boxes, and the not-runnable case is driven from a payload
// whose stage names a plugin no build registers.
//
// What is deliberately NOT asserted here: that the payload is the real
// `Stage` list. That is `crates/strivo-web/tests/pipeline_graph.rs`, and it
// asserts it against `src/pipeline/stage.rs` itself. A mock lane cannot
// establish it — a mock that agreed with the browser about a derivation it
// both invented would prove nothing.

const STAGE_CASEBOOK = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const STAGE_THUMBS = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const T_TRANSCRIBE = "11111111-1111-4111-8111-111111111111";
const T_CUEPOINTS = "22222222-2222-4222-8222-222222222222";
const T_BRANDSAFE = "33333333-3333-4333-8333-333333333333";
const T_PUBLISH = "44444444-4444-4444-8444-444444444444";
const RUN_ID = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

const VENDOR_PATH = "/assets/vendor/x6.umd.js";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
});

/** Open the page and wait for the graph's own paint signal, not a sleep. */
async function openGraphs(page: Page) {
  await page.goto("/app#/pipelines");
  await expect(page.locator("[data-pg-graph]")).toHaveCount(2);
  // `data-pg-painted` is set after X6's render queue has run, so this is a
  // settled state rather than a guess about how long drawing takes.
  await expect(page.locator('[data-pg-canvas][data-pg-painted="true"]')).toHaveCount(2);
}

function nodeIds(page: Page, graphKey: string) {
  return page.locator(`[data-pg-graph="${graphKey}"] [data-pg-node]`);
}

test("the graph draws one node per stage, identified, and the edges it drew", async ({ page }) => {
  await openGraphs(page);

  // The run graph: the two stage ids the payload named, in the payload's
  // order, and nothing else. Counting boxes would pass on any renderer.
  const run = nodeIds(page, `run:${RUN_ID}`);
  await expect(run).toHaveCount(2);
  await expect(run.nth(0)).toHaveAttribute("data-pg-node", STAGE_THUMBS);
  await expect(run.nth(1)).toHaveAttribute("data-pg-node", STAGE_CASEBOOK);

  // Recorded state, copied from the payload — the job table's half.
  await expect(run.nth(0)).toHaveAttribute("data-pg-stage-state", "Done");
  await expect(run.nth(1)).toHaveAttribute("data-pg-stage-state", "Done");

  // The executor registry's half, independently.
  await expect(run.nth(0)).toHaveAttribute("data-pg-runnable", "true");
  await expect(page.locator('[data-pg-readiness="2/2 stages have an executor"]')).toBeVisible();

  // Edges: X6's own edge elements, one per `inputs` link. 1 for the run,
  // 4 for the template — 5 total, which is the payload's edge count and not
  // a number the page could arrive at by drawing nodes.
  await expect(page.locator(".x6-edge")).toHaveCount(5);
  await expect(nodeIds(page, "template:creator_publish")).toHaveCount(4);
});

test("a stage whose executor is absent is drawn not-runnable and names what is missing", async ({ page }) => {
  await openGraphs(page);
  const ghost = page.locator(`[data-pg-node="${T_PUBLISH}"]`);
  await expect(ghost).toHaveAttribute("data-pg-runnable", "false");
  // Naming the absent thing is the contract todo 6 established, and a node
  // that merely lacked a green tick would not satisfy it.
  await expect(ghost).toContainText("no executor: dispatch plugin reuse");
  await expect(ghost).toHaveAttribute("aria-label", /No executor in this build: dispatch plugin reuse/);
  // The other three stages in the same graph are runnable, so the verdict is
  // a per-node answer and not a page-wide one.
  for (const id of [T_TRANSCRIBE, T_CUEPOINTS, T_BRANDSAFE]) {
    await expect(page.locator(`[data-pg-node="${id}"]`)).toHaveAttribute("data-pg-runnable", "true");
  }
  await expect(page.locator('[data-pg-readiness="3/4 stages have an executor"]')).toBeVisible();
});

test("X6 is loaded by a side-effect dynamic import, and the namespace is only on globalThis", async ({ page }) => {
  const vendor: Array<{ url: string; type: string; status: number; phase: string }> = [];
  await page.addInitScript(() => {
    (window as unknown as { __vendorPhase: string }).__vendorPhase = "spa-not-loaded";
  });
  page.on("request", (r) => {
    if (r.url().includes("/assets/vendor/")) {
      vendor.push({
        url: r.url(),
        type: r.headers()["accept"] || "",
        status: 0,
        // Where in the page's life the fetch happened. A static top-level
        // import would be part of loading spa.js itself, before any render.
        phase: (page as unknown as { __vendorPhase?: string }) && "spa-not-loaded",
      });
    }
  });
  await openGraphs(page);

  expect(vendor.length, "the vendor was fetched at all").toBeGreaterThan(0);
  const x6 = await page.evaluate(async (path) => {
    const mod = await import(path);
    const ns = (globalThis as unknown as { X6?: Record<string, unknown> }).X6;
    return {
      // Fetched-and-evaluated is not the same claim as fetched.
      moduleNamespaceKeys: Object.keys(mod),
      graphIsFunction: typeof (ns?.Graph as unknown) === "function",
      memberCount: ns ? Object.keys(ns).length : 0,
    };
  }, VENDOR_PATH);

  // Todo 31 shipped this UMD artefact and left the browser-evaluation
  // assertion unmade, so a version bump that changed the wrapper would not
  // be caught by CI. This is that assertion: the module namespace is EMPTY
  // (which is exactly why a static `import { Graph }` is a link-time
  // SyntaxError that would take the whole concatenated spa.js down), and the
  // global is populated.
  expect(x6.moduleNamespaceKeys, "a UMD bundle exports nothing to an ES importer").toEqual([]);
  expect(x6.graphIsFunction, "globalThis.X6.Graph must be a class after the import").toBe(true);
  expect(x6.memberCount).toBeGreaterThan(100);

  // And the drawing is X6's, not a hand-rolled fallback list: the cells are
  // inside an X6 graph container.
  await expect(page.locator(".x6-graph")).toHaveCount(2);
  await expect(page.locator(".x6-node")).toHaveCount(6);
});

test("the vendor is reached only by a dynamic import, and only from a creator module", async ({ page }) => {
  // The body of the bundle this page actually parsed, not a re-fetch: a
  // static import would have to be IN this response to be the thing the
  // browser resolves at load time.
  let bundle = "";
  page.on("response", async (res) => {
    if (new URL(res.url()).pathname === "/assets/spa.js") bundle = await res.text();
  });
  await openGraphs(page);
  expect(bundle.length, "the spa.js response body was captured").toBeGreaterThan(10_000);

  // A static import of the vendor path is the specific PVR regression:
  // build.rs removes assets/vendor/ from a PVR build, and a surviving
  // reference would be a call site pointing at a 404.
  const staticImports =
    bundle.match(/^[ \t]*import\s[\s\S]{0,200}?from\s*["'][^"']*vendor[^"']*["']/gm) || [];
  expect(staticImports, "no static import of a vendored file").toEqual([]);
  const dynamic = bundle.match(/import\(\s*["'][^"']*x6\.umd\.js["']\s*\)/g) || [];
  expect(dynamic.length, "the X6 import is dynamic, and present exactly once").toBe(1);

  // Creator-only, at the source level: no PVR module may name the vendor
  // path at all. `check-pvr-bundle.mjs` proves the built PVR artifact; this
  // proves the module sources the mock lane serves, which that script does
  // not read.
  const spaDir = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "assets", "spa");
  const offenders = readdirSync(spaDir)
    .filter((f) => f.endsWith("-pvr.js"))
    .filter((f) => readFileSync(join(spaDir, f), "utf8").includes("/assets/vendor/"));
  expect(offenders, "no PVR module references a vendored file").toEqual([]);
  const creatorHolders = readdirSync(spaDir)
    .filter((f) => f.endsWith("-creator.js"))
    .filter((f) => readFileSync(join(spaDir, f), "utf8").includes("x6.umd.js"));
  expect(creatorHolders, "a creator module owns the reference").toEqual(["044-creator.js"]);
});

test("a vendor load that fails is a named state, and the stage list survives it", async ({ page }) => {
  await page.route(`**${VENDOR_PATH}`, (route) => route.abort());
  await page.goto("/app#/pipelines");
  await expect(page.locator('[data-pg-state="vendor-load-failed"]')).toHaveCount(2);
  // The panel says what is missing, in the same vocabulary the timeline uses.
  const panel = page.locator('[data-pg-state="vendor-load-failed"]').first();
  await expect(panel).toContainText("could not be loaded");
  await expect(panel).toHaveAttribute("data-kind", "vendor-load-failed");
  // Nothing is drawn, and the stages are still all there with their verdicts.
  await expect(page.locator(".x6-node")).toHaveCount(0);
  await expect(nodeIds(page, `run:${RUN_ID}`)).toHaveCount(2);
  await expect(page.locator(`[data-pg-node="${T_PUBLISH}"]`)).toHaveAttribute("data-pg-runnable", "false");
});

test("a vendor response that never arrives reaches the state instead of hanging", async ({ page }) => {
  test.setTimeout(60_000);
  // A server that accepts the connection and never sends a body. Todo 43
  // recorded exactly this as its one unresolved item, so it is probed here:
  // the await is bounded, so the surface must reach a named state rather
  // than stay suspended on a promise nobody will settle.
  await page.route(`**${VENDOR_PATH}`, async () => {
    await new Promise(() => {});
  });
  await page.goto("/app#/pipelines");
  await expect(page.locator('[data-pg-state="vendor-load-failed"]')).toHaveCount(2, {
    timeout: 30_000,
  });
  await expect(nodeIds(page, "template:creator_publish")).toHaveCount(4);
});

test("a mapping the server reported as broken is not drawn, and is named", async ({ page }) => {
  await page.route("**/api/v1/pipelines/runs", async (route) => {
    const response = await route.fetch();
    const body = (await response.json()) as { graph: { graphs: Array<{ mapping: { mismatches: string[] } }> } };
    body.graph.graphs[1].mapping.mismatches = ["node deadbeef maps to no stage"];
    await route.fulfill({ response, json: body });
  });
  await page.goto("/app#/pipelines");
  await expect(page.locator('[data-pg-state="malformed"]')).toHaveCount(1);
  await expect(page.locator('[data-pg-state="malformed"]')).toContainText("node deadbeef maps to no stage");
});

test("a cyclic pipeline is reported rather than laid out", async ({ page }) => {
  await page.route("**/api/v1/pipelines/runs", async (route) => {
    const response = await route.fetch();
    const body = await response.json() as {
      graph: { graphs: Array<{ edges: Array<{ from: string; to: string }>; mapping: { inputs_links: number } }> };
    };
    const graph = body.graph.graphs[1];
    // A→B→A. `inputs` still holds, so the mapping check passes and the
    // cycle has to be caught by the layout instead.
    graph.edges[0].from = graph.edges[3].to;
    graph.mapping.inputs_links = 4;
    await route.fulfill({ response, json: body });
  });
  await page.goto("/app#/pipelines");
  await expect(page.locator('[data-pg-state="malformed"]')).toHaveCount(1);
  await expect(page.locator('[data-pg-state="malformed"]')).toContainText(/cycle/);
  await expect(page.locator(".x6-node")).toHaveCount(2); // only the run graph drew
});

test("an empty stage list is its own state, distinct from an unresolved one", async ({ page }) => {
  await page.route("**/api/v1/pipelines/runs", async (route) => {
    const response = await route.fetch();
    const body = await response.json() as { graph: { graphs: Array<Record<string, unknown>> } };
    const g = body.graph.graphs[1];
    g.nodes = [];
    g.edges = [];
    g.totals = { nodes: 0, edges: 0, runnable: 0, not_runnable: 0 };
    g.mapping = { stages: 0, nodes: 0, edges: 0, inputs_links: 0, mismatches: [] };
    await route.fulfill({ response, json: body });
  });
  await page.goto("/app#/pipelines");
  await expect(page.locator('[data-pg-state="empty"]')).toHaveCount(1);
  // The wording has to distinguish this from "waiting on an executor",
  // because the two mean opposite things.
  await expect(page.locator('[data-pg-state="empty"]')).toContainText(/not the same as an unresolved/);
  await expect(page.locator('[data-pg-state="degraded"]')).toHaveCount(0);
});

test("a response with no graph is a named failure, not an empty page", async ({ page }) => {
  await page.route("**/api/v1/pipelines/runs", async (route) => {
    const response = await route.fetch();
    const body = (await response.json()) as Record<string, unknown>;
    delete body.graph;
    await route.fulfill({ response, json: body });
  });
  await page.goto("/app#/pipelines");
  await expect(page.locator('[data-pg-state="failed"]')).toHaveCount(1);
  await expect(page.locator('[data-pg-state="failed"]')).toContainText("no pipeline graph");
});

// ── KEYBOARD ────────────────────────────────────────────────────────────────

test("node selection is driven by the keyboard, and Escape leaves no residue", async ({ page }) => {
  await openGraphs(page);
  const nodes = nodeIds(page, "template:creator_publish");
  // Roving tabindex: the graph is one tab stop. Tabbing through four stages
  // is tedious rather than operable, and eleven would be unusable.
  await expect(page.locator('[data-pg-graph="template:creator_publish"] [data-pg-node][tabindex="0"]')).toHaveCount(1);
  await expect(page.locator('[data-pg-graph="template:creator_publish"] [data-pg-node][tabindex="-1"]')).toHaveCount(3);

  await nodes.nth(0).focus();
  await page.keyboard.press("ArrowRight");
  await expect(page.locator(`[data-pg-node="${T_CUEPOINTS}"][aria-pressed="true"]`)).toHaveCount(1);
  await expect(page.locator(`[data-pg-node="${T_TRANSCRIBE}"][aria-pressed="true"]`)).toHaveCount(0);
  await page.keyboard.press("End");
  await expect(page.locator(`[data-pg-node="${T_PUBLISH}"][aria-pressed="true"]`)).toHaveCount(1);
  await page.keyboard.press("Home");
  await expect(page.locator(`[data-pg-node="${T_TRANSCRIBE}"][aria-pressed="true"]`)).toHaveCount(1);
  await page.keyboard.press("ArrowLeft");
  await expect(page.locator(`[data-pg-node="${T_PUBLISH}"][aria-pressed="true"]`)).toHaveCount(1);

  // The detail region states both halves and updates with the selection.
  const detail = page.locator('[data-pg-detail="template:creator_publish"]');
  await expect(detail).toContainText("Job table");
  await expect(detail).toContainText("Executor registry");

  await page.keyboard.press("Escape");
  await expect(page.locator('[data-pg-node][aria-pressed="true"]')).toHaveCount(0);
  await expect(detail).toBeEmpty();
});

test("focus moves through the whole graph on the keyboard alone, and returns", async ({ page }) => {
  await openGraphs(page);
  const seen: string[] = [];
  await nodeIds(page, "template:creator_publish").nth(0).focus();
  for (let i = 0; i < 4; i++) {
    seen.push(
      await page.evaluate(() => (document.activeElement as HTMLElement)?.dataset.pgNode || "none"),
    );
    await page.keyboard.press("ArrowRight");
  }
  // Four presses from the first of four returns to the first, so a cycle
  // through the whole graph is what the arrow keys actually do.
  expect(new Set(seen).size).toBe(4);
  expect(await page.evaluate(() => (document.activeElement as HTMLElement)?.dataset.pgNode)).toBe(T_TRANSCRIBE);
});

// ── STALE STATE ─────────────────────────────────────────────────────────────

test("a live stage transition moves the node state and admits the verdict is stale", async ({ page }) => {
  // A private SSE pipe, so a frame here cannot repaint another test's page.
  await page.addInitScript(() => {
    const Native = window.EventSource;
    const pipes: ((data: string) => void)[] = [];
    window.EventSource = class extends Native {
      constructor(...args: ConstructorParameters<typeof Native>) {
        super(...args);
        const self = this;
        pipes.push((injected) => {
        self.dispatchEvent(new MessageEvent("message", { data: injected }));
      });
      }
    } as unknown as typeof EventSource;
    (window as unknown as { __push: (d: string) => void }).__push = (data) => pipes.forEach((p) => p(data));
  });
  await openGraphs(page);
  const section = page.locator(`[data-pg-graph="run:${RUN_ID}"]`);
  await expect(section).toHaveAttribute("data-pg-verdict", "as-loaded");

  await page.evaluate((runId) => {
    (window as unknown as { __push: (d: string) => void }).__push(
      JSON.stringify({
        PipelineUpdated: {
          pipeline: {
            id: runId,
            name: "Actual run",
            trigger: "recording-finished",
            state: "Running",
            stages: [
              { id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc", name: "Generate thumbnail candidates", state: "Running" },
              { id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb", name: "Assemble Casebook", state: "Pending" },
            ],
          },
        },
      }),
    );
  }, RUN_ID);

  // The job table's half is live…
  await expect(page.locator(`[data-pg-node="${STAGE_THUMBS}"]`)).toHaveAttribute("data-pg-stage-state", "Running");
  await expect(page.locator(`[data-pg-node="${STAGE_CASEBOOK}"]`)).toHaveAttribute("data-pg-stage-state", "Pending");
  // …and the registry verdict is not re-derived in the browser to match. It is
  // marked stale, which is the honest answer: nothing has re-asked.
  await expect(section).toHaveAttribute("data-pg-verdict", "stale");
  await expect(section.locator("[data-pg-verdict-note]")).toContainText("still the one loaded with the page");
  // A node whose capability vanished must not keep claiming runnable, so the
  // stale mark is the thing that says the claim is unverified.
  await expect(page.locator(`[data-pg-node="${STAGE_CASEBOOK}"]`)).toHaveAttribute("data-pg-runnable", "true");
  await expect(section).toHaveAttribute("data-pg-verdict", "stale");
});

test("re-rendering disposes the old graph instead of stacking a second one on the page", async ({ page }) => {
  await openGraphs(page);
  const before = await page.locator(".x6-node").count();
  expect(before).toBe(6);

  // Three repaints, which is what a run transition and a Recheck produce in
  // normal use. An X6 Graph that is not disposed keeps its container, its
  // cells and its document listeners, so the failure mode is a page that
  // grows a second copy of the graph and keeps answering keystrokes for a
  // DOM node that is no longer in the document.
  for (let i = 0; i < 3; i++) {
    await page.locator("[data-pg-recheck]").first().click();
    await expect(page.locator('[data-pg-canvas][data-pg-painted="true"]')).toHaveCount(2);
  }
  await expect(page.locator(".x6-node")).toHaveCount(before);
  await expect(page.locator(".x6-graph")).toHaveCount(2);
  // The delegated handler still resolves the section it belongs to after the
  // repaints, so a keypress lands in the live graph and not a detached one.
  const nodes = nodeIds(page, "template:creator_publish");
  await nodes.nth(0).focus();
  await page.keyboard.press("ArrowRight");
  await expect(page.locator(`[data-pg-node="${T_CUEPOINTS}"][aria-pressed="true"]`)).toHaveCount(1);
  await expect(page.locator('[data-pg-detail="template:creator_publish"]')).toContainText("Job table");
});

// ── UNTRUSTED TEXT ──────────────────────────────────────────────────────────

test("a stage name is treated as data: markup, a huge name and a bidi override", async ({ page }) => {
  await page.route("**/api/v1/pipelines/runs", async (route) => {
    const response = await route.fetch();
    const body = await response.json() as { graph: { graphs: Array<{ nodes: Array<Record<string, unknown>> }> } };
    const hostile = '"><script>window.__pwned=1</script>\u202E';
    for (const graph of body.graph.graphs) {
      for (const node of graph.nodes) node.name = hostile;
    }
    body.graph.graphs[1].nodes[0].name = "x".repeat(10 * 1024 * 1024);
    await route.fulfill({ response, json: body });
  });
  await openGraphs(page);
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
  await expect(page.locator("[data-pg-node] script")).toHaveCount(0);
  const title = await page.locator(`[data-pg-node="${T_TRANSCRIBE}"]`).getAttribute("title");
  expect((title || "").length, "a 10 MiB name is not a 10 MiB tooltip").toBeLessThan(160);
  const label = await page.locator(`[data-pg-node="${T_TRANSCRIBE}"]`).getAttribute("aria-label");
  expect((label || "").length).toBeLessThan(400);
});
