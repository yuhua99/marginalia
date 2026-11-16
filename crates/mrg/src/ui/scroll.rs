use crate::ui::step;

/// Lines a wheel turn scrolls.
const WHEEL: isize = 3;

/// A cursor over rows of any height, and where the screen is in them: it starts `off`
/// lines into row `top`.
#[derive(Default)]
pub struct Scroll {
    /// Set through `go`, which ends a wheel turn's hold on the screen.
    cursor: usize,
    top: usize,
    off: usize,
    /// Screen lines, as of the last `place`.
    height: usize,
    /// Lines of the cursor row below the screen, as of the last `place`.
    overflow: usize,
    /// Rows above and below the cursor that fit on a screen with it, as of the last
    /// `place`: a page up and down.
    page: (usize, usize),
    /// Those within half a screen of lines, no more than a page: a half page up and down.
    half: (usize, usize),
    /// The next `place` puts the cursor row mid-screen.
    center: bool,
    /// The next `place` shows a cursor row taller than the screen from its end.
    from_end: bool,
    /// The cursor as of the last wheel turn: the screen stays where the wheel left it
    /// until the cursor moves or acts.
    wheeled: Option<usize>,
    /// Lines the wheel turned since the last `place`, down positive.
    wheel: isize,
    /// The last row on screen, when the last `place` rolled the screen for the wheel.
    last: Option<usize>,
}

impl Scroll {
    /// The cursor on `row`, at the top of the screen.
    pub fn at(row: usize) -> Scroll {
        Scroll {
            cursor: row,
            top: row,
            ..Scroll::default()
        }
    }

    /// The cursor on `row`, mid-screen once placed.
    pub fn centered(row: usize) -> Scroll {
        Scroll {
            center: true,
            ..Scroll::at(row)
        }
    }

    /// Moves the cursor to `row` at the top of the screen.
    pub fn jump(&mut self, row: usize) {
        *self = Scroll {
            height: self.height,
            ..Scroll::at(row)
        };
    }

    /// Moves the cursor to `row`, mid-screen.
    pub fn center(&mut self, row: usize) {
        *self = Scroll {
            height: self.height,
            ..Scroll::centered(row)
        };
    }

    /// The first row on screen and the lines of it scrolled past.
    pub fn top(&self) -> (usize, usize) {
        (self.top, self.off)
    }

    /// The cursor row; after a wheel turn, maybe off screen until `settle`.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The cursor row while it is on screen, as of the last `place`; None once the wheel
    /// has turned the screen off it.
    pub fn shown(&self) -> Option<usize> {
        match (self.wheeled, self.last) {
            (Some(_), Some(last)) if !(self.top..=last).contains(&self.cursor) => None,
            _ => Some(self.cursor),
        }
    }

    /// Keeps the cursor within `len` rows.
    pub fn clamp(&mut self, len: usize) {
        self.cursor = self.cursor.min(len.saturating_sub(1));
    }

    /// Moves the cursor to `row`, the screen following it.
    pub fn go(&mut self, row: usize) {
        self.cursor = row;
        self.wheeled = None;
    }

    /// Scrolls the screen a wheel turn down or up, the cursor left where it is.
    pub fn wheel(&mut self, down: bool) {
        self.wheeled = Some(self.cursor);
        self.wheel += if down { WHEEL } else { -WHEEL };
    }

    /// After a wheel turn, brings a cursor the screen left onto its nearest row on screen,
    /// and the screen back to following it; the cursor an action acts on. Actions read
    /// the cursor through this or `act`.
    pub fn settle(&mut self) -> usize {
        if self.wheeled.take().is_some() {
            // Turns not placed yet never showed: the cursor acts on the screen drawn.
            self.wheel = 0;
            if let Some(last) = self.last {
                self.cursor = self.cursor.clamp(self.top, last);
            }
        }
        self.cursor
    }

    /// Moves the cursor over `len` rows for movement actions, or first scrolls a cursor row
    /// taller than the screen; false for other actions. After a wheel turn the cursor moves
    /// from the screen, as `settle` brings it there.
    pub fn act(&mut self, action: &str, len: usize) -> bool {
        let half = (self.height / 2).max(1);
        let last = len.saturating_sub(1);
        let wheeled = self.wheeled.is_some();
        self.settle();
        let scrolled = !wheeled && self.top == self.cursor && self.off > 0;
        match action {
            "down" if self.overflow > 0 => self.off += 1,
            "half_page_down" if self.overflow > 0 => self.off += half.min(self.overflow),
            "page_down" if self.overflow > 0 => self.off += self.height.min(self.overflow),
            "up" if scrolled => self.off -= 1,
            "half_page_up" if scrolled => self.off = self.off.saturating_sub(half),
            "page_up" if scrolled => self.off = self.off.saturating_sub(self.height),
            "page_down" => self.cursor = (self.cursor + self.page.1.max(1)).min(last),
            "page_up" => self.cursor = self.cursor.saturating_sub(self.page.0.max(1)),
            "half_page_down" => self.cursor = (self.cursor + self.half.1.max(1)).min(last),
            "half_page_up" => self.cursor = self.cursor.saturating_sub(self.half.0.max(1)),
            "view_center" => self.center = true,
            "view_top" => (self.top, self.off) = (self.cursor, 0),
            // From the top, the cursor row is below the screen: `place` brings it to the bottom.
            "view_bottom" => {
                (self.top, self.off) = (0, 0);
                self.from_end = true;
            }
            _ => match step(self.cursor, action, len, self.height) {
                Some(cursor) => self.cursor = cursor,
                None => return false,
            },
        }
        true
    }

