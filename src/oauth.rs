//! Signing in with Google or Apple (OpenID Connect, the authorization code flow).
//!
//! The person goes to the provider with a fresh `state` and `nonce`, both also kept in a cookie of
//! ten minutes; they come back with a code, which this app trades for an ID token directly with the
//! provider, over TLS. Of the token's claims it checks the issuer, that it is meant for this app,
//! that it is fresh, that it carries the nonce, and that the address is verified; the address is
//! the account, as with a link by mail. Nothing else of the provider's is kept.
//!
//! Google: `GOOGLE_CLIENT_FILE` (the JSON its console gives; `secrets/google-client.json` when it is
//! not set and that file is there), or `GOOGLE_CLIENT_ID` and
//! `GOOGLE_CLIENT_SECRET`. Apple: `APPLE_CLIENT_ID` (the Services ID),
//! `APPLE_TEAM_ID`, `APPLE_KEY_ID`, `APPLE_KEY_FILE` (the `.p8`); Apple's client secret is a JWT
//! this app signs with that key. Without them, no button.

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use serde_json::Value as J;

/// Where a checkout keeps Google's client file for trying things locally (git ignores it).
const LOCAL_GOOGLE: &str = "secrets/google-client.json";

pub struct OAuth {
    google: Option<Google>,
    apple: Option<Apple>,
}

struct Google {
    id: String,
    secret: String,
}

struct Apple {
    id: String,
    team: String,
    key_id: String,
    /// The private key, PKCS#8 DER.
    key: Vec<u8>,
}

/// Who came back: an address the provider says is theirs.
#[derive(Debug)]
pub struct Person {
    pub email: String,
}

impl OAuth {
    pub fn from_env() -> OAuth {
        let var = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        // The file Google's console gives to download, or its two values on their own.
        let file = var("GOOGLE_CLIENT_FILE").or_else(|| Some(LOCAL_GOOGLE.to_string()).filter(|f| std::path::Path::new(f).is_file()));
        let google = match file {
            Some(file) => {
                let read = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str::<J>(&t).ok()).and_then(|j| {
                    let c = j.get("web").or_else(|| j.get("installed"))?;
                    Some(Google { id: c["client_id"].as_str()?.to_string(), secret: c["client_secret"].as_str()?.to_string() })
                });
                if read.is_none() {
                    eprintln!("GOOGLE_CLIENT_FILE {file}: not a client file from Google's console; no sign-in with Google");
                }
                read
            }
            None => (|| Some(Google { id: var("GOOGLE_CLIENT_ID")?, secret: var("GOOGLE_CLIENT_SECRET")? }))(),
        };
        let apple = (|| {
            let pem = std::fs::read_to_string(var("APPLE_KEY_FILE")?).ok()?;
            Some(Apple { id: var("APPLE_CLIENT_ID")?, team: var("APPLE_TEAM_ID")?, key_id: var("APPLE_KEY_ID")?, key: pem_der(&pem)? })
        })();
        OAuth { google, apple }
    }

    pub fn none() -> OAuth {
        OAuth { google: None, apple: None }
    }

    /// The providers set up here, in the order their buttons stand.
    pub fn providers(&self) -> Vec<&'static str> {
        [("google", self.google.is_some()), ("apple", self.apple.is_some())].into_iter().filter(|(_, on)| *on).map(|(p, _)| p).collect()
    }

    /// Where to send the person, and what the cookie keeps for their return: `provider.state.nonce`.
    pub fn begin(&self, provider: &str, base: &str) -> Option<(String, String)> {
        let (state, nonce) = (crate::crypto::token(), crate::crypto::token());
        let back = format!("{base}/oauth/{provider}/back");
        let q = |pairs: &[(&str, &str)]| form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish();
        let url = match provider {
            "google" => {
                let g = self.google.as_ref()?;
                format!(
                    "https://accounts.google.com/o/oauth2/v2/auth?{}",
                    q(&[("client_id", &g.id), ("redirect_uri", &back), ("response_type", "code"), ("scope", "openid email"), ("state", &state), ("nonce", &nonce), ("prompt", "select_account")])
                )
            }
            "apple" => {
                let a = self.apple.as_ref()?;
                // Asking for the address makes Apple answer by a form posted back, not a redirect.
                format!(
                    "https://appleid.apple.com/auth/authorize?{}",
                    q(&[("client_id", &a.id), ("redirect_uri", &back), ("response_type", "code"), ("scope", "email"), ("response_mode", "form_post"), ("state", &state), ("nonce", &nonce)])
                )
            }
            _ => return None,
        };
        Some((url, format!("{provider}.{state}.{nonce}")))
    }

    /// The code traded for the person: `kept` is the cookie `begin` set, `state` what came back.
    pub fn finish(&self, provider: &str, kept: &str, state: &str, code: &str, base: &str) -> Result<Person, String> {
        let mut parts = kept.splitn(3, '.');
        let (p, s, nonce) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        if p != provider || s.is_empty() || !crate::crypto::same(s, state) {
            return Err("the sign-in came back without the request it answers".into());
        }
        let back = format!("{base}/oauth/{provider}/back");
        let (endpoint, id, secret, issuers): (&str, &str, String, &[&str]) = match provider {
            "google" => {
                let g = self.google.as_ref().ok_or("Google is not set up")?;
                ("https://oauth2.googleapis.com/token", &g.id, g.secret.clone(), &["https://accounts.google.com", "accounts.google.com"])
            }
            "apple" => {
                let a = self.apple.as_ref().ok_or("Apple is not set up")?;
                ("https://appleid.apple.com/auth/token", &a.id, a.client_secret()?, &["https://appleid.apple.com"])
            }
            _ => return Err("no such provider".into()),
        };
        let body = form_urlencoded::Serializer::new(String::new())
            .extend_pairs([("grant_type", "authorization_code"), ("code", code), ("redirect_uri", &back), ("client_id", id), ("client_secret", &secret)])
            .finish();
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(20))).http_status_as_error(false).build().into();
        let mut r = agent.post(endpoint).header("Content-Type", "application/x-www-form-urlencoded").send(body).map_err(|e| format!("{provider}: {e}"))?;
        let answer: J = serde_json::from_str(&r.body_mut().read_to_string().unwrap_or_default()).map_err(|e| format!("{provider}: {e}"))?;
        let token = answer["id_token"].as_str().ok_or_else(|| format!("{provider}: {}", answer["error"].as_str().unwrap_or("no ID token")))?;
        // Received straight from the provider over TLS: its claims are checked, its signature need
        // not be (OpenID Connect Core 3.1.3.7).
        let claims = token.split('.').nth(1).and_then(|p| URL_SAFE_NO_PAD.decode(p.trim_end_matches('=')).ok()).and_then(|b| serde_json::from_slice::<J>(&b).ok()).ok_or("an ID token that does not read")?;
        check(&claims, issuers, id, nonce, crate::store::now()).map(|email| Person { email })
    }
}

