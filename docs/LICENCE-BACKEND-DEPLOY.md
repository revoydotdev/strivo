# Licence backend deployment runbook

`licence-backend/` is fully coded but **has never been deployed**. This
document is the exact, ordered list of steps to take it from "code in a
repo" to "live and activating customers." Every step below requires the
account owner's own Cloudflare / Lemon Squeezy / Resend / DNS access —
none of it can be done by an agent without credentials.

Steps marked **[$]** cost money or require a paid plan. Steps marked
**[free]** don't.

Read this top to bottom once before starting; several steps produce a
value the next step needs.

## 0. Before you start

You'll need accounts on:
- **Cloudflare** (Workers + D1 + a domain/zone you control) — **[free]**
  tier covers this service's expected traffic.
- **Lemon Squeezy** (merchant of record for the $25 one-time Pro
  purchase) — **[free]** to create a store; Lemon Squeezy takes a
  transaction fee on sales, no upfront cost.
- **Resend** (transactional email) — **[free]** tier (100 emails/day,
  3,000/month) is almost certainly enough at launch.

You'll also need `node` 22+ and `npm` locally, or just use the
`deploy-licence` GitHub Actions workflow (see step 8) once secrets are
in place.

## 1. Install wrangler and log in

```bash
cd licence-backend
npm install
npx wrangler login       # opens a browser, authorizes against your Cloudflare account
```

## 2. Create the D1 database — **[free]**

```bash
npx wrangler d1 create strivo-licence
```

This prints a `database_id`. Copy it into `licence-backend/wrangler.toml`:

```toml
[[d1_databases]]
binding = "LICENCE_DB"
database_name = "strivo-licence"
database_id = "PASTE_THE_ID_HERE"   # currently "REPLACE_WITH_D1_ID"
```

Commit that change — the database id is not a secret (it's scoped to
your Cloudflare account and useless without your API token), but the
`database_id = "REPLACE_WITH_D1_ID"` placeholder currently in the repo
is what stops `wrangler deploy` targeting a real database.

## 3. Apply the schema — **[free]**

```bash
npm run schema           # wrangler d1 execute strivo-licence --file migrations/0001_init.sql --remote
```

Re-run `npm run schema:local` (which targets `--local`) any time you
want a matching local D1 for `wrangler dev`.

## 4. Generate the JWT signing keypair — **[free]**

```bash
./scripts/gen-keypair.sh /tmp/strivo-licence-keys
```

This produces `jwt-private.pem` and `jwt-public.pem` (P-256, for
ES256). Do not commit either file — the script refuses to overwrite an
existing pair and reminds you to delete the private key from disk
after it's stored as a Worker secret.

## 5. Set Worker secrets — **[free]**

```bash
npx wrangler secret put JWT_PRIVATE_KEY < /tmp/strivo-licence-keys/jwt-private.pem
npx wrangler secret put LEMONSQUEEZY_WEBHOOK_SECRET
npx wrangler secret put LEMONSQUEEZY_API_KEY   # currently unused by src/, see note below
npx wrangler secret put RESEND_API_KEY
npx wrangler secret put RESEND_FROM            # e.g. "StriVo <licence@yourdomain>"
```

`LEMONSQUEEZY_WEBHOOK_SECRET` comes from step 6. `RESEND_API_KEY` comes
from step 7.

> **Note:** `LEMONSQUEEZY_API_KEY` is declared in `src/env.ts` but not
> currently read anywhere in `src/`. `/activate` calls Lemon Squeezy's
> public, unauthenticated `/v1/licenses/validate` endpoint, which needs
> no key. Set it anyway (harmless, and future code may need the
> authenticated Admin API), but don't expect it to gate anything today.

Once the private key is stored as a secret, delete
`/tmp/strivo-licence-keys/jwt-private.pem` from disk.

## 6. Lemon Squeezy setup — **[$]** (store creation is free; Lemon Squeezy takes a cut per sale)

1. Create a Store in the Lemon Squeezy dashboard if you don't have one.
2. Create a Product for "Strivo Pro" ($25 one-time), with **license
   keys enabled** for that product's variant (Product → Licensing →
   "Create a license key for each order").
3. Note the numeric **Store ID** and **Product ID** (visible in the
   dashboard URLs or via the API) and set them in
   `licence-backend/wrangler.toml`:
   ```toml
   LEMONSQUEEZY_STORE_ID = "your numeric store id"
   LEMONSQUEEZY_PRODUCT_ID = "your numeric product id"
   ```
   Leaving these blank disables the store/product check in
   `src/lemonsqueezy.ts::validateLicenceKey` (any valid Lemon Squeezy
   key from any store would activate) — fill them in before going live.
