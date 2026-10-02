//! A source that is pushed to.
//!
//! Its sender POSTs JSON to the workspace's `/hook/<source>`, signed with a secret both sides hold
//! (HMAC-SHA256 of the body, `X-Hub-Signature-256: sha256=<hex>`, as GitHub and many others send
//! it). Each body that verifies is kept as it arrived, one file in the source's `inbox/`, and an
//! update reads the inbox as it would an API's pages: the declaration's `each` and paths apply to
//! each body. Nothing is taken that does not verify, and nothing that arrived is thrown away, so
//! the inbox is the source's own record of what it was told.

use std::path::Path;

use serde_json::Value as J;

use crate::claim::Origin;
use crate::expr::Row;
use crate::rows::Produced;

pub const INBOX: &str = "inbox";

/// A body, checked against the source's secret and kept. The name it was kept under.
pub fn receive(dir: &Path, body: &[u8], signature: Option<&str>) -> Result<String, String> {
    let decl = crate::sourcedecl::SourceDecl::load(dir)?;
    let crate::sourcedecl::Fetch::Webhook { secret } = &decl.source else {
        return Err(format!("{} is not pushed to", decl.name));
    };
    let secret = crate::fetch::resolve(secret)?.filter(|s| !s.is_empty()).ok_or("the hook's secret is not set")?;
    let mac = crate::place::hmac_sha256(secret.as_bytes(), body);
    let want: String = format!("sha256={}", mac.iter().map(|b| format!("{b:02x}")).collect::<String>());
    let given = signature.unwrap_or("").trim();
    if !crate::place::same(given, &want) {
        return Err("the signature does not verify".into());
    }
    let _: J = serde_json::from_slice(body).map_err(|e| format!("not JSON: {e}"))?;
    let inbox = dir.join(INBOX);
    std::fs::create_dir_all(&inbox).map_err(|e| e.to_string())?;
    // Named by when it came and what it is, so the inbox reads in the order it was told and the
    // same body sent twice is one file.
    let name = format!("{}-{}.json", crate::iso_stamp(crate::now()).replace(':', ""), &crate::place::sha256(body)[..12]);
    let exists = std::fs::read_dir(&inbox)
        .map(|d| d.flatten().any(|e| e.file_name().to_string_lossy().ends_with(&name[name.len() - 17..])))
        .unwrap_or(false);
    if !exists {
        std::fs::write(inbox.join(&name), body).map_err(|e| e.to_string())?;
    }
    Ok(name)
}

/// Every body in the inbox, oldest first, as rows, each one's origin `<what>#<file>`.
pub fn rows(dir: &Path, root: &Path, what: &str, on_row: &mut impl FnMut(Produced) -> Result<(), String>) -> Result<Option<String>, String> {
    let inbox = dir.join(INBOX);
    let mut files: Vec<_> = std::fs::read_dir(&inbox)
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
        .unwrap_or_default();
    files.sort();
    for f in files {
        let text = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
        let Ok(value) = serde_json::from_str::<J>(&text) else { continue };
        let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        on_row(Produced {
            expanded: false,
            row: Row { value, meta: Default::default(), file: None, text: String::new(), root },
            origin: Origin { url: Some(format!("{what}#{name}")), ..Origin::default() },
        })?;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_what_verifies_is_kept() {
        let dir = std::env::temp_dir().join(format!("zetlyn-hook-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("source.yaml"), "name: t/hook\nkind: event\nfetch:\n  type: webhook\n  secret: ${ZETLYN_TEST_HOOK_SECRET}\nclaims:\n  id:\n    scheme: event\n    from: field:id\n  title: field:title\n").unwrap();
        std::env::set_var("ZETLYN_TEST_HOOK_SECRET", "s3cret");
        let body = br#"{"id": "e1", "title": "One"}"#;
        let mac = crate::place::hmac_sha256(b"s3cret", body);
        let sig = format!("sha256={}", mac.iter().map(|b| format!("{b:02x}")).collect::<String>());
        assert!(super::receive(&dir, body, Some("sha256=00")).unwrap_err().contains("does not verify"));
        assert!(super::receive(&dir, body, None).is_err());
        super::receive(&dir, body, Some(&sig)).unwrap();
        super::receive(&dir, body, Some(&sig)).unwrap();
        assert_eq!(std::fs::read_dir(dir.join(super::INBOX)).unwrap().count(), 1, "the same body twice is one file");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
