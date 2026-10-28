use std::collections::HashSet;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use ratatui::{DefaultTerminal, Frame};

use crate::doc::Doc;
use crate::highlight::Highlighter;
use crate::repo::{self, Commit, Repo};
use crate::session::{Anchor, Session};
use crate::store::Store;
use crate::theme::Theme;
use crate::ui;
use crate::ui::review::Review;

/// State the review's layers share; layers keep only their own UI state.
pub struct Ctx {
    pub session: Session,
    pub repo: Repo,
    pub store: Store,
    pub doc: Doc,
    /// Copy the threads waiting on the agent to the clipboard after quitting.
    pub copy: bool,
    /// Quitting goes back to the picker.
    pub back: bool,
    /// Run on quitting; given to `mrg resume` for this run only.
    pub on_quit: Option<String>,
    /// What the diff can show: each commit, then uncommitted changes.
    pub views: Vec<View>,
    pub shown: Shown,
    pub highlighter: Highlighter,
    pub look: Look,
    /// Paths of the files at the range end, which links in threads may open.
    pub files: HashSet<String>,
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

pub enum View {
    /// `C^ C`.
    Commit(Commit),
    /// `HEAD staged` or `HEAD unstaged`.
    Uncommitted,
}

/// The views the diff shows: the whole range (`base end`), or a run of them as one diff.
#[derive(Clone, Copy, PartialEq)]
pub enum Shown {
    All,
    /// First and last of the `views` (oldest to newest), not all of them.
    Run(usize, usize),
}

impl Shown {
    /// The run `first..=last` of `n` views; every one of them is the whole range.
    pub fn of((first, last): (usize, usize), n: usize) -> Shown {
        if first == 0 && last + 1 >= n {
            Shown::All
        } else {
            Shown::Run(first, last)
        }
    }

    /// First and last of the `n` views shown.
    pub fn bounds(self, n: usize) -> (usize, usize) {
        match self {
            Shown::All => (0, n.saturating_sub(1)),
            Shown::Run(first, last) => (first, last),
        }
    }
}

impl Ctx {
    pub fn new(mut session: Session, mut repo: Repo, store: Store, look: Look) -> Result<Ctx> {
        session.check_outdated(&mut repo)?;
        let mut views: Vec<View> = repo
            .commits(&session.base, &session.end)?
            .into_iter()
            .map(View::Commit)
            .collect();
        if repo::worktree(&session.end) {
            views.push(View::Uncommitted);
        }
        let doc = load(&mut repo, &session, &views, Shown::All)?;
        let files = repo.files(&session.end)?;
        Ok(Ctx {
            session,
            repo,
            store,
            doc,
            copy: false,
            back: false,
            on_quit: None,
            views,
            shown: Shown::All,
            highlighter: Highlighter::new(),
            look,
            files,
        })
    }

    /// Switches the diff to `shown`.
    pub fn show(&mut self, shown: Shown) -> Result<()> {
        self.doc = load(&mut self.repo, &self.session, &self.views, shown)?;
        self.shown = shown;
        Ok(())
    }

    /// The user's comment on `anchor`. The inner error is the session turning it down,
    /// the outer one saving failing; so for `reply`.
    pub fn comment(&mut self, anchor: Anchor, body: String) -> Result<Result<()>> {
        if let Err(e) = self.session.comment(anchor, "user", body, &mut self.repo) {
            return Ok(Err(e));
        }
        self.threads_changed().map(Ok)
    }

    /// The user's reply to thread `idx`.
    pub fn reply(&mut self, idx: usize, body: String) -> Result<Result<()>> {
        let id = self.session.threads[idx].id.clone();
        if let Err(e) = self.session.reply(&id, "user", body) {
            return Ok(Err(e));
        }
        self.threads_changed().map(Ok)
    }

    /// Rewrites the last message of thread `idx`.
    pub fn edit(&mut self, idx: usize, body: String) -> Result<()> {
        self.session.edit(idx, body);
        self.threads_changed()
    }

