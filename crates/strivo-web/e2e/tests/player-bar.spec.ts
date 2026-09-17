import { test, expect } from "@playwright/test";

// Coverage for the unified player bar (019a-pvr.js) — the ONE StriVo
// control surface over the media box that replaces the old stacked
// .watch-tile-head + whatever the vendor player renders inside its
// iframe. A fake controller stands in for the real Twitch/YouTube/
// recording controllers so these tests assert on the bar's wiring
// against the new capability contract rather than on a live embed.

test.describe.configure({ mode: "serial" });

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("strivo-tour-done", "1");
    localStorage.setItem("strivo:e2e", "1");
  });
});

// The empty-slot picker is a filterable card (`.ms-picker`), not the old
// native `<select class="ms-slot-pick">` — same "live:<id>"/"rec:<id>"
// value grammar via a `.ms-pick[data-pick="<value>"]` row, always the
// FIRST open picker (filling one tile makes its card disappear, so the
// next empty tile's card becomes "first").
async function pickSlot(page: import("@playwright/test").Page, value: string) {
  await page.locator(".ms-picker").first().locator(`.ms-pick[data-pick="${value}"]`).click();
}

async function waitForFakePlayers(page: import("@playwright/test").Page) {
  await page.waitForFunction(() => (window as any).__fakePlayerFactoryReady === true);
}

// Installs a fake controller carrying the FULL new interface (capabilities,
// play/pause/seek/duration/currentTime/setRate/state/onState) plus a call
// log, so tests can assert on exactly what the bar invoked.
async function installFullFakePlayers(page: import("@playwright/test").Page, caps: Record<string, boolean>) {
  await page.addInitScript((capabilities) => {
    (window as any).__fakeCalls = [];
    (window as any).__fakePlayerFactoryReady = false;
    const install = () => {
      const h = (window as any).__strivoTestHooks;
      if (!h || !h.setPlayerControllerFactory) return false;
      h.setPlayerControllerFactory((kind: string) => {
        const el = document.createElement("div");
        el.className = "watch-tile-iframe ms-iframe fake-player";
        el.dataset.kind = kind;
        let muted = false;
        let volume = 1;
        let playing = false;
        let currentTime = 0;
        const duration = 120;
        let rate = 1;
        const subs = new Set<(s: any) => void>();
        const log = (name: string, ...args: any[]) => (window as any).__fakeCalls.push([name, ...args]);
        const state = () => ({
          ready: true, playing, buffering: false, ended: false, muted,
          volume, currentTime, duration: capabilities.duration ? duration : NaN,
          rate, quality: null, qualities: [], live: !!capabilities.live, error: null,
        });
        const notify = () => subs.forEach((fn) => fn(state()));
        return {
          kind,
          root: el,
          capabilities,
          mount(c: HTMLElement) { if (el.parentElement !== c) c.appendChild(el); },
          destroy() { el.remove(); },
          setMuted(m: boolean) { log("setMuted", m); muted = m; notify(); },
          setVolume(v: number) { log("setVolume", v); volume = v; notify(); },
          setQuality() {},
          repoint() {},
          isReady() { return true; },
          play() { log("play"); playing = true; notify(); },
          pause() { log("pause"); playing = false; notify(); },
          togglePlay() { playing ? this.pause() : this.play(); },
          seek(sec: number) { log("seek", sec); currentTime = sec; notify(); },
          currentTime() { return currentTime; },
          duration() { return capabilities.duration ? duration : NaN; },
          setRate(r: number) { log("setRate", r); rate = r; notify(); },
          state,
          onState(fn: (s: any) => void) { subs.add(fn); fn(state()); return () => subs.delete(fn); },
        };
      });
      (window as any).__fakePlayerFactoryReady = true;
      return true;
    };
    if (!install()) {
      const t = setInterval(() => { if (install()) clearInterval(t); }, 10);
    }
  }, caps);
}

const ALL_FALSE_CAPS = {
  play: false, pause: false, seek: false, duration: false, volume: false,
  mute: false, quality: false, rate: false, pip: false, fullscreen: true,
  live: false, audioOnly: false,
};
const FULL_CAPS = {
  play: true, pause: true, seek: true, duration: true, volume: true,
  mute: true, quality: false, rate: false, pip: false, fullscreen: true,
  live: false, audioOnly: false,
};

