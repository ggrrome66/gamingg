//! Instanced objects — drones, markers, anything that is not terrain.
//!
//! # Instanced from the start
//!
//! The design calls for swarms: dozens of drones sharing one shape. Drawing
//! each with its own draw call and its own uniform would work for the single
//! drone that exists today and would have to be thrown away at the first swarm.
//! So there is one cube mesh, one instance buffer of per-object transforms, and
//! one draw call regardless of how many objects there are.
//!
//! # Shading is shared with terrain deliberately
//!
//! The cube uses [`vx_mesh::Vertex`] and the terrain [`VERTEX_LAYOUT`], and the
//! object vertex stage emits the same `VertexOutput` the terrain stage does, so
//! both go through the *same* fragment entry point. A drone lit differently
//! from the ground it is standing on looks wrong immediately, and sharing the
//! fragment shader makes that impossible rather than merely unlikely.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use vx_mesh::Vertex;
use wgpu::util::DeviceExt;

use crate::frustum::Frustum;

/// One object as the caller describes it, before culling.
///
/// Carries its own world-space bounding box so culling never has to re-derive
/// it from the matrix, and so a caller that knows a tighter bound can supply
/// one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Object {
    pub model: Mat4,
    pub tile: u32,
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
}

impl Object {
    /// An object from an arbitrary transform of the unit cube.
    ///
    /// The bounds come from transforming all eight corners, which stays correct
    /// under rotation — taking the transformed min and max corners alone would
    /// not.
    pub fn new(model: Mat4, tile: u32) -> Self {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for corner in 0..8u32 {
            let unit = Vec3::new(
                (corner & 1) as f32,
                ((corner >> 1) & 1) as f32,
                ((corner >> 2) & 1) as f32,
            );
            let world = model.transform_point3(unit);
            min = min.min(world);
            max = max.max(world);
        }
        Object {
            model,
            tile,
            bounds_min: min,
            bounds_max: max,
        }
    }

    /// An axis-aligned box spanning `min..max`, the common case.
    pub fn box_between(min: Vec3, max: Vec3, tile: u32) -> Self {
        let size = max - min;
        Object {
            model: Mat4::from_translation(min) * Mat4::from_scale(size),
            tile,
            bounds_min: min,
            bounds_max: max,
        }
    }

    /// A cube of edge `size` centred horizontally on `centre` and sitting with
    /// its base at `centre.y` — how a thing standing on the ground is placed.
    pub fn standing(centre: Vec3, size: f32, tile: u32) -> Self {
        let half = size * 0.5;
        let min = Vec3::new(centre.x - half, centre.y, centre.z - half);
        Object::box_between(min, min + Vec3::splat(size), tile)
    }

    fn instance(&self) -> ObjectInstance {
        ObjectInstance {
            model: self.model.to_cols_array_2d(),
            normal: normal_matrix(self.model),
            tile: self.tile,
        }
    }
}

/// The matrix that transforms *normals* for `model`: the inverse-transpose of
/// its upper 3x3, padded to three vec4 columns for the vertex layout.
///
/// Using the model matrix itself is only correct for translation, rotation and
/// uniform scale. `Object::new` accepts arbitrary transforms, and the first
/// elongated drone facing its travel direction — rotation times non-uniform
/// scale — would have had its lighting visibly skewed. Computed on the CPU
/// once per instance per frame, which is where a per-object constant belongs.
///
/// A singular matrix (zero scale on some axis) has no inverse; fall back to the
/// raw 3x3 rather than poisoning the buffer with NaNs — a degenerate object is
/// invisible anyway.
fn normal_matrix(model: Mat4) -> [[f32; 4]; 3] {
    let linear = glam::Mat3::from_mat4(model);
    let inverse_transpose = if linear.determinant().abs() > 1.0e-8 {
        linear.inverse().transpose()
    } else {
        linear
    };
    let column = |c: glam::Vec3| [c.x, c.y, c.z, 0.0];
    [
        column(inverse_transpose.x_axis),
        column(inverse_transpose.y_axis),
        column(inverse_transpose.z_axis),
    ]
}

/// The per-instance data the GPU sees. Rebuilt every frame.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct ObjectInstance {
    /// Column-major, matching WGSL's `mat4x4<f32>`.
    pub model: [[f32; 4]; 4],
    /// Inverse-transpose of the model's upper 3x3, one padded vec4 per column,
    /// for transforming normals — see [`normal_matrix`].
    pub normal: [[f32; 4]; 3],
    pub tile: u32,
}

