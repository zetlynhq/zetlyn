//! What somebody runs, checked against what a tracker's sources say is affected and fixed: an
//! SBOM, `rpm -qa`, a list of package URLs or CPEs. Nothing of it is kept. It is read, matched
//! and answered, and the answer is the page.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde_json::Value as J;

/// One thing somebody runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// An RPM by name, epoch, version and release.
    Rpm { name: String, epoch: u64, version: String, release: String },
    /// A package as GitHub's advisories name its ecosystem: npm, pip, maven, go, rubygems,
    /// nuget, composer, rust, erlang, pub, swift. Empty where the list did not say.
    Package { ecosystem: String, name: String, version: String },
    /// A product as vendor/product, as a CPE spells it, and its version where it says one.
    Product { product: String, version: String },
}

impl Item {
    pub fn label(&self) -> String {
        match self {
            Item::Rpm { name, .. } => name.clone(),
            Item::Package { ecosystem, name, .. } if !ecosystem.is_empty() => format!("{name} ({ecosystem})"),
            Item::Package { name, .. } => name.clone(),
            Item::Product { product, .. } => product.replace('_', " "),
        }
    }

    pub fn version(&self) -> String {
        match self {
            Item::Rpm { epoch, version, release, .. } if *epoch > 0 => format!("{epoch}:{version}-{release}"),
            Item::Rpm { version, release, .. } => format!("{version}-{release}"),
            Item::Package { version, .. } | Item::Product { version, .. } => version.clone(),
        }
    }
}

/// What was read: the items, the lines that were not, and what the list looked like.
#[derive(Debug, Default)]
pub struct Read {
    pub items: Vec<Item>,
    pub unread: Vec<String>,
    pub format: &'static str,
}

/// Reads whatever was pasted: CycloneDX or SPDX as JSON, or lines of `rpm -qa`, package URLs,
/// CPEs, `name==version`, `name@version` or `name version`.
pub fn read(text: &str) -> Read {
    let text = text.trim();
    if text.starts_with('{') {
        if let Ok(doc) = serde_json::from_str::<J>(text) {
            let mut out = Read::default();
            if doc.get("bomFormat").and_then(J::as_str) == Some("CycloneDX") || doc.get("components").is_some() {
                out.format = "CycloneDX";
                cyclonedx(&doc["components"], &mut out.items);
            } else if doc.get("spdxVersion").is_some() || doc.get("packages").is_some() {
                out.format = "SPDX";
                for p in doc["packages"].as_array().into_iter().flatten() {
                    let refs = p["externalRefs"].as_array().cloned().unwrap_or_default();
                    let locator = |kind: &str| refs.iter().find(|r| r["referenceType"].as_str() == Some(kind)).and_then(|r| r["referenceLocator"].as_str()).map(str::to_string);
                    let item = locator("purl").and_then(|u| purl(&u)).or_else(|| locator("cpe23Type").and_then(|c| cpe(&c))).or_else(|| {
                        let name = p["name"].as_str()?.to_string();
                        Some(Item::Package { ecosystem: String::new(), name, version: p["versionInfo"].as_str().unwrap_or("").to_string() })
                    });
                    out.items.extend(item);
                }
            } else {
                out.format = "JSON";
                out.unread.push("a JSON document that is neither CycloneDX nor SPDX".into());
            }
            return out;
        }
    }
    let mut out = Read { format: "lines", ..Read::default() };
    let (mut rpms, mut others) = (0, 0);
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("Desired=") || line.starts_with("||/") || line.starts_with("+++-") {
            continue;
        }
        match line_item(line) {
            Some(item) => {
                if matches!(item, Item::Rpm { .. }) { rpms += 1 } else { others += 1 }
                out.items.push(item);
            }
            None => out.unread.push(line.to_string()),
        }
    }
    if rpms > 0 && others == 0 {
        out.format = "rpm -qa";
    }
    out
}

