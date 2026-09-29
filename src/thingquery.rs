//! A question about things, asked of a tracker's store rather than of its sources: which things
//! are in conflict about a property, which only one source knows, which a source rates critical,
//! which gained an exploit this week. What a watch on a view asks, and what the things list shows.
//!
//! ```text
//! conflict:severity and has:kev
//! nvd.severity=critical or redhat.severity>=high
//! nvd.severity != redhat.severity
//! only:nvd and not has:exploitdb
//! appeared:exploit<7d
//! changed:severity<24h
//! id=CVE-2021-44228
//! ```
//!
//! `and` binds tighter than `or`, `not` tighter than both, and parentheses group. A source is
//! named by its whole name or by the end of it, `nvd` for `zetlyn/cve-nvd`, where only one fits.

use std::collections::{BTreeMap, BTreeSet};

use crate::thingstore::Said;
use crate::trackerdecl::TrackerDecl;

#[derive(Clone, Debug, PartialEq)]
pub enum Q {
    And(Box<Q>, Box<Q>),
    Or(Box<Q>, Box<Q>),
    Not(Box<Q>),
    Conflict(String),
    Has(String),
    Only(String),
    /// A claim of this kind first appeared within this many seconds.
    Appeared(String, i64),
    /// This property changed at a source within this many seconds.
    Changed(String, i64),
    Id(String),
    /// `affects:linux/linux`, or `affects:microsoft/*` for everything of that vendor.
    Related(String, String),
    /// `[source.]property op literal`.
    Cmp { source: Option<String>, property: String, op: Op, value: String },
    /// `source.property op source.property`, which is `=` or `!=` and compares what each says.
    Between { a: (String, String), op: Op, b: (String, String) },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// One thing, as a question is asked of it.
#[derive(Default)]
pub struct ThingView {
    pub key: String,
    pub value: String,
    /// Source, property, what it says.
    pub by: BTreeMap<String, BTreeMap<String, Said>>,
    /// Source, when it first spoke of the thing and the kind of claim it makes.
    pub speaks: BTreeMap<String, (String, String)>,
    pub conflicts: BTreeSet<String>,
    /// Property, every time a change to it was signalled.
    pub changed: BTreeMap<String, Vec<String>>,
    /// Relation, the other side, who says so.
    pub related: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
}

/// What a question is asked against: the tracker's declaration for its scales and the names of
/// its sources, and the time it is asked at.
pub struct Context<'a> {
    pub decl: &'a TrackerDecl,
    pub sources: Vec<String>,
    /// Per source, the properties it says, under the names this tracker shows them by, and the
    /// kind of its claims. Empty where the caller does not know them, and then nothing is refused.
    pub properties: BTreeMap<String, BTreeSet<String>>,
    pub kinds: BTreeSet<String>,
    pub now: i64,
}

impl Context<'_> {
    /// A property some source says, or the one named says. One nobody says is refused by name,
    /// with what there is, rather than read as a question nothing can ever answer.
    pub fn property(&self, source: Option<&str>, name: &str) -> Result<(), String> {
        if self.properties.is_empty() {
            return Ok(());
        }
        match source {
            Some(s) => {
                let has = self.properties.get(s).cloned().unwrap_or_default();
                if has.contains(name) {
                    Ok(())
                } else {
                    Err(format!("{s} says no {name}. It says: {}", has.into_iter().collect::<Vec<_>>().join(", ")))
                }
            }
            None if self.properties.values().any(|p| p.contains(name)) => Ok(()),
            None => {
                let all: BTreeSet<&String> = self.properties.values().flatten().collect();
                Err(format!("no source here says {name}. Properties: {}", all.into_iter().cloned().collect::<Vec<_>>().join(", ")))
            }
        }
    }

    /// A source by its name or the end of it. Ambiguous or unknown is an error that names the
    /// candidates, because a watch that silently matched nothing is one nobody hears from.
    pub fn source(&self, named: &str) -> Result<String, String> {
        if self.sources.iter().any(|s| s == named) {
            return Ok(named.to_string());
        }
        let fits: Vec<&String> = self
            .sources
            .iter()
            .filter(|s| {
                let last = s.rsplit('/').next().unwrap_or(s);
                last == named || last.ends_with(&format!("-{named}"))
            })
            .collect();
        match fits.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err(format!("{named}: no source here is called that. Sources: {}", self.sources.join(", "))),
            many => Err(format!(
                "{named}: more than one source ends that way: {}",
                many.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            )),
        }
    }
}

