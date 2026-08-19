//! Ore deposits, and the surface outcrops that betray them.
//!
//! # Outcrops are not a feature, they are a consequence
//!
//! Deposits are irregular blobs generated at depth. Most never reach the top of
//! the rock and stay completely hidden. A few happen to sit high enough that the
//! blob pushes up through the soil, and *that* is an outcrop — a patch of ore
//! visible on a hillside. Nothing generates outcrops directly; they fall out of
//! burying lumpy shapes at varying heights.
//!
//! This is what makes prospecting honest. The ones you can see are free hints.
//! The scanner's job is finding the ones you cannot. Neither tells you how large
//! the body underneath is, which is why surveying with a drone before committing
//! is a real decision rather than a formality.
//!
//! # Cheap lookup
//!
//! Deposit centres come from a jittered grid: hash each cell of a coarse lattice
//! into a position, radius and presence. That is a pure function of the seed —
//! same discipline as the terrain height field — and it means the deposits near
//! any region can be gathered without keeping a global list.
//!
//! Testing every block against every hashed cell would be far too slow, so
//! callers gather the handful of deposits overlapping a chunk **once**, then test
//! blocks against that short list. Generation does this per chunk.

use vx_core::BlockPos;

use crate::noise::signed_2d;

/// What a deposit is made of. One kind for now; the shape is here so more are
/// data rather than new code paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OreKind {
    Copper,
}

impl OreKind {
    /// The namespaced block this ore places.
    pub fn block_name(self) -> &'static str {
        match self {
            OreKind::Copper => "engine:copper_ore",
        }
    }
}

/// Horizontal spacing of candidate deposits.
const CELL_XZ: i32 = 84;
/// Vertical spacing of candidate deposits.
const CELL_Y: i32 = 40;

/// Deposits are generated within this height band. The upper end deliberately
/// overlaps common terrain heights — that overlap is what produces outcrops.
const BAND_MIN_Y: i32 = 8;
const BAND_MAX_Y: i32 = 104;

const MIN_RADIUS: f32 = 4.0;
const MAX_RADIUS: f32 = 10.5;

/// Fraction of lattice cells that actually hold a deposit. Below 1.0 so ore is
/// worth looking for rather than being everywhere.
const PRESENCE: f32 = 0.42;

/// How far the perturbation can push the boundary, as a fraction of radius.
/// Enough to look geological, not so much that blobs fragment.
const WOBBLE: f32 = 0.30;

/// A single ore body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Deposit {
    pub kind: OreKind,
    /// Centre in world coordinates.
    pub centre: [f32; 3],
    pub radius: f32,
    /// Per-deposit seed, so two deposits of the same size are shaped differently.
    seed: u64,
}

impl Deposit {
    /// The furthest a perturbed boundary can reach from the centre.
    fn reach(&self) -> f32 {
        self.radius * (1.0 + WOBBLE)
    }

    /// Is this block inside the body?
    ///
    /// The boundary is pushed in and out by noise so bodies are lumpy rather
    /// than spherical. Sampling two planes of 2D noise is enough to break up
    /// the silhouette from every viewing angle and costs far less than true 3D
    /// noise.
    pub fn contains(&self, pos: BlockPos) -> bool {
        let dx = pos.x as f32 + 0.5 - self.centre[0];
        let dy = pos.y as f32 + 0.5 - self.centre[1];
        let dz = pos.z as f32 + 0.5 - self.centre[2];
        let distance_squared = dx * dx + dy * dy + dz * dz;

        // Cheap rejection before touching any noise.
        let reach = self.reach();
        if distance_squared > reach * reach {
            return false;
        }

        let distance = distance_squared.sqrt();
        let scale = 0.11;
        let wobble = signed_2d(self.seed, pos.x as f32 * scale, pos.z as f32 * scale) * 0.5
            + signed_2d(
                self.seed ^ 0x5742_4c45,
                pos.y as f32 * scale,
                pos.x as f32 * scale,
            ) * 0.5;

        distance < self.radius * (1.0 + wobble * WOBBLE)
    }
}

