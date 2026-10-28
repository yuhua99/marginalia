use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

use crate::app::{Component, Ctx, Outcome};
use crate::ui::dialog;

/// `save to clipboard? y/n` when quitting a session without an on-quit hook.
pub struct QuitPrompt;

impl Component for QuitPrompt {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Result<Outcome> {
        Ok(match key.code {
            KeyCode::Char('y') => {
                ctx.copy = true;
                Outcome::Quit
            }
            KeyCode::Char('n') => Outcome::Quit,
            KeyCode::Esc => Outcome::Close,
            _ => Outcome::Consumed,
        })
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        dialog::confirm(
            f,
            area,
            "Quit",
            "save threads waiting on the agent to clipboard?",
            "y save  n don't save  Esc stay",
            ctx.theme,
        );
    }
}
