//! Chips off the drilled block.
//!
//! # Cosmetic, and rigorously so
//!
//! A chip has no collision, no mass, no pickup and no opinion about the
//! simulation. [`Debris::advance`] takes nothing but its own pool and a fixed
//! `dt`, and the only thing the game does with the result is draw it. That is
//! the constraint the house style imposes on anything decorative, and it is
//! what lets the pool be as loose as it likes: chips fly through walls, and
//! nobody minds for the half second they live.
//!
//! # Deterministic, because captures must not flicker
//!
//! Every property of a chip — where it starts, which way it goes, how fast it
//! spins, how long it lasts — comes out of `hash(block, tick, index)`. No
//! random source, no wall clock, no frame time. Drill the same block on the
//! same tick in two runs and you get the same ten chips, so a capture of a
//! drill in progress is the same capture twice. The pool advances on the same
//! fixed tick as everything else that must replay.
//!
//! # A placeholder with a scheduled successor
//!
//! When micro-on-damage grows a spray, these chips stop being decoration: the
//! carved quarter-metre cells leaving the mask *are* the debris, one
//! representation serving both wound and spray. That is why a chip is a
//! quarter-metre cube from day one rather than whatever size looked good — the
//! pool built here becomes the renderer for real fragments without changing
//! shape.

use glam::Vec3;

use vx_core::{BlockPos, Face};
use vx_world::micro::SIDE;

/// Chips thrown per drill tick.
pub const CHIPS_PER_TICK: usize = 10;
/// Hard cap. The oldest chip is recycled rather than the newest dropped, so a
/// long drill keeps throwing rather than going quiet once the pool fills.
pub const CHIP_POOL: usize = 2048;
/// A chip's life, in seconds, at the two ends of the hash.
pub const CHIP_LIFE_MIN: f32 = 0.5;
pub const CHIP_LIFE_MAX: f32 = 1.1;
/// A chip is one micro cell across: a quarter of a metre.
pub const CHIP_SIZE: f32 = 1.0 / SIDE as f32;
/// How fast chips leave the face, before the fan and the hash's scatter.
pub const CHIP_SPEED: f32 = 3.4;
/// Sideways spread away from the face normal.
pub const CHIP_FAN: f32 = 0.55;
/// Gravity on a chip. Heavier than the world's, on purpose: a chip that
/// floated would read as a spark rather than as rock.
pub const CHIP_GRAVITY: f32 = 22.0;
/// Beyond this many metres from the camera, chips are not drawn. They are a
/// half-second of detail; at forty metres nobody can see one anyway.
pub const CHIP_CULL: f32 = 40.0;

/// One flying chip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Chip {
    pub position: Vec3,
    pub velocity: Vec3,
    /// Seconds left to live. Zero or below means the slot is free.
    pub life: f32,
    /// What it was when it was born, so the fade knows its span.
    pub span: f32,
    /// The drilled block's atlas tile: dirt throws dirt, ore glints ore.
    pub tile: u32,
    /// Radians per second about each axis, so a chip tumbles.
    pub spin: Vec3,
    /// Accumulated rotation.
    pub angle: Vec3,
}

impl Chip {
    pub fn alive(&self) -> bool {
        self.life > 0.0
    }

    /// How far through its life the chip is, 0 fresh .. 1 gone.
    pub fn age(&self) -> f32 {
        if self.span <= 0.0 {
            return 1.0;
        }
        (1.0 - self.life / self.span).clamp(0.0, 1.0)
    }
}

/// The fixed-capacity pool.
///
/// A `Vec` of a fixed length rather than a growing one: the cap is the point,
/// and a pool that could grow would be a pool that could stutter.
#[derive(Debug, Clone)]
pub struct Debris {
    chips: Vec<Chip>,
    /// Where the next chip goes. Wraps, recycling the oldest.
    next: usize,
}

impl Default for Debris {
    fn default() -> Self {
        Debris {
            chips: Vec::new(),
            next: 0,
        }
    }
}

impl Debris {
    pub fn new() -> Self {
        Debris::default()
    }

