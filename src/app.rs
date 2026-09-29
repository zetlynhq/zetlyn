//! `zetlyn` on its own: the workspace, in a browser, for the person at the machine.
//!
//! Everything here is a command this program already has, asked from a page: `source new` reads a
//! source, `source update` reads it whole, and a tracker is a file naming its sources. What this
//! adds is the order a newcomer meets them in: what do you want to track, the first source, is it
//! right, a second source, how many they share, connect. There are no accounts: whoever runs the
//! program owns what it holds, and sees every page of it. The trackers themselves are served as
//! they are published, each under `/t/<name>/`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use maud::{html, Markup, PreEscaped};
use serde_json::{json, Value as J};

use crate::serve::{self, urlencode};
use crate::servetracker::TrackerSite;
use crate::source::{Query, Source};
use crate::tracker::Tracker;
use crate::trackerdecl::TrackerDecl;

/// The example the start page offers: two files, both CSV, both carrying the CVE number, one
/// saying a vulnerability is exploited and the other that code for it exists.
const EXAMPLE: [(&str, &str, &str); 2] = [
    (
        "https://www.cisa.gov/sites/default/files/csv/known_exploited_vulnerabilities.csv",
        "CISA Known Exploited Vulnerabilities",
        "Which vulnerabilities are being exploited right now, by CISA's evidence.",
    ),
    (
        "https://gitlab.com/exploit-database/exploitdb/-/raw/main/files_exploits.csv",
        "Exploit-DB",
        "Whether working code exists for a vulnerability.",
    ),
];
const EXAMPLE_TITLE: &str = "Exploited vulnerabilities";

pub fn run(args: &[String]) -> Result<(), String> {
    let root = workspace(args)?;
    let first_port: u16 = crate::flag(args, "--port").and_then(|p| p.parse().ok()).unwrap_or(4747);
    // The next free port, so a second copy started by accident says where it is rather than dying.
    let (server, addr) = (first_port..first_port + 20)
        .find_map(|p| {
            let a = format!("127.0.0.1:{p}");
            tiny_http::Server::http(&a).ok().map(|s| (s, a))
        })
        .ok_or_else(|| format!("no free port from {first_port}"))?;
    let url = format!("http://{addr}/");
    println!("{} on {url}", root.display());
    if !args.iter().any(|a| a == "--no-open") {
        open_browser(&url);
    }
    let mut app = App { root, addr, sites: BTreeMap::new(), jobs: Arc::new(Mutex::new(Jobs::default())), base: String::new(), hosted: None, visitor: false };
    for request in server.incoming_requests() {
        app.answer(request);
    }
    Ok(())
}

