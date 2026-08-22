//! Bunkers: the geometry that makes every one unique.
//!
//! # Why quasirandom and not random
//!
//! Uniform noise has no grammar, so a dungeon built from it is unique the way
//! static is unique: every one differs in detail and none differs in
//! character. The golden ratio is the *most irrational* number — its continued
//! fraction is all ones, so no fraction approximates it well — and by the
//! equidistribution theorem the sequence `{n·φ}` never repeats, never clusters
//! and spreads as evenly as a sequence can. That is exactly the property
//! uniqueness needs, and it is plain arithmetic on a site hash: no rejection
//! loops, no stored state, nothing the worldgen rules forbid.
//!
//! # Variation between sites, invariance within one
//!
//! A bunker where every room rolled its own dice reads as noise. Instead the
//! site hash picks a handful of numbers — a proportion system, a tier, a
//! footprint, a bearing — and everything inside derives from those. Each
//! bunker is governed by one geometry the way each town is governed by one
//! plan, so a bunker reads as designed by somebody, and no two read as
//! designed by the same somebody.
//!
//! # The three systems
//!
//! | System | Ratio | Splits | Reads as |
//! |---|---|---|---|
//! | φ | 1.618 | two, at `1/φ`, recursing deeper into the smaller child | a coil: rooms shrinking inward |
//! | √2 | 1.414 | two, at `1/√2`, both children equal | the barracks grid |
//! | √3 | 1.732 | three at a time on the long axis | the industrial hive |
//!
//! # Where the note's plan bends to the block grid
//!
//! Two deliberate deviations, both because voxels are not paper:
//!
//! - **Orientation is the entry bearing, not a rotation of the plan.** Bunker
//!   `k` faces `k · 137.507…°`, and that angle places its hatch on the
//!   surface — so no two hatches on a ridge point the same way. The rooms
//!   themselves stay axis-aligned, because rotating a voxel BSP by an
//!   irrational angle aliases every wall into a staircase.
//! - **The pool furnishes rooms rather than replacing them.** The vocabulary
//!   cross-product is over thirty room shapes per system and the shell is
//!   identical in all of them, so the authored pieces are *furnishings*
//!   anchored inside a generated room. The guarantee the note asks for
//!   survives intact — every legal room size has at least one piece that fits
//!   — and it is still checked at authoring time by test.

use std::collections::HashSet;

use crate::gen::SEA_LEVEL;
use crate::seed::{finalise, unit};

/// Lattice cell for bunker siting. Wider than the town lattice: a bunker
/// should be a find, not scenery.
pub const CELL: i32 = 768;

/// Fraction of cells that hold anything at all.
const PRESENCE: f32 = 0.34;

/// The golden angle in degrees — the irrational rotation.
pub const GOLDEN_ANGLE: f32 = 137.507_77;

const PHI: f32 = 1.618_034;
const ROOT2: f32 = std::f32::consts::SQRT_2;
const ROOT3: f32 = 1.732_050_8;

/// Fibonacci: how an irrational proportion lives on a block grid. Adjacent
/// pairs (5:8, 8:13, 13:21) approximate φ, read as golden, and land exactly
/// on voxels.
pub const VOCABULARY: [i32; 6] = [3, 5, 8, 13, 21, 34];

/// How far a split may wander from its ratio. Enough to break symmetry,
/// not enough to break proportion.
const SPLIT_JITTER: f32 = 0.04;

/// Rock left between the surface and a bunker's top ceiling.
const ROOF_COVER: i32 = 4;

/// Total y a level occupies: floor, four clear, ceiling.
const LEVEL_HEIGHT: i32 = 6;

/// Clear headroom inside a level.
const HEADROOM: i32 = 4;

/// Dry land margin, as for towns.
const MIN_DRY: i32 = 3;

/// How much flatter than a mountainside a bunker plot has to be.
const MAX_RELIEF: i32 = 20;

/// The proportion system governing one site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum System {
    /// The civilian coil.
    Phi,
    /// The military grid — halving preserves the ratio, which is why paper
    /// sizes use it and why a barracks plan can be subdivided forever.
    Root2,
    /// The industrial hive: the hexagon's ratio, three ways at a time.
    Root3,
}

impl System {
    pub fn ratio(self) -> f32 {
        match self {
            System::Phi => PHI,
            System::Root2 => ROOT2,
            System::Root3 => ROOT3,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            System::Phi => "COIL",
            System::Root2 => "GRID",
            System::Root3 => "HIVE",
        }
    }
}

/// How big a bunker is, and therefore how much of everything it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// One room off a stairwell. Somebody's private shelter.
    Small,
    /// Several rooms off a spine. Worth clearing.
    Medium,
    /// Multiple levels. A landmark you plan an expedition to.
    Large,
}

impl Tier {
    /// How deep the partition recurses. Small is one decision, not a system.
    fn depth(self) -> u32 {
        match self {
            Tier::Small => 0,
            Tier::Medium => 4,
            Tier::Large => 6,
        }
    }

    fn levels(self) -> i32 {
        match self {
            Tier::Small => 1,
            Tier::Medium => 2,
            Tier::Large => 3,
        }
    }

    /// The widest footprint edge this tier draws from the vocabulary.
    fn span(self) -> i32 {
        match self {
            Tier::Small => 13,
            Tier::Medium => 21,
            Tier::Large => 34,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Tier::Small => "SHELTER",
            Tier::Medium => "COMPLEX",
            Tier::Large => "WORKS",
        }
    }
}

/// One bunker, derived whole from its lattice cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BunkerSite {
    pub centre: (i32, i32),
    /// Surface height over the centre — everything descends from here.
    pub ground: i32,
    pub tier: Tier,
    pub system: System,
    /// Footprint, both edges drawn from the vocabulary.
    pub width: i32,
    pub depth: i32,
    pub levels: i32,
    /// Where the hatch sits, as an angle around the centre. Bunker `k` takes
    /// `k · GOLDEN_ANGLE`, so orientations never repeat world-wide.
    pub bearing: f32,
    /// The surface column the way in breaks cover at, and the height there.
    /// Sampled once at siting, where the height field is in hand, so every
    /// later query stays pure in the site.
    pub hatch: (i32, i32),
    pub hatch_ground: i32,
    /// Columns the entry stair runs from the hatch toward the works. Long
    /// enough that it never drops more than one block per pace, whatever the
    /// hatch's ground turned out to be.
    pub entry_run: i32,
    pub seed: u64,
}

impl BunkerSite {
    /// Half-extent of the footprint plus the entry's reach, in blocks. What
    /// "near this bunker" means to anything outside this module.
    pub fn reach(&self) -> i32 {
        self.width.max(self.depth) / 2 + self.entry_run + 4
    }

