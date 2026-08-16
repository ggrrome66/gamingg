//! The outline around the block being looked at.
//!
//! Drawn as twelve thin boxes rather than a line list. wgpu offers no control
//! over line width — every line is one pixel, whatever the resolution — and a
//! single-pixel cage is nearly invisible against textured terrain. Solid edges
//! also scale with distance the way the rest of the world does, so the outline
//! shrinks as you back away instead of staying a constant screen width.

use bytemuck::{Pod, Zeroable};
use vx_core::BlockPos;
use wgpu::util::DeviceExt;

/// Phosphor green, to match the HUD.
pub const SELECTION_COLOUR: [f32; 4] = [0.0, 1.0, 0.25, 0.95];

/// Edge thickness, in blocks.
const THICKNESS: f32 = 0.04;
/// How far the cage sits proud of the block's own faces. Without this the
/// outline and the surface it hugs fight over the same depth values and the
/// edges break up into dashes as the camera moves.
const INFLATE: f32 = 0.006;

/// Corners per edge box, edges per block.
const EDGES: usize = 12;
const VERTICES_PER_EDGE: usize = 8;
const INDICES_PER_EDGE: usize = 36;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct SelectionVertex {
    position: [f32; 3],
    colour: [f32; 4],
}

pub const SELECTION_VERTEX_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: std::mem::size_of::<SelectionVertex>() as wgpu::BufferAddress,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &[
        wgpu::VertexAttribute {
            offset: 0,
            shader_location: 0,
            format: wgpu::VertexFormat::Float32x3,
        },
        wgpu::VertexAttribute {
            offset: 12,
            shader_location: 1,
            format: wgpu::VertexFormat::Float32x4,
        },
    ],
};

/// The twelve edges of the block at `pos`, as triangles.
pub fn build_outline(pos: BlockPos, colour: [f32; 4]) -> (Vec<SelectionVertex>, Vec<u32>) {
    let mut vertices = Vec::with_capacity(EDGES * VERTICES_PER_EDGE);
    let mut indices = Vec::with_capacity(EDGES * INDICES_PER_EDGE);
    let origin = [pos.x as f32, pos.y as f32, pos.z as f32];

    // Along each axis, an edge sits at one of the four corners formed by the
    // other two axes.
    for axis in 0..3 {
        let (other_a, other_b) = match axis {
            0 => (1, 2),
            1 => (0, 2),
            _ => (0, 1),
        };

        for corner_a in [0.0f32, 1.0] {
            for corner_b in [0.0f32, 1.0] {
                let mut min = [0.0f32; 3];
                let mut max = [0.0f32; 3];

                // The edge runs the full length of the block, overshooting by
                // half a thickness at each end so corners meet cleanly.
                min[axis] = -THICKNESS / 2.0;
                max[axis] = 1.0 + THICKNESS / 2.0;

                for (other, corner) in [(other_a, corner_a), (other_b, corner_b)] {
                    // Push away from the block's centre, not toward it.
                    let outward = if corner == 0.0 { -INFLATE } else { INFLATE };
                    min[other] = corner + outward - THICKNESS / 2.0;
                    max[other] = corner + outward + THICKNESS / 2.0;
                }

                push_box(&mut vertices, &mut indices, origin, min, max, colour);
            }
        }
    }

    (vertices, indices)
}

/// Append one axis-aligned box. Winding is irrelevant — the pipeline does not
/// cull — so the faces are listed in whatever order reads clearest.
fn push_box(
    vertices: &mut Vec<SelectionVertex>,
    indices: &mut Vec<u32>,
    origin: [f32; 3],
    min: [f32; 3],
    max: [f32; 3],
    colour: [f32; 4],
) {
    let base = vertices.len() as u32;

    for corner in 0..8 {
        let pick = |axis: usize, bit: usize| {
            if corner & (1 << bit) == 0 {
                min[axis]
            } else {
                max[axis]
            }
        };
        vertices.push(SelectionVertex {
            position: [
                origin[0] + pick(0, 0),
                origin[1] + pick(1, 1),
                origin[2] + pick(2, 2),
            ],
            colour,
        });
    }

    // Corner index bits are (x, y, z), so opposite faces differ by one bit.
    const FACES: [[u32; 4]; 6] = [
        [0, 2, 6, 4], // -z
        [1, 5, 7, 3], // +z
        [0, 1, 3, 2], // -x
        [4, 6, 7, 5], // +x
        [0, 4, 5, 1], // -y
        [2, 3, 7, 6], // +y
    ];

    for face in FACES {
        indices.extend_from_slice(&[
            base + face[0],
            base + face[1],
            base + face[2],
            base + face[0],
            base + face[2],
            base + face[3],
        ]);
    }
}

/// GPU resources for the outline.
pub struct SelectionRenderer {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    current: Option<BlockPos>,
    colour: [f32; 4],
}

