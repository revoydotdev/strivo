#!/usr/bin/env python3
"""Project Skinner status dashboard — local TUI (read-only).

A terminal renderer over the SAME projection the web dashboard serves
(`server.build_state()` / `/api/state`), so the two views cannot drift. Mirrors
the web sections as-is — milestone rail, "Now" live activity, active-milestone
outline, recent commits — using the shared semantic tokens (tokens.py, the
revoy-spec visual-system idea). Sole dependency: `rich`.

Usage:
  python3 .agents/dashboard/tui.py                # poll the LAN dashboard API
  python3 .agents/dashboard/tui.py --local        # import build_state (on daedalus, in-repo)
  python3 .agents/dashboard/tui.py --url URL       # custom /api/state endpoint
  python3 .agents/dashboard/tui.py --once          # render one frame and exit
  python3 .agents/dashboard/tui.py --interval 2    # refresh seconds (default 2)
Env: SKINNER_DASH_URL overrides the default endpoint.
"""
import argparse
import json
import os
import sys
import time
import urllib.request
from datetime import datetime
from pathlib import Path

from rich.console import Console, Group
from rich.layout import Layout
from rich.live import Live
from rich.panel import Panel
from rich.text import Text
from rich.tree import Tree

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import tokens as T  # noqa: E402
import governance as G  # noqa: E402

DEFAULT_URL = os.environ.get("SKINNER_DASH_URL", "http://192.168.8.203:8770/api/state")


# --------------------------------------------------------------------------- data
def fetch_state(url, local):
    if local:
        import server  # server.py in the same dir
        return server.build_state()
    with urllib.request.urlopen(url, timeout=6) as r:
        return json.loads(r.read().decode("utf-8"))


def gov_snapshot(n=40):
    """Governance channel state (always read locally — it lives in this repo)."""
    try:
        return {"directives": G.read_directives(), "feed": G.read_feed(n), "unread": G.unread()}
    except Exception as e:
        return {"directives": dict(G.DEFAULTS), "feed": [], "unread": [], "__error__": str(e)}


# ----------------------------------------------------------------------- renderers
def render_header(st, gov=None):
    t = Text()
    bestyle, belabel = T.beacon(st.get("phase"), st.get("live", {}).get("tick", {}).get("active"))
    t.append("  ● ", style=bestyle)
    t.append("PROJECT SKINNER", style=f"bold {T.TX}")
    t.append("   ")
    t.append(f" {st.get('current_milestone','?')} {st.get('phase_detail','')} ",
             style=f"bold {T.phase_style(st.get('phase_detail'))}")
    d = (gov or {}).get("directives", {})
    if d.get("paused"):
        t.append("  PAUSED ", style=f"bold {T.RED}")
    if d.get("focus_milestone"):
        t.append(f"  ▸focus {d['focus_milestone']} ", style=T.BLUE)
    if d.get("priority_note"):
        t.append(f"  ✎ {str(d['priority_note'])[:32]}", style=T.AMBER)
    if (gov or {}).get("unread"):
        t.append(f"  ✉{len(gov['unread'])}", style=f"bold {T.AMBER}")
    aud = st.get("last_audit")
    if aud and aud.get("verdict"):
        label = f" audit {aud['milestone']} {aud['verdict']} " if aud.get("milestone") \
            else f" audit {aud['verdict']} "
        t.append("  ")
        t.append(label, style=f"bold {T.verdict_style(aud['verdict'])}")
    tot = st.get("totals", {})
    t.append(f"   ✓{tot.get('done',0)} done", style=T.GREEN)
    t.append(f"   milestone {tot.get('current_index','?')}/{tot.get('milestones','?')}", style=T.DIM)
    t.append(f"   {belabel}", style=bestyle)
    t.append(f"   {datetime.now().strftime('%H:%M:%S')}", style=T.DIM)
    return t


