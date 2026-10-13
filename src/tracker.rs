//! A tracker. It rewrites a query per source, fans out, merges ranked lists, gathers the hits into
//! things, and renders. A filter of clicked values is the exception: its own store (`thingstore`)
//! already holds every thing and what each source says, and answers it by index.

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
    /// The name a person gave it, or its handle.
    pub fn title(&self) -> String {
        self.described["title"].as_str().filter(|t| !t.is_empty()).unwrap_or(self.member.name()).to_string()
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
    /// How many of this source's claims carry a field, for the coverage beside a facet.
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
    /// The directory the tracker is, where its own store is kept beside its declaration.
    pub dir: PathBuf,
    /// The workspace this tracker was opened in, where its watches live.
    pub root: PathBuf,
    pub decl: TrackerDecl,
    pub members: Vec<Resolved>,
    pub missing: Vec<String>,
    /// The things a person confirmed are one with another: the folded thing, the one it is now.
    /// Read again at every refresh, so a match confirmed on a page counts in the refresh it asks for.
    pub folds: std::sync::RwLock<BTreeMap<String, String>>,
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
    /// The paywall, applied to every source alike.
    pub seen_before: Option<String>,
}

/// One source's claim inside a thing.
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

/// Per field, every value with the source that said it, and what this tracker makes of it.
pub struct PropertyView {
    /// Per source, every distinct value that source said, sorted. A source can say a field
    /// several times for one thing: four quantisations of one model carry four licences, and
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
    /// Grouped by kind, and ordered by the tracker's priority inside each group.
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
    /// The total is a floor: a source had more candidates than were read, and the second pass
    /// could only count what it saw.
    pub truncated: bool,
    /// What the total counts. Without a predicate it is claims, summed from what each source
    /// says it holds. With one it is things, counted after the pass over assembled things,
    /// because that pass is the only place the question can be answered.
    pub subjects: bool,
}

/// How deep each source is read when the query carries a predicate. The second pass runs over
/// assembled things, so a candidate that is never read is a candidate that never counts. Seven
/// sources at this depth is the CVE tracker answering a filtered query in a tenth of a second.
const CANDIDATES: usize = 20_000;

impl Tracker {
    /// Sources are named, not pathed. A workspace holds its sources in one place and a tracker
    /// finds them there, because a source belongs to no tracker and several may name it.
    pub fn open(dir: &Path, datasets: &Path) -> Result<Tracker, String> {
        let decl = TrackerDecl::load(dir)?;
        let registry = registry(datasets);
        let mut members = Vec::new();
        let mut missing = Vec::new();
        for m in &decl.members {
            // A source this workspace holds, or one somewhere else that answers the same six
            // calls. A tracker cannot tell the difference except by where it was named.
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
                // A tracker that refused to open because one of five sources was missing would be
                // less useful than one that says which four it has.
                None => missing.push(m.dataset.clone()),
            }
        }
        members.sort_by_key(|m| m.decl.priority.rank());
        Ok(Tracker {
            dir: dir.to_path_buf(),
            root: datasets.parent().unwrap_or(datasets).to_path_buf(),
            folds: std::sync::RwLock::new(folds(dir)),
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

    /// A source's field name for one of this tracker's. Red Hat calls it `threat_severity`.
    fn field_in(&self, member: &str, scope_field: &str) -> String {
        match self.decl.normalise_for(scope_field) {
            Some(n) => n.field_in(member, scope_field),
            None => scope_field.to_string(),
        }
    }

    /// And back, so a source's answer can be shown under the name this tracker uses.
    pub fn field_out(&self, member: &str, member_field: &str) -> String {
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

    /// A comparison is made against the value an answer shows: the mapped one where the tracker
    /// states a map. So the literal is translated back into the source's own words before it is
    /// asked, and `severity>=high` becomes the set of raw words that sit at or above `high` on the
    /// scale this tracker declared. A filter and an answer that disagreed about what a claim says
    /// would be two readings of one source.
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
        // A value with no thing passes through unchanged, so an unmapped word is still asked for.
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
        // Derived: what more than one source carries, and what every claim has.
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
        // What a reader looks at first: shared by the most sources, and then what says something
        // about the thing (its price, its tags, its date) before what names it again (an id, a
        // key; the thing's own name is under its title already). One column per sort of thing.
        const TELLING: [&str; 11] = ["price", "tag", "genre", "status", "severity", "score", "version", "category", "rating", "platform", "type"];
        // A date is the date column already.
        let naming = |n: &str| n.ends_with("id") || n.ends_with("ids") || n.contains("key") || n.ends_with("_url") || n == "href" || n.contains("date") || n.contains("released");
        let telling = |n: &str| TELLING.iter().position(|w| n.contains(w));
        let mut shared: Vec<(String, usize)> = tally.into_iter().collect();
        shared.sort_by_key(|(n, count)| (std::cmp::Reverse(*count), naming(n), telling(n).is_none(), n.len(), n.clone()));
        let mut cols = vec!["kind".to_string()];
        let mut sorts: Vec<usize> = Vec::new();
        for (n, _) in shared {
            if cols.len() >= 4 {
                break;
            }
            if let Some(t) = telling(&n) {
                if sorts.contains(&t) {
                    continue;
                }
                sorts.push(t);
            }
            cols.push(n);
        }
        cols.push("known".into());
        cols
    }

    /// `adopt` resolves through the interface: the source already declared what its interesting
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

    /// Whether this source can select anything for this query at all. Free text goes to every
    /// source that has an index; a filter goes only to the sources carrying a field it names.
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
        // `source` is the tracker's own word and no source carries it, so the prune drops it and
        // the source would never be read. The source it names is the one that narrows.
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

        // The whole predicate, known before anybody is asked, because how deep each source has to
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

        // A filter of the kinds a reader clicks (a value of a property, a source, a kind) is
        // answered from the tracker's own store, where every thing and what each source says of
        // it is already one row: the count and the page by index, and only the page's things
        // assembled. Reading every source's candidates and assembling them all, up to twenty
        // thousand from each, took most of a second and most of the memory per filtered page.
        if let Some(answer) = self.indexed(q, &full, limit) {
            return answer;
        }

        // With no predicate, a page is a page and each source's own total is the truth. With one,
        // the count is what survives the second pass over the assembled thing, so every candidate
        // that could survive has to be read: a source asked for fifty and filtered afterwards
        // reports the size of the page it was given, not the size of the answer.
        let depth = if full.is_empty() {
            want.max(50)
        } else {
            want.max(CANDIDATES)
        };

        // One pass to select, source by source, each answering the query in its own words. Only
        // the sources that can narrow this query are read: a source carrying no field the query
        // names selects nothing, and the claims it holds for the things the others selected are
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

        // The best remaining hit from each source in turn. Two sources' scores are not on one
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

        // Gathered into things on the joined key, whatever kind of claim each one is.
        let mut entries: Vec<Thing> = Vec::new();
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for (mi, hit) in order {
            let about = about(&keys, &hit.ids);
            if about.is_empty() {
                entries.push(self.new_entry(None, &hit));
                let slot = entries.len() - 1;
                self.add_part(&mut entries[slot], &self.members[mi], &hit);
                continue;
            }
            // Into every thing it is about: an exploit for two CVEs is part of both.
            for k in about {
                let k = &self.kept(k);
                let token = crate::schemes::key(&k.scheme, &k.value);
                let slot = match seen.get(&token) {
                    Some(i) => *i,
                    None => {
                        seen.insert(token, entries.len());
                        entries.push(self.new_entry(Some(k.clone()), &hit));
                        entries.len() - 1
                    }
                };
                self.add_part(&mut entries[slot], &self.members[mi], &hit);
            }
        }

        // Completed and folded before anything is paged, because the predicate is applied over
        // the thing and a thing that fails it must not take a slot on the page first.
        self.complete(&mut entries, &keys);
        for e in entries.iter_mut() {
            self.fold_fields(e);
        }
        // The whole predicate again, now that every source's words are in one place. A question
        // that spans sources is answered here or nowhere: no source holds both `exploited` and
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
            // A filtered count read from a source that had more to give is a floor, not a total,
            // and the page says which of the two it is showing.
            truncated: truncated && !full.is_empty(),
            subjects: !full.is_empty(),
        }
    }

