//! A world's archive, moved in pieces: an upload larger than any one request may be, taken a few
//! megabytes at a time, so no piece waits on a slow line long enough for anything between the
//! browser and here to give up on it, and an upload that breaks off goes on where it stopped.
//!
//!   begin   the size and name of the file: an upload to put the pieces in, or the one already
//!           begun for that same file, with the pieces it has
//!   piece   one piece, by its number, each the same size but the last
//!   finish  the pieces in order as one archive, checked to be an export
//!
//! Where the pieces wait: a cell's `incoming/upload/`, the server's to bring in (cell.rs); on one's
//! own machine, the workspace's `.zetlyn/incoming/upload/`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value as J};

/// A piece: small enough to arrive within a minute on a line of well under one megabit.
pub const PIECE: u64 = 4 << 20;

const META: &str = "upload.json";

fn upload_dir(base: &Path) -> PathBuf {
    base.join("upload")
}

fn piece_path(base: &Path, n: u64) -> PathBuf {
    upload_dir(base).join(format!("{n:06}.part"))
}

fn pieces_of(size: u64) -> u64 {
    size.div_ceil(PIECE).max(1)
}

/// The pieces already here, by number.
fn have(base: &Path, size: u64) -> Vec<u64> {
    (0..pieces_of(size)).filter(|n| std::fs::metadata(piece_path(base, *n)).is_ok_and(|m| m.len() == expected(size, *n))).collect()
}

fn expected(size: u64, n: u64) -> u64 {
    let last = pieces_of(size) - 1;
    if n < last { PIECE } else { size - last * PIECE }
}

