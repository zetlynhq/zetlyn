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
                        std::fs::write(&ws, format!("{}\nurl: http://127.0.0.1:{port}\n", t.trim_end())).unwrap();
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
