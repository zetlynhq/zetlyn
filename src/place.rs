//! Where published bytes sit.
//!
//! A hub is a directory layout, and the layout is the same wherever it is kept. Reading is one
//! operation, `get`, and it is the same everywhere: fetch a path. Writing is where the places
//! differ, and one of them cannot do it at all.
//!
//! A mount is a folder. There is nothing here for NTFS, SMB or anything else the operating system
//! has already made look like a directory.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

/// Read a path, write a path, list what is under one.
pub trait Place {
    /// What this place is, for a message a person reads.
    fn describe(&self) -> String;
    fn get(&self, path: &str) -> Result<Vec<u8>, String>;
    /// `Err` where the place cannot be written to, which is a fact about the place and not a
    /// failure of the call.
    fn put(&self, path: &str, bytes: &[u8]) -> Result<(), String>;
    fn exists(&self, path: &str) -> bool {
        self.get(path).is_ok()
    }
    /// Every path under `prefix`, for whoever renders what a place holds. A place that cannot say,
    /// a web server, says so.
    fn list(&self, prefix: &str) -> Result<Vec<String>, String> {
        let _ = prefix;
        Err(format!("{}: cannot list what it holds", self.describe()))
    }
}

/// What a browser is told a file is, by its name: a hub's pages are read straight out of where
/// they are kept, and a page served as bytes is a download.
pub fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "txt" | "sig" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Pick the place a location names. A location with a scheme is remote, anything else is a path
/// on this machine, which is what makes a mount need no case of its own.
pub fn at(location: &str) -> Result<Box<dyn Place>, String> {
    if let Some(rest) = location.strip_prefix("s3://") {
        return Ok(Box::new(S3::new(rest)?));
    }
    if location.starts_with("https://") || location.starts_with("http://") {
        return Ok(Box::new(Web::new(location)));
    }
    Ok(Box::new(Folder {
        root: PathBuf::from(location),
    }))
}

pub fn sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

// -- a folder, which is also every mount -------------------------------------------------------

pub struct Folder {
    pub root: PathBuf,
}

impl Folder {
    /// A path from a reference becomes a path on disk, and must not climb out of the hub.
    fn resolve(&self, path: &str) -> Result<PathBuf, String> {
        if path
            .split('/')
            .any(|s| s == ".." || s == "." || s.is_empty())
        {
            return Err(format!("{path}: not a path inside a hub"));
        }
        Ok(self.root.join(path))
    }
}

