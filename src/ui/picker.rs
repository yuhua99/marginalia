use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::keymap::{Feed, Keymap};
use crate::repo::Repo;
use crate::session::Session;
use crate::store::Store;

/// Commits offered as range ends and starts.
const COMMITS: usize = 500;

const HINTS: &str = " j/k move  h/l pane  Enter pick  / filter  X delete session  Esc back  q quit";

/// What `mrg` shows first: sessions to resume on the left, a new range on the right
/// (pick the end, then the start A). Returns the session to open, or None to quit.
pub fn run(
    terminal: &mut DefaultTerminal,
    store: &Store,
    repo: &mut Repo,
) -> Result<Option<Session>> {
    let mut p = Picker {
        sessions: store.list()?,
        commits: repo.recent(COMMITS)?,
        ..Picker::default()
    };
    p.right = p.sessions.is_empty();
    loop {
        terminal.draw(|f| p.render(f))?;
        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && let Some(done) = p.key(key, store, repo)?
        {
            return Ok(done);
        }
    }
}

#[derive(Default)]
struct Picker {
    sessions: Vec<(Session, u64)>,
    commits: Vec<(String, String)>,
    /// Focus on the range pane.
    right: bool,
    /// Per pane.
    cursor: [usize; 2],
    filter: [String; 2],
    /// Keys go to the filter of the focused pane.
    typing: bool,
    /// The end picked in the first step.
    end: Option<String>,
    /// Waiting for y/n to delete the session at the cursor.
    deleting: bool,
    error: Option<String>,
    keymap: Keymap,
}

impl Picker {
    fn pane(&self) -> usize {
        self.right as usize
    }

    /// (text, value) of a pane, filtered.
    fn items(&self, pane: usize) -> Vec<(String, String)> {
        let all: Vec<(String, String)> = if pane == 0 {
            self.sessions
                .iter()
                .map(|(s, _)| {
                    let text = format!(
                        "{}  {}  {} open  {} 等你",
                        s.id,
                        s.range,
                        s.open_count(),
                        s.waiting_on_user_count()
                    );
                    (text, s.id.clone())
                })
                .collect()
        } else {
            let commits = self
                .commits
                .iter()
                .map(|(sha, subject)| (format!("{} {subject}", &sha[..7]), sha.clone()));
            match &self.end {
                None => ["unstaged", "staged"]
                    .map(|s| (s.to_string(), s.to_string()))
                    .into_iter()
                    .chain(commits)
                    .collect(),
                Some(end) if matches!(end.as_str(), "staged" | "unstaged") => commits.collect(),
                // A comes after (below) the end.
                Some(end) => commits.skip_while(|c| c.1 != *end).skip(1).collect(),
            }
        };
        let filter = self.filter[pane].to_lowercase();
        all.into_iter()
            .filter(|(text, _)| text.to_lowercase().contains(&filter))
            .collect()
    }

    fn key(
        &mut self,
        key: KeyEvent,
        store: &Store,
        repo: &mut Repo,
    ) -> Result<Option<Option<Session>>> {
        self.error = None;
        let pane = self.pane();
        if self.deleting {
            self.deleting = false;
            if key.code == KeyCode::Char('y')
                && let Some((_, id)) = self.items(0).get(self.cursor[0])
            {
                store.delete(id)?;
                self.sessions = store.list()?;
                self.cursor[0] = self.cursor[0].min(self.items(0).len().saturating_sub(1));
            }
            return Ok(None);
        }
        if self.typing {
            match key.code {
                KeyCode::Char(c) => self.filter[pane].push(c),
                KeyCode::Backspace => {
                    self.filter[pane].pop();
                }
                KeyCode::Enter => self.typing = false,
                KeyCode::Esc => {
                    self.filter[pane].clear();
                    self.typing = false;
                }
                _ => {}
            }
            self.cursor[pane] = 0;
            return Ok(None);
        }
        match key.code {
            KeyCode::Char('/') => {
                self.typing = true;
                return Ok(None);
            }
            KeyCode::Char('X') if !self.right => {
                self.deleting = !self.items(0).is_empty();
                return Ok(None);
            }
            _ => {}
        }
        let Feed::Action(action) = self.keymap.feed(key) else {
            return Ok(None);
        };
        let items = self.items(pane);
        let last = items.len().saturating_sub(1);
        let cursor = self.cursor[pane];
        self.cursor[pane] = match action {
            "down" => (cursor + 1).min(last),
            "up" => cursor.saturating_sub(1),
            "half_page_down" => (cursor + 10).min(last),
            "half_page_up" => cursor.saturating_sub(10),
            "top" => 0,
            "bottom" => last,
            _ => cursor,
        };
        match action {
            "focus_left" => self.right = false,
            "focus_right" => self.right = true,
            "quit" => return Ok(Some(None)),
            "cancel" => {
                if self.right && self.end.is_some() {
                    self.end = None;
                    self.filter[1].clear();
                    self.cursor[1] = 0;
                } else {
                    self.filter[pane].clear();
                    self.cursor[pane] = 0;
                }
            }
            "open" => {
                if let Some((_, value)) = items.into_iter().nth(cursor) {
                    return self.pick(value, store, repo);
                }
            }
            _ => {}
        }
        Ok(None)
    }

