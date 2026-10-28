use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Paragraph};
use ratatui_textarea::{TextArea, WrapMode};

use crate::app::{Component, Ctx, Outcome};
use crate::session::{Anchor, Session};
use crate::ui::diff_view::INDENT;
use unicode_width::UnicodeWidthStr;

const WHO: &str = "user: ";

/// A draft message for a new comment, a reply or an edit, drawn by the diff as the next
/// `user:` line of a thread card. `Enter` sends, `Shift-Enter` or `Ctrl-j` breaks the
/// line, `Esc` cancels.
pub struct Input {
    target: Target,
    /// A new comment's card header; replies and edits sit in their thread's card.
    header: Option<String>,
    text: TextArea<'static>,
    /// The text the box opened with; `Esc` discards at once while it is unchanged.
    original: String,
    /// `Esc` was pressed once with text in the box.
    discarding: bool,
    /// Why the last send failed; the text stays for a retry.
    error: Option<String>,
}

enum Target {
    Comment(Anchor),
    Reply(usize),
    /// The user's last message of a thread.
    Edit(usize),
}

impl Input {
    pub fn comment(anchor: Anchor, session: &Session) -> Input {
        let header = format!("new  {}", session.location(&anchor));
        Input::new(Target::Comment(anchor), Some(header), "")
    }

    pub fn reply(idx: usize) -> Input {
        Input::new(Target::Reply(idx), None, "")
    }

    pub fn edit(idx: usize, session: &Session) -> Input {
        let body = &session.threads[idx]
            .messages
            .last()
            .expect("a message")
            .body;
        Input::new(Target::Edit(idx), None, body)
    }

    fn new(target: Target, header: Option<String>, body: &str) -> Input {
        let mut text = TextArea::default();
        text.set_wrap_mode(WrapMode::WordOrGlyph);
        text.set_cursor_line_style(Style::new());
        text.insert_str(body);
        Input {
            target,
            header,
            text,
            original: body.to_string(),
            discarding: false,
            error: None,
        }
    }

    /// The thread whose last message this replaces.
    pub fn editing(&self) -> Option<usize> {
        match self.target {
            Target::Edit(idx) => Some(idx),
            _ => None,
        }
    }

    /// Rows the card takes at `width`: header, text, hint.
    pub fn height(&self, width: usize) -> usize {
        let w = width.saturating_sub(INDENT + WHO.len()).max(1);
        let text: usize = self
            .text
            .lines()
            .iter()
            // Word wrapping, or by glyph with room for the cursor, whichever is taller.
            .map(|l| textwrap::wrap(l, w).len().max(l.width() / w + 1))
            .sum();
        self.header.is_some() as usize + text + 1
    }
}

impl Component for Input {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Result<Outcome> {
        // Shift-Enter needs a terminal reporting it apart from Enter; Ctrl-j works anywhere.
        let newline = (key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT))
            || (key.code == KeyCode::Char('j') && key.modifiers.contains(KeyModifiers::CONTROL));
        match key.code {
            KeyCode::Enter if !newline => {
                let body = self.text.lines().join("\n");
                if body.trim().is_empty() {
                    return Ok(Outcome::Consumed);
                }
                let sent = match &self.target {
                    Target::Comment(anchor) => ctx.comment(anchor.clone(), body)?,
                    Target::Reply(idx) => ctx.reply(*idx, body)?,
                    Target::Edit(idx) => ctx.edit(*idx, body).map(Ok)?,
                };
                if let Err(e) = sent {
                    self.error = Some(format!("{e:#}"));
                    return Ok(Outcome::Consumed);
                }
                Ok(Outcome::Close)
            }
            KeyCode::Esc => {
                if self.text.lines().join("\n") == self.original || self.discarding {
                    return Ok(Outcome::Close);
                }
                self.discarding = true;
                Ok(Outcome::Consumed)
            }
            _ => {
                self.discarding = false;
                self.error = None;
                if newline {
                    self.text.insert_newline();
                } else {
                    self.text.input(key);
                }
                Ok(Outcome::Consumed)
            }
        }
    }

    fn paste(&mut self, text: &str) {
        self.discarding = false;
        self.error = None;
        self.text
            .insert_str(text.replace("\r\n", "\n").replace('\r', "\n"));
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let theme = ctx.theme;
        f.render_widget(Block::new().style(Style::new().bg(theme.surface)), area);
        let indent = INDENT as u16;
        let who = WHO.len() as u16;
        let row = |y, height| Rect {
            x: area.x + indent,
            y,
            width: area.width.saturating_sub(indent),
            height,
        };
        let mut y = area.y;
        if let Some(header) = &self.header {
            let bold = Style::new().add_modifier(Modifier::BOLD);
            f.render_widget(Paragraph::new(header.as_str()).style(bold), row(y, 1));
            y += 1;
        }
        // A clipped card drops the hint first.
        let hint = area.bottom() > y + 1;
        let text = row(y, area.bottom().saturating_sub(y + hint as u16));
        f.render_widget(Paragraph::new(WHO).style(Style::new().fg(theme.teal)), text);
        f.render_widget(
            &self.text,
            Rect {
                x: text.x + who,
                width: text.width.saturating_sub(who),
                ..text
            },
        );
        let (text_hint, color) = match &self.error {
            _ if self.discarding => ("Esc again to discard".to_string(), theme.dim),
            Some(e) => (format!("error: {e}"), theme.red),
            None => (
                "Enter send / Shift-Enter or Ctrl-j newline / Esc cancel".to_string(),
                theme.dim,
            ),
        };
        if hint {
            let hint_row = row(area.bottom() - 1, 1);
            f.render_widget(
                Paragraph::new(text_hint).style(Style::new().fg(color)),
                hint_row,
            );
        }
    }
}
