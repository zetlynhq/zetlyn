//! A page as a picture, for where a shared link shows one: 1200 by 630, what the page is, its marks
//! and what it is said to be with who says it. Drawn as SVG, as every picture here is, and made a
//! PNG by resvg with the fonts carried in the binary (Inter, IBM Plex Mono; OFL, see NOTICE), so
//! it looks the same on every machine and reads none of the system's.
//!
//! Kept as files beside the tracker, one folder per standing of its store: drawn once for what it
//! holds, and the folder of an earlier standing gone when a new one is made.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

const INTER: &[u8] = include_bytes!("fonts/Inter-Regular.ttf");
const INTER_SEMIBOLD: &[u8] = include_bytes!("fonts/Inter-SemiBold.ttf");
const MONO: &[u8] = include_bytes!("fonts/IBMPlexMono-Medium.ttf");

pub const WIDTH: u32 = 1200;
pub const HEIGHT: u32 = 630;

/// What a card says.
#[derive(Default, Clone)]
pub struct Card {
    /// Above the title, small: the tracker it is in.
    pub overline: String,
    pub title: String,
    /// Marks, each `(name, hot)`.
    pub badges: Vec<(String, bool)>,
    /// Up to four: label, value, who says it.
    pub rows: Vec<(String, String, String)>,
    /// One line under it all: the sources, how many.
    pub footer: String,
}

fn fonts() -> Arc<resvg::usvg::fontdb::Database> {
    static DB: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = resvg::usvg::fontdb::Database::new();
        for f in [INTER, INTER_SEMIBOLD, MONO] {
            db.load_font_data(f.to_vec());
        }
        db.set_sans_serif_family("Inter");
        db.set_monospace_family("IBM Plex Mono");
        Arc::new(db)
    })
    .clone()
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Words into lines of at most `width` characters, at most `lines` of them, the last ending in an
/// ellipsis where there was more.
fn wrap(s: &str, width: usize, lines: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in s.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push(line);
    }
    if out.len() > lines {
        out.truncate(lines);
        if let Some(last) = out.last_mut() {
            let kept: String = last.chars().take(width.saturating_sub(1)).collect();
            *last = format!("{}…", kept.trim_end());
        }
    }
    out
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n - 1).collect::<String>().trim_end())
    } else {
        s.to_string()
    }
}

