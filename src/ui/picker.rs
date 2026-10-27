use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{DefaultTerminal, Frame};

use unicode_width::UnicodeWidthStr;

use crate::keymap::{Feed, Keymap};
use crate::repo::Repo;
use crate::session::Session;
use crate::store::Store;
use crate::theme::Theme;
use crate::ui::help::{self, Entry};
use crate::ui::popup::Popup;
use crate::ui::settings::SettingsPopup;
use crate::ui::{boxed, check, clear};

/// Commits offered for a new range.
const COMMITS: usize = 500;

const HINTS: &str = " j/k move  h/l pane  Space select  Enter open  / filter  X delete session  Esc clear  s settings  ? help  q quit";

const HELP: &[Entry] = &[
    (&["down", "up"], "move"),
    (&["half_page_down", "half_page_up"], "half page down / up"),
    (&["top", "bottom"], "top / bottom"),
    (&["focus_left", "focus_right"], "sessions / new review"),
    (&["check"], "check rows of a new range"),
    (&["open"], "resume the session, or review the checked rows"),
    (&["filter"], "filter; Enter ends typing, Esc clears"),
    (&["delete_session"], "delete the session"),
    (&["cancel"], "clear the checks, then the filter"),
    (&["settings"], "settings: theme, wrap"),
    (&["help"], "this help"),
    (&["quit"], "quit"),
];

