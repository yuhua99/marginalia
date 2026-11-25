mod parse;

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{ChildStdin, ChildStdout, Command, Stdio};

use anyhow::{Context, Result, bail};

pub use parse::{FileDiff, Hunk, Kind, Line};

use crate::session::Source;

pub struct Repo {
    root: String,
    common_dir: String,
    index: String,
    bare: bool,
    trees: HashMap<String, String>,
    ranges: HashMap<String, Vec<Commit>>,
    diffs: HashMap<(String, String, u32), Vec<FileDiff>>,
    changes: HashMap<(String, String), Option<Vec<FileDiff>>>,
    blobs: Blobs,
}

#[derive(Clone)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
    pub author: String,
    pub when: String,
    pub refs: String,
}

impl Commit {
    fn parse(line: &str) -> Commit {
        let mut fields = line.splitn(5, '\x1f').map(str::to_string);
        let mut next = || fields.next().unwrap_or_default();
        Commit {
            sha: next(),
            author: next(),
            when: next(),
            refs: next(),
            subject: next(),
        }
    }
}

pub struct Blobs {
    root: String,
    bare: bool,
    cat: Option<CatFile>,
}

struct CatFile {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Blobs {
    pub fn read(&mut self, object: &str, max: usize) -> Result<Option<String>> {
        if self.cat.is_none() {
            let mut child = git(&self.root, self.bare, &["cat-file", "--batch"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .context("run git cat-file")?;
            let stdin = child.stdin.take().expect("piped");
            let stdout = BufReader::new(child.stdout.take().expect("piped"));
            self.cat = Some(CatFile { stdin, stdout });
        }
        let cat = self.cat.as_mut().expect("started above");
        writeln!(cat.stdin, "{object}")?;
        cat.stdin.flush()?;
        let mut header = String::new();
        cat.stdout.read_line(&mut header)?;
        if header.ends_with(" missing\n") {
            return Ok(None);
        }
        let size: usize = header
            .trim_end()
            .rsplit(' ')
            .next()
            .and_then(|s| s.parse().ok())
            .with_context(|| format!("unexpected git cat-file output: {header:?}"))?;
        if size > max {
            io::copy(
                &mut (&mut cat.stdout).take(size as u64 + 1),
                &mut io::sink(),
            )?;
            return Ok(None);
        }
        let mut buf = vec![0; size + 1];
        cat.stdout.read_exact(&mut buf)?;
        buf.pop();
        Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
    }
}

impl Repo {
    pub fn open() -> Result<Repo> {
        Repo::at(Path::new("."))
    }

    #[expect(clippy::disallowed_methods, reason = "Repo runs git")]
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
            diffs: HashMap::new(),
            changes: HashMap::new(),
            blobs: Blobs {
                root: root.into(),
                bare: false,
                cat: None,
            },
        })
    }

    pub fn bare(dir: &Path) -> Result<Repo> {
        let dir = fs::canonicalize(dir)?;
        let dir = dir.to_str().context("a path not in UTF-8")?.to_string();
        Ok(Repo {
            blobs: Blobs {
                root: dir.clone(),
                bare: true,
                cat: None,
            },
            root: dir.clone(),
            common_dir: dir,
            index: String::new(),
            bare: true,
            trees: HashMap::new(),
            ranges: HashMap::new(),
            diffs: HashMap::new(),
            changes: HashMap::new(),
        })
    }

    pub fn refresh(&mut self) {
        self.trees.clear();
    }

    pub fn common_dir(&self) -> &Path {
        Path::new(&self.common_dir)
    }

    pub fn keep(&self, id: &str, tree: &str) -> Result<()> {
        run(&mut self.git(&["update-ref", &format!("refs/mrg/{id}/{tree}"), tree]))?;
        Ok(())
    }

