//! Driving a swarm: the tick that turns a mine plan into a hole in the ground.
//!
//! # The loop
//!
//! Each drone, each tick, in order: fall if it is standing on nothing, run its
//! load home if it is full, take a job if it has none, cut something if
//! anything in its job is within reach, otherwise drive toward the work.
//!
//! # Reaching first, pathfinding second
//!
//! Checking the twenty-five cells a drone can reach is free; building a flow
//! field is not. Ore bodies are contiguous, so once a drone is at the face
//! almost every tick finds its next block right there and no field is built at
//! all. Fields get rebuilt when the drone runs out of face — which is exactly
//! when the route has changed anyway, because the drone changed it.

use vx_core::{BlockPos, EventBus};
use vx_world::{break_block, World};

use crate::aabb::VoxelAabb;
use crate::drone::{CachedRoute, Drone, DroneState, RouteKey};
use crate::flow::{self, FlowField};
use crate::job::{DroneId, Job, JobBoard, JobKind};
use crate::mine::MinePlan;
use crate::stockpile::Stockpile;

/// Cells a single flow field may cover before the operation gives up on it.
///
/// A field is a breadth-first sweep of every cell in its bounds, so an
/// unbounded one is not slow, it is a hang. Hitting this means a drone was
/// asked to work somewhere absurdly far from where it stands, and reporting
/// that as [`DroneState::Stuck`] is far kinder than freezing.
const MAX_FIELD_CELLS: u64 = 2_000_000;

/// How close a drone has to be to the stockpile to unload into it.
const DROP_OFF_RANGE: i32 = 2;

/// What one tick achieved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TickReport {
    /// Blocks removed from the world this tick.
    pub dug: u64,
    /// Drones that moved.
    pub moved: u32,
    /// Jobs retired this tick.
    pub completed: u32,
    /// Blocks unloaded into the stockpile.
    pub delivered: u64,
}

impl TickReport {
    /// Did anything at all happen? A run of ticks where nothing does means the
    /// operation has stalled rather than finished.
    pub fn is_idle(&self) -> bool {
        *self == TickReport::default()
    }
}

/// How a [`Operation::run`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// The board emptied and every drone unloaded.
    Finished,
    /// Nothing changed for long enough to call it stuck.
    Stalled,
    /// Ran out of ticks with work still outstanding.
    OutOfTicks,
}

/// A mining operation: the work, the drones doing it, and the pile it feeds.
#[derive(Debug)]
pub struct Operation {
    pub board: JobBoard,
    pub stockpile: Stockpile,
    /// Where hauled blocks are dropped off.
    pub home: BlockPos,
    pub drones: Vec<Drone>,
    /// Flow fields built since the operation started. Purely diagnostic: the
    /// route cache exists so a travel leg costs one build, and this is the
    /// number a test can watch to keep that true.
    pub fields_built: u64,
}

impl Operation {
    pub fn new(home: BlockPos) -> Self {
        Operation {
            board: JobBoard::new(),
            stockpile: Stockpile::new(),
            home,
            drones: Vec::new(),
            fields_built: 0,
        }
    }

    /// Add a drone, starting it at `position`.
    pub fn add_drone(&mut self, position: BlockPos) -> DroneId {
        let id = DroneId(self.drones.len() as u32);
        self.drones.push(Drone::new(id, position));
        id
    }

    /// Turn a mine plan into jobs.
    ///
    /// Access first and in order — the outermost cut carries the highest
    /// priority, so the route down is opened from the top and the drone is
    /// never asked to cut a bench it would have to fall into. Extraction sits
    /// below all of it, because ore dug before there is a way out is ore that
    /// stays where it is.
    pub fn post_plan(&mut self, plan: &MinePlan) {
        let access_count = plan.access.len() as i32;
        for (order, region) in plan.access.iter().enumerate() {
            self.board
                .post(JobKind::Access, *region, 1_000 + access_count - order as i32);
        }
        // Extraction layers are ordered top-down too, and for the same reason:
        // the bench under the drone has to still be there when it cuts the one
        // above.
        let layers = plan.extraction.len() as i32;
        for (order, region) in plan.extraction.iter().enumerate() {
            self.board.post(JobKind::Extract, *region, layers - order as i32);
        }
    }

