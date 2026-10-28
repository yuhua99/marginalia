use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::repo::{self, Repo, short};

/// The widest a session name may be, in columns.
pub const NAME_MAX: usize = 32;

#[derive(Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The range as given by the user, for display.
    pub range: String,
    pub base: String,
    pub end: String,
    pub threads: Vec<Thread>,
}

#[derive(Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub anchor: Anchor,
    /// The anchored lines when the thread was created, for `lines` anchors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub messages: Vec<Message>,
    /// The anchored lines no longer match `text`, as of `Session::check_outdated`.
    #[serde(skip)]
    pub outdated: bool,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Anchor {
    Lines {
        /// Defaults to the range end when omitted in input.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
        file: String,
        start: u32,
        end: u32,
        /// The commit the lines belong to; None when the range left them alone.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<Owner>,
    },
    File {
        file: String,
    },
    Commit {
        commit: String,
    },
    Session,
}

/// Where lines sit in the diff of the commit they belong to.
#[derive(Serialize, Deserialize, Clone)]
pub struct Owner {
    /// A commit of the range, or `staged` / `unstaged` for lines not committed.
    pub commit: String,
    /// Lines the commit deleted, numbered in its parent; else numbered in the commit.
    pub deleted: bool,
    pub file: String,
    /// The last anchored line there.
    pub line: u32,
}

#[derive(Serialize, Deserialize)]
pub struct Message {
    pub from: String,
    pub body: String,
}

/// The `mrg comment` input format.
#[derive(Deserialize)]
pub struct Input {
    pub from: String,
    pub comments: Vec<NewComment>,
}

#[derive(Deserialize)]
pub struct NewComment {
    #[serde(default)]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub anchor: Option<Anchor>,
    pub body: String,
}

impl Session {
    pub fn new(id: String, range: String, base: String, end: String) -> Session {
        Session {
            id,
            name: None,
            range,
            base,
            end,
            threads: Vec::new(),
        }
    }

    /// Sets the name, trimmed; a blank one clears it.
    pub fn set_name(&mut self, name: &str) -> Result<()> {
        let name = name.trim();
        if name.chars().any(char::is_control) {
            bail!("a session name is one line");
        }
        if name.width() > NAME_MAX {
            bail!("a session name is at most {NAME_MAX} columns wide");
        }
        self.name = (!name.is_empty()).then(|| name.to_string());
        Ok(())
    }

    /// Adds agent comments; returns the index of the thread each one went to.
    pub fn apply(&mut self, input: Input, repo: &mut Repo) -> Result<Vec<usize>> {
        if input.from == "user" {
            bail!("\"from\" names the agent; \"user\" is the human in the TUI");
        }
        let mut touched = Vec::new();
        for c in input.comments {
            match (c.reply_to, c.anchor) {
                (Some(id), None) => {
                    self.reply(&id, &input.from, c.body)?;
                    touched.push(
                        self.threads
                            .iter()
                            .position(|t| t.id == id)
                            .expect("replied"),
                    );
                }
                (None, Some(mut anchor)) => {
                    // Owners are worked out here, not taken from the agent.
                    if let Anchor::Lines { owner, .. } = &mut anchor {
                        *owner = None;
                    }
                    self.comment(anchor, &input.from, c.body, repo)?;
                    touched.push(self.threads.len() - 1);
                }
                _ => bail!("each comment needs exactly one of reply_to or anchor"),
            }
        }
        Ok(touched)
    }

    /// Deletes notes, threads only the agent wrote in, such as outdated explanations.
    pub fn delete_notes(&mut self, ids: &[String]) -> Result<()> {
        for id in ids {
            let Some(i) = self.threads.iter().position(|t| t.id == *id) else {
                bail!("no thread {id}");
            };
            if self.threads[i].messages.iter().any(|m| m.from == "user") {
                bail!("{id} has a message from the user");
            }
            self.threads.remove(i);
        }
        Ok(())
    }