def render_arrow(st):
    """Left-to-right milestone rail; arrowhead marks the final milestone."""
    t = Text(no_wrap=False)
    ms = st.get("milestones", [])
    for i, m in enumerate(ms):
        if i:
            t.append(" ▸ ", style=T.DIM)
        style = T.state_style(m["status"])
        if m.get("is_last"):
            t.append("▶", style=f"bold {style}")
        label = m["id"]
        if m["status"] == "active":
            label += f" {m.get('todos_done',0)}/{m.get('todos_total',0)}"
        t.append(T.state_glyph(m["status"]) + " ", style=style)
        t.append(label, style=f"bold {style}" if m["status"] != "pending" else style)
    return Panel(t, title="Milestones — path to 1.0.0", title_align="left",
                 border_style=T.BD, padding=(0, 1))


def render_active(st):
    a = st.get("active")
    if not a:
        return Panel(Text("No active milestone.", style=T.RED), title="Active milestone",
                     border_style=T.BD)
    # gate chips
    gates = Text()
    for g in a.get("gates", []):
        gs = T.gate_style(g["status"])
        gates.append(f" {T.gate_glyph(g['status'])} {g['id']} ", style=f"{gs}")
        gates.append("  ")
    if not a.get("gates"):
        gates.append("(no gates parsed)", style=T.DIM)

    tree = Tree(Text(f"{a['id']} — {a.get('title','')}", style=f"bold {T.TX}"), guide_style=T.BD)
    for ph in a.get("phases", []):
        pstyle = T.state_style(ph["status"])
        pnode = tree.add(Text(f"{T.state_glyph(ph['status'])} {ph['id']} {ph.get('title','')} "
                              f"· {ph.get('done',0)}/{ph.get('total',0)}", style=pstyle))
        # expand stages that still have open work; collapse fully-done stages to a line
        for stg in ph.get("stages", []):
            sstyle = T.state_style(stg["status"])
            label = f"{T.state_glyph(stg['status'])} {stg['id']} {stg.get('title','')} " \
                    f"· {stg.get('done',0)}/{stg.get('total',0)}"
            snode = pnode.add(Text(label, style=sstyle))
            if stg["status"] == "done":
                continue
            for td in stg.get("todos", []):
                gl = "✓" if td["done"] else "○"
                tstyle = T.GREEN if td["done"] else T.RED_DIM
                snode.add(Text(f"{gl} {td['id']} {td.get('desc','')}", style=tstyle))

    body = Group(gates, Text(""), tree)
    return Panel(body, title=f"Active milestone — {a['id']} [{a.get('phase_detail','')}]",
                 title_align="left", border_style=T.phase_style(a.get("phase_detail")), padding=(0, 1))


def render_now(st):
    live = st.get("live", {})
    tick = live.get("tick", {})
    bstyle, blabel = T.beacon(st.get("phase"), tick.get("active"))
    parts = []
    hb = Text()
    hb.append("● ", style=bstyle)
    hb.append(blabel, style=f"bold {bstyle}")
    if tick.get("active") and tick.get("age_s") is not None:
        hb.append(f"  ({tick['age_s']}s)", style=T.DIM)
    parts.append(hb)

    working = live.get("working", [])
    wt = Text("\nworking:  ", style=T.DIM)
    if working:
        for w in working:
            wt.append(f" ⚙ {w} ", style=T.AMBER)
            wt.append(" ")
    else:
        wt.append("(idle — no active worktrees)", style=T.DIM)
    parts.append(wt)

    rd = live.get("recent_done", [])
    parts.append(Text("\nrecently verified:", style=T.DIM))
    if rd:
        for e in rd[:6]:
            row = Text()
            row.append(f"  ✓ {e.get('todo','')}", style=T.GREEN)
            if e.get("concern"):
                row.append(f"  {e['concern']}", style=T.DIM)
            parts.append(row)
    else:
        parts.append(Text("  (none live)", style=T.DIM))

    tail = live.get("log_tail", [])
    parts.append(Text("\ntick log:", style=T.DIM))
    for ln in tail[-6:]:
        parts.append(Text("  " + ln[:70], style=T.DIM))

    return Panel(Group(*parts), title="Now — live activity", title_align="left",
                 border_style=T.BD, padding=(0, 1))


