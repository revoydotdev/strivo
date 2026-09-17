import { test, expect } from "@playwright/test";

// Coverage for the multi-view wall's drag-and-drop, keyboard, and picker
// rework (the "multi-viewer is a bit of a mess; click-to-drag doesn't
// work" pass). Root causes fixed here, each with its own assertion below:
//
//   - `draggable` used to sit on the whole `.ms-leaf`; once a tile plays,
//     a cross-origin iframe covers it and swallows the mousedown before it
//     becomes a dragstart. Drag is now delegated to the stage and started
//     only from the player bar's `.pb-grab[data-drag-handle]` — see
//     "handle-drag swap".
//   - Rail rows were wired per-paint and wiped by every rail repaint
//     (`paintChannelList` rebuilds `#channel-list` wholesale on SSE). Drag
//     is now one delegated `document` listener — see "rail drag survives
//     a repaint".
//   - Composer chips emitted `strivo-rec:`, the stage only understood
//     `strivo-recording:` — composer-to-stage drops were silently
//     dropped. One codec (`encodeDragPayload`/`decodeDragPayload`) now
//     backs every drag source — see "codec round-trip".
//   - Drop was bound per `.ms-leaf`; gutters were dead zones — see "drop
//     on a gutter resolves to the nearest leaf".
//   - Clicking a live rail row on #/watch navigated to #/library instead
//     of loading the channel — see "rail click on #/watch".

const YT_LIVE_ROW = '.ch-row[data-live-stream-id="YouTube:UClive0000000000000000aa"]';
const FINISHED_REC_ID = "11111111-1111-1111-1111-111111111111";

test.describe.configure({ mode: "serial" });
// A wide, tall-enough box so grid-regular presets pack the same way the
// rest of this file already assumed BEFORE aspect-aware repacking
// existed (split-screen side by side, quadrant 2x2) — Playwright's
// default 1280x720 is narrow enough that split-screen's 2x1 no longer
// clears the 70% packing threshold there and repacks to 1x2 (stacked),
// which is CORRECT behaviour but not what the swap/neighbour tests below
// are exercising. Packing itself gets its own explicit viewports.
test.use({ viewport: { width: 1440, height: 900 } });

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("strivo-tour-done", "1");
    localStorage.setItem("strivo:e2e", "1");
  });
});

// Same fake-controller pattern as player-controller.spec.ts: CI has no
// route to Twitch/YouTube, and lifecycle — not vendor rendering — is what
// these tests exercise.
async function installFakePlayers(page: import("@playwright/test").Page) {
  await page.addInitScript(() => {
    (window as any).__fakePlayerFactoryReady = false;
    const install = () => {
      const h = (window as any).__strivoTestHooks;
      if (!h || !h.setPlayerControllerFactory) return false;
      h.setPlayerControllerFactory((kind: string) => {
        const el = document.createElement("div");
        el.className = "watch-tile-iframe ms-iframe fake-player";
        el.dataset.kind = kind;
        return {
          kind,
          root: el,
          mount(c: HTMLElement) {
            if (el.parentElement !== c) c.appendChild(el);
          },
          destroy() {
            el.remove();
          },
          setMuted() {},
          setVolume() {},
          setQuality() {},
          repoint() {},
          isReady() {
            return true;
          },
        };
      });
      (window as any).__fakePlayerFactoryReady = true;
      return true;
    };
    if (!install()) {
      const t = setInterval(() => {
        if (install()) clearInterval(t);
      }, 10);
    }
  });
}
async function waitForFakePlayers(page: import("@playwright/test").Page) {
  await page.waitForFunction(() => (window as any).__fakePlayerFactoryReady === true);
}

async function useSplitScreen(page: import("@playwright/test").Page) {
  await page.locator(".ms-preset-summary").click();
  await page.locator('.ms-preset-opt[data-preset="split-screen"]').click();
}

async function pickLive(page: import("@playwright/test").Page, streamId: string) {
  await page.locator(".ms-picker").first().locator(`.ms-pick[data-pick="live:${streamId}"]`).click();
}

