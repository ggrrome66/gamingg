//! What a town is built of: authored blueprints, stamped at a site.
//!
//! Buildings are ASCII layer grids — the cheapest authoring format a human can
//! edit in place and a test can reason about. Layer `i` sits at
//! `site.ground + i`; layer 0 *replaces* the surface block, so floors and
//! paving land on the ground rather than hovering over it. Rows run +z from a
//! blueprint's `min`, characters run +x, and every offset is **relative to the
//! town centre** — which is what lets one set of `&'static` blueprints stamp at
//! any site on the lattice without allocating.
//!
//! Doors and windows are gaps in the wall grid. Nothing is carved afterwards.

use vx_core::{ChunkPos, LocalPos, CHUNK_SIZE};

use crate::chunk::Chunk;
use crate::gen::TerrainBlocks;
use crate::town::{Speciality, TownSite};

/// What an authored cell is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    /// Corrugated container wall.
    Metal,
    /// The same, weathered: the frontier has been out here a while.
    Rusted,
    /// Container roof and catwalk decking.
    Grate,
    /// The radio mast's lattice.
    Mast,
    /// The console the network is worked from.
    Beacon,
    /// The trading counter.
    Counter,
    /// Paving.
    Path,
}

/// One authored building, positioned relative to the town centre.
struct Blueprint {
    min: (i32, i32),
    layers: &'static [&'static [&'static str]],
}

/// The radio tower: the one structure every town on the frontier shares.
///
/// A grated deck, four lattice legs rising well clear of the rooftops, and the
/// beacon console at its foot facing the plaza. `T` mast, `G` grate, `B`
/// console.
const RADIO_TOWER: Blueprint = Blueprint {
    min: (-2, -16),
    layers: &[
        // Deck at ground level.
        &["GGGGG", "GGGGG", "GGGGG", "GGGGG", "GGGGG"],
        // The console stands on the deck's south edge, facing the plaza.
        &["T...T", ".....", ".....", ".....", "T.B.T"],
        &["T...T", ".....", ".....", ".....", "T...T"],
        &["T...T", ".....", ".....", ".....", "T...T"],
        &["T...T", ".....", ".....", ".....", "T...T"],
        &["T...T", ".....", ".....", ".....", "T...T"],
        &["T...T", ".....", ".....", ".....", "T...T"],
        // A service ring part way up.
        &["TGGGT", "G...G", "G...G", "G...G", "TGGGT"],
        &["T...T", ".....", ".....", ".....", "T...T"],
        &["T...T", ".....", ".....", ".....", "T...T"],
        &["T...T", ".....", ".....", ".....", "T...T"],
        &["T...T", ".....", ".....", ".....", "T...T"],
        // The head: aerials clustered at the top.
        &["TTTTT", "T...T", "T...T", "T...T", "TTTTT"],
        &[".T.T.", ".....", ".....", ".....", ".T.T."],
        &["..T..", ".....", ".....", ".....", "..T.."],
    ],
};

/// The supply shed: a container with the trading counter along its back wall.
/// `M` metal, `X` rusted, `G` roof decking, `C` counter.
const SUPPLY_SHED: Blueprint = Blueprint {
    min: (-4, 6),
    layers: &[
        &[
            "GGGGGGGGG",
            "GGGGGGGGG",
            "GGGGGGGGG",
            "GGGGGGGGG",
            "GGGGGGGGG",
            "GGGGGGGGG",
            "GGGGGGGGG",
        ],
        &[
            "MMM...MMM",
            "M.......M",
            "X.......X",
            "M.CCCCC.M",
            "X.......X",
            "M.......M",
            "MMMXMMMXM",
        ],
        &[
            "MMMM.MMMM",
            "M.......X",
            "M.......M",
            "X.......M",
            "M.......X",
            "M.......M",
            "MXMMMXMMM",
        ],
        &[
            "GGGGGGGGG",
            "GGGGGGGGG",
            "GGGGGGGGG",
            "GGGGGGGGG",
            "GGGGGGGGG",
            "GGGGGGGGG",
            "GGGGGGGGG",
        ],
    ],
};

/// A dwelling: one container, door in the east wall.
const CONTAINER_EAST_DOOR: &[&[&str]] = &[
    &["GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG"],
    &["MMMXMMM", "M.....X", "M......", "X......", "M.....M", "MXMMMXM"],
    // The doorway runs two blocks high, or nothing could walk through it.
    &["MMMMMMM", "X.....M", "M......", "M......", "X.....M", "MMMXMMM"],
    &["GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG"],
];

/// The same, door in the west wall.
const CONTAINER_WEST_DOOR: &[&[&str]] = &[
    &["GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG"],
    &["MMMXMMM", "X.....M", "......M", "......X", "M.....M", "MXMMMXM"],
    &["MMMMMMM", "M.....X", "......M", "......M", "M.....X", "MMMXMMM"],
    &["GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG"],
];

/// The same, door in the north wall — and stacked two containers high, with a
/// catwalk over the lower roof.
const CONTAINER_STACK: &[&[&str]] = &[
    &["GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG"],
    &["MMMXMMM", "M.....M", "X.....X", "M.....M", "M.....X", "MM...MM"],
    &["MMMMMMM", "X.....M", "M.....X", "M.....M", "X.....M", "MM...MM"],
    &["GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG"],
    &["XXXMXXX", "X.....X", "M.....M", "X.....X", "M.....M", "XXXXXXX"],
    &["GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG", "GGGGGGG"],
];

/// Paving: an east-west high street past every door and a north-south lane
/// from the tower down to the shed, crossing at the plaza where you wake up.
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

/// The hometown, and the shape every depot follows.
const DEPOT_TOWN: &[Blueprint] = &[
    RADIO_TOWER,
    SUPPLY_SHED,
    Blueprint {
        min: (-17, -3),
        layers: CONTAINER_EAST_DOOR,
    },
    Blueprint {
        min: (10, -3),
        layers: CONTAINER_WEST_DOOR,
    },
    Blueprint {
        min: (-4, -24),
        layers: CONTAINER_STACK,
    },
    PATHS,
];

/// A mining camp: fewer dwellings, stacked bunk containers, same tower.
const MINE_TOWN: &[Blueprint] = &[
    RADIO_TOWER,
    SUPPLY_SHED,
    Blueprint {
        min: (-17, -3),
        layers: CONTAINER_STACK,
    },
    Blueprint {
        min: (10, -3),
        layers: CONTAINER_STACK,
    },
    PATHS,
];

/// A refinery: rusted tanks in place of half the housing.
const REFINERY_TOWN: &[Blueprint] = &[
    RADIO_TOWER,
    SUPPLY_SHED,
    Blueprint {
        min: (-17, -3),
        layers: CONTAINER_WEST_DOOR,
    },
    Blueprint {
        min: (10, -3),
        layers: CONTAINER_STACK,
    },
    Blueprint {
        min: (-4, -24),
        layers: CONTAINER_EAST_DOOR,
    },
    PATHS,
];

/// The buildings a site puts up.
///
/// A plan is picked, never generated: `&'static` throughout, so stamping
/// allocates nothing and the plan stays a pure function of the site.
fn plan_for(site: &TownSite) -> &'static [Blueprint] {
    match site.speciality {
        Speciality::Depot => DEPOT_TOWN,
        Speciality::Mine => MINE_TOWN,
        Speciality::Refinery => REFINERY_TOWN,
    }
}

