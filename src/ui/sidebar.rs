use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{Ctx, View};
use crate::session::{Anchor, Thread};

/// The views (all, each commit, uncommitted), then the files of the shown diff as a tree.
#[derive(Default)]
pub struct Sidebar {
    cursor: usize,
    top: usize,
    height: usize,
}

#[derive(Clone, Copy)]
pub enum Entry {
    View(usize),
    File(usize),
    /// A directory or the gap between views and files.
    Other,
}

impl Entry {
    /// The anchor for a `c` here: commit, file, or the session for the other views.
    pub fn anchor(self, ctx: &Ctx) -> Option<Anchor> {
        match self {
            Entry::View(v) => Some(match &ctx.views[v] {
                View::Commit { sha, .. } => Anchor::Commit {
                    commit: sha.clone(),
                },
                _ => Anchor::Session,
            }),
            Entry::File(file) => Some(Anchor::File {
                file: ctx.doc.paths()[file].1.clone(),
            }),
            Entry::Other => None,
        }
    }
}

struct Item {
    entry: Entry,
    text: String,
    waiting: usize,
}

impl Sidebar {
    pub fn entry(&self, ctx: &Ctx) -> Entry {
        items(ctx)
            .get(self.cursor)
            .map_or(Entry::Other, |i| i.entry)
    }

    pub fn act(&mut self, action: &str, ctx: &Ctx) {
        let last = items(ctx).len() - 1;
        let half = (self.height / 2).max(1);
        self.cursor = match action {
            "down" => self.cursor + 1,
            "up" => self.cursor.saturating_sub(1),
            "half_page_down" => self.cursor + half,
            "half_page_up" => self.cursor.saturating_sub(half),
            "top" => 0,
            "bottom" => last,
            _ => self.cursor,
        }
        .min(last);
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx, focused: bool) {
        let items = items(ctx);
        self.height = area.height as usize;
        self.cursor = self.cursor.min(items.len() - 1);
        self.top = self.top.min(self.cursor);
        if self.cursor >= self.top + self.height {
            self.top = self.cursor + 1 - self.height;
        }
        let lines: Vec<Line> = items
            .iter()
            .enumerate()
            .skip(self.top)
            .take(self.height)
            .map(|(i, item)| {
                let mut style = Style::new();
                if matches!(item.entry, Entry::View(v) if v == ctx.view) {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if i == self.cursor {
                    style = if focused {
                        style.add_modifier(Modifier::REVERSED)
                    } else {
                        style.bg(Color::Rgb(50, 50, 50))
                    };
                }
                let mut spans = vec![Span::styled(item.text.clone(), style)];
                if item.waiting > 0 {
                    spans.push(Span::styled(
                        format!(" ●{}", item.waiting),
                        Style::new().fg(Color::Yellow),
                    ));
                }
                Line::from(spans)
            })
            .collect();
        f.render_widget(
            Paragraph::new(lines).block(Block::new().borders(Borders::RIGHT)),
            area,
        );
    }
}

fn items(ctx: &Ctx) -> Vec<Item> {
    let threads = &ctx.session.threads;
    let mut items: Vec<Item> = ctx
        .views
        .iter()
        .enumerate()
        .map(|(v, view)| {
            let mark = if v == ctx.view { '▸' } else { ' ' };
            let (text, waiting) = match view {
                View::All => (
                    "all".to_string(),
                    count(threads, |rev| {
                        rev == ctx.session.base || rev == ctx.session.end
                    }),
                ),
                View::Commit { sha, subject } => (
                    format!("{} {subject}", &sha[..7]),
                    count(threads, |rev| rev == sha),
                ),
                View::Uncommitted => (
                    ctx.session.end.clone(),
                    count(threads, |rev| rev == ctx.session.end),
                ),
            };
            Item {
                entry: Entry::View(v),
                text: format!("{mark} {text}"),
                waiting,
            }
        })
        .collect();
    items.push(Item {
        entry: Entry::Other,
        text: String::new(),
        waiting: 0,
    });
    // Files come in path order, so a directory's files are contiguous.
    let waiting = ctx.doc.waiting_per_file(threads);
    let mut prev: Vec<&str> = Vec::new();
    for (file, (_, path)) in ctx.doc.paths().iter().enumerate() {
        let mut dirs: Vec<&str> = path.split('/').collect();
        let name = dirs.pop().unwrap_or_default();
        let same = prev.iter().zip(&dirs).take_while(|(a, b)| a == b).count();
        for (depth, dir) in dirs.iter().enumerate().skip(same) {
            items.push(Item {
                entry: Entry::Other,
                text: format!("{}{dir}/", "  ".repeat(depth + 1)),
                waiting: 0,
            });
        }
        items.push(Item {
            entry: Entry::File(file),
            text: format!("{}{name}", "  ".repeat(dirs.len() + 1)),
            waiting: waiting[file],
        });
        prev = dirs;
    }
    items
}

/// Threads waiting on the user anchored at a rev (lines) or commit matching `f`.
fn count(threads: &[Thread], f: impl Fn(&str) -> bool) -> usize {
    threads
        .iter()
        .filter(|t| t.waiting_on_user())
        .filter(|t| match &t.anchor {
            Anchor::Lines { rev: Some(rev), .. } => f(rev),
            Anchor::Commit { commit } => f(commit),
            _ => false,
        })
        .count()
}
