use std::ops::RangeInclusive;

use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::{Component, Ctx, Outcome};
use crate::doc::{Doc, Row, Side};
use crate::highlight::Colors;
use crate::repo::Kind;
use crate::session::Thread;
use crate::theme::Theme;
use crate::ui::confirm::Confirm;
use crate::ui::file_view::{self, FileView, Link, LinkPicker};
use crate::ui::input::Input;
use crate::ui::query::{self, Query};
use crate::ui::scroll::Scroll;
use crate::ui::{cursor_bar, status_style};

const TAB: &str = "    ";
/// Threads start where code text does, after the line numbers.
pub const INDENT: usize = 5;

/// Side-by-side diff with a row cursor. Rows wrap; a cursor row taller than the screen
/// scrolls.
#[derive(Default)]
pub struct DiffView {
    scroll: Scroll,
    /// Start of a `v` selection.
    mark: Option<usize>,
    /// A message being written, drawn under this row; it takes all keys.
    draft: Option<(usize, Input)>,
    /// The links of the thread at the cursor to pick from; it takes all keys.
    links: Option<LinkPicker>,
    /// The `/` search; while typing, it takes all keys.
    search: Query,
}

/// What a row draws: full width with a style, or the two halves of a code row.
pub enum Drawn {
    Full(Vec<Line<'static>>, Style),
    Split([(Vec<Line<'static>>, Style); 2]),
}

impl Drawn {
    pub fn height(&self) -> usize {
        match self {
            Drawn::Full(lines, _) => lines.len(),
            Drawn::Split([l, r]) => l.0.len().max(r.0.len()),
        }
        .max(1)
    }
}

impl DiffView {
    fn selection(&self) -> RangeInclusive<usize> {
        let cursor = self.scroll.cursor;
        let mark = self.mark.unwrap_or(cursor);
        mark.min(cursor)..=mark.max(cursor)
    }

    fn thread_at_cursor(&self, ctx: &Ctx) -> Option<usize> {
        match ctx.doc.window(self.scroll.cursor, 1).first() {
            Some(Row::Thread { idx }) => Some(*idx),
            _ => None,
        }
    }

    /// The file the cursor is in.
    pub fn current_file(&self, ctx: &Ctx) -> Option<usize> {
        ctx.doc.file_at(self.scroll.cursor)
    }

    /// Starts writing `input` under `row`.
    pub fn start(&mut self, row: usize, input: Input) {
        self.scroll.cursor = row;
        self.mark = None;
        self.draft = Some((row, input));
    }

    /// `/text ` for the title while searching.
    pub fn search_title(&self) -> String {
        self.search.title()
    }

    /// Moves to the next (or previous) search match.
    fn find(&mut self, ctx: &mut Ctx, forward: bool) {
        let (cursor, len) = (self.scroll.cursor, ctx.doc.len());
        if let Some(row) = self
            .search
            .find(&mut ctx.doc, cursor, forward, &ctx.session.threads)
        {
            self.scroll.cursor = row;
            // Expanding a file shifts the rows under it.
            if ctx.doc.len() != len {
                self.mark = None;
            }
        }
    }

    /// Sends `key` to the link list, the draft or the search being typed, if any;
    /// None when there is none.
    pub fn modal_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Result<Option<Outcome>> {
        if let Some(picker) = &mut self.links {
            let Some(picked) = picker.key(key) else {
                return Ok(Some(Outcome::Consumed));
            };
            self.links = None;
            return Ok(Some(match picked {
                Some(link) => follow(&link, ctx)?,
                None => Outcome::Consumed,
            }));
        }
        if self.search.typing {
            if self.search.key(key) {
                self.find(ctx, true);
            }
            return Ok(Some(Outcome::Consumed));
        }
        let Some((_, input)) = &mut self.draft else {
            return Ok(None);
        };
        if let Outcome::Close = input.handle_key(key, ctx)? {
            self.draft = None;
        }
        Ok(Some(Outcome::Consumed))
    }

    pub fn paste(&mut self, text: &str) {
        if self.search.typing {
            self.search.paste(text);
        } else if let Some((_, input)) = &mut self.draft {
            input.paste(text);
        }
    }

    /// Moves the cursor to `row` at the top of the screen.
    pub fn jump(&mut self, row: usize) {
        self.scroll.jump(row);
        self.mark = None;
    }

    pub fn act(&mut self, action: &str, ctx: &mut Ctx) -> Result<Outcome> {
        if self.scroll.act(action, ctx.doc.len()) {
            return Ok(Outcome::Consumed);
        }
        let cursor = self.scroll.cursor;
        match action {
            "filter" => self.search = Query::start(),
            "next_thread" | "prev_thread" if !self.search.text.is_empty() => {
                self.find(ctx, action == "next_thread")
            }
            "next_thread" | "prev_thread" => {
                if let Some(i) = ctx.doc.next_thread(cursor, action == "next_thread") {
                    self.scroll.cursor = i;
                }
            }
            "next_file" | "prev_file" => {
                if let Some(i) = ctx.doc.next_file(cursor, action == "next_file") {
                    self.jump(i);
                }
            }
            "open" => {
                // On a thread: follow its link to a file view, or list its links.
                if let Some(idx) = self.thread_at_cursor(ctx) {
                    let mut links = file_view::thread_links(&ctx.session.threads[idx], &ctx.files);
                    if links.len() > 1 {
                        self.links = Some(LinkPicker::new(links));
                    } else if let Some(link) = links.pop() {
                        return follow(&link, ctx);
                    }
                    return Ok(Outcome::Consumed);
                }
                let row = ctx
                    .doc
                    .expand(cursor, |rev, path| ctx.repo.read(rev, path))?;
                self.scroll.cursor = row.min(ctx.doc.len() - 1);
                self.mark = None;
            }
            "select" => {
                self.mark = if self.mark.is_some() {
                    None
                } else {
                    Some(cursor)
                }
            }
            "cancel" if self.mark.is_some() => self.mark = None,
            "cancel" => self.search = Query::default(),
            "comment" => {
                if let Some(idx) = self.thread_at_cursor(ctx) {
                    self.start(cursor, Input::reply(idx));
                } else {
                    let selection = self.selection();
                    let anchor = ctx.doc.anchor_at(selection.clone());
                    let mut row = *selection.end();
                    // A comment on a collapsed file would be hidden with it. Here, that is
                    // one on its header, whose rows the expanding shifts.
                    if let Some(file) = ctx.doc.file_of(&anchor)
                        && ctx.doc.collapsed(file)
                    {
                        row = ctx.doc.expand_file(file);
                    }
                    self.start(row, Input::comment(anchor, &ctx.session));
                }
            }
            // Only the user's last message, before the agent answers it.
            "edit" | "delete" => {
                if let Some(idx) = self.thread_at_cursor(ctx)
                    && ctx.session.threads[idx].waiting_on_agent()
                {
                    if action == "edit" {
                        self.start(cursor, Input::edit(idx, &ctx.session));
                    } else {
                        let t = &ctx.session.threads[idx];
                        let question = if t.messages.len() == 1 {
                            format!("delete thread {}?", t.id)
                        } else {
                            "delete your last message?".to_string()
                        };
                        return Ok(Outcome::Push(Box::new(
                            Confirm::new("Delete", question, "y delete  n/Esc cancel")
                                .on('y', move |ctx: &mut Ctx| {
                                    ctx.delete(idx)?;
                                    Ok(Outcome::Close)
                                })
                                .on('n', |_| Ok(Outcome::Close)),
                        )));
                    }
                }
            }
            "collapse" => {
                if let Some(file) = self.current_file(ctx) {
                    self.scroll.cursor = if ctx.doc.collapsed(file) {
                        ctx.doc.expand_file(file)
                    } else {
                        ctx.doc.collapse_file(file)
                    };
                    self.mark = None;
                }
            }
            _ => {}
        }
        Ok(Outcome::Consumed)
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let doc = &ctx.doc;
        self.scroll.cursor = self.scroll.cursor.min(doc.len() - 1);
        let selection = self.selection();
        let editing = self.draft.as_ref().and_then(|(_, input)| input.editing());
        let search = &self.search.text;
        let draw = |i: usize| {
            let accent = Style::new()
                .fg(ctx.look.theme.on_accent)
                .bg(ctx.look.theme.accent);
            let mark = selection.contains(&i).then_some(accent);
            draw_row(ctx, doc, i, area.width, mark, editing, search)
        };
        let draft_at = self.draft.as_ref().map(|(row, _)| *row);
        let draft_height = self
            .draft
            .as_ref()
            .map_or(0, |(_, input)| input.height(area.width as usize));
        // The draft counts as part of its row, so it stays on screen with the cursor; under
        // a row taller than the screen, it shows from its end.
        let height =
            |i: usize| draw(i).height() + if draft_at == Some(i) { draft_height } else { 0 };
        let from_end = draft_at == Some(self.scroll.cursor);
        self.scroll
            .place(area.height as usize, doc.len(), height, from_end);
        let draft = &mut self.draft;
        draw_rows(
            f,
            area,
            &self.scroll,
            doc,
            ctx.look.theme,
            draw,
            |f, i, rest| match draft {
                Some((row, input)) if *row == i => {
                    let shown = draft_height.min(rest.height as usize);
                    input.render(
                        f,
                        Rect {
                            height: shown as u16,
                            ..rest
                        },
                        ctx,
                    );
                    shown
                }
                _ => 0,
            },
        );
        if let Some(picker) = &mut self.links {
            picker.render(f, area, ctx.look.theme);
        }
    }
}

/// Opens `link` in a file view.
fn follow(link: &Link, ctx: &mut Ctx) -> Result<Outcome> {
    Ok(match FileView::open(ctx, link)? {
        Some(view) => Outcome::Push(Box::new(view)),
        None => Outcome::Consumed,
    })
}

/// Draws `doc`'s rows from the top of `scroll` on, each as `draw` makes it, the cursor
/// row's first line barred unless a code row marks it in its gutter. Under row `i` goes
/// what `under(f, i, rest)` draws in the `rest` of the area, returning its height.
pub fn draw_rows(
    f: &mut Frame,
    area: Rect,
    scroll: &Scroll,
    doc: &Doc,
    theme: &Theme,
    draw: impl Fn(usize) -> Drawn,
    mut under: impl FnMut(&mut Frame, usize, Rect) -> usize,
) {
    let h = area.height as usize;
    let half = area.width / 2;
    let (top, off) = scroll.top();
    let (mut y, mut i, mut skip) = (0, top, off);
    while y < h && i < doc.len() {
        let drawn = draw(i);
        let shown = drawn.height().saturating_sub(skip).min(h - y);
        let rect = Rect {
            y: area.y + y as u16,
            height: shown as u16,
            ..area
        };
        let para = |lines, style| Paragraph::new(lines).style(style).scroll((skip as u16, 0));
        match drawn {
            Drawn::Full(lines, style) => f.render_widget(para(lines, style), rect),
            Drawn::Split([(left, ls), (right, rs)]) => {
                f.render_widget(
                    para(left, ls),
                    Rect {
                        width: half,
                        ..rect
                    },
                );
                let right_rect = Rect {
                    x: area.x + half,
                    width: area.width - half,
                    ..rect
                };
                f.render_widget(para(right, rs), right_rect);
            }
        }
        if i == scroll.cursor && skip == 0 && !matches!(doc.window(i, 1)[0], Row::Code { .. }) {
            cursor_bar(f, area, rect.y, theme);
        }
        y += shown;
        if y < h {
            let rest = Rect {
                y: area.y + y as u16,
                height: (h - y) as u16,
                ..area
            };
            y += under(f, i, rest);
        }
        i += 1;
        skip = 0;
    }
}

/// `mark`: the gutter's style, for the selection or a file view's linked lines.
/// `editing`: the thread whose last message the draft replaces.
/// `search`: the text whose matches to mark.
pub fn draw_row(
    ctx: &Ctx,
    doc: &Doc,
    i: usize,
    width: u16,
    mark: Option<Style>,
    editing: Option<usize>,
    search: &str,
) -> Drawn {
    let theme = ctx.look.theme;
    let header = Style::new().fg(theme.blue).add_modifier(Modifier::BOLD);
    let text = match &doc.window(i, 1)[0] {
        Row::Session => "# session".to_string(),
        Row::File {
            file,
            status,
            path,
            note,
        } => {
            let mut rest = String::new();
            if let Some(note) = note {
                rest.push_str(&format!("  ({note})"));
            }
            if doc.collapsed(*file) {
                rest.push_str("  (reviewed)");
            }
            // Only the path is searched.
            let mut spans = vec![
                Span::styled(status.to_string(), status_style(*status, theme)),
                Span::raw(" "),
            ];
            let path = vec![Span::raw(path.clone())];
            spans.extend(query::mark(path, search, query::hit_style(theme)));
            spans.push(Span::raw(rest));
            let line = Line::from(spans);
            return Drawn::Full(vec![line], header);
        }
        Row::Fold { hidden, .. } => {
            let text = match hidden {
                Some(n) => format!("  ... {n} unchanged lines ..."),
                None => "  ... more lines ...".to_string(),
            };
            return Drawn::Full(vec![Line::raw(text)], Style::new().fg(theme.dim));
        }
        Row::Thread { idx } => {
            return Drawn::Full(
                thread_lines(
                    ctx,
                    *idx,
                    !doc.at_top(i),
                    editing == Some(*idx),
                    width as usize,
                    search,
                ),
                Style::new().bg(theme.surface),
            );
        }
        Row::Code { file, left, right } => {
            let (left_rev, right_rev) = doc.revs();
            let (old_path, new_path) = &doc.paths()[*file];
            let colors = |side: &Option<Side>, rev, path| {
                side.as_ref().and_then(|_| ctx.highlighter.get(rev, path))
            };
            // Added and deleted files, and the file view, take the full width.
            if doc.one_sided(*file) {
                let (side, colors) = match right {
                    Some(_) => (right, colors(right, right_rev, new_path)),
                    None => (left, colors(left, left_rev, old_path)),
                };
                let (lines, style) = code_lines(
                    side.as_ref(),
                    colors,
                    width as usize,
                    mark,
                    theme,
                    ctx.look.wrap,
                    search,
                );
                return Drawn::Full(lines, style);
            }
            let half = width / 2;
            return Drawn::Split([
                code_lines(
                    left.as_ref(),
                    colors(left, left_rev, old_path),
                    half as usize,
                    mark,
                    theme,
                    ctx.look.wrap,
                    search,
                ),
                code_lines(
                    right.as_ref(),
                    colors(right, right_rev, new_path),
                    (width - half) as usize,
                    mark,
                    theme,
                    ctx.look.wrap,
                    search,
                ),
            ]);
        }
    };
    Drawn::Full(vec![Line::raw(text)], header)
}

/// One side of a code row, wrapped by grapheme to `width`; `mark` styles the gutter,
/// and the matches of `search` are marked.
fn code_lines(
    side: Option<&Side>,
    colors: Option<&Colors>,
    width: usize,
    mark: Option<Style>,
    theme: &Theme,
    wrap: bool,
    search: &str,
) -> (Vec<Line<'static>>, Style) {
    let Some(s) = side else {
        return (Vec::new(), Style::new());
    };
    let hits = query::hits(&s.text, search);
    // Changes show in the line background and number color; changed words in `emph`.
    let (color, bg, emph) = match s.kind {
        Kind::Ctx => (theme.dim, theme.bg, theme.bg),
        Kind::Del => (theme.red, theme.del_bg, theme.del_emph),
        Kind::Add => (theme.green, theme.add_bg, theme.add_emph),
    };
    let mut gutter = Style::new().fg(color);
    if let Some(mark) = mark {
        gutter = gutter.patch(mark);
    }
    let number = format!("{:>1$} ", s.lineno, INDENT - 1);
    let gutter_width = number.len();
    let text_width = width.saturating_sub(gutter_width).max(1);
    let mut runs = colors
        .and_then(|c| c.get(s.lineno as usize - 1))
        .into_iter()
        .flatten();
    let (mut run_end, mut fg) = (0, None);

    let mut lines = Vec::new();
    let mut spans = vec![Span::styled(number, gutter)];
    let (mut chunk, mut chunk_style, mut col) = (String::new(), Style::new(), 0);
    for (at, g) in s.text.grapheme_indices(true) {
        while at >= run_end {
            match runs.next() {
                Some((slot, len)) => {
                    fg = Some(theme.slot(*slot));
                    run_end += len;
                }
                None => {
                    fg = None;
                    run_end = usize::MAX;
                }
            }
        }
        let g = match g {
            "\t" => TAB,
            "\r" | "\n" => continue,
            g => g,
        };
        let w = g.width();
        if col + w > text_width && !wrap {
            break;
        }
        if col + w > text_width && col > 0 {
            spans.push(Span::styled(std::mem::take(&mut chunk), chunk_style));
            lines.push(Line::from(std::mem::take(&mut spans)));
            spans.push(Span::styled(" ".repeat(gutter_width), gutter));
            col = 0;
        }
        let mut style = Style::new();
        if let Some(fg) = fg {
            style = style.fg(fg);
        }
        if s.emph.iter().any(|r| r.contains(&at)) {
            style = style.bg(emph);
        }
        if hits.iter().any(|r| r.contains(&at)) {
            style = style.patch(query::hit_style(theme));
        }
        if style != chunk_style && !chunk.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut chunk), chunk_style));
        }
        chunk_style = style;
        chunk.push_str(g);
        col += w;
    }
    spans.push(Span::styled(chunk, chunk_style));
    lines.push(Line::from(spans));
    (lines, Style::new().bg(bg))
}

