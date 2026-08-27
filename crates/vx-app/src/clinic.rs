//! The clinic: the one door on the frontier worth walking a long way to.
//!
//! # Why a free bed is the right price
//!
//! Health has mended on a timer since stage 28, and an arrest has put you
//! back on your feet since the same round. Both are things that *happen to*
//! you. Neither is a place. A hospital makes recovery somewhere you go, and
//! the cost of going is the going: a cot in town is no use at all at the
//! bottom of a shaft forty minutes from anywhere, which is exactly what
//! makes the walk a decision and the medkit in your pocket worth its price.
//!
//! So the bed is free. Charging for it would tax the player for the one
//! thing this game already made expensive — distance — and a frontier that
//! bills a bleeding stranger at the door is a different game than this one.
//!
//! # What the ward is actually for
//!
//! Two things, and they answer two different rounds:
//!
//! * **The cot** puts every hit back and **scrubs the dose**. That second
//!   half is the point after stage 31: standing in a uranium face is a
//!   bargain you take knowing there is somewhere to go afterwards, and
//!   without the ward the only cure was to stop mining and wait it out.
//! * **Medkits** are the field answer: bought here with credits, carried,
//!   and spent wherever you happen to be bleeding.
//!
//! # None of this reaches the oracle
//!
//! Nothing here touches the world or the base pile. Credits are live-only,
//! health has been live-only since it existed, and a medkit is a count in a
//! pocket rather than a good on a pile — so the journal never hears about
//! any of it, and no version bump belongs to this file. The same line stage
//! 28 drew, held.

use vx_render::font::{self, LINE_HEIGHT};

use crate::health::{Health, MEDKITS_MAX, MEDKIT_HITS};

/// What one medkit costs at the counter.
pub const MEDKIT_PRICE: u64 = 45;

/// The rows the ward offers, in the order they are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// Lie down. Free, and it scrubs the dose with it.
    Rest,
    /// One for the road.
    Medkit,
}

pub const ROWS: [Row; 2] = [Row::Rest, Row::Medkit];

impl Row {
    pub fn label(self) -> &'static str {
        match self {
            Row::Rest => "REST ON THE COT",
            Row::Medkit => "BUY A MEDKIT",
        }
    }

    /// What it costs, in credits.
    pub fn price(self) -> u64 {
        match self {
            Row::Rest => 0,
            Row::Medkit => MEDKIT_PRICE,
        }
    }
}

/// Why a row cannot be taken, in the order somebody would notice.
pub fn refuse(row: Row, health: &Health, credits: u64, dose: f32) -> Option<String> {
    match row {
        Row::Rest => {
            (!health.hurt() && dose <= 1.0).then(|| "NOTHING WRONG WITH YOU".to_string())
        }
        Row::Medkit => {
            if health.medkits() >= MEDKITS_MAX {
                return Some("YOU CANNOT CARRY ANY MORE".to_string());
            }
            (credits < MEDKIT_PRICE).then(|| format!("SHORT {} CREDITS", MEDKIT_PRICE - credits))
        }
    }
}

/// The live half: which ward is open, where the cursor is, what it last said.
///
/// Live-only like the wellhead's panel and the electrolyser's, and for the
/// same reason: opening a door and being refused change nothing.
#[derive(Debug, Default)]
pub struct Clinic {
    pub open: bool,
    pub at: Option<vx_core::BlockPos>,
    pub cursor: usize,
    pub feedback: Option<String>,
}

