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
    /// bucket behind it. A hub that runs the service takes a `PUT` with a token, which is the
    /// one case where a name is contended for and somebody has to say who holds it.
    fn put(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let token = std::env::var("ZETLYN_HUB_TOKEN").unwrap_or_default();
        if token.is_empty() {
            return Err(format!(
                "{}: no ZETLYN_HUB_TOKEN. A web server without one is read-only, and publishing \
                 goes to the folder or the bucket behind it",
                self.base
            ));
        }
        let url = format!("{}/{path}", self.base);
        self.agent
            .put(&url)
            .header("Authorization", &format!("Bearer {token}"))
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
        let (date, stamp) = now;
        let payload = sha256(body);
        let mut headers = BTreeMap::new();
        headers.insert("host".to_string(), self.host());
        headers.insert("x-amz-content-sha256".to_string(), payload.clone());
        headers.insert("x-amz-date".to_string(), stamp.clone());
        if self.key.is_empty() || self.secret.is_empty() {
            return headers;
        }

        let canonical_uri = format!("/{}/{}", self.bucket, self.key_for(path));
        let signed_headers = "host;x-amz-content-sha256;x-amz-date";
        let canonical = format!(
            "{method}\n{canonical_uri}\n\nhost:{}\nx-amz-content-sha256:{payload}\nx-amz-date:{stamp}\n\n{signed_headers}\n{payload}",
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
        let mut request = self.agent.put(&url);
        for (k, v) in self.signed("PUT", path, bytes, amz_now()) {
            request = request.header(k.as_str(), v.as_str());
        }
        request
            .send(bytes)
            .map(|_| ())
            .map_err(|e| format!("{url}: {e}"))
    }
}
