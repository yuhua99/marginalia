use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::Ctx;
use crate::review::View;
use crate::session::Anchor;
use crate::ui::commit_row::{Columns, mark};
use crate::ui::query::Query;
use crate::ui::scroll::Scroll;
use crate::ui::{boxed, check, checked_bar, cursor_bar, lit, row_at, status_style};

/// A box: the commits (and uncommitted changes), `☒` on those shown, where Space checks
/// another run for Enter to show; or the files of the shown diff as a tree, `☒` on those reviewed.
#[derive(Default)]
pub struct Sidebar {
    files: bool,
    scroll: Scroll,
    /// While the commits box has focus: the checked views first..=last, or None for none.
    /// Outside it (None) the box marks the views shown.
    checked: Option<Option<(usize, usize)>>,
    /// The view a press in the commits box began on, until the button is let go.
    anchor: Option<usize>,
    /// Rows, as of the last render.
    len: usize,
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
            Entry::View(v) => Some(match &ctx.review.views[v] {
                View::Commit(c) => Anchor::Commit {
                    commit: c.sha.clone(),
                },
                View::Uncommitted(_) => Anchor::Session,
            }),
            Entry::File(file) => Some(ctx.review.doc.file_anchor(file)),
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
            .get(self.scroll.cursor())
            .map_or(Entry::Other, |i| i.entry)
    }

    pub fn checked(&self) -> Option<(usize, usize)> {
        self.checked.flatten()
    }

    /// Entering the commits box checks the views shown; leaving it drops the checks.
    pub fn focus(&mut self, ctx: &Ctx, focused: bool) {
        let shown = ctx.review.shown.bounds(ctx.review.views.len());
        self.checked = focused.then_some(Some(shown));
    }

    /// Typing the filter, which takes all keys and leaves the mouse alone.
    pub fn typing(&self) -> bool {
        self.filter.typing
    }

    /// The row at screen line `y` of the box over `area`, as of the last render.
    pub fn row_at(&self, area: Rect, y: u16) -> Option<usize> {
        row_at(area, self.scroll.top().0, self.len, y)
    }

    /// Moves the cursor to the row at screen line `y` of the box over `area`; false when
    /// there is none.
    pub fn click(&mut self, area: Rect, y: u16) -> bool {
        let row = self.row_at(area, y);
        if let Some(row) = row {
            self.scroll.go(row);
        }
        row.is_some()
    }

    /// A press at screen line `y` of the commits box over `area`: checks the view there
    /// alone, the start of a sweep over views.
    pub fn press(&mut self, area: Rect, y: u16, ctx: &Ctx) {
        self.anchor = None;
        if self.click(area, y)
            && let Entry::View(v) = self.entry(ctx)
        {
            self.anchor = Some(v);
            self.checked = Some(Some((v, v)));
        }
    }

    /// The button held over screen line `y` after a press on a view: checks the views from
    /// it to the one there.
    pub fn drag(&mut self, area: Rect, y: u16, ctx: &Ctx) {
        if let Some(from) = self.anchor
            && self.click(area, y)
            && let Entry::View(v) = self.entry(ctx)
        {
            self.checked = Some(Some((from.min(v), from.max(v))));
        }
    }

    /// The button let go; whether it ends a sweep.
    pub fn release(&mut self) -> bool {
        self.anchor.take().is_some()
    }

    /// Scrolls a wheel turn down or up, the cursor left.
    pub fn wheel(&mut self, down: bool) {
        self.scroll.wheel(down);
    }

    /// After a wheel turn, brings the cursor onto the screen.
    pub fn settle(&mut self) {
        self.scroll.settle();
    }

    /// While typing the filter, takes `key`; false otherwise.
    pub fn filter_key(&mut self, key: KeyEvent) -> bool {
        if !self.filter.typing {
            return false;
        }
        self.filter.key(key);
        self.scroll.go(0);
        true
    }

    /// While typing the filter, adds pasted `text` to it.
    pub fn paste(&mut self, text: &str) {
        if self.filter.typing {
            self.filter.paste(text);
            self.scroll.go(0);
        }
    }

    pub fn act(&mut self, action: &str, ctx: &Ctx) {
        let len = self.items(ctx).len();
        self.scroll.act(action, len);
        match action {
            "filter" if self.files => self.filter.typing = true,
            "cancel" if self.files => {
                self.filter = Query::default();
                self.scroll.go(0);
            }
            _ => {}
        }
        if action == "check"
            && let Entry::View(v) = self.entry(ctx)
            && let Some(checked) = &mut self.checked
        {
            *checked = check(*checked, v);
        }
    }

    /// The box over `area`, the text of the row under the `pointer` underlined.
    pub fn render(
        &mut self,
        f: &mut Frame,
        area: Rect,
        ctx: &Ctx,
        focused: bool,
        pointer: Option<Position>,
    ) {
        let items = self.items(ctx);
        self.len = items.len();
        let height = area.height.saturating_sub(2) as usize;
        self.scroll.place(height, items.len(), |_| 1, false);
        let (top, cursor) = (self.scroll.top().0, self.scroll.cursor());
        let hovered = pointer
            .filter(|&p| area.contains(p))
            .and_then(|p| self.row_at(area, p.y));
        let columns = Columns::new(ctx.review.views.iter().filter_map(|v| match v {
            View::Commit(c) => Some(c),
            _ => None,
        }));
        // Inside the borders, after the mark.
        let width = (area.width as usize).saturating_sub(2 + 2);
        let mut checked_rows = Vec::new();
        let lines: Vec<Line> = items
            .iter()
            .skip(top)
            .take(height)
            .enumerate()
            .map(|(row, item)| {
                if !self.files && item.checked == Some(true) {
                    checked_rows.push(row);
                }
                let on = hovered == Some(top + row);
                let mut spans = vec![Span::raw(" ".repeat(item.indent))];
                spans.extend(item.checked.map(|on| mark(on, ctx.look.theme)));
                if let Entry::File(file) = item.entry {
                    let s = ctx.review.doc.status(file);
                    spans.push(Span::styled(
                        format!("{s} "),
                        status_style(s, ctx.look.theme),
                    ));
                }
                if let Entry::View(v) = item.entry
                    && let View::Commit(c) = &ctx.review.views[v]
                {
                    spans.extend(columns.row(c, width, ctx.look.theme, on));
                    return Line::from(spans);
                }
                // A file's name or the uncommitted changes; a directory takes no click.
                let on = on && !matches!(item.entry, Entry::Other);
                spans.push(lit(Span::raw(item.text.clone()), on));
                Line::from(spans)
            })
            .collect();
        let name = if self.files { " Files " } else { " Commits " };
        let title = name.to_string() + &self.filter.title();
        let block = boxed(title, focused, ctx.look.theme);
        let inner = block.inner(area);
        f.render_widget(Paragraph::new(lines).block(block), area);
        for row in checked_rows {
            checked_bar(f, inner, inner.y + row as u16, ctx.look.theme);
        }
        if focused && cursor < items.len() && (top..top + height).contains(&cursor) {
            let y = inner.y + (cursor - top) as u16;
            cursor_bar(f, inner, y, ctx.look.theme);
        }
    }
}

fn views(ctx: &Ctx, checked: Option<Option<(usize, usize)>>) -> Vec<Item> {
    let within = |range: (usize, usize), v| (range.0..=range.1).contains(&v);
    let shown = ctx.review.shown.bounds(ctx.review.views.len());
    ctx.review
        .views
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
                View::Uncommitted(name) => name.to_string(),
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
    for (file, (_, path)) in ctx.review.doc.paths().iter().enumerate() {
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
            checked: Some(ctx.review.doc.collapsed(file)),
            indent: 2 * dirs.len(),
            text: name.to_string(),
        });
        prev = dirs;
    }
    items
}
