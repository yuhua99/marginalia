mod parse;

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use anyhow::{Context, Result, bail};

pub use parse::{FileDiff, Kind};
#[cfg(test)]
pub use parse::{Hunk, Line};

const UNTRACKED_MAX_BYTES: u64 = 1 << 20;

pub struct Repo {
    root: String,
    common_dir: String,
    index: String,
    /// `staged` / `unstaged` -> tree id, computed once per process.
    trees: HashMap<String, String>,
    /// `base..end` -> its commits, listed once per process.
    ranges: HashMap<String, Vec<Commit>>,
    /// Untracked files over the size limit, left out of the unstaged tree.
    skipped: Vec<String>,
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

/// Where a line of `git blame` comes from.
pub struct Blame {
    /// The zero sha for lines not committed yet.
    pub commit: String,
    pub path: String,
    pub line: u32,
}

impl Blame {
    /// The line is not committed yet.
    pub fn uncommitted(&self) -> bool {
        self.commit.bytes().all(|c| c == b'0')
    }
}

struct CatFile {
    _child: Child,
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
            skipped: Vec::new(),
            cat: None,
        })
    }

    /// Identifies the repo; worktrees of one repo share the same key.
    pub fn key(&self) -> String {
        self.common_dir.replace('/', "%")
    }

    /// Splits `A..end` into the base commit sha (the empty tree when A is left out,
    /// to start before the root commit) and the end (a commit sha, or `HEAD` /
    /// `staged` / `unstaged` kept symbolic).
    pub fn resolve(&self, range: &str) -> Result<(String, String)> {
        let Some((base, end)) = range.split_once("..") else {
            bail!("range must look like A..END or ..END, got {range:?}");
        };
        let base = if base.is_empty() {
            self.empty_tree()?
        } else {
            self.commit(base)?
        };
        let end = if end == "HEAD" || worktree(end) {
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

    /// Files changed from `base` to `end`. With `end = unstaged`, untracked files
    /// over the size limit come last with `skipped` set.
    pub fn diff(&mut self, base: &str, end: &str) -> Result<Vec<FileDiff>> {
        let to = self.object(end)?;
        let out = run(&mut self.git(&[
            "diff",
            "--histogram",
            "-M",
            "--no-ext-diff",
            "--no-color",
            "--no-textconv",
            "--no-relative",
            "--submodule=short",
            "-U3",
            "-O/dev/null",
            "--raw",
            "-z",
            "--patch",
            base,
            &to,
        ]))?;
        let mut files = parse::parse(&out)?;
        if end == "unstaged" {
            files.extend(self.skipped.iter().map(|p| FileDiff::skipped(p)));
            files.sort_by(|a, b| a.new_path.cmp(&b.new_path));
        }
        Ok(files)
    }

    /// Paths of the files at `rev`, leaving out submodules.
    pub fn files(&mut self, rev: &str) -> Result<HashSet<String>> {
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
                _child: child,
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
    /// With `worktree`, tracked changes and untracked files are added to the copy first.
    fn index_tree(&mut self, worktree: bool) -> Result<String> {
        let tmp = format!("{}.mrg-{}", self.index, std::process::id());
        fs::copy(&self.index, &tmp).context("copy git index")?;
        let tree = self.write_tree(&tmp, worktree);
        let _ = fs::remove_file(&tmp);
        tree
    }

    fn write_tree(&mut self, index: &str, worktree: bool) -> Result<String> {
        let git = |args: &[&str]| {
            let mut cmd = self.git(args);
            cmd.env("GIT_INDEX_FILE", index);
            cmd
        };
        if !worktree {
            return text(run(&mut git(&["write-tree"]))?);
        }
        run(&mut git(&["add", "-u"]))?;
        let mut add = Vec::new();
        let mut skipped = Vec::new();
        for path in self.untracked()? {
            if fs::symlink_metadata(Path::new(&self.root).join(OsStr::from_bytes(&path)))?.len()
                > UNTRACKED_MAX_BYTES
            {
                skipped.push(String::from_utf8_lossy(&path).into_owned());
            } else {
                add.push(path);
            }
        }
        if !add.is_empty() {
            let mut cmd = git(&[
                "--literal-pathspecs",
                "add",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ]);
            let mut child = cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()
                .context("run git add")?;
            child.stdin.take().unwrap().write_all(&add.join(&0))?;
            if !child.wait()?.success() {
                bail!("git add untracked files failed");
            }
        }
        let tree = text(run(&mut git(&["write-tree"]))?)?;
        self.skipped = skipped;
        Ok(tree)
    }

    /// Untracked files that go into the unstaged tree.
    fn untracked(&self) -> Result<Vec<Vec<u8>>> {
        let out = run(&mut self.git(&["ls-files", "--others", "--exclude-standard", "-z"]))?;
        Ok(out
            .split(|&b| b == 0)
            // Nested repos are listed as `dir/`; leave them out.
            .filter(|p| !p.is_empty() && !p.ends_with(b"/"))
            .map(<[u8]>::to_vec)
            .collect())
    }

    /// Commits in `base..end`, oldest first.
    pub fn commits(&mut self, base: &str, end: &str) -> Result<Vec<Commit>> {
        // A tree as base (before the root) is ignored by git log, listing all history.
        let range = format!("{base}..{}", tip(end));
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
        if !self.untracked()?.is_empty()
            || differs(&["diff", "--quiet", "--ignore-submodules=dirty", "HEAD"])?
        {
            revs.push("unstaged");
        }
        if differs(&["diff", "--quiet", "--cached", "HEAD"])? {
            revs.push("staged");
        }
        Ok(revs)
    }

    /// Up to `n` first-parent commits back from HEAD, newest first.
    pub fn recent(&self, n: usize) -> Result<Vec<Commit>> {
        self.log(&["-n", &n.to_string(), "HEAD"])
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

    /// The commit (first parent) that last changed line `line` of `path` at `rev`;
    /// `since` when that is no later than `since`.
    pub fn blame(
        &mut self,
        rev: &str,
        since: Option<&str>,
        path: &str,
        line: u32,
    ) -> Result<Blame> {
        let lines = format!("{line},{line}");
        let not = since.map(|s| format!("^{s}"));
        let mut args = vec![
            "-c",
            "core.quotePath=false",
            "blame",
            "--porcelain",
            "--first-parent",
            // Not the user's ignored revs: owners are who really changed a line.
            "--no-ignore-revs-file",
            "-L",
            &lines,
        ];
        args.extend(not.as_deref());
        let out = if worktree(rev) {
            let content = self
                .read(rev, path)?
                .with_context(|| format!("{path} not found at {rev}"))?;
            // Blame needs the file in HEAD; a new one is all uncommitted.
            if self.read("HEAD", path)?.is_none() {
                return Ok(Blame {
                    commit: "0".repeat(40),
                    path: path.to_string(),
                    line,
                });
            }
            // On top of HEAD, as if `rev` were the worktree.
            args.extend(["--contents", "-", "--", path]);
            let mut child = self
                .git(&args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .context("run git blame")?;
            child.stdin.take().unwrap().write_all(content.as_bytes())?;
            let out = child.wait_with_output()?;
            if !out.status.success() {
                bail!("git blame: {}", String::from_utf8_lossy(&out.stderr).trim());
            }
            out.stdout
        } else {
            args.extend([rev, "--", path]);
            run(&mut self.git(&args))?
        };
        blame(&out)
    }

    /// The last commit in `from..to` (first parent) that still has line `line` of
    /// `path` at `from`.
    pub fn blame_reverse(&self, from: &str, to: &str, path: &str, line: u32) -> Result<Blame> {
        let lines = format!("{line},{line}");
        let range = format!("{from}..{to}");
        blame(&run(&mut self.git(&[
            "-c",
            "core.quotePath=false",
            "blame",
            "--porcelain",
            "--first-parent",
            "--no-ignore-revs-file",
            "--reverse",
            "-L",
            &lines,
            &range,
            "--",
            path,
        ]))?)
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

/// A commit sha shortened for display; other revs as is.
pub fn short(rev: &str) -> &str {
    if rev.len() >= 40 && rev.bytes().all(|b| b.is_ascii_hexdigit()) {
        &rev[..7]
    } else {
        rev
    }
}

/// The first line of `git blame --porcelain`: "<sha> <line there> <line here> <count>",
/// then headers including "filename <path>".
fn blame(out: &[u8]) -> Result<Blame> {
    let out = String::from_utf8_lossy(out);
    let mut lines = out.lines();
    let head = lines.next().unwrap_or_default();
    let mut fields = head.split(' ');
    let (Some(commit), Some(Ok(line))) = (fields.next(), fields.next().map(str::parse)) else {
        bail!("unexpected git blame output: {head:?}");
    };
    let path = lines
        .find_map(|l| l.strip_prefix("filename "))
        .context("git blame output without a filename")?;
    Ok(Blame {
        commit: commit.to_string(),
        path: path.to_string(),
        line,
    })
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
