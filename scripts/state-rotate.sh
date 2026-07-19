#!/usr/bin/env python3
"""scripts/state-rotate.sh — cap the live .agents/STATE.md, archive the rest.

Token economy (token-economics postmortem, ADR-0022 lineage): STATE.md is read
in full by the supervisor every tick and grows ~forever (append-only narrative +
an integration-event log). This bounds the LIVE file to the most recent context
and moves older entries into .agents/STATE-archive.md (never read by a tick, only
by humans / a milestone audit). No content is lost — archive + live == original.

STATE.md structure this handles:
  1. header            — lines before the first `## ` heading
  2. CONTROL block     — the two machine-read lines `- MILESTONE_PHASE:` /
                         `- CURRENT_MILESTONE:`. Hoisted to a stable, marked block
                         at the TOP so a grep-based read always finds it (it was
                         previously buried mid-file, findable only by luck).
  3. narrative entries — `## <date> ...` H2 blocks, NEWEST FIRST
  4. integration log   — trailing `- <ISO> — integrated ...` lines, oldest first

Rotation keeps the newest KEEP_ENTRIES narrative blocks and the last KEEP_INTLOG
integration-log lines live; older content is appended to the archive.

Safety: fail-closed. On any parse ambiguity it writes nothing and exits 0 (a tick
must never fail because rotation was skipped). Idempotent. `--dry-run` reports only.

Usage:
  scripts/state-rotate.sh [--dry-run] [--keep-entries N] [--keep-intlog M]
Env: STATE_KEEP_ENTRIES (default 12), STATE_KEEP_INTLOG (default 60).
"""
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
STATE = ROOT / ".agents" / "STATE.md"
ARCHIVE = ROOT / ".agents" / "STATE-archive.md"

CONTROL_MARK = "<!-- CONTROL: machine-read; supervisor updates these two lines -->"
INTLOG_RE = re.compile(r"^- \d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\S* — integrated ")
PHASE_RE = re.compile(r"^- MILESTONE_PHASE:\s*(\S+)")
MILESTONE_RE = re.compile(r"^- CURRENT_MILESTONE:\s*(\S+)")


def _bail(msg):
    # Fail-closed: never break a tick because rotation could not run cleanly.
    print(f"state-rotate: skipped ({msg})")
    sys.exit(0)


