//! The surface. One stylesheet, server-rendered, no build step.

use std::collections::BTreeMap;

use maud::{html, Markup, PreEscaped, DOCTYPE};
use serde_json::{json, Value as J};

use crate::source::{Source, Query};
use crate::sourcedecl::View;
use crate::expr::{self, Lit, Op, Pred};
use crate::claim::{Claim, Value};
use crate::store::Hit;

pub const STYLE: &str = r#"
/* The palette is the website's, to the value. A reader who arrives from zetlyn.com or from a hub
   should not be told by the colours that they have left. The layout is this program's own: a
   scope surface is a dense thing and the site's vocabulary has no rows, facets or chips in it. */
:root {
  --bg: #f2efe7; --fg: #14202a; --dim: #667078; --line: #cfd1ca;
  --panel: #fbfaf6; --accent: #dc4a20; --chip: #dfe2db;
  color-scheme: light;
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    color-scheme: dark;
    --bg: #11181d; --fg: #e9e6de; --dim: #98a3ab; --line: #2b353c;
    --panel: #161e24; --accent: #ff6a3d; --chip: #1c252b;
  }
}
:root[data-theme="dark"] {
  color-scheme: dark;
  --bg: #11181d; --fg: #e9e6de; --dim: #98a3ab; --line: #2b353c;
  --panel: #161e24; --accent: #ff6a3d; --chip: #1c252b;
}
:root[data-theme="light"] { color-scheme: light; }
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--fg);
       font: 15px/1.55 Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont,
             "Segoe UI", sans-serif; -webkit-font-smoothing: antialiased; }
main { max-width: 68rem; margin: 0 auto; padding: 2rem 1rem 5rem; }
a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
h1 { font-size: 1.5rem; margin: 0 0 .2rem; letter-spacing: -.02em; font-weight: 750; }
h2 { font-size: .8rem; text-transform: uppercase; letter-spacing: .12em;
     color: var(--dim); margin: 2.2rem 0 .7rem; font-weight: 600;
     font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace; }
h3 { font-size: 1rem; margin: 1.4rem 0 .4rem; }
.about { color: var(--dim); margin: 0 0 1.2rem; max-width: 48rem; }
.bar { display: flex; gap: .5rem; flex-wrap: wrap; align-items: center; margin: 1rem 0; }
input[type=search] { flex: 1 1 22rem; min-width: 0; padding: .55rem .7rem; font: inherit;
  background: var(--panel); color: var(--fg); border: 1px solid var(--line); border-radius: 6px; }
button { padding: .55rem .9rem; font: inherit; cursor: pointer; border-radius: 6px;
  border: 1px solid var(--line); background: var(--panel); color: var(--fg); }
.chip { display: inline-block; padding: .12rem .5rem; border-radius: 999px;
  background: var(--chip); color: var(--fg); font-size: .82rem; white-space: nowrap; }
