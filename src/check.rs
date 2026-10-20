//! Watching: the first read of a new watch, and every check after it.
//!
//! A page watched by many is read once per round, whoever watches it.

use std::collections::HashMap;

use crate::anchor::{self, Anchor, Found};
use crate::fetch;
use crate::page;
use crate::render;
use crate::store::{now, Mode, Store, Watch};
use crate::text;

/// A page as it is served, or why it could not be read.
pub fn read(url: &str) -> Result<String, String> {
    let f = fetch::get(url).map_err(|e| said(url, &e))?;
    if !f.is_html() {
        return Err(format!("{url}: not a web page ({}); only web pages for now", f.content_type));
    }
    if walled(&f.body) {
        return Err(refused(url));
    }
    Ok(page::sanitized(&f.body))
}

/// A page as a browser shows it once its scripts ran (`render.rs`).
pub fn rendered(url: &str) -> Result<String, String> {
    let (_, html) = render::html(url)?;
    if walled(&html) {
        return Err(refused(url));
    }
    Ok(page::sanitized(&html))
}

/// Whether a page is a wall instead of the page: a captcha, a check that a person is there.
/// Zetlyn reads as a program and says so; a site that will not have that is not got around.
pub fn walled(html: &str) -> bool {
    const WALLS: &[&str] = &[
        "api-services-support@amazon.com",
        "/errors/validateCaptcha",
        "Enter the characters you see below",
        "cf-browser-verification",
        "/cdn-cgi/challenge-platform/",
        "<title>Just a moment...</title>",
        "Attention Required! | Cloudflare",
        "geo.captcha-delivery.com",
        "px-captcha",
        "Please verify you are a human",
        "Access to this page has been denied",
    ];
    // A wall is a short page; a long one that only carries a captcha form somewhere is a page.
    html.len() < 60_000 && WALLS.iter().any(|w| html.contains(w))
}

/// What to tell a person when a site will not be read by a program.
pub fn refused(url: &str) -> String {
    let host = url.split_once("://").map_or(url, |(_, r)| r).split(['/', '?', '#']).next().unwrap_or(url).trim_start_matches("www.");
    format!("{host} does not let programs read this page: it answers with a check that a person is there (a captcha). Zetlyn does not get around that, so this page cannot be watched. Another shop or site with the same information may work.")
}

/// A read that failed, said so a person (or an LLM telling one) knows what it means.
pub fn said(url: &str, e: &str) -> String {
    if [" HTTP 403", " HTTP 429", " HTTP 503", " HTTP 401"].iter().any(|s| e.ends_with(s)) {
        let host = url.split_once("://").map_or(url, |(_, r)| r).split(['/', '?', '#']).next().unwrap_or(url).trim_start_matches("www.");
        return format!("{host} refused to answer Zetlyn ({}): it does not let programs read this page, or not just now. Zetlyn does not get around that; another site with the same information may work.", e.rsplit(": ").next().unwrap_or(e));
    }
    e.to_string()
}

/// A page read the way its watch reads it.
pub fn read_as(url: &str, render: bool) -> Result<String, String> {
    if render { rendered(url) } else { read(url) }
}

/// A page as a person marking it sees it: rendered where Chrome is at hand, else as served.
/// Whether it was rendered comes with it.
pub fn seen(url: &str) -> Result<(String, bool), String> {
    let served = read(url)?;
    if render::available() {
        if let Ok(html) = rendered(url) {
            return Ok((html, true));
        }
    }
    Ok((served, false))
}

/// Whether a page rendered says clearly more than the same page served: its scripts make what a
/// person reads there, so a watch of the whole page must read it rendered.
/// Whether a page as served says next to nothing: its scripts make what a person reads, so only a
/// browser shows it. A page with a few lines of text is no page to mark in as served.
pub fn thin(html: &str) -> bool {
    text::lines(&page::sanitized(html)).iter().map(|l| l.split_whitespace().count()).sum::<usize>() < 120
}

fn says_more(rendered: &[String], served: &[String]) -> bool {
    let words = |lines: &[String]| lines.iter().map(|l| l.split_whitespace().count()).sum::<usize>();
    let (r, s) = (words(rendered), words(served));
    r > s + 40.max(s / 4)
}

/// What a watch reads on a page that has been read.
pub fn value(mode: &Mode, html: &str) -> Result<(String, bool), &'static str> {
    match mode {
        Mode::Place(a) => match anchor::find(html, a) {
            Found::Here { value, moved } => Ok((value, moved)),
            Found::Unclear => Err("unclear"),
            Found::Gone => Err("gone"),
        },
        Mode::Page { volatile } => Ok((text::kept(text::lines(html), volatile).join("\n"), false)),
    }
}