impl SelectionRenderer {
    pub fn new(
        device: &wgpu::Device,
        camera_layout: &wgpu::BindGroupLayout,
        target_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        colour: [f32; 4],
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("selection shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("selection.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("selection pipeline layout"),
            bind_group_layouts: &[Some(camera_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("selection pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[SELECTION_VERTEX_LAYOUT],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // Thin boxes seen edge-on would lose half their faces to
                // culling, so keep both sides.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                // Depth-tested so terrain in front hides the outline, but it
                // writes nothing: the cage must not occlude the world.
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        // The geometry is a fixed twelve boxes, so the buffers never grow.
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("selection vertices"),
            contents: &vec![0u8; EDGES * VERTICES_PER_EDGE * std::mem::size_of::<SelectionVertex>()],
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("selection indices"),
            contents: &vec![0u8; EDGES * INDICES_PER_EDGE * std::mem::size_of::<u32>()],
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        });

        SelectionRenderer {
            pipeline,
            vertex_buffer,
            index_buffer,
            index_count: 0,
            current: None,
            colour,
        }
    }

    /// Point the outline at a block, or clear it with `None`.
    ///
    /// Re-uploading only on change keeps this free while the player holds
    /// still, which is most frames.
    pub fn set_target(&mut self, queue: &wgpu::Queue, target: Option<BlockPos>) {
        if target == self.current {
            return;
        }
        self.current = target;

        let Some(pos) = target else {
            self.index_count = 0;
            return;
        };

        let (vertices, indices) = build_outline(pos, self.colour);
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&vertices));
        queue.write_buffer(&self.index_buffer, 0, bytemuck::cast_slice(&indices));
        self.index_count = indices.len() as u32;
    }

    pub fn target(&self) -> Option<BlockPos> {
        self.current
    }

    /// Record the draw. The caller has already bound the camera group.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.index_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GREEN: [f32; 4] = [0.0, 1.0, 0.25, 1.0];

    #[test]
    fn an_outline_has_twelve_boxes_worth_of_geometry() {
        let (vertices, indices) = build_outline(BlockPos::new(0, 0, 0), GREEN);
        assert_eq!(vertices.len(), EDGES * VERTICES_PER_EDGE);
        assert_eq!(indices.len(), EDGES * INDICES_PER_EDGE);
        assert!(indices.iter().all(|&i| (i as usize) < vertices.len()));
    }

    #[test]
    fn the_cage_wraps_the_block_it_targets() {
        // Every vertex must sit within a hair of the block's own bounds, or
        // the outline is drawn around the wrong cube.
        let pos = BlockPos::new(4, 70, -3);
        let (vertices, _) = build_outline(pos, GREEN);
        let slack = THICKNESS + INFLATE * 2.0;

        for vertex in &vertices {
            let origin = [pos.x as f32, pos.y as f32, pos.z as f32];
            for (axis, base) in origin.iter().enumerate() {
                let local = vertex.position[axis] - base;
                assert!(
                    (-slack..=1.0 + slack).contains(&local),
                    "vertex {:?} escapes the block on axis {axis}",
                    vertex.position
                );
            }
        }
    }

    #[test]
    fn the_outline_follows_the_block_position() {
        let (at_origin, _) = build_outline(BlockPos::new(0, 0, 0), GREEN);
        let (moved, _) = build_outline(BlockPos::new(10, -5, 7), GREEN);

        for (a, b) in at_origin.iter().zip(moved.iter()) {
            assert!((b.position[0] - a.position[0] - 10.0).abs() < 1e-5);
            assert!((b.position[1] - a.position[1] + 5.0).abs() < 1e-5);
            assert!((b.position[2] - a.position[2] - 7.0).abs() < 1e-5);
        }
    }

    #[test]
    fn edges_are_hollow_and_leave_the_block_face_clear() {
        // Nothing may sit in the middle of a face, or the "outline" is a solid
        // box that hides the block it is highlighting.
        let (vertices, _) = build_outline(BlockPos::new(0, 0, 0), GREEN);
        let middle_of_a_face = [0.5f32, 0.5, 0.0];

        let covered = vertices.iter().any(|v| {
            (v.position[0] - middle_of_a_face[0]).abs() < 0.2
                && (v.position[1] - middle_of_a_face[1]).abs() < 0.2
        });
        assert!(!covered, "geometry covers the centre of a face");
    }

    #[test]
    fn every_edge_is_thin_on_exactly_two_axes() {
        // An edge runs the length of the block on one axis and is a thin bar
        // on the other two. Anything else means the box construction is wrong.
        let (vertices, _) = build_outline(BlockPos::new(0, 0, 0), GREEN);

        for edge in vertices.chunks_exact(VERTICES_PER_EDGE) {
            let mut long_axes = 0;
            for axis in 0..3 {
                let lo = edge.iter().map(|v| v.position[axis]).fold(f32::INFINITY, f32::min);
                let hi = edge
                    .iter()
                    .map(|v| v.position[axis])
                    .fold(f32::NEG_INFINITY, f32::max);
                let span = hi - lo;
                if span > 0.5 {
                    long_axes += 1;
                } else {
                    assert!(span <= THICKNESS + 1e-5, "edge is {span} thick");
                }
            }
            assert_eq!(long_axes, 1, "an edge should be long on one axis only");
        }
    }

    #[test]
    fn the_colour_reaches_every_vertex() {
        let (vertices, _) = build_outline(BlockPos::new(0, 0, 0), GREEN);
        assert!(vertices.iter().all(|v| v.colour == GREEN));
    }

    #[test]
    fn the_vertex_layout_matches_the_struct() {
        assert_eq!(std::mem::size_of::<SelectionVertex>(), 28);
        assert_eq!(SELECTION_VERTEX_LAYOUT.array_stride, 28);
    }
}
