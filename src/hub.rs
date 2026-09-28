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
    /// The public half of the key that may write under this name. Not a secret, and not a way
    /// in: a copy of this file tells you who publishes here and lets you do nothing.
    ///
    /// This used to be the hash of a bearer token, and it was the same mistake a token always
    /// is. Whoever held it could publish, so it had to live wherever publishing happened, and a
    /// publisher was a different person from the same human operating a deployment. One key,
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
        if !matches!(tree, "datasets" | "scopes") {
            return Err(format!("{tree}: a hub holds datasets and scopes"));
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
// What this hub carries.
//
// A list of what is here, not an index of what is in it. The hub reads no records and answers no
// query: it walks its own directory, reads the tag and the manifest each tag names, and prints
// what those say. A subscriber that wants more fetches the manifest itself.

/// One thing the hub carries, as the front page and `/index.json` say it.
struct Carried {
    tree: &'static str,
    owner: String,
    name: String,
    tag: String,
    version: String,
    title: String,
    about: String,
    built_at: u64,
    records: u64,
    members: usize,
    bytes: u64,
}

impl Carried {
    fn reference(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

fn read_dir_names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with('.') == false)
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    out.sort();
    out
}

/// Everything under `datasets/` and `scopes/`, one row per tag.
fn carried(dir: &Path) -> Vec<Carried> {
    let mut out = Vec::new();
    for tree in ["datasets", "scopes"] {
        let root = dir.join(tree);
        for owner in read_dir_names(&root) {
            for name in read_dir_names(&root.join(&owner)) {
                let tags = root.join(&owner).join(&name).join("tags");
                for tag in read_dir_names(&tags) {
                    let version = match std::fs::read_to_string(tags.join(&tag)) {
                        Ok(v) => v.trim().to_string(),
                        Err(_) => continue,
                    };
                    let versions = root.join(&owner).join(&name).join("versions").join(&version);
                    let manifest: serde_json::Value =
                        match std::fs::read(versions.join("manifest.json"))
                            .ok()
                            .and_then(|b| serde_json::from_slice(&b).ok())
                        {
                            Some(m) => m,
                            None => continue,
                        };
                    let s = |k: &str| manifest[k].as_str().unwrap_or_default().to_string();
                    out.push(Carried {
                        tree,
                        owner: owner.clone(),
                        name: name.clone(),
                        tag: tag.clone(),
                        version,
                        title: if s("title").is_empty() {
                            format!("{owner}/{name}")
                        } else {
                            s("title")
                        },
                        about: s("about"),
                        built_at: manifest["built_at"].as_u64().unwrap_or(0),
                        records: manifest["records"].as_u64().unwrap_or(0),
                        members: manifest["members"].as_array().map(Vec::len).unwrap_or(0),
                        bytes: manifest["payloads"]["records.jsonl"]["bytes"]
                            .as_u64()
                            .unwrap_or(0),
                    });
                }
            }
        }
    }
    out
}

fn index_json(dir: &Path) -> Vec<u8> {
    let rows: Vec<serde_json::Value> = carried(dir)
        .iter()
        .map(|c| {
            serde_json::json!({
                "tree": c.tree, "reference": c.reference(), "tag": c.tag,
                "version": c.version, "title": c.title, "about": c.about,
                "built_at": c.built_at, "records": c.records,
                "members": c.members, "bytes": c.bytes,
            })
        })
        .collect();
    let body = serde_json::json!({ "spec_version": crate::artifact::SPEC_VERSION, "carries": rows });
    serde_json::to_vec_pretty(&body).unwrap_or_default()
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn megabytes(n: u64) -> String {
    if n == 0 {
        return String::new();
    }
    if n < 1024 * 1024 {
        format!("{} kB", n / 1024)
    } else {
        format!("{} MB", n / (1024 * 1024))
    }
}

/// The front page. Served where a person asks for the hub itself.
fn index_page(dir: &Path, serving: &[String]) -> Vec<u8> {
    use maud::html;
    let rows = carried(dir);
    let (scopes, datasets): (Vec<&Carried>, Vec<&Carried>) =
        rows.iter().partition(|c| c.tree == "scopes");
    let page = html! {
        (maud::DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Zetlyn hub" }
                meta name="description" content="Scopes and datasets you can subscribe to, and the ones served here.";
                style { (maud::PreEscaped(crate::serve::STYLE)) }
            }
            body { main {
                h1 { "Zetlyn hub" }
                p.about {
                    "Every scope and dataset here is bytes somebody already built. Subscribing "
                    "fetches those bytes and reads them on your own machine: the hub holds no "
                    "index, answers no query, and never learns what you asked."
                }

                h2 { "Scopes" }
                @if scopes.is_empty() { p.dim { "None yet." } }
                div.grid {
                    @for c in &scopes {
                        div.card {
                            h4 {
                                @let mount = format!("/{}", c.reference());
                                @if serving.iter().any(|s| *s == c.reference()) {
                                    a href=(mount) { (c.title) }
                                } @else { (c.title) }
                                span.cover { (c.reference()) }
                            }
                            @if !c.about.is_empty() { p.dim { (c.about) } }
                            p.dim {
                                (c.members) " members · version " (c.version)
                                @if c.tag != "latest" { " · tag " (c.tag) }
                            }
                            pre { "zetlyn scope subscribe " (c.reference()) }
                        }
                    }
                }

                h2 { "Datasets" }
                @if datasets.is_empty() { p.dim { "None yet." } }
                table {
                    thead { tr { th { "Dataset" } th { "Records" } th { "Bytes" } th { "Version" } } }
                    tbody {
                        @for c in &datasets {
                            tr {
                                td { (c.reference()) @if !c.title.is_empty() {
                                    div.why { (c.title) } } }
                                td { (thousands(c.records)) }
                                td { (megabytes(c.bytes)) }
                                td { (c.version) }
                            }
                        }
                    }
                }
                p.dim { "zetlyn dataset subscribe owner/name" }

                footer {
                    "A hub serves files. "
                    a href="https://zetlyn.com" { "zetlyn.com" }
                    " · "
                    a href="/index.json" { "index.json" }
                }
            } }
        }
    };
    page.into_string().into_bytes()
}

pub fn serve(dir: &Path, addr: &str, serving: &[String]) -> Result<(), String> {
    let server = tiny_http::Server::http(addr).map_err(|e| e.to_string())?;
    println!("a hub at {} on http://{addr}", dir.display());
    for name in serving {
        println!("  a scope surface is mounted on this host at /{name}");
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
            "GET" | "HEAD" if path.is_empty() => (200, index_page(dir, serving)),
            "GET" | "HEAD" if path == "index.json" => (200, index_json(dir)),
            "GET" | "HEAD" => match crate::place::Place::get(&place, &path) {
                Ok(bytes) => (200, bytes),
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
        let kind = if path.is_empty() {
            "text/html; charset=utf-8"
        } else if path.ends_with(".json") {
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