def main():
    args = sys.argv[1:]
    dry = "--dry-run" in args
    keep_entries = int(os.environ.get("STATE_KEEP_ENTRIES", "12"))
    keep_intlog_n = int(os.environ.get("STATE_KEEP_INTLOG", "60"))
    if "--keep-entries" in args:
        keep_entries = int(args[args.index("--keep-entries") + 1])
    if "--keep-intlog" in args:
        keep_intlog_n = int(args[args.index("--keep-intlog") + 1])

    if not STATE.exists():
        _bail("no STATE.md")
    lines = STATE.read_text(encoding="utf-8").splitlines()

    # --- locate the integration-log: the maximal trailing run of intlog/blank lines
    intlog_start = len(lines)
    i = len(lines) - 1
    saw_intlog = False
    while i >= 0:
        s = lines[i]
        if INTLOG_RE.match(s):
            saw_intlog = True
            intlog_start = i
            i -= 1
        elif s.strip() == "" and saw_intlog:
            i -= 1
        else:
            break
    if not saw_intlog:
        intlog_start = len(lines)

    # --- header = everything before the first `## ` narrative heading
    first_h2 = next((k for k, s in enumerate(lines) if s.startswith("## ")), intlog_start)
    header = lines[:first_h2]
    body = lines[first_h2:intlog_start]
    intlog = lines[intlog_start:]

    # --- derive control values: prefer an existing marked CONTROL block, else the
    #     first standalone pair anywhere in the file.
    phase = milestone = None
    for k, s in enumerate(lines):
        if s.strip() == CONTROL_MARK:
            for t in lines[k + 1:k + 4]:
                m = PHASE_RE.match(t)
                if m:
                    phase = m.group(1)
                m = MILESTONE_RE.match(t)
                if m:
                    milestone = m.group(1)
            break
    if phase is None or milestone is None:
        for s in lines:
            if phase is None:
                m = PHASE_RE.match(s)
                if m:
                    phase = m.group(1)
            if milestone is None:
                m = MILESTONE_RE.match(s)
                if m:
                    milestone = m.group(1)
    if phase is None or milestone is None:
        _bail("could not locate MILESTONE_PHASE / CURRENT_MILESTONE")

    # --- split narrative body into `## ` blocks (newest first)
    blocks, cur = [], []
    for s in body:
        if s.startswith("## "):
            if cur:
                blocks.append(cur)
            cur = [s]
        else:
            cur.append(s)
    if cur:
        blocks.append(cur)

    kept_blocks = blocks[:keep_entries]
    arch_blocks = blocks[keep_entries:]
    if keep_intlog_n and len(intlog) > keep_intlog_n:
        kept_intlog = intlog[-keep_intlog_n:]
        arch_intlog = intlog[:-keep_intlog_n]
    else:
        kept_intlog = intlog
        arch_intlog = []

    if not arch_blocks and not arch_intlog:
        print(f"state-rotate: nothing to rotate ({len(blocks)} entries, {len(intlog)} intlog lines; "
              f"keep {keep_entries}/{keep_intlog_n})")
        sys.exit(0)

    # --- rebuild the header with a canonical CONTROL block at the top (strip any
    #     pre-existing marked control block / stray standalone control lines from
    #     the header first so we don't duplicate).
    clean_header = []
    skip = 0
    for s in header:
        if skip > 0:
            skip -= 1
            continue
        if s.strip() == CONTROL_MARK:
            skip = 2  # drop the two control lines that follow
            continue
        if PHASE_RE.match(s) or MILESTONE_RE.match(s):
            continue
        clean_header.append(s)
    while clean_header and clean_header[-1].strip() == "":
        clean_header.pop()

    control = [CONTROL_MARK, f"- MILESTONE_PHASE: {phase}", f"- CURRENT_MILESTONE: {milestone}"]
    new_state = clean_header + [""] + control + [""] + [l for b in kept_blocks for l in b]
    while new_state and new_state[-1].strip() == "":
        new_state.pop()
    if kept_intlog:
        new_state += [""] + kept_intlog

    archived = []
    for b in arch_blocks:
        archived += b
    if arch_intlog:
        archived += arch_intlog

    if dry:
        print(f"state-rotate --dry-run: would keep {len(kept_blocks)}/{len(blocks)} entries + "
              f"{len(kept_intlog)}/{len(intlog)} intlog lines; archive {len(arch_blocks)} entries + "
              f"{len(arch_intlog)} intlog lines. CONTROL: phase={phase} milestone={milestone}.")
        sys.exit(0)

    # --- write archive (append), then live file
    stamp = os.environ.get("LEDGER_TS", "")
    fresh_archive = (not ARCHIVE.exists()) or ARCHIVE.stat().st_size == 0
    with ARCHIVE.open("a", encoding="utf-8") as fh:
        if fresh_archive:
            fh.write("# STATE archive — rotated out of the live STATE.md (not read by ticks)\n\n")
        fh.write(f"\n<!-- rotated {stamp} : {len(arch_blocks)} entries + {len(arch_intlog)} intlog lines -->\n")
        fh.write("\n".join(archived).rstrip() + "\n")

    STATE.write_text("\n".join(new_state).rstrip() + "\n", encoding="utf-8")
    print(f"state-rotate: kept {len(kept_blocks)}/{len(blocks)} entries + {len(kept_intlog)}/{len(intlog)} "
          f"intlog lines live; archived {len(arch_blocks)} entries + {len(arch_intlog)} intlog lines "
          f"→ {ARCHIVE.name}. CONTROL hoisted (phase={phase}, milestone={milestone}).")


if __name__ == "__main__":
    main()
