//! A workspace written by 0.1, rewritten in the words and the format of 0.2.
//!
//! Every file is read in its old form, its keys are moved to their new names, and the result is
//! read back as the new declaration before anything is written. A file that does not survive that
//! is named and nothing of its source is touched. Comments do not survive a change of format; how
//! many were dropped is said per file, so somebody who wrote one knows to carry it across.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value as J};

/// What was done, one line each, in the order it was done.
pub struct Done(pub Vec<String>);

pub fn workspace(root: &Path) -> Result<Done, String> {
    let mut done = Vec::new();
    for (old, new) in [("datasets", "sources"), ("scopes", "trackers")] {
        if root.join(old).is_dir() && root.join(new).is_dir() {
            return Err(format!(
                "{} holds both {old}/ and {new}/, which is a migration that stopped half way. \
                 Move one of them aside and run it again",
                root.display()
            ));
        }
    }

    if root.join("datasets").is_dir() {
        for dir in children(&root.join("datasets"))? {
            if dir.join("dataset.toml").exists() {
                done.push(source(&dir)?);
            }
        }
        rename(&root.join("datasets"), &root.join("sources"), &mut done)?;
    }
    if root.join("scopes").is_dir() {
        for dir in children(&root.join("scopes"))? {
            if dir.join("scope.toml").exists() {
                done.push(tracker(&dir)?);
            }
        }
        rename(&root.join("scopes"), &root.join("trackers"), &mut done)?;
    }
    if root.join("watches").is_dir() {
        for path in files(&root.join("watches"), "toml")? {
            done.push(watch(&path)?);
        }
    }
    for (old, new) in [("zetlyn.toml", "workspace.yaml"), ("owners.toml", "owners.yaml")] {
        let path = root.join(old);
        if path.exists() {
            done.push(plain(&path, &root.join(new))?);
        }
    }
    // A hub is migrated like a workspace, because its trees have the same names.
    if root.join("owners.yaml").exists() {
        for (old, new) in [("datasets", "sources"), ("scopes", "trackers")] {
            if root.join(old).is_dir() {
                done.push(format!(
                    "{}: this is a hub, and what it carries was built against specification 1.0. \
                     Publish it again under {new}/ rather than moving it",
                    root.join(old).display()
                ));
            }
        }
    }
    if root.join("platform.toml").exists() || root.join("deployments").is_dir() {
        done.push(format!(
            "{}: a platform is not a workspace. Its grants were signed over the words of 0.1 and \
             have to be made again, so there is nothing here to carry across",
            root.display()
        ));
    }
    if done.is_empty() {
        return Err(format!("{}: nothing here is from before 0.2", root.display()));
    }
    Ok(Done(done))
}

/// `~/.zetlyn/identity.toml`, where there is one.
pub fn identity(home: &Path) -> Result<Option<String>, String> {
    let path = home.join("identity.toml");
    if !path.exists() {
        return Ok(None);
    }
    plain(&path, &home.join("identity.yaml")).map(Some)
}

fn source(dir: &Path) -> Result<String, String> {
    let path = dir.join("dataset.toml");
    let (mut j, comments) = read_toml(&path)?;
    let o = object(&mut j, &path)?;
    move_key(o, "source", "fetch");
    move_key(o, "records", "claims");
    move_key(o, "view", "views");
    if let Some(claims) = o.get_mut("claims").and_then(J::as_object_mut) {
        move_key(claims, "fields", "properties");
    }
    if let Some(each) = o
        .get_mut("fetch")
        .and_then(|f| f.get_mut("for_each"))
        .and_then(J::as_object_mut)
    {
        move_key(each, "dataset", "source");
    }
    if let Some(views) = o.get_mut("views").and_then(J::as_array_mut) {
        for v in views {
            rewrite_view(v);
        }
    }
    let decl: crate::decl::Declaration = serde_json::from_value(j)
        .map_err(|e| format!("{}: does not make a source: {e}", path.display()))?;
    crate::yaml::write(&dir.join(crate::decl::FILE), &decl)?;
    std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let _ = std::fs::remove_file(dir.join("dataset.toml.before"));
    for tail in ["", "-wal", "-shm"] {
        let old = dir.join(format!("records.db{tail}"));
        if old.exists() {
            let new = dir.join(format!("claims.db{tail}"));
            std::fs::rename(&old, &new).map_err(|e| format!("{}: {e}", old.display()))?;
        }
    }
    Ok(said(dir.join(crate::decl::FILE), comments))
}