fn cyclonedx(components: &J, out: &mut Vec<Item>) {
    for c in components.as_array().into_iter().flatten() {
        let item = c["purl"].as_str().and_then(purl).or_else(|| c["cpe"].as_str().and_then(cpe)).or_else(|| {
            let name = c["name"].as_str()?;
            let name = match c["group"].as_str().filter(|g| !g.is_empty()) {
                Some(g) => format!("{g}:{name}"),
                None => name.to_string(),
            };
            Some(Item::Package { ecosystem: String::new(), name, version: c["version"].as_str().unwrap_or("").to_string() })
        });
        out.extend(item);
        cyclonedx(&c["components"], out);
    }
}

fn line_item(line: &str) -> Option<Item> {
    if line.starts_with("pkg:") {
        return purl(line);
    }
    if line.starts_with("cpe:") {
        return cpe(line);
    }
    // `dpkg -l`: ii, the name, the version, the architecture, the description.
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.len() >= 3 && matches!(words[0], "ii" | "hi" | "rc" | "iU") {
        let name = words[1].split(':').next().unwrap_or(words[1]);
        return Some(Item::Package { ecosystem: String::new(), name: name.to_string(), version: words[2].to_string() });
    }
    if words.len() == 1 {
        if let Some(r) = rpm_installed(line) {
            return Some(r);
        }
        // requirements.txt, pinned.
        if let Some((name, version)) = line.split_once("==") {
            let name = name.split('[').next().unwrap_or(name).trim();
            return Some(Item::Package { ecosystem: "pip".into(), name: pip_name(name), version: version.split(';').next().unwrap_or("").trim().to_string() });
        }
        // npm's `name@version`, a scoped name keeping its own @.
        if let Some((name, version)) = line.rsplit_once('@').filter(|(n, v)| !n.is_empty() && !v.is_empty()) {
            return Some(Item::Package { ecosystem: String::new(), name: name.to_string(), version: version.to_string() });
        }
        return None;
    }
    // `name version`, as `dpkg-query -W`, `pip list` or a list written by hand gives it.
    if words.len() == 2 && words[1].chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return Some(Item::Package { ecosystem: String::new(), name: words[0].to_string(), version: words[1].to_string() });
    }
    None
}

fn pip_name(name: &str) -> String {
    name.to_lowercase().replace('_', "-").replace('.', "-")
}

const ARCHES: [&str; 10] = ["x86_64", "noarch", "aarch64", "i686", "i386", "ppc64le", "s390x", "armv7hl", "src", "ppc64"];

/// `openssl-libs-3.0.7-27.el9.x86_64`, or with its epoch, `openssl-libs-1:3.0.7-27.el9.x86_64`:
/// a line of `rpm -qa`. It must end in an architecture, so `foo-1.2-3` written by hand is not one.
fn rpm_installed(line: &str) -> Option<Item> {
    let (rest, arch) = line.rsplit_once('.')?;
    if !ARCHES.contains(&arch) {
        return None;
    }
    rpm_nevr(rest)
}

/// `name-[epoch:]version-release`, as Red Hat names a package a fix shipped in.
pub fn rpm_nevr(s: &str) -> Option<Item> {
    let (rest, release) = s.rsplit_once('-')?;
    let (name, ev) = rest.rsplit_once('-')?;
    let (epoch, version) = match ev.split_once(':') {
        Some((e, v)) => (e.parse().ok()?, v),
        None => (0, ev),
    };
    if name.is_empty() || version.is_empty() || release.is_empty() || !version.chars().next()?.is_ascii_alphanumeric() {
        return None;
    }
    Some(Item::Rpm { name: name.to_string(), epoch, version: version.to_string(), release: release.to_string() })
}

