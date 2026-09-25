#!/usr/bin/env node
// Design-token audit for the assembled SPA stylesheet.
//
// `crates/strivo-web/build.rs` assembles `assets/spa.css` by concatenating the
// ordered modules under `assets/spa-css/`, including the `-creator.css` ones
// only when the `creator` Cargo feature is on (ADR 0002). A review cannot read
// "the token layer" off a single file, because no such file exists in source —
// and both editions have to be checked, since a Creator-only module may consume
// a token only a PVR module defines (or the reverse).
//
// This script reproduces the assembly for each edition, then reports every
// `var(--x)` consumption that no `--x:` definition can satisfy.
//
// Three buckets, because they are not the same defect:
//
//   ERROR  consumed with NO fallback and never defined. The declaration is
//          invalid at computed-value time, so the property silently becomes
//          `unset` — a border disappears, a background goes transparent, a
//          colour falls back to inheritance. A real, invisible bug, and the
//          bar is zero.
//   ALIAS  consumed with a fallback and never defined. `DESIGN.md:56` requires
//          consumers to use tokens, and a documented fallback is explicitly
//          not a deviation, so these are reported and left alone.
//   LOCAL  defined, but only inside a scoped rule rather than on `:root`. It
//          resolves for consumers inside that rule and is invalid-at-computed-
//          value-time for consumers outside it. Reported, not failed: whether
//          a given site sits inside or outside that scope is a cascade
//          question, not a grep question.
//
// `--colors` additionally prints the hardcoded-colour census DESIGN.md:56 asks
// for: every literal colour outside `:root`, each annotated with the `:root`
// token whose value it is exactly equal to, if any. A literal sitting in a
// `var(--x, <literal>)` fallback is excluded, because DESIGN.md:56 calls a
// documented fallback a non-deviation — the flag there is the ALIAS bucket
// above, and it already carries the token's name.
import { readdirSync, readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..");
const cssDir = join(repo, "crates/strivo-web/assets/spa-css");
const files = readdirSync(cssDir)
  .filter((f) => f.endsWith(".css"))
  .sort();

// `--x:` inside a `var()` is a consumption, never a definition; `--x: ;` (an
// empty value) is a guaranteed-invalid definition, so require a real value.
const DEF_RE = /(^|[;{\s])(--[A-Za-z0-9_-]+)\s*:\s*[^;\s}]/g;
const USE_RE = /var\(\s*(--[A-Za-z0-9_-]+)\s*([,)])/g;
const COLOUR_RE = /#[0-9a-fA-F]{3,8}\b|hsla?\([^)]*\)|rgba?\([^)]*\)|color-mix\([^;{}]*?\)/g;
const GLOBAL_SELECTOR = /^(:root|:host|html|body|\*)\b/;

/**
 * Offsets covered by the fallback argument of a `var(--x, …)` call, i.e. the
 * literals DESIGN.md:56 exempts. Matching the closing paren by hand is what
 * keeps `var(--x, color-mix(in srgb, var(--y) 8%, transparent))` honest.
 */
/**
 * Index of the `)` closing the `(` at `open`. Counting by hand is what keeps a
 * nested `var(--x, …)` from ending the scan early.
 */
function balancedParenEnd(css, open) {
  let depth = 0;
  for (let i = open; i < css.length; i += 1) {
    if (css[i] === "(") depth += 1;
    else if (css[i] === ")") {
      depth -= 1;
      if (depth === 0) return i;
    }
  }
  return css.length - 1;
}

function fallbackSpans(css) {
  const spans = [];
  for (const m of css.matchAll(/var\(/g)) {
    let i = m.index + 4;
    let depth = 1;
    let comma = -1;
    for (; i < css.length && depth > 0; i++) {
      const c = css[i];
      if (c === "(") depth++;
      else if (c === ")") depth--;
      else if (c === "," && depth === 1 && comma === -1) comma = i;
    }
    if (comma !== -1) spans.push([comma + 1, i - 1]);
  }
  return spans;
}

/** `build.rs` assembly: filename order, `-creator` modules only when enabled. */
function assemble(creatorEnabled) {
  let out = "";
  for (const f of files) {
    if (f.endsWith("-creator.css") && !creatorEnabled) continue;
    out += `\n/* module: ${f} */\n` + readFileSync(join(cssDir, f), "utf8");
  }
  return out;
}

/**
 * The selector list of the rule opening at `open`, and whether any selector in
 * it is a global one. Comments are dropped and anything before the last
 * top-level `;` is discarded, so `@import ...;` and a trailing comment do not
 * become part of the selector.
 */
function prelude(css, open) {
  const raw = css
    .slice(css.lastIndexOf("}", open) + 1, open)
    .replace(/\/\*[\s\S]*?\*\//g, "");
  const sel = raw.slice(raw.lastIndexOf(";") + 1).trim();
  const global = sel.split(",").some((part) => GLOBAL_SELECTOR.test(part.trim()));
  return { sel, global };
}

/**
 * One pass over the sheet: blank comments in place (so every byte offset keeps
 * its meaning) and record, per offset, the originating module, the line, the
 * selector in effect, and whether it sits inside the `:root` palette block.
 */
function index(css) {
  const n = css.length;
  const inRoot = new Uint8Array(n);
  const selectorAt = new Array(n).fill("");
  const moduleAt = new Int32Array(n);
  const lineAt = new Int32Array(n);
  const bounds = [...css.matchAll(/\/\* module: ([^*]+) \*\//g)];
  const moduleNames = bounds.map((m) => m[1].trim());
  // Line number of each module's first byte, so a site is reported as
  // `module:line` a reader can jump to rather than an offset into a
  // 6000-line concatenation of modules that do not exist as a file.
  const lineOf = (offset) => {
    let l = 1;
    for (let k = 0; k < offset; k++) if (css[k] === "\n") l++;
    return l;
  };
  // The marker sits on its own line, so a site on assembled line A inside a
  // module whose marker is on line L is on line A - L of that module file.
  const moduleFirstLine = bounds.map((m) => lineOf(m.index));

  const keep = new Uint8Array(n).fill(1);
  for (const m of css.matchAll(/\/\*[\s\S]*?\*\//g)) {
    keep.fill(0, m.index, m.index + m[0].length);
  }

  let mod = 0;
  let line = 1;
  let depth = 0;
  let rootDepth = -1;
  const stack = [];
  for (let i = 0; i < n; i++) {
    while (mod + 1 < bounds.length && i >= bounds[mod + 1].index) mod++;
    moduleAt[i] = mod;
    lineAt[i] = line;
    if (css[i] === "\n") line++;
    if (!keep[i]) continue;
    if (css[i] === "{") {
      depth++;
      stack.push(prelude(css, i));
      if (rootDepth === -1 && stack[stack.length - 1].global) rootDepth = depth;
    } else if (css[i] === "}") {
      if (rootDepth === depth) rootDepth = -1;
      depth--;
      stack.pop();
    } else {
      const top = stack[stack.length - 1];
      selectorAt[i] = top ? top.sel : "";
      inRoot[i] = rootDepth !== -1 && depth >= rootDepth ? 1 : 0;
    }
  }
  return { keep, inRoot, selectorAt, moduleAt, lineAt, moduleNames, moduleFirstLine };
}

function collect(css) {
  const ix = index(css);
  const defs = new Map();
  const uses = [];
  const colours = [];

  for (const m of css.matchAll(DEF_RE)) {
    if (!ix.keep[m.index]) continue;
    const name = m[2];
    const sel = ix.selectorAt[m.index] ?? "";
    const global = sel === "" || GLOBAL_SELECTOR.test(sel);
    if (!defs.has(name) || (!defs.get(name).global && global)) {
      defs.set(name, { global, selector: sel });
    }
  }
  for (const m of css.matchAll(USE_RE)) {
    if (!ix.keep[m.index]) continue;
    uses.push({
      name: m[1],
      fallback: m[2] === ",",
      line: ix.lineAt[m.index] - ix.moduleFirstLine[ix.moduleAt[m.index]],
      module: ix.moduleNames[ix.moduleAt[m.index]],
      selector: ix.selectorAt[m.index] ?? "",
    });
  }
  const fallbacks = fallbackSpans(css);
  for (const m of css.matchAll(COLOUR_RE)) {
    if (!ix.keep[m.index] || ix.inRoot[m.index]) continue;
    if (fallbacks.some(([a, b]) => m.index >= a && m.index <= b)) continue;
    // `color-mix(in srgb, var(--x) 40%, transparent)` is token-derived, not a
    // hardcoded colour: the whole point of the mix is to reuse --x. Only a mix
    // that carries a literal of its own is a deviation. Without this the
    // census counted its own token hygiene as an offence.
    //
    // COLOUR_RE stops the mix at its FIRST `)`, i.e. inside the nested
    // `var(--x)`, so both the literal test and the reported value have to use
    // the balanced body — otherwise `color-mix(in srgb, var(--a) 50%, #f00)`
    // is judged on `color-mix(in srgb, var(--a)` and slips through, and a real
    // finding is printed without the literal that caused it.
    let value = m[0];
    if (m[0].startsWith("color-mix(")) {
      const open = m.index + "color-mix".length;
      const end = balancedParenEnd(css, open);
      const body = css.slice(open, end + 1);
      if (!/#[0-9a-fA-F]{3,8}\b|hsla?\(/.test(body)) continue;
      // `body` starts at the `(` itself, so do not re-add it.
      value = `color-mix${body}`;
    }
    colours.push({
      value: value.replace(/\s+/g, " "),
      line: ix.lineAt[m.index] - ix.moduleFirstLine[ix.moduleAt[m.index]],
      module: ix.moduleNames[ix.moduleAt[m.index]],
      selector: ix.selectorAt[m.index] ?? "",
    });
  }
  return { defs, uses, colours };
}

const group = (rows) => {
  const by = new Map();
  for (const r of rows) {
    if (!by.has(r.name)) by.set(r.name, []);
    by.get(r.name).push(r);
  }
  return [...by.entries()].sort();
};

const editionReport = [];
let failed = false;

for (const [label, creator] of [
  ["pvr", false],
  ["creator", true],
]) {
  const { defs, uses, colours } = collect(assemble(creator));
  const consumed = new Set(uses.map((u) => u.name));
  const errors = uses.filter((u) => !defs.has(u.name) && !u.fallback);
  const aliases = uses.filter((u) => !defs.has(u.name) && u.fallback);
  const localOnly = [...consumed].filter((n) => defs.has(n) && !defs.get(n).global);
  if (errors.length) failed = true;
  editionReport.push({ label, defs, uses, colours, errors, aliases, localOnly });

  const undefinedNames = new Set([...errors, ...aliases].map((r) => r.name));
  console.log(`\n=== edition: ${label} ===`);
  console.log(
    `consumed ${consumed.size} custom properties against ${defs.size} defined; ` +
      `${consumed.size - undefinedNames.size} resolve to a definition, ` +
      `${undefinedNames.size} consumed names have none`,
  );
  if (!errors.length) {
    console.log("ERROR  undefined consumed token without a fallback: 0");
  } else {
    console.log(
      `ERROR  undefined consumed token without a fallback: ` +
        `${new Set(errors.map((e) => e.name)).size} names / ${errors.length} sites`,
    );
    for (const [name, rows] of group(errors)) {
      console.log(
        `  ${name}  x${rows.length}  ${rows.map((r) => `${r.module}:${r.line}`).join(", ")}`,
      );
    }
  }
  if (aliases.length) {
    console.log(
      `ALIAS  fallback-carried and undefined (allowed, DESIGN.md:56): ` +
        `${new Set(aliases.map((a) => a.name)).size} names / ${aliases.length} sites`,
    );
    for (const [name, rows] of group(aliases)) {
      console.log(
        `  ${name}  x${rows.length}  ${[...new Set(rows.map((r) => r.module))].join(", ")}`,
      );
    }
  }
  if (localOnly.length) {
    console.log(`LOCAL  defined only inside a scoped rule: ${localOnly.length}`);
    for (const n of localOnly.sort()) {
      console.log(`  ${n}  -> ${defs.get(n).selector.split("\n").pop().trim() || "(top level)"}`);
    }
  }
  console.log(`COLOUR hardcoded literals outside :root: ${colours.length} (see --colors)`);
}

if (process.argv.includes("--colors")) {
  // Canonical comparable form of one colour literal, so a literal is only ever
  // matched to a token it is *equal* to: `.5` == `0.5`, `hsl(h,s,l)` ==
  // `hsla(h,s,l,1)`, `#abc` == `#aabbcc`. hsl and rgb literals never match
  // each other — they are different spaces and no conversion is implied.
  const norm = (s) => {
    const t = s
      .toLowerCase()
      .replace(/\s+/g, "")
      .replace(/([,(\/])\.(\d)/g, "$10.$2");
    if (t.startsWith("#")) {
      const h = t.slice(1);
      const full = h.length === 3 ? [...h].map((c) => c + c).join("") : h;
      return "#" + full;
    }
    for (const [fn, tag] of [[/^hsla?\(([^)]*)\)$/, "h"], [/^rgba?\(([^)]*)\)$/, "r"]]) {
      const m = t.match(fn);
      if (m) {
        const p = m[1].split(",");
        return tag + p.slice(0, 3).join(",") + "," + (p[3] ?? "1");
      }
    }
    return t;
  };
  // Only the `:root` block's own literal values form the map: a token defined
  // inside a scoped rule describes that rule, not the palette.
  const valueToToken = new Map();
  const rootCss = assemble(false);
  const rootStart = rootCss.indexOf(":root {");
  const rootBlock = rootCss.slice(rootStart, rootCss.indexOf("\n}", rootStart) + 2);
  for (const m of rootBlock.matchAll(/(--[A-Za-z0-9_-]+)\s*:\s*([^;]+);/g)) {
    const v = m[2].trim();
    if (!/^(#[0-9a-f]{3,8}|hsla?\(|rgba?\()/.test(v)) continue;
    if (!valueToToken.has(norm(v))) valueToToken.set(norm(v), m[1]);
  }
  for (const ed of editionReport) {
    const unmapped = ed.colours.filter((c) => !valueToToken.has(norm(c.value)));
    console.log(`\n=== hardcoded colours outside :root, edition ${ed.label} ===`);
    console.log(`${ed.colours.length} occurrences, ${unmapped.length} with no exact :root token`);
    for (const c of ed.colours) {
      console.log(
        `  ${c.module}:${c.line}\t${c.value}\t${valueToToken.get(norm(c.value)) ?? "—"}`,
      );
    }
  }
}

if (failed) {
  console.error("\nFAIL: an undefined custom property is consumed without a fallback.");
  process.exit(1);
}
console.log("\nOK: no undefined consumed token lacks a fallback.");
