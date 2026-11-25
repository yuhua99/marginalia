use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::str::FromStr;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

use syntect::easy::HighlightLines;
use syntect::highlighting::{ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::repo::{Blobs, Repo};

const MAX_BYTES: usize = 1 << 20;
const MAX_LINE: usize = 2000;

#[derive(Clone, Copy, PartialEq)]
pub enum Slot {
    Text,
    Comment,
    Red,
    Green,
    Yellow,
    Blue,
    Mauve,
    Teal,
    Peach,
}

impl Slot {
    pub const ALL: [Slot; 9] = [
        Slot::Text,
        Slot::Comment,
        Slot::Red,
        Slot::Green,
        Slot::Yellow,
        Slot::Blue,
        Slot::Mauve,
        Slot::Teal,
        Slot::Peach,
    ];
}

pub type Colors = Vec<Vec<(Slot, usize)>>;
type Key = (String, String);

pub struct Highlighter {
    jobs: Sender<(Key, Option<String>)>,
    done: Receiver<(Key, Colors)>,
    cache: HashMap<Key, Colors>,
    wanted: RefCell<HashSet<Key>>,
    sent: HashSet<Key>,
}

impl Highlighter {
    pub fn new(mut blobs: Blobs) -> Highlighter {
        let (jobs, rx) = mpsc::channel::<(Key, Option<String>)>();
        let (tx, done) = mpsc::channel();
        thread::spawn(move || {
            let syntaxes = SyntaxSet::load_defaults_newlines();
            let theme = &slot_theme();
            for (key, object) in rx {
                let text = object
                    .and_then(|o| blobs.read(&o, MAX_BYTES).ok().flatten())
                    .unwrap_or_default();
                let colors = highlight(&syntaxes, theme, &key.1, &text);
                if tx.send((key, colors)).is_err() {
                    return;
                }
            }
        });
        Highlighter {
            jobs,
            done,
            cache: HashMap::new(),
            wanted: RefCell::default(),
            sent: HashSet::new(),
        }
    }

    pub fn get(&self, rev: &str, path: &str) -> Option<&Colors> {
        let key = (rev.to_string(), path.to_string());
        let colors = self.cache.get(&key);
        if colors.is_none() && !self.sent.contains(&key) {
            self.wanted.borrow_mut().insert(key);
        }
        colors
    }

    pub fn flush(&mut self, repo: &mut Repo) {
        for key in self.wanted.take() {
            let object = repo.blob(&key.0, &key.1).ok();
            self.sent.insert(key.clone());
            let _ = self.jobs.send((key, object));
        }
    }

    pub fn receive(&mut self) -> bool {
        let mut any = false;
        loop {
            match self.done.try_recv() {
                Ok((key, colors)) => {
                    self.cache.insert(key, colors);
                    any = true;
                }
                Err(TryRecvError::Empty) => return any,
                Err(TryRecvError::Disconnected) => {
                    for key in &self.sent {
                        self.cache.entry(key.clone()).or_default();
                    }
                    return true;
                }
            }
        }
    }

    pub fn busy(&self) -> bool {
        self.sent.len() > self.cache.len()
    }
}

fn slot_theme() -> Theme {
    let slot = |s: Slot| syntect::highlighting::Color {
        r: Slot::ALL.iter().position(|a| *a == s).expect("slot in ALL") as u8,
        g: 0,
        b: 0,
        a: 0,
    };
    let item = |scope: &str, s| ThemeItem {
        scope: ScopeSelectors::from_str(scope).expect("scope selector"),
        style: StyleModifier {
            foreground: Some(slot(s)),
            ..StyleModifier::default()
        },
    };
    Theme {
        settings: ThemeSettings {
            foreground: Some(slot(Slot::Text)),
            ..ThemeSettings::default()
        },
        scopes: vec![
            item("comment", Slot::Comment),
            item("string", Slot::Green),
            item("constant", Slot::Peach),
            item("keyword, storage", Slot::Mauve),
            item("entity.name, support.type, support.class", Slot::Teal),
            // More specific than `entity.name`, so they win.
            item(
                "entity.name.function, support.function, support.macro",
                Slot::Blue,
            ),
            item(
                "entity.name.tag, entity.name.section, markup.heading",
                Slot::Blue,
            ),
            item("entity.other.attribute-name", Slot::Yellow),
            item("markup.inserted", Slot::Green),
            item("markup.deleted", Slot::Red),
        ],
        ..Theme::default()
    }
}

fn highlight(syntaxes: &SyntaxSet, theme: &Theme, path: &str, text: &str) -> Colors {
    let path = Path::new(path);
    let syntax = [path.extension(), path.file_name()]
        .into_iter()
        .flatten()
        .find_map(|s| syntaxes.find_syntax_by_extension(&s.to_string_lossy()))
        .or_else(|| syntaxes.find_syntax_by_first_line(text.lines().next().unwrap_or("")));
    let Some(syntax) = syntax else {
        return Vec::new();
    };
    let mut lines = HighlightLines::new(syntax, theme);
    let mut out = Vec::new();
    for line in LinesWithEndings::from(text) {
        if line.len() > MAX_LINE {
            break;
        }
        let Ok(runs) = lines.highlight_line(line, syntaxes) else {
            break;
        };
        out.push(
            runs.into_iter()
                .map(|(s, t)| (Slot::ALL[s.foreground.r as usize], t.len()))
                .collect(),
        );
    }
    out
}
