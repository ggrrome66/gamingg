//! Greedy meshing.
//!
//! Drawing one cube per voxel is hopelessly wasteful: a flat stone floor of
//! 16×16 blocks is 256 quads that could be a single one. Greedy meshing sweeps
//! each face direction slice by slice, builds a mask of the faces that are
//! actually visible, and merges adjacent identical entries into the largest
//! rectangles it can.
//!
//! The output is plain vertex/index data with no graphics-API types, so this
//! crate stays testable without a GPU — which matters, since face winding and
//! visibility rules are exactly the things that are painful to debug visually.

use bytemuck::{Pod, Zeroable};
use vx_core::{BlockId, BlockRegistry, Face, Shape, CHUNK_HEIGHT, CHUNK_SIZE};
use vx_world::micro::{self, SIDE};
use vx_world::BlockView;

/// Size of the volume being meshed, per axis.
const DIMS: [i32; 3] = [CHUNK_SIZE, CHUNK_HEIGHT, CHUNK_SIZE];

/// One mesh vertex.
///
/// `uv` spans the merged quad in tile units — a 4×2 merged quad runs 0..4 by
/// 0..2 — so the shader takes `fract(uv)` within the atlas tile named by
/// `tile`. That keeps textures repeating correctly across merged faces.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub tile: u32,
}

/// One quad, packed into eight bytes.
///
/// # Why a quad and not four vertices
///
/// A merged quad currently costs four 36-byte vertices and six 4-byte indices —
/// 168 bytes to say "a rectangle, here, this big, facing this way". But in a
/// blocky world a quad can only sit on the lattice of its own chunk, span a
/// whole number of blocks, and face one of six directions, so all of it fits in
/// a single `u64` with room to spare. The four corners are then synthesised in
/// the vertex shader from `@builtin(vertex_index)`, which needs no vertex data
/// and no index buffer at all.
///
/// # Chunk-local, which is the other half of the point
///
/// Positions are offsets inside the chunk, not world coordinates. Vertices used
/// to carry `(origin + local) as f32`, which baked the `f32` precision wall
/// into the mesh data itself — walk far enough from the origin and the geometry
/// starts falling apart, not just the camera. The chunk's origin now travels
/// separately, once per chunk instead of once per vertex.
///
/// # Layout
///
/// ```text
/// kind:4 | plane:9 | iu:9 | iv:9 | w:9 | h:9 | tile:8 | light:4   = 61 bits
/// ```
///
/// `kind` 0..=5 is a [`Face`] index and `plane`/`iu`/`iv`/`w`/`h` describe a
/// merged rectangle in that face's plane. `kind` 6..=9 are the four crossed
/// quads a plant emits — two diagonals, each with both windings, because the
/// terrain pipeline culls back faces and a plant must carry its own back. Those
/// are always one block square, so they reuse `plane`/`iu`/`iv` as x/y/z.
///
/// Stored as two `u32`s rather than a `u64`: WGSL has no 64-bit integers
/// without an optional feature, and eight bytes is eight bytes either way.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub struct PackedQuad {
    pub low: u32,
    pub high: u32,
}

/// The four crossed quads a plant emits, in the order the mesher pushes them.
pub const CROSS_KINDS: [u32; 4] = [6, 7, 8, 9];

/// Where the micro face kinds begin. `kind` is four bits and cube faces plus
/// plants took 0..=9, so 10..=15 were sitting free: a micro face is the same
/// six faces again, measured in quarter metres.
///
/// This is why micro-on-damage needed no second vertex stream, no second
/// pipeline and no scale uniform. A wounded block's cells ride the terrain
/// buffer beside everything else, and every quad that existed before this
/// round packs to the identical eight bytes it always did.
pub const MICRO_KIND: u32 = 10;

impl PackedQuad {
    /// Pack a cube face. `plane` is the position along the face's own axis;
    /// `iu`/`iv` and `w`/`h` are the rectangle in the other two. `light` is
    /// the face's baked sky exposure, 0 (buried dark) to 15 (open sky).
    ///
    /// Eight arguments because a quad genuinely has eight independent fields;
    /// bundling them into a struct would just move the same list.
    #[allow(clippy::too_many_arguments)]
    pub fn face(kind: u32, plane: i32, iu: i32, iv: i32, w: i32, h: i32, tile: u32, light: u32) -> Self {
        debug_assert!(
            kind < 16,
            "quad kind {kind} is not a face, a cross or a micro face"
        );
        debug_assert!(light < 16, "light {light} does not fit its nibble");
        let bits = (kind as u64)
            | ((plane as u64 & 0x1ff) << 4)
            | ((iu as u64 & 0x1ff) << 13)
            | ((iv as u64 & 0x1ff) << 22)
            | ((w as u64 & 0x1ff) << 31)
            | ((h as u64 & 0x1ff) << 40)
            | ((tile as u64 & 0xff) << 49)
            | ((light as u64 & 0xf) << 57);
        PackedQuad {
            low: bits as u32,
            high: (bits >> 32) as u32,
        }
    }

    /// Pack one quarter-metre face of a wounded block's cell.
    ///
    /// `plane`/`iu`/`iv` stay in *block* coordinates, exactly as a full face
    /// records them; where the face sits inside the block rides in the bits
    /// `w` and `h` no longer need. A micro rectangle spans at most four
    /// cells, so three bits of each nine-bit field carry the size and the
    /// rest carry the sub-cell offsets:
    ///
    /// ```text
    /// w field: [ w_cells:3 | sub_u:2 | sub_plane:3 ]
    /// h field: [ h_cells:3 | sub_v:2 ]
    /// ```
    ///
    /// `sub_plane` runs 0..=4 rather than 0..=3 because a face may sit on
    /// the far side of the last cell — four quarters is the whole block.
    #[allow(clippy::too_many_arguments)]
    pub fn micro(
        face: u32,
        plane: i32,
        iu: i32,
        iv: i32,
        sub_plane: u32,
        sub_u: u32,
        sub_v: u32,
        cells_u: u32,
        cells_v: u32,
        tile: u32,
        light: u32,
    ) -> Self {
        debug_assert!(face < 6, "micro face {face} is not a cube face");
        debug_assert!(sub_plane <= 4, "sub-plane {sub_plane} is off the block");
        debug_assert!(sub_u < 4 && sub_v < 4, "sub-cell offset off the block");
        debug_assert!(
            (1..=4).contains(&cells_u) && (1..=4).contains(&cells_v),
            "a micro rectangle spans one to four cells"
        );
        let w = cells_u | (sub_u << 3) | (sub_plane << 5);
        let h = cells_v | (sub_v << 3);
        PackedQuad::face(
            MICRO_KIND + face,
            plane,
            iu,
            iv,
            w as i32,
            h as i32,
            tile,
            light,
        )
    }

