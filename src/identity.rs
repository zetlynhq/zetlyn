//! Who you are, in the places where that has to be the same person twice.
//!
//! There are two populations here and only one of them needs this.
//!
//! A **reader** browses a scope, subscribes to it and gets a feed. They are an email address in
//! one deployment's `accounts.db`, and that is right: a reader of one operator's scope has no
//! business being the same account as a reader of somebody else's. Nothing below touches them.
//!
//! An **actor** publishes a dataset, operates a deployment, or drives a console. Every one of
//! those is already a signature, and a person doing all three from three directories with three
//! keys is three people for no reason. This is the one key they use everywhere, kept where it
//! belongs to them rather than to a project: `~/.zetlyn`, or `$ZETLYN_HOME`.
//!
//! It is not an account and there is no service behind it. A key says who signed something. Who
//! that is allowed to be is a hub's owners file or an operator's grant, and both of those are
//! somebody's decision rather than an authority's.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const KEY_FILE: &str = "identity.key";
const DESCRIPTION: &str = "identity.toml";

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Who {
    /// What to call you, where a name is shown beside a key.
    #[serde(default)]
    pub name: String,
    /// How to reach you, for the operator of a hub deciding whether to give you a namespace.
    #[serde(default)]
    pub contact: String,
}

/// `$ZETLYN_HOME`, or `~/.zetlyn`. A directory rather than a file, because a key and what it
/// says about itself are two things.
pub fn home() -> PathBuf {
    if let Ok(named) = std::env::var("ZETLYN_HOME") {
        if !named.trim().is_empty() {
            return PathBuf::from(named);
        }
    }
    let base = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join(".zetlyn")
}

pub fn read() -> Who {
    std::fs::read_to_string(home().join(DESCRIPTION))
        .ok()
        .and_then(|raw| toml::from_str(&raw).ok())
        .unwrap_or_default()
}

/// The public half, where there is one.
pub fn key() -> Option<String> {
    crate::key::public(&home(), KEY_FILE)
}

/// Make one. The name and the contact are what a hub operator reads when deciding whether the
/// person asking for a namespace is somebody; neither is checked by anything.
pub fn new(name: &str, contact: &str) -> Result<String, String> {
    let dir = home();
    let public = crate::key::new(&dir, KEY_FILE)?;
    let who = Who {
        name: name.to_string(),
        contact: contact.to_string(),
    };
    let text = toml::to_string_pretty(&who).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(DESCRIPTION), text).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(public)
}

/// Sign as yourself. `Ok(None)` where there is no identity yet, which every caller treats as
/// "then do the thing that needs no signature".
pub fn sign(message: &[u8]) -> Result<Option<String>, String> {
    crate::key::sign(&home(), KEY_FILE, message)
}

/// The identity, or a key kept somewhere else because it belongs to a thing rather than a
/// person. A dataset that carries its own `publishing.key` keeps signing with it: subscribers
/// pinned that one, and a key that changes under them is a publisher they stop trusting.
pub fn or_local(dir: &std::path::Path, file: &str) -> Option<String> {
    crate::key::public(dir, file).or_else(key)
}
