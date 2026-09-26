#!/usr/bin/env node
// DESIGN.md conformance audit for the assembled SPA stylesheet.
//
// `crates/strivo-web/build.rs` concatenates `assets/spa-css/*` in filename
// order into the sheet the daemon serves (`-creator` modules only under the
// `creator` feature), so there is no file in source to read a token table off —
// and both editions have to be audited, since a Creator-only module may be the
// only place a value appears.
//
// This script audits the five axes DESIGN.md is normative about, per edition:
//
//   COLOUR  the token table (DESIGN.md:30-48) and the fixed platform block
//           (DESIGN.md:130-142). GATED.
//   TYPE    the scale table (DESIGN.md:72-84). Reported.
//   SPACING the scale (DESIGN.md:172-186). Reported.
//   RADII   the radii (DESIGN.md:42, 192-199). Reported.
//   MOTION  durations and easings (DESIGN.md:201-219). Reported.
//
// Only COLOUR is a gate, and that is the plan's criterion, not a choice: the
// other four axes describe a system the SPA does not actually use (a
// nine-step type scale against 112 distinct type values; an eight-step spacing
// ladder against a rem continuum; no `--dur-*`/`--ease-*` tokens at all), so
// "gate on off-scale" would fail on 300+ sites whose only correct fix is a
// decision this script is not allowed to make. A zero colour count says
// nothing about a wrong `font-size` or an unwired easing, which is why the four
// other axes are reported with their own counts and their own verdict.
//
// The colour gate is deliberately narrow, and the distinction is the whole
// point of this file:
//
//   REDUCIBLE  a literal that resolves to a colour the `:root` table already
//              names. Substituting `var(--x)` — or `color-mix(in srgb,
//              var(--x) N%, transparent)` for the alpha case, which is the
//              idiom the stylesheet already uses — is a zero-visual-change
//              reconciliation. THE GATE. Bar: zero.
//   NEAR-MISS  a literal in the same hue family as a token but not equal to
//              it (`#fff` vs `--fg`, `hsl(0,70%,45%)` vs `--recording`).
//              Adopting the token CHANGES the colour. A look decision.
//              Reported with the token and the measured delta.
//   UNTOKENISED no token in the table describes it: the `rgba(0,0,0,a)` scrim
//              ladder, the `rgba(255,255,255,a)` wash ladder, the second
//              Tailwind-ish palette. Closing these means minting a token, which
//              the plan forbids ("no new tokens minted to make a number come
//              out"). Reported with a proposed name per ladder.
//   PLATFORM  the three canonical platform hexes (DESIGN.md:130-142). Fixed and
//              not themeable, so they are exempt from the census and guarded
//              separately: if one changes, this fails.
//
// Two things this script refuses to do, both of which would make the numbers
// look better and the tree worse:
//
//   * Wrap a literal in `var(--scrim-25, rgba(0,0,0,0.25))`. That is literally
//     "inside a var(--x, #fallback)" and would zero the census — while minting
//     every token the plan forbids, invisibly, in fallback position. Rejected.
//   * Rewrite a near-miss onto its token. That is 12 `#fff` sites and four
//     recording-red shades changing colour, i.e. a look change disguised as a
//     reconciliation.
//
// Usage:
//   node scripts/check-design-tokens.mjs            # the gate + all five reports
//   node scripts/check-design-tokens.mjs --strict    # also fail on ANY remaining
//                                                      # non-platform literal
//                                                      # (the plan's literal bar;
//                                                      # see the report, which
//                                                      # states why it is not
//                                                      # reachable without the
//                                                      # decisions above)
import { readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { assemble, collect, index } from "./check-css-tokens.mjs";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..");

// ---------------------------------------------------------------------------
// DESIGN.md, encoded. Every entry carries the line it comes from, because a
// conformance report that cannot cite its own rules is an opinion.
// ---------------------------------------------------------------------------

/** DESIGN.md:72-84 §Typography Scale. `rem` there assumes a 16px root. */
const TYPE_SCALE = [
  [72, "Display XL", "hero headlines"],
  [48, "Display", "section headlines"],
  [28, "Display SM", "section titles"],
  [24, "Heading", "card/panel headings"],
  [18, "Subheading", "subtitles, lead text"],
  [16, "Body", "default paragraph text"],
  [14, "Body SM", "secondary text, descriptions"],
  [13, "Caption", "labels, metadata"],
  [11, "Micro", "badges, tags, system labels"],
];

/** DESIGN.md:172-186 §Spacing. Base unit 8px, comfortable density. */
/** 0 is on every spacing scale, and `999px` is the same step as `9999px`. */
const ZERO_STEPS = { spacing: true, radius: true };
const SPACING_SCALE = [
  [4, "2xs", "inline gaps, icon padding"],
  [8, "xs", "tight element spacing"],
  [12, "sm", "input padding, compact gaps"],
  [16, "md", "default element spacing"],
  [24, "lg", "section internal padding"],
  [32, "xl", "card padding, component gaps"],
  [48, "2xl", "section spacing"],
  [64, "3xl", "page section breaks"],
];

/**
 * DESIGN.md carries TWO radii tables and they disagree.
 *   :42   (the JellySkin §Tokens mirror, which §16-28 makes canonical for the
 *         SPA)  — lg 20px, base 10px, sm 8px.
 *   :192-199 (§Layout, written for the TUI era) — sm 4px, md 8px, lg 12px,
 *         full 9999px.
 * The `:root` block ships 20/10/8, so `:42` is the operative set and §Layout
 * contributes 4px and 9999px. Reported as a DESIGN.md contradiction; the CSS
 * is not changed on the strength of a doc that disagrees with itself.
 */
const RADIUS_SCALE = [
  [4, "sm (§Layout)", "inputs, small elements"],
  [8, "sm (§Tokens)", "tokens mirror — matches §Layout md"],
  [10, "base (§Tokens)", "cards, surfaces"],
  [12, "lg (§Layout)", "panels, modals"],
  [20, "lg (§Tokens)", "tokens mirror"],
  [9999, "full (§Layout)", "pills, status dots, toggles"],
  // 999px is what the stylesheet actually writes for a pill. Any value at or
  // above 999px clamps to the same fully-rounded shape on any element smaller
  // than itself, so it is the same step written differently, not a deviation.
  [999, "full (§Layout, as written — same step as 9999px)", "pills, status dots"],
];

/** DESIGN.md:201-219 §Motion. */
const MOTION_STEPS = [
  [50, "micro", "toggles, checkboxes"],
  [100, "micro", "toggles, checkboxes"],
  [150, "short", "hover, focus, small transitions"],
  [250, "medium", "panel transitions, content shifts"],
  [400, "long", "page transitions"],
  [700, "long", "page transitions"],
];
/** DESIGN.md:204 — the REC dot is specified as a 2s pulse. */
const REC_DOT_PULSE_MS = 2000;
const EASINGS = [
  ["cubic-bezier(0.16, 1, 0.3, 1)", "enter"],
  ["ease-in", "exit"],
  ["ease-in-out", "move"],
];
/** DESIGN.md:130-142 §Platform Colors — fixed, not themeable. */
const PLATFORM = [
  ["twitch", "#a970ff"],
  ["youtube", "#ff4d4d"],
  ["patreon", "#f96854"],
];

// `html, body { font-size: 14px }` in 000-pvr.css:113, not the 16px the scale
// table's `rem` column assumes. Every rem in the SPA resolves against this, so
// every axis below is measured in px against it.
const ROOT_FONT_PX = 14;

/** Nearest step in a `[[px, name, usage]]` scale, or null if further than `tol`. */
function nearestStep(px, scale, tol = 0.51) {
  let best = null;
  for (const [v, name, usage] of scale) {
    const d = Math.abs(v - px);
    if (d <= tol && (!best || d < best.d)) best = { d, v, name, usage };
  }
  return best;
}

// ---------------------------------------------------------------------------
// Colour resolution, and the test for it.
//
// The inherited failure this guards against: a normaliser that forced every
// hsla() to alpha 1 (or collapsed alpha entirely, or resolved hsl to 0..1 while
// rgb resolved to 0..255) makes two DIFFERENT colours compare equal, and the
// gate then "proves" substitutions that were never equal. The first version of
// the resolver below had exactly that bug — every hsl literal landed in 0..1 —
// and this test is what caught it. It runs on every invocation and fails the
// script, so a future regression cannot quietly turn the gate into a rubber
// stamp.
// ---------------------------------------------------------------------------

function resolve(value) {
  const v = value.trim().replace(/\s+/g, " ");
  if (/^#[0-9a-fA-F]{3,8}$/.test(v)) {
    let h = v.slice(1);
    if (h.length === 3 || h.length === 4) h = [...h].map((c) => c + c).join("");
    if (h.length !== 6 && h.length !== 8) return null;
    return [
      parseInt(h.slice(0, 2), 16),
      parseInt(h.slice(2, 4), 16),
      parseInt(h.slice(4, 6), 16),
      h.length === 8 ? parseInt(h.slice(6, 8), 16) / 255 : 1,
    ];
  }
  {
    const m = v.match(/^(?:hsl)a?\(([^)]*)\)$/i);
    if (m) {
    const p = m[1].split(/[,/]/).map((s) => s.trim()).filter((s) => s !== "");
    if (p.length < 3) return null;
    const h = p[0];
    // `grad` before `rad`: "100grad".endsWith("rad") is true, so the rad branch
    // would swallow it and turn 100grad into 5729 degrees.
    const deg = h.endsWith("deg") ? parseFloat(h)
      : h.endsWith("turn") ? parseFloat(h) * 360
      : h.endsWith("grad") ? parseFloat(h) * 0.9
      : h.endsWith("rad") ? parseFloat(h) * 180 / Math.PI
      : parseFloat(h);
    const s = parseFloat(p[1]) / 100;
    const l = parseFloat(p[2]) / 100;
    const a = alpha(p[3]);
    if ([deg, s, l, a].some(Number.isNaN)) return null;
    const c = (1 - Math.abs(2 * l - 1)) * s;
    const hp = (((deg % 360) + 360) % 360) / 60;
    const x = c * (1 - Math.abs((hp % 2) - 1));
    const seg = [[c, x, 0], [x, c, 0], [0, c, x], [0, x, c], [x, 0, c], [c, 0, x]][Math.floor(hp) % 6];
    const m0 = l - c / 2;
    // 0..1 -> 0..255, so hsl and rgb literals are comparable at all.
    return [(seg[0] + m0) * 255, (seg[1] + m0) * 255, (seg[2] + m0) * 255, a];
    }
  }
  {
    const m = v.match(/^(?:rgb)a?\(([^)]*)\)$/i);
    if (!m) return null;
    const p = m[1].split(/[,/]/).map((s) => s.trim()).filter((s) => s !== "");
    if (p.length < 3) return null;
    const chan = (x) => (x.endsWith("%") ? (parseFloat(x) * 255) / 100 : parseFloat(x));
    const out = [chan(p[0]), chan(p[1]), chan(p[2]), alpha(p[3])];
    return out.some(Number.isNaN) ? null : out;
  }
}

