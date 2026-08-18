//! The air side of the operation: scanning sectors and ferrying ore home.
//!
//! Mirrors [`crate::operation`]: a coordinator owning the aircraft and the
//! knowledge they produce, ticked once per simulation step. The division of
//! labour is deliberate — [`crate::flier::Flier`] knows how to fly, and the
//! fleet decides where to; drones stay dumb so the swarm stays cheap.
//!
//! # Scanning is progressive
//!
//! A sweep takes real time, and pings exist only for ground already overflown:
//! interrupt a scan halfway and you know half the sector. The covered set is
//! reclustered as the sweep advances, so two halves of one body found on
//! neighbouring passes merge into a single ping rather than lingering as two.
//!
//! # The chain conserves blocks
//!
//! Mine stockpiles, flier cargo and the base pile are the same blocks moving
//! through stations. While nothing digs, their total is constant — the same
//! conservation discipline the ground operation is held to, extended across
//! the whole chain, and the test that catches a duplicated or dropped load.

use std::collections::{HashMap, HashSet};

use vx_core::BlockPos;
use vx_world::World;

use crate::flier::{sweep_path, swath_columns, Flier, FlierState};
use crate::operation::Operation;
use crate::prospect::{column_hit, cluster_pings, Ping, Sector};
use crate::stockpile::Stockpile;

/// The base: a container block the player placed, and what has arrived in it.
#[derive(Debug, Clone)]
pub struct Base {
    pub position: BlockPos,
    pub stockpile: Stockpile,
}

/// One partly- or fully-swept sector.
#[derive(Debug, Clone, Default)]
struct Survey {
    /// Columns the scanner has covered.
    covered: HashSet<(i32, i32)>,
    /// Covered columns with ore in range: `(depth, hover_y)` per column.
    hits: HashMap<(i32, i32), (i32, i32)>,
    complete: bool,
}

/// The fleet: fliers, the base, and everything the scanner has learned.
#[derive(Debug, Default)]
pub struct Fleet {
    pub fliers: Vec<Flier>,
    pub base: Option<Base>,
    surveys: HashMap<Sector, Survey>,
}

impl Fleet {
    pub fn new() -> Self {
        Fleet::default()
    }

    pub fn add_flier(&mut self, position: BlockPos) -> usize {
        self.fliers.push(Flier::new(position));
        self.fliers.len() - 1
    }

    /// Declare the base at a placed container block. Replaces any previous
    /// base but keeps nothing from it — the old pile lives in the old block's
    /// world position conceptually, and losing track of it on replace would
    /// be a conservation leak, so the pile transfers.
    pub fn set_base(&mut self, position: BlockPos) {
        let stockpile = self
            .base
            .take()
            .map(|base| base.stockpile)
            .unwrap_or_default();
        self.base = Some(Base {
            position,
            stockpile,
        });
    }

    /// The container was broken: no base until another is placed.
    pub fn clear_base(&mut self) -> Option<Stockpile> {
        self.base.take().map(|base| base.stockpile)
    }

    /// Send an idle flier to sweep `sector`. Returns whether one was free.
    ///
    /// Re-dispatching a finished sector rescans it — that is a feature, not a
    /// waste: the world changes, and a stale survey lies.
    pub fn dispatch_scan(&mut self, sector: Sector) -> bool {
        let Some(index) = self
            .fliers
            .iter()
            .position(|flier| flier.state == FlierState::Idle)
        else {
            return false;
        };
        self.surveys.insert(sector, Survey::default());
        self.fliers[index].state = FlierState::Scanning { sector, waypoint: 0 };
        true
    }

    /// Every ping the fleet currently knows, across all surveys, in a stable
    /// order.
    pub fn pings(&self) -> Vec<Ping> {
        let mut sectors: Vec<&Sector> = self.surveys.keys().collect();
        sectors.sort_by_key(|sector| (sector.x, sector.z));
        sectors
            .into_iter()
            .flat_map(|sector| cluster_pings(&self.surveys[sector].hits))
            .collect()
    }

    /// Has this sector been fully swept?
    pub fn is_surveyed(&self, sector: Sector) -> bool {
        self.surveys
            .get(&sector)
            .is_some_and(|survey| survey.complete)
    }

