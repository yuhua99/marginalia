use std::ops::Range;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Style;
use ratatui::text::Span;

use crate::doc::{Doc, Row};
use crate::session::Thread;
use crate::theme::Theme;

/// A line typed after `/`: a search, or a filter.
#[derive(Default)]
pub struct Query {
    pub text: String,
    /// Keys go to the text.
    pub typing: bool,
    /// The last search found nothing.
    missed: bool,
}

impl Query {
    /// A new line, typing.
    pub fn start() -> Query {
        Query {
            typing: true,
            ..Query::default()
        }
    }

    /// Takes `key` while typing; true when Enter ends a non-blank line. Esc clears it.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        self.missed = false;
        match key.code {
            KeyCode::Char(c) => self.text.push(c),
            KeyCode::Backspace => {
                self.text.pop();
            }
            KeyCode::Enter => {
                self.typing = false;
                return !self.text.is_empty();
            }
            KeyCode::Esc => *self = Query::default(),
            _ => {}
        }
        false
    }

    /// Adds the first line of pasted `text`.
    pub fn paste(&mut self, text: &str) {
        self.missed = false;
        self.text.push_str(text.lines().next().unwrap_or(""));
    }

    /// `/text ` for a box title while there is one.
    pub fn title(&self) -> String {
        if self.text.is_empty() && !self.typing {
            return String::new();
        }
        let missed = if self.missed { "(no match) " } else { "" };
        format!("/{} {missed}", self.text)
    }

    /// The next (or previous) row of `doc` from `from` with the text in it, any case:
    /// a file path, a code line, or a message of `threads`. Expands a collapsed file it is in.
    pub fn find(
        &mut self,
        doc: &mut Doc,
        from: usize,
        forward: bool,
        threads: &[Thread],
    ) -> Option<usize> {
        let has = |s: &str| !hits(s, &self.text).is_empty();
        let found = doc.search(from, forward, |row| match row {
            Row::File { path, .. } => has(path),
            Row::Code { left, right, .. } => {
                [left, right].into_iter().flatten().any(|s| has(&s.text))
            }
            Row::Thread { idx } => threads[*idx].messages.iter().any(|m| has(&m.body)),
            Row::Session | Row::Fold { .. } => false,
        });
        self.missed = found.is_none();
        found
    }
}

/// How a search match shows.
pub fn hit_style(theme: &Theme) -> Style {
    Style::new().fg(theme.bg).bg(theme.yellow)
}

/// Byte ranges of `query` in `text`, any case.
pub fn hits(text: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let query = query.to_lowercase();
    // Lowercased, with the byte of `text` each of its bytes comes from.
    let (mut lower, mut from) = (String::new(), Vec::new());
    for (at, c) in text.char_indices() {
        for l in c.to_lowercase() {
            lower.push(l);
            from.extend(std::iter::repeat_n(at, l.len_utf8()));
        }
    }
    from.push(text.len());
    lower
        .match_indices(&query)
        .map(|(i, m)| from[i]..from[i + m.len()])
        .collect()
}

/// `spans` with the matches of `query` in their joined text patched with `style`,
/// split where a match starts or ends inside one.
pub fn mark(spans: Vec<Span<'static>>, query: &str, style: Style) -> Vec<Span<'static>> {
    let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
    let hits = hits(&joined, query);
    let mut out = Vec::new();
    // Where the span starts in `joined`.
    let mut start = 0;
    for span in spans {
        let text = span.content.as_ref();
        let end = start + text.len();
        let mut cuts = vec![start, end];
        cuts.extend(
            hits.iter()
                .flat_map(|r| [r.start, r.end])
                .filter(|c| (start + 1..end).contains(c)),
        );
        cuts.sort();
        for w in cuts.windows(2) {
            let on = hits.iter().any(|r| r.start <= w[0] && w[1] <= r.end);
            let s = if on {
                span.style.patch(style)
            } else {
                span.style
            };
            out.push(Span::styled(
                text[w[0] - start..w[1] - start].to_string(),
                s,
            ));
        }
        start = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_any_case() {
        assert_eq!(hits("Foo foo", "FOO"), [0..3, 4..7]);
        assert_eq!(hits("ÄbÄB", "äb"), [0..3, 3..6]);
        assert!(hits("foo", "").is_empty());
    }

    #[test]
    fn marks_across_spans() {
        let on = Style::new().bg(ratatui::style::Color::Yellow);
        let spans = vec![Span::raw("see "), Span::raw("src/a.rs")];
        let out: Vec<_> = mark(spans, "e s", on)
            .into_iter()
            .map(|s| (s.content.to_string(), s.style == on))
            .collect();
        let want = [("se", false), ("e ", true), ("s", true), ("rc/a.rs", false)];
        assert_eq!(out, want.map(|(t, b)| (t.to_string(), b)));
    }
}
