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
use vx_world::town::{self, TownSite};

use crate::awareness::{self, Perception, Sighting, Surroundings, TargetKind};
use crate::clock::TimeOfDay;

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

/// Where somebody goes when the sun is down: the doorstep outside their
/// container, then the spot inside it. Two authored nodes rather than a path
/// search — the straight segments never cross a wall, which is both cheaper
/// and directly testable.
type HomeRoute = &'static [(f32, f32)];

/// Who lives here: a daytime patch, the way home, a line to say, a body to
/// wear. Every coordinate is an offset from the town centre.
const ROSTER: &[(Patch, HomeRoute, &str, usize)] = &[
    (
        (-8.0, -8.0, 8.0, 4.0),
        // The east-door container, west along the high street.
        &[(-10.0, -1.0), (-14.0, -0.5)],
        "MORNIN. FINE DAY FOR DIGGIN.",
        0,
    ),
    (
        (-9.0, 0.0, -5.0, 12.0),
        // The stacked bunks north of the plaza.
        &[(-1.0, 3.0), (-1.0, -21.0)],
        "SHOP IS JUST UP THE PATH.",
        1,
    ),
    (
        (4.0, -8.0, 9.0, 8.0),
        // The west-door container, east along the high street.
        &[(9.0, -1.0), (13.0, -0.5)],
        "MIND THE DRONES OUT THERE.",
        2,
    ),
];

/// Where a villager is in their day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Out on their patch.
    Roaming,
    /// Walking leg `n` of the route home.
    HeadingHome(usize),
    /// In for the night.
    Home,
    /// Walking the route back out, from leg `n` down.
    HeadingOut(usize),
}

/// The feet-level height a town's villagers walk at: on top of its plaza.
fn walk_height(site: &TownSite) -> f32 {
    (site.ground + 1) as f32
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
    route: HomeRoute,
    phase: Phase,
    greeting: &'static str,
    variant: usize,
    /// Which leg of the stroll they are on; the hash streams key off it.
    leg: u32,
    waypoint: Vec3,
    /// Seconds left standing around before the next leg.
    pause: f32,
    /// Whether the current player approach has been greeted.
    greeted: bool,
    /// What this villager can see and remembers.
    perception: Perception,
    /// Seconds left standing and watching rather than strolling.
    attention: f32,
}

impl Villager {
    fn new(
        index: usize,
        patch: Patch,
        route: HomeRoute,
        greeting: &'static str,
        variant: usize,
        site: &TownSite,
    ) -> Self {
        let start = waypoint_in(index, 0, patch, site);
        Villager {
            position: start,
            previous: start,
            yaw: 0.0,
            patch,
            route,
            phase: Phase::Roaming,
            greeting,
            variant,
            leg: 0,
            waypoint: waypoint_in(index, 1, patch, site),
            pause: 0.0,
            greeted: false,
            perception: Perception::default(),
            attention: 0.0,
        }
    }
    /// A world position from a route node, which is authored town-relative.
    fn route_node(&self, leg: usize, site: &TownSite) -> Vec3 {
        let (x, z) = self.route[leg.min(self.route.len() - 1)];
        Vec3::new(
            site.centre.0 as f32 + x,
            walk_height(site),
            site.centre.1 as f32 + z,
        )
    }

    /// Point them at wherever the hour says they should be.
    ///
    /// Only ever *starts* a journey; the walking itself is the same code that
    /// handles a stroll, so there is one movement path rather than two.
    fn retarget(&mut self, index: usize, daylight: bool, site: &TownSite) {
        match (daylight, self.phase) {
            // Dusk: set off for the doorstep.
            (false, Phase::Roaming) => {
                self.phase = Phase::HeadingHome(0);
                self.waypoint = self.route_node(0, site);
                self.pause = 0.0;
            }
            // Dawn: back out the way they came in.
            (true, Phase::Home) => {
                let last = self.route.len() - 1;
                self.phase = Phase::HeadingOut(last);
                self.waypoint = self.route_node(last, site);
                self.pause = 0.0;
            }
            // Caught mid-journey by the hour turning back: turn around.
            (true, Phase::HeadingHome(leg)) => {
                self.phase = Phase::HeadingOut(leg);
                self.waypoint = self.route_node(leg, site);
            }
            (false, Phase::HeadingOut(leg)) => {
                self.phase = Phase::HeadingHome(leg);
                self.waypoint = self.route_node(leg, site);
            }
            _ => {
                let _ = index;
            }
        }
    }