/// The current directory if it is a workspace, and `~/Zetlyn` otherwise, made on first use.
fn workspace(args: &[String]) -> Result<PathBuf, String> {
    if let Some(named) = crate::positional(args, 1).first() {
        return Ok(PathBuf::from(named.as_str()));
    }
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    if here.join("workspace.yaml").exists() || here.join("trackers").is_dir() {
        return Ok(here);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or("no HOME to put ~/Zetlyn in")?;
    let root = home.join("Zetlyn");
    for d in ["sources", "trackers"] {
        std::fs::create_dir_all(root.join(d)).map_err(|e| format!("{}: {e}", root.display()))?;
    }
    let file = root.join("workspace.yaml");
    if !file.exists() {
        std::fs::write(&file, "title: Zetlyn\n").map_err(|e| format!("{}: {e}", file.display()))?;
    }
    Ok(root)
}

fn open_browser(url: &str) {
    let (program, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(target_os = "windows") {
        ("cmd", vec!["/c", "start", "", url])
    } else {
        ("xdg-open", vec![url])
    };
    // The browser is the person's and outlives this program; nothing waits for it.
    if std::process::Command::new(program).args(args).spawn().is_err() {
        println!("open {url} in a browser");
    }
}

#[derive(Default)]
struct Jobs {
    next: u64,
    all: BTreeMap<u64, Job>,
}

#[derive(Default, Clone)]
struct Job {
    lines: Vec<String>,
    done: bool,
    error: Option<String>,
    /// Where the page goes when the job is done.
    then: String,
}

struct App {
    root: PathBuf,
    addr: String,
    /// Each tracker, opened on first visit and opened again after it changes.
    sites: BTreeMap<String, TrackerSite>,
    jobs: Arc<Mutex<Jobs>>,
    /// Where it is mounted: empty on this machine, `/<name>` hosted.
    base: String,
    hosted: Option<Hosted>,
    /// This request is not the owner's: the trackers answer it as they would a reader.
    visitor: bool,
}

/// A job's handle, for the thread doing it.
#[derive(Clone)]
struct Progress {
    jobs: Arc<Mutex<Jobs>>,
    id: u64,
}

impl Progress {
    fn say(&self, line: impl Into<String>) {
        if let Ok(mut j) = self.jobs.lock() {
            if let Some(job) = j.all.get_mut(&self.id) {
                job.lines.push(line.into());
            }
        }
    }
    fn finish(&self, result: Result<String, String>) {
        if let Ok(mut j) = self.jobs.lock() {
            if let Some(job) = j.all.get_mut(&self.id) {
                job.done = true;
                match result {
                    Ok(then) => job.then = then,
                    Err(e) => job.error = Some(e),
                }
            }
        }
    }
}

impl App {
    fn sources(&self) -> PathBuf {
        self.root.join("sources")
    }
    fn trackers(&self) -> PathBuf {
        self.root.join("trackers")
    }

    fn start(&self, work: impl FnOnce(&Progress) -> Result<String, String> + Send + 'static) -> u64 {
        let id = {
            let mut j = self.jobs.lock().expect("the job list");
            j.next += 1;
            let id = j.next;
            j.all.insert(id, Job::default());
            id
        };
        let p = Progress { jobs: self.jobs.clone(), id };
        std::thread::spawn(move || {
            let result = work(&p);
            p.finish(result);
        });
        id
    }

    fn answer(&mut self, request: tiny_http::Request) {
        // Hosted, every address carries the workspace's prefix; the router matches without it.
        let url = serve::unmount(request.url());
        let path = url.split('?').next().unwrap_or("/").to_string();
        let parts: Vec<String> = path.split('/').filter(|s| !s.is_empty()).map(serve::urldecode).collect();
        if self.hosted.is_some() {
            if let Some(request) = self.hosted_gate(request, &url, &path, &parts) {
                return self.answer_as_owner(request, url, path, parts);
            }
            return;
        }
        self.answer_as_owner(request, url, path, parts)
    }

    /// What anybody may reach on a hosted workspace, and whether this request is its owner's. The
    /// request comes back where the owner's app should answer it; otherwise it has been answered.
    fn hosted_gate(&mut self, mut request: tiny_http::Request, url: &str, path: &str, parts: &[String]) -> Option<tiny_http::Request> {
        let h = self.hosted.as_ref()?;
        let header = |name: &'static str| request.headers().iter().find(|x| x.field.equiv(name)).map(|x| x.value.as_str().to_string());
        let (cookie, signature) = (header("Cookie"), header("X-Hub-Signature-256").or_else(|| header("X-Zetlyn-Signature")));
        let session = cookie.and_then(|c| c.split(';').filter_map(|p| p.trim().split_once('=')).find(|(k, _)| *k == "zs").map(|(_, v)| v.to_string()));
        let owner = session.and_then(|s| h.accounts.by_session(&s)).is_some_and(|a| a.email.eq_ignore_ascii_case(&h.owner));
        let post = request.method() == &tiny_http::Method::Post;
        let html_kind = "text/html; charset=utf-8";
        match parts.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["style.css"] => Some(request),
            // Signed by its sender, so nobody signs in to push to a source.
            ["hook", source] if post => {
                let sig = signature;
                let mut body = Vec::new();
                let _ = std::io::Read::read_to_end(request.as_reader(), &mut body);
                let dir = self.sources().join(source);
                match crate::hook::receive(&dir, &body, sig.as_deref()) {
                    Ok(name) => {
                        let root = self.root.clone();
                        // Read now, and the trackers that name it look again, so a push is a
                        // signal in seconds and not at the next scheduled pass.
                        std::thread::spawn(move || {
                            if let Ok(ds) = Source::open(&dir) {
                                let _ = ds.run();
                            }
                            for t in crate::tracker::scope_registry(&root.join("trackers")).values() {
                                let _ = Tracker::open(t, &root.join("sources")).and_then(|t| t.refresh_if_moved());
                            }
                        });
                        respond(request, 202, "application/json", &json!({ "kept": name }).to_string());
                    }
                    Err(e) => respond(request, if e.contains("signature") { 401 } else { 400 }, "application/json", &json!({ "error": e }).to_string()),
                }
                None
            }
            ["signin"] if post => {
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut body);
                let email = parse_form(&body).get("email").cloned().unwrap_or_default();
                // The same words whoever asks, so the page does not say whose workspace it is.
                if email.trim().eq_ignore_ascii_case(&h.owner) {
                    let sent = h.accounts.ensure(&h.owner).and_then(|a| h.accounts.new_link(a.id)).and_then(|raw| {
                        let site = crate::account::Site::load(&self.root);
                        let link = format!("{}{}", site.url.trim_end_matches('/'), serve::at(&format!("/signin/{raw}")));
                        site.send(&h.owner, "Your Zetlyn sign-in link", &format!("{link}\n\nGood for a quarter of an hour, and once."))
                    });
                    if let Err(e) = sent {
                        eprintln!("sign-in mail: {e}");
                    }
                }
                respond(request, 200, html_kind, &page("Sign in", html! { h1 { "Check your mail" } p { "If that address owns this workspace, a link to sign in is on its way. It is good for a quarter of an hour, and once." } }));
                None
            }
            ["signin"] => {
                respond(request, 200, html_kind, &page("Sign in", html! {
                    h1 { "Sign in" }
                    p.about { "The owner of this workspace signs in with a link sent to their address." }
                    form.bar method="post" action=(serve::at("/signin")) {
                        input.wide type="email" name="email" placeholder="you@example.org" required;
                        button.primary type="submit" { "Send me a link" }
                    }
                }));
                None
            }
            ["signin", raw] => {
                match h.accounts.spend_link(raw) {
                    Some(session) => {
                        let cookie = format!("zs={session}; Path={}; HttpOnly; Secure; SameSite=Lax; Max-Age=2592000", if self.base.is_empty() { "/" } else { &self.base });
                        let mut response = tiny_http::Response::from_string("").with_status_code(303);
                        for (k, v) in [("Location", serve::at("/")), ("Set-Cookie", cookie)] {
                            if let Ok(hd) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                                response = response.with_header(hd);
                            }
                        }
                        let _ = request.respond(response);
                    }
                    None => respond(request, 410, html_kind, &page("Sign in", html! { h1 { "That link is spent" } p { a href=(serve::at("/signin")) { "Ask for another" } } })),
                }
                None
            }
            // The trackers answer for themselves, the owner as their operator.
            ["t", ..] => {
                self.visitor = !owner;
                Some(request)
            }
            _ if owner => {
                self.visitor = false;
                Some(request)
            }
            [] => {
                respond(request, 200, html_kind, &self.visitor_page());
                None
            }
            _ => {
                let _ = url;
                let _ = path;
                redirect(request, &serve::at("/signin"));
                None
            }
        }
    }

    /// What somebody who does not own this workspace sees at its address: what it publishes.
    fn visitor_page(&self) -> String {
        let trackers = listed(&self.trackers());
        page("Zetlyn", html! {
            h1 { (crate::account::Site::load(&self.root).title) }
            @if trackers.is_empty() { p.dim { "Nothing published here yet." } }
            table { tbody {
                @for (name, decl, _) in &trackers {
                    tr { td { a href=(serve::at(&format!("/t/{name}/"))) { strong { (decl.title) } } @if decl.visibility == "private" { " " span.chip { "private" } } div.why { (decl.about) } } }
                }
            } }
            p.dim { a href=(serve::at("/signin")) { "Sign in" } " if this workspace is yours." }
        })
    }

    fn answer_as_owner(&mut self, mut request: tiny_http::Request, url: String, _path: String, parts: Vec<String>) {

        // A tracker's own pages, as a reader would see them published.
        if parts.first().map(String::as_str) == Some("t") && parts.len() >= 2 {
            let name = parts[1].clone();
            if !self.sites.contains_key(&name) {
                let dir = self.trackers().join(&name);
                match Tracker::open(&dir, &self.sources())
                    .and_then(|t| TrackerSite::open(t, &dir, &self.sources(), &self.addr, true))
                {
                    Ok(site) => {
                        self.sites.insert(name.clone(), site);
                    }
                    Err(e) => return respond(request, 404, "text/html; charset=utf-8", &page("Not here", html! { p { (e) } })),
                }
            }
            serve::mount(&format!("{}/t/{name}", self.base));
            if let Some(site) = self.sites.get_mut(&name) {
                site.set_operator(!self.visitor);
                site.answer(request);
            }
            serve::mount(&self.base);
            return;
        }

        let post = request.method() == &tiny_http::Method::Post;
        let mut body = Vec::new();
        if post {
            let _ = std::io::Read::read_to_end(request.as_reader(), &mut body);
        }
        let form = parse_form(&String::from_utf8_lossy(&body));
        let query = serve::params(&url);
        let html_kind = "text/html; charset=utf-8";
        let json_kind = "application/json";

        let (status, kind, text) = match (post, parts.iter().map(String::as_str).collect::<Vec<_>>().as_slice()) {
            (false, ["style.css"]) => (200, "text/css; charset=utf-8", format!("{}{APP_STYLE}", serve::STYLE)),
            (false, []) => (200, html_kind, self.start_page()),
            (true, ["new"]) => {
                let title = form.get("title").cloned().unwrap_or_default();
                let title = if title.trim().is_empty() { "My tracker".to_string() } else { title.trim().to_string() };
                let slug = self.free(&self.trackers(), &crate::guess::slug(&title));
                return redirect(request, &serve::at(&format!("/new/{slug}?title={}", urlencode(&title))));
            }
            (true, ["example"]) => {
                let slug = self.free(&self.trackers(), &crate::guess::slug(EXAMPLE_TITLE));
                return redirect(
                    request,
                    &format!("/new/{slug}?title={}&url={}", urlencode(EXAMPLE_TITLE), urlencode(EXAMPLE[0].0)),
                );
            }
            (false, ["new", tracker]) => (200, html_kind, self.source_page(tracker, &query)),
            // A file from the person's machine, kept in the source's own directory.
            (true, ["upload", tracker]) => {
                let name = query.get("name").cloned().unwrap_or_else(|| "upload.csv".into());
                match self.upload(&name, &body) {
                    Ok(path) => {
                        let id = self.analyse(tracker, path.display().to_string(), form_title(&query));
                        (200, json_kind, json!({ "job": id }).to_string())
                    }
                    Err(e) => (400, json_kind, json!({ "error": e }).to_string()),
                }
            }
            (true, ["analyse", tracker]) => {
                let from = form.get("url").cloned().unwrap_or_default();
                if from.trim().is_empty() {
                    (400, json_kind, json!({ "error": "paste an address, or choose a file" }).to_string())
                } else {
                    let id = self.analyse(tracker, from.trim().to_string(), form_title(&query));
                    (200, json_kind, json!({ "job": id }).to_string())
                }
            }
            (false, ["assist", tracker, source]) => (200, html_kind, self.assist_page(tracker, source, &query)),
            (true, ["teach", tracker, source]) => {
                let id = self.teach(tracker, source, query.get("url").cloned().unwrap_or_default(), form_title(&query));
                (200, json_kind, json!({ "job": id }).to_string())
            }
            (true, ["why", tracker, source]) => {
                let id = self.propose_why(tracker, source, form_title(&query));
                (200, json_kind, json!({ "job": id }).to_string())
            }
            (false, ["review", tracker, source]) => match self.review_page(tracker, source, &query) {
                Ok(p) => (200, html_kind, p),
                Err(e) => (404, html_kind, page("Not here", html! { p { (e) } })),
            },
            // Not right: the source goes, and the person tries another address.
            (true, ["discard", tracker, source]) => {
                let _ = std::fs::remove_dir_all(self.sources().join(source));
                return redirect(request, &serve::at(&format!("/new/{tracker}?title={}", urlencode(&form_title(&query)))));
            }
            (true, ["accept", tracker, source]) => {
                let why = form.get("why").cloned().unwrap_or_default();
                if let Some(name) = form.get("name").filter(|n| !n.trim().is_empty()) {
                    if let Err(e) = rename(&self.sources().join(source), name.trim()) {
                        return respond(request, 400, html_kind, &page("Not renamed", html! { div.note { (e) } }));
                    }
                }
                match self.accept(tracker, source, &why, &form_title(&query)) {
                    Ok(next) => {
                        self.sites.remove(*tracker);
                        return redirect(request, &next);
                    }
                    Err(e) => (400, html_kind, page("Not accepted", html! { div.note { (e) } })),
                }
            }
            (false, ["job", id]) => {
                let job = id.parse().ok().and_then(|id: u64| self.jobs.lock().ok()?.all.get(&id).cloned());
                match job {
                    Some(j) => (
                        200,
                        json_kind,
                        json!({ "lines": j.lines, "done": j.done, "error": j.error, "then": j.then }).to_string(),
                    ),
                    None => (404, json_kind, json!({ "error": "no such job" }).to_string()),
                }
            }
            _ => (404, html_kind, page("Not here", html! { h1 { "Nothing here" } p { a href=(serve::at("/")) { "Start" } } })),
        };
        respond(request, status, kind, &text)
    }

    /// A directory name nobody has taken yet.
    fn free(&self, under: &Path, want: &str) -> String {
        let base = if want.is_empty() { "tracker".to_string() } else { want.replace('_', "-") };
        let mut name = base.clone();
        let mut n = 2;
        while under.join(&name).exists() {
            name = format!("{base}-{n}");
            n += 1;
        }
        name
    }

    fn upload(&self, name: &str, body: &[u8]) -> Result<PathBuf, String> {
        let file = Path::new(name).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "upload.csv".into());
        let stem = Path::new(&file).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let dir = self.sources().join(self.free(&self.sources(), &crate::guess::slug(&stem)));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join(&file);
        std::fs::write(&path, body).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }

    /// Read a source from an address or a file, propose its declaration, and read it whole, so
    /// that what the person is shown next is the source as it is and not a guess from a sample.
    fn analyse(&self, tracker: &str, from: String, title: String) -> u64 {
        let sources = self.sources();
        let taken: BTreeSet<String> = std::fs::read_dir(&sources)
            .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        let tracker = tracker.to_string();
        self.start(move |p| {
            let local = Path::new(&from).is_file();
            let github = from.starts_with("github:");
            // An upload already sits in its own directory; an address gets one named after it.
            let dir = if local {
                Path::new(&from).parent().map(Path::to_path_buf).unwrap_or_else(|| sources.clone())
            } else {
                let stem = from.trim_end_matches('/').rsplit('/').next().unwrap_or("source");
                let stem = stem.split(['?', '.']).next().unwrap_or(stem);
                let base = crate::guess::slug(stem).replace('_', "-");
                let base = if base.is_empty() { "source".to_string() } else { base };
                let mut name = base.clone();
                let mut n = 2;
                while taken.contains(&name) {
                    name = format!("{base}-{n}");
                    n += 1;
                }
                sources.join(name)
            };
            let source = dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let name = format!("local/{source}");
            let shown = if local { Path::new(&from).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default() } else { from.clone() };
            p.say(format!("Reading {shown}"));
            let proposed = if github {
                crate::guess::propose_github(&from, &dir, Some(&name))
            } else if local {
                crate::guess::propose(Path::new(&from), &dir, Some(&name), None)
            } else {
                crate::guess::propose_url(&from, &dir, Some(&name), None)
            };
            // A JSON API has no shape to guess from a table. The assist can read it, once the
            // person has seen what that sends: the page that says so is where the job goes.
            if let Err(e) = &proposed {
                if e.contains("answers JSON") {
                    let _ = std::fs::remove_dir_all(&dir);
                    return Ok(serve::at(&format!("/assist/{tracker}/{source}?url={}&title={}", urlencode(&from), urlencode(&title))));
                }
            }
            if let Err(e) = proposed {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(e);
            }
            // The sample `propose_url` read to guess from; the source reads its address itself.
            let _ = std::fs::remove_file(dir.join("source.csv"));
            let ds = Source::open(&dir)?;
            describe_ids(&ds, p);
            p.say("Reading every claim");
            let started = std::time::Instant::now();
            let report = ds.run()?;
            p.say(format!(
                "{} claims in {:.1}s",
                report.added + report.changed + report.unchanged,
                started.elapsed().as_secs_f64()
            ));
            Ok(serve::at(&format!("/review/{tracker}/{source}?title={}", urlencode(&title))))
        })
    }

    /// The source joins the tracker. The first one makes it; a second one is a perspective, and
    /// what the two share is shown before and after.
    fn accept(&self, tracker: &str, source: &str, why: &str, title: &str) -> Result<String, String> {
        let dir = self.trackers().join(tracker);
        let ds = Source::open(&self.sources().join(source))?;
        let why = if why.trim().is_empty() { format!("What {} says.", ds.decl.title) } else { why.trim().to_string() };
        let member = json!({ "source": ds.decl.name, "why": why });
        if !dir.join(crate::trackerdecl::FILE).exists() {
            let scheme = primary_scheme(&ds).ok_or_else(|| {
                format!("{} carries no identifier, so no second source could ever meet it", ds.decl.title)
            })?;
            let decl = json!({
                "name": format!("local/{tracker}"),
                "title": if title.is_empty() { tracker.to_string() } else { title.to_string() },
                "sources": [member],
                "identified_by": [scheme],
            });
            write_tracker(&dir, decl)?;
            return Ok(serve::at(&format!("/review/{tracker}/{source}?added=1")));
        }
        let mut decl = serde_json::to_value(TrackerDecl::load(&dir)?).map_err(|e| e.to_string())?;
        if let Some(list) = decl["sources"].as_array_mut() {
            if !list.iter().any(|m| m["source"] == ds.decl.name.as_str()) {
                list.push(member);
            }
        }
        write_tracker(&dir, decl)?;
        Ok(serve::at(&format!("/review/{tracker}/{source}?added=1")))
    }

    /// What the assist would be sent to read a JSON API, and the button that sends it (D10).
    fn assist_page(&self, tracker: &str, source: &str, query: &BTreeMap<String, String>) -> String {
        let url = query.get("url").cloned().unwrap_or_default();
        let title = form_title(query);
        let assist = crate::assist::Assist::configured(&self.root);
        let body = html! {
            p { a href={(serve::at("/new/")) (tracker) "?title=" (urlencode(&title))} { "← another address" } }
            h1 { "This address answers JSON" }
            p.about { code { (url) } " is an API, and an API has no shape to guess the way a table has. The assist can read it and propose a declaration, which is then tried against the API before you see it." }
            @if assist.available() {
                div.note {
                    "This would send " strong { (assist.who()) } ":"
                    ul {
                        li { "the address" }
                        li { "an outline of its answer: the paths in it, each with one example value of at most 80 characters, from up to 20 items, and the same of its last page where it has one" }
                    }
                    "Nothing else, and only for this source. What was sent is written beside its declaration, in " code { (crate::assist::CONSENT) } "."
                }
                form #teach data-job={(serve::at("/teach/")) (tracker) "/" (source) "?url=" (urlencode(&url)) "&title=" (urlencode(&title))} {
                    button.primary type="submit" { "Send it and read the API" }
                }
            } @else {
                div.note { "There is no assist here: " (crate::assist::Assist::missing()) "." }
            }
            pre #log data-jobs=(serve::at("/job/")) hidden {}
            div #error .note hidden {}
            (PreEscaped(JOB_SCRIPT))
        };
        page("An API", body)
    }

    fn teach(&self, tracker: &str, source: &str, url: String, title: String) -> u64 {
        let root = self.root.clone();
        let dir = self.sources().join(source);
        let (tracker, source) = (tracker.to_string(), source.to_string());
        self.start(move |p| {
            let assist = crate::assist::Assist::configured(&root);
            p.say(format!("Asking {} to read {url}", assist.who()));
            let taught = match crate::teach::api(&assist, &url, &dir, &format!("local/{source}"), true) {
                Ok(crate::teach::Outcome::Done(t)) => t,
                Ok(crate::teach::Outcome::NeedsConsent(_)) => return Err("not agreed to".into()),
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&dir);
                    return Err(e);
                }
            };
            p.say(format!("Proposed{}, and tried: {}", if taught.mended { " and mended once" } else { "" }, taught.trial.lines().next().unwrap_or("")));
            let ds = Source::open(&dir)?;
            describe_ids(&ds, p);
            p.say("Reading every claim");
            let started = std::time::Instant::now();
            let report = ds.run()?;
            p.say(format!("{} claims in {:.1}s", report.added + report.changed + report.unchanged, started.elapsed().as_secs_f64()));
            Ok(serve::at(&format!("/review/{tracker}/{source}?title={}", urlencode(&title))))
        })
    }

    fn propose_why(&self, tracker: &str, source: &str, title: String) -> u64 {
        let root = self.root.clone();
        let (tdir, sdir) = (self.trackers().join(tracker), self.sources().join(source));
        let (tracker, source) = (tracker.to_string(), source.to_string());
        self.start(move |p| {
            let assist = crate::assist::Assist::configured(&root);
            p.say(format!("Asking {}", assist.who()));
            match crate::teach::why(&assist, &tdir, &sdir, true)? {
                crate::teach::Outcome::Done(why) => Ok(serve::at(&format!("/review/{tracker}/{source}?title={}&why={}", urlencode(&title), urlencode(&why)))),
                crate::teach::Outcome::NeedsConsent(_) => Err("not agreed to".into()),
            }
        })
    }

    fn start_page(&self) -> String {
        let trackers = listed(&self.trackers());
        let body = html! {
            @if trackers.is_empty() {
                h1.big { "What do you want to track?" }
                p.about { "A tracker reads several sources about the same things, shows where they agree, where they disagree and what changed. It starts with one source: an address or a file." }
                form.bar method="post" action=(serve::at("/new")) {
                    input.wide type="text" name="title" placeholder="Exploited vulnerabilities, papers on protein folding, our suppliers…" autofocus;
                    button.primary type="submit" { "Start" }
                }
                (example_card())
            } @else {
                h1 { "Your trackers" }
                table { tbody {
                    @for (name, decl, fresh) in &trackers {
                        tr {
                            td { a href={(serve::at("/t/")) (name) "/"} { strong { (decl.title) } } div.why { (decl.members.len()) " sources · identified by " (decl.join.join(", ")) } }
                            td.num { @if *fresh > 0 { span.chip.on { (fresh) " signals since yesterday" } } @else { span.dim { "nothing new since yesterday" } } }
                            td.num { a href={(serve::at("/new/")) (name) "?title=" (urlencode(&decl.title))} { "Add a source" } }
                        }
                    }
                } }
                h2 { "Another tracker" }
                form.bar method="post" action=(serve::at("/new")) {
                    input.wide type="text" name="title" placeholder="What do you want to track?";
                    button.primary type="submit" { "Start" }
                }
                (example_card())
            }
            p.dim { "The workspace is " code { (self.root.display()) } ". Everything here is a file in it." }
        };
        page("Zetlyn", body)
    }

    /// The first source, or another perspective on what the tracker already follows.
    fn source_page(&self, tracker: &str, query: &BTreeMap<String, String>) -> String {
        let title = form_title(query);
        let existing = TrackerDecl::load(&self.trackers().join(tracker)).ok();
        let first = existing.is_none();
        let prefill = query.get("url").cloned().unwrap_or_default();
        // The example's second step says where its second file is, so the whole of it is two pastes.
        let example_next = existing.as_ref().is_some_and(|d| d.title == EXAMPLE_TITLE && d.members.len() == 1);
        let prefill = if prefill.is_empty() && example_next { EXAMPLE[1].0.to_string() } else { prefill };
        let body = html! {
            p { a href=(serve::at("/")) { "← Zetlyn" } }
            h1 { (if title.is_empty() { tracker.to_string() } else { title.clone() }) }
            @if first {
                p.about { "The first source. Paste the address of a CSV file or a feed, or choose a file on this machine." }
            } @else {
                p.about { "Another perspective: a second source about the same things. Zetlyn reads it, then shows how many things the two share before anything is connected." }
            }
            form #analyse .bar data-job={(serve::at("/analyse/")) (tracker) "?title=" (urlencode(&title))} {
                input.wide type="text" name="url" value=(prefill) placeholder="https://…/something.csv, or github:owner/repo/releases" autofocus;
                button.primary type="submit" { "Read it" }
            }
            p.dim { "or " label.file { input #file type="file" data-action={(serve::at("/upload/")) (tracker) "?title=" (urlencode(&title))}; } }
            pre #log data-jobs=(serve::at("/job/")) hidden {}
            div #error .note hidden {}
            (PreEscaped(JOB_SCRIPT))
        };
        page(if first { "The first source" } else { "Another perspective" }, body)
    }

    /// What a source turned out to be, read whole: its identifiers with real values, its
    /// properties, its first claims. And once there is a tracker, how many things it shares.
    fn review_page(&self, tracker: &str, source: &str, query: &BTreeMap<String, String>) -> Result<String, String> {
        let ds = Source::open(&self.sources().join(source))?;
        let tracker_dir = self.trackers().join(tracker);
        let decl = TrackerDecl::load(&tracker_dir).ok();
        let joined = decl.as_ref().is_some_and(|d| d.members.iter().any(|m| m.dataset == ds.decl.name));
        let added = query.get("added").is_some();
        let title = decl.as_ref().map(|d| d.title.clone()).unwrap_or_else(|| form_title(query));
        let schemes = ds.store.schemes();
        let total = ds.store.count();
        let hits = first_claims(&ds, 5);
        let properties: Vec<(String, String)> = ds
            .decl
            .records
            .fields
            .iter()
            .map(|(n, p)| (n.clone(), p.kind.name().to_string()))
            .collect();

        // Against every source the tracker already has: what they would meet on, and how many.
        let mut matches: Vec<Match> = Vec::new();
        if let Some(d) = &decl {
            for m in d.members.iter().filter(|m| m.dataset != ds.decl.name) {
                if let Some(other) = crate::tracker::registry(&self.sources()).get(&m.dataset).and_then(|p| Source::open(p).ok()) {
                    for scheme in &d.join {
                        matches.push(measure(&ds, &other, scheme));
                    }
                }
            }
        }

        let body = html! {
            p { a href=(serve::at("/")) { "← Zetlyn" } @if decl.is_some() { " · " a href={(serve::at("/t/")) (tracker) "/"} { (title) } } }
            h1 { (ds.decl.title) }
            p.about { (total) " claims read from " code { (source_of(&ds)) } }

            h2 { "What names a claim" }
            @if schemes.is_empty() {
                div.note { "No identifier found. A second source could only meet this one on an identifier, so this one would stand alone." }
            } @else {
                table { tbody {
                    @for (scheme, n) in &schemes {
                        tr {
                            td { strong { (scheme) } @if let Some(s) = crate::schemes::named(scheme) { div.why { (s.title) ", e.g. " (s.example) } } }
                            td.num { (n) " claims" }
                            td { span.dim { (examples_of(&ds, scheme, 3).join(" · ")) } }
                        }
                    }
                } }
            }

            h2 { "Properties" }
            p { @for (n, k) in &properties { span.chip { (n) " " span.dim { (k) } } " " } }

            h2 { "The first claims" }
            table { tbody {
                @for h in &hits {
                    tr {
                        td { (h.title) div.why { @for id in &h.ids { (id.scheme) " " (id.value) "  " } } }
                        td.dim { (h.known) }
                    }
                }
            } }

            @for m in &matches {
                h2 { "Against " (m.other) }
                @if m.both == 0 {
                    div.note { "No " (m.scheme) " in common. " (m.mine) " here, " (m.theirs) " there: connecting these would add things, and none of them would have two perspectives." }
                } @else {
                    div.grid {
                        div.card { h4 { (m.both) } span.dim { "covered by both, via " (m.scheme) } }
                        div.card { h4 { (m.mine - m.both) } span.dim { "only here" } }
                        div.card { h4 { (m.theirs - m.both) } span.dim { "only in " (m.other) } }
                    }
                    p { "Of " (m.theirs) " in " (m.other) ", " strong { (m.both) " (" (format!("{:.0}%", 100.0 * m.both as f64 / m.theirs.max(1) as f64)) ")" } " are here too. Of " (m.mine) " here, " (format!("{:.0}%", 100.0 * m.both as f64 / m.mine.max(1) as f64)) " are there." }
                    @if joined {
                        p { "For example " @for (i, (scheme, value)) in m.examples.iter().enumerate() {
                            @if i > 0 { ", " }
                            a href={(serve::at("/t/")) (tracker) "/thing/" (urlencode(scheme)) "/" (urlencode(value))} { (value) }
                        } ", each with two perspectives." }
                    }
                }
            }

            @if joined {
                div.note { @if added { "Connected. " } (ds.decl.title) " is part of " a href={(serve::at("/t/")) (tracker) "/"} { (title) } "." }
                p.bar {
                    a.chip href={(serve::at("/t/")) (tracker) "/"} { "Open the tracker" }
                    a.chip href={(serve::at("/new/")) (tracker) "?title=" (urlencode(&title))} { "Add another perspective" }
                }
            } @else {
                @if decl.is_some() && crate::assist::Assist::configured(&self.root).available() {
                    form #why .bar data-job={(serve::at("/why/")) (tracker) "/" (source) "?title=" (urlencode(&title))} {
                        button type="submit" { "Let the assist propose why" }
                        span.dim { "sends " (crate::assist::Assist::configured(&self.root).who()) " the names and descriptions of these sources, nothing they hold" }
                    }
                    pre #log data-jobs=(serve::at("/job/")) hidden {}
                    div #error .note hidden {}
                    (PreEscaped(JOB_SCRIPT))
                }
                form method="post" action={(serve::at("/accept/")) (tracker) "/" (source) "?title=" (urlencode(&title))} {
                    p { label { "What to call it" br;
                        input.wide type="text" name="name" value=(example_name(&ds).unwrap_or(&ds.decl.title)); } }
                    p { label { "Why this source, in one sentence" br;
                        input.wide type="text" name="why" value=(query.get("why").cloned().unwrap_or_else(|| example_why(&ds))) placeholder={"What " (ds.decl.title) " says that nothing else does."}; } }
                    p.bar {
                        button.primary type="submit" { @if decl.is_none() { "Looks right" } @else { "Connect" } }
                        button type="submit" formaction={(serve::at("/discard/")) (tracker) "/" (source) "?title=" (urlencode(&title))} { "Not right" }
                    }
                }
                p.dim { "The declaration is " code { (self.sources().join(source).join(crate::sourcedecl::FILE).display()) } ", and can be edited." }
            }
        };
        Ok(page(&ds.decl.title, body))
    }
}

