use std::fs;
use std::hash::{BuildHasher, RandomState};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::session::Session;

pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// `$XDG_STATE_HOME/marginalia/<repo key>/`
    pub fn open(repo_key: &str) -> Result<Store> {
        let state = match std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
            Some(v) => PathBuf::from(v),
            None => PathBuf::from(std::env::var_os("HOME").context("HOME not set")?)
                .join(".local/state"),
        };
        let dir = state.join("marginalia").join(repo_key);
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        Ok(Store { dir })
    }

    pub fn new_id(&self) -> String {
        loop {
            let id = format!(
                "{:04x}",
                RandomState::new().hash_one(SystemTime::now()) & 0xffff
            );
            if !self.path(&id).exists() {
                return id;
            }
        }
    }

    pub fn load(&self, id: &str) -> Result<Session> {
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("invalid session id {id:?}");
        }
        let text = fs::read_to_string(self.path(id)).with_context(|| format!("no session {id}"))?;
        serde_json::from_str(&text).with_context(|| format!("parse session {id}"))
    }

    pub fn save(&self, session: &Session) -> Result<()> {
        let path = self.path(&session.id);
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(session)?)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Sessions with their last update time (unix seconds), newest first.
    pub fn list(&self) -> Result<Vec<(Session, u64)>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let id = path.file_stem().unwrap().to_string_lossy();
            let updated = fs::metadata(&path)?
                .modified()?
                .duration_since(UNIX_EPOCH)?
                .as_secs();
            let session = self
                .load(&id)
                .with_context(|| format!("load {}", path.display()))?;
            out.push((session, updated));
        }
        out.sort_by(|a, b| b.1.cmp(&a.1));
        Ok(out)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        fs::remove_file(self.path(id)).with_context(|| format!("delete session {id}"))
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }
}
