//! A scope. It holds no index and searches nothing: it rewrites a query per member, fans out,
//! merges ranked lists, gathers the hits into entries, and renders.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{json, Value as J};

use crate::source::{Source, Interface, Query};
use crate::expr::{self, Lit, Op, Pred};
use crate::claim::{Id, Claim};
use crate::trackerdecl::{KindView, SourceRef, TrackerDecl};
use crate::store::Hit;

pub struct Resolved {
    pub decl: SourceRef,
    pub member: Box<dyn Interface>,
    pub described: J,
}

impl Resolved {
    pub fn name(&self) -> &str {
        self.member.name()
    }
    pub fn kind(&self) -> String {
        self.described["kind"]
            .as_str()
            .unwrap_or("claim")
            .to_string()
    }
    pub fn records(&self) -> u64 {
        self.described["claims"].as_u64().unwrap_or(0)
    }
    pub fn state(&self) -> &str {
        self.described["state"].as_str().unwrap_or("empty")
    }
    pub fn can(&self, what: &str) -> bool {
        self.described["can"]
            .as_array()
            .is_some_and(|a| a.iter().any(|c| c == what))
    }
    /// How many of this member's records carry a field, for the coverage beside a facet.
    pub fn field_records(&self, field: &str) -> Option<u64> {
        self.described["properties"]
            .as_array()?
            .iter()
            .find(|f| f["name"].as_str() == Some(field))?["claims"]
            .as_u64()
    }
    pub fn view(&self, name: &str) -> Option<&J> {
        self.described["views"]
            .as_array()?
            .iter()
            .find(|v| v["name"].as_str() == Some(name))
    }
}

pub struct Tracker {
    /// The deployment this scope was opened in, where its watches live.
    pub root: PathBuf,
    pub decl: TrackerDecl,
    pub members: Vec<Resolved>,
    pub missing: Vec<String>,
}

#[derive(Default)]
pub struct TrackerQuery {
    pub text: String,
    pub pred: Option<Pred>,
    pub named: Option<String>,
    pub kind: Option<String>,
    pub sort: Option<String>,
    pub limit: usize,
    pub offset: usize,
    /// The paywall, applied to every member alike.
    pub seen_before: Option<String>,
}

/// One member's record inside an entry.
pub struct ClaimRef {
    pub member: String,
    pub priority: u8,
    pub kind: String,
    pub record_id: String,
    pub title: String,
    pub url: Option<String>,
    pub known: String,
    pub fields: BTreeMap<String, String>,
}

/// Per field, every value with the member that said it, and what this scope makes of it.
pub struct PropertyView {
    /// Per member, every distinct value that member said, sorted. A member can say a field
    /// several times for one subject: four quantisations of one model carry four licences, and
    /// keeping one of them would mean keeping whichever arrived last.
    pub by: BTreeMap<String, Vec<String>>,
    pub means: BTreeMap<String, Vec<String>>,
    pub divergent: bool,
    pub mapped: bool,
}

pub struct Thing {
    pub key: Option<Id>,
    pub rank: usize,
    pub title: String,
    pub parts: Vec<ClaimRef>,
    pub fields: BTreeMap<String, PropertyView>,
    pub why: Vec<String>,
}

impl Thing {
    /// Grouped by kind, and ordered by the scope's priority inside each group.
    pub fn by_kind(&self) -> Vec<(String, Vec<&ClaimRef>)> {
        let mut kinds: Vec<String> = Vec::new();
        for p in &self.parts {
            if !kinds.contains(&p.kind) {
                kinds.push(p.kind.clone());
            }
        }
        kinds
            .into_iter()
            .map(|k| {
                let mut ps: Vec<&ClaimRef> = self.parts.iter().filter(|p| p.kind == k).collect();
                ps.sort_by_key(|p| p.priority);
                (k, ps)
            })
            .collect()
    }
    pub fn members(&self) -> Vec<&str> {
        let mut seen: Vec<&str> = Vec::new();
        for p in &self.parts {
            if !seen.contains(&p.member.as_str()) {
                seen.push(&p.member);
            }
        }
        seen
    }
}

pub struct Answer {
    pub total: u64,
    pub entries: Vec<Thing>,
    pub answered: Vec<String>,
    pub unanswered: Vec<(String, Vec<String>)>,
    /// The total is a floor: a member had more candidates than were read, and the second pass
    /// could only count what it saw.
    pub truncated: bool,
    /// What the total counts. Without a predicate it is records, summed from what each member
    /// says it holds. With one it is subjects, counted after the pass over assembled entries,
    /// because that pass is the only place the question can be answered.
    pub subjects: bool,
}

/// How deep each member is read when the query carries a predicate. The second pass runs over
/// assembled entries, so a candidate that is never read is a candidate that never counts. Seven
/// members at this depth is the CVE scope answering a filtered query in a tenth of a second.
const CANDIDATES: usize = 20_000;

impl Tracker {
    /// Members are named, not pathed. A deployment holds its datasets in one place and a scope
    /// finds them there, because a dataset belongs to no scope and several may name it.
    pub fn open(dir: &Path, datasets: &Path) -> Result<Tracker, String> {
        let decl = TrackerDecl::load(dir)?;
        let registry = registry(datasets);
        let mut members = Vec::new();
        let mut missing = Vec::new();
        for m in &decl.members {
            // A member this deployment holds, or one somewhere else that answers the same six
            // calls. A scope cannot tell the difference except by where it was named.
            if let Some(url) = &m.remote {
                match crate::remote::Remote::open(url, m.key.clone()) {
                    Ok(r) => {
                        let described = Interface::describe(&r);
                        members.push(Resolved {
                            decl: SourceRef {
                                remote: Some(url.clone()),
                                key: m.key.clone(),
                                dataset: Interface::name(&r).to_string(),
                                priority: m.priority,
                                why: m.why.clone(),
                            },
                            member: Box::new(r),
                            described,
                        });
                    }
                    Err(e) => missing.push(format!("{url}: {e}")),
                }
                continue;
            }
            match registry.get(&m.dataset) {
                Some(path) => {
                    let ds = Source::open(path)?;
                    let described = Interface::describe(&ds);
                    members.push(Resolved {
                        decl: SourceRef {
                            remote: None,
                            key: None,
                            dataset: m.dataset.clone(),
                            priority: m.priority,
                            why: m.why.clone(),
                        },
                        member: Box::new(ds),
                        described,
                    });
                }
                // A scope that refused to open because one of five members was missing would be
                // less useful than one that says which four it has.
                None => missing.push(m.dataset.clone()),
            }
        }
        members.sort_by_key(|m| m.decl.priority.rank());
        Ok(Tracker {
            root: datasets.parent().unwrap_or(datasets).to_path_buf(),
            decl,
            members,
            missing,
        })
    }

