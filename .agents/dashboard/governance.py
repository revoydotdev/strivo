#!/usr/bin/env python3
"""Governance channel — the operator <-> supervisor mailbox + live control surface.

A separate feature from the read-only dashboard: an asynchronous, file-backed
channel so the operator can (a) exchange text with the supervisory governance
instance and (b) change priorities/config that the system honours on its next
run and the TUI reflects immediately.

Store (.agents/governance/, git-tracked like the ledger):
  feed.jsonl       append-only conversation + log:
                   {ts, from: operator|supervisor|system, kind: msg|directive|note, text, ...}
  directives.json  the live control surface the SYSTEM reads:
                   {paused, focus_milestone, priority_note, weights{}, infra_gates{add[],remove[]},
                    budget{}, proposed[], updated}

Directive safety: SAFE keys apply immediately; RISKY keys (hard overrides of the
milestone-audit invariant) are recorded as `proposed` and take effect only after
the supervisor `confirm`s them. Every change also drops a feed line (provenance).

stdlib only. Importable API + a CLI (see `--help`).
"""
import argparse
import json
import os
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent  # dashboard -> .agents -> repo
GOV = ROOT / ".agents" / "governance"
FEED = GOV / "feed.jsonl"
DIRECTIVES = GOV / "directives.json"

SAFE_KEYS = {"paused", "focus_milestone", "priority_note", "weights", "infra_gates", "budget"}
RISKY_KEYS = {"force_phase", "force_milestone"}

DEFAULTS = {
    "paused": False,
    "focus_milestone": None,
    "priority_note": "",
    "weights": {},
    "infra_gates": {"add": [], "remove": []},
    "budget": {},
    "proposed": [],
    "updated": "",
}


def _ts():
    return os.environ.get("LEDGER_TS") or time.strftime("%Y-%m-%dT%H:%M:%S%z")


def _atomic_write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(text, encoding="utf-8")
    os.replace(tmp, path)


# ------------------------------------------------------------------- feed
def read_feed(n=None):
    if not FEED.exists():
        return []
    out = []
    for line in FEED.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line:
            try:
                out.append(json.loads(line))
            except Exception:
                continue
    return out[-n:] if n else out


def post(frm, text, kind="msg", **extra):
    ev = {"ts": _ts(), "from": frm, "kind": kind, "text": text}
    ev.update(extra)
    GOV.mkdir(parents=True, exist_ok=True)
    with FEED.open("a", encoding="utf-8") as fh:
        fh.write(json.dumps(ev, separators=(",", ":"), sort_keys=True) + "\n")
    return ev


def unread():
    """Operator messages posted after the last supervisor message (i.e. awaiting a reply)."""
    feed = read_feed()
    last_sup = max((e["ts"] for e in feed if e.get("from") == "supervisor" and e.get("kind") == "msg"),
                   default="")
    return [e for e in feed if e.get("from") == "operator" and e.get("kind") == "msg"
            and e.get("ts", "") > last_sup]


# ------------------------------------------------------------- directives
def read_directives():
    d = dict(DEFAULTS)
    if DIRECTIVES.exists():
        try:
            d.update(json.loads(DIRECTIVES.read_text(encoding="utf-8")))
        except Exception:
            pass
    return d


def write_directives(d):
    d["updated"] = _ts()
    _atomic_write(DIRECTIVES, json.dumps(d, indent=2, sort_keys=True) + "\n")
    return d


def set_directive(key, value, frm="operator"):
    """Apply a SAFE directive immediately, or record a RISKY one as proposed.
    Returns (status, message)."""
    d = read_directives()
    if key in SAFE_KEYS:
        d[key] = value
        write_directives(d)
        post(frm, f"set {key} = {json.dumps(value)}", kind="directive", key=key, value=value)
        return "applied", f"{key} = {json.dumps(value)}"
    if key in RISKY_KEYS:
        prop = {"key": key, "value": value, "from": frm, "ts": _ts(), "status": "proposed"}
        d.setdefault("proposed", []).append(prop)
        write_directives(d)
        post(frm, f"PROPOSED {key} = {json.dumps(value)} (awaits supervisor confirm)",
             kind="directive", key=key, value=value, proposed=True)
        return "proposed", f"{key} = {json.dumps(value)} queued for supervisor confirmation"
    return "rejected", f"unknown directive key {key!r} (safe: {sorted(SAFE_KEYS)}; risky: {sorted(RISKY_KEYS)})"


def resolve_proposal(index, action, frm="supervisor"):
    """Supervisor confirms/rejects a proposed risky directive by index."""
    d = read_directives()
    props = d.get("proposed", [])
    live = [p for p in props if p.get("status") == "proposed"]
    if index < 0 or index >= len(live):
        return "error", f"no pending proposal at index {index}"
    p = live[index]
    if action == "confirm":
        d[p["key"]] = p["value"]
        p["status"] = "confirmed"
        write_directives(d)
        post(frm, f"confirmed {p['key']} = {json.dumps(p['value'])}", kind="directive")
        return "confirmed", f"{p['key']} = {json.dumps(p['value'])}"
    p["status"] = "rejected"
    write_directives(d)
    post(frm, f"rejected proposed {p['key']} = {json.dumps(p['value'])}", kind="directive")
    return "rejected", p["key"]


def _parse_value(raw):
    try:
        return json.loads(raw)
    except Exception:
        return raw


# --------------------------------------------------------------------- CLI
def main():
    p = argparse.ArgumentParser(prog="governance.py", description="operator<->supervisor channel")
    sub = p.add_subparsers(dest="cmd", required=True)

    f = sub.add_parser("feed"); f.add_argument("--n", type=int, default=20)
    po = sub.add_parser("post")
    po.add_argument("--from", dest="frm", required=True)
    po.add_argument("--text", required=True)
    po.add_argument("--kind", default="msg")
    g = sub.add_parser("get"); g.add_argument("key", nargs="?")
    s = sub.add_parser("set")
    s.add_argument("--key", required=True); s.add_argument("--value", required=True)
    s.add_argument("--from", dest="frm", default="operator")
    sub.add_parser("unread")
    c = sub.add_parser("confirm"); c.add_argument("--index", type=int, required=True)
    r = sub.add_parser("reject"); r.add_argument("--index", type=int, required=True)

    a = p.parse_args()
    if a.cmd == "feed":
        for e in read_feed(a.n):
            print(f"{e.get('ts','')}  {e.get('from','?'):>10}  [{e.get('kind','')}]  {e.get('text','')}")
    elif a.cmd == "post":
        ev = post(a.frm, a.text, a.kind); print(f"posted: {ev['from']}: {ev['text']}")
    elif a.cmd == "get":
        d = read_directives()
        print(json.dumps(d.get(a.key) if a.key else d, indent=2, sort_keys=True))
    elif a.cmd == "set":
        status, msg = set_directive(a.key, _parse_value(a.value), a.frm)
        print(f"{status}: {msg}")
        if status == "rejected":
            sys.exit(2)
    elif a.cmd == "unread":
        u = unread()
        print(f"{len(u)} unread operator message(s)")
        for e in u:
            print(f"  {e.get('ts','')}  {e.get('text','')}")
    elif a.cmd in ("confirm", "reject"):
        status, msg = resolve_proposal(a.index, "confirm" if a.cmd == "confirm" else "reject")
        print(f"{status}: {msg}")
        if status == "error":
            sys.exit(2)


if __name__ == "__main__":
    main()
