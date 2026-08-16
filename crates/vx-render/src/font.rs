//! A built-in bitmap font.
//!
//! The HUD needs text before it needs anything else, and pulling in a font
//! rasteriser plus a licensed typeface for what amounts to a debug readout is a
//! poor trade. This is a 5×7 pixel face defined in the source, drawn into an
//! 8×8 cell grid at startup.
//!
//! Glyphs are written as art rather than hex so they can be read and corrected
//! in place — a wrong bit in a hex table is invisible until it renders.

/// Cell size in the atlas, in pixels. The glyph occupies 5×7 of it and the
/// remainder is the gap that keeps neighbouring characters from touching.
pub const CELL: u32 = 8;
/// Drawn width of a glyph within its cell.
pub const GLYPH_WIDTH: u32 = 5;
/// Drawn height of a glyph within its cell.
pub const GLYPH_HEIGHT: u32 = 7;

/// Cells per atlas row.
pub const COLUMNS: u32 = 16;
/// First character in the table.
pub const FIRST_CHAR: u8 = b' ';
/// Last character in the table.
pub const LAST_CHAR: u8 = b'_';

/// Number of glyphs defined.
pub const GLYPH_COUNT: usize = (LAST_CHAR - FIRST_CHAR + 1) as usize;

pub const ATLAS_WIDTH: u32 = COLUMNS * CELL;
pub const ATLAS_HEIGHT: u32 = (GLYPH_COUNT as u32).div_ceil(COLUMNS) * CELL;

