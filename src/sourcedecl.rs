//! The declaration. Eight blocks, and the whole of what a source creator writes.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceDecl {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// The kind of claim this source makes: an advisory, an exploit, an article.
    /// Left out, every claim is an `item`.
    #[serde(default = "an_item")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
    /// Where the bytes are. `source` is the whole of this file, so this part is `fetch`.
    #[serde(rename = "fetch")]
    pub source: Fetch,
    #[serde(default, skip_serializing_if = "Schedule::is_empty")]
    pub schedule: Schedule,
    /// What names each record, and the column it is in: `isbn: field:EAN`. The same word a
    /// tracker uses for what its sources meet on, so the two files say it the same way.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub identified_by: BTreeMap<String, IdWhere>,
    /// What else each record is about, several each: an exploit names the CVEs it is for.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub refers_to: BTreeMap<String, IdWhere>,
    #[serde(rename = "claims")]
    pub records: ClaimsDecl,
    /// Each publisher's own words, defined in each publisher's own sentence.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vocabulary: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default, rename = "views", skip_serializing_if = "Vec::is_empty")]
    pub view: Vec<View>,
    #[serde(default, skip_serializing_if = "Search::is_empty")]
    pub search: Search,
    #[serde(default, skip_serializing_if = "Retention::is_empty")]
    pub retention: Retention,
    /// What a subscriber may do with these claims, in the publisher's own words. Nothing checks
    /// it. It is the claim a person makes and answers for, and it travels in the manifest.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub terms: String,
    /// How much of these claims a public tracker shows: `yes`, all of them; `summary`, their
    /// titles, values and a link, not their text. A public tracker shows its values whatever its
    /// sources' own pages are (2026-10-09); `no`, from before, reads as `summary` and a private page.
    #[serde(default, skip_serializing_if = "Licence::is_empty")]
    pub licence: Licence,
    /// Whether the source's own page is open to anyone: `public`, `private`, or unset, which is as
    /// its trackers are: open while a public tracker holds it, closed where only private ones do.
    /// Its owner's to say, whatever its trackers are.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub visibility: String,
    /// Who edits this source and nothing else of the world, beside its owners and editors: they
    /// open its page, see its proposals and decide them. An address, `domain:example.org`,
    /// `@zetlyn.com` or `signed-in`, as the world's lists say them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub editors: Vec<String>,
    /// Proposals from readers, for a source that is read from somewhere else: new rows where the
    /// declaration reads fields, and corrections of what a claim says. Who may: as a proposals
    /// source's `readers` (`signed-in`, addresses, `domain:`…; empty, the world's own proposers).
    /// Absent, it takes none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposals: Option<Takes>,
}