function alpha(a) {
  if (a === undefined) return 1;
  return a.endsWith("%") ? parseFloat(a) / 100 : parseFloat(a);
}

/**
 * 1.5/255 tolerance: browsers round an hsl literal to 8 bits, so two literals
 * that mean the same colour can differ in the last bit. Pinned by the self-test
 * at the boundary (1/255 matches, 2/255 does not) — a tolerance that quietly
 * grew to 8 would start calling visibly different colours equal.
 */
const near = (a, b) => a != null && b != null && a.every((v, i) => Math.abs(v - b[i]) <= 1.5);

/** Bit-exact: no channel differs at all. Distinct from `near` on purpose. */
const exactRgb = (a, b) => a != null && b != null && a.every((v, i) => v === b[i]);

const achromatic = (rgb) => Math.max(...rgb) - Math.min(...rgb) <= 4;

function hueOf(rgb) {
  const [r, g, b] = rgb.map((v) => v / 255);
  const mx = Math.max(r, g, b);
  const mn = Math.min(r, g, b);
  const d = mx - mn;
  if (d === 0) return null;
  let h;
  if (mx === r) h = ((g - b) / d) % 6;
  else if (mx === g) h = (b - r) / d + 2;
  else h = (r - g) / d + 4;
  h *= 60;
  return h < 0 ? h + 360 : h;
}

