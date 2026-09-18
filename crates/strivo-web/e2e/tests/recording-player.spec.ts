import { test, expect } from "@playwright/test";

// Dedicated recording player route (#/play?recording=<id>, 019b-pvr.js).
// Every recording-open path used to land on #/watch (the live multiview
// wall) with a chat rail, composer and preset toolbar along for no
// reason; this route is the focused alternative — see
// watch-in-progress-recording.spec.ts for the legacy-redirect coverage
// and player-bar.spec.ts for the #/watch chat-rail-visibility coverage.

const FINISHED_ID = "11111111-1111-1111-1111-111111111111";
const IN_PROGRESS_ID = "22222222-2222-2222-2222-222222222222";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("strivo-tour-done", "1");
    localStorage.setItem("strivo:e2e", "1");
  });
});

test("a finished recording renders a real player with none of the wall's chrome", async ({ page }) => {
  await page.goto(`/app#/play?recording=${FINISHED_ID}`);
  await expect(page.locator("#play")).toBeVisible();
  await expect(page.locator("video.ms-video")).toHaveCount(1);
  await expect(page.locator(".player-bar")).toHaveCount(1);
  await expect(page.locator("#player-chat-rail")).toHaveCount(0);
  await expect(page.locator("#ms-compose-open")).toHaveCount(0);
  await expect(page.locator(".watch-toolbar")).toHaveCount(0);
});

// Defect 1 — #/play must not inherit the multiview wall's silent-by-default
// tile rule. A recording with no saved per-content volume should come up
// audible here (remembered recording-player volume, default 1.0); the wall
// itself (#/watch) keeps every tile silent until raised.
test("#/play starts a recording audible, not muted at 0", async ({ page }) => {
  await page.goto(`/app#/play?recording=${FINISHED_ID}`);
  const video = page.locator("video.ms-video");
  await expect(video).toHaveCount(1);
  await expect.poll(() => video.evaluate((el: HTMLVideoElement) => el.muted)).toBe(false);
  await expect
    .poll(() => video.evaluate((el: HTMLVideoElement) => el.volume))
    .toBeGreaterThan(0);
});

test("a wall tile for the same recording still starts silent (#/watch default unchanged)", async ({ page }) => {
  await page.addInitScript((recordingId) => {
    localStorage.setItem("strivo-tour-done", "1");
    localStorage.setItem(
      "strivo-player-layout",
      JSON.stringify({ kind: "slot", streamId: null, recordingId }),
    );
    localStorage.setItem("strivo-player-preset", "single");
  }, FINISHED_ID);
  await page.goto("/app#/watch");
  const video = page.locator("video.ms-video");
  await expect(video).toHaveCount(1);
  await expect.poll(() => video.evaluate((el: HTMLVideoElement) => el.muted)).toBe(true);
});

// Defect 2 — a recording whose metadata loads but never actually decodes a
// frame (black screen, dead audio, no `error` event) used to sit there
// forever with no feedback. A genuine decode stall isn't reproducible on
// demand in headless Chromium (there's no "valid header, zero decodable
// frames" fixture that behaves the same across engines/CI), so — like the
// existing performance-media.spec.ts controller tests — this drives the
// REAL makeRecordingController (via __strivoTestHooks) directly rather than
// through page navigation, and freezes the real <video> element's
// `currentTime`/`paused`/`ended`/`seeking` getters so the 8s stall watchdog
// sees exactly the "loaded but never advancing" condition it exists to
// catch. `page.clock` fast-forwards past the 8s wait instead of a real
// wall-clock sleep.
test("a recording that loads metadata but never advances shows the stall overlay", async ({ page }) => {
  await page.clock.install();
  await page.addInitScript(() => {
    localStorage.setItem("strivo-tour-done", "1");
    localStorage.setItem("strivo:e2e", "1");
  });
  await page.goto("/app#/library");
  await page.evaluate(() => {
    const hook = (window as any).__strivoTestHooks;
    const host = document.createElement("div");
    host.id = "stall-recording-test";
    document.body.appendChild(host);
    const controller = hook.makeRecordingController({
      playing: true,
      muted: true,
      videoCodec: "av1",
      audioCodec: "opus",
    });
    controller.mount(host);
    const video = host.querySelector("video") as HTMLVideoElement;
    Object.defineProperty(video, "currentTime", { configurable: true, get: () => 0 });
    Object.defineProperty(video, "paused", { configurable: true, get: () => false });
    Object.defineProperty(video, "ended", { configurable: true, get: () => false });
    Object.defineProperty(video, "seeking", { configurable: true, get: () => false });
    video.dispatchEvent(new Event("loadedmetadata"));
    video.dispatchEvent(new Event("playing"));
    (window as any).__stallController = controller;
  });
  await expect(page.locator("#stall-recording-test .ms-media-error")).toHaveCount(0);
  await page.clock.fastForward(8100);
  await expect(page.locator("#stall-recording-test .ms-media-error")).toBeVisible();
  await expect(page.locator("#stall-recording-test .ms-media-error")).toContainText(/never started decoding/i);
  // Codec info (C3) is surfaced when known, with a support verdict per codec.
  await expect(page.locator("#stall-recording-test .ms-media-error")).toContainText(/AV1 video \/ Opus audio/i);
  await expect(page.locator("#stall-recording-test .ms-media-error-retry")).toBeVisible();
});

