//! The starting village: the same town in every world.
//!
//! Everything here is pure data and pure functions of *position only* — no
//! seed enters anywhere, which is not a special case but the type signature.
//! The village is a drawing: an authored plateau height, blended into the
//! natural terrain over a skirt, and a handful of buildings described as
//! ASCII layer blueprints and stamped during generation.
//!
//! The height override is applied inside [`TerrainGenerator::height_at`]
//! rather than only in `generate`, so the minimap (which re-derives unloaded
//! terrain from the height field), spawn placement and physics all agree
//! about the town for free.
//!
//! [`TerrainGenerator::height_at`]: crate::gen::TerrainGenerator::height_at

use vx_core::{BlockPos, ChunkPos, LocalPos, CHUNK_SIZE};

use crate::chunk::Chunk;
use crate::gen::TerrainBlocks;

/// The plateau the town stands on. Above `SEA_LEVEL + 1`, so the village
/// never trades its lawns for beach sand.
pub const GROUND_Y: i32 = 72;

/// Half-width of the flat core square: `|x|, |z| <= CORE_HALF`.
pub const CORE_HALF: i32 = 26;

/// Width of the blending skirt around the core, where the plateau eases
/// into whatever the seed grew there. Wide enough that even a mountain seed
/// (a ~70-block rise) blends at a walkable grade: smoothstep's steepest
/// stretch is 1.5× the mean, so 70/24 × 1.5 ≈ 4.4 blocks per column.
pub const SKIRT: i32 = 24;

/// Is this column on the flat core?
pub fn core_contains(x: i32, z: i32) -> bool {
    x.abs() <= CORE_HALF && z.abs() <= CORE_HALF
}

/// Is this column anywhere the village touches terrain (core or skirt)?
pub fn footprint_contains(x: i32, z: i32) -> bool {
    distance_to_core(x, z) < SKIRT as f32
}

/// Euclidean distance from a column to the core square. Zero inside.
///
/// Euclidean rather than Chebyshev so the skirt wraps the corners in smooth
/// arcs instead of creased diagonals.
fn distance_to_core(x: i32, z: i32) -> f32 {
    let dx = (x.abs() - CORE_HALF).max(0) as f32;
    let dz = (z.abs() - CORE_HALF).max(0) as f32;
    (dx * dx + dz * dz).sqrt()
}

/// The terrain height with the village applied.
///
/// Core columns are exactly [`GROUND_Y`]; columns beyond the skirt are the
/// `natural` height bit-for-bit; the skirt smoothsteps between the two, so
/// town meets wilderness without a cliff.
pub fn blend_height(x: i32, z: i32, natural: i32) -> i32 {
    let distance = distance_to_core(x, z);
    if distance <= 0.0 {
        return GROUND_Y;
    }
    if distance >= SKIRT as f32 {
        return natural;
    }
    let t = distance / SKIRT as f32;
    let smooth = t * t * (3.0 - 2.0 * t);
    GROUND_Y + ((natural - GROUND_Y) as f32 * smooth).round() as i32
}

/// What an authored cell is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    /// Walls and floors.
    Plank,
    /// The slab on top.
    Roof,
    /// The shop's trading counter.
    Counter,
    /// Paving, at ground level.
    Path,
}

