//! Who you are, in the places where that has to be the same person twice.
//!
//! There are two populations here and only one of them needs this.
//!
//! A **reader** browses a tracker, subscribes to it and gets a feed. They are an email address in
//! one workspace's `accounts.db`, and that is right: a reader of one operator's tracker has no
//! business being the same account as a reader of somebody else's. Nothing below touches them.
//!
//! An **actor** publishes a source, operates a workspace, or drives a console. Every one of
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
const DESCRIPTION: &str = "identity.yaml";

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
    crate::yaml::read_or_default(&home().join(DESCRIPTION))
}

/// The public half, where there is one.
pub fn key() -> Option<String> {
    crate::key::public(&home(), KEY_FILE)
}

/// Where a world keeps its own key: `.zetlyn` beside the workspace that `dir` is in, where there is
/// a key there; whoever runs the program otherwise. On a machine of many worlds each signs as itself,
/// and its key goes with it when it leaves, so what subscribers pinned still holds.
pub fn home_of(dir: &std::path::Path) -> PathBuf {
    dir.ancestors()
        .find(|p| p.join(crate::account::WORKSPACE).exists())
        .map(|w| w.join(".zetlyn"))
        .filter(|z| z.join(KEY_FILE).exists())
        .unwrap_or_else(home)
}

/// The key things in `dir` are published with.
pub fn key_at(dir: &std::path::Path) -> Option<String> {
    crate::key::public(&home_of(dir), KEY_FILE)
}

/// Signed with the key things in `dir` are published with.
pub fn sign_at(dir: &std::path::Path, message: &[u8]) -> Result<Option<String>, String> {
    crate::key::sign(&home_of(dir), KEY_FILE, message)
}

/// Make one. The name and the contact are what a hub operator reads when deciding whether the
/// person asking for a namespace is somebody; neither is checked by anything.
pub fn new(name: &str, contact: &str) -> Result<String, String> {
    new_in(&home(), name, contact)
}

/// The same, in a directory named rather than found: a world's own, made for it by `world up`.
pub fn new_in(dir: &std::path::Path, name: &str, contact: &str) -> Result<String, String> {
    let dir = dir.to_path_buf();
    let public = crate::key::new(&dir, KEY_FILE)?;
    let who = Who {
        name: name.to_string(),
        contact: contact.to_string(),
    };
    let text = crate::yaml::to_string(&who)?;
    std::fs::write(dir.join(DESCRIPTION), text).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(public)
}

/// Sign as yourself. `Ok(None)` where there is no identity yet, which every caller treats as
/// "then do the thing that needs no signature".
pub fn sign(message: &[u8]) -> Result<Option<String>, String> {
    crate::key::sign(&home(), KEY_FILE, message)
}

/// The identity, or a key kept somewhere else because it belongs to a thing rather than a
/// person. A source that carries its own `publishing.key` keeps signing with it: subscribers
/// pinned that one, and a key that changes under them is a publisher they stop trusting.
pub fn or_local(dir: &std::path::Path, file: &str) -> Option<String> {
    crate::key::public(dir, file).or_else(|| key_at(dir))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_world_with_a_key_of_its_own_publishes_with_it() {
        let w = std::env::temp_dir().join(format!("zetlyn-world-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&w);
        std::fs::create_dir_all(w.join("sources/prices")).unwrap();
        std::fs::write(w.join(crate::account::WORKSPACE), "title: t\n").unwrap();
        assert_eq!(super::home_of(&w.join("sources/prices")), super::home(), "none of its own: whoever runs it");
        let mine = super::new_in(&w.join(".zetlyn"), "t", "").unwrap();
        assert_eq!(super::key_at(&w.join("sources/prices")).as_deref(), Some(mine.as_str()));
        let sig = super::sign_at(&w.join("sources/prices"), b"m").unwrap().unwrap();
        assert!(crate::key::verify(&mine, b"m", &sig).is_ok());
        let _ = std::fs::remove_dir_all(&w);
    }
}
