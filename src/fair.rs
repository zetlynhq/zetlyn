//! Fair to other sites: what a visitor has Zetlyn read elsewhere (the page to mark in `/frame`,
//! its images, fonts and styles in `/r`, a preview, a new watch's first read) is bounded per site,
//! whoever asks and from however many addresses, and kept for a minute, so that asking again costs
//! the site nothing. Without that, anybody could have Zetlyn read a site over and over for them,
//! from Zetlyn's address instead of their own.
//!
//! Chrome, which reads one page at a time, has at most `WAITING` visitors waiting for it; past that
//! a visitor gets the page as served, and the checks of watches never wait long behind visitors.
//!
//! The checks of watches are not counted here: they read each page once a round (`check.rs`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::fetch::{self, Fetched, Got};

/// What a visitor has read: a page (or a page rendered), or what a page loads.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Page,
    Resource,
}

/// Reads of one site a minute, for all visitors together.
fn most(kind: Kind) -> usize {
    match kind {
        Kind::Page => 30,
        Kind::Resource => 600,
    }
}

const MINUTE: Duration = Duration::from_secs(60);
/// How long what was read is kept for the next visitor who asks.
const KEPT: Duration = Duration::from_secs(60);
/// Kept up to this many bytes, the oldest first to go.
const KEPT_MOST: usize = 64 * 1024 * 1024;
/// Visitors waiting for Chrome at most.
const WAITING: usize = 2;

/// When each site was read lately, by kind.
type Reads = HashMap<(String, Kind), Vec<Instant>>;
static READS: Mutex<Option<Reads>> = Mutex::new(None);
static GOT: Mutex<Vec<(String, Instant, Got)>> = Mutex::new(Vec::new());
static IN_LINE: AtomicUsize = AtomicUsize::new(0);

/// Whether one more read of `url`'s site may go out now: counted if so, else what to say.
pub fn admit(url: &str, kind: Kind) -> Result<(), String> {
    admit_at(url, kind, Instant::now())
}

fn admit_at(url: &str, kind: Kind, at: Instant) -> Result<(), String> {
    let host = site(url);
    let mut reads = READS.lock().unwrap_or_else(|e| e.into_inner());
    let reads = reads.get_or_insert_with(HashMap::new);
    if reads.len() > 10_000 {
        reads.retain(|_, times| times.last().is_some_and(|t| at.duration_since(*t) < MINUTE));
    }
    let times = reads.entry((host.clone(), kind)).or_default();
    times.retain(|t| at.duration_since(*t) < MINUTE);
    if times.len() >= most(kind) {
        return Err(format!("{host} has been read for many people in the last minute; to be fair to the site, Zetlyn waits a moment. Please try again in a minute."));
    }
    times.push(at);
    Ok(())
}

/// The site an address is on, as counted: its host in lower case, without `www.` or a port.
fn site(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = if host.starts_with('[') { host.split_inclusive(']').next().unwrap_or(host) } else { host.split(':').next().unwrap_or(host) };
    host.to_ascii_lowercase().trim_start_matches("www.").to_string()
}

/// A page for a visitor: as `fetch::get`, kept for a minute, counted against its site.
pub fn page(url: &str) -> Result<Fetched, String> {
    let got = bytes(url, Kind::Page)?;
    let body = fetch::decode(&got.bytes, &got.content_type);
    Ok(Fetched { url: got.url, content_type: got.content_type, body })
}

/// What a page loads, for a visitor: at most `fetch::BODY_LIMIT`, kept for a minute, counted
/// against its site.
pub fn resource(url: &str) -> Result<Got, String> {
    bytes(url, Kind::Resource)
}

/// A page for a visitor as Chrome shows it (`render::html`): where it ended up, and its HTML;
/// kept for a minute, counted against its site, and only while Chrome is not too busy (`in_line`).
pub fn rendered(url: &str) -> Result<(String, String), String> {
    let got = kept_or(&format!("rendered {url}"), || {
        admit(url, Kind::Page)?;
        let (at, html) = in_line(|| crate::render::html(url))?;
        Ok(Got { url: at, content_type: "text/html".into(), bytes: html.into_bytes() })
    })?;
    Ok((got.url, String::from_utf8(got.bytes).unwrap_or_default()))
}

fn bytes(url: &str, kind: Kind) -> Result<Got, String> {
    kept_or(url, || {
        admit(url, kind)?;
        fetch::get_bytes(url, fetch::BODY_LIMIT)
    })
}

/// What is kept under `key`, or what `read` reads, then kept; a failed read is not kept.
fn kept_or(key: &str, read: impl FnOnce() -> Result<Got, String>) -> Result<Got, String> {
    {
        let mut kept = GOT.lock().unwrap_or_else(|e| e.into_inner());
        kept.retain(|(_, at, _)| at.elapsed() < KEPT);
        if let Some((_, _, got)) = kept.iter().find(|(k, _, _)| k == key) {
            return Ok(got.clone());
        }
    }
    let got = read()?;
    let mut kept = GOT.lock().unwrap_or_else(|e| e.into_inner());
    kept.retain(|(k, _, _)| k != key);
    kept.push((key.to_string(), Instant::now(), got.clone()));
    while kept.iter().map(|(_, _, g)| g.bytes.len()).sum::<usize>() > KEPT_MOST && kept.len() > 1 {
        kept.remove(0);
    }
    Ok(got)
}

/// `render` for a visitor, if no more than `WAITING` others are waiting for Chrome already.
pub fn in_line<T>(render: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    // One rendering, and `WAITING` behind it.
    if IN_LINE.fetch_add(1, Ordering::SeqCst) > WAITING {
        IN_LINE.fetch_sub(1, Ordering::SeqCst);
        return Err("Zetlyn is reading many pages in a browser just now; please try again in a minute.".into());
    }
    let result = render();
    IN_LINE.fetch_sub(1, Ordering::SeqCst);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_is_read_so_often_a_minute_and_again_after() {
        let t = Instant::now();
        for _ in 0..most(Kind::Page) {
            admit_at("https://www.fair-test-one.example/a", Kind::Page, t).unwrap();
        }
        assert!(admit_at("https://fair-test-one.example/b?c", Kind::Page, t).is_err(), "the same site, with or without www");
        assert!(admit_at("https://fair-test-one.example/b", Kind::Resource, t).is_ok(), "what pages load is counted apart");
        assert!(admit_at("https://fair-test-two.example/", Kind::Page, t).is_ok(), "another site");
        assert!(admit_at("https://fair-test-one.example/b", Kind::Page, t + MINUTE).is_ok(), "a minute later");
    }

    #[test]
    fn the_site_is_the_host() {
        assert_eq!(site("https://WWW.Example.com:8443/x?y#z"), "example.com");
        assert_eq!(site("https://user@shop.example.com/"), "shop.example.com");
        assert_eq!(site("http://[2001:db8::1]:80/"), "[2001:db8::1]");
    }

    #[test]
    fn only_so_many_wait_for_chrome() {
        // One rendering and `WAITING` waiting: the next is turned away, and let in once one is done.
        let held: Vec<_> = (0..=WAITING).map(|_| IN_LINE.fetch_add(1, Ordering::SeqCst)).collect();
        assert!(in_line(|| Ok(())).is_err());
        held.iter().for_each(|_| {
            IN_LINE.fetch_sub(1, Ordering::SeqCst);
        });
        assert!(in_line(|| Ok(())).is_ok());
    }
}