    /// Pack one of a plant's crossed quads at a block.
    pub fn cross(variant: usize, x: i32, y: i32, z: i32, tile: u32, light: u32) -> Self {
        PackedQuad::face(CROSS_KINDS[variant], y, x, z, 1, 1, tile, light)
    }

    fn bits(self) -> u64 {
        u64::from(self.low) | (u64::from(self.high) << 32)
    }

    pub fn kind(self) -> u32 {
        (self.bits() & 0xf) as u32
    }

    pub fn plane(self) -> i32 {
        ((self.bits() >> 4) & 0x1ff) as i32
    }

    pub fn iu(self) -> i32 {
        ((self.bits() >> 13) & 0x1ff) as i32
    }

    pub fn iv(self) -> i32 {
        ((self.bits() >> 22) & 0x1ff) as i32
    }

    pub fn width(self) -> i32 {
        ((self.bits() >> 31) & 0x1ff) as i32
    }

    pub fn height(self) -> i32 {
        ((self.bits() >> 40) & 0x1ff) as i32
    }

    pub fn tile(self) -> u32 {
        ((self.bits() >> 49) & 0xff) as u32
    }

    /// Baked sky exposure, 0..=15.
    pub fn light(self) -> u32 {
        ((self.bits() >> 57) & 0xf) as u32
    }

    pub fn is_cross(self) -> bool {
        self.kind() >= 6
    }

    /// The four corners, in winding order, relative to the chunk origin.
    ///
    /// The inverse of packing, and what the vertex shader reimplements. Kept
    /// here so a test can hold the two to the same answer without a GPU.
    pub fn corners(self) -> [[f32; 3]; 4] {
        if self.is_cross() {
            let (x, y, z) = (self.iu() as f32, self.plane() as f32, self.iv() as f32);
            let diagonal = if (self.kind() - 6) / 2 == 0 {
                [
                    [x, y + 1.0, z],
                    [x + 1.0, y + 1.0, z + 1.0],
                    [x + 1.0, y, z + 1.0],
                    [x, y, z],
                ]
            } else {
                [
                    [x + 1.0, y + 1.0, z],
                    [x, y + 1.0, z + 1.0],
                    [x, y, z + 1.0],
                    [x + 1.0, y, z],
                ]
            };
            // The odd variant of each pair is the same quad wound backwards.
            return if (self.kind() - 6).is_multiple_of(2) {
                diagonal
            } else {
                [diagonal[3], diagonal[2], diagonal[1], diagonal[0]]
            };
        }

        let face = Face::ALL[self.kind() as usize];
        let d = face.axis();
        let u = (d + 1) % 3;
        let v = (d + 2) % 3;
        let corner = |a: i32, b: i32| {
            let mut local = [0i32; 3];
            local[d] = self.plane();
            local[u] = a;
            local[v] = b;
            [local[0] as f32, local[1] as f32, local[2] as f32]
        };
        let (u0, v0) = (self.iu(), self.iv());
        let (u1, v1) = (u0 + self.width(), v0 + self.height());
        if face.is_positive() {
            [
                corner(u0, v0),
                corner(u1, v0),
                corner(u1, v1),
                corner(u0, v1),
            ]
        } else {
            [
                corner(u0, v0),
                corner(u0, v1),
                corner(u1, v1),
                corner(u1, v0),
            ]
        }
    }

    /// The outward normal.
    pub fn normal(self) -> [f32; 3] {
        if self.is_cross() {
            // Lit as if lying flat: plants take the ground's light, and neither
            // diagonal ever shows a dark "back of the leaf".
            return [0.0, 1.0, 0.0];
        }
        Face::ALL[self.kind() as usize].normal()
    }

    /// The `uv` at each corner, spanning the merged quad in tile units.
    pub fn uvs(self) -> [[f32; 2]; 4] {
        let (w, h) = (self.width() as f32, self.height() as f32);
        [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
    }
}

/// Renderable geometry for one chunk: packed quads, chunk-local.
///
/// There is no vertex or index data here any more. The renderer uploads these
/// eight-byte quads directly and the vertex shader synthesises the six vertices
/// of each quad's two triangles from `vertex_index`, so building four vertices
/// and six indices on the CPU would be work thrown away.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub quads: Vec<PackedQuad>,
}

impl Mesh {
    pub fn is_empty(&self) -> bool {
        self.quads.is_empty()
    }

    pub fn quad_count(&self) -> usize {
        self.quads.len()
    }

    /// Each quad is two triangles.
    pub fn triangle_count(&self) -> usize {
        self.quads.len() * 2
    }

