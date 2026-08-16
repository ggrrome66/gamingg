//! Screen-space UI: the crosshair, the HUD readout and the menus.
//!
//! Geometry is built on the CPU every frame in pixel coordinates and converted
//! to clip space here. That sounds wasteful, but the whole overlay is a few
//! hundred quads — far cheaper to rebuild than to diff — and it means the UI
//! layout code never touches a GPU type.
//!
//! Coordinates are pixels with the origin **top-left**, the convention every
//! UI layout is easier to reason about in. Clip space is Y-up, so the
//! conversion flips it.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::font;

/// One overlay corner.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct OverlayVertex {
    /// Clip-space position.
    position: [f32; 2],
    /// Font atlas coordinate. Ignored for solid quads.
    uv: [f32; 2],
    colour: [f32; 4],
    /// 1 to modulate by the font atlas, 0 for a flat fill.
    textured: u32,
}

impl OverlayVertex {
    /// Clip-space position, for callers checking what was laid out.
    pub fn position(self) -> [f32; 2] {
        self.position
    }

    pub fn colour(self) -> [f32; 4] {
        self.colour
    }
}

pub const OVERLAY_VERTEX_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: std::mem::size_of::<OverlayVertex>() as wgpu::BufferAddress,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &[
        wgpu::VertexAttribute {
            offset: 0,
            shader_location: 0,
            format: wgpu::VertexFormat::Float32x2,
        },
        wgpu::VertexAttribute {
            offset: 8,
            shader_location: 1,
            format: wgpu::VertexFormat::Float32x2,
        },
        wgpu::VertexAttribute {
            offset: 16,
            shader_location: 2,
            format: wgpu::VertexFormat::Float32x4,
        },
        wgpu::VertexAttribute {
            offset: 32,
            shader_location: 3,
            format: wgpu::VertexFormat::Uint32,
        },
    ],
};

/// Accumulates overlay geometry in pixel space.
#[derive(Debug, Clone)]
pub struct OverlayBuilder {
    vertices: Vec<OverlayVertex>,
    indices: Vec<u32>,
    width: f32,
    height: f32,
    /// Pixels per font pixel. Keeps the UI readable as the window grows.
    scale: f32,
}

impl OverlayBuilder {
    pub fn new(width: u32, height: u32, scale: f32) -> Self {
        OverlayBuilder {
            vertices: Vec::new(),
            indices: Vec::new(),
            width: width.max(1) as f32,
            height: height.max(1) as f32,
            scale: scale.max(1.0),
        }
    }

    pub fn size(&self) -> (f32, f32) {
        (self.width, self.height)
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Change the text size for subsequent calls, for headings and the like.
    ///
    /// Kept as a mode rather than a per-call argument because a heading is
    /// usually several calls — title, rule, subtitle — and threading a scale
    /// through every one of them reads worse than setting it once.
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.max(1.0);
    }

    /// Height of one line of text, including the gap below it.
    pub fn line_height(&self) -> f32 {
        font::CELL as f32 * self.scale
    }

    /// Width of `text` when drawn at the current scale.
    pub fn text_width(&self, text: &str) -> f32 {
        text.chars().count() as f32 * self.advance()
    }

    /// Horizontal distance from one character's origin to the next.
    fn advance(&self) -> f32 {
        (font::GLYPH_WIDTH + 1) as f32 * self.scale
    }

    fn to_clip(&self, x: f32, y: f32) -> [f32; 2] {
        [
            (x / self.width) * 2.0 - 1.0,
            // Pixel space grows downward, clip space grows upward.
            1.0 - (y / self.height) * 2.0,
        ]
    }