/// A thread as a card: header, then each message wrapped to `width`.
/// `inline` threads sit under what they anchor to, so their header leaves out the path.
fn thread_lines(
    ctx: &Ctx,
    idx: usize,
    inline: bool,
    hide_last: bool,
    width: usize,
    search: &str,
) -> Vec<Line<'static>> {
    let theme = ctx.look.theme;
    let t: &Thread = &ctx.session.threads[idx];
    let outdated = t.outdated;
    let line = |spans: Vec<Span<'static>>| {
        Line::from([vec![Span::raw(" ".repeat(INDENT))], spans].concat())
    };
    let inner = width.saturating_sub(INDENT);
    let place = if inline {
        ctx.session.inline_location(&t.anchor)
    } else {
        ctx.session.location(&t.anchor)
    };
    let mut header = format!("{}  {place}", t.id);
    if t.waiting_on_agent() {
        header.push_str("  waiting on agent");
    }
    if outdated {
        header.push_str("  outdated");
    }
    let header_style = Style::new().add_modifier(Modifier::BOLD);
    let mut header = vec![Span::styled(header, header_style)];
    let links = file_view::thread_links(t, &ctx.files).len();
    if links > 0 {
        let s = if links == 1 { "" } else { "s" };
        header.push(Span::styled(
            format!("  [{links} link{s}]"),
            header_style.fg(theme.blue),
        ));
    }
    let mut lines = vec![line(header)];
    if outdated && let Some(text) = &t.text {
        let dim = Style::new().fg(theme.dim);
        lines.push(line(vec![Span::styled("was:", dim)]));
        for l in text.lines() {
            lines.push(line(vec![Span::styled(format!("  {}", clean(l)), dim)]));
        }
    }
    let link = Style::new()
        .fg(theme.blue)
        .add_modifier(Modifier::UNDERLINED);
    for m in &t.messages[..t.messages.len() - hide_last as usize] {
        let who = Style::new().fg(if m.from == "user" {
            theme.teal
        } else {
            theme.mauve
        });
        let indent = m.from.width() + 2;
        let body_width = inner.saturating_sub(indent).max(10);
        // Broken at spaces only, so a link stays on one line unless longer than it.
        let options = textwrap::Options::new(body_width)
            .word_separator(textwrap::WordSeparator::AsciiSpace)
            .word_splitter(textwrap::WordSplitter::NoHyphenation);
        let mut first = true;
        for l in m.body.lines() {
            for part in textwrap::wrap(&clean(l), &options) {
                let prefix = if first {
                    format!("{}: ", m.from)
                } else {
                    " ".repeat(indent)
                };
                first = false;
                let mut body = Vec::new();
                let mut at = 0;
                for (range, _) in file_view::links(&part, &ctx.files) {
                    body.push(Span::raw(part[at..range.start].to_string()));
                    body.push(Span::styled(part[range.clone()].to_string(), link));
                    at = range.end;
                }
                body.push(Span::raw(part[at..].to_string()));
                // Only the body is searched.
                let mut spans = vec![Span::styled(prefix, who)];
                spans.extend(query::mark(body, search, query::hit_style(theme)));
                lines.push(line(spans));
            }
        }
    }
    lines
}

fn clean(s: &str) -> String {
    s.replace('\t', TAB).replace('\r', "")
}
