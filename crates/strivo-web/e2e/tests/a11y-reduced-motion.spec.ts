// Reduced motion (todo 18) — the one thing this lane must NOT break.
//
// `docs/RESEARCH-PLATFORM-ROADMAP.md:81` lists reduced motion as part of the
// WCAG 2.2 AA target, and the F-18/F-19 mechanism is two independent
// signals: the OS `prefers-reduced-motion` media query, and the
// Settings → Accessibility toggle that puts `.reduce-motion` on <html>
// (`applyReducedMotion`, 012-pvr.js:36-47). Both must suppress.
//
// The observable element is `.recordings-table .state-pill.recording` on
// #/recordings: it carries the same `@keyframes pulse` as the REC-dot family
// and appears in the same reduce block. The REC badges themselves
// (`.ch-rec`, `.live-now h2 .rec-dot`, `.boot-glyph`) are asserted
// structurally instead, because the mock serves no channel that is both
// monitored and mid-recording, so those elements are never painted here —
// that limitation is stated rather than papered over.
import { test, expect, type Page } from "@playwright/test";

/** The REC-dot / boot-glyph family the reduce block must keep naming. */
const DECORATIVE = [
  ".boot-glyph",
  ".ch-rec",
  ".live-now h2 .rec-dot",
  ".recordings-table .state-pill.recording",
  ".state-pill.downloading",
];

const PULSE = ".recordings-table .state-pill.recording";

async function openRecordings(page: Page) {
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
  await page.goto("/app#/recordings");
  await page.locator(".chrome").waitFor();
  await page.waitForTimeout(1200);
}

const animationOf = (page: Page, selector: string) =>
  page.evaluate((sel) => {
    const el = document.querySelector(sel);
    if (!el) return null;
    const cs = getComputedStyle(el);
    return { name: cs.animationName, duration: cs.animationDuration };
  }, selector);

/** Every `prefers-reduced-motion: reduce` block in the served stylesheet, with
 *  the selectors each one names. */
function reduceBlocks(page: Page) {
  return page.evaluate(() => {
    const out: string[][] = [];
    for (const sheet of Array.from(document.styleSheets)) {
      let rules: CSSRuleList;
      try {
        rules = sheet.cssRules;
      } catch {
        continue;
      }
      for (const rule of Array.from(rules)) {
        const media = rule as CSSMediaRule;
        if (!media.media || !String(media.media.mediaText).includes("prefers-reduced-motion")) continue;
        out.push([...media.cssRules].map((r) => (r as CSSStyleRule).selectorText || ""));
      }
    }
    return out;
  });
}

test("reduced motion is suppressed, and the media query is what suppresses it", async ({ page }) => {
  // ── baseline: motion is ON, so a "suppressed" reading below is real ──
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await openRecordings(page);
  const live = await animationOf(page, PULSE);
  expect(live, `no ${PULSE} on #/recordings — the probe has no subject`).not.toBeNull();
  expect(live!.name, "the recording pulse is not running with motion enabled").not.toBe("none");

  // ── the OS signal ──
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.reload();
  await page.locator(".chrome").waitFor();
  await page.waitForTimeout(1200);
  const suppressed = await animationOf(page, PULSE);
  expect(suppressed, `no ${PULSE} after reload`).not.toBeNull();
  expect(suppressed!.name, "prefers-reduced-motion: reduce did not stop the pulse").toBe("none");

  // ── the Settings toggle, a separate signal ──
  // The wait matters: emulateMedia fires the matchMedia change event
  // asynchronously, and the SPA's own listener re-runs applyReducedMotion(),
  // which strips a class added too early. An earlier version of this test
  // added the class immediately and read back a false "not suppressed".
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await page.waitForTimeout(500);
  await page.evaluate(() => document.documentElement.classList.add("reduce-motion"));
  await page.waitForTimeout(100);
  const viaToggle = await animationOf(page, PULSE);
  expect(viaToggle!.name, "the .reduce-motion class did not stop the pulse").toBe("none");
  await page.evaluate(() => document.documentElement.classList.remove("reduce-motion"));

  // ── the structural half: every decorative selector is still named ──
  const blocks = await reduceBlocks(page);
  const named = blocks.flat();
  for (const sel of DECORATIVE) {
    expect(
      named.some((n) => n && n.includes(sel)),
      `${sel} is no longer named by any prefers-reduced-motion block — the REC dot / boot glyph would animate for reduced-motion users`,
    ).toBe(true);
  }
});

test("FAILURE SCENARIO: strip the media query and the pulse comes back", async ({ page }) => {
  // The plan's failure scenario, run in the direction that proves the check
  // has teeth: delete the `prefers-reduced-motion: reduce` block from the
  // CSSOM and confirm the pulse that was suppressed a moment ago is running
  // again. If the "suppressed" assertion in the test above were vacuous —
  // e.g. if the element simply never animated — this would not change.
  await page.emulateMedia({ reducedMotion: "reduce" });
  await openRecordings(page);
  expect((await animationOf(page, PULSE))!.name, "precondition: reduced motion already failed to suppress").toBe("none");

  const removed = await page.evaluate(() => {
    let n = 0;
    for (const sheet of Array.from(document.styleSheets) as CSSStyleSheet[]) {
      let rules: CSSRuleList;
      try {
        rules = sheet.cssRules;
      } catch {
        continue;
      }
      // Walk backwards so index shifts don't skip a block, and delete from
      // the SHEET — `sheet.cssRules` is a plain CSSRuleList and has no
      // deleteRule of its own.
      for (let i = rules.length - 1; i >= 0; i--) {
        const media = rules[i] as CSSMediaRule;
        if (media.media && String(media.media.mediaText).includes("prefers-reduced-motion")) {
          sheet.deleteRule(i);
          n++;
        }
      }
    }
    return n;
  });
  expect(removed, "no prefers-reduced-motion block was found to remove — the probe is looking at nothing").toBeGreaterThan(0);

  // The second, deliberate suppression path is still in play: with the OS
  // signal on, applyReducedMotion() has put `.reduce-motion` on <html>, and
  // `html.reduce-motion .recordings-table .state-pill.recording { animation:
  // none }` is a plain rule, not inside the media query. Clearing the class
  // is what isolates the media query — without this the probe cannot tell a
  // working media query from a redundant one.
  await page.evaluate(() => document.documentElement.classList.remove("reduce-motion"));
  await page.waitForTimeout(100);

  const after = await animationOf(page, PULSE);
  expect(
    after!.name,
    "removing the media query did not bring the animation back — the suppression assertion cannot distinguish the two states",
  ).not.toBe("none");
});
