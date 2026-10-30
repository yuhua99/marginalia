use anyhow::{Context, Result, bail};

#[derive(Debug, PartialEq)]
pub struct FileDiff {
    /// Status letter from `git diff --raw`: A, D, M, R, T...
    pub status: char,
    pub old_path: String,
    pub new_path: String,
    pub binary: bool,
    /// Untracked file over the size limit; no hunks.
    pub skipped: bool,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, PartialEq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_len: u32,
    pub new_start: u32,
    pub new_len: u32,
    pub lines: Vec<Line>,
}

impl Hunk {
    /// The first line of the old and the new side; an empty side's start is the line
    /// before the change.
    pub fn firsts(&self) -> (u32, u32) {
        let first = |start, len| if len == 0 { start + 1 } else { start };
        (
            first(self.old_start, self.old_len),
            first(self.new_start, self.new_len),
        )
    }
}

#[derive(Debug, PartialEq)]
pub struct Line {
    pub kind: Kind,
    pub text: String,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Kind {
    Ctx,
    Del,
    Add,
}

impl FileDiff {
    pub fn skipped(path: &str) -> FileDiff {
        FileDiff {
            status: 'A',
            old_path: path.into(),
            new_path: path.into(),
            binary: false,
            skipped: true,
            hunks: Vec::new(),
        }
    }
}

/// Parses `git diff --raw -z --patch`.
///
/// Paths and status come from the NUL-separated raw records, so the patch headers
/// (quoting, prefixes) are never interpreted. Each raw record has one patch block
/// (two for a type change: deletion then addition), and hunk bodies are read by their declared line counts, so content lines that
/// look like `--- a/x` or `+++ b/x` stay content.
pub fn parse(out: &[u8]) -> Result<Vec<FileDiff>> {
    let mut files = Vec::new();
    let mut rest = out;
    while rest.first() == Some(&b':') {
        let (meta, r) = take_nul(rest)?;
        let status = meta
            .rsplit(' ')
            .next()
            .unwrap()
            .chars()
            .next()
            .context("empty raw status")?;
        let (old_path, r) = take_nul(r)?;
        let (new_path, r) = if matches!(status, 'R' | 'C') {
            take_nul(r)?
        } else {
            (old_path.clone(), r)
        };
        files.push(FileDiff {
            status,
            old_path,
            new_path,
            binary: false,
            skipped: false,
            hunks: Vec::new(),
        });
        rest = r;
    }
    // An extra NUL separates the raw records from the patch.
    let patch = rest.strip_prefix(b"\0").unwrap_or(rest);
    let mut lines = patch.split(|&b| b == b'\n').peekable();

    for f in &mut files {
        for _ in 0..if f.status == 'T' { 2 } else { 1 } {
            if !lines.next().is_some_and(|l| l.starts_with(b"diff --git ")) {
                bail!("missing patch for {}", f.new_path);
            }
            while let Some(l) =
                lines.next_if(|l| !l.starts_with(b"@@ ") && !l.starts_with(b"diff --git "))
            {
                f.binary |= l.starts_with(b"Binary files ");
            }
            while let Some(header) = lines.next_if(|l| l.starts_with(b"@@ ")) {
                let mut hunk = parse_header(header)?;
                let (mut old, mut new) = (hunk.old_len, hunk.new_len);
                while old > 0 || new > 0 {
                    let line = lines
                        .next()
                        .with_context(|| format!("truncated hunk in {}", f.new_path))?;
                    let kind = match line.first() {
                        // diff.suppressBlankEmpty drops the space of empty context lines
                        None | Some(b' ') => Kind::Ctx,
                        Some(b'-') => Kind::Del,
                        Some(b'+') => Kind::Add,
                        Some(b'\\') => continue,
                        Some(_) => bail!(
                            "unexpected hunk line in {}: {}",
                            f.new_path,
                            String::from_utf8_lossy(line)
                        ),
                    };
                    let over = || format!("hunk longer than declared in {}", f.new_path);
                    if kind != Kind::Add {
                        old = old.checked_sub(1).with_context(over)?;
                    }
                    if kind != Kind::Del {
                        new = new.checked_sub(1).with_context(over)?;
                    }
                    let text =
                        String::from_utf8_lossy(line.get(1..).unwrap_or_default()).into_owned();
                    hunk.lines.push(Line { kind, text });
                }
                // "\ No newline at end of file" after the last line
                while lines.next_if(|l| l.starts_with(b"\\")).is_some() {}
                f.hunks.push(hunk);
            }
        }
    }
    if lines.any(|l| !l.is_empty()) {
        bail!("more patches than raw records");
    }
    Ok(files)
}

fn take_nul(s: &[u8]) -> Result<(String, &[u8])> {
    let i = s
        .iter()
        .position(|&b| b == 0)
        .context("truncated raw record")?;
    Ok((String::from_utf8_lossy(&s[..i]).into_owned(), &s[i + 1..]))
}

/// `@@ -a[,b] +c[,d] @@ ...`
fn parse_header(line: &[u8]) -> Result<Hunk> {
    let line = String::from_utf8_lossy(line);
    let bad = || format!("bad hunk header: {line}");
    let mut parts = line.split(' ').skip(1);
    let (old_start, old_len) =
        parse_range(parts.next().and_then(|s| s.strip_prefix('-'))).with_context(bad)?;
    let (new_start, new_len) =
        parse_range(parts.next().and_then(|s| s.strip_prefix('+'))).with_context(bad)?;
    Ok(Hunk {
        old_start,
        old_len,
        new_start,
        new_len,
        lines: Vec::new(),
    })
}

fn parse_range(s: Option<&str>) -> Option<(u32, u32)> {
    let s = s?;
    match s.split_once(',') {
        Some((start, len)) => Some((start.parse().ok()?, len.parse().ok()?)),
        None => Some((s.parse().ok()?, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: Kind, text: &str) -> Line {
        Line {
            kind,
            text: text.into(),
        }
    }

    fn hunk(old: (u32, u32), new: (u32, u32), lines: Vec<Line>) -> Hunk {
        Hunk {
            old_start: old.0,
            old_len: old.1,
            new_start: new.0,
            new_len: new.1,
            lines,
        }
    }

    /// Raw records followed by the patch, as `git diff --raw -z --patch` prints them.
    fn out(raw: &[&str], patch: &str) -> Vec<u8> {
        let mut v = Vec::new();
        for r in raw {
            v.extend_from_slice(r.as_bytes());
            v.push(0);
        }
        v.push(0);
        v.extend_from_slice(patch.as_bytes());
        v
    }

    #[test]
    fn empty_output() {
        assert_eq!(parse(b"").unwrap(), vec![]);
    }

    #[test]
    fn real_git_output() {
        // Captured from git 2.34: added, binary, rename with edit, header-like content
        // with no newline at end, deleted non-ASCII path.
        let o = out(
            &[
                ":000000 100644 0000000 3e75765 A\0add.txt",
                ":100644 100644 87ae6b6 22f6b3b M\0b.bin",
                ":100644 100644 f00c965 0e5ea2a R081\0old.txt\0new.txt",
                ":100644 100644 de98044 4a45b12 M\0x y.txt",
                ":100644 000000 b680253 0000000 D\0ü.txt",
            ],
            "diff --git a/add.txt b/add.txt\nnew file mode 100644\nindex 0000000..3e75765\n--- /dev/null\n+++ b/add.txt\n@@ -0,0 +1 @@\n+new\n\
diff --git a/b.bin b/b.bin\nindex 87ae6b6..22f6b3b 100644\nBinary files a/b.bin and b/b.bin differ\n\
diff --git a/old.txt b/new.txt\nsimilarity index 81%\nrename from old.txt\nrename to new.txt\nindex f00c965..0e5ea2a 100644\n--- a/old.txt\n+++ b/new.txt\n@@ -7,4 +7,4 @@\n 7\n 8\n 9\n-10\n+TEN\n\
diff --git a/x y.txt b/x y.txt\nindex de98044..4a45b12 100644\n--- a/x y.txt\t\n+++ b/x y.txt\t\n@@ -1,3 +1,3 @@\n a\n-b\n-c\n+--- b\n+c\n\\ No newline at end of file\n\
diff --git a/ü.txt b/ü.txt\ndeleted file mode 100644\nindex b680253..0000000\n--- a/ü.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-z\n",
        );
        let files = parse(&o).unwrap();
        let summary: Vec<_> = files
            .iter()
            .map(|f| (f.status, f.old_path.as_str(), f.new_path.as_str(), f.binary))
            .collect();
        assert_eq!(
            summary,
            vec![
                ('A', "add.txt", "add.txt", false),
                ('M', "b.bin", "b.bin", true),
                ('R', "old.txt", "new.txt", false),
                ('M', "x y.txt", "x y.txt", false),
                ('D', "ü.txt", "ü.txt", false),
            ]
        );
        assert_eq!(
            files[0].hunks,
            vec![hunk((0, 0), (1, 1), vec![line(Kind::Add, "new")])]
        );
        assert!(files[1].hunks.is_empty());
        assert_eq!(files[2].hunks[0].lines.len(), 5);
        assert_eq!(
            files[3].hunks,
            vec![hunk(
                (1, 3),
                (1, 3),
                vec![
                    line(Kind::Ctx, "a"),
                    line(Kind::Del, "b"),
                    line(Kind::Del, "c"),
                    line(Kind::Add, "--- b"),
                    line(Kind::Add, "c"),
                ]
            )]
        );
        assert_eq!(
            files[4].hunks,
            vec![hunk((1, 1), (0, 0), vec![line(Kind::Del, "z")])]
        );
    }

    #[test]
    fn header_like_content_and_multiple_hunks() {
        let o = out(
            &[":100644 100644 1 2 M\0a"],
            "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@\n---- a/x\n++++ b/x\n diff --git a/q b/q\n@@ -10 +10 @@ ctx\n-old\n+new\n",
        );
        let f = &parse(&o).unwrap()[0];
        assert_eq!(
            f.hunks,
            vec![
                hunk(
                    (1, 2),
                    (1, 2),
                    vec![
                        line(Kind::Del, "--- a/x"),
                        line(Kind::Add, "+++ b/x"),
                        line(Kind::Ctx, "diff --git a/q b/q")
                    ]
                ),
                hunk(
                    (10, 1),
                    (10, 1),
                    vec![line(Kind::Del, "old"), line(Kind::Add, "new")]
                ),
            ]
        );
    }

    #[test]
    fn no_newline_marker_mid_hunk_and_blank_context() {
        let o = out(
            &[":100644 100644 1 2 M\0a"],
            "diff --git a/a b/a\n@@ -1,3 +1,4 @@\n x\n\n-y\n\\ No newline at end of file\n+y\n+z\n",
        );
        let f = &parse(&o).unwrap()[0];
        assert_eq!(
            f.hunks[0].lines,
            vec![
                line(Kind::Ctx, "x"),
                line(Kind::Ctx, ""),
                line(Kind::Del, "y"),
                line(Kind::Add, "y"),
                line(Kind::Add, "z")
            ]
        );
    }

    #[test]
    fn mode_only_change_has_no_hunks() {
        let o = out(
            &[":100644 100755 1 1 M\0a", ":100644 100644 1 2 M\0b"],
            "diff --git a/a b/a\nold mode 100644\nnew mode 100755\ndiff --git a/b b/b\n@@ -1 +1 @@\n-1\n+2\n",
        );
        let files = parse(&o).unwrap();
        assert!(files[0].hunks.is_empty());
        assert_eq!(files[1].hunks.len(), 1);
    }

    #[test]
    fn type_change_has_two_blocks() {
        let o = out(
            &[":100644 120000 1 2 T\0a", ":100644 100644 1 2 M\0b"],
            "diff --git a/a b/a\ndeleted file mode 100644\n@@ -1 +0,0 @@\n-x\n\
diff --git a/a b/a\nnew file mode 120000\n@@ -0,0 +1 @@\n+target\n\\ No newline at end of file\n\
diff --git a/b b/b\n@@ -1 +1 @@\n-1\n+2\n",
        );
        let files = parse(&o).unwrap();
        assert_eq!(
            files[0].hunks,
            vec![
                hunk((1, 1), (0, 0), vec![line(Kind::Del, "x")]),
                hunk((0, 0), (1, 1), vec![line(Kind::Add, "target")])
            ]
        );
        assert_eq!(files[1].hunks.len(), 1);
    }

    #[test]
    fn rejects_truncated_hunk() {
        let o = out(
            &[":100644 100644 1 2 M\0a"],
            "diff --git a/a b/a\n@@ -1,3 +1,3 @@\n x\n",
        );
        assert!(parse(&o).is_err());
    }

    #[test]
    fn rejects_hunk_longer_than_declared() {
        let o = out(
            &[":100644 100644 1 2 M\0a"],
            "diff --git a/a b/a\n@@ -1 +1,2 @@\n-x\n-y\n+z\n",
        );
        assert!(parse(&o).is_err());
    }

    #[test]
    fn rejects_malformed_range() {
        let o = out(
            &[":100644 100644 1 2 M\0a"],
            "diff --git a/a b/a\n@@ -x +1 @@\n+z\n",
        );
        assert!(parse(&o).is_err());
    }

    #[test]
    fn rejects_raw_patch_count_mismatch() {
        let missing = out(
            &[":100644 100644 1 2 M\0a", ":100644 100644 1 2 M\0b"],
            "diff --git a/a b/a\n@@ -1 +1 @@\n-1\n+2\n",
        );
        assert!(parse(&missing).is_err());
        let extra = out(
            &[":100644 100644 1 2 M\0a"],
            "diff --git a/a b/a\n@@ -1 +1 @@\n-1\n+2\ndiff --git a/b b/b\n",
        );
        assert!(parse(&extra).is_err());
    }
}
