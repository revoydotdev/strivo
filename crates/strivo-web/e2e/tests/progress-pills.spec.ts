import { test, expect } from "@playwright/test";

// Regression coverage for progress pills stuck at 0%: download_pct /
// download_eta_secs / download_rate_bps / bytes_written arrive ONLY via
// SSE RecordingProgress ticks, patched in place onto the in-memory
// recCache array (036-pvr.js). Several surfaces never saw those ticks:
// the Recordings Timeline (built from a separate REST /history
// snapshot) and the Recording Info modal (a point-in-time
// API.recordingOne() fetch with no SSE subscription of its own).
//
// mock-server.mjs has no built-in RecordingProgress simulation and is
// out of this lane's ownership, so this spec drives its own network
// mocks via page.route — entirely self-contained, no shared-server
// changes required.

const JOB_ID = "44444444-4444-4444-4444-444444444444";

function jobFixture(overrides: Record<string, unknown> = {}) {
  return {
    id: JOB_ID,
    channel_name: "Delta",
    stream_title: "Progress test",
    state: "Recording",
    source_url: "https://twitch.tv/videos/999",
    started_at: new Date().toISOString(),
    bytes_written: 0,
    download_pct: null,
    ...overrides,
  };
}

// SSE frame in the exact shape events.source.onmessage expects
// (JSON.parse(e.data)), matching mock-server.mjs's own `broadcast()`
// framing (`data: <json>\n\n`).
function sseFrame(eventObj: unknown) {
  return `data: ${JSON.stringify(eventObj)}\n\n`;
}

async function installProgressJob(
  page: import("@playwright/test").Page,
  { ticks }: { ticks: Record<string, unknown>[] },
) {
  // Full recordings list: add the in-progress job alongside the mock's
  // three fixtures.
  await page.route("**/api/v1/recordings*", async (route) => {
    if (route.request().method() !== "GET") return route.continue();
    const response = await route.fetch();
    const body = await response.json();
    body.recordings = [...body.recordings, jobFixture()];
    if (typeof body.total === "number") body.total += 1;
    await route.fulfill({ response, json: body });
  });

  // Single-job fetch, used by the Recording Info modal.
  await page.route(`**/api/v1/recordings/${JOB_ID}`, async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(
        jobFixture({
          channel_id: "twitch:delta",
          platform: "Twitch",
          transcode: false,
          duration_secs: 0,
          output_path: `/mnt/sda2/strivo/Delta/${JOB_ID}.mkv`,
          error: null,
        }),
      ),
    });
  });

  // History snapshot: add a matching row so the Timeline view shows it.
  await page.route("**/api/v1/history*", async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    body.history = [
      ...body.history,
      {
        id: JOB_ID,
        channel_name: "Delta",
        stream_title: "Progress test",
        platform: "Twitch",
        state: "Recording",
        source_url: "https://twitch.tv/videos/999",
        started_at: jobFixture().started_at,
        bytes_written: 0,
      },
    ];
    await route.fulfill({ response, json: body });
  });

  // Replace the SSE stream outright with one or more canned
  // RecordingProgress ticks, delivered immediately on connect. This
  // spec doesn't depend on any other live event.
  await page.route("**/events", async (route) => {
    const body = ticks
      .map((t) =>
        sseFrame({
          RecordingProgress: {
            job_id: JOB_ID,
            bytes_written: 0,
            duration_secs: 0,
            ...t,
          },
        }),
      )
      .join("");
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body,
    });
  });
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
});

test("Timeline row's percent updates from a live SSE tick, never stuck", async ({ page }) => {
  await installProgressJob(page, {
    ticks: [{ bytes_written: 5_000_000, duration_secs: 10, download_pct: 37, download_eta_secs: 120, download_rate_bps: 500_000 }],
  });
  await page.goto("/app#/recordings?view=timeline");
  await expect(page.getByRole("heading", { name: "Recordings" })).toBeVisible();

  const pill = page.locator(`.hist-pill[data-job-id="${JOB_ID}"]`);
  await expect(pill).toBeVisible();
  await expect(pill.locator(".state-pill-label")).toHaveText("37%");
});

test("Recording Info modal shows a live percent while open and updates on a later tick", async ({ page }) => {
  await installProgressJob(page, {
    ticks: [
      { bytes_written: 3_000_000, duration_secs: 6, download_pct: 25, download_eta_secs: 200, download_rate_bps: 400_000 },
      { bytes_written: 8_000_000, duration_secs: 16, download_pct: 62, download_eta_secs: 90, download_rate_bps: 550_000 },
    ],
  });
  await page.goto("/app#/recordings?view=timeline");
  const pill = page.locator(`.hist-pill[data-job-id="${JOB_ID}"]`);
  await expect(pill).toBeVisible();

  await pill.locator("[data-action=rec-info]").click();
  const modal = page.locator("#rec-info-modal");
  await expect(modal).toBeVisible();
  // Both mocked ticks are delivered on connect (Playwright serves the
  // whole SSE body at once); the modal's live-patch handler
  // (updateRecInfoModalProgress, 028-pvr.js) applies each as it's
  // parsed, so the DOM settles on the last one.
  await expect(modal.locator(".rec-info-head .state-pill-label")).toHaveText("62%");
});

