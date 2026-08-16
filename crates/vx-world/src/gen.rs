//! Terrain generation.
//!
//! Generation is a pure function of `(seed, chunk position)`. It reads no
//! shared state, so chunks can be generated on a worker pool in any order and
//! a saved world regenerates identically.

use vx_core::{BlockDef, BlockId, BlockRegistry, ChunkPos, LocalPos, CHUNK_HEIGHT, CHUNK_SIZE};

use crate::chunk::Chunk;
use crate::noise::{contrast, ridged, Fbm};

/// Sea level. Terrain below this floods.
pub const SEA_LEVEL: i32 = 62;

/// Identifies the shape of the world this build produces.
///
/// Saves only store the chunks somebody modified; everything else is
/// regenerated on demand. So changing terrain generation silently rewrites the
/// untouched parts of every existing world, leaving cliffs where old edits meet
/// new ground. Bumping this on any generation change lets a world record which
/// version shaped it, so the mismatch is reported rather than discovered as a
/// seam through somebody's house.
pub const GENERATOR_VERSION: u32 = 2;

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
    pub lamp: BlockId,
    pub coal_ore: BlockId,
    pub iron_ore: BlockId,
    pub gold_ore: BlockId,
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
            // Hardness finally matters: soil digs fast, rock slow, ore slower.
            stone: register(BlockDef::uniform("engine:stone", 0).with_hardness(Some(1.5))),
            dirt: register(BlockDef::uniform("engine:dirt", 1).with_hardness(Some(0.5))),
            grass: register(BlockDef::columnar("engine:grass", 2, 3, 1).with_hardness(Some(0.6))),
            sand: register(BlockDef::uniform("engine:sand", 4).falling().with_hardness(Some(0.5))),
            water: register(
                BlockDef::uniform("engine:water", 5)
                    .translucent()
                    .non_solid()
                    .with_hardness(None),
            ),
            bedrock: register(BlockDef::uniform("engine:bedrock", 6).with_hardness(None)),
            // Nothing generates lamps; they exist to be placed, and to give
            // the block-light channel a real source to propagate from.
            lamp: register(BlockDef::uniform("engine:lamp", 7).emitting(14)),
            coal_ore: register(BlockDef::uniform("engine:coal_ore", 8).with_hardness(Some(2.0))),
            iron_ore: register(BlockDef::uniform("engine:iron_ore", 9).with_hardness(Some(3.0))),
            gold_ore: register(BlockDef::uniform("engine:gold_ore", 10).with_hardness(Some(3.0))),
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
            lamp: registry.id_of("engine:lamp")?,
            coal_ore: registry.id_of("engine:coal_ore")?,
            iron_ore: registry.id_of("engine:iron_ore")?,
            gold_ore: registry.id_of("engine:gold_ore")?,
        })
    }
}

