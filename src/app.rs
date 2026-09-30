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
/// Beside a tracker that has a name and no source yet.
const DRAFT: &str = "draft.yaml";

pub fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("zetlyn app [WORKSPACE] [--port N] [--no-open]\n\nThe app in the browser, on 127.0.0.1:4747 or the next free port.\nWORKSPACE is the folder it keeps everything in: the current one if it is a workspace, else ~/zetlyn.");
        return Ok(());
    }
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

/// The current directory if it is a workspace, and `~/zetlyn` otherwise, made on first use.
fn workspace(args: &[String]) -> Result<PathBuf, String> {
    if let Some(named) = crate::positional(args, 1).first() {
        return Ok(PathBuf::from(named.as_str()));
    }
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    if here.join("workspace.yaml").exists() || here.join("trackers").is_dir() {
        return Ok(here);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or("no HOME to put ~/zetlyn in")?;
    let root = home.join("zetlyn");
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
                // The topic is kept from the moment it is named, so a first source that comes to
                // nothing does not take the topic with it.
                let dir = self.trackers().join(&slug);
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(dir.join(DRAFT), format!("# A topic with no source yet.\ntitle: {}\n", serde_json::to_string(&title).unwrap_or_default()));
                return redirect(request, &serve::at(&format!("/new/{slug}?title={}", urlencode(&title))));
            }
            (true, ["example"]) => {
                let slug = self.free(&self.trackers(), &crate::guess::slug(EXAMPLE_TITLE));
                return redirect(
                    request,
                    &format!("/new/{slug}?title={}&url={}", urlencode(EXAMPLE_TITLE), urlencode(EXAMPLE[0].0)),
                );
            }
            // Only a topic with no source, and only a source no tracker names.
            (true, ["forget", slug]) => {
                let dir = self.trackers().join(slug);
                if dir.join(DRAFT).exists() && !dir.join(crate::trackerdecl::FILE).exists() {
                    let _ = std::fs::remove_dir_all(&dir);
                }
                return redirect(request, &serve::at("/"));
            }
            (true, ["drop", dir]) => {
                let named: BTreeSet<String> = listed(&self.trackers()).into_iter().flat_map(|(_, d, _)| d.members.into_iter().map(|m| m.dataset)).collect();
                let path = self.sources().join(dir);
                if let Ok(ds) = Source::open(&path) {
                    if !named.contains(&ds.decl.name) {
                        let _ = std::fs::remove_dir_all(&path);
                    }
                }
                return redirect(request, &serve::at("/"));
            }
            (false, ["webpage", tracker]) => (200, html_kind, self.webpage_page(tracker, &query)),
            (true, ["readweb", tracker]) => {
                let pick: usize = query.get("pick").and_then(|p| p.parse().ok()).unwrap_or(0);
                let id = self.read_web(tracker, query.get("url").cloned().unwrap_or_default(), form_title(&query), pick);
                (200, json_kind, json!({ "job": id }).to_string())
            }
            // A property chosen as the identifier, the declaration changed to say so, the source
            // read again from the start, and back to what it is now.
            (true, ["identify", tracker, source]) => {
                let dir = self.sources().join(source);
                let field = form.get("field").cloned().unwrap_or_default();
                let scheme = crate::guess::slug(form.get("scheme").map(String::as_str).unwrap_or("")).replace('_', "-");
                let done = (|| -> Result<(), String> {
                    if scheme.is_empty() {
                        return Err("a name for the identifier".into());
                    }
                    let decl = crate::sourcedecl::SourceDecl::load(&dir)?;
                    let mut j = serde_json::to_value(&decl).map_err(|e| e.to_string())?;
                    let from = j["claims"]["properties"][&field]["from"].as_str().ok_or_else(|| format!("{field}: no such property"))?.to_string();
                    j["claims"]["id"] = json!({ "scheme": scheme, "from": from });
                    let decl: crate::sourcedecl::SourceDecl = serde_json::from_value(j).map_err(|e| e.to_string())?;
                    std::fs::write(dir.join(crate::sourcedecl::FILE), crate::yaml::to_string(&decl)?).map_err(|e| e.to_string())?;
                    Source::open(&dir)?.run_with(true, true).map(|_| ())
                })();
                match done {
                    Ok(()) => return redirect(request, &serve::at(&format!("/review/{tracker}/{source}?title={}", urlencode(&form_title(&query))))),
                    Err(e) => (400, html_kind, page("Not changed", html! { div.note { (e) } })),
                }
            }
            (false, ["assist"]) => (200, html_kind, self.assist_status_page(None)),
            (true, ["assist"]) => {
                let said = self.keep_assist(&form).unwrap_or_else(|e| e);
                (200, html_kind, self.assist_status_page(Some(&said)))
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
            (false, ["publish", tracker]) => (200, html_kind, self.publish_page(tracker, None)),
            (true, ["publish", tracker]) => {
                let said = match form.get("visibility").map(String::as_str) {
                    Some(v @ ("public" | "private")) => self.set_visibility(tracker, v),
                    _ => Err("public or private".into()),
                };
                self.sites.remove(*tracker);
                let said = said.unwrap_or_else(|e| e);
                (200, html_kind, self.publish_page(tracker, Some(&said)))
            }
            // On this machine: the sources and the tracker onto a hub, by the commands a terminal
            // would run, so the page does nothing the command line does not.
            (true, ["publish-hub", tracker]) => {
                let id = self.publish_to_hub(tracker);
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
                if let Some(feeds) = e.strip_prefix(crate::guess::WEB_PAGE) {
                    let _ = std::fs::remove_dir_all(&dir);
                    return Ok(serve::at(&format!("/webpage/{tracker}?url={}&title={}&feeds={}", urlencode(&from), urlencode(&title), urlencode(feeds))));
                }
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
            let _ = std::fs::remove_file(dir.join(DRAFT));
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
                div.note { "No model is set up to read it. " a href=(serve::at("/assist")) { "Set one up" } ": Claude with a key, or a model of your own. Then come back to this address." }
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

    /// Who may see a tracker, and what its sources say about being shown.
    fn publish_page(&self, tracker: &str, said: Option<&str>) -> String {
        let dir = self.trackers().join(tracker);
        let opened = Tracker::open(&dir, &self.sources());
        let body = match &opened {
            Err(e) => html! { div.note { (e) } },
            Ok(t) => {
                let blocked = t.not_public();
                let public = !t.private();
                let where_ = if self.hosted.is_some() {
                    format!("{}{}", crate::account::Site::load(&self.root).url.trim_end_matches('/'), serve::at(&format!("/t/{tracker}/")))
                } else {
                    String::new()
                };
                html! {
                    p { a href=(serve::at("/")) { "← Zetlyn" } " · " a href=(serve::at(&format!("/t/{tracker}/"))) { (t.decl.title) } }
                    h1 { "Who may see " (t.decl.title) }
                    @if let Some(s) = said { div.note { (s) } }
                    p.about {
                        @if public { "Public: its overview and its things are open to anyone, its claims to subscribers." }
                        @else { "Private: every page is for the accounts it gives access to." }
                        @if !where_.is_empty() { " It is at " a href=(where_) { (where_) } "." }
                    }
                    h2 { "What each source says about being shown" }
                    table { tbody {
                        @for (source, republish) in t.licences() {
                            tr {
                                td { (source) }
                                td { @match republish.as_str() {
                                    "yes" => span.chip { "in full" },
                                    "summary" => span.chip { "titles, values and a link" },
                                    "no" => span.chip.on { "not in public" },
                                    _ => span.chip.on { "has not said" },
                                } }
                            }
                        }
                    } }
                    @if !blocked.is_empty() {
                        p.dim { "Public is refused while " (blocked.join("; ")) ". A source says so in its declaration: "
                            code { "licence: { republish: yes | summary | no, terms: <url> }" } "." }
                    }
                    form.bar method="post" action=(serve::at(&format!("/publish/{tracker}"))) {
                        @if public {
                            input type="hidden" name="visibility" value="private";
                            button type="submit" { "Make it private" }
                        } @else {
                            input type="hidden" name="visibility" value="public";
                            button.primary type="submit" disabled[!blocked.is_empty()] { "Make it public" }
                        }
                    }
                    @if self.hosted.is_none() {
                        h2 { "On a hub" }
                        p.dim { "Publishing puts its sources and its statement on hub.zetlyn.com, signed with this machine's key (" code { "zetlyn id" } "), where anybody can subscribe to them." }
                        form #publish data-job=(serve::at(&format!("/publish-hub/{tracker}"))) {
                            button type="submit" disabled[public && !blocked.is_empty()] { "Publish to hub.zetlyn.com" }
                        }
                        pre #log data-jobs=(serve::at("/job/")) hidden {}
                        div #error .note hidden {}
                        (PreEscaped(JOB_SCRIPT))
                    }
                }
            }
        };
        page("Publish", body)
    }

    /// `visibility:` in the tracker's file, as text, so its comments stay.
    fn set_visibility(&self, tracker: &str, visibility: &str) -> Result<String, String> {
        let dir = self.trackers().join(tracker);
        if visibility == "public" {
            let t = Tracker::open(&dir, &self.sources())?;
            let blocked = t.not_public();
            if !blocked.is_empty() {
                return Err(format!("Not made public: {}.", blocked.join("; ")));
            }
        }
        let path = dir.join(crate::trackerdecl::FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let mut out: Vec<String> = text.lines().filter(|l| !l.starts_with("visibility:")).map(str::to_string).collect();
        if visibility == "private" {
            out.push("visibility: private".into());
        }
        let text = format!("{}\n", out.join("\n"));
        let _: TrackerDecl = crate::yaml::parse(&text)?;
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        Ok(format!("It is {visibility} now."))
    }

    fn publish_to_hub(&self, tracker: &str) -> u64 {
        let (dir, sources) = (self.trackers().join(tracker), self.sources());
        self.start(move |p| {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let t = Tracker::open(&dir, &sources)?;
            let registry = crate::tracker::registry(&sources);
            let run = |args: &[&str]| -> Result<String, String> {
                let out = std::process::Command::new(&exe).args(args).output().map_err(|e| e.to_string())?;
                let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
                if out.status.success() { Ok(said) } else { Err(said.trim().to_string()) }
            };
            for m in &t.decl.members {
                let Some(sdir) = registry.get(&m.dataset) else { continue };
                p.say(format!("zetlyn source publish {}", sdir.display()));
                p.say(run(&["source", "publish", &sdir.display().to_string()])?.trim().to_string());
            }
            p.say(format!("zetlyn tracker publish {}", dir.display()));
            p.say(run(&["tracker", "publish", &dir.display().to_string()])?.trim().to_string());
            Ok(serve::at(&format!("/publish/{}", dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())))
        })
    }

    /// What was begun and not finished: a topic with no source yet, and sources read that no
    /// tracker names. Said on the start page, so nothing a person began disappears.
    fn unfinished(&self) -> Markup {
        let drafts: Vec<(String, String)> = std::fs::read_dir(self.trackers())
            .map(|d| d.flatten().map(|e| e.path()).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.join(DRAFT).exists() && !p.join(crate::trackerdecl::FILE).exists())
            .map(|p| (p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), draft_title(&p).unwrap_or_default()))
            .collect();
        let named: BTreeSet<String> = listed(&self.trackers()).into_iter().flat_map(|(_, d, _)| d.members.into_iter().map(|m| m.dataset)).collect();
        let orphans: Vec<(String, String, u64)> = crate::tracker::registry(&self.sources())
            .into_iter()
            .filter(|(name, _)| !named.contains(name))
            .filter_map(|(_, dir)| {
                let ds = Source::open(&dir).ok()?;
                Some((dir.file_name()?.to_string_lossy().into_owned(), ds.decl.title.clone(), ds.store.count()))
            })
            .collect();
        html! {
            @if !drafts.is_empty() || !orphans.is_empty() { h2 { "Begun, not finished" } }
            @if !drafts.is_empty() {
                table { tbody { @for (slug, title) in &drafts {
                    tr {
                        td { strong { (title) } div.why { "a topic with no source yet" } }
                        td.num { a href={(serve::at("/new/")) (slug) "?title=" (urlencode(title))} { "Continue" } }
                        td.num { form method="post" action={(serve::at("/forget/")) (slug)} { button type="submit" { "Forget it" } } }
                    }
                } } }
            }
            @if !orphans.is_empty() {
                table { tbody { @for (dir, title, n) in &orphans {
                    tr {
                        td { strong { (title) } div.why { (n) " claims · a source no tracker names · " code { "sources/" (dir) } } }
                        td.num { form method="post" action={(serve::at("/drop/")) (dir)} { button type="submit" { "Delete it" } } }
                    }
                } } }
            }
        }
    }

    /// An address that is a page people read, not data: what Zetlyn reads instead, the feeds the
    /// page names itself, and where the data behind a page usually is.
    fn webpage_page(&self, tracker: &str, query: &BTreeMap<String, String>) -> String {
        let url = query.get("url").cloned().unwrap_or_default();
        let title = form_title(query);
        let feeds: Vec<(String, String)> = query.get("feeds").and_then(|f| serde_json::from_str(f).ok()).unwrap_or_default();
        let body = html! {
            p { a href={(serve::at("/new/")) (tracker) "?title=" (urlencode(&title))} { "← another address" } }
            h1 { "That is a web page" }
            p.about { code { (url) } " is a page for people to read in a browser. Zetlyn can read a list on it: every item one claim, what is in it the values." }
            @let found = crate::fetch::Fetcher::new(crate::sourcedecl::AGENT, &BTreeMap::new(), 0).and_then(|f| f.get(&url)).map(|b| crate::web::candidates(&b)).unwrap_or_default();
            @if found.is_empty() {
                p { "No list was found on it. The address of a list works best: search results, a category, a page of new releases." }
            } @else {
                h2 { "Lists on this page" }
                @for (i, c) in found.iter().enumerate().take(3) {
                    div.card style="margin-bottom:1rem" {
                        h4 { (c.count) " items " span.dim { code { (c.items) } } }
                        p.dim { @for (n, _, said) in first_said(&c.fields) { strong { (n) } " " (said.first().cloned().unwrap_or_default().chars().take(40).collect::<String>()) " · " } }
                        form data-job={(serve::at("/readweb/")) (tracker) "?title=" (urlencode(&title)) "&pick=" (i) "&url=" (urlencode(&url))} {
                            button.primary[i == 0] type="submit" { "Read this list" }
                        }
                    }
                }
                pre #log data-jobs=(serve::at("/job/")) hidden {}
                div #error .note hidden {}
                (PreEscaped(JOB_SCRIPT))
                p.dim { "A list sorted by date is read back to the first of January, and after that only what is new. Reading many pages takes minutes; the page follows it." }
            }
            @if feeds.is_empty() {
                p { "This page names no feed of its own." }
            } @else {
                h2 { "Feeds this page names" }
                @for (f, name) in &feeds {
                    form.bar data-job={(serve::at("/analyse/")) (tracker) "?title=" (urlencode(&title))} {
                        input type="hidden" name="url" value=(f);
                        button.primary type="submit" { "Read " (name) }
                        span.dim { code { (f) } }
                    }
                }
                pre #log data-jobs=(serve::at("/job/")) hidden {}
                div #error .note hidden {}
                (PreEscaped(JOB_SCRIPT))
            }
            h2 { "Where the data usually is" }
            ul {
                li { "A link on the page that says " em { "RSS" } ", " em { "Atom" } ", " em { "export" } ", " em { "download" } " or " em { "CSV" } "." }
                li { "An API: search for the site's name and " em { "API" } ". An address that answers JSON can be read with the assist (" a href=(serve::at("/assist")) { "is one set up?" } ")." }
                li { "Somebody who already publishes the same data as a table or an API. For games on Steam, SteamSpy answers JSON by tag: " code { "https://steamspy.com/api.php?request=tag&tag=Indie" } "." }
                li { "A file you have: upload it on the page before." }
            }
        };
        page("A web page", body)
    }

    /// Whether a model is asked, which, for what, and how to set one up. Tables and feeds need
    /// none; this is where a person finds that out rather than guessing.
    fn assist_status_page(&self, said: Option<&str>) -> String {
        let a = crate::assist::Assist::configured(&self.root);
        let body = html! {
            p { a href=(serve::at("/")) { "← Zetlyn" } }
            h1 { "The assist" }
            @if let Some(s) = said { div.note { (s) } }
            @if a.available() {
                p.state.current { "Set up: it asks " strong { (a.who()) } "." }
            } @else {
                p.state.empty { "Not set up. Nothing is sent to any model." }
            }
            p.about { "Zetlyn reads tables, feeds, folders and files without a model. A model helps where a pattern cannot: reading a JSON API it has not seen, turning a question in words into filters, saying which words of two sources mean the same thing, proposing why a source is in a tracker. It proposes; what it proposes is tried against the source before you see it, and you decide." }
            p.dim { "Before anything is sent, the page says to whom and what, once per source, and what was sent is written in that source's " code { "assist.yaml" } "." }
            h2 { "Claude" }
            form.bar method="post" action=(serve::at("/assist")) {
                input type="hidden" name="provider" value="anthropic";
                input.wide type="password" name="key" placeholder="An Anthropic API key, sk-ant-…" autocomplete="off";
                button.primary type="submit" { "Keep the key" }
            }
            p.dim { "Kept in " code { "~/.zetlyn/assist/anthropic.key" } ", readable by you alone. " code { "ANTHROPIC_API_KEY" } " works as well." }
            h2 { "A model of your own" }
            form method="post" action=(serve::at("/assist")) {
                input type="hidden" name="provider" value="openai";
                p { input.wide type="url" name="url" placeholder="http://127.0.0.1:11434/v1" ; }
                p.bar { input.wide type="text" name="model" placeholder="a model it serves: gemma3, llama3.1, …";
                    button type="submit" { "Use it" } }
            }
            p.dim { "Anything that speaks the OpenAI chat API: Ollama, llama.cpp, vLLM, or a hosted one. Written into " code { "workspace.yaml" } " as " code { "assist: { provider, url, model }" } ". A large model needs a machine with the memory for it." }
            @if a.available() {
                form method="post" action=(serve::at("/assist")) {
                    input type="hidden" name="provider" value="off";
                    button type="submit" { "Turn it off" }
                }
            }
        };
        page("The assist", body)
    }

    /// The choice made on the assist page, kept where the program reads it.
    fn keep_assist(&self, form: &BTreeMap<String, String>) -> Result<String, String> {
        let path = self.root.join("workspace.yaml");
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        // The workspace's own `assist:` block, replaced as text so the rest of the file stays as
        // it was written.
        let without: Vec<&str> = {
            let mut out = Vec::new();
            let mut skipping = false;
            for l in text.lines() {
                if l.starts_with("assist:") {
                    skipping = true;
                    continue;
                }
                if skipping && (l.starts_with(' ') || l.is_empty()) {
                    continue;
                }
                skipping = false;
                out.push(l);
            }
            out
        };
        let write = |block: &str| -> Result<(), String> {
            let t = format!("{}\n{block}", without.join("\n").trim_end());
            let _: crate::account::Site = crate::yaml::parse(&t)?;
            std::fs::write(&path, format!("{}\n", t.trim_end())).map_err(|e| e.to_string())
        };
        match form.get("provider").map(String::as_str) {
            Some("anthropic") => {
                crate::assist::keep_key("anthropic", form.get("key").map(String::as_str).unwrap_or(""))?;
                write("assist:\n  provider: anthropic\n")?;
                Ok("The key is kept. The assist asks Claude from now on.".into())
            }
            Some("openai") => {
                let url = form.get("url").map(|s| s.trim()).unwrap_or("");
                let model = form.get("model").map(|s| s.trim()).unwrap_or("");
                if !url.starts_with("http") || model.is_empty() {
                    return Err("an address, http… ending in /v1, and the name of a model it serves".into());
                }
                write(&format!("assist:\n  provider: openai\n  url: {}\n  model: {}\n", serde_json::to_string(url).unwrap_or_default(), serde_json::to_string(model).unwrap_or_default()))?;
                Ok(format!("The assist asks {model} at {url} from now on."))
            }
            Some("off") => {
                write("assist:\n  off: true\n")?;
                Ok("Off. Nothing is sent to any model.".into())
            }
            _ => Err("claude, a model of your own, or off".into()),
        }
    }

    /// A list on a web page, proposed as a source and read whole.
    fn read_web(&self, tracker: &str, url: String, title: String, pick: usize) -> u64 {
        let host = url.split('/').nth(2).unwrap_or("site").trim_start_matches("www.").to_string();
        let source = self.free(&self.sources(), &crate::guess::slug(host.split('.').next().unwrap_or("site")));
        let dir = self.sources().join(&source);
        let tracker = tracker.to_string();
        self.start(move |p| {
            p.say(format!("Reading {url}"));
            let body = crate::fetch::Fetcher::new(crate::sourcedecl::AGENT, &BTreeMap::new(), 0)?.get(&url)?;
            if let Err(e) = crate::guess::propose_web(&url, &body, &dir, Some(&format!("local/{source}")), pick) {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(e);
            }
            let ds = Source::open(&dir)?;
            describe_ids(&ds, p);
            p.say("Reading every page of the list");
            let pages = p.clone();
            crate::web::on_page(Some(Box::new(move |line| pages.say(line))));
            let started = std::time::Instant::now();
            let report = ds.run()?;
            p.say(format!("{} claims in {:.1}s", report.added + report.changed + report.unchanged, started.elapsed().as_secs_f64()));
            Ok(serve::at(&format!("/review/{tracker}/{source}?title={}", urlencode(&title))))
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
                            td.num { a href={(serve::at("/publish/")) (name)} { (if decl.visibility == "private" { "Private" } else { "Publish" }) } }
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
            (self.unfinished())
            p.dim { "The workspace is " code { (self.root.display()) } ". Everything here is a file in it. " a href=(serve::at("/assist")) { @if crate::assist::Assist::configured(&self.root).available() { "The assist asks " (crate::assist::Assist::configured(&self.root).who()) } @else { "No model is set up, and none is needed to begin" } } "." }
        };
        page("Zetlyn", body)
    }

    /// The first source, or another perspective on what the tracker already follows.
    fn source_page(&self, tracker: &str, query: &BTreeMap<String, String>) -> String {
        let title = form_title(query);
        let title = if title.is_empty() { draft_title(&self.trackers().join(tracker)).unwrap_or_default() } else { title };
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
                div.note { "No identifier Zetlyn knows was found. A tracker joins its sources on one, so a source needs something that names each of its claims." }
                @let unique = ds.store.unique_fields();
                @if unique.is_empty() {
                    p.dim { @if total == 0 { "It read no claims at all, so there is nothing to choose from: the address may not be the data." } @else { "No property here is different on every claim either." } }
                } @else {
                    form.bar method="post" action={(serve::at("/identify/")) (tracker) "/" (source) "?title=" (urlencode(&title))} {
                        span { "Use" }
                        select name="field" { @for f in &unique { option value=(f) { (f) } } }
                        span { "as its identifier, called" }
                        input type="text" name="scheme" placeholder="e.g. steam, isbn, sku" required;
                        button.primary type="submit" { "Read it again" }
                    }
                    p.dim { "These properties have a different value on every claim. Choose the one that names an item the way another source would name it too: a second source meets this one on it." }
                }
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
                        @if total > 0 { button.primary type="submit" { @if decl.is_none() { "Looks right" } @else { "Connect" } } }
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
    let j = serde_json::to_value(&ds.decl.source).unwrap_or_default();
    // A table or a file names a path, a feed its addresses, an API its list.
    j["path"].as_str().map(str::to_string)
        .or_else(|| j["urls"].as_array().map(|u| u.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")))
        .or_else(|| j["list"].as_str().map(str::to_string))
        .unwrap_or_else(|| ds.decl.source.address())
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

/// The name a topic was given before it had a source.
fn draft_title(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(DRAFT)).ok()?;
    let line = text.lines().find_map(|l| l.strip_prefix("title: "))?;
    serde_json::from_str::<String>(line).ok().or_else(|| Some(line.to_string()))
}

/// The fields a person recognises a list by come first: its title or name, its date, then the rest.
fn first_said<A, B: Clone + Default>(fields: &[(String, A, Vec<B>)]) -> Vec<&(String, A, Vec<B>)> {
    let rank = |n: &str| ["title", "name", "date", "released", "href"].iter().position(|w| n.contains(w)).unwrap_or(9);
    let mut them: Vec<_> = fields.iter().collect();
    them.sort_by_key(|f| rank(&f.0));
    them.truncate(6);
    them
}
