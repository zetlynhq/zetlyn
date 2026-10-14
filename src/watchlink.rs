//! A link anybody pastes at zetlyn.com/watch, and what Zetlyn reads on it: the list on a page, a
//! feed, a table. Nothing of it is kept. Who wants it read again and again takes the source it
//! became, into a world of their own here or into Zetlyn on their own machine.
//!
//! Every address is asked as on a server: it and every place it redirects to on the public
//! internet, and no more of it than a page should be. A visitor's preview is limited per address
//! it comes from, and in all.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

/// What Zetlyn reads at a link.
pub struct Read {
    pub url: String,
    /// `the list on the page`, `the feed`, `the table`.
    pub what: String,
    pub title: String,
    pub count: u64,
    pub columns: Vec<String>,
    /// The first items: their title and the values of `columns`.
    pub rows: Vec<(String, Vec<String>)>,
    /// On a page: every list on it, by how many items it has, and which is shown.
    pub lists: Vec<usize>,
    pub pick: usize,
    /// What the source is called, and its source.yaml: what to take home.
    pub name: String,
    pub decl: String,
}
/// A page is no bigger than this; a link to more is not a page somebody watches.
const LIMIT: u64 = 8 << 20;

/// What a link answers, asked once, quickly, and only where it may be: it and every redirect on
/// the public internet, http or https.
pub fn get(url: &str) -> Result<String, String> {
    let agent = ureq::Agent::config_builder()
        .user_agent(crate::sourcedecl::AGENT)
        .timeout_global(Some(std::time::Duration::from_secs(15)))
        .max_redirects(0)
        .build()
        .new_agent();
    let mut at = url.trim().to_string();
    for _ in 0..6 {
        crate::outbound::public_target(&at)?;
        match agent.get(&at).call() {
            Ok(response) if response.status().is_redirection() => {
                let to = response.headers().get("location").and_then(|v| v.to_str().ok()).ok_or("a redirect that says not where")?;
                at = crate::fetch::absolute(&at, to);
            }
            Ok(mut response) => {
                return response.body_mut().with_config().limit(LIMIT).read_to_string().map_err(|e| format!("{at}: {e}"));
            }
            Err(e) => return Err(format!("{at}: {e}")),
        }
    }
    Err(format!("{url}: redirected too often"))
}

/// The `<title>` of a page, for what to call what is watched on it.
fn page_title(body: &str) -> Option<String> {
    let lower = body.to_lowercase();
    let start = lower.find("<title")?;
    let open = lower[start..].find('>')? + start + 1;
    let end = lower[open..].find("</title>")? + open;
    let t = body[open..end].split_whitespace().collect::<Vec<_>>().join(" ");
    let t = t.replace("&amp;", "&").replace("&#39;", "'").replace("&quot;", "\"");
    (!t.is_empty()).then(|| t.chars().take(120).collect())
}

/// A file by its own name: `country-codes.csv`.
fn file_name(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url).trim_end_matches('/');
    path.rsplit('/').next().filter(|n| !n.is_empty() && !n.contains(':')).map(str::to_string).unwrap_or_else(|| link_name(url))
}

/// A name for what is at a link when it says none: its host and path.
fn link_name(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    rest.trim_end_matches('/').chars().take(80).collect()
}

/// The source a link becomes, written in `dir`: the list on a page, read from that page alone, or
/// the feed or table it is. Its title, and what it is.
fn propose(url: &str, body: &str, dir: &Path, name: &str, pick: usize) -> Result<(String, String, Vec<usize>), String> {
    let lists: Vec<usize> = crate::web::candidates(body).iter().map(|c| c.count).collect();
    let page = body.trim_start().get(..512).unwrap_or(body.trim_start()).to_lowercase();
    let is_page = page.contains("<!doctype html") || page.contains("<html");
    // A page and a feed say what they are called; a table is called after its file.
    let title = if body.trim_start().starts_with('<') { page_title(body).unwrap_or_else(|| link_name(url)) } else { file_name(url) };
    if is_page {
        if lists.is_empty() {
            return Err("No list was found on that page: Zetlyn reads lists, feeds and tables, each item one entry. A page of prose has none.".into());
        }
        crate::guess::propose_page(url, body, dir, Some(name), pick.min(lists.len() - 1))?;
        return Ok((title, "the list on the page".into(), lists));
    }
    crate::guess::propose_body(url, body, dir, Some(name), None)?;
    let what = if body.trim_start().starts_with('<') { "the feed" } else { "the table" };
    Ok((title, what.into(), Vec::new()))
}

/// The source in `dir` read once, on this thread as on a server, from what `url` answered just now.
fn read_once(dir: &Path, url: &str, body: &str) -> Result<crate::source::Source, String> {
    let ds = crate::source::Source::open(dir)?;
    crate::fetch::guarding(|| crate::fetch::primed(url, body, || ds.run_with(false, false)))?;
    Ok(ds)
}

