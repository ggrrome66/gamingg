//! Which forest stands where.
//!
//! Two fields decide it, and both are read off the terrain the generator
//! already produces: how high a column is, and how wet it is. Neither is
//! stored and neither depends on a neighbour being generated first — the
//! same discipline as [`crate::ore`] and [`crate::flora`], because a forest
//! that disagreed with itself across a chunk border would show as a seam in
//! the canopy.
//!
//! **Wetness is local on purpose.** The hydrologist's topographic wetness
//! index is `ln(a / tan β)`, where `a` is the upslope area draining through a
//! point — which needs flow accumulated over the whole catchment, and a
//! non-local field cannot be a pure function of a column. What survives the
//! restriction is the shape of it: water gathers where the ground is *flat*
//! and *convergent*, so curvature over slope says most of what the real index
//! says, and it says it from five height samples.
//!
//! The bands are deliberately not contour lines. A low-frequency noise field
//! wobbles the elevation thresholds by a few blocks, so the treeline wanders
//! the way a real one does instead of tracing a level curve around a hill.

use crate::noise::signed_2d;

/// How far apart the samples that give slope and curvature are taken, in
/// blocks. Close enough to read a stream bank; wide enough that a single
/// block of terrain noise does not read as a basin.
pub const SAMPLE_STEP: i32 = 4;

/// Below this the ground is too low and too wet for the hardwoods, if it is
/// also flat and convergent enough to hold water.
const BOG_MAX_Y: i32 = 104;
/// A bog is flat. Anything steeper drains.
const BOG_MAX_SLOPE: f32 = 0.50;
/// And convergent: this is the curvature-over-slope proxy, plus the water
/// table, above which a hollow holds peat.
const BOG_WETNESS: f32 = 0.20;

/// Where the hardwoods give out and the conifers take over.
const SUBALPINE_Y: i32 = 116;
/// Where an upright conifer gives out and only krummholz survives.
pub const TREELINE_Y: i32 = 150;
/// And where even the mats stop.
pub const TREE_LIMIT_Y: i32 = 178;

/// Cold air drains downhill and pools in hollows, so a deep enough valley is
/// colder than its height says and grows conifers below their own band. This
/// is how many blocks of elevation a fully convergent hollow is worth.
const COLD_POCKET_GAIN: f32 = 26.0;

/// The ecotone: how far the elevation thresholds wander, in blocks, and how
/// coarse that wander is.
const ECOTONE_BAND: f32 = 7.0;
const ECOTONE_SCALE: f32 = 1.0 / 90.0;

/// The water table under everything: a slow field that makes some country
/// wetter than its shape alone would.
const TABLE_SCALE: f32 = 1.0 / 220.0;
const TABLE_WEIGHT: f32 = 0.30;

const SALT_ECOTONE: u64 = 0x0b17_5eed_0f0f_1234;
const SALT_TABLE: u64 = 0x00_a7_e4_b1_00_5e_ed_11;

/// The three forests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Biome {
    /// Black spruce over peat in the flat wet lows: thin crooked stems, moss
    /// underfoot, standing water where the ground dips.
    Bog,
    /// The mixed deciduous cove forest of the middle elevations: broad
    /// crowns, the odd emergent giant, gaps where one has come down.
    Hardwood,
    /// Subalpine conifer up high — narrow spires, thinning with height, and
    /// knee-high krummholz where nothing can stand up to the wind.
    Subalpine,
}

/// A column's terrain, in the terms the forest cares about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ground {
    /// The natural surface height. Never the town-blended one: a plateau
    /// bulldozed for a market square must not decide what grows around it.
    pub height: i32,
    /// Magnitude of the height gradient, in blocks per block.
    pub slope: f32,
    /// Curvature over slope, plus the water table. High in flat hollows.
    pub wetness: f32,
    /// How convergent the ground is on its own, 0 on a plane and rising in a
    /// hollow. Cold air pools here.
    pub hollow: f32,
}

