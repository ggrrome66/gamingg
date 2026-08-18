//! The village's inhabitants: deterministic wanderers with a word for you.
//!
//! No RNG state anywhere — waypoints and pauses come from hashing the
//! villager's index and how many legs of their stroll they have walked, so
//! two runs fed the same frame times are bit-identical, and nothing needs
//! saving. Each villager owns a rectangle of the village authored to contain
//! no buildings, walks its plateau at a stroll, and pauses like somebody with
//! somewhere to be eventually.
//!
//! Greetings use hysteresis: a line fires when the player comes within
//! [`GREET_RANGE`] and cannot fire again until they have stepped back out
//! past [`REARM_RANGE`] — once per approach, not once per frame.

use glam::Vec3;
use vx_render::Object;
use vx_world::village;

use crate::rig::{self, Rig};

/// Metres from the player at which a villager speaks.
pub const GREET_RANGE: f32 = 3.0;

/// Metres beyond which the greeting re-arms.
pub const REARM_RANGE: f32 = 5.0;

/// Stroll speed, metres per second.
const WALK_SPEED: f32 = 1.2;

/// A wander rectangle on the plaza: x0, z0, x1, z1 (inclusive-ish bounds in
/// block coordinates, authored clear of every building footprint).
type Patch = (f32, f32, f32, f32);

/// Who lives here: a patch to wander, a line to say, a body to wear.
const ROSTER: &[(Patch, &str, usize)] = &[
    ((-8.0, -8.0, 8.0, 4.0), "MORNIN. FINE DAY FOR DIGGIN.", 0),
    ((-9.0, 0.0, -5.0, 12.0), "SHOP IS JUST UP THE PATH.", 1),
    ((4.0, -8.0, 9.0, 8.0), "MIND THE DRONES OUT THERE.", 2),
];

/// The feet-level height villagers walk at: on top of the plaza surface.
fn walk_height() -> f32 {
    (village::GROUND_Y + 1) as f32
}

