//! When a change is worth telling: on any change (no condition), or only when the value comes to
//! meet a condition a person set: a number below or above a mark, a text there or gone.
//!
//! A change is told when the value meets the condition now and did not before, so a price that
//! falls under the mark is told once, not again with every cent it falls further; once it has
//! risen above the mark, falling under it again is told again. What changes is still written
//! to the watch's history either way.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    Below(f64),
    Above(f64),
    Contains(String),
    NotContains(String),
}

impl Condition {
    /// Whether a value meets the condition. A number condition on a value without a number is
    /// not met.
    pub fn met(&self, value: &str) -> bool {
        match self {
            Condition::Below(mark) => number_in(value).is_some_and(|n| n < *mark),
            Condition::Above(mark) => number_in(value).is_some_and(|n| n > *mark),
            Condition::Contains(t) => folded(value).contains(&folded(t)),
            Condition::NotContains(t) => !folded(value).contains(&folded(t)),
        }
    }

    /// Whether going from `before` to `now` is the moment the condition came true.
    pub fn came_true(&self, before: &str, now: &str) -> bool {
        self.met(now) && !self.met(before)
    }

    pub fn needs_number(&self) -> bool {
        matches!(self, Condition::Below(_) | Condition::Above(_))
    }

    /// The condition said, as in "Tell me when the value is …".
    pub fn said(&self) -> String {
        match self {
            Condition::Below(n) => format!("below {}", plain(*n)),
            Condition::Above(n) => format!("above {}", plain(*n)),
            Condition::Contains(t) => format!("contains “{t}”"),
            Condition::NotContains(t) => format!("no longer contains “{t}”"),
        }
    }

    /// The moment it came true, said for a mail's subject.
    pub fn now_said(&self) -> String {
        match self {
            Condition::Below(n) => format!("Now below {}", plain(*n)),
            Condition::Above(n) => format!("Now above {}", plain(*n)),
            Condition::Contains(t) => format!("Now says “{}”", crate::mail::clip(t, 30)),
            Condition::NotContains(t) => format!("No longer says “{}”", crate::mail::clip(t, 30)),
        }
    }

    /// As the API writes it: `{"below": 400}` and the like.
    pub fn json(&self) -> J {
        match self {
            Condition::Below(n) => json!({ "below": number(*n) }),
            Condition::Above(n) => json!({ "above": number(*n) }),
            Condition::Contains(t) => json!({ "contains": t }),
            Condition::NotContains(t) => json!({ "not_contains": t }),
        }
    }

    /// As the API takes it: `{"below": 400}`, `{"above": 1000}`, `{"contains": "In stock"}`,
    /// `{"not_contains": "unavailable"}`; null or "any" for any change.
    pub fn from_json(j: &J) -> Result<Option<Condition>, String> {
        const HOW: &str = "notify_when is {\"below\": number}, {\"above\": number}, {\"contains\": \"text\"}, {\"not_contains\": \"text\"}, or null for any change.";
        if j.is_null() || j.as_str() == Some("any") {
            return Ok(None);
        }
        let Some(o) = j.as_object().filter(|o| o.len() == 1) else { return Err(HOW.into()) };
        let (k, v) = o.iter().next().ok_or(HOW)?;
        let number = || v.as_f64().or_else(|| v.as_str().and_then(number_in)).ok_or_else(|| format!("{k} takes a number. {HOW}"));
        let text = || v.as_str().map(crate::text::norm).filter(|t| !t.is_empty() && t.chars().count() <= 200).ok_or_else(|| format!("{k} takes a text of 1 to 200 characters. {HOW}"));
        Ok(Some(match k.as_str() {
            "below" => Condition::Below(number()?),
            "above" => Condition::Above(number()?),
            "contains" => Condition::Contains(text()?),
            "not_contains" => Condition::NotContains(text()?),
            _ => return Err(HOW.into()),
        }))
    }

    /// As a person sets it on a watch's page: what kind, and the number or text.
    pub fn from_form(kind: &str, value: &str) -> Result<Option<Condition>, String> {
        let value = value.trim();
        match kind {
            "" | "any" => Ok(None),
            "below" | "above" => {
                let n = number_in(value).ok_or("Give a number, such as 399 or 1.299,00.")?;
                Ok(Some(if kind == "below" { Condition::Below(n) } else { Condition::Above(n) }))
            }
            "contains" | "not_contains" => {
                let t = crate::text::norm(value);
                if t.is_empty() || t.chars().count() > 200 {
                    return Err("Give a text of 1 to 200 characters.".into());
                }
                Ok(Some(if kind == "contains" { Condition::Contains(t) } else { Condition::NotContains(t) }))
            }
            _ => Err("No such condition.".into()),
        }
    }

    /// The kind, as the page's form names it.
    pub fn kind(&self) -> &'static str {
        match self {
            Condition::Below(_) => "below",
            Condition::Above(_) => "above",
            Condition::Contains(_) => "contains",
            Condition::NotContains(_) => "not_contains",
        }
    }

    /// The number or text, as the page's form shows it.
    pub fn operand(&self) -> String {
        match self {
            Condition::Below(n) | Condition::Above(n) => plain(*n),
            Condition::Contains(t) | Condition::NotContains(t) => t.clone(),
        }
    }
}

