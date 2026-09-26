import { test, expect, type Page, type Route } from "@playwright/test";

// Plan todo 29 (report F01), extending todo 7's contract work.
//
// Todo 7 made the client call a route that exists and stopped masking every
// outcome as `null`. What it left is that all three outcomes still land in
// ONE place — `Toast.error` — so "this recording was never transcribed",
// "you are not entitled to Crunchr" and "the service faulted" are three
// differently-worded copies of the same rendered thing. A user cannot act
// on the difference and a test cannot assert on it, which is the
// "indistinguishable error" defect this spec exists to close.
//
// So the chart panel now carries a state, not just a toast:
//
//   data-dz-state="idle"     the page has not run an experiment yet
//   data-dz-state="chart"    a real series, rendered
//   data-dz-state="skipped"  no selected recording has a transcript (404)
//   data-dz-state="refused"  the Pro gate refused the transcript fetch (402)
//   data-dz-state="failed"   the fetch or the run itself faulted (5xx/other)
//
// The state attribute is the assertion seam, and it is deliberately NOT the
// toast: todo 7's three specs assert the toast text and assert
// `#dz-chart` is empty on the two failure paths, so the state text lives
// outside `#dz-chart` and the existing contract is preserved rather than
// rewritten. Each state's text also names *why* it is that state, so a
// collapsed implementation is not merely a different attribute value.
//
// On the assertion style: a single "some error is shown" check passes for
// all three failures and is the defect, not the fix. Every failure spec
// therefore pins the attribute, the leading word of its own text, its own
// cause, AND the absence of the other two states' words — so if the three
// were ever collapsed, three specs go red at once.

const FINISHED_RECORDING = "11111111-1111-1111-1111-111111111111";
const TRANSCRIPT_GLOB = "**/api/v1/plugins/crunchr/recordings/*";

/** The 402 the Pro gate returns, verbatim in shape. */
const REFUSAL = {
  status: 402,
  contentType: "application/problem+json",
  body: JSON.stringify({
    type: "about:blank",
    title: "Payment Required",
    status: 402,
    detail:
      "crunchr is a Strivo Pro plugin — activate or start a 3-day trial from the Plugins page.",
  }),
};

function problem(status: number, detail: string) {
  return {
    status,
    contentType: "application/problem+json",
    body: JSON.stringify({
      type: "about:blank",
      title: status === 404 ? "Not Found" : "Internal Server Error",
      status,
      detail,
    }),
  };
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
});

async function openDataviz(page: Page) {
  await page.goto("/app#/dataviz");
  await expect(page.getByRole("heading", { name: "Data viz" })).toBeVisible();
}

async function selectFinishedAndRun(page: Page, experiment = "speaker_time") {
  await expect(page.locator(".dz-rec-title", { hasText: "Zebra stream" })).toBeVisible();
  await page.locator(`.dz-rec-pick[data-id="${FINISHED_RECORDING}"]`).check();
  await expect(page.locator("#dz-count")).toHaveText("1 selected");
  await page.locator(`.dz-exp[data-kind="${experiment}"]`).click();
  await page.locator("#dz-run").click();
}

/** The state the chart panel settled on. Read as text, not a boolean, so a
 *  missing attribute is distinguishable from a wrong one. */
async function chartState(page: Page) {
  return page.locator("#dz-chart").getAttribute("data-dz-state");
}

// ── success ──────────────────────────────────────────────────────────

test("a real transcript renders the chart and reports a covered corpus", async ({ page }) => {
  await openDataviz(page);
  await selectFinishedAndRun(page);

  // Falsifiability of the state: the page starts idle, so "chart" is a fact
  // about the run rather than a fact about page load.
  await expect.poll(() => chartState(page)).toBe("chart");

  // Real data, not a stub echo. The mock's transcript has Alpha at 0.0–3.5
  // and 3.5–7.0, and speaker_time divides seconds by 60, so 7.0s is the
  // only way this reads 0.1.
  await expect(page.locator("#dz-chart-title")).toHaveText("Speaker minutes (across corpus)");
  await expect(page.locator("#dz-chart")).toContainText("Alpha");
  await expect(page.locator("#dz-chart")).toContainText("0.1");
  await expect(page.locator("#dz-chart rect")).toHaveCount(1);

  // Every utterance in this fixture carries an end time, so there is
  // nothing to disclose. Asserting the note is ABSENT is what makes its
  // presence in the next test meaningful.
  await expect(page.locator("#dz-coverage")).toBeHidden();

  // No state text, because there is no state to explain.
  await expect(page.locator("#dz-state")).toBeHidden();
});

