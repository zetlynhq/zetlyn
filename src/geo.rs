//! The world, for maps: every country's outline and the names it goes by, from Natural Earth's
//! countries at 1:50m (public domain, see NOTICE), and the arithmetic that puts a place on a page.
//!
//! One projection for every map: longitude and latitude as they are, longitude narrowed by the
//! cosine of the middle latitude of what is shown. A map of France is then France's shape and a
//! map of the world is the world's, without a choice anybody has to make.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value as J;

const SHAPES: &[u8] = include_bytes!("geo/countries.json.gz");

pub struct Country {
    /// ISO 3166 three letters, or Natural Earth's own where a place has none (Kosovo is XKX here).
    pub iso3: String,
    pub name: String,
    /// Rings of points, longitude and latitude: outer rings and holes alike, drawn even-odd.
    pub rings: Vec<Vec<(f64, f64)>>,
    /// West, south, east, north.
    pub bounds: (f64, f64, f64, f64),
}

struct World {
    countries: Vec<Country>,
    /// Every name and code a country goes by, lower case, to its three letters.
    names: HashMap<String, String>,
}

fn world() -> &'static World {
    static WORLD: OnceLock<World> = OnceLock::new();
    WORLD.get_or_init(|| {
        use std::io::Read;
        let mut text = String::new();
        let _ = flate2::read::GzDecoder::new(SHAPES).read_to_string(&mut text);
        let all: J = serde_json::from_str(&text).unwrap_or(J::Null);
        let mut countries = Vec::new();
        let mut names = HashMap::new();
        for f in all["features"].as_array().into_iter().flatten() {
            let p = &f["p"];
            let code = |k: &str| p[k].as_str().filter(|s| s.len() == 3 && *s != "-99").map(str::to_string);
            let Some(iso3) = code("iso3").or_else(|| code("a3")) else { continue };
            let mut rings = Vec::new();
            let polygons: Vec<&J> = match f["g"]["type"].as_str() {
                Some("Polygon") => vec![&f["g"]["coordinates"]],
                Some("MultiPolygon") => f["g"]["coordinates"].as_array().map(|a| a.iter().collect()).unwrap_or_default(),
                _ => Vec::new(),
            };
            for polygon in polygons {
                for ring in polygon.as_array().into_iter().flatten() {
                    let points: Vec<(f64, f64)> = ring
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|pt| Some((pt[0].as_f64()?, pt[1].as_f64()?)))
                        .collect();
                    if points.len() > 2 {
                        rings.push(points);
                    }
                }
            }
            let mut bounds = (180.0f64, 90.0f64, -180.0f64, -90.0f64);
            for &(x, y) in rings.iter().flatten() {
                bounds = (bounds.0.min(x), bounds.1.min(y), bounds.2.max(x), bounds.3.max(y));
            }
            for k in ["iso3", "iso2", "a3", "name", "long", "formal", "de", "fr"] {
                if let Some(s) = p[k].as_str().filter(|s| !s.is_empty() && *s != "-99") {
                    names.entry(s.trim().to_lowercase()).or_insert_with(|| iso3.clone());
                }
            }
            countries.push(Country { name: p["name"].as_str().unwrap_or(&iso3).to_string(), iso3, rings, bounds });
        }
        // The spellings the lists in this program use that Natural Earth does not.
        for (alias, iso3) in ALIASES {
            names.entry(alias.to_string()).or_insert_with(|| iso3.to_string());
        }
        World { countries, names }
    })
}

/// Names lists use for a country where Natural Earth uses another: the UN's, OFAC's, the UK's.
const ALIASES: &[(&str, &str)] = &[
    ("russian federation", "RUS"),
    ("iran (islamic republic of)", "IRN"),
    ("syrian arab republic", "SYR"),
    ("democratic people's republic of korea", "PRK"),
    ("korea, north", "PRK"),
    ("north korea", "PRK"),
    ("korea, south", "KOR"),
    ("republic of korea", "KOR"),
    ("venezuela (bolivarian republic of)", "VEN"),
    ("bolivia (plurinational state of)", "BOL"),
    ("lao people's democratic republic", "LAO"),
    ("viet nam", "VNM"),
    ("türkiye", "TUR"),
    ("turkiye", "TUR"),
    ("united states of america", "USA"),
    ("usa", "USA"),
    ("uk", "GBR"),
    ("el", "GRC"),
    ("burma", "MMR"),
    ("congo, democratic republic of the", "COD"),
    ("democratic republic of the congo", "COD"),
    ("palestinian territories", "PSE"),
    ("occupied palestinian territory", "PSE"),
    ("west bank and gaza", "PSE"),
    ("kosovo", "XKX"),
    ("czech republic", "CZE"),
    ("moldova, republic of", "MDA"),
    ("united republic of tanzania", "TZA"),
];

