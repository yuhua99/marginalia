use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::app::{Component, Ctx, Outcome, Shown};
use crate::keymap::{Feed, Keymap};
use crate::ui::boxed;
use crate::ui::confirm::Confirm;
use crate::ui::diff_view::DiffView;
use crate::ui::help::{self, Help};
use crate::ui::input::Input;
use crate::ui::settings::SettingsPopup;
use crate::ui::sidebar::{Entry, Sidebar};

const HELP: &[help::Entry] = &[
    (&["down", "up"], "move"),
    (&["half_page_down", "half_page_up"], "half page down / up"),
    (&["top", "bottom"], "top / bottom"),
    (
        &["next_thread", "prev_thread"],
        "next / previous thread, or search match while searching",
    ),
    (&["next_file", "prev_file"], "next / previous file"),
    (
        &["focus_left", "focus_right"],
        "focus commits <- files <- diff",
    ),
    (
        &["open"],
        "expand fold, follow a thread's link (from a list when several), show the checked commits (or this one) or open a file",
    ),
    (&["check"], "check commits for Enter to show together"),
    (&["select"], "select lines"),
    (
        &["filter"],
        "search the diff; in files, filter them; Enter ends typing",
    ),
    (
        &["cancel"],
        "clear the selection, then the search or filter",
    ),
    (&["comment"], "comment; on a thread, reply"),
    (&["collapse"], "collapse or expand the file"),
    (
        &["edit", "delete"],
        "edit / delete your last unanswered message",
    ),
    (&["settings"], "settings: theme, wrap"),
    (&["help"], "this help"),
    (&["back"], "back to the picker"),
    (&["quit"], "quit"),
];

const HINTS: &str = " c comment/reply  r collapse  e/d edit/delete  n/N next thread/match  / search/filter  v select  J/K file  h/l focus  Space check  Enter open  s settings  ? help  Q picker  q quit";

/// The bottom layer: commits across the top, files and the diff below.
/// `h` / `l` move the focus along commits, files, diff.
pub struct Review {
    commits: Sidebar,
    files: Sidebar,
    diff: DiffView,
    /// 0 commits, 1 files, 2 diff.
    focus: usize,
    keymap: Keymap,
}

const COMMITS: usize = 0;
const FILES: usize = 1;
const DIFF: usize = 2;

impl Review {
    pub fn new() -> Review {
        Review {
            commits: Sidebar::default(),
            files: Sidebar::files(),
            diff: DiffView::default(),
            focus: DIFF,
            keymap: Keymap::default(),
        }
    }
}

impl Component for Review {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Result<Outcome> {
        if let Some(outcome) = self.diff.modal_key(key, ctx)? {
            return Ok(outcome);
        }
        if self.files.filter_key(key) {
            return Ok(Outcome::Consumed);
        }
        let Feed::Action(action) = self.keymap.feed(key) else {
            return Ok(Outcome::Consumed);
        };
        let focus = self.focus;
        match action {
            "focus_left" => self.focus = self.focus.saturating_sub(1),
            "focus_right" => self.focus = (self.focus + 1).min(DIFF),
            "help" => return Ok(Outcome::Push(Box::new(Help::new(HELP)))),
            "settings" => {
                return Ok(Outcome::Push(Box::new(SettingsPopup::new(&ctx.look))));
            }
            // Leaving for the picker ends the session like quitting does.
            "quit" | "back" => {
                ctx.back = action == "back";
                let order = ctx.doc.thread_order();
                if ctx.on_quit.is_none() && ctx.session.markdown(&order).is_some() {
                    return Ok(Outcome::Push(Box::new(
                        Confirm::new(
                            "Quit",
                            "save threads waiting on the agent to clipboard?",
                            "y save  n don't save  Esc stay",
                        )
                        .on('y', |ctx: &mut Ctx| {
                            ctx.copy = true;
                            Ok(Outcome::Quit)
                        })
                        .on('n', |_| Ok(Outcome::Quit)),
                    )));
                }
                return Ok(Outcome::Quit);
            }
            _ if self.focus != DIFF => {
                let list = if self.focus == FILES {
                    &mut self.files
                } else {
                    &mut self.commits
                };
                match (action, list.entry(ctx)) {
                    ("open", Entry::View(view)) => {
                        let run = self.commits.checked().unwrap_or((view, view));
                        ctx.show(Shown::of(run, ctx.views.len()))?;
                        self.diff = DiffView::default();
                        self.focus = DIFF;
                    }
                    ("open", Entry::File(file)) => {
                        self.diff.jump(ctx.doc.file_row(file));
                        self.focus = DIFF;
                    }
                    ("comment", entry) => {
                        if let Some(anchor) = entry.anchor(ctx) {
                            // Under the file's header, or the session's for commits.
                            let row = match entry {
                                Entry::File(file) => ctx.doc.expand_file(file),
                                _ => 0,
                            };
                            self.diff.start(row, Input::comment(anchor, &ctx.session));
                            self.focus = DIFF;
                        }
                    }
                    _ => list.act(action, ctx),
                }
            }
            _ => return self.diff.act(action, ctx),
        }
        if (focus == COMMITS) != (self.focus == COMMITS) {
            self.commits.focus(ctx, self.focus == COMMITS);
        }
        Ok(Outcome::Consumed)
    }

    fn paste(&mut self, text: &str) {
        self.files.paste(text);
        self.diff.paste(text);
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        // Commits as tall as their rows, up to 4.
        let rows = ctx.views.len().min(4) as u16;
        let [top, bottom] =
            Layout::vertical([Constraint::Length(rows + 2), Constraint::Fill(1)]).areas(area);
        let [left, right] =
            Layout::horizontal([Constraint::Length(32), Constraint::Fill(1)]).areas(bottom);
        self.commits.render(f, top, ctx, self.focus == COMMITS);
        self.files.render(f, left, ctx, self.focus == FILES);
        let block = boxed(
            Span::styled(
                header(ctx, self.diff.current_file(ctx)) + &self.diff.search_title(),
                Style::new().add_modifier(Modifier::BOLD),
            ),
            self.focus == DIFF,
            ctx.look.theme,
        );
        let inner = block.inner(right);
        f.render_widget(block, right);
        self.diff.render(f, inner, ctx);
    }

    fn hints(&self) -> Option<&'static str> {
        Some(HINTS)
    }
}

/// The path of `file`, else the session name, if any.
fn header(ctx: &Ctx, file: Option<usize>) -> String {
    let name = match file {
        Some(file) => Some(&ctx.doc.paths()[file].1),
        None => ctx.session.name.as_ref(),
    };
    name.map_or(String::new(), |n| format!(" {n} "))
}