// `locator.dragTo()` reliably drives a native drag from the rail's `<a>`
// rows, but was observed to silently swallow the drag — dragstart never
// fires — from `.pb-grab[data-drag-handle]`, whose visibility is gated by
// `:hover`/`:focus-within` (the bar is opacity:0 until then). A manual
// mouse sequence (move → down → move in steps → up) reproduces the exact
// same native drag reliably, since the real mouse position triggers the
// CSS hover before mousedown — stage-internal drags below use this.
async function manualDragTo(
  page: import("@playwright/test").Page,
  source: import("@playwright/test").Locator,
  target: import("@playwright/test").Locator,
) {
  // `.pb-grab` only reaches opacity:1 on `:hover`/`:focus-within` (the
  // bar is otherwise invisible so a wall of tiles doesn't read as a
  // toolbar farm) — it is NOT pointer-events:none while hidden, but a
  // direct `mouse.move` straight to its coordinates can still land on
  // whatever's rendered above it before the hover transition starts.
  // `locator.hover()` performs Playwright's own actionability-aware
  // hover (scrolling into view, waiting for stability) first, which is
  // what actually reveals it.
  await source.hover();
  const from = await source.boundingBox();
  const to = await target.boundingBox();
  if (!from || !to) throw new Error("manualDragTo: source or target has no box");
  const fromX = from.x + from.width / 2;
  const fromY = from.y + from.height / 2;
  const toX = to.x + to.width / 2;
  const toY = to.y + to.height / 2;
  await page.mouse.move(fromX, fromY);
  await page.mouse.down();
  await page.mouse.move(fromX + (toX - fromX) / 2, fromY + (toY - fromY) / 2, { steps: 5 });
  await page.mouse.move(toX, toY, { steps: 5 });
  await page.mouse.up();
}

// ── Drag-payload codec ──────────────────────────────────────────────────

test("drag-payload codec round-trips and rejects junk", async ({ page }) => {
  await page.goto("/app#/library");

  const result = await page.evaluate(() => {
    const h = (window as any).__strivoTestHooks;
    const tile = h.decodeDragPayload(h.encodeDragPayload({ type: "tile", path: "a.b" }));
    const stream = h.decodeDragPayload(h.encodeDragPayload({ type: "stream", id: "Twitch:foo-1" }));
    const recording = h.decodeDragPayload(h.encodeDragPayload({ type: "recording", id: "abc-123" }));
    const rootTile = h.decodeDragPayload(h.encodeDragPayload({ type: "tile", path: "" }));
    return {
      tile,
      stream,
      recording,
      rootTile,
      junkUrl: h.decodeDragPayload("https://example.com/evil"),
      junkEmpty: h.decodeDragPayload(""),
      junkBadPath: h.decodeDragPayload("strivo-tile:../../etc"),
      junkBadStream: h.decodeDragPayload("strivo-stream:has spaces"),
    };
  });

  expect(result.tile).toEqual({ type: "tile", path: "a.b" });
  expect(result.stream).toEqual({ type: "stream", id: "Twitch:foo-1" });
  expect(result.recording).toEqual({ type: "recording", id: "abc-123" });
  expect(result.rootTile).toEqual({ type: "tile", path: "" });
  expect(result.junkUrl).toBeNull();
  expect(result.junkEmpty).toBeNull();
  expect(result.junkBadPath).toBeNull();
  expect(result.junkBadStream).toBeNull();
});

// ── Rail → stage drag ────────────────────────────────────────────────────

test("dragging a live rail row onto an empty tile assigns it", async ({ page }) => {
  await installFakePlayers(page);
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);

  const row = page.locator(YT_LIVE_ROW);
  await expect(row).toBeVisible();
  const target = page.locator(".ms-leaf.ms-empty");
  await expect(target).toHaveCount(1);

  await row.dragTo(target);

  await expect(page.locator('.ms-leaf[data-stream-id="YouTube:UClive0000000000000000aa"]')).toHaveCount(1);
});

test("rail drag survives a rail repaint", async ({ page }) => {
  await installFakePlayers(page);
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);

  // Force the rail to rebuild its innerHTML wholesale, the way an SSE
  // channel-state event does — this is exactly what wiped the old
  // per-row `draggable` wiring.
  await page.evaluate(() => (window as any).__strivoTestHooks.paintChannelList());
  await page.evaluate(() => (window as any).__strivoTestHooks.paintChannelList());

  const row = page.locator(YT_LIVE_ROW);
  const target = page.locator(".ms-leaf.ms-empty");
  await row.dragTo(target);

  await expect(page.locator('.ms-leaf[data-stream-id="YouTube:UClive0000000000000000aa"]')).toHaveCount(1);
});

// ── Stage-internal drag ──────────────────────────────────────────────────

