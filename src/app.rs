use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Paragraph;

use crate::doc::Doc;
use crate::repo::Repo;
use crate::session::Session;
use crate::store::Store;
use crate::ui::diff_view::DiffView;

/// State shared by all layers; layers keep only their own UI state.
pub struct Ctx {
    pub session: Session,
    pub repo: Repo,
    pub store: Store,
    pub doc: Doc,
    /// Indexed like `session.threads`.
    pub outdated: Vec<bool>,
    /// Copy the threads waiting on the agent to the clipboard after quitting.
    pub copy: bool,
}

impl Ctx {
    /// Saves the session and refreshes the thread rows after threads changed.
    pub fn threads_changed(&mut self) -> Result<()> {
        self.outdated.resize(self.session.threads.len(), false);
        self.store.save(&self.session)?;
        self.doc.set_threads(&self.session.threads);
        Ok(())
    }
}

pub enum Outcome {
    Consumed,
    Push(Box<dyn Component>),
    Close,
    Quit,
}

/// A layer of the screen. Layers render bottom-up; keys go to the top layer only.
pub trait Component {
    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx);
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Result<Outcome>;
}

const HINTS: &str = " c comment  r reply  x resolve  n/N next waiting  v select  q quit";

pub fn run(ctx: &mut Ctx) -> Result<()> {
    let mut layers: Vec<Box<dyn Component>> = vec![Box::new(DiffView::default())];
    ratatui::run(|terminal| {
        loop {
            terminal.draw(|f| {
                let [head, body, foot] = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Fill(1),
                    Constraint::Length(1),
                ])
                .areas(f.area());
                let s = &ctx.session;
                let header = format!(
                    "mrg  {} · {} · {} open · {} 等你",
                    s.range,
                    s.id,
                    s.open_count(),
                    s.waiting_on_user_count()
                );
                f.render_widget(
                    Paragraph::new(header).style(Style::new().add_modifier(Modifier::BOLD)),
                    head,
                );
                f.render_widget(
                    Paragraph::new(HINTS).style(Style::new().fg(Color::DarkGray)),
                    foot,
                );
                for layer in &mut layers {
                    layer.render(f, body, ctx);
                }
            })?;
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                match layers.last_mut().unwrap().handle_key(key, ctx)? {
                    Outcome::Consumed => {}
                    Outcome::Push(layer) => layers.push(layer),
                    Outcome::Close => {
                        layers.pop();
                    }
                    Outcome::Quit => return Ok(()),
                }
            }
        }
    })
}