    pub fn comment(
        &mut self,
        mut anchor: Anchor,
        from: &str,
        body: String,
        repo: &mut Repo,
    ) -> Result<()> {
        let mut text = None;
        if let Anchor::Lines {
            rev,
            file,
            start,
            end,
            owner,
        } = &mut anchor
        {
            if *start == 0 || start > end {
                bail!("invalid line range {start}-{end}");
            }
            let rev = rev.get_or_insert_with(|| self.end.clone());
            // Commits become shas; only the range end and the worktree states stay symbolic.
            if *rev != self.end && !repo::worktree(rev) {
                *rev = repo.commit(rev)?;
            }
            text = Some(
                snippet(repo, rev, file, *start, *end)?
                    .with_context(|| format!("{file}:{start}-{end} not found at {rev}"))?,
            );
            if owner.is_none() {
                let commits = self.commits(repo)?;
                *owner = self.owner(repo, &commits, rev, file, *end)?;
            }
        }
        if let Anchor::Commit { commit } = &mut anchor {
            *commit = repo.commit(commit)?;
        }
        // After the highest id, since deleted threads leave gaps.
        let n = self
            .threads
            .iter()
            .filter_map(|t| t.id[1..].parse::<usize>().ok())
            .max()
            .unwrap_or(0);
        self.threads.push(Thread {
            id: format!("t{}", n + 1),
            anchor,
            text,
            messages: vec![Message {
                from: from.to_string(),
                body,
            }],
            outdated: false,
        });
        Ok(())
    }

    /// Shas of the range commits, oldest first.
    fn commits(&self, repo: &mut Repo) -> Result<Vec<String>> {
        Ok(repo
            .commits(&self.base, &self.end)?
            .into_iter()
            .map(|c| c.sha)
            .collect())
    }

    /// The owner of line `line` of `file` at `rev`: the commit that deletes it when a
    /// later one of `commits` does, else the one that last changed it; None when that
    /// is outside the range.
    fn owner(
        &self,
        repo: &mut Repo,
        commits: &[String],
        rev: &str,
        file: &str,
        line: u32,
    ) -> Result<Option<Owner>> {
        let in_range = commits.iter().any(|c| c == rev);
        // Only the base and the range commits but the tip can have lines a later one deletes.
        // Reverse blame walks first parents, so a base merged in later is left out.
        if let Some(tip) = commits.last()
            && rev != tip
            && (in_range || rev == self.base && repo.parent(&commits[0])?.as_deref() == Some(rev))
        {
            let b = repo.blame_reverse(rev, tip, file, line)?;
            if b.commit != *tip {
                // Deleted by the commit after the last one having it, the base or one in range.
                let i = commits
                    .iter()
                    .position(|c| *c == b.commit)
                    .map_or(0, |i| i + 1);
                return Ok(Some(Owner {
                    commit: commits[i].clone(),
                    deleted: true,
                    file: b.path,
                    line: b.line,
                }));
            }
        }
        // Lines the base or an older rev has, kept, are not the range's. (Blaming a rev
        // the base excludes would blame HEAD instead.)
        if !in_range && rev != self.end && !repo::worktree(rev) {
            return Ok(None);
        }
        // Stop at the base: older lines are not the range's, and need not be traced.
        let since = (self.base != repo.empty_tree()?).then_some(self.base.as_str());
        let b = repo.blame(rev, since, file, line)?;
        let commit = if b.uncommitted() {
            rev.to_string()
        } else if commits.contains(&b.commit) {
            b.commit
        } else {
            return Ok(None);
        };
        Ok(Some(Owner {
            commit,
            deleted: false,
            file: b.path,
            line: b.line,
        }))
    }

    pub fn reply(&mut self, id: &str, from: &str, body: String) -> Result<()> {
        let Some(t) = self.threads.iter_mut().find(|t| t.id == id) else {
            bail!("no thread {id}");
        };
        t.messages.push(Message {
            from: from.to_string(),
            body,
        });
        Ok(())
    }

    /// Rewrites the last message of thread `idx`.
    pub fn edit(&mut self, idx: usize, body: String) {
        self.threads[idx]
            .messages
            .last_mut()
            .expect("a message")
            .body = body;
    }

    /// Removes the last message of thread `idx`, or the thread when it is the only one.
    pub fn delete(&mut self, idx: usize) {
        let t = &mut self.threads[idx];
        if t.messages.len() > 1 {
            t.messages.pop();
        } else {
            self.threads.remove(idx);
        }
    }

    pub fn waiting_on_agent_count(&self) -> usize {
        self.threads.iter().filter(|t| t.waiting_on_agent()).count()
    }