// ── the three failure fixtures ───────────────────────────────────────

test("a 404 transcript skips the recording: state skipped, not failed", async ({ page }) => {
  await page.route(TRANSCRIPT_GLOB, (route: Route) =>
    route.fulfill(problem(404, "recording not transcribed")),
  );
  await openDataviz(page);
  await selectFinishedAndRun(page);

  await expect.poll(() => chartState(page)).toBe("skipped");
  const note = page.locator("#dz-state");
  await expect(note).toBeVisible();
  await expect(note).toContainText("Skipped");
  await expect(note).toContainText("no Crunchr transcript");
  // The two other states must be absent by name, not merely absent.
  await expect(note).not.toContainText("Refused");
  await expect(note).not.toContainText("Failed");
  // A skip is not a fault, so it must not read as one.
  await expect(page.locator(".toast.error")).not.toContainText("HTTP 404");
  // The plan's failure scenario: the UI says "no transcript" instead of
  // rendering an empty chart.
  await expect(page.locator("#dz-chart")).toBeEmpty();
});

test("a 402 transcript is a refusal, not a missing transcript", async ({ page }) => {
  await page.route(TRANSCRIPT_GLOB, (route: Route) => route.fulfill(REFUSAL));
  await openDataviz(page);
  await selectFinishedAndRun(page);

  await expect.poll(() => chartState(page)).toBe("refused");
  const note = page.locator("#dz-state");
  await expect(note).toBeVisible();
  // The gate's own wording survives to the panel — the actionable half of
  // a refusal is WHICH capability to activate.
  await expect(note).toContainText("crunchr is a Strivo Pro plugin");
  await expect(note).toContainText("Refused");
  await expect(note).not.toContainText("Skipped");
  await expect(note).not.toContainText("Failed");
  await expect(page.locator("#dz-chart")).toBeEmpty();
});

test("a 500 transcript is a service fault, not a refusal", async ({ page }) => {
  await page.route(TRANSCRIPT_GLOB, (route: Route) =>
    route.fulfill(problem(500, "crunchr store is locked by another process")),
  );
  await openDataviz(page);
  await selectFinishedAndRun(page);

  await expect.poll(() => chartState(page)).toBe("failed");
  const note = page.locator("#dz-state");
  await expect(note).toBeVisible();
  await expect(note).toContainText("Failed");
  // The server's own words, so the operator has something to act on.
  await expect(note).toContainText("crunchr store is locked by another process");
  // The distinction the plan is really about: a 500 must not borrow the
  // 402's wording, or an operator cannot tell "activate your licence" from
  // "restart the thing holding the database".
  await expect(note).not.toContainText("Pro plugin");
  await expect(note).not.toContainText("Skipped");
  await expect(note).not.toContainText("Refused");
  await expect(page.locator("#dz-chart")).toBeEmpty();
});

test("a 500 on the run endpoint is a fault too, not an empty chart", async ({ page }) => {
  // The failure can arrive from the second hop, and it must not be reported
  // as "nothing to chart".
  await page.route("**/api/v1/dataviz/run", (route: Route) =>
    route.fulfill(problem(500, "series transform panicked")),
  );
  await openDataviz(page);
  await selectFinishedAndRun(page);

  await expect.poll(() => chartState(page)).toBe("failed");
  await expect(page.locator("#dz-state")).toContainText("series transform panicked");
  await expect(page.locator("#dz-state")).not.toContainText("no Crunchr transcript");
  await expect(page.locator("#dz-chart")).toBeEmpty();
});