.chip.on { background: var(--accent); color: var(--bg); }
.state { font-size: .82rem; }
.state.current { color: #2e7d32; } .state.partial, .state.failing { color: #c0392b; }
.state.empty { color: var(--dim); }
table { border-collapse: collapse; width: 100%; font-size: .92rem; }
th { text-align: left; font-weight: 600; color: var(--dim); font-size: .78rem;
     text-transform: uppercase; letter-spacing: .05em; padding: .4rem .6rem .4rem 0;
     border-bottom: 1px solid var(--line); }
td { padding: .5rem .6rem .5rem 0; border-bottom: 1px solid var(--line);
     vertical-align: top; }
td.num { text-align: right; font-variant-numeric: tabular-nums; }
.grid { display: grid; gap: 1.2rem; grid-template-columns: repeat(auto-fit, minmax(15rem, 1fr)); }
.card { border: 1px solid var(--line); border-radius: 8px; padding: .8rem 1rem;
        background: var(--panel); }
.card h4 { margin: 0 0 .4rem; font-size: .9rem; }
.cover { color: var(--dim); font-size: .8rem; font-weight: 400; }
.facet { display: flex; justify-content: space-between; gap: .6rem; padding: .16rem 0;
         font-size: .88rem; }
.facet .n { color: var(--dim); font-variant-numeric: tabular-nums; }
.dim { color: var(--dim); }
.why { color: var(--dim); font-size: .8rem; }
.note { border-left: 3px solid var(--accent); padding: .5rem .8rem; background: var(--panel);
        margin: 1rem 0; font-size: .9rem; }
.text { white-space: pre-wrap; max-width: 46rem; }
footer { margin-top: 3rem; padding-top: 1rem; border-top: 1px solid var(--line);
         color: var(--dim); font-size: .82rem; }
/* A receipt: where one value came from, opened in place. */
details.receipt { margin-top: .25rem; font-size: .85rem; }
details.receipt > summary { cursor: pointer; color: var(--dim); list-style: none; }
details.receipt > summary::-webkit-details-marker { display: none; }
details.receipt > summary::before { content: "↗ "; color: var(--accent); }
details.receipt[open] { background: var(--panel); border-left: 3px solid var(--accent);
                        padding: .5rem .8rem; margin: .4rem 0; }
.receipt dl { display: grid; grid-template-columns: max-content 1fr; gap: .2rem .9rem; margin: .4rem 0; }
.receipt dt { color: var(--dim); }
.receipt dd { margin: 0; }
.receipt pre { max-height: 18rem; overflow: auto; font-size: .78rem; background: var(--bg);
               padding: .5rem; border: 1px solid var(--line); }
.receipt table { font-size: .82rem; }
@media (max-width: 40rem) { main { padding: 1.2rem .9rem 4rem; } }
"#;

pub fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("20");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b' '));
                i += 3;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|c| match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (c as char).to_string()
            }
            b' ' => "+".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

pub fn params(url: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some((_, q)) = url.split_once('?') {
        for pair in q.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                out.insert(urldecode(k), urldecode(v));
            } else if !pair.is_empty() {
                out.insert(urldecode(pair), String::new());
            }
        }
    }
    out
}

pub fn flatten(pred: &Pred, out: &mut Vec<(String, Op, Lit)>) {
    match pred {
        Pred::And(a, b) | Pred::Or(a, b) => {
            flatten(a, out);
            flatten(b, out);
        }
        Pred::Cmp { left, op, right } => out.push((left.clone(), *op, right.clone())),
    }
}

fn link(base: &str, q: &str, view: &str, sort: &str, page: usize) -> String {
    let mut url = format!("{base}?q={}", urlencode(q));
    if !view.is_empty() {
        url.push_str(&format!("&view={}", urlencode(view)));
    }
    if !sort.is_empty() {
        url.push_str(&format!("&sort={}", urlencode(sort)));
    }
    if page > 1 {
        url.push_str(&format!("&page={page}"));
    }
    url
}


/// Where this surface is mounted, so several of them can sit on one host. Empty at the root.
///
/// One process serves one thing, so this is set once before the loop starts and read from
/// everywhere a link is written. The router strips it; every address a browser is given carries
/// it.
static MOUNT: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// A prefix with a leading slash and no trailing one, or nothing at all.
pub fn mount(prefix: &str) {
    let p = prefix.trim().trim_end_matches('/');
    let p = if p.is_empty() {
        String::new()
    } else if let Some(rest) = p.strip_prefix('/') {
        format!("/{rest}")
    } else {
        format!("/{p}")
    };
    let _ = MOUNT.set(p);
}

pub fn mounted() -> &'static str {
    MOUNT.get().map(String::as_str).unwrap_or("")
}

/// An address on this surface, as a browser has to ask for it.
pub fn at(path: &str) -> String {
    format!("{}{}", mounted(), path)
}

/// What was asked for, with the mount taken off, so the router matches one set of addresses
/// whatever the surface is mounted under. A request for the mount itself is a request for `/`.
pub fn unmount(url: &str) -> String {
    let m = mounted();
    if m.is_empty() {
        return url.to_string();
    }
    match url.strip_prefix(m) {
        Some("") => "/".to_string(),
        Some(rest) if rest.starts_with('/') || rest.starts_with('?') => {
            if rest.starts_with('?') {
                format!("/{rest}")
            } else {
                rest.to_string()
            }
        }
        _ => url.to_string(),
    }
}