    /// Append a quad. `bounds` is `(x, y, width, height)` in pixels and `uv`
    /// is the atlas rectangle as `(u0, v0, u1, v1)`.
    fn quad(
        &mut self,
        bounds: (f32, f32, f32, f32),
        colour: [f32; 4],
        uv: (f32, f32, f32, f32),
        textured: u32,
    ) {
        let (x, y, w, h) = bounds;
        if w <= 0.0 || h <= 0.0 || colour[3] <= 0.0 {
            return;
        }
        let base = self.vertices.len() as u32;
        let (u0, v0, u1, v1) = uv;

        for (px, py, u, v) in [
            (x, y, u0, v0),
            (x + w, y, u1, v0),
            (x + w, y + h, u1, v1),
            (x, y + h, u0, v1),
        ] {
            self.vertices.push(OverlayVertex {
                position: self.to_clip(px, py),
                uv: [u, v],
                colour,
                textured,
            });
        }

        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// A filled rectangle.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, colour: [f32; 4]) {
        self.quad((x, y, w, h), colour, (0.0, 0.0, 0.0, 0.0), 0);
    }

    /// A rectangle outline drawn inside the given bounds.
    pub fn rect_outline(&mut self, x: f32, y: f32, w: f32, h: f32, thickness: f32, colour: [f32; 4]) {
        let t = thickness.max(1.0);
        self.rect(x, y, w, t, colour);
        self.rect(x, y + h - t, w, t, colour);
        self.rect(x, y + t, t, h - 2.0 * t, colour);
        self.rect(x + w - t, y + t, t, h - 2.0 * t, colour);
    }

    /// Draw `text` with its top-left corner at `(x, y)`.
    ///
    /// Characters the face has no glyph for are skipped but still advance, so
    /// alignment survives them.
    pub fn text(&mut self, x: f32, y: f32, text: &str, colour: [f32; 4]) {
        let advance = self.advance();
        let w = font::GLYPH_WIDTH as f32 * self.scale;
        let h = font::GLYPH_HEIGHT as f32 * self.scale;

        for (position, ch) in text.chars().enumerate() {
            let Some(index) = font::glyph_index(ch) else {
                continue;
            };
            // Space has a blank cell; skipping it saves two triangles per gap.
            if ch == ' ' {
                continue;
            }
            let at = x + position as f32 * advance;
            self.quad((at, y, w, h), colour, font::glyph_uv(index), 1);
        }
    }

    /// Draw `text` centred horizontally on `centre_x`.
    pub fn text_centred(&mut self, centre_x: f32, y: f32, text: &str, colour: [f32; 4]) {
        let x = centre_x - self.text_width(text) / 2.0;
        self.text(x, y, text, colour);
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn index_count(&self) -> u32 {
        self.indices.len() as u32
    }

    pub fn vertices(&self) -> &[OverlayVertex] {
        &self.vertices
    }

    pub fn indices(&self) -> &[u32] {
        &self.indices
    }
}

/// GPU resources for drawing an [`OverlayBuilder`].
pub struct OverlayRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    vertex_capacity: usize,
    index_capacity: usize,
    index_count: u32,
}

impl OverlayRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("overlay.wgsl").into()),
        });

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("font atlas"),
            size: wgpu::Extent3d {
                width: font::ATLAS_WIDTH,
                height: font::ATLAS_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &font::atlas_pixels(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(font::ATLAS_WIDTH),
                rows_per_image: Some(font::ATLAS_HEIGHT),
            },
            wgpu::Extent3d {
                width: font::ATLAS_WIDTH,
                height: font::ATLAS_HEIGHT,
                depth_or_array_layers: 1,
            },
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        // Nearest, deliberately: filtering a bitmap face turns crisp pixels
        // into grey mush, which is exactly the look this is avoiding.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("font sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay bind group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[OVERLAY_VERTEX_LAYOUT],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // The overlay is flat quads with no meaningful facing, so
                // culling would only ever discard them by accident.
                cull_mode: None,
                ..Default::default()
            },
            // Shares the render pass with the world, so it must declare the
            // same depth attachment — but it draws on top unconditionally and
            // writes nothing back.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
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

        // Start with room for a modest HUD; it grows on demand.
        let vertex_capacity = 1024;
        let index_capacity = 1536;

        OverlayRenderer {
            pipeline,
            bind_group,
            vertex_buffer: empty_buffer(
                device,
                "overlay vertices",
                wgpu::BufferUsages::VERTEX,
                vertex_capacity * std::mem::size_of::<OverlayVertex>(),
            ),
            index_buffer: empty_buffer(
                device,
                "overlay indices",
                wgpu::BufferUsages::INDEX,
                index_capacity * std::mem::size_of::<u32>(),
            ),
            vertex_capacity,
            index_capacity,
            index_count: 0,
        }
    }

    /// Replace the overlay geometry.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, built: &OverlayBuilder) {
        self.index_count = built.index_count();
        if built.is_empty() {
            return;
        }

        // Reallocating only on growth keeps the steady state to two writes.
        if built.vertices().len() > self.vertex_capacity {
            self.vertex_capacity = built.vertices().len().next_power_of_two();
            self.vertex_buffer = empty_buffer(
                device,
                "overlay vertices",
                wgpu::BufferUsages::VERTEX,
                self.vertex_capacity * std::mem::size_of::<OverlayVertex>(),
            );
        }
        if built.indices().len() > self.index_capacity {
            self.index_capacity = built.indices().len().next_power_of_two();
            self.index_buffer = empty_buffer(
                device,
                "overlay indices",
                wgpu::BufferUsages::INDEX,
                self.index_capacity * std::mem::size_of::<u32>(),
            );
        }

        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(built.vertices()));
        queue.write_buffer(&self.index_buffer, 0, bytemuck::cast_slice(built.indices()));
    }

    /// Record the overlay draw into an in-progress pass.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.index_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}