// A healthy, paused recording must never trip the stall watchdog — pause
// disarms it (018a-pvr.js), so sitting on a loaded frame for longer than
// the 8s window is normal, not broken.
test("a healthy paused recording shows no stall overlay after 8+ seconds", async ({ page }) => {
  await page.clock.install();
  await page.addInitScript(() => {
    localStorage.setItem("strivo-tour-done", "1");
    localStorage.setItem("strivo:e2e", "1");
  });
  await page.goto("/app#/library");
  await page.evaluate(() => {
    const hook = (window as any).__strivoTestHooks;
    const host = document.createElement("div");
    host.id = "healthy-recording-test";
    document.body.appendChild(host);
    const controller = hook.makeRecordingController({ playing: false, muted: true });
    controller.mount(host);
    const video = host.querySelector("video") as HTMLVideoElement;
    // Metadata loads, then the tile is paused — a normal "loaded, waiting
    // for the viewer to press play" state.
    video.dispatchEvent(new Event("loadedmetadata"));
  });
  await page.clock.fastForward(9000);
  await expect(page.locator("#healthy-recording-test .ms-media-error")).toHaveCount(0);
});

test("an in-progress recording shows the still-recording affordance, not a player", async ({ page }) => {
  await page.goto(`/app#/play?recording=${IN_PROGRESS_ID}`);
  await expect(page.locator(".ms-empty-pill")).toHaveText(/still recording/i);
  await expect(page.locator("video")).toHaveCount(0);
});

test("leaving the route destroys the controller", async ({ page }) => {
  await page.goto(`/app#/play?recording=${FINISHED_ID}`);
  await expect(page.locator("video.ms-video")).toHaveCount(1);
  const before = await page.evaluate(() => (window as any).__strivoTestHooks.playerState.controllers.size);
  expect(before).toBeGreaterThan(0);

  await page.evaluate(() => { window.location.hash = "#/library"; });
  await page.waitForFunction(() => window.location.hash.startsWith("#/library"));
  const after = await page.evaluate(() => (window as any).__strivoTestHooks.playerState.controllers.size);
  expect(after).toBe(0);
  await expect(page.locator("video.ms-video")).toHaveCount(0);
});

