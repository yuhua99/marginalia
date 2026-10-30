mod parse;

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use anyhow::{Context, Result, bail};

pub use parse::{FileDiff, Kind};
#[cfg(test)]
pub use parse::{Hunk, Line};

pub struct Repo {
    root: String,
    common_dir: String,
    index: String,
    /// `staged` / `unstaged` -> tree id, computed once per object directory.
    trees: HashMap<String, String>,
    /// `base..tip` -> its commits, the tip pinned so a moved end lists again.
    ranges: HashMap<String, Vec<Commit>>,
    /// (from, to) -> `changes` between them.
    changes: HashMap<(String, String), Option<Vec<FileDiff>>>,
    cat: Option<CatFile>,
    /// Where the objects mrg writes go (trees of `staged` / `unstaged`), the repo's
    /// own read through it, so nothing lands in `.git`; set by `use_objects`.
    objects: Option<PathBuf>,
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

struct CatFile {
    child: Child,
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
            trees: HashMap::new(),
            ranges: HashMap::new(),
            changes: HashMap::new(),
            cat: None,
            objects: None,
        })
    }

    /// Writes objects to `dir` from now on. Trees and the reader made for the
    /// previous directory are dropped.
    pub fn use_objects(&mut self, dir: PathBuf) -> Result<()> {
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        // Absolute: git runs from the repo root.
        self.objects = Some(fs::canonicalize(&dir)?);
        self.trees.clear();
        if let Some(mut cat) = self.cat.take() {
            // Closing its input ends it.
            drop(cat.stdin);
            cat.child.wait()?;
        }
        Ok(())
    }

    /// Identifies the repo; worktrees of one repo share the same key.
    pub fn key(&self) -> String {
        self.common_dir.replace('/', "%")
    }

    /// Splits `A..end` into the base commit sha (the empty tree when A is left out,
    /// to start before the root commit) and the end (a commit sha, or `HEAD` /
    /// `staged` / `unstaged` / a full ref `refs/...` kept symbolic, to follow it).
    pub fn resolve(&self, range: &str) -> Result<(String, String)> {
        let Some((base, end)) = range.split_once("..") else {
            bail!("range must look like A..END or ..END, got {range:?}");
        };
        let empty = self.empty_tree()?;
        // The empty tree as well: the base `mrg show` prints for a range from before the root.
        let base = if base.is_empty() || base == empty {
            empty
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
        let tip = tip(end);
        let ok = self
            .git(&["merge-base", "--is-ancestor", base, tip])
            .stderr(Stdio::null())
            .status()
            .context("run git")?
            .success();
        // The empty tree (a range from before the root) is no commit; nothing to check.
        if !ok && base != self.empty_tree()? {
            bail!("{base} is not an ancestor of {tip} (history rewritten?)");
        }
        Ok(())
    }

    /// Files changed from `base` to `end`, with `context` lines around each change.
    pub fn diff(&mut self, base: &str, end: &str, context: u32) -> Result<Vec<FileDiff>> {
        let to = self.object(end)?;
        self.diff_of(&[base, &to], context, &[])
    }

    /// `diff` from `base` to `end` as it is now, in `paths` if any: `staged` and
    /// `unstaged` are read in place, writing no tree.
    pub fn diff_now(&self, base: &str, end: &str, paths: &[String]) -> Result<Vec<FileDiff>> {
        let revs = match end {
            "unstaged" => vec![base],
            "staged" => vec!["--cached", base],
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
            self.cat = Some(CatFile {
                child,
                stdin,
                stdout,
            });
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

    /// A tree-ish for `rev`: commits as is, `staged` / `unstaged` as a written tree.
    fn object(&mut self, rev: &str) -> Result<String> {
        if !worktree(rev) {
            return Ok(rev.to_string());
        }
        if let Some(tree) = self.trees.get(rev) {
            return Ok(tree.clone());
        }
        let tree = self.index_tree(rev == "unstaged")?;
        self.trees.insert(rev.to_string(), tree.clone());
        Ok(tree)
    }

    /// Writes a tree from a temporary copy of the index, leaving the real index untouched.
    /// With `worktree`, tracked changes are added to the copy first; untracked files stay out.
    fn index_tree(&mut self, worktree: bool) -> Result<String> {
        let Some(objects) = &self.objects else {
            bail!("no object directory set; the tree would be written into .git");
        };
        let tmp = objects.with_file_name(format!("index-{}", std::process::id()));
        fs::copy(&self.index, &tmp).context("copy git index")?;
        let tree = self.write_tree(&tmp, worktree);
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
        // A tree as base (before the root) is ignored by git log, listing all history.
        let range = format!("{base}..{}", self.commit(tip(end))?);
        if !self.ranges.contains_key(&range) {
            let commits = self.log(&["--reverse", &range])?;
            self.ranges.insert(range.clone(), commits);
        }
        Ok(self.ranges[&range].clone())
    }

    /// `unstaged` / `staged` when their tree differs from HEAD, as `index_tree` builds it.
    pub fn uncommitted(&self) -> Result<Vec<&'static str>> {
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
        if differs(&["diff", "--quiet", "--ignore-submodules=dirty", "HEAD"])? {
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
    /// password, else the terminal is the TUI's and they fail. Not through `git`: the
    /// objects go to `.git` with the refs, not to a session's directory.
    pub fn fetch(&self, refspecs: &[String], prompt: bool) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.arg("-C")
            .arg(&self.root)
            .args(["fetch", "--quiet", "--no-tags", "origin"])
            .args(refspecs);
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

    fn git(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(&self.root).args(args);
        if let Some(dir) = &self.objects {
            // Quoted, as the list is split on colons.
            let repo = format!("{}/objects", self.common_dir)
                .replace('\\', "\\\\")
                .replace('"', "\\\"");
            cmd.env("GIT_OBJECT_DIRECTORY", dir)
                .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", format!("\"{repo}\""));
        }
        cmd
    }
}

/// `staged` or `unstaged`: a state of the worktree on top of HEAD, not a commit.
pub fn worktree(rev: &str) -> bool {
    matches!(rev, "staged" | "unstaged")
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
