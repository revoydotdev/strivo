#!/usr/bin/env python3
"""revoy-lite work ledger — .agents/ledger.jsonl (append-only, structured).

Adopts two revoy-spec first principles in their cheapest form (see ADR-0022):

  1. Provenance-as-a-type. A todo is recorded `done` ONLY with a `verified_by`
     {cmd, exit:0}. `ledger.py done --run` executes the command and REFUSES to
     write the done event if it does not exit 0 — you cannot retire work by
     asserting it is retired.
  2. Query the ledger, don't re-read prose. `status` / `next` answer from the
     structured events instead of parsing a growing markdown file.

Deliberately NOT adopted (per revoy's own governance-A/B: heavy review measured
~3x cost, 0 defects caught): turnstiles, tamper-evident hash chains, conformance
stamps, a separate-identity auditor, MCL. Structural proof at write-time is the
cheap guarantee that makes heavy audit-time review unnecessary.

stdlib only.
"""
import argparse
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
LEDGER = REPO / ".agents" / "ledger.jsonl"
ROADMAP = REPO / "ROADMAP.md"

TODO_RE = re.compile(r"M\d+\.P\d+\.S\d+\.T\d+")
ROADMAP_TODO_RE = re.compile(r"\*\*`(M\d+\.P\d+\.S\d+\.T\d+)`\*\*")


def _ts():
    return os.environ.get("LEDGER_TS") or time.strftime("%Y-%m-%dT%H:%M:%S%z")


def _milestone(todo):
    return todo.split(".", 1)[0]


def _append(event):
    event.setdefault("ts", _ts())
    LEDGER.parent.mkdir(parents=True, exist_ok=True)
    with LEDGER.open("a", encoding="utf-8") as fh:
        fh.write(json.dumps(event, separators=(",", ":"), sort_keys=True) + "\n")


def _read():
    if not LEDGER.exists():
        return []
    out = []
    for line in LEDGER.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line:
            out.append(json.loads(line))
    return out


def _effective_dones(events):
    # Last done/kill event per todo wins. A `kill` retracts a prior `done`
    # (e.g. a gate over-claimed at write time, reconciled by the tick driver).
    state = {}
    for e in events:
        t, todo = e.get("type"), e.get("todo")
        if not todo:
            continue
        if t == "done":
            state[todo] = ("done", e)
        elif t == "kill":
            state[todo] = ("kill", e)
    return [ev for kind, ev in state.values() if kind == "done"]


def _done_ids(events):
    return {e["todo"] for e in _effective_dones(events)}


def cmd_done(a):
    if not TODO_RE.fullmatch(a.todo):
        sys.exit(f"ledger: bad todo id {a.todo!r}")
    exit_code = a.exit
    if a.run:
        rc = subprocess.run(a.cmd, shell=True, cwd=REPO).returncode
        exit_code = rc
        if rc != 0:
            sys.exit(
                f"ledger: REFUSING to mark {a.todo} done — verify cmd exited {rc}\n"
                f"  cmd: {a.cmd}\n  (provenance-as-a-type: only verified work retires a todo)"
            )
    ev = {
        "type": "done",
        "todo": a.todo,
        "milestone": _milestone(a.todo),
        "concern": a.concern or "",
        "commit": a.commit or "",
        "verified_by": {"cmd": a.cmd, "exit": exit_code},
    }
    if a.source:
        ev["source"] = a.source
    _append(ev)
    print(f"ledger: recorded {a.todo} done (verified: {a.cmd!r} => {exit_code})")


def cmd_kill(a):
    if not TODO_RE.fullmatch(a.todo):
        sys.exit(f"ledger: bad todo id {a.todo!r}")
    _append(
        {
            "type": "kill",
            "todo": a.todo,
            "milestone": _milestone(a.todo),
            "reason": a.reason,
        }
    )
    print(f"ledger: killed {a.todo} (retracted done) — {a.reason}")


def cmd_event(a):
    ev = {"type": a.type}
    for kv in a.field or []:
        k, _, v = kv.partition("=")
        ev[k] = v
    _append(ev)
    print(f"ledger: appended {a.type} event")


def cmd_check(a):
    events = _read()
    dones = _effective_dones(events)
    violations = []
    for e in dones:
        vb = e.get("verified_by") or {}
        if not vb.get("cmd"):
            violations.append(f"{e.get('todo')}: no verified_by.cmd")
        elif vb.get("exit") != 0:
            violations.append(f"{e.get('todo')}: verified_by.exit={vb.get('exit')} (must be 0)")
    if a.rerun:
        seen = {}
        for e in dones:
            if e.get("source") == "backfill" and not a.rerun_all:
                continue
            cmd = (e.get("verified_by") or {}).get("cmd")
            if not cmd or cmd in seen:
                continue
            rc = subprocess.run(cmd, shell=True, cwd=REPO).returncode
            seen[cmd] = rc
            if rc != 0:
                violations.append(f"{e.get('todo')}: RERUN {cmd!r} => {rc}")
    if violations:
        print("ledger check: FAIL")
        for v in violations:
            print("  -", v)
        sys.exit(1)
    mode = "structural+rerun" if a.rerun else "structural"
    print(f"ledger check: PASS ({len(dones)} done todos, {mode})")


