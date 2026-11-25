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

#[derive(Default)]
pub struct Outline(Vec<Option<(usize, Option<String>)>>);

impl Outline {
    pub fn scope(&self, line: u32) -> Vec<&str> {
        if line as usize > self.0.len() {
            return Vec::new();
        }
        let mut indent = usize::MAX;
        let mut out = Vec::new();
        for (i, name) in self.0.iter().take(line as usize).rev().flatten() {
            if *i < indent {
                indent = *i;
                out.extend(name.as_deref());
            }
            if indent == 0 {
                break;
            }
        }
        out.reverse();
        out
    }
}

type Key = (String, String);

pub struct Highlighter {
    jobs: Sender<(Key, Option<String>)>,
    done: Receiver<(Key, (Colors, Outline))>,
    cache: HashMap<Key, (Colors, Outline)>,
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
                let done = highlight(&syntaxes, theme, &key.1, &text);
                if tx.send((key, done)).is_err() {
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
        self.entry(rev, path).map(|e| &e.0)
    }

    pub fn outline(&self, rev: &str, path: &str) -> Option<&Outline> {
        self.entry(rev, path).map(|e| &e.1)
    }

    fn entry(&self, rev: &str, path: &str) -> Option<&(Colors, Outline)> {
        let key = (rev.to_string(), path.to_string());
        let entry = self.cache.get(&key);
        if entry.is_none() && !self.sent.contains(&key) {
            self.wanted.borrow_mut().insert(key);
        }
        entry
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
                Ok((key, done)) => {
                    self.cache.insert(key, done);
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
    let slot = |s: Slot, def: bool| syntect::highlighting::Color {
        r: Slot::ALL.iter().position(|a| *a == s).expect("slot in ALL") as u8,
        g: def as u8,
        b: 0,
        a: 0,
    };
    let styled = |scope: &str, s, def| ThemeItem {
        scope: ScopeSelectors::from_str(scope).expect("scope selector"),
        style: StyleModifier {
            foreground: Some(slot(s, def)),
            ..StyleModifier::default()
        },
    };
    let item = |scope, s| styled(scope, s, false);
    let def = |scope, s| styled(scope, s, true);
    Theme {
        settings: ThemeSettings {
            foreground: Some(slot(Slot::Text, false)),
            ..ThemeSettings::default()
        },
        scopes: vec![
            item("comment", Slot::Comment),
            item("string", Slot::Green),
            item("constant", Slot::Peach),
            item("keyword, storage", Slot::Mauve),
            item("support.type, support.class", Slot::Teal),
            def("entity.name", Slot::Teal),
            // More specific than `entity.name`, so they win.
            def("entity.name.function", Slot::Blue),
            item("support.function, support.macro", Slot::Blue),
            item("entity.name.tag, markup.heading", Slot::Blue),
            def("entity.name.section", Slot::Blue),
            item("entity.other.attribute-name", Slot::Yellow),
            item("markup.inserted", Slot::Green),
            item("markup.deleted", Slot::Red),
        ],
        ..Theme::default()
    }
}

fn highlight(syntaxes: &SyntaxSet, theme: &Theme, path: &str, text: &str) -> (Colors, Outline) {
    let path = Path::new(path);
    let syntax = [path.extension(), path.file_name()]
        .into_iter()
        .flatten()
        .find_map(|s| syntaxes.find_syntax_by_extension(&s.to_string_lossy()))
        .or_else(|| syntaxes.find_syntax_by_first_line(text.lines().next().unwrap_or("")));
    let Some(syntax) = syntax else {
        return Default::default();
    };
    let mut lines = HighlightLines::new(syntax, theme);
    let mut out = Vec::new();
    let mut outline = Vec::new();
    for line in LinesWithEndings::from(text) {
        if line.len() > MAX_LINE {
            break;
        }
        let Ok(runs) = lines.highlight_line(line, syntaxes) else {
            break;
        };
        let (mut end, mut def) = (0, None);
        out.push(
            runs.into_iter()
                .map(|(s, t)| {
                    end += t.len();
                    if s.foreground.g == 1 {
                        def = Some(end);
                    }
                    (Slot::ALL[s.foreground.r as usize], t.len())
                })
                .collect(),
        );
        let body = line.trim_start();
        let indent = line.len() - body.len();
        let opens = !body.trim_end().is_empty() && !body.starts_with([')', '}', ']', '{']);
        outline.push(opens.then(|| {
            let name = def.and_then(|d| line.get(indent..d)).map(str::to_string);
            (indent, name)
        }));
    }
    (out, Outline(outline))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(path: &str, text: &str, line: u32) -> Vec<String> {
        let syntaxes = SyntaxSet::load_defaults_newlines();
        let (_, outline) = highlight(&syntaxes, &slot_theme(), path, text);
        outline
            .scope(line)
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn scope_skips_blocks_and_closers() {
        let rust = "impl Bar {\n    fn render(\n        &self,\n    ) -> u32 {\n        if x {\n            y();\n        }\n\n    }\n}\n";
        assert_eq!(scope("a.rs", rust, 6), ["impl Bar", "fn render"]);
        assert_eq!(scope("a.rs", rust, 9), ["impl Bar", "fn render"]);
        assert_eq!(scope("a.rs", rust, 1), ["impl Bar"]);
        let python = "class Order:\n    def total(self):\n        return 1\n";
        assert_eq!(scope("a.py", python, 3), ["class Order", "def total"]);
        assert!(scope("a.txt", "a\n  b\n", 2).is_empty());
        assert_eq!(scope("a.c", "void f()\n{\n    g();\n}\n", 3), ["void f"]);
    }
}
