//! The assist. It proposes; code executes; a person confirms.
//!
//! A model is asked what the patterns could not answer: where the list is in a JSON answer, which
//! of two columns is the title, what a source is for in one sentence. Every answer is a
//! declaration or a piece of one, and what it produces is shown before it is kept. It never
//! summarises, never answers a question and never chooses between sources.
//!
//! Two providers behind one call: Claude over the Messages API, and anything that speaks the
//! OpenAI chat API, which includes a model running on this machine (Ollama, llama.cpp, vLLM). A
//! person with documents they will not send anywhere points it at their own.
//!
//! Nothing leaves the machine without the person having seen what: each source records, beside
//! its declaration, that it may be sent and what was sent (D10).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

/// The model a new workspace asks, chosen when this was built: capable enough to read an API it
/// has not seen, cheap enough to ask on every new source.
pub const CLAUDE_MODEL: &str = "claude-sonnet-5";

/// `assist:` in workspace.yaml.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// `anthropic` or `openai`. Unset: Anthropic where a key is found, else nothing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// For `openai`: the base address, `http://127.0.0.1:11434/v1` for Ollama.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// The assist is not asked at all, whatever keys are about.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub off: bool,
}

enum Provider {
    Anthropic { key: String, model: String },
    OpenAi { url: String, key: Option<String>, model: String },
}

pub struct Assist {
    provider: Option<Provider>,
    /// Answers read from files instead of asked, for tests: named by the task and a digest of the
    /// question, so a changed question is a missing answer rather than a stale one.
    replay: Option<PathBuf>,
    record: Option<PathBuf>,
}

/// What one call sends, said before it is sent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Disclosure {
    /// Where it goes: the provider and the model.
    pub to: String,
    /// What goes, in words a person checks: an address, field names, so many rows.
    pub sends: Vec<String>,
}

impl Assist {
    /// The assist as the workspace configures it, or none. Environment first, so a person can
    /// try a model without editing a file: `ZETLYN_ASSIST_URL` and `ZETLYN_ASSIST_MODEL` name an
    /// OpenAI-compatible endpoint.
    pub fn configured(root: &Path) -> Assist {
        let site = crate::account::Site::load(root);
        let c = site.assist;
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let replay = env("ZETLYN_ASSIST_REPLAY").map(PathBuf::from);
        let record = env("ZETLYN_ASSIST_RECORD").map(PathBuf::from);
        if c.off {
            return Assist { provider: None, replay, record };
        }
        let url = env("ZETLYN_ASSIST_URL").unwrap_or(c.url.clone());
        let model = env("ZETLYN_ASSIST_MODEL").unwrap_or(c.model.clone());
        let provider = if c.provider == "openai" || (c.provider.is_empty() && !url.is_empty()) {
            (!url.is_empty()).then(|| Provider::OpenAi {
                url: url.trim_end_matches('/').to_string(),
                key: env("OPENAI_API_KEY").or_else(|| stored_key("openai")),
                model: if model.is_empty() { "gemma4".into() } else { model },
            })
        } else {
            env("ANTHROPIC_API_KEY").or_else(|| world_key(root, "anthropic")).or_else(|| stored_key("anthropic")).map(|key| Provider::Anthropic {
                key,
                model: if model.is_empty() { CLAUDE_MODEL.into() } else { model },
            })
        };
        Assist { provider, replay, record }
    }

    pub fn available(&self) -> bool {
        self.provider.is_some() || self.replay.is_some()
    }

    /// Who is asked, in words.
    pub fn who(&self) -> String {
        match &self.provider {
            Some(Provider::Anthropic { model, .. }) => format!("{model} at Anthropic"),
            Some(Provider::OpenAi { url, model, .. }) => format!("{model} at {url}"),
            None if self.replay.is_some() => "recorded answers".into(),
            None => "nobody".into(),
        }
    }

    /// Why there is no assist, as a person would fix it.
    pub fn missing() -> String {
        "no assist here. Set ANTHROPIC_API_KEY, or `zetlyn assist key anthropic`, or point \
         `assist: { provider: openai, url: http://127.0.0.1:11434/v1, model: … }` in \
         workspace.yaml at a model of your own"
            .into()
    }

