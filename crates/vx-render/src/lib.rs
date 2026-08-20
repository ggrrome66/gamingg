//! The wgpu renderer.
//!
//! Owns the pipeline, the camera uniform, block textures and the per-chunk
//! GPU geometry. It knows nothing about windowing: it draws into whatever
//! texture view it is handed, which is what lets the same code path render to
//! a window and to an offscreen image in tests.

pub mod camera;
pub mod font;
pub mod gpu;
pub mod headless;
pub mod mesh_buffer;
pub mod overlay;
pub mod selection;
pub mod tiles;

use std::collections::HashMap;

use vx_core::{BlockPos, ChunkPos};
use vx_mesh::Mesh;

pub use camera::{Camera, CameraUniform};
pub use gpu::{GpuContext, GpuError, WindowSurface, DEPTH_FORMAT};
pub use mesh_buffer::{ChunkMesh, VERTEX_LAYOUT};
pub use overlay::{OverlayBuilder, OverlayRenderer, Transform2};
pub use selection::{SelectionRenderer, SELECTION_COLOUR};
pub use tiles::TileTextures;

/// Sky colour, used to clear each frame.
pub const SKY_COLOUR: wgpu::Color = wgpu::Color {
    r: 0.62,
    g: 0.74,
    b: 0.88,
    a: 1.0,
};

/// Draws the world.
pub struct Renderer {
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    tiles: TileTextures,
    depth_view: wgpu::TextureView,
    chunks: HashMap<ChunkPos, ChunkMesh>,
    selection: SelectionRenderer,
    overlay: OverlayRenderer,
    width: u32,
    height: u32,
}

impl Renderer {
    pub fn new(
        context: &GpuContext,
        target_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let device = &context.device;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera uniform"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera bind group"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        let tiles = TileTextures::new(device, &context.queue);

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain pipeline layout"),
            bind_group_layouts: &[Some(&camera_layout), Some(&tiles.bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[VERTEX_LAYOUT],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                // The mesher winds quads counter-clockwise seen from outside.
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
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
                    // Straight alpha blending. Water is the only translucent
                    // block so far and is rarely seen through another water
                    // surface; proper sorted transparency can wait until it
                    // actually matters.
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let depth_view = context.create_depth_view(width, height);

        let selection = SelectionRenderer::new(
            device,
            &camera_layout,
            target_format,
            DEPTH_FORMAT,
            SELECTION_COLOUR,
        );
        let overlay = OverlayRenderer::new(device, &context.queue, target_format, DEPTH_FORMAT);

        Renderer {
            pipeline,
            camera_buffer,
            camera_bind_group,
            tiles,
            depth_view,
            chunks: HashMap::new(),
            selection,
            overlay,
            width,
            height,
        }
    }

    /// Outline the block being looked at, or clear it with `None`.
    pub fn set_selection(&mut self, queue: &wgpu::Queue, target: Option<BlockPos>) {
        self.selection.set_target(queue, target);
    }

    /// The block currently outlined.
    pub fn selection(&self) -> Option<BlockPos> {
        self.selection.target()
    }

    /// Replace the screen-space overlay.
    pub fn set_overlay(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        built: &OverlayBuilder,
    ) {
        self.overlay.upload(device, queue, built);
    }

    /// A builder sized to this renderer's target, at a scale that keeps the UI
    /// legible without depending on the window size.
    pub fn overlay_builder(&self) -> OverlayBuilder {
        OverlayBuilder::new(self.width, self.height, self.ui_scale())
    }

    /// Integer UI scale. Text is a bitmap face, so a fractional scale would
    /// land glyph pixels between screen pixels and blur them.
    pub fn ui_scale(&self) -> f32 {
        (self.height as f32 / 240.0).floor().clamp(1.0, 6.0)
    }

    /// Rebuild the depth buffer for a new target size.
    pub fn resize(&mut self, context: &GpuContext, width: u32, height: u32) {
        if width == 0 || height == 0 || (width == self.width && height == self.height) {
            return;
        }
        self.width = width;
        self.height = height;
        self.depth_view = context.create_depth_view(width, height);
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Push the camera's current transform to the GPU.
    pub fn update_camera(&self, queue: &wgpu::Queue, camera: &Camera) {
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&camera.uniform()));
    }

    /// Upload (or replace) one chunk's geometry. An empty mesh drops it.
    pub fn set_chunk_mesh(&mut self, device: &wgpu::Device, pos: ChunkPos, mesh: &Mesh) {
        match ChunkMesh::upload(device, mesh, &format!("chunk {},{}", pos.x, pos.z)) {
            Some(uploaded) => {
                self.chunks.insert(pos, uploaded);
            }
            None => {
                self.chunks.remove(&pos);
            }
        }
    }

    pub fn remove_chunk(&mut self, pos: ChunkPos) {
        self.chunks.remove(&pos);
    }

    pub fn loaded_chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Triangles currently uploaded, for the debug readout.
    pub fn triangle_count(&self) -> u32 {
        self.chunks.values().map(|mesh| mesh.index_count() / 3).sum()
    }

    /// Record a full frame into `encoder`, drawing into `target`.
    pub fn render(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("terrain pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(SKY_COLOUR),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        pass.set_bind_group(1, &self.tiles.bind_group, &[]);

        for mesh in self.chunks.values() {
            mesh.draw(&mut pass);
        }

        // Same pass, different pipelines: the outline is depth-tested against
        // the terrain just drawn, and the overlay ignores depth entirely and
        // lands on top of both.
        self.selection.draw(&mut pass);
        self.overlay.draw(&mut pass);
    }
}