/// A number for JSON: whole numbers without a fraction.
fn number(n: f64) -> J {
    if n.fract() == 0.0 && n.abs() < 1e15 { J::from(n as i64) } else { J::from(n) }
}

/// Text compared the way a reader reads it: case and runs of whitespace aside.
fn folded(s: &str) -> String {
    crate::text::norm(s).to_lowercase()
}

/// A number as a person writes it: 400, 399.5.
fn plain(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{n}") }
}

/// The first number in a text, as the page writes it: `1.299,00 €`, `$1,299.99`, `499,-`,
/// `19,99`, `1 299`. Where only one separator appears once, three digits after it make it a
/// thousands separator (`1.299` is 1299), fewer make it a decimal one (`19,99`).
pub fn number_in(s: &str) -> Option<f64> {
    let chars: Vec<char> = s.chars().collect();
    let start = chars.iter().position(|c| c.is_ascii_digit())?;
    let mut run = String::new();
    let mut i = start;
    while i < chars.len() {
        let c = chars[i];
        let next_digit = chars.get(i + 1).is_some_and(|n| n.is_ascii_digit());
        if c.is_ascii_digit() {
            run.push(c);
        } else if matches!(c, '.' | ',') && next_digit {
            run.push(c);
        } else if matches!(c, ' ' | '\u{a0}' | '\u{202f}' | '\'') && next_digit && chars.get(i + 1..i + 4).is_some_and(|g| g.iter().all(|d| d.is_ascii_digit())) && !chars.get(i + 4).is_some_and(|d| d.is_ascii_digit()) {
            // A space between groups of three: 1 299.
        } else {
            break;
        }
        i += 1;
    }
    let (dots, commas) = (run.matches('.').count(), run.matches(',').count());
    let normal = if dots > 0 && commas > 0 {
        let decimal = if run.rfind('.') > run.rfind(',') { '.' } else { ',' };
        let thousands = if decimal == '.' { ',' } else { '.' };
        run.replace(thousands, "").replace(decimal, ".")
    } else if dots + commas == 0 {
        run
    } else {
        let sep = if dots > 0 { '.' } else { ',' };
        let after = run.rsplit(sep).next().unwrap_or("").len();
        let before = run.split(sep).next().unwrap_or("");
        if dots + commas > 1 || (after == 3 && before != "0") { run.replace(sep, "") } else { run.replace(sep, ".") }
    };
    normal.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_number_is_read_as_the_page_writes_it() {
        assert_eq!(number_in("1.299,00 €"), Some(1299.0));
        assert_eq!(number_in("$1,299.99"), Some(1299.99));
        assert_eq!(number_in("ab 19,99 € / Monat"), Some(19.99));
        assert_eq!(number_in("499,-"), Some(499.0));
        assert_eq!(number_in("1.299"), Some(1299.0));
        assert_eq!(number_in("0.500 kg"), Some(0.5));
        assert_eq!(number_in("1 299 kr"), Some(1299.0));
        assert_eq!(number_in("Version 3.12.1"), Some(3121.0), "not a price; a version is watched with contains");
        assert_eq!(number_in("Currently unavailable."), None);
    }

    #[test]
    fn a_condition_is_told_when_it_comes_true_and_not_again_while_it_holds() {
        let below = Condition::Below(400.0);
        assert!(below.came_true("429,00 €", "399,00 €"));
        assert!(!below.came_true("399,00 €", "389,00 €"), "falling further is not told again");
        assert!(!below.came_true("429,00 €", "419,00 €"));
        assert!(below.came_true("Currently unavailable", "389,00 €"));
        let stock = Condition::Contains("in stock".into());
        assert!(stock.came_true("Currently unavailable.", "In  Stock. Add to basket"));
        assert!(!stock.came_true("In stock", "In stock, 3 left"));
        let gone = Condition::NotContains("Sold out".into());
        assert!(gone.came_true("Sold out", "Add to cart"));
    }

    #[test]
    fn a_condition_is_taken_as_the_api_and_the_page_write_it() {
        assert_eq!(Condition::from_json(&json!({ "below": 400 })).unwrap(), Some(Condition::Below(400.0)));
        assert_eq!(Condition::from_json(&json!({ "below": "1.299,00 €" })).unwrap(), Some(Condition::Below(1299.0)));
        assert_eq!(Condition::from_json(&json!({ "contains": "  In stock " })).unwrap(), Some(Condition::Contains("In stock".into())));
        assert_eq!(Condition::from_json(&J::Null).unwrap(), None);
        assert!(Condition::from_json(&json!({ "below": "cheap" })).is_err());
        assert!(Condition::from_json(&json!({ "below": 1, "above": 2 })).is_err());
        assert_eq!(Condition::from_form("above", "1000").unwrap(), Some(Condition::Above(1000.0)));
        assert_eq!(Condition::from_form("any", "").unwrap(), None);
        let c = Condition::Below(399.5);
        assert_eq!((c.said(), c.json()), ("below 399.5".to_string(), json!({ "below": 399.5 })));
        let stored = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Condition>(&stored).unwrap(), c);
    }
}
