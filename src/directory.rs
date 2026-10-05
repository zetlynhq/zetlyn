//! A directory of worlds (FEDERATION.md, M15).
//!
//! It holds addresses, never claims: a world registers with a call signed by its own key, the
//! directory reads the world's document back and keeps what it says about itself (its title, its
//! trackers, its sources), reads it again every few hours, follows it when it moves, and lets it go
//! after a week of not answering. Search is over those descriptions; a question about data still
//! goes to the world that holds it.
//!
//! `zetlyn.com/directory` is the first one. Any world is one too that says `directory: true`, at
//! `<its address>/directory`, and a world may be in several.

use std::path::Path;

use maud::html;
use rusqlite::Connection;
use serde_json::{json, Value as J};

const DB: &str = "directory.db";
/// How long a world's description stands before it is read again.
const REREAD: i64 = 6 * 3600;
/// How long a world may not answer before it is let go.
const GONE_AFTER: i64 = 7 * 86_400;
/// How far apart the registering world's clock and this one may be.
const WINDOW: i64 = 300;

fn open(root: &Path) -> Result<Connection, String> {
    let db = Connection::open(root.join(DB)).map_err(|e| e.to_string())?;
    db.execute_batch(
        "pragma journal_mode=wal;
         create table if not exists world(
           url        text primary key,
           key        text not null,
           doc        text not null,
           registered text not null,
           read_at    integer not null,
           failing_since integer);",
    )
    .map_err(|e| e.to_string())?;
    Ok(db)
}

/// One world as the directory holds it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub url: String,
    pub doc: J,
    pub registered: String,
}

/// A world registering: `body` is `{"world": <address>, "at": <time>}`, signed with that world's
/// operator key. The world's document is read back, and its key has to be the one that signed.
/// The address it is kept under.
pub fn register(root: &Path, body: &[u8], signature: &str) -> Result<String, String> {
    let asked: J = serde_json::from_slice(body).map_err(|_| "not JSON")?;
    let url = asked["world"].as_str().ok_or("`world`: the address of the world")?.trim_end_matches('/').to_string();
    let at = asked["at"].as_i64().ok_or("`at`: when it was signed, in seconds")?;
    if (crate::now() - at).abs() > WINDOW {
        return Err("signed more than five minutes from now: send it again".into());
    }
    // Asked on a stranger's say-so: a public https address only, never this machine's own network.
    let here = crate::account::Site::load(root).url;
    crate::outbound::allowed(&url, crate::outbound::on_loopback(&here))?;
    let doc = crate::world::fetch(&url)?.ok_or_else(|| format!("{url} does not answer as a world"))?;
    let key = doc["key"].as_str().ok_or("the world names no key")?.to_string();
    crate::key::verify(&key, body, signature).map_err(|_| format!("not signed by {url}'s own key"))?;
    if doc["world"].as_str().map(|w| w.trim_end_matches('/')) != Some(url.as_str()) {
        return Err(format!("{url}'s document says it is {}", doc["world"].as_str().unwrap_or("somewhere else")));
    }
    if doc["moved_to"].as_str().is_some_and(|m| !m.is_empty()) {
        return Err(format!("{url} has moved; register where it is now"));
    }
    let db = open(root)?;
    db.execute(
        "insert into world(url, key, doc, registered, read_at, failing_since) values(?1,?2,?3,?4,?5,null)
         on conflict(url) do update set key = ?2, doc = ?3, read_at = ?5, failing_since = null",
        rusqlite::params![url, key, doc.to_string(), crate::iso_stamp(crate::now()), crate::now()],
    )
    .map_err(|e| e.to_string())?;
    Ok(url)
}

