//! Tokens, their digests, and HMAC signatures. (From the old Zetlyn's `place.rs`.)

use sha2::{Digest, Sha256};

/// 256 bits from the system's own source, as hex: a sign-in link, a session.
pub fn token() -> String {
    use std::io::Read;
    let mut b = [0u8; 32];
    let mut f = std::fs::File::open("/dev/urandom").expect("the system's random source");
    f.read_exact(&mut b).expect("the system's random source");
    hex(&b)
}

/// What is kept of a token: its SHA-256. A token of 256 bits needs no stretching; there is no
/// smaller space to search than the whole of it.
pub fn digest(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.iter().map(|b| b ^ 0x36).collect::<Vec<u8>>());
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(k.iter().map(|b| b ^ 0x5c).collect::<Vec<u8>>());
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// Two strings compared in time that does not depend on where they differ.
pub fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A short name nobody can guess or count on: ten characters of lower-case letters and digits
/// without the ones easily mistaken (Crockford's base 32), 50 bits from the system's own source.
pub fn short_id() -> String {
    const ABC: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";
    token().as_bytes().chunks(2).take(10).map(|h| ABC[(u8::from_str_radix(std::str::from_utf8(h).unwrap_or("0"), 16).unwrap_or(0) % 32) as usize] as char).collect()
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_the_rfc_4231_vector() {
        let mac = hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(hex(&mac), "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
        assert_eq!(token().len(), 64);
        assert_ne!(token(), token());
        assert!(same("abc", "abc") && !same("abc", "abd"));
    }
}
