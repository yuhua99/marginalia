use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::app::{Component, Ctx, Outcome};
use crate::keymap::{Feed, Keymap};
use crate::ui::diff_view::DiffView;
use crate::ui::input::Input;
use crate::ui::quit_prompt::QuitPrompt;
use crate::ui::sidebar::{Entry, Sidebar};

/// The bottom layer: sidebar on the left, diff on the right; `h` / `l` move the focus.
#[derive(Default)]
pub struct Review {
    sidebar: Sidebar,
    diff: DiffView,
    sidebar_focused: bool,
    keymap: Keymap,
}

impl Component for Review {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Result<Outcome> {
        let Feed::Action(action) = self.keymap.feed(key) else {
            return Ok(Outcome::Consumed);
        };
        match action {
            "focus_left" => self.sidebar_focused = true,
            "focus_right" => self.sidebar_focused = false,
            "quit" => {
                if ctx.session.on_quit.is_none()
                    && ctx.session.markdown(&ctx.doc.thread_order()).is_some()
                {
                    return Ok(Outcome::Push(Box::new(QuitPrompt)));
                }
                return Ok(Outcome::Quit);
            }
            _ if self.sidebar_focused => match (action, self.sidebar.entry(ctx)) {
                ("open", Entry::View(view)) => {
                    ctx.show(self.sidebar.take_checked().unwrap_or((view, view)))?;
                    self.diff = DiffView::default();
                }
                ("open", Entry::File(file)) => {
                    self.diff.jump(ctx.doc.file_row(file));
                    self.sidebar_focused = false;
                }
                ("comment", entry) => {
                    if let Some(anchor) = entry.anchor(ctx) {
                        return Ok(Outcome::Push(Box::new(Input::comment(
                            anchor,
                            &ctx.session,
                        ))));
                    }
                }
                _ => self.sidebar.act(action, ctx),
            },
            _ => return self.diff.act(action, ctx),
        }
        Ok(Outcome::Consumed)
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let [left, right] =
            Layout::horizontal([Constraint::Length(32), Constraint::Fill(1)]).areas(area);
        self.sidebar.render(f, left, ctx, self.sidebar_focused);
        self.diff.render(f, right, ctx);
    }
}
