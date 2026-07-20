use rand::Rng;

use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// One signed browser session. (W3.)
///
/// The cookie value is `<base64(payload)>.<base64(hmac)>`. The payload
/// carries an expiry timestamp; the HMAC is over the payload with
/// `WebConfig.session_secret`.
#[derive(Debug, Clone)]
pub struct SessionToken {
    pub expires_at: u64, // unix seconds
}

impl SessionToken {
    /// Build a new token valid for `ttl_secs` from now.
    pub fn new(ttl_secs: u64) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            expires_at: now + ttl_secs,
        }
    }

    /// Encode and sign the token with the given secret. Result is a
    /// cookie-safe string.
    pub fn encode(&self, secret: &str) -> String {
        let payload = format!("{}", self.expires_at);
        let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&payload);
        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC takes any key size");
        mac.update(payload_b64.as_bytes());
        let sig = mac.finalize().into_bytes();
        let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig);
        format!("{payload_b64}.{sig_b64}")
    }

    /// Parse a cookie value, verify the HMAC, and check the expiry.
    /// Returns `None` on any failure — the caller treats every failure
    /// shape identically (401).
    pub fn decode_verify(cookie_value: &str, secret: &str) -> Option<Self> {
        let (payload_b64, sig_b64) = cookie_value.split_once('.')?;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
        mac.update(payload_b64.as_bytes());
        let expected_sig = mac.finalize().into_bytes();
        let actual_sig = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(sig_b64)
            .ok()?;
        if expected_sig.as_slice() != actual_sig.as_slice() {
            return None;
        }
        let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload_b64)
            .ok()?;
        let payload = std::str::from_utf8(&payload_bytes).ok()?;
        let expires_at: u64 = payload.parse().ok()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(u64::MAX);
        if now >= expires_at {
            return None;
        }
        Some(SessionToken { expires_at })
    }
}

/// Generate a 32-byte session secret (URL-safe base64). Persisted in
/// `[web].session_secret` so cookies survive restarts.
pub fn generate_session_secret() -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 32] = std::array::from_fn(|_| rng.gen());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// API key used both for the `X-Api-Key` header and the session cookie.
/// Generated on first run and persisted in `[web] api_key` in `config.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKey(pub String);

impl ApiKey {
    pub fn generate() -> Self {
        const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let mut rng = rand::thread_rng();
        let s: String = (0..32)
            .map(|_| {
                let i = rng.gen_range(0..CHARSET.len());
                CHARSET[i] as char
            })
            .collect();
        ApiKey(s)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Constant-time compare to mitigate timing oracles (M4.6.9). The
    /// API key is short and low-rate, but using a `==` compare on the
    /// raw bytes leaks length-prefix information — better to spend
    /// the cycles.
    pub fn matches(&self, candidate: &str) -> bool {
        let a = self.0.as_bytes();
        let b = candidate.as_bytes();
        // Use usize for the length diff: casting to u8 would alias lengths that
        // are congruent mod 256 (e.g. 32 vs 288), defeating the length guard.
        let mut diff: usize = a.len() ^ b.len();
        let n = a.len().min(b.len());
        for i in 0..n {
            diff |= (a[i] ^ b[i]) as usize;
        }
        diff == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_matches_constant_time() {
        let k = ApiKey("abc123".into());
        assert!(k.matches("abc123"));
        assert!(!k.matches("abc124"));
        assert!(!k.matches("abc1234"));
        assert!(!k.matches(""));
    }

    #[test]
    fn session_round_trip() {
        let secret = generate_session_secret();
        let tok = SessionToken::new(60);
        let encoded = tok.encode(&secret);
        let decoded = SessionToken::decode_verify(&encoded, &secret).unwrap();
        assert_eq!(decoded.expires_at, tok.expires_at);
    }

    #[test]
    fn session_bad_signature_rejected() {
        let secret = generate_session_secret();
        let tok = SessionToken::new(60);
        let encoded = tok.encode(&secret);
        // Flip a sig character.
        let mut bytes: Vec<char> = encoded.chars().collect();
        let last = bytes.len() - 1;
        bytes[last] = if bytes[last] == 'A' { 'B' } else { 'A' };
        let tampered: String = bytes.into_iter().collect();
        assert!(SessionToken::decode_verify(&tampered, &secret).is_none());
    }

    #[test]
    fn session_wrong_secret_rejected() {
        let secret_a = generate_session_secret();
        let secret_b = generate_session_secret();
        let tok = SessionToken::new(60);
        let encoded = tok.encode(&secret_a);
        assert!(SessionToken::decode_verify(&encoded, &secret_b).is_none());
    }

    #[test]
    fn session_expired_rejected() {
        let secret = generate_session_secret();
        // TTL 0 → immediately expired.
        let tok = SessionToken::new(0);
        let encoded = tok.encode(&secret);
        // Sleep 1s to clear the second boundary.
        std::thread::sleep(std::time::Duration::from_secs(1));
        assert!(SessionToken::decode_verify(&encoded, &secret).is_none());
    }
}
