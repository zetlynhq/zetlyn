//! What is not a web page, read as one: JSON, RSS and Atom feeds, other XML, and plain text (CSV
//! among it). Each is turned into a plain page of its own, one paragraph per field, entry or
//! line, so that everything that reads pages reads them too: the preview lists their values, a
//! value is a place with a selector, a condition weighs it, and the whole document is compared
//! line by line ("tag_name: 1.99.0" going, "tag_name: 1.100.0" coming).
//!
//! On such a page every value is a `<span class=v>` with the field's path beside it (which is how
//! a value is found again when fields come and go) and every paragraph carries the line the
//! document is compared by in `data-line`. A feed is told by its new entries only.

use quick_xml::events::Event;
use serde_json::Value as J;

use crate::serve::esc;

/// What kind of document a response is, other than a web page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Json,
    Feed,
    Xml,
    Text,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Json => "json",
            Kind::Feed => "feed",
            Kind::Xml => "xml",
            Kind::Text => "text",
        }
    }
}

/// How many fields, entries or lines a document is read to: as many as a whole page may have.
const MOST: usize = crate::check::PAGE_LINES_MAX;

/// The kind of a response by its type and, where the type says little, its first characters.
/// None for what has no text to watch (a PDF, an image).
pub fn kind(content_type: &str, body: &str) -> Option<Kind> {
    let ct = content_type.to_ascii_lowercase();
    // An image is drawn, not written, though SVG is XML.
    if ct.starts_with("image/") {
        return None;
    }
    let start = body.trim_start();
    if ct.contains("json") {
        return Some(Kind::Json);
    }
    if ct.contains("rss") || ct.contains("atom") {
        return Some(Kind::Feed);
    }
    if ct.contains("xml") {
        let head: String = start.chars().take(2000).collect();
        return Some(if head.contains("<rss") || head.contains("<feed") || head.contains("<rdf:RDF") { Kind::Feed } else { Kind::Xml });
    }
    if ct.starts_with("text/") && !ct.contains("html") {
        if (start.starts_with('{') || start.starts_with('[')) && serde_json::from_str::<J>(start).is_ok() {
            return Some(Kind::Json);
        }
        return Some(Kind::Text);
    }
    None
}

/// Whether a page is a document turned into one here.
pub fn is_document(html: &str) -> bool {
    html.starts_with("<html data-zetlyn=")
}

/// Whether a page is a feed turned into one here: told by its new entries.
pub fn is_feed(html: &str) -> bool {
    html.starts_with("<html data-zetlyn=\"feed\"")
}

/// A document as a page, or why it cannot be read.
pub fn page(kind: Kind, body: &str) -> Result<String, String> {
    let rows = match kind {
        Kind::Json => json_rows(body)?,
        Kind::Feed => feed_rows(body)?,
        Kind::Xml => xml_rows(body)?,
        Kind::Text => text_rows(body),
    };
    if rows.is_empty() {
        return Err("The document has nothing in it to watch.".into());
    }
    let mut out = format!("<html data-zetlyn=\"{}\"><head><meta charset=utf-8><title>{}</title><style>body{{font:15px/1.5 ui-monospace,monospace;margin:16px}}p{{margin:2px 0}}b{{font-weight:normal;color:#777;margin-right:.6em}}</style></head><body>", kind.name(), kind.name().to_uppercase());
    out.push_str(&rows.concat());
    out.push_str("</body></html>");
    Ok(out)
}

/// The lines a document is compared by, from its page (`data-line`); None for a web page.
pub fn lines(html: &str) -> Option<Vec<String>> {
    if !is_document(html) {
        return None;
    }
    let doc = scraper::Html::parse_document(html);
    let p = scraper::Selector::parse("p[data-line]").expect("a fixed selector");
    Some(doc.select(&p).filter_map(|e| e.value().attr("data-line")).map(crate::text::norm).filter(|l| !l.is_empty()).collect())
}

/// One field: its path beside its value.
fn field(path: &str, value: &str) -> String {
    let line = if path.is_empty() { value.to_string() } else { format!("{path}: {value}") };
    format!("<p data-line=\"{}\"><b>{}</b><span class=v id=\"{}\">{}</span></p>", esc(&line), esc(path), esc(&format!("f:{path}")), esc(value))
}

fn json_rows(body: &str) -> Result<Vec<String>, String> {
    let value: J = serde_json::from_str(body.trim_start_matches('\u{feff}')).map_err(|e| format!("The JSON could not be read: {e}"))?;
    let mut rows = Vec::new();
    walk(&value, String::new(), &mut rows);
    Ok(rows)
}

/// Every value of a JSON document with its path: `name`, `items[0].price`.
fn walk(v: &J, path: String, rows: &mut Vec<String>) {
    if rows.len() >= MOST {
        return;
    }
    match v {
        J::Object(o) => {
            if o.is_empty() {
                rows.push(field(&path, "{}"));
            }
            for (k, v) in o {
                walk(v, if path.is_empty() { k.clone() } else { format!("{path}.{k}") }, rows);
            }
        }
        J::Array(a) => {
            if a.is_empty() {
                rows.push(field(&path, "[]"));
            }
            for (i, v) in a.iter().enumerate() {
                walk(v, format!("{path}[{i}]"), rows);
            }
        }
        J::String(s) => rows.push(field(&path, &crate::text::norm(s))),
        other => rows.push(field(&path, &other.to_string())),
    }
}