test("the three failure states are distinguishable from one another", async ({ page }) => {
  // Guards the whole spec's premise in one place: if two of the states ever
  // rendered the same thing, this is the assertion that notices, without
  // having to reason about which of the other specs should go red.
  const seen: Record<string, string> = {};
  for (const [state, fulfil] of [
    ["skipped", () => problem(404, "recording not transcribed")],
    ["refused", () => REFUSAL],
    ["failed", () => problem(500, "boom")],
  ] as const) {
    const p = await page.context().newPage();
    await p.route(TRANSCRIPT_GLOB, (route: Route) => route.fulfill(fulfil()));
    await p.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
    await p.goto("/app#/dataviz");
    await p.locator(`.dz-rec-pick[data-id="${FINISHED_RECORDING}"]`).check();
    await p.locator('.dz-exp[data-kind="speaker_time"]').click();
    await p.locator("#dz-run").click();
    await expect.poll(() => p.locator("#dz-chart").getAttribute("data-dz-state")).toBe(state);
    seen[state] = (await p.locator("#dz-state").innerText()).trim();
    await p.close();
  }
  const texts = Object.values(seen);
  expect(new Set(texts).size).toBe(3);
  expect(new Set(Object.keys(seen)).size).toBe(3);
});

// ── explicit missing timing (the removed one-second fallback) ───────

test("an utterance with no end time is sent as null, never as a fabricated second", async ({ page }) => {
  // The defect: `end_sec: u.end_sec || (u.start_sec || 0) + 1` invented one
  // second of speech for every line the transcript gave no end time, and
  // every duration experiment then reported it. The corpus below has two
  // measured lines (3.5s + 3.5s) and one with no end time, so a UI still
  // fabricating would post `end_sec: 8.0` on the last line.
  let posted: any = null;
  page.on("request", (req) => {
    if (req.url().includes("/api/v1/dataviz/run") && req.method() === "POST") {
      posted = JSON.parse(req.postData() || "{}");
    }
  });
  await page.route(TRANSCRIPT_GLOB, (route: Route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        recording_id: FINISHED_RECORDING,
        channel_name: "Alpha",
        title: "Zebra stream",
        status: "complete",
        segments: [
          { index: 0, start_sec: 0.0, end_sec: 3.5, text: "hey", speaker: "Alpha" },
          { index: 1, start_sec: 3.5, end_sec: 7.0, text: "welcome back", speaker: "Alpha" },
          { index: 2, start_sec: 7.0, end_sec: null, text: "and then", speaker: "Alpha" },
        ],
      }),
    }),
  );

  await openDataviz(page);
  await selectFinishedAndRun(page);

  await expect.poll(() => chartState(page)).toBe("chart");
  const u = posted.corpus.episodes[0].utterances;
  expect(u).toHaveLength(3);
  // The load-bearing assertion: null, not 8.0.
  expect(u[2].end_sec).toBeNull();
  expect(u[0].end_sec).toBe(3.5);
  // And the UI says so, rather than presenting a total that quietly omits
  // a third of the corpus.
  const coverage = page.locator("#dz-coverage");
  await expect(coverage).toBeVisible();
  await expect(coverage).toContainText("1 of 3");
  await expect(coverage).toContainText("no end time");
  // Only the measured 7.0s is charted — 8.0s would read 0.1 and 0.1 would
  // not be distinguishable from the success fixture, which is exactly the
  // vacuous comparison this test refuses to make.
  await expect(page.locator("#dz-chart")).toContainText("0.1");
});

