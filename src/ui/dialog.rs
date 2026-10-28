use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use ratatui_textarea::TextArea;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;
use crate::ui::modal;

/// A centered box asking `question`, with the `keys` that answer it.
pub fn confirm(f: &mut Frame, area: Rect, title: &str, question: &str, keys: &str, theme: &Theme) {
    let body = frame(f, area, title, question.width() as u16, keys, theme);
    f.render_widget(Paragraph::new(question), body);
}

/// A centered box around one line of `text`, `width` columns wide.
pub fn input(
    f: &mut Frame,
    area: Rect,
    title: &str,
    text: &TextArea,
    width: u16,
    keys: &str,
    theme: &Theme,
) {
    let body = frame(f, area, title, width, keys, theme);
    f.render_widget(text, body);
}

/// The box: a one-row body `width` wide, blank rows, then `keys`; returns the body area.
fn frame(f: &mut Frame, area: Rect, title: &str, width: u16, keys: &str, theme: &Theme) -> Rect {
    let width = width.max(keys.width() as u16) + 4;
    let inner = modal(f, area, width, 5, title, theme).inner(Margin::new(1, 0));
    let [body, _, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    f.render_widget(Paragraph::new(keys).style(Style::new().fg(theme.dim)), foot);
    body
}
