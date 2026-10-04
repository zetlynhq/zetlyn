//! JSON Web Tokens, as far as signing in needs them and no further (FEDERATION.md, M14).
//!
//! Signed here: a world's ID tokens (EdDSA, its own Ed25519 key) and the client secret Apple asks
//! for (ES256, the developer's `.p8` key). Checked here: ID tokens from another world (EdDSA), from
//! Google and Apple (RS256), and ES256 where a provider uses it. Nothing else: no encryption, no
//! `none`, no algorithm a token names that its key does not.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{json, Value as J};

pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn unb64(s: &str) -> Result<Vec<u8>, String> {
    URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).map_err(|_| "not base64url".to_string())
}

/// `sha256` of `bytes`, base64url: a PKCE challenge from its verifier, and a key's id.
pub fn sha256_b64(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    b64(&Sha256::digest(bytes))
}

/// A random value of 256 bits, base64url: a state, a nonce, a PKCE verifier.
pub fn random() -> String {
    let mut bytes = [0u8; 32];
    use ring::rand::SecureRandom;
    if ring::rand::SystemRandom::new().fill(&mut bytes).is_err() {
        // The system's source failing is not a reason to hand out something guessable.
        return crate::account::token();
    }
    b64(&bytes)
}

fn compact(header: &J, claims: &J) -> String {
    format!("{}.{}", b64(header.to_string().as_bytes()), b64(claims.to_string().as_bytes()))
}

/// An Ed25519 key's public half as a JWK, its id the hash of that half.
pub fn ed25519_jwk(public: &str) -> Result<J, String> {
    let raw = crate::key::bytes(public)?;
    Ok(json!({ "kty": "OKP", "crv": "Ed25519", "x": b64(&raw), "kid": sha256_b64(&raw)[..16], "alg": "EdDSA", "use": "sig" }))
}

/// Signed with the Ed25519 key in `dir/file`, as `kid` names it.
pub fn sign_eddsa(dir: &std::path::Path, file: &str, claims: &J) -> Result<String, String> {
    let raw = std::fs::read_to_string(dir.join(file)).map_err(|e| format!("{}: {e}", dir.join(file).display()))?;
    let signing = ed25519_dalek::SigningKey::from_bytes(&crate::key::bytes(&raw)?);
    let public = format!("ed25519:{}", crate::key::hex(signing.verifying_key().as_bytes()));
    let kid = ed25519_jwk(&public)?["kid"].clone();
    let input = compact(&json!({ "alg": "EdDSA", "typ": "JWT", "kid": kid }), claims);
    use ed25519_dalek::Signer;
    Ok(format!("{input}.{}", b64(&signing.sign(input.as_bytes()).to_bytes())))
}

/// Signed ES256 with a PKCS#8 key, PEM as Apple hands it out (`-----BEGIN PRIVATE KEY-----`).
pub fn sign_es256(pem: &str, kid: &str, claims: &J) -> Result<String, String> {
    let body: String = pem.lines().filter(|l| !l.starts_with("-----")).map(str::trim).collect();
    let der = base64::engine::general_purpose::STANDARD.decode(body.as_bytes()).map_err(|_| "the key is not PEM")?;
    let rng = ring::rand::SystemRandom::new();
    let pair = ring::signature::EcdsaKeyPair::from_pkcs8(&ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING, &der, &rng).map_err(|e| format!("not a P-256 key in PKCS#8: {e}"))?;
    let input = compact(&json!({ "alg": "ES256", "kid": kid }), claims);
    let sig = pair.sign(&rng, input.as_bytes()).map_err(|e| format!("signing: {e}"))?;
    Ok(format!("{input}.{}", b64(sig.as_ref())))
}

/// The header and the claims of a token, unchecked: only to see what it says it is.
pub fn peek(token: &str) -> Result<(J, J), String> {
    let mut parts = token.split('.');
    let (h, c) = (parts.next().ok_or("not a token")?, parts.next().ok_or("not a token")?);
    let header: J = serde_json::from_slice(&unb64(h)?).map_err(|_| "the header is not JSON")?;
    let claims: J = serde_json::from_slice(&unb64(c)?).map_err(|_| "the claims are not JSON")?;
    Ok((header, claims))
}