/// A country as three letters, from a code or a name in the spellings above. None for what is
/// no country (an aggregate, a region, nothing).
pub fn iso3_of(said: &str) -> Option<String> {
    let s = said.trim().to_lowercase();
    if s.is_empty() {
        return None;
    }
    let w = world();
    if let Some(iso3) = w.names.get(&s) {
        return Some(iso3.clone());
    }
    // Two letters as the country schemes write them (Eurostat's EL and UK included).
    let three = crate::schemes::country(said);
    w.names.get(&three.to_lowercase()).cloned()
}

pub fn country(iso3: &str) -> Option<&'static Country> {
    world().countries.iter().find(|c| c.iso3 == iso3)
}

pub fn countries() -> &'static [Country] {
    &world().countries
}

/// Where a map puts things: the extent shown, fitted into a width and height.
pub struct Frame {
    west: f64,
    north: f64,
    /// Units of the page per degree of latitude, and of longitude after narrowing.
    scale: f64,
    narrow: f64,
    pub width: f64,
    pub height: f64,
}

impl Frame {
    /// The extent (west, south, east, north) with a margin, fitted into `width`; the height
    /// follows the extent's shape.
    pub fn fit(bounds: (f64, f64, f64, f64), width: f64) -> Frame {
        let (w, s, e, n) = bounds;
        let pad_x = ((e - w) * 0.04).max(0.2);
        let pad_y = ((n - s) * 0.04).max(0.2);
        let (w, s, e, n) = ((w - pad_x).max(-180.0), (s - pad_y).max(-85.0), (e + pad_x).min(180.0), (n + pad_y).min(85.0));
        let narrow = ((s + n) / 2.0).to_radians().cos().max(0.2);
        let scale = width / ((e - w) * narrow);
        let height = (n - s) * scale;
        Frame { west: w, north: n, scale, narrow, width, height }
    }

    pub fn point(&self, lon: f64, lat: f64) -> (f64, f64) {
        ((lon - self.west) * self.narrow * self.scale, (self.north - lat) * self.scale)
    }

    /// A country's outline as an SVG path, leaving out points closer than half a unit to the last
    /// one kept: a coastline of thousands of points is a few hundred at the size it is drawn.
    pub fn path(&self, c: &Country) -> String {
        let mut d = String::new();
        for ring in &c.rings {
            let mut last: Option<(f64, f64)> = None;
            for (i, &(lon, lat)) in ring.iter().enumerate() {
                let (x, y) = self.point(lon, lat);
                if let Some((lx, ly)) = last {
                    if (x - lx).abs() < 0.5 && (y - ly).abs() < 0.5 && i + 1 != ring.len() {
                        continue;
                    }
                }
                d.push_str(&format!("{}{:.1},{:.1}", if last.is_none() { "M" } else { "L" }, x, y));
                last = Some((x, y));
            }
            d.push('Z');
        }
        d
    }

    /// Whether a country's extent meets the frame at all.
    pub fn shows(&self, c: &Country) -> bool {
        let (x0, y0) = self.point(c.bounds.0, c.bounds.3);
        let (x1, y1) = self.point(c.bounds.2, c.bounds.1);
        x1 >= 0.0 && x0 <= self.width && y1 >= 0.0 && y0 <= self.height
    }
}

