use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::repo::{Commit, FileDiff, short, worktree};

/// What a session needs of the code it reviews: a git repo, or a snapshot of one.
pub trait Source {
    /// Content of `path` at `rev`; None if absent.
    fn read(&mut self, rev: &str, path: &str) -> Result<Option<String>>;
    /// An id for `rev` that does not move.
    fn pin(&mut self, rev: &str) -> Result<String>;
    /// Keeps `tree`, pinned of `staged` / `unstaged`, while session `id` lasts.
    fn keep(&self, id: &str, tree: &str) -> Result<()>;
    /// Full sha of commit `rev`.
    fn commit(&self, rev: &str) -> Result<String>;
    /// Where the diff of `commit` starts: its first parent, or the empty tree.
    fn left(&self, commit: &str) -> Result<String>;
    /// Commits in `base..end`, oldest first.
    fn commits(&mut self, base: &str, end: &str) -> Result<Vec<Commit>>;
    /// The diff without context of pinned revs `from` to `to`; None when what it
    /// needs is gone.
    fn changes(&mut self, from: &str, to: &str) -> Result<Option<&[FileDiff]>>;
}

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
}

/// What a thread is on. A `rev` is pinned: a commit sha, or the tree mrg wrote for
/// `staged` / `unstaged`; lines are numbered there. Lines and files of a commit's
/// diff have that `commit`, `rev` being it or where its diff starts; those of
/// `staged` / `unstaged` have none. `old`: `rev` is the left side of the diff
/// written on, for lines or a file it deletes.
#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Anchor {
    Lines {
        rev: String,
        file: String,
        start: u32,
        end: u32,
        #[serde(default)]
        old: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
    },
    File {
        rev: String,
        file: String,
        #[serde(default)]
        old: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
    },
    Commit {
        commit: String,
    },
    Session,
}

/// Where a thread is in a diff.
#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    /// Above the files: the session, or a commit of the diff with what of it changed
    /// further on.
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

