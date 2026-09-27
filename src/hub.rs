//! A hub that decides who may write to it.
//!
//! A folder, a mount and somebody's own bucket need none of this: whoever can write there may
//! write there, and there is no name to contend for. This exists for the one case where the
//! namespace is shared, which is a hub several people publish to.
//!
//! An owner name is taken first come, first served, and it is taken for good. What it allows is
//! writing under `datasets/{owner}/` and `scopes/{owner}/` and nothing else.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::account::{digest, token};

/// Names nobody gets, whoever asks first.
///
/// Three reasons, and each of them is somebody being deceived rather than inconvenienced: a name
/// that reads as this project speaking, a name that reads as a place on the hub rather than a
/// publisher on it, and a name a person would mistake for the thing it describes.
const REFUSED: &[&str] = &[
    // This project, and anything that would be read as it speaking.
    "zetlyn",
    "zetlynhq",
    "zetlyn-official",
    "official",
    "admin",
    "administrator",
    "root",
    "support",
    "security",
    "abuse",
    "help",
    "staff",
    "team",
    "system",
    "hub",
    "www",
    "api",
    "cdn",
    "static",
    "assets",
    "docs",
    "status",
    "mail",
    "smtp",
    "ftp",
    "ns",
    "ns1",
    "ns2",
    // Reserved by the layout itself: a reference could not tell these from a tree.
    "datasets",
    "scopes",
    "versions",
    "tags",
    "owners",
    "index",
    // A publisher named after a body it is not.
    "cve",
    "nvd",
    "cisa",
    "mitre",
    "nist",
    "cert",
    "redhat",
    "ubuntu",
    "debian",
    "microsoft",
    "google",
    "apple",
    "github",
    "gitlab",
    "eu",
    "europa",
    "iso",
    "ietf",
    "w3c",
];

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Owners {
    #[serde(default)]
    pub owner: BTreeMap<String, Owner>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Owner {
    pub email: String,
    pub registered: String,
    /// SHA-256 of the token. A copy of this file is not a licence to publish.
    pub token: String,
}

/// What a name has to be before anybody may have it: lower case, a digit or a letter at each end,
/// and hyphens inside. The same shape a host label has, because an owner sits where one does in a
/// reference and a reader should not have to know which they are looking at.
pub fn why_not(name: &str) -> Option<String> {
    if name.len() < 2 || name.len() > 39 {
        return Some("between 2 and 39 characters".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Some("lower-case letters, digits and hyphens".into());
    }
    if name.starts_with('-') || name.ends_with('-') || name.contains("--") {
        return Some("no hyphen at either end, and not two in a row".into());
    }
    if name.contains('.') {
        return Some("no dots: the first segment of a reference is a host when it has one".into());
    }
    if REFUSED.contains(&name) {
        return Some("reserved".into());
    }
    if name.starts_with("zetlyn") {
        return Some("reserved: it would read as this project speaking".into());
    }
    None
}

impl Owners {
    pub fn load(dir: &Path) -> Owners {
        std::fs::read_to_string(Self::path(dir))
            .ok()
            .and_then(|raw| toml::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn path(dir: &Path) -> PathBuf {
        dir.join("owners.toml")
    }

    fn save(&self, dir: &Path) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        std::fs::write(Self::path(dir), text).map_err(|e| format!("{}: {e}", dir.display()))
    }

    /// First come, first served, and once taken it stays taken. Returns the token, which is shown
    /// once: only its hash is kept.
    pub fn register(&mut self, dir: &Path, name: &str, email: &str) -> Result<String, String> {
        if let Some(why) = why_not(name) {
            return Err(format!("{name}: {why}"));
        }
        if let Some(held) = self.owner.get(name) {
            return Err(format!(
                "{name} was taken on {} and does not come free",
                held.registered
            ));
        }
        let secret = token();
        self.owner.insert(
            name.to_string(),
            Owner {
                email: email.to_string(),
                registered: crate::iso_date(crate::now()),
                token: digest(&secret),
            },
        );
        self.save(dir)?;
        Ok(secret)
    }

    /// Which owner a token speaks for, if any.
    pub fn speaks_for(&self, secret: &str) -> Option<&str> {
        let hash = digest(secret);
        self.owner
            .iter()
            .find(|(_, o)| o.token == hash)
            .map(|(name, _)| name.as_str())
    }

    /// Whether this token may write to this path. The path's second segment is the owner, and
    /// that is the whole of the rule.
    pub fn may_write(&self, secret: &str, path: &str) -> Result<(), String> {
        let Some(owner) = self.speaks_for(secret) else {
            return Err("that token belongs to nobody here".into());
        };
        let mut parts = path.split('/');
        let tree = parts.next().unwrap_or_default();
        let named = parts.next().unwrap_or_default();
        if !matches!(tree, "datasets" | "scopes") {
            return Err(format!("{tree}: a hub holds datasets and scopes"));
        }
        if named != owner {
            return Err(format!("{owner} may not write under {named}"));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Serving one.

/// A hub over HTTP: `GET` for anybody, `PUT` for a token that speaks for the owner in the path.
///
/// This is the only piece a folder, a mount or a private bucket does not need. It exists because
/// a hub several people publish to has one namespace, and a namespace needs somebody to say who
/// holds what.
pub fn serve(dir: &Path, addr: &str) -> Result<(), String> {
    let server = tiny_http::Server::http(addr).map_err(|e| e.to_string())?;
    println!("a hub at {} on http://{addr}", dir.display());
    let place = crate::place::Folder {
        root: dir.to_path_buf(),
    };
    for mut request in server.incoming_requests() {
        let path = request
            .url()
            .trim_start_matches('/')
            .split('?')
            .next()
            .unwrap_or("")
            .to_string();
        let method = request.method().as_str().to_string();
        let bearer = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Authorization"))
            .map(|h| h.value.as_str().trim_start_matches("Bearer ").to_string())
            .unwrap_or_default();

        let (status, body) = match method.as_str() {
            "GET" | "HEAD" => match crate::place::Place::get(&place, &path) {
                Ok(bytes) => (200, bytes),
                Err(_) => (404, b"nothing at that address\n".to_vec()),
            },
            "PUT" => {
                let mut bytes = Vec::new();
                let read = std::io::Read::read_to_end(request.as_reader(), &mut bytes);
                // Loaded fresh each time: an owner registered a minute ago may publish now.
                let owners = Owners::load(dir);
                match (read, owners.may_write(&bearer, &path)) {
                    (Err(e), _) => (400, format!("{e}\n").into_bytes()),
                    (_, Err(why)) => (403, format!("{why}\n").into_bytes()),
                    (Ok(_), Ok(())) => match crate::place::Place::put(&place, &path, &bytes) {
                        Ok(()) => (201, b"written\n".to_vec()),
                        Err(e) => (500, format!("{e}\n").into_bytes()),
                    },
                }
            }
            _ => (405, b"a hub answers GET and PUT\n".to_vec()),
        };
        let kind = if path.ends_with(".json") {
            "application/json"
        } else if path.ends_with(".jsonl") {
            "application/x-ndjson"
        } else {
            "text/plain; charset=utf-8"
        };
        let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], kind.as_bytes())
            .map_err(|_| "bad header".to_string())?;
        let response = tiny_http::Response::from_data(body)
            .with_status_code(status)
            .with_header(header);
        let _ = request.respond(response);
    }
    Ok(())
}
