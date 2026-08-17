//! Terrain generation.
//!
//! Generation is a pure function of `(seed, chunk position)`. It reads no
//! shared state, so chunks can be generated on a worker pool in any order and
//! a saved world regenerates identically.

use vx_core::{BlockDef, BlockId, BlockRegistry, ChunkPos, LocalPos, CHUNK_HEIGHT, CHUNK_SIZE};

use crate::chunk::Chunk;
use crate::noise::{warp_2d, Fbm, Ridged, Spline};

/// Sea level. Terrain below this floods.
pub const SEA_LEVEL: i32 = 62;

/// Per-field seed offsets. Each noise field must be decorrelated from the
/// others, or "erosion" and "peaks" would be the same landscape twice and the
/// three-field split would buy nothing.
const SEED_EROSION: u64 = 0x00e5_0510_a17f_2b64;
const SEED_PEAKS: u64 = 0x0bea_c521_37d9_11e5;
const SEED_RIDGES: u64 = 0x0d1d_9e17_c40a_88f3;

/// The block ids terrain generation needs, resolved once against the registry
/// so generation never does string lookups per block.
#[derive(Debug, Clone, Copy)]
pub struct TerrainBlocks {
    pub stone: BlockId,
    pub dirt: BlockId,
    pub grass: BlockId,
    pub sand: BlockId,
    pub water: BlockId,
    pub bedrock: BlockId,
}

impl TerrainBlocks {
    /// Register the engine's built-in blocks and return their ids.
    ///
    /// Texture indices are atlas tile slots; the atlas builder generates
    /// matching placeholder tiles until real art exists.
    pub fn register_builtins(registry: &mut BlockRegistry) -> Self {
        let mut register = |def: BlockDef| {
            registry
                .register(def)
                .expect("built-in blocks register exactly once into a fresh registry")
        };

        TerrainBlocks {
            stone: register(BlockDef::uniform("engine:stone", 0)),
            dirt: register(BlockDef::uniform("engine:dirt", 1)),
            grass: register(BlockDef::columnar("engine:grass", 2, 3, 1)),
            sand: register(BlockDef::uniform("engine:sand", 4)),
            water: register(
                BlockDef::uniform("engine:water", 5)
                    .translucent()
                    .non_solid()
                    .with_hardness(None),
            ),
            bedrock: register(BlockDef::uniform("engine:bedrock", 6).with_hardness(None)),
        }
    }

    /// Look up already-registered built-ins, for callers that did not register
    /// them themselves.
    pub fn from_registry(registry: &BlockRegistry) -> Option<Self> {
        Some(TerrainBlocks {
            stone: registry.id_of("engine:stone")?,
            dirt: registry.id_of("engine:dirt")?,
            grass: registry.id_of("engine:grass")?,
            sand: registry.id_of("engine:sand")?,
            water: registry.id_of("engine:water")?,
            bedrock: registry.id_of("engine:bedrock")?,
        })
    }
}

/// Turns a seed into terrain.
///
/// Height comes from three independent low-frequency fields, each mapped through
/// its own [`Spline`], plus ridged detail gated to inland areas.
///
/// The three-field split is what buys separable control: continentalness decides
/// how high the land sits, erosion decides how rugged it is allowed to be, and
/// peaks/valleys supplies the local variation that erosion then scales. Summing
/// octaves into a single number cannot express "high but flat" or "low but
/// jagged"; three fields can.
#[derive(Debug, Clone)]
pub struct TerrainGenerator {
    seed: u64,
    blocks: TerrainBlocks,

    /// Where landmasses are. Lowest frequency of the three.
    continent: Fbm,
    /// How worn down the land is. High erosion flattens.
    erosion: Fbm,
    /// Local rise and fall, scaled by erosion.
    peaks: Fbm,
    /// Mountain ridgelines, only inland.
    ridges: Ridged,

