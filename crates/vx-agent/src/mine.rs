//! How to open a mine on a body of ore.
//!
//! # The method *is* the access problem
//!
//! It is tempting to treat "how the drone digs" as decoration on top of "the
//! drone digs". It is not. A ground drone drives, and [`crate::flow`] lets it
//! change height by one block per step and no more. So the shape of the
//! excavation decides whether the ore can be reached and hauled out at all —
//! which is exactly the question real operations answer when they choose
//! between an adit, a decline and an open pit.
//!
//! Three methods, ordered by what they cost the drone:
//!
//! - [`MineMethod::Adit`] — a level tunnel straight into a hillside. No
//!   climbing at all, in or out. Only possible when the body meets a slope,
//!   which is exactly the situation the stage-1 outcrop rule produces. A
//!   hillside find is therefore worth materially more than the same body under
//!   flat ground, and nobody had to write that rule down.
//! - [`MineMethod::Decline`] — the diagonal ramp. Works for any body at any
//!   depth; the loaded haul is uphill, which is what will make carry capacity
//!   and fuel matter. The ramp is walkable, which is what will make recovering
//!   a broken-down drone possible.
//! - [`MineMethod::Pit`] — benched terraces, where the benches *are* the road.
//!   Access comes free, but the volume grows with roughly the cube of depth,
//!   which is why real pits eventually become underground mines. Capped at
//!   [`PIT_MAX_DEPTH`] for that reason.
//!
//! A vertical shaft is **deliberately absent**. It is the cheapest volume of
//! the lot and nothing that drives can climb out of one, so it belongs to the
//! flying drone as a later unlock — and the asymmetry is the point: a drone
//! that dies down a shaft is stuck until the flier can lift it, while a drone
//! that dies down a decline can be walked to.
//!
//! # Grade, and what it does not do yet
//!
//! `grade` is blocks of run per block of rise, and it shapes every ramp cut
//! here: a 1:4 decline is four times the tunnel of a 1:1 one.
//! [`MinePlan::steepest_step`] measures what actually came out, so the geometry
//! side of it is pinned.
//!
//! **It is not yet a limit on what a drone can drive.** [`crate::flow`] allows
//! a one-block change per step, so a 1:1 ramp — a staircase, the steepest
//! anything can climb a block at a time — is traversable by every drone. What
//! stops a real loaded machine taking a staircase is traction, and modelling
//! that needs a field aware of how far a drone has run since it last climbed:
//! state per `(cell, run)` rather than per cell. That is a contained change and
//! it is the honest next step, but until it lands, "the drone needs a gentler
//! ramp" is a statement about the excavation, not about the drone.

use vx_core::{BlockPos, CHUNK_HEIGHT};
use vx_world::World;

use crate::aabb::VoxelAabb;

/// How to open a mine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MineMethod {
    /// Level tunnel into a hillside.
    Adit,
    /// Diagonal ramp down from a surface portal.
    Decline,
    /// Benched terraces from the surface down.
    Pit,
}

impl MineMethod {
    pub const ALL: [MineMethod; 3] = [MineMethod::Adit, MineMethod::Decline, MineMethod::Pit];

    pub fn name(self) -> &'static str {
        match self {
            MineMethod::Adit => "adit",
            MineMethod::Decline => "decline",
            MineMethod::Pit => "open pit",
        }
    }
}

/// Half-width of a tunnel, so corridors come out three blocks wide.
///
/// One-wide tunnels are cheaper and read as mouse holes; three is the narrowest
/// that looks like something a machine drove through.
const TUNNEL_HALF_WIDTH: i32 = 1;

/// Height of a tunnel, in blocks.
///
/// Two, and not by taste: a drone cuts level with itself and one block up, so
/// two is the tallest tunnel it can drive out of its own floor. A three-block
/// corridor needs somebody standing a block off the ground to cut the roof, and
/// on a level tunnel there is nowhere for that somebody to stand — the job
/// would sit there forever with its top layer intact.
const TUNNEL_HEIGHT: i32 = 1 + crate::flow::STEP;

/// How far to look sideways for a hillside before giving up on an adit.
const ADIT_MAX_RUN: i32 = 64;

/// Cap on how far a decline portal may be set back from the body.
///
/// Generous: the ramp itself needs `depth * grade`, and rising ground needs
/// more still. Reaching this means the terrain climbs faster than the drone
/// can, and the pit takes over.
const DECLINE_MAX_RUN: i32 = 512;

/// Past this depth an open pit stops being sensible.
///
/// The excavated volume grows with roughly the cube of depth, which is the same
/// arithmetic that turns real open pits into underground mines.
pub const PIT_MAX_DEPTH: i32 = 24;

/// How far each pit bench is set back from the one below, per block of height.
///
/// One block, which with one-block benches is a 45° wall — the steepest thing a
/// drone can drive and therefore the least rock moved. Note that the drone's
/// grade does **not** come into this: a one-block bench is one step whatever
/// the drone is, so the pit is the one method where the stat has no say.
const BENCH_SETBACK: i32 = crate::flow::STEP;