    pub(crate) fn forget(&self, id: &str) -> Result<()> {
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
        child.stdin.take().expect("piped").write_all(&refs)?;
        let out = child.wait_with_output()?;
        if !out.status.success() {
            bail!(
                "git update-ref: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    pub fn resolve(&self, range: &str) -> Result<(String, String)> {
        let Some((base, end)) = range.split_once("..") else {
            bail!("range must look like A..END or ..END, got {range:?}");
        };
        let empty = self.empty_tree()?;
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
            self.commit(end)?;
            end.to_string()
        } else {
            self.commit(end)?
        };
        self.verify(&base, &end)?;
        Ok((base, end))
    }

    pub fn verify(&self, base: &str, end: &str) -> Result<()> {
        if base == "staged" {
            if end != "unstaged" {
                bail!("a range from staged ends at unstaged, got {end:?}");
            }
            return Ok(());
        }
        let tip = tip(end);
        let ok = self.ancestor(base, tip)?;
        if !ok && base != self.empty_tree()? {
            bail!("{base} is not an ancestor of {tip} (history rewritten?)");
        }
        Ok(())
    }

    pub fn ancestor(&self, a: &str, b: &str) -> Result<bool> {
        Ok(self
            .git(&["merge-base", "--is-ancestor", a, b])
            .stderr(Stdio::null())
            .status()
            .context("run git")?
            .success())
    }

    pub fn tip_of(&self, end: &str) -> Result<String> {
        self.commit(tip(end))
    }

    pub fn diff(&mut self, base: &str, end: &str, context: u32) -> Result<Vec<FileDiff>> {
        let key = (self.pin(base)?, self.pin(end)?, context);
        if !self.diffs.contains_key(&key) {
            let files = self.diff_of(&[&key.0, &key.1], context, &[])?;
            self.diffs.insert(key.clone(), files);
        }
        Ok(self.diffs[&key].clone())
    }

    pub fn diff_now(&self, base: &str, end: &str, paths: &[String]) -> Result<Vec<FileDiff>> {
        let revs = match (base, end) {
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

    pub fn changes(&mut self, from: &str, to: &str) -> Result<Option<&[FileDiff]>> {
        let key = (from.to_string(), to.to_string());
        if !self.changes.contains_key(&key) {
            let files = match self.diff_of(&[from, to], 0, &[]) {
                Ok(files) => Some(files),
                Err(e) if missing(&e) => None,
                Err(e) => return Err(e),
            };
            self.changes.insert(key.clone(), files);
        }
        Ok(self.changes[&key].as_deref())
    }

    pub fn files(&mut self, rev: &str) -> Result<Vec<String>> {
        let object = self.object(rev)?;
        let out = run(&mut self.git(&["ls-tree", "-r", "-z", &object]))?;
        Ok(out
            .split(|&b| b == 0)
            .filter_map(|entry| {
                let (meta, path) = std::str::from_utf8(entry).ok()?.split_once('\t')?;
                (meta.split(' ').nth(1) == Some("blob")).then(|| path.to_string())
            })
            .collect())
    }

    pub fn read(&mut self, rev: &str, path: &str) -> Result<Option<String>> {
        let object = self.blob(rev, path)?;
        self.blobs.read(&object, usize::MAX)
    }

    pub fn blob(&mut self, rev: &str, path: &str) -> Result<String> {
        let object = format!("{}:{path}", self.object(rev)?);
        if object.contains('\n') {
            bail!("invalid path {path:?}");
        }
        Ok(object)
    }

    pub fn blobs(&self) -> Blobs {
        Blobs {
            root: self.root.clone(),
            bare: self.bare,
            cat: None,
        }
    }

    pub fn pin(&mut self, rev: &str) -> Result<String> {
        if sha(rev) {
            Ok(rev.to_string())
        } else if worktree(rev) {
            self.object(rev)
        } else {
            self.commit(rev)
        }
    }

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

    fn index_tree(&mut self, worktree: bool) -> Result<String> {
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

    pub fn commits(&mut self, base: &str, end: &str) -> Result<Vec<Commit>> {
        if base == "staged" {
            return Ok(Vec::new());
        }
        let range = format!("{base}..{}", self.commit(tip(end))?);
        if !self.ranges.contains_key(&range) {
            let commits = self.log(&["--reverse", &range])?;
            self.ranges.insert(range.clone(), commits);
        }
        Ok(self.ranges[&range].clone())
    }

    pub fn uncommitted(&self) -> Result<Vec<&'static str>> {
        if self.bare {
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

    pub fn recent(&self, from: &str, n: usize) -> Result<Vec<Commit>> {
        self.log(&["-n", &n.to_string(), from])
    }

    fn log(&self, args: &[&str]) -> Result<Vec<Commit>> {
        let mut log = vec![
            "log",
            "--first-parent",
            "--format=%H%x1f%an%x1f%ar%x1f%D%x1f%s",
        ];
        log.extend(args);
        let out = run(&mut self.git(&log))?;
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .map(Commit::parse)
            .collect())
    }

    pub fn parent(&self, commit: &str) -> Result<Option<String>> {
        let parents = text(run(&mut self.git(&["log", "-1", "--format=%P", commit]))?)?;
        Ok(parents
            .split(' ')
            .next()
            .filter(|p| !p.is_empty())
            .map(str::to_string))
    }

    pub fn message(&self, commit: &str) -> Result<String> {
        text(run(&mut self.git(&["log", "-1", "--format=%B", commit]))?)
    }

    pub fn origin(&self) -> Result<String> {
        text(run(&mut self.git(&["remote", "get-url", "origin"]))?)
    }

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

    pub fn merge_base(&self, a: &str, b: &str) -> Result<String> {
        text(run(&mut self.git(&["merge-base", a, b]))?)
    }

    pub fn empty_tree(&self) -> Result<String> {
        text(run(self
            .git(&["hash-object", "-t", "tree", "--stdin"])
            .stdin(Stdio::null()))?)
    }

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

    fn tree(&self, rev: &str) -> Result<String> {
        text(run(&mut self.git(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{tree}}"),
        ]))?)
    }

    fn git(&self, args: &[&str]) -> Command {
        git(&self.root, self.bare, args)
    }
}

#[expect(clippy::disallowed_methods, reason = "Repo runs git")]
fn git(root: &str, bare: bool, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root).args(args);
    if bare {
        // GIT_DIR: a broken repo there fails, rather than git finding one around it.
        cmd.env("GIT_DIR", root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
    }
    cmd
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

pub fn worktree(rev: &str) -> bool {
    matches!(rev, "staged" | "unstaged")
}

pub fn tracking(branch: &str) -> String {
    format!("refs/remotes/origin/{branch}")
}

fn tip(end: &str) -> &str {
    if worktree(end) { "HEAD" } else { end }
}

pub fn sha(rev: &str) -> bool {
    rev.len() >= 40 && rev.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn short(rev: &str) -> &str {
    if sha(rev) { &rev[..7] } else { rev }
}

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

pub fn numbered(files: &[FileDiff]) -> String {
    let num = |n: Option<u32>| n.map_or(String::new(), |n| n.to_string());
    let mut out = String::new();
    for f in files {
        if f.old_path == f.new_path {
            out.push_str(&format!("{} {}\n", f.status, f.new_path));
        } else {
            out.push_str(&format!("{} {} -> {}\n", f.status, f.old_path, f.new_path));
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
                out.push_str(&format!("{:>5} {:>5} {sign} {}\n", num(o), num(n), l.text));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_lines() {
        let c = Commit::parse("abc\x1fAnn\x1f2 days ago\x1fHEAD -> main, tag: v1\x1ffix a\x1fb");
        assert_eq!(c.sha, "abc");
        assert_eq!(c.author, "Ann");
        assert_eq!(c.when, "2 days ago");
        assert_eq!(c.refs, "HEAD -> main, tag: v1");
        assert_eq!(c.subject, "fix a\x1fb");
        let c = Commit::parse("abc\x1fAnn\x1f2 days ago\x1f\x1ffix");
        assert_eq!(c.refs, "");
        assert_eq!(c.subject, "fix");
    }

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