    /// Arrived at the current waypoint: pick the next one.
    fn arrive(&mut self, index: usize, site: &TownSite) {
        match self.phase {
            Phase::Roaming => {
                self.leg += 1;
                self.pause = 0.8 + hash01(index, self.leg, 0xa3) * 2.5;
                self.waypoint = waypoint_in(index, self.leg + 1, self.patch, site);
            }
            Phase::HeadingHome(leg) => {
                if leg + 1 < self.route.len() {
                    self.phase = Phase::HeadingHome(leg + 1);
                    self.waypoint = self.route_node(leg + 1, site);
                } else {
                    // In, and settled for the night.
                    self.phase = Phase::Home;
                }
            }
            Phase::HeadingOut(leg) => {
                if leg > 0 {
                    self.phase = Phase::HeadingOut(leg - 1);
                    self.waypoint = self.route_node(leg - 1, site);
                } else {
                    // Out on the street: back to the day's stroll.
                    self.phase = Phase::Roaming;
                    self.leg += 1;
                    self.waypoint = waypoint_in(index, self.leg + 1, self.patch, site);
                }
            }
            Phase::Home => {}
        }
    }
}

/// The `leg`-th waypoint inside a patch, in world coordinates.
///
/// Patches are authored as offsets from the town centre, so one roster serves
/// every town on the lattice.
fn waypoint_in(index: usize, leg: u32, patch: Patch, site: &TownSite) -> Vec3 {
    let (x0, z0, x1, z1) = patch;
    Vec3::new(
        site.centre.0 as f32 + x0 + hash01(index, leg, 0xa1) * (x1 - x0),
        walk_height(site),
        site.centre.1 as f32 + z0 + hash01(index, leg, 0xa2) * (z1 - z0),
    )
}

/// Every villager in town.
pub struct Villagers {
    /// The town these people live in. Patches are offsets from its centre.
    site: TownSite,
    folk: Vec<Villager>,
    /// Counts `update` calls, not wall time — which is what keeps the
    /// round-robin line-of-sight schedule deterministic.
    tick: u64,
}

impl Default for Villagers {
    fn default() -> Self {
        Self::new()
    }
}

impl Villagers {
    /// The hometown's people.
    pub fn new() -> Self {
        Villagers::for_site(&town::home_site())
    }

    /// The people of one town on the lattice.
    pub fn for_site(site: &TownSite) -> Self {
        Villagers {
            site: *site,
            tick: 0,
            folk: ROSTER
                .iter()
                .enumerate()
                .map(|(index, (patch, route, line, variant))| {
                    Villager::new(index, *patch, route, line, *variant, site)
                })
                .collect(),
        }
    }

    /// Which town these people belong to.
    pub fn site(&self) -> &TownSite {
        &self.site
    }

    /// The rigs the roster wears, in variant order for [`Villagers::objects`].
    pub fn rigs() -> Vec<Rig> {
        (0..3).map(Rig::villager).collect()
    }

    /// Advance every stroll by `dt` seconds, and let the town notice what is
    /// around it.
    ///
    /// The stroll runs first and is untouched by what anyone sees, so the
    /// deterministic wander stays deterministic; awareness only decides
    /// whether a villager *stops* and which way they face.
    pub fn update(&mut self, dt: f32, time: TimeOfDay, around: &Surroundings) {
        let site = self.site;
        let daylight = time.is_daylight();

        for (index, villager) in self.folk.iter_mut().enumerate() {
            villager.previous = villager.position;

            // The hour decides where they are heading; the walking below is
            // the same walking as ever.
            villager.retarget(index, daylight, &site);

            // Standing and watching counts as standing: a villager who has
            // stopped to look at you should not slide across the plaza. But
            // somebody on their way in for the night keeps walking — being
            // caught somebody's eye is not a reason to sleep in the street.
            let travelling = matches!(
                villager.phase,
                Phase::HeadingHome(_) | Phase::HeadingOut(_)
            );
            if villager.attention > 0.0 && !travelling {
                villager.attention = (villager.attention - dt).max(0.0);
            } else if villager.pause > 0.0 {
                villager.pause = (villager.pause - dt).max(0.0);
            } else if villager.phase != Phase::Home {
                let to = villager.waypoint - villager.position;
                let distance = to.length();
                let step = WALK_SPEED * dt;
                if distance <= step {
                    villager.position = villager.waypoint;
                    villager.arrive(index, &site);
                } else {
                    villager.position += to / distance * step;
                }
                let moved = villager.position - villager.previous;
                if let Some(yaw) = rig::yaw_towards(moved.x, moved.z) {
                    villager.yaw = yaw;
                }
            }
            villager.perception.forget(dt);
        }

        self.observe(around);
        self.react();
        self.tick = self.tick.wrapping_add(1);
    }