    /// Places a screen of `h` lines over `len` rows of `height(i)` lines, scrolling as little
    /// as shows the cursor row: whole if it fits, else a line of it, or its end with `from_end`.
    pub fn place(&mut self, h: usize, len: usize, height: impl Fn(usize) -> usize, from_end: bool) {
        self.height = h;
        self.clamp(len);
        let lines = std::mem::take(&mut self.wheel);
        if self.wheeled == Some(self.cursor) && self.top < len {
            self.roll(lines, h, len, &height);
            self.overflow = 0;
            // Pages from where `settle` brings the cursor.
            let at = self
                .last
                .map_or(self.cursor, |l| self.cursor.clamp(self.top, l))
                .min(len - 1);
            self.reach(at, h, len, &height);
            return;
        }
        self.wheeled = None;
        self.last = None;
        let from_end = std::mem::take(&mut self.from_end) || from_end;
        let cursor = self.cursor;
        let hc = height(cursor);
        let tall = hc > h;
        let need = if tall { 1 } else { hc };
        if self.center {
            // Half the lines left over above the cursor row.
            self.center = false;
            let above = h.saturating_sub(hc) / 2;
            let mut used = 0;
            self.top = cursor;
            while self.top > 0 && used < above {
                self.top -= 1;
                used += height(self.top);
            }
            self.off = used.saturating_sub(above);
        }
        if cursor < self.top {
            // Above the screen: down to its end.
            self.top = cursor;
            self.off = hc - need;
        }
        // Scrolled on into the rows under the top one.
        while self.top < cursor && self.off >= height(self.top) {
            self.off -= height(self.top);
            self.top += 1;
        }
        if self.top == cursor && !tall {
            self.off = 0;
        }
        self.off = self.off.min(height(self.top) - 1);
        let mut bottom = need;
        for i in self.top..cursor {
            bottom += height(i);
            if bottom > h + self.off {
                break;
            }
        }
        if bottom > h + self.off {
            // Below the screen: up to the bottom, under what fits of the rows above it.
            self.top = cursor;
            let mut used = need;
            while self.top > 0 && used < h {
                self.top -= 1;
                used += height(self.top);
            }
            self.off = used.saturating_sub(h);
        }
        if from_end && tall {
            self.top = cursor;
            self.off = hc - h;
        }
        let end = (self.top..cursor).map(&height).sum::<usize>() + hc;
        self.overflow = (end - self.off).saturating_sub(h);
        self.reach(cursor, h, len, &height);
    }

    /// Sets how many rows a page and a half page are above and below `row`, on a screen of
    /// `h` lines over `len` rows of `height(i)` lines.
    fn reach(&mut self, row: usize, h: usize, len: usize, height: impl Fn(usize) -> usize) {
        // Rows above and below `row` of no more than `lines` lines together.
        let within = |lines: usize| {
            let (mut up, mut used) = (0, 0);
            while up < row && used + height(row - up - 1) <= lines {
                up += 1;
                used += height(row - up);
            }
            let (mut down, mut used) = (0, 0);
            while row + down + 1 < len && used + height(row + down + 1) <= lines {
                down += 1;
                used += height(row + down);
            }
            (up, down)
        };
        // A page keeps `row` on screen.
        let page = h.saturating_sub(height(row));
        self.page = within(page);
        self.half = within((h / 2).max(1).min(page));
    }

