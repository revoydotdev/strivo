# Swarm State Ledger

> Append-only-ish ledger the supervisor maintains each tick. Keep newest at top.
> Status keys: CLAIMED · IN_PROGRESS · DONE · BLOCKED · GATE-FAILED

<!-- CONTROL: machine-read; supervisor updates these two lines -->
- MILESTONE_PHASE: NORMAL
- CURRENT_MILESTONE: M1

## tick 2026-07-19b — NORMAL (M1, first feature tick)
Preflight CLEAN; no governance directives / operator messages. First NORMAL tick of
M1 (0 done) → **gate-decomposition:** appended explicit gate-closing todos
`M1.P9.S1.T1` (M1G1) / `M1.P9.S1.T2` (M1G2) to ROADMAP+STATE; M1G3/M1G4 already owned
by feature todos (clippy-creator, licence-verify T3).

Recon findings folded into worker briefs:
- **licence-verify** (`M1.P1.S1.T1/T2/T3`): production P-256 pubkey does **not** exist
  in-repo (`licence-backend/*.pem` gitignored; backend pre-launch). Verification made
  real + fail-closed with an operator-supplied key (embedded const, `STRIVO_LICENCE_PUBKEY`
  env override); test injects an ephemeral keypair. JWT claims: `sub`=machine_hash,
  `tier`, `exp`, optional `licence_exp`. Also clears repo-wide `TODO(licence-verify)`
  (client.rs, cache.rs) for M1G4.
- **viewguard-path** (`M1.P3.S2.T1`): root cause = `src/plugin/registry.rs:153`
  pre-scopes `ctx.data_dir` to `plugins/<name>`; `viewguard/mod.rs:150` re-joins
  `plugins/viewguard` → double nest. Fix = match crunchr's `ctx.data_dir.clone()`; drop
  the two-path probe in `plugins.rs`. (Noted: `archiver/mod.rs:65` has the identical
  double-join bug — out of scope this todo, flagged for a future concern.)
- **ffprobe-cache** (`M1.P2.S1.T1`): `api.rs:135 recording_probe` shells `ffprobe` every
  call; add `(path,mtime,size)`-keyed in-process cache.

CLAIMED (3 disjoint-file concerns; clippy-creator deferred — workspace-wide, conflicts):
- CLAIMED `M1.P1.S1.T1/T2/T3` · concern licence-verify · files `crates/strivo-web/src/routes/licence.rs`, `src/licence/{client,cache}.rs`
- CLAIMED `M1.P2.S1.T1` · concern ffprobe-cache · file `crates/strivo-web/src/routes/api.rs`
- DONE `M1.P3.S2.T1` · concern viewguard-path · verified `cargo test -p strivo-web --features creator viewguard_data_path` (worker `789bca8`). Root-cause fix in `viewguard/mod.rs` (`ctx.data_dir.clone()` — registry already scopes); two-path probe dropped in `plugins.rs`. `archiver/mod.rs:65` carries the identical double-join bug — left for a future concern.


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
