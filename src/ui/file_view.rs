use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::widgets::Clear;

use crate::app::{Component, Ctx, Outcome};
use crate::doc::Doc;
use crate::keymap::{Feed, Keymap};
use crate::ui::diff_view::DiffView;
use crate::ui::help::{Entry, Help};

const HELP: &[Entry] = &[
    (&["down", "up"], "move"),
    (&["half_page_down", "half_page_up"], "half page down / up"),
    (&["top", "bottom"], "top / bottom"),
    (
        &["next_waiting", "prev_waiting"],
        "next / previous thread waiting on you",
    ),
    (&["open"], "follow a thread's link"),
    (&["select"], "select lines"),
    (&["comment"], "comment; on a thread, reply"),
    (&["reply"], "reply to the thread"),
    (&["resolve"], "resolve or reopen the thread"),
    (
        &["edit", "delete"],
        "edit / delete your last unanswered message",
    ),
    (&["help"], "this help"),
    (&["quit", "cancel"], "back to the diff"),
];

/// A whole file at the range end, opened from a link; `q` / `Esc` go back.
pub struct FileView {
    view: DiffView,
    keymap: Keymap,
}

impl FileView {
    pub fn new(start: usize, end: usize) -> FileView {
        FileView {
            view: DiffView::file(start, end),
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
            "quit" | "cancel" => {
                ctx.file = None;
                Ok(Outcome::Close)
            }
            _ => self.view.act(action, ctx),
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        f.render_widget(Clear, area);
        self.view.render(f, area, ctx);
    }
}

/// Opens the first link in thread `idx` to a file that exists at the range end as
/// `ctx.file`; returns the rows of the linked first and last line.
pub fn follow(ctx: &mut Ctx, idx: usize) -> Result<Option<(usize, usize)>> {
    let links: Vec<_> = ctx.session.threads[idx]
        .messages
        .iter()
        .flat_map(|m| links(&m.body))
        .collect();
    for (path, start, end) in links {
        let Some(text) = ctx.repo.read(&ctx.session.end, &path)? else {
            continue;
        };
        let mut doc = Doc::file(ctx.session.end.clone(), path, &text);
        doc.set_threads(&ctx.session.threads);
        let rows = (doc.line_row(start), doc.line_row(end));
        ctx.file = Some(doc);
        return Ok(Some(rows));
    }
    Ok(None)
}

/// `path:line` and `path:start-end` in `text`, where the path has a `.` or `/`.
fn links(text: &str) -> Vec<(String, u32, u32)> {
    text.split(|c: char| !(c.is_alphanumeric() || "_./-:".contains(c)))
        .filter_map(|token| {
            let mut parts = token.split(':');
            let path = parts.next()?;
            let lines = parts.next()?.trim_end_matches('.');
            if !path.contains(['.', '/']) {
                return None;
            }
            let path = path.trim_start_matches("./");
            let (start, end) = match lines.split_once('-') {
                Some((a, b)) => (a.parse().ok()?, b.parse().ok()?),
                None => {
                    let n = lines.parse().ok()?;
                    (n, n)
                }
            };
            (start > 0 && start <= end).then(|| (path.to_string(), start, end))
        })
        .collect()
}
