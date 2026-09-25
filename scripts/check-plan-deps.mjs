#!/usr/bin/env node
// Dependency bookkeeping for .omo/plans/strivo-review-and-pro-edition.md.
//
// Implement the `### scripts/check-plan-deps.mjs — the contract` section of the
// plan literally. This tool never edits authored data: the matrix `Depends on`
// column, each todo's `Parallelization: Wave N` label, each `Can parallelize with`
// cell, and all todo prose are inputs. It only writes the values the contract
// declares GENERATED: `Blocked by:`, `Blocks:`, the four generated sub-blocks of
// the `### Phase DAG matrix` section, and the `{{...}}` count placeholders.
//
// Modes:
//   --write   regenerate every generated view in place
//   (bare)    assert invariants (a)-(j); exit 1 on any failure
//   --wave    recompute the wave DAG only and print the node table
//
// Three readings the contract leaves open are resolved here and reported by
// `check-plan-deps.mjs --wave` as CONTRACT GAPs rather than hidden:
//   1. The node table's `Phase`, `Todos` and `n` columns have no generating
//      rule in the contract. Rules used, all taken from the plan's own text:
//      Phase 1 = waves 0-4 ("waves 1, 2, 3 and 4 are Phase 1"), `n` = member
//      count, and `Todos` = maximal consecutive runs joined with an en dash.
//      That is the format the authored table already used for the
//      non-contiguous case (`69, 98`), and it is lossless: todo 7 in W4 and
//      todo 103 in W3 cannot be written as `20-28` / `15-19`.
//   2. Invariant (j) says no `{{...}}` survives, but the contract section
//      itself names the placeholders and quotes (j). Substituting there would
//      make the specification describe itself in the past tense and would
//      destroy the spec. Substitution is therefore confined to the rest of the
//      document and (j) is asserted over the rest of the document.
//   3. The adjacency matrix only has a lower triangle under a topological row
//      and column order. Rows and columns are emitted in (layer, wave label)
//      order, which is topological by construction, so the "lower triangle
//      only" claim holds. The authored table's own row order did not.

import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

const PLAN_PATH = resolve(
  process.argv.find((a) => !a.startsWith('-') && a.endsWith('.md')) ??
    '.omo/plans/strivo-review-and-pro-edition.md',
);
const MANIFEST_PATH = 'docs/CREATOR-EDITION-REPORT-2026-09-07.md';
const CONTRACT_HEADING = '### `scripts/check-plan-deps.mjs` — the contract';
const MATRIX_HEADING = '### Dependency matrix';
const MATRIX_HEADER = /^\|\s*Todo\s*\|\s*Depends on\s*\|\s*Can parallelize with\s*\|/;
const GENERATED_BLOCKS = [
  '#### Node table',
  '#### Adjacency matrix',
  '#### Topological layers — the parallel schedule',
  '#### Critical path at todo level',
];
const EMPTY = '—';

// ---------------------------------------------------------------- parsing ---

/**
 * A markdown table cell listing ids, as a set. Handles `a–b` runs, because the
 * generated views collapse consecutive ids and a `Blocked by:` copied from a
 * neighbouring column may be written either way; comparison is on the set, not
 * on the rendering.
 */
function idList(cell) {
  const t = (cell ?? '').trim();
  if (t === '' || t === EMPTY || t === '-') return [];
  const out = [];
  for (const part of t.split(',')) {
    const s = part.trim().replace(/^`|`$/g, '');
    if (s === '' || s === EMPTY || s === '-') continue;
    const run = /^(\d+)\s*[–—]\s*(\d+)$/.exec(s);
    if (run) {
      const [a, b] = [Number(run[1]), Number(run[2])];
      if (a > b) return [];                       // not an id list
      for (let n = a; n <= b; n++) out.push(n);
      continue;
    }
    const n = Number(s);
    if (!Number.isInteger(n)) return [];         // prose, e.g. "all 103 other todos"
    out.push(n);
  }
  return out;
}