fn text_rows(body: &str) -> Vec<String> {
    body.lines()
        .map(crate::text::norm)
        .filter(|l| !l.is_empty())
        .take(MOST)
        .map(|l| format!("<p data-line=\"{}\"><span class=v>{}</span></p>", esc(&l), esc(&l)))
        .collect()
}

/// The elements of an XML document as a tree of names and texts.
struct Element {
    name: String,
    attrs: Vec<(String, String)>,
    text: String,
    children: Vec<Element>,
}

fn parse_xml(body: &str) -> Result<Element, String> {
    let mut reader = quick_xml::Reader::from_str(body);
    let mut stack: Vec<Element> = vec![Element { name: String::new(), attrs: Vec::new(), text: String::new(), children: Vec::new() }];
    let name_of = |n: &[u8]| String::from_utf8_lossy(n).into_owned();
    loop {
        match reader.read_event().map_err(|e| format!("The XML could not be read: {e}"))? {
            Event::Start(e) => {
                let attrs = e.attributes().flatten().map(|a| (name_of(a.key.as_ref()), a.unescape_value().map(|v| v.into_owned()).unwrap_or_default())).collect();
                stack.push(Element { name: name_of(e.name().as_ref()), attrs, text: String::new(), children: Vec::new() });
            }
            Event::Empty(e) => {
                let attrs = e.attributes().flatten().map(|a| (name_of(a.key.as_ref()), a.unescape_value().map(|v| v.into_owned()).unwrap_or_default())).collect();
                let el = Element { name: name_of(e.name().as_ref()), attrs, text: String::new(), children: Vec::new() };
                if let Some(top) = stack.last_mut() {
                    top.children.push(el);
                }
            }
            Event::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&t.decode().map(|s| s.into_owned()).unwrap_or_default());
                }
            }
            Event::GeneralRef(r) => {
                if let Some(top) = stack.last_mut() {
                    let name = String::from_utf8_lossy(&r).into_owned();
                    top.text.push_str(match name.as_str() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => "",
                    });
                }
            }
            Event::CData(c) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&String::from_utf8_lossy(&c));
                }
            }
            Event::End(_) => {
                if stack.len() > 1 {
                    let done = stack.pop().expect("more than one");
                    stack.last_mut().expect("its parent").children.push(done);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let root = stack.into_iter().next().expect("the document");
    root.children.into_iter().next().ok_or_else(|| "The XML has no element in it.".into())
}

/// The local part of a name: `title` of `atom:title`.
fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

fn child<'a>(el: &'a Element, name: &str) -> Option<&'a Element> {
    el.children.iter().find(|c| local(&c.name) == name)
}

fn text_of(el: Option<&Element>) -> String {
    el.map(|e| crate::text::norm(&e.text)).unwrap_or_default()
}

/// A feed's entries, newest first as feeds list them: title and link; RSS (items under the
/// channel or, in RSS 1.0, beside it) and Atom.
fn feed_rows(body: &str) -> Result<Vec<String>, String> {
    let root = parse_xml(body)?;
    let channel = child(&root, "channel");
    let mut entries: Vec<&Element> = Vec::new();
    for scope in [Some(&root), channel].into_iter().flatten() {
        entries.extend(scope.children.iter().filter(|c| matches!(local(&c.name), "item" | "entry")));
    }
    if entries.is_empty() {
        return Err("The feed has no entries in it yet.".into());
    }
    Ok(entries
        .into_iter()
        .take(MOST)
        .map(|e| {
            let title = text_of(child(e, "title"));
            let link = child(e, "link").map(|l| l.attrs.iter().find(|(k, _)| k == "href").map(|(_, v)| v.clone()).unwrap_or_else(|| crate::text::norm(&l.text))).unwrap_or_default();
            let title = if title.is_empty() { text_of(child(e, "description").or_else(|| child(e, "summary"))).chars().take(120).collect() } else { title };
            let line = if link.is_empty() { title.clone() } else { format!("{title} — {link}") };
            let id = text_of(child(e, "guid").or_else(|| child(e, "id")));
            format!("<p data-line=\"{}\"><span class=v id=\"{}\">{}</span> <a href=\"{}\">{}</a></p>", esc(&line), esc(&format!("e:{}", if id.is_empty() { &link } else { &id })), esc(&title), esc(&link), esc(&link))
        })
        .collect())
}

/// Every element with text, and every attribute, of an XML document with its path:
/// `catalog.book[2].price`, `catalog.book[2]@id`.
fn xml_rows(body: &str) -> Result<Vec<String>, String> {
    let root = parse_xml(body)?;
    let mut rows = Vec::new();
    xml_walk(&root, local(&root.name).to_string(), &mut rows);
    Ok(rows)
}

