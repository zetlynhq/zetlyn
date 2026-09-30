//! A source that is a query against PostgreSQL.
//!
//! Every row comes back as JSON, made by the database itself (`row_to_json`), so a declaration
//! reads a row by the same paths it reads an API answer by, and no column type needs a case here.
//! The watermark goes into the query as a quoted literal: a bound parameter would need the
//! column's type, which only the database knows, and an untyped literal takes the column's.

use std::path::Path;
use std::sync::Arc;

use serde_json::Value as J;

use crate::claim::Origin;
use crate::expr::Row;
use crate::rows::Produced;

pub struct Spec<'a> {
    pub dsn: &'a str,
    pub query: &'a str,
    pub since: Option<&'a str>,
    pub since_default: &'a str,
}

/// The rows, and the latest value of `since` among them, which is the next update's mark.
pub fn rows(
    spec: &Spec,
    mark: Option<String>,
    root: &Path,
    on_row: &mut impl FnMut(Produced) -> Result<(), String>,
) -> Result<Option<String>, String> {
    let dsn = crate::fetch::resolve(spec.dsn)?
        .filter(|d| !d.is_empty())
        .ok_or("the connection string's variable is not set")?;
    let from = mark.clone().unwrap_or_else(|| spec.since_default.to_string());
    let query = spec.query.replace("{since}", &literal(&from));
    let wrapped = format!("select row_to_json(q)::text from ({query}) q");

    let mut client = postgres::Client::connect(&dsn, tls()?).map_err(|e| format!("PostgreSQL: {}", plain(&e)))?;
    let found = client.query(wrapped.as_str(), &[]).map_err(|e| format!("PostgreSQL: {}", plain(&e)))?;

    let mut high = mark;
    for (n, r) in found.iter().enumerate() {
        let text: String = r.try_get(0).map_err(|e| format!("PostgreSQL: {e}"))?;
        let value: J = serde_json::from_str(&text).map_err(|e| format!("a row that is not JSON: {e}"))?;
        if let Some(path) = spec.since {
            let path = path.trim_start_matches("field:");
            for v in crate::expr::walk(&value, path) {
                let s = crate::expr::as_string(&v);
                if !s.is_empty() && high.as_deref().map_or(true, |h| s.as_str() > h) {
                    high = Some(s);
                }
            }
        }
        on_row(Produced {
            expanded: false,
            row: Row { value, meta: Default::default(), file: None, text: String::new(), root },
            // The row's place in this answer. A claim is named by its identifier; this is only
            // what a claim with none is called.
            origin: Origin { url: Some(format!("sql#{}", n + 1)), ..Origin::default() },
        })?;
    }
    Ok(if spec.since.is_some() { high } else { None })
}

/// A value as SQL spells a string: quoted, with its quotes doubled.
fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// What a connection error says, without the connection string in it.
fn plain(e: &postgres::Error) -> String {
    if let Some(db) = e.as_db_error() {
        return db.message().to_string();
    }
    // The reason is a cause below the error, and the error alone says only that it failed.
    let mut s = e.to_string();
    let mut cause = std::error::Error::source(e);
    while let Some(c) = cause {
        s.push_str(&format!(": {c}"));
        cause = c.source();
    }
    s
}

/// TLS where the server offers it and the connection string asks for it (`sslmode`), over the
/// same rustls and roots every other call here uses.
fn tls() -> Result<tokio_postgres_rustls::MakeRustlsConnect, String> {
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(tokio_postgres_rustls::MakeRustlsConnect::new(config))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_watermark_cannot_end_the_string_it_is_in() {
        assert_eq!(super::literal("2026-09-01"), "'2026-09-01'");
        assert_eq!(super::literal("x'; drop table t; --"), "'x''; drop table t; --'");
    }
}