function sameSet(a, b) {
  if (a.length !== b.length) return false;
  const s = new Set(b);
  return a.every((x) => s.has(x));
}

/** True when a cell is an id list (or the empty marker), false for prose. */
function isIdListCell(cell) {
  const t = (cell ?? '').trim();
  if (t === '' || t === EMPTY || t === '-') return true;
  for (const part of t.split(',')) {
    const s = part.trim().replace(/^`|`$/g, '');
    if (s === '' || s === EMPTY || s === '-') continue;
    if (/^\d+\s*[–—]\s*\d+$/.test(s)) continue;
    if (Number.isInteger(Number(s))) continue;
    return false;
  }
  return true;
}

function sectionBounds(lines, heading) {
  const start = lines.findIndex((l) => l.trim() === heading);
  if (start < 0) throw new Error(`heading not found: ${heading}`);
  let end = lines.length;
  for (let i = start + 1; i < lines.length; i++) {
    if (/^## /.test(lines[i])) { end = i; break; }
  }
  return [start, end];
}

function parse(text) {
  const lines = text.split('\n');

  // -- authored: the dependency matrix. Anchor on its own heading and its own
  //    header row, and stop at the next `##`. The plan carries several other
  //    tables with numeric first columns; a loose scan picks them up and
  //    manufactures a phantom cycle.
  const [mStart, mEnd] = sectionBounds(lines, MATRIX_HEADING);
  const hdr = lines.slice(mStart, mEnd).findIndex((l) => MATRIX_HEADER.test(l));
  if (hdr < 0) throw new Error(`no '| Todo | Depends on | Can parallelize with |' header under ${MATRIX_HEADING}`);
  const matrix = new Map();
  for (const l of lines.slice(mStart + hdr + 1, mEnd)) {
    const m = l.match(/^\|\s*(\d+)\s*\|\s*([^|]*?)\s*\|\s*([^|]*?)\s*\|\s*$/);
    if (!m) continue;
    const deps = idList(m[2]);
    matrix.set(Number(m[1]), { deps, parallel: idList(m[3]) });
  }
  if (matrix.size === 0) throw new Error('dependency matrix parsed to zero rows');

  // -- authored: the todo bodies. A todo block runs from its checkbox to the
  //    next checkbox, so `Parallelization:` is found wherever multi-line prose
  //    puts it (todos 31 and 43 push it several lines down).
  const [tStart, tEnd] = sectionBounds(lines, '## Todos');
  const todos = new Map();
  const allBlocks = [];
  for (let i = tStart; i < tEnd; i++) {
    const cb = lines[i].match(/^- \[ \] ([0-9]+|F[0-9]+)\./);
    if (!cb) continue;
    let end = tEnd;
    for (let k = i + 1; k < tEnd; k++) if (/^- \[ \] /.test(lines[k])) { end = k; break; }
    const body = lines.slice(i, end);
    const find = (re) => {
      for (let k = 0; k < body.length; k++) { const m = body[k].match(re); if (m) return { m, line: i + k, text: body[k] }; }
      return null;
    };
    const par = find(/^(\s*Parallelization:\s*)(.+?)\s*\|\s*Blocked by:\s*(.*?)\s*\|\s*Blocks:\s*(.*?)\s*$/);
    const block = {
      id: cb[1], startLine: i, endLine: end,
      parLabel: par ? par.m[2].trim() : null,
      blockedByRaw: par ? par.m[3] : null,
      blocksRaw: par ? par.m[4] : null,
      parText: par ? par.text : null, parLine: par ? par.line : -1,
      acceptance: body.filter((l) => /^  Acceptance criteria/.test(l)).length,
      qa: body.filter((l) => /^  QA scenarios/.test(l)).length,
      commit: body.filter((l) => /^  Commit:/.test(l)).length,
      glued: body.filter((l) => /^  References:.*(Acceptance criteria|QA scenarios|Commit:)/.test(l)).length,
    };
    allBlocks.push(block);
    if (/^\d+$/.test(block.id)) todos.set(Number(block.id), block);
  }

  const [cStart, cEnd] = sectionBounds(lines, CONTRACT_HEADING);
  return { lines, matrix, todos, allBlocks, matrixRange: [mStart, mEnd], todoRange: [tStart, tEnd], contractRange: [cStart, cEnd] };
}

// -------------------------------------------------------------- derivation ---

const shortWave = (label) => label.replace(/^Wave /, 'W');

function waveOrder(a, b) {
  const pa = /^Wave (\d+)([a-z]*)$/.exec(a); const pb = /^Wave (\d+)([a-z]*)$/.exec(b);
  if (Number(pa[1]) !== Number(pb[1])) return Number(pa[1]) - Number(pb[1]);
  return pa[2].localeCompare(pb[2]);
}

/** Phase from the plan's own definition: "waves 1, 2, 3 and 4 are Phase 1". */
const phaseOf = (label) => (Number(/^Wave (\d+)/.exec(label)[1]) <= 4 ? 1 : 2);

function derive(p) {
  const errors = [];
  const waveOf = new Map();
  for (const [id, t] of p.todos) {
    if (t.parLabel === null) { errors.push(`todo ${id} has no Parallelization: line`); continue; }
    if (t.parLabel === 'Final wave') continue;
    if (!/^Wave \d+[a-z]*$/.test(t.parLabel)) { errors.push(`todo ${id}: unrecognised wave label '${t.parLabel}'`); continue; }
    waveOf.set(id, t.parLabel);
  }

  const labels = [...new Set(waveOf.values())].sort(waveOrder);
  const members = new Map(labels.map((w) => [w, [...waveOf.entries()].filter(([, v]) => v === w).map(([k]) => k).sort((a, b) => a - b)]));

  // Intra-wave collapse: wave edges are the union of member dependencies with
  // intra-wave edges removed. Required, not cosmetic -- 13 of 16 waves have
  // intra-wave edges, and without the removal (a) and (g) are unsatisfiable.
  const waveDeps = new Map();
  for (const w of labels) {
    const s = new Set();
    for (const m of members.get(w)) {
      for (const d of p.matrix.get(m)?.deps ?? []) {
        const dw = waveOf.get(d);
        if (dw === undefined) { errors.push(`todo ${m} depends on todo ${d}, which has no wave label`); continue; }
        s.add(dw);
      }
    }
    s.delete(w);
    waveDeps.set(w, [...s].sort(waveOrder));
  }

  // layer(W) = 0 when waveDeps(W) is empty, else 1 + max(layer(W'))
  const layer = new Map(); const mark = new Map(); const cycles = [];
  const visit = (w) => {
    if (mark.get(w) === 2) return layer.get(w);
    if (mark.get(w) === 1) { cycles.push(w); return 0; }
    mark.set(w, 1);
    const ds = waveDeps.get(w);
    const l = ds.length === 0 ? 0 : 1 + Math.max(...ds.map(visit));
    mark.set(w, 2); layer.set(w, l); return l;
  };
  for (const w of labels) visit(w);

  // Topological row/column order: layer first, then label. Every edge raises
  // the layer, so this order puts every edge in the lower triangle.
  const order = [...labels].sort((a, b) => (layer.get(a) - layer.get(b)) || waveOrder(a, b));

  const layers = [];
  for (const l of [...new Set(order.map((w) => layer.get(w)))].sort((a, b) => a - b)) {
    const ws = order.filter((w) => layer.get(w) === l);
    const n = ws.reduce((a, w) => a + members.get(w).length, 0);
    const prev = layers[layers.length - 1];
    layers.push({ layer: l, waves: ws, n, running: (prev ? prev.running : 0) + n });
  }

  // Longest todo chain, memoised DFS.
  const memo = new Map();
  const chain = (n) => {
    if (memo.has(n)) return memo.get(n);
    const best = (p.matrix.get(n)?.deps ?? []).reduce((a, d) => (chain(d).length > a.length ? chain(d) : a), []);
    const out = [...best, n];
    memo.set(n, out);
    return out;
  };
  let longest = [];
  for (const id of p.matrix.keys()) { const c = chain(id); if (c.length > longest.length) longest = c; }

  // Blocks(X) = { Y : X in Depends(Y) }
  const blocksOf = new Map();
  for (const id of p.matrix.keys()) blocksOf.set(id, []);
  for (const [id, row] of p.matrix) for (const d of row.deps) if (blocksOf.has(d)) blocksOf.get(d).push(id);
  for (const v of blocksOf.values()) v.sort((a, b) => a - b);

  return {
    errors, waveOf, labels, members, waveDeps, layer, order, layers, longest, blocksOf,
    todoCount: p.matrix.size,
    waveCount: labels.length,
    layerCount: layers.length,
    manifestCount: countManifest(),
  };
}

/**
 * The 76 manifest IDs. `## 9. Delivery manifest` is one section holding ten
 * `| ID |` sub-tables, so the reader walks the whole section rather than
 * stopping at the first table -- stopping early once saw only 10 of 76 and made
 * invariant (i) pass vacuously.
 */
function manifestIds() {
  const lines = readFileSync(MANIFEST_PATH, 'utf8').split('\n');
  const start = lines.findIndex((l) => /^##\s+\d+\.\s+Delivery manifest\s*$/.test(l));
  if (start < 0) throw new Error(`delivery manifest section not found in ${MANIFEST_PATH}`);
  let end = lines.length;
  for (let i = start + 1; i < lines.length; i++) if (/^## /.test(lines[i])) { end = i; break; }
  const ids = new Set();
  for (let i = start + 1; i < end; i++) {
    const m = lines[i].match(/^\|\s*([A-Z]\d{2})\s*\|/);
    if (m) ids.add(m[1]);
  }
  return ids;
}

function countManifest() {
  return manifestIds().size;
}

// --------------------------------------------------------------- rendering ---

/** Maximal consecutive runs joined with an en dash; singletons listed plainly. */
function ranges(ids) {
  if (!ids.length) return EMPTY;
  const s = [...new Set(ids)].sort((a, b) => a - b);
  const out = []; let i = 0;
  while (i < s.length) {
    let j = i;
    while (j + 1 < s.length && s[j + 1] === s[j] + 1) j++;
    out.push(j > i ? `${s[i]}–${s[j]}` : `${s[i]}`);
    i = j + 1;
  }
  return out.join(', ');
}

const nodeTable = (d) => [
  '| Wave | Phase | Todos | n | Depends on | Layer |',
  '|---|---|---|--:|---|--:|',
  ...d.order.map((w) =>
    `| ${shortWave(w)} | ${phaseOf(w)} | ${ranges(d.members.get(w))} | ${d.members.get(w).length} | ${ranges(d.waveDeps.get(w).map(shortWave))} | ${d.layer.get(w)} |`),
].join('\n');

const adjacency = (d) => {
  const w = d.order.map(shortWave);
  const head = `| ↓ depends on / → is depended on by | ${w.join(' | ')} |`;
  const sep = `|---|${w.map(() => ':--:').join('|')}|`;
  const body = d.order.map((row) => {
    const cells = d.order.map((col) => (d.waveDeps.get(row).includes(col) ? 'X' : '·'));
    return `| **${shortWave(row)}** | ${cells.join(' | ')} |`;
  });
  return [head, sep, ...body].join('\n');
};

const layerTable = (d) => [
  '| Layer | Waves (parallel) | Todos | Running total |',
  '|--:|---|--:|--:|',
  ...d.layers.map((l) => `| ${l.layer} | ${l.waves.map(shortWave).join(' · ')} | ${l.n} | ${l.running} |`),
].join('\n');

/** Preserves the authored `>` interpretation note; replaces the measured claim. */
function criticalPath(d, existing) {
  const note = (existing ?? []).filter((l) => l.startsWith('>'));
  const chain = d.longest;
  const tail = [
    '',
    `The longest dependency chain, **${chain.length} todos** — this is the schedule's spine and every slip on it slips the whole plan:`,
    '',
    `\`${chain.join(' → ')}\``,
  ];
  return [...note, ...tail].join('\n');
}

// ------------------------------------------------------------------ write ---

function regenerate(text, p, d) {
  let out = text;

  // 1. per-todo `Blocked by:` and `Blocks:`, leaving the authored
  //    `Parallelization:` label byte-identical.
  let lines = out.split('\n');
  for (const [id, t] of [...p.todos].sort((a, b) => b[1].parLine - a[1].parLine)) {
    if (t.parLine < 0 || t.parLabel === 'Final wave') continue;
    const label = t.parLabel;
    lines[t.parLine] =
      `  Parallelization: ${label} | Blocked by: ${ranges((p.matrix.get(id)?.deps ?? []))} | Blocks: ${ranges(d.blocksOf.get(id) ?? [])}`;
  }
  out = lines.join('\n');

  // 2. the four generated sub-blocks of the Phase DAG matrix section. Each is
  //    located by its own heading (unique in the document) and replaced in
  //    place, so authored narrative between them and the authored
  //    `#### Invariants this matrix proves` table survive untouched.
  lines = out.split('\n');
  for (const heading of GENERATED_BLOCKS) {
    const s = lines.findIndex((l) => l.trim() === heading);
    if (s < 0) throw new Error(`generated block not found: ${heading}`);
    let e = s + 1;
    while (e < lines.length && !/^#### /.test(lines[e])) e++;
    let body;
    if (heading === '#### Node table') body = [heading, '', nodeTable(d)].join('\n');
    else if (heading === '#### Adjacency matrix') body = [heading, '', adjacency(d)].join('\n');
    else if (heading === '#### Topological layers — the parallel schedule') {
      body = [heading, '', 'Everything in a layer may run concurrently; a layer starts only when the one above it is complete.', '', layerTable(d),
        '', `**Critical path: ${d.layerCount} layers** (0→${d.layerCount - 1}). **Widest layer: ${Math.max(...d.layers.map((l) => l.n))} todos** (layer ${d.layers.reduce((a, b) => (b.n > a.n ? b : a)).layer}). Mean parallelism: ${d.todoCount} ÷ ${d.layerCount} ≈ **${(d.todoCount / d.layerCount).toFixed(1)} todos in flight**.`].join('\n');
    } else body = [heading, '', criticalPath(d, lines.slice(s + 1, e))].join('\n');
    lines = [...lines.slice(0, s), ...body.split('\n'), '', ...lines.slice(e)];
  }
  out = lines.join('\n');

  // 3. `{{...}}` counts, outside the contract section only -- see header note 2.
  lines = out.split('\n');
  const cs = sectionBounds(lines, CONTRACT_HEADING);
  const counts = {
    TODO_COUNT: String(d.todoCount), WAVE_COUNT: String(d.waveCount),
    LAYER_COUNT: String(d.layerCount), MANIFEST_COUNT: String(d.manifestCount),
  };
  for (let i = 0; i < lines.length; i++) {
    if (i >= cs[0] && i < cs[1]) continue;
    lines[i] = lines[i].replace(/\{\{([A-Z_]+)\}\}/g, (m, k) => (k in counts ? counts[k] : m));
  }
  return lines.join('\n');
}

// ------------------------------------------------------------- assertions ---

function assert(text, p, d) {
  const lines = text.split('\n');
  const f = [];
  const A = (inv, msg) => f.push(`(${inv}) ${msg}`);

  // (a) todo graph acyclic, no self-dependency; wave graph acyclic after the
  //     collapse and self-edge-free by construction.
  const todoState = new Map(); let todoCycle = null;
  const walk = (n) => {
    if (todoState.get(n) === 2) return;
    if (todoState.get(n) === 1) { todoCycle = n; return; }
    todoState.set(n, 1);
    for (const dep of p.matrix.get(n)?.deps ?? []) walk(dep);
    todoState.set(n, 2);
  };
  for (const id of p.matrix.keys()) walk(id);
  if (todoCycle !== null) A('a', `todo graph has a cycle through todo ${todoCycle}`);
  for (const [id, row] of p.matrix) if (row.deps.includes(id)) A('a', `todo ${id} depends on itself`);
  for (const w of d.labels) {
    if (d.waveDeps.get(w).includes(w)) A('a', `wave ${shortWave(w)} has a self-edge after the collapse`);
  }

  // (b) every Depends on / Can parallelize with entry names a real todo.
  for (const [id, row] of p.matrix) {
    for (const dep of row.deps) if (!p.matrix.has(dep)) A('b', `todo ${id} Depends on unknown todo ${dep}`);
    for (const q of row.parallel) if (!p.matrix.has(q)) A('b', `todo ${id} Can parallelize with unknown todo ${q}`);
  }

  // (c) Blocked by equals the Depends on cell.
  for (const [id, t] of p.todos) {
    if (t.parLabel === 'Final wave') continue;
    if (t.blockedByRaw === null) { A('c', `todo ${id} has no Blocked by: value`); continue; }
    if (!sameSet(idList(t.blockedByRaw), p.matrix.get(id)?.deps ?? [])) {
      A('c', `todo ${id}: Blocked by: '${t.blockedByRaw}' != Depends on '${ranges(p.matrix.get(id)?.deps ?? [])}'`);
    }
  }

  // (d) parallelize n depends == empty in both directions.
  for (const [id, row] of p.matrix) {
    const deps = new Set(row.deps);
    for (const q of row.parallel) {
      if (deps.has(q)) A('d', `todo ${id} lists ${q} as both a dependency and parallelizable`);
      if ((p.matrix.get(q)?.deps ?? []).includes(id)) A('d', `todo ${id} lists ${q} as parallelizable but ${q} depends on ${id}`);
    }
  }

  // (e) every wave's documented Depends on equals waveDeps(W) exactly.
  // First: the four generated blocks must exist. Without this guard a deleted
  // heading silently empties `documented` and (e) and (f) pass vacuously --
  // the exact way a generated region can rot unnoticed.
  for (const heading of GENERATED_BLOCKS) {
    if (!lines.some((l) => l.trim() === heading)) A('e', `generated block missing: ${heading} — invariants (e) and (f) cannot be evaluated`);
  }
  const nodeStart = lines.findIndex((l) => l.trim() === GENERATED_BLOCKS[0]);
  const documented = new Map();
  // `| Wave | Phase | Todos | n | Depends on | Layer |` -> split('|') indices
  // 1..6, because a leading pipe yields an empty element 0.
  for (let i = nodeStart + 1; i < lines.length && !/^#### /.test(lines[i]); i++) {
    const m = lines[i].match(/^\|\s*\*{0,2}(W\d+[a-z]?)\*{0,2}\s*\|/);
    if (!m) continue;
    const c = lines[i].split('|').map((s) => s.trim());
    documented.set(m[1], { n: c[4], dependsOn: c[5], layer: c[6] });
  }
  for (const w of d.order) {
    const s = shortWave(w);
    const want = ranges(d.waveDeps.get(w).map(shortWave));
    if (!documented.has(s)) { A('e', `node table has no row for ${s} (waveDeps = [${want}])`); continue; }
    const got = (documented.get(s).dependsOn ?? '').replace(/^`|`$/g, '').trim();
    if (got !== want) A('e', `${s}: node table Depends on '${got || EMPTY}' != waveDeps '${want || EMPTY}'`);
  }
  for (const s of documented.keys()) if (!d.order.map(shortWave).includes(s)) A('e', `node table has a row for unknown wave ${s}`);

  // (f) layer column, layer table, running totals, layer count.
  for (const s of documented.keys()) {
    const w = d.labels.find((x) => shortWave(x) === s);
    if (w === undefined) continue;
    if (Number(documented.get(s).layer) !== d.layer.get(w)) A('f', `${s}: node table Layer ${documented.get(s).layer} != computed ${d.layer.get(w)}`);
    if (Number(documented.get(s).n) !== d.members.get(w).length) A('f', `${s}: node table n ${documented.get(s).n} != member count ${d.members.get(w).length}`);
  }
  const lStart = lines.findIndex((l) => l.trim() === GENERATED_BLOCKS[2]);
  const layerRows = [];
  for (let i = lStart + 1; i < lines.length && !/^#### /.test(lines[i]); i++) {
    const m = lines[i].match(/^\|\s*(\d+)\s*\|\s*([^|]*)\|\s*(\d+)\s*\|\s*(\d+)\s*\|\s*$/);
    if (m) layerRows.push({ layer: Number(m[1]), waves: m[2].trim(), n: Number(m[3]), running: Number(m[4]) });
  }
  if (layerRows.length !== d.layerCount) A('f', `layer table has ${layerRows.length} rows, computed layer count is ${d.layerCount}`);
  const last = layerRows[layerRows.length - 1];
  if (!last || last.running !== d.todoCount) A('f', `layer table running total ends at ${last?.running}, TODO_COUNT is ${d.todoCount}`);
  for (const row of layerRows) {
    const want = d.layers.find((l) => l.layer === row.layer);
    if (!want) { A('f', `layer table has no computed layer ${row.layer}`); continue; }
    if (row.waves !== want.waves.map(shortWave).join(' · ')) A('f', `layer ${row.layer}: waves '${row.waves}' != '${want.waves.map(shortWave).join(' · ')}'`);
    if (row.n !== want.n) A('f', `layer ${row.layer}: todos ${row.n} != ${want.n}`);
  }

  // (g) every todo's Blocks: equals Blocks(X).
  for (const [id, t] of p.todos) {
    if (t.parLabel === 'Final wave') continue;
    if (t.blocksRaw === null) { A('g', `todo ${id} has no Blocks: value`); continue; }
    if (!isIdListCell(t.blocksRaw)) { A('g', `todo ${id}: Blocks: '${t.blocksRaw}' is not a todo list`); continue; }
    if (!sameSet(idList(t.blocksRaw), d.blocksOf.get(id) ?? [])) {
      A('g', `todo ${id}: Blocks: '${t.blocksRaw}' != computed '${ranges(d.blocksOf.get(id) ?? [])}'`);
    }
  }

  // (h) exactly one Acceptance criteria / QA scenarios / Commit per todo.
  for (const b of p.allBlocks) {
    if (b.acceptance !== 1) A('h', `todo ${b.id} (line ${b.startLine + 1}) has ${b.acceptance} '  Acceptance criteria' lines`);
    if (b.qa !== 1) A('h', `todo ${b.id} (line ${b.startLine + 1}) has ${b.qa} '  QA scenarios' lines`);
    if (b.commit !== 1) A('h', `todo ${b.id} (line ${b.startLine + 1}) has ${b.commit} '  Commit:' lines`);
    if (b.glued) A('h', `todo ${b.id} (line ${b.startLine + 1}) has Acceptance/QA/Commit text appended to a References: line`);
  }

  // (i) all manifest IDs appear at least once in the plan.
  const ids = manifestIds();
  if (ids.size === 0) A('i', 'no manifest IDs were read — invariant (i) cannot be evaluated');
  const absent = [...ids].filter((id) => !new RegExp(`(?<![A-Z0-9])${id}(?![0-9])`).test(text));
  if (absent.length) A('i', `manifest IDs absent from the plan: ${absent.join(', ')}`);

  // (j) no named count placeholder survives outside the contract section, which
  //     names them. `{{...}}` is the contract's own prose for "any placeholder"
  //     and is not itself a placeholder, so only a NAME counts here.
  const cs = sectionBounds(lines, CONTRACT_HEADING);
  for (let i = 0; i < lines.length; i++) {
    if (i >= cs[0] && i < cs[1]) continue;
    const m = lines[i].match(/\{\{[A-Z_]+\}\}/g);
    if (m) A('j', `line ${i + 1}: placeholder ${m.join(' ')} survived`);
  }

  for (const e of d.errors) A('-', e);
  return f;
}

// ------------------------------------------------------------------- main ---

function main() {
  const argv = process.argv.slice(2);
  const mode = argv.includes('--write') ? 'write' : argv.includes('--wave') ? 'wave' : 'bare';
  const unknown = argv.filter((a) => a !== '--write' && a !== '--wave' && !a.endsWith('.md'));
  if (unknown.length) { console.error(`unknown argument: ${unknown.join(' ')}`); process.exitCode = 2; return; }

  const text = readFileSync(PLAN_PATH, 'utf8');
  const p = parse(text);
  const d = derive(p);

  if (mode === 'wave') {
    console.log(`plan:   ${PLAN_PATH}`);
    console.log(`todos:  ${d.todoCount}   waves: ${d.waveCount}   layers: ${d.layerCount}   manifest IDs: ${d.manifestCount}`);
    console.log(`longest todo chain: ${d.longest.length} -> ${d.longest.join(' -> ')}`);
    console.log('');
    console.log(nodeTable(d));
    if (d.errors.length) { console.log('\nparse errors:'); for (const e of d.errors) console.log(`  ${e}`); }
    console.log('\nCONTRACT GAP 1: the contract gives no generating rule for the node table\'s `Phase`,');
    console.log('  `Todos` or `n` columns, and no `Todos` range format that can represent the authored');
    console.log('  data (todo 7 sits in W4 and todo 103 in W3, so `20-28` and `15-19` are both false).');
    console.log('  Rules used here, all evidenced from the plan: Phase 1 = waves 0-4, n = member count,');
    console.log('  Todos = maximal consecutive runs joined with an en dash (the authored table already used');
    console.log('  `69, 98` for its one non-contiguous case).');
    console.log('\nCONTRACT GAP 2: invariant (j) cannot hold literally -- the contract section itself names');
    console.log('  the placeholders and quotes (j). Substitution is confined to the rest of the document.');
    console.log(`  {{LAYER_COUNT}} and {{MANIFEST_COUNT}} have no call site outside the contract, which is why`);
    console.log('  they never appear in prose; only TODO_COUNT and WAVE_COUNT are substituted.');
    return;
  }

  if (mode === 'write') {
    const next = regenerate(text, p, d);
    if (next === text) { console.log('already up to date; no write performed'); return; }
    // Assert the candidate before it touches disk: a regeneration that would
    // leave the plan failing its own invariants is not written at all.
    const candidate = next;
    const cp = parse(candidate);
    const fails = assert(candidate, cp, derive(cp));
    if (fails.length) {
      console.error(`check-plan-deps: --write produced a document that fails ${fails.length} invariant(s); nothing written\n`);
      for (const x of fails) console.error(`  ${x}`);
      process.exitCode = 1;
      return;
    }
    writeFileSync(PLAN_PATH, candidate);
    const onDisk = readFileSync(PLAN_PATH, 'utf8');
    if (onDisk !== candidate) {
      console.error('check-plan-deps: post-write read does not match what was written');
      process.exitCode = 1;
      return;
    }
    console.log(`wrote ${PLAN_PATH} (${text.length} -> ${candidate.length} bytes); invariants (a)-(j) all pass`);
    return;
  }

  const fails = assert(text, p, d);
  if (fails.length) {
    console.error(`check-plan-deps: ${fails.length} invariant failure(s) in ${PLAN_PATH}\n`);
    for (const x of fails) console.error(`  ${x}`);
    console.error('\nrun `node scripts/check-plan-deps.mjs --write` to regenerate the derived views');
    process.exitCode = 1;
    return;
  }
  console.log(`check-plan-deps: OK — ${d.todoCount} todos, ${d.waveCount} waves, ${d.layerCount} layers, ` +
    `longest chain ${d.longest.length}, ${d.manifestCount} manifest IDs; invariants (a)-(j) all pass`);
}

try {
  main();
} catch (err) {
  console.error(`check-plan-deps: ${err && err.message ? err.message : err}`);
  process.exitCode = 1;
}
