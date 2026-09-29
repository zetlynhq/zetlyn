//! The identifier schemes Zetlyn knows without being told: a pattern, a check where the standard
//! has one, a normalisation, and an example to show a person.
//!
//! Two sources meet on an identifier, so finding one is the first thing a new source needs, and
//! most of the world's are well defined. A column of `CVE-2021-44228` is a CVE column whatever it
//! is called, and a column of thirteen digits is an ISBN column only where the check digit says so.
//! None of this needs a model.

use std::sync::OnceLock;

use regex::Regex;

pub struct Scheme {
    /// What a declaration calls it, and the first half of a thing's key.
    pub name: &'static str,
    pub title: &'static str,
    /// One identifier, unanchored. Case is ignored where the standard ignores it.
    pub pattern: &'static str,
    pub example: &'static str,
    check: fn(&str) -> bool,
    normalise: fn(&str) -> String,
}

fn any(_: &str) -> bool {
    true
}
fn lower(s: &str) -> String {
    s.trim().to_lowercase()
}

pub const ALL: &[Scheme] = &[
    Scheme {
        name: "cve",
        title: "CVE",
        pattern: r"(?i)\bCVE-\d{4}-\d{4,7}\b",
        example: "CVE-2021-44228",
        check: any,
        normalise: lower,
    },
    Scheme {
        name: "ghsa",
        title: "GitHub Security Advisory",
        pattern: r"(?i)\bGHSA(?:-[23456789cfghjmpqrvwx]{4}){3}\b",
        example: "GHSA-jfh8-c2jp-5v3q",
        check: any,
        normalise: lower,
    },
    Scheme {
        name: "cwe",
        title: "CWE",
        pattern: r"(?i)\bCWE-\d{1,4}\b",
        example: "CWE-79",
        check: any,
        normalise: lower,
    },
    Scheme {
        name: "cpe",
        title: "CPE",
        pattern: r"(?i)\bcpe:(?:2\.3:[aho*-](?::[^:\s]+){10}|/[aho](?::[^:\s]*){1,6})",
        example: "cpe:2.3:a:apache:log4j:2.14.1:*:*:*:*:*:*:*",
        check: any,
        normalise: lower,
    },
    Scheme {
        name: "purl",
        title: "Package URL",
        pattern: r"\bpkg:[a-zA-Z][a-zA-Z0-9.+-]*/[^\s,;]+",
        example: "pkg:npm/lodash@4.17.21",
        check: any,
        // The type and namespace are case-insensitive by the spec; a name mostly is in practice.
        normalise: lower,
    },
    Scheme {
        name: "doi",
        title: "DOI",
        pattern: r"(?i)\b10\.\d{4,9}/[^\s,;<>]+",
        example: "10.1038/nature12373",
        check: any,
        normalise: doi,
    },
    Scheme {
        name: "arxiv",
        title: "arXiv",
        pattern: r"(?i)(?:\barXiv:)?\b\d{4}\.\d{4,5}(?:v\d+)?\b",
        example: "2106.09685",
        check: arxiv_month,
        normalise: arxiv,
    },
    Scheme {
        name: "isbn",
        title: "ISBN",
        pattern: r"(?i)\b(?:97[89][- ]?)?\d{1,5}[- ]?\d{1,7}[- ]?\d{1,7}[- ]?[\dX]\b",
        example: "978-3-16-148410-0",
        check: isbn_ok,
        normalise: isbn,
    },
    Scheme {
        name: "issn",
        title: "ISSN",
        pattern: r"(?i)\b\d{4}-\d{3}[\dX]\b",
        example: "0317-8471",
        check: issn_ok,
        normalise: lower,
    },
    Scheme {
        name: "orcid",
        title: "ORCID",
        pattern: r"(?i)\b\d{4}-\d{4}-\d{4}-\d{3}[\dX]\b",
        example: "0000-0002-1825-0097",
        check: orcid_ok,
        normalise: orcid,
    },
    Scheme {
        name: "lei",
        title: "Legal Entity Identifier",
        pattern: r"\b[A-Z0-9]{18}\d{2}\b",
        example: "5493001KJTIIGC8Y1R12",
        check: lei_ok,
        normalise: lower,
    },
    Scheme {
        name: "isin",
        title: "ISIN",
        pattern: r"\b[A-Z]{2}[A-Z0-9]{9}\d\b",
        example: "US0378331005",
        check: isin_ok,
        normalise: lower,
    },
    Scheme {
        name: "celex",
        title: "CELEX",
        pattern: r"\b[0-9CE]\d{4}[A-Z]{1,2}\d{4}(?:\(\d+\))?\b",
        example: "32016R0679",
        check: any,
        normalise: lower,
    },
    Scheme {
        name: "iso",
        title: "ISO standard",
        pattern: r"(?i)\bISO(?:/IEC)?(?:/IEEE)? ?\d{1,5}(?:-\d+)*(?::\d{4})?\b",
        example: "ISO/IEC 27001:2022",
        check: any,
        normalise: iso,
    },
    Scheme {
        name: "rfc",
        title: "IETF RFC",
        pattern: r"(?i)\bRFC ?\d{1,5}\b",
        example: "RFC 9110",
        check: any,
        normalise: rfc,
    },
];

