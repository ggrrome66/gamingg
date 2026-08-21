//! The arsenal: the compact slug launcher, and what firing it costs you.
//!
//! # A projectile steps on the journal clock
//!
//! A slug is spawned by a journalled `Fire` order and then advanced one step
//! per journal tick — the same clock the drones dig on — with a fixed step
//! length, so its whole flight is a pure function of the order. Hit detection
//! is `raycast_solid` over the segment the round swept that tick; no new
//! traversal, no per-frame physics, and a replayed journal carves the same
//! craters, which is what keeps the world hash honest after a firefight.
//!
//! # A slug does not ask permission
//!
//! Ballistic block damage goes through `World::set_block` directly (bedrock
//! and hardness guarded here) rather than through the cancellable break event.
//! The permits gate exists to *refuse* an edit before it happens; a fired slug
//! is past refusing. The consequence arrives as a bill instead: the caller
//! reads the [`Sweep`]s this module returns and charges bounty for whatever
//! was hit in view of witnesses.
//!
//! # What a slug can break
//!
//! [`SLUG_PUNCH`] sits between sheet metal and mast steel on the hardness
//! scale: a slug shatters planks, roofs, logs, sheet metal — and craters
//! soft rock, which sits at the bottom of the scale — but glances off ore
//! bodies, mast steel and every grade of lockbox. It is still no mining
//! tool: one block per slug at shop prices is the worst drill money can buy,
//! and the ore a mine exists for is exactly what it cannot touch.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::Path;

use glam::Vec3;
use vx_core::BlockPos;
use vx_world::{PlayerBody, World};

use crate::movement::{self, Movement};
use crate::tuning::Tuning;

const MAGIC: &[u8; 4] = b"VXAR";
const VERSION: u32 = 1;

/// Seconds per journal tick — the projectile integration step.
pub const TICK_SECONDS: f32 = (1.0 / crate::mining::TICK_RATE) as f32;

/// Muzzle speed, metres per second. Slow for a bullet on purpose: you can
/// watch the round go, and lead a moving caravan.
pub const SLUG_SPEED: f32 = 55.0;

/// Gravity on a slug, metres per second squared. Heavier than honest physics
/// so range is a skill rather than a given.
pub const SLUG_GRAVITY: f32 = 14.0;

/// The hardest block a slug breaks. Between sheet metal (1.6) and mast steel
/// (2.0): buildings are vulnerable; ore, masts and lockboxes are not.
pub const SLUG_PUNCH: f32 = 1.7;

/// Recoil speed handed to the shooter, metres per second, opposite the aim.
pub const SLUG_KICK: f32 = 4.5;

/// Journal ticks between shots.
pub const SLUG_RATE: f32 = 6.0;

/// Ticks a slug flies before it is spent. 32 ticks at 55 m/s is roughly 200 m
/// of reach, less what gravity takes.
pub const SLUG_TTL: u32 = 32;

/// Screen shake multiplier. Visual only — it never touches the simulation —
/// but it lives in the tuning table because "how scary" is a number a
/// designer drags.
pub const SHAKE_POWER: f32 = 1.0;

/// How far in front of the eye the round spawns, clear of the shooter's own
/// hull.
const MUZZLE_CLEARANCE: f32 = 0.4;

/// What the launcher costs at the shop counter.
pub const LAUNCHER_COST: u64 = 600;

/// What a box of slugs costs, and how many come in it.
pub const SLUG_COST: u64 = 40;
pub const SLUG_BATCH: u32 = 8;

/// How much the slung launcher weighs, in the same pack units the movement
/// load byte speaks. Folded into the load *before* the command is journalled,
/// so the weight replays without the oracle ever learning what a weapon is.
pub const LAUNCHER_HEFT: u64 = 15;

/// Base bounty for one block of somebody's property destroyed, before the
/// witness multiplier.
pub const BOUNTY_PROPERTY: u64 = 12;

/// Bounty for pointing the muzzle at a person in view of anyone — or of the
/// person themselves. Assault without a health bar.
pub const BOUNTY_MENACE: u64 = 10;

