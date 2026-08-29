//! Water that moves.
//!
//! **The fill level is the damage mask.** A block cut up into sixty-four
//! cells by [`crate::micro`] can say how much of itself is missing; a wet
//! block uses the same sixty-four to say how full it is. One `u64` on a wet
//! block, in the sparse map the chunk already keeps for wounds, and
//! `popcount` is the volume. Sixty-four steps of fill is far finer than the
//! seven or eight a level-per-block system gives, it costs no new
//! representation, and the mesher already draws a masked block — so a
//! half-full block comes out as a slab with a surface on it, for free.
//!
//! **The layout is canonical, and that is the trick.** [`level_mask`] fills
//! from the bottom layer up, so a partly-filled block is always flat-topped
//! and the *only* state is the count. The automaton moves integers between
//! cells and rebuilds masks; it never has to reason about which sub-cell
//! went where, and two blocks holding the same volume are the same block.
//!
//! **Determinism comes from the update order, not from luck.** The awake set
//! is kept sorted, so the sweep visits cells in one canonical order however
//! they were woken; the tick is split into two passes by cell parity so a
//! cell never settles against another cell of its own colour mid-pass; and
//! nothing anywhere asks the clock or a thread. Shuffle the wake list and the
//! water ends up in exactly the same place, which is what the replay oracle
//! needs and what the test asserts.
//!
//! **And it is bounded on purpose.** A body flows no further than [`REACH`]
//! from where it woke and retires after [`PATIENCE`] quiet steps. An
//! unbounded automaton would wander out of the ground a replay has loaded and
//! the two sides would disagree — so this is bucket-scale water: flooding a
//! gallery, draining a pool, filling a cistern. Redistributing an ocean is
//! not a thing that happens here, which is also why the sea is a *source*
//! rather than a finite body: one player with a drill should not be able to
//! empty a coastline, and the simulation should not have to move one.

use vx_core::{BlockId, BlockPos, Face};

use crate::micro;
use crate::world::World;

/// A full block, in cells. The same sixty-four the damage mask has.
pub const FULL: u32 = micro::CELLS;

/// How far from where it woke a body may reach, in blocks.
pub const REACH: i32 = 28;

/// Quiet steps before a body decides it has settled and stops ticking.
pub const PATIENCE: u32 = 8;

/// Steps of the player clock between one move of the water and the next.
///
/// Water does not need sixty-four updates a second, and at that rate it would
/// cross a gallery faster than the eye follows. A quarter rate reads as a
/// flood rather than a teleport.
pub const EVERY: u32 = 4;

/// The most that crosses one face in one step, in cells.
///
/// The cap is what makes a flood look like a flood. Without it the levelling
/// rule empties a column into its neighbour in a single step.
const MAX_FLOW: u32 = 10;

/// Below this a body is not worth keeping: it evaporates rather than leaving
/// a scatter of single cells behind.
const DREGS: u32 = 2;

/// The volume in a block: sixty-four for plain water, the popcount for a
/// part-filled one, nothing for anything else.
pub fn level_at(world: &World, water: BlockId, pos: BlockPos) -> u32 {
    if world.block(pos) != water {
        return 0;
    }
    world.mask(pos).map_or(FULL, micro::remaining)
}

/// The mask for a given volume: filled from the bottom layer up, so the
/// surface is flat and the count is the whole of the state.
pub fn level_mask(count: u32) -> micro::Mask {
    let count = count.min(FULL);
    if count == FULL {
        return micro::FULL;
    }
    let mut mask = 0;
    let layers = count / 16;
    let rest = count % 16;
    for layer in 0..layers {
        mask |= 0xffffu64 << (16 * layer);
    }
    if rest > 0 {
        mask |= ((1u64 << rest) - 1) << (16 * layers);
    }
    mask
}

