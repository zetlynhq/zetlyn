//! A dataset that lives somewhere else.
//!
//! The same six calls, over HTTP, against the surface a dataset already serves. One interface and
//! not two: nothing here is a second protocol, and a scope cannot tell which of its members is
//! local except by looking at where it was named.

use serde_json::{json, Value as J};

use crate::dataset::{Member, Query};
use crate::record::{Id, Origin, Record};
use crate::store::{parse_fields, parse_ids, Hit, Unanswered};

pub struct Remote {
    base: String,
    name: String,
    agent: ureq::Agent,
    key: Option<String>,
    /// Asked once and kept. A scope builds its whole overview from this.
    described: J,
}

impl Remote {
    pub fn open(base: &str, key: Option<String>) -> Result<Remote, String> {
        let base = base.trim_end_matches('/').to_string();
        let agent = ureq::Agent::config_builder()
            .user_agent(crate::decl::AGENT)
            .timeout_global(Some(std::time::Duration::from_secs(30)))
            .build()
            .new_agent();
        let mut remote = Remote {
            base,
            name: String::new(),
            agent,
            key,
            described: J::Null,
        };
        remote.described = remote.ask("/api/describe", &[])?;
        remote.name = remote.described["dataset"]
            .as_str()
            .ok_or("that address does not answer as a dataset")?
            .to_string();
        Ok(remote)
    }

    fn ask(&self, path: &str, params: &[(&str, String)]) -> Result<J, String> {
        let mut url = format!("{}{path}", self.base);
        for (i, (k, v)) in params.iter().enumerate() {
            url.push(if i == 0 { '?' } else { '&' });
            url.push_str(&format!("{k}={}", crate::serve::urlencode(v)));
        }
        let mut request = self.agent.get(&url);
        if let Some(key) = &self.key {
            request = request.header("Authorization", &format!("Bearer {key}"));
        }
        let body = request
            .call()
            .map_err(|e| format!("{url}: {e}"))?
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("{url}: {e}"))?;
        serde_json::from_str(&body).map_err(|e| format!("{url}: not JSON: {e}"))
    }

    /// The query as the surface takes it. A remote member is asked in the same words a person
    /// types, because that is the only query language this program has.
    fn as_params(q: &Query) -> Vec<(&'static str, String)> {
        let mut out = vec![("q", spell(q))];
        if !q.ids.is_empty() {
            out.push(("ids", q.ids.join(",")));
        }
        if let Some(v) = &q.view {
            out.push(("view", v.clone()));
        }
        if let Some(s) = &q.sort {
            out.push(("sort", s.clone()));
        }
        if let Some(b) = &q.seen_before {
            out.push(("seen_before", b.clone()));
        }
        out.push(("limit", q.limit.to_string()));
        out.push(("offset", q.offset.to_string()));
        out
    }
}

/// A predicate back into the words it was typed in. The wire carries the query, not a parse tree:
/// a member on the other side has its own parser and its own fields, and handing it an AST would
/// be handing it this program's idea of what it holds.
fn spell(q: &Query) -> String {
    let mut out = q.text.clone();
    if let Some(p) = &q.pred {
        let mut parts = Vec::new();
        crate::serve::flatten(p, &mut parts);
        for (name, op, lit) in parts {
            out.push(' ');
            out.push_str(&format!("{name}{}{}", op.sql(), lit.display()));
        }
    }
    out.trim().to_string()
}

fn hit_of(v: &J, rank: usize) -> Hit {
    Hit {
        record_id: v["record_id"].as_str().unwrap_or("").to_string(),
        rank,
        title: v["title"].as_str().unwrap_or("").to_string(),
        url: v["url"].as_str().map(str::to_string),
        kind: v["kind"].as_str().unwrap_or("record").to_string(),
        known: v["known"].as_str().unwrap_or("").to_string(),
        ids: parse_ids(&v["ids"].to_string()),
        fields: parse_fields(&v["fields"].to_string()),
        why_text: v["why"]["text"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        why_field: v["why"]["field"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        why_id: v["why"]["id"].as_object().map(|o| Id {
            scheme: o
                .get("scheme")
                .and_then(J::as_str)
                .unwrap_or("")
                .to_string(),
            value: o.get("value").and_then(J::as_str).unwrap_or("").to_string(),
        }),
        snippet: String::new(),
    }
}

impl Member for Remote {
    fn name(&self) -> &str {
        &self.name
    }

    fn describe(&self) -> J {
        self.described.clone()
    }

    fn search(&self, q: &Query) -> Result<(u64, Vec<Hit>, Unanswered), String> {
        let answer = self.ask("/api/search", &Remote::as_params(q))?;
        if let Some(why) = answer["error"].as_str() {
            // A remote member that will not answer says so, and the scope shows it beside the
            // members that did rather than pretending the answer is whole.
            return Ok((0, Vec::new(), Unanswered(vec![why.to_string()])));
        }
        let empty = Vec::new();
        let hits: Vec<Hit> = answer["hits"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .enumerate()
            .map(|(i, v)| hit_of(v, q.offset + i + 1))
            .collect();
        let unanswered: Vec<String> = answer["unanswered"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(|s| s.as_str().map(str::to_string))
            .collect();
        Ok((
            answer["total"].as_u64().unwrap_or(0),
            hits,
            Unanswered(unanswered),
        ))
    }

    fn facet(&self, q: &Query, field: &str, limit: usize) -> Vec<(String, u64)> {
        let mut params = Remote::as_params(q);
        params.push(("field", field.to_string()));
        params.push(("limit", limit.to_string()));
        let Ok(answer) = self.ask("/api/facet", &params) else {
            return Vec::new();
        };
        let empty = Vec::new();
        answer["values"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(|v| {
                Some((
                    v["value"].as_str()?.to_string(),
                    v["records"].as_u64().unwrap_or(0),
                ))
            })
            .collect()
    }

    fn fetch(&self, ids: &[String]) -> Vec<Record> {
        let Ok(answer) = self.ask("/api/fetch", &[("id", ids.join(","))]) else {
            return Vec::new();
        };
        let empty = Vec::new();
        answer["records"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .map(|v| Record {
                record_id: v["record_id"].as_str().unwrap_or("").to_string(),
                dataset: self.name.clone(),
                kind: v["kind"].as_str().unwrap_or("record").to_string(),
                ids: parse_ids(&v["ids"].to_string()),
                title: v["title"].as_str().unwrap_or("").to_string(),
                url: v["url"].as_str().map(str::to_string),
                text: v["text"].as_str().unwrap_or("").to_string(),
                fields: parse_fields(&v["fields"].to_string()),
                known: v["known"].as_str().unwrap_or("").to_string(),
                valid: None,
                from: Origin {
                    url: v["from"]["url"].as_str().map(str::to_string),
                    file: v["from"]["file"].as_str().map(str::to_string),
                    row: v["from"]["row"].as_u64(),
                    span: None,
                },
                attachments: Vec::new(),
                hash: v["hash"].as_str().unwrap_or("").to_string(),
            })
            .collect()
    }

    fn changes(&self, since: i64, limit: usize) -> J {
        self.ask(
            "/api/changes",
            &[("since", since.to_string()), ("limit", limit.to_string())],
        )
        .unwrap_or_else(|e| json!({ "error": e, "changed": [], "removed": [] }))
    }

    fn mark(&self) -> i64 {
        self.ask("/api/mark", &[])
            .ok()
            .and_then(|j| j["mark"].as_i64())
            .unwrap_or(0)
    }
}