test("vod-dl-label never shows a literal 0% when download_pct is null", async ({ page }) => {
  await page.goto("/app#/library");
  await page.locator(".ch-row", { hasText: "Live Channel" }).click();
  await expect(page.getByText("Yesterday's livestream")).toBeVisible();
  const dlBtn = page
    .locator(".media-pill", { hasText: "Yesterday's livestream" })
    .locator(".vod-dl");
  await dlBtn.click();
  await expect(dlBtn).toHaveClass(/vod-dl-downloading/);
  const label = dlBtn.locator(".vod-dl-label");
  await expect(label).toBeVisible();
  // download_pct is null for this job (no SSE tick ever arrives in this
  // flow) — the label must never render the literal string "0%".
  await expect(label).not.toHaveText("0%");
  await expect(label).not.toContainText("0%");
});

// Regression coverage for the VOD download pill: a monotonic
// "downloading"/"downloaded" state map that nothing ever cleared left the
// pill stuck forever once its job disappeared (pruned, failed, deleted),
// and a long "NN% · Xh Ym left · R MB/s" label squeezed the progress bar
// to zero width. seedVodDownloadStateFromRecCache() (012c-pvr.js) now
// REBUILDS vodDownloadState from recCache on every call instead of only
// ever upgrading it, and the CSS stacks bar-above-label instead of
// sharing a row. This VOD ("Yesterday's livestream" / stream1, under Live
// Channel's Past Broadcasts) is the same one used by the 0%-label test
// above; each test here installs its own job via `**/api/v1/recordings*`
// so the scenarios stay self-contained per mock-server.mjs's own
// documented convention (see file header comment).
//
// The mock server is one process shared by every parallel worker/test, and
// `/__test__/broadcast` (used below) fans an event out to EVERY open SSE
// connection, not just the page that requested it — so a hardcoded job id
// shared across tests would let one test's RecordingsPruned prune another
// concurrently-running test's job out from under it. Each test gets its
// own random id instead.
const VOD_URL = "https://youtu.be/stream1";

function vodJobFixture(jobId: string, overrides: Record<string, unknown> = {}) {
  return {
    id: jobId,
    channel_name: "Live Channel",
    stream_title: "Yesterday's livestream",
    state: "Recording",
    source_url: VOD_URL,
    started_at: new Date().toISOString(),
    bytes_written: 0,
    download_pct: null,
    ...overrides,
  };
}

async function installVodJob(
  page: import("@playwright/test").Page,
  job: Record<string, unknown>,
) {
  await page.route("**/api/v1/recordings*", async (route) => {
    if (route.request().method() !== "GET") return route.continue();
    const response = await route.fetch();
    const body = await response.json();
    body.recordings = [...body.recordings, job];
    if (typeof body.total === "number") body.total += 1;
    await route.fulfill({ response, json: body });
  });
}

function vodPillButton(page: import("@playwright/test").Page) {
  return page
    .locator(".media-pill", { hasText: "Yesterday's livestream" })
    .locator(".vod-dl");
}

test("downloading pill shows a non-collapsed bar even with a long ETA/rate label", async ({ page }) => {
  await installVodJob(
    page,
    vodJobFixture(crypto.randomUUID(), {
      download_pct: 57,
      download_eta_secs: 3725, // 1h 2m
      download_rate_bps: 12_300_000, // 12.3 MB/s
    }),
  );
  await page.goto("/app#/library");
  await page.locator(".ch-row", { hasText: "Live Channel" }).click();
  await expect(page.getByText("Yesterday's livestream")).toBeVisible();

  const dlBtn = vodPillButton(page);
  await expect(dlBtn).toHaveClass(/vod-dl-downloading/);
  const label = dlBtn.locator(".vod-dl-label");
  await expect(label).toContainText("57%");
  await expect(label).toContainText("1h 2m left");
  await expect(label).toContainText("12.3 MB/s");
  // The full, untruncated label lives in the button's title attribute so
  // it stays readable even when the visible text is CSS-ellipsized.
  await expect(dlBtn).toHaveAttribute("title", "57% · 1h 2m left · 12.3 MB/s");

  // Read both rects in one atomic DOM query per poll — two sequential
  // locator.boundingBox() calls can straddle a live repaint (the mock's
  // ChannelVods SSE answer, or a later recordings refresh) and observe a
  // stale/detached element mid-swap. expect.poll rides out that kind of
  // transient (or a slow layout pass under a loaded test runner) while
  // still failing for real on a genuine collapse-to-zero regression,
  // which — unlike a mid-swap glitch — persists for the whole window.
  const readRects = () =>
    dlBtn.evaluate((btn) => {
      const bar = btn.querySelector(".vod-dl-bar");
      const fill = btn.querySelector(".vod-dl-fill");
      return {
        bar: bar ? bar.getBoundingClientRect().width : 0,
        fill: fill ? fill.getBoundingClientRect().width : 0,
      };
    });
  // The regression: a long label used to squeeze the bar to ~0 width. Poll
  // on both values from the SAME read together — polling them separately
  // can straddle a repaint and see one before, one after.
  await expect
    .poll(async () => {
      const r = await readRects();
      // The bar must render at a real width, and the fill (57%) must
      // occupy a visible share of it.
      return r.bar > 40 && r.fill > r.bar * 0.3;
    })
    .toBe(true);
});

