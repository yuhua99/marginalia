use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

use crate::app::{Component, Ctx, Outcome};
use crate::ui::clear;

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
        let rect = Rect {
            y: area.bottom() - 1,
            height: 1,
            ..area
        };
        clear(f, rect, ctx.theme);
        let text = "save threads waiting on the agent to clipboard? y/n (Esc to stay)";
        f.render_widget(
            Paragraph::new(text).style(Style::new().fg(ctx.theme.on_accent).bg(ctx.theme.accent)),
            rect,
        );
    }
}