    /// The y of level `level`'s floor.
    pub fn level_base(&self, level: i32) -> i32 {
        self.ground - ROOF_COVER - LEVEL_HEIGHT + 1 - level * LEVEL_HEIGHT
    }

}

/// The shortest the entry staircase may be, before the drop it has to cover
/// is taken into account.
const ENTRY_RUN: i32 = 10;

/// The stairwell: the invariant column every level shares. Eight along x for
/// the run, five along z — three of them the run itself, the rest landings.
const STAIR_W: i32 = 8;
const STAIR_D: i32 = 5;

/// How many of the well's lanes the descending run takes. The rest stay
/// plated at every floor, so stepping out of a room lands you on ground
/// rather than in the shaft.
const RUN_LANES: i32 = 3;

/// A corridor the partition asked for: two points to join, and which axis
/// the elbow turns on first.
type Corridor = ((i32, i32), (i32, i32), bool);

/// A rectangle of columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rect {
    x: i32,
    z: i32,
    w: i32,
    d: i32,
}

impl Rect {
    fn centre(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.z + self.d / 2)
    }
}

/// One hash stream per property, in the town module's idiom.
fn hash01(seed: u64, salt: u64, a: i32, b: i32) -> f32 {
    unit(finalise(
        seed ^ salt
            ^ (a as i64 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ (b as i64 as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f),
    ))
}

/// The bunker in one lattice cell, or nothing.
///
/// `natural_height_at` must be the pre-town height field, exactly as town
/// siting requires: a bunker sited against ground another structure already
/// flattened is how a height field starts feeding itself.
fn site_in_cell(
    seed: u64,
    cell_x: i32,
    cell_z: i32,
    natural_height_at: &impl Fn(i32, i32) -> i32,
) -> Option<BunkerSite> {
    let key = |salt: u64| hash01(seed, salt, cell_x, cell_z);

    if key(0x11) > PRESENCE {
        return None;
    }

    // Jitter inside the middle half, which keeps two bunkers at least CELL/2
    // apart and makes cross-cell rejection unnecessary — the same trick towns
    // use, for the same reason.
    let quarter = CELL / 4;
    let centre = (
        cell_x * CELL + quarter + (key(0x12) * quarter as f32 * 2.0) as i32,
        cell_z * CELL + quarter + (key(0x13) * quarter as f32 * 2.0) as i32,
    );

    let ground = natural_height_at(centre.0, centre.1);
    if ground <= SEA_LEVEL + MIN_DRY {
        return None;
    }

    // Never under a town. Checked from the site alone rather than per chunk,
    // so the answer cannot depend on what happens to be loaded.
    let towns = crate::town::towns_near(seed, centre, crate::town::CELL, natural_height_at);
    let tier = match key(0x14) {
        roll if roll < 0.55 => Tier::Small,
        roll if roll < 0.88 => Tier::Medium,
        _ => Tier::Large,
    };
    let span = tier.span();
    for town in &towns {
        let clearance = town.core_half + crate::town::SKIRT + span;
        if (town.centre.0 - centre.0).abs() < clearance
            && (town.centre.1 - centre.1).abs() < clearance
        {
            return None;
        }
    }

    // A bunker is dug, not levelled, so it only asks that the roof over it be
    // roughly even — a hatch on a cliff face would hang in the air.
    let mut lowest = ground;
    let mut highest = ground;
    for (dx, dz) in [
        (-span / 2, -span / 2),
        (span / 2, -span / 2),
        (-span / 2, span / 2),
        (span / 2, span / 2),
    ] {
        let corner = natural_height_at(centre.0 + dx, centre.1 + dz);
        lowest = lowest.min(corner);
        highest = highest.max(corner);
    }
    if highest - lowest > MAX_RELIEF {
        return None;
    }

    // Tier weights the system draw: shelters coil, works branch.
    let roll = key(0x15);
    let system = match tier {
        Tier::Small => {
            if roll < 0.60 {
                System::Phi
            } else if roll < 0.88 {
                System::Root2
            } else {
                System::Root3
            }
        }
        Tier::Medium => {
            if roll < 0.40 {
                System::Phi
            } else if roll < 0.78 {
                System::Root2
            } else {
                System::Root3
            }
        }
        Tier::Large => {
            if roll < 0.25 {
                System::Phi
            } else if roll < 0.60 {
                System::Root2
            } else {
                System::Root3
            }
        }
    };

    // The footprint: the tier's span, paired with the vocabulary value
    // nearest `span / ratio`. The vocabulary is coarse enough that the three
    // systems often land on the same outline — the systems separate *inside*,
    // which is where a plan is actually read.
    let width = span;
    let want = width as f32 / system.ratio();
    let depth = *VOCABULARY
        .iter()
        .filter(|value| **value >= 5 && **value <= span)
        .min_by_key(|value| ((**value as f32 - want).abs() * 1000.0) as i32)
        .unwrap_or(&5);

    // The world index: lattice order, so the golden angle sequence is a
    // property of the world rather than of the order chunks happen to load.
    // The world index — lattice order, so the golden-angle sequence belongs
    // to the world rather than to whatever order chunks happened to load.
    //
    // Computed in f64 and only then narrowed: `k · 137.5°` runs to tens of
    // millions of degrees before the modulo, and an f32 of that size has an
    // ulp of about four degrees — which would quietly collapse the whole
    // irrational-rotation argument into a handful of repeated bearings.
    let index = (cell_x as i64)
        .wrapping_mul(65_536)
        .wrapping_add(cell_z as i64) as f64;
    let bearing = ((index * GOLDEN_ANGLE as f64).rem_euclid(360.0) as f32).to_radians();

    // The way in. Its length answers the ground it found: the stair drops at
    // most one block per pace, so a hatch on higher ground simply starts
    // further out rather than turning into a ladder nothing can climb.
    let half = width.max(depth) / 2;
    let deepest = ground - (ground - ROOF_COVER - LEVEL_HEIGHT + 1);
    let mut entry_run = ENTRY_RUN.max(deepest + 2);
    let (mut hatch, mut hatch_ground) = ((0, 0), ground);
    for _ in 0..6 {
        hatch = (
            centre.0 + (bearing.cos() * (half + entry_run) as f32).round() as i32,
            centre.1 + (bearing.sin() * (half + entry_run) as f32).round() as i32,
        );
        hatch_ground = natural_height_at(hatch.0, hatch.1);
        let drop = hatch_ground - (ground - ROOF_COVER - LEVEL_HEIGHT + 1);
        if drop + 2 <= entry_run {
            break;
        }
        entry_run = drop + 2;
    }
    // The way in has to come *down*. Ground that falls away toward the
    // bearing would put the approach above the hillside it is supposed to be
    // cut into — a corridor hanging in the sky. Rather than bend the stair,
    // refuse the site: bunkers are meant to be rare, and one that cannot be
    // entered is worse than one that is not there.
    let top_floor = ground - ROOF_COVER - LEVEL_HEIGHT + 1;
    if hatch_ground <= SEA_LEVEL + MIN_DRY || hatch_ground < top_floor + 2 {
        return None;
    }

    Some(BunkerSite {
        centre,
        ground,
        tier,
        system,
        width,
        depth,
        levels: tier.levels(),
        bearing,
        hatch,
        hatch_ground,
        entry_run,
        seed: seed
            ^ (cell_x as i64 as u64).wrapping_mul(0x2545_f491_4f6c_dd1d)
            ^ (cell_z as i64 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15),
    })
}

/// Every bunker whose works could reach the column box `min..=max`.
pub fn bunkers_overlapping(
    seed: u64,
    min: (i32, i32),
    max: (i32, i32),
    natural_height_at: &impl Fn(i32, i32) -> i32,
) -> Vec<BunkerSite> {
    let margin = Tier::Large.span() + ENTRY_RUN + 8;
    let lo_x = (min.0 - margin).div_euclid(CELL);
    let hi_x = (max.0 + margin).div_euclid(CELL);
    let lo_z = (min.1 - margin).div_euclid(CELL);
    let hi_z = (max.1 + margin).div_euclid(CELL);

    let mut found = Vec::new();
    for cell_x in lo_x..=hi_x {
        for cell_z in lo_z..=hi_z {
            let Some(site) = site_in_cell(seed, cell_x, cell_z, natural_height_at) else {
                continue;
            };
            let reach = site.reach();
            if site.centre.0 + reach >= min.0
                && site.centre.0 - reach <= max.0
                && site.centre.1 + reach >= min.1
                && site.centre.1 - reach <= max.1
            {
                found.push(site);
            }
        }
    }
    found
}

/// Bunkers near a column, nearest first, loading nothing.
pub fn bunkers_near(
    seed: u64,
    at: (i32, i32),
    radius: i32,
    natural_height_at: &impl Fn(i32, i32) -> i32,
) -> Vec<BunkerSite> {
    let lo_x = (at.0 - radius).div_euclid(CELL);
    let hi_x = (at.0 + radius).div_euclid(CELL);
    let lo_z = (at.1 - radius).div_euclid(CELL);
    let hi_z = (at.1 + radius).div_euclid(CELL);
    let reach = (radius as i64) * (radius as i64);

    let mut found: Vec<BunkerSite> = Vec::new();
    for cell_x in lo_x..=hi_x {
        for cell_z in lo_z..=hi_z {
            let Some(site) = site_in_cell(seed, cell_x, cell_z, natural_height_at) else {
                continue;
            };
            let dx = (site.centre.0 - at.0) as i64;
            let dz = (site.centre.1 - at.1) as i64;
            if dx * dx + dz * dz <= reach {
                found.push(site);
            }
        }
    }
    found.sort_by_key(|site| {
        let dx = (site.centre.0 - at.0) as i64;
        let dz = (site.centre.1 - at.1) as i64;
        dx * dx + dz * dz
    });
    found
}

/// Does a column stand over any of these bunkers' works?
///
/// What the cave carve and the ore lattice ask, so neither opens a hole in a
/// sealed shell nor hides a vein under a floor nothing can reach.
pub fn works_contains(sites: &[BunkerSite], x: i32, z: i32) -> bool {
    sites.iter().any(|site| {
        let reach = site.reach();
        (x - site.centre.0).abs() <= reach && (z - site.centre.1).abs() <= reach
    })
}

// ---------------------------------------------------------------------------
// The layout: golden BSP, the pool, and the stairs that thread them
// ---------------------------------------------------------------------------

/// What a bunker cell is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cell {
    /// The outer skin, and every floor and ceiling. Very hard on purpose:
    /// digging in is a choice you can make and usually regret.
    Shell,
    /// An internal partition. Ordinary metal — cutting between two rooms you
    /// are already standing in should not be the hard part.
    Panel,
    /// Open air inside the works.
    Air,
    /// A workbench or radio desk.
    Bench,
    /// A cot.
    Cot,
    /// Crates and drums.
    Crate,
    /// Machinery: the generator, the pumps.
    Machine,
    /// A supply cache — the reason to come down here.
    Cache,
}

