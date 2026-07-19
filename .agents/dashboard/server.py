#!/usr/bin/env python3
"""Project Skinner status dashboard — dependency-free LAN webui (read-only).

Portfolio-aware: reads the swarm docket (ADR-0026) and serves per-project state,
so one dashboard shows every enrolled member behind a project switcher. Projects
`/api/projects`; per-project state `/api/state?project=<id>`; UI at `/`.
Falls back to single-project (this repo) when no docket is present.
No external deps; mirrors revoy-spec's stdlib-webui posture.
"""
import json
import os
import re
import subprocess
import time
import http.server
import socketserver
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent  # .agents/dashboard -> .agents -> repo
INDEX = HERE / "index.html"
PORT = int(os.environ.get("SKINNER_DASH_PORT", "8770"))
DOCKET = Path(os.environ.get("SWARM_DOCKET", os.path.expanduser("~/Dev/revelri/swarm/docket.toml")))

MS_RE = re.compile(r"^# (M\d+) — (.+?)\s*$", re.M)
PHASE_RE = re.compile(r"^## (M\d+\.P\d+) — (.+?)\s*$")
STAGE_RE = re.compile(r"^### (M\d+\.P\d+\.S\d+) — (.+?)\s*$")
TODO_RE = re.compile(r"^- \*\*`(M\d+\.P\d+\.S\d+\.T\d+)`\*\* — (.+?)\s*$")
GATE_RE = re.compile(r"^- \*\*(M\d+G\d+)\*\* — (.+?)\s*$", re.M)
TODO_ID = re.compile(r"M\d+\.P\d+\.S\d+\.T\d+")


def _read(p):
    try:
        return Path(p).read_text(encoding="utf-8")
    except Exception:
        return ""


def _clip(s, n=140):
    s = re.sub(r"\s+", " ", s).strip()
    for sep in (" → *Artifact", " · *Concern", ". →"):
        if sep in s:
            s = s.split(sep)[0]
    return s[:n]


# ------------------------------------------------------------------ docket / projects
def _coerce(v):
    v = v.strip()
    if v.startswith('"') and v.endswith('"'):
        return v[1:-1]
    if v in ("true", "false"):
        return v == "true"
    return v


def projects():
    """Enrolled members from the docket, else a single-project fallback (this repo)."""
    if DOCKET.exists():
        out, cur = [], None
        for raw in _read(DOCKET).splitlines():
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            line = line.split(" #", 1)[0].rstrip()
            if line == "[[project]]":
                cur = {}
                out.append(cur)
                continue
            if line.startswith("[") or cur is None:
                continue
            if "=" in line:
                k, v = line.split("=", 1)
                cur[k.strip()] = _coerce(v)
        projs = [{"id": p.get("id"), "repo": os.path.expanduser(str(p.get("repo", ""))),
                  "status": p.get("status", "?"), "workflow": p.get("workflow", "dev")}
                 for p in out if p.get("id")]
        if projs:
            return projs
    return [{"id": REPO.name.lower().replace(" ", "-"), "repo": str(REPO), "status": "active", "workflow": "dev"}]


def _resolve(pid=None):
    ps = projects()
    if pid:
        for p in ps:
            if p["id"] == pid:
                return p
    return ps[0]


# ------------------------------------------------------------------ per-repo readers
def done_set(ledger):
    out = set()
    for line in _read(ledger).splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            e = json.loads(line)
        except Exception:
            continue
        if e.get("type") == "done" and e.get("todo"):
            out.add(e["todo"])
    return out


def control(state):
    txt = _read(state)
    cm = re.search(r"CURRENT_MILESTONE:\s*(M\d+)", txt)
    ph = re.search(r"MILESTONE_PHASE:\s*(\w+)", txt)
    return (cm.group(1) if cm else "M1"), (ph.group(1) if ph else "NORMAL")


def last_audit(txt):
    ms = re.search(r"##\s*(M\d+)\s+AUDIT[^\n]*VERDICT:\s*(PASS|FAIL)", txt)
    if ms:
        return {"milestone": ms.group(1), "verdict": ms.group(2)}
    m = re.search(r"VERDICT:\s*(PASS|FAIL)", txt)
    return {"milestone": None, "verdict": m.group(1)} if m else None