/// A new watch: a marked place, which must be found where it was marked, or the whole page, read
/// twice so that what changes on its own is known. Returns the watch's mode, its first value, and
/// whether it is read rendered: only where the page as served does not have it, since rendering
/// costs a browser's time on every check.
pub fn first(url: &str, anchor: Option<Anchor>) -> Result<(Mode, String, bool), String> {
    let (mode, value, render) = first_read(url, anchor)?;
    // A watch of nothing would never say anything: refused here, before it is made or costs a
    // free day.
    match &mode {
        Mode::Place(_) if value.trim().is_empty() => Err("The marked place is empty on the page as Zetlyn reads it, so a change there could never be told. Mark a place with text in it, or watch the whole page.".into()),
        Mode::Page { .. } if value.lines().filter(|l| !l.trim().is_empty()).count() < 3 => {
            Err("The page shows Zetlyn almost no text, so there is nothing to watch. It may need signing in, or not let programs read it.".into())
        }
        _ => Ok((mode, value, render)),
    }
}

fn first_read(url: &str, anchor: Option<Anchor>) -> Result<(Mode, String, bool), String> {
    let served = read(url)?;
    match anchor {
        // The place was described on an earlier read of the page; this one keeps of it what held.
        Some(a) => {
            let found = |html: &str| match anchor::steadied(html, &a).map(|a| (anchor::find(html, &a), a)) {
                Some((Found::Here { value, .. }, a)) => Some((Mode::Place(a), value)),
                _ => None,
            };
            if let Some((mode, value)) = found(&served) {
                return Ok((mode, value, false));
            }
            match render::available().then(|| rendered(url)).transpose()?.and_then(|html| found(&html)) {
                Some((mode, value)) => Ok((mode, value, true)),
                None => Err("The marked place could not be found on the page again. Please mark it once more.".into()),
            }
        }
        None => {
            let shown = if render::available() { rendered(url).ok() } else { None };
            let (render, html) = match shown {
                Some(html) if says_more(&text::lines(&html), &text::lines(&served)) => (true, html),
                _ => (false, served),
            };
            let once = text::lines(&html);
            std::thread::sleep(std::time::Duration::from_secs(2));
            let twice = text::lines(&read_as(url, render)?);
            let volatile = text::volatile(&once, &twice);
            let value = text::kept(twice, &volatile).join("\n");
            Ok((Mode::Page { volatile }, value, render))
        }
    }
}

/// A watch made without the frame: the place a CSS selector names, or the whole page without one.
pub fn add(store: &Store, url: &str, select: Option<&str>, every_min: i64) -> Result<(i64, String), String> {
    let anchor = match select {
        Some(css) => Some(anchor::at(&seen(url)?.0, css)?),
        None => None,
    };
    let (mode, value, render) = first(url, anchor)?;
    let id = store.add_watch(store.local_account()?, url, &mode, every_min, None, &value)?;
    store.set_render(id, render)?;
    Ok((id, value))
}

/// One check of a watch against a page read (or not) this round.
pub fn assess(w: &Watch, page: &Result<String, String>) -> (String, bool, String, Option<(String, String)>) {
    let html = match page {
        Ok(h) => h,
        Err(e) => return ("failed".into(), false, e.clone(), None),
    };
    match value(&w.mode, html) {
        Ok((now, moved)) => {
            let before = w.last_value.clone().unwrap_or_default();
            if now == before {
                ("same".into(), moved, now, None)
            } else {
                ("changed".into(), moved, now.clone(), Some((before, now)))
            }
        }
        Err(why) => (why.into(), false, String::new(), None),
    }
}

/// One watch checked: how it came out, and the change when there was one.
pub struct Checked {
    pub watch: Watch,
    pub outcome: String,
    pub moved: bool,
    pub change: Option<(String, String)>,
}

impl Checked {
    pub fn line(&self) -> String {
        format!("{:>5} {:<8}{} {}", self.watch.id, self.outcome, if self.moved { " moved" } else { "" }, self.watch.url)
    }
}

/// Checks the running watches that are due (every running one, with `all`).
pub fn round(store: &Store, all: bool) -> Result<Vec<Checked>, String> {
    let at = now();
    let watches = if all { store.watches()?.into_iter().filter(|w| w.state == "running" && w.until.map_or(true, |u| u > at)).collect() } else { store.due(at)? };
    round_of(store, watches)
}