/// What Zetlyn reads at `url`: read into a scratch directory under `root` and gone after.
pub fn read(root: &Path, url: &str, pick: usize) -> Result<Read, String> {
    let url = with_scheme(url);
    let url = url.as_str();
    if url.len() <= "https://".len() || url.contains(char::is_whitespace) {
        return Err("That is not an address. One like news.ycombinator.com, or the address of a feed or a CSV file.".into());
    }
    let body = get(url)?;
    let name = crate::guess::page_name(url);
    let dir = root.join(".zetlyn").join("watch").join(format!("try-{}", crate::jwt::random()));
    let result = (|| {
        let (title, what, lists) = propose(url, &body, &dir, &format!("local/{name}"), pick)?;
        // Taken home, it is read again once a day, as somebody watching a page would want.
        let mut sd = crate::sourcedecl::SourceDecl::load(&dir)?;
        if sd.schedule.every.is_none() {
            sd.schedule.every = Some("1d".into());
            std::fs::write(dir.join(crate::sourcedecl::FILE), crate::yaml::to_string(&sd)?).map_err(|e| e.to_string())?;
        }
        let decl = std::fs::read_to_string(dir.join(crate::sourcedecl::FILE)).map_err(|e| e.to_string())?;
        let ds = read_once(&dir, url, &body)?;
        let q = crate::source::Query { limit: 12, ..Default::default() };
        let (count, hits, _) = ds.search(&q)?;
        // An item with nothing that reads as its title is named by the first thing it does say.
        let titled: Vec<(String, &crate::store::Hit)> = hits
            .iter()
            .map(|h| {
                let named = !h.title.is_empty() && !h.title.starts_with(url);
                let title = if named { h.title.clone() } else { h.fields.values().map(|v| v.display()).find(|v| !v.is_empty() && !v.starts_with('/') && !v.contains("://")).unwrap_or_default() };
                (title, h)
            })
            .collect();
        // Four columns at most, and none that only says the title again.
        let mut columns: Vec<String> = Vec::new();
        for (_, h) in &titled {
            for k in h.fields.keys() {
                let echoes = titled.iter().all(|(t, h)| h.fields.get(k).map(|v| v.display()).is_none_or(|v| v.is_empty() || v == *t || t.contains(&v)));
                if !columns.contains(k) && columns.len() < 4 && !echoes {
                    columns.push(k.clone());
                }
            }
        }
        let rows = titled.iter().map(|(t, h)| (t.clone(), columns.iter().map(|c| h.fields.get(c).map(|v| v.display()).unwrap_or_default()).collect())).collect();
        Ok(Read { url: url.to_string(), what, title, count, columns, rows, lists, pick, name: name.clone(), decl })
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// An address as somebody types it, `news.ycombinator.com` as much as `https://…`.
pub fn with_scheme(url: &str) -> String {
    let url = url.trim();
    if url.starts_with("https://") || url.starts_with("http://") {
        url.to_string()
    } else {
        format!("https://{}", url.trim_start_matches('/'))
    }
}

/// Previews asked per address in the last hour, and in all: a visitor reading a link is a
/// request this server makes, and it makes no more of them than a person would ask for.
fn allowed_now(from: &str) -> Result<(), String> {
    static ASKED: Mutex<BTreeMap<String, Vec<i64>>> = Mutex::new(BTreeMap::new());
    let now = crate::now();
    let mut asked = ASKED.lock().unwrap_or_else(|e| e.into_inner());
    asked.retain(|_, times| {
        times.retain(|t| now - t < 3600);
        !times.is_empty()
    });
    let all: usize = asked.values().map(Vec::len).sum();
    let mine = asked.get(from).map(Vec::len).unwrap_or(0);
    if mine >= 20 || all >= 300 {
        return Err("That is as many links as can be read for now. Try again in an hour.".into());
    }
    asked.entry(from.to_string()).or_default().push(now);
    Ok(())
}

/// A visitor's preview: as `read`, within what may be asked from where they are.
pub fn preview(root: &Path, from: &str, url: &str, pick: usize) -> Result<Read, String> {
    allowed_now(from)?;
    read(root, url, pick)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_page_says_what_it_is_called() {
        assert_eq!(super::page_title("<html><head><title>  Top &amp; new\n stories </title></head>").as_deref(), Some("Top & new stories"));
        assert_eq!(super::link_name("https://example.org/list/"), "example.org/list");
        assert_eq!(super::file_name("https://example.org/data/country-codes.csv?x=1"), "country-codes.csv");
    }

    #[test]
    fn a_visitor_reads_no_more_links_than_a_person_would() {
        let from = format!("test-{}", std::process::id());
        for _ in 0..20 {
            super::allowed_now(&from).unwrap();
        }
        assert!(super::allowed_now(&from).is_err());
    }

    #[test]
    fn nothing_on_the_server_s_own_network_is_read() {
        assert!(super::get("http://127.0.0.1:9/").unwrap_err().contains("not on the public internet"));
        assert!(super::get("http://169.254.169.254/").is_err());
    }
}
