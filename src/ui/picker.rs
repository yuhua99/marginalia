use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{DefaultTerminal, Frame};
use ratatui_textarea::TextArea;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::keymap::{Feed, Keymap};
use crate::repo::{self, Commit, Repo, short};
use crate::session::{NAME_MAX, Session};
use crate::store::Store;
use crate::theme::Theme;
use crate::ui::commit_row::{Columns, mark};
use crate::ui::help::{self, Entry};
use crate::ui::popup::Popup;
use crate::ui::settings::SettingsPopup;
use crate::ui::{boxed, check, clear, cursor_bar, dialog};

/// Commits offered for a new range.
const COMMITS: usize = 500;

const HINTS: &str = " j/k move  h/l pane  Space select  Enter open  / filter  R rename  X delete session  Esc clear  s settings  ? help  q quit";

const HELP: &[Entry] = &[
    (&["down", "up"], "move"),
    (&["half_page_down", "half_page_up"], "half page down / up"),
    (&["top", "bottom"], "top / bottom"),
    (&["focus_left", "focus_right"], "sessions / new review"),
    (&["check"], "check rows of a new range"),
    (&["open"], "resume the session, or review the checked rows"),
    (&["filter"], "filter; Enter ends typing, Esc clears"),
    (&["rename_session"], "rename the session; blank clears"),
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
        uncommitted: repo.uncommitted()?,
        ..Picker::default()
    };
    p.right = p.sessions.is_empty();
    loop {
        terminal.draw(|f| p.render(f, theme))?;
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if let Some(done) = p.key(key, store, repo, theme, wrap)? {
                    return Ok(done);
                }
            }
            Event::Paste(text) if p.renaming.is_some() => {
                let name = p.renaming.as_mut().expect("renaming");
                // As much as fits in NAME_MAX columns.
                let mut room = NAME_MAX.saturating_sub(name.lines()[0].width());
                let fits: String = text
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .filter(|c| !c.is_control())
                    .take_while(|c| {
                        let w = c.width().unwrap_or(0);
                        let fits = w <= room;
                        room = room.saturating_sub(w);
                        fits
                    })
                    .collect();
                name.insert_str(fits);
            }
            Event::Paste(text) if p.typing => {
                let pane = p.pane();
                p.filter[pane].push_str(text.lines().next().unwrap_or(""));
                p.cursor[pane] = 0;
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct Picker {
    sessions: Vec<(Session, u64)>,
    commits: Vec<Commit>,
    /// The rows of the range pane above the commits: `unstaged` / `staged` when not empty.
    uncommitted: Vec<&'static str>,
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
    /// The name being typed for the session at the cursor.
    renaming: Option<TextArea<'static>>,
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
                .map(|c| (format!("{} {}", &c.sha[..7], c.subject), c.sha.clone()));
            return self
                .uncommitted
                .iter()
                .map(|s| (s.to_string(), s.to_string()))
                .chain(commits)
                .collect();
        }
        let name_width = self
            .sessions
            .iter()
            .filter_map(|(s, _)| s.name.as_ref().map(|n| n.width()))
            .max();
        self.sessions
            .iter()
            .map(|(s, _)| {
                // A name column only when some session has a name.
                let name = name_width.map_or(String::new(), |w| {
                    let n = s.name.as_deref().unwrap_or("");
                    format!("{n}{}  ", " ".repeat(w - n.width()))
                });
                let text = format!("{}  {name}{}", s.id, s.range);
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
        if let Some(text) = &mut self.renaming {
            match key.code {
                KeyCode::Enter => {
                    if let Some(i) = at {
                        let name = text.lines().join("\n");
                        // Reloaded so changes saved since the list was read are kept.
                        let mut session = store.load(&self.sessions[i].0.id)?;
                        if let Err(e) = session.set_name(&name) {
                            self.error = Some(format!("{e:#}"));
                            return Ok(None);
                        }
                        store.save(&session)?;
                        self.sessions[i].0 = session;
                        // The row may leave the filter with its old name.
                        self.cursor[0] = self.cursor[0].min(self.shown(0).len().saturating_sub(1));
                    }
                    self.renaming = None;
                }
                KeyCode::Esc => self.renaming = None,
                // Kept to one line; no wider past NAME_MAX, so a stored longer name can shrink.
                _ => {
                    let before = text.clone();
                    let width = text.lines()[0].width();
                    if text.input(key)
                        && (text.lines().len() > 1 || text.lines()[0].width() > NAME_MAX.max(width))
                    {
                        *text = before;
                    }
                }
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
            "rename_session" if !self.right => {
                if let Some(i) = at {
                    let mut text = TextArea::default();
                    text.set_cursor_line_style(Style::new());
                    text.insert_str(self.sessions[i].0.name.as_deref().unwrap_or(""));
                    self.renaming = Some(text);
                }
            }
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
        let base = if repo::worktree(bottom) {
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
                let session = Session::new(store.new_id(), range, resolved, end);
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
            let columns = Columns::new(&self.commits);
            // Inside the borders, after the mark.
            let width = (area.width as usize).saturating_sub(2 + 2);
            let shown = self.shown(pane);
            let rows = shown.len();
            let lines: Vec<Line> = shown
                .into_iter()
                .skip(skip)
                .take(height)
                .map(|i| {
                    if pane == 0 {
                        return Line::raw(all[i].0.clone());
                    }
                    let checked = self.checked.is_some_and(|(t, b)| (t..=b).contains(&i));
                    let mut spans = vec![mark(checked, theme)];
                    match i.checked_sub(self.uncommitted.len()) {
                        Some(c) => spans.extend(columns.row(&self.commits[c], width, theme)),
                        None => spans.push(Span::raw(all[i].0.clone())),
                    }
                    Line::from(spans)
                })
                .collect();
            let block = boxed(title, focused, theme);
            let inner = block.inner(area);
            f.render_widget(Paragraph::new(lines).block(block), area);
            if focused && cursor < rows && height > 0 {
                cursor_bar(f, inner, inner.y + (cursor - skip) as u16, theme);
            }
        }
        let (text, style) = if let Some(e) = &self.error {
            (e.clone(), Style::new().fg(theme.red))
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
        if self.deleting {
            dialog::confirm(
                f,
                f.area(),
                "Delete",
                "delete this session?",
                "y delete  n/Esc cancel",
                theme,
            );
        }
        if let Some(text) = &self.renaming {
            dialog::input(
                f,
                f.area(),
                "Rename session",
                text,
                NAME_MAX as u16 + 1,
                "Enter save  Esc cancel  blank clears",
                theme,
            );
        }
    }
}
