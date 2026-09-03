//! Snow that settles, and ice.
//!
//! Stage 41 drew a line: a season edits no blocks. The sky, the leaves and
//! the growing clock are readings, and the year moves nothing by being read.
//! This module is the other half of a winter — the half that *is* ground —
//! and it lives on the other side of that line, as an automaton.
//!
//! **The shape is the rain's, moved to the oracle's side.** The rain source
//! term pours a few cells on one hashed column near the player every
//! [`EVERY`] ticks. The frost does the same with a wider hand: every
//! [`EVERY`] ticks it reads the sky over the player once and touches
//! [`SAMPLES`] columns hashed off `(seed, tick, n)` within [`REACH`], so it is
//! a pure function of `(seed, tick, where you stand, the ground)` — and the
//! player's path is replayed, so both sides freeze and thaw the same
//! country from the same journal with no order recorded for any of it.
//!
//! **Snow is the surface block swapped for its snowed twin, same shape.**
//! Grass, sphagnum and sand each have one. The ground stays the height it
//! was, doors still open and paths are where they were; what changes is the
//! block's *name*, which is what the region files, the hash and the minimap
//! all already read. A tuft on the grass counts as open sky; a roof, a log,
//! a canopy or a wall over the column does not, and the snow stops there.
//!
//! **Ice is solid, with everything that follows.** Full, still water at or
//! below sea level becomes `engine:ice`, which is a solid translucent block
//! and nothing else: you walk across it, so do the drones and the deputies;
//! a pump on it lifts nothing and an electrolyser beside it finds no water;
//! fire finds no fuel in it; and the mesher's roofs being opaque, the lake
//! bed under it keeps its light. The thaw puts back a full water block and
//! the live side wakes the water automaton, which then replays identically
//! because the block itself is on both sides.
//!
//! **Worldgen places none of the four.** A chunk generated fresh is bare
//! whatever the month, and the automaton snows it in when you are near —
//! the same rule the rain follows.

use vx_core::{BlockId, BlockPos};
use vx_world::gen::SEA_LEVEL;
use vx_world::weather::{self, Conditions};
use vx_world::World;

/// Ticks between one pass of the frost and the next. The rain's own cadence.
pub const EVERY: u64 = 64;

/// Columns touched per pass. Sixteen a second whitens the ground within
/// reach in a few hours of snow, which is how long a front takes to pass.
pub const SAMPLES: u32 = 16;

/// How far from the player a pass reaches. The rain's spread, wider: a frost
/// is a sheet where a shower is a patch.
pub const REACH: i32 = 24;

/// What one pass did, for the terminal and the water. Replay drops it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// Bare ground gone white.
    pub snowed: u32,
    /// Snow gone back to the ground it lay on.
    pub thawed: u32,
    /// Water gone solid.
    pub froze: u32,
    /// Ice gone back to water.
    pub melted: u32,
    /// Where the ice went back to water, for the live side to wake. The
    /// block is on both sides already; the waking is bookkeeping.
    pub melted_at: Vec<BlockPos>,
}

impl Report {
    /// Fold what happened to one column in.
    fn add(&mut self, touched: Touched) {
        match touched {
            Touched::Nothing => {}
            Touched::Snowed => self.snowed += 1,
            Touched::Thawed => self.thawed += 1,
            Touched::Froze => self.froze += 1,
            Touched::Melted(at) => {
                self.melted += 1;
                self.melted_at.push(at);
            }
        }
    }

    /// The line for the terminal, the coldest news first.
    pub fn line(&self) -> Option<&'static str> {
        if self.froze > 0 {
            Some("THE LAKE HAS FROZEN")
        } else if self.snowed > 0 {
            Some("SNOW IS SETTLING")
        } else if self.thawed + self.melted > 0 {
            Some("THE THAW")
        } else {
            None
        }
    }
}

/// What the frost did to one column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Touched {
    Nothing,
    Snowed,
    Thawed,
    Froze,
    /// Ice back to water, at this block — the live side wakes it.
    Melted(BlockPos),
}

/// The block ids the frost reads and writes, looked up once per pass.
#[derive(Debug, Clone, Copy)]
struct Cold {
    grass: BlockId,
    sphagnum: BlockId,
    sand: BlockId,
    snowy_grass: BlockId,
    snowy_sphagnum: BlockId,
    snowy_sand: BlockId,
    water: BlockId,
    ice: BlockId,
    tuft: BlockId,
}