impl Place for Folder {
    fn describe(&self) -> String {
        self.root.display().to_string()
    }
    fn get(&self, path: &str) -> Result<Vec<u8>, String> {
        let p = self.resolve(path)?;
        std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))
    }
    fn put(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let p = self.resolve(path)?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(&p, bytes).map_err(|e| format!("{}: {e}", p.display()))
    }
    fn exists(&self, path: &str) -> bool {
        self.resolve(path).map(|p| p.exists()).unwrap_or(false)
    }
    fn list(&self, prefix: &str) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Ok(rel) = p.strip_prefix(&self.root) {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    if rel.starts_with(prefix) {
                        out.push(rel);
                    }
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

// -- a web server, which is read-only ----------------------------------------------------------

pub struct Web {
    base: String,
    agent: ureq::Agent,
}

impl Web {
    pub fn new(base: &str) -> Web {
        let agent = ureq::Agent::config_builder()
            .user_agent(concat!("zetlyn/", env!("CARGO_PKG_VERSION")))
            .timeout_global(Some(std::time::Duration::from_secs(120)))
            .build()
            .into();
        Web {
            base: base.trim_end_matches('/').to_string(),
            agent,
        }
    }
}

impl Place for Web {
    fn describe(&self) -> String {
        self.base.clone()
    }
    fn get(&self, path: &str) -> Result<Vec<u8>, String> {
        let url = format!("{}/{path}", self.base);
        let mut response = self
            .agent
            .get(&url)
            .call()
            .map_err(|e| format!("{url}: {e}"))?;
        let mut body = Vec::new();
        response
            .body_mut()
            .as_reader()
            .read_to_end(&mut body)
            .map_err(|e| format!("{url}: {e}"))?;
        Ok(body)
    }
    /// A plain web server is read-only, and writing to one means writing to the folder or the
    /// bucket behind it. A hub that runs the service takes a signed `PUT`, which is the one case
    /// where a name is contended for and somebody has to say who holds it.
    ///
    /// Signed as the identity, over the same statement a console call is signed over. A token
    /// would be the permission itself, so a copy of one would be the permission again.
    fn put(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let Some(who) = crate::identity::key() else {
            return Err(format!(
                "{}: no identity. `zetlyn id new` makes one, and a hub takes nothing unsigned",
                self.base
            ));
        };
        let at = crate::iso_stamp(crate::now());
        let statement = crate::grant::request_statement("PUT", &format!("/{path}"), bytes, &at);
        let signature =
            crate::identity::sign(statement.as_bytes())?.ok_or("no identity to sign with")?;
        let url = format!("{}/{path}", self.base);
        self.agent
            .put(&url)
            .header("Zetlyn-Key", &who)
            .header("Zetlyn-Date", &at)
            .header("Zetlyn-Signature", &signature)
            .send(bytes)
            .map(|_| ())
            .map_err(|e| format!("{url}: {e}"))
    }
}

// -- an S3 bucket -------------------------------------------------------------------------------

/// `s3://bucket/prefix` with the endpoint, the region and the keys from the environment, because
/// a key in a declaration is a key in a backup.
pub struct S3 {
    endpoint: String,
    region: String,
    bucket: String,
    prefix: String,
    key: String,
    secret: String,
    agent: ureq::Agent,
}

impl S3 {
    pub fn new(rest: &str) -> Result<S3, String> {
        let (bucket, prefix) = match rest.split_once('/') {
            Some((b, p)) => (b.to_string(), p.trim_matches('/').to_string()),
            None => (rest.to_string(), String::new()),
        };
        let env = |name: &str| std::env::var(name).unwrap_or_default();
        let endpoint = match env("ZETLYN_S3_ENDPOINT").as_str() {
            "" => "https://s3.amazonaws.com".to_string(),
            e => e.trim_end_matches('/').to_string(),
        };
        let region = match env("ZETLYN_S3_REGION").as_str() {
            "" => "us-east-1".to_string(),
            r => r.to_string(),
        };
        let (key, secret) = (env("ZETLYN_S3_KEY"), env("ZETLYN_S3_SECRET"));
        let agent = ureq::Agent::config_builder()
            .user_agent(concat!("zetlyn/", env!("CARGO_PKG_VERSION")))
            .timeout_global(Some(std::time::Duration::from_secs(300)))
            .build()
            .into();
        Ok(S3 {
            endpoint,
            region,
            bucket,
            prefix,
            key,
            secret,
            agent,
        })
    }

    fn key_for(&self, path: &str) -> String {
        if self.prefix.is_empty() {
            path.to_string()
        } else {
            format!("{}/{path}", self.prefix)
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}/{}", self.endpoint, self.bucket, self.key_for(path))
    }

    fn host(&self) -> String {
        self.endpoint
            .split("://")
            .nth(1)
            .unwrap_or(&self.endpoint)
            .trim_end_matches('/')
            .to_string()
    }

    /// Signature Version 4, the path-style form. Reading a public bucket needs none of this and
    /// goes through the same call unsigned, because a request with no key is not a request with a
    /// bad one.
    fn signed(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
        now: (String, String),
    ) -> BTreeMap<String, String> {
        let uri = format!("/{}/{}", self.bucket, self.key_for(path));
        self.signed_at(method, &uri, "", body, now)
    }

    /// The same signature for any address on the endpoint, with its query already in canonical
    /// form: names sorted, every value encoded.
    fn signed_at(
        &self,
        method: &str,
        canonical_uri: &str,
        query: &str,
        body: &[u8],
        now: (String, String),
    ) -> BTreeMap<String, String> {
        let (date, stamp) = now;
        let payload = sha256(body);
        let mut headers = BTreeMap::new();
        headers.insert("host".to_string(), self.host());
        headers.insert("x-amz-content-sha256".to_string(), payload.clone());
        headers.insert("x-amz-date".to_string(), stamp.clone());
        if self.key.is_empty() || self.secret.is_empty() {
            return headers;
        }

        let signed_headers = "host;x-amz-content-sha256;x-amz-date";
        let canonical = format!(
            "{method}\n{canonical_uri}\n{query}\nhost:{}\nx-amz-content-sha256:{payload}\nx-amz-date:{stamp}\n\n{signed_headers}\n{payload}",
            self.host()
        );
        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{stamp}\n{scope}\n{}",
            sha256(canonical.as_bytes())
        );
        let mut signing = hmac(format!("AWS4{}", self.secret).as_bytes(), date.as_bytes());
        signing = hmac(&signing, self.region.as_bytes());
        signing = hmac(&signing, b"s3");
        signing = hmac(&signing, b"aws4_request");
        let signature: String = hmac(&signing, to_sign.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        headers.insert(
            "authorization".to_string(),
            format!(
                "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
                self.key
            ),
        );
        headers
    }
}

/// HMAC-SHA256, written out because the one hash this program already depends on is enough.
fn hmac(key: &[u8], message: &[u8]) -> Vec<u8> {
    const BLOCK: usize = 64;
    let mut k = if key.len() > BLOCK {
        let mut h = Sha256::new();
        h.update(key);
        h.finalize().to_vec()
    } else {
        key.to_vec()
    };
    k.resize(BLOCK, 0);
    let inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    let outer: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    let mut h = Sha256::new();
    h.update(&inner);
    h.update(message);
    let first = h.finalize();
    let mut h = Sha256::new();
    h.update(&outer);
    h.update(first);
    h.finalize().to_vec()
}

/// `20260927`, `20260927T101530Z`. The same two values the rest of the program prints with
/// hyphens and colons, spelled the way this signature wants them.
fn amz_now() -> (String, String) {
    let stamp = crate::iso_stamp(crate::now());
    let flat: String = stamp.chars().filter(|c| *c != '-' && *c != ':').collect();
    (flat[..8].to_string(), flat)
}

impl Place for S3 {
    fn describe(&self) -> String {
        format!("s3://{}/{}", self.bucket, self.prefix)
    }
    fn get(&self, path: &str) -> Result<Vec<u8>, String> {
        let url = self.url(path);
        let mut request = self.agent.get(&url);
        for (k, v) in self.signed("GET", path, b"", amz_now()) {
            request = request.header(k.as_str(), v.as_str());
        }
        let mut response = request.call().map_err(|e| format!("{url}: {e}"))?;
        let mut body = Vec::new();
        response
            .body_mut()
            .as_reader()
            .read_to_end(&mut body)
            .map_err(|e| format!("{url}: {e}"))?;
        Ok(body)
    }
    fn put(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        if self.key.is_empty() || self.secret.is_empty() {
            return Err("ZETLYN_S3_KEY and ZETLYN_S3_SECRET are not set".into());
        }
        let url = self.url(path);
        let mut request = self.agent.put(&url).header("content-type", content_type(path));
        for (k, v) in self.signed("PUT", path, bytes, amz_now()) {
            request = request.header(k.as_str(), v.as_str());
        }
        request
            .send(bytes)
            .map(|_| ())
            .map_err(|e| format!("{url}: {e}"))
    }
    /// ListObjectsV2, a thousand keys a page, following the continuation until there is none.
    fn list(&self, prefix: &str) -> Result<Vec<String>, String> {
        let full = self.key_for(prefix);
        let strip = if self.prefix.is_empty() { String::new() } else { format!("{}/", self.prefix) };
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params: Vec<(String, String)> = vec![("list-type".into(), "2".into()), ("prefix".into(), full.clone())];
            if let Some(t) = &token {
                params.push(("continuation-token".into(), t.clone()));
            }
            params.sort();
            let query: String = params.iter().map(|(k, v)| format!("{}={}", uri_encode(k), uri_encode(v))).collect::<Vec<_>>().join("&");
            let url = format!("{}/{}?{query}", self.endpoint, self.bucket);
            let mut request = self.agent.get(&url);
            for (k, v) in self.signed_at("GET", &format!("/{}", self.bucket), &query, b"", amz_now()) {
                request = request.header(k.as_str(), v.as_str());
            }
            let mut response = request.call().map_err(|e| format!("{url}: {e}"))?;
            let body = response.body_mut().read_to_string().map_err(|e| format!("{url}: {e}"))?;
            for key in between(&body, "<Key>", "</Key>") {
                let key = unescape(&key);
                out.push(key.strip_prefix(&strip).unwrap_or(&key).to_string());
            }
            token = between(&body, "<NextContinuationToken>", "</NextContinuationToken>").into_iter().next().map(|t| unescape(&t));
            if !body.contains("<IsTruncated>true</IsTruncated>") || token.is_none() {
                break;
            }
        }
        out.sort();
        Ok(out)
    }
}