pub fn shell(title: &str, body: Markup) -> String {
    let page = html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) }
                link rel="stylesheet" href=(at("/style.css"));
            }
            body { main { (body) } }
        }
    };
    page.into_string()
}

fn value_cell(v: &Value) -> Markup {
    html! { (v.display()) }
}

fn column_of(hit: &Hit, name: &str) -> Markup {
    match name {
        "known" => html! { (hit.known) },
        "kind" => html! { (hit.kind) },
        "title" => html! { a href={(at("/claim/")) (hit.record_id)} { (hit.title) } },
        other => match hit.fields.get(other) {
            Some(v) => value_cell(v),
            None => html! { span.dim { "—" } },
        },
    }
}

fn numeric(ds: &Source, name: &str) -> bool {
    ds.decl
        .records
        .fields
        .get(name)
        .map(|f| matches!(f.kind, crate::sourcedecl::PropertyType::Number))
        .unwrap_or(false)
}

fn table(ds: &Source, hits: &[Hit], view: Option<&View>, q: &str, sort: &str) -> Markup {
    let mut cols: Vec<String> = view.map(|v| v.columns.clone()).unwrap_or_default();
    if cols.is_empty() {
        cols = vec!["known".into()];
    }
    let view_name = view.map(|v| v.name.clone()).unwrap_or_default();
    html! {
        table {
            thead { tr {
                th { "Title" }
                @for c in &cols {
                    th {
                        a href=(link("/", q, &view_name, &format!("{c} desc"), 1)) { (c) }
                    }
                }
            } }
            tbody {
                @for hit in hits {
                    tr {
                        td {
                            a href={(at("/claim/")) (hit.record_id)} { (hit.title) }
                            @if !hit.why_text.is_empty() || hit.why_id.is_some() {
                                div.why {
                                    @if let Some(id) = &hit.why_id {
                                        "identifier " (id.value)
                                    } @else {
                                        "matched " (hit.why_text.join(", "))
                                    }
                                }
                            }
                        }
                        @for c in &cols {
                            td.num[numeric(ds, c)] { (column_of(hit, c)) }
                        }
                    }
                }
            }
        }
        @if hits.is_empty() {
            p.dim { "Nothing here." }
        }
        @let _ = sort;
    }
}

