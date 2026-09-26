//! What reaches over the network: a JSON API, and RSS or Atom.
//!
//! A credential is never in `dataset.toml`. `${NAME}` is resolved from the deployment's
//! environment when the run starts, and a run whose variable is unset refuses to start rather than
//! falling back to an unauthenticated fetch.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Map, Value as J};

use crate::expr::{FileInfo, Row};
use crate::record::Origin;
use crate::source::Produced;

/// `${NAME}` from the environment, and nothing else from it.
///
/// `${NAME?}` is the one that may be missing: a credential that lifts a rate limit rather than
/// granting access. Unset, it takes the whole header with it, and the run goes on slowly instead
/// of refusing to start.
pub fn resolve(raw: &str) -> Result<Option<String>, String> {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let Some(end) = rest[at..].find('}') else {
            out.push_str(&rest[at..]);
            return Ok(Some(out));
        };
        let named = &rest[at + 2..at + end];
        let (name, optional) = match named.strip_suffix('?') {
            Some(n) => (n, true),
            None => (named, false),
        };
        match std::env::var(name) {
            Ok(value) => out.push_str(&value),
            Err(_) if optional => return Ok(None),
            Err(_) => {
                return Err(format!(
                    "{name} is not set. A run does not fall back to an unauthenticated fetch, \
                     because the quiet version of that failure is a complete-looking run holding \
                     a fraction of the source. Write ${{{name}?}} where it only lifts a limit"
                ));
            }
        }
        rest = &rest[at + end + 1..];
    }
    out.push_str(rest);
    Ok(Some(out))
}
const BODY_LIMIT: u64 = 128 * 1024 * 1024;

pub struct Fetcher {
    agent: ureq::Agent,
    headers: BTreeMap<String, String>,
    /// What a declaration asked for, and what the source is currently tolerating. A
    /// throttle slows the whole run down rather than making one request fail: retrying the
    /// same call at the same pace gives up at 455 subjects out of 2,684 and calls the run
    /// partial, when the source was only asking to be approached more slowly.
    declared: Duration,
    pause: std::cell::Cell<Duration>,
    easy: std::cell::Cell<u32>,
}
impl Fetcher {
    pub fn new(
        user_agent: &str,
        headers: &BTreeMap<String, String>,
        pause_ms: u64,
    ) -> Result<Fetcher, String> {
        let agent = ureq::Agent::config_builder()
            .user_agent(user_agent)
            // A publisher that has not answered in thirty seconds is throttling, and the next
            // page is a better use of the time than this one.
            .timeout_global(Some(Duration::from_secs(30)))
            .max_redirects(5)
            .build()
            .new_agent();
        let mut resolved = BTreeMap::new();
        for (k, v) in headers {
            // A header whose optional credential is unset is left out entirely.
            if let Some(value) = resolve(v)? {
                resolved.insert(k.clone(), value);
            }
        }
        Ok(Fetcher {
            agent,
            headers: resolved,
            declared: Duration::from_millis(pause_ms),
            pause: std::cell::Cell::new(Duration::from_millis(pause_ms)),
            easy: std::cell::Cell::new(0),
        })
    }

    /// A 429 or a 503 is a source asking to be left alone. The run waits and retries, doubling the
    /// wait to a ceiling, and a run that exhausts its retries is partial rather than finished.
    pub fn get(&self, url: &str) -> Result<String, String> {
        self.fetch(url).map(|(body, _)| body)
    }

