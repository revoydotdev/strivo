# Swarm State Ledger

> Append-only-ish ledger the supervisor maintains each tick. Keep newest at top.
> Status keys: CLAIMED · IN_PROGRESS · DONE · BLOCKED · GATE-FAILED

<!-- CONTROL: machine-read; supervisor updates these two lines -->
- MILESTONE_PHASE: NORMAL
- CURRENT_MILESTONE: M1

## tick 2026-07-19 — RECOVERY (no feature work)
Preflight `RECOVER:extra-worktrees;integration-behind-main`. Actioned:
- **Reconciled** `integration` onto `main` via rebase — `main` had 7 revoy/dependabot
  commits (dep migration, ffprobe cache #29) that `integration` lacked; the 3 swarm
  scaffold commits (VISION, ADRs, ROADMAP rewrite, `.agents/`) now sit on top of
  `744af45`. Sole conflict: `ROADMAP.md` — took the swarm rewrite (ADR-0005 supersedes
  the revoy block; both outstanding revoy todos — licence ES256, Creator clippy — are
  captured as M1.P1.S1 / M1.P3.S1). `integration` is now a clean FF-able descendant of
  `main` (0 behind / 3 ahead). `main == origin/main`, nothing to push.
- No stale lock; no `concern/*` branches; 0 landed gates to re-verify.
- **NOT actioned (boundary):** the remaining `extra-worktrees` flag points at
  `/home/revelri/Dev/chorosyne/strivo` — the **primary repo** (its `.git` is the common
  dir; our swarm dir is a linked worktree) holding **23 modified/staged files + untracked
  new files** of live revoy work. `worktree-check` mislabels it "orphaned-landed / safe
  to remove" because it treats the *current* toplevel as `active`, not the primary. It is
  **not** killed-tick residue; removing it would clobber uncommitted work (and git refuses
  the primary worktree). Left untouched.
- **Harness note for operator:** with the swarm running from a linked worktree while the
  primary always coexists, `preflight.sh` (`extra-worktrees` on any `>1` worktree) will
  report `RECOVER` **every tick**, permanently blocking normal work. preflight/worktree-check
  need to exclude the primary repo worktree (by common-dir, not `--show-toplevel`) before
  normal ticks can run.

## enrollment
Scaffolded into the swarm by `enroll.py` (ADR-0028). Awaiting its first tick.
