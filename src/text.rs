//! What a page says, as lines of text, and what of it changes on its own.
//!
//! A whole page is watched by what a reader sees, not by its HTML: the order of attributes, a
//! session token in a hidden field or a renamed class says nothing. What is left still moves on
//! its own (a clock, a visitor count, an advert), so a page is read twice when it is first watched
//! and what differs between the two reads is never reported. Lines are remembered by their shape,
//! each word with a digit in it as `#`, so tomorrow's clock is recognised as today's.

use std::collections::{BTreeSet, HashMap, HashSet};

use scraper::{Html, Node, Selector};

/// Whitespace as a reader sees it: runs of it one space, none at the ends.
pub fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A line with each word that has a digit in it as `#`: what a clock, a counter or a generated
/// code keeps from one read to the next, however it reads today.
pub fn shape(s: &str) -> String {
    s.split_whitespace().map(|w| if w.chars().any(|c| c.is_ascii_digit()) { "#" } else { w }).collect::<Vec<_>>().join(" ")
}

/// The text of the page's body, one line per piece of text, as it reads.
pub fn lines(html: &str) -> Vec<String> {
    let doc = Html::parse_document(html);
    let body = Selector::parse("body").expect("a fixed selector");
    let mut out: Vec<String> = Vec::new();
    let Some(body) = doc.select(&body).next() else { return out };
    for node in body.descendants() {
        let Node::Text(t) = node.value() else { continue };
        let hidden = node.ancestors().any(|a| matches!(a.value().as_element().map(|e| e.name()), Some("script" | "style" | "template" | "noscript")));
        if hidden {
            continue;
        }
        let s = norm(t);
        if !s.is_empty() && out.last() != Some(&s) {
            out.push(s);
        }
    }
    out
}

/// The shapes of the lines that changed between two reads moments apart.
pub fn volatile(first: &[String], second: &[String]) -> Vec<String> {
    let later: HashSet<&String> = second.iter().collect();
    let set: BTreeSet<String> = first.iter().filter(|l| !later.contains(l)).map(|l| shape(l)).collect();
    set.into_iter().collect()
}

/// The lines without those that change on their own.
pub fn kept(lines: Vec<String>, volatile: &[String]) -> Vec<String> {
    let skip: HashSet<&str> = volatile.iter().map(String::as_str).collect();
    lines.into_iter().filter(|l| !skip.contains(shape(l).as_str())).collect()
}

/// The lines that went and the lines that came, each as often as it went or came.
pub fn diff(before: &str, after: &str) -> (Vec<String>, Vec<String>) {
    let count = |s: &str| {
        let mut m: HashMap<String, i64> = HashMap::new();
        for l in s.lines() {
            *m.entry(l.to_string()).or_default() += 1;
        }
        m
    };
    let (b, a) = (count(before), count(after));
    let mut gone = Vec::new();
    for l in before.lines() {
        if b[l] > *a.get(l).unwrap_or(&0) && !gone.contains(&l.to_string()) {
            gone.push(l.to_string());
        }
    }
    let mut came = Vec::new();
    for l in after.lines() {
        if a[l] > *b.get(l).unwrap_or(&0) && !came.contains(&l.to_string()) {
            came.push(l.to_string());
        }
    }
    (gone, came)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_reads_as_its_text_without_scripts_or_styles() {
        let page = "<html><head><title>T</title></head><body><h1>Prices</h1><style>p{}</style><p>Pro   <b>99 €</b></p><script>x()</script></body></html>";
        assert_eq!(lines(page), vec!["Prices", "Pro", "99 €"]);
    }

    #[test]
    fn what_moves_between_two_reads_is_not_a_change() {
        let first = vec!["Prices".to_string(), "Updated 09:00:01".to_string(), "Pro 99 €".to_string()];
        let second = vec!["Prices".to_string(), "Updated 09:00:04".to_string(), "Pro 99 €".to_string()];
        let v = volatile(&first, &second);
        assert_eq!(v, vec!["Updated #"]);
        let tomorrow = vec!["Prices".to_string(), "Updated 17:45:12".to_string(), "Pro 89 €".to_string()];
        assert_eq!(kept(tomorrow, &v), vec!["Prices", "Pro 89 €"]);
    }

    #[test]
    fn a_diff_names_what_went_and_what_came() {
        let (gone, came) = diff("a\nb\nc", "a\nc\nd");
        assert_eq!(gone, vec!["b"]);
        assert_eq!(came, vec!["d"]);
    }
}
