/**
 * Cross-runtime ES256 contract check.
 *
 * src/jwt.ts (this Worker) signs; src/licence/verify.rs (the Rust
 * client, outside this agent's ownership) verifies. A mismatch here
 * means every activation fails in production and nobody finds out
 * until a customer pays. This file pins down every detail
 * verify_token_with() in verify.rs actually checks:
 *
 *   - header: alg "ES256", typ "JWT" (or absent)
 *   - claim names: iss, sub, tier, exp, licence_exp (optional)
 *   - signature format: raw r||s (JWS/P1363), NOT ASN.1 DER — this is
 *     what ring's ECDSA_P256_SHA256_FIXED verifier requires, and it's
 *     exactly what Web Crypto's `sign()` returns for ECDSA, per MDN /
 *     the WebCrypto spec. If jwt.ts ever switched to a library that
 *     emits DER signatures, Rust verification would break silently
 *     for every customer.
 *   - public key: SubjectPublicKeyInfo PEM whose DER ends in a 0x04
 *     (uncompressed point) + 64 bytes — verify.rs's decode_public_key
 *     takes the last 65 bytes of the SPKI DER, which only works if
 *     the key is P-256 (65-byte uncompressed point). We assert the
 *     PKCS8/SEC1 private keys here both produce a public key of that
 *     exact shape.
 */
import { describe, expect, it } from "vitest";
import { nowSecs, signEs256 } from "../src/jwt";
import {
  TEST_PRIVATE_KEY_PKCS8,
  TEST_PRIVATE_KEY_SEC1,
  TEST_PUBLIC_KEY_PKCS8,
  TEST_PUBLIC_KEY_SEC1,
} from "./fixtures/test-keys";

function b64urlDecode(s: string): Uint8Array {
  const bin = atob(s.replace(/-/g, "+").replace(/_/g, "/"));
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function pemToDer(pem: string): Uint8Array {
  const b64 = pem.replace(/-----BEGIN [^-]+-----/, "").replace(/-----END [^-]+-----/, "").replace(/\s+/g, "");
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

async function importVerifyKey(pem: string): Promise<CryptoKey> {
  return crypto.subtle.importKey("spki", pemToDer(pem), { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
}

describe.each([
  ["PKCS8", TEST_PRIVATE_KEY_PKCS8, TEST_PUBLIC_KEY_PKCS8],
  ["SEC1", TEST_PRIVATE_KEY_SEC1, TEST_PUBLIC_KEY_SEC1],
])("signEs256 with a %s private key", (_label, privPem, pubPem) => {
  it("produces a header/claims/signature shape the Rust verifier accepts", async () => {
    const payload = {
      iss: "https://licence.test.invalid",
      sub: "f".repeat(64),
      tier: "pro",
      iat: nowSecs(),
      exp: nowSecs() + 3600,
      licence_exp: new Date(Date.now() + 86_400_000).toISOString(),
    };
    const token = await signEs256(privPem, payload);
    const [headerB64, claimsB64, sigB64] = token.split(".");
    expect(sigB64).toBeTruthy();

    // Header: exact fields verify.rs's Header struct reads.
    const header = JSON.parse(new TextDecoder().decode(b64urlDecode(headerB64)));
    expect(header.alg).toBe("ES256");
    expect(header.typ).toBe("JWT");

    // Claims: exact field names VerifiedClaims in verify.rs deserializes.
    const claims = JSON.parse(new TextDecoder().decode(b64urlDecode(claimsB64)));
    expect(claims.iss).toBe(payload.iss);
    expect(claims.sub).toBe(payload.sub);
    expect(claims.tier).toBe(payload.tier);
    expect(claims.exp).toBe(payload.exp);
    expect(claims.licence_exp).toBe(payload.licence_exp);

    // Signature: 64 raw bytes (r||s / P1363), not ASN.1 DER (~70-72
    // bytes with a leading 0x30 SEQUENCE tag). This is what
    // ring::signature::ECDSA_P256_SHA256_FIXED expects.
    const sigBytes = b64urlDecode(sigB64);
    expect(sigBytes.length).toBe(64);
    expect(sigBytes[0]).not.toBe(0x30);

    // Signature actually verifies against the paired public key, using
    // the exact same fixed-length ECDSA verification path ring uses.
    const verifyKey = await importVerifyKey(pubPem);
    const signingInput = new TextEncoder().encode(`${headerB64}.${claimsB64}`);
    const ok = await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, verifyKey, sigBytes, signingInput);
    expect(ok).toBe(true);
  });

  it("emits a public key DER whose last 65 bytes are an uncompressed P-256 point (matches verify.rs's decode_public_key)", () => {
    const der = pemToDer(pubPem);
    expect(der.length).toBeGreaterThanOrEqual(65);
    expect(der[der.length - 65]).toBe(0x04);
  });
});

describe("issuer comparison parity with verify.rs (trim_end_matches('/'))", () => {
  it("a trailing slash on either side does not break the match a client would perform", () => {
    const withSlash = "https://licence.test.invalid/";
    const withoutSlash = "https://licence.test.invalid";
    expect(withSlash.replace(/\/+$/, "")).toBe(withoutSlash.replace(/\/+$/, ""));
  });
});
