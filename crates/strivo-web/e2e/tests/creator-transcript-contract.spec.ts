import { test, expect, type Page } from "@playwright/test";

// The DataViz corpus is assembled SPA-side from Crunchr transcripts, and
// that assembly is where the contract gap lived: `API.crunchrTranscript`
// called `/api/v1/plugins/crunchr/transcript/{id}` — a route that has never
// existed — behind `.catch(() => null)`, so a request that 404s and a
// recording with no transcript were the same value, and the page reported
// "None of the selected recordings have Crunchr transcripts yet" to a user
// who had them.
//
// These specs drive the real journey: pick a finished recording, pick an
// experiment, run. The mock computes the answer from the corpus the SPA
// actually posted (see mock-server.mjs's /dataviz/run branch), so a spec
// that renders a chart has proved the transcript was fetched, parsed, and
// shaped correctly — not merely that a stub replied.

const FINISHED_RECORDING = "11111111-1111-1111-1111-111111111111";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
});

/** Open the DataViz page and select the one finished recording. */
async function selectFinishedRecording(page: Page) {
  await page.goto("/app#/dataviz");
  await expect(page.getByRole("heading", { name: "Data viz" })).toBeVisible();
  await expect(page.locator(".dz-rec-title", { hasText: "Zebra stream" })).toBeVisible();
  await page.locator('.dz-rec-pick[data-id="' + FINISHED_RECORDING + '"]').check();
  await expect(page.locator("#dz-count")).toHaveText("1 selected");
}

test("a real Crunchr transcript drives the DataViz series", async ({ page }) => {
  // The transcript request must be the route the server actually serves.
  const transcriptRequests: string[] = [];
  page.on("request", (r) => {
    if (r.url().includes("/plugins/crunchr/")) transcriptRequests.push(r.url());
  });

  await selectFinishedRecording(page);
  await page.locator('.dz-exp[data-kind="speaker_time"]').click();
  await page.locator("#dz-run").click();

  // The chart rendered, from real data. The mock's crunchr fixture returns
  // two segments for speaker "Alpha" totalling 7.0s, and speaker_time
  // divides by 60, so the only way this label reads "0.1" is if the
  // transcript was fetched, its segments were mapped into utterances, and
  // they were posted as the corpus. A wrong or empty corpus would render
  // "No data points" instead.
  await expect(page.locator("#dz-chart-title")).toHaveText("Speaker minutes (across corpus)");
  await expect(page.locator("#dz-chart")).toContainText("Alpha");
  await expect(page.locator("#dz-chart")).toContainText("0.1");
  // Exactly one bar — one speaker in the fixture, and no phantom points.
  await expect(page.locator("#dz-chart rect")).toHaveCount(1);
  await expect(page.locator("#dz-chart")).not.toContainText("No data points");

  // And it asked for the transcript over the real route, exactly once.
  expect(transcriptRequests).toEqual([
    `http://localhost:8199/api/v1/plugins/crunchr/recordings/${FINISHED_RECORDING}`,
  ]);

  // The empty-state toast must NOT be what happened — this is the exact
  // symptom the mask produced.
  await expect(page.locator(".toast.error")).toHaveCount(0);
});

test("a refused transcript surfaces the refusal instead of an empty corpus", async ({ page }) => {
  // The 402 the Pro gate returns. Under the old `.catch(() => null)` this
  // was indistinguishable from "no transcript"; the two must now differ.
  await page.route("**/api/v1/plugins/crunchr/recordings/*", (route) =>
    route.fulfill({
      status: 402,
      contentType: "application/problem+json",
      body: JSON.stringify({
        type: "about:blank",
        title: "Payment Required",
        status: 402,
        detail: "crunchr is a Strivo Pro plugin — activate or start a 3-day trial from the Plugins page.",
      }),
    }),
  );

  await selectFinishedRecording(page);
  await page.locator("#dz-run").click();

  // A real, attributable error — the gate's own wording, not a generic
  // "HTTP 402" and not "None of the selected recordings have Crunchr
  // transcripts yet".
  const toast = page.locator(".toast.error");
  await expect(toast).toContainText("crunchr is a Strivo Pro plugin");
  await expect(toast).not.toContainText("None of the selected recordings");
  // Nothing was charted: the refusal must not leave a half-built series.
  await expect(page.locator("#dz-chart")).toBeEmpty();
});

test("a recording with genuinely no transcript is skipped, not failed", async ({ page }) => {
  // 404 is the "not transcribed yet" answer and stays a skip. This is the
  // half of the distinction that must NOT regress into a hard error.
  await page.route("**/api/v1/plugins/crunchr/recordings/*", (route) =>
    route.fulfill({
      status: 404,
      contentType: "application/problem+json",
      body: JSON.stringify({
        type: "about:blank",
        title: "Not Found",
        status: 404,
        detail: "recording not transcribed",
      }),
    }),
  );

  await selectFinishedRecording(page);
  await page.locator("#dz-run").click();

  const toast = page.locator(".toast.error");
  await expect(toast).toContainText("None of the selected recordings have Crunchr transcripts yet");
  await expect(toast).not.toContainText("Pro plugin");
  await expect(page.locator("#dz-chart")).toBeEmpty();
});
