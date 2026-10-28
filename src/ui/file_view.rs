use std::collections::HashSet;
use std::ops::{Range, RangeInclusive};

use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::app::{Component, Ctx, Outcome};
use crate::doc::Doc;
use crate::keymap::{Feed, Keymap};
use crate::session::Thread;
use crate::theme::Theme;
use crate::ui::boxed;
use crate::ui::clear;
use crate::ui::diff_view::{draw_row, draw_rows};
use crate::ui::help::{Entry, Help};
use crate::ui::popup::Popup;
use crate::ui::scroll::Scroll;
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
    doc: Doc,
    /// The link it was opened from, as `path:line`.
    label: String,
    /// The rows of the linked lines.
    linked: RangeInclusive<usize>,
    scroll: Scroll,
    keymap: Keymap,
}

impl FileView {
    /// The file of `link` at the range end, from its first linked line; None when absent.
    pub fn open(ctx: &mut Ctx, link: &Link) -> Result<Option<FileView>> {
        let Some(text) = ctx.repo.read(&ctx.session.end, &link.path)? else {
            return Ok(None);
        };
        let doc = Doc::file(ctx.session.end.clone(), link.path.clone(), &text);
        let (start, end) = (doc.line_row(link.start), doc.line_row(link.end));
        Ok(Some(FileView {
            doc,
            label: link.label(),
            linked: start..=end,
            scroll: Scroll::at(start),
            keymap: Keymap::default(),
        }))
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
            "quit" | "cancel" => Ok(Outcome::Close),
            _ => {
                self.scroll.act(action, self.doc.len());
                Ok(Outcome::Consumed)
            }
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        clear(f, area, ctx.theme);
        let block = boxed(format!(" {} ", self.label), true, ctx.theme);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let (doc, linked, cursor) = (&self.doc, &self.linked, self.scroll.cursor);
        let draw = |i: usize| {
            // The cursor as an accent block; linked lines as bold blue numbers, apart from
            // it in themes whose accent is blue.
            let mark = if i == cursor {
                Some(Style::new().fg(ctx.theme.on_accent).bg(ctx.theme.accent))
            } else if linked.contains(&i) {
                Some(Style::new().fg(ctx.theme.blue).add_modifier(Modifier::BOLD))
            } else {
                None
            };
            draw_row(ctx, doc, i, inner.width, mark, None)
        };
        self.scroll.place(
            inner.height as usize,
            doc.len(),
            |i| draw(i).height(),
            false,
        );
        draw_rows(f, inner, &self.scroll, doc, ctx.theme, draw, |_, _, _| 0);
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

    pub fn render(&mut self, f: &mut Frame, area: Rect, theme: &Theme) {
        self.popup.render(f, area, theme);
    }
}