/// Instance-rate vertex layout. Locations continue past the terrain vertex's
/// 0..=3 so both buffers can be bound to the same pipeline.
pub const INSTANCE_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: std::mem::size_of::<ObjectInstance>() as wgpu::BufferAddress,
    step_mode: wgpu::VertexStepMode::Instance,
    attributes: &[
        // A mat4 arrives as four separate vec4 attributes; there is no matrix
        // vertex format.
        wgpu::VertexAttribute {
            offset: 0,
            shader_location: 4,
            format: wgpu::VertexFormat::Float32x4,
        },
        wgpu::VertexAttribute {
            offset: 16,
            shader_location: 5,
            format: wgpu::VertexFormat::Float32x4,
        },
        wgpu::VertexAttribute {
            offset: 32,
            shader_location: 6,
            format: wgpu::VertexFormat::Float32x4,
        },
        wgpu::VertexAttribute {
            offset: 48,
            shader_location: 7,
            format: wgpu::VertexFormat::Float32x4,
        },
        // The normal matrix, three more vec4 columns.
        wgpu::VertexAttribute {
            offset: 64,
            shader_location: 8,
            format: wgpu::VertexFormat::Float32x4,
        },
        wgpu::VertexAttribute {
            offset: 80,
            shader_location: 9,
            format: wgpu::VertexFormat::Float32x4,
        },
        wgpu::VertexAttribute {
            offset: 96,
            shader_location: 10,
            format: wgpu::VertexFormat::Float32x4,
        },
        wgpu::VertexAttribute {
            offset: 112,
            shader_location: 11,
            format: wgpu::VertexFormat::Uint32,
        },
    ],
};