// The mock server 404s /api/v1/recordings/:id/download (no fixture video
// content), so a real <video> never reaches a "ready" readyState — a fake
// controller stands in here, the same way player-bar.spec.ts does, so the
// `?t=` seek path can be asserted without needing real playable media.
async function installFakeRecordingPlayer(page: import("@playwright/test").Page) {
  await page.addInitScript(() => {
    (window as any).__fakeSeeks = [];
    (window as any).__fakePlayerFactoryReady = false;
    const install = () => {
      const h = (window as any).__strivoTestHooks;
      if (!h || !h.setPlayerControllerFactory) return false;
      h.setPlayerControllerFactory((kind: string) => {
        const el = document.createElement("div");
        el.className = "watch-tile-iframe ms-video fake-player";
        let currentTime = 0;
        // Starts NOT ready, like a real <video> whose metadata hasn't
        // loaded yet — flips async, on a later tick, exactly once. This
        // matters for the seek-on-ready caller (019b-pvr.js): it
        // subscribes with `const unsub = ctl.onState(cb)` and calls
        // `unsub()` from inside `cb` once `s.ready` is true. A fake that
        // is ready from its very first (synchronous, immediate) `onState`
        // callback invocation would call `unsub()` before the `const`
        // assignment finishes, and — via this fake's own `seek()` calling
        // `notify()` synchronously — recurse into `cb` again before ever
        // reaching that `unsub()` call, forever. Deferring readiness
        // sidesteps the whole reentrancy trap the same way real, genuinely
        // asynchronous media events do.
        let ready = false;
        setTimeout(() => { ready = true; notify(); }, 0);
        const subs = new Set<(s: any) => void>();
        const state = () => ({
          ready, playing: true, buffering: false, ended: false, muted: false,
          volume: 1, currentTime, duration: 120, rate: 1, quality: null, qualities: [],
          live: false, error: null,
        });
        function notify() { subs.forEach((fn) => fn(state())); }
        return {
          kind,
          root: el,
          capabilities: {
            play: true, pause: true, seek: true, duration: true, volume: true,
            mute: true, quality: false, rate: true, pip: true, fullscreen: true,
            live: false, audioOnly: false,
          },
          mount(c: HTMLElement) { if (el.parentElement !== c) c.appendChild(el); },
          destroy() { el.remove(); },
          setMuted() {},
          setVolume() {},
          setQuality() {},
          repoint() {},
          isReady() { return ready; },
          play() {},
          pause() {},
          togglePlay() {},
          seek(sec: number) { (window as any).__fakeSeeks.push(sec); currentTime = sec; },
          currentTime() { return currentTime; },
          duration() { return 120; },
          setRate() {},
          state,
          onState(fn: (s: any) => void) {
            subs.add(fn);
            fn(state());
            return () => subs.delete(fn);
          },
        };
      });
      (window as any).__fakePlayerFactoryReady = true;
      return true;
    };
    if (!install()) {
      const t = setInterval(() => { if (install()) clearInterval(t); }, 10);
    }
  });
}

test("?t= seeks once the controller reports ready", async ({ page }) => {
  await installFakeRecordingPlayer(page);
  await page.goto(`/app#/play?recording=${FINISHED_ID}&t=42`);
  await page.waitForFunction(() => (window as any).__fakePlayerFactoryReady === true);
  await expect(page.locator(".fake-player")).toHaveCount(1);
  await expect.poll(() => page.evaluate(() => (window as any).__fakeSeeks)).toEqual([42]);
});

test("fullscreen centers the recording video at an ultrawide viewport", async ({ page }) => {
  await page.setViewportSize({ width: 3440, height: 1440 });
  await page.addInitScript(() => {
    (Element.prototype as any).requestFullscreen = function () {
      Object.defineProperty(document, "fullscreenElement", { configurable: true, get: () => this });
      this.dispatchEvent(new Event("fullscreenchange", { bubbles: true }));
      return Promise.resolve();
    };
  });
  await page.goto(`/app#/play?recording=${FINISHED_ID}`);
  const leaf = page.locator(".ms-leaf-rec");
  await expect(leaf).toBeVisible();
  await leaf.evaluate((el) => (el as HTMLElement).requestFullscreen());

  const geo = await page.evaluate(() => {
    // .ms-media (not .ms-leaf-rec) is the reference box: it's absolutely
    // positioned with inset:0 inside the leaf, so it's the leaf's CONTENT
    // box (leaf minus its own 1px border) — the same box .ms-mount and the
    // video itself fill via the same inset:0/flex chain, so this comparison
    // isn't skewed by the leaf's border width.
    const mediaEl = document.querySelector(".ms-leaf-rec .ms-media")!;
    const videoEl = document.querySelector("video.ms-video")!;
    const mediaRect = mediaEl.getBoundingClientRect();
    const videoRect = videoEl.getBoundingClientRect();
    return {
      mediaWidth: mediaRect.width,
      videoWidth: videoRect.width,
      videoLeft: videoRect.left - mediaRect.left,
      objectFit: getComputedStyle(videoEl).objectFit,
    };
  });
  expect(geo.objectFit).toBe("contain");
  expect(Math.abs(geo.videoWidth - geo.mediaWidth)).toBeLessThanOrEqual(1);
  expect(Math.abs(geo.videoLeft)).toBeLessThanOrEqual(1);
});