struct Match {
    other: String,
    scheme: String,
    mine: usize,
    theirs: usize,
    both: usize,
    examples: Vec<(String, String)>,
}

fn measure(mine: &Source, other: &Source, scheme: &str) -> Match {
    let a = mine.store.identifiers(scheme);
    let b = other.store.identifiers(scheme);
    let both: Vec<&String> = a.intersection(&b).collect();
    let examples = both
        .iter()
        .rev()
        .take(3)
        .filter_map(|k| k.split_once(':'))
        .map(|(s, v)| (s.to_string(), v.to_uppercase()))
        .collect();
    Match { other: other.decl.title.clone(), scheme: scheme.to_string(), mine: a.len(), theirs: b.len(), both: both.len(), examples }
}

/// What the tracker will be identified by: the source's best-known scheme, else its first.
fn primary_scheme(ds: &Source) -> Option<String> {
    let schemes = ds.store.schemes();
    schemes
        .iter()
        .find(|(s, _)| crate::schemes::named(s).is_some_and(|k| !k.classifies()))
        .or(schemes.first())
        .map(|(s, _)| s.clone())
}

fn describe_ids(ds: &Source, p: &Progress) {
    let ids: Vec<(String, String)> = ds
        .decl
        .records
        .id
        .as_ref()
        .map(|i| i.each().iter().map(|s| (s.scheme.clone().unwrap_or_default(), s.from.clone())).collect())
        .unwrap_or_default();
    if ids.is_empty() {
        p.say("No identifier found");
    }
    for (scheme, from) in ids {
        match crate::schemes::named(&scheme) {
            Some(s) => p.say(format!("Identifier: {} ({}) from {from}", s.title, scheme)),
            None => p.say(format!("Identifier: {scheme} from {from}")),
        }
    }
}