/// Plain vertex geometry for an object.
///
/// Terrain is packed quads now, but an object is a handful of cuboids drawn
/// with a model matrix per instance rather than a chunk of blocks on a lattice,
/// so there is nothing for the packing to exploit and the old vertex form is
/// simply the right shape here.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CubeMesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl CubeMesh {
    /// Each quad is four vertices and six indices.
    pub fn quad_count(&self) -> usize {
        self.indices.len() / 6
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Geometry for a unit cube spanning `0..1` on every axis.
///
/// Wound to match the greedy mesher exactly — counter-clockwise seen from
/// outside, reversed for negative faces — so the pipeline's back-face culling
/// treats terrain and objects identically. Getting this backwards produces a
/// cube that is invisible from outside and solid from within, which is a
/// genuinely confusing thing to debug.
pub fn unit_cube(tile: u32) -> CubeMesh {
    let mut mesh = CubeMesh::default();

    for face in vx_core::Face::ALL {
        let d = face.axis();
        let u = (d + 1) % 3;
        let v = (d + 2) % 3;
        let normal = face.normal();
        let plane = f32::from(u8::from(face.is_positive()));

        let corner = |a: f32, b: f32| {
            let mut position = [0.0f32; 3];
            position[d] = plane;
            position[u] = a;
            position[v] = b;
            position
        };

        let corners = if face.is_positive() {
            [
                corner(0.0, 0.0),
                corner(1.0, 0.0),
                corner(1.0, 1.0),
                corner(0.0, 1.0),
            ]
        } else {
            [
                corner(0.0, 0.0),
                corner(0.0, 1.0),
                corner(1.0, 1.0),
                corner(1.0, 0.0),
            ]
        };

        let base = mesh.vertices.len() as u32;
        let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        for (position, uv) in corners.into_iter().zip(uvs) {
            mesh.vertices.push(Vertex {
                position,
                normal,
                uv,
                // Ignored: the object shader takes its tile from the instance.
                tile,
            });
        }
        mesh.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    mesh
}

/// The shared cube plus a growable instance buffer.
pub struct ObjectBatch {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    instance_buffer: wgpu::Buffer,
    /// Instances the buffer can hold before it has to be reallocated.
    capacity: usize,
    /// Instances actually written for this frame.
    live: u32,
}

/// Instances a fresh batch allocates room for. Reallocating is cheap and rare;
/// this only avoids doing it for the first few frames.
const INITIAL_CAPACITY: usize = 64;

impl ObjectBatch {
    pub fn new(device: &wgpu::Device) -> Self {
        let cube = unit_cube(0);

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("object cube vertices"),
            contents: bytemuck::cast_slice(&cube.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("object cube indices"),
            contents: bytemuck::cast_slice(&cube.indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        ObjectBatch {
            vertex_buffer,
            index_buffer,
            index_count: cube.indices.len() as u32,
            instance_buffer: Self::allocate(device, INITIAL_CAPACITY),
            capacity: INITIAL_CAPACITY,
            live: 0,
        }
    }

    fn allocate(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("object instances"),
            size: (capacity * std::mem::size_of::<ObjectInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Cull `objects` against `frustum` and upload what survives.
    ///
    /// Culling here rather than in the shader is what keeps the draw to a
    /// single call: the GPU cannot skip an instance mid-draw, so anything that
    /// should not be drawn must not be in the buffer.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        objects: &[Object],
        frustum: Option<Frustum>,
    ) {
        let instances: Vec<ObjectInstance> = objects
            .iter()
            .filter(|object| match frustum {
                Some(frustum) => frustum.intersects_aabb(object.bounds_min, object.bounds_max),
                None => true,
            })
            .map(Object::instance)
            .collect();

        if instances.len() > self.capacity {
            // Grow generously: a swarm that adds one drone per frame would
            // otherwise reallocate every frame.
            self.capacity = instances.len().next_power_of_two();
            self.instance_buffer = Self::allocate(device, self.capacity);
        }

        self.live = instances.len() as u32;
        if !instances.is_empty() {
            queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&instances));
        }
    }

    /// Instances that would be drawn this frame.
    pub fn live_count(&self) -> u32 {
        self.live
    }

    /// Record the draw. The caller has already bound the pipeline and groups.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.live == 0 {
            return;
        }
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..self.live);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_instance_layout_matches_the_instance_struct() {
        // A mismatch scrambles every transform, which shows up as objects in
        // wild positions rather than as an error, so pin it explicitly.
        assert_eq!(std::mem::size_of::<ObjectInstance>(), 116);
        assert_eq!(INSTANCE_LAYOUT.array_stride, 116);
        assert_eq!(INSTANCE_LAYOUT.step_mode, wgpu::VertexStepMode::Instance);

        let offsets: Vec<u64> = INSTANCE_LAYOUT.attributes.iter().map(|a| a.offset).collect();
        assert_eq!(offsets, vec![0, 16, 32, 48, 64, 80, 96, 112]);
    }

    #[test]
    fn instance_locations_do_not_collide_with_the_vertex_layout() {
        // Both buffers feed the same pipeline; a shared location is a link error
        // at best and silent aliasing at worst.
        let vertex: Vec<u32> = crate::VERTEX_LAYOUT
            .attributes
            .iter()
            .map(|a| a.shader_location)
            .collect();
        for attribute in INSTANCE_LAYOUT.attributes {
            assert!(
                !vertex.contains(&attribute.shader_location),
                "location {} is used by both layouts",
                attribute.shader_location
            );
        }
    }

    #[test]
    fn the_cube_has_six_faces_worth_of_geometry() {
        let cube = unit_cube(3);
        assert_eq!(cube.vertices.len(), 24);
        assert_eq!(cube.quad_count(), 6);
        assert_eq!(cube.triangle_count(), 12);
    }

    #[test]
    fn the_cube_spans_the_unit_box() {
        let cube = unit_cube(0);
        for axis in 0..3 {
            let min = cube
                .vertices
                .iter()
                .map(|v| v.position[axis])
                .fold(f32::INFINITY, f32::min);
            let max = cube
                .vertices
                .iter()
                .map(|v| v.position[axis])
                .fold(f32::NEG_INFINITY, f32::max);
            assert_eq!((min, max), (0.0, 1.0));
        }
    }

    #[test]
    fn every_cube_face_is_wound_outward() {
        // The real check on winding: for each triangle, the cross product of
        // its edges must point the same way as the face normal. Getting this
        // wrong makes the cube visible only from inside, and back-face culling
        // means it fails silently rather than loudly.
        let cube = unit_cube(0);
        for triangle in cube.indices.chunks_exact(3) {
            let corners: Vec<Vec3> = triangle
                .iter()
                .map(|&index| Vec3::from(cube.vertices[index as usize].position))
                .collect();
            let normal = Vec3::from(cube.vertices[triangle[0] as usize].normal);
            let geometric = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
            assert!(
                geometric.dot(normal) > 0.0,
                "a triangle on the {normal:?} face is wound inward"
            );
        }
    }

    #[test]
    fn cube_normals_point_away_from_the_centre() {
        let cube = unit_cube(0);
        let centre = Vec3::splat(0.5);
        for vertex in &cube.vertices {
            let outward = Vec3::from(vertex.position) - centre;
            assert!(
                outward.dot(Vec3::from(vertex.normal)) > 0.0,
                "a normal points inward at {:?}",
                vertex.position
            );
        }
    }

    #[test]
    fn box_between_produces_a_matrix_that_maps_the_unit_cube_onto_it() {
        let min = Vec3::new(-3.0, 12.0, 7.5);
        let max = Vec3::new(1.0, 14.0, 9.5);
        let object = Object::box_between(min, max, 2);

        assert_eq!(object.model.transform_point3(Vec3::ZERO), min);
        assert_eq!(object.model.transform_point3(Vec3::ONE), max);
        assert_eq!((object.bounds_min, object.bounds_max), (min, max));
    }

    #[test]
    fn standing_sits_on_the_ground_rather_than_straddling_it() {
        // A drone placed at a surface position should stand on it, not sink
        // half a body into it.
        let object = Object::standing(Vec3::new(4.0, 70.0, -2.0), 0.8, 1);
        assert_eq!(object.bounds_min.y, 70.0);
        assert_eq!(object.bounds_max.y, 70.8);
        assert!((object.bounds_min.x - 3.6).abs() < 1e-6);
        assert!((object.bounds_max.x - 4.4).abs() < 1e-6);
    }

    #[test]
    fn bounds_from_an_arbitrary_transform_enclose_every_corner() {
        // Rotation is why bounds are computed from all eight corners: the
        // transformed min corner is no longer the minimum of anything.
        let model = Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0))
            * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_4)
            * Mat4::from_scale(Vec3::splat(2.0));
        let object = Object::new(model, 0);

        for corner in 0..8u32 {
            let unit = Vec3::new(
                (corner & 1) as f32,
                ((corner >> 1) & 1) as f32,
                ((corner >> 2) & 1) as f32,
            );
            let world = model.transform_point3(unit);
            assert!(world.cmpge(object.bounds_min - 1e-5).all());
            assert!(world.cmple(object.bounds_max + 1e-5).all());
        }

        // And the bounds are genuinely wider than the unrotated box would be.
        assert!(object.bounds_max.x - object.bounds_min.x > 2.5);
    }

    #[test]
    fn the_normal_matrix_keeps_normals_perpendicular_under_skewing_transforms() {
        // Review finding A8. Rotation times non-uniform scale is exactly the
        // transform a long drone facing its travel direction uses, and exactly
        // where "just rotate the normal by the model matrix" breaks. The
        // inverse-transpose must keep a transformed normal perpendicular to a
        // transformed surface tangent.
        let model = Mat4::from_rotation_y(0.7)
            * Mat4::from_scale(Vec3::new(3.0, 1.0, 0.5))
            * Mat4::from_rotation_x(0.3);
        let columns = normal_matrix(model);
        let normal_transform = glam::Mat3::from_cols(
            Vec3::new(columns[0][0], columns[0][1], columns[0][2]),
            Vec3::new(columns[1][0], columns[1][1], columns[1][2]),
            Vec3::new(columns[2][0], columns[2][1], columns[2][2]),
        );

        // Several surface directions with their true normals.
        let pairs = [
            (Vec3::X, Vec3::Y),
            (Vec3::Z, Vec3::Y),
            (Vec3::Y, Vec3::X),
            (Vec3::new(1.0, 0.0, 1.0).normalize(), Vec3::Y),
        ];
        let mut naive_broke_somewhere = false;
        for (tangent, normal) in pairs {
            let moved_tangent = model.transform_vector3(tangent);
            let moved_normal = normal_transform * normal;
            let alignment = moved_normal.normalize().dot(moved_tangent.normalize());
            assert!(
                alignment.abs() < 1.0e-5,
                "normal drifted {alignment} off perpendicular for tangent {tangent:?}"
            );

            let naive = model.transform_vector3(normal);
            naive_broke_somewhere |=
                naive.normalize().dot(moved_tangent.normalize()).abs() > 1.0e-3;
        }
        // Prove the naive version really is wrong on this transform for at
        // least one face, so the test cannot pass vacuously on one too tame
        // to tell the two apart.
        assert!(
            naive_broke_somewhere,
            "the raw model matrix survived every pair; pick a harsher transform"
        );
    }

    #[test]
    fn an_instance_carries_the_matrix_in_column_major_order() {
        // WGSL reads mat4x4 columns; row-major would transpose every object.
        let object = Object::box_between(Vec3::new(5.0, 6.0, 7.0), Vec3::new(6.0, 7.0, 8.0), 0);
        let instance = object.instance();
        // Translation lives in the last column for column-major storage.
        assert_eq!(instance.model[3][0], 5.0);
        assert_eq!(instance.model[3][1], 6.0);
        assert_eq!(instance.model[3][2], 7.0);
    }
}
