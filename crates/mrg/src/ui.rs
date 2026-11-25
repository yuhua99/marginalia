pub mod commit_row;
pub mod confirm;
pub mod dialog;
pub mod diff_view;
pub mod file_view;
pub mod help;
pub mod input;
pub mod picker;
pub mod popup;
pub mod query;
pub mod review;
pub mod scroll;
pub mod settings;
pub mod sidebar;

use anyhow::Result;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear};

use crate::app::{Mouse, Outcome};
use crate::theme::Theme;

pub fn boxed<'a>(title: impl Into<Line<'a>>, focused: bool, theme: &Theme) -> Block<'a> {
    let color = if focused { theme.accent } else { theme.dim };
    Block::bordered()
        .title(title)
        .border_style(Style::new().fg(color))
}

pub fn clear(f: &mut Frame, area: Rect, theme: &Theme) {
    f.render_widget(Clear, area);
    f.buffer_mut()
        .set_style(area, Style::new().fg(theme.text).bg(theme.bg));
}

const MODAL_MIN: (u16, u16) = (40, 6);

pub fn modal(
    f: &mut Frame,
    area: Rect,
    width: u16,
    height: u16,
    title: &str,
    theme: &Theme,
) -> Rect {
    let width = width.max(MODAL_MIN.0).min(area.width);
    let height = height.max(MODAL_MIN.1).min(area.height);
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let block = boxed(format!(" {title} "), true, theme);
    clear(f, rect, theme);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    inner
}

pub fn cursor_bar(f: &mut Frame, area: Rect, y: u16, theme: &Theme) {
    bar(
        f,
        area,
        y,
        Style::new().bg(theme.accent).fg(theme.on_accent),
    );
}

pub fn checked_bar(f: &mut Frame, area: Rect, y: u16, theme: &Theme) {
    bar(f, area, y, Style::new().bg(theme.surface));
}

fn bar(f: &mut Frame, area: Rect, y: u16, style: Style) {
    let rect = Rect {
        y,
        height: 1,
        ..area
    };
    f.buffer_mut().set_style(rect, style);
}

#[derive(Clone, Copy, Default)]
pub struct Mark {
    pub style: Style,
    pub hover: bool,
}

impl Mark {
    pub fn new(theme: &Theme, on: bool, hover: bool) -> Mark {
        let style = if on {
            Style::new().bg(theme.accent).fg(theme.on_accent)
        } else {
            Style::new()
        };
        Mark { style, hover }
    }

    pub fn lit(self, span: Span<'_>) -> Span<'_> {
        let span = span.patch_style(self.style);
        if self.hover { span.underlined() } else { span }
    }
}

pub fn status_style(status: char, theme: &Theme) -> Style {
    let color = match status {
        'A' => theme.green,
        'D' => theme.red,
        'M' => theme.yellow,
        'R' => theme.blue,
        'T' => theme.mauve,
        _ => return Style::new(),
    };
    Style::new().fg(color)
}

pub fn step(cursor: usize, action: &str, len: usize, height: usize) -> Option<usize> {
    let half = (height / 2).max(1);
    let last = len.saturating_sub(1);
    let to = match action {
        "down" => cursor + 1,
        "up" => cursor.saturating_sub(1),
        "half_page_down" => cursor + half,
        "half_page_up" => cursor.saturating_sub(half),
        "page_down" => cursor + height.max(1),
        "page_up" => cursor.saturating_sub(height.max(1)),
        "top" => 0,
        "bottom" => last,
        _ => return None,
    };
    Some(to.min(last))
}

pub fn row_at(area: Rect, top: usize, len: usize, y: u16) -> Option<usize> {
    let line = y.checked_sub(area.y + 1).filter(|l| l + 2 < area.height)?;
    let row = top + line as usize;
    (row < len).then_some(row)
}

pub trait Panes<C> {
    fn panes(&self) -> &[Rect];
    fn focus(&mut self, pane: usize, ctx: &mut C);
    fn busy(&self) -> bool;
    fn act(&mut self, action: &str, ctx: &mut C) -> Result<Outcome<C>>;
    fn scroll_focused(&mut self, down: bool, ctx: &mut C);
    fn press(&mut self, pane: usize, y: u16, ctx: &mut C) -> Result<Outcome<C>>;
    fn drag(&mut self, pane: usize, y: u16, ctx: &mut C);
    fn release(&mut self, pane: usize, ctx: &mut C) -> Result<Outcome<C>>;

    fn mouse_at(&mut self, m: Mouse, at: Position, ctx: &mut C) -> Result<Outcome<C>> {
        let on = match m {
            Mouse::Drag { from } | Mouse::Release { from } => from,
            Mouse::Wheel { .. } | Mouse::Press => at,
        };
        let Some(pane) = self.pane_at(on) else {
            return Ok(Outcome::Unchanged);
        };
        match m {
            Mouse::Wheel { down } => {
                self.focus(pane, ctx);
                self.scroll_focused(down, ctx);
            }
            Mouse::Press => {
                self.focus(pane, ctx);
                return self.press(pane, at.y, ctx);
            }
            Mouse::Drag { .. } => self.drag(pane, at.y, ctx),
            Mouse::Release { .. } => return self.release(pane, ctx),
        }
        Ok(Outcome::Consumed)
    }

    fn pane_at(&self, at: Position) -> Option<usize> {
        if self.busy() {
            return None;
        }
        self.panes().iter().position(|r| r.contains(at))
    }
}

pub fn check(checked: Option<(usize, usize)>, i: usize) -> Option<(usize, usize)> {
    match checked {
        None => Some((i, i)),
        Some((first, last)) if (first..=last).contains(&i) => None,
        Some((first, last)) => Some((first.min(i), last.max(i))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps() {
        assert_eq!(step(0, "half_page_down", 20, 10), Some(5));
        assert_eq!(step(0, "half_page_down", 3, 10), Some(2));
        assert_eq!(step(7, "half_page_up", 20, 10), Some(2));
        assert_eq!(step(0, "half_page_down", 5, 1), Some(1));
        assert_eq!(step(0, "page_down", 30, 10), Some(10));
        assert_eq!(step(12, "page_up", 30, 10), Some(2));
        assert_eq!(step(0, "page_down", 5, 0), Some(1));
        assert_eq!(step(0, "bottom", 0, 10), Some(0));
        assert_eq!(step(2, "open", 5, 10), None);
    }

    #[test]
    fn checks() {
        assert_eq!(check(None, 2), Some((2, 2)));
        assert_eq!(check(Some((2, 2)), 4), Some((2, 4)));
        assert_eq!(check(Some((2, 4)), 0), Some((0, 4)));
        assert_eq!(check(Some((2, 4)), 3), None);
    }
}
