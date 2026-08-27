//! The gold panel: the operator's console.
//!
//! # A cheat is an order like any other
//!
//! This module owns **no mutation path**. Every button resolves to a
//! [`crate::journal::Admin`] order in the same journal as movement and
//! dispatches, so a session full of cheats still replays to a hash — the
//! moment an admin action mutated state outside the log, the replay oracle
//! would be blind to it and every replay of that session would report a
//! divergence that is nobody's fault. Set up a situation through this panel
//! and the journal *is* that scenario: committed, it is a regression fixture;
//! handed over, it is a perfect reproduction case.
//!
//! Reading is free: inspectors journal nothing, because looking at state does
//! not change it. And nothing here reads a wall clock — panels display the
//! sim's tick, so captures stay byte-identical with the panel open.
//!
//! # Fiction never sees gold
//!
//! Compiled out of the shipped build (`--no-default-features`), opened only by
//! `--gold`, and no diegetic system may depend on it. One honest retrofit to
//! the design note: the gold *hue* is already the house accent in every panel,
//! so what marks this one is chrome no other panel has — the double gold
//! border. If a screenshot shows the border, the session was touched; nobody
//! has to ask whether a bug report came from a clean run.
//!
//! # Deck-shaped, keyboard-driven
//!
//! The focus model is built for a controller — directional focus, bumper-style
//! tab cycling, one activate button, hold-to-slide on numeric fields — but no
//! gamepad backend exists anywhere yet, so today the keys stand in: arrows for
//! the d-pad, Tab for the bumper, Enter for A, X to reset a field. When a
//! gamepad crate lands (with hardware to test it on), it maps onto this model
//! without the panel changing.

use crate::journal::Admin;
use crate::tuning;
use vx_render::font::{self, LINE_HEIGHT};

/// Panel size in texture pixels, at the shop's display scale.
pub const GOLD_WIDTH: u32 = 300;
pub const GOLD_HEIGHT: u32 = 240;

const TEXT: [u8; 4] = [235, 235, 235, 255];
const DIM: [u8; 4] = [150, 150, 155, 255];
const GOLD: [u8; 4] = [255, 200, 40, 255];
const GOOD: [u8; 4] = [120, 220, 120, 255];
const DIRTY: [u8; 4] = [255, 140, 200, 255];
/// Nearly opaque and nearly black: operator chrome, not game fiction.
const BACKGROUND: [u8; 4] = [6, 6, 10, 245];

/// The tabs, bumper-cycled in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Player,
    Spawn,
    Town,
    World,
    Tuning,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Player, Tab::Spawn, Tab::Town, Tab::World, Tab::Tuning];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Player => "PLAYER",
            Tab::Spawn => "SPAWN",
            Tab::Town => "TOWN",
            Tab::World => "WORLD",
            Tab::Tuning => "TUNING",
        }
    }

    pub fn next(self) -> Tab {
        let index = Tab::ALL.iter().position(|tab| *tab == self).unwrap_or(0);
        Tab::ALL[(index + 1) % Tab::ALL.len()]
    }
}

/// One actionable row on a tab. The panel renders these; activating one asks
/// the app to journal the order it carries.
#[derive(Debug, Clone, PartialEq)]
pub enum RowAction {
    /// Issue this order as-is.
    Order(Admin),
    /// A numeric field: hold-activate turns the vertical axis into a slider.
    /// `step` is one notch; the app journals `SetTuning` on release.
    Slider { key: &'static str, step: f32 },
    /// Plain information; activating does nothing.
    Note,
}

/// A row: what it says, and what it does.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: String,
    pub value: String,
    pub action: RowAction,
    /// Drawn in the dirty colour: this field differs from its default.
    pub dirty: bool,
}

impl Row {
    fn note(label: impl Into<String>, value: impl Into<String>) -> Self {
        Row {
            label: label.into(),
            value: value.into(),
            action: RowAction::Note,
            dirty: false,
        }
    }

