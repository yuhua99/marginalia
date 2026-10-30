use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::doc::{carry, carry_file};
use crate::repo::{Repo, short};

/// The widest a session name may be, in columns.
pub const NAME_MAX: usize = 32;

/// Who the human in the TUI is; agents are anyone else.
const USER: &str = "user";

/// Who writes through `mrg comment`.
const AGENT: &str = "agent";

/// A review of `base..end`, saved as `<id>.json`. Files with fields this one has not,
/// like the `id` of older ones, do not load.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    /// The file name, not saved in the file.
    #[serde(skip)]
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
    pub messages: Vec<Message>,
    /// Where the thread is in the whole range, as of `Session::place`; None when its
    /// lines changed since, or its commit left the range.
    #[serde(skip)]
    pub place: Option<Place>,
    /// The lines as written, for lines that changed since; as of `Session::place`.
    #[serde(skip)]
    pub was: Option<String>,
}

/// What a thread is on. A `rev` is pinned: a commit sha, or the tree mrg wrote for
/// `staged` / `unstaged`; lines are numbered there.
#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Anchor {
    Lines {
        rev: String,
        file: String,
        start: u32,
        end: u32,
    },
    File {
        rev: String,
        file: String,
    },
    Commit {
        commit: String,
    },
    Session,
}

/// Where a thread is in a diff.
#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    /// Above the files: the session, or a commit of the diff.
    Whole,
    /// On the old (left) side, or the new.
    File { file: String, old: bool },
    Lines {
        file: String,
        start: u32,
        end: u32,
        old: bool,
    },
}

#[derive(Serialize, Deserialize)]
pub struct Message {
    pub from: String,
    pub body: String,
}

