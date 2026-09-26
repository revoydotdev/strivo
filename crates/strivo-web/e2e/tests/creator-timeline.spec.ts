import { test, expect, type Page } from "@playwright/test";

// The multi-track timeline (todo 43). Four claims are under test, and each is
// written so that a surface which merely *renders something* fails it:
//
//   1. INTERVAL — the plan's clips are placed at the microsecond coordinates
//      the server sent, and an edit is sent as an *intent* rather than applied
//      locally. Asserted on exact `data-start`/`data-end` values, on the
//      derived pixel geometry, and on the request body.
//   2. TIMING — the keyboard frame step is a whole number of microseconds
//      (33_334 at 30fps, not 33_333), because a frame boundary is not a whole
//      microsecond. Asserted on the playhead position after two presses.
//   3. MIME — the vendored ES module is fetched with a content type a browser
//      executes, and evaluates: a module that was fetched but refused leaves
//      the surface's degraded-vendor panel in place.
//   4. DEGRADED MODE — a browser that cannot decode says so, names the remedy,
//      and still draws the timeline. The feature detection is a pure function
//      over a capability record, so it is driven directly with an
//      `AudioDecoder`-less browser rather than by hoping a real one appears.

const CLIP_COUNT = 2;
const COMPOSITION_US = 2_500_000;
const FIRST_CLIP_END_US = 1_000_000;
const FRAME_US = 33_334; // ceil(1e6 / 30)

type Edit =
  | { op: "split"; clip: number; at: number }
  | { op: "ripple"; range: { start: number; end: number } }
  | { op: "link"; clip: number; with: number; linked: boolean }
  | { op: "trim"; clip: number; edge: "in" | "out"; to: number }
  | { op: "slip"; clip: number; by: number };

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
});

async function openTimeline(page: Page) {
  await page.goto("/app#/timeline");
  await expect(page.getByRole("heading", { name: "Timeline" })).toBeVisible();
  await expect(page.locator("[data-tl-clip]")).toHaveCount(CLIP_COUNT);
}

/** Every edit intent the surface sent, in order. */
function captureEdits(page: Page): Edit[] {
  const edits: Edit[] = [];
  page.on("request", (r) => {
    if (r.url().includes("/creator/timeline/apply") && r.method() === "POST") {
      const body = r.postDataJSON() as { edit: Edit | null };
      if (body.edit) edits.push(body.edit);
    }
  });
  return edits;
}

/** The pure feature-detection function, driven with a capability record. */
async function capability(page: Page, env: Record<string, unknown>) {
  // The function lives in the assembled bundle, so the page has to have loaded
  // it; a capability record on `about:blank` would be testing nothing.
  if (!(await page.locator("[data-tl-clip]").count())) {
    await openTimeline(page);
  }
  return page.evaluate(
    (e) =>
      (
        globalThis as unknown as {
          timelinePreviewCapability: (env: unknown) => Record<string, unknown>;
        }
      ).timelinePreviewCapability(e),
    env,
  );
}

// ── 1. INTERVAL ────────────────────────────────────────────────────────────

test("the timeline places every clip at the plan's microsecond coordinates", async ({ page }) => {
  await openTimeline(page);
  const clips = page.locator("[data-tl-clip]");
  // Not "two boxes": the plan places clip 0 at output 0..1_000_000 and clip 1
  // at 1_000_000..2_500_000. A surface that drew a decorative bar chart of
  // "two clips" passes a count check and fails this one.
  await expect(clips.nth(0)).toHaveAttribute("data-start", "0");
  await expect(clips.nth(0)).toHaveAttribute("data-end", String(FIRST_CLIP_END_US));
  await expect(clips.nth(1)).toHaveAttribute("data-start", String(FIRST_CLIP_END_US));
  await expect(clips.nth(1)).toHaveAttribute("data-end", String(COMPOSITION_US));
  // Geometry is derived from the coordinate and the measured lane, so the
  // assertion is a ratio rather than a pixel count that would change with the
  // viewport: the second clip starts a third of the way along a 2.5s
  // composition and is 1.5s long.
  const lane = await page.locator("#tl-clips").evaluate((el) => el.clientWidth);
  const left = await clips.nth(1).evaluate((el) => (el as HTMLElement).style.left);
  expect(parseFloat(left)).toBeCloseTo((FIRST_CLIP_END_US / COMPOSITION_US) * lane, 1);
  const width = await clips.nth(1).evaluate((el) => (el as HTMLElement).style.width);
  expect(parseFloat(width)).toBeCloseTo((1_500_000 / COMPOSITION_US) * lane, 1);
});

test("an edit is sent as an intent, and the plan that comes back is what is drawn", async ({ page }) => {
  const edits = captureEdits(page);
  await openTimeline(page);
  await page.locator("[data-tl-clip]").first().focus();
  await page.keyboard.press(".");
  await page.keyboard.press(".");
  await page.keyboard.press("s");
  await expect.poll(() => edits.length).toBe(1);
  // Split is stated in *composition* time, because that is what a playhead
  // means, and it names a clip rather than a pixel.
  expect(edits[0]).toEqual({ op: "split", clip: 0, at: 2 * FRAME_US });
  expect(edits[0]).not.toHaveProperty("source");
  expect(edits[0]).not.toHaveProperty("px");
});