/// A comment of `mrg comment`: a reply, or a thread on a commit, a file of a commit (or
/// of `staged` / `unstaged`), lines of it, or, with none of those, the session.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewComment {
    reply_to: Option<String>,
    commit: Option<String>,
    file: Option<String>,
    /// `3` or `3-5`, of `file` in `commit`.
    lines: Option<String>,
    /// `file` is numbered where the diff of `commit` starts: lines it deletes.
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
        self.name = (!name.is_empty()).then(|| name.to_string());
        Ok(())
    }

    /// Adds the agent's comments; returns the index of the thread each one went to.
    pub fn apply(
        &mut self,
        comments: Vec<NewComment>,
        repo: &mut dyn Source,
    ) -> Result<Vec<usize>> {
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
                (None, Some(commit), Some(file), lines, old) => {
                    // The sides of the diff of `commit`.
                    let (left, right, commit) = match commit.as_str() {
                        "staged" if !worktree(&self.end) || self.base == "staged" => {
                            bail!("staged is not in the range")
                        }
                        "unstaged" if self.end != "unstaged" => {
                            bail!("unstaged is not in the range")
                        }
                        "staged" => ("HEAD".to_string(), commit, None),
                        "unstaged" => ("staged".to_string(), commit, None),
                        _ => {
                            let commit = repo.commit(&commit)?;
                            (repo.left(&commit)?, commit.clone(), Some(commit))
                        }
                    };
                    // A file the right side has not is the left's.
                    let old = old || repo.read(&right, &file)?.is_none();
                    let rev = if old { left } else { right };
                    match lines {
                        None => Anchor::File {
                            rev,
                            file,
                            old,
                            commit,
                        },
                        Some(lines) => {
                            let (start, end) = parse_lines(&lines)?;
                            Anchor::Lines {
                                rev,
                                file,
                                start,
                                end,
                                old,
                                commit,
                            }
                        }
                    }
                }
                (None, None, None, None, false) => Anchor::Session,
                _ => bail!(
                    "a comment has reply_to, commit (with file, lines and old), \
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
    /// and its commit made a sha.
    pub fn comment(
        &mut self,
        mut anchor: Anchor,
        from: &str,
        body: String,
        repo: &mut dyn Source,
    ) -> Result<()> {
        match &mut anchor {
            Anchor::Lines {
                rev,
                file,
                start,
                end,
                ..
            } => {
                if *start == 0 || start > end {
                    bail!("invalid line range {start}-{end}");
                }
                snippet(repo, rev, file, *start, *end)?
                    .with_context(|| format!("{file}:{start}-{end} not found at {}", short(rev)))?;
                *rev = pin(repo, &self.id, rev)?;
            }
            Anchor::File { rev, file, .. } => {
                repo.read(rev, file)?
                    .with_context(|| format!("{file} not found at {}", short(rev)))?;
                *rev = pin(repo, &self.id, rev)?;
            }
            Anchor::Commit { commit } => *commit = repo.commit(commit)?,
            Anchor::Session => {}
        }
        // One of a commit out of the range would show nowhere.
        if let Some(commit) = anchor.commit()
            && !self.commits(repo)?.contains(commit)
        {
            bail!("commit {} is not in the range", short(commit));
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
        });
        Ok(())
    }

    /// Shas of the range commits, oldest first.
    fn commits(&self, repo: &mut dyn Source) -> Result<Vec<String>> {
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
    /// `waiting_on` is `agent` or null; `location` is where its anchor points.
    pub fn view(&self) -> Result<Value> {
        let mut v = serde_json::to_value(self)?;
        for (t, tv) in self
            .threads
            .iter()
            .zip(v["threads"].as_array_mut().expect("threads array"))
        {
            tv["waiting_on"] = t.waiting_on_agent().then_some("agent").into();
            tv["location"] = self.location(&t.anchor).into();
        }
        Ok(v)
    }

    /// Where each thread is in the diff `left..right` of `commits`: lines and files
    /// where they are, unchanged, on the right side, else on the left; the session and
    /// those commits above the files. Lines and files of those commits that changed
    /// further on go with their commit; of other commits, nowhere. None for the rest.
    pub fn locate(
        &self,
        repo: &mut dyn Source,
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

    /// Where `anchor` points, e.g. `src/a.rs:3-5 @ 1a2b3c4`; `(old)` on the left side.
    pub fn location(&self, anchor: &Anchor) -> String {
        let at = |commit: &Option<String>| {
            commit
                .as_ref()
                .map_or(String::new(), |c| format!(" @ {}", short(c)))
        };
        match anchor {
            Anchor::Lines {
                file,
                start,
                end,
                old,
                commit,
                ..
            } => format!("{file}:{}{}", lines(*start, *end, *old), at(commit)),
            Anchor::File {
                file, old, commit, ..
            } => format!("{file}{}{}", side(*old), at(commit)),
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
        self.messages.last().is_some_and(|m| m.from == USER)
    }
}

impl Anchor {
    /// The commit it goes with; None for the session and `staged` / `unstaged`.
    pub fn commit(&self) -> Option<&String> {
        match self {
            Anchor::Lines { commit, .. } | Anchor::File { commit, .. } => commit.as_ref(),
            Anchor::Commit { commit } => Some(commit),
            Anchor::Session => None,
        }
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
    repo: &mut dyn Source,
    anchor: &Anchor,
    sides: &[(String, bool)],
    commits: &[String],
) -> Result<Option<Place>> {
    let (rev, file) = match anchor {
        Anchor::Session => return Ok(Some(Place::Whole)),
        Anchor::Commit { commit } => return Ok(commits.contains(commit).then_some(Place::Whole)),
        Anchor::Lines { rev, file, .. } | Anchor::File { rev, file, .. } => (rev, file),
    };
    let commit = anchor.commit();
    if commit.is_some_and(|c| !commits.contains(c)) {
        return Ok(None);
    }
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
    // Changed further on: with its commit.
    Ok(commit.map(|_| Place::Whole))
}

/// `anchor`, written on the diff of `views` as one, put on the view it is about. Views
/// are oldest first, each its left and right revs and its commit (None for `staged` /
/// `unstaged`). One view takes all; else lines and files go to the newest view that
/// changes them, as they are on its right side, those on the left side (`old`) to the
/// newest they reach unchanged, as they are on its left side. Lines no view changes
/// are the session's.
pub fn attribute(
    repo: &mut dyn Source,
    anchor: Anchor,
    views: &[(String, String, Option<String>)],
) -> Result<Anchor> {
    let (rev, mut file, mut lines, old) = match anchor {
        Anchor::Lines {
            rev,
            file,
            start,
            end,
            old,
            ..
        } => (rev, file, Some((start, end)), old),
        Anchor::File { rev, file, old, .. } => (rev, file, None, old),
        _ => return Ok(anchor),
    };
    let on = |rev: &str, file: &str, lines: Option<(u32, u32)>, commit: &Option<String>| {
        let (rev, file, commit) = (rev.to_string(), file.to_string(), commit.clone());
        match lines {
            Some((start, end)) => Anchor::Lines {
                rev,
                file,
                start,
                end,
                old,
                commit,
            },
            None => Anchor::File {
                rev,
                file,
                old,
                commit,
            },
        }
    };
    if let [(_, _, commit)] = views {
        return Ok(on(&rev, &file, lines, commit));
    }
    // From the side they are on, view by view, carried across each.
    let order: Vec<_> = if old {
        views.iter().collect()
    } else {
        views.iter().rev().collect()
    };
    let mut found = None;
    for (l, r, commit) in order {
        let (from, to) = if old { (l, r) } else { (r, l) };
        let (from_pin, to_pin) = (repo.pin(from)?, repo.pin(to)?);
        let files = repo
            .changes(&from_pin, &to_pin)?
            .with_context(|| format!("the diff of {} is gone", short(to)))?;
        let (changed, next) = match lines {
            Some((start, end)) => {
                let next = carry(files, &file, start, end);
                (next.is_none(), next.map(|(f, s, e)| (f, Some((s, e)))))
            }
            None => (
                files.iter().any(|f| f.old_path == file),
                carry_file(files, &file).map(|f| (f, None)),
            ),
        };
        if changed {
            found = Some(on(from, &file, lines, commit));
            if !old {
                break;
            }
        }
        let Some(next) = next else { break };
        (file, lines) = next;
    }
    Ok(found.unwrap_or(Anchor::Session))
}

/// Lines `start..=end` of `path` on the old side of `files`, a diff without context, on
/// their new side; None when they change, delete or insert any line among them.
fn carry(files: &[FileDiff], path: &str, start: u32, end: u32) -> Option<(String, u32, u32)> {
    let Some(f) = files.iter().find(|f| f.old_path == path) else {
        return Some((path.to_string(), start, end));
    };
    // A binary change has no hunks to tell what it kept.
    if f.status == 'D' || f.binary {
        return None;
    }
    // Where the last hunk before `start` ends, old and new.
    let (mut old_end, mut new_end) = (1, 1);
    for h in &f.hunks {
        let (old, new) = h.firsts();
        if end < old {
            break;
        }
        // Overlaps them, or inserts right before one past `start`.
        if start < old + h.old_len {
            return None;
        }
        (old_end, new_end) = (old + h.old_len, new + h.new_len);
    }
    let at = |line: u32| line - old_end + new_end;
    Some((f.new_path.clone(), at(start), at(end)))
}

/// `path` on the old side of `files`, a diff, on their new side; None when deleted.
fn carry_file(files: &[FileDiff], path: &str) -> Option<String> {
    match files.iter().find(|f| f.old_path == path) {
        Some(f) if f.status == 'D' => None,
        Some(f) => Some(f.new_path.clone()),
        None => Some(path.to_string()),
    }
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

/// `rev` pinned for a thread of session `id`: the tree of `staged` / `unstaged` is
/// kept, as nothing else keeps it from git gc.
fn pin(repo: &mut dyn Source, id: &str, rev: &str) -> Result<String> {
    let pinned = repo.pin(rev)?;
    if worktree(rev) {
        repo.keep(id, &pinned)?;
    }
    Ok(pinned)
}

/// Lines `start..=end` of `file` at `rev`; None if the file or lines are absent.
fn snippet(
    repo: &mut dyn Source,
    rev: &str,
    file: &str,
    start: u32,
    end: u32,
) -> Result<Option<String>> {
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
    use super::*;
    use crate::repo::{Hunk, Kind, Line};

    #[test]
    fn carry_shifts_kept_lines() {
        let line = |kind, text: &str| Line {
            kind,
            text: text.into(),
        };
        // Renamed `a` -> `b`, line 2 changed to two lines, then line 9 deleted.
        let files = [FileDiff {
            status: 'R',
            old_path: "a".into(),
            new_path: "b".into(),
            binary: false,
            hunks: vec![
                Hunk {
                    old_start: 2,
                    old_len: 1,
                    new_start: 2,
                    new_len: 2,
                    lines: vec![
                        line(Kind::Del, "x"),
                        line(Kind::Add, "y"),
                        line(Kind::Add, "z"),
                    ],
                },
                Hunk {
                    old_start: 9,
                    old_len: 1,
                    new_start: 9,
                    new_len: 0,
                    lines: vec![line(Kind::Del, "9")],
                },
            ],
        }];
        let at = |path, n| carry(&files, path, n, n).map(|(f, _, l)| (f, l));
        assert_eq!(at("a", 1), Some(("b".into(), 1)));
        assert_eq!(at("a", 2), None);
        assert_eq!(at("a", 3), Some(("b".into(), 4)));
        assert_eq!(at("a", 5), Some(("b".into(), 6)));
        assert_eq!(at("a", 9), None);
        assert_eq!(at("a", 10), Some(("b".into(), 10)));
        assert_eq!(at("c", 7), Some(("c".into(), 7)));
        // A run of lines moves whole, and none of it may change.
        assert_eq!(carry(&files, "a", 3, 8), Some(("b".into(), 4, 9)));
        assert_eq!(carry(&files, "a", 1, 3), None);
        assert_eq!(carry(&files, "a", 8, 10), None);
        // Lines added right before the run move it; lines added inside it change it.
        let insert = |at| FileDiff {
            status: 'M',
            old_path: "a".into(),
            new_path: "a".into(),
            binary: false,
            hunks: vec![Hunk {
                old_start: at - 1,
                old_len: 0,
                new_start: at,
                new_len: 1,
                lines: vec![line(Kind::Add, "n")],
            }],
        };
        assert_eq!(carry(&[insert(3)], "a", 3, 4), Some(("a".into(), 4, 5)));
        assert_eq!(carry(&[insert(4)], "a", 3, 4), None);
    }

    /// Diffs by (from, to), none where unset; revs are pinned as they are.
    struct Diffs(Vec<((&'static str, &'static str), Vec<FileDiff>)>);

    impl Source for Diffs {
        fn read(&mut self, _: &str, _: &str) -> Result<Option<String>> {
            unreachable!()
        }
        fn pin(&mut self, rev: &str) -> Result<String> {
            Ok(rev.to_string())
        }
        fn keep(&self, _: &str, _: &str) -> Result<()> {
            unreachable!()
        }
        fn commit(&self, _: &str) -> Result<String> {
            unreachable!()
        }
        fn left(&self, _: &str) -> Result<String> {
            unreachable!()
        }
        fn commits(&mut self, _: &str, _: &str) -> Result<Vec<Commit>> {
            unreachable!()
        }
        fn changes(&mut self, from: &str, to: &str) -> Result<Option<&[FileDiff]>> {
            let found = self.0.iter().find(|((f, t), _)| *f == from && *t == to);
            Ok(Some(found.map_or(&[][..], |(_, files)| files)))
        }
    }

    #[test]
    fn attribute_finds_the_view_that_changed_it() {
        let hunk = |old_start, old_len, new_start, new_len| Hunk {
            old_start,
            old_len,
            new_start,
            new_len,
            lines: Vec::new(),
        };
        let a = |hunk| {
            vec![FileDiff {
                status: 'M',
                old_path: "a".into(),
                new_path: "a".into(),
                binary: false,
                hunks: vec![hunk],
            }]
        };
        // c1 changes line 2 of `a`; c2 adds a line 6.
        let mut repo = Diffs(vec![
            (("b", "c1"), a(hunk(2, 1, 2, 1))),
            (("c1", "b"), a(hunk(2, 1, 2, 1))),
            (("c1", "c2"), a(hunk(5, 0, 6, 1))),
            (("c2", "c1"), a(hunk(6, 1, 5, 0))),
        ]);
        let views: [(String, String, Option<String>); 2] = [
            ("b".into(), "c1".into(), Some("c1".into())),
            ("c1".into(), "c2".into(), Some("c2".into())),
        ];
        let lines = |rev: &str, n| Anchor::Lines {
            rev: rev.into(),
            file: "a".into(),
            start: n,
            end: n,
            old: rev == "b",
            commit: None,
        };
        let mut at = |anchor, views: &[_]| {
            let anchor = attribute(&mut repo, anchor, views).unwrap();
            match anchor {
                Anchor::Lines {
                    rev, end, commit, ..
                } => format!("{rev} {end} {}", commit.unwrap()),
                Anchor::File { rev, commit, .. } => format!("{rev} {}", commit.unwrap()),
                Anchor::Session => "session".into(),
                Anchor::Commit { .. } => unreachable!(),
            }
        };
        assert_eq!(at(lines("c2", 6), &views), "c2 6 c2");
        assert_eq!(at(lines("c2", 2), &views), "c1 2 c1");
        assert_eq!(at(lines("c2", 3), &views), "session");
        assert_eq!(at(lines("b", 2), &views), "b 2 c1");
        let file = Anchor::File {
            rev: "c2".into(),
            file: "a".into(),
            old: false,
            commit: None,
        };
        assert_eq!(at(file, &views), "c2 c2");
        // One view takes all, changed or not.
        assert_eq!(at(lines("c1", 3), &views[..1]), "c1 3 c1");
    }

    #[test]
    fn names() {
        let mut s = Session::new("1".into(), "r".into(), "b".into(), "e".into());
        s.set_name("  a b  ").unwrap();
        assert_eq!(s.name.as_deref(), Some("a b"));
        s.set_name("   ").unwrap();
        assert_eq!(s.name, None);
        assert!(s.set_name("a\nb").is_err());
    }
}
