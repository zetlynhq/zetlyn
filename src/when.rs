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
            Condition::Below(mark) => the_number(value).ok().flatten().is_some_and(|n| n < *mark),
            Condition::Above(mark) => the_number(value).ok().flatten().is_some_and(|n| n > *mark),
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

    /// Whether the condition can be weighed on what the place says now: a number condition on a
    /// place with two different amounts cannot, since which one is meant is not clear.
    pub fn fits(&self, value: &str) -> Result<(), String> {
        match (self.needs_number(), the_number(value)) {
            (true, Err(amounts)) => Err(format!(
                "The place shows several amounts ({}), so it is not clear which one to compare. Mark a narrower place: the one amount you mean.",
                amounts.join(", ")
            )),
            _ => Ok(()),
        }
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
    numbers(s).first().map(|n| n.0)
}

/// The number a condition compares: the amount of money in the text where there is one (a number
/// beside €, $, EUR and the like), else its first number. Two different amounts are not one
/// number: which of them is meant ("RRP 999 € now 799 €") is the person's to say, by marking a
/// narrower place. None where the text has no number.
pub fn the_number(s: &str) -> Result<Option<f64>, Vec<String>> {
    let found = numbers(s);
    let chars: Vec<char> = s.chars().collect();
    let mut amounts: Vec<(f64, String)> = Vec::new();
    for (n, start, end) in &found {
        if money_beside(&chars, *start, *end) && !amounts.iter().any(|(m, _)| m == n) {
            amounts.push((*n, chars[*start..*end].iter().collect::<String>().trim().to_string()));
        }
    }
    match amounts.len() {
        0 => Ok(found.first().map(|n| n.0)),
        1 => Ok(Some(amounts[0].0)),
        _ => Err(amounts.into_iter().map(|(_, said)| said).collect()),
    }
}

/// Whether a text says an amount of money: a number beside a currency.
pub fn money_in(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    numbers(s).iter().any(|(_, start, end)| money_beside(&chars, *start, *end))
}

/// Every number in a text, with where it starts and ends (in chars).
fn numbers(s: &str) -> Vec<(f64, usize, usize)> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        let mut run = String::new();
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
        if let Some(n) = parse_run(&run) {
            out.push((n, start, i));
        }
    }
    out
}

/// A run of digits and separators as a number (see `number_in`).
fn parse_run(run: &str) -> Option<f64> {
    let (dots, commas) = (run.matches('.').count(), run.matches(',').count());
    let normal = if dots > 0 && commas > 0 {
        let decimal = if run.rfind('.') > run.rfind(',') { '.' } else { ',' };
        let thousands = if decimal == '.' { ',' } else { '.' };
        run.replace(thousands, "").replace(decimal, ".")
    } else if dots + commas == 0 {
        run.to_string()
    } else {
        let sep = if dots > 0 { '.' } else { ',' };
        let after = run.rsplit(sep).next().unwrap_or("").len();
        let before = run.split(sep).next().unwrap_or("");
        if dots + commas > 1 || (after == 3 && before != "0") { run.replace(sep, "") } else { run.replace(sep, ".") }
    };
    normal.parse().ok()
}

/// Whether a currency stands right before or after the number at `start..end`: a sign
/// (€, $, £ …) or a code (EUR, USD, CHF …), with at most a space between.
fn money_beside(chars: &[char], start: usize, end: usize) -> bool {
    const SIGNS: &[char] = &['€', '$', '£', '¥', '₹', '₽', '₺', '₩', '₪', '₫', '₴', '฿', '₣', '₱'];
    const CODES: &[&str] = &["EUR", "USD", "GBP", "CHF", "JPY", "CNY", "SEK", "NOK", "DKK", "PLN", "CZK", "HUF", "CAD", "AUD", "INR", "BRL", "TRY", "kr", "zł", "Fr", "Kč"];
    let space = |c: char| matches!(c, ' ' | '\u{a0}' | '\u{202f}');
    let mut b = start;
    while b > 0 && space(chars[b - 1]) && start - b < 2 {
        b -= 1;
    }
    let before: String = chars[b.saturating_sub(3)..b].iter().collect();
    let mut a = end;
    // "499,-" and "12.-": the dash that stands for no cents.
    if chars.get(a).is_some_and(|c| *c == '-' || *c == '–') {
        a += 1;
    }
    while a < chars.len() && space(chars[a]) && a - end < 3 {
        a += 1;
    }
    let after: String = chars[a..(a + 3).min(chars.len())].iter().collect();
    before.chars().last().is_some_and(|c| SIGNS.contains(&c))
        || after.chars().next().is_some_and(|c| SIGNS.contains(&c))
        || CODES.iter().any(|code| before.ends_with(code) || (after.starts_with(code) && !after[code.len()..].starts_with(|c: char| c.is_alphabetic())))
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

    #[test]
    fn the_amount_of_money_is_the_number_and_two_amounts_are_none() {
        assert_eq!(the_number("3 Angebote ab 699 €"), Ok(Some(699.0)), "not the count of offers");
        assert_eq!(the_number("Nur noch 2 Stück auf Lager: 1.299,00 €"), Ok(Some(1299.0)));
        assert_eq!(the_number("$1,299.99 (3 colors)"), Ok(Some(1299.99)));
        assert_eq!(the_number("EUR 49,90 inkl. MwSt."), Ok(Some(49.9)));
        assert_eq!(the_number("CHF 12.50"), Ok(Some(12.5)));
        assert_eq!(the_number("499,- €"), Ok(Some(499.0)));
        assert_eq!(the_number("799 € 799 €"), Ok(Some(799.0)), "the same amount twice is one");
        assert_eq!(the_number("Rang 5 von 20"), Ok(Some(5.0)), "no money: the first number, as before");
        assert_eq!(the_number("Currently unavailable"), Ok(None));
        assert_eq!(the_number("UVP 999 € jetzt 799 €"), Err(vec!["999".to_string(), "799".to_string()]));
        assert_eq!(the_number("Free shipping from 5 Fr"), Ok(Some(5.0)));
        assert_eq!(the_number("Fresh 12 items"), Ok(Some(12.0)), "Fresh is no franc");
        let below = Condition::Below(700.0);
        assert!(below.met("3 Angebote ab 699 €"));
        assert!(!below.met("UVP 999 € jetzt 799 €"), "unclear is not met");
        assert!(below.fits("UVP 999 € jetzt 799 €").unwrap_err().contains("999, 799"));
        assert!(below.fits("ab 699 €").is_ok());
        assert!(Condition::Contains("x".into()).fits("UVP 999 € jetzt 799 €").is_ok(), "a text condition minds no amounts");
    }
}