/// Put a volume into a block, collapsing both ends.
///
/// Full becomes a plain water block with no mask at all — the common case,
/// and the one that costs nothing to store or draw. Empty becomes air.
pub fn set_level(world: &mut World, water: BlockId, pos: BlockPos, count: u32) {
    let count = count.min(FULL);
    let standing = world.block(pos);
    if count == 0 {
        if standing == water {
            world.set_block(pos, BlockId::AIR);
        }
        return;
    }
    if standing != water {
        // Only air gives way to water. Rock holds it, and so does a plank.
        if !standing.is_air() {
            return;
        }
        world.set_block(pos, water);
    }
    if count == FULL {
        world.set_mask(pos, micro::FULL);
    } else {
        world.set_mask(pos, level_mask(count));
    }
}

/// Does this block take water?
fn open(world: &World, water: BlockId, pos: BlockPos) -> bool {
    let block = world.block(pos);
    block.is_air() || block == water
}

/// Is this cell part of the sea?
///
/// A full water block at or below the sea's own level supplies without
/// draining. It is not a cheat so much as an admission: the ocean is larger
/// than anything this automaton is allowed to touch, and pretending otherwise
/// means one player with a drill empties a coastline one bucket at a time.
/// Everything above the line, and everything the player put there, is finite
/// and honestly conserved — which is what makes draining a cave pool to get
/// at its floor a real thing to do.
pub fn is_source(world: &World, water: BlockId, pos: BlockPos) -> bool {
    pos.y <= crate::gen::SEA_LEVEL && world.block(pos) == water && world.mask(pos).is_none()
}

/// How far a pump reaches for something to lift.
pub const PUMP_REACH: i32 = 2;

/// How much a pump lifts per step of the water.
///
/// Slower than a burst main and faster than a bucket: a block every few
/// seconds, which is quick enough to empty a gallery you flooded on yourself
/// and slow enough that you watch it happen.
pub const LIFT: u32 = 12;

/// Lift water from around a pump and put it out of the spout on top.
///
/// No facing and no plumbing: the pump takes from whatever it can reach and
/// puts it above itself, and the automaton carries it from there. That is
/// enough to move water *uphill*, which is the one thing gravity will not do
/// for you — fill a cistern on a rise, or get a flooded gallery back.
///
/// Returns the block the water came out into, when anything moved, so the
/// caller can wake a body there.
pub fn pump(world: &mut World, water: BlockId, at: BlockPos) -> Option<BlockPos> {
    let spout = at.neighbour(Face::PosY);
    if !open(world, water, spout) {
        return None;
    }
    let held = level_at(world, water, spout);
    let room = FULL - held;
    if room == 0 {
        return None;
    }

    // The fullest cell within reach, scanned in one canonical order so two
    // equal cells always resolve the same way.
    let mut best: Option<(u32, BlockPos)> = None;
    for dy in -PUMP_REACH..=PUMP_REACH {
        for dz in -PUMP_REACH..=PUMP_REACH {
            for dx in -PUMP_REACH..=PUMP_REACH {
                let from = BlockPos::new(at.x + dx, at.y + dy, at.z + dz);
                if from == spout || from == at {
                    continue;
                }
                let level = level_at(world, water, from);
                if level == 0 {
                    continue;
                }
                if best.is_none_or(|(most, _)| level > most) {
                    best = Some((level, from));
                }
            }
        }
    }
    let (level, from) = best?;

    let source = is_source(world, water, from);
    let give = LIFT.min(room).min(if source { LIFT } else { level });
    if give == 0 {
        return None;
    }
    set_level(world, water, spout, held + give);
    if !source {
        set_level(world, water, from, level - give);
    }
    Some(spout)
}

/// What one step of the water did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    /// Cells that crossed a face this step.
    pub moved: u32,
    /// The body has settled and stopped ticking.
    pub settled: bool,
    /// How many blocks are still awake.
    pub awake: usize,
}

