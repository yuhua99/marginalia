use std::process::Command;

use anyhow::{Context, Result, bail};

pub struct Repo {
    common_dir: String,
}

impl Repo {
    pub fn open() -> Result<Repo> {
        let common_dir = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
        Ok(Repo { common_dir })
    }

    /// Identifies the repo; worktrees of one repo share the same key.
    pub fn key(&self) -> String {
        self.common_dir.replace('/', "%")
    }

    /// Splits `A..end` into the base commit sha and the end
    /// (a commit sha, or `HEAD` / `staged` / `unstaged` kept symbolic).
    pub fn resolve(&self, range: &str) -> Result<(String, String)> {
        let Some((base, end)) = range.split_once("..") else {
            bail!("range must look like A..END, got {range:?}");
        };
        let base = commit(base)?;
        let end = match end {
            "HEAD" | "staged" | "unstaged" => end.to_string(),
            _ => commit(end)?,
        };
        self.verify(&base, &end)?;
        Ok((base, end))
    }

    /// Fails when `base` is no longer an ancestor of the range tip (history rewritten).
    pub fn verify(&self, base: &str, end: &str) -> Result<()> {
        let tip = match end {
            "staged" | "unstaged" => "HEAD",
            _ => end,
        };
        let ok = Command::new("git")
            .args(["merge-base", "--is-ancestor", base, tip])
            .status()
            .context("run git")?
            .success();
        if !ok {
            bail!("{base} is not an ancestor of {tip} (history rewritten?)");
        }
        Ok(())
    }
}

fn commit(rev: &str) -> Result<String> {
    git(&["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")])
        .with_context(|| format!("unknown commit {rev:?}"))
}

fn git(args: &[&str]) -> Result<String> {
    let out = Command::new("git").args(args).output().context("run git")?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8(out.stdout)?.trim_end().to_string())
}