    /// Advance every drone by one tick.
    pub fn tick(&mut self, world: &mut World, events: &EventBus) -> TickReport {
        let mut report = TickReport::default();
        for index in 0..self.drones.len() {
            self.tick_drone(index, world, events, &mut report);
        }
        report
    }

    /// Tick until the work is done, it stalls, or `max_ticks` runs out.
    ///
    /// Returns the outcome and how many ticks it took.
    pub fn run(
        &mut self,
        world: &mut World,
        events: &EventBus,
        max_ticks: u64,
    ) -> (RunOutcome, u64) {
        // Enough consecutive quiet ticks to be sure it is not just a drone
        // walking a long corridor with nothing to report.
        const PATIENCE: u32 = 64;
        let mut quiet = 0;

        for tick in 1..=max_ticks {
            let report = self.tick(world, events);

            if self.board.is_empty() && self.drones.iter().all(|drone| drone.carrying() == 0) {
                return (RunOutcome::Finished, tick);
            }

            if report.is_idle() {
                quiet += 1;
                if quiet >= PATIENCE {
                    return (RunOutcome::Stalled, tick);
                }
            } else {
                quiet = 0;
            }
        }
        (RunOutcome::OutOfTicks, max_ticks)
    }

    fn tick_drone(
        &mut self,
        index: usize,
        world: &mut World,
        events: &EventBus,
        report: &mut TickReport,
    ) {
        // Gravity first: a drone that cut the ground out from under itself last
        // tick is falling, and everything below assumes it is standing.
        let settled = flow::settle(world, self.drones[index].position);
        if settled != self.drones[index].position {
            self.drones[index].move_to(settled);
            report.moved += 1;
        }

        // A full drone runs its load home — unless it cannot get out, in which
        // case it keeps cutting.
        //
        // That fallback is what makes the whole thing live. A drone working the
        // middle of a layer can fill up before it has cut its way back to the
        // bench, and stopping there would be a deadlock: it will not dig
        // because it is full, and it cannot leave because it has not dug. A
        // loaded machine that keeps cutting until it can get out is both the
        // obvious real answer and the one that always terminates, since digging
        // only ever adds routes. The cost is that a drone may come home over
        // its rated load, which is a far better failure than a frozen one.
        if self.drones[index].is_full() && self.haul(index, world, report) {
            return;
        }

        let Some(job) = self.current_job(index) else {
            // Nothing left to do. A part-load still has to come home, or the
            // last few blocks of every body would sit in a parked drone.
            if self.drones[index].carrying() > 0 && self.haul(index, world, report) {
                return;
            }
            self.drones[index].state = DroneState::Idle;
            return;
        };

        // Cheap path: something to cut without moving.
        let region = job.region;
        if let Some(target) = self.drones[index].next_cut(world, &region) {
            match break_block(world, events, target) {
                Ok(removed) => {
                    let registry = world.registry();
                    if !self.drones[index].cargo.add_block(registry, removed, 1) {
                        log::warn!("drone dug an unregistered block at {target:?}");
                    }
                    self.drones[index].state = DroneState::Digging(job.id);
                    report.dug += 1;
                    // Take the trunk, take the tree: a felled tree's crown
                    // cannot be pruned from mid-air, so cutting any part of
                    // one brings the connected whole down into the cargo bed.
                    if crate::mine::is_vegetation_id(world.registry(), removed) {
                        report.dug += self.fell_tree(index, world, events, target);
                    }
                }
                Err(error) => {
                    // A veto — `next_cut` already refuses bedrock, so a mod
                    // said no. Remember the position so the drone moves on
                    // instead of chewing the same block until the end of time,
                    // which is exactly what the previous version did while its
                    // comment claimed otherwise.
                    log::debug!("drone denied at {target:?}: {error}");
                    self.drones[index].denied.insert(target);
                    self.drones[index].state = DroneState::Digging(job.id);
                }
            }
            return;
        }

        // Cached route still valid? While the world is unedited, the region's
        // remaining work cannot have changed either, so both the scan and the
        // field build below are skippable and travelling costs one step.
        if let Some(step) = self.cached_step(index, RouteKey::Work(job.id), world) {
            match step {
                Some(next) => {
                    self.drones[index].move_to(next);
                    self.drones[index].state = DroneState::Travelling(job.id);
                    report.moved += 1;
                }
                None => {
                    self.drones[index].route = None;
                    self.give_up(index, job.id);
                }
            }
            return;
        }

        // Slow path: is the job even still outstanding? This is the only place
        // the region is scanned, which is why the fast path above is worth
        // having.
        //
        // "Outstanding" counts *breakable* blocks. Bedrock does not count as
        // work, so a region overlapping the world floor completes once all the
        // rock that can come out has come out. Vegetation does not count
        // either: a crown hanging into a bench from a tree rooted elsewhere
        // has no cell to stand and prune it from, and it falls with its trunk
        // when any drone finally cuts one — it must never wedge a job open.
        let remaining: Vec<BlockPos> = region
            .clamped_to_world()
            .blocks()
            .filter(|pos| {
                crate::drone::is_breakable(world, *pos)
                    && !crate::mine::is_vegetation(world, *pos)
            })
            .collect();

        if remaining.is_empty() {
            self.board.complete(job.id);
            self.drones[index].job = None;
            self.drones[index].state = DroneState::Idle;
            report.completed += 1;
            return;
        }

        // Breakable work remains, but everything left may have been vetoed for
        // this drone. Release rather than complete — the blocks genuinely
        // remain, and claiming otherwise would lie — and the operation stalls
        // *visibly* with the job outstanding, which is the correct outcome for
        // "a mod forbids this dig".
        let workable: Vec<BlockPos> = remaining
            .iter()
            .copied()
            .filter(|pos| !self.drones[index].denied.contains(pos))
            .collect();
        if workable.is_empty() {
            self.give_up(index, job.id);
            return;
        }

        self.travel_to_work(index, world, &region, &workable, job.id, report);
    }

