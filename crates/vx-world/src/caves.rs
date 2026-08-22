//! Caves: the first genuinely 3D shape in a height-field world.
//!
//! # Pure in `(seed, x, y, z)`
//!
//! Terrain fills columns; a cave is a hole *through* columns. The one way to
//! carve holes without breaking chunk-parallel generation is a field that any
//! chunk can evaluate for any block with no cross-chunk context — the same
//! contract ore deposits and towns already honour. [`carved`] is that field:
//! a pure function, so two chunks meeting at a cave wall agree about every
//! block of it, and so anything else in the engine (flora, capture framing,
//! tests) can ask "is this rock or void?" without generating a chunk.
//!
//! # The shape
//!
//! Two independent signed 3D noise fields, carved where both are near zero at
//! once: each field's zero set is a surface, and the intersection of two
//! near-zero shells is a long winding *tube* — tunnels, not bubbles. A third,
//! slower field opens occasional chambers where it peaks, but only well below
//! the surface where a room-sized void cannot crater a hillside. Tunnel girth
//! tapers toward the surface, so mouths exist — a cave you cannot find by
//! walking is landscape the player never gets — but are rare enough to stay
//! landmarks.
//!
//! # What is never carved
//!
//! Town footprints (the caller masks them — a plaza must not open into a
//! void), the bedrock floor and a margin above it, and the top of any column
//! at or below the waterline: there is no fluid simulation, so a mouth under
//! the sea would be a hole the ocean visibly fails to pour into.

use crate::gen::SEA_LEVEL;
use crate::noise::signed_3d;

/// Per-field seed salts, decorrelated like the terrain's field seeds.
const SEED_TUNNEL_A: u64 = 0x00ca_5e5a_11b0_7e21;
const SEED_TUNNEL_B: u64 = 0x00ca_5e5b_92c4_d1f7;
const SEED_CHAMBER: u64 = 0x00ca_5ec4_a3b8_650d;

/// Blocks per tunnel-noise lattice cell. Smaller is twistier.
const TUNNEL_SCALE: f32 = 18.0;

/// Vertical squash: y varies this much faster than x/z, so tunnels run wide
/// and low — walkable galleries rather than chimneys.
const TUNNEL_SQUASH: f32 = 1.8;

/// How near zero both tunnel fields must sit for the rock to open. This is
/// the tunnels' girth, in noise units.
const TUNNEL_RADIUS: f32 = 0.085;

/// Blocks per chamber-noise lattice cell.
const CHAMBER_SCALE: f32 = 30.0;

/// The chamber field's threshold; it peaks this high only in rare blobs.
const CHAMBER_LEVEL: f32 = 0.62;

/// No chamber shallower than this below the surface. A tunnel mouth is a
/// doorway; a chamber breaking the surface would be a crater.
const CHAMBER_COVER: i32 = 16;

/// Depth at which tunnels reach full girth. Above it they thin toward
/// [`MOUTH_FACTOR`], which is what keeps surface mouths scarce.
const FULL_GIRTH_DEPTH: f32 = 16.0;

/// Tunnel girth right at the surface, as a fraction of full girth.
const MOUTH_FACTOR: f32 = 0.45;

/// Nothing is carved at or below this height: the world keeps its bottom,
/// with rock to spare under the deepest gallery.
pub const CAVE_FLOOR: i32 = 6;

/// Columns ending at or below the waterline keep this many blocks of roof, so
/// no cave opens into standing water.
const SEA_BED_COVER: i32 = 4;

/// Is the block at `(x, y, z)` hollowed out of a column whose surface sits at
/// `surface`? Pure in `(seed, position, surface)`; the caller supplies the
/// same blended surface height the column was filled against, which is what
/// keeps every consumer of the field agreeing with the terrain it carved.
pub fn carved(seed: u64, x: i32, y: i32, z: i32, surface: i32) -> bool {
    if y <= CAVE_FLOOR || y > surface {
        return false;
    }
    let depth = surface - y;
    if surface <= SEA_LEVEL + 1 && depth < SEA_BED_COVER {
        return false;
    }

    let (fx, fy, fz) = (
        x as f32 / TUNNEL_SCALE,
        y as f32 * TUNNEL_SQUASH / TUNNEL_SCALE,
        z as f32 / TUNNEL_SCALE,
    );
    let girth = TUNNEL_RADIUS
        * (depth as f32 / FULL_GIRTH_DEPTH).clamp(MOUTH_FACTOR, 1.0);
    let a = signed_3d(seed ^ SEED_TUNNEL_A, fx, fy, fz);
    let b = signed_3d(seed ^ SEED_TUNNEL_B, fx, fy, fz);
    if a * a + b * b < girth * girth {
        return true;
    }

    depth >= CHAMBER_COVER
        && signed_3d(
            seed ^ SEED_CHAMBER,
            x as f32 / CHAMBER_SCALE,
            y as f32 / CHAMBER_SCALE,
            z as f32 / CHAMBER_SCALE,
        ) > CHAMBER_LEVEL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_field_is_deterministic_and_positional() {
        let mut hollow = 0u32;
        for x in -60..60 {
            for y in CAVE_FLOOR + 1..70 {
                let now = carved(9, x, y, 17, 72);
                assert_eq!(now, carved(9, x, y, 17, 72), "same question, two answers");
                hollow += u32::from(now);
            }
        }
        assert!(hollow > 0, "no cave anywhere in a 120-column slice");
    }

    #[test]
    fn the_floor_holds_and_nothing_carves_above_ground() {
        for x in -200..200 {
            for y in 0..=CAVE_FLOOR {
                assert!(!carved(9, x, y, 5, 80), "carved through the floor at y={y}");
            }
            for y in 81..90 {
                assert!(!carved(9, x, y, 5, 80), "carved open air at y={y}");
            }
        }
    }

    #[test]
    fn the_sea_bed_keeps_its_roof() {
        // A column ending under the waterline never opens in its top blocks,
        // whatever the noise wants.
        let surface = SEA_LEVEL - 6;
        for x in -300..300 {
            for z in -3..3 {
                for depth in 0..SEA_BED_COVER {
                    assert!(
                        !carved(9, x, surface - depth, z, surface),
                        "a cave mouth under the sea at ({x}, {z}), depth {depth}"
                    );
                }
            }
        }
    }

    #[test]
    fn tunnels_thin_toward_the_surface() {
        // Count carved blocks in a deep band and a shallow band of the same
        // volume: the taper must make shallow rock tighter than deep rock.
        let surface = 120;
        let mut deep = 0u32;
        let mut shallow = 0u32;
        for x in -80..80 {
            for z in -80..80 {
                for slice in 0..6 {
                    deep += u32::from(carved(9, x, surface - 40 - slice, z, surface));
                    shallow += u32::from(carved(9, x, surface - 1 - slice, z, surface));
                }
            }
        }
        assert!(
            shallow < deep / 2,
            "shallow rock ({shallow}) is not meaningfully tighter than deep rock ({deep})"
        );
        assert!(deep > 0, "no caves in the deep band at all");
    }
}
