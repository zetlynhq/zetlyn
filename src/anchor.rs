//! The place a person marked on a page, and finding it again.
//!
//! A CSS path alone breaks the first time a page adds a banner above what was marked. So what is
//! kept is several things about the place: its path, its tag, its `id` and classes, and the text
//! just before it, its label ("Price", "Version"). The next read looks where the path points and
//! checks that the place still sits beside its label; if it does not, every element of the same
//! tag is scored on the rest and one clear winner is taken. Two close candidates, or none, is said
//! as such: a person is asked to mark it again rather than told about a change that is not one.
//!
//! The label is worked out the same way here and in the frame's script (`page.rs`), so what the
//! person saw beside the place is what is looked for.

use std::collections::HashSet;

use ego_tree::NodeRef;
use scraper::{ElementRef, Html, Node, Selector};
use serde::{Deserialize, Serialize};

use crate::text::{norm, shape};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Anchor {
    pub path: String,
    pub tag: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub classes: Vec<String>,
    /// What the place said when it was marked, to show; never used to find it, since it changes.
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, PartialEq)]
pub enum Found {
    /// Found, with what it says now. `moved`: not where the path pointed, but found by the rest.
    Here { value: String, moved: bool },
    /// More than one place could be it.
    Unclear,
    Gone,
}

/// The most of a place's text that is kept as its value.
const VALUE_MAX: usize = 2000;

pub fn find(html: &str, a: &Anchor) -> Found {
    let doc = Html::parse_document(html);
    if let Ok(sel) = Selector::parse(&a.path) {
        if let Some(el) = doc.select(&sel).next() {
            if el.value().name() == a.tag && consistent(el, a) {
                return Found::Here { value: value_of(el), moved: false };
            }
        }
    }
    let Ok(same_tag) = Selector::parse(&a.tag) else { return Found::Gone };
    let mut scored: Vec<(i32, ElementRef)> = doc.select(&same_tag).map(|el| (score(el, a), el)).filter(|(s, _)| *s >= 4).collect();
    scored.sort_by(|x, y| y.0.cmp(&x.0));
    match scored.as_slice() {
        [] => Found::Gone,
        [(_, el)] => Found::Here { value: value_of(*el), moved: true },
        [(best, el), (next, _), ..] if best - next >= 2 => Found::Here { value: value_of(*el), moved: true },
        _ => Found::Unclear,
    }
}

/// Whether the element the path points at is still the place that was marked: beside the same
/// label (a label that is itself a number may have changed its digits), or failing a label, with
/// the same `id` or classes.
fn consistent(el: ElementRef, a: &Anchor) -> bool {
    if !a.label.is_empty() {
        return shape(&label_of(el)) == shape(&a.label);
    }
    // Either will do: some pages give an element a new `id` every few minutes (GitHub), others
    // rename their classes with each release.
    let id = !a.id.is_empty() && el.value().id() == Some(a.id.as_str());
    let classes = !a.classes.is_empty() && overlap(el, a) >= 0.5;
    id || classes || (a.id.is_empty() && a.classes.is_empty())
}

fn score(el: ElementRef, a: &Anchor) -> i32 {
    let mut s = 0;
    if !a.id.is_empty() && el.value().id() == Some(a.id.as_str()) {
        s += 5;
    }
    if !a.label.is_empty() {
        let label = label_of(el);
        if label == a.label {
            s += 4;
        } else if shape(&label) == shape(&a.label) {
            s += 2;
        }
    }
    s += (overlap(el, a) * 3.0).round() as i32;
    if parents(el, 2) == path_parents(&a.path, 2) {
        s += 1;
    }
    s
}

fn overlap(el: ElementRef, a: &Anchor) -> f64 {
    let mine: HashSet<&str> = el.value().classes().collect();
    let theirs: HashSet<&str> = a.classes.iter().map(String::as_str).collect();
    let union = mine.union(&theirs).count();
    if union == 0 {
        return 0.0;
    }
    mine.intersection(&theirs).count() as f64 / union as f64
}

/// The tags of the element's nearest `n` ancestors, nearest first.
fn parents(el: ElementRef, n: usize) -> Vec<String> {
    el.ancestors().filter_map(|p| p.value().as_element().map(|e| e.name().to_string())).take(n).collect()
}

