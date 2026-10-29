use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{DefaultTerminal, Frame};
use ratatui_textarea::TextArea;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{Component, Layers, Look, Outcome};
use crate::keymap::{Feed, Keymap};
use crate::repo::{self, Commit, Repo, short};
use crate::session::{NAME_MAX, Session};
use crate::store::Store;
use crate::theme::Theme;
use crate::ui::commit_row::{Columns, fit, mark};
use crate::ui::confirm::Confirm;
use crate::ui::help::{self, Entry, Help};
use crate::ui::query::Query;
use crate::ui::settings::SettingsPopup;
use crate::ui::{Panes, boxed, check, clear, cursor_bar, dialog, row_at, step};

/// Commits offered for a new range.
const COMMITS: usize = 500;

const HINTS: &[Entry] = &[
    (&["down", "up"], "move"),
    (&["focus_left", "focus_right"], "pane"),
    (&["check"], "select"),
    (&["open"], "open"),
    (&["filter"], "filter"),
    (&["edit"], "rename"),
    (&["delete"], "delete session"),
    (&["cancel"], "clear"),
    (&["settings"], "settings"),
    (&["help"], "help"),
    (&["quit"], "quit"),
];

const HELP: &[Entry] = &[
    (&["down", "up"], "move"),
    (&["half_page_down", "half_page_up"], "half page down / up"),
    (&["top", "bottom"], "top / bottom"),
    (&["focus_left", "focus_right"], "sessions / new review"),
    (&["check"], "check rows of a new range"),
    (&["open"], "resume the session, or review the checked rows"),
    (&["filter"], "filter; Enter ends typing, Esc clears"),
    (&["edit"], "rename the session; blank clears"),
    (&["delete"], "delete the session"),
    (&["cancel"], "clear the checks, then the filter"),
    (&["settings"], "settings: theme, wrap"),
    (&["help"], "this help"),
    (&["quit"], "quit"),
];

/// State the picker's layers share.
pub struct Pick {
    pub store: Store,
    pub repo: Repo,
    pub look: Look,
    sessions: Vec<(Session, u64)>,
    /// Shown in the footer until the next key, click or wheel turn.
    error: Option<String>,
    /// The session to open after the picker quits; None quits mrg.
    pub open: Option<Session>,
}

impl Pick {
    pub fn new(store: Store, repo: Repo, look: Look) -> Result<Pick> {
        Ok(Pick {
            sessions: store.list()?,
            store,
            repo,
            look,
            error: None,
            open: None,
        })
    }
}

impl AsRef<Look> for Pick {
    fn as_ref(&self) -> &Look {
        &self.look
    }
}

impl AsMut<Look> for Pick {
    fn as_mut(&mut self) -> &mut Look {
        &mut self.look
    }
}

/// What `mrg` shows first: sessions to resume on the left, a new range on the right
/// (rows checked with Space, or the cursor row). Ends with the session to open in
/// `pick.open`, or None to quit.
pub fn run(terminal: &mut DefaultTerminal, pick: &mut Pick) -> Result<()> {
    let mut layers = Layers::new(Picker::new(pick)?);
    loop {
        terminal.draw(|f| {
            let foot = layers.render(f, pick);
            if let Some(e) = &pick.error {
                let theme = pick.look.theme;
                clear(f, foot, theme);
                f.render_widget(
                    Paragraph::new(e.as_str()).style(Style::new().fg(theme.red)),
                    foot,
                );
            }
        })?;
        let event = event::read()?;
        // Not on a button's release, which follows the click that made the error.
        if matches!(&event, Event::Key(key) if key.kind == KeyEventKind::Press)
            || matches!(&event, Event::Mouse(m) if !matches!(m.kind, MouseEventKind::Up(_)))
        {
            pick.error = None;
        }
        if layers.event(event, pick)? {
            return Ok(());
        }
    }
}

/// The bottom layer: the two panes.
#[derive(Default)]
struct Picker {
    commits: Vec<Commit>,
    /// `commits` reach the root commit.
    exhausted: bool,
    /// The rows of the range pane above the commits: `unstaged` / `staged` when not empty.
    uncommitted: Vec<&'static str>,
    /// Focus on the range pane.
    right: bool,
    /// Per pane, into its shown rows.
    cursor: [usize; 2],
    filter: [Query; 2],
    /// Checked rows of the range pane, top..=bottom into all its rows.
    checked: Option<(usize, usize)>,
    /// Rows a pane shows, as of the last render.
    height: usize,
    keymap: Keymap,
    /// The footer, from `HINTS`.
    hints: String,
    /// The two panes, as of the last render.
    panes: [Rect; 2],
}

