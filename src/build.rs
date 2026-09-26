//! Rows to records, per the declaration. `each` first and `where` second.

use std::collections::BTreeMap;

use serde_json::Value as J;

use crate::decl::{Declaration, FieldType, Spec};
use crate::expr::{self, Row};
use crate::record::{Id, Origin, Record, Value};

#[derive(Default)]
pub struct Notes {
    pub no_text: u64,
    pub no_known: u64,
    pub unparsed: u64,
    /// Rows the source gave twice under one key.
    pub duplicates: u64,
    pub examples: Vec<String>,
}

impl Notes {
    fn unparsed(&mut self, field: &str, raw: &str, want: FieldType) {
        self.unparsed += 1;
        if self.examples.len() < 3 {
            self.examples
                .push(format!("{field}: {raw:?} is not a {}", want.name()));
        }
    }
}

/// Every value a spec yields, after `separator`, `match`, `all` and `default`.
fn values(spec: &Spec, row: &Row) -> Vec<String> {
    let mut out: Vec<String> = expr::eval(&spec.from, row)
        .iter()
        .map(expr::as_string)
        .collect();
    if let Some(sep) = &spec.separator {
        out = out
            .into_iter()
            .flat_map(|s| {
                s.split(sep.as_str())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect();
    }
    out = out
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if let Some(pattern) = &spec.matches {
        if let Ok(re) = regex::Regex::new(pattern.trim_matches('/')) {
            out.retain(|s| re.is_match(s));
        }
    }
    if !spec.all {
        out.truncate(1);
    }
    if out.is_empty() {
        if let Some(d) = &spec.default {
            out.push(d.clone());
        }
    }
    out
}

fn one(spec: &Spec, row: &Row) -> Option<String> {
    values(spec, row).into_iter().next()
}

fn plain(from: &str) -> Spec {
    Spec {
        from: from.to_string(),
        matches: None,
        separator: None,
        all: false,
        default: None,
    }
}

/// ISO 8601, or as close as the source gets. A timestamp keeps its date.
pub fn as_date(raw: &str) -> Option<String> {
    let s = raw.trim();
    let bytes = s.as_bytes();
    if bytes.len() >= 10
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[7] == b'-'
    {
        return Some(s[0..10].to_string());
    }
    // 31.12.2026 and 12/31/2026, which is what a spreadsheet hands over.
    let split = |sep: char| -> Option<(u32, u32, i64)> {
        let p: Vec<&str> = s.split(sep).collect();
        if p.len() != 3 {
            return None;
        }
        Some((p[0].parse().ok()?, p[1].parse().ok()?, p[2].parse().ok()?))
    };
    if let Some((d, m, y)) = split('.') {
        if (1..=31).contains(&d) && (1..=12).contains(&m) && y > 1000 {
            return Some(format!("{y:04}-{m:02}-{d:02}"));
        }
    }
    if let Some((m, d, y)) = split('/') {
        if (1..=31).contains(&d) && (1..=12).contains(&m) && y > 1000 {
            return Some(format!("{y:04}-{m:02}-{d:02}"));
        }
    }
    None
}

fn as_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "y" => Some(true),
        "false" | "0" | "no" | "n" | "" => Some(false),
        _ => None,
    }
}

fn typed(kind: FieldType, vocabulary: Option<&str>, raw: &str) -> Option<Value> {
    Some(match kind {
        FieldType::Text => Value::Text(raw.to_string()),
        FieldType::Code => Value::Code {
            code: raw.to_string(),
            vocabulary: vocabulary.map(str::to_string),
        },
        FieldType::Number => Value::Number(raw.trim().replace(',', ".").parse::<f64>().ok()?),
        FieldType::Bool => Value::Bool(as_bool(raw)?),
        FieldType::Date => Value::Date(as_date(raw)?),
        FieldType::Interval => {
            let (a, b) = raw.split_once("..")?;
            Value::Interval {
                from: as_date(a),
                to: if b.trim().is_empty() {
                    None
                } else {
                    as_date(b)
                },
            }
        }
    })
}

/// Where one fetched thing holds many records, the list it holds. Absent, the thing is the record.
///
/// Each sub-row gets an address of its own. Without one, 2,698 Metasploit modules would share the
/// address of the single file they came out of.
pub fn expand<'a>(
    decl: &Declaration,
    row: Row<'a>,
    origin: Origin,
    already: bool,
) -> Vec<(Row<'a>, Origin)> {
    // A source that fetched one page per item has already split it.
    if already {
        return vec![(row, origin)];
    }
    let Some(each) = &decl.records.each else {
        return vec![(row, origin)];
    };

    // `each = "file:index.csv"` is a source that names its own index, and the row is its row.
    if let Some(rest) = each.strip_prefix("file:") {
        if rest.ends_with(".csv") || rest.ends_with(".tsv") {
            return each_csv(&row, rest, &origin);
        }
    }

    // An object keyed by name hands over its keys, which are better addresses than a position.
    let keyed: Option<Vec<(String, J)>> = if each == "field:*" || each == "*" {
        row.value
            .as_object()
            .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
    } else {
        None
    };

    let parts: Vec<(Option<String>, J)> = match keyed {
        Some(entries) => entries.into_iter().map(|(k, v)| (Some(k), v)).collect(),
        None => expr::eval(each, &row)
            .into_iter()
            .map(|v| (None, v))
            .collect(),
    };

    parts
        .into_iter()
        .enumerate()
        .map(|(i, (key, value))| {
            let mut meta = row.meta.clone();
            if let Some(k) = &key {
                meta.insert("key".into(), k.clone());
            }
            let mut sub = origin.clone();
            sub.row = Some(i as u64 + 1);
            if let (Some(k), Some(f)) = (&key, &origin.file) {
                sub.file = Some(format!("{f}#{k}"));
                sub.row = None;
            }
            (
                Row {
                    value,
                    meta,
                    file: None,
                    // Not the container's text: one file holding 7,180 records would be copied
                    // 7,180 times. A record's text is what `records.text` builds from it.
                    text: String::new(),
                    root: row.root,
                },
                sub,
            )
        })
        .collect()
}