def gate_status(txt, milestone):
    out = {}
    pat = re.compile(rf"({milestone}G\d+)\b[^\n]{{0,80}}?\b(PASS|FAIL|NOT MET|PENDING|MET)\b")
    for m in pat.finditer(txt):
        out.setdefault(m.group(1), m.group(2))
    return out


def parse_active(section, done):
    phases = []
    cur_phase = cur_stage = None
    for line in section.splitlines():
        pm, sm, tm = PHASE_RE.match(line), STAGE_RE.match(line), TODO_RE.match(line)
        if pm:
            cur_phase = {"id": pm.group(1), "title": pm.group(2), "stages": []}
            phases.append(cur_phase)
            cur_stage = None
        elif sm:
            if cur_phase is None:
                cur_phase = {"id": sm.group(1).rsplit(".", 1)[0], "title": "", "stages": []}
                phases.append(cur_phase)
            cur_stage = {"id": sm.group(1), "title": sm.group(2), "todos": []}
            cur_phase["stages"].append(cur_stage)
        elif tm:
            if cur_stage is None:
                if cur_phase is None:
                    cur_phase = {"id": tm.group(1).rsplit(".", 2)[0], "title": "", "stages": []}
                    phases.append(cur_phase)
                cur_stage = {"id": tm.group(1).rsplit(".", 1)[0], "title": "", "todos": []}
                cur_phase["stages"].append(cur_stage)
            cur_stage["todos"].append(
                {"id": tm.group(1), "desc": _clip(tm.group(2)), "done": tm.group(1) in done})
    for ph in phases:
        for st in ph["stages"]:
            d = sum(1 for t in st["todos"] if t["done"])
            st["done"], st["total"] = d, len(st["todos"])
            st["status"] = "done" if st["total"] and d == st["total"] else ("active" if d else "pending")
        d = sum(s["done"] for s in ph["stages"])
        t = sum(s["total"] for s in ph["stages"])
        ph["done"], ph["total"] = d, t
        ph["status"] = "done" if t and d == t else ("active" if d else "pending")
    return phases


def commits(repo, n=18):
    try:
        out = subprocess.run(["git", "-C", str(repo), "log", f"-{n}", "--pretty=%h%x1f%s%x1f%cr"],
                             capture_output=True, text=True, timeout=6).stdout
    except Exception:
        return []
    res = []
    for line in out.splitlines():
        parts = line.split("\x1f")
        if len(parts) == 3:
            h, s, rel = parts
            m = re.match(r"(feat|fix|chore|docs|test|refactor|ci|perf)", s)
            res.append({"hash": h, "subject": s, "rel": rel, "type": m.group(1) if m else "other"})
    return res


def tick_status(repo):
    lock = Path(repo) / ".agents" / "locks" / "RUN.lock"
    if lock.exists():
        try:
            age = int(time.time() - lock.stat().st_mtime)
        except Exception:
            age = None
        return {"active": True, "age_s": age}
    return {"active": False, "age_s": None}


def worktrees(repo):
    try:
        out = subprocess.run(["git", "-C", str(repo), "worktree", "list", "--porcelain"],
                             capture_output=True, text=True, timeout=5).stdout
    except Exception:
        return []
    return [line.split("concern/", 1)[1].strip()
            for line in out.splitlines() if line.startswith("branch ") and "concern/" in line]


def recent_done(ledger, n=8):
    evs = []
    for line in _read(ledger).splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            e = json.loads(line)
        except Exception:
            continue
        if e.get("type") == "done":
            evs.append(e)
    evs = [e for e in evs if e.get("source") != "backfill"]
    evs.sort(key=lambda e: e.get("ts", ""))
    return [{"todo": e.get("todo"), "concern": e.get("concern", ""), "ts": e.get("ts"),
             "commit": e.get("commit", "")[:8], "cmd": (e.get("verified_by") or {}).get("cmd", "")}
            for e in evs[-n:][::-1]]


