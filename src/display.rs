//! How a value reads on a page, as the tracker's `values` say: a number to its precision and with
//! its unit, a country code as the country's name, a thing's title from what its sources say,
//! and the colours a map or a table gives a value. What a source said is never changed by any of
//! it; the page under it still shows the words as they came.

use crate::trackerdecl::{TrackerDecl, ValueDecl};

pub fn decl_of<'a>(decl: &'a TrackerDecl, property: &str) -> Option<&'a ValueDecl> {
    decl.values.get(property)
}

/// What a property is called on a page.
pub fn label(decl: &TrackerDecl, property: &str) -> String {
    match decl_of(decl, property).map(|d| d.label.as_str()).filter(|l| !l.is_empty()) {
        Some(l) => l.to_string(),
        None => {
            let s = property.replace('_', " ");
            let mut c = s.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

/// Places after the point a number needs where the tracker does not say: few for a large one,
/// more for a small one, and none for a whole one.
fn places_for(v: f64) -> usize {
    let a = v.abs();
    if v.fract() == 0.0 || a >= 1000.0 {
        0
    } else if a >= 100.0 {
        1
    } else if a >= 1.0 {
        2
    } else {
        3
    }
}

fn grouped(s: &str) -> String {
    let (sign, rest) = s.strip_prefix('-').map(|r| ("-", r)).unwrap_or(("", s));
    let (int, frac) = rest.split_once('.').map(|(i, f)| (i, Some(f))).unwrap_or((rest, None));
    let mut out = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 && int.len() > 4 {
            out.push(',');
        }
        out.push(ch);
    }
    match frac {
        Some(f) => format!("{sign}{out}.{f}"),
        None => format!("{sign}{out}"),
    }
}

/// A number, to the places the tracker says and with its unit.
pub fn number(v: f64, d: Option<&ValueDecl>) -> String {
    let v = v / d.and_then(|d| d.per).filter(|p| *p != 0.0).unwrap_or(1.0);
    let places = d.and_then(|d| d.decimals).unwrap_or_else(|| places_for(v));
    let n = grouped(&format!("{v:.places$}"));
    match d.map(|d| d.unit.as_str()).unwrap_or("") {
        "" => n,
        "%" => format!("{n} %"),
        u => format!("{n} {u}"),
    }
}

/// A change, with its sign, in the value's own unit: `+0.4 %`, `−1.2`.
pub fn change(v: f64, d: Option<&ValueDecl>) -> String {
    let s = number(v.abs(), d);
    if v > 0.0 {
        format!("+{s}")
    } else if v < 0.0 {
        format!("−{s}")
    } else {
        s
    }
}

/// One word a source said, as it reads here.
pub fn word(w: &str, d: Option<&ValueDecl>) -> String {
    if let Some(d) = d {
        if d.is == "country" {
            if let Some(c) = crate::geo::iso3_of(w).and_then(|c| crate::geo::country(&c)) {
                return c.name.clone();
            }
        }
        if d.is == "quarter" {
            if let Some((y, q)) = w.split_once("-Q") {
                return format!("Q{q} {y}");
            }
        }
        if d.is.is_empty() || d.is == "number" {
            if let Ok(n) = w.trim().parse::<f64>() {
                return number(n, Some(d));
            }
        }
    }
    w.to_string()
}

/// Words of a property as they read, by the tracker's declaration of it.
pub fn shown(decl: &TrackerDecl, property: &str, w: &str) -> String {
    word(w, decl_of(decl, property))
}

/// More than one of a word: countries, models, vulnerabilities.
pub fn plural(word: &str) -> String {
    let consonant_y = word.ends_with('y') && !word.ends_with("ay") && !word.ends_with("ey") && !word.ends_with("oy");
    if consonant_y {
        format!("{}ies", &word[..word.len() - 1])
    } else if word.ends_with('s') || word.ends_with("ch") || word.ends_with("sh") || word.ends_with('x') {
        format!("{word}es")
    } else {
        format!("{word}s")
    }
}

/// Whether a word is written in Latin letters: a name a reader here can read.
pub fn latin(s: &str) -> bool {
    let letters: Vec<char> = s.chars().filter(|c| c.is_alphabetic()).collect();
    !letters.is_empty() && letters.iter().all(|c| (*c as u32) < 0x0250)
}

/// A thing's title from a template, given what its sources say of each property in their order of
/// priority, its identifier's value and the title its source gave. None where nothing in the
/// template is said.
pub fn title(template: &str, decl: &TrackerDecl, said: &dyn Fn(&str) -> Vec<String>, value: &str, own: &str) -> Option<String> {
    let mut out = String::new();
    let mut pending = String::new();
    let mut rest = template;
    let mut any = false;
    let mut seen = false;
    while !rest.is_empty() {
        match rest.find('{') {
            Some(0) => {
                let end = rest.find('}').unwrap_or(rest.len());
                let slot = &rest[1..end];
                rest = rest.get(end + 1..).unwrap_or("");
                let filled = slot.split('|').find_map(|alt| {
                    let (name, filter) = alt.trim().split_once(':').map(|(n, f)| (n, Some(f))).unwrap_or((alt.trim(), None));
                    let words: Vec<String> = match name {
                        "@value" => vec![value.to_string()],
                        "@title" => vec![own.to_string()],
                        p => said(p).into_iter().map(|w| shown(decl, p, &w)).collect(),
                    };
                    words.into_iter().map(|w| w.trim().to_string()).find(|w| !w.is_empty() && (filter != Some("latin") || latin(w)))
                });
                // Words before a `{}` go with it; before the first, they lead whatever is said. A
                // `{}` that says again what is there already says nothing: an EPSS row's title is
                // its CVE number.
                if let Some(f) = filled.filter(|f| !out.contains(f.as_str())) {
                    if !out.is_empty() || !seen {
                        out.push_str(&pending);
                    }
                    out.push_str(&f);
                    any = true;
                }
                seen = true;
                pending.clear();
            }
            Some(i) => {
                pending.push_str(&rest[..i]);
                rest = &rest[i..];
            }
            None => {
                pending.push_str(rest);
                rest = "";
            }
        }
    }
    if !out.is_empty() {
        out.push_str(pending.trim_end());
    }
    let out = out.trim().to_string();
    if !any || out.is_empty() {
        return None;
    }
    Some(if out.chars().count() > 120 { format!("{}…", out.chars().take(119).collect::<String>().trim_end()) } else { out })
}

// -- colours --------------------------------------------------------------------------------------

/// Seven steps from good to bad: green, through sand, to red.
pub const STEPS: [&str; 7] = ["#1a9850", "#66bd63", "#a6d96a", "#e6dc8c", "#fdae61", "#f46d43", "#d73027"];
/// Seven of one hue, for a count, where more is neither better nor worse.
pub const NEUTRAL: [&str; 7] = ["#e8eef4", "#c9d9e8", "#a3c0da", "#7aa3c9", "#5486b5", "#356a9c", "#1f4e7a"];
/// Two hues from a centre, for a change neither way of which is better: falling blue, rising red.
pub const TWO_SIDED: [&str; 7] = ["#2166ac", "#67a9cf", "#d1e5f0", "#f2f0ea", "#fddbc7", "#ef8a62", "#b2182b"];

/// Classes a value falls in and the colour of each.
#[derive(Debug, Clone)]
pub struct Scale {
    /// The edges between classes, lowest first; one class more than edges.
    pub edges: Vec<f64>,
    pub colours: Vec<&'static str>,
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

impl Scale {
    /// Classes for these values: the declared value's centre splits them where it has one, each
    /// side in evenly filled thirds; otherwise seven evenly filled classes. Coloured by which way
    /// is better, or in one hue where neither is. `cheap` stands for `better: lower` where the
    /// tracker declares nothing.
    pub fn of(values: &[f64], d: Option<&ValueDecl>, cheap: bool) -> Scale {
        let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let better = d.and_then(|d| d.better.clone()).or_else(|| cheap.then(|| "lower".to_string()));
        let centre = d.and_then(|d| d.centre);
        let mut edges: Vec<f64> = match centre {
            Some(c) => {
                let below: Vec<f64> = v.iter().copied().filter(|x| *x < c).collect();
                let above: Vec<f64> = v.iter().copied().filter(|x| *x >= c).collect();
                let mut e = Vec::new();
                if !below.is_empty() {
                    e.extend([quantile(&below, 1.0 / 3.0), quantile(&below, 2.0 / 3.0)]);
                }
                e.push(c);
                if !above.is_empty() {
                    e.extend([quantile(&above, 1.0 / 3.0), quantile(&above, 2.0 / 3.0)]);
                }
                e
            }
            None => (1..7).map(|i| quantile(&v, i as f64 / 7.0)).collect(),
        };
        edges.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
        let n = edges.len() + 1;
        // The first class above the centre, and how many classes from it a class is: 1 for the two
        // beside it.
        let split = centre.map(|c| edges.iter().filter(|e| **e < c + 1e-12).count()).unwrap_or(n / 2);
        let from_centre = |i: usize| -> (usize, usize) {
            let steps = if i < split { split - i } else { i - split + 1 };
            (steps, split.max(n - split))
        };
        let colours: Vec<&'static str> = (0..n)
            .map(|i| {
                let t = if n <= 1 { 0.5 } else { i as f64 / (n - 1) as f64 };
                match better.as_deref() {
                    Some("higher") => STEPS[((1.0 - t) * 6.0).round() as usize],
                    Some("lower") => STEPS[(t * 6.0).round() as usize],
                    // Nearest the centre best, by how many classes away a class is, not by its
                    // values: one country at 250 % would make every other look near.
                    Some("middle") => {
                        let (steps, most) = from_centre(i);
                        STEPS[(((steps - 1) as f64 / (most - 1).max(1) as f64) * 6.0).round() as usize]
                    }
                    // A centre and no better side: deeper the further from it, a hue a side.
                    _ if centre.is_some() => {
                        let (steps, most) = from_centre(i);
                        let k = (steps as f64 / most.max(1) as f64 * 3.0).round().clamp(1.0, 3.0) as usize;
                        if i < split { TWO_SIDED[3 - k] } else { TWO_SIDED[3 + k] }
                    }
                    _ => NEUTRAL[(t * 6.0).round() as usize],
                }
            })
            .collect();
        Scale { edges, colours }
    }

    pub fn class(&self, v: f64) -> usize {
        self.edges.iter().filter(|e| v > **e).count().min(self.colours.len().saturating_sub(1))
    }

    pub fn colour(&self, v: f64) -> &'static str {
        self.colours.get(self.class(v)).copied().unwrap_or("#bbb")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(values: &str) -> TrackerDecl {
        crate::yaml::parse(&format!("name: t/t\nvalues:\n{values}")).unwrap()
    }

    #[test]
    fn a_number_reads_to_its_precision_with_its_unit() {
        let d = decl("  inflation: { unit: \"%\", decimals: 1 }\n  diesel: { unit: €, decimals: 3 }\n  country: { is: country }\n");
        assert_eq!(shown(&d, "inflation", "7.975268"), "8.0 %");
        assert_eq!(shown(&d, "diesel", "1.849"), "1.849 €");
        assert_eq!(shown(&d, "country", "DEU"), "Germany");
        assert_eq!(shown(&d, "downloads", "59900000"), "59900000");
        assert_eq!(number(59_912_345.0, None), "59,912,345");
        assert_eq!(number(204.498891118386, None), "204.5");
        assert_eq!(change(-1.25, Some(&d.values["inflation"])), "−1.2 %");
    }

    #[test]
    fn a_title_from_a_template_leaves_out_what_is_not_said() {
        let d = decl("  country: { is: country }\n");
        let said = |p: &str| -> Vec<String> {
            match p {
                "brand" => vec!["Total Access".into()],
                "city" => vec!["Troyes".into()],
                "country" => vec!["ZWE".into()],
                "year" => vec!["2026".into()],
                "names" => vec!["ООО Уралмаш".into(), "Uralmash Holding LLC".into()],
                _ => vec![],
            }
        };
        assert_eq!(title("{name|brand} · {city}", &d, &said, "1", "RD 619").as_deref(), Some("Total Access · Troyes"));
        assert_eq!(title("{name} · {city}", &d, &said, "1", "RD 619").as_deref(), Some("Troyes"));
        assert_eq!(title("{country} {year}", &d, &said, "zwe-2026", "").as_deref(), Some("Zimbabwe 2026"));
        assert_eq!(title("{@title:latin|names:latin|@title}", &d, &said, "", "ООО Уралмаш").as_deref(), Some("Uralmash Holding LLC"));
        assert_eq!(title("{nothing}", &d, &said, "", "own"), None);
    }

    #[test]
    fn colours_turn_at_the_centre_and_counts_keep_one_hue() {
        let d = decl("  growth: { better: higher, centre: 0 }\n");
        let s = Scale::of(&[-3.0, -2.0, -1.0, 1.0, 2.0, 3.0], d.values.get("growth"), false);
        assert!(s.edges.contains(&0.0));
        assert_eq!(s.colour(-3.0), STEPS[6]);
        assert_eq!(s.colour(3.0), STEPS[0]);
        let n = Scale::of(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0], None, false);
        assert!(NEUTRAL.contains(&n.colour(8.0)));
    }
}
