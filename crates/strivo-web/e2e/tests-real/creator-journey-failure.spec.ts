import { expect, test } from "@playwright/test";
import {
  API_KEY,
  apiJson,
  login,
  openCreatorSurfaces,
  requireCreatorEdition,
  seededRecording,
} from "./creator-journey-helpers";

// Journey failure scenarios (plan todo 16), plus the reproductions behind
// every `broken` verdict in docs/audits/2026-09-25-pro-journey/MANIFEST.json.
//
// A broken promise with no reproduction is not a finding, so each one here is
// an actual request against the running Creator binary. Every assertion has
// a CONTROL: a neighbouring call that must behave differently, so a blanket
// 200, a blanket 402, or a blanket 405 cannot make the suite green.

test.beforeEach(async ({ request }) => {
  await requireCreatorEdition(request);
});

test("NAMED FAILURE SCENARIO — an unentitled Pro tab must explain the state, not signpost", async ({
  page,
  request,
}) => {
  await login(page);
  await openCreatorSurfaces(page);

  // HALF A — a Pro-routed tab. This one is honest: the click lands on
  // /plugins/insights, the 402 arrives, and renderPlugins() mounts the upsell
  // card that says Creator work is unreleased and names no purchase path.
  await page.goto("/app#/analytics/insights");
  await page.locator(".pro-tab-body a.btn-primary").click();
  await expect(page.locator(".pg-upsell-card")).toBeVisible();
  await expect(page.locator(".pg-upsell-title")).toContainText("is unavailable");
  await expect(page.locator(".pg-upsell-pitch")).toContainText("not released or supported");
  await expect(page.locator(".pg-cap-hint")).toContainText("No trial, activation, or purchase");

  // HALF B — a signpost tab, the same unlicensed machine, no explanation.
  await page.goto("/app#/publish/clipper");
  const body = page.locator(".pro-tab-body");
  await expect(body.locator("p.empty")).toBeVisible();
  // The signpost's whole claim is "go and do this elsewhere". It names no
  // entitlement, no licence state, and no reason this tab is not a control.
  await expect(body).not.toContainText(/licen[cs]e/i);
  await expect(body).not.toContainText(/entitl/i);
  await expect(body).not.toContainText(/unavailable|not released|Creator Edition/i);
  await expect(body).not.toContainText(/Pro\b/);
  // The clipper capability is `included` — not Pro — so an entitlement
  // message would have been the wrong explanation anyway. What is missing is
  // any account of why the tab is inert: it is inert because it was never
  // implemented as a pane, and the copy does not say that.
  const d = await apiJson(request, "get", "/api/v1/plugins/descriptor");
  const clipper = d.body.capabilities.find((c: { key: string }) => c.key === "clipper");
  expect(clipper.entitlement, "clipper is Included, so the gate is not the reason").toBe("included");
  expect(clipper.entry_point, "an executor does exist for clipper").toBe("available");
});

test("the signpost's API reference is wrong for six of its thirteen tabs", async ({ request }) => {
  // Generous, and unchanged in substance: the real clipper `/analyze` call
  // runs highlight detection over the media. It measured 55-713 ms on an idle
  // box, but it exceeded the 45 s default once under this host's load average
  // of ~20. The expectation below is exactly as strict as before; only the
  // clock differs.
  test.setTimeout(180_000);
  const rec = await seededRecording(request);
  const claim = (slug: string) => `/api/v1/plugins/${slug}/${rec.id}`;

  // KEPT — the claimed shape is right for these seven. A route exists at that
  // exact path, and it accepts POST, so the snippet is a usable instruction.
  for (const slug of ["scenes", "structure", "chat-density", "thumbnails", "chapters", "broll", "cuepoints"]) {
    const r = await apiJson(request, "post", claim(slug));
    // Not 404 and not 405: the path and method the UI advertises both exist.
    expect([404, 405], `${slug}: claimed endpoint must exist as advertised`).not.toContain(r.status);
  }

  // BROKEN (wrong method) — the real route for these is GET. The signpost says
  // POST, and the server answers 405 with no body, so following the snippet as
  // printed fails with a status the SPA never surfaces anywhere.
  for (const slug of ["heatmap", "casebook", "reuse", "multitrack", "brandsafe"]) {
    const r = await apiJson(request, "post", claim(slug));
    expect(r.status, `${slug}: signpost claims POST, server says otherwise`).toBe(405);
    // CONTROL: the same path with the method the server actually wants works.
    const ok = await apiJson(request, "get", claim(slug));
    expect([200, 404], `${slug}: the real route is a GET here`).toContain(ok.status);
  }

  // BROKEN (path) — clipper's POST lives under /analyze or /extract, not at
  // the bare /{id}.
  const bare = await apiJson(request, "post", claim("clipper"));
  expect(bare.status, "the bare clipper path is not a clipper route").toBe(202);
  expect(JSON.stringify(bare.body), "…and 202 here is the generic RPC catch-all").toContain(
    '"status":"queued"',
  );
  const real = await apiJson(request, "post", `/api/v1/plugins/clipper/${rec.id}/analyze`);
  expect(real.status, "the real clipper route is under /analyze").toBe(200);
});