/// Blocks of digging that one block of loaded climb is judged to be worth,
/// as a fraction of the excavation — a lift of one block over a mine of
/// `volume` blocks costs `volume / LIFT_DIVISOR`.
///
/// Without this, ranking on dug blocks alone would never choose an adit: a
/// decline can always start part-way down a slope and cut a shorter tunnel. But
/// the excavation is dug once and the haul is made on **every load, forever** —
/// and later the fuel run and the walk to recover a dead drone use the same
/// route. A level way in is worth more than its block count says, and this is
/// where that is written down.
const LIFT_DIVISOR: u64 = 16;

/// An excavation plan: where to go in, what to cut, and what it costs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinePlan {
    pub method: MineMethod,
    /// Where a drone enters from the surface. The mouth of an adit, the top of
    /// a decline, the rim of a pit.
    pub portal: BlockPos,
    /// Excavation that creates the route in, **outermost first**. Cutting these
    /// in order is what keeps the drone always standing on ground it can leave.
    pub access: Vec<VoxelAabb>,
    /// The body itself.
    pub extraction: Vec<VoxelAabb>,
    /// Blocks to move, access and extraction together. What the player is shown
    /// when choosing between methods.
    pub volume: u64,
}

impl MinePlan {
    /// The largest single change in floor height between consecutive access
    /// steps.
    ///
    /// Must be at most [`crate::flow::STEP`] or the excavation contains a step
    /// no drone can climb, and the mine is a hole with the ore stuck at the
    /// bottom of it.
    pub fn steepest_step(&self) -> i32 {
        self.access
            .windows(2)
            .map(|pair| (pair[0].min.y - pair[1].min.y).abs())
            .max()
            .unwrap_or(0)
    }

    /// How far a loaded drone has to climb to get from the top of the body out
    /// to the portal. Zero for an adit; that is the whole point of one.
    pub fn lift(&self) -> i32 {
        let top = self
            .extraction
            .iter()
            .map(|region| region.max.y)
            .max()
            .unwrap_or(self.portal.y);
        (self.portal.y - top).max(0)
    }

    /// What this plan is judged to be worth, for ranking methods against each
    /// other.
    ///
    /// Blocks to dig, plus a charge for the climb every load will make. Keep
    /// [`MinePlan::volume`] for anything shown to a player as "rock moved" —
    /// that is a fact, this is a judgement.
    pub fn cost(&self) -> u64 {
        self.volume + (self.lift() as u64) * self.volume / LIFT_DIVISOR
    }

    /// Every region, access before extraction.
    pub fn regions(&self) -> impl Iterator<Item = (bool, &VoxelAabb)> {
        self.access
            .iter()
            .map(|region| (true, region))
            .chain(self.extraction.iter().map(|region| (false, region)))
    }
}

/// Height of the topmost solid block in a column, or `None` when the column is
/// not loaded or is empty.
pub fn ground_height(world: &World, x: i32, z: i32) -> Option<i32> {
    // `surface_y` reports the first clear block above the ground.
    world.surface_y(x, z).map(|clear| clear - 1)
}

/// The part of `area` worth mining: the bounding box of the ore inside it, or
/// the whole marked area when there is none.
///
/// Marking a box with no ore in it is not an error — it is "dig this out", and
/// it is how a player cuts a cellar or a road. The planner does not need to
/// know how ore is generated, only how to recognise it, which keeps
/// [`vx_world::ore`] out of the dependency path entirely.
pub fn target_body(world: &World, area: VoxelAabb) -> VoxelAabb {
    let area = area.clamped_to_world();
    let ore: Vec<BlockPos> = area
        .blocks()
        .filter(|pos| {
            world
                .registry()
                .get(world.block(*pos))
                .is_some_and(|def| def.name.ends_with("_ore"))
        })
        .collect();
    VoxelAabb::containing(ore).unwrap_or(area)
}

/// A box `TUNNEL_HALF_WIDTH` either side of the centre line, `TUNNEL_HEIGHT`
/// tall, spanning `along` on the tunnel's axis.
fn corridor_slice(floor: BlockPos, axis_x: bool, along: (i32, i32)) -> VoxelAabb {
    let (lo, hi) = (along.0.min(along.1), along.0.max(along.1));
    if axis_x {
        VoxelAabb::new(
            BlockPos::new(lo, floor.y, floor.z - TUNNEL_HALF_WIDTH),
            BlockPos::new(hi, floor.y + TUNNEL_HEIGHT - 1, floor.z + TUNNEL_HALF_WIDTH),
        )
    } else {
        VoxelAabb::new(
            BlockPos::new(floor.x - TUNNEL_HALF_WIDTH, floor.y, lo),
            BlockPos::new(floor.x + TUNNEL_HALF_WIDTH, floor.y + TUNNEL_HEIGHT - 1, hi),
        )
    }
}

/// The four horizontal directions a tunnel can run.
const HEADINGS: [[i32; 3]; 4] = [[1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]];

/// Sum of region volumes, counting overlaps once would be nicer but the regions
/// a planner emits do not overlap.
fn total_volume(regions: &[VoxelAabb], extraction: &[VoxelAabb]) -> u64 {
    regions.iter().chain(extraction).map(VoxelAabb::volume).sum()
}

