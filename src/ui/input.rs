use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::widgets::Block;
use ratatui_textarea::{TextArea, WrapMode};

use crate::app::{Component, Ctx, Outcome};
use crate::session::{Anchor, Session};
use crate::ui::clear;

/// Text box for a new comment, a reply or an edit. `Ctrl-s` sends, `Esc` cancels.
pub struct Input {
    target: Target,
    title: String,
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
        let title = format!("comment on {}", session.location(&anchor));
        Input::new(Target::Comment(anchor), title, "")
    }

    pub fn reply(idx: usize, session: &Session) -> Input {
        let title = format!("reply to {}", session.threads[idx].id);
        Input::new(Target::Reply(idx), title, "")
    }

    pub fn edit(idx: usize, session: &Session) -> Input {
        let t = &session.threads[idx];
        let title = format!("edit your message in {}", t.id);
        let body = &t.messages.last().expect("a message").body;
        Input::new(Target::Edit(idx), title, body)
    }

    fn new(target: Target, title: String, body: &str) -> Input {
        let mut text = TextArea::default();
        text.set_wrap_mode(WrapMode::WordOrGlyph);
        text.insert_str(body);
        Input {
            target,
            title,
            text,
            original: body.to_string(),
            discarding: false,
            error: None,
        }
    }
}

impl Component for Input {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Result<Outcome> {
        match key.code {
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let body = self.text.lines().join("\n");
                if body.trim().is_empty() {
                    return Ok(Outcome::Consumed);
                }
                let sent = match &self.target {
                    Target::Comment(anchor) => {
                        ctx.session
                            .comment(anchor.clone(), "user", body, &mut ctx.repo)
                    }
                    Target::Reply(idx) => {
                        let id = ctx.session.threads[*idx].id.clone();
                        ctx.session.reply(&id, "user", body)
                    }
                    Target::Edit(idx) => {
                        ctx.session.edit(*idx, body);
                        Ok(())
                    }
                };
                if let Err(e) = sent {
                    self.error = Some(format!("{e:#}"));
                    return Ok(Outcome::Consumed);
                }
                ctx.threads_changed()?;
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
                self.text.input(key);
                Ok(Outcome::Consumed)
            }
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let height = area.height.min(12);
        let rect = Rect {
            y: area.bottom() - height,
            height,
            ..area
        };
        let hint = match &self.error {
            _ if self.discarding => "Esc again to discard".to_string(),
            Some(e) => format!("error: {e}"),
            None => "Ctrl-s send · Esc cancel".to_string(),
        };
        self.text.set_block(
            Block::bordered()
                .title(format!(" {} ", self.title))
                .title_bottom(format!(" {hint} ")),
        );
        clear(f, rect, ctx.theme);
        f.render_widget(&self.text, rect);
    }
}