/// Every world whose description is older than it should be, read again: a world that moved is
/// followed, signed by the same key; one that does not answer for a week is let go. What happened.
pub fn refresh(root: &Path) -> Result<(usize, usize, usize), String> {
    if !root.join(DB).exists() {
        return Ok((0, 0, 0));
    }
    let loopback_ok = crate::outbound::on_loopback(&crate::account::Site::load(root).url);
    let db = open(root)?;
    let due: Vec<(String, String, Option<i64>)> = {
        let mut stmt = db.prepare("select url, key, failing_since from world where read_at < ?1").map_err(|e| e.to_string())?;
        let rows = stmt.query_map(rusqlite::params![crate::now() - REREAD], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(|e| e.to_string())?;
        rows.flatten().collect()
    };
    let (mut read, mut moved, mut gone) = (0, 0, 0);
    for (url, key, failing) in due {
        let now = crate::now();
        match crate::world::fetch(&url) {
            // Signed by the key it was listed with, and saying it is the world at this address.
            Ok(Some(doc)) if doc["key"].as_str() == Some(key.as_str()) && same(&doc, &url) => {
                if let Some(to) = doc["moved_to"].as_str().filter(|m| !m.is_empty()).map(|m| m.trim_end_matches('/').to_string()) {
                    let there = if crate::outbound::allowed(&to, loopback_ok).is_ok() { crate::world::fetch(&to) } else { Ok(None) };
                    match there {
                        Ok(Some(there)) if there["key"].as_str() == Some(key.as_str()) && same(&there, &to) && there["moved_to"].as_str().map_or(true, str::is_empty) => {
                            let _ = db.execute("delete from world where url = ?1", rusqlite::params![url]);
                            let _ = db.execute(
                                "insert or replace into world(url, key, doc, registered, read_at, failing_since) values(?1,?2,?3,?4,?5,null)",
                                rusqlite::params![to, key, there.to_string(), crate::iso_stamp(now), now],
                            );
                            moved += 1;
                        }
                        // Gone somewhere that is not it: as good as not answering, and let go in time.
                        _ => {
                            let since = failing.unwrap_or(now);
                            if now - since > GONE_AFTER {
                                let _ = db.execute("delete from world where url = ?1", rusqlite::params![url]);
                                gone += 1;
                            } else {
                                let _ = db.execute("update world set read_at = ?2, failing_since = ?3 where url = ?1", rusqlite::params![url, now, since]);
                            }
                        }
                    }
                } else {
                    let _ = db.execute("update world set doc = ?2, read_at = ?3, failing_since = null where url = ?1", rusqlite::params![url, doc.to_string(), now]);
                    read += 1;
                }
            }
            // Not answering, answering as somebody else, or not a world any more.
            _ => {
                let since = failing.unwrap_or(now);
                if now - since > GONE_AFTER {
                    let _ = db.execute("delete from world where url = ?1", rusqlite::params![url]);
                    gone += 1;
                } else {
                    let _ = db.execute("update world set read_at = ?2, failing_since = ?3 where url = ?1", rusqlite::params![url, now, since]);
                }
            }
        }
    }
    Ok((read, moved, gone))
}

/// A world's document says it is the world at this address.
fn same(doc: &serde_json::Value, url: &str) -> bool {
    doc["world"].as_str().map(|w| w.trim_end_matches('/')) == Some(url.trim_end_matches('/'))
}

/// The worlds it holds, those that answer first and then by title; with `q`, only those whose
/// title, trackers or sources say it.
pub fn list(root: &Path, q: &str) -> Vec<Entry> {
    let Ok(db) = open(root) else { return Vec::new() };
    let Ok(mut stmt) = db.prepare("select url, doc, registered from world order by failing_since is not null, url") else { return Vec::new() };
    let rows: Vec<Entry> = stmt
        .query_map([], |r| {
            let doc: String = r.get(1)?;
            Ok(Entry { url: r.get(0)?, doc: serde_json::from_str(&doc).unwrap_or(J::Null), registered: r.get(2)? })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    let q = q.trim().to_lowercase();
    let mut out: Vec<Entry> = rows.into_iter().filter(|e| q.is_empty() || said(&e.doc).to_lowercase().contains(&q)).collect();
    out.sort_by_key(|e| e.doc["title"].as_str().unwrap_or(&e.url).to_lowercase());
    out
}

/// A link a registered world's document asks for, where it is one this page may carry: http(s),
/// and at or under that world's own address. Anything else is shown as text, never as a link a
/// reader might follow into a script.
fn under(world: &str, href: &str) -> Option<String> {
    let world = world.trim_end_matches('/');
    let ok_scheme = world.starts_with("https://") || world.starts_with("http://");
    let inside = href == world || href.starts_with(&format!("{world}/"));
    (ok_scheme && inside && !href.chars().any(|c| c.is_control() || c.is_whitespace() || c == '"' || c == '<' || c == '>')).then(|| href.to_string())
}

/// Everything a world says about itself that a search may match.
fn said(doc: &J) -> String {
    let mut words = vec![doc["title"].as_str().unwrap_or("").to_string(), doc["world"].as_str().unwrap_or("").to_string()];
    for list in ["trackers", "sources"] {
        for t in doc[list].as_array().cloned().unwrap_or_default() {
            words.push(t["title"].as_str().unwrap_or("").to_string());
            words.push(t["name"].as_str().unwrap_or("").to_string());
        }
    }
    words.join(" ")
}

/// `/directory`, `/directory.json` and `/directory/register`, for the directory at `root`.
pub fn answer(root: &Path, mut request: tiny_http::Request, rel: &[String], url: &str) -> Option<tiny_http::Request> {
    let parts: Vec<&str> = rel.iter().map(String::as_str).collect();
    let reply = |request: tiny_http::Request, status: u16, kind: &str, body: String| {
        let mut response = tiny_http::Response::from_string(body).with_status_code(status);
        if let Ok(h) = tiny_http::Header::from_bytes(&b"Content-Type"[..], kind.as_bytes()) {
            response = response.with_header(h);
        }
        let _ = request.respond(response);
    };
    let q = url.split_once('?').and_then(|(_, q)| q.split('&').find_map(|p| p.strip_prefix("q="))).map(|v| crate::serve::urldecode(&v.replace('+', " "))).unwrap_or_default();
    match parts.as_slice() {
        ["directory.json"] => {
            let all: Vec<J> = list(root, &q).into_iter().map(|e| json!({ "world": e.url, "registered": e.registered, "title": e.doc["title"], "trackers": e.doc["trackers"], "sources": e.doc["sources"] })).collect();
            reply(request, 200, "application/json", json!({ "worlds": all }).to_string());
        }
        ["directory", "register"] if request.method() == &tiny_http::Method::Post => {
            let signature = request.headers().iter().find(|h| h.field.equiv("X-Zetlyn-Signature")).map(|h| h.value.as_str().to_string()).unwrap_or_default();
            let mut body = Vec::new();
            let _ = std::io::Read::read_to_end(&mut std::io::Read::take(request.as_reader(), 16 * 1024), &mut body);
            match register(root, &body, &signature) {
                Ok(world) => reply(request, 200, "application/json", json!({ "registered": world }).to_string()),
                Err(e) => reply(request, 400, "application/json", json!({ "error": e }).to_string()),
            }
        }
        ["directory"] => {
            let all = list(root, &q);
            let body = html! {
                h1 { "Worlds" }
                p.about { "Every world that asked to be listed here, as it describes itself. Each one keeps its own data and answers its own questions; this only says where they are." }
                form.bar method="get" { input type="search" name="q" value=(q) placeholder="a tracker, a source, a world"; button type="submit" { "Search" } }
                @if all.is_empty() { p.dim { @if q.is_empty() { "None yet." } @else { "Nothing here says that." } } }
                @for e in &all {
                    div.card {
                        h3 { a href=(under(&e.url, &e.url).unwrap_or_default()) { (e.doc["title"].as_str().filter(|t| !t.is_empty()).unwrap_or(&e.url)) } }
                        p.dim { code { (e.url) } }
                        @for t in e.doc["trackers"].as_array().cloned().unwrap_or_default() {
                            p { @match under(&e.url, t["at"].as_str().unwrap_or("")) { Some(href) => { a href=(href) { (t["title"].as_str().unwrap_or("")) } } None => { (t["title"].as_str().unwrap_or("")) } } " " span.chip { "tracker" } }
                        }
                        @let sources = e.doc["sources"].as_array().cloned().unwrap_or_default();
                        @if !sources.is_empty() {
                            p.dim { (sources.iter().filter_map(|s| s["title"].as_str()).collect::<Vec<_>>().join(" · ")) }
                        }
                    }
                }
                p.dim { "A world of yours lists itself here with " code { "zetlyn world register <workspace> --at <this address>" } "." }
            };
            reply(request, 200, "text/html; charset=utf-8", crate::serve::shell("Worlds", body));
        }
        _ => return Some(request),
    }
    None
}

/// The world at `root` asks the directory at `at` to list it, signed with its own key, and
/// remembers that it is listed there.
pub fn ask_to_be_listed(root: &Path, at: &str) -> Result<String, String> {
    let at = at.trim_end_matches('/');
    let world = crate::account::Site::for_workspace(root).url;
    if world.is_empty() {
        return Err("this workspace names no address (`url:`), so there is nothing to list".into());
    }
    let body = json!({ "world": world, "at": crate::now() }).to_string();
    // Made where it has never been needed yet, as its document would make it.
    crate::propose::operator_key(root)?;
    let signature = crate::key::sign(root, crate::grant::OPERATOR_KEY, body.as_bytes())?.ok_or("this world has no key")?;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(60))).http_status_as_error(false).build().into();
    let target = format!("{at}/register");
    let mut r = agent.post(&target).header("Content-Type", "application/json").header("X-Zetlyn-Signature", &signature).send(body.as_str()).map_err(|e| format!("{target}: {e}"))?;
    let status = r.status();
    let answer: J = r.body_mut().read_json().unwrap_or(J::Null);
    if !status.is_success() {
        return Err(format!("{at}: {}", answer["error"].as_str().unwrap_or("refused")));
    }
    // Said in its workspace.yaml, so its document names the directories it is in.
    let site = crate::account::Site::load(root);
    if !site.directories.iter().any(|d| d.trim_end_matches('/') == at) {
        let file = root.join(crate::account::WORKSPACE);
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let mut all = site.directories.clone();
        all.push(at.to_string());
        let line = format!("directories: {}", serde_json::to_string(&all).unwrap_or_default());
        let kept: Vec<&str> = text.lines().filter(|l| !l.starts_with("directories:")).collect();
        std::fs::write(&file, format!("{}\n{line}\n", kept.join("\n").trim_end())).map_err(|e| e.to_string())?;
    }
    Ok(world)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    fn up(port: u16) {
        for _ in 0..50 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("nothing answered on {port}");
    }

    fn get(url: &str) -> String {
        let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
        agent.get(url).call().unwrap().body_mut().read_to_string().unwrap()
    }

    #[test]
    fn a_world_lists_itself_with_its_own_key_and_is_found_there_and_kept_current() {
        let base = std::env::temp_dir().join(format!("zetlyn-directory-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // The directory: a machine.
        let pd = free_port();
        let d = base.join("machine");
        std::fs::create_dir_all(d.join("orgs")).unwrap();
        let url_d = format!("http://127.0.0.1:{pd}");
        std::fs::write(d.join("workspace.yaml"), format!("title: The machine\nurl: {url_d}\n")).unwrap();
        let args: Vec<String> = ["hosting", "serve", d.to_str().unwrap(), "--addr", &format!("127.0.0.1:{pd}")].iter().map(|s| s.to_string()).collect();
        // Ends with the test process; nothing outlives it.
        std::thread::spawn(move || crate::app::hosting(&args));
        up(pd);
        // A world with a tracker nobody here knew of.
        let pa = free_port();
        let a = base.join("a");
        std::fs::create_dir_all(a.join("sources")).unwrap();
        std::fs::create_dir_all(a.join("trackers/bikes")).unwrap();
        let url_a = format!("http://127.0.0.1:{pa}");
        std::fs::write(a.join("workspace.yaml"), format!("title: Bike prices\nurl: {url_a}\nowners: [ann@example.org]\n")).unwrap();
        std::fs::write(a.join("trackers/bikes/tracker.yaml"), "name: a/bikes\ntitle: Cargo bikes, what they cost\nidentified_by: [price]\n").unwrap();
        let args: Vec<String> = ["world", "serve", a.to_str().unwrap(), "--addr", &format!("127.0.0.1:{pa}")].iter().map(|s| s.to_string()).collect();
        std::thread::spawn(move || crate::app::world_serve(&args));
        up(pa);

        assert!(get(&format!("{url_d}/directory")).contains("None yet."));
        let listed = ask_to_be_listed(&a, &format!("{url_d}/directory")).unwrap();
        assert_eq!(listed, url_a);
        assert_eq!(crate::account::Site::load(&a).directories, vec![format!("{url_d}/directory")]);
        assert_eq!(crate::world::signed_document(&a).unwrap()["directories"], json!([format!("{url_d}/directory")]));
        // Found, by what it is about.
        let page = get(&format!("{url_d}/directory?q=cargo"));
        assert!(page.contains("Bike prices") && page.contains("Cargo bikes, what they cost") && page.contains(&format!("{url_a}/trackers/bikes/")), "{page}");
        assert!(!get(&format!("{url_d}/directory?q=submarines")).contains("Bike prices"));
        let json: J = serde_json::from_str(&get(&format!("{url_d}/directory.json"))).unwrap();
        assert_eq!(json["worlds"][0]["world"], url_a);

        // Nobody lists a world but its own key, and not with an old signature.
        let body = json!({ "world": url_a, "at": crate::now() }).to_string();
        crate::key::new(&base, "stranger.key").unwrap();
        let forged = crate::key::sign(&base, "stranger.key", body.as_bytes()).unwrap().unwrap();
        assert!(register(&d, body.as_bytes(), &forged).unwrap_err().contains("own key"));
        let old = json!({ "world": url_a, "at": crate::now() - 3600 }).to_string();
        let signed = crate::key::sign(&a, crate::grant::OPERATOR_KEY, old.as_bytes()).unwrap().unwrap();
        assert!(register(&d, old.as_bytes(), &signed).unwrap_err().contains("five minutes"));

        // Read again when it is due: what it says now is what is listed.
        std::fs::write(a.join("trackers/bikes/tracker.yaml"), "name: a/bikes\ntitle: Cargo bikes and trailers\nidentified_by: [price]\n").unwrap();
        open(&d).unwrap().execute("update world set read_at = 0", []).unwrap();
        assert_eq!(refresh(&d).unwrap(), (1, 0, 0));
        assert!(get(&format!("{url_d}/directory")).contains("Cargo bikes and trailers"));

        // It moves, and the directory follows it to the world with the same key.
        let pb = free_port();
        let (b, url_b) = (base.join("b"), format!("http://127.0.0.1:{pb}"));
        crate::world::export(&a, &base.join("a.tar.gz")).unwrap();
        crate::world::import(&base.join("a.tar.gz"), &b, Some(&url_b), None).unwrap();
        let args: Vec<String> = ["world", "serve", b.to_str().unwrap(), "--addr", &format!("127.0.0.1:{pb}")].iter().map(|s| s.to_string()).collect();
        std::thread::spawn(move || crate::app::world_serve(&args));
        up(pb);
        crate::world::move_to(&a, &url_b, false).unwrap();
        open(&d).unwrap().execute("update world set read_at = 0", []).unwrap();
        assert_eq!(refresh(&d).unwrap(), (0, 1, 0));
        let held: Vec<String> = list(&d, "").into_iter().map(|e| e.url).collect();
        assert_eq!(held, vec![url_b.clone()]);

        // A world that has not answered for a week is let go; one that has for a day is kept.
        let db = open(&d).unwrap();
        for (url, since) in [("http://127.0.0.1:1", crate::now() - 8 * 86_400), ("http://127.0.0.1:2", crate::now() - 86_400)] {
            db.execute("insert into world(url, key, doc, registered, read_at, failing_since) values(?1, 'k', '{}', 'then', 0, ?2)", rusqlite::params![url, since]).unwrap();
        }
        let (_, _, gone) = refresh(&d).unwrap();
        assert_eq!(gone, 1);
        let held: Vec<String> = list(&d, "").into_iter().map(|e| e.url).collect();
        assert!(held.contains(&"http://127.0.0.1:2".to_string()) && !held.contains(&"http://127.0.0.1:1".to_string()));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_link_from_a_registered_world_is_one_only_under_its_own_address() {
        let w = "https://w.example";
        assert_eq!(under(w, "https://w.example/t/x/").as_deref(), Some("https://w.example/t/x/"));
        for bad in ["javascript:alert(1)", "https://w.example.evil/", "https://other.example/", "https://w.example/\"onmouseover=x", "data:text/html,x"] {
            assert!(under(w, bad).is_none(), "{bad}");
        }
        assert!(under("javascript:x", "javascript:x").is_none());
    }

    #[test]
    fn nobody_registers_a_world_on_this_machines_own_network() {
        let root = std::env::temp_dir().join(format!("zetlyn-directory-ssrf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("workspace.yaml"), "url: https://directory.example\n").unwrap();
        for world in ["http://169.254.169.254/latest", "https://127.0.0.1:6379", "https://10.0.0.5"] {
            let body = json!({ "world": world, "at": crate::now() }).to_string();
            let e = register(&root, body.as_bytes(), "ed25519:00").unwrap_err();
            assert!(e.contains("public internet") || e.contains("only https"), "{world}: {e}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