/// Every glyph from space to underscore, in ASCII order.
///
/// Uppercase only, which is both period-correct for a terminal face and half
/// the table to maintain. [`glyph_index`] folds lowercase onto these.
const GLYPHS: [[&str; 7]; GLYPH_COUNT] = [
    // space
    ["     ", "     ", "     ", "     ", "     ", "     ", "     "],
    // !
    ["  #  ", "  #  ", "  #  ", "  #  ", "  #  ", "     ", "  #  "],
    // "
    [" # # ", " # # ", "     ", "     ", "     ", "     ", "     "],
    // #
    [" # # ", " # # ", "#####", " # # ", "#####", " # # ", " # # "],
    // $
    ["  #  ", " ####", "# #  ", " ### ", "  # #", "#### ", "  #  "],
    // %
    ["##   ", "##  #", "   # ", "  #  ", " #   ", "#  ##", "   ##"],
    // &
    [" ##  ", "#  # ", "#  # ", " ##  ", "#  ##", "#  # ", " ## #"],
    // '
    ["  #  ", "  #  ", "     ", "     ", "     ", "     ", "     "],
    // (
    ["   # ", "  #  ", " #   ", " #   ", " #   ", "  #  ", "   # "],
    // )
    [" #   ", "  #  ", "   # ", "   # ", "   # ", "  #  ", " #   "],
    // *
    ["     ", "# # #", " ### ", "#####", " ### ", "# # #", "     "],
    // +
    ["     ", "  #  ", "  #  ", "#####", "  #  ", "  #  ", "     "],
    // ,
    ["     ", "     ", "     ", "     ", "  ## ", "  ## ", "  #  "],
    // -
    ["     ", "     ", "     ", "#####", "     ", "     ", "     "],
    // .
    ["     ", "     ", "     ", "     ", "     ", "  ## ", "  ## "],
    // /
    ["    #", "   # ", "   # ", "  #  ", " #   ", " #   ", "#    "],
    // 0
    [" ### ", "#   #", "#  ##", "# # #", "##  #", "#   #", " ### "],
    // 1
    ["  #  ", " ##  ", "  #  ", "  #  ", "  #  ", "  #  ", " ### "],
    // 2
    [" ### ", "#   #", "    #", "   # ", "  #  ", " #   ", "#####"],
    // 3
    ["#####", "   # ", "  #  ", "   # ", "    #", "#   #", " ### "],
    // 4
    ["   # ", "  ## ", " # # ", "#  # ", "#####", "   # ", "   # "],
    // 5
    ["#####", "#    ", "#### ", "    #", "    #", "#   #", " ### "],
    // 6
    ["  ## ", " #   ", "#    ", "#### ", "#   #", "#   #", " ### "],
    // 7
    ["#####", "    #", "   # ", "  #  ", " #   ", " #   ", " #   "],
    // 8
    [" ### ", "#   #", "#   #", " ### ", "#   #", "#   #", " ### "],
    // 9
    [" ### ", "#   #", "#   #", " ####", "    #", "   # ", " ##  "],
    // :
    ["     ", "  ## ", "  ## ", "     ", "  ## ", "  ## ", "     "],
    // ;
    ["     ", "  ## ", "  ## ", "     ", "  ## ", "  ## ", "  #  "],
    // <
    ["   # ", "  #  ", " #   ", "#    ", " #   ", "  #  ", "   # "],
    // =
    ["     ", "     ", "#####", "     ", "#####", "     ", "     "],
    // >
    [" #   ", "  #  ", "   # ", "    #", "   # ", "  #  ", " #   "],
    // ?
    [" ### ", "#   #", "    #", "   # ", "  #  ", "     ", "  #  "],
    // @
    [" ### ", "#   #", "# ###", "# # #", "# ###", "#    ", " ### "],
    // A
    [" ### ", "#   #", "#   #", "#####", "#   #", "#   #", "#   #"],
    // B
    ["#### ", "#   #", "#   #", "#### ", "#   #", "#   #", "#### "],
    // C
    [" ### ", "#   #", "#    ", "#    ", "#    ", "#   #", " ### "],
    // D
    ["###  ", "#  # ", "#   #", "#   #", "#   #", "#  # ", "###  "],
    // E
    ["#####", "#    ", "#    ", "#### ", "#    ", "#    ", "#####"],
    // F
    ["#####", "#    ", "#    ", "#### ", "#    ", "#    ", "#    "],
    // G
    [" ### ", "#   #", "#    ", "# ###", "#   #", "#   #", " ####"],
    // H
    ["#   #", "#   #", "#   #", "#####", "#   #", "#   #", "#   #"],
    // I
    [" ### ", "  #  ", "  #  ", "  #  ", "  #  ", "  #  ", " ### "],
    // J
    ["    #", "    #", "    #", "    #", "#   #", "#   #", " ### "],
    // K
    ["#   #", "#  # ", "# #  ", "##   ", "# #  ", "#  # ", "#   #"],
    // L
    ["#    ", "#    ", "#    ", "#    ", "#    ", "#    ", "#####"],
    // M
    ["#   #", "## ##", "# # #", "#   #", "#   #", "#   #", "#   #"],
    // N
    ["#   #", "##  #", "# # #", "#  ##", "#   #", "#   #", "#   #"],
    // O
    [" ### ", "#   #", "#   #", "#   #", "#   #", "#   #", " ### "],
    // P
    ["#### ", "#   #", "#   #", "#### ", "#    ", "#    ", "#    "],
    // Q
    [" ### ", "#   #", "#   #", "#   #", "# # #", "#  # ", " ## #"],
    // R
    ["#### ", "#   #", "#   #", "#### ", "# #  ", "#  # ", "#   #"],
    // S
    [" ####", "#    ", "#    ", " ### ", "    #", "    #", "#### "],
    // T
    ["#####", "  #  ", "  #  ", "  #  ", "  #  ", "  #  ", "  #  "],
    // U
    ["#   #", "#   #", "#   #", "#   #", "#   #", "#   #", " ### "],
    // V
    ["#   #", "#   #", "#   #", "#   #", "#   #", " # # ", "  #  "],
    // W
    ["#   #", "#   #", "#   #", "#   #", "# # #", "## ##", "#   #"],
    // X
    ["#   #", "#   #", " # # ", "  #  ", " # # ", "#   #", "#   #"],
    // Y
    ["#   #", "#   #", " # # ", "  #  ", "  #  ", "  #  ", "  #  "],
    // Z
    ["#####", "    #", "   # ", "  #  ", " #   ", "#    ", "#####"],
    // [
    ["  ###", "  #  ", "  #  ", "  #  ", "  #  ", "  #  ", "  ###"],
    // backslash
    ["#    ", " #   ", " #   ", "  #  ", "   # ", "   # ", "    #"],
    // ]
    ["###  ", "  #  ", "  #  ", "  #  ", "  #  ", "  #  ", "###  "],
    // ^
    ["  #  ", " # # ", "#   #", "     ", "     ", "     ", "     "],
    // _
    ["     ", "     ", "     ", "     ", "     ", "     ", "#####"],
];

/// Atlas cell for a character, or `None` if the face has no glyph for it.
///
/// Lowercase folds onto uppercase; the face has no lowercase forms and a
/// missing-glyph box everywhere would be worse than shouting.
pub fn glyph_index(ch: char) -> Option<usize> {
    let upper = ch.to_ascii_uppercase() as u32;
    let first = FIRST_CHAR as u32;
    let last = LAST_CHAR as u32;
    if (first..=last).contains(&upper) {
        Some((upper - first) as usize)
    } else {
        None
    }
}

/// Rasterise the whole face into a single-channel atlas, row-major, one byte
/// per pixel: 255 where the glyph is inked, 0 elsewhere.
pub fn atlas_pixels() -> Vec<u8> {
    let mut pixels = vec![0u8; (ATLAS_WIDTH * ATLAS_HEIGHT) as usize];

    for (index, rows) in GLYPHS.iter().enumerate() {
        let cell_x = (index as u32 % COLUMNS) * CELL;
        let cell_y = (index as u32 / COLUMNS) * CELL;

        for (row, line) in rows.iter().enumerate() {
            for (column, ink) in line.bytes().enumerate() {
                if ink != b'#' {
                    continue;
                }
                let x = cell_x + column as u32;
                let y = cell_y + row as u32;
                pixels[(y * ATLAS_WIDTH + x) as usize] = 255;
            }
        }
    }

    pixels
}