/// What a room is for. The must-place kinds are the anchors every bunker has,
/// the way every town has its counter and its mast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomKind {
    Stair,
    Generator,
    Radio,
    Bunk,
    Store,
    Hall,
}

/// One room, in world columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Room {
    pub level: i32,
    pub x: i32,
    pub z: i32,
    pub w: i32,
    pub d: i32,
    pub kind: RoomKind,
    /// Does this room hold a cache, and where.
    pub cache: Option<(i32, i32)>,
}

impl Room {
    pub fn centre(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.z + self.d / 2)
    }
}

/// An authored furnishing, stamped inside a generated room.
///
/// Every piece deeper than one row keeps its last row clear, so furniture
/// always has a front and can never ring an air pocket nothing walks into.
/// A one-cell piece is a single object standing in a room and is exempt.
///
/// Rows run +z, characters +x, layers up from the floor — the town
/// blueprints' convention, so one authoring habit covers both.
struct Piece {
    kind: RoomKind,
    layers: &'static [&'static [&'static str]],
}

impl Piece {
    fn extent(&self) -> (i32, i32) {
        let depth = self.layers.first().map_or(0, |rows| rows.len()) as i32;
        let width = self
            .layers
            .first()
            .and_then(|rows| rows.first())
            .map_or(0, |line| line.len()) as i32;
        (width, depth)
    }
}

/// The pool. `M` machinery, `B` bench, `K` cot, `D` crate, `.` clear.
const POOL: &[Piece] = &[
    // The generator: every bunker has one, and it is the loudest thing in a
    // silent place.
    Piece {
        kind: RoomKind::Generator,
        layers: &[&["MMM", "M.M", "..."], &["M.M", "...", "..."]],
    },
    Piece {
        kind: RoomKind::Generator,
        layers: &[&["MM", ".."], &["M.", ".."]],
    },
    // The smallest shelter still gets its generator: a single unit, which is
    // what makes "every legal room size has a piece" true rather than nearly
    // true. The pool is matched to the vocabulary by hand, and the test that
    // proves it fails at authoring time.
    Piece {
        kind: RoomKind::Generator,
        layers: &[&["M"]],
    },
    // Radio benches and breaker boxes.
    Piece {
        kind: RoomKind::Radio,
        layers: &[&["BBB", "D.D", "..."]],
    },
    Piece {
        kind: RoomKind::Radio,
        layers: &[&["BB", ".."], &["B.", ".."]],
    },
    Piece {
        kind: RoomKind::Radio,
        layers: &[&["B"]],
    },
    // Somebody lived here.
    Piece {
        kind: RoomKind::Bunk,
        layers: &[&["K.K", "K.K", "..."]],
    },
    Piece {
        kind: RoomKind::Bunk,
        layers: &[&["K", "."]],
    },
    Piece {
        kind: RoomKind::Bunk,
        layers: &[&["K"]],
    },
    // Stores.
    Piece {
        kind: RoomKind::Store,
        layers: &[&["DDD", "..."], &["D.D", "..."]],
    },
    Piece {
        kind: RoomKind::Store,
        layers: &[&["D"]],
    },
    // A hall keeps its floor clear; the columns are structure, not furniture.
    Piece {
        kind: RoomKind::Hall,
        layers: &[&["M...M", ".....", ".....", ".....", "M...M"]],
    },
    Piece {
        kind: RoomKind::Hall,
        layers: &[&["D"]],
    },
];

/// The whole works, resolved to blocks.
///
/// Built once and read many times: a full voxel map rather than a set of
/// rules re-evaluated per block, because the rules are recursive and the
/// works are small. Ordered, so two runs iterate identically and a layout
/// can be hashed.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    cells: std::collections::BTreeMap<(i32, i32, i32), Cell>,
    pub rooms: Vec<Room>,
    pub hatch: (i32, i32),
}

