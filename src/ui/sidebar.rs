use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{Ctx, View};
use crate::session::Anchor;
use crate::ui::commit_row::{Columns, mark};
use crate::ui::{boxed, check, cursor_bar, status_style};

/// A box: the commits (and uncommitted changes), `☒` on those shown, where Space checks
/// another run for Enter to show; or the files of the shown diff as a tree, `☒` on those reviewed.
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
                View::Uncommitted => Anchor::Session,
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
    /// `☒` or `☐` on a view or file.
    checked: Option<bool>,
    /// Columns before the mark: a file's or directory's depth in the tree.
    indent: usize,
    text: String,
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
        let shown = ctx.shown.bounds(ctx.views.len());
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

    pub fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx, focused: bool) {
        let items = self.items(ctx);
        self.height = area.height.saturating_sub(2) as usize;
        self.cursor = self.cursor.min(items.len().saturating_sub(1));
        self.top = self.top.min(self.cursor);
        if self.cursor >= self.top + self.height {
            self.top = self.cursor + 1 - self.height;
        }
        let columns = Columns::new(ctx.views.iter().filter_map(|v| match v {
            View::Commit(c) => Some(c),
            _ => None,
        }));
        // Inside the borders, after the mark.
        let width = (area.width as usize).saturating_sub(2 + 2);
        let lines: Vec<Line> = items
            .iter()
            .skip(self.top)
            .take(self.height)
            .map(|item| {
                let mut spans = vec![Span::raw(" ".repeat(item.indent))];
                spans.extend(item.checked.map(|on| mark(on, ctx.theme)));
                if let Entry::File(file) = item.entry {
                    let s = ctx.doc.status(file);
                    spans.push(Span::styled(format!("{s} "), status_style(s, ctx.theme)));
                }
                if let Entry::View(v) = item.entry
                    && let View::Commit(c) = &ctx.views[v]
                {
                    spans.extend(columns.row(c, width, ctx.theme));
                    return Line::from(spans);
                }
                spans.push(Span::raw(item.text.clone()));
                Line::from(spans)
            })
            .collect();
        let title = if self.files { " Files " } else { " Commits " };
        let block = boxed(title, focused, ctx.theme);
        let inner = block.inner(area);
        f.render_widget(Paragraph::new(lines).block(block), area);
        if focused && self.cursor < items.len() && self.height > 0 {
            let y = inner.y + (self.cursor - self.top) as u16;
            cursor_bar(f, inner, y, ctx.theme);
        }
    }
}

fn views(ctx: &Ctx, checked: Option<Option<(usize, usize)>>) -> Vec<Item> {
    let within = |range: (usize, usize), v| (range.0..=range.1).contains(&v);
    let shown = ctx.shown.bounds(ctx.views.len());
    ctx.views
        .iter()
        .enumerate()
        .map(|(v, view)| {
            // The checks while the box has focus, else the views shown.
            let on = match checked {
                Some(range) => range.is_some_and(|range| within(range, v)),
                None => within(shown, v),
            };
            let text = match view {
                // Drawn by `Columns::row`.
                View::Commit(_) => String::new(),
                View::Uncommitted => ctx.session.end.clone(),
            };
            Item {
                entry: Entry::View(v),
                checked: Some(on),
                indent: 0,
                text,
            }
        })
        .collect()
}

fn files(ctx: &Ctx) -> Vec<Item> {
    let mut items = Vec::new();
    // Files come in path order, so a directory's files are contiguous.
    let mut prev: Vec<&str> = Vec::new();
    for (file, (_, path)) in ctx.doc.paths().iter().enumerate() {
        let mut dirs: Vec<&str> = path.split('/').collect();
        let name = dirs.pop().unwrap_or_default();
        let same = prev.iter().zip(&dirs).take_while(|(a, b)| a == b).count();
        for (depth, dir) in dirs.iter().enumerate().skip(same) {
            items.push(Item {
                entry: Entry::Other,
                checked: None,
                indent: 2 * depth,
                text: format!("{dir}/"),
            });
        }
        items.push(Item {
            entry: Entry::File(file),
            checked: Some(ctx.doc.collapsed(file)),
            indent: 2 * dirs.len(),
            text: name.to_string(),
        });
        prev = dirs;
    }
    items
}