    /// Expand into the vertex and index arrays the GPU used to be given.
    ///
    /// Nothing in the render path calls this — the shader does the same work
    /// from packed quads. It exists because geometry is far easier to *check*
    /// as explicit triangles than as bit fields, so the mesher's tests read
    /// through it, and any future tool that wants real triangles (an exporter,
    /// a collision bake) has them without reimplementing the unpacking.
    ///
    /// `origin` is the chunk's corner, added back because packed positions are
    /// chunk-local.
    pub fn to_vertices(&self, origin: [i32; 3]) -> (Vec<Vertex>, Vec<u32>) {
        let mut vertices = Vec::with_capacity(self.quads.len() * 4);
        let mut indices = Vec::with_capacity(self.quads.len() * 6);
        for quad in &self.quads {
            let base = vertices.len() as u32;
            let normal = quad.normal();
            let tile = quad.tile();
            for (corner, uv) in quad.corners().into_iter().zip(quad.uvs()) {
                vertices.push(Vertex {
                    position: [
                        corner[0] + origin[0] as f32,
                        corner[1] + origin[1] as f32,
                        corner[2] + origin[2] as f32,
                    ],
                    normal,
                    uv,
                    tile,
                });
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        (vertices, indices)
    }

    fn push_quad(&mut self, packed: PackedQuad) {
        self.quads.push(packed);
    }
}

/// Should this block show a face toward `neighbour`?
/// Full daylight, the top of the light nibble.
pub const FULL_LIGHT: u32 = 15;

/// Depth below a column's roof at which the last light dies.
const LIGHT_REACH: f32 = 18.0;

/// Baked sky exposure for a cell `depth` blocks beneath the topmost opaque
/// block of its column. Zero or negative depth is open sky.
///
/// The curve is gentle at first and steep later on purpose: two or three
/// blocks of cover — a house interior, the shade under a canopy — reads as
/// shade, while a gallery twenty blocks down is genuinely black. Column
/// depth is an approximation of enclosure, not a light transport model; it
/// is cheap, pure, and rebakes for free whenever a chunk remeshes, which is
/// exactly when digging changes what the sky can reach.
pub fn sky_light(depth: i32) -> u32 {
    if depth <= 0 {
        return FULL_LIGHT;
    }
    let fade = 1.0 - (depth as f32 / LIGHT_REACH).powf(1.2);
    (FULL_LIGHT as f32 * fade.max(0.0)).round() as u32
}

/// The topmost opaque block per column, for the chunk plus a one-block ring —
/// every column a face's *air* neighbour can sit in. `i32::MIN` marks a column
/// with no opaque block at all.
///
/// Opaque, not solid: water covers the sea floor without plunging it into
/// night, and a pane of anything see-through would behave the same way.
struct ColumnRoofs {
    min_x: i32,
    min_z: i32,
    top: Vec<i32>,
}

const ROOF_SPAN: i32 = CHUNK_SIZE + 2;

impl ColumnRoofs {
    fn survey(view: &impl BlockView, registry: &BlockRegistry, origin: [i32; 3]) -> Self {
        let (min_x, min_z) = (origin[0] - 1, origin[2] - 1);
        let mut top = vec![i32::MIN; (ROOF_SPAN * ROOF_SPAN) as usize];
        for dz in 0..ROOF_SPAN {
            for dx in 0..ROOF_SPAN {
                let (x, z) = (min_x + dx, min_z + dz);
                for y in (0..CHUNK_HEIGHT).rev() {
                    if registry.is_opaque(view.block_at(x, y, z)) {
                        top[(dz * ROOF_SPAN + dx) as usize] = y;
                        break;
                    }
                }
            }
        }
        ColumnRoofs { min_x, min_z, top }
    }

    /// Sky exposure of the (air) cell at a world position.
    fn light_at(&self, x: i32, y: i32, z: i32) -> u32 {
        let (dx, dz) = (x - self.min_x, z - self.min_z);
        debug_assert!(
            (0..ROOF_SPAN).contains(&dx) && (0..ROOF_SPAN).contains(&dz),
            "light sampled outside the surveyed ring at ({x}, {z})"
        );
        let roof = self.top[(dz * ROOF_SPAN + dx) as usize];
        if roof == i32::MIN {
            return FULL_LIGHT;
        }
        sky_light(roof - y)
    }
}

fn face_is_visible(registry: &BlockRegistry, block: BlockId, neighbour: BlockId) -> bool {
    if block.is_air() {
        return false;
    }
    // Cross blocks have no cube faces; their geometry comes from the cross
    // pass, and they must never merge into the greedy sweep.
    if registry.get_or_air(block).shape == Shape::Cross {
        return false;
    }
    // A fully opaque neighbour hides this face entirely.
    if registry.is_opaque(neighbour) {
        return false;
    }
    // Two touching blocks of the same see-through type (water against water)
    // would otherwise draw a face between them inside the volume.
    if block == neighbour && !registry.is_opaque(block) {
        return false;
    }
    true
}

/// Build a mesh for the chunk at `chunk_origin` by reading `view`.
///
/// `view` is read in world coordinates and must cover one block beyond the
/// chunk on every side, so seam faces can be culled against real neighbours.
pub fn build_mesh(view: &impl BlockView, registry: &BlockRegistry, chunk_origin: [i32; 3]) -> Mesh {
    let mut mesh = Mesh::default();
    let roofs = ColumnRoofs::survey(view, registry, chunk_origin);
    for face in Face::ALL {
        mesh_face_direction(&mut mesh, view, registry, chunk_origin, face, &roofs);
    }
    mesh_cross_blocks(&mut mesh, view, registry, chunk_origin, &roofs);
    mesh_wounds(&mut mesh, view, registry, chunk_origin, &roofs);
    mesh
}

/// Emit the quarter-metre faces of every wounded block in the chunk.
///
/// One quad per exposed cell face. No greedy merge here on purpose: wounds
/// are rare by construction, and the cheap thing to do with a rare case is
/// the simple thing. If a measurement ever says otherwise, the note's intern
/// table is the answer — each distinct mask meshes once, ever — and it can
/// arrive without this function's callers changing.
fn mesh_wounds(
    mesh: &mut Mesh,
    view: &impl BlockView,
    registry: &BlockRegistry,
    origin: [i32; 3],
    roofs: &ColumnRoofs,
) {
    for y in 0..CHUNK_HEIGHT {
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let world = [origin[0] + x, origin[1] + y, origin[2] + z];
                let Some(mask) = view.mask_at(world[0], world[1], world[2]) else {
                    continue;
                };
                let block = view.block_at(world[0], world[1], world[2]);
                if block.is_air() {
                    continue;
                }
                let def = registry.get_or_air(block);
                // A cell face takes the light of whatever it looks into —
                // the same rule the metre-scale sweep uses. A face on the
                // block's skin looks at outside air and is lit like the wall
                // around it; a face lining a crater looks into the block's
                // own buried column and comes out dim, which is what makes a
                // wound read as depth rather than as a hole punched through.
                // Faces lining a cavity take the block's own column light,
                // knocked down a few steps. Without the knock-down a wound
                // in a thin wall reads as a hole punched clean through: the
                // column is near the surface, so its light is nearly full
                // daylight, and a brightly lit crater floor is indis-
                // tinguishable from sky. This is the one deliberately
                // unphysical line in the round, and it is what makes damage
                // legible.
                const CAVITY_SHADE: u32 = 5;
                let inside_light = roofs
                    .light_at(world[0], world[1], world[2])
                    .saturating_sub(CAVITY_SHADE);

                for (index, face) in Face::ALL.into_iter().enumerate() {
                    let axis = face.axis();
                    let step = face.offset();
                    let tile = def.texture(face) as u32;
                    // The neighbouring block decides whether cells on this
                    // block's own boundary are exposed at all.
                    let outside_open = !registry.is_opaque(view.block_at(
                        world[0] + step[0],
                        world[1] + step[1],
                        world[2] + step[2],
                    ));
                    let outside_light =
                        roofs.light_at(world[0] + step[0], world[1] + step[1], world[2] + step[2]);

                    for cell in 0..(SIDE * SIDE * SIDE) {
                        let cx = cell % SIDE;
                        let cz = (cell / SIDE) % SIDE;
                        let cy = cell / (SIDE * SIDE);
                        if !micro::has(mask, cx, cy, cz) {
                            continue;
                        }
                        let (nx, ny, nz) = (cx + step[0], cy + step[1], cz + step[2]);
                        let inside = (0..SIDE).contains(&nx)
                            && (0..SIDE).contains(&ny)
                            && (0..SIDE).contains(&nz);
                        // Exposed when the cell beyond is gone, or when this
                        // cell is on the block's skin and the block beyond
                        // does not cover it.
                        let (exposed, light) = if inside {
                            (!micro::has(mask, nx, ny, nz), inside_light)
                        } else {
                            (outside_open, outside_light)
                        };
                        if !exposed {
                            continue;
                        }

                        let local = [x, y, z];
                        let cells = [cx, cy, cz];
                        let u = (axis + 1) % 3;
                        let v = (axis + 2) % 3;
                        // The face sits on the far side of the cell for a
                        // positive face, and on the near side otherwise.
                        let sub_plane = cells[axis] + i32::from(face.is_positive());

                        mesh.push_quad(PackedQuad::micro(
                            index as u32,
                            local[axis],
                            local[u],
                            local[v],
                            sub_plane as u32,
                            cells[u] as u32,
                            cells[v] as u32,
                            1,
                            1,
                            tile,
                            light,
                        ));
                    }
                }
            }
        }
    }
}