fn overview(ds: &Source, url: &str) -> String {
    let p = params(url);
    let q = p.get("q").cloned().unwrap_or_default();
    let sort = p.get("sort").cloned().unwrap_or_default();
    let page: usize = p
        .get("page")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .max(1);
    let view_name = p
        .get("view")
        .cloned()
        .or_else(|| ds.decl.default_view().map(|v| v.name.clone()))
        .unwrap_or_default();

    let (text, pred) = expr::parse_query(&q);
    let limit = 50;
    let query = Query {
        text,
        pred: pred.clone(),
        ids: Vec::new(),
        seen_before: None,
        view: if view_name.is_empty() {
            None
        } else {
            Some(view_name.clone())
        },
        sort: if sort.is_empty() {
            None
        } else {
            Some(sort.clone())
        },
        limit,
        offset: (page - 1) * limit,
    };
    let view = ds.decl.view(&view_name);
    let (total, hits, unanswered) = match ds.search(&query) {
        Ok(v) => v,
        Err(e) => (0, Vec::new(), crate::store::Unanswered(vec![e])),
    };

    let d = &ds.decl;
    let report = ds.store.run_report(ds.store.last_run());
    let state = ds.state();
    let summaries = ds.store.fields(d);
    let total_records = ds.store.count();

    let mut active = Vec::new();
    if let Some(pr) = &pred {
        flatten(pr, &mut active);
    }

    let facets: Vec<String> = view
        .map(|v| v.facets.clone())
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| summaries.iter().map(|f| f.name.clone()).take(4).collect());

    let body = html! {
        h1 { (d.title) }
        @if !d.about.is_empty() { p.about { (d.about) } }
        p.state.(state) {
            (state) " · " (total_records) " claims"
            @if let Some(r) = &report {
                " · update " (r.id) " " (r.started)
                @if r.added + r.changed + r.removed > 0 {
                    " · +" (r.added) " ~" (r.changed) " −" (r.removed)
                }
            }
        }

        @if let Some(r) = &report {
            @if let Some(why) = &r.refused {
                div.note { "The last update was refused and the store was not replaced: " (why) }
            }
            @if let Some(err) = &r.error {
                div.note { "The last update did not finish: " (err) }
            }
            @if r.no_text > 0 || r.unparsed > 0 || r.no_known > 0 {
                div.note {
                    @if r.no_text > 0 { (r.no_text) " claims came out with no text. " }
                    @if r.no_known > 0 { (r.no_known) " took their date from the file. " }
                    @if r.unparsed > 0 {
                        (r.unparsed) " values did not parse as their type"
                        @if let Some(n) = &r.note { @if !n.is_empty() { " — " (n) } }
                        "."
                    }
                }
            }
        }

        form.bar method="get" action=(at("/")) {
            input type="search" name="q" value=(q) placeholder="Search, or filter with name=value";
            @if !view_name.is_empty() { input type="hidden" name="view" value=(view_name); }
            button type="submit" { "Search" }
        }

        @if q.is_empty() && !d.search.examples.is_empty() {
            p.bar {
                span.dim { "Try:" }
                @for ex in &d.search.examples {
                    a.chip href=(link("/", ex, &view_name, "", 1)) { (ex) }
                }
            }
        }

        @if !active.is_empty() {
            p.bar {
                @for (name, op, lit) in &active {
                    @let one = format!("{name}{}{}", op.sql(), lit.display());
                    @let without = q.replace(&one, "").split_whitespace()
                        .collect::<Vec<_>>().join(" ");
                    a.chip.on href=(link("/", &without, &view_name, &sort, 1)) {
                        (name) (op.sql()) (lit.display()) " ✕"
                    }
                }
            }
        }

        @if !unanswered.0.is_empty() {
            div.note {
                "Not answered here: "
                @for (i, u) in unanswered.0.iter().enumerate() {
                    @if i > 0 { "; " }
                    (u)
                }
            }
        }

        @if d.view.len() > 1 {
            p.bar {
                @for v in &d.view {
                    @let t = if v.title.is_empty() { v.name.clone() } else { v.title.clone() };
                    @if v.name == view_name {
                        span.chip.on { (t) }
                    } @else {
                        a.chip href=(link("/", &q, &v.name, "", 1)) { (t) }
                    }
                }
            }
        }

        h2 { (total) " of " (total_records) }
        (table(ds, &hits, view, &q, &sort))

        @if total > limit as u64 {
            p.bar {
                @if page > 1 {
                    a href=(link("/", &q, &view_name, &sort, page - 1)) { "← previous" }
                }
                span.dim { "page " (page) " of " ((total as usize).div_ceil(limit)) }
                @if (page * limit) < total as usize {
                    a href=(link("/", &q, &view_name, &sort, page + 1)) { "next →" }
                }
            }
        }

        @if !facets.is_empty() {
            h2 { "Facets" }
            div.grid {
                @for name in &facets {
                    @let counts = ds.facet(&query, name, 8);
                    @let summary = summaries.iter().find(|s| &s.name == name);
                    @if !counts.is_empty() {
                        div.card {
                            h4 {
                                (name)
                                @if let Some(s) = summary {
                                    " " span.cover {
                                        (s.records) " of " (total_records)
                                    }
                                }
                            }
                            @for (v, n) in &counts {
                                div.facet {
                                    a href=(link("/", format!("{q} {name}={v}").trim(),
                                                 &view_name, &sort, 1)) { (v) }
                                    span.n { (n) }
                                }
                            }
                        }
                    }
                }
            }
        }

        h2 { "What this source holds" }
        div.grid {
            div.card {
                h4 { "Identifiers" }
                @let schemes = ds.store.schemes();
                @if schemes.is_empty() {
                    p.dim { "None. Claims are addressed by where they came from." }
                } @else {
                    @for (s, n) in &schemes {
                        div.facet { span { (s) } span.n { (n) } }
                    }
                }
            }
            div.card {
                h4 { "Properties" }
                @for f in &summaries {
                    div.facet {
                        span { (f.name) " " span.dim { (f.kind) } }
                        span.n {
                            (f.records)
                            @if let (Some(lo), Some(hi)) = (&f.min, &f.max) {
                                " · " (lo) "–" (hi)
                            }
                        }
                    }
                }
                @if summaries.is_empty() { p.dim { "None declared." } }
            }
            div.card {
                h4 { "Can answer" }
                p { @for c in ds.can() { span.chip { (c) } " " } }
                @if !d.search.compare.is_empty() {
                    p.dim { "Compares: " (d.search.compare.join(", ")) }
                }
            }
        }

        footer {
            (d.name) " · " (d.source.kind_name()) " · kind " (d.kind)
            " · " a href=(at("/api/describe")) { "describe" }
            " · " a href=(at("/changes")) { "changes" }
        }
    };
    shell(&d.title, body)
}

