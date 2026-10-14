# Keel migration postmortem: Strivo (2026-10-14)

Migrated at HEAD 6a2cd1c by a fresh agent following the `keel-migrate` skill, then audited.
Every number here was measured on that HEAD or read from the file cited.

## What the legacy claimed, against the repo

- **"58 of the 104 todos are done; the gate is green on main"** (ROADMAP.md, "Current position
  and next steps (2026-10-13)"). The 58 holds. `cargo fmt`, both clippy lanes and all four cargo
  test lanes pass (1,658 workspace tests, 295/299 Creator). The browser mock lane does not: `npm
  test` in `crates/strivo-web/e2e` fails 9 of 353 deterministically, twice by the agent and once
  by the auditor: a11y keyboard and scan on `#/pipelines`, `player-controller.spec.ts:336`, and
  six ux-budget cases on `#/pipelines`, `#/plugins`, `#/archive`, `#/dataviz`. "Green" is the
  cargo gate only. Todos 1-A3, 1-A4, 1-A5, 1-B4 and 1-B5 carry it.
- **Six `Commit:` lines in the .omo plan name subjects that never landed** (plan lines 399, 409,
  419, 429, 477, 959 against c631486, 4324ded, beefa8e, 1cc5ef1, bfdecef, a4e90ec). Done todos
  now cite the hash with `git merge-base --is-ancestor`, which a reword cannot break.
- **CONTRIBUTING.md lists three gate commands; CI runs at least nine more** (ci.yml lines 157,
  162, 165, 173, 211, 217, 232, 239, 258). AGENTS.md, which is untracked, is closer to CI.
- **The ledger's F-12 count is 14 of 20 panes at `route: null`; the file has 15** (021-creator.js).
- **Stale pointers in the plan:** `CLAUDE.md` is `AGENTS.md` since c182310; `src/edl/` moved to
  `crates/editor/`; most strivo-web paths lack the `crates/strivo-web/` prefix.
- **Releases:** v0.7.0 and v0.8.0 are tagged, no GitHub release beyond v0.6.0 (gh release list).
- **Licence backend:** `wrangler.toml:11` still `REPLACE_WITH_D1_ID`; no workflow references
  wrangler. Carried as 2-A1 with an `owner:` line (create the D1 database).
- **Held as claimed:** F-11, F-38, WS/V05, the 90-row consolidated audit register.
- **Environment trap:** with a long `TMPDIR`, three `ipc_handshake` tests fail on `SUN_LEN`. CI
  avoids it with `$RUNNER_TEMP`; nothing tracked says so. Todos 1-A1 and 2-C7 carry it.

## The plan

6 milestones, 19 phases, 129 todos: 58 landed (recorded done in the ledger, qa by commit
ancestry), 71 open, one `owner:` act. M1 is the in-flight Wave 8 plus the repo's own claims;
M2 the seventeen revoy rows re-derived against HEAD; M3 to M6 the legacy Waves 9, 10, 11 and
6b to 11b, deferred, not dropped. Four to seven lanes per milestone. `keel qa` reports zero
vacuous open todos and zero stale done ones. `keel coverage` over four weeks: 109 commits, 2%
uncovered, every uncovered row dispositioned (shipped PVR work of 2026-09-18 and dependabot
merges), floor 3%.

## Where the first pass went wrong, and what changed in the tooling

- A 14-day window held 9 commits, so the fraction floor was zero-tolerance. `keel validate
  --activity` now warns under twenty commits; the skill says to widen the window.
- Every milestone had one lane, which serialized 70 todos. `keel validate` now warns on a
  single-lane milestone with six or more todos.
- Done todos' qa grepped commit subjects. The skill now prescribes `merge-base --is-ancestor`.
- Two open todos had a qa that already passed. `keel qa` now exists to catch that; the skill
  requires every open todo's qa to fail today.
- Landed work looked uncovered because `coverage` counted only open todos. Paths only a done
  todo touches are now `history`.
- `keel dag` listed a todo both as a step and as waiting. Fixed.
- The .omo `boulder.json` had none of the keys the skill's reference assumed. The reference now
  says the shape varies and where the state is when they are absent.

## Judgement calls for the owner

- 95 `needs` edges were added from lane overlap by directory prefix; 25 of them order open
  todos behind each other (1-B1 waits on the two e2e fixes and the two Creator gates because
  they touch the same SPA files). Loosen them if the ordering is wrong.
- Eleven uncovered rows are dispositioned `ignore` as shipped PVR work outside the Creator plan.
  Any of them could be a todo instead.
- The plan was rendered by a script (kept as `render_plan_2026-10-14.py` here) and is
  hand-maintained from this commit on.
