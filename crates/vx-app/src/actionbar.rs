//! The action bar: what is in your hands, and what else is a keypress away.
//!
//! The game already had a hotbar — number keys one to eight pick a block to
//! place, and seven-- sorry, `Digit7`'s neighbour on the far right takes the
//! launcher in and out of hand — but it had no *face*. Nothing on screen said
//! which slot was live, what was in it, or that the slots existed at all, so
//! the only way to learn the hotbar was to read the source or press keys and
//! watch the log. That is fine for the person who wrote it and useless for
//! anybody testing it.
//!
//! So: one strip, centred along the bottom, one cell per slot, the live one
//! lit. It is drawn from a description the caller assembles rather than from
//! game state directly, which keeps it a pure function of its inputs the way
//! every other panel here is — a capture of the bar is the same capture twice.

use vx_render::font::{self, LINE_HEIGHT};

/// The digit keys that pick palette slots, in slot order.
///
/// Seven is missing on purpose: it belongs to the launcher, and has since
/// before this bar existed. The table lives here, and both the bar and the
/// keyboard handler read it, because the one thing worse than an undiscoverable
/// hotbar is a hotbar whose labels disagree with its keys.
pub const PALETTE_KEYS: [char; 8] = ['1', '2', '3', '4', '5', '6', '8', '9'];

/// The key that takes the launcher in and out of hand.
pub const LAUNCHER_KEY: char = '7';

/// Which palette slot a digit picks, if any.
pub fn slot_of_key(key: char) -> Option<usize> {
    PALETTE_KEYS.iter().position(|candidate| *candidate == key)
}

/// The key that picks palette slot `index`, if it has one.
pub fn key_of_slot(index: usize) -> Option<char> {
    PALETTE_KEYS.get(index).copied()
}

/// One cell, in texture pixels.
pub const CELL: u32 = 34;
/// Gap between cells.
pub const GAP: u32 = 3;
/// Room under the cells for the name of whatever is selected.
pub const LABEL_BAND: u32 = LINE_HEIGHT + 4;

/// On-screen scale, matching the HUD's so the two read as one interface.
pub const BAR_SCALE: f32 = 2.0;

const TEXT: [u8; 4] = [235, 235, 235, 255];
const DIM: [u8; 4] = [140, 140, 148, 255];
const ACCENT: [u8; 4] = [255, 170, 60, 255];
const CELL_BACK: [u8; 4] = [18, 20, 26, 190];
const CELL_LIVE: [u8; 4] = [38, 42, 52, 225];
const EDGE: [u8; 4] = [70, 74, 84, 220];
/// The launcher's cell goes warm when it is loaded and cold when it is dry,
/// because "can I actually fire this" is the only question the cell answers.
const LOADED: [u8; 4] = [255, 170, 60, 255];
const DRY: [u8; 4] = [235, 90, 70, 255];
/// A slot you cannot use at all — a launcher you have not bought. Darker than
/// `DIM`, which is merely "not in hand": the two states are different answers
/// and must not share a colour.
const UNUSABLE: [u8; 4] = [78, 78, 86, 255];

/// What one cell holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    /// The key that picks it, drawn small in the corner.
    pub key: char,
    /// Two or three letters naming the contents.
    pub short: String,
    /// The full name, shown under the bar when this slot is live.
    pub label: String,
    /// A count worth showing — slugs in the satchel. `None` draws nothing.
    pub count: Option<u32>,
    /// Whether the slot can actually be used. A launcher you do not own, or
    /// one with an empty satchel, draws dim.
    pub usable: bool,
}

/// Everything the bar shows this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarContent {
    pub slots: Vec<Slot>,
    /// Which slot is in hand.
    pub live: usize,
}

impl BarContent {
    /// Width the strip needs, in texture pixels.
    pub fn width(&self) -> u32 {
        let count = self.slots.len().max(1) as u32;
        count * CELL + (count - 1) * GAP
    }

