//! A hub that decides who may write to it.
//!
//! A folder, a mount and somebody's own bucket need none of this: whoever can write there may
//! write there, and there is no name to contend for. This exists for the one case where the
//! namespace is shared, which is a hub several people publish to.
//!
//! An owner name is taken first come, first served, and it is taken for good. What it allows is
//! writing under `sources/{owner}/` and `trackers/{owner}/` and nothing else.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
    "sources",
    "trackers",
    "versions",
    "tags",
    "owners",
    "index",
    // The paths of zetlyn.com itself, where the hub, the website and every hosted organisation share
    // one name: an organisation called one of these would be a page of the site, or the machine.
    "app",
    // The machine as a provider, at /oauth/ since 2026-10-06.
    "oauth",
    // Stripe's webhook for the hosted worlds, since 2026-10-06.
    "billing",
    // Ordering an organisation, and the terms it is ordered on, since 2026-10-07.
    "order",
    "terms",
    "withdrawal",
    "dpa",
    "docs",
    "legal",
    "privacy",
    "directory",
    "packages",
    "examples",
    "signin",
    "signout",
    "account",
    "proposals",
    "propose",
    // Where the worlds a machine hosts are, beneath it.
    "worlds",
    // The site's other pages, and the names it used to have.
    "hosting",
    "hub-about",
    "install",
    "404",
    "scopes",
    "datasets",
    "impressum",
    "datenschutz",
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
    /// The public half of the key that may write under this name. Not a secret, and not a way
    /// in: a copy of this file tells you who publishes here and lets you do nothing.
    ///
    /// This used to be the hash of a bearer token, and it was the same mistake a token always
    /// is. Whoever held it could publish, so it had to live wherever publishing happened, and a
    /// publisher was a different person from the same human operating a workspace. One key,
    /// everywhere somebody acts.
    pub key: String,
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
        crate::yaml::read_or_default(&Self::path(dir))
    }

    fn path(dir: &Path) -> PathBuf {
        dir.join("owners.yaml")
    }

    fn save(&self, dir: &Path) -> Result<(), String> {
        let text = crate::yaml::to_string(self)?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        std::fs::write(Self::path(dir), text).map_err(|e| format!("{}: {e}", dir.display()))
    }

    /// First come, first served, and once taken it stays taken. Nothing secret comes back: the
    /// person registering already holds the key, and the hub is only told which one it is.
    pub fn register(
        &mut self,
        dir: &Path,
        name: &str,
        email: &str,
        key: &str,
    ) -> Result<(), String> {
        if let Some(why) = why_not(name) {
            return Err(format!("{name}: {why}"));
        }
        crate::key::bytes(key).map_err(|e| format!("{key}: {e}"))?;
        if let Some(held) = self.owner.get(name) {
            return Err(format!(
                "{name} was taken on {} and does not come free",
                held.registered
            ));
        }
        if let Some((held, _)) = self.owner.iter().find(|(_, o)| o.key == key.trim()) {
            return Err(format!("that key already publishes as {held}"));
        }
        self.owner.insert(
            name.to_string(),
            Owner {
                email: email.to_string(),
                registered: crate::iso_date(crate::now()),
                key: key.trim().to_string(),
            },
        );
        self.save(dir)
    }

    /// Which owner a key speaks for, if any.
    pub fn speaks_for(&self, key: &str) -> Option<&str> {
        self.owner
            .iter()
            .find(|(_, o)| o.key == key.trim())
            .map(|(name, _)| name.as_str())
    }

    /// Whether whoever signed this call may write here. The path's second segment is the owner,
    /// and that is the whole of the rule.
    ///
    /// The call is signed the same way a console call is: over the method, the path, the body and
    /// the time. A signature cannot be lifted into a different request, and a hub that is read by
    /// everybody therefore hands out nothing by being read.
    pub fn may_write(
        &self,
        key: &str,
        path: &str,
        target: &str,
        body: &[u8],
        at: &str,
        signature: &str,
    ) -> Result<(), String> {
        let Some(owner) = self.speaks_for(key) else {
            return Err("that key publishes as nobody here".into());
        };
        let mut parts = path.split('/');
        let tree = parts.next().unwrap_or_default();
        let named = parts.next().unwrap_or_default();
        if !matches!(tree, "sources" | "trackers" | "packages") {
            return Err(format!("{tree}: a hub holds sources, trackers and packages"));
        }
        if named != owner {
            return Err(format!("{owner} may not write under {named}"));
        }
        let now = crate::now();
        let then = crate::fetch::seconds_of(at);
        if then == 0 || (now - then).abs() > crate::grant::WINDOW_SECONDS {
            return Err(format!(
                "that call is stamped {at}, which is not within {} seconds of now",
                crate::grant::WINDOW_SECONDS
            ));
        }
        crate::key::verify(
            key,
            crate::grant::request_statement("PUT", target, body, at).as_bytes(),
            signature,
        )
        .map_err(|e| format!("the call is not signed by that key: {e}"))
    }
}

// ---------------------------------------------------------------------------------------------
// Serving one.

/// A hub over HTTP: `GET` for anybody, `PUT` for a token that speaks for the owner in the path.
///
/// This is the only piece a folder, a mount or a private bucket does not need. It exists because
/// a hub several people publish to has one namespace, and a namespace needs somebody to say who
/// holds what.