/// Read the ground at a column.
///
/// `height_at` must be the *natural* height field, and pure — every caller
/// that asks about this column has to get the same answer or the forest
/// changes shape depending on who generated the chunk first.
pub fn survey(seed: u64, x: i32, z: i32, height_at: &impl Fn(i32, i32) -> i32) -> Ground {
    let step = SAMPLE_STEP;
    let here = height_at(x, z);
    let west = height_at(x - step, z) as f32;
    let east = height_at(x + step, z) as f32;
    let north = height_at(x, z - step) as f32;
    let south = height_at(x, z + step) as f32;
    let centre = here as f32;

    let span = (2 * step) as f32;
    let grad_x = (east - west) / span;
    let grad_z = (south - north) / span;
    let slope = (grad_x * grad_x + grad_z * grad_z).sqrt();

    // The discrete Laplacian: positive where the ground sits below the
    // country around it — a hollow — and negative on a nose. Scaled by the
    // sample spacing so the number means the same thing whatever the step is.
    let curvature = (west + east + north + south - 4.0 * centre) / (step * step) as f32;
    let hollow = curvature.max(0.0);

    let table = signed_2d(
        seed ^ SALT_TABLE,
        x as f32 * TABLE_SCALE,
        z as f32 * TABLE_SCALE,
    ) * TABLE_WEIGHT;

    Ground {
        height: here,
        slope,
        wetness: hollow / (slope + 0.25) + table,
        hollow,
    }
}

/// Which forest a surveyed column grows.
pub fn classify(seed: u64, x: i32, z: i32, ground: Ground) -> Biome {
    // The bands wander rather than tracing a contour.
    let wander = signed_2d(
        seed ^ SALT_ECOTONE,
        x as f32 * ECOTONE_SCALE,
        z as f32 * ECOTONE_SCALE,
    ) * ECOTONE_BAND;

    // Flat, low and convergent: peat. Asked first, because a cold hollow is
    // also a hollow and the wet answer is the one you can see.
    if ground.height as f32 <= BOG_MAX_Y as f32 + wander
        && ground.slope <= BOG_MAX_SLOPE
        && ground.wetness >= BOG_WETNESS
    {
        return Biome::Bog;
    }

    // Cold air pools, so a deep hollow counts as higher ground than it is —
    // which is why spruce fingers down the drainages into hardwood country.
    let effective = ground.height as f32 + (ground.hollow.min(1.0) * COLD_POCKET_GAIN) + wander;
    if effective >= SUBALPINE_Y as f32 {
        Biome::Subalpine
    } else {
        Biome::Hardwood
    }
}