// -- reading one ------------------------------------------------------------------------------

pub fn parse(text: &str, cx: &Context) -> Result<Q, String> {
    let tokens = tokenize(text);
    if tokens.is_empty() {
        return Err("an empty question".into());
    }
    let mut at = 0;
    let q = or(&tokens, &mut at, cx)?;
    if at != tokens.len() {
        return Err(format!("{}: not understood here", tokens[at..].join(" ")));
    }
    Ok(q)
}

fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            '(' | ')' if !quoted => {
                if !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
                out.push(c.to_string());
            }
            c if c.is_whitespace() && !quoted => {
                if !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    // `a != b` and `a!=b` are one comparison; join a comparison written with spaces around it.
    let mut joined: Vec<String> = Vec::new();
    let mut i = 0;
    while i < out.len() {
        let op_alone = matches!(out[i].as_str(), "=" | "!=" | "<" | "<=" | ">" | ">=");
        if op_alone && !joined.is_empty() && i + 1 < out.len() {
            let left = joined.pop().unwrap();
            joined.push(format!("{left}{}{}", out[i], out[i + 1]));
            i += 2;
            continue;
        }
        joined.push(out[i].clone());
        i += 1;
    }
    joined
}

fn or(t: &[String], at: &mut usize, cx: &Context) -> Result<Q, String> {
    let mut left = and(t, at, cx)?;
    while *at < t.len() && t[*at].eq_ignore_ascii_case("or") {
        *at += 1;
        let right = and(t, at, cx)?;
        left = Q::Or(Box::new(left), Box::new(right));
    }
    Ok(left)
}

fn and(t: &[String], at: &mut usize, cx: &Context) -> Result<Q, String> {
    let mut left = unary(t, at, cx)?;
    loop {
        if *at < t.len() && t[*at].eq_ignore_ascii_case("and") {
            *at += 1;
        } else if *at < t.len() && t[*at] != ")" && !t[*at].eq_ignore_ascii_case("or") {
            // Two terms side by side mean both, as they do in a search box.
        } else {
            break;
        }
        let right = unary(t, at, cx)?;
        left = Q::And(Box::new(left), Box::new(right));
    }
    Ok(left)
}

fn unary(t: &[String], at: &mut usize, cx: &Context) -> Result<Q, String> {
    let Some(word) = t.get(*at) else {
        return Err("the question ends where a term should be".into());
    };
    if word.eq_ignore_ascii_case("not") {
        *at += 1;
        return Ok(Q::Not(Box::new(unary(t, at, cx)?)));
    }
    if word == "(" {
        *at += 1;
        let inner = or(t, at, cx)?;
        if t.get(*at).map(String::as_str) != Some(")") {
            return Err("a parenthesis is opened and not closed".into());
        }
        *at += 1;
        return Ok(inner);
    }
    *at += 1;
    atom(word, cx)
}

