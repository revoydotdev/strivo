#!/usr/bin/env node
// Validator for docs/audits/2026-09-25-pro-journey/MANIFEST.json.
//
// A register full of confident findings is worthless if the ids do not map,
// if it quietly re-reports something an earlier audit already closed, or if
// a "verified" claim rests on an assertion that cannot fail. This asserts
// nine things and fails loudly on any of them:
//
//   A1  every finding id is well-formed and appears exactly once;
//   A2  every file:line resolves to a line that exists inside its file;
//   A3  the verdict vocabulary is closed;
//   A4  no id collides with any id in the 2026-09-25-consolidated register;
//   A5  every cross_checked_against id EXISTS in that register;
//   A6  every such id's recorded verdict MATCHES that register's actual
//       verdict, so a stale "it was open" claim cannot survive;
//   A7  no finding is cross-checked against a `closed` item without a stated
//       reason it is a different thing  <- the "must not re-report a closed
//       item" acceptance criterion, mechanised rather than asserted;
//   A8  every finding carries a reproduction and names the spec that drives it;
//   A9  every `broken` finding states a kept control, so a broken promise is
//       never the only thing asserted about that behaviour.
//
// A9 is the addition over the PVR sibling's validator (todo 15). This audit's
// whole subject is kept-vs-broken, so a `broken` row with no control cannot
// distinguish "this is broken" from "everything is broken".
//
// Run:  node docs/audits/2026-09-25-pro-journey/validate.mjs
// Exit: 0 all assertions pass, 1 otherwise (failures printed to stderr).

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, "../../..");
const MANIFEST = join(HERE, "MANIFEST.json");
const UPSTREAM = join(REPO, "docs/audits/2026-09-25-consolidated/MANIFEST.json");

const VERDICTS = new Set([
  "confirmed",
  "broken",
  "kept",
  "by-design",
  "unreachable",
  "not-applicable",
]);
const failures = [];
const fail = (m) => failures.push(m);

const manifest = JSON.parse(readFileSync(MANIFEST, "utf8"));
const findings = manifest.findings ?? [];

// ── A1: ids are well-formed and unique ────────────────────────────────────
const seen = new Set();
for (const f of findings) {
  if (!/^PJ-C\d\d$/.test(f.id)) fail(`A1 ${f.id} does not match the PJ-Cnn namespace`);
  if (seen.has(f.id)) fail(`A1 ${f.id} appears more than once`);
  seen.add(f.id);
}
if (findings.length === 0)
  fail("A1 the register contains no findings; a validator that cannot fail is worse than none");

// ── A2: every file:line resolves ─────────────────────────────────────────
const lineCounts = new Map();
for (const f of findings) {
  if (!f.file || typeof f.line !== "number") {
    fail(`A2 ${f.id} has no file:line`);
    continue;
  }
  const abs = join(REPO, f.file);
  let text;
  try {
    text = lineCounts.get(abs) ?? readFileSync(abs, "utf8");
  } catch {
    fail(`A2 ${f.id} names ${f.file}, which does not exist`);
    continue;
  }
  lineCounts.set(abs, text);
  const n = text.split("\n").length;
  if (f.line < 1 || f.line > n) {
    fail(`A2 ${f.id}: ${f.file}:${f.line} is out of range (file has ${n} lines)`);
  }
}

// ── A3: the verdict vocabulary is closed ──────────────────────────────────
for (const f of findings) {
  if (!VERDICTS.has(f.verdict)) {
    fail(`A3 ${f.id} uses verdict "${f.verdict}", which is not in the closed vocabulary`);
  }
}

// ── A4/A5/A6/A7: the cross-check against the 2026-09-25-consolidated register ──
let upstream;
try {
  upstream = JSON.parse(readFileSync(UPSTREAM, "utf8"));
} catch (e) {
  fail(`A4 the consolidated register could not be read: ${e.message}`);
  upstream = { issues: [] };
}
const upstreamById = new Map((upstream.issues ?? []).map((i) => [i.id, i]));

if (upstreamById.size === 0)
  fail("A4 the consolidated register parsed to zero rows; this gate would be vacuous");