    pub fn records(&self) -> u64 {
        self.members.iter().map(Resolved::records).sum()
    }

    pub fn kinds(&self) -> Vec<(String, u64)> {
        let mut out: BTreeMap<String, u64> = BTreeMap::new();
        for m in &self.members {
            *out.entry(m.kind()).or_default() += m.records();
        }
        out.into_iter().collect()
    }

    /// A member's field name for one of this scope's. Red Hat calls it `threat_severity`.
    fn field_in(&self, member: &str, scope_field: &str) -> String {
        match self.decl.normalise_for(scope_field) {
            Some(n) => n.field_in(member, scope_field),
            None => scope_field.to_string(),
        }
    }

    /// And back, so a member's answer can be shown under the name this scope uses.
    fn field_out(&self, member: &str, member_field: &str) -> String {
        for (scope_field, n) in &self.decl.normalise {
            if n.from.get(member).map(String::as_str) == Some(member_field) {
                return scope_field.clone();
            }
        }
        member_field.to_string()
    }

    fn rewrite(&self, pred: &Pred, member: &str) -> Pred {
        match pred {
            Pred::And(a, b) => Pred::And(
                Box::new(self.rewrite(a, member)),
                Box::new(self.rewrite(b, member)),
            ),
            Pred::Or(a, b) => Pred::Or(
                Box::new(self.rewrite(a, member)),
                Box::new(self.rewrite(b, member)),
            ),
            Pred::Cmp { left, op, right } => self.rewrite_cmp(member, left, *op, right),
        }
    }

    /// A comparison is made against the value an answer shows: the mapped one where the scope
    /// states a map. So the literal is translated back into the member's own words before it is
    /// asked, and `severity>=high` becomes the set of raw words that sit at or above `high` on the
    /// scale this scope declared. A filter and an answer that disagreed about what a record says
    /// would be two readings of one dataset.
    fn rewrite_cmp(&self, member: &str, left: &str, op: Op, right: &Lit) -> Pred {
        let their = self.field_in(member, left);
        let plain = || Pred::Cmp {
            left: their.clone(),
            op,
            right: right.clone(),
        };
        let Some(n) = self.decl.normalise_for(left) else {
            return plain();
        };

        let wanted: Vec<String> = match op {
            Op::Eq | Op::Ne => vec![right.display()],
            _ => {
                if n.scale.is_empty() {
                    return plain();
                }
                let Some(pos) = n.position(&right.display()) else {
                    return plain();
                };
                // The scale is best first, so `>= high` is everything at or before `high`.
                n.scale
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| match op {
                        Op::Ge => *i <= pos,
                        Op::Gt => *i < pos,
                        Op::Le => *i >= pos,
                        Op::Lt => *i > pos,
                        _ => false,
                    })
                    .map(|(_, v)| v.clone())
                    .collect()
            }
        };

        let raws: Vec<String> = match n.members.get(member) {
            Some(map) => map
                .iter()
                .filter(|(_, mapped)| wanted.iter().any(|w| w.eq_ignore_ascii_case(mapped)))
                .map(|(raw, _)| raw.clone())
                .collect(),
            None => Vec::new(),
        };
        // A value with no entry passes through unchanged, so an unmapped word is still asked for.
        let asked = if raws.is_empty() { wanted } else { raws };
        let Some(first) = asked.first() else {
            return plain();
        };