const hueDelta = (a, b) => {
  const d = Math.abs(a - b) % 360;
  return d > 180 ? 360 - d : d;
};

function selfTest() {
  const failures = [];
  const check = (ok, msg) => { if (!ok) failures.push(msg); };

  // Same colour, different notation — must compare equal.
  const SAME = [
    ["#eef3f8", "#EeF3F8"],
    ["#abc", "#aabbcc"],
    ["#abcd", "#aabbccdd"],
    ["hsl(208, 89%, 5%)", "hsla(208,89%,5%,1)"],
    ["hsla(0, 0%, 100%, 0.12)", "rgba(255,255,255,0.12)"], // cross-space
    ["rgb(0, 0, 0)", "rgba(0, 0, 0, 0.5)"],                 // alpha-only delta
    ["hsl(150deg, 60%, 50%)", "hsl(0.4166667turn, 60%, 50%)"],
    ["hsl(90, 60%, 50%)", "hsl(100grad, 60%, 50%)"],
    ["hsl(90, 60%, 50%)", "hsl(1.5708rad, 60%, 50%)"],
    ["hsl(285,46%,56%)", "hsl(285, 46%, 56%)"],
  ];
  for (const [a, b] of SAME) {
    const ra = resolve(a), rb = resolve(b);
    check(!!ra && near(ra.slice(0, 3), rb.slice(0, 3)),
      `resolver: "${a}" and "${b}" are the same colour but did not compare equal (${JSON.stringify(ra)} vs ${JSON.stringify(rb)})`);
  }

  // Different colours that a sloppy normaliser would collapse — the exact
  // failures already made twice in this repo's history.
  const DIFF = [
    ["hsl(0,70%,45%)", "hsl(0,70%,60%)", "45% vs 60% lightness"],
    ["#fff", "#eef3f8", "white vs --fg"],
    ["#9146ff", "#a970ff", "official vs shipped Twitch"],
    ["hsl(285,46%,56%)", "hsl(285,46%,57%)", "1% lightness"],
    ["#3b82f6", "#4ec9b0", "two Tailwind-ish hexes"],
    ["hsl(195,100%,43%)", "hsl(285,46%,56%)", "accent2 vs accent"],
  ];
  for (const [a, b, why] of DIFF) {
    const ra = resolve(a), rb = resolve(b);
    check(!(ra && near(ra.slice(0, 3), rb.slice(0, 3))),
      `resolver: "${a}" and "${b}" are DIFFERENT colours (${why}) but compared equal`);
  }

  // Alpha must survive resolution. Forcing it to 1 is the bug that made
  // hsla(0,0%,100%,.15) look like --border's .12.
  check(resolve("hsla(0,0%,100%,0.15)")[3] !== resolve("#ffffff")[3],
    "resolver: alpha was collapsed to 1");
  check(Math.abs(resolve("hsla(0,0%,100%,0.12)")[3] - 0.12) < 1e-9,
    "resolver: alpha 0.12 not preserved");

  // The tolerance boundary, so it cannot grow unnoticed.
  check(near(resolve("#ffffff").slice(0, 3), resolve("#fffffe").slice(0, 3)),
    "resolver: 1/255 apart should compare equal");
  check(!near(resolve("#ffffff").slice(0, 3), resolve("#fdfdfe").slice(0, 3)),
    "resolver: 2/255 apart must not compare equal");

  return failures;
}

// ---------------------------------------------------------------------------
// The `:root` palette. Read out of the ASSEMBLED PVR sheet's `:root` block, not
// out of a hand-kept list, so adding a token to the CSS cannot leave the audit
// describing the old palette.
// ---------------------------------------------------------------------------