impl Picker {
    fn new(pick: &Pick) -> Result<Picker> {
        let commits = pick.repo.recent("HEAD", COMMITS)?;
        let keymap = Keymap::new("picker");
        Ok(Picker {
            exhausted: commits.len() < COMMITS,
            commits,
            uncommitted: pick.repo.uncommitted()?,
            right: pick.sessions.is_empty(),
            hints: help::hints(HINTS, &keymap),
            keymap,
            ..Picker::default()
        })
    }

    /// Appends the next `COMMITS` commits below the last one.
    fn more(&mut self, repo: &Repo) -> Result<()> {
        let Some(last) = self.commits.last() else {
            return Ok(());
        };
        // One more to skip `last` itself, so a root commit needs no parent lookup.
        let page = repo.recent(&last.sha, COMMITS + 1)?;
        self.exhausted = page.len() <= COMMITS;
        self.commits.extend(page.into_iter().skip(1));
        Ok(())
    }

    fn pane(&self) -> usize {
        self.right as usize
    }

    /// The first row `pane` shows: enough down for its cursor.
    fn skip(&self, pane: usize) -> usize {
        (self.cursor[pane] + 1).saturating_sub(self.height)
    }

    /// (text, value) of every row of a pane: session ids, or unstaged, staged and commit shas.
    fn all(&self, pane: usize, sessions: &[(Session, u64)]) -> Vec<(String, String)> {
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
        let name_width = name_width(sessions);
        sessions
            .iter()
            .map(|(s, _)| {
                let name = name_column(s, name_width);
                let text = format!("{}{name}  {}", s.id, s.range);
                (text, s.id.clone())
            })
            .collect()
    }

    /// Indices into `all(pane)` that pass the filter.
    fn shown(&self, pane: usize, sessions: &[(Session, u64)]) -> Vec<usize> {
        let filter = self.filter[pane].text.to_lowercase();
        self.all(pane, sessions)
            .iter()
            .enumerate()
            .filter(|(_, (text, _))| text.to_lowercase().contains(&filter))
            .map(|(i, _)| i)
            .collect()
    }

    /// Enter on row `i`: resumes that session, or makes a session of the checked rows
    /// (or row `i` alone): from the parent of the bottom commit (or before the root) to the top row.
    fn open(&self, i: usize, ctx: &mut Pick) -> Result<Outcome<Pick>> {
        if !self.right {
            let session = ctx.store.load(&ctx.sessions[i].0.id)?;
            if let Err(e) = ctx.repo.verify(&session.base, &session.end) {
                ctx.error = Some(format!("{e:#}"));
                return Ok(Outcome::Consumed);
            }
            ctx.open = Some(session);
            return Ok(Outcome::Quit);
        }
        let (top, bottom) = self.checked.unwrap_or((i, i));
        let all = self.all(1, &ctx.sessions);
        let (end, bottom) = (&all[top].1, &all[bottom].1);
        let base = if repo::worktree(bottom) {
            "HEAD".to_string()
        } else {
            // Left out for the root commit: the range starts before it.
            ctx.repo.parent(bottom)?.unwrap_or_default()
        };
        match ctx.repo.resolve(&format!("{base}..{end}")) {
            Ok((resolved, end)) => {
                let from = if base.is_empty() {
                    ""
                } else {
                    short(&resolved)
                };
                let range = format!("{from}..{}", short(&end));
                let session = Session::new(ctx.store.new_id(), range, resolved, end);
                ctx.store.save(&session)?;
                ctx.open = Some(session);
                Ok(Outcome::Quit)
            }
            Err(e) => {
                ctx.error = Some(format!("{e:#}"));
                Ok(Outcome::Consumed)
            }
        }
    }
}

