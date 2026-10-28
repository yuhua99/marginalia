use std::collections::HashMap;
use std::ops::{Range, RangeInclusive};

use anyhow::Result;
use similar::{ChangeTag, TextDiff};

use crate::repo::{FileDiff, Kind};
use crate::session::{Anchor, Owner, Thread};

/// The logical rows of a side-by-side diff, with threads placed under what they anchor to.
pub struct Doc {
    /// Rev of the left (old) and right (new) side.
    left_rev: String,
    right_rev: String,
    /// (old_path, new_path) per file.
    paths: Vec<(String, String)>,
    /// Per file, added or deleted: only one side has lines.
    one_sided: Vec<bool>,
    rows: Vec<Row>,
    /// One whole file in one column (`Doc::file`), showing only the threads on it.
    single: bool,
    /// The commits shown, oldest first: shas, or the range end for uncommitted changes.
    /// Empty for the whole range, which places lines by their rev and shows every thread;
    /// otherwise lines go by their owner, and threads of other commits are left out.
    commits: Vec<String>,
    /// Per collapsed file, its rows other than the header; its threads are left out.
    collapsed: HashMap<usize, Vec<Row>>,
}

pub enum Row {
    /// The whole session; always the first row.
    Session,
    File {
        file: usize,
        status: char,
        path: String,
        note: Option<&'static str>,
    },
    Code {
        file: usize,
        left: Option<Side>,
        right: Option<Side>,
    },
    /// Unchanged lines starting at `old` / `new`; `hidden: None` after the last
    /// hunk, where the file may or may not go on.
    Fold {
        file: usize,
        old: u32,
        new: u32,
        hidden: Option<u32>,
    },
    /// Index into the session threads. Threads that have no row in the diff
    /// (session, commits, lines in folds or of other revs) come right after `Session`;
    /// those of collapsed files have none.
    Thread { idx: usize },
}

pub struct Side {
    pub lineno: u32,
    pub kind: Kind,
    pub text: String,
    /// Byte ranges of `text` that differ from the paired line on the other side.
    pub emph: Ranges,
}

type Ranges = Vec<Range<usize>>;

/// Where a thread row goes.
#[derive(Hash, PartialEq, Eq)]
enum Spot {
    Top,
    /// At the top in every view: the session, a commit shown, or its lines that have no
    /// side here.
    Whole,
    File(usize),
    /// File, right side, last anchored line.
    Line(usize, bool, u32),
}