fn path_parents(path: &str, n: usize) -> Vec<String> {
    let mut steps: Vec<String> = path.split('>').map(|s| s.trim().split(':').next().unwrap_or("").to_string()).collect();
    steps.pop();
    steps.into_iter().rev().take(n).collect()
}

/// The anchor without what changes on its own: the place read a second time where its path
/// points, and an `id`, a class or a label that differs from the first read dropped (GitHub gives
/// its headings a new `id` on every request). None when the path no longer points at the place.
pub fn steadied(html: &str, a: &Anchor) -> Option<Anchor> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse(&a.path).ok()?;
    let el = doc.select(&sel).next()?;
    if el.value().name() != a.tag {
        return None;
    }
    let again = describe(el);
    Some(Anchor {
        id: if again.id == a.id { a.id.clone() } else { String::new() },
        classes: a.classes.iter().filter(|c| again.classes.contains(c)).cloned().collect(),
        label: if shape(&again.label) == shape(&a.label) { a.label.clone() } else { String::new() },
        ..a.clone()
    })
}

/// The place a CSS selector names on a page, described as a click on it in the frame describes
/// it: what a script or a person who knows the page can mark without the frame.
pub fn at(html: &str, css: &str) -> Result<Anchor, String> {
    let sel = Selector::parse(css).map_err(|e| format!("{css}: {e:?}"))?;
    let doc = Html::parse_document(html);
    let el = doc.select(&sel).next().ok_or_else(|| format!("{css}: nothing on the page matches"))?;
    Ok(describe(el))
}

pub fn describe(el: ElementRef) -> Anchor {
    Anchor {
        path: path_of(el),
        tag: el.value().name().to_string(),
        id: el.value().id().unwrap_or("").to_string(),
        classes: el.value().classes().map(String::from).collect(),
        text: value_of(el).chars().take(500).collect(),
        label: label_of(el),
    }
}

/// `html > body:nth-of-type(1) > …`: each step the element's tag and its place among siblings of
/// that tag, as the frame's script writes it.
pub fn path_of(el: ElementRef) -> String {
    let mut steps = Vec::new();
    let mut node: NodeRef<Node> = *el;
    loop {
        let Some(e) = node.value().as_element() else { break };
        if e.name() == "html" {
            break;
        }
        let nth = 1 + node.prev_siblings().filter(|s| s.value().as_element().is_some_and(|x| x.name() == e.name())).count();
        steps.push(format!("{}:nth-of-type({nth})", e.name()));
        match node.parent() {
            Some(p) => node = p,
            None => break,
        }
    }
    steps.reverse();
    format!("html > {}", steps.join(" > "))
}

/// The places on a page one could mark, for somebody who cannot click (an LLM over the API): each
/// element with text of its own, short enough to be a value, with the selector that marks it, what
/// it says and what stands beside it. In page order, each text once, at most `max`.
pub fn outline(html: &str, max: usize) -> Vec<Anchor> {
    places(html, max, None)
}

/// The places on a page a person would mark, at most `max`: what the page is about first, its
/// menus, headers, footers and side bars after. A value a page writes in pieces ("799", ",99",
/// "€" in elements of their own) is offered whole too, as the element around the pieces. With
/// `find`, only the places whose text, or the text beside them, has it in it, from the whole page.
pub fn places(html: &str, max: usize, find: Option<&str>) -> Vec<Anchor> {
    let doc = Html::parse_document(html);
    let Ok(body) = Selector::parse("body") else { return Vec::new() };
    let Some(body) = doc.select(&body).next() else { return Vec::new() };
    let wanted = find.map(|f| norm(f).to_lowercase()).filter(|f| !f.is_empty());
    let mut seen_text = HashSet::new();
    let mut seen_node = HashSet::new();
    let (mut content, mut frame) = (Vec::new(), Vec::new());
    let mut take = |el: ElementRef, content: &mut Vec<Anchor>, frame: &mut Vec<Anchor>| {
        let text = value_of(el);
        if text.is_empty() || text.chars().count() > 120 || !seen_node.insert(el.id()) || !seen_text.insert(text.clone()) {
            return;
        }
        let place = describe(el);
        if in_frame(el) { frame.push(place) } else { content.push(place) }
    };
    for node in body.descendants() {
        let Some(el) = ElementRef::wrap(node) else { continue };
        if matches!(el.value().name(), "script" | "style" | "template" | "noscript" | "svg" | "head" | "title") {
            continue;
        }
        let own = el.children().any(|c| matches!(c.value(), Node::Text(t) if !t.trim().is_empty()));
        if !own {
            continue;
        }
        // A piece of a value: the element around it that says it whole, before the piece.
        if piece(&value_of(el)) {
            let whole = el.ancestors().take(3).filter_map(ElementRef::wrap).find(|a| {
                let t = value_of(*a);
                t.chars().count() <= 40 && crate::when::money_in(&t)
            });
            if let Some(whole) = whole {
                take(whole, &mut content, &mut frame);
            }
        }
        take(el, &mut content, &mut frame);
        if wanted.is_none() && content.len() >= max {
            break;
        }
    }
    content.extend(frame);
    // What was looked for: the places that say it; where none does, those beside which it is said.
    if let Some(w) = &wanted {
        let says = |t: &str| t.to_lowercase().contains(w.as_str());
        content = if content.iter().any(|p| says(&p.text)) { content.into_iter().filter(|p| says(&p.text)).collect() } else { content.into_iter().filter(|p| says(&p.label)).collect() };
    }
    content.truncate(max);
    content
}

