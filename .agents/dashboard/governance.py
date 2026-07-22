#!/usr/bin/env python3
"""Governance channel — the operator <-> supervisor mailbox + live control surface,
now also the A2A bus for in-tick resource/tool/decision requests (docs/superpowers/
specs/2026-07-21-a2a-bus-design.md).

A separate feature from the read-only dashboard: an asynchronous, file-backed
channel so the operator can (a) exchange text with the supervisory governance
instance and (b) change priorities/config that the system honours on its next
run and the TUI reflects immediately.

Store (.agents/governance/, git-tracked like the ledger):
  feed.jsonl       append-only conversation + log:
                   {ts, from: operator|supervisor|system, kind: msg|directive|note|
                    a2a-request|a2a-resolved, text, ...}
  directives.json  the live control surface the SYSTEM reads:
                   {paused, focus_milestone, priority_note, weights{}, infra_gates{add[],remove[]},
                    budget{}, proposed[], updated}

Directive safety: SAFE keys apply immediately; RISKY keys (hard overrides of the
milestone-audit invariant) are recorded as `proposed` and take effect only after
the supervisor `confirm`s them. Every change also drops a feed line (provenance).

A2A requests (`kind=a2a-request`): a worker or supervisor posts a stable
`request_id`, a `request_type` (decision | tool-allowance | unavailable-tooling |
resource), and enough context for a respondent to act without re-deriving it.
`kind=a2a-resolved` closes the loop by `request_id`. Decision-type requests are
meant to be resolved same-tick by an ephemeral respondent the supervisor spawns
(SendMessage delivery back to the waiting worker); tool-allowance/
unavailable-tooling/resource requests are structurally next-tick (a running
session's MCP/plugin surface can't change mid-session) and may need OpenClaw
escalation if not already covered by tool-index.toml.

stdlib only. Importable API + a CLI (see `--help`).
"""
import argparse
import json
import os
import sys
import time
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent  # dashboard -> .agents -> repo
GOV = ROOT / ".agents" / "governance"
FEED = GOV / "feed.jsonl"
DIRECTIVES = GOV / "directives.json"

SAFE_KEYS = {"paused", "focus_milestone", "priority_note", "weights", "infra_gates", "budget"}
RISKY_KEYS = {"force_phase", "force_milestone"}
A2A_REQUEST_TYPES = {"decision", "tool-allowance", "unavailable-tooling", "resource"}

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


# ------------------------------------------------------------------- A2A bus
def post_a2a_request(frm, request_type, text, todo_id="", tried="", **extra):
    """A worker/supervisor asks for something: a judgment call (decision), a tool
    not currently tagged on this todo but possibly already indexed
    (tool-allowance), something genuinely missing (unavailable-tooling), or a
    limit/config change (resource). Returns the event, whose `request_id` the
    caller needs to later resolve() it or match a reply to it."""
    if request_type not in A2A_REQUEST_TYPES:
        raise ValueError(f"unknown request_type {request_type!r} (known: {sorted(A2A_REQUEST_TYPES)})")
    request_id = uuid.uuid4().hex[:12]
    return post(frm, text, kind="a2a-request", request_id=request_id,
                request_type=request_type, todo_id=todo_id, tried=tried, **extra)


def pending_a2a(request_type=None):
    """a2a-request events with no matching a2a-resolved event yet, newest first.
    Optionally filtered to one request_type (e.g. only what a same-tick respondent
    can act on vs. what needs next-tick/OpenClaw handling)."""
    feed = read_feed()
    resolved_ids = {e.get("request_id") for e in feed if e.get("kind") == "a2a-resolved"}
    out = [e for e in feed if e.get("kind") == "a2a-request" and e.get("request_id") not in resolved_ids]
    if request_type:
        out = [e for e in out if e.get("request_type") == request_type]
    return list(reversed(out))


def unescalated_a2a(request_type=None):
    """pending_a2a() entries with no a2a-escalated marker yet — what the OpenClaw
    relay still needs to actually send, as opposed to ones already sent and only
    awaiting the operator's reply (which stay `pending` but shouldn't be re-sent)."""
    feed = read_feed()
    escalated_ids = {e.get("request_id") for e in feed if e.get("kind") == "a2a-escalated"}
    return [e for e in pending_a2a(request_type) if e.get("request_id") not in escalated_ids]