impl Doc {
    pub fn new(
        left_rev: String,
        right_rev: String,
        files: Vec<FileDiff>,
        commits: Vec<String>,
    ) -> Doc {
        let mut rows = vec![Row::Session];
        let mut paths = Vec::new();
        let mut one_sided = Vec::new();
        for (file, f) in files.into_iter().enumerate() {
            one_sided.push(matches!(f.status, 'A' | 'D'));
            let path = if f.old_path == f.new_path {
                f.new_path.clone()
            } else {
                format!("{} -> {}", f.old_path, f.new_path)
            };
            let note = if f.skipped {
                Some("too large, skipped")
            } else if f.binary {
                Some("binary")
            } else {
                None
            };
            rows.push(Row::File {
                file,
                status: f.status,
                path,
                note,
            });
            paths.push((f.old_path, f.new_path));
            let (mut next_old, mut next_new) = (1, 1);
            // The file may go on after a hunk ending in full (-U3) context.
            let mut more = false;
            for h in f.hunks {
                let (mut old, mut new) =
                    (first(h.old_start, h.old_len), first(h.new_start, h.new_len));
                if old > next_old {
                    rows.push(Row::Fold {
                        file,
                        old: next_old,
                        new: next_new,
                        hidden: Some(old - next_old),
                    });
                }
                next_old = old + h.old_len;
                next_new = new + h.new_len;
                more = h
                    .lines
                    .iter()
                    .rev()
                    .take_while(|l| l.kind == Kind::Ctx)
                    .count()
                    >= 3;
                // Consecutive deletions followed by additions are paired up row by row.
                let (mut dels, mut adds) = (Vec::new(), Vec::new());
                for line in h.lines {
                    match line.kind {
                        Kind::Del => {
                            if !adds.is_empty() {
                                pair(&mut rows, file, &mut dels, &mut adds);
                            }
                            dels.push(Side {
                                lineno: old,
                                kind: Kind::Del,
                                text: line.text,
                                emph: Vec::new(),
                            });
                            old += 1;
                        }
                        Kind::Add => {
                            adds.push(Side {
                                lineno: new,
                                kind: Kind::Add,
                                text: line.text,
                                emph: Vec::new(),
                            });
                            new += 1;
                        }
                        Kind::Ctx => {
                            pair(&mut rows, file, &mut dels, &mut adds);
                            let left = Side {
                                lineno: old,
                                kind: Kind::Ctx,
                                text: line.text.clone(),
                                emph: Vec::new(),
                            };
                            let right = Side {
                                lineno: new,
                                kind: Kind::Ctx,
                                text: line.text,
                                emph: Vec::new(),
                            };
                            rows.push(Row::Code {
                                file,
                                left: Some(left),
                                right: Some(right),
                            });
                            old += 1;
                            new += 1;
                        }
                    }
                }
                pair(&mut rows, file, &mut dels, &mut adds);
            }
            if more {
                rows.push(Row::Fold {
                    file,
                    old: next_old,
                    new: next_new,
                    hidden: None,
                });
            }
        }
        Doc {
            left_rev,
            right_rev,
            paths,
            one_sided,
            rows,
            single: false,
            commits,
            collapsed: HashMap::new(),
        }
    }

    /// `path` at `rev` with content `text`, every line as a right side.
    pub fn file(rev: String, path: String, text: &str) -> Doc {
        let header = Row::File {
            file: 0,
            status: ' ',
            path: path.clone(),
            note: None,
        };
        let lines = text.lines().enumerate().map(|(i, line)| Row::Code {
            file: 0,
            left: None,
            right: Some(Side {
                lineno: i as u32 + 1,
                kind: Kind::Ctx,
                text: line.to_string(),
                emph: Vec::new(),
            }),
        });
        Doc {
            left_rev: rev.clone(),
            right_rev: rev,
            paths: vec![(path.clone(), path)],
            one_sided: vec![true],
            rows: [header].into_iter().chain(lines).collect(),
            single: true,
            commits: Vec::new(),
            collapsed: HashMap::new(),
        }
    }

    pub fn one_sided(&self, file: usize) -> bool {
        self.one_sided[file]
    }

    /// The row of right-side line `line`, or the last row.
    pub fn line_row(&self, line: u32) -> usize {
        self.rows
            .iter()
            .position(|r| matches!(r, Row::Code { right: Some(s), .. } if s.lineno == line))
            .unwrap_or(self.rows.len() - 1)
    }

    /// Replaces the thread rows.
    pub fn set_threads(&mut self, threads: &[Thread]) {
        let mut at: HashMap<Spot, Vec<usize>> = HashMap::new();
        for (idx, t) in threads.iter().enumerate() {
            at.entry(self.spot(&t.anchor)).or_default().push(idx);
        }
        let thread_row = |idx: usize| Row::Thread { idx };
        let old = std::mem::take(&mut self.rows);
        let mut rows = Vec::with_capacity(old.len() + threads.len());
        for row in old {
            let spots = match &row {
                Row::File { file, .. } if self.collapsed.contains_key(file) => [None, None],
                Row::File { file, .. } => [Some(Spot::File(*file)), None],
                Row::Code { file, left, right } => [
                    left.as_ref().map(|s| Spot::Line(*file, false, s.lineno)),
                    right.as_ref().map(|s| Spot::Line(*file, true, s.lineno)),
                ],
                Row::Thread { .. } => continue,
                _ => [None, None],
            };
            rows.push(row);
            for spot in spots.into_iter().flatten() {
                rows.extend(at.remove(&spot).into_iter().flatten().map(thread_row));
            }
        }
        if !self.single {
            // Left over are threads in folds, `Whole` ones, `Top` ones (whole range only),
            // and those of collapsed files, which stay out.
            let mut rest: Vec<usize> = at
                .into_iter()
                .filter(|(spot, _)| self.commits.is_empty() || *spot != Spot::Top)
                .filter(|(spot, _)| {
                    !matches!(spot, Spot::File(f) | Spot::Line(f, ..) if self.collapsed.contains_key(f))
                })
                .flat_map(|(_, idx)| idx)
                .collect();
            rest.sort();
            rows.splice(1..1, rest.into_iter().map(thread_row));
        }
        self.rows = rows;
    }

