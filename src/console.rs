//! A deployment answering for itself.
//!
//! This is what a platform drives. It is not a second way of doing what the runtime does: every
//! call here is the runtime answering the same question a person asks it at a terminal, so a
//! console feature that is not one of those would be a fork of the program rather than a view of
//! it.
//!
//! It holds no secret. What it holds is the public half of the operator's own key, and it takes
//! nothing that does not trace back to that: a grant the operator signed, and a request signed by
//! the key the grant names.

use std::path::{Path, PathBuf};

use serde_json::{json, Value as J};

use crate::dataset::Dataset;
use crate::grant::{self, Signed};
use crate::scope::{self, Scope};

pub fn serve(root: &Path, addr: &str) -> Result<(), String> {
    let operator = crate::key::public(root, grant::OPERATOR_KEY).ok_or_else(|| {
        format!(
            "{} has no {}. `zetlyn console grant` makes one, and until there is one there is \
             nobody to check a grant against",
            root.display(),
            grant::OPERATOR_KEY
        )
    })?;
    let name = root
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "deployment".into());

    let server = tiny_http::Server::http(addr).map_err(|e| e.to_string())?;
    println!("the console for {name} on http://{addr}");
    println!("it answers to {operator}");

    for mut request in server.incoming_requests() {
        let method = request.method().as_str().to_string();
        let path = request
            .url()
            .split('?')
            .next()
            .unwrap_or("/")
            .trim_end_matches('/')
            .to_string();
        let path = if path.is_empty() {
            "/".to_string()
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
        let (carried, at, signature) = (
            header("Zetlyn-Grant"),
            header("Zetlyn-Date"),
            header("Zetlyn-Signature"),
        );
        let mut body = Vec::new();
        let _ = std::io::Read::read_to_end(request.as_reader(), &mut body);

        let (status, answer) = match allowed(
            &operator, &name, &carried, &method, &path, &body, &at, &signature,
        ) {
            Err(why) => (403, json!({ "refused": why })),
            Ok(()) => answer(root, &name, &method, &path, &body),
        };
        let kind = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
            .map_err(|_| "bad header".to_string())?;
        let response = tiny_http::Response::from_string(answer.to_string())
            .with_status_code(status)
            .with_header(kind);
        let _ = request.respond(response);
    }
    Ok(())
}

/// Three questions in order, and each of them is about somebody else's signature. Which act this
/// is comes first, because a grant that may only read should be refused at the door rather than
/// after the work.
#[allow(clippy::too_many_arguments)]
fn allowed(
    operator: &str,
    name: &str,
    carried: &str,
    method: &str,
    path: &str,
    body: &[u8],
    at: &str,
    signature: &str,
) -> Result<(), String> {
    let what = match (method, path) {
        ("GET", _) => "read",
        ("POST", p) if p.ends_with("/run") => "run",
        ("PUT", p) if p.ends_with("/declaration") => "apply",
        _ => {
            return Err(format!(
                "{method} {path} is not a call this console answers"
            ))
        }
    };
    if carried.trim().is_empty() {
        return Err("no grant. A console takes nothing without one".into());
    }
    let raw = decode(carried)?;
    let signed: Signed =
        toml::from_str(&raw).map_err(|e| format!("that grant does not read: {e}"))?;
    signed.by(operator)?;
    signed
        .grant
        .holds(name, what, &crate::iso_date(crate::now()))?;
    grant::check_request(&signed.grant, method, path, body, at, signature)
}

/// A grant travels in a header, so it travels base64. Written out because it is forty lines and
/// the alternative is a dependency for forty lines.
fn decode(raw: &str) -> Result<String, String> {
    const SET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits = 0u32;
    let mut have = 0u32;
    let mut out = Vec::new();
    for byte in raw.trim().bytes() {
        if byte == b'=' || byte.is_ascii_whitespace() {
            continue;
        }
        let Some(value) = SET.iter().position(|c| *c == byte) else {
            return Err("that grant is not base64".into());
        };
        bits = (bits << 6) | value as u32;
        have += 6;
        if have >= 8 {
            have -= 8;
            out.push((bits >> have) as u8);
        }
    }
    String::from_utf8(out).map_err(|_| "that grant is not text".into())
}

