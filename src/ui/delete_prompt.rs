use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::{Component, Ctx, Outcome};

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
        let rect = Rect {
            y: area.bottom() - 1,
            height: 1,
            ..area
        };
        f.render_widget(Clear, rect);
        let t = &ctx.session.threads[self.0];
        let text = if t.messages.len() == 1 {
            format!("delete thread {}? y/n", t.id)
        } else {
            "delete your last message? y/n".to_string()
        };
        f.render_widget(
            Paragraph::new(text).style(Style::new().fg(Color::Black).bg(Color::Yellow)),
            rect,
        );
    }
}
