//! The mails: the whole of Zetlyn once a watch is set up.
//!
//! Sent over SMTP when the instance is given a server (`SMTP_HOST`, `SMTP_PORT`, `SMTP_USER`,
//! `SMTP_PASSWORD_FILE`, `MAIL_FROM`); otherwise written to `outbox/` beside the instance's file,
//! one `.eml` each, so a local instance can be tried without mailing anybody.
//!
//! The SMTP client is the old Zetlyn's `mail.rs`: plain text in UTF-8, TLS from the first byte
//! (465) or after STARTTLS (587), AUTH PLAIN where there is a user, and nothing in the clear to
//! another machine.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Default, Clone)]
pub struct Smtp {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password_file: String,
}

/// Where mail goes: a server, or a folder.
#[derive(Debug, Clone)]
pub struct Mailer {
    pub from: String,
    pub smtp: Option<Smtp>,
    pub outbox: PathBuf,
}

impl Mailer {
    pub fn from_env(data: &Path) -> Mailer {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        let smtp = (!var("SMTP_HOST").is_empty()).then(|| Smtp { host: var("SMTP_HOST"), port: var("SMTP_PORT").parse().unwrap_or(465), user: var("SMTP_USER"), password_file: var("SMTP_PASSWORD_FILE") });
        let from = if var("MAIL_FROM").is_empty() { "Zetlyn <hello@zetlyn.com>".to_string() } else { var("MAIL_FROM") };
        Mailer { from, smtp, outbox: data.join("outbox") }
    }

    /// A mail sent, and noted in `<data>/mail.log` either way (`log`).
    pub fn send(&self, to: &str, letter: &Letter) -> Result<(), String> {
        let sent = self.deliver(to, letter);
        self.log(to, &letter.subject, &sent);
        sent
    }

    /// The mail log keeps 90 days (as the privacy page says): older lines go, and the log before
    /// it (`mail.log.1`) once all of it is older. Rewritten only when its first line is too old.
    pub fn prune_log(&self, now: i64) {
        let Some(dir) = self.outbox.parent() else { return };
        let keep_from = now - LOG_DAYS * 86400;
        let at = |line: &str| line.split('\t').next().and_then(|t| t.parse::<i64>().ok()).unwrap_or(0);
        let old = dir.join("mail.log.1");
        if let Ok(text) = std::fs::read_to_string(&old) {
            if text.lines().map(at).max().unwrap_or(0) < keep_from {
                let _ = std::fs::remove_file(&old);
            }
        }
        let path = dir.join("mail.log");
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        if text.lines().next().is_none_or(|l| at(l) >= keep_from) {
            return;
        }
        let kept: String = text.lines().filter(|l| at(l) >= keep_from).map(|l| format!("{l}\n")).collect();
        let fresh = dir.join("mail.log.new");
        if std::fs::write(&fresh, kept).is_ok() {
            let _ = std::fs::rename(&fresh, &path);
        }
    }

    /// One line per mail: when, `sent` or what went wrong, to whom, about what. Past 5 MB the log
    /// starts again, the old one kept as `mail.log.1`; past 90 days a line goes (`prune_log`).
    fn log(&self, to: &str, subject: &str, sent: &Result<(), String>) {
        let Some(dir) = self.outbox.parent() else { return };
        let path = dir.join("mail.log");
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 5 * 1024 * 1024) {
            let _ = std::fs::rename(&path, dir.join("mail.log.1"));
        }
        let flat = |s: &str| s.replace(['\t', '\r', '\n'], " ");
        let outcome = match sent {
            Ok(()) => "sent".to_string(),
            Err(e) => flat(e),
        };
        let line = format!("{}\t{}\t{}\t{}\n", crate::store::now(), outcome, flat(to), flat(subject));
        use std::io::Write;
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(&path).and_then(|mut f| f.write_all(line.as_bytes()));
    }

    fn deliver(&self, to: &str, letter: &Letter) -> Result<(), String> {
        if !plausible(to) {
            return Err(format!("{to}: not an address to send to"));
        }
        let message = compose(&self.from, to, &letter.subject, &letter.body);
        match &self.smtp {
            Some(s) => smtp(s, &self.from, to, &message),
            None => {
                std::fs::create_dir_all(&self.outbox).map_err(|e| e.to_string())?;
                let name = format!("{}-{}.eml", crate::store::now(), &crate::crypto::digest(&message)[..8]);
                std::fs::write(self.outbox.join(&name), message).map_err(|e| e.to_string())?;
                println!("mail to {to}: {} (outbox/{name})", letter.subject);
                Ok(())
            }
        }
    }
}