fn each_csv<'a>(row: &Row<'a>, rel: &str, origin: &Origin) -> Vec<(Row<'a>, Origin)> {
    let path = row.root.join(rel);
    let Ok(mut rdr) = csv::ReaderBuilder::new().flexible(true).from_path(&path) else {
        return Vec::new();
    };
    let Ok(headers) = rdr.headers().cloned() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (n, rec) in rdr.records().flatten().enumerate() {
        let mut o = serde_json::Map::new();
        for (i, h) in headers.iter().enumerate() {
            o.insert(
                h.to_string(),
                J::String(rec.get(i).unwrap_or("").to_string()),
            );
        }
        let text = rec.iter().collect::<Vec<_>>().join(" ");
        let mut sub = origin.clone();
        sub.file = Some(rel.to_string());
        sub.row = Some(n as u64 + 2);
        out.push((
            Row {
                value: J::Object(o),
                meta: row.meta.clone(),
                file: None,
                text,
                root: row.root,
            },
            sub,
        ));
    }
    out
}

pub fn build(
    decl: &Declaration,
    mut row: Row,
    origin: Origin,
    notes: &mut Notes,
) -> Option<Record> {
    if let Some(filter) = &decl.records.filter {
        let pred = expr::parse_pred(filter)?;
        if !expr::holds(&pred, &row) {
            return None;
        }
    }

    // The text is settled before anything else, because `text:` reads it.
    let container = std::mem::take(&mut row.text);
    let text = if decl.records.text.is_empty() {
        container
    } else {
        let parts: Vec<String> = decl
            .records
            .text
            .iter()
            .flat_map(|e| {
                expr::eval(e, &row)
                    .iter()
                    .map(expr::as_string)
                    .collect::<Vec<_>>()
            })
            .filter(|s| !s.trim().is_empty())
            .collect();
        parts.join("\n\n")
    };
    row.text = text;

    let ids: Vec<Id> = match &decl.records.id {
        Some(ids) => {
            let mut seen: Vec<Id> = Vec::new();
            for spec in ids.each() {
                let scheme = spec.scheme.clone().unwrap_or_else(|| "address".into());
                for v in values(&spec.spec(), &row) {
                    let id = Id {
                        scheme: scheme.clone(),
                        // As the source wrote it. Identity folds case when two are compared; the value does
                        // not, because `Qwen/Qwen3` is a path and `cve-2021-44228` is not.
                        value: v.clone(),
                    };
                    if !seen.contains(&id) {
                        seen.push(id);
                    }
                }
            }
            seen
        }
        None => Vec::new(),
    };

    let title = one(&plain(&decl.records.title), &row).unwrap_or_default();
    let title = if title.trim().is_empty() {
        origin.address()
    } else {
        title
    };
    let url = decl.records.url.as_ref().and_then(|e| one(&plain(e), &row));

    let known = decl
        .records
        .known
        .as_ref()
        .and_then(|e| one(&plain(e), &row))
        .and_then(|raw| as_date(&raw));
    let known = match known {
        Some(k) => k,
        None => {
            notes.no_known += 1;
            row.file
                .as_ref()
                .map(|f| f.modified.clone())
                .unwrap_or_else(|| crate::iso_date(crate::now()))
        }
    };

    let mut fields: BTreeMap<String, Value> = BTreeMap::new();
    for (name, spec) in &decl.records.fields {
        let raws = values(&spec.spec(), &row);
        let mut vs = Vec::new();
        for raw in &raws {
            match typed(spec.kind, spec.vocabulary.as_deref(), raw) {
                Some(v) => vs.push(v),
                None => notes.unparsed(name, raw, spec.kind),
            }
        }
        match vs.len() {
            0 => {}
            1 if !spec.all => {
                fields.insert(name.clone(), vs.pop().unwrap());
            }
            _ => {
                fields.insert(name.clone(), Value::List(vs));
            }
        }
    }

    // An identifier names the record only where the declaration says there is one of them.
    let one_identifier = decl
        .records
        .id
        .as_ref()
        .map(|s| s.names_record())
        .unwrap_or(false);
    let names_it = match ids.first() {
        Some(id) if one_identifier => format!("{}:{}", id.scheme, id.value.to_lowercase()),
        _ => origin.address(),
    };

    if row.text.trim().is_empty() {
        notes.no_text += 1;
    }

    let mut rec = Record {
        record_id: Record::compute_id(&decl.name, &decl.kind, &names_it),
        dataset: decl.name.clone(),
        kind: decl.kind.clone(),
        ids,
        title,
        url,
        text: row.text,
        fields,
        known,
        valid: None,
        from: origin,
        attachments: Vec::new(),
        hash: String::new(),
    };
    rec.hash = rec.compute_hash();
    Some(rec)
}
