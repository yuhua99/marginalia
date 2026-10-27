use std::ptr;

use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use crate::app::{Component, Ctx, Outcome};
use crate::keymap::{Feed, Keymap};
use crate::settings::Settings;
use crate::theme::{THEMES, Theme};
use crate::ui::popup::Popup;

/// `s`: the settings. The theme row opens a list that previews the theme under the
/// cursor; Enter applies and saves it, Esc restores the one before.
pub struct SettingsPopup {
    list: Popup,
    /// The theme list, and the theme to restore.
    themes: Option<(Popup, &'static Theme)>,
    keymap: Keymap,
}

impl SettingsPopup {
    pub fn new(theme: &Theme) -> SettingsPopup {
        SettingsPopup {
            list: list(theme, None),
            themes: None,
            keymap: Keymap::default(),
        }
    }

    /// Handles a key, changing `theme` to preview or apply; true when the popup closes.
    pub fn key(&mut self, key: KeyEvent, theme: &mut &'static Theme) -> bool {
        let Feed::Action(action) = self.keymap.feed(key) else {
            return false;
        };
        let mut error = None;
        if let Some((popup, original)) = &mut self.themes {
            match action {
                // A failed save keeps the theme for this run and says so.
                "open" | "focus_right" => error = save(theme.name).err(),
                "cancel" | "quit" | "focus_left" => *theme = *original,
                _ => {
                    popup.act(action);
                    *theme = &THEMES[popup.cursor];
                    return false;
                }
            }
            self.themes = None;
            self.list = list(theme, error);
            return false;
        }
        match action {
            // The theme is the only row so far.
            "open" | "focus_right" => {
                let rows = THEMES
                    .iter()
                    .map(|t| (t.name.to_string(), String::new()))
                    .collect();
                let mut popup = Popup::new("Theme", rows);
                popup.cursor = THEMES.iter().position(|t| ptr::eq(t, *theme)).unwrap_or(0);
                self.themes = Some((popup, *theme));
            }
            "settings" | "cancel" | "quit" | "focus_left" => return true,
            _ => self.list.act(action),
        }
        false
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, theme: &Theme) {
        self.list.render(f, area, theme);
        if let Some((popup, _)) = &self.themes {
            popup.render(f, area, theme);
        }
    }
}

fn list(theme: &Theme, error: Option<anyhow::Error>) -> Popup {
    let mut value = theme.name.to_string();
    if let Some(e) = error {
        value.push_str(&format!(" (not saved: {e:#})"));
    }
    Popup::new("Settings", vec![("theme".to_string(), value)])
}

fn save(theme: &str) -> Result<()> {
    let mut settings = Settings::load()?;
    settings.theme = theme.to_string();
    settings.save()
}

impl Component for SettingsPopup {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Result<Outcome> {
        Ok(if self.key(key, &mut ctx.theme) {
            Outcome::Close
        } else {
            Outcome::Consumed
        })
    }

    fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        self.draw(f, area, ctx.theme);
    }
}
