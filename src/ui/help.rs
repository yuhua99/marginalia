use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use crate::app::{Component, Ctx, Outcome};
use crate::keymap::{self, Feed, Keymap};
use crate::ui::popup::Popup;

/// Actions shown on one help row, and what they do there.
pub type Entry = (&'static [&'static str], &'static str);

/// The keys of `entries` as a popup; actions on one row join their keys with `/`.
pub fn popup(entries: &[Entry]) -> Popup {
    let rows = entries
        .iter()
        .map(|(actions, text)| {
            let keys: Vec<_> = actions.iter().map(|a| keymap::keys(a)).collect();
            (keys.join("/"), text.to_string())
        })
        .collect();
    Popup::new("Keys", rows)
}

/// `?`: the keys of the layer below; `?`, `q` or `Esc` close it.
pub struct Help {
    popup: Popup,
    keymap: Keymap,
}

impl Help {
    pub fn new(entries: &[Entry]) -> Help {
        Help {
            popup: popup(entries),
            keymap: Keymap::default(),
        }
    }
}

impl Component for Help {
    fn handle_key(&mut self, key: KeyEvent, _ctx: &mut Ctx) -> Result<Outcome> {
        Ok(match self.keymap.feed(key) {
            Feed::Action("help" | "quit" | "cancel") => Outcome::Close,
            Feed::Action(action) => {
                self.popup.act(action);
                Outcome::Consumed
            }
            _ => Outcome::Consumed,
        })
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        self.popup.render(f, area, ctx.theme);
    }
}
