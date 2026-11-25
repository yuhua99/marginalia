use std::ops::Range;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Style;
use ratatui::text::Span;

use crate::doc::{Doc, Row};
use crate::session::Thread;
use crate::theme::Theme;

#[derive(Default)]
pub struct Query {
    pub text: String,
    pub typing: bool,
    counted: Option<(u64, (Vec<usize>, usize))>,
}

impl Query {
    pub fn start() -> Query {
        Query {
            typing: true,
            ..Query::default()
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> bool {
        self.counted = None;
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

    pub fn paste(&mut self, text: &str) {
        self.counted = None;
        self.text.push_str(text.lines().next().unwrap_or(""));
    }

    pub fn title(&self) -> String {
        if self.text.is_empty() && !self.typing {
            return String::new();
        }
        format!("/{} ", self.text)
    }

    pub fn search_title(&mut self, doc: &Doc, at: usize, threads: &[Thread]) -> String {
        if self.typing || self.text.is_empty() {
            return self.title();
        }
        let version = doc.version();
        if self.counted.as_ref().is_none_or(|c| c.0 != version) {
            self.counted = Some((version, doc.tally(|row| self.hit(row, threads))));
        }
        let (_, (before, total)) = self.counted.as_ref().expect("counted above");
        let on = match doc.window(at, 1) {
            [row] if self.hit(row, threads) => (before[at] + 1).to_string(),
            _ => "-".to_string(),
        };
        format!("/{} [{on}/{total}] ", self.text)
    }

    fn hit(&self, row: &Row, threads: &[Thread]) -> bool {
        let has = |s: &str| !hits(s, &self.text).is_empty();
        match row {
            Row::File { path, .. } => has(path),
            Row::Code { left, right, .. } => {
                [left, right].into_iter().flatten().any(|s| has(&s.text))
            }
            Row::Thread { idx } => threads[*idx].messages.iter().any(|m| has(&m.body)),
            Row::Commit { message, .. } => has(message),
            Row::Session | Row::Fold { .. } => false,
        }
    }

    pub fn find(
        &self,
        doc: &mut Doc,
        from: usize,
        forward: bool,
        threads: &[Thread],
    ) -> Option<usize> {
        doc.search(from, forward, |row| self.hit(row, threads))
    }
}

pub fn hit_style(theme: &Theme, current: bool) -> Style {
    let bg = if current { theme.yellow } else { theme.teal };
    Style::new().fg(theme.bg).bg(bg)
}

pub fn hits(text: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let query = query.to_lowercase();
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

pub fn mark(spans: Vec<Span<'static>>, query: &str, style: Style) -> Vec<Span<'static>> {
    let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
    let hits = hits(&joined, query);
    let mut out = Vec::new();
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
    fn titles_which_match_of_all() {
        let doc = Doc::file("r".into(), "a".into(), "foo\nfoo foo\nbar");
        let mut q = Query::start();
        q.paste("foo");
        assert_eq!(q.search_title(&doc, 2, &[]), "/foo ");
        q.typing = false;
        assert_eq!(q.search_title(&doc, 2, &[]), "/foo [2/2] ");
        assert_eq!(q.search_title(&doc, 3, &[]), "/foo [-/2] ");
    }

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
