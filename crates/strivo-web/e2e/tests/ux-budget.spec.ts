import { test, expect, type Page } from "@playwright/test";

// UX budget for the PVR web UI. Measures, per route, the things a native
// player gets right by default: hit-target size, a type floor, one overlay
// per shortcut, a stage that fits the viewport, and tiles that don't
// letterbox their video away.
//
// Enforcement: EVERY check below is now ENFORCED unconditionally — small
// hit targets, tiny text, overlay count, viewport overflow and letterbox.
// Tiny text and letterbox used to be report-only behind UX_BUDGET=enforce,
// and were converted (WCAG 2.2 AA pass, todo 18):
//   - tiny text: 35 sub-12px sites across all eight routes, all traced to a
//     single cause — rem/em multipliers of 0.6-0.85 against the 14px
//     `html` root at 000-pvr.css:113 — were lifted to var(--text-xs), the
//     scale's own 12px floor. No threshold was moved and no site was
//     reclassified as `.micro` to buy the pass.
//   - letterbox: `.has-aspect` (004b) paired a definite `width: 100%` with
//     `max-height: 100%`, and a definite width makes `max-height` clamp the
//     height without feeding back into the width — so the clamped box was
//     never the declared ratio. A 1x2 split-screen measured 1053x391 tiles
//     (2.69:1) at 34% against a 10% budget. `.watch-content` is now a size
//     container so the fit can use both axes.
//
// Measured at 1440×900 because that is the desktop size the sweep was
// assessed at; the mock lane's default 1280×720 is kept for every other spec.
const MIN_HIT = 28;
const MIN_FONT_PX = 12;
const MAX_LETTERBOX = 0.1;

test.use({ viewport: { width: 1440, height: 900 } });

// #/play is a real ROUTES entry (008-pvr.js:318-338) and was missing here
// until the WCAG pass added it; the two lanes disagreed on the route list.
const ROUTES = ["library", "recordings", "schedule", "watch", "play", "chat", "settings", "system", "logs"];

// Live stream ids served by mock-server.mjs /multistream/tiles.
const MOCK_STREAMS = ["Twitch:twitch-live-1", "YouTube:UClive0000000000000000aa"];

type Offender = { path: string; w?: number; h?: number; px?: number };

// Hit targets deliberately left under 28px — each entry names the owner
// and why it isn't fixed from this lane. Checked against smallHitTargets'
// `path` strings (a truncated ` > `-joined ancestor chain).
const HIT_TARGET_ALLOWLIST: { match: (path: string) => boolean; reason: string }[] = [
  {
    match: (p) => p.includes("ms-preset-summary") || p.includes("ms-layout-menu"),
    reason:
      "#/watch's preset/layout <summary> toggles are Lane A's toolbar HTML " +
      "(018-pvr.js) and out of this lane's ownership. Reported target: 32px " +
      "(matches the rail sort control and rail section headers) — Lane A to apply.",
  },
  {
    match: (p) => p.includes("ch-section-title"),
    reason:
      "Only actually offends on #/watch: Lane A's body.route-watch rule " +
      "(019a-pvr.css) collapses .leftrail to a 56px icon strip and hides " +
      "this button's label/caret/count children, leaving the empty button " +
      "shell at ~22px wide. That's inside Lane A's owned route-watch scope " +
      "(this lane was told not to touch .leftrail/.chrome grid geometry " +
      "this round) — flagging for Lane A/coordinator rather than fixing here.",
  },
];

function report(title: string, offenders: unknown[], enforce: boolean) {
  test.info().annotations.push({ type: "ux-budget", description: `${title}: ${offenders.length}` });
  if (offenders.length) {
    void test.info().attach(title, {
      body: JSON.stringify(offenders, null, 2),
      contentType: "application/json",
    });
  }
  if (enforce) expect(offenders, title).toEqual([]);
}