def resolve_a2a(request_id, frm, text, **extra):
    """Close the loop on a pending request — same-tick respondent answer, or an
    operator reply relayed back in by request_id (the OpenClaw relay's job)."""
    return post(frm, text, kind="a2a-resolved", request_id=request_id, **extra)


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
    p = argparse.ArgumentParser(prog="governance.py", description="operator<->supervisor channel + A2A bus")
    sub = p.add_subparsers(dest="cmd", required=True)

    f = sub.add_parser("feed"); f.add_argument("--n", type=int, default=20)
    po = sub.add_parser("post")
    po.add_argument("--from", dest="frm", required=True)
    po.add_argument("--text", required=True)
    po.add_argument("--kind", default="msg")
    po.add_argument("--request-id", dest="request_id", default=None,
                     help="tag this post against an a2a-request (e.g. kind=a2a-escalated)")
    g = sub.add_parser("get"); g.add_argument("key", nargs="?")
    s = sub.add_parser("set")
    s.add_argument("--key", required=True); s.add_argument("--value", required=True)
    s.add_argument("--from", dest="frm", default="operator")
    sub.add_parser("unread")
    c = sub.add_parser("confirm"); c.add_argument("--index", type=int, required=True)
    r = sub.add_parser("reject"); r.add_argument("--index", type=int, required=True)

    ar = sub.add_parser("a2a-request")
    ar.add_argument("--from", dest="frm", required=True)
    ar.add_argument("--type", dest="request_type", required=True, choices=sorted(A2A_REQUEST_TYPES))
    ar.add_argument("--text", required=True)
    ar.add_argument("--todo", dest="todo_id", default="")
    ar.add_argument("--tried", default="")

    ap_ = sub.add_parser("a2a-pending")
    ap_.add_argument("--type", dest="request_type", default=None, choices=sorted(A2A_REQUEST_TYPES))
    ap_.add_argument("--json", action="store_true")

    au = sub.add_parser("a2a-unescalated")
    au.add_argument("--type", dest="request_type", default=None, choices=sorted(A2A_REQUEST_TYPES))
    au.add_argument("--json", action="store_true")

    ares = sub.add_parser("a2a-resolve")
    ares.add_argument("--id", dest="request_id", required=True)
    ares.add_argument("--from", dest="frm", required=True)
    ares.add_argument("--text", required=True)

    a = p.parse_args()
    if a.cmd == "feed":
        for e in read_feed(a.n):
            print(f"{e.get('ts','')}  {e.get('from','?'):>10}  [{e.get('kind','')}]  {e.get('text','')}")
    elif a.cmd == "post":
        extra = {"request_id": a.request_id} if a.request_id else {}
        ev = post(a.frm, a.text, a.kind, **extra); print(f"posted: {ev['from']}: {ev['text']}")
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
    elif a.cmd == "a2a-request":
        ev = post_a2a_request(a.frm, a.request_type, a.text, a.todo_id, a.tried)
        print(f"request_id={ev['request_id']}")
    elif a.cmd == "a2a-pending":
        pend = pending_a2a(a.request_type)
        if a.json:
            print(json.dumps(pend))
        else:
            print(f"{len(pend)} pending a2a-request(s)")
            for e in pend:
                print(f"  {e.get('request_id','')}  [{e.get('request_type','')}]  {e.get('text','')}")
    elif a.cmd == "a2a-unescalated":
        pend = unescalated_a2a(a.request_type)
        if a.json:
            print(json.dumps(pend))
        else:
            print(f"{len(pend)} unescalated a2a-request(s)")
            for e in pend:
                print(f"  {e.get('request_id','')}  [{e.get('request_type','')}]  {e.get('text','')}")
    elif a.cmd == "a2a-resolve":
        ev = resolve_a2a(a.request_id, a.frm, a.text)
        print(f"resolved {a.request_id}: {ev['text']}")


if __name__ == "__main__":
    main()
