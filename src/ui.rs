pub mod commit_row;
pub mod delete_prompt;
pub mod diff_view;
pub mod file_view;
pub mod help;
pub mod input;
pub mod picker;
pub mod popup;
pub mod quit_prompt;
pub mod review;
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

/// Space on row `i` of a contiguous check: extends it to `i`, or clears it when `i` is in it.
pub fn check(checked: Option<(usize, usize)>, i: usize) -> Option<(usize, usize)> {
    match checked {
        None => Some((i, i)),
        Some((first, last)) if (first..=last).contains(&i) => None,
        Some((first, last)) => Some((first.min(i), last.max(i))),
    }
}