def log_tail(repo, n=14):
    lines = [l for l in _read(Path(repo) / ".agents" / "tick.log").splitlines() if l.strip()]
    return lines[-n:]


def build_state(pid=None):
    proj = _resolve(pid)
    repo = Path(proj["repo"])
    roadmap, ledger, state = repo / "ROADMAP.md", repo / ".agents" / "ledger.jsonl", repo / ".agents" / "STATE.md"
    road, st_txt = _read(roadmap), _read(state)
    done = done_set(ledger)
    cur, phase = control(state)
    heads = list(MS_RE.finditer(road))
    ids = [h.group(1) for h in heads]
    titles = {h.group(1): h.group(2) for h in heads}
    sections = {}
    for i, h in enumerate(heads):
        end = heads[i + 1].start() if i + 1 < len(heads) else len(road)
        sections[h.group(1)] = road[h.end():end]
    cur_idx = ids.index(cur) if cur in ids else 0

    milestones = []
    for i, mid in enumerate(ids):
        todos = [t for t in set(TODO_ID.findall(sections[mid])) if t.startswith(mid + ".")]
        d = sum(1 for t in todos if t in done)
        status = "done" if i < cur_idx else ("active" if i == cur_idx else "pending")
        milestones.append({"id": mid, "title": titles[mid], "status": status,
                           "todos_done": d, "todos_total": len(todos), "is_last": i == len(ids) - 1})

    phase_detail = {"NORMAL": "building", "AUDIT": "auditing", "REMEDIATION": "remediation"}.get(phase, phase.lower())
    active = None
    if cur in sections:
        active = {"id": cur, "title": titles.get(cur, ""), "phase": phase, "phase_detail": phase_detail,
                  "phases": parse_active(sections[cur], done),
                  "gates": [{"id": g, "desc": _clip(dd, 120), "status": gate_status(st_txt, cur).get(g, "unknown")}
                            for g, dd in GATE_RE.findall(sections[cur])]}

    return {
        "project": proj["id"], "workflow": proj.get("workflow", "dev"), "project_status": proj.get("status"),
        "current_milestone": cur, "phase": phase, "phase_detail": phase_detail,
        "last_audit": last_audit(st_txt), "milestones": milestones, "active": active,
        "commits": commits(repo),
        "live": {"tick": tick_status(repo), "working": worktrees(repo),
                 "recent_done": recent_done(ledger), "log_tail": log_tail(repo)},
        "totals": {"done": len(done), "milestones": len(ids), "current_index": cur_idx + 1},
    }


def projects_summary():
    out = []
    for p in projects():
        try:
            cur, phase = control(Path(p["repo"]) / ".agents" / "STATE.md")
            dn = len(done_set(Path(p["repo"]) / ".agents" / "ledger.jsonl"))
        except Exception:
            cur, phase, dn = "M1", "NORMAL", 0
        out.append({"id": p["id"], "status": p["status"], "workflow": p["workflow"],
                    "milestone": cur, "phase": phase, "done": dn})
    return out


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, code, body, ctype):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body if isinstance(body, bytes) else body.encode("utf-8"))

    def do_GET(self):
        from urllib.parse import urlparse, parse_qs
        u = urlparse(self.path)
        path, q = u.path, parse_qs(u.query)
        try:
            if path == "/api/projects":
                self._send(200, json.dumps(projects_summary()), "application/json")
            elif path == "/api/state":
                self._send(200, json.dumps(build_state(q.get("project", [None])[0])), "application/json")
            elif path in ("/", "/index.html"):
                self._send(200, _read(INDEX) or "<h1>index.html missing</h1>", "text/html; charset=utf-8")
            elif path == "/healthz":
                self._send(200, "ok", "text/plain")
            else:
                self._send(404, "not found", "text/plain")
        except Exception as e:
            self._send(500, json.dumps({"error": str(e)}), "application/json")


class DashServer(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def main():
    with DashServer(("0.0.0.0", PORT), Handler) as srv:
        print(f"skinner-dashboard serving on 0.0.0.0:{PORT} (docket {DOCKET if DOCKET.exists() else 'none → single-project'})", flush=True)
        srv.serve_forever()


if __name__ == "__main__":
    main()