test("selecting by arrow key moves between real clips and Escape clears it", async ({ page }) => {
  const edits = captureEdits(page);
  await openTimeline(page);
  // Focus goes to a clip, not to the lane: the lane is a container, and making
  // it a focus target gave it a 4px clear space, which fails WCAG 2.2 AA
  // target-size. The clips are the things you act on.
  await page.locator("[data-tl-clip]").first().focus();
  await expect(page.locator('[data-tl-clip][data-selected="true"]')).toHaveCount(0);
  await page.keyboard.press("ArrowRight");
  await expect(page.locator('[data-tl-clip][data-selected="true"]')).toHaveAttribute("data-cut", "0");
  await page.keyboard.press("ArrowRight");
  await expect(page.locator('[data-tl-clip][data-selected="true"]')).toHaveAttribute("data-cut", "1");
  await page.keyboard.press("l");
  await expect.poll(() => edits.length).toBe(1);
  expect(edits[0]).toEqual({ op: "link", clip: 1, with: 0, linked: true });
  // Todo 16 found a dialog that did not close on Escape; the selection is the
  // thing that has to give, and it has to give with no residue.
  await page.keyboard.press("Escape");
  await expect(page.locator('[data-tl-clip][data-selected="true"]')).toHaveCount(0);
});

test("select, edit, undo and export complete on the keyboard alone", async ({ page }) => {
  const edits = captureEdits(page);
  await openTimeline(page);
  await page.locator("[data-tl-clip]").first().focus();
  await page.keyboard.press("ArrowRight"); // select the first clip
  await page.keyboard.press("."); // playhead forward one frame
  await page.keyboard.press("x"); // ripple delete
  await expect.poll(() => edits.length).toBe(1);
  expect(edits[0].op).toBe("ripple");
  await page.keyboard.press("u"); // undo
  await expect(page.locator("[data-tl-clip]")).toHaveCount(CLIP_COUNT);
  await page.keyboard.press("e"); // export contract
  await expect(page.locator('[data-tl-notice="export"]')).toContainText("Export contract");
  // Every clip control exposes a name and a state that TRACKS the selection —
  // not a name and a constant. Asserting `aria-selected="false"` on an
  // unselected clip passes just as well as a hard-coded attribute would.
  const clips = page.locator("[data-tl-clip]");
  await expect(clips.nth(0)).toHaveAttribute("aria-selected", "true");
  await expect(clips.nth(1)).toHaveAttribute("aria-selected", "false");
  await expect(clips.nth(0)).toHaveAttribute("aria-label", /Clip 0, 0:00\.000 to 0:01\.000/);
  await expect(clips.nth(1)).toHaveAttribute("aria-label", /Clip 1, 0:01\.000 to 0:02\.500/);
});

// ── 2. TIMING ──────────────────────────────────────────────────────────────

test("the playhead moves by a whole frame in microseconds, not a rounded one", async ({ page }) => {
  await openTimeline(page);
  const playhead = page.locator("#tl-playhead");
  await expect(playhead).toHaveCSS("left", "0px");
  await page.locator("[data-tl-clip]").first().focus();
  await page.keyboard.press(".");
  await page.keyboard.press(".");
  const lane = await page.locator("#tl-clips").evaluate((el) => el.clientWidth);
  const left = await playhead.evaluate((el) => (el as HTMLElement).style.left);
  expect(parseFloat(left)).toBeCloseTo(((2 * FRAME_US) / COMPOSITION_US) * lane, 1);
});

test("the playhead is clamped to the composition rather than running off it", async ({ page }) => {
  await openTimeline(page);
  await page.locator("[data-tl-clip]").first().focus();
  for (let i = 0; i < 90; i++) await page.keyboard.press(".");
  const left = await page
    .locator("#tl-playhead")
    .evaluate((el) => (el as HTMLElement).style.left);
  const lane = await page.locator("#tl-clips").evaluate((el) => el.clientWidth);
  expect(parseFloat(left)).toBeLessThanOrEqual(lane);
});

test("the playhead snaps to a clip edge the server published, and says so", async ({ page }) => {
  await openTimeline(page);
  await page.locator("[data-tl-clip]").first().focus();
  // 30 frames of 30fps is 1_000_020 us, twenty microseconds past the clip
  // boundary the server published. Half a frame is 16_667 us, so this is
  // inside the threshold and must snap; the frame count is what makes it a
  // test of the threshold rather than of the arithmetic.
  for (let i = 0; i < 30; i++) await page.keyboard.press(".");
  await page.locator("#tl-snap").click();
  await expect(page.locator('[data-tl-notice="snapped"]')).toContainText("0:01.000");
  const left = await page
    .locator("#tl-playhead")
    .evaluate((el) => (el as HTMLElement).style.left);
  const lane = await page.locator("#tl-clips").evaluate((el) => el.clientWidth);
  expect(parseFloat(left)).toBeCloseTo((FIRST_CLIP_END_US / COMPOSITION_US) * lane, 1);
});