test("a controller with all-false capabilities renders only remove + fullscreen", async ({ page }) => {
  await installFullFakePlayers(page, ALL_FALSE_CAPS);
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);

  await pickSlot(page, "live:Twitch:twitch-live-1");
  await page.locator(".ms-play").first().click();
  await expect(page.locator(".fake-player")).toHaveCount(1);

  const bar = page.locator(".player-bar").first();
  await expect(bar.locator(".ms-remove")).toHaveCount(1);
  await expect(bar.locator(".ms-fs")).toHaveCount(1);
  await expect(bar.locator(".pb-play")).toHaveCount(0);
  await expect(bar.locator(".ms-vol")).toHaveCount(0);
  await expect(bar.locator(".ms-solo, .ms-unsolo")).toHaveCount(0);
  await expect(bar.locator(".pb-seek")).toHaveCount(0);
});

test("play button calls play(), seek range calls seek() with seconds", async ({ page }) => {
  await installFullFakePlayers(page, FULL_CAPS);
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);

  await pickSlot(page, "live:Twitch:twitch-live-1");
  await page.locator(".ms-play").first().click();
  await expect(page.locator(".fake-player")).toHaveCount(1);

  const bar = page.locator(".player-bar").first();
  await bar.locator(".pb-play").click();
  let calls = await page.evaluate(() => (window as any).__fakeCalls);
  expect(calls.some((c: any[]) => c[0] === "play")).toBe(true);

  await bar.locator(".pb-seek").fill("500"); // 50% of a 120s duration
  calls = await page.evaluate(() => (window as any).__fakeCalls);
  const seekCall = calls.find((c: any[]) => c[0] === "seek");
  expect(seekCall).toBeTruthy();
  expect(seekCall[1]).toBeCloseTo(60, 0);
});

test("'m' on a focused leaf zeroes volume and mutes", async ({ page }) => {
  await installFullFakePlayers(page, FULL_CAPS);
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);

  await pickSlot(page, "live:Twitch:twitch-live-1");
  await page.locator(".ms-play").first().click();
  await expect(page.locator(".fake-player")).toHaveCount(1);

  const leaf = page.locator(".ms-leaf").first();
  await leaf.click({ position: { x: 5, y: 5 } });
  await leaf.focus();
  await leaf.press("m");

  const calls = await page.evaluate(() => (window as any).__fakeCalls);
  expect(calls.some((c: any[]) => c[0] === "setVolume" && c[1] === 0)).toBe(true);
  expect(calls.some((c: any[]) => c[0] === "setMuted" && c[1] === true)).toBe(true);
  const vol = await page.evaluate(() => Object.values((window as any).__strivoTestHooks.playerState.volumes));
  expect(vol).toContain(0);
});

// The legacy e2e fake (player-controller.spec.ts) predates the capability
// contract entirely — no `.capabilities`, no new methods. mountPlayerBar
// must infer volume/mute from which methods it bothers to implement so
// that spec keeps passing untouched.
test("legacy fake controller (no capabilities) still yields a volume slider per live tile", async ({ page }) => {
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
          mount(c: HTMLElement) { if (el.parentElement !== c) c.appendChild(el); },
          destroy() { el.remove(); },
          setMuted(m: boolean) { el.dataset.muted = String(m); },
          setVolume() {},
          setQuality() {},
          repoint() {},
          isReady() { return true; },
        };
      });
      (window as any).__fakePlayerFactoryReady = true;
      return true;
    };
    if (!install()) {
      const t = setInterval(() => { if (install()) clearInterval(t); }, 10);
    }
  });
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);

  await page.locator(".ms-preset-summary").click();
  await page.locator('.ms-preset-opt[data-preset="split-screen"]').click();
  const pickers = page.locator(".ms-picker");
  await pickSlot(page, "live:Twitch:twitch-live-1");
  await expect(pickers).toHaveCount(1);
  await pickSlot(page, "live:YouTube:UClive0000000000000000aa");
  await page.locator("#watch-playall").click();
  await expect(page.locator(".fake-player")).toHaveCount(2);

  await expect(page.locator(".ms-vol")).toHaveCount(2);
});

test("fullscreen button requests fullscreen on the .ms-leaf", async ({ page }) => {
  await page.addInitScript(() => {
    (window as any).__fsCalls = [];
    (Element.prototype as any).requestFullscreen = function () {
      (window as any).__fsCalls.push(this.className);
      return Promise.resolve();
    };
  });
  await installFullFakePlayers(page, FULL_CAPS);
  await page.goto("/app#/watch");
  await waitForFakePlayers(page);

  await pickSlot(page, "live:Twitch:twitch-live-1");
  await page.locator(".ms-play").first().click();
  await expect(page.locator(".fake-player")).toHaveCount(1);

  await page.locator(".player-bar .ms-fs").first().click();
  const calls = await page.evaluate(() => (window as any).__fsCalls);
  expect(calls.length).toBeGreaterThan(0);
  expect(calls[0]).toContain("ms-leaf");
});