/// Bounty added when a panicked villager reaches the security office and
/// reports you. The office is its own witness.
pub const BOUNTY_REPORTED: u64 = 20;

/// One slug in flight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shot {
    pub position: Vec3,
    pub velocity: Vec3,
    pub age: u32,
}

/// Where one shot went this tick, and what it met.
#[derive(Debug, Clone, PartialEq)]
pub struct Sweep {
    pub from: Vec3,
    pub to: Vec3,
    pub hit: Option<Impact>,
}

/// The end of a flight.
#[derive(Debug, Clone, PartialEq)]
pub struct Impact {
    /// Where the round stopped.
    pub at: Vec3,
    /// The block it struck.
    pub block: BlockPos,
    /// Whether the block gave way. A glance off rock is still an impact —
    /// still loud, still a crime if the rock was somebody's wall's footing —
    /// but only a broken block owes property bounty.
    pub broke: bool,
}

/// Spawn a slug from a `Fire` order and stagger the shooter.
///
/// Called by live fire and by journal replay with the same arguments — the
/// recorded muzzle and the quantised aim — which is what makes a firefight
/// replay: the two sides run on different clocks, so the muzzle position is
/// part of the order rather than derived from the body.
pub fn launch(
    shots: &mut Vec<Shot>,
    movement: &mut Movement,
    muzzle: Vec3,
    yaw_q: i16,
    pitch_q: i16,
) {
    let aim = movement::aim_vector(yaw_q, pitch_q);
    shots.push(Shot {
        position: muzzle + aim * MUZZLE_CLEARANCE,
        velocity: aim * movement.tuning.slug_speed,
        age: 0,
    });
    movement.kick(-aim * movement.tuning.slug_kick);
}

/// Where the muzzle sits for a body about to fire.
pub fn muzzle_of(player: &PlayerBody) -> Vec3 {
    player.eye_position()
}

/// Step every shot one journal tick, breaking what they break.
///
/// Returns each shot's sweep so the live game can bill the damage and test
/// the segment against caravans and bystanders; replay drops the return value
/// — the world edits are the part the oracle checks.
pub fn advance_shots(shots: &mut Vec<Shot>, world: &mut World, tuning: &Tuning) -> Vec<Sweep> {
    let mut sweeps = Vec::with_capacity(shots.len());
    shots.retain_mut(|shot| {
        let from = shot.position;
        shot.velocity.y -= tuning.slug_gravity * TICK_SECONDS;
        let travel = shot.velocity * TICK_SECONDS;
        let length = travel.length();
        let hit = if length > 1.0e-6 {
            vx_world::raycast_solid(&*world, world.registry(), from, travel / length, length)
        } else {
            None
        };

        let (alive, to, impact) = match hit {
            Some(found) => {
                let stopped = from + travel / length * found.distance;
                let breakable = world
                    .registry()
                    .get(found.id)
                    .and_then(|def| def.hardness)
                    .is_some_and(|hardness| hardness <= tuning.slug_punch);
                if breakable {
                    world.set_block(found.block, vx_core::BlockId::AIR);
                }
                (
                    false,
                    stopped,
                    Some(Impact {
                        at: stopped,
                        block: found.block,
                        broke: breakable,
                    }),
                )
            }
            None => {
                shot.position = from + travel;
                shot.age += 1;
                (shot.age < SLUG_TTL, from + travel, None)
            }
        };
        sweeps.push(Sweep {
            from,
            to,
            hit: impact,
        });
        alive
    });
    sweeps
}

/// A downed caravan's load, on the ground where it fell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crash {
    pub x: f32,
    pub z: f32,
    pub good: usize,
    pub amount: u64,
}

/// What the player owns and has done with it. Live state, saved beside the
/// wallet; the shots themselves are transient and the orders are the
/// journal's.
#[derive(Debug, Default)]
pub struct Arsenal {
    /// Whether the launcher has been bought.
    pub owned: bool,
    /// Slugs in the satchel.
    pub ammo: u32,
    /// Whether the launcher is in hand rather than the drill.
    pub equipped: bool,
    /// Journal ticks until the next shot is allowed.
    pub cooldown: f32,
    /// Towns that have already spent their one warning, by name.
    warned: BTreeSet<String>,
    /// Cargo on the ground where caravans came down.
    pub crashes: Vec<Crash>,
}

