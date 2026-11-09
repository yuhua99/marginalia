mod parse;

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{ChildStdin, ChildStdout, Command, Stdio};

use anyhow::{Context, Result, bail};

pub use parse::{FileDiff, Hunk, Kind, Line};

use crate::session::Source;

pub struct Repo {
    root: String,
    common_dir: String,
    index: String,
    /// Of `Repo::bare`: an upload, with no worktree or index.
    bare: bool,
    /// `staged` / `unstaged` -> tree id, computed once until `refresh`.
    trees: HashMap<String, String>,
    /// `base..tip` -> its commits, the tip pinned so a moved end lists again.
    ranges: HashMap<String, Vec<Commit>>,
    /// (from, to) -> `changes` between them.
    changes: HashMap<(String, String), Option<Vec<FileDiff>>>,
    cat: Option<CatFile>,
}

/// A commit of `git log`.
#[derive(Clone)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
    pub author: String,
    /// Relative author date, as "3 days ago".
    pub when: String,
}

/// A `git cat-file --batch`; it ends when its input closes, with the repo.
struct CatFile {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Repo {
    pub fn open() -> Result<Repo> {
        Repo::at(Path::new("."))
    }

    /// The repo containing `dir`.
    pub fn at(dir: &Path) -> Result<Repo> {
        let out = run(Command::new("git").arg("-C").arg(dir).args([
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
            "--show-toplevel",
            "--git-path",
            "index",
        ]))?;
        let out = String::from_utf8(out)?;
        let [common_dir, root, index] = out.lines().collect::<Vec<_>>()[..] else {
            bail!("unexpected git rev-parse output: {out}");
        };
        Ok(Repo {
            root: root.into(),
            common_dir: common_dir.into(),
            index: index.into(),
            bare: false,
            trees: HashMap::new(),
            ranges: HashMap::new(),
            changes: HashMap::new(),
            cat: None,
        })
    }

    /// The bare repo at `dir`, of what a client uploaded: HEAD is the client's, and
    /// `staged` / `unstaged` the trees of `refs/mrg/staged` / `refs/mrg/unstaged`.
    /// Git reads no user or system config here, as that of the server is not the user's.
    pub fn bare(dir: &Path) -> Result<Repo> {
        let dir = fs::canonicalize(dir)?;
        let dir = dir.to_str().context("a path not in UTF-8")?.to_string();
        Ok(Repo {
            root: dir.clone(),
            common_dir: dir,
            index: String::new(),
            bare: true,
            trees: HashMap::new(),
            ranges: HashMap::new(),
            changes: HashMap::new(),
            cat: None,
        })
    }

    /// Writes the trees of `staged` / `unstaged` again on their next use, as they are then.
    pub fn refresh(&mut self) {
        self.trees.clear();
    }

    /// The git dir the worktrees of the repo share.
    pub fn common_dir(&self) -> &Path {
        Path::new(&self.common_dir)
    }

    /// Keeps `tree` from git gc while session `id` lasts, as `refs/mrg/<id>/<tree>`.
    pub fn keep(&self, id: &str, tree: &str) -> Result<()> {
        run(&mut self.git(&["update-ref", &format!("refs/mrg/{id}/{tree}"), tree]))?;
        Ok(())
    }