/// What an ID token must say: from the issuer, for this app, not expired, with the nonce sent, and
/// an address the provider has verified.
fn check(claims: &J, issuers: &[&str], audience: &str, nonce: &str, now: i64) -> Result<String, String> {
    let aud_ok = match &claims["aud"] {
        J::String(a) => a == audience,
        J::Array(all) => all.iter().any(|a| a == audience),
        _ => false,
    };
    let verified = matches!(&claims["email_verified"], J::Bool(true)) || claims["email_verified"] == "true";
    let email = claims["email"].as_str().unwrap_or("").trim().to_lowercase();
    if !issuers.iter().any(|i| claims["iss"] == *i) {
        return Err("an ID token from someone else".into());
    }
    if !aud_ok {
        return Err("an ID token for another app".into());
    }
    if claims["exp"].as_i64().unwrap_or(0) <= now {
        return Err("an ID token that has expired".into());
    }
    if claims["nonce"].as_str() != Some(nonce) || nonce.is_empty() {
        return Err("an ID token for another sign-in".into());
    }
    if !verified || !crate::mail::plausible(&email) {
        return Err("no verified email address".into());
    }
    Ok(email)
}

impl Apple {
    /// Apple's client secret: a JWT, ES256, signed with the team's key, good for an hour.
    fn client_secret(&self) -> Result<String, String> {
        use ring::signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
        let now = crate::store::now();
        let header = serde_json::json!({ "alg": "ES256", "kid": self.key_id });
        let claims = serde_json::json!({ "iss": self.team, "iat": now, "exp": now + 3600, "aud": "https://appleid.apple.com", "sub": self.id });
        let input = format!("{}.{}", URL_SAFE_NO_PAD.encode(header.to_string()), URL_SAFE_NO_PAD.encode(claims.to_string()));
        let rng = ring::rand::SystemRandom::new();
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &self.key, &rng).map_err(|_| "APPLE_KEY_FILE: not an ES256 key")?;
        let signature = key.sign(&rng, input.as_bytes()).map_err(|_| "Apple's client secret could not be signed")?;
        Ok(format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref())))
    }
}