fn meta(base: &Path) -> Option<J> {
    std::fs::read(upload_dir(base).join(META)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// An upload of `size` bytes called `name`, at most `limit`: the one begun for the same file if
/// there is one, with what it has, or a new one in its place.
pub fn begin(base: &Path, size: u64, name: &str, limit: u64, by: &str) -> Result<J, String> {
    if size == 0 {
        return Err("The file is empty.".into());
    }
    if size > limit {
        return Err(format!("Larger than the {} GB allowed here.", limit >> 30));
    }
    if let Some(m) = meta(base) {
        if m["size"].as_u64() == Some(size) && m["name"].as_str() == Some(name) {
            return Ok(json!({ "id": m["id"], "piece": PIECE, "pieces": pieces_of(size), "have": have(base, size) }));
        }
    }
    let dir = upload_dir(base);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let id = crate::jwt::random();
    let m = json!({ "id": id, "size": size, "name": name, "by": by, "at": crate::iso_stamp(crate::now()) });
    std::fs::write(dir.join(META), m.to_string()).map_err(|e| e.to_string())?;
    Ok(json!({ "id": id, "piece": PIECE, "pieces": pieces_of(size), "have": [] }))
}

/// The upload `id` is, its size, or why there is none.
fn current(base: &Path, id: &str) -> Result<u64, String> {
    let m = meta(base).ok_or("No upload is waiting here; begin again.")?;
    if m["id"].as_str() != Some(id) {
        return Err("Another upload took this one's place; begin again.".into());
    }
    m["size"].as_u64().ok_or_else(|| "The upload says no size.".to_string())
}

/// Piece `n` of upload `id`, as it arrives.
pub fn piece(base: &Path, id: &str, n: u64, body: &mut dyn Read) -> Result<J, String> {
    let size = current(base, id)?;
    if n >= pieces_of(size) {
        return Err(format!("There is no piece {n} of {}.", pieces_of(size)));
    }
    let want = expected(size, n);
    let path = piece_path(base, n);
    let partial = path.with_extension("arriving");
    let mut out = std::fs::File::create(&partial).map_err(|e| e.to_string())?;
    let got = std::io::copy(&mut body.take(want + 1), &mut out).map_err(|e| format!("The piece broke off: {e}"))?;
    out.flush().map_err(|e| e.to_string())?;
    if got != want {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("Piece {n} came with {got} bytes, not {want}."));
    }
    std::fs::rename(&partial, &path).map_err(|e| e.to_string())?;
    Ok(json!({ "have": have(base, size).len(), "pieces": pieces_of(size) }))
}

/// Upload `id`, whole: its pieces in order as `to`, checked to be a world's export, and the pieces
/// gone. Who uploaded it, and how many files it holds.
pub fn finish(base: &Path, id: &str, to: &Path) -> Result<(String, usize, u64), String> {
    let size = current(base, id)?;
    let missing: Vec<u64> = (0..pieces_of(size)).filter(|n| !have(base, size).contains(n)).collect();
    if !missing.is_empty() {
        return Err(format!("{} pieces are still missing.", missing.len()));
    }
    let by = meta(base).and_then(|m| m["by"].as_str().map(str::to_string)).unwrap_or_default();
    let partial = to.with_extension("partial");
    let result = (|| {
        let mut out = std::fs::File::create(&partial).map_err(|e| e.to_string())?;
        for n in 0..pieces_of(size) {
            let mut f = std::fs::File::open(piece_path(base, n)).map_err(|e| e.to_string())?;
            std::io::copy(&mut f, &mut out).map_err(|e| e.to_string())?;
        }
        out.flush().map_err(|e| e.to_string())?;
        let files = crate::world::check_export(&partial)?;
        std::fs::rename(&partial, to).map_err(|e| e.to_string())?;
        Ok(files)
    })();
    let _ = std::fs::remove_file(&partial);
    let _ = std::fs::remove_dir_all(upload_dir(base));
    result.map(|files| (by, files, size))
}

/// Whether an upload is under way, and how far: for the settings page.
pub fn progress(base: &Path) -> Option<(String, u64, u64)> {
    let m = meta(base)?;
    let size = m["size"].as_u64()?;
    Some((m["name"].as_str().unwrap_or("").to_string(), have(base, size).len() as u64, pieces_of(size)))
}

// -- on one's own machine, or a server of one's own -----------------------------------------------
//
// What a cell's server does for it (cell.rs), done here by the program itself, each on a thread of
// its own, its state in a file beside the workspace's other state for the page to show.

/// Where uploads wait, and what the last import came to.
pub fn local_incoming(root: &Path) -> PathBuf {
    root.join(".zetlyn").join("incoming")
}

fn exports_dir(root: &Path) -> PathBuf {
    root.join(".zetlyn").join("exports")
}

fn read_json(path: &Path) -> Option<J> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

fn write_json(path: &Path, value: &J) {
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(path, value.to_string());
}

/// The export being made, or the last one made: `state` making, ready or failed.
pub fn export_state(root: &Path) -> Option<J> {
    read_json(&exports_dir(root).join("state.json"))
}

/// The whole world as one archive, made on a thread of its own; the one before it goes.
pub fn start_export(root: &Path) -> Result<(), String> {
    if export_state(root).is_some_and(|s| s["state"] == "making") {
        return Err("An export is being made already.".into());
    }
    let root = root.to_path_buf();
    let dir = exports_dir(&root);
    let state = dir.join("state.json");
    // Where nothing can be written (a full disk, a folder not ours), said now rather than never.
    std::fs::create_dir_all(&dir).map_err(|e| format!("Not exported: {}: {e}", dir.display()))?;
    std::fs::write(&state, json!({ "state": "making", "at": crate::iso_stamp(crate::now()) }).to_string()).map_err(|e| format!("Not exported: {}: {e}", dir.display()))?;
    std::thread::spawn(move || {
        let name = format!("{}-{}.tar.gz", root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "world".into()), crate::cell::stamp());
        let file = dir.join(&name);
        let done = match crate::world::export(&root, &file) {
            Ok(files) => {
                for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                    let n = e.file_name().to_string_lossy().into_owned();
                    if n.ends_with(".tar.gz") && n != name {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
                let bytes = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
                json!({ "state": "ready", "at": crate::iso_stamp(crate::now()), "file": name, "files": files, "bytes": bytes })
            }
            Err(e) => json!({ "state": "failed", "at": crate::iso_stamp(crate::now()), "error": e }),
        };
        write_json(&state, &done);
    });
    Ok(())
}

/// The export ready under that name, and nothing else of the workspace.
pub fn export_file(root: &Path, name: &str) -> Option<PathBuf> {
    let ready = export_state(root).filter(|s| s["state"] == "ready")?;
    (ready["file"].as_str() == Some(name) && !name.contains(['/', '\\'])).then(|| exports_dir(root).join(name)).filter(|p| p.is_file())
}

/// Whether an import is under way, and what the last one came to.
pub fn import_state(root: &Path) -> (bool, Option<J>) {
    let incoming = local_incoming(root);
    (incoming.join("importing.json").exists(), read_json(&incoming.join("last-import.json")))
}

/// The archive at `archive` in this workspace's place, on a thread of its own: imported beside it,
/// then the two swapped, so the workspace as it was stays whole beside it as
/// `<name>.before-import-<stamp>`. What `keep` names of this workspace.yaml stays as it is: on a
/// server of one's own, who owns it and where it answers.
pub fn start_import(root: &Path, archive: PathBuf, keep: &[&str], by: &str, beside: bool) -> Result<String, String> {
    let incoming = local_incoming(root);
    if incoming.join("importing.json").exists() {
        return Err("An import is under way already.".into());
    }
    write_json(&incoming.join("importing.json"), &json!({ "at": crate::iso_stamp(crate::now()), "by": by }));
    let (root, keep) = (root.to_path_buf(), keep.iter().map(|k| k.to_string()).collect::<Vec<_>>());
    let by = by.to_string();
    std::thread::spawn(move || {
        let last = if beside {
            // Beside what is here: added to it, nothing here replaced (world.rs).
            match crate::world::import_beside(&archive, &root) {
                Ok(said) => json!({ "at": crate::iso_stamp(crate::now()), "by": by, "ok": true, "beside": said }),
                Err(e) => json!({ "at": crate::iso_stamp(crate::now()), "by": by, "ok": false, "error": e }),
            }
        } else {
            match import_local(&root, &archive, &keep) {
                Ok((n, before)) => json!({ "at": crate::iso_stamp(crate::now()), "by": by, "ok": true, "files": n, "before": before.display().to_string() }),
                Err(e) => json!({ "at": crate::iso_stamp(crate::now()), "by": by, "ok": false, "error": e }),
            }
        };
        let _ = std::fs::remove_file(&archive);
        // Into whichever workspace is here now: the new one, or the one that stayed.
        write_json(&local_incoming(&root).join("last-import.json"), &last);
        let _ = std::fs::remove_file(local_incoming(&root).join("importing.json"));
    });
    Ok(if beside { "Received. It is added beside what is here in a moment.".into() } else { "Received. It takes this world's place in a moment; what is here now is kept beside it.".into() })
}

fn import_local(root: &Path, archive: &Path, keep: &[String]) -> Result<(usize, PathBuf), String> {
    let parent = root.parent().ok_or("the workspace has no folder around it")?;
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or("the workspace has no name")?;
    let stamp = crate::cell::stamp();
    let fresh = parent.join(format!("{name}.importing-{stamp}"));
    let before = parent.join(format!("{name}.before-import-{stamp}"));
    let n = match crate::world::import(archive, &fresh, None, None) {
        Ok(n) => n,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&fresh);
            return Err(e);
        }
    };
    let ws = crate::account::WORKSPACE;
    for k in keep {
        carry(&root.join(ws), &fresh.join(ws), k)?;
    }
    std::fs::rename(root, &before).map_err(|e| format!("{}: {e}", root.display()))?;
    if let Err(e) = std::fs::rename(&fresh, root) {
        let _ = std::fs::rename(&before, root);
        let _ = std::fs::remove_dir_all(&fresh);
        return Err(format!("{}: {e}", root.display()));
    }
    // What was waiting here is the old one's; the new one starts with none.
    let _ = std::fs::create_dir_all(local_incoming(root));
    Ok((n, before))
}

