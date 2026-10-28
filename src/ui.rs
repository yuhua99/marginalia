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

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear};

use crate::theme::Theme;

/// A bordered box; the focused one in the accent color.
pub fn boxed<'a>(title: impl Into<Line<'a>>, focused: bool, theme: &Theme) -> Block<'a> {
    let color = if focused { theme.accent } else { theme.dim };
    Block::bordered()
        .title(title)
        .border_style(Style::new().fg(color))
}

/// Blanks `area` to the theme's background and default text.
pub fn clear(f: &mut Frame, area: Rect, theme: &Theme) {
    f.render_widget(Clear, area);
    f.buffer_mut()
        .set_style(area, Style::new().fg(theme.text).bg(theme.bg));
}

/// Smallest width and height of a modal box, borders included.
const MODAL_MIN: (u16, u16) = (40, 6);

/// A box of `width` x `height` (at least `MODAL_MIN`, at most `area`) centered in `area`,
/// cleared and framed in the accent color; returns its inner area.
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

/// Marks row `y` of `area` as the cursor: an accent bar across it, over any colors.
pub fn cursor_bar(f: &mut Frame, area: Rect, y: u16, theme: &Theme) {
    let rect = Rect {
        y,
        height: 1,
        ..area
    };
    f.buffer_mut()
        .set_style(rect, Style::new().bg(theme.accent).fg(theme.on_accent));
}

/// A file's status letter: added green, deleted red, modified yellow, renamed blue,
/// type changed mauve; others, like `Doc::file`'s ' ', keep the surrounding style.
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

/// Where movement `action` takes a cursor over `len` rows on a screen of `height` rows,
/// a half page being half of it; None for other actions.
pub fn step(cursor: usize, action: &str, len: usize, height: usize) -> Option<usize> {
    let half = (height / 2).max(1);
    let last = len.saturating_sub(1);
    let to = match action {
        "down" => cursor + 1,
        "up" => cursor.saturating_sub(1),
        "half_page_down" => cursor + half,
        "half_page_up" => cursor.saturating_sub(half),
        "top" => 0,
        "bottom" => last,
        _ => return None,
    };
    Some(to.min(last))
}

/// Space on row `i` of a contiguous check: extends it to `i`, or clears it when `i` is in it.
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
        assert_eq!(step(3, "down", 5, 10), Some(4));
        assert_eq!(step(4, "down", 5, 10), Some(4));
        assert_eq!(step(0, "up", 5, 10), Some(0));
        assert_eq!(step(0, "half_page_down", 20, 10), Some(5));
        assert_eq!(step(0, "half_page_down", 3, 10), Some(2));
        assert_eq!(step(7, "half_page_up", 20, 10), Some(2));
        // At least one row on a tiny screen.
        assert_eq!(step(0, "half_page_down", 5, 1), Some(1));
        assert_eq!(step(2, "top", 5, 10), Some(0));
        assert_eq!(step(2, "bottom", 5, 10), Some(4));
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