        let one = |v: &String, op: Op| Pred::Cmp {
            left: their.clone(),
            op,
            right: Lit::Str(v.clone()),
        };
        match op {
            Op::Ne => asked.iter().skip(1).fold(one(first, Op::Ne), |acc, v| {
                Pred::And(Box::new(acc), Box::new(one(v, Op::Ne)))
            }),
            _ => asked.iter().skip(1).fold(one(first, Op::Eq), |acc, v| {
                Pred::Or(Box::new(acc), Box::new(one(v, Op::Eq)))
            }),
        }
    }

    fn named_view(&self, name: &str) -> Option<&crate::trackerdecl::NamedView> {
        self.decl.view.named.iter().find(|v| v.name == name)
    }

    /// Five layers, each falling back to the one below it.
    pub fn columns(&self, q: &TrackerQuery) -> Vec<String> {
        if let Some(n) = q.named.as_deref().and_then(|n| self.named_view(n)) {
            if !n.columns.is_empty() {
                return n.columns.clone();
            }
        }
        if let Some(kind) = &q.kind {
            if let Some(k) = self.kind_view(kind) {
                if !k.columns.is_empty() {
                    return k.columns;
                }
            }
        }
        if !self.decl.view.columns.is_empty() {
            return self.decl.view.columns.clone();
        }
        // Derived: what more than one member carries, and what every record has.
        let mut tally: BTreeMap<String, usize> = BTreeMap::new();
        for m in &self.members {
            if let Some(fields) = m.described["properties"].as_array() {
                for f in fields {
                    if let Some(name) = f["name"].as_str() {
                        *tally.entry(self.field_out(m.name(), name)).or_default() += 1;
                    }
                }
            }
        }
        let mut shared: Vec<(String, usize)> = tally.into_iter().collect();
        shared.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut cols = vec!["kind".to_string()];
        cols.extend(shared.into_iter().take(3).map(|(n, _)| n));
        cols.push("known".into());
        cols
    }

    /// `adopt` resolves through the interface: the member already declared what its interesting
    /// subset is, and a curator who repeats it maintains the same statement twice.
    pub fn kind_view(&self, kind: &str) -> Option<KindView> {
        let declared = self.decl.view.kinds.get(kind)?;
        let Some(adopt) = &declared.adopt else {
            return Some(declared.clone());
        };
        let (dataset, view) = adopt.split_once(':')?;
        let m = self.members.iter().find(|m| m.name() == dataset)?;
        let v = m.view(view)?;
        let list = |key: &str| -> Vec<String> {
            v[key]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        Some(KindView {
            columns: if declared.columns.is_empty() {
                list("columns")
            } else {
                declared.columns.clone()
            },
            facets: if declared.facets.is_empty() {
                list("facets")
            } else {
                declared.facets.clone()
            },
            sort: declared
                .sort
                .clone()
                .or_else(|| v["sort"].as_str().map(str::to_string)),
            adopt: None,
        })
    }

    pub fn facets(&self, q: &TrackerQuery) -> Vec<String> {
        if let Some(n) = q.named.as_deref().and_then(|n| self.named_view(n)) {
            if !n.facets.is_empty() {
                return n.facets.clone();
            }
        }
        if let Some(kind) = &q.kind {
            if let Some(k) = self.kind_view(kind) {
                if !k.facets.is_empty() {
                    return k.facets;
                }
            }
        }
        if !self.decl.view.facets.is_empty() {
            return self.decl.view.facets.clone();
        }
        vec!["kind".into(), "source".into()]
    }

    /// Whether this member can select anything for this query at all. Free text goes to every
    /// member that has an index; a filter goes only to the members carrying a field it names.
    fn narrows(&self, q: &TrackerQuery, m: &Resolved) -> bool {
        if !q.text.trim().is_empty() {
            return true;
        }
        let named = q
            .named
            .as_deref()
            .and_then(|n| self.named_view(n))
            .and_then(|v| v.filter.as_deref())
            .and_then(expr::parse_pred);
        let by_field = q.pred.as_ref().and_then(|p| self.prune(p, m)).is_some()
            || named.as_ref().and_then(|p| self.prune(p, m)).is_some();
        // `dataset` is the scope's own word and no member carries it, so the prune drops it and
        // the member would never be read. The member it names is the one that narrows.
        by_field
            || q.pred.as_ref().is_some_and(|p| names_member(p, m.name()))
            || named.is_some_and(|p| names_member(&p, m.name()))
    }

    fn member_query(&self, q: &TrackerQuery, m: &Resolved, limit: usize) -> Query {
        let member = m.name();
        // The sort falls through the same chain the columns do.
        let sort = q.sort.clone().or_else(|| {
            q.named
                .as_deref()
                .and_then(|n| self.named_view(n))
                .and_then(|v| v.sort.clone())
                .or_else(|| {
                    q.kind
                        .as_deref()
                        .and_then(|k| self.kind_view(k))
                        .and_then(|k| k.sort)
                })
                .or_else(|| self.decl.view.sort.clone())
        });
        let mut pred = q
            .pred
            .as_ref()
            .and_then(|p| self.prune(p, m))
            .map(|p| self.rewrite(&p, member));
        if let Some(extra) = q
            .named
            .as_deref()
            .and_then(|n| self.named_view(n))
            .and_then(|v| v.filter.as_deref())
        {
            if let Some(p) = expr::parse_pred(extra)
                .and_then(|p| self.prune(&p, m))
                .map(|p| self.rewrite(&p, member))
            {
                pred = Some(match pred {
                    Some(a) => Pred::And(Box::new(a), Box::new(p)),
                    None => p,
                });
            }
        }
        Query {
            text: q.text.clone(),
            pred,
            view: None,
            sort,
            ids: Vec::new(),
            seen_before: q.seen_before.clone(),
            limit,
            offset: 0,
        }
    }

    pub fn search(&self, q: &TrackerQuery) -> Answer {
        let limit = if q.limit == 0 { 25 } else { q.limit };
        let want = limit + q.offset;
        let keys: Vec<&str> = self.decl.keys();

        // The whole predicate, known before anybody is asked, because how deep each member has to
        // be read depends on whether there is one.
        let full: Vec<Pred> = q
            .pred
            .clone()
            .into_iter()
            .chain(
                q.named
                    .as_deref()
                    .and_then(|n| self.named_view(n))
                    .and_then(|v| v.filter.as_deref())
                    .and_then(expr::parse_pred),
            )
            .collect();

        // With no predicate, a page is a page and each member's own total is the truth. With one,
        // the count is what survives the second pass over the assembled entry, so every candidate
        // that could survive has to be read: a member asked for fifty and filtered afterwards
        // reports the size of the page it was given, not the size of the answer.
        let depth = if full.is_empty() {
            want.max(50)
        } else {
            want.max(CANDIDATES)
        };

        // One pass to select, member by member, each answering the query in its own words. Only
        // the members that can narrow this query are read: a member carrying no field the query
        // names selects nothing, and the records it holds for the subjects the others selected are
        // fetched by identifier afterwards, by `complete`. Reading it here instead would be tens
        // of thousands of candidates read to be discarded.
        let mut per_member: Vec<Vec<Hit>> = Vec::new();
        let mut answered = Vec::new();
        let mut unanswered = Vec::new();
        let mut total = 0u64;
        let mut truncated = false;
        for m in &self.members {
            if let Some(kind) = &q.kind {
                if &m.kind() != kind {
                    per_member.push(Vec::new());
                    continue;
                }
            }
            if !full.is_empty() && !self.narrows(q, m) {
                answered.push(m.name().to_string());
                per_member.push(Vec::new());
                continue;
            }
            let mq = self.member_query(q, m, depth);
            match m.member.search(&mq) {
                Ok((n, hits, un)) => {
                    if un.0.is_empty() {
                        answered.push(m.name().to_string());
                    } else {
                        unanswered.push((m.name().to_string(), un.0.clone()));
                    }
                    total += n;
                    truncated |= hits.len() >= depth && (n as usize) > hits.len();
                    per_member.push(hits);
                }
                Err(e) => {
                    unanswered.push((m.name().to_string(), vec![e]));
                    per_member.push(Vec::new());
                }
            }
        }

        // The best remaining hit from each member in turn. Two members' scores are not on one
        // scale, so ranks are merged rather than scores, and `priority` breaks the tie.
        let mut order: Vec<(usize, Hit)> = Vec::new();
        let deepest = per_member.iter().map(Vec::len).max().unwrap_or(0);
        for i in 0..deepest {
            for (mi, hits) in per_member.iter().enumerate() {
                if let Some(h) = hits.get(i) {
                    order.push((mi, h.clone()));
                }
            }
        }

        // Gathered into entries on the joined key, whatever kind of record each one is.
        let mut entries: Vec<Thing> = Vec::new();
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for (mi, hit) in order {
            let key = hit
                .ids
                .iter()
                .find(|id| keys.iter().any(|k| *k == id.scheme))
                .cloned();
            let slot = match &key {
                Some(k) => {
                    let token = format!("{}:{}", k.scheme, k.value.to_lowercase());
                    match seen.get(&token) {
                        Some(i) => *i,
                        None => {
                            seen.insert(token, entries.len());
                            entries.push(self.new_entry(Some(k.clone()), &hit));
                            entries.len() - 1
                        }
                    }
                }
                None => {
                    entries.push(self.new_entry(None, &hit));
                    entries.len() - 1
                }
            };
            self.add_part(&mut entries[slot], &self.members[mi], &hit);
        }

        // Completed and folded before anything is paged, because the predicate is applied over
        // the entry and an entry that fails it must not take a slot on the page first.
        self.complete(&mut entries, &keys);
        for e in entries.iter_mut() {
            self.fold_fields(e);
        }
        // The whole predicate again, now that every member's words are in one place. A question
        // that spans sources is answered here or nowhere: no member holds both `exploited` and
        // `severity`, and asking each of them separately selects nothing.
        for p in &full {
            entries.retain(|e| self.entry_holds(e, p));
        }
        let matched = entries.len() as u64;

        let mut page: Vec<Thing> = entries.into_iter().skip(q.offset).take(limit).collect();
        for (i, e) in page.iter_mut().enumerate() {
            e.rank = q.offset + i + 1;
        }
        Answer {
            total: if full.is_empty() { total } else { matched },
            entries: page,
            answered,
            unanswered,
            // A filtered count read from a member that had more to give is a floor, not a total,
            // and the page says which of the two it is showing.
            truncated: truncated && !full.is_empty(),
            subjects: !full.is_empty(),
        }
    }

    fn new_entry(&self, key: Option<Id>, hit: &Hit) -> Thing {
        let mut why = hit.why_text.clone();
        if let Some(id) = &hit.why_id {
            why.push(format!("identifier {}", id.value));
        }
        Thing {
            key,
            rank: 0,
            title: hit.title.clone(),
            parts: Vec::new(),
            fields: BTreeMap::new(),
            why,
        }
    }

    fn add_part(&self, entry: &mut Thing, m: &Resolved, hit: &Hit) {
        if entry.parts.iter().any(|p| p.record_id == hit.record_id) {
            return;
        }
        let fields = hit
            .fields
            .iter()
            .map(|(k, v)| (self.field_out(m.name(), k), v.display()))
            .collect();
        entry.parts.push(ClaimRef {
            member: m.name().to_string(),
            priority: m.decl.priority.rank(),
            kind: hit.kind.clone(),
            record_id: hit.record_id.clone(),
            title: hit.title.clone(),
            url: hit.url.clone(),
            known: hit.known.clone(),
            fields,
        });
        // The title a reader sees is the one the scope calls primary.
        entry.parts.sort_by_key(|p| p.priority);
        if let Some(first) = entry.parts.first() {
            entry.title = first.title.clone();
        }
    }

    /// A member that did not answer the filter still has something to say about an entry the
    /// others selected. One round of identifier lookups completes every entry on the page.
    fn complete(&self, page: &mut [Thing], keys: &[&str]) {
        if keys.is_empty() || page.is_empty() {
            return;
        }
        let wanted: Vec<Id> = page.iter().filter_map(|e| e.key.clone()).collect();
        if wanted.is_empty() {
            return;
        }
        let values: Vec<String> = wanted.iter().map(|i| i.value.clone()).collect();
        for m in &self.members {
            if !m.can("ids") {
                continue;
            }
            let q = Query {
                text: String::new(),
                pred: None,
                view: None,
                sort: None,
                ids: values.clone(),
                seen_before: None,
                limit: values.len() * 8,
                offset: 0,
            };
            let Ok((_, hits, _)) = m.member.search(&q) else {
                continue;
            };
            for hit in hits {
                let Some(key) = hit
                    .ids
                    .iter()
                    .find(|id| keys.iter().any(|k| *k == id.scheme))
                    .cloned()
                else {
                    continue;
                };
                if let Some(e) = page
                    .iter_mut()
                    .find(|e| e.key.as_ref().is_some_and(|k| k.same(&key)))
                {
                    self.add_part(e, m, &hit);
                }
            }
        }
    }

    /// Per field, every value with the member that said it, its raw word beside the mapped one,
    /// and `divergent` computed after the map.
    fn fold_fields(&self, entry: &mut Thing) {
        let mut names: BTreeSet<String> = BTreeSet::new();
        for p in &entry.parts {
            names.extend(p.fields.keys().cloned());
        }
        for name in names {
            let mut by: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            let mut means: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            let n = self.decl.normalise_for(&name);
            for p in &entry.parts {
                let Some(raw) = p.fields.get(&name) else {
                    continue;
                };
                by.entry(p.member.clone()).or_default().insert(raw.clone());
                let mapped = match n {
                    Some(n) => n.means(&p.member, raw),
                    None => raw.clone(),
                };
                means.entry(p.member.clone()).or_default().insert(mapped);
            }
            if by.is_empty() {
                continue;
            }
            // Collected into sets and read out in order, so an entry is the same whatever order
            // its records arrived in. Keeping one value per member kept whichever arrived last,
            // and two stores holding the same records answered differently.
            let divergent = disagree(means.values());
            entry.fields.insert(
                name,
                PropertyView {
                    divergent,
                    mapped: n.is_some(),
                    by: by.into_iter().map(|(m, v)| (m, into_sorted(v))).collect(),
                    means: means
                        .into_iter()
                        .map(|(m, v)| (m, into_sorted(v)))
                        .collect(),
                },
            );
        }
    }

    /// Counted per value across every member, on the scale this scope declares where it has one.
    pub fn facet(&self, q: &TrackerQuery, field: &str, limit: usize) -> (Vec<(String, u64)>, u64) {
        let mut tally: BTreeMap<String, u64> = BTreeMap::new();
        let mut coverage = 0u64;
        for m in &self.members {
            if field == "kind" {
                *tally.entry(m.kind()).or_default() += m.records();
                coverage += m.records();
                continue;
            }
            if field == "source" {
                *tally.entry(m.name().to_string()).or_default() += m.records();
                coverage += m.records();
                continue;
            }
            let their = self.field_in(m.name(), field);
            let Some(n) = m.field_records(&their) else {
                continue;
            };
            coverage += n;
            let mq = self.member_query(q, m, 0);
            for (value, count) in m.member.facet(&mq, &their, limit * 3) {
                let shown = match self.decl.normalise_for(field) {
                    Some(nm) => nm.means(m.name(), &value),
                    None => value,
                };
                *tally.entry(shown).or_default() += count;
            }
        }
        let mut out: Vec<(String, u64)> = tally.into_iter().collect();
        // On the scale where the scope declares one, by count where it does not.
        match self.decl.normalise_for(field) {
            Some(n) if !n.scale.is_empty() => {
                out.sort_by_key(|(v, _)| n.position(v).unwrap_or(usize::MAX));
            }
            _ => out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0))),
        }
        out.truncate(limit);
        (out, coverage)
    }

    /// One subject, and everything any member says about it.
    pub fn entry(&self, scheme: &str, value: &str) -> Option<Thing> {
        let key = Id {
            scheme: scheme.to_string(),
            // As asked for. The lookup folds case; the key a page shows does not.
            value: value.to_string(),
        };
        let mut entry = Thing {
            key: Some(key.clone()),
            rank: 1,
            title: value.to_string(),
            parts: Vec::new(),
            fields: BTreeMap::new(),
            why: Vec::new(),
        };
        for m in &self.members {
            let q = Query {
                text: String::new(),
                pred: None,
                view: None,
                sort: None,
                ids: vec![key.value.clone()],
                seen_before: None,
                limit: 64,
                offset: 0,
            };
            let Ok((_, hits, _)) = m.member.search(&q) else {
                continue;
            };
            for hit in hits {
                self.add_part(&mut entry, m, &hit);
            }
        }
        if entry.parts.is_empty() {
            return None;
        }
        self.fold_fields(&mut entry);
        Some(entry)
    }

    pub fn records_of(&self, member: &str, ids: &[String]) -> Vec<Claim> {
        self.members
            .iter()
            .find(|m| m.name() == member)
            .map(|m| m.member.fetch(ids))
            .unwrap_or_default()
    }

    /// Everything a person needs before asking anything, and all of it from `describe`.
    pub fn describe(&self) -> J {
        let members: Vec<J> = self
            .members
            .iter()
            .map(|m| {
                json!({
                    "source": m.name(),
                    "priority": m.decl.priority.name(),
                    "why": m.decl.why,
                    "kind": m.kind(),
                    "claims": m.records(),
                    "state": m.state(),
                    "last_update": m.described["last_update"],
                    "can": m.described["can"],
                    "views": m.described["views"],
                })
            })
            .collect();
        json!({
            "tracker": self.decl.name,
            "title": self.decl.title,
            "about": self.decl.about,
            "claims": self.records(),
            "kinds": J::Array(self.kinds().iter()
                .map(|(k, n)| json!({ "kind": k, "claims": n })).collect()),
            "sources": J::Array(members),
            "missing": self.missing,
            "identified_by": self.decl.keys(),
            "align": J::Object(self.decl.normalise.iter()
                .map(|(k, v)| (k.clone(), json!({ "scale": v.scale, "from": v.from })))
                .collect()),
            "promise": json!({
                "fresh_within": self.decl.promise.fresh_within,
                "covers": self.decl.promise.covers,
                "excludes": self.decl.promise.excludes,
            }),
            "views": J::Array(self.decl.view.named.iter().map(|v| json!({
                "name": v.name,
                "title": if v.title.is_empty() { v.name.clone() } else { v.title.clone() },
                "where": v.filter,
            })).collect()),
        })
    }
}