/// The DER inside a PEM file such as Apple's `.p8`.
fn pem_der(pem: &str) -> Option<Vec<u8>> {
    let body: String = pem.lines().filter(|l| !l.starts_with("-----")).map(str::trim).collect();
    STANDARD.decode(body).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_id_token_is_taken_only_from_its_issuer_for_this_app_fresh_with_its_nonce() {
        let good = json!({ "iss": "https://accounts.google.com", "aud": "app", "exp": 2000, "nonce": "n1", "email": "A@Example.com", "email_verified": true });
        let iss = ["https://accounts.google.com"];
        assert_eq!(check(&good, &iss, "app", "n1", 1000).unwrap(), "a@example.com");
        let with = |k: &str, v: J| {
            let mut c = good.clone();
            c[k] = v;
            c
        };
        assert!(check(&with("iss", json!("https://evil.example")), &iss, "app", "n1", 1000).is_err());
        assert!(check(&with("aud", json!("other")), &iss, "app", "n1", 1000).is_err());
        assert!(check(&good, &iss, "app", "n1", 3000).is_err(), "expired");
        assert!(check(&good, &iss, "app", "n2", 1000).is_err(), "another sign-in's nonce");
        assert!(check(&with("email_verified", json!(false)), &iss, "app", "n1", 1000).is_err());
        assert!(check(&with("email_verified", json!("true")), &iss, "app", "n1", 1000).is_ok(), "Apple says it as a string");
    }

    #[test]
    fn a_sign_in_that_comes_back_must_answer_the_one_that_went_out() {
        let o = OAuth { google: Some(Google { id: "app".into(), secret: "s".into() }), apple: None };
        let (url, kept) = o.begin("google", "https://zetlyn.test").unwrap();
        assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?") && url.contains("redirect_uri=https%3A%2F%2Fzetlyn.test%2Foauth%2Fgoogle%2Fback"));
        let state = kept.split('.').nth(1).unwrap();
        assert!(url.contains(state));
        assert!(o.finish("google", &kept, "forged", "code", "https://zetlyn.test").unwrap_err().contains("without the request"));
        assert!(o.finish("apple", &kept, state, "code", "https://zetlyn.test").is_err(), "another provider's cookie");
        assert!(o.begin("apple", "https://zetlyn.test").is_none(), "Apple is not set up");
    }

    #[test]
    fn apples_client_secret_is_an_es256_jwt() {
        use ring::signature::{EcdsaKeyPair, KeyPair, UnparsedPublicKey, ECDSA_P256_SHA256_FIXED, ECDSA_P256_SHA256_FIXED_SIGNING};
        let rng = ring::rand::SystemRandom::new();
        let der = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let pem = format!("-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n", STANDARD.encode(der.as_ref()));
        let apple = Apple { id: "com.zetlyn.web".into(), team: "TEAM".into(), key_id: "KEY".into(), key: pem_der(&pem).unwrap() };
        let jwt = apple.client_secret().unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        let header: J = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        let claims: J = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!((header["alg"].as_str(), header["kid"].as_str()), (Some("ES256"), Some("KEY")));
        assert_eq!((claims["iss"].as_str(), claims["sub"].as_str(), claims["aud"].as_str()), (Some("TEAM"), Some("com.zetlyn.web"), Some("https://appleid.apple.com")));
        let public = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, der.as_ref(), &rng).unwrap().public_key().as_ref().to_vec();
        let signed = format!("{}.{}", parts[0], parts[1]);
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, public).verify(signed.as_bytes(), &URL_SAFE_NO_PAD.decode(parts[2]).unwrap()).expect("a signature Apple can check");
    }

    /// The real key, where a checkout keeps it (`secrets/`): signs Apple's client secret before it
    /// goes to a server. Without the file there is nothing to check.
    #[test]
    fn the_apple_key_in_secrets_signs_a_client_secret() {
        let Ok(pem) = std::fs::read_to_string("secrets/apple-key.p8") else { return };
        let apple = Apple { id: "com.zetlyn.web".into(), team: "TEAM".into(), key_id: "KEY".into(), key: pem_der(&pem).expect("a PEM file") };
        assert_eq!(apple.client_secret().expect("an ES256 key").split('.').count(), 3);
    }
}