    fn order(label: impl Into<String>, value: impl Into<String>, order: Admin) -> Self {
        Row {
            label: label.into(),
            value: value.into(),
            action: RowAction::Order(order),
            dirty: false,
        }
    }
}

/// What the panel needs to read to draw itself. References only — the panel
/// never holds game state, it looks at it.
pub struct Telemetry<'a> {
    pub tick: u64,
    pub position: glam::Vec3,
    pub stance: &'static str,
    pub stamina: f32,
    pub credits: u64,
    pub bounty: u64,
    pub drones: u32,
    pub fliers: u32,
    pub base_total: u64,
    pub town_name: Option<String>,
    pub town_centre: Option<(i32, i32)>,
    /// One entry per traded good, sized by the catalogue rather than by a
    /// number typed here: HHO joined the economy in the fuel round and this
    /// array did not follow, which indexed off the end the moment the town
    /// tab drew. Deriving the length means the next good cannot repeat it.
    pub stocks: [f32; crate::economy::GOODS.len()],
    pub tuning: &'a tuning::Tuning,
    /// The world hash, computed only when the operator asked for it — it walks
    /// every loaded chunk, which is not a per-frame cost.
    pub world_hash: Option<u64>,
}

/// The panel's state: which tab, what has focus, slider-in-progress.
#[derive(Debug, Default)]
pub struct Gold {
    pub open: bool,
    pub tab_index: usize,
    pub cursor: usize,
    /// A slider being held: the key, and the pending (uncommitted) value.
    pub sliding: Option<(&'static str, f32)>,
    pub feedback: Option<String>,
}

impl Gold {
    pub fn tab(&self) -> Tab {
        Tab::ALL[self.tab_index % Tab::ALL.len()]
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.sliding = None;
        self.feedback = None;
    }

    pub fn cycle_tab(&mut self) {
        let next = self.tab().next();
        self.tab_index = Tab::ALL.iter().position(|tab| *tab == next).unwrap_or(0);
        self.cursor = 0;
        self.sliding = None;
    }

    pub fn move_cursor(&mut self, delta: i32, rows: usize) {
        if rows == 0 {
            self.cursor = 0;
            return;
        }
        self.cursor = (self.cursor as i32 + delta).clamp(0, rows as i32 - 1) as usize;
    }

    /// The rows for the current tab, rebuilt fresh each frame — the same
    /// pure-list-plus-cursor idiom every panel here uses.
    pub fn rows(&self, telemetry: &Telemetry) -> Vec<Row> {
        match self.tab() {
            Tab::Player => self.player_rows(telemetry),
            Tab::Spawn => spawn_rows(),
            Tab::Town => town_rows(telemetry),
            Tab::World => world_rows(telemetry),
            Tab::Tuning => tuning_rows(telemetry, self.sliding),
        }
    }

