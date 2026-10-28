use ratatui::style::Style;
use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

use crate::repo::Commit;
use crate::theme::Theme;

/// Columns kept for a subject before the author and date are left out.
const MIN_SUBJECT: usize = 20;

/// The widest author column.
const MAX_AUTHOR: usize = 16;

/// `☒ ` or `☐ ` before a checkable row.
pub fn mark(checked: bool, theme: &Theme) -> Span<'static> {
    let mark = if checked { "☒ " } else { "☐ " };
    Span::styled(mark, Style::new().fg(theme.accent))
}

/// Widths of the author and date columns, shared by the rows of a commit list.
pub struct Columns {
    author: usize,
    when: usize,
}

impl Columns {
    pub fn new<'a>(commits: impl IntoIterator<Item = &'a Commit>) -> Columns {
        let none = Columns { author: 0, when: 0 };
        commits.into_iter().fold(none, |cols, c| Columns {
            author: cols.author.max(c.author.width()).min(MAX_AUTHOR),
            when: cols.when.max(c.when.width()),
        })
    }

    /// `sha subject  author  date` in `width` columns, the author and date
    /// right-aligned.
    pub fn row<'a>(&self, commit: &'a Commit, width: usize, theme: &Theme) -> Vec<Span<'a>> {
        let author = fit(&commit.author, self.author);
        let mut meta = format!(
            "{author}{}  {}{}",
            " ".repeat(self.author - author.width()),
            " ".repeat(self.when - commit.when.width()),
            commit.when
        );
        // After the sha, the spaces around the subject.
        let width = width.saturating_sub(8 + 2);
        // Too narrow: the subject goes first.
        if width < meta.width() + MIN_SUBJECT {
            meta.clear();
        }
        let room = width - meta.width();
        let subject = fit(&commit.subject, room);
        let pad = " ".repeat(room - subject.width() + 2);
        vec![
            Span::styled(&commit.sha[..7], Style::new().fg(theme.dim)),
            Span::raw(format!(" {subject}")),
            Span::raw(pad),
            Span::styled(meta, Style::new().fg(theme.dim)),
        ]
    }
}

/// `s` cut to `width` columns, as `str::width` counts them.
pub fn fit(s: &str, width: usize) -> String {
    let end = s
        .char_indices()
        .map(|(i, c)| i + c.len_utf8())
        .take_while(|&end| s[..end].width() <= width)
        .last()
        .unwrap_or(0);
    s[..end].to_string()
}