    /// Sectors fully swept, for the map to shade as explored.
    pub fn surveyed_sectors(&self) -> impl Iterator<Item = Sector> + '_ {
        self.surveys
            .iter()
            .filter(|(_, survey)| survey.complete)
            .map(|(sector, _)| *sector)
    }

    /// Advance every flier one tick.
    pub fn tick(&mut self, world: &World, mines: &mut [Operation]) {
        for index in 0..self.fliers.len() {
            self.tick_flier(index, world, mines);
        }
    }

    fn tick_flier(&mut self, index: usize, world: &World, mines: &mut [Operation]) {
        match self.fliers[index].state {
            FlierState::Idle => self.consider_ferrying(index, mines),
            FlierState::Scanning { sector, waypoint } => {
                self.advance_scan(index, world, sector, waypoint)
            }
            FlierState::ToPickup { mine } => {
                // The mine may have been dismantled between dispatch and
                // arrival; go home rather than orbiting a ghost.
                let Some(operation) = mines.get_mut(mine) else {
                    self.fliers[index].state = FlierState::Idle;
                    return;
                };
                let target = (operation.home.x, operation.home.z);
                if self.fliers[index].fly_towards(world, target) {
                    Self::transfer(&mut operation.stockpile, index, &mut self.fliers[..]);
                    self.fliers[index].state = FlierState::ToBase;
                }
            }
            FlierState::ToBase => {
                let Some(base) = &mut self.base else {
                    // Base broken mid-flight: hold the cargo and wait. The
                    // blocks stay aboard, so conservation holds.
                    self.fliers[index].state = FlierState::Idle;
                    return;
                };
                let target = (base.position.x, base.position.z);
                if self.fliers[index].fly_towards(world, target) {
                    let cargo = std::mem::take(&mut self.fliers[index].cargo);
                    for (name, count) in cargo.entries() {
                        base.stockpile.add(name.to_string(), count);
                    }
                    self.fliers[index].state = FlierState::Idle;
                }
            }
        }
    }

    /// Idle, with a base and ore waiting somewhere: go get it.
    fn consider_ferrying(&mut self, index: usize, mines: &mut [Operation]) {
        if self.base.is_none() {
            return;
        }
        let here = self.fliers[index].position;
        let nearest = mines
            .iter()
            .enumerate()
            .filter(|(_, operation)| !operation.stockpile.is_empty())
            .min_by_key(|(_, operation)| {
                let (dx, dz) = (
                    (operation.home.x - here.x) as i64,
                    (operation.home.z - here.z) as i64,
                );
                dx * dx + dz * dz
            })
            .map(|(mine, _)| mine);

        if let Some(mine) = nearest {
            self.fliers[index].state = FlierState::ToPickup { mine };
        }
    }

    /// Load up to capacity from `pile` into flier `index`.
    fn transfer(pile: &mut Stockpile, index: usize, fliers: &mut [Flier]) {
        let flier = &mut fliers[index];
        let mut room = flier.capacity.saturating_sub(flier.carrying());
        let kinds: Vec<String> = pile.entries().map(|(name, _)| name.to_string()).collect();
        for name in kinds {
            if room == 0 {
                break;
            }
            let taken = pile.take(&name, room);
            flier.cargo.add(name, taken);
            room -= taken;
        }
    }

    /// One tick of a sweep: fly to the next waypoint, scan the swath there.
    fn advance_scan(&mut self, index: usize, world: &World, sector: Sector, waypoint: usize) {
        let path = sweep_path(sector);
        let Some(&target) = path.get(waypoint) else {
            self.surveys.entry(sector).or_default().complete = true;
            self.fliers[index].state = FlierState::Idle;
            return;
        };

        if !self.fliers[index].fly_towards(world, target) {
            return;
        }

        // Over the waypoint: the swath under the flight line is now covered.
        let survey = self.surveys.entry(sector).or_default();
        let (min_x, min_z) = sector.min_column();
        let size = crate::prospect::SECTOR_SIZE;
        for column in swath_columns(target) {
            let inside = (min_x..min_x + size).contains(&column.0)
                && (min_z..min_z + size).contains(&column.1);
            if inside && survey.covered.insert(column) {
                if let Some(hit) = column_hit(world, column.0, column.1) {
                    survey.hits.insert(column, hit);
                }
            }
        }

        self.fliers[index].state = FlierState::Scanning {
            sector,
            waypoint: waypoint + 1,
        };
    }

    /// Blocks held across the whole air side: aboard fliers plus in the base.
    ///
    /// Together with each mine's `accounted_blocks`, this is the conservation
    /// figure for the entire chain.
    pub fn accounted_blocks(&self) -> u64 {
        let aboard: u64 = self.fliers.iter().map(Flier::carrying).sum();
        aboard
            + self
                .base
                .as_ref()
                .map(|base| base.stockpile.total())
                .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aabb::VoxelAabb;
    use crate::fixture::{flat, ore_body};
    use crate::flier::CLEARANCE;

    /// A fleet with one flier hovering near the origin over flat ground.
    fn fleet_over(world: &World) -> Fleet {
        let clear = world.surface_y(0, 0).expect("origin loaded");
        let mut fleet = Fleet::new();
        fleet.add_flier(BlockPos::new(0, clear + CLEARANCE, 0));
        fleet
    }

    /// Run the fleet until the flier idles or `limit` ticks pass.
    fn run_until_idle(fleet: &mut Fleet, world: &World, mines: &mut [Operation], limit: u32) {
        for _ in 0..limit {
            fleet.tick(world, mines);
            if fleet.fliers.iter().all(|flier| flier.state == FlierState::Idle)
                && mines.iter().all(|mine| mine.stockpile.is_empty())
            {
                return;
            }
        }
    }

    #[test]
    fn a_sweep_finds_the_buried_body_and_reports_its_depth() {
        // The scanner's whole reason to exist: ore the eye cannot see.
        let mut world = flat(5, 60);
        let body = VoxelAabb::new(BlockPos::new(20, 48, 20), BlockPos::new(24, 52, 24));
        ore_body(&mut world, body);

        let mut fleet = fleet_over(&world);
        assert!(fleet.dispatch_scan(Sector { x: 0, z: 0 }));
        run_until_idle(&mut fleet, &world, &mut [], 5_000);

        assert!(fleet.is_surveyed(Sector { x: 0, z: 0 }));
        let pings = fleet.pings();
        assert_eq!(pings.len(), 1, "expected one ping, got {pings:?}");
        // Surface at 60, body top at 52: eight blocks of overburden.
        assert_eq!(pings[0].depth, 8);
        assert_eq!(pings[0].ore_columns, 25);
        assert!(body.expanded(1).contains(BlockPos::new(
            pings[0].position.x,
            body.min.y,
            pings[0].position.z
        )));
    }

    #[test]
    fn pings_exist_only_for_ground_already_overflown() {
        // Progressive scanning: interrupt a sweep halfway and you know half
        // the sector — no more.
        let mut world = flat(5, 60);
        // One body early in the sweep (low z), one late (high z).
        ore_body(&mut world, VoxelAabb::new(BlockPos::new(10, 55, 4), BlockPos::new(12, 60, 6)));
        ore_body(&mut world, VoxelAabb::new(BlockPos::new(10, 55, 58), BlockPos::new(12, 60, 60)));

        let mut fleet = fleet_over(&world);
        fleet.dispatch_scan(Sector { x: 0, z: 0 });

        // Tick until the first ping appears, then stop immediately.
        let mut ticks = 0;
        while fleet.pings().is_empty() {
            fleet.tick(&world, &mut []);
            ticks += 1;
            assert!(ticks < 5_000, "the sweep never found the first body");
        }

        let pings = fleet.pings();
        assert_eq!(pings.len(), 1, "found more than the overflown body: {pings:?}");
        assert!(pings[0].position.z < 32, "the late body pinged before being overflown");
        assert!(!fleet.is_surveyed(Sector { x: 0, z: 0 }));
    }

    #[test]
    fn a_body_deeper_than_scan_range_stays_invisible() {
        let mut world = flat(5, 60);
        let deep = VoxelAabb::new(BlockPos::new(20, 20, 20), BlockPos::new(24, 24, 24));
        ore_body(&mut world, deep);

        let mut fleet = fleet_over(&world);
        fleet.dispatch_scan(Sector { x: 0, z: 0 });
        run_until_idle(&mut fleet, &world, &mut [], 5_000);

        assert!(fleet.is_surveyed(Sector { x: 0, z: 0 }));
        assert!(fleet.pings().is_empty(), "pinged a body below SCAN_DEPTH");
    }

    #[test]
    fn a_mined_out_body_stops_pinging_on_rescan() {
        // The scanner reads the world as it is, not the deposit function —
        // this is the test that keeps that promise.
        let mut world = flat(5, 60);
        let body = VoxelAabb::new(BlockPos::new(20, 50, 20), BlockPos::new(22, 52, 22));
        ore_body(&mut world, body);

        let mut fleet = fleet_over(&world);
        fleet.dispatch_scan(Sector { x: 0, z: 0 });
        run_until_idle(&mut fleet, &world, &mut [], 5_000);
        assert_eq!(fleet.pings().len(), 1);

        // Mine it out by hand.
        let stone = world.registry().id_of("engine:stone").unwrap();
        for pos in body.blocks() {
            world.set_block(pos, stone);
        }

        fleet.dispatch_scan(Sector { x: 0, z: 0 });
        run_until_idle(&mut fleet, &world, &mut [], 5_000);
        assert!(fleet.pings().is_empty(), "a mined-out body still pings");
    }

    #[test]
    fn the_ferry_moves_every_block_and_loses_none() {
        // Conservation across the whole chain: mine pile + cargo + base pile
        // is constant while nothing digs, and the run ends with everything in
        // the base, by name.
        let world = flat(6, 60);
        let mut fleet = fleet_over(&world);
        fleet.set_base(BlockPos::new(-30, 61, -30));

        let mut mine = Operation::new(BlockPos::new(40, 61, 40));
        mine.stockpile.add("engine:copper_ore", 130);
        mine.stockpile.add("engine:stone", 70);
        let total = 200;

        let mut mines = [mine];
        for _ in 0..10_000 {
            fleet.tick(&world, &mut mines);
            let in_flight = mines[0].stockpile.total() + fleet.accounted_blocks();
            assert_eq!(in_flight, total, "blocks appeared or vanished mid-ferry");
            if mines[0].stockpile.is_empty() && fleet.fliers[0].carrying() == 0 {
                break;
            }
        }

        let base = fleet.base.as_ref().expect("base still set");
        assert_eq!(base.stockpile.count("engine:copper_ore"), 130);
        assert_eq!(base.stockpile.count("engine:stone"), 70);
        assert!(mines[0].stockpile.is_empty(), "ore left at the mine");
    }

    #[test]
    fn no_base_means_no_ferrying() {
        // Without somewhere to put it, hauling ore into the air would just be
        // carrying it around.
        let world = flat(4, 60);
        let mut fleet = fleet_over(&world);

        let mut mine = Operation::new(BlockPos::new(20, 61, 20));
        mine.stockpile.add("engine:copper_ore", 10);
        let mut mines = [mine];

        for _ in 0..50 {
            fleet.tick(&world, &mut mines);
        }
        assert_eq!(fleet.fliers[0].state, FlierState::Idle);
        assert_eq!(mines[0].stockpile.total(), 10);
    }

    #[test]
    fn a_busy_flier_cannot_be_dispatched_again() {
        let world = flat(4, 60);
        let mut fleet = fleet_over(&world);
        assert!(fleet.dispatch_scan(Sector { x: 0, z: 0 }));
        assert!(!fleet.dispatch_scan(Sector { x: 1, z: 0 }), "one flier took two jobs");
    }

    #[test]
    fn the_flier_never_enters_terrain_during_a_whole_errand() {
        // The flight-safety invariant over a real errand on rough ground:
        // scan, then ferry over a ridge between mine and base.
        let world = crate::fixture::shaped(6, |x| if (20..30).contains(&x) { 85 } else { 60 });
        let clear = world.surface_y(0, 0).unwrap();
        let mut fleet = Fleet::new();
        fleet.add_flier(BlockPos::new(0, clear + CLEARANCE, 0));
        fleet.set_base(BlockPos::new(-20, 61, 0));

        let mut mine = Operation::new(BlockPos::new(45, 61, 0));
        mine.stockpile.add("engine:stone", 40);
        let mut mines = [mine];

        for tick in 0..10_000 {
            fleet.tick(&world, &mut mines);
            assert!(
                !world.is_solid(fleet.fliers[0].position),
                "flier inside terrain at {:?} on tick {tick}",
                fleet.fliers[0].position
            );
            if mines[0].stockpile.is_empty() && fleet.fliers[0].carrying() == 0 {
                return;
            }
        }
        panic!("the ferry never completed");
    }
}