impl Cold {
    fn of(world: &World) -> Self {
        let blocks = world.generator().blocks();
        Cold {
            grass: blocks.grass,
            sphagnum: blocks.sphagnum,
            sand: blocks.sand,
            snowy_grass: blocks.snowy_grass,
            snowy_sphagnum: blocks.snowy_sphagnum,
            snowy_sand: blocks.snowy_sand,
            water: blocks.water,
            ice: blocks.ice,
            tuft: blocks.tall_grass,
        }
    }

    /// The snowed twin of a bare block, if it has one.
    fn snowed(&self, bare: BlockId) -> Option<BlockId> {
        if bare == self.grass {
            Some(self.snowy_grass)
        } else if bare == self.sphagnum {
            Some(self.snowy_sphagnum)
        } else if bare == self.sand {
            Some(self.snowy_sand)
        } else {
            None
        }
    }

    /// The bare block a snowed one came from, if it is snowed at all.
    fn bare(&self, snowed: BlockId) -> Option<BlockId> {
        if snowed == self.snowy_grass {
            Some(self.grass)
        } else if snowed == self.snowy_sphagnum {
            Some(self.sphagnum)
        } else if snowed == self.snowy_sand {
            Some(self.sand)
        } else {
            None
        }
    }
}

/// One tick of the frost. Self-gated on [`EVERY`]; called every tick from
/// both `Advance` loops, and does nothing on the ticks between.
pub fn settle(world: &mut World, seed: u64, tick: u64, standing: BlockPos) -> Report {
    let mut report = Report::default();
    if !tick.is_multiple_of(EVERY) {
        return report;
    }
    let sky = weather::at(seed, tick, standing.x, standing.z);
    let cold = Cold::of(world);
    for n in 0..SAMPLES {
        let (x, z) = sample(seed, tick, n, standing);
        report.add(touch(world, &cold, &sky, x, z));
    }
    report
}

/// The `n`th column of this pass, hashed off the seed and the tick so both
/// sides pick the same one.
fn sample(seed: u64, tick: u64, n: u32, standing: BlockPos) -> (i32, i32) {
    let hash = vx_world::seed::finalise(
        seed ^ tick.wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ u64::from(n).wrapping_mul(0xd1b5_4a32_d192_ed03),
    );
    let span = f32::from(REACH as u16 * 2 + 1);
    // Two draws, each finalised: `unit` reads the top bits, so a hash merely
    // xored with a small constant would put every column on the diagonal.
    let dx = (vx_world::seed::unit(hash) * span) as i32 - REACH;
    let dz = (vx_world::seed::unit(vx_world::seed::finalise(hash ^ 0x51)) * span) as i32 - REACH;
    (standing.x + dx, standing.z + dz)
}