test("downloading pill self-heals to Download when its job is pruned", async ({ page }) => {
  const jobId = crypto.randomUUID();
  await installVodJob(page, vodJobFixture(jobId));
  await page.goto("/app#/library");
  await page.locator(".ch-row", { hasText: "Live Channel" }).click();
  await expect(page.getByText("Yesterday's livestream")).toBeVisible();

  const dlBtn = vodPillButton(page);
  // Confirm the pill actually reached "downloading" (seeded from the job
  // already in recCache) before pruning it — otherwise this wouldn't be
  // testing self-healing at all.
  await expect(dlBtn).toHaveClass(/vod-dl-downloading/);

  // Real daemon prune, delivered over the real SSE connection (not a
  // replacement of the whole /events route, which would also cut off the
  // ChannelVods answer the pill above depends on).
  await page.request.post("/__test__/broadcast", {
    data: { RecordingsPruned: { job_ids: [jobId] } },
  });

  // The derived state must settle on idle "Download" — never stuck on
  // the empty "Downloading…" fallback the old monotonic map left behind
  // once its job vanished from recCache.
  await expect(dlBtn).toHaveClass(/vod-dl-idle/);
  await expect(dlBtn).toHaveText("Download");
});

test("a Failed job leaves the pill idle, not stuck downloading", async ({ page }) => {
  await installVodJob(page, vodJobFixture(crypto.randomUUID(), { state: "Failed" }));
  await page.goto("/app#/library");
  await page.locator(".ch-row", { hasText: "Live Channel" }).click();
  await expect(page.getByText("Yesterday's livestream")).toBeVisible();

  const dlBtn = vodPillButton(page);
  await expect(dlBtn).toHaveClass(/vod-dl-idle/);
  await expect(dlBtn).toHaveText("Download");
});

test("a Finished job shows Downloaded", async ({ page }) => {
  await installVodJob(page, vodJobFixture(crypto.randomUUID(), { state: "Finished", file_exists: true }));
  await page.goto("/app#/library");
  await page.locator(".ch-row", { hasText: "Live Channel" }).click();
  await expect(page.getByText("Yesterday's livestream")).toBeVisible();

  const dlBtn = vodPillButton(page);
  await expect(dlBtn).toHaveClass(/vod-dl-downloaded/);
  await expect(dlBtn).toHaveText("Downloaded");
});

// ── Recent uploads never link out (mirrors the Past Broadcasts rule) ──
// mock-server.mjs's ChannelVods answer for every channel includes an
// "upload1" Upload-kind entry (url https://youtu.be/upload1, title "How I
// edit my videos") that renders under the "Recent uploads" section —
// vodSectionHtml (012c-pvr.js) used to only apply the never-link-out rule
// to "Past Broadcasts"; it now applies everywhere.

const UPLOAD_URL = "https://youtu.be/upload1";

function uploadJobFixture(jobId: string, overrides: Record<string, unknown> = {}) {
  return {
    id: jobId,
    channel_name: "Live Channel",
    stream_title: "How I edit my videos",
    source_url: UPLOAD_URL,
    started_at: new Date().toISOString(),
    bytes_written: 0,
    download_pct: null,
    ...overrides,
  };
}

function uploadRow(page: import("@playwright/test").Page) {
  return page.locator(".media-pill", { hasText: "How I edit my videos" }).locator(".mp-link");
}

test("Recent uploads row has no external link, and is inert while downloading", async ({ page }) => {
  await installVodJob(page, uploadJobFixture(crypto.randomUUID(), { state: "Recording" }));
  await page.goto("/app#/library");
  await page.locator(".ch-row", { hasText: "Live Channel" }).click();
  await expect(page.getByText("How I edit my videos")).toBeVisible();

  const row = uploadRow(page);
  await expect(row).toHaveCount(1);
  await expect(page.locator(".media-pill", { hasText: "How I edit my videos" }).locator("a")).toHaveCount(0);
  await expect(row).toHaveClass(/mp-link-inert/);
});

test("a Finished upload's row opens the in-app player, never the source platform", async ({ page }) => {
  const jobId = crypto.randomUUID();
  await installVodJob(page, uploadJobFixture(jobId, { state: "Finished", file_exists: true }));
  await page.goto("/app#/library");
  await page.locator(".ch-row", { hasText: "Live Channel" }).click();
  await expect(page.getByText("How I edit my videos")).toBeVisible();

  const row = uploadRow(page);
  await expect(row).not.toHaveClass(/mp-link-inert/);
  await expect(row).toHaveAttribute("data-job-id", jobId);

  await row.click();
  await page.waitForFunction(() => /#\/play\?recording=/.test(window.location.hash));
  expect(page.url()).toContain(`#/play?recording=${jobId}`);
});
