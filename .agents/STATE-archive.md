# STATE archive — rotated out of the live STATE.md (not read by ticks)


<!-- rotated  : 1 entries + 0 intlog lines -->
## enrollment
Scaffolded into the swarm by `enroll.py` (ADR-0028). Awaiting its first tick.

<!-- rotated  : 1 entries + 0 intlog lines -->
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
- DONE `M1.P1.S1.T1/T2/T3` · concern licence-verify (worker `f075b2e`) · verified `licence_verify` + `licence_reject` + `! git grep TODO(licence-verify) -- crates/strivo-web`. Real ES256 gate in `persist_and_reply` via `jsonwebtoken`: fail-closed on missing key / bad sig / `sub`≠machine / expired `exp`/`licence_exp` — no `Licence` built and no tier granted on failure. Key operator-supplied (`STRIVO_LICENCE_PUBKEY` env / embedded `LICENCE_PUBKEY_PEM` const, empty pre-launch). M1G4 rescoped to `*.rs` (ROADMAP legitimately names the todo). **Follow-up captured `M1.P1.S1.T4`:** daemon refresh path (`src/licence/client.rs`) still trusts the payload — not covered here.
- DONE `M1.P2.S1.T1` · concern ffprobe-cache (worker `46f08ae`) · verified `cargo test -p strivo-web ffprobe_cache`. Process-global `(path,mtime,size)`→normalised-summary cache in front of the `recording_probe` ffprobe spawn; changed mtime/size invalidates; only success values cached. Manifest conflict with licence-verify (`once_cell` vs `jsonwebtoken`) resolved by unioning `Cargo.toml` + regenerating `Cargo.lock`.

**Tick result:** 3 concerns integrated onto `integration` (`eed0939` viewguard, `93c0c05` licence, this commit ffprobe); 5 todos closed (`M1.P1.S1.T1/T2/T3`, `M1.P2.S1.T1`, `M1.P3.S2.T1`). Remaining M1: `M1.P1.S1.T4` (daemon verify, new), `M1.P3.S1.T1` clippy-creator (deferred — workspace-wide), `M1.P9.S1.T1/T2` (M1G1/M1G2 build-green, audit-time). No push (no remote on `integration` worktree). MILESTONE_PHASE stays NORMAL.
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

<!-- rotated  : 1 entries + 0 intlog lines -->
## tick 2026-07-19c — NORMAL (M1)
Preflight CLEAN; worktrees clean; no governance directives / operator messages.
M1: 5→8 done. **DONE** `M1.P1.S1.T4` (licence-daemon-verify, `573b1f4`) — mirrored the
web route's ES256 `verify_licence_token` into `src/licence/client.rs` (core crate can't
depend on strivo-web); `refresh_now` now verifies signature + `sub`/`exp`/`licence_exp`
and derives tier/expiry from the verified claims before persisting, fail-closed when no
key resolves. Added `jsonwebtoken`/`p256` (unconditional — the refresh loop runs in the
default PVR binary). 4 verify tests. Closes AX-7 on the daemon path.
Gates verified on the integrated tree and recorded: **M1G1** `cargo test` ✓,
**M1G2** `cargo test --workspace --features creator` ✓ (`M1.P9.S1.T1`/`T2`). M1G4 already
clean (no `TODO(licence-verify)` in `*.rs`). Deferred `M1.P3.S1.T1` (clippy-creator, M1G3)
to its own tick: it may edit `src/licence/client.rs` (non-disjoint with T4) and warrants a
full tick now that T4 has landed. **M1 remaining: 1** (clippy-creator).