/// Cut the body into benches, stepping out one block per layer toward `heading`.
///
/// **A body cannot be taken out as a plain box.** Clearing a box layer by layer
/// leaves vertical walls, and a drone that has worked to the floor of one
/// cannot climb four blocks back out — it ends up standing on its own ore with
/// a full load and nowhere to take it. That is not hypothetical; it is what the
/// straight-box version did.
///
/// So the stope is benched, exactly like the open pit and for the same reason.
/// Each layer reaches one block further along `heading` than the layer beneath,
/// leaving a one-block step at every level: a staircase down one wall of the
/// working. The bottom bench is the body's true footprint, so every block of
/// ore still comes out. The extra is waste cut to make the ramp, and it grows
/// with the square of the body's height — the honest price of taking a deep
/// body out through a hole you can drive.
///
/// **`heading` must point at the access.** The staircase is the only part of
/// the finished stope that is still standable once the layer beneath has gone,
/// so the tunnel has to arrive at its top. Bench one way and drive in from
/// another and the ramp ends over thin air. Benching one wall rather than all
/// four is also a quarter of the waste for the same guarantee.
fn benched_extraction(target: VoxelAabb, heading: [i32; 3]) -> Vec<VoxelAabb> {
    (target.min.y..=target.max.y)
        .rev()
        .map(|y| {
            let out = y - target.min.y;
            // Grow the side the heading points at, leaving the other three
            // walls where the body ends.
            let (dx, dz) = (heading[0] * out, heading[2] * out);
            VoxelAabb::new(
                BlockPos::new(target.min.x + dx.min(0), y, target.min.z + dz.min(0)),
                BlockPos::new(target.max.x + dx.max(0), y, target.max.z + dz.max(0)),
            )
        })
        .collect()
}

/// How far the topmost bench of a stope reaches past the body.
fn stope_reach(target: VoxelAabb) -> i32 {
    target.max.y - target.min.y
}

/// Where a tunnel has to end to meet the top of the benched stope: one block
/// past the widest bench, and **level with it**.
///
/// Two mistakes are baked out of this. Aiming at the body's centre — the
/// obvious thing — puts the end of the tunnel directly over ore that is later
/// dug out from under it, leaving the tunnel hanging over the hole it was meant
/// to serve. Arriving one block *above* the top bench looks harmless and is
/// not: a drone cuts level with itself and above, never below, so from a floor
/// one block high it can never start the stope at all.
fn tunnel_entry(target: VoxelAabb, heading: [i32; 3]) -> BlockPos {
    let out = stope_reach(target) + 1;
    let centre = target.centre();
    let face = if heading[0] > 0 {
        BlockPos::new(target.max.x, 0, centre.z)
    } else if heading[0] < 0 {
        BlockPos::new(target.min.x, 0, centre.z)
    } else if heading[2] > 0 {
        BlockPos::new(centre.x, 0, target.max.z)
    } else {
        BlockPos::new(centre.x, 0, target.min.z)
    };
    BlockPos::new(
        face.x + heading[0] * out,
        target.max.y,
        face.z + heading[2] * out,
    )
}

/// Plan an adit: a level tunnel from a hillside into the top of the body.
///
/// Returns `None` when the body does not meet a slope within [`ADIT_MAX_RUN`],
/// which is most of the time — that scarcity is what makes a hillside outcrop
/// valuable.
fn plan_adit(world: &World, target: VoxelAabb) -> Option<MinePlan> {
    for heading in HEADINGS {
        // The tunnel ends past the top bench of the stope, not at the body's
        // centre — see `tunnel_entry`.
        let entry = tunnel_entry(target, heading);
        let axis_x = heading[0] != 0;
        let step = if axis_x { heading[0] } else { heading[2] };
        let floor_y = entry.y;
        let start = if axis_x { entry.x } else { entry.z };

        // March outward from the entry until the hillside surface drops to the
        // tunnel's own level: that is where the mouth would be.
        for run in 1..=ADIT_MAX_RUN {
            let offset = start + step * run;
            let (x, z) = if axis_x { (offset, entry.z) } else { (entry.x, offset) };
            let Some(ground) = ground_height(world, x, z) else {
                break; // Off the loaded world; no adit this way.
            };

            // The mouth is where the ground meets the tunnel floor. One block
            // of slack keeps a drone from having to climb in.
            if ground >= floor_y {
                continue;
            }
            if ground < floor_y - 1 - crate::flow::STEP {
                // The ground fell away faster than a drone can drop: this
                // heading is a cliff, not a portal.
                break;
            }

            let portal = BlockPos::new(x, floor_y, z);
            // The corridor runs from the mouth back to the entry; the body
            // itself is extraction, not access, so it is not counted twice.
            let access = vec![corridor_slice(entry, axis_x, (offset, start))];

            let extraction = benched_extraction(target, heading);
            let volume = total_volume(&access, &extraction);
            return Some(MinePlan {
                method: MineMethod::Adit,
                portal,
                access,
                extraction,
                volume,
            });
        }
    }
    None
}