function readPalette() {
  const css = assemble(false);
  const open = css.indexOf(":root {");
  if (open === -1) return { tokens: [], blockFound: false };
  const close = css.indexOf("\n}", open);
  const block = css.slice(open, close);
  const tokens = [];
  for (const m of block.matchAll(/(--[A-Za-z0-9_-]+)\s*:\s*([^;]+);/g)) {
    // A token's value may be a composite — `--shadow: 0 8px 24px hsla(...)`,
    // `--gradient: linear-gradient(...hsl(...)..., hsl(...))`. Reading only
    // whole-value literals would leave the shadow colour invisible to the
    // audit, so every colour function inside the value is registered against
    // its token. The name is annotated when a value carries more than one.
    const found = [...m[2].matchAll(/#[0-9a-fA-F]{3,8}\b|(?:hsl|rgb)a?\([^)]*\)/g)]
      .map((x) => resolve(x[0]))
      .filter(Boolean);
    // A token whose value is a COMPOSITE (`--shadow: 0 8px 24px hsla(...)`,
    // `--gradient: linear-gradient(...)`) carries a colour but cannot be
    // substituted for a bare colour literal: `var(--shadow)` in place of a
    // gradient stop renders the whole shadow. Carriers are used for
    // attribution only; only a single-literal token is substitutable. Getting
    // this wrong was caught by reading the proposed substitution, not the
    // count — the classifier was perfectly confident and completely wrong.
    const substitutable = found.length === 1 && resolve(m[2].trim()) != null;
    found.forEach((rgb, i) => {
      tokens.push({
        name: found.length > 1 ? `${m[1]} (colour ${i + 1}/${found.length})` : m[1],
        raw: m[2].trim(),
        rgb,
        substitutable,
      });
    });
  }
  return { tokens, blockFound: true };
}

// ---------------------------------------------------------------------------
// Per-axis declaration extraction, reusing todo 8's `index()` so a site is
// reported as `module:line` rather than an offset into a concatenation.
// ---------------------------------------------------------------------------

function declarations(css, valueAt) {
  const ix = index(css);
  const out = [];
  for (const m of css.matchAll(valueAt)) {
    if (!ix.keep[m.index]) continue;
    out.push({
      value: (m.groups?.v ?? m[0]).trim().replace(/\s+/g, " "),
      module: ix.moduleNames[ix.moduleAt[m.index]],
      line: ix.lineAt[m.index] - ix.moduleFirstLine[ix.moduleAt[m.index]],
    });
  }
  return out;
}

const LENGTH = String.raw`(?:0|[-+]?\d*\.?\d+)(?:px|rem|em|pt|%)?`;
// The value group MUST be named `v`: an unnamed group leaves `m.groups.v`
// undefined, the collector falls back to the whole `prop: value` match, every
// length parse fails, and the axis reports "0 off-scale" for free. That is the
// vacuous-pass shape this audit exists to catch, and it shipped in the first
// draft of this file.
const propRegex = (props) =>
  new RegExp(String.raw`\b(?:${props})\s*:\s*(?<v>[^;{}]+)`, "g");

/** One length in px, or null when the value is not a single length. */
function toPx(v, rootPx = ROOT_FONT_PX) {
  const m = v.trim().match(new RegExp(String.raw`^(${LENGTH})$`));
  if (!m) return null;
  const t = m[1];
  if (t === "0") return 0;
  if (t.endsWith("rem")) return parseFloat(t) * rootPx;
  if (t.endsWith("em")) return null; // relative to the parent's own font-size
  if (t.endsWith("pt")) return (parseFloat(t) * 96) / 72;
  if (t.endsWith("%")) return null; // relative to the containing block
  if (t.endsWith("px")) return parseFloat(t);
  return parseFloat(t); // unitless, only meaningful for line-height/font-size
}

/** Durations in ms from a `transition`/`animation` shorthand. */
function durationsMs(value) {
  return [...value.matchAll(/(\d*\.?\d+)(ms|s)\b/g)].map((m) =>
    parseFloat(m[1]) * (m[2] === "s" ? 1000 : 1));
}

const groupByValue = (rows) => {
  const by = new Map();
  for (const r of rows) {
    if (!by.has(r.value)) by.set(r.value, []);
    by.get(r.value).push(`${r.module}:${r.line}`);
  }
  return [...by.entries()].sort((a, b) => b[1].length - a[1].length || a[0].localeCompare(b[0]));
};

// ---------------------------------------------------------------------------

const palette = readPalette();
const selfTestFailures = selfTest();
/**
 * Reducible sites that are NOT substituted, each with the reason. An entry here
 * keeps the gate honest in both directions: the gate still fires on any other
 * reducible colour, and the exception is printed in the report rather than
 * quietly tolerated. An empty list would mean the bar was met outright.
 */
const DECLINED = new Map([
  ["002-pvr.css:597",
    "The literal is #9146FF at 18% — the OFFICIAL Twitch hex, one channel off " +
    ":root --twitch (#9146ff is rgb 145,70,255; the literal says 145,71,255). " +
    "Substituting would entrench the official hex at a new site while " +
    "DESIGN.md:137-142 says the desaturated --cc-accent set is the shipped " +
    "source of truth and the :root block still holds the official one. Todo 8 " +
    "escalated that contradiction as an open question for the orchestrator; " +
    "this audit does not decide it. The sibling rows in the same rule " +
    "(.mon-plat.plat-youtube/.patreon) are NOT --youtube/--patreon either, so " +
    "substituting one of three would make the set less consistent, not more."],
]);

const strict = process.argv.includes("--strict");
const failures = [];

if (!palette.blockFound) {
  console.error("FAIL: no `:root` block in the assembled PVR sheet — the palette is empty, so every check below would pass for free.");
  process.exit(1);
}
if (selfTestFailures.length) {
  for (const f of selfTestFailures) console.error(`FAIL: ${f}`);
  console.error("FAIL: the colour resolver's own self-test failed; its output cannot be trusted.");
  process.exit(1);
}

console.log("=".repeat(72));
console.log("DESIGN.md conformance audit — assembled SPA stylesheet, both editions");
console.log("=".repeat(72));
console.log(
  `palette: ${palette.tokens.length} colour tokens read from the assembled :root block`,
);
console.log("resolver self-test: PASS (11 equal-pairs, 6 near-miss pairs, alpha + tolerance boundaries)");

const editions = [
  ["pvr", false],
  ["creator", true],
];
const report = [];

for (const [label, creatorEnabled] of editions) {
  const css = assemble(creatorEnabled);
  const { colours, defs, uses } = collect(css);

  // ---- classification ------------------------------------------------------
  const rows = colours.map((c) => {
    const rgb = resolve(c.value);
    const site = `${c.module}:${c.line}`;
    // A colour expression the resolver cannot read is not automatically a
    // defect: `hsl(var(--ch-hue, 285deg), 46%, 56%)` takes its hue from a
    // custom property, so the colour IS token-derived and there is no literal
    // to reconcile. A literal the resolver genuinely cannot read is a defect in
    // the audit itself and fails.
    if (!rgb) {
      return {
        ...c, site, rgb: null,
        klass: c.value.includes("var(") ? "TOKEN-DERIVED" : "UNPARSED",
      };
    }
    const platform = PLATFORM.find(([, hex]) => near(rgb.slice(0, 3), resolve(hex).slice(0, 3)));
    if (platform) return { ...c, site, rgb, klass: "PLATFORM", token: platform[0] };
    // Prefer a substitutable token; a carrier of the same colour is only
    // acceptable when nothing else matches, and then only for attribution.
    const exact =
      palette.tokens.find((t) => t.substitutable && near(rgb.slice(0, 3), t.rgb.slice(0, 3))) ??
      palette.tokens.find((t) => !t.substitutable && near(rgb.slice(0, 3), t.rgb.slice(0, 3)));
    if (exact) {
      // Matching on the RGB triple is NOT sufficient, and getting this wrong
      // twice is what the first two versions of this classifier did:
      //   1. `--border` is white at 12% alpha, so a bare opaque `#fff` shares
      //      its colour triple and `var(--border)` would render a 12% white
      //      border where an opaque one was written.
      //   2. every `rgba(255,255,255,a)` wash then "matched" `--border` on the
      //      triple and wanted `color-mix(in srgb, var(--border) 750%,
      //      transparent)` — exact on paper, and nonsense in a stylesheet:
      //      a 12% hairline is not the base of a 90% wash. The token table has
      //      no wash scale, which is a gap to report, not a substitution.
      // So reducible means the token's OWN value is the literal's value: equal
      // alpha, or the literal is a translucent form of an OPAQUE token — the
      // one case where `color-mix()` is a genuinely readable expression, and
      // the idiom the stylesheet already uses at 000-pvr.css:677.
      const la = rgb[3];
      const ta = exact.rgb[3];
      // A carrier is never substitutable, so the whole block is skipped and
      // the value falls through to near-miss / untokenised. The colour may be
      // the shadow's, but no token names it on its own.
      if (exact.substitutable) {
        // A match that needed the 1.5/255 tolerance is not bit-exact, and the
        // report says so rather than presenting a sub-perceptual shift as a
        // zero-change substitution.
        // The alpha ratio is exact by construction (mixing an opaque token at
        // p% with transparent yields that colour at p alpha), so bit-exactness
        // is a question about the colour channels alone.
        const klass = exactRgb(rgb.slice(0, 3), exact.rgb.slice(0, 3))
          ? "REDUCIBLE" : "REDUCIBLE-TOLERANT";
        if (la === 1 && ta === 1) {
          return { ...c, site, rgb, klass, token: exact.name, substitution: `var(${exact.name})` };
        }
        if (la < 1 && ta === 1) {
          const pct = Math.round(la * 10000) / 100;
          return {
            ...c, site, rgb, klass, token: exact.name, pct,
            substitution: `color-mix(in srgb, var(${exact.name}) ${pct}%, transparent)`,
          };
        }
        if (la === ta) {
          return { ...c, site, rgb, klass, token: exact.name, substitution: `var(${exact.name})` };
        }
      }
      // Same colour triple, different effective alpha. Falls through to the
      // near-miss / untokenised branches, which is where a wash or a scrim
      // belongs.
    }
    // Near-miss: same hue family as a token. Achromatic literals have no hue,
    // so they are matched by lightness against --fg (light) and the black
    // scrim ladder (dark).
    const h = hueOf(rgb.slice(0, 3));
    const nearMiss = achromatic(rgb.slice(0, 3))
      ? (rgb[0] > 128 ? palette.tokens.find((t) => t.name === "--fg") : null)
      : palette.tokens
          .filter((t) => hueOf(t.rgb.slice(0, 3)) != null)
          .map((t) => ({ t, d: hueDelta(h, hueOf(t.rgb.slice(0, 3))) }))
          .filter((x) => x.d <= 15)
          .sort((a, b) => a.d - b.d)[0]?.t;
    return { ...c, site, rgb, klass: nearMiss ? "NEAR-MISS" : "UNTOKENISED", token: nearMiss?.name };
  });

  const count = (k) => rows.filter((r) => r.klass === k).length;
  const declined = rows.filter(
    (r) => (r.klass === "REDUCIBLE" || r.klass === "REDUCIBLE-TOLERANT") && DECLINED.has(r.site),
  );
  const reducible = rows.filter(
    (r) => (r.klass === "REDUCIBLE" || r.klass === "REDUCIBLE-TOLERANT") && !DECLINED.has(r.site),
  );
  const nonPlatform = rows.filter((r) => r.klass !== "PLATFORM");
  if (reducible.length) {
    failures.push(
      `${label}: ${reducible.length} hardcoded colour(s) duplicate a :root token — ` +
        reducible.map((r) => `${r.site} ${r.value}==${r.token}`).join("; "),
    );
  }

  // ---- the four non-colour axes -------------------------------------------
  const fontSizes = declarations(css, propRegex("font-size"));
  const spacings = declarations(css, propRegex("padding|margin|gap|row-gap|column-gap"));
  const radii = declarations(css, propRegex("border-radius|border-top-left-radius|border-top-right-radius|border-bottom-left-radius|border-bottom-right-radius"));
  const transitions = declarations(css, propRegex("transition|transition-[a-z-]+"));
  const animations = declarations(css, propRegex("animation|animation-[a-z-]+"));
  const easings = declarations(css, /((?:cubic-bezier\([^)]*\))|(?:ease-in-out|ease-in-out|ease-in|ease-out|linear|ease))(?=[;\s])/g);

  const typeOff = [];
  for (const r of fontSizes) {
    if (/var\(|calc\(|clamp\(|min\(|max\(/.test(r.value)) continue;
    const px = toPx(r.value);
    if (px == null) continue;
    if (!nearestStep(px, TYPE_SCALE)) {
      typeOff.push({ ...r, px });
    }
  }
  const spacingOff = [];
  for (const r of spacings) {
    for (const part of r.value.split(/\s+/)) {
      if (/var\(|calc\(|clamp\(|auto|inherit|min\(|max\(/.test(part)) continue;
      const px = toPx(part);
      if (px == null) continue;
      if (ZERO_STEPS.spacing && px === 0) continue;
      if (!nearestStep(px, SPACING_SCALE)) {
        spacingOff.push({ ...r, value: part, px });
        break;
      }
    }
  }
  const radiusOff = [];
  for (const r of radii) {
    for (const part of r.value.split(/\s+/)) {
      if (/var\(|inherit|initial/.test(part)) continue;
      const px = toPx(part);
      if (px == null) continue; // 50% is a pill, not a scale step
      if (ZERO_STEPS.radius && px === 0) continue;
      if (!nearestStep(px, RADIUS_SCALE)) {
        radiusOff.push({ ...r, value: part, px });
        break;
      }
    }
  }
  const motionSites = [
    ...transitions.map((r) => ({ ...r, kind: "transition" })),
    ...animations.map((r) => ({ ...r, kind: "animation" })),
  ];
  const motionOff = [];
  for (const r of motionSites) {
    for (const ms of durationsMs(r.value)) {
      if (!nearestStep(ms, MOTION_STEPS)) motionOff.push({ ...r, ms });
    }
  }
  const recDot = motionSites.filter((r) => /pulse/.test(r.value));

  report.push({ label, rows, count, reducible, declined, nonPlatform, fontSizes, typeOff, spacings, spacingOff, radii, radiusOff, motionSites, motionOff, easings, recDot, defs, uses });
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

for (const ed of report) {
  const c = ed.count;
  console.log(`\n${"=".repeat(72)}\nEDITION ${ed.label}\n${"=".repeat(72)}`);

  console.log("\n-- COLOUR (the gate) " + "-".repeat(51));
  console.log(`  hardcoded literals outside :root, not inside a var(--x, …) fallback : ${ed.rows.length}`);
  console.log(`    PLATFORM     (canonical, DESIGN.md:130-142, exempt + guarded) : ${c("PLATFORM")}`);
  console.log(`    REDUCIBLE    (a :root colour, BIT-EXACT; substituting is invisible): ${c("REDUCIBLE")}
    TOLERANT     (a :root colour within 1.5/255; see the site list)     : ${c("REDUCIBLE-TOLERANT")}`);
  console.log(`    TOKEN-DERIVED (hue/colour comes from a var(); nothing to reconcile) : ${c("TOKEN-DERIVED")}`);
  console.log(`    NEAR-MISS    (same hue as a token; adopting it CHANGES colour) : ${c("NEAR-MISS")}`);
  console.log(`    UNTOKENISED  (no token describes it; needs a new token)          : ${c("UNTOKENISED")}`);
  if (c("UNPARSED")) {
    console.log(`    UNPARSED (resolver could not read a literal it should have)   : ${c("UNPARSED")}`);
    for (const r of rows.filter((x) => x.klass === "UNPARSED")) {
      console.log(`      ${r.site}\t${r.value}`);
    }
    failures.push(`${label}: ${c("UNPARSED")} colour literal(s) the resolver could not read`);
  }
  console.log(`  GATE: reducible hardcoded colours = ${ed.reducible.length} (bar: 0), plus ${ed.declined.length} documented exception(s)`);
  for (const d of ed.declined) {
    console.log(`    DECLINED ${d.site}\t${d.value}  == ${d.token}  ->  ${d.substitution}`);
    console.log(`              why not: ${DECLINED.get(d.site)}`);
  }
  if (ed.reducible.length) {
    for (const r of ed.reducible) {
      console.log(`    ${r.site}\t${r.value}  == ${r.token}${r.klass === "REDUCIBLE-TOLERANT" ? " (within 1.5/255, not bit-exact)" : ""}  ->  ${r.substitution}`);
    }
  }
  console.log(`  --strict would additionally fail on ${ed.nonPlatform.filter((r) => !r.klass.startsWith("REDUCIBLE") || DECLINED.has(r.site)).length} remaining non-platform literals`);

  for (const [klass, label] of [
    ["PLATFORM", "platform (canonical — listed, not deviations)"],
    ["TOKEN-DERIVED", "token-derived (a var() supplies the colour — no literal to reconcile)"],
    ["NEAR-MISS", "near-miss of a token (proposed mapping)"],
    ["UNTOKENISED", "untokenised (proposed new token)"],
  ]) {
    const set = ed.rows.filter((r) => r.klass === klass);
    if (!set.length) continue;
    console.log(`\n  ${label} — ${set.length} sites`);
    for (const [value, sites] of groupByValue(set)) {
      const sample = set.find((r) => r.value === value);
      const mapping = klass === "PLATFORM"
        ? `--cc-${sample.token} (fixed)`
        : klass === "NEAR-MISS"
          ? `${sample.token} (delta lightness/hue — look change)`
          : proposedToken(value, sample.rgb);
      console.log(`    x${String(sites.length).padStart(3)}  ${value.padEnd(30)} ${mapping}`);
      console.log(`         ${sites.slice(0, 6).join(", ")}${sites.length > 6 ? ` … +${sites.length - 6}` : ""}`);
    }
  }

  console.log(`\n-- TYPE (DESIGN.md:72-84) ${"-".repeat(46)}  [reported, not gated]`);
  console.log(`  font-size declarations: ${ed.fontSizes.length}`);
  console.log(`  on the documented scale: ${ed.fontSizes.length - ed.typeOff.length}`);
  console.log(`  OFF the scale: ${ed.typeOff.length} sites, ${new Set(ed.typeOff.map((r) => r.value)).size} distinct values`);
  // DESIGN.md states the scale in px with a `rem` column computed at a 16px
  // root; 000-pvr.css:113 sets html/body to 14px, so every rem here renders
  // 12.5% smaller than that column says. Recomputing the off-scale values at a
  // 16px root separates "the author used a scale step and the root size
  // shrinks it" from "the author used a value the scale does not contain".
  const typeAt16 = ed.typeOff.filter((r) => nearestStep((r.px * 16) / ROOT_FONT_PX, TYPE_SCALE));
  console.log(`    … of which ${typeAt16.length} ARE a documented step at a 16px root (0.875rem -> 14px "Body SM" there, ${toPx("0.875rem").toFixed(2)}px here): the root size, not the scale`);
  console.log(`    … and ${ed.typeOff.length - typeAt16.length} are values the scale does not contain at any root size`);
  for (const [value, sites] of groupByValue(ed.typeOff).slice(0, 12)) {
    console.log(`    x${String(sites.length).padStart(3)}  ${value.padEnd(12)} = ${toPx(value).toFixed(2)}px  (no scale step within 0.51px)  e.g. ${sites[0]}`);
  }
  if (ed.typeOff.length > 12) console.log(`         … ${new Set(ed.typeOff.map((r) => r.value)).size - 12} more distinct values`);

  console.log(`\n-- SPACING (DESIGN.md:172-186) ${"-".repeat(42)}  [reported, not gated]`);
  console.log(`  padding/margin/gap declarations: ${ed.spacings.length}`);
  console.log(`  on the 4/8/12/16/24/32/48/64 ladder: ${ed.spacings.length - ed.spacingOff.length}`);
  console.log(`  OFF the ladder: ${ed.spacingOff.length} sites, ${new Set(ed.spacingOff.map((r) => r.value)).size} distinct values`);
  const spacingAt16 = ed.spacingOff.filter((r) => nearestStep((r.px * 16) / ROOT_FONT_PX, SPACING_SCALE));
  console.log(`    … of which ${spacingAt16.length} ARE a documented step at a 16px root (1rem -> 16px "md" there, 14.00px here): the root size, not the ladder`);
  console.log(`    … and ${ed.spacingOff.length - spacingAt16.length} are values the ladder does not contain at any root size`);
  const spaceTokens = [...ed.uses.map((u) => u.name)].filter((n) => n.startsWith("--space-"));
  console.log(`  --space-* tokens consumed: ${spaceTokens.length} sites of ${ed.spacings.length} spacing declarations (${((spaceTokens.length / ed.spacings.length) * 100).toFixed(1)}%) — the ladder exists and is barely used`);
  for (const [value, sites] of groupByValue(ed.spacingOff).slice(0, 8)) {
    console.log(`    x${String(sites.length).padStart(3)}  ${value.padEnd(12)} = ${toPx(value).toFixed(2)}px  e.g. ${sites[0]}`);
  }

  console.log(`\n-- RADII (DESIGN.md:42, 192-199) ${"-".repeat(41)}  [reported, not gated]`);
  console.log(`  border-radius declarations: ${ed.radii.length}`);
  console.log(`  on a documented step (4/8/10/12/20/9999 + 50%): ${ed.radii.length - ed.radiusOff.length}`);
  console.log(`  OFF every step in BOTH radii tables: ${ed.radiusOff.length} sites, ${new Set(ed.radiusOff.map((r) => r.value)).size} distinct values`);
  for (const [value, sites] of groupByValue(ed.radiusOff)) {
    console.log(`    x${String(sites.length).padStart(3)}  ${value.padEnd(10)} e.g. ${sites.slice(0, 3).join(", ")}`);
  }

  console.log(`\n-- MOTION (DESIGN.md:201-219) ${"-".repeat(43)}  [reported, not gated]`);
  console.log(`  transition/animation shorthands: ${ed.motionSites.length}`);
  const msSeen = new Map();
  for (const r of ed.motionSites) {
    for (const ms of durationsMs(r.value)) {
      if (!msSeen.has(ms)) msSeen.set(ms, []);
      msSeen.get(ms).push(`${r.kind} ${r.module}:${r.line}`);
    }
  }
  const msRows = [...msSeen.entries()].sort((a, b) => a[0] - b[0]);
  for (const [ms, sites] of msRows) {
    const step = nearestStep(ms, MOTION_STEPS);
    console.log(`    x${String(sites.length).padStart(3)}  ${String(ms).padStart(5)}ms  ${step ? `= ${step.name}` : "OFF the duration table"}  e.g. ${sites[0]}`);
  }
  const enterEasings = ed.easings.filter((e) => e.value.replace(/\s+/g, "") === "cubic-bezier(0.16,1,0.3,1)");
  console.log(`  DESIGN.md:209 enter easing cubic-bezier(0.16, 1, 0.3, 1) used: ${enterEasings.length} sites`);
  console.log(`  easing keywords in use: ${[...new Set(ed.easings.map((e) => e.value))].filter((v) => !v.startsWith("cubic-bezier")).sort().join(", ") || "none"}`);
  console.log(`  REC dot pulse (DESIGN.md:204, specified ${REC_DOT_PULSE_MS}ms): declared as ${[...new Set(ed.recDot.map((r) => durationsMs(r.value).join("/")))].join(", ") || "no pulse found"}`);
  const motionTokens = [...ed.uses.map((u) => u.name)].filter((n) => /^--(dur|ease|motion|t)-/.test(n));
  console.log(`  motion custom properties consumed: ${motionTokens.length ? motionTokens.join(", ") : "NONE — the duration and easing tokens are not wired at all"}`);
}

/** A name for a value the table does not describe, derived from its own ladder. */
function proposedToken(value, rgb) {
  if (!rgb) return "(unreadable literal)";
  const [r, g, b] = rgb.slice(0, 3);
  const a = rgb[3];
  const alphaPct = Math.round(a * 100);
  if (r === g && g === b) {
    return r === 0
      ? `black scrim — a --scrim-* step (no token: DESIGN.md stops at --surface/--overlay/--border)`
      : r === 255
        ? `white wash — a --wash-* step; also a near-miss of --fg (adopting it CHANGES the colour)`
        : `grey step ${r}`;
  }
  const h = Math.round(hueOf(rgb.slice(0, 3)));
  return `hue ${h}deg at ${alphaPct}% — needs a --tint-${h}-${alphaPct} token; a second palette, not the token table`;
}

// ---- the platform guard (DESIGN.md:130-142) --------------------------------
console.log(`\n${"=".repeat(72)}\nPLATFORM COLOUR GUARD (DESIGN.md:130-142, fixed — not themeable)\n${"=".repeat(72)}`);
const ccFile = readFileSync(join(repo, "crates/strivo-web/assets/spa-css/020-pvr.css"), "utf8");
for (const [platform, hex] of PLATFORM) {
  const re = new RegExp(`--cc-accent:\\s*${hex}\\s*;`, "i");
  const found = re.test(ccFile);
  if (!found) failures.push(`platform guard: ${platform} ${hex} is missing or was altered in 020-pvr.css`);
  console.log(`  ${platform.padEnd(8)} ${hex}  in .chat-compose-bar[data-platform] : ${found ? "present, unchanged" : "MISSING or ALTERED — the fixed platform block was modified"}`);
}
const rootTw = /--twitch:\s*#9146ff;\s*--youtube:\s*#ff0000;\s*--patreon:\s*#ff424d;/.test(
  readFileSync(join(repo, "crates/strivo-web/assets/spa-css/000-pvr.css"), "utf8").replace(/\s+/g, " ").replace(/(--twitch:[^;]*;)/, "$1 "),
);
console.log(`  :root --twitch/--youtube/--patreon (the official, saturated set — todo 8's open question, untouched) : ${rootTw ? "unchanged" : "ALTERED"}`);

console.log(`\n${"=".repeat(72)}`);
for (const ed of report) {
  const remaining = ed.nonPlatform.filter((r) => !r.klass.startsWith("REDUCIBLE") || DECLINED.has(r.site)).length;
  console.log(
    `${ed.label.padEnd(8)} hardcoded colours outside :root and outside a var(--x, …) fallback: ${ed.rows.length}` +
      ` = ${ed.reducible.length} gated + ${ed.declined.length} declined + ${remaining - ed.declined.length} the token table does not name` +
      ` + ${ed.count("PLATFORM")} canonical platform`,
  );
  console.log(`         gated (bar 0): ${ed.reducible.length}   documented exception: ${ed.declined.length}   near-miss (look change): ${ed.count("NEAR-MISS")}   untokenised (needs a new token): ${ed.count("UNTOKENISED")}   token-derived: ${ed.count("TOKEN-DERIVED")}   platform (exempt): ${ed.count("PLATFORM")}`);
}
console.log("=".repeat(72));

if (strict) {
  const remaining = report.reduce((n, ed) => n + ed.nonPlatform.filter((r) => !r.klass.startsWith("REDUCIBLE") || DECLINED.has(r.site)).length, 0);
  if (remaining) {
    console.error(`\nFAIL(--strict): ${remaining} non-platform hardcoded colours remain outside :root.`);
    process.exit(1);
  }
}

if (failures.length) {
  console.error("\nFAIL:");
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log("\nOK: no hardcoded colour duplicates a :root palette token; the fixed platform block is intact.");
console.log("NOTE: a green run is not a claim that the SPA uses DESIGN.md's type, spacing,");
console.log("      radii or motion scales. The four reports above say how far off it is.");
