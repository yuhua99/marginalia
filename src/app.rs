use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::widgets::Paragraph;

use crate::session::Session;

/// TUI shell: shows the session header until `q`.
pub fn run(session: &Session) -> Result<()> {
    let header = format!(
        "mrg  {} · {} · {} open · {} 等你",
        session.range,
        session.id,
        session.open_count(),
        session.waiting_on_user_count()
    );
    ratatui::run(|terminal| loop {
        terminal.draw(|f| f.render_widget(Paragraph::new(header.as_str()), f.area()))?;
        if let Event::Key(k) = event::read()?
            && k.kind == KeyEventKind::Press
            && k.code == KeyCode::Char('q')
        {
            return Ok(());
        }
    })
}
