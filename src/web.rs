//! A source that is a web page.
//!
//! Most of what people want to track is on pages made for reading, and a page is data once it is
//! said what one item is and what of it is a value. `items` is a CSS selector, and each element
//! it finds becomes one row: `fields` names what goes in it, each a selector for the text of the
//! first element it finds inside the item (`.title`), that selector and an attribute
//! (`.price@data-final`), or an attribute of the item itself (`@href`). From there the row is read
//! by the declaration's `claims`, the same as an API's answer, so identifiers, types, dates and
//! receipts work as they do everywhere else.
//!
//! A page that pages is followed by the declaration's `page` (a number or an offset in the
//! address), and stops at the first page with no item, at a page that says only what the one
//! before it said, or at `top`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use scraper::{ElementRef, Html, Selector};
use serde_json::{Map, Value as J};

use crate::claim::Origin;
use crate::expr::Row;
use crate::rows::Produced;

pub struct Spec<'a> {
    pub url: &'a str,
    pub items: &'a str,
    pub fields: &'a BTreeMap<String, String>,
    pub page: Option<&'a crate::sourcedecl::Page>,
    pub top: usize,
    /// A trial: this many items and no more, and nothing left to resume.
    pub limit: usize,
    pub since: Option<&'a str>,
    pub since_default: &'a str,
    pub user_agent: &'a str,
    pub pause_ms: u64,
}

fn selector(s: &str) -> Result<Selector, String> {
    Selector::parse(s).map_err(|e| format!("{s}: not a CSS selector ({e})"))
}

/// Text as a person reads it: whitespace collapsed, ends trimmed.
fn text_of(e: ElementRef) -> String {
    e.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One field of one item: `sel`, `sel@attr`, `@attr`, or `sel[]` for the text of every match.
fn field(item: ElementRef, spec: &str) -> Result<J, String> {
    let spec = spec.trim();
    let (sel, attr) = match spec.rsplit_once('@') {
        Some((s, a)) => (s.trim(), Some(a.trim())),
        None => (spec, None),
    };
    let (sel, all) = match sel.strip_suffix("[]") {
        Some(s) => (s.trim(), true),
        None => (sel, false),
    };
    let found: Vec<ElementRef> = if sel.is_empty() { vec![item] } else { item.select(&selector(sel)?).collect() };
    let value = |e: ElementRef| -> Option<String> {
        match attr {
            Some(a) => e.value().attr(a).map(|v| v.trim().to_string()),
            None => Some(text_of(e)),
        }
        .filter(|v| !v.is_empty())
    };
    Ok(if all {
        J::Array(found.into_iter().filter_map(value).map(J::String).collect())
    } else {
        found.into_iter().find_map(value).map(J::String).unwrap_or(J::Null)
    })
}

/// Every item a page holds, as the rows its fields make.
pub fn extract(html: &str, items: &str, fields: &BTreeMap<String, String>) -> Result<Vec<J>, String> {
    let doc = Html::parse_document(html);
    let sel = selector(items)?;
    let mut out = Vec::new();
    for item in doc.select(&sel) {
        let mut row = Map::new();
        for (name, spec) in fields {
            row.insert(name.clone(), field(item, spec)?);
        }
        out.push(J::Object(row));
    }
    Ok(out)
}

/// The address of one page, by the declaration's paging: a number from one, or an offset.
fn page_url(base: &str, page: Option<&crate::sourcedecl::Page>, n: usize, size: usize) -> String {
    let Some(p) = page.filter(|p| !p.offset.is_empty()) else { return base.to_string() };
    let at = if p.by == "page" { n + 1 } else { n * size };
    let mut params: Vec<String> = Vec::new();
    let (head, query) = base.split_once('?').unwrap_or((base, ""));
    for kv in query.split('&').filter(|kv| !kv.is_empty()) {
        let k = kv.split('=').next().unwrap_or("");
        if k != p.offset && (p.size.is_empty() || k != p.size) {
            params.push(kv.to_string());
        }
    }
    params.push(format!("{}={at}", p.offset));
    if !p.size.is_empty() {
        params.push(format!("{}={size}", p.size));
    }
    format!("{head}?{}", params.join("&"))
}

/// How far a read is, told after every page.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Step {
    pub page: usize,
    pub items: usize,
    /// The date of the last item on the page, where the list is sorted by one.
    pub reached: Option<String>,
}