/// Plan a decline: a ramp from a surface portal down to the top of the body.
///
/// The portal is chosen, not assumed. For each heading the search walks outward
/// looking for the *closest* surface point from which a ramp at the drone's
/// grade still reaches — which on a slope is much closer than on the flat, and
/// is why a decline into a hillside costs less than one into a plain.
fn plan_decline(world: &World, target: VoxelAabb, grade: i32) -> Option<MinePlan> {
    let grade = grade.max(1);
    let mut best: Option<(u64, MinePlan)> = None;

    for heading in HEADINGS {
        // The ramp ends past the top bench of the stope, not over the body's
        // centre — see `tunnel_entry`.
        let entry = tunnel_entry(target, heading);
        let entry_y = entry.y;
        let axis_x = heading[0] != 0;
        let step = if axis_x { heading[0] } else { heading[2] };
        let start = if axis_x { entry.x } else { entry.z };

        for run in 1..=DECLINE_MAX_RUN {
            let offset = start + step * run;
            let (x, z) = if axis_x { (offset, entry.z) } else { (entry.x, offset) };
            let Some(ground) = ground_height(world, x, z) else {
                break;
            };

            // Height the ramp has to lose, measured from the standable cell at
            // the portal down to the entry it must actually arrive at. Getting
            // this off by one leaves the ramp hanging a block above the stope
            // with no way across.
            let rise = ground + 1 - entry_y;
            if rise < 0 {
                // The surface here is already below the entry; a level tunnel
                // would do, and that is the adit's job.
                continue;
            }
            // The ramp has `run` blocks of horizontal distance to lose `rise`
            // blocks of height at one block per `grade`.
            if rise * grade > run {
                continue;
            }

            let portal = BlockPos::new(x, ground + 1, z);
            let access = ramp_slices(portal, axis_x, step, run, rise);
            let extraction = benched_extraction(target, heading);
            let volume = total_volume(&access, &extraction);

            if best.as_ref().is_none_or(|(cheapest, _)| volume < *cheapest) {
                best = Some((
                    volume,
                    MinePlan {
                        method: MineMethod::Decline,
                        portal,
                        access,
                        extraction,
                        volume,
                    },
                ));
            }
            break; // The first workable run on this heading is the cheapest.
        }
    }

    best.map(|(_, plan)| plan)
}

/// Cut a ramp into one box per bench, outermost first.
///
/// Floors are interpolated so the drop is spread evenly over the run, which is
/// what keeps every step within one block. Consecutive positions at the same
/// floor are merged into a single box, so a 1:4 ramp emits one four-long bench
/// per block of descent rather than four boxes.
fn ramp_slices(portal: BlockPos, axis_x: bool, step: i32, run: i32, rise: i32) -> Vec<VoxelAabb> {
    let mut slices = Vec::new();
    let mut bench_start = 0;
    let mut bench_floor = portal.y;

    let floor_at = |i: i32| -> i32 {
        if run == 0 {
            return portal.y - rise;
        }
        // Rounded so the descent is as even as the integers allow.
        portal.y - (rise as i64 * i as i64 / run as i64) as i32
    };

    let position = |i: i32| -> i32 {
        let base = if axis_x { portal.x } else { portal.z };
        base - step * i
    };

    for i in 1..=run {
        let floor = floor_at(i);
        if floor != bench_floor {
            slices.push(corridor_slice(
                BlockPos::new(portal.x, bench_floor, portal.z),
                axis_x,
                (position(bench_start), position(i - 1)),
            ));
            bench_start = i;
            bench_floor = floor;
        }
    }
    slices.push(corridor_slice(
        BlockPos::new(portal.x, bench_floor, portal.z),
        axis_x,
        (position(bench_start), position(run)),
    ));

    slices
}

/// Plan an open pit: one-block benches stepping down to the body, each set back
/// from the one below so the terraces themselves are the haul road.
///
/// Always possible — you can dig down from the surface anywhere — which makes
/// this the geometric backstop when no tunnel works.
fn plan_pit(world: &World, target: VoxelAabb) -> Option<MinePlan> {
    let setback = BENCH_SETBACK;
    // The stope is benched toward -z, so the pit's own benches have to be wide
    // enough on that side to sit over the stope's staircase rather than cut it
    // off in mid-air.
    const HEADING: [i32; 3] = [0, 0, -1];
    let extraction = benched_extraction(target, HEADING);
    let workings = extraction
        .iter()
        .copied()
        .reduce(VoxelAabb::union)
        .unwrap_or(target);

    // The rim sits at the highest ground over the workings; taking the lowest
    // would leave benches buried in the hillside.
    let mut rim = i32::MIN;
    for x in workings.min.x..=workings.max.x {
        for z in workings.min.z..=workings.max.z {
            rim = rim.max(ground_height(world, x, z)?);
        }
    }

    let mut access = Vec::new();
    // Top bench first, so the drone always cuts from ground it can stand on.
    for y in ((target.max.y + 1)..=rim).rev() {
        let height_above = y - target.max.y;
        let bench = VoxelAabb::new(
            BlockPos::new(
                workings.min.x - height_above * setback,
                y,
                workings.min.z - height_above * setback,
            ),
            BlockPos::new(
                workings.max.x + height_above * setback,
                y,
                workings.max.z + height_above * setback,
            ),
        );
        access.push(bench);
    }

    let volume = total_volume(&access, &extraction);

    // The portal is *searched for*, not fabricated. The earlier version placed
    // it arithmetically one block beyond the -z rim — and on ground rising
    // that way the column is higher than the rim, the "portal" was inside the
    // hill, and the drone spawned entombed. Instead: search every column of
    // the pit's footprint (rim ring *and* interior — starting on top of rock
    // that is about to be dug is fine, the pit is worked top-down) for ground
    // within a drivable step of the rim, and stand the drone there.
    //
    // The column where the rim height is attained always qualifies, so a pit
    // that passed the depth check always finds a portal; the `?` below only
    // fires at the edge of the loaded world.
    let rim_setback = (rim - target.max.y).max(0) * setback;
    let edge = VoxelAabb::new(
        BlockPos::new(workings.min.x - rim_setback - 1, rim, workings.min.z - rim_setback - 1),
        BlockPos::new(workings.max.x + rim_setback + 1, rim, workings.max.z + rim_setback + 1),
    );
    let centre = target.centre();
    let portal = (edge.min.x..=edge.max.x)
        .flat_map(|x| (edge.min.z..=edge.max.z).map(move |z| (x, z)))
        .filter_map(|(x, z)| {
            let ground = ground_height(world, x, z)?;
            ((ground - rim).abs() <= crate::flow::STEP).then_some(BlockPos::new(x, ground + 1, z))
        })
        .min_by_key(|pos| {
            let (dx, dz) = ((pos.x - centre.x) as i64, (pos.z - centre.z) as i64);
            (dx * dx + dz * dz, pos.x, pos.z)
        })?;

    Some(MinePlan {
        method: MineMethod::Pit,
        portal,
        access,
        extraction,
        volume,
    })
}

