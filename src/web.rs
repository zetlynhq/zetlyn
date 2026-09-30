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

thread_local! {
    static ON_PAGE: std::cell::RefCell<Option<Box<dyn Fn(String)>>> = const { std::cell::RefCell::new(None) };
}

/// Where a long read says how far it is; without it, every tenth page goes to stderr.
pub fn on_page(say: Option<Box<dyn Fn(String)>>) {
    ON_PAGE.with(|h| *h.borrow_mut() = say);
}

fn tell(n: usize, taken: usize, reached: Option<&str>) {
    let line = format!("page {n}: {taken} items{}", reached.map(|d| format!(", back to {d}")).unwrap_or_default());
    ON_PAGE.with(|h| match &*h.borrow() {
        Some(say) => say(line),
        None if n % 10 == 0 => eprintln!("{line}"),
        None => {}
    });
}

pub fn rows(spec: &Spec, mark: Option<String>, root: &Path, on_row: &mut impl FnMut(Produced) -> Result<(), String>) -> Result<Option<String>, String> {
    // Back to the newest date the last update read, or the first time to `since_default`.
    let cutoff = spec.since.map(|_| mark.clone().unwrap_or_else(|| spec.since_default.to_string()));
    let date_of = |row: &J| -> Option<String> {
        let path = spec.since?.trim_start_matches("field:");
        crate::expr::walk(row, path).first().and_then(|v| crate::build::as_date(&crate::expr::as_string(v)))
    };
    let mut high: Option<String> = mark.clone();
    let f = crate::fetch::Fetcher::new(spec.user_agent, &BTreeMap::new(), spec.pause_ms)?;
    let size = spec.page.map(|p| p.max).filter(|m| *m > 0).unwrap_or(50);
    let mut taken = 0usize;
    let mut seen_before: BTreeSet<String> = BTreeSet::new();
    for n in 0.. {
        let url = page_url(spec.url, spec.page, n, size);
        let body = f.get(&url)?;
        let found = extract(&body, spec.items, spec.fields)?;
        // The same items again is a site that answers its last page for every number after it.
        let keys: BTreeSet<String> = found.iter().map(|r| r.to_string()).collect();
        if found.is_empty() || (!seen_before.is_empty() && keys.is_subset(&seen_before)) {
            break;
        }
        let found_dates: Vec<String> = found.iter().filter_map(&date_of).collect();
        for (i, value) in found.into_iter().enumerate() {
            if let (Some(c), Some(d)) = (&cutoff, date_of(&value)) {
                // Sorted newest first, so the first item older than the cutoff ends the list.
                if d.as_str() < &c[..10.min(c.len())] {
                    return Ok(high);
                }
                if high.as_deref().map_or(true, |h| d.as_str() > h) {
                    high = Some(d);
                }
            }
            if spec.top > 0 && taken >= spec.top {
                return Ok(if spec.since.is_some() { high } else { None });
            }
            taken += 1;
            on_row(Produced {
                expanded: false,
                row: Row { value, meta: Default::default(), file: None, text: String::new(), root },
                origin: Origin { url: Some(format!("{url}#{}", i + 1)), ..Origin::default() },
            })?;
        }
        let reached = found_dates.last().cloned();
        tell(n + 1, taken, reached.as_deref());
        seen_before.extend(keys);
        if spec.page.map_or(true, |p| p.offset.is_empty()) {
            break;
        }
    }
    Ok(if spec.since.is_some() { high } else { None })
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
