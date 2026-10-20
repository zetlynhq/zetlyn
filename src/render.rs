//! A page as a browser shows it once its scripts ran: read by a headless Chrome that this process
//! starts and drives over the DevTools protocol (CDP).
//!
//! Chrome is driven through a pipe (`--remote-debugging-pipe`), not a port: nothing else on the
//! machine can drive it, and it ends when this process does. Each page is read in a browser context
//! of its own (no cookies or storage carried from one page to the next), and every request the page
//! makes is seen first (`Fetch`): images, fonts and media are not loaded at all, since only the
//! page's text and structure are wanted; an address that is not on the public internet is refused,
//! as `outbound.rs` refuses it for a plain read. One page at a time.
//!
//! `ZETLYN_CHROME` names the Chrome to use, or `off`; without it the usual places are looked in.
//! `ZETLYN_CHROME_SANDBOX=off` starts it without its own sandbox: only where something else is the
//! sandbox, as the container of the Docker image is, which would not let Chrome make its own.

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value as J};

/// How long a page may take to load and settle before what it shows by then is taken.
const DEADLINE: Duration = Duration::from_secs(20);
/// How long a loaded page must ask for nothing more to count as settled.
const QUIET: Duration = Duration::from_millis(1200);

struct Config {
    chrome: Option<PathBuf>,
    profile: PathBuf,
}

static CONFIG: OnceLock<Config> = OnceLock::new();
static CHROME: Mutex<Option<Chrome>> = Mutex::new(None);

/// Where Chrome is, and where it keeps its profile (`<data>/chrome`). Once, at the start.
pub fn configure(data: &Path) {
    let chrome = match std::env::var("ZETLYN_CHROME") {
        Ok(v) if v.trim() == "off" || v.trim().is_empty() => None,
        Ok(v) => Some(PathBuf::from(v.trim())),
        Err(_) => [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/opt/google/chrome/chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/usr/bin/google-chrome",
            "/headless-shell/headless-shell",
        ]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file()),
    };
    let _ = CONFIG.set(Config { chrome, profile: data.join("chrome") });
}

/// Whether pages can be rendered here at all.
pub fn available() -> bool {
    CONFIG.get().is_some_and(|c| c.chrome.is_some())
}

/// For the start line and the admin's system page.
pub fn describe() -> String {
    match CONFIG.get().and_then(|c| c.chrome.as_ref()) {
        Some(p) => format!("pages rendered by {}", p.display()),
        None => "no Chrome: pages as served only".into(),
    }
}

/// Chrome ended, when this process ends by itself.
pub fn shutdown() {
    if let Ok(mut c) = CHROME.lock() {
        if let Some(mut chrome) = c.take() {
            end(chrome.child.id() as i32);
            let _ = chrome.child.wait();
        }
    }
    if let Some(c) = CONFIG.get() {
        let _ = std::fs::remove_dir_all(own_profile(&c.profile));
    }
}

/// Chrome's process group: Chrome runs in a group of its own with its helpers, so that all of it
/// ends at once. Chrome does not end by itself when its pipe closes; this process ends it, also
/// when it is ended by a signal (`guard`), and on Linux the kernel does when it dies outright.
static GROUP: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

fn end(group: i32) {
    if group > 0 {
        // SAFETY: kill is a plain system call.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
}

extern "C" fn on_signal(signal: libc::c_int) {
    end(GROUP.load(std::sync::atomic::Ordering::SeqCst));
    // SAFETY: signal and raise are async-signal-safe; the default action then ends this process.
    unsafe {
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

/// Ctrl-C, `kill` and a closed terminal end Chrome before they end this process.
fn guard() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        for s in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            // SAFETY: the handler does only async-signal-safe things.
            unsafe {
                libc::signal(s, on_signal as *const () as libc::sighandler_t);
            }
        }
    });
}

/// This process's own profile, `<data>/chrome/<pid>`: never one a Chrome left over from an
/// earlier run still holds. Those of processes gone are cleared away.
fn own_profile(profile: &Path) -> PathBuf {
    profile.join(std::process::id().to_string())
}

fn clear_stale(profile: &Path) {
    let Ok(dirs) = std::fs::read_dir(profile) else { return };
    for d in dirs.flatten() {
        let pid: Option<i32> = d.file_name().to_str().and_then(|n| n.parse().ok());
        // SAFETY: kill with signal 0 only asks whether the process is there.
        if pid.is_some_and(|p| unsafe { libc::kill(p, 0) } != 0) {
            let _ = std::fs::remove_dir_all(d.path());
        }
    }
}

