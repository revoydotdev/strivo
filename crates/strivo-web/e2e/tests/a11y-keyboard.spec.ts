// Keyboard-only operability pass (todo 18).
//
// Every assertion here is on FOCUS or on an observable state change, never on
// the absence of an error — a driver that never presses anything produces no
// error either. The Tab sweeps are bounded: a control that cannot be found
// fails with its selector named rather than stalling the run.
import { test, expect, type Page } from "@playwright/test";

/** Anything the browser will put in the tab order. Kept in sync with what the
 *  sweep proves rather than with any one component's markup. */
const TABBABLE =
  "a[href], button, input, select, textarea, summary, [tabindex]:not([tabindex='-1'])";

/** Lower bound on Tab presses per sweep; the real cap is
 *  `tabCap(focusableCount)` below. A cap is what turns "the walk cannot find
 *  its target" into a named failure instead of a hang. */
const MIN_TABS = 400;

/** #/recordings legitimately carries a tab stop per row (checkbox, row
 *  actions) and the mock serves a long library, so the cap scales with what
 *  is actually on the page rather than being a number that quietly starts
 *  truncating the walk — which would have read as "Tab never came back
 *  around", a false failure. */
const tabCap = (focusableCount: number) => Math.max(MIN_TABS, focusableCount * 3 + 50);

async function open(page: Page, route: string) {
  await page.addInitScript(() => {
    localStorage.setItem("strivo-tour-done", "1");
    localStorage.setItem("strivo-player-autoplay", "0");
  });
  await page.goto(`/app#/${route}`);
  await page.locator(".chrome").waitFor();
  await page.waitForTimeout(1000);
}

/** Walk the tab order from the top of the document, recording each stop,
 *  until focus returns to the first stop or the cap is reached. */
async function tabSweep(page: Page, maxTabs = MIN_TABS) {
  await page.evaluate(() => {
    (document.activeElement as HTMLElement | null)?.blur();
    document.body.focus();
  });
  const stops: { key: string; sel: string; text: string; domIndex: number }[] = [];
  let wrapped = false;
  for (let i = 0; i < maxTabs; i++) {
    await page.keyboard.press("Tab");
    const stop = await page.evaluate(() => {
      const el = document.activeElement as HTMLElement | null;
      if (!el || el === document.body) return null;
      const style = getComputedStyle(el);
      return {
        // An ordinal into document.querySelectorAll('*'), not a rebuilt
        // selector string: `a#.skip-link` is not a valid selector, and a
        // selector that throws is a check that reports nothing.
        domIndex: [...document.querySelectorAll("*")].indexOf(el),
        key: `${el.tagName.toLowerCase()}#${el.id || ""}.${[...el.classList].join(".")}`,
        sel: el.tagName.toLowerCase() + (el.id ? `#${el.id}` : `[class="${[...el.classList].join(" ")}"]`),
        text: (el.getAttribute("aria-label") || el.textContent || "").trim().slice(0, 30),
        outlineStyle: style.outlineStyle,
        outlineWidth: style.outlineWidth,
        boxShadow: style.boxShadow,
        backgroundColor: style.backgroundColor,
      };
    });
    if (!stop) continue;
    if (stops.length && stops[0].key === stop.key) {
      wrapped = true;
      break;
    }
    stops.push(stop);
  }
  return { stops, wrapped };
}

/** Everything visible that can hold script focus. The Tab sweep's stop set
 *  must be a superset of this: a visible, enabled control that can be
 *  focused but is never reached by Tab is a keyboard trap in the wrong
 *  direction — reachable only by script. */
async function focusableVisible(page: Page) {
  return page.evaluate((sel: string) => {
    const out: { key: string; sel: string }[] = [];
    document.querySelectorAll(sel).forEach((el) => {
      const html = el as HTMLInputElement;
      if (html.disabled) return;
      if (el.closest("[hidden], [inert]")) return;
      if (el.closest("details:not([open])")) return;
      const r = el.getBoundingClientRect();
      if (r.width === 0 || r.height === 0) return;
      const cs = getComputedStyle(el);
      if (cs.visibility === "hidden" || cs.display === "none" || Number(cs.opacity) === 0) return;
      html.focus();
      if (document.activeElement !== el) return;
      out.push({
        key: `${el.tagName.toLowerCase()}#${el.id || ""}.${[...el.classList].join(".")}`,
        sel: el.tagName.toLowerCase() + (el.id ? `#${el.id}` : `[class="${[...el.classList].join(" ")}"]`),
      });
    });
    return out;
  }, TABBABLE);
}

/** The five PVR routes this lane originally covered. Kept as its own list
 *  because the two groups are asserted separately below: a Creator route that
 *  cannot be reached by Tab is a different defect from a PVR one, and the
 *  failure message has to say which surface it was on. */
const PVR_ROUTES = ["library", "recordings", "settings", "system", "logs"];

/** The Creator-gated routes (012-pvr.js `CREATOR_ROUTES`). They were absent
 *  from this lane while `a11y-scan.spec.ts` already swept them for axe, so
 *  "keyboard-operable" was being claimed for the whole UI on the strength of
 *  five PVR screens. The timeline is the hard case among them: a lane full of
 *  absolutely-positioned clips, addressed by a key map rather than by Tab
 *  order. */