/// Emit the crossed-quad plants.
///
/// Each cross block becomes two diagonal quads, and each quad is pushed twice
/// with reversed winding: the terrain pipeline culls back faces, so a plant
/// must carry its own back or vanish from half the compass.
fn mesh_cross_blocks(
    mesh: &mut Mesh,
    view: &impl BlockView,
    registry: &BlockRegistry,
    origin: [i32; 3],
    roofs: &ColumnRoofs,
) {
    for y in 0..CHUNK_HEIGHT {
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let block = view.block_at(origin[0] + x, origin[1] + y, origin[2] + z);
                if block.is_air() {
                    continue;
                }
                let def = registry.get_or_air(block);
                if def.shape != Shape::Cross {
                    continue;
                }
                let tile = def.texture(Face::PosX) as u32;
                // A plant is see-through, so its own cell's exposure is the
                // light falling on it.
                let light = roofs.light_at(origin[0] + x, origin[1] + y, origin[2] + z);
                for diagonal in 0..2 {
                    // Each diagonal is pushed twice with reversed winding: the
                    // terrain pipeline culls back faces, so a plant must carry
                    // its own back or vanish from half the compass.
                    mesh.push_quad(PackedQuad::cross(diagonal * 2, x, y, z, tile, light));
                    mesh.push_quad(PackedQuad::cross(diagonal * 2 + 1, x, y, z, tile, light));
                }
            }
        }
    }
}

/// Sweep one face direction, slice by slice.
fn mesh_face_direction(
    mesh: &mut Mesh,
    view: &impl BlockView,
    registry: &BlockRegistry,
    origin: [i32; 3],
    face: Face,
    roofs: &ColumnRoofs,
) {
    let d = face.axis();
    // u and v are the two axes spanning the face plane. Choosing them
    // cyclically means u × v = +d, which is what makes the winding below
    // produce an outward normal.
    let u = (d + 1) % 3;
    let v = (d + 2) % 3;

    let (du, dv, dd) = (DIMS[u], DIMS[v], DIMS[d]);
    let offset = face.offset();

    // Each visible face carries the sky exposure of the air cell it faces —
    // that is the cell the light actually stands in. Part of the merge key:
    // a rectangle spanning a light change would smear one value across both.
    let mut mask: Vec<Option<(BlockId, u32)>> = vec![None; (du * dv) as usize];

    for slice in 0..dd {
        // Build the visibility mask for this slice.
        for iv in 0..dv {
            for iu in 0..du {
                let mut local = [0i32; 3];
                local[d] = slice;
                local[u] = iu;
                local[v] = iv;

                let world = [
                    origin[0] + local[0],
                    origin[1] + local[1],
                    origin[2] + local[2],
                ];
                let block = view.block_at(world[0], world[1], world[2]);
                let neighbour = view.block_at(
                    world[0] + offset[0],
                    world[1] + offset[1],
                    world[2] + offset[2],
                );

                // A wounded block has no full faces — the micro pass draws
                // its cells — and it no longer hides the block behind it,
                // because you can see through what has been chewed.
                let wounded = view.mask_at(world[0], world[1], world[2]).is_some();
                let neighbour_wounded = view
                    .mask_at(
                        world[0] + offset[0],
                        world[1] + offset[1],
                        world[2] + offset[2],
                    )
                    .is_some();

                mask[(iv * du + iu) as usize] = (!wounded
                    && (neighbour_wounded || face_is_visible(registry, block, neighbour))
                    && !block.is_air()
                    && registry.get_or_air(block).shape != Shape::Cross)
                    .then(|| {
                        let light = roofs.light_at(
                            world[0] + offset[0],
                            world[1] + offset[1],
                            world[2] + offset[2],
                        );
                        (block, light)
                    });
            }
        }

        emit_merged_quads(&mut mask, du, dv, |iu, iv, w, h, (block, light)| {
            let tile = registry.get_or_air(block).texture(face) as u32;

            // The quad sits on the far side of the voxel for positive faces.
            let plane = slice + i32::from(face.is_positive());

            mesh.push_quad(PackedQuad::face(
                Face::ALL.iter().position(|other| *other == face).unwrap() as u32,
                plane,
                iu,
                iv,
                w,
                h,
                tile,
                light,
            ));
        });
    }
}

/// Merge the mask into maximal rectangles, calling `emit` for each.
///
/// Consumed entries are cleared as it goes, so every face is emitted once.
fn emit_merged_quads<K: Copy + PartialEq>(
    mask: &mut [Option<K>],
    du: i32,
    dv: i32,
    mut emit: impl FnMut(i32, i32, i32, i32, K),
) {
    for iv in 0..dv {
        let mut iu = 0;
        while iu < du {
            let index = (iv * du + iu) as usize;
            let Some(block) = mask[index] else {
                iu += 1;
                continue;
            };

            // Grow along u while the block matches.
            let mut width = 1;
            while iu + width < du && mask[(iv * du + iu + width) as usize] == Some(block) {
                width += 1;
            }

            // Then grow along v, but only in whole rows of that width, so the
            // result stays rectangular.
            let mut height = 1;
            'grow: while iv + height < dv {
                for offset in 0..width {
                    if mask[((iv + height) * du + iu + offset) as usize] != Some(block) {
                        break 'grow;
                    }
                }
                height += 1;
            }

            emit(iu, iv, width, height, block);

            // Clear the consumed rectangle.
            for row in 0..height {
                for col in 0..width {
                    mask[((iv + row) * du + iu + col) as usize] = None;
                }
            }

            iu += width;
        }
    }
}

#[cfg(test)]
mod wound_tests {
    use super::*;
    use vx_core::{BlockDef, BlockId, BlockRegistry};

    /// A single block at the chunk origin, optionally wounded, in open air.
    struct OneBlock {
        mask: Option<micro::Mask>,
    }