impl Clinic {
    pub fn open_at(&mut self, at: vx_core::BlockPos) {
        self.open = true;
        self.at = Some(at);
        self.feedback = None;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn move_cursor(&mut self, delta: i32) {
        let last = ROWS.len() as i32 - 1;
        self.cursor = (self.cursor as i32 + delta).clamp(0, last) as usize;
    }

    pub fn row(&self) -> Row {
        ROWS[self.cursor.min(ROWS.len() - 1)]
    }
}

// ---------------------------------------------------------------- the panel

const TEXT: [u8; 4] = [235, 235, 235, 255];
const DIM: [u8; 4] = [150, 150, 155, 255];
const ACCENT: [u8; 4] = [150, 220, 200, 255];
const SHORT: [u8; 4] = [235, 110, 90, 255];
const GOOD: [u8; 4] = [150, 220, 150, 255];
const BACKGROUND: [u8; 4] = [10, 16, 18, 240];

pub const WARD_WIDTH: u32 = 264;
/// Derived from the rows, never typed: the fabricator's panel overran a
/// hand-written height the day its catalogue grew, and once was enough.
/// Seven lines of text — title, town, condition, the rows, what you are
/// carrying and the feedback — plus the margins and the three gaps between
/// blocks. Counted rather than eyeballed: the first cut of this panel ate
/// its own last line.
pub const WARD_HEIGHT: u32 = 6 + LINE_HEIGHT * (5 + ROWS.len() as u32) + 6 + 2 + 2 + 3 + 3;

/// Everything the panel draws, snapshotted.
#[derive(Debug, Clone, PartialEq)]
pub struct ClinicContent {
    /// The town this ward belongs to.
    pub town: String,
    pub cursor: usize,
    /// Hits left, and of how many.
    pub condition: (u8, u8),
    /// Rads carried, for the line that says the cot will take them away.
    pub rads: f32,
    pub medkits: u8,
    pub credits: u64,
    pub feedback: Option<String>,
}

/// Draw the ward. Pure in its input, like every panel here.
pub fn render_clinic(content: &ClinicContent) -> Vec<u8> {
    let mut pixels = vec![0u8; (WARD_WIDTH * WARD_HEIGHT * 4) as usize];
    for texel in pixels.chunks_exact_mut(4) {
        texel.copy_from_slice(&BACKGROUND);
    }

    let margin = 6i32;
    let mut y = margin;
    font::draw_text(&mut pixels, WARD_WIDTH, margin, y, 1, ACCENT, "THE CLINIC");
    let credits = format!("{}C", content.credits);
    font::draw_text(
        &mut pixels,
        WARD_WIDTH,
        WARD_WIDTH as i32 - margin - font::text_width(&credits, 1) as i32,
        y,
        1,
        DIM,
        &credits,
    );
    y += LINE_HEIGHT as i32 + 2;

    font::draw_text(&mut pixels, WARD_WIDTH, margin, y, 1, DIM, &content.town);
    y += LINE_HEIGHT as i32 + 2;

    // What is actually wrong with you, which is what the rows are answering.
    let (hits, of) = content.condition;
    let hurt = hits < of;
    let hot = content.rads > 1.0;
    let state = match (hurt, hot) {
        (false, false) => "WHOLE AND CLEAN".to_string(),
        (true, false) => format!("HITS {hits}/{of}"),
        (false, true) => format!("CLEAN - DOSE {:.0}", content.rads),
        (true, true) => format!("HITS {hits}/{of}   DOSE {:.0}", content.rads),
    };
    font::draw_text(
        &mut pixels,
        WARD_WIDTH,
        margin,
        y,
        1,
        if hurt || hot { SHORT } else { GOOD },
        &state,
    );
    y += LINE_HEIGHT as i32 + 3;

    for (index, row) in ROWS.iter().enumerate() {
        if index == content.cursor {
            font::draw_text(&mut pixels, WARD_WIDTH, margin, y, 1, ACCENT, ">");
        }
        let colour = if index == content.cursor { TEXT } else { DIM };
        font::draw_text(&mut pixels, WARD_WIDTH, margin + 10, y, 1, colour, row.label());
        let price = match row {
            Row::Rest => "FREE".to_string(),
            Row::Medkit => format!("{}C", row.price()),
        };
        font::draw_text(
            &mut pixels,
            WARD_WIDTH,
            WARD_WIDTH as i32 - margin - font::text_width(&price, 1) as i32,
            y,
            1,
            if matches!(row, Row::Rest) { GOOD } else { colour },
            &price,
        );
        y += LINE_HEIGHT as i32;
    }
    y += 3;

    let carried = format!(
        "CARRYING {}/{MEDKITS_MAX} MEDKITS - {MEDKIT_HITS} HITS EACH",
        content.medkits
    );
    font::draw_text(&mut pixels, WARD_WIDTH, margin, y, 1, DIM, &carried);
    y += LINE_HEIGHT as i32;

    if let Some(line) = &content.feedback {
        font::draw_text(&mut pixels, WARD_WIDTH, margin, y, 1, TEXT, line);
    }

    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn broke() -> ClinicContent {
        ClinicContent {
            town: "STONEHAVEN".into(),
            cursor: 0,
            condition: (2, crate::health::MAX_HITS),
            rads: 143.0,
            medkits: 0,
            credits: 3,
            feedback: Some("SHORT 42 CREDITS".into()),
        }
    }

    #[test]
    fn the_bed_is_free_and_the_kit_is_not() {
        // The whole economy of the building, pinned: charging for the cot
        // would tax the player for distance, which this game already
        // charges for.
        assert_eq!(Row::Rest.price(), 0);
        assert!(Row::Medkit.price() > 0);
    }

    #[test]
    fn a_ward_turns_away_somebody_with_nothing_wrong_with_them() {
        let mut health = Health::default();
        assert!(refuse(Row::Rest, &health, 500, 0.0).is_some());
        // Hurt, or hot, and the bed is yours.
        health.take(1);
        assert!(refuse(Row::Rest, &health, 0, 0.0).is_none());
        assert!(refuse(Row::Rest, &Health::default(), 0, 90.0).is_none());
    }

    #[test]
    fn a_medkit_is_refused_when_broke_or_full() {
        let mut health = Health::default();
        assert!(refuse(Row::Medkit, &health, 0, 0.0).is_some());
        assert!(refuse(Row::Medkit, &health, MEDKIT_PRICE, 0.0).is_none());
        for _ in 0..MEDKITS_MAX {
            health.stock_medkit();
        }
        assert_eq!(
            refuse(Row::Medkit, &health, 10_000, 0.0),
            Some("YOU CANNOT CARRY ANY MORE".to_string())
        );
    }

    #[test]
    fn the_cursor_stays_on_the_rows() {
        let mut ward = Clinic::default();
        ward.move_cursor(-5);
        assert_eq!(ward.row(), Row::Rest);
        ward.move_cursor(9);
        assert_eq!(ward.row(), Row::Medkit);
    }

    #[test]
    fn every_state_of_the_panel_draws_inside_its_frame() {
        let states = [
            broke(),
            ClinicContent {
                cursor: 1,
                condition: (crate::health::MAX_HITS, crate::health::MAX_HITS),
                rads: 0.0,
                medkits: MEDKITS_MAX,
                credits: 999_999,
                feedback: None,
                ..broke()
            },
            ClinicContent {
                rads: 0.0,
                condition: (1, crate::health::MAX_HITS),
                feedback: Some("PATCHED UP - REST WELL".into()),
                ..broke()
            },
        ];
        for content in states {
            let pixels = render_clinic(&content);
            assert_eq!(pixels.len(), (WARD_WIDTH * WARD_HEIGHT * 4) as usize);
            assert!(
                pixels.chunks_exact(4).any(|texel| texel != BACKGROUND),
                "a ward drew nothing at all"
            );
        }

        // And the busiest one actually *fits*: the feedback line is the last
        // thing drawn, so ink in the bottom band is the proof the panel is
        // tall enough for everything it says.
        let full = ClinicContent {
            feedback: Some("PATCHED UP AND SCRUBBED - 143 RADS OFF YOU".into()),
            ..broke()
        };
        let pixels = render_clinic(&full);
        let band = ((WARD_HEIGHT - LINE_HEIGHT - 2) * WARD_WIDTH * 4) as usize;
        assert!(
            pixels[band..].chunks_exact(4).any(|texel| texel != BACKGROUND),
            "the ward clipped its own last line"
        );
    }
}
