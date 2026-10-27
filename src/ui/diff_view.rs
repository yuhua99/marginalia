use std::ops::RangeInclusive;

use anyhow::Result;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{Ctx, Outcome};
use crate::doc::{Row, Side};
use crate::repo::Kind;
use crate::session::{Status, Thread};
use crate::ui::input::Input;

/// Side-by-side diff with a row cursor.
#[derive(Default)]
pub struct DiffView {
    cursor: usize,
    top: usize,
    height: usize,
    /// Start of a `v` selection.
    mark: Option<usize>,
}

impl DiffView {
    fn selection(&self) -> RangeInclusive<usize> {
        let mark = self.mark.unwrap_or(self.cursor);
        mark.min(self.cursor)..=mark.max(self.cursor)
    }

    fn thread_at_cursor(&self, ctx: &Ctx) -> Option<usize> {
        match ctx.doc.window(self.cursor, 1).first() {
            Some(Row::Thread { idx }) => Some(*idx),
            _ => None,
        }
    }
}

impl DiffView {
    pub fn jump(&mut self, row: usize) {
        self.cursor = row;
        self.mark = None;
    }

    pub fn act(&mut self, action: &str, ctx: &mut Ctx) -> Result<Outcome> {
        let last = ctx.doc.len() - 1;
        let half = (self.height / 2).max(1);
        match action {
            "down" => self.cursor = (self.cursor + 1).min(last),
            "up" => self.cursor = self.cursor.saturating_sub(1),
            "half_page_down" => self.cursor = (self.cursor + half).min(last),
            "half_page_up" => self.cursor = self.cursor.saturating_sub(half),
            "top" => self.cursor = 0,
            "bottom" => self.cursor = last,
            "next_waiting" | "prev_waiting" => {
                if let Some(i) = ctx.doc.next_waiting(
                    self.cursor,
                    action == "next_waiting",
                    &ctx.session.threads,
                ) {
                    self.cursor = i;
                }
            }
            "next_file" | "prev_file" => {
                if let Some(i) = ctx.doc.next_file(self.cursor, action == "next_file") {
                    self.cursor = i;
                }
            }
            "open" => {
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
            "comment" | "reply" => {
                if let Some(idx) = self.thread_at_cursor(ctx) {
                    return Ok(Outcome::Push(Box::new(Input::reply(idx, &ctx.session))));
                }
                if action == "comment" {
                    let anchor = ctx.doc.anchor_at(self.selection());
                    self.mark = None;
                    return Ok(Outcome::Push(Box::new(Input::comment(
                        anchor,
                        &ctx.session,
                    ))));
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
        let doc = &ctx.doc;
        self.cursor = self.cursor.min(doc.len() - 1);
        self.height = area.height as usize;
        let height = |i: usize| match &doc.window(i, 1)[0] {
            Row::Thread { idx } => thread_lines(ctx, *idx, false).len(),
            _ => 1,
        };
        // Scroll so the whole cursor row fits, if it can.
        if self.cursor < self.top {
            self.top = self.cursor;
        }
        let mut used: usize = (self.top..=self.cursor).map(height).sum();
        while used > self.height && self.top < self.cursor {
            used -= height(self.top);
            self.top += 1;
        }

        let selection = self.selection();
        let half = area.width / 2;
        let mut y = area.y;
        for (i, row) in doc.window(self.top, self.height).iter().enumerate() {
            if y >= area.bottom() {
                break;
            }
            let i = self.top + i;
            let at_cursor = i == self.cursor;
            let full = Rect {
                y,
                height: 1,
                ..area
            };
            let mark = |style: Style| {
                if at_cursor {
                    style.add_modifier(Modifier::REVERSED)
                } else {
                    style
                }
            };
            match row {
                Row::Session => {
                    let style = mark(
                        Style::new()
                            .bg(Color::Rgb(40, 40, 70))
                            .add_modifier(Modifier::BOLD),
                    );
                    f.render_widget(Paragraph::new("◆ session").style(style), full);
                }
                Row::File {
                    status, path, note, ..
                } => {
                    let mut text = format!("{status} {path}");
                    if let Some(note) = note {
                        text.push_str(&format!("  ({note})"));
                    }
                    let style = mark(
                        Style::new()
                            .bg(Color::Rgb(40, 40, 70))
                            .add_modifier(Modifier::BOLD),
                    );
                    f.render_widget(Paragraph::new(text).style(style), full);
                }
                Row::Fold { hidden, .. } => {
                    let style = mark(Style::new().fg(Color::DarkGray));
                    let text = match hidden {
                        Some(n) => format!("  ··· {n} unchanged lines ···"),
                        None => "  ··· more lines ···".to_string(),
                    };
                    f.render_widget(Paragraph::new(text).style(style), full);
                }
                Row::Code { left, right, .. } => {
                    let selected = selection.contains(&i);
                    f.render_widget(
                        side(left.as_ref(), selected),
                        Rect {
                            width: half,
                            ..full
                        },
                    );
                    let right_area = Rect {
                        x: area.x + half,
                        width: area.width - half,
                        ..full
                    };
                    f.render_widget(side(right.as_ref(), selected), right_area);
                }
                Row::Thread { idx } => {
                    let lines = thread_lines(ctx, *idx, at_cursor);
                    let h = (lines.len() as u16).min(area.bottom() - y);
                    f.render_widget(Paragraph::new(lines), Rect { height: h, ..full });
                    y += h;
                    continue;
                }
            }
            y += 1;
        }
    }
}

/// A thread: header, then each message; resolved threads collapse to the header.
fn thread_lines(ctx: &Ctx, idx: usize, at_cursor: bool) -> Vec<Line<'static>> {
    let outdated = ctx.outdated[idx];
    let t: &Thread = &ctx.session.threads[idx];
    let open = t.status == Status::Open;
    let waiting_on_user = open && t.waiting_on() == "user";
    let bar_color = if waiting_on_user {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    let bar = || Span::styled("┃ ", Style::new().fg(bar_color));
    let state = match (open, waiting_on_user) {
        (false, _) => "resolved",
        (true, true) => "waiting on you",
        (true, false) => "waiting on agent",
    };
    let mut header = format!("{} · {} · {state}", t.id, ctx.session.location(&t.anchor));
    if outdated {
        header.push_str(" · outdated");
    }
    let mut header_style = Style::new().add_modifier(Modifier::BOLD);
    if !open {
        header_style = Style::new().fg(Color::DarkGray);
        if let Some(m) = t.messages.first() {
            header.push_str(&format!(
                " · {}: {}",
                m.from,
                m.body.lines().next().unwrap_or("")
            ));
        }
    }
    if at_cursor {
        header_style = header_style.add_modifier(Modifier::REVERSED);
    }
    let mut lines = vec![Line::from(vec![bar(), Span::styled(header, header_style)])];
    if !open {
        return lines;
    }
    if outdated && let Some(text) = &t.text {
        let dim = Style::new().fg(Color::DarkGray);
        lines.push(Line::from(vec![bar(), Span::styled("was:", dim)]));
        for l in text.lines() {
            lines.push(Line::from(vec![
                bar(),
                Span::styled(format!("  {}", clean(l)), dim),
            ]));
        }
    }
    for m in &t.messages {
        let who = Style::new().fg(if m.from == "user" {
            Color::Cyan
        } else {
            Color::Magenta
        });
        for (i, l) in m.body.lines().enumerate() {
            let prefix = if i == 0 {
                format!("{}: ", m.from)
            } else {
                " ".repeat(m.from.len() + 2)
            };
            lines.push(Line::from(vec![
                bar(),
                Span::styled(prefix, who),
                Span::raw(clean(l)),
            ]));
        }
    }
    lines
}

fn side(side: Option<&Side>, selected: bool) -> Paragraph<'static> {
    let Some(s) = side else {
        return Paragraph::new("").style(Style::new().bg(Color::Rgb(30, 30, 30)));
    };
    let (sign, bg) = match s.kind {
        Kind::Ctx => (' ', Color::Reset),
        Kind::Del => ('-', Color::Rgb(70, 25, 25)),
        Kind::Add => ('+', Color::Rgb(25, 60, 25)),
    };
    let mut gutter = Style::new().fg(Color::DarkGray);
    if selected {
        gutter = gutter.fg(Color::Black).bg(Color::Yellow);
    }
    Paragraph::new(Line::from(vec![
        Span::styled(format!("{:>4} ", s.lineno), gutter),
        Span::raw(format!("{sign}{}", clean(&s.text))),
    ]))
    .style(Style::new().bg(bg))
}

fn clean(s: &str) -> String {
    s.replace('\t', "    ").replace('\r', "")
}
