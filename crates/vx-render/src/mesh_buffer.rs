//! Chunk geometry on the GPU.

use wgpu::util::DeviceExt;

use vx_mesh::{Mesh, Vertex};

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
pub struct ChunkMesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

impl ChunkMesh {
    /// Upload `mesh`, or `None` if it has no geometry — empty buffers are
    /// invalid in wgpu, and an air chunk is the common case.
    pub fn upload(device: &wgpu::Device, mesh: &Mesh, label: &str) -> Option<Self> {
        if mesh.is_empty() {
            return None;
        }

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{label} vertices")),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{label} indices")),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        Some(ChunkMesh {
            vertex_buffer,
            index_buffer,
            index_count: mesh.indices.len() as u32,
        })
    }

    pub fn index_count(&self) -> u32 {
        self.index_count
    }

    /// Record the draw. The caller has already bound the pipeline and groups.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
