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
    ("v", "select"),
    ("Esc", "cancel"),
    ("c", "comment"),
    ("r", "reply"),
    ("x", "resolve"),
    ("q", "quit"),
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

/// `x`, `G`, `C-x` or `Esc`.
fn parse(key: &str) -> Key {
    if key == "Esc" {
        return (KeyCode::Esc, KeyModifiers::NONE);
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