fn tracker(dir: &Path) -> Result<String, String> {
    let path = dir.join("scope.toml");
    let (mut j, comments) = read_toml(&path)?;
    let o = object(&mut j, &path)?;
    move_key(o, "members", "sources");
    move_key(o, "normalise", "align");
    if let Some(members) = o.get_mut("sources").and_then(J::as_array_mut) {
        for m in members.iter_mut().filter_map(J::as_object_mut) {
            move_key(m, "dataset", "source");
        }
    }
    if let Some(join) = o.remove("join") {
        let keys: Vec<J> = join
            .as_array()
            .map(|a| a.iter().filter_map(|k| k.get("key").cloned()).collect())
            .unwrap_or_default();
        o.insert("identified_by".into(), J::Array(keys));
    }
    if let Some(view) = o.get_mut("view").and_then(J::as_object_mut) {
        rename_in_list(view.get_mut("facets"), "dataset", "source");
        rename_in_list(view.get_mut("columns"), "dataset", "source");
        if let Some(named) = view.get_mut("named").and_then(J::as_array_mut) {
            for v in named {
                rewrite_view(v);
            }
        }
        for (_, kind) in view.iter_mut() {
            if let Some(k) = kind.as_object_mut() {
                rename_in_list(k.get_mut("facets"), "dataset", "source");
                rename_in_list(k.get_mut("columns"), "dataset", "source");
                k.remove("divergence");
            }
        }
    }
    let decl: crate::scopedecl::ScopeDecl = serde_json::from_value(j)
        .map_err(|e| format!("{}: does not make a tracker: {e}", path.display()))?;
    crate::yaml::write(&dir.join(crate::scopedecl::FILE), &decl)?;
    std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(said(dir.join(crate::scopedecl::FILE), comments))
}

fn watch(path: &Path) -> Result<String, String> {
    let (mut j, comments) = read_toml(path)?;
    let o = object(&mut j, path)?;
    move_key(o, "scope", "tracker");
    move_key(o, "dataset", "source");
    if let Some(q) = o.get_mut("query") {
        *q = json!(query(q.as_str().unwrap_or_default()));
    }
    let decl: crate::watch::WatchDecl = serde_json::from_value(j)
        .map_err(|e| format!("{}: does not make a watch: {e}", path.display()))?;
    let new = path.with_extension("yaml");
    crate::yaml::write(&new, &decl)?;
    std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(said(new, comments))
}

/// A file whose keys did not change, only its format.
fn plain(old: &Path, new: &Path) -> Result<String, String> {
    let (j, comments) = read_toml(old)?;
    crate::yaml::write(new, &j)?;
    std::fs::remove_file(old).map_err(|e| format!("{}: {e}", old.display()))?;
    Ok(said(new.to_path_buf(), comments))
}

/// `dataset` was a word a query could use, as the name of the member a record came from.
fn query(q: &str) -> String {
    regex::Regex::new(r"\bdataset\b")
        .map(|re| re.replace_all(q, "source").into_owned())
        .unwrap_or_else(|_| q.to_string())
}

fn rewrite_view(v: &mut J) {
    let Some(o) = v.as_object_mut() else {
        return;
    };
    if let Some(w) = o.get_mut("where") {
        *w = json!(query(w.as_str().unwrap_or_default()));
    }
    rename_in_list(o.get_mut("facets"), "dataset", "source");
    rename_in_list(o.get_mut("columns"), "dataset", "source");
}

fn rename_in_list(list: Option<&mut J>, old: &str, new: &str) {
    if let Some(items) = list.and_then(J::as_array_mut) {
        for item in items {
            if item.as_str() == Some(old) {
                *item = json!(new);
            }
        }
    }
}

fn move_key(o: &mut Map<String, J>, old: &str, new: &str) {
    if let Some(v) = o.remove(old) {
        o.insert(new.to_string(), v);
    }
}

fn object<'a>(j: &'a mut J, path: &Path) -> Result<&'a mut Map<String, J>, String> {
    j.as_object_mut()
        .ok_or_else(|| format!("{}: not a table", path.display()))
}

/// The file as JSON, and how many comment lines it carried.
fn read_toml(path: &Path) -> Result<(J, usize), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let value: toml::Value = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let comments = text
        .lines()
        .filter(|l| l.trim_start().starts_with('#'))
        .count();
    let j = serde_json::to_value(value).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((j, comments))
}

fn said(path: PathBuf, comments: usize) -> String {
    match comments {
        0 => format!("{}", path.display()),
        1 => format!("{}  (1 comment line not carried)", path.display()),
        n => format!("{}  ({n} comment lines not carried)", path.display()),
    }
}

fn rename(old: &Path, new: &Path, done: &mut Vec<String>) -> Result<(), String> {
    std::fs::rename(old, new).map_err(|e| format!("{}: {e}", old.display()))?;
    done.push(format!("{} → {}", old.display(), new.display()));
    Ok(())
}

fn children(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    Ok(out)
}

fn files(dir: &Path, ext: &str) -> Result<Vec<PathBuf>, String> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == ext).unwrap_or(false))
        .collect();
    out.sort();
    Ok(out)
}
