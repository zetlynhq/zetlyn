//! What the assist is asked, and how each answer is held against the source before anybody sees
//! it.
//!
//! Every task has the same shape. Code first finds what patterns can find: the identifiers the
//! library knows, the values that are dates, the numbers that look like paging. Those go to the
//! model as facts, with an outline of what the source actually said. The model answers in one
//! JSON shape, code turns that into a declaration, and the declaration is tried against the
//! source. Where the try complains, the complaint goes back once, which is what mending is.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{json, Value as J};

use crate::assist::{Assist, Disclosure};

/// How many items of a list the model sees, and how long a value may be.
const ITEMS: usize = 20;
const VALUE: usize = 80;
const PATHS: usize = 220;

/// A JSON API taught, and what trying it produced.
pub struct Taught {
    pub declaration: String,
    pub trial: String,
    pub mended: bool,
}

/// What a caller must show before asking: the task needs the person's yes (D10).
pub enum Outcome<T> {
    NeedsConsent(Disclosure),
    Done(T),
}

/// The disclosure, where this source may be sent; none where the person has not said yes yet.
fn consent(assist: &Assist, dir: &Path, sends: Vec<String>, yes: bool) -> Result<Option<Disclosure>, String> {
    if !assist.available() {
        return Err(Assist::missing());
    }
    if yes {
        crate::assist::allow(dir)?;
    }
    Ok(crate::assist::allowed(dir).then(|| Disclosure { to: assist.who(), sends }))
}

// -- teaching a JSON API ---------------------------------------------------------------------

/// Read one answer of a JSON API, ask what it is, write a declaration, and try it.
pub fn api(assist: &Assist, url: &str, dir: &Path, name: &str, yes: bool) -> Result<Outcome<Taught>, String> {
    let f = crate::fetch::Fetcher::new(crate::sourcedecl::AGENT, &BTreeMap::new(), 0)?;
    let (body, next) = f.fetch(url)?;
    let answer: J = serde_json::from_str(&body).map_err(|e| format!("{url} does not answer JSON: {e}"))?;
    let first_body = body.clone();
    let mut facts = Facts::of(&answer, next.as_deref(), None);
    let outline = facts.outline();
    let sends = vec![
        format!("the address {url}"),
        format!("an outline of its answer: {} paths, each with one example value of at most {VALUE} characters, from up to {ITEMS} items", facts.paths.len().min(PATHS)),
    ];
    let Some(disclosure) = consent(assist, dir, sends.clone(), yes)? else {
        return Ok(Outcome::NeedsConsent(Disclosure { to: assist.who(), sends }));
    };

    let user = format!("The address: {url}\n\n{outline}");
    let mut answer = assist.ask("api", &format!("{url}#first"), API_SYSTEM, &user, &api_schema())?;
    crate::assist::sent(dir, "api", &disclosure)?;
    // The first answer says how the API pages, which is enough to read its last page. An API
    // ordered by age hands out its oldest items first, and what it says of items now (NVD's CVSS
    // 3.1) may not be on the first page at all. Where the last page has paths the first did
    // not, the question is asked again with both.
    let mut user = user;
    if let Some(last) = last_page(url, &answer, &facts) {
        if let Ok((body, _)) = f.fetch(&last) {
            if let Ok(later) = serde_json::from_str::<J>(&body) {
                let richer = Facts::of(&serde_json::from_str::<J>(&first_body).unwrap_or(J::Null), next.as_deref(), Some(&later));
                if richer.paths.len() > facts.paths.len() {
                    facts = richer;
                    user = format!("The address: {url}\n\nThe outline is of its first page and its last, {last}.\n\n{}", facts.outline());
                    answer = assist.ask("api", &format!("{url}#last"), API_SYSTEM, &user, &api_schema())?;
                    crate::assist::sent(dir, "api", &disclosure)?;
                }
            }
        }
    }
    let mut decl = declaration_from(&answer, url, name, &facts)?;
    let mut trial = try_out(&decl, dir)?;
    let mut mended = false;
    if let Some(said) = complaint(&trial) {
        // Once. A second answer that is still wrong is for a person to read, not a loop to run.
        let again = format!(
            "{user}\n\nYour last answer made this declaration:\n\n{}\n\nTried against the source, it said: {said}\nAnswer again, mending what is wrong.",
            crate::yaml::to_string(&decl)?
        );
        if let Ok(answer) = assist.ask("api-mend", &format!("{url}#mend"), API_SYSTEM, &again, &api_schema()) {
            crate::assist::sent(dir, "api-mend", &disclosure)?;
            if let Ok(d) = declaration_from(&answer, url, name, &facts) {
                if let Ok(t) = try_out(&d, dir) {
                    if complaint(&t).is_none() || t.claims > trial.claims {
                        decl = d;
                        trial = t;
                        mended = true;
                    }
                }
            }
        }
    }
    let text = crate::yaml::to_string(&decl)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::write(dir.join(crate::sourcedecl::FILE), &text).map_err(|e| e.to_string())?;
    Ok(Outcome::Done(Taught { declaration: text, trial: trial.said(), mended }))
}

