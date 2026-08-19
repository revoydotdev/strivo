import { SELF, env } from "cloudflare:test";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TEST_WEBHOOK_SECRET } from "./fixtures/test-keys";

/** Decode a JWT payload without verifying — used only to inspect shape. */
function decodePayload(token: string): Record<string, unknown> {
  const [, payloadB64] = token.split(".");
  const json = atob(payloadB64.replace(/-/g, "+").replace(/_/g, "/"));
  return JSON.parse(json);
}

async function hmacHex(secret: string, body: string): Promise<string> {
  const key = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(body)));
  return Array.from(mac, (b) => b.toString(16).padStart(2, "0")).join("");
}

function jsonReq(path: string, body: unknown, headers: Record<string, string> = {}): Request {
  return new Request(`https://licence.test.invalid${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", ...headers },
    body: JSON.stringify(body),
  });
}

const machineA = "a".repeat(64);
const machineB = "b".repeat(64);

// Miniflare isolates storage per *test file*, not per individual `it()` —
// setup files (which apply migrations) run once and share state across every
// test below. Clear the tables ourselves before each test so tests don't leak
// into each other via shared D1 rows.
beforeEach(async () => {
  await env.LICENCE_DB.batch([
    env.LICENCE_DB.prepare("DELETE FROM licences"),
    env.LICENCE_DB.prepare("DELETE FROM trial_claims"),
    env.LICENCE_DB.prepare("DELETE FROM webhook_events"),
    env.LICENCE_DB.prepare("DELETE FROM denied_attempts"),
  ]);
});

describe("GET /health", () => {
  it("reports ok", async () => {
    const res = await SELF.fetch("https://licence.test.invalid/health");
    expect(res.status).toBe(200);
    const body = (await res.json()) as { ok: boolean };
    expect(body.ok).toBe(true);
  });
});

describe("POST /trial", () => {
  it("issues a trial token bound to the machine", async () => {
    const res = await SELF.fetch(jsonReq("/trial", { machine_hash: machineA }));
    expect(res.status).toBe(200);
    const body = (await res.json()) as { token: string; tier: string; expires_at: string };
    expect(body.tier).toBe("trial");
    const claims = decodePayload(body.token);
    expect(claims.tier).toBe("trial");
    expect(claims.sub).toBe(machineA);
    expect(claims.licence_exp).toBe(body.expires_at);
  });

  it("refuses a second trial for the same machine", async () => {
    const first = await SELF.fetch(jsonReq("/trial", { machine_hash: machineA }));
    expect(first.status).toBe(200);
    const second = await SELF.fetch(jsonReq("/trial", { machine_hash: machineA }));
    expect(second.status).toBe(409);
  });

  it("allows different machines to each claim a trial", async () => {
    const a = await SELF.fetch(jsonReq("/trial", { machine_hash: machineA }));
    const b = await SELF.fetch(jsonReq("/trial", { machine_hash: machineB }));
    expect(a.status).toBe(200);
    expect(b.status).toBe(200);
  });
});

describe("POST /activate", () => {
  const originalFetch = globalThis.fetch;

  afterEach(() => {
    globalThis.fetch = originalFetch;
    vi.unstubAllGlobals();
  });

  function mockLemonValidate(valid: boolean, extra: Record<string, unknown> = {}) {
    const spy = vi.fn(async (input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input.toString();
      if (url.includes("lemonsqueezy.com/v1/licenses/validate")) {
        return new Response(
          JSON.stringify({
            valid,
            license_key: { status: valid ? "active" : "disabled" },
            meta: { store_id: 123, product_id: 456, customer_email: "buyer@example.com" },
            ...extra,
          }),
          { status: 200 },
        );
      }
      return new Response("not mocked", { status: 500 });
    });
    vi.stubGlobal("fetch", spy);
    return spy;
  }

  it("rejects a missing body", async () => {
    const res = await SELF.fetch(jsonReq("/activate", {}));
    expect(res.status).toBe(400);
  });

  it("rejects an invalid licence key", async () => {
    mockLemonValidate(false);
    const res = await SELF.fetch(jsonReq("/activate", { licence_key: "bad-key", machine_hash: machineA }));
    expect(res.status).toBe(403);
  });

  it("activates a valid key and binds it to the machine", async () => {
    const spy = mockLemonValidate(true);
    const res = await SELF.fetch(jsonReq("/activate", { licence_key: "good-key", machine_hash: machineA }));
    expect(res.status).toBe(200);
    const body = (await res.json()) as { token: string; tier: string };
    expect(body.tier).toBe("pro");
    const claims = decodePayload(body.token);
    expect(claims.tier).toBe("pro");
    expect(claims.sub).toBe(machineA);
    expect(spy).toHaveBeenCalledTimes(1);

    const row = await env.LICENCE_DB.prepare(
      "SELECT * FROM licences WHERE licence_key = ?1 AND machine_hash = ?2",
    )
      .bind("good-key", machineA)
      .first();
    expect(row).toBeTruthy();
  });

  it("re-activation of an already-activated (key, machine) pair skips Lemon Squeezy and just re-signs", async () => {
    const spy = mockLemonValidate(true);
    const first = await SELF.fetch(jsonReq("/activate", { licence_key: "good-key", machine_hash: machineA }));
    expect(first.status).toBe(200);
    expect(spy).toHaveBeenCalledTimes(1);

    const second = await SELF.fetch(jsonReq("/activate", { licence_key: "good-key", machine_hash: machineA }));
    expect(second.status).toBe(200);
    // Lemon Squeezy is not re-queried for an already-bound (key, machine) pair.
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it("machine rebinding: the same key activated from a second machine creates a second, independent row", async () => {
    // NOTE: the backend enforces uniqueness on (licence_key, machine_hash),
    // not on licence_key alone. Nothing here stops a leaked key from being
    // activated on any number of distinct machines — each call simply
    // re-validates against Lemon Squeezy and inserts another row. This
    // contradicts the "one key, one machine" claim in README.md /
    // resend.ts's purchase-receipt copy. Flagged for the owner; not fixed
    // here since it's a product/business-logic decision (e.g. whether to
    // rely on Lemon Squeezy's own `activation_limit` via the `/activate`
    // licence-API endpoint instead of `/validate`).
    const spy = mockLemonValidate(true);
    const onA = await SELF.fetch(jsonReq("/activate", { licence_key: "shared-key", machine_hash: machineA }));
    const onB = await SELF.fetch(jsonReq("/activate", { licence_key: "shared-key", machine_hash: machineB }));
    expect(onA.status).toBe(200);
    expect(onB.status).toBe(200);
    expect(spy).toHaveBeenCalledTimes(2);

    const rows = await env.LICENCE_DB.prepare(
      "SELECT machine_hash FROM licences WHERE licence_key = ?1",
    )
      .bind("shared-key")
      .all();
    expect(rows.results.length).toBe(2);
  });
});

describe("POST /refresh", () => {
  const originalFetch = globalThis.fetch;
  afterEach(() => {
    globalThis.fetch = originalFetch;
    vi.unstubAllGlobals();
  });

  it("404s when there's no licence on file", async () => {
    const res = await SELF.fetch(jsonReq("/refresh", { licence_key: "nope", machine_hash: machineA }));
    expect(res.status).toBe(404);
  });

  it("re-signs an active licence", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response(
            JSON.stringify({
              valid: true,
              license_key: { status: "active" },
              meta: { store_id: 123, product_id: 456, customer_email: "buyer@example.com" },
            }),
            { status: 200 },
          ),
      ),
    );
    await SELF.fetch(jsonReq("/activate", { licence_key: "refresh-key", machine_hash: machineA }));
    const res = await SELF.fetch(jsonReq("/refresh", { licence_key: "refresh-key", machine_hash: machineA }));
    expect(res.status).toBe(200);
    const body = (await res.json()) as { token: string; tier: string };
    expect(body.tier).toBe("pro");
  });

  it("403s a revoked licence", async () => {
    const now = new Date().toISOString();
    await env.LICENCE_DB.prepare(
      `INSERT INTO licences (licence_key, machine_hash, tier, email, activated_at, last_refreshed, expires_at, revoked_at)
       VALUES (?1, ?2, 'pro', NULL, ?3, ?3, NULL, ?3)`,
    )
      .bind("revoked-key", machineA, now)
      .run();
    const res = await SELF.fetch(jsonReq("/refresh", { licence_key: "revoked-key", machine_hash: machineA }));
    expect(res.status).toBe(403);
  });
});

describe("POST /webhook/lemonsqueezy", () => {
  const originalFetch = globalThis.fetch;
  afterEach(() => {
    globalThis.fetch = originalFetch;
    vi.unstubAllGlobals();
  });

  function orderCreatedPayload(eventId: string, key: string, email: string) {
    return JSON.stringify({
      meta: { event_name: "order_created", webhook_id: eventId },
      data: {
        id: "order-1",
        attributes: {
          first_order_item: { license_key: { key } },
          user_email: email,
        },
      },
    });
  }

  it("rejects a missing signature", async () => {
    const raw = orderCreatedPayload("evt-1", "k", "e@example.com");
    const res = await SELF.fetch(
      new Request("https://licence.test.invalid/webhook/lemonsqueezy", { method: "POST", body: raw }),
    );
    expect(res.status).toBe(401);
  });

  it("rejects an invalid signature", async () => {
    const raw = orderCreatedPayload("evt-2", "k", "e@example.com");
    const res = await SELF.fetch(
      new Request("https://licence.test.invalid/webhook/lemonsqueezy", {
        method: "POST",
        headers: { "X-Signature": "0".repeat(64) },
        body: raw,
      }),
    );
    expect(res.status).toBe(401);
  });

  it("accepts a valid signature, records the event, and emails the receipt", async () => {
    const raw = orderCreatedPayload("evt-3", "new-key", "buyer@example.com");
    const sig = await hmacHex(TEST_WEBHOOK_SECRET, raw);
    const mailSpy = vi.fn(async (_input: string | URL | Request, _init?: RequestInit) => new Response("{}", { status: 200 }));
    vi.stubGlobal("fetch", mailSpy);

    const res = await SELF.fetch(
      new Request("https://licence.test.invalid/webhook/lemonsqueezy", {
        method: "POST",
        headers: { "X-Signature": sig },
        body: raw,
      }),
    );
    expect(res.status).toBe(200);
    expect(mailSpy).toHaveBeenCalledTimes(1);
    const [url, init] = mailSpy.mock.calls[0] as [string, RequestInit];
    expect(url).toBe("https://api.resend.com/emails");
    expect(JSON.parse(init.body as string).to).toBe("buyer@example.com");

    const stored = await env.LICENCE_DB.prepare("SELECT * FROM webhook_events WHERE event_id = ?1")
      .bind("evt-3")
      .first();
    expect(stored).toBeTruthy();
  });

  it("revokes the licence on refund and a subsequent refresh is denied", async () => {
    const now = new Date().toISOString();
    await env.LICENCE_DB.prepare(
      `INSERT INTO licences (licence_key, machine_hash, tier, email, activated_at, last_refreshed, expires_at, revoked_at)
       VALUES (?1, ?2, 'pro', 'buyer@example.com', ?3, ?3, NULL, NULL)`,
    )
      .bind("refund-key", machineA, now)
      .run();

    const raw = JSON.stringify({
      meta: { event_name: "order_refunded", webhook_id: "evt-4" },
      data: {
        id: "order-2",
        attributes: {
          first_order_item: { license_key: { key: "refund-key" } },
          user_email: "buyer@example.com",
        },
      },
    });
    const sig = await hmacHex(TEST_WEBHOOK_SECRET, raw);
    vi.stubGlobal("fetch", vi.fn(async () => new Response("{}", { status: 200 })));

    const res = await SELF.fetch(
      new Request("https://licence.test.invalid/webhook/lemonsqueezy", {
        method: "POST",
        headers: { "X-Signature": sig },
        body: raw,
      }),
    );
    expect(res.status).toBe(200);

    const refresh = await SELF.fetch(jsonReq("/refresh", { licence_key: "refund-key", machine_hash: machineA }));
    expect(refresh.status).toBe(403);
  });
});

describe("rate limiting", () => {
  it("returns 429 once the per-IP/route limit (30/60s per wrangler.toml) is exceeded", async () => {
    const ip = "203.0.113.7";
    let lastStatus = 0;
    for (let i = 0; i < 31; i++) {
      const res = await SELF.fetch(
        jsonReq("/trial", { machine_hash: `${ip.replace(/\./g, "")}-${i}` }, { "CF-Connecting-IP": ip }),
      );
      lastStatus = res.status;
    }
    expect(lastStatus).toBe(429);
  });
});
