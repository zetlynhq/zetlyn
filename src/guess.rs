//! `zetlyn source new`. The declaration is proposed rather than demanded: a CSV of forty columns
//! should not need forty lines of configuration before it shows anything.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{json, Value as J};

use crate::build::as_date;
use crate::schemes::Scheme;
use crate::sourcedecl::PropertyType;

const SAMPLE: usize = 500;

struct Column {
    name: String,
    values: Vec<String>,
}

impl Column {
    fn filled(&self) -> Vec<&String> {
        self.values
            .iter()
            .filter(|v| !v.trim().is_empty())
            .collect()
    }
    fn distinct(&self) -> usize {
        self.filled().into_iter().collect::<BTreeSet<_>>().len()
    }
    /// Every row has one and nearly every one is different. Real files repeat a row now and then
    /// (Exploit-DB's does), and a repeated row is the same claim said twice, not a second one.
    fn unique(&self) -> bool {
        let f = self.filled();
        !f.is_empty() && f.len() == self.values.len() && self.distinct() * 100 >= f.len() * 98
    }
    fn mean_len(&self) -> usize {
        let f = self.filled();
        if f.is_empty() {
            return 0;
        }
        f.iter().map(|v| v.chars().count()).sum::<usize>() / f.len()
    }
    fn kind(&self) -> PropertyType {
        let f = self.filled();
        if f.is_empty() {
            return PropertyType::Text;
        }
        let bools = ["true", "false", "yes", "no", "y", "n"];
        if f.iter()
            .all(|v| bools.contains(&v.trim().to_ascii_lowercase().as_str()))
        {
            return PropertyType::Bool;
        }
        if f.iter()
            .all(|v| v.trim().replace(',', ".").parse::<f64>().is_ok())
        {
            return PropertyType::Number;
        }
        if f.iter().all(|v| as_date(v).is_some()) {
            return PropertyType::Date;
        }
        let d = self.distinct();
        if d <= 24 && d * 5 <= f.len().max(5) {
            return PropertyType::Code;
        }
        PropertyType::Text
    }
}

fn hints(name: &str, words: &[&str]) -> bool {
    let n = name.to_ascii_lowercase();
    words.iter().any(|w| n.contains(w))
}

fn columns_from_rows(headers: &[String], rows: &[Vec<String>]) -> Vec<Column> {
    headers
        .iter()
        .enumerate()
        .map(|(i, name)| Column {
            name: name.clone(),
            values: rows
                .iter()
                .map(|r| r.get(i).cloned().unwrap_or_default())
                .collect(),
        })
        .collect()
}

fn read_csv(path: &Path) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let headers: Vec<String> = rdr
        .headers()
        .map_err(|e| e.to_string())?
        .iter()
        .map(str::to_string)
        .collect();
    let mut rows = Vec::new();
    for r in rdr.records().take(SAMPLE) {
        let r = r.map_err(|e| e.to_string())?;
        rows.push(r.iter().map(str::to_string).collect());
    }
    Ok((headers, rows))
}

/// Spreadsheets carry a title and a logo above the header. The header is the first row where most
/// cells are text and the row below it has data.
/// The sheet, the row the header sat on, the header, and the rows under it.
type Sheet = (String, usize, Vec<String>, Vec<Vec<String>>);

