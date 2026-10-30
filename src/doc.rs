use std::collections::HashMap;
use std::ops::{Range, RangeInclusive};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use similar::{ChangeTag, TextDiff};

use crate::repo::{FileDiff, Kind};
use crate::session::{Anchor, Place, Thread};

/// The logical rows of a side-by-side diff, with threads placed under what they anchor to.
pub struct Doc {
    /// Rev of the left (old) and right (new) side.
    left_rev: String,
    right_rev: String,
    /// (old_path, new_path) per file.
    paths: Vec<(String, String)>,
    /// Status letter per file, as in `FileDiff::status`.
    statuses: Vec<char>,
    /// Per file, added or deleted: only one side has lines.
    one_sided: Vec<bool>,
    rows: Vec<Row>,
    scope: Scope,
    /// Per collapsed file, its rows other than the header; its threads are left out.
    collapsed: HashMap<usize, Vec<Row>>,
    /// Where each of the session threads is here, and so goes, as of `set_threads`.
    places: Vec<Option<Place>>,
    spots: Vec<Spot>,
    /// Changes with the rows, and is no other doc's.
    version: u64,
}

/// The next `Doc::version`.
fn next_version() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// What the diff shows.
#[derive(Clone, Copy, PartialEq)]
pub enum Scope {
    /// The whole range: every thread shows, at the top when on no row.
    Range,
    /// A run of commits: only threads on its rows, and those on all of it.
    Run,
}