    /// Moves the screen of `h` lines `lines` down (up when negative) over `len` rows of
    /// `height(i)` lines, down no further than shows the last line at the bottom.
    fn roll(&mut self, lines: isize, h: usize, len: usize, height: impl Fn(usize) -> usize) {
        // Lines from the top of the screen on, up to one more than it shows.
        let rest = |s: &Scroll| (s.top..len).map(&height).take(h + 1).sum::<usize>() - s.off;
        let up = |s: &mut Scroll| {
            if s.off > 0 {
                s.off -= 1;
            } else if s.top > 0 {
                s.top -= 1;
                s.off = height(s.top) - 1;
            }
        };
        for _ in 0..lines.max(0) {
            if rest(self) > h {
                self.off += 1;
                if self.off == height(self.top) {
                    self.top += 1;
                    self.off = 0;
                }
            }
        }
        for _ in 0..(-lines).max(0) {
            up(self);
        }
        // Rows gone from under the screen: back up to fill it.
        while rest(self) < h && (self.top > 0 || self.off > 0) {
            up(self);
        }
        // Lines from the top of the top row to where each row starts.
        let (mut i, mut start, mut last) = (self.top, 0, self.top);
        while i < len && start < self.off + h {
            last = i;
            start += height(i);
            i += 1;
        }
        self.last = Some(last);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `s` after `actions`, placing a screen of 3 lines over `rows` after each.
    fn run(s: &mut Scroll, rows: &[usize], actions: &[&str]) {
        let height = |i: usize| rows[i];
        s.place(3, rows.len(), height, false);
        for a in actions {
            s.act(a, rows.len());
            s.place(3, rows.len(), height, false);
        }
    }

    #[test]
    fn keeps_the_cursor_on_screen() {
        let mut s = Scroll::default();
        run(&mut s, &[1; 6], &["down", "down", "down", "down"]);
        assert_eq!((s.cursor, s.top()), (4, (2, 0)));
        run(&mut s, &[1; 6], &["up", "up", "up"]);
        assert_eq!((s.cursor, s.top()), (1, (1, 0)));
        run(&mut s, &[1; 6], &["bottom"]);
        assert_eq!((s.cursor, s.top()), (5, (3, 0)));
    }

    #[test]
    fn scrolls_a_tall_row_by_lines() {
        let rows = [1, 1, 5, 1];
        let mut s = Scroll::default();
        run(&mut s, &rows, &["down", "down"]);
        assert_eq!((s.cursor, s.top()), (2, (0, 0)));
        run(&mut s, &rows, &["down", "down", "down", "down"]);
        assert_eq!((s.cursor, s.top()), (2, (2, 2)));
        // Its end is on screen: down moves on a line.
        run(&mut s, &rows, &["down"]);
        assert_eq!((s.cursor, s.top()), (3, (2, 3)));
        // Up the same way back.
        run(&mut s, &rows, &["up", "up"]);
        assert_eq!((s.cursor, s.top()), (2, (2, 2)));
    }

    #[test]
    fn pages_through_a_tall_row() {
        let rows = [1, 1, 5, 1];
        let mut s = Scroll::default();
        run(&mut s, &rows, &["down", "down", "page_down"]);
        assert_eq!((s.cursor, s.top()), (2, (2, 1)));
        run(&mut s, &rows, &["page_up"]);
        assert_eq!((s.cursor, s.top()), (2, (2, 0)));
    }

    #[test]
    fn pages_by_the_lines_of_rows() {
        let mut s = Scroll::default();
        let place = |s: &mut Scroll| s.place(4, 10, |_| 2, false);
        place(&mut s);
        // Two rows fill the screen: a page is one row on, not four.
        s.act("page_down", 10);
        place(&mut s);
        assert_eq!((s.cursor, s.top()), (1, (0, 0)));
        s.act("page_down", 10);
        place(&mut s);
        assert_eq!((s.cursor, s.top()), (2, (1, 0)));
        s.act("page_up", 10);
        place(&mut s);
        assert_eq!((s.cursor, s.top()), (1, (1, 0)));
    }

    #[test]
    fn half_pages_by_the_lines_of_rows() {
        let mut s = Scroll::default();
        let place = |s: &mut Scroll| s.place(8, 10, |_| 2, false);
        place(&mut s);
        // Half the screen is two rows of two lines, not four rows.
        s.act("half_page_down", 10);
        place(&mut s);
        assert_eq!(s.cursor, 2);
        s.act("half_page_up", 10);
        assert_eq!(s.cursor, 0);
        // No further than a page: one row fits with a row of three lines on four.
        let mut s = Scroll::default();
        s.place(4, 3, |i| [3, 1, 1][i], false);
        s.act("half_page_down", 3);
        assert_eq!(s.cursor, 1);
    }

    #[test]
    fn aligns_the_screen_to_the_cursor_row() {
        let rows = [1; 10];
        let mut s = Scroll::at(5);
        run(&mut s, &rows, &["view_bottom"]);
        assert_eq!((s.cursor, s.top()), (5, (3, 0)));
        run(&mut s, &rows, &["view_center"]);
        assert_eq!((s.cursor, s.top()), (5, (4, 0)));
        run(&mut s, &rows, &["view_top"]);
        assert_eq!((s.cursor, s.top()), (5, (5, 0)));
        // A row taller than the screen: its end at the bottom.
        let mut s = Scroll::at(2);
        run(&mut s, &[1, 1, 5], &["view_bottom"]);
        assert_eq!(s.top(), (2, 2));
    }

    #[test]
    fn pages_from_the_screen_after_the_wheel() {
        let rows = [1, 1, 1, 2, 2, 2, 1];
        let mut s = Scroll::default();
        run(&mut s, &rows, &[]);
        s.wheel(true);
        s.place(3, rows.len(), |i| rows[i], false);
        assert_eq!(s.top(), (3, 0));
        // From row 3, which no row under it fits with: one row on.
        run(&mut s, &rows, &["page_down"]);
        assert_eq!(s.cursor, 4);
    }

    #[test]
    fn shows_a_tall_row_from_its_end() {
        let mut s = Scroll::at(1);
        s.place(3, 2, |i| [1, 5][i], true);
        assert_eq!(s.top(), (1, 2));
    }

    #[test]
    fn centers_the_cursor_row() {
        let mut s = Scroll::centered(5);
        s.place(5, 10, |_| 1, false);
        assert_eq!(s.top(), (3, 0));
        // Only once.
        s.act("down", 10);
        s.place(5, 10, |_| 1, false);
        assert_eq!(s.top(), (3, 0));
    }

    #[test]
    fn wheels_without_the_cursor() {
        let rows = [1, 2, 1, 1, 1, 1, 1];
        let mut s = Scroll::default();
        run(&mut s, &rows, &[]);
        s.wheel(true);
        s.place(3, rows.len(), |i| rows[i], false);
        assert_eq!((s.cursor, s.top()), (0, (2, 0)));
        // No further than the last line at the bottom.
        s.wheel(true);
        s.place(3, rows.len(), |i| rows[i], false);
        assert_eq!((s.cursor, s.top()), (0, (4, 0)));
        s.wheel(false);
        s.place(3, rows.len(), |i| rows[i], false);
        assert_eq!((s.cursor, s.top()), (0, (1, 1)));
        // A move goes on from the top row on screen.
        run(&mut s, &rows, &["down"]);
        assert_eq!((s.cursor, s.top()), (2, (1, 1)));
    }

    #[test]
    fn moves_from_the_screen_after_the_wheel() {
        let rows = [1; 10];
        let mut s = Scroll::default();
        run(&mut s, &rows, &[]);
        s.wheel(true);
        s.wheel(true);
        s.place(3, rows.len(), |i| rows[i], false);
        assert_eq!((s.cursor, s.top()), (0, (6, 0)));
        // From the top row on screen.
        run(&mut s, &rows, &["down"]);
        assert_eq!((s.cursor, s.top()), (7, (6, 0)));
        s.wheel(false);
        s.place(3, rows.len(), |i| rows[i], false);
        assert_eq!((s.cursor, s.top()), (7, (3, 0)));
        // From the bottom row on screen.
        run(&mut s, &rows, &["up"]);
        assert_eq!((s.cursor, s.top()), (4, (3, 0)));
    }

    #[test]
    fn acts_on_the_screen_drawn_before_a_wheel_turn_is_placed() {
        let rows = [1; 10];
        let mut s = Scroll::default();
        run(&mut s, &rows, &["bottom"]);
        assert_eq!((s.cursor, s.top()), (9, (7, 0)));
        // The turn never showed: the cursor moves from where it is.
        s.wheel(false);
        s.act("up", rows.len());
        assert_eq!(s.cursor, 8);
        s.place(3, rows.len(), |i| rows[i], false);
        assert_eq!(s.top(), (7, 0));
    }

    #[test]
    fn shows_the_cursor_until_the_wheel_leaves_it() {
        let rows = [1; 10];
        let mut s = Scroll::at(4);
        s.place(5, rows.len(), |i| rows[i], false);
        assert_eq!(s.shown(), Some(4));
        // A turn up: still on screen.
        s.wheel(false);
        s.place(5, rows.len(), |i| rows[i], false);
        assert_eq!((s.top(), s.shown()), ((1, 0), Some(4)));
        // Two down: the screen is past it.
        s.wheel(true);
        s.wheel(true);
        s.place(5, rows.len(), |i| rows[i], false);
        assert_eq!((s.top(), s.shown()), ((5, 0), None));
        // A move brings it onto the screen.
        s.act("down", rows.len());
        assert_eq!(s.shown(), Some(6));
    }

    #[test]
    fn clamps_to_fewer_rows() {
        let mut s = Scroll::at(9);
        s.place(3, 4, |_| 1, false);
        assert_eq!((s.cursor, s.top()), (3, (3, 0)));
    }
}