    /// Re-cast line of sight for whichever villagers are due this update.
    fn observe(&mut self, around: &Surroundings) {
        if self.folk.is_empty() {
            return;
        }
        let positions: Vec<Vec3> = self.folk.iter().map(|villager| villager.position).collect();

        for index in awareness::due(self.tick, self.folk.len()) {
            let eye = positions[index] + Vec3::Y * TargetKind::Villager.eye_height();

            // Everyone worth looking at: the player, the other townsfolk, and
            // the machines trundling past.
            let mut targets = Vec::new();
            if let Some(player) = around.player {
                targets.push(Sighting {
                    kind: TargetKind::Player,
                    index: 0,
                    position: player,
                    distance: (player - positions[index]).length(),
                });
            }
            for (other, at) in positions.iter().enumerate() {
                if other == index {
                    continue;
                }
                targets.push(Sighting {
                    kind: TargetKind::Villager,
                    index: other,
                    position: *at,
                    distance: (*at - positions[index]).length(),
                });
            }
            for (machine, (kind, at)) in around.machines.iter().enumerate() {
                targets.push(Sighting {
                    kind: *kind,
                    index: machine,
                    position: *at,
                    distance: (*at - positions[index]).length(),
                });
            }

            let registry = around.world.map(|world| world.registry());
            self.folk[index].perception.observe(
                around.world,
                registry,
                eye,
                &targets,
                awareness::SIGHT_RANGE,
            );
        }
    }

    /// Turn what each villager can see into what they do about it.
    fn react(&mut self) {
        for villager in &mut self.folk {
            let Some(watched) = villager.perception.watching() else {
                continue;
            };
            if watched.distance > awareness::NOTICE_RANGE {
                continue;
            }
            // Face whatever has their attention — including the remembered
            // spot, which is what stops the head snapping away the instant
            // somebody steps behind a tree.
            let to = watched.position - villager.position;
            if let Some(yaw) = rig::yaw_towards(to.x, to.z) {
                villager.yaw = yaw;
            }
            // And stop to look, if it is close and actually in view.
            if villager.perception.visible.is_some() && watched.distance < GREET_RANGE * 2.0 {
                villager.attention = villager.attention.max(0.35);
            }
        }
    }

