use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Layout, Margin, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::app::{Component, Ctx, Mouse, Outcome};
use crate::keymap::{Feed, Keymap};
use crate::review::Shown;
use crate::session::Anchor;
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
    (
        &["prev_file", "next_file"],
        "previous / next file in the diff",
    ),
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
    (&["help"], "help"),
    (&["back"], "picker"),
    (&["quit"], "quit"),
];

/// The bottom layer: commits across the top, files and the diff below.
/// `Tab` / `S-Tab` cycle the focus along commits, files, diff; `h` / `l` or `Left` /
/// `Right` move by file.
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
        let keymap = Keymap::new("review");
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

    /// Shows the checked commits, unless shown already.
    fn show_checked(&mut self, ctx: &mut Ctx) -> Result<()> {
        if let Some(run) = self.commits.checked()
            && run != ctx.review.shown.bounds(ctx.review.views.len())
        {
            self.show(run, ctx)?;
        }
        Ok(())
    }

    /// Shows the views `run`, the diff from its top.
    fn show(&mut self, run: (usize, usize), ctx: &mut Ctx) -> Result<()> {
        ctx.review.show(Shown::of(run, ctx.review.views.len()))?;
        self.diff = DiffView::default();
        Ok(())
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
                let order = ctx.review.doc.thread_order();
                if ctx.on_quit.is_none() && ctx.review.session.markdown(&order).is_some() {
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
                list.settle();
                match (action, list.entry(ctx)) {
                    ("open", Entry::View(view)) => {
                        let run = self.commits.checked().unwrap_or((view, view));
                        self.show(run, ctx)?;
                        self.focus = DIFF;
                    }
                    ("open", Entry::File(file)) => {
                        self.diff.jump(ctx.review.doc.file_row(file));
                        self.focus = DIFF;
                    }
                    ("comment", entry) => {
                        if let Some(anchor) = entry.anchor(ctx) {
                            // Under the file's header, the commit's when shown alone,
                            // else the session's.
                            let row = match (entry, &anchor) {
                                (Entry::File(file), _) => ctx.review.doc.expand_file(file),
                                (_, Anchor::Commit { commit }) => {
                                    ctx.review.doc.commit_row(commit).unwrap_or(0)
                                }
                                _ => 0,
                            };
                            self.diff
                                .start(row, Input::comment(anchor, &ctx.review.session));
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

    fn focus(&mut self, pane: usize, ctx: &mut Ctx) {
        let focus = self.focus;
        self.focus = pane;
        self.refocus(focus, ctx);
    }

    fn scroll_focused(&mut self, down: bool, _ctx: &mut Ctx) {
        match self.focus {
            COMMITS => self.commits.wheel(down),
            FILES => self.files.wheel(down),
            _ => self.diff.wheel(down),
        }
    }

    fn busy(&self) -> bool {
        self.diff.busy() || self.files.typing()
    }

    /// A commit checks alone, the start of a drag over the commits to show; a file opens
    /// as Enter does; a diff row takes the cursor, the start of a drag over the lines to
    /// select.
    fn press(&mut self, pane: usize, y: u16, ctx: &mut Ctx) -> Result<Outcome> {
        match pane {
            COMMITS => self.commits.press(self.panes[COMMITS], y, ctx),
            FILES if self.files.click(self.panes[FILES], y) => return self.act("open", ctx),
            DIFF => self.diff.press(y),
            _ => {}
        }
        Ok(Outcome::Consumed)
    }

    /// Over the commits, checks those dragged over; over the diff, selects the lines.
    fn drag(&mut self, pane: usize, y: u16, ctx: &mut Ctx) {
        match pane {
            COMMITS => self.commits.drag(self.panes[COMMITS], y, ctx),
            DIFF => self.diff.drag(y, ctx.review.doc.len()),
            _ => {}
        }
    }

    /// Letting go over the commits shows those checked.
    fn release(&mut self, pane: usize, ctx: &mut Ctx) -> Result<Outcome> {
        match pane {
            COMMITS if self.commits.release() => self.show_checked(ctx)?,
            DIFF => self.diff.release(),
            _ => {}
        }
        Ok(Outcome::Consumed)
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

    fn mouse(&mut self, m: Mouse, at: Position, ctx: &mut Ctx) -> Result<Outcome> {
        self.mouse_at(m, at, ctx)
    }

    /// The box under `at` and its row there.
    fn target(&self, at: Position) -> Option<(usize, Option<usize>)> {
        let pane = self.pane_at(at)?;
        let row = match pane {
            COMMITS => self.commits.row_at(self.panes[COMMITS], at.y),
            FILES => self.files.row_at(self.panes[FILES], at.y),
            // Inside the borders, where the diff underlines.
            _ if self.panes[DIFF].inner(Margin::new(1, 1)).contains(at) => self.diff.row_at(at.y),
            _ => None,
        }?;
        Some((pane, Some(row)))
    }

    fn paste(&mut self, text: &str) {
        self.files.paste(text);
        self.diff.paste(text);
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx, pointer: Option<Position>) {
        // Typing leaves the mouse alone.
        let pointer = pointer.filter(|_| !self.busy());
        // Commits as tall as their rows, up to 4.
        let rows = ctx.review.views.len().min(4) as u16;
        let [top, bottom] =
            Layout::vertical([Constraint::Length(rows + 2), Constraint::Fill(1)]).areas(area);
        let [left, right] =
            Layout::horizontal([Constraint::Length(32), Constraint::Fill(1)]).areas(bottom);
        self.panes = [top, left, right];
        self.commits
            .render(f, top, ctx, self.focus == COMMITS, pointer);
        self.files
            .render(f, left, ctx, self.focus == FILES, pointer);
        let block = boxed(
            Span::styled(
                header(ctx, self.diff.current_file(ctx)) + &self.diff.search_title(ctx),
                Style::new().add_modifier(Modifier::BOLD),
            ),
            self.focus == DIFF,
            ctx.look.theme,
        );
        let inner = block.inner(right);
        f.render_widget(block, right);
        self.diff.render(f, inner, ctx, pointer);
    }

    fn hints(&self) -> Option<&str> {
        Some(&self.hints)
    }
}

/// The path of `file`, else the session name, if any.
fn header(ctx: &Ctx, file: Option<usize>) -> String {
    let name = match file {
        Some(file) => Some(&ctx.review.doc.paths()[file].1),
        None => ctx.review.session.name.as_ref(),
    };
    name.map_or(String::new(), |n| format!(" {n} "))
}
