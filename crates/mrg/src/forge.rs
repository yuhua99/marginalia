use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::process::{Command, Stdio};

use anyhow::{Result, anyhow, bail};
use serde::Deserialize;

use crate::repo::{Repo, tracking};
use crate::session::Session;
use crate::settings;
use crate::store::Store;

const LIMIT: &str = "100";

#[derive(Clone, Copy)]
enum Kind {
    Github,
    Gitlab,
}

#[derive(Clone)]
pub struct Forge {
    kind: Kind,
    host: String,
    path: String,
}

#[derive(Clone)]
pub struct Request {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub target: String,
}

impl Forge {
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

    pub fn word(&self) -> &'static str {
        match self.kind {
            Kind::Github => "pull",
            Kind::Gitlab => "merge",
        }
    }

    pub fn sign(&self, number: u64) -> String {
        match self.kind {
            Kind::Github => format!("#{number}"),
            Kind::Gitlab => format!("!{number}"),
        }
    }

    pub fn head(&self, number: u64) -> String {
        match self.kind {
            Kind::Github => format!("refs/pull/{number}/head"),
            Kind::Gitlab => format!("refs/merge-requests/{number}/head"),
        }
    }

    pub fn fetch(&self, repo: &Repo, r: &Request) -> Result<String> {
        let head = self.head(r.number);
        repo.fetch(&[format!("+{head}:{head}"), target(&r.target)], false)?;
        repo.merge_base(&tracking(&r.target), &head)
    }
}

fn target(branch: &str) -> String {
    format!("+refs/heads/{branch}:{}", tracking(branch))
}

fn refresh(repo: &mut Repo, session: &mut Session, prompt: bool) -> Result<bool> {
    let end = &session.end;
    if end.starts_with("refs/pull/") || end.starts_with("refs/merge-requests/") {
        let mut refspecs = vec![format!("+{end}:{end}")];
        refspecs.extend(session.target.as_deref().map(target));
        repo.fetch(&refspecs, prompt)?;
    }
    session.follow(repo)
}

pub fn resume(repo: &mut Repo, store: &Store, id: &str, prompt: bool) -> Result<Session> {
    let mut session = store.load(id)?;
    if refresh(repo, &mut session, prompt)? {
        store.save(&session)?;
    }
    Ok(session)
}

#[expect(clippy::disallowed_methods, reason = "gh / glab")]
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
