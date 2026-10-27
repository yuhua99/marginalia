use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::str::FromStr;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

use ratatui::style::Color;
use syntect::easy::HighlightLines;
use syntect::highlighting::{ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::repo::Repo;

/// Larger files are not colored.
const MAX_BYTES: usize = 1 << 20;
/// Coloring stops at the first longer line.
const MAX_LINE: usize = 2000;

/// Per line, runs of (color, byte length).
pub type Colors = Vec<Vec<(Color, usize)>>;
type Key = (String, String);

/// Syntax colors of whole files per (rev, path), computed on a background thread.
pub struct Highlighter {
    jobs: Sender<(Key, String)>,
    done: Receiver<(Key, Colors)>,
    cache: HashMap<Key, Colors>,
    /// Asked for while rendering; read and sent by `flush`.
    wanted: RefCell<HashSet<Key>>,
    sent: HashSet<Key>,
}

impl Highlighter {
    pub fn new() -> Highlighter {
        let (jobs, rx) = mpsc::channel::<(Key, String)>();
        let (tx, done) = mpsc::channel();
        thread::spawn(move || {
            let syntaxes = SyntaxSet::load_defaults_newlines();
            let theme = &ansi_theme();
            for (key, text) in rx {
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

    /// Colors of `path` at `rev`; None until ready, and then they are asked for.
    pub fn get(&self, rev: &str, path: &str) -> Option<&Colors> {
        let key = (rev.to_string(), path.to_string());
        let colors = self.cache.get(&key);
        if colors.is_none() && !self.sent.contains(&key) {
            self.wanted.borrow_mut().insert(key);
        }
        colors
    }

    /// Reads the files asked for and hands them to the background thread.
    /// Colors are optional: a file that can't be read is just not colored.
    pub fn flush(&mut self, repo: &mut Repo) {
        for key in self.wanted.take() {
            let text = repo.read(&key.0, &key.1).ok().flatten().unwrap_or_default();
            self.sent.insert(key.clone());
            let _ = self.jobs.send((key, text));
        }
    }

    /// Takes the finished files; true if any arrived.
    pub fn receive(&mut self) -> bool {
        let mut any = false;
        loop {
            match self.done.try_recv() {
                Ok((key, colors)) => {
                    self.cache.insert(key, colors);
                    any = true;
                }
                Err(TryRecvError::Empty) => return any,
                // The thread died: what it had left goes uncolored.
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

/// Token colors as ANSI indices so they follow the terminal palette: the index in `r`
/// with `a` 0; `a` 1 is the default foreground.
fn ansi_theme() -> Theme {
    let ansi = |r, a| syntect::highlighting::Color { r, g: 0, b: 0, a };
    let item = |scope: &str, n| ThemeItem {
        scope: ScopeSelectors::from_str(scope).expect("scope selector"),
        style: StyleModifier {
            foreground: Some(ansi(n, 0)),
            ..StyleModifier::default()
        },
    };
    Theme {
        settings: ThemeSettings {
            foreground: Some(ansi(0, 1)),
            ..ThemeSettings::default()
        },
        scopes: vec![
            item("comment", 8),
            item("string", 2),
            item("constant", 3),
            item("keyword, storage", 5),
            item("entity.name, support.type, support.class", 6),
            // More specific than `entity.name`, so it wins.
            item("entity.name.function, support.function, support.macro", 4),
            item("entity.name.tag, entity.name.section, markup.heading", 4),
            item("entity.other.attribute-name", 3),
            item("markup.inserted", 2),
            item("markup.deleted", 1),
        ],
        ..Theme::default()
    }
}

fn highlight(syntaxes: &SyntaxSet, theme: &Theme, path: &str, text: &str) -> Colors {
    if text.len() > MAX_BYTES {
        return Vec::new();
    }
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
                .map(|(s, t)| {
                    let color = match s.foreground.a {
                        0 => Color::Indexed(s.foreground.r),
                        _ => Color::Reset,
                    };
                    (color, t.len())
                })
                .collect(),
        );
    }
    out
}