    /// Chips currently flying.
    pub fn live_count(&self) -> usize {
        self.chips.iter().filter(|chip| chip.alive()).count()
    }

    /// Slots the pool has ever needed. Never above [`CHIP_POOL`]. Exists so
    /// the cap can be asserted directly rather than inferred from behaviour.
    #[allow(dead_code)]
    pub fn capacity_used(&self) -> usize {
        self.chips.len()
    }

    pub fn chips(&self) -> impl Iterator<Item = &Chip> {
        self.chips.iter().filter(|chip| chip.alive())
    }

    /// Throw a tick's worth of chips off `face` of `block`.
    ///
    /// `tick` is the simulation's, and with the block position it is the whole
    /// seed: the same drill tick in a replay throws the same chips.
    pub fn drill(&mut self, block: BlockPos, face: Face, tile: u32, tick: u64) {
        for index in 0..CHIPS_PER_TICK {
            let chip = spawn(block, face, tile, tick, index);
            self.push(chip);
        }
    }

    /// Put one chip in the pool, recycling the oldest slot once full.
    fn push(&mut self, chip: Chip) {
        if self.chips.len() < CHIP_POOL {
            self.chips.push(chip);
            // `next` only matters once the pool is full; keep it pointing at
            // the oldest slot so the first wrap recycles slot zero.
            self.next = self.chips.len() % CHIP_POOL;
            return;
        }
        self.chips[self.next] = chip;
        self.next = (self.next + 1) % CHIP_POOL;
    }

    /// Advance every chip by one fixed tick.
    ///
    /// Ballistic and nothing more: no collision, no drag worth the name. A
    /// chip that clips a wall on a bad angle is a chip nobody is looking at.
    pub fn advance(&mut self, dt: f32) {
        for chip in &mut self.chips {
            if !chip.alive() {
                continue;
            }
            chip.velocity.y -= CHIP_GRAVITY * dt;
            chip.position += chip.velocity * dt;
            chip.angle += chip.spin * dt;
            chip.life -= dt;
            if chip.life <= 0.0 {
                // Zeroed rather than removed: the slot stays where it is, and
                // a dead chip draws nothing.
                chip.life = 0.0;
            }
        }
    }

    /// Every chip worth drawing from `eye`, as instanced objects.
    ///
    /// Distance culling happens here rather than in the renderer because the
    /// pool is the only thing that knows a chip is disposable.
    pub fn objects(&self, eye: Vec3) -> Vec<vx_render::Object> {
        let cull = CHIP_CULL * CHIP_CULL;
        self.chips()
            .filter(|chip| (chip.position - eye).length_squared() <= cull)
            .map(|chip| {
                // Shrinking as it dies, so a chip leaves rather than blinking
                // out. The alpha channel is not ours to use here — the object
                // pipeline is opaque — so size carries the fade.
                let size = CHIP_SIZE * (1.0 - chip.age() * 0.5);
                let model = glam::Mat4::from_scale_rotation_translation(
                    Vec3::splat(size),
                    glam::Quat::from_euler(
                        glam::EulerRot::XYZ,
                        chip.angle.x,
                        chip.angle.y,
                        chip.angle.z,
                    ),
                    chip.position,
                );
                let mut object = vx_render::Object::new(model, chip.tile);
                // Chips are lit like the face they came off rather than like
                // open sky; the caller may overwrite this, and the frame does.
                object.light = 1.0;
                object
            })
            .collect()
    }
}