/// How long the mail log keeps a line, in days.
pub const LOG_DAYS: i64 = 90;

/// An address somebody may have typed: one `@`, no spaces or line breaks, not too long.
pub fn plausible(to: &str) -> bool {
    let to = to.trim();
    to.len() <= 254 && to.matches('@').count() == 1 && !to.starts_with('@') && !to.ends_with('@') && !to.contains(|c: char| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | ',' | ';')) && to.rsplit('@').next().is_some_and(|d| d.contains('.'))
}

pub struct Letter {
    pub subject: String,
    pub body: String,
}

pub const SIGNATURE: &str = "\n--\nZetlyn · https://zetlyn.com\n";

pub fn confirm(url: &str, what: &str, link: &str, note: &str) -> Letter {
    Letter {
        subject: format!("Confirm: watch {}", short(url)),
        body: format!(
            "Hello,\n\nplease confirm that you want to watch\n\n  {url}\n  {what}\n\nStart watching:\n\n  {link}\n\n{note}If you did not ask for this, ignore this mail; nothing is watched without the link.\n{SIGNATURE}"
        ),
    }
}

/// To the new address an account wants to move to: the link that moves it.
pub fn new_address(old: &str, link: &str) -> Letter {
    Letter {
        subject: "Confirm your new Zetlyn address".into(),
        body: format!("Hello,\n\nthe Zetlyn account of {old} is to use this address from now on. To confirm, follow this link:\n\n  {link}\n\nIt works once, for the next 24 hours. If you did not ask for this, ignore this mail: without the click nothing changes.\n{SIGNATURE}"),
    }
}

/// To the address an account left: that it did, and where it went.
pub fn address_changed(new: &str) -> Letter {
    Letter {
        subject: "Your Zetlyn address was changed".into(),
        body: format!("Hello,\n\nyour Zetlyn account now uses {new}; changes are emailed there, and you sign in with it. This address gets nothing more from Zetlyn.\n\nIf you did not do this, write to hello@zetlyn.com at once.\n{SIGNATURE}"),
    }
}

pub fn signin(link: &str) -> Letter {
    Letter {
        subject: "Your Zetlyn sign-in link".into(),
        body: format!("Hello,\n\nhere is your link to sign in to Zetlyn; if you have no account yet, it makes one:\n\n  {link}\n\nIt works once, for the next 30 minutes. If you did not ask for it, ignore this mail: without the click nothing happens.\n{SIGNATURE}"),
    }
}

pub fn changed(url: &str, what: &str, before: &str, after: &str, at: &str, page: &str, account: &str, reason: Option<&str>) -> Letter {
    let lines = |s: &str| s.lines().take(10).map(|l| format!("    {}", clip(l, 160))).collect::<Vec<_>>().join("\n");
    let body = if before.lines().count() <= 1 && after.lines().count() <= 1 {
        format!("  before   {}\n  now      {}", clip(before, 200), clip(after, 200))
    } else {
        let (gone, came) = crate::text::diff(before, after);
        format!("  gone\n{}\n\n  new\n{}", lines(&gone.join("\n")), lines(&came.join("\n")))
    };
    Letter {
        subject: format!("{}: {} · {}", reason.unwrap_or("Changed"), short(url), clip(what, 40)),
        body: format!("{}{what} on\n  {url}\nchanged, noticed {at}:\n\n{body}\n\nSee the page:\n  {page}\n\nThis watch, its changes and checks:\n  {account}\n{SIGNATURE}", reason.map(|r| format!("{r}, as you asked to be told.\n\n")).unwrap_or_default()),
    }
}

pub fn mark_again(url: &str, link: &str) -> Letter {
    Letter {
        subject: format!("Please mark the place on {} again", short(url)),
        body: format!("Hello,\n\nthe page\n\n  {url}\n\nhas been rebuilt, and Zetlyn cannot tell for sure where the place you marked is now. Rather than report a change that may not be one, it asks you to mark it again:\n\n  {link}\n{SIGNATURE}"),
    }
}

pub fn short(url: &str) -> String {
    clip(url.split("://").nth(1).unwrap_or(url).trim_end_matches('/'), 50)
}

pub fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n { format!("{}…", s.chars().take(n - 1).collect::<String>()) } else { s.to_string() }
}

trait Stream: Read + Write {}
impl<T: Read + Write> Stream for T {}

struct Session {
    io: BufReader<Box<dyn Stream>>,
}

