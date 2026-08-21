//! The kestrel: a palm-sized scout that rides its owner's pack.
//!
//! # Tethered by recharge, not fuelled
//!
//! Machines cost credits and the fuel loop is coming, so a machine with no
//! operating cost needs a stated reason. The kestrel's is that it does not
//! *work* — it looks, and looking is the one job cheap enough to be free. It
//! flies on a small cell: [`ENDURANCE`] ticks aloft, then it must come home
//! and recharge, and the recharge is proportional to what the flight spent —
//! flight time and cooldown are one budget.
//!
//! # One machine, no parallel system
//!
//! Movement is [`Flier`] movement, reused whole: the same terrain-safe
//! stepping, the same climb rate, the same never-inside-terrain rule. What is
//! new is only the *standing orders* — a mode the owner sets, held until
//! changed — and the dock, which is a person rather than a pad. The kestrel
//! knows that person only as `anchor`: a position handed in every tick,
//! because this crate does not know what a player is.
//!
//! # It reveals contacts, never terrain
//!
//! The kestrel is deliberately not in any fleet roster and has no scanning
//! state: `dispatch_scan` cannot reach it, surveys cannot come from it, and
//! the survey layer stays the paid flier's trade. What the kestrel sees is
//! the caller's business — this module only flies.

use vx_core::BlockPos;
use vx_world::World;

use crate::flier::{Flier, CLIMB_RATE};

/// Ticks of flight in a full cell (45 s at the 8 Hz journal clock).
pub const ENDURANCE: u32 = 360;

/// Ticks to recharge a fully spent cell, before upgrades (90 s).
pub const COOLDOWN: u32 = 720;

/// Ticks to recharge a fully spent cell at the top upgrade mark (30 s).
pub const COOLDOWN_BEST: u32 = 240;

/// Perched sentry work drains the cell at one tick in this many.
pub const PERCH_DIVISOR: u32 = 4;

/// Blocks from the kestrel within which a contact can be scanned.
pub const SCAN_RADIUS: f32 = 24.0;

/// Radius of the overwatch circle around the anchor.
pub const ORBIT_RADIUS: i32 = 12;

/// Blocks ahead of the anchor the vanguard holds.
pub const VANGUARD_DISTANCE: i32 = 18;

/// Ticks a sortie lingers over its target before turning home.
pub const SORTIE_LINGER: u32 = 24;

/// How close to the anchor counts as home for docking.
const DOCK_REACH: i32 = 2;

/// Waypoints on the orbit circle. Eight is enough for the circle to read as
/// a circle while each leg stays a straight flier hop.
const ORBIT_POINTS: u32 = 8;

/// Ticks the orbit dwells per waypoint step, so the ring is a patrol rather
/// than a blur.
const ORBIT_DWELL: u32 = 12;

/// The kestrel's standing order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KestrelMode {
    /// On the pack, recharging or ready.
    Docked,
    /// Circling the anchor, scanning.
    Orbit,
    /// Fly to a column, linger, come home. `linger` counts down over the
    /// target.
    Sortie { x: i32, z: i32, linger: u32 },
    /// Land where it is and watch as a static sentry. The column is fixed at
    /// order time; the cell drains at a quarter rate once landed.
    Perch { x: i32, z: i32 },
    /// Hold ahead of the anchor along its heading.
    Vanguard,
    /// Under direct control; the pilot moves it, the cell still drains.
    Manual,
    /// Cell spent or recalled: coming home to dock.
    Returning,
}

/// The scout itself.
#[derive(Debug, Clone)]
pub struct Kestrel {
    /// Position and terrain-safe movement, borrowed whole from the flier.
    pub craft: Flier,
    pub mode: KestrelMode,
    /// Flight ticks left in the cell.
    pub endurance: u32,
    /// Recharge ticks left. Only ever nonzero while docked.
    pub cooldown: u32,
    /// Recharge cost of a fully spent cell — the upgrade line lowers it.
    pub recharge_cost: u32,
    /// Beats the perch drain: one endurance tick per `PERCH_DIVISOR` calls.
    perch_beat: u32,
    /// Steps taken around the orbit ring.
    orbit_step: u32,
}