/// The claims of a token whose signature one of `keys` (a JWKS's `keys`) holds. The key is chosen
/// by `kid`, or is the only one, and must be of the kind the token's algorithm needs.
pub fn verify(token: &str, keys: &[J]) -> Result<J, String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err("not a signed token".into());
    }
    let (header, claims) = peek(token)?;
    let alg = header["alg"].as_str().unwrap_or("");
    let key = match header["kid"].as_str() {
        Some(kid) => keys.iter().find(|k| k["kid"].as_str() == Some(kid)),
        None if keys.len() == 1 => keys.first(),
        None => None,
    }
    .ok_or("no key for that token")?;
    let input = format!("{}.{}", parts[0], parts[1]);
    let sig = unb64(parts[2])?;
    let field = |k: &str| key[k].as_str().ok_or_else(|| format!("the key has no {k}")).and_then(unb64);
    match (alg, key["kty"].as_str()) {
        ("EdDSA", Some("OKP")) => {
            let x: [u8; 32] = field("x")?.try_into().map_err(|_| "not an Ed25519 key")?;
            let vk = ed25519_dalek::VerifyingKey::from_bytes(&x).map_err(|e| e.to_string())?;
            let s: [u8; 64] = sig.try_into().map_err(|_| "not an Ed25519 signature")?;
            use ed25519_dalek::Verifier;
            vk.verify(input.as_bytes(), &ed25519_dalek::Signature::from_bytes(&s)).map_err(|_| "the signature does not hold".to_string())?;
        }
        ("RS256", Some("RSA")) => {
            let (n, e) = (field("n")?, field("e")?);
            ring::signature::RsaPublicKeyComponents { n: &n, e: &e }
                .verify(&ring::signature::RSA_PKCS1_2048_8192_SHA256, input.as_bytes(), &sig)
                .map_err(|_| "the signature does not hold".to_string())?;
        }
        ("ES256", Some("EC")) => {
            let (x, y) = (field("x")?, field("y")?);
            let mut point = vec![4u8];
            point.extend_from_slice(&x);
            point.extend_from_slice(&y);
            ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, &point)
                .verify(input.as_bytes(), &sig)
                .map_err(|_| "the signature does not hold".to_string())?;
        }
        (alg, kty) => return Err(format!("a token signed {alg} with a {} key is not one taken here", kty.unwrap_or("nameless"))),
    }
    Ok(claims)
}

/// The claims say who issued them, for whom, for how long, and in answer to what.
pub fn check(claims: &J, issuer: &str, audience: &str, nonce: Option<&str>, now: i64) -> Result<(), String> {
    if claims["iss"].as_str().map(|i| i.trim_end_matches('/')) != Some(issuer.trim_end_matches('/')) {
        return Err(format!("issued by {}, not {issuer}", claims["iss"].as_str().unwrap_or("nobody")));
    }
    let for_us = match &claims["aud"] {
        J::String(a) => a == audience,
        J::Array(all) => all.iter().any(|a| a.as_str() == Some(audience)),
        _ => false,
    };
    if !for_us {
        return Err("issued for somebody else".into());
    }
    // A minute either way, for two clocks that are not one.
    if claims["exp"].as_i64().map_or(true, |exp| exp + 60 < now) {
        return Err("expired".into());
    }
    if claims["iat"].as_i64().is_some_and(|iat| iat > now + 60) {
        return Err("issued in the future".into());
    }
    if claims["nbf"].as_i64().is_some_and(|nbf| nbf > now + 60) {
        return Err("not good yet".into());
    }
    if let Some(n) = nonce {
        if claims["nonce"].as_str() != Some(n) {
            return Err("not in answer to this sign-in".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eddsa_signed_here_is_verified_here_and_nothing_else_is() {
        let dir = std::env::temp_dir().join(format!("zetlyn-jwt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let public = crate::key::new(&dir, "oidc.key").unwrap();
        let jwk = ed25519_jwk(&public).unwrap();
        let claims = json!({ "iss": "https://a.example", "aud": "https://b.example/oauth/client.json", "sub": "reader:1", "exp": 2_000_000_000, "iat": 1_000, "nonce": "n" });
        let token = sign_eddsa(&dir, "oidc.key", &claims).unwrap();
        assert_eq!(verify(&token, std::slice::from_ref(&jwk)).unwrap()["sub"], "reader:1");
        check(&claims, "https://a.example/", "https://b.example/oauth/client.json", Some("n"), 1_500).unwrap();
        assert!(check(&claims, "https://a.example", "https://c.example", Some("n"), 1_500).unwrap_err().contains("somebody else"));
        assert!(check(&claims, "https://a.example", "https://b.example/oauth/client.json", Some("other"), 1_500).is_err());
        assert!(check(&claims, "https://a.example", "https://b.example/oauth/client.json", None, 2_000_000_100).unwrap_err().contains("expired"));
        // A changed claim, another key, and a token that says it needs no signature.
        let (h, _) = token.rsplit_once('.').unwrap();
        let mut parts: Vec<String> = token.split('.').map(str::to_string).collect();
        parts[1] = b64(json!({ "sub": "reader:2" }).to_string().as_bytes());
        assert!(verify(&parts.join("."), std::slice::from_ref(&jwk)).is_err());
        let other = ed25519_jwk(&crate::key::new(&dir, "other.key").unwrap()).unwrap();
        assert!(verify(&token, &[json!({ "kty": "OKP", "crv": "Ed25519", "x": other["x"], "kid": jwk["kid"] })]).is_err());
        let none = format!("{}.{}.", b64(br#"{"alg":"none"}"#), h.split('.').nth(1).unwrap());
        assert!(verify(&none, std::slice::from_ref(&jwk)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pkce_challenge_is_the_verifiers_hash() {
        // RFC 7636, appendix B.
        assert_eq!(sha256_b64(b"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert_ne!(random(), random());
        assert_eq!(unb64(&random()).unwrap().len(), 32);
    }
}
