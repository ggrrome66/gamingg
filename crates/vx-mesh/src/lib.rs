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
use vx_core::{BlockId, BlockRegistry, Face, CHUNK_HEIGHT, CHUNK_SIZE};
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
    /// Packed light for the whole quad: sky in the high nibble, block light in
    /// the low. Flat across the face rather than interpolated per corner,
    /// which suits blocky terrain and keeps merged quads honest.
    pub light: u32,
}

/// Renderable geometry for one chunk.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
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
    fn push_quad(
        &mut self,
        corners: [[f32; 3]; 4],
        normal: [f32; 3],
        size: [f32; 2],
        tile: u32,
        light: u32,
    ) {
        let base = self.vertices.len() as u32;
        let [w, h] = size;
        let uvs = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]];

        for (corner, uv) in corners.into_iter().zip(uvs) {
            self.vertices.push(Vertex {
                position: corner,
                normal,
                uv,
                tile,
                light,
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
    mesh
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

    /// What a visible face looks like. Merging compares the whole thing, so a
    /// face lit differently from its neighbour is never folded into it.
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct Facet {
        block: BlockId,
        light: u8,
    }

    let mut mask: Vec<Option<Facet>> = vec![None; (du * dv) as usize];

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

                // Light is sampled from the open block the face looks into,
                // not from the solid block itself, which is always dark.
                mask[(iv * du + iu) as usize] = face_is_visible(registry, block, neighbour)
                    .then(|| Facet {
                        block,
                        light: view.light_at(
                            world[0] + offset[0],
                            world[1] + offset[1],
                            world[2] + offset[2],
                        ),
                    });
            }
        }

        emit_merged_quads(&mut mask, du, dv, |iu, iv, w, h, facet| {
            let tile = registry.get_or_air(facet.block).texture(face) as u32;

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
                corners,
                normal,
                [w as f32, h as f32],
                tile,
                facet.light as u32,
            );
        });
    }
}

/// Merge the mask into maximal rectangles, calling `emit` for each.
///
/// Consumed entries are cleared as it goes, so every face is emitted once.
fn emit_merged_quads<T: Copy + PartialEq>(
    mask: &mut [Option<T>],
    du: i32,
    dv: i32,
    mut emit: impl FnMut(i32, i32, i32, i32, T),
) {
    for iv in 0..dv {
        let mut iu = 0;
        while iu < du {
            let index = (iv * du + iu) as usize;
            let Some(facet) = mask[index] else {
                iu += 1;
                continue;
            };

            // Grow along u while the facet matches exactly.
            let mut width = 1;
            while iu + width < du && mask[(iv * du + iu + width) as usize] == Some(facet) {
                width += 1;
            }

            // Then grow along v, but only in whole rows of that width, so the
            // result stays rectangular.
            let mut height = 1;
            'grow: while iv + height < dv {
                for offset in 0..width {
                    if mask[((iv + height) * du + iu + offset) as usize] != Some(facet) {
                        break 'grow;
                    }
                }
                height += 1;
            }

            emit(iu, iv, width, height, facet);

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
        Fixture { registry, stone, glass, water }
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
}