impl Kestrel {
    pub fn new(anchor: BlockPos) -> Self {
        Kestrel {
            craft: Flier::new(anchor),
            mode: KestrelMode::Docked,
            endurance: ENDURANCE,
            cooldown: 0,
            recharge_cost: COOLDOWN,
            perch_beat: 0,
            orbit_step: 0,
        }
    }

    /// Whether it is in the air (including landed on a perch — off the pack).
    pub fn aloft(&self) -> bool {
        !matches!(self.mode, KestrelMode::Docked)
    }

    /// Whether a launch order would be honoured right now.
    pub fn ready(&self) -> bool {
        matches!(self.mode, KestrelMode::Docked) && self.cooldown == 0
    }

    /// Give a standing order. Launch orders while recharging are refused
    /// (`false`); a mode change while already aloft is always honoured, and
    /// `Docked` while aloft means "come home".
    pub fn order(&mut self, mode: KestrelMode) -> bool {
        match (self.aloft(), mode) {
            (false, KestrelMode::Docked) => true,
            (false, _) => {
                if self.cooldown > 0 {
                    return false;
                }
                self.mode = mode;
                self.orbit_step = 0;
                true
            }
            (true, KestrelMode::Docked) => {
                self.mode = KestrelMode::Returning;
                true
            }
            (true, other) => {
                self.mode = other;
                self.orbit_step = 0;
                true
            }
        }
    }

    /// One journal tick of flight. `anchor` is where home is this tick;
    /// `heading` is home's level facing (for the vanguard), snapped to
    /// whatever precision the caller has.
    pub fn tick(&mut self, world: &World, anchor: BlockPos, heading: (i32, i32)) {
        match self.mode {
            KestrelMode::Docked => {
                self.cooldown = self.cooldown.saturating_sub(1);
                // Riding the pack: position follows the anchor so the next
                // launch starts from wherever its owner walked to.
                self.craft.move_to(anchor);
            }
            KestrelMode::Returning => {
                self.drain(1);
                let home = (anchor.x, anchor.z);
                self.craft.fly_towards(world, home);
                let dx = self.craft.position.x - anchor.x;
                let dz = self.craft.position.z - anchor.z;
                if dx.abs() <= DOCK_REACH && dz.abs() <= DOCK_REACH {
                    self.dock(anchor);
                }
            }
            KestrelMode::Orbit => {
                if self.spend(1) {
                    return;
                }
                let angle = (self.orbit_step / ORBIT_DWELL) % ORBIT_POINTS;
                let target = orbit_point(anchor, angle);
                if self.craft.fly_towards(world, target) {
                    self.orbit_step += 1;
                }
            }
            KestrelMode::Sortie { x, z, linger } => {
                if self.spend(1) {
                    return;
                }
                if self.craft.fly_towards(world, (x, z)) {
                    if linger == 0 {
                        self.mode = KestrelMode::Returning;
                    } else {
                        self.mode = KestrelMode::Sortie {
                            x,
                            z,
                            linger: linger - 1,
                        };
                    }
                }
            }
            KestrelMode::Perch { x, z } => {
                let landed = self.perch_step(world, (x, z));
                let cost = if landed {
                    // Sentry work is the cheap mode on purpose.
                    self.perch_beat = (self.perch_beat + 1) % PERCH_DIVISOR;
                    u32::from(self.perch_beat == 0)
                } else {
                    1
                };
                self.spend(cost);
            }
            KestrelMode::Vanguard => {
                if self.spend(1) {
                    return;
                }
                let ahead = (
                    anchor.x + heading.0 * VANGUARD_DISTANCE,
                    anchor.z + heading.1 * VANGUARD_DISTANCE,
                );
                self.craft.fly_towards(world, ahead);
            }
            KestrelMode::Manual => {
                // The pilot moves it; this only keeps the meter honest.
                self.spend(1);
            }
        }
    }

