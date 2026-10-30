//! The `system` theme: the terminal's own colors, asked with OSC 10 / 11 / 4, made a
//! palette as the RGB themes are.

use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use ratatui::crossterm::terminal;

use super::{Palette, Rgb, Theme, palette};

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
    palette(
        "system",
        Palette {
            bg,
            text: fg,
            dim: gray,
            comment: gray,
            accent: yellow,
            red,
            green,
            yellow,
            blue,
            mauve: magenta,
            teal: cyan,
            peach: yellow,
        },
    )
}
