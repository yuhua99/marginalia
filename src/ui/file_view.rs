use std::collections::HashMap;
use std::ops::{Range, RangeInclusive};

use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};

use crate::app::{Component, Ctx, Outcome};
use crate::doc::Doc;
use crate::keymap::{Feed, Keymap};
use crate::session::Thread;
use crate::theme::Theme;
use crate::ui::boxed;
use crate::ui::clear;
use crate::ui::diff_view::{draw_row, draw_rows, drawn_at};
use crate::ui::help::{self, Entry, Help};
use crate::ui::popup::Popup;
use crate::ui::query::Query;
use crate::ui::scroll::Scroll;
use crate::ui::settings::SettingsPopup;

const HELP: &[Entry] = &[
    (&["down", "up"], "move"),
    (&["half_page_down", "half_page_up"], "half page down / up"),
    (&["top", "bottom"], "top / bottom"),
    (&["filter"], "search; Enter ends typing"),
    (&["next_thread", "prev_thread"], "next / previous match"),
    (&["settings"], "settings: theme, wrap"),
    (&["help"], "this help"),
    (
        &["quit", "cancel"],
        "back to the diff; Esc clears the search first",
    ),
];

const HINTS: &[Entry] = &[
    (&["down", "up"], "move"),
    (&["half_page_down", "half_page_up"], "half page"),
    (&["top", "bottom"], "top/bottom"),
    (&["filter"], "search"),
    (&["next_thread", "prev_thread"], "match"),
    (&["settings"], "settings"),
    (&["help"], "help"),
    (&["quit", "cancel"], "back"),
];

/// A whole file at the range end, opened from a link, read only and without threads;
/// the linked lines are marked in the gutter. `q` / `Esc` go back.
pub struct FileView {
    doc: Doc,
    /// The link it was opened from, as `path:line`.
    label: String,
    /// The rows of the linked lines.
    linked: RangeInclusive<usize>,
    scroll: Scroll,
    /// The `/` search; while typing, it takes all keys.
    search: Query,
    keymap: Keymap,
    /// The footer, from `HINTS`.
    hints: String,
    /// The rows drawn and their screen lines, as of the last render.
    rows: Vec<(usize, Range<u16>)>,
}

impl FileView {
    /// The file of `link` at the range end, from its first linked line; None when absent.
    pub fn open(ctx: &mut Ctx, link: &Link) -> Result<Option<FileView>> {
        let Some(text) = ctx.repo.read(&ctx.session.end, &link.path)? else {
            return Ok(None);
        };
        let doc = Doc::file(ctx.session.end.clone(), link.path.clone(), &text);
        let (start, end) = (doc.line_row(link.start), doc.line_row(link.end));
        let keymap = Keymap::new("file");
        Ok(Some(FileView {
            doc,
            label: link.label(),
            linked: start..=end,
            scroll: Scroll::centered(start),
            search: Query::default(),
            hints: help::hints(HINTS, &keymap),
            keymap,
            rows: Vec::new(),
        }))
    }
}

impl FileView {
    /// Moves to the next (or previous) search match.
    fn find(&mut self, forward: bool) {
        let cursor = self.scroll.cursor;
        // `Doc::file` has no thread rows.
        if let Some(row) = self.search.find(&mut self.doc, cursor, forward, &[]) {
            self.scroll.cursor = row;
        }
    }
}

impl Component for FileView {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Result<Outcome> {
        if self.search.typing {
            if self.search.key(key) {
                self.find(true);
            }
            return Ok(Outcome::Consumed);
        }
        let Feed::Action(action) = self.keymap.feed(key) else {
            return Ok(Outcome::Consumed);
        };
        match action {
            "help" => return Ok(Outcome::Push(Box::new(Help::new(HELP, &self.keymap)))),
            "settings" => {
                return Ok(Outcome::Push(Box::new(SettingsPopup::new(&ctx.look))));
            }
            "filter" => self.search = Query::start(),
            "next_thread" | "prev_thread" if !self.search.text.is_empty() => {
                self.find(action == "next_thread")
            }
            "cancel" if !self.search.text.is_empty() => self.search = Query::default(),
            "quit" | "cancel" => return Ok(Outcome::Close),
            _ => {
                self.scroll.act(action, self.doc.len());
            }
        }
        Ok(Outcome::Consumed)
    }

    fn paste(&mut self, text: &str) {
        if self.search.typing {
            self.search.paste(text);
        }
    }

    fn wheel(&mut self, action: &str, _at: Position, _ctx: &mut Ctx) -> Result<()> {
        if !self.search.typing {
            self.scroll.act(action, self.doc.len());
        }
        Ok(())
    }

    fn click(&mut self, at: Position, _ctx: &mut Ctx) -> Result<Outcome> {
        if !self.search.typing
            && let Some(row) = drawn_at(&self.rows, at.y)
        {
            self.scroll.cursor = row;
        }
        Ok(Outcome::Consumed)
    }