for (const f of findings) {
  if (upstreamById.has(f.id)) {
    fail(`A4 ${f.id} collides with an id in the consolidated register; this namespace must be disjoint`);
  }
  const against = f.cross_checked_against ?? [];
  if (against.length === 0) {
    // A finding may legitimately overlap nothing upstream, but it must SAY
    // so — an empty array and an unstated absence are indistinguishable to
    // the next reader, and the acceptance criterion is "zero duplicates",
    // which needs to be a claim someone made, not a gap.
    if (f.cross_check_note === undefined) {
      fail(
        `A7 ${f.id} declares no cross_checked_against entries and no cross_check_note explaining why it overlaps nothing`,
      );
    }
  }
  for (const a of against) {
    const up = upstreamById.get(a.id);
    if (!up) {
      fail(`A5 ${f.id} cross-checks against "${a.id}", which the consolidated register never issued`);
      continue;
    }
    if (a.register_verdict !== up.verdict) {
      fail(
        `A6 ${f.id} records ${a.id} as "${a.register_verdict}" but the consolidated register says "${up.verdict}"`,
      );
    }
    if (up.verdict === "closed") {
      const why = (a.distinct_because ?? "").trim();
      if (why.length < 20) {
        fail(
          `A7 ${f.id} cross-checks against CLOSED item ${a.id} without a stated reason it is a different thing`,
        );
      }
    }
  }
}

// ── A8: every finding carries a reproduction and a spec ───────────────────
for (const f of findings) {
  const repro = (f.repro ?? "").trim();
  if (repro.length < 20) fail(`A8 ${f.id} has no usable reproduction`);
  const spec = (f.spec ?? "").trim();
  if (!spec.includes("tests-real/")) {
    fail(`A8 ${f.id} does not name the Playwright spec that drives it`);
  }
}

// ── A9: a `broken` finding must state its kept control ────────────────────
for (const f of findings) {
  if (f.verdict !== "broken") continue;
  if (typeof f.control !== "string" || f.control.trim().length < 20) {
    fail(
      `A9 ${f.id} is verdict "broken" but states no control; without one, "broken" cannot be told apart from "everything is broken"`,
    );
  }
}

// ── Non-vacuity of the register's own claims ─────────────────────────────
if (!Array.isArray(manifest.kept_promises) || manifest.kept_promises.length === 0) {
  fail("A9 the register records no kept promises; a register with no 'kept' rows cannot support a 'broken' verdict");
}
for (const k of manifest.kept_promises ?? []) {
  if (!["licence", "transcribe", "pipelines", "editor", "publish"].includes(k.path)) {
    fail(`A9 kept promise names journey step "${k.path}", which is not one of the five`);
  }
  if (!["kept", "broken"].includes(k.verdict)) fail(`A9 kept promise verdict "${k.verdict}" is not kept|broken`);
}
if ((manifest.limitations ?? []).length === 0) {
  fail("A9 the register states no limitations; an audit that reports no gaps is not reporting honestly");
}

// ── Report ────────────────────────────────────────────────────────────────
const upstreamClosed = [...upstreamById.values()].filter((i) => i.verdict === "closed");
const checkedAgainstClosed = findings.flatMap((f) =>
  (f.cross_checked_against ?? [])
    .filter((a) => upstreamById.get(a.id)?.verdict === "closed")
    .map((a) => `${f.id} vs ${a.id}`),
);

if (failures.length) {
  for (const f of failures) console.error(`- ${f}`);
  console.error(`\nFAIL: ${failures.length} assertion(s) failed`);
  process.exit(1);
}

console.log("PASS: 0 failures");
console.log(`  findings validated        ${findings.length} (${findings.map((f) => f.id).join(", ")})`);
console.log(
  `  by verdict                ${findings.map((f) => `${f.id}=${f.verdict}`).join(", ")}`,
);
console.log(`  kept promises recorded    ${(manifest.kept_promises ?? []).length}`);
console.log(`  observations recorded     ${(manifest.observations ?? []).length}`);
console.log(`  limitations declared      ${(manifest.limitations ?? []).length}`);
console.log(`  upstream register rows    ${upstreamById.size} (${upstreamClosed.length} closed)`);
console.log(
  `  cross-checked vs closed   ${checkedAgainstClosed.length ? checkedAgainstClosed.join(", ") : "none"}`,
);
console.log("  every cross-checked id exists upstream, its recorded verdict matches, each closed");
console.log("  overlap carries a stated reason, and every 'broken' row names its kept control.");
