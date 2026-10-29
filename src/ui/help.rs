use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use crate::app::{Component, Look, Outcome};
use crate::keymap::{Feed, Keymap};
use crate::ui::popup::Popup;

/// Actions shown on one help row, and what they do there.
pub type Entry = (&'static [&'static str], &'static str);

/// The keys of `actions` in `keymap`, joined with `/`.
fn keys(actions: &[&str], keymap: &Keymap) -> String {
    let keys: Vec<_> = actions
        .iter()
        .map(|a| keymap.keys(a))
        .filter(|k| !k.is_empty())
        .collect();
    keys.join("/")
}

/// The entries with a key bound to any of their actions.
fn bound<'a>(entries: &'a [Entry], keymap: &'a Keymap) -> impl Iterator<Item = &'a Entry> {
    entries
        .iter()
        .filter(|(actions, _)| !keys(actions, keymap).is_empty())
}

/// The keys of `entries` as a popup.
fn popup(entries: &[Entry], keymap: &Keymap) -> Popup {
    let rows = bound(entries, keymap)
        .map(|(actions, text)| (keys(actions, keymap), text.to_string()))
        .collect();
    Popup::new("Keys", rows)
}

/// The keys of `entries` on one footer line, sequences such as `g g` written `gg`.
pub fn hints(entries: &[Entry], keymap: &Keymap) -> String {
    let hints: Vec<_> = bound(entries, keymap)
        .map(|(actions, text)| format!("{} {text}", keys(actions, keymap).replace(' ', "")))
        .collect();
    format!(" {}", hints.join("  "))
}

/// `?`: the keys of the layer below; `?`, `q`, `Esc` or `h` close it.
pub struct Help {
    popup: Popup,
    keymap: Keymap,
}

impl Help {
    /// The keys of `entries` as `keymap` binds them.
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

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &C) {
        self.popup.render(f, area, ctx.as_ref().theme);
    }
}