impl Layout {
    /// What stands at a world position, if anything.
    pub fn cell_at(&self, x: i32, y: i32, z: i32) -> Option<Cell> {
        self.cells.get(&(x, y, z)).copied()
    }

    /// Every cell, in a fixed order.
    pub fn cells(&self) -> impl Iterator<Item = (&(i32, i32, i32), &Cell)> {
        self.cells.iter()
    }

    /// A fingerprint of the whole arrangement. Two bunkers with the same
    /// hash are the same bunker.
    pub fn fingerprint(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for ((x, y, z), cell) in &self.cells {
            for value in [*x as i64 as u64, *y as i64 as u64, *z as i64 as u64, *cell as u64] {
                hash = finalise(hash ^ value.wrapping_mul(0x100_0000_01b3));
            }
        }
        hash
    }

    fn put(&mut self, x: i32, y: i32, z: i32, cell: Cell) {
        self.cells.insert((x, y, z), cell);
    }

    /// Lay a floor plate, unless open space is already there.
    ///
    /// The entry is cut *into* finished works, and the works were there
    /// first: paving over a stairwell's air seals the shaft, which is the one
    /// failure that turns a whole lower floor into loot nothing can collect.
    fn tread(&mut self, x: i32, y: i32, z: i32) {
        if self.cells.get(&(x, y, z)) != Some(&Cell::Air) {
            self.cells.insert((x, y, z), Cell::Shell);
        }
    }
}

