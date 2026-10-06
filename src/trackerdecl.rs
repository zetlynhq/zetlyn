//! What a tracker declares. It holds no documents: it names sources, says what makes two of their
//! claims the same thing, maps their words onto one scale, and states a promise.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrackerDecl {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
    #[serde(default, rename = "sources", skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<SourceRef>,
    /// The identifier schemes that say two claims are about one thing. A scheme, and nothing else.
    #[serde(default, rename = "identified_by", skip_serializing_if = "Vec::is_empty")]
    pub join: Vec<String>,
    #[serde(default, rename = "align", skip_serializing_if = "BTreeMap::is_empty")]
    pub normalise: BTreeMap<String, Align>,
    /// What a thing is to something else, where one claim states both identifiers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<Relation>,
    /// `public`, the default: its overview and thing pages are open to anyone, its claims to
    /// subscribers. `private`: every page for its accounts only, and no free edge.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub visibility: String,
    #[serde(default, skip_serializing_if = "Views::is_empty")]
    pub view: Views,
    #[serde(default, skip_serializing_if = "Promise::is_empty")]
    pub promise: Promise,
    /// How a thing's own page reads before its claims: what is said first, a ladder of how far it
    /// has got, and which dates make its timeline. Without it, the page is the claims.
    #[serde(default, skip_serializing_if = "ThingView::is_empty")]
    pub thing: ThingView,
    /// What a list of what somebody runs is checked against: an SBOM, `rpm -qa`, CPEs. Each
    /// names a property, or the relation, that says which packages or products a thing is in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory: Option<Inventory>,
    /// Set on a tracker that arrived as a package: where from, which version, whose key. A sealed
    /// one is never built here, because it holds no recipe to build it with; a newer package
    /// replaces it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<Packaged>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Packaged {
    /// A hub, or the file it was opened from.
    pub from: String,
    /// `owner/name@tag` on that hub; absent for a file.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    pub version: String,
    #[serde(default)]
    pub sealed: bool,
    /// The publisher's key, pinned: a newer version not signed by it is not taken.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRef {
    /// One of the two. A source is a source this workspace holds, or one somewhere else that
    /// answers the same six calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// A key, where the other side asks for one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, rename = "source", skip_serializing_if = "String::is_empty")]
    pub dataset: String,
    #[serde(default, skip_serializing_if = "Priority::is_normal")]
    pub priority: Priority,
    /// Required, one sentence, and not decoration. A source nobody can justify in a sentence is a
    /// source somebody added and nobody removed.
    pub why: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Primary,
    High,
    #[default]
    Normal,
}