/// A package URL: `pkg:npm/%40scope/name@1.2.3`, `pkg:maven/org.apache/commons@1.0`,
/// `pkg:rpm/redhat/openssl@3.0.7-27.el9?epoch=1`.
pub fn purl(s: &str) -> Option<Item> {
    let s = s.strip_prefix("pkg:")?;
    let (s, qualifiers) = match s.split_once('?') {
        Some((a, q)) => (a, q.split('#').next().unwrap_or("")),
        None => (s.split('#').next().unwrap_or(s), ""),
    };
    let (path, version) = match s.rsplit_once('@') {
        Some((p, v)) if !p.ends_with('/') => (p, decode(v)),
        _ => (s, String::new()),
    };
    let (kind, rest) = path.split_once('/')?;
    let kind = kind.to_lowercase();
    let (namespace, name) = match rest.rsplit_once('/') {
        Some((ns, n)) => (decode(ns), decode(n)),
        None => (String::new(), decode(rest)),
    };
    if kind == "rpm" {
        let epoch = qualifiers.split('&').find_map(|q| q.strip_prefix("epoch=")).and_then(|e| e.parse().ok()).unwrap_or(0);
        let (v, r) = version.rsplit_once('-').map(|(v, r)| (v.to_string(), r.to_string())).unwrap_or((version.clone(), String::new()));
        return Some(Item::Rpm { name, epoch, version: v, release: r });
    }
    let (ecosystem, name) = match kind.as_str() {
        "npm" => ("npm", if namespace.is_empty() { name } else { format!("{namespace}/{name}") }),
        "pypi" => ("pip", pip_name(&name)),
        "maven" => ("maven", if namespace.is_empty() { name } else { format!("{namespace}:{name}") }),
        "golang" => ("go", if namespace.is_empty() { name } else { format!("{namespace}/{name}") }),
        "gem" => ("rubygems", name),
        "nuget" => ("nuget", name),
        "composer" => ("composer", if namespace.is_empty() { name } else { format!("{namespace}/{name}") }),
        "cargo" => ("rust", name),
        "hex" => ("erlang", name),
        "pub" => ("pub", name),
        "swift" => ("swift", if namespace.is_empty() { name } else { format!("{namespace}/{name}") }),
        "github" => ("actions", if namespace.is_empty() { name } else { format!("{namespace}/{name}") }),
        _ => ("", if namespace.is_empty() { name } else { format!("{namespace}/{name}") }),
    };
    Some(Item::Package { ecosystem: ecosystem.to_string(), name, version })
}

fn decode(s: &str) -> String {
    crate::serve::urldecode(s)
}

/// `cpe:2.3:a:apache:http_server:2.4.57:*:…` or `cpe:/a:apache:http_server:2.4.57`.
pub fn cpe(s: &str) -> Option<Item> {
    let parts: Vec<&str> = if let Some(rest) = s.strip_prefix("cpe:2.3:") {
        rest.split(':').collect()
    } else {
        s.strip_prefix("cpe:/")?.split(':').collect()
    };
    let (vendor, product) = (parts.get(1)?, parts.get(2)?);
    if vendor.is_empty() || product.is_empty() || *vendor == "*" {
        return None;
    }
    let version = parts.get(3).filter(|v| !matches!(**v, "*" | "-" | "")).map(|v| v.to_string()).unwrap_or_default();
    Some(Item::Product { product: format!("{}/{}", vendor.to_lowercase(), product.to_lowercase()), version })
}

// -- comparing versions ---------------------------------------------------------------------