    fn hints(&self) -> Option<&str> {
        Some(&self.hints)
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        clear(f, area, ctx.look.theme);
        let title = format!(" {} {}", self.label, self.search.title());
        let block = boxed(title, true, ctx.look.theme);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let (doc, linked, cursor) = (&self.doc, &self.linked, self.scroll.cursor);
        let search = &self.search.text;
        let draw = |i: usize| {
            // The cursor as an accent block; linked lines as bold blue numbers, apart from
            // it in themes whose accent is blue.
            let mark = if i == cursor {
                Some(
                    Style::new()
                        .fg(ctx.look.theme.on_accent)
                        .bg(ctx.look.theme.accent),
                )
            } else if linked.contains(&i) {
                Some(
                    Style::new()
                        .fg(ctx.look.theme.blue)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                None
            };
            draw_row(ctx, doc, i, inner.width, mark, None, search)
        };
        self.scroll.place(
            inner.height as usize,
            doc.len(),
            |i| draw(i).height(),
            false,
        );
        self.rows = draw_rows(f, inner, &self.scroll, doc, draw, |_, _, _| 0);
    }
}

/// A `path:line` or `path:start-end` in a thread, to a file at the range end.
#[derive(Clone, PartialEq)]
pub struct Link {
    pub path: String,
    pub start: u32,
    pub end: u32,
}

impl Link {
    pub fn label(&self) -> String {
        if self.start == self.end {
            format!("{}:{}", self.path, self.start)
        } else {
            format!("{}:{}-{}", self.path, self.start, self.end)
        }
    }
}

/// Paths of the files at the range end, which links in threads may open.
pub struct Files {
    /// The paths by their file name.
    by_name: HashMap<String, Vec<String>>,
}

impl Files {
    pub fn new(paths: impl IntoIterator<Item = String>) -> Files {
        let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
        for path in paths {
            let name = path.rsplit_once('/').map_or(path.as_str(), |(_, n)| n);
            by_name.entry(name.to_string()).or_default().push(path);
        }
        Files { by_name }
    }

    /// The path that is `path`, or else the only one ending in `/path`.
    pub fn find(&self, path: &str) -> Option<&str> {
        let name = path.rsplit_once('/').map_or(path, |(_, n)| n);
        let paths = self.by_name.get(name)?;
        if let Some(p) = paths.iter().find(|p| *p == path) {
            return Some(p);
        }
        let mut hits = paths
            .iter()
            .filter(|p| p.strip_suffix(path).is_some_and(|rest| rest.ends_with('/')));
        match (hits.next(), hits.next()) {
            (Some(p), None) => Some(p),
            _ => None,
        }
    }
}

/// The links in `text` to paths in `files`, with their byte ranges in `text`.
pub fn links(text: &str, files: &Files) -> Vec<(Range<usize>, Link)> {
    text.split(|c: char| !(c.is_alphanumeric() || "_./-:".contains(c)))
        .filter_map(|token| {
            let token = token.trim_end_matches(['.', ':']);
            let at = token.as_ptr() as usize - text.as_ptr() as usize;
            let mut parts = token.split(':');
            let path = parts.next()?.trim_start_matches("./");
            let lines = parts.next()?;
            let path = files.find(path)?;
            let (start, end) = match lines.split_once('-') {
                Some((a, b)) => (a.parse().ok()?, b.parse().ok()?),
                None => {
                    let n = lines.parse().ok()?;
                    (n, n)
                }
            };
            let link = Link {
                path: path.to_string(),
                start,
                end,
            };
            (start > 0 && start <= end).then_some((at..at + token.len(), link))
        })
        .collect()
}

/// The links in thread `t`, each once, in order.
pub fn thread_links(t: &Thread, files: &Files) -> Vec<Link> {
    let mut out: Vec<Link> = Vec::new();
    for (_, link) in t.messages.iter().flat_map(|m| links(&m.body, files)) {
        if !out.contains(&link) {
            out.push(link);
        }
    }
    out
}

/// Enter on a thread with several links: a list of them to pick one.
pub struct LinkPicker {
    popup: Popup,
    links: Vec<Link>,
    keymap: Keymap,
}

impl LinkPicker {
    pub fn new(links: Vec<Link>) -> LinkPicker {
        let rows = links.iter().map(|l| (l.label(), String::new())).collect();
        LinkPicker {
            popup: Popup::new("Links", rows),
            links,
            keymap: Keymap::new("popup"),
        }
    }

    /// None while the list stays open; then the picked link, or None when cancelled.
    pub fn key(&mut self, key: KeyEvent) -> Option<Option<Link>> {
        let Feed::Action(action) = self.keymap.feed(key) else {
            return None;
        };
        match action {
            "open" | "focus_right" => Some(Some(self.links[self.popup.cursor].clone())),
            "cancel" | "quit" | "focus_left" => Some(None),
            _ => {
                self.popup.act(action);
                None
            }
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, theme: &Theme) {
        self.popup.render(f, area, theme);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find() {
        let files = Files::new(
            [
                "src/a.rs",
                "src/ui/a.rs",
                "lib/a.rs",
                "x/b.rs",
                "b.rs",
                "src/c.rs",
            ]
            .map(String::from),
        );
        assert_eq!(files.find("src/c.rs"), Some("src/c.rs"));
        assert_eq!(files.find("c.rs"), Some("src/c.rs"));
        assert_eq!(files.find("ui/a.rs"), Some("src/ui/a.rs"));
        assert_eq!(files.find("a.rs"), None);
        assert_eq!(files.find("b.rs"), Some("b.rs"));
        assert_eq!(files.find("rc/c.rs"), None);
        assert_eq!(files.find("d.rs"), None);
    }

    #[test]
    fn links_resolve_suffixes() {
        let files = Files::new(["src/x/controller.cpp".to_string()]);
        let text = "see controller.cpp:294-296.";
        let found = links(text, &files);
        assert_eq!(found.len(), 1);
        assert_eq!(&text[found[0].0.clone()], "controller.cpp:294-296");
        assert_eq!(found[0].1.label(), "src/x/controller.cpp:294-296");
    }
}
