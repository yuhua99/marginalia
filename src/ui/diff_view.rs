use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::doc::{Doc, Row, Side};
use crate::repo::Kind;

/// Side-by-side diff with a row cursor.
#[derive(Default)]
pub struct DiffView {
    cursor: usize,
    top: usize,
    height: usize,
}

impl DiffView {
    pub fn act(&mut self, action: &str, doc: &Doc) {
        let last = doc.len().saturating_sub(1);
        let half = (self.height / 2).max(1);
        self.cursor = match action {
            "down" => self.cursor + 1,
            "up" => self.cursor.saturating_sub(1),
            "half_page_down" => self.cursor + half,
            "half_page_up" => self.cursor.saturating_sub(half),
            "top" => 0,
            "bottom" => last,
            _ => self.cursor,
        }
        .min(last);
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, doc: &Doc) {
        if doc.is_empty() {
            f.render_widget(Paragraph::new("no changes"), area);
            return;
        }
        self.height = area.height as usize;
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + self.height {
            self.top = self.cursor + 1 - self.height;
        }
        let half = area.width / 2;
        for (i, row) in doc.window(self.top, self.height).iter().enumerate() {
            let y = area.y + i as u16;
            let full = Rect {
                y,
                height: 1,
                ..area
            };
            let at_cursor = self.top + i == self.cursor;
            match row {
                Row::File { status, path, note } => {
                    let mut text = format!("{status} {path}");
                    if let Some(note) = note {
                        text.push_str(&format!("  ({note})"));
                    }
                    let mut style = Style::new()
                        .bg(Color::Rgb(40, 40, 70))
                        .add_modifier(Modifier::BOLD);
                    if at_cursor {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    f.render_widget(Paragraph::new(text).style(style), full);
                }
                Row::Fold { hidden } => {
                    let mut style = Style::new().fg(Color::DarkGray);
                    if at_cursor {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    f.render_widget(
                        Paragraph::new(format!("  ··· {hidden} unchanged lines ···")).style(style),
                        full,
                    );
                }
                Row::Code { left, right } => {
                    let left_area = Rect {
                        width: half,
                        ..full
                    };
                    let right_area = Rect {
                        x: area.x + half,
                        width: area.width - half,
                        ..full
                    };
                    f.render_widget(side(left.as_ref(), at_cursor), left_area);
                    f.render_widget(side(right.as_ref(), at_cursor), right_area);
                }
            }
        }
    }
}

fn side(side: Option<&Side>, at_cursor: bool) -> Paragraph<'static> {
    let Some(s) = side else {
        return Paragraph::new("").style(Style::new().bg(Color::Rgb(30, 30, 30)));
    };
    let (sign, bg) = match s.kind {
        Kind::Ctx => (' ', Color::Reset),
        Kind::Del => ('-', Color::Rgb(70, 25, 25)),
        Kind::Add => ('+', Color::Rgb(25, 60, 25)),
    };
    let mut gutter = Style::new().fg(Color::DarkGray);
    if at_cursor {
        gutter = gutter.fg(Color::Black).bg(Color::Yellow);
    }
    let text = s.text.replace('\t', "    ").replace('\r', "");
    Paragraph::new(Line::from(vec![
        Span::styled(format!("{:>4} ", s.lineno), gutter),
        Span::raw(format!("{sign}{text}")),
    ]))
    .style(Style::new().bg(bg))
}