test("handle-drag swaps two populated tiles", async ({ page }) => {
  await installFakePlayers(page);
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);

  await useSplitScreen(page);
  await pickLive(page, "Twitch:twitch-live-1");
  await pickLive(page, "YouTube:UClive0000000000000000aa");

  const twitchTile = page.locator('.ms-leaf[data-stream-id="Twitch:twitch-live-1"]');
  const ytTile = page.locator('.ms-leaf[data-stream-id="YouTube:UClive0000000000000000aa"]');
  const twitchPathBefore = await twitchTile.getAttribute("data-path");
  const ytPathBefore = await ytTile.getAttribute("data-path");
  expect(twitchPathBefore).not.toBe(ytPathBefore);

  await manualDragTo(page, twitchTile.locator("[data-drag-handle]"), ytTile);

  // Same two content keys, now on the OTHER path each.
  await expect(page.locator(`.ms-leaf[data-path="${ytPathBefore}"][data-stream-id="Twitch:twitch-live-1"]`)).toHaveCount(1);
  await expect(page.locator(`.ms-leaf[data-path="${twitchPathBefore}"][data-stream-id="YouTube:UClive0000000000000000aa"]`)).toHaveCount(1);
});

test("dropping on a gutter resolves to the nearest leaf", async ({ page }) => {
  await installFakePlayers(page);
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);

  await useSplitScreen(page);
  await pickLive(page, "Twitch:twitch-live-1");
  // Leave the second slot empty so the drop has an unambiguous target.

  const gutter = page.locator(".ms-gutter");
  await expect(gutter).toHaveCount(1);
  const emptyLeaf = page.locator(".ms-leaf.ms-empty");

  await manualDragTo(page, page.locator('.ms-leaf[data-stream-id="Twitch:twitch-live-1"] [data-drag-handle]'), gutter);

  // The gutter itself is never a valid content target — the drop must
  // have resolved to a real leaf (either it landed on the empty one via
  // nearest-leaf resolution, or nothing moved because it resolved back to
  // its own tile). Either way, no stream keys are left dangling: exactly
  // one populated tile still exists with the Twitch content.
  await expect(page.locator('.ms-leaf[data-stream-id="Twitch:twitch-live-1"]')).toHaveCount(1);
});

test(".ms-stage carries is-dnd only while a drag is in flight", async ({ page }) => {
  await installFakePlayers(page);
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);

  await useSplitScreen(page);
  await pickLive(page, "Twitch:twitch-live-1");

  await expect(page.locator(".ms-stage")).not.toHaveClass(/is-dnd/);

  await page.evaluate(() => {
    const handle = document.querySelector("[data-drag-handle]") as HTMLElement;
    const dt = new DataTransfer();
    handle.dispatchEvent(new DragEvent("dragstart", { bubbles: true, cancelable: true, dataTransfer: dt }));
  });
  await expect(page.locator(".ms-stage")).toHaveClass(/is-dnd/);

  await page.evaluate(() => {
    document.dispatchEvent(new DragEvent("dragend", { bubbles: true, cancelable: true }));
  });
  await expect(page.locator(".ms-stage")).not.toHaveClass(/is-dnd/);
});

// ── Picker card ────────────────────────────────────────────────────────

test("picker assigns a live channel and a recording", async ({ page }) => {
  await installFakePlayers(page);
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);

  await useSplitScreen(page);
  await expect(page.locator(".ms-picker")).toHaveCount(2);

  await pickLive(page, "Twitch:twitch-live-1");
  await expect(page.locator('.ms-leaf[data-stream-id="Twitch:twitch-live-1"]')).toHaveCount(1);

  await page.locator(".ms-picker").first().locator(`.ms-pick[data-pick="rec:${FINISHED_REC_ID}"]`).click();
  await expect(page.locator(`.ms-leaf[data-recording-id="${FINISHED_REC_ID}"]`)).toHaveCount(1);
  await expect(page.locator(".ms-picker")).toHaveCount(0);
});

test("picker filter narrows rows and supports arrow-key + Enter selection", async ({ page }) => {
  await page.goto("/app#/watch");

  const picker = page.locator(".ms-picker").first();
  const filter = picker.locator(".ms-picker-filter");
  await filter.fill("twitchlive");
  await expect(picker.locator(".ms-pick:not([hidden])")).toHaveCount(1);

  await filter.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(page.locator('.ms-leaf[data-stream-id="Twitch:twitch-live-1"]')).toHaveCount(1);
});

// ── Keyboard ──────────────────────────────────────────────────────────