async function open(page: Page, route: string, layout?: unknown, preset = "custom") {
  await page.addInitScript(
    ({ layout, preset }) => {
      localStorage.setItem("strivo-tour-done", "1");
      localStorage.setItem("strivo-player-autoplay", "0");
      if (layout) {
        localStorage.setItem("strivo-player-layout", JSON.stringify(layout));
        // The preset name matters: aspect-aware sizing only engages for the
        // grid-regular presets, so a quadrant seeded as "custom" measures the
        // unconstrained path instead of the one users get from the menu.
        localStorage.setItem("strivo-player-preset", preset);
      }
    },
    { layout, preset },
  );
  await page.goto(`/app#/${route}`);
  await page.locator(".chrome").waitFor();
  // Let async hydration (channels, recordings, tiles) paint.
  await page.waitForTimeout(800);
}

// Every visible interactive element smaller than MIN_HIT in either
// dimension. Range inputs and rail rows are judged on height only: they
// are deliberately wide-and-short.
async function smallHitTargets(page: Page): Promise<Offender[]> {
  return page.evaluate((min) => {
    const cssPath = (el: Element) => {
      const parts: string[] = [];
      let e: Element | null = el;
      while (e && parts.length < 4 && e !== document.body) {
        let s = e.tagName.toLowerCase();
        if (e.id) s += `#${e.id}`;
        else if (e.classList.length) s += `.${[...e.classList].slice(0, 2).join(".")}`;
        parts.unshift(s);
        e = e.parentElement;
      }
      return parts.join(" > ");
    };
    const visible = (el: Element) => {
      const r = el.getBoundingClientRect();
      if (r.width === 0 || r.height === 0) return false;
      const cs = getComputedStyle(el);
      if (cs.visibility === "hidden" || cs.display === "none" || Number(cs.opacity) === 0) return false;
      if (el.closest("[hidden], [aria-hidden='true']")) return false;
      return true;
    };
    const heightOnly = (el: Element) =>
      el.matches("input[type=range], .ch-row, .hist-hm-cell");
    const out: { path: string; w: number; h: number }[] = [];
    document
      .querySelectorAll("button, a[href], input, select, summary, [role=button], [tabindex='0']")
      .forEach((el) => {
        if (!visible(el)) return;
        const r = el.getBoundingClientRect();
        const tooSmall = heightOnly(el) ? r.height < min : r.width < min || r.height < min;
        if (tooSmall) out.push({ path: cssPath(el), w: Math.round(r.width), h: Math.round(r.height) });
      });
    return out;
  }, MIN_HIT);
}

// Every visible text node whose computed font-size is under the floor,
// unless an ancestor opts in with `.micro` (badges, tags).
async function tinyText(page: Page): Promise<Offender[]> {
  return page.evaluate((min) => {
    const seen = new Map<string, number>();
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    let n: Node | null;
    while ((n = walker.nextNode())) {
      if (!n.textContent || !n.textContent.trim()) continue;
      const el = n.parentElement;
      if (!el || el.closest(".micro, script, style")) continue;
      const r = el.getBoundingClientRect();
      if (r.width === 0 || r.height === 0) continue;
      const px = parseFloat(getComputedStyle(el).fontSize);
      if (px >= min) continue;
      const key = `${el.tagName.toLowerCase()}${el.classList.length ? "." + [...el.classList].slice(0, 2).join(".") : ""}@${px}px`;
      seen.set(key, (seen.get(key) || 0) + 1);
    }
    return [...seen.entries()].map(([path, count]) => ({ path, px: count }));
  }, MIN_FONT_PX);
}

// Outermost visible overlay containers only — a palette's inner
// [role=dialog] must not count twice, but two stacked palettes must.
async function visibleDialogs(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const sel = "[role=dialog], .app-modal.open, .kbd-help.open, #cmdk";
    return [...document.querySelectorAll(sel)]
      .filter((el) => {
        const r = el.getBoundingClientRect();
        if (r.width === 0 || r.height === 0 || getComputedStyle(el).visibility === "hidden") return false;
        return !el.parentElement?.closest(sel);
      })
      .map((el) => `${el.tagName.toLowerCase()}#${el.id}.${[...el.classList].join(".")}`);
  });
}