/// "48.183, 3.309", "48.183 3.309" or two numbers: latitude and longitude, where they make one.
pub fn place_of(said: &str) -> Option<(f64, f64)> {
    let mut parts = said.split(|c: char| c == ',' || c.is_whitespace()).filter(|p| !p.is_empty());
    let lat: f64 = parts.next()?.parse().ok()?;
    let lon: f64 = parts.next()?.parse().ok()?;
    ((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)).then_some((lat, lon))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_country_is_found_by_any_name_the_lists_use() {
        for (said, iso3) in [("France", "FRA"), ("FR", "FRA"), ("FRA", "FRA"), ("Russian Federation", "RUS"), ("Russia", "RUS"),
                             ("Deutschland", "DEU"), ("EL", "GRC"), ("UK", "GBR"), ("Iran (Islamic Republic of)", "IRN"), ("North Korea", "PRK")] {
            assert_eq!(super::iso3_of(said).as_deref(), Some(iso3), "{said}");
        }
        assert_eq!(super::iso3_of("EU27_2020"), None);
        assert!(super::country("FRA").is_some_and(|c| c.bounds.0 < 0.0 && c.bounds.3 > 50.0));
    }

    #[test]
    fn a_place_is_read_and_drawn_where_it_is() {
        assert_eq!(super::place_of("48.183, 3.309"), Some((48.183, 3.309)));
        assert_eq!(super::place_of("not a place"), None);
        let f = super::Frame::fit((-5.0, 41.0, 10.0, 51.5), 800.0);
        let (x, y) = f.point(2.35, 48.86);
        assert!(x > 300.0 && x < 500.0 && y > 50.0 && y < 300.0, "{x},{y}");
        assert!(f.path(super::country("FRA").unwrap()).len() > 500);
    }
}

// -- maps drawn once ------------------------------------------------------------------------------

/// A map made once from Natural Earth with mapshaper, in a projection fit for what it shows: the
/// world in Equal Earth at 1:110m, Europe in its own equal-area projection (EPSG:3035) at 1:50m.
/// Neighbours share their borders, so no edge frays where two meet. Countries too small to see are
/// a point, drawn where there is something to say of them.
pub struct Atlas {
    pub width: f64,
    pub height: f64,
    pub shapes: Vec<Shape>,
}

pub struct Shape {
    pub iso3: String,
    /// An outline's path, or a small country's point.
    pub path: Option<String>,
    pub point: Option<(f64, f64)>,
}

const WORLD_SVG: &str = include_str!("geo/world.svg");
const EUROPE_SVG: &str = include_str!("geo/europe.svg");

fn attr<'a>(element: &'a str, name: &str) -> Option<&'a str> {
    let at = element.find(&format!(" {name}=\""))? + name.len() + 3;
    let rest = &element[at..];
    Some(&rest[..rest.find('"')?])
}

fn atlas_of(svg: &str) -> Atlas {
    let view: Vec<f64> = attr(svg, "viewBox").unwrap_or("0 0 900 450").split_whitespace().filter_map(|n| n.parse().ok()).collect();
    let mut shapes = Vec::new();
    for element in svg.split('<').skip(1) {
        let Some(id) = attr(element, "id").filter(|i| i.len() == 3) else { continue };
        if element.starts_with("path ") {
            shapes.push(Shape { iso3: id.to_string(), path: attr(element, "d").map(str::to_string), point: None });
        } else if element.starts_with("circle ") {
            let n = |a: &str| attr(element, a).and_then(|v| v.parse::<f64>().ok());
            if let (Some(x), Some(y)) = (n("cx"), n("cy")) {
                shapes.push(Shape { iso3: id.to_string(), path: None, point: Some((x, y)) });
            }
        }
    }
    Atlas { width: view.get(2).copied().unwrap_or(900.0), height: view.get(3).copied().unwrap_or(450.0), shapes }
}

/// `world` or `europe`.
pub fn atlas(which: &str) -> &'static Atlas {
    static WORLD_ATLAS: OnceLock<Atlas> = OnceLock::new();
    static EUROPE_ATLAS: OnceLock<Atlas> = OnceLock::new();
    match which {
        "europe" => EUROPE_ATLAS.get_or_init(|| atlas_of(EUROPE_SVG)),
        _ => WORLD_ATLAS.get_or_init(|| atlas_of(WORLD_SVG)),
    }
}

#[cfg(test)]
mod atlas_tests {
    #[test]
    fn the_maps_name_every_country_by_its_three_letters() {
        let w = super::atlas("world");
        assert!(w.shapes.iter().any(|s| s.iso3 == "DEU" && s.path.is_some()));
        assert!(w.shapes.iter().any(|s| s.iso3 == "SGP" && s.point.is_some()));
        assert!(w.shapes.iter().any(|s| s.iso3 == "FRA"), "France, which Natural Earth gives -99 as its ISO code");
        let e = super::atlas("europe");
        assert!(e.shapes.iter().any(|s| s.iso3 == "MLT" && s.point.is_some()));
        assert!(e.width > 800.0 && e.height > 500.0);
    }
}
