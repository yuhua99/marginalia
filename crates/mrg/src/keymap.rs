use std::collections::BTreeMap;
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

type Key = (KeyCode, KeyModifiers);

/// The keys as written, as parsed, and their action.
type Binding = (String, Vec<Key>, &'static str);

/// The `[keys.<context>]` tables of the settings: keys to an action, or `none` to unbind.
pub type Keys = BTreeMap<String, BTreeMap<String, String>>;

/// Maps key sequences such as `g g` to action names.
pub struct Keymap {
    bindings: Vec<Binding>,
    pending: Vec<Key>,
}

pub enum Feed {
    Pending,
    Action(&'static str),
    Unbound,
}

const DEFAULTS: &[(&str, &str)] = &[
    ("j", "down"),
    ("Down", "down"),
    ("k", "up"),
    ("Up", "up"),
    ("C-d", "half_page_down"),
    ("C-u", "half_page_up"),
    ("g g", "top"),
    ("G", "bottom"),
    ("n", "next_thread"),
    ("N", "prev_thread"),
    ("h", "focus_left"),
    ("Left", "focus_left"),
    ("l", "focus_right"),
    ("Right", "focus_right"),
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

/// Bindings over the defaults in each context; the user's `global` binds in all of them.
const CONTEXTS: &[(&str, &[(&str, &str)])] = &[
    ("global", &[]),
    ("picker", &[]),
    (
        "review",
        &[
            ("h", "prev_file"),
            ("Left", "prev_file"),
            ("l", "next_file"),
            ("Right", "next_file"),
            ("Tab", "next_focus"),
            ("S-Tab", "prev_focus"),
        ],
    ),
    ("file", &[]),
    ("popup", &[]),
];

/// The built-in bindings per context, which a new settings file starts with; written
/// under their own context, they bind as the built-ins do.
pub fn defaults() -> impl Iterator<Item = (&'static str, &'static [(&'static str, &'static str)])> {
    [("global", DEFAULTS)]
        .into_iter()
        .chain(CONTEXTS.iter().copied().filter(|(_, b)| !b.is_empty()))
}

/// The user's bindings and their context, from `init`.
static USER: OnceLock<Vec<(&'static str, Binding)>> = OnceLock::new();

/// Takes the user's bindings, which every `Keymap::new` after lays over the defaults.
pub fn init(keys: &Keys) -> Result<()> {
    assert!(USER.set(load(keys)?).is_ok(), "keymap::init twice");
    Ok(())
}

/// `keys` checked against the contexts, actions and key names, and for keys that
/// hide a longer sequence in some context.
fn load(keys: &Keys) -> Result<Vec<(&'static str, Binding)>> {
    let actions = DEFAULTS
        .iter()
        .chain(CONTEXTS.iter().flat_map(|(_, b)| b.iter()))
        .map(|(_, a)| *a)
        .chain(["none"]);
    let mut out = Vec::new();
    for (context, table) in keys {
        let Some((name, _)) = CONTEXTS.iter().find(|(c, _)| c == context) else {
            bail!("unknown context [keys.{context}]");
        };
        for (keys, action) in table {
            let Some(action) = actions.clone().find(|a| a == action) else {
                bail!("[keys.{context}] {keys:?}: unknown action {action:?}");
            };
            let seq = parse_seq(keys).with_context(|| format!("[keys.{context}] {keys:?}"))?;
            out.push((*name, (keys.clone(), seq, action)));
        }
    }
    for (context, _) in CONTEXTS {
        let bindings = Keymap::build(context, &out).bindings;
        for (a, seq, _) in &bindings {
            if let Some((b, _, _)) = bindings
                .iter()
                .find(|(_, s, _)| s != seq && s.starts_with(seq))
            {
                bail!("{a:?} hides {b:?} in {context}; unbind one with \"none\"");
            }
        }
    }
    Ok(out)
}

impl Default for Keymap {
    fn default() -> Keymap {
        Keymap::new("global")
    }
}

impl Keymap {
    /// The defaults of `context`, then the user's `global` and `context` bindings; a
    /// later binding replaces an earlier one on the same keys.
    pub fn new(context: &str) -> Keymap {
        assert!(CONTEXTS.iter().any(|(c, _)| *c == context), "{context}");
        Keymap::build(context, USER.get().map_or(&[], Vec::as_slice))
    }

    fn build(context: &str, user: &[(&str, Binding)]) -> Keymap {
        let builtin = CONTEXTS
            .iter()
            .filter(|(c, _)| *c == context)
            .flat_map(|(_, b)| b.iter());
        let builtin = DEFAULTS.iter().chain(builtin).map(|(keys, action)| {
            let seq = parse_seq(keys).expect("built-in keys");
            (keys.to_string(), seq, *action)
        });
        let user = ["global", context]
            .into_iter()
            .flat_map(|c| user.iter().filter(move |(u, _)| *u == c))
            .map(|(_, b)| b.clone());
        let mut bindings: Vec<Binding> = Vec::new();
        for (keys, seq, action) in builtin.chain(user) {
            bindings.retain(|(_, s, _)| *s != seq);
            if action != "none" {
                bindings.push((keys, seq, action));
            }
        }
        Keymap {
            bindings,
            pending: Vec::new(),
        }
    }

    /// The keys bound to `action`, such as `g g`, or `j,J` for two bindings.
    pub fn keys(&self, action: &str) -> String {
        self.bindings
            .iter()
            .filter(|(_, _, a)| *a == action)
            .map(|(keys, _, _)| keys.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The keys of the first binding of `action`, such as `g g`.
    pub fn first_key(&self, action: &str) -> Option<&str> {
        self.bindings
            .iter()
            .find(|(_, _, a)| *a == action)
            .map(|(keys, _, _)| keys.as_str())
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

/// Keys separated by spaces, such as `g g`.
fn parse_seq(keys: &str) -> Result<Vec<Key>> {
    let seq = keys
        .split_whitespace()
        .map(parse)
        .collect::<Result<Vec<_>>>()?;
    if seq.is_empty() {
        bail!("no keys");
    }
    Ok(seq)
}

/// `x`, `G`, `C-x`, `Esc`, `Enter`, `Space`, `Tab`, `S-Tab`, `Up`, `Down`, `Left` or `Right`.
fn parse(key: &str) -> Result<Key> {
    let none = KeyModifiers::NONE;
    Ok(match key {
        "Esc" => (KeyCode::Esc, none),
        "Enter" => (KeyCode::Enter, none),
        "Tab" => (KeyCode::Tab, none),
        "S-Tab" => (KeyCode::BackTab, none),
        "Space" => (KeyCode::Char(' '), none),
        "Up" => (KeyCode::Up, none),
        "Down" => (KeyCode::Down, none),
        "Left" => (KeyCode::Left, none),
        "Right" => (KeyCode::Right, none),
        _ => {
            let (k, modifiers) = match key.strip_prefix("C-") {
                Some(k) => (k, KeyModifiers::CONTROL),
                None => (key, none),
            };
            let mut chars = k.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                bail!("unknown key {key:?}");
            };
            (KeyCode::Char(c), modifiers)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(rows: &[(&str, &str, &str)]) -> Keys {
        let mut out = Keys::new();
        for (context, keys, action) in rows {
            out.entry(context.to_string())
                .or_default()
                .insert(keys.to_string(), action.to_string());
        }
        out
    }

    #[test]
    fn layers() {
        let user = load(&keys(&[
            ("global", "J", "down"),
            ("global", "d", "none"),
            ("review", "J", "next_file"),
            ("picker", "x", "delete"),
        ]))
        .unwrap();
        let review = Keymap::build("review", &user);
        assert_eq!(review.keys("prev_file"), "h,Left");
        assert_eq!(review.keys("focus_left"), "");
        assert_eq!(review.keys("down"), "j,Down");
        assert_eq!(review.keys("next_file"), "l,Right,J");
        assert_eq!(review.keys("delete"), "");
        let picker = Keymap::build("picker", &user);
        assert_eq!(picker.keys("down"), "j,Down,J");
        assert_eq!(picker.keys("delete"), "x");
        assert_eq!(picker.keys("focus_left"), "h,Left");
    }

    #[test]
    fn defaults_bind_as_built_ins() {
        let rows: Vec<_> = defaults()
            .flat_map(|(c, b)| b.iter().map(move |(k, a)| (c, *k, *a)))
            .collect();
        let user = load(&keys(&rows)).unwrap();
        for (context, _) in CONTEXTS {
            let sorted = |keymap: Keymap| {
                let mut b: Vec<_> = keymap
                    .bindings
                    .into_iter()
                    .map(|(k, _, a)| (k, a))
                    .collect();
                b.sort();
                b
            };
            assert_eq!(
                sorted(Keymap::build(context, &user)),
                sorted(Keymap::build(context, &[])),
                "{context}"
            );
        }
    }

    #[test]
    fn rejects() {
        for row in [
            ("nope", "j", "down"),
            ("review", "j", "nope"),
            ("review", "Foo", "down"),
            ("review", "C-", "down"),
            ("review", " ", "down"),
            ("review", "d d", "delete"),
            ("global", "g", "top"),
        ] {
            assert!(load(&keys(&[row])).is_err(), "{row:?}");
        }
    }
}