for (const route of ROUTES) {
  test.describe(`#/${route}`, () => {
    test("hit targets are at least 28px", async ({ page }) => {
      await open(page, route);
      const all = await smallHitTargets(page);
      const allowed = all.filter((o) => HIT_TARGET_ALLOWLIST.some((a) => a.match(o.path)));
      const offenders = all.filter((o) => !HIT_TARGET_ALLOWLIST.some((a) => a.match(o.path)));
      if (allowed.length) {
        test.info().annotations.push({
          type: "ux-budget-allowlisted",
          description: `small hit targets (allowlisted, not enforced): ${allowed.length}`,
        });
      }
      report("small hit targets", offenders, true);
    });

    test("no text under 12px outside .micro", async ({ page }) => {
      await open(page, route);
      report("tiny text", await tinyText(page), true);
    });

    test("Ctrl+K opens exactly one overlay and Escape closes it", async ({ page }) => {
      await open(page, route);
      await page.keyboard.press("Control+k");
      await page.waitForTimeout(500);
      const opened = await visibleDialogs(page);
      await page.keyboard.press("Escape");
      await page.waitForTimeout(500);
      const closed = await visibleDialogs(page);
      report("overlays after Ctrl+K (want 1)", opened.length === 1 ? [] : opened.map((path) => ({ path })), true);
      report("overlays after Escape (want 0)", closed.map((path) => ({ path })), true);
    });
  });
}

test.describe("#/watch and #/chat fit the viewport", () => {
  for (const route of ["watch", "chat"]) {
    test(`#/${route} does not scroll`, async ({ page }) => {
      await open(page, route);
      const m = await page.evaluate(() => ({
        doc: document.documentElement.scrollHeight - document.documentElement.clientHeight,
        content: (() => {
          const c = document.getElementById("content");
          return c ? c.scrollHeight - c.clientHeight : 0;
        })(),
      }));
      report("viewport overflow px", m.doc > 1 || m.content > 1 ? [{ path: `document +${m.doc}px, #content +${m.content}px` }] : [], true);
    });
  }
});

// Letterbox = share of a tile's area not covered by a 16:9 fit. Presets
// are seeded through localStorage as poster tiles, so no player factory is
// needed to measure geometry.
const split = (dir: "h" | "v", a: unknown, b: unknown) => ({ kind: "split", dir, ratio: 0.5, a, b });
const slot = (streamId: string | null) => ({ kind: "slot", streamId, recordingId: null });
const LAYOUTS: Record<string, unknown> = {
  single: slot(MOCK_STREAMS[0]),
  "split-screen": split("h", slot(MOCK_STREAMS[0]), slot(MOCK_STREAMS[1])),
  quadrant: split(
    "v",
    split("h", slot(MOCK_STREAMS[0]), slot(MOCK_STREAMS[1])),
    split("h", slot(MOCK_STREAMS[1]), slot(MOCK_STREAMS[0])),
  ),
};

test.describe("#/watch tiles keep their video", () => {
  for (const [name, layout] of Object.entries(LAYOUTS)) {
    test(`${name}: letterbox ≤ 10% per tile`, async ({ page }) => {
      await open(page, "watch", layout, name);
      const offenders = await page.evaluate((max) => {
        const out: { path: string; w: number; h: number; px: number }[] = [];
        document.querySelectorAll(".ms-leaf:not(.ms-empty)").forEach((leaf) => {
          const r = leaf.getBoundingClientRect();
          if (r.width === 0 || r.height === 0) return;
          const fit = Math.min(r.width / 16, r.height / 9);
          const letterbox = 1 - (fit * 16 * fit * 9) / (r.width * r.height);
          if (letterbox > max) {
            out.push({
              path: `.ms-leaf[data-path="${(leaf as HTMLElement).dataset.path}"]`,
              w: Math.round(r.width),
              h: Math.round(r.height),
              px: Math.round(letterbox * 100),
            });
          }
        });
        return out;
      }, MAX_LETTERBOX);
      report("letterboxed tiles (% wasted)", offenders, true);
    });
  }
});
