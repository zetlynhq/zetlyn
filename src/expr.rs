//! Where a value comes from, and the predicate used by `where`, by a view and by a query.
//!
//! Five prefixes, the same for every source kind. A bare string is `field:`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value as J;

pub struct FileInfo {
    pub path: PathBuf,
    pub rel: String,
    pub modified: String,
    pub size: u64,
    pub media_type: String,
}

/// What an expression is evaluated against: the structured claim, what the container says, the
/// file it came from, and the text already extracted for it.
pub struct Row<'a> {
    pub value: J,
    pub meta: BTreeMap<String, String>,
    pub file: Option<FileInfo>,
    pub text: String,
    pub root: &'a Path,
}

/// Walks `a.b.c`, expanding `a[]` into every element of a list.
pub(crate) fn walk(value: &J, path: &str) -> Vec<J> {
    let mut here = vec![value.clone()];
    if path == "*" {
        // Every value of an object, which is how a file keyed by name hands over its claims.
        return match value {
            J::Object(o) => o.values().cloned().collect(),
            J::Array(a) => a.clone(),
            other => vec![other.clone()],
        };
    }
    for raw in path.split('.') {
        let (key, explode) = match raw.strip_suffix("[]") {
            Some(k) => (k, true),
            None => (raw, false),
        };
        let mut next = Vec::new();
        for v in &here {
            let picked = if key.is_empty() {
                Some(v.clone())
            } else {
                match v {
                    J::Object(o) => o.get(key).cloned(),
                    _ => None,
                }
            };
            match picked {
                Some(J::Array(a)) if explode => next.extend(a),
                Some(other) => next.push(other),
                None => {}
            }
        }
        here = next;
        if here.is_empty() {
            return here;
        }
    }
    // A list reached without `[]` is still several values: nothing downstream wants the array.
    here.into_iter()
        .flat_map(|v| match v {
            J::Array(a) => a,
            other => vec![other],
        })
        .collect()
}

/// What a file says, through the same extractor a folder run uses. A `.db` is not text because it
/// decodes as one: `file:self` over a SQLite store would otherwise read eight megabytes of it.
fn read_text(path: &Path) -> Option<String> {
    crate::rows::extract(path)
}

/// Substitutes `{…}` from the other prefixes.
pub fn fill(s: &str, row: &Row) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let inner = &rest[open + 1..open + close];
        if let Some(v) = eval(inner, row).first() {
            out.push_str(&as_string(v));
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out
}