pub enum Row {
    /// The whole session; always the first row.
    Session,
    /// The commit shown, when it is the only one: its sha and full message.
    Commit { sha: String, message: String },
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
    /// Index into the session threads. Threads on lines in a fold come after the fold;
    /// those on the commit come after `Commit`; those that have no row in the diff
    /// (session, lines of other revs) come right after `Session`; those of collapsed
    /// files have none.
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
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
enum Spot {
    /// At the top of the whole range only: on no side here.
    Top,
    /// At the top in every view: the session.
    Whole,
    /// Under `Row::Commit`: the commit shown alone.
    Commit,
    /// Nowhere: a commit not shown alone.
    Hidden,
    File(usize),
    /// File, right side, last anchored line.
    Line(usize, bool, u32),
}

impl Doc {
    /// `commit`: (sha, full message) of the commit shown, when it is the only one.
    pub fn new(
        left_rev: String,
        right_rev: String,
        files: Vec<FileDiff>,
        scope: Scope,
        commit: Option<(String, String)>,
    ) -> Doc {
        let mut rows = vec![Row::Session];
        rows.extend(commit.map(|(sha, message)| Row::Commit { sha, message }));
        let mut paths = Vec::new();
        let mut statuses = Vec::new();
        let mut one_sided = Vec::new();
        for (file, f) in files.into_iter().enumerate() {
            one_sided.push(matches!(f.status, 'A' | 'D'));
            let path = if f.old_path == f.new_path {
                f.new_path.clone()
            } else {
                format!("{} -> {}", f.old_path, f.new_path)
            };
            let note = f.binary.then_some("binary");
            rows.push(Row::File {
                file,
                status: f.status,
                path,
                note,
            });
            paths.push((f.old_path, f.new_path));
            statuses.push(f.status);
            let (mut next_old, mut next_new) = (1, 1);
            // The file may go on after a hunk ending in full (-U3) context.
            let mut more = false;
            for h in f.hunks {
                let (mut old, mut new) = h.firsts();
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
            statuses,
            one_sided,
            rows,
            scope,
            collapsed: HashMap::new(),
            places: Vec::new(),
            spots: Vec::new(),
            version: next_version(),
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
            statuses: vec![' '],
            one_sided: vec![true],
            rows: [header].into_iter().chain(lines).collect(),
            scope: Scope::Range,
            collapsed: HashMap::new(),
            places: Vec::new(),
            spots: Vec::new(),
            version: next_version(),
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

    /// Replaces the thread rows with those of `threads`, the session's after they changed,
    /// at `places`, where `Session::locate` finds them in this diff.
    pub fn set_threads(&mut self, threads: &[Thread], places: Vec<Option<Place>>) {
        self.spots = threads
            .iter()
            .zip(&places)
            .map(|(t, p)| self.spot(&t.anchor, p.as_ref()))
            .collect();
        self.places = places;
        self.place();
    }

    /// Where thread `idx` is in this diff.
    pub fn place_of(&self, idx: usize) -> Option<&Place> {
        self.places.get(idx)?.as_ref()
    }

    /// Puts the thread rows where `spots` say, for the rows there are now.
    fn place(&mut self) {
        let mut at: HashMap<Spot, Vec<usize>> = HashMap::new();
        for (idx, spot) in self.spots.iter().enumerate() {
            at.entry(*spot).or_default().push(idx);
        }
        let thread_row = |idx: usize| Row::Thread { idx };
        let old = std::mem::take(&mut self.rows);
        let mut rows = Vec::with_capacity(old.len() + self.spots.len());
        for row in old {
            let spots = match &row {
                Row::File { file, .. } if self.collapsed.contains_key(file) => [None, None],
                Row::File { file, .. } => [Some(Spot::File(*file)), None],
                Row::Commit { .. } => [Some(Spot::Commit), None],
                Row::Code { file, left, right } => [
                    left.as_ref().map(|s| Spot::Line(*file, false, s.lineno)),
                    right.as_ref().map(|s| Spot::Line(*file, true, s.lineno)),
                ],
                Row::Thread { .. } => continue,
                _ => [None, None],
            };
            let fold = match &row {
                Row::Fold {
                    file,
                    old,
                    new,
                    hidden,
                } => Some((*file, *old, *new, *hidden)),
                _ => None,
            };
            rows.push(row);
            for spot in spots.into_iter().flatten() {
                rows.extend(at.remove(&spot).into_iter().flatten().map(thread_row));
            }
            // Threads on lines the fold hides go under it; a run shows only its rows'.
            if let Some((file, old, new, hidden)) = fold
                && self.scope == Scope::Range
            {
                let inside = |spot: &Spot| match *spot {
                    Spot::Line(f, right, line) => {
                        let start = if right { new } else { old };
                        f == file && line >= start && hidden.is_none_or(|n| line < start + n)
                    }
                    _ => false,
                };
                let mut idx: Vec<usize> = at
                    .extract_if(|spot, _| inside(spot))
                    .flat_map(|(_, idx)| idx)
                    .collect();
                idx.sort();
                rows.extend(idx.into_iter().map(thread_row));
            }
        }
        // Left over are `Whole` ones, `Top` ones, lines past the file's rows (or in its
        // folds, in a run), and those of collapsed files, which stay out.
        let mut rest: Vec<usize> = at
            .into_iter()
            .filter(|(spot, _)| match self.scope {
                Scope::Range => *spot != Spot::Hidden,
                Scope::Run => *spot == Spot::Whole,
            })
            .filter(|(spot, _)| {
                !matches!(spot, Spot::File(f) | Spot::Line(f, ..) if self.collapsed.contains_key(f))
            })
            .flat_map(|(_, idx)| idx)
            .collect();
        rest.sort();
        rows.splice(1..1, rest.into_iter().map(thread_row));
        self.rows = rows;
        self.version = next_version();
    }

    /// Changes with the rows, and is no other doc's.
    pub fn version(&self) -> u64 {
        self.version
    }

    fn spot(&self, anchor: &Anchor, place: Option<&Place>) -> Spot {
        let find = |file: &str, old: bool| {
            self.paths
                .iter()
                .position(|p| if old { p.0 == file } else { p.1 == file })
        };
        match place {
            Some(Place::Whole) if matches!(anchor, Anchor::Commit { .. }) => Spot::Commit,
            Some(Place::Whole) => Spot::Whole,
            Some(Place::File { file, old }) => find(file, *old).map_or(Spot::Top, Spot::File),
            Some(Place::Lines { file, end, old, .. }) => {
                find(file, *old).map_or(Spot::Top, |i| Spot::Line(i, !old, *end))
            }
            None if matches!(anchor, Anchor::Commit { .. }) => Spot::Hidden,
            None => Spot::Top,
        }
    }

    /// The anchor for a `c` on these rows: their code lines, preferring the right
    /// (new) side; otherwise the file or commit of the first row, or the session.
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
            let (rev, path, lines) = if right.is_empty() {
                let left = code
                    .filter_map(|c| c.1.as_ref().map(|s| s.lineno))
                    .collect();
                (&self.left_rev, &self.paths[file].0, left)
            } else {
                (&self.right_rev, &self.paths[file].1, right)
            };
            return Anchor::Lines {
                rev: rev.clone(),
                file: path.clone(),
                start: *lines.iter().min().unwrap(),
                end: *lines.iter().max().unwrap(),
            };
        }
        match rows.first() {
            Some(Row::File { file, .. } | Row::Fold { file, .. }) => self.file_anchor(*file),
            Some(Row::Commit { sha, .. }) => Anchor::Commit {
                commit: sha.clone(),
            },
            _ => Anchor::Session,
        }
    }

    /// The row of `commit`, when it is the commit shown.
    pub fn commit_row(&self, commit: &str) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| matches!(r, Row::Commit { sha, .. } if sha == commit))
    }

