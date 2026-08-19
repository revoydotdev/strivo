import { test, expect } from "@playwright/test";

// Coverage for the player/embed layer of the multi-view wall.
//
// This file exists because there was previously NO e2e coverage of a live
// tile at all — the only player test in smoke.spec.ts drives a recording
// <video>. The bugs this area actually has (embed URLs that break on a LAN
// address, and repaints that reload streams the user never touched) were
// therefore invisible to CI.

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("strivo-tour-done", "1");
    // spa.js is a module; its internals are only reachable through the
    // opt-in test-hook surface.
    localStorage.setItem("strivo:e2e", "1");
  });
});

// ── Embed URL derivation ────────────────────────────────────────────────
//
// Twitch's `parent=` takes a hostname only: a scheme or port produces
// "embed misconfigured", and a bare IPv4 is rejected outright, which is why
// a LAN address has to be rewritten to the matching nip.io hostname. Three
// call sites used to derive this independently and disagreed — the channel
// detail preview used a bare `location.hostname`, so reaching strivo over a
// LAN IP produced an embed Twitch refuses to load.
test("embed parent host matches the Rust builder for every host shape", async ({ page }) => {
  await page.goto("/app#/library");

  const results = await page.evaluate(() => {
    const h = (window as any).__strivoTestHooks;
    return [
      "127.0.0.1:8181",
      "192.168.1.50:8181",
      "homepc.tail74e6d3.ts.net:8443",
      "https://example.com/app",
      "localhost:8181",
    ].map((host) => [host, h.embedParentHost(host)]);
  });

  expect(Object.fromEntries(results)).toEqual({
    "127.0.0.1:8181": "127-0-0-1.nip.io",
    "192.168.1.50:8181": "192-168-1-50.nip.io",
    "homepc.tail74e6d3.ts.net:8443": "homepc.tail74e6d3.ts.net",
    "https://example.com/app": "example.com",
    "localhost:8181": "localhost",
  });
});

test("buildEmbedUrl omits playback params when the caller manages them", async ({ page }) => {
  await page.goto("/app#/library");

  const urls = await page.evaluate(() => {
    const h = (window as any).__strivoTestHooks;
    return {
      withPlayback: h.buildEmbedUrl("Twitch", "cohh", { host: "192.168.1.50:8181", muted: true, autoplay: true }),
      bare: h.buildEmbedUrl("Twitch", "cohh", { host: "127.0.0.1:8181" }),
      youtube: h.buildEmbedUrl("YouTube", "UCabc", { muted: true, autoplay: false }),
    };
  });

  expect(urls.withPlayback).toBe(
    "https://player.twitch.tv/?channel=cohh&parent=192-168-1-50.nip.io&muted=true&autoplay=true",
  );
  // A caller driving playback through a player API must not bake a
  // conflicting state into the src, so undefined options emit nothing.
  expect(urls.bare).toBe("https://player.twitch.tv/?channel=cohh&parent=127-0-0-1.nip.io");
  expect(urls.bare).not.toContain("autoplay");
  expect(urls.bare).not.toContain("muted");

  expect(urls.youtube).toContain("mute=1");
  expect(urls.youtube).toContain("autoplay=0");
});

// ── Mute policy ─────────────────────────────────────────────────────────
//
// Mute-all is the default and one soloed path is the single audible tile.
// This was computed inline in three places, which is how they could drift.
test("computeMuted implements mute-all with a single audible solo", async ({ page }) => {
  await page.goto("/app#/watch");

  const verdicts = await page.evaluate(() => {
    const h = (window as any).__strivoTestHooks;
    const prev = h.playerState.soloPath;
    h.playerState.soloPath = "";
    const allMuted = [h.computeMuted(""), h.computeMuted("a"), h.computeMuted("b.a")];
    h.playerState.soloPath = "a";
    const soloed = [h.computeMuted("a"), h.computeMuted("b.a"), h.computeMuted("")];
    h.playerState.soloPath = prev;
    return { allMuted, soloed };
  });

  // No solo → everything muted, including the root path "".
  expect(verdicts.allMuted).toEqual([true, true, true]);
  // Solo "a" → only "a" is audible.
  expect(verdicts.soloed).toEqual([false, true, true]);
});


