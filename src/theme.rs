use std::sync::OnceLock;

use ratatui::style::Color;

mod system;

/// A palette color by role. Syntax tokens are stored as slots, so switching
/// themes needs no re-coloring.
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

/// Colors of the UI and of syntax tokens.
#[derive(Clone)]
pub struct Theme {
    pub name: &'static str,
    /// The screen background.
    pub bg: Color,
    /// Default text.
    pub text: Color,
    /// Muted text: hints, line numbers, resolved threads.
    pub dim: Color,
    /// Code comments, as in helix's theme of the same name.
    pub comment: Color,
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
            Slot::Comment => self.comment,
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

/// Built-in themes; the first is the default. Palettes as in herdr.
/// system, then the dark themes, then the light ones.
static THEMES: &[Theme] = &[
    // The terminal draws its ANSI colors; `themes()` replaces it when the terminal
    // reports its colors.
    Theme {
        name: "system",
        bg: Color::Reset,
        text: Color::Reset,
        dim: Color::DarkGray,
        comment: Color::DarkGray,
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
    },
    palette(
        "catppuccin",
        Palette {
            bg: (24, 24, 37),
            text: (205, 214, 244),
            dim: (108, 112, 134),
            comment: (147, 153, 178),
            accent: (137, 180, 250),
            red: (243, 139, 168),
            green: (166, 227, 161),
            yellow: (249, 226, 175),
            blue: (137, 180, 250),
            mauve: (203, 166, 247),
            teal: (148, 226, 213),
            peach: (250, 179, 135),
        },
    ),
    palette(
        "tokyo-night",
        Palette {
            bg: (26, 27, 38),
            text: (192, 202, 245),
            dim: (86, 95, 137),
            comment: (86, 95, 137),
            accent: (122, 162, 247),
            red: (247, 118, 142),
            green: (158, 206, 106),
            yellow: (224, 175, 104),
            blue: (122, 162, 247),
            mauve: (187, 154, 247),
            teal: (125, 207, 255),
            peach: (255, 158, 100),
        },
    ),
    palette(
        "dracula",
        Palette {
            bg: (40, 42, 54),
            text: (248, 248, 242),
            dim: (98, 114, 164),
            comment: (98, 114, 164),
            accent: (189, 147, 249),
            red: (255, 85, 85),
            green: (80, 250, 123),
            yellow: (241, 250, 140),
            blue: (139, 233, 253),
            mauve: (255, 121, 198),
            teal: (139, 233, 253),
            peach: (255, 184, 108),
        },
    ),
    palette(
        "nord",
        Palette {
            bg: (46, 52, 64),
            text: (236, 239, 244),
            dim: (76, 86, 106),
            comment: (97, 110, 136),
            accent: (136, 192, 208),
            red: (191, 97, 106),
            green: (163, 190, 140),
            yellow: (235, 203, 139),
            blue: (129, 161, 193),
            mauve: (180, 142, 173),
            teal: (143, 188, 187),
            peach: (208, 135, 112),
        },
    ),
    palette(
        "gruvbox",
        Palette {
            bg: (40, 40, 40),
            text: (235, 219, 178),
            dim: (146, 131, 116),
            comment: (146, 131, 116),
            accent: (215, 153, 33),
            red: (251, 73, 52),
            green: (184, 187, 38),
            yellow: (250, 189, 47),
            blue: (131, 165, 152),
            mauve: (211, 134, 155),
            teal: (142, 192, 124),
            peach: (254, 128, 25),
        },
    ),
    palette(
        "one-dark",
        Palette {
            bg: (40, 44, 52),
            text: (171, 178, 191),
            dim: (92, 99, 112),
            comment: (92, 99, 112),
            accent: (97, 175, 239),
            red: (224, 108, 117),
            green: (152, 195, 121),
            yellow: (229, 192, 123),
            blue: (97, 175, 239),
            mauve: (198, 120, 221),
            teal: (86, 182, 194),
            peach: (209, 154, 102),
        },
    ),
    palette(
        "solarized",
        Palette {
            bg: (0, 43, 54),
            text: (147, 161, 161),
            dim: (88, 110, 117),
            comment: (88, 110, 117),
            accent: (38, 139, 210),
            red: (220, 50, 47),
            green: (133, 153, 0),
            yellow: (181, 137, 0),
            blue: (38, 139, 210),
            mauve: (211, 54, 130),
            teal: (42, 161, 152),
            peach: (203, 75, 22),
        },
    ),
    palette(
        "kanagawa",
        Palette {
            bg: (31, 31, 40),
            text: (220, 215, 186),
            dim: (114, 113, 105),
            comment: (114, 113, 105),
            accent: (126, 156, 216),
            red: (195, 64, 67),
            green: (118, 148, 106),
            yellow: (192, 163, 110),
            blue: (126, 156, 216),
            mauve: (149, 127, 184),
            teal: (127, 180, 202),
            peach: (255, 160, 102),
        },
    ),
    palette(
        "rose-pine",
        Palette {
            bg: (25, 23, 36),
            text: (224, 222, 244),
            dim: (110, 106, 134),
            comment: (110, 106, 134),
            accent: (196, 167, 231),
            red: (235, 111, 146),
            green: (49, 116, 143),
            yellow: (246, 193, 119),
            blue: (49, 116, 143),
            mauve: (196, 167, 231),
            teal: (156, 207, 216),
            peach: (234, 154, 151),
        },
    ),
    palette(
        "vesper",
        Palette {
            bg: (26, 26, 26),
            text: (255, 255, 255),
            dim: (92, 92, 92),
            comment: (139, 139, 139),
            accent: (255, 199, 153),
            red: (255, 128, 128),
            green: (153, 255, 228),
            yellow: (255, 199, 153),
            blue: (176, 176, 176),
            mauve: (255, 209, 168),
            teal: (102, 221, 204),
            peach: (255, 199, 153),
        },
    ),
    palette(
        "catppuccin-latte",
        Palette {
            bg: (239, 241, 245),
            text: (76, 79, 105),
            dim: (156, 160, 176),
            comment: (124, 127, 147),
            accent: (30, 102, 245),
            red: (210, 15, 57),
            green: (64, 160, 43),
            yellow: (223, 142, 29),
            blue: (30, 102, 245),
            mauve: (136, 57, 239),
            teal: (23, 146, 153),
            peach: (254, 100, 11),
        },
    ),
    palette(
        "tokyo-night-day",
        Palette {
            bg: (225, 226, 231),
            text: (55, 96, 191),
            dim: (137, 144, 179),
            comment: (132, 140, 181),
            accent: (46, 125, 233),
            red: (245, 42, 101),
            green: (88, 117, 57),
            yellow: (140, 108, 62),
            blue: (46, 125, 233),
            mauve: (120, 71, 189),
            teal: (17, 140, 116),
            peach: (177, 92, 0),
        },
    ),
    palette(
        "gruvbox-light",
        Palette {
            bg: (251, 241, 199),
            text: (60, 56, 54),
            dim: (146, 131, 116),
            comment: (146, 131, 116),
            accent: (7, 102, 120),
            red: (157, 0, 6),
            green: (121, 116, 14),
            yellow: (181, 118, 20),
            blue: (7, 102, 120),
            mauve: (143, 63, 113),
            teal: (66, 123, 88),
            peach: (175, 58, 3),
        },
    ),
    palette(
        "one-light",
        Palette {
            bg: (250, 250, 250),
            text: (56, 58, 66),
            dim: (160, 161, 167),
            comment: (92, 99, 112),
            accent: (64, 120, 242),
            red: (228, 86, 73),
            green: (80, 161, 79),
            yellow: (193, 132, 1),
            blue: (64, 120, 242),
            mauve: (166, 38, 164),
            teal: (1, 132, 188),
            peach: (152, 104, 1),
        },
    ),
    palette(
        "solarized-light",
        Palette {
            bg: (253, 246, 227),
            text: (101, 123, 131),
            dim: (147, 161, 161),
            comment: (147, 161, 161),
            accent: (38, 139, 210),
            red: (220, 50, 47),
            green: (133, 153, 0),
            yellow: (181, 137, 0),
            blue: (38, 139, 210),
            mauve: (211, 54, 130),
            teal: (42, 161, 152),
            peach: (203, 75, 22),
        },
    ),
    palette(
        "kanagawa-lotus",
        Palette {
            bg: (242, 236, 188),
            text: (84, 84, 100),
            dim: (160, 156, 172),
            comment: (143, 144, 156),
            accent: (77, 105, 155),
            red: (200, 64, 83),
            green: (111, 137, 78),
            yellow: (119, 113, 63),
            blue: (77, 105, 155),
            mauve: (98, 76, 131),
            teal: (78, 140, 162),
            peach: (204, 109, 0),
        },
    ),
    palette(
        "rose-pine-dawn",
        Palette {
            bg: (250, 244, 237),
            text: (70, 66, 97),
            dim: (152, 147, 165),
            comment: (152, 147, 165),
            accent: (144, 122, 169),
            red: (180, 99, 122),
            green: (40, 105, 131),
            yellow: (234, 157, 52),
            blue: (40, 105, 131),
            mauve: (144, 122, 169),
            teal: (86, 148, 159),
            peach: (215, 130, 126),
        },
    ),
];

/// The built-in themes, `system` from the terminal's colors when it reports them.
/// The first call asks the terminal, so it comes before the TUI starts.
pub fn themes() -> &'static [Theme] {
    static ALL: OnceLock<Vec<Theme>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut all = THEMES.to_vec();
        if let Some(system) = system::query() {
            all[0] = system;
        }
        all
    })
}