    pub fn height(&self) -> u32 {
        CELL + LABEL_BAND
    }
}

/// Draw the bar. A pure function of `content`, like every other panel here.
pub fn render_bar(content: &BarContent) -> Vec<u8> {
    let width = content.width();
    let height = content.height();
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    // Transparent ground: the bar is cells, not a slab. Only the cells
    // themselves paint, so the world shows between them.
    for texel in pixels.chunks_exact_mut(4) {
        texel.copy_from_slice(&[0, 0, 0, 0]);
    }

    for (index, slot) in content.slots.iter().enumerate() {
        let live = index == content.live;
        let x0 = index as u32 * (CELL + GAP);
        let back = if live { CELL_LIVE } else { CELL_BACK };
        for y in 0..CELL {
            for x in x0..x0 + CELL {
                put(&mut pixels, width, height, x, y, back);
            }
        }
        // The border: accent on the live cell, plain otherwise. Two pixels on
        // the live one so it reads at a glance rather than on inspection.
        let edge = if live { ACCENT } else { EDGE };
        let thickness = if live { 2 } else { 1 };
        for inset in 0..thickness {
            for x in x0 + inset..x0 + CELL - inset {
                put(&mut pixels, width, height, x, inset, edge);
                put(&mut pixels, width, height, x, CELL - 1 - inset, edge);
            }
            for y in inset..CELL - inset {
                put(&mut pixels, width, height, x0 + inset, y, edge);
                put(&mut pixels, width, height, x0 + CELL - 1 - inset, y, edge);
            }
        }

        // The key, small in the top-left corner.
        let key = slot.key.to_string();
        font::draw_text(
            &mut pixels,
            width,
            x0 as i32 + 4,
            4,
            1,
            if live { ACCENT } else { DIM },
            &key,
        );

        // The contents, centred.
        let tint = match (slot.usable, live) {
            (false, _) => UNUSABLE,
            (true, true) => TEXT,
            (true, false) => DIM,
        };
        let short_width = font::text_width(&slot.short, 1) as i32;
        font::draw_text(
            &mut pixels,
            width,
            x0 as i32 + (CELL as i32 - short_width) / 2,
            (CELL / 2) as i32 - 3,
            1,
            tint,
            &slot.short,
        );

        // A count, along the bottom of the cell.
        if let Some(count) = slot.count {
            let text = count.to_string();
            let count_width = font::text_width(&text, 1) as i32;
            font::draw_text(
                &mut pixels,
                width,
                x0 as i32 + CELL as i32 - 4 - count_width,
                CELL as i32 - LINE_HEIGHT as i32 - 2,
                1,
                if count == 0 { DRY } else { LOADED },
                &text,
            );
        }
    }

    // The live slot's full name, centred under the strip.
    if let Some(slot) = content.slots.get(content.live) {
        let label_width = font::text_width(&slot.label, 1) as i32;
        font::draw_text(
            &mut pixels,
            width,
            (width as i32 - label_width) / 2,
            CELL as i32 + 3,
            1,
            TEXT,
            &slot.label,
        );
    }

    pixels
}

fn put(pixels: &mut [u8], width: u32, height: u32, x: u32, y: u32, colour: [u8; 4]) {
    if x >= width || y >= height {
        return;
    }
    let at = ((y * width + x) * 4) as usize;
    pixels[at..at + 4].copy_from_slice(&colour);
}

/// Turn a block's registry name into something that fits a cell.
///
/// `engine:oak_log` becomes `OAK`, which is not perfect and is a great deal
/// better than three characters of `engine:`.
pub fn short_name(name: &str) -> String {
    let bare = name.split_once(':').map_or(name, |(_, rest)| rest);
    let first = bare.split('_').next().unwrap_or(bare);
    first.chars().take(4).collect::<String>().to_uppercase()
}

