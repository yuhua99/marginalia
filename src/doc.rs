use crate::repo::{FileDiff, Kind};

/// The logical rows of a side-by-side diff.
pub struct Doc {
    rows: Vec<Row>,
}

pub enum Row {
    File {
        status: char,
        path: String,
        note: Option<&'static str>,
    },
    Code {
        left: Option<Side>,
        right: Option<Side>,
    },
    /// Unchanged lines between hunks.
    Fold { hidden: u32 },
}

pub struct Side {
    pub lineno: u32,
    pub kind: Kind,
    pub text: String,
}

impl Doc {
    pub fn new(files: Vec<FileDiff>) -> Doc {
        let mut rows = Vec::new();
        for f in files {
            let path = if f.old_path == f.new_path {
                f.new_path
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
                status: f.status,
                path,
                note,
            });
            let mut next_old = 1;
            for h in f.hunks {
                let (mut old, mut new) =
                    (first(h.old_start, h.old_len), first(h.new_start, h.new_len));
                if old > next_old {
                    rows.push(Row::Fold {
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
                                pair(&mut rows, &mut dels, &mut adds);
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
                            pair(&mut rows, &mut dels, &mut adds);
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
                                left: Some(left),
                                right: Some(right),
                            });
                            old += 1;
                            new += 1;
                        }
                    }
                }
                pair(&mut rows, &mut dels, &mut adds);
            }
        }
        Doc { rows }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
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

fn pair(rows: &mut Vec<Row>, dels: &mut Vec<Side>, adds: &mut Vec<Side>) {
    let mut dels = dels.drain(..);
    let mut adds = adds.drain(..);
    loop {
        let (left, right) = (dels.next(), adds.next());
        if left.is_none() && right.is_none() {
            return;
        }
        rows.push(Row::Code { left, right });
    }
}