    fn player_rows(&self, telemetry: &Telemetry) -> Vec<Row> {
        let at = telemetry.position;
        vec![
            Row::note(
                "AT",
                format!("{:.0} {:.0} {:.0}  {}", at.x, at.y, at.z, telemetry.stance),
            ),
            Row::note("STAMINA", format!("{:.0}", telemetry.stamina)),
            Row::note("CREDITS", format!("{}", telemetry.credits)),
            Row::note("BOUNTY", format!("{}", telemetry.bounty)),
            Row::order(
                "TELEPORT HOME",
                "YOUR HOUSE",
                Admin::Teleport { x: -14, y: 73, z: 9 },
            ),
            Row::order(
                "TELEPORT AHEAD",
                "50 BLOCKS",
                // Filled in by the app with the real heading at activate time;
                // this placeholder keeps the row constructible for rendering.
                Admin::Teleport { x: 0, y: 0, z: 0 },
            ),
            Row::order(
                "FILL STAMINA",
                "100",
                Admin::SetStat { key: "stamina".into(), value: 100 },
            ),
        ]
    }
}

fn spawn_rows() -> Vec<Row> {
    let mut rows = vec![
        Row::order(
            "GIVE ORE",
            "X60 TO BASE",
            Admin::Give { good: "engine:copper_ore".into(), amount: 60 },
        ),
        Row::order(
            "GIVE BARS",
            "X20 TO BASE",
            Admin::Give { good: "engine:copper_bar".into(), amount: 20 },
        ),
        Row::order(
            "GIVE LOGS",
            "X60 TO BASE",
            Admin::Give { good: "engine:log".into(), amount: 60 },
        ),
        Row::order(
            "GIVE STONE",
            "X120 TO BASE",
            Admin::Give { good: "engine:stone".into(), amount: 120 },
        ),
    ];
    rows.push(Row::order(
        "SPAWN DRONE",
        "+1",
        Admin::SpawnMachine { kind: "drone".into(), count: 1 },
    ));
    rows.push(Row::order(
        "SPAWN FLIER",
        "+1",
        Admin::SpawnMachine { kind: "flier".into(), count: 1 },
    ));
    rows.push(Row::order(
        "GIVE CREDITS",
        "+1000",
        Admin::SetStat { key: "credits".into(), value: 1_000 },
    ));
    rows
}

fn town_rows(telemetry: &Telemetry) -> Vec<Row> {
    let Some(centre) = telemetry.town_centre else {
        return vec![Row::note("NO TOWN IN RANGE", "")];
    };
    let name = telemetry.town_name.clone().unwrap_or_default();
    let mut rows = vec![Row::note("TOWN", name)];
    for (index, good) in crate::economy::GOODS.iter().enumerate() {
        let label = crate::shop::display_name(good);
        rows.push(Row::order(
            format!("STOCK {label}"),
            format!("{:.0} -> 600", telemetry.stocks[index]),
            Admin::SetStock {
                x: centre.0,
                z: centre.1,
                good: (*good).to_string(),
                amount: 600,
            },
        ));
    }
    rows
}

fn world_rows(telemetry: &Telemetry) -> Vec<Row> {
    vec![
        Row::note("TICK", format!("{}", telemetry.tick)),
        Row::note(
            "WORLD HASH",
            match telemetry.world_hash {
                Some(hash) => format!("{hash:016X}"),
                None => "PRESS ENTER TO COMPUTE".into(),
            },
        ),
        Row::note(
            "FLEET",
            format!(
                "{} DRONES {} FLIERS  PILE {}",
                telemetry.drones, telemetry.fliers, telemetry.base_total
            ),
        ),
        // Advancing time is an ordinary Advance in the journal; the app caps
        // one press at a bounded burst so a fat finger cannot freeze a frame
        // for a minute of unbounded catch-up.
        Row::note("ADVANCE 80 TICKS", "10 SECONDS, ENTER"),
    ]
}

fn tuning_rows(telemetry: &Telemetry, sliding: Option<(&'static str, f32)>) -> Vec<Row> {
    let defaults = tuning::Tuning::default();
    tuning::KEYS
        .iter()
        .map(|key| {
            let live = telemetry.tuning.get(key).unwrap_or(0.0);
            let shown = match sliding {
                Some((held, pending)) if held == *key => pending,
                _ => live,
            };
            let default = defaults.get(key).unwrap_or(0.0);
            Row {
                label: tuning::label(key),
                value: if (shown - default).abs() > 1.0e-6 {
                    format!("{shown:.2} ({default:.2})")
                } else {
                    format!("{shown:.2}")
                },
                action: RowAction::Slider {
                    key: leak_key(key),
                    step: (default.abs() * 0.05).max(0.05),
                },
                dirty: (live - default).abs() > 1.0e-6,
            }
        })
        .collect()
}

/// `tuning::KEYS` entries are already `&'static str`; this keeps the row type
/// simple without inventing a lifetime for one field.
fn leak_key(key: &&'static str) -> &'static str {
    key
}

/// Draw the panel. Pure in its inputs, like every panel here — the border is
/// the signature: two gold rules no diegetic panel draws.
pub fn render_gold(gold: &Gold, telemetry: &Telemetry) -> Vec<u8> {
    let mut pixels = vec![0u8; (GOLD_WIDTH * GOLD_HEIGHT * 4) as usize];
    for texel in pixels.chunks_exact_mut(4) {
        texel.copy_from_slice(&BACKGROUND);
    }

    // The double border. If a capture shows this, the session was touched.
    for inset in [0u32, 2] {
        for x in inset..GOLD_WIDTH - inset {
            put(&mut pixels, x, inset, GOLD);
            put(&mut pixels, x, GOLD_HEIGHT - 1 - inset, GOLD);
        }
        for y in inset..GOLD_HEIGHT - inset {
            put(&mut pixels, inset, y, GOLD);
            put(&mut pixels, GOLD_WIDTH - 1 - inset, y, GOLD);
        }
    }

    let margin = 8i32;
    let mut y = margin;

    // The tab bar.
    let mut x = margin;
    for tab in Tab::ALL {
        let selected = tab == gold.tab();
        let tint = if selected { GOLD } else { DIM };
        font::draw_text(&mut pixels, GOLD_WIDTH, x, y, 1, tint, tab.title());
        x += font::text_width(tab.title(), 1) as i32 + 10;
    }
    let tick = format!("T {}", telemetry.tick);
    font::draw_text(
        &mut pixels,
        GOLD_WIDTH,
        GOLD_WIDTH as i32 - margin - font::text_width(&tick, 1) as i32,
        y,
        1,
        DIM,
        &tick,
    );
    y += LINE_HEIGHT as i32 + 4;

    let rows = gold.rows(telemetry);
    for (index, row) in rows.iter().enumerate() {
        let selected = index == gold.cursor;
        if selected {
            font::draw_text(&mut pixels, GOLD_WIDTH, margin, y, 1, GOLD, ">");
        }
        let tint = if row.dirty {
            DIRTY
        } else if selected {
            TEXT
        } else {
            DIM
        };
        font::draw_text(&mut pixels, GOLD_WIDTH, margin + 10, y, 1, tint, &row.label);
        font::draw_text(
            &mut pixels,
            GOLD_WIDTH,
            GOLD_WIDTH as i32 - margin - font::text_width(&row.value, 1) as i32,
            y,
            1,
            if row.dirty { DIRTY } else { GOOD },
            &row.value,
        );
        y += LINE_HEIGHT as i32;
        if y > GOLD_HEIGHT as i32 - 2 * LINE_HEIGHT as i32 - margin {
            break;
        }
    }

    if let Some(feedback) = &gold.feedback {
        font::draw_text(
            &mut pixels,
            GOLD_WIDTH,
            margin,
            GOLD_HEIGHT as i32 - LINE_HEIGHT as i32 - margin,
            1,
            GOLD,
            feedback,
        );
    } else {
        font::draw_text(
            &mut pixels,
            GOLD_WIDTH,
            margin,
            GOLD_HEIGHT as i32 - LINE_HEIGHT as i32 - margin,
            1,
            DIM,
            "TAB CYCLES. ENTER ACTS AND SLIDES. X RESETS.",
        );
    }
    pixels
}

fn put(pixels: &mut [u8], x: u32, y: u32, colour: [u8; 4]) {
    let at = ((y * GOLD_WIDTH + x) * 4) as usize;
    if at + 4 <= pixels.len() {
        pixels[at..at + 4].copy_from_slice(&colour);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn telemetry(tuning: &tuning::Tuning) -> Telemetry<'_> {
        Telemetry {
            tick: 4_200,
            position: glam::Vec3::new(-13.5, 73.0, 9.5),
            stance: "STAND",
            stamina: 87.0,
            credits: 250,
            bounty: 0,
            drones: 1,
            fliers: 1,
            base_total: 340,
            town_name: Some("STONEHAVEN".into()),
            town_centre: Some((0, 0)),
            stocks: [400.0, 200.0, 100.0, 40.0, 15.0, 2.0, 24.0, 31.0],
            tuning,
            world_hash: None,
        }
    }

    #[test]
    fn every_tab_renders_deterministically() {
        let tuning = tuning::Tuning::default();
        let telemetry = telemetry(&tuning);
        for index in 0..Tab::ALL.len() {
            let gold = Gold {
                open: true,
                tab_index: index,
                ..Gold::default()
            };
            assert_eq!(
                render_gold(&gold, &telemetry),
                render_gold(&gold, &telemetry),
                "{:?} is not a pure function of its inputs",
                gold.tab()
            );
        }
    }

    #[test]
    fn the_border_is_gold_and_the_game_panels_have_no_such_thing() {
        // The self-labelling rule: pixel (0,0) of this panel is the gold
        // border, and no diegetic panel draws one.
        let tuning = tuning::Tuning::default();
        let gold = Gold { open: true, ..Gold::default() };
        let pixels = render_gold(&gold, &telemetry(&tuning));
        assert_eq!(&pixels[0..4], &GOLD, "the top-left texel is not the signature");

        let shop_pixels = {
            let here = vx_world::town::home_site();
            let mut book = crate::economy::Economy::new();
            let market = book.market(&here, 0).clone();
            crate::shop::render_shop(
                &crate::shop::Shop::new(),
                None,
                &crate::wallet::Wallet::new(),
                &here,
                &market,
                &crate::garage::Garage::new(),
                &crate::arsenal::Arsenal::default(),
                &crate::intrusion::Intrusions::default(),
                1,
                &[],
            )
        };
        assert_ne!(
            &shop_pixels[0..4],
            &GOLD,
            "a diegetic panel opens with the operator's signature"
        );
    }

    #[test]
    fn a_dirty_tunable_reads_differently_from_a_stock_one() {
        let stock = tuning::Tuning::default();
        let mut bent = tuning::Tuning::default();
        assert!(bent.set("sprint_speed", 9.9));

        let gold = Gold {
            open: true,
            tab_index: 4, // Tuning
            ..Gold::default()
        };
        assert_ne!(
            render_gold(&gold, &telemetry(&stock)),
            render_gold(&gold, &telemetry(&bent)),
            "a changed tunable is invisible on the panel"
        );
    }

    #[test]
    fn tab_cycling_visits_every_tab_and_comes_home() {
        let mut gold = Gold::default();
        let start = gold.tab();
        let mut seen = vec![start];
        for _ in 0..Tab::ALL.len() {
            gold.cycle_tab();
            seen.push(gold.tab());
        }
        assert_eq!(seen.first(), seen.last(), "the cycle does not close");
        for tab in Tab::ALL {
            assert!(seen.contains(&tab), "{tab:?} is unreachable by bumper");
        }
    }

    #[test]
    fn the_cursor_clamps_to_the_rows_that_exist() {
        let tuning = tuning::Tuning::default();
        let telemetry = telemetry(&tuning);
        let mut gold = Gold { open: true, ..Gold::default() };
        let rows = gold.rows(&telemetry).len();
        gold.move_cursor(100, rows);
        assert_eq!(gold.cursor, rows - 1);
        gold.move_cursor(-100, rows);
        assert_eq!(gold.cursor, 0);
    }

    #[test]
    fn every_gold_label_is_drawable() {
        let tuning = tuning::Tuning::default();
        let telemetry = telemetry(&tuning);
        for index in 0..Tab::ALL.len() {
            let gold = Gold { open: true, tab_index: index, ..Gold::default() };
            for row in gold.rows(&telemetry) {
                for character in row.label.chars().chain(row.value.chars()) {
                    assert!(
                        font::knows(character),
                        "undrawable {character:?} in {:?}/{}",
                        gold.tab(),
                        row.label
                    );
                }
            }
        }
    }
}
