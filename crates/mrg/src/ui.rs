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
    bar(
        f,
        area,
        y,
        Style::new().bg(theme.accent).fg(theme.on_accent),
    );
}

/// The surface background on row `y` of `area`, under a checked row.
pub fn checked_bar(f: &mut Frame, area: Rect, y: u16, theme: &Theme) {
    bar(f, area, y, Style::new().bg(theme.surface));
}

/// `style` across row `y` of `area`.
fn bar(f: &mut Frame, area: Rect, y: u16, style: Style) {
    let rect = Rect {
        y,
        height: 1,
        ..area
    };
    f.buffer_mut().set_style(rect, style);
}

/// `span`, underlined when `on`: the text a click acts on, under the pointer.
pub fn lit(span: Span<'_>, on: bool) -> Span<'_> {
    if on { span.underlined() } else { span }
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

/// The row at screen line `y` of a list of `len` rows boxed over `area`, shown from `top`;
/// None on the borders and below the last row.
pub fn row_at(area: Rect, top: usize, len: usize, y: u16) -> Option<usize> {
    let line = y.checked_sub(area.y + 1).filter(|l| l + 2 < area.height)?;
    let row = top + line as usize;
    (row < len).then_some(row)
}

/// A screen of boxes, one focused, that the mouse focuses and moves in: the picker's
/// and the review's.
pub trait Panes<C> {
    /// The boxes, as of the last render.
    fn panes(&self) -> &[Rect];
    fn focus(&mut self, pane: usize, ctx: &mut C);
    /// Typing, which takes all keys and leaves the mouse alone.
    fn busy(&self) -> bool;
    /// The key action `action` in the focused box.
    fn act(&mut self, action: &str, ctx: &mut C) -> Result<Outcome<C>>;
    /// Scrolls the focused box a wheel turn down or up, its cursor left.
    fn scroll_focused(&mut self, down: bool, ctx: &mut C);
    /// A press on screen line `y` of the focused `pane`: moves its cursor to the row
    /// there, and acts on it or starts a drag from it.
    fn press(&mut self, pane: usize, y: u16, ctx: &mut C) -> Result<Outcome<C>>;
    /// The button held over screen line `y` after a press in `pane`.
    fn drag(&mut self, pane: usize, y: u16, ctx: &mut C);
    /// The button let go after a press in `pane`.
    fn release(&mut self, pane: usize, ctx: &mut C) -> Result<Outcome<C>>;

    /// The wheel focuses the box under it and scrolls it; a press focuses the box and
    /// presses in it; a drag and the letting go go to the box pressed in.
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

    /// The box under `at`, unless typing.
    fn pane_at(&self, at: Position) -> Option<usize> {
        if self.busy() {
            return None;
        }
        self.panes().iter().position(|r| r.contains(at))
    }
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
        assert_eq!(step(0, "half_page_down", 20, 10), Some(5));
        assert_eq!(step(0, "half_page_down", 3, 10), Some(2));
        assert_eq!(step(7, "half_page_up", 20, 10), Some(2));
        // At least one row on a tiny screen.
        assert_eq!(step(0, "half_page_down", 5, 1), Some(1));
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
