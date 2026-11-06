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
    /// Nothing on screen changed: no draw.
    Unchanged,
    Push(Box<dyn Component<C>>),
    Close,
    Quit,
}

/// A wheel turn or the left button, over a cell.
#[derive(Clone, Copy)]
pub enum Mouse {
    Wheel {
        down: bool,
    },
    Press,
    /// The button, held since a press on the cell `from`, moved.
    Drag {
        from: Position,
    },
    /// The button let go after a press on the cell `from`.
    Release {
        from: Position,
    },
}

/// A layer of the screen over the state `C` the layers share: the review's `Ctx`, or
/// the picker's. Layers render bottom-up; keys and the mouse go to the top layer only.
pub trait Component<C = Ctx> {
    /// `pointer`: the cell under the mouse, for the top layer only.
    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &C, pointer: Option<Position>);
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut C) -> Result<Outcome<C>>;
    /// Pasted text; only text being typed takes it.
    fn paste(&mut self, _text: &str) {}
    /// The mouse event `m` over the cell `at`.
    fn mouse(&mut self, _m: Mouse, _at: Position, _ctx: &mut C) -> Result<Outcome<C>> {
        Ok(Outcome::Unchanged)
    }
    /// What a click on the cell `at` acts on, as of the last render: a box and its row
    /// there. A pointer move that changes it draws, for `render` to underline it.
    fn target(&self, _at: Position) -> Option<(usize, Option<usize>)> {
        None
    }
    /// Footer hints; None leaves the footer to the layers below.
    fn hints(&self) -> Option<&str> {
        None
    }
}

/// The layers of a screen, the bottom one first; it never closes.
pub struct Layers<C> {
    layers: Vec<Box<dyn Component<C>>>,
    /// The cell under the mouse.
    pointer: Option<Position>,
    /// The cell the left button was pressed on, until it is let go.
    press: Option<Position>,
    /// An event read after mouse events that changed the screen, held for after the draw.
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

    /// Renders the layers over the body and the top hints in the footer; returns the footer.
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

    /// Gives events from `read` to the layers until the screen needs a draw; true when they
    /// quit. `read` gives None to draw without an event.
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

    /// Gives a key press, pasted text or a mouse event to the top layer, and pushes or
    /// pops the layer it asks for; a pointer move draws only when it changes the target.
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
            // A resize, among others.
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
            // Wait for an event, redrawing as colors arrive.
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