/// The full name for the label band: `engine:oak_log` becomes `OAK LOG`.
pub fn full_name(name: &str) -> String {
    let bare = name.split_once(':').map_or(name, |(_, rest)| rest);
    bare.replace('_', " ").to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(key: char, short: &str) -> Slot {
        Slot {
            key,
            short: short.to_string(),
            label: short.to_string(),
            count: None,
            usable: true,
        }
    }

    fn bar() -> BarContent {
        BarContent {
            slots: vec![slot('1', "STON"), slot('2', "DIRT"), slot('3', "SAND")],
            live: 0,
        }
    }

    #[test]
    fn the_bar_is_as_wide_as_its_cells_and_gaps() {
        let content = bar();
        assert_eq!(content.width(), 3 * CELL + 2 * GAP);
        assert_eq!(content.height(), CELL + LABEL_BAND);
    }

    #[test]
    fn the_drawn_bar_matches_its_declared_size() {
        let content = bar();
        let pixels = render_bar(&content);
        assert_eq!(
            pixels.len(),
            (content.width() * content.height() * 4) as usize
        );
    }

    #[test]
    fn the_live_slot_looks_different_from_the_others() {
        // The entire point of the bar: you can see what is in your hands.
        let mut content = bar();
        let first = render_bar(&content);
        content.live = 1;
        let second = render_bar(&content);
        assert_ne!(first, second, "moving the selection changed nothing on screen");
    }

    #[test]
    fn drawing_is_a_function_of_the_content_alone() {
        let content = bar();
        assert_eq!(render_bar(&content), render_bar(&content));
    }

    #[test]
    fn a_count_of_zero_draws_differently_from_a_full_satchel() {
        // Dry versus loaded is the only question the launcher's cell answers,
        // so the two must not look the same.
        let with = |count| {
            let mut content = bar();
            content.slots[0].count = Some(count);
            render_bar(&content)
        };
        assert_ne!(with(0), with(9));
    }

    #[test]
    fn an_unusable_slot_draws_dim() {
        let mut usable = bar();
        usable.live = 2;
        let mut unusable = usable.clone();
        unusable.slots[0].usable = false;
        assert_ne!(render_bar(&usable), render_bar(&unusable));
    }

    #[test]
    fn an_empty_bar_still_draws_without_panicking() {
        // Width uses a `max(1)`; this is the test that says why.
        let content = BarContent { slots: Vec::new(), live: 0 };
        let pixels = render_bar(&content);
        assert_eq!(pixels.len(), (content.width() * content.height() * 4) as usize);
    }

    #[test]
    fn a_live_index_past_the_end_draws_no_label_rather_than_panicking() {
        let content = BarContent { slots: vec![slot('1', "STON")], live: 7 };
        let pixels = render_bar(&content);
        assert_eq!(pixels.len(), (content.width() * content.height() * 4) as usize);
    }

    #[test]
    fn the_key_table_round_trips_and_leaves_seven_to_the_launcher() {
        // The bar's labels and the keyboard handler read this one table, so a
        // slot can never be drawn with a key that does not pick it.
        for (index, key) in PALETTE_KEYS.iter().enumerate() {
            assert_eq!(slot_of_key(*key), Some(index));
            assert_eq!(key_of_slot(index), Some(*key));
        }
        assert_eq!(
            slot_of_key(LAUNCHER_KEY),
            None,
            "the launcher's key also picks a block"
        );
        assert_eq!(key_of_slot(PALETTE_KEYS.len()), None);
    }

    #[test]
    fn names_are_shortened_without_their_namespace() {
        assert_eq!(short_name("engine:stone"), "STON");
        assert_eq!(short_name("engine:oak_log"), "OAK");
        assert_eq!(short_name("dirt"), "DIRT");
        assert_eq!(full_name("engine:oak_log"), "OAK LOG");
        assert_eq!(full_name("engine:electrolyser"), "ELECTROLYSER");
    }
}