/// Checks these watches now, whatever their state; a page several of them watch is read once.
pub fn round_of(store: &Store, watches: Vec<Watch>) -> Result<Vec<Checked>, String> {
    let at = now();
    let mut pages: HashMap<String, Result<String, String>> = HashMap::new();
    let mut again: HashMap<String, Result<String, String>> = HashMap::new();
    let mut done = Vec::new();
    for w in watches {
        // The same page read as served and read rendered are two reads.
        let key = format!("{} {}", w.render, w.url);
        let page = pages.entry(key.clone()).or_insert_with(|| read_as(&w.url, w.render));
        let (mut outcome, moved, mut detail, mut change) = assess(&w, page);
        // A whole page that changed is read once more before the change is believed: an advert
        // that rotates, a teaser that swaps, is then seen moving and learned as moving.
        if outcome == "changed" && matches!(w.mode, Mode::Page { .. }) {
            let second = again.entry(key.clone()).or_insert_with(|| {
                std::thread::sleep(std::time::Duration::from_secs(3));
                read_as(&w.url, w.render)
            });
            if let (Ok(first), Ok(second)) = (&*page, &*second) {
                let c = confirm(&w, first, second);
                if !c.learned.is_empty() {
                    store.add_volatile(w.id, &c.learned)?;
                    store.set_last_value(w.id, &c.before)?;
                }
                (outcome, detail, change) = (c.outcome, c.now.clone(), c.change);
            }
        }
        // A place that changed is read once more too: a box that says one thing on one read and
        // another on the next (an A/B test, a shop guessing where the reader is) is seen flipping,
        // and the values it flips between are learned, so that moving between them is no change.
        if outcome == "changed" && matches!(w.mode, Mode::Place(_)) {
            let (before, now) = change.clone().unwrap_or_default();
            if flips(&w.variants, &before, &now) {
                (outcome, detail, change) = ("same".into(), before, None);
            } else {
                let second = again.entry(key.clone()).or_insert_with(|| {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    read_as(&w.url, w.render)
                });
                let then = second.as_ref().ok().and_then(|h| value(&w.mode, h).ok()).map(|(v, _)| v);
                if let Some(then) = then.filter(|t| *t != now) {
                    let seen = if then == before { [before.clone(), now] } else { [now, then] };
                    store.add_variants(w.id, &variants_of(&seen))?;
                    (outcome, detail, change) = ("same".into(), before, None);
                }
            }
        }
        store.record(w.id, at, &outcome, moved, &detail, change.as_ref().map(|(b, a)| (b.as_str(), a.as_str())))?;
        done.push(Checked { watch: w, outcome, moved, change });
    }
    Ok(done)
}

/// Whether a place moving from `before` to `now` is it flipping between values it is known to
/// take. Values that differ only in their numbers (a price, a count) are never variants of each
/// other: that is the change a person watches for.
pub fn flips(variants: &[String], before: &str, now: &str) -> bool {
    let (b, n) = (text::shape(before), text::shape(now));
    b != n && variants.contains(&b) && variants.contains(&n)
}

/// What to remember of values a place was seen flipping between: their shapes, unless they
/// differ only in their numbers.
pub fn variants_of(seen: &[String; 2]) -> Vec<String> {
    let (a, b) = (text::shape(&seen[0]), text::shape(&seen[1]));
    if a == b { Vec::new() } else { vec![a, b] }
}

/// A whole page's change, weighed against a second read.
pub struct Confirmed {
    pub outcome: String,
    /// The page now, without what moves on its own (as learned with this read).
    pub now: String,
    /// The value before, likewise.
    pub before: String,
    pub change: Option<(String, String)>,
    /// Shapes of the lines that differed between the two reads moments apart.
    pub learned: Vec<String>,
}