def render_commits(st):
    t = Text()
    for c in st.get("commits", [])[:12]:
        t.append(f"{c['hash']} ", style=f"bold {T.AMBER}")
        t.append("▏", style=T.commit_style(c.get("type", "other")))
        t.append(f" {c['subject'][:66]}", style=T.TX)
        t.append(f"  {c.get('rel','')}\n", style=T.DIM)
    if not st.get("commits"):
        t.append("(no commits)", style=T.DIM)
    return Panel(t, title="Recent commits", title_align="left", border_style=T.BD, padding=(0, 1))


def _from_style(frm):
    return {"operator": T.BLUE, "supervisor": T.GREEN, "system": T.DIM}.get(frm, T.TX)


def render_feed(gov):
    t = Text()
    feed = gov.get("feed", [])
    if not feed:
        t.append("(no messages yet — press i to send one to the supervisor)", style=T.DIM)
    for e in feed[-24:]:
        frm = e.get("from", "?")
        t.append(f"{(e.get('ts','') or '')[11:19]} ", style=T.DIM)
        t.append(f"{frm:>10}", style=f"bold {_from_style(frm)}")
        kind = e.get("kind", "msg")
        mark = "»" if kind == "msg" else ("⚙" if kind == "directive" else "·")
        t.append(f" {mark} ", style=T.DIM)
        t.append(f"{e.get('text','')}\n", style=_from_style(frm) if kind == "msg" else T.TX)
    return Panel(t, title="Conversation — operator ⇄ supervisor", title_align="left",
                border_style=T.BD, padding=(0, 1))


def render_directives(gov):
    d = gov.get("directives", {})
    t = Text()

    def row(label, val, style=T.TX):
        t.append(f"  {label:<16}", style=T.DIM)
        t.append(f"{val}\n", style=style)

    row("paused", str(d.get("paused", False)), T.RED if d.get("paused") else T.GREEN)
    row("focus_milestone", d.get("focus_milestone") or "—", T.BLUE if d.get("focus_milestone") else T.DIM)
    row("priority_note", d.get("priority_note") or "—", T.AMBER if d.get("priority_note") else T.DIM)
    w = d.get("weights") or {}
    row("weights", ", ".join(f"{k}={v}" for k, v in w.items()) or "—", T.DIM)
    ig = d.get("infra_gates") or {}
    row("infra_gates", f"+{ig.get('add', [])} -{ig.get('remove', [])}", T.DIM)
    pend = [p for p in d.get("proposed", []) if p.get("status") == "proposed"]
    if pend:
        t.append("\n  pending (await supervisor confirm):\n", style=T.AMBER)
        for i, p in enumerate(pend):
            t.append(f"    [{i}] {p['key']} = {json.dumps(p['value'])}\n", style=T.AMBER)
    row("updated", (d.get("updated") or "—")[:19], T.DIM)
    return Panel(t, title="Directives — live control surface", title_align="left",
                border_style=T.BD, padding=(0, 1))


def render_help_bar(mode):
    t = Text()
    keys = [("d", "dashboard"), ("g", "governance"), ("i", "message"),
            (":", "set directive"), ("q", "quit")]
    for k, lbl in keys:
        t.append(f" {k} ", style=f"bold {T.TX} on {T.BD}")
        t.append(f"{lbl}   ", style=T.DIM)
    t.append(f"  [{mode}]", style=T.AMBER)
    return t