pub fn as_string(v: &J) -> String {
    match v {
        J::String(s) => s.clone(),
        J::Null => String::new(),
        J::Number(n) => n.to_string(),
        J::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

const FILE_WORDS: [&str; 8] = [
    "path",
    "rel",
    "name",
    "stem",
    "ext",
    "modified",
    "size",
    "media_type",
];

/// Every value an expression yields. Empty where the source has none.
pub fn eval(expr: &str, row: &Row) -> Vec<J> {
    let (prefix, rest) = match expr.split_once(':') {
        // `field:` is the default, and a bare string with no prefix is one.
        Some((p, r)) if matches!(p, "field" | "file" | "meta" | "text" | "const") => (p, r),
        _ => ("field", expr),
    };
    match prefix {
        "field" => walk(&row.value, rest),
        "meta" => row
            .meta
            .get(rest)
            .map(|s| vec![J::String(s.clone())])
            .unwrap_or_default(),
        "const" => vec![J::String(fill(rest, row))],
        "text" => {
            let pattern = rest.trim_matches('/');
            let Ok(re) = regex::Regex::new(pattern) else {
                return Vec::new();
            };
            re.find_iter(&row.text)
                .map(|m| J::String(m.as_str().to_string()))
                .collect()
        }
        "file" => {
            let Some(f) = &row.file else {
                return Vec::new();
            };
            let one = |s: String| vec![J::String(s)];
            if FILE_WORDS.contains(&rest) {
                let p = &f.path;
                return one(match rest {
                    "path" => p.display().to_string(),
                    "rel" => f.rel.clone(),
                    "name" => p
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    "stem" => p
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    "ext" => p
                        .extension()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    "modified" => f.modified.clone(),
                    "size" => f.size.to_string(),
                    _ => f.media_type.clone(),
                });
            }
            if rest == "self" {
                return read_text(&f.path)
                    .map(|s| vec![J::String(s)])
                    .unwrap_or_default();
            }
            if rest == "sidecar" {
                // Whatever a person wrote beside the file, under any of the plain-text suffixes.
                for suffix in [".txt", ".md", ".caption"] {
                    let side = f.path.with_extension(suffix.trim_start_matches('.'));
                    if let Some(s) = read_text(&side) {
                        return vec![J::String(s)];
                    }
                }
                return Vec::new();
            }
            // Anything else is a path, relative to the source root, and the value is its text.
            let named = fill(rest, row);
            if named.is_empty() {
                return Vec::new();
            }
            read_text(&row.root.join(&named))
                .map(|s| vec![J::String(s)])
                .unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------------------------
// The predicate. One parser, three uses: `claims.where`, a view's `where`, and a typed query.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Op {
    pub fn sql(self) -> &'static str {
        match self {
            Op::Eq => "=",
            Op::Ne => "!=",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Lit {
    Str(String),
    Num(f64),
    Bool(bool),
}

impl Lit {
    pub fn display(&self) -> String {
        match self {
            Lit::Str(s) => s.clone(),
            Lit::Num(n) => {
                if n.fract() == 0.0 {
                    format!("{}", *n as i64)
                } else {
                    format!("{n}")
                }
            }
            Lit::Bool(b) => b.to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Pred {
    Cmp { left: String, op: Op, right: Lit },
    And(Box<Pred>, Box<Pred>),
    Or(Box<Pred>, Box<Pred>),
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Word(String),
    Str(String),
    Op(Op),
    And,
    Or,
    Open,
    Close,
}

fn lex(s: &str) -> Vec<Tok> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '(' => {
                out.push(Tok::Open);
                i += 1;
            }
            ')' => {
                out.push(Tok::Close);
                i += 1;
            }
            '\'' | '"' => {
                let quote = c;
                i += 1;
                let mut buf = String::new();
                while i < chars.len() && chars[i] != quote {
                    buf.push(chars[i]);
                    i += 1;
                }
                i += 1;
                out.push(Tok::Str(buf));
            }
            '=' | '!' | '<' | '>' => {
                let two = i + 1 < chars.len() && chars[i + 1] == '=';
                let op = match (c, two) {
                    ('=', _) => Op::Eq,
                    ('!', true) => Op::Ne,
                    ('<', true) => Op::Le,
                    ('<', false) => Op::Lt,
                    ('>', true) => Op::Ge,
                    ('>', false) => Op::Gt,
                    _ => Op::Eq,
                };
                i += if two { 2 } else { 1 };
                out.push(Tok::Op(op));
            }
            _ => {
                let mut buf = String::new();
                while i < chars.len()
                    && !chars[i].is_whitespace()
                    && !"()=!<>'\"".contains(chars[i])
                {
                    buf.push(chars[i]);
                    i += 1;
                }
                match buf.to_ascii_lowercase().as_str() {
                    "and" => out.push(Tok::And),
                    "or" => out.push(Tok::Or),
                    _ => out.push(Tok::Word(buf)),
                }
            }
        }
    }
    out
}

fn literal(t: &Tok) -> Option<Lit> {
    match t {
        Tok::Str(s) => Some(Lit::Str(s.clone())),
        Tok::Word(w) => Some(match w.to_ascii_lowercase().as_str() {
            "true" => Lit::Bool(true),
            "false" => Lit::Bool(false),
            _ => match w.parse::<f64>() {
                Ok(n) => Lit::Num(n),
                Err(_) => Lit::Str(w.clone()),
            },
        }),
        _ => None,
    }
}

struct Parser {
    toks: Vec<Tok>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }
    fn or(&mut self) -> Option<Pred> {
        let mut left = self.and()?;
        while self.peek() == Some(&Tok::Or) {
            self.at += 1;
            let right = self.and()?;
            left = Pred::Or(Box::new(left), Box::new(right));
        }
        Some(left)
    }
    fn and(&mut self) -> Option<Pred> {
        let mut left = self.unary()?;
        while self.peek() == Some(&Tok::And) {
            self.at += 1;
            let right = self.unary()?;
            left = Pred::And(Box::new(left), Box::new(right));
        }
        Some(left)
    }
    fn unary(&mut self) -> Option<Pred> {
        if self.peek() == Some(&Tok::Open) {
            self.at += 1;
            let inner = self.or()?;
            if self.peek() == Some(&Tok::Close) {
                self.at += 1;
            }
            return Some(inner);
        }
        let left = match self.peek()? {
            Tok::Word(w) => w.clone(),
            _ => return None,
        };
        self.at += 1;
        let op = match self.peek()? {
            Tok::Op(o) => *o,
            _ => return None,
        };
        self.at += 1;
        let right = literal(self.peek()?)?;
        self.at += 1;
        Some(Pred::Cmp { left, op, right })
    }
}

pub fn parse_pred(s: &str) -> Option<Pred> {
    let toks = lex(s);
    if toks.is_empty() {
        return None;
    }
    Parser { toks, at: 0 }.or()
}

/// A query is free text and comparisons mixed. Anything that reads as `name op literal` is a
/// filter; every other word is a term. A person types both without being told which is which.
pub fn parse_query(s: &str) -> (String, Option<Pred>) {
    let toks = lex(s);
    let mut terms: Vec<String> = Vec::new();
    let mut preds: Vec<Pred> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if let (Some(Tok::Word(name)), Some(Tok::Op(op))) = (toks.get(i), toks.get(i + 1)) {
            if let Some(lit) = toks.get(i + 2).and_then(literal) {
                preds.push(Pred::Cmp {
                    left: name.clone(),
                    op: *op,
                    right: lit,
                });
                i += 3;
                continue;
            }
        }
        match &toks[i] {
            Tok::Word(w) => terms.push(w.clone()),
            Tok::Str(w) => terms.push(w.clone()),
            _ => {}
        }
        i += 1;
    }
    let pred = preds
        .into_iter()
        .reduce(|a, b| Pred::And(Box::new(a), Box::new(b)));
    (terms.join(" "), pred)
}

/// For `claims.where`, which runs over a row before the claim exists.
pub fn holds(pred: &Pred, row: &Row) -> bool {
    match pred {
        Pred::And(a, b) => holds(a, row) && holds(b, row),
        Pred::Or(a, b) => holds(a, row) || holds(b, row),
        Pred::Cmp { left, op, right } => eval(left, row).iter().any(|v| compare(v, *op, right)),
    }
}

fn compare(v: &J, op: Op, lit: &Lit) -> bool {
    let ordering = match (v, lit) {
        (J::Number(n), Lit::Num(m)) => n.as_f64().unwrap_or(f64::NAN).partial_cmp(m),
        (J::Bool(a), Lit::Bool(b)) => Some(a.cmp(b)),
        _ => {
            let a = as_string(v);
            let b = lit.display();
            Some(a.to_lowercase().cmp(&b.to_lowercase()))
        }
    };
    let Some(ord) = ordering else { return false };
    match op {
        Op::Eq => ord.is_eq(),
        Op::Ne => ord.is_ne(),
        Op::Lt => ord.is_lt(),
        Op::Le => ord.is_le(),
        Op::Gt => ord.is_gt(),
        Op::Ge => ord.is_ge(),
    }
}

/// Every field a predicate names, so the tracker can say which of them a source could not answer.
pub fn fields_named(pred: &Pred, out: &mut Vec<String>) {
    match pred {
        Pred::And(a, b) | Pred::Or(a, b) => {
            fields_named(a, out);
            fields_named(b, out);
        }
        Pred::Cmp { left, .. } => out.push(left.clone()),
    }
}