/// One authored building: layers of rows of characters, bottom-up.
///
/// Layer `i` lands at `GROUND_Y + i`; layer 0 *replaces* the surface block
/// (floors and paving), higher layers stand in the air above. Rows run +z
/// from `min.1`, characters run +x from `min.0`. `#` plank, `R` roof,
/// `C` counter, `P` path, `.` nothing — doors and windows are wall gaps,
/// nothing is carved.
struct Blueprint {
    min: (i32, i32),
    layers: &'static [&'static [&'static str]],
}

/// The supply shop: 9 wide, 7 deep, door south onto the plaza, the counter
/// row standing behind it. The counter is the block trades happen over.
const SHOP: Blueprint = Blueprint {
    min: (-4, 6),
    layers: &[
        &[
            "#########",
            "#########",
            "#########",
            "#########",
            "#########",
            "#########",
            "#########",
        ],
        &[
            "###...###",
            "#.......#",
            "#.......#",
            "#.CCCCC.#",
            "#.......#",
            "#.......#",
            "#########",
        ],
        &[
            "####.####",
            "#.......#",
            "#.......#",
            "#.......#",
            "#.......#",
            "#.......#",
            "#########",
        ],
        &[
            "RRRRRRRRR",
            "RRRRRRRRR",
            "RRRRRRRRR",
            "RRRRRRRRR",
            "RRRRRRRRR",
            "RRRRRRRRR",
            "RRRRRRRRR",
        ],
    ],
};

/// A house: 7 wide, 6 deep. The door position varies per copy by rotating
/// which wall carries the gap in the blueprint itself.
const HOUSE_EAST_DOOR: &[&[&str]] = &[
    &[
        "#######", "#######", "#######", "#######", "#######", "#######",
    ],
    &[
        "#######", "#.....#", "#......", "#......", "#.....#", "#######",
    ],
    &[
        "#######", "#.....#", "#.....#", "#.....#", "#.....#", "#######",
    ],
    &[
        "RRRRRRR", "RRRRRRR", "RRRRRRR", "RRRRRRR", "RRRRRRR", "RRRRRRR",
    ],
];

const HOUSE_WEST_DOOR: &[&[&str]] = &[
    &[
        "#######", "#######", "#######", "#######", "#######", "#######",
    ],
    &[
        "#######", "#.....#", "......#", "......#", "#.....#", "#######",
    ],
    &[
        "#######", "#.....#", "#.....#", "#.....#", "#.....#", "#######",
    ],
    &[
        "RRRRRRR", "RRRRRRR", "RRRRRRR", "RRRRRRR", "RRRRRRR", "RRRRRRR",
    ],
];

const HOUSE_NORTH_DOOR: &[&[&str]] = &[
    &[
        "#######", "#######", "#######", "#######", "#######", "#######",
    ],
    &[
        "#######", "#.....#", "#.....#", "#.....#", "#.....#", "##...##",
    ],
    &[
        "#######", "#.....#", "#.....#", "#.....#", "#.....#", "#######",
    ],
    &[
        "RRRRRRR", "RRRRRRR", "RRRRRRR", "RRRRRRR", "RRRRRRR", "RRRRRRR",
    ],
];

/// Paving: an east-west high street past every house door, and a north-south
/// lane from the north house up to the shop's porch, crossing at the plaza
/// where the player wakes up.
const PATHS: Blueprint = Blueprint {
    min: (-16, -9),
    layers: &[&[
        "...............PPP...............",
        "...............PPP...............",
        "...............PPP...............",
        "...............PPP...............",
        "...............PPP...............",
        "...............PPP...............",
        "PPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPP",
        "PPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPP",
        "PPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPP",
        "...............PPP...............",
        "...............PPP...............",
        "...............PPP...............",
        "...............PPP...............",
        "...............PPP...............",
        "...............PPP...............",
    ]],
};

const BUILDINGS: &[Blueprint] = &[
    SHOP,
    Blueprint {
        min: (-17, -3),
        layers: HOUSE_EAST_DOOR,
    },
    Blueprint {
        min: (10, -3),
        layers: HOUSE_WEST_DOOR,
    },
    Blueprint {
        min: (-4, -15),
        layers: HOUSE_NORTH_DOOR,
    },
    PATHS,
];

/// The authored cell at a world position, if any.
pub fn blocks_at(x: i32, y: i32, z: i32) -> Option<Cell> {
    let layer = y - GROUND_Y;
    if layer < 0 {
        return None;
    }
    for blueprint in BUILDINGS {
        let Some(rows) = blueprint.layers.get(layer as usize) else {
            continue;
        };
        let (row, col) = (z - blueprint.min.1, x - blueprint.min.0);
        if row < 0 || col < 0 {
            continue;
        }
        let Some(line) = rows.get(row as usize) else {
            continue;
        };
        match line.as_bytes().get(col as usize) {
            Some(b'#') => return Some(Cell::Plank),
            Some(b'R') => return Some(Cell::Roof),
            Some(b'C') => return Some(Cell::Counter),
            Some(b'P') => return Some(Cell::Path),
            _ => continue,
        }
    }
    None
}

/// The counter block trades happen over: middle of the shop's counter row.
pub fn counter_position() -> BlockPos {
    BlockPos::new(0, GROUND_Y + 1, 9)
}

/// The tallest authored layer, for the stamping loop's vertical bound.
fn max_layers() -> i32 {
    BUILDINGS
        .iter()
        .map(|blueprint| blueprint.layers.len() as i32)
        .max()
        .unwrap_or(0)
}

/// Stamp the village's authored blocks into a freshly generated chunk.
///
/// Pure in `(chunk position)`: the same chunk always receives the same
/// blocks, so regeneration is idempotent and nothing here needs saving.
pub fn stamp(chunk: &mut Chunk, position: ChunkPos, blocks: &TerrainBlocks) {
    let origin = position.origin();
    // Quick reject: does this chunk overlap the buildable square at all?
    let reach = CORE_HALF;
    if origin.x > reach
        || origin.z > reach
        || origin.x + CHUNK_SIZE <= -reach
        || origin.z + CHUNK_SIZE <= -reach
    {
        return;
    }

    for local_z in 0..CHUNK_SIZE {
        for local_x in 0..CHUNK_SIZE {
            let world_x = origin.x + local_x;
            let world_z = origin.z + local_z;
            for layer in 0..max_layers() {
                let world_y = GROUND_Y + layer;
                let Some(cell) = blocks_at(world_x, world_y, world_z) else {
                    continue;
                };
                let block = match cell {
                    Cell::Plank => blocks.plank,
                    Cell::Roof => blocks.roof,
                    Cell::Counter => blocks.counter,
                    Cell::Path => blocks.stone,
                };
                if let Some(local) = LocalPos::new(local_x, world_y, local_z) {
                    chunk.set(local, block);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_core_is_flat_and_the_far_field_is_untouched() {
        for (x, z) in [(0, 0), (26, 26), (-26, 13), (9, -26)] {
            assert_eq!(blend_height(x, z, 95), GROUND_Y, "core column ({x},{z})");
        }
        // Beyond the skirt the natural height passes through bit-for-bit.
        let far = CORE_HALF + SKIRT;
        for (x, z) in [(far, 0), (0, -far), (far + 20, far + 20), (-100, 4)] {
            for natural in [1, 62, 72, 140] {
                assert_eq!(blend_height(x, z, natural), natural, "wild column ({x},{z})");
            }
        }
    }

    #[test]
    fn the_skirt_blends_without_cliffs() {
        // March straight out of town over dramatically higher terrain: no
        // adjacent step may exceed what a drone can climb a few times over.
        for natural in [30, 95, 140] {
            let mut previous = blend_height(CORE_HALF, 0, natural);
            for x in CORE_HALF + 1..CORE_HALF + SKIRT + 4 {
                let here = blend_height(x, 0, natural);
                assert!(
                    (here - previous).abs() <= 5,
                    "cliff of {} at x={x} toward natural {natural}",
                    (here - previous).abs()
                );
                let (lo, hi) = if natural < GROUND_Y {
                    (natural, GROUND_Y)
                } else {
                    (GROUND_Y, natural)
                };
                assert!((lo..=hi).contains(&here), "overshoot at x={x}");
                previous = here;
            }
        }
    }

    #[test]
    fn the_footprint_covers_core_and_skirt_only() {
        assert!(footprint_contains(0, 0));
        assert!(footprint_contains(CORE_HALF + SKIRT - 1, 0));
        assert!(!footprint_contains(CORE_HALF + SKIRT, 0));
        // Corners: Euclidean, so the diagonal corner at most of the skirt's
        // reach in both axes is already outside.
        assert!(!footprint_contains(CORE_HALF + SKIRT - 6, CORE_HALF + SKIRT - 6));
    }

    #[test]
    fn the_counter_stands_inside_the_shop() {
        let counter = counter_position();
        assert_eq!(blocks_at(counter.x, counter.y, counter.z), Some(Cell::Counter));
        // Walls surround it on the shop's outline at the same height.
        assert_eq!(blocks_at(-4, GROUND_Y + 1, 12), Some(Cell::Plank));
    }

    #[test]
    fn spawn_stays_clear_of_the_furniture() {
        // The player appears at (0, 0); nothing authored may stand above the
        // plaza there, and the ground layer is paving, not a wall.
        for layer in 1..max_layers() {
            assert_eq!(blocks_at(0, GROUND_Y + layer, 0), None, "layer {layer}");
        }
        assert_eq!(blocks_at(0, GROUND_Y, 0), Some(Cell::Path));
    }

    #[test]
    fn every_authored_cell_sits_inside_the_core() {
        // A building leaking past the flat core would float or drown.
        for blueprint in BUILDINGS {
            for (layer, rows) in blueprint.layers.iter().enumerate() {
                for (row, line) in rows.iter().enumerate() {
                    for (col, byte) in line.bytes().enumerate() {
                        if byte == b'.' {
                            continue;
                        }
                        let x = blueprint.min.0 + col as i32;
                        let z = blueprint.min.1 + row as i32;
                        assert!(
                            core_contains(x, z),
                            "cell at ({x},{z}) layer {layer} row {row} leaves the core"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn doors_exist_where_the_blueprints_promise() {
        // The shop's south wall has a gap at plaza level.
        assert_eq!(blocks_at(0, GROUND_Y + 1, 6), None, "shop doorway blocked");
        // And each house has its authored gap.
        assert_eq!(blocks_at(-11, GROUND_Y + 1, -1), None, "east-door house");
        assert_eq!(blocks_at(10, GROUND_Y + 1, -1), None, "west-door house");
        assert_eq!(blocks_at(-1, GROUND_Y + 1, -10), None, "north-door house");
    }
}