    /// The drone's claimed job, taking a new one if it has none.
    fn current_job(&mut self, index: usize) -> Option<Job> {
        let held = self.drones[index].job;
        if let Some(id) = held {
            if let Some(job) = self.board.get(id) {
                return Some(job.clone());
            }
            // Completed by someone else while this drone held the id.
            self.drones[index].job = None;
        }

        let from = self.drones[index].position;
        let id = self.drones[index].id;
        let job = self.board.claim_nearest(id, from)?;
        self.drones[index].job = Some(job.id);
        // Denials are per job, and survive re-claiming the *same* job so a
        // veto stays learned; a different job starts clean.
        if self.drones[index].denied_job != Some(job.id) {
            self.drones[index].denied.clear();
            self.drones[index].denied_job = Some(job.id);
        }
        Some(job)
    }

    /// Step one cell toward somewhere the job can be worked from.
    fn travel_to_work(
        &mut self,
        index: usize,
        world: &World,
        region: &VoxelAabb,
        remaining: &[BlockPos],
        job: crate::job::JobId,
        report: &mut TickReport,
    ) {
        // Anywhere a drone could stand and cut a block that is ready to be cut.
        // Built from the remaining blocks rather than from the region, so a
        // half-dug region does not send drones to its empty end — and built
        // with `stations_for`, which is the exact inverse of the reach rule, so
        // a drone that arrives can always actually work.
        //
        let position = self.drones[index].position;
        let goals: Vec<BlockPos> = remaining
            .iter()
            .flat_map(|pos| crate::drone::stations_for(world, *pos))
            .filter(|pos| flow::is_standable(world, *pos))
            // Never route a drone to where it already is. Standing on a station
            // it cannot work from is normal — the block under its feet may be
            // waiting on something still left above — and treating that as
            // "arrived" would strand it on the spot instead of sending it to
            // one of the other faces.
            .filter(|pos| *pos != position)
            .collect();
        let bounds = region
            .union(VoxelAabb::single(position))
            .expanded(8)
            .clamped_to_world();

        if goals.is_empty() || bounds.volume() > MAX_FIELD_CELLS {
            self.give_up(index, job);
            return;
        }

        let field = FlowField::build(world, bounds, goals);
        self.fields_built += 1;
        match field.step_from(world, position) {
            Some(next) => {
                self.drones[index].move_to(next);
                self.drones[index].state = DroneState::Travelling(job);
                self.drones[index].route = Some(CachedRoute {
                    key: RouteKey::Work(job),
                    edits: world.edit_count(),
                    field,
                });
                report.moved += 1;
            }
            None => self.give_up(index, job),
        }
    }