/// Texture coordinates of a glyph's cell, as `(u0, v0, u1, v1)` covering only
/// the inked 5×7 area rather than the whole 8×8 cell.
pub fn glyph_uv(index: usize) -> (f32, f32, f32, f32) {
    let cell_x = (index as u32 % COLUMNS) * CELL;
    let cell_y = (index as u32 / COLUMNS) * CELL;
    (
        cell_x as f32 / ATLAS_WIDTH as f32,
        cell_y as f32 / ATLAS_HEIGHT as f32,
        (cell_x + GLYPH_WIDTH) as f32 / ATLAS_WIDTH as f32,
        (cell_y + GLYPH_HEIGHT) as f32 / ATLAS_HEIGHT as f32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_is_the_declared_size() {
        // A short row would shift every pixel after it in that glyph.
        for (index, rows) in GLYPHS.iter().enumerate() {
            let ch = (FIRST_CHAR + index as u8) as char;
            assert_eq!(rows.len(), GLYPH_HEIGHT as usize, "glyph {ch:?} row count");
            for (row, line) in rows.iter().enumerate() {
                assert_eq!(
                    line.chars().count(),
                    GLYPH_WIDTH as usize,
                    "glyph {ch:?} row {row} is {line:?}"
                );
                assert!(
                    line.chars().all(|c| c == '#' || c == ' '),
                    "glyph {ch:?} row {row} has stray characters: {line:?}"
                );
            }
        }
    }

    #[test]
    fn the_table_covers_exactly_the_declared_range() {
        assert_eq!(GLYPHS.len(), GLYPH_COUNT);
        assert_eq!(glyph_index(' '), Some(0));
        assert_eq!(glyph_index('_'), Some(GLYPH_COUNT - 1));
        assert_eq!(glyph_index('A'), Some((b'A' - FIRST_CHAR) as usize));
        assert_eq!(glyph_index('0'), Some((b'0' - FIRST_CHAR) as usize));
    }

    #[test]
    fn lowercase_folds_onto_uppercase() {
        assert_eq!(glyph_index('a'), glyph_index('A'));
        assert_eq!(glyph_index('z'), glyph_index('Z'));
    }

    #[test]
    fn characters_outside_the_face_have_no_glyph() {
        assert_eq!(glyph_index('~'), None);
        assert_eq!(glyph_index('\u{00e9}'), None);
        assert_eq!(glyph_index('\n'), None);
    }

    #[test]
    fn the_atlas_is_the_expected_shape_and_not_blank() {
        let pixels = atlas_pixels();
        assert_eq!(pixels.len(), (ATLAS_WIDTH * ATLAS_HEIGHT) as usize);

        let inked = pixels.iter().filter(|&&p| p > 0).count();
        assert!(inked > 500, "atlas looks empty: {inked} inked pixels");
        // Space is the first cell and must be entirely blank, or every gap in
        // every string renders as a block.
        for y in 0..CELL {
            for x in 0..CELL {
                assert_eq!(pixels[(y * ATLAS_WIDTH + x) as usize], 0, "space is inked");
            }
        }
    }

    #[test]
    fn glyphs_land_in_their_own_cells() {
        // Nothing may spill into the padding column or row, or characters
        // smear into their neighbours.
        let pixels = atlas_pixels();
        for y in 0..ATLAS_HEIGHT {
            for x in 0..ATLAS_WIDTH {
                let within_glyph = (x % CELL) < GLYPH_WIDTH && (y % CELL) < GLYPH_HEIGHT;
                if !within_glyph {
                    assert_eq!(
                        pixels[(y * ATLAS_WIDTH + x) as usize],
                        0,
                        "ink at ({x}, {y}) is outside its glyph box"
                    );
                }
            }
        }
    }

    #[test]
    fn uv_rectangles_stay_inside_the_atlas_and_move_per_glyph() {
        let (u0, v0, u1, v1) = glyph_uv(0);
        assert_eq!((u0, v0), (0.0, 0.0));
        assert!(u1 > u0 && v1 > v0);

        for index in 0..GLYPH_COUNT {
            let (u0, v0, u1, v1) = glyph_uv(index);
            assert!((0.0..=1.0).contains(&u0) && (0.0..=1.0).contains(&u1));
            assert!((0.0..=1.0).contains(&v0) && (0.0..=1.0).contains(&v1));
        }

        // The second cell sits one cell to the right, not on top of the first.
        assert_ne!(glyph_uv(1).0, glyph_uv(0).0);
    }
}
