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

/// Two bookshops that sell mostly the same books and keep their lists their own way: Leafline
/// Books publishes a CSV, Bücherstube Lindenhof only has its shop's page. The example the app
/// and the docs begin with (see `examples.rs`); Lindenhof's page changes every two minutes.
const EXAMPLE: [(&str, &str, &str); 2] = [
    (
        "https://hub.zetlyn.com/examples/leafline-books.csv",
        "Leafline Books",
        "What Leafline Books charges for a book, and whether it has it.",
    ),
    (
        "https://hub.zetlyn.com/examples/lindenhof/",
        "Bücherstube Lindenhof",
        "What Bücherstube Lindenhof charges for a book, and whether it has it.",
    ),
];
const EXAMPLE_TITLE: &str = "Two bookshops";
/// Beside a tracker that has a name and no source yet.
const DRAFT: &str = "draft.yaml";

pub fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("zetlyn app [WORKSPACE] [--port N] [--no-open]\n\nThe app in the browser, on 127.0.0.1:4747 or the next free port.\nWORKSPACE is the folder it keeps everything in, made a workspace if it is not one yet (`zetlyn .`). Without it: the workspace you are in, else ~/zetlyn.");
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
    let mut app = App { root, addr, sites: BTreeMap::new(), jobs: Arc::new(Mutex::new(Jobs::default())), base: String::new(), hosted: None, visitor: false, who: None, orgs_of_who: Vec::new(), public_of_machine: Vec::new() };
    {
        let (root, jobs) = (app.root.clone(), app.jobs.clone());
        std::thread::spawn(move || background(&root, &jobs));
    }
    for request in server.incoming_requests() {
        app.answer(request);
    }
    Ok(())
}

/// The current directory if it is a workspace, and `~/zetlyn` otherwise, made on first use.

/// The background pass, for a workspace that turned it on: once a minute it looks for sources
/// that are due and updates them as one job, which the bar at the bottom of every page shows and
/// can stop. Never beside a job a person started: theirs comes first, and this waits a minute.
fn background(root: &Path, jobs: &Arc<Mutex<Jobs>>) {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
        if crate::autoupdate::every(root).is_none() {
            continue;
        }
        let busy = jobs.lock().map(|j| j.all.values().any(|job| !job.done)).unwrap_or(true);
        if busy {
            continue;
        }
        // Every watch says what is new since it last spoke: to its feed, by mail, to a webhook.
        // Each pass, not only after an update here, so an Update now is told about too.
        deliver_watches(root);
        let due = crate::autoupdate::due(root, crate::now());
        if due.is_empty() {
            continue;
        }
        let id = {
            let Ok(mut j) = jobs.lock() else { continue };
            j.next += 1;
            let id = j.next;
            j.all.insert(id, Job::default());
            id
        };
        let p = Progress { jobs: jobs.clone(), id };
        crate::autoupdate::RUNNING.store(true, std::sync::atomic::Ordering::SeqCst);
        let n = due.len();
        p.label(format!("Checking {n} source{}", if n == 1 { "" } else { "s" }));
        for (i, (_, title, dir)) in due.iter().enumerate() {
            if p.stopping() {
                p.say("Stopped: the rest wait for the next pass.");
                break;
            }
            p.show(Some(i as f64 / n as f64), format!("{title} ({} of {n})", i + 1));
            let Ok(ds) = Source::open(dir) else { continue };
            let hook = p.clone();
            crate::web::on_page(Some(Box::new(move |_| !hook.stopping())));
            let outcome = ds.run();
            crate::web::on_page(None);
            crate::autoupdate::record(&ds, &outcome);
            p.say(match &outcome {
                Ok(r) if r.added + r.changed + r.removed == 0 => format!("{title}: nothing changed"),
                Ok(r) => format!("{title}: {} new, {} changed, {} gone", r.added, r.changed, r.removed),
                Err(e) => format!("{title}: {e}"),
            });
        }
        // What changed becomes signals now, so the Changes tab and the counts say it.
        for dir in crate::tracker::scope_registry(&root.join("trackers")).values() {
            let _ = Tracker::open(dir, &root.join("sources")).and_then(|t| t.refresh_if_moved());
        }
        deliver_watches(root);
        crate::autoupdate::RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
        p.finish(Ok(String::new()));
    }
}