/// rpm's own comparison of two versions or two releases: runs of digits as numbers, runs of
/// letters as text, a digit run newer than a letter run, `~` older than anything, `^` newer.
pub fn rpmvercmp(a: &str, b: &str) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    let (mut x, mut y) = (a.as_bytes(), b.as_bytes());
    loop {
        // Separators are skipped, except the two that order.
        while let Some(&c) = x.first() { if c.is_ascii_alphanumeric() || c == b'~' || c == b'^' { break } x = &x[1..]; }
        while let Some(&c) = y.first() { if c.is_ascii_alphanumeric() || c == b'~' || c == b'^' { break } y = &y[1..]; }
        match (x.first(), y.first()) {
            (Some(b'~'), Some(b'~')) => { x = &x[1..]; y = &y[1..]; continue }
            (Some(b'~'), _) => return Ordering::Less,
            (_, Some(b'~')) => return Ordering::Greater,
            (Some(b'^'), Some(b'^')) => { x = &x[1..]; y = &y[1..]; continue }
            (Some(b'^'), None) => return Ordering::Greater,
            (None, Some(b'^')) => return Ordering::Less,
            (Some(b'^'), _) => return Ordering::Less,
            (_, Some(b'^')) => return Ordering::Greater,
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            _ => {}
        }
        let digits = x[0].is_ascii_digit();
        let run = |s: &[u8]| s.iter().take_while(|c| if digits { c.is_ascii_digit() } else { c.is_ascii_alphabetic() }).count();
        let (nx, ny) = (run(x), run(y));
        if ny == 0 {
            // A number against letters: the number is newer.
            return if digits { Ordering::Greater } else { Ordering::Less };
        }
        let (sx, sy) = (&x[..nx], &y[..ny]);
        let o = if digits {
            let (tx, ty) = (trim_zeros(sx), trim_zeros(sy));
            tx.len().cmp(&ty.len()).then_with(|| tx.cmp(ty))
        } else {
            sx.cmp(sy)
        };
        if o != Ordering::Equal {
            return o;
        }
        x = &x[nx..];
        y = &y[ny..];
    }
}

fn trim_zeros(s: &[u8]) -> &[u8] {
    let n = s.iter().take_while(|c| **c == b'0').count();
    &s[n..]
}

/// Epoch, then version, then release.
pub fn evr_cmp(a: (u64, &str, &str), b: (u64, &str, &str)) -> Ordering {
    a.0.cmp(&b.0).then_with(|| rpmvercmp(a.1, b.1)).then_with(|| rpmvercmp(a.2, b.2))
}

/// The Red Hat stream a release is built for: `27.el9_2` is 9, minor 2; `27.el9` is 9 alone.
fn stream(release: &str) -> Option<(u32, Option<u32>)> {
    let at = release.find(".el")? + 3;
    let rest = &release[at..];
    let major: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let after = &rest[major.len()..];
    let minor = after.strip_prefix('_').map(|m| m.chars().take_while(|c| c.is_ascii_digit()).collect::<String>()).and_then(|m| m.parse().ok());
    Some((major.parse().ok()?, minor))
}

/// Two versions as the ecosystems write them: numbers as numbers, and a pre-release
/// (`1.0.0-rc.1`, `2.0b1`) older than the release it comes before.
pub fn version_cmp(a: &str, b: &str) -> Ordering {
    let pieces = |s: &str| -> Vec<(bool, String)> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.chars() {
            if c.is_ascii_alphanumeric() {
                let digit = c.is_ascii_digit();
                match out.last_mut() {
                    Some((d, run)) if *d == digit && !run.is_empty() => run.push(c.to_ascii_lowercase()),
                    _ => out.push((digit, c.to_ascii_lowercase().to_string())),
                }
            } else {
                out.push((true, String::new()));
            }
        }
        out.retain(|(_, r)| !r.is_empty());
        out
    };
    let (x, y) = (pieces(a), pieces(b));
    for i in 0..x.len().max(y.len()) {
        match (x.get(i), y.get(i)) {
            (Some((dx, rx)), Some((dy, ry))) => {
                let o = match (dx, dy) {
                    (true, true) => {
                        let (tx, ty) = (rx.trim_start_matches('0'), ry.trim_start_matches('0'));
                        tx.len().cmp(&ty.len()).then_with(|| tx.cmp(ty))
                    }
                    // A number where the other has letters: the other is a pre-release here.
                    (true, false) => Ordering::Greater,
                    (false, true) => Ordering::Less,
                    (false, false) => rx.cmp(ry),
                };
                if o != Ordering::Equal {
                    return o;
                }
            }
            // One ends: letters after it on the other side are a pre-release, numbers are newer.
            (None, Some((d, _))) => return if *d { Ordering::Less } else { Ordering::Greater },
            (Some((d, _)), None) => return if *d { Ordering::Greater } else { Ordering::Less },
            (None, None) => break,
        }
    }
    Ordering::Equal
}