/// A top-level key of one workspace.yaml put in another as it is, or taken out where the first has
/// none: written in YAML's flow form, which holds a list as well as a word.
fn carry(from: &Path, to: &Path, key: &str) -> Result<(), String> {
    let source: J = crate::yaml::parse(&std::fs::read_to_string(from).unwrap_or_default()).unwrap_or(J::Null);
    let text = std::fs::read_to_string(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let mut out: Vec<&str> = Vec::new();
    let mut skipping = false;
    for l in text.lines() {
        if l.starts_with(&format!("{key}:")) {
            skipping = true;
            continue;
        }
        if skipping && (l.starts_with(' ') || l.starts_with('-') || l.is_empty()) {
            continue;
        }
        skipping = false;
        out.push(l);
    }
    let mut new = out.join("\n");
    if let Some(v) = source.get(key).filter(|v| !v.is_null()) {
        new.push_str(&format!("\n{key}: {v}"));
    }
    std::fs::write(to, format!("{}\n", new.trim_end())).map_err(|e| e.to_string())
}

/// The last sync asked from the page, or the one under way.
pub fn sync_state(root: &Path) -> Option<J> {
    crate::sync::last(root)
}

/// A sync with the world at `url`, on a thread of its own; where both changed one thing and
/// `take` does not decide, nothing is synced and the state names each (sync.rs).
pub fn start_sync(root: &Path, url: &str, key: &str, take: Option<&str>) -> Result<(), String> {
    if sync_state(root).is_some_and(|s| s["state"] == "running") {
        return Err("A sync is under way already.".into());
    }
    let (root, url, key, take) = (root.to_path_buf(), url.to_string(), key.to_string(), take.map(str::to_string));
    // Said as running before the page asks again, so it shows it.
    let file = root.join(".zetlyn").join("sync").join("last.json");
    if let Some(d) = file.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    crate::sync::write_whole(&file, &json!({ "state": "running", "at": crate::iso_stamp(crate::now()), "t": crate::now(), "url": url }));
    std::thread::spawn(move || {
        crate::sync::run_recorded(&root, &url, &key, take.as_deref());
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_upload_in_pieces_is_whole_again_and_goes_on_where_it_stopped() {
        let base = std::env::temp_dir().join(format!("zetlyn-transfer-{}", crate::jwt::random()));
        let world = base.join("world");
        std::fs::create_dir_all(&world).unwrap();
        std::fs::write(world.join(crate::account::WORKSPACE), "title: Pieces\n").unwrap();
        // Something that does not compress, so the archive is more than one piece.
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let noise: Vec<u8> = (0..(PIECE as usize + 300_000))
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect();
        std::fs::write(world.join("noise.bin"), &noise).unwrap();
        let archive = base.join("w.tar.gz");
        crate::world::export(&world, &archive).unwrap();
        let bytes = std::fs::read(&archive).unwrap();
        let size = bytes.len() as u64;
        assert!(size > PIECE, "{size}");

        let incoming = base.join("incoming");
        let b = begin(&incoming, size, "w.tar.gz", 1 << 30, "a@example.org").unwrap();
        let id = b["id"].as_str().unwrap().to_string();
        assert_eq!(b["pieces"], 2);
        piece(&incoming, &id, 0, &mut &bytes[..PIECE as usize]).unwrap();
        // Broken off: begun again for the same file, it has the first piece already.
        let again = begin(&incoming, size, "w.tar.gz", 1 << 30, "a@example.org").unwrap();
        assert_eq!(again["id"].as_str(), Some(id.as_str()));
        assert_eq!(again["have"], json!([0]));
        assert!(finish(&incoming, &id, &base.join("in.tar.gz")).is_err(), "a piece is missing");
        assert!(piece(&incoming, &id, 1, &mut &bytes[PIECE as usize..size as usize - 1]).is_err(), "a short piece is refused");
        piece(&incoming, &id, 1, &mut &bytes[PIECE as usize..]).unwrap();
        let (by, files, n) = finish(&incoming, &id, &base.join("in.tar.gz")).unwrap();
        assert_eq!((by.as_str(), files, n), ("a@example.org", 2, size));
        assert_eq!(std::fs::read(base.join("in.tar.gz")).unwrap(), bytes);
        assert!(progress(&incoming).is_none());
        assert!(begin(&incoming, size, "w.tar.gz", PIECE, "").is_err(), "larger than allowed");
        let _ = std::fs::remove_dir_all(&base);
    }
}