/// A body of water that is not finished moving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Water {
    /// Where it was disturbed. Nothing flows further than [`REACH`] from
    /// here, which is what keeps a body inside the ground a replay loaded.
    pub origin: BlockPos,
    /// Blocks worth looking at, kept sorted so the sweep has one canonical
    /// order whatever order they were woken in.
    awake: Vec<BlockPos>,
    /// Steps since anything moved.
    quiet: u32,
    /// Steps taken, for the quarter-rate tick.
    step: u32,
}

impl Water {
    /// Wake the water around a disturbance.
    pub fn new(origin: BlockPos) -> Self {
        Water {
            origin,
            awake: Vec::new(),
            quiet: 0,
            step: 0,
        }
    }

    /// Is anything still moving?
    pub fn busy(&self) -> bool {
        !self.awake.is_empty() && self.quiet < PATIENCE
    }

    pub fn awake(&self) -> &[BlockPos] {
        &self.awake
    }

    /// Add a block to the set, if it is inside the body's reach.
    pub fn wake(&mut self, pos: BlockPos) {
        let (dx, dy, dz) = (
            pos.x - self.origin.x,
            pos.y - self.origin.y,
            pos.z - self.origin.z,
        );
        if dx.abs() > REACH || dy.abs() > REACH || dz.abs() > REACH {
            return;
        }
        if let Err(index) = self.awake.binary_search_by(|other| order(other, &pos)) {
            self.awake.insert(index, pos);
            self.quiet = 0;
        }
    }

    /// Wake everything around a hole that was just opened.
    ///
    /// The six neighbours, and the block itself: cut into the side of a lake
    /// and it is the lake that has to notice, not the hole.
    pub fn wake_around(&mut self, world: &World, water: BlockId, pos: BlockPos) {
        if level_at(world, water, pos) > 0 {
            self.wake(pos);
        }
        for face in Face::ALL {
            let neighbour = pos.neighbour(face);
            if level_at(world, water, neighbour) > 0 {
                self.wake(neighbour);
            }
        }
    }

    /// One step of the water.
    ///
    /// Every edit happens in here, so the live game and a replay of it change
    /// the same blocks in the same order.
    pub fn settle(&mut self, world: &mut World, water: BlockId) -> Report {
        self.step += 1;
        if !self.step.is_multiple_of(EVERY) || !self.busy() {
            return Report {
                moved: 0,
                settled: !self.busy(),
                awake: self.awake.len(),
            };
        }

        let mut moved = 0;
        let mut woken: Vec<BlockPos> = Vec::new();

        // Two passes by cell parity, each visiting a canonical order. A cell
        // never levels against another cell of its own colour inside a pass,
        // and the sorted sweep means the answer cannot depend on the order
        // the blocks happened to be woken in.
        for parity in 0..2u8 {
            let cells: Vec<BlockPos> = self
                .awake
                .iter()
                .copied()
                .filter(|pos| colour(*pos) == parity)
                .collect();
            for pos in cells {
                moved += self.drain(world, water, pos, &mut woken);
            }
        }

        for pos in woken {
            self.wake(pos);
        }
        // Cells that have nothing left in them stop being interesting.
        let origin = self.origin;
        self.awake.retain(|pos| {
            let level = level_at(world, water, *pos);
            level > 0 && within(origin, *pos)
        });

        if moved == 0 {
            self.quiet += 1;
        } else {
            self.quiet = 0;
        }
        Report {
            moved,
            settled: !self.busy(),
            awake: self.awake.len(),
        }
    }