fn first_claims(ds: &Source, n: usize) -> Vec<crate::store::Hit> {
    let q = Query { text: String::new(), pred: None, view: None, ids: Vec::new(), seen_before: None, sort: None, limit: n, offset: 0 };
    crate::source::Interface::search(ds, &q).map(|(_, hits, _)| hits).unwrap_or_default()
}

fn examples_of(ds: &Source, scheme: &str, n: usize) -> Vec<String> {
    ds.store
        .identifiers(scheme)
        .into_iter()
        .rev()
        .take(n)
        .filter_map(|k| k.split_once(':').map(|(_, v)| v.to_uppercase()))
        .collect()
}

fn example_why(ds: &Source) -> String {
    EXAMPLE
        .iter()
        .find(|(url, _, _)| source_of(ds) == *url)
        .map(|(_, _, why)| why.to_string())
        .unwrap_or_default()
}

fn source_of(ds: &Source) -> String {
    serde_json::to_value(&ds.decl.source)
        .ok()
        .and_then(|j| j["path"].as_str().map(str::to_string))
        .unwrap_or_default()
}

fn write_tracker(dir: &Path, decl: J) -> Result<(), String> {
    let decl: TrackerDecl = serde_json::from_value(decl).map_err(|e| format!("the tracker does not make a declaration: {e}"))?;
    let text = crate::yaml::to_string(&decl)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(crate::trackerdecl::FILE);
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Every tracker in the workspace, with how many signals it has had since yesterday.
fn listed(dir: &Path) -> Vec<(String, TrackerDecl, usize)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    let since = crate::iso_stamp(crate::now() - 86_400);
    for e in entries.flatten() {
        let path = e.path();
        let Ok(decl) = TrackerDecl::load(&path) else { continue };
        let fresh = crate::thingstore::ThingStore::open(&path)
            .map(|s| s.signals(None, 5000).iter().filter(|s| s["at"].as_str().is_some_and(|a| a >= since.as_str())).count())
            .unwrap_or(0);
        out.push((e.file_name().to_string_lossy().into_owned(), decl, fresh));
    }
    out.sort_by(|a, b| a.1.title.cmp(&b.1.title));
    out
}