impl Priority {
    /// Orders the answer between sources. Not a weight, and it never multiplies a score.
    pub fn rank(self) -> u8 {
        match self {
            Priority::Primary => 0,
            Priority::High => 1,
            Priority::Normal => 2,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Priority::Primary => "primary",
            Priority::High => "high",
            Priority::Normal => "normal",
        }
    }
    fn is_normal(&self) -> bool {
        *self == Priority::Normal
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Align {
    /// Best first. What makes `severity>=high` answerable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scale: Vec<String>,
    /// A source's field name onto the tracker's, where they differ.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub from: BTreeMap<String, String>,
    /// How far apart two sources may be and still agree: a number (`0.1`), or for dates a count
    /// of days (`1d`). Absent, every difference is a conflict, because a small real difference
    /// hidden by a default is worse than a loud one somebody can declare away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<String>,
    /// Per source, what its words mean on the scale above.
    #[serde(flatten)]
    pub members: BTreeMap<String, BTreeMap<String, String>>,
}

impl Align {
    pub fn field_in(&self, member: &str, scope_field: &str) -> String {
        self.from
            .get(member)
            .cloned()
            .unwrap_or_else(|| scope_field.to_string())
    }
    /// What the source said, and what this tracker makes of it. A value with no thing passes through
    /// unchanged rather than becoming `unknown`.
    pub fn means(&self, member: &str, raw: &str) -> String {
        self.members
            .get(member)
            .and_then(|m| m.get(&raw.to_lowercase()).or_else(|| m.get(raw)))
            .cloned()
            .unwrap_or_else(|| raw.to_string())
    }
    pub fn position(&self, value: &str) -> Option<usize> {
        self.scale
            .iter()
            .position(|s| s.eq_ignore_ascii_case(value))
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Views {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facets: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    /// Views the curator wrote, across every source.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub named: Vec<NamedView>,
    /// One per kind. Everything else in this table is a kind.
    #[serde(flatten)]
    pub kinds: BTreeMap<String, KindView>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KindView {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facets: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    /// A view the source already declared about itself, as `source:view`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adopt: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NamedView {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(rename = "where", skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facets: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
}

/// The product, written down so that it can be checked.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Promise {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fresh_within: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub covers: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub excludes: String,
}

impl Views {
    fn is_empty(&self) -> bool {
        self.columns.is_empty()
            && self.facets.is_empty()
            && self.sort.is_none()
            && self.named.is_empty()
            && self.kinds.is_empty()
    }
}

impl Promise {
    fn is_empty(&self) -> bool {
        self.fresh_within.is_none() && self.covers.is_empty() && self.excludes.is_empty()
    }
}

/// The file a tracker is, inside its directory.
pub const FILE: &str = "tracker.yaml";

impl TrackerDecl {
    pub fn load(dir: &Path) -> Result<TrackerDecl, String> {
        let mut d: TrackerDecl = crate::yaml::read(&dir.join(FILE))?;
        if d.title.is_empty() {
            d.title = d.name.clone();
        }
        Ok(d)
    }

    pub fn keys(&self) -> Vec<&str> {
        self.join.iter().map(String::as_str).collect()
    }

    pub fn normalise_for(&self, field: &str) -> Option<&Align> {
        self.normalise.get(field)
    }
}

impl Align {
    /// The tolerance as a number, for numbers.
    pub fn number_tolerance(&self) -> f64 {
        self.tolerance
            .as_deref()
            .and_then(|t| t.trim().parse::<f64>().ok())
            .unwrap_or(0.0)
    }
    /// The tolerance in days, for dates: `1d`, or a bare number.
    pub fn day_tolerance(&self) -> i64 {
        self.tolerance
            .as_deref()
            .map(|t| t.trim().trim_end_matches('d'))
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or(0)
    }
}

/// A relation, as a result: a claim that names the thing and names something else of `to` says
/// that the thing is `name` that. Nobody draws it; it is there where a source states it, or where
/// a person has confirmed it and signed it (D9).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Relation {
    /// The verb, read from the thing: `affects`, `made_by`, `quantises`.
    pub name: String,
    /// The other identifier's scheme.
    pub to: String,
    /// Which part of it: a CPE names a version of a product of a vendor, and `product` or
    /// `vendor` says which of those this relation is about. Absent, the whole identifier.
    #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    /// Where a source says the other side in words and not by identifier: the properties that
    /// name it, joined into the same spelling, offered to a person to confirm and never used
    /// on their own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggest_from: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
}

impl Relation {
    /// The other side as this relation compares it, from one identifier of `to`.
    pub fn target(&self, value: &str) -> Option<String> {
        let v = value.trim().to_lowercase();
        if self.to != "cpe" || self.part.is_none() {
            return Some(v).filter(|v| !v.is_empty());
        }
        // cpe:2.3:a:vendor:product:version:… and cpe:/a:vendor:product:version
        let parts: Vec<&str> = match v.strip_prefix("cpe:2.3:") {
            Some(rest) => rest.split(':').skip(1).collect(),
            None => v.strip_prefix("cpe:/")?.split(':').skip(1).collect(),
        };
        let (vendor, product) = (parts.first().copied().unwrap_or(""), parts.get(1).copied().unwrap_or(""));
        let named = |s: &str| !s.is_empty() && s != "*" && s != "-";
        match self.part.as_deref() {
            Some("vendor") if named(vendor) => Some(vendor.to_string()),
            Some("product") if named(vendor) && named(product) => Some(format!("{vendor}/{product}")),
            _ => None,
        }
    }

    /// A spelling of words for the other side, the way a CPE spells it: `Microsoft`,
    /// `Windows Server 2025` is `microsoft/windows_server_2025`.
    pub fn spell(words: &[&str]) -> String {
        words
            .iter()
            .map(|w| {
                let s: String = w.trim().to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
                s.split('_').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("_")
            })
            .collect::<Vec<_>>()
            .join("/")
    }
}

/// What a thing's page says first, declared by the tracker, since only it knows which of its
/// properties answer the first question somebody asks of one of its things. Every line still
/// names the source that said it: this orders what the sources say, and says nothing itself.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ThingView {
    /// The properties shown first, in this order, each with every source that says it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub summary: Vec<String>,
    /// Steps a thing climbs, each reached when what it names is said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ladder: Option<Ladder>,
    /// Date properties that are events, beside the day each source first spoke of the thing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timeline: Vec<String>,
}

impl ThingView {
    pub fn is_empty(&self) -> bool {
        self.summary.is_empty() && self.ladder.is_none() && self.timeline.is_empty()
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Ladder {
    pub title: String,
    pub steps: Vec<Step>,
}

/// One rung. `when` is terms joined by `and`: `has:<source>`, `<property>=<value>` said by any
/// source, or `<source>.<property>=<value>`. Empty, the rung every thing stands on.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub when: String,
}

/// Where a tracker finds what an inventory is checked against.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Inventory {
    /// A property holding the packages a fix was shipped in, as RPMs name them:
    /// `openssl-1:3.0.7-27.el9`. An installed package of that name and stream below it is affected.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rpm: String,
    /// A property holding, per package, `<ecosystem> <name> <range>; fixed in <version>`, as
    /// GitHub's advisories say it: `npm lodash < 4.17.21; fixed in 4.17.21`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub packages: String,
    /// A relation to products as vendor/product, which a CPE in the inventory is looked up by.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub products: String,
    /// What the page says first, about where the lists come from.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
}