    /// Move what this block can give away: down first, then sideways to
    /// whatever is holding less.
    fn drain(
        &self,
        world: &mut World,
        water: BlockId,
        pos: BlockPos,
        woken: &mut Vec<BlockPos>,
    ) -> u32 {
        let source = is_source(world, water, pos);
        let mut level = level_at(world, water, pos);
        if level == 0 {
            return 0;
        }
        let mut moved = 0;

        // Down, and take as much as the cell below can hold: water falls
        // before it spreads, which is the whole reason a flood runs down a
        // shaft rather than creeping along the floor.
        let below = pos.neighbour(Face::NegY);
        if within(self.origin, below) && open(world, water, below) {
            let there = level_at(world, water, below);
            let room = FULL - there;
            let give = room.min(if source { MAX_FLOW } else { level }).min(MAX_FLOW);
            if give > 0 {
                set_level(world, water, below, there + give);
                if !source {
                    level -= give;
                    set_level(world, water, pos, level);
                }
                moved += give;
                woken.push(below);
                if level == 0 {
                    return moved;
                }
            }
        }

        // Then sideways, levelling with whatever is lower. Half the
        // difference, capped, so it converges instead of ringing between two
        // cells for ever.
        for face in [Face::NegX, Face::PosX, Face::NegZ, Face::PosZ] {
            if level <= DREGS && !source {
                break;
            }
            let side = pos.neighbour(face);
            if !within(self.origin, side) || !open(world, water, side) {
                continue;
            }
            let there = level_at(world, water, side);
            if there + 1 >= level && !source {
                continue;
            }
            let difference = if source { FULL - there } else { level - there };
            let give = (difference / 2).clamp(1, MAX_FLOW).min(FULL - there);
            let give = if source { give } else { give.min(level) };
            if give == 0 {
                continue;
            }
            set_level(world, water, side, there + give);
            if !source {
                level -= give;
                set_level(world, water, pos, level);
            }
            moved += give;
            woken.push(side);
        }
        moved
    }
}

/// Inside the body's reach?
fn within(origin: BlockPos, pos: BlockPos) -> bool {
    (pos.x - origin.x).abs() <= REACH
        && (pos.y - origin.y).abs() <= REACH
        && (pos.z - origin.z).abs() <= REACH
}

/// Which half of the checkerboard a block is on.
fn colour(pos: BlockPos) -> u8 {
    ((pos.x + pos.y + pos.z).rem_euclid(2)) as u8
}