type Hook = Box<dyn Fn(&Step) -> bool>;

thread_local! {
    static ON_PAGE: std::cell::RefCell<Option<Hook>> = const { std::cell::RefCell::new(None) };
}

/// Who hears how far a read is, and may stop it by answering false. Without one, every tenth
/// page goes to stderr.
pub fn on_page(hook: Option<Hook>) {
    ON_PAGE.with(|h| *h.borrow_mut() = hook);
}

fn tell(step: &Step) -> bool {
    ON_PAGE.with(|h| match &*h.borrow() {
        Some(hook) => hook(step),
        None => {
            if step.page % 10 == 0 {
                eprintln!("page {}: {} items{}", step.page, step.items, step.reached.as_ref().map(|d| format!(", back to {d}")).unwrap_or_default());
            }
            true
        }
    })
}

/// Where a read that did not finish stopped: the next update starts there instead of at the
/// first page, so an hour of pages is not read twice because one of them timed out.
pub const RESUME: &str = "resume.json";

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Resume {
    /// The page to read next, counted from 0.
    pub page: usize,
    pub items: usize,
    /// The newest date read before it stopped: the mark once the rest is read.
    pub high: Option<String>,
    pub why: String,
    /// A read further back than the mark: the date it goes to, instead of stopping at what was
    /// read before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    /// Stopped by a person: it waits for them to say go on, and an update meanwhile reads only
    /// what is new.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paused: bool,
}

pub fn resume_of(dir: &Path) -> Option<Resume> {
    std::fs::read_to_string(dir.join(RESUME)).ok().and_then(|t| serde_json::from_str(&t).ok())
}

pub fn rows(spec: &Spec, mark: Option<String>, dir: &Path, root: &Path, on_row: &mut impl FnMut(Produced) -> Result<(), String>) -> Result<Option<String>, String> {
    let date_of = |row: &J| -> Option<String> {
        let path = spec.since?.trim_start_matches("field:");
        crate::expr::walk(row, path).first().and_then(|v| crate::build::as_date(&crate::expr::as_string(v)))
    };
    let f = crate::fetch::Fetcher::new(spec.user_agent, &BTreeMap::new(), spec.pause_ms)?;
    let size = spec.page.map(|p| p.max).filter(|m| *m > 0).unwrap_or(50);
    // A trial reads nothing of what an earlier read left to do; that is for the real one.
    let waiting = resume_of(dir).filter(|r| r.paused);
    let resumed = if spec.limit == 0 { resume_of(dir).filter(|r| !r.paused) } else { None };
    // Meanwhile a stopped read is not started again from the top: it reads down to what it read.
    let mark = mark.or_else(|| waiting.as_ref().and_then(|r| r.high.clone()));
    let keep = spec.limit > 0 || waiting.is_some();
    let first = resumed.as_ref().map(|r| r.page).unwrap_or(0);
    let until = resumed.as_ref().and_then(|r| r.until.clone());
    // Back to the newest date the last update read, or the first time to `since_default`, or
    // further back than the mark when that is what was asked for.
    let cutoff = spec.since.map(|_| until.clone().or_else(|| mark.clone()).unwrap_or_else(|| spec.since_default.to_string()));
    let mut high: Option<String> = [mark.clone(), resumed.as_ref().and_then(|r| r.high.clone())].into_iter().flatten().max();
    let mut taken = resumed.as_ref().map(|r| r.items).unwrap_or(0);
    let cap = if spec.limit > 0 { spec.limit } else { spec.top };
    let mut seen_before: BTreeSet<String> = BTreeSet::new();
    let done = |high: Option<String>| -> Result<Option<String>, String> {
        if !keep {
            let _ = std::fs::remove_file(dir.join(RESUME));
        }
        Ok(if spec.since.is_some() { high } else { None })
    };
    for n in first.. {
        let url = page_url(spec.url, spec.page, n, size);
        let stopped = |why: String, high: &Option<String>, taken: usize| -> String {
            if !keep {
                let r = Resume { page: n, items: taken, high: high.clone(), why: why.clone(), until: until.clone(), paused: false };
                let _ = std::fs::write(dir.join(RESUME), serde_json::to_string(&r).unwrap_or_default());
            }
            why
        };
        let body = f.get(&url).map_err(|e| stopped(e, &high, taken))?;
        let found = extract(&body, spec.items, spec.fields)?;
        // The same items again is a site that answers its last page for every number after it.
        let keys: BTreeSet<String> = found.iter().map(|r| r.to_string()).collect();
        if found.is_empty() || (!seen_before.is_empty() && keys.is_subset(&seen_before)) {
            break;
        }
        let reached = found.iter().filter_map(&date_of).last();
        for (i, value) in found.into_iter().enumerate() {
            if let (Some(c), Some(d)) = (&cutoff, date_of(&value)) {
                // Sorted newest first, so the first item older than the cutoff ends the list.
                if d.as_str() < &c[..10.min(c.len())] {
                    return done(high);
                }
                if high.as_deref().map_or(true, |h| d.as_str() > h) {
                    high = Some(d);
                }
            }
            if cap > 0 && taken >= cap {
                return done(high);
            }
            taken += 1;
            on_row(Produced {
                expanded: false,
                row: Row { value, meta: Default::default(), file: None, text: String::new(), root },
                origin: Origin { url: Some(format!("{url}#{}", i + 1)), ..Origin::default() },
            })?;
        }
        seen_before.extend(keys);
        if !tell(&Step { page: n + 1, items: taken, reached }) {
            // The next page is where it goes on.
            let why = format!("stopped by you at page {}", n + 1);
            let r = Resume { page: n + 1, items: taken, high: high.clone(), why: why.clone(), until: until.clone(), paused: true };
            if !keep {
                let _ = std::fs::write(dir.join(RESUME), serde_json::to_string(&r).unwrap_or_default());
            }
            return Err(why);
        }
        // Enough is enough before the next page is asked for, not after.
        if cap > 0 && taken >= cap {
            return done(high);
        }
        if spec.page.map_or(true, |p| p.offset.is_empty()) {
            break;
        }
    }
    done(high)
}

