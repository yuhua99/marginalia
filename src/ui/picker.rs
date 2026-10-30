use std::collections::BTreeMap;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use ratatui_textarea::TextArea;

use unicode_width::UnicodeWidthStr;

use crate::app::{Component, Layers, Look, Outcome};
use crate::forge::{self, Forge, Request};
use crate::keymap::{Feed, Keymap};
use crate::repo::{self, Commit, Repo, short};
use crate::session::Session;
use crate::store::Store;
use crate::theme::Theme;
use crate::ui::commit_row::{Columns, MIN_SUBJECT, fit, mark};
use crate::ui::confirm::Confirm;
use crate::ui::help::{self, Entry, Help};
use crate::ui::query::Query;
use crate::ui::settings::SettingsPopup;
use crate::ui::{Panes, boxed, check, clear, cursor_bar, dialog, row_at, step};

/// Commits offered for a new range.
const COMMITS: usize = 500;

const HINTS: &[Entry] = &[
    (&["focus_left", "focus_right"], "block"),
    (&["check"], "select"),
    (&["open"], "open"),
    (&["filter"], "filter"),
    (&["edit"], "rename"),
    (&["delete"], "delete session"),
    (&["help"], "help"),
    (&["quit"], "quit"),
];

const HELP: &[Entry] = &[
    (&["down", "up"], "move"),
    (&["half_page_down", "half_page_up"], "half page down / up"),
    (&["top", "bottom"], "top / bottom"),
    (&["focus_left", "focus_right"], "previous / next block"),
    (&["check"], "check commits of a new range"),
    (
        &["open"],
        "resume the session, review the checked commits or the request",
    ),
    (&["filter"], "filter; Enter ends typing, Esc clears"),
    (&["edit"], "rename the session; blank clears"),
    (&["delete"], "delete the session"),
    (&["cancel"], "clear the checks, then the filter"),
    (&["settings"], "settings: theme, wrap"),
    (&["help"], "this help"),
    (&["quit"], "quit"),
];

/// The blocks, top to bottom.
const SESSIONS: usize = 0;
const RANGE: usize = 1;
const REVIEWS: usize = 2;
const TITLES: [&str; 3] = ["Sessions", "Commits", "Reviews"];

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
    /// `[forges]` of the settings.
    forges: BTreeMap<String, String>,
    /// Open requests of origin's forge, listed the first time their block is focused.
    reviews: Reviews,
}

enum Reviews {
    Unlisted,
    /// To list after the next draw, which shows it: listing waits on gh / glab.
    Listing,
    Listed(Forge, Vec<Request>),
    /// Why there is no list, for the user to act on.
    Failed(String),
}