    fn spot(&self, anchor: &Anchor) -> Spot {
        let find = |f: &dyn Fn(&(String, String)) -> bool| self.paths.iter().position(f);
        match anchor {
            Anchor::Lines { owner, .. } if !self.commits.is_empty() => {
                let Some(o) = owner else {
                    return Spot::Top;
                };
                let Some(i) = self.commits.iter().position(|c| *c == o.commit) else {
                    return Spot::Top;
                };
                // A side shows deleted lines of the oldest commit, the others of the newest.
                if i != if o.deleted { 0 } else { self.commits.len() - 1 } {
                    return Spot::Whole;
                }
                let right = !o.deleted;
                match find(&|p| if right { p.1 == o.file } else { p.0 == o.file }) {
                    Some(f) => Spot::Line(f, right, o.line),
                    None => Spot::Whole,
                }
            }
            Anchor::Lines {
                rev: Some(rev),
                file,
                end,
                ..
            } => {
                let right = *rev == self.right_rev;
                if !right && *rev != self.left_rev {
                    return Spot::Top;
                }
                match find(&|p| if right { p.1 == *file } else { p.0 == *file }) {
                    Some(i) => Spot::Line(i, right, *end),
                    None => Spot::Top,
                }
            }
            Anchor::File { file } => match find(&|p| p.0 == *file || p.1 == *file) {
                Some(i) => Spot::File(i),
                None => Spot::Top,
            },
            Anchor::Session => Spot::Whole,
            Anchor::Commit { commit } if self.commits.contains(commit) => Spot::Whole,
            _ => Spot::Top,
        }
    }

    /// The anchor for a `c` on these rows: their code lines, preferring the right
    /// (new) side; otherwise the file of the first row, or the session.
    pub fn anchor_at(&self, rows: RangeInclusive<usize>) -> Anchor {
        let rows = &self.rows[rows];
        let code: Vec<_> = rows
            .iter()
            .filter_map(|r| match r {
                Row::Code { file, left, right } => Some((*file, left, right)),
                _ => None,
            })
            .collect();
        if let Some(&(file, ..)) = code.first() {
            let code = code.iter().filter(|c| c.0 == file);
            let right: Vec<u32> = code
                .clone()
                .filter_map(|c| c.2.as_ref().map(|s| s.lineno))
                .collect();
            let deleted = right.is_empty();
            let (rev, path, lines) = if deleted {
                let left = code
                    .filter_map(|c| c.1.as_ref().map(|s| s.lineno))
                    .collect();
                (&self.left_rev, &self.paths[file].0, left)
            } else {
                (&self.right_rev, &self.paths[file].1, right)
            };
            let end = *lines.iter().max().unwrap();
            // A commit's own view knows whose lines these are, context lines too.
            let owner = match &self.commits[..] {
                [commit] => Some(Owner {
                    commit: commit.clone(),
                    deleted,
                    file: path.clone(),
                    line: end,
                }),
                _ => None,
            };
            return Anchor::Lines {
                rev: Some(rev.clone()),
                file: path.clone(),
                start: *lines.iter().min().unwrap(),
                end,
                owner,
            };
        }
        match rows.first() {
            Some(Row::File { file, .. } | Row::Fold { file, .. }) => Anchor::File {
                file: self.paths[*file].1.clone(),
            },
            _ => Anchor::Session,
        }
    }

