//! Terrain generation.
//!
//! Generation is a pure function of `(seed, chunk position)`. It reads no
//! shared state, so chunks can be generated on a worker pool in any order and
//! a saved world regenerates identically.

use vx_core::{
    BlockDef, BlockId, BlockPos, BlockRegistry, ChunkPos, LocalPos, CHUNK_HEIGHT, CHUNK_SIZE,
};

use crate::chunk::Chunk;
use crate::noise::{warp_2d, Fbm, Ridged, Spline};
use crate::ore::{breaks_surface, deposits_overlapping, ore_at, Deposit};
use crate::seed::SeedPath;
use crate::town::TownSite;

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
    pub copper_ore: BlockId,
    pub container: BlockId,
    pub plank: BlockId,
    pub roof: BlockId,
    pub counter: BlockId,
    pub log: BlockId,
    pub leaves: BlockId,
    pub tall_grass: BlockId,
    pub metal_wall: BlockId,
    /// The watch box the sheriff's drone lives in — and, bought, the one on
    /// your own roof. Hard enough that a slug glances off it; a drill gets
    /// through eventually, which is the loud way to blind the town.
    pub roost: BlockId,
    pub rusted_metal: BlockId,
    pub catwalk: BlockId,
    pub mast: BlockId,
    pub beacon: BlockId,
    /// What a refinery makes. Worth far more than the ore and stone it eats,
    /// which is what turns the trade network into a chain rather than a
    /// gradient.
    pub copper_bar: BlockId,
    /// The player's storage chest: breakable, because moving house is the
    /// feature.
    pub chest: BlockId,
    /// The mailbox outside the player's house. Town furniture like the counter
    /// and the beacon: unbreakable, which is what deletes every "what if the
    /// mailbox is gone when the mail lands" edge case.
    pub mailbox: BlockId,
    /// The lockboxes that hold a building's permissions, in three grades.
    ///
    /// Deliberately *breakable*, unlike the counter and the beacon. A lock you
    /// cannot attack is a wall with extra steps; the whole design rests on
    /// there being a loud, slow, expensive way through, so that the quiet
    /// legitimate way is worth preferring.
    pub permit_box_i: BlockId,
    pub permit_box_ii: BlockId,
    pub permit_box_iii: BlockId,
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
            // Harder than the stone it sits in, so mining ore is a commitment.
            copper_ore: register(
                BlockDef::uniform("engine:copper_ore", 7).with_hardness(Some(2.5)),
            ),
            // The fleet's drop-off. Placing one declares the base; the world
            // knows it only as a block, and the fleet keys off its position.
            container: register(BlockDef::uniform("engine:container", 8).with_hardness(Some(1.5))),
            plank: register(BlockDef::uniform("engine:plank", 13).with_hardness(Some(1.2))),
            roof: register(BlockDef::uniform("engine:roof", 14).with_hardness(Some(1.2))),
            // The shop counter is the town's one immovable fixture: no
            // hardness, so neither drill nor drone can take the economy
            // apart.
            counter: register(BlockDef::uniform("engine:counter", 15).with_hardness(None)),
            log: register(BlockDef::columnar("engine:log", 19, 18, 19).with_hardness(Some(1.0))),
            leaves: register(BlockDef::uniform("engine:leaves", 20).with_hardness(Some(0.3))),
            tall_grass: register(
                BlockDef::uniform("engine:tall_grass", 21)
                    .cross()
                    .with_hardness(Some(0.05)),
            ),
            metal_wall: register(
                BlockDef::uniform("engine:metal_wall", 22).with_hardness(Some(1.6)),
            ),
            rusted_metal: register(
                BlockDef::uniform("engine:rusted_metal", 23).with_hardness(Some(1.6)),
            ),
            catwalk: register(BlockDef::uniform("engine:catwalk", 24).with_hardness(Some(1.0))),
            mast: register(BlockDef::uniform("engine:mast", 25).with_hardness(Some(2.0))),
            // The beacon is the town's link to the network, and like the shop
            // counter it is not something a drill or a drone gets to dismantle.
            beacon: register(BlockDef::uniform("engine:beacon", 26).with_hardness(None)),
            // Soft for metal — a stack of bars is stacked, not welded.
            copper_bar: register(
                BlockDef::uniform("engine:copper_bar", 27).with_hardness(Some(2.5)),
            ),
            chest: register(BlockDef::uniform("engine:chest", 28).with_hardness(Some(1.5))),
            mailbox: register(BlockDef::uniform("engine:mailbox", 29).with_hardness(None)),
            // Hardness sets how long a breach takes once you are past the
            // tier's power gate; the gate itself lives in the drill, because
            // "impossible for a new player" cannot be expressed in seconds.
            permit_box_i: register(
                BlockDef::uniform("engine:permit_box_i", 30).with_hardness(Some(30.0)),
            ),
            permit_box_ii: register(
                BlockDef::uniform("engine:permit_box_ii", 31).with_hardness(Some(150.0)),
            ),
            permit_box_iii: register(
                BlockDef::uniform("engine:permit_box_iii", 32).with_hardness(Some(400.0)),
            ),
            roost: register(BlockDef::uniform("engine:roost", 33).with_hardness(Some(6.0))),
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
            copper_ore: registry.id_of("engine:copper_ore")?,
            container: registry.id_of("engine:container")?,
            plank: registry.id_of("engine:plank")?,
            roof: registry.id_of("engine:roof")?,
            counter: registry.id_of("engine:counter")?,
            log: registry.id_of("engine:log")?,
            leaves: registry.id_of("engine:leaves")?,
            tall_grass: registry.id_of("engine:tall_grass")?,
            metal_wall: registry.id_of("engine:metal_wall")?,
            rusted_metal: registry.id_of("engine:rusted_metal")?,
            catwalk: registry.id_of("engine:catwalk")?,
            mast: registry.id_of("engine:mast")?,
            beacon: registry.id_of("engine:beacon")?,
            copper_bar: registry.id_of("engine:copper_bar")?,
            chest: registry.id_of("engine:chest")?,
            mailbox: registry.id_of("engine:mailbox")?,
            permit_box_i: registry.id_of("engine:permit_box_i")?,
            permit_box_ii: registry.id_of("engine:permit_box_ii")?,
            permit_box_iii: registry.id_of("engine:permit_box_iii")?,
            roost: registry.id_of("engine:roost")?,
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
    /// Where in the seed tree this world sits. Today always a root, which
    /// folds to the bare seed it was built from — see [`crate::seed`].
    path: SeedPath,
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
        TerrainGenerator::at(SeedPath::root(seed), blocks)
    }

    /// A generator for a named place in the seed tree.
    ///
    /// Everything downstream keys on [`TerrainGenerator::seed`], which is the
    /// path's folded value — so a world one level down is generated by exactly
    /// the same code as the flat world, with a different number.
    pub fn at(path: SeedPath, blocks: TerrainBlocks) -> Self {
        let seed = path.seed();
        TerrainGenerator {
            path,
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

    /// Where this world sits in the seed tree.
    pub fn path(&self) -> &SeedPath {
        &self.path
    }

    pub fn blocks(&self) -> TerrainBlocks {
        self.blocks
    }

    /// Surface height at a world column: the y of its topmost solid block.
    ///
    /// Pure in `(seed, x, z)` — no state, no ordering, so columns can be
    /// evaluated in any order and on any thread.
    ///
    /// This is the seed's *own* terrain, before any town flattens a plot into
    /// it, and it is the only height a town-site test may consult. Siting
    /// against the blended field would let one town's plateau decide where the
    /// next town stands.
    pub fn natural_height_at(&self, world_x: i32, world_z: i32) -> i32 {
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

    /// Surface height with every nearby town's plateau blended in.
    ///
    /// What the minimap, spawn placement and physics all ask, so they agree
    /// about where the ground is. Gathers the towns that could reach this one
    /// column; for the per-chunk path use [`TerrainGenerator::height_with_sites`]
    /// instead, which gathers once for the whole chunk.
    pub fn height_at(&self, world_x: i32, world_z: i32) -> i32 {
        let sites = self.towns_overlapping((world_x, world_z), (world_x, world_z));
        self.height_with_sites(world_x, world_z, &sites)
    }

    /// The same, against an already-gathered site list.
    ///
    /// `sites` must have been gathered over a box containing this column —
    /// the superset contract [`crate::town`] documents. Break it and two
    /// chunks disagree about a shared column, which shows up as a seam.
    pub fn height_with_sites(&self, world_x: i32, world_z: i32, sites: &[TownSite]) -> i32 {
        let natural = self.natural_height_at(world_x, world_z);
        crate::town::blend_height(sites, world_x, world_z, natural)
    }

    /// Every town whose plateau could reach the column box.
    ///
    /// Binds the *natural* height field, never the blended one: siting a town
    /// against terrain another town already flattened is how the height field
    /// starts feeding itself.
    pub fn towns_overlapping(&self, min: (i32, i32), max: (i32, i32)) -> Vec<TownSite> {
        crate::town::towns_overlapping(self.seed, min, max, &|x, z| self.natural_height_at(x, z))
    }

    /// Towns near a column, nearest first — and without loading a thing.
    pub fn towns_near(&self, at: (i32, i32), radius: i32) -> Vec<TownSite> {
        crate::town::towns_near(self.seed, at, radius, &|x, z| self.natural_height_at(x, z))
    }

    /// The towns reaching a chunk, gathered with the margin every per-chunk
    /// helper needs, so one gather serves terrain, ore masking, flora and
    /// stamping alike.
    fn sites_for_chunk(&self, pos: ChunkPos) -> Vec<TownSite> {
        let origin = pos.origin();
        let margin = crate::flora::CANOPY_REACH;
        self.towns_overlapping(
            (origin.x - margin, origin.z - margin),
            (origin.x + CHUNK_SIZE - 1 + margin, origin.z + CHUNK_SIZE - 1 + margin),
        )
    }

    /// Generate the chunk at `pos`.
    pub fn generate(&self, pos: ChunkPos) -> Chunk {
        let mut chunk = Chunk::empty(pos);
        let origin = pos.origin();

        // The towns reaching this chunk, gathered once. Every helper below
        // shares the list, which is what keeps them agreeing about a column.
        let sites = self.sites_for_chunk(pos);

        // Gather the ore bodies reaching this chunk once. Hashing the deposit
        // lattice per block would cost far more than the terrain itself.
        let deposits = deposits_overlapping(
            self.seed,
            origin,
            BlockPos::new(origin.x + CHUNK_SIZE, CHUNK_HEIGHT, origin.z + CHUNK_SIZE),
        );

        for local_z in 0..CHUNK_SIZE {
            for local_x in 0..CHUNK_SIZE {
                let world_x = origin.x + local_x;
                let world_z = origin.z + local_z;
                let surface = self.height_with_sites(world_x, world_z, &sites);
                self.fill_column(
                    &mut chunk,
                    [local_x, local_z],
                    [world_x, world_z],
                    surface,
                    &deposits,
                    &sites,
                );
            }
        }

        // Trees, gathered like ore: every tree whose canopy reaches this
        // chunk, stamped only where it lands inside. Trunks overwrite the
        // tufts fill_column may have dropped on their base columns.
        let height_at = |x: i32, z: i32| self.height_with_sites(x, z, &sites);
        let trees = crate::flora::trees_overlapping(
            self.seed,
            (origin.x, origin.z),
            (origin.x + CHUNK_SIZE - 1, origin.z + CHUNK_SIZE - 1),
            &height_at,
            &sites,
        );
        for tree in &trees {
            let top = tree.base.y + tree.height + 2;
            for local_z in 0..CHUNK_SIZE {
                for local_x in 0..CHUNK_SIZE {
                    let world_x = origin.x + local_x;
                    let world_z = origin.z + local_z;
                    if (world_x - tree.base.x).abs() > crate::flora::CANOPY_REACH
                        || (world_z - tree.base.z).abs() > crate::flora::CANOPY_REACH
                    {
                        continue;
                    }
                    for y in tree.base.y + 1..=top {
                        let Some(part) = crate::flora::tree_part_at(tree, world_x, y, world_z)
                        else {
                            continue;
                        };
                        let block = match part {
                            crate::flora::TreePart::Trunk => self.blocks.log,
                            crate::flora::TreePart::Leaves => self.blocks.leaves,
                        };
                        if let Some(cell) = LocalPos::new(local_x, y, local_z) {
                            // Leaves fill only air (or a tuft caught under
                            // the crown), so canopies interleave with the
                            // hillside instead of eating it, and never eat a
                            // trunk.
                            let standing = chunk.get(cell);
                            if part == crate::flora::TreePart::Trunk
                                || standing.is_air()
                                || standing == self.blocks.tall_grass
                            {
                                chunk.set(cell, block);
                            }
                        }
                    }
                }
            }
        }

        // Each town's authored buildings, where this chunk overlaps them.
        crate::town::plan::stamp(&mut chunk, pos, &sites, &self.blocks);

        // Generation touches the palette heavily; compact before it is cached.
        chunk.optimise();
        chunk.clear_dirty();
        // Freshly generated terrain matches the seed, so there is nothing to
        // save until a player changes it.
        chunk.clear_modified();
        chunk
    }

    /// Lay down the vertical stack for one column.
    ///
    /// `local` is the position within the chunk, `world` the same column in
    /// world coordinates — ore lookup needs the latter.
    fn fill_column(
        &self,
        chunk: &mut Chunk,
        local: [i32; 2],
        world: [i32; 2],
        surface: i32,
        deposits: &[Deposit],
        sites: &[TownSite],
    ) {
        let blocks = self.blocks;
        let [x, z] = local;
        let [world_x, world_z] = world;

        // No ore under main street: prospecting belongs to the wilderness,
        // and nobody should be tempted to dig up a town.
        let in_village = crate::town::footprint_contains(sites, world_x, world_z);

        // Beaches: columns near sea level get sand instead of grass.
        let coastal = surface <= SEA_LEVEL + 1;
        let top = if coastal { blocks.sand } else { blocks.grass };
        let subsoil = if coastal { blocks.sand } else { blocks.dirt };

        chunk.fill_column(x, z, 0, 1, blocks.bedrock);

        let soil_depth = 4;
        let stone_top = (surface - soil_depth).max(1);

        let place = |chunk: &mut Chunk, y: i32, block: BlockId| {
            if let Some(cell) = LocalPos::new(x, y, z) {
                chunk.set(cell, block);
            }
        };

        // Rock, with ore wherever a body reaches. Restricting ore to what would
        // otherwise be stone or soil is what guarantees it never hangs in air.
        for y in 1..stone_top {
            let has_ore =
                !in_village && ore_at(deposits, BlockPos::new(world_x, y, world_z)).is_some();
            place(chunk, y, if has_ore { blocks.copper_ore } else { blocks.stone });
        }

        // Overburden. A body reaching up through here is on its way to becoming
        // a visible outcrop.
        for y in stone_top.max(1)..surface {
            let has_ore =
                !in_village && ore_at(deposits, BlockPos::new(world_x, y, world_z)).is_some();
            place(chunk, y, if has_ore { blocks.copper_ore } else { subsoil });
        }

        // The surface block itself only shows ore when the body also fills the
        // blocks beneath it, so every outcrop a player can see leads somewhere.
        let surface_pos = BlockPos::new(world_x, surface, world_z);
        let outcrop = !in_village && breaks_surface(deposits, surface_pos);
        place(chunk, surface, if outcrop { blocks.copper_ore } else { top });

        // A scattering of grass tufts on plain grass tops — never on an
        // outcrop, which players need to be able to spot at distance.
        if !coastal
            && !outcrop
            && crate::flora::tuft_at(self.seed, world_x, world_z, surface, sites)
        {
            place(chunk, surface + 1, blocks.tall_grass);
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
        // Away from the origin: the starting village makes the chunks there
        // identical across seeds by design.
        let pos = ChunkPos::new(8, 8);
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

        let flora = [
            generator.blocks().log,
            generator.blocks().leaves,
            generator.blocks().tall_grass,
        ];
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let expected = generator.height_at(origin.x + x, origin.z + z);
                // Vegetation legitimately stands above the height field, so
                // the surface is the topmost non-air, non-water, non-plant.
                let solid_top = (0..CHUNK_HEIGHT)
                    .rev()
                    .find(|&y| {
                        let block = chunk.get(LocalPos::new(x, y, z).unwrap());
                        !block.is_air()
                            && block != generator.blocks().water
                            && !flora.contains(&block)
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
        // Out in the wild: village buildings would put authored blocks above
        // the height field on the origin chunks.
        let left = generator.generate(ChunkPos::new(10, 0));
        let right = generator.generate(ChunkPos::new(11, 0));

        let flora = [
            generator.blocks().log,
            generator.blocks().leaves,
            generator.blocks().tall_grass,
        ];
        let ground_top = |chunk: &Chunk, x: i32, z: i32| {
            (0..CHUNK_HEIGHT)
                .rev()
                .find(|&y| {
                    let block = chunk.get(LocalPos::new(x, y, z).unwrap());
                    !block.is_air() && !flora.contains(&block)
                })
                .unwrap()
        };

        let seam = 11 * CHUNK_SIZE;
        for z in 0..CHUNK_SIZE {
            let left_edge = ground_top(&left, CHUNK_SIZE - 1, z);
            let right_edge = ground_top(&right, 0, z);
            let expected_left = generator.height_at(seam - 1, z);
            let expected_right = generator.height_at(seam, z);

            // The ground top skips vegetation, and a column below sea level
            // is flooded to exactly SEA_LEVEL.
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
    fn ore_never_hangs_in_the_air_or_sits_above_the_surface() {
        // Ore only ever replaces rock or overburden, so it can never be found
        // floating or perched on top of the ground.
        let (_, generator) = generator(2024);

        for cx in -3..3 {
            for cz in -3..3 {
                let pos = ChunkPos::new(cx, cz);
                let chunk = generator.generate(pos);
                let origin = pos.origin();

                for (local, block) in chunk.iter_blocks() {
                    if block != generator.blocks().copper_ore {
                        continue;
                    }
                    let surface = generator.height_at(
                        origin.x + local.x(),
                        origin.z + local.z(),
                    );
                    assert!(
                        local.y() <= surface,
                        "ore at y={} sits above the surface at {surface}",
                        local.y()
                    );
                    assert!(local.y() >= 1, "ore replaced the bedrock floor");
                }
            }
        }
    }

    /// Scan a block of chunks and report (outcrop columns, total columns, ore blocks, total blocks).
    fn survey_ore(generator: &TerrainGenerator, radius: i32) -> (usize, usize, usize, usize) {
        let ore = generator.blocks().copper_ore;
        let (mut outcrops, mut columns, mut ore_blocks, mut blocks) = (0, 0, 0, 0);

        // Surveyed away from the origin: the starting village masks all ore
        // in its footprint, so a window centred there sees nothing. (16, 16)
        // rather than the nearest wild chunk because deposits cluster — the
        // window immediately east of town happens to be barren at the test
        // seeds, which is the scarcity working as intended.
        let centre = 16;
        for cx in centre - radius..centre + radius {
            for cz in centre - radius..centre + radius {
                let pos = ChunkPos::new(cx, cz);
                let chunk = generator.generate(pos);
                let origin = pos.origin();

                for x in 0..CHUNK_SIZE {
                    for z in 0..CHUNK_SIZE {
                        columns += 1;
                        let surface = generator.height_at(origin.x + x, origin.z + z);
                        if let Some(cell) = LocalPos::new(x, surface, z) {
                            if chunk.get(cell) == ore {
                                outcrops += 1;
                            }
                        }
                    }
                }
                for (_, block) in chunk.iter_blocks() {
                    blocks += 1;
                    if block == ore {
                        ore_blocks += 1;
                    }
                }
            }
        }
        (outcrops, columns, ore_blocks, blocks)
    }

    /// Columns whose surface would show ore, found without generating chunks.
    ///
    /// Outcrops **cluster** around the few deposits that happen to breach, so a
    /// small sample is worse than useless — a 128-block patch usually contains
    /// none at all even though the world-wide rate is healthy. This sweeps wide
    /// and sparsely instead.
    fn outcrop_columns(generator: &TerrainGenerator, reach: i32, step: usize) -> (usize, usize) {
        use crate::ore::{breaks_surface, deposits_overlapping};

        let (mut outcrops, mut columns) = (0, 0);
        for x in (-reach..reach).step_by(step) {
            for z in (-reach..reach).step_by(step) {
                let surface = generator.height_at(x, z);
                let nearby = deposits_overlapping(
                    generator.seed(),
                    BlockPos::new(x, surface - crate::ore::OUTCROP_MIN_DEPTH, z),
                    BlockPos::new(x, surface, z),
                );
                columns += 1;
                if breaks_surface(&nearby, BlockPos::new(x, surface, z)) {
                    outcrops += 1;
                }
            }
        }
        (outcrops, columns)
    }

    #[test]
    fn outcrops_are_rare_but_do_occur() {
        // Too common and they stop being a find; absent and the prospecting
        // loop has no entry point at all.
        let (_, generator) = generator(2024);
        let (outcrops, columns) = outcrop_columns(&generator, 1500, 7);

        let rate = outcrops as f32 / columns as f32;
        assert!(outcrops > 0, "no outcrop anywhere in a 3000-block sweep");
        assert!(
            rate < 0.02,
            "{:.3}% of the surface is ore; outcrops should be a find, not scenery",
            rate * 100.0
        );
        assert!(
            rate > 0.0002,
            "{:.4}% outcrop rate is so low a player would never stumble on one",
            rate * 100.0
        );
    }

    #[test]
    fn every_visible_outcrop_has_ore_continuing_beneath_it() {
        // The invariant the mechanic rests on. An outcrop that led nowhere
        // would teach players to ignore outcrops, and the scanner with them.
        //
        // Finds real outcrops cheaply first, then generates only those chunks
        // and checks the blocks actually placed.
        let (_, generator) = generator(2024);
        let ore = generator.blocks().copper_ore;

        let mut checked = 0;
        'sweep: for x in (-1500..1500).step_by(11) {
            for z in (-1500..1500).step_by(11) {
                let surface = generator.height_at(x, z);
                let nearby = crate::ore::deposits_overlapping(
                    generator.seed(),
                    BlockPos::new(x, surface - crate::ore::OUTCROP_MIN_DEPTH, z),
                    BlockPos::new(x, surface, z),
                );
                if !crate::ore::breaks_surface(&nearby, BlockPos::new(x, surface, z)) {
                    continue;
                }

                let pos = BlockPos::new(x, surface, z).chunk();
                let chunk = generator.generate(pos);
                let local = BlockPos::new(x, surface, z)
                    .local()
                    .expect("a surface block is inside the world");
                assert_eq!(chunk.get(local), ore, "expected an outcrop at ({x}, {surface}, {z})");

                for depth in 1..crate::ore::OUTCROP_MIN_DEPTH {
                    let below = LocalPos::new(local.x(), surface - depth, local.z())
                        .expect("an outcrop always has blocks beneath it");
                    assert_eq!(
                        chunk.get(below),
                        ore,
                        "outcrop at ({x}, {surface}, {z}) runs only {depth} deep"
                    );
                }

                checked += 1;
                if checked >= 12 {
                    break 'sweep;
                }
            }
        }
        assert!(checked > 0, "found no outcrops to check");
    }

    #[test]
    fn ore_is_scarce_enough_to_be_worth_hunting() {
        let (_, generator) = generator(31337);
        let (_, _, ore_blocks, blocks) = survey_ore(&generator, 3);

        let share = ore_blocks as f64 / blocks as f64;
        assert!(ore_blocks > 0, "no ore generated at all");
        assert!(
            share < 0.02,
            "ore is {:.3}% of the world; far too common",
            share * 100.0
        );
    }

    #[test]
    fn most_ore_is_buried_where_only_a_scanner_would_find_it() {
        // If every body broke the surface there would be nothing for the flying
        // drone to do in stage 4.
        let (_, generator) = generator(2024);
        let (surfacing, columns) = outcrop_columns(&generator, 1500, 7);
        let (_, _, ore_blocks, _) = survey_ore(&generator, 3);

        assert!(ore_blocks > 0, "no ore generated at all");
        assert!(
            surfacing * 20 < columns,
            "{surfacing} of {columns} columns surface ore; almost nothing is hidden"
        );
    }

    #[test]
    fn ore_placement_is_deterministic_across_regenerations() {
        let (_, a) = generator(555);
        let (_, b) = generator(555);
        let pos = ChunkPos::new(2, -3);
        assert_eq!(a.generate(pos), b.generate(pos));
    }

    #[test]
    fn generated_chunks_come_back_clean_and_compacted() {
        let (_, generator) = generator(4);
        // Wilderness: the village chunks near the origin legitimately carry
        // more block kinds and get their own bound below.
        let chunk = generator.generate(ChunkPos::new(20, 20));

        assert!(!chunk.is_dirty(), "a freshly generated chunk needs no remesh flag");
        // Only the handful of terrain blocks should survive palette compaction.
        assert!(
            chunk.storage().palette_len() <= 12,
            "palette not compacted: {} entries",
            chunk.storage().palette_len()
        );
    }

    #[test]
    fn village_chunks_compact_and_stay_within_their_own_palette_budget() {
        let (_, generator) = generator(4);
        let chunk = generator.generate(ChunkPos::new(0, 0));

        assert!(!chunk.is_dirty());
        assert!(
            chunk.storage().palette_len() <= 14,
            "village palette blew up: {} entries",
            chunk.storage().palette_len()
        );
    }

    #[test]
    fn the_village_is_identical_whatever_the_seed() {
        // The whole point: two different worlds, one hometown. Compare the
        // four chunks meeting at the origin block-for-block.
        let (_, a) = generator(2024);
        let (_, b) = generator(555);
        for pos in [
            ChunkPos::new(0, 0),
            ChunkPos::new(-1, 0),
            ChunkPos::new(0, -1),
            ChunkPos::new(-1, -1),
        ] {
            assert_eq!(a.generate(pos), b.generate(pos), "chunk {pos:?} differs");
        }
    }

    #[test]
    fn the_village_has_no_ore_and_the_counter_is_home() {
        let (registry, generator) = generator(2024);
        let ore = registry.id_of("engine:copper_ore").unwrap();
        let counter = registry.id_of("engine:counter").unwrap();

        let mut counters = 0;
        for cx in -3..3 {
            for cz in -3..3 {
                let pos = ChunkPos::new(cx, cz);
                let origin = pos.origin();
                let chunk = generator.generate(pos);
                for z in 0..CHUNK_SIZE {
                    for x in 0..CHUNK_SIZE {
                        let sites = generator.towns_overlapping(
                            (origin.x + x, origin.z + z),
                            (origin.x + x, origin.z + z),
                        );
                        let in_town =
                            crate::town::footprint_contains(&sites, origin.x + x, origin.z + z);
                        // Town columns top out at the plateau plus rooftops,
                        // so scanning higher buys nothing but test time.
                        for y in 0..100 {
                            let block = chunk.get(LocalPos::new(x, y, z).unwrap());
                            if block == ore {
                                assert!(!in_town, "ore under main street at ({x},{y},{z})");
                            }
                            if block == counter {
                                counters += 1;
                            }
                        }
                    }
                }
            }
        }

        let at = crate::town::counter_position(&crate::town::home_site());
        let counter_chunk = generator.generate(ChunkPos::new(
            at.x.div_euclid(CHUNK_SIZE),
            at.z.div_euclid(CHUNK_SIZE),
        ));
        let local = LocalPos::new(at.x.rem_euclid(CHUNK_SIZE), at.y, at.z.rem_euclid(CHUNK_SIZE));
        assert_eq!(counter_chunk.get(local.unwrap()), counter, "no counter at its post");
        assert_eq!(counters, 5, "the shop's counter row should be five blocks");
    }

    #[test]
    fn a_canopy_crossing_a_chunk_border_is_whole_on_both_sides() {
        let (_, generator) = generator(2024);
        let height = |x: i32, z: i32| generator.height_at(x, z);

        // Find a tree whose canopy reaches over an x-border between chunks.
        let sites = generator.towns_overlapping((-400, -400), (400, 400));
        let trees = crate::flora::trees_overlapping(
            generator.seed(),
            (-400, -400),
            (400, 400),
            &height,
            &sites,
        );
        let straddler = trees
            .iter()
            .find(|tree| {
                let offset = tree.base.x.rem_euclid(CHUNK_SIZE);
                !(crate::flora::CANOPY_REACH..CHUNK_SIZE - crate::flora::CANOPY_REACH)
                    .contains(&offset)
            })
            .expect("no border-straddling tree in an 800-block square");

        let mut chunks = std::collections::HashMap::new();
        let top = straddler.base.y + straddler.height + 2;
        for x in straddler.base.x - 2..=straddler.base.x + 2 {
            for z in straddler.base.z - 2..=straddler.base.z + 2 {
                for y in straddler.base.y + 1..=top {
                    let Some(part) = crate::flora::tree_part_at(straddler, x, y, z) else {
                        continue;
                    };
                    let pos = ChunkPos::new(x.div_euclid(CHUNK_SIZE), z.div_euclid(CHUNK_SIZE));
                    let chunk = chunks.entry(pos).or_insert_with(|| generator.generate(pos));
                    let cell =
                        LocalPos::new(x.rem_euclid(CHUNK_SIZE), y, z.rem_euclid(CHUNK_SIZE))
                            .unwrap();
                    let block = chunk.get(cell);
                    match part {
                        crate::flora::TreePart::Trunk => assert_eq!(
                            block,
                            generator.blocks().log,
                            "trunk missing at ({x},{y},{z})"
                        ),
                        // Leaves only fill air, so a hillside or another
                        // tree's trunk may legitimately hold the cell.
                        crate::flora::TreePart::Leaves => assert!(
                            block == generator.blocks().leaves
                                || block == generator.blocks().log
                                || height(x, z) >= y,
                            "canopy hole at ({x},{y},{z})"
                        ),
                    }
                }
            }
        }
        assert!(chunks.len() >= 2, "the straddler did not actually straddle");
    }

    #[test]
    fn tufts_grow_on_grass_and_never_in_town() {
        let (_, generator) = generator(2024);
        let tuft = generator.blocks().tall_grass;

        // The village chunk must have none; a wild chunk should have some.
        let town = generator.generate(ChunkPos::new(0, 0));
        for (local, block) in town.iter_blocks() {
            assert_ne!(block, tuft, "a tuft in town at {local:?}");
        }

        let mut found = 0;
        for cx in 12..18 {
            let wild = generator.generate(ChunkPos::new(cx, 12));
            for (_, block) in wild.iter_blocks() {
                if block == tuft {
                    found += 1;
                }
            }
        }
        assert!(found > 0, "no tufts anywhere in six wild chunks");
    }

    #[test]
    fn spawn_column_is_paved_flat_ground() {
        let (registry, generator) = generator(7);
        let chunk = generator.generate(ChunkPos::new(0, 0));
        let surface = chunk.height_at(0, 0).unwrap();
        assert_eq!(surface, crate::town::HOME_GROUND_Y, "spawn is off the plateau");
        let top = chunk.get(LocalPos::new(0, surface, 0).unwrap());
        assert_eq!(registry.get(top).unwrap().name, "engine:stone", "spawn is not paved");
    }
}