/// Who may propose to a source that is not itself a proposals source.
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Takes {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readers: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
// The variants differ in size because `http` carries far more than `csv` does. Boxing one
// of them to even that out would put an indirection in the hot path of every row for the
// sake of a declaration that is read once.
#[allow(clippy::large_enum_variant)]
pub enum Fetch {
    /// A directory of files, recursively. May name a git repository, pulled before each run.
    Folder {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        git: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        include: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        exclude: Vec<String>,
    },
    Csv {
        path: String,
        #[serde(default = "comma", skip_serializing_if = "is_comma")]
        delimiter: String,
        #[serde(default, skip_serializing_if = "is_zero")]
        skip: usize,
        /// The names of the columns, for a file that starts with its first row: OFAC's lists
        /// have no header.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        columns: Vec<String>,
        /// What the file writes for an empty cell, read as empty: OFAC writes `-0-`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blank: Option<String>,
        /// More files of the same kind, read after `path`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        paths: Vec<String>,
        /// Sent with a file fetched from a URL: the IMF answers in XML unless asked for CSV.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        headers: BTreeMap<String, String>,
    },
    Xlsx {
        path: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sheets: Vec<String>,
        #[serde(default = "one", skip_serializing_if = "is_one")]
        header_row: usize,
    },
    /// A JSON API, optionally a list call and a detail call per item.
    Http {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        list: String,
        /// Where the identifiers come from, when they come from another source rather
        /// than from a list call. A GGUF repository names the model it quantised, and
        /// nothing but that source knows which models those are.
        #[serde(skip_serializing_if = "Option::is_none")]
        for_each: Option<ForEach>,
        /// One call per item. The row is that answer, with the list item under `_list`.
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        page: Option<Page>,
        /// Splits a stretch into calls the source will accept.
        #[serde(skip_serializing_if = "Option::is_none")]
        window: Option<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        headers: BTreeMap<String, String>,
        #[serde(default = "agent", skip_serializing_if = "is_agent")]
        user_agent: String,
        #[serde(default, skip_serializing_if = "is_zero")]
        pause_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<String>,
        #[serde(default = "epoch", skip_serializing_if = "is_epoch")]
        since_default: String,
        /// For trying a shape. A truncated run has not seen the source, so it never removes
        /// and never advances the mark.
        #[serde(default, skip_serializing_if = "is_zero")]
        limit: usize,
        /// The first this many the source lists, in its own order. A declared coverage
        /// rather than a truncation: a source ordered by popularity has no date to stop
        /// at, and `the 5,000 most downloaded` is a promise somebody can check.
        #[serde(default, skip_serializing_if = "is_zero")]
        top: usize,
    },
    /// RSS and Atom. Several feeds of the same shape are one source.
    Feed {
        urls: Vec<String>,
        /// The licence decision, made once and visible in one line.
        #[serde(default = "summary", skip_serializing_if = "is_summary")]
        text_is: String,
        #[serde(default = "agent", skip_serializing_if = "is_agent")]
        user_agent: String,
        #[serde(default = "thousand", skip_serializing_if = "is_thousand")]
        pause_ms: u64,
    },
    /// A web page, read as people see it: each element `items` selects is a claim, and `fields`
    /// says what of it becomes a value, so the rest of the declaration reads it as it reads an
    /// API's answer. `.title` is the text of the first element that selector finds in the item,
    /// `.price@data-final` an attribute of it, `@href` the item's own.
    Web {
        url: String,
        items: String,
        #[serde(default)]
        fields: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        page: Option<Page>,
        /// For trying a shape: this many items, one page or so. A truncated run has not seen the
        /// list, so it never removes and never advances the mark.
        #[serde(default, skip_serializing_if = "is_zero")]
        limit: usize,
        /// The first this many items, in the page's own order: a declared coverage.
        #[serde(default, skip_serializing_if = "is_zero")]
        top: usize,
        /// A list sorted newest first, read back to this: the path of an item's date, and where to
        /// stop the first time. After that the newest date read is the mark, and an update reads
        /// only what is newer.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<String>,
        #[serde(default = "epoch", skip_serializing_if = "is_epoch")]
        since_default: String,
        #[serde(default = "agent", skip_serializing_if = "is_agent")]
        user_agent: String,
        #[serde(default = "thousand", skip_serializing_if = "is_thousand")]
        pause_ms: u64,
    },
    /// Pushed to rather than fetched: every signed POST to the workspace's `/hook/<source>` is kept
    /// as a file in `inbox/`, and an update reads those as it reads an API's answers.
    Webhook {
        /// A variable, `${HOOK_SECRET}`: the key the sender signs each body with (HMAC-SHA256,
        /// `X-Hub-Signature-256: sha256=…`).
        secret: String,
    },
    /// Read by people for it: every signed POST to the workspace's `/propose/<source>` from a key
    /// in `from` is kept in `proposals/`, and only what the owner accepts reaches `inbox/`, which
    /// an update reads as a webhook's.
    Proposals {
        /// The keys invited to propose, `ed25519:…` as `zetlyn id` prints them.
        #[serde(default)]
        from: Vec<String>,
        /// Readers of this workspace's published pages who may propose from the browser, the
        /// workspace signing for them: `signed-in` for anybody signed in, or their addresses.
        /// Empty, nobody may.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        readers: Vec<String>,
    },
    /// A query against PostgreSQL. The connection string names a variable, never a password,
    /// and the query carries the watermark as `{since}`.
    Sql {
        dsn: String,
        query: String,
        /// Where a row says when it changed: the next update asks only for rows after it.
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<String>,
        #[serde(default = "epoch", skip_serializing_if = "is_epoch")]
        since_default: String,
    },
    /// Subscribed rather than fetched. The claims arrived built, so there is nothing here to
    /// extract and nothing to re-run: a run against this asks the hub for a newer version.
    Hub {
        /// Where the hub is: a folder, a mount, `s3://bucket/prefix`, or an address.
        at: String,
        /// `[host/]owner/name[@tag]`.
        #[serde(rename = "ref")]
        reference: String,
        /// What the publisher declared. The licence fact travels with the claims, because a
        /// subscriber holding them has to answer for them too.
        #[serde(default = "summary", skip_serializing_if = "is_summary")]
        text_is: String,
        /// The publisher's public key, pinned here. Where it is set, a version whose manifest is
        /// not signed by it is not applied. It is the one thing a hub cannot produce, and the
        /// reason a hub that is only a directory over HTTPS is enough.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        key: String,
    },
    /// Part of a packaged tracker: the claims arrived inside it, built, and only a newer package
    /// changes them.
    Package {
        /// The tracker it came with.
        tracker: String,
        #[serde(default = "summary", skip_serializing_if = "is_summary")]
        text_is: String,
    },
}

/// What the program calls itself when it asks a source, where the declaration does not say.
pub const AGENT: &str = concat!("zetlyn/", env!("CARGO_PKG_VERSION"));

fn agent() -> String {
    AGENT.into()
}

// A default is not written back. A file the interface writes says what somebody chose, and a
// line for every value nobody chose would bury it.
fn is_agent(s: &str) -> bool {
    s == AGENT
}
fn is_epoch(s: &str) -> bool {
    s == epoch()
}
fn is_summary(s: &str) -> bool {
    s == summary()
}
fn is_thousand(n: &u64) -> bool {
    *n == thousand()
}
fn is_comma(s: &str) -> bool {
    s == comma()
}
fn is_one(n: &usize) -> bool {
    *n == one()
}
fn is_zero<T: Default + PartialEq>(n: &T) -> bool {
    *n == T::default()
}
fn is_false(b: &bool) -> bool {
    !*b
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

impl Fetch {
    /// A run that stopped at a declared `limit` has not seen the source, so it never
    /// licenses a removal and never advances the mark. `limit` is for trying a shape.
    pub fn truncating(&self) -> bool {
        matches!(self, Fetch::Http { limit, .. } | Fetch::Web { limit, .. } if *limit > 0)
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            Fetch::Folder { .. } => "folder",
            Fetch::Csv { .. } => "csv",
            Fetch::Xlsx { .. } => "xlsx",
            Fetch::Http { .. } => "http",
            Fetch::Feed { .. } => "feed",
            Fetch::Hub { .. } => "hub",
            Fetch::Package { .. } => "package",
            Fetch::Sql { .. } => "sql",
            Fetch::Webhook { .. } => "webhook",
            Fetch::Proposals { .. } => "proposals",
            Fetch::Web { .. } => "web",
        }
    }

    /// Where these claims came from, in one line, for a reader deciding whether they may hold
    /// them. A folder is the operator's own machine and says so rather than naming a path.
    pub fn address(&self) -> String {
        match self {
            Fetch::Folder { git: Some(url), .. } => url.clone(),
            Fetch::Folder { .. } => "a directory on the publisher's machine".into(),
            Fetch::Csv { path, .. } | Fetch::Xlsx { path, .. } => path.clone(),
            Fetch::Http { list, detail, .. } => {
                let named = if list.is_empty() {
                    detail.clone().unwrap_or_default()
                } else {
                    list.clone()
                };
                named.split(['?', '{']).next().unwrap_or("").to_string()
            }
            Fetch::Feed { urls, .. } => urls.join(", "),
            Fetch::Hub { at, reference, .. } => format!("{at} {reference}"),
            Fetch::Package { tracker, .. } => format!("inside the package {tracker}"),
            // Never the connection string: it is a password, however it was written.
            Fetch::Sql { .. } => "a PostgreSQL database".into(),
            Fetch::Webhook { .. } => "what its sender pushes to it".into(),
            Fetch::Proposals { .. } => "what people who read it propose, as its owner accepts it".into(),
            Fetch::Web { url, .. } => url.clone(),
        }
    }

    /// Whether the text in these claims is what the source published about itself or the thing
    /// itself. Fetching, indexing and republishing are three acts, and this is the third one
    /// stated in one word. A source that carries no text of its own says `none`.
    pub fn text_is(&self) -> &str {
        match self {
            Fetch::Feed { text_is, .. } => text_is,
            Fetch::Folder { .. } | Fetch::Csv { .. } | Fetch::Xlsx { .. } => "whole",
            Fetch::Http { .. } | Fetch::Sql { .. } | Fetch::Webhook { .. } | Fetch::Proposals { .. } | Fetch::Web { .. } => "whole",
            Fetch::Hub { text_is, .. } => text_is,
            Fetch::Package { text_is, .. } => text_is,
        }
    }
    /// What `file:` paths resolve against, and what a run reads.
    pub fn root(&self, base: &Path) -> std::path::PathBuf {
        let p = match self {
            Fetch::Folder { path, git, .. } => {
                let named = path.clone().unwrap_or_else(|| {
                    if git.is_some() {
                        "checkout".into()
                    } else {
                        ".".into()
                    }
                });
                return base.join(named);
            }
            Fetch::Csv { path, .. } => path,
            Fetch::Xlsx { path, .. } => path,
            Fetch::Http { .. } | Fetch::Feed { .. } | Fetch::Hub { .. } | Fetch::Package { .. } | Fetch::Sql { .. } | Fetch::Webhook { .. } | Fetch::Proposals { .. } | Fetch::Web { .. } => {
                return base.to_path_buf()
            }
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
        let Fetch::Folder { git: Some(url), .. } = self else {
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

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub every: Option<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Dates {
    #[default]
    MonthFirst,
    DayFirst,
}

impl Dates {
    fn is_month_first(&self) -> bool {
        *self == Dates::MonthFirst
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimsDecl {
    /// Names the list where one fetched thing holds many claims.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub each: Option<String>,
    #[serde(rename = "where", skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    /// One, or several. A GitHub advisory issues a GHSA and names the CVE it is about,
    /// and that cross-reference is what lets two sources meet.
    #[serde(rename = "id", skip_serializing_if = "Option::is_none")]
    pub id_before: Option<Ids>,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub known: Option<String>,
    /// How the source writes 04/08/2026: `day-first` for 4 August, as Britain and most of Europe
    /// do. Without it a slash date is read month first.
    #[serde(default, skip_serializing_if = "Dates::is_month_first")]
    pub dates: Dates,
    /// Rows under one key gathered into one claim, where a list gives one row per name, address
    /// or birth date. Without it the first row under a key is the claim and the rest are counted.
    #[serde(default, skip_serializing_if = "is_false")]
    pub gather: bool,
    /// Which of the gathered rows leads, where the first is not always the one: the row with
    /// the primary name gives the claim its title, the aliases come after.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lead: Option<String>,
    #[serde(default, rename = "properties", skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, PropertySpec>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub from: String,
    #[serde(rename = "match", skip_serializing_if = "Option::is_none")]
    pub matches: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub separator: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub all: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub from: String,
    #[serde(rename = "match", skip_serializing_if = "Option::is_none")]
    pub matches: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub separator: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
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

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PropertyType {
    Text,
    Code,
    Number,
    Bool,
    Date,
    Interval,
}

impl PropertyType {
    pub fn name(self) -> &'static str {
        match self {
            PropertyType::Text => "text",
            PropertyType::Code => "code",
            PropertyType::Number => "number",
            PropertyType::Bool => "bool",
            PropertyType::Date => "date",
            PropertyType::Interval => "interval",
        }
    }
    /// `number` and `date` compare by their own order. `text`, `bool` and `code` compare for
    /// equality: ordering a code needs a scale, and a scale is a tracker's declaration.
    pub fn ordered(self) -> bool {
        matches!(
            self,
            PropertyType::Number | PropertyType::Date | PropertyType::Interval
        )
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PropertySpec {
    #[serde(rename = "type")]
    pub kind: PropertyType,
    /// Only from the rows this holds for: a table that gives one row per country, year and
    /// indicator says inflation in the rows of that indicator, and nothing in the others.
    #[serde(rename = "where", skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vocabulary: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub from: String,
    #[serde(rename = "match", skip_serializing_if = "Option::is_none")]
    pub matches: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub separator: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub all: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

impl PropertySpec {
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub default: bool,
    #[serde(rename = "where", skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facets: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Search {
    /// Which parts of a claim the full-text index covers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text: Vec<String>,
    /// Which fields answer a comparison.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compare: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggest: Vec<String>,
    /// Real queries in this source's own vocabulary, shown on an empty search box.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
}

/// Every version is kept unless a source says otherwise. A change can then say what moved and
/// from what, and every value has a history behind its receipt.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Retention {
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub history: bool,
}

impl Schedule {
    fn is_empty(&self) -> bool {
        self.every.is_none()
    }
}

impl Search {
    fn is_empty(&self) -> bool {
        self.text.is_empty()
            && self.compare.is_empty()
            && self.suggest.is_empty()
            && self.examples.is_empty()
    }
}

impl Retention {
    fn is_empty(&self) -> bool {
        self.history
    }
}

/// The file a source is, inside its directory.
pub const FILE: &str = "source.yaml";

impl SourceDecl {
    pub fn load(dir: &Path) -> Result<SourceDecl, String> {
        let path = dir.join(FILE);
        let mut d: SourceDecl = crate::yaml::read(&path)?;
        d.settle_ids();
        if d.title.is_empty() {
            d.title = d.name.clone();
        }
        // A subscribed source has nothing to extract from, so its specs carry a name and a type
        // and no expression. Everywhere else an absent `from` is a field that would silently
        // produce nothing, and saying so here costs one pass over the declaration.
        if !matches!(d.source, Fetch::Hub { .. } | Fetch::Package { .. }) {
            let mut missing: Vec<String> = Vec::new();
            if d.records.title.trim().is_empty() {
                missing.push("title".into());
            }
            for (name, spec) in &d.records.fields {
                if spec.from.trim().is_empty() {
                    missing.push(name.clone());
                }
            }
            if let Some(ids) = &d.ids() {
                for (i, one) in ids.each().iter().enumerate() {
                    if one.from.trim().is_empty() {
                        missing.push(format!("id[{i}]"));
                    }
                }
            }
            if !missing.is_empty() {
                return Err(format!(
                    "{}: {} says where it comes from with `from`, and does not",
                    path.display(),
                    missing.join(", ")
                ));
            }
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
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    /// Where the next page is named. `link` reads the `Link` header; otherwise the page is
    /// asked for by number or by offset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub offset: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub size: String,
    /// Where the answer says how many there are in total.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<String>,
    /// `offset` counts claims, `page` counts pages from one. Sources do both.
    #[serde(default = "offset_word", skip_serializing_if = "is_offset_word")]
    pub by: String,
    #[serde(default = "hundred", skip_serializing_if = "is_hundred")]
    pub max: usize,
}

fn offset_word() -> String {
    "offset".into()
}

fn is_offset_word(s: &str) -> bool {
    s == offset_word()
}

fn is_hundred(n: &usize) -> bool {
    *n == hundred()
}

fn hundred() -> usize {
    100
}

/// A declaration writes one table or a list of them, and both mean the same thing. The form
/// before `identified_by`: read still, and written as `identified_by` and `refers_to`.
#[derive(Clone, Debug, Deserialize, Serialize)]
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
    /// What names the claim, where the first scheme carries one identifier rather than several.
    pub fn names_record(&self) -> bool {
        self.each().first().map(|s| !s.all).unwrap_or(false)
    }
}

/// A source that takes its things from another one. The named source has to be installed in
/// the same workspace, and a run without it refuses rather than reading nothing.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ForEach {
    #[serde(default, rename = "source", skip_serializing_if = "String::is_empty")]
    pub dataset: String,
    pub scheme: String,
}

impl Fetch {
    /// The one address this source reads, where there is exactly one. A source that pages, or
    /// crawls, or reads a directory has no single thing to ask about, so the answer is `None`
    /// and it is fetched the way it always was.
    pub fn single_url(&self) -> Option<&str> {
        let named = match self {
            Fetch::Csv { path, .. } | Fetch::Xlsx { path, .. } => path.as_str(),
            Fetch::Feed { urls, .. } if urls.len() == 1 => urls[0].as_str(),
            _ => return None,
        };
        named
            .starts_with("http://")
            .then_some(named)
            .or_else(|| named.starts_with("https://").then_some(named))
    }

    /// What to call itself when asking. A source that declares one is asked under it.
    pub fn agent(&self) -> &str {
        match self {
            Fetch::Http { user_agent, .. } | Fetch::Feed { user_agent, .. } | Fetch::Web { user_agent, .. } => user_agent,
            _ => AGENT,
        }
    }
}

impl Fetch {
    /// The source this one takes its things from, where it takes them from one. `models/hf`
    /// asks Hugging Face about the models `models/gguf` names, so it has nothing to do until
    /// that one has found something new.
    pub fn after(&self) -> Option<&str> {
        match self {
            Fetch::Http {
                for_each: Some(f), ..
            } if !f.dataset.is_empty() => Some(&f.dataset),
            _ => None,
        }
    }
}

impl Default for Retention {
    fn default() -> Self {
        Retention { history: true }
    }
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

/// A source's licence, as its publisher states it and the person who declared the source read it.
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Licence {
    /// `yes`: its claims may be shown in full on a public page. `summary`: their titles, values
    /// and a link, not their text. `no`: on no public page at all. Unset is `yes`: a source is
    /// shown until its owner says otherwise (2026-10-09), and [`shown`] says it so.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub republish: String,
    /// Where the publisher says so.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub terms: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// How much of a source a public tracker shows: `yes` (all) or `summary` (titles, values and a
/// link); unset is `yes`, and `no` from before is `summary`.
pub fn shown(republish: &str) -> &str {
    match republish.trim() {
        "summary" | "no" => "summary",
        _ => "yes",
    }
}

/// What the owner said of a source's own page: Some(true) public, Some(false) private, None as its
/// trackers are. `no` from before, with nothing said since, is private.
pub fn page_said(decl: &SourceDecl) -> Option<bool> {
    match decl.visibility.as_str() {
        "public" => Some(true),
        "private" => Some(false),
        _ if decl.licence.republish.trim() == "no" => Some(false),
        _ => None,
    }
}

impl Licence {
    pub fn is_empty(&self) -> bool {
        self.republish.is_empty() && self.terms.is_empty() && self.note.is_empty()
    }
}

/// Where an identifier is written: a column alone (`field:EAN`), or with how to find it in there.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum IdWhere {
    At(String),
    Found {
        from: String,
        #[serde(rename = "match", default, skip_serializing_if = "Option::is_none")]
        matches: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        separator: Option<String>,
    },
}

impl IdWhere {
    fn spec(&self, scheme: &str, all: bool) -> IdSpec {
        let (from, matches, separator) = match self {
            IdWhere::At(from) => (from.clone(), None, None),
            IdWhere::Found { from, matches, separator } => (from.clone(), matches.clone(), separator.clone()),
        };
        IdSpec { scheme: Some(scheme.to_string()), from, matches, separator, all }
    }
    fn of(spec: &IdSpec) -> IdWhere {
        if spec.matches.is_none() && spec.separator.is_none() {
            IdWhere::At(spec.from.clone())
        } else {
            IdWhere::Found { from: spec.from.clone(), matches: spec.matches.clone(), separator: spec.separator.clone() }
        }
    }
}

impl SourceDecl {
    /// The identifiers, however the file wrote them: what names the record first, then what it
    /// refers to.
    pub fn ids(&self) -> Option<Ids> {
        if self.identified_by.is_empty() && self.refers_to.is_empty() {
            return self.records.id_before.clone();
        }
        let mut all: Vec<IdSpec> = self.identified_by.iter().map(|(s, w)| w.spec(s, false)).collect();
        all.extend(self.refers_to.iter().map(|(s, w)| w.spec(s, true)));
        Some(if all.len() == 1 { Ids::One(all.remove(0)) } else { Ids::Many(all) })
    }

    /// The form before `identified_by`, written in the new one where it says the same: the first
    /// identifier names the record, and every one after it is several each. One that does not fit
    /// (no scheme, or a second that names the record) stays as it was written.
    pub fn settle_ids(&mut self) {
        let Some(ids) = &self.records.id_before else { return };
        if !self.identified_by.is_empty() || !self.refers_to.is_empty() {
            return;
        }
        let each = ids.each();
        let fits = each.iter().all(|s| s.scheme.is_some())
            && each.iter().enumerate().all(|(i, s)| (i == 0) || s.all);
        if !fits {
            return;
        }
        let (mut by, mut to) = (BTreeMap::new(), BTreeMap::new());
        for (i, s) in each.iter().enumerate() {
            let scheme = s.scheme.clone().unwrap_or_default();
            if i == 0 && !s.all {
                by.insert(scheme, IdWhere::of(s));
            } else {
                to.insert(scheme, IdWhere::of(s));
            }
        }
        self.identified_by = by;
        self.refers_to = to;
        self.records.id_before = None;
    }
}

fn an_item() -> String {
    "item".into()
}