    impl BlockView for OneBlock {
        fn block_at(&self, x: i32, y: i32, z: i32) -> BlockId {
            if (x, y, z) == (0, 0, 0) {
                BlockId(1)
            } else {
                BlockId::AIR
            }
        }
        fn mask_at(&self, x: i32, y: i32, z: i32) -> Option<micro::Mask> {
            ((x, y, z) == (0, 0, 0)).then_some(self.mask).flatten()
        }
    }

    fn registry() -> BlockRegistry {
        let mut registry = BlockRegistry::new();
        registry.register(BlockDef::uniform("test:stone", 0)).unwrap();
        registry
    }

    fn quads_by_kind(mask: Option<micro::Mask>) -> std::collections::BTreeMap<u32, usize> {
        let mesh = build_mesh(&OneBlock { mask }, &registry(), [0, 0, 0]);
        let mut counts = std::collections::BTreeMap::new();
        for quad in &mesh.quads {
            *counts.entry(quad.kind()).or_insert(0) += 1;
        }
        counts
    }

    #[test]
    fn an_intact_block_meshes_exactly_as_it_always_did() {
        // The floor the whole round stands on: a world nobody has damaged
        // produces the same six full faces and not one micro quad.
        let counts = quads_by_kind(None);
        assert_eq!(counts.values().sum::<usize>(), 6, "not six faces: {counts:?}");
        assert!(
            counts.keys().all(|kind| *kind < MICRO_KIND),
            "an intact block emitted micro geometry: {counts:?}"
        );
    }

    #[test]
    fn a_wounded_block_draws_its_cells_and_no_full_faces() {
        // An untouched *mask* still covers every side, so the block must
        // show sixteen cell faces per side and nothing at metre scale.
        let counts = quads_by_kind(Some(micro::FULL));
        assert!(
            counts.keys().all(|kind| *kind >= MICRO_KIND),
            "a wounded block still drew a full face: {counts:?}"
        );
        for face in 0..6 {
            assert_eq!(
                counts.get(&(MICRO_KIND + face)).copied().unwrap_or(0),
                16,
                "face {face} did not draw its sixteen cells: {counts:?}"
            );
        }
    }

    #[test]
    fn carving_removes_the_cells_it_took_and_opens_the_ones_behind() {
        // Take the whole front layer, as a drill tick does. The face that
        // pointed at the drill loses all sixteen cells; the layer behind it
        // is now exposed and picks them up.
        let face = 4u32; // NegZ, the layer at z = 0.
        let mask = micro::carve(micro::FULL, micro::face_layer(face as usize));
        let counts = quads_by_kind(Some(mask));
        assert_eq!(
            counts.get(&(MICRO_KIND + face)).copied().unwrap_or(0),
            16,
            "the drilled face should still show sixteen cells, one layer deeper"
        );
        // And the sides lost a column each: fifteen cells a side would be
        // wrong, twelve is right — four cells of each side went with the
        // layer.
        for side in [0u32, 1, 2, 3] {
            assert_eq!(
                counts.get(&(MICRO_KIND + side)).copied().unwrap_or(0),
                12,
                "side {side} kept cells the drill took: {counts:?}"
            );
        }
    }

    /// The four corners a quad becomes, computed exactly as `shader.wgsl`
    /// computes them. If this and the shader ever disagree the geometry
    /// lands in the wrong place, which is a class of bug no unit test on the
    /// mesher alone can see — so the arithmetic is mirrored here and pinned.
    fn shader_corners(quad: PackedQuad) -> [[f32; 3]; 4] {
        let raw_kind = quad.kind();
        let mut plane = quad.plane() as f32;
        let mut iu = quad.iu() as f32;
        let mut iv = quad.iv() as f32;
        let w_bits = quad.width() as u32;
        let h_bits = quad.height() as u32;
        let mut w = w_bits as f32;
        let mut h = h_bits as f32;

        let kind = if raw_kind >= MICRO_KIND {
            let cells_u = (w_bits & 7) as f32;
            let sub_u = ((w_bits >> 3) & 3) as f32;
            let sub_plane = ((w_bits >> 5) & 7) as f32;
            let cells_v = (h_bits & 7) as f32;
            let sub_v = ((h_bits >> 3) & 3) as f32;
            plane += sub_plane * 0.25;
            iu += sub_u * 0.25;
            iv += sub_v * 0.25;
            w = cells_u * 0.25;
            h = cells_v * 0.25;
            raw_kind - MICRO_KIND
        } else {
            raw_kind
        };

        let axis = kind / 2;
        let corners = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
        corners.map(|(du, dv)| {
            let a = iu + du;
            let b = iv + dv;
            match axis {
                0 => [plane, a, b],
                1 => [b, plane, a],
                _ => [a, b, plane],
            }
        })
    }

    #[test]
    fn micro_faces_land_exactly_on_the_cell_they_belong_to() {
        // Walk every cell and every face of a wounded block and check the
        // quad the mesher emits covers precisely that cell's quarter-metre
        // face — the right plane, and the right square within it.
        let mask = micro::FULL;
        let mesh = build_mesh(&OneBlock { mask: Some(mask) }, &registry(), [0, 0, 0]);
        let cell = 1.0 / micro::SIDE as f32;

        for quad in &mesh.quads {
            let face = (quad.kind() - MICRO_KIND) as usize;
            let axis = face / 2;
            let positive = face % 2 == 1;
            let corners = shader_corners(*quad);

            // Flat in its own axis, and on a quarter-metre plane.
            let depth = corners[0][axis];
            assert!(
                corners.iter().all(|corner| corner[axis] == depth),
                "face {face} is not flat: {corners:?}"
            );
            let quarters = depth / cell;
            assert!(
                (quarters - quarters.round()).abs() < 1.0e-4,
                "face {face} sits at {depth}, off the cell grid"
            );
            // On the block's skin, on the correct side.
            assert_eq!(
                depth,
                if positive { 1.0 } else { 0.0 },
                "face {face} of an intact mask should sit on the block's skin"
            );
            // And a quarter of a metre square in the other two axes.
            for other in 0..3 {
                if other == axis {
                    continue;
                }
                let low = corners.iter().map(|c| c[other]).fold(f32::MAX, f32::min);
                let high = corners.iter().map(|c| c[other]).fold(f32::MIN, f32::max);
                assert!(
                    (high - low - cell).abs() < 1.0e-4,
                    "face {face} spans {} on axis {other}, not a cell",
                    high - low
                );
                assert!(
                    (0.0..=1.0).contains(&low) && (0.0..=1.0).contains(&high),
                    "face {face} left the block on axis {other}: {low}..{high}"
                );
            }
        }
    }