fn record_page(ds: &Source, id: &str, url: &str) -> Option<String> {
    let asked = params(url).get("as_of").cloned();
    // `as_of` shows a claim as it stood, from the revisions the source kept. It reads one
    // claim: the text index is current, so it does not make a whole query answer as of a date.
    let then = asked.as_deref().and_then(|at| ds.store.as_of(id, at));
    let mut rec = ds.fetch(&[id.to_string()], true).into_iter().next()?;
    let answered = ds
        .store
        .run_report(ds.store.last_run())
        .and_then(|r| r.finished);
    let found = then.is_some();
    if let Some((title, fields)) = then {
        rec.title = title;
        rec.fields = fields;
    }
    let d = &ds.decl;
    let body = html! {
        p { a href=(at("/")) { "← " (d.title) } }
        h1 { (rec.title) }
        @if let Some(when) = &asked {
            @if found {
                div.note { "As it stood on " (when) ". "
                    a href={(at("/claim/")) (rec.record_id)} { "Now" } }
            } @else {
                div.note { "No version of this claim from on or before " (when)
                    " is kept, so these are today's values." }
            }
        }
        p.state {
            span.chip { (rec.kind) } " "
            @for i in &rec.ids { span.chip { (i.scheme) " " (i.value) } " " }
            span.dim { "known " (rec.known) }
        }
        @if let Some(u) = &rec.url {
            p { a href=(u) { (u) } }
        }
        @if !rec.fields.is_empty() {
            h2 { "Properties" }
            table {
                tbody {
                    @for (name, value) in &rec.fields {
                        tr {
                            th style="width: 12rem" { (name) }
                            td {
                                (value.display())
                                @if let Value::Code { code, vocabulary } = value {
                                    @if let Some(v) = vocabulary {
                                        @if let Some(means) = d.vocabulary.get(v)
                                            .and_then(|m| m.get(code)) {
                                            div.why { (means) }
                                        }
                                    }
                                }
                                (receipt(&rec, name, &d.title, answered.as_deref()))
                            }
                        }
                    }
                }
            }
        }
        @if !rec.text.trim().is_empty() {
            h2 { "Text" }
            div.text { (rec.text) }
        }
        footer {
            "from " (rec.from.address())
            " · " (rec.hash)
            " · " a href={(at("/api/fetch?id=")) (rec.record_id)} { "json" }
        }
    };
    Some(shell(&rec.title, body))
}

fn changes_page(ds: &Source, url: &str) -> String {
    let p = params(url);
    let since: i64 = p
        .get("since")
        .and_then(|s| s.parse().ok())
        .unwrap_or(ds.mark() - 1);
    let j = ds.changes(since, 200);
    let empty = Vec::new();
    let changed = j["changed"].as_array().unwrap_or(&empty);
    let removed = j["removed"].as_array().unwrap_or(&empty);
    let body = html! {
        p { a href=(at("/")) { "← " (ds.decl.title) } }
        h1 { "Changes" }
        p.dim { "Since update " (since) ". The mark now is " (ds.mark()) "." }
        @if changed.is_empty() && removed.is_empty() {
            p.dim { "Nothing since then." }
        }
        table { tbody {
            @for c in changed {
                tr {
                    td style="width: 6rem" { span.chip { (c["how"].as_str().unwrap_or("")) } }
                    td { a href={(at("/claim/")) (c["claim_id"].as_str().unwrap_or(""))} {
                        (c["title"].as_str().unwrap_or("")) } }
                }
            }
            @for c in removed {
                tr {
                    td { span.chip { "removed" } }
                    td.dim { (c["title"].as_str().unwrap_or("")) }
                }
            }
        } }
    };
    shell("Changes", body)
}