/// What a tracker compares, once a source joins it. A number, a yes or no, or a date is compared
/// where the sources say the same thing, and every source's field for it is written out under
/// `from:`, so tracker.yaml says which column of each source a property is read from. Words are
/// left to a person, who knows which mean one thing (the assist can propose them).
///
/// Three steps. Every comparison already there is made explicit for every source that has the
/// field under that name. Then each field of the joining source is held against what the tracker
/// already compares: a number or a date that agrees with the other sources' values for at least
/// half the things they share (and at least three) joins that property. What is left is paired
/// with the other sources' fields nothing compares yet, by name or by the same agreement. A yes
/// or no says too little to tell two fields apart by its values, so it is paired by value only
/// where each side has exactly one left.
fn connect_properties(decl: &mut J, ds: &Source, registry: &BTreeMap<String, PathBuf>) {
    let scheme = decl["identified_by"][0].as_str().unwrap_or("").to_string();
    let me = ds.decl.name.clone();
    let comparable = |kind: &str| matches!(kind, "number" | "bool" | "date");
    let members: Vec<String> = decl["sources"].as_array().map(|l| l.iter().filter_map(|m| m["source"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    let mut sources: BTreeMap<String, Source> = BTreeMap::new();
    for m in &members {
        if *m == me {
            continue;
        }
        if let Some(s) = registry.get(m).and_then(|p| Source::open(p).ok()) {
            sources.insert(m.clone(), s);
        }
    }
    let source_of = |m: &str| -> Option<&Source> { if m == me { Some(ds) } else { sources.get(m) } };
    let kind_of = |m: &str, field: &str| -> Option<String> {
        source_of(m)?.decl.records.fields.get(field).map(|p| p.kind.name().to_string())
    };
    if !decl["align"].is_object() {
        decl["align"] = json!({});
    }

    // 1. Explicit: the field each source gives a compared property, written out.
    let keys: Vec<String> = decl["align"].as_object().map(|a| a.keys().cloned().collect()).unwrap_or_default();
    for key in &keys {
        for m in &members {
            let mapped = decl["align"][key]["from"].get(m).and_then(J::as_str).is_some();
            if !mapped && kind_of(m, key).is_some_and(|k| comparable(&k)) {
                if !decl["align"][key]["from"].is_object() {
                    decl["align"][key]["from"] = json!({});
                }
                decl["align"][key]["from"][m] = json!(key);
            }
        }
    }

    if scheme.is_empty() {
        return;
    }
    let same = |x: &str, y: &str| match (x.parse::<f64>(), y.parse::<f64>()) {
        (Ok(p), Ok(q)) => (p - q).abs() < 1e-9,
        _ => x == y,
    };
    // How far one field of mine agrees with a set of (source, field): agreeing, and both known.
    let agreement = |mine: &BTreeMap<String, String>, theirs: &[(String, String)]| -> (usize, usize) {
        let (mut agree, mut both) = (0, 0);
        for (m, f) in theirs {
            let Some(src) = source_of(m) else { continue };
            let values = src.store.values_by_identifier(&scheme, f);
            for (k, v) in mine {
                if let Some(w) = values.get(k) {
                    both += 1;
                    if same(v, w) {
                        agree += 1;
                    }
                }
            }
        }
        (agree, both)
    };
    let used_by_me = |decl: &J, field: &str| -> bool {
        decl["align"].as_object().is_some_and(|a| a.values().any(|v| v["from"].get(&me).and_then(J::as_str) == Some(field)))
    };
    let mine_left = |decl: &J, kind: &str| -> Vec<String> {
        ds.decl.records.fields.iter()
            .filter(|(n, p)| p.kind.name() == kind && !used_by_me(decl, n))
            .map(|(n, _)| n.clone())
            .collect()
    };

    for kind in ["number", "date", "bool"] {
        // 2. Against what the tracker compares already.
        let props: Vec<(String, Vec<(String, String)>)> = decl["align"].as_object().map(|a| a.iter().filter_map(|(k, v)| {
            if v["from"].get(&me).is_some() {
                return None;
            }
            let theirs: Vec<(String, String)> = v["from"].as_object()?.iter()
                .filter(|(m, f)| **m != me && f.as_str().is_some_and(|f| kind_of(m, f).as_deref() == Some(kind)))
                .map(|(m, f)| (m.clone(), f.as_str().unwrap_or("").to_string()))
                .collect();
            (!theirs.is_empty()).then(|| (k.clone(), theirs))
        }).collect()).unwrap_or_default();
        let left = mine_left(decl, kind);
        if kind != "bool" || (left.len() == 1 && props.len() == 1) {
            for field in &left {
                let mine = ds.store.values_by_identifier(&scheme, field);
                let best = props.iter().filter(|(k, _)| decl["align"][k]["from"].get(&me).is_none()).filter_map(|(k, theirs)| {
                    let (agree, both) = agreement(&mine, theirs);
                    (both >= 3 && agree * 2 >= both).then(|| (agree * 1000 / both, k.clone()))
                }).max();
                if let Some((_, key)) = best {
                    decl["align"][&key]["from"][&me] = json!(field);
                }
            }
        }

        // 3. With what nothing compares yet: by name first, then by the values.
        for (m, src) in &sources {
            let taken = |decl: &J, f: &str| decl["align"].as_object().is_some_and(|a| a.iter().any(|(k, v)| {
                v["from"].get(m).and_then(J::as_str).map_or(k == f, |g| g == f)
            }));
            let theirs_left: Vec<String> = src.decl.records.fields.iter()
                .filter(|(n, p)| p.kind.name() == kind && !taken(decl, n))
                .map(|(n, _)| n.clone())
                .collect();
            for field in mine_left(decl, kind) {
                let by_name = theirs_left.iter().find(|t| **t == field).cloned();
                let by_value = || -> Option<String> {
                    if kind == "bool" && (mine_left(decl, kind).len() != 1 || theirs_left.len() != 1) {
                        return None;
                    }
                    let mine = ds.store.values_by_identifier(&scheme, &field);
                    theirs_left.iter().filter_map(|t| {
                        let (agree, both) = agreement(&mine, &[(m.clone(), t.clone())]);
                        (both >= 3 && agree * 2 >= both).then(|| (agree * 1000 / both, t.clone()))
                    }).max().map(|(_, t)| t)
                };
                let Some(theirs) = by_name.or_else(by_value) else { continue };
                if decl["align"][&theirs].is_object() && decl["align"][&theirs]["from"].get(m).is_some() {
                    continue;
                }
                // Added to what is there under that name (a scale, a tolerance), never in place of it.
                if !decl["align"][&theirs].is_object() {
                    decl["align"][&theirs] = json!({});
                }
                if !decl["align"][&theirs]["from"].is_object() {
                    decl["align"][&theirs]["from"] = json!({});
                }
                decl["align"][&theirs]["from"][m] = json!(theirs.clone());
                decl["align"][&theirs]["from"][&me] = json!(field);
            }
        }
    }
    if decl["align"].as_object().is_some_and(|a| a.is_empty()) {
        decl.as_object_mut().map(|o| o.remove("align"));
    }
}
/// What every watch has to tell, delivered. A delivery that fails moves nothing, and the next
/// pass says it again.
fn deliver_watches(root: &Path) {
    for w in crate::watch::all(root) {
        match w.check(root).and_then(|(report, mark)| w.deliver(&report, &mark)) {
            Ok(_) => {}
            Err(e) => eprintln!("{}: {e}", w.decl.name),
        }
    }
}
/// What the person at the machine has not seen yet on each tracker's Changes page.
fn unseen(root: &Path) -> BTreeMap<String, i64> {
    crate::tracker::scope_registry(&root.join("trackers"))
        .values()
        .filter_map(|dir| {
            let n = crate::thingstore::ThingStore::open(dir).ok()?.unseen("you");
            Some((dir.file_name()?.to_string_lossy().into_owned(), n))
        })
        .collect()
}
fn workspace(args: &[String]) -> Result<PathBuf, String> {
    // A folder named is made a workspace, if it is not one yet: `zetlyn .` in an empty folder.
    if let Some(named) = crate::positional(args, 1).first() {
        return made(PathBuf::from(named.as_str()));
    }
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    let is_one = |p: &Path| p.join("workspace.yaml").exists() || p.join("sources").is_dir() || p.join("trackers").is_dir();
    if let Some(found) = here.ancestors().find(|p| is_one(p)) {
        return Ok(found.to_path_buf());
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or("no HOME to put ~/zetlyn in")?;
    made(home.join("zetlyn"))
}

fn made(root: PathBuf) -> Result<PathBuf, String> {
    for d in ["sources", "trackers"] {
        std::fs::create_dir_all(root.join(d)).map_err(|e| format!("{}: {e}", root.display()))?;
    }
    let file = root.join("workspace.yaml");
    if !file.exists() {
        std::fs::write(&file, "title: Zetlyn\n").map_err(|e| format!("{}: {e}", file.display()))?;
    }
    Ok(root.canonicalize().unwrap_or(root))
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
    /// What it is doing, in words, for the bar every page shows while it runs.
    label: String,
    /// How far it is: `{ share, text }`, where it can tell.
    progress: Option<serde_json::Value>,
    /// Asked to stop; the work looks at this between pages.
    stop: bool,
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
    /// Who is signed in, hosted: shown in the header, member or not.
    who: Option<String>,
    /// The organisations whoever is signed in belongs to, for the switch in the header.
    orgs_of_who: Vec<(String, String)>,
    /// Every public tracker on the machine, for a reader's sidebar: its title, where it is.
    public_of_machine: Vec<(String, String)>,
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
    fn label(&self, label: impl Into<String>) {
        if let Ok(mut j) = self.jobs.lock() {
            if let Some(job) = j.all.get_mut(&self.id) {
                job.label = label.into();
            }
        }
    }
    fn show(&self, share: Option<f64>, text: String) {
        if let Ok(mut j) = self.jobs.lock() {
            if let Some(job) = j.all.get_mut(&self.id) {
                job.progress = Some(json!({ "share": share, "text": text }));
            }
        }
    }
    fn stopping(&self) -> bool {
        self.jobs.lock().ok().and_then(|j| j.all.get(&self.id).map(|job| job.stop)).unwrap_or(true)
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
        // A name in an address is one name: never a way out of the directory it names something
        // in. `..%2F..` decodes after the split, so it is looked at here, once, for every route.
        let unsafe_name = |s: &String| s == "." || s == ".." || s.contains(['/', '\\', '\0']);
        let named = if parts.first().map(String::as_str) == Some("t") { &parts[1.min(parts.len())..2.min(parts.len())] } else { &parts[..] };
        if named.iter().any(unsafe_name) {
            return respond(request, 400, "text/plain; charset=utf-8", "not a name");
        }
        // Another site's page cannot act here: a browser says where a form came from, and a POST
        // from anywhere but this app's own pages is refused. Programs (a webhook) say nothing.
        if request.method() == &tiny_http::Method::Post {
            let header = |name: &'static str| request.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str().to_string());
            let host = header("Host").unwrap_or_default();
            let cross = header("Sec-Fetch-Site").is_some_and(|s| s == "cross-site")
                || header("Origin").is_some_and(|o| o == "null" || o.split("://").nth(1).unwrap_or("") != host);
            if cross {
                return respond(request, 403, "text/plain; charset=utf-8", "a form from another site");
            }
        }
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
        let signed_in = session.and_then(|s| h.accounts.by_session(&s)).map(|a| a.email);
        let owner = signed_in.as_deref().is_some_and(|e| h.is_member(e));
        self.who = signed_in;
        let post = request.method() == &tiny_http::Method::Post;
        let html_kind = "text/html; charset=utf-8";
        // One sign-in for every workspace on the machine: it is at the root, not here.
        if h.shared && parts.first().is_some_and(|p| p == "signin" || p == "signout") {
            redirect(request, "/signin");
            return None;
        }
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
            // Signed by its proposer's own key, so nobody signs in to propose either.
            ["propose", source] if post => {
                let key = request.headers().iter().find(|x| x.field.equiv("X-Zetlyn-Key")).map(|x| x.value.as_str().to_string());
                let mut body = Vec::new();
                let _ = std::io::Read::read_to_end(&mut std::io::Read::take(request.as_reader(), crate::propose::MAX_BODY as u64 + 1), &mut body);
                let (status, answer) = self.take_proposal(source, &body, key.as_deref(), signature.as_deref());
                respond(request, status, "application/json", &answer);
                None
            }
            ["signin"] if post => {
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut body);
                let email = parse_form(&body).get("email").cloned().unwrap_or_default();
                // The same words whoever asks, so the page does not say whose workspace it is.
                if h.is_member(&email) {
                    let email = email.trim().to_lowercase();
                    let sent = h.accounts.ensure(&email).and_then(|a| h.accounts.new_link(a.id)).and_then(|raw| {
                        let site = crate::account::Site::load(&self.root);
                        let link = format!("{}{}", site.url.trim_end_matches('/'), serve::at(&format!("/signin/{raw}")));
                        site.send(&email, "Your Zetlyn sign-in link", &format!("{link}\n\nGood for a quarter of an hour, and once."))
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
        // The header every page has: home is the workspace, and a page about one tracker is in it.
        let home = format!("{}/", self.base);
        let nav = if self.visitor {
            Vec::new()
        } else {
            vec![
                ("Trackers".to_string(), home.clone()),
                ("Proposals".to_string(), format!("{}/proposals", self.base)),
                ("Assist".to_string(), format!("{}/assist", self.base)),
                ("Docs".to_string(), "https://zetlyn.com/docs".to_string()),
            ]
        };
        serve::frame_home("Your trackers", &home, nav);
        serve::frame_hosted(None, None);
        // On app.zetlyn.com an organisation's pages wear the website's header and footer, under
        // the machine they are part of, with who is signed in at the right.
        if self.hosted.as_ref().is_some_and(|h| h.shared) {
            let title = crate::account::Site::load(&self.root).title;
            let title = if title.is_empty() { "Organisation".to_string() } else { title };
            // A member's links are the organisation's own; a reader's are what anybody may read.
            let (links, current) = if self.visitor {
                // A reader's sidebar is what anybody may read on the machine, this one marked.
                let here = match parts.as_slice() {
                    [t, name, ..] if t == "t" => self.public_of_machine.iter().find(|(_, h)| h.ends_with(&format!("/t/{name}/"))).map(|(l, _)| l.clone()),
                    _ => None,
                };
                (self.public_of_machine.clone(), here)
            } else {
                let here = match parts.first().map(String::as_str) {
                    Some("assist") => "Assist",
                    Some("settings") => "Settings",
                    Some("proposals") => "Proposals",
                    None => "Trackers",
                    _ => "",
                };
                (
                    vec![
                        ("Trackers".to_string(), home.clone()),
                        ("Proposals".to_string(), format!("{}/proposals", self.base)),
                        ("Assist".to_string(), format!("{}/assist", self.base)),
                        ("Settings".to_string(), format!("{}/settings", self.base)),
                    ],
                    Some(here.to_string()).filter(|h| !h.is_empty()),
                )
            };
            serve::frame_site("App");
            serve::frame_home(&title, &home, links);
            serve::frame_current(current);
            serve::frame_hosted(Some(("App".into(), "/".into())), Some(self.who.clone()));
            serve::frame_area("app", Some(title.clone()), self.orgs_of_who.clone());
            serve::frame_side(if self.visitor { "Public trackers" } else { "" });
        } else {
            serve::frame_area("", None, Vec::new());
        }
        if self.visitor {
            serve::frame_app(None, None);
        } else {
            let words = match crate::autoupdate::every(&self.root) {
                Some(s) => format!("Auto-update: {}", crate::autoupdate::words(s)),
                None => "Auto-update: off".to_string(),
            };
            let on = crate::autoupdate::every(&self.root).is_some();
            serve::frame_app(Some(home.clone()), Some((words, format!("{}/settings", self.base), on)));
        }
        serve::frame_section(None, Vec::new());
        if let [first, tracker, ..] = parts.as_slice() {
            if first != "t" && first != "job" {
                let dir = self.trackers().join(tracker);
                let title = TrackerDecl::load(&dir).ok().map(|d| d.title).or_else(|| draft_title(&dir));
                if let Some(title) = title {
                    let href = if dir.join("tracker.yaml").exists() { format!("{}/t/{tracker}/", self.base) } else { format!("{}/new/{tracker}?title={}", self.base, urlencode(&title)) };
                    serve::frame_section(Some((title, href)), Vec::new());
                }
            }
        }

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
        // A proposal says who signed it in two headers; on this machine as on a hosted one.
        let signed_by = |name: &'static str| request.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str().to_string());
        let (proposer, proposal_signature) = (signed_by("X-Zetlyn-Key"), signed_by("X-Zetlyn-Signature"));

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
            (true, ["reach", tracker, source]) => {
                let id = self.reach(tracker, source, query.get("choice").cloned().unwrap_or_default(), form_title(&query));
                (200, json_kind, json!({ "job": id }).to_string())
            }
            (true, ["job", id, "stop"]) => {
                if let (Ok(id), Ok(mut j)) = (id.parse::<u64>(), self.jobs.lock()) {
                    if let Some(job) = j.all.get_mut(&id) {
                        job.stop = true;
                    }
                }
                (200, json_kind, json!({ "stopping": true }).to_string())
            }
            // What runs in the background, for the bar on every page.
            (false, ["jobs"]) => {
                let running: Vec<J> = self.jobs.lock().map(|j| {
                    j.all.iter().filter(|(_, job)| !job.done && !job.label.is_empty())
                        .map(|(id, job)| json!({ "id": id, "label": job.label, "progress": job.progress, "stop": job.stop }))
                        .collect()
                }).unwrap_or_default();
                // And how much is new, for the count in the browser tab's title.
                let unseen: i64 = if self.visitor { 0 } else { unseen(&self.root).values().sum() };
                (200, json_kind, json!({ "running": running, "unseen": unseen }).to_string())
            }
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
                    j["identified_by"] = json!({ scheme.clone(): from });
                    if let Some(c) = j["claims"].as_object_mut() {
                        c.remove("id");
                    }
                    let decl: crate::sourcedecl::SourceDecl = serde_json::from_value(j).map_err(|e| e.to_string())?;
                    std::fs::write(dir.join(crate::sourcedecl::FILE), crate::yaml::to_string(&decl)?).map_err(|e| e.to_string())?;
                    Source::open(&dir)?.run_with(true, true).map(|_| ())
                })();
                match done {
                    Ok(()) => return redirect(request, &serve::at(&format!("/review/{tracker}/{source}?title={}", urlencode(&form_title(&query))))),
                    Err(e) => (400, html_kind, page("Not changed", html! { div.note { (e) } })),
                }
            }
            (true, ["propose", source]) => {
                let (status, answer) = self.take_proposal(source, &body, proposer.as_deref(), proposal_signature.as_deref());
                (status, json_kind, answer)
            }
            (false, ["proposals"]) => (200, html_kind, self.proposal_sources_page(&query)),
            (true, ["proposals"]) => match self.new_proposal_source(&form) {
                Ok(slug) => return redirect(request, &serve::at(&format!("/proposals/{slug}?said={}", urlencode("Made. Invite the keys that may propose.")))),
                Err(e) => return redirect(request, &serve::at(&format!("/proposals?said={}", urlencode(&e)))),
            },
            (true, ["proposals", source, verb @ ("invite" | "uninvite")]) => {
                let said = self.invite(source, form.get("key").map(String::as_str).unwrap_or(""), *verb == "invite").unwrap_or_else(|e| e);
                return redirect(request, &serve::at(&format!("/proposals/{source}?said={}", urlencode(&said))));
            }
            (false, ["proposals", source]) => match self.proposals_page(source, &query) {
                Ok(p) => (200, html_kind, p),
                Err(e) => (404, html_kind, page("Not here", html! { p { (e) } })),
            },
            (true, ["proposals", source, name, verb @ ("accept" | "reject")]) => {
                let dir = self.sources().join(source);
                let by = self.decider();
                match crate::propose::decide(&dir, name, *verb == "accept", &by, form.get("why").map(String::as_str).unwrap_or("")) {
                    Ok(()) => {
                        // Read now, as a push is, so what was accepted is a claim before the page comes back.
                        if let Ok(ds) = Source::open(&dir) {
                            let _ = ds.run();
                        }
                        return redirect(request, &serve::at(&format!("/proposals/{source}")));
                    }
                    Err(e) => (400, html_kind, page("Not decided", html! { p { (e) } p { a href=(serve::at(&format!("/proposals/{source}"))) { "Back to the proposals" } } })),
                }
            }
            (false, ["settings"]) => (200, html_kind, self.settings_page(&query)),
            (true, ["settings"]) => {
                let every = form.get("every").map(String::as_str).filter(|e| matches!(*e, "1h" | "6h" | "1d"));
                let said = match crate::autoupdate::set(&self.root, every, true) {
                    Ok(()) => match every.and_then(crate::fetch::duration) {
                        Some(s) => format!("Saved. Zetlyn now updates your sources {}.", crate::autoupdate::words(s)),
                        None => "Saved. Automatic updates are off.".to_string(),
                    },
                    Err(e) => e,
                };
                let back = query.get("back").filter(|b| b.starts_with('/') && !b.starts_with("//")).cloned();
                return redirect(request, &back.unwrap_or_else(|| serve::at(&format!("/settings?saved={}", urlencode(&said)))));
            }
            // The offer after a second source, answered "not now": it is not made again.
            (true, ["settings", "offered"]) => {
                let current = crate::account::Site::load(&self.root).update.every;
                let _ = crate::autoupdate::set(&self.root, current.as_deref(), true);
                let back = query.get("back").filter(|b| b.starts_with('/') && !b.starts_with("//")).cloned();
                return redirect(request, &back.unwrap_or_else(|| serve::at("/")));
            }
            (true, ["settings", "source", slug]) => {
                let dir = self.sources().join(slug);
                let every = form.get("every").map(String::as_str).filter(|e| matches!(*e, "15m" | "1h" | "6h" | "1d" | "never"));
                let said = (|| -> Result<String, String> {
                    let mut decl = crate::sourcedecl::SourceDecl::load(&dir)?;
                    decl.schedule.every = every.map(str::to_string);
                    let path = dir.join(crate::sourcedecl::FILE);
                    std::fs::write(&path, crate::yaml::to_string(&decl)?).map_err(|e| format!("{}: {e}", path.display()))?;
                    Ok(format!("{}: {}.", decl.title, match every { Some("never") => "never updated by itself".to_string(), Some(e) => crate::fetch::duration(e).map(crate::autoupdate::words).unwrap_or_default(), None => "as above".to_string() }))
                })()
                .unwrap_or_else(|e| e);
                return redirect(request, &serve::at(&format!("/settings?saved={}", urlencode(&said))));
            }
            (true, ["settings", "retry", slug]) => {
                crate::autoupdate::forgive(&self.sources().join(slug));
                return redirect(request, &serve::at(&format!("/settings?saved={}", urlencode("It will be asked again on the next pass."))));
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
                let id = self.publish_to_hub(tracker, false);
                (200, json_kind, json!({ "job": id }).to_string())
            }
            (true, ["publish-hub-sealed", tracker]) => {
                let id = self.publish_to_hub(tracker, true);
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
                        json!({ "lines": j.lines, "done": j.done, "error": j.error, "then": j.then, "label": j.label, "progress": j.progress, "stop": j.stop }).to_string(),
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
            p.label(format!("Reading {}", if from.contains("://") { from.split('/').nth(2).unwrap_or(&from) } else { from.rsplit('/').next().unwrap_or(&from) }));
            let local = Path::new(&from).is_file();
            let github = from.starts_with("github:");
            // An upload already sits in its own directory under sources/; anything else, an address or
            // a file somewhere on the machine, gets one there named after it. A file elsewhere is
            // read where it is, and its folder is never written to, let alone removed.
            let upload = local && Path::new(&from).parent().is_some_and(|p| p.parent() == Some(sources.as_path()));
            let dir = if upload {
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
                    if dir.starts_with(&sources) { let _ = std::fs::remove_dir_all(&dir); }
                    return Ok(serve::at(&format!("/webpage/{tracker}?url={}&title={}&feeds={}", urlencode(&from), urlencode(&title), urlencode(feeds))));
                }
                if e.contains("answers JSON") {
                    if dir.starts_with(&sources) { let _ = std::fs::remove_dir_all(&dir); }
                    return Ok(serve::at(&format!("/assist/{tracker}/{source}?url={}&title={}", urlencode(&from), urlencode(&title))));
                }
            }
            if let Err(e) = proposed {
                if dir.starts_with(&sources) { let _ = std::fs::remove_dir_all(&dir); }
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
        connect_properties(&mut decl, &ds, &crate::tracker::registry(&self.sources()));
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
            p.label(format!("Asking {} about {url}", assist.who()));
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
            p.label(format!("Asking {} why", assist.who()));
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
                    @if let Some(p) = &t.decl.package {
                        h2 { "On a hub" }
                        p.dim {
                            "It came as a " @if p.sealed { "sealed " } "package, version " code { (p.version.get(..8).unwrap_or(&p.version)) }
                            @if !p.key.is_empty() { ", signed by " code { (p.key) } }
                            ". A package is published by whoever made it."
                        }
                    } @else if self.hosted.is_none() {
                        h2 { "On a hub" }
                        p.dim { "Sealed, it travels as one file on hub.zetlyn.com: everything it answers with, every claim and its history, and not how it was made. Which source is read where, their own field names, the mappings and the receipts stay here. Signed with this machine's key (" code { "zetlyn id" } ")." }
                        form #publish-sealed data-job=(serve::at(&format!("/publish-hub-sealed/{tracker}"))) {
                            button.primary type="submit" disabled[public && !blocked.is_empty()] { "Publish sealed" }
                        }
                        p.dim { "Open, it puts its sources and its statement there as they are, recipes included, for somebody to copy and change." }
                        form #publish data-job=(serve::at(&format!("/publish-hub/{tracker}"))) {
                            button type="submit" disabled[public && !blocked.is_empty()] { "Publish open" }
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

    /// Open: every source and the statement, recipes included. Sealed: one file with what the
    /// tracker answers with, and not how it was made.
    fn publish_to_hub(&self, tracker: &str, sealed: bool) -> u64 {
        let (dir, sources) = (self.trackers().join(tracker), self.sources());
        self.start(move |p| {
            p.label(if sealed { "Publishing sealed to the hub" } else { "Publishing to the hub" });
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let t = Tracker::open(&dir, &sources)?;
            let registry = crate::tracker::registry(&sources);
            let run = |args: &[&str]| -> Result<String, String> {
                let out = std::process::Command::new(&exe).args(args).output().map_err(|e| e.to_string())?;
                let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
                if out.status.success() { Ok(said) } else { Err(said.trim().to_string()) }
            };
            let back = serve::at(&format!("/publish/{}", dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
            if sealed {
                p.say(format!("zetlyn tracker publish {} --sealed", dir.display()));
                p.say(run(&["tracker", "publish", &dir.display().to_string(), "--sealed"])?.trim().to_string());
                return Ok(back);
            }
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
                @if url.split('/').skip(3).all(|p| p.is_empty() || p.starts_with('?')) {
                    div.note { "This is the front page of " (url.split('/').nth(2).unwrap_or("the site")) ": a mix of lists, each short, with the same item in several. For a topic, the address of a search or a category on the site works better (search there, and paste the address the search shows)." }
                }
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

    /// A list on a web page, proposed as a source: one page read as a trial, and the list measured
    /// by asking for a few of its pages, so how much more to read is chosen knowing how long it is.
    fn read_web(&self, tracker: &str, url: String, title: String, pick: usize) -> u64 {
        let host = url.split('/').nth(2).unwrap_or("site").trim_start_matches("www.").to_string();
        // Named after what the address says: …/examples/lindenhof/ is lindenhof, and
        // store.steampowered.com/search is steampowered.
        let source = self.free(&self.sources(), &crate::guess::page_name(&url));
        let dir = self.sources().join(&source);
        let tracker = tracker.to_string();
        self.start(move |p| {
            p.label(format!("Trying {host}"));
            p.say(format!("Reading the first page of {url}"));
            let body = crate::fetch::Fetcher::new(crate::sourcedecl::AGENT, &BTreeMap::new(), 0)?.get(&url)?;
            if let Err(e) = crate::guess::propose_web(&url, &body, &dir, Some(&format!("local/{source}")), pick) {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(e);
            }
            let ds = Source::open(&dir)?;
            describe_ids(&ds, p);
            let report = ds.run()?;
            p.say(format!("A trial: {} claims from the first page", report.added + report.changed + report.unchanged));
            if matches!(&ds.decl.source, crate::sourcedecl::Fetch::Web { page: Some(_), .. }) {
                p.say("How long is the list? Asking for a few of its pages, not reading them:");
                let cutoff = match &ds.decl.source {
                    crate::sourcedecl::Fetch::Web { since: Some(_), since_default, .. } => Some(since_default.clone()),
                    _ => None,
                };
                let say = |line: String| p.say(format!("  {line}"));
                let m = crate::web::with_spec(&ds.decl.source, |spec| crate::web::measure(spec, cutoff.as_deref(), &say)).transpose()?;
                if let Some(m) = m {
                    let _ = std::fs::write(dir.join(crate::web::MEASURE), serde_json::to_string(&m).unwrap_or_default());
                }
            }
            Ok(serve::at(&format!("/review/{tracker}/{source}?title={}", urlencode(&title))))
        })
    }

    /// How much of a web list to read, chosen after its trial: the newest 500, back to the date
    /// the list is cut at, all of it, or on from where a read stopped. Said page by page.
    fn reach(&self, tracker: &str, source: &str, choice: String, title: String) -> u64 {
        let dir = self.sources().join(source);
        let (tracker, source) = (tracker.to_string(), source.to_string());
        self.start(move |p| {
            let before = Source::open(&dir)?;
            let (held, mark) = (before.store.count() as usize, before.store.meta("mark"));
            drop(before);
            let mut decl = crate::sourcedecl::SourceDecl::load(&dir)?;
            let m = crate::web::measure_of(&dir);
            let per = m.as_ref().map(|m| m.per_page.max(1)).unwrap_or(25);
            let crate::sourcedecl::Fetch::Web { limit, top, since_default, .. } = &mut decl.source else {
                return Err("only a web list is read this way".into());
            };
            let mut further: Option<String> = None;
            let pages: Option<usize> = match choice.as_str() {
                // Further back than a read that is done: on from the page it got to, down to a date.
                "back" | "back-all" => {
                    *limit = 0;
                    *top = 0;
                    if choice == "back-all" {
                        *since_default = "1970-01-01".into();
                    }
                    further = Some(since_default.clone());
                    if choice == "back" { m.as_ref().and_then(|m| m.to_cutoff) } else { m.as_ref().and_then(|m| m.last) }
                }
                "newest" => {
                    *limit = 0;
                    *top = 500;
                    Some(500usize.div_ceil(per))
                }
                "since" => {
                    *limit = 0;
                    *top = 0;
                    m.as_ref().and_then(|m| m.to_cutoff)
                }
                "all" => {
                    *limit = 0;
                    *top = 0;
                    *since_default = "1970-01-01".into();
                    m.as_ref().and_then(|m| m.last)
                }
                // Going on: as far as the read that stopped was going.
                _ if *top > 0 => Some(top.div_ceil(per)),
                _ => m.as_ref().and_then(|m| if since_default.as_str() <= "1970-01-01" { m.last } else { m.to_cutoff }),
            };
            let path = dir.join(crate::sourcedecl::FILE);
            std::fs::write(&path, crate::yaml::to_string(&decl)?).map_err(|e| format!("{}: {e}", path.display()))?;
            let ds = Source::open(&dir)?;
            if let Some(until) = further {
                let r = crate::web::Resume { page: held / per, items: held, high: mark, why: "reading further back".into(), until: Some(until), paused: false };
                let _ = std::fs::write(dir.join(crate::web::RESUME), serde_json::to_string(&r).unwrap_or_default());
            }
            // Whoever stopped it says go on.
            if let Some(mut r) = crate::web::resume_of(&dir).filter(|r| r.paused) {
                r.paused = false;
                let _ = std::fs::write(dir.join(crate::web::RESUME), serde_json::to_string(&r).unwrap_or_default());
            }
            let from = crate::web::resume_of(&dir).map(|r| r.page).unwrap_or(0);
            let of = pages.map(|n| n.max(from + 1));
            p.label(format!("Reading {}", ds.decl.title));
            p.say(match of {
                Some(n) => format!("Reading {} pages, one a second or so; you can leave this page, the bar at the bottom follows it", n - from),
                None => "Reading every page".to_string(),
            });
            let started = std::time::Instant::now();
            let hook = p.clone();
            crate::web::on_page(Some(Box::new(move |step: &crate::web::Step| {
                let done = step.page.saturating_sub(from);
                let a_page = started.elapsed().as_secs_f64() / done.max(1) as f64;
                let reached = step.reached.as_ref().map(|d| format!(" · back to {d}")).unwrap_or_default();
                let text = match of {
                    Some(n) => format!(
                        "page {} of about {n} · {} items{reached} · {} left",
                        step.page, crate::web::thousands(step.items), crate::web::duration(n.saturating_sub(step.page) as f64 * a_page)
                    ),
                    None => format!("page {} · {} items{reached}", step.page, crate::web::thousands(step.items)),
                };
                hook.show(of.map(|n| (step.page as f64 / n as f64).min(1.0)), text);
                !hook.stopping()
            })));
            let report = ds.run();
            crate::web::on_page(None);
            report?;
            match crate::web::resume_of(&dir) {
                Some(r) => p.say(format!("Not finished ({}). {} claims are kept; it goes on from page {} when you ask.", r.why, crate::web::thousands(ds.store.count() as usize), r.page + 1)),
                None => p.say(format!("{} claims in {}", ds.store.count(), crate::web::duration(started.elapsed().as_secs_f64()))),
            }
            Ok(serve::at(&format!("/review/{tracker}/{source}?title={}", urlencode(&title))))
        })
    }


    /// Automatic updates: the one switch, what it never does, and each source's own rhythm.
    fn settings_page(&self, query: &BTreeMap<String, String>) -> String {
        let every = crate::autoupdate::every(&self.root);
        let current = crate::account::Site::load(&self.root).update.every.unwrap_or_default();
        let choices = [("", "Off", "You update with Update now."), ("1h", "Every hour", ""), ("6h", "Every 6 hours", ""), ("1d", "Once a day", "")];
        let sources: Vec<(String, PathBuf)> = crate::tracker::registry(&self.sources()).into_iter().collect();
        let now = crate::now();
        let body = html! {
            h1 { "Automatic updates" }
            p.lede { "Zetlyn can read your sources again by itself and tell you what changed. It is off until you turn it on." }
            @if let Some(s) = query.get("saved") { div.note { (s) } }
            form.settings method="post" action=(serve::at("/settings")) {
                @for (value, label, hint) in choices {
                    label.choice {
                        input type="radio" name="every" value=(value) checked[current == value];
                        span { strong { (label) } @if !hint.is_empty() { " " span.dim { (hint) } } }
                    }
                }
                p { button.primary type="submit" { "Save" } }
            }
            div.note {
                "It works for as long as Zetlyn runs, with or without a page open in the browser, and shows at the bottom of every page while it works. Quit Zetlyn and it stops. "
                "On a machine that should keep watching without the app, " code { "zetlyn run " (self.root.display()) } " does the same, or use the hosted version."
            }
            h2 { "What it never does by itself" }
            ul {
                li { "Read a source for the first time, or a trial of one page: that is yours to start." }
                li { "Go on with a read you stopped, or read a list further back." }
                li { "Ask any source more often than every 15 minutes." }
                li { "Keep asking a source that failed three times running: it waits, with the reason, until you say try again." }
            }
            details open[sources.iter().any(|(_, d)| Source::open(d).ok().is_some_and(|ds| ds.decl.schedule.every.is_some()))] {
                summary { "Each source" }
                p.dim { "A source follows the setting above unless it has its own." }
                table {
                    thead { tr { th { "Source" } th { "How often" } th { "Now" } } }
                    tbody {
                        @for (_, dir) in &sources {
                            @if let Ok(ds) = Source::open(dir) {
                                @let slug = dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                                @let own = ds.decl.schedule.every.clone().unwrap_or_default();
                                @let held = crate::autoupdate::held(&ds, dir);
                                tr {
                                    td { strong { (ds.decl.title) } div.why.mono { (slug) } }
                                    td {
                                        form.bar method="post" action={(serve::at("/settings/source/")) (slug)} {
                                            select name="every" {
                                                @for (v, l) in [("", "As above"), ("15m", "Every 15 minutes"), ("1h", "Every hour"), ("6h", "Every 6 hours"), ("1d", "Once a day"), ("never", "Never")] {
                                                    option value=(v) selected[own == v] { (l) }
                                                }
                                            }
                                            button type="submit" { "Set" }
                                        }
                                    }
                                    td {
                                        @match (&held, crate::autoupdate::next_at(&ds, every)) {
                                            (Some(why), _) => {
                                                span.dim { (why) }
                                                @if crate::autoupdate::failures(&ds) >= crate::autoupdate::PATIENCE {
                                                    form method="post" action={(serve::at("/settings/retry/")) (slug)} { button type="submit" { "Try again" } }
                                                }
                                            }
                                            (None, None) => span.dim { "not updated by itself" },
                                            (None, Some(at)) if at <= now => span { "due now" },
                                            (None, Some(at)) => span { "next in " (crate::web::duration((at - now) as f64)) },
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        };
        page("Automatic updates", body)
    }
    /// A proposal for one of this workspace's sources, as `/propose/<source>` answers it.
    fn take_proposal(&self, source: &str, body: &[u8], key: Option<&str>, signature: Option<&str>) -> (u16, String) {
        let dir = self.sources().join(source);
        if !dir.join(crate::sourcedecl::FILE).exists() {
            return (404, json!({ "error": format!("{source}: no such source here") }).to_string());
        }
        match crate::propose::receive(&dir, body, key, signature) {
            Ok(name) => (202, json!({ "kept": name, "waiting": "for the source's owner" }).to_string()),
            Err(e) => {
                let status = if e.contains("not invited") { 403 } else if e.contains("unsigned") || e.contains("signature") { 401 } else { 400 };
                (status, json!({ "error": e }).to_string())
            }
        }
    }

    /// Who decides, in the words a decision is recorded with: the signed-in member on a hosted
    /// workspace, the identity on this machine.
    fn decider(&self) -> String {
        if let Some(who) = self.who.clone().filter(|w| !w.is_empty()) {
            return who;
        }
        let me = crate::identity::read();
        if !me.name.is_empty() {
            return me.name;
        }
        crate::identity::key().unwrap_or_else(|| "the owner".into())
    }

    /// The sources that take proposals, each with how many wait.
    fn proposal_sources(&self) -> Vec<(String, String, usize)> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(self.sources()).into_iter().flatten().flatten() {
            let dir = e.path();
            let Ok(decl) = crate::sourcedecl::SourceDecl::load(&dir) else { continue };
            if matches!(decl.source, crate::sourcedecl::Fetch::Proposals { .. }) {
                let waiting = crate::propose::list(&dir).iter().filter(|p| p.status == "pending").count();
                out.push((e.file_name().to_string_lossy().into_owned(), if decl.title.is_empty() { decl.name } else { decl.title }, waiting));
            }
        }
        out.sort();
        out
    }

    /// Every source here that people read for, and a form for another.
    fn proposal_sources_page(&self, query: &BTreeMap<String, String>) -> String {
        let all = self.proposal_sources();
        let body = html! {
            h1 { "Proposals" }
            p.lede { "A source nobody publishes as data can still be read by people. Each row is proposed by somebody you invited, signed with their own key, and becomes a claim only when you accept it." }
            @if let Some(s) = query.get("said") { div.note { (s) } }
            @if all.is_empty() { p.dim { "No source here takes proposals yet." } }
            table { tbody {
                @for (dir, title, waiting) in &all {
                    tr {
                        td { a href=(serve::at(&format!("/proposals/{dir}"))) { strong { (title) } } div.why.mono { (dir) } }
                        td.num { @if *waiting == 0 { span.dim { "nothing waiting" } } @else { a.chip.on href=(serve::at(&format!("/proposals/{dir}"))) { (waiting) " waiting" } } }
                    }
                }
            } }
            h2 { "A new source people read for" }
            p.dim { "Name the fields a row has. The ones that identify a row together make its identifier: country, trim and week make " code { "DEU-rwd-2026-W40" } ". Field names are lowercase letters, digits and underscores." }
            form.settings method="post" action=(serve::at("/proposals")) {
                p { label { "Title" br; input.wide type="text" name="title" placeholder="Tesla Model Y prices" required; } }
                p { label { "Identified by" br; input.wide type="text" name="identify" placeholder="country, trim, week" required; } }
                p { label { "Numbers" br; input.wide type="text" name="numbers" placeholder="price"; } }
                p { label { "Words" br; input.wide type="text" name="words" placeholder="currency"; } }
                p { button.primary type="submit" { "Make it" } }
            }
            p.dim { "It is made with no licence, so no public page shows it until you decide what may be shown, in its " code { "source.yaml" } "." }
        };
        page("Proposals", body)
    }

    /// A proposals source from the form: its declaration written, and read back before it is kept.
    fn new_proposal_source(&self, form: &BTreeMap<String, String>) -> Result<String, String> {
        let title = form.get("title").map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).ok_or("A title, please.")?;
        let fields = |k: &str| -> Result<Vec<String>, String> {
            let mut out = Vec::new();
            for f in form.get(k).map(String::as_str).unwrap_or("").split(',').map(str::trim).filter(|f| !f.is_empty()) {
                if !f.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                    return Err(format!("{f}: a field name is lowercase letters, digits and underscores"));
                }
                if crate::propose::RESERVED.contains(&f) {
                    return Err(format!("{f}: the proposal itself says that, so a row may not"));
                }
                if !out.iter().any(|o| o == f) {
                    out.push(f.to_string());
                }
            }
            Ok(out)
        };
        let (identify, numbers, words) = (fields("identify")?, fields("numbers")?, fields("words")?);
        if identify.is_empty() {
            return Err("Name at least one field that identifies a row.".into());
        }
        let slug = self.free(&self.sources(), &crate::guess::slug(&title));
        let owner = self.root.file_name().map(|s| crate::guess::slug(&s.to_string_lossy())).filter(|s| !s.is_empty()).unwrap_or_else(|| "mine".into());
        let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
        let braced = |sep: &str| identify.iter().map(|f| format!("{{{f}}}")).collect::<Vec<_>>().join(sep);
        let mut yaml = format!(
            "name: {}\ntitle: {}\nkind: observation\nabout: {}\nfetch:\n  type: proposals\n  from: []\nclaims:\n  id:\n    scheme: {}\n    from: {}\n  title: {}\n  known: field:read_at\n  properties:\n",
            q(&format!("{owner}/{slug}")),
            q(&title),
            q("Rows read by people and proposed, each signed with the proposer's key; a row is here once the owner accepted it."),
            q(&slug),
            q(&format!("const:{}", braced("-"))),
            q(&format!("const:{}", braced(" "))),
        );
        for f in &numbers {
            yaml.push_str(&format!("    {f}:\n      type: number\n      from: field:{f}\n"));
        }
        for f in identify.iter().chain(&words).filter(|f| !numbers.contains(f)) {
            yaml.push_str(&format!("    {f}:\n      type: code\n      from: field:{f}\n"));
        }
        yaml.push_str("    proposed_by:\n      type: text\n      from: field:proposed_by\n    read_from:\n      type: text\n      from: field:read_from\n");
        let dir = self.sources().join(&slug);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(crate::sourcedecl::FILE), yaml).map_err(|e| e.to_string())?;
        if let Err(e) = crate::sourcedecl::SourceDecl::load(&dir) {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!("Not made: {e}"));
        }
        Ok(slug)
    }

    /// A key added to or taken from the ones a source takes proposals from.
    fn invite(&self, source: &str, key: &str, yes: bool) -> Result<String, String> {
        let dir = self.sources().join(source);
        let mut decl = crate::sourcedecl::SourceDecl::load(&dir)?;
        let crate::sourcedecl::Fetch::Proposals { from } = &mut decl.source else {
            return Err(format!("{} takes no proposals", decl.name));
        };
        let key = key.trim().to_lowercase();
        let hex = key.strip_prefix("ed25519:").unwrap_or("");
        if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("A key is ed25519: and 64 hex digits, as `zetlyn id` prints it.".into());
        }
        let said = if yes {
            if from.contains(&key) {
                return Ok("That key is invited already.".into());
            }
            from.push(key);
            "Invited. Proposals signed with that key are taken from now on."
        } else {
            from.retain(|k| k != &key);
            "No longer invited. What it proposed before stays, with its decisions."
        };
        let path = dir.join(crate::sourcedecl::FILE);
        std::fs::write(&path, crate::yaml::to_string(&decl)?).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(said.into())
    }

    /// What people proposed for one source: what waits, with who else said the same, and what
    /// was decided.
    fn proposals_page(&self, source: &str, query: &BTreeMap<String, String>) -> Result<String, String> {
        let dir = self.sources().join(source);
        let decl = crate::sourcedecl::SourceDecl::load(&dir)?;
        let crate::sourcedecl::Fetch::Proposals { from } = &decl.source else {
            return Err(format!("{} takes no proposals", decl.name));
        };
        let all = crate::propose::list(&dir);
        let show = query.get("show").map(String::as_str).unwrap_or("pending");
        let shown: Vec<_> = all.iter().filter(|e| show == "all" || e.status == show).rev().collect();
        let count = |s: &str| all.iter().filter(|e| e.status == s).count();
        let short = |k: &str| if k.len() > 24 { format!("{}…", &k[..24]) } else { k.to_string() };
        let body = html! {
            h1 { "Proposals: " (if decl.title.is_empty() { decl.name.clone() } else { decl.title.clone() }) }
            p.about { "Rows people read for this source and proposed, each signed with their own key. Only what is accepted reaches the source; a rejection withdraws a row accepted before, and every decision stays in " code { "decisions.jsonl" } "." }
            @if let Some(s) = query.get("said") { div.note { (s) } }
            details open[from.is_empty()] {
                summary { "Who may propose: " (from.len()) (if from.len() == 1 { " key" } else { " keys" }) }
                @if from.is_empty() { p.dim { "Nobody yet, so every proposal is refused. A proposer runs " code { "zetlyn id new --name … --contact …" } " once and sends you the key it prints." } }
                table { tbody {
                    @for k in from {
                        tr {
                            td.mono { (k) }
                            td.num { form method="post" action=(serve::at(&format!("/proposals/{source}/uninvite"))) { input type="hidden" name="key" value=(k); button type="submit" { "Remove" } } }
                        }
                    }
                } }
                form.bar method="post" action=(serve::at(&format!("/proposals/{source}/invite"))) {
                    input.wide type="text" name="key" placeholder="ed25519:…" required;
                    button.primary type="submit" { "Invite" }
                }
            }
            details {
                summary { "How a proposer sends a row" }
                p { "A file, " code { "row.json" } ", with the row and how it was read:" }
                pre { (format!("{{\n  \"row\": {{ … the fields of one row … }},\n  \"read_at\": \"{}\",\n  \"read_from\": \"https://… where it was read\",\n  \"attest\": \"read\",\n  \"note\": \"optional\"\n}}", crate::iso_stamp(crate::now()).get(..10).unwrap_or(""))) }
                p { "Then, signed with their own key:" }
                pre { "zetlyn source propose " (format!("{}{}", crate::account::Site::load(&self.root).url.trim_end_matches('/'), serve::at(&format!("/propose/{source}")))) " row.json" }
                p.dim { code { "attest" } " is " code { "read" } " when they looked themselves, " code { "relayed" } " when somebody who did allows it, named in " code { "note" } "." }
            }
            p {
                @for (s, label) in [("pending", "Waiting"), ("accepted", "Accepted"), ("rejected", "Rejected"), ("all", "All")] {
                    a.chip.on[show == s] href={(serve::at(&format!("/proposals/{source}"))) "?show=" (s)} { (label) @if s != "all" { " " (count(s)) } }
                    " "
                }
            }
            @if shown.is_empty() { p.dim { "Nothing here." } }
            table { tbody {
                @for e in &shown {
                    tr {
                        td {
                            code { (e.row) }
                            div.why {
                                "read " (e.read_at) " from " a href=(e.read_from) { (e.read_from) } " · " (if e.attest == "read" { "read by the proposer" } else { "relayed" })
                                @if !e.note.is_empty() { " · " (e.note) }
                            }
                            div.why {
                                "proposed by " code title=(e.by) { (short(&e.by)) } " · arrived " (e.received)
                                @if !e.agreeing.is_empty() { " · " strong { (e.agreeing.len()) (if e.agreeing.len() == 1 { " other key says the same" } else { " other keys say the same" }) } }
                            }
                        }
                        td.num {
                            @if e.status == "pending" || e.status == "rejected" {
                                form method="post" action=(serve::at(&format!("/proposals/{source}/{}/accept", e.file))) { button.primary type="submit" { "Accept" } }
                            }
                            @if e.status == "pending" || e.status == "accepted" {
                                form method="post" action=(serve::at(&format!("/proposals/{source}/{}/reject", e.file))) {
                                    input type="text" name="why" placeholder="why, optional";
                                    button type="submit" { (if e.status == "accepted" { "Withdraw" } else { "Reject" }) }
                                }
                            }
                            @if e.status != "pending" { div.dim { (e.status) } }
                        }
                    }
                }
            } }
        };
        Ok(page("Proposals", body))
    }

    fn start_page(&self) -> String {
        let unseen = unseen(&self.root);
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
                    @for (name, decl, _fresh) in &trackers {
                        tr {
                            td { a href={(serve::at("/t/")) (name) "/"} { strong { (decl.title) } } div.why { (decl.members.len()) (if decl.members.len() == 1 { " source" } else { " sources" }) " · identified by " (decl.join.join(", ")) } }
                            td.num { @match unseen.get(name).copied().unwrap_or(0) { 0 => span.dim { "nothing new since you looked" }, n => a.chip.on href={(serve::at("/t/")) (name) "/changes"} { (n) " new since you looked" } } }
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
            @let proposed = self.proposal_sources();
            @if !proposed.is_empty() {
                h2 { "Proposals" }
                table { tbody {
                    @for (dir, title, waiting) in &proposed {
                        tr {
                            td { a href=(serve::at(&format!("/proposals/{dir}"))) { strong { (title) } } }
                            td.num { @if *waiting == 0 { span.dim { "nothing waiting" } } @else { a.chip.on href=(serve::at(&format!("/proposals/{dir}"))) { (waiting) " waiting" } } }
                        }
                    }
                } }
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

        // A web list: its trial, how long it is, and what of it to read.
        let src_dir = self.sources().join(source);
        let web = match &ds.decl.source {
            crate::sourcedecl::Fetch::Web { limit, top, since, since_default, .. } => Some((*limit > 0, *top, since.is_some(), since_default.clone())),
            _ => None,
        };
        let measured = crate::web::measure_of(&src_dir);
        let resume = crate::web::resume_of(&src_dir);
        let undecided = web.as_ref().is_some_and(|w| w.0);
        let reach = |choice: &str| format!("{}{tracker}/{source}?choice={choice}&title={}", serve::at("/reach/"), urlencode(&title));
        let body = html! {
            h1 { (ds.decl.title) }
            p.about { (total) " claims read from " code { (source_of(&ds)) } }
            // Just connected: said first, where the eye lands, with the way on.
            @if joined && added {
                div.offer {
                    p { strong { "Connected." } " " (ds.decl.title) " is part of " (title) "." }
                    a.button href={(serve::at("/t/")) (tracker) "/"} { "Open the tracker" }
                // What Zetlyn compares from here on, and under which of this source's columns:
                // a pairing it made by the values is said, so it can be checked.
                @let compared: Vec<(String, String)> = decl.as_ref().map(|d| d.normalise.iter().map(|(name, a)| {
                    let field = a.field_in(&ds.decl.name, name);
                    let column = ds.decl.records.fields.get(&field).map(|p| p.from.trim_start_matches("field:").to_string()).unwrap_or(field);
                    (name.clone(), column)
                }).filter(|(n, c)| ds.decl.records.fields.values().any(|p| p.from.trim_start_matches("field:") == c) || n == c).collect()).unwrap_or_default();
                @if !compared.is_empty() {
                    p.dim style="flex-basis:100%" {
                        "Compared from now on: "
                        @for (i, (name, column)) in compared.iter().enumerate() {
                            @if i > 0 { ", " }
                            strong { (name) }
                            @if column != name { " (here " code { (column) } ")" }
                        }
                        ". Where the names differ, Zetlyn paired them because the values agree on most things both know; it is written in tracker.yaml, and can be changed there."
                    }
                }
                }
            }
            // The moment it is worth something: two sources joined. Asked once, and "not
            // now" is an answer that is kept.
            @if added && decl.as_ref().is_some_and(|d| d.members.len() >= 2) && crate::autoupdate::every(&self.root).is_none() && !crate::autoupdate::offered(&self.root) {
                div.offer {
                    p { strong { "Keep an eye on these for you?" } br; span.dim { "Zetlyn reads both sources again every hour while it runs, and Changes says what moved. You can change this at any time under Auto-update." } }
                    form method="post" action={(serve::at("/settings?back=")) (urlencode(&serve::at(&format!("/t/{tracker}/"))))} {
                        input type="hidden" name="every" value="1h";
                        button.primary type="submit" { "Every hour" }
                    }
                    form method="post" action={(serve::at("/settings/offered?back=")) (urlencode(&serve::at(&format!("/review/{tracker}/{source}?title={}", urlencode(&title)))))} {
                        button type="submit" { "Not now" }
                    }
                }
            }

            @if let Some((trial, top, dated, since_default)) = &web {
                h2 { "How much to read" }
                @if let Some(r) = &resume {
                    div.note { "Not finished (" (r.why) "). The " (crate::web::thousands(total as usize)) " claims read are kept." }
                    form.bar data-job=(reach("continue")) { button.primary type="submit" { "Go on from page " (r.page + 1) } }
                } @else if *trial {
                    p { "That was a trial: the first page, " (total) " items. Nothing more is read until you choose." }
                    @if let Some(m) = &measured {
                        p.dim {
                            (m.per_page) " items a page"
                            @if let (Some(n), Some(o)) = (&m.newest, &m.oldest) { ", from " (n) " back to " (o) }
                            @match m.last { Some(l) => { ", " (crate::web::thousands(l)) " pages in all." } None => { ", more than 32,768 pages." } }
                        }
                    }
                    @let per = measured.as_ref().map(|m| m.per_page.max(1)).unwrap_or(25);
                    table { tbody {
                        tr {
                            td { form data-job=(reach("newest")) { button.primary type="submit" { "The newest 500" } } }
                            td.dim { @if let Some(m) = &measured { (m.about(500usize.div_ceil(per))) } }
                        }
                        @if *dated {
                            tr {
                                td { form data-job=(reach("since")) { button type="submit" { "Back to " (since_default) } } }
                                td.dim { @match measured.as_ref().and_then(|m| m.to_cutoff.map(|p| m.about(p))) { Some(a) => (a), None => "how far that is was not found" } }
                            }
                        }
                        tr {
                            td { form data-job=(reach("all")) { button type="submit" { "All of it" } } }
                            td.dim { @match measured.as_ref().and_then(|m| m.last.map(|p| m.about(p))) { Some(a) => (a), None => "longer than could be measured" } }
                        }
                    } }
                    p.dim { "It reads one page a second or so, to be a polite reader. You can leave the page while it reads: the bar at the bottom follows it and can stop it, and a stopped read goes on where it stopped. After this, an update reads only what is new, a page or two." }
                } @else {
                    p.dim { (crate::web::thousands(total as usize)) " items read" @if *top > 0 { ", the newest " (top) } ". An update reads only what is new." }
                    @if let Some(m) = &measured {
                        @let done = total as usize / m.per_page.max(1);
                        @let everything = since_default.as_str() <= "1970-01-01";
                        table { tbody {
                            @if *top > 0 && *dated {
                                @if let Some(p) = m.to_cutoff.filter(|p| *p > done) {
                                    tr {
                                        td { form data-job=(reach("back")) { button type="submit" { "Further back, to " (since_default) } } }
                                        td.dim { (m.about(p - done)) " more" }
                                    }
                                }
                            }
                            @if !everything || *top > 0 {
                                @if let Some(p) = m.last.filter(|p| *p > done) {
                                    tr {
                                        td { form data-job=(reach("back-all")) { button type="submit" { "All of it" } } }
                                        td.dim { (m.about(p - done)) " more" }
                                    }
                                }
                            }
                        } }
                    }
                }
                pre #log data-jobs=(serve::at("/job/")) hidden {}
                div #error .note hidden {}
                (PreEscaped(JOB_SCRIPT))
            }

            h2 { "What names a claim" }
            @if schemes.is_empty() {
                div.note { "No identifier Zetlyn knows was found. A tracker joins its sources on one, so a source needs something that names each of its claims." }
                @let exact = ds.store.unique_fields();
                @let repeated = exact.is_empty();
                @let unique = if repeated { ds.store.nearly_unique_fields() } else { exact };
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
                    p.dim { @if repeated { "These properties are on every claim and different on most: the page shows some items more than once, and each is kept once. " } @else { "These properties have a different value on every claim. " } "Choose the one that names an item the way another source would name it too: a second source meets this one on it." }
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
                        @if total > 0 && !undecided && !schemes.is_empty() { button.primary type="submit" { @if decl.is_none() { "Looks right" } @else { "Connect" } } }
                        button type="submit" formaction={(serve::at("/discard/")) (tracker) "/" (source) "?title=" (urlencode(&title))} { "Not right" }
                        @if schemes.is_empty() && total > 0 { span.dim { "Looks right comes once something names each claim: choose it above." } }
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
        .ids()
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
            p.dim { "Leafline Books publishes a price list; Bücherstube Lindenhof only has its shop's page. A few clicks, and you see where the two disagree on price and stock; two minutes later, what one of them changed." }
            form method="post" action=(serve::at("/example")) { button type="submit" { "Start from the bookshop example" } }
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


/// Whatever runs in the background, at the bottom of every page, with a way to stop it: a read of
/// a thousand pages is not something to find out about afterwards.
pub const BAR_SCRIPT: &str = r#"<script>
(function () {
  var box = document.getElementById('jobs'); if (!box) return;
  var at = box.dataset.at, title = document.title.replace(/^\(\d+\) /, '');
  function draw(running) {
    box.hidden = running.length == 0;
    document.body.classList.toggle('busy', running.length > 0);
    box.textContent = '';
    running.forEach(function (j) {
      var row = document.createElement('div'); row.className = 'job';
      var what = document.createElement('strong'); what.textContent = j.label; row.appendChild(what);
      var bar = document.createElement('progress'); bar.max = 1;
      if (j.progress && j.progress.share != null) bar.value = j.progress.share;
      row.appendChild(bar);
      var said = document.createElement('span'); said.className = 'dim';
      said.textContent = j.stop ? 'stopping after this page…' : (j.progress ? j.progress.text : 'working…');
      row.appendChild(said);
      if (!j.stop) {
        var stop = document.createElement('button'); stop.textContent = 'Stop';
        stop.onclick = function () { fetch(at + 'job/' + j.id + '/stop', { method: 'POST' }).then(look); };
        row.appendChild(stop);
      }
      box.appendChild(row);
    });
  }
  function look() {
    fetch(at + 'jobs').then(function (r) { return r.json(); }).then(function (j) {
      draw(j.running || []);
      document.title = (j.unseen > 0 ? '(' + j.unseen + ') ' : '') + title;
    })
      .catch(function () { box.hidden = true; });
  }
  look(); setInterval(look, 1500);
})();
</script>"#;
/// The one script: send a form or a file, then follow the job until it says where to go.
const JOB_SCRIPT: &str = r#"<script>
(function () {
  if (window.zetlynJobs) return; window.zetlynJobs = true;
  var log = document.getElementById('log'), error = document.getElementById('error'), bar = null;
  function follow(id) {
    fetch(((log && log.dataset.jobs) || '/job/') + id).then(function (r) { return r.json(); }).then(function (j) {
      log.hidden = false;
      log.textContent = j.lines.join('\n') + (j.done ? '' : '\n' + (j.progress ? j.progress.text : '…'));
      if (!bar) { bar = document.createElement('progress'); bar.max = 1; log.parentNode.insertBefore(bar, log); }
      if (j.progress && j.progress.share != null) bar.value = j.progress.share; else bar.removeAttribute('value');
      bar.hidden = j.done;
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
    /// Who may run it: one owner for `zetlyn host`, an organisation's members for
    /// `zetlyn hosting`. Everybody else reads what it publishes.
    members: Vec<String>,
    accounts: crate::account::Accounts,
    /// Signed in once for every workspace on the machine, at its root: `zetlyn hosting`.
    shared: bool,
}

impl Hosted {
    fn is_member(&self, email: &str) -> bool {
        self.members.iter().any(|m| m.eq_ignore_ascii_case(email.trim()))
    }
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
        hosted: Some(Hosted { members: vec![owner], accounts, shared: false }),
        visitor: false,
        who: None,
        orgs_of_who: Vec::new(),
        public_of_machine: Vec::new(),
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

// ---------------------------------------------------------------------------------------------
// Every organisation's workspace, in one process: `zetlyn hosting serve <dir>`.
//
//   <dir>/workspace.yaml     the machine's address, its mailer
//   <dir>/accounts.db        everybody who has signed in, once, for every organisation
//   <dir>/members.yaml       who belongs to which organisation, and as what
//   <dir>/orgs/<org>/        one workspace per organisation, the same as on anybody's machine
//
// An organisation's members run its workspace at `/<org>/`, as its owner would on their own
// machine. Anybody else, signed in or not, reads the trackers it publishes, and the front page
// lists every public tracker on the machine.

/// `members.yaml`: per organisation, who belongs to it and as what.
#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
struct Membership {
    #[serde(flatten)]
    orgs: BTreeMap<String, Vec<Member>>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct Member {
    email: String,
    #[serde(default = "an_owner")]
    role: String,
}

fn an_owner() -> String {
    "owner".into()
}

const MEMBERS: &str = "members.yaml";

impl Membership {
    fn load(dir: &Path) -> Membership {
        crate::yaml::read_or_default(&dir.join(MEMBERS))
    }
    fn save(&self, dir: &Path) -> Result<(), String> {
        let path = dir.join(MEMBERS);
        std::fs::write(&path, crate::yaml::to_string(self)?).map_err(|e| format!("{}: {e}", path.display()))
    }
    fn of(&self, org: &str) -> Vec<String> {
        self.orgs.get(org).map(|m| m.iter().map(|x| x.email.to_lowercase()).collect()).unwrap_or_default()
    }
    /// The organisations somebody belongs to, with what they are in each.
    fn orgs_of(&self, email: &str) -> Vec<(String, String)> {
        self.orgs
            .iter()
            .filter_map(|(org, ms)| ms.iter().find(|m| m.email.eq_ignore_ascii_case(email)).map(|m| (org.clone(), m.role.clone())))
            .collect()
    }
}

/// An organisation's name is an address and a directory: lower case, digits and hyphens.
fn org_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 40 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !s.starts_with('-')
}

fn orgs_in(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir.join("orgs"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| org_name(n))
        .collect();
    out.sort();
    out
}

/// `zetlyn hosting serve | org | member`.
pub fn hosting(args: &[String]) -> Result<(), String> {
    match args.get(1).map(String::as_str) {
        Some("serve") => hosting_serve(args),
        // A new organisation: its workspace, empty, beside the others.
        Some("org") => {
            let dir = PathBuf::from(crate::positional(args, 2).first().ok_or("which hosting directory?")?.as_str());
            let name = crate::positional(args, 2).get(1).map(|s| s.to_string()).ok_or("which organisation?")?;
            if !org_name(&name) {
                return Err(format!("{name}: an organisation's name is lower case letters, digits and hyphens"));
            }
            let root = dir.join("orgs").join(&name);
            for d in ["sources", "trackers"] {
                std::fs::create_dir_all(root.join(d)).map_err(|e| format!("{}: {e}", root.display()))?;
            }
            let file = root.join(crate::account::WORKSPACE);
            if !file.exists() {
                let title = crate::flag(args, "--title").unwrap_or(&name).to_string();
                std::fs::write(&file, format!("title: {}\n", serde_json::to_string(&title).unwrap_or_default()))
                    .map_err(|e| format!("{}: {e}", file.display()))?;
            }
            println!("{name} in {}", root.display());
            Ok(())
        }
        // Somebody in an organisation, as owner, editor or reader. `--remove` takes them out.
        Some("member") => {
            let dir = PathBuf::from(crate::positional(args, 2).first().ok_or("which hosting directory?")?.as_str());
            let org = crate::positional(args, 2).get(1).map(|s| s.to_string()).ok_or("which organisation?")?;
            let email = crate::positional(args, 2).get(2).map(|s| s.trim().to_lowercase()).ok_or("whose address?")?;
            if !dir.join("orgs").join(&org).is_dir() {
                return Err(format!("{org}: no such organisation. `zetlyn hosting org {} {org}` makes it", dir.display()));
            }
            let role = crate::flag(args, "--role").unwrap_or("owner").to_string();
            if !matches!(role.as_str(), "owner" | "editor" | "reader") {
                return Err(format!("{role}: an owner, an editor or a reader"));
            }
            let mut m = Membership::load(&dir);
            let list = m.orgs.entry(org.clone()).or_default();
            list.retain(|x| !x.email.eq_ignore_ascii_case(&email));
            let removing = args.iter().any(|a| a == "--remove");
            if !removing {
                list.push(Member { email: email.clone(), role: role.clone() });
            }
            m.save(&dir)?;
            println!("{email} {} {org}", if removing { "is no longer in".to_string() } else { format!("is {role} in") });
            Ok(())
        }
        _ => Err("zetlyn hosting serve <dir> [--addr 127.0.0.1:2400] | org <dir> <name> [--title …] | member <dir> <org> <email> [--role owner|editor|reader] [--remove]".into()),
    }
}

/// Every public tracker on the machine, by organisation.
fn public_trackers(dir: &Path) -> Vec<(String, String, TrackerDecl)> {
    let mut out = Vec::new();
    for org in orgs_in(dir) {
        for (name, decl, _) in listed(&dir.join("orgs").join(&org).join("trackers")) {
            if decl.visibility != "private" {
                out.push((org.clone(), name, decl));
            }
        }
    }
    out
}

fn hosting_serve(args: &[String]) -> Result<(), String> {
    let dir = PathBuf::from(crate::positional(args, 2).first().ok_or("which hosting directory?")?.as_str());
    std::fs::create_dir_all(dir.join("orgs")).map_err(|e| format!("{}: {e}", dir.display()))?;
    let addr = crate::flag(args, "--addr").unwrap_or("127.0.0.1:2400").to_string();
    let accounts = crate::account::Accounts::open(&dir)?;
    let server = tiny_http::Server::http(&addr).map_err(|e| e.to_string())?;
    println!("{} organisations from {} on http://{addr}/", orgs_in(&dir).len(), dir.display());

    // Every organisation's sources, trackers and watches, one after another, as `zetlyn run`
    // does them for one workspace.
    {
        let dir = dir.clone();
        std::thread::spawn(move || loop {
            let mut soonest: Option<i64> = None;
            for org in orgs_in(&dir) {
                let root = dir.join("orgs").join(&org);
                if let Some(s) = crate::schedule_pass(&root, true, &crate::Limits::default()) {
                    soonest = Some(soonest.map_or(s, |x| x.min(s)));
                }
                if let Err(e) = publish_moved(&dir, &org) {
                    eprintln!("{org}: not published: {e}");
                }
            }
            let wait = soonest.map(|s| (s - crate::now()).clamp(60, 900)).unwrap_or(900);
            std::thread::sleep(std::time::Duration::from_secs(wait as u64));
        });
    }

    let mut apps: BTreeMap<String, App> = BTreeMap::new();
    for request in server.incoming_requests() {
        let url = request.url().to_string();
        let path = url.split('?').next().unwrap_or("/").to_string();
        let parts: Vec<String> = path.split('/').filter(|s| !s.is_empty()).map(serve::urldecode).collect();
        let first = parts.first().cloned().unwrap_or_default();
        if org_name(&first) && dir.join("orgs").join(&first).is_dir() {
            let members = Membership::load(&dir).of(&first);
            if !apps.contains_key(&first) {
                let accounts = match crate::account::Accounts::open(&dir) {
                    Ok(a) => a,
                    Err(e) => {
                        respond(request, 500, "text/plain; charset=utf-8", &e);
                        continue;
                    }
                };
                apps.insert(first.clone(), App {
                    root: dir.join("orgs").join(&first),
                    addr: addr.clone(),
                    sites: BTreeMap::new(),
                    jobs: Arc::new(Mutex::new(Jobs::default())),
                    base: format!("/{first}"),
                    hosted: Some(Hosted { members: Vec::new(), accounts, shared: true }),
                    visitor: false,
                    who: None,
                    orgs_of_who: Vec::new(),
                    public_of_machine: Vec::new(),
                });
            }
            let Some(app) = apps.get_mut(&first) else { continue };
            // Who belongs is read afresh each time: somebody added a minute ago is in now.
            if let Some(h) = app.hosted.as_mut() {
                h.members = members;
            }
            // Every organisation whoever is signed in belongs to, by its title, for the header.
            app.public_of_machine = public_links(&dir);
            app.orgs_of_who = signed_in(&request, &accounts)
                .map(|email| {
                    Membership::load(&dir)
                        .orgs_of(&email)
                        .into_iter()
                        .map(|(org, _)| {
                            let t = crate::account::Site::load(&dir.join("orgs").join(&org)).title;
                            (if t.is_empty() { org.clone() } else { t }, format!("/{org}/"))
                        })
                        .collect()
                })
                .unwrap_or_default();
            serve::mount(&app.base);
            app.answer(request);
            continue;
        }
        serve::mount("");
        hosting_root(request, &dir, &accounts, &parts);
    }
    Ok(())
}

/// The machine's own pages: what anybody can read here, signing in, and your organisations.
fn hosting_root(mut request: tiny_http::Request, dir: &Path, accounts: &crate::account::Accounts, parts: &[String]) {
    let html_kind = "text/html; charset=utf-8";
    let post = request.method() == &tiny_http::Method::Post;
    let cookie = request.headers().iter().find(|h| h.field.equiv("Cookie")).map(|h| h.value.as_str().to_string());
    let session = cookie.and_then(|c| c.split(';').filter_map(|p| p.trim().split_once('=')).find(|(k, _)| *k == "zs").map(|(_, v)| v.to_string()));
    let who = session.as_deref().and_then(|s| accounts.by_session(s));
    let membership = Membership::load(dir);
    // The app's own frame: its sidebar lists what anybody may read here, and where whoever is
    // signed in belongs.
    serve::frame_site("App");
    serve::frame_home("App", "/", public_links(dir));
    serve::frame_current(None);
    serve::frame_hosted(None, Some(who.as_ref().map(|a| a.email.clone())));
    serve::frame_area("app", None, who.as_ref().map(|a| orgs_links(dir, &a.email)).unwrap_or_default());
    serve::frame_side("Public trackers");
    serve::frame_section(None, Vec::new());
    serve::frame_app(None, None);
    match parts.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["style.css"] => respond(request, 200, "text/css; charset=utf-8", &format!("{}{APP_STYLE}", serve::STYLE)),
        [] => {
            let public = public_trackers(dir);
            let mine = who.as_ref().map(|a| membership.orgs_of(&a.email)).unwrap_or_default();
            respond(request, 200, html_kind, &page("Zetlyn", html! {
                h1 { "Trackers you can read" }
                p.about { "Every public tracker on this machine, readable without an account. Its organisation keeps it current." }
                @if public.is_empty() { p.dim { "None yet." } }
                table { tbody {
                    @for (org, name, decl) in &public {
                        tr {
                            td { a href={"/" (org) "/t/" (name) "/"} { strong { (decl.title) } } div.why { (decl.about) } }
                            td.dim { (org) }
                        }
                    }
                } }
                @match &who {
                    Some(a) => {
                        h2 { "Your organisations" }
                        @if mine.is_empty() { p.dim { (a.email) " belongs to none yet." } }
                        table { tbody {
                            @for (org, role) in &mine { tr { td { a href={"/" (org) "/"} { strong { (org) } } } td.dim { (role) } } }
                        } }
                        form.bar method="post" action="/signout" { span.dim { "Signed in as " (a.email) } button type="submit" { "Sign out" } }
                    }
                    None => {
                        p { a href="/signin" { "Sign in" } " to run your organisation's trackers." }
                    }
                }
            }));
        }
        ["signin"] if post => {
            let mut body = String::new();
            let _ = std::io::Read::read_to_string(request.as_reader(), &mut body);
            let email = parse_form(&body).get("email").cloned().unwrap_or_default().trim().to_lowercase();
            // The same words whoever asks, so the page does not say who belongs anywhere.
            if !membership.orgs_of(&email).is_empty() {
                let sent = accounts.ensure(&email).and_then(|a| accounts.new_link(a.id)).and_then(|raw| {
                    let site = crate::account::Site::load(dir);
                    let link = format!("{}/signin/{raw}", site.url.trim_end_matches('/'));
                    site.send(&email, "Your Zetlyn sign-in link", &format!("{link}\n\nGood for a quarter of an hour, and once."))
                });
                if let Err(e) = sent {
                    eprintln!("sign-in mail: {e}");
                }
            }
            respond(request, 200, html_kind, &page("Sign in", html! { h1 { "Check your mail" } p { "If that address belongs to an organisation here, a link to sign in is on its way. It is good for a quarter of an hour, and once." } }));
        }
        ["signin"] => respond(request, 200, html_kind, &page("Sign in", html! {
            h1 { "Sign in" }
            p.about { "With a link sent to your address. No password." }
            form.bar method="post" action="/signin" {
                input.wide type="email" name="email" placeholder="you@example.org" required;
                button.primary type="submit" { "Send me a link" }
            }
        })),
        ["signin", raw] => match accounts.spend_link(raw) {
            Some(session) => {
                let cookie = format!("zs={session}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=2592000");
                let mut response = tiny_http::Response::from_string("").with_status_code(303);
                for (k, v) in [("Location", "/".to_string()), ("Set-Cookie", cookie)] {
                    if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                        response = response.with_header(h);
                    }
                }
                let _ = request.respond(response);
            }
            None => respond(request, 410, html_kind, &page("Sign in", html! { h1 { "That link is spent" } p { a href="/signin" { "Ask for another" } } })),
        },
        ["signout"] if post => {
            if let Some(s) = &session {
                accounts.end_session(s);
            }
            let mut response = tiny_http::Response::from_string("").with_status_code(303);
            for (k, v) in [("Location", "/"), ("Set-Cookie", "zs=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0")] {
                if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                    response = response.with_header(h);
                }
            }
            let _ = request.respond(response);
        }
        _ => respond(request, 404, html_kind, &page("Not here", html! { h1 { "Not here" } p { a href="/" { "Every tracker on this machine" } } })),
    }
}

/// What an organisation's update moved, published where its workspace says, and the hub's pages
/// written again. A source is published when it has run since it last was; a tracker when one of
/// its sources was, or its statement is not the one it published.
fn publish_moved(dir: &Path, org: &str) -> Result<usize, String> {
    let root = dir.join("orgs").join(org);
    let Some(to) = crate::account::Site::load(&root).publish else { return Ok(0) };
    let place = crate::place::at(&to.to)?;
    let mut published: BTreeSet<String> = BTreeSet::new();
    let mut moved = 0;
    for (name, sdir) in crate::tracker::registry(&root.join("sources")) {
        let Ok(ds) = Source::open(&sdir) else { continue };
        // What arrived built belongs to whoever built it.
        if matches!(ds.decl.source, crate::sourcedecl::Fetch::Hub { .. } | crate::sourcedecl::Fetch::Package { .. }) {
            continue;
        }
        let last = ds.store.last_run().to_string();
        if ds.store.meta("published_run").as_deref() == Some(last.as_str()) {
            continue;
        }
        let wrong = ds.check();
        if !wrong.is_empty() {
            eprintln!("{org}: {name} is not published: {}", wrong.join("; "));
            continue;
        }
        match crate::artifact::publish(&ds, place.as_ref(), "latest", None) {
            Ok(v) => {
                ds.store.set_meta("published_run", &last)?;
                println!("{org}: {name}@latest is {v}");
                published.insert(name);
                moved += 1;
            }
            Err(e) => eprintln!("{org}: {name}: {e}"),
        }
    }
    for tdir in crate::tracker::scope_registry(&root.join("trackers")).values() {
        let Ok(decl) = TrackerDecl::load(tdir) else { continue };
        if decl.package.is_some() {
            continue;
        }
        let text = std::fs::read_to_string(tdir.join(crate::trackerdecl::FILE)).unwrap_or_default();
        let said = crate::place::sha256(text.as_bytes());
        let held = std::fs::read_to_string(tdir.join(".published")).unwrap_or_default();
        if held.trim() == said && !decl.members.iter().any(|m| published.contains(&m.dataset)) {
            continue;
        }
        match crate::artifact::publish_scope(tdir, &root.join("sources"), place.as_ref(), "latest", None) {
            Ok(v) => {
                let _ = std::fs::write(tdir.join(".published"), &said);
                println!("{org}: {}@latest is {v}", decl.name);
                moved += 1;
            }
            Err(e) => eprintln!("{org}: {}: {e}", decl.name),
        }
    }
    if moved > 0 {
        // Every tracker on the machine opens where it answers: its organisation, in the app.
        let mut answers: BTreeMap<String, String> = BTreeMap::new();
        if !to.app.is_empty() {
            for o in orgs_in(dir) {
                for (name, path) in crate::tracker::scope_registry(&dir.join("orgs").join(&o).join("trackers")) {
                    if let Some(d) = path.file_name() {
                        answers.insert(name, format!("{}/{o}/t/{}/", to.app.trim_end_matches('/'), d.to_string_lossy()));
                    }
                }
            }
        }
        let opens = |r: &crate::hubpages::Row| answers.get(&r.reference()).cloned();
        let n = crate::hubpages::render(place.as_ref(), &opens)?;
        println!("{org}: {n} pages in {}", place.describe());
    }
    Ok(moved)
}

/// Who a request is signed in as, on a machine where everybody signs in once.
fn signed_in(request: &tiny_http::Request, accounts: &crate::account::Accounts) -> Option<String> {
    let cookie = request.headers().iter().find(|h| h.field.equiv("Cookie"))?.value.as_str().to_string();
    let session = cookie.split(';').filter_map(|p| p.trim().split_once('=')).find(|(k, _)| *k == "zs").map(|(_, v)| v.to_string())?;
    accounts.by_session(&session).map(|a| a.email)
}

/// Every public tracker on the machine as a link: its title, and where it answers.
fn public_links(dir: &Path) -> Vec<(String, String)> {
    public_trackers(dir).into_iter().map(|(org, name, decl)| (decl.title, format!("/{org}/t/{name}/"))).collect()
}

/// The organisations somebody belongs to, by their titles, for the app's sidebar.
fn orgs_links(dir: &Path, email: &str) -> Vec<(String, String)> {
    Membership::load(dir)
        .orgs_of(email)
        .into_iter()
        .map(|(org, _)| {
            let t = crate::account::Site::load(&dir.join("orgs").join(&org)).title;
            (if t.is_empty() { org.clone() } else { t }, format!("/{org}/"))
        })
        .collect()
}
