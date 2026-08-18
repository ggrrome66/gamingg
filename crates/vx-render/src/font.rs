//! A tiny procedural bitmap font.
//!
//! Five by seven pixels per glyph, hand-set as bit patterns — original work,
//! like every other placeholder asset in the engine. Uppercase letters,
//! digits and enough punctuation for a HUD. Lowercase input is drawn with the
//! uppercase shapes rather than rejected, because a HUD that panics over a
//! lowercase letter is a HUD nobody trusts.
//!
//! `draw_text` stamps straight into any RGBA byte buffer, which is exactly
//! the kind of buffer the minimap already composites — the overlay pass
//! neither knows nor cares that some pixels used to be letters.

/// Glyph cell width in pixels, including no spacing.
pub const GLYPH_WIDTH: u32 = 5;
/// Glyph cell height in pixels.
pub const GLYPH_HEIGHT: u32 = 7;
/// Horizontal advance per character (one blank column between glyphs).
pub const ADVANCE: u32 = GLYPH_WIDTH + 1;
/// Suggested line spacing.
pub const LINE_HEIGHT: u32 = GLYPH_HEIGHT + 2;

/// One glyph: seven rows of five bits, most significant bit leftmost.
type Glyph = [u8; 7];

/// The shape drawn for characters the font does not know: a filled box, so a
/// missing glyph is visible rather than silently absent.
const UNKNOWN: Glyph = [0b11111, 0b10001, 0b10101, 0b10101, 0b10101, 0b10001, 0b11111];

/// Does the font have a real shape for this character?
///
/// Panels are built from formatted strings, and a character the font has
/// never heard of draws as [`UNKNOWN`] — a filled box that reads as damage.
/// Callers building labels can assert against this rather than eyeballing a
/// capture.
pub fn knows(character: char) -> bool {
    glyph(character) != UNKNOWN
}

/// Look up a character's pattern.
pub fn glyph(character: char) -> Glyph {
    let upper = character.to_ascii_uppercase();
    match upper {
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110],
        ' ' => [0, 0, 0, 0, 0, 0, 0],
        '.' => [0, 0, 0, 0, 0, 0b00100, 0b00100],
        ',' => [0, 0, 0, 0, 0b00100, 0b00100, 0b01000],
        ':' => [0, 0b00100, 0b00100, 0, 0b00100, 0b00100, 0],
        '%' => [0b11001, 0b11010, 0b00010, 0b00100, 0b01000, 0b01011, 0b10011],
        '/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        '-' => [0, 0, 0, 0b01110, 0, 0, 0],
        '+' => [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0, 0b00100],
        '?' => [0b01110, 0b10001, 0b00001, 0b00110, 0b00100, 0, 0b00100],
        '\'' => [0b00100, 0b00100, 0b01000, 0, 0, 0, 0],
        '(' => [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010],
        ')' => [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000],
        '[' => [0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110],
        ']' => [0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110],
        '>' => [0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000],
        '<' => [0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010],
        '=' => [0, 0, 0b01110, 0, 0b01110, 0, 0],
        _ => UNKNOWN,
    }
}

/// Stamp `text` into a tightly-packed RGBA buffer of row length `stride`
/// pixels, top-left of the first glyph at `(x, y)`, `scale` pixels per font
/// pixel. Pixels outside the buffer are skipped, so text can safely run off
/// an edge.
pub fn draw_text(
    pixels: &mut [u8],
    stride: u32,
    x: i32,
    y: i32,
    scale: u32,
    colour: [u8; 4],
    text: &str,
) {
    let rows = pixels.len() as u32 / 4 / stride.max(1);
    let mut pen_x = x;

    for character in text.chars() {
        let pattern = glyph(character);
        for (row, bits) in pattern.iter().enumerate() {
            for column in 0..GLYPH_WIDTH {
                if bits & (1 << (GLYPH_WIDTH - 1 - column)) == 0 {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let px = pen_x + (column * scale + sx) as i32;
                        let py = y + (row as u32 * scale + sy) as i32;
                        if px < 0 || py < 0 || px as u32 >= stride || py as u32 >= rows {
                            continue;
                        }
                        let at = ((py as u32 * stride + px as u32) * 4) as usize;
                        pixels[at..at + 4].copy_from_slice(&colour);
                    }
                }
            }
        }
        pen_x += (ADVANCE * scale) as i32;
    }
}