fn read_xlsx(path: &Path) -> Result<Sheet, String> {
    use calamine::Reader;
    let mut wb =
        calamine::open_workbook_auto(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let sheet = wb
        .sheet_names()
        .first()
        .cloned()
        .ok_or("the workbook has no sheet")?;
    let range = wb.worksheet_range(&sheet).map_err(|e| e.to_string())?;
    let rows: Vec<Vec<String>> = range
        .rows()
        .map(|r| r.iter().map(|c| crate::expr::as_string(&cell(c))).collect())
        .collect();
    let mut head_at = 0;
    for (i, row) in rows.iter().enumerate().take(20) {
        let filled = row.iter().filter(|c| !c.trim().is_empty()).count();
        let below = rows
            .get(i + 1)
            .map(|r| r.iter().filter(|c| !c.trim().is_empty()).count());
        if filled >= 2 && below.unwrap_or(0) >= filled.saturating_sub(1) {
            head_at = i;
            break;
        }
    }
    let headers: Vec<String> = rows
        .get(head_at)
        .map(|r| {
            r.iter()
                .enumerate()
                .map(|(i, c)| {
                    if c.trim().is_empty() {
                        format!("column{}", i + 1)
                    } else {
                        c.clone()
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let body: Vec<Vec<String>> = rows
        .into_iter()
        .skip(head_at + 1)
        .filter(|r| r.iter().any(|c| !c.trim().is_empty()))
        .take(SAMPLE)
        .collect();
    Ok((sheet, head_at + 1, headers, body))
}

fn cell(d: &calamine::Data) -> J {
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

struct Shape {
    id: Option<String>,
    /// The scheme the id column is, where the library knows it.
    id_scheme: Option<&'static Scheme>,
    /// Every other column that carries a known identifier.
    also: Vec<Found>,
    title: String,
    known: Option<String>,
    text: Vec<String>,
    fields: Vec<(String, PropertyType)>,
}

/// An identifier a column carries, by a scheme the library knows.
pub struct Found {
    pub column: String,
    pub scheme: &'static Scheme,
    pub separator: Option<&'static str>,
    /// Some row carries more than one.
    pub several: bool,
    /// Of the rows with anything in this column, how many carry one.
    pub share: f64,
}

const SEPARATORS: [Option<&str>; 5] = [None, Some(";"), Some(","), Some("|"), Some(" ")];

/// Every known identifier in the sample, the best reading of each column first. A scheme that
/// rests on a check digit needs most of a column: one number in ten passes a mod-10 check by
/// chance, and a column of prices is not a column of ISBNs.
fn identifiers(cols: &[Column]) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    for c in cols {
        let filled = c.filled();
        if filled.len() < 3 {
            continue;
        }
        let mut best: Option<Found> = None;
        for scheme in crate::schemes::ALL {
            for sep in SEPARATORS {
                let (mut hit, mut several) = (0usize, false);
                for v in &filled {
                    let n = match sep {
                        None => usize::from(scheme.is(v)),
                        Some(s) => v.split(s).filter(|t| scheme.is(t)).count(),
                    };
                    hit += usize::from(n > 0);
                    several |= n > 1;
                }
                let share = hit as f64 / filled.len() as f64;
                let enough = if scheme.checked() { 0.5 } else { 0.02 };
                if hit < 3 || share < enough {
                    continue;
                }
                // A separator earns its place only by finding more.
                if best.as_ref().is_none_or(|b| share > b.share + 1e-9) {
                    best = Some(Found { column: c.name.clone(), scheme, separator: sep, several, share });
                }
            }
        }
        out.extend(best);
    }
    out.sort_by(|a, b| b.share.total_cmp(&a.share));
    out
}


fn shape(cols: &[Column]) -> Shape {
    let found = identifiers(cols);
    // A column that is wholly one known scheme is the identifier before one merely called `id`:
    // a CVE column is what a second source can meet this one on.
    let whole = |c: &Column| {
        found
            .iter()
            .find(|f| f.column == c.name && f.separator.is_none() && !f.several && f.share >= 0.9)
            .map(|f| f.scheme)
    };
    let id = cols
        .iter()
        .filter(|c| c.unique() && c.mean_len() <= 64)
        .min_by_key(|c| {
            let named = hints(
                &c.name,
                &["id", "key", "number", "code", "ref", "isbn", "sku"],
            );
            (whole(c).is_none(), !named)
        })
        .map(|c| c.name.clone());
    let id_scheme = cols.iter().find(|c| Some(&c.name) == id.as_ref()).and_then(whole);

    let title = cols
        .iter()
        .filter(|c| matches!(c.kind(), PropertyType::Text) && c.mean_len() >= 4)
        .min_by_key(|c| {
            let named = hints(
                &c.name,
                &[
                    "title",
                    "name",
                    "subject",
                    "description",
                    "summary",
                    "label",
                ],
            );
            let length = (c.mean_len() as i64 - 48).abs();
            (if named { 0 } else { 1 }, length)
        })
        .or_else(|| cols.iter().find(|c| c.mean_len() > 0))
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "column1".into());

    let known = cols
        .iter()
        .filter(|c| c.kind() == PropertyType::Date)
        .min_by_key(|c| {
            if hints(&c.name, &["publish", "date", "created", "known", "issued"]) {
                0
            } else {
                1
            }
        })
        .map(|c| c.name.clone());

    // Everything a reader would read, which is the title and every long column.
    let mut text: Vec<String> = vec![title.clone()];
    for c in cols {
        if c.name != title && c.mean_len() >= 60 {
            text.push(c.name.clone());
        }
    }

    // Identifiers in prose are mentions, not what a row is about: "see also CVE-…" in a
    // description does not make the row a claim about that CVE.
    let also: Vec<Found> = found
        .into_iter()
        .filter(|f| Some(&f.column) != id.as_ref() && f.column != title && !text.contains(&f.column))
        .filter(|f| !f.scheme.classifies())
        .filter(|f| Some(f.scheme.name) != id_scheme.map(|s| s.name) || f.separator.is_some())
        .collect();
    let fields = cols
        .iter()
        .filter(|c| {
            Some(&c.name) != id.as_ref() && c.name != title && Some(&c.name) != known.as_ref()
        })
        .filter(|c| !text.contains(&c.name))
        .filter(|c| !also.iter().any(|f| f.column == c.name))
        .filter(|c| !c.filled().is_empty())
        .map(|c| (c.name.clone(), c.kind()))
        .collect();

    Shape {
        id,
        id_scheme,
        also,
        title,
        known,
        text,
        fields,
    }
}

fn examples(cols: &[Column], sh: &Shape) -> Vec<String> {
    let mut out = Vec::new();
    // A real word from a real title, so the box is answerable before anything is read.
    if let Some(c) = cols.iter().find(|c| c.name == sh.title) {
        if let Some(word) = c
            .filled()
            .iter()
            .flat_map(|v| v.split_whitespace())
            .find(|w| w.chars().count() > 4 && w.chars().all(|ch| ch.is_alphanumeric()))
        {
            out.push(word.to_lowercase());
        }
    }
    for (name, kind) in &sh.fields {
        if *kind != PropertyType::Code {
            continue;
        }
        if let Some(c) = cols.iter().find(|c| &c.name == name) {
            let mut tally: BTreeMap<&String, usize> = BTreeMap::new();
            for v in c.filled() {
                *tally.entry(v).or_default() += 1;
            }
            if let Some((v, _)) = tally.into_iter().max_by_key(|(_, n)| *n) {
                out.push(format!("{}={v}", slug(name)));
                break;
            }
        }
    }
    if let (Some(id), Some(c)) = (
        &sh.id,
        cols.iter().find(|c| Some(&c.name) == sh.id.as_ref()),
    ) {
        let _ = id;
        if let Some(v) = c.filled().first() {
            out.push((*v).clone());
        }
    }
    out
}

/// What a proposal becomes: read back as a declaration before it is written, so a proposal is a
/// file this program opens, and written by the same code as every other declaration.
fn finish(built: J, dir: &Path) -> Result<String, String> {
    let decl: crate::sourcedecl::SourceDecl = serde_json::from_value(built)
        .map_err(|e| format!("the proposal does not make a declaration: {e}"))?;
    let text = crate::yaml::to_string(&decl)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(crate::sourcedecl::FILE);
    std::fs::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(text)
}

fn write_blocks(name: &str, kind: &str, fetch: J, cols: &[Column], sh: &Shape) -> J {
    let about = format!("Read from {}.", source_name(&fetch));
    let mut claims = serde_json::Map::new();
    let mut ids: Vec<J> = Vec::new();
    if let Some(id) = &sh.id {
        // A column called `id` names this source's rows and nobody else's, and two sources that
        // both said `id` would meet on numbers that mean nothing to each other.
        let scheme = sh.id_scheme.map(|s| s.name.to_string()).unwrap_or_else(|| {
            match scheme_name(id).as_str() {
                "" | "id" | "key" | "number" | "no" | "code" | "ref" | "row" => {
                    slug(name.rsplit('/').next().unwrap_or(name))
                }
                s => s.to_string(),
            }
        });
        ids.push(json!({ "scheme": scheme, "from": format!("field:{id}") }));
    }
    // Each further identifier is a way a second source can meet this one, so each is declared,
    // and only the values that are that scheme are taken from the column.
    for f in &sh.also {
        let mut one = json!({
            "scheme": f.scheme.name,
            "from": format!("field:{}", f.column),
            "match": f.scheme.declared(),
        });
        if let Some(sep) = f.separator {
            one["separator"] = json!(sep);
        }
        if f.several {
            one["all"] = json!(true);
        }
        ids.push(one);
    }
    match ids.len() {
        0 => {}
        1 => {
            claims.insert("id".into(), ids.remove(0));
        }
        _ => {
            claims.insert("id".into(), J::Array(ids));
        }
    }
    claims.insert("title".into(), json!(format!("field:{}", sh.title)));
    claims.insert(
        "text".into(),
        json!(sh.text.iter().map(|c| format!("field:{c}")).collect::<Vec<_>>()),
    );
    claims.insert(
        "known".into(),
        json!(match &sh.known {
            Some(k) => format!("field:{k}"),
            None => "file:modified".into(),
        }),
    );
    let mut properties = serde_json::Map::new();
    for (n, kind) in &sh.fields {
        properties.insert(
            slug(n),
            json!({ "type": kind.name(), "from": format!("field:{n}") }),
        );
    }
    claims.insert("properties".into(), J::Object(properties));

    // One default view, plus one per code field, because that is what a person clicks first.
    let mut columns: Vec<String> = Vec::new();
    for (n, k) in &sh.fields {
        if matches!(
            k,
            PropertyType::Code | PropertyType::Number | PropertyType::Bool | PropertyType::Date
        ) && columns.len() < 4
        {
            columns.push(slug(n));
        }
    }
    if sh.known.is_some() {
        columns.push("known".into());
    }
    let facets: Vec<String> = sh
        .fields
        .iter()
        .filter(|(_, k)| matches!(k, PropertyType::Code | PropertyType::Bool))
        .map(|(n, _)| slug(n))
        .take(4)
        .collect();
    let mut views = vec![json!({
        "name": "recent", "title": "Newest first", "default": true,
        "columns": columns, "facets": facets, "sort": "known desc",
    })];
    if let Some((n, _)) = sh.fields.iter().find(|(_, k)| *k == PropertyType::Code) {
        views.push(json!({
            "name": format!("by-{}", slug(n)), "title": format!("By {}", slug(n)),
            "group": slug(n), "columns": ["title", "known"],
        }));
    }

    let compare: Vec<String> = sh
        .fields
        .iter()
        .filter(|(_, k)| k.ordered())
        .map(|(n, _)| slug(n))
        .chain(std::iter::once("known".to_string()))
        .collect();
    let suggest: Vec<String> = sh
        .fields
        .iter()
        .filter(|(_, k)| *k == PropertyType::Code)
        .map(|(n, _)| slug(n))
        .take(3)
        .collect();
    json!({
        "name": name,
        "title": title_case(name),
        "kind": kind,
        "about": about,
        "fetch": fetch,
        "claims": claims,
        "views": views,
        "search": {
            "text": ["title", "text"],
            "compare": compare,
            "suggest": suggest,
            "examples": examples(cols, sh),
        },
    })
}

/// The file a fetch block names, for the one sentence a proposal can honestly write.
fn source_name(fetch: &J) -> String {
    match fetch["path"].as_str() {
        Some(path) => path.rsplit('/').next().unwrap_or(path).to_string(),
        None => "the source".into(),
    }
}
/// A column name as a field name. `knownRansomwareCampaignUse` is four words and reads as four.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut last_sep = true;
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_alphanumeric() {
            let boundary = c.is_ascii_uppercase()
                && i > 0
                && (chars[i - 1].is_ascii_lowercase()
                    || chars[i - 1].is_ascii_digit()
                    || (chars[i - 1].is_ascii_uppercase()
                        && chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase())));
            if boundary && !last_sep {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
            last_sep = false;
        } else if !last_sep {
            out.push('_');
            last_sep = true;
        }
    }
    out.trim_matches('_').to_string()
}

/// A column called `cveID` issues a `cve`, not a `cveid`.
fn scheme_name(column: &str) -> String {
    let s = slug(column);
    for tail in ["_id", "_key", "_number", "_no", "_code"] {
        if let Some(stem) = s.strip_suffix(tail) {
            if !stem.is_empty() {
                return stem.to_string();
            }
        }
    }
    s
}

fn title_case(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    let mut c = base.replace(['_', '-'], " ");
    if let Some(first) = c.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    c
}

/// Reads the source, writes `source.yaml`, and hands back what it wrote.
pub fn propose(
    from: &Path,
    dir: &Path,
    name: Option<&str>,
    kind: Option<&str>,
) -> Result<String, String> {
    let from = from
        .canonicalize()
        .map_err(|e| format!("{}: {e}", from.display()))?;
    let stem = from
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "source".into());
    let name = name
        .map(str::to_string)
        .unwrap_or_else(|| format!("local/{}", slug(&stem)));

    // A path inside the source directory travels with it; anything else is where it is.
    let shown = match from.strip_prefix(dir.canonicalize().unwrap_or(dir.to_path_buf())) {
        Ok(rel) => format!("./{}", rel.display()),
        Err(_) => from.display().to_string(),
    };

    let ext = from
        .extension()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let built = if from.is_dir() {
        let mut files = Vec::new();
        let exc: Vec<_> = BUILD_OUTPUT
            .iter()
            .map(|p| crate::rows::glob_to_regex(p))
            .collect();
        crate::rows::walk_dir(&from, &from, &exc, &mut files, 200_000);
        folder_declaration(&name, kind.unwrap_or("document"), &shown, files.len())
    } else if ext == "csv" || ext == "tsv" {
        let (headers, rows) = read_csv(&from)?;
        let cols = columns_from_rows(&headers, &rows);
        let sh = shape(&cols);
        let delim = if ext == "tsv" { "\t" } else { "," };
        let fetch = json!({ "type": "csv", "path": shown, "delimiter": delim });
        write_blocks(&name, kind.unwrap_or("row"), fetch, &cols, &sh)
    } else if ext == "xlsx" || ext == "xls" || ext == "xlsm" {
        let (sheet, header_row, headers, rows) = read_xlsx(&from)?;
        let cols = columns_from_rows(&headers, &rows);
        let sh = shape(&cols);
        let fetch = json!({ "type": "xlsx", "path": shown, "sheets": [sheet],
                            "header_row": header_row });
        write_blocks(&name, kind.unwrap_or("row"), fetch, &cols, &sh)
    } else {
        return Err(format!(
            "{}: a source is proposed from a folder, a .csv, a .tsv or an .xlsx",
            from.display()
        ));
    };
    finish(built, dir)
}

/// What a build leaves behind, which nobody points a source at on purpose. Proposed as `exclude`
/// so a creator can see it and take it out, rather than hidden in the walk.
const BUILD_OUTPUT: [&str; 8] = [
    "**/target/**",
    "**/node_modules/**",
    "**/dist/**",
    "**/build/**",
    "**/vendor/**",
    "**/.venv/**",
    "**/venv/**",
    "**/__pycache__/**",
];

fn folder_declaration(name: &str, kind: &str, path: &str, files: usize) -> J {
    json!({
        "name": name,
        "title": title_case(name),
        "kind": kind,
        "about": format!("{files} files, excluding what a build left behind."),
        "fetch": { "type": "folder", "path": path, "exclude": BUILD_OUTPUT },
        "claims": {
            "title": "file:stem",
            "text": ["file:self"],
            "known": "file:modified",
            "properties": {
                "media_type": { "type": "code", "from": "file:media_type" },
                "bytes": { "type": "number", "from": "file:size" },
            },
        },
        "views": [{
            "name": "recent", "title": "Newest first", "default": true,
            "columns": ["media_type", "bytes", "known"], "facets": ["media_type"],
            "sort": "known desc",
        }],
        "search": {
            "text": ["title", "text"],
            "compare": ["bytes", "known"],
            "suggest": ["media_type"],
        },
    })
}

/// A URL, read once into the scratch of the source directory so the shape can be guessed, and
/// left in the declaration so every update fetches it again. Somebody who has a link should not
/// have to download it first.
pub fn propose_url(
    url: &str,
    dir: &Path,
    name: Option<&str>,
    kind: Option<&str>,
) -> Result<String, String> {
    let f = crate::fetch::Fetcher::new(crate::sourcedecl::AGENT, &BTreeMap::new(), 0)?;
    let body = f.get(url)?;

    // The directory is made once the shape is known. Refusing a JSON API after making it
    // leaves an empty source directory that every later update trips over.
    let looks_like = if body.trim_start().starts_with('<') {
        "feed"
    } else if body.trim_start().starts_with('{') || body.trim_start().starts_with('[') {
        "json"
    } else {
        "csv"
    };
    let stem = url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .map(|s| s.split('?').next().unwrap_or(s))
        .map(|s| s.split('.').next().unwrap_or(s))
        .unwrap_or("source");
    let name = name
        .map(str::to_string)
        .unwrap_or_else(|| format!("local/{}", slug(stem)));

    let built = match looks_like {
        // A feed knows its own shape, so the declaration is the same every time.
        "feed" => feed_declaration(&name, kind.unwrap_or("article"), url, &body),
        "json" => {
            return Err(format!(
                "{url} answers JSON, and a JSON API needs a declaration somebody writes: which \
                 list holds the claims, which field is the identifier, what each field means. \
                 `zetlyn source new` guesses a shape from a table, not from an API"
            ))
        }
        _ => {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let scratch = dir.join("source.csv");
            std::fs::write(&scratch, &body).map_err(|e| format!("{}: {e}", scratch.display()))?;
            let (headers, rows) = read_csv(&scratch)?;
            let cols = columns_from_rows(&headers, &rows);
            let sh = shape(&cols);
            let fetch = json!({ "type": "csv", "path": url });
            write_blocks(&name, kind.unwrap_or("row"), fetch, &cols, &sh)
        }
    };
    finish(built, dir)
}

fn feed_declaration(name: &str, kind: &str, url: &str, body: &str) -> J {
    let mut d = json!({
        "name": name,
        "title": title_case(name),
        "kind": kind,
        "about": format!("Read from {url}."),
        "fetch": { "type": "feed", "urls": [url] },
        "schedule": { "every": "1h" },
        "claims": {
            "title": "meta:title",
            "url": "meta:link",
            "text": ["meta:summary"],
            "known": "meta:published",
            "properties": { "author": { "type": "text", "from": "meta:author" } },
        },
        "views": [{
            "name": "recent", "title": "Newest first", "default": true,
            "columns": ["author", "known"], "facets": ["author"], "sort": "known desc",
        }],
        "search": {
            "text": ["title", "text"],
            "compare": ["known"],
            "suggest": ["author"],
        },
    });
    // A post about a vulnerability names it in its text, not in a field. Where enough items
    // name something the library knows, every mention is an identifier, which is what lets a
    // feed of write-ups meet a list of advisories.
    if let Some(scheme) = mentioned(body) {
        d["claims"]["id"] = json!({
            "scheme": scheme.name,
            "from": format!("text:/{}/", scheme.pattern),
            "all": true,
        });
    }
    d
}

/// The scheme a feed's items mention most, where at least two items and one in ten mention it.
fn mentioned(body: &str) -> Option<&'static Scheme> {
    let items: Vec<&str> = body
        .split("<item")
        .skip(1)
        .chain(body.split("<entry").skip(1))
        .collect();
    if items.is_empty() {
        return None;
    }
    crate::schemes::ALL
        .iter()
        .filter(|s| !s.classifies())
        .map(|s| (s, items.iter().filter(|i| !s.find(i).is_empty()).count()))
        .filter(|(_, n)| *n >= 2 && *n * 10 >= items.len())
        .max_by_key(|(_, n)| *n)
        .map(|(s, _)| s)
}

/// GitHub, by what a person would say: `github:advisories` (the whole Advisory Database),
/// `github:owner/repo/releases`, `github:owner/repo/advisories` (one repository's), or
/// `github:owner/repo` for the files of a checkout. The API's shape is GitHub's, the same for
/// every repository, so it is written here once rather than taught each time. The token is
/// optional and lifts the limit from sixty calls an hour.
pub fn propose_github(spec: &str, dir: &Path, name: Option<&str>) -> Result<String, String> {
    let rest = spec.strip_prefix("github:").unwrap_or(spec).trim_matches('/');
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    let headers = json!({ "Accept": "application/vnd.github+json", "Authorization": "Bearer ${GITHUB_TOKEN?}" });
    let named = |fallback: String| name.map(str::to_string).unwrap_or(fallback);
    let advisory_claims = json!({
        "each": "field:*",
        "id": [{ "scheme": "ghsa", "from": "field:ghsa_id" }, { "scheme": "cve", "from": "field:cve_id" }],
        "title": "field:summary",
        "url": "field:html_url",
        "text": ["field:summary", "field:description"],
        "known": "field:published_at",
        "properties": {
            "severity": { "type": "code", "from": "field:severity" },
            "cvss": { "type": "number", "from": "field:cvss_severities.cvss_v3.score" },
            "cwe": { "type": "code", "from": "field:cwes[].cwe_id" },
            "ecosystem": { "type": "code", "from": "field:vulnerabilities[].package.ecosystem" },
            "package": { "type": "text", "from": "field:vulnerabilities[].package.name" },
            "patched_version": { "type": "text", "from": "field:vulnerabilities[].first_patched_version" },
        },
    });
    let view = |columns: J, facets: J| json!([{ "name": "recent", "title": "Newest first", "default": true,
                                               "columns": columns, "facets": facets, "sort": "known desc" }]);
    let built = match parts.as_slice() {
        ["advisories"] => json!({
            "name": named("github/advisories".into()),
            "title": "GitHub Advisory Database",
            "kind": "advisory",
            "about": "GitHub's reviewed advisories: the packages and versions a vulnerability affects, where it is patched, how severe GitHub judges it.",
            "fetch": { "type": "http", "list": "https://api.github.com/advisories?published={since_date}..{until_date}",
                       "page": { "cursor": "link", "size": "per_page" }, "headers": headers, "pause_ms": 1200,
                       "since": "field:published_at", "since_default": crate::iso_date(crate::now() - 30 * 86_400) },
            "schedule": { "every": "6h" },
            "claims": advisory_claims,
            "views": view(json!(["severity", "cvss", "ecosystem", "known"]), json!(["severity", "ecosystem"])),
            "search": { "text": ["title", "text"], "compare": ["cvss", "known"], "suggest": ["severity", "ecosystem"] },
        }),
        [owner, repo, "advisories"] => json!({
            "name": named(format!("github/{}-advisories", slug(repo).replace('_', "-"))),
            "title": format!("{owner}/{repo} security advisories"),
            "kind": "advisory",
            "about": format!("The security advisories {owner}/{repo} publishes for itself."),
            "fetch": { "type": "http", "list": format!("https://api.github.com/repos/{owner}/{repo}/security-advisories"),
                       "page": { "cursor": "link", "size": "per_page" }, "headers": headers, "pause_ms": 1200 },
            "schedule": { "every": "6h" },
            "claims": advisory_claims,
            "views": view(json!(["severity", "cvss", "known"]), json!(["severity"])),
            "search": { "text": ["title", "text"], "compare": ["cvss", "known"], "suggest": ["severity"] },
        }),
        [owner, repo, "releases"] => json!({
            "name": named(format!("github/{}-releases", slug(repo).replace('_', "-"))),
            "title": format!("{owner}/{repo} releases"),
            "kind": "release",
            "about": format!("Every release {owner}/{repo} publishes on GitHub, with its notes."),
            "fetch": { "type": "http", "list": format!("https://api.github.com/repos/{owner}/{repo}/releases"),
                       "page": { "cursor": "link", "size": "per_page" }, "headers": headers, "pause_ms": 1200 },
            "schedule": { "every": "6h" },
            "claims": {
                "each": "field:*",
                // A release is named by its tag, which is what a changelog, a package and an
                // advisory's patched version all say.
                "id": { "scheme": format!("{}-release", slug(repo).replace('_', "-")), "from": "field:tag_name" },
                "title": "field:name",
                "url": "field:html_url",
                "text": ["field:name", "field:body"],
                "known": "field:published_at",
                "properties": {
                    "tag": { "type": "code", "from": "field:tag_name" },
                    "prerelease": { "type": "bool", "from": "field:prerelease" },
                    "draft": { "type": "bool", "from": "field:draft" },
                    "author": { "type": "code", "from": "field:author.login" },
                },
            },
            "views": view(json!(["tag", "prerelease", "known"]), json!(["prerelease"])),
            "search": { "text": ["title", "text"], "compare": ["known"], "suggest": ["tag"] },
        }),
        [owner, repo] => folder_declaration(
            &named(format!("github/{}", slug(repo).replace('_', "-"))),
            "document",
            "checkout",
            0,
        )
        .as_object()
        .cloned()
        .map(|mut o| {
            o.insert("title".into(), json!(format!("{owner}/{repo}")));
            o.insert("about".into(), json!(format!("The files of {owner}/{repo}, from a checkout pulled before each update.")));
            if let Some(f) = o.get_mut("fetch") {
                f["git"] = json!(format!("https://github.com/{owner}/{repo}.git"));
            }
            J::Object(o)
        })
        .ok_or("the folder declaration is not an object")?,
        _ => {
            return Err(format!(
                "{spec}: github:advisories, github:owner/repo/releases, github:owner/repo/advisories or github:owner/repo"
            ))
        }
    };
    finish(built, dir)
}
