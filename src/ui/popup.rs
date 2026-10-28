use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;
use crate::ui::modal;

/// A centered list of (label, text) rows with a cursor, for help and settings.
pub struct Popup {
    title: String,
    rows: Vec<(String, String)>,
    pub cursor: usize,
}

impl Popup {
    pub fn new(title: &str, rows: Vec<(String, String)>) -> Popup {
        Popup {
            title: title.to_string(),
            rows,
            cursor: 0,
        }
    }

    /// Moves the cursor for movement actions; others are ignored.
    pub fn act(&mut self, action: &str) {
        let last = self.rows.len().saturating_sub(1);
        self.cursor = match action {
            "down" => self.cursor + 1,
            "up" => self.cursor.saturating_sub(1),
            "half_page_down" => self.cursor + 10,
            "half_page_up" => self.cursor.saturating_sub(10),
            "top" => 0,
            "bottom" => last,
            _ => self.cursor,
        }
        .min(last);
    }

    pub fn render(&self, f: &mut Frame, area: Rect, theme: &Theme) {
        let label_width = self.rows.iter().map(|r| r.0.width()).max().unwrap_or(0);
        let text_width = self.rows.iter().map(|r| r.1.width()).max().unwrap_or(0);
        let width = ((label_width + text_width + 6) as u16).max(self.title.width() as u16 + 6);
        let height = self.rows.len() as u16 + 2;
        let inner = modal(f, area, width, height, &self.title, theme);
        let shown = inner.height as usize;
        let skip = (self.cursor + 1).saturating_sub(shown);
        let lines: Vec<Line> = self
            .rows
            .iter()
            .skip(skip)
            .take(shown)
            .map(|(label, text)| {
                let pad = " ".repeat(label_width - label.width());
                Line::from(vec![
                    Span::styled(format!(" {label}{pad}  "), Style::new().fg(theme.accent)),
                    Span::raw(format!("{text} ")),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
        // The cursor row reversed across the box, which may be wider than the rows.
        if let Some(row) = self.cursor.checked_sub(skip).filter(|&r| r < shown) {
            let rect = Rect {
                y: inner.y + row as u16,
                height: 1,
                ..inner
            };
            f.buffer_mut()
                .set_style(rect, Style::new().add_modifier(Modifier::REVERSED));
        }
    }
}
