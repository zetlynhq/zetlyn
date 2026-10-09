//! One story, told in every way a world runs (zetlyn-ops/USECASES.md; each test names the cases it
//! covers): the app on one's own machine (L), on a server of its own (`world serve`, S), on a
//! machine of many worlds (`hosting serve`, P), and as a cell of zetlyn.com (M: a cell's process
//! beside a main server it asks who is signed in, as there). Each is the binary on ports of its own
//! over a copy of `tests/fixtures/workspace`; nobody signs in by mail, the link is read off what the
//! process printed.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

const OWNER: &str = "owner@example.org";
const MEMBERS: [&str; 4] = ["kev", "vendor-a", "vendor-b", "exploits"];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    /// The app on one's own machine: nobody signs in, whoever is at it owns it.
    L,
    /// `zetlyn world serve`: one world on its own address.
    S,
    /// `zetlyn hosting serve`: many worlds on one machine, this one at `/acme`.
    P,
    /// A cell of zetlyn.com: `hosting serve` as a cell, signing in at the main server.
    M,
}

/// Where a world runs for others.
const SERVED: [Mode; 3] = [Mode::S, Mode::P, Mode::M];

/// A process of the binary, answering on its port until it is dropped.
struct Proc {
    child: Child,
    port: u16,
    log: PathBuf,
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    static NEXT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
    (0..400)
        .map(|_| 20_000 + (std::process::id() % 1_000) as u16 * 8 + NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst) % 400)
        .find(|p| std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok())
        .expect("a free port")
}

