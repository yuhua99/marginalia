use std::collections::HashMap;
use std::ops::RangeInclusive;

use crate::repo::{FileDiff, Kind};
use crate::session::{Anchor, Status, Thread};

/// The logical rows of a side-by-side diff, with threads placed under what they anchor to.
pub struct Doc {
    /// Rev of the left (old) and right (new) side.
    left_rev: String,
    right_rev: String,
    /// (old_path, new_path) per file.
    paths: Vec<(String, String)>,
    rows: Vec<Row>,
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
    /// Unchanged lines between hunks.
    Fold { file: usize, hidden: u32 },
    /// Index into the session threads. Threads that have no place in the diff
    /// (session, commit, other revs, lines outside hunks) come right after `Session`.
    Thread { idx: usize },
}

pub struct Side {
    pub lineno: u32,
    pub kind: Kind,
    pub text: String,
}

/// Where a thread row goes.
#[derive(Hash, PartialEq, Eq)]
enum Spot {
    Top,
    File(usize),
    /// File, right side, last anchored line.
    Line(usize, bool, u32),
}

impl Doc {
    pub fn new(left_rev: String, right_rev: String, files: Vec<FileDiff>) -> Doc {
        let mut rows = vec![Row::Session];
        let mut paths = Vec::new();
        for (file, f) in files.into_iter().enumerate() {
            let path = if f.old_path == f.new_path {
                f.new_path.clone()
            } else {
                format!("{} → {}", f.old_path, f.new_path)
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
            let mut next_old = 1;
            for h in f.hunks {
                let (mut old, mut new) =
                    (first(h.old_start, h.old_len), first(h.new_start, h.new_len));
                if old > next_old {
                    rows.push(Row::Fold {
                        file,
                        hidden: old - next_old,
                    });
                }
                next_old = old + h.old_len;
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
                            });
                            old += 1;
                        }
                        Kind::Add => {
                            adds.push(Side {
                                lineno: new,
                                kind: Kind::Add,
                                text: line.text,
                            });
                            new += 1;
                        }
                        Kind::Ctx => {
                            pair(&mut rows, file, &mut dels, &mut adds);
                            let left = Side {
                                lineno: old,
                                kind: Kind::Ctx,
                                text: line.text.clone(),
                            };
                            let right = Side {
                                lineno: new,
                                kind: Kind::Ctx,
                                text: line.text,
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
        }
        Doc {
            left_rev,
            right_rev,
            paths,
            rows,
        }
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
        let mut rest: Vec<usize> = at.into_values().flatten().collect();
        rest.sort();
        rows.splice(1..1, rest.into_iter().map(thread_row));
        self.rows = rows;
    }

    fn spot(&self, anchor: &Anchor) -> Spot {
        let find = |f: &dyn Fn(&(String, String)) -> bool| self.paths.iter().position(f);
        match anchor {
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
            let (rev, path, lines) = if right.is_empty() {
                let left = code
                    .filter_map(|c| c.1.as_ref().map(|s| s.lineno))
                    .collect();
                (&self.left_rev, &self.paths[file].0, left)
            } else {
                (&self.right_rev, &self.paths[file].1, right)
            };
            return Anchor::Lines {
                rev: Some(rev.clone()),
                file: path.clone(),
                start: *lines.iter().min().unwrap(),
                end: *lines.iter().max().unwrap(),
            };
        }
        match rows.first() {
            Some(Row::File { file, .. } | Row::Fold { file, .. }) => Anchor::File {
                file: self.paths[*file].1.clone(),
            },
            _ => Anchor::Session,
        }
    }

    /// The next (or previous) thread row waiting on the user.
    pub fn next_waiting(&self, from: usize, forward: bool, threads: &[Thread]) -> Option<usize> {
        let waiting = |&i: &usize| match self.rows[i] {
            Row::Thread { idx } => {
                threads[idx].status == Status::Open && threads[idx].waiting_on() == "user"
            }
            _ => false,
        };
        if forward {
            (from + 1..self.rows.len()).find(waiting)
        } else {
            (0..from).rev().find(waiting)
        }
    }

    /// Thread indices in row order.
    pub fn thread_order(&self) -> Vec<usize> {
        self.rows
            .iter()
            .filter_map(|r| match r {
                Row::Thread { idx } => Some(*idx),
                _ => None,
            })
            .collect()
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
        let (left, right) = (dels.next(), adds.next());
        if left.is_none() && right.is_none() {
            return;
        }
        rows.push(Row::Code { file, left, right });
    }
}