/// Whether a text is only a piece of a value: digits, a separator or a currency, and short.
fn piece(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty() && t.chars().count() <= 6 && !crate::when::money_in(t) && t.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | '€' | '$' | '£' | '¥' | ' ' | '\u{a0}'))
}

/// Whether an element is part of a page's frame rather than what the page is about: its header,
/// menus, footer, side bars, breadcrumbs, cookie notices.
fn in_frame(el: ElementRef) -> bool {
    const ROLES: &[&str] = &["navigation", "banner", "contentinfo", "complementary", "menu", "menubar"];
    const WORDS: &[&str] = &["nav", "menu", "breadcrumb", "cookie", "consent", "sidebar"];
    // A header or footer of an article (a product's title, often) belongs to what the page is about.
    let in_main = |a: ElementRef| a.ancestors().filter_map(ElementRef::wrap).any(|m| matches!(m.value().name(), "main" | "article"));
    el.ancestors().filter_map(ElementRef::wrap).chain(std::iter::once(el)).any(|a| {
        let e = a.value();
        matches!(e.name(), "nav" | "aside")
            || (matches!(e.name(), "header" | "footer") && !in_main(a))
            || e.attr("role").is_some_and(|r| ROLES.contains(&r))
            || e.id().into_iter().chain(e.classes()).any(|n| {
                let n = n.to_ascii_lowercase();
                WORDS.iter().any(|w| n == *w || n.starts_with(&format!("{w}-")) || n.starts_with(&format!("{w}_")) || n.ends_with(&format!("-{w}")) || n.ends_with(&format!("_{w}")))
            })
    })
}

/// What the place says: its text as a reader sees it, bounded.
pub fn value_of(el: ElementRef) -> String {
    // A style or script inside the place (Wikipedia puts styles in its infoboxes) is not what it says.
    let hidden = |n: &NodeRef<Node>| n.ancestors().any(|a| matches!(a.value().as_element().map(|e| e.name()), Some("style" | "script" | "template")));
    // Text in two cells or blocks is two words; text split by inline elements stays one ("1.<b>99</b>").
    const BLOCK: &[&str] = &["td", "th", "tr", "li", "p", "div", "dt", "dd", "h1", "h2", "h3", "h4", "h5", "h6", "section", "article", "table", "ul", "ol", "header", "footer"];
    let mut shown = String::new();
    let mut last_block = None;
    for n in el.descendants().filter(|n| !hidden(n)) {
        if let Node::Text(t) = n.value() {
            let block = n.ancestors().find(|a| a.value().as_element().is_some_and(|e| BLOCK.contains(&e.name()))).map(|a| a.id());
            // A price in pieces ("14," and "99" in blocks of their own) stays one number.
            let joins_number = shown.trim_end().ends_with([',', '.']) && shown.trim_end().chars().rev().nth(1).is_some_and(|c| c.is_ascii_digit()) && t.trim_start().starts_with(|c: char| c.is_ascii_digit());
            if last_block.is_some() && block != last_block && !joins_number {
                shown.push(' ');
            }
            last_block = block;
            shown.push_str(t);
        } else if n.value().as_element().is_some_and(|e| e.name() == "br") {
            shown.push(' ');
        }
    }
    let v = norm(&shown);
    if v.chars().count() > VALUE_MAX {
        v.chars().take(VALUE_MAX).collect()
    } else {
        v
    }
}

