# API mapping / reproduction / probing / pentesting / construction checklist

Read this when a claimed todo is tagged `api`. Picks the right mode for what's
actually known about the API right now, then names the concrete tool. Prior art:
`mitm` skill (already on this box) + `mitmproxy2swagger` (MIT) + `schemathesis`
(MIT) + gstack's `api-contract.md` review checklist (MIT, gstack Copyright (c) 2026
Garry Tan, adapted in ADR-0012). Not gstack's own `browse`/`qa` infra, not Postman's
official plugin (needs an account + OAuth, bundles 100+ MCP tools — see ADR-0013 for
why that was skipped).

## Pick a mode

| What's true right now | Mode |
|---|---|
| Behavior is unknown/undocumented, no spec exists | **Map** |
| A spec or prior capture exists and is trusted; need to exercise it again | **Reproduce** |
| A spec exists; want general correctness/edge-case coverage | **Probe** |
| A spec exists; want adversarial/security coverage (OWASP API Top 10) | **Pentest** |
| Building a new endpoint or evolving an existing one | **Build** |

## Map — undocumented behavior → OpenAPI spec

1. Capture real traffic: `mitm capture [port]` (writes mitmdump flows to
   `/tmp/mitm/flows`; point the exercising client's traffic through that proxy —
   forward-proxy for an outbound client, `mitmproxy --mode reverse:http://<target>`
   if proxying directly into the target server is more natural for this project's
   topology). Exercise the real endpoints while capturing — a capture with no
   traffic produces no spec.
2. `pip install --user mitmproxy2swagger` (first use only), then run it against the
   captured flows to generate a candidate OpenAPI 3.0 spec.
3. The tool's default workflow marks candidate paths with an `ignore:` prefix for
   human review before a second pass. Headless equivalent: read the candidate list
   yourself, decide which paths are real endpoints vs. noise (static assets,
   third-party calls incidentally captured), and re-run the second pass against
   that filtered set — this is a normal judgment call, not a blocking prompt.
4. Save the resulting spec under the project (e.g. `docs/openapi.yaml`) as the
   todo's artifact.

## Reproduce — replay known behavior

Replay previously captured requests directly (`mitm flows` to inspect/filter what
was captured, then replay the specific request(s) relevant to this todo). No new
tooling — this is what `mitm`'s existing capture/replay path is for.

## Probe — general correctness from a spec

`pip install --user schemathesis` (first use only), then run it against the
project's OpenAPI spec in its default (non-adversarial) profile. Verifies responses
actually conform to what the spec says they should — a strong artifact/verify-command
pair for a todo about API correctness.

## Pentest — adversarial coverage from a spec

Same `schemathesis` run, adversarial/negative-testing profile — generates inputs
that intentionally violate constraints (mutation/negative modes), covering
injection, broken auth, and mass-assignment classes from the OWASP API Top 10.

**Scope rule: target only the project's own local/dev instance the tick already
controls** (e.g. `localhost`) — never a production or third-party URL a config file
happens to mention. This isn't a new authorization boundary (a tick only ever has
shell access to its own dispatch worktree), just made explicit here so a worker
doesn't casually point an adversarial fuzzer at something outside its own sandbox.

## Build — author or evolve an endpoint well

No new tool — self-review the change against gstack's adapted `api-contract.md`
checklist (`.agents/checklists/security-review.md`'s sibling concerns: breaking
changes, versioning strategy, error-response consistency, rate-limiting/pagination,
and keeping the OpenAPI spec itself from drifting out of sync with what the code
actually does).

## Rules

- **Don't spawn a parallel verifier subagent per finding.** Same cost discipline as
  the security-review checklist (ADR-0011/0012) — self-verify by re-reading with a
  skeptic's eye, not by dispatching another fresh context.
- **A capture/spec with no real traffic behind it isn't a map.** Exercise the actual
  endpoints; don't hand-write a plausible-looking spec and call it discovered.
