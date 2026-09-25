#!/usr/bin/env node
// Validator for docs/audits/2026-09-25-consolidated/MANIFEST.json.
//
// A register full of confident verdicts is worthless if the ids do not map.
// This asserts four things and fails loudly on any of them:
//   1. every prior item appears exactly once (no duplicate id, no orphan id);
//   2. every file:line resolves to a line that exists inside its file;
//   3. the verdict vocabulary is closed;
//   4. the id namespaces do not collide.
//
// The per-source item counts are re-derived by PARSING THE SOURCE AUDITS at
// run time, not read from the manifest, so a manifest that quietly drops an
// item cannot pass.
//
// Run:  node docs/audits/2026-09-25-consolidated/validate.mjs
// Exit: 0 all assertions pass, 1 otherwise (failures printed to stderr).

import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve, relative, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, "../../..");
const MANIFEST = join(HERE, "MANIFEST.json");

const VERDICTS = new Set(["open", "closed", "superseded", "still-open"]);
const failures = [];
const fail = (m) => failures.push(m);

// ── A1/A2: re-derive the prior item set from the source audits ─────────────
// Parse, do not trust a number. PHASE0 is a markdown table; WS is the JSON
// issues[] array; CE is numbered section headings in the report.

const PHASE0_SRC = "docs/audits/PHASE0-EXPERIENCE-AUDIT-2026-07-30.md";
const WS_SRC = "docs/audits/2026-09-06-web-surface/MANIFEST.json";
const CE_SRC = "docs/audits/2026-09-08-performance-ux/REPORT.md";

const phase0Ids = new Set();
{
  const md = readFileSync(join(REPO, PHASE0_SRC), "utf8");
  for (const line of md.split("\n")) {
    const m = /^\|\s*(F-\d\d)\s*\|/.exec(line);
    if (m) phase0Ids.add(m[1]);
  }
}

const wsIds = new Set();
{
  const json = JSON.parse(readFileSync(join(REPO, WS_SRC), "utf8"));
  for (const i of json.issues) wsIds.add(i.id);
}

const ceIds = new Set();
{
  // Findings are bold section headings: **1. P1 — ...** through **9. P2 — ...**
  const md = readFileSync(join(REPO, CE_SRC), "utf8");
  for (const line of md.split("\n")) {
    const m = /^\*\*(\d)\.\s+[A-Z]\d\s+—/.exec(line);
    if (m) ceIds.add(m[1].padStart(2, "0"));
  }
}

const derived = { PHASE0: phase0Ids, WS: wsIds, CE: ceIds };
const derivedCounts = {
  PHASE0: phase0Ids.size,
  WS: wsIds.size,
  CE: ceIds.size,
};
const derivedTotal = derivedCounts.PHASE0 + derivedCounts.WS + derivedCounts.CE;

// ── Load the register ──────────────────────────────────────────────────────
const reg = JSON.parse(readFileSync(MANIFEST, "utf8"));
const rows = reg.issues;

const claimed = reg.prior_item_counts ?? {};
for (const ns of ["PHASE0", "WS", "CE"]) {
  if (claimed[ns] !== derivedCounts[ns]) {
    fail(`A1 ${ns}: manifest claims ${claimed[ns]} prior items, source audit yields ${derivedCounts[ns]}`);
  }
}

// ── A1: every prior item appears exactly once ──────────────────────────────
const seen = new Map();
for (const r of rows) {
  if (seen.has(r.id)) fail(`A1 duplicate id: ${r.id} appears at rows ${seen.get(r.id)} and later`);
  else seen.set(r.id, rows.indexOf(r));
}

const orphans = [];
for (const [ns, ids] of Object.entries(derived)) {
  for (const id of ids) {
    if (!seen.has(`${ns}/${id}`)) orphans.push(`${ns}/${id}`);
  }
}
if (orphans.length) fail(`A1 prior items absent from the register (${orphans.length}): ${orphans.join(", ")}`);

for (const r of rows) {
  if (r.id.startsWith("DRV-")) continue; // derived rows are appended, not prior items
  const slash = r.id.indexOf("/");
  if (slash < 0) { fail(`A4 unnamespaced id: ${r.id}`); continue; }
  const ns = r.id.slice(0, slash);
  if (!(ns in derived)) { fail(`A4 unknown namespace ${ns} in ${r.id}`); continue; }
  if (!derived[ns].has(r.id.slice(slash + 1))) fail(`A4 ${r.id} names an id that the ${ns} audit never issued`);
}

