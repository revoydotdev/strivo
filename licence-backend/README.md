# strivo-licence

Cloudflare Workers + D1 activation backend for **Strivo Pro**.

This subtree is meant to be lifted into its own private repo at
`revoydotdev/strivo-licence` before going live — keeping it here for now
so the StriVo client and backend evolve in lockstep until the schema
stabilises.

## What it does

Three jobs:

1. **Activate** — exchange a Lemon Squeezy licence key for a
   machine-bound JWT (ES256). One key, one machine.
2. **Refresh** — re-sign the JWT every 72h so a revoked / refunded
   licence stops working within three days. Clients keep the cached
   token valid offline (no internet-kill).
3. **Trial** — 3-day token, bound to a fresh machine_hash, no payment
   required. Each machine_hash can only ever take one trial.

It also receives Lemon Squeezy webhooks (`order_created`,
`subscription_payment_refunded`) and writes them to D1 so the next
`/refresh` either issues a new token or returns 403.

## Stack

- **Cloudflare Workers** — single-region edge compute, free tier
  covers expected traffic. Native `fetch` handler, no framework.
- **Cloudflare D1** — SQLite at the edge, one binding `LICENCE_DB`.
- **Lemon Squeezy** — Merchant of Record for the $25 one-time
  purchase. Handles tax + EU VAT.
- **Resend** — transactional email (purchase receipt, refund
  notice).
- **Web Crypto** — ES256 JWT signing using a P-256 key stored as a
  Worker secret.

## Endpoints

| Method | Path                          | Auth                           | Purpose |
|--------|-------------------------------|--------------------------------|---------|
| POST   | `/activate`                   | none (rate-limited per IP)     | Lemon Squeezy key + machine_hash → JWT |
| POST   | `/refresh`                    | existing JWT in Bearer header  | Re-sign / revoke check |
| POST   | `/trial`                      | none (rate-limited per IP)     | machine_hash → 3-day JWT |
| POST   | `/webhook/lemonsqueezy`       | `X-Signature` HMAC-SHA256      | Update licence state from LS events |
| GET    | `/health`                     | none                           | Liveness for status checks |

## Local dev

```bash
cd licence-backend
npm install
cp .dev.vars.example .dev.vars  # fill secrets
npm run dev                     # wrangler dev on :8787
```

## Testing

```bash
npm test        # vitest, running the real Worker code in workerd via
                 # @cloudflare/vitest-pool-workers (Miniflare) — not a
                 # mock runtime. D1 migrations, rate limiting, and the
                 # ES256 signer all run for real; only the outbound
                 # Lemon Squeezy / Resend HTTP calls are stubbed.
npm run typecheck
```

See `test/jwt-contract.test.ts` for the cross-runtime check that a
token this backend signs is byte-shape-compatible with what
`src/licence/verify.rs` (the Rust client) requires.

## Deploy

Full first-time deployment (D1 creation, secrets, Lemon Squeezy,
Resend, DNS, client config) is documented step by step in
[`docs/LICENCE-BACKEND-DEPLOY.md`](../docs/LICENCE-BACKEND-DEPLOY.md).
Short version, once you've done that once:

```bash
npm run typecheck
npm test
npx wrangler deploy
```

CI does the same three steps automatically on push to `licence-backend/**`
via `.github/workflows/deploy-licence.yml`, gated on the
`CLOUDFLARE_API_TOKEN` repo secret.

## Key generation

```bash
./scripts/gen-keypair.sh /path/to/output/dir
```

Produces a fresh P-256 keypair for ES256. The **private** key goes into
the Worker secret `JWT_PRIVATE_KEY`; the **public** key's PEM contents
go to the StriVo client as `STRIVO_LICENCE_PUBLIC_KEY`. Never commit
either file.

The client fails closed if either `STRIVO_LICENCE_URL` or
`STRIVO_LICENCE_PUBLIC_KEY` is absent. Local self-issued trials are not
supported: trials use the same signed, once-per-machine backend flow as paid
activations.