test("x removes a populated tile back to empty", async ({ page }) => {
  await installFakePlayers(page);
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);

  await pickLive(page, "Twitch:twitch-live-1");
  await expect(page.locator('.ms-leaf[data-stream-id="Twitch:twitch-live-1"]')).toHaveCount(1);

  // The volume slider is the one focusable descendant of a populated tile
  // today, ahead of Lane A's tabindex on `.ms-leaf` itself.
  await page.locator(".ms-vol").focus();
  await page.keyboard.press("x");

  await expect(page.locator(".ms-leaf.ms-empty")).toHaveCount(1);
});

test("Shift+ArrowRight swaps the focused tile with its neighbour", async ({ page }) => {
  await installFakePlayers(page);
  // Short enough that split-screen packs side by side (2x1) rather than
  // stacked (1x2, the file's default 1440x900 box's own better packing)
  // — this test is specifically about a HORIZONTAL neighbour.
  await page.setViewportSize({ width: 1440, height: 680 });
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);

  await useSplitScreen(page);
  await pickLive(page, "Twitch:twitch-live-1");
  await pickLive(page, "YouTube:UClive0000000000000000aa");

  const twitchTile = page.locator('.ms-leaf[data-stream-id="Twitch:twitch-live-1"]');
  const twitchPathBefore = await twitchTile.getAttribute("data-path");

  await twitchTile.locator(".ms-vol").focus();
  await page.keyboard.press("Shift+ArrowRight");

  await expect(page.locator(`.ms-leaf[data-path="${twitchPathBefore}"][data-stream-id="YouTube:UClive0000000000000000aa"]`)).toHaveCount(1);
});

// ── Persistence ──────────────────────────────────────────────────────

test("soloPath survives a reload", async ({ page }) => {
  await installFakePlayers(page);
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);

  await useSplitScreen(page);
  await pickLive(page, "Twitch:twitch-live-1");
  await pickLive(page, "YouTube:UClive0000000000000000aa");

  const twitchPath = await page
    .locator('.ms-leaf[data-stream-id="Twitch:twitch-live-1"]')
    .getAttribute("data-path");
  await page.locator('.ms-leaf[data-stream-id="Twitch:twitch-live-1"] .ms-solo').click();

  const soloBefore = await page.evaluate(() => (window as any).__strivoTestHooks.playerState.soloPath);
  expect(soloBefore).toBe(twitchPath);

  await installFakePlayers(page);
  await page.reload();
  await waitForFakePlayers(page);

  const soloAfter = await page.evaluate(() => (window as any).__strivoTestHooks.playerState.soloPath);
  expect(soloAfter).toBe(twitchPath);
});

// ── Rail click on #/watch ──────────────────────────────────────────────

test("rail click on #/watch fills the focused tile; Ctrl-click still goes to #/library", async ({ page }) => {
  await installFakePlayers(page);
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);

  await expect(page.locator(".ms-leaf.ms-empty")).toHaveCount(1);
  await page.locator(YT_LIVE_ROW).click();

  await expect(page).toHaveURL(/#\/watch/);
  await expect(page.locator('.ms-leaf[data-stream-id="YouTube:UClive0000000000000000aa"]')).toHaveCount(1);

  // Reset back to a fresh empty single tile, then prove Ctrl-click keeps
  // the old "go to channel" behaviour.
  await page.evaluate(() => localStorage.removeItem("strivo-player-layout"));
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);
  await page.locator(YT_LIVE_ROW).click({ modifiers: ["Control"] });
  await expect(page).toHaveURL(/#\/library/);
});

// ── Round 2: aspect-aware repacking, offline slot, gutter ratio ─────────

test("packing prefers side-by-side for 2 tiles on a wide-short stage, 2x2 stays on a squarer one", async ({ page }) => {
  await page.goto("/app#/library");
  const result = await page.evaluate(() => {
    const h = (window as any).__strivoTestHooks;
    return {
      wideShort: h.bestPackedGridShape(2, 1000, 400, { cols: 2, rows: 1 }),
      squareQuadrant: h.bestPackedGridShape(4, 1000, 700, { cols: 2, rows: 2 }),
    };
  });
  expect(result.wideShort).toEqual({ cols: 2, rows: 1 });
  expect(result.squareQuadrant).toEqual({ cols: 2, rows: 2 });
});

