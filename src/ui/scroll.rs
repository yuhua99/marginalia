use crate::ui::step;

/// A cursor over rows of any height, and where the screen is in them: it starts `off`
/// lines into row `top`, which only happens for a cursor row taller than the screen.
#[derive(Default)]
pub struct Scroll {
    pub cursor: usize,
    top: usize,
    off: usize,
    /// Screen lines, as of the last `place`.
    height: usize,
    /// The cursor row goes on below the screen, as of the last `place`.
    overflow: bool,
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

    /// Moves the cursor to `row` at the top of the screen.
    pub fn jump(&mut self, row: usize) {
        *self = Scroll {
            height: self.height,
            ..Scroll::at(row)
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
            "down" if self.overflow => self.off += 1,
            "half_page_down" if self.overflow => self.off += half,
            "up" if scrolled => self.off -= 1,
            "half_page_up" if scrolled => self.off = self.off.saturating_sub(half),
            _ => match step(self.cursor, action, len, self.height) {
                Some(cursor) => self.cursor = cursor,
                None => return false,
            },
        }
        true
    }

    /// Places a screen of `h` lines over `len` rows of `height(i)` lines so the cursor row
    /// shows: whole if it fits, else from its start, or from its end with `from_end`.
    pub fn place(&mut self, h: usize, len: usize, height: impl Fn(usize) -> usize, from_end: bool) {
        self.height = h;
        self.cursor = self.cursor.min(len.saturating_sub(1));
        let cursor = self.cursor;
        if cursor < self.top {
            self.top = cursor;
            self.off = 0;
        }
        let mut bottom = 0;
        for i in self.top..=cursor {
            bottom += height(i);
            if bottom > h + self.off {
                break;
            }
        }
        if bottom > h + self.off && self.top != cursor {
            self.top = cursor;
            self.off = 0;
            let mut used = height(cursor);
            while self.top > 0 && used + height(self.top - 1) <= h {
                self.top -= 1;
                used += height(self.top);
            }
        }
        let top_height = height(self.top);
        self.off = if self.top == cursor {
            self.off.min(top_height.saturating_sub(h))
        } else {
            self.off.min(top_height - 1)
        };
        if from_end && self.top == cursor {
            self.off = top_height.saturating_sub(h);
        }
        self.overflow = self.top == cursor && top_height - self.off > h;
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
    fn scrolls_a_tall_row_first() {
        let rows = [1, 1, 5, 1];
        let mut s = Scroll::default();
        run(&mut s, &rows, &["down", "down"]);
        assert_eq!((s.cursor, s.top()), (2, (2, 0)));
        run(&mut s, &rows, &["down", "down"]);
        assert_eq!((s.cursor, s.top()), (2, (2, 2)));
        // Its end is on screen: down moves on.
        run(&mut s, &rows, &["down"]);
        assert_eq!(s.cursor, 3);
        // Back on it, from its start.
        run(&mut s, &rows, &["up"]);
        assert_eq!((s.cursor, s.top()), (2, (2, 0)));
    }

    #[test]
    fn shows_a_tall_row_from_its_end() {
        let mut s = Scroll::at(1);
        s.place(3, 2, |i| [1, 5][i], true);
        assert_eq!(s.top(), (1, 2));
    }

    #[test]
    fn clamps_to_fewer_rows() {
        let mut s = Scroll::at(9);
        s.place(3, 4, |_| 1, false);
        assert_eq!((s.cursor, s.top()), (3, (3, 0)));
    }
}