// ── 3. MIME ────────────────────────────────────────────────────────────────

test("the vendored ES module is served with a content type the browser executes", async ({ page }) => {
  const responses: Array<{ url: string; type: string; status: number }> = [];
  page.on("response", (r) => {
    if (r.url().includes("/assets/vendor/")) {
      responses.push({
        url: r.url(),
        type: r.headers()["content-type"] || "",
        status: r.status(),
      });
    }
  });
  await openTimeline(page);
  // The import is lazy and fires after first paint, so wait on the fetch
  // rather than sleeping for it.
  await expect.poll(() => responses.length, { timeout: 20_000 }).toBeGreaterThanOrEqual(2);
  for (const r of responses) {
    expect(r.status, r.url).toBe(200);
    expect(r.type, r.url).toMatch(/javascript/);
  }
  expect(responses.some((r) => r.url.endsWith(".mjs")), "the ES module was fetched").toBe(true);
  // Fetched is not evaluated. A module refused for its MIME type rejects the
  // dynamic import, and the surface then says so in a degraded panel — which
  // is exactly the "silently nothing" failure this assertion exists to catch.
  await expect(page.locator("#tl-preview [data-tl-state]")).toHaveCount(0);
});

// ── 4. DEGRADED MODE ───────────────────────────────────────────────────────

test("a browser without AudioDecoder is told audio is render-only, and still draws", async ({ page }) => {
  const result = await capability(page, {
    videoDecoder: true,
    audioDecoder: false,
    videoCodecDecodable: null,
  });
  expect(result.mode).toBe("picture-render-only-audio");
  expect(result.picture).toBe(true);
  expect(result.audibleAudio).toBe(false);
  expect(result.waveform).toBe(true);
  expect(result.reason).toContain("AudioDecoder");
  expect(result.remedy).toContain("render-only");
});

test("a browser without VideoDecoder is told the picture is unavailable and why", async ({ page }) => {
  const result = await capability(page, { videoDecoder: false, audioDecoder: true });
  expect(result.mode).toBe("no-picture");
  expect(result.picture).toBe(false);
  expect(result.waveform).toBe(false);
  expect(result.reason).toContain("VideoDecoder");
  expect(result.remedy).toMatch(/render/i);
});

test("a codec the demuxer refused is a distinct answer from an unprobed one", async ({ page }) => {
  const refused = await capability(page, {
    videoDecoder: true,
    audioDecoder: true,
    videoCodecDecodable: false,
    videoCodec: "hevc",
  });
  expect(refused.mode).toBe("no-picture");
  expect(refused.reason).toContain("hevc");
  // `null` means "not probed yet", which is NOT `false`. A `||` here would
  // report a browser as broken before anything had been asked of it.
  const unprobed = await capability(page, {
    videoDecoder: true,
    audioDecoder: true,
    videoCodecDecodable: null,
  });
  expect(unprobed.mode).toBe("picture-and-audio");
  const probed = await capability(page, {
    videoDecoder: true,
    audioDecoder: true,
    videoCodecDecodable: true,
  });
  expect(probed.mode).toBe("picture-and-audio");
});

test("the surface names the degraded state it is in rather than showing a blank panel", async ({ page }) => {
  await openTimeline(page);
  const panel = page.locator("#tl-preview [data-tl-state]");
  if ((await panel.count()) > 0) {
    // Asserting the panel *exists* is not enough: an empty div passes that.
    await expect(panel.locator("strong")).not.toBeEmpty();
    await expect(panel).toContainText(/render/i);
  }
  // A degraded preview is a degraded *preview*, not a degraded timeline.
  await expect(page.locator("[data-tl-clip]")).toHaveCount(CLIP_COUNT);
});

// ── UNTRUSTED TEXT ─────────────────────────────────────────────────────────

test("a source name is treated as data: markup, a huge name and a bidi override", async ({ page }) => {
  await openTimeline(page);
  const hostile = '"><script>window.__pwned=1</script>\u202E';
  await page.route("**/api/v1/creator/timeline/apply", async (route) => {
    const response = await route.fetch();
    const body = (await response.json()) as {
      plan: { clips: Array<{ span: { source: string } }> };
    };
    const label = "x".repeat(10 * 1024 * 1024);
    for (const clip of body.plan.clips) clip.span.source = hostile;
    body.plan.clips[1].span.source = label;
    await route.fulfill({ response, json: body });
  });
  await page.reload();
  await expect(page.locator("[data-tl-clip]")).toHaveCount(CLIP_COUNT);
  expect(
    await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned),
  ).toBeUndefined();
  await expect(page.locator("[data-tl-clip] script")).toHaveCount(0);
  const title = await page.locator("[data-tl-clip]").nth(1).getAttribute("title");
  expect((title || "").length).toBeLessThan(64);
});