    #[test]
    fn a_recessed_face_sits_where_the_material_actually_is() {
        // Drill one layer off the front. The face the player now looks at
        // must sit a quarter of a metre deeper, not on the old skin.
        let face = 4usize; // NegZ
        let mask = micro::carve(micro::FULL, micro::face_layer(face));
        let mesh = build_mesh(&OneBlock { mask: Some(mask) }, &registry(), [0, 0, 0]);
        let fronts: Vec<f32> = mesh
            .quads
            .iter()
            .filter(|quad| quad.kind() == MICRO_KIND + face as u32)
            .map(|quad| shader_corners(*quad)[0][2])
            .collect();
        assert_eq!(fronts.len(), 16);
        for depth in fronts {
            assert!(
                (depth - 0.25).abs() < 1.0e-4,
                "the drilled face drew at {depth} rather than one cell in"
            );
        }
    }

    #[test]
    fn every_micro_quad_round_trips_through_the_packing() {
        // The packing steals bits from `w`/`h`, so a mistake there is a
        // silently misplaced face rather than an error. Check the fields
        // come back out as they went in.
        for face in 0..6u32 {
            for sub_plane in 0..=4u32 {
                for sub in 0..4u32 {
                    let quad =
                        PackedQuad::micro(face, 9, 3, 7, sub_plane, sub, 3 - sub, 2, 4, 5, 11);
                    assert_eq!(quad.kind(), MICRO_KIND + face);
                    assert_eq!(quad.plane(), 9);
                    assert_eq!(quad.iu(), 3);
                    assert_eq!(quad.iv(), 7);
                    assert_eq!(quad.tile(), 5);
                    assert_eq!(quad.light(), 11);
                    // The sub-cell fields, unpacked the way the shader does.
                    let w = quad.width() as u32;
                    let h = quad.height() as u32;
                    assert_eq!(w & 7, 2, "cells_u");
                    assert_eq!((w >> 3) & 3, sub, "sub_u");
                    assert_eq!((w >> 5) & 7, sub_plane, "sub_plane");
                    assert_eq!(h & 7, 4, "cells_v");
                    assert_eq!((h >> 3) & 3, 3 - sub, "sub_v");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::{BlockDef, ChunkPos, LocalPos};
    use vx_world::{Chunk, SoloChunkView};

    struct Fixture {
        registry: BlockRegistry,
        stone: BlockId,
        glass: BlockId,
        water: BlockId,
        plant: BlockId,
    }

    fn fixture() -> Fixture {
        let mut registry = BlockRegistry::new();
        let stone = registry.register(BlockDef::uniform("test:stone", 0)).unwrap();
        let glass = registry
            .register(BlockDef::uniform("test:glass", 1).translucent())
            .unwrap();
        let water = registry
            .register(BlockDef::uniform("test:water", 2).translucent().non_solid())
            .unwrap();
        let plant = registry
            .register(BlockDef::uniform("test:plant", 3).cross())
            .unwrap();
        Fixture { registry, stone, glass, water, plant }
    }

    fn local(x: i32, y: i32, z: i32) -> LocalPos {
        LocalPos::new(x, y, z).expect("test coordinates in range")
    }

    /// A mesh plus its expanded triangles.
    ///
    /// Geometry is far easier to assert about as explicit corners than as bit
    /// fields, so the tests read through `Mesh::to_vertices` — the same
    /// unpacking the vertex shader does, which is why checking it here is
    /// checking what the GPU draws.
    struct Geometry {
        vertices: Vec<Vertex>,
        indices: Vec<u32>,
        quads: Vec<PackedQuad>,
    }

    impl Geometry {
        fn quad_count(&self) -> usize {
            self.quads.len()
        }

        fn triangle_count(&self) -> usize {
            self.quads.len() * 2
        }

        fn is_empty(&self) -> bool {
            self.quads.is_empty()
        }
    }

    fn expand(mesh: &Mesh, origin: [i32; 3]) -> Geometry {
        let (vertices, indices) = mesh.to_vertices(origin);
        Geometry {
            vertices,
            indices,
            quads: mesh.quads.clone(),
        }
    }

    fn mesh_at(chunk: &Chunk, registry: &BlockRegistry, origin: [i32; 3]) -> Geometry {
        expand(&build_mesh(&SoloChunkView(chunk), registry, origin), origin)
    }

    fn mesh_of(chunk: &Chunk, registry: &BlockRegistry) -> Geometry {
        mesh_at(chunk, registry, [0, 0, 0])
    }

    /// Recompute a triangle's normal from its winding, to check the geometry
    /// faces outward. A back-facing quad is invisible under backface culling,
    /// and that is miserable to diagnose on screen.
    fn winding_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cross = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let length = (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt();
        assert!(length > 0.0, "degenerate triangle: {a:?} {b:?} {c:?}");
        [cross[0] / length, cross[1] / length, cross[2] / length]
    }

    #[test]
    fn an_empty_chunk_produces_no_geometry() {
        let fixture = fixture();
        let chunk = Chunk::empty(ChunkPos::new(0, 0));
        let mesh = mesh_of(&chunk, &fixture.registry);

        assert!(mesh.is_empty());
        assert_eq!(mesh.vertices.len(), 0);
        assert_eq!(mesh.quad_count(), 0);
    }

    #[test]
    fn a_lone_block_produces_exactly_six_quads() {
        let fixture = fixture();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.set(local(5, 40, 5), fixture.stone);

        let mesh = mesh_of(&chunk, &fixture.registry);

        assert_eq!(mesh.quad_count(), 6);
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
        assert_eq!(mesh.triangle_count(), 12);
    }

    #[test]
    fn a_cross_block_is_two_double_sided_diagonals() {
        let fixture = fixture();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.set(local(5, 40, 5), fixture.plant);

        let mesh = mesh_of(&chunk, &fixture.registry);

        // Two diagonals, each emitted front and back: 4 quads, 8 triangles,
        // and not a single axis-aligned cube face.
        assert_eq!(mesh.quad_count(), 4);
        assert_eq!(mesh.triangle_count(), 8);
        for vertex in &mesh.vertices {
            let [x, _, z] = vertex.position;
            assert!(
                (x - 5.0).abs() < 1.0e-6 && (z - 5.0).abs() < 1.0e-6
                    || (x - 6.0).abs() < 1.0e-6 && (z - 6.0).abs() < 1.0e-6
                    || (x - 6.0).abs() < 1.0e-6 && (z - 5.0).abs() < 1.0e-6
                    || (x - 5.0).abs() < 1.0e-6 && (z - 6.0).abs() < 1.0e-6,
                "cross vertex off the cell corners: {:?}",
                vertex.position
            );
        }
    }

    #[test]
    fn a_plant_never_hides_its_neighbours_faces() {
        let fixture = fixture();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.set(local(5, 40, 5), fixture.stone);
        let bare = mesh_of(&chunk, &fixture.registry);

        // Grass against the stone's +X face: the stone must keep all six
        // faces, and the mesh gains exactly the plant's four quads.
        chunk.set(local(6, 40, 5), fixture.plant);
        let planted = mesh_of(&chunk, &fixture.registry);
        assert_eq!(planted.quad_count(), bare.quad_count() + 4);
    }

    #[test]
    fn every_quad_winds_to_face_outward() {
        let fixture = fixture();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.set(local(5, 40, 5), fixture.stone);
        // A second, separated block exercises the other slices too.
        chunk.set(local(9, 44, 2), fixture.stone);

        let mesh = mesh_of(&chunk, &fixture.registry);

        for triangle in mesh.indices.chunks_exact(3) {
            let vertices: Vec<_> = triangle
                .iter()
                .map(|&i| mesh.vertices[i as usize])
                .collect();
            let geometric =
                winding_normal(vertices[0].position, vertices[1].position, vertices[2].position);
            let declared = vertices[0].normal;

            for axis in 0..3 {
                assert!(
                    (geometric[axis] - declared[axis]).abs() < 1e-5,
                    "winding normal {geometric:?} disagrees with face normal {declared:?}"
                );
            }
        }
    }

    #[test]
    fn touching_faces_between_two_blocks_are_culled() {
        let fixture = fixture();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.set(local(5, 40, 5), fixture.stone);
        chunk.set(local(6, 40, 5), fixture.stone);

        let mesh = mesh_of(&chunk, &fixture.registry);

        // Ten faces, not twelve: the shared face is hidden from both sides.
        // Of those, the four pairs spanning both blocks merge into one quad
        // each, leaving 4 merged + 2 end caps.
        assert_eq!(mesh.quad_count(), 6);
    }

    #[test]
    fn a_flat_slab_merges_into_one_quad_per_side() {
        let fixture = fixture();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                chunk.set(local(x, 40, z), fixture.stone);
            }
        }

        let mesh = mesh_of(&chunk, &fixture.registry);

        // Top and bottom merge to one 16×16 quad each; the four edges merge to
        // one 16×1 quad each. Naive meshing would emit 256 × 6 = 1536.
        assert_eq!(mesh.quad_count(), 6, "greedy merging failed to collapse the slab");
    }

    #[test]
    fn a_completely_solid_chunk_shows_only_its_shell() {
        let fixture = fixture();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                chunk.fill_column(x, z, 0, CHUNK_HEIGHT, fixture.stone);
            }
        }

        let mesh = mesh_of(&chunk, &fixture.registry);

        // Interior faces are all culled; the shell merges to six quads.
        assert_eq!(mesh.quad_count(), 6);
    }

