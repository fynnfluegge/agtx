//! The pairing URL as a QR code, for a terminal.
//!
//! The alternative is typing `http://192.168.178.26:8787/#token=<48 hex chars>`
//! into a phone, which nobody should be asked to do twice.

use qrcode::{EcLevel, QrCode};

/// Two module rows per character cell.
///
/// A terminal cell is about twice as tall as it is wide, so one module per cell
/// gives a QR stretched 2:1 — scannable by a patient phone, but only just.
/// `▀` splits a cell into an upper and a lower pixel, coloured independently,
/// which restores square modules and halves the height. A version-5 code then
/// fits in ~23 rows instead of ~45.
const UPPER_HALF: char = '▀';

/// White, then black, as basic ANSI rather than truecolor.
///
/// The colours are set explicitly rather than left to the terminal because a QR
/// has to be dark-on-light to scan reliably, and half of all terminals are
/// dark-on-light the other way round. Basic ANSI (not 24-bit) because this only
/// needs two colours and every terminal has had these since the 1980s.
const FG_LIGHT: &str = "\x1b[97m";
const FG_DARK: &str = "\x1b[30m";
const BG_LIGHT: &str = "\x1b[107m";
const BG_DARK: &str = "\x1b[40m";
const RESET: &str = "\x1b[0m";

/// Modules of light border around the code.
///
/// Four is the specified minimum quiet zone. Dropping it is the classic reason
/// a QR that "looks fine" will not scan: without the border a decoder cannot
/// find the code's edges against whatever is printed around it.
const QUIET: usize = 4;

/// The module grid, including its quiet zone.
///
/// Returns `(width, dark)` where `dark[y * width + x]` is true for a dark
/// module. Separate from [`render`] because the two consumers need different
/// output and neither should re-derive the layout: the CLI banner wants ANSI
/// escapes, while a ratatui overlay needs styled spans and would draw those
/// escapes as literal garbage.
pub fn grid(data: &str) -> Option<(usize, Vec<bool>)> {
    // Low error correction: this is displayed on a screen a few inches from the
    // camera, not printed on a box that might get scuffed. Lower correction
    // means fewer modules, which means a smaller code in a terminal where
    // vertical space is the scarce thing.
    let code = QrCode::with_error_correction_level(data, EcLevel::L).ok()?;
    let width = code.width();
    let modules: Vec<bool> = code
        .into_colors()
        .iter()
        .map(|c| *c == qrcode::Color::Dark)
        .collect();

    let total = width + QUIET * 2;
    let mut padded = vec![false; total * total];
    for y in 0..width {
        for x in 0..width {
            padded[(y + QUIET) * total + (x + QUIET)] = modules[y * width + x];
        }
    }
    Some((total, padded))
}

/// Render `data` as a QR code using half-block characters and ANSI colour.
///
/// For a terminal that interprets escapes — the `agtx serve` banner. A ratatui
/// widget must use [`grid`] instead.
///
/// `None` when the data will not fit any QR version, which for a URL means
/// something has gone very wrong upstream — a caller should fall back to
/// printing the URL rather than treating it as fatal.
pub fn render(data: &str) -> Option<String> {
    // Low error correction: this is displayed on a screen a few inches from the
    // camera, not printed on a box that might get scuffed. Lower correction
    // means fewer modules, which means a smaller code in a terminal where
    // vertical space is the scarce thing.
    let (total, dark) = grid(data)?;
    let dark_at = |x: usize, y: usize| -> bool { dark[y * total + x] };

    let mut out = String::new();
    // Two module rows per line; an odd final row pairs with a light one, which
    // is what the quiet zone would have been anyway.
    for row in (0..total).step_by(2) {
        for x in 0..total {
            let upper = dark_at(x, row);
            let lower = row + 1 < total && dark_at(x, row + 1);
            out.push_str(if upper { FG_DARK } else { FG_LIGHT });
            out.push_str(if lower { BG_DARK } else { BG_LIGHT });
            out.push(UPPER_HALF);
        }
        out.push_str(RESET);
        out.push('\n');
    }
    Some(out)
}