    /// The session as JSON, with derived fields added to each thread.
    /// `waiting_on` is `agent` or null; `outdated` is as of `check_outdated`.
    pub fn view(&self) -> Result<Value> {
        let mut v = serde_json::to_value(self)?;
        for (t, tv) in self
            .threads
            .iter()
            .zip(v["threads"].as_array_mut().expect("threads array"))
        {
            tv["waiting_on"] = t.waiting_on_agent().then_some("agent").into();
            tv["outdated"] = t.outdated.into();
        }
        Ok(v)
    }

    /// Sets each thread's `outdated`: its anchored lines no longer match the stored `text`.
    pub fn check_outdated(&mut self, repo: &mut Repo) -> Result<()> {
        for t in &mut self.threads {
            t.outdated = match &t.anchor {
                Anchor::Lines {
                    rev: Some(rev),
                    file,
                    start,
                    end,
                    ..
                } if t.text.is_some() => snippet(repo, rev, file, *start, *end)? != t.text,
                _ => false,
            };
        }
        Ok(())
    }

    /// Where a thread is anchored, e.g. `src/a.rs:3-5`, plus `@rev` when not the range end.
    pub fn location(&self, anchor: &Anchor) -> String {
        match anchor {
            Anchor::Lines {
                file,
                rev,
                start,
                end,
                ..
            } => format!("{file}:{}", self.lines(rev, *start, *end)),
            Anchor::File { file } => file.clone(),
            Anchor::Commit { commit } => format!("commit {}", short(commit)),
            Anchor::Session => "session".into(),
        }
    }

    /// `location` for a thread shown under what it anchors to: lines as `L3-5`, a file as `file`.
    pub fn inline_location(&self, anchor: &Anchor) -> String {
        match anchor {
            Anchor::Lines {
                rev, start, end, ..
            } => format!("L{}", self.lines(rev, *start, *end)),
            Anchor::File { .. } => "file".into(),
            _ => self.location(anchor),
        }
    }

    /// Lines `3` or `3-5`, plus `@rev` when not the range end.
    fn lines(&self, rev: &Option<String>, start: u32, end: u32) -> String {
        let mut s = if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        };
        if let Some(rev) = rev
            && *rev != self.end
        {
            s.push_str(&format!(" @{}", short(rev)));
        }
        s
    }

    /// Threads waiting on the agent, in `order`, as markdown; None if there are none.
    pub fn markdown(&self, order: &[usize]) -> Option<String> {
        let mut out = format!("# mrg review {} / {}\n", self.id, self.range);
        let mut any = false;
        for t in order.iter().map(|&i| &self.threads[i]) {
            if !t.waiting_on_agent() {
                continue;
            }
            any = true;
            out.push_str(&format!("\n## {} / {}\n", t.id, self.location(&t.anchor)));
            for m in &t.messages {
                out.push_str(&format!(
                    "- **{}**: {}\n",
                    m.from,
                    m.body.replace('\n', "\n  ")
                ));
            }
        }
        any.then_some(out)
    }
}

impl Thread {
    /// The last speaker being `user` means the agent should answer.
    pub fn waiting_on_agent(&self) -> bool {
        self.messages.last().is_some_and(|m| m.from == "user")
    }
}

