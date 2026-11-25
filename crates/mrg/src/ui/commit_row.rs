use ratatui::style::Style;
use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

use crate::repo::Commit;
use crate::theme::Theme;
use crate::ui::Mark;

pub const MIN_SUBJECT: usize = 20;

const MAX_AUTHOR: usize = 16;

const MAX_REFS: usize = 24;

pub fn mark(checked: bool, theme: &Theme) -> Span<'static> {
    let mark = if checked { "☒ " } else { "☐ " };
    Span::styled(mark, Style::new().fg(theme.accent))
}

#[derive(Default)]
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

    pub fn row<'a>(
        &self,
        commit: &'a Commit,
        width: usize,
        theme: &Theme,
        mark: Mark,
    ) -> Vec<Span<'a>> {
        let author = fit(&commit.author, self.author);
        let mut meta = format!(
            "{author}{}  {}{}",
            " ".repeat(self.author - author.width()),
            " ".repeat(self.when - commit.when.width()),
            commit.when
        );
        let width = width.saturating_sub(8 + 2);
        if width < meta.width() + MIN_SUBJECT {
            meta.clear();
        }
        let room = width - meta.width();
        let mut refs = fit(&commit.refs, MAX_REFS);
        if !refs.is_empty() {
            refs = format!(" ({refs})");
        }
        if room < refs.width() + MIN_SUBJECT {
            refs.clear();
        }
        let subject = fit(&commit.subject, room - refs.width());
        let pad = " ".repeat(room - refs.width() - subject.width() + 2);
        vec![
            mark.lit(Span::styled(&commit.sha[..7], Style::new().fg(theme.dim))),
            mark.lit(Span::raw(format!(" {subject}"))),
            mark.lit(Span::styled(refs, Style::new().fg(theme.yellow))),
            Span::raw(pad),
            Span::styled(meta, Style::new().fg(theme.dim)),
        ]
    }
}

pub fn fit(s: &str, width: usize) -> String {
    let end = s
        .char_indices()
        .map(|(i, c)| i + c.len_utf8())
        .take_while(|&end| s[..end].width() <= width)
        .last()
        .unwrap_or(0);
    s[..end].to_string()
}