/// Where this town's counter stands, as an offset from its centre.
pub fn counter_offset(_site: &TownSite) -> (i32, i32) {
    (0, 9)
}

/// Where this town's beacon console stands, as an offset from its centre:
/// the south face of the radio tower's deck.
pub fn beacon_offset(_site: &TownSite) -> (i32, i32) {
    (0, -12)
}

/// The authored cell at a world position for one site, if any.
pub fn cell_at(site: &TownSite, x: i32, y: i32, z: i32) -> Option<Cell> {
    let layer = y - site.ground;
    if layer < 0 {
        return None;
    }
    let (local_x, local_z) = (x - site.centre.0, z - site.centre.1);

    for blueprint in plan_for(site) {
        let Some(rows) = blueprint.layers.get(layer as usize) else {
            continue;
        };
        let (row, col) = (local_z - blueprint.min.1, local_x - blueprint.min.0);
        if row < 0 || col < 0 {
            continue;
        }
        let Some(line) = rows.get(row as usize) else {
            continue;
        };
        match line.as_bytes().get(col as usize) {
            Some(b'M') => return Some(Cell::Metal),
            Some(b'X') => return Some(Cell::Rusted),
            Some(b'G') => return Some(Cell::Grate),
            Some(b'T') => return Some(Cell::Mast),
            Some(b'B') => return Some(Cell::Beacon),
            Some(b'C') => return Some(Cell::Counter),
            Some(b'P') => return Some(Cell::Path),
            _ => continue,
        }
    }
    None
}