/// What code can tell about an answer without being told.
struct Facts {
    /// The candidate lists, largest first, as `each` would name them.
    lists: Vec<(String, usize)>,
    /// Inside one item of the largest list: every path and one value.
    paths: Vec<(String, String)>,
    /// Paths whose every value is one known scheme.
    ids: Vec<(String, &'static str, f64, bool)>,
    dates: Vec<String>,
    top: Vec<(String, String)>,
    next: Option<String>,
}

impl Facts {
    fn of(answer: &J, next: Option<&str>, later: Option<&J>) -> Facts {
        let mut lists: Vec<(String, usize)> = Vec::new();
        match answer {
            J::Array(a) => lists.push(("*".into(), a.len())),
            J::Object(o) => {
                // A file keyed by name: every value an object of the same shape.
                let keyed = o.len() >= 5 && o.values().all(J::is_object);
                if keyed {
                    lists.push(("*".into(), o.len()));
                }
                // Its values are the items, and a list inside one of them is that item's own.
                if !keyed {
                    find_lists(answer, "", 0, &mut lists);
                }
            }
            _ => {}
        }
        lists.sort_by(|a, b| b.1.cmp(&a.1));
        let items: Vec<J> = lists
            .first()
            .map(|(each, _)| {
                let mut all = crate::expr::walk(answer, each);
                // A later page of the same list, where one was read: what the source says now.
                if let Some(l) = later {
                    all.extend(crate::expr::walk(l, each));
                }
                all
            })
            .unwrap_or_default();
        let mut paths: Vec<(String, String)> = Vec::new();
        let mut seen = BTreeSet::new();
        // Spread over the list rather than its head: the first items of an API ordered by age are
        // the oldest, and a property that began later (CVSS 3.1 in NVD) is not in them at all.
        let step = (items.len() / ITEMS).max(1);
        for item in items.iter().step_by(step).take(ITEMS) {
            outline(item, "", &mut paths, &mut seen);
        }
        // Across up to fifty items: which leaves are one scheme, which are dates.
        let sample: Vec<&J> = items.iter().take(50).collect();
        let mut ids = Vec::new();
        let mut dates = Vec::new();
        for (path, _) in &paths {
            let values: Vec<Vec<String>> = sample
                .iter()
                .map(|i| crate::expr::walk(i, path).iter().filter_map(scalar).collect())
                .collect();
            let filled: Vec<&Vec<String>> = values.iter().filter(|v| !v.is_empty()).collect();
            if filled.is_empty() {
                continue;
            }
            for s in crate::schemes::ALL.iter().filter(|s| !s.classifies()) {
                let hit = filled.iter().filter(|v| v.iter().any(|x| s.is(x))).count();
                let share = hit as f64 / sample.len().max(1) as f64;
                if hit >= 2 && share >= if s.checked() { 0.5 } else { 0.1 } {
                    let several = filled.iter().any(|v| v.iter().filter(|x| s.is(x)).count() > 1)
                        || filled.iter().any(|v| v.iter().any(|x| !s.is(x)));
                    ids.push((path.clone(), s.name, share, several));
                }
            }
            if filled.iter().all(|v| v.iter().all(|x| x.len() >= 8 && crate::build::as_date(x).is_some())) {
                dates.push(path.clone());
            }
        }
        let top = match answer {
            J::Object(o) => o
                .iter()
                .filter(|(_, v)| !v.is_array() && !v.is_object())
                .map(|(k, v)| (k.clone(), short(v)))
                .collect(),
            _ => Vec::new(),
        };
        Facts { lists, paths, ids, dates, top, next: next.map(str::to_string) }
    }

