use crate::ui::step;

/// A cursor over rows of any height, and where the screen is in them: it starts `off`
/// lines into row `top`.
#[derive(Default)]
pub struct Scroll {
    pub cursor: usize,
    top: usize,
    off: usize,
    /// Screen lines, as of the last `place`.
    height: usize,
    /// Lines of the cursor row below the screen, as of the last `place`.
    overflow: usize,
    /// The next `place` puts the cursor row mid-screen.
    center: bool,
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

    /// Moves the cursor over `len` rows for movement actions, or first scrolls a cursor row
    /// taller than the screen; false for other actions.
    pub fn act(&mut self, action: &str, len: usize) -> bool {
        let half = (self.height / 2).max(1);
        let scrolled = self.top == self.cursor && self.off > 0;
        match action {
            "down" if self.overflow > 0 => self.off += 1,
            "half_page_down" if self.overflow > 0 => self.off += half.min(self.overflow),
            "up" if scrolled => self.off -= 1,
            "half_page_up" if scrolled => self.off = self.off.saturating_sub(half),
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
        self.cursor = self.cursor.min(len.saturating_sub(1));
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
    fn clamps_to_fewer_rows() {
        let mut s = Scroll::at(9);
        s.place(3, 4, |_| 1, false);
        assert_eq!((s.cursor, s.top()), (3, (3, 0)));
    }
}