test("a recording tile renders video.ms-video without a controls attribute", async ({ page }) => {
  // Old link shape (every recording-open path used to build this) —
  // #/watch redirects it to the dedicated player route (019b-pvr.js)
  // rather than building a wall around one recording.
  await page.goto("/app#/watch?recording=11111111-1111-1111-1111-111111111111&fresh=1");
  await page.waitForFunction(() => /#\/play\?recording=/.test(window.location.hash));
  const v = page.locator("video.ms-video");
  await expect(v).toHaveCount(1);
  await expect(v).not.toHaveAttribute("controls", /.*/);
});

test("#/viewer redirects to #/watch?focus=...", async ({ page }) => {
  await page.goto("/app#/viewer?room=offlinechan");
  await page.waitForFunction(() => /#\/watch\?focus=/.test(window.location.hash));
  expect(page.url()).toMatch(/#\/watch\?focus=/);
});

test("a single tile at 1440x900 does not overflow the page", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
  await page.goto("/app#/watch");
  await page.waitForSelector(".ms-leaf");
  const overflow = await page.evaluate(() => document.documentElement.scrollHeight - document.documentElement.clientHeight);
  expect(overflow).toBeLessThanOrEqual(1);
});

// ── Round 2: left-rail collapse + compact bar ────────────────────────

const R2_STREAMS = ["Twitch:twitch-live-1", "YouTube:UClive0000000000000000aa"];
const r2Slot = (streamId: string) => ({ kind: "slot", streamId, recordingId: null });
const r2Split = (dir: "h" | "v", a: unknown, b: unknown) => ({ kind: "split", dir, ratio: 0.5, a, b });
// Same shape multiview-dnd.spec.ts / ux-budget.spec.ts use to seed a
// layout without needing a real controller — poster tiles are enough to
// measure leaf/bar geometry.
const R2_QUADRANT = r2Split(
  "v",
  r2Split("h", r2Slot(R2_STREAMS[0]), r2Slot(R2_STREAMS[1])),
  r2Split("h", r2Slot(R2_STREAMS[1]), r2Slot(R2_STREAMS[0])),
);

async function openQuadrant(page: import("@playwright/test").Page, { chatRailOpen }: { chatRailOpen: boolean }) {
  await page.addInitScript(
    ({ layout, chatRailOpen }) => {
      localStorage.setItem("strivo-tour-done", "1");
      localStorage.setItem("strivo-player-layout", JSON.stringify(layout));
      // "quadrant" (not "custom") so the aspect-locked stage sizing
      // (paintPlayerStage's stageAspectFor, gated on the named preset)
      // actually kicks in — that lock is what makes a quadrant tile
      // shrink below 300px in the first place; a "custom" layout with
      // the same shape stays unconstrained and never goes compact.
      localStorage.setItem("strivo-player-preset", "quadrant");
      localStorage.setItem("strivo-player-chat-rail-open", chatRailOpen ? "1" : "0");
    },
    { layout: R2_QUADRANT, chatRailOpen },
  );
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/app#/watch");
  await page.waitForSelector(".ms-leaf:not(.ms-empty)");
  // Let the ResizeObserver's first callback (and any layout settling)
  // land before reading compact state.
  await page.waitForTimeout(150);
}

test("quadrant with the chat rail open docks a compact bar below the media, no overlap", async ({ page }) => {
  await openQuadrant(page, { chatRailOpen: true });
  const leaves = page.locator(".ms-leaf:not(.ms-empty)");
  await expect(leaves).toHaveCount(4);
  await expect(page.locator(".ms-leaf.is-compact")).toHaveCount(4);

  const geo = await page.locator(".ms-leaf.is-compact").first().evaluate((leaf) => {
    const media = leaf.querySelector(".ms-media")!.getBoundingClientRect();
    const bar = leaf.querySelector(".player-bar")!.getBoundingClientRect();
    return { mediaBottom: media.bottom, barTop: bar.top, barHeight: bar.height };
  });
  // Docked below, not overlaid: the media box ends at or before the bar
  // begins, and the bar is the compact 32px strip, not the full overlay.
  expect(geo.mediaBottom).toBeLessThanOrEqual(geo.barTop + 1);
  expect(geo.barHeight).toBeLessThanOrEqual(36);
});

test("quadrant with the chat rail collapsed gives tiles room and no compact mode", async ({ page }) => {
  await openQuadrant(page, { chatRailOpen: false });
  const leaves = page.locator(".ms-leaf:not(.ms-empty)");
  await expect(leaves).toHaveCount(4);
  await expect(page.locator(".ms-leaf.is-compact")).toHaveCount(0);

  const heights = await leaves.evaluateAll((els) => els.map((el) => el.getBoundingClientRect().height));
  for (const h of heights) expect(h).toBeGreaterThanOrEqual(300);
});

test("body.route-watch is present on #/watch and cleared off-route", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
  await page.goto("/app#/watch");
  await page.waitForSelector(".ms-leaf");
  await expect(page.locator("body")).toHaveClass(/route-watch/);

  await page.evaluate(() => { window.location.hash = "#/library"; });
  await page.waitForFunction(() => window.location.hash.startsWith("#/library"));
  await expect(page.locator("body")).not.toHaveClass(/route-watch/);
});

test("the rail toggle persists across reload", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
  await page.goto("/app#/watch");
  await page.waitForSelector(".ms-leaf");

  // A genuinely first-ever visit (no stored rail preference at all) now
  // defaults the left rail open, so it's never a dead end on first watch.
  await expect(page.locator("body")).toHaveClass(/watch-rail-open/);
  await page.locator(".watch-rail-toggle").click();
  await expect(page.locator("body")).not.toHaveClass(/watch-rail-open/);
  const stored = await page.evaluate(() => localStorage.getItem("strivo-player-rail-open"));
  expect(stored).toBe("0");

  await page.reload();
  await page.waitForSelector(".ms-leaf");
  await expect(page.locator("body")).not.toHaveClass(/watch-rail-open/);
  await expect(page.locator(".watch-rail-toggle")).toHaveAttribute("aria-pressed", "false");
});

