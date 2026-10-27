use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::{Component, Ctx, Outcome};

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

    fn render(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let rect = Rect {
            y: area.bottom() - 1,
            height: 1,
            ..area
        };
        f.render_widget(Clear, rect);
        let text = "save threads waiting on the agent to clipboard? y/n (Esc to stay)";
        f.render_widget(
            Paragraph::new(text).style(Style::new().fg(Color::Black).bg(Color::Yellow)),
            rect,
        );
    }
}
