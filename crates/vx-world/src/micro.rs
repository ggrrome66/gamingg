//! Micro-on-damage: a block gains an interior only once violence touches it.
//!
//! # Detail is allocated by damage
//!
//! The world stays one metre. Worldgen, the town lattice, blueprints, forts,
//! bunkers, flow fields, footings and every block-denominated constant are
//! untouched by this module — a *composite* is a wound on the existing grid,
//! not a resolution increase. Detail therefore costs nothing anywhere the
//! player has not shot, drilled or blasted, and is spent precisely where
//! they are looking hardest.
//!
//! # A damaged block is one `u64`
//!
//! Sixty-four cells, four to a side, one bit each, at bit `x + 4z + 16y` —
//! so each vertical layer is a sixteen-bit slice and the whole wound is one
//! register. Single-material by decree: a chewed stone block is stone with
//! pieces missing, because that is what damage *is*. No sub-palette, no new
//! block ids, no octree.
//!
//! # SWAR, not `PEXT` — the Deck is Zen 2
//!
//! The tempting instructions for bit-plane surgery are `PEXT`/`PDEP`, and on
//! the Steam Deck's Van Gogh APU they are microcoded: tens to hundreds of
//! cycles, fixed only in Zen 3. Everything here is therefore SIMD-within-a-
//! register on plain shifts, ANDs and `popcnt`, which are single-cycle
//! everywhere this game will ever run. No loops over cells, no heap, no
//! pointer in sight — and, because it is all integer arithmetic, the same
//! answer on every machine, which is what lets wounds ride the replay oracle
//! for free.
//!
//! # The journal is already the entropy coder
//!
//! A carve is an impact and a shape, and the journal records orders rather
//! than outcomes — so the minimal encoding of every mask in the world is
//! already on disk before this module writes a byte. Saved masks are a
//! *cache* of what replay would recompute, which is why they get pragmatic
//! compression and not optimal compression: raw masks now, interning when a
//! measurement asks for it, and not one line before.

/// Cells to a side. Four, so a mask is exactly one register.
pub const SIDE: i32 = 4;

/// Cells in a whole block.
pub const CELLS: u32 = 64;

/// Below this many remaining cells the block is gone: less than half a face
/// of material is rubble, not cover.
pub const DEATH_CELLS: u32 = 8;

/// At or above this many cells an untouched wound quietly consolidates back
/// to intact. Frontier stonework gets repaired; the fiction is free, and it
/// is what stops a battlefield becoming a museum of every shot.
pub const HEAL_CELLS: u32 = 56;

/// A block's occupancy: bit `x + 4z + 16y` set means that cell is still
/// there. `!0` is an intact block.
pub type Mask = u64;

/// Every cell present.
pub const FULL: Mask = u64::MAX;

/// Cells with `x == 0`, and the same plane at the far side.
const X0: Mask = 0x1111_1111_1111_1111;
const X3: Mask = X0 << 3;
/// Cells with `z == 0` (bits 0..3 of each sixteen-bit layer), and `z == 3`.
const Z0: Mask = 0x000f_000f_000f_000f;
const Z3: Mask = Z0 << 12;
/// Cells with `y == 0` (the lowest layer), and `y == 3`.
const Y0: Mask = 0x0000_0000_0000_ffff;
const Y3: Mask = Y0 << 48;

/// The bit for one cell. Out-of-range coordinates have no bit.
#[inline]
pub fn bit(x: i32, y: i32, z: i32) -> Mask {
    if !(0..SIDE).contains(&x) || !(0..SIDE).contains(&y) || !(0..SIDE).contains(&z) {
        return 0;
    }
    1u64 << (x + 4 * z + 16 * y)
}

/// Is this cell still there?
#[inline]
pub fn has(mask: Mask, x: i32, y: i32, z: i32) -> bool {
    mask & bit(x, y, z) != 0
}

// The six neighbour shifts. Each clears the wrapping plane first, so a cell
// at the edge has no neighbour off the far side.
#[inline]
pub fn shift_px(mask: Mask) -> Mask {
    (mask & !X3) << 1
}
#[inline]
pub fn shift_nx(mask: Mask) -> Mask {
    (mask & !X0) >> 1
}
#[inline]
pub fn shift_pz(mask: Mask) -> Mask {
    (mask & !Z3) << 4
}
#[inline]
pub fn shift_nz(mask: Mask) -> Mask {
    (mask & !Z0) >> 4
}
#[inline]
pub fn shift_py(mask: Mask) -> Mask {
    (mask & !Y3) << 16
}
#[inline]
pub fn shift_ny(mask: Mask) -> Mask {
    (mask & !Y0) >> 16
}

