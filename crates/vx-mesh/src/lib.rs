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
/// kind:4 | plane:9 | iu:9 | iv:9 | w:9 | h:9 | tile:8   = 57 bits
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
    /// `iu`/`iv` and `w`/`h` are the rectangle in the other two.
    pub fn face(kind: u32, plane: i32, iu: i32, iv: i32, w: i32, h: i32, tile: u32) -> Self {
        debug_assert!(kind < 10, "quad kind {kind} is not a face or a cross");
        let bits = (kind as u64)
            | ((plane as u64 & 0x1ff) << 4)
            | ((iu as u64 & 0x1ff) << 13)
            | ((iv as u64 & 0x1ff) << 22)
            | ((w as u64 & 0x1ff) << 31)
            | ((h as u64 & 0x1ff) << 40)
            | ((tile as u64 & 0xff) << 49);
        PackedQuad {
            low: bits as u32,
            high: (bits >> 32) as u32,
        }
    }

    /// Pack one of a plant's crossed quads at a block.
    pub fn cross(variant: usize, x: i32, y: i32, z: i32, tile: u32) -> Self {
        PackedQuad::face(CROSS_KINDS[variant], y, x, z, 1, 1, tile)
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

/// Renderable geometry for one chunk.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// The same geometry as packed quads, chunk-local.
    pub quads: Vec<PackedQuad>,
}

impl Mesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Number of quads. Each is two triangles, so six indices.
    pub fn quad_count(&self) -> usize {
        self.indices.len() / 6
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Append one quad, given its four corners in winding order.
    ///
    /// Takes the packed form too, and the two are held to the same geometry by
    /// `packed_quads_reproduce_the_vertex_geometry`. Once the renderer reads
    /// quads the vertex half goes away; until then both exist so the change can
    /// be proved before it is relied on.
    fn push_quad(
        &mut self,
        packed: PackedQuad,
        corners: [[f32; 3]; 4],
        normal: [f32; 3],
        size: [f32; 2],
        tile: u32,
    ) {
        self.quads.push(packed);
        let base = self.vertices.len() as u32;
        let [w, h] = size;
        let uvs = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]];

        for (corner, uv) in corners.into_iter().zip(uvs) {
            self.vertices.push(Vertex {
                position: corner,
                normal,
                uv,
                tile,
            });
        }
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// Should this block show a face toward `neighbour`?
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
    for face in Face::ALL {
        mesh_face_direction(&mut mesh, view, registry, chunk_origin, face);
    }
    mesh_cross_blocks(&mut mesh, view, registry, chunk_origin);
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
                let (fx, fy, fz) = (
                    (origin[0] + x) as f32,
                    (origin[1] + y) as f32,
                    (origin[2] + z) as f32,
                );
                // Lit as if lying flat: plants take the ground's light, and
                // neither diagonal ever shows a dark "back of the leaf".
                let up = [0.0, 1.0, 0.0];
                // Corners run top-left, top-right, bottom-right, bottom-left
                // so v grows downward — texture row 0 is the top of the
                // blades.
                let diagonals = [
                    [
                        [fx, fy + 1.0, fz],
                        [fx + 1.0, fy + 1.0, fz + 1.0],
                        [fx + 1.0, fy, fz + 1.0],
                        [fx, fy, fz],
                    ],
                    [
                        [fx + 1.0, fy + 1.0, fz],
                        [fx, fy + 1.0, fz + 1.0],
                        [fx, fy, fz + 1.0],
                        [fx + 1.0, fy, fz],
                    ],
                ];
                for (diagonal, corners) in diagonals.into_iter().enumerate() {
                    mesh.push_quad(
                        PackedQuad::cross(diagonal * 2, x, y, z, tile),
                        corners,
                        up,
                        [1.0, 1.0],
                        tile,
                    );
                    let reversed = [corners[3], corners[2], corners[1], corners[0]];
                    mesh.push_quad(
                        PackedQuad::cross(diagonal * 2 + 1, x, y, z, tile),
                        reversed,
                        up,
                        [1.0, 1.0],
                        tile,
                    );
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
) {
    let d = face.axis();
    // u and v are the two axes spanning the face plane. Choosing them
    // cyclically means u × v = +d, which is what makes the winding below
    // produce an outward normal.
    let u = (d + 1) % 3;
    let v = (d + 2) % 3;

    let (du, dv, dd) = (DIMS[u], DIMS[v], DIMS[d]);
    let offset = face.offset();
    let normal = face.normal();

    let mut mask: Vec<Option<BlockId>> = vec![None; (du * dv) as usize];

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

                mask[(iv * du + iu) as usize] =
                    face_is_visible(registry, block, neighbour).then_some(block);
            }
        }

        emit_merged_quads(&mut mask, du, dv, |iu, iv, w, h, block| {
            let tile = registry.get_or_air(block).texture(face) as u32;

            // The quad sits on the far side of the voxel for positive faces.
            let plane = slice + i32::from(face.is_positive());

            // Corner (a, b) in the (u, v) plane, as a world position.
            let corner = |a: i32, b: i32| {
                let mut local = [0i32; 3];
                local[d] = plane;
                local[u] = a;
                local[v] = b;
                [
                    (origin[0] + local[0]) as f32,
                    (origin[1] + local[1]) as f32,
                    (origin[2] + local[2]) as f32,
                ]
            };

            let (u0, v0, u1, v1) = (iu, iv, iu + w, iv + h);
            // Counter-clockwise seen from outside. Reversed for negative
            // faces, whose outward direction is -d.
            let corners = if face.is_positive() {
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
            };

            mesh.push_quad(
                PackedQuad::face(
                    Face::ALL.iter().position(|other| *other == face).unwrap() as u32,
                    plane,
                    iu,
                    iv,
                    w,
                    h,
                    tile,
                ),
                corners,
                normal,
                [w as f32, h as f32],
                tile,
            );
        });
    }
}

