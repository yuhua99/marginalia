//! Colors made readable: moved in OKLab lightness, keeping their hue, until they have
//! the WCAG contrast their role needs on the backgrounds they are drawn on.

use super::Rgb;

/// WCAG contrast each role needs.
pub const TEXT: f64 = 4.5;
pub const COLOR: f64 = 3.0;
pub const DIM: f64 = 2.5;

/// `c`, made lighter on dark backgrounds or darker on light ones until it has `min`
/// contrast on each of `on`; as it is when it already has.
pub fn readable(c: Rgb, on: &[Rgb], min: f64, dark: bool) -> Rgb {
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

pub fn luminance(c: Rgb) -> f64 {
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