/// Whether a version is in a range as GitHub writes one: `< 4.17.21`, `>= 2.0.0, < 2.3.1`, `= 1.2`.
pub fn in_range(version: &str, range: &str) -> Option<bool> {
    let mut any = false;
    for clause in range.split(',') {
        let clause = clause.trim();
        if clause.is_empty() {
            continue;
        }
        let (op, v) = ["<=", ">=", "<", ">", "="]
            .iter()
            .find_map(|op| clause.strip_prefix(op).map(|v| (*op, v.trim())))?;
        let o = version_cmp(version, v);
        let holds = match op {
            "<" => o == Ordering::Less,
            "<=" => o != Ordering::Greater,
            ">" => o == Ordering::Greater,
            ">=" => o != Ordering::Less,
            _ => o == Ordering::Equal,
        };
        if !holds {
            return Some(false);
        }
        any = true;
    }
    any.then_some(true)
}

// -- matching -------------------------------------------------------------------------------

/// One thing somebody runs that a thing of the tracker is about.
#[derive(Clone, Debug)]
pub struct Finding {
    /// Which item of the inventory.
    pub item: usize,
    /// The thing, `cve:cve-2024-3400`.
    pub key: String,
    /// The version with the fix, where the source names one.
    pub fixed: String,
    /// True where the version was compared and is below the fix; false where the source names
    /// the product but not which of its versions, and a person has to look.
    pub below_fix: bool,
    /// Which source said so.
    pub source: String,
}

/// What a tracker's store says each package or product is affected by, read once per check.
pub struct Known {
    /// RPM name → (thing, source, epoch, version, release) of each fix.
    rpm: BTreeMap<String, Vec<(String, String, u64, String, String)>>,
    /// Package name, lower case → (thing, source, ecosystem, range, fix).
    packages: BTreeMap<String, Vec<(String, String, String, String, String)>>,
    /// Relation to products, for a CPE.
    products: String,
}

impl Known {
    pub fn of(store: &crate::thingstore::ThingStore, decl: &crate::trackerdecl::Inventory) -> Known {
        let mut rpm: BTreeMap<String, Vec<(String, String, u64, String, String)>> = BTreeMap::new();
        if !decl.rpm.is_empty() {
            for (key, source, words) in store.said_all(&decl.rpm) {
                for w in words {
                    if let Some(Item::Rpm { name, epoch, version, release }) = rpm_nevr(&w) {
                        rpm.entry(name).or_default().push((key.clone(), source.clone(), epoch, version, release));
                    }
                }
            }
        }
        let mut packages: BTreeMap<String, Vec<(String, String, String, String, String)>> = BTreeMap::new();
        if !decl.packages.is_empty() {
            for (key, source, words) in store.said_all(&decl.packages) {
                for w in words {
                    // `npm lodash < 4.17.21; fixed in 4.17.21`
                    let (head, fix) = w.split_once("; fixed in").map(|(h, f)| (h, f.trim())).unwrap_or((w.as_str(), ""));
                    let mut parts = head.splitn(3, ' ');
                    let (Some(eco), Some(name)) = (parts.next(), parts.next()) else { continue };
                    let range = parts.next().unwrap_or("").trim();
                    packages.entry(name.to_lowercase()).or_default().push((key.clone(), source.clone(), eco.to_string(), range.to_string(), fix.to_string()));
                }
            }
        }
        Known { rpm, packages, products: decl.products.clone() }
    }

