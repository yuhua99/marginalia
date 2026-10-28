use std::collections::HashSet;
use std::ops::Range;

use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use crate::app::{Component, Ctx, Outcome};
use crate::doc::Doc;
use crate::keymap::{Feed, Keymap};
use crate::session::Thread;
use crate::theme::Theme;
use crate::ui::boxed;
use crate::ui::clear;
use crate::ui::diff_view::DiffView;
use crate::ui::help::{Entry, Help};
use crate::ui::popup::Popup;
use crate::ui::settings::SettingsPopup;

const HELP: &[Entry] = &[
    (&["down", "up"], "move"),
    (&["half_page_down", "half_page_up"], "half page down / up"),
    (&["top", "bottom"], "top / bottom"),
    (&["settings"], "settings: theme, wrap"),
    (&["help"], "this help"),
    (&["quit", "cancel"], "back to the diff"),
];

/// A whole file at the range end, opened from a link, read only and without threads;
/// the linked lines are marked in the gutter. `q` / `Esc` go back.
pub struct FileView {
    view: DiffView,
    keymap: Keymap,
}

impl FileView {
    pub fn new(start: usize, end: usize, link: &Link) -> FileView {
        FileView {
            view: DiffView::file(start, end, link),
            keymap: Keymap::default(),
        }
    }
}

impl Component for FileView {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Result<Outcome> {
        let Feed::Action(action) = self.keymap.feed(key) else {
            return Ok(Outcome::Consumed);
        };
        match action {
            "help" => Ok(Outcome::Push(Box::new(Help::new(HELP)))),
            "settings" => Ok(Outcome::Push(Box::new(SettingsPopup::new(
                ctx.theme, ctx.wrap,
            )))),
            "quit" | "cancel" => {
                ctx.file = None;
                Ok(Outcome::Close)
            }
            _ => self.view.act(action, ctx),
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        clear(f, area, ctx.theme);
        let block = boxed(format!(" {} ", self.view.link()), true, ctx.theme);
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.view.render(f, inner, ctx);
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

/// The links in `text` to paths in `files`, with their byte ranges in `text`.
pub fn links(text: &str, files: &HashSet<String>) -> Vec<(Range<usize>, Link)> {
    text.split(|c: char| !(c.is_alphanumeric() || "_./-:".contains(c)))
        .filter_map(|token| {
            let token = token.trim_end_matches(['.', ':']);
            let at = token.as_ptr() as usize - text.as_ptr() as usize;
            let mut parts = token.split(':');
            let path = parts.next()?.trim_start_matches("./");
            let lines = parts.next()?;
            if !files.contains(path) {
                return None;
            }
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
pub fn thread_links(t: &Thread, files: &HashSet<String>) -> Vec<Link> {
    let mut out: Vec<Link> = Vec::new();
    for (_, link) in t.messages.iter().flat_map(|m| links(&m.body, files)) {
        if !out.contains(&link) {
            out.push(link);
        }
    }
    out
}

/// Opens the file of `link` at the range end as `ctx.file`; returns the rows of the
/// linked first and last line.
pub fn open(ctx: &mut Ctx, link: &Link) -> Result<Option<(usize, usize)>> {
    let Some(text) = ctx.repo.read(&ctx.session.end, &link.path)? else {
        return Ok(None);
    };
    let doc = Doc::file(ctx.session.end.clone(), link.path.clone(), &text);
    let rows = (doc.line_row(link.start), doc.line_row(link.end));
    ctx.file = Some(doc);
    Ok(Some(rows))
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
            keymap: Keymap::default(),
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

    pub fn render(&self, f: &mut Frame, area: Rect, theme: &Theme) {
        self.popup.render(f, area, theme);
    }
}