/// A comment of `mrg comment`: a reply, or a thread on a commit, a file, lines of it,
/// or, with none of those, the session.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewComment {
    reply_to: Option<String>,
    commit: Option<String>,
    file: Option<String>,
    /// `3` or `3-5`, of `file`.
    lines: Option<String>,
    /// `file` is numbered at the base: lines the range deletes.
    #[serde(default)]
    old: bool,
    body: String,
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

    /// Adds the agent's comments; returns the index of the thread each one went to.
    pub fn apply(&mut self, comments: Vec<NewComment>, repo: &mut Repo) -> Result<Vec<usize>> {
        let mut touched = Vec::new();
        for c in comments {
            let NewComment {
                reply_to,
                commit,
                file,
                lines,
                old,
                body,
            } = c;
            let anchor = match (reply_to, commit, file, lines, old) {
                (Some(id), None, None, None, false) => {
                    self.reply(&id, AGENT, body)?;
                    touched.push(
                        self.threads
                            .iter()
                            .position(|t| t.id == id)
                            .expect("replied"),
                    );
                    continue;
                }
                (None, Some(commit), None, None, false) => Anchor::Commit { commit },
                (None, None, Some(file), lines, old) => {
                    // A file the end has not is the base's.
                    let rev = if old || repo.read(&self.end, &file)?.is_none() {
                        self.base.clone()
                    } else {
                        self.end.clone()
                    };
                    match lines {
                        None => Anchor::File { rev, file },
                        Some(lines) => {
                            let (start, end) = parse_lines(&lines)?;
                            Anchor::Lines {
                                rev,
                                file,
                                start,
                                end,
                            }
                        }
                    }
                }
                (None, None, None, None, false) => Anchor::Session,
                _ => bail!(
                    "a comment has reply_to, commit, or file (with lines and old), \
                     or none of them for the session"
                ),
            };
            self.comment(anchor, AGENT, body, repo)?;
            touched.push(self.threads.len() - 1);
        }
        Ok(touched)
    }

    /// Deletes notes, threads only the agent wrote in, such as outdated explanations.
    pub fn delete_notes(&mut self, ids: &[String]) -> Result<()> {
        for id in ids {
            let Some(i) = self.threads.iter().position(|t| t.id == *id) else {
                bail!("no thread {id}");
            };
            if self.threads[i].messages.iter().any(|m| m.from == USER) {
                bail!("{id} has a message from the user");
            }
            self.threads.remove(i);
        }
        Ok(())
    }

    /// Adds a thread on `anchor`, its revs (any a commit or the range end can be) pinned
    /// and its commit made a sha. Call `place` after.
    pub fn comment(
        &mut self,
        mut anchor: Anchor,
        from: &str,
        body: String,
        repo: &mut Repo,
    ) -> Result<()> {
        match &mut anchor {
            Anchor::Lines {
                rev,
                file,
                start,
                end,
            } => {
                if *start == 0 || start > end {
                    bail!("invalid line range {start}-{end}");
                }
                snippet(repo, rev, file, *start, *end)?
                    .with_context(|| format!("{file}:{start}-{end} not found at {}", short(rev)))?;
                *rev = repo.pin(rev)?;
            }
            Anchor::File { rev, file } => {
                repo.read(rev, file)?
                    .with_context(|| format!("{file} not found at {}", short(rev)))?;
                *rev = repo.pin(rev)?;
            }
            Anchor::Commit { commit } => {
                *commit = repo.commit(commit)?;
                // One out of it would show nowhere.
                if !self.commits(repo)?.contains(commit) {
                    bail!("commit {} is not in the range", short(commit));
                }
            }
            Anchor::Session => {}
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
            messages: vec![Message {
                from: from.to_string(),
                body,
            }],
            place: None,
            was: None,
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
            tv["outdated"] = t.outdated().into();
            tv["location"] = self.location(t).into();
        }
        Ok(v)
    }

    /// Sets each thread's `place` in the whole range, and `was` of lines that changed.
    pub fn place(&mut self, repo: &mut Repo) -> Result<()> {
        let commits = self.commits(repo)?;
        let places = self.locate(repo, &self.base, &self.end, &commits)?;
        for (t, place) in self.threads.iter_mut().zip(places) {
            t.was = match (&place, &t.anchor) {
                (
                    None,
                    Anchor::Lines {
                        rev,
                        file,
                        start,
                        end,
                    },
                ) => snippet(repo, rev, file, *start, *end)?,
                _ => None,
            };
            t.place = place;
        }
        Ok(())
    }

    /// Where each thread is in the diff `left..right` of `commits`: lines and files
    /// where they are, unchanged, on the right side, else on the left; the session and
    /// those commits above the files. None for the rest.
    pub fn locate(
        &self,
        repo: &mut Repo,
        left: &str,
        right: &str,
        commits: &[String],
    ) -> Result<Vec<Option<Place>>> {
        let sides = [(repo.pin(right)?, false), (repo.pin(left)?, true)];
        self.threads
            .iter()
            .map(|t| locate(repo, &t.anchor, &sides, commits))
            .collect()
    }

    /// Where thread `t` is, e.g. `src/a.rs:3-5`: where it is in the whole range, else
    /// where it was written.
    pub fn location(&self, t: &Thread) -> String {
        match &t.place {
            Some(Place::Lines {
                file,
                start,
                end,
                old,
            }) => format!("{file}:{}", lines(*start, *end, *old)),
            Some(Place::File { file, old }) => format!("{file}{}", side(*old)),
            _ => self.anchor_location(&t.anchor),
        }
    }

    /// Where `anchor` points, e.g. `src/a.rs:3-5`; `(old)` on the base.
    pub fn anchor_location(&self, anchor: &Anchor) -> String {
        match anchor {
            Anchor::Lines {
                rev,
                file,
                start,
                end,
            } => format!("{file}:{}", lines(*start, *end, *rev == self.base)),
            Anchor::File { rev, file } => format!("{file}{}", side(*rev == self.base)),
            Anchor::Commit { commit } => format!("commit {}", short(commit)),
            Anchor::Session => "session".into(),
        }
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
        self.messages.last().is_some_and(|m| m.from == USER)
    }

    /// Not in the whole range any more, as of `Session::place`.
    pub fn outdated(&self) -> bool {
        self.place.is_none()
    }
}

impl Place {
    /// As shown under what it is on: lines as `L3-5`, a file as `file`.
    pub fn inline(&self) -> String {
        match self {
            Place::Lines {
                start, end, old, ..
            } => format!("L{}", lines(*start, *end, *old)),
            Place::File { old, .. } => format!("file{}", side(*old)),
            Place::Whole => String::new(),
        }
    }
}

