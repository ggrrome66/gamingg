//! The wgpu renderer.
//!
//! Owns the pipeline, the camera uniform, block textures and the per-chunk
//! GPU geometry. It knows nothing about windowing: it draws into whatever
//! texture view it is handed, which is what lets the same code path render to
//! a window and to an offscreen image in tests.

pub mod camera;
pub mod font;
pub mod frustum;
pub mod gpu;
pub mod headless;
pub mod mesh_buffer;
pub mod object;
pub mod overlay;
pub mod tiles;

use std::collections::HashMap;

use glam::Vec3;
use vx_core::{ChunkPos, CHUNK_HEIGHT, CHUNK_SIZE};
use vx_mesh::Mesh;

pub use camera::{Camera, CameraUniform};
pub use frustum::Frustum;
pub use gpu::{GpuContext, GpuError, WindowSurface, DEPTH_FORMAT};
pub use mesh_buffer::{ChunkMesh, QUAD_LAYOUT, VERTEX_LAYOUT};
pub use object::{Object, ObjectBatch, ObjectInstance, INSTANCE_LAYOUT};
pub use overlay::{OverlayPass, OverlayRect, OVERLAY_SLOTS};
pub use tiles::TileTextures;

/// Sun, sky and light level, as the shader sees them.
///
/// A second uniform rather than three more fields on the camera: the camera is
/// written every frame on the hot path, this is written when the light
/// changes, and when lamps arrive they belong here beside the sun rather than
/// inside a matrix.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable, PartialEq)]
pub struct SunUniform {
    /// Unit vector *toward* the key light. `w` unused.
    pub direction: [f32; 4],
    /// Linear sky colour — the frame's clear colour and its fog, one value,
    /// so the horizon can never disagree with the sky.
    pub sky: [f32; 4],
    /// `x` diffuse strength, `y` ambient floor, `zw` reserved.
    pub light: [f32; 4],
}

