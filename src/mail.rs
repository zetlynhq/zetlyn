//! Mail sent by Zetlyn itself, over SMTP, through whichever provider the operator names.
//!
//! Small on purpose: one message to one address, plain text in UTF-8, over TLS from the first byte
//! (port 465) or after STARTTLS (587), with AUTH PLAIN where there is a user. The password is a
//! variable (`${SMTP_PASSWORD}`), never the file. A connection without TLS is refused unless the
//! server is this machine, where there is nothing between the two ends to read it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// `mail: { smtp: … }` in workspace.yaml.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Smtp {
    pub host: String,
    #[serde(default)]
    pub port: u16,
    /// `implicit` (465, the default there), `starttls` (587 and 25, the default otherwise), or
    /// `none`, for a server on this machine only.
    #[serde(default)]
    pub tls: String,
    #[serde(default)]
    pub user: String,
    /// A variable, `${SMTP_PASSWORD}`, or empty where `password_file` names the file it is in.
    #[serde(default)]
    pub password: String,
    /// A file holding the password and nothing else, readable by the services that send.
    #[serde(default)]
    pub password_file: String,
    /// The address mail is from: `Zetlyn <mail@zetlyn.com>`.
    pub from: String,
}

trait Stream: Read + Write {}
impl<T: Read + Write> Stream for T {}

struct Session {
    io: BufReader<Box<dyn Stream>>,
}

impl Session {
    fn line(&mut self) -> Result<(u16, String), String> {
        // A reply is one line, or several with a dash after the code and a space on the last.
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
        if want.contains(&code) {
            Ok(text)
        } else {
            Err(format!("SMTP {what}: {code} {text}"))
        }
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

fn loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

/// One message. `Ok` means the server took it for delivery, which is as far as SMTP can say.
pub fn send(s: &Smtp, to: &str, subject: &str, body: &str) -> Result<(), String> {
    let mode = if s.tls.is_empty() { if s.port == 465 { "implicit" } else { "starttls" } } else { s.tls.as_str() };
    let port = if s.port != 0 { s.port } else if mode == "implicit" { 465 } else { 587 };
    if mode == "none" && !loopback(&s.host) {
        return Err(format!("{}: mail without TLS goes only to a server on this machine", s.host));
    }
    if to.contains(['\r', '\n', '<', '>']) || !to.contains('@') {
        return Err(format!("{to}: not an address to send to"));
    }
    let tcp = TcpStream::connect((s.host.as_str(), port)).map_err(|e| format!("{}:{port}: {e}", s.host))?;
    tcp.set_read_timeout(Some(Duration::from_secs(60))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(60))).ok();
    // The same socket, kept to be wrapped in TLS once the server has said STARTTLS.
    let spare = tcp.try_clone().map_err(|e| e.to_string())?;
    let first: Box<dyn Stream> = if mode == "implicit" { tls(&s.host, tcp)? } else { Box::new(tcp) };
    let mut session = Session { io: BufReader::new(first) };
    session.expect(&[220], "greeting")?;
    let hello = session.say("EHLO zetlyn", &[250])?;
    if mode == "starttls" {
        if !hello.to_uppercase().contains("STARTTLS") {
            return Err(format!("{} offers no STARTTLS, and a password is not sent in the clear", s.host));
        }
        session.say("STARTTLS", &[220])?;
        session = Session { io: BufReader::new(tls(&s.host, spare)?) };
        session.say("EHLO zetlyn", &[250])?;
    }
    if !s.user.is_empty() {
        let password = if s.password_file.is_empty() {
            crate::fetch::resolve(&s.password)?.unwrap_or_default()
        } else {
            std::fs::read_to_string(&s.password_file).map_err(|e| format!("{}: {e}", s.password_file))?.trim().to_string()
        };
        let token = base64(format!("\0{}\0{password}", s.user).as_bytes());
        session.say(&format!("AUTH PLAIN {token}"), &[235])?;
    }
    let from_addr = address_of(&s.from);
    session.say(&format!("MAIL FROM:<{from_addr}>"), &[250])?;
    session.say(&format!("RCPT TO:<{to}>"), &[250, 251])?;
    session.say("DATA", &[354])?;
    let message = compose(&s.from, to, subject, body);
    session.io.get_mut().write_all(message.as_bytes()).map_err(|e| format!("SMTP: {e}"))?;
    session.say(".", &[250])?;
    let _ = session.say("QUIT", &[221]);
    Ok(())
}

/// `Name <a@b>` or `a@b`, the address alone.
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
    let id = format!("<{}@{}>", &crate::place::sha256(format!("{}{to}{subject}", crate::now()).as_bytes())[..24],
        address_of(from).split('@').nth(1).unwrap_or("zetlyn"));
    let mut out = format!(
        "From: {from}\r\nTo: {to}\r\nSubject: {subject}\r\nDate: {}\r\nMessage-ID: {id}\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n",
        rfc2822(crate::now())
    );
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
    let date = crate::iso_date(t);
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
            if i <= chunk.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_well_formed() {
        assert_eq!(base64(b"\0me\0pw"), "AG1lAHB3");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(rfc2822(0), "Thu, 01 Jan 1970 00:00:00 +0000");
        let m = compose("Zetlyn <mail@zetlyn.com>", "a@b.c", "Größe", "one\n.two\n");
        assert!(m.contains("Subject: =?UTF-8?B?R3LDtsOfZQ==?=\r\n"), "{m}");
        assert!(m.contains("\r\n\r\none\r\n..two\r\n"), "{m}");
        assert_eq!(address_of("Zetlyn <mail@zetlyn.com>"), "mail@zetlyn.com");
    }

