use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Paragraph;

use crate::doc::Doc;
use crate::keymap::{Feed, Keymap};
use crate::session::Session;
use crate::ui::diff_view::DiffView;

pub fn run(session: &Session, doc: &Doc) -> Result<()> {
    let header = format!(
        "mrg  {} · {} · {} open · {} 等你",
        session.range,
        session.id,
        session.open_count(),
        session.waiting_on_user_count()
    );
    let mut keymap = Keymap::default();
    let mut view = DiffView::default();
    ratatui::run(|terminal| {
        loop {
            terminal.draw(|f| {
                let [head, body] =
                    Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(f.area());
                f.render_widget(
                    Paragraph::new(header.as_str())
                        .style(Style::new().add_modifier(Modifier::BOLD)),
                    head,
                );
                view.render(f, body, doc);
            })?;
            if let Event::Key(k) = event::read()?
                && k.kind == KeyEventKind::Press
                && let Feed::Action(action) = keymap.feed(k)
            {
                if action == "quit" {
                    return Ok(());
                }
                view.act(action, doc);
            }
        }
    })
}