/// The same six calls, over HTTP. One interface and not two. The second value is true when the
/// path names no call, because a caller who mistypes one should hear that and not a search.
fn api(ds: &Source, path: &str, url: &str) -> (J, bool) {
    let p = params(url);
    let q = || {
        let (text, pred) = expr::parse_query(p.get("q").map(String::as_str).unwrap_or(""));
        Query {
            text,
            pred,
            view: p.get("view").cloned(),
            ids: p
                .get("ids")
                .map(|s| s.split(',').map(str::to_string).collect())
                .unwrap_or_default(),
            seen_before: p.get("seen_before").cloned(),
            sort: p.get("sort").cloned(),
            limit: p.get("limit").and_then(|s| s.parse().ok()).unwrap_or(50),
            offset: p.get("offset").and_then(|s| s.parse().ok()).unwrap_or(0),
        }
    };
    let answer = match path {
        "/api/describe" => ds.describe(),
        "/api/mark" => json!({ "mark": ds.mark() }),
        "/api/changes" => ds.changes(
            p.get("since").and_then(|s| s.parse().ok()).unwrap_or(0),
            p.get("limit").and_then(|s| s.parse().ok()).unwrap_or(200),
        ),
        "/api/facet" => {
            let field = p.get("property").cloned().unwrap_or_default();
            let counts = ds.facet(
                &q(),
                &field,
                p.get("limit").and_then(|s| s.parse().ok()).unwrap_or(50),
            );
            json!({ "property": field, "values":
                J::Array(counts.iter().map(|(v, n)| json!({ "value": v, "claims": n })).collect()) })
        }
        "/api/fetch" => {
            let ids: Vec<String> = p
                .get("id")
                .map(|s| s.split(',').map(str::to_string).collect())
                .unwrap_or_default();
            let versions = p.get("versions").is_some_and(|v| v == "1" || v == "true");
            json!({ "claims": J::Array(ds.fetch(&ids, versions).iter().map(|r| r.to_json()).collect()) })
        }
        "/api/search" => {
            let query = q();
            match ds.search(&query) {
                Ok((total, hits, unanswered)) => json!({
                    "total": total,
                    "unanswered": unanswered.0,
                    "hits": J::Array(hits.iter().map(|h| json!({
                        "claim_id": h.record_id,
                        "rank": h.rank,
                        "why": { "text": h.why_text, "property": h.why_field,
                                 "id": h.why_id.as_ref().map(|i| json!({
                                     "scheme": i.scheme, "value": i.value })) },
                        "title": h.title,
                        "url": h.url,
                        "kind": h.kind,
                        "known": h.known,
                        "ids": J::Array(h.ids.iter().map(|i| json!({
                            "scheme": i.scheme, "value": i.value })).collect()),
                        "properties": J::Object(h.fields.iter()
                            .map(|(k, v)| (k.clone(), v.to_json())).collect()),
                    })).collect()),
                }),
                Err(e) => json!({ "error": e }),
            }
        }
        _ => {
            return (
                json!({ "error": "no such call", "calls": [
                    "/api/describe", "/api/search", "/api/fetch",
                    "/api/facet", "/api/changes", "/api/mark",
                ] }),
                true,
            )
        }
    };
    (answer, false)
}

