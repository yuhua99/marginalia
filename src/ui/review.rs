use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::app::{Component, Ctx, Outcome, View};
use crate::keymap::{Feed, Keymap};
use crate::ui::boxed;
use crate::ui::diff_view::DiffView;
use crate::ui::help::{self, Help};
use crate::ui::input::Input;
use crate::ui::quit_prompt::QuitPrompt;
use crate::ui::settings::SettingsPopup;
use crate::ui::sidebar::{Entry, Sidebar};

const HELP: &[help::Entry] = &[
    (&["down", "up"], "move"),
    (&["half_page_down", "half_page_up"], "half page down / up"),
    (&["top", "bottom"], "top / bottom"),
    (
        &["next_waiting", "prev_waiting"],
        "next / previous thread waiting on you",
    ),
    (&["next_file", "prev_file"], "next / previous file"),
    (
        &["focus_left", "focus_right"],
        "focus files <- commits <- diff",
    ),
    (
        &["open"],
        "expand fold, follow a thread's link, open a view or file",
    ),
    (&["check"], "check commits to view together"),
    (&["select"], "select lines"),
    (&["cancel"], "clear the selection or checks"),
    (&["comment"], "comment; on a thread, reply"),
    (&["reply"], "reply to the thread"),
    (&["resolve"], "resolve or reopen the thread"),
    (
        &["edit", "delete"],
        "edit / delete your last unanswered message",
    ),
    (&["settings"], "settings: theme"),
    (&["help"], "this help"),
    (&["quit"], "quit"),
];

/// The bottom layer: commits over files on the left, the diff on the right.
/// `h` / `l` move the focus along files, commits, diff.
pub struct Review {
    commits: Sidebar,
    files: Sidebar,
    diff: DiffView,
    /// 0 files, 1 commits, 2 diff.
    focus: usize,
    keymap: Keymap,
}

const FILES: usize = 0;
const COMMITS: usize = 1;
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
        let Feed::Action(action) = self.keymap.feed(key) else {
            return Ok(Outcome::Consumed);
        };
        match action {
            "focus_left" => self.focus = self.focus.saturating_sub(1),
            "focus_right" => self.focus = (self.focus + 1).min(DIFF),
            "help" => return Ok(Outcome::Push(Box::new(Help::new(HELP)))),
            "settings" => return Ok(Outcome::Push(Box::new(SettingsPopup::new(ctx.theme)))),
            "quit" => {
                if ctx.session.on_quit.is_none()
                    && ctx.session.markdown(&ctx.doc.thread_order()).is_some()
                {
                    return Ok(Outcome::Push(Box::new(QuitPrompt)));
                }
                return Ok(Outcome::Quit);
            }
            _ if self.focus != DIFF => {
                // Checks live in the commits box; Esc clears them from either box.
                if action == "cancel" {
                    self.commits.take_checked();
                }
                let list = if self.focus == FILES {
                    &mut self.files
                } else {
                    &mut self.commits
                };
                match (action, list.entry(ctx)) {
                    ("open", Entry::View(view)) => {
                        ctx.show(self.commits.take_checked().unwrap_or((view, view)))?;
                        self.diff = DiffView::default();
                    }
                    ("open", Entry::File(file)) => {
                        self.diff.jump(ctx.doc.file_row(file));
                        self.focus = DIFF;
                    }
                    ("comment", entry) => {
                        if let Some(anchor) = entry.anchor(ctx) {
                            return Ok(Outcome::Push(Box::new(Input::comment(
                                anchor,
                                &ctx.session,
                            ))));
                        }
                    }
                    _ => list.act(action, ctx),
                }
            }
            _ => return self.diff.act(action, ctx),
        }
        Ok(Outcome::Consumed)
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let [left, right] =
            Layout::horizontal([Constraint::Length(32), Constraint::Fill(1)]).areas(area);
        // Commits as tall as their rows, up to 40%.
        let commits = (ctx.views.len() as u16 + 2).min(left.height * 2 / 5);
        let [top, bottom] =
            Layout::vertical([Constraint::Length(commits), Constraint::Fill(1)]).areas(left);
        self.commits.render(f, top, ctx, self.focus == COMMITS);
        self.files.render(f, bottom, ctx, self.focus == FILES);
        let block = boxed(
            Span::styled(header(ctx), Style::new().add_modifier(Modifier::BOLD)),
            self.focus == DIFF,
            ctx.theme,
        );
        let inner = block.inner(right);
        f.render_widget(block, right);
        self.diff.render(f, inner, ctx);
    }
}

/// The range, id, thread counts and the views shown.
fn header(ctx: &Ctx) -> String {
    let s = &ctx.session;
    let mut header = format!(
        " {} / {} / {} open / {} waiting on you",
        s.range,
        s.id,
        s.open_count(),
        s.waiting_on_user_count()
    );
    let label = |v: usize| match &ctx.views[v] {
        View::All => "all",
        View::Commit { sha, .. } => &sha[..7],
        View::Uncommitted => &s.end,
    };
    match ctx.view {
        (0, 0) => {}
        (first, last) if first == last => header.push_str(&format!(" / viewing {}", label(first))),
        (first, last) => header.push_str(&format!(" / viewing {}..{}", label(first), label(last))),
    }
    header.push(' ');
    header
}