fn xml_walk(el: &Element, path: String, rows: &mut Vec<String>) {
    if rows.len() >= MOST {
        return;
    }
    for (k, v) in &el.attrs {
        if !k.starts_with("xmlns") {
            rows.push(field(&format!("{path}@{}", local(k)), &crate::text::norm(v)));
        }
    }
    let text = crate::text::norm(&el.text);
    if !text.is_empty() {
        rows.push(field(&path, &text));
    }
    for (i, c) in el.children.iter().enumerate() {
        let name = local(&c.name);
        let same = el.children.iter().filter(|o| local(&o.name) == name).count();
        let nth = el.children[..i].iter().filter(|o| local(&o.name) == name).count();
        let p = if same > 1 { format!("{path}.{name}[{nth}]") } else { format!("{path}.{name}") };
        xml_walk(c, p, rows);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_is_told_by_its_type_and_first_characters() {
        assert_eq!(kind("application/json; charset=utf-8", "{}"), Some(Kind::Json));
        assert_eq!(kind("application/rss+xml", "<rss/>"), Some(Kind::Feed));
        assert_eq!(kind("application/xml", "<?xml version=\"1.0\"?><feed xmlns=\"http://www.w3.org/2005/Atom\">"), Some(Kind::Feed));
        assert_eq!(kind("text/xml", "<catalog/>"), Some(Kind::Xml));
        assert_eq!(kind("text/plain", "{\"a\": 1}"), Some(Kind::Json), "JSON served as text");
        assert_eq!(kind("text/csv", "a,b\n1,2"), Some(Kind::Text));
        assert_eq!(kind("application/pdf", "%PDF"), None);
        assert_eq!(kind("image/png", ""), None);
        assert_eq!(kind("image/svg+xml", "<svg/>"), None, "SVG is XML, but a picture");
    }

    #[test]
    fn json_is_one_line_per_value_with_its_path() {
        let page = page(Kind::Json, r#"{"tag_name": "1.99.0", "assets": [{"name": "a.tar.gz", "size": 1200}], "draft": false}"#).unwrap();
        assert!(is_document(&page) && !is_feed(&page));
        assert_eq!(lines(&page).unwrap(), vec!["assets[0].name: a.tar.gz", "assets[0].size: 1200", "draft: false", "tag_name: 1.99.0"], "fields in the order of their names, so an API that reorders them changes nothing");
        let places = crate::anchor::places(&page, 50, None);
        let tag = places.iter().find(|p| p.text == "1.99.0").expect("the value is a place");
        assert_eq!(tag.label, "tag_name", "with its path beside it");
        let moved = page_of(r#"{"name": "new first", "tag_name": "1.100.0"}"#);
        assert_eq!(crate::anchor::find(&moved, tag), crate::anchor::Found::Here { value: "1.100.0".into(), moved: true }, "found again by its path when fields come before it");
        assert!(json_rows("{oops").is_err());
    }

    fn page_of(json: &str) -> String {
        page(Kind::Json, json).unwrap()
    }

    #[test]
    fn a_feed_is_its_entries_rss_and_atom() {
        let rss = r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Blog</title><item><title>Rust 1.100 &amp; more</title><link>https://blog.example/1100</link><guid>1100</guid></item><item><title><![CDATA[Rust <1.99>]]></title><link>https://blog.example/199</link></item></channel></rss>"#;
        let page = page(Kind::Feed, rss).unwrap();
        assert!(is_feed(&page));
        assert_eq!(lines(&page).unwrap(), vec!["Rust 1.100 & more — https://blog.example/1100", "Rust <1.99> — https://blog.example/199"]);
        let atom = r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>Releases</title><entry><id>tag:1</id><title>v2.0</title><link rel="alternate" href="https://example.com/v2"/></entry></feed>"#;
        assert_eq!(lines(&page_of_feed(atom)).unwrap(), vec!["v2.0 — https://example.com/v2"]);
        assert!(feed_rows("<rss><channel><title>empty</title></channel></rss>").is_err());
    }

    fn page_of_feed(xml: &str) -> String {
        page(Kind::Feed, xml).unwrap()
    }

    #[test]
    fn xml_and_text_are_read_line_by_line() {
        let xml = r#"<catalog><book id="b1"><title>Rust</title><price>39.90</price></book><book id="b2"><title>Go</title><price>29.90</price></book></catalog>"#;
        assert_eq!(
            lines(&page(Kind::Xml, xml).unwrap()).unwrap(),
            vec!["catalog.book[0]@id: b1", "catalog.book[0].title: Rust", "catalog.book[0].price: 39.90", "catalog.book[1]@id: b2", "catalog.book[1].title: Go", "catalog.book[1].price: 29.90"]
        );
        let text = page(Kind::Text, "name,price\n\nwidget, 9.99 €\n<script>x</script>\n").unwrap();
        assert_eq!(lines(&text).unwrap(), vec!["name,price", "widget, 9.99 €", "<script>x</script>"], "kept as text, escaped on the page");
        assert!(!text.contains("<script>x"), "nothing of the document is markup on its page");
        assert!(page(Kind::Text, "\n\n").is_err());
    }
}