    /// One question, one answer shaped by `schema`. The answer is a proposal: every caller holds
    /// it against what the source actually says before a person sees it.
    /// `key` names the question for replay: what it is about and which step, not its wording,
    /// since an API answer carries values (a timestamp) that differ on every call.
    pub fn ask(&self, task: &str, key: &str, system: &str, user: &str, schema: &J) -> Result<J, String> {
        let digest = crate::place::sha256(key.as_bytes())[..12].to_string();
        let file = format!("{task}-{digest}.json");
        if let Some(dir) = &self.replay {
            let path = dir.join(&file);
            if let Ok(text) = std::fs::read_to_string(&path) {
                return serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()));
            }
            if self.provider.is_none() {
                // The question, where whoever answers it can read it: a person, or a model run
                // somewhere else. The answer goes beside it and the next run takes it.
                let asked = dir.join(format!("{task}-{digest}.question.md"));
                let _ = std::fs::create_dir_all(dir);
                let _ = std::fs::write(
                    &asked,
                    format!("# System\n\n{system}\n\n# Answer as one JSON object of this schema\n\n{}\n\n# Question\n\n{user}\n",
                        serde_json::to_string_pretty(schema).unwrap_or_default()),
                );
                return Err(format!("asked in {}; the answer goes in {}", asked.display(), path.display()));
            }
        }
        let answer = match &self.provider {
            Some(Provider::Anthropic { key, model }) => anthropic(key, model, system, user, schema)?,
            Some(Provider::OpenAi { url, key, model }) => openai(url, key.as_deref(), model, system, user, schema)?,
            None => return Err(Self::missing()),
        };
        if let Some(dir) = &self.record {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(dir.join(&file), serde_json::to_string_pretty(&answer).unwrap_or_default());
        }
        Ok(answer)
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(600)))
        .http_status_as_error(false)
        .build()
        .into()
}

/// The answer as a tool call the model is made to make, so it comes back as JSON of this shape
/// and not as prose around some.
fn anthropic(key: &str, model: &str, system: &str, user: &str, schema: &J) -> Result<J, String> {
    let body = json!({
        "model": model,
        "max_tokens": 8000,
        "system": system,
        "messages": [{ "role": "user", "content": user }],
        "tools": [{ "name": "answer", "description": "The answer, in this shape.", "input_schema": schema }],
        "tool_choice": { "type": "tool", "name": "answer" },
    });
    let mut resp = agent()
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .send_json(&body)
        .map_err(|e| format!("Anthropic: {e}"))?;
    let status = resp.status().as_u16();
    let j: J = resp.body_mut().read_json().map_err(|e| format!("Anthropic: {e}"))?;
    if status != 200 {
        return Err(format!("Anthropic answered {status}: {}", j["error"]["message"].as_str().unwrap_or("")));
    }
    j["content"]
        .as_array()
        .and_then(|c| c.iter().find(|b| b["type"] == "tool_use"))
        .map(|b| b["input"].clone())
        .ok_or_else(|| "Anthropic answered without the answer".into())
}

/// An OpenAI-compatible endpoint, asked for JSON of the schema's shape. Not every server holds a
/// model to a schema, so the answer is also looked for inside whatever text comes back.
fn openai(url: &str, key: Option<&str>, model: &str, system: &str, user: &str, schema: &J) -> Result<J, String> {
    let body = json!({
        "model": model,
        "temperature": 0,
        "messages": [
            { "role": "system", "content": format!("{system}\n\nAnswer with one JSON object and nothing else, of this JSON Schema:\n{schema}") },
            { "role": "user", "content": user },
        ],
        "response_format": { "type": "json_schema", "json_schema": { "name": "answer", "schema": schema } },
    });
    let mut req = agent().post(&format!("{url}/chat/completions"));
    if let Some(k) = key {
        req = req.header("Authorization", &format!("Bearer {k}"));
    }
    let mut resp = req.send_json(&body).map_err(|e| format!("{url}: {e}"))?;
    let status = resp.status().as_u16();
    let j: J = resp.body_mut().read_json().map_err(|e| format!("{url}: {e}"))?;
    if status != 200 {
        return Err(format!("{url} answered {status}: {}", j["error"]["message"].as_str().unwrap_or(&j.to_string())));
    }
    let text = j["choices"][0]["message"]["content"].as_str().unwrap_or("");
    json_in(text).ok_or_else(|| format!("{model} did not answer with JSON: {}", text.chars().take(300).collect::<String>()))
}