pub fn serve(ds: Source, addr: &str) -> Result<(), String> {
    let server = tiny_http::Server::http(addr).map_err(|e| e.to_string())?;
    println!("{} on http://{addr}", ds.decl.name);
    for request in server.incoming_requests() {
        let url = unmount(request.url());
        let path = url.split('?').next().unwrap_or("/").to_string();
        // The same rule as the tracker surface: a miss is a 404, and only the front page is the
        // front page. Every other address that matches nothing is nothing.
        let mut status = 200u16;
        let (body, content_type) = if path == "/style.css" {
            (STYLE.to_string(), "text/css; charset=utf-8")
        } else if path.starts_with("/api/") {
            let (answer, no_such_call) = api(&ds, &path, &url);
            if no_such_call {
                status = 404;
            }
            (answer.to_string(), "application/json")
        } else if let Some(id) = path.strip_prefix("/claim/") {
            match record_page(&ds, id, &url) {
                Some(html) => (html, "text/html; charset=utf-8"),
                None => {
                    status = 404;
                    (
                        shell("Not here", html! { h1 { "No such claim" } }),
                        "text/html; charset=utf-8",
                    )
                }
            }
        } else if path == "/changes" {
            (changes_page(&ds, &url), "text/html; charset=utf-8")
        } else if path != "/" {
            status = 404;
            (
                shell(
                    "Nothing here",
                    html! {
                        p { a href=(at("/")) { "← " (ds.decl.title) } }
                        h1 { "Nothing here at that address" }
                    },
                ),
                "text/html; charset=utf-8",
            )
        } else {
            (overview(&ds, &url), "text/html; charset=utf-8")
        };
        let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes())
            .map_err(|_| "bad header".to_string())?;
        let response = tiny_http::Response::from_string(body)
            .with_status_code(status)
            .with_header(header);
        let _ = request.respond(response);
    }
    Ok(())
}

#[allow(dead_code)]
fn unused(_: PreEscaped<String>) {}

/// Where one value came from: the words its source used and the expression that read them, since
/// when it has said so, when the source last answered, what the source handed over, and every
/// value it said before. Every fact has a receipt, and this is it.
pub fn receipt(claim: &Claim, property: &str, source: &str, answered: Option<&str>) -> Markup {
    let said = &claim.excerpt.as_ref().map(|e| e["properties"][property].clone()).unwrap_or(J::Null);
    let words: Vec<String> = said["raw"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let now = claim.fields.get(property).map(|v| v.display()).unwrap_or_default();
    // The value over time, one row per change rather than one per version: a version that moved
    // another property did not move this one.
    let mut changes: Vec<(String, String)> = Vec::new();
    for v in &claim.versions {
        let value = crate::claim::Value::from_json(&v.properties[property])
            .map(|x| x.display())
            .unwrap_or_else(|| "—".into());
        if changes.last().map(|(_, last)| last != &value).unwrap_or(true) {
            changes.push((v.at.clone(), value));
        }
    }
    let since = changes.last().filter(|(_, v)| *v == now).map(|(at, _)| at.clone());
    let original = claim.url.clone().or_else(|| claim.from.url.clone());
    let row = claim.excerpt.as_ref().and_then(|e| e.get("row")).cloned();
    let too_large = claim.excerpt.as_ref().and_then(|e| e["row_bytes"].as_u64());
    html! {
        details.receipt {
            summary { "receipt" }
            dl {
                dt { "Source" } dd { (source) }
                @if !words.is_empty() {
                    dt { "Its words" } dd { code { (words.join(", ")) } }
                }
                @if let Some(from) = said["from"].as_str() {
                    dt { "Read by" } dd { code { (from) } }
                }
                @if let Some(at) = &since {
                    dt { "Said since" } dd { (stamp(at)) }
                }
                @if let Some(at) = answered {
                    dt { "Last answered" } dd { (stamp(at)) }
                }
                @if let Some(u) = &original {
                    dt { "Original" } dd { a href=(u) { "open at the source" } }
                }
            }
            @if changes.len() > 1 {
                table { tbody {
                    @for (at, value) in changes.iter().rev() {
                        tr { td.dim { (stamp(at)) } td { (value) } }
                    }
                } }
            }
            @if let Some(row) = &row {
                details {
                    summary { "What the source handed over" }
                    pre { (serde_json::to_string_pretty(row).unwrap_or_default()) }
                }
            } @else if let Some(bytes) = too_large {
                p.dim { "The source handed over " (bytes) " bytes for this, which is kept whole at the source and not here." }
            }
            @if claim.excerpt.is_none() {
                p.dim { "This source has not kept a receipt for this claim yet. The next update that reads it will." }
            }
        }
    }
}

/// `2026-09-28T18:42:10Z` as a person reads it.
fn stamp(at: &str) -> String {
    match (at.get(..10), at.get(11..16)) {
        (Some(d), Some(t)) => format!("{d} {t} UTC"),
        _ => at.to_string(),
    }
}