fn atom(word: &str, cx: &Context) -> Result<Q, String> {
    // A relation the tracker declares, by its name. One it does not is not a term.
    if let Some((name, rest)) = word.split_once(':') {
        if let Some(r) = cx.decl.relations.iter().find(|r| r.name == name) {
            if rest.trim().is_empty() {
                return Err(format!("{word}: say what, as {}:{}", r.name, if r.part.as_deref() == Some("product") { "vendor/product" } else { "value" }));
            }
            return Ok(Q::Related(r.name.clone(), rest.trim().to_string()));
        }
    }
    if let Some(p) = word.strip_prefix("conflict:") {
        // Only an aligned property is compared, so any other would quietly hold for nothing.
        let aligned = &cx.decl.normalise;
        if !aligned.contains_key(p) {
            let names: Vec<&str> = aligned.keys().map(String::as_str).collect();
            return Err(if names.is_empty() {
                format!("{word}: this tracker aligns no property, so nothing here can conflict")
            } else {
                format!("{word}: only aligned properties are compared: {}", names.join(", "))
            });
        }
        return Ok(Q::Conflict(p.to_string()));
    }
    if let Some(s) = word.strip_prefix("has:") {
        return Ok(Q::Has(cx.source(s)?));
    }
    if let Some(s) = word.strip_prefix("only:") {
        return Ok(Q::Only(cx.source(s)?));
    }
    for (prefix, make) in [
        ("appeared:", Q::Appeared as fn(String, i64) -> Q),
        ("changed:", Q::Changed as fn(String, i64) -> Q),
    ] {
        if let Some(rest) = word.strip_prefix(prefix) {
            let (what, within) = rest
                .split_once('<')
                .ok_or_else(|| format!("{word}: say how recent, as {prefix}…<7d"))?;
            if prefix == "appeared:" && !cx.kinds.is_empty() && !cx.kinds.contains(what) {
                return Err(format!("{word}: no source here makes claims of kind {what}. Kinds: {}", cx.kinds.iter().cloned().collect::<Vec<_>>().join(", ")));
            }
            if prefix == "changed:" {
                cx.property(None, what)?;
            }
            return Ok(make(what.to_string(), duration(within)?));
        }
    }
    let (left, op, right) = split_op(word).ok_or_else(|| {
        let rel: Vec<String> = cx.decl.relations.iter().map(|r| format!("{}:", r.name)).collect();
        format!("{word}: not a term. conflict:, has:, only:, appeared:, changed:, {}or name=value", rel.iter().map(|r| format!("{r}, ")).collect::<String>())
    })?;
    if left.eq_ignore_ascii_case("id") {
        return Ok(Q::Id(right.to_string()));
    }
    // A property name holds no dot, so a dot names a source, and one that is not here is said
    // rather than read as a property nothing has.
    let side = |s: &str| -> Result<Option<(String, String)>, String> {
        match s.split_once('.') {
            Some((src, prop)) if !src.is_empty() && !prop.is_empty() && !is_number(s) => {
                Ok(Some((cx.source(src)?, prop.to_string())))
            }
            _ => Ok(None),
        }
    };
    let l = side(left)?;
    if let (Some(a), Some(b)) = (l.clone(), side(right)?) {
        if !matches!(op, Op::Eq | Op::Ne) {
            return Err(format!("{word}: two sources are compared with = or != only"));
        }
        cx.property(Some(&a.0), &a.1)?;
        cx.property(Some(&b.0), &b.1)?;
        return Ok(Q::Between { a, op, b });
    }
    let (source, property) = match l {
        Some((s, p)) => (Some(s), p),
        None => (None, left.to_string()),
    };
    cx.property(source.as_deref(), &property)?;
    Ok(Q::Cmp { source, property, op, value: right.trim_matches('"').to_string() })
}

fn split_op(word: &str) -> Option<(&str, Op, &str)> {
    for (text, op) in [
        ("!=", Op::Ne),
        (">=", Op::Ge),
        ("<=", Op::Le),
        ("=", Op::Eq),
        (">", Op::Gt),
        ("<", Op::Lt),
    ] {
        if let Some(i) = word.find(text) {
            return Some((&word[..i], op, &word[i + text.len()..]));
        }
    }
    None
}

/// `30m`, `24h`, `7d`, as seconds.
fn duration(s: &str) -> Result<i64, String> {
    let (n, unit) = s.split_at(s.trim_end_matches(|c: char| c.is_ascii_alphabetic()).len());
    let n: i64 = n.parse().map_err(|_| format!("{s}: a duration is a number and m, h or d"))?;
    match unit {
        "m" => Ok(n * 60),
        "h" => Ok(n * 3600),
        "d" | "" => Ok(n * 86_400),
        _ => Err(format!("{s}: a duration is a number and m, h or d")),
    }
}

