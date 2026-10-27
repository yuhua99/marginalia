use std::ops::RangeInclusive;

use anyhow::Result;
use ratatui::Frame;
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
use crate::session::{Anchor, Status, Thread};
use crate::theme::Theme;
use crate::ui::delete_prompt::DeletePrompt;
use crate::ui::file_view::{self, FileView};
use crate::ui::input::Input;

const TAB: &str = "    ";
/// Threads start where code text does, after the line numbers.
const INDENT: usize = 5;

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
    /// Shows `ctx.file` instead of `ctx.doc`.
    file: bool,
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

    /// A view of `ctx.file` with lines `start..=end` selected.
    pub fn file(start: usize, end: usize) -> DiffView {
        DiffView {
            cursor: end,
            mark: (start != end).then_some(start),
            file: true,
            ..DiffView::default()
        }
    }

    fn doc<'a>(&self, ctx: &'a Ctx) -> &'a Doc {
        if self.file {
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

    pub fn jump(&mut self, row: usize) {
        self.cursor = row;
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
            "next_waiting" | "prev_waiting" => {
                if let Some(i) = self.doc(ctx).next_waiting(
                    self.cursor,
                    action == "next_waiting",
                    &ctx.session.threads,
                ) {
                    self.cursor = i;
                }
            }
            "next_file" | "prev_file" => {
                if let Some(i) = self.doc(ctx).next_file(self.cursor, action == "next_file") {
                    self.cursor = i;
                }
            }
            "open" => {
                // On a thread: follow its first link to a file view.
                if let Some(idx) = self.thread_at_cursor(ctx) {
                    if let Some((start, end)) = file_view::follow(ctx, idx)? {
                        if self.file {
                            *self = DiffView::file(start, end);
                        } else {
                            return Ok(Outcome::Push(Box::new(FileView::new(start, end))));
                        }
                    }
                    return Ok(Outcome::Consumed);
                }
                if self.file {
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
                    return Ok(Outcome::Push(Box::new(Input::reply(idx, &ctx.session))));
                }
                let anchor = self.doc(ctx).anchor_at(self.selection());
                self.mark = None;
                return Ok(Outcome::Push(Box::new(Input::comment(
                    anchor,
                    &ctx.session,
                ))));
            }
            // Only the user's last message of an open thread, before the agent answers it.
            "edit" | "delete" => {
                if let Some(idx) = self.thread_at_cursor(ctx)
                    && ctx.session.threads[idx].status == Status::Open
                    && ctx.session.threads[idx].waiting_on() == "agent"
                {
                    let next: Box<dyn Component> = if action == "edit" {
                        Box::new(Input::edit(idx, &ctx.session))
                    } else {
                        Box::new(DeletePrompt(idx))
                    };
                    return Ok(Outcome::Push(next));
                }
            }
            "resolve" => {
                if let Some(idx) = self.thread_at_cursor(ctx) {
                    let t = &mut ctx.session.threads[idx];
                    t.status = if t.status == Status::Open {
                        Status::Resolved
                    } else {
                        Status::Open
                    };
                    ctx.threads_changed()?;
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
        let draw =
            |i: usize| draw_row(ctx, doc, i, area.width, i == cursor, selection.contains(&i));
        let height = |i: usize| draw(i).height();

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
        self.overflow = self.top == cursor && top_height - self.off > h;

        let half = area.width / 2;
        let (mut y, mut i, mut skip) = (0, self.top, self.off);
        while y < h && i < doc.len() {
            let drawn = draw(i);
            let shown = (drawn.height() - skip).min(h - y);
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
            y += shown;
            i += 1;
            skip = 0;
        }
    }
}

fn draw_row(ctx: &Ctx, doc: &Doc, i: usize, width: u16, at_cursor: bool, selected: bool) -> Drawn {
    let mark = |style: Style| {
        if at_cursor {
            style.add_modifier(Modifier::REVERSED)
        } else {
            style
        }
    };
    let theme = ctx.theme;
    let header = Style::new().fg(theme.blue).add_modifier(Modifier::BOLD);
    let text = match &doc.window(i, 1)[0] {
        Row::Session => "# session".to_string(),
        Row::File {
            status, path, note, ..
        } => {
            let mut text = format!("{status} {path}");
            if let Some(note) = note {
                text.push_str(&format!("  ({note})"));
            }
            text
        }
        Row::Fold { hidden, .. } => {
            let text = match hidden {
                Some(n) => format!("  ... {n} unchanged lines ..."),
                None => "  ... more lines ...".to_string(),
            };
            return Drawn::Full(vec![Line::raw(text)], mark(Style::new().fg(theme.dim)));
        }
        Row::Thread { idx } => {
            return Drawn::Full(
                thread_lines(ctx, *idx, at_cursor, !doc.at_top(i), width as usize),
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
                    selected,
                    theme,
                    ctx.wrap,
                );
                return Drawn::Full(lines, style);
            }
            let half = width / 2;
            return Drawn::Split([
                code_lines(
                    left.as_ref(),
                    colors(left, left_rev, old_path),
                    half as usize,
                    selected,
                    theme,
                    ctx.wrap,
                ),
                code_lines(
                    right.as_ref(),
                    colors(right, right_rev, new_path),
                    (width - half) as usize,
                    selected,
                    theme,
                    ctx.wrap,
                ),
            ]);
        }
    };
    Drawn::Full(vec![Line::raw(text)], mark(header))
}

/// One side of a code row, wrapped by grapheme to `width`.
fn code_lines(
    side: Option<&Side>,
    colors: Option<&Colors>,
    width: usize,
    selected: bool,
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
    if selected {
        gutter = gutter.fg(theme.on_accent).bg(theme.accent);
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

/// A thread as a card: header, then each message wrapped to `width`;
/// resolved threads collapse to the header. `inline` threads sit under what they
/// anchor to, so their header leaves out the path.
fn thread_lines(
    ctx: &Ctx,
    idx: usize,
    at_cursor: bool,
    inline: bool,
    width: usize,
) -> Vec<Line<'static>> {
    let theme = ctx.theme;
    let outdated = ctx.outdated[idx];
    let t: &Thread = &ctx.session.threads[idx];
    let open = t.status == Status::Open;
    let waiting_on_user = t.waiting_on_user();
    let line = |spans: Vec<Span<'static>>| {
        Line::from([vec![Span::raw(" ".repeat(INDENT))], spans].concat())
    };
    let inner = width.saturating_sub(INDENT);
    let state = match (open, waiting_on_user) {
        (false, _) => "resolved",
        (true, true) => "waiting on you",
        (true, false) => "waiting on agent",
    };
    let location = ctx.session.location(&t.anchor);
    let place = match (&t.anchor, inline) {
        (Anchor::Lines { file, .. }, true) => location.replacen(&format!("{file}:"), "L", 1),
        (Anchor::File { .. }, true) => "file".into(),
        _ => location,
    };
    let mut header = format!("{}  {place}  {state}", t.id);
    if outdated {
        header.push_str("  outdated");
    }
    let mut header_style = Style::new().add_modifier(Modifier::BOLD);
    if waiting_on_user {
        header_style = header_style.fg(theme.accent);
    }
    if !open {
        header_style = Style::new().fg(theme.dim);
        if let Some(m) = t.messages.first() {
            header.push_str(&format!(
                "  {}: {}",
                m.from,
                m.body.lines().next().unwrap_or("")
            ));
        }
    }
    if at_cursor {
        header_style = header_style.add_modifier(Modifier::REVERSED);
    }
    let mut lines = vec![line(vec![Span::styled(header, header_style)])];
    if !open {
        return lines;
    }
    if outdated && let Some(text) = &t.text {
        let dim = Style::new().fg(theme.dim);
        lines.push(line(vec![Span::styled("was:", dim)]));
        for l in text.lines() {
            lines.push(line(vec![Span::styled(format!("  {}", clean(l)), dim)]));
        }
    }
    for m in &t.messages {
        let who = Style::new().fg(if m.from == "user" {
            theme.teal
        } else {
            theme.mauve
        });
        let indent = m.from.width() + 2;
        let body_width = inner.saturating_sub(indent).max(10);
        let mut first = true;
        for l in m.body.lines() {
            for part in textwrap::wrap(&clean(l), body_width) {
                let prefix = if first {
                    format!("{}: ", m.from)
                } else {
                    " ".repeat(indent)
                };
                first = false;
                lines.push(line(vec![
                    Span::styled(prefix, who),
                    Span::raw(part.into_owned()),
                ]));
            }
        }
    }
    lines
}

fn clean(s: &str) -> String {
    s.replace('\t', TAB).replace('\r', "")
}
