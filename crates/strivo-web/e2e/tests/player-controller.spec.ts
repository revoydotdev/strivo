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