/// Build the whole bunker. Pure in the site, like every other derived thing.
pub fn layout(site: &BunkerSite) -> Layout {
    let mut plan = Layout {
        hatch: site.hatch,
        ..Layout::default()
    };

    // The stairwell: the invariant column, identical on every level, which is
    // what lets the descent be a switchback instead of a shaft.
    //
    // It sits in the corner the entry arrives from, not the middle. A shaft
    // through the centre of a plan eats the rooms the plan is *for* — and
    // walking in off the surface straight onto the stairhead is how a real
    // building is entered.
    let (stair_w, stair_d) = if site.levels > 1 {
        (STAIR_W, STAIR_D)
    } else {
        // Nothing to descend to: the landing is all the stairwell it needs.
        (3, 3)
    };
    let smallest = Rect {
        x: 0,
        z: 0,
        w: ((site.width as f32 / site.system.ratio().powi(site.levels - 1)).round() as i32)
            .max(STAIR_W + 6),
        d: ((site.depth as f32 / site.system.ratio().powi(site.levels - 1)).round() as i32)
            .max(STAIR_D + 6),
    };
    let corner = |extent: i32, inner: i32, toward: f32| -> i32 {
        if toward >= 0.0 {
            extent / 2 - inner
        } else {
            -extent / 2
        }
    };
    let stair = Rect {
        x: site.centre.0 + corner(smallest.w, stair_w, site.bearing.cos()),
        z: site.centre.1 + corner(smallest.d, stair_d, site.bearing.sin()),
        w: stair_w,
        d: stair_d,
    };

    let mut interiors: Vec<(i32, HashSet<(i32, i32)>)> = Vec::new();
    for level in 0..site.levels {
        let base = site.level_base(level);

        // Each floor down is scaled by 1/P: the taper the note asks for, and
        // the reason a large works reads as an inverted ziggurat.
        let shrink = site.system.ratio().powi(level);
        let footprint = Rect {
            x: 0,
            z: 0,
            w: ((site.width as f32 / shrink).round() as i32).max(STAIR_W + 6),
            d: ((site.depth as f32 / shrink).round() as i32).max(STAIR_D + 6),
        };
        let footprint = Rect {
            x: site.centre.0 - footprint.w / 2,
            z: site.centre.1 - footprint.d / 2,
            ..footprint
        };

        let mut leaves = Vec::new();
        let mut corridors: Vec<Corridor> = Vec::new();
        partition(
            footprint,
            site,
            level,
            site.tier.depth(),
            1,
            &mut leaves,
            &mut corridors,
        );

        // Leaves become rooms, centred in their cell so a corridor aimed at
        // the cell's middle arrives in the room's middle. A leaf overlapping
        // the stairwell is given up to it.
        let mut interior: HashSet<(i32, i32)> = HashSet::new();
        let mut level_rooms: Vec<Room> = Vec::new();
        for (index, leaf) in leaves.iter().enumerate() {
            let Some(room) = room_in(*leaf, site, level, index as u64) else {
                continue;
            };
            if overlaps(room, stair) {
                continue;
            }
            for x in room.x..room.x + room.w {
                for z in room.z..room.z + room.d {
                    interior.insert((x, z));
                }
            }
            level_rooms.push(Room {
                level,
                x: room.x,
                z: room.z,
                w: room.w,
                d: room.d,
                kind: RoomKind::Hall,
                cache: None,
            });
        }

        // The anchors: the biggest room on the top floor is the generator
        // room, and everything else takes a kind from its own hash.
        if let Some(biggest) = level_rooms
            .iter_mut()
            .max_by_key(|room| room.w * room.d)
            .filter(|_| level == 0)
        {
            biggest.kind = RoomKind::Generator;
        }
        for (index, room) in level_rooms.iter_mut().enumerate() {
            if room.kind == RoomKind::Generator {
                continue;
            }
            let roll = hash01(site.seed, 0x40 + index as u64, level, index as i32);
            room.kind = if room.w * room.d >= 21 * 8 {
                RoomKind::Hall
            } else if roll < 0.34 {
                RoomKind::Store
            } else if roll < 0.67 {
                RoomKind::Bunk
            } else {
                RoomKind::Radio
            };
        }

        // The stairwell's own floor plate.
        for x in stair.x..stair.x + stair.w {
            for z in stair.z..stair.z + stair.d {
                interior.insert((x, z));
            }
        }

        // Corridors: one-wide runs threading the partition tree, which is
        // what makes connectivity a property of the construction rather than
        // something to check afterwards and hope.
        for (from, to, bend_first_x) in corridors {
            for column in elbow(from, to, bend_first_x) {
                interior.insert(column);
            }
        }
        // And the spine: every room reaches the stairwell. The partition tree
        // already connects siblings, but "connected by construction" has to
        // mean *proved*, not argued — a room you cannot walk to is loot the
        // ledger promises and nothing can collect. The star is the proof; the
        // tree's corridors are what make it read as a building rather than a
        // hub and spokes.
        let hub = (stair.x + stair.w / 2, stair.z + stair.d / 2);
        for room in &level_rooms {
            let bend = hash01(site.seed, 0x50, level, room.x) < 0.5;
            for column in elbow(room.centre(), hub, bend) {
                interior.insert(column);
            }
        }

        // Floors, air and ceilings — except inside the stairwell, which is one
        // open shaft from the bottom floor to the top ceiling. Plating it per
        // level would seal the very column the levels are threaded on. What
        // it keeps is a *landing* at each floor: solid ground to step out
        // onto, beside the run rather than across it.
        let in_run = |x: i32, z: i32| {
            (stair.x..stair.x + stair.w).contains(&x)
                && (stair.z..stair.z + RUN_LANES.min(stair.d)).contains(&z)
        };
        let in_well = |x: i32, z: i32| {
            (stair.x..stair.x + stair.w).contains(&x) && (stair.z..stair.z + stair.d).contains(&z)
        };
        for &(x, z) in &interior {
            let shaft = in_well(x, z);
            let bottom = level == site.levels - 1;
            if !shaft || !in_run(x, z) || bottom {
                plan.put(x, base, z, Cell::Shell);
            }
            for step in 1..=HEADROOM {
                plan.put(x, base + step, z, Cell::Air);
            }
            if !shaft || level == 0 {
                plan.put(x, base + HEADROOM + 1, z, Cell::Shell);
            } else {
                plan.put(x, base + HEADROOM + 1, z, Cell::Air);
            }
        }

        // Walls: every column touching the inside. A column with open air on
        // both sides of an axis is a partition between two rooms — ordinary
        // metal. Everything else is the skin.
        let mut walls: Vec<((i32, i32), Cell)> = Vec::new();
        for &(x, z) in &interior {
            for (dx, dz) in [
                (1, 0),
                (-1, 0),
                (0, 1),
                (0, -1),
                (1, 1),
                (1, -1),
                (-1, 1),
                (-1, -1),
            ] {
                let column = (x + dx, z + dz);
                if interior.contains(&column) {
                    continue;
                }
                let flanked = (interior.contains(&(column.0 + 1, column.1))
                    && interior.contains(&(column.0 - 1, column.1)))
                    || (interior.contains(&(column.0, column.1 + 1))
                        && interior.contains(&(column.0, column.1 - 1)));
                walls.push((column, if flanked { Cell::Panel } else { Cell::Shell }));
            }
        }
        for ((x, z), cell) in walls {
            for step in 0..=HEADROOM + 1 {
                plan.put(x, base + step, z, cell);
            }
        }

        // Furnishings last: the pool decorates what the mathematics placed.
        for room in &mut level_rooms {
            furnish(&mut plan, site, room, base);
        }
        plan.rooms.extend(level_rooms.iter().copied());
        plan.rooms.push(Room {
            level,
            x: stair.x,
            z: stair.z,
            w: stair.w,
            d: stair.d,
            kind: RoomKind::Stair,
            cache: None,
        });
        interiors.push((level, interior));
    }

    // The switchback between levels, cut through the floor plate it lands on.
    for level in 0..site.levels - 1 {
        let base = site.level_base(level);
        let descending = level % 2 == 0;
        for step in 0..=LEVEL_HEIGHT {
            let x = if descending {
                stair.x + step.min(stair.w - 1)
            } else {
                stair.x + stair.w - 1 - step.min(stair.w - 1)
            };
            let floor = base - step;
            // The run takes only part of the well's depth. A staircase cut
            // across the full width would land a solid step exactly at the
            // lower floor's walking height and wall the level in half — the
            // open lanes beside it are what keep the stairwell a stairwell
            // rather than a plug.
            for z in stair.z..stair.z + RUN_LANES.min(stair.d) {
                plan.put(x, floor, z, Cell::Shell);
                for clear in 1..=HEADROOM {
                    plan.put(x, floor + clear, z, Cell::Air);
                }
            }
        }
    }

    // The way in: a stair from the hatch down to the top floor, then level
    // until it meets the works.
    carve_entry(&mut plan, site, stair);

    // Caches, the reason to be down here at all. Every bunker holds at least
    // one, and the rest are the site's business.
    let mut placed = 0;
    let rooms: Vec<usize> = (0..plan.rooms.len())
        .filter(|index| plan.rooms[*index].kind != RoomKind::Stair)
        .collect();
    for (order, index) in rooms.iter().enumerate() {
        let room = plan.rooms[*index];
        let roll = hash01(site.seed, 0x60, room.x, room.z);
        let wanted = roll < 0.45 || (order + 1 == rooms.len() && placed == 0);
        if !wanted {
            continue;
        }
        // A Fibonacci offset in from the corner: the same proportion the
        // rooms are built on, at the scale a person walks.
        let spot = (
            room.x + 1 + (VOCABULARY[0] % (room.w - 2).max(1)),
            room.z + 1 + (VOCABULARY[0] % (room.d - 2).max(1)),
        );
        let base = site.level_base(room.level);
        plan.put(spot.0, base + 1, spot.1, Cell::Cache);
        plan.rooms[*index].cache = Some(spot);
        placed += 1;
    }

    plan
}

