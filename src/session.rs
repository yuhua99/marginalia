use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::doc::carry;
use crate::repo::{self, FileDiff, Repo, short};

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
    /// The anchored lines changed since, as of `Session::place`.
    #[serde(skip)]
    pub outdated: bool,
    /// Where the anchored lines are in the whole range, as of `Session::place`; None
    /// for other anchors, or lines that changed since.
    #[serde(skip)]
    pub place: Option<Place>,
}

/// Lines in the whole range: on the end, or on the base for deleted ones.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub file: String,
    pub start: u32,
    pub end: u32,
    pub base: bool,
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
            // Commits become shas; the range end and the worktree states stay symbolic
            // until the owner is found.
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
            // Then pinned too, so the lines can be carried as the end moves.
            if !repo::sha(rev) {
                *rev = repo.pin(rev)?;
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
        let mut thread = Thread {
            id: format!("t{}", n + 1),
            anchor,
            text,
            messages: vec![Message {
                from: from.to_string(),
                body,
            }],
            outdated: false,
            place: None,
        };
        let pinned = repo.pin(&self.end)?;
        (thread.place, thread.outdated) =
            self.locate(repo, &thread, &pinned, &mut HashMap::new())?;
        self.threads.push(thread);
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
    /// `waiting_on` is `agent` or null; `outdated` and `location` (where it is now)
    /// are as of `place`.
    pub fn view(&self) -> Result<Value> {
        let mut v = serde_json::to_value(self)?;
        for (t, tv) in self
            .threads
            .iter()
            .zip(v["threads"].as_array_mut().expect("threads array"))
        {
            tv["waiting_on"] = t.waiting_on_agent().then_some("agent").into();
            tv["outdated"] = t.outdated.into();
            tv["location"] = self.location(t).into();
        }
        Ok(v)
    }

    /// Sets each thread's `place` and `outdated`: lines written at another rev are
    /// carried to the range end along the diff from there, outdated when it changes
    /// them. Symbolic revs (`HEAD`, `staged`, `unstaged`, of threads from before
    /// revs were pinned) stand for the end as is, outdated when `text` no longer matches.
    pub fn place(&mut self, repo: &mut Repo) -> Result<()> {
        let pinned = repo.pin(&self.end)?;
        let mut diffs = HashMap::new();
        for i in 0..self.threads.len() {
            let (place, outdated) = self.locate(repo, &self.threads[i], &pinned, &mut diffs)?;
            let t = &mut self.threads[i];
            (t.place, t.outdated) = (place, outdated);
        }
        Ok(())
    }

    /// `place` and `outdated` of thread `t`, the end pinned as `pinned`; `diffs` keeps
    /// the diff from each rev to the end.
    fn locate(
        &self,
        repo: &mut Repo,
        t: &Thread,
        pinned: &str,
        diffs: &mut HashMap<String, Vec<FileDiff>>,
    ) -> Result<(Option<Place>, bool)> {
        let Anchor::Lines {
            rev: Some(rev),
            file,
            start,
            end,
            owner,
        } = &t.anchor
        else {
            return Ok((None, false));
        };
        let here = Place {
            file: file.clone(),
            start: *start,
            end: *end,
            base: false,
        };
        if *rev == self.base {
            return Ok((Some(Place { base: true, ..here }), false));
        }
        if !repo::sha(rev) {
            let changed = t.text.is_some() && snippet(repo, rev, file, *start, *end)? != t.text;
            return Ok(((*rev == self.end).then_some(here), changed));
        }
        if rev == pinned {
            return Ok((Some(here), false));
        }
        // Lines a commit of the range deletes are on no side of the whole range, as written.
        if owner.as_ref().is_some_and(|o| o.deleted) {
            return Ok((None, false));
        }
        if repo.read(rev, file)?.is_none() {
            // Gone with rewritten history: nothing to carry from.
            return Ok((None, true));
        }
        if !diffs.contains_key(rev) {
            let diff = repo.diff(rev, &self.end, 0)?;
            diffs.insert(rev.clone(), diff);
        }
        Ok(match carry(&diffs[rev], file, *start, *end) {
            Some((file, start, end)) => (
                Some(Place {
                    file,
                    start,
                    end,
                    ..here
                }),
                false,
            ),
            None => (None, true),
        })
    }

    /// Where thread `t` is, e.g. `src/a.rs:3-5`: where its lines are now, plus `@base`
    /// on the base; else where it was written, as `anchor_location`.
    pub fn location(&self, t: &Thread) -> String {
        match &t.place {
            Some(p) => format!("{}:{}", p.file, self.place_lines(p)),
            None => self.anchor_location(&t.anchor),
        }
    }

    /// Where `anchor` points, e.g. `src/a.rs:3-5`, plus `@rev` when not the range end.
    pub fn anchor_location(&self, anchor: &Anchor) -> String {
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
    pub fn inline_location(&self, t: &Thread) -> String {
        match (&t.place, &t.anchor) {
            (Some(p), _) => format!("L{}", self.place_lines(p)),
            (
                None,
                Anchor::Lines {
                    rev, start, end, ..
                },
            ) => {
                format!("L{}", self.lines(rev, *start, *end))
            }
            (None, Anchor::File { .. }) => "file".into(),
            _ => self.location(t),
        }
    }

    fn place_lines(&self, p: &Place) -> String {
        let rev = p.base.then(|| self.base.clone());
        self.lines(&rev, p.start, p.end)
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
            out.push_str(&format!("\n## {} / {}\n", t.id, self.location(t)));
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
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// Commits `content` as file `f`; its sha.
    fn commit(dir: &Path, content: &str) -> String {
        std::fs::write(dir.join("f"), content).unwrap();
        git(dir, &["add", "f"]);
        git(dir, &["commit", "-qm", "c"]);
        git(dir, &["rev-parse", "HEAD"])
    }

    /// A temporary repo committing each of `versions` of file `f` in turn; with its
    /// directory and the shas, oldest first.
    fn repo(name: &str, versions: &[&str]) -> (Repo, PathBuf, Vec<String>) {
        let dir = std::env::temp_dir().join(format!("mrg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        let shas = versions.iter().map(|v| commit(&dir, v)).collect();
        let mut repo = Repo::at(&dir).unwrap();
        let objects = dir.with_extension("mrg").join("objects");
        let _ = std::fs::remove_dir_all(dir.with_extension("mrg"));
        repo.use_objects(objects).unwrap();
        (repo, dir, shas)
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

    fn lines(rev: Option<&str>, line: u32) -> Anchor {
        Anchor::Lines {
            rev: rev.map(Into::into),
            file: "f".into(),
            start: line,
            end: line,
            owner: None,
        }
    }

    /// Where thread `t` is placed, as (start, base), and whether it is outdated.
    fn at(t: &Thread) -> (Option<(u32, bool)>, bool) {
        (t.place.as_ref().map(|p| (p.start, p.base)), t.outdated)
    }

    #[test]
    fn threads_follow_the_end() {
        let (mut repo, dir, c) = repo("follow", &VERSIONS[..2]);
        let mut s = Session::new("1".into(), "r".into(), c[0].clone(), "HEAD".into());
        // b and c of a X b c d.
        s.comment(lines(None, 3), "agent", "x".into(), &mut repo)
            .unwrap();
        s.comment(lines(None, 4), "agent", "x".into(), &mut repo)
            .unwrap();
        // Written at the commit HEAD was, not at HEAD.
        assert!(matches!(&s.threads[0].anchor, Anchor::Lines { rev: Some(r), .. } if *r == c[1]));
        // a of the base.
        s.comment(lines(Some(&c[0]), 1), "agent", "x".into(), &mut repo)
            .unwrap();
        // From before revs were pinned: b, as HEAD.
        s.comment(lines(None, 3), "agent", "x".into(), &mut repo)
            .unwrap();
        if let Anchor::Lines { rev, .. } = &mut s.threads[3].anchor {
            *rev = Some("HEAD".into());
        }
        // Later commits delete c, then add a line above; read by another run, as the
        // range is listed once per run.
        commit(&dir, VERSIONS[2]);
        commit(&dir, VERSIONS[3]);
        let mut repo = Repo::at(&dir).unwrap();
        repo.use_objects(dir.with_extension("mrg").join("objects"))
            .unwrap();
        // c, deleted, as its deleter's view writes it: at its parent.
        s.comment(lines(Some(&c[1]), 4), "agent", "x".into(), &mut repo)
            .unwrap();
        s.place(&mut repo).unwrap();
        let got: Vec<_> = s.threads.iter().map(at).collect();
        assert_eq!(
            got,
            [
                (Some((4, false)), false),
                (None, true),
                (Some((1, true)), false),
                // Read at HEAD as is: X now, not b.
                (Some((3, false)), true),
                (None, false),
            ]
        );
        assert_eq!(s.location(&s.threads[0]), "f:4");
        assert_eq!(s.location(&s.threads[1]), format!("f:4 @{}", short(&c[1])));
        assert_eq!(s.location(&s.threads[2]), format!("f:1 @{}", short(&c[0])));
    }

    #[test]
    fn worktree_threads_follow_edits() {
        let (mut repo, dir, c) = repo("worktree", &VERSIONS[..1]);
        let mut s = Session::new("1".into(), "r".into(), c[0].clone(), "unstaged".into());
        std::fs::write(
            dir.join("f"),
            "a
b
c
Z
d
",
        )
        .unwrap();
        s.comment(lines(None, 3), "agent", "x".into(), &mut repo)
            .unwrap();
        s.comment(lines(None, 4), "agent", "x".into(), &mut repo)
            .unwrap();
        // Pinned to the tree written for the worktree.
        assert!(matches!(&s.threads[0].anchor, Anchor::Lines { rev: Some(r), .. } if repo::sha(r)));
        // Another run: a line above, Z changed.
        std::fs::write(
            dir.join("f"),
            "Y
a
b
c
W
d
",
        )
        .unwrap();
        repo.use_objects(dir.with_extension("mrg").join("objects"))
            .unwrap();
        s.place(&mut repo).unwrap();
        assert_eq!(at(&s.threads[0]), (Some((4, false)), false));
        assert_eq!(at(&s.threads[1]), (None, true));
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
