use ratatui::style::Color;

/// A palette color by role. Syntax tokens are stored as slots, so switching
/// themes needs no re-coloring.
#[derive(Clone, Copy, PartialEq)]
pub enum Slot {
    Text,
    Dim,
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
        Slot::Dim,
        Slot::Red,
        Slot::Green,
        Slot::Yellow,
        Slot::Blue,
        Slot::Mauve,
        Slot::Teal,
        Slot::Peach,
    ];
}

/// Colors of the UI and of syntax tokens.
pub struct Theme {
    /// Default text.
    pub text: Color,
    /// Muted text: hints, line numbers, comments, resolved threads.
    pub dim: Color,
    /// Focused borders, marks, prompts.
    pub accent: Color,
    /// Text on accent backgrounds.
    pub on_accent: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub blue: Color,
    pub mauve: Color,
    pub teal: Color,
    pub peach: Color,
    /// Backgrounds of deleted / added lines, and of the changed words in them.
    pub del_bg: Color,
    pub add_bg: Color,
    pub del_emph: Color,
    pub add_emph: Color,
}

impl Theme {
    pub fn slot(&self, slot: Slot) -> Color {
        match slot {
            Slot::Text => self.text,
            Slot::Dim => self.dim,
            Slot::Red => self.red,
            Slot::Green => self.green,
            Slot::Yellow => self.yellow,
            Slot::Blue => self.blue,
            Slot::Mauve => self.mauve,
            Slot::Teal => self.teal,
            Slot::Peach => self.peach,
        }
    }
}

/// The terminal's own palette.
pub const ANSI: Theme = Theme {
    text: Color::Reset,
    dim: Color::DarkGray,
    accent: Color::Yellow,
    on_accent: Color::Black,
    red: Color::Red,
    green: Color::Green,
    yellow: Color::Yellow,
    blue: Color::Blue,
    mauve: Color::Magenta,
    teal: Color::Cyan,
    peach: Color::Yellow,
    del_bg: Color::Red,
    add_bg: Color::Green,
    del_emph: Color::LightRed,
    add_emph: Color::LightGreen,
};