// ── Controller lifecycle ────────────────────────────────────────────────
//
// The bug these pin: stage HTML is rebuilt wholesale on preset switches,
// composer edits and Play-all. While the player lived inside that HTML,
// every rebuild tore it down, so rearranging the wall reloaded streams the
// viewer never touched. Players are now owned by a registry that outlives
// any paint and is keyed on content rather than layout position.
//
// A fake player stands in for the vendor embed: CI has no route to Twitch
// or YouTube, and asserting against real embeds would be flaky. Lifecycle
// is what matters, and the fake records it exactly.
async function installFakePlayers(page: import("@playwright/test").Page) {
  await page.addInitScript(() => {
    (window as any).__fakeLog = { created: [], destroyed: [], mounted: [] };
    const install = () => {
      const h = (window as any).__strivoTestHooks;
      if (!h || !h.setPlayerControllerFactory) return false;
      h.setPlayerControllerFactory((kind: string, spec: any) => {
        const el = document.createElement("div");
        el.className = "watch-tile-iframe ms-iframe fake-player";
        el.dataset.kind = kind;
        (el as any).__id = Math.random().toString(36).slice(2);
        (window as any).__fakeLog.created.push((el as any).__id);
        return {
          kind,
          root: el,
          mount(c: HTMLElement) {
            if (el.parentElement !== c) {
              c.appendChild(el);
              (window as any).__fakeLog.mounted.push((el as any).__id);
            }
          },
          destroy() {
            (window as any).__fakeLog.destroyed.push((el as any).__id);
            el.remove();
          },
          setMuted(m: boolean) { el.dataset.muted = String(m); },
          setVolume() {},
          setQuality() {},
          repoint() {},
          isReady() { return true; },
        };
      });
      return true;
    };
    // The hooks object is created at the end of module evaluation, so
    // retry until the SPA has booted.
    if (!install()) {
      const t = setInterval(() => { if (install()) clearInterval(t); }, 10);
      setTimeout(() => clearInterval(t), 5000);
    }
  });
}

test("starting one tile does not disturb another", async ({ page }) => {
  await installFakePlayers(page);
  await page.goto("/app#/watch");

  // Two tiles, both fed from the mock's live streams.
  await page.locator(".ms-preset-summary").click();
  await page.locator('.ms-preset-opt[data-preset="split-screen"]').click();
  const pickers = page.locator(".ms-slot-pick");
  await expect(pickers).toHaveCount(2);
  // Filling a slot removes its picker, so the remaining one collapses back
  // to index 0 — always take the first.
  await pickers.first().selectOption("live:Twitch:twitch-live-1");
  await expect(pickers).toHaveCount(1);
  await pickers.first().selectOption("live:YouTube:UClive0000000000000000aa");
  await expect(pickers).toHaveCount(0);

  // Wall opens paused: posters, no players.
  await expect(page.locator(".ms-poster")).toHaveCount(2);
  expect(await page.evaluate(() => (window as any).__fakeLog.created.length)).toBe(0);

  // Start the first tile only.
  await page.locator(".ms-play").first().click();
  await expect(page.locator(".fake-player")).toHaveCount(1);
  const firstId = await page.evaluate(() => (window as any).__fakeLog.created[0]);

  // Start the second. The first must survive: same instance, never destroyed.
  await page.locator(".ms-play").first().click();
  await expect(page.locator(".fake-player")).toHaveCount(2);

  const log = await page.evaluate(() => (window as any).__fakeLog);
  expect(log.created).toHaveLength(2);
  expect(log.destroyed).not.toContain(firstId);
});