/// Chrome started before anybody needs it, so that the first page rendered does not wait for it
/// to start (seconds). Nothing when there is no Chrome, or it runs already.
pub fn warm() {
    let Some(path) = CONFIG.get().and_then(|c| c.chrome.clone()) else { return };
    let profile = CONFIG.get().map(|c| c.profile.clone()).unwrap_or_default();
    let mut slot = CHROME.lock().unwrap_or_else(|e| e.into_inner());
    if slot.as_mut().is_none_or(|c| !c.alive()) {
        match Chrome::start(&path, &profile) {
            Ok(c) => *slot = Some(c),
            Err(e) => eprintln!("chrome: {e}"),
        }
    }
}

/// The page at `url` once its scripts ran: where it ended up, and its HTML. Its `<noscript>` is
/// taken out: with scripts on, a browser does not show it either.
pub fn html(url: &str) -> Result<(String, String), String> {
    let config = CONFIG.get().ok_or("rendering is not configured")?;
    let path = config.chrome.as_ref().ok_or("no Chrome here")?;
    let mut slot = CHROME.lock().unwrap_or_else(|e| e.into_inner());
    if slot.as_mut().is_none_or(|c| !c.alive()) {
        // What is left of a Chrome that ended (its helpers) goes with it.
        if let Some(mut old) = slot.take() {
            end(old.child.id() as i32);
            let _ = old.child.wait();
        }
        *slot = Some(Chrome::start(path, &config.profile)?);
    }
    let chrome = slot.as_mut().expect("just started");
    let result = chrome.render(url);
    if result.is_err() && !chrome.alive() {
        end(chrome.child.id() as i32);
        *slot = None;
    }
    let (at, html) = result?;
    // As large as a page read without Chrome may be, and no larger: past that it is read without.
    if html.len() as u64 > crate::fetch::BODY_LIMIT {
        return Err(format!("{url}: larger than {} MB once its scripts ran", crate::fetch::BODY_LIMIT / 1024 / 1024));
    }
    let noscript = regex::Regex::new(r"(?is)<noscript\b.*?</noscript\s*>").expect("a fixed pattern");
    Ok((at, noscript.replace_all(&html, "").into_owned()))
}

struct Chrome {
    child: Child,
    to: std::io::PipeWriter,
    from: Receiver<J>,
    next: u64,
    agent: String,
}

/// What a page's request becomes: let through, or refused. Images, fonts and media are never
/// wanted; other requests go only to the public internet (or stay in the page: `data:`, `blob:`).
fn allowed(url: &str, kind: &str) -> bool {
    if matches!(kind, "Image" | "Media" | "Font" | "Ping" | "CSPViolationReport" | "Prefetch" | "Manifest" | "SignedExchange") {
        return false;
    }
    if url.starts_with("data:") || url.starts_with("blob:") {
        return true;
    }
    (url.starts_with("https://") || url.starts_with("http://")) && !unwanted(url) && crate::outbound::public_target(url).is_ok()
}

/// Consent boxes, adverts and trackers: none of it is what anybody watches; consent boxes cover
/// the page to mark, adverts change on every read, and all of it costs time.
const UNWANTED: &[&str] = &[
    // Consent
    "fundingchoicesmessages.google.com", "cookielaw.org", "onetrust.com", "cookiebot.com", "cookiebot.eu", "usercentrics.eu",
    "usercentrics.com", "didomi.io", "privacy-center.org", "quantcast.com", "quantserve.com", "consensu.org", "consentmanager.net",
    "consentmanager.de", "sp-prod.net", "privacy-mgmt.com", "sourcepoint.com", "trustarc.com", "truste.com", "iubenda.com",
    "cookieyes.com", "cookie-script.com", "termly.io", "osano.com", "cookiefirst.com", "cookiehub.net", "ccm19.de", "cookie-law-info.com",
    // Adverts and tracking
    "doubleclick.net", "googlesyndication.com", "googleadservices.com", "googletagservices.com", "googletagmanager.com",
    "google-analytics.com", "adservice.google.com", "amazon-adsystem.com", "adnxs.com", "criteo.com", "criteo.net", "taboola.com",
    "outbrain.com", "connect.facebook.net", "hotjar.com", "clarity.ms", "scorecardresearch.com", "moatads.com", "pubmatic.com",
    "rubiconproject.com", "casalemedia.com", "adsrvr.org", "yieldlove.com", "smartadserver.com", "teads.tv",
];

fn unwanted(url: &str) -> bool {
    let host = url.split_once("://").map_or("", |(_, r)| r).split(['/', '?', '#', ':']).next().unwrap_or("").to_ascii_lowercase();
    UNWANTED.iter().any(|d| host == *d || host.ends_with(&format!(".{d}"))) || host.starts_with("adservice.google.")
}

