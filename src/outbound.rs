//! What may be fetched because somebody else named it.
//!
//! A client document, a world somebody types, a world registering with a directory, the endpoints a
//! stranger's discovery document lists: each is an address this machine would ask on the say-so of
//! whoever sent it. Without a check that is a way to make the machine ask its own loopback, its
//! cloud provider's metadata service or anything else on its private network, and to read the
//! answer back out of an error message. So: https only, to a host that resolves to public
//! addresses only, with no user information in it. A machine that itself answers on loopback (a
//! test, a laptop) may ask loopback too, because there is nothing behind it to protect.

use std::net::{IpAddr, ToSocketAddrs};

/// Whether `url` may be fetched on a stranger's say-so. `loopback_ok`: this machine is itself on
/// loopback, so loopback addresses (and plain http to them) are allowed.
pub fn allowed(url: &str, loopback_ok: bool) -> Result<(), String> {
    let refuse = |why: &str| Err(format!("{url}: {why}"));
    let (scheme, rest) = match url.split_once("://") {
        Some(p) => p,
        None => return refuse("not an address"),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') || authority.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return refuse("not an address this machine asks");
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || authority.starts_with('[') => match p.parse::<u16>() {
            Ok(n) => (h.trim_start_matches('[').trim_end_matches(']').to_string(), n),
            Err(_) => (authority.trim_start_matches('[').trim_end_matches(']').to_string(), if scheme == "https" { 443 } else { 80 }),
        },
        _ => (authority.trim_start_matches('[').trim_end_matches(']').to_string(), if scheme == "https" { 443 } else { 80 }),
    };
    let addrs: Vec<IpAddr> = match (host.as_str(), port).to_socket_addrs() {
        Ok(a) => a.map(|s| s.ip()).collect(),
        Err(_) => return refuse("that name does not resolve"),
    };
    if addrs.is_empty() {
        return refuse("that name does not resolve");
    }
    let all_loopback = addrs.iter().all(|ip| ip.is_loopback());
    match scheme {
        "https" => {}
        "http" if loopback_ok && all_loopback => {}
        _ => return refuse("only https"),
    }
    for ip in &addrs {
        if ip.is_loopback() && loopback_ok {
            continue;
        }
        if !public(ip) {
            return refuse("it is not on the public internet");
        }
    }
    Ok(())
}

/// Whether the place serving at `url` is on loopback itself.
pub fn on_loopback(url: &str) -> bool {
    let host = url.split("://").nth(1).unwrap_or("").split(['/', ':', '?']).next().unwrap_or("");
    host == "127.0.0.1" || host == "localhost" || host == "[::1]"
}

fn public(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1]))
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return public(&IpAddr::V4(v4));
            }
            let s = v6.segments();
            !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_https_unless_this_machine_is_loopback_itself() {
        for refused in [
            "http://example.com/",
            "https://127.0.0.1/",
            "https://localhost/x",
            "https://169.254.169.254/latest/meta-data/",
            "https://10.0.0.1/",
            "https://192.168.1.1/",
            "https://[::1]/",
            "https://user@example.com/",
            "file:///etc/passwd",
            "https:///nohost",
        ] {
            assert!(allowed(refused, false).is_err(), "{refused}");
        }
        assert!(allowed("http://127.0.0.1:8080/x", true).is_ok());
        assert!(allowed("http://10.0.0.1/", true).is_err(), "loopback, not the private network");
        assert!(on_loopback("http://127.0.0.1:5/x") && !on_loopback("https://zetlyn.com"));
    }
}