/// How long a list is, found by asking for a few of its pages rather than reading them all: its
/// size per page, the page where it reaches `cutoff` (a date, for a list sorted by one) and its
/// last page. Pages 2, 4, 8, … until one is past, then halved down to the page.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Measure {
    pub per_page: usize,
    /// The first page with an item older than the cutoff.
    pub to_cutoff: Option<usize>,
    pub cutoff: Option<String>,
    /// The last page with items; `None` where it was more than the pages asked for.
    pub last: Option<usize>,
    /// Newest and oldest dates the list has, where it has them.
    pub newest: Option<String>,
    pub oldest: Option<String>,
    /// Seconds a page takes, pause included.
    pub seconds_a_page: f64,
}

/// The reading of a declared web source, for what is done with it outside an update.
pub fn with_spec<T>(fetch: &crate::sourcedecl::Fetch, go: impl FnOnce(&Spec) -> T) -> Option<T> {
    let crate::sourcedecl::Fetch::Web { url, items, fields, page, top, limit, since, since_default, user_agent, pause_ms } = fetch else {
        return None;
    };
    Some(go(&Spec { url, items, fields, page: page.as_ref(), top: *top, limit: *limit, since: since.as_deref(), since_default, user_agent, pause_ms: *pause_ms }))
}

pub const MEASURE: &str = "measure.json";

pub fn measure_of(dir: &Path) -> Option<Measure> {
    std::fs::read_to_string(dir.join(MEASURE)).ok().and_then(|t| serde_json::from_str(&t).ok())
}

impl Measure {
    /// `about 868 pages, 21,700 items, 36 minutes`.
    pub fn about(&self, pages: usize) -> String {
        format!("about {} pages, {} items, {}", thousands(pages), thousands(pages * self.per_page.max(1)), duration(pages as f64 * self.seconds_a_page))
    }
}

pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn duration(seconds: f64) -> String {
    let s = seconds.round() as u64;
    match s {
        0..=59 => format!("{s} seconds"),
        60..=89 => "a minute".to_string(),
        90..=5399 => format!("{} minutes", (s + 30) / 60),
        _ if s % 3600 < 180 || s % 3600 > 3420 => format!("{} hours", (s + 1800) / 3600),
        _ => format!("{:.1} hours", s as f64 / 3600.0),
    }
}