    /// One step from the drone's cached field, if the cache is still valid for
    /// `key`. `Some(None)` means "valid field, but no route from here" — which
    /// is an answer, not a miss.
    fn cached_step(
        &self,
        index: usize,
        key: RouteKey,
        world: &World,
    ) -> Option<Option<BlockPos>> {
        let cached = self.drones[index].route.as_ref()?;
        (cached.key == key && cached.edits == world.edit_count())
            .then(|| cached.field.step_from(world, self.drones[index].position))
    }

    /// Fell the vegetation connected to a just-cut block: flood out over
    /// touching vegetation cells, breaking each through the same cancellable
    /// event path as any other edit (a veto stops the spread through that
    /// cell), and load the lot. Bounded, so a modded megaflora cannot hold a
    /// tick hostage.
    fn fell_tree(
        &mut self,
        index: usize,
        world: &mut World,
        events: &EventBus,
        seed: BlockPos,
    ) -> u64 {
        const FELL_LIMIT: usize = 512;
        const SIDES: [[i32; 3]; 6] = [
            [1, 0, 0],
            [-1, 0, 0],
            [0, 1, 0],
            [0, -1, 0],
            [0, 0, 1],
            [0, 0, -1],
        ];

        let mut queue = vec![seed];
        let mut seen = std::collections::HashSet::from([seed]);
        let mut felled = 0u64;
        while let Some(at) = queue.pop() {
            for side in SIDES {
                let next = at.offset(side);
                if seen.len() >= FELL_LIMIT || !seen.insert(next) {
                    continue;
                }
                if !crate::mine::is_vegetation(world, next) {
                    continue;
                }
                let Ok(removed) = break_block(world, events, next) else {
                    continue;
                };
                if !self.drones[index].cargo.add_block(world.registry(), removed, 1) {
                    log::warn!("drone felled an unregistered block at {next:?}");
                }
                felled += 1;
                queue.push(next);
            }
        }
        felled
    }

    /// Hand the job back and mark the drone stuck.
    ///
    /// Releasing rather than completing matters: the work still needs doing,
    /// and a drone with a better route — or a later one, once more access is
    /// cut — should be able to pick it up.
    fn give_up(&mut self, index: usize, job: crate::job::JobId) {
        self.drones[index].route = None;
        self.board.release(job);
        self.drones[index].job = None;
        self.drones[index].state = DroneState::Stuck;
    }

    /// Move toward the stockpile, unloading on arrival.
    ///
    /// Returns whether the drone is dealing with its load. `false` means there
    /// is no route home at all, and the caller puts it back to work rather than
    /// letting it stand there full.
    fn haul(&mut self, index: usize, world: &World, report: &mut TickReport) -> bool {
        let position = self.drones[index].position;

        // The drop-off is where `home` *settles to today*, and the proximity
        // check and the navigation target must both use it. Checking closeness
        // against the unsettled `home` while walking toward the settled one
        // livelocked a drone the day the home column itself got dug away: it
        // stood exactly on the settled spot, forever four blocks from a point
        // in mid-air.
        let drop_off = flow::settle(world, self.home);
        let close = (position.x - drop_off.x).abs() <= DROP_OFF_RANGE
            && (position.y - drop_off.y).abs() <= DROP_OFF_RANGE
            && (position.z - drop_off.z).abs() <= DROP_OFF_RANGE;

        if close {
            let delivered = self.drones[index].cargo.total();
            let entries: Vec<(String, u64)> = self.drones[index]
                .cargo
                .entries()
                .map(|(name, count)| (name.to_string(), count))
                .collect();
            for (name, count) in entries {
                self.stockpile.add(name, count);
            }
            self.drones[index].cargo = Stockpile::new();
            self.drones[index].state = DroneState::Idle;
            report.delivered += delivered;
            return true;
        }

        // The homeward leg caches its field exactly like the outward one.
        if let Some(step) = self.cached_step(index, RouteKey::Home, world) {
            return match step {
                Some(next) => {
                    self.drones[index].move_to(next);
                    self.drones[index].state = DroneState::Hauling;
                    report.moved += 1;
                    true
                }
                None => {
                    self.drones[index].route = None;
                    false
                }
            };
        }

        let bounds = VoxelAabb::new(position, self.home)
            .expanded(12)
            .clamped_to_world();
        if bounds.volume() > MAX_FIELD_CELLS {
            return false;
        }

        let field = FlowField::build(world, bounds, [drop_off]);
        self.fields_built += 1;
        match field.step_from(world, position) {
            Some(next) => {
                self.drones[index].move_to(next);
                self.drones[index].state = DroneState::Hauling;
                self.drones[index].route = Some(CachedRoute {
                    key: RouteKey::Home,
                    edits: world.edit_count(),
                    field,
                });
                report.moved += 1;
                true
            }
            None => false,
        }
    }

