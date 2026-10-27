use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::store::xdg_dir;

/// What the `s` popup changes, kept in `$XDG_CONFIG_HOME/marginalia/settings.json`.
#[derive(Serialize, Deserialize)]
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
        match fs::read_to_string(&path) {
            Ok(text) => {
                serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = path()?;
        let dir = path.parent().expect("settings dir");
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        fs::write(&path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("write {}", path.display()))
    }
}

fn path() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?.join("settings.json"))
}