/// Golden BSP. Splits land at `1/P` of the parent, jittered a few percent,
/// on the longer axis — which for a P-proportioned rectangle alternates by
/// itself, and never produces the slivers a uniform split does.
fn partition(
    rect: Rect,
    site: &BunkerSite,
    level: i32,
    depth: u32,
    node: u64,
    leaves: &mut Vec<Rect>,
    corridors: &mut Vec<Corridor>,
) -> (i32, i32) {
    let smallest = VOCABULARY[0] + 2;
    if depth == 0 || (rect.w < smallest * 2 && rect.d < smallest * 2) {
        leaves.push(rect);
        return rect.centre();
    }

    let along_x = rect.w >= rect.d;
    let extent = if along_x { rect.w } else { rect.d };
    let jitter = (hash01(site.seed, 0x30 + node, level, extent) - 0.5) * 2.0 * SPLIT_JITTER;

    let cut = |fraction: f32| -> i32 {
        ((extent as f32) * (fraction + jitter)).round() as i32
    };

    let mut parts: Vec<Rect> = Vec::new();
    match site.system {
        // The hive: three ways at a time on the long axis.
        System::Root3 if extent >= smallest * 3 => {
            let first = cut(1.0 / 3.0).clamp(smallest, extent - smallest * 2);
            let second = cut(2.0 / 3.0).clamp(first + smallest, extent - smallest);
            parts.push(slice(rect, along_x, 0, first));
            parts.push(slice(rect, along_x, first, second - first));
            parts.push(slice(rect, along_x, second, extent - second));
        }
        _ => {
            let ratio = 1.0 / site.system.ratio();
            let first = cut(ratio).clamp(smallest, extent - smallest);
            parts.push(slice(rect, along_x, 0, first));
            parts.push(slice(rect, along_x, first, extent - first));
        }
    }

    // How the children recurse *is* the grammar. The coil drives deeper into
    // its smaller child and leaves the larger one whole, so rooms shrink as
    // the plan turns inward; the grid halves both equally; the hive branches
    // wide and shallow.
    let reps: Vec<(i32, i32)> = parts
        .iter()
        .enumerate()
        .map(|(index, part)| {
            let child_depth = match site.system {
                System::Phi => {
                    if index == 0 {
                        depth.saturating_sub(2)
                    } else {
                        depth - 1
                    }
                }
                System::Root2 => depth - 1,
                System::Root3 => depth.saturating_sub(2),
            };
            partition(
                *part,
                site,
                level,
                child_depth,
                node * 4 + index as u64 + 1,
                leaves,
                corridors,
            )
        })
        .collect();

    let bend = hash01(site.seed, 0x38 + node, level, rect.x) < 0.5;
    for pair in reps.windows(2) {
        corridors.push((pair[0], pair[1], bend));
    }

    // What the parent hands upward: the coil returns its inner room, the hive
    // its middle, the grid its first.
    match site.system {
        System::Phi => *reps.first().unwrap_or(&rect.centre()),
        System::Root3 => *reps.get(reps.len() / 2).unwrap_or(&rect.centre()),
        System::Root2 => *reps.first().unwrap_or(&rect.centre()),
    }
}

/// One part of a split rectangle.
fn slice(rect: Rect, along_x: bool, offset: i32, extent: i32) -> Rect {
    if along_x {
        Rect {
            x: rect.x + offset,
            w: extent,
            ..rect
        }
    } else {
        Rect {
            z: rect.z + offset,
            d: extent,
            ..rect
        }
    }
}

/// The room inside a partition cell: vocabulary dimensions, centred, with at
/// least one block of wall left on every side.
fn room_in(leaf: Rect, site: &BunkerSite, level: i32, index: u64) -> Option<Rect> {
    let pick = |available: i32, salt: u64| -> Option<i32> {
        let usable = available - 2;
        let mut best = None;
        for value in VOCABULARY {
            if value <= usable {
                best = Some(value);
            }
        }
        let largest = best?;
        // Sometimes take one size down: a plan where every room is as big as
        // it could be reads as a packing problem rather than a building.
        let smaller = VOCABULARY
            .iter()
            .rev()
            .find(|value| **value < largest)
            .copied();
        match smaller {
            Some(value) if hash01(site.seed, salt, level, largest) < 0.3 => Some(value),
            _ => Some(largest),
        }
    };
    let w = pick(leaf.w, 0x70 + index)?;
    let d = pick(leaf.d, 0x78 + index)?;
    Some(Rect {
        x: leaf.x + (leaf.w - w) / 2,
        z: leaf.z + (leaf.d - d) / 2,
        w,
        d,
    })
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.z < b.z + b.d && b.z < a.z + a.d
}

/// The columns of an L-shaped corridor. The bend sits a Fibonacci step along
/// the first run rather than at the plain corner, so corridors read as turns
/// somebody chose.
fn elbow(from: (i32, i32), to: (i32, i32), x_first: bool) -> Vec<(i32, i32)> {
    let mut columns = Vec::new();
    let stride = VOCABULARY[2];
    let bend = if x_first {
        let reach = (to.0 - from.0).abs();
        let along = if reach > stride { stride } else { reach };
        (from.0 + along * (to.0 - from.0).signum(), from.1)
    } else {
        let reach = (to.1 - from.1).abs();
        let along = if reach > stride { stride } else { reach };
        (from.0, from.1 + along * (to.1 - from.1).signum())
    };
    for (a, b) in [(from, bend), (bend, (to.0, bend.1)), ((to.0, bend.1), to)] {
        let steps = (b.0 - a.0).abs().max((b.1 - a.1).abs());
        for step in 0..=steps {
            let x = a.0 + (b.0 - a.0).signum() * step.min((b.0 - a.0).abs());
            let z = a.1 + (b.1 - a.1).signum() * step.min((b.1 - a.1).abs());
            columns.push((x, z));
        }
    }
    columns
}

/// Stamp an authored piece into a room.
fn furnish(plan: &mut Layout, site: &BunkerSite, room: &Room, base: i32) {
    let inner = (room.w - 2, room.d - 2);
    let candidates: Vec<&Piece> = POOL
        .iter()
        .filter(|piece| {
            let (w, d) = piece.extent();
            piece.kind == room.kind && w <= inner.0 && d <= inner.1
        })
        .collect();
    if candidates.is_empty() {
        return;
    }
    let roll = hash01(site.seed, 0x80, room.x, room.z);
    let piece = candidates[(roll * candidates.len() as f32) as usize % candidates.len()];
    let (w, d) = piece.extent();
    let origin = (
        room.x + 1 + (inner.0 - w) / 2,
        room.z + 1 + (inner.1 - d) / 2,
    );
    for (layer, rows) in piece.layers.iter().enumerate() {
        for (row, line) in rows.iter().enumerate() {
            for (column, glyph) in line.bytes().enumerate() {
                let cell = match glyph {
                    b'M' => Cell::Machine,
                    b'B' => Cell::Bench,
                    b'K' => Cell::Cot,
                    b'D' => Cell::Crate,
                    _ => continue,
                };
                plan.put(
                    origin.0 + column as i32,
                    base + 1 + layer as i32,
                    origin.1 + row as i32,
                    cell,
                );
            }
        }
    }
}