/// The first whole JSON object in a text, fenced or not.
fn json_in(text: &str) -> Option<J> {
    if let Ok(j) = serde_json::from_str::<J>(text.trim()) {
        return Some(j);
    }
    let start = text.find('{')?;
    let mut depth = 0i32;
    let (mut quoted, mut escaped) = (false, false);
    for (i, c) in text[start..].char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '{' if !quoted => depth += 1,
            '}' if !quoted => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_str(&text[start..start + i + 1]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

// -- keys ------------------------------------------------------------------------------------

fn key_path(provider: &str) -> PathBuf {
    crate::identity::home().join("assist").join(format!("{provider}.key"))
}

/// A world's own key, beside its workspace: where worlds share a machine, each pays for its own.
fn world_key(root: &Path, provider: &str) -> Option<String> {
    std::fs::read_to_string(root.join(format!("assist-{provider}.key"))).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn stored_key(provider: &str) -> Option<String> {
    std::fs::read_to_string(key_path(provider)).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// A key, from standard input, kept where only this user reads it.
pub fn store_key(provider: &str) -> Result<PathBuf, String> {
    if !matches!(provider, "anthropic" | "openai") {
        return Err(format!("{provider}: a key is for anthropic or openai"));
    }
    let mut key = String::new();
    std::io::stdin().read_line(&mut key).map_err(|e| e.to_string())?;
    let key = key.trim();
    if key.is_empty() {
        return Err("no key on standard input".into());
    }
    let path = key_path(provider);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, format!("{key}\n")).map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

// -- consent ---------------------------------------------------------------------------------

/// Beside a source's declaration: that its sample may be sent, to whom, and every time it was.
pub const CONSENT: &str = "assist.yaml";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Consent {
    allowed: bool,
    #[serde(default)]
    sent: Vec<Sent>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sent {
    at: String,
    to: String,
    task: String,
    sends: Vec<String>,
}

/// Whether this source's material may go to a model. Asked once per source (D10): the person
/// sees what goes and says yes, and the yes is a file they can read and delete.
pub fn allowed(dir: &Path) -> bool {
    crate::yaml::read_or_default::<Consent>(&dir.join(CONSENT)).allowed
}

pub fn allow(dir: &Path) -> Result<(), String> {
    let path = dir.join(CONSENT);
    let mut c: Consent = crate::yaml::read_or_default(&path);
    c.allowed = true;
    write_consent(&path, &c)
}

/// Written down when it happens, not before: a record of what was sent is not a promise.
pub fn sent(dir: &Path, task: &str, d: &Disclosure) -> Result<(), String> {
    let path = dir.join(CONSENT);
    let mut c: Consent = crate::yaml::read_or_default(&path);
    c.sent.push(Sent { at: crate::iso_stamp(crate::now()), to: d.to.clone(), task: task.into(), sends: d.sends.clone() });
    write_consent(&path, &c)
}

fn write_consent(path: &Path, c: &Consent) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let text = format!(
        "# Whether the assist may send what this source holds to a model, and every time it did.\n\
         # Delete this file to be asked again.\n{}",
        crate::yaml::to_string(c)?
    );
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_is_found_in_what_a_model_wraps_it_in() {
        assert_eq!(json_in("{\"a\": 1}"), Some(json!({"a": 1})));
        assert_eq!(json_in("Here:\n```json\n{\"a\": \"}\", \"b\": {\"c\": 2}}\n```\nDone."), Some(json!({"a": "}", "b": {"c": 2}})));
        assert_eq!(json_in("no json"), None);
    }
}

/// A key given on a page rather than on standard input, kept the same way: for whoever runs the
/// program, or, given a world, for that world alone.
pub fn keep_key(provider: &str, key: &str, world: Option<&Path>) -> Result<PathBuf, String> {
    if !matches!(provider, "anthropic" | "openai") {
        return Err(format!("{provider}: a key is for anthropic or openai"));
    }
    let key = key.trim();
    if key.is_empty() || key.contains(char::is_whitespace) {
        return Err("that is not a key".into());
    }
    let path = match world {
        Some(root) => root.join(format!("assist-{provider}.key")),
        None => key_path(provider),
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, format!("{key}\n")).map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