/// Survey a column and classify it in one call.
pub fn biome_at(seed: u64, x: i32, z: i32, height_at: &impl Fn(i32, i32) -> i32) -> Biome {
    classify(seed, x, z, survey(seed, x, z, height_at))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generator(seed: u64) -> crate::gen::TerrainGenerator {
        let mut registry = vx_core::BlockRegistry::new();
        let _ = crate::gen::TerrainBlocks::register_builtins(&mut registry);
        let blocks = crate::gen::TerrainBlocks::from_registry(&registry).unwrap();
        crate::gen::TerrainGenerator::new(seed, blocks)
    }

    /// Walk a wide square of real terrain, counting what grows where.
    fn census(seed: u64) -> (usize, usize, usize, usize) {
        let gen = generator(seed);
        let height = |x: i32, z: i32| gen.natural_height_at(x, z);
        let (mut bog, mut hardwood, mut subalpine, mut land) = (0, 0, 0, 0);
        for x in (-2400..2400).step_by(29) {
            for z in (-2400..2400).step_by(29) {
                let ground = survey(seed, x, z, &height);
                if ground.height <= crate::gen::SEA_LEVEL + 1 {
                    continue;
                }
                land += 1;
                match classify(seed, x, z, ground) {
                    Biome::Bog => bog += 1,
                    Biome::Hardwood => hardwood += 1,
                    Biome::Subalpine => subalpine += 1,
                }
            }
        }
        (land, bog, hardwood, subalpine)
    }

    #[test]
    fn the_answer_is_the_same_every_time_it_is_asked() {
        let gen = generator(11);
        let height = |x: i32, z: i32| gen.natural_height_at(x, z);
        for (x, z) in [(0, 0), (137, -412), (-2001, 883), (54_321, 12_345)] {
            let first = survey(11, x, z, &height);
            assert_eq!(first, survey(11, x, z, &height), "the ground moved");
            assert_eq!(
                classify(11, x, z, first),
                biome_at(11, x, z, &height),
                "two ways of asking, two answers"
            );
        }
    }

    #[test]
    fn all_three_forests_exist_and_none_of_them_swallows_the_map() {
        let (land, bog, hardwood, subalpine) = census(7);
        assert!(land > 10_000, "not enough land sampled: {land}");
        let share = |count: usize| 100.0 * count as f32 / land as f32;
        assert!(
            (2.0..20.0).contains(&share(bog)),
            "bog is {:.1}% of the land",
            share(bog)
        );
        assert!(
            (40.0..80.0).contains(&share(hardwood)),
            "hardwood is {:.1}% of the land",
            share(hardwood)
        );
        assert!(
            (12.0..45.0).contains(&share(subalpine)),
            "subalpine is {:.1}% of the land",
            share(subalpine)
        );
    }

    #[test]
    fn another_seed_grows_another_country_but_the_same_three_forests() {
        let (land, bog, hardwood, subalpine) = census(4242);
        assert!(bog > 0 && hardwood > 0 && subalpine > 0, "a seed missing a forest");
        assert!(hardwood > land / 4, "the middle country all but vanished");
    }

    #[test]
    fn a_bog_is_low_flat_and_convergent() {
        let gen = generator(7);
        let height = |x: i32, z: i32| gen.natural_height_at(x, z);
        let mut checked = 0;
        for x in (-1500..1500).step_by(23) {
            for z in (-1500..1500).step_by(23) {
                let ground = survey(7, x, z, &height);
                if classify(7, x, z, ground) != Biome::Bog {
                    continue;
                }
                checked += 1;
                assert!(
                    ground.slope <= BOG_MAX_SLOPE,
                    "peat on a hillside: slope {:.2} at {x},{z}",
                    ground.slope
                );
                assert!(
                    ground.wetness >= BOG_WETNESS,
                    "a dry bog at {x},{z}: {:.2}",
                    ground.wetness
                );
                assert!(
                    ground.height <= BOG_MAX_Y + ECOTONE_BAND as i32,
                    "a bog above the band at {x},{z}: {}",
                    ground.height
                );
            }
        }
        assert!(checked > 20, "only {checked} bog columns to check");
    }

    #[test]
    fn cold_hollows_grow_conifers_below_their_own_band() {
        // A plane well below the subalpine line is not high country.
        let plane = |_: i32, _: i32| SUBALPINE_Y - 18;
        assert_ne!(biome_at(3, 512, 512, &plane), Biome::Subalpine);

        // The same elevation, but cut by a steep-sided draw running
        // downhill: cold air pools in it and the conifers finger down. It
        // drains too fast to be peat — this is a frost pocket, not a bog.
        let draw = |x: i32, z: i32| {
            let across = (x - 512).abs().min(9);
            SUBALPINE_Y - 18 + across * 3 - (z - 512)
        };
        assert_eq!(biome_at(3, 512, 512, &draw), Biome::Subalpine);
    }

    #[test]
    fn the_treeline_wanders_instead_of_tracing_a_contour() {
        // Held at one height, the band still changes its mind across the
        // map, because the thresholds ride a low-frequency field. A treeline
        // that was a level curve would look drawn on.
        let level = |_: i32, _: i32| SUBALPINE_Y - 3;
        let mut hardwood = 0;
        let mut subalpine = 0;
        for x in (-3000..3000).step_by(37) {
            match biome_at(9, x, 640, &level) {
                Biome::Subalpine => subalpine += 1,
                _ => hardwood += 1,
            }
        }
        assert!(
            subalpine > 20 && hardwood > 20,
            "the band is a contour line: {subalpine} high, {hardwood} low"
        );
    }
}