    /// The line a villager says when the player walks up — and only when they
    /// can actually *see* them. A wall between you and a villager should not
    /// produce a cheery hello.
    ///
    /// Hysteresis unchanged in shape: a line fires inside [`GREET_RANGE`] and
    /// re-arms only once the player is back past [`REARM_RANGE`].
    pub fn greeting_for(&mut self) -> Option<&'static str> {
        let mut spoken = None;
        for villager in &mut self.folk {
            let seen = villager.perception.sees_player();
            let distance = seen.map(|player| player.distance);
            match distance {
                Some(distance) if distance < GREET_RANGE && !villager.greeted => {
                    // Everyone who can see you counts as met — walking into a
                    // group gets one line, not a queue of them.
                    villager.greeted = true;
                    if spoken.is_none() {
                        spoken = Some(villager.greeting);
                    }
                }
                // Out of view, or far enough away, re-arms the greeting.
                None => villager.greeted = false,
                Some(distance) if distance > REARM_RANGE => villager.greeted = false,
                Some(_) => {}
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
            town.update(1.0 / 60.0, TimeOfDay::NOON, &Surroundings::empty());
        }
        for villager in &town.folk {
            let (x0, z0, x1, z1) = villager.patch;
            let at = villager.position;
            assert!(
                at.x >= x0 - 0.01 && at.x <= x1 + 0.01 && at.z >= z0 - 0.01 && at.z <= z1 + 0.01,
                "a villager wandered off their patch: {at:?}"
            );
            assert_eq!(at.y, walk_height(&town::home_site()), "a villager left the ground");
            assert!(
                vx_world::town::plan::cell_at(&town::home_site(), at.x as i32, town::HOME_GROUND_Y + 1, at.z as i32)
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
            a.update(dt, TimeOfDay::NOON, &Surroundings::empty());
            b.update(dt, TimeOfDay::NOON, &Surroundings::empty());
        }
        assert_eq!(a.positions(), b.positions());
    }

    /// Drive the town for one update with the player standing at `player`,
    /// long enough for the round-robin to have looked at everybody.
    fn look_around(town: &mut Villagers, player: Vec3) {
        let around = Surroundings {
            world: None,
            player: Some(player),
            machines: &[],
        };
        for _ in 0..town.folk.len() {
            town.update(0.0, TimeOfDay::NOON, &around);
        }
    }

    #[test]
    fn a_greeting_fires_once_per_approach() {
        let mut town = Villagers::new();
        // Park the others out of earshot so only villager 0 is in play.
        town.folk[1].position.x += 1000.0;
        town.folk[2].position.x -= 1000.0;
        let villager = town.folk[0].position;
        let near = Vec3::new(villager.x + 1.0, walk_height(&town::home_site()), villager.z);
        let far = Vec3::new(villager.x + REARM_RANGE + 2.0, walk_height(&town::home_site()), villager.z);

        look_around(&mut town, near);
        assert!(town.greeting_for().is_some(), "no greeting on approach");
        look_around(&mut town, near);
        assert!(town.greeting_for().is_none(), "greeted every frame");

        // Just outside greeting range but inside re-arm range must NOT re-arm.
        let lurking = Vec3::new(villager.x + GREET_RANGE + 0.5, walk_height(&town::home_site()), villager.z);
        look_around(&mut town, lurking);
        assert!(town.greeting_for().is_none());
        look_around(&mut town, near);
        assert!(town.greeting_for().is_none(), "re-armed too eagerly");

        // Properly away, then back: a fresh approach, a fresh hello.
        look_around(&mut town, far);
        assert!(town.greeting_for().is_none());
        look_around(&mut town, near);
        assert!(town.greeting_for().is_some(), "never re-armed");
    }

    #[test]
    fn a_villager_turns_to_face_a_player_they_can_see() {
        let mut town = Villagers::new();
        let at = town.folk[0].position;
        // Stand due +x of them.
        let player = Vec3::new(at.x + 2.0, walk_height(&town::home_site()), at.z);
        look_around(&mut town, player);

        let facing = town.folk[0].yaw;
        let nose = glam::Mat4::from_rotation_y(facing).transform_vector3(Vec3::X);
        assert!(
            nose.x > 0.9,
            "villager faced {nose:?} instead of toward the player"
        );
    }

    #[test]
    fn a_villager_behind_a_wall_neither_turns_nor_greets() {
        // Real terrain with a wall dropped between the two.
        let mut world = vx_world::World::new(2024);
        world.load_around(vx_core::ChunkPos::new(0, 0), 1);
        let stone = world.registry().id_of("engine:stone").unwrap();

        let mut town = Villagers::new();
        town.folk[1].position.x += 1000.0;
        town.folk[2].position.x -= 1000.0;
        let at = town.folk[0].position;
        town.folk[0].yaw = 0.0;
        let player = Vec3::new(at.x + 2.5, at.y, at.z);

        let ground = at.y as i32;
        for dy in 0..4 {
            for dz in -3..=3 {
                world.set_block(
                    vx_core::BlockPos::new(at.x as i32 + 1, ground + dy, at.z as i32 + dz),
                    stone,
                );
            }
        }

        let around = Surroundings {
            world: Some(&world),
            player: Some(player),
            machines: &[],
        };
        for _ in 0..town.folk.len() {
            town.update(0.0, TimeOfDay::NOON, &around);
        }

        assert!(town.greeting_for().is_none(), "greeted through a wall");
        assert_eq!(town.folk[0].yaw, 0.0, "turned to face somebody they cannot see");
    }

    #[test]
    fn a_watched_villager_pauses_their_stroll_and_resumes_after() {
        let mut town = Villagers::new();
        town.folk[1].position.x += 1000.0;
        town.folk[2].position.x -= 1000.0;
        // Clear any starting pause so movement is the only variable.
        town.folk[0].pause = 0.0;
        let at = town.folk[0].position;
        let player = Vec3::new(at.x + 1.5, walk_height(&town::home_site()), at.z);

        let around = Surroundings {
            world: None,
            player: Some(player),
            machines: &[],
        };
        for _ in 0..3 {
            town.update(1.0 / 60.0, TimeOfDay::NOON, &around);
        }
        let watching_from = town.folk[0].position;
        for _ in 0..10 {
            town.update(1.0 / 60.0, TimeOfDay::NOON, &around);
        }
        assert_eq!(
            town.folk[0].position, watching_from,
            "kept strolling while being watched"
        );

        // Player leaves: the stroll picks back up.
        let alone = Surroundings::empty();
        for _ in 0..120 {
            town.update(1.0 / 60.0, TimeOfDay::NOON, &alone);
        }
        assert_ne!(
            town.folk[0].position, watching_from,
            "never resumed the stroll"
        );
    }

    #[test]
    fn a_villager_notices_a_passing_drone() {
        let mut town = Villagers::new();
        let at = town.folk[0].position;
        let drone = Vec3::new(at.x + 3.0, at.y, at.z);
        let machines = [(TargetKind::Digger, drone)];
        let around = Surroundings {
            world: None,
            player: None,
            machines: &machines,
        };
        for _ in 0..town.folk.len() {
            town.update(0.0, TimeOfDay::NOON, &around);
        }

        let watched = town.folk[0].perception.watching().expect("saw nothing at all");
        assert_eq!(watched.kind, TargetKind::Digger);
    }

    #[test]
    fn two_runs_over_the_same_world_and_route_stay_identical() {
        // The stronger determinism claim: not just an empty room, but real
        // terrain and a player walking about.
        let mut world = vx_world::World::new(2024);
        world.load_around(vx_core::ChunkPos::new(0, 0), 1);

        let run = || {
            let mut town = Villagers::new();
            for step in 0..600 {
                let dt = 1.0 / 60.0 + (step % 7) as f32 * 0.001;
                let player = Vec3::new(
                    (step as f32 * 0.02).sin() * 6.0,
                    walk_height(&town::home_site()),
                    (step as f32 * 0.017).cos() * 6.0,
                );
                let around = Surroundings {
                    world: Some(&world),
                    player: Some(player),
                    machines: &[],
                };
                town.update(dt, TimeOfDay::NOON, &around);
            }
            town.positions()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn the_town_goes_indoors_at_night_and_back_out_at_dawn() {
        let mut town = Villagers::new();
        let alone = Surroundings::empty();

        // A night's worth of updates: everyone should be home and settled.
        for _ in 0..4000 {
            town.update(1.0 / 60.0, TimeOfDay::MIDNIGHT, &alone);
        }
        for (index, villager) in town.folk.iter().enumerate() {
            assert_eq!(villager.phase, Phase::Home, "villager {index} never got home");
            let inside = villager.route_node(villager.route.len() - 1, &town.site);
            assert!(
                (villager.position - inside).length() < 1.0,
                "villager {index} settled at {:?}, not indoors at {inside:?}",
                villager.position
            );
        }

        // And a morning's worth: back out on the plaza.
        for _ in 0..4000 {
            town.update(1.0 / 60.0, TimeOfDay::NOON, &alone);
        }
        for (index, villager) in town.folk.iter().enumerate() {
            assert_eq!(villager.phase, Phase::Roaming, "villager {index} stayed in bed");
            let (x0, z0, x1, z1) = villager.patch;
            let at = villager.position;
            let centre = town.site.centre;
            assert!(
                at.x >= centre.0 as f32 + x0 - 0.01
                    && at.x <= centre.0 as f32 + x1 + 0.01
                    && at.z >= centre.1 as f32 + z0 - 0.01
                    && at.z <= centre.1 as f32 + z1 + 0.01,
                "villager {index} came out somewhere odd: {at:?}"
            );
        }
    }

    #[test]
    fn nobody_walks_through_a_wall_on_the_way_home() {
        // The route is two authored nodes precisely so its straight segments
        // stay in the doorway. Sample them against the town's own blueprint.
        let site = town::home_site();
        let town_folk = Villagers::new();
        for (index, villager) in town_folk.folk.iter().enumerate() {
            let mut nodes = vec![villager.position];
            for leg in 0..villager.route.len() {
                nodes.push(villager.route_node(leg, &site));
            }
            for pair in nodes.windows(2) {
                for step in 0..=20 {
                    let t = step as f32 / 20.0;
                    let at = pair[0] + (pair[1] - pair[0]) * t;
                    let (x, z) = (at.x.round() as i32, at.z.round() as i32);
                    // Head height: a wall blocks, a doorway does not.
                    let cell = vx_world::town::plan::cell_at(&site, x, site.ground + 2, z);
                    assert!(
                        cell.is_none(),
                        "villager {index} walks through {cell:?} at ({x},{z})"
                    );
                }
            }
        }
    }

    #[test]
    fn the_clock_is_an_input_so_two_identical_days_are_identical() {
        // Determinism has to survive the schedule: the hour is a parameter,
        // never a wall-clock read.
        let run = || {
            let mut town = Villagers::new();
            let alone = Surroundings::empty();
            for step in 0..3000 {
                let time = TimeOfDay::new(step as f32 / 3000.0);
                town.update(1.0 / 60.0, time, &alone);
            }
            town.positions()
        };
        assert_eq!(run(), run());
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