    /// The fixes for an installed package. Red Hat names the package a fix shipped in by its
    /// source, `openssl`, and a machine has its parts, `openssl-libs`: a part is taken as its
    /// source only where a fix of that source has the very version installed, so `python3-requests`
    /// is never read as `python3`.
    fn rpm_fixes(&self, name: &str, version: &str) -> Option<&Vec<(String, String, u64, String, String)>> {
        if let Some(fixes) = self.rpm.get(name) {
            return Some(fixes);
        }
        let mut shorter = name;
        while let Some((head, _)) = shorter.rsplit_once('-') {
            if let Some(fixes) = self.rpm.get(head) {
                return fixes.iter().any(|f| f.3 == version).then_some(fixes);
            }
            shorter = head;
        }
        None
    }

    /// Every finding for every item.
    pub fn check(&self, store: &crate::thingstore::ThingStore, items: &[Item]) -> Vec<Finding> {
        let mut out = Vec::new();
        for (i, item) in items.iter().enumerate() {
            match item {
                Item::Rpm { name, epoch, version, release } => {
                    let Some(fixes) = self.rpm_fixes(name, version) else { continue };
                    let mine = stream(release);
                    let mut by_thing: BTreeMap<&str, Vec<&(String, String, u64, String, String)>> = BTreeMap::new();
                    for f in fixes {
                        by_thing.entry(f.0.as_str()).or_default().push(f);
                    }
                    for (key, fixes) in by_thing {
                        // The fix for this stream: the same minor where one was shipped for it,
                        // else the newest for the same major. Another major is not this system's.
                        let same_major: Vec<_> = fixes.iter().filter(|f| stream(&f.4).map(|s| s.0) == mine.map(|m| m.0)).collect();
                        let exact = same_major.iter().find(|f| mine.is_some_and(|m| m.1.is_some() && stream(&f.4) == Some(m)));
                        let chosen = exact.copied().or_else(|| {
                            same_major.iter().copied().max_by(|a, b| evr_cmp((a.2, &a.3, &a.4), (b.2, &b.3, &b.4)))
                        });
                        let Some(fix) = chosen else { continue };
                        if evr_cmp((*epoch, version, release), (fix.2, &fix.3, &fix.4)) == Ordering::Less {
                            let shown = if fix.2 > 0 { format!("{}:{}-{}", fix.2, fix.3, fix.4) } else { format!("{}-{}", fix.3, fix.4) };
                            out.push(Finding { item: i, key: key.to_string(), fixed: shown, below_fix: true, source: fix.1.clone() });
                        }
                    }
                }
                Item::Package { ecosystem, name, version } => {
                    let Some(advised) = self.packages.get(&name.to_lowercase()) else { continue };
                    for (key, source, eco, range, fix) in advised {
                        if !ecosystem.is_empty() && !eco.eq_ignore_ascii_case(ecosystem) {
                            continue;
                        }
                        if version.is_empty() {
                            out.push(Finding { item: i, key: key.clone(), fixed: fix.clone(), below_fix: false, source: source.clone() });
                        } else if in_range(version, range) == Some(true) {
                            out.push(Finding { item: i, key: key.clone(), fixed: fix.clone(), below_fix: true, source: source.clone() });
                        }
                    }
                }
                Item::Product { product, .. } => {
                    if self.products.is_empty() {
                        continue;
                    }
                    for (key, sources) in store.related_to(&self.products, product) {
                        out.push(Finding { item: i, key, fixed: String::new(), below_fix: false, source: sources });
                    }
                }
            }
        }
        // One finding per item and thing.
        out.sort_by(|a, b| (a.item, &a.key).cmp(&(b.item, &b.key)).then(b.below_fix.cmp(&a.below_fix)));
        out.dedup_by(|a, b| a.item == b.item && a.key == b.key);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpm_orders_versions_as_rpm_does() {
        assert_eq!(rpmvercmp("1.0", "1.0"), Ordering::Equal);
        assert_eq!(rpmvercmp("1.0", "1.0.1"), Ordering::Less);
        assert_eq!(rpmvercmp("2.10", "2.9"), Ordering::Greater);
        assert_eq!(rpmvercmp("1.0~rc1", "1.0"), Ordering::Less);
        assert_eq!(rpmvercmp("1.0a", "1.0"), Ordering::Greater);
        assert_eq!(rpmvercmp("9.el9", "9.1.el9_7"), Ordering::Less);
        assert_eq!(rpmvercmp("284.11.1.el9_2", "284.30.1.el9_2"), Ordering::Less);
        assert_eq!(evr_cmp((1, "1.0", "1"), (0, "9.9", "9")), Ordering::Greater);
    }

    #[test]
    fn ecosystems_order_versions_with_prereleases_first() {
        assert_eq!(version_cmp("4.17.20", "4.17.21"), Ordering::Less);
        assert_eq!(version_cmp("4.0.0-canary.33", "4.0.0-canary.34"), Ordering::Less);
        assert_eq!(version_cmp("4.0.0-rc.1", "4.0.0"), Ordering::Less);
        assert_eq!(version_cmp("1.10", "1.9"), Ordering::Greater);
        assert_eq!(version_cmp("v2.0", "2.0"), Ordering::Equal);
        assert_eq!(in_range("3.89.9", "< 3.90.0"), Some(true));
        assert_eq!(in_range("3.90.0", "< 3.90.0"), Some(false));
        assert_eq!(in_range("2.1", ">= 2.0.0, < 2.3.1"), Some(true));
        assert_eq!(in_range("1.9", ">= 2.0.0, < 2.3.1"), Some(false));
        assert_eq!(in_range("1.0", "nonsense"), None);
    }

    #[test]
    fn every_kind_of_list_is_read() {
        let lines = "openssl-libs-3.0.7-27.el9.x86_64\nkernel-core-5.14.0-284.11.1.el9_2.x86_64\nperl-XML-Parser-2.46-9.el9.noarch\n";
        let r = read(lines);
        assert_eq!(r.format, "rpm -qa");
        assert_eq!(r.items[0], Item::Rpm { name: "openssl-libs".into(), epoch: 0, version: "3.0.7".into(), release: "27.el9".into() });
        assert_eq!(r.items.len(), 3);

        let mixed = read("pkg:npm/%40babel/core@7.0.0\npkg:maven/org.apache.logging.log4j/log4j-core@2.14.1\ncpe:2.3:a:apache:http_server:2.4.57:*:*:*:*:*:*:*\nDjango==4.2.1\nlodash@4.17.20\nnot a package at all here\n");
        assert_eq!(mixed.items[0], Item::Package { ecosystem: "npm".into(), name: "@babel/core".into(), version: "7.0.0".into() });
        assert_eq!(mixed.items[1], Item::Package { ecosystem: "maven".into(), name: "org.apache.logging.log4j:log4j-core".into(), version: "2.14.1".into() });
        assert_eq!(mixed.items[2], Item::Product { product: "apache/http_server".into(), version: "2.4.57".into() });
        assert_eq!(mixed.items[3], Item::Package { ecosystem: "pip".into(), name: "django".into(), version: "4.2.1".into() });
        assert_eq!(mixed.items[4], Item::Package { ecosystem: String::new(), name: "lodash".into(), version: "4.17.20".into() });
        assert_eq!(mixed.unread, vec!["not a package at all here".to_string()]);

        let bom = r#"{"bomFormat":"CycloneDX","components":[{"name":"lodash","version":"4.17.20","purl":"pkg:npm/lodash@4.17.20"},{"group":"com.fasterxml","name":"jackson","version":"2.0","components":[{"name":"inner","version":"1"}]}]}"#;
        let r = read(bom);
        assert_eq!(r.format, "CycloneDX");
        assert_eq!(r.items.len(), 3);
        assert_eq!(r.items[1], Item::Package { ecosystem: String::new(), name: "com.fasterxml:jackson".into(), version: "2.0".into() });

        let spdx = r#"{"spdxVersion":"SPDX-2.3","packages":[{"name":"x","versionInfo":"1","externalRefs":[{"referenceType":"purl","referenceLocator":"pkg:pypi/Requests@2.0"}]}]}"#;
        assert_eq!(read(spdx).items[0], Item::Package { ecosystem: "pip".into(), name: "requests".into(), version: "2.0".into() });
    }


    #[test]
    fn an_inventory_is_checked_against_what_the_store_holds() {
        let dir = std::env::temp_dir().join(format!("zetlyn-inventory-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = crate::thingstore::ThingStore::open(&dir).unwrap();
        let said = |key: &str, source: &str, property: &str, words: &[&str]| {
            let raw = serde_json::to_string(words).unwrap();
            store.db.execute("insert into said(key, source, property, raw, means, kind, understood) values (?1, ?2, ?3, ?4, ?4, 'text', 1)", [key, source, property, raw.as_str()]).unwrap();
        };
        said("cve:cve-1", "zetlyn/cve-redhat", "packages", &["openssl-1:3.0.7-27.el9", "openssl-1:3.0.7-25.el9_4.2", "openssl-1:1.1.1k-12.el8_9"]);
        said("cve:cve-2", "zetlyn/cve-ghsa", "affected", &["npm lodash < 4.17.21; fixed in 4.17.21", "pip django >= 4.2, < 4.2.8; fixed in 4.2.8"]);
        store.db.execute("insert into related(key, name, target, sources) values ('cve:cve-3', 'affects', 'apache/http_server', '[\"zetlyn/cve-nvd\"]')", []).unwrap();
        let decl = crate::trackerdecl::Inventory { rpm: "packages".into(), packages: "affected".into(), products: "affects".into(), about: String::new() };
        let known = Known::of(&store, &decl);
        let items = read("openssl-libs-1:3.0.7-24.el9.x86_64\nopenssl-1:3.0.7-25.el9_4.2.x86_64\npython3-requests-2.25.1-8.el9.noarch\npkg:npm/lodash@4.17.20\npkg:npm/lodash@4.17.21\nDjango==4.2.1\nDjango==5.0\ncpe:2.3:a:apache:http_server:2.4.57:*:*:*:*:*:*:*\n").items;
        let found: Vec<(usize, String, String, bool)> = known.check(&store, &items).into_iter().map(|f| (f.item, f.key, f.fixed, f.below_fix)).collect();
        let want = |i: usize, k: &str, f: &str, b: bool| (i, k.to_string(), f.to_string(), b);
        assert_eq!(found, vec![
            // A part of openssl, of el9 and older than the newest el9 fix.
            want(0, "cve:cve-1", "1:3.0.7-27.el9", true),
            // An EUS 9.4 machine at its own stream's fix is not below it.
            want(3, "cve:cve-2", "4.17.21", true),
            want(5, "cve:cve-2", "4.2.8", true),
            want(7, "cve:cve-3", "", false),
        ]);
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn a_red_hat_fix_is_taken_for_the_same_stream() {
        assert_eq!(stream("9.el9"), Some((9, None)));
        assert_eq!(stream("9.el9_4.1"), Some((9, Some(4))));
        assert_eq!(stream("10.el7_9.1"), Some((7, Some(9))));
        assert_eq!(rpm_nevr("perl-XML-Parser-0:2.46-9.el9_4.1"), Some(Item::Rpm { name: "perl-XML-Parser".into(), epoch: 0, version: "2.46".into(), release: "9.el9_4.1".into() }));
    }
}