const CREATOR_ROUTES = [
  "timeline",
  "studio",
  "analytics",
  "publish",
  "pipelines",
  "plugins",
  "dataviz",
  "archive",
];

const ALL_ROUTES = [
  ...PVR_ROUTES.map((r) => [r, "PVR"] as const),
  ...CREATOR_ROUTES.map((r) => [r, "Creator"] as const),
];

for (const [route, edition] of ALL_ROUTES) {
  test(`${edition} #/${route}: every visible control is reachable by Tab alone`, async ({ page }) => {
    await open(page, route);

    const focusable = await focusableVisible(page);
    expect(focusable.length, `#/${route} has no focusable controls at all — the sweep would be vacuous`).toBeGreaterThan(5);

    const cap = tabCap(focusable.length);
    const { stops, wrapped } = await tabSweep(page, cap);
    expect(wrapped, `Tab never came back around within ${cap} presses on #/${route}`).toBe(true);
    expect(stops.length, `Tab reached fewer stops (${stops.length}) than there are focusable controls (${focusable.length}) on #/${route}`).toBeGreaterThanOrEqual(
      focusable.length,
    );

    const reached = new Set(stops.map((s) => s.key));
    const unreachable = focusable.filter((f) => !reached.has(f.key));
    expect(
      unreachable.map((u) => u.sel),
      `#/${route}: controls that hold focus but Tab never reaches`,
    ).toEqual([]);

    // Every stop must be a native control (or an element carrying an explicit
    // tabindex, which `TABBABLE` also matches) — a stray focusable div left
    // in the tree is a stop nobody can reason about.
    const notControls = stops.filter(
      (s) => !/^(a|button|input|select|textarea|summary)(#|\[|\.)/.test(s.sel) && !/^[a-z]+\[class=/.test(s.sel),
    );
    expect(notControls, `#/${route}: tab stops that are not native controls`).toEqual([]);
  });

  test(`${edition} #/${route}: no positive tabindex (WCAG 2.4.3 focus order)`, async ({ page }) => {
    await open(page, route);
    const positive = await page.evaluate(() =>
      [...document.querySelectorAll("[tabindex]")]
        .map((el) => ({ el: el as HTMLElement, v: parseInt((el as HTMLElement).tabIndex, 10) }))
        .filter((x) => x.v > 0)
        .map((x) => `${x.el.tagName.toLowerCase()}#${x.el.id || ""}[tabindex=${x.v}]`),
    );
    expect(positive, `#/${route}: positive tabindex overrides DOM focus order`).toEqual([]);
  });
}

const PAINTED = [
  "outlineStyle",
  "outlineWidth",
  "outlineColor",
  "boxShadow",
  "backgroundColor",
  "borderColor",
  "color",
  "textDecorationLine",
] as const;

test("focused controls paint a visible focus indicator", async ({ page }) => {
  await open(page, "recordings");

  // Resting state, captured before a single Tab press.
  const resting = await page.evaluate(
    ({ sel, keys }) => {
      const map: Record<number, string[]> = {};
      document.querySelectorAll(sel).forEach((el, i) => {
        const cs = getComputedStyle(el);
        map[i] = keys.map((k) => cs[k as never]);
      });
      return map;
    },
    { sel: TABBABLE, keys: PAINTED as unknown as string[] },
  );

  const { stops } = await tabSweep(page);
  expect(stops.length).toBeGreaterThan(5);

  // Compare each stop's style WHILE it holds real keyboard focus. Doing this
  // with a later programmatic .focus() reads as "no ring" for controls whose
  // ring is behind :focus-visible, which is most of them.
  const readFocused = await page.evaluate(
    ({ keys, sel }) => {
      const out: Record<number, string[]> = {};
      document.querySelectorAll(sel).forEach((el, i) => {
        if (el !== document.activeElement) return;
        const cs = getComputedStyle(el);
        out[i] = keys.map((k) => cs[k as never]);
      });
      return out;
    },
    { keys: PAINTED as unknown as string[], sel: TABBABLE },
  );

  // Compare at each stop while it genuinely holds keyboard focus. Doing this
  // with a later programmatic .focus() reads as "no ring" for controls whose
  // ring lives behind :focus-visible, which is most of them.
  const invisible: string[] = [];
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  for (let i = 0; i < MIN_TABS; i++) {
    await page.keyboard.press("Tab");
    const now = await page.evaluate(
      ({ keys, sel }) => {
        const el = document.activeElement as HTMLElement | null;
        if (!el || el === document.body) return null;
        const idx = [...document.querySelectorAll(sel)].indexOf(el as Element);
        const cs = getComputedStyle(el);
        return {
          idx,
          name: `${el.tagName.toLowerCase()}#${el.id || ""}`,
          style: keys.map((k) => cs[k as never]),
        };
      },
      { keys: PAINTED as unknown as string[], sel: TABBABLE },
    );
    if (!now || now.idx < 0) continue;
    const before = resting[now.idx];
    if (!before) continue;
    if (before.every((v, k) => v === now.style[k])) invisible.push(now.name);
  }
  expect(invisible, "controls whose focus state is visually identical to their resting state").toEqual([]);
});

