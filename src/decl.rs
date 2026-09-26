//! The declaration. Eight blocks, and the whole of what a dataset creator writes.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Declaration {
    pub name: String,
    #[serde(default)]
    pub title: String,
    pub kind: String,
    #[serde(default)]
    pub about: String,
    pub source: Source,
    #[serde(default)]
    pub schedule: Schedule,
    pub records: Records,
    /// Each publisher's own words, defined in each publisher's own sentence.
    #[serde(default)]
    pub vocabulary: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    pub view: Vec<View>,
    #[serde(default)]
    pub search: Search,
    #[serde(default)]
    pub retention: Retention,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
// The variants differ in size because `http` carries far more than `csv` does. Boxing one
// of them to even that out would put an indirection in the hot path of every row for the
// sake of a declaration that is read once.
#[allow(clippy::large_enum_variant)]
pub enum Source {
    /// A directory of files, recursively. May name a git repository, pulled before each run.
    Folder {
        #[serde(default)]
        path: Option<String>,
        git: Option<String>,
        #[serde(default)]
        include: Vec<String>,
        #[serde(default)]
        exclude: Vec<String>,
    },
    Csv {
        path: String,
        #[serde(default = "comma")]
        delimiter: String,
        #[serde(default)]
        skip: usize,
    },
    Xlsx {
        path: String,
        #[serde(default)]
        sheets: Vec<String>,
        #[serde(default = "one")]
        header_row: usize,
    },
    /// A JSON API, optionally a list call and a detail call per item.
    Http {
        #[serde(default)]
        list: String,
        /// Where the identifiers come from, when they come from another dataset rather
        /// than from a list call. A GGUF repository names the model it quantised, and
        /// nothing but that dataset knows which models those are.
        for_each: Option<ForEach>,
        /// One call per item. The row is that answer, with the list item under `_list`.
        detail: Option<String>,
        #[serde(default)]
        page: Option<Page>,
        /// Splits a stretch into calls the source will accept.
        window: Option<String>,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default = "agent")]
        user_agent: String,
        #[serde(default)]
        pause_ms: u64,
        since: Option<String>,
        #[serde(default = "epoch")]
        since_default: String,
        /// For trying a shape. A truncated run has not seen the source, so it never removes
        /// and never advances the mark.
        #[serde(default)]
        limit: usize,
        /// The first this many the source lists, in its own order. A declared coverage
        /// rather than a truncation: a source ordered by popularity has no date to stop
        /// at, and `the 5,000 most downloaded` is a promise somebody can check.
        #[serde(default)]
        top: usize,
    },
    /// RSS and Atom. Several feeds of the same shape are one dataset.
    Feed {
        urls: Vec<String>,
        /// The licence decision, made once and visible in one line.
        #[serde(default = "summary")]
        text_is: String,
        #[serde(default = "agent")]
        user_agent: String,
        #[serde(default = "thousand")]
        pause_ms: u64,
    },
}

fn agent() -> String {
    "zetlyn/3".into()
}
fn epoch() -> String {
    "1970-01-01".into()
}
fn summary() -> String {
    "summary".into()
}
fn thousand() -> u64 {
    1000
}

fn comma() -> String {
    ",".into()
}
fn one() -> usize {
    1
}

