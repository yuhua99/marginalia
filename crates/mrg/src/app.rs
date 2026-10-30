use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyEvent, KeyEventKind, MouseButton, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use ratatui::{DefaultTerminal, Frame};

use crate::highlight::Highlighter;
use crate::repo::Repo;
use crate::review;
use crate::session::Session;
use crate::store::Store;
use crate::theme::Theme;
use crate::ui;
use crate::ui::file_view::Files;
use crate::ui::review::Review;

/// State the review's layers share; layers keep only their own UI state.
pub struct Ctx {
    pub review: review::Review,
    /// Copy the threads waiting on the agent to the clipboard after quitting.
    pub copy: bool,
    /// Quitting goes back to the picker.
    pub back: bool,
    /// Run on quitting; given to `mrg resume` for this run only.
    pub on_quit: Option<String>,
    pub highlighter: Highlighter,
    pub look: Look,
    pub files: Files,
}

/// What the settings change; the picker and the review share it.
pub struct Look {
    pub theme: &'static Theme,
    /// Wrap long code lines.
    pub wrap: bool,
}

impl AsRef<Look> for Ctx {
    fn as_ref(&self) -> &Look {
        &self.look
    }
}

impl AsMut<Look> for Ctx {
    fn as_mut(&mut self) -> &mut Look {
        &mut self.look
    }
}

impl Ctx {
    pub fn new(session: Session, repo: Repo, store: Store, look: Look) -> Result<Ctx> {
        let mut review = review::Review::new(session, repo, store)?;
        let files = Files::new(review.repo.files(&review.session.end)?);
        Ok(Ctx {
            review,
            copy: false,
            back: false,
            on_quit: None,
            highlighter: Highlighter::new(),
            look,
            files,
        })
    }
}

pub enum Outcome<C = Ctx> {
    Consumed,
    Push(Box<dyn Component<C>>),
    Close,
    Quit,
}

/// A layer of the screen over the state `C` the layers share: the review's `Ctx`, or
/// the picker's. Layers render bottom-up; keys and the mouse go to the top layer only.
pub trait Component<C = Ctx> {
    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &C);
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut C) -> Result<Outcome<C>>;
    /// Pasted text; only text being typed takes it.
    fn paste(&mut self, _text: &str) {}
    /// A wheel turn over the cell `at`, as the movement `action`: "down" or "up".
    fn wheel(&mut self, _action: &str, _at: Position, _ctx: &mut C) -> Result<()> {
        Ok(())
    }
    /// A left click on the cell `at`.
    fn click(&mut self, _at: Position, _ctx: &mut C) -> Result<Outcome<C>> {
        Ok(Outcome::Consumed)
    }
    /// Footer hints; None leaves the footer to the layers below.
    fn hints(&self) -> Option<&str> {
        None
    }
}

/// The layers of a screen, the bottom one first; it never closes.
pub struct Layers<C>(Vec<Box<dyn Component<C>>>);

impl<C: AsRef<Look>> Layers<C> {
    pub fn new(bottom: impl Component<C> + 'static) -> Layers<C> {
        Layers(vec![Box::new(bottom)])
    }

    /// Renders the layers over the body and the top hints in the footer; returns the footer.
    pub fn render(&mut self, f: &mut Frame, ctx: &C) -> Rect {
        let theme = ctx.as_ref().theme;
        ui::clear(f, f.area(), theme);
        let [body, foot] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(f.area());
        f.render_widget(
            Paragraph::new(
                self.0
                    .iter()
                    .rev()
                    .find_map(|l| l.hints())
                    .unwrap_or_default(),
            )
            .style(Style::new().fg(theme.dim)),
            foot,
        );
        for layer in &mut self.0 {
            layer.render(f, body, ctx);
        }
        foot
    }

    /// Gives a key press, pasted text, a wheel turn or a click to the top layer; true when
    /// it quits.
    pub fn event(&mut self, event: Event, ctx: &mut C) -> Result<bool> {
        let top = self.0.last_mut().expect("the bottom layer");
        let outcome = match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => top.handle_key(key, ctx)?,
            Event::Paste(text) => {
                top.paste(&text);
                Outcome::Consumed
            }
            Event::Mouse(m) => {
                let at = Position::new(m.column, m.row);
                match m.kind {
                    MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                        let down = m.kind == MouseEventKind::ScrollDown;
                        top.wheel(if down { "down" } else { "up" }, at, ctx)?;
                        Outcome::Consumed
                    }
                    MouseEventKind::Down(MouseButton::Left) => top.click(at, ctx)?,
                    _ => Outcome::Consumed,
                }
            }
            _ => Outcome::Consumed,
        };
        match outcome {
            Outcome::Consumed => {}
            Outcome::Push(layer) => self.0.push(layer),
            Outcome::Close => {
                self.0.pop();
            }
            Outcome::Quit => return Ok(true),
        }
        Ok(false)
    }
}

pub fn run(terminal: &mut DefaultTerminal, ctx: &mut Ctx) -> Result<()> {
    let mut layers = Layers::new(Review::new());
    'draw: loop {
        ctx.highlighter.receive();
        terminal.draw(|f| {
            layers.render(f, ctx);
        })?;
        ctx.highlighter.flush(&mut ctx.review.repo);
        // Wait for a key, redrawing as colors arrive.
        while ctx.highlighter.busy() && !event::poll(Duration::from_millis(30))? {
            if ctx.highlighter.receive() {
                continue 'draw;
            }
        }
        if layers.event(event::read()?, ctx)? {
            return Ok(());
        }
    }
}