    /// Shows up to 20 lines of the fold at `row` (the ten at each end of a longer fold),
    /// reading the file with `read(rev, path)`. Returns the row the fold was at, which
    /// shifts when threads move out of the top group into the shown lines.
    pub fn expand(
        &mut self,
        row: usize,
        threads: &[Thread],
        read: impl FnOnce(&str, &str) -> Result<Option<String>>,
    ) -> Result<usize> {
        let Row::Fold {
            file,
            old,
            new,
            hidden,
        } = self.rows[row]
        else {
            return Ok(row);
        };
        let Some(text) = read(&self.right_rev, &self.paths[file].1)? else {
            return Ok(row);
        };
        let lines: Vec<&str> = text.lines().skip(new as usize - 1).collect();
        let hidden = hidden.map_or(lines.len(), |h| h as usize).min(lines.len());
        let code = |k: usize| {
            let side = |lineno| Side {
                lineno,
                kind: Kind::Ctx,
                text: lines[k].to_string(),
                emph: Vec::new(),
            };
            Row::Code {
                file,
                left: Some(side(old + k as u32)),
                right: Some(side(new + k as u32)),
            }
        };
        let shown: Vec<Row> = if hidden <= 20 {
            (0..hidden).map(code).collect()
        } else {
            let fold = Row::Fold {
                file,
                old: old + 10,
                new: new + 10,
                hidden: Some(hidden as u32 - 20),
            };
            (0..10)
                .map(code)
                .chain([fold])
                .chain((hidden - 10..hidden).map(code))
                .collect()
        };
        let top = self.top_threads();
        self.rows.splice(row..=row, shown);
        self.set_threads(threads);
        Ok(row - (top - self.top_threads()))
    }

    /// Whether `row` is in the top thread group, away from what it anchors to.
    pub fn at_top(&self, row: usize) -> bool {
        !self.single && row <= self.top_threads()
    }

    /// Number of thread rows in the top group.
    fn top_threads(&self) -> usize {
        self.rows[1..]
            .iter()
            .take_while(|r| matches!(r, Row::Thread { .. }))
            .count()
    }

    /// The next (or previous) thread row.
    pub fn next_thread(&self, from: usize, forward: bool) -> Option<usize> {
        self.find(from, forward, |r| matches!(r, Row::Thread { .. }))
    }

    pub fn collapsed(&self, file: usize) -> bool {
        self.collapsed.contains_key(&file)
    }

    /// Collapses `file` to its header. Returns the header row.
    pub fn collapse_file(&mut self, file: usize, threads: &[Thread]) -> usize {
        let row = self.file_row(file);
        let end = self.next_file(row, true).unwrap_or(self.rows.len());
        let rows = self
            .rows
            .drain(row + 1..end)
            .filter(|r| !matches!(r, Row::Thread { .. }))
            .collect();
        self.collapsed.insert(file, rows);
        self.set_threads(threads);
        self.file_row(file)
    }

    /// Expands `file` if collapsed. Returns the header row.
    pub fn expand_file(&mut self, file: usize, threads: &[Thread]) -> usize {
        let row = self.file_row(file);
        if let Some(rows) = self.collapsed.remove(&file) {
            self.rows.splice(row + 1..row + 1, rows);
            self.set_threads(threads);
        }
        self.file_row(file)
    }

    /// The next (or previous) file header row.
    pub fn next_file(&self, from: usize, forward: bool) -> Option<usize> {
        self.find(from, forward, |r| matches!(r, Row::File { .. }))
    }