impl Pick {
    pub fn new(
        store: Store,
        repo: Repo,
        look: Look,
        forges: BTreeMap<String, String>,
    ) -> Result<Pick> {
        Ok(Pick {
            sessions: store.list()?,
            store,
            repo,
            look,
            error: None,
            open: None,
            forges,
            reviews: Reviews::Unlisted,
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

/// What `mrg` shows first: blocks of sessions to resume, commits for a new range (rows
/// checked with Space, or the cursor row) and the forge's open requests. Ends with the
/// session to open in `pick.open`, or None to quit.
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
        if matches!(pick.reviews, Reviews::Listing) {
            let listed = Forge::of(&pick.repo, &pick.forges).and_then(|f| Ok((f.requests()?, f)));
            pick.reviews = match listed {
                Ok((requests, forge)) => Reviews::Listed(forge, requests),
                Err(e) => Reviews::Failed(format!("{e:#}")),
            };
            continue;
        }
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

/// The bottom layer: the blocks, the focused one open, the others a title line.
#[derive(Default)]
struct Picker {
    commits: Vec<Commit>,
    /// `commits` reach the root commit.
    exhausted: bool,
    /// The rows of the range block above the commits: `unstaged` / `staged` when not empty.
    uncommitted: Vec<&'static str>,
    /// `SESSIONS`, `RANGE` or `REVIEWS`.
    focused: usize,
    /// Per block, into its shown rows.
    cursor: [usize; 3],
    filter: [Query; 3],
    /// Checked rows of the range block, top..=bottom into all its rows.
    checked: Option<(usize, usize)>,
    /// Rows the focused block shows, as of the last render.
    height: usize,
    keymap: Keymap,
    /// The footer, from `HINTS`.
    hints: String,
    /// The blocks, as of the last render.
    panes: [Rect; 3],
}

impl Picker {
    fn new(pick: &Pick) -> Result<Picker> {
        let commits = pick.repo.recent("HEAD", COMMITS)?;
        let keymap = Keymap::new("picker");
        Ok(Picker {
            exhausted: commits.len() < COMMITS,
            commits,
            uncommitted: pick.repo.uncommitted()?,
            focused: if pick.sessions.is_empty() {
                RANGE
            } else {
                SESSIONS
            },
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
        self.focused
    }

    /// The first row `pane` shows: enough down for its cursor.
    fn skip(&self, pane: usize) -> usize {
        (self.cursor[pane] + 1).saturating_sub(self.height)
    }

    /// (text, value) of every row of a block: session ids; unstaged, staged and commit
    /// shas; or request numbers.
    fn all(&self, pane: usize, ctx: &Pick) -> Vec<(String, String)> {
        match pane {
            RANGE => {
                let commits = self
                    .commits
                    .iter()
                    .map(|c| (format!("{} {}", &c.sha[..7], c.subject), c.sha.clone()));
                self.uncommitted
                    .iter()
                    .map(|s| (s.to_string(), s.to_string()))
                    .chain(commits)
                    .collect()
            }
            REVIEWS => match &ctx.reviews {
                Reviews::Listed(forge, requests) => requests
                    .iter()
                    .map(|r| {
                        let text =
                            format!("{} {}  {}  {}", forge.word(), r.number, r.title, r.author);
                        (text, r.number.to_string())
                    })
                    .collect(),
                _ => Vec::new(),
            },
            _ => {
                let name_width = name_width(&ctx.sessions);
                ctx.sessions
                    .iter()
                    .map(|(s, _)| {
                        let name = name_column(s, name_width);
                        let text = format!("{}{name}  {}", s.id, s.range);
                        (text, s.id.clone())
                    })
                    .collect()
            }
        }
    }

    /// Indices into `all(pane)` that pass the filter.
    fn shown(&self, pane: usize, ctx: &Pick) -> Vec<usize> {
        let filter = self.filter[pane].text.to_lowercase();
        self.all(pane, ctx)
            .iter()
            .enumerate()
            .filter(|(_, (text, _))| text.to_lowercase().contains(&filter))
            .map(|(i, _)| i)
            .collect()
    }

    /// Enter on row `i`: resumes that session, reviews request `i`, or makes a session of
    /// the checked rows (or row `i` alone): from the parent of the bottom commit (or
    /// before the root) to the top row.
    fn open(&self, i: usize, ctx: &mut Pick) -> Result<Outcome<Pick>> {
        match self.focused {
            SESSIONS => {
                let id = ctx.sessions[i].0.id.clone();
                return resume(&id, ctx);
            }
            REVIEWS => return review(i, ctx),
            _ => {}
        }
        let (top, bottom) = self.checked.unwrap_or((i, i));
        let all = self.all(RANGE, ctx);
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

/// Opens session `id`, its request fetched again first.
fn resume(id: &str, ctx: &mut Pick) -> Result<Outcome<Pick>> {
    let session = ctx.store.load(id)?;
    if let Err(e) = forge::refresh(&ctx.repo, &session.base, &session.end, false) {
        ctx.error = Some(format!("{e:#}"));
        return Ok(Outcome::Consumed);
    }
    ctx.open = Some(session);
    Ok(Outcome::Quit)
}

/// Enter on request `i`: resumes its newest session, or fetches it and starts one from
/// where it left its target.
fn review(i: usize, ctx: &mut Pick) -> Result<Outcome<Pick>> {
    let Reviews::Listed(forge, requests) = &ctx.reviews else {
        return Ok(Outcome::Consumed);
    };
    let (forge, r) = (forge.clone(), requests[i].clone());
    let head = forge.head(r.number);
    if let Some((s, _)) = ctx.sessions.iter().find(|(s, _)| s.end == head) {
        let id = s.id.clone();
        return resume(&id, ctx);
    }
    let started = forge.fetch(&ctx.repo, &r).and_then(|base| {
        let (base, end) = ctx.repo.resolve(&format!("{base}..{head}"))?;
        let range = format!("{}..{end}", short(&base));
        let mut session = Session::new(ctx.store.new_id(), range, base, end);
        // A name is one line.
        let title = r.title.replace(char::is_control, " ");
        session.set_name(&format!("{} {title}", forge.sign(r.number)))?;
        Ok(session)
    });
    match started {
        Ok(session) => {
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

impl Panes<Pick> for Picker {
    fn act(&mut self, action: &str, ctx: &mut Pick) -> Result<Outcome<Pick>> {
        let pane = self.pane();
        // Down on the last row of the unfiltered range block loads older commits.
        if pane == RANGE
            && action == "down"
            && !self.exhausted
            && self.filter[RANGE].text.is_empty()
            && self.cursor[RANGE] + 1 == self.uncommitted.len() + self.commits.len()
            && let Err(e) = self.more(&ctx.repo)
        {
            ctx.error = Some(format!("{e:#}"));
        }
        let shown = self.shown(pane, ctx);
        let at = shown.get(self.cursor[pane]).copied();
        let cursor = self.cursor[pane];
        if let Some(to) = step(cursor, action, shown.len(), self.height) {
            self.cursor[pane] = to;
        }
        match action {
            "check" if pane == RANGE => {
                if let Some(i) = at {
                    self.checked = check(self.checked, i);
                }
            }
            "filter" => self.filter[pane].typing = true,
            "delete" if pane == SESSIONS => {
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
            "edit" if pane == SESSIONS => {
                if let Some(i) = at {
                    return Ok(Outcome::Push(Box::new(Rename::new(&ctx.sessions[i].0))));
                }
            }
            "help" => return Ok(Outcome::Push(Box::new(Help::new(HELP, &self.keymap)))),
            "settings" => return Ok(Outcome::Push(Box::new(SettingsPopup::new(&ctx.look)))),
            "focus_left" => self.focus((pane + TITLES.len() - 1) % TITLES.len(), ctx),
            "focus_right" => self.focus((pane + 1) % TITLES.len(), ctx),
            "quit" => return Ok(Outcome::Quit),
            "cancel" => {
                if pane == RANGE && self.checked.is_some() {
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

    /// Focusing the requests lists them the first time, and again after a failure.
    fn focus(&mut self, pane: usize, ctx: &mut Pick) {
        // A wheel turn or click focuses too: only entering lists again.
        let entering = self.focused != pane;
        self.focused = pane;
        let failed = matches!(ctx.reviews, Reviews::Failed(_));
        if pane == REVIEWS && (matches!(ctx.reviews, Reviews::Unlisted) || entering && failed) {
            ctx.reviews = Reviews::Listing;
        }
    }

    fn busy(&self) -> bool {
        self.filter[self.pane()].typing
    }

    /// A session or a request opens as Enter does, a range row checks as Space does.
    fn pick(&mut self, pane: usize, y: u16, ctx: &mut Pick) -> Result<Outcome<Pick>> {
        let len = self.shown(pane, ctx).len();
        let Some(row) = row_at(self.panes[pane], self.skip(pane), len, y) else {
            return Ok(Outcome::Consumed);
        };
        self.cursor[pane] = row;
        self.act(["open", "check", "open"][pane], ctx)
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
        self.cursor[SESSIONS] =
            self.cursor[SESSIONS].min(self.shown(SESSIONS, ctx).len().saturating_sub(1));
        // The focused block takes the height the others leave, a title line each.
        let heights = (0..TITLES.len()).map(|pane| {
            if pane == self.focused {
                Constraint::Fill(1)
            } else {
                Constraint::Length(1)
            }
        });
        self.panes = Layout::vertical(heights).areas(area);
        let pane = self.pane();
        let area = self.panes[pane];
        self.height = area.height.saturating_sub(2) as usize;
        for (p, &rect) in self.panes.iter().enumerate() {
            let count = match (p, &ctx.reviews) {
                (SESSIONS, _) => format!(" ({})", ctx.sessions.len()),
                (REVIEWS, Reviews::Listed(_, requests)) => format!(" ({})", requests.len()),
                _ => String::new(),
            };
            let title = format!(" {}{count} {}", TITLES[p], self.filter[p].title());
            if p == pane {
                let block = boxed(title, true, theme);
                let inner = block.inner(rect);
                f.render_widget(block, rect);
                self.render_rows(f, inner, ctx);
            } else {
                let line = Block::new()
                    .borders(Borders::TOP)
                    .border_style(Style::new().fg(theme.dim))
                    .title(title);
                f.render_widget(line, rect);
            }
        }
    }
}

impl Picker {
    /// The rows of the focused block in `inner`, the cursor's marked.
    fn render_rows(&self, f: &mut Frame, inner: Rect, ctx: &Pick) {
        let theme = ctx.look.theme;
        let pane = self.pane();
        let (height, width) = (inner.height as usize, inner.width as usize);
        let cursor = self.cursor[pane];
        let skip = self.skip(pane);
        let shown = self.shown(pane, ctx);
        let rows = shown.len();
        let shown = shown.into_iter().skip(skip).take(height);
        let lines: Vec<Line> = match pane {
            SESSIONS => {
                let name_width = name_width(&ctx.sessions);
                shown
                    .map(|i| Line::from(session_row(&ctx.sessions[i].0, name_width, width, theme)))
                    .collect()
            }
            RANGE => {
                let columns = Columns::new(&self.commits);
                shown
                    .map(|i| {
                        let checked = self.checked.is_some_and(|(t, b)| (t..=b).contains(&i));
                        let mut spans = vec![mark(checked, theme)];
                        // After the mark.
                        let width = width.saturating_sub(2);
                        match i.checked_sub(self.uncommitted.len()) {
                            Some(c) => spans.extend(columns.row(&self.commits[c], width, theme)),
                            None => spans.push(Span::raw(self.uncommitted[i])),
                        }
                        Line::from(spans)
                    })
                    .collect()
            }
            _ => match &ctx.reviews {
                Reviews::Listed(forge, requests) if !requests.is_empty() => {
                    let number_width = requests.iter().map(|r| r.number.to_string().len());
                    let number_width = number_width.max().unwrap_or(0);
                    shown
                        .map(|i| {
                            let r = &requests[i];
                            let head = forge.head(r.number);
                            let reviewed = ctx.sessions.iter().any(|(s, _)| s.end == head);
                            let row = (forge.word(), r, reviewed, number_width);
                            Line::from(review_row(row, width, theme))
                        })
                        .collect()
                }
                Reviews::Listed(..) => vec![Line::styled("no open requests", theme.dim)],
                Reviews::Failed(e) => e.lines().map(|l| Line::styled(l, theme.red)).collect(),
                Reviews::Unlisted | Reviews::Listing => {
                    vec![Line::styled("listing…", theme.dim)]
                }
            },
        };
        f.render_widget(Paragraph::new(lines), inner);
        if cursor < rows && height > 0 {
            cursor_bar(f, inner, inner.y + (cursor - skip) as u16, theme);
        }
    }
}

/// `● pull 12  title  author → target` in `width` columns, the author and target
/// right-aligned and left out first when narrow; ● when a session reviews it.
fn review_row<'a>(
    (word, r, reviewed, number_width): (&str, &'a Request, bool, usize),
    width: usize,
    theme: &Theme,
) -> Vec<Span<'a>> {
    let mark = if reviewed { "● " } else { "  " };
    let id = format!("{word} {:<number_width$}  ", r.number);
    let mut meta = format!("{} → {}", r.author, r.target);
    // After the mark and the id, the spaces before the meta.
    let width = width.saturating_sub(2 + id.width() + 2);
    if width < meta.width() + MIN_SUBJECT {
        meta.clear();
    }
    let room = width - meta.width();
    let title = fit(&r.title, room);
    let pad = " ".repeat(room - title.width() + 2);
    vec![
        Span::styled(mark, Style::new().fg(theme.accent)),
        Span::styled(id, Style::new().fg(theme.dim)),
        Span::raw(title),
        Span::raw(pad),
        Span::styled(meta, Style::new().fg(theme.dim)),
    ]
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

/// Columns the rename box shows; a longer name scrolls in it.
const RENAME_WIDTH: u16 = 80;

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
            // Kept to one line.
            _ => {
                let before = text.clone();
                if text.input(key) && text.lines().len() > 1 {
                    *text = before;
                }
            }
        }
        Ok(Outcome::Consumed)
    }

    fn paste(&mut self, text: &str) {
        // Its first line.
        let line: String = text
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_control())
            .collect();
        self.text.insert_str(line);
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Pick) {
        dialog::input(
            f,
            area,
            "Rename session",
            &self.text,
            // And the cursor after the name.
            RENAME_WIDTH + 1,
            "Enter save  Esc cancel  blank clears",
            ctx.look.theme,
        );
    }
}
