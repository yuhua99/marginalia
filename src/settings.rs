use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use toml_edit::{DocumentMut, Item, Value};

use crate::store::xdg_dir;

/// What the `s` popup changes, kept in `$XDG_CONFIG_HOME/marginalia/settings.toml`.
#[derive(Deserialize)]
#[serde(default)]
pub struct Settings {
    /// A theme name; empty or unknown means the default.
    pub theme: String,
    /// Wrap long code lines; off cuts them at the edge.
    pub wrap: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            theme: String::new(),
            wrap: true,
        }
    }
}

impl Settings {
    /// The defaults when there is no file yet.
    pub fn load() -> Result<Settings> {
        let path = path()?;
        let Some(text) = read(&path)? else {
            return Ok(Settings::default());
        };
        toml_edit::de::from_str(&text).with_context(|| format!("parse {}", path.display()))
    }

    /// Writes the fields into the file, keeping the rest of it and its comments.
    pub fn save(&self) -> Result<()> {
        let path = path()?;
        let mut doc: DocumentMut = read(&path)?
            .unwrap_or_default()
            .parse()
            .with_context(|| format!("parse {}", path.display()))?;
        set(&mut doc, "theme", self.theme.as_str());
        set(&mut doc, "wrap", self.wrap);
        let dir = path.parent().expect("settings dir");
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        fs::write(&path, doc.to_string()).with_context(|| format!("write {}", path.display()))
    }
}

fn path() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?.join("settings.toml"))
}

/// Sets `key` to `v`, keeping the comments around the old value.
fn set(doc: &mut DocumentMut, key: &str, v: impl Into<Value>) {
    let mut v = v.into();
    if let Some(old) = doc.get(key).and_then(Item::as_value) {
        *v.decor_mut() = old.decor().clone();
    }
    doc[key] = Item::Value(v);
}

/// The file's text, or None when there is no file yet.
fn read(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}