    /// The body, and where the source says the next page is. Many APIs page by a cursor in a
    /// `Link` header rather than by a number, and asking for page 61 of one of those returns page
    /// 1 sixty-one times.
    ///
    /// A 429 or a 503 slows the whole run down and then retries. Retrying at the old pace is what
    /// gives up at 455 subjects out of 2,684 and calls a perfectly good source broken.
    pub fn fetch(&self, url: &str) -> Result<(String, Option<String>), String> {
        for attempt in 0..6 {
            let pause = self.pause.get();
            if attempt > 0 || !pause.is_zero() {
                std::thread::sleep(pause.max(Duration::from_millis(200)));
            }
            let mut request = self.agent.get(url);
            for (k, v) in &self.headers {
                request = request.header(k.as_str(), v.as_str());
            }
            let throttled = match request.call() {
                Ok(mut response) => {
                    let status = response.status().as_u16();
                    if status == 429 || status == 503 {
                        true
                    } else {
                        self.easier();
                        let next = response
                            .headers()
                            .get("link")
                            .and_then(|v| v.to_str().ok())
                            .and_then(next_link);
                        // A page of 2,000 CVEs is 30 MB. Bounded, because a source that answers
                        // with a gigabyte is a source that has gone wrong.
                        return response
                            .body_mut()
                            .with_config()
                            .limit(BODY_LIMIT)
                            .read_to_string()
                            .map(|body| (body, next))
                            .map_err(|e| e.to_string());
                    }
                }
                Err(ureq::Error::StatusCode(code)) if code == 429 || code == 503 => true,
                Err(e) => return Err(format!("{url}: {e}")),
            };
            if throttled {
                self.harder();
            }
        }
        Err(format!("{url}: throttled, and the retries ran out"))
    }

    /// Twice as slow, to a ceiling. The pace stays there for the rest of the run unless the source
    /// shows it can take more.
    fn harder(&self) {
        let now = self.pause.get().max(Duration::from_millis(100));
        self.pause.set((now * 2).min(Duration::from_secs(20)));
        self.easy.set(0);
    }

    /// And back towards what the declaration asked for, a step at a time, after the source has
    /// answered fifty times without complaining.
    fn easier(&self) {
        let seen = self.easy.get() + 1;
        self.easy.set(seen);
        if seen < 50 {
            return;
        }
        self.easy.set(0);
        let now = self.pause.get();
        if now > self.declared {
            self.pause.set((now / 2).max(self.declared));
        }
    }
    /// For one subject out of many. A source that will not answer for a gated model has not
    /// failed; it has declined one of thousands, and a run that stopped there would report a
    /// broken source because somebody made one repository private.
    pub fn get_subject(&self, url: &str) -> Result<Option<String>, String> {
        match self.get(url) {
            Ok(body) => Ok(Some(body)),
            Err(e) if e.contains("http status: 4") => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// `<https://…>; rel="next", <https://…>; rel="last"`
fn next_link(header: &str) -> Option<String> {
    for part in header.split(',') {
        if !part.contains("rel=\"next\"") {
            continue;
        }
        let start = part.find('<')? + 1;
        let end = part.find('>')?;
        return Some(part[start..end].to_string());
    }
    None
}

/// Days, hours or minutes. What a declaration writes for `window`.
pub fn duration(spec: &str) -> Option<i64> {
    let s = spec.trim();
    let (number, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit())?);
    let n: i64 = number.parse().ok()?;
    Some(match unit {
        "d" => n * 86_400,
        "h" => n * 3_600,
        "m" => n * 60,
        "s" => n,
        _ => return None,
    })
}

fn day_seconds(date: &str) -> i64 {
    // A date this program wrote, so the ten characters in front are the whole of it.
    let p: Vec<&str> = date.get(..10).unwrap_or("1970-01-01").split('-').collect();
    let (y, m, d) = (
        p.first()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(1970),
        p.get(1).and_then(|s| s.parse::<i64>().ok()).unwrap_or(1),
        p.get(2).and_then(|s| s.parse::<i64>().ok()).unwrap_or(1),
    );
    let (y, m) = if m <= 2 { (y - 1, m + 12) } else { (y, m) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m - 3) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) * 86_400
}

fn stamp(secs: i64) -> String {
    format!("{}.000", crate::iso_stamp(secs).trim_end_matches('Z'))
}

/// A stamp this program wrote, back to seconds.
pub fn seconds_of(stamp: &str) -> i64 {
    let days = day_seconds(stamp);
    let time = stamp
        .split_once('T')
        .map(|(_, t)| {
            let p: Vec<i64> = t
                .trim_end_matches('Z')
                .split(':')
                .filter_map(|s| s.split('.').next().and_then(|n| n.parse().ok()))
                .collect();
            p.first().copied().unwrap_or(0) * 3600
                + p.get(1).copied().unwrap_or(0) * 60
                + p.get(2).copied().unwrap_or(0)
        })
        .unwrap_or(0);
    days + time
}

pub struct Http<'a> {
    pub list: &'a str,
    pub detail: Option<&'a str>,
    pub page: Option<&'a crate::decl::Page>,
    pub window: Option<&'a str>,
    pub since: Option<&'a str>,
    pub since_default: &'a str,
    pub limit: usize,
    /// When `detail` is set, what splits the list response into items.
    pub each: Option<&'a str>,
}