/// The way in: a stair descending from the hatch to the top floor.
fn carve_entry(plan: &mut Layout, site: &BunkerSite, stair: Rect) {
    let target = site.level_base(0);
    let half = site.width.max(site.depth) / 2;
    let hub = (stair.x + stair.w / 2, stair.z + stair.d / 2);
    let reach = half + site.entry_run;
    let (dx, dz) = (site.bearing.cos(), site.bearing.sin());

    let mut floor = site.hatch_ground;
    let mut last: Option<(i32, i32)> = None;
    for step in 0..=reach {
        let along = (reach - step) as f32;
        let column = (
            site.centre.0 + (dx * along).round() as i32,
            site.centre.1 + (dz * along).round() as i32,
        );
        if last == Some(column) {
            continue;
        }
        last = Some(column);
        floor = (floor - 1).max(target);

        // A three-wide cut, so the stair is a passage rather than a slot the
        // player has to thread.
        for side in -1..=1 {
            let (wx, wz) = if dx.abs() >= dz.abs() {
                (column.0, column.1 + side)
            } else {
                (column.0 + side, column.1)
            };
            plan.tread(wx, floor, wz);
            for clear in 1..=3 {
                plan.put(wx, floor + clear, wz, Cell::Air);
            }
            // A rim, so the passage does not simply open into loose dirt.
            for rim in -2..=2i32 {
                if rim.abs() < 2 {
                    continue;
                }
                let (rx, rz) = if dx.abs() >= dz.abs() {
                    (column.0, column.1 + rim)
                } else {
                    (column.0 + rim, column.1)
                };
                for clear in 0..=4 {
                    plan.cells.entry((rx, floor + clear, rz)).or_insert(Cell::Shell);
                }
            }
        }
    }

    // And in from wherever the descent ended to the stairhead itself. The
    // run keeps descending along the way, and whatever drop is left over when
    // it arrives is finished inside the stairwell — which is the one part of
    // a bunker built to move people vertically. Trusting the siting loop to
    // have guessed a long enough approach would leave a stair hanging above
    // its own building on exactly the ground that is hardest to test.
    let mouth = (
        site.centre.0 + (dx * half as f32).round() as i32,
        site.centre.1 + (dz * half as f32).round() as i32,
    );
    for column in elbow(mouth, hub, dx.abs() >= dz.abs()) {
        floor = (floor - 1).max(target);
        plan.tread(column.0, floor, column.1);
        for clear in 1..=3 {
            plan.put(column.0, floor + clear, column.1, Cell::Air);
        }
    }
    let mut lane = 0;
    while floor > target {
        let z = stair.z + lane % stair.d;
        for step in 0..stair.w {
            if floor == target {
                break;
            }
            floor -= 1;
            let x = if lane % 2 == 0 {
                stair.x + step
            } else {
                stair.x + stair.w - 1 - step
            };
            plan.tread(x, floor, z);
            for clear in 1..=3 {
                plan.put(x, floor + clear, z, Cell::Air);
            }
        }
        lane += 1;
        if lane > stair.d * 4 {
            break;
        }
    }
}

/// Which block a cell is made of.
pub fn block_for(cell: Cell, blocks: &crate::gen::TerrainBlocks) -> Option<vx_core::BlockId> {
    Some(match cell {
        Cell::Shell => blocks.bunker_shell,
        Cell::Panel => blocks.metal_wall,
        Cell::Air => return None,
        Cell::Bench => blocks.catwalk,
        Cell::Cot => blocks.plank,
        Cell::Crate => blocks.container,
        Cell::Machine => blocks.rusted_metal,
        Cell::Cache => blocks.supply_cache,
    })
}