/// The same across every gathered site, naming the site that owns the cell.
pub fn cell_at_any(
    sites: &[TownSite],
    x: i32,
    y: i32,
    z: i32,
) -> Option<(&TownSite, Cell)> {
    sites
        .iter()
        .find_map(|site| cell_at(site, x, y, z).map(|cell| (site, cell)))
}

/// The tallest authored layer of a site's plan, for the stamping loop's bound.
fn max_layers(site: &TownSite) -> i32 {
    plan_for(site)
        .iter()
        .map(|blueprint| blueprint.layers.len() as i32)
        .max()
        .unwrap_or(0)
}

/// Stamp every gathered town's authored blocks into a freshly generated chunk.
///
/// Pure in `(chunk position, sites)`: the same chunk always receives the same
/// blocks, so regeneration is idempotent and nothing here is ever saved.
pub fn stamp(chunk: &mut Chunk, position: ChunkPos, sites: &[TownSite], blocks: &TerrainBlocks) {
    let origin = position.origin();

    for site in sites {
        // Quick reject: does this chunk overlap the site's buildable square?
        let reach = site.core_half;
        if origin.x > site.centre.0 + reach
            || origin.z > site.centre.1 + reach
            || origin.x + CHUNK_SIZE <= site.centre.0 - reach
            || origin.z + CHUNK_SIZE <= site.centre.1 - reach
        {
            continue;
        }

        let layers = max_layers(site);
        for local_z in 0..CHUNK_SIZE {
            for local_x in 0..CHUNK_SIZE {
                let world_x = origin.x + local_x;
                let world_z = origin.z + local_z;
                for layer in 0..layers {
                    let world_y = site.ground + layer;
                    let Some(cell) = cell_at(site, world_x, world_y, world_z) else {
                        continue;
                    };
                    let block = match cell {
                        Cell::Metal => blocks.metal_wall,
                        Cell::Rusted => blocks.rusted_metal,
                        Cell::Grate => blocks.catwalk,
                        Cell::Mast => blocks.mast,
                        Cell::Beacon => blocks.beacon,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::town::{self, HOME_GROUND_Y};

    #[test]
    fn the_counter_stands_inside_the_shop() {
        let site = town::home_site();
        let counter = town::counter_position(&site);
        assert_eq!(
            cell_at(&site, counter.x, counter.y, counter.z),
            Some(Cell::Counter)
        );
        // Container walls surround it at the same height.
        assert!(matches!(
            cell_at(&site, -4, HOME_GROUND_Y + 1, 12),
            Some(Cell::Metal | Cell::Rusted)
        ));
    }

    #[test]
    fn spawn_stays_clear_of_the_furniture() {
        // The player appears at the centre; nothing authored may stand there,
        // and the ground under them is paving rather than a wall.
        let site = town::home_site();
        for layer in 1..max_layers(&site) {
            assert_eq!(cell_at(&site, 0, HOME_GROUND_Y + layer, 0), None, "layer {layer}");
        }
        assert_eq!(cell_at(&site, 0, HOME_GROUND_Y, 0), Some(Cell::Path));
    }

    #[test]
    fn every_authored_cell_sits_inside_its_core() {
        // A building leaking past the flat core would float or drown.
        let site = town::home_site();
        for blueprint in plan_for(&site) {
            for (layer, rows) in blueprint.layers.iter().enumerate() {
                for (row, line) in rows.iter().enumerate() {
                    for (col, byte) in line.bytes().enumerate() {
                        if byte == b'.' {
                            continue;
                        }
                        let x = blueprint.min.0 + col as i32;
                        let z = blueprint.min.1 + row as i32;
                        assert!(
                            x.abs() <= site.core_half && z.abs() <= site.core_half,
                            "cell at ({x},{z}) layer {layer} leaves the core"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn doors_exist_where_the_blueprints_promise() {
        let site = town::home_site();
        assert_eq!(cell_at(&site, 0, HOME_GROUND_Y + 1, 6), None, "shed doorway blocked");
        assert_eq!(cell_at(&site, -11, HOME_GROUND_Y + 1, -1), None, "east-door container");
        assert_eq!(cell_at(&site, 10, HOME_GROUND_Y + 1, -1), None, "west-door container");
    }

    #[test]
    fn every_town_has_a_beacon_and_a_counter() {
        // Both are the town's link to the wider game: one to the network, one
        // to the economy. A plan missing either would strand a traveller.
        for speciality in [Speciality::Depot, Speciality::Mine, Speciality::Refinery] {
            let mut site = town::home_site();
            site.speciality = speciality;

            let beacon = town::beacon_position(&site);
            assert_eq!(
                cell_at(&site, beacon.x, beacon.y, beacon.z),
                Some(Cell::Beacon),
                "{speciality:?} has no beacon at its post"
            );

            let counter = town::counter_position(&site);
            assert_eq!(
                cell_at(&site, counter.x, counter.y, counter.z),
                Some(Cell::Counter),
                "{speciality:?} has no counter"
            );
        }
    }

    #[test]
    fn the_radio_tower_stands_well_clear_of_the_rooftops() {
        // It is the landmark you navigate a town by, so it has to clear the
        // containers by a good margin.
        let site = town::home_site();
        let mast_top = (0..40)
            .rev()
            .find(|layer| cell_at(&site, 0, HOME_GROUND_Y + layer, -14) == Some(Cell::Mast))
            .or_else(|| {
                (0..40).rev().find(|layer| {
                    cell_at(&site, -2, HOME_GROUND_Y + layer, -16) == Some(Cell::Mast)
                })
            })
            .expect("no mast anywhere in the hometown");
        assert!(mast_top >= 12, "the mast tops out at only {mast_top} blocks");
    }

    #[test]
    fn a_plan_stamps_the_same_wherever_its_town_sits() {
        // The point of site-relative blueprints: geometry travels with the
        // town rather than being nailed to the origin.
        let home = town::home_site();
        let mut moved = home;
        moved.centre = (1500, -2200);
        moved.ground = 88;

        for (dx, dz, dy) in [(0, 9, 1), (-4, 12, 1), (0, 0, 0), (-11, -1, 1)] {
            let here = cell_at(&home, dx, HOME_GROUND_Y + dy, dz);
            let there = cell_at(&moved, moved.centre.0 + dx, moved.ground + dy, moved.centre.1 + dz);
            assert_eq!(here, there, "offset ({dx},{dz},{dy}) differs between sites");
        }
    }
}