    /// Blocks held by the operation and by every drone still carrying.
    ///
    /// The conservation figure: it must equal the blocks actually removed from
    /// the world, or work is being double-counted or dropped somewhere.
    pub fn accounted_blocks(&self) -> u64 {
        self.stockpile.total() + self.drones.iter().map(Drone::carrying).sum::<u64>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{ore_body, solid_count as solid_blocks};
    use crate::mine::{self, MineMethod};
    use vx_core::Cancellable;

    /// Wide enough that a decline's run-up lands inside the loaded world.
    const RADIUS: i32 = 8;

    fn flat_world(floor: i32) -> World {
        crate::fixture::flat(RADIUS, floor)
    }

    /// Count every solid block across a generous box around a work site.
    fn solid_total(world: &World, around: VoxelAabb) -> u64 {
        solid_blocks(world, around.expanded(30).clamped_to_world())
    }

    struct Site {
        world: World,
        operation: Operation,
        plan: MinePlan,
    }

    /// A named site: a world to build and the body buried in it.
    type Case = (MineMethod, fn() -> World, VoxelAabb);

    /// A world, a plan and a drone at the portal, ready to dig.
    fn site(floor: i32, body: VoxelAabb, method: MineMethod) -> Site {
        site_in(flat_world(floor), body, method)
    }

    fn site_in(mut world: World, body: VoxelAabb, method: MineMethod) -> Site {
        ore_body(&mut world, body);

        let plan = mine::plan(&world, body, 3, method)
            .unwrap_or_else(|| panic!("no {} plan for this body", method.name()));

        let start = flow::settle(&world, plan.portal);
        let mut operation = Operation::new(start);
        operation.add_drone(start);
        operation.post_plan(&plan);

        Site {
            world,
            operation,
            plan,
        }
    }

    #[test]
    fn posting_a_plan_puts_access_ahead_of_extraction() {
        let site = site(60, VoxelAabb::new(BlockPos::new(0, 40, 0), BlockPos::new(2, 42, 2)), MineMethod::Decline);
        let access: Vec<i32> = site
            .operation
            .board
            .jobs()
            .filter(|job| job.kind == JobKind::Access)
            .map(|job| job.priority)
            .collect();
        let extract: Vec<i32> = site
            .operation
            .board
            .jobs()
            .filter(|job| job.kind == JobKind::Extract)
            .map(|job| job.priority)
            .collect();

        assert!(!access.is_empty() && !extract.is_empty());
        assert!(
            access.iter().min() > extract.iter().max(),
            "extraction outranks access somewhere: {access:?} against {extract:?}"
        );
    }

    #[test]
    fn the_outermost_access_cut_is_claimed_first() {
        // Cutting a ramp from the bottom up is not a thing a drone can do.
        let mut site = site(60, VoxelAabb::new(BlockPos::new(0, 40, 0), BlockPos::new(2, 42, 2)), MineMethod::Decline);
        let first = site.plan.access[0];
        let claimed = site
            .operation
            .board
            .claim_nearest(DroneId(0), site.plan.portal)
            .unwrap();
        assert_eq!(claimed.region, first);
    }

    #[test]
    fn a_drone_clears_a_marked_body_and_the_haul_is_tallied() {
        // The milestone, end to end: mark a body, the game plans the mine, a
        // drone cuts its way in, digs it out, and the pile matches.
        let body = VoxelAabb::new(BlockPos::new(0, 52, 0), BlockPos::new(3, 55, 3));
        let mut site = site(60, body, MineMethod::Decline);
        let events = EventBus::new();

        let before = solid_total(&site.world, site.plan.access.iter().fold(body, |a, b| a.union(*b)));
        let (outcome, ticks) = site.operation.run(&mut site.world, &events, 200_000);

        assert_eq!(
            outcome,
            RunOutcome::Finished,
            "the operation did not finish after {ticks} ticks \
             ({} jobs left, drone {:?})",
            site.operation.board.len(),
            site.operation.drones[0].state
        );

        assert_eq!(
            solid_blocks(&site.world, body),
            0,
            "the body still has ore in it"
        );

        let after = solid_total(&site.world, site.plan.access.iter().fold(body, |a, b| a.union(*b)));
        let removed = before - after;
        assert_eq!(
            site.operation.accounted_blocks(),
            removed,
            "the pile holds {} but {removed} blocks left the world: jobs are being \
             double-counted or dropped",
            site.operation.accounted_blocks()
        );
        assert!(
            site.operation.stockpile.count("engine:copper_ore") >= body.volume(),
            "only {} ore reached the pile from a {}-block body",
            site.operation.stockpile.count("engine:copper_ore"),
            body.volume()
        );
        eprintln!("decline: {ticks} ticks, {removed} blocks moved");
    }

    #[test]
    fn every_method_gets_the_ore_out() {
        // The reachability invariant, proved the hard way: not "a flow field
        // says it could", but "a drone actually did". All three excavation
        // shapes, one drone, ore in the pile at the end or the test fails.
        let cases: [Case; 3] = [
            (
                MineMethod::Pit,
                || flat_world(60),
                VoxelAabb::new(BlockPos::new(0, 54, 0), BlockPos::new(2, 56, 2)),
            ),
            (
                MineMethod::Decline,
                || flat_world(60),
                VoxelAabb::new(BlockPos::new(0, 44, 0), BlockPos::new(2, 46, 2)),
            ),
            (
                MineMethod::Adit,
                || crate::fixture::slope(RADIUS, 60, 0, 2),
                VoxelAabb::new(BlockPos::new(-12, 30, 0), BlockPos::new(-10, 33, 2)),
            ),
        ];

        for (method, build, body) in cases {
            let mut site = site_in(build(), body, method);
            let events = EventBus::new();
            let (outcome, ticks) = site.operation.run(&mut site.world, &events, 200_000);

            assert_eq!(
                outcome,
                RunOutcome::Finished,
                "{}: stopped after {ticks} ticks with the drone {:?}",
                method.name(),
                site.operation.drones[0].state
            );
            assert_eq!(
                solid_blocks(&site.world, body),
                0,
                "{}: ore left in the ground",
                method.name()
            );
            eprintln!("{}: {ticks} ticks", method.name());
        }
    }

    #[test]
    fn the_drone_comes_home_rather_than_stranding_itself() {
        // The reason climb and drop share a limit. A drone that dug its way
        // down and could not get back would show up here as an unfinished run
        // with cargo still aboard.
        let body = VoxelAabb::new(BlockPos::new(0, 40, 0), BlockPos::new(3, 44, 3));
        let mut site = site(60, body, MineMethod::Decline);
        site.operation.drones[0].capacity = 12; // force several round trips
        let events = EventBus::new();

        let (outcome, _) = site.operation.run(&mut site.world, &events, 400_000);
        assert_eq!(outcome, RunOutcome::Finished);
        assert_eq!(site.operation.drones[0].carrying(), 0, "cargo never got home");
        assert!(site.operation.stockpile.total() > 50);
    }

    #[test]
    fn waste_rock_and_ore_stay_separate_in_the_pile() {
        // Opening a mine moves a lot of nothing. Keeping the two apart is what
        // will let a later readout say what the operation actually cost.
        let body = VoxelAabb::new(BlockPos::new(0, 44, 0), BlockPos::new(2, 46, 2));
        let mut site = site(60, body, MineMethod::Decline);
        let events = EventBus::new();
        site.operation.run(&mut site.world, &events, 200_000);

        let ore = site.operation.stockpile.count("engine:copper_ore");
        let stone = site.operation.stockpile.count("engine:stone");
        assert!(ore > 0, "no ore in the pile");
        assert!(stone > 0, "no waste rock in the pile; the ramp dug itself?");
        assert!(
            stone > ore,
            "a ramp {} blocks down should move more waste ({stone}) than ore ({ore})",
            60 - body.max.y
        );
    }

    #[test]
    fn a_mod_can_veto_a_drone_exactly_as_it_vetoes_a_player() {
        // Drone digging goes through the same `break_block` the player uses, so
        // the cancellable event already covers it with no new plumbing. Worth
        // pinning: it is the whole reason digging was not given its own path.
        let body = VoxelAabb::new(BlockPos::new(0, 54, 0), BlockPos::new(2, 56, 2));
        let mut site = site(60, body, MineMethod::Pit);

        let mut events = EventBus::new();
        events.subscribe("guard", |event: &mut vx_world::BlockBreakEvent| {
            event.cancel();
        });

        let before = solid_blocks(&site.world, body);
        site.operation.run(&mut site.world, &events, 2_000);

        assert_eq!(
            solid_blocks(&site.world, body),
            before,
            "the veto was ignored and the drone dug anyway"
        );
        assert_eq!(site.operation.accounted_blocks(), 0);
    }

    #[test]
    fn digging_is_deterministic() {
        // Same seed, same plan, same tick count, same hole. Without this the
        // conservation checks above could pass by luck.
        let body = VoxelAabb::new(BlockPos::new(0, 50, 0), BlockPos::new(2, 52, 2));
        let outcome: Vec<(u64, u64, BlockPos)> = (0..2)
            .map(|_| {
                let mut site = site(60, body, MineMethod::Decline);
                let events = EventBus::new();
                for _ in 0..500 {
                    site.operation.tick(&mut site.world, &events);
                }
                (
                    site.operation.stockpile.total(),
                    site.operation.accounted_blocks(),
                    site.operation.drones[0].position,
                )
            })
            .collect();

        assert_eq!(outcome[0], outcome[1], "two identical runs diverged");
    }

    #[test]
    fn a_drone_with_nothing_to_do_goes_idle_rather_than_spinning() {
        let mut world = flat_world(60);
        let events = EventBus::new();
        let mut operation = Operation::new(BlockPos::new(0, 61, 0));
        operation.add_drone(BlockPos::new(0, 61, 0));

        let report = operation.tick(&mut world, &events);
        assert!(report.is_idle());
        assert_eq!(operation.drones[0].state, DroneState::Idle);
    }

    #[test]
    fn work_it_cannot_reach_is_given_back_to_the_board() {
        // A released job is one another drone — or the same one, after more
        // access is cut — can still take. Completing it would lose the work.
        let mut world = flat_world(60);
        let events = EventBus::new();
        let mut operation = Operation::new(BlockPos::new(0, 61, 0));
        operation.add_drone(BlockPos::new(0, 61, 0));

        // A region of solid rock sealed under the surface, far from any
        // excavation, with no route to it.
        let sealed = VoxelAabb::new(BlockPos::new(20, 30, 20), BlockPos::new(22, 32, 22));
        let id = operation.board.post(JobKind::Extract, sealed, 0);

        operation.tick(&mut world, &events);
        assert_eq!(operation.drones[0].state, DroneState::Stuck);
        assert!(operation.board.get(id).is_some(), "the job was thrown away");
        assert!(
            operation.board.claimant(id).is_none(),
            "the job is still held by a drone that cannot do it"
        );
    }

    #[test]
    fn a_long_approach_builds_one_field_not_one_per_step() {
        // Finding A6. Travelling changes nothing about the world, so a whole
        // leg should reuse one cached field; rebuilding a full BFS every step
        // was the single biggest cost in the loop. The counter is the guard
        // that keeps the cache honest.
        let mut world = flat_world(60);
        let region = VoxelAabb::new(BlockPos::new(30, 60, 0), BlockPos::new(32, 60, 2));
        let start = BlockPos::new(-30, 61, 1);

        let mut operation = Operation::new(start);
        operation.add_drone(start);
        operation.board.post(JobKind::Extract, region, 0);
        let events = EventBus::new();

        // Walk the ~60-block approach until the first dig happens.
        let mut ticks = 0;
        while operation.stockpile.total() + operation.drones[0].carrying() == 0 {
            operation.tick(&mut world, &events);
            ticks += 1;
            assert!(ticks < 2_000, "never reached the work");
        }

        assert!(ticks > 40, "the approach was too short to prove anything");
        assert!(
            operation.fields_built <= 3,
            "{} fields built over a {ticks}-tick approach; the route cache is not caching",
            operation.fields_built
        );
    }

    #[test]
    fn a_region_floored_with_bedrock_still_completes() {
        // Finding A1, half one: unbreakable blocks are not work. A job whose
        // region contains bedrock must finish once everything that *can* come
        // out has, rather than chewing the unbreakable block forever.
        //
        // A single surface layer, deliberately: a plain posted box with depth
        // strands the drone in its own straight-sided hole, which is exactly
        // why real excavations come from benched mine plans.
        let mut world = flat_world(60);
        let bedrock = world.registry().id_of("engine:bedrock").unwrap();
        let region = VoxelAabb::new(BlockPos::new(0, 60, 0), BlockPos::new(2, 60, 2));
        let unbreakable = BlockPos::new(1, 60, 1);
        world.set_block(unbreakable, bedrock);

        let start = BlockPos::new(-2, 61, 1);
        let mut operation = Operation::new(start);
        operation.add_drone(start);
        operation.board.post(JobKind::Extract, region, 0);

        let events = EventBus::new();
        let (outcome, ticks) = operation.run(&mut world, &events, 20_000);

        assert_eq!(
            outcome,
            RunOutcome::Finished,
            "stalled after {ticks} ticks with the drone {:?}",
            operation.drones[0].state
        );
        // The eight stone blocks came out; the bedrock did not, and the job
        // still counts as done.
        assert_eq!(operation.stockpile.count("engine:stone"), 8);
        assert_eq!(world.block(unbreakable), bedrock);
    }

    #[test]
    fn a_vetoed_block_is_skipped_and_the_job_is_never_falsely_completed() {
        // Finding A1, half two: a mod veto is dynamic, so the drone learns the
        // position, digs everything else, and *releases* the job rather than
        // completing it — the block genuinely remains, and the visible stall is
        // the honest outcome of "a mod forbids this dig".
        let mut world = flat_world(60);
        let forbidden = BlockPos::new(1, 60, 1);
        let region = VoxelAabb::new(BlockPos::new(0, 60, 0), BlockPos::new(2, 60, 2));

        let mut events = EventBus::new();
        events.subscribe("guard", move |event: &mut vx_world::BlockBreakEvent| {
            if event.position == forbidden {
                event.cancel();
            }
        });

        let start = BlockPos::new(-2, 61, 1);
        let mut operation = Operation::new(start);
        operation.add_drone(start);
        let id = operation.board.post(JobKind::Extract, region, 0);

        let (outcome, _) = operation.run(&mut world, &events, 20_000);

        // Not Finished — and crucially not an endless loop: the run terminates
        // as a visible stall with the job still on the board.
        assert_eq!(outcome, RunOutcome::Stalled);
        assert!(
            operation.board.get(id).is_some(),
            "the job was completed or lost while its block still stands"
        );
        assert!(world.is_solid(forbidden), "the veto was ignored");
        // Everything the mod allowed came out.
        let left: Vec<BlockPos> = region
            .blocks()
            .filter(|pos| world.is_solid(*pos))
            .collect();
        assert_eq!(left, vec![forbidden], "more than the vetoed block remains");
        assert_eq!(operation.stockpile.total() + operation.drones[0].carrying(), 8);
    }

    #[test]
    fn a_drone_never_falls_further_than_it_can_climb() {
        // The invariant that keeps a drone recoverable. Undermining itself by
        // one block is allowed and is how it descends; dropping further is how
        // it ends up at the bottom of its own hole with no way back, and the
        // first anyone would know is a haul that never arrives.
        let body = VoxelAabb::new(BlockPos::new(0, 50, 0), BlockPos::new(3, 54, 3));
        let mut site = site(60, body, MineMethod::Decline);
        let events = EventBus::new();

        for _ in 0..5_000 {
            let before = site.operation.drones[0].position;
            site.operation.tick(&mut site.world, &events);
            let after = site.operation.drones[0].position;

            assert!(
                before.y - after.y <= flow::STEP,
                "the drone fell {} blocks in one tick, from {before:?} to {after:?}",
                before.y - after.y
            );
        }
    }
}