/// Every unreserved character as it is and everything else as `%XX`, which is what a signed
/// query asks for.
fn uri_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The text between each `open` and the `close` after it, in a listing that has no nesting.
fn between(text: &str, open: &str, close: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(open) {
        let after = &rest[i + open.len()..];
        let Some(j) = after.find(close) else { break };
        out.push(after[..j].to_string());
        rest = &after[j + close.len()..];
    }
    out
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one piece of cryptography in this program written by hand, and it is checked against
    /// somebody else's implementation rather than against itself. The expected values below came
    /// from `openssl dgst -sha256 -mac HMAC` driving the same four derivations in a shell, on the
    /// same inputs, which is the closest thing to an independent witness available without a
    /// bucket to ask.
    ///
    /// What this does not say is whether a real endpoint accepts the request. That needs an
    /// endpoint.
    #[test]
    fn sigv4_agrees_with_openssl() {
        let s3 = S3 {
            endpoint: "https://s3.example.com".into(),
            region: "us-east-1".into(),
            bucket: "b".into(),
            prefix: String::new(),
            key: "AKIDEXAMPLE".into(),
            secret: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            agent: ureq::Agent::config_builder().build().into(),
        };
        let headers = s3.signed(
            "GET",
            "datasets/x/y/tags/latest",
            b"",
            ("20260927".into(), "20260927T101530Z".into()),
        );
        let authorization = headers.get("authorization").expect("signed");
        assert!(
            authorization.ends_with(
                "Signature=bfaabbee5c6db1ef019cec4950da766d7ee9d00c209d878e1db3f4af8cff1bad"
            ),
            "{authorization}"
        );
        assert!(authorization.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260927/us-east-1/s3/aws4_request, \
             SignedHeaders=host;x-amz-content-sha256;x-amz-date, "
        ));
    }

    /// An empty body hashes to the value every S3 implementation expects to see in
    /// `x-amz-content-sha256`, which is one of the two places a wrong hash is silent.
    #[test]
    fn the_empty_payload_hash() {
        assert_eq!(
            sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    /// Without keys there is no authorization header, and a public bucket is read through the
    /// same call unsigned. A request with no key is not a request with a bad one.
    #[test]
    fn unsigned_without_keys() {
        let s3 = S3 {
            endpoint: "https://s3.example.com".into(),
            region: "us-east-1".into(),
            bucket: "b".into(),
            prefix: String::new(),
            key: String::new(),
            secret: String::new(),
            agent: ureq::Agent::config_builder().build().into(),
        };
        let headers = s3.signed(
            "GET",
            "a/b",
            b"",
            ("20260927".into(), "20260927T101530Z".into()),
        );
        assert!(!headers.contains_key("authorization"));
        assert_eq!(
            headers.get("host").map(String::as_str),
            Some("s3.example.com")
        );
    }
}

/// HMAC-SHA256 (RFC 2104), what a webhook's sender signs a body with.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.iter().map(|b| b ^ 0x36).collect::<Vec<u8>>());
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(k.iter().map(|b| b ^ 0x5c).collect::<Vec<u8>>());
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// Two strings compared in time that does not depend on where they differ.
pub fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod hmac_tests {
    #[test]
    fn hmac_is_rfc_4231() {
        // Test case 2 of RFC 4231.
        let mac = super::hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        assert!(super::same("abc", "abc") && !super::same("abc", "abd") && !super::same("abc", "ab"));
    }
}