    /// The anchor for `file`: on the right side, unless only the left has it.
    pub fn file_anchor(&self, file: usize) -> Anchor {
        let (rev, path) = if self.statuses[file] == 'D' {
            (&self.left_rev, &self.paths[file].0)
        } else {
            (&self.right_rev, &self.paths[file].1)
        };
        Anchor::File {
            rev: rev.clone(),
            file: path.clone(),
        }
    }

    /// Shows up to 20 lines of the fold at `row` (the ten at each end of a longer fold),
    /// reading the file with `read(rev, path)`. Returns the row the fold was at, which
    /// shifts when threads on lines past the file's end move from under it to the top group.
    pub fn expand(
        &mut self,
        row: usize,
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
        self.place();
        Ok(row + (self.top_threads() - top))
    }

    /// Whether `row` is in the top thread group, away from what it anchors to.
    pub fn at_top(&self, row: usize) -> bool {
        row <= self.top_threads()
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

    /// The next (or previous) row `hit` is true of, going around from `from` and back to
    /// it, the rows of a collapsed file included: that file is expanded.
    pub fn search(
        &mut self,
        from: usize,
        forward: bool,
        hit: impl Fn(&Row) -> bool,
    ) -> Option<usize> {
        let len = self.rows.len();
        let order: Vec<usize> = if forward {
            (from..len).chain(0..from).collect()
        } else {
            (0..from).rev().chain((from..len).rev()).collect()
        };
        for i in order {
            // A collapsed file's rows come after its header.
            let on_row = i != from && hit(&self.rows[i]);
            if forward && on_row {
                return Some(i);
            }
            if let Row::File { file, .. } = self.rows[i]
                && self.hides(file, &hit)
            {
                let header = self.expand_file(file);
                let end = self.next_file(header, true).unwrap_or(self.rows.len());
                // A thread of the file whose line has no row goes to the top group instead.
                let rows = 0..self.rows.len();
                let hit_at = |&r: &usize| hit(&self.rows[r]);
                return if forward {
                    self.find(header, true, &hit)
                        .or_else(|| rows.clone().find(hit_at))
                } else {
                    self.find(end, false, &hit)
                        .or_else(|| rows.rev().find(hit_at))
                };
            }
            if on_row {
                return Some(i);
            }
        }
        hit(&self.rows[from]).then_some(from)
    }

    /// Whether `hit` is true of a row collapsed `file` hides, its threads included.
    fn hides(&self, file: usize, hit: impl Fn(&Row) -> bool) -> bool {
        self.hidden(file, |r| usize::from(hit(r))) > 0
    }

    /// The sum of `count` over the rows collapsed `file` hides, its threads included.
    fn hidden(&self, file: usize, count: impl Fn(&Row) -> usize) -> usize {
        let Some(rows) = self.collapsed.get(&file) else {
            return 0;
        };
        // As `place` puts them once expanded: a run shows only threads on its code rows.
        let on_row = |right: bool, line: u32| {
            rows.iter().any(|row| match row {
                Row::Code {
                    left, right: new, ..
                } => (if right { new } else { left })
                    .as_ref()
                    .is_some_and(|s| s.lineno == line),
                _ => false,
            })
        };
        let shows = |spot: Spot| match spot {
            Spot::Line(_, right, line) if self.scope == Scope::Run => on_row(right, line),
            _ => true,
        };
        rows.iter().map(&count).sum::<usize>()
            + (0..self.spots.len())
                .filter(|&idx| self.spots[idx].file() == Some(file) && shows(self.spots[idx]))
                .map(|idx| count(&Row::Thread { idx }))
                .sum::<usize>()
    }

    /// Per row, the matches `count` finds before it, and how many there are in all; those
    /// of a collapsed file come right under its header.
    pub fn tally(&self, count: impl Fn(&Row) -> usize) -> (Vec<usize>, usize) {
        let mut before = Vec::with_capacity(self.rows.len());
        let mut total = 0;
        for r in &self.rows {
            before.push(total);
            total += count(r);
            if let Row::File { file, .. } = r {
                total += self.hidden(*file, &count);
            }
        }
        (before, total)
    }

    pub fn collapsed(&self, file: usize) -> bool {
        self.collapsed.contains_key(&file)
    }

    /// Collapses `file` to its header. Returns the header row.
    pub fn collapse_file(&mut self, file: usize) -> usize {
        let row = self.file_row(file);
        let end = self.next_file(row, true).unwrap_or(self.rows.len());
        let rows = self
            .rows
            .drain(row + 1..end)
            .filter(|r| !matches!(r, Row::Thread { .. }))
            .collect();
        self.collapsed.insert(file, rows);
        self.place();
        self.file_row(file)
    }

    /// Expands `file` if collapsed. Returns the header row.
    pub fn expand_file(&mut self, file: usize) -> usize {
        let row = self.file_row(file);
        if let Some(rows) = self.collapsed.remove(&file) {
            self.rows.splice(row + 1..row + 1, rows);
            self.place();
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
            .expect("a file's header row")
    }

    /// Revs of the left and right side.
    pub fn revs(&self) -> (&str, &str) {
        (&self.left_rev, &self.right_rev)
    }

    /// (old_path, new_path) per file.
    pub fn paths(&self) -> &[(String, String)] {
        &self.paths
    }

    /// Status letter of `file`: A, D, M, R, T, or ' ' for `Doc::file`.
    pub fn status(&self, file: usize) -> char {
        self.statuses[file]
    }

    /// The file a new `anchor`, of `anchor_at`, is on in this view.
    pub fn file_of(&self, anchor: &Anchor) -> Option<usize> {
        match anchor {
            Anchor::Lines { file, .. } | Anchor::File { file, .. } => {
                self.paths.iter().position(|p| p.0 == *file || p.1 == *file)
            }
            _ => None,
        }
    }

    /// Thread indices in row order, those of a collapsed file at its header,
    /// then those hidden in this view.
    pub fn thread_order(&self) -> Vec<usize> {
        let n = self.spots.len();
        let mut order: Vec<usize> = Vec::new();
        for r in &self.rows {
            match r {
                Row::Thread { idx } => order.push(*idx),
                Row::File { file, .. } if self.collapsed(*file) => {
                    order.extend((0..n).filter(|&i| self.spots[i].file() == Some(*file)))
                }
                _ => {}
            }
        }
        let hidden: Vec<usize> = (0..n).filter(|i| !order.contains(i)).collect();
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

impl Spot {
    fn file(self) -> Option<usize> {
        match self {
            Spot::File(f) | Spot::Line(f, ..) => Some(f),
            _ => None,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{Hunk, Line};
    use crate::session::Message;

    /// `a` changing line 2 of `x` to `y`, with a line of context on each side.
    fn doc() -> Doc {
        doc_of(Scope::Range, None)
    }

    fn doc_of(scope: Scope, commit: Option<(String, String)>) -> Doc {
        let line = |kind, text: &str| Line {
            kind,
            text: text.into(),
        };
        let file = FileDiff {
            status: 'M',
            old_path: "a".into(),
            new_path: "a".into(),
            binary: false,
            hunks: vec![Hunk {
                old_start: 1,
                old_len: 3,
                new_start: 1,
                new_len: 3,
                lines: vec![
                    line(Kind::Ctx, "1"),
                    line(Kind::Del, "x"),
                    line(Kind::Add, "y"),
                    line(Kind::Ctx, "3"),
                ],
            }],
        };
        Doc::new("b".into(), "e".into(), vec![file], scope, commit)
    }

    fn thread(anchor: Anchor) -> Thread {
        Thread {
            id: "t1".into(),
            anchor,
            messages: vec![Message {
                from: "user".into(),
                body: "?".into(),
            }],
            place: None,
            was: None,
        }
    }

    fn thread_rows(doc: &Doc) -> Vec<usize> {
        (0..doc.len())
            .filter(|&i| matches!(doc.window(i, 1)[0], Row::Thread { .. }))
            .collect()
    }

    #[test]
    fn commit_threads_go_under_the_commit_row() {
        let mut doc = doc_of(Scope::Run, Some(("c".into(), "subject".into())));
        let threads = [
            thread(Anchor::Commit { commit: "c".into() }),
            thread(Anchor::Session),
        ];
        doc.set_threads(&threads, vec![Some(Place::Whole), Some(Place::Whole)]);
        // Session, its thread, the commit, its thread, the file.
        assert_eq!(thread_rows(&doc), [1, 3]);
        assert_eq!(doc.thread_order(), [1, 0]);
        assert_eq!(doc.commit_row("c"), Some(2));
        assert!(matches!(doc.anchor_at(2..=2), Anchor::Commit { commit } if commit == "c"));
    }

    #[test]
    fn threads_follow_collapsing() {
        let mut doc = doc();
        let line2 = thread(Anchor::Lines {
            rev: "e".into(),
            file: "a".into(),
            start: 2,
            end: 2,
        });
        let at2 = Place::Lines {
            file: "a".into(),
            start: 2,
            end: 2,
            old: false,
        };
        doc.set_threads(
            &[thread(Anchor::Session), line2],
            vec![Some(Place::Whole), Some(at2)],
        );
        // Session, its thread, the file, line 1, the changed line 2, its thread, line 3.
        assert_eq!(thread_rows(&doc), [1, 5]);
        assert_eq!(doc.thread_order(), [0, 1]);
        // A collapsed file keeps its threads out of the rows but in the order.
        doc.collapse_file(0);
        assert_eq!(thread_rows(&doc), [1]);
        assert_eq!(doc.thread_order(), [0, 1]);
        doc.expand_file(0);
        assert_eq!(thread_rows(&doc), [1, 5]);
    }

    #[test]
    fn runs_show_only_threads_on_their_rows() {
        let at = |line| {
            Some(Place::Lines {
                file: "a".into(),
                start: line,
                end: line,
                old: false,
            })
        };
        let lines = |line| {
            thread(Anchor::Lines {
                rev: "e".into(),
                file: "a".into(),
                start: line,
                end: line,
            })
        };
        let threads = [
            thread(Anchor::Session),
            lines(2),
            lines(9),
            thread(Anchor::Commit {
                commit: "gone".into(),
            }),
        ];
        let places = || vec![Some(Place::Whole), at(2), at(9), None];
        // Session, the file, line 1, the changed line 2, line 3; threads after.
        let mut range = doc_of(Scope::Range, None);
        range.set_threads(&threads, places());
        assert_eq!(thread_rows(&range), [1, 2, 6]);
        assert_eq!(range.thread_order(), [0, 2, 1, 3]);
        let mut run = doc_of(Scope::Run, None);
        run.set_threads(&threads, places());
        assert_eq!(thread_rows(&run), [1, 5]);
        assert_eq!(run.thread_order(), [0, 1, 2, 3]);
    }

    #[test]
    fn search_goes_around_and_into_collapsed_files() {
        let mut doc = doc();
        // Session, the file, line 1, the changed line 2, line 3.
        let y = |r: &Row| matches!(r, Row::Code { right: Some(s), .. } if s.text == "y");
        assert_eq!(doc.search(4, true, y), Some(3));
        assert_eq!(doc.search(3, true, y), Some(3));
        doc.collapse_file(0);
        assert_eq!(doc.search(0, false, y), Some(3));
        assert!(!doc.collapsed(0));
    }

    #[test]
    fn collapsed_runs_hide_only_threads_they_show() {
        let mut doc = doc_of(Scope::Run, None);
        let past_rows = Some(Place::Lines {
            file: "a".into(),
            start: 9,
            end: 9,
            old: false,
        });
        let line9 = thread(Anchor::Lines {
            rev: "e".into(),
            file: "a".into(),
            start: 9,
            end: 9,
        });
        doc.set_threads(&[line9], vec![past_rows]);
        let is_thread = |r: &Row| matches!(r, Row::Thread { .. });
        assert_eq!(doc.tally(|r| usize::from(is_thread(r))).1, 0);
        doc.collapse_file(0);
        assert_eq!(doc.tally(|r| usize::from(is_thread(r))).1, 0);
        assert_eq!(doc.search(0, true, is_thread), None);
        assert!(doc.collapsed(0));
    }

    #[test]
    fn tally_counts_into_collapsed_files() {
        let mut doc = doc();
        // Session, the file, line 1, the changed line 2, line 3: "1" and "3" on both sides.
        let count = |r: &Row| match r {
            Row::Code { left, right, .. } => [left, right]
                .into_iter()
                .flatten()
                .filter(|s| matches!(s.text.as_str(), "1" | "3"))
                .count(),
            _ => 0,
        };
        assert_eq!(doc.tally(count), (vec![0, 0, 0, 2, 2], 4));
        doc.collapse_file(0);
        assert_eq!(doc.tally(count), (vec![0, 0], 4));
    }
}