/// A paged JSON API, over as many windows as the source will accept at once.
pub fn http_rows(
    f: &Fetcher,
    spec: &Http,
    mark: Option<String>,
    root: &std::path::Path,
    on_row: &mut dyn FnMut(Produced) -> Result<(), String>,
) -> Result<Option<String>, String> {
    let now = crate::now();
    let start = mark.unwrap_or_else(|| spec.since_default.to_string());
    let start_secs = day_seconds(&start);
    let step = spec
        .window
        .and_then(duration)
        .unwrap_or(now - start_secs + 1)
        .max(1);

    let mut high = start.clone();
    let mut taken = 0usize;
    let mut from = start_secs;
    while from <= now {
        let until = (from + step).min(now);
        let (offset_key, size_key, total_key, max, by) = match spec.page {
            Some(p) => (
                Some(p.offset.clone()),
                Some(p.size.clone()),
                p.total.clone(),
                p.max,
                p.by.clone(),
            ),
            None => (None, None, None, 0, String::new()),
        };
        let filtering = spec.list.contains("{since}");
        let mut next_url: Option<String> = None;
        let by_cursor = spec.page.and_then(|p| p.cursor.as_deref()) == Some("link");
        let mut offset = 0usize;
        loop {
            let mut url = match &next_url {
                Some(u) => u.clone(),
                None => spec
                    .list
                    .replace("{since}", &stamp(from))
                    .replace("{until}", &stamp(until))
                    .replace("{now}", &stamp(now))
                    // Sources differ in what they take. NVD wants a timestamp and GitHub wants a
                    // date, so a declaration names which of the two it needs.
                    .replace("{since_date}", &stamp(from)[..10])
                    .replace("{until_date}", &stamp(until)[..10])
                    .replace("{now_date}", &stamp(now)[..10]),
            };
            if next_url.is_none() {
                if let (Some(o), Some(s)) = (&offset_key, &size_key) {
                    if !o.is_empty() && !s.is_empty() {
                        let joiner = if url.contains('?') { '&' } else { '?' };
                        let at = if by == "page" {
                            offset / max.max(1) + 1
                        } else {
                            offset
                        };
                        url.push_str(&format!("{joiner}{o}={at}&{s}={max}"));
                    }
                }
            }
            let (body, next) = f.fetch(&url)?;
            let value: J =
                serde_json::from_str(&body).map_err(|e| format!("{url}: not JSON: {e}"))?;

            let total = total_key
                .as_deref()
                .map(|t| t.trim_start_matches("field:"))
                .and_then(|t| value.pointer(&format!("/{}", t.replace('.', "/"))))
                .and_then(J::as_u64)
                .unwrap_or(0);

            // Without `detail` the response is the row and `each` splits it later. With it, the
            // source splits the list itself and fetches one answer per item.
            let probe = |v: &J| Row {
                value: v.clone(),
                meta: BTreeMap::new(),
                file: None,
                text: String::new(),
                root,
            };
            // The items a page holds, whoever ends up splitting it. A page is one row without
            // `detail`, so counting rows would stop the paging after the first one.
            let split: Vec<J> = match spec.each {
                Some(each) => crate::expr::eval(each, &probe(&value)),
                None => vec![value.clone()],
            };
            let produced = split.len();

            let mut older = 0usize;
            if let Some(field) = spec.since {
                for item in &split {
                    let Some(seen) = crate::expr::eval(field, &probe(item))
                        .first()
                        .map(crate::expr::as_string)
                    else {
                        continue;
                    };
                    if seen > high {
                        high = seen.clone();
                    }
                    // A source that pages newest first and cannot be asked for a date is read
                    // until it reaches one. `since` names the field either way: a filter where the
                    // URL takes it, a stop where it does not.
                    if !filtering && seen.as_str() < start.as_str() {
                        older += 1;
                    }
                }
            }

            let rows: Vec<J> = if spec.detail.is_some() {
                split
            } else {
                vec![value.clone()]
            };
            for item in rows {
                let (row_value, at, expanded) = match spec.detail {
                    Some(template) => {
                        let one = crate::expr::fill(template, &probe(&item));
                        let body = f.get(&one)?;
                        let mut v: J = serde_json::from_str(&body)
                            .map_err(|e| format!("{one}: not JSON: {e}"))?;
                        // The list item stays reachable, because it carries what the detail
                        // answer leaves out.
                        if let Some(o) = v.as_object_mut() {
                            o.insert("_list".into(), item.clone());
                        }
                        (v, one, true)
                    }
                    None => (item, url.clone(), false),
                };
                let mut meta = BTreeMap::new();
                meta.insert("url".into(), at.clone());
                let origin = Origin {
                    url: Some(at),
                    ..Origin::default()
                };
                on_row(Produced {
                    expanded,
                    row: Row {
                        value: row_value,
                        meta,
                        file: None,
                        text: String::new(),
                        root,
                    },
                    origin,
                })?;
            }
            taken += produced;

            // Every item on the page is behind the window, so there is nothing further back
            // worth asking for.
            if !filtering && produced > 0 && older == produced {
                break;
            }
            if spec.limit > 0 && taken >= spec.limit {
                return Ok(Some(high));
            }
            offset += max;
            // A source that names its next page is followed until it stops naming one. A source
            // that says how many there are is paged until they run out, and one that does neither
            // is paged until a page comes back short.
            if by_cursor {
                match next {
                    Some(n) => {
                        next_url = Some(n);
                        continue;
                    }
                    None => break,
                }
            }
            let exhausted = match total_key {
                Some(_) => total == 0 || offset >= total as usize,
                None => produced < max,
            };
            if max == 0 || exhausted {
                break;
            }
        }
        if until == now {
            break;
        }
        from = until;
    }
    // Only a complete pass advances the mark, and this returns it for the caller to store.
    Ok(Some(if high > start {
        high
    } else {
        stamp(now)[..10].to_string()
    }))
}

