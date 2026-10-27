mod parse;

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use anyhow::{Context, Result, bail};

pub use parse::{FileDiff, Kind};

const UNTRACKED_MAX_BYTES: u64 = 1 << 20;

pub struct Repo {
    root: String,
    common_dir: String,
    index: String,
    /// `staged` / `unstaged` -> tree id, computed once per process.
    trees: HashMap<String, String>,
    /// Untracked files over the size limit, left out of the unstaged tree.
    skipped: Vec<String>,
    cat: Option<CatFile>,
}

struct CatFile {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Repo {
    pub fn open() -> Result<Repo> {
        let out = run(Command::new("git").args([
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
            skipped: Vec::new(),
            cat: None,
        })
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
        let base = self.commit(base)?;
        let end = match end {
            "HEAD" | "staged" | "unstaged" => end.to_string(),
            _ => self.commit(end)?,
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
        let ok = self
            .git(&["merge-base", "--is-ancestor", base, tip])
            .status()
            .context("run git")?
            .success();
        if !ok {
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
            "--raw",
            "-z",
            "--patch",
            base,
            &to,
        ]))?;
        let mut files = parse::parse(&out)?;
        if end == "unstaged" {
            files.extend(self.skipped.iter().map(|p| FileDiff::skipped(p)));
        }
        Ok(files)
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
        if !matches!(rev, "staged" | "unstaged") {
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
        let untracked = run(&mut git(&[
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
        ]))?;
        for path in untracked.split(|&b| b == 0) {
            // Nested repos are listed as `dir/`; leave them out.
            if path.is_empty() || path.ends_with(b"/") {
                continue;
            }
            if fs::symlink_metadata(Path::new(&self.root).join(OsStr::from_bytes(path)))?.len()
                > UNTRACKED_MAX_BYTES
            {
                skipped.push(String::from_utf8_lossy(path).into_owned());
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
