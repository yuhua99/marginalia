use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

use crate::app::{Component, Look, Outcome};
use crate::ui::dialog;

type Answer<C> = Box<dyn FnMut(&mut C) -> Result<Outcome<C>>>;

/// A question answered by a key: the key of an answer runs it, Esc closes, other keys wait.
pub struct Confirm<C> {
    title: &'static str,
    question: String,
    keys: &'static str,
    answers: Vec<(char, Answer<C>)>,
}

impl<C> Confirm<C> {
    pub fn new(title: &'static str, question: impl Into<String>, keys: &'static str) -> Confirm<C> {
        Confirm {
            title,
            question: question.into(),
            keys,
            answers: Vec::new(),
        }
    }

    /// Runs `answer` on `key`.
    pub fn on(
        mut self,
        key: char,
        answer: impl FnMut(&mut C) -> Result<Outcome<C>> + 'static,
    ) -> Confirm<C> {
        self.answers.push((key, Box::new(answer)));
        self
    }
}

impl<C: AsRef<Look>> Component<C> for Confirm<C> {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut C) -> Result<Outcome<C>> {
        if key.code == KeyCode::Esc {
            return Ok(Outcome::Close);
        }
        for (k, answer) in &mut self.answers {
            if key.code == KeyCode::Char(*k) {
                return answer(ctx);
            }
        }
        Ok(Outcome::Consumed)
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &C) {
        let theme = ctx.as_ref().theme;
        dialog::confirm(f, area, self.title, &self.question, self.keys, theme);
    }
}
