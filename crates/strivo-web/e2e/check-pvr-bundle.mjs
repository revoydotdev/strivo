#!/usr/bin/env node
// S10 regression guard, runtime-verified version.
//
// build.rs assembles `assets/spa.js` by concatenating the ordered modules
// under `assets/spa/<seq>-<edition>.js`, including the `-creator.js` ones
// only when the `creator` Cargo feature is on (see CE03). It is easy for a
// PVR build to omit a definition (an API method, a render function) while a
// PVR module still *calls* it — the call site then survives into the PVR
// bundle referencing a name that no longer exists, and throws at runtime the
// moment a user reaches it. That's exactly the S10 defect, just with the
// split-module mechanism instead of line-marker deletion.
//
// A source-level review cannot catch this: the bug only exists in the
// *emitted* bundle. So this script builds the real PVR artifact (no
// --features creator) into a scratch target dir, reads the actual
// out/assets/spa.js the build produced, and fails if any surviving call
// site references an identifier that only ever appears in a `-creator.js`
// source module.
//
// Wired as `pretest` in package.json so `npm test` always runs this before
// Playwright.
import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync, mkdtempSync, mkdirSync, rmSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const crateDir = join(here, ".."); // crates/strivo-web
const spaModuleDir = join(crateDir, "assets/spa");
const spaModuleFiles = readdirSync(spaModuleDir)
  .filter((f) => f.endsWith(".js"))
  .sort();

const pvrSource = spaModuleFiles
  .filter((f) => f.endsWith("-pvr.js"))
  .map((f) => readFileSync(join(spaModuleDir, f), "utf8"))
  .join("");
const creatorSource = spaModuleFiles
  .filter((f) => f.endsWith("-creator.js"))
  .map((f) => readFileSync(join(spaModuleDir, f), "utf8"))
  .join("");

