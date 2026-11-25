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

pub struct Ctx {
    pub review: review::Review,
    pub copy: bool,
    pub back: bool,
    pub on_quit: Option<String>,
    pub highlighter: Highlighter,
    pub look: Look,
    pub files: Files,
}

pub struct Look {
    pub theme: &'static Theme,
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
        let end = review.session().end.clone();
        let files = Files::new(review.repo.files(&end)?);
        let highlighter = Highlighter::new(review.repo.blobs());
        Ok(Ctx {
            review,
            copy: false,
            back: false,
            on_quit: None,
            highlighter,
            look,
            files,
        })
    }
}

pub enum Outcome<C = Ctx> {
    Consumed,
    Unchanged,
    Push(Box<dyn Component<C>>),
    Close,
    Quit,
}

#[derive(Clone, Copy)]
pub enum Mouse {
    Wheel { down: bool },
    Press,
    Drag { from: Position },
    Release { from: Position },
}

pub trait Component<C = Ctx> {
    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &C, pointer: Option<Position>);
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut C) -> Result<Outcome<C>>;
    fn paste(&mut self, _text: &str) {}
    fn mouse(&mut self, _m: Mouse, _at: Position, _ctx: &mut C) -> Result<Outcome<C>> {
        Ok(Outcome::Unchanged)
    }
    fn target(&self, _at: Position) -> Option<(usize, Option<usize>)> {
        None
    }
    fn hints(&self) -> Option<&str> {
        None
    }
}

pub struct Layers<C> {
    layers: Vec<Box<dyn Component<C>>>,
    pointer: Option<Position>,
    press: Option<Position>,
    pending: Option<Event>,
}

impl<C: AsRef<Look>> Layers<C> {
    pub fn new(bottom: impl Component<C> + 'static) -> Layers<C> {
        Layers {
            layers: vec![Box::new(bottom)],
            pointer: None,
            press: None,
            pending: None,
        }
    }

    pub fn render(&mut self, f: &mut Frame, ctx: &C) -> Rect {
        let theme = ctx.as_ref().theme;
        ui::clear(f, f.area(), theme);
        let [body, foot] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(f.area());
        f.render_widget(
            Paragraph::new(
                self.layers
                    .iter()
                    .rev()
                    .find_map(|l| l.hints())
                    .unwrap_or_default(),
            )
            .style(Style::new().fg(theme.dim)),
            foot,
        );
        let top = self.layers.len() - 1;
        for (i, layer) in self.layers.iter_mut().enumerate() {
            let pointer = self.pointer.filter(|_| i == top);
            layer.render(f, body, ctx, pointer);
        }
        foot
    }

    pub fn wait(
        &mut self,
        ctx: &mut C,
        mut read: impl FnMut(&mut C) -> Result<Option<Event>>,
    ) -> Result<bool> {
        let mut draw = false;
        // Mouse events already waiting go in before the draw: one draw catches up with a
        // burst of pointer moves, rather than one per move. Anything else acts on the screen
        // drawn: a key after them waits for the draw, and a key that changes the screen draws
        // before the next event, as moves through a tall row go by the lines laid out.
        while !draw || event::poll(Duration::ZERO)? {
            let event = match self.pending.take() {
                Some(event) => event,
                None => match read(ctx)? {
                    Some(event) => event,
                    None => break,
                },
            };
            let mouse = matches!(event, Event::Mouse(_));
            if draw && !mouse {
                self.pending = Some(event);
                break;
            }
            match self.event(event, ctx)? {
                Outcome::Unchanged => {}
                Outcome::Quit => return Ok(true),
                _ if !mouse => break,
                _ => draw = true,
            }
        }
        Ok(false)
    }

    fn event(&mut self, event: Event, ctx: &mut C) -> Result<Outcome<C>> {
        let top = self.layers.last_mut().expect("the bottom layer");
        let outcome = match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => top.handle_key(key, ctx)?,
            Event::Paste(text) => {
                top.paste(&text);
                Outcome::Consumed
            }
            Event::Mouse(m) => {
                let at = Position::new(m.column, m.row);
                let before = self.pointer.replace(at);
                match m.kind {
                    MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                        let down = m.kind == MouseEventKind::ScrollDown;
                        top.mouse(Mouse::Wheel { down }, at, ctx)?
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        self.press = Some(at);
                        top.mouse(Mouse::Press, at, ctx)?
                    }
                    MouseEventKind::Drag(MouseButton::Left) => match self.press {
                        Some(from) => top.mouse(Mouse::Drag { from }, at, ctx)?,
                        None => Outcome::Unchanged,
                    },
                    MouseEventKind::Up(MouseButton::Left) => match self.press.take() {
                        Some(from) => top.mouse(Mouse::Release { from }, at, ctx)?,
                        None => Outcome::Unchanged,
                    },
                    MouseEventKind::Moved
                        if before.and_then(|p| top.target(p)) != top.target(at) =>
                    {
                        Outcome::Consumed
                    }
                    _ => Outcome::Unchanged,
                }
            }
            _ => Outcome::Consumed,
        };
        match outcome {
            Outcome::Push(layer) => self.layers.push(layer),
            Outcome::Close => {
                self.layers.pop();
            }
            outcome => return Ok(outcome),
        }
        Ok(Outcome::Consumed)
    }
}

pub fn run(terminal: &mut DefaultTerminal, ctx: &mut Ctx) -> Result<()> {
    let mut layers = Layers::new(Review::new());
    loop {
        ctx.highlighter.receive();
        terminal.draw(|f| {
            layers.render(f, ctx);
        })?;
        ctx.highlighter.flush(&mut ctx.review.repo);
        let quit = layers.wait(ctx, |ctx| {
            while ctx.highlighter.busy() && !event::poll(Duration::from_millis(30))? {
                if ctx.highlighter.receive() {
                    return Ok(None);
                }
            }
            Ok(Some(event::read()?))
        })?;
        if quit {
            return Ok(());
        }
    }
}