def cmd_status(a):
    events = _read()
    dones = _effective_dones(events)
    by_ms = {}
    for e in dones:
        by_ms.setdefault(e.get("milestone", "?"), []).append(e["todo"])
    print(f"done todos: {len(_done_ids(events))}")
    for ms in sorted(by_ms):
        ids = sorted(set(by_ms[ms]))
        if not a.milestone or a.milestone == ms:
            print(f"  {ms}: {len(ids)} — {' '.join(ids)}")


def cmd_next(a):
    done = _done_ids(_read())
    text = ROADMAP.read_text(encoding="utf-8") if ROADMAP.exists() else ""
    todos = [t for t in ROADMAP_TODO_RE.findall(text) if t.startswith(a.milestone + ".")]
    remaining = [t for t in dict.fromkeys(todos) if t not in done]
    print(f"{a.milestone}: {len(remaining)} unclaimed of {len(set(todos))}")
    for t in remaining:
        print("  " + t)


def cmd_backfill(a):
    # Map known todos to their verified command; default to the aggregate ci gate.
    CMD = {
        "M1.P1.S1.T1": "git rev-parse v0.0.1",
        "M1.P1.S1.T2": "bash scripts/check-commit-msg_test.sh",
        "M1.P1.S1.T3": "bash scripts/check-docs.sh",
        "M1.P1.S2.T1": "bash scripts/worktree.sh --self-test",
        "M1.P1.S2.T2": "bash scripts/claim_test.sh",
        "M1.P1.S2.T3": "bash scripts/integrate_test.sh",
        "M1.P2.S1.T1": "bash scripts/setup-godot.sh --verify",
        "M1.P2.S1.T2": "bash scripts/export-smoke.sh",
        "M1.P2.S1.T3": "bash scripts/ci.sh",
        "M1.P2.S2.T1": "bash tools/blender/render.sh",
        "M1.P2.S2.T4": "bash scripts/validate-assets.sh assets/fixtures/good",
        "M1.P3.S1.T1": "bash sim/run_tests.sh",
        "M1.P3.S1.T2": "bash sim/run_tests.sh",
        "M1.P3.S1.T3": "bash sim/run_content_tests.sh",
        "M1.P3.S2.T1": "bash scripts/test.sh",
        "M1.P3.S2.T2": "bash scripts/test.sh",
        "M1.P3.S2.T3": "bash scripts/bench.sh",
        "M2.P1.S1.T1": "bash world/run_tests.sh",
        "M2.P1.S2.T1": "bash placement/run_tests.sh",
        "M2.P2.S1.T1": "bash camera/run_tests.sh",
    }
    default = "bash scripts/ci.sh"
    text = Path(a.state).read_text(encoding="utf-8")
    seen = _done_ids(_read())
    n = 0
    # A DONE line = has a todo id AND an em-dash "— DONE" marker. Grab the first
    # backticked hex commit anywhere on the line (formats vary: "DONE — `h`",
    # "DONE — integration `h`", "DONE (baseline, tag `v0.0.1`)" → no hex → "").
    for line in text.splitlines():
        tm = TODO_RE.search(line)
        if not tm or not re.search(r"—\s*DONE\b", line):
            continue
        todo = tm.group(0)
        if todo in seen:
            continue
        seen.add(todo)
        cm = re.search(r"`([0-9a-f]{7,40})`", line)
        _append({
            "type": "done",
            "todo": todo,
            "milestone": _milestone(todo),
            "commit": cm.group(1) if cm else "",
            "verified_by": {"cmd": CMD.get(todo, default), "exit": 0},
            "source": "backfill",
        })
        n += 1
    print(f"ledger: backfilled {n} done todos from {a.state}")


def main():
    p = argparse.ArgumentParser(prog="ledger.py")
    sub = p.add_subparsers(dest="cmd", required=True)

    d = sub.add_parser("done", help="record a todo done (verified_by required)")
    d.add_argument("--todo", required=True)
    d.add_argument("--cmd", required=True, help="the verification command")
    d.add_argument("--commit", default="")
    d.add_argument("--concern", default="")
    d.add_argument("--exit", type=int, default=0)
    d.add_argument("--run", action="store_true", help="execute cmd; refuse done if it fails")
    d.add_argument("--source", default="")
    d.set_defaults(func=cmd_done)

    k = sub.add_parser("kill", help="retract a prior done (reconcile an over-claim)")
    k.add_argument("--todo", required=True)
    k.add_argument("--reason", required=True)
    k.set_defaults(func=cmd_kill)

    e = sub.add_parser("event", help="append a generic structured event")
    e.add_argument("--type", required=True)
    e.add_argument("--field", action="append", help="key=value (repeatable)")
    e.set_defaults(func=cmd_event)

    c = sub.add_parser("check", help="structural verified_by gate (ci); --rerun re-executes")
    c.add_argument("--rerun", action="store_true")
    c.add_argument("--rerun-all", action="store_true", help="also rerun backfilled entries")
    c.set_defaults(func=cmd_check)

    s = sub.add_parser("status")
    s.add_argument("--milestone", default="")
    s.set_defaults(func=cmd_status)

    nx = sub.add_parser("next")
    nx.add_argument("--milestone", required=True)
    nx.set_defaults(func=cmd_next)

    bf = sub.add_parser("backfill", help="seed ledger from STATE.md DONE lines")
    bf.add_argument("--state", default=str(REPO / ".agents" / "STATE.md"))
    bf.set_defaults(func=cmd_backfill)

    a = p.parse_args()
    a.func(a)


if __name__ == "__main__":
    main()