/// What the sky does to one column. The top of the column is the highest
/// block that is not air; a tuft standing on it is looked through.
fn touch(world: &mut World, cold: &Cold, sky: &Conditions, x: i32, z: i32) -> Touched {
    let Some(surface) = world.surface_y(x, z) else {
        return Touched::Nothing;
    };
    let mut top = BlockPos::new(x, surface - 1, z);
    if world.block(top) == cold.tuft {
        top = BlockPos::new(x, top.y - 1, z);
    }
    let block = world.block(top);

    if sky.freezing() {
        if sky.snowing() {
            if let Some(white) = cold.snowed(block) {
                world.set_block(top, white);
                return Touched::Snowed;
            }
        }
        // Still water, full to the brim, low enough to be a lake rather
        // than a puddle a shower left on a hill.
        if block == cold.water
            && top.y <= SEA_LEVEL
            && vx_world::fluid::level_at(world, cold.water, top) == vx_world::fluid::FULL
        {
            world.set_block(top, cold.ice);
            return Touched::Froze;
        }
        return Touched::Nothing;
    }

    if let Some(bare) = cold.bare(block) {
        world.set_block(top, bare);
        return Touched::Thawed;
    }
    if block == cold.ice {
        // A replaced block is a block made whole: no mask, so a full one.
        world.set_block(top, cold.water);
        return Touched::Melted(top);
    }
    Touched::Nothing
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::ChunkPos;
    use vx_world::season::SEASON_TICKS;

    const SEED: u64 = 2024;

    fn world() -> World {
        let mut world = World::new(SEED);
        world.load_around(ChunkPos::new(0, 0), 2);
        world
    }

    /// The first tick on the frost's cadence, from `from`, whose sky over the
    /// origin satisfies `want`.
    fn tick_where(from: u64, want: impl Fn(&Conditions) -> bool) -> u64 {
        (0..(2 * SEASON_TICKS / EVERY))
            .map(|n| from + n * EVERY)
            .find(|&tick| want(&weather::at(SEED, tick, 0, 0)))
            .expect("no such sky in two seasons")
    }

    fn snowing_tick() -> u64 {
        tick_where(3 * SEASON_TICKS, |sky| sky.snowing())
    }

    fn freezing_dry_tick() -> u64 {
        tick_where(3 * SEASON_TICKS, |sky| sky.freezing() && !sky.state.wet())
    }

    fn warm_tick() -> u64 {
        tick_where(SEASON_TICKS, |sky| !sky.freezing())
    }

    /// Put `block` on top of the column at `(x, z)`, with open sky above.
    fn crown(world: &mut World, x: i32, z: i32, block: BlockId) -> BlockPos {
        let top = BlockPos::new(x, world.surface_y(x, z).unwrap() - 1, z);
        world.set_block(top, block);
        top
    }

    /// Dig the column at `(x, z)` down to a lake bed and fill one block of
    /// water at `y`.
    fn pond(world: &mut World, x: i32, z: i32, y: i32) -> BlockPos {
        let surface = world.surface_y(x, z).unwrap();
        for dig in y..surface {
            world.set_block(BlockPos::new(x, dig, z), BlockId::AIR);
        }
        let at = BlockPos::new(x, y, z);
        world.set_block(at, world.generator().blocks().water);
        at
    }

    #[test]
    fn the_cold_exists_in_this_country() {
        let snow = snowing_tick();
        let dry = freezing_dry_tick();
        let warm = warm_tick();
        assert!(weather::at(SEED, snow, 0, 0).snowing());
        assert!(!weather::at(SEED, dry, 0, 0).state.wet());
        assert!(!weather::at(SEED, warm, 0, 0).freezing());
    }

    #[test]
    fn snow_settles_on_open_grass_sand_and_bog_and_the_thaw_gives_them_back() {
        let mut world = world();
        let cold = Cold::of(&world);
        let sky = weather::at(SEED, snowing_tick(), 0, 0);
        let warm = weather::at(SEED, warm_tick(), 0, 0);
        for (n, (bare, white)) in [
            (cold.grass, cold.snowy_grass),
            (cold.sand, cold.snowy_sand),
            (cold.sphagnum, cold.snowy_sphagnum),
        ]
        .into_iter()
        .enumerate()
        {
            let x = 3 + n as i32 * 2;
            let top = crown(&mut world, x, 3, bare);
            assert_eq!(touch(&mut world, &cold, &sky, x, 3), Touched::Snowed);
            assert_eq!(world.block(top), white, "the snow did not settle");
            // Snowed ground stays snowed while it is cold.
            assert_eq!(touch(&mut world, &cold, &sky, x, 3), Touched::Nothing);
            assert_eq!(touch(&mut world, &cold, &warm, x, 3), Touched::Thawed);
            assert_eq!(world.block(top), bare, "the thaw gave back the wrong block");
        }
    }

    #[test]
    fn a_tuft_is_open_sky_and_a_roof_is_not() {
        let mut world = world();
        let cold = Cold::of(&world);
        let sky = weather::at(SEED, snowing_tick(), 0, 0);

        let top = crown(&mut world, 5, 5, cold.grass);
        world.set_block(BlockPos::new(5, top.y + 1, 5), cold.tuft);
        assert_eq!(touch(&mut world, &cold, &sky, 5, 5), Touched::Snowed);
        assert_eq!(world.block(top), cold.snowy_grass);
        assert_eq!(world.block(BlockPos::new(5, top.y + 1, 5)), cold.tuft, "the tuft was lost");

        let roofed = crown(&mut world, 7, 5, cold.grass);
        let plank = world.registry().id_of("engine:plank").unwrap();
        world.set_block(BlockPos::new(7, roofed.y + 2, 5), plank);
        assert_eq!(touch(&mut world, &cold, &sky, 7, 5), Touched::Nothing);
        assert_eq!(world.block(roofed), cold.grass, "it snowed under a roof");

        // A log, a wall and a rock take no snow of their own.
        for name in ["engine:stone", "engine:plank"] {
            let block = world.registry().id_of(name).unwrap();
            let at = crown(&mut world, 9, 5, block);
            assert_eq!(touch(&mut world, &cold, &sky, 9, 5), Touched::Nothing);
            assert_eq!(world.block(at), block);
        }
    }

    #[test]
    fn still_low_water_freezes_wet_or_dry_and_the_thaw_gives_back_full_water() {
        let mut world = world();
        let cold = Cold::of(&world);
        let dry = weather::at(SEED, freezing_dry_tick(), 0, 0);
        let warm = weather::at(SEED, warm_tick(), 0, 0);

        let lake = pond(&mut world, 4, 8, SEA_LEVEL - 2);
        assert_eq!(touch(&mut world, &cold, &dry, 4, 8), Touched::Froze);
        assert_eq!(world.block(lake), cold.ice);
        assert!(world.is_solid(lake), "ice is not solid");
        assert_eq!(vx_world::fluid::level_at(&world, cold.water, lake), 0);
        assert_eq!(touch(&mut world, &cold, &dry, 4, 8), Touched::Nothing);

        assert_eq!(touch(&mut world, &cold, &warm, 4, 8), Touched::Melted(lake));
        assert_eq!(world.block(lake), cold.water);
        assert_eq!(
            vx_world::fluid::level_at(&world, cold.water, lake),
            vx_world::fluid::FULL,
            "the thaw left a part-filled block"
        );
        assert_eq!(world.mask(lake), None);
    }

    #[test]
    fn a_puddle_on_a_hill_and_a_half_filled_block_do_not_freeze() {
        let mut world = world();
        let cold = Cold::of(&world);
        let dry = weather::at(SEED, freezing_dry_tick(), 0, 0);

        let puddle = crown(&mut world, 6, 8, cold.water);
        assert!(puddle.y > SEA_LEVEL, "the fixture is not on a hill");
        assert_eq!(touch(&mut world, &cold, &dry, 6, 8), Touched::Nothing);
        assert_eq!(world.block(puddle), cold.water);

        let shallow = pond(&mut world, 8, 8, SEA_LEVEL - 1);
        vx_world::fluid::set_level(&mut world, cold.water, shallow, 20);
        assert_eq!(touch(&mut world, &cold, &dry, 8, 8), Touched::Nothing);
        assert_eq!(world.block(shallow), cold.water);
    }

    #[test]
    fn nothing_changes_outside_reach_and_a_few_hours_of_snow_whiten_the_ground() {
        // A square of country wider than the reach, loaded to its corners.
        let mut world = World::new(SEED);
        world.load_around(ChunkPos::new(0, 0), 3);
        let cold = Cold::of(&world);
        // Lay grass everywhere the frost can reach, and well beyond it.
        for x in -30..=30 {
            for z in -30..=30 {
                crown(&mut world, x, z, cold.grass);
            }
        }
        let standing = BlockPos::new(0, 80, 0);
        // Run the frost through five hours' worth of snowing passes, however
        // the fronts space them out across the winter.
        let mut passes = 0;
        let mut snowed = 0;
        let mut tick = snowing_tick();
        while passes < 5 * 60 * 64 / EVERY && tick < 4 * SEASON_TICKS {
            if weather::at(SEED, tick, 0, 0).snowing() {
                snowed += settle(&mut world, SEED, tick, standing).snowed;
                passes += 1;
            }
            tick += EVERY;
        }
        assert!(snowed > 1_000, "five hours of snow settled on {snowed} columns");
        let (mut far, mut near, mut bare) = (0, 0, 0);
        for x in -30..=30 {
            for z in -30..=30 {
                let top = BlockPos::new(x, world.surface_y(x, z).unwrap() - 1, z);
                let white = world.block(top) == cold.snowy_grass;
                if x.abs() > REACH || z.abs() > REACH {
                    assert!(!white, "it snowed at ({x}, {z}), outside reach");
                    far += 1;
                } else if white {
                    near += 1;
                } else {
                    bare += 1;
                }
            }
        }
        assert!(far > 0);
        assert!(near > 3 * bare, "only {near} columns within reach went white, {bare} stayed bare");
    }

    #[test]
    fn the_frost_holds_its_cadence() {
        let mut world = world();
        let cold = Cold::of(&world);
        crown(&mut world, 0, 0, cold.grass);
        let tick = snowing_tick();
        let standing = BlockPos::new(0, 80, 0);
        assert_eq!(settle(&mut world, SEED, tick + 1, standing), Report::default());
        assert_eq!(settle(&mut world, SEED, tick + EVERY - 1, standing), Report::default());
    }

    #[test]
    fn the_same_pass_touches_the_same_columns() {
        let standing = BlockPos::new(10, 80, -10);
        let mut off_diagonal = 0;
        for tick in [0, 64, 4096] {
            for n in 0..SAMPLES {
                let (x, z) = sample(SEED, tick, n, standing);
                assert_eq!((x, z), sample(SEED, tick, n, standing));
                assert!((x - standing.x).abs() <= REACH);
                assert!((z - standing.z).abs() <= REACH);
                if x - standing.x != z - standing.z {
                    off_diagonal += 1;
                }
            }
        }
        assert!(off_diagonal > SAMPLES, "the frost falls on a diagonal");
    }
}
