use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use toml_edit::{DocumentMut, Item, Table, Value};

use crate::keymap::{self, Keys};
use crate::store::xdg_dir;

#[derive(Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: String,
    pub wrap: bool,
    pub keys: Keys,
    pub forges: BTreeMap<String, String>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            theme: String::new(),
            wrap: true,
            keys: Keys::new(),
            forges: BTreeMap::new(),
        }
    }
}

impl Settings {
    pub fn load() -> Result<Settings> {
        let path = path()?;
        let text = match read(&path)? {
            Some(text) => text,
            None => {
                let text = defaults().to_string();
                write(&path, &text)?;
                text
            }
        };
        toml_edit::de::from_str(&text).with_context(|| format!("parse {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = path()?;
        let mut doc: DocumentMut = read(&path)?
            .unwrap_or_default()
            .parse()
            .with_context(|| format!("parse {}", path.display()))?;
        set(&mut doc, "theme", self.theme.as_str());
        set(&mut doc, "wrap", self.wrap);
        write(&path, &doc.to_string())
    }
}

fn defaults() -> DocumentMut {
    let settings = Settings::default();
    let mut doc = DocumentMut::new();
    set(&mut doc, "theme", settings.theme.as_str());
    set(&mut doc, "wrap", settings.wrap);
    let mut keys = Table::new();
    keys.set_implicit(true);
    for (context, bindings) in keymap::defaults() {
        keys[context] = Item::Table(bindings.iter().copied().collect());
    }
    doc["keys"] = Item::Table(keys);
    doc
}

fn write(path: &Path, text: &str) -> Result<()> {
    let dir = path.parent().expect("settings dir");
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

pub fn path() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?.join("settings.toml"))
}

fn set(doc: &mut DocumentMut, key: &str, v: impl Into<Value>) {
    let mut v = v.into();
    if let Some(old) = doc.get(key).and_then(Item::as_value) {
        *v.decor_mut() = old.decor().clone();
    }
    doc[key] = Item::Value(v);
}

fn read(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}
