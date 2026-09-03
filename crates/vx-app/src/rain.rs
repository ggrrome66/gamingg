//! What the rain looks like, without a particle system.
//!
//! Stage 38's weather is a pure function of `(seed, tick, region)`, and the
//! rain you can see is held to the same standard: every streak's position is
//! derived from its index and the clock, so two machines on the same tick
//! draw the same rain and nothing has to be stored, spawned or recycled.
//!
//! It rides the instanced object path the falling trunk rides — a rig, not a
//! new pipeline. Each streak is one stretched cube running along the drop's
//! own velocity, which is straight down plus the wind, so a gale visibly
//! slants the weather rather than merely moving a number on the panel.

use glam::{Mat4, Quat, Vec3};
use vx_render::object::Object;
use vx_render::tiles::slot;
use vx_world::weather::Conditions;

/// How far from the eye the rain is drawn, on each horizontal axis. Beyond
/// this the streaks are too small to read and would only cost fill.
pub const COLUMN: f32 = 22.0;

/// And how close one is allowed to get. A streak on the lens is a
/// centimetre-wide box a metre from the near plane, which fills a quarter of
/// the frame with translucent grey — so the sheet is a hollow cylinder around
/// the eye rather than a solid one.
pub const NEAR: f32 = 3.5;

/// How far above the eye a streak starts, and how far below it stops. The
/// span is what a drop falls through before it is recycled to the top.
pub const CEILING: f32 = 13.0;
pub const FLOOR: f32 = -9.0;

/// The most streaks a downpour draws. Cheap — they are instances of the one
/// cube the drones already use — but not free, and past this the picture
/// stops getting wetter.
pub const MAX_DROPS: usize = 420;

/// Blocks per second, straight down. Real rain falls at about nine metres a
/// second; this is faster because a streak that reads as rain has to cross
/// the frame in well under a second.
pub const FALL_SPEED: f32 = 26.0;

/// How long one streak is, and how thick.
pub const LENGTH: f32 = 1.7;
pub const THICKNESS: f32 = 0.10;

/// How many streaks these conditions are worth.
///
/// Zero in the dry, so the whole rig costs one comparison on a clear day.
pub fn drops(sky: &Conditions) -> usize {
    if sky.rain <= 0.0 {
        return 0;
    }
    // Squared, so a light shower is genuinely light rather than half a
    // downpour: the eye reads density, not the number behind it.
    let weight = sky.rain.clamp(0.0, 1.0);
    (MAX_DROPS as f32 * weight * weight).round() as usize
}