/// Plan `area` using a specific method, or `None` when that method does not
/// apply here.
///
/// This is what backs the override: a player can ask for an adit and be told
/// there is no hillside to drive into.
pub fn plan(world: &World, area: VoxelAabb, grade: i32, method: MineMethod) -> Option<MinePlan> {
    let target = target_body(world, area);
    if target.max.y >= CHUNK_HEIGHT {
        return None;
    }
    match method {
        MineMethod::Adit => plan_adit(world, target),
        MineMethod::Decline => plan_decline(world, target, grade),
        MineMethod::Pit => {
            if (pit_depth(world, target)?) > PIT_MAX_DEPTH {
                return None;
            }
            plan_pit(world, target)
        }
    }
}

/// How far below the surface the top of the body sits.
fn pit_depth(world: &World, target: VoxelAabb) -> Option<i32> {
    let mut rim = i32::MIN;
    for x in target.min.x..=target.max.x {
        for z in target.min.z..=target.max.z {
            rim = rim.max(ground_height(world, x, z)?);
        }
    }
    Some((rim - target.max.y).max(0))
}

/// Every method that would work here, with its cost, cheapest first.
///
/// This is what the override menu shows: not just what the game chose, but what
/// disagreeing would cost.
pub fn options(world: &World, area: VoxelAabb, grade: i32) -> Vec<MinePlan> {
    let mut found: Vec<MinePlan> = MineMethod::ALL
        .into_iter()
        .filter_map(|method| plan(world, area, grade, method))
        .collect();
    found.sort_by_key(MinePlan::cost);
    found
}

