//! Oil and gas: the first resource this world holds that is not a *block* you
//! were meant to carry away.
//!
//! # Why fluids are not ore with a different colour
//!
//! Every resource until now answered the same question — "is this block worth
//! the seconds it takes to break?" — and a drill answered it. A reservoir
//! answers a different one. It is enormous, it is deep, it is worthless one
//! block at a time, and the only way to get at it in quantity is to stand a
//! machine over it and wait. That turns prospecting from *looking at a
//! hillside* into *reading a map*: a reservoir is a place, and a well is a
//! commitment to that place.
//!
//! # No fluid simulation, and none needed
//!
//! Nothing here flows. A reservoir is saturated rock — oil sand, gas shale —
//! stamped into the stone exactly the way an ore body is, on its own much
//! coarser lattice. Digging into one by hand gets you the smell of it and
//! very little else; [`crate::reservoir::Reservoir::volume`] is what a well
//! can lift, and the well is in `vx-app` where the machines live. The world's
//! job is only to say, purely and repeatably, *what is under this column and
//! how much of it there is*.
//!
//! # One body per neighbourhood
//!
//! The lattice cell is [`CELL_XZ`] blocks across — eight times the ore
//! lattice — because a field the player can name has to be rare enough to be
//! worth naming. Between that and [`PRESENCE`], most of the map has nothing
//! under it at all, which is what makes the survey that finds one matter.

use vx_core::BlockPos;

use crate::noise::signed_2d;

/// What a reservoir holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fluid {
    Oil,
    Gas,
}

impl Fluid {
    /// The saturated rock this fluid shows up as when something cuts into it.
    pub fn block_name(self) -> &'static str {
        match self {
            Fluid::Oil => "engine:oil_sand",
            Fluid::Gas => "engine:gas_shale",
        }
    }

    /// What a well lifting this fluid puts on the pile.
    pub fn product(self) -> &'static str {
        match self {
            Fluid::Oil => "engine:oil_barrel",
            Fluid::Gas => "engine:gas_cell",
        }
    }

    /// How the panels and the terminal say it. Uppercase for the bitmap font.
    pub fn name(self) -> &'static str {
        match self {
            Fluid::Oil => "OIL",
            Fluid::Gas => "GAS",
        }
    }
}

/// Horizontal spacing of candidate reservoirs. Eight ore lattices wide.
pub const CELL_XZ: i32 = 672;

/// Reservoirs live below the ore band: deep enough that finding one is a
/// consequence of committing to the underground, never of a walk.
const BAND_MIN_Y: i32 = 10;
const BAND_MAX_Y: i32 = 46;

const MIN_RADIUS: f32 = 16.0;
const MAX_RADIUS: f32 = 30.0;

/// Fraction of lattice cells holding a reservoir.
const PRESENCE: f32 = 0.38;

/// Share of reservoirs that are gas rather than oil. Gas is the commoner
/// find and the lesser prize — it fuels your own fleet, where oil is what
/// the towns pay for.
const GAS_SHARE: f32 = 0.45;

/// How far noise may push the boundary, as a fraction of radius.
const WOBBLE: f32 = 0.22;

/// Barrels (or canisters) per cubic block of body. A well lifts this much
/// times the body's volume over its whole life, so a big field is a season's
/// work and a small one is an afternoon's.
const YIELD_PER_BLOCK: f32 = 0.028;

/// One body of oil or gas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reservoir {
    pub fluid: Fluid,
    /// Centre in world coordinates.
    pub centre: [f32; 3],
    pub radius: f32,
    /// Per-body seed, so two fields of a size are shaped differently.
    seed: u64,
}

impl Reservoir {
    /// The furthest a perturbed boundary can reach from the centre.
    fn reach(&self) -> f32 {
        self.radius * (1.0 + WOBBLE)
    }

    /// Is this block inside the body? Same lumpy-boundary trick as an ore
    /// deposit, at a coarser noise scale because the body is far bigger.
    pub fn contains(&self, pos: BlockPos) -> bool {
        let dx = pos.x as f32 + 0.5 - self.centre[0];
        let dy = pos.y as f32 + 0.5 - self.centre[1];
        let dz = pos.z as f32 + 0.5 - self.centre[2];
        let distance_squared = dx * dx + dy * dy + dz * dz;

        let reach = self.reach();
        if distance_squared > reach * reach {
            return false;
        }

        let distance = distance_squared.sqrt();
        let scale = 0.035;
        let wobble = signed_2d(self.seed, pos.x as f32 * scale, pos.z as f32 * scale) * 0.5
            + signed_2d(
                self.seed ^ 0x4f49_4c00,
                pos.y as f32 * scale,
                pos.x as f32 * scale,
            ) * 0.5;

        distance < self.radius * (1.0 + wobble * WOBBLE)
    }