impl Panes<Pick> for Picker {
    fn act(&mut self, action: &str, ctx: &mut Pick) -> Result<Outcome<Pick>> {
        let pane = self.pane();
        // Down on the last row of the unfiltered range pane loads older commits.
        if self.right
            && action == "down"
            && !self.exhausted
            && self.filter[1].text.is_empty()
            && self.cursor[1] + 1 == self.uncommitted.len() + self.commits.len()
            && let Err(e) = self.more(&ctx.repo)
        {
            ctx.error = Some(format!("{e:#}"));
        }
        let shown = self.shown(pane, &ctx.sessions);
        let at = shown.get(self.cursor[pane]).copied();
        let last = shown.len().saturating_sub(1);
        let cursor = self.cursor[pane];
        if let Some(to) = step(cursor, action, shown.len(), self.height) {
            self.cursor[pane] = to;
        }
        match action {
            "check" if self.right => {
                if let Some(i) = at {
                    self.checked = check(self.checked, i);
                    if self.checked.is_some() {
                        self.cursor[1] = (cursor + 1).min(last);
                    }
                }
            }
            "filter" => self.filter[pane].typing = true,
            "delete" if !self.right => {
                if let Some(i) = at {
                    let id = ctx.sessions[i].0.id.clone();
                    return Ok(Outcome::Push(Box::new(
                        Confirm::new("Delete", "delete this session?", "y delete  n/Esc cancel")
                            .on('y', move |ctx: &mut Pick| {
                                ctx.store.delete(&id)?;
                                ctx.sessions = ctx.store.list()?;
                                Ok(Outcome::Close)
                            })
                            .on('n', |_| Ok(Outcome::Close)),
                    )));
                }
            }
            "edit" if !self.right => {
                if let Some(i) = at {
                    return Ok(Outcome::Push(Box::new(Rename::new(&ctx.sessions[i].0))));
                }
            }
            "help" => return Ok(Outcome::Push(Box::new(Help::new(HELP, &self.keymap)))),
            "settings" => return Ok(Outcome::Push(Box::new(SettingsPopup::new(&ctx.look)))),
            "focus_left" => self.right = false,
            "focus_right" => self.right = true,
            "quit" => return Ok(Outcome::Quit),
            "cancel" => {
                if self.right && self.checked.is_some() {
                    self.checked = None;
                } else {
                    self.filter[pane] = Query::default();
                    self.cursor[pane] = 0;
                }
            }
            "open" => {
                if let Some(i) = at {
                    return self.open(i, ctx);
                }
            }
            _ => {}
        }
        Ok(Outcome::Consumed)
    }

    fn panes(&self) -> &[Rect] {
        &self.panes
    }

    fn focus(&mut self, pane: usize, _ctx: &Pick) {
        self.right = pane == 1;
    }

    fn busy(&self) -> bool {
        self.filter[self.pane()].typing
    }

    /// A session opens as Enter does, a range row checks as Space does.
    fn pick(&mut self, pane: usize, y: u16, ctx: &mut Pick) -> Result<Outcome<Pick>> {
        let len = self.shown(pane, &ctx.sessions).len();
        let Some(row) = row_at(self.panes[pane], self.skip(pane), len, y) else {
            return Ok(Outcome::Consumed);
        };
        self.cursor[pane] = row;
        self.act(["open", "check"][pane], ctx)
    }
}

