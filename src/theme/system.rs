//! The `system` theme: the terminal's own colors, asked with OSC 10 / 11 / 4, each moved
//! in OKLab lightness, keeping its hue, until it reads on the backgrounds it is drawn on.

use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use ratatui::crossterm::terminal;

use super::{Palette, Rgb, Theme, mix, palette};

/// WCAG contrast each role needs.
const TEXT: f64 = 4.5;
const COLOR: f64 = 3.0;
const DIM: f64 = 2.5;

/// The ANSI colors asked for: red, green, yellow, blue, magenta, cyan, bright black.
const ANSI: [u8; 7] = [1, 2, 3, 4, 5, 6, 8];

/// The theme from the terminal's colors, or None when it doesn't report them.
pub fn query() -> Option<Theme> {
    // Replies come on the terminal; with stdin elsewhere they would land in the TUI.
    if !std::io::stdin().is_terminal() {
        return None;
    }
    terminal::enable_raw_mode().ok()?;
    let reply = ask();
    terminal::disable_raw_mode().ok()?;
    let reply = reply.ok()?;
    let colors = parse(&reply);
    let ansi: Vec<Rgb> = ANSI
        .iter()
        .map(|n| colors.get(format!("4;{n}").as_str()).copied())
        .collect::<Option<_>>()?;
    Some(build(*colors.get("10")?, *colors.get("11")?, &ansi))
}

/// Sends the queries and reads the replies, up to the reply to DA1, which every
/// terminal answers, or half a second.
fn ask() -> std::io::Result<String> {
    let mut query = String::from("\x1b]10;?\x07\x1b]11;?\x07");
    for n in ANSI {
        query.push_str(&format!("\x1b]4;{n};?\x07"));
    }
    query.push_str("\x1b[c");
    let mut out = std::io::stdout();
    out.write_all(query.as_bytes())?;
    out.flush()?;
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut reply = Vec::new();
    while !answered(&reply) {
        let left = deadline
            .saturating_duration_since(Instant::now())
            .as_millis() as i32;
        let mut fd = libc::pollfd {
            fd: 0,
            events: libc::POLLIN,
            revents: 0,
        };
        // Unbuffered reads, so nothing after the replies is kept from the TUI.
        let mut buf = [0u8; 1024];
        if left == 0 || unsafe { libc::poll(&mut fd, 1, left) } <= 0 {
            break;
        }
        let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            break;
        }
        reply.extend_from_slice(&buf[..n as usize]);
    }
    Ok(String::from_utf8_lossy(&reply).into_owned())
}

/// Whether `reply` holds the DA1 reply, `ESC [ ? ... c`.
fn answered(reply: &[u8]) -> bool {
    reply
        .windows(3)
        .position(|w| w == b"\x1b[?")
        .is_some_and(|i| reply[i..].contains(&b'c'))
}

/// `ESC ] key ; rgb:RR/GG/BB` replies (1 to 4 hex digits a channel), by key: `10`, `11`, `4;N`.
fn parse(reply: &str) -> HashMap<&str, Rgb> {
    reply
        .split("\x1b]")
        .filter_map(|seg| {
            let seg = seg.split(['\x07', '\x1b']).next()?;
            let (key, spec) = seg.split_once(";rgb:")?;
            let mut channels = spec.split('/').map(|h| {
                if h.len() > 4 {
                    return None;
                }
                let v = u32::from_str_radix(h, 16).ok()?;
                Some((v * 255 / ((1 << (4 * h.len())) - 1)) as u8)
            });
            Some((
                key,
                (channels.next()??, channels.next()??, channels.next()??),
            ))
        })
        .collect()
}

fn build(fg: Rgb, bg: Rgb, ansi: &[Rgb]) -> Theme {
    let [red, green, yellow, blue, magenta, cyan, gray] = ansi.try_into().expect("7 colors");
    let dark = luminance(bg) < luminance(fg);
    // Code is drawn on the background and on the tints of changed lines.
    let surfaces = [
        bg,
        mix(readable(red, &[bg], COLOR, dark), bg, 15),
        mix(readable(green, &[bg], COLOR, dark), bg, 15),
    ];
    let code = |c, min| readable(c, &surfaces, min, dark);
    palette(
        "system",
        Palette {
            bg,
            text: code(fg, TEXT),
            dim: readable(gray, &[bg], DIM, dark),
            comment: code(gray, DIM),
            accent: readable(yellow, &[bg], COLOR, dark),
            red: code(red, COLOR),
            green: code(green, COLOR),
            yellow: code(yellow, COLOR),
            blue: code(blue, COLOR),
            mauve: code(magenta, COLOR),
            teal: code(cyan, COLOR),
            peach: code(yellow, COLOR),
        },
    )
}

/// `c`, made lighter on dark backgrounds or darker on light ones until it has `min`
/// contrast on each of `on`; as it is when it already has.
fn readable(c: Rgb, on: &[Rgb], min: f64, dark: bool) -> Rgb {
    let ok = |c: Rgb| on.iter().all(|s| contrast(c, *s) >= min);
    if ok(c) {
        return c;
    }
    let [l, a, b] = oklab(c);
    let (mut bad, mut good) = (l, if dark { 1.0 } else { 0.0 });
    for _ in 0..16 {
        let mid = (bad + good) / 2.0;
        if ok(from_oklab([mid, a, b])) {
            good = mid;
        } else {
            bad = mid;
        }
    }
    from_oklab([good, a, b])
}

fn linear(c: u8) -> f64 {
    let c = c as f64 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn gamma(c: f64) -> u8 {
    let c = if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (c.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn luminance(c: Rgb) -> f64 {
    0.2126 * linear(c.0) + 0.7152 * linear(c.1) + 0.0722 * linear(c.2)
}

/// WCAG 2 contrast ratio, 1 to 21.
fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

fn oklab(c: Rgb) -> [f64; 3] {
    let (r, g, b) = (linear(c.0), linear(c.1), linear(c.2));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

/// The color at OKLab `[l, a, b]`, its chroma lowered until it fits in sRGB.
fn from_oklab([l, a, b]: [f64; 3]) -> Rgb {
    let mut k = 1.0;
    loop {
        let (a, b) = (a * k, b * k);
        let lc = (l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
        let mc = (l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
        let sc = (l - 0.0894841775 * a - 1.2914855480 * b).powi(3);
        let rgb = [
            4.0767416621 * lc - 3.3077115913 * mc + 0.2309699292 * sc,
            -1.2684380046 * lc + 2.6097574011 * mc - 0.3413193965 * sc,
            -0.0041960863 * lc - 0.7034186147 * mc + 1.7076147010 * sc,
        ];
        if k < 0.01 || rgb.iter().all(|c| (-0.0001..=1.0001).contains(c)) {
            return (gamma(rgb[0]), gamma(rgb[1]), gamma(rgb[2]));
        }
        k *= 0.9;
    }
}