/// Pixel width of `text` at `scale` — for right-aligning and panel sizing.
pub fn text_width(text: &str, scale: u32) -> u32 {
    (text.chars().count() as u32) * ADVANCE * scale
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(width: u32, height: u32) -> Vec<u8> {
        vec![0u8; (width * height * 4) as usize]
    }

    fn lit_pixels(pixels: &[u8]) -> usize {
        pixels.chunks_exact(4).filter(|texel| texel[3] != 0).count()
    }

    #[test]
    fn every_declared_glyph_fits_five_by_seven() {
        // A stray sixth bit would bleed into the neighbouring glyph cell.
        for character in
            "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 .,:%/-+!?'()[]<>=".chars()
        {
            for (row, bits) in glyph(character).iter().enumerate() {
                assert!(
                    bits & !0b11111 == 0,
                    "{character:?} row {row} uses bits outside the 5-wide cell"
                );
            }
        }
    }

    #[test]
    fn drawing_is_deterministic_and_actually_draws() {
        let mut a = buffer(64, 16);
        let mut b = buffer(64, 16);
        draw_text(&mut a, 64, 1, 1, 1, [255; 4], "MIN 12");
        draw_text(&mut b, 64, 1, 1, 1, [255; 4], "MIN 12");
        assert_eq!(a, b);
        assert!(lit_pixels(&a) > 30, "the text barely drew anything");
    }

    #[test]
    fn distinct_strings_draw_distinct_pixels() {
        let mut a = buffer(64, 16);
        let mut b = buffer(64, 16);
        draw_text(&mut a, 64, 1, 1, 1, [255; 4], "LEVEL 10");
        draw_text(&mut b, 64, 1, 1, 1, [255; 4], "LEVEL 11");
        assert_ne!(a, b);
    }

    #[test]
    fn lowercase_draws_as_uppercase_rather_than_placeholder() {
        let mut lower = buffer(64, 16);
        let mut upper = buffer(64, 16);
        draw_text(&mut lower, 64, 0, 0, 1, [255; 4], "ore");
        draw_text(&mut upper, 64, 0, 0, 1, [255; 4], "ORE");
        assert_eq!(lower, upper);
    }

    #[test]
    fn unknown_characters_draw_the_placeholder_box_not_nothing() {
        let mut pixels = buffer(16, 16);
        draw_text(&mut pixels, 16, 0, 0, 1, [255; 4], "\u{263A}");
        // The placeholder's border alone is 20 pixels.
        assert!(lit_pixels(&pixels) >= 16, "an unknown glyph vanished silently");
    }

    #[test]
    fn text_off_the_edge_clips_instead_of_panicking() {
        let mut pixels = buffer(16, 8);
        draw_text(&mut pixels, 16, -3, -2, 1, [255; 4], "CLIPPED TEXT WAY TOO LONG");
        draw_text(&mut pixels, 16, 14, 6, 2, [255; 4], "MORE");
        // Reaching here without a panic is the point; and something landed.
        assert!(lit_pixels(&pixels) > 0);
    }

    #[test]
    fn scale_multiplies_the_footprint() {
        let mut small = buffer(32, 16);
        let mut big = buffer(32, 16);
        draw_text(&mut small, 32, 0, 0, 1, [255; 4], "A");
        draw_text(&mut big, 32, 0, 0, 2, [255; 4], "A");
        assert_eq!(lit_pixels(&big), lit_pixels(&small) * 4);
    }

    #[test]
    fn width_matches_the_advance() {
        assert_eq!(text_width("ABC", 1), 3 * ADVANCE);
        assert_eq!(text_width("ABC", 2), 6 * ADVANCE);
        assert_eq!(text_width("", 3), 0);
    }
}
