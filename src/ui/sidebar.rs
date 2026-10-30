use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{Ctx, View};
use crate::session::Anchor;
use crate::ui::commit_row::{Columns, mark};
use crate::ui::query::Query;
use crate::ui::{boxed, check, cursor_bar, row_at, status_style, step};

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
    /// The files box's `/` filter on paths.
    filter: Query,
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
            Entry::File(file) => Some(ctx.doc.file_anchor(file)),
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
            files(ctx, &self.filter.text)
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

    /// Typing the filter, which takes all keys and leaves the mouse alone.
    pub fn typing(&self) -> bool {
        self.filter.typing
    }

    /// Moves the cursor to the row at screen line `y` of the box over `area`; false when
    /// there is none.
    pub fn click(&mut self, area: Rect, y: u16, ctx: &Ctx) -> bool {
        let row = row_at(area, self.top, self.items(ctx).len(), y);
        if let Some(row) = row {
            self.cursor = row;
        }
        row.is_some()
    }

    /// While typing the filter, takes `key`; false otherwise.
    pub fn filter_key(&mut self, key: KeyEvent) -> bool {
        if !self.filter.typing {
            return false;
        }
        self.filter.key(key);
        self.cursor = 0;
        true
    }

    /// While typing the filter, adds pasted `text` to it.
    pub fn paste(&mut self, text: &str) {
        if self.filter.typing {
            self.filter.paste(text);
            self.cursor = 0;
        }
    }

    pub fn act(&mut self, action: &str, ctx: &Ctx) {
        let len = self.items(ctx).len();
        match action {
            "filter" if self.files => self.filter.typing = true,
            "cancel" if self.files => {
                self.filter = Query::default();
                self.cursor = 0;
            }
            _ => {}
        }
        if action == "check" {
            if let Entry::View(v) = self.entry(ctx)
                && let Some(checked) = &mut self.checked
            {
                *checked = check(*checked, v);
                if checked.is_some() {
                    self.cursor = (self.cursor + 1).min(len.saturating_sub(1));
                }
            }
            return;
        }
        if let Some(cursor) = step(self.cursor, action, len, self.height) {
            self.cursor = cursor;
        }
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
                spans.extend(item.checked.map(|on| mark(on, ctx.look.theme)));
                if let Entry::File(file) = item.entry {
                    let s = ctx.doc.status(file);
                    spans.push(Span::styled(
                        format!("{s} "),
                        status_style(s, ctx.look.theme),
                    ));
                }
                if let Entry::View(v) = item.entry
                    && let View::Commit(c) = &ctx.views[v]
                {
                    spans.extend(columns.row(c, width, ctx.look.theme));
                    return Line::from(spans);
                }
                spans.push(Span::raw(item.text.clone()));
                Line::from(spans)
            })
            .collect();
        let name = if self.files { " Files " } else { " Commits " };
        let title = name.to_string() + &self.filter.title();
        let block = boxed(title, focused, ctx.look.theme);
        let inner = block.inner(area);
        f.render_widget(Paragraph::new(lines).block(block), area);
        if focused && self.cursor < items.len() && self.height > 0 {
            let y = inner.y + (self.cursor - self.top) as u16;
            cursor_bar(f, inner, y, ctx.look.theme);
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
        // Newest first, as in the picker.
        .rev()
        .collect()
}

/// The files with `filter` in their path, any case, under their directories.
fn files(ctx: &Ctx, filter: &str) -> Vec<Item> {
    let mut items = Vec::new();
    let filter = filter.to_lowercase();
    // Files come in path order, so a directory's files are contiguous.
    let mut prev: Vec<&str> = Vec::new();
    for (file, (_, path)) in ctx.doc.paths().iter().enumerate() {
        if !path.to_lowercase().contains(&filter) {
            continue;
        }
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