    /// Does the column at `(x, z)` pass through this body at all?
    ///
    /// The well's question, asked before a drill string exists: a wellhead
    /// stands on the surface and everything below it is the bet.
    pub fn under(&self, x: i32, z: i32) -> bool {
        let dx = x as f32 + 0.5 - self.centre[0];
        let dz = z as f32 + 0.5 - self.centre[2];
        // Conservative: the horizontal footprint of the unperturbed sphere.
        // A hole drilled at the very lip of a lumpy boundary might miss, and
        // that is the well's business to discover, not the map's to promise.
        dx * dx + dz * dz < self.radius * self.radius * 0.64
    }

    /// The depth a drill string has to reach: the top of the body under this
    /// column, in world y.
    pub fn crown(&self) -> i32 {
        (self.centre[1] + self.radius * 0.6).round() as i32
    }

    /// Everything this body can be made to give up, in barrels or canisters.
    pub fn volume(&self) -> u64 {
        let cubic = self.radius * self.radius * self.radius * 4.19;
        (cubic * YIELD_PER_BLOCK) as u64
    }
}

/// Hash a lattice cell into a reservoir, or nothing.
fn reservoir_in_cell(seed: u64, cell_x: i32, cell_z: i32) -> Option<Reservoir> {
    let key = |salt: u64| -> f32 {
        crate::seed::unit(crate::seed::finalise(
            seed ^ salt
                ^ 0x5245_5345_5256_0000
                ^ (cell_x as i64 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
                ^ (cell_z as i64 as u64).wrapping_mul(0x1656_67b1_9e37_79f9),
        ))
    };

    if key(0x01) > PRESENCE {
        return None;
    }

    let centre_y = BAND_MIN_Y as f32 + key(0x03) * (BAND_MAX_Y - BAND_MIN_Y) as f32;
    let fluid = if key(0x06) < GAS_SHARE {
        Fluid::Gas
    } else {
        Fluid::Oil
    };

    Some(Reservoir {
        fluid,
        centre: [
            cell_x as f32 * CELL_XZ as f32 + key(0x02) * CELL_XZ as f32,
            centre_y,
            cell_z as f32 * CELL_XZ as f32 + key(0x04) * CELL_XZ as f32,
        ],
        radius: MIN_RADIUS + key(0x05) * (MAX_RADIUS - MIN_RADIUS),
        seed: seed ^ ((cell_x as i64 as u64) << 33) ^ (cell_z as i64 as u64),
    })
}

/// Every reservoir that could reach the box `min..=max`.
///
/// Gathered once per chunk beside the ore bodies, for the same reason.
pub fn reservoirs_overlapping(seed: u64, min: BlockPos, max: BlockPos) -> Vec<Reservoir> {
    let margin = (MAX_RADIUS * (1.0 + WOBBLE)).ceil() as i32;

    let lo_x = (min.x - margin).div_euclid(CELL_XZ);
    let hi_x = (max.x + margin).div_euclid(CELL_XZ);
    let lo_z = (min.z - margin).div_euclid(CELL_XZ);
    let hi_z = (max.z + margin).div_euclid(CELL_XZ);

    let mut found = Vec::new();
    for cell_x in lo_x..=hi_x {
        for cell_z in lo_z..=hi_z {
            if let Some(reservoir) = reservoir_in_cell(seed, cell_x, cell_z) {
                found.push(reservoir);
            }
        }
    }
    found
}

/// The fluid at a position, given bodies already gathered for the area.
pub fn fluid_at(reservoirs: &[Reservoir], pos: BlockPos) -> Option<Fluid> {
    reservoirs
        .iter()
        .find(|reservoir| reservoir.contains(pos))
        .map(|reservoir| reservoir.fluid)
}

/// What a well sunk at `(x, z)` would find, if anything.
///
/// Pure in the seed and the column, which is what lets a wellhead be a
/// *decision*: the answer is the same for every session of this world, so a
/// dry hole is a place, not a dice roll.
pub fn reservoir_under(seed: u64, x: i32, z: i32) -> Option<Reservoir> {
    reservoirs_overlapping(
        seed,
        BlockPos::new(x, BAND_MIN_Y, z),
        BlockPos::new(x, BAND_MAX_Y, z),
    )
    .into_iter()
    .find(|reservoir| reservoir.under(x, z))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn survey(seed: u64) -> Vec<Reservoir> {
        reservoirs_overlapping(
            seed,
            BlockPos::new(-4000, 0, -4000),
            BlockPos::new(4000, 64, 4000),
        )
    }

    #[test]
    fn reservoirs_are_deterministic_in_the_seed() {
        assert_eq!(survey(7), survey(7));
        assert_ne!(survey(7), survey(8));
    }

    #[test]
    fn reservoirs_stay_deep_and_inside_their_band() {
        for reservoir in survey(2024) {
            assert!(
                reservoir.centre[1] >= BAND_MIN_Y as f32
                    && reservoir.centre[1] <= BAND_MAX_Y as f32,
                "a field surfaced at y={}",
                reservoir.centre[1]
            );
            assert!((MIN_RADIUS..=MAX_RADIUS).contains(&reservoir.radius));
        }
    }

    #[test]
    fn both_fluids_exist_somewhere() {
        let fluids: std::collections::BTreeSet<&str> = survey(11)
            .iter()
            .map(|reservoir| reservoir.fluid.name())
            .collect();
        assert_eq!(fluids.len(), 2, "the world holds only {fluids:?}");
    }

    #[test]
    fn most_of_the_map_has_nothing_under_it() {
        // The whole point of a well is that where you sink it matters. If
        // much of the map struck fluid, it would not. Sampled finely inside
        // a few lattice cells rather than sparsely across kilometres, so the
        // number means coverage rather than luck.
        let seed = 4242;
        let mut hits = 0;
        let mut columns = 0;
        for x in (-1400..1400).step_by(28) {
            for z in (-1400..1400).step_by(28) {
                columns += 1;
                if reservoir_under(seed, x, z).is_some() {
                    hits += 1;
                }
            }
        }
        assert!(hits > 0, "no reservoir anywhere in three kilometres");
        assert!(
            hits * 8 < columns,
            "{hits} of {columns} columns struck fluid — a well is not a decision"
        );
    }

    #[test]
    fn a_column_that_strikes_is_a_column_that_holds_the_rock() {
        // `under` promises the drill will meet the body. Pin that promise
        // against `contains`, which is what the world actually stamps: a
        // wellhead that reports a strike over empty stone is the one bug
        // that would make the whole machine a liar. Every body in a wide
        // survey is checked at its own centre column and at eight offsets
        // out towards its lip.
        let seed = 909;
        let mut checked = 0;
        for reservoir in survey(seed) {
            let cx = reservoir.centre[0].round() as i32;
            let cz = reservoir.centre[2].round() as i32;
            let step = (reservoir.radius * 0.2) as i32;
            for (dx, dz) in [
                (0, 0),
                (step, 0),
                (-step, 0),
                (0, step),
                (0, -step),
                (step, step),
                (-step, -step),
                (step, -step),
                (-step, step),
            ] {
                let (x, z) = (cx + dx, cz + dz);
                if !reservoir.under(x, z) {
                    continue;
                }
                checked += 1;
                let struck = (BAND_MIN_Y - MAX_RADIUS as i32
                    ..=BAND_MAX_Y + MAX_RADIUS as i32)
                    .any(|y| reservoir.contains(BlockPos::new(x, y, z)));
                assert!(struck, "a strike at {x},{z} met no saturated rock");
            }
        }
        assert!(checked > 40, "only {checked} strikes — not worth asserting on");
    }

    #[test]
    fn a_bigger_field_is_worth_more() {
        let small = Reservoir {
            fluid: Fluid::Oil,
            centre: [0.0, 30.0, 0.0],
            radius: MIN_RADIUS,
            seed: 1,
        };
        let large = Reservoir {
            radius: MAX_RADIUS,
            ..small
        };
        assert!(small.volume() > 0);
        assert!(large.volume() > small.volume() * 4);
    }
}