/// Cut every bunker reaching this chunk into it.
///
/// Runs after the columns are filled, like the town stamp — and unlike it,
/// this one also writes *air*, because a bunker is a hole with a building in
/// it rather than a building on the ground.
pub fn stamp(
    chunk: &mut crate::chunk::Chunk,
    position: vx_core::ChunkPos,
    sites: &[BunkerSite],
    blocks: &crate::gen::TerrainBlocks,
) {
    let origin = position.origin();
    for site in sites {
        let reach = site.reach();
        if origin.x > site.centre.0 + reach
            || origin.z > site.centre.1 + reach
            || origin.x + vx_core::CHUNK_SIZE <= site.centre.0 - reach
            || origin.z + vx_core::CHUNK_SIZE <= site.centre.1 - reach
        {
            continue;
        }
        let plan = layout(site);
        for ((x, y, z), cell) in plan.cells() {
            let (local_x, local_z) = (x - origin.x, z - origin.z);
            if !(0..vx_core::CHUNK_SIZE).contains(&local_x)
                || !(0..vx_core::CHUNK_SIZE).contains(&local_z)
            {
                continue;
            }
            let Some(local) = vx_core::LocalPos::new(local_x, *y, local_z) else {
                continue;
            };
            let block = block_for(*cell, blocks).unwrap_or(vx_core::BlockId::AIR);
            chunk.set(local, block);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A height field with hills, so siting has real ground to judge.
    fn ground() -> impl Fn(i32, i32) -> i32 {
        |x: i32, z: i32| {
            let base = crate::noise::value_2d(0x51de, x as f32 / 90.0, z as f32 / 90.0);
            80 + (base * 40.0) as i32
        }
    }

    fn sample(count: usize) -> Vec<BunkerSite> {
        let height = ground();
        let mut sites = Vec::new();
        let mut radius = CELL * 4;
        while sites.len() < count && radius < CELL * 80 {
            sites = bunkers_near(2024, (0, 0), radius, &height);
            radius += CELL * 4;
        }
        sites
    }

    /// Every air cell reachable from the hatch, by walking rather than by
    /// wishing: six-connected through open space only.
    fn reachable(site: &BunkerSite, plan: &Layout) -> HashSet<(i32, i32, i32)> {
        let mut seen = HashSet::new();
        let mut queue: Vec<(i32, i32, i32)> = Vec::new();
        for y in site.level_base(site.levels - 1)..=site.hatch_ground + 3 {
            let cell = (site.hatch.0, y, site.hatch.1);
            if plan.cell_at(cell.0, cell.1, cell.2) == Some(Cell::Air) && seen.insert(cell) {
                queue.push(cell);
            }
        }
        while let Some((x, y, z)) = queue.pop() {
            for (dx, dy, dz) in [
                (1, 0, 0),
                (-1, 0, 0),
                (0, 0, 1),
                (0, 0, -1),
                (0, 1, 0),
                (0, -1, 0),
            ] {
                let next = (x + dx, y + dy, z + dz);
                if seen.contains(&next) {
                    continue;
                }
                if plan.cell_at(next.0, next.1, next.2) == Some(Cell::Air) {
                    seen.insert(next);
                    queue.push(next);
                }
            }
        }
        seen
    }

    #[test]
    fn the_same_seed_lays_out_the_same_bunker() {
        let height = ground();
        for tier in [Tier::Small, Tier::Medium, Tier::Large] {
            let site = sample(60)
                .into_iter()
                .find(|site| site.tier == tier)
                .unwrap_or_else(|| panic!("no {tier:?} bunker in the sample"));
            assert_eq!(layout(&site).fingerprint(), layout(&site).fingerprint());
            // And re-derived from the lattice rather than from the struct:
            // the whole claim is that nothing is stored.
            let again = bunkers_near(2024, site.centre, 8, &height)
                .into_iter()
                .next()
                .expect("the site re-derives from its own cell");
            assert_eq!(again, site);
            assert_eq!(layout(&again).fingerprint(), layout(&site).fingerprint());
        }
    }

    #[test]
    fn no_two_bunkers_share_a_layout() {
        // The uniqueness claim, as a property rather than a hope. Uniform
        // noise would pass this too — what it is really guarding is that the
        // quasirandom draw never collapses two sites onto one arrangement.
        let sites = sample(300);
        assert!(sites.len() >= 200, "only {} bunkers sampled", sites.len());
        let prints: HashSet<u64> = sites.iter().map(|site| layout(site).fingerprint()).collect();
        assert_eq!(
            prints.len(),
            sites.len(),
            "{} of {} bunkers share a layout with another",
            sites.len() - prints.len(),
            sites.len()
        );
    }

    #[test]
    fn a_route_in_is_a_route_out() {
        // Every room, on every level, of every tier, reachable on foot from
        // the hatch. A sealed bunker is loot the ledger promises and nothing
        // can collect, and the failure is invisible from the surface.
        for site in sample(200) {
            let plan = layout(&site);
            let seen = reachable(&site, &plan);
            assert!(!seen.is_empty(), "the hatch at {:?} opens on nothing", site.hatch);
            for room in &plan.rooms {
                let base = site.level_base(room.level);
                let entered = (room.x..room.x + room.w).any(|x| {
                    (room.z..room.z + room.d).any(|z| {
                        (1..=HEADROOM).any(|step| seen.contains(&(x, base + step, z)))
                    })
                });
                assert!(
                    entered,
                    "{:?} {:?} at {:?}: no way into the {:?} room on level {}",
                    site.tier, site.system, site.centre, room.kind, room.level
                );
            }
        }
    }

    #[test]
    fn every_room_size_has_a_piece_that_fits() {
        // The test that fails at authoring time instead of generation time:
        // add a vocabulary value without adding a piece and this says so.
        for kind in [
            RoomKind::Generator,
            RoomKind::Radio,
            RoomKind::Bunk,
            RoomKind::Store,
            RoomKind::Hall,
        ] {
            for width in VOCABULARY {
                for depth in VOCABULARY {
                    let inner = (width - 2, depth - 2);
                    if inner.0 < 1 || inner.1 < 1 {
                        continue;
                    }
                    assert!(
                        POOL.iter().any(|piece| {
                            let (w, d) = piece.extent();
                            piece.kind == kind && w <= inner.0 && d <= inner.1
                        }),
                        "nothing in the pool furnishes a {width}x{depth} {kind:?} room"
                    );
                }
            }
        }
    }

    #[test]
    fn no_piece_walls_itself_in() {
        // A furnishing that rings its own footprint leaves an air pocket
        // nothing can walk into — a room that is technically reachable and
        // practically not. The real property is not "keep a row clear" (a
        // hall's corner columns enclose nothing); it is that every clear cell
        // in a piece is reachable from the walkway around it.
        for piece in POOL {
            let (w, d) = piece.extent();
            for rows in piece.layers {
                let clear = |x: i32, z: i32| -> bool {
                    if !(0..w).contains(&x) || !(0..d).contains(&z) {
                        // Outside the piece is the room's own walkway, which
                        // `furnish` always leaves open.
                        return true;
                    }
                    rows[z as usize].as_bytes()[x as usize] == b'.'
                };
                let mut seen: HashSet<(i32, i32)> = HashSet::new();
                let mut queue = vec![(-1, -1)];
                seen.insert((-1, -1));
                while let Some((x, z)) = queue.pop() {
                    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                        let next = (x + dx, z + dz);
                        if !(-1..=w).contains(&next.0) || !(-1..=d).contains(&next.1) {
                            continue;
                        }
                        if seen.contains(&next) || !clear(next.0, next.1) {
                            continue;
                        }
                        seen.insert(next);
                        queue.push(next);
                    }
                }
                for z in 0..d {
                    for x in 0..w {
                        assert!(
                            !clear(x, z) || seen.contains(&(x, z)),
                            "a {:?} piece seals an air pocket at ({x}, {z}): {rows:?}",
                            piece.kind
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn orientations_never_repeat() {
        // The golden angle doing its one job. Sorted bearings across a real
        // sample must never sit on top of each other — no two hatches on a
        // ridge line point the same way, and not one byte is stored to say so.
        let mut bearings: Vec<f32> = sample(200)
            .iter()
            .map(|site| site.bearing.to_degrees())
            .collect();
        bearings.sort_by(|a, b| a.partial_cmp(b).expect("bearings are finite"));
        for pair in bearings.windows(2) {
            assert!(
                (pair[1] - pair[0]).abs() > 1.0e-3,
                "two bunkers face the same way: {pair:?}"
            );
        }
        // And they spread: a clustered sequence would leave big empty arcs.
        let widest = bearings
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .fold(0.0f32, f32::max);
        assert!(widest < 30.0, "bearings clustered, widest gap {widest}°");
    }

    #[test]
    fn a_bunker_never_opens_under_a_town() {
        // Towns are levelled plots with cellars nobody dug. A shell breaking
        // a plaza would be the same class of bug as a cave under main street.
        let height = ground();
        for site in bunkers_near(2024, (0, 0), CELL * 20, &height) {
            for town in crate::town::towns_near(2024, site.centre, crate::town::CELL, &height) {
                let clearance = town.core_half + crate::town::SKIRT;
                assert!(
                    (town.centre.0 - site.centre.0).abs() > clearance
                        || (town.centre.1 - site.centre.1).abs() > clearance,
                    "a bunker at {:?} is under {:?}",
                    site.centre,
                    town.centre
                );
            }
        }
    }

    #[test]
    fn the_way_in_always_comes_down() {
        // The one siting rule that cannot be argued: an approach cut into
        // ground that falls away is a corridor hanging in the sky.
        let height = ground();
        for site in bunkers_near(2024, (0, 0), CELL * 20, &height) {
            assert!(
                site.hatch_ground >= site.level_base(0) + 2,
                "the hatch at {:?} sits below the floor it leads to",
                site.hatch
            );
            assert_eq!(
                site.hatch_ground,
                height(site.hatch.0, site.hatch.1),
                "the stored hatch ground disagrees with the height field"
            );
        }
    }
}