// -- asking it --------------------------------------------------------------------------------

pub fn holds(q: &Q, t: &ThingView, cx: &Context) -> bool {
    match q {
        Q::And(a, b) => holds(a, t, cx) && holds(b, t, cx),
        Q::Or(a, b) => holds(a, t, cx) || holds(b, t, cx),
        Q::Not(a) => !holds(a, t, cx),
        Q::Conflict(p) => t.conflicts.contains(p),
        Q::Has(s) => t.speaks.contains_key(s),
        Q::Only(s) => t.speaks.len() == 1 && t.speaks.contains_key(s),
        Q::Appeared(kind, within) => t.speaks.values().any(|(first, k)| {
            k == kind && seconds(first).is_some_and(|at| cx.now - at <= *within)
        }),
        Q::Changed(p, within) => t
            .changed
            .get(p)
            .is_some_and(|ats| ats.iter().any(|at| seconds(at).is_some_and(|s| cx.now - s <= *within))),
        Q::Id(v) => t.value.eq_ignore_ascii_case(v),
        Q::Related(name, want) => t.related.get(name).is_some_and(|targets| {
            let want = want.to_lowercase();
            match want.strip_suffix('*') {
                Some(prefix) => targets.keys().any(|k| k.starts_with(prefix)),
                None => targets.contains_key(&want),
            }
        }),
        Q::Cmp { source, property, op, value } => t
            .by
            .iter()
            .filter(|(s, _)| source.as_ref().map(|want| want == *s).unwrap_or(true))
            .filter_map(|(_, props)| props.get(property))
            .flat_map(|said| said.means.iter())
            .any(|v| compare(cx, property, v, *op, value)),
        Q::Between { a, op, b } => {
            let words = |(s, p): &(String, String)| t.by.get(s).and_then(|props| props.get(p)).map(|x| x.means.clone());
            match (words(a), words(b)) {
                (Some(x), Some(y)) => (x == y) == (*op == Op::Eq),
                // A source that says nothing about it neither agrees nor disagrees.
                _ => false,
            }
        }
    }
}

/// On the tracker's scale where it has one, as numbers or dates where both are, and as words
/// otherwise, which compare for equality only.
fn compare(cx: &Context, property: &str, have: &str, op: Op, want: &str) -> bool {
    use std::cmp::Ordering;
    let ord: Option<Ordering> = match cx.decl.normalise_for(property).filter(|a| !a.scale.is_empty()) {
        // Best first, so `>= high` is a smaller position.
        Some(a) => match (a.position(have), a.position(want)) {
            (Some(x), Some(y)) => Some(y.cmp(&x)),
            _ => None,
        },
        None => match (have.parse::<f64>(), want.parse::<f64>()) {
            (Ok(x), Ok(y)) => x.partial_cmp(&y),
            _ if have.len() >= 10 && want.len() >= 10 && have.as_bytes()[4] == b'-' && want.as_bytes()[4] == b'-' => {
                Some(have[..10].cmp(&want[..10]))
            }
            _ => None,
        },
    };
    match ord {
        Some(o) => match op {
            Op::Eq => o == Ordering::Equal,
            Op::Ne => o != Ordering::Equal,
            Op::Lt => o == Ordering::Less,
            Op::Le => o != Ordering::Greater,
            Op::Gt => o == Ordering::Greater,
            Op::Ge => o != Ordering::Less,
        },
        None => match op {
            Op::Eq => have.eq_ignore_ascii_case(want),
            Op::Ne => !have.eq_ignore_ascii_case(want),
            _ => false,
        },
    }
}

/// Unix seconds, from `2026-09-29T10:36:20Z`.
pub fn seconds(stamp: &str) -> Option<i64> {
    (stamp.len() >= 10).then(|| crate::fetch::seconds_of(stamp))
}

/// `9.8` is a number and not a source called `9`.
fn is_number(s: &str) -> bool {
    s.parse::<f64>().is_ok()
}
