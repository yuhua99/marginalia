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
use crate::session::{Anchor, Thread};
use crate::theme::Theme;
use crate::ui::delete_prompt::DeletePrompt;
use crate::ui::file_view::{self, FileView, Link, LinkPicker};
use crate::ui::input::Input;
use crate::ui::{cursor_bar, status_style};

const TAB: &str = "    ";
/// Threads start where code text does, after the line numbers.
pub const INDENT: usize = 5;

/// Side-by-side diff with a row cursor. Rows wrap; the screen starts `off` lines
/// into row `top`, which only happens for a cursor row taller than the screen.
#[derive(Default)]
pub struct DiffView {
    cursor: usize,
    top: usize,
    off: usize,
    height: usize,
    /// The cursor row goes on below the screen.
    overflow: bool,
    /// Start of a `v` selection.
    mark: Option<usize>,
    /// Shows `ctx.file` instead of `ctx.doc`, read only: the link it was opened from, as
    /// `path:line`, and the rows of the linked lines.
    file: Option<(String, RangeInclusive<usize>)>,
    /// A message being written, drawn under this row; it takes all keys.
    draft: Option<(usize, Input)>,
    /// The links of the thread at the cursor to pick from; it takes all keys.
    links: Option<LinkPicker>,
}

/// What a row draws: full width with a style, or the two halves of a code row.
enum Drawn {
    Full(Vec<Line<'static>>, Style),
    Split([(Vec<Line<'static>>, Style); 2]),
}

impl Drawn {
    fn height(&self) -> usize {
        match self {
            Drawn::Full(lines, _) => lines.len(),
            Drawn::Split([l, r]) => l.0.len().max(r.0.len()),
        }
        .max(1)
    }
}

impl DiffView {
    fn selection(&self) -> RangeInclusive<usize> {
        let mark = self.mark.unwrap_or(self.cursor);
        mark.min(self.cursor)..=mark.max(self.cursor)
    }

    /// A view of `ctx.file`, opened from `link`, with the cursor on its rows `start..=end`.
    pub fn file(start: usize, end: usize, link: &Link) -> DiffView {
        DiffView {
            cursor: start,
            top: start,
            file: Some((link.label(), start..=end)),
            ..DiffView::default()
        }
    }

    /// The link a file view was opened from.
    pub fn link(&self) -> &str {
        self.file.as_ref().map_or("", |(link, _)| link)
    }

    fn doc<'a>(&self, ctx: &'a Ctx) -> &'a Doc {
        if self.file.is_some() {
            ctx.file.as_ref().expect("file view without ctx.file")
        } else {
            &ctx.doc
        }
    }

    fn thread_at_cursor(&self, ctx: &Ctx) -> Option<usize> {
        match self.doc(ctx).window(self.cursor, 1).first() {
            Some(Row::Thread { idx }) => Some(*idx),
            _ => None,
        }
    }

    /// The file the cursor is in.
    pub fn current_file(&self, ctx: &Ctx) -> Option<usize> {
        self.doc(ctx).file_at(self.cursor)
    }

    /// Starts writing `input` under `row`.
    pub fn start(&mut self, row: usize, input: Input) {
        self.cursor = row;
        self.mark = None;
        self.draft = Some((row, input));
    }

    /// Sends `key` to the link list or the draft, if one is open; None when neither is.
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
        let Some((_, input)) = &mut self.draft else {
            return Ok(None);
        };
        if let Outcome::Close = input.handle_key(key, ctx)? {
            self.draft = None;
        }
        Ok(Some(Outcome::Consumed))
    }

    pub fn paste(&mut self, text: &str) {
        if let Some((_, input)) = &mut self.draft {
            input.paste(text);
        }
    }

    /// Moves the cursor to `row` at the top of the screen.
    pub fn jump(&mut self, row: usize) {
        self.cursor = row;
        self.top = row;
        self.off = 0;
        self.mark = None;
    }

    pub fn act(&mut self, action: &str, ctx: &mut Ctx) -> Result<Outcome> {
        let last = self.doc(ctx).len() - 1;
        let half = (self.height / 2).max(1);
        // Inside a cursor row taller than the screen, j / k scroll the row first.
        let scrolled = self.top == self.cursor && self.off > 0;
        match action {
            "down" if self.overflow => self.off += 1,
            "half_page_down" if self.overflow => self.off += half,
            "up" if scrolled => self.off -= 1,
            "half_page_up" if scrolled => self.off = self.off.saturating_sub(half),
            "down" => self.cursor = (self.cursor + 1).min(last),
            "up" => self.cursor = self.cursor.saturating_sub(1),
            "half_page_down" => self.cursor = (self.cursor + half).min(last),
            "half_page_up" => self.cursor = self.cursor.saturating_sub(half),
            "top" => self.cursor = 0,
            "bottom" => self.cursor = last,
            "next_thread" | "prev_thread" => {
                if let Some(i) = self
                    .doc(ctx)
                    .next_thread(self.cursor, action == "next_thread")
                {
                    self.cursor = i;
                }
            }
            "next_file" | "prev_file" => {
                if let Some(i) = self.doc(ctx).next_file(self.cursor, action == "next_file") {
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
                if self.file.is_some() {
                    return Ok(Outcome::Consumed);
                }
                let row = ctx
                    .doc
                    .expand(self.cursor, &ctx.session.threads, |rev, path| {
                        ctx.repo.read(rev, path)
                    })?;
                self.cursor = row.min(ctx.doc.len() - 1);
                self.mark = None;
            }
            // A file view is read only.
            "select" | "comment" | "edit" | "delete" if self.file.is_some() => {}
            "select" => {
                self.mark = if self.mark.is_some() {
                    None
                } else {
                    Some(self.cursor)
                }
            }
            "cancel" => self.mark = None,
            "comment" => {
                if let Some(idx) = self.thread_at_cursor(ctx) {
                    self.start(self.cursor, Input::reply(idx));
                } else {
                    let selection = self.selection();
                    let anchor = self.doc(ctx).anchor_at(selection.clone());
                    let mut row = *selection.end();
                    // A comment on a collapsed file would be hidden with it. Here, that is
                    // one on its header, whose rows the expanding shifts.
                    if let Some(file) = ctx.doc.file_of(&anchor)
                        && ctx.doc.collapsed(file)
                    {
                        row = ctx.doc.expand_file(file, &ctx.session.threads);
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
                        self.start(self.cursor, Input::edit(idx, &ctx.session));
                    } else {
                        return Ok(Outcome::Push(Box::new(DeletePrompt(idx))));
                    }
                }
            }
            "collapse" => {
                if self.file.is_none()
                    && let Some(file) = self.current_file(ctx)
                {
                    self.cursor = if ctx.doc.collapsed(file) {
                        ctx.doc.expand_file(file, &ctx.session.threads)
                    } else {
                        ctx.doc.collapse_file(file, &ctx.session.threads)
                    };
                    self.mark = None;
                }
            }
            _ => {}
        }
        Ok(Outcome::Consumed)
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let doc = self.doc(ctx);
        self.cursor = self.cursor.min(doc.len() - 1);
        self.height = area.height as usize;
        let (h, cursor, selection) = (self.height, self.cursor, self.selection());
        let editing = self.draft.as_ref().and_then(|(_, input)| input.editing());
        let linked = self.file.as_ref().map(|(_, rows)| rows.clone());
        let draw = |i: usize| {
            // The selection as an accent block; linked lines as bold blue numbers, apart
            // from it in themes whose accent is blue.
            let mark = if selection.contains(&i) {
                Some(Style::new().fg(ctx.theme.on_accent).bg(ctx.theme.accent))
            } else if linked.as_ref().is_some_and(|rows| rows.contains(&i)) {
                Some(Style::new().fg(ctx.theme.blue).add_modifier(Modifier::BOLD))
            } else {
                None
            };
            draw_row(ctx, doc, i, area.width, mark, editing)
        };
        let draft_at = self.draft.as_ref().map(|(row, _)| *row);
        let draft_height = self
            .draft
            .as_ref()
            .map_or(0, |(_, input)| input.height(area.width as usize));
        // The draft counts as part of its row, so it stays on screen with the cursor.
        let height =
            |i: usize| draw(i).height() + if draft_at == Some(i) { draft_height } else { 0 };

        // Keep the cursor row on screen: whole if it fits, else from its start.
        if cursor < self.top {
            self.top = cursor;
            self.off = 0;
        }
        let mut bottom = 0;
        for i in self.top..=cursor {
            bottom += height(i);
            if bottom > h + self.off {
                break;
            }
        }
        if bottom > h + self.off && self.top != cursor {
            self.top = cursor;
            self.off = 0;
            let mut used = height(cursor);
            while self.top > 0 && used + height(self.top - 1) <= h {
                self.top -= 1;
                used += height(self.top);
            }
        }
        let top_height = height(self.top);
        self.off = if self.top == cursor {
            self.off.min(top_height.saturating_sub(h))
        } else {
            self.off.min(top_height - 1)
        };
        // A draft under a row taller than the screen shows from its end.
        if draft_at == Some(cursor) && self.top == cursor {
            self.off = top_height.saturating_sub(h);
        }
        self.overflow = self.top == cursor && top_height - self.off > h;

        let half = area.width / 2;
        let (mut y, mut i, mut skip) = (0, self.top, self.off);
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
            // A code row shows the cursor in its gutter; other rows, a bar on their first line.
            if i == cursor && skip == 0 && !matches!(doc.window(i, 1)[0], Row::Code { .. }) {
                cursor_bar(f, area, rect.y, ctx.theme);
            }
            y += shown;
            if draft_at == Some(i)
                && y < h
                && let Some((_, input)) = &mut self.draft
            {
                let shown = draft_height.min(h - y);
                let rect = Rect {
                    y: area.y + y as u16,
                    height: shown as u16,
                    ..area
                };
                input.render(f, rect, ctx);
                y += shown;
            }
            i += 1;
            skip = 0;
        }
        if let Some(picker) = &self.links {
            picker.render(f, area, ctx.theme);
        }
    }
}

/// Opens `link` in a file view.
fn follow(link: &Link, ctx: &mut Ctx) -> Result<Outcome> {
    let Some((start, end)) = file_view::open(ctx, link)? else {
        return Ok(Outcome::Consumed);
    };
    Ok(Outcome::Push(Box::new(FileView::new(start, end, link))))
}

/// `mark`: the gutter's style, for the selection or a file view's linked lines.
/// `editing`: the thread whose last message the draft replaces.
fn draw_row(
    ctx: &Ctx,
    doc: &Doc,
    i: usize,
    width: u16,
    mark: Option<Style>,
    editing: Option<usize>,
) -> Drawn {
    let theme = ctx.theme;
    let header = Style::new().fg(theme.blue).add_modifier(Modifier::BOLD);
    let text = match &doc.window(i, 1)[0] {
        Row::Session => "# session".to_string(),
        Row::File {
            file,
            status,
            path,
            note,
        } => {
            let mut text = path.clone();
            if let Some(note) = note {
                text.push_str(&format!("  ({note})"));
            }
            if doc.collapsed(*file) {
                text.push_str("  (reviewed)");
            }
            let line = Line::from(vec![
                Span::styled(status.to_string(), status_style(*status, theme)),
                Span::raw(format!(" {text}")),
            ]);
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
                let (lines, style) =
                    code_lines(side.as_ref(), colors, width as usize, mark, theme, ctx.wrap);
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
                    ctx.wrap,
                ),
                code_lines(
                    right.as_ref(),
                    colors(right, right_rev, new_path),
                    (width - half) as usize,
                    mark,
                    theme,
                    ctx.wrap,
                ),
            ]);
        }
    };
    Drawn::Full(vec![Line::raw(text)], header)
}

