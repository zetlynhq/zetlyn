//! Three source kinds, each handing rows to the same builder.
//!
//! A row is the structured value, what the container says about it, the file it came from, and
//! whatever text was extracted. What a declaration makes of that is not the source's business.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value as J};

use crate::sourcedecl::{SourceDecl, Fetch};
use crate::expr::{FileInfo, Row};
use crate::claim::Origin;

pub struct Produced<'a> {
    /// The source already split this row, so the builder does not split it again.
    pub expanded: bool,
    pub row: Row<'a>,
    pub origin: Origin,
}

/// A glob, as much of one as a declaration needs: `*` inside a segment, `**` across them.
/// `**/target/**` matches a `target` at any depth, which is where build output actually sits.
pub fn glob_to_regex(pattern: &str) -> regex::Regex {
    let mut re = String::from("^");
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if i + 1 < chars.len() && chars[i + 1] == '*' => {
                i += 2;
                if i < chars.len() && chars[i] == '/' {
                    // `**/` is any number of leading segments, including none.
                    re.push_str("(?:.*/)?");
                    i += 1;
                } else {
                    re.push_str(".*");
                }
            }
            '*' => {
                re.push_str("[^/]*");
                i += 1;
            }
            '?' => {
                re.push_str("[^/]");
                i += 1;
            }
            c => {
                re.push_str(&regex::escape(&c.to_string()));
                i += 1;
            }
        }
    }
    re.push('$');
    regex::Regex::new(&re).unwrap_or_else(|_| regex::Regex::new("^$").unwrap())
}

/// A tree with a `target/` in it is otherwise a quarter of a million files nobody asked for.
pub fn walk_dir(
    dir: &Path,
    base: &Path,
    exclude: &[regex::Regex],
    out: &mut Vec<PathBuf>,
    cap: usize,
) {
    if out.len() >= cap {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut items: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    items.sort();
    for p in items {
        let name = p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        // A checkout carries its own history, and none of it is the source's content.
        if name.starts_with('.') {
            continue;
        }
        let rel = p.strip_prefix(base).unwrap_or(&p).display().to_string();
        if p.is_dir() {
            if exclude.iter().any(|r| r.is_match(&format!("{rel}/x"))) {
                continue;
            }
            walk_dir(&p, base, exclude, out, cap);
        } else {
            if exclude.iter().any(|r| r.is_match(&rel)) {
                continue;
            }
            out.push(p);
        }
        if out.len() >= cap {
            return;
        }
    }
}

const TEXTUAL: [&str; 24] = [
    "md", "markdown", "txt", "text", "rst", "adoc", "org", "csv", "tsv", "json", "toml", "yaml",
    "yml", "rs", "py", "rb", "go", "js", "ts", "c", "h", "sh", "sql", "log",
];

/// Strips tags, keeping the words between them, and drops what a reader never sees.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut depth_skip = 0usize;
    let bytes: Vec<char> = html.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '<' {
            let start = i;
            while i < bytes.len() && bytes[i] != '>' {
                i += 1;
            }
            let tag: String = bytes[start + 1..i.min(bytes.len())].iter().collect();
            let lower = tag.trim_start_matches('/').to_ascii_lowercase();
            let name: &str = lower.split_whitespace().next().unwrap_or("");
            if matches!(name, "script" | "style") {
                if tag.starts_with('/') {
                    depth_skip = depth_skip.saturating_sub(1);
                } else {
                    depth_skip += 1;
                }
            }
            if matches!(
                name,
                "p" | "br" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4"
            ) {
                out.push('\n');
            }
            i += 1;
            continue;
        }
        if depth_skip == 0 {
            out.push(bytes[i]);
        }
        i += 1;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What a file says. `None` where nothing here can read it, which the run counts and reports.
pub fn extract(path: &Path) -> Option<String> {
    let ext = path
        .extension()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let bytes = std::fs::read(path).ok()?;
    if matches!(ext.as_str(), "html" | "htm" | "xhtml") {
        return Some(html_to_text(&String::from_utf8_lossy(&bytes)));
    }
    if TEXTUAL.contains(&ext.as_str()) {
        return Some(String::from_utf8_lossy(&bytes).into_owned());
    }
    // A file with no extension is text when it reads as text.
    if ext.is_empty() && std::str::from_utf8(&bytes).is_ok() {
        return Some(String::from_utf8_lossy(&bytes).into_owned());
    }
    None
}

fn media_type(path: &Path) -> String {
    let ext = path
        .extension()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "md" | "markdown" => "text/markdown",
        "txt" | "text" | "log" => "text/plain",
        "html" | "htm" | "xhtml" => "text/html",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "json" => "application/json",
        "toml" => "application/toml",
        "yaml" | "yml" => "application/yaml",
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "" => "application/octet-stream",
        other => return format!("application/{other}"),
    }
    .to_string()
}