/// Where `anchor` is on `sides`, (pinned rev, old) tried in turn; see `Session::locate`.
fn locate(
    repo: &mut Repo,
    anchor: &Anchor,
    sides: &[(String, bool)],
    commits: &[String],
) -> Result<Option<Place>> {
    let (rev, file) = match anchor {
        Anchor::Session => return Ok(Some(Place::Whole)),
        Anchor::Commit { commit } => return Ok(commits.contains(commit).then_some(Place::Whole)),
        Anchor::Lines { rev, file, .. } | Anchor::File { rev, file } => (rev, file),
    };
    // Gone with its rev, as a commit amended away and collected: nothing to carry.
    if repo.read(rev, file)?.is_none() {
        return Ok(None);
    }
    for (side, old) in sides {
        let files = if rev == side {
            &[][..]
        } else {
            match repo.changes(rev, side)? {
                Some(files) => files,
                // What it needs to be carried is gone too.
                None => return Ok(None),
            }
        };
        let place = match anchor {
            Anchor::Lines { start, end, .. } => {
                carry(files, file, *start, *end).map(|(file, start, end)| Place::Lines {
                    file,
                    start,
                    end,
                    old: *old,
                })
            }
            _ => carry_file(files, file).map(|file| Place::File { file, old: *old }),
        };
        if place.is_some() {
            return Ok(place);
        }
    }
    Ok(None)
}

/// `3` or `3-5` as (start, end).
fn parse_lines(lines: &str) -> Result<(u32, u32)> {
    let (start, end) = lines.split_once('-').unwrap_or((lines, lines));
    let n = |s: &str| {
        s.trim()
            .parse()
            .with_context(|| format!("lines must look like 3 or 3-5, got {lines:?}"))
    };
    Ok((n(start)?, n(end)?))
}

/// Lines `3` or `3-5`, plus `(old)` on the old side.
fn lines(start: u32, end: u32, old: bool) -> String {
    let lines = if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    };
    format!("{lines}{}", side(old))
}