/// Cells that still have all six neighbours: the interior.
#[inline]
pub fn erode(mask: Mask) -> Mask {
    mask & shift_px(mask)
        & shift_nx(mask)
        & shift_pz(mask)
        & shift_nz(mask)
        & shift_py(mask)
        & shift_ny(mask)
}

/// Cells with at least one open side: everything the mesher has to draw.
#[inline]
pub fn surface(mask: Mask) -> Mask {
    mask & !erode(mask)
}

/// How much of the block is left.
#[inline]
pub fn remaining(mask: Mask) -> u32 {
    mask.count_ones()
}

/// Is what is left too little to be a block at all?
#[inline]
pub fn dead(mask: Mask) -> bool {
    remaining(mask) < DEATH_CELLS
}

/// Is what is left nearly whole enough to consolidate back to intact?
#[inline]
pub fn whole_enough(mask: Mask) -> bool {
    remaining(mask) >= HEAL_CELLS
}

/// The single connected component containing the lowest remaining cell.
///
/// Dilate-and-mask to a fixpoint, entirely in registers: the longest path
/// inside a 4³ is nine steps, so ten iterations is a proof rather than a
/// guess. Anything outside the grown component is a crumb the carve knocked
/// loose, and crumbs are dropped rather than left floating — deterministic,
/// because the seed is the lowest set bit and nothing here rolls a die.
pub fn largest_component(mask: Mask) -> Mask {
    if mask == 0 {
        return 0;
    }
    // Seed: the lowest remaining cell.
    let mut grown = mask & mask.wrapping_neg();
    for _ in 0..10 {
        let spread = (grown
            | shift_px(grown)
            | shift_nx(grown)
            | shift_pz(grown)
            | shift_nz(grown)
            | shift_py(grown)
            | shift_ny(grown))
            & mask;
        if spread == grown {
            break;
        }
        grown = spread;
    }
    grown
}

/// Cells connected to the seed component, with crumbs dropped.
#[inline]
pub fn without_crumbs(mask: Mask) -> Mask {
    largest_component(mask)
}

/// A named damage shape, in cells, centred on an impact.
///
/// The dictionary is deliberately small: the same handful of shapes applied
/// thousands of times is what makes wounds repeat, and repeats are what make
/// them cheap to store and cheap to mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// A slug's bite: a small blob where it struck.
    SlugBite,
    /// One tick of a drill face: the layer of cells nearest the tool.
    DrillFace,
    /// A blast: everything within three cells of the impact.
    Blast,
    /// A feller's face notch, `cells` deep into the struck face.
    ///
    /// Not a layer and not a ball: a wedge that starts low, in the middle,
    /// against the face, and grows — which is the shape a notch actually is,
    /// and which gives the cut a resolution of one cell in sixty-four
    /// instead of the four steps a layer-at-a-time drill has. The felling
    /// rule is written in fractions of the cross-section, so it needs the
    /// finer step to say "a third of the way through" at all.
    Notch { cells: u32 },
}

impl Shape {
    /// The cells this shape removes when it lands at cell `(x, y, z)` of a
    /// block, entering through `face`.
    ///
    /// `face` is a [`vx_core::Face`] index — the same numbering the raycast
    /// reports and the mesher uses, so an impact's face needs no translation
    /// on the way in.
    pub fn cells(self, x: i32, y: i32, z: i32, face: usize) -> Mask {
        match self {
            Shape::SlugBite => ball(x, y, z, 1) | bit(x, y, z),
            Shape::DrillFace => face_layer(face),
            Shape::Blast => ball(x, y, z, 3),
            Shape::Notch { cells } => notch(face, cells),
        }
    }
}

/// Every cell within `radius` of a centre, by squared distance in cells.
fn ball(cx: i32, cy: i32, cz: i32, radius: i32) -> Mask {
    let mut mask = 0;
    let reach = radius * radius;
    for y in 0..SIDE {
        for z in 0..SIDE {
            for x in 0..SIDE {
                let (dx, dy, dz) = (x - cx, y - cy, z - cz);
                if dx * dx + dy * dy + dz * dz <= reach {
                    mask |= bit(x, y, z);
                }
            }
        }
    }
    mask
}

/// The sixteen cells of one face's layer: what a drill takes per tick.
///
/// Face indices follow `vx_core::Face::ALL`: NegX, PosX, NegY, PosY, NegZ,
/// PosZ — axis is `face / 2`, and the odd one is the positive side.
pub fn face_layer(face: usize) -> Mask {
    match face {
        0 => X0,
        1 => X3,
        2 => Y0,
        3 => Y3,
        4 => Z0,
        _ => Z3,
    }
}

