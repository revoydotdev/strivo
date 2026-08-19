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

// ── Tile audio ──────────────────────────────────────────────────────────
//
// Volume per tile is the source of truth and muted means zero. Before this,
// "which tile is focused" and "which tile is audible" were one flag, so the
// only reachable states were one-audible or all-silent — you could not run a
// main stream loud with a second quietly underneath.
test("each tile carries its own level, and muted means zero", async ({ page }) => {
  await installFakePlayers(page);
  await page.goto("/app#/watch");

  await page.locator(".ms-preset-summary").click();
  await page.locator('.ms-preset-opt[data-preset="split-screen"]').click();
  const pickers = page.locator(".ms-slot-pick");
  await pickers.first().selectOption("live:Twitch:twitch-live-1");
  await expect(pickers).toHaveCount(1);
  await pickers.first().selectOption("live:YouTube:UClive0000000000000000aa");
  await page.locator("#watch-playall").click();
  await expect(page.locator(".fake-player")).toHaveCount(2);

  // A wall opens silent — starting one that makes noise is hostile.
  const sliders = page.locator(".ms-vol");
  await expect(sliders).toHaveCount(2);
  expect(await sliders.first().inputValue()).toBe("0");

  // Two tiles at genuinely different levels is the whole point.
  await sliders.nth(0).fill("80");
  await sliders.nth(1).fill("25");

  const vols = await page.evaluate(() => (window as any).__strivoTestHooks.playerState.volumes);
  const levels = Object.values(vols).sort();
  expect(levels).toEqual([0.25, 0.8]);
});

test("solo raises one tile and silences the rest", async ({ page }) => {
  await installFakePlayers(page);
  await page.goto("/app#/watch");

  await page.locator(".ms-preset-summary").click();
  await page.locator('.ms-preset-opt[data-preset="split-screen"]').click();
  const pickers = page.locator(".ms-slot-pick");
  await pickers.first().selectOption("live:Twitch:twitch-live-1");
  await expect(pickers).toHaveCount(1);
  await pickers.first().selectOption("live:YouTube:UClive0000000000000000aa");
  await page.locator("#watch-playall").click();

  // Bring both up, then solo the first.
  const sliders = page.locator(".ms-vol");
  await sliders.nth(0).fill("40");
  await sliders.nth(1).fill("60");
  await page.locator(".ms-solo").first().click();

  const vols = await page.evaluate(() => (window as any).__strivoTestHooks.playerState.volumes);
  expect(Object.values(vols).sort()).toEqual([0, 1]);

  // Muted is derived, never stored separately.
  const muted = await page.evaluate(() => {
    const h = (window as any).__strivoTestHooks;
    return [h.computeMuted(""), h.computeMuted("a"), h.computeMuted("b")];
  });
  expect(muted.filter(Boolean).length).toBeGreaterThan(0);
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

// Play-all deliberately forces a full repaint (every tile's playing state
// changes at once). The tile already running must ride through it.
test("play-all starts the rest without rebuilding what is already playing", async ({ page }) => {
  await installFakePlayers(page);
  await page.goto("/app#/watch");

  await page.locator(".ms-preset-summary").click();
  await page.locator('.ms-preset-opt[data-preset="split-screen"]').click();
  const pickers = page.locator(".ms-slot-pick");
  await pickers.first().selectOption("live:Twitch:twitch-live-1");
  await expect(pickers).toHaveCount(1);
  await pickers.first().selectOption("live:YouTube:UClive0000000000000000aa");

  // Start exactly one tile.
  await page.locator(".ms-play").first().click();
  await expect(page.locator(".fake-player")).toHaveCount(1);
  const firstId = await page.evaluate(() => (window as any).__fakeLog.created[0]);

  await page.locator("#watch-playall").click();

  // Second tile joins; the first is neither destroyed nor recreated.
  await expect(page.locator(".fake-player")).toHaveCount(2);
  const log = await page.evaluate(() => (window as any).__fakeLog);
  expect(log.destroyed).not.toContain(firstId);
  expect(log.created).toHaveLength(2);
});

// ── Vendor SDK ──────────────────────────────────────────────────────────
//
// The safety property that matters most: if Twitch's script is blocked,
// offline, or simply slow to fail, the tile must still play. A missing SDK
// degrades to the plain iframe of before, never to an empty tile.
test("a blocked Twitch SDK degrades to a working iframe", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
  // Deterministic: never reach the real CDN from a test.
  await page.route("**/player.twitch.tv/js/embed/v1.js", (r) => r.abort());
  await page.goto("/app#/watch");

  await page.locator(".ms-slot-pick").first().selectOption("live:Twitch:twitch-live-1");
  await page.locator(".ms-play").first().click();

  // The controller swaps its host element for a real iframe on SDK failure.
  const frame = page.locator(".ms-leaf iframe.ms-iframe");
  await expect(frame).toHaveCount(1);
  await expect(frame).toHaveAttribute("src", /player\.twitch\.tv/);
});

// ── Multi-view settings ─────────────────────────────────────────────────
test("multi-view settings offer Twitch quality and are honest about YouTube", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
  await page.goto("/app#/settings/multiview");

  const twitch = page.locator('[data-mv-quality="twitch"]');
  await expect(twitch).toBeVisible();
  // Ships defaulting to the best the stream offers.
  await expect(twitch).toHaveValue("best");

  // YouTube's quality API is decommissioned, so offering a control that
  // silently did nothing would be worse than offering none.
  const yt = page.locator(".stg-select[disabled]");
  await expect(yt).toBeVisible();
  await expect(yt).toContainText("Controlled by YouTube");

  // The choice persists per browser.
  await twitch.selectOption("low");
  await page.reload();
  await expect(page.locator('[data-mv-quality="twitch"]')).toHaveValue("low");
});