/// Every dataset a deployment holds, by the name it calls itself.
pub fn registry(datasets: &Path) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(datasets) else {
        return out;
    };
    for e in entries.flatten() {
        let dir = e.path();
        if !dir.join(crate::sourcedecl::FILE).exists() {
            continue;
        }
        if let Ok(d) = crate::sourcedecl::SourceDecl::load(&dir) {
            out.insert(d.name, dir);
        }
    }
    out
}

/// A scope counts in its members' marks, because each of them runs on its own cadence.
fn parse_marks(raw: &str) -> BTreeMap<String, i64> {
    raw.split(',')
        .filter_map(|p| p.split_once('='))
        .filter_map(|(k, v)| Some((k.trim().to_string(), v.trim().parse().ok()?)))
        .collect()
}

impl Tracker {
    /// One mark per member, so a reader hands back exactly what they were last told.
    pub fn mark(&self) -> String {
        self.members
            .iter()
            .map(|m| format!("{}={}", m.name(), m.member.mark()))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The mark as it stood before each member's last run, which is what "since I last looked"
    /// means when nobody has looked yet.
    pub fn mark_before(&self) -> String {
        self.members
            .iter()
            .map(|m| format!("{}={}", m.name(), (m.member.mark() - 1).max(0)))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// What entered, what left, and what changed inside an entry, field by field.
    pub fn changes(&self, since: &str, limit: usize) -> J {
        let marks = parse_marks(since);
        let keys = self.decl.keys();
        let mut entries: Vec<(Option<Id>, String, Vec<J>)> = Vec::new();
        let mut index: BTreeMap<String, usize> = BTreeMap::new();
        let mut members_without_history = Vec::new();

        for m in &self.members {
            let mark = marks.get(m.name()).copied().unwrap_or(0);
            let report = m.member.changes(mark, limit);
            if report["history"] == J::Bool(false) {
                members_without_history.push(m.name().to_string());
            }
            let empty = Vec::new();
            for list in ["changed", "removed"] {
                for change in report[list].as_array().unwrap_or(&empty) {
                    let ids = crate::store::parse_ids(&change["ids"].to_string());
                    let key = ids
                        .into_iter()
                        .find(|id| keys.iter().any(|k| *k == id.scheme));
                    let title = change["title"].as_str().unwrap_or("").to_string();
                    let mut one = change.clone();
                    one["source"] = json!(m.name());
                    let token = match &key {
                        Some(k) => format!("{}:{}", k.scheme, k.value.to_lowercase()),
                        None => format!("{}#{}", m.name(), change["claim_id"]),
                    };
                    match index.get(&token) {
                        Some(i) => entries[*i].2.push(one),
                        None => {
                            index.insert(token, entries.len());
                            entries.push((key, title, vec![one]));
                        }
                    }
                }
            }
        }

        json!({
            "tracker": self.decl.name,
            "since": since,
            "mark": self.mark(),
            "without_history": members_without_history,
            "things": J::Array(entries.iter().map(|(key, title, changes)| json!({
                "identifier": key.as_ref().map(|k| json!({ "scheme": k.scheme, "value": k.value })),
                "title": title,
                "sources": J::Array(changes.iter()
                    .map(|c| c["source"].clone()).collect()),
                "changes": J::Array(changes.clone()),
            })).collect()),
        })
    }
}

/// Every scope a deployment holds, by the name it calls itself. A watch names a scope, not a path.
pub fn scope_registry(scopes: &Path) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(scopes) else {
        return out;
    };
    for e in entries.flatten() {
        let dir = e.path();
        if !dir.join(crate::trackerdecl::FILE).exists() {
            continue;
        }
        if let Ok(d) = TrackerDecl::load(&dir) {
            out.insert(d.name, dir);
        }
    }
    out
}

impl Tracker {
    /// Checked against when each member last finished a run, complete or not: this is a claim
    /// about freshness, not about coverage. What a run reached is a separate fact and the
    /// member's state carries it. A scope that is stale says so on its own front page, rather
    /// than answering with less in it and saying nothing.
    pub fn promise(&self) -> (bool, Vec<String>) {
        let Some(within) = self.decl.promise.fresh_within.as_deref() else {
            return (true, Vec::new());
        };
        let Some(seconds) = crate::fetch::duration(within) else {
            return (true, Vec::new());
        };
        let now = crate::now();
        let mut late = Vec::new();
        for m in &self.members {
            let finished = m.described["last_update"]["finished"]
                .as_str()
                .map(crate::fetch::seconds_of);
            match finished {
                Some(at) if now - at <= seconds => {}
                Some(at) => late.push(format!(
                    "{} last finished {} ago",
                    m.name(),
                    human(now - at)
                )),
                None => late.push(format!("{} has not completed an update", m.name())),
            }
        }
        (late.is_empty(), late)
    }