pub fn encode(raw: &[u8]) -> String {
    const SET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in raw.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(SET[((n >> (18 - i * 6)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

// -- the calls ----------------------------------------------------------------------------------

fn answer(root: &Path, name: &str, method: &str, path: &str, body: &[u8]) -> (u16, J) {
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    match (method, parts.as_slice()) {
        ("GET", [""]) | ("GET", []) => (200, holdings(root, name)),
        ("GET", ["dataset", named]) => match open(root, named) {
            Ok(ds) => (200, ds.describe()),
            Err(e) => (404, json!({ "refused": e })),
        },
        ("GET", ["dataset", named, "runs"]) => match open(root, named) {
            Ok(ds) => (200, runs(&ds)),
            Err(e) => (404, json!({ "refused": e })),
        },
        ("GET", ["dataset", named, "declaration"]) => match declaration_of(root, named) {
            Ok(text) => (200, json!({ "dataset": named, "declaration": text })),
            Err(e) => (404, json!({ "refused": e })),
        },
        ("GET", ["dataset", named, "check"]) => match open(root, named) {
            Ok(ds) => (200, json!({ "dataset": ds.decl.name, "wrong": ds.check() })),
            Err(e) => (404, json!({ "refused": e })),
        },
        ("POST", ["dataset", named, "run"]) => run_now(root, named),
        ("PUT", ["dataset", named, "declaration"]) => apply(root, named, body),
        ("GET", ["scope", named]) => match open_scope(root, named) {
            Ok(s) => (200, s.describe()),
            Err(e) => (404, json!({ "refused": e })),
        },
        ("GET", ["scope", named, "check"]) => match open_scope(root, named) {
            Ok(s) => (200, json!({ "scope": s.decl.name, "wrong": s.check() })),
            Err(e) => (404, json!({ "refused": e })),
        },
        _ => (
            404,
            json!({ "refused": format!("{method} {path} is nothing here") }),
        ),
    }
}

fn datasets(root: &Path) -> PathBuf {
    root.join("datasets")
}

/// A name in a call is one segment, and a call cannot reach outside the deployment it is against.
fn at(root: &Path, kind: &str, named: &str) -> Result<PathBuf, String> {
    if named.is_empty() || named.contains('/') || named.contains("..") {
        return Err(format!("{named}: a name, not a path"));
    }
    let dir = root.join(kind).join(named);
    if !dir.exists() {
        return Err(format!("{named}: nothing of that name here"));
    }
    Ok(dir)
}

fn open(root: &Path, named: &str) -> Result<Dataset, String> {
    Dataset::open(&at(root, "datasets", named)?)
}

fn open_scope(root: &Path, named: &str) -> Result<Scope, String> {
    Scope::open(&at(root, "scopes", named)?, &datasets(root))
}

fn declaration_of(root: &Path, named: &str) -> Result<String, String> {
    let path = at(root, "datasets", named)?.join("dataset.toml");
    std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
}

fn holdings(root: &Path, name: &str) -> J {
    let held: Vec<J> = scope::registry(&datasets(root))
        .iter()
        .filter_map(|(named, dir)| {
            let ds = Dataset::open(dir).ok()?;
            Some(json!({
                "dataset": named,
                "kind": ds.decl.kind,
                "records": ds.store.count(),
                "state": ds.state(),
                "at": dir.file_name().map(|n| n.to_string_lossy().to_string()),
                "next_run": ds.next_run(),
            }))
        })
        .collect();
    let scopes: Vec<J> = scope::scope_registry(&root.join("scopes"))
        .iter()
        .filter_map(|(named, dir)| {
            let s = Scope::open(dir, &datasets(root)).ok()?;
            let (holds, late) = s.promise();
            Some(json!({
                "scope": named,
                "members": s.members.len(),
                "records": s.records(),
                "promise_holds": holds,
                "behind": late,
                "at": dir.file_name().map(|n| n.to_string_lossy().to_string()),
            }))
        })
        .collect();
    json!({
        "deployment": name,
        "zetlyn": env!("CARGO_PKG_VERSION"),
        "datasets": held,
        "scopes": scopes,
    })
}

fn runs(ds: &Dataset) -> J {
    let last = ds.store.last_run();
    let reports: Vec<J> = (0..20)
        .filter_map(|back| {
            let id = last.checked_sub(back)?;
            let r = ds.store.run_report(id)?;
            Some(json!({
                "id": r.id, "started": r.started, "finished": r.finished,
                "complete": r.complete, "added": r.added, "changed": r.changed,
                "removed": r.removed, "unchanged": r.unchanged,
                "error": r.error, "refused": r.refused,
                // What the run could not make sense of. A fact rather than a guess, and the only
                // honest input to anything that proposes a change to a declaration.
                "unparsed": r.unparsed, "no_text": r.no_text, "no_known": r.no_known,
                "duplicates": r.duplicates, "note": r.note,
            }))
        })
        .collect();
    json!({ "dataset": ds.decl.name, "runs": reports })
}

fn run_now(root: &Path, named: &str) -> (u16, J) {
    let dir = match at(root, "datasets", named) {
        Ok(d) => d,
        Err(e) => return (404, json!({ "refused": e })),
    };
    let ds = match Dataset::open(&dir) {
        Ok(d) => d,
        Err(e) => return (404, json!({ "refused": e })),
    };
    match ds.run() {
        Ok(r) => (
            200,
            json!({ "dataset": ds.decl.name, "run": r.id, "complete": r.complete,
                    "added": r.added, "changed": r.changed, "removed": r.removed,
                    "unchanged": r.unchanged, "error": r.error, "refused": r.refused }),
        ),
        Err(e) => (500, json!({ "failed": e })),
    }
}

/// The dangerous one, and the reason a grant separates it from `run`.
///
/// What arrives is held against three things before it is kept: it parses, the dataset opens
/// under it, and the dataset still answers for itself. The declaration that was there is written
/// beside it first, so an apply that passes all three and is still wrong is one file move away
/// from undone.
fn apply(root: &Path, named: &str, body: &[u8]) -> (u16, J) {
    let dir = match at(root, "datasets", named) {
        Ok(d) => d,
        Err(e) => return (404, json!({ "refused": e })),
    };
    let Ok(text) = String::from_utf8(body.to_vec()) else {
        return (400, json!({ "refused": "a declaration is text" }));
    };
    let path = dir.join("dataset.toml");
    let before = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return (500, json!({ "failed": format!("{}: {e}", path.display()) })),
    };
    if text == before {
        return (
            200,
            json!({ "dataset": named, "applied": false, "why": "it is what is there" }),
        );
    }
    if let Err(e) = std::fs::write(dir.join("dataset.toml.before"), &before) {
        return (
            500,
            json!({ "failed": format!("keeping the old one: {e}") }),
        );
    }
    if let Err(e) = std::fs::write(&path, &text) {
        return (500, json!({ "failed": format!("{}: {e}", path.display()) }));
    }
    // Nothing was applied, so nothing is kept. A copy left beside a declaration that was never
    // replaced is a file somebody finds later and cannot date.
    let put_back = |why: String| -> (u16, J) {
        let _ = std::fs::write(&path, &before);
        let _ = std::fs::remove_file(dir.join("dataset.toml.before"));
        (400, json!({ "refused": why, "applied": false }))
    };
    let ds = match Dataset::open(&dir) {
        Ok(d) => d,
        Err(e) => return put_back(e),
    };
    if ds.decl.name != *named && !ds.decl.name.ends_with(&format!("/{named}")) {
        return put_back(format!(
            "that declaration calls itself {}, and this is {named}",
            ds.decl.name
        ));
    }
    let wrong = ds.check();
    if !wrong.is_empty() {
        return put_back(wrong.join("; "));
    }
    (
        200,
        json!({ "dataset": ds.decl.name, "applied": true, "kept": "dataset.toml.before" }),
    )
}

// ---------------------------------------------------------------------------------------------
// Driving one.

/// One signed call to a console somewhere else. This is the whole of what a platform does to a
/// deployment, so it lives beside the console it talks to rather than in whatever calls it.
pub struct Driver {
    pub at: String,
    /// The grant, as the operator signed it, carried on every call.
    pub grant: Vec<u8>,
    agent: ureq::Agent,
}

impl Driver {
    pub fn new(at: &str, grant: Vec<u8>) -> Driver {
        let agent = ureq::Agent::config_builder()
            .user_agent(concat!("zetlyn/", env!("CARGO_PKG_VERSION")))
            .timeout_global(Some(std::time::Duration::from_secs(600)))
            // A refusal carries its reason in the body, and a client that turns the status into
            // an error throws the reason away.
            .http_status_as_error(false)
            .build()
            .into();
        Driver {
            at: at.trim_end_matches('/').to_string(),
            grant,
            agent,
        }
    }

    /// Signs as the identity, because the grant names it.
    pub fn call(&self, method: &str, path: &str, body: &[u8]) -> Result<(u16, String), String> {
        let at = crate::iso_stamp(crate::now());
        let statement = crate::grant::request_statement(method, path, body, &at);
        let signature = crate::identity::sign(statement.as_bytes())?.ok_or(
            "no identity to sign with. `zetlyn id new` makes one, and a console takes nothing \
             unsigned",
        )?;
        let carried = encode(&self.grant);
        let address = format!("{}{path}", self.at);
        let mut response = match method {
            "GET" => self
                .agent
                .get(&address)
                .header("Zetlyn-Grant", &carried)
                .header("Zetlyn-Date", &at)
                .header("Zetlyn-Signature", &signature)
                .call(),
            "POST" | "PUT" => {
                let builder = if method == "POST" {
                    self.agent.post(&address)
                } else {
                    self.agent.put(&address)
                };
                builder
                    .header("Zetlyn-Grant", &carried)
                    .header("Zetlyn-Date", &at)
                    .header("Zetlyn-Signature", &signature)
                    .header("Content-Type", "text/plain")
                    .send(body)
            }
            other => return Err(format!("{other}: a console answers GET, POST and PUT")),
        }
        .map_err(|e| format!("{address}: {e}"))?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("{address}: {e}"))?;
        Ok((status, text))
    }

    /// The same, where the answer is expected to be JSON and a refusal is a failure.
    pub fn ask(&self, method: &str, path: &str, body: &[u8]) -> Result<J, String> {
        let (status, text) = self.call(method, path, body)?;
        let answer: J = serde_json::from_str(&text)
            .unwrap_or_else(|_| json!({ "refused": text.trim().to_string() }));
        if status >= 400 {
            let why = answer["refused"].as_str().unwrap_or("refused").to_string();
            return Err(format!("{}{path}: {why}", self.at));
        }
        Ok(answer)
    }
}