/// RSS and Atom. One item is one record, and `text_is` is the licence decision.
pub fn feed_rows(
    f: &Fetcher,
    urls: &[String],
    text_is: &str,
    root: &std::path::Path,
    on_row: &mut dyn FnMut(Produced) -> Result<(), String>,
) -> Result<(), String> {
    for url in urls {
        let body = f.get(url)?;
        for item in parse_feed(&body) {
            let text = match text_is {
                // At `whole` the dataset fetches and stores the article, which is a separate act
                // from linking to it and is only done where the source permits it.
                "whole" => match item.get("link") {
                    Some(link) => f
                        .get(link)
                        .map(|html| crate::source::html_to_text(&html))
                        .unwrap_or_else(|_| item.get("summary").cloned().unwrap_or_default()),
                    None => item.get("summary").cloned().unwrap_or_default(),
                },
                _ => item.get("summary").cloned().unwrap_or_default(),
            };
            let mut meta = item.clone();
            meta.insert("feed".into(), url.clone());
            let origin = Origin {
                url: item.get("link").cloned().or_else(|| Some(url.clone())),
                ..Origin::default()
            };
            let mut o = Map::new();
            for (k, v) in &meta {
                o.insert(k.clone(), J::String(v.clone()));
            }
            on_row(Produced {
                expanded: false,
                row: Row {
                    value: J::Object(o),
                    meta,
                    file: None,
                    text,
                    root,
                },
                origin,
            })?;
        }
    }
    Ok(())
}