/// Merge the mask into maximal rectangles, calling `emit` for each.
///
/// Consumed entries are cleared as it goes, so every face is emitted once.
fn emit_merged_quads(
    mask: &mut [Option<BlockId>],
    du: i32,
    dv: i32,
    mut emit: impl FnMut(i32, i32, i32, i32, BlockId),
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

    fn mesh_of(chunk: &Chunk, registry: &BlockRegistry) -> Mesh {
        build_mesh(&SoloChunkView(chunk), registry, [0, 0, 0])
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

        let mesh = build_mesh(&SoloChunkView(&chunk), &fixture.registry, [32, 0, 0]);

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
    fn packed_quads_reproduce_the_vertex_geometry_exactly() {
        // The gate for replacing vertices with quads. Unpacking a quad has to
        // give back the same four corners, the same normal and the same uvs the
        // mesher wrote as vertices — with the chunk origin added back, since
        // packed positions are chunk-local and vertex positions are not.
        // All four shapes in one chunk — opaque, translucent, non-solid and a
        // plant — so every quad kind is exercised. Meshed far from the origin
        // as well as at it, because chunk-local is the whole point: the packed
        // form must not know or care where its chunk sits.
        let f = fixture();
        let at = |pos: ChunkPos| {
            let mut chunk = Chunk::empty(pos);
            for x in 0..4 {
                for z in 0..4 {
                    chunk.fill_column(x, z, 0, 3, f.stone);
                }
            }
            chunk.set(local(1, 3, 1), f.glass);
            chunk.set(local(2, 3, 2), f.water);
            chunk.set(local(3, 3, 3), f.plant);
            chunk
        };

        for pos in [ChunkPos::new(0, 0), ChunkPos::new(1, -2), ChunkPos::new(-16, 32)] {
            let chunk = at(pos);
            let corner = pos.origin();
            let origin = [corner.x, corner.y, corner.z];
            let mesh = build_mesh(&SoloChunkView(&chunk), &f.registry, origin);
            assert!(!mesh.quads.is_empty(), "nothing was meshed at {origin:?}");
            assert_eq!(
                mesh.quads.len(),
                mesh.quad_count(),
                "a quad went missing between the two representations"
            );

            for (index, quad) in mesh.quads.iter().enumerate() {
                let corners = quad.corners();
                let uvs = quad.uvs();
                let normal = quad.normal();
                for corner in 0..4 {
                    let vertex = mesh.vertices[index * 4 + corner];
                    let expected = [
                        corners[corner][0] + origin[0] as f32,
                        corners[corner][1] + origin[1] as f32,
                        corners[corner][2] + origin[2] as f32,
                    ];
                    assert_eq!(
                        vertex.position, expected,
                        "quad {index} corner {corner} moved at origin {origin:?}"
                    );
                    assert_eq!(vertex.normal, normal, "quad {index} normal");
                    assert_eq!(vertex.uv, uvs[corner], "quad {index} corner {corner} uv");
                    assert_eq!(vertex.tile, quad.tile(), "quad {index} tile");
                }
            }
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
                        let quad = PackedQuad::face(kind, plane, iu, iv, w, h, tile);
                        assert_eq!(quad.kind(), kind);
                        assert_eq!(quad.plane(), plane);
                        assert_eq!(quad.iu(), iu);
                        assert_eq!(quad.iv(), iv);
                        assert_eq!(quad.width(), w);
                        assert_eq!(quad.height(), h);
                        assert_eq!(quad.tile(), tile);
                        assert!(!quad.is_cross());
                    }
                }
            }
        }

        for variant in 0..4 {
            let quad = PackedQuad::cross(variant, 15, 200, 3, 12);
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