    /// How long ago the member that finished least recently finished, in seconds. `None` where
    /// any member has never completed a run, because then there is no oldest to name.
    pub fn oldest_finish(&self) -> Option<i64> {
        let now = crate::now();
        let mut oldest: Option<i64> = None;
        for m in &self.members {
            let at = m.described["last_update"]["finished"]
                .as_str()
                .map(crate::fetch::seconds_of)?;
            let age = now - at;
            if oldest.map(|o| age > o).unwrap_or(true) {
                oldest = Some(age);
            }
        }
        oldest
    }
}

pub fn human(seconds: i64) -> String {
    match seconds {
        s if s < 90 => format!("{s}s"),
        s if s < 5400 => format!("{}m", s / 60),
        s if s < 172_800 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

impl Tracker {
    /// What this scope actually holds, counted rather than claimed.
    ///
    /// Every member is paged through the same `search` a reader uses, so the numbers are the ones
    /// an answer would give. A measurement taken from the store behind the interface would be a
    /// measurement of something nobody can ask for.
    pub fn measure(&self) -> J {
        let keys = self.decl.keys();
        // key value -> member -> (kinds, field -> (what it said, what this scope makes of it)).
        // Every record a member holds for the subject, as the entry page shows them: keeping the
        // last one measured a different thing from the one a reader sees.
        type Words = (BTreeSet<String>, BTreeSet<String>);
        type Said = BTreeMap<String, (BTreeSet<String>, BTreeMap<String, Words>)>;
        let mut subjects: BTreeMap<String, Said> = BTreeMap::new();
        let mut per_member = Vec::new();

        for m in &self.members {
            let mut seen = 0u64;
            let mut offset = 0usize;
            loop {
                let q = Query {
                    text: String::new(),
                    pred: None,
                    view: None,
                    ids: Vec::new(),
                    seen_before: None,
                    sort: None,
                    limit: 5000,
                    offset,
                };
                let Ok((_, hits, _)) = m.member.search(&q) else {
                    break;
                };
                if hits.is_empty() {
                    break;
                }
                for hit in &hits {
                    seen += 1;
                    let Some(key) = hit
                        .ids
                        .iter()
                        .find(|id| keys.iter().any(|k| *k == id.scheme))
                    else {
                        continue;
                    };
                    // Every field, under the name this scope shows it by. A measurement
                    // of one field is a measurement of the field somebody guessed.
                    // Compared with the case folded, as the entry is gathered: `cve-2021-44228`
                    // and `CVE-2021-44228` are one subject there and must be one here, and so is the scheme.
                    let said = subjects
                        .entry(format!("{}:{}", key.scheme, key.value.to_lowercase()))
                        .or_default()
                        .entry(m.name().to_string())
                        .or_default();
                    said.0.insert(hit.kind.clone());
                    for (name, value) in &hit.fields {
                        let scope_name = self.field_out(m.name(), name);
                        let raw = value.display();
                        let mapped = match self.decl.normalise_for(&scope_name) {
                            Some(n) => n.means(m.name(), &raw),
                            None => raw.clone(),
                        };
                        let words = said.1.entry(scope_name).or_default();
                        words.0.insert(raw);
                        words.1.insert(mapped);
                    }
                }
                offset += hits.len();
                if hits.len() < 5000 {
                    break;
                }
            }
            per_member.push(json!({ "source": m.name(), "kind": m.kind(), "claims": seen }));
        }

        let mut by_count: BTreeMap<usize, u64> = BTreeMap::new();
        let mut with_exploit = 0u64;
        // field -> (carried by two or more, differ in words, differ after the map)
        let mut fields: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();

        for members in subjects.values() {
            *by_count.entry(members.len()).or_default() += 1;
            if members.values().any(|(kinds, _)| kinds.contains("exploit")) {
                with_exploit += 1;
            }
            let mut names: BTreeSet<&String> = BTreeSet::new();
            for (_, said) in members.values() {
                names.extend(said.keys());
            }
            for name in names {
                let carried: Vec<&Words> = members
                    .values()
                    .filter_map(|(_, s)| s.get(name))
                    .collect();
                if carried.len() < 2 {
                    continue;
                }
                let e = fields.entry(name.clone()).or_default();
                e.0 += 1;
                if disagree(carried.iter().map(|w| &w.0)) {
                    e.1 += 1;
                }
                if disagree(carried.iter().map(|w| &w.1)) {
                    e.2 += 1;
                }
            }
        }

        json!({
            "tracker": self.decl.name,
            "sources": J::Array(per_member),
            "things": subjects.len(),
            "by_sources": J::Object(by_count.iter()
                .map(|(n, c)| (n.to_string(), json!(c))).collect()),
            "with_an_exploit": with_exploit,
            "properties": J::Object(fields.iter().map(|(name, (two, raw, mapped))| {
                (name.clone(), json!({
                    "said_by_two_or_more": two,
                    "differ_in_words": raw,
                    "differ_after_the_map": mapped,
                    "normalised": self.decl.normalise_for(name).is_some(),
                }))
            }).collect()),
        })
    }
}

impl Tracker {
    /// What this member can be asked of a predicate, and nothing more.
    ///
    /// A filter is pushed down so a member does the narrowing it can do, and a conjunct it cannot
    /// answer is dropped rather than sent: a member that holds the exploit for an entry another
    /// member selected still has to be reachable. What is dropped here is applied again over the
    /// assembled entry, where every member's words are in one place.
    fn prune(&self, pred: &Pred, m: &Resolved) -> Option<Pred> {
        match pred {
            Pred::And(a, b) => match (self.prune(a, m), self.prune(b, m)) {
                (Some(x), Some(y)) => Some(Pred::And(Box::new(x), Box::new(y))),
                (Some(x), None) | (None, Some(x)) => Some(x),
                (None, None) => None,
            },
            // Dropping one arm of an `or` would narrow rather than widen, so it goes whole or not
            // at all.
            Pred::Or(a, b) => match (self.prune(a, m), self.prune(b, m)) {
                (Some(x), Some(y)) => Some(Pred::Or(Box::new(x), Box::new(y))),
                _ => None,
            },
            Pred::Cmp { left, .. } => {
                if matches!(left.as_str(), "known" | "title" | "kind" | "url" | "id") {
                    return Some(pred.clone());
                }
                m.field_records(&self.field_in(m.name(), left))
                    .map(|_| pred.clone())
            }
        }
    }

    /// The whole predicate again, over the values the entry shows. A question that spans sources is
    /// answered here or nowhere: no member holds both `exploited` and `severity`.
    pub fn entry_holds(&self, entry: &Thing, pred: &Pred) -> bool {
        match pred {
            Pred::And(a, b) => self.entry_holds(entry, a) && self.entry_holds(entry, b),
            Pred::Or(a, b) => self.entry_holds(entry, a) || self.entry_holds(entry, b),
            Pred::Cmp { left, op, right } => {
                // What an entry is, as against what its members say about it. These four are the
                // same names a member answers them under, and they pass the prune, so a query on
                // one of them reaches here and has to be answered rather than dropped.
                let want = right.display();
                match left.as_str() {
                    "kind" => return entry.parts.iter().any(|p| cmp_str(&p.kind, op, &want)),
                    "source" => return entry.parts.iter().any(|p| cmp_str(&p.member, op, &want)),
                    "title" => {
                        return cmp_str(&entry.title, op, &want)
                            || entry.parts.iter().any(|p| cmp_str(&p.title, op, &want))
                    }
                    "known" => return entry.parts.iter().any(|p| cmp_str(&p.known, op, &want)),
                    "url" => {
                        return entry
                            .parts
                            .iter()
                            .any(|p| p.url.as_deref().is_some_and(|u| cmp_str(u, op, &want)))
                    }
                    // Stored as the source wrote it, compared with the case folded.
                    "id" => {
                        return entry.key.iter().any(|k| cmp_str(&k.value, op, &want))
                            || entry.parts.iter().any(|p| cmp_str(&p.record_id, op, &want));
                    }
                    _ => {}
                }
                let Some(field) = entry.fields.get(left) else {
                    return false;
                };
                let scale = self
                    .decl
                    .normalise_for(left)
                    .filter(|n| !n.scale.is_empty());
                field.means.values().flatten().any(|v| match scale {
                    // On a declared scale, best first, so `>= high` is a smaller index.
                    Some(n) => match (n.position(v), n.position(&right.display())) {
                        (Some(a), Some(b)) => match op {
                            Op::Eq => a == b,
                            Op::Ne => a != b,
                            Op::Ge => a <= b,
                            Op::Gt => a < b,
                            Op::Le => a >= b,
                            Op::Lt => a > b,
                        },
                        _ => v.eq_ignore_ascii_case(&right.display()),
                    },
                    None => {
                        let ord = match (v.parse::<f64>(), right.display().parse::<f64>()) {
                            (Ok(a), Ok(b)) => a.partial_cmp(&b),
                            _ => Some(v.to_lowercase().cmp(&right.display().to_lowercase())),
                        };
                        match ord {
                            Some(o) => match op {
                                Op::Eq => o.is_eq(),
                                Op::Ne => o.is_ne(),
                                Op::Lt => o.is_lt(),
                                Op::Le => o.is_le(),
                                Op::Gt => o.is_gt(),
                                Op::Ge => o.is_ge(),
                            },
                            None => false,
                        }
                    }
                })
            }
        }
    }
}

impl Tracker {
    /// How many records a viewer under this bound can reach. The paywall is a bound on a query,
    /// so what it hides is the difference between two counts rather than a rule somewhere else.
    pub fn reachable(&self, bound: Option<&str>) -> u64 {
        let q = TrackerQuery {
            limit: 1,
            seen_before: bound.map(str::to_string),
            ..Default::default()
        };
        self.members
            .iter()
            .filter_map(|m| m.member.search(&self.member_query(&q, m, 1)).ok())
            .map(|(total, _, _)| total)
            .sum()
    }
}

impl Tracker {
    /// What a scope claims, held against what its members actually answer.
    pub fn check(&self) -> Vec<String> {
        let mut wrong = Vec::new();
        for name in &self.missing {
            wrong.push(format!("{name} is named and not installed here"));
        }
        if self.members.is_empty() {
            wrong.push("no source resolved, so there is nothing to check".into());
            return wrong;
        }

        // A join key only one member carries joins nothing. The scope still answers, and every
        // entry is one record, which looks like a working scope and is not one.
        for key in self.decl.keys() {
            let carrying: Vec<&str> = self
                .members
                .iter()
                .filter(|m| {
                    m.described["schemes"]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|s| s["scheme"].as_str() == Some(key)))
                })
                .map(|m| m.name())
                .collect();
            match carrying.len() {
                0 => wrong.push(format!("the identifier {key} is carried by no source")),
                1 => wrong.push(format!(
                    "the join key {key} is carried only by {}, so it joins nothing",
                    carrying[0]
                )),
                _ => {}
            }
        }

        for (field, n) in &self.decl.normalise {
            for (member, their) in &n.from {
                match self.members.iter().find(|m| m.name() == member) {
                    None => wrong.push(format!(
                        "align.{field} names {member}, which is not a source here"
                    )),
                    Some(m) if m.field_records(their).is_none() => wrong.push(format!(
                        "align.{field} reads {their} from {member}, which has no such property"
                    )),
                    _ => {}
                }
            }
            for member in n.members.keys() {
                if !self.members.iter().any(|m| m.name() == member) {
                    wrong.push(format!(
                        "align.{field} maps {member}, which is not a source here"
                    ));
                }
            }
        }

        for v in &self.decl.view.named {
            if let Some(filter) = &v.filter {
                match crate::expr::parse_pred(filter) {
                    None => wrong.push(format!(
                        "the view {:?} has a `where` nothing can parse",
                        v.name
                    )),
                    Some(p) => {
                        let q = TrackerQuery {
                            named: Some(v.name.clone()),
                            limit: 1,
                            ..Default::default()
                        };
                        let _ = p;
                        if self.search(&q).entries.is_empty() {
                            wrong.push(format!("the view {:?} selects nothing", v.name));
                        }
                    }
                }
            }
        }

        for kind in self.decl.view.kinds.keys() {
            if !self.kinds().iter().any(|(k, _)| k == kind) {
                wrong.push(format!(
                    "there is a view for {kind}, and no source holds one"
                ));
            }
        }

        if self.decl.promise.covers.trim().is_empty() {
            wrong.push("the promise says nothing about what is covered".into());
        }
        let (holds, late) = self.promise();
        if !holds {
            wrong.push(format!("the promise does not hold: {}", late.join("; ")));
        }
        wrong
    }
}

/// One comparison between two strings, case folded, for the four things an entry is rather than
/// says. A date and a title order the same way here: lexically, which is what an ISO date wants.
/// Whether members disagree: two of them, each with everything it said, and not the same. One
/// member saying two things is not a disagreement with itself: two exploits for one CVE on two
/// platforms are two statements by one publisher, and counting them as a conflict made one.
fn disagree<'a, S: PartialEq + 'a>(mut said: impl Iterator<Item = &'a S>) -> bool {
    match said.next() {
        Some(first) => said.any(|s| s != first),
        None => false,
    }
}

fn cmp_str(have: &str, op: &Op, want: &str) -> bool {
    let o = have.to_lowercase().cmp(&want.to_lowercase());
    match op {
        Op::Eq => o.is_eq(),
        Op::Ne => o.is_ne(),
        Op::Lt => o.is_lt(),
        Op::Le => o.is_le(),
        Op::Gt => o.is_gt(),
        Op::Ge => o.is_ge(),
    }
}

/// Whether a predicate has a `dataset` clause this member satisfies. A member the query excludes
/// is not read; a member it may include is.
fn names_member(pred: &Pred, member: &str) -> bool {
    match pred {
        Pred::And(a, b) | Pred::Or(a, b) => names_member(a, member) || names_member(b, member),
        Pred::Cmp { left, op, right } => left == "source" && cmp_str(member, op, &right.display()),
    }
}

/// A set, read out in its own order, which is the order that does not depend on arrival.
fn into_sorted(values: std::collections::BTreeSet<String>) -> Vec<String> {
    values.into_iter().collect()
}
