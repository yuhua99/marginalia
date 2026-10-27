use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Paragraph;
use ratatui::{DefaultTerminal, Frame};

use crate::doc::Doc;
use crate::highlight::Highlighter;
use crate::repo::Repo;
use crate::session::Session;
use crate::store::Store;
use crate::theme::Theme;
use crate::ui::review::Review;

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
    /// What the diff can show: the whole range, each commit, then uncommitted changes.
    pub views: Vec<View>,
    /// First and last of the `views` shown together (oldest to newest).
    pub view: (usize, usize),
    pub highlighter: Highlighter,
    pub theme: &'static Theme,
    /// The doc of the file view layer while it is open.
    pub file: Option<Doc>,
}

pub enum View {
    /// `base end`.
    All,
    /// `C^ C`.
    Commit { sha: String, subject: String },
    /// `HEAD staged` or `HEAD unstaged`.
    Uncommitted,
}

impl Ctx {
    pub fn new(
        session: Session,
        mut repo: Repo,
        store: Store,
        theme: &'static Theme,
    ) -> Result<Ctx> {
        let outdated = session.outdated(&mut repo)?;
        let mut views = vec![View::All];
        for (sha, subject) in repo.commits(&session.base, &session.end)? {
            views.push(View::Commit { sha, subject });
        }
        if matches!(session.end.as_str(), "staged" | "unstaged") {
            views.push(View::Uncommitted);
        }
        let doc = load(&mut repo, &session, &views, (0, 0))?;
        Ok(Ctx {
            session,
            repo,
            store,
            doc,
            outdated,
            copy: false,
            views,
            view: (0, 0),
            highlighter: Highlighter::new(),
            theme,
            file: None,
        })
    }

    /// Switches the diff to `views[first..=last]` as one diff.
    pub fn show(&mut self, view: (usize, usize)) -> Result<()> {
        self.doc = load(&mut self.repo, &self.session, &self.views, view)?;
        self.view = view;
        Ok(())
    }

    /// Saves the session and refreshes the thread rows after threads changed.
    pub fn threads_changed(&mut self) -> Result<()> {
        self.outdated.resize(self.session.threads.len(), false);
        self.store.save(&self.session)?;
        self.doc.set_threads(&self.session.threads);
        if let Some(file) = &mut self.file {
            file.set_threads(&self.session.threads);
        }
        Ok(())
    }
}

/// The diff from the left side of the first view to the right side of the last.
fn load(
    repo: &mut Repo,
    session: &Session,
    views: &[View],
    (first, last): (usize, usize),
) -> Result<Doc> {
    let left = match &views[first] {
        View::All => session.base.clone(),
        View::Commit { sha, .. } => match repo.parent(sha)? {
            Some(parent) => parent,
            None => repo.empty_tree()?,
        },
        View::Uncommitted => repo.commit("HEAD")?,
    };
    let right = match &views[last] {
        View::All | View::Uncommitted => session.end.clone(),
        View::Commit { sha, .. } => sha.clone(),
    };
    let files = repo.diff(&left, &right)?;
    let mut doc = Doc::new(left, right, files);
    doc.set_threads(&session.threads);
    Ok(doc)
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

const HINTS: &str = " c comment  r reply  x resolve  e/d edit/delete  n/N next waiting  v select  J/K file  h/l focus  Space check  Enter open  ? help  q quit";

pub fn run(terminal: &mut DefaultTerminal, ctx: &mut Ctx) -> Result<()> {
    let mut layers: Vec<Box<dyn Component>> = vec![Box::new(Review::default())];
    'draw: loop {
        ctx.highlighter.receive();
        terminal.draw(|f| {
            let [head, body, foot] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Fill(1),
                Constraint::Length(1),
            ])
            .areas(f.area());
            let s = &ctx.session;
            let mut header = format!(
                "mrg  {} · {} · {} open · {} waiting on you",
                s.range,
                s.id,
                s.open_count(),
                s.waiting_on_user_count()
            );
            let label = |v: usize| match &ctx.views[v] {
                View::All => "all",
                View::Commit { sha, .. } => &sha[..7],
                View::Uncommitted => &s.end,
            };
            match ctx.view {
                (0, 0) => {}
                (first, last) if first == last => {
                    header.push_str(&format!(" · viewing {}", label(first)))
                }
                (first, last) => {
                    header.push_str(&format!(" · viewing {} – {}", label(first), label(last)))
                }
            }
            f.render_widget(
                Paragraph::new(header).style(Style::new().add_modifier(Modifier::BOLD)),
                head,
            );
            f.render_widget(
                Paragraph::new(HINTS).style(Style::new().fg(ctx.theme.dim)),
                foot,
            );
            for layer in &mut layers {
                layer.render(f, body, ctx);
            }
        })?;
        ctx.highlighter.flush(&mut ctx.repo);
        // Wait for a key, redrawing as colors arrive.
        while ctx.highlighter.busy() && !event::poll(Duration::from_millis(30))? {
            if ctx.highlighter.receive() {
                continue 'draw;
            }
        }
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
}