fn compiled() -> &'static Vec<(Regex, Regex)> {
    static RE: OnceLock<Vec<(Regex, Regex)>> = OnceLock::new();
    RE.get_or_init(|| {
        ALL.iter()
            .map(|s| {
                (
                    Regex::new(s.pattern).expect("a scheme's pattern"),
                    Regex::new(&whole(s)).expect("a scheme's whole pattern"),
                )
            })
            .collect()
    })
}

/// The pattern as a whole value, which is what a declaration's `match` holds.
fn whole(s: &Scheme) -> String {
    match s.pattern.strip_prefix("(?i)") {
        Some(rest) => format!("(?i)^(?:{rest})$"),
        None => format!("^(?:{})$", s.pattern),
    }
}

pub fn named(name: &str) -> Option<&'static Scheme> {
    ALL.iter().find(|s| s.name == name)
}

impl Scheme {
    fn index(&self) -> usize {
        ALL.iter().position(|s| s.name == self.name).unwrap_or(0)
    }
    /// Is this whole value one of these?
    pub fn is(&self, value: &str) -> bool {
        let v = value.trim();
        compiled()[self.index()].1.is_match(v) && (self.check)(v)
    }
    /// Every one of these inside a text.
    pub fn find<'a>(&self, text: &'a str) -> Vec<&'a str> {
        compiled()[self.index()]
            .0
            .find_iter(text)
            .map(|m| m.as_str())
            .filter(|m| (self.check)(m))
            .collect()
    }
    /// What a declaration writes under `match:` for a column of these.
    pub fn declared(&self) -> String {
        format!("/{}/", whole(self))
    }
    /// Whether a value is this scheme's only by its check digit, which chance passes often.
    pub fn checked(&self) -> bool {
        matches!(self.name, "isbn" | "issn" | "orcid" | "lei" | "isin" | "arxiv")
    }
    /// Whether a value sorts things into kinds rather than naming one. A CWE, a CPE and a purl
    /// name a weakness, a product and a package: identifiers of a catalogue of those, and
    /// properties of anything else.
    pub fn classifies(&self) -> bool {
        matches!(self.name, "cwe" | "cpe" | "purl")
    }
    pub fn normalise(&self, value: &str) -> String {
        (self.normalise)(value)
    }
}

/// A thing's key: the scheme and the value as that scheme compares it. A scheme this program does
/// not know compares without case, as every key always has.
pub fn key(scheme: &str, value: &str) -> String {
    let v = match named(scheme) {
        Some(s) => s.normalise(value),
        None => lower(value),
    };
    format!("{scheme}:{v}")
}

/// Which schemes a value is, most specific first. A value can be two: `0317-8471` is shaped like
/// an ISSN and an ORCID prefix, and only the check digits tell them apart.
#[cfg(test)]
fn recognise(value: &str) -> Vec<&'static Scheme> {
    ALL.iter().filter(|s| s.is(value)).collect()
}

fn doi(s: &str) -> String {
    let s = s.trim();
    let lowered = s.to_lowercase();
    for prefix in ["https://doi.org/", "http://doi.org/", "https://dx.doi.org/", "http://dx.doi.org/", "doi:"] {
        if let Some(rest) = lowered.strip_prefix(prefix) {
            return rest.to_string();
        }
    }
    lowered
}

/// A paper, not a revision of it: v1 and v3 are the same thing, said twice.
fn arxiv(s: &str) -> String {
    let s = s.trim().to_lowercase();
    let s = s.strip_prefix("arxiv:").unwrap_or(&s);
    match s.rfind('v') {
        Some(i) if s[i + 1..].chars().all(|c| c.is_ascii_digit()) && i > 0 => s[..i].to_string(),
        _ => s.to_string(),
    }
}
fn arxiv_month(s: &str) -> bool {
    let digits: String = s.chars().skip_while(|c| !c.is_ascii_digit()).collect();
    let month: u32 = digits.get(2..4).and_then(|m| m.parse().ok()).unwrap_or(0);
    let year: u32 = digits.get(0..2).and_then(|y| y.parse().ok()).unwrap_or(0);
    (1..=12).contains(&month) && year >= 7
}

fn digits(s: &str) -> Vec<u32> {
    s.chars()
        .filter(|c| c.is_ascii_digit() || *c == 'X' || *c == 'x')
        .map(|c| c.to_digit(10).unwrap_or(10))
        .collect()
}