/// One chip, entirely from the hash.
fn spawn(block: BlockPos, face: Face, tile: u32, tick: u64, index: usize) -> Chip {
    let seed = mix(block, tick, index);

    // Position: somewhere on the drilled face, jittered across it rather than
    // all from the centre, so a tick reads as a spray and not as a shot.
    let step = face.offset();
    let normal = Vec3::new(step[0] as f32, step[1] as f32, step[2] as f32);
    let centre = Vec3::new(
        block.x as f32 + 0.5,
        block.y as f32 + 0.5,
        block.z as f32 + 0.5,
    );
    // Two axes across the face, one along it.
    let (across_a, across_b) = face_axes(normal);
    let jitter_a = unit(seed, 0) - 0.5;
    let jitter_b = unit(seed, 1) - 0.5;
    let position = centre + normal * 0.5 + across_a * jitter_a + across_b * jitter_b;

    // Velocity: out of the face, fanned sideways.
    let fan_a = (unit(seed, 2) - 0.5) * 2.0 * CHIP_FAN;
    let fan_b = (unit(seed, 3) - 0.5) * 2.0 * CHIP_FAN;
    let speed = CHIP_SPEED * (0.6 + unit(seed, 4) * 0.8);
    let velocity = (normal + across_a * fan_a + across_b * fan_b).normalize_or_zero() * speed;

    let life = CHIP_LIFE_MIN + unit(seed, 5) * (CHIP_LIFE_MAX - CHIP_LIFE_MIN);
    let spin = Vec3::new(
        (unit(seed, 6) - 0.5) * 24.0,
        (unit(seed, 7) - 0.5) * 24.0,
        (unit(seed, 8) - 0.5) * 24.0,
    );

    Chip {
        position,
        velocity,
        life,
        span: life,
        tile,
        spin,
        angle: Vec3::ZERO,
    }
}

/// Two unit axes spanning the plane of `normal`.
fn face_axes(normal: Vec3) -> (Vec3, Vec3) {
    // Y is the obvious second axis for the four side faces; for the top and
    // bottom it is the normal itself, so X takes over.
    let helper = if normal.y.abs() > 0.5 { Vec3::X } else { Vec3::Y };
    let a = normal.cross(helper).normalize_or_zero();
    let b = normal.cross(a).normalize_or_zero();
    (a, b)
}

/// The seed for one chip: block, tick and index, stirred together.
fn mix(block: BlockPos, tick: u64, index: usize) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for value in [
        block.x as i64 as u64,
        block.y as i64 as u64,
        block.z as i64 as u64,
        tick,
        index as u64,
    ] {
        hash ^= value;
        hash = hash.wrapping_mul(0x100_0000_01b3);
        hash ^= hash >> 29;
    }
    hash
}