test("fullscreen clears compact mode even on a short tile", async ({ page }) => {
  await page.addInitScript(() => {
    (Element.prototype as any).requestFullscreen = function () {
      Object.defineProperty(document, "fullscreenElement", { configurable: true, get: () => this });
      this.dispatchEvent(new Event("fullscreenchange", { bubbles: true }));
      return Promise.resolve();
    };
  });
  await openQuadrant(page, { chatRailOpen: true });
  await expect(page.locator(".ms-leaf.is-compact")).toHaveCount(4);

  await page.evaluate(() => (document.querySelector(".ms-leaf") as HTMLElement).requestFullscreen());
  await expect(page.locator(".ms-leaf").first()).not.toHaveClass(/is-compact/);
});

// ── Chat rail only for a live, chat-capable tile ─────────────────────
// Only Twitch chat is wired end-to-end (collectChatableStreams,
// 018d-pvr.js), so a recording-only wall — or one with no chatable tile
// at all — must hide the rail rather than reserving a dead gutter.

test("a recording-only wall hides the chat rail entirely", async ({ page }) => {
  await page.addInitScript(
    (recordingId) => {
      localStorage.setItem("strivo-tour-done", "1");
      localStorage.setItem(
        "strivo-player-layout",
        JSON.stringify({ kind: "slot", streamId: null, recordingId }),
      );
      localStorage.setItem("strivo-player-preset", "single");
      localStorage.setItem("strivo-player-chat-rail-open", "1");
    },
    "11111111-1111-1111-1111-111111111111",
  );
  await page.goto("/app#/watch");
  await page.waitForSelector(".ms-leaf-rec");

  await expect(page.locator("#player-chat-rail")).toBeHidden();
  await expect(page.locator("#watch")).toHaveClass(/no-chat-target/);
  await expect(page.locator("#watch")).not.toHaveClass(/has-chat-rail/);
});

test("adding a live Twitch tile to a recording-only wall reveals the chat rail", async ({ page }) => {
  await page.addInitScript(
    ({ recordingId, streamId }) => {
      localStorage.setItem("strivo-tour-done", "1");
      localStorage.setItem(
        "strivo-player-layout",
        JSON.stringify({
          kind: "split",
          dir: "h",
          ratio: 0.5,
          a: { kind: "slot", streamId: null, recordingId },
          b: { kind: "slot", streamId, recordingId: null },
        }),
      );
      localStorage.setItem("strivo-player-preset", "custom");
      localStorage.setItem("strivo-player-chat-rail-open", "1");
    },
    { recordingId: "11111111-1111-1111-1111-111111111111", streamId: "Twitch:twitch-live-1" },
  );
  await page.goto("/app#/watch");
  await page.waitForSelector(".ms-leaf-rec");

  await expect(page.locator("#player-chat-rail")).toBeVisible();
  await expect(page.locator("#watch")).toHaveClass(/has-chat-rail/);
  await expect(page.locator("#watch")).not.toHaveClass(/no-chat-target/);
});