/// The theme named `name`, or the default.
pub fn find(name: &str) -> &'static Theme {
    let themes = themes();
    themes.iter().find(|t| t.name == name).unwrap_or(&themes[0])
}

type Rgb = (u8, u8, u8);

/// The colors an RGB theme is made of.
struct Palette {
    bg: Rgb,
    text: Rgb,
    dim: Rgb,
    comment: Rgb,
    accent: Rgb,
    red: Rgb,
    green: Rgb,
    yellow: Rgb,
    blue: Rgb,
    mauve: Rgb,
    teal: Rgb,
    peach: Rgb,
}

/// Changed lines tint the background with red / green; changed words tint it more.
const fn palette(name: &'static str, p: Palette) -> Theme {
    Theme {
        name,
        bg: rgb(p.bg),
        text: rgb(p.text),
        dim: rgb(p.dim),
        comment: rgb(p.comment),
        accent: rgb(p.accent),
        on_accent: rgb(p.bg),
        red: rgb(p.red),
        green: rgb(p.green),
        yellow: rgb(p.yellow),
        blue: rgb(p.blue),
        mauve: rgb(p.mauve),
        teal: rgb(p.teal),
        peach: rgb(p.peach),
        del_bg: rgb(mix(p.red, p.bg, 15)),
        add_bg: rgb(mix(p.green, p.bg, 15)),
        del_emph: rgb(mix(p.red, p.bg, 45)),
        add_emph: rgb(mix(p.green, p.bg, 45)),
    }
}

const fn rgb(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

/// `pct` percent of `a` over `b`.
const fn mix(a: Rgb, b: Rgb, pct: u16) -> Rgb {
    const fn channel(a: u8, b: u8, pct: u16) -> u8 {
        ((a as u16 * pct + b as u16 * (100 - pct)) / 100) as u8
    }
    (
        channel(a.0, b.0, pct),
        channel(a.1, b.1, pct),
        channel(a.2, b.2, pct),
    )
}