    /// Removes the last message of thread `idx`, or the thread when it is the only one.
    pub fn delete(&mut self, idx: usize) -> Result<()> {
        self.session.delete(idx);
        self.threads_changed()
    }

    /// Saves the session and refreshes the thread rows after threads changed.
    fn threads_changed(&mut self) -> Result<()> {
        self.store.save(&self.session)?;
        self.doc.set_threads(&self.session.threads);
        Ok(())
    }
}

/// The diff of `shown`: from the left side of its first view to the right side of the last.
fn load(repo: &mut Repo, session: &Session, views: &[View], shown: Shown) -> Result<Doc> {
    let (left, right, commits, carry) = match shown {
        Shown::All => (
            session.base.clone(),
            session.end.clone(),
            Vec::new(),
            Vec::new(),
        ),
        Shown::Run(first, last) => {
            let run = &views[first..=last];
            let left = parent(repo, &run[0])?;
            let commits: Vec<String> = run.iter().map(|v| rev(session, v)).collect();
            let right = commits[commits.len() - 1].clone();
            let mut carry = Vec::new();
            for (i, v) in run.iter().enumerate() {
                let kept = if i + 1 < run.len() {
                    Some(repo.diff(&commits[i], &right, 0)?)
                } else {
                    None
                };
                let deleted = if i > 0 {
                    let from = parent(repo, v)?;
                    Some(repo.diff(&from, &left, 0)?)
                } else {
                    None
                };
                carry.push([kept, deleted]);
            }
            (left, right, commits, carry)
        }
    };
    let files = repo.diff(&left, &right, 3)?;
    let mut doc = Doc::new(left, right, files, commits, carry);
    doc.set_threads(&session.threads);
    Ok(doc)
}

/// The left side of `view`.
fn parent(repo: &Repo, view: &View) -> Result<String> {
    match view {
        View::Commit(c) => match repo.parent(&c.sha)? {
            Some(parent) => Ok(parent),
            None => repo.empty_tree(),
        },
        View::Uncommitted => repo.commit("HEAD"),
    }
}

/// The right side of `view`.
fn rev(session: &Session, view: &View) -> String {
    match view {
        View::Commit(c) => c.sha.clone(),
        View::Uncommitted => session.end.clone(),
    }
}

pub enum Outcome<C = Ctx> {
    Consumed,
    Push(Box<dyn Component<C>>),
    Close,
    Quit,
}

/// A layer of the screen over the state `C` the layers share: the review's `Ctx`, or
/// the picker's. Layers render bottom-up; keys go to the top layer only.
pub trait Component<C = Ctx> {
    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &C);
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut C) -> Result<Outcome<C>>;
    /// Pasted text; only text being typed takes it.
    fn paste(&mut self, _text: &str) {}
    /// Footer hints; None leaves the footer to the layers below.
    fn hints(&self) -> Option<&'static str> {
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

    /// Gives a key press or pasted text to the top layer; true when it quits.
    pub fn event(&mut self, event: Event, ctx: &mut C) -> Result<bool> {
        let top = self.0.last_mut().expect("the bottom layer");
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => match top.handle_key(key, ctx)? {
                Outcome::Consumed => {}
                Outcome::Push(layer) => self.0.push(layer),
                Outcome::Close => {
                    self.0.pop();
                }
                Outcome::Quit => return Ok(true),
            },
            Event::Paste(text) => top.paste(&text),
            _ => {}
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
        ctx.highlighter.flush(&mut ctx.repo);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_view_is_all() {
        assert!(Shown::of((0, 3), 4) == Shown::All);
        assert!(Shown::of((0, 0), 1) == Shown::All);
        assert!(Shown::of((1, 3), 4) == Shown::Run(1, 3));
        assert!(Shown::of((0, 2), 4) == Shown::Run(0, 2));
        assert_eq!(Shown::All.bounds(4), (0, 3));
        assert_eq!(Shown::All.bounds(0), (0, 0));
        assert_eq!(Shown::Run(1, 2).bounds(4), (1, 2));
    }
}