test("removing a tile destroys exactly its own player", async ({ page }) => {
  await installFakePlayers(page);
  await page.goto("/app#/watch");

  await page.locator(".ms-preset-summary").click();
  await page.locator('.ms-preset-opt[data-preset="split-screen"]').click();
  const pickers = page.locator(".ms-slot-pick");
  await pickers.first().selectOption("live:Twitch:twitch-live-1");
  await expect(pickers).toHaveCount(1);
  await pickers.first().selectOption("live:YouTube:UClive0000000000000000aa");
  await expect(pickers).toHaveCount(0);
  await page.locator(".ms-play").first().click();
  await page.locator(".ms-play").first().click();
  await expect(page.locator(".fake-player")).toHaveCount(2);

  const [keptId] = await page.evaluate(() => (window as any).__fakeLog.created.slice(1));
  await page.locator(".ms-remove").first().click();

  // One player torn down, the other still mounted and never destroyed.
  await expect(page.locator(".fake-player")).toHaveCount(1);
  const log = await page.evaluate(() => (window as any).__fakeLog);
  expect(log.destroyed).toHaveLength(1);
  expect(log.destroyed).not.toContain(keptId);
});

test("leaving the watch route tears every player down", async ({ page }) => {
  await installFakePlayers(page);
  await page.goto("/app#/watch");

  await page.locator(".ms-slot-pick").first().selectOption("live:Twitch:twitch-live-1");
  await page.locator(".ms-play").first().click();
  await expect(page.locator(".fake-player")).toHaveCount(1);

  // Navigating away must close vendor connections, not just drop the DOM.
  await page.goto("/app#/library");
  const log = await page.evaluate(() => (window as any).__fakeLog);
  expect(log.destroyed).toHaveLength(1);
  expect(
    await page.evaluate(() => (window as any).__strivoTestHooks.playerState.controllers.size),
  ).toBe(0);
});

// This is the regression that motivated the whole refactor: every composer
// action called repaintStage(), which forced a full repaint, which rebuilt
// the stage HTML and took every playing tile down with it. Rearranging the
// wall reloaded streams the viewer never touched.
test("composer edits leave an untouched tile's player alone", async ({ page }) => {
  await installFakePlayers(page);
  await page.goto("/app#/watch");

  await page.locator(".ms-preset-summary").click();
  await page.locator('.ms-preset-opt[data-preset="split-screen"]').click();
  const pickers = page.locator(".ms-slot-pick");
  await pickers.first().selectOption("live:Twitch:twitch-live-1");
  await expect(pickers).toHaveCount(1);

  // Start the Twitch tile; the other slot stays empty.
  await page.locator(".ms-play").first().click();
  await expect(page.locator(".fake-player")).toHaveCount(1);
  const playingId = await page.evaluate(() => (window as any).__fakeLog.created[0]);

  // Drive the composer: assign a stream to the OTHER slot.
  await page.locator("#ms-compose-open").click();
  await expect(page.locator("#ms-composer")).toBeVisible();
  await page.locator("#msc-map .msc-cell:not(.is-filled)").first().click();
  await page.locator(".msc-src").first().click();
  await page.locator('[data-action="modal-close"]').click();

  // The tile that was already playing must not have been rebuilt.
  const log = await page.evaluate(() => (window as any).__fakeLog);
  expect(log.destroyed).not.toContain(playingId);
  await expect(page.locator(".fake-player")).toHaveCount(1);
});

test("a preset change preserves a playing tile", async ({ page }) => {
  await installFakePlayers(page);
  await page.goto("/app#/watch");

  await page.locator(".ms-slot-pick").first().selectOption("live:Twitch:twitch-live-1");
  await page.locator(".ms-play").first().click();
  await expect(page.locator(".fake-player")).toHaveCount(1);
  const playingId = await page.evaluate(() => (window as any).__fakeLog.created[0]);

  // Single -> quadrant is a tree-SHAPE change, so it takes the full-repaint
  // path rather than the surgical patch. The stream is carried into the new
  // layout, so its player must be re-parented, not rebuilt.
  await page.locator(".ms-preset-summary").click();
  await page.locator('.ms-preset-opt[data-preset="quadrant"]').click();

  const log = await page.evaluate(() => (window as any).__fakeLog);
  expect(log.destroyed).not.toContain(playingId);
  expect(log.created).toHaveLength(1);
  await expect(page.locator(".fake-player")).toHaveCount(1);
});
