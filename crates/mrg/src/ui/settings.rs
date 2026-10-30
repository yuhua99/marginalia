use std::ptr;

use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use crate::app::{Component, Look, Outcome};
use crate::keymap::{Feed, Keymap};
use crate::settings::Settings;
use crate::theme::{Theme, themes};
use crate::ui::popup::Popup;

/// `s`: the settings. The theme row opens a list that previews the theme under the
/// cursor; Enter applies and saves it, Esc restores the one before. Enter on wrap
/// toggles it.
pub struct SettingsPopup {
    list: Popup,
    /// The theme list, and the theme to restore.
    themes: Option<(Popup, &'static Theme)>,
    keymap: Keymap,
}

impl SettingsPopup {
    pub fn new(look: &Look) -> SettingsPopup {
        SettingsPopup {
            list: list(look.theme, look.wrap, None),
            themes: None,
            keymap: Keymap::new("popup"),
        }
    }

    /// Handles a key, changing the theme to preview or apply, or wrap; true when the
    /// popup closes.
    fn key(&mut self, key: KeyEvent, look: &mut Look) -> bool {
        let Look { theme, wrap } = look;
        let Feed::Action(action) = self.keymap.feed(key) else {
            return false;
        };
        let mut error = None;
        if let Some((popup, original)) = &mut self.themes {
            match action {
                // A failed save keeps the theme for this run and says so.
                "open" | "focus_right" => error = save(theme, *wrap).err(),
                "cancel" | "quit" | "focus_left" => *theme = *original,
                _ => {
                    popup.act(action);
                    *theme = &themes()[popup.cursor];
                    return false;
                }
            }
            self.themes = None;
            self.list = list(theme, *wrap, error);
            return false;
        }
        match action {
            "open" | "focus_right" if self.list.cursor == 1 => {
                *wrap = !*wrap;
                let error = save(theme, *wrap).err();
                self.list = list(theme, *wrap, error);
                self.list.cursor = 1;
            }
            "open" | "focus_right" => {
                let rows = themes()
                    .iter()
                    .map(|t| (t.name.to_string(), String::new()))
                    .collect();
                let mut popup = Popup::new("Theme", rows);
                popup.cursor = themes()
                    .iter()
                    .position(|t| ptr::eq(t, *theme))
                    .unwrap_or(0);
                self.themes = Some((popup, *theme));
            }
            "settings" | "cancel" | "quit" | "focus_left" => return true,
            _ => self.list.act(action),
        }
        false
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, theme: &Theme) {
        self.list.render(f, area, theme);
        if let Some((popup, _)) = &mut self.themes {
            popup.render(f, area, theme);
        }
    }
}

/// The rows; a failed save shows in the title.
fn list(theme: &Theme, wrap: bool, error: Option<anyhow::Error>) -> Popup {
    let title = match error {
        Some(e) => format!("Settings (not saved: {e:#})"),
        None => "Settings".to_string(),
    };
    let wrap = if wrap { "on" } else { "off" };
    Popup::new(
        &title,
        vec![
            ("theme".to_string(), theme.name.to_string()),
            ("wrap".to_string(), wrap.to_string()),
        ],
    )
}

fn save(theme: &Theme, wrap: bool) -> Result<()> {
    Settings {
        theme: theme.name.to_string(),
        wrap,
        ..Settings::default()
    }
    .save()
}

impl<C: AsRef<Look> + AsMut<Look>> Component<C> for SettingsPopup {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut C) -> Result<Outcome<C>> {
        Ok(if self.key(key, ctx.as_mut()) {
            Outcome::Close
        } else {
            Outcome::Consumed
        })
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &C) {
        self.draw(f, area, ctx.as_ref().theme);
    }
}