test("a measured zero-length line is not reported as missing timing", async ({ page }) => {
  // `end_sec: 0` is falsy in JavaScript. The old `||` could not tell a
  // measured zero from an absent value, which is why the type is now an
  // Option on the wire; this pins the UI half of that distinction.
  let posted: any = null;
  page.on("request", (req) => {
    if (req.url().includes("/api/v1/dataviz/run") && req.method() === "POST") {
      posted = JSON.parse(req.postData() || "{}");
    }
  });
  await page.route(TRANSCRIPT_GLOB, (route: Route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        recording_id: FINISHED_RECORDING,
        channel_name: "Alpha",
        title: "Zebra stream",
        status: "complete",
        segments: [
          { index: 0, start_sec: 0.0, end_sec: 0.0, text: "click", speaker: "Alpha" },
          { index: 1, start_sec: 0.0, end_sec: 6.0, text: "hello", speaker: "Alpha" },
        ],
      }),
    }),
  );

  await openDataviz(page);
  await selectFinishedAndRun(page);

  await expect.poll(() => chartState(page)).toBe("chart");
  expect(posted.corpus.episodes[0].utterances[0].end_sec).toBe(0);
  // Nothing is missing, so nothing is disclosed.
  await expect(page.locator("#dz-coverage")).toBeHidden();
  await expect(page.locator("#dz-chart")).toContainText("0.1");
});

// ── stale state (the 2s GET cache) ───────────────────────────────────

test("a failure is not cached: a later successful run in the same page charts", async ({ page }) => {
  // `API._fetch` collapses duplicate GETs behind a 2s window, so a state
  // read inside that window could be a cached one. A *rejection* must never
  // enter that cache, or the second run would inherit the first run's
  // failure and the panel would be stale rather than wrong.
  let mode: "fail" | "ok" = "fail";
  await page.route(TRANSCRIPT_GLOB, (route: Route) =>
    mode === "fail"
      ? route.fulfill(problem(500, "transient store lock"))
      : route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            recording_id: FINISHED_RECORDING,
            channel_name: "Alpha",
            title: "Zebra stream",
            status: "complete",
            segments: [{ index: 0, start_sec: 0.0, end_sec: 7.0, text: "hi", speaker: "Alpha" }],
          }),
        }),
  );

  await openDataviz(page);
  await page.locator(`.dz-rec-pick[data-id="${FINISHED_RECORDING}"]`).check();
  await page.locator('.dz-exp[data-kind="speaker_time"]').click();
  await page.locator("#dz-run").click();
  await expect.poll(() => chartState(page)).toBe("failed");

  mode = "ok";
  await page.locator("#dz-run").click();
  await expect.poll(() => chartState(page)).toBe("chart");
  await expect(page.locator("#dz-state")).toBeHidden();
  await expect(page.locator("#dz-chart")).toContainText("Alpha");
});

// ── model-produced text reaching the UI ──────────────────────────────

test("transcript text containing markup is escaped, not injected", async ({ page }) => {
  // A transcript is model-produced text that reaches a chart label. The
  // word-frequency experiment puts transcript words straight into an SVG
  // <text> node, so this is the injection surface.
  let alerted = false;
  page.on("dialog", async (d) => {
    alerted = true;
    await d.dismiss();
  });
  await page.route(TRANSCRIPT_GLOB, (route: Route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        recording_id: FINISHED_RECORDING,
        channel_name: "Alpha",
        title: "Zebra stream",
        status: "complete",
        segments: [
          { index: 0, start_sec: 0.0, end_sec: 2.0, speaker: "Alpha", text: "<script>window.__pwned=1</script>" },
          { index: 1, start_sec: 2.0, end_sec: 4.0, speaker: "Alpha", text: '<img src=x onerror="window.__pwned=1">' },
          { index: 2, start_sec: 4.0, end_sec: 6.0, speaker: "Alpha", text: "</svg><script>window.__pwned=2</script>" },
        ],
      }),
    }),
  );

  await openDataviz(page);
  await selectFinishedAndRun(page, "word_frequency");

  await expect.poll(() => chartState(page)).toBe("chart");
  // Nothing executed. Both the payload variable and the dialog path are
  // checked because an injected inline handler can fire either way.
  expect(await page.evaluate(() => (window as any).__pwned)).toBeUndefined();
  expect(alerted).toBe(false);
  expect(await page.locator("#dz-chart script").count()).toBe(0);
  expect(await page.locator("#dz-chart img").count()).toBe(0);
  // The words are still charted, as text.
  await expect(page.locator("#dz-chart")).toContainText("script");
  await expect(page.locator("#dz-chart")).toContainText("window");
});