pub fn measure(spec: &Spec, cutoff: Option<&str>, say: &dyn Fn(String)) -> Result<Measure, String> {
    let date_of = |row: &J| -> Option<String> {
        let path = spec.since?.trim_start_matches("field:");
        crate::expr::walk(row, path).first().and_then(|v| crate::build::as_date(&crate::expr::as_string(v)))
    };
    let f = crate::fetch::Fetcher::new(spec.user_agent, &BTreeMap::new(), spec.pause_ms)?;
    let size = spec.page.map(|p| p.max).filter(|m| *m > 0).unwrap_or(50);
    let mut asked = 0usize;
    let mut oldest_seen: Option<String> = None;
    let started = std::time::Instant::now();
    // A page's items and its dates, oldest last; the page numbers here count from 1.
    let mut look = |n: usize| -> Result<(usize, Option<String>, Option<String>), String> {
        asked += 1;
        let rows = extract(&f.get(&page_url(spec.url, spec.page, n - 1, size))?, spec.items, spec.fields)?;
        let dates: Vec<String> = rows.iter().filter_map(&date_of).collect();
        let (newest, oldest) = (dates.iter().max().cloned(), dates.iter().min().cloned());
        if let Some(o) = &oldest {
            if oldest_seen.as_ref().map_or(true, |s| o < s) {
                oldest_seen = Some(o.clone());
            }
        }
        say(format!("page {n}: {} items{}", rows.len(), oldest.as_ref().map(|d| format!(", back to {d}")).unwrap_or_default()));
        Ok((rows.len(), newest, oldest))
    };
    let (per_page, newest, first_oldest) = look(1)?;
    let mut m = Measure { per_page, newest, oldest: first_oldest.clone(), cutoff: cutoff.map(str::to_string), ..Measure::default() };
    if per_page == 0 || spec.page.map_or(true, |p| p.offset.is_empty()) {
        m.last = Some(1);
        m.seconds_a_page = started.elapsed().as_secs_f64();
        return Ok(m);
    }
    let before = |d: &Option<String>| match (cutoff, d) {
        (Some(c), Some(d)) => d.as_str() < &c[..10.min(c.len())],
        _ => false,
    };
    if before(&first_oldest) {
        m.to_cutoff = Some(1);
    }
    // Doubling until a page is empty, or past the cutoff when that is what is asked; a list of
    // more than 32,768 pages is said to be that long.
    const FAR: usize = 32_768;
    let (mut good, mut n) = (1usize, 2usize);
    let mut past_cutoff: Option<(usize, usize)> = m.to_cutoff.map(|_| (0, 1));
    let mut empty: Option<usize> = None;
    while n <= FAR {
        let (count, _, oldest) = look(n)?;
        if count == 0 {
            empty = Some(n);
            break;
        }

        if past_cutoff.is_none() && before(&oldest) {
            past_cutoff = Some((good, n));
        }
        good = n;
        n *= 2;
    }
    // Halving down to where the cutoff is.
    if let Some((mut lo, mut hi)) = past_cutoff.filter(|(lo, hi)| hi > lo && *lo > 0) {
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            let (count, _, oldest) = look(mid)?;
            if count == 0 || before(&oldest) { hi = mid } else { lo = mid }
        }
        m.to_cutoff = Some(hi);
    }
    // And to where the list ends.
    if let Some(mut hi) = empty {
        let mut lo = good;
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if look(mid)?.0 == 0 { hi = mid } else { lo = mid }
        }
        m.last = Some(lo);
    }
    m.seconds_a_page = started.elapsed().as_secs_f64() / asked.max(1) as f64;
    m.oldest = oldest_seen.or(m.oldest);
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_become_rows_by_their_fields() {
        let html = r#"<html><body>
          <a class="row" href="/app/1" data-id="1"><span class="title"> One  game </span><div class="when">30 Sep, 2026</div><b class="p" data-v="0">Free</b></a>
          <a class="row" href="/app/2" data-id="2"><span class="title">Two</span><i class="tag">rpg</i><i class="tag">indie</i></a>
        </body></html>"#;
        let fields: BTreeMap<String, String> = [
            ("id", "@data-id"), ("title", ".title"), ("released", ".when"), ("price", ".p@data-v"), ("tags", ".tag[]"), ("url", "@href"),
        ].into_iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        let rows = extract(html, "a.row", &fields).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["title"], "One game");
        assert_eq!(rows[0]["released"], "30 Sep, 2026");
        assert_eq!(rows[0]["price"], "0");
        assert_eq!(rows[1]["tags"], serde_json::json!(["rpg", "indie"]));
        assert_eq!(rows[1]["released"], J::Null);
        assert!(extract(html, "a..row", &fields).unwrap_err().contains("not a CSS selector"));
    }

    #[test]
    fn a_page_is_asked_for_by_number_or_offset() {
        let p = crate::sourcedecl::Page { cursor: None, offset: "page".into(), size: String::new(), total: None, by: "page".into(), max: 50 };
        assert_eq!(page_url("https://x/s?tags=1&page=4", Some(&p), 1, 50), "https://x/s?tags=1&page=2");
        let o = crate::sourcedecl::Page { offset: "start".into(), size: "count".into(), by: "offset".into(), ..p };
        assert_eq!(page_url("https://x/s", Some(&o), 2, 25), "https://x/s?start=50&count=25");
        assert!(crate::build::as_date("30 Sep, 2026").is_some_and(|d| d.starts_with("2026-09-30")));
    }
}