    fn find(&self, from: usize, forward: bool, f: impl Fn(&Row) -> bool) -> Option<usize> {
        let f = |&i: &usize| f(&self.rows[i]);
        if forward {
            (from + 1..self.rows.len()).find(f)
        } else {
            (0..from).rev().find(f)
        }
    }

    /// The file `row` is in: the nearest file header at or above it.
    pub fn file_at(&self, row: usize) -> Option<usize> {
        self.rows.iter().take(row + 1).rev().find_map(|r| match r {
            Row::File { file, .. } => Some(*file),
            _ => None,
        })
    }

    /// The header row of `file`.
    pub fn file_row(&self, file: usize) -> usize {
        self.rows
            .iter()
            .position(|r| matches!(r, Row::File { file: f, .. } if *f == file))
            .unwrap_or(0)
    }

    /// Revs of the left and right side.
    pub fn revs(&self) -> (&str, &str) {
        (&self.left_rev, &self.right_rev)
    }

    /// (old_path, new_path) per file.
    pub fn paths(&self) -> &[(String, String)] {
        &self.paths
    }

    /// The file `anchor` is on in this view.
    pub fn file_of(&self, anchor: &Anchor) -> Option<usize> {
        match self.spot(anchor) {
            Spot::File(f) | Spot::Line(f, ..) => Some(f),
            _ => None,
        }
    }

    /// Thread indices in row order, those of a collapsed file at its header,
    /// then those hidden in this view.
    pub fn thread_order(&self, threads: &[Thread]) -> Vec<usize> {
        let mut order: Vec<usize> = Vec::new();
        for r in &self.rows {
            match r {
                Row::Thread { idx } => order.push(*idx),
                Row::File { file, .. } if self.collapsed(*file) => order.extend(
                    (0..threads.len()).filter(|&i| self.file_of(&threads[i].anchor) == Some(*file)),
                ),
                _ => {}
            }
        }
        let hidden: Vec<usize> = (0..threads.len()).filter(|i| !order.contains(i)).collect();
        order.extend(hidden);
        order
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Up to `n` rows starting at `start`.
    pub fn window(&self, start: usize, n: usize) -> &[Row] {
        let start = start.min(self.rows.len());
        &self.rows[start..(start + n).min(self.rows.len())]
    }
}

/// First line of a hunk side; an empty side's start is the line before the change.
fn first(start: u32, len: u32) -> u32 {
    if len == 0 { start + 1 } else { start }
}

fn pair(rows: &mut Vec<Row>, file: usize, dels: &mut Vec<Side>, adds: &mut Vec<Side>) {
    let mut dels = dels.drain(..);
    let mut adds = adds.drain(..);
    loop {
        let (mut left, mut right) = (dels.next(), adds.next());
        if left.is_none() && right.is_none() {
            return;
        }
        if let (Some(l), Some(r)) = (&mut left, &mut right)
            && let Some((old, new)) = word_diff(&l.text, &r.text)
        {
            l.emph = old;
            r.emph = new;
        }
        rows.push(Row::Code { file, left, right });
    }
}

/// Changed word ranges of a paired old and new line; None when the lines are long
/// or mostly different, where highlighting words would only add noise.
fn word_diff(old: &str, new: &str) -> Option<(Ranges, Ranges)> {
    if old.len() > 1000 || new.len() > 1000 {
        return None;
    }
    let diff = TextDiff::from_unicode_words(old, new);
    if diff.ratio() < 0.5 {
        return None;
    }
    let (mut o, mut n) = (Vec::new(), Vec::new());
    let (mut oi, mut ni) = (0, 0);
    for change in diff.iter_all_changes() {
        let len = change.value().len();
        match change.tag() {
            ChangeTag::Equal => {
                oi += len;
                ni += len;
            }
            ChangeTag::Delete => {
                o.push(oi..oi + len);
                oi += len;
            }
            ChangeTag::Insert => {
                n.push(ni..ni + len);
                ni += len;
            }
        }
    }
    Some((o, n))
}