    /// Continentalness to a base height.
    continent_curve: Spline,
    /// Erosion to a ruggedness factor in `[0, 1]`.
    erosion_curve: Spline,
    /// Peaks/valleys to a signed height offset.
    peaks_curve: Spline,
    /// Continentalness to how much ridged mountain height applies.
    ridge_gate: Spline,

    warp_frequency: f32,
    warp_strength: f32,
}

impl TerrainGenerator {
    pub fn new(seed: u64, blocks: TerrainBlocks) -> Self {
        TerrainGenerator {
            seed,
            blocks,

            continent: Fbm {
                octaves: 4,
                persistence: 0.5,
                lacunarity: 2.0,
                frequency: 1.0 / 380.0,
            },
            erosion: Fbm {
                octaves: 3,
                persistence: 0.5,
                lacunarity: 2.0,
                frequency: 1.0 / 240.0,
            },
            peaks: Fbm {
                octaves: 4,
                persistence: 0.5,
                lacunarity: 2.2,
                frequency: 1.0 / 85.0,
            },
            ridges: Ridged {
                octaves: 4,
                persistence: 0.5,
                lacunarity: 2.1,
                frequency: 1.0 / 120.0,
            },

            // Summed octaves cluster hard around 0.5, so the curve spends its
            // detail where the samples actually land. The steep run from 0.42
            // to 0.58 is the coastline-to-upland transition; almost every
            // sample falls in that window, and it is what replaces the old flat
            // terraces with real relief.
            continent_curve: Spline::new(vec![
                (0.00, 24.0),
                (0.32, 38.0),
                (0.42, 52.0),  // continental shelf
                (0.48, 63.0),  // coastline, just above sea level
                (0.56, 84.0),  // lowland rising
                (0.66, 116.0), // uplands
                (0.80, 152.0),
                (1.00, 178.0),
            ]),

            // High erosion means worn flat. Deliberately non-linear: most of
            // the world is moderately eroded and calm, with rugged country
            // confined to the low-erosion tail.
            erosion_curve: Spline::new(vec![
                (0.00, 1.00),
                (0.35, 0.72),
                (0.50, 0.38),
                (0.65, 0.16),
                (1.00, 0.05),
            ]),

            // Signed: below the midpoint carves valleys, above raises peaks.
            peaks_curve: Spline::new(vec![
                (0.00, -34.0),
                (0.35, -12.0),
                (0.50, 0.0),
                (0.65, 18.0),
                (1.00, 52.0),
            ]),

            // Ridges only inland, ramping in past the coast so mountains never
            // erupt straight out of the sea.
            ridge_gate: Spline::new(vec![
                (0.00, 0.0),
                (0.55, 0.0),
                (0.70, 0.55),
                (1.00, 1.0),
            ]),

            // Bends the coordinate space so coastlines and ridges wander
            // instead of following the sample lattice.
            warp_frequency: 1.0 / 150.0,
            warp_strength: 45.0,
        }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn blocks(&self) -> TerrainBlocks {
        self.blocks
    }

    /// Surface height at a world column: the y of its topmost solid block.
    ///
    /// Pure in `(seed, x, z)` — no state, no ordering, so columns can be
    /// evaluated in any order and on any thread.
    pub fn height_at(&self, world_x: i32, world_z: i32) -> i32 {
        // Warp first, so every field downstream samples the bent space and the
        // lattice alignment never shows through.
        let (x, z) = warp_2d(
            self.seed,
            world_x as f32,
            world_z as f32,
            self.warp_frequency,
            self.warp_strength,
        );

        // Independent seed offsets, or the three fields would be identical.
        let continentalness = self.continent.sample(self.seed, x, z);
        let erosion = self.erosion.sample(self.seed ^ SEED_EROSION, x, z);
        let peaks_valleys = self.peaks.sample(self.seed ^ SEED_PEAKS, x, z);

        let base = self.continent_curve.sample(continentalness);
        let ruggedness = self.erosion_curve.sample(erosion);
        let local = self.peaks_curve.sample(peaks_valleys) * ruggedness;

        // Ridges: inland only, and flattened out by erosion like everything
        // else, so a worn-down highland is a plateau rather than a saw blade.
        let gate = self.ridge_gate.sample(continentalness);
        let ridge = if gate > 0.0 {
            self.ridges.sample(self.seed ^ SEED_RIDGES, x, z) * gate * ruggedness * 46.0
        } else {
            0.0
        };

        let height = base + local + ridge;
        (height as i32).clamp(1, CHUNK_HEIGHT - 1)
    }

    /// Generate the chunk at `pos`.
    pub fn generate(&self, pos: ChunkPos) -> Chunk {
        let mut chunk = Chunk::empty(pos);
        let origin = pos.origin();

        for local_z in 0..CHUNK_SIZE {
            for local_x in 0..CHUNK_SIZE {
                let world_x = origin.x + local_x;
                let world_z = origin.z + local_z;
                let surface = self.height_at(world_x, world_z);
                self.fill_column(&mut chunk, local_x, local_z, surface);
            }
        }

        // Generation touches the palette heavily; compact before it is cached.
        chunk.optimise();
        chunk.clear_dirty();
        // Freshly generated terrain matches the seed, so there is nothing to
        // save until a player changes it.
        chunk.clear_modified();
        chunk
    }

    /// Lay down the vertical stack for one column.
    fn fill_column(&self, chunk: &mut Chunk, x: i32, z: i32, surface: i32) {
        let blocks = self.blocks;

        // Beaches: columns near sea level get sand instead of grass.
        let coastal = surface <= SEA_LEVEL + 1;
        let top = if coastal { blocks.sand } else { blocks.grass };
        let subsoil = if coastal { blocks.sand } else { blocks.dirt };

        chunk.fill_column(x, z, 0, 1, blocks.bedrock);

        let soil_depth = 4;
        let stone_top = (surface - soil_depth).max(1);
        chunk.fill_column(x, z, 1, stone_top, blocks.stone);
        chunk.fill_column(x, z, stone_top, surface, subsoil);

        if let Some(local) = LocalPos::new(x, surface, z) {
            chunk.set(local, top);
        }

        // Flood anything below sea level.
        if surface < SEA_LEVEL {
            chunk.fill_column(x, z, surface + 1, SEA_LEVEL + 1, blocks.water);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::BlockRegistry;

    fn generator(seed: u64) -> (BlockRegistry, TerrainGenerator) {
        let mut registry = BlockRegistry::new();
        let blocks = TerrainBlocks::register_builtins(&mut registry);
        (registry, TerrainGenerator::new(seed, blocks))
    }

    #[test]
    fn builtins_register_and_resolve_by_name() {
        let (registry, generator) = generator(1);
        let looked_up = TerrainBlocks::from_registry(&registry).unwrap();

        assert_eq!(looked_up.stone, generator.blocks().stone);
        assert_eq!(looked_up.water, generator.blocks().water);
        assert_eq!(registry.get(looked_up.stone).unwrap().name, "engine:stone");

        // Water is see-through and passable; stone is neither.
        assert!(!registry.is_opaque(looked_up.water));
        assert!(!registry.is_solid(looked_up.water));
        assert!(registry.is_opaque(looked_up.stone));
        assert!(registry.is_solid(looked_up.stone));
    }

    #[test]
    fn the_same_seed_generates_identical_chunks() {
        let (_, a) = generator(12345);
        let (_, b) = generator(12345);
        let pos = ChunkPos::new(3, -7);
        assert_eq!(a.generate(pos), b.generate(pos));
    }

    #[test]
    fn different_seeds_generate_different_terrain() {
        let (_, a) = generator(1);
        let (_, b) = generator(2);
        let pos = ChunkPos::new(0, 0);
        assert_ne!(a.generate(pos), b.generate(pos));
    }

    #[test]
    fn generated_chunks_are_not_empty_and_have_bedrock_floors() {
        let (_, generator) = generator(99);
        let chunk = generator.generate(ChunkPos::new(0, 0));

        assert!(!chunk.is_empty());
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let floor = LocalPos::new(x, 0, z).unwrap();
                assert_eq!(
                    chunk.get(floor),
                    generator.blocks().bedrock,
                    "column ({x},{z}) is missing its bedrock floor"
                );
            }
        }
    }

    #[test]
    fn terrain_height_stays_inside_the_world() {
        let (_, generator) = generator(7);
        for x in -300..300 {
            let height = generator.height_at(x, x * 3);
            assert!(
                (1..CHUNK_HEIGHT).contains(&height),
                "height {height} at x={x} escaped the world"
            );
        }
    }

    #[test]
    fn chunk_contents_match_the_height_field() {
        // The column filler and the height function must agree, or the surface
        // ends up buried or floating.
        let (_, generator) = generator(2024);
        let pos = ChunkPos::new(-2, 5);
        let chunk = generator.generate(pos);
        let origin = pos.origin();

        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let expected = generator.height_at(origin.x + x, origin.z + z);
                let solid_top = (0..CHUNK_HEIGHT)
                    .rev()
                    .find(|&y| {
                        let block = chunk.get(LocalPos::new(x, y, z).unwrap());
                        !block.is_air() && block != generator.blocks().water
                    })
                    .unwrap();
                assert_eq!(solid_top, expected, "surface mismatch at ({x},{z})");
            }
        }
    }

    #[test]
    fn water_fills_to_sea_level_and_no_higher() {
        let (_, generator) = generator(31337);
        let water = generator.blocks().water;

        // Find an ocean column via the (cheap) height field first. Sampling a
        // handful of adjacent chunks is not enough: the continent noise has a
        // ~220 block wavelength, so any smaller window can sit entirely on one
        // landmass and see no water at all.
        let ocean = (-40..40)
            .flat_map(|cx| (-40..40).map(move |cz| ChunkPos::new(cx, cz)))
            .find(|pos| {
                let origin = pos.origin();
                generator.height_at(origin.x, origin.z) < SEA_LEVEL
            })
            .expect("no ocean anywhere in 80x80 chunks; check the height range");

        let chunk = generator.generate(ocean);
        let mut saw_water = false;
        for (local, block) in chunk.iter_blocks() {
            if block == water {
                saw_water = true;
                assert!(
                    local.y() <= SEA_LEVEL,
                    "water at y={} is above sea level",
                    local.y()
                );
            }
        }
        assert!(saw_water, "chunk below sea level generated without water");
    }

    #[test]
    fn the_world_has_both_land_and_ocean() {
        // Guards the shaping constants: it is easy to end up with a world that
        // is entirely ocean or entirely plateau, and either only shows up
        // visually.
        let (_, generator) = generator(2024);

        let mut below = 0;
        let mut total = 0;
        for x in (-1400..1400).step_by(37) {
            for z in (-1400..1400).step_by(37) {
                let height = generator.height_at(x, z);
                assert!((1..CHUNK_HEIGHT).contains(&height));
                below += i32::from(height < SEA_LEVEL);
                total += 1;
            }
        }

        let ocean_fraction = below as f32 / total as f32;
        assert!(
            (0.05..0.60).contains(&ocean_fraction),
            "ocean coverage {ocean_fraction:.2} is outside a playable range"
        );
    }

    #[test]
    fn adjacent_chunks_line_up_across_their_shared_edge() {
        // Generation is per-chunk, so a seam is only correct if the height
        // field is continuous in world space. This is the test that catches
        // using local instead of world coordinates in the noise lookup.
        let (_, generator) = generator(555);
        let left = generator.generate(ChunkPos::new(0, 0));
        let right = generator.generate(ChunkPos::new(1, 0));

        for z in 0..CHUNK_SIZE {
            let left_edge = left.height_at(CHUNK_SIZE - 1, z).unwrap();
            let right_edge = right.height_at(0, z).unwrap();
            let expected_left = generator.height_at(CHUNK_SIZE - 1, z);
            let expected_right = generator.height_at(CHUNK_SIZE, z);

            // `Chunk::height_at` reports the topmost non-air block, and a
            // column below sea level is flooded to exactly SEA_LEVEL.
            assert_eq!(left_edge, expected_left.max(SEA_LEVEL));
            assert_eq!(right_edge, expected_right.max(SEA_LEVEL));
            assert!(
                (expected_left - expected_right).abs() <= 4,
                "seam jump of {} blocks at z={z}",
                (expected_left - expected_right).abs()
            );
        }
    }

    #[test]
    fn terrain_has_real_relief_rather_than_flat_terraces() {
        // The regression this milestone exists to prevent, stated numerically.
        // Summed octaves alone gave ~22 blocks of spread and read as a paved
        // car park; the spline stack should give well over 60.
        let (_, generator) = generator(2024);

        let mut heights: Vec<i32> = Vec::new();
        for x in (-3000..3000).step_by(29) {
            for z in (-3000..3000).step_by(29) {
                heights.push(generator.height_at(x, z));
            }
        }
        heights.sort_unstable();

        let n = heights.len();
        let p05 = heights[n * 5 / 100];
        let p95 = heights[n * 95 / 100];
        let spread = p95 - p05;

        assert!(
            spread > 60,
            "only {spread} blocks between the 5th and 95th percentile ({p05}..{p95}); \
             terrain has flattened back out"
        );
        // And it should use the vertical space without slamming into the roof.
        assert!(heights[0] < SEA_LEVEL, "no terrain below sea level at all");
        assert!(
            heights[n - 1] < CHUNK_HEIGHT - 20,
            "terrain is pressed against the world ceiling at {}",
            heights[n - 1]
        );
    }

    #[test]
    fn terrain_is_walkable_rather_than_a_staircase_of_cliffs() {
        // Relief is worthless if every column is a sheer wall. Adjacent columns
        // should mostly differ by a step the player can walk up.
        let (_, generator) = generator(99);

        let mut steep = 0;
        let mut total = 0;
        for x in -1500..1500 {
            let delta = (generator.height_at(x + 1, 7) - generator.height_at(x, 7)).abs();
            if delta > 2 {
                steep += 1;
            }
            total += 1;
        }

        let steep_fraction = steep as f32 / total as f32;
        assert!(
            steep_fraction < 0.05,
            "{:.1}% of columns are unclimbable steps",
            steep_fraction * 100.0
        );
    }

    #[test]
    fn domain_warping_breaks_axis_alignment() {
        // Without warping, terrain features line up with the sample lattice and
        // the world reads as a grid. Compare variation along an axis against
        // variation along a diagonal: neither should be conspicuously flatter.
        let (_, generator) = generator(404);

        let variation = |step: (i32, i32)| -> f64 {
            let samples: Vec<f64> = (0..600)
                .map(|i| generator.height_at(i * step.0, i * step.1) as f64)
                .collect();
            let mean = samples.iter().sum::<f64>() / samples.len() as f64;
            (samples.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
        };

        let along_axis = variation((1, 0));
        let along_diagonal = variation((1, 1));

        assert!(along_axis > 5.0, "terrain barely varies along an axis");
        assert!(along_diagonal > 5.0, "terrain barely varies diagonally");
    }

    #[test]
    fn generated_chunks_come_back_clean_and_compacted() {
        let (_, generator) = generator(4);
        let chunk = generator.generate(ChunkPos::new(0, 0));

        assert!(!chunk.is_dirty(), "a freshly generated chunk needs no remesh flag");
        // Only the handful of terrain blocks should survive palette compaction.
        assert!(
            chunk.storage().palette_len() <= 8,
            "palette not compacted: {} entries",
            chunk.storage().palette_len()
        );
    }
}