    /// Spend flight ticks; on an empty cell, turn for home. True when the
    /// turn happened, so callers skip the rest of their arm.
    fn spend(&mut self, ticks: u32) -> bool {
        self.endurance = self.endurance.saturating_sub(ticks);
        if self.endurance == 0 {
            self.mode = KestrelMode::Returning;
            return true;
        }
        false
    }

    /// Returning's own drain: never triggers another return.
    fn drain(&mut self, ticks: u32) {
        self.endurance = self.endurance.saturating_sub(ticks);
    }

    /// Home: start the recharge, priced by what the flight actually spent.
    fn dock(&mut self, anchor: BlockPos) {
        let spent = ENDURANCE - self.endurance;
        self.cooldown = (spent as u64 * self.recharge_cost as u64 / ENDURANCE as u64) as u32;
        self.endurance = ENDURANCE;
        self.mode = KestrelMode::Docked;
        self.craft.move_to(anchor);
    }

    /// Fly to the perch column, then settle onto the ground: one block above
    /// the surface, not the cruise altitude. Returns true once landed.
    fn perch_step(&mut self, world: &World, target: (i32, i32)) -> bool {
        let here = self.craft.position;
        if (here.x, here.z) != target {
            self.craft.fly_towards(world, target);
            return false;
        }
        let Some(surface) = world.surface_y(here.x, here.z) else {
            return false;
        };
        let seat = surface + 1;
        if here.y == seat {
            return true;
        }
        let step = (seat - here.y).clamp(-CLIMB_RATE, CLIMB_RATE);
        self.craft.move_to(here.offset([0, step, 0]));
        self.craft.position.y == seat
    }
}

