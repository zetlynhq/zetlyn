//! `zetlyn dataset new`. The declaration is proposed rather than demanded: a CSV of forty columns
//! should not need forty lines of configuration before it shows anything.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value as J;

use crate::build::as_date;
use crate::decl::FieldType;

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
    fn unique(&self) -> bool {
        let f = self.filled();
        !f.is_empty() && f.len() == self.values.len() && self.distinct() == f.len()
    }
    fn mean_len(&self) -> usize {
        let f = self.filled();
        if f.is_empty() {
            return 0;
        }
        f.iter().map(|v| v.chars().count()).sum::<usize>() / f.len()
    }
    fn kind(&self) -> FieldType {
        let f = self.filled();
        if f.is_empty() {
            return FieldType::Text;
        }
        let bools = ["true", "false", "yes", "no", "y", "n"];
        if f.iter()
            .all(|v| bools.contains(&v.trim().to_ascii_lowercase().as_str()))
        {
            return FieldType::Bool;
        }
        if f.iter()
            .all(|v| v.trim().replace(',', ".").parse::<f64>().is_ok())
        {
            return FieldType::Number;
        }
        if f.iter().all(|v| as_date(v).is_some()) {
            return FieldType::Date;
        }
        let d = self.distinct();
        if d <= 24 && d * 5 <= f.len().max(5) {
            return FieldType::Code;
        }
        FieldType::Text
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
fn read_xlsx(path: &Path) -> Result<(String, usize, Vec<String>, Vec<Vec<String>>), String> {
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

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

struct Shape {
    id: Option<String>,
    title: String,
    known: Option<String>,
    text: Vec<String>,
    fields: Vec<(String, FieldType)>,
}

fn shape(cols: &[Column]) -> Shape {
    let id = cols
        .iter()
        .filter(|c| c.unique() && c.mean_len() <= 64)
        .min_by_key(|c| {
            if hints(
                &c.name,
                &["id", "key", "number", "code", "ref", "isbn", "sku"],
            ) {
                0
            } else {
                1
            }
        })
        .map(|c| c.name.clone());

    let title = cols
        .iter()
        .filter(|c| matches!(c.kind(), FieldType::Text) && c.mean_len() >= 4)
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
        .filter(|c| c.kind() == FieldType::Date)
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

    let fields = cols
        .iter()
        .filter(|c| {
            Some(&c.name) != id.as_ref() && c.name != title && Some(&c.name) != known.as_ref()
        })
        .filter(|c| !text.contains(&c.name))
        .filter(|c| !c.filled().is_empty())
        .map(|c| (c.name.clone(), c.kind()))
        .collect();

    Shape {
        id,
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
        if *kind != FieldType::Code {
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

fn write_blocks(
    name: &str,
    kind: &str,
    source: &str,
    cols: &[Column],
    sh: &Shape,
    _total_hint: usize,
) -> String {
    let mut t = String::new();
    t.push_str(&format!("name  = {}\n", quote(name)));
    t.push_str(&format!("title = {}\n", quote(&title_case(name))));
    t.push_str(&format!("kind  = {}\n", quote(kind)));
    t.push_str(&format!(
        "about = {}\n\n",
        quote(&format!("Read from {}.", source_name(source)))
    ));
    t.push_str(source);
    t.push_str("\n[records]\n");
    if let Some(id) = &sh.id {
        t.push_str(&format!(
            "id    = {{ scheme = {}, from = {} }}\n",
            quote(&scheme_name(id)),
            quote(&format!("field:{id}"))
        ));
    }
    t.push_str(&format!(
        "title = {}\n",
        quote(&format!("field:{}", sh.title))
    ));
    let text: Vec<String> = sh
        .text
        .iter()
        .map(|c| quote(&format!("field:{c}")))
        .collect();
    t.push_str(&format!("text  = [{}]\n", text.join(", ")));
    match &sh.known {
        Some(k) => t.push_str(&format!("known = {}\n", quote(&format!("field:{k}")))),
        None => t.push_str(&format!("known = {}\n", quote("file:modified"))),
    }

    if !sh.fields.is_empty() {
        t.push_str("\n[records.fields]\n");
        let width = sh
            .fields
            .iter()
            .map(|(n, _)| slug(n).len())
            .max()
            .unwrap_or(4);
        for (n, kind) in &sh.fields {
            t.push_str(&format!(
                "{:width$} = {{ type = {}, from = {} }}\n",
                slug(n),
                quote(kind.name()),
                quote(&format!("field:{n}")),
            ));
        }
    }

    // One default view, plus one per code field, because that is what a person clicks first.
    let mut columns: Vec<String> = Vec::new();
    for (n, k) in &sh.fields {
        if matches!(
            k,
            FieldType::Code | FieldType::Number | FieldType::Bool | FieldType::Date
        ) && columns.len() < 4
        {
            columns.push(slug(n));
        }
    }
    if sh.known.is_some() {
        columns.push("known".into());
    }
    t.push_str("\n[[view]]\nname    = \"recent\"\ntitle   = \"Newest first\"\ndefault = true\n");
    t.push_str(&format!(
        "columns = [{}]\n",
        columns
            .iter()
            .map(|c| quote(c))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    let facets: Vec<String> = sh
        .fields
        .iter()
        .filter(|(_, k)| matches!(k, FieldType::Code | FieldType::Bool))
        .map(|(n, _)| quote(&slug(n)))
        .take(4)
        .collect();
    t.push_str(&format!("facets  = [{}]\n", facets.join(", ")));
    t.push_str("sort    = \"known desc\"\n");

    if let Some((n, _)) = sh.fields.iter().find(|(_, k)| *k == FieldType::Code) {
        t.push_str(&format!(
            "\n[[view]]\nname    = \"by-{0}\"\ntitle   = \"By {0}\"\ngroup   = {1}\ncolumns = [\"title\", \"known\"]\n",
            slug(n),
            quote(&slug(n))
        ));
    }

    let compare: Vec<String> = sh
        .fields
        .iter()
        .filter(|(_, k)| k.ordered())
        .map(|(n, _)| quote(&slug(n)))
        .chain(std::iter::once(quote("known")))
        .collect();
    let suggest: Vec<String> = sh
        .fields
        .iter()
        .filter(|(_, k)| *k == FieldType::Code)
        .map(|(n, _)| quote(&slug(n)))
        .take(3)
        .collect();
    t.push_str("\n[search]\ntext     = [\"title\", \"text\"]\n");
    t.push_str(&format!("compare  = [{}]\n", compare.join(", ")));
    t.push_str(&format!("suggest  = [{}]\n", suggest.join(", ")));
    let ex: Vec<String> = examples(cols, sh).iter().map(|e| quote(e)).collect();
    t.push_str(&format!("examples = [{}]\n", ex.join(", ")));
    t.push_str("\n[retention]\nhistory = false\n");
    t
}

/// The file a source block names, for the one sentence a proposal can honestly write.
fn source_name(block: &str) -> String {
    for line in block.lines() {
        let Some(rest) = line.strip_prefix("path") else {
            continue;
        };
        let Some(start) = rest.find('"') else {
            continue;
        };
        let inner = &rest[start + 1..];
        let Some(end) = inner.rfind('"') else {
            continue;
        };
        let path = &inner[..end];
        return path.rsplit('/').next().unwrap_or(path).to_string();
    }
    "the source".into()
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

/// Reads the source, writes `dataset.toml`, and hands back what it wrote.
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
        .unwrap_or_else(|| "dataset".into());
    let name = name
        .map(str::to_string)
        .unwrap_or_else(|| format!("local/{}", slug(&stem)));

    // A path inside the dataset directory travels with it; anything else is where it is.
    let shown = match from.strip_prefix(dir.canonicalize().unwrap_or(dir.to_path_buf())) {
        Ok(rel) => format!("./{}", rel.display()),
        Err(_) => from.display().to_string(),
    };

    let ext = from
        .extension()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let toml = if from.is_dir() {
        let source = format!("[source]\ntype = \"folder\"\npath = {}\n", quote(&shown));
        let mut files = Vec::new();
        let exc: Vec<_> = BUILD_OUTPUT
            .iter()
            .map(|p| crate::source::glob_to_regex(p))
            .collect();
        crate::source::walk_dir(&from, &from, &exc, &mut files, 200_000);
        folder_declaration(&name, kind.unwrap_or("document"), &source, files.len())
    } else if ext == "csv" || ext == "tsv" {
        let (headers, rows) = read_csv(&from)?;
        let cols = columns_from_rows(&headers, &rows);
        let sh = shape(&cols);
        let delim = if ext == "tsv" { "\\t" } else { "," };
        let source = format!(
            "[source]\ntype      = \"csv\"\npath      = {}\ndelimiter = \"{delim}\"\n",
            quote(&shown)
        );
        write_blocks(
            &name,
            kind.unwrap_or("row"),
            &source,
            &cols,
            &sh,
            rows.len(),
        )
    } else if ext == "xlsx" || ext == "xls" || ext == "xlsm" {
        let (sheet, header_row, headers, rows) = read_xlsx(&from)?;
        let cols = columns_from_rows(&headers, &rows);
        let sh = shape(&cols);
        let source = format!(
            "[source]\ntype       = \"xlsx\"\npath       = {}\nsheets     = [{}]\nheader_row = {header_row}\n",
            quote(&shown),
            quote(&sheet)
        );
        write_blocks(
            &name,
            kind.unwrap_or("row"),
            &source,
            &cols,
            &sh,
            rows.len(),
        )
    } else {
        return Err(format!(
            "{}: a dataset is proposed from a folder, a .csv, a .tsv or an .xlsx",
            from.display()
        ));
    };

    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join("dataset.toml");
    std::fs::write(&path, &toml).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(toml)
}

/// What a build leaves behind, which nobody points a dataset at on purpose. Proposed as `exclude`
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

fn folder_declaration(name: &str, kind: &str, source: &str, files: usize) -> String {
    let exclude = BUILD_OUTPUT
        .iter()
        .map(|p| quote(p))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "name  = {}\ntitle = {}\nkind  = {}\nabout = {}\n\n{source}exclude = [{exclude}]\n\n\
         [records]\n\
         title = \"file:stem\"\n\
         text  = [\"file:self\"]\n\
         known = \"file:modified\"\n\n\
         [records.fields]\n\
         media_type = {{ type = \"code\",   from = \"file:media_type\" }}\n\
         bytes      = {{ type = \"number\", from = \"file:size\" }}\n\n\
         [[view]]\n\
         name    = \"recent\"\n\
         title   = \"Newest first\"\n\
         default = true\n\
         columns = [\"media_type\", \"bytes\", \"known\"]\n\
         facets  = [\"media_type\"]\n\
         sort    = \"known desc\"\n\n\
         [search]\n\
         text     = [\"title\", \"text\"]\n\
         compare  = [\"bytes\", \"known\"]\n\
         suggest  = [\"media_type\"]\n\
         examples = []\n\n\
         [retention]\n\
         history = false\n",
        quote(name),
        quote(&title_case(name)),
        quote(kind),
        quote(&format!(
            "{files} files, excluding what a build left behind."
        )),
    )
}
/// A URL, read once into the scratch of the dataset directory so the shape can be guessed, and
/// left in the declaration so every run fetches it again. Somebody who has a link should not have
/// to download it first.
pub fn propose_url(
    url: &str,
    dir: &Path,
    name: Option<&str>,
    kind: Option<&str>,
) -> Result<String, String> {
    let f = crate::fetch::Fetcher::new("zetlyn/3", &BTreeMap::new(), 0)?;
    let body = f.get(url)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;

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
        .unwrap_or("dataset");
    let name = name
        .map(str::to_string)
        .unwrap_or_else(|| format!("local/{}", slug(stem)));

    let toml = match looks_like {
        // A feed knows its own shape, so the declaration is the same every time.
        "feed" => feed_declaration(&name, kind.unwrap_or("article"), url),
        "json" => {
            return Err(format!(
            "{url} answers JSON, and a JSON API needs a declaration somebody writes: which list \
             holds the records, which field is the identifier, what each field means. \
             `zetlyn dataset new` guesses a shape from a table, not from an API"
        ))
        }
        _ => {
            let scratch = dir.join("source.csv");
            std::fs::write(&scratch, &body).map_err(|e| format!("{}: {e}", scratch.display()))?;
            let (headers, rows) = read_csv(&scratch)?;
            let cols = columns_from_rows(&headers, &rows);
            let sh = shape(&cols);
            let source = format!(
                "[source]\ntype      = \"csv\"\npath      = {}\ndelimiter = \",\"\n",
                quote(url)
            );
            write_blocks(
                &name,
                kind.unwrap_or("row"),
                &source,
                &cols,
                &sh,
                rows.len(),
            )
        }
    };
    let path = dir.join("dataset.toml");
    std::fs::write(&path, &toml).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(toml)
}

fn feed_declaration(name: &str, kind: &str, url: &str) -> String {
    format!(
        "name  = {}\ntitle = {}\nkind  = {}\nabout = {}\n\n\
         [source]\n\
         type       = \"feed\"\n\
         urls       = [{}]\n\
         text_is    = \"summary\"\n\
         user_agent = \"zetlyn/3\"\n\
         pause_ms   = 1000\n\n\
         [schedule]\n\
         every = \"1h\"\n\n\
         [records]\n\
         title = \"meta:title\"\n\
         url   = \"meta:link\"\n\
         text  = [\"meta:summary\"]\n\
         known = \"meta:published\"\n\n\
         [records.fields]\n\
         author = {{ type = \"text\", from = \"meta:author\" }}\n\n\
         [[view]]\n\
         name    = \"recent\"\n\
         title   = \"Newest first\"\n\
         default = true\n\
         columns = [\"author\", \"known\"]\n\
         facets  = [\"author\"]\n\
         sort    = \"known desc\"\n\n\
         [search]\n\
         text     = [\"title\", \"text\"]\n\
         compare  = [\"known\"]\n\
         suggest  = [\"author\"]\n\
         examples = []\n\n\
         [retention]\n\
         history = false\n",
        quote(name),
        quote(&title_case(name)),
        quote(kind),
        quote(&format!("Read from {url}.")),
        quote(url),
    )
}