/// The streaks to draw this frame, around `eye`.
///
/// Pure in `(seed, seconds, eye, sky)`. `seconds` is wall time rather than
/// the game tick so the rain keeps moving smoothly between the 64 Hz steps —
/// nothing downstream of this reads it, so it never touches the hash.
pub fn streaks(seed: u64, seconds: f32, eye: glam::DVec3, sky: &Conditions) -> Vec<Object> {
    // The sheet is drawn around the eye and never more than a chunk from it,
    // so it is built in the eye's own frame and narrowed once — a streak's
    // position is exact however far the eye is from the world's origin.
    let eye = eye.as_vec3();
    let count = drops(sky);
    if count == 0 {
        return Vec::new();
    }
    let span = CEILING - FLOOR;
    let wind = Vec3::new(sky.wind.0, 0.0, sky.wind.1);
    let velocity = wind - Vec3::Y * FALL_SPEED;
    let along = velocity.normalize_or_zero();
    // The whole sheet shares one orientation, so it is worked out once
    // rather than per streak.
    let lean = Quat::from_rotation_arc(Vec3::NEG_Y, if along == Vec3::ZERO { Vec3::NEG_Y } else { along });
    // A streak's cube is centred across its own width and hangs a full
    // length below its head; the rotation then swings that down-hanging box
    // onto the drop's actual path.
    let shape = Mat4::from_quat(lean)
        * Mat4::from_scale(Vec3::new(THICKNESS, LENGTH, THICKNESS))
        * Mat4::from_translation(Vec3::new(-0.5, -1.0, -0.5));

    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let hash = vx_world::seed::finalise(
            seed ^ (index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x5241_494e_0000_0001,
        );
        let unit = |shift: u32| ((hash >> shift) & 0xffff) as f32 / 65_535.0;
        // Offsets are anchored to the eye, so the sheet travels with you and
        // a streak never has to be born or retired.
        let mut ox = (unit(0) - 0.5) * 2.0 * COLUMN;
        let mut oz = (unit(16) - 0.5) * 2.0 * COLUMN;
        // Pushed out of the lens rather than dropped, so a downpour stays a
        // downpour instead of thinning by however many drops fell near you.
        let reach = (ox * ox + oz * oz).sqrt();
        if reach < NEAR {
            let (dx, dz) = if reach > 1.0e-4 {
                (ox / reach, oz / reach)
            } else {
                (1.0, 0.0)
            };
            ox = dx * NEAR;
            oz = dz * NEAR;
        }
        let phase = unit(32) * span;
        // Modulo the fall span: a drop leaving the bottom is the same drop
        // arriving at the top, which is why nothing is ever allocated.
        let fallen = (phase + seconds * FALL_SPEED).rem_euclid(span);
        let y = eye.y + CEILING - fallen;
        // The wind carries the drop sideways by however long it has been
        // falling, which is what makes the sheet drift as well as slant.
        let carried = wind * (fallen / FALL_SPEED);
        let head = Vec3::new(eye.x + ox + carried.x, y, eye.z + oz + carried.z);
        let mut streak = Object::new(Mat4::from_translation(head) * shape, slot::WATER);
        // Rain is lit by the sky it fell out of, not by the ground under it.
        streak.light = 1.0;
        out.push(streak);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sky(rain: f32, wind: (f32, f32)) -> Conditions {
        Conditions {
            temperature: 12.0,
            humidity: 0.8,
            wind,
            rain,
            state: vx_world::weather::State::Rain,
        }
    }

    /// A clear day costs nothing at all.
    #[test]
    fn dry_weather_draws_no_rain() {
        assert_eq!(drops(&sky(0.0, (0.0, 0.0))), 0);
        assert!(streaks(7, 3.0, glam::DVec3::ZERO, &sky(0.0, (0.0, 0.0))).is_empty());
    }

    /// And a downpour draws more than a shower.
    #[test]
    fn heavier_rain_draws_more() {
        let light = drops(&sky(0.3, (0.0, 0.0)));
        let heavy = drops(&sky(1.0, (0.0, 0.0)));
        assert!(light > 0);
        assert!(heavy > light * 2, "{light} then {heavy}");
        assert!(heavy <= MAX_DROPS);
    }

    /// Same seed, same clock, same rain — the oracle's rule, applied to
    /// something that never touches the hash, because a sheet of rain that
    /// flickered between two machines would still be wrong.
    #[test]
    fn the_same_moment_draws_the_same_rain() {
        let conditions = sky(0.7, (3.0, -2.0));
        let first = streaks(11, 4.25, glam::DVec3::new(8.0, 70.0, -3.0), &conditions);
        let again = streaks(11, 4.25, glam::DVec3::new(8.0, 70.0, -3.0), &conditions);
        assert_eq!(first, again);
        let elsewhen = streaks(11, 4.30, glam::DVec3::new(8.0, 70.0, -3.0), &conditions);
        assert_ne!(first, elsewhen);
    }

    /// The sheet stays around the eye however far out you walk, which is
    /// what lets it be a fixed number of instances rather than a spawner.
    #[test]
    fn the_sheet_travels_with_you() {
        let conditions = sky(1.0, (0.0, 0.0));
        let eye = Vec3::new(2_000.0, 90.0, -1_400.0);
        for streak in streaks(3, 1.0, eye.as_dvec3(), &conditions) {
            let centre = (streak.bounds_min + streak.bounds_max) * 0.5;
            assert!((centre.x - eye.x).abs() <= COLUMN + LENGTH);
            assert!((centre.z - eye.z).abs() <= COLUMN + LENGTH);
            let out = ((centre.x - eye.x).powi(2) + (centre.z - eye.z).powi(2)).sqrt();
            assert!(out >= NEAR - LENGTH, "a streak landed on the lens: {out}");
            assert!(centre.y > eye.y + FLOOR - LENGTH);
            assert!(centre.y < eye.y + CEILING + LENGTH);
        }
    }

    /// Wind slants the weather: with a gale blowing, a streak's head and its
    /// tail are no longer over the same spot.
    #[test]
    fn wind_slants_the_streaks() {
        let eye = Vec3::new(0.0, 64.0, 0.0);
        let calm = streaks(5, 2.0, eye.as_dvec3(), &sky(1.0, (0.0, 0.0)));
        let blown = streaks(5, 2.0, eye.as_dvec3(), &sky(1.0, (13.0, 0.0)));
        let width = |objects: &[Object]| {
            objects
                .iter()
                .map(|object| object.bounds_max.x - object.bounds_min.x)
                .fold(0.0f32, f32::max)
        };
        assert!(width(&calm) < 0.2, "{}", width(&calm));
        assert!(width(&blown) > 0.4, "{}", width(&blown));
    }
}
