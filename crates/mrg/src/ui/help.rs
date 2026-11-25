use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::{Position, Rect};

use crate::app::{Component, Look, Outcome};
use crate::keymap::{Feed, Keymap};
use crate::ui::popup::Popup;

pub type Entry = (&'static [&'static str], &'static str);

fn keys(actions: &[&str], keymap: &Keymap) -> String {
    let keys: Vec<_> = actions
        .iter()
        .map(|a| keymap.keys(a))
        .filter(|k| !k.is_empty())
        .collect();
    keys.join("/")
}

fn bound<'a>(entries: &'a [Entry], keymap: &'a Keymap) -> impl Iterator<Item = &'a Entry> {
    entries
        .iter()
        .filter(|(actions, _)| !keys(actions, keymap).is_empty())
}

fn popup(entries: &[Entry], keymap: &Keymap) -> Popup {
    let rows = bound(entries, keymap)
        .map(|(actions, text)| (keys(actions, keymap), text.to_string()))
        .collect();
    Popup::new("Keys", rows)
}

pub fn hints(entries: &[Entry], keymap: &Keymap) -> String {
    let hints: Vec<_> = bound(entries, keymap)
        .map(|(actions, text)| {
            let keys: Vec<_> = actions.iter().filter_map(|a| keymap.first_key(a)).collect();
            format!("{} {text}", keys.join("/").replace(' ', ""))
        })
        .collect();
    format!(" {}", hints.join("  "))
}

pub struct Help {
    popup: Popup,
    keymap: Keymap,
}

impl Help {
    pub fn new(entries: &[Entry], keymap: &Keymap) -> Help {
        Help {
            popup: popup(entries, keymap),
            keymap: Keymap::new("popup"),
        }
    }
}

impl<C: AsRef<Look>> Component<C> for Help {
    fn handle_key(&mut self, key: KeyEvent, _ctx: &mut C) -> Result<Outcome<C>> {
        Ok(match self.keymap.feed(key) {
            Feed::Action("help" | "quit" | "cancel" | "focus_left") => Outcome::Close,
            Feed::Action(action) => {
                self.popup.act(action);
                Outcome::Consumed
            }
            _ => Outcome::Consumed,
        })
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &C, _pointer: Option<Position>) {
        self.popup.render(f, area, ctx.as_ref().theme);
    }
}