impl Component<Pick> for Picker {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Pick) -> Result<Outcome<Pick>> {
        let pane = self.pane();
        if self.filter[pane].typing {
            self.filter[pane].key(key);
            self.cursor[pane] = 0;
            return Ok(Outcome::Consumed);
        }
        let Feed::Action(action) = self.keymap.feed(key) else {
            return Ok(Outcome::Consumed);
        };
        self.act(action, ctx)
    }

    fn wheel(&mut self, action: &str, at: Position, ctx: &mut Pick) -> Result<()> {
        self.wheel_at(action, at, ctx)
    }

    fn click(&mut self, at: Position, ctx: &mut Pick) -> Result<Outcome<Pick>> {
        self.click_at(at, ctx)
    }

    fn paste(&mut self, text: &str) {
        let pane = self.pane();
        if self.filter[pane].typing {
            self.filter[pane].paste(text);
            self.cursor[pane] = 0;
        }
    }

    fn hints(&self) -> Option<&str> {
        Some(&self.hints)
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Pick) {
        let theme = ctx.look.theme;
        // Sessions change under the layers above: deleted, or renamed out of the filter.
        self.cursor[0] = self.cursor[0].min(self.shown(0, &ctx.sessions).len().saturating_sub(1));
        let panes: [_; 2] =
            Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
                .areas(area);
        self.height = area.height.saturating_sub(2) as usize;
        self.panes = panes;
        let name_width = name_width(&ctx.sessions);
        let columns = Columns::new(&self.commits);
        for (pane, area) in panes.into_iter().enumerate() {
            let focused = pane == self.pane();
            let mut title = [" Sessions ", " New review "][pane].to_string();
            title.push_str(&self.filter[pane].title());
            let height = area.height.saturating_sub(2) as usize;
            let cursor = self.cursor[pane];
            let skip = self.skip(pane);
            // Inside the borders.
            let inner_width = (area.width as usize).saturating_sub(2);
            // After the mark.
            let width = inner_width.saturating_sub(2);
            let shown = self.shown(pane, &ctx.sessions);
            let rows = shown.len();
            let lines: Vec<Line> = shown
                .into_iter()
                .skip(skip)
                .take(height)
                .map(|i| {
                    if pane == 0 {
                        let s = &ctx.sessions[i].0;
                        return Line::from(session_row(s, name_width, inner_width, theme));
                    }
                    let checked = self.checked.is_some_and(|(t, b)| (t..=b).contains(&i));
                    let mut spans = vec![mark(checked, theme)];
                    match i.checked_sub(self.uncommitted.len()) {
                        Some(c) => spans.extend(columns.row(&self.commits[c], width, theme)),
                        None => spans.push(Span::raw(self.uncommitted[i])),
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
    }
}

/// The widest session name; None when no session has a name.
fn name_width(sessions: &[(Session, u64)]) -> Option<usize> {
    sessions
        .iter()
        .filter_map(|(s, _)| s.name.as_ref().map(|n| n.width()))
        .max()
}

/// `  name` padded to `name_width`, a longer name cut to `nam…`; empty when no
/// session has a name.
fn name_column(s: &Session, name_width: Option<usize>) -> String {
    name_width.map_or(String::new(), |w| {
        let mut n = s.name.clone().unwrap_or_default();
        if n.width() > w {
            n = format!("{}…", fit(&n, w.saturating_sub(1)));
        }
        format!("  {n}{}", " ".repeat(w.saturating_sub(n.width())))
    })
}

/// `id  name  range` in `width` columns, the range right-aligned; the name column
/// only when some session has a name, narrowed so the range fits.
fn session_row<'a>(
    s: &'a Session,
    name_width: Option<usize>,
    width: usize,
    theme: &Theme,
) -> Vec<Span<'a>> {
    // Width left for the name after the id, the range and the spaces around the name.
    let room = width.saturating_sub(s.id.width() + s.range.width() + 4);
    // No room: the name column goes, so the range fits.
    let name = name_column(s, name_width.map(|w| w.min(room)).filter(|&w| w > 0));
    let used = s.id.width() + name.width() + s.range.width();
    let pad = " ".repeat(width.saturating_sub(used).max(2));
    vec![
        Span::styled(s.id.as_str(), Style::new().fg(theme.dim)),
        Span::raw(name),
        Span::raw(pad),
        Span::styled(s.range.as_str(), Style::new().fg(theme.dim)),
    ]
}

/// `e`: the name being typed for session `id`; Enter saves it, Esc cancels.
struct Rename {
    id: String,
    text: TextArea<'static>,
}

impl Rename {
    fn new(session: &Session) -> Rename {
        let mut text = TextArea::default();
        text.set_cursor_line_style(Style::new());
        text.insert_str(session.name.as_deref().unwrap_or(""));
        Rename {
            id: session.id.clone(),
            text,
        }
    }
}

impl Component<Pick> for Rename {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Pick) -> Result<Outcome<Pick>> {
        let text = &mut self.text;
        match key.code {
            KeyCode::Enter => {
                let name = text.lines().join("\n");
                // Reloaded so changes saved since the list was read are kept.
                let mut session = ctx.store.load(&self.id)?;
                if let Err(e) = session.set_name(&name) {
                    ctx.error = Some(format!("{e:#}"));
                    return Ok(Outcome::Consumed);
                }
                ctx.store.save(&session)?;
                if let Some(row) = ctx.sessions.iter_mut().find(|(s, _)| s.id == self.id) {
                    row.0 = session;
                }
                return Ok(Outcome::Close);
            }
            KeyCode::Esc => return Ok(Outcome::Close),
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
        Ok(Outcome::Consumed)
    }

    fn paste(&mut self, text: &str) {
        // As much as fits in NAME_MAX columns.
        let mut room = NAME_MAX.saturating_sub(self.text.lines()[0].width());
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
        self.text.insert_str(fits);
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Pick) {
        dialog::input(
            f,
            area,
            "Rename session",
            &self.text,
            NAME_MAX as u16 + 1,
            "Enter save  Esc cancel  blank clears",
            ctx.look.theme,
        );
    }
}