/// The method to use unless the player says otherwise: the cheapest that works.
///
/// Adit usually wins outright when it is available, because a level tunnel
/// moves far less rock than a ramp or a pit — the ranking falls out of the
/// volume rather than being asserted.
pub fn propose(world: &World, area: VoxelAabb, grade: i32) -> Option<MinePlan> {
    options(world, area, grade).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{hillside, ore_body};
    use crate::flow::{FlowField, STEP};
    use vx_core::BlockId;

    /// Wide enough that a decline's run-up lands inside the loaded world.
    const RADIUS: i32 = 8;

    fn flat_world(floor: i32) -> World {
        crate::fixture::flat(RADIUS, floor)
    }

    fn hillside_world(high: i32, crest: i32) -> World {
        hillside(RADIUS, high, crest)
    }

    /// Dig out every access region, as the drone eventually will.
    fn cut_access(world: &mut World, plan: &MinePlan) {
        for region in &plan.access {
            for pos in region.clamped_to_world().blocks() {
                world.set_block(pos, BlockId::AIR);
            }
        }
    }

    /// Cells a drone could stand in and reach into the body from.
    ///
    /// Deliberately measured against *access only*, with the body still in the
    /// ground. Clearing the whole extraction volume first would leave a
    /// straight-sided void the drone never actually cuts — it digs the body
    /// down layer by layer, always standing on what is left. So the question
    /// the access has to answer is "can it reach the face", not "is the
    /// finished void drivable".
    fn dig_stations(world: &World, plan: &MinePlan) -> Vec<BlockPos> {
        let body = plan
            .extraction
            .iter()
            .copied()
            .reduce(VoxelAabb::union)
            .expect("a plan with nothing to extract");

        body.expanded(1)
            .clamped_to_world()
            .blocks()
            .filter(|pos| !body.contains(*pos))
            .filter(|pos| crate::flow::is_standable(world, *pos))
            .collect()
    }

    /// The invariant every method has to satisfy: after cutting the access, a
    /// drone starting at the portal can drive to the face of the body.
    fn assert_ore_is_drivable(world: &mut World, plan: &MinePlan) {
        cut_access(world, plan);

        let start = crate::flow::settle(world, plan.portal);
        assert!(
            crate::flow::is_standable(world, start),
            "{}: the portal at {:?} is not somewhere a drone can stand",
            plan.method.name(),
            plan.portal
        );

        let stations = dig_stations(world, plan);
        assert!(
            !stations.is_empty(),
            "{}: nowhere to stand next to the body once the access is cut",
            plan.method.name()
        );

        let bounds = plan
            .access
            .iter()
            .chain(&plan.extraction)
            .fold(VoxelAabb::single(start), |acc, region| acc.union(*region))
            .expanded(4);

        let field = FlowField::build(world, bounds, [start]);
        let reached = stations.iter().filter(|pos| field.is_reachable(**pos)).count();

        assert!(
            reached > 0,
            "{}: none of the {} cells beside the body can be driven to from the portal; \
             the excavation is a hole with the ore stuck at the bottom of it",
            plan.method.name(),
            stations.len()
        );
    }

    #[test]
    fn the_target_is_the_ore_inside_the_marked_area_not_the_whole_area() {
        let mut world = flat_world(60);
        let body = VoxelAabb::new(BlockPos::new(2, 40, 2), BlockPos::new(5, 44, 5));
        ore_body(&mut world, body);

        let marked = VoxelAabb::new(BlockPos::new(-10, 30, -10), BlockPos::new(20, 55, 20));
        assert_eq!(target_body(&world, marked), body);
    }

    #[test]
    fn marking_ground_with_no_ore_just_excavates_the_box() {
        let world = flat_world(60);
        let marked = VoxelAabb::new(BlockPos::new(0, 50, 0), BlockPos::new(4, 54, 4));
        assert_eq!(target_body(&world, marked), marked);
    }

    #[test]
    fn an_adit_is_only_offered_where_the_body_meets_a_slope() {
        // On the flat there is no hillside to drive into, and offering one
        // would be a lie the player pays for in dug rock.
        let mut world = flat_world(60);
        let buried = VoxelAabb::new(BlockPos::new(0, 40, 0), BlockPos::new(3, 43, 3));
        ore_body(&mut world, buried);
        assert!(
            plan(&world, buried, 3, MineMethod::Adit).is_none(),
            "an adit was offered on flat ground"
        );

        // On a hillside, at a level the slope actually reaches, it is offered.
        let mut hill = hillside_world(60, 0);
        let exposed = VoxelAabb::new(BlockPos::new(-12, 52, 0), BlockPos::new(-9, 56, 3));
        ore_body(&mut hill, exposed);
        let adit = plan(&hill, exposed, 3, MineMethod::Adit)
            .expect("a body inside a hillside should be reachable by adit");
        assert_eq!(adit.method, MineMethod::Adit);
    }

    #[test]
    fn an_adit_is_level_from_end_to_end() {
        // The property that makes it the cheapest haul: no climbing at all.
        let mut hill = hillside_world(60, 0);
        let body = VoxelAabb::new(BlockPos::new(-12, 52, 0), BlockPos::new(-9, 56, 3));
        ore_body(&mut hill, body);

        let adit = plan(&hill, body, 3, MineMethod::Adit).unwrap();
        assert_eq!(adit.steepest_step(), 0, "the adit changes height");
        // Level with the stope's top bench: a drone cuts level and above, so a
        // tunnel arriving any higher could never start the body.
        assert_eq!(adit.portal.y, body.max.y);
    }

    #[test]
    fn a_decline_never_exceeds_the_drones_climbing_limit() {
        // The check that keeps grade an actual constraint rather than a label.
        // A generated ramp with a two-block step is a ramp no drone can use.
        for grade in [1, 2, 3, 5] {
            let mut world = flat_world(60);
            // Shallow enough that even a 1:5 ramp's run-up fits in the fixture.
            let body = VoxelAabb::new(BlockPos::new(0, 45, 0), BlockPos::new(3, 48, 3));
            ore_body(&mut world, body);

            let decline = plan(&world, body, grade, MineMethod::Decline)
                .unwrap_or_else(|| panic!("no decline at grade 1:{grade}"));
            assert!(
                decline.steepest_step() <= STEP,
                "grade 1:{grade} produced a {}-block step",
                decline.steepest_step()
            );
        }
    }

    #[test]
    fn a_gentler_grade_costs_more_digging() {
        // The upgrade path's whole justification: a drone that climbs better
        // opens mines more cheaply.
        let mut world = flat_world(60);
        let body = VoxelAabb::new(BlockPos::new(0, 30, 0), BlockPos::new(3, 33, 3));
        ore_body(&mut world, body);

        let steep = plan(&world, body, 1, MineMethod::Decline).unwrap();
        let gentle = plan(&world, body, 4, MineMethod::Decline).unwrap();
        assert!(
            gentle.volume > steep.volume,
            "1:4 ({}) should move more rock than 1:1 ({})",
            gentle.volume,
            steep.volume
        );
    }

    #[test]
    fn a_pit_is_widest_at_the_rim_and_narrowest_at_the_ore() {
        let mut world = flat_world(60);
        let body = VoxelAabb::new(BlockPos::new(0, 50, 0), BlockPos::new(3, 53, 3));
        ore_body(&mut world, body);

        let pit = plan(&world, body, 3, MineMethod::Pit).unwrap();
        assert!(pit.access.len() > 1, "the pit has no benches");

        let widths: Vec<i64> = pit.access.iter().map(|bench| bench.size()[0]).collect();
        assert!(
            widths.windows(2).all(|pair| pair[0] > pair[1]),
            "benches do not narrow with depth: {widths:?}"
        );
        // Benches are one block tall, so every step down is climbable.
        assert!(pit.access.iter().all(|bench| bench.size()[1] == 1));
    }

    #[test]
    fn a_pit_is_refused_once_it_would_be_absurd() {
        // Volume grows with the cube of depth. Past the cap the answer is a
        // tunnel, which is the same conclusion real operations reach.
        let mut world = flat_world(60);
        let deep = VoxelAabb::new(BlockPos::new(0, 20, 0), BlockPos::new(3, 23, 3));
        ore_body(&mut world, deep);

        assert!(
            plan(&world, deep, 3, MineMethod::Pit).is_none(),
            "a pit was offered for a body {} blocks down",
            60 - 23
        );
        assert!(
            plan(&world, deep, 3, MineMethod::Decline).is_some(),
            "a decline should still work at any depth"
        );
    }

    #[test]
    fn the_pit_portal_is_found_on_the_ground_not_fabricated_into_it() {
        // Finding A2. Ground rising toward -z is exactly where the old
        // arithmetic portal ended up inside the hill, entombing the drone. The
        // portal must now be a cell a drone can actually occupy.
        let mut world = crate::fixture::shaped_xz(RADIUS, |_, z| (60 - z).clamp(30, 90));
        let body = VoxelAabb::new(BlockPos::new(0, 52, 0), BlockPos::new(3, 55, 3));
        ore_body(&mut world, body);

        let pit = plan(&world, body, 3, MineMethod::Pit)
            .expect("a shallow body on a gentle slope should still offer a pit");
        let settled = crate::flow::settle(&world, pit.portal);
        assert!(
            crate::flow::is_standable(&world, settled),
            "the pit portal at {:?} is not somewhere a drone can stand",
            pit.portal
        );
    }

    #[test]
    fn a_body_under_a_steep_knoll_gets_a_pit_from_the_crown() {
        // The other terrain the old arithmetic portal could not survive: a
        // body just under the crown of a knoll falling away steeply on every
        // side. No column *outside* the workings is anywhere near the rim, so
        // the portal has to be allowed inside the footprint — the drone starts
        // on top of rock it is about to dig, which is fine, because the pit is
        // worked top-down.
        let mut world = crate::fixture::shaped_xz(RADIUS, |x, z| {
            let distance = x.abs().max(z.abs());
            (66 - 3 * (distance - 2).max(0)).clamp(24, 66)
        });
        let body = VoxelAabb::new(BlockPos::new(-1, 60, -1), BlockPos::new(1, 62, 1));
        ore_body(&mut world, body);

        let pit = plan(&world, body, 3, MineMethod::Pit)
            .expect("a shallow body under a knoll crown should still offer a pit");
        let settled = crate::flow::settle(&world, pit.portal);
        assert!(
            crate::flow::is_standable(&world, settled),
            "the pit portal at {:?} is not somewhere a drone can stand",
            pit.portal
        );
    }

    #[test]
    fn every_method_leaves_the_ore_drivable() {
        // The one that matters. A plan that digs a beautiful hole the drone
        // cannot get into is worthless, and this catches it for all three
        // generators with one assertion.
        type Site = (&'static str, fn() -> World, VoxelAabb);
        let sites: [Site; 3] = [
            (
                "shallow flat",
                || flat_world(60),
                VoxelAabb::new(BlockPos::new(0, 48, 0), BlockPos::new(3, 51, 3)),
            ),
            (
                "deep flat",
                || flat_world(60),
                VoxelAabb::new(BlockPos::new(0, 28, 0), BlockPos::new(3, 32, 3)),
            ),
            (
                "hillside",
                || hillside_world(60, 0),
                VoxelAabb::new(BlockPos::new(-12, 52, 0), BlockPos::new(-9, 56, 3)),
            ),
        ];

        for (label, build, body) in sites {
            let mut world = build();
            ore_body(&mut world, body);
            let plans = options(&world, body, 3);
            assert!(!plans.is_empty(), "{label}: no method applies at all");

            // Each plan gets a fresh world: cutting one method's access would
            // otherwise make the next method's look better than it is.
            for plan in plans {
                let mut scratch = build();
                ore_body(&mut scratch, body);
                assert_ore_is_drivable(&mut scratch, &plan);
            }
        }
    }

    #[test]
    fn something_is_always_proposed() {
        // A player marking a body must never be told "no". The pit is the
        // geometric backstop: digging down from the surface always works.
        for floor in [40, 60, 90] {
            let mut world = flat_world(floor);
            let body = VoxelAabb::new(
                BlockPos::new(0, floor - 20, 0),
                BlockPos::new(3, floor - 17, 3),
            );
            ore_body(&mut world, body);
            assert!(
                propose(&world, body, 3).is_some(),
                "nothing proposed for a body under a floor at {floor}"
            );
        }
    }

    #[test]
    fn the_proposal_is_the_cheapest_option() {
        let mut hill = hillside_world(60, 0);
        let body = VoxelAabb::new(BlockPos::new(-12, 52, 0), BlockPos::new(-9, 56, 3));
        ore_body(&mut hill, body);

        let all = options(&hill, body, 3);
        let chosen = propose(&hill, body, 3).unwrap();
        assert!(all.iter().all(|plan| plan.cost() >= chosen.cost()));
    }

    #[test]
    fn an_adit_wins_where_there_is_real_relief_to_drive_into() {
        // A body 26 blocks under a plateau, level with a valley floor 22 blocks
        // away. Driving in flat beats ramping down, and this is the whole
        // reason real adits are cut into valley walls.
        let mut hill = crate::fixture::slope(RADIUS, 60, 0, 2);
        let body = VoxelAabb::new(BlockPos::new(-12, 30, 0), BlockPos::new(-9, 34, 3));
        ore_body(&mut hill, body);

        let chosen = propose(&hill, body, 3).expect("nothing proposed");
        assert_eq!(
            chosen.method,
            MineMethod::Adit,
            "chose {} at cost {} over the adit",
            chosen.method.name(),
            chosen.cost()
        );
        // One block, from the top bench up onto the tunnel floor. Every other
        // method climbs the whole overburden.
        assert!(chosen.lift() <= 1, "an adit that climbs {} is not an adit", chosen.lift());
    }

    #[test]
    fn flat_ground_gets_a_ramp_because_there_is_nothing_to_drive_into() {
        // The counterpart to the relief case, and the reason ranking is on cost
        // rather than a hardcoded preference for tunnels: with no slope there
        // is no adit to offer at all, and the answer is a ramp or a pit.
        let mut world = flat_world(60);
        let body = VoxelAabb::new(BlockPos::new(0, 40, 0), BlockPos::new(3, 43, 3));
        ore_body(&mut world, body);

        assert!(plan(&world, body, 3, MineMethod::Adit).is_none());
        let chosen = propose(&world, body, 3).unwrap();
        assert_ne!(chosen.method, MineMethod::Adit);
    }

    #[test]
    fn a_body_breaking_the_surface_is_a_pit_job() {
        // Zero overburden means zero access for a pit, while a decline still
        // has to cut its portal. Pleasingly, that makes the game recommend an
        // open pit for exactly the thing you find by eye: an outcrop.
        let mut world = flat_world(60);
        let body = VoxelAabb::new(BlockPos::new(0, 58, 0), BlockPos::new(4, 60, 4));
        ore_body(&mut world, body);

        let chosen = propose(&world, body, 3).unwrap();
        assert_eq!(
            chosen.method,
            MineMethod::Pit,
            "chose {} for a body at the surface",
            chosen.method.name()
        );
        assert!(chosen.access.is_empty(), "a surface body needs no access cut");
    }

    #[test]
    fn cost_charges_for_the_climb_but_volume_reports_only_the_digging() {
        // Two numbers on purpose: what you dig is a fact and gets shown, what
        // it is worth is a judgement and only ranks methods.
        let mut world = flat_world(60);
        let body = VoxelAabb::new(BlockPos::new(0, 40, 0), BlockPos::new(3, 43, 3));
        ore_body(&mut world, body);

        let decline = plan(&world, body, 3, MineMethod::Decline).unwrap();
        assert!(decline.lift() > 0);
        assert!(
            decline.cost() > decline.volume,
            "a plan that climbs is not being charged for it"
        );
    }

    #[test]
    fn the_volume_estimate_matches_the_regions_it_describes() {
        // The number shown when choosing between methods. If it drifts from
        // what actually gets dug, the choice is being made on a lie.
        let mut world = flat_world(60);
        let body = VoxelAabb::new(BlockPos::new(0, 40, 0), BlockPos::new(3, 43, 3));
        ore_body(&mut world, body);

        for plan in options(&world, body, 3) {
            let summed: u64 = plan.regions().map(|(_, region)| region.volume()).sum();
            assert_eq!(
                plan.volume,
                summed,
                "{}: estimate {} against {summed} of actual regions",
                plan.method.name(),
                plan.volume
            );
        }
    }

    #[test]
    fn access_regions_are_ordered_from_the_portal_inward() {
        // Cutting them out of order means digging a bench the drone has to
        // climb down to reach, which it cannot.
        let mut world = flat_world(60);
        let body = VoxelAabb::new(BlockPos::new(0, 40, 0), BlockPos::new(3, 43, 3));
        ore_body(&mut world, body);

        for method in [MineMethod::Decline, MineMethod::Pit] {
            let Some(plan) = plan(&world, body, 3, method) else {
                continue;
            };
            let floors: Vec<i32> = plan.access.iter().map(|region| region.min.y).collect();
            assert!(
                floors.windows(2).all(|pair| pair[0] >= pair[1]),
                "{}: access does not descend: {floors:?}",
                method.name()
            );
        }
    }

    #[test]
    fn planning_is_deterministic() {
        let mut world = flat_world(60);
        let body = VoxelAabb::new(BlockPos::new(0, 40, 0), BlockPos::new(3, 43, 3));
        ore_body(&mut world, body);
        assert_eq!(options(&world, body, 3), options(&world, body, 3));
    }
}