/// The binary, answering on `port`: started again on another where that one was taken meanwhile by
/// a process beside this test (a port free when it was picked is not free for good). The port it
/// answers on is the one in what comes back.
fn spawn(args: &[&str], env: &[(&str, String)], cwd: Option<&Path>, log: PathBuf, port: u16) -> Proc {
    let mut port = port;
    let mut args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    for _ in 0..5 {
        let out = std::fs::File::create(&log).unwrap();
        let mut c = Command::new(env!("CARGO_BIN_EXE_zetlyn"));
        c.args(&args).stdin(Stdio::null()).stdout(Stdio::from(out.try_clone().unwrap())).stderr(Stdio::from(out));
        if let Some(d) = cwd {
            c.current_dir(d);
        }
        for (k, v) in env {
            c.env(k, v);
        }
        let mut child = c.spawn().expect("the binary runs");
        // Answering on the port, and this process the one answering.
        let mine = format!(":{port}/");
        let mut taken = false;
        for _ in 0..200 {
            if let Ok(Some(status)) = child.try_wait() {
                let said = std::fs::read_to_string(&log).unwrap_or_default();
                if said.contains("already in use") {
                    taken = true;
                    break;
                }
                panic!("{args:?} ended ({status}) before answering on {port}: {said}");
            }
            let said = std::fs::read_to_string(&log).unwrap_or_default();
            if said.contains(&mine) && TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return Proc { child, port, log };
            }
            // The app on one's own machine takes the next free port where its own is taken, and
            // says which: that one, then.
            if let Some(other) = said.split("on http://127.0.0.1:").nth(1).and_then(|r| r.split('/').next()).and_then(|p| p.parse::<u16>().ok()) {
                if other != port && TcpStream::connect(("127.0.0.1", other)).is_ok() {
                    return Proc { child, port: other, log };
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if !taken {
            panic!("{args:?} did not answer on {port}: {}", std::fs::read_to_string(&log).unwrap_or_default());
        }
        let next = free_port();
        let (old, new) = (port.to_string(), next.to_string());
        args = args.iter().map(|a| if a.ends_with(&format!(":{old}")) || *a == old { a.replace(&old, &new) } else { a.clone() }).collect();
        port = next;
    }
    panic!("{args:?}: no free port in five tries");
}

/// The binary run as a person runs it in a terminal: whether it went, and what it printed.
fn z(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_zetlyn")).args(args).output().expect("the binary runs");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), target).unwrap();
        }
    }
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

/// One request, by hand: status, headers, body. No redirect is followed.
fn http_raw(port: u16, method: &str, path: &str, cookie: &str, kind: &str, body: &[u8]) -> (u16, String, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(30))).unwrap();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    if !cookie.is_empty() {
        head.push_str(&format!("Cookie: {cookie}\r\n"));
    }
    if method != "GET" {
        head.push_str(&format!("Content-Type: {kind}\r\nContent-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(body).unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (h, b) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = h.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    (status, h.to_string(), b.to_string())
}

fn http(port: u16, method: &str, path: &str, cookie: &str, form: &str) -> (u16, String, String) {
    http_raw(port, method, path, cookie, "application/x-www-form-urlencoded", form.as_bytes())
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The cookies a response sets, as one `Cookie:` header says them back.
fn cookies_of(headers: &str) -> Vec<String> {
    headers
        .lines()
        .filter_map(|l| l.strip_prefix("Set-Cookie: ").or_else(|| l.strip_prefix("set-cookie: ")))
        .filter_map(|c| c.split(';').next())
        .filter(|kv| kv.split_once('=').is_some_and(|(_, v)| !v.is_empty()))
        .map(str::to_string)
        .collect()
}

/// The last sign-in link a process printed, from `signin/` on.
fn last_link(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let at = text.rfind("signin/").unwrap_or_else(|| panic!("no sign-in link in {}:\n{text}", log.display()));
    text[at..].split(|c: char| c.is_whitespace() || c == '?').next().unwrap().to_string()
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..300 {
        if done() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("{what}: not within 15 seconds");
}

/// A world called Acme, run one way, until it is dropped.
struct World {
    mode: Mode,
    tmp: PathBuf,
    /// Its workspace: what an owner would see in a terminal.
    root: PathBuf,
    /// What runs it: the hosting directory for P and M, the workspace itself for L and S.
    host: PathBuf,
    /// Where its pages are on the port that answers them: "" or "/acme".
    base: String,
    /// The process answering its pages, and for M the main server beside it.
    web: Option<Proc>,
    main: Option<Proc>,
    keep: bool,
}

/// The fixture as a world called Acme, its sources read once.
fn acme(root: &Path, extra: &str) {
    copy(&fixture("workspace"), root);
    std::fs::write(root.join("workspace.yaml"), format!("title: Acme\n{extra}")).unwrap();
    for m in MEMBERS {
        let (ok, said) = z(&["source", "update", &root.join("sources").join(m).display().to_string()]);
        assert!(ok, "{said}");
    }
}

impl World {
    fn start(mode: Mode, name: &str) -> World {
        let tmp = std::env::temp_dir().join(format!("zetlyn-modes-{}-{name}-{mode:?}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        World::start_in(mode, tmp, None, false)
    }

    /// Run `mode` in `tmp`, over the workspace `existing` where one is given, as it is.
    fn start_in(mode: Mode, tmp: PathBuf, existing: Option<PathBuf>, apart: bool) -> World {
        World::start_prepared(mode, tmp, existing, apart, &|_| {})
    }

    /// As `start_in`, with `prepare` given the workspace before anything runs it.
    fn start_prepared(mode: Mode, tmp: PathBuf, existing: Option<PathBuf>, apart: bool, prepare: &dyn Fn(&Path)) -> World {
        std::fs::create_dir_all(&tmp).unwrap();
        match mode {
            Mode::L => {
                let root = existing.unwrap_or_else(|| {
                    let r = tmp.join("w");
                    acme(&r, "");
                    r
                });
                let port = free_port();
                let web = spawn(&["--port", &port.to_string(), "--no-open"], &[], Some(&root), tmp.join("web.log"), port);
                World { mode, host: root.clone(), root, base: String::new(), web: Some(web), main: None, tmp, keep: false }
            }
            Mode::S => {
                let port = free_port();
                let root = match existing {
                    Some(r) => {
                        let ws = r.join("workspace.yaml");
                        let t = std::fs::read_to_string(&ws).unwrap();
                        let kept: Vec<&str> = t.lines().filter(|l| !l.starts_with("url:")).collect();
                        std::fs::write(&ws, format!("{}\nurl: http://127.0.0.1:{port}\n", kept.join("\n").trim_end())).unwrap();
                        r
                    }
                    None => {
                        let r = tmp.join("w");
                        acme(&r, &format!("url: http://127.0.0.1:{port}\naccess:\n  owners: [{OWNER}]\n"));
                        r
                    }
                };
                let web = spawn(&["world", "serve", &root.display().to_string(), "--addr", &format!("127.0.0.1:{port}")], &[], None, tmp.join("web.log"), port);
                World { mode, host: root.clone(), root, base: String::new(), web: Some(web), main: None, tmp, keep: false }
            }
            Mode::P => {
                let machine = tmp.join("machine");
                let root = machine.join("orgs/acme");
                let port = free_port();
                acme(&root, "");
                std::fs::write(machine.join("workspace.yaml"), format!("title: A machine\nurl: http://127.0.0.1:{port}\n")).unwrap();
                std::fs::write(machine.join("members.yaml"), format!("acme:\n  - email: {OWNER}\n    role: owner\n")).unwrap();
                let web = spawn(&["hosting", "serve", &machine.display().to_string(), "--addr", &format!("127.0.0.1:{port}"), "--no-updates"], &[], None, tmp.join("web.log"), port);
                World { mode, root, host: machine, base: "/acme".into(), web: Some(web), main: None, tmp, keep: false }
            }
            Mode::M => {
                let control = tmp.join("control");
                copy(&fixture("control"), &control);
                let main_port = free_port();
                std::fs::write(control.join("workspace.yaml"), format!("title: Zetlyn\nurl: http://127.0.0.1:{main_port}\naccess:\n  owners: [op@example.org]\n")).unwrap();
                let main = spawn(&["hosting", "serve", &control.display().to_string(), "--addr", &format!("127.0.0.1:{main_port}"), "--no-updates"], &[], None, tmp.join("main.log"), main_port);
                let cell = tmp.join("cell");
                let root = cell.join("orgs/acme");
                acme(&root, if apart { "update:\n  every: 5m\n" } else { "" });
                prepare(&root);
                std::fs::write(cell.join("cell.yaml"), "active: true\n").unwrap();
                std::fs::write(cell.join("workspace.yaml"), format!("title: Acme\nurl: http://127.0.0.1:{}\n", main.port)).unwrap();
                std::fs::write(cell.join("members.yaml"), format!("acme:\n  - email: {OWNER}\n    role: owner\n")).unwrap();
                std::fs::create_dir_all(cell.join("usage")).unwrap();
                let port = free_port();
                let env = [
                    ("ZETLYN_USAGE", cell.join("usage").display().to_string()),
                    ("ZETLYN_CELL", "acme".to_string()),
                    ("ZETLYN_HOME", cell.join(".zetlyn").display().to_string()),
                    ("HOME", cell.display().to_string()),
                ];
                let web = spawn(&["hosting", "serve", &cell.display().to_string(), "--addr", &format!("127.0.0.1:{port}"), "--updates", if apart { "apart" } else { "none" }], &env, None, tmp.join("web.log"), port);
                World { mode, root, host: cell, base: "/acme".into(), web: Some(web), main: Some(main), tmp, keep: false }
            }
        }
    }

    /// Stopped, its folder kept: where it was, and its workspace.
    fn stop(mut self) -> (PathBuf, PathBuf) {
        self.keep = true;
        self.web = None;
        self.main = None;
        (self.tmp.clone(), self.root.clone())
    }

    fn port(&self) -> u16 {
        self.web.as_ref().unwrap().port
    }

    /// Its address, as another program would be given it.
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}{}", self.port(), self.base)
    }

    /// Signed in as `email`, the way this world signs people in: the cookies to send. On one's
    /// own machine nobody signs in, and whoever is there is its owner.
    fn sign_in(&self, email: &str) -> String {
        if self.mode == Mode::L {
            return String::new();
        }
        let web = self.web.as_ref().unwrap();
        let (port, ask, log) = match &self.main {
            Some(m) => (m.port, "/account/signin".to_string(), m.log.clone()),
            None => (web.port, format!("{}/signin", self.base), web.log.clone()),
        };
        let (s, _, b) = http(port, "POST", &ask, "", &format!("email={}", enc(email)));
        assert!(s == 200 || s == 303, "{:?}: signing in {email} answered {s}: {b}", self.mode);
        let link = last_link(&log);
        let path = match &self.main {
            Some(_) => format!("/account/{link}"),
            None => format!("{}/{link}", self.base),
        };
        let (s, h, b) = http(port, "GET", &path, "", "");
        assert_eq!(s, 303, "{:?}: the link signs {email} in: {b}", self.mode);
        let c = cookies_of(&h);
        assert!(!c.is_empty(), "{:?}: no session for {email}: {h}", self.mode);
        c.join("; ")
    }

    fn get(&self, path: &str, cookie: &str) -> (u16, String) {
        let (s, _, b) = http(self.port(), "GET", &format!("{}{path}", self.base), cookie, "");
        (s, b)
    }

    fn post(&self, path: &str, cookie: &str, form: &str) -> (u16, String) {
        let (s, h, b) = http(self.port(), "POST", &format!("{}{path}", self.base), cookie, form);
        (s, format!("{h}\n{b}"))
    }

    fn put(&self, path: &str, cookie: &str, body: &[u8]) -> (u16, String) {
        let (s, _, b) = http_raw(self.port(), "PUT", &format!("{}{path}", self.base), cookie, "application/octet-stream", body);
        (s, b)
    }

    /// Whether the tracker's overview is what this reader is shown, or the page saying it is closed.
    fn reads_tracker(&self, cookie: &str) -> bool {
        let (s, b) = self.get("/trackers/cve/", cookie);
        assert!(matches!(s, 200 | 303 | 401), "{:?}: the tracker answered {s}", self.mode);
        s == 200 && !b.contains("A private tracker.")
    }

    fn workspace(&self) -> String {
        std::fs::read_to_string(self.root.join("workspace.yaml")).unwrap()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        self.web = None;
        self.main = None;
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.tmp);
        }
    }
}

/// UC-D1, UC-D2, UC-D10, UC-D12: a private tracker for some people only, a whole domain at once,
/// nothing else of the world for them, and somebody taken off out at once. In every mode.
#[test]
fn a_private_tracker_is_for_its_readers_only_and_one_taken_off_is_out_at_once() {
    for mode in SERVED {
        let w = World::start(mode, "readers");
        let owner = w.sign_in(OWNER);
        let anna = w.sign_in("anna@example.org");
        let bob = w.sign_in("bob@elsewhere.org");

        // D1: made private, Anna named.
        let (s, b) = w.post("/settings/seen", &owner, "tracker=cve&visibility=private");
        assert_eq!(s, 303, "{mode:?}: made private: {b}");
        let (s, b) = w.post("/settings/readers/cve", &owner, &format!("readers={}", enc("anna@example.org")));
        assert_eq!(s, 303, "{mode:?}: readers saved: {b}");
        assert!(std::fs::read_to_string(w.root.join("trackers/cve/tracker.yaml")).unwrap().contains("anna@example.org"), "{mode:?}");
        assert!(w.reads_tracker(&owner), "{mode:?}: its owner reads it");
        assert!(w.reads_tracker(&anna), "{mode:?}: Anna reads it");
        assert!(!w.reads_tracker(&bob), "{mode:?}: Bob does not");
        assert!(!w.reads_tracker(""), "{mode:?}: nobody signed in does not");

        // D10: and nothing else of the world for Anna.
        let (s, _) = w.get("/settings", &anna);
        assert_ne!(s, 200, "{mode:?}: Anna has no settings here");
        let (_, said) = w.post("/settings/readers/cve", &anna, &format!("readers={}", enc("anna@example.org\nbob@elsewhere.org")));
        assert!(!std::fs::read_to_string(w.root.join("trackers/cve/tracker.yaml")).unwrap().contains("bob@"), "{mode:?}: Anna names nobody: {said}");
        assert!(!w.reads_tracker(&bob), "{mode:?}: still not Bob");

        // D12: taken off, out at once.
        let (s, _) = w.post("/settings/readers/cve", &owner, "readers=");
        assert_eq!(s, 303, "{mode:?}");
        assert!(!w.reads_tracker(&anna), "{mode:?}: Anna is out at once");

        // D2: a whole domain.
        let (s, _) = w.post("/settings/readers/cve", &owner, &format!("readers={}", enc("domain:elsewhere.org")));
        assert_eq!(s, 303, "{mode:?}");
        assert!(w.reads_tracker(&bob), "{mode:?}: Bob, by his domain");
        assert!(!w.reads_tracker(&anna), "{mode:?}: not Anna, of another domain");

        // Nonsense in the list is refused, and the list stays as it was.
        let (s, _) = w.post("/settings/readers/cve", &owner, "readers=not+an+address");
        assert_eq!(s, 303, "{mode:?}");
        assert!(std::fs::read_to_string(w.root.join("trackers/cve/tracker.yaml")).unwrap().contains("domain:elsewhere.org"), "{mode:?}: kept");
    }
}

/// UC-D3, UC-D5, UC-D9: an editor works on sources and trackers but changes nothing of the world;
/// an owner cannot take themselves off where nobody could give it back; one the machine names stays.
#[test]
fn an_editor_edits_and_only_an_owner_changes_the_world() {
    for mode in SERVED {
        let w = World::start(mode, "roles");
        let owner = w.sign_in(OWNER);
        let (s, b) = w.post("/settings/access", &owner, &format!("owners={}&editors={}", enc(OWNER), enc("ed@example.org")));
        assert_eq!(s, 303, "{mode:?}: {b}");
        assert!(w.workspace().contains("ed@example.org"), "{mode:?}: {}", w.workspace());

        let ed = w.sign_in("ed@example.org");
        assert_eq!(w.get("/sources", &ed).0, 200, "{mode:?}: the editor sees the sources");
        assert!(w.reads_tracker(&ed), "{mode:?}: and the trackers");
        let (s, _) = w.post("/settings/profile", &ed, "title=Taken");
        assert_eq!(s, 403, "{mode:?}: the editor changes no setting");
        let (s, _) = w.post("/settings/access", &ed, &format!("owners={}", enc("ed@example.org")));
        assert_eq!(s, 403, "{mode:?}: nor who may do what");
        assert!(w.workspace().contains("title: Acme"), "{mode:?}");

        // Nobody signed in, or signed in as nobody of the world: no settings at all.
        let stranger = w.sign_in("someone@elsewhere.org");
        for c in ["", stranger.as_str()] {
            assert_ne!(w.get("/settings", c).0, 200, "{mode:?}: settings for {c:?}");
        }

        // The owner off the list: on its own server nobody could give it back, so it is refused;
        // on a machine that names its owners, they stay owners whatever the list says.
        let (s, _) = w.post("/settings/access", &owner, &format!("editors={}", enc("ed@example.org")));
        assert_eq!(s, 303, "{mode:?}");
        let (s, b) = w.post("/settings/profile", &owner, "title=Acme+still");
        assert_eq!(s, 303, "{mode:?}: the owner still changes settings: {b}");
        assert!(w.workspace().contains("Acme still"), "{mode:?}");
        if mode == Mode::S {
            assert!(w.workspace().contains(OWNER), "{mode:?}: refused, the owner stays on the list");
        }
    }
}

/// UC-E1, UC-E2, UC-E3, UC-E9, UC-E10: its owner says freely whether a tracker and a source are
/// public. A public tracker shows its values whatever its sources' pages are; a source's page is as
/// its trackers are until its owner says otherwise.
#[test]
fn what_is_public_is_its_owners_to_say_tracker_by_tracker_and_source_by_source() {
    for mode in SERVED {
        let w = World::start(mode, "public");
        let owner = w.sign_in(OWNER);
        let page = |c: &str| w.get("/sources/kev", c).0 == 200;

        // As it comes: a public tracker, its sources as it is.
        assert!(page(""), "{mode:?}: a source a public tracker holds is shown");
        assert!(w.reads_tracker(""), "{mode:?}: and the tracker");

        // E10: the source private, the tracker still public with its values.
        let (s, b) = w.post("/settings/sources", &owner, "page.kev=private");
        assert_eq!(s, 303, "{mode:?}: {b}");
        assert!(!page(""), "{mode:?}: the source's page is closed");
        assert!(w.reads_tracker(""), "{mode:?}: the tracker is open, with its values");
        let (_, t) = w.get("/trackers/cve/", "");
        assert!(!t.contains("/sources/kev\""), "{mode:?}: and no way from it to the source's page");
        assert_eq!(w.get("/sources/kev", &owner).0, 200, "{mode:?}: its owner still opens it");

        // The tracker private, the source made public: each as its owner said.
        let (s, _) = w.post("/settings/seen", &owner, "tracker=cve&visibility=private");
        assert_eq!(s, 303, "{mode:?}");
        let (s, _) = w.post("/settings/sources", &owner, "page.kev=public");
        assert_eq!(s, 303, "{mode:?}");
        assert!(page(""), "{mode:?}: a public source is shown though only a private tracker holds it");
        assert!(!w.reads_tracker(""), "{mode:?}: the private tracker is not");

        // E3: said nothing, it is as its trackers are: only a private one holds it, so it is closed.
        let (s, _) = w.post("/settings/sources", &owner, "page.kev=auto");
        assert_eq!(s, 303, "{mode:?}");
        assert!(!page(""), "{mode:?}: as its trackers, private");

        // E2: how much a public tracker shows of it is a choice of its own.
        let (s, _) = w.post("/settings/sources", &owner, "licence.kev=summary");
        assert_eq!(s, 303, "{mode:?}");
        assert!(std::fs::read_to_string(w.root.join("sources/kev/source.yaml")).unwrap().contains("republish: summary"), "{mode:?}");

        // A `no` from before: the page private, the values in public trackers as titles and values.
        let (s, _) = w.post("/settings/seen", &owner, "tracker=cve&visibility=public");
        assert_eq!(s, 303, "{mode:?}");
        let f = w.root.join("sources/kev/source.yaml");
        let t = std::fs::read_to_string(&f).unwrap().replace("republish: summary", "republish: no");
        std::fs::write(&f, t).unwrap();
        assert!(!page(""), "{mode:?}: no, from before, is a private page");
        assert!(w.reads_tracker(""), "{mode:?}: and the public tracker stays open");
    }
}

/// UC-D4: who may do what, said on one's own machine, holds once the world runs on a server of its own.
#[test]
fn the_roles_set_on_ones_machine_hold_on_a_server_of_ones_own() {
    let local = World::start(Mode::L, "machine-to-server");
    let (s, b) = local.post("/settings/access", "", &format!("owners={}&editors={}", enc(OWNER), enc("ed@example.org")));
    assert_eq!(s, 303, "{b}");
    assert!(local.workspace().contains("ed@example.org"), "{}", local.workspace());
    let (tmp, root) = local.stop();

    let served = World::start_in(Mode::S, tmp, Some(root), false);
    let ed = served.sign_in("ed@example.org");
    assert_eq!(served.get("/sources", &ed).0, 200, "the editor named on the machine edits on the server");
    let owner = served.sign_in(OWNER);
    let (s, _) = served.post("/settings/profile", &owner, "title=Served");
    assert_eq!(s, 303);
    assert!(served.workspace().contains("Served"), "{}", served.workspace());
}

/// UC-H3, UC-H5: a copy on one's machine taken from, and kept one with, a world wherever it runs,
/// by a key its owner makes there.
#[test]
fn a_copy_on_ones_machine_syncs_with_a_world_wherever_it_runs() {
    for mode in SERVED {
        let w = World::start(mode, "sync");
        let owner = w.sign_in(OWNER);
        let (s, page) = w.post("/settings/sync-key", &owner, "");
        assert_eq!(s, 200, "{mode:?}: {page}");
        let key = page.split("--key ").nth(1).and_then(|k| k.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').next()).unwrap_or("").to_string();
        assert!(key.starts_with("zk_"), "{mode:?}: a key on the page");
        assert_eq!(http_raw(w.port(), "GET", &format!("{}/sync/state", w.base), "", "", b"").0, 401, "{mode:?}: nobody without it");

        let mine = w.tmp.join("mine");
        let (ok, said) = z(&["world", "sync", &w.url(), &mine.display().to_string(), "--key", &key]);
        assert!(ok, "{mode:?}: {said}");
        assert!(mine.join("trackers/cve/tracker.yaml").exists(), "{mode:?}: {said}");

        // A change here goes there.
        let t = mine.join("trackers/cve/tracker.yaml");
        std::fs::write(&t, std::fs::read_to_string(&t).unwrap().replacen("title: CVE", "title: CVE mine", 1)).unwrap();
        let (ok, said) = z(&["world", "sync", &w.url(), &mine.display().to_string()]);
        assert!(ok, "{mode:?}: {said}");
        assert!(std::fs::read_to_string(w.root.join("trackers/cve/tracker.yaml")).unwrap().contains("CVE mine"), "{mode:?}: {said}");
    }
}

/// UC-H1, UC-H2, UC-H11 as a cell does them: an export asked of its server, an upload in pieces
/// handed to its server; and on a server of one's own, both done there.
#[test]
fn moving_in_and_out_is_handed_to_whoever_does_it() {
    let archive_of = |w: &World| {
        let a = w.tmp.join("other.tar.gz");
        let (ok, said) = z(&["world", "export", &w.root.display().to_string(), "--to", &a.display().to_string()]);
        assert!(ok, "{said}");
        std::fs::read(&a).unwrap()
    };
    for mode in [Mode::S, Mode::M] {
        let w = World::start(mode, "moving");
        let owner = w.sign_in(OWNER);

        // Out.
        let (s, b) = w.post("/settings/export", &owner, "");
        assert_eq!(s, 303, "{mode:?}: {b}");
        match mode {
            Mode::M => assert!(w.host.join("incoming/export.json").exists(), "the server is asked for it"),
            _ => {
                let state = w.root.join(".zetlyn/exports/state.json");
                wait_for("the export", || std::fs::read_to_string(&state).is_ok_and(|s| s.contains("\"ready\"")));
                let file = std::fs::read_to_string(&state).unwrap().split("\"file\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
                let (s, _, b) = http_raw(w.port(), "GET", &format!("{}/settings/export/{file}", w.base), &owner, "", b"");
                assert_eq!(s, 200, "{mode:?}: downloaded");
                assert!(!b.is_empty());
                assert_ne!(w.get(&format!("/settings/export/{file}"), "").0, 200, "{mode:?}: by its owner only");
            }
        }

        // In, in pieces.
        let bytes = archive_of(&w);
        let (s, begun) = w.post("/settings/upload", &owner, &format!("size={}&name=other.tar.gz", bytes.len()));
        assert_eq!(s, 200, "{mode:?}: {begun}");
        let id = begun.split("\"id\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
        assert_ne!(w.put(&format!("/settings/upload/{id}/0"), "", &bytes).0, 200, "{mode:?}: nobody but an owner sends a piece");
        let (s, b) = w.put(&format!("/settings/upload/{id}/0"), &owner, &bytes);
        assert_eq!(s, 200, "{mode:?}: {b}");
        let (s, b) = w.post(&format!("/settings/upload/{id}/done"), &owner, "");
        assert_eq!(s, 200, "{mode:?}: {b}");
        match mode {
            Mode::M => {
                assert!(w.host.join("incoming/import.json").exists(), "the server is asked to bring it in");
                assert!(w.host.join("incoming/world.tar.gz").exists());
            }
            _ => {
                let last = w.root.join(".zetlyn/incoming/last-import.json");
                wait_for("the import", || std::fs::read_to_string(&last).is_ok_and(|s| s.contains("\"ok\":true")));
            }
        }
    }
}

/// UC-I6, UC-I7: a world on zetlyn.com without a plan has settings that are not empty, and Assist
/// says how to have it there; on one's own machine Assist is set up in place.
#[test]
fn settings_say_what_there_is_where_the_world_runs() {
    let m = World::start(Mode::M, "managed-settings");
    let owner = m.sign_in(OWNER);
    let (s, b) = m.get("/settings", &owner);
    assert_eq!(s, 200);
    assert!(b.contains("settings-nav") && !b.contains("Plan and usage"), "no plan, no usage to show");
    let (_, b) = m.get("/settings/assist", &owner);
    assert!(b.contains("hello@zetlyn.com") && !b.contains("Keep the key"), "Assist on zetlyn.com: how to have it");
    let (_, b) = m.get("/settings/moving", &owner);
    assert!(!b.contains("Where it went"), "nothing moves away from zetlyn.com by saying so");

    let l = World::start(Mode::L, "local-settings");
    let (_, b) = l.get("/settings/assist", "");
    assert!(b.contains("Keep the key"), "on one's own machine it is set up here");
    let (_, b) = l.get("/settings/access", "");
    assert!(b.contains("Nobody signs in to the app on your machine"), "who may do what, and when it holds");
}

/// UC-D14, UC-E8, UC-B4: the app on one's own machine is its owner's, says a tracker is only there,
/// and reads no source more often than every quarter of an hour.
#[test]
fn the_app_on_ones_machine_is_its_owners_and_reads_within_its_floor() {
    let l = World::start(Mode::L, "local");
    let (s, b) = l.get("/trackers/cve/", "");
    assert_eq!(s, 200);
    assert!(b.contains("On this machine"), "said to be on this machine, not live for anybody");
    assert_eq!(l.get("/settings", "").0, 200, "whoever is at the machine owns it");

    let (s, _) = l.post("/settings/updates", "", "every=5m");
    assert_eq!(s, 303);
    assert!(!l.workspace().contains("5m"), "five minutes is under the floor here: {}", l.workspace());
    let (s, _) = l.post("/settings/updates", "", "every=15m");
    assert_eq!(s, 303);
    assert!(l.workspace().contains("15m"), "{}", l.workspace());
}


/// UC-B9, UC-B10: a cell wakes itself to read what is due, no timer from outside: a pass in a process
/// of its own as soon as it starts, how it went written down, and the next one set for when its
/// five-minute sources are due again, not five minutes after this one ends.
#[test]
fn a_cell_reads_its_sources_when_they_are_due_and_nothing_wakes_it_but_itself() {
    let tmp = std::env::temp_dir().join(format!("zetlyn-modes-{}-wakes-M", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let w = World::start_in(Mode::M, tmp, None, true);
    let last = w.host.join("last-run.json");
    wait_for("a first pass", || std::fs::read_to_string(&last).is_ok_and(|s| s.contains("\"result\"")));
    let run: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&last).unwrap()).unwrap();
    assert_eq!(run["result"], "success", "{run}");
    let next: i64 = std::fs::read_to_string(w.host.join(".next-run")).unwrap().trim().parse().expect("when the next is due");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    assert!((now + 200..=now + 310).contains(&next), "the sources were read just now, every five minutes: next in {}s", next - now);
    // And the pages answer meanwhile, from the process that started it.
    assert_eq!(w.get("/about", "").0, 200);
}

/// UC-H8: two worlds that both run for others, one on a server of its own and one on zetlyn.com,
/// kept one by the first asking the second, when its owner says and by itself on a rhythm; no copy
/// on anybody's machine between them.
#[test]
fn a_world_on_its_own_server_and_one_on_zetlyn_com_stay_one() {
    let a = World::start(Mode::S, "server-to-cell");
    let b = World::start(Mode::M, "server-to-cell");
    let (owner_a, owner_b) = (a.sign_in(OWNER), b.sign_in(OWNER));
    // Who may do what said alike on both, as two copies of one world would.
    let (s, _) = b.post("/settings/access", &owner_b, &format!("owners={}", enc(OWNER)));
    assert_eq!(s, 303);
    let (s, page) = b.post("/settings/sync-key", &owner_b, "");
    assert_eq!(s, 200, "{page}");
    let key = page.split("--key ").nth(1).and_then(|k| k.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').next()).unwrap().to_string();

    let last = a.root.join(".zetlyn/sync/last.json");
    let synced = |what: &str| {
        wait_for(what, || std::fs::read_to_string(&last).is_ok_and(|s| !s.contains("\"running\"")));
        let l: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&last).unwrap()).unwrap();
        assert_eq!(l["state"], "ok", "{what}: {l}");
    };

    // Asked from the page, the key kept, and a rhythm said.
    let (s, b2) = a.post("/settings/sync", &owner_a, &format!("url={}&key={key}&every=15m", enc(&b.url())));
    assert_eq!(s, 303, "{b2}");
    synced("the first sync");
    assert_eq!(std::fs::read_to_string(a.root.join(".zetlyn/sync/every")).unwrap().trim(), "15m");

    // A change on zetlyn.com, brought here by asking again with nothing said: address and key kept.
    let theirs = b.root.join("trackers/cve/tracker.yaml");
    std::fs::write(&theirs, std::fs::read_to_string(&theirs).unwrap().replacen("title: CVE", "title: CVE on zetlyn.com", 1)).unwrap();
    let (s, _) = a.post("/settings/sync", &owner_a, "");
    assert_eq!(s, 303);
    synced("the second sync");
    assert!(std::fs::read_to_string(a.root.join("trackers/cve/tracker.yaml")).unwrap().contains("CVE on zetlyn.com"));

    // By itself: due once its rhythm has passed, done by the pass that reads the sources.
    let ours = a.root.join("sources/kev/source.yaml");
    std::fs::write(&ours, std::fs::read_to_string(&ours).unwrap().replacen("title:", "title: On my server,", 1)).unwrap();
    let mut l: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&last).unwrap()).unwrap();
    l["t"] = serde_json::json!(l["t"].as_i64().unwrap() - 16 * 60);
    std::fs::write(&last, l.to_string()).unwrap();
    let (ok, said) = z(&["run", &a.root.display().to_string(), "--once"]);
    assert!(ok, "{said}");
    synced("the scheduled sync");
    assert!(std::fs::read_to_string(b.root.join("sources/kev/source.yaml")).unwrap().contains("On my server,"), "{said}");
}

/// UC-D15: a team in one office runs a world of its own in its network: started with `--lan`, its
/// owner said on the command line, reached over plain http, and somebody without mail there signed
/// in by a link its owner hands on.
#[test]
fn a_team_runs_a_world_in_its_own_network() {
    let tmp = std::env::temp_dir().join(format!("zetlyn-modes-{}-lan", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let root = tmp.join("w");
    acme(&root, "");
    let port = free_port();
    let web = spawn(&["world", "serve", &root.display().to_string(), "--lan", "--port", &port.to_string(), "--owner", OWNER], &[], None, tmp.join("web.log"), port);
    let port = web.port;
    let ws = std::fs::read_to_string(root.join("workspace.yaml")).unwrap();
    assert!(ws.contains(OWNER), "the owner said on the command line: {ws}");
    let log = std::fs::read_to_string(&web.log).unwrap();
    assert!(log.contains("0.0.0.0:") && (log.contains("In this network, open http://") || log.contains("No address in a network")), "{log}");
    let plain = ws.lines().any(|l| l.starts_with("url: ") && l.contains("http://"));

    // The owner, by the link printed where it runs; the cookie one a browser keeps over http.
    let (s, _, _) = http(port, "POST", "/signin", "", &format!("email={}", enc(OWNER)));
    assert_eq!(s, 200);
    let link = last_link(&web.log);
    let (s, h, _) = http(port, "GET", &format!("/{link}"), "", "");
    assert_eq!(s, 303, "{h}");
    if plain {
        assert!(!h.contains("Secure"), "over plain http no cookie is marked secure: {h}");
    }
    let owner = cookies_of(&h).join("; ");

    // Anna, an editor, given a link by the owner rather than by mail.
    let (s, _, _) = http(port, "POST", "/settings/access", &owner, &format!("owners={}&editors={}", enc(OWNER), enc("anna@example.org")));
    assert_eq!(s, 303);
    let (s, _, page) = http(port, "POST", "/settings/signin-link", &owner, &format!("email={}", enc("anna@example.org")));
    assert_eq!(s, 200, "{page}");
    let raw = page.split("/signin/").nth(1).and_then(|r| r.split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_').next()).expect("a link on the page").to_string();
    let (s, _, _) = http(port, "POST", "/settings/signin-link", "", "email=x%40y.org");
    assert_ne!(s, 200, "nobody but an owner makes one");
    let (s, h, _) = http(port, "GET", &format!("/signin/{raw}"), "", "");
    assert_eq!(s, 303, "the link signs Anna in: {h}");
    let anna = cookies_of(&h).join("; ");
    assert_eq!(http(port, "GET", "/sources", &anna, "").0, 200, "Anna edits");
    assert_eq!(http(port, "GET", &format!("/signin/{raw}"), "", "").0, 410, "once");
    drop(web);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// UC-D11, UC-J2: a program reads a private tracker with a key of a reader of it, and with nothing
/// more than that reader may: taken off, the key is out at once; revoked, it is dead; a key makes
/// no key.
#[test]
fn a_program_reads_with_a_key_what_its_person_may_and_no_more() {
    for mode in SERVED {
        let w = World::start(mode, "keys");
        let owner = w.sign_in(OWNER);
        let anna = w.sign_in("anna@example.org");
        let bob = w.sign_in("bob@elsewhere.org");
        let (s, _) = w.post("/settings/seen", &owner, "tracker=cve&visibility=private");
        assert_eq!(s, 303, "{mode:?}");
        let (s, _) = w.post("/settings/readers/cve", &owner, &format!("readers={}", enc("anna@example.org")));
        assert_eq!(s, 303, "{mode:?}");

        let make = |cookie: &str| -> Option<String> {
            let (_, page) = w.post("/trackers/cve/account/key", cookie, "name=script");
            page.split("zk_").nth(1).map(|k| format!("zk_{}", k.split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-').next().unwrap()))
        };
        let api = |key: &str| http_raw(w.port(), "GET", &format!("{}/trackers/cve/api/describe", w.base), &format!("x=y\r\nAuthorization: Bearer {key}"), "", b"").0;
        let anna_key = make(&anna).unwrap_or_else(|| panic!("{mode:?}: Anna makes a key"));
        assert_eq!(api(&anna_key), 200, "{mode:?}: Anna's program reads it");
        assert_ne!(http_raw(w.port(), "GET", &format!("{}/trackers/cve/api/describe", w.base), "", "", b"").0, 200, "{mode:?}: nobody without a key");
        if let Some(bob_key) = make(&bob) {
            assert_ne!(api(&bob_key), 200, "{mode:?}: Bob's key reads what Bob reads: not this");
        }
        let (_, page) = w.post("/trackers/cve/account/key", &format!("x=y\r\nAuthorization: Bearer {anna_key}"), "name=another");
        assert!(!page.contains("zk_"), "{mode:?}: a key makes no key");

        // Taken off: out at once, key and all. Named again: in again.
        let (s, _) = w.post("/settings/readers/cve", &owner, "readers=");
        assert_eq!(s, 303, "{mode:?}");
        assert_ne!(api(&anna_key), 200, "{mode:?}: Anna's key is out with her");
        let (s, _) = w.post("/settings/readers/cve", &owner, &format!("readers={}", enc("anna@example.org")));
        assert_eq!(s, 303, "{mode:?}");
        assert_eq!(api(&anna_key), 200, "{mode:?}");

        // Revoked by Anna: dead.
        let (s, _) = w.post("/trackers/cve/account/key/drop", &anna, "name=script");
        assert!(s == 200 || s == 303, "{mode:?}: {s}");
        assert_ne!(api(&anna_key), 200, "{mode:?}: a revoked key reads nothing");
    }
}

/// UC-J4: an owner's or an editor's program reads all they may, private trackers included, with a
/// key of their own from Settings, and changes nothing; off the lists, the key is out.
#[test]
fn a_members_key_reads_all_they_may_and_changes_nothing() {
    for mode in SERVED {
        let w = World::start(mode, "member-keys");
        let owner = w.sign_in(OWNER);
        let (s, _) = w.post("/settings/access", &owner, &format!("owners={}&editors={}", enc(OWNER), enc("ed@example.org")));
        assert_eq!(s, 303, "{mode:?}");
        let (s, _) = w.post("/settings/seen", &owner, "tracker=cve&visibility=private");
        assert_eq!(s, 303, "{mode:?}");
        let key_of = |cookie: &str| -> String {
            let (s, page) = w.post("/settings/keys", cookie, "name=script");
            assert_eq!(s, 200, "{mode:?}: {page}");
            page.split("<pre id=\"api-key\">").nth(1).and_then(|k| k.split('<').next()).unwrap_or_else(|| panic!("{mode:?}: a key on {page}")).to_string()
        };
        let with = |key: &str| format!("x=y\r\nAuthorization: Bearer {key}");
        let api = |key: &str| http_raw(w.port(), "GET", &format!("{}/trackers/cve/api/describe", w.base), &with(key), "", b"").0;

        let k = key_of(&owner);
        assert_eq!(api(&k), 200, "{mode:?}: the owner's program reads the private tracker");
        assert_eq!(w.get("/sources", &with(&k)).0, 200, "{mode:?}: and the world's pages");
        let (s, _) = w.post("/settings/profile", &with(&k), "title=By+a+program");
        assert_ne!(s, 200, "{mode:?}");
        assert!(!w.workspace().contains("By a program"), "{mode:?}: a key changes nothing");
        let (_, list) = w.get("/settings/keys", &owner);
        assert!(list.contains("script"), "{mode:?}: listed");

        let ed = w.sign_in("ed@example.org");
        let e = key_of(&ed);
        assert_eq!(api(&e), 200, "{mode:?}: an editor's program reads it too");
        let (s, _) = w.post("/settings/access", &owner, &format!("owners={}", enc(OWNER)));
        assert_eq!(s, 303, "{mode:?}");
        assert_ne!(api(&e), 200, "{mode:?}: off the lists, the editor's key is out");

        let (s, _) = w.post("/settings/keys/drop", &owner, "name=script");
        assert_eq!(s, 303, "{mode:?}");
        assert_ne!(api(&k), 200, "{mode:?}: revoked, it reads nothing");
    }
}

/// A claim as `zetlyn claim` prints it for a source of a workspace: its current raw value of a
/// property, and how many versions it has.
fn claim_of(root: &Path, source: &str, id: &str, property: &str) -> (String, usize) {
    let (ok, printed) = z(&["claim", &root.join("sources").join(source).display().to_string(), id]);
    assert!(ok, "{printed}");
    let j: serde_json::Value = serde_json::from_str(&printed[printed.find(['{', '[']).unwrap_or(0)..]).unwrap_or_default();
    let c = if j.is_array() { j[0].clone() } else { j };
    (c["excerpt"]["row"][property].as_str().unwrap_or("").to_string(), c["versions"].as_array().map_or(0, Vec::len))
}

/// The sync key an owner makes for a world, off the page that shows it.
fn sync_key(w: &World, owner: &str) -> String {
    let (s, page) = w.post("/settings/sync-key", owner, "");
    assert_eq!(s, 200, "{page}");
    page.split("--key ").nth(1).and_then(|k| k.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').next()).unwrap().to_string()
}

fn edit(file: &Path, from: &str, to: &str) {
    let t = std::fs::read_to_string(file).unwrap();
    assert!(t.contains(from), "{}: no {from:?}", file.display());
    std::fs::write(file, t.replacen(from, to, 1)).unwrap();
}

/// UC-H13: two people, each with a copy on their own machine, kept one with the same world on
/// zetlyn.com: what either changes reaches the other through it; the same thing changed by both is
/// the second one's to decide; what either read is read by all, once.
#[test]
fn two_machines_and_one_world_on_zetlyn_com_stay_one() {
    let m = World::start(Mode::M, "two-machines");
    let owner = m.sign_in(OWNER);
    let key = sync_key(&m, &owner);
    let (l1, l2) = (m.tmp.join("one"), m.tmp.join("two"));
    let sync = |l: &Path, more: &[&str]| -> (bool, String) {
        let mut args = vec!["world", "sync"];
        let (url, dir) = (m.url(), l.display().to_string());
        args.push(&url);
        args.push(&dir);
        args.extend_from_slice(more);
        z(&args)
    };
    for l in [&l1, &l2] {
        let (ok, said) = sync(l, &["--key", &key]);
        assert!(ok, "{said}");
    }

    // Each changes something of their own; both have both after a round.
    edit(&l1.join("trackers/cve/tracker.yaml"), "title: CVE", "title: CVE from one");
    edit(&l2.join("sources/vendor-b/source.yaml"), "title:", "title: From two,");
    for l in [&l1, &l2, &l1] {
        let (ok, said) = sync(l, &[]);
        assert!(ok, "{said}");
    }
    for root in [&l1, &l2, &m.root] {
        assert!(std::fs::read_to_string(root.join("trackers/cve/tracker.yaml")).unwrap().contains("CVE from one"), "{}", root.display());
        assert!(std::fs::read_to_string(root.join("sources/vendor-b/source.yaml")).unwrap().contains("From two,"), "{}", root.display());
    }

    // The same thing changed by both: the first to sync has it there; the second is asked, and
    // nothing of theirs goes until they say.
    edit(&l1.join("trackers/cve/tracker.yaml"), "title: CVE from one", "title: CVE by one");
    edit(&l2.join("trackers/cve/tracker.yaml"), "title: CVE from one", "title: CVE by two");
    let (ok, said) = sync(&l1, &[]);
    assert!(ok, "{said}");
    let (ok, said) = sync(&l2, &[]);
    assert!(!ok && said.contains("not synced"), "the second is asked: {said}");
    assert!(std::fs::read_to_string(m.root.join("trackers/cve/tracker.yaml")).unwrap().contains("CVE by one"), "nothing of two's went");
    let (ok, said) = sync(&l2, &["--take", "theirs"]);
    assert!(ok, "{said}");
    assert!(std::fs::read_to_string(l2.join("trackers/cve/tracker.yaml")).unwrap().contains("CVE by one"));

    // Read on one machine: the new value everywhere, its version before kept, and each once
    // however often the copies sync after.
    std::fs::copy(fixture("update-2/vendor-a.csv"), l1.join("sources/vendor-a/advisories.csv")).unwrap();
    let (ok, said) = z(&["source", "update", &l1.join("sources/vendor-a").display().to_string()]);
    assert!(ok, "{said}");
    for l in [&l1, &l2, &l1, &l2, &l1] {
        let (ok, said) = sync(l, &[]);
        assert!(ok, "{said}");
    }
    for root in [&l1, &l2, &m.root] {
        assert_eq!(claim_of(root, "vendor-a", "CVE-2026-0001", "cvss"), ("8.1".to_string(), 2), "{}", root.display());
    }
}

/// UC-H16: both copies read the same sources on their own rhythm and sync between: every sync goes
/// through, nothing about the data is ever a conflict, and the same observation read on both sides
/// is one version, not two.
#[test]
fn both_copies_reading_their_sources_and_syncing_never_conflict() {
    let m = World::start(Mode::M, "both-read");
    let owner = m.sign_in(OWNER);
    let key = sync_key(&m, &owner);
    let mine = m.tmp.join("mine");
    let (url, dir) = (m.url(), mine.display().to_string());
    let (ok, said) = z(&["world", "sync", &url, &dir, "--key", &key]);
    assert!(ok, "{said}");
    let read = |root: &Path| {
        for s in MEMBERS {
            let (ok, said) = z(&["source", "update", &root.join("sources").join(s).display().to_string()]);
            assert!(ok, "{said}");
        }
    };
    // Rounds of both reading, nothing new at the sources, and syncing.
    for _ in 0..3 {
        read(&m.root);
        read(&mine);
        let (ok, said) = z(&["world", "sync", &url, &dir]);
        assert!(ok, "a round: {said}");
    }
    // Something new at a source, read on both sides, at different times.
    std::fs::copy(fixture("update-2/vendor-a.csv"), m.root.join("sources/vendor-a/advisories.csv")).unwrap();
    read(&m.root);
    let (ok, said) = z(&["world", "sync", &url, &dir]);
    assert!(ok, "{said}");
    read(&mine);
    for _ in 0..2 {
        let (ok, said) = z(&["world", "sync", &url, &dir]);
        assert!(ok, "{said}");
    }
    for root in [&mine, &m.root] {
        assert_eq!(claim_of(root, "vendor-a", "CVE-2026-0001", "cvss"), ("8.1".to_string(), 2), "one version each, read twice: {}", root.display());
    }
}

/// UC-K3, UC-B7: a source that cannot be read is tried again at its rhythm, and after three
/// failures in a row it waits, says why, and is not asked again until its owner says try again,
/// wherever the world runs: here the pass a server of its own and a cell run, `zetlyn run`.
#[test]
fn a_source_that_keeps_failing_waits_for_its_owner() {
    let w = World::start(Mode::S, "failing");
    let owner = w.sign_in(OWNER);
    let decl = w.root.join("sources/kev/source.yaml");
    let t = std::fs::read_to_string(&decl).unwrap();
    std::fs::write(&decl, format!("{}\nschedule:\n  every: 1s\n", t.trim_end())).unwrap();
    let file = w.root.join("sources/kev/kev.csv");
    let kept = std::fs::read(&file).unwrap();
    std::fs::remove_file(&file).unwrap();
    let root = w.root.display().to_string();
    let pass = || {
        std::thread::sleep(std::time::Duration::from_millis(1100));
        z(&["run", &root, "--once"]).1
    };
    for i in 1..=3 {
        let said = pass();
        assert!(said.contains("test/kev"), "try {i}: {said}");
    }
    let (_, page) = w.get("/settings/updates", &owner);
    assert!(page.contains("waits for you") && page.contains("Try it again"), "after three, it waits and says so");
    let said = pass();
    assert!(!said.contains("test/kev"), "not asked again: {said}");

    std::fs::write(&file, kept).unwrap();
    let (s, _) = w.post("/settings/retry/kev", &owner, "");
    assert_eq!(s, 303);
    let said = pass();
    assert!(said.contains("test/kev update"), "asked again, and read: {said}");
    let (_, page) = w.get("/settings/updates", &owner);
    assert!(!page.contains("waits for you"), "and no longer waiting");
}

/// UC-K2: the machine goes down in the middle of reading a source (here: the process killed while
/// the source is half sent). What the source held before is still what it holds, and the next pass
/// reads it whole.
#[test]
fn a_read_cut_off_halfway_leaves_the_source_as_it_was_and_the_next_one_reads_it() {
    let tmp = std::env::temp_dir().join(format!("zetlyn-modes-{}-cut-off", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let root = tmp.join("w");
    acme(&root, "");
    let csv = std::fs::read(root.join("sources/kev/kev.csv")).unwrap();
    let before = claim_of(&root, "kev", "CVE-2026-0001", "vendorProject");

    // A source that sends half and then nothing, until it is told to send all.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let whole = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let (csv, whole) = (csv.clone(), whole.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (csv, whole) = (csv.clone(), whole.clone());
                std::thread::spawn(move || {
                let mut s = stream;
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/csv\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", csv.len()).as_bytes());
                if whole.load(std::sync::atomic::Ordering::SeqCst) {
                    let _ = s.write_all(&csv);
                } else {
                    let _ = s.write_all(&csv[..csv.len() / 2]);
                    std::thread::sleep(std::time::Duration::from_secs(60));
                }
                });
            }
        });
    }
    let decl = root.join("sources/kev/source.yaml");
    let t = std::fs::read_to_string(&decl).unwrap();
    std::fs::write(&decl, t.replace("path: kev.csv", &format!("path: http://127.0.0.1:{port}/kev.csv"))).unwrap();

    let mut reading = Command::new(env!("CARGO_BIN_EXE_zetlyn")).args(["source", "update", &root.join("sources/kev").display().to_string()]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert!(reading.try_wait().unwrap().is_none(), "still reading when it is cut off");
    reading.kill().unwrap();
    let _ = reading.wait();
    assert_eq!(claim_of(&root, "kev", "CVE-2026-0001", "vendorProject"), before, "what it held before, it holds");

    whole.store(true, std::sync::atomic::Ordering::SeqCst);
    let (ok, said) = z(&["source", "update", &root.join("sources/kev").display().to_string()]);
    assert!(ok, "read whole the next time: {said}");
    assert_eq!(claim_of(&root, "kev", "CVE-2026-0001", "vendorProject").0, before.0);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// UC-K1: where nothing more can be written (a full disk, a folder not ours), an export and an
/// import say so and leave what was there as it was: no half archive, no half world.
#[test]
fn where_nothing_can_be_written_an_export_and_an_import_change_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let l = World::start(Mode::L, "unwritable");
    let ro = |p: &Path, on: bool| std::fs::set_permissions(p, std::fs::Permissions::from_mode(if on { 0o555 } else { 0o755 })).unwrap();

    // Out: the exports folder takes nothing.
    let exports = l.root.join(".zetlyn/exports");
    std::fs::create_dir_all(&exports).unwrap();
    ro(&exports, true);
    let (s, said) = l.post("/settings/export", "", "");
    assert_eq!(s, 303);
    assert!(said.contains("Not+exported") || said.contains("Not%20exported"), "said at once: {said}");
    let (_, page) = l.get("/settings/moving", "");
    assert!(!page.contains("Download the archive"), "no archive offered");
    ro(&exports, false);
    assert!(std::fs::read_dir(&exports).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".tar.gz")), "no half archive");

    // In: the folder around the workspace takes no new one.
    let archive = l.tmp.join("other.tar.gz");
    let (ok, said) = z(&["world", "export", &l.root.display().to_string(), "--to", &archive.display().to_string()]);
    assert!(ok, "{said}");
    let bytes = std::fs::read(&archive).unwrap();
    let title_before = l.workspace();
    let (s, begun) = l.post("/settings/upload", "", &format!("size={}&name=other.tar.gz", bytes.len()));
    assert_eq!(s, 200, "{begun}");
    let id = begun.split("\"id\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
    assert_eq!(l.put(&format!("/settings/upload/{id}/0"), "", &bytes).0, 200);
    let around = l.root.parent().unwrap().to_path_buf();
    ro(&around, true);
    let (s, b) = l.post(&format!("/settings/upload/{id}/done"), "", "");
    assert_eq!(s, 200, "{b}");
    let last = l.root.join(".zetlyn/incoming/last-import.json");
    wait_for("the import to end", || std::fs::read_to_string(&last).is_ok());
    ro(&around, false);
    let said = std::fs::read_to_string(&last).unwrap();
    assert!(said.contains("\"ok\":false"), "it says it did not go: {said}");
    assert_eq!(l.workspace(), title_before, "the world is as it was");
    assert!(l.reads_tracker(""), "and answers");
}

/// A source at an address that sends half of what it says it will and then nothing, for as long as
/// anybody waits: a read that never ends by itself.
fn a_source_that_hangs(csv: Vec<u8>) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let csv = csv.clone();
            std::thread::spawn(move || {
                let mut s = stream;
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/csv\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", csv.len()).as_bytes());
                let _ = s.write_all(&csv[..csv.len() / 2]);
                std::thread::sleep(std::time::Duration::from_secs(120));
            });
        }
    });
    port
}

/// UC-K6: a cell's pass killed from outside (its memory gone over, as the kernel ends it): the cell
/// says how it ended, keeps answering, and its sources are as they were.
#[test]
fn a_cells_pass_killed_from_outside_is_said_and_the_cell_answers_on() {
    let tmp = std::env::temp_dir().join(format!("zetlyn-modes-{}-killed-M", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let prepare = |root: &Path| {
        let csv = std::fs::read(root.join("sources/kev/kev.csv")).unwrap();
        let port = a_source_that_hangs(csv);
        let decl = root.join("sources/kev/source.yaml");
        let t = std::fs::read_to_string(&decl).unwrap();
        std::fs::write(&decl, format!("{}\nschedule:\n  every: 1s\n", t.replace("path: kev.csv", &format!("path: http://127.0.0.1:{port}/kev.csv")).trim_end())).unwrap();
        // Due by the time the cell starts.
        std::thread::sleep(std::time::Duration::from_millis(1200));
    };
    let w = World::start_prepared(Mode::M, tmp, None, true, &prepare);
    let before = claim_of(&w.root, "kev", "CVE-2026-0001", "vendorProject");
    let cell = w.host.display().to_string();
    let mut pid = String::new();
    wait_for("the cell's pass to start", || {
        let out = Command::new("pgrep").args(["-f", &format!("hosting run {cell}")]).output().unwrap();
        pid = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").to_string();
        !pid.is_empty()
    });
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(Command::new("kill").args(["-9", &pid]).status().unwrap().success());
    let last = w.host.join("last-run.json");
    wait_for("the cell to say how its pass ended", || std::fs::read_to_string(&last).is_ok_and(|s| s.contains("\"result\"")));
    let said = std::fs::read_to_string(&last).unwrap();
    assert!(!said.contains("\"success\""), "not a success: {said}");
    assert!(said.contains("signal") || said.contains("ended"), "{said}");
    assert_eq!(w.get("/about", "").0, 200, "the cell answers on");
    assert_eq!(claim_of(&w.root, "kev", "CVE-2026-0001", "vendorProject"), before, "its source as it was");
}

/// A source at an address that sends all of what it says, slowly: a read that takes a while.
fn a_slow_source(csv: Vec<u8>, millis: u64) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let csv = csv.clone();
            std::thread::spawn(move || {
                let mut s = stream;
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/csv\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", csv.len()).as_bytes());
                let half = csv.len() / 2;
                let _ = s.write_all(&csv[..half]);
                std::thread::sleep(std::time::Duration::from_millis(millis));
                let _ = s.write_all(&csv[half..]);
            });
        }
    });
    port
}

/// UC-B13: the app and `zetlyn run` (or two of anything) read the same source at the same time: one
/// reads, the other leaves it to it and says so, and what the source holds is whole.
#[test]
fn two_processes_reading_one_source_at_once_read_it_once() {
    let tmp = std::env::temp_dir().join(format!("zetlyn-modes-{}-two-readers", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let root = tmp.join("w");
    acme(&root, "");
    let csv = std::fs::read(root.join("sources/kev/kev.csv")).unwrap();
    let port = a_slow_source(csv, 1500);
    let decl = root.join("sources/kev/source.yaml");
    let t = std::fs::read_to_string(&decl).unwrap();
    std::fs::write(&decl, t.replace("path: kev.csv", &format!("path: http://127.0.0.1:{port}/kev.csv"))).unwrap();
    let dir = root.join("sources/kev").display().to_string();
    let start = { let dir = dir.clone(); move || Command::new(env!("CARGO_BIN_EXE_zetlyn")).args(["source", "update", &dir]).output() };
    let start_b = start.clone();
    let a = std::thread::spawn(start);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let b = start_b().unwrap();
    let a = a.join().unwrap().unwrap();
    let said = |o: &std::process::Output| format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    eprintln!("A: {} {}\nB: {} {}", a.status, said(&a), b.status, said(&b));
    assert!(a.status.success(), "the first reads: {}", said(&a));
    assert!(said(&b).contains("being read already"), "the second leaves it to the first, and says so: {}", said(&b));
    let (ok, printed) = z(&["claim", &dir, "CVE-2026-0001"]);
    assert!(ok, "whole: {printed}");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// UC-A2: the app started on a folder that is not a workspace yet makes it one, and asks what to
/// track first.
#[test]
fn the_app_on_an_empty_folder_makes_a_workspace_and_asks_what_to_track() {
    let tmp = std::env::temp_dir().join(format!("zetlyn-modes-{}-first", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let root = tmp.join("new-world");
    let port = free_port();
    let app = spawn(&[&root.display().to_string(), "--port", &port.to_string(), "--no-open"], &[], None, tmp.join("app.log"), port);
    assert!(root.join("workspace.yaml").exists() && root.join("sources").is_dir() && root.join("trackers").is_dir(), "made a workspace");
    let (s, _, page) = http(app.port, "GET", "/", "", "");
    assert_eq!(s, 200);
    assert!(page.contains("What do you want to track?") && page.contains("action=\"/new\""), "asks what to track first");
    drop(app);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// UC-A14: two worlds of one person on one machine, each its own folder and app: the second takes
/// the next port, and each answers as itself.
#[test]
fn two_worlds_on_one_machine_each_answer_as_themselves() {
    let tmp = std::env::temp_dir().join(format!("zetlyn-modes-{}-two-worlds", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (a, b) = (tmp.join("a"), tmp.join("b"));
    for (d, t) in [(&a, "Prices"), (&b, "Papers")] {
        std::fs::create_dir_all(d).unwrap();
        std::fs::write(d.join("workspace.yaml"), format!("title: {t}\n")).unwrap();
    }
    let port = free_port();
    let first = spawn(&[&a.display().to_string(), "--port", &port.to_string(), "--no-open"], &[], None, tmp.join("a.log"), port);
    let second = spawn(&[&b.display().to_string(), "--port", &port.to_string(), "--no-open"], &[], None, tmp.join("b.log"), port);
    assert_ne!(first.port, second.port, "the second on a port of its own");
    for (p, t) in [(first.port, "Prices"), (second.port, "Papers")] {
        let (_, _, page) = http(p, "GET", "/settings/profile", "", "");
        assert!(page.contains(&format!("value=\"{t}\"")), "{t} on {p}");
    }
    drop((first, second));
    let _ = std::fs::remove_dir_all(&tmp);
}

/// UC-B5: a source missed while the machine slept is read once when it wakes, not once for every
/// time it missed.
#[test]
fn a_source_missed_while_asleep_is_read_once() {
    let tmp = std::env::temp_dir().join(format!("zetlyn-modes-{}-asleep", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let root = tmp.join("w");
    acme(&root, "");
    let decl = root.join("sources/kev/source.yaml");
    let t = std::fs::read_to_string(&decl).unwrap();
    std::fs::write(&decl, format!("{}\nschedule:\n  every: 1s\n", t.trim_end())).unwrap();
    // Asleep for several of its rhythms.
    std::thread::sleep(std::time::Duration::from_millis(3500));
    let (ok, said) = z(&["run", &root.display().to_string(), "--once"]);
    assert!(ok, "{said}");
    assert_eq!(said.matches("test/kev update").count(), 1, "once: {said}");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// UC-F8, UC-G5: on one's own machine nobody signs in, so its owner proposes as the world itself;
/// and the app says it reads and tells only while it runs.
#[test]
fn on_ones_machine_the_owner_proposes_and_is_told_what_runs_when() {
    let l = World::start(Mode::L, "local-propose");
    let (s, _) = l.post("/settings/sources", "", "proposals.kev=world");
    assert_eq!(s, 303);
    let (s, page) = l.get("/trackers/cve/propose/test%2Fkev", "");
    assert_eq!(s, 200, "{page}");
    assert!(!page.contains("Sign in to propose") && page.contains("type=\"submit\""), "the owner proposes, signed in as nobody");
    let (_, page) = l.get("/settings/updates", "");
    assert!(page.contains("Quit Zetlyn and it stops"), "only while it runs, and says so");
}

/// UC-C10: what a world published and then made private is taken out of its hub, so nobody takes
/// it any more; whoever took it keeps their copy, and their next pull says it is withdrawn.
#[test]
fn what_is_made_private_is_withdrawn_from_the_hub_and_subscribers_keep_their_copy() {
    let w = World::start(Mode::P, "withdrawn");
    let owner = w.sign_in(OWNER);
    let ws = w.root.join("workspace.yaml");
    let t = std::fs::read_to_string(&ws).unwrap();
    std::fs::write(&ws, format!("{}\npublish:\n  to: hub\n", t.trim_end())).unwrap();
    let pass = || {
        let (ok, said) = z(&["hosting", "run", &w.host.display().to_string()]);
        assert!(ok, "{said}");
        said
    };
    let said = pass();
    let hub = w.root.join("hub");
    assert!(hub.join("trackers/test/cve/tags/latest").exists(), "published: {said}");
    assert!(hub.join("sources/test/kev/tags/latest").exists(), "{said}");

    // Somebody takes the tracker from that hub.
    let other = w.tmp.join("other");
    std::fs::create_dir_all(other.join("sources")).unwrap();
    std::fs::create_dir_all(other.join("trackers")).unwrap();
    std::fs::write(other.join("workspace.yaml"), "title: Other\n").unwrap();
    let (ok, said) = z(&["tracker", "subscribe", "test/cve", "--from", &hub.display().to_string(), "--at", &other.display().to_string()]);
    assert!(ok, "{said}");
    let held = other.join("trackers/cve/tracker.yaml");
    assert!(held.exists(), "{said}");

    // Made private: out of the hub, with a note where it was.
    let (s, _) = w.post("/settings/seen", &owner, "tracker=cve&visibility=private");
    assert_eq!(s, 303);
    let said = pass();
    assert!(said.contains("withdrawn"), "{said}");
    assert!(!hub.join("trackers/test/cve/tags/latest").exists() && !hub.join("trackers/test/cve/versions").exists(), "nothing of it to take");
    assert!(hub.join("trackers/test/cve/withdrawn").exists());

    assert!(held.exists(), "their copy stays");

    // A source said private by its owner: out of the hub too.
    let (s, _) = w.post("/settings/sources", &owner, "page.kev=private");
    assert_eq!(s, 303);
    let said = pass();
    assert!(!hub.join("sources/test/kev/tags/latest").exists(), "{said}");
    // The one who took it: told, and keeps what they hold.
    let theirs = std::fs::read_dir(other.join("sources")).unwrap().flatten().map(|e| e.path()).find(|p| std::fs::read_to_string(p.join("source.yaml")).is_ok_and(|t| t.contains("test/kev"))).expect("the source they took");
    let (ok, said) = z(&["source", "pull", &theirs.display().to_string()]);
    assert!(ok && said.contains("no longer published"), "{said}");
    assert!(theirs.join("source.yaml").exists(), "their copy stays");
    // And made public again, published again.
    let (s, _) = w.post("/settings/sources", &owner, "page.kev=public");
    assert_eq!(s, 303);
    let said = pass();
    assert!(hub.join("sources/test/kev/tags/latest").exists(), "{said}");
}

/// UC-A10: a world on a server of its own, back from its backup: as it was when the backup was made,
/// its data, its settings and its accounts, keys included.
#[test]
fn a_world_comes_back_from_its_backup_as_it_was() {
    let w = World::start(Mode::S, "backup");
    let owner = w.sign_in(OWNER);
    let (s, page) = w.post("/settings/keys", &owner, "name=script");
    assert_eq!(s, 200);
    let key = page.split("<pre id=\"api-key\">").nth(1).and_then(|k| k.split('<').next()).unwrap().to_string();
    let backups = w.tmp.join("backups");
    let (ok, said) = z(&["world", "backup", &w.root.display().to_string(), &backups.display().to_string()]);
    assert!(ok, "{said}");
    let archive = std::fs::read_dir(&backups).unwrap().flatten().map(|e| e.path()).find(|p| p.to_string_lossy().ends_with(".tar.gz")).expect("an archive");

    // Things change after it.
    std::fs::copy(fixture("update-2/vendor-a.csv"), w.root.join("sources/vendor-a/advisories.csv")).unwrap();
    let (ok, said) = z(&["source", "update", &w.root.join("sources/vendor-a").display().to_string()]);
    assert!(ok, "{said}");
    let (s, _) = w.post("/settings/profile", &owner, "title=Changed");
    assert_eq!(s, 303);
    let (tmp, _) = w.stop();

    // Back from the backup, in a folder of its own, served.
    let restored = tmp.join("restored");
    let (ok, said) = z(&["world", "import", &archive.display().to_string(), "--to", &restored.display().to_string()]);
    assert!(ok, "{said}");
    let back = World::start_in(Mode::S, tmp.join("again"), Some(restored.clone()), false);
    assert_eq!(claim_of(&restored, "vendor-a", "CVE-2026-0001", "cvss").0, "9.8", "its data as it was");
    assert!(back.workspace().contains("title: Acme"), "its settings as they were");
    let (s, _, _) = http_raw(back.port(), "GET", "/sources", &format!("x=y\r\nAuthorization: Bearer {key}"), "", b"");
    assert_eq!(s, 200, "its accounts and their keys as they were");
    drop(back);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// UC-C4, UC-G6: a tracker taken from another world's hub is kept current by the pass that keeps
/// everything else current, and what the publisher changes reaches the taker's trackers, where a
/// watch would hear it.
#[test]
fn a_tracker_taken_from_a_hub_keeps_itself_current() {
    let w = World::start(Mode::P, "taken");
    let ws = w.root.join("workspace.yaml");
    let t = std::fs::read_to_string(&ws).unwrap();
    std::fs::write(&ws, format!("{}\npublish:\n  to: hub\n", t.trim_end())).unwrap();
    let publish = || {
        let (ok, said) = z(&["hosting", "run", &w.host.display().to_string()]);
        assert!(ok, "{said}");
    };
    publish();
    let hub = w.root.join("hub");
    let other = w.tmp.join("other");
    std::fs::create_dir_all(other.join("sources")).unwrap();
    std::fs::create_dir_all(other.join("trackers")).unwrap();
    std::fs::write(other.join("workspace.yaml"), "title: Other\nupdate:\n  every: 15m\n").unwrap();
    let (ok, said) = z(&["tracker", "subscribe", "test/cve", "--from", &hub.display().to_string(), "--at", &other.display().to_string()]);
    assert!(ok, "{said}");
    let theirs = std::fs::read_dir(other.join("sources")).unwrap().flatten().map(|e| e.path()).find(|p| std::fs::read_to_string(p.join("source.yaml")).is_ok_and(|t| t.contains("test/vendor-a"))).expect("vendor-a taken");
    let slug = theirs.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(claim_of(&other, &slug, "CVE-2026-0001", "cvss").0, "9.8");

    // The publisher reads something new and publishes it.
    std::fs::copy(fixture("update-2/vendor-a.csv"), w.root.join("sources/vendor-a/advisories.csv")).unwrap();
    let (ok, said) = z(&["source", "update", &w.root.join("sources/vendor-a").display().to_string()]);
    assert!(ok, "{said}");
    publish();

    // The taker's own pass, nothing asked by hand: it is there.
    let (ok, said) = z(&["run", &other.display().to_string(), "--once"]);
    assert!(ok, "{said}");
    assert_eq!(claim_of(&other, &slug, "CVE-2026-0001", "cvss").0, "8.1", "kept current by its pass: {said}");
}

type Kept = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

/// A mail server on this machine that takes every message and keeps it: SMTP without TLS, as a
/// world may use one on its own machine.
fn a_mailer() -> (u16, Kept) {
    use std::io::BufRead;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let kept: Kept = Default::default();
    let k = kept.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let k = k.clone();
            std::thread::spawn(move || {
                let mut out = stream.try_clone().unwrap();
                let mut lines = std::io::BufReader::new(stream);
                let _ = out.write_all(b"220 test\r\n");
                let mut line = String::new();
                while lines.read_line(&mut line).unwrap_or(0) > 0 {
                    let upper = line.to_uppercase();
                    if upper.starts_with("DATA") {
                        let _ = out.write_all(b"354 go on\r\n");
                        let mut message = String::new();
                        let mut l = String::new();
                        while lines.read_line(&mut l).unwrap_or(0) > 0 && l != ".\r\n" {
                            message.push_str(&l);
                            l.clear();
                        }
                        k.lock().unwrap().push(message);
                        let _ = out.write_all(b"250 kept\r\n");
                    } else if upper.starts_with("QUIT") {
                        let _ = out.write_all(b"221 bye\r\n");
                        break;
                    } else {
                        let _ = out.write_all(b"250 ok\r\n");
                    }
                    line.clear();
                }
            });
        }
    });
    (port, kept)
}

/// A program at an address that takes what is posted to it and keeps the bodies.
fn a_webhook(listener: std::net::TcpListener) -> Kept {
    let kept: Kept = Default::default();
    let k = kept.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut s = stream;
            let mut raw = Vec::new();
            let mut buf = [0u8; 65536];
            // Headers, then as much body as they say.
            loop {
                let n = s.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&raw).into_owned();
                if let Some((h, b)) = text.split_once("\r\n\r\n") {
                    let len = h.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                    if b.len() >= len {
                        k.lock().unwrap().push(b.to_string());
                        break;
                    }
                }
            }
            let _ = s.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
        }
    });
    kept
}

fn wait_kept(kept: &Kept, what: &str) -> String {
    wait_for(what, || !kept.lock().unwrap().is_empty());
    kept.lock().unwrap().last().cloned().unwrap()
}

/// UC-D6, UC-G3: a world on a server of its own sends its mail through its own mail server: a
/// sign-in link that signs in; and where the mail server does not answer, the page still does.
#[test]
fn a_world_on_its_own_server_signs_people_in_by_its_own_mail() {
    let w = World::start(Mode::S, "mail");
    let (port, mails) = a_mailer();
    let ws = w.root.join("workspace.yaml");
    let t = std::fs::read_to_string(&ws).unwrap();
    std::fs::write(&ws, format!("{}\nmail:\n  smtp:\n    host: 127.0.0.1\n    port: {port}\n    tls: none\n    from: Acme <world@example.org>\n", t.trim_end())).unwrap();

    let (s, _) = w.post("/signin", "", "email=anna%40example.org");
    assert_eq!(s, 200);
    let mail = wait_kept(&mails, "the sign-in mail");
    assert!(mail.contains("To: anna@example.org") && mail.contains("From: Acme <world@example.org>"), "{mail}");
    let link = mail.split("/signin/").nth(1).and_then(|r| r.split(|c: char| c.is_whitespace()).next()).expect("a link in the mail");
    let (s, h, _) = http(w.port(), "GET", &format!("/signin/{link}"), "", "");
    assert_eq!(s, 303, "the link signs in: {h}");
    assert!(!cookies_of(&h).is_empty());

    // The mail server gone: the page answers still, and says the mail is on its way, as it cannot
    // tell a stranger whether that address is known.
    let t = std::fs::read_to_string(&ws).unwrap();
    std::fs::write(&ws, t.replace(&format!("port: {port}"), "port: 1")).unwrap();
    let (s, _) = w.post("/signin", "", "email=bob%40example.org");
    assert_eq!(s, 200);
    assert!(std::fs::read_to_string(&w.web.as_ref().unwrap().log).unwrap().contains("sign-in mail"), "and says why where it runs");
}

/// UC-G2: what a watch catches reaches a program by webhook and a person by mail; a webhook not
/// reached is told the next time, not lost.
#[test]
fn what_a_watch_catches_reaches_a_webhook_and_a_mail_and_is_not_lost() {
    let tmp = std::env::temp_dir().join(format!("zetlyn-modes-{}-watch", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let root = tmp.join("w");
    acme(&root, "");
    let (mail_port, mails) = a_mailer();
    let hook = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let hook_port = hook.local_addr().unwrap().port();
    drop(hook); // Not there yet.
    let ws = root.join("workspace.yaml");
    std::fs::write(&ws, format!("title: Acme\nmail:\n  smtp:\n    host: 127.0.0.1\n    port: {mail_port}\n    tls: none\n    from: world@example.org\n")).unwrap();
    std::fs::write(
        root.join("watches/vendor-a-critical.yaml"),
        format!("name: vendor-a-critical\ntitle: Critical at vendor A\nsource: test/vendor-a\nquery: severity=critical\ndeliver:\n- to: webhook\n  url: http://127.0.0.1:{hook_port}/hook\n- to: mail\n  address: anna@example.org\n"),
    )
    .unwrap();
    let run = || z(&["run", &root.display().to_string(), "--once"]).1;
    run(); // What there is now is where it starts.
    mails.lock().unwrap().clear();

    std::fs::copy(fixture("update-2/vendor-a.csv"), root.join("sources/vendor-a/advisories.csv")).unwrap();
    let (ok, said) = z(&["source", "update", &root.join("sources/vendor-a").display().to_string()]);
    assert!(ok, "{said}");
    let said = run();
    assert!(said.contains(&format!("127.0.0.1:{hook_port}")), "the webhook not reached is said: {said}");

    // Reached the next time: what was caught then, not lost.
    let hooked = a_webhook(std::net::TcpListener::bind(("127.0.0.1", hook_port)).unwrap());
    run();
    let body = wait_kept(&hooked, "the webhook");
    assert!(body.contains("CVE-2026-0004"), "{body}");
    let mail = wait_kept(&mails, "the mail");
    assert!(mail.contains("To: anna@example.org") && mail.contains("Critical at vendor A"), "{mail}");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// UC-F6: a source made only of what people propose, made from the Sources page: a declaration
/// whose fields are the ones said, taking proposals from anybody signed in, with its own page of
/// proposals.
#[test]
fn a_source_made_of_proposals_is_made_from_the_page() {
    let l = World::start(Mode::L, "proposals-source");
    let (s, h) = l.post("/proposals", "", "title=Stall+prices&identify=stall%2C+week&numbers=price&words=currency");
    assert_eq!(s, 303, "{h}");
    let decl = std::fs::read_to_string(l.root.join("sources/stall-prices/source.yaml")).expect("its declaration");
    assert!(decl.contains("type: proposals") && decl.contains("signed-in"), "{decl}");
    for field in ["stall", "week", "price", "currency"] {
        assert!(decl.contains(field), "{field}: {decl}");
    }
    let (s, page) = l.get("/proposals/stall-prices", "");
    assert_eq!(s, 200);
    assert!(page.contains("Stall prices"), "its own page of proposals");
}

/// UC-E4: a world's About page says to anybody who runs it, how to reach them, its imprint, and
/// what it makes public: its public trackers and the public pages of its sources, and nothing private.
#[test]
fn a_worlds_about_page_says_who_runs_it_and_what_it_makes_public() {
    for mode in SERVED {
        let w = World::start(mode, "about");
        let owner = w.sign_in(OWNER);
        let (s, _) = w.post("/settings/profile", &owner, "title=Acme&operator=Acme+Research+GmbH&contact=hello%40acme.example&imprint=Acme+Research+GmbH%2C+Hauptstr.+1%2C+Berlin&about=What+four+sources+say.");
        assert_eq!(s, 303, "{mode:?}");
        let (s, _) = w.post("/settings/sources", &owner, "page.exploits=private");
        assert_eq!(s, 303, "{mode:?}");
        let (s, page) = w.get("/about", "");
        assert_eq!(s, 200, "{mode:?}: for anybody");
        for said in ["Acme Research GmbH", "hello@acme.example", "Hauptstr. 1", "What four sources say.", "CVE", "Known exploited"] {
            assert!(page.contains(said), "{mode:?}: {said}");
        }
        assert!(!page.contains("Exploits"), "{mode:?}: a private source is not listed");
    }
}

/// UC-H10: a world on somebody else's machine of many worlds (P) and one on zetlyn.com (M): what one
/// exported the other takes in, and the two kept one by sync, asked from the first.
#[test]
fn a_world_from_another_hosting_and_one_on_zetlyn_com_move_and_stay_one() {
    let p = World::start(Mode::P, "other-hosting");
    let m = World::start(Mode::M, "other-hosting");
    let (owner_p, owner_m) = (p.sign_in(OWNER), m.sign_in(OWNER));

    // Its archive taken in on zetlyn.com, in pieces, handed to the cell's server.
    let archive = p.tmp.join("p.tar.gz");
    let (ok, said) = z(&["world", "export", &p.root.display().to_string(), "--to", &archive.display().to_string()]);
    assert!(ok, "{said}");
    let bytes = std::fs::read(&archive).unwrap();
    let (s, begun) = m.post("/settings/upload", &owner_m, &format!("size={}&name=p.tar.gz", bytes.len()));
    assert_eq!(s, 200, "{begun}");
    let id = begun.split("\"id\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
    assert_eq!(m.put(&format!("/settings/upload/{id}/0"), &owner_m, &bytes).0, 200);
    let (s, b) = m.post(&format!("/settings/upload/{id}/done"), &owner_m, "");
    assert_eq!(s, 200, "{b}");
    assert!(m.host.join("incoming/import.json").exists());

    // And the two kept one, asked from the other hosting.
    let (s, _) = m.post("/settings/access", &owner_m, &format!("owners={}", enc(OWNER)));
    assert_eq!(s, 303);
    let (s, _) = p.post("/settings/access", &owner_p, &format!("owners={}", enc(OWNER)));
    assert_eq!(s, 303);
    let key = sync_key(&m, &owner_m);
    let last = p.root.join(".zetlyn/sync/last.json");
    let synced = |what: &str| {
        wait_for(what, || std::fs::read_to_string(&last).is_ok_and(|s| !s.contains("\"running\"")));
        let l: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&last).unwrap()).unwrap();
        assert_eq!(l["state"], "ok", "{what}: {l}");
    };
    let (s, _) = p.post("/settings/sync", &owner_p, &format!("url={}&key={key}", enc(&m.url())));
    assert_eq!(s, 303);
    synced("the first sync");
    edit(&m.root.join("trackers/cve/tracker.yaml"), "title: CVE", "title: CVE on zetlyn.com");
    let (s, _) = p.post("/settings/sync", &owner_p, "");
    assert_eq!(s, 303);
    synced("the second sync");
    assert!(std::fs::read_to_string(p.root.join("trackers/cve/tracker.yaml")).unwrap().contains("CVE on zetlyn.com"));
}

/// A place that says what Zetlyn's latest release is, as GitHub's releases do.
fn a_release_feed(tag: &str) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let body = format!(r#"{{"tag_name":"{tag}","body":"- something new"}}"#);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut s = stream;
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
        }
    });
    port
}

/// UC-A7: a newer release out: the app on one's machine says so on its About page, with how to
/// take it, and so does `zetlyn world upgrade --check`; the newest, it says that.
#[test]
fn the_app_says_when_a_newer_release_is_out() {
    let tmp = std::env::temp_dir().join(format!("zetlyn-modes-{}-releases", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let root = tmp.join("w");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("workspace.yaml"), "title: Mine\n").unwrap();
    for (tag, newer) in [("v99.0.0", true), (concat!("v", env!("CARGO_PKG_VERSION")), false)] {
        let feed = format!("http://127.0.0.1:{}", a_release_feed(tag));
        let port = free_port();
        let app = spawn(&[&root.display().to_string(), "--port", &port.to_string(), "--no-open"], &[("ZETLYN_RELEASES", feed.clone())], None, tmp.join("app.log"), port);
        let (_, _, page) = http(app.port, "GET", "/about", "", "");
        let out = Command::new(env!("CARGO_BIN_EXE_zetlyn")).args(["world", "upgrade", "--check"]).env("ZETLYN_RELEASES", &feed).output().unwrap();
        let said = String::from_utf8_lossy(&out.stdout).into_owned();
        if newer {
            assert!(page.contains("v99.0.0") && page.contains("is out") && page.contains("install.sh"), "the page says it, and how");
            assert!(said.contains("v99.0.0 is out"), "{said}");
        } else {
            assert!(page.contains("the newest"), "the newest, said");
            assert!(said.contains("is current"), "{said}");
        }
        drop(app);
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
