use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

use crate::app::{Component, Ctx, Outcome};
use crate::ui::dialog;

/// `delete ...? y/n` for the user's last message of thread `.0`.
pub struct DeletePrompt(pub usize);

impl Component for DeletePrompt {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Result<Outcome> {
        Ok(match key.code {
            KeyCode::Char('y') => {
                if ctx.session.delete(self.0) {
                    ctx.outdated.remove(self.0);
                }
                ctx.threads_changed()?;
                Outcome::Close
            }
            KeyCode::Char('n') | KeyCode::Esc => Outcome::Close,
            _ => Outcome::Consumed,
        })
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.session.threads[self.0];
        let text = if t.messages.len() == 1 {
            format!("delete thread {}?", t.id)
        } else {
            "delete your last message?".to_string()
        };
        dialog::confirm(
            f,
            area,
            "Delete",
            &text,
            "y delete  n/Esc cancel",
            ctx.theme,
        );
    }
}
