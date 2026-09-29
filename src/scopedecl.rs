//! What a scope declares. It holds no documents: it names members, says what makes two of their
//! records the same thing, maps their words onto one scale, and states a promise.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeDecl {
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub about: String,
    #[serde(default)]
    pub members: Vec<MemberDecl>,
    #[serde(default)]
    pub join: Vec<Join>,
    #[serde(default)]
    pub normalise: BTreeMap<String, Normalise>,
    #[serde(default)]
    pub view: Views,
    #[serde(default)]
    pub promise: Promise,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberDecl {
    /// One of the two. A member is a dataset this deployment holds, or one somewhere else that
    /// answers the same six calls.
    pub remote: Option<String>,
    /// A key, where the other side asks for one.
    pub key: Option<String>,
    #[serde(default)]
    pub dataset: String,
    #[serde(default)]
    pub priority: Priority,
    /// Required, one sentence, and not decoration. A member nobody can justify in a sentence is a
    /// member somebody added and nobody removed.
    pub why: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Primary,
    High,
    #[default]
    Normal,
}

impl Priority {
    /// Orders the answer between members. Not a weight, and it never multiplies a score.
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
}

/// A join names an identifier scheme, and nothing else.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Join {
    pub key: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct Normalise {
    /// Best first. What makes `severity>=high` answerable.
    #[serde(default)]
    pub scale: Vec<String>,
    /// A member's field name onto the scope's, where they differ.
    #[serde(default)]
    pub from: BTreeMap<String, String>,
    /// Per member, what its words mean on the scale above.
    #[serde(flatten)]
    pub members: BTreeMap<String, BTreeMap<String, String>>,
}

impl Normalise {
    pub fn field_in(&self, member: &str, scope_field: &str) -> String {
        self.from
            .get(member)
            .cloned()
            .unwrap_or_else(|| scope_field.to_string())
    }
    /// What the member said, and what this scope makes of it. A value with no entry passes through
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

#[derive(Debug, Default, Deserialize)]
pub struct Views {
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub facets: Vec<String>,
    pub sort: Option<String>,
    /// Views the curator wrote, across every member.
    #[serde(default)]
    pub named: Vec<NamedView>,
    /// One per kind. Everything else in this table is a kind.
    #[serde(flatten)]
    pub kinds: BTreeMap<String, KindView>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KindView {
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub facets: Vec<String>,
    pub sort: Option<String>,
    /// A view the member already declared about itself, as `dataset:view`.
    pub adopt: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedView {
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(rename = "where")]
    pub filter: Option<String>,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub facets: Vec<String>,
    pub sort: Option<String>,
}

/// The product, written down so that it can be checked.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Promise {
    pub fresh_within: Option<String>,
    #[serde(default)]
    pub covers: String,
    #[serde(default)]
    pub excludes: String,
}

impl ScopeDecl {
    pub fn load(dir: &Path) -> Result<ScopeDecl, String> {
        let path = dir.join("scope.toml");
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut d: ScopeDecl =
            toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if d.title.is_empty() {
            d.title = d.name.clone();
        }
        Ok(d)
    }

    pub fn keys(&self) -> Vec<&str> {
        self.join.iter().map(|j| j.key.as_str()).collect()
    }

    pub fn normalise_for(&self, field: &str) -> Option<&Normalise> {
        self.normalise.get(field)
    }
}