/// The `count` cells a notch takes out through `face`.
///
/// Cells are taken in the order a feller's saw reaches them: depth into the
/// block first, then low before high, then near the centre line before the
/// corners. Sixty-four cells means the cut can be spoken of in percentages,
/// which is what the notch and hinge rules are written in.
pub fn notch(face: usize, count: u32) -> Mask {
    if count == 0 {
        return 0;
    }
    let axis = face / 2;
    let from_far = face % 2 == 1;
    let mut taken = 0;
    let mut mask = 0;
    // Keys are small and bounded, so the order comes out of counting rather
    // than a sort — and counting cannot allocate.
    for key in 0..=16 {
        for y in 0..SIDE {
            for z in 0..SIDE {
                for x in 0..SIDE {
                    if taken >= count {
                        return mask;
                    }
                    let along = [x, y, z][axis];
                    let depth = if from_far { SIDE - 1 - along } else { along };
                    // The two axes the notch spreads across: the height it is
                    // cut at, and how far off the centre line it reaches.
                    let across = match axis {
                        0 => z,
                        1 => x,
                        _ => x,
                    };
                    let lateral = if across < SIDE / 2 {
                        SIDE / 2 - 1 - across
                    } else {
                        across - SIDE / 2
                    };
                    let rise = if axis == 1 { z } else { y };
                    // Depth counts least: a saw drives a slot through the
                    // middle of the trunk long before it has taken the
                    // corners, which is why the wood left holding a felled
                    // stem is at the edges rather than the far face.
                    if depth + 2 * lateral + rise != key {
                        continue;
                    }
                    let cell = bit(x, y, z);
                    if mask & cell != 0 {
                        continue;
                    }
                    mask |= cell;
                    taken += 1;
                }
            }
        }
    }
    mask
}

/// How much holding wood is left on the side away from `face`: the hinge, in
/// cells out of the sixteen that layer started with.
///
/// The hinge is what steers a falling tree. When it is gone the tree goes
/// where it is heavy instead of where it was aimed.
#[inline]
pub fn hinge_left(mask: Mask, face: usize) -> u32 {
    remaining(mask & face_layer(face ^ 1))
}

/// Take a shape out of a mask, dropping anything it knocked loose.
///
/// One AND against a precomputed constant, then the register flood fill.
/// Nothing else — this is the whole of "damage" in this game.
#[inline]
pub fn carve(mask: Mask, shape: Mask) -> Mask {
    without_crumbs(mask & !shape)
}