/// One side of a code row, wrapped by grapheme to `width`; `mark` styles the gutter.
fn code_lines(
    side: Option<&Side>,
    colors: Option<&Colors>,
    width: usize,
    mark: Option<Style>,
    theme: &Theme,
    wrap: bool,
) -> (Vec<Line<'static>>, Style) {
    let Some(s) = side else {
        return (Vec::new(), Style::new());
    };
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
) -> Vec<Line<'static>> {
    let theme = ctx.theme;
    let outdated = ctx.outdated[idx];
    let t: &Thread = &ctx.session.threads[idx];
    let line = |spans: Vec<Span<'static>>| {
        Line::from([vec![Span::raw(" ".repeat(INDENT))], spans].concat())
    };
    let inner = width.saturating_sub(INDENT);
    let location = ctx.session.location(&t.anchor);
    let place = match (&t.anchor, inline) {
        (Anchor::Lines { file, .. }, true) => location.replacen(&format!("{file}:"), "L", 1),
        (Anchor::File { .. }, true) => "file".into(),
        _ => location,
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
                let mut spans = vec![Span::styled(prefix, who)];
                let mut at = 0;
                for (range, _) in file_view::links(&part, &ctx.files) {
                    spans.push(Span::raw(part[at..range.start].to_string()));
                    spans.push(Span::styled(part[range.clone()].to_string(), link));
                    at = range.end;
                }
                spans.push(Span::raw(part[at..].to_string()));
                lines.push(line(spans));
            }
        }
    }
    lines
}

fn clean(s: &str) -> String {
    s.replace('\t', TAB).replace('\r', "")
}
