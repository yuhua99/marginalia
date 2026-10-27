use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

type Key = (KeyCode, KeyModifiers);

/// Maps key sequences such as `g g` to action names.
pub struct Keymap {
    bindings: Vec<(Vec<Key>, &'static str)>,
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
    ("n", "next_waiting"),
    ("N", "prev_waiting"),
    ("J", "next_file"),
    ("K", "prev_file"),
    ("h", "focus_left"),
    ("l", "focus_right"),
    ("Enter", "open"),
    ("Space", "check"),
    ("v", "select"),
    ("Esc", "cancel"),
    ("c", "comment"),
    ("r", "reply"),
    ("x", "resolve"),
    ("e", "edit"),
    ("d", "delete"),
    ("q", "quit"),
    ("?", "help"),
    ("/", "filter"),
    ("X", "delete_session"),
];

impl Default for Keymap {
    fn default() -> Keymap {
        let bindings = DEFAULTS
            .iter()
            .map(|(keys, action)| (keys.split(' ').map(parse).collect(), *action))
            .collect();
        Keymap {
            bindings,
            pending: Vec::new(),
        }
    }
}

impl Keymap {
    pub fn feed(&mut self, key: KeyEvent) -> Feed {
        let mut modifiers = key.modifiers;
        if matches!(key.code, KeyCode::Char(_)) {
            // The case of the char already carries shift.
            modifiers.remove(KeyModifiers::SHIFT);
        }
        self.pending.push((key.code, modifiers));
        let mut prefix = false;
        for (seq, action) in &self.bindings {
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

/// The keys bound to `action`, such as `g g`.
pub fn keys(action: &str) -> String {
    DEFAULTS
        .iter()
        .filter(|(_, a)| *a == action)
        .map(|(keys, _)| *keys)
        .collect::<Vec<_>>()
        .join(" ")
}

/// `x`, `G`, `C-x`, `Esc`, `Enter` or `Space`.
fn parse(key: &str) -> Key {
    match key {
        "Esc" => return (KeyCode::Esc, KeyModifiers::NONE),
        "Enter" => return (KeyCode::Enter, KeyModifiers::NONE),
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