fn example_card() -> Markup {
    html! {
        div.card.example {
            h4 { "Or start from an example" }
            p.dim { "CISA's list of exploited vulnerabilities and Exploit-DB: two CSV files that both carry the CVE number. Two pastes, and you see which exploited vulnerabilities have public code." }
            form method="post" action=(serve::at("/example")) { button type="submit" { "Start from the CVE example" } }
        }
    }
}

fn form_title(q: &BTreeMap<String, String>) -> String {
    q.get("title").cloned().unwrap_or_default()
}

fn parse_form(s: &str) -> BTreeMap<String, String> {
    s.split('&')
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (serve::urldecode(k), serve::urldecode(v)))
        .collect()
}

fn page(title: &str, body: Markup) -> String {
    serve::shell(title, body)
}

fn respond(request: tiny_http::Request, status: u16, kind: &str, body: &str) {
    let mut response = tiny_http::Response::from_string(body).with_status_code(status);
    if let Ok(h) = tiny_http::Header::from_bytes(&b"Content-Type"[..], kind.as_bytes()) {
        response = response.with_header(h);
    }
    let _ = request.respond(response);
}

fn redirect(request: tiny_http::Request, to: &str) {
    let mut response = tiny_http::Response::from_string("").with_status_code(303);
    if let Ok(h) = tiny_http::Header::from_bytes(&b"Location"[..], to.as_bytes()) {
        response = response.with_header(h);
    }
    let _ = request.respond(response);
}