impl Default for SunUniform {
    /// Exactly the constants the shader used before there was a day, so a
    /// fresh renderer is pixel-identical to the build that had no clock.
    fn default() -> Self {
        SunUniform {
            direction: [0.42, 0.86, 0.29, 0.0],
            sky: [0.62, 0.74, 0.88, 1.0],
            light: [0.58, 0.42, 0.0, 0.0],
        }
    }
}

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
    /// Layout of the per-chunk origin uniform. Chunk meshes build their own
    /// bind group against it as they are uploaded.
    chunk_origin_layout: wgpu::BindGroupLayout,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    /// The sun, the sky and the light level. Pushed explicitly — never read
    /// from a clock in here, which is what keeps captures reproducible.
    sun_buffer: wgpu::Buffer,
    sun_bind_group: wgpu::BindGroup,
    /// The colour the frame clears to. Kept in step with the sun's sky by
    /// [`Renderer::set_sun`], which writes both from the same value.
    sky: wgpu::Color,
    tiles: TileTextures,
    depth_view: wgpu::TextureView,
    chunks: HashMap<ChunkPos, ChunkMesh>,
    /// Second pipeline for instanced objects. Shares the camera bind group,
    /// the tile textures and the depth buffer with the terrain pass.
    object_pipeline: wgpu::RenderPipeline,
    objects: ObjectBatch,
    /// The 2D overlay (minimap and friends). Draws only when set, so every
    /// path that never sets one — headless captures, the culling tests — is
    /// byte-identical to a build without it.
    overlay: OverlayPass,
    /// Refreshed whenever the camera moves. `None` until the first update, so
    /// a frame drawn before any camera is set shows everything rather than
    /// culling against a meaningless volume.
    frustum: Option<Frustum>,
    /// Off only for tests, which compare a culled frame against an unculled one
    /// to prove culling changes nothing visible.
    culling_enabled: bool,
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

        let sun_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sun uniform"),
            size: std::mem::size_of::<SunUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let sun_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sun layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let sun_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sun bind group"),
            layout: &sun_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: sun_buffer.as_entire_binding(),
            }],
        });

        // Start at the documented default, so a renderer nobody has told the
        // time to draws exactly what it drew before there was a clock.
        context
            .queue
            .write_buffer(&sun_buffer, 0, bytemuck::bytes_of(&SunUniform::default()));

        let tiles = TileTextures::new(device, &context.queue);

        // Terrain needs one more group than objects do: the chunk origin its
        // quads are relative to. Objects carry a full model matrix per
        // instance and have no chunk.
        let chunk_origin_layout = ChunkMesh::origin_layout(device);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain pipeline layout"),
            bind_group_layouts: &[
                Some(&camera_layout),
                Some(&tiles.bind_group_layout),
                Some(&sun_layout),
                Some(&chunk_origin_layout),
            ],
            immediate_size: 0,
        });
        let object_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("object pipeline layout"),
            bind_group_layouts: &[
                Some(&camera_layout),
                Some(&tiles.bind_group_layout),
                Some(&sun_layout),
            ],
            immediate_size: 0,
        });

        // Terrain and objects differ only in their vertex stage and buffers.
        // Sharing everything else — winding, depth state, blending, colour
        // target — is what keeps an object from shading or z-fighting
        // differently from the ground it stands on.
        let build_pipeline = |label: &str,
                              vertex_entry: &str,
                              buffers: &[wgpu::VertexBufferLayout<'_>],
                              layout: &wgpu::PipelineLayout| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vertex_entry),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    // The mesher winds quads counter-clockwise seen from
                    // outside, and `object::unit_cube` matches it.
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
            })
        };

        let pipeline = build_pipeline(
            "terrain pipeline",
            "vs_main",
            &[QUAD_LAYOUT],
            &pipeline_layout,
        );
        let object_pipeline = build_pipeline(
            "object pipeline",
            "vs_object",
            &[VERTEX_LAYOUT, INSTANCE_LAYOUT],
            &object_pipeline_layout,
        );

        let depth_view = context.create_depth_view(width, height);

        Renderer {
            pipeline,
            chunk_origin_layout,
            camera_buffer,
            sun_buffer,
            sun_bind_group,
            sky: SKY_COLOUR,
            camera_bind_group,
            tiles,
            depth_view,
            chunks: HashMap::new(),
            object_pipeline,
            objects: ObjectBatch::new(device),
            overlay: OverlayPass::new(device, target_format),
            frustum: None,
            culling_enabled: true,
            width,
            height,
        }
    }

    /// Turn frustum culling off. Only useful for proving it is invisible.
    pub fn set_culling_enabled(&mut self, enabled: bool) {
        self.culling_enabled = enabled;
    }

    /// The world-space bounding box of a chunk column.
    ///
    /// Spans the full world height. A tighter bound from each chunk's actual
    /// filled range would cull more, but needs the mesher to report it and is
    /// not where the win is.
    fn chunk_bounds(pos: ChunkPos) -> (Vec3, Vec3) {
        let origin = pos.origin();
        let min = Vec3::new(origin.x as f32, 0.0, origin.z as f32);
        let max = min + Vec3::new(CHUNK_SIZE as f32, CHUNK_HEIGHT as f32, CHUNK_SIZE as f32);
        (min, max)
    }

    /// Chunks that would be drawn this frame.
    pub fn visible_chunk_count(&self) -> usize {
        match (self.culling_enabled, self.frustum) {
            (true, Some(frustum)) => self
                .chunks
                .keys()
                .filter(|pos| {
                    let (min, max) = Self::chunk_bounds(**pos);
                    frustum.intersects_aabb(min, max)
                })
                .count(),
            _ => self.chunks.len(),
        }
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

    /// Push the camera's current transform to the GPU and refresh the frustum.
    ///
    /// Deriving the frustum from the very matrix that was just uploaded is what
    /// keeps culling and rasterisation from ever disagreeing.
    pub fn update_camera(&mut self, queue: &wgpu::Queue, camera: &Camera) {
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&camera.uniform()));
        self.frustum = Some(Frustum::from_view_projection(camera.view_projection()));
    }

    /// Push the sun, the sky and the light level.
    ///
    /// Both the shader's fog and the frame's clear colour come from `sun.sky`,
    /// written here in one call — so the horizon and the sky are the same
    /// value by construction and cannot drift apart.
    pub fn set_sun(&mut self, queue: &wgpu::Queue, sun: SunUniform) {
        queue.write_buffer(&self.sun_buffer, 0, bytemuck::bytes_of(&sun));
        self.sky = wgpu::Color {
            r: sun.sky[0] as f64,
            g: sun.sky[1] as f64,
            b: sun.sky[2] as f64,
            a: 1.0,
        };
    }

    /// The colour this renderer currently clears to.
    pub fn sky(&self) -> wgpu::Color {
        self.sky
    }

    /// Upload (or replace) one chunk's geometry. An empty mesh drops it.
    pub fn set_chunk_mesh(&mut self, device: &wgpu::Device, pos: ChunkPos, mesh: &Mesh) {
        let corner = pos.origin();
        match ChunkMesh::upload(
            device,
            mesh,
            &format!("chunk {},{}", pos.x, pos.z),
            [corner.x as f32, corner.y as f32, corner.z as f32],
            &self.chunk_origin_layout,
        ) {
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

    /// Replace the objects drawn this frame.
    ///
    /// Call it *after* [`Renderer::update_camera`]: objects are culled against
    /// the current frustum as they are uploaded, because a single instanced
    /// draw cannot skip an instance once it is in the buffer. The whole list is
    /// rebuilt each frame, which is what a moving swarm needs anyway.
    pub fn set_objects(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        objects: &[Object],
    ) {
        let cull = self.culling_enabled.then_some(self.frustum).flatten();
        self.objects.upload(device, queue, objects, cull);
    }

    /// Object instances that survived culling and will be drawn.
    pub fn visible_object_count(&self) -> u32 {
        self.objects.live_count()
    }

    /// Put a 2D picture on screen at `rect`, in one of the overlay slots,
    /// until cleared. See [`OverlayPass`].
    pub fn set_overlay(
        &mut self,
        slot: usize,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (u32, u32),
        pixels: &[u8],
        rect: OverlayRect,
    ) {
        let screen = (self.width, self.height);
        self.overlay
            .set_picture(slot, device, queue, size, pixels, rect, screen);
    }

    pub fn clear_overlay(&mut self, slot: usize) {
        self.overlay.clear(slot);
    }

    pub fn has_overlay(&self, slot: usize) -> bool {
        self.overlay.is_set(slot)
    }

    pub fn loaded_chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Triangles currently uploaded, for the debug readout.
    pub fn triangle_count(&self) -> u32 {
        self.chunks.values().map(|mesh| mesh.index_count() / 3).sum()
    }

    /// Record a full frame into `encoder`, drawing into `target`.
    ///
    /// Returns how many chunks were actually drawn, so callers can report the
    /// culling win rather than assume it.
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    ) -> usize {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("terrain pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(self.sky),
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
        pass.set_bind_group(2, &self.sun_bind_group, &[]);

        let cull = self.culling_enabled.then_some(self.frustum).flatten();

        let mut drawn = 0;
        for (pos, mesh) in &self.chunks {
            if let Some(frustum) = cull {
                let (min, max) = Self::chunk_bounds(*pos);
                if !frustum.intersects_aabb(min, max) {
                    continue;
                }
            }
            mesh.draw(&mut pass);
            drawn += 1;
        }

        // Objects last, in the same pass so they share the depth buffer the
        // terrain just wrote. A separate pass would have to load that depth
        // back, and any mismatch would show as drones floating over hills they
        // are standing behind.
        pass.set_pipeline(&self.object_pipeline);
        self.objects.draw(&mut pass);

        // And the 2D overlay over everything, if one is set.
        self.overlay.draw(&mut pass);

        drawn
    }
}