/// What might be the items of a page, found without being told: elements that repeat, with a
/// class, and hold something. The best first.
pub struct Candidate {
    pub items: String,
    pub count: usize,
    /// Name, the field's spec, and what the first items say in it.
    pub fields: Vec<(String, String, Vec<String>)>,
}

pub fn candidates(html: &str) -> Vec<Candidate> {
    let doc = Html::parse_document(html);
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    for e in doc.root_element().descendants().filter_map(ElementRef::wrap) {
        // The class the element is written with first, which is its own; the ones after it are
        // usually states and modifiers shared with other things.
        if let Some(class) = e.value().attr("class").and_then(|c| c.split_whitespace().next()) {
            if !matches!(e.value().name(), "html" | "body" | "head" | "script" | "style") {
                *groups.entry(format!("{}.{}", e.value().name(), class)).or_default() += 1;
            }
        }
    }
    let mut out: Vec<(usize, Candidate)> = Vec::new();
    for (sel, count) in groups.into_iter().filter(|(_, n)| (5..=2000).contains(n)) {
        let Ok(s) = Selector::parse(&sel) else { continue };
        let items: Vec<ElementRef> = doc.select(&s).collect();
        let sample: Vec<ElementRef> = items.iter().take(20).copied().collect();
        // A field is what most items have: a classed element with text inside, or an attribute
        // of the item itself that says something (an address, a data- value).
        let mut specs: BTreeMap<String, String> = BTreeMap::new();
        let mut tally: BTreeMap<String, usize> = BTreeMap::new();
        for item in &sample {
            for (a, v) in item.value().attrs() {
                if (a == "href" || a.starts_with("data-")) && !v.trim().is_empty() && v.len() < 200 {
                    *tally.entry(format!("@{a}")).or_default() += 1;
                }
            }
            let mut classes: BTreeSet<String> = BTreeSet::new();
            for d in item.descendants().skip(1).filter_map(ElementRef::wrap) {
                if let Some(c) = d.value().attr("class").and_then(|c| c.split_whitespace().next()) {
                    let t = text_of(d);
                    if !t.is_empty() && t.chars().count() <= 300 && classes.insert(c.to_string()) {
                        *tally.entry(format!(".{c}")).or_default() += 1;
                    }
                }
            }
        }
        let enough = (sample.len() + 1) / 2;
        for (spec, _) in tally.into_iter().filter(|(_, n)| *n >= enough.max(2)) {
            let name = crate::guess::slug(spec.trim_start_matches(['.', '@']));
            if !name.is_empty() && !specs.contains_key(&name) {
                specs.insert(name, spec);
            }
        }
        // What says nothing (the same on every item) and what says what another field says (the
        // same values, in another element) are left out; of two that agree, the shorter name stays.
        let mut specs: Vec<(String, String)> = specs.into_iter().collect();
        specs.sort_by_key(|(n, _)| (n.len(), n.clone()));
        let mut kept: Vec<(String, String)> = Vec::new();
        let mut said_before: BTreeSet<Vec<String>> = BTreeSet::new();
        for (name, spec) in specs {
            let v: Vec<String> = sample.iter().map(|i| field(*i, &spec).ok().map(|j| j.to_string()).unwrap_or_default()).collect();
            let distinct: BTreeSet<&String> = v.iter().collect();
            // A date the same on every item sampled is a list sorted by it, and still a date.
            let dated = v.iter().any(|x| crate::build::as_date(x.trim_matches('"')).is_some());
            if distinct.len() <= 1 && sample.len() > 1 && !dated {
                continue;
            }
            if said_before.insert(v) {
                kept.push((name, spec));
            }
        }
        // An element whose text holds two other fields'"'"' in every item is their container.
        let texts: Vec<Vec<String>> = kept.iter().map(|(_, s)| sample.iter().map(|i| field(*i, s).ok().and_then(|j| j.as_str().map(str::to_string)).unwrap_or_default()).collect()).collect();
        let container = |k: usize| -> bool {
            (0..kept.len()).filter(|j| *j != k).filter(|j| (0..sample.len()).all(|n| !texts[*j][n].is_empty() && texts[k][n].contains(texts[*j][n].as_str()) && texts[k][n] != texts[*j][n])).count() >= 2
        };
        let specs: BTreeMap<String, String> = kept.iter().enumerate().filter(|(k, _)| !container(*k)).map(|(_, (n, s))| (n.clone(), s.clone())).collect();
        let fields: Vec<(String, String, Vec<String>)> = specs
            .into_iter()
            .map(|(name, spec)| {
                let said: Vec<String> = sample.iter().take(3).filter_map(|i| field(*i, &spec).ok()).filter_map(|v| v.as_str().map(str::to_string)).collect();
                (name, spec, said)
            })
            .collect();
        let texts = fields.iter().filter(|(_, s, _)| s.starts_with('.')).count();
        if fields.len() < 2 || texts == 0 {
            continue;
        }
        // A list is a few dozen items that differ: what repeats six hundred times alike is a menu.
        let values = |spec: &str| -> Vec<String> { sample.iter().filter_map(|i| field(*i, spec).ok()).filter_map(|v| v.as_str().map(str::to_string)).collect() };
        let mut varying = 0usize;
        let mut bonus = 0usize;
        for (_, spec, _) in &fields {
            let v = values(spec);
            let distinct: BTreeSet<&String> = v.iter().collect();
            if distinct.len() * 2 >= sample.len().max(2) {
                varying += 1;
            }
            if !v.is_empty() && distinct.len() == v.len() && v.len() == sample.len() && v.iter().all(|x| x.len() <= 64) {
                bonus = bonus.max(50);
            }
            if v.iter().filter(|x| crate::build::as_date(x).is_some()).count() * 2 >= sample.len() {
                bonus += 50;
            }
        }
        out.push((count.min(60) * varying + bonus, Candidate { items: sel, count, fields }));
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.into_iter().map(|(_, c)| c).take(5).collect()
}

/// Where the page links to its own second page: the parameter that numbers its pages.
pub fn paging_parameter(html: &str, url: &str) -> Option<String> {
    let path = url.split('?').next().unwrap_or(url).trim_end_matches('/');
    let path = path.rsplit('/').next().unwrap_or("");
    let re = regex::Regex::new(r#"href="([^"]*[?&](page|p|pg|seite)=2(&[^"]*)?)""#).ok()?;
    let found = re.captures_iter(html)
        .find(|c| path.is_empty() || c[1].contains(path))
        .map(|c| c[2].to_string());
    found
}

#[cfg(test)]
mod found {
    #[test]
    fn a_list_on_a_page_is_found_with_its_fields_and_its_paging() {
        let mut html = String::from(r#"<html><body><div class="nav"><a class="link">Home</a></div>"#);
        for i in 1..=12 {
            html.push_str(&format!(r#"<a class="search_result_row" href="/app/{i}/" data-ds-appid="{i}"><span class="title">Game {i}</span><div class="search_released">{i} Sep, 2026</div><span class="platform_img win"></span></a>"#));
        }
        html.push_str(r#"<a href="/search/?tags=492&page=2">next</a></body></html>"#);
        let found = super::candidates(&html);
        let best = &found[0];
        assert_eq!(best.items, "a.search_result_row");
        assert_eq!(best.count, 12);
        let names: Vec<&str> = best.fields.iter().map(|(n, ..)| n.as_str()).collect();
        assert!(names.contains(&"title") && names.contains(&"search_released") && names.contains(&"data_ds_appid"), "{names:?}");
        assert_eq!(super::paging_parameter(&html, "https://store.steampowered.com/search/?tags=492").as_deref(), Some("page"));
    }
}