    /// Drops what `keep` kept for session `id`.
    pub fn forget(&self, id: &str) -> Result<()> {
        let refs = run(&mut self.git(&[
            "for-each-ref",
            "--format=delete %(refname)",
            &format!("refs/mrg/{id}"),
        ]))?;
        let mut child = self
            .git(&["update-ref", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("run git update-ref")?;
        child.stdin.take().unwrap().write_all(&refs)?;
        let out = child.wait_with_output()?;
        if !out.status.success() {
            bail!(
                "git update-ref: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    /// Splits `A..end` into the base commit sha (the empty tree when A is left out,
    /// to start before the root commit; `staged` kept symbolic, before `unstaged`) and
    /// the end (a commit sha, or `HEAD` / `staged` / `unstaged` / a full ref `refs/...`
    /// kept symbolic, to follow it).
    pub fn resolve(&self, range: &str) -> Result<(String, String)> {
        let Some((base, end)) = range.split_once("..") else {
            bail!("range must look like A..END or ..END, got {range:?}");
        };
        let empty = self.empty_tree()?;
        // The empty tree as well: the base `mrg show` prints for a range from before the root.
        let base = if base.is_empty() || base == empty {
            empty
        } else if base == "staged" {
            base.to_string()
        } else {
            self.commit(base)?
        };
        let end = if end == "HEAD" || worktree(end) {
            end.to_string()
        } else if end.starts_with("refs/") {
            // Checked now, not on the first load.
            self.commit(end)?;
            end.to_string()
        } else {
            self.commit(end)?
        };
        self.verify(&base, &end)?;
        Ok((base, end))
    }

    /// Fails when `base` is no longer an ancestor of the range tip (history rewritten).
    pub fn verify(&self, base: &str, end: &str) -> Result<()> {
        // The index is no commit: only what is not staged comes after it.
        if base == "staged" {
            if end != "unstaged" {
                bail!("a range from staged ends at unstaged, got {end:?}");
            }
            return Ok(());
        }
        let tip = tip(end);
        let ok = self.ancestor(base, tip)?;
        // The empty tree (a range from before the root) is no commit; nothing to check.
        if !ok && base != self.empty_tree()? {
            bail!("{base} is not an ancestor of {tip} (history rewritten?)");
        }
        Ok(())
    }

    /// Commit `a` is `b` or an ancestor of it.
    pub fn ancestor(&self, a: &str, b: &str) -> Result<bool> {
        Ok(self
            .git(&["merge-base", "--is-ancestor", a, b])
            .stderr(Stdio::null())
            .status()
            .context("run git")?
            .success())
    }

    /// Full sha of the commit `end` stands on now.
    pub fn tip_of(&self, end: &str) -> Result<String> {
        self.commit(tip(end))
    }

    /// Files changed from `base` to `end`, with `context` lines around each change.
    pub fn diff(&mut self, base: &str, end: &str, context: u32) -> Result<Vec<FileDiff>> {
        let from = self.object(base)?;
        let to = self.object(end)?;
        self.diff_of(&[&from, &to], context, &[])
    }

    /// `diff` from `base` to `end` as it is now, in `paths` if any: `staged` and
    /// `unstaged` are read in place, writing no tree.
    pub fn diff_now(&self, base: &str, end: &str, paths: &[String]) -> Result<Vec<FileDiff>> {
        let revs = match (base, end) {
            // The index to the worktree; `verify` made the end `unstaged`.
            ("staged", _) => vec![],
            (_, "unstaged") => vec![base],
            (_, "staged") => vec!["--cached", base],
            _ => vec![base, end],
        };
        self.diff_of(&revs, 3, paths)
    }

    fn diff_of(&self, revs: &[&str], context: u32, paths: &[String]) -> Result<Vec<FileDiff>> {
        let context = format!("-U{context}");
        let mut cmd = self.git(&[
            "diff",
            "--histogram",
            "-M",
            "--no-ext-diff",
            "--no-color",
            "--no-textconv",
            "--no-relative",
            "--submodule=short",
            &context,
            // Not the user's: context between hunks would put kept lines in them.
            "--inter-hunk-context=0",
            "-O/dev/null",
            "--raw",
            "-z",
            "--patch",
        ]);
        cmd.args(revs).arg("--").args(paths);
        // Errors in English, for `changes` to tell objects missing.
        let out = run(cmd.env("LC_ALL", "C"))?;
        parse::parse(&out)
    }

    /// `diff(from, to, 0)` of pinned revs, kept: what carries lines from one to the other.
    /// None when objects it needs are gone, as blobs only ever staged, then collected.
    pub fn changes(&mut self, from: &str, to: &str) -> Result<Option<&[FileDiff]>> {
        let key = (from.to_string(), to.to_string());
        if !self.changes.contains_key(&key) {
            let files = match self.diff(from, to, 0) {
                Ok(files) => Some(files),
                Err(e) if missing(&e) => None,
                Err(e) => return Err(e),
            };
            self.changes.insert(key.clone(), files);
        }
        Ok(self.changes[&key].as_deref())
    }

    /// Paths of the files at `rev`, leaving out submodules.
    pub fn files(&mut self, rev: &str) -> Result<Vec<String>> {
        let object = self.object(rev)?;
        let out = run(&mut self.git(&["ls-tree", "-r", "-z", &object]))?;
        // `<mode> <type> <object>\t<path>` per entry.
        Ok(out
            .split(|&b| b == 0)
            .filter_map(|entry| {
                let (meta, path) = std::str::from_utf8(entry).ok()?.split_once('\t')?;
                (meta.split(' ').nth(1) == Some("blob")).then(|| path.to_string())
            })
            .collect())
    }

    /// Content of `path` at `rev` (commit, `HEAD`, `staged` or `unstaged`); None if absent.
    pub fn read(&mut self, rev: &str, path: &str) -> Result<Option<String>> {
        let object = format!("{}:{path}", self.object(rev)?);
        if object.contains('\n') {
            bail!("invalid path {path:?}");
        }
        if self.cat.is_none() {
            let mut child = self
                .git(&["cat-file", "--batch"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .context("run git cat-file")?;
            let stdin = child.stdin.take().unwrap();
            let stdout = BufReader::new(child.stdout.take().unwrap());
            self.cat = Some(CatFile { stdin, stdout });
        }
        let cat = self.cat.as_mut().unwrap();
        writeln!(cat.stdin, "{object}")?;
        cat.stdin.flush()?;
        let mut header = String::new();
        cat.stdout.read_line(&mut header)?;
        if header.ends_with(" missing\n") {
            return Ok(None);
        }
        // "<oid> <type> <size>"
        let size: usize = header
            .trim_end()
            .rsplit(' ')
            .next()
            .and_then(|s| s.parse().ok())
            .with_context(|| format!("unexpected git cat-file output: {header:?}"))?;
        let mut buf = vec![0; size + 1];
        cat.stdout.read_exact(&mut buf)?;
        buf.pop();
        Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
    }

    /// An id for `rev` that does not move: the commit sha, or the tree of `staged` /
    /// `unstaged` as of now; an id as is.
    pub fn pin(&mut self, rev: &str) -> Result<String> {
        if sha(rev) {
            Ok(rev.to_string())
        } else if worktree(rev) {
            self.object(rev)
        } else {
            self.commit(rev)
        }
    }

    /// A tree-ish for `rev`: commits as is, `staged` / `unstaged` as a written tree, or
    /// the uploaded one in a bare repo.
    fn object(&mut self, rev: &str) -> Result<String> {
        if !worktree(rev) {
            return Ok(rev.to_string());
        }
        if let Some(tree) = self.trees.get(rev) {
            return Ok(tree.clone());
        }
        let tree = if self.bare {
            self.tree(&format!("refs/mrg/{rev}"))
                .with_context(|| format!("no {rev} uploaded"))?
        } else {
            self.index_tree(rev == "unstaged")?
        };
        self.trees.insert(rev.to_string(), tree.clone());
        Ok(tree)
    }

    /// Writes a tree from a temporary copy of the index, leaving the real index untouched.
    /// With `worktree`, tracked changes are added to the copy first; untracked files stay out.
    fn index_tree(&mut self, worktree: bool) -> Result<String> {
        // Beside the index, as git stash's was.
        let tmp = format!("{}.mrg.{}", self.index, std::process::id());
        fs::copy(&self.index, &tmp).context("copy git index")?;
        let tree = self.write_tree(Path::new(&tmp), worktree);
        let _ = fs::remove_file(&tmp);
        tree
    }

    fn write_tree(&self, index: &Path, worktree: bool) -> Result<String> {
        let git = |args: &[&str]| {
            let mut cmd = self.git(args);
            cmd.env("GIT_INDEX_FILE", index);
            cmd
        };
        if worktree {
            run(&mut git(&["add", "-u"]))?;
        }
        text(run(&mut git(&["write-tree"]))?)
    }

    /// Commits in `base..end`, oldest first.
    pub fn commits(&mut self, base: &str, end: &str) -> Result<Vec<Commit>> {
        // From the index, there are none.
        if base == "staged" {
            return Ok(Vec::new());
        }
        // A tree as base (before the root) is ignored by git log, listing all history.
        let range = format!("{base}..{}", self.commit(tip(end))?);
        if !self.ranges.contains_key(&range) {
            let commits = self.log(&["--reverse", &range])?;
            self.ranges.insert(range.clone(), commits);
        }
        Ok(self.ranges[&range].clone())
    }

    /// `unstaged` when the worktree differs from the index, `staged` when the index
    /// differs from HEAD; as `index_tree` builds them, or as uploaded in a bare repo.
    pub fn uncommitted(&self) -> Result<Vec<&'static str>> {
        if self.bare {
            // Both are uploaded for an end of `unstaged`, the only one that asks.
            let tree = |rev: &str| self.tree(rev).with_context(|| format!("no {rev} uploaded"));
            let head = tree("HEAD")?;
            let staged = tree("refs/mrg/staged")?;
            let unstaged = tree("refs/mrg/unstaged")?;
            let mut revs = Vec::new();
            if unstaged != staged {
                revs.push("unstaged");
            }
            if staged != head {
                revs.push("staged");
            }
            return Ok(revs);
        }
        let differs = |args: &[&str]| -> Result<bool> {
            let mut cmd = self.git(args);
            let out = cmd.output().context("run git")?;
            match out.status.code() {
                Some(0) => Ok(false),
                Some(1) => Ok(true),
                _ => bail!("{:?}: {}", cmd, String::from_utf8_lossy(&out.stderr).trim()),
            }
        };
        let mut revs = Vec::new();
        if differs(&["diff", "--quiet", "--ignore-submodules=dirty"])? {
            revs.push("unstaged");
        }
        if differs(&["diff", "--quiet", "--cached", "HEAD"])? {
            revs.push("staged");
        }
        Ok(revs)
    }

    /// Up to `n` first-parent commits back from `from` (included), newest first.
    pub fn recent(&self, from: &str, n: usize) -> Result<Vec<Commit>> {
        self.log(&["-n", &n.to_string(), from])
    }

    /// First-parent `git log`.
    fn log(&self, args: &[&str]) -> Result<Vec<Commit>> {
        let mut log = vec!["log", "--first-parent", "--format=%H%x1f%an%x1f%ar%x1f%s"];
        log.extend(args);
        let out = run(&mut self.git(&log))?;
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .map(|l| {
                let mut fields = l.splitn(4, '\x1f').map(str::to_string);
                let mut next = || fields.next().unwrap_or_default();
                Commit {
                    sha: next(),
                    author: next(),
                    when: next(),
                    subject: next(),
                }
            })
            .collect())
    }

    /// The first parent of `commit`; None for a root commit.
    pub fn parent(&self, commit: &str) -> Result<Option<String>> {
        let parents = text(run(&mut self.git(&["log", "-1", "--format=%P", commit]))?)?;
        Ok(parents
            .split(' ')
            .next()
            .filter(|p| !p.is_empty())
            .map(str::to_string))
    }

    /// The full message of `commit`.
    pub fn message(&self, commit: &str) -> Result<String> {
        text(run(&mut self.git(&["log", "-1", "--format=%B", commit]))?)
    }

    /// The URL of origin.
    pub fn origin(&self) -> Result<String> {
        text(run(&mut self.git(&["remote", "get-url", "origin"]))?)
    }

    /// Fetches `refspecs` from origin; with `prompt`, git and ssh may ask for a
    /// password, else the terminal is the TUI's and they fail.
    pub fn fetch(&self, refspecs: &[String], prompt: bool) -> Result<()> {
        let mut cmd = self.git(&["fetch", "--quiet", "--no-tags", "origin"]);
        cmd.args(refspecs);
        if !prompt {
            cmd.env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::null());
            // ssh asks on /dev/tty, past stdin; unless the user set how ssh runs.
            let set = ["GIT_SSH_COMMAND", "GIT_SSH"]
                .iter()
                .any(|v| std::env::var_os(v).is_some())
                || self
                    .git(&["config", "core.sshCommand"])
                    .output()?
                    .status
                    .success();
            if !set {
                cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
            }
        }
        run(&mut cmd)?;
        Ok(())
    }

    /// The best common ancestor of commits `a` and `b`.
    pub fn merge_base(&self, a: &str, b: &str) -> Result<String> {
        text(run(&mut self.git(&["merge-base", a, b]))?)
    }

    /// The empty tree, to diff a root commit against.
    pub fn empty_tree(&self) -> Result<String> {
        text(run(self
            .git(&["hash-object", "-t", "tree", "--stdin"])
            .stdin(Stdio::null()))?)
    }

    /// Full sha of commit `rev`.
    pub fn commit(&self, rev: &str) -> Result<String> {
        run(&mut self.git(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ]))
        .and_then(text)
        .with_context(|| format!("unknown commit {rev:?}"))
    }

    /// The tree of `rev`.
    fn tree(&self, rev: &str) -> Result<String> {
        text(run(&mut self.git(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{tree}}"),
        ]))?)
    }

    fn git(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(&self.root).args(args);
        if self.bare {
            // GIT_DIR: a broken repo there fails, rather than git finding one around it.
            cmd.env("GIT_DIR", &self.root)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1");
        }
        cmd
    }
}

impl Source for Repo {
    fn read(&mut self, rev: &str, path: &str) -> Result<Option<String>> {
        Repo::read(self, rev, path)
    }

