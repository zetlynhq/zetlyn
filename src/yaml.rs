//! Every file a person writes is YAML. One place reads them and one writes them, so an error
//! names the file and the line the same way everywhere.

use std::path::Path;

use serde::de::DeserializeOwned;
use serde::Serialize;

/// The files 0.1 wrote, by what they became. A file in the old form is refused rather than read:
/// two formats with two sets of keys is a second parser kept alive for nobody.
pub const BEFORE: [(&str, &str); 8] = [
    ("dataset.toml", "source.yaml"),
    ("scope.toml", "tracker.yaml"),
    ("zetlyn.toml", "workspace.yaml"),
    ("platform.toml", "platform.yaml"),
    ("owners.toml", "owners.yaml"),
    ("grant.toml", "grant.yaml"),
    ("identity.toml", "identity.yaml"),
    ("records.db", "claims.db"),
];

pub fn read<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => match older(path) {
            Some(old) => format!(
                "{} is from before 0.2. `zetlyn migrate` rewrites it as {}",
                old.display(),
                path.display()
            ),
            None => format!("{}: {e}", path.display()),
        },
        _ => format!("{}: {e}", path.display()),
    })?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// For a file that may be absent, where absent means the defaults. One that is there and does
/// not read, or one from before 0.2, is said on stderr: the defaults are then in force, and a
/// mailer somebody named and nobody uses is a thing that has to be visible somewhere.
pub fn read_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    if !path.exists() && older(path).is_none() {
        return T::default();
    }
    read(path).unwrap_or_else(|e| {
        eprintln!("zetlyn: {e}");
        T::default()
    })
}

pub fn parse<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    serde_saphyr::from_str(text).map_err(|e| e.to_string())
}

pub fn write<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = to_string(value)?;
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn to_string<T: Serialize>(value: &T) -> Result<String, String> {
    serde_saphyr::to_string(value).map_err(|e| e.to_string())
}

/// The file this one was called before 0.2, where it is still there under that name.
pub fn older(path: &Path) -> Option<std::path::PathBuf> {
    let name = path.file_name()?.to_str()?;
    let dir = path.parent()?;
    if let Some((old, _)) = BEFORE.iter().find(|(_, new)| *new == name) {
        let p = dir.join(old);
        return p.exists().then_some(p);
    }
    // A watch was `watches/<name>.toml`.
    let stem = name.strip_suffix(".yaml")?;
    let p = dir.join(format!("{stem}.toml"));
    p.exists().then_some(p)
}
