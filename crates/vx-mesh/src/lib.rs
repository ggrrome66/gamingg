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

impl PackedQuad {
    /// Pack a cube face. `plane` is the position along the face's own axis;
    /// `iu`/`iv` and `w`/`h` are the rectangle in the other two. `light` is
    /// the face's baked sky exposure, 0 (buried dark) to 15 (open sky).
    ///
    /// Eight arguments because a quad genuinely has eight independent fields;
    /// bundling them into a struct would just move the same list.
    #[allow(clippy::too_many_arguments)]
    pub fn face(kind: u32, plane: i32, iu: i32, iv: i32, w: i32, h: i32, tile: u32, light: u32) -> Self {
        debug_assert!(kind < 10, "quad kind {kind} is not a face or a cross");
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
    mesh
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

                mask[(iv * du + iu) as usize] = face_is_visible(registry, block, neighbour)
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
