use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::app::{Component, Ctx, Outcome, Shown};
use crate::keymap::{Feed, Keymap};
use crate::ui::confirm::Confirm;
use crate::ui::diff_view::DiffView;
use crate::ui::help::{self, Help};
use crate::ui::input::Input;
use crate::ui::settings::SettingsPopup;
use crate::ui::sidebar::{Entry, Sidebar};
use crate::ui::{Panes, boxed};

const HELP: &[help::Entry] = &[
    (&["down", "up"], "move"),
    (&["half_page_down", "half_page_up"], "half page down / up"),
    (&["top", "bottom"], "top / bottom"),
    (
        &["next_thread", "prev_thread"],
        "next / previous thread, or search match while searching",
    ),
    (&["prev_file", "next_file"], "previous / next file in the diff"),
    (
        &["next_focus", "prev_focus"],
        "cycle focus: commits, files, diff",
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

const HINTS: &[help::Entry] = &[
    (&["comment"], "comment/reply"),
    (&["collapse"], "collapse"),
    (&["edit", "delete"], "edit/delete"),
    (&["next_thread", "prev_thread"], "next thread/match"),
    (&["filter"], "search/filter"),
    (&["select"], "select"),
    (&["prev_file", "next_file"], "file"),
    (&["next_focus", "prev_focus"], "focus"),
    (&["check"], "check"),
    (&["open"], "open"),
    (&["settings"], "settings"),
    (&["help"], "help"),
    (&["back"], "picker"),
    (&["quit"], "quit"),
];

/// The bottom layer: commits across the top, files and the diff below.
/// `Tab` / `S-Tab` cycle the focus along commits, files, diff; `h` / `l` move by file.
pub struct Review {
    commits: Sidebar,
    files: Sidebar,
    diff: DiffView,
    /// 0 commits, 1 files, 2 diff.
    focus: usize,
    keymap: Keymap,
    /// The footer, from `HINTS`.
    hints: String,
    /// The commits, files and diff boxes, as of the last render.
    panes: [Rect; 3],
}

const COMMITS: usize = 0;
const FILES: usize = 1;
const DIFF: usize = 2;

impl Review {
    pub fn new() -> Review {
        let keymap = Keymap::with(&[
            ("h", "prev_file"),
            ("l", "next_file"),
            ("Tab", "next_focus"),
            ("S-Tab", "prev_focus"),
        ]);
        Review {
            commits: Sidebar::default(),
            files: Sidebar::files(),
            diff: DiffView::default(),
            focus: DIFF,
            hints: help::hints(HINTS, &keymap),
            keymap,
            panes: [Rect::default(); 3],
        }
    }

    /// Entering or leaving the commits box, from `focus`, checks or drops its views.
    fn refocus(&mut self, focus: usize, ctx: &Ctx) {
        if (focus == COMMITS) != (self.focus == COMMITS) {
            self.commits.focus(ctx, self.focus == COMMITS);
        }
    }
}

impl Panes<Ctx> for Review {
    fn act(&mut self, action: &str, ctx: &mut Ctx) -> Result<Outcome> {
        let focus = self.focus;
        match action {
            "next_focus" => self.focus = (self.focus + 1) % 3,
            "prev_focus" => self.focus = (self.focus + 2) % 3,
            "help" => return Ok(Outcome::Push(Box::new(Help::new(HELP, &self.keymap)))),
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
        self.refocus(focus, ctx);
        Ok(Outcome::Consumed)
    }

    fn panes(&self) -> &[Rect] {
        &self.panes
    }

    fn focus(&mut self, pane: usize, ctx: &Ctx) {
        let focus = self.focus;
        self.focus = pane;
        self.refocus(focus, ctx);
    }

    fn busy(&self) -> bool {
        self.diff.busy() || self.files.typing()
    }

    /// A commit checks as Space does, a file opens as Enter does; a diff row only takes
    /// the cursor.
    fn pick(&mut self, pane: usize, y: u16, ctx: &mut Ctx) -> Result<Outcome> {
        let action = match pane {
            COMMITS if self.commits.click(self.panes[COMMITS], y, ctx) => "check",
            FILES if self.files.click(self.panes[FILES], y, ctx) => "open",
            DIFF => {
                self.diff.click(y);
                return Ok(Outcome::Consumed);
            }
            _ => return Ok(Outcome::Consumed),
        };
        self.act(action, ctx)
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
        self.act(action, ctx)
    }

    fn wheel(&mut self, action: &str, at: Position, ctx: &mut Ctx) -> Result<()> {
        self.wheel_at(action, at, ctx)
    }

    fn click(&mut self, at: Position, ctx: &mut Ctx) -> Result<Outcome> {
        self.click_at(at, ctx)
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
        self.panes = [top, left, right];
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

    fn hints(&self) -> Option<&str> {
        Some(&self.hints)
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
