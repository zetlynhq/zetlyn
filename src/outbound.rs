//! What may be fetched because somebody typed it.
//!
//! Every address Zetlyn asks is one a stranger gave it. Without a check that is a way to make the
//! machine ask its own loopback, its cloud provider's metadata service or anything else on its
//! private network, and to read the answer back in a change mail. So: http or https, to a host
//! that resolves to public addresses only, with no user information in it. Each redirect is
//! checked again. (From the old Zetlyn's `outbound.rs`.)

use std::net::{IpAddr, ToSocketAddrs};

/// Whether `url` is somewhere on the public internet, http or https. Plain http to a public
/// address hides nothing of this machine; what is refused is an address of its own or of the
/// network it is in.
pub fn public_target(url: &str) -> Result<(), String> {
    let refuse = |why: &str| Err(format!("{url}: {why}"));
    let (scheme, rest) = match url.split_once("://") {
        Some(p) => p,
        None => return refuse("not an address"),
    };
    if scheme != "https" && scheme != "http" {
        return refuse("only http and https");
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') || authority.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return refuse("not an address this machine asks");
    }
    let default = if scheme == "https" { 443 } else { 80 };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || authority.starts_with('[') => match p.parse::<u16>() {
            Ok(n) => (h.trim_start_matches('[').trim_end_matches(']').to_string(), n),
            Err(_) => (authority.trim_start_matches('[').trim_end_matches(']').to_string(), default),
        },
        _ => (authority.trim_start_matches('[').trim_end_matches(']').to_string(), default),
    };
    let addrs: Vec<IpAddr> = match (host.as_str(), port).to_socket_addrs() {
        Ok(a) => a.map(|s| s.ip()).collect(),
        Err(_) => return refuse("that name does not resolve"),
    };
    if addrs.is_empty() {
        return refuse("that name does not resolve");
    }
    if addrs.iter().any(|ip| !public(ip)) {
        return refuse("it is not on the public internet");
    }
    Ok(())
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
    fn only_the_public_internet() {
        for refused in [
            "https://127.0.0.1/",
            "http://localhost/x",
            "https://169.254.169.254/latest/meta-data/",
            "https://10.0.0.1/",
            "http://192.168.1.1/",
            "https://[::1]/",
            "https://user@example.com/",
            "file:///etc/passwd",
            "ftp://example.com/",
            "https:///nohost",
        ] {
            assert!(public_target(refused).is_err(), "{refused}");
        }
    }
}