fn modified(path: &Path) -> String {
    let stamp = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    crate::iso_date(stamp as i64)
}

fn file_info(path: &Path, root: &Path) -> FileInfo {
    FileInfo {
        rel: path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string(),
        modified: modified(path),
        size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        media_type: media_type(path),
        path: path.to_path_buf(),
    }
}

fn cell_to_json(d: &calamine::Data) -> J {
    use calamine::Data;
    match d {
        Data::Empty => J::Null,
        Data::String(s) => J::String(s.clone()),
        Data::Float(f) => serde_json::Number::from_f64(*f)
            .map(J::Number)
            .unwrap_or(J::Null),
        Data::Int(i) => J::Number((*i).into()),
        Data::Bool(b) => J::Bool(*b),
        Data::DateTime(dt) => J::String(dt.to_string()),
        Data::DateTimeIso(s) => J::String(s.clone()),
        Data::DurationIso(s) => J::String(s.clone()),
        Data::Error(e) => J::String(format!("{e:?}")),
    }
}

/// Every row the source hands over, in order. The callback keeps the build streaming: a 200 MB
/// export is read without being held.
pub fn each_row(
    decl: &SourceDecl,
    base: &Path,
    root: &Path,
    mark: Option<String>,
    mut on_row: impl FnMut(Produced) -> Result<(), String>,
) -> Result<Option<String>, String> {
    match &decl.source {
        Fetch::Folder {
            include, exclude, ..
        } => {
            let dir = decl.source.root(base);
            let inc: Vec<_> = include.iter().map(|p| glob_to_regex(p)).collect();
            let exc: Vec<_> = exclude.iter().map(|p| glob_to_regex(p)).collect();
            let mut files = Vec::new();
            walk_dir(&dir, &dir, &exc, &mut files, 2_000_000);
            for f in files {
                let rel = f.strip_prefix(&dir).unwrap_or(&f).display().to_string();
                // A source pointed at its own directory would otherwise read its own store.
                if matches!(rel.as_str(), "source.yaml" | "updates.jsonl")
                    || rel.starts_with("claims.db")
                    || rel.starts_with("blobs/")
                {
                    continue;
                }
                if !inc.is_empty() && !inc.iter().any(|r| r.is_match(&rel)) {
                    continue;
                }
                let info = file_info(&f, &dir);
                let text = extract(&f).unwrap_or_default();
                // A .json file is structured, and its contents are reachable by `field:`.
                let value = if f.extension().map(|e| e == "json").unwrap_or(false) {
                    serde_json::from_str(&text).unwrap_or(J::Null)
                } else {
                    J::Null
                };
                let mut meta = BTreeMap::new();
                meta.insert("file".into(), info.rel.clone());
                let origin = Origin {
                    file: Some(info.rel.clone()),
                    ..Origin::default()
                };
                on_row(Produced {
                    expanded: false,
                    row: Row {
                        value,
                        meta,
                        file: Some(info),
                        text,
                        root,
                    },
                    origin,
                })?;
            }
            Ok(None)
        }
        Fetch::Csv {
            path,
            delimiter,
            skip,
            columns,
            blank,
            paths,
            headers: asked_with,
        } => {
            // One file or several of the same kind: Eurostat keeps each indicator in a dataset of
            // its own, and a source of three indicators reads three.
            for (k, path) in std::iter::once(path).chain(paths.iter()).filter(|p| !p.is_empty()).enumerate() {
                // `local or at a URL`. A URL is fetched once into the source directory, so the
                // extraction reads a file either way.
                let file = if path.starts_with("http://") || path.starts_with("https://") {
                    let f = crate::fetch::Fetcher::new(crate::sourcedecl::AGENT, asked_with, 0)?.whole_file();
                    let body = f.get(path)?;
                    let cached = base.join(if k == 0 { "source.csv".to_string() } else { format!("source-{}.csv", k + 1) });
                    std::fs::write(&cached, body).map_err(|e| format!("{}: {e}", cached.display()))?;
                    cached
                } else {
                    base.join(path)
                };
                let info = file_info(&file, root);
                let delim = delimiter.as_bytes().first().copied().unwrap_or(b',');
                // `skip` is the lines before the header: a report date, a title, a licence line.
                let whole = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
                let mut body: &[u8] = &whole;
                for _ in 0..*skip {
                    match body.iter().position(|b| *b == b'\n') {
                        Some(i) => body = &body[i + 1..],
                        None => body = &[],
                    }
                }
                // The DOS end of file some lists still end with (OFAC's do) is no row.
                if let Some(rest) = body.strip_suffix(b"\x1a") {
                    body = rest;
                }
                let mut rdr = csv::ReaderBuilder::new()
                    .delimiter(delim)
                    .flexible(true)
                    .has_headers(columns.is_empty())
                    .from_reader(body);
                let headers = if columns.is_empty() {
                    rdr.headers()
                        .map_err(|e| format!("{}: {e}", file.display()))?
                        .clone()
                } else {
                    csv::StringRecord::from(columns.clone())
                };
                for (n, result) in rdr.records().enumerate() {
                    // The line in the file: after the lines skipped, and the header where there is one.
                    let line = n + *skip + if columns.is_empty() { 2 } else { 1 };
                    let rec = result.map_err(|e| format!("{}: row {line}: {e}", file.display()))?;
                    let mut o = Map::new();
                    for (i, h) in headers.iter().enumerate() {
                        o.insert(
                            h.to_string(),
                            J::String(match rec.get(i).unwrap_or("") {
                                v if blank.as_deref().is_some_and(|b| v.trim() == b) => String::new(),
                                v => v.to_string(),
                            }),
                        );
                    }
                    let text = rec.iter().collect::<Vec<_>>().join(" ");
                    let mut meta = BTreeMap::new();
                    meta.insert("file".into(), info.rel.clone());
                    meta.insert("row".into(), line.to_string());
                    let origin = Origin {
                        file: Some(info.rel.clone()),
                        row: Some(line as u64),
                        ..Origin::default()
                    };
                    on_row(Produced {
                        expanded: false,
                        row: Row {
                            value: J::Object(o),
                            meta,
                            file: Some(file_info(&file, root)),
                            text,
                            root,
                        },
                        origin,
                    })?;
                }
            }
            Ok(None)
        }
        Fetch::Xlsx {
            path,
            sheets,
            header_row,
        } => {
            use calamine::Reader;
            let file = base.join(path);
            let info = file_info(&file, root);
            let mut wb = calamine::open_workbook_auto(&file)
                .map_err(|e| format!("{}: {e}", file.display()))?;
            let names: Vec<String> = if sheets.is_empty() {
                wb.sheet_names().to_vec()
            } else {
                sheets.clone()
            };
            for sheet in names {
                let range = wb
                    .worksheet_range(&sheet)
                    .map_err(|e| format!("{}: sheet {sheet}: {e}", file.display()))?;
                let rows: Vec<_> = range.rows().collect();
                let head_at = header_row.saturating_sub(1);
                let Some(head) = rows.get(head_at) else {
                    continue;
                };
                let headers: Vec<String> = head
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        let n = crate::expr::as_string(&cell_to_json(c));
                        if n.trim().is_empty() {
                            format!("column{}", i + 1)
                        } else {
                            n
                        }
                    })
                    .collect();
                for (n, row) in rows.iter().enumerate().skip(head_at + 1) {
                    if row.iter().all(|c| matches!(c, calamine::Data::Empty)) {
                        continue;
                    }
                    let mut o = Map::new();
                    for (i, h) in headers.iter().enumerate() {
                        o.insert(h.clone(), row.get(i).map(cell_to_json).unwrap_or(J::Null));
                    }
                    let text = row
                        .iter()
                        .map(|c| crate::expr::as_string(&cell_to_json(c)))
                        .collect::<Vec<_>>()
                        .join(" ");
                    let mut meta = BTreeMap::new();
                    meta.insert("sheet".into(), sheet.clone());
                    meta.insert("file".into(), info.rel.clone());
                    meta.insert("row".into(), (n + 1).to_string());
                    let origin = Origin {
                        file: Some(format!("{}#{sheet}", info.rel)),
                        row: Some(n as u64 + 1),
                        ..Origin::default()
                    };
                    on_row(Produced {
                        expanded: false,
                        row: Row {
                            value: J::Object(o),
                            meta,
                            file: Some(file_info(&file, root)),
                            text,
                            root,
                        },
                        origin,
                    })?;
                }
            }
            Ok(None)
        }
        Fetch::Http {
            list,
            for_each,
            detail,
            page,
            window,
            headers,
            user_agent,
            pause_ms,
            since,
            since_default,
            limit,
            top,
        } => {
            let f = crate::fetch::Fetcher::new(user_agent, headers, *pause_ms)?;
            // Subjects from another source, one detail call each. The list loop never
            // runs: there is no list, only names somebody else already holds.
            if let Some(each) = for_each {
                let mut declined = 0u64;
                let template = detail
                    .as_deref()
                    .ok_or("a source that follows another needs a detail call")?;
                let root = base
                    .parent()
                    .and_then(|p| p.parent())
                    .ok_or("this source is not inside a workspace")?;
                let values = crate::rows::subjects_of(root, &each.dataset, &each.scheme)?;
                for value in values {
                    let url = template.replace("{value}", &value);
                    let Some(body) = f.get_subject(&url)? else {
                        declined += 1;
                        continue;
                    };
                    let mut item: J =
                        serde_json::from_str(&body).map_err(|e| format!("{url}: not JSON: {e}"))?;
                    if let Some(o) = item.as_object_mut() {
                        o.insert("_asked".into(), J::String(value.clone()));
                    }
                    let mut meta = BTreeMap::new();
                    meta.insert("url".into(), url.clone());
                    meta.insert("asked".into(), value);
                    on_row(Produced {
                        expanded: true,
                        row: Row {
                            value: item,
                            meta,
                            file: None,
                            text: String::new(),
                            root,
                        },
                        origin: Origin {
                            url: Some(url),
                            ..Origin::default()
                        },
                    })?;
                }
                if declined > 0 {
                    println!("  {declined} things the source would not answer for");
                }
                return Ok(None);
            }
            let spec = crate::fetch::Http {
                list: &crate::fetch::resolve(list)?.unwrap_or_default(),
                detail: detail.as_deref(),
                page: page.as_ref(),
                window: window.as_deref(),
                since: since.as_deref(),
                since_default,
                limit: if *limit > 0 { *limit } else { *top },
                each: decl.records.each.as_deref(),
            };
            // What is kept about each row says where it was read as the declaration writes it:
            // `apikey=${KEY}`, never the key, since a receipt is published with its claim.
            let secrets = crate::fetch::secrets(list);
            let mut redacted = |mut p: Produced| -> Result<(), String> {
                if let Some(u) = p.origin.url.as_mut() {
                    *u = crate::fetch::redact(u, &secrets);
                }
                for v in p.row.meta.values_mut() {
                    *v = crate::fetch::redact(v, &secrets);
                }
                on_row(p)
            };
            let high = crate::fetch::http_rows(&f, &spec, mark, root, &mut redacted).map_err(|e| crate::fetch::redact(&e, &secrets))?;
            Ok(high)
        }
        Fetch::Webhook { .. } => crate::hook::rows(base, root, "webhook", &mut on_row),
        Fetch::Proposals { .. } => crate::hook::rows(base, root, "proposal", &mut on_row),
        Fetch::Web { url, items, fields, page, top, limit, since, since_default, user_agent, pause_ms } => {
            let spec = crate::web::Spec { url, items, fields, page: page.as_ref(), top: *top, limit: *limit, since: since.as_deref(), since_default, user_agent, pause_ms: *pause_ms };
            crate::web::rows(&spec, mark, base, root, &mut on_row)
        }
        Fetch::Sql { dsn, query, since, since_default } => {
            let spec = crate::sql::Spec { dsn, query, since: since.as_deref(), since_default };
            crate::sql::rows(&spec, mark, root, &mut on_row)
        }
        Fetch::Feed {
            urls,
            text_is,
            user_agent,
            pause_ms,
        } => {
            let f = crate::fetch::Fetcher::new(user_agent, &BTreeMap::new(), *pause_ms)?;
            crate::fetch::feed_rows(&f, urls, text_is, root, &mut on_row)?;
            Ok(None)
        }
        // Nothing to read here. A subscribed source holds claims somebody else produced, and
        // asking its hub for newer ones is a different command.
        Fetch::Hub { reference, .. } => Err(format!(
            "{reference} is subscribed. `zetlyn source pull` asks its hub for a newer version"
        )),
        Fetch::Package { tracker, .. } => Err(format!(
            "it came in the package {tracker}. `zetlyn tracker pull` takes a newer version"
        )),
    }
}

/// Every identifier of a scheme another source holds, through the same `search` a reader uses.
pub fn subjects_of(root: &Path, dataset: &str, scheme: &str) -> Result<Vec<String>, String> {
    let dir = crate::tracker::registry(&root.join("sources"))
        .get(dataset)
        .cloned()
        .ok_or_else(|| format!("{dataset} is not installed here"))?;
    let ds = crate::source::Source::open(&dir)?;
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    let mut offset = 0usize;
    loop {
        let q = crate::source::Query {
            limit: 5000,
            offset,
            ..Default::default()
        };
        let (_, hits, _) = crate::source::Interface::search(&ds, &q)?;
        if hits.is_empty() {
            break;
        }
        for hit in &hits {
            for id in hit.ids.iter().filter(|i| i.scheme == scheme) {
                seen.entry(id.value.to_lowercase())
                    .or_insert_with(|| id.value.clone());
            }
        }
        offset += hits.len();
        if hits.len() < 5000 {
            break;
        }
    }
    Ok(seen.into_values().collect())
}