impl Arsenal {
    /// Whether a shot may be fired right now.
    pub fn ready(&self) -> bool {
        self.owned && self.equipped && self.ammo > 0 && self.cooldown <= 0.0
    }

    /// Spend one slug and start the cooldown.
    pub fn spend(&mut self, tuning: &Tuning) {
        self.ammo = self.ammo.saturating_sub(1);
        self.cooldown = tuning.slug_rate;
    }

    /// Run the cooldown down by some journal ticks.
    pub fn cool(&mut self, ticks: u32) {
        self.cooldown = (self.cooldown - ticks as f32).max(0.0);
    }

    /// Spend a town's one warning. True when this was it — the caller shows
    /// the warning; every later shot in that town is past warnings.
    pub fn warn_once(&mut self, town: &str) -> bool {
        self.warned.insert(town.to_string())
    }

    /// Write the arsenal beside the world save.
    pub fn save(&self, directory: &Path) -> std::io::Result<()> {
        let mut file =
            std::io::BufWriter::new(std::fs::File::create(directory.join("arsenal.dat"))?);
        file.write_all(MAGIC)?;
        file.write_all(&VERSION.to_le_bytes())?;
        file.write_all(&[u8::from(self.owned)])?;
        file.write_all(&self.ammo.to_le_bytes())?;
        file.write_all(&(self.warned.len() as u32).to_le_bytes())?;
        for town in &self.warned {
            let bytes = town.as_bytes();
            file.write_all(&(bytes.len() as u32).to_le_bytes())?;
            file.write_all(bytes)?;
        }
        file.write_all(&(self.crashes.len() as u32).to_le_bytes())?;
        for crash in &self.crashes {
            file.write_all(&crash.x.to_bits().to_le_bytes())?;
            file.write_all(&crash.z.to_bits().to_le_bytes())?;
            file.write_all(&(crash.good as u32).to_le_bytes())?;
            file.write_all(&crash.amount.to_le_bytes())?;
        }
        file.flush()
    }

    /// Load the arsenal, tolerating absence and damage.
    pub fn load(&mut self, directory: &Path) {
        let path = directory.join("arsenal.dat");
        match read_arsenal(&path) {
            Ok(Some((owned, ammo, warned, crashes))) => {
                self.owned = owned;
                self.ammo = ammo;
                self.warned = warned;
                self.crashes = crashes;
            }
            Ok(None) => {}
            Err(error) => {
                log::warn!("could not read {}: {error}; starting fresh", path.display());
                *self = Arsenal::default();
            }
        }
    }
}

type ArsenalData = (bool, u32, BTreeSet<String>, Vec<Crash>);

fn read_arsenal(path: &Path) -> std::io::Result<Option<ArsenalData>> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => std::io::BufReader::new(file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };

    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(std::io::Error::other("bad magic"));
    }
    let mut word = [0u8; 4];
    file.read_exact(&mut word)?;
    if u32::from_le_bytes(word) != VERSION {
        return Err(std::io::Error::other("unknown version"));
    }
    let mut byte = [0u8; 1];
    file.read_exact(&mut byte)?;
    let owned = byte[0] != 0;
    file.read_exact(&mut word)?;
    let ammo = u32::from_le_bytes(word);

    file.read_exact(&mut word)?;
    let towns = u32::from_le_bytes(word);
    let mut warned = BTreeSet::new();
    for _ in 0..towns {
        file.read_exact(&mut word)?;
        let length = u32::from_le_bytes(word) as usize;
        if length > 256 {
            return Err(std::io::Error::other("town name implausibly long"));
        }
        let mut bytes = vec![0u8; length];
        file.read_exact(&mut bytes)?;
        let name = String::from_utf8(bytes)
            .map_err(|_| std::io::Error::other("town name not utf-8"))?;
        warned.insert(name);
    }

    file.read_exact(&mut word)?;
    let count = u32::from_le_bytes(word);
    let mut crashes = Vec::new();
    for _ in 0..count {
        let mut long = [0u8; 8];
        file.read_exact(&mut word)?;
        let x = f32::from_bits(u32::from_le_bytes(word));
        file.read_exact(&mut word)?;
        let z = f32::from_bits(u32::from_le_bytes(word));
        file.read_exact(&mut word)?;
        let good = u32::from_le_bytes(word) as usize;
        file.read_exact(&mut long)?;
        let amount = u64::from_le_bytes(long);
        crashes.push(Crash { x, z, good, amount });
    }

    Ok(Some((owned, ammo, warned, crashes)))
}