/// Turns a seed into terrain.
#[derive(Debug, Clone)]
pub struct TerrainGenerator {
    seed: u64,
    blocks: TerrainBlocks,
    /// Broad landmass shape.
    continent: Fbm,
    /// Local roughness, layered on top.
    detail: Fbm,
    /// Ridged noise picking out mountain ranges.
    peaks: Fbm,
    /// Carves caves. Three-dimensional, because a heightmap cannot express an
    /// overhang, let alone a tunnel.
    caves: Fbm,
    /// Scatters ore through stone.
    ores: Fbm,
    /// Lowest terrain height, at continent noise 0.
    base_height: i32,
    /// Height range added by the noise stack.
    height_range: i32,
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
                frequency: 1.0 / 220.0,
            },
            detail: Fbm {
                octaves: 3,
                persistence: 0.45,
                lacunarity: 2.3,
                frequency: 1.0 / 48.0,
            },
            peaks: Fbm {
                octaves: 3,
                persistence: 0.5,
                lacunarity: 2.1,
                frequency: 1.0 / 150.0,
            },
            caves: Fbm {
                octaves: 2,
                persistence: 0.5,
                lacunarity: 2.0,
                frequency: 1.0 / 34.0,
            },
            ores: Fbm {
                octaves: 1,
                persistence: 0.5,
                lacunarity: 2.0,
                frequency: 1.0 / 9.0,
            },
            base_height: 30,
            height_range: 96,
        }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn blocks(&self) -> TerrainBlocks {
        self.blocks
    }

    /// Surface height at a world column: the y of its topmost solid block.
    pub fn height_at(&self, world_x: i32, world_z: i32) -> i32 {
        let (x, z) = (world_x as f32, world_z as f32);

        // Raw fbm clusters hard around its mean, which is what made the old
        // terrain a set of broad terraces spanning barely twenty blocks.
        // Shaping pushes lowland down and highland up so the same noise covers
        // the whole height range — but only gently: a strong curve flattens the
        // middle of the distribution into literal plateaus joined by cliffs,
        // which trades one kind of terracing for a worse-looking one.
        let continent = contrast(self.continent.sample(self.seed, x, z), 1.35);

        // Detail is weighted by the continent value so lowlands stay flat and
        // highlands get rugged, instead of uniform noise everywhere.
        let detail = self.detail.sample(self.seed ^ 0xa5a5, x, z);

        // Ridges, applied only where the land is already high, so mountains
        // rise out of highland rather than erupting from the sea.
        let ridge = ridged(self.peaks.sample(self.seed ^ 0x5eed, x, z));
        let mountain = ridge * ridge * continent * continent;

        let combined =
            (continent * 0.52 + detail * 0.24 * continent + mountain * 0.34).clamp(0.0, 1.0);

        let height = self.base_height as f32 + combined * self.height_range as f32;
        (height as i32).clamp(1, CHUNK_HEIGHT - 1)
    }

    /// True when this block should be hollowed out into a cave.
    ///
    /// Two independent fields, each folded about its midpoint so cheap value
    /// noise yields winding tunnels rather than blobs. Requiring both to be
    /// near their fold at once is what makes the result connected passages
    /// instead of isolated bubbles.
    pub fn is_cave(&self, x: i32, y: i32, z: i32) -> bool {
        // Never breach the bedrock floor or the top of the world.
        if y <= 1 || y >= CHUNK_HEIGHT - 1 {
            return false;
        }

        let (fx, fy, fz) = (x as f32, y as f32, z as f32);
        // Squashed vertically, so passages run along rather than as chimneys.
        let first = ridged(self.caves.sample_3d(self.seed ^ 0xcafe, fx, fy * 1.9, fz));
        let second = ridged(self.caves.sample_3d(self.seed ^ 0xbeef, fx, fy * 1.9, fz));

        // The threshold is high because fbm output clusters hard around its
        // mean and `ridged` peaks exactly there, so a value that looks
        // selective is not: at 0.88 this carved a sixth of the underground
        // into swiss cheese, which shredded the greedy mesher.
        let depth = ((SEA_LEVEL - y) as f32 / 48.0).clamp(0.0, 1.0);
        let threshold = 0.962 - 0.02 * depth;

        first > threshold && second > threshold
    }

    /// Which ore, if any, belongs in the stone at this position.
    ///
    /// Depth bands rather than uniform scatter: something has to reward
    /// digging past the first cave you fall into.
    pub fn ore_at(&self, x: i32, y: i32, z: i32) -> Option<BlockId> {
        let (fx, fy, fz) = (x as f32, y as f32, z as f32);
        let sample = self.ores.sample_3d(self.seed ^ 0x0_1e5, fx, fy, fz);

        // Rarer and richer with depth.
        if y < 24 && sample > 0.955 {
            Some(self.blocks.gold_ore)
        } else if y < 52 && sample > 0.930 {
            Some(self.blocks.iron_ore)
        } else if y < 96 && sample > 0.905 {
            Some(self.blocks.coal_ore)
        } else {
            None
        }
    }

    /// Generate the chunk at `pos`.
    pub fn generate(&self, pos: ChunkPos) -> Chunk {
        let mut chunk = Chunk::empty(pos);
        let origin = pos.origin();

        for local_z in 0..CHUNK_SIZE {
            for local_x in 0..CHUNK_SIZE {
                let world_x = origin.x.saturating_add(local_x);
                let world_z = origin.z.saturating_add(local_z);
                let surface = self.height_at(world_x, world_z);
                self.fill_column(&mut chunk, local_x, local_z, surface);
            }
        }

        // Generation touches the palette heavily; compact before it is cached.
        chunk.optimise();
        chunk.clear_dirty();
        // Generation is reproducible from the seed, so a freshly generated
        // chunk is already "saved" — writing it would store nothing that
        // regenerating could not recreate exactly.
        chunk.mark_saved();
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

        // Ore replaces stone, then caves hollow out whatever is left. Carving
        // last means a cave never leaves ore hanging in mid-air where the rock
        // around it went.
        let origin = chunk.pos().origin();
        // Saturating: the origin of a chunk at the far edge of the coordinate
        // space is already clamped, and adding the in-chunk offset to it would
        // tip it over.
        let (world_x, world_z) = (origin.x.saturating_add(x), origin.z.saturating_add(z));
        let ceiling = surface.min(CHUNK_HEIGHT - 1);

        for y in 1..=ceiling {
            let Some(local) = LocalPos::new(x, y, z) else {
                continue;
            };
            if chunk.get(local) == blocks.stone {
                if let Some(ore) = self.ore_at(world_x, y, world_z) {
                    chunk.set(local, ore);
                }
            }
        }

        for y in 1..=ceiling {
            let Some(local) = LocalPos::new(x, y, z) else {
                continue;
            };
            // Leave the seabed intact: opening a cave under water would drain
            // the ocean into it, and there is no fluid simulation to cope.
            if surface < SEA_LEVEL && y > surface - 3 {
                continue;
            }
            if self.is_cave(world_x, y, world_z) {
                chunk.set(local, BlockId::AIR);
            }
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

    pub(super) fn generator(seed: u64) -> (BlockRegistry, TerrainGenerator) {
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


#[cfg(test)]
mod calibration {
    use super::tests::generator;
    use super::*;
    use vx_core::BlockRegistry;

    /// Prints the shape of what generation actually produces.
    ///
    /// Kept as a test rather than notes in a commit message: the numbers it
    /// reports are the ones the assertions below are calibrated against, and
    /// they move whenever the noise stack is touched.
    #[test]
    fn report_terrain_and_cave_statistics() {
        let mut registry = BlockRegistry::new();
        let blocks = TerrainBlocks::register_builtins(&mut registry);
        let generator = TerrainGenerator::new(2024, blocks);

        let mut heights = Vec::new();
        for x in (-512..512).step_by(4) {
            for z in (-512..512).step_by(4) {
                heights.push(generator.height_at(x, z));
            }
        }
        heights.sort_unstable();
        let at = |q: f64| heights[(heights.len() as f64 * q) as usize % heights.len()];

        eprintln!(
            "height  min {}  p10 {}  p50 {}  p90 {}  max {}",
            heights[0],
            at(0.10),
            at(0.50),
            at(0.90),
            heights[heights.len() - 1]
        );

        let mut carved = 0usize;
        let mut sampled = 0usize;
        for x in (-128..128).step_by(2) {
            for z in (-128..128).step_by(2) {
                for y in (4..90).step_by(2) {
                    sampled += 1;
                    if generator.is_cave(x, y, z) {
                        carved += 1;
                    }
                }
            }
        }
        let fraction = carved as f64 / sampled as f64 * 100.0;
        eprintln!("cave  {carved}/{sampled} = {fraction:.2}% of underground blocks");

        // Relief is the point of the shaping step. The old stack spanned
        // barely twenty blocks between the tenth and ninetieth percentile.
        assert!(
            at(0.90) - at(0.10) > 30,
            "terrain relief collapsed to {} blocks between p10 and p90",
            at(0.90) - at(0.10)
        );
        assert!(heights[0] < SEA_LEVEL, "nowhere is low enough to be sea");
        assert!(at(0.90) > SEA_LEVEL + 15, "nowhere is high enough to be hill");

        // Caves have to be common enough to find and rare enough to leave the
        // world standing. Too generous and the terrain becomes swiss cheese
        // that the greedy mesher cannot merge, which is exactly what a first
        // attempt at 0.88 did.
        assert!(
            (0.3..6.0).contains(&fraction),
            "caves carve {fraction:.2}% of the underground, outside the sane band"
        );
    }

    #[test]
    fn caves_never_breach_the_bedrock_floor_or_the_sky() {
        let (_registry, generator) = generator(2024);
        for x in -64..64 {
            for z in -64..64 {
                assert!(!generator.is_cave(x, 0, z), "a cave opened in the bedrock");
                assert!(!generator.is_cave(x, 1, z), "a cave opened at the floor");
                assert!(!generator.is_cave(x, CHUNK_HEIGHT - 1, z));
            }
        }
    }

    #[test]
    fn ore_only_appears_within_its_depth_band() {
        let (registry, generator) = generator(2024);
        let blocks = generator.blocks();

        let mut seen = std::collections::HashMap::new();
        for x in -48..48 {
            for z in -48..48 {
                for y in 1..CHUNK_HEIGHT {
                    if let Some(ore) = generator.ore_at(x, y, z) {
                        seen.entry(ore).or_insert_with(Vec::new).push(y);
                    }
                }
            }
        }

        for (ore, limit) in [
            (blocks.gold_ore, 24),
            (blocks.iron_ore, 52),
            (blocks.coal_ore, 96),
        ] {
            let depths = seen.get(&ore).unwrap_or_else(|| {
                panic!("no {} generated at all", registry.get(ore).unwrap().name)
            });
            let deepest = depths.iter().copied().max().unwrap();
            assert!(
                deepest < limit,
                "{} appears at y={deepest}, above its {limit} ceiling",
                registry.get(ore).unwrap().name
            );
        }

        // Gold is the deep reward, so it must be rarer than coal.
        assert!(seen[&blocks.gold_ore].len() < seen[&blocks.coal_ore].len());
    }

    #[test]
    fn generated_chunks_contain_ore_and_caves() {
        // The end-to-end check that the column builder actually applies both,
        // rather than computing them and throwing the answer away.
        let (_registry, generator) = generator(2024);
        let blocks = generator.blocks();

        let mut ore = 0;
        let mut air_underground = 0;
        for cx in -2..2 {
            for cz in -2..2 {
                let chunk = generator.generate(ChunkPos::new(cx, cz));
                for (local, block) in chunk.iter_blocks() {
                    if local.y() < 4 || local.y() > 60 {
                        continue;
                    }
                    if block == blocks.coal_ore
                        || block == blocks.iron_ore
                        || block == blocks.gold_ore
                    {
                        ore += 1;
                    }
                    if block.is_air() {
                        air_underground += 1;
                    }
                }
            }
        }

        assert!(ore > 0, "no ore reached a generated chunk");
        assert!(air_underground > 0, "no caves reached a generated chunk");
    }

    #[test]
    fn generation_survives_the_far_edges_of_the_world() {
        // Chunk coordinates come out of region files, so generation has to
        // cope with values no player could ever walk to.
        let (_registry, generator) = generator(2024);
        for chunk in [
            ChunkPos::new(i32::MAX, i32::MAX),
            ChunkPos::new(i32::MIN, i32::MIN),
            ChunkPos::new(i32::MAX / 2, i32::MIN / 2),
        ] {
            let generated = generator.generate(chunk);
            // Reaching here at all is the test; the arithmetic inside would
            // otherwise overflow on the way.
            assert_eq!(generated.pos(), chunk);
        }

        for coordinate in [i32::MAX, i32::MIN, i32::MAX - 1] {
            let height = generator.height_at(coordinate, coordinate);
            assert!((1..CHUNK_HEIGHT).contains(&height));
            let _ = generator.is_cave(coordinate, 40, coordinate);
            let _ = generator.ore_at(coordinate, 20, coordinate);
        }
    }
}