fn isbn_ok(s: &str) -> bool {
    let d = digits(s);
    match d.len() {
        10 => {
            d[..9].iter().all(|&x| x < 10)
                && d.iter().enumerate().map(|(i, &x)| (10 - i as u32) * x).sum::<u32>() % 11 == 0
        }
        13 => {
            d.iter().all(|&x| x < 10)
                && (d[0], d[1]) == (9, 7)
                && d.iter().enumerate().map(|(i, &x)| if i % 2 == 0 { x } else { 3 * x }).sum::<u32>() % 10 == 0
        }
        _ => false,
    }
}
/// Thirteen digits, whichever length it was written in, so the two editions of one number meet.
fn isbn(s: &str) -> String {
    let d = digits(s);
    if d.len() != 10 {
        return d.iter().map(|x| x.to_string()).collect();
    }
    let mut out: Vec<u32> = vec![9, 7, 8];
    out.extend(&d[..9]);
    let sum: u32 = out.iter().enumerate().map(|(i, &x)| if i % 2 == 0 { x } else { 3 * x }).sum();
    out.push((10 - sum % 10) % 10);
    out.iter().map(|x| x.to_string()).collect()
}

fn issn_ok(s: &str) -> bool {
    let d = digits(s);
    d.len() == 8
        && d[..7].iter().all(|&x| x < 10)
        && d.iter().enumerate().map(|(i, &x)| (8 - i as u32) * x).sum::<u32>() % 11 == 0
}

fn orcid_ok(s: &str) -> bool {
    let d = digits(s);
    if d.len() != 16 || d[..15].iter().any(|&x| x > 9) {
        return false;
    }
    let total = d[..15].iter().fold(0, |t, &x| (t + x) * 2);
    (12 - total % 11) % 11 == d[15]
}
fn orcid(s: &str) -> String {
    let s = s.trim().to_lowercase();
    let s = s.rsplit('/').next().unwrap_or(&s).to_string();
    s
}

/// ISO 7064 mod 97-10 over the characters as numbers, which is what an LEI and an IBAN carry.
fn mod97(s: &str) -> u32 {
    let mut r: u64 = 0;
    for c in s.chars() {
        let n = c.to_digit(36).unwrap_or(0) as u64;
        r = if n > 9 { (r * 100 + n) % 97 } else { (r * 10 + n) % 97 };
    }
    r as u32
}
fn lei_ok(s: &str) -> bool {
    s.len() == 20 && mod97(s) == 1
}

fn isin_ok(s: &str) -> bool {
    if s.len() != 12 {
        return false;
    }
    let expanded: String = s.chars().map(|c| c.to_digit(36).unwrap_or(0).to_string()).collect();
    let sum: u32 = expanded
        .chars()
        .rev()
        .enumerate()
        .map(|(i, c)| {
            let d = c.to_digit(10).unwrap_or(0);
            if i % 2 == 1 {
                let dd = d * 2;
                dd / 10 + dd % 10
            } else {
                d
            }
        })
        .sum();
    sum % 10 == 0
}

/// A standard, not an edition of it: the year after the colon is dropped, the part is kept.
fn iso(s: &str) -> String {
    let s = s.trim().to_lowercase().replace(' ', "");
    match s.split_once(':') {
        Some((head, _)) => head.to_string(),
        None => s,
    }
}
fn rfc(s: &str) -> String {
    s.trim().to_lowercase().replace(' ', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_example_is_its_own_scheme() {
        for s in ALL {
            assert!(s.is(s.example), "{}: {}", s.name, s.example);
        }
    }

    #[test]
    fn check_digits_tell_numbers_apart() {
        let isbn = named("isbn").unwrap();
        assert!(isbn.is("0-306-40615-2"));
        assert_eq!(isbn.normalise("0-306-40615-2"), isbn.normalise("978-0-306-40615-7"));
        assert!(!isbn.is("978-0-306-40615-8"));
        assert!(!named("isin").unwrap().is("US0378331006"));
        assert!(!named("lei").unwrap().is("5493001KJTIIGC8Y1R13"));
        assert!(named("orcid").unwrap().is("0000-0002-1694-233X"));
        assert!(!named("orcid").unwrap().is("0000-0002-1694-2330"));
        assert_eq!(recognise("0317-8471").iter().map(|s| s.name).collect::<Vec<_>>(), ["issn"]);
    }

    #[test]
    fn keys_meet_where_the_standard_says_they_are_one() {
        assert_eq!(key("cve", "CVE-2021-44228"), "cve:cve-2021-44228");
        assert_eq!(key("doi", "https://doi.org/10.1038/NATURE12373"), key("doi", "10.1038/nature12373"));
        assert_eq!(key("arxiv", "arXiv:2106.09685v2"), "arxiv:2106.09685");
        assert_eq!(key("iso", "ISO/IEC 27001:2022"), key("iso", "iso/iec 27001"));
        assert_eq!(key("rfc", "RFC 9110"), "rfc:rfc9110");
        assert_eq!(key("model", "Meta/Llama"), "model:meta/llama");
    }

    #[test]
    fn found_inside_text() {
        let cve = named("cve").unwrap();
        assert_eq!(cve.find("see CVE-2021-44228 and cve-2021-45046."), ["CVE-2021-44228", "cve-2021-45046"]);
        assert!(named("arxiv").unwrap().find("version 2024.99999 of the tool").is_empty());
    }
}