/// The text just before the place: the nearest earlier sibling with any text, if that is short;
/// otherwise the same one level up, three levels at most, never above `body`. The frame's script
/// does exactly this.
pub fn label_of(el: ElementRef) -> String {
    let mut node: NodeRef<Node> = *el;
    for _ in 0..3 {
        if matches!(node.value().as_element().map(|e| e.name()), Some("body" | "html")) {
            break;
        }
        let mut sibling = node.prev_sibling();
        while let Some(s) = sibling {
            if !matches!(s.value(), Node::Comment(_)) {
                let t = norm(&text_of(s));
                if !t.is_empty() {
                    if t.chars().count() <= 80 {
                        return t;
                    }
                    break;
                }
            }
            sibling = s.prev_sibling();
        }
        match node.parent() {
            Some(p) if p.value().is_element() => node = p,
            _ => break,
        }
    }
    String::new()
}

fn text_of(node: NodeRef<Node>) -> String {
    node.descendants().filter_map(|n| if let Node::Text(t) = n.value() { Some(t.to_string()) } else { None }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<html><body><h1>Pricing</h1><div class="plans"><div class="plan"><h2>Starter</h2><span class="price">29 €</span></div><div class="plan"><h2>Pro</h2><span class="price">99 €</span></div></div></body></html>"#;

    fn anchor_on(page: &str, path: &str) -> Anchor {
        let doc = Html::parse_document(page);
        let el = doc.select(&Selector::parse(path).unwrap()).next().unwrap();
        Anchor {
            path: path.into(),
            tag: el.value().name().into(),
            id: el.value().id().unwrap_or("").into(),
            classes: el.value().classes().map(String::from).collect(),
            text: value_of(el),
            label: label_of(el),
        }
    }

    const PRO: &str = "html > body:nth-of-type(1) > div:nth-of-type(1) > div:nth-of-type(2) > span:nth-of-type(1)";

    #[test]
    fn a_style_inside_the_place_is_not_its_value() {
        let page = r#"<html><body><table class="infobox"><style>.x{color:red}</style><tr><td>Stable release</td><td>1.99</td></tr></table></body></html>"#;
        assert_eq!(at(page, ".infobox").unwrap().text, "Stable release 1.99");
    }

    #[test]
    fn the_label_is_the_text_just_before() {
        let a = anchor_on(PAGE, PRO);
        assert_eq!(a.label, "Pro");
        assert_eq!(a.text, "99 €");
    }

    #[test]
    fn found_where_it_was_with_what_it_says_now() {
        let a = anchor_on(PAGE, PRO);
        let tomorrow = PAGE.replace("99 €", "89 €");
        assert_eq!(find(&tomorrow, &a), Found::Here { value: "89 €".into(), moved: false });
    }

    #[test]
    fn found_by_its_label_when_a_plan_is_added_before_it() {
        let a = anchor_on(PAGE, PRO);
        let tomorrow = PAGE.replace(r#"<div class="plans">"#, r#"<div class="plans"><div class="plan"><h2>Free</h2><span class="price">0 €</span></div>"#);
        assert_eq!(find(&tomorrow, &a), Found::Here { value: "99 €".into(), moved: true });
    }

    #[test]
    fn two_places_alike_with_the_same_label_is_unclear() {
        let mut a = anchor_on(PAGE, PRO);
        a.path = "html > body > p".into();
        let tomorrow = PAGE.replace("<h2>Starter</h2>", "<h2>Pro</h2>");
        assert_eq!(find(&tomorrow, &a), Found::Unclear);
    }

    #[test]
    fn a_selector_marks_the_place_a_click_would() {
        let a = at(PAGE, ".plan:nth-of-type(2) .price").unwrap();
        assert_eq!(a, anchor_on(PAGE, PRO));
        assert_eq!(a.path, PRO);
        assert!(at(PAGE, "table").is_err());
    }

    #[test]
    fn an_id_made_anew_on_every_request_is_not_kept() {
        let first = PAGE.replace(r#"<span class="price">99"#, r#"<span id="hd-a8eabda8" class="price">99"#);
        let a = at(&first, ".plan:nth-of-type(2) .price").unwrap();
        assert_eq!(a.id, "hd-a8eabda8");
        let second = PAGE.replace(r#"<span class="price">99"#, r#"<span id="hd-77c01f2e" class="price">99"#);
        let steady = steadied(&second, &a).unwrap();
        assert_eq!((steady.id.as_str(), steady.label.as_str()), ("", "Pro"));
        let third = PAGE.replace(r#"<span class="price">99 €"#, r#"<span id="hd-0b3c9d11" class="price">89 €"#);
        assert_eq!(find(&third, &steady), Found::Here { value: "89 €".into(), moved: false });
    }

    #[test]
    fn an_id_that_changed_later_does_not_lose_a_place_with_its_classes() {
        let page = r#"<html><body><section><h2 class="sr-only" id="hd-0633f2a3">0.60.9</h2></section></body></html>"#;
        let a = at(page, "h2").unwrap();
        assert_eq!(a.label, "");
        let later = page.replace("hd-0633f2a3", "hd-386b91fe").replace("0.60.9", "0.61.0");
        assert_eq!(find(&later, &a), Found::Here { value: "0.61.0".into(), moved: false });
    }

    #[test]
    fn the_outline_names_each_place_with_the_selector_that_marks_it() {
        let places = outline(PAGE, 50);
        let texts: Vec<&str> = places.iter().map(|a| a.text.as_str()).collect();
        assert_eq!(texts, vec!["Pricing", "Starter", "29 €", "Pro", "99 €"]);
        let pro = places.iter().find(|a| a.text == "99 €").unwrap();
        assert_eq!((pro.path.as_str(), pro.label.as_str()), (PRO, "Pro"));
        assert_eq!(find(PAGE, pro), Found::Here { value: "99 €".into(), moved: false });
    }

    #[test]
    fn gone_when_nothing_like_it_is_left() {
        let a = anchor_on(PAGE, PRO);
        assert_eq!(find("<html><body><p>Closed</p></body></html>", &a), Found::Gone);
    }

    #[test]
    fn a_shop_page_offers_its_product_first_and_its_price_whole() {
        let page = r#"<html><body>
            <header><nav><a>Home</a><a>Phones</a><a>Deals</a></nav></header>
            <div class="cookie-banner"><p>We use cookies</p></div>
            <main><article><header><h1>iPhone 16, 128 GB</h1></header>
              <div class="price"><span class="whole">799</span><sup>,99</sup><span class="cur">€</span></div>
              <p class="stock">In stock</p></article></main>
            <footer><a>Imprint</a></footer></body></html>"#;
        let texts: Vec<String> = places(page, 50, None).into_iter().map(|a| a.text).collect();
        assert_eq!(texts.first().map(String::as_str), Some("iPhone 16, 128 GB"), "the product, though in a header, comes first: {texts:?}");
        let whole = texts.iter().position(|t| t == "799,99€").expect("the price offered whole");
        let piece = texts.iter().position(|t| t == "799").unwrap();
        assert!(whole < piece, "the whole before its pieces: {texts:?}");
        let home = texts.iter().position(|t| t == "Home").unwrap();
        assert!(home > texts.iter().position(|t| t == "In stock").unwrap(), "menus after the content");
        assert!(texts.iter().position(|t| t == "We use cookies").unwrap() > whole, "cookie notices after the content");
        let found: Vec<String> = places(page, 50, Some("€")).into_iter().map(|a| a.text).collect();
        assert_eq!(found, vec!["799,99€".to_string(), "€".to_string()], "find searches text");
        assert_eq!(places(page, 50, Some("in STOCK")).len(), 1, "case aside");
        let blocks = r#"<html><body><main><div class="p"><div>14,</div><div>99</div><span>&nbsp;€</span></div><ul><li>Rot,</li><li>Blau</li></ul></main></body></html>"#;
        assert!(places(blocks, 50, Some("€")).iter().any(|a| a.text == "14,99 €"), "cents in a block of their own stay with the euros");
        assert_eq!(at(blocks, "ul").unwrap().text, "Rot, Blau", "words in blocks stay apart");
        let beside: Vec<String> = places(page, 50, Some("Phones")).into_iter().map(|a| a.text).collect();
        assert_eq!(beside, vec!["Phones".to_string()], "said in a place: that place, not those beside it");
        assert!(places(page, 50, Some("nothing like this")).is_empty());
    }
}
