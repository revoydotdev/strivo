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
    // Stable element identity for the Tab walk. A DOM ordinal from
    // querySelectorAll('*') shifts whenever the page inserts a node, so the
    // same element can carry two ids within one sweep.
    const ids = new WeakMap<Element, number>();
    let next = 0;
    (window as any).__swpId = (el: Element | null) => {
      if (!el) return -1;
      let id = ids.get(el);
      if (id === undefined) {
        id = ++next;
        ids.set(el, id);
      }
      return id;
    };
    localStorage.setItem("strivo-tour-done", "1");
    localStorage.setItem("strivo-player-autoplay", "0");
  });
  await page.goto(`/app#/${route}`);
  await page.locator(".chrome").waitFor();
  await page.waitForTimeout(1000);
}

/** Walk the tab order from the top of the document, recording each stop,
 *  until focus returns to the first stop or the cap is reached. */
async function tabSweep(
  page: Page,
  maxTabs = MIN_TABS,
  onStop?: (stop: { domIndex: number }) => Promise<void>,
) {
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
        domIndex: (window as any).__swpId(el),
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
    // Identity is the element, not its key: a row of identical buttons shares
    // one key, and a key match ended the walk at the second button.
    if (stops.length && stops[0].domIndex === stop.domIndex) {
      wrapped = true;
      break;
    }
    stops.push(stop);
    if (onStop) await onStop(stop);
  }
  return { stops, wrapped };
}

/** Roving-tabindex groups currently on the page. A group is a container in
 *  which the visible same-tag, same-role peers carry tabindex attributes, with
 *  exactly one at tabindex=0 (the rover) and at least one at tabindex=-1. The
 *  search climbs at most six ancestors and takes the first level that
 *  qualifies. Members are ordinals into document.querySelectorAll('*'). */
async function rovingGroups(page: Page) {
  return page.evaluate(() => {
    const all = [...document.querySelectorAll("*")];
    const visible = (el: Element) => {
      const r = el.getBoundingClientRect();
      return r.width > 0 && r.height > 0 && !el.closest("[hidden], [inert], details:not([open])");
    };
    const claimed = new Set<Element>();
    const groups: { rover: number; members: number[]; label: string }[] = [];
    for (const el of all) {
      if (claimed.has(el) || el.getAttribute("tabindex") !== "-1" || !visible(el)) continue;
      const tag = el.tagName;
      const role = el.getAttribute("role");
      let peers: Element[] = [];
      for (let c = el.parentElement, depth = 0; c && depth < 6 && !peers.length; c = c.parentElement, depth++) {
        const cands = [...c.querySelectorAll(tag)].filter(
          (p) => p.getAttribute("role") === role && visible(p) && ["0", "-1"].includes(p.getAttribute("tabindex") ?? ""),
        );
        const rovers = cands.filter((p) => p.getAttribute("tabindex") === "0");
        if (rovers.length === 1 && cands.length > 1 && cands.includes(el)) peers = cands;
      }
      if (!peers.length) continue;
      peers.forEach((p) => claimed.add(p));
      const rover = peers.find((p) => p.getAttribute("tabindex") === "0")!;
      groups.push({
        rover: (window as any).__swpId(rover),
        members: peers.map((p) => (window as any).__swpId(p)),
        label: `${tag.toLowerCase()}${role ? `[role=${role}]` : ""} x${peers.length} (rover ${rover.outerHTML.slice(0, 90)})`,
      });
    }
    return groups;
  });
}

/** Press ArrowDown `size` times from the focused rover and report the ordinal
 *  of the element that holds focus after each press. A full cycle visits every
 *  member and ends back on the rover, which also restores the roving tab stop. */
async function arrowProbe(page: Page, size: number) {
  const visited: number[] = [];
  for (let i = 0; i < size; i++) {
    await page.keyboard.press("ArrowDown");
    visited.push(
      await page.evaluate(() => (window as any).__swpId(document.activeElement)),
    );
  }
  return visited;
}

/** Everything visible that can hold script focus. The Tab sweep's stop set
 *  must be a superset of this: a visible, enabled control that can be
 *  focused but is never reached by Tab is a keyboard trap in the wrong
 *  direction — reachable only by script. */
async function focusableVisible(page: Page) {
  return page.evaluate((sel: string) => {
    const out: { domIndex: number; key: string; sel: string }[] = [];
    // Focusing a roving sibling makes it the group's rover, so the probe
    // below would silently rewrite every tabindex it visits. Snapshot and put
    // them back so the sweep sees the page as it was rendered.
    const saved = [...document.querySelectorAll("[tabindex]")].map((el) => [el, el.getAttribute("tabindex")] as const);
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
        domIndex: (window as any).__swpId(el),
        key: `${el.tagName.toLowerCase()}#${el.id || ""}.${[...el.classList].join(".")}`,
        sel: el.tagName.toLowerCase() + (el.id ? `#${el.id}` : `[class="${[...el.classList].join(" ")}"]`),
      });
    });
    saved.forEach(([el, v]) => (v === null ? el.removeAttribute("tabindex") : el.setAttribute("tabindex", v)));
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

    // Roving-tabindex groups (one Tab stop, arrows move within it) hold their
    // siblings at tabindex=-1, which Tab can never reach by design. Each group
    // is probed for real: when its rover is a Tab stop the sweep presses the
    // arrow key round the whole group, and every member must take focus.
    const groups = await rovingGroups(page);
    const probed = new Map<number, number[]>();
    const cap = tabCap(focusable.length);
    const { stops, wrapped } = await tabSweep(page, cap, async (stop) => {
      const g = groups.find((x) => x.rover === stop.domIndex);
      if (g) probed.set(g.rover, await arrowProbe(page, g.members.length));
    });
    expect(wrapped, `Tab never came back around within ${cap} presses on #/${route}`).toBe(true);

    const reached = new Set(stops.map((s) => s.domIndex));
    const rovingMembers = new Set<number>();
    for (const g of groups) {
      const visited = probed.get(g.rover);
      expect(visited, `#/${route}: roving group ${g.label} has a rover that Tab never reaches`).toBeDefined();
      const missing = g.members.filter((m) => m !== g.rover && !visited!.includes(m));
      expect(missing, `#/${route}: roving group ${g.label}: arrow keys from the tab stop never reach these members`).toEqual([]);
      expect(visited!.at(-1), `#/${route}: roving group ${g.label}: a full arrow cycle did not return to the rover`).toBe(g.rover);
      g.members.forEach((m) => rovingMembers.add(m));
    }
    // Non-rover roving members are not tab stops by design, so the stop count
    // is owed only for the rest of the focusable set.
    const roverIdx = new Set(groups.map((g) => g.rover));
    const expectedStops = focusable.filter((f) => !rovingMembers.has(f.domIndex) || roverIdx.has(f.domIndex)).length;
    expect(stops.length, `Tab reached fewer stops (${stops.length}) than there are focusable controls (${expectedStops}) on #/${route}`).toBeGreaterThanOrEqual(
      expectedStops,
    );
    const unreachable = focusable.filter((f) => !reached.has(f.domIndex) && !rovingMembers.has(f.domIndex));
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

