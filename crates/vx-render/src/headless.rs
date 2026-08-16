//! Offscreen rendering and readback.
//!
//! Renders a frame to a texture and copies it back to the CPU. This is how the
//! render path gets verified without a GPU or a display — against a software
//! Vulkan implementation such as lavapipe — and how screenshots are produced.

use crate::{GpuContext, Renderer};

/// Format used for offscreen targets. Matches a typical sRGB surface so
/// offscreen output and on-screen output shade identically.
pub const CAPTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// A captured frame.
pub struct Capture {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8, row padding already removed.
    pub pixels: Vec<u8>,
}

impl Capture {
    /// RGBA of the pixel at `(x, y)`, with `(0, 0)` top-left.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[at],
            self.pixels[at + 1],
            self.pixels[at + 2],
            self.pixels[at + 3],
        ]
    }

    /// How many distinct colours the frame contains. A frame showing only the
    /// clear colour has exactly one.
    pub fn distinct_colours(&self) -> usize {
        let mut seen = std::collections::HashSet::new();
        for texel in self.pixels.chunks_exact(4) {
            seen.insert([texel[0], texel[1], texel[2], texel[3]]);
        }
        seen.len()
    }

    /// Fraction of pixels differing from `colour`, ignoring alpha.
    pub fn fraction_differing_from(&self, colour: [u8; 3]) -> f32 {
        let total = (self.width * self.height) as f32;
        let differing = self
            .pixels
            .chunks_exact(4)
            .filter(|texel| texel[0..3] != colour)
            .count();
        differing as f32 / total
    }

    /// Write a binary PPM. Chosen over PNG so this needs no image dependency;
    /// most viewers and ImageMagick read it.
    pub fn write_ppm(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
        write!(file, "P6\n{} {}\n255\n", self.width, self.height)?;
        for texel in self.pixels.chunks_exact(4) {
            file.write_all(&texel[0..3])?;
        }
        file.flush()
    }
}

/// Render one frame offscreen and read it back.
pub fn capture_frame(
    context: &GpuContext,
    renderer: &Renderer,
    width: u32,
    height: u32,
) -> Capture {
    let device = &context.device;

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("capture target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: CAPTURE_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    // Buffer rows must be a multiple of 256 bytes, so the readback is usually
    // padded and has to be unpacked below.
    let unpadded_row = width * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_row = unpadded_row.div_ceil(align) * align;

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("capture readback"),
        size: (padded_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("capture encoder"),
    });
    renderer.render(&mut encoder, &view);
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    context.queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| {
        result.expect("failed to map the readback buffer");
    });
    context
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("device poll failed while waiting for readback");

    let mapped = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity((unpadded_row * height) as usize);
    for row in 0..height {
        let start = (row * padded_row) as usize;
        let end = start + unpadded_row as usize;
        pixels.extend_from_slice(&mapped[start..end]);
    }
    drop(mapped);
    readback.unmap();

    Capture {
        width,
        height,
        pixels,
    }
}