// ── YouTube ─────────────────────────────────────────────────────────────
//
// Google's script must never load merely because the wall was opened —
// DESIGN.md records avoiding Google for fonts on exactly this reasoning.
// It may load only once a YouTube tile is actually played, by which point
// the viewer has chosen to contact Google anyway.
test("YouTube's script loads on demand, not on page load", async ({ page }) => {
  const googleHits: string[] = [];
  await page.route("**/www.youtube.com/iframe_api*", (r) => {
    googleHits.push(r.request().url());
    return r.abort(); // never actually reach Google from a test
  });
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));

  await page.goto("/app#/watch");
  expect(googleHits, "opening the wall must not contact Google").toHaveLength(0);

  // A Twitch tile must not drag the Google script in either.
  await page.locator(".ms-slot-pick").first().selectOption("live:Twitch:twitch-live-1");
  await page.locator(".ms-play").first().click();
  await page.waitForTimeout(300);
  expect(googleHits, "a Twitch tile must not contact Google").toHaveLength(0);
});

test("a YouTube tile without a video id still plays via the iframe", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
  await page.route("**/www.youtube.com/iframe_api*", (r) => r.abort());
  await page.goto("/app#/watch");

  await page.locator(".ms-slot-pick").first().selectOption("live:YouTube:UClive0000000000000000aa");
  await page.locator(".ms-play").first().click();

  // Whether the API is reachable or not, the tile must end up with a
  // working player rather than an empty box.
  const media = page.locator(".ms-leaf .ms-iframe");
  await expect(media).toHaveCount(1);
});

// ── Routing a click to the wall ─────────────────────────────────────────
//
// Clicking a live channel used to be silently ignored whenever the
// persisted layout still held something — the guard required the slot be
// EMPTY — so you were left looking at the last recording you watched.
test("clicking a live channel replaces whatever was loaded", async ({ page }) => {
  await installFakePlayers(page);
  // Persist a layout holding a recording, exactly as a previous session would.
  await page.addInitScript(() => {
    localStorage.setItem(
      "strivo-player-layout",
      JSON.stringify({ kind: "slot", streamId: null, recordingId: "rec-from-last-time" }),
    );
  });

  await page.goto("/app#/watch?mode=focus&focus=Twitch:twitch-live-1");

  // The requested stream owns the tile; the stale recording is gone.
  await expect(page.locator('.ms-leaf[data-stream-id="Twitch:twitch-live-1"]')).toHaveCount(1);
  await expect(page.locator(".ms-leaf-rec")).toHaveCount(0);
  // And an explicit "watch this" starts playing rather than sitting paused.
  await expect(page.locator(".fake-player")).toHaveCount(1);
});