if (rows.length !== derivedTotal + (reg.derived_item_count ?? 0)) {
  fail(`A1 register has ${rows.length} rows; expected ${derivedTotal} prior + ${reg.derived_item_count} derived`);
}

// ── A2: every file:line resolves inside its file ───────────────────────────
const lineCache = new Map();
const lengthOf = (f) => {
  if (!lineCache.has(f)) {
    if (!existsSync(join(REPO, f))) { lineCache.set(f, -1); return -1; }
    const text = readFileSync(join(REPO, f), "utf8");
    // Trailing newline must not inflate the count.
    lineCache.set(f, text === "" ? 0 : text.replace(/\n$/, "").split("\n").length);
  }
  return lineCache.get(f);
};
for (const r of rows) {
  if (typeof r.line !== "number" || !Number.isInteger(r.line) || r.line < 1) {
    fail(`A2 ${r.id}: line is not a positive integer (${r.line})`);
    continue;
  }
  const len = lengthOf(r.file);
  if (len === -1) fail(`A2 ${r.id}: file does not exist — ${r.file}`);
  else if (r.line > len) fail(`A2 ${r.id}: ${r.file}:${r.line} is out of range (file has ${len} lines)`);
  if (r.loc !== `${r.file}:${r.line}`) fail(`A2 ${r.id}: loc "${r.loc}" disagrees with file/line`);
}

// ── A3: closed verdict vocabulary, and required fields ─────────────────────
const hist = {};
for (const r of rows) {
  if (!VERDICTS.has(r.verdict)) fail(`A3 ${r.id}: verdict "${r.verdict}" is outside the closed vocabulary ${[...VERDICTS].join("|")}`);
  hist[r.verdict] = (hist[r.verdict] ?? 0) + 1;
  for (const f of ["file", "line", "changed", "evidence"]) {
    if (r[f] === undefined || r[f] === null || r[f] === "") fail(`A3 ${r.id}: empty required field "${f}"`);
  }
}
for (const v of VERDICTS) if (hist[v] === undefined) fail(`A3 vocabulary declares "${v}" but no row uses it`);
for (const [v, n] of Object.entries(reg.verdict_histogram ?? {})) {
  if (hist[v] !== n) fail(`A3 verdict_histogram.${v} says ${n}, rows say ${hist[v] ?? 0}`);
}
if (reg.open_count !== (hist.open ?? 0)) fail(`A3 open_count says ${reg.open_count}, rows say ${hist.open ?? 0}`);
if (reg.total_rows !== rows.length) fail(`A3 total_rows says ${reg.total_rows}, array has ${rows.length}`);

// ── A4: namespaces do not collide ──────────────────────────────────────────
// Two audits both issue R01..R07 and one issues a CE01..CE06 series that reads
// like the performance audit's finding 1. A bare id is therefore ambiguous, and
// a validator cannot assert "exactly once" over it. Require that every id
// carries a namespace and that the bare (post-slash) ids of DIFFERENT namespaces
// never both appear.
const byBare = new Map();
for (const r of rows) {
  const bare = r.id.includes("/") ? r.id.slice(r.id.indexOf("/") + 1) : r.id;
  if (!byBare.has(bare)) byBare.set(bare, new Set());
  byBare.get(bare).add(r.id);
}
for (const [bare, owners] of byBare) {
  if (owners.size > 1) fail(`A4 bare id "${bare}" is claimed by ${owners.size} namespaces: ${[...owners].join(", ")}`);
}

// ── report ─────────────────────────────────────────────────────────────────
const out = {
  register: relative(REPO, MANIFEST),
  head: reg.head,
  prior_items_rederived_from_source: derivedCounts,
  prior_items_total: derivedTotal,
  derived_rows: reg.derived_item_count,
  rows: rows.length,
  verdict_histogram: hist,
  open_count: hist.open ?? 0,
  still_open_count: hist["still-open"] ?? 0,
  unacted_remaining: (hist.open ?? 0) + (hist["still-open"] ?? 0),
  assertions: ["A1 exactly-once", "A2 file:line resolves", "A3 closed vocabulary + fields", "A4 no ns collision"],
  failures: failures.length,
  result: failures.length ? "FAIL" : "PASS",
};
console.log(JSON.stringify(out, null, 2));
if (failures.length) {
  console.error("\nVALIDATION FAILED:");
  for (const f of failures) console.error("  - " + f);
  process.exit(1);
}