    fn outline(&self) -> String {
        let mut out = String::new();
        out.push_str("## Lists in the answer (candidates for `each`, most items first)\n");
        for (each, n) in &self.lists {
            out.push_str(&format!("{each}: {n} items\n"));
        }
        if !self.top.is_empty() {
            out.push_str("\n## Top-level values beside the list\n");
            for (k, v) in &self.top {
                out.push_str(&format!("{k}: {v}\n"));
            }
        }
        match &self.next {
            Some(n) => out.push_str(&format!("\n## The answer's Link header names a next page\n{n}\n")),
            None => out.push_str("\n## No Link header naming a next page\n"),
        }
        out.push_str("\n## Paths inside one item of the first list, with an example\n");
        for (p, v) in self.paths.iter().take(PATHS) {
            out.push_str(&format!("{p}: {v}\n"));
        }
        out.push_str("\n## Found by pattern (facts, not guesses)\n");
        for (p, s, share, several) in &self.ids {
            out.push_str(&format!(
                "{p} holds {s} identifiers in {:.0}% of items{}\n",
                share * 100.0,
                if *several { ", several or mixed per item" } else { "" }
            ));
        }
        for d in &self.dates {
            out.push_str(&format!("{d} holds dates\n"));
        }
        out
    }
}

fn find_lists(v: &J, prefix: &str, depth: usize, out: &mut Vec<(String, usize)>) {
    if depth > 3 {
        return;
    }
    if let J::Object(o) = v {
        for (k, child) in o {
            let path = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
            match child {
                J::Array(a) if a.iter().any(J::is_object) => out.push((format!("{path}[]"), a.len())),
                J::Object(_) => find_lists(child, &path, depth + 1, out),
                _ => {}
            }
        }
    }
}

fn outline(v: &J, prefix: &str, out: &mut Vec<(String, String)>, seen: &mut BTreeSet<String>) {
    if out.len() >= PATHS * 2 {
        return;
    }
    match v {
        J::Object(o) => {
            // An object keyed by names (a package, a release) says the same thing under every
            // name. Three of them say it; the rest would push the useful paths out of the outline.
            let alike = o.len() > 12 && o.values().all(|v| v.is_array() || v.is_object());
            for (i, (k, child)) in o.iter().enumerate() {
                if alike && i == 3 {
                    let path = if prefix.is_empty() { "…".to_string() } else { format!("{prefix}.…") };
                    out.push((path, format!("({} more keys like the three before)", o.len() - 3)));
                    break;
                }
                let path = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                outline(child, &path, out, seen);
            }
        }
        J::Array(a) => {
            let path = format!("{prefix}[]");
            if a.is_empty() {
                if seen.insert(path.clone()) {
                    out.push((path.clone(), "(empty list)".into()));
                }
            }
            for child in a.iter().take(2) {
                outline(child, &path, out, seen);
            }
        }
        leaf => {
            if seen.insert(prefix.to_string()) {
                out.push((prefix.to_string(), short(leaf)));
            }
        }
    }
}

fn scalar(v: &J) -> Option<String> {
    match v {
        J::String(s) => Some(s.clone()),
        J::Number(n) => Some(n.to_string()),
        J::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn short(v: &J) -> String {
    let s = scalar(v).unwrap_or_else(|| v.to_string());
    let s = s.replace('\n', " ");
    if s.chars().count() > VALUE {
        format!("{}…", s.chars().take(VALUE).collect::<String>())
    } else {
        s
    }
}

const API_SYSTEM: &str = "You write source declarations for Zetlyn, a program that reads a source into claims and \
joins several sources on shared identifiers. You are shown an outline of one answer of a JSON API. \
\n\nPaths are relative to one item of the list and are dot-separated keys. `[]` after a key walks every \
element of an array, for example `metrics.cvssMetricV31[].cvssData.baseScore`. The list is named by \
`each`: `*` is every element of an array answer, or every value of an object keyed by name; otherwise a \
path ending in `[]`, like `vulnerabilities[]`. Use only paths that appear in the outline. \
\n\nChoose: the identifier (an identifier found by pattern is a fact: use its scheme name; otherwise the \
path that names one item, with a short lowercase scheme named after the source), the title (the short \
human name of one item, not a long description), the text (the paths a person would read), the date the \
item became known (published or created, not modified), and the properties worth comparing across \
sources: severities, scores, statuses, versions, counts, flags. Not every path is a property; ten good \
ones are better than forty. Name properties in lowercase snake_case, say what they mean \
(`cvss` for a CVSS base score, `severity` for a severity word). \
\n\nPaging: `offset` where a parameter counts items (with the parameter for the page size), `page` where \
it counts pages from one, `link` where the Link header names the next page, `none` where the answer is \
everything. Use the top-level values beside the list to decide, and the address's own parameters. \
\n\nIncremental: if the API can be asked for what changed since a time, the query parameters that do it \
with {since} and {until} (full timestamps) or {since_date} and {until_date} (dates), and the path in an \
item that says when it changed or appeared. Leave both empty if you cannot tell from the outline. \
\n\nNever invent a path, a parameter or a value.";

fn api_schema() -> J {
    let path = json!({ "type": "string" });
    json!({
        "type": "object",
        "required": ["kind", "title", "about", "each", "id", "title_from", "text", "known", "properties", "paging", "incremental"],
        "properties": {
            "kind": { "type": "string", "description": "one lowercase word for what one item is: vulnerability, advisory, model, article, package, product" },
            "title": { "type": "string", "description": "the source's own name" },
            "about": { "type": "string", "description": "one sentence: what this source says about each item" },
            "each": path,
            "id": { "type": "array", "items": { "type": "object", "required": ["scheme", "from"], "properties": {
                "scheme": { "type": "string" }, "from": path, "all": { "type": "boolean" } } } },
            "title_from": path,
            "text": { "type": "array", "items": path },
            "known": path,
            "url": { "type": "string", "description": "a path holding the item's web page, or a template like https://example.org/item/{id} with a path in braces, or empty" },
            "properties": { "type": "array", "items": { "type": "object", "required": ["name", "type", "from"], "properties": {
                "name": { "type": "string" },
                "type": { "type": "string", "enum": ["text", "code", "number", "date", "bool"] },
                "from": path } } },
            "where": { "type": "string", "description": "only where the list mixes kinds: a condition on one path that keeps the items that are this kind, like type == 'exploit'; else empty" },
            "paging": { "type": "object", "required": ["style"], "properties": {
                "style": { "type": "string", "enum": ["none", "offset", "page", "link"] },
                "offset_param": { "type": "string" },
                "size_param": { "type": "string" },
                "total": { "type": "string", "description": "the top-level value that says how many there are in all, or empty" },
                "size": { "type": "integer" } } },
            "incremental": { "type": "object", "required": ["query", "field"], "properties": {
                "query": { "type": "string" }, "field": path } },
        },
    })
}

/// The answer as a declaration, in this program's grammar. Anything the answer names that the
/// outline did not hold is dropped here rather than written and found out later.
fn declaration_from(a: &J, url: &str, name: &str, facts: &Facts) -> Result<crate::sourcedecl::SourceDecl, String> {
    let known_paths: BTreeSet<&str> = facts.paths.iter().map(|(p, _)| p.as_str()).collect();
    // A path the model shortened (`cve.id` for `cve.id`) is fine; one it made up is not.
    let real = |p: &str| -> bool {
        let p = p.trim().trim_start_matches("field:");
        !p.is_empty() && (known_paths.contains(p) || known_paths.iter().any(|k| k.starts_with(&format!("{p}.")) || k.starts_with(&format!("{p}[]"))))
    };
    let field = |p: &str| format!("field:{}", p.trim().trim_start_matches("field:"));
    let str_of = |v: &J| v.as_str().unwrap_or("").trim().to_string();

    let each = str_of(&a["each"]);
    let each = if each.is_empty() { facts.lists.first().map(|l| l.0.clone()).unwrap_or_else(|| "*".into()) } else { each };

    let mut ids: Vec<J> = Vec::new();
    for i in a["id"].as_array().into_iter().flatten() {
        let from = str_of(&i["from"]);
        if !real(&from) {
            continue;
        }
        let scheme = crate::guess::slug(&str_of(&i["scheme"]));
        let mut one = json!({ "scheme": scheme, "from": field(&from) });
        // What the patterns found overrides what the model said about the same path: whether
        // there are several per item, and which values are that scheme.
        if let Some((_, s, _, several)) = facts.ids.iter().find(|(p, ..)| *p == from.trim_start_matches("field:")) {
            one["scheme"] = json!(s);
            if *several {
                one["all"] = json!(true);
                one["match"] = json!(crate::schemes::named(s).map(|k| k.declared()).unwrap_or_default());
            }
        } else if i["all"].as_bool() == Some(true) {
            one["all"] = json!(true);
        }
        ids.push(one);
    }
    // A known scheme the model left out is still an identifier another source can meet.
    for (p, s, _, several) in &facts.ids {
        if !ids.iter().any(|i| i["from"] == json!(field(p))) && ids.iter().all(|i| i["scheme"] != json!(s)) {
            let mut one = json!({ "scheme": s, "from": field(p) });
            if *several {
                one["all"] = json!(true);
                one["match"] = json!(crate::schemes::named(s).map(|k| k.declared()).unwrap_or_default());
            }
            ids.push(one);
        }
    }
    if ids.is_empty() {
        return Err("the answer names no identifier in the outline".into());
    }

    let title_from = str_of(&a["title_from"]);
    let title = if real(&title_from) { field(&title_from) } else { ids[0]["from"].as_str().unwrap_or("").to_string() };
    let text: Vec<String> = a["text"].as_array().into_iter().flatten().map(str_of).filter(|p| real(p)).map(|p| field(&p)).collect();
    let known = str_of(&a["known"]);
    let mut claims = json!({
        "each": field(&each),
        "id": if ids.len() == 1 { ids[0].clone() } else { J::Array(ids) },
        "title": title,
        "text": if text.is_empty() { vec![title.clone()] } else { text },
    });
    if real(&known) {
        claims["known"] = json!(field(&known));
    }
    // A condition names a path the outline holds and compares it with a quoted word.
    let cond = str_of(&a["where"]);
    if let Some((path, rest)) = cond.split_once(['=', '!']) {
        let path = path.trim().trim_end_matches(['!', '=']).trim();
        if real(path) && rest.contains('\'') {
            claims["where"] = json!(format!("{}{}", field(path), &cond[path.len()..]));
        }
    }
    let url_of = str_of(&a["url"]);
    if url_of.contains("{") && url_of.starts_with("http") {
        let mut t = url_of.clone();
        for (p, _) in &facts.paths {
            t = t.replace(&format!("{{{p}}}"), &format!("{{field:{p}}}"));
        }
        if !t.contains("{field:") {
            // A placeholder the outline does not hold makes an address that is always wrong.
        } else if t.matches('{').count() == t.matches("{field:").count() {
            claims["url"] = json!(format!("const:{t}"));
        }
    } else if real(&url_of) {
        claims["url"] = json!(field(&url_of));
    }
    let mut properties = serde_json::Map::new();
    for p in a["properties"].as_array().into_iter().flatten() {
        let from = str_of(&p["from"]);
        let name = crate::guess::slug(&str_of(&p["name"]));
        let kind = str_of(&p["type"]);
        if !real(&from) || name.is_empty() || !matches!(kind.as_str(), "text" | "code" | "number" | "date" | "bool") {
            continue;
        }
        if matches!(name.as_str(), "id" | "title" | "text" | "known" | "url" | "kind" | "source") {
            continue;
        }
        properties.insert(name, json!({ "type": kind, "from": field(&from) }));
    }
    claims["properties"] = J::Object(properties.clone());

    // Paging and the incremental query, only as far as the answer and the address support them.
    let mut list = url.to_string();
    let mut fetch = json!({ "type": "http" });
    let pg = &a["paging"];
    let param = |k: &str| str_of(&pg[k]);
    let page = match str_of(&pg["style"]).as_str() {
        "link" if facts.next.is_some() => Some(json!({ "cursor": "link", "size": param("size_param") })),
        "offset" if !param("offset_param").is_empty() => Some(json!({ "offset": param("offset_param"), "size": param("size_param") })),
        "page" if !param("offset_param").is_empty() => Some(json!({ "offset": param("offset_param"), "size": param("size_param"), "by": "page" })),
        _ => None,
    };
    if let Some(mut p) = page {
        let total = param("total");
        if !total.is_empty() && facts.top.iter().any(|(k, _)| *k == total) {
            p["total"] = json!(format!("field:{total}"));
        }
        if let Some(size) = pg["size"].as_u64().filter(|n| *n > 0) {
            p["max"] = json!(size);
        }
        if p["size"].as_str() == Some("") {
            p.as_object_mut().map(|o| o.remove("size"));
        }
        // An address that already names the page it starts at would be asked for that page again.
        list = without_params(&list, &[param("offset_param"), param("size_param")]);
        fetch["page"] = p;
    }
    let inc = &a["incremental"];
    let (query, changed) = (str_of(&inc["query"]), str_of(&inc["field"]));
    if query.contains("{since") && real(&changed) {
        list = format!("{list}{}{}", if list.contains('?') { "&" } else { "?" }, query.trim_start_matches(['?', '&']));
        fetch["since"] = json!(field(&changed));
        fetch["since_default"] = json!(crate::iso_date(crate::now() - 30 * 86_400));
    }
    fetch["list"] = json!(list);
    fetch["pause_ms"] = json!(1000);

    let columns: Vec<String> = properties
        .iter()
        .filter(|(_, p)| matches!(p["type"].as_str(), Some("code" | "number" | "bool")))
        .map(|(n, _)| n.clone())
        .take(3)
        .chain(claims.get("known").map(|_| "known".to_string()))
        .collect();
    let facets: Vec<String> = properties.iter().filter(|(_, p)| p["type"] == "code").map(|(n, _)| n.clone()).take(3).collect();
    let decl = json!({
        "name": name,
        "title": str_of(&a["title"]),
        "kind": crate::guess::slug(&str_of(&a["kind"])).replace('_', "-"),
        "about": str_of(&a["about"]),
        "fetch": fetch,
        "schedule": { "every": "6h" },
        "claims": claims,
        "views": [{ "name": "recent", "title": "Newest first", "default": true, "columns": columns, "facets": facets,
                    "sort": if claims.get("known").is_some() { "known desc" } else { "title" } }],
        "search": { "text": ["title", "text"] },
    });
    serde_json::from_value(decl).map_err(|e| format!("the answer does not make a declaration: {e}"))
}

fn without_params(url: &str, names: &[String]) -> String {
    let Some((base, query)) = url.split_once('?') else { return url.to_string() };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|p| !names.iter().any(|n| !n.is_empty() && p.split('=').next() == Some(n.as_str())))
        .collect();
    if kept.is_empty() { base.to_string() } else { format!("{base}?{}", kept.join("&")) }
}

/// A declaration tried on at most 200 claims, in a directory of its own that is removed again.
struct Trial {
    claims: u64,
    error: Option<String>,
    no_known: u64,
    unparsed: u64,
    duplicates: u64,
    first: Vec<String>,
}

impl Trial {
    fn said(&self) -> String {
        let mut s = match &self.error {
            Some(e) => format!("the try failed: {e}"),
            None => format!("{} claims tried", self.claims),
        };
        if self.no_known > 0 {
            s.push_str(&format!(", {} with no date", self.no_known));
        }
        if self.unparsed > 0 {
            s.push_str(&format!(", {} values not of their type", self.unparsed));
        }
        if self.duplicates > 0 {
            s.push_str(&format!(", {} given twice under one key", self.duplicates));
        }
        for f in &self.first {
            s.push_str(&format!("\n  {f}"));
        }
        s
    }
}

fn try_out(decl: &crate::sourcedecl::SourceDecl, dir: &Path) -> Result<Trial, String> {
    let scratch = dir.join(".trial");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let mut j = serde_json::to_value(decl).map_err(|e| e.to_string())?;
    j["fetch"]["limit"] = json!(200);
    let tried: crate::sourcedecl::SourceDecl = serde_json::from_value(j).map_err(|e| e.to_string())?;
    std::fs::write(scratch.join(crate::sourcedecl::FILE), crate::yaml::to_string(&tried)?).map_err(|e| e.to_string())?;
    let out = (|| -> Result<Trial, String> {
        let ds = crate::source::Source::open(&scratch)?;
        let r = ds.run()?;
        // What an apply refuses, refused here too, once there is something to check.
        let wrong = ds.check();
        if !wrong.is_empty() && r.error.is_none() {
            return Err(wrong.join("; "));
        }
        let q = crate::source::Query { text: String::new(), pred: None, view: None, ids: Vec::new(), seen_before: None, sort: None, limit: 3, offset: 0 };
        let first = crate::source::Interface::search(&ds, &q)
            .map(|(_, hits, _)| {
                hits.iter()
                    .map(|h| format!("{} · {} · {}", h.ids.iter().map(|i| format!("{} {}", i.scheme, i.value)).collect::<Vec<_>>().join(", "), h.title.chars().take(70).collect::<String>(), h.known))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Trial {
            claims: r.added + r.changed + r.unchanged,
            error: r.error.clone(),
            no_known: r.no_known,
            unparsed: r.unparsed,
            duplicates: r.duplicates,
            first,
        })
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    out.or_else(|e| Ok(Trial { claims: 0, error: Some(e), no_known: 0, unparsed: 0, duplicates: 0, first: Vec::new() }))
}

fn complaint(t: &Trial) -> Option<String> {
    if let Some(e) = &t.error {
        return Some(e.clone());
    }
    if t.claims == 0 {
        return Some("no claims came out of it".into());
    }
    if t.no_known * 2 > t.claims {
        return Some(format!("{} of {} claims have no date", t.no_known, t.claims));
    }
    if t.duplicates * 4 > t.claims {
        return Some(format!("{} of {} claims came twice under one identifier, which is not what names one item", t.duplicates, t.claims));
    }
    None
}

// -- scoring ---------------------------------------------------------------------------------

/// A proposal against the declaration a person wrote, by the standard in REFRAME: the
/// identifier, the title and the dated property recovered, and most of the typed properties.
pub fn score(proposed: &Path, reference: &Path) -> Result<J, String> {
    let p = serde_json::to_value(crate::sourcedecl::SourceDecl::load(proposed)?).map_err(|e| e.to_string())?;
    let r = serde_json::to_value(crate::sourcedecl::SourceDecl::load(reference)?).map_err(|e| e.to_string())?;
    let c = |d: &J, k: &str| d["claims"][k].clone();
    let norm = |v: &J| v.as_str().unwrap_or("").trim_start_matches("field:").to_string();
    let ids = |d: &J| -> Vec<(String, String)> {
        match c(d, "id") {
            J::Array(a) => a.iter().map(|i| (i["scheme"].as_str().unwrap_or("").to_string(), norm(&i["from"]))).collect(),
            J::Null => Vec::new(),
            one => vec![(one["scheme"].as_str().unwrap_or("").to_string(), norm(&one["from"]))],
        }
    };
    let (pi, ri) = (ids(&p), ids(&r));
    // The reference's first identifier, found anywhere in the proposal's: by path, and by scheme.
    // Two patterns over the text for the same scheme are one rule, however each is spelled.
    let same_from = |a: &str, b: &str| a == b || (a.starts_with("text:") && b.starts_with("text:"));
    let identifier = ri.first().is_some_and(|(s, f)| {
        pi.iter().any(|(ps, pf)| same_from(pf, f) && (ps == s || crate::schemes::named(s).is_none()))
    });
    let title = norm(&c(&p, "title")) == norm(&c(&r, "title"));
    let known = c(&r, "known").is_null() || norm(&c(&p, "known")) == norm(&c(&r, "known"));
    let props = |d: &J| -> BTreeMap<String, String> {
        c(d, "properties")
            .as_object()
            .map(|o| o.values().map(|v| (norm(&v["from"]), v["type"].as_str().unwrap_or("").to_string())).collect())
            .unwrap_or_default()
    };
    let (pp, rp) = (props(&p), props(&r));
    let typed = rp.iter().filter(|(f, t)| pp.get(*f) == Some(*t)).count();
    let found = rp.keys().filter(|f| pp.contains_key(*f)).count();
    Ok(json!({
        "identifier": identifier,
        "title": title,
        "known": known,
        "properties": { "reference": rp.len(), "found": found, "typed": typed },
        "meets": identifier && title && known && (rp.is_empty() || typed * 2 > rp.len()),
    }))
}

/// The address of the list's last page, by the paging the first answer described, or none where
/// it pages by a cursor, which can only be followed from the start.
fn last_page(url: &str, a: &J, facts: &Facts) -> Option<String> {
    let pg = &a["paging"];
    let param = |k: &str| pg[k].as_str().unwrap_or("").trim().to_string();
    let total_name = param("total");
    let total: u64 = facts.top.iter().find(|(k, _)| *k == total_name)?.1.parse().ok()?;
    let size = facts.lists.first()?.1 as u64;
    if size == 0 || total <= size {
        return None;
    }
    let at = match pg["style"].as_str()? {
        "offset" => total - size,
        "page" => total.div_ceil(size),
        _ => return None,
    };
    let name = param("offset_param");
    if name.is_empty() {
        return None;
    }
    let base = without_params(url, std::slice::from_ref(&name));
    Some(format!("{base}{}{name}={at}", if base.contains('?') { "&" } else { "?" }))
}

// -- aligning two sources --------------------------------------------------------------------

pub struct Aligned {
    /// The `align:` entry for this property, as YAML.
    pub entry: String,
    pub before: u64,
    pub after: u64,
}

/// Which words of several sources mean the same thing for one property, proposed from the words
/// they actually use, and counted: how many disagreements the map takes away.
pub fn align(assist: &Assist, dir: &Path, sources: &Path, property: &str, yes: bool) -> Result<Outcome<Aligned>, String> {
    let tracker = crate::tracker::Tracker::open(dir, sources)?;
    let existing = tracker.decl.normalise.get(property);
    let q = crate::source::Query { text: String::new(), pred: None, view: None, ids: Vec::new(), seen_before: None, sort: None, limit: 0, offset: 0 };
    let mut words: Vec<(String, Vec<(String, u64)>)> = Vec::new();
    for m in &tracker.members {
        let field = existing.map(|a| a.field_in(m.name(), property)).unwrap_or_else(|| property.to_string());
        let said = m.member.facet(&q, &field, 60);
        if !said.is_empty() {
            words.push((m.name().to_string(), said));
        }
    }
    if words.len() < 2 {
        return Err(format!("{property}: fewer than two sources say it, so there is nothing to align"));
    }
    let sends = vec![format!(
        "the words {} sources use for {property}, at most 60 each, with how often: {}",
        words.len(),
        words.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>().join(", ")
    )];
    let Some(disclosure) = consent(assist, dir, sends.clone(), yes)? else {
        return Ok(Outcome::NeedsConsent(Disclosure { to: assist.who(), sends }));
    };
    let before = differ(&tracker, property);

    let mut user = format!("The property: {property}\n\n");
    for (s, said) in &words {
        user.push_str(&format!("## {s}\n"));
        for (w, n) in said {
            user.push_str(&format!("{w}: {n}\n"));
        }
        user.push('\n');
    }
    let answer = assist.ask("align", &format!("{}#{property}", tracker.decl.name), ALIGN_SYSTEM, &user, &align_schema())?;
    crate::assist::sent(dir, "align", &disclosure)?;

    // Only words a source actually uses, onto the scale the answer gives, or onto each other.
    let scale: Vec<String> = answer["scale"].as_array().into_iter().flatten().filter_map(J::as_str).map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
    let mut entry = serde_json::Map::new();
    if !scale.is_empty() {
        entry.insert("scale".into(), json!(scale));
    }
    if let Some(a) = existing {
        if !a.from.is_empty() {
            entry.insert("from".into(), json!(a.from));
        }
    }
    let tolerance = answer["tolerance"].as_str().unwrap_or("").trim().to_string();
    if tolerance.parse::<f64>().is_ok() || tolerance.strip_suffix('d').is_some_and(|d| d.parse::<u32>().is_ok()) {
        entry.insert("tolerance".into(), json!(tolerance));
    }
    for m in answer["maps"].as_array().into_iter().flatten() {
        let (s, w, means) = (m["source"].as_str().unwrap_or(""), m["word"].as_str().unwrap_or(""), m["means"].as_str().unwrap_or("").trim().to_lowercase());
        let Some((_, said)) = words.iter().find(|(name, _)| name == s) else { continue };
        if means.is_empty() || !said.iter().any(|(x, _)| x == w) || (!scale.is_empty() && !scale.contains(&means)) {
            continue;
        }
        if w.to_lowercase() == means {
            continue;
        }
        entry.entry(s.to_string()).or_insert_with(|| json!({}))[w] = json!(means);
    }
    let entry = J::Object(entry);
    let mut decl = serde_json::to_value(&tracker.decl).map_err(|e| e.to_string())?;
    decl["align"][property] = entry.clone();

    // Counted under the proposal, from a copy of the declaration nobody serves.
    let trial = dir.join(".trial");
    let _ = std::fs::remove_dir_all(&trial);
    std::fs::create_dir_all(&trial).map_err(|e| e.to_string())?;
    let proposed: crate::trackerdecl::TrackerDecl = serde_json::from_value(decl.clone()).map_err(|e| format!("the answer does not make a tracker: {e}"))?;
    std::fs::write(trial.join(crate::trackerdecl::FILE), crate::yaml::to_string(&proposed)?).map_err(|e| e.to_string())?;
    let after = crate::tracker::Tracker::open(&trial, sources).map(|t| differ(&t, property));
    let _ = std::fs::remove_dir_all(&trial);
    let mut shown = serde_json::Map::new();
    shown.insert(property.to_string(), entry);
    Ok(Outcome::Done(Aligned { entry: crate::yaml::to_string(&J::Object(shown))?, before, after: after? }))
}

fn differ(t: &crate::tracker::Tracker, property: &str) -> u64 {
    t.measure()["properties"][property]["differ_after_the_map"].as_u64().unwrap_or(0)
}

const ALIGN_SYSTEM: &str = "You align what several sources say about one property, so that Zetlyn \
compares meanings and not spellings. You are shown, per source, the words it uses and how often. \
\n\nIf the words are grades of one thing (a severity, a priority, a rank), give the scale from the most \
to the least, in the tracker's own lowercase words, and map every source word onto one step of it. \
If they are not ordered but still the same things spelled differently (a licence, a platform), give no \
scale and map each word onto one shared spelling. If they are numbers, give no maps; say a tolerance \
only if the sources plainly round differently. \
\n\nMap a word only where its meaning is clear from the word itself. A word that means nothing on the \
scale (unknown, n/a, not set) maps to nothing: leave it out. Never map two different grades onto one \
step to make a disagreement go away: a disagreement between sources is what the reader is here to see.";

fn align_schema() -> J {
    json!({
        "type": "object",
        "required": ["scale", "maps"],
        "properties": {
            "scale": { "type": "array", "items": { "type": "string" } },
            "maps": { "type": "array", "items": { "type": "object", "required": ["source", "word", "means"], "properties": {
                "source": { "type": "string" }, "word": { "type": "string" }, "means": { "type": "string" } } } },
            "tolerance": { "type": "string" },
        },
    })
}

// -- a why -----------------------------------------------------------------------------------

/// One sentence: why this tracker reads this source, which a person keeps or edits.
pub fn why(assist: &Assist, tracker: &Path, source: &Path, yes: bool) -> Result<Outcome<String>, String> {
    let t = crate::trackerdecl::TrackerDecl::load(tracker)?;
    let s = crate::sourcedecl::SourceDecl::load(source)?;
    let sends = vec![format!("the names and one-sentence descriptions of {} and of the tracker's sources, and the names of its properties", s.title)];
    let Some(disclosure) = consent(assist, source, sends.clone(), yes)? else {
        return Ok(Outcome::NeedsConsent(Disclosure { to: assist.who(), sends }));
    };
    let mut user = format!("The tracker: {}. {}\n\nIts sources and why:\n", t.title, t.about);
    for m in t.members.iter().filter(|m| m.dataset != s.name) {
        user.push_str(&format!("- {}: {}\n", m.dataset, m.why));
    }
    user.push_str(&format!(
        "\nThe new source: {} ({}), {}\nIts properties: {}\n",
        s.title,
        s.kind,
        s.about,
        s.records.fields.keys().cloned().collect::<Vec<_>>().join(", ")
    ));
    let a = assist.ask("why", &format!("{}#{}", t.name, s.name), WHY_SYSTEM, &user, &json!({
        "type": "object", "required": ["why"], "properties": { "why": { "type": "string" } } }))?;
    crate::assist::sent(source, "why", &disclosure)?;
    let why = a["why"].as_str().unwrap_or("").trim().to_string();
    if why.is_empty() {
        return Err("the answer held no sentence".into());
    }
    Ok(Outcome::Done(why))
}

const WHY_SYSTEM: &str = "Write one sentence, at most twenty-five words, saying what this source tells \
the tracker that its other sources do not. Say it plainly, from what the source is; do not praise it, \
do not summarise what it contains, and do not claim anything its description does not say.";

// -- mending ---------------------------------------------------------------------------------

/// A declaration mended from what its updates complained about. Proposed, not applied: it is held
/// against the checks an apply runs and tried, and a person decides.
pub fn mend(assist: &Assist, brief: &str, key: &str, dir: &Path, yes: bool) -> Result<Outcome<String>, String> {
    let sends = vec!["the source's declaration, its property names and counts, and what its last three updates said".to_string()];
    let Some(disclosure) = consent(assist, dir, sends.clone(), yes)? else {
        return Ok(Outcome::NeedsConsent(Disclosure { to: assist.who(), sends }));
    };
    let (text, changed) = mend_text(assist, brief, key)?;
    crate::assist::sent(dir, "mend", &disclosure)?;
    let proposed: crate::sourcedecl::SourceDecl = crate::yaml::parse(&text).map_err(|e| format!("the proposal does not parse: {e}"))?;
    let held = crate::sourcedecl::SourceDecl::load(dir)?;
    if proposed.name != held.name {
        return Err(format!("the proposal calls itself {}, and this is {}", proposed.name, held.name));
    }
    let trial = try_out(&proposed, dir)?;
    let text = crate::yaml::to_string(&proposed)?;
    Ok(Outcome::Done(format!(
        "# {}\n# Tried: {}\n{text}",
        changed,
        trial.said().replace('\n', "\n#")
    )))
}

const MEND_SYSTEM: &str = "You mend a Zetlyn source declaration. You are shown the declaration, the \
properties it produced and what its last updates said: values that did not parse, claims with no text \
or no date, errors, refusals. Change only what those complaints point at, keep the source's name, and \
answer with the whole declaration. If the complaints do not say what is wrong, change nothing and say so.";

/// The question alone: a proposed declaration and one sentence on what changed. What holds it
/// against the source is the caller's: locally the checks and a try, on a platform the console's
/// apply.
pub fn mend_text(assist: &Assist, brief: &str, key: &str) -> Result<(String, String), String> {
    let a = assist.ask("mend", key, MEND_SYSTEM, brief, &json!({
        "type": "object", "required": ["declaration", "changed"], "properties": {
            "declaration": { "type": "string", "description": "the whole declaration, as YAML" },
            "changed": { "type": "string", "description": "one sentence: what was wrong and what the new one does differently" } } }))?;
    let text = a["declaration"].as_str().unwrap_or("").trim().to_string();
    if text.is_empty() {
        return Err("the answer held no declaration".into());
    }
    Ok((text, a["changed"].as_str().unwrap_or("").trim().to_string()))
}

/// One property's `align:` entry put into a tracker's file as text, so every comment a person
/// wrote in it stays where they wrote it. Written back through the parser, a file loses them.
pub fn splice_align(text: &str, property: &str, entry: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    // The entry as the lines under `align:`: its own YAML, two spaces in.
    let block: Vec<String> = entry.lines().map(|l| format!("  {l}")).collect();
    let Some(align) = lines.iter().position(|l| l.trim_end() == "align:") else {
        let mut out = text.trim_end().to_string();
        out.push_str("\nalign:\n");
        out.push_str(&block.join("\n"));
        out.push('\n');
        return out;
    };
    let end_of_align = lines[align + 1..]
        .iter()
        .position(|l| !l.is_empty() && !l.starts_with(' ') && !l.starts_with('#'))
        .map(|i| align + 1 + i)
        .unwrap_or(lines.len());
    let head = format!("  {property}:");
    let start = (align + 1..end_of_align).find(|&i| lines[i] == head || lines[i].starts_with(&format!("{head} ")));
    let mut out: Vec<String> = Vec::new();
    match start {
        Some(s) => {
            // Its block runs to the next line at its own depth or less. A comment at that depth
            // is the next entry's, written above it.
            let e = lines[s + 1..end_of_align]
                .iter()
                .position(|l| !l.is_empty() && !l.starts_with("   "))
                .map(|i| s + 1 + i)
                .unwrap_or(end_of_align);
            out.extend(lines[..s].iter().map(|l| l.to_string()));
            out.extend(block);
            out.extend(lines[e..].iter().map(|l| l.to_string()));
        }
        None => {
            out.extend(lines[..end_of_align].iter().map(|l| l.to_string()));
            out.extend(block);
            out.extend(lines[end_of_align..].iter().map(|l| l.to_string()));
        }
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_alignment_goes_in_without_taking_the_comments_out() {
        let file = "name: t/x\n# why these\nalign:\n  # CVSS from everyone\n  cvss: {}\n  severity:\n    scale:\n    - high\n  # the last one\n  status: {}\nview:\n  columns: []\n";
        let out = splice_align(file, "severity", "severity:\n  scale:\n  - critical\n  - high\n");
        assert_eq!(out, "name: t/x\n# why these\nalign:\n  # CVSS from everyone\n  cvss: {}\n  severity:\n    scale:\n    - critical\n    - high\n  # the last one\n  status: {}\nview:\n  columns: []\n");
        let added = splice_align(file, "licence", "licence: {}\n");
        assert!(added.contains("  status: {}\n  licence: {}\nview:"), "{added}");
        assert_eq!(splice_align("name: t/x\n", "cvss", "cvss: {}\n"), "name: t/x\nalign:\n  cvss: {}\n");
    }

    fn nvd_like() -> J {
        let item = |id: &str, score: f64| json!({ "cve": {
            "id": id, "published": "2026-09-01T00:00:00.000",
            "descriptions": [{ "lang": "en", "value": format!("A flaw in {id}") }],
            "metrics": { "cvssMetricV31": [{ "cvssData": { "baseScore": score, "baseSeverity": "HIGH" } }] } } });
        json!({ "resultsPerPage": 3, "startIndex": 0, "totalResults": 9,
                "vulnerabilities": [item("CVE-2026-0001", 7.5), item("CVE-2026-0002", 9.8), item("CVE-2026-0003", 5.0)] })
    }

    #[test]
    fn an_answer_becomes_a_declaration_and_what_it_invents_does_not() {
        let facts = Facts::of(&nvd_like(), None, None);
        assert_eq!(facts.lists, [("vulnerabilities[]".to_string(), 3)]);
        assert!(facts.ids.iter().any(|(p, s, ..)| p == "cve.id" && *s == "cve"));
        let answer = json!({
            "kind": "vulnerability", "title": "NVD", "about": "CVEs.", "each": "vulnerabilities[]",
            "id": [{ "scheme": "cve", "from": "cve.id" }],
            "title_from": "cve.descriptions[].value", "text": ["cve.descriptions[].value"], "known": "cve.published",
            "url": "https://nvd.nist.gov/vuln/detail/{cve.id}",
            "properties": [
                { "name": "cvss", "type": "number", "from": "cve.metrics.cvssMetricV31[].cvssData.baseScore" },
                { "name": "epss", "type": "number", "from": "cve.metrics.epss.score" }
            ],
            "paging": { "style": "offset", "offset_param": "startIndex", "size_param": "resultsPerPage", "total": "totalResults" },
            "incremental": { "query": "lastModStartDate={since}", "field": "cve.lastModified" }
        });
        let d = serde_json::to_value(declaration_from(&answer, "https://x/cves?startIndex=0", "t/nvd", &facts).unwrap()).unwrap();
        assert_eq!(d["claims"]["each"], "field:vulnerabilities[]");
        assert_eq!(d["claims"]["url"], "const:https://nvd.nist.gov/vuln/detail/{field:cve.id}");
        // A property on a path the answer never had is dropped, and so is an incremental query
        // keyed on a path that is not there.
        assert!(d["claims"]["properties"].get("epss").is_none());
        assert!(d["claims"]["properties"].get("cvss").is_some());
        assert!(d["fetch"].get("since").is_none());
        assert_eq!(d["fetch"]["page"]["total"], "field:totalResults");
        // The page the address named is not asked for again on every page.
        assert_eq!(d["fetch"]["list"], "https://x/cves");
        assert_eq!(last_page("https://x/cves", &answer, &facts).as_deref(), Some("https://x/cves?startIndex=6"));
    }
}