impl Source {
    /// A run that stopped at a declared `limit` has not seen the source, so it never
    /// licenses a removal and never advances the mark. `limit` is for trying a shape.
    pub fn truncating(&self) -> bool {
        matches!(self, Source::Http { limit, .. } if *limit > 0)
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            Source::Folder { .. } => "folder",
            Source::Csv { .. } => "csv",
            Source::Xlsx { .. } => "xlsx",
            Source::Http { .. } => "http",
            Source::Feed { .. } => "feed",
        }
    }
    /// What `file:` paths resolve against, and what a run reads.
    pub fn root(&self, base: &Path) -> std::path::PathBuf {
        let p = match self {
            Source::Folder { path, git, .. } => {
                let named = path.clone().unwrap_or_else(|| {
                    if git.is_some() {
                        "checkout".into()
                    } else {
                        ".".into()
                    }
                });
                return base.join(named);
            }
            Source::Csv { path, .. } => path,
            Source::Xlsx { path, .. } => path,
            Source::Http { .. } | Source::Feed { .. } => return base.to_path_buf(),
        };
        let full = base.join(p);
        if full.is_dir() {
            full
        } else {
            full.parent().map(Path::to_path_buf).unwrap_or(base.into())
        }
    }

    /// A checkout is fetched before it is read, and `git` is the whole of what that needs.
    pub fn prepare(&self, base: &Path) -> Result<(), String> {
        let Source::Folder { git: Some(url), .. } = self else {
            return Ok(());
        };
        let dir = self.root(base);
        let run = |args: Vec<String>| -> Result<(), String> {
            let out = std::process::Command::new("git")
                .args(&args)
                .output()
                .map_err(|e| format!("git: {e}"))?;
            if out.status.success() {
                return Ok(());
            }
            Err(format!(
                "git {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        };
        if dir.join(".git").is_dir() {
            let d = dir.display().to_string();
            run(vec![
                "-C".into(),
                d.clone(),
                "fetch".into(),
                "--depth".into(),
                "1".into(),
                "origin".into(),
            ])?;
            run(vec![
                "-C".into(),
                d,
                "reset".into(),
                "--hard".into(),
                "FETCH_HEAD".into(),
            ])
        } else {
            std::fs::create_dir_all(dir.parent().unwrap_or(base)).map_err(|e| e.to_string())?;
            run(vec![
                "clone".into(),
                "--depth".into(),
                "1".into(),
                url.clone(),
                dir.display().to_string(),
            ])
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    pub every: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Records {
    /// Names the list where one fetched thing holds many records.
    pub each: Option<String>,
    #[serde(rename = "where")]
    pub filter: Option<String>,
    /// One, or several. A GitHub advisory issues a GHSA and names the CVE it is about,
    /// and that cross-reference is what lets two datasets meet.
    pub id: Option<Ids>,
    pub title: String,
    pub url: Option<String>,
    #[serde(default)]
    pub text: Vec<String>,
    pub known: Option<String>,
    #[serde(default)]
    pub fields: BTreeMap<String, FieldSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub from: String,
    #[serde(rename = "match")]
    pub matches: Option<String>,
    pub separator: Option<String>,
    #[serde(default)]
    pub all: bool,
    pub default: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdSpec {
    pub scheme: Option<String>,
    pub from: String,
    #[serde(rename = "match")]
    pub matches: Option<String>,
    pub separator: Option<String>,
    #[serde(default)]
    pub all: bool,
}

impl IdSpec {
    pub fn spec(&self) -> Spec {
        Spec {
            from: self.from.clone(),
            matches: self.matches.clone(),
            separator: self.separator.clone(),
            all: self.all,
            default: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Text,
    Code,
    Number,
    Bool,
    Date,
    Interval,
}

impl FieldType {
    pub fn name(self) -> &'static str {
        match self {
            FieldType::Text => "text",
            FieldType::Code => "code",
            FieldType::Number => "number",
            FieldType::Bool => "bool",
            FieldType::Date => "date",
            FieldType::Interval => "interval",
        }
    }
    /// `number` and `date` compare by their own order. `text`, `bool` and `code` compare for
    /// equality: ordering a code needs a scale, and a scale is a scope's declaration.
    pub fn ordered(self) -> bool {
        matches!(
            self,
            FieldType::Number | FieldType::Date | FieldType::Interval
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldSpec {
    #[serde(rename = "type")]
    pub kind: FieldType,
    pub vocabulary: Option<String>,
    pub from: String,
    #[serde(rename = "match")]
    pub matches: Option<String>,
    pub separator: Option<String>,
    #[serde(default)]
    pub all: bool,
    pub default: Option<String>,
}

impl FieldSpec {
    pub fn spec(&self) -> Spec {
        Spec {
            from: self.from.clone(),
            matches: self.matches.clone(),
            separator: self.separator.clone(),
            all: self.all,
            default: self.default.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub default: bool,
    #[serde(rename = "where")]
    pub filter: Option<String>,
    pub group: Option<String>,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub facets: Vec<String>,
    pub sort: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Search {
    /// Which parts of a record the full-text index covers.
    #[serde(default)]
    pub text: Vec<String>,
    /// Which fields answer a comparison.
    #[serde(default)]
    pub compare: Vec<String>,
    #[serde(default)]
    pub suggest: Vec<String>,
    /// Real queries in this source's own vocabulary, shown on an empty search box.
    #[serde(default)]
    pub examples: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(Default)]
pub struct Retention {
    #[serde(default)]
    pub history: bool,
}

impl Declaration {
    pub fn load(dir: &Path) -> Result<Declaration, String> {
        let path = dir.join("dataset.toml");
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut d: Declaration =
            toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if d.title.is_empty() {
            d.title = d.name.clone();
        }
        Ok(d)
    }

    pub fn default_view(&self) -> Option<&View> {
        self.view
            .iter()
            .find(|v| v.default)
            .or_else(|| self.view.first())
    }

    pub fn view(&self, name: &str) -> Option<&View> {
        self.view.iter().find(|v| v.name == name)
    }
}

/// How a source pages. `max` is what it will hand over in one answer.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    /// Where the next page is named. `link` reads the `Link` header; otherwise the page is
    /// asked for by number or by offset.
    pub cursor: Option<String>,
    #[serde(default)]
    pub offset: String,
    #[serde(default)]
    pub size: String,
    /// Where the answer says how many there are in total.
    pub total: Option<String>,
    /// `offset` counts records, `page` counts pages from one. Sources do both.
    #[serde(default = "offset_word")]
    pub by: String,
    #[serde(default = "hundred")]
    pub max: usize,
}

fn offset_word() -> String {
    "offset".into()
}

fn hundred() -> usize {
    100
}

/// A declaration writes one table or a list of them, and both mean the same thing.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Ids {
    One(IdSpec),
    Many(Vec<IdSpec>),
}

impl Ids {
    pub fn each(&self) -> Vec<&IdSpec> {
        match self {
            Ids::One(one) => vec![one],
            Ids::Many(many) => many.iter().collect(),
        }
    }
    /// What names the record, where the first scheme carries one identifier rather than several.
    pub fn names_record(&self) -> bool {
        self.each().first().map(|s| !s.all).unwrap_or(false)
    }
}

/// A dataset that takes its subjects from another one. The named dataset has to be installed in
/// the same deployment, and a run without it refuses rather than reading nothing.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForEach {
    #[serde(default)]
    pub dataset: String,
    pub scheme: String,
}