// ---------------------------------------------------------------------------------------------
// What this hub carries, and its pages: in `hubpages`, so a hub that is only storage has the same.


pub fn serve(dir: &Path, addr: &str, serving: &[String]) -> Result<(), String> {
    let server = tiny_http::Server::http(addr).map_err(|e| e.to_string())?;
    println!("a hub at {} on http://{addr}", dir.display());
    for name in serving {
        println!("  a tracker surface is mounted on this host at /{name}");
    }
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
        // Its pages link under /hub/, where a hub is on a name it shares with a site or a world;
        // read here, that is the same hub. What subscribers ask for is at its root, as before.
        let path = if matches!(method.as_str(), "GET" | "HEAD") {
            match path.strip_prefix("hub") {
                Some(rest) if rest.is_empty() || rest.starts_with('/') => rest.trim_start_matches('/').to_string(),
                _ => path,
            }
        } else {
            path
        };
        let header = |name: &'static str| -> String {
            request
                .headers()
                .iter()
                .find(|h| h.field.equiv(name))
                .map(|h| h.value.as_str().to_string())
                .unwrap_or_default()
        };
        let (who, at, signature) = (
            header("Zetlyn-Key"),
            header("Zetlyn-Date"),
            header("Zetlyn-Signature"),
        );

        let (status, body) = match method.as_str() {
            // The front page and a page per thing, rendered as they are asked for, from the same
            // manifests a rendered hub writes them from.
            "GET" | "HEAD" if path.is_empty() || is_page(&path) => {
                let opens = |r: &crate::hubpages::Row| serving.iter().any(|s| *s == r.reference()).then(|| format!("/{}", r.reference()));
                match crate::hubpages::page_at(&place, &path, &opens) {
                    Some(p) => (200, p.into_bytes()),
                    None => (404, b"nothing at that address\n".to_vec()),
                }
            }
            "GET" | "HEAD" if path == "index.json" => {
                (200, crate::hubpages::index_json(&crate::hubpages::shelf(&place).unwrap_or_default()))
            }
            "GET" | "HEAD" if crate::examples::file(&path).is_some() => (200, crate::examples::file(&path).unwrap_or_default().into_bytes()),
            "GET" | "HEAD" => match crate::place::Place::get(&place, &path) {
                Ok(bytes) => (200, bytes),
                // A hub with no stylesheet of its own still has to be readable, so that one
                // address falls back to the product's own sheet. Nothing else does.
                Err(_) if path == "style.css" || path == "zetlyn.css" => (200, crate::serve::STYLE.as_bytes().to_vec()),
                Err(_) => (404, b"nothing at that address\n".to_vec()),
            },
            "PUT" => {
                let mut bytes = Vec::new();
                let read = std::io::Read::read_to_end(request.as_reader(), &mut bytes);
                // Loaded fresh each time: an owner registered a minute ago may publish now.
                let owners = Owners::load(dir);
                // Signed over the address as it was asked for, which is the path with its slash.
                let allowed =
                    owners.may_write(&who, &path, &format!("/{path}"), &bytes, &at, &signature);
                match (read, allowed) {
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
        // A tag and a payload are the two things a program fetches, and they are the two the
        // layout names. The rest is what a person's browser asked for on the way to reading this.
        let kind = if path.is_empty() || is_page(&path) || path.ends_with(".html")
            || body.get(..14).is_some_and(|b| b.eq_ignore_ascii_case(b"<!doctype html"))
        {
            "text/html; charset=utf-8"
        } else if path.ends_with(".csv") {
            "text/csv; charset=utf-8"
        } else if path.ends_with(".json") {
            "application/json"
        } else if path.ends_with(".jsonl") {
            "application/x-ndjson"
        } else if path.ends_with(".css") {
            "text/css; charset=utf-8"
        } else if path.ends_with(".png") {
            "image/png"
        } else if path.ends_with(".svg") {
            "image/svg+xml"
        } else if path.ends_with(".js") {
            "text/javascript; charset=utf-8"
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

/// `sources/owner/name`, `trackers/owner/name/`, `packages/owner/name`: the page about one thing.
/// A page rather than a file: a publisher (`<owner>`), or a thing by the address a person reads
/// (`<owner>/<tree>/<name>`) or the one its files are kept at (`<tree>/<owner>/<name>`).
fn is_page(path: &str) -> bool {
    let parts: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    let tree = |p: &str| matches!(p, "sources" | "trackers" | "packages");
    if parts.iter().any(|p| p.is_empty()) {
        return false;
    }
    match parts.as_slice() {
        [owner] => !owner.contains('.') && !tree(owner),
        [a, b, _] => tree(a) || tree(b),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::why_not;

    #[test]
    fn a_path_of_zetlyn_com_is_nobodys_name() {
        // The site, the hub's roots and the machine share one name with every organisation.
        for taken in ["app", "hub", "docs", "api", "trackers", "sources", "packages", "examples", "legal", "privacy", "directory", "signin", "signout", "account"] {
            assert_eq!(why_not(taken).as_deref(), Some("reserved"), "{taken}");
        }
        assert!(why_not("zetlyn").is_some() && why_not("zetlyn-labs").is_some());
        for free in ["acme", "car-prices", "app-store", "docs2"] {
            assert_eq!(why_not(free), None, "{free}");
        }
    }
}