/// Hash a lattice cell into a deposit, or nothing.
fn deposit_in_cell(seed: u64, cell_x: i32, cell_y: i32, cell_z: i32) -> Option<Deposit> {
    // One hash stream per property, so changing the radius range cannot shift
    // where deposits sit.
    let key = |salt: u64| -> f32 {
        crate::seed::unit(crate::seed::finalise(
            seed ^ salt
                ^ (cell_x as i64 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
                ^ (cell_y as i64 as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f)
                ^ (cell_z as i64 as u64).wrapping_mul(0x1656_67b1_9e37_79f9),
        ))
    };

    if key(0x01) > PRESENCE {
        return None;
    }

    // Jitter inside the cell so deposits are not on a visible grid.
    let centre_y = cell_y as f32 * CELL_Y as f32 + key(0x03) * CELL_Y as f32;
    if centre_y < BAND_MIN_Y as f32 || centre_y > BAND_MAX_Y as f32 {
        return None;
    }

    Some(Deposit {
        kind: OreKind::Copper,
        centre: [
            cell_x as f32 * CELL_XZ as f32 + key(0x02) * CELL_XZ as f32,
            centre_y,
            cell_z as f32 * CELL_XZ as f32 + key(0x04) * CELL_XZ as f32,
        ],
        radius: MIN_RADIUS + key(0x05) * (MAX_RADIUS - MIN_RADIUS),
        seed: seed ^ ((cell_x as i64 as u64) << 42)
            ^ ((cell_y as i64 as u64) << 21)
            ^ (cell_z as i64 as u64),
    })
}

/// Every deposit that could reach the box `min..=max`.
///
/// Gather once per chunk and reuse; calling this per block would be far slower
/// than the terrain generation it sits inside.
pub fn deposits_overlapping(seed: u64, min: BlockPos, max: BlockPos) -> Vec<Deposit> {
    let margin = (MAX_RADIUS * (1.0 + WOBBLE)).ceil() as i32;

    let lo_x = (min.x - margin).div_euclid(CELL_XZ);
    let hi_x = (max.x + margin).div_euclid(CELL_XZ);
    let lo_y = (min.y - margin).div_euclid(CELL_Y);
    let hi_y = (max.y + margin).div_euclid(CELL_Y);
    let lo_z = (min.z - margin).div_euclid(CELL_XZ);
    let hi_z = (max.z + margin).div_euclid(CELL_XZ);

    let mut found = Vec::new();
    for cell_x in lo_x..=hi_x {
        for cell_y in lo_y..=hi_y {
            for cell_z in lo_z..=hi_z {
                if let Some(deposit) = deposit_in_cell(seed, cell_x, cell_y, cell_z) {
                    found.push(deposit);
                }
            }
        }
    }
    found
}

/// The ore at a position, given deposits already gathered for the area.
pub fn ore_at(deposits: &[Deposit], pos: BlockPos) -> Option<OreKind> {
    deposits
        .iter()
        .find(|deposit| deposit.contains(pos))
        .map(|deposit| deposit.kind)
}

/// How deep an ore body must run to break the surface.
///
/// A body that merely grazes the top would leave a lone speck of ore with
/// nothing beneath it. Players would learn that outcrops mean nothing, and the
/// whole prospecting loop dies. Requiring the blob to hold the blocks below as
/// well means **every visible outcrop leads somewhere**.
pub const OUTCROP_MIN_DEPTH: i32 = 3;

/// May the surface block at `pos` show ore?
///
/// True only when the body also fills `OUTCROP_MIN_DEPTH` blocks straight down.
pub fn breaks_surface(deposits: &[Deposit], pos: BlockPos) -> bool {
    (0..OUTCROP_MIN_DEPTH)
        .all(|depth| ore_at(deposits, BlockPos::new(pos.x, pos.y - depth, pos.z)).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deposits across a wide area, for statistical checks.
    fn survey(seed: u64) -> Vec<Deposit> {
        deposits_overlapping(
            seed,
            BlockPos::new(-1200, 0, -1200),
            BlockPos::new(1200, 128, 1200),
        )
    }

    #[test]
    fn deposit_generation_is_deterministic() {
        assert_eq!(survey(7), survey(7));
        assert_ne!(survey(7), survey(8));
    }

    #[test]
    fn deposits_stay_inside_their_height_band() {
        for deposit in survey(2024) {
            assert!(
                deposit.centre[1] >= BAND_MIN_Y as f32 && deposit.centre[1] <= BAND_MAX_Y as f32,
                "deposit at y={} escaped the band",
                deposit.centre[1]
            );
            assert!((MIN_RADIUS..=MAX_RADIUS).contains(&deposit.radius));
        }
    }

    #[test]
    fn deposits_are_sparse_but_present() {
        // Ore should be worth hunting for: neither absent nor carpeting the map.
        let deposits = survey(11);
        assert!(!deposits.is_empty(), "no deposits generated anywhere");

        // The surveyed box is 2400 x 2400 horizontally.
        let area_cells = (2400 / CELL_XZ) * (2400 / CELL_XZ);
        assert!(
            deposits.len() < (area_cells * (BAND_MAX_Y / CELL_Y + 2)) as usize,
            "more deposits than lattice cells, which is impossible"
        );
    }

    #[test]
    fn gathering_finds_every_deposit_that_reaches_a_region() {
        // The gather margin must be wide enough that no body straddling the
        // edge is missed — a missed one would leave ore abruptly cut off at a
        // chunk boundary.
        let seed = 4242;
        let min = BlockPos::new(0, 0, 0);
        let max = BlockPos::new(16, 128, 16);
        let near = deposits_overlapping(seed, min, max);

        // Gather from a much larger area and keep only those that actually
        // touch the small box; the small gather must contain all of them.
        let wide = deposits_overlapping(
            seed,
            BlockPos::new(-400, -100, -400),
            BlockPos::new(400, 300, 400),
        );
        for deposit in wide {
            let reach = deposit.reach();
            let touches = deposit.centre[0] + reach >= min.x as f32
                && deposit.centre[0] - reach <= max.x as f32
                && deposit.centre[1] + reach >= min.y as f32
                && deposit.centre[1] - reach <= max.y as f32
                && deposit.centre[2] + reach >= min.z as f32
                && deposit.centre[2] - reach <= max.z as f32;
            if touches {
                assert!(
                    near.contains(&deposit),
                    "gather missed a deposit overlapping the region: {deposit:?}"
                );
            }
        }
    }

    #[test]
    fn a_deposit_contains_its_own_centre() {
        for deposit in survey(5).into_iter().take(50) {
            let centre = BlockPos::new(
                deposit.centre[0] as i32,
                deposit.centre[1] as i32,
                deposit.centre[2] as i32,
            );
            assert!(deposit.contains(centre), "deposit is hollow at its centre");
        }
    }

    #[test]
    fn a_deposit_never_reaches_beyond_its_perturbed_radius() {
        for deposit in survey(9).into_iter().take(30) {
            let reach = deposit.reach().ceil() as i32 + 1;
            let centre = BlockPos::new(
                deposit.centre[0] as i32,
                deposit.centre[1] as i32,
                deposit.centre[2] as i32,
            );
            for offset in [-reach, reach] {
                for axis in 0..3 {
                    let mut probe = [centre.x, centre.y, centre.z];
                    probe[axis] += offset;
                    let probe = BlockPos::new(probe[0], probe[1], probe[2]);
                    assert!(
                        !deposit.contains(probe),
                        "ore found {reach} blocks out, past the boundary"
                    );
                }
            }
        }
    }

    #[test]
    fn bodies_are_irregular_rather_than_spheres() {
        // A perfect sphere would mean the wobble is not being applied, and the
        // ore would read as billiard balls buried in the rock.
        let deposit = survey(31337)
            .into_iter()
            .find(|d| d.radius > 7.0)
            .expect("no reasonably large deposit to inspect");

        let centre = [
            deposit.centre[0] as i32,
            deposit.centre[1] as i32,
            deposit.centre[2] as i32,
        ];

        // Walk out along several directions and record where the body ends.
        let mut edges = Vec::new();
        for (dx, dz) in [(1, 0), (0, 1), (-1, 0), (0, -1), (1, 1), (-1, 1)] {
            let mut last_inside = 0;
            for step in 1..40 {
                let probe = BlockPos::new(centre[0] + dx * step, centre[1], centre[2] + dz * step);
                if deposit.contains(probe) {
                    last_inside = step;
                }
            }
            edges.push(last_inside);
        }

        let min = *edges.iter().min().unwrap();
        let max = *edges.iter().max().unwrap();
        assert!(
            max > min,
            "the body is perfectly symmetric ({edges:?}); perturbation is not applied"
        );
    }

    #[test]
    fn ore_lookup_agrees_with_deposit_membership() {
        let deposits = survey(77);
        let deposit = deposits[0];
        let centre = BlockPos::new(
            deposit.centre[0] as i32,
            deposit.centre[1] as i32,
            deposit.centre[2] as i32,
        );

        assert_eq!(ore_at(&deposits, centre), Some(OreKind::Copper));
        // Far from everything.
        assert_eq!(
            ore_at(&deposits, BlockPos::new(500_000, 60, 500_000)),
            None
        );
    }

    #[test]
    fn breaking_the_surface_requires_ore_running_downward() {
        // The invariant the whole mechanic rests on.
        let deposits = survey(2024);
        let deposit = deposits
            .iter()
            .find(|d| d.radius > 7.0)
            .copied()
            .expect("no large deposit to inspect");

        let centre = BlockPos::new(
            deposit.centre[0] as i32,
            deposit.centre[1] as i32,
            deposit.centre[2] as i32,
        );

        // At the very top of the body there is nothing below-and-inside for
        // long, so it must not qualify; deeper in, it must.
        let mut top = centre.y;
        while deposit.contains(BlockPos::new(centre.x, top + 1, centre.z)) {
            top += 1;
        }

        assert!(
            breaks_surface(&[deposit], BlockPos::new(centre.x, top, centre.z)),
            "the top of a large body should have ore beneath it"
        );

        // One block above the body has no ore at all beneath it within range.
        assert!(!breaks_surface(
            &[deposit],
            BlockPos::new(centre.x, top + OUTCROP_MIN_DEPTH + 1, centre.z)
        ));
    }

    #[test]
    fn a_lone_grazing_block_never_counts_as_an_outcrop() {
        // Hand-built: a tiny body barely one block deep. Without the depth
        // requirement this would show a single speck of ore on the surface
        // leading nowhere, which teaches players to ignore outcrops.
        let tiny = Deposit {
            kind: OreKind::Copper,
            centre: [0.5, 60.5, 0.5],
            radius: 0.9,
            seed: 1,
        };
        let surface = BlockPos::new(0, 60, 0);
        assert!(tiny.contains(surface), "the test body should hold its centre");
        assert!(
            !breaks_surface(&[tiny], surface),
            "a one-block body was allowed to surface"
        );
    }
}