    /// Opens a session, takes the end, or makes a session from A and the end.
    fn pick(
        &mut self,
        value: String,
        store: &Store,
        repo: &mut Repo,
    ) -> Result<Option<Option<Session>>> {
        if !self.right {
            let session = store.load(&value)?;
            if let Err(e) = repo.verify(&session.base, &session.end) {
                self.error = Some(format!("{e:#}"));
                return Ok(None);
            }
            return Ok(Some(Some(session)));
        }
        let Some(end) = &self.end else {
            self.end = Some(value);
            self.filter[1].clear();
            self.cursor[1] = 0;
            return Ok(None);
        };
        let range = format!("{}..{}", short(&value), short(end));
        match repo.resolve(&format!("{value}..{end}")) {
            Ok((base, end)) => {
                let session = Session::new(store.new_id(), range, base, end, None);
                store.save(&session)?;
                Ok(Some(Some(session)))
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                Ok(None)
            }
        }
    }

    fn render(&self, f: &mut Frame) {
        let [body, foot] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(f.area());
        let panes: [_; 2] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .areas(body);
        for (pane, area) in panes.into_iter().enumerate() {
            let focused = pane == self.pane();
            let mut title = match (pane, &self.end) {
                (0, _) => " Sessions ".to_string(),
                (_, None) => " New review: pick the end ".to_string(),
                (_, Some(end)) => format!(" New review: pick the start A of ..{} ", short(end)),
            };
            if !self.filter[pane].is_empty() || (focused && self.typing) {
                title.push_str(&format!("/{} ", self.filter[pane]));
            }
            let height = area.height.saturating_sub(2) as usize;
            let cursor = self.cursor[pane];
            let skip = (cursor + 1).saturating_sub(height);
            let lines: Vec<Line> = self
                .items(pane)
                .into_iter()
                .enumerate()
                .skip(skip)
                .take(height)
                .map(|(i, (text, _))| {
                    let style = match (i == cursor, focused) {
                        (true, true) => Style::new().add_modifier(Modifier::REVERSED),
                        (true, false) => Style::new().bg(Color::Rgb(50, 50, 50)),
                        _ => Style::new(),
                    };
                    Line::styled(text, style)
                })
                .collect();
            let border = if focused {
                Color::Yellow
            } else {
                Color::DarkGray
            };
            let block = Block::bordered()
                .title(title)
                .border_style(Style::new().fg(border));
            f.render_widget(Paragraph::new(lines).block(block), area);
        }
        let (text, style) = if let Some(e) = &self.error {
            (e.clone(), Style::new().fg(Color::Red))
        } else if self.deleting {
            (
                "delete this session? y/n".to_string(),
                Style::new().fg(Color::Black).bg(Color::Yellow),
            )
        } else {
            (HINTS.to_string(), Style::new().fg(Color::DarkGray))
        };
        f.render_widget(Paragraph::new(text).style(style), foot);
    }
}

/// A commit sha shortened for a range; `staged` / `unstaged` as is.
fn short(rev: &str) -> &str {
    if rev.len() == 40 { &rev[..7] } else { rev }
}