/// Draw `stream` of the seed as a number in 0..1.
///
/// Each stream is a different splitmix of the same seed, so the nine values a
/// chip needs are independent without nine hashes being computed.
fn unit(seed: u64, stream: u32) -> f32 {
    let mut z = seed
        .wrapping_add(u64::from(stream).wrapping_mul(0x9e37_79b9_7f4a_7c15))
        .wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    // The top 24 bits, which is more precision than an f32 mantissa holds.
    ((z >> 40) as f32) / ((1u32 << 24) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movement::MOVE_TICK;

    const TILE: u32 = 3;

    fn block() -> BlockPos {
        BlockPos::new(4, 62, -7)
    }

    #[test]
    fn a_fresh_pool_is_empty() {
        let debris = Debris::new();
        assert_eq!(debris.live_count(), 0);
        assert_eq!(debris.objects(Vec3::ZERO).len(), 0);
    }

    #[test]
    fn a_drill_tick_throws_its_chips() {
        let mut debris = Debris::new();
        debris.drill(block(), Face::PosY, TILE, 100);
        assert_eq!(debris.live_count(), CHIPS_PER_TICK);
    }

    #[test]
    fn the_same_block_and_tick_always_throw_the_same_chips() {
        // The capture guarantee. Two runs of the same journal drill the same
        // block on the same tick, and must throw chips that agree exactly.
        let spray = || {
            let mut debris = Debris::new();
            debris.drill(block(), Face::NegX, TILE, 9_001);
            for _ in 0..12 {
                debris.advance(MOVE_TICK);
            }
            debris.chips().copied().collect::<Vec<_>>()
        };
        assert_eq!(spray(), spray());
        assert!(!spray().is_empty(), "the spray was vacuously equal");
    }

    #[test]
    fn a_different_tick_throws_different_chips() {
        // Otherwise every tick of a held drill would throw the identical ten
        // chips and the spray would read as a strobe.
        let at = |tick| {
            let mut debris = Debris::new();
            debris.drill(block(), Face::PosY, TILE, tick);
            debris.chips().copied().collect::<Vec<_>>()
        };
        assert_ne!(at(10), at(11));
    }

    #[test]
    fn a_different_block_throws_different_chips() {
        let at = |pos| {
            let mut debris = Debris::new();
            debris.drill(pos, Face::PosY, TILE, 5);
            debris.chips().copied().collect::<Vec<_>>()
        };
        assert_ne!(at(BlockPos::new(0, 60, 0)), at(BlockPos::new(1, 60, 0)));
    }

    #[test]
    fn chips_leave_the_face_they_were_drilled_from() {
        // A chip that flew *into* the block would be the one thing a player
        // would actually notice.
        for face in Face::ALL {
            let mut debris = Debris::new();
            debris.drill(block(), face, TILE, 77);
            let step = face.offset();
            let normal = Vec3::new(step[0] as f32, step[1] as f32, step[2] as f32);
            for chip in debris.chips() {
                assert!(
                    chip.velocity.dot(normal) > 0.0,
                    "a chip off {face:?} flew back into the block: {:?}",
                    chip.velocity
                );
            }
        }
    }

    #[test]
    fn chips_start_on_the_drilled_face_and_spread_across_it() {
        let mut debris = Debris::new();
        debris.drill(block(), Face::PosY, TILE, 3);
        let positions: Vec<Vec3> = debris.chips().map(|chip| chip.position).collect();

        for at in &positions {
            // On the top face of the block, within its footprint.
            assert!((at.y - (block().y as f32 + 1.0)).abs() < 1e-4, "off the face: {at:?}");
            assert!((at.x - (block().x as f32 + 0.5)).abs() <= 0.5 + 1e-4);
            assert!((at.z - (block().z as f32 + 0.5)).abs() <= 0.5 + 1e-4);
        }
        // And they are not all in the same spot.
        let first = positions[0];
        assert!(
            positions.iter().any(|at| (*at - first).length() > 0.05),
            "every chip started in the same place"
        );
    }

    #[test]
    fn chips_take_the_drilled_blocks_tile() {
        let mut debris = Debris::new();
        debris.drill(block(), Face::PosX, 42, 1);
        assert!(debris.chips().all(|chip| chip.tile == 42));
    }

    #[test]
    fn chips_fall_and_then_die() {
        let mut debris = Debris::new();
        debris.drill(block(), Face::PosY, TILE, 12);
        let launched: Vec<f32> = debris.chips().map(|chip| chip.velocity.y).collect();

        for _ in 0..8 {
            debris.advance(MOVE_TICK);
        }
        let falling: Vec<f32> = debris.chips().map(|chip| chip.velocity.y).collect();
        assert_eq!(launched.len(), falling.len());
        for (before, after) in launched.iter().zip(falling.iter()) {
            assert!(after < before, "gravity did not pull the chip down");
        }

        // Past the longest life, the pool is quiet again.
        for _ in 0..((CHIP_LIFE_MAX / MOVE_TICK) as u32 + 4) {
            debris.advance(MOVE_TICK);
        }
        assert_eq!(debris.live_count(), 0, "chips outlived their span");
    }

    #[test]
    fn lives_are_spread_across_the_declared_range() {
        let mut debris = Debris::new();
        for tick in 0..20 {
            debris.drill(block(), Face::PosY, TILE, tick);
        }
        let lives: Vec<f32> = debris.chips().map(|chip| chip.span).collect();
        assert!(lives.iter().all(|life| *life >= CHIP_LIFE_MIN - 1e-6));
        assert!(lives.iter().all(|life| *life <= CHIP_LIFE_MAX + 1e-6));

        let shortest = lives.iter().cloned().fold(f32::MAX, f32::min);
        let longest = lives.iter().cloned().fold(0.0f32, f32::max);
        assert!(
            longest - shortest > (CHIP_LIFE_MAX - CHIP_LIFE_MIN) * 0.5,
            "lives clustered: {shortest} to {longest}"
        );
    }

    #[test]
    fn the_pool_never_exceeds_its_cap() {
        // The hard cap is the whole reason the pool is fixed. Drill far more
        // ticks than it can hold and it must recycle rather than grow.
        let mut debris = Debris::new();
        let ticks = (CHIP_POOL / CHIPS_PER_TICK) as u64 * 3;
        for tick in 0..ticks {
            debris.drill(block(), Face::PosY, TILE, tick);
        }
        assert_eq!(debris.capacity_used(), CHIP_POOL);
        assert!(debris.live_count() <= CHIP_POOL);
    }

    #[test]
    fn a_full_pool_keeps_throwing_fresh_chips() {
        // Recycling the oldest rather than dropping the newest: a long drill
        // must not go quiet the moment the pool fills.
        let mut debris = Debris::new();
        // One tick past the exact fill: the cap is not a multiple of the
        // per-tick count, so a tick that straddles it is the interesting case.
        for tick in 0..(CHIP_POOL / CHIPS_PER_TICK + 1) as u64 {
            debris.drill(block(), Face::PosY, TILE, tick);
        }
        assert_eq!(debris.capacity_used(), CHIP_POOL);

        let marker = 999;
        debris.drill(block(), Face::PosY, marker, 100_000);
        assert_eq!(
            debris.chips().filter(|chip| chip.tile == marker).count(),
            CHIPS_PER_TICK,
            "the newest chips were dropped instead of the oldest recycled"
        );
        assert_eq!(debris.capacity_used(), CHIP_POOL, "the pool grew past its cap");
    }

    #[test]
    fn distant_chips_are_not_drawn() {
        let mut debris = Debris::new();
        debris.drill(block(), Face::PosY, TILE, 8);

        let near = Vec3::new(block().x as f32, block().y as f32, block().z as f32);
        assert_eq!(debris.objects(near).len(), CHIPS_PER_TICK);

        let far = near + Vec3::X * (CHIP_CULL + 10.0);
        assert!(debris.objects(far).is_empty(), "a distant chip was still drawn");
    }

    #[test]
    fn dead_chips_are_not_drawn() {
        let mut debris = Debris::new();
        debris.drill(block(), Face::PosY, TILE, 8);
        let near = Vec3::new(block().x as f32, block().y as f32, block().z as f32);

        for _ in 0..((CHIP_LIFE_MAX / MOVE_TICK) as u32 + 4) {
            debris.advance(MOVE_TICK);
        }
        assert!(debris.objects(near).is_empty());
        // But the slots are still there, ready to be reused.
        assert_eq!(debris.capacity_used(), CHIPS_PER_TICK);
    }

    #[test]
    fn a_chip_is_one_micro_cell_across() {
        // The scheduled successor depends on this: when micro-on-damage grows
        // a spray, these chips *are* the carved cells, so a chip has to be a
        // cell's size rather than whatever looked good.
        assert!((CHIP_SIZE - 1.0 / SIDE as f32).abs() < 1e-6);
        assert!((CHIP_SIZE - 0.25).abs() < 1e-6);
    }

    #[test]
    fn the_hash_streams_are_independent() {
        // Nine properties come off one seed. If two streams agreed, a chip's
        // life would track its spin and the spray would look mechanical.
        let seed = mix(block(), 5, 0);
        let values: Vec<f32> = (0..9).map(|stream| unit(seed, stream)).collect();
        for (index, value) in values.iter().enumerate() {
            assert!((0.0..=1.0).contains(value), "stream {index} left 0..1: {value}");
            for (other, second) in values.iter().enumerate().skip(index + 1) {
                assert!(
                    (value - second).abs() > 1e-6,
                    "streams {index} and {other} agreed"
                );
            }
        }
    }

    #[test]
    fn advancing_an_empty_pool_does_nothing() {
        let mut debris = Debris::new();
        debris.advance(MOVE_TICK);
        assert_eq!(debris.live_count(), 0);
    }
}