    #[test]
    fn merged_quads_stretch_their_uvs_so_textures_repeat() {
        let fixture = fixture();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        for x in 0..4 {
            chunk.set(local(x, 40, 0), fixture.stone);
        }

        let mesh = mesh_of(&chunk, &fixture.registry);

        // The 4-long top face should span 4 tiles on its long axis.
        let longest = mesh
            .vertices
            .chunks_exact(4)
            .map(|quad| {
                quad.iter()
                    .flat_map(|vertex| vertex.uv)
                    .fold(0.0f32, f32::max)
            })
            .fold(0.0f32, f32::max);
        assert_eq!(longest, 4.0, "merged quads must scale UVs by their size");
    }

    #[test]
    fn transparent_blocks_do_not_hide_their_neighbours() {
        let fixture = fixture();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.set(local(5, 40, 5), fixture.stone);
        chunk.set(local(6, 40, 5), fixture.glass);

        let mesh = mesh_of(&chunk, &fixture.registry);

        // Culling is one-directional and depends on the *neighbour*:
        //   - stone keeps all 6 faces, because see-through glass hides nothing;
        //   - glass keeps only 5, because opaque stone does hide the face
        //     pressed against it.
        // Nothing merges, since the two block types differ.
        assert_eq!(mesh.quad_count(), 11);
    }

    #[test]
    fn matching_transparent_blocks_do_not_draw_faces_against_each_other() {
        let fixture = fixture();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.set(local(5, 40, 5), fixture.water);
        chunk.set(local(6, 40, 5), fixture.water);

        let mesh = mesh_of(&chunk, &fixture.registry);

        // Without the same-block rule this would be 12, and the inside of every
        // ocean would be full of invisible-but-drawn faces.
        assert_eq!(mesh.quad_count(), 6);
    }

    #[test]
    fn different_block_types_never_merge_into_one_quad() {
        let fixture = fixture();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        chunk.set(local(0, 40, 0), fixture.stone);
        chunk.set(local(1, 40, 0), fixture.glass);

        let mesh = mesh_of(&chunk, &fixture.registry);

        // Every quad must carry a single tile id, so no quad may straddle the
        // two block types.
        for quad in mesh.vertices.chunks_exact(4) {
            let tile = quad[0].tile;
            assert!(quad.iter().all(|vertex| vertex.tile == tile));
        }
        assert!(mesh.quad_count() >= 11);
    }

