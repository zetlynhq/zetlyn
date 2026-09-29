//! The claim: one source's statement about one thing.
//!
//! Six value types and the list is closed. Nothing nests, nothing is conditional, and no field
//! refers to another. Anything the form cannot hold stays in the text.

use std::collections::BTreeMap;

use serde_json::{json, Value as J};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Text(String),
    Code {
        code: String,
        vocabulary: Option<String>,
    },
    Number(f64),
    Bool(bool),
    Date(String),
    Interval {
        from: Option<String>,
        to: Option<String>,
    },
    List(Vec<Value>),
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Text(_) => "text",
            Value::Code { .. } => "code",
            Value::Number(_) => "number",
            Value::Bool(_) => "bool",
            Value::Date(_) => "date",
            Value::Interval { .. } => "interval",
            // A list carries the type of what is in it. An empty one has nothing to carry.
            Value::List(v) => v.first().map(|f| f.type_name()).unwrap_or("text"),
        }
    }

    pub fn to_json(&self) -> J {
        match self {
            Value::Text(s) => json!({ "text": s }),
            Value::Code { code, vocabulary } => match vocabulary {
                Some(v) => json!({ "code": code, "vocabulary": v }),
                None => json!({ "code": code }),
            },
            Value::Number(n) => json!({ "number": n }),
            Value::Bool(b) => json!({ "bool": b }),
            Value::Date(d) => json!({ "date": d }),
            Value::Interval { from, to } => json!({ "interval": { "from": from, "to": to } }),
            Value::List(v) => J::Array(v.iter().map(|x| x.to_json()).collect()),
        }
    }

    pub fn from_json(j: &J) -> Option<Value> {
        if let Some(a) = j.as_array() {
            return Some(Value::List(a.iter().filter_map(Value::from_json).collect()));
        }
        let o = j.as_object()?;
        if let Some(s) = o.get("text").and_then(J::as_str) {
            return Some(Value::Text(s.into()));
        }
        if let Some(s) = o.get("code").and_then(J::as_str) {
            return Some(Value::Code {
                code: s.into(),
                vocabulary: o.get("vocabulary").and_then(J::as_str).map(str::to_string),
            });
        }
        if let Some(n) = o.get("number").and_then(J::as_f64) {
            return Some(Value::Number(n));
        }
        if let Some(b) = o.get("bool").and_then(J::as_bool) {
            return Some(Value::Bool(b));
        }
        if let Some(s) = o.get("date").and_then(J::as_str) {
            return Some(Value::Date(s.into()));
        }
        if let Some(i) = o.get("interval").and_then(J::as_object) {
            return Some(Value::Interval {
                from: i.get("from").and_then(J::as_str).map(str::to_string),
                to: i.get("to").and_then(J::as_str).map(str::to_string),
            });
        }
        None
    }

    /// What is shown, and what an equality filter compares against.
    pub fn display(&self) -> String {
        match self {
            Value::Text(s) => s.clone(),
            Value::Code { code, .. } => code.clone(),
            Value::Number(n) => {
                if n.fract() == 0.0 {
                    format!("{}", *n as i64)
                } else {
                    format!("{n}")
                }
            }
            Value::Bool(b) => b.to_string(),
            Value::Date(d) => d.clone(),
            Value::Interval { from, to } => format!(
                "{} – {}",
                from.as_deref().unwrap_or("…"),
                to.as_deref().unwrap_or("…")
            ),
            Value::List(v) => v.iter().map(Value::display).collect::<Vec<_>>().join(", "),
        }
    }

    /// Every element, so a list is stored as one row per element and a comparison is satisfied
    /// when one of them satisfies it.
    pub fn flatten(&self) -> Vec<&Value> {
        match self {
            Value::List(v) => v.iter().flat_map(Value::flatten).collect(),
            other => vec![other],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Id {
    pub scheme: String,
    pub value: String,
}

#[derive(Clone, Debug, Default)]
pub struct Origin {
    pub url: Option<String>,
    pub file: Option<String>,
    pub row: Option<u64>,
    pub span: Option<(usize, usize)>,
}

impl Origin {
    pub fn to_json(&self) -> J {
        json!({
            "url": self.url, "file": self.file, "row": self.row,
            "span": self.span.map(|(a, b)| json!([a, b])),
        })
    }
    pub fn address(&self) -> String {
        if let Some(u) = &self.url {
            return u.clone();
        }
        match (&self.file, self.row) {
            (Some(f), Some(r)) => format!("{f}#{r}"),
            (Some(f), None) => f.clone(),
            _ => String::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Attachment {
    pub path: String,
    pub media_type: String,
    pub bytes: u64,
    pub sha256: String,
}

impl Attachment {
    pub fn to_json(&self) -> J {
        json!({ "path": self.path, "media_type": self.media_type,
                "bytes": self.bytes, "sha256": self.sha256 })
    }
}

#[derive(Clone, Debug)]
pub struct Claim {
    pub record_id: String,
    pub dataset: String,
    pub kind: String,
    pub ids: Vec<Id>,
    pub title: String,
    pub url: Option<String>,
    pub text: String,
    pub fields: BTreeMap<String, Value>,
    pub known: String,
    pub valid: Option<(Option<String>, Option<String>)>,
    pub from: Origin,
    pub attachments: Vec<Attachment>,
    pub hash: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Claim {
    /// SHA-256 of the source name, the kind, and what names this claim.
    ///
    /// What names it is the identifier where the declaration says a claim carries one, and the
    /// address otherwise. A declaration with `all = true` says its identifiers are references
    /// rather than names: 2,698 Metasploit modules name 2,408 CVEs, and several modules exploit
    /// one vulnerability, so the CVE names the thing and not the claim.
    ///
    /// The source name is in it because a claim is one source's statement. Two sources
    /// describing one thing hold two claims, which is what makes the join a derivation rather
    /// than a collision.
    pub fn compute_id(dataset: &str, kind: &str, names_it: &str) -> String {
        let mut h = Sha256::new();
        h.update(dataset.as_bytes());
        h.update(b"\n");
        h.update(kind.as_bytes());
        h.update(b"\n");
        h.update(names_it.as_bytes());
        hex(&h.finalize())
    }
    /// changed what it says.
    pub fn compute_hash(&self) -> String {
        let mut h = Sha256::new();
        for id in &self.ids {
            h.update(id.scheme.as_bytes());
            h.update(b"\x1f");
            h.update(id.value.as_bytes());
            h.update(b"\x1e");
        }
        h.update(self.title.as_bytes());
        h.update(b"\x1e");
        h.update(self.text.as_bytes());
        h.update(b"\x1e");
        for (name, value) in &self.fields {
            h.update(name.as_bytes());
            h.update(b"\x1f");
            h.update(value.to_json().to_string().as_bytes());
            h.update(b"\x1e");
        }
        h.update(self.known.as_bytes());
        format!("sha256:{}", hex(&h.finalize()))
    }

    pub fn ids_json(&self) -> J {
        J::Array(
            self.ids
                .iter()
                .map(|i| json!({ "scheme": i.scheme, "value": i.value }))
                .collect(),
        )
    }

    pub fn fields_json(&self) -> J {
        J::Object(
            self.fields
                .iter()
                .map(|(k, v)| (k.clone(), v.to_json()))
                .collect(),
        )
    }

    pub fn to_json(&self) -> J {
        json!({
            "claim_id": self.record_id,
            "source": self.dataset,
            "kind": self.kind,
            "ids": self.ids_json(),
            "title": self.title,
            "url": self.url,
            "text": self.text,
            "properties": self.fields_json(),
            "known": self.known,
            "valid": self.valid.as_ref().map(|(f, t)| json!({ "from": f, "to": t })),
            "from": self.from.to_json(),
            "attachments": J::Array(self.attachments.iter().map(Attachment::to_json).collect()),
            "hash": self.hash,
        })
    }
}

impl Id {
    /// Two identifiers denote one thing when they differ only in case. `CVE-2021-44228` and
    /// `cve-2021-44228` are one; the value each source shows is the one its source wrote.
    pub fn same(&self, other: &Id) -> bool {
        self.scheme == other.scheme && self.value.eq_ignore_ascii_case(&other.value)
    }
}

impl Claim {
    /// The inverse of `to_json`, for a claim that arrives in a published artifact rather than
    /// out of a source. The hash travels with it and is checked against a recomputation, because
    /// a claim whose hash does not describe it would be a claim this store cannot compare.
    pub fn from_json(dataset: &str, j: &J) -> Result<Claim, String> {
        let s = |k: &str| j.get(k).and_then(J::as_str).unwrap_or("").to_string();
        let opt = |k: &str| j.get(k).and_then(J::as_str).map(str::to_string);
        let ids = j
            .get("ids")
            .and_then(J::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| {
                        Some(Id {
                            scheme: v.get("scheme")?.as_str()?.to_string(),
                            value: v.get("value")?.as_str()?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let fields = j
            .get("properties")
            .and_then(J::as_object)
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| Some((k.clone(), Value::from_json(v)?)))
                    .collect()
            })
            .unwrap_or_default();
        let from = j.get("from").cloned().unwrap_or(J::Null);
        let valid = j.get("valid").filter(|v| !v.is_null()).map(|v| {
            (
                v.get("from").and_then(J::as_str).map(str::to_string),
                v.get("to").and_then(J::as_str).map(str::to_string),
            )
        });
        let record = Claim {
            record_id: s("claim_id"),
            dataset: dataset.to_string(),
            kind: s("kind"),
            ids,
            title: s("title"),
            url: opt("url"),
            text: s("text"),
            fields,
            known: s("known"),
            valid,
            from: Origin {
                url: from.get("url").and_then(J::as_str).map(str::to_string),
                file: from.get("file").and_then(J::as_str).map(str::to_string),
                row: from.get("row").and_then(J::as_u64),
                span: None,
            },
            attachments: Vec::new(),
            hash: s("hash"),
        };
        if record.record_id.is_empty() {
            return Err("a claim with no id".into());
        }
        let recomputed = record.compute_hash();
        if record.hash != recomputed {
            return Err(format!(
                "{}: the hash does not describe the claim",
                record.record_id
            ));
        }
        Ok(record)
    }
}