pub fn confirm(w: &Watch, first: &str, second: &str) -> Confirmed {
    let known = match &w.mode {
        Mode::Page { volatile } => volatile.clone(),
        Mode::Place(_) => Vec::new(),
    };
    let (one, two) = (text::lines(first), text::lines(second));
    let mut learned = text::volatile(&one, &two);
    for s in text::volatile(&two, &one) {
        if !learned.contains(&s) {
            learned.push(s);
        }
    }
    learned.retain(|s| !known.contains(s));
    let all: Vec<String> = known.iter().chain(learned.iter()).cloned().collect();
    let now = text::kept(two, &all).join("\n");
    let before = text::kept(w.last_value.as_deref().unwrap_or("").lines().map(str::to_string).collect(), &all).join("\n");
    if now == before {
        Confirmed { outcome: "same".into(), now, before, change: None, learned }
    } else {
        let change = Some((before.clone(), now.clone()));
        Confirmed { outcome: "changed".into(), now, before, change, learned }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wall_is_said_for_what_it_is() {
        let amazon = r#"<html><head><title>Amazon.de</title></head><body><p>To discuss automated access to Amazon data please contact api-services-support@amazon.com.</p><form action="/errors/validateCaptcha"></form></body></html>"#;
        assert!(walled(amazon));
        assert!(walled("<html><head><title>Just a moment...</title></head><body></body></html>"), "Cloudflare's check");
        assert!(!walled("<html><body><h1>Prices</h1><p>Pro 499 €</p><form class=contact><div class=g-recaptcha></div></form></body></html>"), "a page with a captcha in its contact form is a page");
        let said = said("https://www.amazon.de/dp/B0", "https://www.amazon.de/dp/B0: HTTP 503");
        assert!(said.starts_with("amazon.de refused to answer Zetlyn (HTTP 503)"), "{said}");
        assert_eq!(super::said("https://a.example/", "a.example: that name does not resolve"), "a.example: that name does not resolve");
    }

    fn watch(mode: Mode, last: &str) -> Watch {
        Watch { id: 1, account: 1, url: "https://example.com/".into(), mode, interval_min: 60, created: 0, until: None, last_checked: None, last_value: Some(last.into()), status: "new".into(), state: "running".into(), reminded: false, told: String::new(), render: false, key: "k".into(), variants: Vec::new(), condition: None }
    }

    #[test]
    fn the_whole_page_changes_only_by_what_does_not_move_on_its_own() {
        let w = watch(Mode::Page { volatile: vec!["Updated #".into()] }, "Prices\nPro 99 €");
        let same = Ok("<body><p>Prices</p><p>Updated 17:45</p><p>Pro 99 €</p></body>".to_string());
        assert_eq!(assess(&w, &same).0, "same");
        let changed = Ok("<body><p>Prices</p><p>Updated 17:50</p><p>Pro 89 €</p></body>".to_string());
        let (outcome, _, _, change) = assess(&w, &changed);
        assert_eq!(outcome, "changed");
        assert_eq!(change, Some(("Prices\nPro 99 €".to_string(), "Prices\nPro 89 €".to_string())));
    }

    #[test]
    fn a_rotating_advert_is_learned_and_a_real_change_still_counts() {
        let w = watch(Mode::Page { volatile: vec![] }, "Prices\nPro 99 €\nDeal: GPU");
        // The advert swapped between this read and the last; read again, it swaps back.
        let first = "<body><p>Prices</p><p>Pro 99 €</p><p>Deal: SSD</p></body>";
        let second = "<body><p>Prices</p><p>Pro 99 €</p><p>Deal: GPU</p></body>";
        let c = confirm(&w, first, second);
        assert_eq!(c.outcome, "same");
        assert_eq!(c.learned.len(), 2, "both adverts learned as moving: {:?}", c.learned);
        // A real change is in both reads and is reported.
        let first = "<body><p>Prices</p><p>Pro 89 €</p><p>Deal: SSD</p></body>";
        let second = "<body><p>Prices</p><p>Pro 89 €</p><p>Deal: GPU</p></body>";
        let c = confirm(&w, first, second);
        assert_eq!(c.outcome, "changed");
        assert_eq!(c.change, Some(("Prices\nPro 99 €".to_string(), "Prices\nPro 89 €".to_string())));
    }

    #[test]
    fn a_page_that_could_not_be_read_is_a_failed_check_not_a_change() {
        let w = watch(Mode::Page { volatile: vec![] }, "x");
        assert_eq!(assess(&w, &Err("timeout".into())).0, "failed");
    }

    #[test]
    fn a_place_flipping_between_known_texts_is_no_change_but_a_new_number_is() {
        let away = "Currently unavailable. We don't know when or if this item will be back in stock.".to_string();
        let here = "Deliver to KASSEL 34117 - Update location".to_string();
        let learned = variants_of(&[away.clone(), here.clone()]);
        assert_eq!(learned.len(), 2);
        assert!(flips(&learned, &away, &here));
        assert!(flips(&learned, &away, "Deliver to KASSEL 34119 - Update location"), "the same text with other numbers is the same variant");
        assert!(!flips(&learned, &away, "In stock. Add to basket"), "a text never seen is a change");
        assert!(variants_of(&["499,00 €".into(), "449,00 €".into()]).is_empty(), "two prices are never variants of each other");
        assert!(!flips(&[text::shape("499,00 €")], "499,00 €", "449,00 €"));
    }
}