    /// The filtered page from the tracker's store, where every clause is one it answers exactly:
    /// `property=value`, `source=…`, `kind=…`, joined by `and`, with no words to search for. `None`
    /// for anything else (and where there is no store yet), which the sources answer as before.
    fn indexed(&self, q: &TrackerQuery, full: &[Pred], limit: usize) -> Option<Answer> {
        if !q.text.trim().is_empty() || full.is_empty() || !self.dir.join("tracker.db").exists() {
            return None;
        }
        // Each clause as SQL over one thing `t`, with its parameters.
        let mut clauses: Vec<String> = Vec::new();
        let mut params: Vec<String> = Vec::new();
        fn conjuncts<'a>(p: &'a Pred, out: &mut Vec<&'a Pred>) -> bool {
            match p {
                Pred::And(a, b) => conjuncts(a, out) && conjuncts(b, out),
                Pred::Or(..) => false,
                cmp => {
                    out.push(cmp);
                    true
                }
            }
        }
        let mut cmps = Vec::new();
        for p in full {
            if !conjuncts(p, &mut cmps) {
                return None;
            }
        }
        for p in cmps {
            let Pred::Cmp { left, op, right } = p else { return None };
            let want = right.display();
            // An order on a declared scale is a set of its words: `severity>=high` is critical or
            // high, best first, as `entry_holds` reads it. Any other order goes to the sources.
            if *op != Op::Eq {
                let scale = self.decl.normalise_for(left).filter(|n| !n.scale.is_empty())?;
                let at = scale.position(&want)?;
                let words: Vec<String> = scale
                    .scale
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| match op {
                        Op::Ge => *i <= at,
                        Op::Gt => *i < at,
                        Op::Le => *i >= at,
                        Op::Lt => *i > at,
                        Op::Ne => *i != at,
                        Op::Eq => *i == at,
                    })
                    .map(|(_, w)| w.to_lowercase())
                    .collect();
                if words.is_empty() || *op == Op::Ne {
                    // Not one of them, or every word but one, which a thing with no word at all
                    // also is: the sources say that better than an index of words.
                    return None;
                }
                clauses.push(format!("t.key in (select key from word where property = ? and word in ({}))", vec!["?"; words.len()].join(", ")));
                params.push(left.to_string());
                params.extend(words);
                continue;
            }
            match left.as_str() {
                "kind" => clauses.push("t.key in (select key from speaks where lower(kind) = lower(?))".into()),
                "source" => clauses.push("t.key in (select key from speaks where lower(source) = lower(?))".into()),
                // Answered over the claims themselves, which the store does not keep word for word.
                "title" | "known" | "url" | "id" => return None,
                field => {
                    // A number is compared as a number (9.80 is 9.8), which the store's words are
                    // not; those go to the sources.
                    if matches!(right, Lit::Num(_)) || want.parse::<f64>().is_ok() {
                        return None;
                    }
                    clauses.push("t.key in (select key from word where property = ? and word = ?)".into());
                    params.push(field.to_string());
                    params.push(want.to_lowercase());
                    continue;
                }
            }
            params.push(want);
        }
        if let Some(kind) = &q.kind {
            clauses.push("t.key in (select key from speaks where lower(kind) = lower(?))".into());
            params.push(kind.clone());
        }
        // The paywall: a thing a free reader may see is one some source spoke of before it.
        if let Some(edge) = &q.seen_before {
            clauses.push("t.key in (select key from speaks where first_seen <= ?)".into());
            params.push(edge.clone());
        }
        let store = crate::thingstore::ThingStore::open(&self.dir).ok()?;
        // Only a store whose words are written answers by them.
        store.meta("words")?;
        let wher = clauses.join(" and ");
        let total: i64 = store
            .db
            .query_row(&format!("select count(*) from thing t where {wher}"), rusqlite::params_from_iter(params.iter()), |r| r.get(0))
            .ok()?;
        let mut page_params = params.clone();
        page_params.push(limit.to_string());
        page_params.push(q.offset.to_string());
        let ids: Vec<Id> = {
            let mut stmt = store
                .db
                .prepare(&format!("select scheme, value from thing t where {wher} order by t.changed desc, t.key limit cast(? as integer) offset cast(? as integer)"))
                .ok()?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(page_params.iter()), |r| Ok(Id { scheme: r.get(0)?, value: r.get(1)? }))
                .ok()?;
            rows.flatten().collect()
        };
        // Only the page is assembled: each thing's claims fetched by its identifier from every
        // source, folded, and held against the filter once more, as a page from the sources is.
        let mut entries: Vec<Thing> = ids
            .into_iter()
            .map(|id| Thing { key: Some(id), rank: 0, title: String::new(), parts: Vec::new(), fields: BTreeMap::new(), why: Vec::new() })
            .collect();
        let keys = self.decl.keys();
        self.complete(&mut entries, &keys);
        for e in entries.iter_mut() {
            self.fold_fields(e);
        }
        entries.retain(|e| !e.parts.is_empty() && full.iter().all(|p| self.entry_holds(e, p)));
        for (i, e) in entries.iter_mut().enumerate() {
            e.rank = q.offset + i + 1;
        }
        Some(Answer {
            total: total.max(0) as u64,
            entries,
            answered: self.members.iter().map(|m| m.name().to_string()).collect(),
            unanswered: Vec::new(),
            truncated: false,
            subjects: true,
        })
    }

    /// A thing's title as the tracker makes it from what its sources say, where it declares how;
    /// its highest source's own otherwise. The same the store keeps, so a list and a page agree.
    pub fn title_of(&self, e: &Thing) -> String {
        let template = &self.decl.thing.title;
        if template.is_empty() {
            return e.title.clone();
        }
        let order: Vec<&str> = self.members.iter().map(|m| m.name()).collect();
        let said = |p: &str| -> Vec<String> {
            e.fields.get(p).map(|f| order.iter().filter_map(|m| f.means.get(*m).or_else(|| f.by.get(*m))).flatten().cloned().collect()).unwrap_or_default()
        };
        let value = e.key.as_ref().map(|k| k.value.clone()).unwrap_or_default();
        crate::display::title(template, &self.decl, &said, &value, &e.title).unwrap_or_else(|| e.title.clone())
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
        // The title a reader sees is the one the tracker calls primary.
        entry.parts.sort_by_key(|p| p.priority);
        if let Some(first) = entry.parts.first() {
            entry.title = first.title.clone();
        }
    }

    /// A source that did not answer the filter still has something to say about a thing the
    /// others selected. One round of identifier lookups completes every thing on the page.
    fn complete(&self, page: &mut [Thing], keys: &[&str]) {
        if keys.is_empty() || page.is_empty() {
            return;
        }
        let wanted: Vec<Id> = page.iter().filter_map(|e| e.key.clone()).collect();
        if wanted.is_empty() {
            return;
        }
        let mut values: Vec<String> = wanted.iter().map(|i| i.value.clone()).collect();
        for w in &wanted {
            values.extend(self.folded_into(w));
        }
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
                    .map(|id| self.kept(id))
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

    /// Per field, every value with the source that said it, its raw word beside the mapped one,
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
            // Collected into sets and read out in order, so a thing is the same whatever order
            // its claims arrived in. Keeping one value per source kept whichever arrived last,
            // and two stores holding the same claims answered differently.
            // Numbers within the tracker's tolerance agree here as they do in its store: 3.39 and
            // 3.4 are one unemployment rate where it allows 0.3.
            let tolerance = n.map(|a| a.number_tolerance()).unwrap_or(0.0);
            let numbers: Option<Vec<Vec<f64>>> = (tolerance > 0.0)
                .then(|| means.values().map(|v| v.iter().map(|s| s.parse::<f64>().ok()).collect::<Option<Vec<f64>>>()).collect::<Option<Vec<_>>>())
                .flatten();
            let divergent = match numbers {
                Some(sets) => sets.iter().enumerate().any(|(i, a)| {
                    sets.iter().skip(i + 1).any(|b| a.iter().any(|x| b.iter().any(|y| (x - y).abs() > tolerance + f64::EPSILON)))
                }),
                None => disagree(means.values()),
            };
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

    /// Counted per value across every source, on the scale this tracker declares where it has one.
    /// A facet's counts for a question, kept in the tracker's store while its sources stand where
    /// they stood: the same counts for every page of a list, and counted once after each refresh.
    pub fn facet(&self, q: &TrackerQuery, field: &str, limit: usize) -> (Vec<(String, u64)>, u64) {
        let question = format!(
            "{field}|{limit}|{}|{:?}|{}|{}|{}",
            q.text,
            q.pred,
            q.named.as_deref().unwrap_or(""),
            q.kind.as_deref().unwrap_or(""),
            q.seen_before.as_deref().unwrap_or("")
        );
        // The sources where they stand, and the statement and matches the counts were made under.
        let mark = format!("{}|{}", self.mark(), self.stated());
        let store = crate::thingstore::ThingStore::open(&self.dir).ok();
        if let Some(kept) = store.as_ref().and_then(|s| s.facet_cached(&question, &mark)) {
            return kept;
        }
        let counts = self.facet_counted(q, field, limit);
        if let Some(s) = &store {
            s.keep_facet(&question, &mark, &counts);
        }
        counts
    }

    /// The overview's facets and those of each named view, counted ahead of the first visitor.
    fn count_facets_ahead(&self) {
        let mut views: Vec<Option<String>> = vec![None];
        views.extend(self.decl.view.named.iter().map(|n| Some(n.name.clone())));
        for named in views {
            let q = TrackerQuery { named, limit: 25, ..TrackerQuery::default() };
            for f in self.facets(&q) {
                let _ = self.facet(&q, &f, 8);
            }
        }
    }

    fn facet_counted(&self, q: &TrackerQuery, field: &str, limit: usize) -> (Vec<(String, u64)>, u64) {
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
        // On the scale where the tracker declares one, by count where it does not.
        match self.decl.normalise_for(field) {
            Some(n) if !n.scale.is_empty() => {
                out.sort_by_key(|(v, _)| n.position(v).unwrap_or(usize::MAX));
            }
            _ => out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0))),
        }
        out.truncate(limit);
        (out, coverage)
    }

    /// One thing, and everything any source says about it.
    /// The identifier of the thing this one is, where a person confirmed it is the same as another.
    pub fn kept(&self, id: &Id) -> Id {
        let own = crate::schemes::key(&id.scheme, &id.value);
        match self.folds.read().ok().and_then(|f| f.get(&own).cloned()) {
            Some(k) => match k.split_once(':') {
                Some((s, v)) => Id { scheme: s.to_string(), value: v.to_string() },
                None => id.clone(),
            },
            None => id.clone(),
        }
    }

    /// The values of the things folded into this one, to ask the sources for as well.
    fn folded_into(&self, id: &Id) -> Vec<String> {
        let own = crate::schemes::key(&id.scheme, &id.value);
        self.folds
            .read()
            .map(|f| f.iter().filter(|(_, kept)| **kept == own).filter_map(|(f, _)| f.split_once(':').map(|(_, v)| v.to_string())).collect())
            .unwrap_or_default()
    }

    pub fn entry(&self, scheme: &str, value: &str) -> Option<Thing> {
        // A thing folded into another is that other, at its old address too.
        let asked = Id { scheme: scheme.to_string(), value: value.to_string() };
        let kept = self.kept(&asked);
        if kept != asked && crate::schemes::key(&kept.scheme, &kept.value) != crate::schemes::key(scheme, value) {
            return self.entry(&kept.scheme, &kept.value);
        }
        let mut values = vec![value.to_string()];
        values.extend(self.folded_into(&asked));
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
                ids: values.clone(),
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

    pub fn records_of(&self, member: &str, ids: &[String], versions: bool) -> Vec<Claim> {
        self.members
            .iter()
            .find(|m| m.name() == member)
            .map(|m| m.member.fetch(ids, versions))
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

/// Every source a workspace holds, by the name it calls itself.
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

/// The things a refresh reads again, and per source the claims it reads them from.
struct Piecemeal {
    keys: BTreeSet<String>,
    claims: BTreeMap<String, BTreeSet<String>>,
}

/// From this many moved claims on, a refresh reads every thing rather than the ones they are
/// about: one pass over the sources is then shorter than a lookup per claim.
const PIECEMEAL: usize = 20_000;

/// The shape of what a source says: its properties and their types, and the identifier schemes
/// it names things by. Counts are not in it, so an update does not change it.
fn shape(m: &Resolved) -> String {
    let mut props: Vec<String> = m.described["properties"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| format!("{}:{}", p["name"].as_str().unwrap_or(""), p["type"].as_str().unwrap_or("")))
        .collect();
    props.sort();
    let mut schemes: Vec<String> = m.described["schemes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s["scheme"].as_str().map(str::to_string))
        .collect();
    schemes.sort();
    format!("{} | {}", props.join(","), schemes.join(","))
}

/// A tracker counts in its sources' marks, because each of them runs on its own cadence.
fn parse_marks(raw: &str) -> BTreeMap<String, i64> {
    raw.split(',')
        .filter_map(|p| p.split_once('='))
        .filter_map(|(k, v)| Some((k.trim().to_string(), v.trim().parse().ok()?)))
        .collect()
}

impl Tracker {
    /// One mark per source, so a reader hands back exactly what they were last told.
    pub fn mark(&self) -> String {
        self.members
            .iter()
            .map(|m| format!("{}={}", m.name(), m.member.mark()))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The mark as it stood before each source's last run, which is what "since I last looked"
    /// means when nobody has looked yet.
    pub fn mark_before(&self) -> String {
        self.members
            .iter()
            .map(|m| format!("{}={}", m.name(), (m.member.mark() - 1).max(0)))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// What entered, what left, and what changed inside a thing, field by field.
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
                    let title = change["title"].as_str().unwrap_or("").to_string();
                    let mut one = change.clone();
                    one["source"] = json!(m.name());
                    // Under every thing the claim is about, or on its own where it is about none.
                    let about: Vec<Option<crate::claim::Id>> = match about(&keys, &ids) {
                        a if a.is_empty() => vec![None],
                        a => a.into_iter().map(|k| Some(self.kept(k))).collect(),
                    };
                    for key in about {
                        let token = match &key {
                            Some(k) => crate::schemes::key(&k.scheme, &k.value),
                            None => format!("{}#{}", m.name(), change["claim_id"]),
                        };
                        match index.get(&token) {
                            Some(i) => entries[*i].2.push(one.clone()),
                            None => {
                                index.insert(token, entries.len());
                                entries.push((key, title.clone(), vec![one.clone()]));
                            }
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

/// Every tracker a workspace holds, by the name it calls itself. A watch names a tracker, not a path.
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
    /// Checked against when each source last finished a run, complete or not: this is a claim
    /// about freshness, not about coverage. What a run reached is a separate fact and the
    /// source's state carries it. A tracker that is stale says so on its own front page, rather
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

    /// How long ago the source that finished least recently finished, in seconds. `None` where
    /// any source has never completed a run, because then there is no oldest to name.
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
    /// What this tracker actually holds, counted rather than claimed.
    ///
    /// Every source is paged through the same `search` a reader uses, so the numbers are the ones
    /// an answer would give. A measurement taken from the store behind the interface would be a
    /// measurement of something nobody can ask for.
    pub fn measure(&self) -> J {
        let keys = self.decl.keys();
        // key value -> source -> (kinds, field -> (what it said, what this tracker makes of it)).
        // Every claim a source holds for the thing, as the thing page shows them: keeping the
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
                    for key in about(&keys, &hit.ids) {
                    // Every field, under the name this tracker shows it by. A measurement
                    // of one field is a measurement of the field somebody guessed.
                    // Compared with the case folded, as the thing is gathered: `cve-2021-44228`
                    // and `CVE-2021-44228` are one thing there and must be one here, and so is the scheme.
                    let own = crate::schemes::key(&key.scheme, &key.value);
                    let said = subjects
                        .entry(self.folds.read().ok().and_then(|f| f.get(&own).cloned()).unwrap_or(own))
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
    /// What this source can be asked of a predicate, and nothing more.
    ///
    /// A filter is pushed down so a source does the narrowing it can do, and a conjunct it cannot
    /// answer is dropped rather than sent: a source that holds the exploit for a thing another
    /// source selected still has to be reachable. What is dropped here is applied again over the
    /// assembled thing, where every source's words are in one place.
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

    /// The whole predicate again, over the values the thing shows. A question that spans sources is
    /// answered here or nowhere: no source holds both `exploited` and `severity`.
    pub fn entry_holds(&self, entry: &Thing, pred: &Pred) -> bool {
        match pred {
            Pred::And(a, b) => self.entry_holds(entry, a) && self.entry_holds(entry, b),
            Pred::Or(a, b) => self.entry_holds(entry, a) || self.entry_holds(entry, b),
            Pred::Cmp { left, op, right } => {
                // What a thing is, as against what its sources say about it. These four are the
                // same names a source answers them under, and they pass the prune, so a query on
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
    /// How many claims a viewer under this bound can reach. The paywall is a bound on a query,
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
    /// What a tracker claims, held against what its sources actually answer.
    pub fn check(&self) -> Vec<String> {
        let mut wrong = Vec::new();
        for name in &self.missing {
            wrong.push(format!("{name} is named and not installed here"));
        }
        if self.members.is_empty() {
            wrong.push("no source resolved, so there is nothing to check".into());
            return wrong;
        }

        // A join key only one source carries joins nothing. The tracker still answers, and every
        // thing is one claim, which looks like a working tracker and is not one.
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
                // A tracker of one source has nothing to join yet: the pages of a list are what it is
                // for, and saying it is untrue would stop every CI it is checked in.
                1 if self.members.len() > 1 => wrong.push(format!(
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

        // No promise claims nothing. One that says how fresh it is has to say of what.
        let promised = self.decl.promise.fresh_within.is_some() || !self.decl.promise.excludes.trim().is_empty();
        if promised && self.decl.promise.covers.trim().is_empty() {
            wrong.push("the promise says nothing about what is covered".into());
        }
        let (holds, late) = self.promise();
        if !holds {
            wrong.push(format!("the promise does not hold: {}", late.join("; ")));
        }
        wrong
    }
}

/// One comparison between two strings, case folded, for the four things a thing is rather than
/// says. A date and a title order the same way here: lexically, which is what an ISO date wants.
/// Whether sources disagree: two of them, each with everything it said, and not the same. One
/// source saying two things is not a disagreement with itself: two exploits for one CVE on two
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

/// Whether a predicate has a `source` clause this source satisfies. A source the query excludes
/// is not read; a source it may include is.
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

impl Tracker {
    /// Everything the sources say, as a tracker store holds it: per thing, per source, per
    /// property, the words, what the tracker makes of them, and whether it understood them. Read
    /// through `search`, the call a reader has, as `measure` reads it.
    pub fn snapshot(&self) -> crate::thingstore::Snapshot {
        self.snapshot_of(None)
    }

    /// The snapshot, or with `only` the part of it about the things a moved claim is about.
    fn snapshot_of(&self, only: Option<&Piecemeal>) -> crate::thingstore::Snapshot {
        use crate::thingstore::Snapshot;
        let keys = self.decl.keys();
        let mut snap = Snapshot::default();
        // Sources in priority order, so a thing's title is its highest-priority source's.
        for m in &self.members {
            snap.states.insert(m.name().to_string(), m.state().to_string());
            snap.kinds.insert(m.name().to_string(), m.kind());
            snap.shapes.insert(m.name().to_string(), shape(m));
            // In part: the claims named, and of what they are about only the things named.
            if let Some(p) = only {
                let ids: Vec<String> = p.claims.get(m.name()).into_iter().flatten().cloned().collect();
                for chunk in ids.chunks(200) {
                    for c in m.member.fetch(chunk, false) {
                        self.take(&mut snap, &keys, m, &c.record_id, &c.title, &c.known, &c.ids, &c.fields, Some(&p.keys));
                    }
                }
                continue;
            }
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
                    self.take(&mut snap, &keys, m, &hit.record_id, &hit.title, &hit.known, &hit.ids, &hit.fields, None);
                }
                offset += hits.len();
                if hits.len() < 5000 {
                    break;
                }
            }
        }
        // And what a person has matched and signed, for a thing the tracker holds.
        for m in crate::matches::standing(&self.dir) {
            if let Some(t) = snap.things.get_mut(&m.key) {
                t.related.entry(m.relation.clone()).or_default().entry(m.target.clone()).or_default().insert(format!("person:{}", m.by));
            }
        }
        // The title the tracker makes of what its sources say, where it declares one: a station by
        // its brand and town rather than its address, a country-year by the country's name.
        let template = &self.decl.thing.title;
        if !template.is_empty() {
            let order: Vec<&str> = self.members.iter().map(|m| m.name()).collect();
            for t in snap.things.values_mut() {
                let made = {
                    let said = |p: &str| -> Vec<String> { order.iter().filter_map(|m| t.by.get(*m)?.get(p)).flat_map(|s| s.means.iter().cloned()).collect() };
                    crate::display::title(template, &self.decl, &said, &t.value, &t.title)
                };
                if let Some(title) = made {
                    t.title = title;
                }
            }
        }
        snap
    }

    /// One claim taken into the snapshot: a thing for each of what it is about, its title, what
    /// it says of each property in the tracker's words, and what it says the thing is to another.
    /// With `only`, the things outside it are left as they are.
    #[allow(clippy::too_many_arguments)]
    fn take(
        &self,
        snap: &mut crate::thingstore::Snapshot,
        keys: &[&str],
        m: &Resolved,
        record_id: &str,
        title: &str,
        known: &str,
        ids: &[Id],
        fields: &BTreeMap<String, crate::claim::Value>,
        only: Option<&BTreeSet<String>>,
    ) {
        use crate::thingstore::{kind_of, Said, Snap};
        // A thing for every value of the first of the tracker's schemes the claim
        // carries: an exploit for two CVEs is about both. Other schemes it carries are
        // shown on the thing and make no thing of their own.
        for id in about(keys, ids) {
        let own = crate::schemes::key(&id.scheme, &id.value);
        // A thing a person confirmed is one with another is that other.
        let key = self.folds.read().ok().and_then(|f| f.get(&own).cloned()).unwrap_or_else(|| own.clone());
        if only.is_some_and(|o| !o.contains(&key)) {
            continue;
        }
        let folded = key != own;
        let (scheme, value) = match key.split_once(':') {
            Some((s, v)) if folded => (s.to_string(), v.to_string()),
            _ => (id.scheme.clone(), id.value.clone()),
        };
        let rank = self.members.iter().position(|x| x.name() == m.name()).unwrap_or(usize::MAX);
        let by = (folded, rank, record_id.to_string());
        let thing = snap.things.entry(key).or_insert_with(|| Snap {
            scheme,
            value,
            title: title.to_string(),
            named_by: Some(by.clone()),
            ..Snap::default()
        });
        // Named by its own claim before a folded one, by the higher source, then by the least id.
        if thing.named_by.as_ref().is_some_and(|held| by < *held) {
            if !folded {
                thing.scheme = id.scheme.clone();
                thing.value = id.value.clone();
            }
            thing.title = title.to_string();
            thing.named_by = Some(by);
        }
        thing
            .claims
            .entry(m.name().to_string())
            .or_default()
            .insert(record_id.to_string());
        // The source's own date for it, the earliest of its claims': when NVD published a CVE.
        if !known.trim().is_empty() {
            let held = thing.known.entry(m.name().to_string()).or_insert_with(|| known.to_string());
            if known < held.as_str() {
                *held = known.to_string();
            }
        }
        // What this claim says the thing is to something else: every other
        // identifier it states of a scheme a relation names.
        for r in &self.decl.relations {
            for other in ids.iter().filter(|i| i.scheme == r.to) {
                if let Some(target) = r.target(&other.value) {
                    thing.related.entry(r.name.clone()).or_default().entry(target).or_default().insert(m.name().to_string());
                }
            }
        }
        let props = thing.by.entry(m.name().to_string()).or_default();
        for (name, value) in fields {
            let property = self.field_out(m.name(), name);
            let align = self.decl.normalise_for(&property);
            let words: Vec<String> = match value {
                crate::claim::Value::List(items) => items.iter().map(|v| v.display()).collect(),
                v => vec![v.display()],
            };
            let said = props.entry(property.clone()).or_insert_with(|| Said {
                kind: kind_of(value).to_string(),
                understood: true,
                ..Said::default()
            });
            for w in words {
                let mapped = match align {
                    Some(a) => a.means(m.name(), &w),
                    None => w.clone(),
                };
                // Understood where the map names the word or the scale holds what it
                // became. A word that passed through unchanged and is on no scale is
                // one nobody has said the meaning of.
                let named = align
                    .map(|a| {
                        a.position(&mapped).is_some()
                            || a.members.get(m.name()).is_some_and(|map| {
                                map.contains_key(&w) || map.contains_key(&w.to_lowercase())
                            })
                    })
                    .unwrap_or(false);
                if !named {
                    said.understood = false;
                }
                said.raw.insert(w);
                said.means.insert(mapped);
            }
        }
        }
    }


    /// The tracker's own store, made to hold what the sources say now.
    pub fn refresh(&self, rebuild: bool) -> Result<crate::thingstore::Refreshed, String> {
        self.refresh_holding(rebuild, false)
    }

    /// As `refresh`, waiting for a refresh another process is making rather than leaving it to
    /// that one: what a person asks for at the command line.
    pub fn refresh_waiting(&self, rebuild: bool) -> Result<crate::thingstore::Refreshed, String> {
        self.refresh_holding(rebuild, true)
    }

    /// One refresh of a tracker's store at a time, whoever asks: the server after a page, a cell's
    /// pass, a person. A refresh of 400,000 things takes minutes, and two at once each found the
    /// store locked by the other and neither was made (2026-10-11). The lock is the system's, held
    /// while this refresh is, so one that dies leaves none behind.
    fn refresh_holding(&self, rebuild: bool, wait: bool) -> Result<crate::thingstore::Refreshed, String> {
        if let Some(p) = self.decl.package.as_ref().filter(|p| p.sealed) {
            return Err(format!(
                "{} is a sealed package, {}: it carries no recipe to build it with. `zetlyn tracker pull` takes a newer version",
                self.decl.name, p.version
            ));
        }
        let lock_path = self.dir.join(".refreshing");
        let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&lock_path).map_err(|e| format!("{}: {e}", lock_path.display()))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) if wait => {
                eprintln!("{}: another refresh is being made; this one waits for it", self.decl.name);
                lock.lock().map_err(|e| format!("{}: {e}", lock_path.display()))?;
            }
            Err(std::fs::TryLockError::WouldBlock) => return Err(format!("{}: {REFRESHING}", self.decl.name)),
            Err(std::fs::TryLockError::Error(e)) => return Err(format!("{}: {e}", lock_path.display())),
        }
        // And the world's turn for heavy work: another tracker's refresh or a version for the hub
        // is not made beside this one.
        let Some(_turn) = crate::source::heavy_turn(&self.root, wait)? else {
            return Err(format!("{}: {REFRESHING}", self.decl.name));
        };
        if let Ok(mut f) = self.folds.write() {
            *f = folds(&self.dir);
        }
        let mut store = crate::thingstore::ThingStore::open(&self.dir)?;
        // Only the things a claim moved about since the last refresh, where that can be said;
        // every thing otherwise.
        let piecemeal = if rebuild || std::env::var_os("ZETLYN_REFRESH_WHOLE").is_some() { None } else { self.moved(&store) };
        let r = match &piecemeal {
            Some(p) => store.refresh(&self.decl, &self.snapshot_of(Some(p)), false, Some(&p.keys), false)?,
            None if self.records() > in_parts_from() => self.refresh_in_parts(&mut store, rebuild)?,
            None => store.refresh(&self.decl, &self.snapshot(), rebuild, None, false)?,
        };
        store.set_meta("mark", &self.mark())?;
        store.set_meta("stated", &self.stated())?;
        // What the store holds moved: everything read again, or something said differently. A run
        // that found nothing new leaves what was drawn of it standing, and draws nothing again.
        let moved = piecemeal.is_none() || r.signals > 0;
        if moved {
            store.set_meta("content", &format!("{}.{}", crate::now(), store.last_signal()))?;
        }
        drop(store);
        // And the pictures of it drawn again ahead of the first visitor, once the sources running
        // about now have all run.
        if moved {
            crate::warm::due(&self.dir, &self.root);
        }
        // While the lock is still held: the first visitor after an update finds the counts made.
        self.count_facets_ahead();
        Ok(r)
    }

    /// What the store was made from besides the sources' claims: the statement, and the matches
    /// people signed. Either changed, every thing is read again.
    /// Where the sources stand and what the statement and matches say: what something drawn of
    /// the tracker, or counted, is kept under, and made again when it moves.
    pub fn standing(&self) -> String {
        standing_of(&self.dir)
    }

    fn stated(&self) -> String {
        stated_of(&self.dir)
    }

    /// The things a claim moved about since the store's mark, and the claims to read them from,
    /// or None where the whole must be read: a store that does not hold which claim is about
    /// what, a statement, a match or a source's shape that changed, a source more or less, a
    /// source that started again, or so much moved that reading everything is the shorter way.
    fn moved(&self, store: &crate::thingstore::ThingStore) -> Option<Piecemeal> {
        if store.meta("refreshed").is_none() || store.meta("claims").as_deref() != Some("1") || store.meta("words").as_deref() != Some("1") {
            return None;
        }
        if store.meta("stated")? != self.stated() {
            return None;
        }
        let shapes: BTreeMap<String, String> = serde_json::from_str(&store.meta("shapes")?).ok()?;
        let marks = parse_marks(&store.meta("mark")?);
        if marks.len() != self.members.len() || !self.missing.is_empty() {
            return None;
        }
        let keys = self.decl.keys();
        let mut moved: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut things = BTreeSet::new();
        let mut count = 0usize;
        for m in &self.members {
            if shapes.get(m.name()) != Some(&shape(m)) {
                return None;
            }
            let was = *marks.get(m.name())?;
            let is = m.member.mark();
            if is == was {
                continue;
            }
            if is < was {
                return None;
            }
            let said = m.member.changes(was, PIECEMEAL);
            let changed = said["changed"].as_array()?;
            let removed = said["removed"].as_array()?;
            if changed.len() >= PIECEMEAL || removed.len() >= PIECEMEAL {
                return None;
            }
            count += changed.len() + removed.len();
            let ids: Vec<String> = changed
                .iter()
                .chain(removed)
                .filter_map(|c| c["claim_id"].as_str().map(str::to_string))
                .collect();
            // What they were about, and what the changed ones are about now.
            things.extend(store.about_claims(m.name(), &ids));
            for c in changed {
                let ids: Vec<Id> = c["ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|i| Some(Id { scheme: i["scheme"].as_str()?.to_string(), value: i["value"].as_str()?.to_string() }))
                    .collect();
                things.extend(about(&keys, &ids).into_iter().map(|i| {
                    let k = crate::schemes::key(&i.scheme, &i.value);
                    self.folds.read().ok().and_then(|f| f.get(&k).cloned()).unwrap_or(k)
                }));
            }
            moved.insert(m.name().to_string(), ids);
        }
        if count >= PIECEMEAL {
            return None;
        }
        // Every claim about those things, the ones that stayed as well as the ones that moved.
        let mut claims = store.claims_about(&things);
        for (source, ids) in moved {
            claims.entry(source).or_default().extend(ids);
        }
        Some(Piecemeal { keys: things, claims })
    }

    /// Refreshed where a source has moved since the last refresh, and left alone otherwise, so a
    /// surface that reads its sources again every minute does the work once an update, not once
    /// a minute.
    pub fn refresh_if_moved(&self) -> Result<Option<crate::thingstore::Refreshed>, String> {
        if self.decl.package.as_ref().is_some_and(|p| p.sealed) {
            return Ok(None);
        }
        let store = crate::thingstore::ThingStore::open(&self.dir)?;
        if store.meta("mark").as_deref() == Some(self.mark().as_str()) {
            return Ok(None);
        }
        drop(store);
        self.refresh(false).map(Some)
    }
}

impl Tracker {
    /// The context a question about this tracker's things is read and asked in.
    pub fn context(&self) -> crate::thingquery::Context<'_> {
        crate::thingquery::Context {
            decl: &self.decl,
            sources: self.members.iter().map(|m| m.name().to_string()).collect(),
            properties: self
                .members
                .iter()
                .map(|m| {
                    let said = m.described["properties"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|f| f["name"].as_str())
                        .map(|n| self.field_out(m.name(), n))
                        .collect();
                    (m.name().to_string(), said)
                })
                .collect(),
            kinds: self.members.iter().map(|m| m.kind()).collect(),
            now: crate::now(),
        }
    }
}

/// What a claim is about, as a tracker counts things: every value of the first of the tracker's
/// schemes the claim carries. An exploit for two CVEs is a claim about both of them. A claim that
/// carries a GHSA and a CVE is about the CVE where the tracker is identified by CVE, and the GHSA
/// is shown on it rather than making a thing of its own (D14).
pub fn about<'a, K: AsRef<str>>(keys: &[K], ids: &'a [Id]) -> Vec<&'a Id> {
    let Some(scheme) = keys.iter().map(AsRef::as_ref).find(|k| ids.iter().any(|i| i.scheme == *k)) else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    ids.iter()
        .filter(|i| i.scheme == *scheme)
        .filter(|i| seen.insert(crate::schemes::key(&i.scheme, &i.value)))
        .collect()
}

impl Tracker {
    pub fn private(&self) -> bool {
        self.decl.visibility == "private"
    }

    /// Each source's word on being shown in public: `yes`, `summary` or `no`; one that has said
    /// nothing is `yes`, shown until its owner says otherwise.
    pub fn licences(&self) -> Vec<(String, String)> {
        self.members
            .iter()
            .map(|m| (m.name().to_string(), crate::sourcedecl::shown(m.described["licence"]["republish"].as_str().unwrap_or("")).to_string()))
            .collect()
    }

    /// Why this tracker may not be on a page anyone can open: every source that says no, or
    /// says nothing. Empty where it may.
    pub fn not_public(&self) -> Vec<String> {
        self.licences()
            .into_iter()
            .filter(|(_, r)| !matches!(r.as_str(), "yes" | "summary"))
            .map(|(s, _)| format!("{s} may not be republished"))
            .collect()
    }

    /// Whether the text of this source's claims may be shown, or only their titles, values and
    /// where to read them.
    pub fn text_shown(&self, source: &str) -> bool {
        self.licences().iter().find(|(s, _)| s == source).map_or(true, |(_, r)| r != "summary")
    }
}

/// The statement and the matches a store was made under, as one stamp.
pub fn stated_of(dir: &Path) -> String {
    let statement = std::fs::read_to_string(dir.join(crate::trackerdecl::FILE)).unwrap_or_default();
    let matches = std::fs::read_to_string(dir.join(crate::matches::FILE)).unwrap_or_default();
    format!("{}|{}", crate::place::sha256(statement.as_bytes()), crate::place::sha256(matches.as_bytes()))
}

/// What a picture of the tracker is kept under: what its store holds, which moves only when a
/// refresh found something, and the statement it was made under. Asked of the directory alone, so
/// a picture can be drawn ahead without opening the tracker and every source.
pub fn standing_of(dir: &Path) -> String {
    let content = crate::thingstore::ThingStore::open(dir).ok().and_then(|s| s.meta("content").or_else(|| s.meta("refreshed"))).unwrap_or_default();
    format!("{content}|{}", stated_of(dir))
}

/// Every `same` a person confirmed and did not take back, followed to its end: where c was
/// confirmed the same as b and b the same as a, both are a.
fn folds(dir: &Path) -> BTreeMap<String, String> {
    let direct: BTreeMap<String, String> = crate::matches::standing(dir)
        .into_iter()
        .filter(|m| m.relation == crate::trackerdecl::SAME && m.key != m.target)
        .map(|m| (m.target, m.key))
        .collect();
    let mut out = BTreeMap::new();
    for start in direct.keys() {
        let mut at = start;
        let mut seen = BTreeSet::new();
        while let Some(next) = direct.get(at) {
            if !seen.insert(next) {
                break;
            }
            at = next;
        }
        if at != start {
            out.insert(start.clone(), at.clone());
        }
    }
    out
}

/// What a refresh says that found another one being made: no failure, that one makes it.
pub const REFRESHING: &str = "being refreshed already by another process";

/// From this many claims on, a refresh of every thing is made in parts: a CVE tracker of 400,000
/// NVD records in one piece needed 5.3 GB (2026-10-11), and its cell may use two.
const IN_PARTS_FROM: u64 = 60_000;
/// Things per part: a few hundred megabytes at most, whatever the tracker holds.
const PART_THINGS: usize = 20_000;

// `ZETLYN_IN_PARTS_FROM` and `ZETLYN_PART_THINGS` set both small, and nothing else: how a test makes
// a tracker of fifty things in slices of seven.
fn in_parts_from() -> u64 {
    std::env::var("ZETLYN_IN_PARTS_FROM").ok().and_then(|s| s.parse().ok()).unwrap_or(IN_PARTS_FROM)
}
fn part_things() -> usize {
    if let Some(n) = std::env::var("ZETLYN_PART_THINGS").ok().and_then(|s| s.parse().ok()).filter(|n| *n > 0) {
        return n;
    }
    // Ten things a megabyte of what the process may use: a part of 20,000 took some 750 MB in all,
    // so a cell of 1 GB makes parts of 10,000 and one of 512 MB parts of 5,000.
    match memory_limit() {
        Some(bytes) => ((bytes >> 20) as usize * 10).clamp(2_000, PART_THINGS),
        None => PART_THINGS,
    }
}

/// The memory this process may use, where a cgroup says so: a cell's `MemoryMax`. None on a
/// machine that sets none, or says `max`.
fn memory_limit() -> Option<u64> {
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let path = cgroup.lines().find_map(|l| l.strip_prefix("0::"))?;
    std::fs::read_to_string(format!("/sys/fs/cgroup{}/memory.max", path.trim())).ok()?.trim().parse().ok()
}

impl Tracker {
    /// Every thing again, a slice at a time. One pass over the sources says which claims each thing
    /// is made of, and holds only that; each slice then reads its claims, as a refresh in part does,
    /// and writes its things. A rebuild, or the first look, clears the store before the first slice
    /// and says nothing of what it writes, as it would in one piece.
    fn refresh_in_parts(&self, store: &mut crate::thingstore::ThingStore, rebuild: bool) -> Result<crate::thingstore::Refreshed, String> {
        use std::hash::{Hash, Hasher};
        let keys = self.decl.keys();
        let quiet = rebuild || store.meta("refreshed").is_none();
        // Each thing into one of `n` slices by its key, and each slice holding only its keys and a
        // flat list of its claims: the plan of a refresh of 400,000 things in tens of megabytes,
        // where nested maps of every key took hundreds. There are at least as many claims as
        // things, so a slice holds at most a part's worth.
        let size = part_things();
        let n = (self.records() as usize).div_ceil(size).max(1);
        let slice_of = |key: &str| -> usize {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            key.hash(&mut h);
            (h.finish() % n as u64) as usize
        };
        let mut keys_in: Vec<BTreeSet<String>> = vec![BTreeSet::new(); n];
        let mut claims_in: Vec<Vec<(usize, String)>> = vec![Vec::new(); n];
        for (mi, m) in self.members.iter().enumerate() {
            let mut offset = 0usize;
            loop {
                let q = Query { text: String::new(), pred: None, view: None, ids: Vec::new(), seen_before: None, sort: None, limit: 5000, offset };
                let Ok((_, hits, _)) = m.member.search(&q) else { break };
                if hits.is_empty() {
                    break;
                }
                for hit in &hits {
                    let mut slices: Vec<usize> = Vec::new();
                    for id in about(&keys, &hit.ids) {
                        let kept = self.kept(id);
                        let key = crate::schemes::key(&kept.scheme, &kept.value);
                        let s = slice_of(&key);
                        keys_in[s].insert(key);
                        if !slices.contains(&s) {
                            slices.push(s);
                            claims_in[s].push((mi, hit.record_id.clone()));
                        }
                    }
                }
                offset += hits.len();
                if hits.len() < 5000 {
                    break;
                }
            }
        }
        if quiet {
            store.clear_for_parts()?;
        } else {
            // What the store holds and no claim names any more goes in its slice, with a signal.
            for k in store.keys() {
                let s = slice_of(&k);
                keys_in[s].insert(k);
            }
        }
        let mut total = crate::thingstore::Refreshed { first: quiet, ..Default::default() };
        // Each part asks whether a source was read again under a new declaration, and each is to
        // hear the same answer: the first part writes the new shapes, and the rest would otherwise
        // take everything that source says for news.
        let shapes_before = store.meta("shapes");
        // Every row written again: the charts' index made after, in one pass, and not kept up a row
        // at a time. Made again however the parts end.
        store.without_said_index()?;
        let parts = self.refresh_parts(store, keys_in, claims_in, shapes_before, quiet, &mut total);
        let made = store.with_said_index();
        parts?;
        made?;
        Ok(total)
    }

    #[allow(clippy::too_many_arguments)]
    fn refresh_parts(
        &self,
        store: &mut crate::thingstore::ThingStore,
        keys_in: Vec<BTreeSet<String>>,
        claims_in: Vec<Vec<(usize, String)>>,
        shapes_before: Option<String>,
        quiet: bool,
        total: &mut crate::thingstore::Refreshed,
    ) -> Result<(), String> {
        for (part_keys, part_claims) in keys_in.into_iter().zip(claims_in) {
            if part_keys.is_empty() {
                continue;
            }
            if let Some(s) = &shapes_before {
                store.set_meta("shapes", s)?;
            }
            let mut p = Piecemeal { keys: part_keys, claims: BTreeMap::new() };
            for (mi, id) in part_claims {
                p.claims.entry(self.members[mi].name().to_string()).or_default().insert(id);
            }
            let r = store.refresh(&self.decl, &self.snapshot_of(Some(&p)), false, Some(&p.keys), quiet)?;
            total.signals += r.signals;
            total.things = r.things;
            total.conflicts = r.conflicts;
            total.wording = r.wording;
        }
        Ok(())
    }
}