/// What `mrg` shows first: sessions to resume on the left, a new range on the right
/// (rows checked with Space, or the cursor row). Returns the session to open, or None to quit.
pub fn run(
    terminal: &mut DefaultTerminal,
    store: &Store,
    repo: &mut Repo,
    theme: &mut &'static Theme,
    wrap: &mut bool,
) -> Result<Option<Session>> {
    let mut p = Picker {
        sessions: store.list()?,
        commits: repo.recent(COMMITS)?,
        ..Picker::default()
    };
    p.right = p.sessions.is_empty();
    loop {
        terminal.draw(|f| p.render(f, theme))?;
        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && let Some(done) = p.key(key, store, repo, theme, wrap)?
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
    /// Per pane, into its shown rows.
    cursor: [usize; 2],
    filter: [String; 2],
    /// Keys go to the filter of the focused pane.
    typing: bool,
    /// Checked rows of the range pane, top..=bottom into all its rows.
    checked: Option<(usize, usize)>,
    /// The `?` popup.
    help: Option<Popup>,
    /// The `s` popup.
    settings: Option<SettingsPopup>,
    /// Waiting for y/n to delete the session at the cursor.
    deleting: bool,
    error: Option<String>,
    keymap: Keymap,
}

impl Picker {
    fn pane(&self) -> usize {
        self.right as usize
    }

    /// (text, value) of every row of a pane: session ids, or unstaged, staged and commit shas.
    fn all(&self, pane: usize) -> Vec<(String, String)> {
        if pane == 1 {
            let commits = self
                .commits
                .iter()
                .map(|(sha, subject)| (format!("{} {subject}", &sha[..7]), sha.clone()));
            return ["unstaged", "staged"]
                .map(|s| (s.to_string(), s.to_string()))
                .into_iter()
                .chain(commits)
                .collect();
        }
        let width = self.sessions.iter().map(|(s, _)| s.range.width()).max();
        self.sessions
            .iter()
            .map(|(s, _)| {
                let pad = " ".repeat(width.unwrap_or(0) - s.range.width());
                let text = format!(
                    "{}  {}{pad}  {:>3} open  {:>3} waiting on you",
                    s.id,
                    s.range,
                    s.open_count(),
                    s.waiting_on_user_count()
                );
                (text, s.id.clone())
            })
            .collect()
    }

    /// Indices into `all(pane)` that pass the filter.
    fn shown(&self, pane: usize) -> Vec<usize> {
        let filter = self.filter[pane].to_lowercase();
        self.all(pane)
            .iter()
            .enumerate()
            .filter(|(_, (text, _))| text.to_lowercase().contains(&filter))
            .map(|(i, _)| i)
            .collect()
    }

    fn key(
        &mut self,
        key: KeyEvent,
        store: &Store,
        repo: &mut Repo,
        theme: &mut &'static Theme,
        wrap: &mut bool,
    ) -> Result<Option<Option<Session>>> {
        self.error = None;
        if let Some(settings) = &mut self.settings {
            if settings.key(key, theme, wrap) {
                self.settings = None;
            }
            return Ok(None);
        }
        let pane = self.pane();
        let shown = self.shown(pane);
        let at = shown.get(self.cursor[pane]).copied();
        if self.deleting {
            self.deleting = false;
            if key.code == KeyCode::Char('y')
                && let Some(i) = at
            {
                store.delete(&self.sessions[i].0.id)?;
                self.sessions = store.list()?;
                self.cursor[0] = self.cursor[0].min(self.shown(0).len().saturating_sub(1));
            }
            return Ok(None);
        }
        if let Some(popup) = &mut self.help {
            match self.keymap.feed(key) {
                Feed::Action("help" | "quit" | "cancel") => self.help = None,
                Feed::Action(action) => popup.act(action),
                _ => {}
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
        let Feed::Action(action) = self.keymap.feed(key) else {
            return Ok(None);
        };
        let last = shown.len().saturating_sub(1);
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
            "check" if self.right => {
                if let Some(i) = at {
                    self.checked = check(self.checked, i);
                    if self.checked.is_some() {
                        self.cursor[1] = (cursor + 1).min(last);
                    }
                }
            }
            "filter" => self.typing = true,
            "delete_session" if !self.right => self.deleting = at.is_some(),
            "help" => self.help = Some(help::popup(HELP)),
            "settings" => self.settings = Some(SettingsPopup::new(theme, *wrap)),
            "focus_left" => self.right = false,
            "focus_right" => self.right = true,
            "quit" => return Ok(Some(None)),
            "cancel" => {
                if self.right && self.checked.is_some() {
                    self.checked = None;
                } else {
                    self.filter[pane].clear();
                    self.cursor[pane] = 0;
                }
            }
            "open" => {
                if let Some(i) = at {
                    return self.open(i, store, repo);
                }
            }
            _ => {}
        }
        Ok(None)
    }

    /// Enter on row `i`: resumes that session, or makes a session of the checked rows
    /// (or row `i` alone): from the parent of the bottom commit (or before the root) to the top row.
    fn open(
        &mut self,
        i: usize,
        store: &Store,
        repo: &mut Repo,
    ) -> Result<Option<Option<Session>>> {
        if !self.right {
            let session = store.load(&self.sessions[i].0.id)?;
            if let Err(e) = repo.verify(&session.base, &session.end) {
                self.error = Some(format!("{e:#}"));
                return Ok(None);
            }
            return Ok(Some(Some(session)));
        }
        let (top, bottom) = self.checked.unwrap_or((i, i));
        let all = self.all(1);
        let (end, bottom) = (&all[top].1, &all[bottom].1);
        let base = if matches!(bottom.as_str(), "staged" | "unstaged") {
            "HEAD".to_string()
        } else {
            // Left out for the root commit: the range starts before it.
            repo.parent(bottom)?.unwrap_or_default()
        };
        match repo.resolve(&format!("{base}..{end}")) {
            Ok((resolved, end)) => {
                let from = if base.is_empty() {
                    ""
                } else {
                    short(&resolved)
                };
                let range = format!("{from}..{}", short(&end));
                let session = Session::new(store.new_id(), range, resolved, end, None);
                store.save(&session)?;
                Ok(Some(Some(session)))
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                Ok(None)
            }
        }
    }

    fn render(&self, f: &mut Frame, theme: &Theme) {
        clear(f, f.area(), theme);
        let [body, foot] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(f.area());
        let panes: [_; 2] =
            Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
                .areas(body);
        for (pane, area) in panes.into_iter().enumerate() {
            let focused = pane == self.pane();
            let mut title = [" Sessions ", " New review "][pane].to_string();
            if !self.filter[pane].is_empty() || (focused && self.typing) {
                title.push_str(&format!("/{} ", self.filter[pane]));
            }
            let height = area.height.saturating_sub(2) as usize;
            let cursor = self.cursor[pane];
            let skip = (cursor + 1).saturating_sub(height);
            let all = self.all(pane);
            let lines: Vec<Line> = self
                .shown(pane)
                .into_iter()
                .enumerate()
                .skip(skip)
                .take(height)
                .map(|(row, i)| {
                    let style = match (row == cursor, focused) {
                        (true, true) => Style::new().add_modifier(Modifier::REVERSED),
                        (true, false) => Style::new().add_modifier(Modifier::UNDERLINED),
                        _ => Style::new(),
                    };
                    let text = Span::styled(all[i].0.clone(), style);
                    if pane == 0 {
                        return Line::from(text);
                    }
                    let checked = self.checked.is_some_and(|(t, b)| (t..=b).contains(&i));
                    let mark = if checked { "[x] " } else { "[ ] " };
                    Line::from(vec![
                        Span::styled(mark, Style::new().fg(theme.accent)),
                        text,
                    ])
                })
                .collect();
            f.render_widget(
                Paragraph::new(lines).block(boxed(title, focused, theme)),
                area,
            );
        }
        let (text, style) = if let Some(e) = &self.error {
            (e.clone(), Style::new().fg(theme.red))
        } else if self.deleting {
            (
                "delete this session? y/n".to_string(),
                Style::new().fg(theme.on_accent).bg(theme.accent),
            )
        } else {
            (HINTS.to_string(), Style::new().fg(theme.dim))
        };
        f.render_widget(Paragraph::new(text).style(style), foot);
        if let Some(popup) = &self.help {
            popup.render(f, f.area(), theme);
        }
        if let Some(settings) = &self.settings {
            settings.draw(f, f.area(), theme);
        }
    }
}

/// A commit sha shortened for a range label; other revs as is.
fn short(rev: &str) -> &str {
    if rev.len() == 40 { &rev[..7] } else { rev }
}