fn empty_buffer(
    device: &wgpu::Device,
    label: &str,
    usage: wgpu::BufferUsages,
    size: usize,
) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: &vec![0u8; size],
        usage: usage | wgpu::BufferUsages::COPY_DST,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

    #[test]
    fn a_fresh_builder_has_no_geometry() {
        let builder = OverlayBuilder::new(320, 240, 2.0);
        assert!(builder.is_empty());
        assert_eq!(builder.index_count(), 0);
    }

    #[test]
    fn a_rect_makes_one_quad() {
        let mut builder = OverlayBuilder::new(320, 240, 1.0);
        builder.rect(10.0, 20.0, 30.0, 40.0, WHITE);

        assert_eq!(builder.vertices().len(), 4);
        assert_eq!(builder.indices().len(), 6);
    }

    #[test]
    fn pixel_coordinates_map_onto_clip_space_with_y_flipped() {
        // Top-left of the screen must land at clip (-1, +1). Getting the flip
        // wrong renders the whole UI upside down.
        let builder = OverlayBuilder::new(320, 240, 1.0);

        assert_eq!(builder.to_clip(0.0, 0.0), [-1.0, 1.0]);
        assert_eq!(builder.to_clip(320.0, 240.0), [1.0, -1.0]);
        assert_eq!(builder.to_clip(160.0, 120.0), [0.0, 0.0]);
    }

    #[test]
    fn degenerate_and_invisible_quads_are_dropped() {
        let mut builder = OverlayBuilder::new(320, 240, 1.0);
        builder.rect(0.0, 0.0, 0.0, 10.0, WHITE);
        builder.rect(0.0, 0.0, 10.0, -5.0, WHITE);
        builder.rect(0.0, 0.0, 10.0, 10.0, [1.0, 1.0, 1.0, 0.0]);

        assert!(builder.is_empty(), "invisible geometry was still emitted");
    }

    #[test]
    fn text_emits_one_quad_per_inked_character() {
        let mut builder = OverlayBuilder::new(320, 240, 1.0);
        builder.text(0.0, 0.0, "AB", WHITE);
        assert_eq!(builder.vertices().len(), 8);

        // Spaces are blank in the atlas, so they cost no geometry.
        let mut spaced = OverlayBuilder::new(320, 240, 1.0);
        spaced.text(0.0, 0.0, "A B", WHITE);
        assert_eq!(spaced.vertices().len(), 8);
    }

    #[test]
    fn unknown_characters_still_advance_the_cursor() {
        // Otherwise a stray character shifts the rest of the line left and
        // columns stop lining up.
        let mut builder = OverlayBuilder::new(320, 240, 1.0);
        builder.text(0.0, 0.0, "A~B", WHITE);

        assert_eq!(builder.vertices().len(), 8, "the tilde drew something");
        let xs: Vec<f32> = builder.vertices().iter().map(|v| v.position[0]).collect();
        // The B sits two advances along, not one.
        let advance = builder.advance();
        let expected = builder.to_clip(2.0 * advance, 0.0)[0];
        assert!(
            xs.iter().any(|x| (x - expected).abs() < 1e-5),
            "B is not in the third column"
        );
    }

    #[test]
    fn text_width_matches_what_was_drawn() {
        let builder = OverlayBuilder::new(320, 240, 2.0);
        assert_eq!(builder.text_width(""), 0.0);
        assert_eq!(builder.text_width("AAA"), 3.0 * builder.advance());
    }

    #[test]
    fn centred_text_straddles_the_centre_line() {
        let mut builder = OverlayBuilder::new(320, 240, 1.0);
        let width = builder.text_width("HELLO");
        builder.text_centred(160.0, 0.0, "HELLO", WHITE);

        let xs: Vec<f32> = builder.vertices().iter().map(|v| v.position[0]).collect();
        let left = xs.iter().cloned().fold(f32::INFINITY, f32::min);
        let expected_left = builder.to_clip(160.0 - width / 2.0, 0.0)[0];
        assert!((left - expected_left).abs() < 1e-5, "{left} vs {expected_left}");
    }

    #[test]
    fn scale_makes_text_bigger_not_more_numerous() {
        let mut small = OverlayBuilder::new(320, 240, 1.0);
        small.text(0.0, 0.0, "TEST", WHITE);
        let mut large = OverlayBuilder::new(320, 240, 3.0);
        large.text(0.0, 0.0, "TEST", WHITE);

        assert_eq!(small.vertices().len(), large.vertices().len());
        assert!(large.text_width("TEST") > small.text_width("TEST"));
        assert!(large.line_height() > small.line_height());
    }

    #[test]
    fn an_outline_is_four_bars_and_leaves_the_middle_alone() {
        let mut builder = OverlayBuilder::new(320, 240, 1.0);
        builder.rect_outline(10.0, 10.0, 100.0, 50.0, 2.0, WHITE);

        assert_eq!(builder.vertices().len(), 16, "expected four bars");
        // Nothing may cover the centre of the box.
        let centre = builder.to_clip(60.0, 35.0);
        let covered = builder.vertices().chunks_exact(4).any(|quad| {
            let xs: Vec<f32> = quad.iter().map(|v| v.position[0]).collect();
            let ys: Vec<f32> = quad.iter().map(|v| v.position[1]).collect();
            let (x0, x1) = (xs.iter().cloned().fold(f32::INFINITY, f32::min), xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max));
            let (y0, y1) = (ys.iter().cloned().fold(f32::INFINITY, f32::min), ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max));
            (x0..=x1).contains(&centre[0]) && (y0..=y1).contains(&centre[1])
        });
        assert!(!covered, "the outline filled its own interior");
    }

    #[test]
    fn indices_stay_within_the_vertices_emitted() {
        // An out-of-range index is a GPU-side crash rather than a wrong pixel.
        let mut builder = OverlayBuilder::new(320, 240, 2.0);
        builder.rect(0.0, 0.0, 10.0, 10.0, WHITE);
        builder.text(5.0, 5.0, "MENU 01", WHITE);
        builder.rect_outline(1.0, 1.0, 50.0, 20.0, 1.0, WHITE);

        let count = builder.vertices().len() as u32;
        assert!(builder.indices().iter().all(|&i| i < count));
        assert_eq!(builder.indices().len() % 3, 0);
    }

    #[test]
    fn the_vertex_layout_matches_the_struct() {
        assert_eq!(std::mem::size_of::<OverlayVertex>(), 36);
        assert_eq!(OVERLAY_VERTEX_LAYOUT.array_stride, 36);
        let offsets: Vec<u64> = OVERLAY_VERTEX_LAYOUT
            .attributes
            .iter()
            .map(|a| a.offset)
            .collect();
        assert_eq!(offsets, vec![0, 8, 16, 32]);
    }
}
