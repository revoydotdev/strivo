"""Semantic visual tokens for the Skinner dashboards.

Mirrors the web palette in index.html and adopts revoy-spec's visual-system
idea (ADR-083, tui-visual-system-tokens): every colour/glyph encodes a harness
*meaning* — lifecycle state, gate verdict, action-class — not a look. Raw hues
live here once; renderers (the TUI, and a future web refresh) consume the
semantic helpers, never a bare hex. Pure stdlib; no dependencies.
"""

# --- raw hues (match .agents/dashboard/index.html :root) ---
GREEN = "#3fb950"
AMBER = "#d29922"
RED = "#f85149"
RED_DIM = "#7d4a4a"
BLUE = "#58a6ff"
PURPLE = "#a371f7"
DIM = "#8b949e"
TX = "#e6edf3"
BD = "#30363d"

# --- milestone / phase / stage / todo lifecycle (build_state status) ---
# done = complete (green), active = in-progress (amber), pending = not started
# (dim red — matches the web's red-dim "stopped/non-started" treatment).
_STATE = {
    "done": (GREEN, "●"),
    "active": (AMBER, "◐"),
    "pending": (RED_DIM, "○"),
}


def state_style(s):
    return _STATE.get(s, _STATE["pending"])[0]


def state_glyph(s):
    return _STATE.get(s, _STATE["pending"])[1]


# --- gate verdicts (gate.status: PASS/MET/FAIL/NOT MET/PENDING/unknown) ---
def gate_style(v):
    v = (v or "").strip().lower()
    if v in ("pass", "met"):
        return GREEN
    if v == "fail":
        return RED
    if v in ("not met", "notmet", "pending"):
        return AMBER
    return DIM  # unknown


def gate_glyph(v):
    v = (v or "").strip().lower()
    if v in ("pass", "met"):
        return "✓"
    if v == "fail":
        return "✗"
    if v in ("not met", "notmet"):
        return "✗"
    if v == "pending":
        return "…"
    return "?"


# --- phase pill / badge (phase_detail: building/auditing/remediation) ---
def phase_style(pd):
    return {
        "building": AMBER, "normal": AMBER,
        "auditing": BLUE, "audit": BLUE,
        "remediation": RED,
    }.get((pd or "").lower(), AMBER)


# --- audit verdict ---
def verdict_style(v):
    return GREEN if (v or "").upper() == "PASS" else RED


# --- commit type accents (matches index.html .commit.<type>) ---
def commit_style(t):
    return {
        "feat": GREEN, "fix": AMBER, "docs": BLUE, "test": PURPLE,
        "perf": GREEN, "refactor": DIM, "chore": DIM, "ci": DIM,
    }.get(t, DIM)


# --- tick beacon (live activity) ---
def beacon(phase, active):
    """Return (style, label) for the running/idle beacon."""
    if not active:
        return DIM, "○ idle"
    p = (phase or "").upper()
    if p == "AUDIT":
        return BLUE, "◉ auditing"
    if p == "REMEDIATION":
        return RED, "◉ remediation"
    return GREEN, "◉ running"