test("packing actually reshapes the rendered tree for a short stage", async ({ page }) => {
  await installFakePlayers(page);
  // Narrow and tall — split-screen's 2x1 wastes over 30% of this box, so
  // it should repack to a vertical 1x2 stack instead. Neither slot is
  // populated here, so the chat rail stays hidden entirely
  // (reconcilePlayerChatRail, 018d-pvr.js) and the stage gets the full
  // viewport width back — this spec's previous 1440x900/1440x680 pair
  // relied on a 32px collapsed-rail gutter that no longer reserves space
  // once there's no live, chat-capable tile to show a rail for, and sat
  // close enough to the repack threshold that the extra width silently
  // flipped both outcomes. These dimensions keep a wide margin on both
  // sides of the threshold instead of hugging it.
  await page.setViewportSize({ width: 700, height: 1300 });
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);
  await useSplitScreen(page);
  const dir = await page.evaluate(() => (window as any).__strivoTestHooks.playerState.layout.dir);
  expect(dir).toBe("v");

  // And back to side-by-side on a wide, short stage where 2x1 wins.
  await page.setViewportSize({ width: 1440, height: 500 });
  await page.locator(".ms-preset-summary").click();
  await page.locator('.ms-preset-opt[data-preset="split-screen"]').click();
  const dir2 = await page.evaluate(() => (window as any).__strivoTestHooks.playerState.layout.dir);
  expect(dir2).toBe("h");
});

test("offline slot pill opens the picker card", async ({ page }) => {
  await installFakePlayers(page);
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);
  await pickLive(page, "Twitch:twitch-live-1");
  await expect(page.locator('.ms-leaf[data-stream-id="Twitch:twitch-live-1"]')).toHaveCount(1);

  // The stream leaves the live set (offline, or dropped off the 30s
  // poll) — repaint with no live streams at all, the same way that poll
  // does (tryPatchPlayerStage's own fast path, not a forced full
  // repaint), so renderPopulatedSlotHtml can't resolve it and falls onto
  // the "Stream offline" pill. streamId itself never changes when a
  // channel goes offline, only whether `streams` resolves it — the patch
  // path's diff has to compare against what's actually painted, not just
  // node identity, or this silently leaves the stale tile in place.
  await page.evaluate(() => {
    const h = (window as any).__strivoTestHooks;
    const watch = document.getElementById("watch");
    h.paintPlayerStage(watch.querySelector(".watch-content"), []);
  });
  const pill = page.locator(".ms-empty-pill");
  await expect(pill).toBeVisible();
  await pill.click();

  await expect(page.locator(".ms-picker")).toHaveCount(1);
});

test("Ctrl+Shift+ArrowRight nudges the split ratio and it survives reload", async ({ page }) => {
  await installFakePlayers(page);
  // Short enough that split-screen packs side by side — Ctrl+Shift+Right
  // only acts on an "h" split.
  await page.setViewportSize({ width: 1440, height: 680 });
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);
  await useSplitScreen(page);
  await pickLive(page, "Twitch:twitch-live-1");

  await page.locator('.ms-leaf[data-stream-id="Twitch:twitch-live-1"] .ms-vol').focus();
  await page.keyboard.press("Control+Shift+ArrowRight");

  const ratioAfter = await page.evaluate(() => (window as any).__strivoTestHooks.playerState.layout.ratio);
  expect(ratioAfter).toBeCloseTo(0.55, 5);

  await installFakePlayers(page);
  await page.reload();
  await waitForFakePlayers(page);
  // The hook is installed while the module is loading, before renderWatch()
  // restores localStorage into playerState. Wait for the restored layout,
  // not merely for the factory hook, before reading its split ratio.
  await page.waitForFunction(() => (window as any).__strivoTestHooks.playerState.layout != null);
  const ratioReloaded = await page.evaluate(() => (window as any).__strivoTestHooks.playerState.layout.ratio);
  expect(ratioReloaded).toBeCloseTo(0.55, 5);
});

test("the stage sits directly under the toolbar, no dead band above it", async ({ page }) => {
  await installFakePlayers(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);
  await useSplitScreen(page);

  const gap = await page.evaluate(() => {
    const toolbar = document.querySelector(".watch-toolbar").getBoundingClientRect();
    const stage = document.querySelector(".ms-stage").getBoundingClientRect();
    return stage.top - toolbar.bottom;
  });
  // The old bug was ~200px of dead space split above AND below an
  // aspect-locked stage; this only allows the small intentional flex
  // gap between the toolbar and the stage, never a centering band.
  expect(gap).toBeLessThanOrEqual(16);
});