/// The card as SVG.
pub fn svg(c: &Card) -> String {
    let (bg, fg, dim, line, accent, panel) = ("#f2efe7", "#14202a", "#667078", "#cfd1ca", "#dc4a20", "#fbfaf6");
    let mut s = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{HEIGHT}" viewBox="0 0 {WIDTH} {HEIGHT}"><rect width="{WIDTH}" height="{HEIGHT}" fill="{bg}"/><rect x="0" y="0" width="{WIDTH}" height="10" fill="{accent}"/>"#);
    s.push_str(&format!(r#"<text x="64" y="86" font-family="IBM Plex Mono" font-size="22" letter-spacing="3" fill="{accent}">{}</text>"#, escape(&clip(&c.overline.to_uppercase(), 60))));
    let title = wrap(&c.title, 34, 2);
    let mut y: f64 = 160.0;
    for l in &title {
        s.push_str(&format!(r#"<text x="64" y="{y}" font-family="Inter" font-weight="600" font-size="58" fill="{fg}">{}</text>"#, escape(l)));
        y += 68.0;
    }
    // The marks, side by side, as wide as their words.
    let mut x = 64.0;
    let by = y - 16.0;
    if !c.badges.is_empty() {
        for (name, hot) in c.badges.iter().take(4) {
            let w = name.chars().count() as f64 * 12.2 + 32.0;
            if x + w > (WIDTH - 64) as f64 {
                break;
            }
            let (fill, stroke, text) = if *hot { (accent, accent, "#ffffff") } else { ("none", fg, fg) };
            s.push_str(&format!(r#"<rect x="{x}" y="{by}" width="{w:.0}" height="40" fill="{fill}" stroke="{stroke}" stroke-width="2"/><text x="{:.0}" y="{:.0}" font-family="Inter" font-weight="600" font-size="22" fill="{text}">{}</text>"#, x + 16.0, by + 28.0, escape(name)));
            x += w + 12.0;
        }
        y += 54.0;
    }
    // What it is said to be: up to four in a row of panels.
    let rows: Vec<&(String, String, String)> = c.rows.iter().take(4).collect();
    if !rows.is_empty() {
        let top = if c.badges.is_empty() { (y + 24.0).max(250.0) } else { y.max(330.0) };
        let n = rows.len() as f64;
        let gap = 16.0;
        let w = ((WIDTH as f64 - 128.0) - gap * (n - 1.0)) / n;
        for (i, (label, value, source)) in rows.iter().enumerate() {
            let px = 64.0 + i as f64 * (w + gap);
            let chars = (w / 24.0) as usize;
            s.push_str(&format!(r#"<rect x="{px:.0}" y="{top:.0}" width="{w:.0}" height="170" fill="{panel}" stroke="{line}" stroke-width="2"/>"#));
            s.push_str(&format!(r#"<text x="{:.0}" y="{:.0}" font-family="IBM Plex Mono" font-size="18" letter-spacing="2" fill="{dim}">{}</text>"#, px + 22.0, top + 40.0, escape(&clip(&label.to_uppercase(), (w / 13.0) as usize))));
            let size = if value.chars().count() as f64 * 23.0 > w - 44.0 { 32 } else { 40 };
            s.push_str(&format!(r#"<text x="{:.0}" y="{:.0}" font-family="Inter" font-weight="600" font-size="{size}" fill="{fg}">{}</text>"#, px + 22.0, top + 98.0, escape(&clip(value, chars + 4))));
            s.push_str(&format!(r#"<text x="{:.0}" y="{:.0}" font-family="Inter" font-size="20" fill="{dim}">{}</text>"#, px + 22.0, top + 140.0, escape(&clip(source, (w / 11.0) as usize))));
        }
    }
    s.push_str(&format!(r#"<line x1="64" y1="560" x2="{}" y2="560" stroke="{line}" stroke-width="2"/>"#, WIDTH - 64));
    s.push_str(&format!(r#"<text x="64" y="598" font-family="Inter" font-size="22" fill="{dim}">{}</text>"#, escape(&clip(&c.footer, 78))));
    s.push_str(&format!(r#"<text x="{}" y="598" font-family="Inter" font-weight="600" font-size="26" fill="{fg}" text-anchor="end">Zetlyn</text>"#, WIDTH - 64));
    s.push_str("</svg>");
    s
}

/// The card as PNG.
pub fn png(c: &Card) -> Result<Vec<u8>, String> {
    let mut opt = resvg::usvg::Options { fontdb: fonts(), ..Default::default() };
    opt.font_family = "Inter".into();
    let tree = resvg::usvg::Tree::from_str(&svg(c), &opt).map_err(|e| e.to_string())?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(WIDTH, HEIGHT).ok_or("no room for the picture")?;
    resvg::render(&tree, resvg::tiny_skia::Transform::default(), &mut pixmap.as_mut());
    pixmap.encode_png().map_err(|e| e.to_string())
}

/// Where the cards of a tracker are kept for what its store holds now.
fn folder(dir: &Path, standing: &str) -> PathBuf {
    let h = crate::place::sha256(standing.as_bytes());
    dir.join(".previews").join(&h[..16])
}

/// A card, drawn once per standing of the tracker's store and kept beside it; the folders of
/// earlier standings gone as a new one is made.
pub fn kept(dir: &Path, standing: &str, name: &str, card: impl FnOnce() -> Option<Card>) -> Option<Vec<u8>> {
    let here = folder(dir, standing);
    let file = here.join(format!("{}.png", crate::place::sha256(name.as_bytes())));
    if let Ok(b) = std::fs::read(&file) {
        return Some(b);
    }
    let made = png(&card()?).map_err(|e| eprintln!("{}: preview not drawn: {e}", dir.display())).ok()?;
    if !here.exists() {
        if let Ok(old) = std::fs::read_dir(dir.join(".previews")) {
            for e in old.flatten() {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
        let _ = std::fs::create_dir_all(&here);
    }
    let tmp = file.with_extension("tmp");
    if std::fs::write(&tmp, &made).is_ok() {
        let _ = std::fs::rename(&tmp, &file);
    }
    Some(made)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_card_is_a_png_of_its_size_with_its_words_in_it() {
        let c = super::Card {
            overline: "CVE · Zetlyn".into(),
            title: "CVE-2021-44228 · Apache Log4j2 Remote Code Execution Vulnerability".into(),
            badges: vec![("Exploited in the wild (CISA)".into(), true), ("Metasploit module".into(), false)],
            rows: vec![("CVSS".into(), "10.0".into(), "NVD".into()), ("EPSS".into(), "1.00".into(), "EPSS".into())],
            footer: "5 sources · what each says, and where they differ".into(),
        };
        let png = super::png(&c).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        // The width and height in the header, big-endian at 16 and 20.
        assert_eq!(u32::from_be_bytes([png[16], png[17], png[18], png[19]]), super::WIDTH);
        assert_eq!(u32::from_be_bytes([png[20], png[21], png[22], png[23]]), super::HEIGHT);
        assert!(super::svg(&c).contains("Log4j2"));
        assert_eq!(super::wrap("one two three four", 9, 1), vec!["one two…".to_string()]);
    }
}