    fn pin(&mut self, rev: &str) -> Result<String> {
        Repo::pin(self, rev)
    }

    fn keep(&self, id: &str, tree: &str) -> Result<()> {
        Repo::keep(self, id, tree)
    }

    fn commit(&self, rev: &str) -> Result<String> {
        Repo::commit(self, rev)
    }

    fn left(&self, commit: &str) -> Result<String> {
        match self.parent(commit)? {
            Some(parent) => Ok(parent),
            None => self.empty_tree(),
        }
    }

    fn commits(&mut self, base: &str, end: &str) -> Result<Vec<Commit>> {
        Repo::commits(self, base, end)
    }

    fn changes(&mut self, from: &str, to: &str) -> Result<Option<&[FileDiff]>> {
        Repo::changes(self, from, to)
    }
}

/// `staged` or `unstaged`: a state of the worktree on top of HEAD, not a commit.
pub fn worktree(rev: &str) -> bool {
    matches!(rev, "staged" | "unstaged")
}

/// The ref origin's `branch` is fetched to.
pub fn tracking(branch: &str) -> String {
    format!("refs/remotes/origin/{branch}")
}

/// The commit an end stands on: `staged` / `unstaged` stand on HEAD.
fn tip(end: &str) -> &str {
    if worktree(end) { "HEAD" } else { end }
}

/// `rev` is a full object id, not a name like `HEAD` or `unstaged`.
pub fn sha(rev: &str) -> bool {
    rev.len() >= 40 && rev.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A commit sha shortened for display; other revs as is.
pub fn short(rev: &str) -> &str {
    if sha(rev) { &rev[..7] } else { rev }
}

/// `e`, of a git run, is about an object it could not find.
fn missing(e: &anyhow::Error) -> bool {
    let e = format!("{e:#}");
    e.contains("unable to read") || e.contains("bad object")
}

fn run(cmd: &mut Command) -> Result<Vec<u8>> {
    let out = cmd.output().context("run git")?;
    if !out.status.success() {
        bail!("{:?}: {}", cmd, String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

fn text(out: Vec<u8>) -> Result<String> {
    Ok(String::from_utf8(out)?.trim_end().to_string())
}

/// `files` with each line's old and new number, as `comment` takes them: a header per
/// file, then its hunks, `...` between them.
pub fn numbered(files: &[FileDiff]) -> String {
    let num = |n: Option<u32>| n.map_or(String::new(), |n| n.to_string());
    let mut out = String::new();
    for f in files {
        if f.old_path == f.new_path {
            writeln!(out, "{} {}", f.status, f.new_path).unwrap();
        } else {
            writeln!(out, "{} {} -> {}", f.status, f.old_path, f.new_path).unwrap();
        }
        if f.binary {
            out.push_str("  (binary)\n");
        }
        for (i, h) in f.hunks.iter().enumerate() {
            if i > 0 {
                out.push_str("  ...\n");
            }
            let (mut old, mut new) = h.firsts();
            for l in &h.lines {
                let (o, n, sign) = match l.kind {
                    Kind::Ctx => (Some(old), Some(new), ' '),
                    Kind::Del => (Some(old), None, '-'),
                    Kind::Add => (None, Some(new), '+'),
                };
                old += o.is_some() as u32;
                new += n.is_some() as u32;
                writeln!(out, "{:>5} {:>5} {sign} {}", num(o), num(n), l.text).unwrap();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_lines() {
        let line = |kind, text: &str| Line {
            kind,
            text: text.into(),
        };
        let hunk = |old_start, old_len, new_start, new_len, lines| Hunk {
            old_start,
            old_len,
            new_start,
            new_len,
            lines,
        };
        let files = [FileDiff {
            status: 'R',
            old_path: "a".into(),
            new_path: "b".into(),
            binary: false,
            hunks: vec![
                hunk(
                    1,
                    2,
                    1,
                    2,
                    vec![
                        line(Kind::Ctx, "1"),
                        line(Kind::Del, "x"),
                        line(Kind::Add, "y"),
                    ],
                ),
                // An addition only: the old side starts at the line before.
                hunk(8, 0, 9, 1, vec![line(Kind::Add, "z")]),
            ],
        }];
        assert_eq!(
            numbered(&files),
            "R a -> b
    1     1   1
    2       - x
          2 + y
  ...
          9 + z
"
        );
    }
}