impl Session {
    fn line(&mut self) -> Result<(u16, String), String> {
        let mut text = String::new();
        loop {
            let mut l = String::new();
            if self.io.read_line(&mut l).map_err(|e| format!("SMTP: {e}"))? == 0 {
                return Err("SMTP: the server closed the connection".into());
            }
            let code: u16 = l.get(..3).and_then(|c| c.parse().ok()).ok_or_else(|| format!("SMTP: not a reply: {}", l.trim()))?;
            text.push_str(l.get(4..).unwrap_or("").trim_end());
            text.push(' ');
            if l.as_bytes().get(3) != Some(&b'-') {
                return Ok((code, text.trim().to_string()));
            }
        }
    }
    fn say(&mut self, cmd: &str, want: &[u16]) -> Result<String, String> {
        self.io.get_mut().write_all(format!("{cmd}\r\n").as_bytes()).map_err(|e| format!("SMTP: {e}"))?;
        self.expect(want, cmd.split(' ').next().unwrap_or(cmd))
    }
    fn expect(&mut self, want: &[u16], what: &str) -> Result<String, String> {
        let (code, text) = self.line()?;
        if want.contains(&code) { Ok(text) } else { Err(format!("SMTP {what}: {code} {text}")) }
    }
}

fn tls(host: &str, tcp: TcpStream) -> Result<Box<dyn Stream>, String> {
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| format!("{host}: {e}"))?;
    let conn = rustls::ClientConnection::new(Arc::new(config), name).map_err(|e| e.to_string())?;
    Ok(Box::new(rustls::StreamOwned::new(conn, tcp)))
}

fn smtp(s: &Smtp, from: &str, to: &str, message: &str) -> Result<(), String> {
    let implicit = s.port == 465;
    let tcp = TcpStream::connect((s.host.as_str(), s.port)).map_err(|e| format!("{}:{}: {e}", s.host, s.port))?;
    tcp.set_read_timeout(Some(Duration::from_secs(60))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(60))).ok();
    let spare = tcp.try_clone().map_err(|e| e.to_string())?;
    let first: Box<dyn Stream> = if implicit { tls(&s.host, tcp)? } else { Box::new(tcp) };
    let mut session = Session { io: BufReader::new(first) };
    session.expect(&[220], "greeting")?;
    let hello = session.say("EHLO zetlyn", &[250])?;
    if !implicit {
        if !hello.to_uppercase().contains("STARTTLS") {
            return Err(format!("{} offers no STARTTLS, and nothing is sent in the clear", s.host));
        }
        session.say("STARTTLS", &[220])?;
        session = Session { io: BufReader::new(tls(&s.host, spare)?) };
        session.say("EHLO zetlyn", &[250])?;
    }
    if !s.user.is_empty() {
        let password = std::fs::read_to_string(&s.password_file).map_err(|e| format!("{}: {e}", s.password_file))?.trim().to_string();
        session.say(&format!("AUTH PLAIN {}", base64(format!("\0{}\0{password}", s.user).as_bytes())), &[235])?;
    }
    session.say(&format!("MAIL FROM:<{}>", address_of(from)), &[250])?;
    session.say(&format!("RCPT TO:<{to}>"), &[250, 251])?;
    session.say("DATA", &[354])?;
    session.io.get_mut().write_all(message.as_bytes()).map_err(|e| format!("SMTP: {e}"))?;
    session.say(".", &[250])?;
    let _ = session.say("QUIT", &[221]);
    Ok(())
}

fn address_of(s: &str) -> String {
    match (s.find('<'), s.find('>')) {
        (Some(a), Some(b)) if b > a => s[a + 1..b].trim().to_string(),
        _ => s.trim().to_string(),
    }
}

/// Headers, a blank line, the body with every line ending CRLF and a line starting with a dot
/// doubled, so it cannot end the message early.
fn compose(from: &str, to: &str, subject: &str, body: &str) -> String {
    let subject = if subject.is_ascii() { subject.to_string() } else { format!("=?UTF-8?B?{}?=", base64(subject.as_bytes())) };
    let now = crate::store::now();
    let id = format!("<{}@{}>", &crate::crypto::digest(&format!("{now}{to}{subject}"))[..24], address_of(from).split('@').nth(1).unwrap_or("zetlyn"));
    let mut out = format!("From: {from}\r\nTo: {to}\r\nSubject: {subject}\r\nDate: {}\r\nMessage-ID: {id}\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n", rfc2822(now));
    for line in body.replace("\r\n", "\n").split('\n') {
        if line.starts_with('.') {
            out.push('.');
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out
}

fn rfc2822(t: i64) -> String {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][days.rem_euclid(7) as usize];
    let date = crate::serve::time(t);
    let (y, m, d) = (&date[0..4], date[5..7].parse::<usize>().unwrap_or(1), &date[8..10]);
    let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][m - 1];
    format!("{weekday}, {d} {month} {y} {:02}:{:02}:{:02} +0000", secs / 3600, secs / 60 % 60, secs % 60)
}

fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() { out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char) } else { out.push('=') }
        }
    }
    out
}


/// The account was deleted by its owner: said once, to the address it was.
pub fn deleted(cancelled: bool) -> Letter {
    let sub = if cancelled { "\n\nYour subscription is cancelled; nothing more is charged. Your invoices stay with Stripe, our payment provider, as the law requires." } else { "" };
    Letter {
        subject: "Your Zetlyn account is deleted".into(),
        body: format!("Hello,\n\nas you asked, your Zetlyn account is deleted, with all its watches, their checks and changes, and its API keys.{sub}\n\nIf you did not ask for this, write to hello@zetlyn.com.\n{SIGNATURE}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mail_log_keeps_ninety_days() {
        let dir = std::env::temp_dir().join(format!("zetlyn-maillog-{}", crate::crypto::token()));
        std::fs::create_dir_all(&dir).unwrap();
        let mailer = Mailer { from: "Zetlyn <hello@zetlyn.test>".into(), smtp: None, outbox: dir.join("outbox") };
        let day = 86400;
        let now = 400 * day;
        std::fs::write(dir.join("mail.log"), format!("{}\tsent\told@example.com\tOld\n{}\tsent\tnew@example.com\tNew\n", now - 91 * day, now - 89 * day)).unwrap();
        std::fs::write(dir.join("mail.log.1"), format!("{}\tsent\tolder@example.com\tOlder\n", now - 200 * day)).unwrap();
        mailer.prune_log(now);
        let kept = std::fs::read_to_string(dir.join("mail.log")).unwrap();
        assert!(!kept.contains("old@example.com") && kept.contains("new@example.com"), "{kept}");
        assert!(!dir.join("mail.log.1").exists(), "a log before it that is all old goes");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_message_is_well_formed() {
        assert_eq!(base64(b"\0me\0pw"), "AG1lAHB3");
        assert_eq!(rfc2822(0), "Thu, 01 Jan 1970 00:00:00 +0000");
        let m = compose("Zetlyn <hello@zetlyn.com>", "a@b.c", "Größe", "one\n.two\n");
        assert!(m.contains("Subject: =?UTF-8?B?R3LDtsOfZQ==?=\r\n"), "{m}");
        assert!(m.contains("\r\n\r\none\r\n..two\r\n"), "{m}");
    }

    #[test]
    fn only_an_address_somebody_could_have_is_mailed() {
        assert!(plausible("a@example.com"));
        for bad in ["a@b", "a b@example.com", "a@example.com\r\nRCPT TO:<x@y.z>", "@example.com", "a@@example.com", "<a@example.com>"] {
            assert!(!plausible(bad), "{bad}");
        }
    }

    #[test]
    fn a_change_of_one_value_reads_before_and_now() {
        let l = changed("https://acme.example/pricing", "Place beside “Enterprise”", "499 €", "399 €", "18 Oct 09:00", "https://acme.example/pricing", "https://zetlyn.com/account", None);
        assert!(l.subject.starts_with("Changed: acme.example/pricing"));
        assert!(l.body.contains("before   499 €\n  now      399 €"), "{}", l.body);
    }
}
pub fn trial_ending(url: &str, checks: i64, value: &str, link: &str) -> Letter {
    Letter {
        subject: format!("Your free watch of {} ends tomorrow", short(url)),
        body: format!(
            "Hello,\n\nZetlyn has checked\n\n  {url}\n\n{checks} times so far. It reads now:\n\n  {}\n\nTo keep watching, subscribe: 9.99 € a month for up to 10 watches, 0.50 € for each one more, VAT included.\n\n  {link}\n\nWithout a subscription the watch ends tomorrow.\n{SIGNATURE}",
            clip(value.lines().next().unwrap_or(""), 200)
        ),
    }
}

pub fn ended(url: &str, link: &str) -> Letter {
    Letter {
        subject: format!("Your watch of {} has ended", short(url)),
        body: format!("Hello,\n\nthe free three days are over and Zetlyn no longer watches\n\n  {url}\n\nTo start again with a subscription (9.99 € a month for up to 10 watches, VAT included):\n\n  {link}\n{SIGNATURE}"),
    }
}
