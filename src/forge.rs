//! Pull requests of GitHub and merge requests of GitLab on origin, listed through gh /
//! glab and fetched with git.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::process::{Command, Stdio};

use anyhow::{Result, anyhow, bail};
use serde::Deserialize;

use crate::repo::Repo;
use crate::settings;

/// Requests listed, at most.
const LIMIT: &str = "100";

#[derive(Clone, Copy)]
enum Kind {
    Github,
    Gitlab,
}

/// Origin's project on its forge.
#[derive(Clone)]
pub struct Forge {
    kind: Kind,
    host: String,
    /// `owner/repo`, or `group/subgroup/project`.
    path: String,
}

/// An open pull or merge request.
#[derive(Clone)]
pub struct Request {
    pub number: u64,
    pub title: String,
    pub author: String,
    /// The branch it merges into.
    pub target: String,
}

impl Forge {
    /// Origin's forge: github.com and gitlab.com, other hosts as `[forges]` of the
    /// settings says. The error tells the user what to do.
    pub fn of(repo: &Repo, forges: &BTreeMap<String, String>) -> Result<Forge> {
        let url = repo.origin()?;
        let Some((host, path)) = parse_url(&url) else {
            bail!("origin {url} is not on a forge");
        };
        let set = forges
            .iter()
            .find(|(h, _)| h.eq_ignore_ascii_case(&host))
            .map(|(_, kind)| kind.as_str());
        let kind = match set {
            Some("github") => Kind::Github,
            Some("gitlab") => Kind::Gitlab,
            Some(kind) => bail!(
                "[forges] \"{host}\" = \"{kind}\" in {}: use \"github\" or \"gitlab\"",
                settings::path()?.display()
            ),
            None if host == "github.com" => Kind::Github,
            None if host == "gitlab.com" => Kind::Gitlab,
            None => bail!(
                "{host} is not a known forge; add to {}:\n[forges]\n\"{host}\" = \"github\"  # or \"gitlab\"",
                settings::path()?.display()
            ),
        };
        Ok(Forge { kind, host, path })
    }

    /// The open requests, newest first.
    pub fn requests(&self) -> Result<Vec<Request>> {
        match self.kind {
            Kind::Github => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Pr {
                    number: u64,
                    title: String,
                    author: Login,
                    base_ref_name: String,
                }
                #[derive(Deserialize)]
                struct Login {
                    login: String,
                }
                let repo = format!("{}/{}", self.host, self.path);
                let fields = "number,title,author,baseRefName";
                let out = tool(
                    "gh",
                    &[
                        "pr", "list", "-R", &repo, "--limit", LIMIT, "--json", fields,
                    ],
                )?;
                let prs: Vec<Pr> = serde_json::from_slice(&out)?;
                Ok(prs
                    .into_iter()
                    .map(|p| Request {
                        number: p.number,
                        title: p.title,
                        author: p.author.login,
                        target: p.base_ref_name,
                    })
                    .collect())
            }
            Kind::Gitlab => {
                #[derive(Deserialize)]
                struct Mr {
                    iid: u64,
                    title: String,
                    author: Username,
                    target_branch: String,
                }
                #[derive(Deserialize)]
                struct Username {
                    username: String,
                }
                let repo = format!("https://{}/{}", self.host, self.path);
                let out = tool(
                    "glab",
                    &["mr", "list", "-R", &repo, "-P", LIMIT, "-F", "json"],
                )?;
                let mrs: Vec<Mr> = serde_json::from_slice(&out)?;
                Ok(mrs
                    .into_iter()
                    .map(|m| Request {
                        number: m.iid,
                        title: m.title,
                        author: m.author.username,
                        target: m.target_branch,
                    })
                    .collect())
            }
        }
    }

    /// `pull` or `merge`, before a request's number in a list.
    pub fn word(&self) -> &'static str {
        match self.kind {
            Kind::Github => "pull",
            Kind::Gitlab => "merge",
        }
    }

    /// Request `number` as the forge writes it: `#12` or `!12`.
    pub fn sign(&self, number: u64) -> String {
        match self.kind {
            Kind::Github => format!("#{number}"),
            Kind::Gitlab => format!("!{number}"),
        }
    }

    /// The ref of request `number`'s head, the same on origin and here.
    pub fn head(&self, number: u64) -> String {
        match self.kind {
            Kind::Github => format!("refs/pull/{number}/head"),
            Kind::Gitlab => format!("refs/merge-requests/{number}/head"),
        }
    }

    /// Fetches `r`'s head to `head` and its target to origin's branch; returns the base,
    /// where the head left the target.
    pub fn fetch(&self, repo: &Repo, r: &Request) -> Result<String> {
        let head = self.head(r.number);
        let target = format!("refs/remotes/origin/{}", r.target);
        repo.fetch(
            &[
                format!("+{head}:{head}"),
                format!("+refs/heads/{}:{target}", r.target),
            ],
            false,
        )?;
        repo.merge_base(&target, &head)
    }
}

/// Readies `base..end` to open: a request's head, as `Forge::head` names it, fetched
/// again, then the range verified. A rewritten head is taken too, for `verify` to tell.
/// `prompt`: the terminal is free to ask for a password.
pub fn refresh(repo: &Repo, base: &str, end: &str, prompt: bool) -> Result<()> {
    if end.starts_with("refs/pull/") || end.starts_with("refs/merge-requests/") {
        repo.fetch(&[format!("+{end}:{end}")], prompt)?;
    }
    repo.verify(base, end)
}

/// The output of `cli args`; its error is the cli's, which says how to sign in.
fn tool(cli: &str, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new(cli)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => anyhow!("{cli} not found; install it to list requests"),
            _ => anyhow!("run {cli}: {e}"),
        })?;
    if !out.status.success() {
        bail!("{cli}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

/// Host and path of a remote URL, `scheme://[user@]host[:port]/path` or
/// `[user@]host:path`, the path without `.git`; None for a local path.
fn parse_url(url: &str) -> Option<(String, String)> {
    let (host, path) = match url.split_once("://") {
        Some((_, rest)) => {
            let (authority, path) = rest.split_once('/')?;
            let host = authority.rsplit('@').next()?;
            (host.split(':').next()?, path)
        }
        None => {
            let (authority, path) = url.split_once(':')?;
            if authority.contains('/') {
                return None;
            }
            (authority.rsplit('@').next()?, path)
        }
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    (!host.is_empty() && !path.is_empty()).then(|| (host.to_lowercase(), path.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        for (url, want) in [
            ("git@github.com:o/r.git", Some(("github.com", "o/r"))),
            ("https://github.com/o/r", Some(("github.com", "o/r"))),
            (
                "https://u@GitLab.example.com/g/s/p.git/",
                Some(("gitlab.example.com", "g/s/p")),
            ),
            (
                "ssh://git@git.example.com:2222/g/p.git",
                Some(("git.example.com", "g/p")),
            ),
            ("/srv/repo.git", None),
            ("./a:b", None),
            ("file:///srv/repo.git", None),
        ] {
            let got = parse_url(url);
            let got = got.as_ref().map(|(h, p)| (h.as_str(), p.as_str()));
            assert_eq!(got, want, "{url}");
        }
    }
}
