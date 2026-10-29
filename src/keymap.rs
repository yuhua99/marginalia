use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

type Key = (KeyCode, KeyModifiers);

/// Maps key sequences such as `g g` to action names.
pub struct Keymap {
    /// The keys as written, as parsed, and their action.
    bindings: Vec<(&'static str, Vec<Key>, &'static str)>,
    pending: Vec<Key>,
}

pub enum Feed {
    Pending,
    Action(&'static str),
    Unbound,
}

const DEFAULTS: &[(&str, &str)] = &[
    ("j", "down"),
    ("k", "up"),
    ("C-d", "half_page_down"),
    ("C-u", "half_page_up"),
    ("g g", "top"),
    ("G", "bottom"),
    ("n", "next_thread"),
    ("N", "prev_thread"),
    ("h", "focus_left"),
    ("l", "focus_right"),
    ("Enter", "open"),
    ("Space", "check"),
    ("v", "select"),
    ("Esc", "cancel"),
    ("c", "comment"),
    ("r", "collapse"),
    ("e", "edit"),
    ("d", "delete"),
    ("q", "quit"),
    ("Q", "back"),
    ("?", "help"),
    ("s", "settings"),
    ("/", "filter"),
];

impl Default for Keymap {
    fn default() -> Keymap {
        Keymap::with(&[])
    }
}

impl Keymap {
    /// The defaults with `overrides`, which replace the defaults on the same keys.
    pub fn with(overrides: &[(&'static str, &'static str)]) -> Keymap {
        let defaults = DEFAULTS
            .iter()
            .filter(|(keys, _)| overrides.iter().all(|(k, _)| k != keys));
        let bindings = overrides
            .iter()
            .chain(defaults)
            .map(|(keys, action)| (*keys, keys.split(' ').map(parse).collect(), *action))
            .collect();
        Keymap {
            bindings,
            pending: Vec::new(),
        }
    }

    /// The keys bound to `action`, such as `g g`.
    pub fn keys(&self, action: &str) -> String {
        self.bindings
            .iter()
            .filter(|(_, _, a)| *a == action)
            .map(|(keys, _, _)| *keys)
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn feed(&mut self, key: KeyEvent) -> Feed {
        let mut modifiers = key.modifiers;
        if matches!(key.code, KeyCode::Char(_) | KeyCode::BackTab) {
            // The case of the char, or BackTab itself, already carries shift.
            modifiers.remove(KeyModifiers::SHIFT);
        }
        self.pending.push((key.code, modifiers));
        let mut prefix = false;
        for (_, seq, action) in &self.bindings {
            if *seq == self.pending {
                self.pending.clear();
                return Feed::Action(action);
            }
            prefix |= seq.starts_with(&self.pending);
        }
        if prefix {
            return Feed::Pending;
        }
        // A broken sequence: retry the last key on its own.
        let retry = self.pending.len() > 1;
        self.pending.clear();
        if retry { self.feed(key) } else { Feed::Unbound }
    }
}

/// `x`, `G`, `C-x`, `Esc`, `Enter`, `Space`, `Tab` or `S-Tab`.
fn parse(key: &str) -> Key {
    match key {
        "Esc" => return (KeyCode::Esc, KeyModifiers::NONE),
        "Enter" => return (KeyCode::Enter, KeyModifiers::NONE),
        "Tab" => return (KeyCode::Tab, KeyModifiers::NONE),
        "S-Tab" => return (KeyCode::BackTab, KeyModifiers::NONE),
        "Space" => return (KeyCode::Char(' '), KeyModifiers::NONE),
        _ => {}
    }
    match key.strip_prefix("C-") {
        Some(k) => (
            KeyCode::Char(k.chars().next().unwrap()),
            KeyModifiers::CONTROL,
        ),
        None => (
            KeyCode::Char(key.chars().next().unwrap()),
            KeyModifiers::NONE,
        ),
    }
}