const APP_STYLE: &str = r#"
h1.big { font-size: 2rem; margin-top: 3rem; }
input.wide { flex: 1 1 26rem; min-width: 0; width: 100%; padding: .65rem .8rem; font: inherit;
  background: var(--panel); color: var(--fg); border: 1px solid var(--line); border-radius: 6px; }
button.primary { background: var(--accent); color: var(--bg); border-color: var(--accent); font-weight: 600; }
.example { margin-top: 2.5rem; max-width: 40rem; }
pre#log { background: var(--panel); border: 1px solid var(--line); border-radius: 6px;
  padding: .7rem .9rem; font-size: .85rem; white-space: pre-wrap; }
.grid .card h4 { font-size: 1.6rem; margin: 0; }
"#;

/// The one script: send a form or a file, then follow the job until it says where to go.
const JOB_SCRIPT: &str = r#"<script>
(function () {
  var log = document.getElementById('log'), error = document.getElementById('error');
  function follow(id) {
    fetch(((log && log.dataset.jobs) || '/job/') + id).then(function (r) { return r.json(); }).then(function (j) {
      log.hidden = false;
      log.textContent = j.lines.join('\n') + (j.done ? '' : '\n…');
      if (!j.done) { setTimeout(function () { follow(id); }, 400); return; }
      if (j.error) { error.hidden = false; error.textContent = j.error; return; }
      location.href = j.then;
    });
  }
  function started(r) {
    return r.json().then(function (j) {
      if (j.error) { error.hidden = false; error.textContent = j.error; return; }
      error.hidden = true; follow(j.job);
    });
  }
  document.querySelectorAll('form[data-job]').forEach(function (form) {
    form.addEventListener('submit', function (e) {
      e.preventDefault();
      fetch(form.dataset.job, { method: 'POST', body: new URLSearchParams(new FormData(form)) }).then(started);
    });
  });
  var file = document.getElementById('file');
  if (file) file.addEventListener('change', function () {
    var f = file.files[0]; if (!f) return;
    fetch(file.dataset.action + '&name=' + encodeURIComponent(f.name), { method: 'POST', body: f }).then(started);
  });
})();
</script>"#;