    #[test]
    fn quads_stay_within_the_chunk_bounds() {
        let fixture = fixture();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                chunk.set(local(x, 40, z), fixture.stone);
            }
        }

        let mesh = mesh_of(&chunk, &fixture.registry);

        for vertex in &mesh.vertices {
            assert!((0.0..=CHUNK_SIZE as f32).contains(&vertex.position[0]));
            assert!((0.0..=CHUNK_HEIGHT as f32).contains(&vertex.position[1]));
            assert!((0.0..=CHUNK_SIZE as f32).contains(&vertex.position[2]));
        }
    }

    #[test]
    fn the_origin_offsets_geometry_into_world_space() {
        let fixture = fixture();
        let mut chunk = Chunk::empty(ChunkPos::new(2, 0));
        chunk.set(local(0, 40, 0), fixture.stone);

        let mesh = mesh_at(&chunk, &fixture.registry, [32, 0, 0]);

        let min_x = mesh
            .vertices
            .iter()
            .map(|vertex| vertex.position[0])
            .fold(f32::INFINITY, f32::min);
        assert_eq!(min_x, 32.0, "chunk geometry must be placed at its world origin");
    }

    #[test]
    fn every_index_refers_to_a_real_vertex() {
        let fixture = fixture();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        for x in 0..CHUNK_SIZE {
            chunk.set(local(x, 40, 3), fixture.stone);
            chunk.set(local(x, 41, 3), fixture.glass);
        }

        let mesh = mesh_of(&chunk, &fixture.registry);

        assert!(!mesh.is_empty());
        assert_eq!(mesh.indices.len() % 3, 0);
        for &index in &mesh.indices {
            assert!(
                (index as usize) < mesh.vertices.len(),
                "index {index} is past the end of the vertex buffer"
            );
        }
    }

    #[test]
    fn unpacking_a_quad_gives_the_corners_it_was_built_from() {
        // The vertex shader does this same arithmetic, so these expectations
        // are what the GPU draws. Written out by hand rather than compared
        // against another implementation, or the two could agree on being
        // wrong together.
        //
        // Kind 3 is PosY, whose plane axis is Y and whose (u, v) are (Z, X).
        let top = PackedQuad::face(3, 8, 1, 2, 3, 4, 7, 15);
        assert_eq!(top.normal(), [0.0, 1.0, 0.0]);
        assert_eq!(
            top.corners(),
            [
                [2.0, 8.0, 1.0],
                [2.0, 8.0, 4.0],
                [6.0, 8.0, 4.0],
                [6.0, 8.0, 1.0],
            ]
        );
        assert_eq!(
            top.uvs(),
            [[0.0, 0.0], [3.0, 0.0], [3.0, 4.0], [0.0, 4.0]]
        );

        // Kind 0 is NegX, which winds the other way so its outward face is -X.
        let west = PackedQuad::face(0, 5, 0, 0, 1, 1, 2, 15);
        assert_eq!(west.normal(), [-1.0, 0.0, 0.0]);
        assert_eq!(
            west.corners(),
            [
                [5.0, 0.0, 0.0],
                [5.0, 0.0, 1.0],
                [5.0, 1.0, 1.0],
                [5.0, 1.0, 0.0],
            ]
        );

        // A plant's second winding is its first one backwards, which is how it
        // survives backface culling from either side.
        let front = PackedQuad::cross(0, 3, 9, 4, 5, 15);
        let back = PackedQuad::cross(1, 3, 9, 4, 5, 15);
        let forward = front.corners();
        assert_eq!(
            back.corners(),
            [forward[3], forward[2], forward[1], forward[0]]
        );
        assert_eq!(front.normal(), [0.0, 1.0, 0.0], "plants light as if flat");
    }

    #[test]
    fn expanding_a_mesh_places_it_at_its_chunk_origin() {
        // Packed positions are chunk-local; the origin is added back once,
        // here and in the shader, instead of being baked into every vertex.
        let f = fixture();
        let mut chunk = Chunk::empty(ChunkPos::new(1, -2));
        chunk.set(local(0, 40, 0), f.stone);
        let mesh = build_mesh(&SoloChunkView(&chunk), &f.registry, [16, 0, -32]);

        let (local_vertices, _) = mesh.to_vertices([0, 0, 0]);
        let (world_vertices, _) = mesh.to_vertices([16, 0, -32]);
        for (near, far) in local_vertices.iter().zip(&world_vertices) {
            assert_eq!(far.position[0], near.position[0] + 16.0);
            assert_eq!(far.position[1], near.position[1]);
            assert_eq!(far.position[2], near.position[2] - 32.0);
        }
        // And every packed coordinate stays inside its own chunk.
        for quad in &mesh.quads {
            assert!((0..=256).contains(&quad.plane()));
            assert!((0..=256).contains(&quad.iu()));
            assert!((0..=256).contains(&quad.iv()));
        }
    }

    #[test]
    fn faces_bake_the_sky_they_can_see() {
        // A stone floor with a slab of roof high over half of it: the covered
        // half bakes dark, the open half bakes full light, and the two halves
        // must not merge into one quad smearing a single value across both.
        let f = fixture();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0));
        for x in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                chunk.set(local(x, 10, z), f.stone);
                if x < 8 {
                    chunk.set(local(x, 40, z), f.stone);
                }
            }
        }
        let mesh = build_mesh(&SoloChunkView(&chunk), &f.registry, [0, 0, 0]);

        let floor: Vec<_> = mesh
            .quads
            .iter()
            .filter(|quad| quad.kind() == 3 && quad.plane() == 11)
            .collect();
        assert!(floor.len() >= 2, "covered and open floor merged into one quad");
        for quad in &floor {
            // PosY: u is z, v is x — iv is the x edge of the rectangle.
            let covered = quad.iv() < 8;
            let expected = if covered { sky_light(40 - 11) } else { FULL_LIGHT };
            assert_eq!(
                quad.light(),
                expected,
                "wrong bake at x={} (covered: {covered})",
                quad.iv()
            );
        }
        assert_eq!(sky_light(40 - 11), 0, "a 29-block shaft should be black");
    }

    #[test]
    fn the_light_curve_is_gentle_then_gone() {
        assert_eq!(sky_light(0), FULL_LIGHT, "open sky is full light");
        assert_eq!(sky_light(-5), FULL_LIGHT);
        assert!(sky_light(3) >= 11, "a house interior should read as shade, not night");
        assert_eq!(sky_light(30), 0, "deep galleries are black");
        for depth in 0..40 {
            assert!(
                sky_light(depth + 1) <= sky_light(depth),
                "the curve brightened while descending at depth {depth}"
            );
        }
    }

    #[test]
    fn a_packed_quad_round_trips_every_field() {
        // Bit-packing is exactly the kind of code that silently loses the top
        // of a field, so walk the extremes of each one.
        for kind in 0..6u32 {
            for plane in [0, 1, 16, 255, 256] {
                for (iu, iv, w, h) in [(0, 0, 1, 1), (15, 255, 1, 1), (0, 0, 16, 256)] {
                    for tile in [0, 1, 26, 255] {
                        for light in [0, 1, 9, 15] {
                            let quad = PackedQuad::face(kind, plane, iu, iv, w, h, tile, light);
                            assert_eq!(quad.kind(), kind);
                            assert_eq!(quad.plane(), plane);
                            assert_eq!(quad.iu(), iu);
                            assert_eq!(quad.iv(), iv);
                            assert_eq!(quad.width(), w);
                            assert_eq!(quad.height(), h);
                            assert_eq!(quad.tile(), tile);
                            assert_eq!(quad.light(), light);
                            assert!(!quad.is_cross());
                        }
                    }
                }
            }
        }

        for variant in 0..4 {
            let quad = PackedQuad::cross(variant, 15, 200, 3, 12, 15);
            assert!(quad.is_cross());
            assert_eq!(quad.tile(), 12);
            assert_eq!((quad.iu(), quad.plane(), quad.iv()), (15, 200, 3));
        }
    }

    #[test]
    fn a_quad_is_eight_bytes() {
        // The whole argument. A merged quad used to cost four 36-byte vertices
        // plus six 4-byte indices.
        assert_eq!(std::mem::size_of::<PackedQuad>(), 8);
        assert_eq!(std::mem::size_of::<Vertex>() * 4 + 4 * 6, 168);
    }
}
