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
    #[serde(default, skip_serializing_if = "Views::is_empty")]
    pub view: Views,
    #[serde(default, skip_serializing_if = "Promise::is_empty")]
    pub promise: Promise,
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
