//! Chunk geometry on the GPU.

use wgpu::util::DeviceExt;

use vx_mesh::{Mesh, PackedQuad, Vertex};

/// The instance layout the terrain shader expects: one packed quad per
/// instance, drawn as six synthesised vertices. Must stay in step with
/// [`PackedQuad`] and `shader.wgsl`'s `QuadInput`.
pub const QUAD_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: std::mem::size_of::<PackedQuad>() as wgpu::BufferAddress,
    step_mode: wgpu::VertexStepMode::Instance,
    attributes: &[wgpu::VertexAttribute {
        offset: 0,
        shader_location: 0,
        format: wgpu::VertexFormat::Uint32x2,
    }],
};

/// The vertex layout the shader expects. Must stay in step with [`Vertex`]
/// and with `shader.wgsl`'s `VertexInput`.
pub const VERTEX_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &[
        wgpu::VertexAttribute {
            offset: 0,
            shader_location: 0,
            format: wgpu::VertexFormat::Float32x3, // position
        },
        wgpu::VertexAttribute {
            offset: 12,
            shader_location: 1,
            format: wgpu::VertexFormat::Float32x3, // normal
        },
        wgpu::VertexAttribute {
            offset: 24,
            shader_location: 2,
            format: wgpu::VertexFormat::Float32x2, // uv
        },
        wgpu::VertexAttribute {
            offset: 32,
            shader_location: 3,
            format: wgpu::VertexFormat::Uint32, // tile
        },
    ],
};

/// One chunk's geometry, uploaded and ready to draw.
///
/// One buffer of packed quads and a small uniform holding where the chunk sits.
/// No vertex buffer and no index buffer: the six vertices of a quad's two
/// triangles are synthesised in the vertex shader from `vertex_index`, so the
/// only geometry on the GPU is eight bytes per quad.
pub struct ChunkMesh {
    quads: wgpu::Buffer,
    quad_count: u32,
    /// Held only so the bind group's buffer outlives it.
    _origin: wgpu::Buffer,
    origin_bind_group: wgpu::BindGroup,
}

impl ChunkMesh {
    /// The layout of the per-chunk origin uniform, at `@group(3)`.
    pub fn origin_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chunk origin layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        })
    }

    /// Upload `mesh`, or `None` if it has no geometry — empty buffers are
    /// invalid in wgpu, and an air chunk is the common case.
    pub fn upload(
        device: &wgpu::Device,
        mesh: &Mesh,
        label: &str,
        chunk_origin: [f32; 3],
        origin_layout: &wgpu::BindGroupLayout,
    ) -> Option<Self> {
        if mesh.quads.is_empty() {
            return None;
        }

        let quads = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{label} quads")),
            contents: bytemuck::cast_slice(&mesh.quads),
            usage: wgpu::BufferUsages::VERTEX,
        });
        // Padded to a vec4: uniform members align to 16 bytes.
        let origin_value = [chunk_origin[0], chunk_origin[1], chunk_origin[2], 0.0];
        let origin = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{label} origin")),
            contents: bytemuck::cast_slice(&origin_value),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let origin_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("{label} origin bind group")),
            layout: origin_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: origin.as_entire_binding(),
            }],
        });

        Some(ChunkMesh {
            quads,
            quad_count: mesh.quads.len() as u32,
            _origin: origin,
            origin_bind_group,
        })
    }

    pub fn quad_count(&self) -> u32 {
        self.quad_count
    }

    /// Indices this chunk would have needed in the old vertex form, for the
    /// triangle readout.
    pub fn index_count(&self) -> u32 {
        self.quad_count * 6
    }

    /// Record the draw. The caller has already bound the pipeline and the
    /// camera, tile and sun groups.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(3, &self.origin_bind_group, &[]);
        pass.set_vertex_buffer(0, self.quads.slice(..));
        // Six vertices, one instance per quad.
        pass.draw(0..6, 0..self.quad_count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quad_layout_matches_the_packed_quad() {
        // A mismatch here scrambles every field and shows up as geometry
        // exploding across the world, so pin it explicitly.
        assert_eq!(std::mem::size_of::<PackedQuad>(), 8);
        assert_eq!(QUAD_LAYOUT.array_stride, 8);
        assert_eq!(QUAD_LAYOUT.step_mode, wgpu::VertexStepMode::Instance);
        assert_eq!(QUAD_LAYOUT.attributes.len(), 1);
        assert_eq!(QUAD_LAYOUT.attributes[0].offset, 0);
        assert_eq!(QUAD_LAYOUT.attributes[0].shader_location, 0);
        assert_eq!(
            QUAD_LAYOUT.attributes[0].format,
            wgpu::VertexFormat::Uint32x2
        );
    }

    #[test]
    fn a_quad_is_a_twentieth_of_what_it_used_to_cost() {
        // Four vertices plus six indices, against eight bytes.
        let before = std::mem::size_of::<Vertex>() * 4 + 4 * 6;
        let after = std::mem::size_of::<PackedQuad>();
        assert_eq!(before, 168);
        assert_eq!(after, 8);
        assert!(before / after >= 20);
    }

    #[test]
    fn the_vertex_layout_matches_the_vertex_struct() {
        // A mismatch here corrupts every attribute downstream and shows up as
        // scrambled geometry, so pin the offsets explicitly.
        assert_eq!(std::mem::size_of::<Vertex>(), 36);
        assert_eq!(VERTEX_LAYOUT.array_stride, 36);

        let offsets: Vec<u64> = VERTEX_LAYOUT.attributes.iter().map(|a| a.offset).collect();
        assert_eq!(offsets, vec![0, 12, 24, 32]);

        let locations: Vec<u32> = VERTEX_LAYOUT
            .attributes
            .iter()
            .map(|a| a.shader_location)
            .collect();
        assert_eq!(locations, vec![0, 1, 2, 3]);
    }

    #[test]
    fn the_declared_formats_cover_the_whole_stride() {
        let total: u64 = VERTEX_LAYOUT
            .attributes
            .iter()
            .map(|attribute| attribute.format.size())
            .sum();
        assert_eq!(total, VERTEX_LAYOUT.array_stride);
    }
}