/// The canonical sweep order: y, then z, then x.
fn order(left: &BlockPos, right: &BlockPos) -> std::cmp::Ordering {
    left.y
        .cmp(&right.y)
        .then(left.z.cmp(&right.z))
        .then(left.x.cmp(&right.x))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::ChunkPos;

    fn water_id(world: &World) -> BlockId {
        world.registry().id_of("engine:water").expect("no water")
    }

    /// A slab of stone in the air with a hollow in it, well above the sea, so
    /// nothing here is a source and every drop is conserved.
    fn basin() -> (World, BlockId, BlockPos) {
        let mut world = World::new(2024);
        world.load_around(ChunkPos::new(0, 0), 1);
        let stone = world.registry().id_of("engine:stone").expect("no stone");
        let floor = 140;
        for x in 0..12 {
            for z in 0..12 {
                world.set_block(BlockPos::new(x, floor, z), stone);
                for y in floor + 1..floor + 6 {
                    world.set_block(BlockPos::new(x, y, z), BlockId::AIR);
                }
            }
        }
        // A rim, so the water has somewhere to stop.
        for x in 0..12 {
            for y in floor + 1..floor + 6 {
                world.set_block(BlockPos::new(x, y, 0), stone);
                world.set_block(BlockPos::new(x, y, 11), stone);
                world.set_block(BlockPos::new(0, y, x), stone);
                world.set_block(BlockPos::new(11, y, x), stone);
            }
        }
        let id = water_id(&world);
        (world, id, BlockPos::new(6, floor + 1, 6))
    }

    /// Every drop in the basin.
    fn total(world: &World, water: BlockId, floor: i32) -> u32 {
        let mut sum = 0;
        for y in floor..floor + 8 {
            for x in 0..12 {
                for z in 0..12 {
                    sum += level_at(world, water, BlockPos::new(x, y, z));
                }
            }
        }
        sum
    }

    #[test]
    fn a_level_is_a_flat_topped_mask_and_survives_the_round_trip() {
        for count in [0u32, 1, 7, 16, 33, 63, 64] {
            let mask = level_mask(count);
            assert_eq!(micro::remaining(mask), count, "{count} cells went in");
        }
        assert_eq!(level_mask(64), micro::FULL);
        assert_eq!(level_mask(200), micro::FULL, "a level cannot overflow");

        // Flat-topped: a filled layer is filled all the way across, and
        // nothing floats above the surface.
        let mask = level_mask(40);
        for y in 0..micro::SIDE {
            for z in 0..micro::SIDE {
                for x in 0..micro::SIDE {
                    let cell = micro::has(mask, x, y, z);
                    let index = x + 4 * z + 16 * y;
                    assert_eq!(cell, index < 40, "cell {x},{y},{z} of a forty-cell fill");
                }
            }
        }
    }

    #[test]
    fn water_falls_before_it_spreads() {
        let (mut world, water, at) = basin();
        // A block of water hanging three above the floor.
        let high = BlockPos::new(at.x, at.y + 3, at.z);
        set_level(&mut world, water, high, FULL);
        let mut body = Water::new(high);
        body.wake(high);

        for _ in 0..64 * 4 {
            body.settle(&mut world, water);
            if !body.busy() {
                break;
            }
        }
        assert_eq!(level_at(&world, water, high), 0, "it is still up there");
        assert!(
            level_at(&world, water, at) > 0,
            "it did not reach the floor"
        );
    }

    #[test]
    fn a_pool_conserves_every_drop_and_settles_flat() {
        let (mut world, water, at) = basin();
        let floor = at.y - 1;
        // Four full blocks poured into one column.
        let mut body = Water::new(at);
        for step in 0..4 {
            let pos = BlockPos::new(at.x, at.y + step, at.z);
            set_level(&mut world, water, pos, FULL);
            body.wake(pos);
        }
        let poured = total(&world, water, floor);
        assert_eq!(poured, 4 * FULL);

        for _ in 0..64 * 12 {
            body.settle(&mut world, water);
            if !body.busy() {
                break;
            }
        }

        let after = total(&world, water, floor);
        // Nothing here is a source, so nothing may appear or vanish beyond
        // the dregs the automaton is allowed to drop.
        assert!(
            after.abs_diff(poured) <= 4 * DREGS,
            "poured {poured} and ended with {after}"
        );
        assert!(!body.busy(), "the pool never settled");

        // And it spread out rather than staying in its column.
        let column = level_at(&world, water, at);
        assert!(column < 4 * FULL, "the water stayed in a tower: {column}");
        let mut wet = 0;
        for x in 1..11 {
            for z in 1..11 {
                if level_at(&world, water, BlockPos::new(x, at.y, z)) > 0 {
                    wet += 1;
                }
            }
        }
        assert!(wet > 4, "only {wet} blocks got wet");
    }

    #[test]
    fn the_answer_does_not_depend_on_the_order_the_blocks_woke() {
        // The property the replay oracle rests on. Same pours, different
        // wake order, same water.
        let run = |reverse: bool| {
            let (mut world, water, at) = basin();
            let floor = at.y - 1;
            let mut body = Water::new(at);
            let mut pours = Vec::new();
            for step in 0..3 {
                pours.push(BlockPos::new(at.x, at.y + step, at.z));
                pours.push(BlockPos::new(at.x + 1, at.y + step, at.z + 1));
            }
            for pos in &pours {
                set_level(&mut world, water, *pos, FULL);
            }
            if reverse {
                for pos in pours.iter().rev() {
                    body.wake(*pos);
                }
            } else {
                for pos in &pours {
                    body.wake(*pos);
                }
            }
            for _ in 0..64 * 12 {
                body.settle(&mut world, water);
                if !body.busy() {
                    break;
                }
            }
            let mut levels = Vec::new();
            for y in floor..floor + 8 {
                for x in 0..12 {
                    for z in 0..12 {
                        levels.push(level_at(&world, water, BlockPos::new(x, y, z)));
                    }
                }
            }
            levels
        };
        assert_eq!(run(false), run(true), "the wake order changed the water");
    }

    #[test]
    fn nothing_flows_past_the_body_s_reach() {
        let (_world, _water, at) = basin();
        let mut body = Water::new(at);
        // A block far outside the reach is refused rather than tracked.
        let far = BlockPos::new(at.x + REACH + 4, at.y, at.z);
        body.wake(far);
        assert!(body.awake().is_empty(), "a distant block joined the body");
        body.wake(at);
        assert_eq!(body.awake(), &[at]);
    }

    #[test]
    fn a_body_with_nowhere_to_go_retires() {
        let (mut world, water, at) = basin();
        set_level(&mut world, water, at, FULL);
        let mut body = Water::new(at);
        body.wake(at);
        let mut steps = 0;
        while body.busy() && steps < 64 * 20 {
            body.settle(&mut world, water);
            steps += 1;
        }
        assert!(!body.busy(), "the water is still fidgeting");
        assert!(steps < 64 * 20, "it took {steps} steps to give up");
    }

    #[test]
    fn a_pump_lifts_water_over_its_own_head() {
        let (mut world, water, at) = basin();
        // A pool beside the pump, and the pump standing in it.
        let stone = world.registry().id_of("engine:stone").unwrap();
        let pump_at = BlockPos::new(at.x, at.y, at.z);
        world.set_block(pump_at, stone);
        let pool = BlockPos::new(at.x + 1, at.y, at.z);
        set_level(&mut world, water, pool, FULL);

        let spout = pump_at.neighbour(Face::PosY);
        let mut lifted = 0;
        for _ in 0..6 {
            if pump(&mut world, water, pump_at).is_some() {
                lifted += 1;
            }
        }
        assert!(lifted > 0, "the pump never moved anything");
        assert!(
            level_at(&world, water, spout) > 0,
            "nothing came out of the spout"
        );
        assert!(
            level_at(&world, water, pool) < FULL,
            "the pool did not go down"
        );
        // Conserved: what left the pool is what arrived at the spout.
        assert_eq!(
            level_at(&world, water, pool) + level_at(&world, water, spout),
            FULL
        );
    }

    #[test]
    fn a_pump_on_the_shore_never_empties_the_sea() {
        let mut world = World::new(2024);
        world.load_around(ChunkPos::new(0, 0), 1);
        let water = water_id(&world);
        let stone = world.registry().id_of("engine:stone").unwrap();
        let deep = crate::gen::SEA_LEVEL - 3;
        let pump_at = BlockPos::new(5, deep, 5);
        world.set_block(pump_at, stone);
        // The spout has to have somewhere to spill: this deep the ground is
        // solid, and a pump under a roof pumps nothing.
        world.set_block(pump_at.neighbour(Face::PosY), BlockId::AIR);
        let sea = BlockPos::new(6, deep, 5);
        world.set_block(sea, water);
        for _ in 0..8 {
            pump(&mut world, water, pump_at);
        }
        assert_eq!(level_at(&world, water, sea), FULL, "the sea went down");
        assert!(level_at(&world, water, pump_at.neighbour(Face::PosY)) > 0);
    }

    #[test]
    fn the_sea_supplies_without_draining() {
        // A source cell is one the generator filled at or below sea level.
        let mut world = World::new(2024);
        world.load_around(ChunkPos::new(0, 0), 1);
        let water = water_id(&world);
        let sea = BlockPos::new(3, crate::gen::SEA_LEVEL - 2, 3);
        world.set_block(sea, water);
        assert!(is_source(&world, water, sea), "the sea is not a source");

        // Half-empty it and it is no longer the sea: what the player put
        // there is finite, and so is what they took.
        set_level(&mut world, water, sea, 32);
        assert!(!is_source(&world, water, sea));
        set_level(&mut world, water, sea, FULL);
        assert!(is_source(&world, water, sea));

        // And a block above the line is finite however full it is.
        let pond = BlockPos::new(3, crate::gen::SEA_LEVEL + 6, 3);
        world.set_block(pond, water);
        assert!(!is_source(&world, water, pond));
    }
}
