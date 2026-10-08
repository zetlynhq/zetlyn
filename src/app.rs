//! `zetlyn` on its own: the workspace, in a browser, for the person at the machine.
//!
//! Everything here is a command this program already has, asked from a page: `source new` reads a
//! source, `source update` reads it whole, and a tracker is a file naming its sources. What this
//! adds is the order a newcomer meets them in: what do you want to track, the first source, is it
//! right, a second source, how many they share, connect. There are no accounts: whoever runs the
//! program owns what it holds, and sees every page of it. The trackers themselves are served as
//! they are published, each under `/trackers/<name>/`.

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
        "https://zetlyn.com/examples/leafline-books.csv",
        "Leafline Books",
        "What Leafline Books charges for a book, and whether it has it.",
    ),
    (
        "https://zetlyn.com/examples/lindenhof/",
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
    let mut app = App { root, addr, jobs: Arc::new(Mutex::new(Jobs::default())), base: String::new(), hosted: None, visitor: false, who: None, orgs_of_who: Vec::new(), public_of_machine: Vec::new() };
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

/// Every tracker opened in this process, by its directory, for every thread: opened once on its
/// first visit and again after it changes. A copy for each thread was eight copies of the largest
/// tracker on the machine, and more memory than the machine had (2026-10-05).
type SharedSite = Arc<Mutex<TrackerSite>>;

fn open_trackers() -> &'static Mutex<BTreeMap<PathBuf, SharedSite>> {
    static OPEN: std::sync::OnceLock<Mutex<BTreeMap<PathBuf, SharedSite>>> = std::sync::OnceLock::new();
    OPEN.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn tracker_site(dir: &Path, sources: &Path, addr: &str) -> Result<SharedSite, String> {
    if let Some(site) = open_trackers().lock().unwrap_or_else(|e| e.into_inner()).get(dir) {
        return Ok(site.clone());
    }
    // Opened outside the lock, so a tracker being opened holds up no other; two threads that
    // open the same one at once keep the first.
    let opened = Tracker::open(dir, sources).and_then(|t| TrackerSite::open(t, dir, sources, addr, true))?;
    let mut all = open_trackers().lock().unwrap_or_else(|e| e.into_inner());
    Ok(all.entry(dir.to_path_buf()).or_insert_with(|| Arc::new(Mutex::new(opened))).clone())
}

/// A tracker changed by its owner: opened afresh on its next visit.
fn forget_tracker(dir: &Path) {
    open_trackers().lock().unwrap_or_else(|e| e.into_inner()).remove(dir);
}

struct App {
    root: PathBuf,
    addr: String,
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
        // A tracker was at `t/<name>/` until 2026-10-05; asked for there, it says where it is now.
        if parts.first().map(String::as_str) == Some("t") && matches!(request.method(), tiny_http::Method::Get | tiny_http::Method::Head) {
            let rest = url.strip_prefix("/t").unwrap_or("");
            return redirect_permanently(request, &format!("{}/trackers{rest}", self.base));
        }
        let unsafe_name = |s: &String| s == "." || s == ".." || s.contains(['/', '\\', '\0']);
        let named = if parts.first().map(String::as_str) == Some("trackers") { &parts[1.min(parts.len())..2.min(parts.len())] } else { &parts[..] };
        if named.iter().any(unsafe_name) {
            return respond(request, 400, "text/plain; charset=utf-8", "not a name");
        }
        // Another site's page cannot act here: a browser says where a form came from, and a POST
        // from anywhere but this app's own pages is refused. Programs (a webhook) say nothing.
        // Except where another site's form is how it works: Apple answers a sign-in with a POST from
        // its own page, and a token is asked for by a server. Both are held by what they carry, a
        // state that was handed out here and a code with its verifier.
        let signing_in = matches!(parts.iter().map(String::as_str).collect::<Vec<_>>().as_slice(), ["oauth", "callback"] | ["oauth", "token"]);
        if request.method() == &tiny_http::Method::Post && !signing_in && from_another_site(&request) {
            return respond(request, 403, "text/plain; charset=utf-8", "a form from another site");
        }
        // What a world says about itself and what it publishes, to anybody, signed in or not.
        if matches!(request.method(), tiny_http::Method::Get | tiny_http::Method::Head) {
            match parts.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
                [".well-known", "zetlyn.json"] => {
                    return match crate::world::signed_document(&self.root) {
                        Ok(doc) => respond(request, 200, "application/json", &serde_json::to_string_pretty(&doc).unwrap_or_default()),
                        Err(e) => respond(request, 500, "application/json", &json!({ "error": e }).to_string()),
                    };
                }
                ["hub", ..] => {
                    return match crate::world::hub_file(&self.root, &parts[1..], &format!("{}/hub", self.base)) {
                        Some((bytes, kind)) => {
                            let mut response = tiny_http::Response::from_data(bytes);
                            if let Ok(h) = tiny_http::Header::from_bytes(&b"Content-Type"[..], kind.as_bytes()) {
                                response = response.with_header(h);
                            }
                            let _ = request.respond(response);
                        }
                        None => respond(request, 404, "text/plain; charset=utf-8", "nothing at that address"),
                    };
                }
                _ => {}
            }
        }
        // Signing in, with this world as the provider or as the relying party: to anybody.
        let here = crate::oidc::Here::world(&self.root, &self.base);
        let Some(request) = crate::oidc::answer(&here, request, &parts, &url) else { return };
        // The directory of other worlds this one keeps, where it keeps one.
        let request = if crate::account::Site::load(&self.root).directory {
            let Some(request) = crate::directory::answer(&self.root, request, &parts, &url) else { return };
            request
        } else {
            request
        };
        // A world that has moved answers its document (above) and sends everything else to where
        // it is now, the same page there. What is sent to it is refused, and says where to send it.
        let moved = crate::account::Site::load(&self.root).moved_to.trim().trim_end_matches('/').to_string();
        // Except for its own people: they sign in, take it with them, and say it did not move after all,
        // here. And told so for now, not for good: a browser keeps a permanent answer past an undo.
        let owners_way = self.hosted.is_none()
            || matches!(parts.first().map(String::as_str), Some("signin" | "signout" | "settings" | "export.tar.gz" | "style.css" | "zetlyn.css"))
            || self.member_signed_in(&request);
        if !moved.is_empty() && !owners_way {
            let query = url.split_once('?').map(|(_, q)| format!("?{q}")).unwrap_or_default();
            let there = format!("{moved}/{}{query}", parts.iter().map(|p| urlencode(p)).collect::<Vec<_>>().join("/"));
            if matches!(request.method(), tiny_http::Method::Get | tiny_http::Method::Head) {
                let mut response = tiny_http::Response::from_string("").with_status_code(302);
                if let Ok(h) = tiny_http::Header::from_bytes(&b"Location"[..], there.as_bytes()) {
                    response = response.with_header(h);
                }
                let _ = request.respond(response);
            } else {
                respond(request, 410, "text/plain; charset=utf-8", &format!("This world has moved. Send it to {there}\n"));
            }
            return;
        }
        if self.hosted.is_some() {
            if let Some(request) = self.hosted_gate(request, &url, &path, &parts) {
                return self.answer_as_owner(request, url, path, parts);
            }
            return;
        }
        self.answer_as_owner(request, url, path, parts)
    }

    /// Whether a member of this hosted world is signed in on this request.
    fn member_signed_in(&self, request: &tiny_http::Request) -> bool {
        let Some(h) = self.hosted.as_ref() else { return false };
        let cookie = request.headers().iter().find(|x| x.field.equiv("Cookie")).map(|x| x.value.as_str().to_string()).unwrap_or_default();
        cookie
            .split(';')
            .filter_map(|p| p.trim().split_once('='))
            .filter(|(k, _)| *k == "zs")
            .filter_map(|(_, v)| h.accounts.by_session(v, crate::account::Kind::Member).map(|a| a.email).or_else(|| crate::account::remote_member(v)))
            .any(|e| h.is_member(&e))
    }

    /// What anybody may reach on a hosted workspace, and whether this request is its owner's. The
    /// request comes back where the owner's app should answer it; otherwise it has been answered.
    fn hosted_gate(&mut self, mut request: tiny_http::Request, url: &str, path: &str, parts: &[String]) -> Option<tiny_http::Request> {
        let h = self.hosted.as_ref()?;
        let header = |name: &'static str| request.headers().iter().find(|x| x.field.equiv(name)).map(|x| x.value.as_str().to_string());
        let (cookie, signature) = (header("Cookie"), header("X-Hub-Signature-256").or_else(|| header("X-Zetlyn-Signature")));
        let session = cookie.and_then(|c| c.split(';').filter_map(|p| p.trim().split_once('=')).find(|(k, _)| *k == "zs").map(|(_, v)| v.to_string()));
        let signed_in = session.and_then(|s| h.accounts.by_session(&s, crate::account::Kind::Member).map(|a| a.email).or_else(|| crate::account::remote_member(&s)));
        let owner = signed_in.as_deref().is_some_and(|e| h.is_member(e));
        self.who = signed_in;
        let post = request.method() == &tiny_http::Method::Post;
        let html_kind = "text/html; charset=utf-8";
        match parts.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["style.css" | "zetlyn.css"] => Some(request),
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
            // In a cell, signing in and out is the main server's, for everything on zetlyn.com at once:
            // the organisation sends people there and is sent back to.
            ["signin", ..] | ["signout"] if crate::account::remote_identity() => {
                let origin = crate::account::identity_origin().unwrap_or_default();
                let next = serve::params(url).get("next").and_then(|n| crate::servetracker::next_of(n)).unwrap_or_else(|| "/".to_string());
                let to = if parts.first().map(String::as_str) == Some("signout") { "signout" } else { "signin" };
                // Signed in, back where it was asked; signed out, to the front page of zetlyn.com.
                let back = if to == "signout" { "/".to_string() } else { format!("{}{next}", self.base) };
                let mut response = tiny_http::Response::from_string("").with_status_code(303);
                for (k, v) in [("Location".to_string(), format!("{origin}/account/{to}?next={}", urlencode(&back))), ("Set-Cookie".to_string(), crate::servetracker::reader_cookie(&crate::account::Site::for_workspace(&self.root), "", 0))] {
                    if let Ok(hd) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                        response = response.with_header(hd);
                    }
                }
                let _ = request.respond(response);
                None
            }
            // One sign-in for everybody in this world, at its own address: a reader, and whoever
            // `access:` or the machine's members name, who is signed in as that too by the same link.
            ["signin"] if post => {
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut body);
                let form = parse_form(&body);
                let email = form.get("email").cloned().unwrap_or_default().trim().to_lowercase();
                let next = form.get("next").and_then(|n| crate::servetracker::next_of(n)).map(|n| format!("?next={}", urlencode(&n))).unwrap_or_default();
                let site = crate::account::Site::for_workspace(&self.root);
                let sent = crate::account::Accounts::open(&self.root).and_then(|readers| {
                    let raw = readers.ensure(&email).and_then(|a| readers.new_link(a.id))?;
                    let link = site.link(&serve::at(&format!("/signin/{raw}{next}")));
                    { let (subject, text) = crate::mail::signin_letter(&link); site.send(&email, &subject, &text) }
                });
                if let Err(e) = sent {
                    eprintln!("sign-in mail: {e}");
                }
                respond(request, 200, html_kind, &page("Sign in", html! { h1 { "Check your mail" } p { "A link to sign in is on its way to " (email) ". It is good for a quarter of an hour, and once." } }));
                None
            }
            ["signin"] => {
                let site = crate::account::Site::for_workspace(&self.root);
                let next = serve::params(url).get("next").and_then(|n| crate::servetracker::next_of(n));
                respond(request, 200, html_kind, &crate::servetracker::signin_page_to(&site, None, next.as_deref()));
                None
            }
            ["signin", raw] => {
                let site = crate::account::Site::for_workspace(&self.root);
                let readers = crate::account::Accounts::open(&self.root).ok();
                let session = readers.as_ref().and_then(|r| r.spend_link(raw, crate::account::Kind::Reader));
                let Some(session) = session else {
                    respond(request, 410, html_kind, &page("Sign in", html! { h1 { "That link is spent" } p { "It was used, or it is older than a quarter of an hour. " a href=(serve::at("/signin")) { "Ask for another" } } }));
                    return None;
                };
                let email = readers.as_ref().and_then(|r| r.by_session(&session, crate::account::Kind::Reader)).map(|a| a.email).unwrap_or_default();
                let mut cookies: Vec<String> = Vec::new();
                // Somebody who runs it is signed in for that too: on the machine, for every world
                // they belong to; on its own, for this one.
                if h.is_member(&email) {
                    let path = if h.shared || self.base.is_empty() { "/".to_string() } else { self.base.clone() };
                    if let Ok(member) = h.accounts.ensure(&email).and_then(|a| h.accounts.new_session(a.id, crate::account::Kind::Member)) {
                        cookies.push(format!("zs={member}; Path={path}; HttpOnly; Secure; SameSite=Lax; Max-Age=2592000"));
                    }
                }
                cookies.push(crate::servetracker::reader_cookie(&site, &session, 2_592_000));
                let next = serve::params(url).get("next").and_then(|n| crate::servetracker::next_of(n)).unwrap_or_else(|| "/".to_string());
                let mut response = tiny_http::Response::from_string("").with_status_code(303);
                if let Ok(hd) = tiny_http::Header::from_bytes(&b"Location"[..], serve::at(&next).as_bytes()) {
                    response = response.with_header(hd);
                }
                for c in cookies {
                    if let Ok(hd) = tiny_http::Header::from_bytes(&b"Set-Cookie"[..], c.as_bytes()) {
                        response = response.with_header(hd);
                    }
                }
                let _ = request.respond(response);
                None
            }
            ["signout"] => {
                let site = crate::account::Site::for_workspace(&self.root);
                let path = if h.shared || self.base.is_empty() { "/".to_string() } else { self.base.clone() };
                let mut response = tiny_http::Response::from_string("").with_status_code(303);
                for (k, v) in [
                    ("Location".to_string(), serve::at("/")),
                    ("Set-Cookie".to_string(), crate::servetracker::reader_cookie(&site, "", 0)),
                    ("Set-Cookie".to_string(), format!("zs=; Path={path}; HttpOnly; Secure; SameSite=Lax; Max-Age=0")),
                ] {
                    if let Ok(hd) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                        response = response.with_header(hd);
                    }
                }
                let _ = request.respond(response);
                None
            }
            // A tracker's own sign-in was where a reader signed in until 2026-10-06; it is the
            // world's now. A link it mailed before is still spent there.
            ["trackers", tracker, "signin"] if !post => {
                // Where the tracker would have sent them back to, within the tracker.
                let within = serve::params(url).get("next").and_then(|n| crate::servetracker::next_of(n)).unwrap_or_else(|| "/".to_string());
                let next = format!("/trackers/{tracker}{within}");
                redirect(request, &serve::at(&format!("/signin?next={}", urlencode(&next))));
                None
            }
            // The trackers answer for themselves, the owner as their operator.
            ["trackers", ..] => {
                self.visitor = !owner;
                Some(request)
            }
            // A source of the world, for anybody where it may be shown; the rest of it is its owners'.
            ["sources", name] | ["sources", name, ""] if !owner && !post => {
                if shown_source(&self.root, name).is_some() {
                    self.visitor = true;
                    Some(request)
                } else {
                    redirect(request, &serve::at("/signin"));
                    None
                }
            }
            // What the world is, where it went, all of it at once: its owners', not every editor's.
            ["export.tar.gz"] | ["settings", "moved"] | ["settings", "access"] | ["settings", "seen"] | ["settings", "licence", _] | ["publish", _] | ["assist"] if owner && (post || parts.len() == 1 && parts[0] == "export.tar.gz") && !self.who.as_deref().is_some_and(|e| h.is_owner(e)) => {
                respond(request, 403, html_kind, &page("Owners only", html! { h1 { "Only an owner of this world changes that" } p { a href=(serve::at("/")) { "Back" } } }));
                None
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
                    tr { td { a href=(serve::at(&format!("/trackers/{name}/"))) { strong { (decl.title) } } @if decl.visibility == "private" { " " span.chip { "private" } } div.why { (decl.about) } } }
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
        // On zetlyn.com an organisation's pages wear the website's header and footer, under
        // the machine they are part of, with who is signed in at the right.
        if self.hosted.as_ref().is_some_and(|h| h.shared) {
            let title = crate::account::Site::load(&self.root).title;
            let title = if title.is_empty() { "Organisation".to_string() } else { title };
            // A member's links are the organisation's own; a reader's are what anybody may read.
            let (links, current) = if self.visitor {
                // A reader's sidebar is what anybody may read on the machine, this one marked.
                let here = match parts.as_slice() {
                    [t, name, ..] if t == "trackers" => self.public_of_machine.iter().find(|(_, h)| h.ends_with(&format!("/trackers/{name}/"))).map(|(l, _)| l.clone()),
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
            // A world is at /<name>/ on the machine as it would be at the root of a domain of its own,
            // and nothing is above it in either.
            serve::frame_hosted(None, Some(self.who.clone()));
            serve::frame_area("app", Some(title.clone()), self.orgs_of_who.clone());
            serve::frame_side(if self.visitor { "Public trackers" } else { "" });
            // Beside them, what anybody may read of this world's sources, each at its own page.
            let sources: Vec<(String, String)> = if self.visitor {
                shown_sources(&self.root).into_iter().map(|s| (s.title, format!("{}/sources/{}/", self.base, s.short))).collect()
            } else {
                Vec::new()
            };
            let here = format!("/{}/", parts.join("/"));
            serve::frame_side_more(if sources.is_empty() { Vec::new() } else { vec![("Public sources".to_string(), sources)] }, Some(format!("{}{here}", self.base)));
        } else {
            serve::frame_area("", None, Vec::new());
            serve::frame_side_more(Vec::new(), None);
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
        // A tracker's pages are under the world's trackers, as the address says (trackers/<name>/).
        serve::frame_group((parts.first().map(String::as_str) == Some("trackers")).then(|| ("Trackers".to_string(), format!("{}/", self.base))));
        serve::frame_section(None, Vec::new());
        // A source's own page, for a visitor: under the world's sources, which is no page of its own.
        if self.visitor && parts.first().map(String::as_str) == Some("sources") && parts.len() >= 2 {
            serve::frame_group(Some(("Sources".to_string(), String::new())));
            match public_source_page(&self.root, &self.base, &parts[1]) {
                Some(body) => respond(request, 200, "text/html; charset=utf-8", &body),
                None => redirect(request, &serve::at("/signin")),
            }
            return;
        }
        if let [first, tracker, ..] = parts.as_slice() {
            if first != "trackers" && first != "job" {
                let dir = self.trackers().join(tracker);
                let title = TrackerDecl::load(&dir).ok().map(|d| d.title).or_else(|| draft_title(&dir));
                if let Some(title) = title {
                    let href = if dir.join("tracker.yaml").exists() { format!("{}/trackers/{tracker}/", self.base) } else { format!("{}/new/{tracker}?title={}", self.base, urlencode(&title)) };
                    serve::frame_section(Some((title, href)), Vec::new());
                }
            }
        }

        // A tracker's own pages, as a reader would see them published.
        if parts.first().map(String::as_str) == Some("trackers") && parts.len() >= 2 {
            let name = parts[1].clone();
            let dir = self.trackers().join(&name);
            let site = match tracker_site(&dir, &self.sources(), &self.addr) {
                Ok(site) => site,
                Err(e) => return respond(request, 404, "text/html; charset=utf-8", &page("Not here", html! { p { (e) } })),
            };
            serve::mount(&format!("{}/trackers/{name}", self.base));
            // One request at a time for each tracker, whichever thread it came in on: what a
            // tracker holds open is held once, and its expensive pages cannot run eight at once.
            let mut site = site.lock().unwrap_or_else(|e| e.into_inner());
            site.set_operator(!self.visitor);
            site.answer(request);
            drop(site);
            serve::mount(&self.base);
            return;
        }

        // A world brought here from elsewhere, uploaded by an owner: kept for the server, which
        // takes a snapshot, puts it in place of this one and starts it again (cell.rs). Its own
        // method, so the upload is streamed to a file and never held as a form.
        if request.method() == &tiny_http::Method::Put && parts.len() == 2 && parts[0] == "settings" && parts[1] == "import" {
            let owner = self.hosted.as_ref().zip(self.who.as_deref()).is_some_and(|(h, e)| h.is_owner(e));
            let (code, said) = if !owner {
                (403, "Only an owner of this world brings another one here.".to_string())
            } else {
                let by = self.who.clone().unwrap_or_default();
                match crate::cell::take_upload(request.as_reader(), &by) {
                    Ok(s) => (200, s),
                    Err(e) => (400, e),
                }
            };
            return respond(request, code, "text/plain; charset=utf-8", &said);
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
            (false, ["style.css" | "zetlyn.css"]) => (200, "text/css; charset=utf-8", format!("{}{APP_STYLE}", serve::STYLE)),
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
                let url = query.get("url").cloned().unwrap_or_default();
                if let Err(e) = self.may_read(&url) {
                    (400, json_kind, json!({ "error": e }).to_string())
                } else {
                    let id = self.read_web(tracker, query.get("url").cloned().unwrap_or_default(), form_title(&query), pick);
                (200, json_kind, json!({ "job": id }).to_string())
                }
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
                Ok(slug) => return redirect(request, &serve::at(&format!("/proposals/{slug}?said={}", urlencode("Made. Anybody signed in to its published pages may propose to it; keys you invite may too.")))),
                Err(e) => return redirect(request, &serve::at(&format!("/proposals?said={}", urlencode(&e)))),
            },
            (true, ["proposals", source, "readers"]) => {
                let readers: Vec<String> = match form.get("readers").map(String::as_str).unwrap_or("") {
                    "signed-in" => vec![crate::propose::SIGNED_IN.to_string()],
                    "listed" => form.get("addresses").map(String::as_str).unwrap_or("").split([',', '\n', ' ']).map(|a| a.trim().to_lowercase()).filter(|a| a.contains('@') || a.strip_prefix("domain:").is_some_and(|d| d.contains('.'))).collect(),
                    _ => Vec::new(),
                };
                let said = match crate::propose::set_readers(&self.sources().join(source), readers.clone()) {
                    Ok(()) if readers.is_empty() => "Readers can no longer propose here. What they proposed before stays, with its decisions.".to_string(),
                    Ok(()) if readers.iter().any(|r| r == crate::propose::SIGNED_IN) => "Anybody signed in to the published pages may propose here now.".to_string(),
                    Ok(()) => format!("{} may propose here now.", readers.join(", ")),
                    Err(e) => e,
                };
                return redirect(request, &serve::at(&format!("/proposals/{source}?said={}", urlencode(&said))));
            }
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
                let why = form.get("why").map(String::as_str).unwrap_or("");
                match crate::propose::decide(&dir, name, *verb == "accept", &by, why) {
                    Ok(()) => {
                        // Read now, as a push is, so what was accepted is a claim before the page comes back.
                        if let Ok(ds) = Source::open(&dir) {
                            let _ = ds.run();
                        }
                        crate::propose::tell_proposer(&self.root, &dir, name, *verb == "accept", why);
                        return redirect(request, &serve::at(&format!("/proposals/{source}")));
                    }
                    Err(e) => (400, html_kind, page("Not decided", html! { p { (e) } p { a href=(serve::at(&format!("/proposals/{source}"))) { "Back to the proposals" } } })),
                }
            }
            (false, ["settings"]) => (200, html_kind, self.settings_page(&query)),
            // All of it, as one archive, for its owner to take away. Made and sent on a thread of its
            // own: it takes most of a minute for a large world and as long as the download takes
            // after, and everybody else is answered meanwhile. The thread ends when the download does.
            (false, ["export.tar.gz"]) => {
                let root = self.root.clone();
                std::thread::spawn(move || {
                    let file = std::env::temp_dir().join(format!("zetlyn-export-{}.tar.gz", crate::jwt::random()));
                    match crate::world::export(&root, &file).and_then(|_| std::fs::File::open(&file).map_err(|e| e.to_string())) {
                        Ok(handle) => {
                            // Read from the open handle; the name is gone at once, so nothing is left behind.
                            let _ = std::fs::remove_file(&file);
                            let name = format!("{}-{}.tar.gz", root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "world".into()), crate::iso_date(crate::now()));
                            let mut response = tiny_http::Response::from_file(handle);
                            for (k, v) in [("Content-Type", "application/gzip".to_string()), ("Content-Disposition", format!("attachment; filename=\"{name}\""))] {
                                if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                                    response = response.with_header(h);
                                }
                            }
                            let _ = request.respond(response);
                        }
                        Err(e) => {
                            let _ = std::fs::remove_file(&file);
                            respond(request, 500, "text/plain; charset=utf-8", &format!("Not exported: {e}\n"));
                        }
                    }
                });
                return;
            }
            // Who may do what here, from the three lists of the form, one word to a line.
            (true, ["settings", "access"]) => {
                let list = |k: &str| -> Vec<String> { form.get(k).map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()).unwrap_or_default() };
                let access = crate::account::Access { owners: list("owners"), editors: list("editors"), proposers: list("proposers") };
                let wrong: Vec<String> = access.owners.iter().chain(&access.editors).chain(&access.proposers).filter_map(|p| crate::account::pattern_problem(p)).collect();
                let me = self.who.clone().unwrap_or_default();
                let site = crate::account::Site::load(&self.root);
                let mut owners_after = site.owners.clone();
                owners_after.extend(access.owners.iter().cloned());
                // An owner the machine names in members.yaml stays one whatever this list says.
                let machine_owner = self.root.parent().and_then(Path::parent).zip(self.root.file_name()).is_some_and(|(m, org)| Membership::load(m).of(&org.to_string_lossy(), &["owner"]).iter().any(|e| e.eq_ignore_ascii_case(&me)));
                let said = if !wrong.is_empty() {
                    format!("Not saved. {}", wrong.join("; "))
                } else if !machine_owner && !crate::account::admits(&owners_after, &me, &[]) {
                    "Not saved: you would no longer be an owner of this world, and nobody could give it back from here.".to_string()
                } else {
                    match crate::account::set_access(&self.root, &access) {
                        Ok(()) => "Saved. Who may do what here is as the lists say now.".to_string(),
                        Err(e) => e,
                    }
                };
                return redirect(request, &serve::at(&format!("/settings?saved={}#access", urlencode(&said))));
            }
            (true, ["settings", "moved"]) => {
                let to = form.get("to").cloned().unwrap_or_default();
                let said = match if to.trim().is_empty() { crate::world::stay(&self.root) } else { crate::world::move_to(&self.root, &to, false) } {
                    Ok(()) if to.trim().is_empty() => "This world answers here again.".to_string(),
                    Ok(()) => format!("This world says it lives at {to} now."),
                    Err(e) => e,
                };
                return redirect(request, &serve::at(&format!("/settings?saved={}", urlencode(&said))));
            }
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
            // A tracker public or private, from Who sees what.
            (true, ["settings", "seen"]) => {
                let tracker = form.get("tracker").cloned().unwrap_or_default();
                let said = match form.get("visibility").map(String::as_str) {
                    Some(v @ ("public" | "private")) if self.trackers().join(&tracker).join(crate::trackerdecl::FILE).exists() && !tracker.contains(['/', '.']) => self.set_visibility(&tracker, v),
                    _ => Err("Not changed: which tracker, public or private?".into()),
                };
                forget_tracker(&self.trackers().join(&tracker));
                let said = said.map(|s| format!("{tracker}: {s}")).unwrap_or_else(|e| e);
                return redirect(request, &serve::at(&format!("/settings?seen={}#seen", urlencode(&said))));
            }
            // What a source lets a public page show of it: `licence: { republish }` in its file.
            (true, ["settings", "licence", slug]) => {
                let dir = self.sources().join(slug);
                let republish = form.get("republish").map(String::as_str).filter(|r| matches!(*r, "yes" | "summary" | "no"));
                let said = (|| -> Result<String, String> {
                    let republish = republish.ok_or("in full, titles and values, or not at all")?;
                    let mut decl = crate::sourcedecl::SourceDecl::load(&dir)?;
                    decl.licence.republish = republish.to_string();
                    let path = dir.join(crate::sourcedecl::FILE);
                    std::fs::write(&path, crate::yaml::to_string(&decl)?).map_err(|e| format!("{}: {e}", path.display()))?;
                    // Every tracker reads its sources' licences when it opens.
                    open_trackers().lock().unwrap_or_else(|e| e.into_inner()).clear();
                    Ok(format!("{}: {}.", decl.title, republish_words(republish)))
                })()
                .unwrap_or_else(|e| e);
                return redirect(request, &serve::at(&format!("/settings?seen={}#seen", urlencode(&said))));
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
                } else if let Err(e) = self.may_read(from.trim()) {
                    (400, json_kind, json!({ "error": e }).to_string())
                } else {
                    let id = self.analyse(tracker, from.trim().to_string(), form_title(&query));
                    (200, json_kind, json!({ "job": id }).to_string())
                }
            }
            (false, ["assist", tracker, source]) => (200, html_kind, self.assist_page(tracker, source, &query)),
            (true, ["teach", tracker, source]) => {
                match self.may_read(query.get("url").map(String::as_str).unwrap_or("")) {
                    Err(e) => (400, json_kind, json!({ "error": e }).to_string()),
                    Ok(()) => {
                        let id = self.teach(tracker, source, query.get("url").cloned().unwrap_or_default(), form_title(&query));
                        (200, json_kind, json!({ "job": id }).to_string())
                    }
                }
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
                forget_tracker(&self.trackers().join(tracker));
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
                        forget_tracker(&self.trackers().join(tracker));
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
    /// Whether a pasted address is one this app reads. On a person's own machine, anything,
    /// their files included. Hosted, the machine is not theirs: an address on the public
    /// internet, or a GitHub repository, and nothing on the machine or beside it.
    fn may_read(&self, from: &str) -> Result<(), String> {
        if self.hosted.is_none() || from.starts_with("github:") {
            return Ok(());
        }
        if !from.starts_with("http://") && !from.starts_with("https://") {
            return Err("an address, https://…, or a file chosen from your computer".into());
        }
        // Plain http is a source like any other; what is checked is where it points.
        crate::outbound::allowed(&from.replacen("http://", "https://", 1), false)
    }

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
                    crate::account::Site::for_workspace(&self.root).link(&serve::at(&format!("/trackers/{tracker}/")))
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
                        p.dim { "Sealed, it travels as one file on the hub at zetlyn.com: everything it answers with, every claim and its history, and not how it was made. Which source is read where, their own field names, the mappings and the receipts stay here. Signed with this machine's key (" code { "zetlyn id" } ")." }
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

    /// Who sees what, for an owner: each tracker public or private, and what each source lets a
    /// public page show of it. A source only private trackers hold is never published.
    fn seen_section(&self, query: &BTreeMap<String, String>) -> Markup {
        let trackers: Vec<(String, PathBuf, TrackerDecl)> = crate::tracker::scope_registry(&self.trackers())
            .into_iter()
            .filter_map(|(n, d)| TrackerDecl::load(&d).ok().map(|t| (n, d, t)))
            .collect();
        let sources: Vec<(String, PathBuf)> = crate::tracker::registry(&self.sources()).into_iter().collect();
        let republish_of = |name: &str| -> String {
            sources.iter().find(|(n, _)| n == name).and_then(|(_, d)| crate::sourcedecl::SourceDecl::load(d).ok()).map(|d| d.licence.republish).unwrap_or_default()
        };
        let in_public = |name: &str| trackers.iter().any(|(_, _, t)| t.visibility != "private" && t.members.iter().any(|m| m.dataset == name));
        html! {
            h2 #seen { "Who sees what" }
            p.lede { "A public tracker is open to anyone: its overview and its pages, at its address and on the hub. A private one is for the signed-in readers it is given to. "
                "Whether a source may be shown in public is yours to know from its terms: making it public says you may." }
            @if let Some(s) = query.get("seen") { div.note { (s) } }
            @if trackers.is_empty() { p.dim { "No tracker here yet." } }
            @else {
                table { thead { tr { th { "Tracker" } th { "Now" } th {} } } tbody {
                    @for (name, _, t) in &trackers {
                        @let private = t.visibility == "private";
                        @let blocked: Vec<String> = t.members.iter().map(|m| m.dataset.clone()).filter(|s| !matches!(republish_of(s).as_str(), "yes" | "summary")).collect();
                        tr {
                            td { strong { (if t.title.is_empty() { name.clone() } else { t.title.clone() }) } div.why.mono { (name) } }
                            td { @if private { span.chip { "Private" } } @else { span.chip.on { "Public" } } }
                            td {
                                form.bar method="post" action=(serve::at("/settings/seen")) {
                                    input type="hidden" name="tracker" value=(name);
                                    @if private {
                                        input type="hidden" name="visibility" value="public";
                                        button type="submit" disabled[!blocked.is_empty()] { "Make it public" }
                                    } @else {
                                        input type="hidden" name="visibility" value="private";
                                        button type="submit" { "Make it private" }
                                    }
                                }
                                @if private && !blocked.is_empty() { div.why { "First let " (blocked.join(", ")) " be shown in public, below." } }
                            }
                        }
                    }
                } }
            }
            @if !sources.is_empty() {
                table { thead { tr { th { "Source" } th { "On public pages" } th { "Published" } } } tbody {
                    @for (name, dir) in &sources {
                        @if let Ok(d) = crate::sourcedecl::SourceDecl::load(dir) {
                            @let slug = dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                            @let now = d.licence.republish.clone();
                            tr {
                                td { strong { (if d.title.is_empty() { name.clone() } else { d.title.clone() }) } div.why.mono { (name) }
                                    @if !d.licence.terms.is_empty() { div.why { a href=(d.licence.terms) rel="noopener" { "its terms" } } } }
                                td {
                                    form.bar method="post" action={(serve::at("/settings/licence/")) (slug)} {
                                        select name="republish" {
                                            @for (v, l) in [("no", "Not shown"), ("summary", "Titles, values and a link"), ("yes", "In full")] {
                                                option value=(v) selected[now == v || (now.is_empty() && v == "no")] { (l) }
                                            }
                                        }
                                        button type="submit" { "Set" }
                                    }
                                }
                                td { @if in_public(name) && matches!(now.as_str(), "yes" | "summary") { "with its public trackers" } @else { span.dim { "no" } } }
                            }
                        }
                    }
                } }
            }
        }
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
            @if self.hosted.is_some() {
                p.dim { "Kept beside this world's workspace, as " code { "assist-anthropic.key" } ", for this world alone, and in its export." }
            } @else {
                p.dim { "Kept in " code { "~/.zetlyn/assist/anthropic.key" } ", readable by you alone. " code { "ANTHROPIC_API_KEY" } " works as well." }
            }
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
                // Hosted, the key is this world's, beside its workspace, and not the machine's for every world on it.
                crate::assist::keep_key("anthropic", form.get("key").map(String::as_str).unwrap_or(""), self.hosted.is_some().then_some(self.root.as_path()))?;
                write("assist:\n  provider: anthropic\n")?;
                Ok("The key is kept. The assist asks Claude from now on.".into())
            }
            Some("openai") => {
                let url = form.get("url").map(|s| s.trim()).unwrap_or("");
                let model = form.get("model").map(|s| s.trim()).unwrap_or("");
                if !url.starts_with("http") || model.is_empty() {
                    return Err("an address, http… ending in /v1, and the name of a model it serves".into());
                }
                // Hosted, the machine asks it, and a machine is not asked to talk to its own insides.
                if self.hosted.is_some() {
                    crate::outbound::allowed(url, false)?;
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
            (cell_usage_section(&sources))
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
            // Leaving is part of what a world is: all of it, and saying where it went.
            h2 { "Taking it with you" }
            p.dim { "Everything this world holds in one archive: its sources and their history, its trackers, its readers and what they proposed, its keys. "
                code { "zetlyn world up <domain> --owner <you> --from <archive>" } " makes it again on a machine of yours." }
            p { a.chip href=(serve::at("/export.tar.gz")) { "Download all of it" } }
            @if crate::usage::in_cell() && self.hosted.as_ref().zip(self.who.as_deref()).is_some_and(|(h, e)| h.is_owner(e)) {
                @let (waiting, last) = crate::cell::import_state();
                h2 #import { "Bringing a world here" }
                p.dim { "A world from your own machine, or from another Zetlyn, takes the place of this one: its sources and their history, its trackers, its readers and their proposals. On your machine, "
                    code { "zetlyn world export <workspace> --to <file>.tar.gz" } " makes the archive; another Zetlyn's " em { "Download all of it" } " does too. "
                    "A snapshot of this world is taken first. Who may do what here stays as it is, and the address stays " code { "zetlyn.com/…" } "." }
                @if let Some(l) = &last {
                    div.note {
                        @if l["ok"].as_bool() == Some(true) { "Imported on " (l["at"].as_str().unwrap_or("")) ", " (l["files"]) " files." }
                        @else { "The import on " (l["at"].as_str().unwrap_or("")) " did not go through: " (l["error"].as_str().unwrap_or("")) ". Nothing changed." }
                    }
                }
                @if waiting {
                    div.note { "An upload is waiting; within a minute it takes this world's place, and a mail says when it is done." }
                } @else {
                    form.bar #import-form data-to=(serve::at("/settings/import")) {
                        input #import-file type="file" accept=".gz,.tgz,application/gzip" required;
                        button.primary type="submit" { "Upload and replace this world" }
                    }
                    p #import-said .dim {}
                    script { (PreEscaped(IMPORT_SCRIPT)) }
                }
            }
            @let moved = crate::account::Site::load(&self.root).moved_to;
            @if moved.is_empty() {
                p.dim { "Once it answers at its new address, say so here: this one then says where it went, and sends everybody there." }
                form.bar method="post" action=(serve::at("/settings/moved")) {
                    input.wide type="url" name="to" placeholder="https://your-world.example" required;
                    button type="submit" { "It lives there now" }
                }
            } @else {
                div.note { "This world lives at " a href=(moved) { (moved) } " now, and sends everybody there." }
                form.bar method="post" action=(serve::at("/settings/moved")) {
                    input type="hidden" name="to" value="";
                    button type="submit" { "It did not move: answer here again" }
                }
            }
            @if self.hosted.as_ref().is_none_or(|h| self.who.as_deref().is_some_and(|e| h.is_owner(e))) {
                (self.seen_section(query))
            }
            @if self.hosted.is_some() {
                @let access = crate::account::Site::load(&self.root).access;
                @let lines = |l: &[String]| l.join("\n");
                h2 #access { "Who may do what" }
                p.lede { "Everybody reads what this world makes public. These lists say who may do more. One to a line: an address, "
                    code { "domain:example.org" } " for every confirmed address there, " code { "@zetlyn.com" } " for whoever that world vouches for, or "
                    code { "signed-in" } " for anybody signed in." }
                @if let Some(s) = query.get("saved").filter(|s| s.contains("lists") || s.starts_with("Not saved")) { div.note { (s) } }
                form.settings method="post" action=(serve::at("/settings/access")) {
                    p { label { strong { "Owners" } span.dim { " · everything, its settings and its export too" } br;
                        textarea.wide name="owners" rows="3" { (lines(&access.owners)) } } }
                    p { label { strong { "Editors" } span.dim { " · its sources and trackers, and deciding proposals" } br;
                        textarea.wide name="editors" rows="3" { (lines(&access.editors)) } } }
                    p { label { strong { "Proposers" } span.dim { " · rows for every source that names no readers of its own" } br;
                        textarea.wide name="proposers" rows="3" { (lines(&access.proposers)) } } }
                    p { button.primary type="submit" { "Save who may do what" } }
                }
                p.dim { "Only an owner changes these. Whoever this machine's own list of members names counts besides; an owner named there stays one whatever these lists say. Another world, zetlyn.com among them, only says who somebody is: what they may do here, this world says." }
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
            Ok(name) => {
                crate::propose::tell_owner(&self.root, &dir, &name);
                (202, json!({ "kept": name, "waiting": "for the source's owner" }).to_string())
            }
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
            "name: {}\ntitle: {}\nkind: observation\nabout: {}\nfetch:\n  type: proposals\n  from: []\n  readers: [signed-in]\nclaims:\n  id:\n    scheme: {}\n    from: {}\n  title: {}\n  known: field:read_at\n  properties:\n",
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
        let crate::sourcedecl::Fetch::Proposals { from, .. } = &mut decl.source else {
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
        let crate::sourcedecl::Fetch::Proposals { from, readers } = &decl.source else {
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
            @let everybody = readers.iter().any(|r| r == crate::propose::SIGNED_IN);
            @let listed: Vec<&String> = readers.iter().filter(|r| r.as_str() != crate::propose::SIGNED_IN).collect();
            details open[!readers.is_empty()] {
                summary { "Readers who may propose from the browser: "
                    (if everybody { "anybody signed in".to_string() } else if listed.is_empty() { "nobody".to_string() } else { format!("{} listed", listed.len()) }) }
                p.dim { "A reader of the published pages signs in with a link sent to their address and proposes through a form; this workspace signs for them. Their proposals name a pseudonym, never the address, and they are mailed what you decide." }
                form.settings method="post" action=(serve::at(&format!("/proposals/{source}/readers"))) {
                    p { label { input type="radio" name="readers" value="none" checked[readers.is_empty()]; " Nobody" } }
                    p { label { input type="radio" name="readers" value="signed-in" checked[everybody]; " Anybody signed in" } }
                    p { label { input type="radio" name="readers" value="listed" checked[!everybody && !listed.is_empty()]; " Only these: addresses, " code { "domain:example.org" } " for everybody whose verified address is there, " code { "@zetlyn.com" } " for everybody signed in through that world" } br;
                        input.wide type="text" name="addresses" placeholder="ann@example.org, domain:example.org, @zetlyn.com" value=(listed.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")); }
                    p { button.primary type="submit" { "Save" } }
                }
            }
            details {
                summary { "How a proposer sends a row" }
                p { "A file, " code { "row.json" } ", with the row and how it was read:" }
                pre { (format!("{{\n  \"row\": {{ … the fields of one row … }},\n  \"read_at\": \"{}\",\n  \"read_from\": \"https://… where it was read\",\n  \"attest\": \"read\",\n  \"note\": \"optional\"\n}}", crate::iso_stamp(crate::now()).get(..10).unwrap_or(""))) }
                p { "Then, signed with their own key:" }
                pre { "zetlyn source propose " (crate::account::Site::for_workspace(&self.root).link(&serve::at(&format!("/propose/{source}")))) " row.json" }
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
                                "read " (e.read_at) " from " @if e.read_from.starts_with("https://") || e.read_from.starts_with("http://") { a href=(e.read_from) { (e.read_from) } } @else { (e.read_from) } " · " (if e.attest == "read" { "read by the proposer" } else { "relayed" })
                                @if !e.note.is_empty() { " · " (e.note) }
                            }
                            div.why {
                                "proposed by " @if e.via == "browser" { strong { (if e.name.is_empty() { "a reader" } else { e.name.as_str() }) } " " code { (e.by) } " · signed in, from the browser" } @else { code title=(e.by) { (short(&e.by)) } } " · arrived " (e.received)
                                @if !e.verified { " · " strong { "its signature does not verify: the file was changed after it arrived" } } @if !e.agreeing.is_empty() { " · " strong { (e.agreeing.len()) (if e.agreeing.len() == 1 { " other key says the same" } else { " other keys say the same" }) } }
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
                            td { a href={(serve::at("/trackers/")) (name) "/"} { strong { (decl.title) } } div.why { (decl.members.len()) (if decl.members.len() == 1 { " source" } else { " sources" }) " · identified by " (decl.join.join(", ")) } }
                            td.num { @match unseen.get(name).copied().unwrap_or(0) { 0 => span.dim { "nothing new since you looked" }, n => a.chip.on href={(serve::at("/trackers/")) (name) "/changes"} { (n) " new since you looked" } } }
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
            // Run for somebody, where its files are is ours; on their own machine, it is theirs to know.
            @if self.hosted.is_some() { p.dim { "Your organisation runs for you on Zetlyn's servers. You can download all of it from " a href=(serve::at("/settings")) { "Settings" } "." } } @else {
            p.dim { "The workspace is " code { (self.root.display()) } ". Everything here is a file in it. " a href=(serve::at("/assist")) { @if crate::assist::Assist::configured(&self.root).available() { "The assist asks " (crate::assist::Assist::configured(&self.root).who()) } @else { "No model is set up, and none is needed to begin" } } "." } }
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
                    a.button href={(serve::at("/trackers/")) (tracker) "/"} { "Open the tracker" }
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
                    form method="post" action={(serve::at("/settings?back=")) (urlencode(&serve::at(&format!("/trackers/{tracker}/"))))} {
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
                            a href={(serve::at("/trackers/")) (tracker) "/thing/" (urlencode(scheme)) "/" (urlencode(value))} { (value) }
                        } ", each with two perspectives." }
                    }
                }
            }

            @if joined {
                div.note { @if added { "Connected. " } (ds.decl.title) " is part of " a href={(serve::at("/trackers/")) (tracker) "/"} { (title) } "." }
                p.bar {
                    a.chip href={(serve::at("/trackers/")) (tracker) "/"} { "Open the tracker" }
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

/// An address that moved for good: a browser and a search engine remember where it went.
fn redirect_permanently(request: tiny_http::Request, to: &str) {
    let mut response = tiny_http::Response::from_string("").with_status_code(301);
    if let Ok(h) = tiny_http::Header::from_bytes(&b"Location"[..], to.as_bytes()) {
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
.order { max-width: 64rem; margin: 2rem 0 3rem; }
.order h1 { margin: .6rem 0 1.4rem; font-size: 2rem; }
.order-steps { display: flex; gap: 1.6rem; list-style: none; padding: 0; margin: 0; font-family: var(--mono); font-size: .78rem; letter-spacing: .06em; text-transform: uppercase; color: var(--dim); }
.order-steps li.on { color: var(--accent); font-weight: 600; }
.order-grid { display: grid; grid-template-columns: minmax(0, 1.4fr) minmax(16rem, 1fr); gap: 2rem; align-items: start; }
@media (max-width: 52rem) { .order-grid { grid-template-columns: 1fr; } .order-summary { order: -1; } }
.order-form { display: flex; flex-direction: column; gap: 1.2rem; }
.order-form label.field { display: flex; flex-direction: column; gap: .35rem; }
.order-form .field-name { font-weight: 600; }
.order-form input { flex: none; }
.order-form input[type=text], .order-form input[type=email] { padding: .7rem .8rem; font: inherit; background: var(--panel); color: var(--fg); border: 1px solid var(--line); border-radius: 0; }
.order-form input:focus { outline: 2px solid var(--accent); outline-offset: -1px; }
.order-form .slug { display: flex; align-items: stretch; border: 1px solid var(--line); background: var(--panel); }
.order-form .slug-base { padding: .7rem 0 .7rem .8rem; color: var(--dim); font-family: var(--mono); }
.order-form .slug input { border: 0 !important; padding-left: .1rem !important; font-family: var(--mono) !important; flex: 1 1 auto; min-width: 0; outline: none; }
.order-form .slug:focus-within { outline: 2px solid var(--accent); outline-offset: -1px; }
.slug-state { font-size: .88rem; min-height: 1.2em; }
.slug-state.ok { color: #2f7d4a; }
.slug-state.no { color: var(--accent); }
.order-form label.check { display: flex; gap: .6rem; align-items: flex-start; font-size: .94rem; }
.order-form label.check input { margin-top: .25rem; }
.order-button { align-self: flex-start; padding: .8rem 1.6rem; font-size: 1rem; }
.order-summary { background: var(--panel); border: 1px solid var(--line); padding: 1.3rem 1.4rem; display: flex; flex-direction: column; gap: .5rem; }
.order-price { margin: 0; font-size: 1.05rem; }
.order-price strong { font-size: 2rem; letter-spacing: -.02em; }
.order-included { list-style: none; padding: .6rem 0; margin: 0; border-top: 1px solid var(--line); border-bottom: 1px solid var(--line); display: flex; flex-direction: column; gap: .3rem; }
.order-included li::before { content: "\2713\00a0\00a0"; color: var(--accent); }
.order-beyond-title { margin: .4rem 0 0; font-weight: 600; font-size: .92rem; }
table.order-beyond { width: 100%; font-size: .9rem; margin: 0; }
table.order-beyond td { padding: .2rem 0; border: 0; }
table.order-beyond td + td { text-align: right; }
.account-hero { margin: 2.5rem 0 1.5rem; }
.account-hero h1 { margin: .2rem 0 .4rem; font-size: 1.9rem; word-break: break-all; }
.account-worlds { display: grid; grid-template-columns: repeat(auto-fill, minmax(17rem, 1fr)); gap: 1rem; margin: 1rem 0 2rem; }
.card.account-world, a.card.account-new, a.card.account-admin { display: flex; flex-direction: column; gap: .45rem; padding: 1.1rem 1.2rem;
  background: var(--panel); border: 1px solid var(--line); text-decoration: none; color: var(--fg); }
.account-world-head { display: flex; justify-content: space-between; align-items: baseline; gap: .6rem; }
.account-world-head a { color: var(--fg); text-decoration: none; font-size: 1.1rem; }
.account-world-head a:hover { color: var(--accent); }
.account-plan { font-size: .92rem; }
.account-links { display: flex; gap: 1rem; margin-top: auto; padding-top: .5rem; border-top: 1px solid var(--line); font-size: .92rem; }
a.card.account-new { border-style: dashed; justify-content: center; }
a.card.account-new strong::before { content: "+ "; color: var(--accent); }
a.card.account-new:hover, a.card.account-admin:hover { border-color: var(--accent); }
a.card.account-admin { border-left: 3px solid var(--accent); margin: 0 0 1rem; max-width: 34rem; }
a.card.account-admin strong { font-size: 1.1rem; }
table.account-usage { width: 100%; margin: .2rem 0 0; font-size: .9rem; }
table.account-usage td { padding: .15rem .4rem .15rem 0; border: 0; }
.billing-failed { margin: .3rem 0 0; padding: .55rem .7rem; font-size: .9rem; border-left: 3px solid var(--accent); background: var(--bg); }
form.billing-open { margin: 0; }
table.usage-meter { width: 100%; max-width: 44rem; margin: .4rem 0 .6rem; }
table.usage-meter td { padding: .35rem .6rem .35rem 0; vertical-align: middle; }
table.usage-meter td.num, td.num, th.num { text-align: right; font-variant-numeric: tabular-nums; white-space: nowrap; }
table.usage-meter meter { width: 8rem; height: .6rem; }
form.billing-open button { font-size: .88rem; padding: .3rem .7rem; }
.account-links { align-items: center; }
h1.big { font-size: 2rem; margin-top: 3rem; }
input.wide { flex: 1 1 26rem; min-width: 0; width: 100%; padding: .65rem .8rem; font: inherit;
  background: var(--panel); color: var(--fg); border: 1px solid var(--line); border-radius: 0; }
button.primary { background: var(--accent); color: var(--bg); border-color: var(--accent); font-weight: 600; }
.example { margin-top: 2.5rem; max-width: 40rem; }
pre#log { background: var(--panel); border: 1px solid var(--line); border-radius: 0;
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
    /// Of the members, who may also change what the world is: its settings, where it went, its
    /// whole export. Everybody else among them edits sources and trackers. The same people as
    /// `members` where nobody is told apart.
    owners: Vec<String>,
    /// Signed in once for every workspace on the machine, at its root: `zetlyn hosting`.
    shared: bool,
}

impl Hosted {
    // Addresses, `domain:` and `signed-in`, as `access:` writes them: a member signs in by a link to
    // their address, so that is what they are known by here.
    fn is_member(&self, email: &str) -> bool {
        crate::account::admits(&self.members, email, &[])
    }
    fn is_owner(&self, email: &str) -> bool {
        crate::account::admits(&self.owners, email, &[])
    }
}

/// `zetlyn world serve <workspace> [--addr 127.0.0.1:2500]`: one world on a domain of its own, at
/// its root (FEDERATION.md, M11). Its owners, `owners:` in its workspace.yaml, sign in by a link to
/// their address and run it; everybody else reads what it publishes. Kept current and published on
/// its own clock, as `zetlyn run` would.
pub fn world_serve(args: &[String]) -> Result<(), String> {
    let root = crate::positional(args, 2).first().map(|s| PathBuf::from(s.as_str())).ok_or("which workspace?")?;
    let addr = crate::flag(args, "--addr").unwrap_or("127.0.0.1:2500").to_string();
    let owners = crate::account::Site::load(&root).all_owners();
    if owners.is_empty() {
        return Err(format!("{}: its workspace.yaml names no owners (`access: {{ owners: [...] }}`), so nobody could sign in to run it", root.display()));
    }
    let server = tiny_http::Server::http(&addr).map_err(|e| e.to_string())?;
    println!("{} on http://{addr}/, run by {}", root.display(), owners.join(", "));
    {
        let root = root.clone();
        std::thread::spawn(move || loop {
            let soonest = crate::schedule_pass(&root, true, &crate::Limits::default());
            // A world's trackers open where it serves them, at its root.
            if let Err(e) = publish_root(&root, "world", |app| {
                crate::tracker::scope_registry(&root.join("trackers"))
                    .into_iter()
                    .filter_map(|(name, path)| Some((name, format!("{app}/trackers/{}/", path.file_name()?.to_string_lossy()))))
                    .collect()
            }) {
                eprintln!("not published: {e}");
            }
            if let Err(e) = crate::directory::refresh(&root) {
                eprintln!("directory: {e}");
            }
            let wait = soonest.map(|s| (s - crate::now()).clamp(60, 900)).unwrap_or(900);
            std::thread::sleep(std::time::Duration::from_secs(wait as u64));
        });
    }
    crate::account::Accounts::open(&root)?;
    // Several requests at once, as on the hosting machine: each thread its own app and its own
    // connection to the accounts, the jobs shared.
    let server = Arc::new(server);
    let jobs = Arc::new(Mutex::new(Jobs::default()));
    let workers: Vec<_> = (0..WORKERS)
        .map(|_| {
            let (server, root, addr, jobs, owners) = (server.clone(), root.clone(), addr.clone(), jobs.clone(), owners.clone());
            std::thread::spawn(move || {
                let accounts = match crate::account::Accounts::open(&root) {
                    Ok(a) => a,
                    Err(e) => return eprintln!("accounts: {e}"),
                };
                let members = crate::account::Site::load(&root).all_editors();
                let mut app = App {
                    root,
                    addr,
                    jobs,
                    base: String::new(),
                    hosted: Some(Hosted { members, owners, accounts, shared: false }),
                    visitor: false,
                    who: None,
                    orgs_of_who: Vec::new(),
                    public_of_machine: Vec::new(),
                };
                for request in server.incoming_requests() {
                    // Who runs it is read afresh each time: somebody added a minute ago is in now.
                    let site = crate::account::Site::load(&app.root);
                    if let Some(h) = app.hosted.as_mut() {
                        h.members = site.all_editors();
                        h.owners = site.all_owners();
                    }
                    serve::mount("");
                    app.answer(request);
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
    Ok(())
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
        jobs: Arc::new(Mutex::new(Jobs::default())),
        base,
        hosted: Some(Hosted { members: vec![owner.clone()], owners: vec![owner], accounts, shared: false }),
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
    /// The machine it was read from, so each world's own `access:` counts beside this file.
    #[serde(skip)]
    dir: PathBuf,
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
        let mut m: Membership = crate::yaml::read_or_default(&dir.join(MEMBERS));
        m.dir = dir.to_path_buf();
        m
    }
    fn save(&self, dir: &Path) -> Result<(), String> {
        let path = dir.join(MEMBERS);
        std::fs::write(&path, crate::yaml::to_string(self)?).map_err(|e| format!("{}: {e}", path.display()))
    }
    fn of(&self, org: &str, roles: &[&str]) -> Vec<String> {
        self.orgs.get(org).map(|m| m.iter().filter(|x| roles.contains(&x.role.as_str())).map(|x| x.email.to_lowercase()).collect()).unwrap_or_default()
    }
    /// The organisations somebody belongs to, with what they are in each.
    fn orgs_of(&self, email: &str) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .orgs
            .iter()
            .filter_map(|(org, ms)| ms.iter().find(|m| m.email.eq_ignore_ascii_case(email)).map(|m| (org.clone(), m.role.clone())))
            .collect();
        // And every world whose own `access:` names them, as owner before editor.
        if !self.dir.as_os_str().is_empty() {
            for org in orgs_in(&self.dir) {
                if out.iter().any(|(o, _)| *o == org) {
                    continue;
                }
                let site = crate::account::Site::load(&self.dir.join("orgs").join(&org));
                if crate::account::admits(&site.all_owners(), email, &[]) {
                    out.push((org, "owner".into()));
                } else if crate::account::admits(&site.access.editors, email, &[]) {
                    out.push((org, "editor".into()));
                }
            }
        }
        out
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
        // One pass and out, for a timer: the updates in a process of their own.
        Some("run") => {
            let dir = PathBuf::from(crate::positional(args, 2).first().ok_or("which hosting directory?")?.as_str());
            hosting_pass(&dir);
            Ok(())
        }
        // A new organisation: its workspace, empty, beside the others.
        Some("org") => {
            let dir = PathBuf::from(crate::positional(args, 2).first().ok_or("which hosting directory?")?.as_str());
            let name = crate::positional(args, 2).get(1).map(|s| s.to_string()).ok_or("which organisation?")?;
            let root = make_org(&dir, &name, crate::flag(args, "--title").unwrap_or(&name))?;
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
            let removing = args.iter().any(|a| a == "--remove");
            set_member(&dir, &org, &email, (!removing).then_some(role.as_str()))?;
            println!("{email} {} {org}", if removing { "is no longer in".to_string() } else { format!("is {role} in") });
            Ok(())
        }
        _ => Err("zetlyn hosting serve <dir> [--addr 127.0.0.1:2400] [--no-updates] | run <dir> | org <dir> <name> [--title …] | member <dir> <org> <email> [--role owner|editor|reader] [--remove]".into()),
    }
}

/// Whose a world's name is, where it is taken: on the main server by the register of cells, on a
/// machine of its own by its directory and its owner there. Empty for a world nobody owns.
fn world_taken(dir: &Path, name: &str) -> Option<String> {
    if crate::ops::is_control(dir) {
        return crate::ops::register(dir).ok()?.cells.get(name).map(|c| c.owner.clone());
    }
    dir.join("orgs").join(name).is_dir().then(|| {
        Membership::load(dir).orgs.get(name).and_then(|ms| ms.iter().find(|m| m.role == "owner").map(|m| m.email.clone())).unwrap_or_default()
    })
}

/// The operator's pages on the main server, `/account/admin/…`: a status, a page, or (303) where to
/// go after an action was asked for.
fn admin(dir: &Path, rest: &[&str], post: bool, form: &BTreeMap<String, String>, by: &str) -> (u16, String) {
    let r = match crate::ops::register(dir) {
        Ok(r) => r,
        Err(e) => return (500, page("Admin", html! { h1 { "Admin" } div.note { (e) } })),
    };
    let home = serve::at("/admin/");
    let cell_home = |c: &str| serve::at(&format!("/admin/cell/{c}"));
    let mb = |b: &J| b.as_f64().map(|x| format!("{:.0} MB", x / 1_048_576.0)).unwrap_or_else(|| "—".into());
    let alarms: BTreeMap<String, J> = std::fs::read(crate::ops::ops_dir(dir).join("alarms.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let billing = crate::billing::Book::read(&dir.join(BILLING)).ok();
    match (post, rest) {
        (true, ["cell", cell, action]) => {
            let args: BTreeMap<String, String> = form.iter().filter(|(k, _)| matches!(k.as_str(), "to" | "version" | "title" | "owner" | "node" | "confirm")).map(|(k, v)| (k.clone(), v.trim().to_string())).collect();
            match crate::ops::ask(dir, action, cell, &args, by) {
                Ok(_) => (303, format!("{}?asked={action}", cell_home(cell))),
                Err(e) => (400, page("Admin", html! { h1 { "Not asked" } div.note { (e) } p { a href=(cell_home(cell)) { "Back" } } })),
            }
        }
        (true, ["new"]) => {
            let cell = form.get("cell").cloned().unwrap_or_default().trim().to_lowercase();
            let args: BTreeMap<String, String> = form.iter().filter(|(k, _)| matches!(k.as_str(), "title" | "owner" | "node")).map(|(k, v)| (k.clone(), v.trim().to_string())).collect();
            match crate::ops::ask(dir, "create", &cell, &args, by) {
                Ok(_) => (303, format!("{home}?asked=create")),
                Err(e) => (400, page("Admin", html! { h1 { "Not asked" } div.note { (e) } })),
            }
        }
        (true, ["mail"]) => {
            let to: Vec<String> = match form.get("to").map(String::as_str) {
                Some("all") => r.cells.values().filter(|c| !c.house).map(|c| c.owner.clone()).filter(|o| !o.is_empty()).collect::<BTreeSet<_>>().into_iter().collect(),
                Some(cell) => r.cells.get(cell).map(|c| vec![c.owner.clone()]).unwrap_or_default(),
                None => Vec::new(),
            };
            let subject = form.get("subject").cloned().unwrap_or_default();
            let text = form.get("text").cloned().unwrap_or_default();
            if to.is_empty() || subject.trim().is_empty() || text.trim().is_empty() {
                return (400, page("Admin", html! { h1 { "Not sent" } div.note { "A recipient, a subject and a text." } p { a href={(home) "mail"} { "Back" } } }));
            }
            let sent = crate::ops::mail(dir, &to, &subject, &text, by).unwrap_or(0);
            (303, format!("{home}mail?sent={sent}"))
        }
        (false, ["mail"]) => {
            let log: Vec<J> = std::fs::read_to_string(crate::ops::ops_dir(dir).join("mail.jsonl")).unwrap_or_default().lines().rev().take(50).filter_map(|l| serde_json::from_str(l).ok()).collect();
            (200, page("Admin · Mail", html! {
                p { a href=(home) { "← Admin" } }
                h1 { "Write to customers" }
                form method="post" action={(home) "mail"} {
                    p { label { "To" br; select name="to" {
                        option value="all" { "Every customer's owner" }
                        @for (cell, c) in r.cells.iter().filter(|(_, c)| !c.house) { option value=(cell) { (cell) " · " (c.owner) } }
                    } } }
                    p { label { "Subject" br; input.wide type="text" name="subject" required; } }
                    p { label { "Text" br; textarea.wide name="text" rows="10" required {} } }
                    p { button.primary type="submit" { "Send" } }
                }
                h2 { "Sent" }
                table { tbody { @for l in &log { tr {
                    td.dim { (l["at"].as_str().unwrap_or("")) } td { (l["to"].as_str().unwrap_or("")) } td { (l["subject"].as_str().unwrap_or("")) }
                    td { @if l["ok"] == true { "sent" } @else { span.chip.on { (l["error"].as_str().unwrap_or("failed")) } } } td.dim { (l["by"].as_str().unwrap_or("")) }
                } } } }
            }))
        }
        (false, ["cell", cell]) => {
            let Some(c) = r.cells.get(*cell) else { return (404, page("Admin", html! { h1 { "No such cell" } })) };
            let s = crate::ops::cell_status(dir, &r, cell);
            let jobs: Vec<J> = crate::ops::jobs(dir, 200).into_iter().filter(|j| j["cell"] == *cell).take(20).collect();
            let logs = jobs.iter().find(|j| j["action"] == "logs" && j.get("said").is_some()).and_then(|j| j["said"].as_str().map(str::to_string));
            let customer = billing.as_ref().and_then(|b| b.get(cell));
            let act = |a: &str, label: &str| html! { form.bar method="post" action={(cell_home(cell)) "/" (a)} { button type="submit" { (label) } } };
            (200, page(&format!("Admin · {cell}"), html! {
                p { a href=(home) { "← Admin" } }
                h1 { (cell) }
                p.about { (c.title) " · " (c.owner) " · on " (c.node) @if c.house { " · the house's" } }
                table { tbody {
                    tr { th { "Running" } td { (s["active"].as_str().unwrap_or("?")) " (" (s["state"].as_str().unwrap_or("")) "), since " (s["since"].as_str().unwrap_or("")) ", restarts " (s["restarts"].as_str().unwrap_or("0")) } }
                    tr { th { "Answers" } td { (s["answers"]) } }
                    tr { th { "Version" } td { (s["version"].as_str().unwrap_or("")) } }
                    tr { th { "Port" } td { (s["port"].as_str().unwrap_or("")) } }
                    tr { th { "Memory" } td { (mb(&s["memory"])) " of " (mb(&s["memory_max"])) } }
                    tr { th { "On disk" } td { (mb(&s["bytes"])) } }
                    tr { th { "Last reading" } td { (s["run_result"].as_str().unwrap_or("")) " · " (s["run_finished"].as_str().unwrap_or("")) } }
                    tr { th { "Last snapshot" } td { (s["snapshot"].as_str().unwrap_or("none")) } }
                    tr { th { "Domain" } td { (s["domain"].as_str().unwrap_or("—")) } }
                    tr { th { "Terms" } td { code { (s["terms"].to_string()) } } }
                    @if let Some(cu) = customer { tr { th { "Customer" } td { (cu.plan) " · " (cu.state) " · paid until " (cu.paid_until.clone().unwrap_or_default()) } } }
                } }
                h2 { "Do" }
                div.bar { (act("restart", "Restart")) (act("stop", "Stop")) (act("start", "Start")) (act("snapshot", "Snapshot now")) (act("suspend", "Suspend updates")) (act("resume", "Resume updates")) (act("logs", "Fetch logs")) }
                form.bar method="post" action={(cell_home(cell)) "/move"} {
                    select name="to" { @for n in r.nodes.keys().filter(|n| **n != c.node) { option value=(n) { (n) } } }
                    button type="submit" { "Move there" }
                }
                form.bar method="post" action={(cell_home(cell)) "/upgrade"} {
                    input type="text" name="version" placeholder="0.3.62" required;
                    button type="submit" { "Upgrade" }
                }
                @if !c.house {
                    form.bar method="post" action={(cell_home(cell)) "/remove"} {
                        input type="text" name="confirm" placeholder={"type " (cell) " to remove it"} required;
                        button type="submit" { "Remove" }
                    }
                }
                h2 { "Jobs" }
                table { tbody { @for j in &jobs { tr {
                    td.dim { (j["at"].as_str().unwrap_or("")) } td { (j["action"].as_str().unwrap_or("")) } td.dim { (j["by"].as_str().unwrap_or("")) }
                    td { @if let Some(e) = j["error"].as_str() { span.chip.on { (e) } } @else if let Some(sd) = j["said"].as_str() { (sd.lines().next().unwrap_or("")) } @else { "waiting" } }
                } } } }
                @if let Some(l) = logs { h2 { "Logs" } pre { (l) } }
            }))
        }
        (false, []) => {
            let rows: Vec<(String, crate::ops::CellEntry, J)> = r.cells.iter().map(|(n, c)| (n.clone(), c.clone(), crate::ops::cell_status(dir, &r, n))).collect();
            (200, page("Admin", html! {
                h1 { "Admin" }
                @if alarms.is_empty() { p.dim { "Nothing is wrong." } } @else {
                    h2 { "Wrong" }
                    ul { @for (_, a) in &alarms { li { (a["text"].as_str().unwrap_or("")) span.dim { " · since " (a["since"].as_str().unwrap_or("")) } } } }
                }
                h2 { "Servers" }
                table { thead { tr { th { "Server" } th { "Address" } th { "Cells" } th { "Memory free" } th { "Disk free" } th { "Load" } th { "Heard" } } }
                    tbody { @for (name, n) in &r.nodes {
                        @let s = crate::ops::last_status(dir, name);
                        tr {
                            td { strong { (name) } @if n.draining { " " span.chip { "draining" } } }
                            td.dim { (n.host) }
                            td { (s["cells"].as_array().map_or(0, Vec::len)) }
                            td { (mb(&s["memory_available"])) " of " (mb(&s["memory_total"])) }
                            td { (mb(&s["disk_free"])) }
                            td { (s["load"].as_str().unwrap_or("")) }
                            td { @if let Some(e) = s["error"].as_str() { span.chip.on { (e) } } @else { (s["at"].as_str().unwrap_or("never")) } }
                        }
                    } }
                }
                h2 { "Cells" }
                table { thead { tr { th { "Cell" } th { "Owner" } th { "Server" } th { "Running" } th { "Answers" } th { "Version" } th { "Memory" } th { "Snapshot" } th { "Paid" } } }
                    tbody { @for (name, c, s) in &rows {
                        tr {
                            td { a href=(cell_home(name)) { strong { (name) } } div.why { (c.title) } }
                            td.dim { (c.owner) }
                            td { (c.node) }
                            td { (s["active"].as_str().unwrap_or("?")) }
                            td { (s["answers"]) }
                            td { (s["version"].as_str().unwrap_or("")) }
                            td { (mb(&s["memory"])) }
                            td.dim { (s["snapshot"].as_str().unwrap_or("none")) }
                            td { @if c.house { "house" } @else if let Some(cu) = billing.as_ref().and_then(|b| b.get(name)) { (cu.state) " · " (cu.paid_until.clone().unwrap_or_default()) } @else { "—" } }
                        }
                    } }
                }
                h2 { "A new cell" }
                form.bar method="post" action={(home) "new"} {
                    input type="text" name="cell" placeholder="name" pattern="[a-z0-9-]+" required;
                    input type="text" name="title" placeholder="Title";
                    input type="email" name="owner" placeholder="owner@example.org" required;
                    select name="node" { option value="" { "where there is room" } @for n in r.nodes.keys() { option value=(n) { (n) } } }
                    button type="submit" { "Make it" }
                }
                p { a href={(home) "mail"} { "Write to customers" } }
            }))
        }
        _ => (404, page("Admin", html! { h1 { "Not here" } })),
    }
}

/// Where the machine keeps its plans and its customers: `plans.yaml`, written by the operator, and
/// `customers.db`, moved by Stripe's events.
const BILLING: &str = "billing";

/// What a world may run today: as much as it likes where nobody pays for it (the operator's own,
/// or one granted by hand without a plan); its plan's where somebody does; nothing where that
/// payment has lapsed past its grace.
fn world_terms(dir: &Path, org: &str) -> (bool, crate::Limits) {
    // A cell's terms are its own file, written by the main server from the plan.
    if let Some(t) = crate::cell::terms(dir) {
        return (t.active, crate::Limits { sources: t.sources, every: crate::fetch::duration(&t.every).unwrap_or(0) });
    }
    let billing = dir.join(BILLING);
    let customer = crate::billing::Book::read(&billing).ok().and_then(|b| b.get(org));
    match customer {
        None => (true, crate::Limits::default()),
        Some(_) => {
            let (ok, limits, _mails) = crate::billing::limits(&billing, org);
            (ok, limits)
        }
    }
}

/// Stripe's event, at `POST /billing/stripe`: checked by its signature, applied to the book, and
/// a checkout that completed is a world, made for whoever paid, who is told where it is.
fn stripe_webhook(mut request: tiny_http::Request, dir: &Path) {
    let json_kind = "application/json";
    let Ok(secret) = std::env::var("STRIPE_WEBHOOK_SECRET") else {
        return respond(request, 503, json_kind, &json!({ "error": "STRIPE_WEBHOOK_SECRET is not set" }).to_string());
    };
    let signature = request.headers().iter().find(|h| h.field.equiv("Stripe-Signature")).map(|h| h.value.as_str().to_string()).unwrap_or_default();
    let mut body = Vec::new();
    let _ = std::io::Read::read_to_end(&mut std::io::Read::take(request.as_reader(), 1 << 20), &mut body);
    let event = match crate::billing::verified(&body, &signature, &secret) {
        Ok(e) => e,
        // Refused, and Stripe gives up: it is not Stripe's, or not now.
        Err(e) => {
            eprintln!("billing: an event refused: {e}");
            return respond(request, 400, json_kind, &json!({ "error": e }).to_string());
        }
    };
    match take_event(dir, &event) {
        Ok(said) => {
            eprintln!("billing: {said}");
            respond(request, 200, json_kind, &json!({ "ok": said }).to_string());
        }
        // A moment's failure is 500, and Stripe sends it again.
        Err(e) => {
            eprintln!("billing: {e}");
            respond(request, 500, json_kind, &json!({ "error": e }).to_string());
        }
    }
}

/// A Stripe event, however it came (the webhook, the page a buyer returns to, the hourly look at
/// Stripe): applied to the book, and a checkout that was paid is a world, made once. What it did.
pub(crate) fn take_event(dir: &Path, event: &J) -> Result<String, String> {
    let billing = dir.join(BILLING);
    let checkout = event["type"] == "checkout.session.completed";
    let o = &event["data"]["object"];
    let name = o["client_reference_id"].as_str().unwrap_or("").to_string();
    let payer = o["customer_details"]["email"].as_str().or(o["customer_email"].as_str()).unwrap_or("").to_lowercase();
    if checkout {
        if !matches!(o["payment_status"].as_str(), Some("paid" | "no_payment_required")) || o["status"].as_str().is_some_and(|s| s != "complete") {
            return Ok(format!("{name}: not paid yet"));
        }
        // Taken already, by the same customer: the webhook and the page and the look at Stripe all
        // bring the same checkout, and it is one world.
        let book = crate::billing::Book::open(&billing)?;
        if book.get(&name).is_some_and(|c| c.stripe_customer.is_some() && c.stripe_customer.as_deref() == o["customer"].as_str()) {
            return Ok(format!("{name}: taken already"));
        }
        // A world already here that is not the payer's is not handed over: the payment is kept,
        // and the operator is the one to sort it out.
        if world_taken(dir, &name).is_some_and(|owner| !owner.eq_ignore_ascii_case(&payer)) {
            return Ok(format!("needs attention: {payer} paid for {name}, which is somebody else's"));
        }
    }
    let said = crate::billing::apply(&billing, event)?;
    if checkout {
        // On the main server a world is a cell, made on whichever server has room by the root
        // side; on a machine of its own it is made here.
        if crate::ops::is_control(dir) {
            let title = crate::billing::Book::open(&billing).ok().and_then(|b| b.reservation(&name)).map(|(_, t)| t).filter(|t| !t.is_empty()).unwrap_or_else(|| name.clone());
            let args: BTreeMap<String, String> = [("title".to_string(), title), ("owner".to_string(), payer.clone())].into_iter().collect();
            crate::ops::ask(dir, "create", &name, &args, "stripe")?;
            // Named for it at Stripe, where the key may: one customer per organisation, and the
            // dashboard and the invoices say which.
            if let Some(customer) = o["customer"].as_str() {
                let title = args.get("title").cloned().unwrap_or_else(|| name.clone());
                if let Err(e) = crate::stripe::name_customer(customer, &title, &name) {
                    eprintln!("billing: {name}: not named at Stripe: {e}");
                }
            }
        } else {
            provision(dir, &name, &payer)?;
        }
    }
    Ok(said)
}

/// The world somebody paid for: made under the name they chose, with the title they gave it, them
/// as its owner; and a mail saying where it is and how to sign in.
fn provision(dir: &Path, name: &str, email: &str) -> Result<(), String> {
    let book = crate::billing::Book::open(&dir.join(BILLING))?;
    let title = book.reservation(name).map(|(_, t)| t).filter(|t| !t.is_empty()).unwrap_or_else(|| name.to_string());
    let fresh = !dir.join("orgs").join(name).is_dir();
    make_org(dir, name, &title)?;
    set_member(dir, name, email, Some("owner"))?;
    if fresh {
        let site = crate::account::Site::load(dir);
        let home = format!("{}/{name}/", site.url.trim_end_matches('/'));
        let (subject, text) = crate::mail::welcome_letter(&title, &home, &format!("{}/account/", site.url.trim_end_matches('/')), email);
        site.send(email, &subject, &text)?;
    }
    Ok(())
}

/// A new organisation: its workspace, empty, beside the others, with its own key to publish with.
/// One already made is left as it is.
pub(crate) fn make_org(dir: &Path, name: &str, title: &str) -> Result<PathBuf, String> {
    if !org_name(name) {
        return Err(format!("{name}: an organisation's name is lower case letters, digits and hyphens"));
    }
    let root = dir.join("orgs").join(name);
    // One name for the site, the hub and every organisation: a name the hub refuses, or a
    // path of the site, is refused here too. One already made stays reachable.
    if !root.is_dir() {
        if let Some(why) = crate::hub::why_not(name) {
            return Err(format!("{name}: {why}"));
        }
    }
    for d in ["sources", "trackers"] {
        std::fs::create_dir_all(root.join(d)).map_err(|e| format!("{}: {e}", root.display()))?;
    }
    let file = root.join(crate::account::WORKSPACE);
    if !file.exists() {
        std::fs::write(&file, format!("title: {}\n", serde_json::to_string(title).unwrap_or_default()))
            .map_err(|e| format!("{}: {e}", file.display()))?;
    }
    // Its own key to publish with, not the machine's: it is in its export, and what its
    // subscribers pinned still holds where it goes.
    let signs = root.join(".zetlyn");
    if !signs.join(crate::identity::KEY_FILE).exists() {
        let title = crate::account::Site::load(&root).title;
        crate::identity::new_in(&signs, if title.is_empty() { name } else { &title }, "")?;
    }
    Ok(root)
}

/// Somebody in an organisation on the machine as owner, editor or reader; with no role, out of it.
pub(crate) fn set_member(dir: &Path, org: &str, email: &str, role: Option<&str>) -> Result<(), String> {
    let mut m = Membership::load(dir);
    let list = m.orgs.entry(org.to_string()).or_default();
    list.retain(|x| !x.email.eq_ignore_ascii_case(email));
    if let Some(role) = role {
        list.push(Member { email: email.to_lowercase(), role: role.to_string() });
    }
    // An organisation nobody is in any more is not listed as one.
    if m.orgs.get(org).is_some_and(|l| l.is_empty()) {
        m.orgs.remove(org);
    }
    m.save(dir)
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

/// One pass over every organisation on the machine: what is due read, what moved published, the
/// directory's worlds read again. When the next thing is due, where anything is.
fn hosting_pass(dir: &Path) -> Option<i64> {
    let mut soonest: Option<i64> = None;
    for org in orgs_in(dir) {
        let root = dir.join("orgs").join(&org);
        // A world nobody pays for any more is read no further; its pages stay, and its export.
        let (go, limits) = world_terms(dir, &org);
        if !go {
            eprintln!("{org}: not paid for, so nothing is updated");
            continue;
        }
        if let Some(s) = crate::schedule_pass(&root, true, &limits) {
            soonest = Some(soonest.map_or(s, |x| x.min(s)));
        }
        if let Err(e) = publish_moved(dir, &org) {
            eprintln!("{org}: not published: {e}");
        }
    }
    // The directory's worlds, read again where they are due.
    if crate::cell::terms(dir).is_some() {
        return soonest;
    }
    if let Err(e) = crate::directory::refresh(dir) {
        eprintln!("directory: {e}");
    }
    soonest
}

fn hosting_serve(args: &[String]) -> Result<(), String> {
    let dir = PathBuf::from(crate::positional(args, 2).first().ok_or("which hosting directory?")?.as_str());
    std::fs::create_dir_all(dir.join("orgs")).map_err(|e| format!("{}: {e}", dir.display()))?;
    let addr = crate::flag(args, "--addr").unwrap_or("127.0.0.1:2400").to_string();
    // Opened once here so a store that cannot be opened stops the machine at the start, not in each
    // thread; every thread opens its own after.
    crate::account::Accounts::open(&dir)?;
    // The machine and its organisations answer each other here, not over HTTP to themselves.
    // A cell is not the machine: "Sign in with zetlyn.com" is the main server's, asked over HTTP.
    if crate::cell::terms(&dir).is_none() {
        crate::oidc::serving_machine(&dir);
    } else {
        // And signing in is the main server's: a cell asks it who a session is.
        crate::account::ask_identity_at(&format!("{}/account/me", crate::account::Site::load(&dir).url.trim_end_matches('/')));
    }
    let server = tiny_http::Server::http(&addr).map_err(|e| e.to_string())?;
    println!("{} organisations from {} on http://{addr}/", orgs_in(&dir).len(), dir.display());

    // Every organisation's sources, trackers and watches, one after another, as `zetlyn run`
    // does them for one workspace. With --no-updates that is somebody else's: `zetlyn hosting run`,
    // a process of its own on a timer, whose reading every source whole is gone from memory when
    // it ends, rather than kept by the process that answers pages (2026-10-05).
    if !args.iter().any(|a| a == "--no-updates") {
        let dir = dir.clone();
        std::thread::spawn(move || loop {
            let soonest = hosting_pass(&dir);
            let wait = soonest.map(|s| (s - crate::now()).clamp(60, 900)).unwrap_or(900);
            std::thread::sleep(std::time::Duration::from_secs(wait as u64));
        });
    }

    // Several requests at once: a slow page (a tracker's filter over thousands of claims) holds up
    // its own thread and nobody else's. Crawlers walking those filters, two a second, once kept
    // every other page waiting a minute (2026-10-04). Each thread keeps its own opened apps and its
    // own connection to the accounts; what a job says is shared, so a job started on one thread is
    // read on any.
    let server = Arc::new(server);
    let jobs: SharedJobs = Arc::new(Mutex::new(BTreeMap::new()));
    let workers: Vec<_> = (0..WORKERS)
        .map(|_| {
            let (server, dir, addr, jobs) = (server.clone(), dir.clone(), addr.clone(), jobs.clone());
            std::thread::spawn(move || hosting_worker(&server, &dir, &addr, &jobs))
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
    Ok(())
}

/// How many requests are answered at once.
const WORKERS: usize = 8;

/// Each app's jobs, by the key it is kept under, the same for every thread.
type SharedJobs = Arc<Mutex<BTreeMap<String, Arc<Mutex<Jobs>>>>>;

/// The jobs of the app under `key`, made the first time any thread asks.
fn jobs_of(jobs: &SharedJobs, key: &str) -> Arc<Mutex<Jobs>> {
    let mut all = jobs.lock().unwrap_or_else(|e| e.into_inner());
    all.entry(key.to_string()).or_insert_with(|| Arc::new(Mutex::new(Jobs::default()))).clone()
}

/// One of the threads answering requests on the machine.
fn hosting_worker(server: &tiny_http::Server, dir: &Path, addr: &str, jobs: &SharedJobs) {
    let accounts = match crate::account::Accounts::open(dir) {
        Ok(a) => a,
        Err(e) => return eprintln!("accounts: {e}"),
    };
    let mut apps: BTreeMap<String, App> = BTreeMap::new();
    for request in server.incoming_requests() {
        let url = request.url().to_string();
        let path = url.split('?').next().unwrap_or("/").to_string();
        // On the main server its owners run zetlyn.com: their account menu leads to the admin pages.
        if crate::ops::is_control(dir) {
            crate::account::note_operators(&crate::account::Site::load(dir).all_owners());
        }
        let parts: Vec<String> = path.split('/').filter(|s| !s.is_empty()).map(serve::urldecode).collect();
        let first = parts.first().cloned().unwrap_or_default();
        // Stripe's events: a program's POST, from another site by nature, held by its signature.
        if parts.len() == 2 && first == BILLING && parts[1] == "stripe" && request.method() == &tiny_http::Method::Post {
            stripe_webhook(request, dir);
            continue;
        }
        // A form from another site is refused here too, before the machine's own pages and its
        // provider see it, as an organisation's app refuses one. A sign-in answer and a token are
        // what they are (Apple's answer is a form from Apple's page), held by state and code.
        let signing_in = path.ends_with("/oauth/callback") || path.ends_with("/oauth/token");
        if request.method() == &tiny_http::Method::Post && !signing_in && from_another_site(&request) {
            respond(request, 403, "text/plain; charset=utf-8", "a form from another site");
            continue;
        }
        // Asked by a name that is not the machine's: a hosted world's own domain, whole, at its
        // root; or nothing here. Only a name an organisation says is its own is ever answered for.
        let host = request.headers().iter().find(|h| h.field.equiv("Host")).map(|h| h.value.as_str().split(':').next().unwrap_or("").to_lowercase()).unwrap_or_default();
        let machine_host = crate::account::Site::load(dir).url.split("://").nth(1).unwrap_or("").split(['/', ':']).next().unwrap_or("").to_lowercase();
        if !host.is_empty() && !machine_host.is_empty() && host != machine_host && host != "127.0.0.1" && host != "localhost" {
            match org_by_domain(dir, &host) {
                Some(org) if first == APP_PREFIX || first == ACCOUNT => {
                    // Signing in to it happens at its own name, so the cookie is for that name.
                    serve::mount(&format!("/{first}"));
                    hosting_root(request, dir, &accounts, &parts[1..], &format!("https://{host}"));
                    let _ = org;
                }
                Some(org) => answer_org(&mut apps, jobs, &format!("{org}@{host}"), &org, "", dir, addr, &accounts, request),
                None => respond(request, 421, "text/plain; charset=utf-8", &format!("No world on this machine is at {host}.\n")),
            }
            continue;
        }
        // The examples the docs and the app point at: a shop's list and a bookshop's page, made as
        // they are asked for, because one of them changes every two minutes. "examples" is a
        // reserved name, so no organisation is here.
        // The sheet the machine's own pages at its root name (the directory of worlds, a page that
        // is not here): its own, under a name the website's /style.css does not answer for.
        if parts.len() == 1 && first == "zetlyn.css" {
            respond(request, 200, "text/css; charset=utf-8", &format!("{}{APP_STYLE}", serve::STYLE));
            continue;
        }
        if first == "examples" && matches!(request.method(), tiny_http::Method::Get | tiny_http::Method::Head) {
            let asked = path.trim_start_matches('/');
            match crate::examples::file(asked) {
                Some(body) => respond(request, 200, if asked.ends_with(".csv") { "text/csv; charset=utf-8" } else { "text/html; charset=utf-8" }, &body),
                None => respond(request, 404, "text/plain; charset=utf-8", "nothing at that address\n"),
            }
            continue;
        }
        // A world is at /<name>/, as it would be at the root of a domain of its own, its trackers at
        // /<name>/trackers/<tracker>/. It was at /worlds/<name>/ from 2026-10-05 to 10-06, and at
        // /<name>/ with its trackers under t/ before that: a page asked for at either is sent here,
        // and what a program sends there, a webhook, a proposal or another world's sign-in, is
        // still taken there, as a program seldom follows.
        let legacy = first == WORLDS && parts.len() >= 2;
        let org = if legacy { parts[1].clone() } else { first.clone() };
        if org_name(&org) && dir.join("orgs").join(&org).is_dir() {
            let base = if legacy { format!("/{WORLDS}/{org}") } else { format!("/{org}") };
            let rest = url.strip_prefix(&base).unwrap_or("").to_string();
            let get = matches!(request.method(), tiny_http::Method::Get | tiny_http::Method::Head);
            let old_t = rest == "/t" || rest.starts_with("/t/") || rest.starts_with("/t?");
            if get && (legacy || old_t) {
                let rest = if old_t { format!("/trackers{}", &rest[2..]) } else { rest };
                redirect_permanently(request, &format!("/{org}{rest}"));
                continue;
            }
            // A world with a domain of its own is there: a page asked for here goes to it.
            let own = crate::account::Site::load(&dir.join("orgs").join(&org)).domain.trim().to_lowercase();
            // Not its sign-in, which a world that knew it here before still asks for here; not the
            // machine's own name, which is no domain of its own. And for now, not for good: a
            // domain can be given up, and a browser keeps a permanent answer past that.
            let signing = rest.starts_with("/oauth") || rest.starts_with("/.well-known");
            if !own.is_empty() && own != machine_host && !signing && get {
                let rest = if rest.is_empty() { "/".to_string() } else { rest };
                let mut response = tiny_http::Response::from_string("").with_status_code(302);
                if let Ok(h) = tiny_http::Header::from_bytes(&b"Location"[..], format!("https://{own}{rest}").as_bytes()) {
                    response = response.with_header(h);
                }
                let _ = request.respond(response);
                continue;
            }
            let key = if legacy { format!("{org}#worlds") } else { org.clone() };
            answer_org(&mut apps, jobs, &key, &org, &base, dir, addr, &accounts, request);
            continue;
        }
        // The machine's own pages are under `/app/`: the site, the hub and every organisation share
        // one name, and the site has `/` and `/style.css`.
        // The machine as a provider, "Sign in with zetlyn.com": its issuer is its address, so its
        // discovery is at the root; everything else of it is under /app/oauth/.
        if first == ".well-known" && parts.get(1).map(String::as_str) == Some("openid-configuration") {
            serve::mount("");
            let here = crate::oidc::Here::machine(dir);
            let _ = crate::oidc::answer(&here, request, &parts, &url);
            continue;
        }
        // The machine's directory of worlds, zetlyn.com/directory.
        if first == "directory" || first == "directory.json" {
            serve::mount("");
            if let Some(request) = crate::directory::answer(dir, request, &parts, &url) {
                respond(request, 404, "text/plain; charset=utf-8", "nothing at that address");
            }
            continue;
        }
        // Ordering an organisation, at a name that says so: the account's order page.
        if first == "order" && crate::ops::is_control(dir) {
            serve::mount(&format!("/{ACCOUNT}"));
            let rest: Vec<String> = std::iter::once("new".to_string()).chain(parts[1..].iter().cloned()).collect();
            hosting_root(request, dir, &accounts, &rest, "");
            continue;
        }
        // The machine's own pages: whoever is signed in, and the worlds they belong to.
        if first == ACCOUNT {
            serve::mount(&format!("/{ACCOUNT}"));
            hosting_root(request, dir, &accounts, &parts[1..], "");
            continue;
        }
        // The machine as a provider, "Sign in with zetlyn.com", at /oauth/ as a world's is at its own.
        if first == "oauth" {
            serve::mount("");
            let here = crate::oidc::Here::machine(dir);
            if let Some(request) = crate::oidc::answer(&here, request, &parts, &url) {
                respond(request, 404, "text/plain; charset=utf-8", "nothing at that address");
            }
            continue;
        }
        // Where all of that was until 2026-10-06, under /app/: a page asked for there is sent where
        // it is now. What a program sends there, a token asked for or a certificate checked, is
        // still answered there, as a program seldom follows.
        if first == APP_PREFIX {
            let get = matches!(request.method(), tiny_http::Method::Get | tiny_http::Method::Head);
            let rest = url.strip_prefix(&format!("/{APP_PREFIX}")).unwrap_or("").to_string();
            let checked = parts.get(1).is_some_and(|p| p == "domain-check");
            if get && !checked {
                let to = if rest.starts_with("/oauth") { rest.clone() } else { format!("/{ACCOUNT}{}", if rest.is_empty() { "/".to_string() } else { rest.clone() }) };
                redirect_permanently(request, &to);
                continue;
            }
            serve::mount(&format!("/{APP_PREFIX}"));
            let here = crate::oidc::Here::machine_at(dir, &format!("/{APP_PREFIX}"));
            let Some(request) = crate::oidc::answer(&here, request, &parts[1..], &url) else { continue };
            hosting_root(request, dir, &accounts, &parts[1..], "");
            continue;
        }
        serve::mount("");
        respond(request, 404, "text/html; charset=utf-8", &page("Not here", html! { h1 { "Not here" } p { a href="/hub/" { "Every tracker on the hub" } } }));
    }
}

/// Where the machine's own pages are, on a name it shares with the site and the hub.
const APP_PREFIX: &str = "app";

/// Where the machine's own pages are now: whoever is signed in, and their worlds.
const ACCOUNT: &str = "account";

/// Where the worlds the machine hosts are, each under its name.
pub const WORLDS: &str = "worlds";

/// The machine's own pages: what anybody can read here, signing in, and your organisations.
/// One request for an organisation, by its app under `key`: `/<org>` on the machine's name, or the
/// root of its own domain. The app is made the first time it is asked for.
#[allow(clippy::too_many_arguments)]
fn answer_org(apps: &mut BTreeMap<String, App>, jobs: &SharedJobs, key: &str, org: &str, base: &str, dir: &Path, addr: &str, accounts: &crate::account::Accounts, request: tiny_http::Request) {
    let membership = Membership::load(dir);
    // A reader among them reads, as anybody may who is let read: the app is for who changes things.
    // And whoever the world's own `access:` names, in the same words a self-hosted world uses.
    let site = crate::account::Site::load(&dir.join("orgs").join(org));
    let mut members = membership.of(org, &["owner", "editor"]);
    members.extend(site.all_editors());
    let mut owners = membership.of(org, &["owner"]);
    owners.extend(site.all_owners());
    if !apps.contains_key(key) {
        let own = match crate::account::Accounts::open(dir) {
            Ok(a) => a,
            Err(e) => return respond(request, 500, "text/plain; charset=utf-8", &e),
        };
        apps.insert(key.to_string(), App {
            root: dir.join("orgs").join(org),
            addr: addr.to_string(),
            jobs: jobs_of(jobs, key),
            base: base.to_string(),
            hosted: Some(Hosted { members: Vec::new(), owners: Vec::new(), accounts: own, shared: true }),
            visitor: false,
            who: None,
            orgs_of_who: Vec::new(),
            public_of_machine: Vec::new(),
        });
    }
    let Some(app) = apps.get_mut(key) else { return };
    // Who belongs is read afresh each time: somebody added a minute ago is in now.
    if let Some(h) = app.hosted.as_mut() {
        h.members = members;
        h.owners = owners;
    }
    // Every organisation whoever is signed in belongs to, by its title, for the header.
    // At a domain of its own, the machine's pages are at the machine's name, and said with it.
    let machine = if base.is_empty() { crate::account::Site::load(dir).url.trim_end_matches('/').to_string() } else { String::new() };
    app.public_of_machine = public_links(dir).into_iter().map(|(t, l)| (t, format!("{machine}{l}"))).collect();
    app.orgs_of_who = signed_in(&request, accounts)
        .map(|email| {
            Membership::load(dir)
                .orgs_of(&email)
                .into_iter()
                .map(|(o, _)| {
                    let t = crate::account::Site::load(&dir.join("orgs").join(&o)).title;
                    (if t.is_empty() { o.clone() } else { t }, format!("{machine}/{o}/"))
                })
                .collect()
        })
        .unwrap_or_default();
    serve::mount(&app.base);
    app.answer(request);
}

/// A request a page of another site made: a browser says where a form came from, and a form from
/// anywhere but this site's own pages is refused. Programs (a webhook) say nothing and pass.
pub(crate) fn from_another_site(request: &tiny_http::Request) -> bool {
    let header = |name: &'static str| request.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str().to_string());
    let host = header("Host").unwrap_or_default();
    header("Sec-Fetch-Site").is_some_and(|s| s == "cross-site") || header("Origin").is_some_and(|o| o == "null" || o.split("://").nth(1).unwrap_or("") != host)
}

/// The organisation whose own domain `host` is, where one says so.
fn org_by_domain(dir: &Path, host: &str) -> Option<String> {
    let host = host.trim().trim_end_matches('.').to_lowercase();
    if host.is_empty() {
        return None;
    }
    orgs_in(dir)
        .into_iter()
        .find(|org| crate::account::Site::load(&dir.join("orgs").join(org)).domain.trim().eq_ignore_ascii_case(&host))
        .filter(|org| domain_allowed(dir, org))
}

/// Whether a world may answer at a domain of its own: always where nobody pays for it, and where
/// somebody does, on a plan that has one.
fn domain_allowed(dir: &Path, org: &str) -> bool {
    if let Some(t) = crate::cell::terms(dir) {
        return t.domain;
    }
    let billing = dir.join(BILLING);
    let Some(c) = crate::billing::Book::read(&billing).ok().and_then(|b| b.get(org)) else { return true };
    crate::billing::plans(&billing).ok().and_then(|(_, p)| p.get(&c.plan).map(|p| p.domain)).unwrap_or(false)
}

/// `site_url` is the address it is asked at: the machine's, or a hosted world's own domain, where
/// a sign-in link has to lead back to the same name its cookie is for.
fn hosting_root(mut request: tiny_http::Request, dir: &Path, accounts: &crate::account::Accounts, parts: &[String], site_url: &str) {
    let html_kind = "text/html; charset=utf-8";
    // Caddy asks before it takes a certificate for a name: only one a hosted world says is its own.
    if parts.len() == 1 && parts[0] == "domain-check" {
        let asked = serve::params(request.url()).get("domain").cloned().unwrap_or_default();
        return match org_by_domain(dir, &asked) {
            Some(org) => respond(request, 200, "text/plain; charset=utf-8", &org),
            None => respond(request, 404, "text/plain; charset=utf-8", "no world here is at that name"),
        };
    }
    let post = request.method() == &tiny_http::Method::Post;
    let cookie = request.headers().iter().find(|h| h.field.equiv("Cookie")).map(|h| h.value.as_str().to_string());
    let session = cookie.and_then(|c| c.split(';').filter_map(|p| p.trim().split_once('=')).find(|(k, _)| *k == "zs").map(|(_, v)| v.to_string()));
    let who = session.as_deref().and_then(|s| accounts.by_session(s, crate::account::Kind::Member));
    let membership = Membership::load(dir);
    // The app's own frame: its sidebar lists what anybody may read here, and where whoever is
    // signed in belongs.
    serve::frame_site("App");
    serve::frame_home("Account", &serve::at("/"), public_links(dir));
    serve::frame_current(None);
    serve::frame_hosted(None, Some(who.as_ref().map(|a| a.email.clone())));
    serve::frame_area("app", None, who.as_ref().map(|a| orgs_links(dir, &a.email)).unwrap_or_default());
    // The main server holds no organisation of its own any more: no heading over an empty list.
    serve::frame_side(if public_links(dir).is_empty() { "" } else { "Public trackers" });
    serve::frame_section(None, Vec::new());
    serve::frame_app(None, None);
    match parts.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["style.css" | "zetlyn.css"] => respond(request, 200, "text/css; charset=utf-8", &format!("{}{APP_STYLE}", serve::STYLE)),
        // Nobody signed in has nothing here the hub does not show better: what may be read is
        // found there, each opening where it runs.
        // Nobody signed in at the account page: signing in is what it is for.
        [] if who.is_none() && !post => redirect(request, &serve::at("/signin")),
        // Ending a contract, as § 312k BGB asks: reachable without signing in, from every page's
        // footer; what is needed to find the contract, a button that ends it, a page and a mail
        // that say so with the moment it was asked. Where the world and the address it was bought
        // with go together, Stripe ends the subscription at the end of the month; every
        // cancellation is kept and told to the operator either way.
        ["cancel"] => {
            let billing = dir.join(BILLING);
            let base = crate::account::Site::load(dir).url.trim_end_matches('/').to_string();
            if !post {
                return respond(request, 200, html_kind, &page("Cancel a contract", html! {
                    div.account-hero { p.overline { "Cancel a contract" } h1 { "End your Zetlyn Managed plan" } }
                    form method="post" action=(serve::at("/cancel")) {
                        p { label { "Your organisation's address on Zetlyn, zetlyn.com/<address>" br; input type="text" name="world" required pattern="[a-z0-9-]+" placeholder="acme-research"; } }
                        p { label { "The address it was ordered with" br; input.wide type="email" name="email" required; } }
                        p { "Kind of cancellation" br
                            label { input type="radio" name="kind" value="ordinary" checked; " Ordinary, at the end of the current month" } br
                            label { input type="radio" name="kind" value="extraordinary"; " Extraordinary, for a reason" } }
                        p { label { "The reason, for an extraordinary cancellation" br; textarea.wide name="reason" rows="3" {} } }
                        p { button.primary type="submit" { "Cancel now" } }
                        p.dim { "You get a confirmation by mail, with the date and time it was received." }
                    }
                }));
            }
            let mut body = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::Read::take(request.as_reader(), 32 << 10), &mut body);
            let form = parse_form(&body);
            let get = |k: &str| form.get(k).map(|v| v.trim().to_string()).unwrap_or_default();
            let (world, email, kind, reason) = (get("world").to_lowercase(), get("email").to_lowercase(), get("kind"), get("reason"));
            let at = crate::iso_stamp(crate::now());
            let book = crate::billing::Book::read(&billing).ok();
            let customer = book.as_ref().and_then(|b| b.get(&world)).filter(|c| c.email.eq_ignore_ascii_case(&email));
            // Ended at Stripe where the contract was found, at the end of the month either way.
            let done = match (&customer, book.as_ref().and_then(|b| b.subscription_of(&world))) {
                (Some(_), Some(sub)) => crate::stripe::cancel_at_period_end(&sub).map(|_| ()),
                (Some(_), None) => Err("no subscription on record".into()),
                (None, _) => Err("no contract for that organisation and address".into()),
            };
            let until = customer.as_ref().and_then(|c| c.paid_until.clone()).unwrap_or_default();
            let line = json!({ "at": at, "world": world, "email": email, "kind": kind, "reason": reason, "found": customer.is_some(), "ended_at_stripe": done.is_ok(), "error": done.as_ref().err() });
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(billing.join("cancellations.jsonl")) {
                let _ = std::io::Write::write_all(&mut f, format!("{line}\n").as_bytes());
            }
            let site = crate::account::Site::load(dir);
            // The confirmation goes to the address on the contract where there is one, else to the
            // one typed, so nobody learns of a contract by typing another's name.
            let to = customer.as_ref().map(|c| c.email.clone()).unwrap_or(email.clone());
            let text = format!(
                "Hello,\n\nwe received your cancellation on {at} (UTC).\n\n  Organisation: zetlyn.com/{world}\n  Kind: {kind}\n{}\n{}\n\nIf you did not send it, write to hello@zetlyn.com straight away.\n\nBest regards,\nThe Zetlyn team\n\n--\nZetlyn · https://zetlyn.com · hello@zetlyn.com\n",
                if reason.is_empty() { String::new() } else { format!("  Reason: {reason}\n") },
                if customer.is_some() { format!("Your plan ends at the end of the current month{}. Until then your organisation runs as before, and you can take all of it with you from its settings.", if until.is_empty() { String::new() } else { format!(", on {until}") }) } else { "We look up the contract it belongs to and confirm it to the address it was ordered with.".to_string() }
            );
            if !to.is_empty() {
                let _ = site.send(&to, "Your Zetlyn cancellation was received", &text);
            }
            if let Ok(r) = crate::ops::register(dir) {
                if !r.alarm.is_empty() {
                    let _ = site.send(&r.alarm, &format!("Zetlyn: cancellation of {world}"), &format!("{line}\n\n{base}/account/admin/cell/{world}\n"));
                }
            }
            respond(request, 200, html_kind, &page("Cancellation received", html! {
                div.account-hero {
                    p.overline { "Cancel a contract" }
                    h1 { "Your cancellation was received" }
                    p.lede { "On " (at) " (UTC), for " code { (world) } "." }
                    p { "A confirmation is on its way by mail. "
                        @if customer.is_some() { "Your plan ends at the end of the current month; until then your organisation runs as before." }
                        @else { "We look up the contract it belongs to and confirm it to the address it was ordered with." } }
                }
            }));
        }
        // Billing, at /account/billing: every organisation the signed-in owner pays for, its plan
        // and this month's usage, and a button into its own portal at Stripe for the payment
        // method and the invoices. The portal opens on a POST, so nothing prefetches a session;
        // where Stripe cannot open it, the page says so instead of going anywhere.
        ["billing"] => {
            let Some(a) = who.as_ref() else { return redirect(request, &serve::at("/signin")) };
            let billing = dir.join(BILLING);
            let book = crate::billing::Book::read(&billing).ok();
            let (portal, grace) = crate::billing::terms(&billing);
            let register = crate::ops::register(dir).ok();
            let paid: Vec<(String, crate::billing::Customer)> = membership
                .orgs_of(&a.email)
                .into_iter()
                .filter(|(_, r)| r == "owner")
                .filter_map(|(o, _)| book.as_ref().and_then(|b| b.get(&o)).map(|c| (o, c)))
                .collect();
            let mut failed: Option<String> = None;
            if post {
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(&mut std::io::Read::take(request.as_reader(), 4 << 10), &mut body);
                let org = parse_form(&body).get("org").cloned().unwrap_or_default();
                let back = format!("{}/account/billing", crate::account::Site::load(dir).url.trim_end_matches('/'));
                let opened = match paid.iter().find(|(o, _)| *o == org).and_then(|(_, c)| c.stripe_customer.clone()) {
                    Some(c) => crate::stripe::portal_session(&c, &back),
                    None => Err("no Stripe customer on record".into()),
                };
                match opened {
                    Ok(to) => return redirect(request, &to),
                    Err(e) => {
                        eprintln!("billing portal {org}: {e}");
                        failed = Some(org);
                    }
                }
            }
            respond(request, 200, html_kind, &page("Billing", html! {
                div.account-hero {
                    p.overline { "Your account" }
                    h1 { "Billing" }
                    p.lede { @if paid.is_empty() { "No plan in your name yet." } @else { "Your plan, this month's usage, and your payment method and invoices at Stripe." } }
                }
                div.account-worlds {
                    @for (org, c) in &paid {
                        @let title = register.as_ref().and_then(|r| r.cells.get(org)).map(|c| c.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| org.clone());
                        div.card.account-world {
                            div.account-world-head { a href={"/" (org) "/"} { strong { (title) } } }
                            span.dim.mono { "zetlyn.com/" (org) }
                            (plan_summary(dir, &billing, org, c, grace))
                            @if failed.as_deref() == Some(org.as_str()) {
                                p.billing-failed {
                                    "Stripe could not open your billing page just now. Please try again in a minute"
                                    @if !portal.is_empty() { ", or " a href={(portal) (if portal.contains('?') { "&" } else { "?" }) "prefilled_email=" (urlencode(&a.email))} { "sign in to it at Stripe" } " with the address you ordered with" }
                                    ". If it keeps failing, write to " a href="mailto:hello@zetlyn.com" { "hello@zetlyn.com" } "."
                                }
                            }
                            div.account-links {
                                @if c.stripe_customer.is_some() {
                                    form.billing-open method="post" action=(serve::at("/billing")) {
                                        input type="hidden" name="org" value=(org);
                                        button.primary type="submit" { "Payment method and invoices" }
                                    }
                                }
                                a href=(serve::at("/cancel")) { "Cancel" }
                            }
                        }
                    }
                }
                p.dim { a href=(serve::at("/")) { "Back to your account" } }
            }));
        }
        // Where the Billing link pointed until 0.3.75.
        ["billing", _] => redirect(request, &serve::at("/billing")),
        // Where Stripe sends a buyer back: the checkout asked of Stripe itself, the world made
        // from it if the webhook has not made it already, and the buyer told what happens next.
        // No sign-in: the session's id is what it shows, and it shows only that.
        ["welcome"] => {
            let id = serve::params(request.url()).get("session").cloned().unwrap_or_default();
            let ok = id.starts_with("cs_") && id.len() < 200 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            let taken = if ok { crate::stripe::session_event(&id).and_then(|e| take_event(dir, &e).map(|s| (s, e))) } else { Err("not a checkout".into()) };
            let base = crate::account::Site::load(dir).url.trim_end_matches('/').to_string();
            respond(request, 200, html_kind, &page("Thank you", html! {
                div.account-hero {
                    @match &taken {
                        Ok((_, e)) => {
                            @let o = &e["data"]["object"];
                            @let name = o["client_reference_id"].as_str().unwrap_or("");
                            @let email = o["customer_details"]["email"].as_str().unwrap_or("");
                            p.overline { "Thank you" }
                            h1 { "Your organisation is on its way" }
                            p.lede { "It is being set up now, at " a href={(base) "/" (name) "/"} { (base) "/" (name) "/" } ", and a mail to " (email) " says when it is ready, usually within a minute." }
                            p { "Sign in there with " (email) ": a link comes by mail, no password. Your plan, your usage and your invoices are at " a href=(serve::at("/")) { "your account" } "." }
                        }
                        Err(e) => {
                            p.overline { "Thank you" }
                            h1 { "We could not look at your order just now" }
                            p.lede { "Your payment is safe with Stripe, and your organisation is set up as soon as we hear of it. If no mail has come within a few minutes, write to " a href="mailto:hello@zetlyn.com" { "hello@zetlyn.com" } "." }
                            p.dim { (e) }
                        }
                    }
                }
            }));
        }
        // Who is signed in here, for the website's and the hub's pages, which are files and know
        // nobody: their script asks, and shows the address where "Sign in" was.
        ["me"] => {
            let site = crate::account::Site::load(dir);
            let body = match &who {
                Some(a) => json!({ "email": a.email, "operator": site.all_owners().iter().any(|o| o.eq_ignore_ascii_case(&a.email)) && crate::ops::is_control(dir) }),
                None => json!({}),
            };
            let mut response = tiny_http::Response::from_string(body.to_string());
            for (k, v) in [("Content-Type", "application/json"), ("Cache-Control", "no-store")] {
                if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                    response = response.with_header(h);
                }
            }
            let _ = request.respond(response);
        }
        [] => {
            let Some(a) = who.as_ref() else { return redirect(request, &serve::at("/signin")) };
            let mine = membership.orgs_of(&a.email);
            let site = crate::account::Site::load(dir);
            let operator = site.all_owners().iter().any(|o| o.eq_ignore_ascii_case(&a.email)) && crate::ops::is_control(dir);
            let register = crate::ops::register(dir).ok();
            let billing = dir.join(BILLING);
            let book = crate::billing::Book::read(&billing).ok();
            let (_, grace) = crate::billing::terms(&billing);
            let alarms: BTreeMap<String, J> = if operator { std::fs::read(crate::ops::ops_dir(dir).join("alarms.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default() } else { BTreeMap::new() };
            respond(request, 200, html_kind, &page("Your account", html! {
                div.account-hero {
                    p.overline { "Your account" }
                    h1 { (a.email) }
                    p.lede { @if mine.is_empty() { "No organisation yet." } @else { (mine.len()) @if mine.len() == 1 { " organisation" } @else { " organisations" } " you belong to." } }
                }
                @if operator {
                    a.card.account-admin href=(serve::at("/admin/")) {
                        span.overline { "Operator" }
                        strong { "Admin" }
                        span.dim { @if alarms.is_empty() { "Every server and cell, nothing wrong." } @else { (alarms.len()) " things wrong now." } }
                    }
                }
                div.account-worlds {
                    @for (org, role) in &mine {
                        @let title = register.as_ref().and_then(|r| r.cells.get(org)).map(|c| c.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| org.clone());
                        @let customer = book.as_ref().and_then(|b| b.get(org));
                        div.card.account-world {
                            div.account-world-head {
                                a href={"/" (org) "/"} { strong { (title) } }
                                span.chip { (role) }
                            }
                            span.dim.mono { "zetlyn.com/" (org) }
                            @if let Some(c) = &customer { (plan_summary(dir, &billing, org, c, grace)) }
                            div.account-links {
                                a href={"/" (org) "/"} { "Open" }
                                @if role == "owner" { a href={"/" (org) "/settings"} { "Settings" } }
                                @if role == "owner" && customer.is_some() { a href=(serve::at("/billing")) { "Billing" } }
                                @if role == "owner" && customer.is_some() { a href=(serve::at("/cancel")) { "Cancel" } }
                            }
                        }
                    }
                    a.card.account-new href="/order" {
                        strong { "Order an organisation" }
                        span.dim { "Managed Zetlyn: your sources, trackers and readers, run for you at zetlyn.com." }
                    }
                }
            }));
        }
        // The operator's pages: every cell, every server, what is wrong, and what can be done.
        ["admin", rest @ ..] => {
            let site = crate::account::Site::load(dir);
            let operator = who.as_ref().is_some_and(|a| site.all_owners().iter().any(|o| o.eq_ignore_ascii_case(&a.email)));
            if !operator || !crate::ops::is_control(dir) {
                return respond(request, 404, html_kind, &page("Not here", html! { h1 { "Not here" } }));
            }
            let by = who.as_ref().map(|a| a.email.clone()).unwrap_or_default();
            let mut form = BTreeMap::new();
            if post {
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(&mut std::io::Read::take(request.as_reader(), 256 << 10), &mut body);
                form = parse_form(&body);
            }
            let (status, html) = admin(dir, rest, post, &form, &by);
            match status {
                303 => redirect(request, &html),
                s => respond(request, s, html_kind, &html),
            }
        }
        // Whether a name can be ordered, asked by the order page as it is typed.
        ["new", "check"] => {
            let name = serve::params(request.url()).get("name").cloned().unwrap_or_default().trim().to_lowercase();
            let (ok, said) = if name.is_empty() {
                (false, "")
            } else if !org_name(&name) || !crate::cell::name_ok(&name) {
                (false, "lower case letters, digits and hyphens, at most 28")
            } else if crate::hub::why_not(&name).is_some() {
                (false, "reserved")
            } else if world_taken(dir, &name).is_some() {
                (false, "taken")
            } else {
                (true, "available")
            };
            let mut response = tiny_http::Response::from_string(json!({ "ok": ok, "said": said }).to_string());
            for (k, v) in [("Content-Type", "application/json"), ("Cache-Control", "no-store")] {
                if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                    response = response.with_header(h);
                }
            }
            let _ = request.respond(response);
        }
        // Its old address, where the order page was until 2026-10-07.
        ["new"] if !post && request.url().starts_with(&format!("/{ACCOUNT}/new")) => {
            let query = request.url().split_once('?').map(|(_, q)| format!("?{q}")).unwrap_or_default();
            redirect_permanently(request, &format!("/order{query}"));
        }
        // Ordering an organisation, at zetlyn.com/order: its name and its address on Zetlyn, the
        // buyer's address, the terms; the name held for half an hour while it is paid at Stripe.
        ["new"] => {
            let billing = dir.join(BILLING);
            let (_, plans) = crate::billing::plans(&billing).unwrap_or_default();
            let Some((plan_key, plan)) = plans.iter().next().map(|(k, p)| (k.clone(), p.clone())) else {
                return respond(request, 200, html_kind, &page("Order", html! { div.account-hero { h1 { "Ordering is not open yet" } p.lede { "Write to " a href="mailto:hello@zetlyn.com" { "hello@zetlyn.com" } " and we set it up for you." } } }));
            };
            let mut said: Option<String> = None;
            let mut form: BTreeMap<String, String> = serve::params(request.url()).into_iter().collect();
            if post {
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(&mut std::io::Read::take(request.as_reader(), 64 << 10), &mut body);
                form = parse_form(&body);
                let get = |k: &str| form.get(k).map(|v| v.trim().to_string()).unwrap_or_default();
                let (name, title, email) = (get("name").to_lowercase(), get("title"), get("email").to_lowercase());
                let refused = if title.is_empty() {
                    Some("Please give your organisation's name.".to_string())
                } else if !org_name(&name) || !crate::cell::name_ok(&name) {
                    Some("Your address on Zetlyn is lower case letters, digits and hyphens, at most 28: acme-research.".to_string())
                } else if crate::hub::why_not(&name).is_some() {
                    Some(format!("zetlyn.com/{name} is reserved; please choose another address."))
                } else if world_taken(dir, &name).is_some() {
                    Some(format!("zetlyn.com/{name} is taken; please choose another address."))
                } else if !email.contains('@') {
                    Some("Please give the email address you will sign in with.".to_string())
                } else if get("terms") != "yes" {
                    Some("Please accept the terms and confirm you have read the privacy notice and the cancellation policy.".to_string())
                } else if get("start") != "yes" {
                    Some("Please confirm that your organisation may start right away.".to_string())
                } else if plan.link.is_empty() && crate::stripe::ids(&billing).is_none() {
                    Some("Ordering is not open yet. Write to hello@zetlyn.com and we set it up for you.".to_string())
                } else {
                    crate::billing::Book::open(&billing).and_then(|b| b.reserve(&name, &email, &title, &plan_key)).err()
                };
                // What the buyer agreed to, and when: the terms, and that the organisation starts
                // before the withdrawal period ends. Kept as long as the customer is, as the record of it.
                if refused.is_none() {
                    let line = json!({ "at": crate::iso_stamp(crate::now()), "organisation": name, "title": title, "email": email, "plan": plan_key, "terms": true, "start_before_withdrawal_ends": true,
                        "said": "I accept the terms and have read the privacy notice and the cancellation policy. I ask that my organisation starts right away, before the 14-day withdrawal period ends. If I withdraw as a consumer, I pay for the time it ran until then." });
                    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(billing.join("consents.jsonl")) {
                        let _ = std::io::Write::write_all(&mut f, format!("{line}\n").as_bytes());
                    }
                }
                match refused {
                    // Through Stripe's API where it is set up (the monthly price and the usage prices),
                    // through the plan's Payment Link where it is not.
                    None if crate::stripe::ids(&billing).is_some() => {
                        let base = crate::account::Site::load(dir).url.trim_end_matches('/').to_string();
                        match crate::stripe::checkout(&billing, &name, &email, &base) {
                            Ok(url) => return redirect(request, &url),
                            Err(e) => {
                                eprintln!("checkout: {e}");
                                said = Some("The payment could not be started just now. Please try again in a moment, or write to hello@zetlyn.com.".to_string());
                            }
                        }
                    }
                    None => {
                        let sep = if plan.link.contains('?') { '&' } else { '?' };
                        return redirect(request, &format!("{}{sep}client_reference_id={}&prefilled_email={}", plan.link, urlencode(&name), urlencode(&email)));
                    }
                    why => said = why,
                }
            }
            // Signed in already: the organisations they own, said before they order another, and
            // the address they sign in with, filled in.
            if let Some(a) = &who {
                form.entry("email".to_string()).or_insert_with(|| a.email.clone());
            }
            let register = crate::ops::register(dir).ok();
            let owned: Vec<(String, String)> = who.as_ref().map(|a| membership.orgs_of(&a.email).into_iter().filter(|(_, r)| r == "owner").map(|(o, _)| {
                let t = register.as_ref().and_then(|r| r.cells.get(&o)).map(|c| c.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| o.clone());
                (o, t)
            }).collect()).unwrap_or_default();
            let get = |k: &str| form.get(k).cloned().unwrap_or_default();
            let net: f64 = plan.price.parse().unwrap_or(0.0);
            respond(request, 200, html_kind, &page("Order Zetlyn Managed", html! {
                div.order {
                    ol.order-steps { li.on { "1 · Details" } li { "2 · Payment" } li { "3 · Your organisation" } }
                    h1 { "Order " (plan.title) }
                    @if !owned.is_empty() {
                        div.note.order-owned {
                            "You already have "
                            @for (i, (o, t)) in owned.iter().enumerate() { @if i > 0 { ", " } strong { (t) } " (" a href={"/" (o) "/"} { "zetlyn.com/" (o) } ")" }
                            ". This order adds another organisation, billed on its own at €" (plan.price) " a month plus usage."
                        }
                    }
                    @if let Some(s) = &said { div.note { (s) } }
                    div.order-grid {
                        form.order-form method="post" action="/order" {
                            label.field {
                                span.field-name { "Organisation name" }
                                input #org type="text" name="title" value=(get("title")) placeholder="Acme Research" required maxlength="80" autocomplete="organization";
                            }
                            label.field {
                                span.field-name { "Your address on Zetlyn" }
                                span.slug { span.slug-base { "zetlyn.com/" } input #slug type="text" name="name" value=(get("name")) placeholder="acme-research" required maxlength="28" pattern="[a-z0-9]([a-z0-9-]*[a-z0-9])?" autocapitalize="none" spellcheck="false"; }
                                span #slug-state .slug-state {}
                                small.dim { "Lower case letters, digits and hyphens. This is your organisation's address on Zetlyn and cannot be changed later." }
                            }
                            label.field {
                                span.field-name { "Your email" }
                                input type="email" name="email" value=(get("email")) placeholder="you@example.org" required autocomplete="email";
                                small.dim { "You sign in with it: a link comes by mail, there is no password." }
                            }
                            label.check { input type="checkbox" name="terms" value="yes" required; span { "I accept the " a href="https://zetlyn.com/terms" { "terms" } ", for a business with the " a href="https://zetlyn.com/dpa" { "data processing agreement" } ", and have read the " a href="https://zetlyn.com/privacy" { "privacy notice" } " and the " a href="https://zetlyn.com/withdrawal" { "cancellation policy" } "." } }
                            label.check { input type="checkbox" name="start" value="yes" required; span { "I ask that my organisation starts right away, before the 14-day withdrawal period ends. If I withdraw as a consumer, I pay for the time it ran until then." } }
                            button.primary.order-button type="submit" { "Order and pay" }
                            p.dim { "Next you pay securely at Stripe, which holds your card; we never see it. Your organisation is set up the moment the payment is through." }
                        }
                        aside.order-summary {
                            p.overline { (plan.title) }
                            p.order-price { strong { "€" (plan.price) } " a month plus VAT" }
                            p.dim { "€" (format!("{:.2}", net * 1.19)) " with 19 % German VAT; in other EU countries their VAT applies." }
                            ul.order-included {
                                li { (plan.storage_gb) " GB storage" }
                                li { (thousands_of(plan.reads)) " source reads a month" }
                                li { (thousands_of(plan.mails)) " mails a month" }
                                li { "Unlimited users, sources and trackers" }
                            }
                            p.order-beyond-title { "Beyond that" }
                            table.order-beyond { tbody {
                                tr { td { "Storage" } td { "€0.50 per GB a month" } }
                                tr { td { "Source reads" } td { "€1 per 10,000" } }
                                tr { td { "Mails" } td { "€1 per 1,000" } }
                            } }
                            p.dim { "At most €" (plan.cap) " a month beyond the plan; more if you ask us. Billed monthly from the first of the month; cancel any time, to the end of the month." }
                        }
                    }
                }
                script { (PreEscaped(ORDER_SCRIPT)) }
            }));
        }
        // Signing in to everything on zetlyn.com: on the main server for any address, a reader of
        // an organisation as much as its owner; on a machine of its own for its members. Back to
        // where it was asked from, a page of an organisation included.
        ["signin"] if post => {
            let mut body = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::Read::take(request.as_reader(), 16 << 10), &mut body);
            let form = parse_form(&body);
            let email = form.get("email").cloned().unwrap_or_default().trim().to_lowercase();
            let next = form.get("next").and_then(|n| crate::servetracker::next_of(n)).map(|n| format!("?next={}", urlencode(&n))).unwrap_or_default();
            let anybody = crate::ops::is_control(dir);
            if email.contains('@') && (anybody || !membership.orgs_of(&email).is_empty()) {
                let sent = accounts.ensure(&email).and_then(|a| accounts.new_link(a.id)).and_then(|raw| {
                    let mut site = crate::account::Site::load(dir);
                    if !site_url.is_empty() {
                        site.url = site_url.to_string();
                    }
                    let link = site.link(&serve::at(&format!("/signin/{raw}{next}")));
                    { let (subject, text) = crate::mail::signin_letter(&link); site.send(&email, &subject, &text) }
                });
                if let Err(e) = sent {
                    eprintln!("sign-in mail: {e}");
                }
            }
            respond(request, 200, html_kind, &page("Sign in", html! {
                div.account-hero {
                    p.overline { "Sign in" }
                    h1 { "Check your mail" }
                    p.lede { @if anybody { "A link to sign in is on its way to " (email) "." } @else { "If that address belongs to an organisation here, a link to sign in is on its way." } }
                    p.dim { "It works once, for the next 15 minutes." }
                }
            }));
        }
        ["signin"] => {
            let next = serve::params(request.url()).get("next").and_then(|n| crate::servetracker::next_of(n)).unwrap_or_default();
            respond(request, 200, html_kind, &page("Sign in", html! {
                div.account-hero {
                    p.overline { "Zetlyn" }
                    h1 { "Sign in" }
                    p.lede { "With a link sent to your address, no password. One sign-in for zetlyn.com and every organisation on it." }
                }
                form.bar method="post" action=(serve::at("/signin")) {
                    input type="hidden" name="next" value=(next);
                    input.wide type="email" name="email" placeholder="you@example.org" required;
                    button.primary type="submit" { "Send me a link" }
                }
            }))
        }
        ["signin", raw] => match accounts.spend_link(raw, crate::account::Kind::Member) {
            Some(session) => {
                let cookie = format!("zs={session}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=2592000");
                let mut response = tiny_http::Response::from_string("").with_status_code(303);
                // Back to where signing in was asked from; at a world's own name, back to the
                // world; else to the account.
                let next = serve::params(request.url()).get("next").and_then(|n| crate::servetracker::next_of(n));
                let home = match next {
                    Some(n) => n,
                    None if site_url.is_empty() => serve::at("/"),
                    None => "/".to_string(),
                };
                for (k, v) in [("Location", home), ("Set-Cookie", cookie)] {
                    if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                        response = response.with_header(h);
                    }
                }
                let _ = request.respond(response);
            }
            None => respond(request, 410, html_kind, &page("Sign in", html! { h1 { "That link is spent" } p { a href=(serve::at("/signin")) { "Ask for another" } } })),
        },
        // Signing out of everything on zetlyn.com, from the account's menu or from an
        // organisation's page, which sends people here and is sent back to.
        ["signout"] => {
            if let Some(s) = &session {
                accounts.end_session(s);
            }
            // Signed out, to the front page of zetlyn.com unless a page asked to be returned to.
            let next = serve::params(request.url()).get("next").and_then(|n| crate::servetracker::next_of(n)).unwrap_or_else(|| "/".to_string());
            let mut response = tiny_http::Response::from_string("").with_status_code(303);
            for (k, v) in [("Location", next), ("Set-Cookie", "zs=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0".to_string())] {
                if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                    response = response.with_header(h);
                }
            }
            let _ = request.respond(response);
        }
        _ => respond(request, 404, html_kind, &page("Not here", html! { h1 { "Not here" } p { a href=(serve::at("/")) { "Every tracker on this machine" } } })),
    }
}

/// What an organisation's update moved, published where its workspace says, and the hub's pages
/// written again. A source is published when it has run since it last was; a tracker when one of
/// its sources was, or its statement is not the one it published.
fn publish_moved(dir: &Path, org: &str) -> Result<usize, String> {
    let root = dir.join("orgs").join(org);
    // Every tracker on the machine opens where it answers: its organisation, in the app.
    publish_root(&root, org, |app| {
        let mut answers: BTreeMap<String, String> = BTreeMap::new();
        for o in orgs_in(dir) {
            for (name, path) in crate::tracker::scope_registry(&dir.join("orgs").join(&o).join("trackers")) {
                if let Some(d) = path.file_name() {
                    answers.insert(name, format!("{app}/{o}/trackers/{}/", d.to_string_lossy()));
                }
            }
            // A source with a page of its own in its world is that page, as a tracker is.
            let oroot = dir.join("orgs").join(&o);
            for name in crate::tracker::registry(&oroot.join("sources")).into_keys() {
                let short = name.rsplit('/').next().unwrap_or(&name).to_string();
                if shown_source(&oroot, &short).is_some() {
                    answers.insert(name, format!("{app}/{o}/sources/{short}/"));
                }
            }
        }
        answers
    })
}

/// One workspace's pass of publishing, and the hub's pages written again when anything moved.
/// `answers`, given the address trackers open under (`publish.app`), says where each one does.
pub(crate) fn publish_root(root: &Path, org: &str, answers: impl FnOnce(&str) -> BTreeMap<String, String>) -> Result<usize, String> {
    let root = root.to_path_buf();
    let Some(to) = crate::account::Site::load(&root).publish else { return Ok(0) };
    let place = crate::place::at(&to.place_for(&root))?;
    let mut published: BTreeSet<String> = BTreeSet::new();
    let mut moved = 0;
    // A private tracker is for the readers it names, and is not put where anybody can fetch it;
    // nor is a source only private trackers hold.
    let trackers: Vec<(PathBuf, TrackerDecl)> = crate::tracker::scope_registry(&root.join("trackers")).into_values().filter_map(|t| TrackerDecl::load(&t).ok().map(|d| (t, d))).collect();
    let in_public: BTreeSet<String> = trackers.iter().filter(|(_, d)| d.visibility != "private").flat_map(|(_, d)| d.members.iter().map(|m| m.dataset.clone())).collect();
    let in_private: BTreeSet<String> = trackers.iter().filter(|(_, d)| d.visibility == "private").flat_map(|(_, d)| d.members.iter().map(|m| m.dataset.clone())).collect();
    for (name, sdir) in crate::tracker::registry(&root.join("sources")) {
        if in_private.contains(&name) && !in_public.contains(&name) {
            continue;
        }
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
    for (tdir, decl) in &trackers {
        if decl.package.is_some() || decl.visibility == "private" {
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
        let answers = if to.app.is_empty() { BTreeMap::new() } else { answers(to.app.trim_end_matches('/')) };
        let opens = |r: &crate::hubpages::Row| answers.get(&r.reference()).cloned();
        // A world hosted here publishes beside a directory of worlds the machine keeps; what they
        // run elsewhere is listed beneath the catalog, as each describes it, leading there.
        let machine = root.parent().and_then(|p| p.parent()).filter(|m| m.join("orgs").is_dir());
        let docs: Vec<(String, serde_json::Value)> = machine
            .map(|m| crate::directory::list(m, "").into_iter().map(|e| (e.url, e.doc)).collect())
            .unwrap_or_default();
        let elsewhere = crate::hubpages::Elsewhere::from_documents(&docs);
        let n = crate::hubpages::render_with(place.as_ref(), &opens, &elsewhere)?;
        println!("{org}: {n} pages in {}", place.describe());
    }
    Ok(moved)
}

/// Who a request is signed in as, on a machine where everybody signs in once.
fn signed_in(request: &tiny_http::Request, accounts: &crate::account::Accounts) -> Option<String> {
    let cookie = request.headers().iter().find(|h| h.field.equiv("Cookie"))?.value.as_str().to_string();
    let session = cookie.split(';').filter_map(|p| p.trim().split_once('=')).find(|(k, _)| *k == "zs").map(|(_, v)| v.to_string())?;
    accounts.by_session(&session, crate::account::Kind::Member).map(|a| a.email).or_else(|| crate::account::remote_member(&session))
}

/// Every public tracker on the machine as a link: its title, and where it answers.
fn public_links(dir: &Path) -> Vec<(String, String)> {
    public_trackers(dir).into_iter().map(|(org, name, decl)| (decl.title, format!("/{org}/trackers/{name}/"))).collect()
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

#[cfg(test)]
mod tests {

    #[test]
    fn a_world_paid_for_is_made_for_whoever_paid_and_runs_on_its_plan() {
        let dir = std::env::temp_dir().join(format!("zetlyn-paid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let billing = dir.join(super::BILLING);
        std::fs::create_dir_all(&billing).unwrap();
        std::fs::create_dir_all(dir.join("orgs")).unwrap();
        std::fs::write(billing.join("plans.yaml"), "currency: €\nplans:\n  solo:\n    sources: 10\n    every: 1h\n    mails: 500\n    price: \"19\"\n    link: https://buy.stripe.com/test_solo\n    link_id: plink_solo\n    trial_days: 14\n  team:\n    sources: 50\n    every: 15m\n    mails: 5000\n    price: \"49\"\n    link_id: plink_team\n    domain: true\n").unwrap();
        let book = crate::billing::Book::open(&billing).unwrap();
        book.reserve("circle", "ann@example.org", "Our reading circle", "solo").unwrap();
        // Somebody else may not take the name while it is held.
        assert!(book.reserve("circle", "eve@example.org", "Mine", "solo").is_err());
        let checkout = serde_json::json!({ "id": "evt_paid", "type": "checkout.session.completed", "data": { "object": {
            "client_reference_id": "circle", "customer": "cus_ann", "subscription": "sub_ann", "payment_link": "plink_solo",
            "customer_details": { "email": "Ann@Example.org" } } } });
        assert!(crate::billing::apply(&billing, &checkout).unwrap().contains("circle: trialing on solo"));
        super::provision(&dir, "circle", "ann@example.org").unwrap();
        assert_eq!(crate::account::Site::load(&dir.join("orgs/circle")).title, "Our reading circle");
        assert!(super::Membership::load(&dir).orgs["circle"].iter().any(|m| m.email == "ann@example.org" && m.role == "owner"));
        // Its plan's limits; a world nobody pays for is the operator's, and unlimited.
        let (go, limits) = super::world_terms(&dir, "circle");
        assert!(go && limits.sources == Some(10) && limits.every == 3600);
        assert!(super::world_terms(&dir, "zetlyn").1.sources.is_none());
        // A domain of its own is the team plan's.
        assert!(!super::domain_allowed(&dir, "circle"));
        assert!(super::domain_allowed(&dir, "zetlyn"));
        let mut c = book.get("circle").unwrap();
        c.plan = "team".into();
        book.set(&c, None).unwrap();
        assert!(super::domain_allowed(&dir, "circle"));
        // Past what was paid and past the grace, it is read no further.
        c.state = "past_due".into();
        c.paid_until = Some(crate::iso_date(crate::now() - 8 * 86_400));
        book.set(&c, None).unwrap();
        assert!(!super::world_terms(&dir, "circle").0);
        c.paid_until = Some(crate::iso_date(crate::now() - 3 * 86_400));
        book.set(&c, None).unwrap();
        assert!(super::world_terms(&dir, "circle").0, "within the seven days of grace");
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn an_organisation_may_not_take_a_path_of_the_site_but_one_already_made_stays() {
        let dir = std::env::temp_dir().join(format!("zetlyn-hosting-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let args = |name: &str| ["hosting", "org", dir.to_str().unwrap(), name].iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(super::hosting(&args("docs")).unwrap_err().contains("reserved"));
        assert!(super::hosting(&args("app")).unwrap_err().contains("reserved"));
        assert!(!dir.join("orgs/docs").exists());
        super::hosting(&args("acme")).unwrap();
        assert!(dir.join("orgs/acme/sources").is_dir());
        // `zetlyn` was made before the list said so, and is not refused for being there.
        std::fs::create_dir_all(dir.join("orgs/zetlyn")).unwrap();
        super::hosting(&args("zetlyn")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A free port on this machine, for a server a test starts.
    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    fn ask(url: &str, cookie: Option<&str>, form: Option<&str>) -> (u16, String, Option<String>) {
        let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).max_redirects(0).build().into();
        let mut r = match form {
            Some(body) => {
                let mut q = agent.post(url).header("Content-Type", "application/x-www-form-urlencoded");
                if let Some(c) = cookie {
                    q = q.header("Cookie", c);
                }
                q.send(body).unwrap()
            }
            None => {
                let mut q = agent.get(url);
                if let Some(c) = cookie {
                    q = q.header("Cookie", c);
                }
                q.call().unwrap()
            }
        };
        let cookie = r.headers().get("set-cookie").map(|v| v.to_str().unwrap_or("").to_string());
        (r.status().as_u16(), r.body_mut().read_to_string().unwrap_or_default(), cookie)
    }

    #[test]
    fn a_world_on_its_own_domain_is_its_owners_to_run_and_everybody_elses_to_read() {
        let root = std::env::temp_dir().join(format!("zetlyn-world-serve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sources")).unwrap();
        std::fs::create_dir_all(root.join("trackers")).unwrap();
        std::fs::write(root.join("workspace.yaml"), "title: Car prices\nurl: https://prices.example\nowners: [ann@example.org]\n").unwrap();
        // Nobody named to run it is refused before anything is served.
        let nobody = std::env::temp_dir().join(format!("zetlyn-world-serve-nobody-{}", std::process::id()));
        std::fs::create_dir_all(&nobody).unwrap();
        std::fs::write(nobody.join("workspace.yaml"), "title: x\n").unwrap();
        let args = |dir: &std::path::Path, addr: &str| ["world", "serve", dir.to_str().unwrap(), "--addr", addr].iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(super::world_serve(&args(&nobody, "127.0.0.1:0")).unwrap_err().contains("owners"));

        let addr = format!("127.0.0.1:{}", free_port());
        let a = args(&root, &addr);
        // Ends with the test process; nothing outlives it.
        std::thread::spawn(move || super::world_serve(&a));
        let base = format!("http://{addr}");
        let mut up = false;
        for _ in 0..50 {
            if std::net::TcpStream::connect(&addr).is_ok() {
                up = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(up, "the world never answered");

        // A visitor reads it, at its root.
        let (status, page, _) = ask(&format!("{base}/"), None, None);
        assert_eq!(status, 200);
        assert!(page.contains("Car prices") && page.contains("Sign in"), "{page}");
        assert!(!page.contains("What do you want to track?"));
        // Somebody who is not an owner signs in too, as a reader of it, and runs nothing.
        let accounts = crate::account::Accounts::open(&root).unwrap();
        let (status, _, _) = ask(&format!("{base}/signin"), None, Some("email=mallory%40example.org"));
        assert_eq!(status, 200);
        let mallory = accounts.by_email("mallory@example.org").expect("a reader, with a link of their own");
        let (status, _, cookie) = ask(&format!("{base}/signin/{}", accounts.new_link(mallory.id).unwrap()), None, None);
        assert_eq!(status, 303);
        assert!(cookie.as_deref().is_some_and(|c| c.starts_with("zr=")), "a reader's cookie and no member's: {cookie:?}");
        // The owner follows a link and runs it.
        let ann = accounts.ensure("ann@example.org").unwrap();
        let (status, _, cookie) = ask(&format!("{base}/signin/{}", accounts.new_link(ann.id).unwrap()), None, None);
        assert_eq!(status, 303);
        let cookie = cookie.unwrap();
        assert!(cookie.starts_with("zs=") && cookie.contains("Path=/;"), "{cookie}");
        let session = cookie.split(';').next().unwrap().to_string();
        let (_, page, _) = ask(&format!("{base}/"), Some(&session), None);
        assert!(page.contains("What do you want to track?"), "{page}");
        // An owner added to the file is one at once.
        std::fs::write(root.join("workspace.yaml"), "title: Car prices\nurl: https://prices.example\nowners: [ann@example.org, ben@example.org]\n").unwrap();
        let ben = accounts.ensure("ben@example.org").unwrap();
        let (_, _, cookie) = ask(&format!("{base}/signin/{}", accounts.new_link(ben.id).unwrap()), None, None);
        let (_, page, _) = ask(&format!("{base}/"), Some(cookie.unwrap().split(';').next().unwrap()), None);
        assert!(page.contains("What do you want to track?"));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&nobody);
    }

    /// One request by hand, so the Host it says is exactly the one given: status, headers, body.
    fn raw(port: u16, method: &str, host: &str, path: &str, cookie: Option<&str>) -> (u16, String, Vec<u8>) {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let cookie = cookie.map(|c| format!("Cookie: {c}\r\n")).unwrap_or_default();
        write!(s, "{method} {path} HTTP/1.1\r\nHost: {host}\r\n{cookie}Content-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        let mut all = Vec::new();
        s.read_to_end(&mut all).unwrap();
        let split = all.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&all[..split]).to_string();
        let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        let mut body = all[split + 4..].to_vec();
        if head.to_lowercase().contains("transfer-encoding: chunked") {
            // Unchunked, for a file sent as it is read.
            let mut out = Vec::new();
            let mut rest = &body[..];
            loop {
                let end = rest.windows(2).position(|w| w == b"\r\n").unwrap();
                let size = usize::from_str_radix(String::from_utf8_lossy(&rest[..end]).trim(), 16).unwrap();
                if size == 0 {
                    break;
                }
                out.extend_from_slice(&rest[end + 2..end + 2 + size]);
                rest = &rest[end + 4 + size..];
            }
            body = out;
        }
        (status, head, body)
    }

    #[test]
    fn a_hosted_world_on_a_domain_of_its_own_is_whole_there_and_its_owner_can_take_it_away() {
        let dir = std::env::temp_dir().join(format!("zetlyn-own-domain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let port = free_port();
        let machine = format!("http://127.0.0.1:{port}");
        std::fs::write({ std::fs::create_dir_all(&dir).unwrap(); dir.join("workspace.yaml") }, format!("title: The machine\nurl: {machine}\n")).unwrap();
        for (org, extra) in [("acme", "domain: acme.example\n"), ("plain", "")] {
            std::fs::create_dir_all(dir.join(format!("orgs/{org}/sources"))).unwrap();
            std::fs::create_dir_all(dir.join(format!("orgs/{org}/trackers"))).unwrap();
            std::fs::write(dir.join(format!("orgs/{org}/workspace.yaml")), format!("title: {org} world\n{extra}")).unwrap();
        }
        std::fs::write(dir.join("members.yaml"), "plain:\n- email: ann@example.org\n  role: owner\n- email: ed@example.org\n  role: editor\n- email: rita@example.org\n  role: reader\n").unwrap();
        let args: Vec<String> = ["hosting", "serve", dir.to_str().unwrap(), "--addr", &format!("127.0.0.1:{port}")].iter().map(|s| s.to_string()).collect();
        // Ends with the test process; nothing outlives it.
        std::thread::spawn(move || super::hosting(&args));
        for _ in 0..50 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let host = format!("127.0.0.1:{port}");

        // At its own name, whole, at the root.
        let (status, _, body) = raw(port, "GET", "acme.example", "/", None);
        assert_eq!(status, 200);
        assert!(String::from_utf8_lossy(&body).contains("acme world"));
        let (_, _, disc) = raw(port, "GET", "acme.example", "/.well-known/openid-configuration", None);
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&disc).unwrap()["issuer"], "https://acme.example");
        let (_, _, doc) = raw(port, "GET", "acme.example", "/.well-known/zetlyn.json", None);
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&doc).unwrap()["world"], "https://acme.example");
        // Under the machine's name, it sends everybody to its own.
        let (status, head, _) = raw(port, "GET", &host, "/acme/trackers/x/?q=1", None);
        assert_eq!(status, 302);
        assert!(head.contains("Location: https://acme.example/trackers/x/?q=1"), "{head}");
        // Where a world and its trackers were until 2026-10-05, and where it was until 10-06, a page
        // says where it is now.
        let (status, head, _) = raw(port, "GET", &host, "/plain/t/x/?q=1", None);
        assert_eq!(status, 301);
        assert!(head.contains("Location: /plain/trackers/x/?q=1"), "{head}");
        let (status, head, _) = raw(port, "GET", &host, "/worlds/plain", None);
        assert_eq!(status, 301);
        assert!(head.lines().any(|l| l.trim() == "Location: /plain"), "{head}");
        let (status, head, _) = raw(port, "GET", &host, "/worlds/plain/t/x/", None);
        assert_eq!(status, 301);
        assert!(head.contains("Location: /plain/trackers/x/"), "{head}");
        let (status, head, _) = raw(port, "GET", &host, "/worlds/plain/trackers/x/?q=1", None);
        assert_eq!(status, 301);
        assert!(head.contains("Location: /plain/trackers/x/?q=1"), "{head}");
        // A name nobody here said is theirs is nothing, and no certificate is taken for it.
        assert_eq!(raw(port, "GET", "elsewhere.example", "/", None).0, 421);
        assert_eq!(raw(port, "GET", &host, "/app/domain-check?domain=acme.example", None).0, 200);
        assert_eq!(raw(port, "GET", &host, "/app/domain-check?domain=elsewhere.example", None).0, 404);
        // The examples the docs point at, made as they are asked for.
        assert_eq!(raw(port, "GET", &host, "/examples/leafline-books.csv", None).0, 200);
        assert!(String::from_utf8_lossy(&raw(port, "GET", &host, "/examples/lindenhof/", None).2).contains("Lindenhof"));
        assert_eq!(raw(port, "GET", &host, "/examples/nothing", None).0, 404);

        // Its owner takes all of it away, and may not say it went somewhere that is no world.
        let accounts = crate::account::Accounts::open(&dir).unwrap();
        let ann = accounts.ensure("ann@example.org").unwrap();
        let zs = format!("zs={}", accounts.new_session(ann.id, crate::account::Kind::Member).unwrap());
        assert_ne!(raw(port, "GET", &host, "/plain/export.tar.gz", None).0, 200, "only its owner");
        // An editor edits, and does not take the world away; a reader is not in the app at all.
        let ed = accounts.ensure("ed@example.org").unwrap();
        let zs_ed = format!("zs={}", accounts.new_session(ed.id, crate::account::Kind::Member).unwrap());
        assert_eq!(raw(port, "GET", &host, "/plain/export.tar.gz", Some(&zs_ed)).0, 403);
        assert_eq!(raw(port, "GET", &host, "/plain/settings", Some(&zs_ed)).0, 200);
        let rita = accounts.ensure("rita@example.org").unwrap();
        let zs_rita = format!("zs={}", accounts.new_session(rita.id, crate::account::Kind::Member).unwrap());
        assert_ne!(raw(port, "GET", &host, "/plain/settings", Some(&zs_rita)).0, 200);
        // Nor is anybody whose session was made for reading.
        let zr = format!("zs={}", accounts.new_session(ann.id, crate::account::Kind::Reader).unwrap());
        assert_ne!(raw(port, "GET", &host, "/plain/export.tar.gz", Some(&zr)).0, 200);
        let (status, head, body) = raw(port, "GET", &host, "/plain/export.tar.gz", Some(&zs));
        assert_eq!(status, 200, "{head}");
        assert!(head.contains("attachment; filename=\"plain-"), "{head}");
        let mut names = Vec::new();
        for e in tar::Archive::new(flate2::read::GzDecoder::new(&body[..])).entries().unwrap() {
            names.push(e.unwrap().path().unwrap().to_string_lossy().into_owned());
        }
        assert!(names.contains(&"world/workspace.yaml".to_string()) && names.contains(&"EXPORT.json".to_string()), "{names:?}");
        let (status, head, _) = {
            use std::io::{Read, Write};
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            let body = "to=http%3A%2F%2F127.0.0.1%3A1";
            write!(s, "POST /plain/settings/moved HTTP/1.1\r\nHost: {host}\r\nCookie: {zs}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            let mut all = String::new();
            s.read_to_string(&mut all).unwrap();
            (all.split_whitespace().nth(1).unwrap().parse::<u16>().unwrap(), all.clone(), ())
        };
        assert_eq!(status, 303);
        assert!(crate::account::Site::load(&dir.join("orgs/plain")).moved_to.is_empty(), "{head}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// A source a world shows anybody: its short name, its folder, its title, and the public trackers
/// that hold it, by folder and title.
#[derive(Clone)]
struct Shown {
    short: String,
    dir: PathBuf,
    title: String,
    holding: Vec<(String, String)>,
}

/// The sources a world shows anybody: one that said it may be shown, in a tracker that is not
/// private. Every page a visitor sees lists them, so they are read off the declarations alone and
/// kept a minute; opening each store for each page was seconds a page (2026-10-06).
fn shown_sources(root: &Path) -> Vec<Shown> {
    static KEPT: std::sync::OnceLock<Mutex<BTreeMap<PathBuf, (i64, Vec<Shown>)>>> = std::sync::OnceLock::new();
    let kept = KEPT.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Some((at, list)) = kept.lock().unwrap_or_else(|e| e.into_inner()).get(root) {
        if crate::now() - at < 60 {
            return list.clone();
        }
    }
    let trackers: Vec<(String, TrackerDecl)> = std::fs::read_dir(root.join("trackers"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), TrackerDecl::load(&e.path()).ok()?)))
        .filter(|(_, t)| t.visibility != "private")
        .collect();
    let list: Vec<Shown> = crate::tracker::registry(&root.join("sources"))
        .into_iter()
        .filter_map(|(name, dir)| {
            let d = crate::sourcedecl::SourceDecl::load(&dir).ok()?;
            if !matches!(d.licence.republish.as_str(), "yes" | "summary") {
                return None;
            }
            let mut holding: Vec<(String, String)> = trackers
                .iter()
                .filter(|(_, t)| t.members.iter().any(|m| m.dataset == name))
                .map(|(folder, t)| (folder.clone(), if t.title.is_empty() { folder.clone() } else { t.title.clone() }))
                .collect();
            holding.sort_by(|a, b| a.1.cmp(&b.1));
            let short = name.rsplit('/').next().unwrap_or(&name).to_string();
            let title = if d.title.is_empty() { short.clone() } else { d.title.clone() };
            (!holding.is_empty()).then_some(Shown { short, dir, title, holding })
        })
        .collect();
    kept.lock().unwrap_or_else(|e| e.into_inner()).insert(root.to_path_buf(), (crate::now(), list.clone()));
    list
}

/// One of them, by the name it is published under, `cve-redhat` of `zetlyn/cve-redhat`, which is
/// how the hub and every link say it; or by its folder, `redhat`.
fn shown_source(root: &Path, asked: &str) -> Option<Shown> {
    shown_sources(root)
        .into_iter()
        .find(|s| s.short == asked || s.dir.file_name().is_some_and(|f| f == asked))
}

/// A source's own page in its world, for anybody, where the source said it may be shown and a
/// public tracker of the world holds it: what it is, where it reads, what one claim carries, the
/// trackers it is in, and its versions on the hub. None for any other, which a visitor is then
/// asked to sign in for, as before, so the page says nothing about a source it may not show.
fn public_source_page(root: &Path, base: &str, dir_name: &str) -> Option<String> {
    let shown = shown_source(root, dir_name)?;
    let ds = Source::open(&shown.dir).ok()?;
    let d = &ds.decl;
    let described = ds.describe();
    let site = crate::account::Site::load(root);
    let hub = site.publish.as_ref().map(|p| p.read_at.trim().trim_end_matches('/').to_string()).filter(|h| !h.is_empty());
    let entry = hub.as_deref().and_then(|h| crate::place::at(h).ok()).and_then(|place| crate::hubpages::read_entry(place.as_ref(), "sources", &d.name));
    let title = if d.title.is_empty() { d.name.clone() } else { d.title.clone() };
    let finished = described["last_update"]["finished"].as_str().unwrap_or("");
    let properties = described["properties"].as_array().cloned().unwrap_or_default();
    let body = html! {
        p.overline { "Source" @if !site.title.is_empty() { " · run by " (site.title) } }
        h1 { (title) }
        @if !d.about.is_empty() { p.lede { (d.about) } }
        div.meta {
            span.(if described["state"] == "current" { "current" } else { "partial" }) { (described["state"].as_str().unwrap_or("")) }
            span { (described["claims"].as_u64().unwrap_or(0)) " claims" }
            @if !d.kind.is_empty() { span { (d.kind) } }
            @if !finished.is_empty() { span { "read " (finished.get(..16).unwrap_or(finished).replace('T', " ")) " UTC" } }
            @if let Some(every) = &d.schedule.every { span { "every " (every) } }
        }
        h2 { "Where it reads" }
        p { code { (d.source.address()) } }
        p.dim {
            @match d.licence.republish.as_str() { "yes" => { "It may be republished." } _ => { "Its titles and values may be shown, not its text." } }
            @if !d.licence.terms.is_empty() { " " a href=(d.licence.terms) { "Its terms" } "." }
        }
        h2 { "In" }
        ul { @for (name, t) in &shown.holding { li { a href={(base) "/trackers/" (name) "/"} { (t) } } } }
        @if !properties.is_empty() {
            h2 { "What one claim carries" }
            table { thead { tr { th { "Property" } th { "Type" } th { "Claims" } } }
                tbody { @for p in &properties { tr {
                    td { code { (p["name"].as_str().unwrap_or("")) } }
                    td.dim { (p["type"].as_str().unwrap_or("")) }
                    td { (p["claims"].as_u64().unwrap_or(0)) }
                } } }
            }
        }
        @if let Some((row, held)) = &entry {
            h2 { "Take a copy" }
            pre { code { "zetlyn source subscribe " (row.reference()) } }
            p.dim { "Its claims and their receipts, signed; an update fetches only what changed." }
            h2 { "Versions" }
            table { thead { tr { th { "Version" } th { "Published" } th { "Claims" } th {} } }
                tbody { @for (at, v, claims) in held { tr {
                    td { code { (v.get(..12).unwrap_or(v)) } }
                    td { (crate::iso_date(*at)) }
                    td { @if *claims > 0 { (claims) } }
                    td { @if *v == row.version { span.chip.on { (row.tag) } } }
                } } }
            }
        }
    };
    Some(page(&title, body))
}

/// 25000 as 25,000.
fn thousands_of(n: u64) -> String {
    crate::web::thousands(n as usize)
}

/// The order page's address field: suggested from the organisation's name until it is typed in
/// itself, kept to what an address may hold, and checked as it changes.
const ORDER_SCRIPT: &str = r#"(function () {
  var org = document.getElementById("org"), slug = document.getElementById("slug"), state = document.getElementById("slug-state");
  if (!org || !slug || !window.fetch) return;
  var typed = slug.value !== "", timer;
  function address(s) {
    return s.toLowerCase().replace(/ä/g, "ae").replace(/ö/g, "oe").replace(/ü/g, "ue").replace(/ß/g, "ss")
      .normalize("NFKD").replace(/[̀-ͯ]/g, "").replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 28).replace(/-+$/, "");
  }
  function check() {
    clearTimeout(timer);
    var v = slug.value;
    if (!v) { state.textContent = ""; state.className = "slug-state"; return; }
    timer = setTimeout(function () {
      fetch("/order/check?name=" + encodeURIComponent(v), { cache: "no-store" })
        .then(function (r) { return r.json(); })
        .then(function (j) { if (slug.value !== v) return; state.textContent = (j.ok ? "✓ " : "✗ ") + "zetlyn.com/" + v + " is " + j.said; state.className = "slug-state " + (j.ok ? "ok" : "no"); })
        .catch(function () {});
    }, 250);
  }
  org.addEventListener("input", function () { if (!typed) { slug.value = address(org.value); check(); } });
  slug.addEventListener("input", function () {
    typed = slug.value !== "";
    var kept = slug.value.toLowerCase().replace(/[^a-z0-9-]/g, "").slice(0, 28);
    if (kept !== slug.value) slug.value = kept;
    check();
  });
  if (slug.value) check();
})();"#;

/// One organisation's plan as its owner sees it: which, in what state, paid until when, and this
/// month's usage against what the plan includes.
fn plan_summary(dir: &Path, billing: &Path, org: &str, c: &crate::billing::Customer, grace: i64) -> Markup {
    let state = match c.state.as_str() { "trialing" => "Free trial", "active" => "Active", "past_due" => "Payment failed, being tried again", "cancelled" => "Cancelled", other => other }.to_string();
    let plan = crate::billing::plans(billing).ok().and_then(|(_, p)| p.get(&c.plan).cloned()).unwrap_or_default();
    let name = if !plan.title.is_empty() { plan.title.clone() } else if c.plan.is_empty() { "—".to_string() } else { c.plan.clone() };
    html! {
        div.account-plan {
            span { (name) " · " (state) }
            @if let Some(u) = &c.paid_until { span.dim { (if c.state == "trialing" { " · free until " } else { " · paid until " }) (u) } }
            @if !c.in_good_standing(grace) { " " span.chip.on { "not updated" } }
        }
        @if plan.storage_gb > 0 {
            @let u = crate::ops::usage_of(dir, org);
            @let (so, ro, mo) = crate::ops::overage(&u, &plan);
            @let days = crate::iso_date(crate::now()).get(8..10).and_then(|d| d.parse::<u64>().ok()).unwrap_or(1).max(1);
            table.account-usage { tbody {
                tr { td.dim { "Storage" } td { (format!("{:.2}", u.mb_days as f64 / days as f64 / 1024.0)) " GB" } td.dim { "of " (plan.storage_gb) " GB" } }
                tr { td.dim { "Source reads" } td { (thousands_of(u.reads)) } td.dim { "of " (thousands_of(plan.reads)) } }
                tr { td.dim { "Mails" } td { (thousands_of(u.mails)) } td.dim { "of " (thousands_of(plan.mails)) } }
            } }
            @if so + ro + mo > 0.0 { span.dim { "Beyond the plan this month: €" (format!("{:.2}", so + ro + mo)) " of at most €" (plan.cap) } }
        }
    }
}

/// A hosted world's plan and its month so far, in its own settings: what it used against what the
/// plan includes, what went beyond it, and which sources the reads went to. From the cell's own
/// counts, and the plan and storage its main server last wrote into `cell.yaml`.
fn cell_usage_section(sources: &[(String, PathBuf)]) -> Markup {
    let Some(cell) = crate::usage::cell_dir() else { return html! {} };
    let Some(terms) = crate::cell::terms(&cell) else { return html! {} };
    let Some(shown) = terms.plan.clone() else { return html! {} };
    let month = crate::usage::month();
    let reads = crate::usage::used(&cell, crate::usage::READS, &month);
    let mails = crate::usage::used(&cell, crate::usage::MAILS, &month);
    let days = crate::iso_date(crate::now()).get(8..10).and_then(|d| d.parse::<u64>().ok()).unwrap_or(1).max(1);
    let u = crate::ops::Usage { month: month.clone(), mb_days: shown.mb_days, reads, mails, ..crate::ops::Usage::default() };
    let plan = crate::billing::Plan { storage_gb: shown.storage_gb, reads: shown.reads, mails: shown.mails, cap: shown.cap, ..crate::billing::Plan::default() };
    let (so, ro, mo) = crate::ops::overage(&u, &plan);
    let paused = terms.reads.is_some_and(|c| reads >= c) || terms.mails.is_some_and(|c| mails >= c);
    let by_source = crate::usage::reads_by_source(&cell, &month);
    let title_of = |name: &str| {
        sources
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, d)| Source::open(d).ok())
            .map(|ds| ds.decl.title)
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| name.to_string())
    };
    let percent = |used: u64, of: u64| if of == 0 { 0 } else { (used * 100 / of).min(100) };
    let storage_gb = shown.mb_days as f64 / days as f64 / 1024.0;
    html! {
        h1 #usage { "Plan and usage" }
        p.lede { (shown.title) ", " (month_words(&month)) " so far." }
        @if !terms.active { div.note { "This world's plan is not paid up: its sources are not read until it is. " a href="/account/billing" { "Billing" } } }
        @else if paused { div.note { "The spending limit of €" (shown.cap) " for this month is reached: sources are not read and no mails are sent until the month ends. Write to " a href="mailto:hello@zetlyn.com" { "hello@zetlyn.com" } " to raise it." } }
        table.usage-meter { tbody {
            tr { td { "Storage" } td.num { (format!("{storage_gb:.2}")) " GB" } td.dim { "of " (shown.storage_gb) " GB, as the month's average" } td { meter min="0" max="100" value=(percent((storage_gb * 1024.0) as u64, shown.storage_gb * 1024)) {} } }
            tr { td { "Source reads" } td.num { (thousands_of(reads)) } td.dim { "of " (thousands_of(shown.reads)) } td { meter min="0" max="100" value=(percent(reads, shown.reads)) {} } }
            tr { td { "Mails" } td.num { (thousands_of(mails)) } td.dim { "of " (thousands_of(shown.mails)) } td { meter min="0" max="100" value=(percent(mails, shown.mails)) {} } }
        } }
        p.dim {
            @if so + ro + mo > 0.0 { "Beyond the plan this month: €" (format!("{:.2}", so + ro + mo)) ", of at most €" (shown.cap) ". " }
            @else { "Nothing beyond the plan this month. " }
            "Storage is counted once a day. Invoices, payment method and cancelling: " a href="/account/billing" { "Billing" } "."
        }
        @if !by_source.is_empty() {
            details open[by_source.len() <= 8] {
                summary { "Reads by source" }
                table { thead { tr { th { "Source" } th.num { "Reads" } th.num { "Share" } } } tbody {
                    @for (name, n) in &by_source {
                        tr { td { strong { (title_of(name)) } div.why.mono { (name) } } td.num { (thousands_of(*n)) } td.num { (if reads == 0 { 0 } else { n * 100 / reads }) "%" } }
                    }
                } }
                p.dim { "A read is one update of one source. A source read less often uses fewer: set how often under Each source below." }
            }
        }
    }
}

/// `2026-10` as `October 2026`.
fn month_words(month: &str) -> String {
    const NAMES: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let m: usize = month.get(5..7).and_then(|m| m.parse().ok()).unwrap_or(1);
    format!("{} {}", NAMES.get(m.wrapping_sub(1)).unwrap_or(&""), month.get(..4).unwrap_or(""))
}

/// The import form: the archive sent as it is (PUT), what happens said as it does.
const IMPORT_SCRIPT: &str = r#"(function () {
  var form = document.getElementById("import-form"), file = document.getElementById("import-file"), said = document.getElementById("import-said");
  if (!form || !window.XMLHttpRequest) return;
  form.addEventListener("submit", function (e) {
    e.preventDefault();
    var f = file.files && file.files[0];
    if (!f) return;
    if (!confirm("Replace this world with " + f.name + "? A snapshot of it is taken first.")) return;
    var x = new XMLHttpRequest();
    x.open("PUT", form.getAttribute("data-to"));
    x.upload.onprogress = function (p) { if (p.lengthComputable) said.textContent = "Uploading, " + Math.round(p.loaded * 100 / p.total) + " %"; };
    x.onload = function () { said.textContent = x.responseText; if (x.status === 200) { form.hidden = true; } else { form.querySelector("button").disabled = false; } };
    x.onerror = function () { said.textContent = "The upload broke off. Please try again."; form.querySelector("button").disabled = false; };
    form.querySelector("button").disabled = true;
    x.send(f);
  });
})();"#;

/// `licence: { republish }` as an owner reads it.
fn republish_words(r: &str) -> &'static str {
    match r {
        "yes" => "shown in full on public pages",
        "summary" => "titles, values and a link on public pages",
        _ => "not shown on public pages",
    }
}