/// Lines `start..=end` of `file` at `rev`; None if the file or lines are absent.
fn snippet(repo: &mut Repo, rev: &str, file: &str, start: u32, end: u32) -> Result<Option<String>> {
    let Some(content) = repo.read(rev, file)? else {
        return Ok(None);
    };
    let lines: Vec<&str> = content.lines().collect();
    Ok(lines
        .get(start as usize - 1..end as usize)
        .map(|l| l.join("\n")))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use super::*;

    /// A temporary repo committing each of `versions` of file `f` in turn; with its
    /// directory and the shas, oldest first.
    fn repo(name: &str, versions: &[&str]) -> (Repo, PathBuf, Vec<String>) {
        let dir = std::env::temp_dir().join(format!("mrg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(["-c", "commit.gpgsign=false"])
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        git(&["init", "-q"]);
        let mut shas = Vec::new();
        for v in versions {
            std::fs::write(dir.join("f"), v).unwrap();
            git(&["add", "f"]);
            git(&["commit", "-qm", "c"]);
            shas.push(git(&["rev-parse", "HEAD"]));
        }
        (Repo::at(&dir).unwrap(), dir, shas)
    }

    /// The owner given to a comment on line `line` of `f` at `rev`, as (commit, deleted, line).
    fn owner(
        s: &mut Session,
        repo: &mut Repo,
        rev: &str,
        line: u32,
    ) -> Option<(String, bool, u32)> {
        let anchor = Anchor::Lines {
            rev: Some(rev.into()),
            file: "f".into(),
            start: line,
            end: line,
            owner: None,
        };
        s.comment(anchor, "agent", "x".into(), repo).unwrap();
        match &s.threads.last().unwrap().anchor {
            Anchor::Lines { owner, .. } => owner
                .as_ref()
                .map(|o| (o.commit.clone(), o.deleted, o.line)),
            _ => unreachable!(),
        }
    }

    // base a b c d; c1 adds X; c2 deletes c; c3 adds Y.
    const VERSIONS: [&str; 4] = [
        "a\nb\nc\nd\n",
        "a\nX\nb\nc\nd\n",
        "a\nX\nb\nd\n",
        "Y\na\nX\nb\nd\n",
    ];

    #[test]
    fn owners_of_committed_lines() {
        let (mut repo, _, c) = repo("owners", &VERSIONS);
        let mut s = Session::new("1".into(), "r".into(), c[0].clone(), c[3].clone());
        let end = c[3].clone();
        // Lines at the end belong to the commit that wrote them, numbered there.
        assert_eq!(
            owner(&mut s, &mut repo, &end, 3),
            Some((c[1].clone(), false, 2))
        );
        assert_eq!(
            owner(&mut s, &mut repo, &end, 1),
            Some((c[3].clone(), false, 1))
        );
        // Untouched by the range.
        assert_eq!(owner(&mut s, &mut repo, &end, 2), None);
        // Deleted base lines belong to their deleter, numbered in its parent.
        assert_eq!(
            owner(&mut s, &mut repo, &c[0], 3),
            Some((c[2].clone(), true, 4))
        );
        assert_eq!(owner(&mut s, &mut repo, &c[0], 1), None);
        // Lines of a rev before the base are not the range's to delete.
        let mut later = Session::new("2".into(), "r".into(), c[1].clone(), c[3].clone());
        assert_eq!(owner(&mut later, &mut repo, &c[0], 3), None);
        // A line kept from a middle commit belongs to whoever wrote it.
        assert_eq!(
            owner(&mut s, &mut repo, &c[1], 2),
            Some((c[1].clone(), false, 2))
        );
    }

    #[test]
    fn owners_of_uncommitted_lines() {
        let (mut repo, dir, c) = repo("uncommitted", &VERSIONS);
        // The worktree is read once per process, so it changes before the first comment.
        std::fs::write(dir.join("f"), "Y\na\nZ\nX\nb\nd\n").unwrap();
        std::fs::write(dir.join("u"), "u\n").unwrap();
        let mut s = Session::new("1".into(), "r".into(), c[0].clone(), "unstaged".into());
        assert_eq!(
            owner(&mut s, &mut repo, "unstaged", 3),
            Some(("unstaged".into(), false, 3))
        );
        assert_eq!(
            owner(&mut s, &mut repo, "unstaged", 4),
            Some((c[1].clone(), false, 2))
        );
        // A file new to the worktree.
        let anchor = Anchor::Lines {
            rev: None,
            file: "u".into(),
            start: 1,
            end: 1,
            owner: None,
        };
        s.comment(anchor, "agent", "x".into(), &mut repo).unwrap();
        assert!(matches!(
            &s.threads.last().unwrap().anchor,
            Anchor::Lines { owner: Some(o), .. } if o.commit == "unstaged"
        ));
    }

    #[test]
    fn names() {
        let mut s = Session::new("1".into(), "r".into(), "b".into(), "e".into());
        s.set_name("  a b  ").unwrap();
        assert_eq!(s.name.as_deref(), Some("a b"));
        s.set_name("   ").unwrap();
        assert_eq!(s.name, None);
        assert!(s.set_name("a\nb").is_err());
        s.set_name(&"中".repeat(16)).unwrap();
        assert!(s.set_name(&"中".repeat(17)).is_err());
    }
}
