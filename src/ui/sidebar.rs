use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{Ctx, View};
use crate::session::{Anchor, Thread};
use crate::ui::commit_row::{Columns, mark};
use crate::ui::{boxed, check};

/// A box: the commits (and uncommitted changes), `[x]` on those shown, where Space checks
/// another run for Enter to show; or the files of the shown diff as a tree.
#[derive(Default)]
pub struct Sidebar {
    files: bool,
    cursor: usize,
    top: usize,
    height: usize,
    /// While the commits box has focus: the checked views first..=last, or None for none.
    /// Outside it (None) the box marks the views shown.
    checked: Option<Option<(usize, usize)>>,
}

#[derive(Clone, Copy)]
pub enum Entry {
    View(usize),
    File(usize),
    /// A directory.
    Other,
}

impl Entry {
    /// The anchor for a `c` here: commit, file, or the session for the other views.
    pub fn anchor(self, ctx: &Ctx) -> Option<Anchor> {
        match self {
            Entry::View(v) => Some(match &ctx.views[v] {
                View::Commit(c) => Anchor::Commit {
                    commit: c.sha.clone(),
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
    /// `[x]` or `[ ]` on a view.
    checked: Option<bool>,
    text: String,
    waiting: usize,
}

impl Sidebar {
    pub fn files() -> Sidebar {
        Sidebar {
            files: true,
            ..Sidebar::default()
        }
    }

    fn items(&self, ctx: &Ctx) -> Vec<Item> {
        if self.files {
            files(ctx)
        } else {
            views(ctx, self.checked)
        }
    }

    pub fn entry(&self, ctx: &Ctx) -> Entry {
        self.items(ctx)
            .get(self.cursor)
            .map_or(Entry::Other, |i| i.entry)
    }

    pub fn checked(&self) -> Option<(usize, usize)> {
        self.checked.flatten()
    }

    /// Entering the commits box checks the views shown; leaving it drops the checks.
    pub fn focus(&mut self, ctx: &Ctx, focused: bool) {
        let shown = match ctx.view {
            (0, 0) => (1, ctx.views.len() - 1),
            view => view,
        };
        self.checked = focused.then_some(Some(shown));
    }

    pub fn act(&mut self, action: &str, ctx: &Ctx) {
        let last = self.items(ctx).len().saturating_sub(1);
        if action == "check" {
            if let Entry::View(v) = self.entry(ctx)
                && let Some(checked) = &mut self.checked
            {
                *checked = check(*checked, v);
                if checked.is_some() {
                    self.cursor = (self.cursor + 1).min(last);
                }
            }
            return;
        }
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

    /// `file`, when set, is marked instead of the cursor.
    pub fn render(
        &mut self,
        f: &mut Frame,
        area: Rect,
        ctx: &Ctx,
        focused: bool,
        file: Option<usize>,
    ) {
        let items = self.items(ctx);
        self.height = area.height.saturating_sub(2) as usize;
        self.cursor = self.cursor.min(items.len().saturating_sub(1));
        let marked = match file {
            Some(file) => items
                .iter()
                .position(|i| matches!(i.entry, Entry::File(n) if n == file)),
            None => focused.then_some(self.cursor),
        };
        let shown = marked.unwrap_or(self.cursor);
        self.top = self.top.min(shown);
        if shown >= self.top + self.height {
            self.top = shown + 1 - self.height;
        }
        let columns = Columns::new(ctx.views.iter().filter_map(|v| match v {
            View::Commit(c) => Some(c),
            _ => None,
        }));
        // Inside the borders, after the mark.
        let width = (area.width as usize).saturating_sub(2 + 4);
        let lines: Vec<Line> = items
            .iter()
            .enumerate()
            .skip(self.top)
            .take(self.height)
            .map(|(i, item)| {
                let mut style = Style::new();
                if Some(i) == marked {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                let mut spans: Vec<Span> = item
                    .checked
                    .map(|on| mark(on, ctx.theme))
                    .into_iter()
                    .collect();
                if let Entry::View(v) = item.entry
                    && let View::Commit(c) = &ctx.views[v]
                {
                    spans.extend(columns.row(c, item.waiting, width, style, ctx.theme));
                    return Line::from(spans);
                }
                spans.push(Span::styled(item.text.clone(), style));
                if item.waiting > 0 {
                    spans.push(Span::styled(
                        format!(" ({})", item.waiting),
                        Style::new().fg(ctx.theme.accent),
                    ));
                }
                Line::from(spans)
            })
            .collect();
        let title = if self.files { " Files " } else { " Commits " };
        f.render_widget(
            Paragraph::new(lines).block(boxed(title, focused, ctx.theme)),
            area,
        );
    }
}

fn views(ctx: &Ctx, checked: Option<Option<(usize, usize)>>) -> Vec<Item> {
    let threads = &ctx.session.threads;
    let within = |range: (usize, usize), v| (range.0..=range.1).contains(&v);
    // View 0, all, is every row checked.
    ctx.views
        .iter()
        .enumerate()
        .skip(1)
        .map(|(v, view)| {
            // The checks while the box has focus, else the views shown.
            let on = match checked {
                Some(range) => range.is_some_and(|range| within(range, v)),
                None => ctx.view == (0, 0) || within(ctx.view, v),
            };
            let (text, waiting) = match view {
                // Drawn by `Columns::row`.
                View::Commit(c) => (String::new(), count(threads, &c.sha)),
                _ => (ctx.session.end.clone(), count(threads, &ctx.session.end)),
            };
            Item {
                entry: Entry::View(v),
                checked: Some(on),
                text,
                waiting,
            }
        })
        .collect()
}

fn files(ctx: &Ctx) -> Vec<Item> {
    let threads = &ctx.session.threads;
    let mut items = Vec::new();
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
                checked: None,
                text: format!("{}{dir}/", "  ".repeat(depth)),
                waiting: 0,
            });
        }
        items.push(Item {
            entry: Entry::File(file),
            checked: None,
            text: format!("{}{name}", "  ".repeat(dirs.len())),
            waiting: waiting[file],
        });
        prev = dirs;
    }
    items
}

/// Threads of `commit` waiting on the user.
fn count(threads: &[Thread], commit: &str) -> usize {
    threads
        .iter()
        .filter(|t| t.waiting_on_user() && t.anchor.commit() == Some(commit))
        .count()
}
