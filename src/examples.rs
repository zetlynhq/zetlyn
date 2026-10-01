//! The example the docs begin with: two bookshops' price lists, served by the hub.
//!
//! Two shops sell mostly the same books and keep their lists their own way. One writes a plain CSV
//! with `isbn,title,price,in_stock`; the other a German spreadsheet with `EAN;Titel;Preis (EUR);
//! Lieferbar;Verlag`, a decimal comma and ja or nein, the ISBN without hyphens and `Nineteen
//! Eighty-Four` for `1984`. They agree on most prices and not on three, and on whether one book
//! is in stock; Zetlyn pairs `Preis (EUR)` with `price` by what the two say. That is what a
//! tracker shows the moment the second list is connected.
//!
//! The second shop's list is live. Every two minutes it changes as a shop's does: a price goes
//! up, a book sells out, a new one arrives, and two minutes later it is back. So the other half
//! of what Zetlyn does, saying what changed since the last look, is seen in the same sitting.

const SHOP_A: &str = "isbn,title,author,price,in_stock
978-0-451-52493-5,1984,George Orwell,9.99,yes
978-0-7432-7356-5,The Great Gatsby,F. Scott Fitzgerald,10.99,yes
978-0-06-112008-4,To Kill a Mockingbird,Harper Lee,12.49,yes
978-0-14-143951-8,Pride and Prejudice,Jane Austen,7.99,yes
978-0-316-76948-8,The Catcher in the Rye,J. D. Salinger,9.49,no
978-0-441-17271-9,Dune,Frank Herbert,11.99,yes
978-0-06-231609-7,Sapiens,Yuval Noah Harari,16.99,yes
978-0-374-53355-7,\"Thinking, Fast and Slow\",Daniel Kahneman,14.99,yes
978-0-261-10221-7,The Hobbit,J. R. R. Tolkien,10.49,yes
978-0-553-41802-6,The Martian,Andy Weir,10.99,yes
";

/// The shop's list in the two minutes that contain `now`, in seconds. A shop that keeps its list
/// in a German spreadsheet: semicolons, its own column names, a decimal comma, ja and nein, and
/// a column the other shop has not got.
fn shop_b(now: i64) -> String {
    let later = (now / 120) % 2 == 1;
    let mut rows = vec![
        ("9780451524935", "Nineteen Eighty-Four", "9,99", "ja", "Signet"),
        ("9780743273565", "The Great Gatsby", "10,99", "ja", "Scribner"),
        ("9780061120084", "To Kill a Mockingbird", "17,99", "ja", "Harper Perennial"),
        ("9780141439518", "Pride & Prejudice", "7,99", "ja", "Penguin Classics"),
        ("9780316769488", "The Catcher in the Rye", "9,49", "ja", "Little, Brown"),
        ("9780441172719", "Dune", if later { "13,49" } else { "11,99" }, "ja", "Ace"),
        ("9780062316097", "Sapiens: A Brief History of Humankind", "22,00", "ja", "Harper"),
        ("9780374533557", "Thinking, Fast and Slow", "14,99", if later { "ja" } else { "nein" }, "Farrar, Straus and Giroux"),
        ("9780747532699", "Harry Potter and the Philosopher's Stone", "8,99", "ja", "Bloomsbury"),
        ("9780307474278", "The Da Vinci Code", "9,99", "ja", "Anchor"),
    ];
    if later {
        rows.push(("9780553418026", "The Martian", "12,99", "ja", "Crown"));
    }
    let field = |s: &str| if s.contains([';', '"']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
    let mut out = String::from("EAN;Titel;Preis (EUR);Lieferbar;Verlag\n");
    for (ean, title, price, available, publisher) in rows {
        out.push_str(&format!("{ean};{};{price};{available};{}\n", field(title), field(publisher)));
    }
    out
}

/// `examples/bookshop-a.csv` and `examples/bookshop-b.csv`.
pub fn file(path: &str) -> Option<String> {
    match path {
        "examples/bookshop-a.csv" => Some(SHOP_A.to_string()),
        "examples/bookshop-b.csv" => Some(shop_b(crate::now())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_second_shop_changes_every_two_minutes_and_changes_back() {
        let (now, later, again) = (super::shop_b(0), super::shop_b(120), super::shop_b(240));
        assert_eq!(now, again);
        assert!(now.contains("Dune;11,99") && later.contains("Dune;13,49"));
        assert!(!now.contains("The Martian") && later.contains("The Martian"));
    }
}
