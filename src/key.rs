//! An ed25519 key in a directory, and the two things done with one.
//!
//! Two places use this and they are the same shape. A publisher signs a manifest so a subscriber
//! can tell who wrote it. An operator signs a grant so a console can tell who is allowed to drive
//! it. Both are a signature over a statement, held against a key the other side already knows.
//!
//! A private half is a file with mode 600 and nothing else. There is no keyring, no passphrase
//! and no agent: a key somebody has to type is a key that ends up in a script.

use std::path::Path;

pub fn bytes(raw: &str) -> Result<[u8; 32], String> {
    let hex = raw.trim().trim_start_matches("ed25519:");
    if hex.len() != 64 {
        return Err("a key is 32 bytes, written as 64 hex characters".into());
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| "a key is hexadecimal".to_string())?;
    }
    Ok(out)
}

pub fn hex(raw: &[u8]) -> String {
    raw.iter().map(|b| format!("{b:02x}")).collect()
}

/// A new pair. The private half is written where it was asked for and nowhere else; the public
/// half is what the other side is told, and it is not a secret.
pub fn new(dir: &Path, file: &str) -> Result<String, String> {
    let path = dir.join(file);
    if path.exists() {
        return Err(format!(
            "{} exists. A second key makes everybody who trusted the first stop",
            path.display()
        ));
    }
    let seed = bytes(&crate::account::token())?;
    let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::write(&path, format!("ed25519:{}\n", hex(&seed)))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(format!(
        "ed25519:{}",
        hex(signing.verifying_key().as_bytes())
    ))
}

/// The public half of a key in a directory, which is what its holder hands out.
pub fn public(dir: &Path, file: &str) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join(file)).ok()?;
    let signing = ed25519_dalek::SigningKey::from_bytes(&bytes(&raw).ok()?);
    Some(format!(
        "ed25519:{}",
        hex(signing.verifying_key().as_bytes())
    ))
}

/// `Ok(None)` where there is no key, which is not a failure: a publisher who signs nothing
/// publishes, and a subscriber who pinned nothing takes it.
pub fn sign(dir: &Path, file: &str, message: &[u8]) -> Result<Option<String>, String> {
    let Ok(raw) = std::fs::read_to_string(dir.join(file)) else {
        return Ok(None);
    };
    use ed25519_dalek::Signer;
    let signing = ed25519_dalek::SigningKey::from_bytes(&bytes(&raw)?);
    Ok(Some(format!(
        "ed25519:{}",
        hex(&signing.sign(message).to_bytes())
    )))
}

/// Held against the key the other side already knows, over the bytes as they were served.
pub fn verify(pinned: &str, message: &[u8], signature: &str) -> Result<(), String> {
    use ed25519_dalek::Verifier;
    let key = ed25519_dalek::VerifyingKey::from_bytes(&bytes(pinned)?)
        .map_err(|e| format!("that key is not one: {e}"))?;
    let raw = signature.trim().trim_start_matches("ed25519:");
    if raw.len() != 128 {
        return Err("a signature is 64 bytes, written as 128 hex characters".into());
    }
    let mut out = [0u8; 64];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&raw[i * 2..i * 2 + 2], 16)
            .map_err(|_| "a signature is hexadecimal".to_string())?;
    }
    key.verify(message, &ed25519_dalek::Signature::from_bytes(&out))
        .map_err(|_| "the signature is not that key's, over these bytes".to_string())
}