fn side(old: bool) -> &'static str {
    if old { " (old)" } else { "" }
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
        let _ = std::fs::remove_dir_all(dir.with_extension("mrg"));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        let shas = versions.iter().map(|v| commit(&dir, v)).collect();
        (reopen(&dir), dir, shas)
    }

    /// The repo at `dir` read afresh, as by another run of mrg.
    fn reopen(dir: &Path) -> Repo {
        let mut repo = Repo::at(dir).unwrap();
        repo.use_objects(dir.with_extension("mrg").join("objects"))
            .unwrap();
        repo
    }

    // base a b c d; c1 adds X; c2 deletes c; c3 adds Y.
    const VERSIONS: [&str; 4] = [
        "a\nb\nc\nd\n",
        "a\nX\nb\nc\nd\n",
        "a\nX\nb\nd\n",
        "Y\na\nX\nb\nd\n",
    ];

    fn lines(rev: &str, start: u32, end: u32) -> Anchor {
        Anchor::Lines {
            rev: rev.into(),
            file: "f".into(),
            start,
            end,
        }
    }

    fn on(file: &str, start: u32, end: u32, old: bool) -> Option<Place> {
        Some(Place::Lines {
            file: file.into(),
            start,
            end,
            old,
        })
    }

    /// Comments `json`, an `mrg comment` input.
    fn apply(s: &mut Session, repo: &mut Repo, json: &str) -> Result<Vec<usize>> {
        s.apply(serde_json::from_str(json)?, repo)
    }

    #[test]
    fn threads_are_where_each_view_has_their_lines() {
        let (mut repo, _, c) = repo("views", &VERSIONS);
        let mut s = Session::new("1".into(), "r".into(), c[0].clone(), c[3].clone());
        apply(
            &mut s,
            &mut repo,
            &format!(
                r#"[
                    {{"file": "f", "lines": "3", "body": "X, c1's"}},
                    {{"file": "f", "lines": "3", "old": true, "body": "c, deleted by c2"}},
                    {{"file": "f", "lines": "1", "body": "Y, c3's"}},
                    {{"commit": "{}", "body": "c2"}},
                    {{"body": "all"}}
                ]"#,
                c[2]
            ),
        )
        .unwrap();
        let at = |repo: &mut Repo, left: &str, right: &str, commits: &[String]| {
            s.locate(repo, left, right, commits).unwrap()
        };
        let whole = Some(Place::Whole);
        assert_eq!(
            at(&mut repo, &c[0], &c[3], &c[1..]),
            [
                on("f", 3, 3, false),
                on("f", 3, 3, true),
                on("f", 1, 1, false),
                whole.clone(),
                whole.clone(),
            ]
        );
        // c1: X is its own; c is kept, so on its right side; Y is not yet.
        assert_eq!(
            at(&mut repo, &c[0], &c[1], &c[1..2]),
            [
                on("f", 2, 2, false),
                on("f", 4, 4, false),
                None,
                None,
                whole.clone(),
            ]
        );
        // c2 deletes c, on its left side.
        assert_eq!(
            at(&mut repo, &c[1], &c[2], &c[2..3]),
            [
                on("f", 2, 2, false),
                on("f", 4, 4, true),
                None,
                whole.clone(),
                whole.clone(),
            ]
        );
    }

    #[test]
    fn threads_follow_new_and_amended_commits() {
        let (mut repo, dir, c) = repo("amend", &VERSIONS[..2]);
        let mut s = Session::new("1".into(), "r".into(), c[0].clone(), "HEAD".into());
        // c and X of a X b c d, and the commit.
        apply(
            &mut s,
            &mut repo,
            r#"[
                {"file": "f", "lines": "4", "body": "c"},
                {"file": "f", "lines": "2", "body": "X"},
                {"commit": "HEAD", "body": "c1"}
            ]"#,
        )
        .unwrap();
        // Pinned to the commit HEAD was.
        assert!(matches!(&s.threads[0].anchor, Anchor::Lines { rev, .. } if *rev == c[1]));
        // The commit amended: a line above, X changed.
        std::fs::write(dir.join("f"), "Y\na\nZ\nb\nc\nd\n").unwrap();
        git(&dir, &["commit", "-qam", "c", "--amend"]);
        let mut repo = reopen(&dir);
        s.place(&mut repo).unwrap();
        assert_eq!(s.threads[0].place, on("f", 5, 5, false));
        // X is on neither side now.
        assert!(s.threads[1].outdated());
        assert_eq!(s.threads[1].was.as_deref(), Some("X"));
        // Amended away, so out of the range.
        assert!(s.threads[2].outdated());
        assert_eq!(s.location(&s.threads[0]), "f:5");
        assert_eq!(s.location(&s.threads[1]), "f:2");
    }

    #[test]
    fn threads_follow_renames() {
        let (mut repo, dir, c) = repo("rename", &VERSIONS[..2]);
        let mut s = Session::new("1".into(), "r".into(), c[0].clone(), "HEAD".into());
        apply(
            &mut s,
            &mut repo,
            r#"[{"file": "f", "lines": "2", "body": "X"}, {"file": "f", "body": "f"}]"#,
        )
        .unwrap();
        git(&dir, &["mv", "f", "g"]);
        git(&dir, &["commit", "-qm", "c"]);
        let mut repo = reopen(&dir);
        s.place(&mut repo).unwrap();
        assert_eq!(s.threads[0].place, on("g", 2, 2, false));
        assert_eq!(
            s.threads[1].place,
            Some(Place::File {
                file: "g".into(),
                old: false
            })
        );
    }

    #[test]
    fn worktree_threads_follow_edits() {
        let (mut repo, dir, c) = repo("worktree", &VERSIONS[..1]);
        let mut s = Session::new("1".into(), "r".into(), c[0].clone(), "unstaged".into());
        std::fs::write(dir.join("f"), "a\nb\nc\nZ\nd\n").unwrap();
        s.comment(lines("unstaged", 3, 3), "agent", "x".into(), &mut repo)
            .unwrap();
        s.comment(lines("unstaged", 4, 4), "agent", "x".into(), &mut repo)
            .unwrap();
        // Pinned to the tree written for the worktree.
        assert!(matches!(&s.threads[0].anchor, Anchor::Lines { rev, .. } if crate::repo::sha(rev)));
        // Another run: a line above, Z changed.
        std::fs::write(dir.join("f"), "Y\na\nb\nc\nW\nd\n").unwrap();
        let mut repo = reopen(&dir);
        s.place(&mut repo).unwrap();
        assert_eq!(s.threads[0].place, on("f", 4, 4, false));
        assert!(s.threads[1].outdated());
        assert_eq!(s.threads[1].was.as_deref(), Some("Z"));
    }

    #[test]
    fn threads_on_collected_objects_are_gone() {
        let (_, dir, c) = repo("collected", &VERSIONS[..1]);
        std::fs::write(dir.join("g"), "g\n").unwrap();
        git(&dir, &["add", "g"]);
        git(&dir, &["commit", "-qm", "c"]);
        std::fs::write(dir.join("f"), "a\nZ\n").unwrap();
        git(&dir, &["add", "f"]);
        let blob = git(&dir, &["rev-parse", ":f"]);
        let mut repo = reopen(&dir);
        let mut s = Session::new("1".into(), "r".into(), c[0].clone(), "staged".into());
        let g = Anchor::File {
            rev: "staged".into(),
            file: "g".into(),
        };
        s.comment(g, "agent", "x".into(), &mut repo).unwrap();
        // f staged anew, and its blob of the pinned tree collected.
        std::fs::write(dir.join("f"), "a\nW\n").unwrap();
        git(&dir, &["add", "f"]);
        std::fs::remove_file(dir.join(".git/objects").join(&blob[..2]).join(&blob[2..])).unwrap();
        let mut repo = reopen(&dir);
        s.place(&mut repo).unwrap();
        assert!(s.threads[0].outdated());
    }

    #[test]
    fn agent_comments() {
        let (mut repo, dir, _) = repo("input", &VERSIONS[..2]);
        std::fs::write(dir.join("gone"), "g\n").unwrap();
        git(&dir, &["add", "gone"]);
        git(&dir, &["commit", "-qm", "c"]);
        let base = git(&dir, &["rev-parse", "HEAD"]);
        git(&dir, &["rm", "-q", "gone"]);
        commit(&dir, VERSIONS[2]);
        let mut s = Session::new("1".into(), "r".into(), base.clone(), "HEAD".into());
        let touched = apply(
            &mut s,
            &mut repo,
            r#"[
                {"file": "f", "lines": "1-2", "body": "a X"},
                {"file": "f", "lines": "4", "old": true, "body": "c"},
                {"file": "gone", "lines": "1", "body": "a file the end has not"},
                {"body": "all"},
                {"reply_to": "t1", "body": "more"}
            ]"#,
        )
        .unwrap();
        assert_eq!(touched, [0, 1, 2, 3, 0]);
        let end = git(&dir, &["rev-parse", "HEAD"]);
        assert!(
            matches!(&s.threads[0].anchor, Anchor::Lines { rev, start: 1, end: 2, .. } if *rev == end)
        );
        assert!(matches!(&s.threads[1].anchor, Anchor::Lines { rev, .. } if *rev == base));
        assert!(matches!(&s.threads[2].anchor, Anchor::Lines { rev, .. } if *rev == base));
        assert!(matches!(s.threads[3].anchor, Anchor::Session));
        assert_eq!(s.threads[0].messages[1].from, "agent");
        for bad in [
            r#"[{"lines": "1", "body": "no file"}]"#,
            r#"[{"file": "f", "line": "1", "body": "a typo"}]"#,
            r#"[{"reply_to": "t1", "file": "f", "body": "both"}]"#,
            r#"[{"file": "f", "lines": "x", "body": "not a number"}]"#,
            r#"[{"file": "f", "lines": "3-2", "body": "backwards"}]"#,
            r#"[{"file": "f", "lines": "9", "body": "past the end"}]"#,
            &*format!(r#"[{{"commit": "{base}", "body": "out of the range"}}]"#),
        ] {
            assert!(apply(&mut s, &mut repo, bad).is_err(), "{bad}");
        }
        assert_eq!(s.threads.len(), 4);
    }

    #[test]
    fn older_sessions_do_not_load() {
        let old = r#"{"id": "ab12", "range": "r", "base": "b", "end": "e", "threads": []}"#;
        assert!(serde_json::from_str::<Session>(old).is_err());
        let new = r#"{"range": "r", "base": "b", "end": "e", "threads": []}"#;
        assert!(serde_json::from_str::<Session>(new).is_ok());
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
