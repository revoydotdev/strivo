/**
 * Throwaway P-256 test keypairs — NOT used for anything but the test
 * suite. Never wire these into a deployment; `wrangler secret put
 * JWT_PRIVATE_KEY` always takes a freshly generated key (see
 * scripts/gen-keypair.sh).
 *
 * Two private-key encodings are included because `src/jwt.ts` supports
 * both and the test suite exercises both code paths:
 *   - PKCS8 ("BEGIN PRIVATE KEY"), from `openssl genpkey`.
 *   - SEC1  ("BEGIN EC PRIVATE KEY"), from `openssl ecparam -genkey`,
 *     which jwt.ts converts to PKCS8 before handing it to Web Crypto.
 */

export const TEST_PRIVATE_KEY_PKCS8 = `-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgnLOXdcF20szvZzY+
pygntLynjAzilGth5tufETr9k/uhRANCAAQYfhwu02EW5O2mBw8/4PvQ+lVxzfD7
c2WlYL4ceToyyJGJP5N1/MOtW8lODT8EfkNir9xls+XCMWD8sKQH96VS
-----END PRIVATE KEY-----`;

export const TEST_PUBLIC_KEY_PKCS8 = `-----BEGIN PUBLIC KEY-----
MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEGH4cLtNhFuTtpgcPP+D70PpVcc3w
+3NlpWC+HHk6MsiRiT+TdfzDrVvJTg0/BH5DYq/cZbPlwjFg/LCkB/elUg==
-----END PUBLIC KEY-----`;

export const TEST_PRIVATE_KEY_SEC1 = `-----BEGIN EC PRIVATE KEY-----
MHcCAQEEILbS7OVOt+Rphs8buZk1QbK2HsKFYWen08LO4A01IkmboAoGCCqGSM49
AwEHoUQDQgAEn6NqFZEkaj80aM+Z8pNI9aLotSoVyJk0l9K1WM5K8EaBBSBvQKSb
P/6BzUr3n72Cm6mr5zmAF4DK7qoYcGOkTw==
-----END EC PRIVATE KEY-----`;

export const TEST_PUBLIC_KEY_SEC1 = `-----BEGIN PUBLIC KEY-----
MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEn6NqFZEkaj80aM+Z8pNI9aLotSoV
yJk0l9K1WM5K8EaBBSBvQKSbP/6BzUr3n72Cm6mr5zmAF4DK7qoYcGOkTw==
-----END PUBLIC KEY-----`;

/** Fixed HMAC secret for webhook signature tests. */
export const TEST_WEBHOOK_SECRET = "test-webhook-secret-do-not-use-in-prod";