/// The name a person gives a source, written into its declaration: a file called
/// `files_exploits.csv` is Exploit-DB to whoever pasted it.
fn rename(dir: &Path, title: &str) -> Result<(), String> {
    let mut decl = crate::sourcedecl::SourceDecl::load(dir)?;
    if decl.title == title {
        return Ok(());
    }
    decl.title = title.to_string();
    let path = dir.join(crate::sourcedecl::FILE);
    std::fs::write(&path, crate::yaml::to_string(&decl)?).map_err(|e| format!("{}: {e}", path.display()))
}

fn example_name(ds: &Source) -> Option<&'static str> {
    EXAMPLE.iter().find(|(url, _, _)| source_of(ds) == *url).map(|(_, name, _)| *name)
}

/// A workspace hosted for somebody: whose it is, and where its plan is kept.
struct Hosted {
    owner: String,
    accounts: crate::account::Accounts,
}

/// `zetlyn host <workspace> --name <name> --billing <dir> [--addr 127.0.0.1:2300]`: one person's
/// workspace, served under `/<name>`, updated on its plan's terms. The same program as on their
/// own machine; what is added is that the owner signs in, visitors read, and the plan decides how
/// much runs.
pub fn host(args: &[String]) -> Result<(), String> {
    let root = crate::positional(args, 1).first().map(|s| PathBuf::from(s.as_str())).ok_or("which workspace?")?;
    let name = crate::flag(args, "--name").ok_or("--name, the workspace's address")?.to_string();
    let billing = PathBuf::from(crate::flag(args, "--billing").ok_or("--billing, where plans.yaml and customers.db are")?);
    let addr = crate::flag(args, "--addr").unwrap_or("127.0.0.1:2300").to_string();
    let owner = crate::billing::Book::read(&billing)?.get(&name).map(|c| c.email).ok_or_else(|| format!("{name}: no customer by that name"))?;
    let base = format!("/{}", name.trim_matches('/'));
    serve::mount(&base);
    let server = tiny_http::Server::http(&addr).map_err(|e| e.to_string())?;
    println!("{} for {owner} on http://{addr}{base}/", root.display());

    // The plan's pass, on its own thread: sources, trackers, watches, as `zetlyn run` does them,
    // within what the plan allows, and not at all while it is not paid for.
    {
        let (root, billing, name) = (root.clone(), billing.clone(), name.clone());
        std::thread::spawn(move || loop {
            let (paid, limits, _mails) = crate::billing::limits(&billing, &name);
            let wait = if paid {
                let soonest = crate::schedule_pass(&root, true, &limits);
                soonest.map(|s| (s - crate::now()).clamp(60, 3600)).unwrap_or(3600)
            } else {
                eprintln!("{name}: not paid for, so nothing is updated");
                600
            };
            std::thread::sleep(std::time::Duration::from_secs(wait as u64));
        });
    }

    let accounts = crate::account::Accounts::open(&root)?;
    let mut app = App {
        root,
        addr,
        sites: BTreeMap::new(),
        jobs: Arc::new(Mutex::new(Jobs::default())),
        base,
        hosted: Some(Hosted { owner, accounts }),
        visitor: false,
    };
    for request in server.incoming_requests() {
        serve::mount(&app.base);
        app.answer(request);
    }
    Ok(())
}