/// The `n`-th waypoint of the orbit ring around `anchor`. A fixed integer
/// octagon rather than trigonometry: deterministic, and the flier's
/// block-hop movement would quantise a true circle to this anyway.
fn orbit_point(anchor: BlockPos, n: u32) -> (i32, i32) {
    let r = ORBIT_RADIUS;
    let half = (r * 100 / 141).max(1); // r / sqrt(2), integer
    let ring = [
        (r, 0),
        (half, half),
        (0, r),
        (-half, half),
        (-r, 0),
        (-half, -half),
        (0, -r),
        (half, -half),
    ];
    let (dx, dz) = ring[(n % ORBIT_POINTS) as usize];
    (anchor.x + dx, anchor.z + dz)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture;

    fn home() -> BlockPos {
        BlockPos::new(0, 61, 0)
    }

    fn run(kestrel: &mut Kestrel, world: &World, ticks: u32) {
        for _ in 0..ticks {
            kestrel.tick(world, home(), (1, 0));
        }
    }

    #[test]
    fn two_identical_flights_are_identical() {
        let world = fixture::flat(4, 60);
        let fly = || {
            let mut kestrel = Kestrel::new(home());
            assert!(kestrel.order(KestrelMode::Orbit));
            let mut path = Vec::new();
            for _ in 0..200 {
                kestrel.tick(&world, home(), (1, 0));
                path.push(kestrel.craft.position);
            }
            path
        };
        assert_eq!(fly(), fly(), "the orbit is not deterministic");
    }

    #[test]
    fn the_orbit_stays_near_the_ring_and_off_the_ground() {
        let world = fixture::flat(4, 60);
        let mut kestrel = Kestrel::new(home());
        kestrel.order(KestrelMode::Orbit);
        run(&mut kestrel, &world, 60);
        for _ in 0..120 {
            kestrel.tick(&world, home(), (1, 0));
            let at = kestrel.craft.position;
            let level = (((at.x - home().x).pow(2) + (at.z - home().z).pow(2)) as f32).sqrt();
            assert!(
                level <= ORBIT_RADIUS as f32 + 2.0,
                "wandered off the ring to {at:?}"
            );
            assert!(!world.is_solid(at), "inside the ground at {at:?}");
        }
    }

    #[test]
    fn an_empty_cell_forces_the_flight_home_and_recharge_is_proportional() {
        let world = fixture::flat(4, 60);
        let mut kestrel = Kestrel::new(home());
        kestrel.order(KestrelMode::Orbit);

        // Fly the cell dry, then keep ticking: it must dock by itself.
        let mut docked_at = None;
        for tick in 0..(ENDURANCE + 400) {
            kestrel.tick(&world, home(), (1, 0));
            if !kestrel.aloft() {
                docked_at = Some(tick);
                break;
            }
        }
        let docked_at = docked_at.expect("never came home");
        assert!(docked_at >= ENDURANCE - 1, "gave up early at {docked_at}");
        // A full cell spent means (close to) a full recharge.
        assert!(
            kestrel.cooldown >= COOLDOWN * 9 / 10,
            "a spent cell recharged too cheaply: {}",
            kestrel.cooldown
        );
        assert!(!kestrel.ready(), "flying again with an empty cell");
        // And a launch order while recharging is refused.
        assert!(!kestrel.order(KestrelMode::Orbit));

        let wait = kestrel.cooldown;
        run(&mut kestrel, &world, wait);
        assert!(kestrel.ready(), "recharge never finished");
    }

    #[test]
    fn a_short_hop_costs_a_short_recharge() {
        let world = fixture::flat(4, 60);
        let mut kestrel = Kestrel::new(home());
        kestrel.order(KestrelMode::Sortie {
            x: 6,
            z: 0,
            linger: 2,
        });
        for _ in 0..200 {
            kestrel.tick(&world, home(), (1, 0));
            if !kestrel.aloft() {
                break;
            }
        }
        assert!(!kestrel.aloft(), "the sortie never came home");
        assert!(
            kestrel.cooldown < COOLDOWN / 4,
            "a short hop cost {} recharge ticks",
            kestrel.cooldown
        );
    }

    #[test]
    fn a_perched_sentry_outlasts_an_orbit_by_about_fourfold() {
        let world = fixture::flat(6, 60);
        let endurance_of = |mode: KestrelMode| {
            let mut kestrel = Kestrel::new(home());
            kestrel.order(mode);
            let mut ticks = 0u32;
            for _ in 0..(ENDURANCE * (PERCH_DIVISOR + 2)) {
                kestrel.tick(&world, home(), (1, 0));
                ticks += 1;
                if matches!(kestrel.mode, KestrelMode::Returning) {
                    break;
                }
            }
            ticks
        };
        let orbiting = endurance_of(KestrelMode::Orbit);
        let perched = endurance_of(KestrelMode::Perch { x: 4, z: 0 });
        assert!(
            perched > orbiting * 3,
            "perch should be the cheap mode: {perched} vs {orbiting}"
        );
    }

    #[test]
    fn the_kestrel_is_never_inside_terrain_crossing_a_cliff() {
        // The flier's non-negotiable rule, inherited and re-pinned.
        let world = fixture::shaped(4, |x| if x < 12 { 40 } else { 70 });
        let start = BlockPos::new(0, 47, 0);
        let mut kestrel = Kestrel::new(start);
        kestrel.order(KestrelMode::Sortie {
            x: 30,
            z: 0,
            linger: 4,
        });
        for tick in 0..600 {
            kestrel.tick(&world, start, (1, 0));
            assert!(
                !world.is_solid(kestrel.craft.position),
                "inside the cliff at {:?} on tick {tick}",
                kestrel.craft.position
            );
            if !kestrel.aloft() {
                return;
            }
        }
        panic!("never made it home over the cliff");
    }

    #[test]
    fn the_vanguard_holds_ahead_of_the_heading() {
        let world = fixture::flat(6, 60);
        let mut kestrel = Kestrel::new(home());
        kestrel.order(KestrelMode::Vanguard);
        run(&mut kestrel, &world, 80);
        let at = kestrel.craft.position;
        assert!(
            at.x > home().x + VANGUARD_DISTANCE / 2,
            "never took the lead: {at:?}"
        );
    }
}