/// Beyond the detail range a wound is not worth a single quad: the block
/// renders whole or gone on a majority of its cells, one instruction
/// deciding one block.
#[inline]
pub fn lod_solid(mask: Mask) -> bool {
    remaining(mask) >= CELLS / 2
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_notch_is_a_wedge_that_grows_from_the_struck_face() {
        for face in 0..6 {
            let mut last = 0u64;
            for count in [1u32, 4, 8, 19, 32, 64] {
                let cut = notch(face, count);
                assert_eq!(
                    remaining(cut),
                    count,
                    "face {face} at {count} cells took {}",
                    remaining(cut)
                );
                // Deeper cuts contain shallower ones: the saw does not
                // un-cut wood it has already been through.
                assert_eq!(cut & last, last, "face {face} notch went backwards");
                last = cut;
            }
            // The first cells are against the struck face, and the last wood
            // to go is the holding wood on the far side.
            assert_eq!(notch(face, 1) & face_layer(face), notch(face, 1));
            assert_eq!(notch(face, 4) & face_layer(face ^ 1), 0);
            assert_eq!(notch(face, 64), FULL);
        }
        assert_eq!(notch(0, 0), 0);
    }

    #[test]
    fn the_hinge_is_the_wood_left_on_the_far_side() {
        // An intact block has its whole hinge; a block cut halfway from one
        // face still has it; only a cut that reaches the far side spends it.
        for face in 0..6 {
            assert_eq!(hinge_left(FULL, face), 16);
            let half = carve(FULL, notch(face, 32));
            assert!(hinge_left(half, face) > 0, "face {face} lost its hinge early");
            let through = carve(FULL, notch(face, 62));
            assert!(
                hinge_left(through, face) <= 2,
                "face {face} kept {} cells of hinge after a cut through",
                hinge_left(through, face)
            );
        }
    }
    use super::*;

    /// Every cell, as coordinates, for the exhaustive tests below.
    fn all_cells() -> impl Iterator<Item = (i32, i32, i32)> {
        (0..SIDE).flat_map(|y| (0..SIDE).flat_map(move |z| (0..SIDE).map(move |x| (x, y, z))))
    }

    #[test]
    fn the_bit_layout_is_the_one_the_note_specifies() {
        // x + 4z + 16y, so a vertical layer is a sixteen-bit slice you can
        // read in hex. Every other kernel here depends on this.
        assert_eq!(bit(0, 0, 0), 1);
        assert_eq!(bit(1, 0, 0), 1 << 1);
        assert_eq!(bit(0, 0, 1), 1 << 4);
        assert_eq!(bit(0, 1, 0), 1 << 16);
        assert_eq!(bit(3, 3, 3), 1 << 63);
        // Out of range has no bit, which is what makes `has` total.
        assert_eq!(bit(-1, 0, 0), 0);
        assert_eq!(bit(4, 0, 0), 0);
        assert_eq!(bit(0, 9, 0), 0);
    }

    #[test]
    fn the_plane_constants_name_the_right_cells() {
        for (x, y, z) in all_cells() {
            assert_eq!(X0 & bit(x, y, z) != 0, x == 0);
            assert_eq!(X3 & bit(x, y, z) != 0, x == 3);
            assert_eq!(Z0 & bit(x, y, z) != 0, z == 0);
            assert_eq!(Z3 & bit(x, y, z) != 0, z == 3);
            assert_eq!(Y0 & bit(x, y, z) != 0, y == 0);
            assert_eq!(Y3 & bit(x, y, z) != 0, y == 3);
        }
    }

    #[test]
    fn the_neighbour_shifts_never_wrap_around_an_edge() {
        // The classic SWAR bug: a shift that carries a cell off one face and
        // back on the other. Checked exhaustively, one cell at a time.
        for (x, y, z) in all_cells() {
            let one = bit(x, y, z);
            assert_eq!(shift_px(one), bit(x + 1, y, z), "px at {x},{y},{z}");
            assert_eq!(shift_nx(one), bit(x - 1, y, z), "nx at {x},{y},{z}");
            assert_eq!(shift_pz(one), bit(x, y, z + 1), "pz at {x},{y},{z}");
            assert_eq!(shift_nz(one), bit(x, y, z - 1), "nz at {x},{y},{z}");
            assert_eq!(shift_py(one), bit(x, y + 1, z), "py at {x},{y},{z}");
            assert_eq!(shift_ny(one), bit(x, y - 1, z), "ny at {x},{y},{z}");
        }
    }

    #[test]
    fn an_intact_block_has_eight_interior_cells_and_the_rest_is_surface() {
        // A 4³ has a 2³ interior; every other cell touches a face. This is
        // the mesher's whole workload, so it is worth pinning by number.
        assert_eq!(remaining(FULL), CELLS);
        assert_eq!(erode(FULL).count_ones(), 8);
        assert_eq!(surface(FULL).count_ones(), CELLS - 8);
        // And an empty block has neither.
        assert_eq!(erode(0), 0);
        assert_eq!(surface(0), 0);
    }

    #[test]
    fn surface_agrees_with_the_slow_definition() {
        // The kernels are fast; this is the obvious version, and they must
        // agree on every mask a carve can produce.
        let slow = |mask: Mask| {
            let mut out = 0;
            for (x, y, z) in all_cells() {
                if !has(mask, x, y, z) {
                    continue;
                }
                let open = [
                    (x + 1, y, z),
                    (x - 1, y, z),
                    (x, y + 1, z),
                    (x, y - 1, z),
                    (x, y, z + 1),
                    (x, y, z - 1),
                ]
                .into_iter()
                .any(|(nx, ny, nz)| !has(mask, nx, ny, nz));
                if open {
                    out |= bit(x, y, z);
                }
            }
            out
        };
        // A spread of real wounds rather than random noise: shapes at every
        // cell, which is what the game will actually produce.
        for face in 0..6 {
            for (x, y, z) in all_cells() {
                for shape in [Shape::SlugBite, Shape::DrillFace, Shape::Blast] {
                    let mask = carve(FULL, shape.cells(x, y, z, face));
                    assert_eq!(surface(mask), slow(mask), "{shape:?} at {x},{y},{z}");
                }
            }
        }
    }

    #[test]
    fn a_carve_never_leaves_a_floating_crumb() {
        // The note's invariant: after any carve the mask is one connected
        // component. Checked against every shape at every cell and face.
        for face in 0..6 {
            for (x, y, z) in all_cells() {
                for shape in [Shape::SlugBite, Shape::DrillFace, Shape::Blast] {
                    let mask = carve(FULL, shape.cells(x, y, z, face));
                    assert_eq!(
                        mask,
                        largest_component(mask),
                        "{shape:?} at {x},{y},{z} left a crumb"
                    );
                }
            }
        }
    }

    #[test]
    fn the_flood_fill_finds_one_component_and_drops_the_rest() {
        // Two cells with a gap between them: only the seed's component
        // survives, and the seed is the lowest bit, so this is decidable
        // without knowing which one "should" win.
        let split = bit(0, 0, 0) | bit(3, 3, 3);
        assert_eq!(largest_component(split), bit(0, 0, 0));

        // A bar that reaches corner to corner the long way round is one
        // component, and ten iterations is enough to prove it: this is the
        // longest path a 4³ can hold.
        let mut snake = 0;
        for x in 0..SIDE {
            snake |= bit(x, 0, 0);
        }
        for z in 1..SIDE {
            snake |= bit(3, 0, z);
        }
        for y in 1..SIDE {
            snake |= bit(3, y, 3);
        }
        assert_eq!(largest_component(snake), snake, "the fixpoint came up short");
    }

    #[test]
    fn a_drill_face_takes_a_whole_layer_and_four_ticks_take_the_block() {
        for face in 0..6 {
            assert_eq!(face_layer(face).count_ones(), 16, "face {face} is not a layer");
        }
        // Four ticks of drilling one face clears the block — which is what
        // makes cell-carving and the old hardness countdown the same amount
        // of work, only visible.
        let mut mask = FULL;
        for tick in 0..4 {
            assert!(!dead(mask), "died after {tick} ticks");
            // Each tick takes the frontmost layer that is still there.
            let layer = (0..4)
                .map(|depth| {
                    let mut plane = 0;
                    for y in 0..SIDE {
                        for z in 0..SIDE {
                            plane |= bit(depth, y, z);
                        }
                    }
                    plane
                })
                .find(|plane| mask & plane != 0)
                .unwrap_or(0);
            mask = carve(mask, layer);
        }
        assert!(dead(mask), "four drill ticks did not finish the block");
    }

    #[test]
    fn death_and_healing_sit_where_the_constants_say() {
        assert!(!dead(FULL));
        assert!(whole_enough(FULL));
        // Exactly at the threshold is alive; one below is not.
        let alive = FULL >> (CELLS - DEATH_CELLS);
        assert_eq!(remaining(alive), DEATH_CELLS);
        assert!(!dead(alive));
        assert!(dead(alive >> 1));
        // And healing needs nearly the whole block.
        let nearly = FULL >> (CELLS - HEAL_CELLS);
        assert!(whole_enough(nearly));
        assert!(!whole_enough(nearly >> 1));
    }

    #[test]
    fn wounds_converge_rather_than_accumulating() {
        // The note's convergence guarantee: unbounded fire at one block
        // reaches air within bounded shots, whatever order the hits land in.
        // No RNG — the sequence is a deterministic walk over the cells.
        let mut mask = FULL;
        let mut shots = 0;
        let mut cell = 0usize;
        while !dead(mask) {
            let x = (cell % 4) as i32;
            let z = ((cell / 4) % 4) as i32;
            let y = ((cell / 16) % 4) as i32;
            mask = carve(mask, Shape::SlugBite.cells(x, y, z, 1));
            cell = cell.wrapping_add(7);
            shots += 1;
            assert!(shots < 500, "a wall outlasted five hundred slugs");
        }
        assert!(dead(mask));
    }

    #[test]
    fn carving_is_deterministic_and_order_independent_for_disjoint_shapes() {
        // Same impact, same shape, same mask — the property the SWAR-only
        // rule buys, and the reason wounds can ride the replay oracle.
        let once = carve(FULL, Shape::Blast.cells(1, 1, 1, 0));
        assert_eq!(once, carve(FULL, Shape::Blast.cells(1, 1, 1, 0)));

        // Two carves that do not knock anything loose commute.
        let a = Shape::SlugBite.cells(0, 3, 0, 3);
        let b = Shape::SlugBite.cells(3, 3, 3, 3);
        assert_eq!(carve(carve(FULL, a), b), carve(carve(FULL, b), a));
    }

    #[test]
    fn distant_wounds_round_to_whole_or_gone() {
        assert!(lod_solid(FULL));
        assert!(!lod_solid(0));
        // Exactly half stays solid: the tie goes to the wall, so a distant
        // battlefield reads as cover rather than as lace.
        let half = FULL >> (CELLS / 2);
        assert_eq!(remaining(half), CELLS / 2);
        assert!(lod_solid(half));
    }
}