def build_layout(st, gov=None, mode="dashboard"):
    gov = gov or {}
    if st.get("__error__") and mode != "governance":
        return Panel(Text(f"dashboard unreachable: {st['__error__']}\n(retrying…)", style=T.RED),
                     title="Project Skinner TUI", border_style=T.RED)
    root = Layout()
    if mode == "governance":
        root.split_column(
            Layout(render_header(st, gov), name="header", size=1),
            Layout(name="body"),
            Layout(render_help_bar(mode), name="help", size=1),
        )
        root["body"].split_row(
            Layout(render_feed(gov), name="feed", ratio=2),
            Layout(render_directives(gov), name="directives", ratio=1),
        )
        return root
    root.split_column(
        Layout(render_header(st, gov), name="header", size=1),
        Layout(render_arrow(st), name="arrow", size=3),
        Layout(name="body"),
        Layout(render_commits(st), name="commits", size=10),
        Layout(render_help_bar(mode), name="help", size=1),
    )
    root["body"].split_row(
        Layout(render_active(st), name="active", ratio=2),
        Layout(render_now(st), name="now", ratio=1),
    )
    return root


# ----------------------------------------------------------------------- commands
def _apply_command(line):
    """Parse a `:` directive command: `key value` or `key=value`. Returns a status str."""
    line = line.strip()
    if not line:
        return ""
    if "=" in line and " " not in line.split("=", 1)[0]:
        key, val = line.split("=", 1)
    else:
        parts = line.split(None, 1)
        key, val = parts[0], (parts[1] if len(parts) > 1 else "")
    status, msg = G.set_directive(key.strip(), G._parse_value(val.strip()), frm="operator")
    return f"{status}: {msg}"


# ----------------------------------------------------------------------------- main
def main():
    ap = argparse.ArgumentParser(description="Project Skinner local TUI dashboard + governance channel")
    ap.add_argument("--url", default=DEFAULT_URL, help="/api/state endpoint")
    ap.add_argument("--local", action="store_true", help="import build_state() directly (on daedalus)")
    ap.add_argument("--once", action="store_true", help="render a single frame and exit")
    ap.add_argument("--view", choices=("dashboard", "governance"), default="dashboard")
    ap.add_argument("--project", help="portfolio project id to view (via /api/state?project=)")
    ap.add_argument("--interval", type=float, default=2.0, help="refresh seconds")
    args = ap.parse_args()

    console = Console()
    url = args.url
    if args.project and not args.local:
        url += ("&" if "?" in url else "?") + "project=" + args.project

    def snapshot():
        try:
            return fetch_state(url, args.local)
        except Exception as e:
            return {"__error__": str(e)}

    def frame(mode):
        return build_layout(snapshot(), gov_snapshot(), mode)

    if args.once or not sys.stdin.isatty():
        console.print(frame(args.view))
        return

    import termios
    import tty
    import select

    mode = {"v": args.view}
    old = termios.tcgetattr(sys.stdin)

    def read_line(live, prompt):
        # drop to cooked mode for line editing, then restore cbreak
        live.stop()
        termios.tcsetattr(sys.stdin, termios.TCSADRAIN, old)
        try:
            sys.stdout.write("\r\n" + prompt)
            sys.stdout.flush()
            line = sys.stdin.readline()
        except (EOFError, KeyboardInterrupt):
            line = ""
        tty.setcbreak(sys.stdin.fileno())
        live.start(refresh=True)
        return line.strip()

    try:
        with Live(frame(mode["v"]), console=console, screen=True, refresh_per_second=4) as live:
            tty.setcbreak(sys.stdin.fileno())
            while True:
                r, _, _ = select.select([sys.stdin], [], [], args.interval)
                if r:
                    ch = sys.stdin.read(1)
                    if ch in ("q", "Q", "\x03"):
                        break
                    elif ch == "d":
                        mode["v"] = "dashboard"
                    elif ch == "g":
                        mode["v"] = "governance"
                    elif ch == "i":
                        msg = read_line(live, "message to supervisor> ")
                        if msg:
                            G.post("operator", msg)
                    elif ch == ":":
                        cmd = read_line(live, "directive (key value | key=value)> ")
                        if cmd:
                            _apply_command(cmd)
                live.update(frame(mode["v"]))
    except KeyboardInterrupt:
        pass
    finally:
        termios.tcsetattr(sys.stdin, termios.TCSADRAIN, old)


if __name__ == "__main__":
    main()