/// Both shapes, because a feed reader that handled one of them would cover half the web.
fn parse_feed(xml: &str) -> Vec<BTreeMap<String, String>> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.trim_text(true);
    let mut out = Vec::new();
    let mut current: Option<BTreeMap<String, String>> = None;
    let mut path: Vec<String> = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local(e.name().as_ref());
                if matches!(name.as_str(), "item" | "entry") {
                    current = Some(BTreeMap::new());
                }
                if let Some(item) = current.as_mut() {
                    // Atom puts the address in an attribute rather than in the text.
                    if name == "link" {
                        for a in e.attributes().flatten() {
                            if local(a.key.as_ref()) == "href" {
                                let v = String::from_utf8_lossy(&a.value).into_owned();
                                item.entry("link".into()).or_insert(v);
                            }
                        }
                    }
                }
                path.push(name);
            }
            Ok(Event::Empty(e)) => {
                let name = local(e.name().as_ref());
                if name == "link" {
                    if let Some(item) = current.as_mut() {
                        for a in e.attributes().flatten() {
                            if local(a.key.as_ref()) == "href" {
                                let v = String::from_utf8_lossy(&a.value).into_owned();
                                item.entry("link".into()).or_insert(v);
                            }
                        }
                    }
                }
            }
            Ok(Event::Text(t)) => {
                take(&mut current, &path, &String::from_utf8_lossy(t.as_ref()));
            }
            Ok(Event::CData(t)) => {
                take(&mut current, &path, &String::from_utf8_lossy(t.as_ref()));
            }
            Ok(Event::End(e)) => {
                let name = local(e.name().as_ref());
                path.pop();
                if matches!(name.as_str(), "item" | "entry") {
                    if let Some(mut item) = current.take() {
                        if let Some(s) = item.get("summary").cloned() {
                            item.insert("summary".into(), crate::source::html_to_text(&s));
                        }
                        if let Some(p) = item.get("published").cloned() {
                            if let Some(d) = feed_date(&p) {
                                item.insert("published".into(), d);
                            }
                        }
                        out.push(item);
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

fn local(name: &[u8]) -> String {
    let s = String::from_utf8_lossy(name);
    s.rsplit(':').next().unwrap_or(&s).to_string()
}

/// One text node, under whatever tag it sat in.
fn take(current: &mut Option<BTreeMap<String, String>>, path: &[String], text: &str) {
    let Some(item) = current.as_mut() else { return };
    let Some(tag) = path.last() else { return };
    if text.trim().is_empty() {
        return;
    }
    let key = match tag.as_str() {
        "title" => "title",
        "link" => "link",
        "id" | "guid" => "guid",
        "description" | "summary" | "content" | "encoded" => "summary",
        "pubDate" | "published" | "updated" | "date" => "published",
        "author" | "creator" | "name" => "author",
        "category" => "category",
        _ => return,
    };
    item.entry(key.to_string())
        .and_modify(|v| {
            // A feed that carries both a summary and the full content keeps the longer one.
            if key == "summary" && v.len() < text.len() {
                *v = text.to_string();
            }
        })
        .or_insert_with(|| text.to_string());
}

/// RFC 822 as RSS writes it, and ISO 8601 as Atom does.
fn feed_date(raw: &str) -> Option<String> {
    if let Some(d) = crate::build::as_date(raw) {
        return Some(d);
    }
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let parts: Vec<&str> = raw.split_whitespace().collect();
    let at = parts
        .iter()
        .position(|p| MONTHS.contains(&p.to_ascii_lowercase().as_str()))?;
    let month = MONTHS
        .iter()
        .position(|m| *m == parts[at].to_ascii_lowercase())
        .map(|i| i + 1)?;
    let day: u32 = parts
        .get(at.checked_sub(1)?)?
        .trim_matches(',')
        .parse()
        .ok()?;
    let year: i64 = parts.get(at + 1)?.parse().ok()?;
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

#[allow(dead_code)]
fn unused(_: FileInfo) {}