/// Hash a villager's stroll leg into `0..1`. Same construction as the tile
/// jitter; index and leg get their own streams via the salt.
fn hash01(index: usize, leg: u32, salt: u64) -> f32 {
    let mut hash = salt
        ^ (index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (leg as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
    hash ^= hash >> 30;
    hash = hash.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash ^= hash >> 27;
    hash = hash.wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^= hash >> 31;
    ((hash >> 40) as f32) / ((1u32 << 24) as f32)
}

struct Villager {
    position: Vec3,
    /// Where the last frame left them, for deriving facing.
    previous: Vec3,
    yaw: f32,
    patch: Patch,
    greeting: &'static str,
    variant: usize,
    /// Which leg of the stroll they are on; the hash streams key off it.
    leg: u32,
    waypoint: Vec3,
    /// Seconds left standing around before the next leg.
    pause: f32,
    /// Whether the current player approach has been greeted.
    greeted: bool,
}

impl Villager {
    fn new(index: usize, patch: Patch, greeting: &'static str, variant: usize) -> Self {
        let start = waypoint_in(index, 0, patch);
        Villager {
            position: start,
            previous: start,
            yaw: 0.0,
            patch,
            greeting,
            variant,
            leg: 0,
            waypoint: waypoint_in(index, 1, patch),
            pause: 0.0,
            greeted: false,
        }
    }
}

/// The `leg`-th waypoint inside a patch.
fn waypoint_in(index: usize, leg: u32, patch: Patch) -> Vec3 {
    let (x0, z0, x1, z1) = patch;
    Vec3::new(
        x0 + hash01(index, leg, 0xa1) * (x1 - x0),
        walk_height(),
        z0 + hash01(index, leg, 0xa2) * (z1 - z0),
    )
}

/// Every villager in town.
pub struct Villagers {
    folk: Vec<Villager>,
}

impl Default for Villagers {
    fn default() -> Self {
        Self::new()
    }
}

impl Villagers {
    pub fn new() -> Self {
        Villagers {
            folk: ROSTER
                .iter()
                .enumerate()
                .map(|(index, (patch, line, variant))| {
                    Villager::new(index, *patch, line, *variant)
                })
                .collect(),
        }
    }

    /// The rigs the roster wears, in variant order for [`Villagers::objects`].
    pub fn rigs() -> Vec<Rig> {
        (0..3).map(Rig::villager).collect()
    }

    /// Advance every stroll by `dt` seconds.
    pub fn update(&mut self, dt: f32) {
        for (index, villager) in self.folk.iter_mut().enumerate() {
            villager.previous = villager.position;
            if villager.pause > 0.0 {
                villager.pause = (villager.pause - dt).max(0.0);
                continue;
            }
            let to = villager.waypoint - villager.position;
            let distance = to.length();
            let step = WALK_SPEED * dt;
            if distance <= step {
                villager.position = villager.waypoint;
                villager.leg += 1;
                villager.pause = 0.8 + hash01(index, villager.leg, 0xa3) * 2.5;
                villager.waypoint = waypoint_in(index, villager.leg + 1, villager.patch);
            } else {
                villager.position += to / distance * step;
            }
            let moved = villager.position - villager.previous;
            if let Some(yaw) = rig::yaw_towards(moved.x, moved.z) {
                villager.yaw = yaw;
            }
        }
    }

    /// The line a newly greeted villager says, if the player just walked up
    /// to one. At most one line per call; hysteresis keeps it one per
    /// approach.
    pub fn greeting_for(&mut self, player: Vec3) -> Option<&'static str> {
        let mut spoken = None;
        for villager in &mut self.folk {
            let flat = Vec3::new(
                villager.position.x - player.x,
                0.0,
                villager.position.z - player.z,
            );
            let distance = flat.length();
            if distance > REARM_RANGE {
                villager.greeted = false;
            } else if distance < GREET_RANGE && !villager.greeted {
                // Everyone this close counts as met — walking into a group
                // gets one line, not a queue of them.
                villager.greeted = true;
                if spoken.is_none() {
                    spoken = Some(villager.greeting);
                }
            }
        }
        spoken
    }

    /// This frame's drawn bodies. `rigs` comes from [`Villagers::rigs`].
    pub fn objects(&self, rigs: &[Rig]) -> Vec<Object> {
        self.folk
            .iter()
            .flat_map(|villager| {
                let rig = &rigs[villager.variant % rigs.len()];
                rig.objects(villager.position, villager.yaw, 0.0)
            })
            .collect()
    }

    #[cfg(test)]
    fn positions(&self) -> Vec<Vec3> {
        self.folk.iter().map(|villager| villager.position).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strolls_stay_inside_their_patches_and_on_the_plateau() {
        let mut town = Villagers::new();
        for _ in 0..4000 {
            town.update(1.0 / 60.0);
        }
        for villager in &town.folk {
            let (x0, z0, x1, z1) = villager.patch;
            let at = villager.position;
            assert!(
                at.x >= x0 - 0.01 && at.x <= x1 + 0.01 && at.z >= z0 - 0.01 && at.z <= z1 + 0.01,
                "a villager wandered off their patch: {at:?}"
            );
            assert_eq!(at.y, walk_height(), "a villager left the ground");
            assert!(
                vx_world::village::blocks_at(at.x as i32, village::GROUND_Y + 1, at.z as i32)
                    .is_none(),
                "a patch overlaps a building at {at:?}"
            );
        }
    }

    #[test]
    fn two_runs_fed_the_same_frames_are_identical() {
        let mut a = Villagers::new();
        let mut b = Villagers::new();
        for step in 0..2000 {
            // Uneven frame times, same sequence for both.
            let dt = 1.0 / 60.0 + (step % 7) as f32 * 0.001;
            a.update(dt);
            b.update(dt);
        }
        assert_eq!(a.positions(), b.positions());
    }

    #[test]
    fn a_greeting_fires_once_per_approach() {
        let mut town = Villagers::new();
        // Park the others out of earshot so only villager 0 is in play.
        town.folk[1].position.x += 1000.0;
        town.folk[2].position.x -= 1000.0;
        let villager = town.folk[0].position;
        let near = Vec3::new(villager.x + 1.0, walk_height(), villager.z);
        let far = Vec3::new(villager.x + REARM_RANGE + 2.0, walk_height(), villager.z);

        let first = town.greeting_for(near);
        assert!(first.is_some(), "no greeting on approach");
        assert!(town.greeting_for(near).is_none(), "greeted every frame");

        // Stepping just outside greet range but inside re-arm range must NOT
        // re-arm; going properly away must.
        let lurking = Vec3::new(villager.x + GREET_RANGE + 0.5, walk_height(), villager.z);
        assert!(town.greeting_for(lurking).is_none());
        assert!(town.greeting_for(near).is_none(), "re-armed too eagerly");
        assert!(town.greeting_for(far).is_none());
        assert!(town.greeting_for(near).is_some(), "never re-armed");
    }

    #[test]
    fn the_town_draws_one_body_per_villager() {
        let town = Villagers::new();
        let rigs = Villagers::rigs();
        let objects = town.objects(&rigs);
        let parts: usize = town
            .folk
            .iter()
            .map(|villager| rigs[villager.variant % rigs.len()].parts.len())
            .sum();
        assert_eq!(objects.len(), parts);
    }
}