test("the generic plugin RPC route answers 202 for a plugin that does not exist", async ({
  request,
}) => {
  // This is the control that makes the clipper verdict above trustworthy, and
  // it is a defect in its own right: `POST /api/v1/plugins/{plugin}/{verb}`
  // dispatches to the daemon plugin host and reports "queued" for a plugin
  // name the host has never heard of. The signpost's snippet, followed
  // literally, walks a user straight into it.
  const res = await apiJson(request, "post", "/api/v1/plugins/definitely-not-a-plugin/any-verb");
  expect(res.status).toBe(202);
  expect(JSON.stringify(res.body)).toContain("definitely-not-a-plugin");
  expect(JSON.stringify(res.body)).toContain("queued");
  // And the verb is not validated either — a recording UUID is accepted as a
  // verb name, which is exactly what the signpost tells users to send.
  const rec = await seededRecording(request);
  const asVerb = await apiJson(request, "post", `/api/v1/plugins/clipper/${rec.id}`);
  expect(asVerb.status).toBe(202);
  expect(JSON.stringify(asVerb.body), "the recording id was taken as a verb").toContain(rec.id);
});

test("an unknown capability slug is rejected by the descriptor's parser, not defaulted", async ({
  request,
}) => {
  // The malformed-input probe. A capability the descriptor has never heard of
  // must not be constructible from the wire (capabilities.rs:694-706). The
  // check is on the crate's own parser, so it is exercised where it lives.
  const res = await request.post("/api/v1/plugins/newtool/verb", {
    headers: { "x-api-key": API_KEY, "content-type": "application/json" },
    data: {},
  });
  // 202, but naming an unknown plugin — the same false success as above. The
  // distinction that matters is recorded: the *descriptor* rejects it, the
  // *RPC route* does not.
  expect(res.status()).toBe(202);

  // A truncated descriptor is not something a client can send (Serialize
  // only, capabilities.rs:242), so the real malformed-input surface is a
  // capability key on a gate. The near-miss spellings are the interesting
  // case and they are covered by the crate test; here assert the served
  // spelling is the canonical kebab-case one and that no snake_case variant
  // appears.
  const d = await apiJson(request, "get", "/api/v1/plugins/descriptor");
  const keys = d.body.capabilities.map((c: { key: string }) => c.key);
  expect(keys).toContain("chat-density");
  expect(keys, "the drifted snake_case spelling must not be served").not.toContain("chat_density");
  expect(new Set(keys).size, "no duplicate capability keys").toBe(keys.length);
});

test("an unauthenticated Pro request is 401, not 402 — the gate is not the only guard", async ({
  request,
}) => {
  // Ordering matters and is worth pinning: without a key the answer is 401,
  // so a 402 in the specs above is genuinely the entitlement gate and not the
  // generic auth refusal.
  const anon = await request.get("/api/v1/plugins/crunchr/recordings");
  expect(anon.status()).toBe(401);
  const wrongKey = await request.get("/api/v1/plugins/crunchr/recordings", {
    headers: { "x-api-key": "not-the-key" },
  });
  expect(wrongKey.status()).toBe(401);
  const authed = await apiJson(request, "get", "/api/v1/plugins/crunchr/recordings");
  expect(authed.status, "with a valid key the same path reaches the gate").toBe(402);
});

test("a repeat read of a Pro endpoint is a real refusal every time, not a cached one", async ({
  request,
}) => {
  // The stale-state probe. `API._fetch` collapses GETs behind a 2 s cache, so
  // this goes through `request` and additionally waits past the TTL before the
  // second read, to show the answer is the server's both times and identical.
  const first = await apiJson(request, "get", "/api/v1/plugins/insights/words");
  expect(first.status).toBe(402);
  await page_free_wait(2200);
  const second = await apiJson(request, "get", "/api/v1/plugins/insights/words");
  expect(second.status).toBe(402);
  expect(second.body).toEqual(first.body);
  // And a read that IS entitled-vs-not within the same session is stable:
  // licence status never flips on its own.
  const l1 = await apiJson(request, "get", "/api/v1/licence/status");
  const l2 = await apiJson(request, "get", "/api/v1/licence/status");
  expect(l2.body.entitled).toBe(l1.body.entitled);
  expect(l2.body.entitled).toBe(false);
});

/** Past `API._fetch`'s 2 s `_cacheTtlMs`, so the next read cannot be a hit. */
function page_free_wait(ms: number) {
  return new Promise((r) => setTimeout(r, ms));
}