4. Store → Settings → Webhooks → add a webhook:
   - URL: `https://<your-worker-domain>/webhook/lemonsqueezy` (the
     domain from step 9)
   - Events: at minimum `order_created`, `order_refunded`,
     `subscription_payment_refunded`
   - Copy the **Signing secret** it generates — that's
     `LEMONSQUEEZY_WEBHOOK_SECRET` from step 5.

## 7. Resend setup — **[free]** tier is fine at launch

1. Create a Resend account, verify a sending domain (or use their
   shared test domain during setup — verify a real domain before
   relying on deliverability).
2. Create an API key → that's `RESEND_API_KEY` from step 5.
3. `RESEND_FROM` must be an address on the verified domain, e.g.
   `StriVo <licence@yourdomain.com>`.

## 8. DNS / routing — **[$ if buying a new domain, otherwise free]**

Decide what domain the Worker is reachable at (e.g.
`licence.yourdomain.com`). Two options:

- **Workers.dev subdomain** (fastest, free, no DNS changes): after
  `wrangler deploy`, the Worker is live at
  `strivo-licence.<your-subdomain>.workers.dev`. Fine for launch.
- **Custom domain**: add a Worker Route or Custom Domain in the
  Cloudflare dashboard (Workers & Pages → strivo-licence → Settings →
  Domains & Routes), pointing at a zone you control in Cloudflare.

Whichever you pick, put the exact URL (scheme + host, no trailing
slash needed — the code trims one) into `PUBLIC_BASE_URL` in
`licence-backend/wrangler.toml`, replacing the current
`https://REPLACE_WITH_LICENCE_DOMAIN.example` placeholder — the old
value (`https://licence.chorosyne.com`) pointed at a retired org and
must not be reused.

**This value becomes the JWT `iss` claim.** It must exactly match (mod
trailing slash) `STRIVO_LICENCE_URL` in step 10, or every client-side
verification in `src/licence/verify.rs` fails with "JWT issuer
mismatch."

## 9. Deploy — **[free]**

Either run locally:

```bash
cd licence-backend
npm run typecheck
npm test
npx wrangler deploy
```

...or push to `main` with changes under `licence-backend/**` (or run
the workflow manually from the Actions tab): the
`.github/workflows/deploy-licence.yml` workflow does the same three
steps, gated on the `CLOUDFLARE_API_TOKEN` repo secret. Create that
token at Cloudflare dashboard → My Profile → API Tokens → "Edit
Cloudflare Workers" template (scope it to the account/zone you're
deploying into), then add it as a GitHub Actions repo secret named
`CLOUDFLARE_API_TOKEN`.

Verify it's live:

```bash
curl https://<your-worker-domain>/health
# {"ok":true,"ts":"..."}
```

## 10. Point the StriVo client at it

In the StriVo client's environment (wherever `STRIVO_LICENCE_URL` and
`STRIVO_LICENCE_PUBLIC_KEY` are configured for a release build — outside
this agent's ownership, coordinate with whoever owns client config/CI):

```
STRIVO_LICENCE_URL=https://<your-worker-domain>       # must match PUBLIC_BASE_URL exactly (mod trailing slash)
STRIVO_LICENCE_PUBLIC_KEY=-----BEGIN PUBLIC KEY-----
...contents of jwt-public.pem from step 4, verbatim...
-----END PUBLIC KEY-----
```

`src/licence/verify.rs` (client) fails closed if either is absent —
StriVo just runs in the free tier, no crash. Both must be set for
Pro activation to work at all.

## 11. Smoke test end-to-end — **[free]**

1. Buy a real (or Lemon Squeezy test-mode) licence.
2. Confirm the purchase-receipt email arrives (via Resend) with the
   licence key.
3. In the StriVo client, activate with that key. Confirm Pro features
   unlock.
4. Issue a refund in Lemon Squeezy. Confirm the refund-notice email
   arrives and that the next `/refresh` (or a fresh `/activate` on
   that machine) returns 403.

## Known gaps to be aware of (not blockers, but read before relying on this)

- **Machine binding is per-(licence_key, machine_hash), not
  per-licence_key.** Nothing currently stops the same key from being
  activated on any number of distinct machines — each activation just
  re-validates against Lemon Squeezy and inserts another D1 row. This
  contradicts the "one key, one machine" language in the purchase
  email (`src/resend.ts`) and README.md. If you want real enforcement,
  either check for an existing row under a *different* machine_hash
  before allowing a new one in `activate()` (src/index.ts), or switch
  to Lemon Squeezy's authenticated `/v1/licenses/activate` endpoint
  (which enforces the product's configured `activation_limit`
  server-side) instead of the current unauthenticated `/validate`
  call.
- **`LEMONSQUEEZY_API_KEY` is declared but unused.** See the note in
  step 5.
