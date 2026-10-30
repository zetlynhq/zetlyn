//! ALTCHA (altcha.org): a little work the browser does before a sign-in link is mailed, so a
//! program cannot have Zetlyn mail whatever addresses it is given without doing it too.
//!
//! The widget's protocol v3 with SHA-256, counter mode: the challenge is a nonce, a salt and a
//! counter picked here; its key is SHA-256(salt ‖ nonce ‖ counter as 4 bytes, big-endian), and the
//! challenge carries the key's first half, so the browser counts up from 0 until it finds the
//! counter again. The parameters are signed (HMAC-SHA-256 over their JSON with the keys sorted),
//! so nothing is kept between the page and the answer; `fresh` keeps each answer to one use.
//!
//! The widget (`assets/altcha.min.js`, MIT, from the npm package `altcha`) is served as
//! `/altcha.js`, from here, like everything else.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// The widget, as published (`altcha` 3.5.0, `dist/main/altcha.min.js`).
pub const WIDGET: &str = include_str!("../assets/altcha.min.js");

/// How long a challenge may be answered, in seconds.
pub const LIFE: i64 = 3600;

/// The counters the browser has to reach: a second or two of work in a browser, at most.
const COUNTERS: std::ops::Range<u32> = 20_000..60_000;

/// A challenge for the widget's `challenge` attribute, expiring `LIFE` seconds after `now`.
pub fn challenge(secret: &str, now: i64) -> String {
    let random = crate::crypto::token();
    let (nonce, salt) = (&random[..32], &random[32..]);
    let counter = COUNTERS.start + u32::from_str_radix(&crate::crypto::token()[..8], 16).unwrap_or(0) % (COUNTERS.end - COUNTERS.start);
    let key = derive(salt, nonce, counter).unwrap_or_default();
    let parameters = json!({
        "algorithm": "SHA-256",
        "cost": 1,
        "expiresAt": now + LIFE,
        "keyLength": 32,
        "keyPrefix": crate::crypto::hex(&key[..16]),
        "nonce": nonce,
        "salt": salt,
    });
    json!({ "parameters": parameters, "signature": sign(secret, &parameters) }).to_string()
}

/// Whether `payload` (the widget's field, base64 of JSON) answers a challenge made here with
/// `secret` that has not expired; the nonce, to keep it to one use. Nothing about why not: the
/// page says the same either way.
pub fn verify(secret: &str, payload: &str, now: i64) -> Option<String> {
    let v: Value = serde_json::from_slice(&STANDARD.decode(payload.trim()).ok()?).ok()?;
    let parameters = v.pointer("/challenge/parameters")?;
    let signature = v.pointer("/challenge/signature")?.as_str()?;
    if !crate::crypto::same(signature, &sign(secret, parameters)) {
        return None;
    }
    let p = |k: &str| parameters.get(k);
    if p("algorithm")?.as_str()? != "SHA-256" || p("cost")?.as_i64()? != 1 || p("keyLength")?.as_i64()? != 32 || p("expiresAt")?.as_i64()? < now {
        return None;
    }
    let (nonce, salt, prefix) = (p("nonce")?.as_str()?, p("salt")?.as_str()?, p("keyPrefix")?.as_str()?);
    let counter = u32::try_from(v.pointer("/solution/counter")?.as_u64()?).ok()?;
    let key = crate::crypto::hex(&derive(salt, nonce, counter)?);
    let said = v.pointer("/solution/derivedKey")?.as_str()?;
    (crate::crypto::same(&key, said) && key.starts_with(prefix)).then(|| nonce.to_string())
}

/// SHA-256 of the salt, the nonce and the counter (4 bytes, big-endian), the salt and nonce as
/// the bytes their hex stands for.
fn derive(salt: &str, nonce: &str, counter: u32) -> Option<Vec<u8>> {
    let mut h = Sha256::new();
    h.update(unhex(salt)?);
    h.update(unhex(nonce)?);
    h.update(counter.to_be_bytes());
    Some(h.finalize().to_vec())
}

/// The signature: HMAC-SHA-256, in hex, of the parameters as JSON with their keys sorted, as the
/// widget's own library writes it.
fn sign(secret: &str, parameters: &Value) -> String {
    crate::crypto::hex(&crate::crypto::hmac_sha256(secret.as_bytes(), sorted(parameters).to_string().as_bytes()))
}

fn sorted(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<_> = m.keys().collect();
            keys.sort();
            Value::Object(keys.into_iter().map(|k| (k.clone(), sorted(&m[k]))).collect::<Map<_, _>>())
        }
        other => other.clone(),
    }
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    s.len().is_multiple_of(2).then_some(())?;
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the widget does: count from 0 until the key starts with the prefix.
    fn solve(challenge: &str) -> String {
        let c: Value = serde_json::from_str(challenge).unwrap();
        let p = &c["parameters"];
        let (nonce, salt, prefix) = (p["nonce"].as_str().unwrap(), p["salt"].as_str().unwrap(), p["keyPrefix"].as_str().unwrap());
        let counter = (0..u32::MAX).find(|n| crate::crypto::hex(&derive(salt, nonce, *n).unwrap()).starts_with(prefix)).unwrap();
        let key = crate::crypto::hex(&derive(salt, nonce, counter).unwrap());
        STANDARD.encode(json!({ "challenge": c, "solution": { "counter": counter, "derivedKey": key, "time": 812.4 } }).to_string())
    }

    #[test]
    fn an_answer_is_taken_once_found_and_only_while_fresh() {
        let c = challenge("s3cret", 1000);
        let answer = solve(&c);
        let nonce = verify("s3cret", &answer, 1000).expect("the answer");
        assert_eq!(nonce.len(), 32);
        assert_eq!(verify("s3cret", &answer, 1000 + LIFE), Some(nonce), "still in time");
        assert_eq!(verify("s3cret", &answer, 1001 + LIFE), None, "expired");
        assert_eq!(verify("another", &answer, 1000), None, "made with another secret");
    }

    #[test]
    fn nothing_made_up_or_changed_passes() {
        let c = challenge("s3cret", 1000);
        let answer: Value = serde_json::from_slice(&STANDARD.decode(solve(&c)).unwrap()).unwrap();
        let again = |v: &Value| STANDARD.encode(v.to_string());
        let mut later = answer.clone();
        later["challenge"]["parameters"]["expiresAt"] = json!(9_999_999_999i64);
        assert_eq!(verify("s3cret", &again(&later), 1000), None, "a changed parameter breaks the signature");
        let mut wrong = answer.clone();
        wrong["solution"]["counter"] = json!(answer["solution"]["counter"].as_u64().unwrap() + 1);
        assert_eq!(verify("s3cret", &again(&wrong), 1000), None, "another counter");
        assert_eq!(verify("s3cret", "", 1000), None);
        assert_eq!(verify("s3cret", "not base64!", 1000), None);
        assert_eq!(verify("s3cret", &STANDARD.encode(r#"{"challenge":null,"solution":null,"test":true}"#), 1000), None, "the widget's test mode");
    }

    #[test]
    fn the_signature_is_over_the_keys_sorted() {
        let a = json!({ "salt": "00", "algorithm": "SHA-256", "cost": 1 });
        assert_eq!(sorted(&a).to_string(), r#"{"algorithm":"SHA-256","cost":1,"salt":"00"}"#);
    }
}
