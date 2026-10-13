//! Pictures of what a tracker holds, declared in its statement under `view.show` and `thing.show`:
//! a map of places, a map of countries, a value over time, how far its publishers differ, how its
//! values spread, two dimensions against each other, a ranking, a table of entities, averages by
//! group, which sources overlap, and what arrived when. Each names properties and nothing else, so
//! any tracker whose things have such properties can show them; how a value reads, which way is
//! better and which values are real is the tracker's `values`, and every picture asks it.
//!
//! Drawn here as SVG, so a page needs nothing from anywhere else and shows without JavaScript; a
//! few lines of it make the maps zoom, the legends choose, and the marks say what they are.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use maud::{html, Markup, PreEscaped};
use serde::{Deserialize, Serialize};

use crate::display::{self, Scale};
use crate::serve::{at, urlencode};
use crate::thingstore::ThingStore;
use crate::trackerdecl::{TrackerDecl, ValueDecl};

/// One picture, as a statement declares it.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Show {
    /// Things with a place, coloured by a value: stations by what diesel costs there.
    Map(MapSpec),
    /// Countries coloured by a value, for a time a reader chooses: inflation in 2024.
    Areas(AreasSpec),
    /// A value of one entity over time, a line per source: Germany's inflation since 2000.
    Series(SeriesSpec),
    /// One value as each source says it, side by side on one axis: where publishers differ.
    Spread(SpreadSpec),
    /// How a value is spread over every thing, a thing's own marked on its page.
    Distribution(DistributionSpec),
    /// Two properties against each other, counted: severity against how likely an exploit is.
    Matrix(MatrixSpec),
    /// The things with the most or the least of a value.
    Ranking(RankingSpec),
    /// How many things a date property puts in each month, and the newest of them.
    Arrivals(ArrivalsSpec),
    /// Every entity at one time, a column a value, with its change and its rank: countries in 2024.
    Table(TableSpec),
    /// A value's average over every thing and per group, and how it moved: diesel by region.
    Averages(AveragesSpec),
    /// Which sources speak of the same things, and which of a thing only one knows.
    Overlap(OverlapSpec),
    /// On a thing's page: a value as each source said it over time.
    History(HistorySpec),
    /// On a model's page: what each usual quantisation of it takes, from its parameter count.
    Memory(MemorySpec),
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MapSpec {
    /// A property saying where: "48.18, 3.30", latitude first.
    pub place: String,
    /// The values a reader may colour by, the first by default.
    #[serde(default, deserialize_with = "one_or_many")]
    pub colour: Vec<String>,
    /// Less is better, where the tracker's `values` do not say.
    #[serde(default)]
    pub cheap: bool,
    /// A property naming where a thing is in words, a town: the names written on the map, more of
    /// them the closer one zooms.
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AreasSpec {
    /// The properties naming the country, in any spelling a list uses: the first a thing has.
    #[serde(deserialize_with = "one_or_many")]
    pub country: Vec<String>,
    #[serde(default, deserialize_with = "one_or_many")]
    pub value: Vec<String>,
    /// A property for the time chosen, where things are a country at a time: a year, a quarter.
    #[serde(default)]
    pub by: Option<String>,
    #[serde(default)]
    pub cheap: bool,
    /// Fewer countries with a figure than this, and they are a list rather than a map.
    #[serde(default = "eighty")]
    pub min_map: usize,
    #[serde(default)]
    pub title: String,
}

fn eighty() -> usize {
    80
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeriesSpec {
    /// What one line is about: a country.
    pub entity: String,
    /// Where it is along the line: a year, a quarter.
    pub time: String,
    #[serde(default, deserialize_with = "one_or_many")]
    pub value: Vec<String>,
    /// The entity an overview shows before a reader chooses one.
    #[serde(default)]
    pub first: Option<String>,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SpreadSpec {
    #[serde(default, deserialize_with = "one_or_many")]
    pub value: Vec<String>,
    /// Where things are an entity at a time, the time an overview compares.
    #[serde(default)]
    pub time: Option<String>,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DistributionSpec {
    #[serde(default, deserialize_with = "one_or_many")]
    pub value: Vec<String>,
    /// Counted on a log scale, for values from one to millions: downloads.
    #[serde(default)]
    pub log: bool,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MatrixSpec {
    pub rows: String,
    pub columns: String,
    /// Where `columns` is a number: the edges of its bands.
    #[serde(default)]
    pub bands: Vec<f64>,
    /// At most this many rows and as many columns, the ones with most things.
    #[serde(default = "twelve")]
    pub limit: usize,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RankingSpec {
    pub value: String,
    /// The least first, where the tracker's `values` do not say less is better.
    #[serde(default)]
    pub cheap: bool,
    /// What orders things the value ties: CVSS after EPSS.
    #[serde(default, deserialize_with = "one_or_many")]
    pub then: Vec<String>,
    #[serde(default = "fifteen")]
    pub limit: usize,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArrivalsSpec {
    /// A date property, or `known` with `source`: the day that source first spoke of a thing.
    pub date: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default = "thirty_six")]
    pub months: usize,
    /// And the things no source speaks of any longer, by the month they went.
    #[serde(default)]
    pub withdrawn: bool,
    /// The newest things listed under it, this many.
    #[serde(default)]
    pub recent: usize,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TableSpec {
    /// The property naming the entity a row is: a country.
    pub entity: String,
    /// The property naming the time, where things are an entity at a time.
    #[serde(default)]
    pub time: Option<String>,
    #[serde(default, deserialize_with = "one_or_many")]
    pub value: Vec<String>,
    #[serde(default = "fifty")]
    pub limit: usize,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AveragesSpec {
    /// A property things are grouped by: a region.
    #[serde(default)]
    pub by: Option<String>,
    #[serde(default, deserialize_with = "one_or_many")]
    pub value: Vec<String>,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OverlapSpec {
    #[serde(default = "twelve")]
    pub limit: usize,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HistorySpec {
    #[serde(default, deserialize_with = "one_or_many")]
    pub value: Vec<String>,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySpec {
    /// The property holding how many parameters a model has.
    pub parameters: String,
    #[serde(default)]
    pub title: String,
}

fn twelve() -> usize {
    12
}
fn fifteen() -> usize {
    15
}
fn thirty_six() -> usize {
    36
}
fn fifty() -> usize {
    50
}

/// `colour: diesel` and `colour: [diesel, e10]` alike.
fn one_or_many<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
}

fn cmp(a: &f64, b: &f64) -> Ordering {
    a.partial_cmp(b).unwrap_or(Ordering::Equal)
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

/// One number for a thing where its sources say several: the middle one.
fn middle(values: &[(String, f64)]) -> f64 {
    let mut v: Vec<f64> = values.iter().map(|(_, n)| *n).collect();
    v.sort_by(cmp);
    v[v.len() / 2]
}

fn short(v: f64) -> String {
    let a = v.abs();
    if a >= 1e6 {
        format!("{:.1}M", v / 1e6)
    } else if a >= 1e4 {
        format!("{:.0}k", v / 1e3)
    } else if a >= 100.0 || v.fract() == 0.0 {
        format!("{v:.0}")
    } else if a >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

/// The first year written in words: `2024`, `FY2024/25`.
pub fn year_in(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    (0..b.len().saturating_sub(3)).find_map(|i| {
        let w = s.get(i..i + 4)?;
        (w.bytes().all(|c| c.is_ascii_digit()) && (w.starts_with("19") || w.starts_with("20"))).then(|| w.parse().ok()).flatten()
    })
}

/// The year a time is in: `2024`, `2024-Q3`, `2024-07-01`.
fn year_of(t: &str) -> Option<i64> {
    t.get(..4)?.parse().ok()
}

// -- what is drawn from ---------------------------------------------------------------------------

/// The store, the tracker's statement, and what a picture needs from around it.
pub struct Ctx<'a> {
    pub store: &'a ThingStore,
    pub decl: &'a TrackerDecl,
    /// Each source's title, by its name.
    pub sources: BTreeMap<String, String>,
    /// On a thing's page: per property, what each source said it was and when.
    pub history: BTreeMap<String, Vec<(String, i64, f64)>>,
    /// The titles asked for so far, each by its key: a picture names fifteen things, or the ten
    /// thousand places of a map, and not every thing the store holds.
    titles: std::cell::RefCell<BTreeMap<String, String>>,
    /// What the store holds and the statement it was made under: what a picture is kept under.
    pub standing: String,
    /// New data is in and its pictures are being drawn: until they are, the last ones are shown.
    pub warming: bool,
    /// Where the tracker is, whole, for the code that embeds its pictures elsewhere.
    pub embed_base: Option<String>,
}

/// Numbers of a property, the ones a picture counts, and how many it left out and why.
#[derive(Default)]
struct Numbers {
    by: BTreeMap<String, Vec<(String, f64)>>,
    implausible: usize,
    stale: usize,
}

impl<'a> Ctx<'a> {
    pub fn new(store: &'a ThingStore, decl: &'a TrackerDecl) -> Ctx<'a> {
        Ctx { store, decl, sources: BTreeMap::new(), history: BTreeMap::new(), titles: Default::default(), standing: String::new(), warming: false, embed_base: None }
    }

    fn value(&self, property: &str) -> Option<&ValueDecl> {
        self.decl.values.get(property)
    }

    fn label(&self, property: &str) -> String {
        display::label(self.decl, property)
    }

    fn fmt(&self, property: &str, v: f64) -> String {
        display::number(v, self.value(property))
    }

    fn source_title(&self, source: &str) -> String {
        self.sources.get(source).cloned().unwrap_or_else(|| source.rsplit('/').next().unwrap_or(source).to_string())
    }

    fn title(&self, key: &str) -> String {
        if let Some(t) = self.titles.borrow().get(key) {
            return t.clone();
        }
        let t = self.store.named(key).map(|n| n.0).unwrap_or_else(|| key.split_once(':').map(|k| k.1).unwrap_or(key).to_string());
        self.titles.borrow_mut().insert(key.to_string(), t.clone());
        t
    }

    fn href(&self, key: &str) -> String {
        match key.split_once(':') {
            Some((scheme, value)) => at(&format!("/thing/{}/{}", urlencode(scheme), urlencode(value))),
            None => String::new(),
        }
    }

    /// Every number a property has, per thing, per source, as the sources said them.
    fn said_numbers(&self, property: &str) -> BTreeMap<String, Vec<(String, f64)>> {
        let mut out: BTreeMap<String, Vec<(String, f64)>> = BTreeMap::new();
        for (key, source, means) in self.store.said_of_property(property) {
            if let Some(n) = means.iter().find_map(|m| m.trim().parse::<f64>().ok()).filter(|n| n.is_finite()) {
                out.entry(key).or_default().push((source, n));
            }
        }
        out
    }

    /// What each source says one thing's property is, as numbers: asked by its key.
    fn own_numbers(&self, key: &str, property: &str) -> Vec<(String, f64)> {
        self.store
            .said_by_key(key, property)
            .into_iter()
            .filter_map(|(s, means)| Some((s, means.iter().find_map(|m| m.trim().parse::<f64>().ok()).filter(|n| n.is_finite())?)))
            .collect()
    }

    /// One thing's words for a property, every source's together.
    fn own_words(&self, key: &str, property: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (_, means) in self.store.said_by_key(key, property) {
            for m in means {
                if !out.contains(&m) {
                    out.push(m);
                }
            }
        }
        out
    }

    /// The numbers a picture counts: none outside the range a real one falls in, and none older
    /// than the value stays current.
    fn numbers(&self, property: &str) -> Numbers {
        let mut n = Numbers::default();
        let d = self.value(property);
        let stale = self.stale(property);
        let said_all = self.said_numbers(property);
        // Where the tracker says how far out a real value can be: the middle, and the middle
        // distance to it, of every value said.
        let within = d.and_then(|d| d.outliers).and_then(|k| {
            let mut all: Vec<f64> = said_all.values().flatten().map(|x| x.1).filter(|v| d.is_none_or(|d| d.plausible(*v))).collect();
            if all.len() < 20 {
                return None;
            }
            all.sort_by(cmp);
            let mid = quantile(&all, 0.5);
            let mut off: Vec<f64> = all.iter().map(|v| (v - mid).abs()).collect();
            off.sort_by(cmp);
            let mad = quantile(&off, 0.5).max(1e-9);
            Some((mid - k * mad, mid + k * mad))
        });
        let d_owned = d.cloned().map(|mut x| {
            if let Some((lo, hi)) = within {
                x.plausible = Some(x.plausible.map(|(a, b)| (a.max(lo), b.min(hi))).unwrap_or((lo, hi)));
            }
            x
        });
        let d = d_owned.as_ref();
        for (key, said) in said_all {
            if stale.contains(&key) {
                n.stale += 1;
                continue;
            }
            let kept: Vec<(String, f64)> = said.into_iter().filter(|(_, v)| d.is_none_or(|d| d.plausible(*v))).collect();
            if kept.is_empty() {
                n.implausible += 1;
            } else {
                n.by.insert(key, kept);
            }
        }
        n
    }

    /// The things whose value of a property is older than it stays current.
    fn stale(&self, property: &str) -> BTreeSet<String> {
        let Some(d) = self.value(property) else { return BTreeSet::new() };
        let (Some(at_prop), Some(fresh)) = (d.at.as_deref(), d.fresh_secs()) else { return BTreeSet::new() };
        let now = crate::now();
        self.words(at_prop)
            .into_iter()
            .filter(|(_, w)| w.iter().filter_map(|s| crate::thingquery::seconds(s)).max().is_some_and(|t| now - t > fresh))
            .map(|(k, _)| k)
            .collect()
    }

    /// The words a property has, per thing, every source's together.
    fn words(&self, property: &str) -> BTreeMap<String, Vec<String>> {
        let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (key, _, means) in self.store.said_of_property(property) {
            let held = out.entry(key).or_default();
            for m in means {
                if !held.contains(&m) {
                    held.push(m);
                }
            }
        }
        out
    }

    /// An entity as one spelling: a country as its three letters, whatever a list wrote.
    fn entity_code(&self, property: &str, w: &str) -> String {
        if self.value(property).is_some_and(|d| d.is == "country") || crate::geo::iso3_of(w).is_some() && self.value(property).is_none_or(|d| d.is.is_empty()) {
            if let Some(c) = crate::geo::iso3_of(w) {
                return c;
            }
        }
        w.to_string()
    }

    fn entity_name(&self, property: &str, code: &str) -> String {
        match crate::geo::country(code) {
            Some(c) if self.value(property).is_none_or(|d| d.is == "country" || d.is.is_empty()) => c.name.clone(),
            _ => display::shown(self.decl, property, code),
        }
    }

    /// The entity each thing is, as one spelling. Where entities are countries, only countries: a
    /// publisher's region or income group (the World Bank's G903) is not one.
    fn entities_of(&self, property: &str) -> BTreeMap<String, String> {
        let countries = self.value(property).is_some_and(|d| d.is == "country");
        self.words(property)
            .into_iter()
            .filter_map(|(k, w)| Some((k.clone(), self.entity_code(property, w.first()?))))
            .filter(|(_, c)| !countries || crate::geo::country(c).is_some())
            .collect()
    }

    /// Where an entity has a page of its own, its address.
    fn entity_href(&self, code: &str) -> Option<String> {
        self.decl.entity.as_ref().map(|_| at(&format!("/entity/{}", urlencode(code))))
    }

    /// The time each thing is at, and every time there is, earliest first.
    fn times_of(&self, property: &str) -> (BTreeMap<String, String>, Vec<String>) {
        let of: BTreeMap<String, String> = self.words(property).into_iter().filter_map(|(k, w)| Some((k, w.first()?.clone()))).collect();
        let all: BTreeSet<String> = of.values().cloned().collect();
        (of, all.into_iter().collect())
    }

    /// The last year most of a source's figures are measured, every later one its projection: the
    /// year most of its things give for `actual_until`.
    fn actual_until(&self) -> BTreeMap<String, i64> {
        let Some(p) = self.decl.entity.as_ref().and_then(|e| e.actual_until.clone()) else { return BTreeMap::new() };
        let mut counts: BTreeMap<String, BTreeMap<i64, usize>> = BTreeMap::new();
        for (_, source, means) in self.store.said_of_property(&p) {
            if let Some(y) = means.iter().find_map(|m| year_in(m)) {
                *counts.entry(source).or_default().entry(y).or_default() += 1;
            }
        }
        counts.into_iter().filter_map(|(s, c)| Some((s, c.into_iter().max_by_key(|(_, n)| *n)?.0))).collect()
    }

    /// The latest time that is past and measured, and held for at least half as many things as
    /// the best covered time: this year once it is in, last year while it is not, and never the
    /// projections ahead. One rule for every picture that chooses a time for a reader.
    fn default_time(&self, per_time: &BTreeMap<String, usize>) -> Option<String> {
        let best = per_time.values().copied().max().unwrap_or(0);
        let now_year = year_of(&crate::iso_date(crate::now())).unwrap_or(9999);
        let measured = self.actual_until().values().copied().max().unwrap_or(now_year);
        per_time
            .iter()
            .filter(|(t, n)| **n * 2 >= best && year_of(t).is_some_and(|y| y <= now_year && y <= measured))
            .map(|(t, _)| t.clone())
            .last()
    }

    fn time_word(&self, property: &str, t: &str) -> String {
        display::shown(self.decl, property, t)
    }
}

/// A scale and legend for these values of a property.
fn scale_for(ctx: &Ctx, property: &str, values: &[f64], cheap: bool) -> Scale {
    Scale::of(values, ctx.value(property), cheap)
}

/// The classes of a scale, each a key that, clicked, shows only its own on the picture named.
fn legend(ctx: &Ctx, property: &str, scale: &Scale, counts: &[usize], picture: &str) -> Markup {
    let n = scale.colours.len();
    html! {
        div.legend.keys data-for=(picture) {
            @for i in 0..n {
                @let (lo, hi) = (if i == 0 { None } else { scale.edges.get(i - 1) }, scale.edges.get(i));
                button.key type="button" data-k=(i) title="Show only these" {
                    span.swatch style={"background:" (scale.colours[i])} {}
                    span.dim { @match (lo, hi) {
                        (None, Some(h)) => { "≤ " (ctx.fmt(property, *h)) },
                        (Some(l), Some(h)) => { (ctx.fmt(property, *l)) "–" (ctx.fmt(property, *h)) },
                        (Some(l), None) => { "> " (ctx.fmt(property, *l)) },
                        _ => { "all" },
                    } @if let Some(c) = counts.get(i) { " · " (c) } }
                }
            }
        }
    }
}

/// Choices that make the page again with one parameter set: `colour=e10`, `by=2024`.
fn choices(ctx: &Ctx, here: &str, param: &str, options: &[String], chosen: &str) -> Markup {
    html! {
        @if options.len() > 1 {
            div.bar {
                @for o in options {
                    a.chip.on[o == chosen] rel="nofollow" href=(with_param(here, param, o)) { (ctx.label(o)) }
                }
            }
        }
    }
}

fn with_param(here: &str, param: &str, value: &str) -> String {
    let (path, query) = here.split_once('?').unwrap_or((here, ""));
    let mut kept: Vec<String> = query.split('&').filter(|p| !p.is_empty() && !p.starts_with(&format!("{param}="))).map(str::to_string).collect();
    kept.push(format!("{param}={}", urlencode(value)));
    format!("{path}?{}", kept.join("&"))
}

/// A note of what a picture left out: values outside the range a real one falls in, and values
/// older than they stay current.
fn left_out(n: &Numbers) -> Markup {
    html! {
        @if n.implausible > 0 || n.stale > 0 {
            " Left out: "
            @if n.implausible > 0 { (n.implausible) " outside the range a real value falls in" }
            @if n.implausible > 0 && n.stale > 0 { ", " }
            @if n.stale > 0 { (n.stale) " older than they stay current" }
            "."
        }
    }
}

// -- the pictures, over every thing ---------------------------------------------------------------

/// What `view.show` declares, drawn for the overview. Each kept while the sources stand where they
/// stood, per the choices the reader made.
pub fn overview(ctx: &Ctx, show: &[Show], params: &BTreeMap<String, String>) -> Markup {
    html! {
        @for (i, s) in show.iter().enumerate() {
            (PreEscaped(picture(ctx, s, i, params)))
            @if let Some(base) = &ctx.embed_base {
                (embed_code(&format!("{base}/embed/{i}{}", query_of(&choices_of(s, params))), &title_of_show(ctx, s), height_of(s)))
            }
        }
        (PreEscaped(SCRIPT))
    }
}

/// One picture of the overview as it is shown: kept, the last one while new data's is drawn ahead,
/// or drawn now; its links put back where the tracker is served.
pub fn picture(ctx: &Ctx, s: &Show, i: usize, params: &BTreeMap<String, String>) -> String {
    let mine = choices_of(s, params);
    let question = question_of(s, i, &mine);
    let drawn = match ctx.store.drawn(&question, &ctx.standing) {
        Some(d) => d,
        None => match ctx.store.drawn_any(&question).filter(|_| ctx.warming) {
            Some(d) => d,
            None => draw(ctx, s, i, &mine),
        },
    };
    drawn.replace(MOUNTED, &crate::serve::mounted())
}

/// `?k=v&…` of a picture's own choices, or nothing.
fn query_of(mine: &BTreeMap<String, String>) -> String {
    if mine.is_empty() {
        return String::new();
    }
    format!("?{}", mine.iter().map(|(k, v)| format!("{k}={}", urlencode(v))).collect::<Vec<_>>().join("&"))
}

/// What a picture is called, for the title of the frame it is embedded in.
pub fn title_of_show(ctx: &Ctx, s: &Show) -> String {
    let named = |t: &str, v: &str| if t.is_empty() { ctx.label(v) } else { t.to_string() };
    match s {
        Show::Map(m) => named(&m.title, m.colour.first().map(String::as_str).unwrap_or("")),
        Show::Areas(a) => named(&a.title, a.value.first().map(String::as_str).unwrap_or("")),
        Show::Series(x) => named(&x.title, x.value.first().map(String::as_str).unwrap_or("")),
        Show::Spread(x) => named(&x.title, x.value.first().map(String::as_str).unwrap_or("")),
        Show::Distribution(d) => named(&d.title, d.value.first().map(String::as_str).unwrap_or("")),
        Show::Matrix(m) => named(&m.title, &m.rows),
        Show::Ranking(r) => named(&r.title, &r.value),
        Show::Arrivals(a) => named(&a.title, &a.date),
        Show::Table(t) => named(&t.title, &t.entity),
        Show::Averages(a) => named(&a.title, a.value.first().map(String::as_str).unwrap_or("")),
        Show::Overlap(o) => named(&o.title, "sources"),
        Show::History(h) => named(&h.title, h.value.first().map(String::as_str).unwrap_or("")),
        Show::Memory(m) => named(&m.title, &m.parameters),
    }
}

/// How tall a frame a picture wants, before it says so itself.
pub fn height_of(s: &Show) -> u32 {
    match s {
        Show::Map(_) => 760,
        Show::Areas(_) => 620,
        Show::Table(_) | Show::Ranking(_) | Show::Averages(_) => 640,
        Show::Matrix(_) | Show::Overlap(_) => 480,
        _ => 420,
    }
}

/// The code that puts a page of Zetlyn into somebody else's: a frame, and a line that makes it as
/// tall as what it shows.
pub fn embed_code(src: &str, title: &str, height: u32) -> Markup {
    let code = format!(
        "<iframe src=\"{src}\" title=\"{}\" width=\"100%\" height=\"{height}\" style=\"border:0\" loading=\"lazy\"></iframe>\n<script>addEventListener('message',function(e){{var d=e.data;if(d&&d.zetlynEmbed)document.querySelectorAll('iframe[src=\"'+d.zetlynEmbed+'\"]').forEach(function(f){{f.style.height=d.height+'px'}})}})</script>",
        title.replace('"', "&quot;")
    );
    html! {
        details.embed {
            summary { "Embed" }
            p.dim { "Put this where the picture should be: it shows what the tracker holds as it is updated, and links back here." }
            textarea.embed-code readonly rows="4" onclick="this.select()" { (code) }
            p.dim.small { a href=(src) { "Open it on its own" } }
        }
    }
}

/// Where a picture is kept drawn, a stand-in for where its tracker is served from, put back as
/// it is shown: one picture for every address the tracker answers at, and one drawn ahead by a
/// thread that answers none.
const MOUNTED: &str = "/~zetlyn-mounted~";

/// The choices a reader made that a picture is drawn from: only its own, so a colour chosen on
/// the map draws no other picture again.
fn choices_of(show: &Show, params: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let own: &[&str] = match show {
        Show::Map(_) => &["colour"],
        Show::Areas(_) => &["value", "by", "as", "area"],
        Show::Series(_) => &["value", "entity"],
        Show::Spread(_) => &["value", "by"],
        Show::Distribution(_) => &["value"],
        Show::Table(_) => &["at", "sort"],
        _ => &[],
    };
    params.iter().filter(|(k, _)| own.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// What a picture is kept under. The version in it: a picture drawn by an older program is drawn
/// again by a newer one.
fn question_of(show: &Show, i: usize, mine: &BTreeMap<String, String>) -> String {
    format!("overview|{}|{i}|{}|{:?}", env!("CARGO_PKG_VERSION"), serde_json::to_string(show).unwrap_or_default(), mine)
}

/// A picture drawn and kept, its links under the stand-in for where it is served.
fn draw(ctx: &Ctx, s: &Show, i: usize, mine: &BTreeMap<String, String>) -> String {
    let real = crate::serve::mounted();
    crate::serve::mount(MOUNTED);
    let query: Vec<String> = mine.iter().map(|(k, v)| format!("{k}={}", urlencode(v))).collect();
    let here = if query.is_empty() { at("/") } else { format!("{}?{}", at("/"), query.join("&")) };
    let d = overview_one(ctx, s, mine, &here, i).into_string();
    crate::serve::mount(&real);
    ctx.store.keep_drawn(&question_of(s, i, mine), &ctx.standing, &d);
    d
}

/// Every choice the overview's pictures offer a reader by a button: each picture as it opens, and
/// with each value, colour and map its buttons choose. What new data is drawn ahead for.
fn variants(show: &Show) -> Vec<BTreeMap<String, String>> {
    let one = |k: &str, v: &str| BTreeMap::from([(k.to_string(), v.to_string())]);
    let mut out = vec![BTreeMap::new()];
    match show {
        Show::Map(m) => out.extend(m.colour.iter().skip(1).map(|c| one("colour", c))),
        Show::Areas(a) => {
            out.extend(a.value.iter().skip(1).map(|v| one("value", v)));
            out.extend(["list", "map"].map(|v| one("as", v)));
            out.extend(["world", "europe"].map(|v| BTreeMap::from([("as".to_string(), "map".to_string()), ("area".to_string(), v.to_string())])));
        }
        Show::Series(s) => out.extend(s.value.iter().skip(1).map(|v| one("value", v))),
        Show::Spread(s) => out.extend(s.value.iter().skip(1).map(|v| one("value", v))),
        Show::Distribution(d) => out.extend(d.value.iter().skip(1).map(|v| one("value", v))),
        _ => {}
    }
    out
}

/// Every picture of an overview, drawn now for what the store holds, unless it is drawn already;
/// and each distribution a thing's page marks its own value on. How many were drawn.
pub fn draw_ahead(ctx: &Ctx, show: &[Show], things: &[Show]) -> usize {
    let mut drawn = 0;
    for (i, s) in show.iter().enumerate() {
        for mine in variants(s) {
            if ctx.store.drawn(&question_of(s, i, &mine), &ctx.standing).is_none() {
                draw(ctx, s, i, &mine);
                drawn += 1;
            }
        }
    }
    for s in things {
        if let Show::Distribution(d) = s {
            for v in &d.value {
                let log = d.log || ctx.value(v).is_some_and(|x| x.log);
                spread_of(ctx, v, log);
                drawn += 1;
            }
        }
    }
    drawn
}

fn overview_one(ctx: &Ctx, show: &Show, params: &BTreeMap<String, String>, here: &str, i: usize) -> Markup {
    let id = format!("picture-{i}");
    match show {
        Show::Map(m) => point_map(ctx, m, params, here, &id),
        Show::Areas(a) => area_map(ctx, a, params, here, &id),
        Show::Series(s) => {
            let entity = params.get("entity").cloned().or_else(|| s.first.clone()).unwrap_or_default();
            series(ctx, s, &entity, None, params, here, true)
        }
        Show::Spread(s) => spread_overview(ctx, s, params, here),
        Show::Distribution(d) => distribution(ctx, d, None, params, here),
        Show::Matrix(m) => matrix(ctx, m),
        Show::Ranking(r) => ranking(ctx, r),
        Show::Arrivals(a) => arrivals(ctx, a),
        Show::Table(t) => table(ctx, t, params, here),
        Show::Averages(a) => averages(ctx, a, params, here),
        Show::Overlap(o) => overlap(ctx, o),
        Show::History(_) | Show::Memory(_) => html! {},
    }
}

fn point_map(ctx: &Ctx, m: &MapSpec, params: &BTreeMap<String, String>, here: &str, id: &str) -> Markup {
    let chosen = params.get("colour").filter(|c| m.colour.contains(c)).cloned().or_else(|| m.colour.first().cloned()).unwrap_or_default();
    let places: BTreeMap<String, (f64, f64)> = ctx
        .words(&m.place)
        .into_iter()
        .filter_map(|(k, w)| Some((k, w.iter().find_map(|s| crate::geo::place_of(s))?)))
        .collect();
    let numbers = ctx.numbers(&chosen);
    let mut shown: Vec<(String, f64, f64, Option<f64>)> = places.iter().map(|(k, &(lat, lon))| (k.clone(), lat, lon, numbers.by.get(k).map(|v| middle(v)))).collect();
    if shown.is_empty() {
        return html! {};
    }
    // The extent of where most things are: a station on La Réunion does not shrink France.
    let mut lats: Vec<f64> = shown.iter().map(|s| s.1).collect();
    let mut lons: Vec<f64> = shown.iter().map(|s| s.2).collect();
    lats.sort_by(cmp);
    lons.sort_by(cmp);
    let bounds = (quantile(&lons, 0.005), quantile(&lats, 0.005), quantile(&lons, 0.995), quantile(&lats, 0.995));
    let frame = crate::geo::Frame::fit(bounds, 900.0);
    let all: Vec<f64> = shown.iter().filter_map(|s| s.3).collect();
    let scale = scale_for(ctx, &chosen, &all, m.cheap);
    let mut counts = vec![0usize; scale.colours.len()];
    for v in &all {
        counts[scale.class(*v)] += 1;
    }
    // The best drawn last, on top: the cheapest station is not hidden under its neighbours.
    let lower_better = ctx.value(&chosen).and_then(|d| d.better.as_deref()).map(|b| b == "lower").unwrap_or(m.cheap);
    shown.sort_by(|a, b| match (a.3, b.3) {
        (Some(x), Some(y)) => if lower_better { cmp(&y, &x) } else { cmp(&x, &y) },
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        _ => Ordering::Equal,
    });
    let without = shown.iter().filter(|s| s.3.is_none()).count();
    // Names on the map, from the things' own words: the towns with most things first, more of them
    // the closer one looks.
    let labels: Vec<(String, f64, f64, usize)> = match &m.label {
        Some(p) => {
            let mut at: BTreeMap<String, (f64, f64, usize)> = BTreeMap::new();
            for (k, w) in ctx.words(p) {
                if let (Some(name), Some((lat, lon))) = (w.first(), places.get(&k)) {
                    let e = at.entry(name.clone()).or_insert((0.0, 0.0, 0));
                    e.0 += lat;
                    e.1 += lon;
                    e.2 += 1;
                }
            }
            let mut v: Vec<(String, f64, f64, usize)> = at.into_iter().map(|(n, (la, lo, c))| (n, la / c as f64, lo / c as f64, c)).collect();
            v.sort_by(|a, b| b.3.cmp(&a.3));
            v.truncate(600);
            v
        }
        None => Vec::new(),
    };
    let ages = ctx.value(&chosen).and_then(|d| d.at.clone()).map(|p| ctx.words(&p)).unwrap_or_default();
    let now = crate::now();
    // Seen whole, the places are hexagons, each the average of the places in it: ten thousand
    // stations, or thirty thousand in three countries, read as one surface. Closer, each place.
    let hexes = hexbins(shown.iter().filter_map(|(_, lat, lon, v)| Some((frame.point(*lon, *lat), (*v)?))));
    html! {
        section.chart {
            h2 { (if m.title.is_empty() { format!("{} by place", ctx.label(&chosen)) } else { m.title.clone() }) }
            (choices(ctx, here, "colour", &m.colour, &chosen))
            div.map {
                svg.zoomable.hexed id=(id) viewBox={"0 0 " (frame.width as i64) " " (frame.height as i64)} role="img" aria-label=(m.title) {
                    @for c in crate::geo::countries().iter().filter(|c| frame.shows(c)) {
                        path.land d=(frame.path(c)) { title { (c.name) } }
                    }
                    @for ((x, y), mean, n) in &hexes {
                        path class=(format!("hex k k{}", scale.class(*mean))) d=(hexagon(*x, *y)) fill=(scale.colour(*mean)) {
                            title { (n) " " (if *n == 1 { "place" } else { "places" }) ", on average " (ctx.fmt(&chosen, *mean)) }
                        }
                    }
                    @for (key, lat, lon, v) in &shown {
                        @let (x, y) = frame.point(*lon, *lat);
                        @if x >= -5.0 && y >= -5.0 && x <= frame.width + 5.0 && y <= frame.height + 5.0 {
                            @let (fill, class) = match v { Some(v) => (scale.colour(*v), format!("dot k k{}", scale.class(*v))), None => ("#bbb", "dot k none".to_string()) };
                            @let age = ages.get(key).and_then(|w| w.iter().filter_map(|s| crate::thingquery::seconds(s)).max()).map(|t| crate::tracker::human(now - t));
                            a href=(ctx.href(key)) {
                                circle class=(class) cx=(format!("{x:.1}")) cy=(format!("{y:.1}")) r="2.2" fill=(fill) {
                                    title { (ctx.title(key)) " · " (v.map(|v| ctx.fmt(&chosen, v)).unwrap_or_else(|| "no price".into())) @if let Some(a) = age { " · " (a) " ago" } }
                                }
                            }
                        }
                    }
                    @for (i, (name, lat, lon, _)) in labels.iter().enumerate() {
                        @let (x, y) = frame.point(*lon, *lat);
                        text.place.(if i < 12 { "t1" } else if i < 80 { "t2" } else { "t3" }) x=(format!("{x:.1}")) y=(format!("{y:.1}")) text-anchor="middle" { (name) }
                    }
                }
            }
            (legend(ctx, &chosen, &scale, &counts, id))
            p.dim { (all.len()) " places with " (ctx.label(&chosen)) @if without > 0 { ", " (without) " without (grey)" } "." (left_out(&numbers)) " Scroll or pinch to zoom, drag to move; a colour below shows only its places." }
        }
    }
}

/// Whether a change is good or bad news, by which way its value is better; neither where it
/// reads as no change at all.
fn tone(ctx: &Ctx, property: &str, d: f64) -> &'static str {
    if display::number(d.abs(), ctx.value(property)).chars().all(|c| !c.is_ascii_digit() || c == '0') {
        return "";
    }
    match ctx.value(property).and_then(|v| v.better.as_deref()) {
        Some("lower") if d < 0.0 => "good",
        Some("lower") => "bad",
        Some("higher") if d > 0.0 => "good",
        Some("higher") => "bad",
        _ => "",
    }
}

/// The radius of a hexagon on a map 900 wide.
const HEX: f64 = 9.0;

/// Points in hexagons, pointed at the top: each hexagon's centre, the average of its values and
/// how many there are.
fn hexbins(points: impl Iterator<Item = ((f64, f64), f64)>) -> Vec<((f64, f64), f64, usize)> {
    let s3 = 3f64.sqrt();
    let mut cells: BTreeMap<(i64, i64), (f64, usize)> = BTreeMap::new();
    for ((x, y), v) in points {
        // Axial coordinates, rounded to the nearest hexagon through its cube coordinates.
        let q = (s3 / 3.0 * x - y / 3.0) / HEX;
        let r = (2.0 / 3.0 * y) / HEX;
        let (cx, cz) = (q, r);
        let cy = -cx - cz;
        let (mut rx, ry, mut rz) = (cx.round(), cy.round(), cz.round());
        let (dx, dy, dz) = ((rx - cx).abs(), (ry - cy).abs(), (rz - cz).abs());
        if dx > dy && dx > dz {
            rx = -ry - rz;
        } else if dy <= dz {
            rz = -rx - ry;
        }
        let e = cells.entry((rx as i64, rz as i64)).or_insert((0.0, 0));
        e.0 += v;
        e.1 += 1;
    }
    cells
        .into_iter()
        .map(|((q, r), (sum, n))| ((HEX * s3 * (q as f64 + r as f64 / 2.0), HEX * 1.5 * r as f64), sum / n as f64, n))
        .collect()
}

fn hexagon(x: f64, y: f64) -> String {
    let mut d = String::new();
    for i in 0..6 {
        let a = std::f64::consts::PI / 180.0 * (60.0 * i as f64 - 30.0);
        d.push_str(&format!("{}{:.1},{:.1}", if i == 0 { "M" } else { "L" }, x + HEX * a.cos(), y + HEX * a.sin()));
    }
    d.push('Z');
    d
}

fn area_map(ctx: &Ctx, a: &AreasSpec, params: &BTreeMap<String, String>, here: &str, id: &str) -> Markup {
    let chosen = params.get("value").filter(|c| a.value.contains(c)).cloned().or_else(|| a.value.first().cloned()).unwrap_or_default();
    // The first of the properties naming a country that a thing has: where one list writes the
    // nationality and another only the address.
    let mut country: BTreeMap<String, String> = BTreeMap::new();
    for p in &a.country {
        for (k, w) in ctx.words(p) {
            if !country.contains_key(&k) {
                if let Some(c) = w.iter().find_map(|s| crate::geo::iso3_of(s)) {
                    country.insert(k, c);
                }
            }
        }
    }
    // Without a value, how many things each country has: listings by country.
    let counting = a.value.is_empty();
    let numbers = if counting { Numbers { by: country.keys().map(|k| (k.clone(), vec![(String::new(), 1.0)])).collect(), ..Numbers::default() } } else { ctx.numbers(&chosen) };
    let (time_of, times) = match &a.by {
        Some(by) => ctx.times_of(by),
        None => (BTreeMap::new(), Vec::new()),
    };
    let mut per_time: BTreeMap<String, usize> = BTreeMap::new();
    for (k, t) in &time_of {
        if numbers.by.contains_key(k) {
            *per_time.entry(t.clone()).or_default() += 1;
        }
    }
    let time = params.get("by").filter(|t| times.contains(t)).cloned().or_else(|| ctx.default_time(&per_time)).unwrap_or_default();
    // One value a country: the middle of what its sources say, at the time chosen.
    let mut per_country: BTreeMap<String, (f64, Vec<(String, f64)>, String)> = BTreeMap::new();
    for (k, v) in &numbers.by {
        if a.by.is_some() && time_of.get(k) != Some(&time) {
            continue;
        }
        if let Some(c) = country.get(k) {
            if counting {
                per_country.entry(c.clone()).or_insert((0.0, Vec::new(), String::new())).0 += 1.0;
            } else {
                per_country.insert(c.clone(), (middle(v), v.clone(), k.clone()));
            }
        }
    }
    let all: Vec<f64> = per_country.values().map(|v| v.0).collect();
    let scale = if counting { Scale::of(&all, None, false) } else { scale_for(ctx, &chosen, &all, a.cheap) };
    let mut counts = vec![0usize; scale.colours.len()];
    for v in &all {
        counts[scale.class(*v)] += 1;
    }
    let position = times.iter().position(|t| *t == time);
    let by = a.by.clone().unwrap_or_default();
    let fmt = |v: f64| if counting { display::number(v, None) } else { ctx.fmt(&chosen, v) };
    // A map where there are enough countries to fill one, a list where there are not: forty
    // countries on a map of the world are mostly grey.
    let as_map = match params.get("as").map(String::as_str) {
        Some("map") => true,
        Some("list") => false,
        _ => per_country.len() >= a.min_map,
    };
    // Europe where most countries with a figure are in it, the world otherwise.
    let in_europe = per_country.keys().filter(|c| crate::geo::atlas("europe").shapes.iter().any(|s| s.iso3 == **c)).count();
    let region = params.get("area").filter(|r| *r == "world" || *r == "europe").cloned().unwrap_or_else(|| if in_europe * 10 >= per_country.len() * 7 { "europe".into() } else { "world".into() });
    let atlas = crate::geo::atlas(&region);
    let name = |iso3: &str| crate::geo::country(iso3).map(|c| c.name.clone()).unwrap_or_else(|| iso3.to_string());
    let href = |iso3: &str, key: &str| if key.is_empty() { at(&format!("/?q={}", urlencode(&name(iso3)))) } else { ctx.entity_href(iso3).unwrap_or_else(|| ctx.href(key)) };
    let says = |iso3: &str, v: f64, said: &[(String, f64)]| -> String {
        let mut s = format!("{}: {}", name(iso3), fmt(v));
        if said.len() > 1 {
            for (src, n) in said {
                s.push_str(&format!(" · {} {}", ctx.source_title(src), fmt(*n)));
            }
        }
        s
    };
    let mut listed: Vec<(&String, &(f64, Vec<(String, f64)>, String))> = per_country.iter().collect();
    listed.sort_by(|x, y| cmp(&y.1 .0, &x.1 .0));
    // The bars of the list start at the value's centre where it has one, at nought otherwise.
    let base = ctx.value(&chosen).and_then(|d| d.centre).filter(|_| !counting).unwrap_or(0.0);
    let reach = listed.iter().map(|(_, v)| (v.0 - base).abs()).fold(1e-9, f64::max);
    let two_sided = listed.iter().any(|(_, v)| v.0 < base);
    html! {
        section.chart {
            h2 { (if a.title.is_empty() { ctx.label(&chosen) } else { a.title.clone() }) @if !time.is_empty() { " · " (ctx.time_word(&by, &time)) } }
            (choices(ctx, here, "value", &a.value, &chosen))
            div.bar {
                @if let Some(p) = position {
                    @if p > 0 { a.chip rel="nofollow" href=(with_param(here, "by", &times[p - 1])) { "← " (ctx.time_word(&by, &times[p - 1])) } }
                    span.chip.on { (ctx.time_word(&by, &time)) }
                    @if p + 1 < times.len() { a.chip rel="nofollow" href=(with_param(here, "by", &times[p + 1])) { (ctx.time_word(&by, &times[p + 1])) " →" } }
                    span.gap {}
                }
                a.chip.on[!as_map] rel="nofollow" href=(with_param(here, "as", "list")) { "List" }
                a.chip.on[as_map && region == "world"] rel="nofollow" href=(with_param(&with_param(here, "as", "map"), "area", "world")) { "World" }
                a.chip.on[as_map && region == "europe"] rel="nofollow" href=(with_param(&with_param(here, "as", "map"), "area", "europe")) { "Europe" }
            }
            @if as_map {
                div.map {
                    svg.zoomable.atlas id=(id) viewBox={"0 0 " (atlas.width) " " (atlas.height)} style={"width:" (format!("{:.0}", (560.0 * atlas.width / atlas.height).min(1150.0))) "px;max-width:100%;height:auto;margin:0 auto"} role="img" {
                        @for s in atlas.shapes.iter().filter(|s| s.path.is_some()) {
                            @let d = s.path.as_deref().unwrap_or("");
                            @match per_country.get(&s.iso3) {
                                Some((v, said, key)) => {
                                    a href=(href(&s.iso3, key)) {
                                        path class=(format!("area k k{}", scale.class(*v))) d=(d) fill=(scale.colour(*v)) { title { (says(&s.iso3, *v, said)) } }
                                    }
                                }
                                None => { path.land d=(d) { title { (name(&s.iso3)) ": no figure" } } }
                            }
                        }
                        // A country too small to see, where there is something to say of it.
                        @for s in atlas.shapes.iter().filter(|s| s.point.is_some()) {
                            @if let (Some((x, y)), Some((v, said, key))) = (s.point, per_country.get(&s.iso3)) {
                                a href=(href(&s.iso3, key)) {
                                    circle class=(format!("small k k{}", scale.class(*v))) cx=(x) cy=(y) r="3" fill=(scale.colour(*v)) { title { (says(&s.iso3, *v, said)) } }
                                }
                            }
                        }
                    }
                }
                (legend(ctx, &chosen, &scale, &counts, id))
            } @else {
                table.ranking.bars {
                    tbody {
                        @for (i, (iso3, (v, said, key))) in listed.iter().enumerate() {
                            @let w = (v - base).abs() / reach * if two_sided { 50.0 } else { 100.0 };
                            @let left = if two_sided { if *v < base { 50.0 - w } else { 50.0 } } else { 0.0 };
                            tr title=(says(iso3, *v, said)) {
                                td.num.dim { (i + 1) }
                                td { a href=(href(iso3, key)) { (name(iso3)) } }
                                td.barcell { div.track.(if two_sided { "centred" } else { "" }) { span.fill style={"left:" (format!("{left:.1}")) "%;width:" (format!("{:.1}", w.max(0.4))) "%;background:" (scale.colour(*v))} {} } }
                                td.num { (fmt(*v)) @if said.len() > 1 && !counting && differ(ctx, &chosen, said) { " " span.chip.on.small { "differ" } } }
                            }
                        }
                    }
                }
            }
            p.dim { (per_country.len()) " countries." @if counting { " How many things each has." } @else { " The middle of what the sources say where they differ; hover a country for each." } (left_out(&numbers)) }
        }
    }
}

/// How far apart the sources are on the things at the time chosen, widest first.
fn spread_overview(ctx: &Ctx, s: &SpreadSpec, params: &BTreeMap<String, String>, here: &str) -> Markup {
    let chosen = params.get("value").filter(|c| s.value.contains(c)).cloned().or_else(|| s.value.first().cloned()).unwrap_or_default();
    let values = ctx.numbers(&chosen).by;
    let (time_of, times) = match &s.time {
        Some(t) => ctx.times_of(t),
        None => (BTreeMap::new(), Vec::new()),
    };
    let mut per_time: BTreeMap<String, usize> = BTreeMap::new();
    for (k, t) in &time_of {
        if values.get(k).is_some_and(|v| v.len() >= 2) {
            *per_time.entry(t.clone()).or_default() += 1;
        }
    }
    let time = params.get("by").filter(|t| times.contains(t)).cloned().or_else(|| ctx.default_time(&per_time)).unwrap_or_default();
    let mut rows: Vec<(String, f64, &Vec<(String, f64)>)> = values
        .iter()
        .filter(|(k, v)| v.len() >= 2 && (s.time.is_none() || time_of.get(*k) == Some(&time)))
        .map(|(k, v)| {
            let lo = v.iter().map(|x| x.1).fold(f64::INFINITY, f64::min);
            let hi = v.iter().map(|x| x.1).fold(f64::NEG_INFINITY, f64::max);
            (k.clone(), hi - lo, v)
        })
        .collect();
    rows.sort_by(|a, b| cmp(&b.1, &a.1));
    rows.truncate(20);
    let sources: BTreeSet<String> = rows.iter().flat_map(|r| r.2.iter().map(|x| x.0.clone())).collect();
    let time_prop = s.time.clone().unwrap_or_default();
    html! {
        section.chart {
            h2 { (if s.title.is_empty() { format!("Where the sources differ most on {}", ctx.label(&chosen).to_lowercase()) } else { s.title.clone() }) @if !time.is_empty() { " · " (ctx.time_word(&time_prop, &time)) } }
            (choices(ctx, here, "value", &s.value, &chosen))
            @if rows.is_empty() { p.dim { "No thing has this from two sources." } }
            @else {
                (source_key(ctx, &sources))
                (spread_rows(ctx, &chosen, &rows.iter().map(|r| (r.0.clone(), r.2.clone())).collect::<Vec<_>>(), &sources))
            }
        }
    }
}

fn source_colour(sources: &BTreeSet<String>, s: &str) -> &'static str {
    const C: [&str; 6] = ["#2166ac", "#d6604d", "#4d9221", "#8073ac", "#e08214", "#35978f"];
    C[sources.iter().position(|x| x == s).unwrap_or(0) % C.len()]
}

fn source_key(ctx: &Ctx, sources: &BTreeSet<String>) -> Markup {
    html! {
        div.legend {
            @for s in sources {
                span.swatch.round style={"background:" (source_colour(sources, s))} {}
                span.dim { (ctx.source_title(s)) }
            }
        }
    }
}

/// A row a thing: its sources' values as dots on one axis, and how far apart the two ends are.
fn spread_rows(ctx: &Ctx, property: &str, rows: &[(String, Vec<(String, f64)>)], sources: &BTreeSet<String>) -> Markup {
    let all: Vec<f64> = rows.iter().flat_map(|r| r.1.iter().map(|x| x.1)).collect();
    let (lo, hi) = (all.iter().copied().fold(f64::INFINITY, f64::min), all.iter().copied().fold(f64::NEG_INFINITY, f64::max));
    let span = (hi - lo).max(1e-9);
    let (left, width) = (230.0, 560.0);
    let x = |v: f64| left + (v - lo) / span * width;
    let height = rows.len() as f64 * 22.0 + 26.0;
    html! {
        svg.spread viewBox={"0 0 900 " (height as i64)} {
            @for t in ticks(lo, hi, 6) {
                line.grid x1=(x(t)) x2=(x(t)) y1="0" y2=(height - 18.0) {}
                text.tick x=(x(t)) y=(height - 4.0) text-anchor="middle" { (short(t)) }
            }
            @for (i, (key, said)) in rows.iter().enumerate() {
                @let y = i as f64 * 22.0 + 12.0;
                @let lo_r = said.iter().map(|s| s.1).fold(f64::INFINITY, f64::min);
                @let hi_r = said.iter().map(|s| s.1).fold(f64::NEG_INFINITY, f64::max);
                a href=(ctx.href(key)) {
                    text.label x=(left - 8.0) y=(y + 4.0) text-anchor="end" { (clip(&ctx.title(key), 34)) }
                }
                line.range x1=(x(lo_r)) x2=(x(hi_r)) y1=(y) y2=(y) {}
                @for (s, v) in said {
                    circle cx=(format!("{:.1}", x(*v))) cy=(y) r="5" fill=(source_colour(sources, s)) { title { (ctx.source_title(s)) ": " (ctx.fmt(property, *v)) } }
                }
                text.tick x=(left + width + 12.0) y=(y + 4.0) { "±" (ctx.fmt(property, (hi_r - lo_r) / 2.0)) }
            }
        }
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n - 1).collect::<String>().trim_end())
    } else {
        s.to_string()
    }
}

/// Round numbers along an axis.
fn ticks(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    let span = (hi - lo).max(1e-9);
    let raw = span / n as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0].iter().map(|m| m * mag).find(|s| *s >= raw).unwrap_or(raw);
    let mut out = Vec::new();
    let mut t = (lo / step).ceil() * step;
    while t <= hi + step * 1e-9 {
        out.push(if t.abs() < step * 1e-9 { 0.0 } else { t });
        t += step;
    }
    out
}

/// One entity's values over time, a line per source; a thing's own time marked on its page.
fn series(ctx: &Ctx, s: &SeriesSpec, entity: &str, mark: Option<&str>, params: &BTreeMap<String, String>, here: &str, choosing: bool) -> Markup {
    let chosen = params.get("value").filter(|c| s.value.contains(c)).cloned().or_else(|| s.value.first().cloned()).unwrap_or_default();
    let wanted = ctx.entity_code(&s.entity, entity);
    let entity_of = ctx.entities_of(&s.entity);
    let (time_of, _) = ctx.times_of(&s.time);
    let values = ctx.numbers(&chosen).by;
    let mut lines: BTreeMap<String, Vec<(String, f64)>> = BTreeMap::new();
    for (k, said) in values.iter().filter(|(k, _)| entity_of.get(*k) == Some(&wanted)) {
        let Some(t) = time_of.get(k) else { continue };
        for (src, v) in said {
            lines.entry(src.clone()).or_default().push((t.clone(), *v));
        }
    }
    let times: Vec<String> = lines.values().flatten().map(|p| p.0.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    let entities: Vec<String> = if choosing { entity_of.values().cloned().collect::<BTreeSet<_>>().into_iter().collect() } else { Vec::new() };
    let mut names: Vec<(String, String)> = entities.iter().map(|e| (e.clone(), ctx.entity_name(&s.entity, e))).collect();
    names.sort_by(|a, b| a.1.cmp(&b.1));
    let name = ctx.entity_name(&s.entity, &wanted);
    html! {
        section.chart {
            h2 { (if s.title.is_empty() { ctx.label(&chosen) } else { s.title.clone() }) " · "
                @match ctx.entity_href(&wanted) { Some(h) => a href=(h) { (name) }, None => (name) } }
            (choices(ctx, here, "value", &s.value, &chosen))
            @if choosing && !names.is_empty() {
                form.bar method="get" {
                    @for (k, v) in params.iter().filter(|(k, _)| k.as_str() != "entity") { input type="hidden" name=(k) value=(v); }
                    select name="entity" onchange="this.form.submit()" {
                        @for (code, n) in &names { option value=(code) selected[*code == wanted] { (n) } }
                    }
                    noscript { button type="submit" { "Show" } }
                }
            }
            @if times.len() < 2 { p.dim { "Not enough points to draw a line." } }
            @else { (lines_chart(ctx, &chosen, &s.time, &lines, &times, mark)) }
        }
    }
}

/// Lines over a shared time axis. What a source projects, past the last time it measured or
/// past today where it does not say, dashed; a thing's own time a line across.
fn lines_chart(ctx: &Ctx, property: &str, time_prop: &str, lines: &BTreeMap<String, Vec<(String, f64)>>, times: &[String], mark: Option<&str>) -> Markup {
    let sources: BTreeSet<String> = lines.keys().cloned().collect();
    let until = ctx.actual_until();
    let now_year = year_of(&crate::iso_date(crate::now())).unwrap_or(9999);
    let all: Vec<f64> = lines.values().flatten().map(|p| p.1).collect();
    let (mut lo, mut hi) = (all.iter().copied().fold(f64::INFINITY, f64::min), all.iter().copied().fold(f64::NEG_INFINITY, f64::max));
    if lo > 0.0 && lo < hi * 0.3 {
        lo = 0.0;
    }
    if (hi - lo).abs() < 1e-9 {
        hi = lo + 1.0;
    }
    let (left, top, width, height) = (60.0, 10.0, 810.0, 260.0);
    let x = |t: &str| left + times.iter().position(|x| x == t).unwrap_or(0) as f64 / (times.len() - 1).max(1) as f64 * width;
    let y = |v: f64| top + (hi - v) / (hi - lo) * height;
    let every = (times.len() / 10).max(1);
    let projected = |src: &str, t: &str| match until.get(src) {
        Some(u) => year_of(t).is_some_and(|y| y > *u),
        None if !until.is_empty() => false,
        None => year_of(t).is_some_and(|y| y > now_year),
    };
    let any_projected = lines.iter().any(|(src, points)| points.iter().any(|p| projected(src, &p.0)));
    html! {
        (source_key(ctx, &sources))
        svg.series viewBox="0 0 900 300" {
            @for t in ticks(lo, hi, 5) {
                line.grid x1=(left) x2=(left + width) y1=(y(t)) y2=(y(t)) {}
                text.tick x=(left - 6.0) y=(y(t) + 4.0) text-anchor="end" { (ctx.fmt(property, t)) }
            }
            @if lo < 0.0 && hi > 0.0 { line.zero x1=(left) x2=(left + width) y1=(y(0.0)) y2=(y(0.0)) {} }
            @for (i, t) in times.iter().enumerate() {
                @if i % every == 0 { text.tick x=(x(t)) y=(top + height + 18.0) text-anchor="middle" { (ctx.time_word(time_prop, t)) } }
            }
            @if let Some(m) = mark.filter(|m| times.iter().any(|t| t == m)) {
                line.mark x1=(x(m)) x2=(x(m)) y1=(top) y2=(top + height) {}
            }
            @for (src, points) in lines {
                @let mut points = points.clone();
                @let _ = points.sort_by(|a, b| a.0.cmp(&b.0));
                @let first_ahead = points.iter().position(|p| projected(src, &p.0)).unwrap_or(points.len());
                @let past: Vec<String> = points[..first_ahead].iter().map(|p| format!("{:.1},{:.1}", x(&p.0), y(p.1))).collect();
                @let ahead: Vec<String> = points[first_ahead.saturating_sub(1)..].iter().filter(|_| first_ahead < points.len()).map(|p| format!("{:.1},{:.1}", x(&p.0), y(p.1))).collect();
                polyline.line points=(past.join(" ")) stroke=(source_colour(&sources, src)) {}
                @if ahead.len() > 1 { polyline.line.ahead points=(ahead.join(" ")) stroke=(source_colour(&sources, src)) {} }
                @for (t, v) in &points {
                    circle cx=(format!("{:.1}", x(t))) cy=(format!("{:.1}", y(*v))) r="2.5" fill=(source_colour(&sources, src)) { title { (ctx.source_title(src)) " · " (ctx.time_word(time_prop, t)) ": " (ctx.fmt(property, *v)) @if projected(src, t) { " (projected)" } } }
                }
            }
        }
        @if any_projected { p.dim { "Dashed: what a source projects, past the last year it measured." } }
    }
}

/// The step a property's values are written to, where they all are: CVSS to one decimal.
fn grid_of(values: &[f64]) -> Option<f64> {
    [1.0, 0.5, 0.1, 0.05, 0.01].into_iter().find(|g| values.iter().all(|v| ((v / g).round() * g - v).abs() < 1e-6))
}

/// How a value is spread over every thing, counted once while the store holds what it holds: the
/// same for each of 400,000 pages that marks its own thing on it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Spread {
    n: usize,
    lo: f64,
    hi: f64,
    width_of: f64,
    counts: Vec<u64>,
    below: usize,
    above: usize,
    middle: f64,
    /// Two hundred and one points, from the least to the most, for where a thing's value stands.
    points: Vec<f64>,
    implausible: usize,
    stale: usize,
}

impl Spread {
    /// The share of things below a value, from the points between the least and the most.
    fn below(&self, v: f64) -> f64 {
        let p = &self.points;
        if p.is_empty() {
            return 0.0;
        }
        let i = p.iter().filter(|x| **x < v).count();
        (i as f64 / (p.len() - 1).max(1) as f64 * 100.0).min(100.0)
    }
}

fn spread_of(ctx: &Ctx, property: &str, log: bool) -> Option<Spread> {
    let question = format!("spread|{}|{property}|{log}", env!("CARGO_PKG_VERSION"));
    if let Some(kept) = ctx.store.drawn(&question, &ctx.standing).and_then(|s| serde_json::from_str::<Option<Spread>>(&s).ok()) {
        return kept;
    }
    let made = count_spread(ctx, property, log);
    ctx.store.keep_drawn(&question, &ctx.standing, &serde_json::to_string(&made).unwrap_or_default());
    made
}

fn count_spread(ctx: &Ctx, property: &str, log: bool) -> Option<Spread> {
    let numbers = ctx.numbers(property);
    let mut all: Vec<f64> = numbers.by.values().map(|v| middle(v)).filter(|v| !log || *v > 0.0).collect();
    if all.len() < 3 {
        return None;
    }
    all.sort_by(cmp);
    let f = |v: f64| if log { v.log10() } else { v };
    // The bars over where most values are; the few far out counted beside them, not piled into
    // the end bars as if they were there.
    let (mut lo, mut hi) = (f(quantile(&all, 0.01)), f(quantile(&all, 0.99)));
    let grid = if log { None } else { grid_of(&all) };
    let (bins, width_of) = match grid {
        // Values written to a step, one bar a step, where that is few enough: CVSS, a rating.
        Some(g) if (hi - lo) / g <= 120.0 => {
            // Few enough steps from the least to the most: all of them, nothing counted beside.
            let (min, max) = (f(all[0]), f(all[all.len() - 1]));
            if (max - min) / g <= 120.0 {
                (lo, hi) = (min, max);
            }
            lo = (lo / g).round() * g - g / 2.0;
            hi = (hi / g).round() * g + g / 2.0;
            (((hi - lo) / g).round().max(1.0) as usize, g)
        }
        _ => (40usize, ((hi - lo) / 40.0).max(1e-9)),
    };
    let mut counts = vec![0u64; bins];
    let (mut below, mut above) = (0usize, 0usize);
    for v in &all {
        let p = (f(*v) - lo) / width_of;
        if p < 0.0 {
            below += 1;
        } else if p >= bins as f64 {
            if p < bins as f64 + 1e-9 {
                counts[bins - 1] += 1;
            } else {
                above += 1;
            }
        } else {
            counts[p.floor() as usize] += 1;
        }
    }
    Some(Spread {
        n: all.len(),
        lo,
        hi,
        width_of,
        counts,
        below,
        above,
        middle: quantile(&all, 0.5),
        points: (0..=200).map(|i| quantile(&all, i as f64 / 200.0)).collect(),
        implausible: numbers.implausible,
        stale: numbers.stale,
    })
}

fn distribution(ctx: &Ctx, d: &DistributionSpec, mark: Option<(&str, f64)>, params: &BTreeMap<String, String>, here: &str) -> Markup {
    let chosen = mark.map(|m| m.0.to_string()).or_else(|| params.get("value").filter(|c| d.value.contains(c)).cloned()).or_else(|| d.value.first().cloned()).unwrap_or_default();
    let log = d.log || ctx.value(&chosen).is_some_and(|v| v.log);
    let Some(s) = spread_of(ctx, &chosen, log) else { return html! {} };
    let f = |v: f64| if log { v.log10() } else { v };
    let inv = |v: f64| if log { 10f64.powf(v) } else { v };
    let (lo, hi, bins, width_of) = (s.lo, s.hi, s.counts.len(), s.width_of);
    let most = s.counts.iter().copied().max().unwrap_or(1).max(1) as f64;
    let (left, width, height) = (40.0, 830.0, 160.0);
    let xv = |v: f64| left + (f(v) - lo) / (hi - lo).max(1e-9) * width;
    let mx = mark.map(|(_, v)| xv(v).clamp(left, left + width));
    let below_pct = mark.map(|(_, v)| s.below(v));
    let fmt = |v: f64| ctx.fmt(&chosen, v);
    let numbers = Numbers { implausible: s.implausible, stale: s.stale, ..Numbers::default() };
    html! {
        section.chart {
            h2 { (if d.title.is_empty() { format!("How {} is spread", ctx.label(&chosen).to_lowercase()) } else { d.title.clone() }) }
            @if mark.is_none() { (choices(ctx, here, "value", &d.value, &chosen)) }
            svg.histogram viewBox="0 0 900 200" {
                @for (i, c) in s.counts.iter().enumerate() {
                    @let h = *c as f64 / most * height;
                    rect.bin x=(format!("{:.1}", left + i as f64 * width / bins as f64)) y=(format!("{:.1}", 10.0 + height - h)) width=(format!("{:.1}", (width / bins as f64 - 1.0).max(0.5))) height=(format!("{h:.1}")) {
                        title { (c) " from " (fmt(inv(lo + i as f64 * width_of))) " to " (fmt(inv(lo + (i + 1) as f64 * width_of))) }
                    }
                }
                @for t in ticks(lo, hi, 6) {
                    text.tick x=(left + (t - lo) / (hi - lo).max(1e-9) * width) y="190" text-anchor="middle" { (if log && ctx.value(&chosen).is_none() { short(inv(t)) } else { fmt(inv(t)) }) }
                }
                @if s.below > 0 { text.tick x=(left) y="8" text-anchor="start" { "← " (s.below) " lower" } }
                @if s.above > 0 { text.tick x=(left + width) y="8" text-anchor="end" { (s.above) " higher →" } }
                @if let Some(x) = mx { line.mark x1=(x) x2=(x) y1="0" y2=(height + 10.0) {} }
            }
            p.dim {
                (display::number(s.n as f64, None)) " things, the middle " (fmt(s.middle)) "."
                @if let (Some((_, v)), Some(b)) = (mark, below_pct) { " This one, " (fmt(v)) ", is above " (format!("{b:.0}")) " % of them." }
                (left_out(&numbers))
            }
        }
    }
}

fn matrix(ctx: &Ctx, m: &MatrixSpec) -> Markup {
    let rows = ctx.words(&m.rows);
    let numeric = !m.bands.is_empty();
    let col_numbers = if numeric { ctx.numbers(&m.columns).by } else { BTreeMap::new() };
    let col_words = if numeric { BTreeMap::new() } else { ctx.words(&m.columns) };
    let band_name = |i: usize| -> String {
        match i {
            0 => format!("< {}", ctx.fmt(&m.columns, m.bands[0])),
            i if i == m.bands.len() => format!("≥ {}", ctx.fmt(&m.columns, m.bands[i - 1])),
            i => format!("{}–{}", ctx.fmt(&m.columns, m.bands[i - 1]), ctx.fmt(&m.columns, m.bands[i])),
        }
    };
    let mut cells: BTreeMap<(String, String), u64> = BTreeMap::new();
    let mut columns: Vec<String> = Vec::new();
    for (k, rw) in &rows {
        let cols: Vec<String> = if numeric {
            col_numbers.get(k).map(|v| vec![band_name(m.bands.iter().filter(|e| middle(v) >= **e).count())]).unwrap_or_default()
        } else {
            col_words.get(k).cloned().unwrap_or_default()
        };
        for r in rw {
            for c in &cols {
                *cells.entry((r.clone(), c.clone())).or_default() += 1;
                if !columns.contains(c) {
                    columns.push(c.clone());
                }
            }
        }
    }
    // Words in the order the tracker's scale gives them, best first; by how many otherwise.
    let order = |p: &str, names: &mut Vec<String>, total: &dyn Fn(&str) -> u64| {
        match ctx.decl.normalise_for(p).filter(|a| !a.scale.is_empty()) {
            Some(a) => names.sort_by_key(|n| a.position(n).unwrap_or(usize::MAX)),
            None => names.sort_by_key(|n| std::cmp::Reverse(total(n))),
        }
    };
    if numeric {
        columns = (0..=m.bands.len()).map(band_name).filter(|c| cells.keys().any(|(_, x)| x == c)).collect();
    } else {
        let col_total = |c: &str| cells.iter().filter(|((_, x), _)| x == c).map(|(_, n)| *n).sum::<u64>();
        order(&m.columns, &mut columns, &col_total);
    }
    let mut row_names: Vec<String> = cells.keys().map(|(r, _)| r.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    let row_total = |r: &str| cells.iter().filter(|((x, _), _)| x == r).map(|(_, n)| *n).sum::<u64>();
    order(&m.rows, &mut row_names, &row_total);
    // The rows and columns with most things, in the order they had.
    let keep = |names: &mut Vec<String>, total: &dyn Fn(&str) -> u64| {
        if names.len() > m.limit {
            let mut by: Vec<(u64, String)> = names.iter().map(|n| (total(n), n.clone())).collect();
            by.sort_by(|a, b| b.0.cmp(&a.0));
            let top: BTreeSet<String> = by.into_iter().take(m.limit).map(|x| x.1).collect();
            names.retain(|n| top.contains(n));
        }
    };
    let (rows_all, cols_all) = (row_names.len(), columns.len());
    keep(&mut row_names, &row_total);
    let col_total = |c: &str| cells.iter().filter(|((_, x), _)| x == c).map(|(_, n)| *n).sum::<u64>();
    keep(&mut columns, &col_total);
    let all: Vec<f64> = cells.values().map(|n| (*n as f64).ln_1p()).collect();
    let scale = Scale::of(&all, None, false);
    html! {
        section.chart {
            h2 { (if m.title.is_empty() { format!("{} against {}", ctx.label(&m.rows), ctx.label(&m.columns).to_lowercase()) } else { m.title.clone() }) }
            div.scroll {
                table.matrix {
                    thead { tr { th {} @for c in &columns { th.num { (c) } } th.num { "All" } } }
                    tbody {
                        @for r in &row_names {
                            tr {
                                th { (display::shown(ctx.decl, &m.rows, r)) }
                                @for c in &columns {
                                    @let n = cells.get(&(r.clone(), c.clone())).copied().unwrap_or(0);
                                    @let colour = scale.colour((n as f64).ln_1p());
                                    @let dark = scale.class((n as f64).ln_1p()) >= 4;
                                    td.num.dark[dark] style={"background:" @if n > 0 { (colour) } @else { "transparent" }} { @if n > 0 { (display::number(n as f64, None)) } }
                                }
                                td.num.dim { (display::number(row_total(r) as f64, None)) }
                            }
                        }
                    }
                }
            }
            @if rows_all > row_names.len() || cols_all > columns.len() {
                p.dim { "The " (row_names.len()) " of " (rows_all) " " (ctx.label(&m.rows).to_lowercase()) " and " (columns.len()) " of " (cols_all) " " (ctx.label(&m.columns).to_lowercase()) " with most things." }
            }
        }
    }
}

/// The most or least of a value, ties ordered by what the ranking names next, each value to as
/// many places as tell it from the next.
fn ranking(ctx: &Ctx, r: &RankingSpec) -> Markup {
    let numbers = ctx.numbers(&r.value);
    let then: Vec<BTreeMap<String, f64>> = r.then.iter().map(|p| ctx.numbers(p).by.into_iter().map(|(k, v)| (k, middle(&v))).collect()).collect();
    // Which end a ranking shows is the ranking's: the cheapest diesel, the CVEs most likely to be
    // exploited, whichever way the value is better.
    let lower = r.cheap;
    let mut all: Vec<(String, f64)> = numbers.by.iter().map(|(k, v)| (k.clone(), middle(v))).collect();
    all.sort_by(|a, b| {
        let first = if lower { cmp(&a.1, &b.1) } else { cmp(&b.1, &a.1) };
        then.iter().fold(first, |o, t| o.then_with(|| cmp(t.get(&b.0).unwrap_or(&f64::NEG_INFINITY), t.get(&a.0).unwrap_or(&f64::NEG_INFINITY))))
    });
    all.truncate(r.limit);
    // Places enough that two shown values that differ read differently.
    let d = ctx.value(&r.value).cloned().unwrap_or_default();
    let base = d.decimals.unwrap_or(2);
    let places = (base..=base + 4)
        .find(|p| all.windows(2).all(|w| (w[0].1 - w[1].1).abs() < 1e-12 || format!("{:.*}", p, w[0].1) != format!("{:.*}", p, w[1].1)))
        .unwrap_or(base + 4);
    let shown = ValueDecl { decimals: Some(places), ..d };
    let (lo, hi) = (all.iter().map(|x| x.1).fold(f64::INFINITY, f64::min), all.iter().map(|x| x.1).fold(f64::NEG_INFINITY, f64::max));
    let bar = |v: f64| if hi > lo { 15.0 + (v - lo) / (hi - lo) * 85.0 } else { 100.0 };
    let bar = |v: f64| if lower { 115.0 - bar(v) } else { bar(v) };
    html! {
        section.chart {
            h2 { (if r.title.is_empty() { format!("{} {}", if lower { "Least" } else { "Most" }, ctx.label(&r.value).to_lowercase()) } else { r.title.clone() }) }
            table.ranking {
                tbody {
                    @for (i, (key, v)) in all.iter().enumerate() {
                        @let id = key.split_once(':').map(|k| k.1).unwrap_or(key);
                        @let title = ctx.title(key);
                        tr {
                            td.num.dim { (i + 1) }
                            td { a href=(ctx.href(key)) { (clip(&title, 90)) }
                                @if !title.to_lowercase().contains(&id.to_lowercase()) { div.why.mono { (id) } }
                                @if !r.then.is_empty() { div.why { @for (p, t) in r.then.iter().zip(&then) { @if let Some(x) = t.get(key) { (ctx.label(p)) " " (ctx.fmt(p, *x)) " " } } } } }
                            td.barcell { span.bar style={"width:" (format!("{:.1}", bar(*v).clamp(2.0, 100.0))) "%"} {} }
                            td.num { (display::number(*v, Some(&shown))) }
                        }
                    }
                }
            }
            p.dim { (display::number(numbers.by.len() as f64, None)) " things have it." (left_out(&numbers)) }
        }
    }
}

fn arrivals(ctx: &Ctx, a: &ArrivalsSpec) -> Markup {
    let dated: Vec<(String, String)> = match (a.date.as_str(), &a.source) {
        ("known", Some(s)) => ctx.store.first_said_by(s),
        _ => ctx.words(&a.date).into_iter().filter_map(|(k, w)| Some((k, w.into_iter().min()?))).collect(),
    };
    let mut per_month: BTreeMap<String, u64> = BTreeMap::new();
    for (_, d) in &dated {
        if let Some(m) = d.get(..7) {
            *per_month.entry(m.to_string()).or_default() += 1;
        }
    }
    let gone = if a.withdrawn { ctx.store.withdrawn_by_month() } else { BTreeMap::new() };
    let two = !gone.is_empty();
    let now_month = crate::iso_date(crate::now())[..7].to_string();
    let mut months: Vec<String> = per_month.keys().chain(gone.keys()).filter(|m| m.as_str() <= now_month.as_str()).cloned().collect::<BTreeSet<_>>().into_iter().collect();
    months = months.split_off(months.len().saturating_sub(a.months));
    if months.is_empty() {
        return html! {};
    }
    let most = months.iter().map(|m| per_month.get(m).copied().unwrap_or(0).max(gone.get(m).copied().unwrap_or(0))).max().unwrap_or(1).max(1) as f64;
    let (left, width) = (40.0, 830.0);
    let up = if two { 110.0 } else { 150.0 };
    let w = width / months.len() as f64;
    let total = dated.len();
    let mut recent: Vec<&(String, String)> = dated.iter().filter(|(_, d)| d.as_str() <= now_month.as_str() || d.get(..7).is_some_and(|m| m <= now_month.as_str())).collect();
    recent.sort_by(|x, y| y.1.cmp(&x.1));
    recent.truncate(a.recent);
    let label = if a.date == "known" { a.source.as_deref().map(|s| ctx.source_title(s)).unwrap_or_default() } else { ctx.label(&a.date) };
    html! {
        section.chart {
            h2 { (if a.title.is_empty() { format!("By month of {}", label.to_lowercase()) } else { a.title.clone() }) }
            svg.histogram viewBox={"0 0 900 " (if two { 270 } else { 190 })} {
                @for (i, m) in months.iter().enumerate() {
                    @let n = per_month.get(m).copied().unwrap_or(0);
                    @let h = n as f64 / most * up;
                    rect.bin x=(format!("{:.1}", left + i as f64 * w)) y=(format!("{:.1}", 10.0 + up - h)) width=(format!("{:.1}", (w - 2.0).max(0.5))) height=(format!("{h:.1}")) { title { (m) ": " (n) " new" } }
                    @if two {
                        @let g = gone.get(m).copied().unwrap_or(0);
                        @let hg = g as f64 / most * up;
                        rect.bin.out x=(format!("{:.1}", left + i as f64 * w)) y=(format!("{:.1}", 10.0 + up + 2.0)) width=(format!("{:.1}", (w - 2.0).max(0.5))) height=(format!("{hg:.1}")) { title { (m) ": " (g) " gone" } }
                    }
                    @if i % (months.len() / 8).max(1) == 0 { text.tick x=(format!("{:.1}", left + i as f64 * w + w / 2.0)) y=(if two { "262" } else { "180" }) text-anchor="middle" { (m) } }
                }
                @if two { line.zero x1=(left) x2=(left + width) y1=(10.0 + up + 1.0) y2=(10.0 + up + 1.0) {} }
            }
            p.dim { (display::number(total as f64, None)) " things with a date." @if two { " Below the line, the things no source speaks of any longer, by the month they went — since the tracker began to watch." } }
            @if !recent.is_empty() {
                h3 { "Newest" }
                table.ranking { tbody {
                    @for (key, d) in &recent {
                        tr { td.when.dim { (d.get(..10).unwrap_or(d)) } td { a href=(ctx.href(key)) { (clip(&ctx.title(key), 100)) } } }
                    }
                } }
            }
        }
    }
}

/// The things that are one entity at one time, as rows: the middle of what the sources say of
/// each value, its change on the time before, and whether the sources differ.
struct Grid {
    /// Entity, time, value: the middle and every source's.
    at: BTreeMap<(String, String), BTreeMap<String, (f64, Vec<(String, f64)>, String)>>,
    times: Vec<String>,
}

fn grid(ctx: &Ctx, entity: &str, time: &str, values: &[String]) -> Grid {
    let entity_of = ctx.entities_of(entity);
    let (time_of, times) = ctx.times_of(time);
    let mut at: BTreeMap<(String, String), BTreeMap<String, (f64, Vec<(String, f64)>, String)>> = BTreeMap::new();
    for v in values {
        for (k, said) in ctx.numbers(v).by {
            if let (Some(e), Some(t)) = (entity_of.get(&k), time_of.get(&k)) {
                at.entry((e.clone(), t.clone())).or_default().insert(v.clone(), (middle(&said), said, k.clone()));
            }
        }
    }
    Grid { at, times }
}

impl Grid {
    fn get(&self, e: &str, t: &str, v: &str) -> Option<&(f64, Vec<(String, f64)>, String)> {
        self.at.get(&(e.to_string(), t.to_string()))?.get(v)
    }
    /// The time a year before: four quarters back, twelve months, or the year before.
    fn year_before(t: &str) -> Option<String> {
        let y: i64 = t.get(..4)?.parse().ok()?;
        Some(format!("{}{}", y - 1, &t[4..]))
    }
    fn before(&self, t: &str) -> Option<String> {
        let i = self.times.iter().position(|x| x == t)?;
        self.times.get(i.checked_sub(1)?).cloned()
    }
}

/// Whether the sources of a value differ by more than the tracker lets them.
fn differ(ctx: &Ctx, property: &str, said: &[(String, f64)]) -> bool {
    let tol = ctx.decl.normalise_for(property).map(|a| a.number_tolerance()).unwrap_or(0.0);
    let lo = said.iter().map(|s| s.1).fold(f64::INFINITY, f64::min);
    let hi = said.iter().map(|s| s.1).fold(f64::NEG_INFINITY, f64::max);
    said.len() > 1 && hi - lo > tol + 1e-9
}

fn table(ctx: &Ctx, t: &TableSpec, params: &BTreeMap<String, String>, here: &str) -> Markup {
    let time_prop = t.time.clone().unwrap_or_default();
    let g = grid(ctx, &t.entity, &time_prop, &t.value);
    let mut per_time: BTreeMap<String, usize> = BTreeMap::new();
    for ((_, time), _) in &g.at {
        *per_time.entry(time.clone()).or_default() += 1;
    }
    let time = params.get("at").filter(|x| g.times.contains(x)).cloned().or_else(|| ctx.default_time(&per_time)).unwrap_or_default();
    let quarterly = time.contains("-Q") || time.len() == 7;
    let entities: BTreeSet<String> = g.at.keys().filter(|(_, x)| *x == time).map(|(e, _)| e.clone()).collect();
    let sort = params.get("sort").filter(|s| t.value.iter().any(|v| s.trim_start_matches('-') == v)).cloned().unwrap_or_else(|| t.value.first().cloned().unwrap_or_default());
    let (sort_value, ascending) = (sort.trim_start_matches('-').to_string(), sort.starts_with('-'));
    let mut rows: Vec<String> = entities.into_iter().collect();
    rows.sort_by(|a, b| {
        let (x, y) = (g.get(a, &time, &sort_value).map(|v| v.0), g.get(b, &time, &sort_value).map(|v| v.0));
        match (x, y) {
            (Some(x), Some(y)) => if ascending { cmp(&x, &y) } else { cmp(&y, &x) },
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            _ => Ordering::Equal,
        }
    });
    let ranked = rows.len();
    rows.truncate(t.limit);
    let before = g.before(&time);
    let year_before = Grid::year_before(&time);
    let scales: BTreeMap<String, Scale> = t
        .value
        .iter()
        .map(|v| {
            let all: Vec<f64> = g.at.iter().filter(|((_, x), _)| *x == time).filter_map(|(_, m)| m.get(v).map(|x| x.0)).collect();
            (v.clone(), scale_for(ctx, v, &all, false))
        })
        .collect();
    // How many rose: the share of entities whose first value is up on the year before.
    let first = t.value.first().cloned().unwrap_or_default();
    let compared: Vec<f64> = g
        .at
        .keys()
        .filter(|(_, x)| *x == time)
        .filter_map(|(e, _)| Some(g.get(e, &time, &first)?.0 - g.get(e, year_before.as_deref()?, &first)?.0))
        .collect();
    let rising = compared.iter().filter(|d| **d > 0.0).count();
    let position = g.times.iter().position(|x| *x == time);
    html! {
        section.chart {
            h2 { (if t.title.is_empty() { format!("Every {} at once", ctx.decl.entity.as_ref().map(|e| e.called.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| ctx.label(&t.entity).to_lowercase())) } else { t.title.clone() }) @if !time.is_empty() { " · " (ctx.time_word(&time_prop, &time)) } }
            @if let Some(p) = position {
                div.bar {
                    @if p > 0 { a.chip rel="nofollow" href=(with_param(here, "at", &g.times[p - 1])) { "← " (ctx.time_word(&time_prop, &g.times[p - 1])) } }
                    span.chip.on { (ctx.time_word(&time_prop, &time)) }
                    @if p + 1 < g.times.len() { a.chip rel="nofollow" href=(with_param(here, "at", &g.times[p + 1])) { (ctx.time_word(&time_prop, &g.times[p + 1])) " →" } }
                }
            }
            @if !compared.is_empty() {
                p.lede-small { strong { (rising) " of " (compared.len()) } " up on a year before (" (format!("{:.0}", rising as f64 / compared.len() as f64 * 100.0)) " %), in " (ctx.label(&first).to_lowercase()) "." }
            }
            div.scroll {
                table.entities {
                    thead { tr {
                        th.num { "#" }
                        th { (ctx.label(&t.entity)) }
                        @for v in &t.value {
                            @let next = if sort_value == *v && !ascending { format!("-{v}") } else { v.clone() };
                            th.num { a rel="nofollow" href=(with_param(here, "sort", &next)) { (ctx.label(v)) @if sort_value == *v { (if ascending { " ↑" } else { " ↓" }) } } }
                        }
                    } }
                    tbody {
                        @for (i, e) in rows.iter().enumerate() {
                            tr {
                                td.num.dim { (i + 1) }
                                td { @match ctx.entity_href(e) { Some(h) => a href=(h) { (ctx.entity_name(&t.entity, e)) }, None => (ctx.entity_name(&t.entity, e)) } }
                                @for v in &t.value {
                                    @match g.get(e, &time, v) {
                                        Some((x, said, key)) => {
                                            td.num.cell {
                                                span.dot style={"background:" (scales[v].colour(*x))} {}
                                                a href=(ctx.href(key)) { (ctx.fmt(v, *x)) }
                                                @if differ(ctx, v, said) { span.chip.on.small title=(said.iter().map(|(s, n)| format!("{} {}", ctx.source_title(s), ctx.fmt(v, *n))).collect::<Vec<_>>().join(" · ")) { "sources differ" } }
                                                div.why {
                                                    @if let Some(p) = before.as_deref().and_then(|b| g.get(e, b, v)) { (display::change(x - p.0, ctx.value(v))) " on " (ctx.time_word(&time_prop, before.as_deref().unwrap_or(""))) }
                                                    @if quarterly { @if let Some(p) = year_before.as_deref().and_then(|b| g.get(e, b, v)) { " · " (display::change(x - p.0, ctx.value(v))) " on a year" } }
                                                }
                                            }
                                        }
                                        None => { td.num.dim { "—" } }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            p.dim { (ranked) " with a figure for " (ctx.time_word(&time_prop, &time)) @if ranked > rows.len() { ", the first " (rows.len()) " shown" } ". The middle of what the sources say; a heading orders by it." }
        }
    }
}

/// What every thing's values average now, how that moved over a week and a month, and per group.
fn averages(ctx: &Ctx, a: &AveragesSpec, params: &BTreeMap<String, String>, here: &str) -> Markup {
    let _ = (params, here);
    let now = crate::now();
    let since = ctx.store.signals_since();
    let mean = |v: &[f64]| if v.is_empty() { None } else { Some(v.iter().sum::<f64>() / v.len() as f64) };
    let groups = a.by.as_ref().map(|b| ctx.words(b)).unwrap_or_default();
    struct Tile {
        value: String,
        now: Option<f64>,
        n: usize,
        moved: Vec<(String, f64)>,
        groups: BTreeMap<String, Vec<f64>>,
    }
    let tiles: Vec<Tile> = a
        .value
        .iter()
        .map(|v| {
            let numbers = ctx.numbers(v);
            let current: BTreeMap<String, f64> = numbers.by.iter().map(|(k, s)| (k.clone(), middle(s))).collect();
            let d = ctx.value(v);
            // What each thing said at a moment past, from the changes the store holds since.
            let then = |back: i64| -> Option<(String, f64)> {
                let t = crate::iso_date(now - back);
                let held_from = since.clone()?;
                let at = if held_from.as_str() > t.as_str() { held_from.clone() } else { format!("{t}T00:00:00Z") };
                let was = ctx.store.was_at(v, &at);
                let mut vals: Vec<f64> = Vec::new();
                for (k, x) in &current {
                    match was.get(k) {
                        Some(Some(w)) => {
                            if let Some(p) = w.iter().find_map(|s| s.parse::<f64>().ok()).filter(|p| d.is_none_or(|d| d.plausible(*p))) {
                                vals.push(p)
                            }
                        }
                        Some(None) => {}
                        None => vals.push(*x),
                    }
                }
                let label = if held_from.as_str() > t.as_str() { format!("since {}", &held_from[..10]) } else { format!("on {}", crate::web::duration(back as f64)) };
                Some((label, mean(&vals)?))
            };
            let now_mean = mean(&current.values().copied().collect::<Vec<_>>());
            let mut moved: Vec<(String, f64)> = Vec::new();
            for back in [7 * 86400, 30 * 86400] {
                if let (Some(m), Some((label, p))) = (now_mean, then(back)) {
                    if !moved.iter().any(|(l, _)| *l == label) {
                        moved.push((label, m - p));
                    }
                }
            }
            let mut by_group: BTreeMap<String, Vec<f64>> = BTreeMap::new();
            for (k, x) in &current {
                if let Some(gw) = groups.get(k).and_then(|w| w.first()) {
                    by_group.entry(gw.clone()).or_default().push(*x);
                }
            }
            Tile { value: v.clone(), now: now_mean, n: current.len(), moved, groups: by_group }
        })
        .collect();
    let group_names: BTreeSet<String> = tiles.iter().flat_map(|t| t.groups.keys().cloned()).collect();
    let first = tiles.first();
    let mut ordered: Vec<String> = group_names.into_iter().collect();
    if let Some(f) = first {
        ordered.sort_by(|x, y| cmp(&f.groups.get(x).and_then(|v| mean(v)).unwrap_or(f64::INFINITY), &f.groups.get(y).and_then(|v| mean(v)).unwrap_or(f64::INFINITY)));
    }
    let by = a.by.clone().unwrap_or_default();
    html! {
        section.chart {
            h2 { (if a.title.is_empty() { "On average".to_string() } else { a.title.clone() }) }
            div.tiles {
                @for t in &tiles {
                    div.tile {
                        span.dim { (ctx.label(&t.value)) }
                        b { @match t.now { Some(m) => (ctx.fmt(&t.value, m)), None => "—" } }
                        @for (label, d) in &t.moved {
                            span.change.(tone(ctx, &t.value, *d)) { (display::change(*d, ctx.value(&t.value))) " " span.dim { (label) } }
                        }
                        span.dim.small { (display::number(t.n as f64, None)) " current" }
                    }
                }
            }
            @if !ordered.is_empty() {
                div.scroll {
                    table.entities {
                        thead { tr { th { (ctx.label(&by)) } @for t in &tiles { th.num { (ctx.label(&t.value)) } } th.num { "Places" } } }
                        tbody {
                            @for gname in &ordered {
                                tr {
                                    td { a rel="nofollow" href=(at(&format!("/?q={}", urlencode(&format!("{by}={gname}"))))) { (gname) } }
                                    @for t in &tiles {
                                        @let all: Vec<f64> = t.groups.values().filter_map(|v| mean(v)).collect();
                                        @let scale = scale_for(ctx, &t.value, &all, false);
                                        @match t.groups.get(gname).and_then(|v| mean(v)) {
                                            Some(m) => { td.num.cell { span.dot style={"background:" (scale.colour(m))} {} (ctx.fmt(&t.value, m)) } }
                                            None => { td.num.dim { "—" } }
                                        }
                                    }
                                    td.num.dim { (first.and_then(|f| f.groups.get(gname)).map(|v| v.len()).unwrap_or(0)) }
                                }
                            }
                        }
                    }
                }
            }
            p.dim { "Averages over the values that are current and in the range a real one falls in." }
        }
    }
}

/// Which sources speak of the same things: each set of them with how many things it has in
/// common, and how many only one source knows.
fn overlap(ctx: &Ctx, o: &OverlapSpec) -> Markup {
    let sets = ctx.store.source_sets();
    let total: u64 = sets.iter().map(|s| s.1).sum();
    let most = sets.iter().map(|s| s.1).max().unwrap_or(1).max(1) as f64;
    let short = |s: &str| s.rsplit('/').next().unwrap_or(s).to_string();
    let rows: Vec<&(Vec<String>, u64)> = sets.iter().take(o.limit).collect();
    let several: u64 = sets.iter().filter(|s| s.0.len() > 1).map(|s| s.1).sum();
    html! {
        section.chart {
            h2 { (if o.title.is_empty() { "Which sources know which things".to_string() } else { o.title.clone() }) }
            p.lede-small { strong { (display::number(several as f64, None)) } " of " (display::number(total as f64, None)) " things are named by two sources or more." }
            table.ranking {
                tbody {
                    @for (set, n) in &rows {
                        @let q = if set.len() == 1 { format!("only:{}", short(&set[0])) } else { set.iter().map(|s| format!("has:{}", short(s))).collect::<Vec<_>>().join(" and ") };
                        tr {
                            td { a rel="nofollow" href=(at(&format!("/?q={}", urlencode(&q)))) {
                                @if set.len() == 1 { "Only " (ctx.source_title(&set[0])) } @else { (set.iter().map(|s| ctx.source_title(s)).collect::<Vec<_>>().join(" + ")) }
                            } }
                            td.barcell { span.bar.(if set.len() > 1 { "joined" } else { "" }) style={"width:" (format!("{:.1}", (*n as f64 / most * 100.0).max(1.0))) "%"} {} }
                            td.num { (display::number(*n as f64, None)) }
                        }
                    }
                }
            }
        }
    }
}

// -- the pictures, on one thing's page ------------------------------------------------------------

/// What `thing.show` declares, drawn for one thing.
pub fn thing(ctx: &Ctx, show: &[Show], key: &str, params: &BTreeMap<String, String>, here: &str) -> Markup {
    html! {
        @for (i, s) in show.iter().enumerate() {
            (thing_one(ctx, s, key, params, here, i))
        }
        (PreEscaped(SCRIPT))
    }
}

fn thing_one(ctx: &Ctx, show: &Show, key: &str, params: &BTreeMap<String, String>, here: &str, i: usize) -> Markup {
    match show {
        Show::Series(s) => {
            let entity = ctx.own_words(key, &s.entity).first().cloned().unwrap_or_default();
            let mark = ctx.own_words(key, &s.time).first().cloned();
            if entity.is_empty() {
                html! {}
            } else {
                series(ctx, s, &entity, mark.as_deref(), params, here, false)
            }
        }
        Show::Spread(s) => {
            let rows: Vec<(String, Vec<(String, f64)>)> = s
                .value
                .iter()
                .filter_map(|v| Some((v.clone(), Some(ctx.own_numbers(key, v)).filter(|n| !n.is_empty())?)))
                .filter(|(_, said)| said.len() >= 2)
                .collect();
            if rows.is_empty() {
                return html! {};
            }
            let sources: BTreeSet<String> = rows.iter().flat_map(|r| r.1.iter().map(|x| x.0.clone())).collect();
            html! {
                section.chart {
                    h2 { "What each source says" }
                    (source_key(ctx, &sources))
                    @for (name, said) in &rows {
                        h3.dim { (ctx.label(name)) }
                        (spread_rows(ctx, name, &[(key.to_string(), said.clone())], &sources))
                    }
                }
            }
        }
        Show::Distribution(d) => html! {
            @for v in &d.value {
                @if let Some(mine) = Some(ctx.own_numbers(key, v)).filter(|n| !n.is_empty()).map(|s| middle(&s)) {
                    (distribution(ctx, d, Some((v, mine)), params, here))
                }
            }
        },
        // A map of one thing, where it is among its neighbours, the nearest listed with theirs.
        Show::Map(m) => {
            let places = ctx.words(&m.place);
            let Some((lat, lon)) = places.get(key).and_then(|w| w.iter().find_map(|s| crate::geo::place_of(s))) else { return html! {} };
            let chosen = params.get("colour").filter(|c| m.colour.contains(c)).cloned().or_else(|| m.colour.first().cloned()).unwrap_or_default();
            let values = ctx.numbers(&chosen).by;
            let near: Vec<(String, f64, f64, Option<f64>)> = places
                .iter()
                .filter_map(|(k, w)| {
                    let (a, b) = w.iter().find_map(|s| crate::geo::place_of(s))?;
                    ((a - lat).abs() < 0.15 && (b - lon).abs() < 0.22).then(|| (k.clone(), a, b, values.get(k).map(|v| middle(v))))
                })
                .collect();
            let all: Vec<f64> = near.iter().filter_map(|n| n.3).collect();
            let scale = scale_for(ctx, &chosen, &all, m.cheap);
            let frame = crate::geo::Frame::fit((lon - 0.2, lat - 0.13, lon + 0.2, lat + 0.13), 900.0);
            let km = |a: f64, b: f64| ((a - lat).powi(2) + ((b - lon) * lat.to_radians().cos()).powi(2)).sqrt() * 111.2;
            let mut nearest: Vec<&(String, f64, f64, Option<f64>)> = near.iter().filter(|n| n.0 != key && n.3.is_some()).collect();
            nearest.sort_by(|x, y| cmp(&km(x.1, x.2), &km(y.1, y.2)));
            nearest.truncate(6);
            let mine = values.get(key).map(|v| middle(v));
            let id = format!("near-{i}");
            html! {
                section.chart {
                    h2 { "Nearby · " (ctx.label(&chosen)) }
                    (choices(ctx, here, "colour", &m.colour, &chosen))
                    div.map {
                        svg.near id=(id) viewBox={"0 0 " (frame.width as i64) " " (frame.height as i64)} {
                            @for c in crate::geo::countries().iter().filter(|c| frame.shows(c)) { path.land d=(frame.path(c)) {} }
                            @for (k, a, b, v) in &near {
                                @let (x, y) = frame.point(*b, *a);
                                a href=(ctx.href(k)) {
                                    circle class=(match v { Some(v) => format!("dot k k{}", scale.class(*v)), None => "dot k none".into() }) cx=(format!("{x:.1}")) cy=(format!("{y:.1}")) r=(if k == key { "8" } else { "5" }) fill=(v.map(|v| scale.colour(v)).unwrap_or("#bbb")) stroke=(if k == key { "#111" } else { "none" }) stroke-width="2" {
                                        title { (ctx.title(k)) " · " (v.map(|v| ctx.fmt(&chosen, v)).unwrap_or_else(|| "—".into())) }
                                    }
                                }
                            }
                        }
                    }
                    (legend(ctx, &chosen, &scale, &[], &id))
                    @if !nearest.is_empty() {
                        table.ranking { tbody {
                            @for (k, a, b, v) in &nearest {
                                tr {
                                    td { a href=(ctx.href(k)) { (ctx.title(k)) } div.why { (format!("{:.1}", km(*a, *b))) " km" } }
                                    td.num { (v.map(|v| ctx.fmt(&chosen, v)).unwrap_or_default())
                                        @if let (Some(v), Some(m)) = (v, mine) { div.why { (display::change(*v - m, ctx.value(&chosen))) " on this one" } } }
                                }
                            }
                        } }
                    }
                }
            }
        }
        Show::History(h) => history(ctx, h, params, here),
        Show::Memory(m) => memory(ctx, m, key, params, here),
        _ => html! {},
    }
}

/// The usual quantisations, the bits each keeps of a weight, and what that is said to cost in
/// quality: llama.cpp's own figures for its formats.
const QUANTS: [(&str, f64, &str); 8] = [
    ("Q2_K", 2.63, "much lower quality"),
    ("Q3_K_M", 3.91, "lower quality"),
    ("Q4_K_M", 4.85, "the usual choice"),
    ("Q5_K_M", 5.69, "close to the original"),
    ("Q6_K", 6.59, "nearly the original"),
    ("Q8_0", 8.50, "the original, near enough"),
    ("F16", 16.0, "the original"),
    ("F32", 32.0, "full precision"),
];

/// What a model takes to run at each usual quantisation: its file, and the memory it needs with
/// a context of 8,000 tokens beside it, from its parameter count alone. An estimate, and said so.
fn memory(ctx: &Ctx, m: &MemorySpec, key: &str, params: &BTreeMap<String, String>, here: &str) -> Markup {
    let Some(p) = Some(ctx.own_numbers(key, &m.parameters)).filter(|n| !n.is_empty()).map(|s| middle(&s)).filter(|p| *p > 0.0) else { return html! {} };
    let sizes = ["8", "12", "16", "24", "32", "64"].map(String::from);
    let fits: Option<f64> = params.get("fits").filter(|f| sizes.contains(f)).and_then(|f| f.parse().ok());
    // The weights, and for the rest about a tenth more and a gigabyte: the context and the runtime.
    let need = |bpw: f64| p * bpw / 8.0 * 1.1 / 1e9 + 1.0;
    html! {
        section.chart {
            h2 { (if m.title.is_empty() { "What it takes to run".to_string() } else { m.title.clone() }) }
            div.bar {
                span.dim { "Fits in " }
                @for s in &sizes { a.chip.on[fits == s.parse().ok()] rel="nofollow" href=(with_param(here, "fits", s)) { (s) " GB" } }
            }
            table.ranking {
                thead { tr { th { "Quantisation" } th.num { "File" } th.num { "Memory" } th { "" } } }
                tbody {
                    @for (name, bpw, says) in QUANTS {
                        @let file = p * bpw / 8.0 / 1e9;
                        @let n = need(bpw);
                        tr.(match fits { Some(f) if n <= f * 0.9 => "fits", Some(f) if n <= f => "tight", Some(_) => "no", None => "" }) {
                            td { strong { (name) } div.why { (says) " · " (format!("{bpw:.1}")) " bits a weight" } }
                            td.num { (format!("{file:.1}")) " GB" }
                            td.num { (format!("{n:.1}")) " GB" }
                            td.dim { @match fits { Some(f) if n <= f * 0.9 => "fits", Some(f) if n <= f => "tight", Some(_) => "too large", None => "" } }
                        }
                    }
                }
            }
            p.dim { "Estimated from " (display::number(p / 1e9, None)) " billion parameters: the file at each format's bits a weight, and memory a tenth more and a gigabyte for a context of some thousands of tokens. A long context takes more." }
        }
    }
}

/// A value as each source said it over time, from the versions of its claims: the last week,
/// month, quarter or all of it, with its lowest and highest.
fn history(ctx: &Ctx, h: &HistorySpec, params: &BTreeMap<String, String>, here: &str) -> Markup {
    let ranges = ["7d", "30d", "90d", "all"].map(String::from);
    let range = params.get("range").filter(|r| ranges.contains(r)).cloned().unwrap_or_else(|| "30d".into());
    let back = crate::fetch::duration(&range);
    let now = crate::now();
    html! {
        @for v in &h.value {
            @let points: Vec<&(String, i64, f64)> = ctx.history.get(v).map(|p| p.iter().filter(|(_, t, _)| back.is_none_or(|b| *t >= now - b)).collect()).unwrap_or_default();
            @if points.len() >= 2 {
                @let sources: BTreeSet<String> = points.iter().map(|p| p.0.clone()).collect();
                @let (t0, t1) = (points.iter().map(|p| p.1).min().unwrap_or(0), now);
                @let lo = points.iter().map(|p| p.2).fold(f64::INFINITY, f64::min);
                @let hi = points.iter().map(|p| p.2).fold(f64::NEG_INFINITY, f64::max);
                @let (lo_y, hi_y) = if (hi - lo).abs() < 1e-9 { (lo - 1.0, hi + 1.0) } else { (lo - (hi - lo) * 0.1, hi + (hi - lo) * 0.1) };
                @let (left, top, width, height) = (60.0, 10.0, 810.0, 180.0);
                @let x = |t: i64| left + (t - t0) as f64 / (t1 - t0).max(1) as f64 * width;
                @let y = |v: f64| top + (hi_y - v) / (hi_y - lo_y) * height;
                @let lowest = points.iter().min_by(|a, b| cmp(&a.2, &b.2));
                @let highest = points.iter().max_by(|a, b| cmp(&a.2, &b.2));
                section.chart {
                    h2 { (if h.title.is_empty() { format!("{} over time", ctx.label(v)) } else { h.title.clone() }) }
                    div.bar { @for r in &ranges { a.chip.on[*r == range] rel="nofollow" href=(with_param(here, "range", r)) { (if r == "all" { "All".to_string() } else { r.clone() }) } } }
                    @if sources.len() > 1 { (source_key(ctx, &sources)) }
                    svg.series viewBox="0 0 900 220" {
                        @for t in ticks(lo_y, hi_y, 4) {
                            line.grid x1=(left) x2=(left + width) y1=(y(t)) y2=(y(t)) {}
                            text.tick x=(left - 6.0) y=(y(t) + 4.0) text-anchor="end" { (ctx.fmt(v, t)) }
                        }
                        text.tick x=(left) y="212" text-anchor="start" { (crate::iso_date(t0)) }
                        text.tick x=(left + width) y="212" text-anchor="end" { "now" }
                        @for s in &sources {
                            @let mine: Vec<&&(String, i64, f64)> = points.iter().filter(|p| p.0 == *s).collect();
                            // A price holds until it changes: steps, and the last one to now.
                            @let mut path: Vec<String> = Vec::new();
                            @let _ = { for (j, p) in mine.iter().enumerate() {
                                if j > 0 { path.push(format!("{:.1},{:.1}", x(p.1), y(mine[j - 1].2))); }
                                path.push(format!("{:.1},{:.1}", x(p.1), y(p.2)));
                            } if let Some(last) = mine.last() { path.push(format!("{:.1},{:.1}", x(now), y(last.2))); } };
                            polyline.line points=(path.join(" ")) stroke=(source_colour(&sources, s)) {}
                            @for p in &mine { circle cx=(format!("{:.1}", x(p.1))) cy=(format!("{:.1}", y(p.2))) r="2.5" fill=(source_colour(&sources, s)) { title { (crate::iso_date(p.1)) ": " (ctx.fmt(v, p.2)) } } }
                        }
                    }
                    p.dim {
                        (points.len() - sources.len()) " changes."
                        @if let Some(l) = lowest { " Lowest " (ctx.fmt(v, l.2)) " on " (crate::iso_date(l.1)) "." }
                        @if let Some(h) = highest { " Highest " (ctx.fmt(v, h.2)) " on " (crate::iso_date(h.1)) "." }
                    }
                }
            }
        }
    }
}

// -- an entity's own page -------------------------------------------------------------------------

/// The latest measured time an entity has a value for, the value, the one before, and where it
/// ranks among every entity at that time.
struct Headline {
    time: String,
    value: f64,
    said: Vec<(String, f64)>,
    key: String,
    before: Option<(String, f64)>,
    rank: Option<(usize, usize)>,
    line: Vec<(String, f64)>,
}

fn headline(ctx: &Ctx, g: &Grid, entity: &str, v: &str) -> Option<Headline> {
    let now_year = year_of(&crate::iso_date(crate::now())).unwrap_or(9999);
    let until = ctx.actual_until();
    let measured = |t: &str, said: &[(String, f64)]| {
        year_of(t).is_some_and(|y| y <= now_year) && said.iter().any(|(s, _)| until.get(s).is_none_or(|u| year_of(t).is_some_and(|y| y <= *u)))
    };
    let line: Vec<(String, f64)> = g.times.iter().filter_map(|t| g.get(entity, t, v).filter(|x| measured(t, &x.1)).map(|x| (t.clone(), x.0))).collect();
    let (time, _) = line.last()?.clone();
    let (value, said, key) = g.get(entity, &time, v)?.clone();
    let before = line.len().checked_sub(2).and_then(|i| line.get(i)).cloned();
    let lower = ctx.value(v).and_then(|d| d.better.as_deref()) == Some("lower");
    // Where the middle is best, ranked by how near it: inflation nearest 2 % first.
    let centre = ctx.value(v).filter(|d| d.better.as_deref() == Some("middle")).and_then(|d| d.centre);
    let mut others: Vec<f64> = g.at.iter().filter(|((_, t), _)| *t == time).filter_map(|(_, m)| m.get(v).map(|x| x.0)).collect();
    others.sort_by(|a, b| match centre {
        Some(c) => cmp(&(a - c).abs(), &(b - c).abs()),
        None if lower => cmp(a, b),
        None => cmp(b, a),
    });
    let rank = others.iter().position(|x| (x - value).abs() < 1e-12).map(|p| (p + 1, others.len()));
    Some(Headline { time, value, said, key, before, rank, line })
}

fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

fn sparkline(points: &[(String, f64)]) -> Markup {
    if points.len() < 2 {
        return html! {};
    }
    let lo = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let hi = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    let span = (hi - lo).max(1e-9);
    let pts: Vec<String> = points.iter().enumerate().map(|(i, p)| format!("{:.1},{:.1}", i as f64 / (points.len() - 1) as f64 * 200.0, 38.0 - (p.1 - lo) / span * 34.0)).collect();
    html! { svg.spark viewBox="0 0 200 40" preserveAspectRatio="none" { polyline points=(pts.join(" ")) {} } }
}

/// An entity's page: what it is now on each value, with the time before and its rank, every
/// value over time, and every time as a row.
/// An entity's name and what its page leads with, as words: each value's latest, its time and who
/// says it. What a picture of the page shows where a link to it is shared.
pub fn entity_said(ctx: &Ctx, code: &str) -> Option<(String, Vec<(String, String, String)>)> {
    let e = ctx.decl.entity.as_ref()?;
    let values: Vec<String> = if e.values.is_empty() { ctx.decl.view.columns.iter().filter(|c| **c != e.property && **c != e.time).cloned().collect() } else { e.values.clone() };
    let code = ctx.entity_code(&e.property, code);
    let g = grid(ctx, &e.property, &e.time, &values);
    if !g.at.keys().any(|(x, _)| *x == code) {
        return None;
    }
    let rows = values
        .iter()
        .filter_map(|v| {
            let h = headline(ctx, &g, &code, v)?;
            let who = h.said.iter().map(|(s, _)| ctx.source_title(s)).collect::<Vec<_>>().join(", ");
            Some((format!("{} · {}", ctx.label(v), ctx.time_word(&e.time, &h.time)), ctx.fmt(v, h.value), who))
        })
        .collect();
    Some((ctx.entity_name(&e.property, &code), rows))
}

pub fn entity_page(ctx: &Ctx, code: &str, params: &BTreeMap<String, String>, here: &str) -> Option<(String, Markup)> {
    let e = ctx.decl.entity.as_ref()?;
    let values: Vec<String> = if e.values.is_empty() { ctx.decl.view.columns.iter().filter(|c| **c != e.property && **c != e.time).cloned().collect() } else { e.values.clone() };
    let code = ctx.entity_code(&e.property, code);
    let g = grid(ctx, &e.property, &e.time, &values);
    if !g.at.keys().any(|(x, _)| *x == code) {
        return None;
    }
    let name = ctx.entity_name(&e.property, &code);
    let called = if e.called.is_empty() { ctx.label(&e.property).to_lowercase() } else { e.called.clone() };
    let heads: Vec<(String, Option<Headline>)> = values.iter().map(|v| (v.clone(), headline(ctx, &g, &code, v))).collect();
    let times: Vec<String> = g.times.iter().filter(|t| g.at.contains_key(&(code.clone(), (*t).clone()))).cloned().rev().collect();
    // Past the last year any source measured: projections, shown as such.
    let measured = ctx.actual_until().values().copied().max().unwrap_or_else(|| year_of(&crate::iso_date(crate::now())).unwrap_or(9999));
    let spec = SeriesSpec { entity: e.property.clone(), time: e.time.clone(), value: Vec::new(), first: None, title: String::new() };
    let body = html! {
        p.overline { a href=(at("/entity/")) { "Every " (called) } }
        h1 { (name) span.dim.small { " " (code) } }
        div.tiles {
            @for (v, h) in &heads {
                @if let Some(h) = h {
                    a.tile href=(ctx.href(&h.key)) {
                        span.dim { (ctx.label(v)) " · " (ctx.time_word(&e.time, &h.time)) }
                        b { (ctx.fmt(v, h.value)) }
                        @if let Some((bt, bv)) = &h.before {
                            @let d = h.value - bv;
                            span.change.(tone(ctx, v, d)) { (display::change(d, ctx.value(v))) " " span.dim { "on " (ctx.time_word(&e.time, bt)) } }
                        }
                        @if let Some((r, n)) = h.rank.filter(|_| !ctx.value(v).is_some_and(|x| x.peak)) { span.dim.small { (ordinal(r)) " of " (n) " " (crate::display::plural(&called))
                            @match (ctx.value(v).and_then(|x| x.better.as_deref()), ctx.value(v).and_then(|x| x.centre)) {
                                (Some("middle"), Some(c)) => { ", nearest " (ctx.fmt(v, c)) " first" },
                                (Some("lower"), _) => { ", lowest first" },
                                _ => { ", highest first" },
                            } } }
                        // An index against its highest; a ratio against its own average.
                        @if ctx.value(v).is_some_and(|x| x.peak) {
                            @if let Some((pt, pv)) = h.line.iter().max_by(|a, b| cmp(&a.1, &b.1)).filter(|p| p.1 > 0.0) {
                                @if (pv - h.value).abs() > 1e-9 { span.dim.small { (format!("{:.1}", (h.value / pv - 1.0) * 100.0).replace('-', "−")) " % from its highest, " (ctx.fmt(v, *pv)) " in " (ctx.time_word(&e.time, pt)) } }
                                @else { span.dim.small { "its highest yet" } }
                            }
                        }
                        @if ctx.value(v).is_some_and(|x| x.own_average) && h.line.len() >= 8 {
                            @let avg = h.line.iter().map(|p| p.1).sum::<f64>() / h.line.len() as f64;
                            span.change.(if h.value > avg { "bad" } else { "good" }) { (format!("{:+.0}", (h.value / avg - 1.0) * 100.0).replace('-', "−")) " % against its average since " (ctx.time_word(&e.time, &h.line[0].0)) }
                        }
                        @if differ(ctx, v, &h.said) { span.chip.on.small { "sources differ: " (h.said.iter().map(|(s, n)| format!("{} {}", ctx.source_title(s), ctx.fmt(v, *n))).collect::<Vec<_>>().join(" · ")) } }
                        @else { span.dim.small { (h.said.iter().map(|(s, _)| ctx.source_title(s)).collect::<Vec<_>>().join(", ")) } }
                        (sparkline(&h.line))
                    }
                }
            }
        }
        @for v in &values {
            (series(ctx, &SeriesSpec { value: vec![v.clone()], ..spec.clone() }, &code, None, params, here, false))
        }
        h2 { "Every " (ctx.label(&e.time).to_lowercase()) }
        div.scroll {
            table.entities {
                thead { tr { th { (ctx.label(&e.time)) } @for v in &values { th.num { (ctx.label(v)) } } } }
                tbody {
                    @for t in &times {
                        @let ahead = year_of(t).is_some_and(|y| y > measured);
                        tr.(if ahead { "ahead" } else { "" }) {
                            td { (ctx.time_word(&e.time, t)) @if ahead { span.dim.small { " projected" } } }
                            @for v in &values {
                                @match g.get(&code, t, v) {
                                    Some((x, said, key)) => {
                                        td.num {
                                            a href=(ctx.href(key)) { (ctx.fmt(v, *x)) }
                                            @if differ(ctx, v, said) { " " span.chip.on.small title=(said.iter().map(|(s, n)| format!("{} {}", ctx.source_title(s), ctx.fmt(v, *n))).collect::<Vec<_>>().join(" · ")) { "differ" } }
                                        }
                                    }
                                    None => { td.num.dim { "—" } }
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    Some((name, body))
}

/// Every entity, with its latest on each value: the page the entity pages are found from.
pub fn entity_index(ctx: &Ctx, params: &BTreeMap<String, String>, here: &str) -> Option<Markup> {
    let e = ctx.decl.entity.as_ref()?;
    let values: Vec<String> = if e.values.is_empty() { ctx.decl.view.columns.iter().filter(|c| **c != e.property && **c != e.time).cloned().collect() } else { e.values.clone() };
    let called = if e.called.is_empty() { ctx.label(&e.property).to_lowercase() } else { e.called.clone() };
    let spec = TableSpec { entity: e.property.clone(), time: Some(e.time.clone()), value: values, limit: 400, title: format!("Every {called}") };
    Some(html! {
        h1 { "Every " (called) }
        (table(ctx, &spec, params, here))
    })
}

/// The few lines that make a picture zoom and its legend choose, for a page that is one picture.
pub fn script() -> &'static str {
    SCRIPT
}

/// Wheel to zoom and drag to move, for a map whose `viewBox` is the whole of what it shows; the
/// closer, the more names on it. A legend's key shows only its class, and again shows all.
const SCRIPT: &str = r#"<script>
if(!window.zetlynCharts){window.zetlynCharts=1;
document.querySelectorAll('svg.zoomable').forEach(function(s){
  var b=s.viewBox.baseVal, w0=b.width, h0=b.height, drag=null;
  function z(){var r=b.width/w0;s.style.setProperty('--r',r);s.style.setProperty('--rs',Math.sqrt(r));
    s.setAttribute('data-z', r>0.45?1:(r>0.12?2:3));}
  z();
  s.addEventListener('wheel',function(e){e.preventDefault();var r=s.getBoundingClientRect(),
    px=b.x+(e.clientX-r.left)/r.width*b.width, py=b.y+(e.clientY-r.top)/r.height*b.height,
    k=e.deltaY<0?0.8:1.25, nw=Math.min(w0,Math.max(w0/60,b.width*k)), nh=nw*h0/w0;
    b.x=px-(px-b.x)*nw/b.width; b.y=py-(py-b.y)*nh/b.height; b.width=nw; b.height=nh; z();},{passive:false});
  s.addEventListener('pointerdown',function(e){drag={x:e.clientX,y:e.clientY,bx:b.x,by:b.y};});
  window.addEventListener('pointerup',function(){drag=null;});
  s.addEventListener('pointermove',function(e){if(!drag)return;var r=s.getBoundingClientRect();
    b.x=drag.bx-(e.clientX-drag.x)/r.width*b.width; b.y=drag.by-(e.clientY-drag.y)/r.height*b.height;});
});
document.querySelectorAll('.legend.keys').forEach(function(l){
  var s=document.getElementById(l.getAttribute('data-for')); if(!s)return;
  l.querySelectorAll('button.key').forEach(function(k){k.addEventListener('click',function(){
    var on=s.getAttribute('data-only')===k.dataset.k;
    if(on){s.removeAttribute('data-only');}else{s.setAttribute('data-only',k.dataset.k);}
    l.querySelectorAll('button.key').forEach(function(o){o.classList.toggle('off',!on&&o!==k);});
  });});
});}
</script>"#;

#[cfg(test)]
mod tests {
    #[test]
    fn a_statement_says_what_to_show_in_few_words() {
        let show: Vec<super::Show> = crate::yaml::parse(
            "- map: { place: location, colour: [diesel, e10], cheap: true, label: city }\n- areas: { country: country, value: inflation, by: year }\n- matrix: { rows: severity, columns: epss, bands: [0.01, 0.1, 0.5] }\n- table: { entity: country, time: year, value: [inflation] }\n- averages: { by: region, value: diesel }\n- overlap: {}\n- arrivals: { date: known, source: zetlyn/cve-nvd, recent: 10 }\n",
        )
        .unwrap();
        assert!(matches!(&show[0], super::Show::Map(m) if m.colour == ["diesel", "e10"] && m.cheap && m.label.as_deref() == Some("city")));
        assert!(matches!(&show[1], super::Show::Areas(a) if a.value == ["inflation"] && a.country == ["country"] && a.by.as_deref() == Some("year")));
        assert!(matches!(&show[2], super::Show::Matrix(m) if m.bands.len() == 3));
        assert!(matches!(&show[3], super::Show::Table(t) if t.limit == 50));
        assert!(matches!(&show[5], super::Show::Overlap(o) if o.limit == 12));
        assert!(matches!(&show[6], super::Show::Arrivals(a) if a.recent == 10 && a.source.is_some()));
    }

    #[test]
    fn round_numbers_on_an_axis() {
        assert_eq!(super::ticks(0.0, 10.0, 5), vec![0.0, 2.0, 4.0, 6.0, 8.0, 10.0]);
        assert_eq!(super::with_param("/x?colour=a&q=1", "colour", "e10"), "/x?q=1&colour=e10");
    }

    #[test]
    fn values_written_to_a_step_are_counted_a_bar_a_step() {
        assert_eq!(super::grid_of(&[7.5, 9.8, 3.1]), Some(0.1));
        assert_eq!(super::grid_of(&[1.0, 4.0]), Some(1.0));
        assert_eq!(super::grid_of(&[0.123456]), None);
        assert_eq!(super::Grid::year_before("2026-Q2").as_deref(), Some("2025-Q2"));
        assert_eq!(super::ordinal(22), "22nd");
        assert_eq!(super::ordinal(12), "12th");
    }
}