function extractDefinedName(line) {
  // Only top-level (column-0) declarations, plus the API object's own
  // methods (its properties sit at exactly 2-space indent in spa.js).
  // Anything more deeply nested is a local helper/option key that just
  // happens to share a name with something else in the file (e.g. a
  // `.filter(...)`/`.trim(...)` builtin, or an unrelated `plugins:` field
  // inside some other object literal) — treating those as "the definition
  // that got stripped" is how this check produces false positives.
  const indent = line.length - line.trimStart().length;
  const t = line.trim();
  if (indent === 0) {
    let m = t.match(/^(?:async\s+)?function\s+([A-Za-z_$][\w$]*)/);
    if (m) return m[1];
    m = t.match(/^(?:const|let)\s+([A-Za-z_$][\w$]*)\s*=/);
    if (m) return m[1];
  }
  if (indent === 2) {
    const m = t.match(/^([A-Za-z_$][\w$]*)\s*:\s*(?:async\s*)?\(/);
    if (m) return m[1];
  }
  return null;
}

function definedNames(source) {
  const names = new Set();
  for (const line of source.split("\n")) {
    const n = extractDefinedName(line);
    if (n) names.add(n);
  }
  return names;
}

function stillDefined(strippedText) {
  const names = new Set();
  for (const line of strippedText.split("\n")) {
    const n = extractDefinedName(line);
    if (n) names.add(n);
  }
  return names;
}

function callSitesFor(strippedText, name) {
  // Word-boundary "name(" or "name (" — covers both bare calls
  // (renderProApp(...)) and API.method(...) calls.
  const callRe = new RegExp(`(?<![\\w$])${name}\\s*\\(`, "g");
  // `typeof name === "function"` guards the call a line or two above it
  // (the codebase's own idiom for a call whose target may not exist in
  // this bundle, e.g. teardownDataviz/teardownArchive/renderPipelines) —
  // that's the fix, not a violation, so skip calls sitting under the guard.
  // The member form `typeof API.name === "function"` guards an API method
  // that only a Creator module defines (e.g. API.pluginList in 028-pvr.js).
  const guardRe = new RegExp(`typeof\\s+(?:API\\.)?${name}\\s*===?\\s*["']function["']`);
  const lines = strippedText.split("\n");
  const hits = [];
  lines.forEach((line, i) => {
    if (!callRe.test(line)) return;
    callRe.lastIndex = 0;
    const windowStart = Math.max(0, i - 3);
    const guarded = lines.slice(windowStart, i + 1).some((l) => guardRe.test(l));
    if (!guarded) hits.push(i + 1);
  });
  return hits;
}

// 1. Build the real PVR (non-creator) bundle into an isolated target dir,
//    so we read exactly one, unambiguous out/assets/spa.js (and spa.css).
//    The scratch dir lives under cargo's own target directory rather than
//    the OS temp directory: a fresh debug build of strivo-web is several GB,
//    and on hosts where /tmp is a small tmpfs (the self-hosted runner
//    included) that overflowed with "Disk quota exceeded". Override with
//    STRIVO_BUNDLE_CHECK_DIR to put it somewhere else.
function cargoTargetDir() {
  const meta = execFileSync(
    "cargo", ["metadata", "--no-deps", "--format-version", "1"],
    { cwd: crateDir, encoding: "utf8" },
  );
  return JSON.parse(meta).target_directory;
}
const scratchBase = process.env.STRIVO_BUNDLE_CHECK_DIR || cargoTargetDir();
mkdirSync(scratchBase, { recursive: true });
const scratch = mkdtempSync(join(scratchBase, "strivo-pvr-bundle-check-"));
let builtSpaJs;
let builtSpaCss;
try {
  execFileSync("cargo", ["build", "-p", "strivo-web"], {
    cwd: crateDir,
    env: { ...process.env, CARGO_TARGET_DIR: scratch },
    stdio: "inherit",
  });
  const out = execFileSync(
    "find", [join(scratch, "debug/build"), "-maxdepth", "4", "-path", "*/out/assets/spa.js"],
    { encoding: "utf8" },
  ).trim().split("\n").filter(Boolean);
  if (out.length !== 1) {
    throw new Error(`expected exactly one built spa.js, found ${out.length}: ${out.join(", ")}`);
  }
  builtSpaJs = readFileSync(out[0], "utf8");
  builtSpaCss = readFileSync(join(dirname(out[0]), "spa.css"), "utf8");
} finally {
  rmSync(scratch, { recursive: true, force: true });
}

// 2. Compute which identifiers are defined ONLY in a `-creator.js` source
//    module (never in a `-pvr.js` one) — those are the names a PVR build
//    genuinely omits. If a name is defined in both (moved out to shared code
//    on purpose — see S17), it's not actually removed and isn't checked.
const creatorOnlyNames = new Set(
  [...definedNames(creatorSource)].filter((n) => !definedNames(pvrSource).has(n)),
);
const keptDefs = stillDefined(builtSpaJs);

// 3. Any creator-only name with a surviving call site in the REAL built
//    bundle is the S10 bug: a call to a definition that isn't in this
//    edition's bundle.
const violations = [];
for (const name of creatorOnlyNames) {
  if (keptDefs.has(name)) continue;
  const hits = callSitesFor(builtSpaJs, name);
  if (hits.length) violations.push({ name, hits });
}

if (violations.length) {
  console.error("PVR bundle (real `cargo build -p strivo-web` output) calls " +
    "identifiers whose definitions live only in a Creator spa module:");
  for (const v of violations) {
    console.error(`  ${v.name} — called at out/assets/spa.js:${v.hits.join(", ")}`);
  }
  console.error(
    "\nMove the call site into a `-creator.js` module too (or its enclosing " +
    "dead function), or move the definition into a `-pvr.js` module if the " +
    "call site is genuinely reachable in the PVR build.",
  );
  process.exit(1);
}

// 4. CE03's acceptance bar, checked directly against the artifact: the PVR
//    bundle contains ZERO occurrences (not just call sites — any reference,
//    including a leaked definition) of a name that only ever exists in
//    Creator source. This is the literal "contains zero Creator symbols"
//    check, independent of the call/definition heuristics above.
//
// One narrow, deliberate exception: shared route/teardown code guards a call
// to a Creator-only render/teardown function with
// `typeof <name> === "function"` so the same PVR module works whether or not
// that function exists in this bundle (the same idiom `callSitesFor` above
// already treats as the fix, not the bug, for S10). The guard's own line
// necessarily spells the name it's checking for — that's not the function
// shipping, it's the PVR code correctly staying silent about its absence.
// Any occurrence NOT on a guard line, or a call that isn't inside a guarded
// window, is a real leak. A full-line `//` comment mentioning the name (e.g.
// explaining why a nearby call is guarded) ships inert bytes, not a symbol —
// excluded the same way.
const symbolLeaks = [];
for (const name of creatorOnlyNames) {
  const wordRe = new RegExp(`(?<![\\w$])${name}(?![\\w$])`, "g");
  const guardRe = new RegExp(`typeof\\s+(?:API\\.)?${name}\\s*===?\\s*["']function["']`);
  const callRe = new RegExp(`(?<![\\w$])${name}\\s*\\(`);
  const lines = builtSpaJs.split("\n");
  let leaks = 0;
  lines.forEach((line, i) => {
    wordRe.lastIndex = 0;
    const hits = (line.match(wordRe) || []).length;
    if (hits === 0) return;
    if (line.trim().startsWith("//")) return;
    if (guardRe.test(line)) {
      // The guard line itself: fine. A call on the SAME line beyond the
      // guard (not the `if (... ) name();` idiom split across two lines)
      // still counts every other occurrence as a leak.
      leaks += hits - 1;
      return;
    }
    const windowStart = Math.max(0, i - 3);
    const guarded = lines.slice(windowStart, i + 1).some((l) => guardRe.test(l));
    if (guarded && callRe.test(line)) return;
    leaks += hits;
  });
  if (leaks > 0) symbolLeaks.push({ name, count: leaks });
}

if (symbolLeaks.length) {
  console.error(
    "PVR bundle contains Creator-only symbols (CE03 acceptance violated):",
  );
  for (const s of symbolLeaks) {
    console.error(`  ${s.name} — ${s.count} occurrence(s)`);
  }
  process.exit(1);
}

console.log(
  `OK — PVR bundle has zero surviving call sites and zero occurrences of ` +
  `Creator-only symbols (checked ${creatorOnlyNames.size} names found only ` +
  `in assets/spa/*-creator.js).`,
);

// 5. Creator product-vocabulary denylist, checked against the actual built
//    artifact (js AND css) — independent of the identifier-based checks
//    above, which only catch names that are *defined* in a `-creator.js`
//    module. A marketing string, a price, or a plugin's display name is
//    never "defined" anywhere in the S10 sense, so it needs its own check.
//
//    Each entry is a case-insensitive substring/regex. A hit is allowed
//    through only if it matches one of the explicit ALLOWLIST entries below
//    — never by a blanket "it's probably fine" — so every exception is
//    named and justified in one place.
const DENYLIST = [
  { term: "crunchr", desc: "Crunchr (transcription plugin) product name" },
  { term: "viewguard", desc: "Viewguard (fraud-scoring plugin) product name" },
  { term: "insights", desc: "Insights (analytics plugin) product name" },
  { term: "upsell", desc: "pro-gate upsell copy" },
  { term: "upgrade", desc: "Strivo Pro upgrade-card copy" },
  { term: "licen[cs]e", desc: "Strivo Pro licence/license copy or API" },
  { term: "\\btrial\\b", desc: "Strivo Pro trial offer" },
  { term: "\\$25", desc: "Strivo Pro one-time-unlock price" },
  { term: "3-day", desc: "Strivo Pro trial length" },
  { term: "marketplace", desc: "plugin marketplace catalog" },
  { term: "progated|pro-gated", desc: "plugin-registry pro-gating marker" },
];

// Explicit, commented allowlist. Each entry documents WHY a denylist hit at
// that exact spot is not a leak — see the inventory in the pvr-invisibility
// remediation report for the full classification.
const ALLOWLIST = [
  // Route-parsing / hash-structure comments: explanatory only, never
  // rendered. E.g. "// #/plugins/crunchr → transcribed-recordings list".
  { pattern: /^\s*(\/\/|\*|\/\*)/, desc: "full-line source comment" },
  // Live-data feature probe: checks the daemon's actual /plugins response
  // for a plugin named "crunchr". A PVR daemon never reports one available
  // (it mounts no crunchr route), so this is dead-but-harmless, and it is
  // an identifier/URL-segment check against runtime data, not shipped copy.
  { pattern: /p\.name === "crunchr"/, desc: "live plugin-availability check, not marketing copy" },
  { pattern: /#\/plugins\/crunchr\/rec\//, desc: "deep-link built only when the check above is true" },
  { pattern: /\bconst crunchr = /, desc: "local variable bound to the live-data check above" },
  { pattern: /showTranscriptHtml = crunchr && crunchr\.available/, desc: "reads the same local variable, not a string literal" },
  // Command-palette / keyboard-help nav row lists: the underlying "Go to
  // Pipelines" / "Go to Plugins" routes already bounce Home in a PVR build
  // (CREATOR_ROUTES, see 008-pvr.js) and are filtered out of the palette at
  // runtime (036-pvr.js's commandList()) — the same "needed by the gating
  // mechanism itself" exception already applied to navItems/CREATOR_ROUTES.
  { pattern: /\["pipelines", "Go to Pipelines"\]/, desc: "filtered command-palette entry" },
  { pattern: /\["plugins", "Go to Plugins"\]/, desc: "filtered command-palette entry" },
];

function isAllowlisted(line) {
  return ALLOWLIST.some((a) => a.pattern.test(line));
}

function scanForDenylist(text, fileLabel) {
  const hits = [];
  const lines = text.split("\n");
  lines.forEach((line, i) => {
    if (isAllowlisted(line)) return;
    for (const { term, desc } of DENYLIST) {
      const re = new RegExp(term, "i");
      if (re.test(line)) {
        hits.push({ file: fileLabel, lineNo: i + 1, term, desc, line: line.trim().slice(0, 160) });
      }
    }
  });
  return hits;
}

const vocabHits = [
  ...scanForDenylist(builtSpaJs, "out/assets/spa.js"),
  ...scanForDenylist(builtSpaCss, "out/assets/spa.css"),
];

if (vocabHits.length) {
  console.error(
    "PVR bundle contains Creator product vocabulary (pvr-invisibility remediation violated):",
  );
  for (const h of vocabHits) {
    console.error(`  ${h.file}:${h.lineNo} — "${h.term}" (${h.desc})`);
    console.error(`    ${h.line}`);
  }
  console.error(
    "\nMove the offending copy into a `-creator.js`/`-creator.css` module, or " +
    "add a justified ALLOWLIST entry in check-pvr-bundle.mjs if this is a " +
    "genuinely legitimate PVR-side occurrence.",
  );
  process.exit(1);
}

console.log(
  `OK — PVR bundle (js + css) has zero un-allowlisted occurrences of ` +
  `Creator product vocabulary (checked ${DENYLIST.length} denylist terms).`,
);