/// The bounty a piece of witnessed damage costs: the damage, plus half again
/// for every witness beyond the first. Zero witnesses is zero bounty — seen
/// is the rule, for gunfire the same as for lockpicks.
pub fn witnessed_bounty(damage: u64, witnesses: usize) -> u64 {
    if witnesses == 0 {
        return 0;
    }
    let scale = 1.0 + 0.5 * (witnesses as f64 - 1.0);
    (damage as f64 * scale).round() as u64
}

/// Whether a swept segment passes through an axis-aligned box.
///
/// The caravan test: the box is the machine's hull, the segment is where a
/// slug went this tick. Standard slab test, degenerate axes handled by the
/// containment check.
pub fn segment_hits_box(from: Vec3, to: Vec3, centre: Vec3, half: Vec3) -> bool {
    let direction = to - from;
    let mut enter = 0.0f32;
    let mut exit = 1.0f32;
    for axis in 0..3 {
        let start = from[axis] - centre[axis];
        let step = direction[axis];
        if step.abs() < 1.0e-6 {
            if start.abs() > half[axis] {
                return false;
            }
            continue;
        }
        let low = (-half[axis] - start) / step;
        let high = (half[axis] - start) / step;
        let (near, far) = if low < high { (low, high) } else { (high, low) };
        enter = enter.max(near);
        exit = exit.min(far);
        if enter > exit {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::ChunkPos;

    fn open_sky() -> World {
        let mut world = World::new(2024);
        world.load_around(ChunkPos::new(0, 0), 2);
        world
    }

    /// A shot fired level from high above the ground: it must fall, and its
    /// path must be identical every time it is refought.
    #[test]
    fn a_slug_drops_and_two_flights_match() {
        let mut world = open_sky();
        let tuning = Tuning::default();
        let fly = |world: &mut World| {
            let mut shots = vec![Shot {
                position: Vec3::new(0.5, 200.0, 0.5),
                velocity: Vec3::new(tuning.slug_speed, 0.0, 0.0),
                age: 0,
            }];
            let mut path = Vec::new();
            for _ in 0..8 {
                advance_shots(&mut shots, world, &tuning);
                if let Some(shot) = shots.first() {
                    path.push(shot.position);
                }
            }
            path
        };
        let first = fly(&mut world);
        let second = fly(&mut world);
        assert_eq!(first, second, "a flight is not deterministic");
        let start = 200.0;
        let end = first.last().expect("shot died in open sky").y;
        assert!(end < start - 3.0, "no gravity: still at {end}");
    }

    #[test]
    fn a_slug_breaks_planks_and_glances_off_ore() {
        let mut world = open_sky();
        let tuning = Tuning::default();
        let plank = world.registry().id_of("engine:plank").expect("no plank");
        let ore = world
            .registry()
            .id_of("engine:copper_ore")
            .expect("no copper ore");
        let air = vx_core::BlockId::AIR;

        for (target, expect_broken) in [(plank, true), (ore, false)] {
            let at = BlockPos::new(10, 180, 0);
            world.set_block(at, target);
            let mut shots = vec![Shot {
                position: Vec3::new(0.5, 180.5, 0.5),
                velocity: Vec3::new(tuning.slug_speed, 0.0, 0.0),
                age: 0,
            }];
            // A tick sweeps ~7 m; the wall at x = 10 takes two.
            let mut landed = None;
            for _ in 0..4 {
                let sweeps = advance_shots(&mut shots, &mut world, &tuning);
                if let Some(sweep) = sweeps.into_iter().find(|sweep| sweep.hit.is_some()) {
                    landed = sweep.hit;
                    break;
                }
            }
            let impact = landed.as_ref().expect("shot missed the wall");
            assert_eq!(impact.block, at);
            assert_eq!(impact.broke, expect_broken);
            assert_eq!(world.block(at) == air, expect_broken);
            assert!(shots.is_empty(), "a shot survives its own impact");
            world.set_block(at, air);
        }
    }

    #[test]
    fn a_slug_expires_in_open_sky() {
        let mut world = open_sky();
        // Aimed up, so gravity brings it back slowly enough to time out.
        let mut shots = vec![Shot {
            position: Vec3::new(0.5, 220.0, 0.5),
            velocity: Vec3::new(0.0, 30.0, 0.0),
            age: 0,
        }];
        for _ in 0..SLUG_TTL {
            advance_shots(&mut shots, &mut world, &Tuning::default());
        }
        assert!(shots.is_empty(), "a slug should be spent after {SLUG_TTL} ticks");
    }

    #[test]
    fn recoil_shoves_opposite_the_aim() {
        let mut shots = Vec::new();
        let mut movement = Movement::default();
        let mut body = PlayerBody::default();
        // Aim straight down -Z (yaw_q = 0, pitch_q = 0).
        launch(&mut shots, &mut movement, body.eye_position(), 0, 0);
        assert_eq!(shots.len(), 1);
        assert!(shots[0].velocity.z < 0.0, "yaw 0 must fire down -Z");

        // The queued kick lands on the body at the next tick.
        let world = open_sky();
        let idle = crate::movement::MoveCommand::default();
        movement.advance(&mut body, &world, idle, 1.0, crate::movement::MOVE_TICK);
        assert!(
            body.velocity.z > 0.0,
            "recoil should shove the shooter back, got {:?}",
            body.velocity
        );
    }

    #[test]
    fn the_witness_multiplier_matches_the_agreed_curve() {
        assert_eq!(witnessed_bounty(12, 0), 0);
        assert_eq!(witnessed_bounty(12, 1), 12);
        assert_eq!(witnessed_bounty(12, 2), 18);
        assert_eq!(witnessed_bounty(12, 3), 24);
        assert_eq!(witnessed_bounty(BOUNTY_MENACE, 1), BOUNTY_MENACE);
    }

    #[test]
    fn the_segment_box_test_hits_and_misses() {
        let centre = Vec3::new(10.0, 26.0, 0.0);
        let half = Vec3::new(2.0, 1.5, 2.0);
        assert!(segment_hits_box(
            Vec3::new(0.0, 26.0, 0.0),
            Vec3::new(20.0, 26.0, 0.0),
            centre,
            half
        ));
        assert!(!segment_hits_box(
            Vec3::new(0.0, 40.0, 0.0),
            Vec3::new(20.0, 40.0, 0.0),
            centre,
            half
        ));
        // Degenerate axis: a segment running inside the slab on x only.
        assert!(!segment_hits_box(
            Vec3::new(9.0, 0.0, 30.0),
            Vec3::new(11.0, 0.0, 30.0),
            centre,
            half
        ));
    }

    #[test]
    fn the_satchel_survives_the_trip_to_disk() {
        let directory = std::env::temp_dir().join(format!(
            "gamingg-arsenal-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).expect("temp dir");

        let mut kept = Arsenal {
            owned: true,
            ammo: 13,
            ..Arsenal::default()
        };
        assert!(kept.warn_once("DUSTPAN"));
        assert!(!kept.warn_once("DUSTPAN"), "one warning per town");
        kept.crashes.push(Crash {
            x: 120.5,
            z: -44.25,
            good: 2,
            amount: 90,
        });
        kept.save(&directory).expect("save");

        let mut back = Arsenal::default();
        back.load(&directory);
        assert!(back.owned);
        assert_eq!(back.ammo, 13);
        assert!(!back.warn_once("DUSTPAN"), "the warning ledger must persist");
        assert_eq!(back.crashes, kept.crashes);

        let _ = std::fs::remove_dir_all(&directory);
    }
}