impl Chrome {
    fn start(path: &Path, profile: &Path) -> Result<Chrome, String> {
        // Chrome reads its commands on fd 3 and writes its answers on fd 4.
        let (chrome_reads, to) = std::io::pipe().map_err(|e| e.to_string())?;
        let (mut from, chrome_writes) = std::io::pipe().map_err(|e| e.to_string())?;
        let (r, w) = (chrome_reads.as_raw_fd(), chrome_writes.as_raw_fd());
        let mut command = Command::new(path);
        command
            .args([
                "--headless",
                "--remote-debugging-pipe",
                &format!("--user-data-dir={}", own_profile(profile).display()),
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-extensions",
                "--disable-background-networking",
                "--disable-component-update",
                "--disable-default-apps",
                "--disable-sync",
                // Nothing to Google: no crash reports (one could hold the page being read), no pings.
                "--disable-crash-reporter",
                "--disable-breakpad",
                "--no-pings",
                "--disable-domain-reliability",
                "--disable-gpu",
                "--disable-dev-shm-usage",
                "--mute-audio",
                "--hide-scrollbars",
                "--blink-settings=imagesEnabled=false",
                "--window-size=1280,900",
                "about:blank",
            ])
            .args(std::env::var("ZETLYN_CHROME_SANDBOX").is_ok_and(|v| v == "off").then_some("--no-sandbox"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: between fork and exec only setpgid, prctl, dup2 and fcntl: async-signal-safe.
        unsafe {
            command.pre_exec(move || {
                libc::setpgid(0, 0);
                // Ended with the thread that started it, which here lives as long as the process.
                #[cfg(target_os = "linux")]
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                for (fd, at) in [(r, 3), (w, 4)] {
                    let done = if fd == at { libc::fcntl(fd, libc::F_SETFD, 0) } else { libc::dup2(fd, at) };
                    if done < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        clear_stale(profile);
        guard();
        let child = command.spawn().map_err(|e| format!("{}: {e}", path.display()))?;
        GROUP.store(child.id() as i32, std::sync::atomic::Ordering::SeqCst);
        drop(chrome_reads);
        drop(chrome_writes);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut pending = Vec::new();
            let mut buf = [0u8; 65536];
            loop {
                let n = match from.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                pending.extend_from_slice(&buf[..n]);
                while let Some(end) = pending.iter().position(|b| *b == 0) {
                    let message: Vec<u8> = pending.drain(..=end).collect();
                    if let Ok(m) = serde_json::from_slice::<J>(&message[..end]) {
                        if tx.send(m).is_err() {
                            return;
                        }
                    }
                }
            }
        });
        let mut chrome = Chrome { child, to, from: rx, next: 0, agent: String::new() };
        let version = chrome.call("Browser.getVersion", json!({}), None, Instant::now() + Duration::from_secs(15), &mut Page::default())?;
        chrome.agent = format!("{} {}", version["userAgent"].as_str().unwrap_or("Chrome"), crate::fetch::AGENT);
        Ok(chrome)
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn send(&mut self, method: &str, params: J, session: Option<&str>) -> Result<u64, String> {
        self.next += 1;
        let mut m = json!({ "id": self.next, "method": method, "params": params });
        if let Some(s) = session {
            m["sessionId"] = json!(s);
        }
        let mut bytes = serde_json::to_vec(&m).map_err(|e| e.to_string())?;
        bytes.push(0);
        self.to.write_all(&bytes).map_err(|e| format!("Chrome: {e}"))?;
        Ok(self.next)
    }

    /// A command, and its answer; what the page does meanwhile is seen to (`event`).
    fn call(&mut self, method: &str, params: J, session: Option<&str>, until: Instant, page: &mut Page) -> Result<J, String> {
        let id = self.send(method, params, session)?;
        loop {
            let left = until.checked_duration_since(Instant::now()).ok_or_else(|| format!("Chrome: {method} took too long"))?;
            match self.from.recv_timeout(left) {
                Ok(m) if m["id"].as_u64() == Some(id) => {
                    return match m.get("error") {
                        Some(e) => Err(format!("Chrome: {method}: {}", e["message"].as_str().unwrap_or("failed"))),
                        None => Ok(m["result"].clone()),
                    };
                }
                Ok(m) => self.event(&m, page)?,
                Err(RecvTimeoutError::Timeout) => return Err(format!("Chrome: {method} took too long")),
                Err(RecvTimeoutError::Disconnected) => return Err("Chrome ended".into()),
            }
        }
    }

    fn event(&mut self, m: &J, page: &mut Page) -> Result<(), String> {
        match m["method"].as_str() {
            Some("Fetch.requestPaused") => {
                let p = &m["params"];
                let id = p["requestId"].clone();
                let session = m["sessionId"].as_str().map(str::to_string);
                page.last = Instant::now();
                if allowed(p["request"]["url"].as_str().unwrap_or(""), p["resourceType"].as_str().unwrap_or("")) {
                    self.send("Fetch.continueRequest", json!({ "requestId": id }), session.as_deref())?;
                } else {
                    self.send("Fetch.failRequest", json!({ "requestId": id, "errorReason": "BlockedByClient" }), session.as_deref())?;
                }
            }
            Some("Page.loadEventFired") => {
                page.loaded = true;
                page.last = Instant::now();
            }
            _ => {}
        }
        Ok(())
    }

    fn render(&mut self, url: &str) -> Result<(String, String), String> {
        crate::outbound::public_target(url)?;
        let until = Instant::now() + DEADLINE;
        let mut page = Page::default();
        let context = self.call("Target.createBrowserContext", json!({ "disposeOnDetach": true }), None, until, &mut page)?["browserContextId"].clone();
        let read = self.read(url, &context, until, &mut page);
        let _ = self.call("Target.disposeBrowserContext", json!({ "browserContextId": context }), None, Instant::now() + Duration::from_secs(5), &mut page);
        read
    }

    fn read(&mut self, url: &str, context: &J, until: Instant, page: &mut Page) -> Result<(String, String), String> {
        let target = self.call("Target.createTarget", json!({ "url": "about:blank", "browserContextId": context }), None, until, page)?["targetId"].clone();
        let session = self.call("Target.attachToTarget", json!({ "targetId": target, "flatten": true }), None, until, page)?["sessionId"].as_str().unwrap_or("").to_string();
        let s = Some(session.as_str());
        let agent = self.agent.clone();
        self.call("Network.setUserAgentOverride", json!({ "userAgent": agent }), s, until, page)?;
        self.call("Fetch.enable", json!({ "patterns": [{ "urlPattern": "*", "requestStage": "Request" }] }), s, until, page)?;
        self.call("Page.enable", json!({}), s, until, page)?;
        page.last = Instant::now();
        let went = self.call("Page.navigate", json!({ "url": url }), s, until, page)?;
        if let Some(e) = went["errorText"].as_str().filter(|e| !e.is_empty()) {
            return Err(format!("{url}: {e}"));
        }
        // Loaded, and then a moment without new requests: what scripts fetch after the load is in.
        while Instant::now() < until && !(page.loaded && page.last.elapsed() >= QUIET) {
            match self.from.recv_timeout(Duration::from_millis(200)) {
                Ok(m) => self.event(&m, page)?,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err("Chrome ended".into()),
            }
        }
        let got = self.call(
            "Runtime.evaluate",
            json!({ "expression": "location.href + '\\n' + document.documentElement.outerHTML", "returnByValue": true }),
            s,
            Instant::now() + Duration::from_secs(10),
            page,
        )?;
        let text = got["result"]["value"].as_str().ok_or("Chrome: the page gave no HTML")?;
        let (at, html) = text.split_once('\n').unwrap_or((url, text));
        Ok((at.to_string(), html.to_string()))
    }
}

/// What a page being read has done so far.
struct Page {
    loaded: bool,
    /// When it last asked for something, or finished loading.
    last: Instant,
}

impl Default for Page {
    fn default() -> Page {
        Page { loaded: false, last: Instant::now() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rendered_page_loads_no_images_and_nothing_private() {
        assert!(!allowed("https://example.com/a.png", "Image"));
        assert!(!allowed("https://example.com/f.woff2", "Font"));
        assert!(!allowed("http://127.0.0.1:4848/admin", "XHR"));
        assert!(!allowed("http://169.254.169.254/latest/meta-data/", "Fetch"));
        assert!(!allowed("file:///etc/passwd", "Document"));
        assert!(allowed("data:text/javascript,1", "Script"));
        assert!(!allowed("https://fundingchoicesmessages.google.com/i/pub-1?ers=3", "Script"));
        assert!(!allowed("https://cdn.cookielaw.org/scripttemplates/otSDKStub.js", "Script"));
        assert!(!allowed("https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js", "Script"));
        assert!(!allowed("https://adservice.google.de/adsid/integrator.js", "Script"));
        assert!(!unwanted("https://notcookiebot.com/x") && !unwanted("https://www.google.com/"));
    }
}