    #[test]
    fn nothing_goes_in_the_clear_to_another_machine() {
        let s = Smtp { host: "smtp.example.org".into(), tls: "none".into(), from: "a@b.c".into(), ..Smtp::default() };
        assert!(send(&s, "x@y.z", "s", "b").unwrap_err().contains("only to a server on this machine"));
        let s = Smtp { host: "127.0.0.1".into(), port: 1, tls: "none".into(), from: "a@b.c".into(), ..Smtp::default() };
        assert!(send(&s, "x@y.z\r\nRCPT TO:<evil@e.e>", "s", "b").unwrap_err().contains("not an address"));
    }
}

/// The words of the mail a sign-in link goes out in: subject and body.
pub fn signin_letter(link: &str) -> (String, String) {
    (
        "Your Zetlyn sign-in link".to_string(),
        format!(
            "Hello,\n\n\
             here is your link to sign in to Zetlyn:\n\n  {link}\n\n\
             It works once, for the next 15 minutes. If you did not ask for it, you can ignore this mail; nothing happens without the link.\n\n\
             Best regards,\nThe Zetlyn team\n\n--\nZetlyn · https://zetlyn.com · hello@zetlyn.com\n"
        ),
    )
}

/// The words of the mail a new world's owner gets once it runs: subject and body.
pub fn welcome_letter(title: &str, home: &str, account: &str, email: &str) -> (String, String) {
    (
        format!("Your Zetlyn organisation \u{201c}{title}\u{201d} is ready"),
        format!(
            "Hello,\n\n\
             thank you for choosing Zetlyn Managed. Your organisation \u{201c}{title}\u{201d} is set up and running:\n\n  {home}\n\n\
             To sign in, open {home}signin and enter {email}. We send you a link by mail; there is no password.\n\n\
             Your plan, this month's usage and your invoices are in your account:\n\n  {account}\n\n\
             Your plan includes 2 GB of storage, 25,000 source reads and 1,000 mails a month, with as many users, sources and trackers as you like. \
             To get started, see https://zetlyn.com/docs/getting_started.\n\n\
             If you have any questions, write to hello@zetlyn.com; we are glad to help.\n\n\
             Best regards,\nThe Zetlyn team\n\n--\nZetlyn · https://zetlyn.com · hello@zetlyn.com\n"
        ),
    )
}
