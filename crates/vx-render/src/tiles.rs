//! Block textures, as a GPU texture array.
//!
//! An array rather than an atlas, deliberately. Greedy meshing emits UVs that
//! span the whole merged quad (a 4×2 quad runs 0..4 by 0..2), so the texture
//! has to repeat across it. With an atlas that means doing the wrap by hand in
//! the shader, and the seam where `fract()` rolls over samples the neighbouring
//! tile — the classic atlas bleed. An array layer per tile lets the sampler's
//! `Repeat` address mode do it correctly for free.
//!
//! The textures here are generated procedurally: original placeholder art, so
//! the repository carries no borrowed assets. Replacing this with loaded PNGs
//! later changes nothing above it.

/// Edge length of one tile in pixels.
pub const TILE_SIZE: u32 = 16;

/// Tile slots, matching the texture indices `vx-world` assigns its built-ins.
pub mod slot {
    pub const STONE: u32 = 0;
    pub const DIRT: u32 = 1;
    pub const GRASS_TOP: u32 = 2;
    pub const GRASS_SIDE: u32 = 3;
    pub const SAND: u32 = 4;
    pub const WATER: u32 = 5;
    pub const BEDROCK: u32 = 6;
    pub const LAMP: u32 = 7;
    /// Total generated tiles.
    pub const COUNT: u32 = 8;
}

/// Deterministic per-pixel jitter, so tiles look grainy rather than flat.
fn jitter(tile: u32, x: u32, y: u32) -> f32 {
    let mut hash = (tile as u64)
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (x as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9)
        ^ (y as u64).wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^= hash >> 29;
    hash = hash.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash ^= hash >> 32;
    // Centred on zero, spanning roughly -0.5..0.5.
    ((hash >> 40) as f32) / ((1u32 << 24) as f32) - 0.5
}

fn shade(base: [f32; 3], amount: f32) -> [u8; 4] {
    [
        ((base[0] + amount).clamp(0.0, 1.0) * 255.0) as u8,
        ((base[1] + amount).clamp(0.0, 1.0) * 255.0) as u8,
        ((base[2] + amount).clamp(0.0, 1.0) * 255.0) as u8,
        255,
    ]
}

/// Generate the RGBA pixels for one tile.
fn generate_tile(tile: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((TILE_SIZE * TILE_SIZE * 4) as usize);

    for y in 0..TILE_SIZE {
        for x in 0..TILE_SIZE {
            let noise = jitter(tile, x, y);
            let texel = match tile {
                slot::STONE => shade([0.50, 0.50, 0.52], noise * 0.10),
                slot::DIRT => shade([0.45, 0.32, 0.21], noise * 0.12),
                slot::GRASS_TOP => shade([0.32, 0.55, 0.24], noise * 0.13),
                slot::GRASS_SIDE => {
                    // Dirt, with a band of grass draped over the top few rows.
                    // The band edge wobbles so it does not read as a ruler line.
                    let wobble = (jitter(tile, x, 0) * 2.0).round() as i32;
                    let band = (3 + wobble).clamp(1, 6) as u32;
                    if y < band {
                        shade([0.32, 0.55, 0.24], noise * 0.13)
                    } else {
                        shade([0.45, 0.32, 0.21], noise * 0.12)
                    }
                }
                slot::SAND => shade([0.80, 0.74, 0.52], noise * 0.08),
                slot::WATER => {
                    let mut texel = shade([0.16, 0.35, 0.62], noise * 0.06);
                    texel[3] = 160; // see-through, so the bed shows below
                    texel
                }
                slot::BEDROCK => shade([0.18, 0.18, 0.20], noise * 0.22),
                slot::LAMP => {
                    // A bright core inside a darker frame, so a lamp reads as
                    // a light source even in a frame where everything around
                    // it is already lit by it.
                    let edge = x == 0 || y == 0 || x == TILE_SIZE - 1 || y == TILE_SIZE - 1;
                    if edge {
                        shade([0.42, 0.34, 0.16], noise * 0.10)
                    } else {
                        shade([0.98, 0.90, 0.62], noise * 0.06)
                    }
                }
                // Unknown slots get magenta, the universal "missing texture".
                _ => [255, 0, 255, 255],
            };
            pixels.extend_from_slice(&texel);
        }
    }
    pixels
}

/// The block texture array and its sampler, ready to bind.
pub struct TileTextures {
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
    pub layer_count: u32,
}

impl TileTextures {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let layer_count = slot::COUNT;

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("block tiles"),
            size: wgpu::Extent3d {
                width: TILE_SIZE,
                height: TILE_SIZE,
                depth_or_array_layers: layer_count,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // sRGB: the shader's lighting maths wants linear values, and the
            // hardware converts on read.
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        for layer in 0..layer_count {
            let pixels = generate_tile(layer);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(TILE_SIZE * 4),
                    rows_per_image: Some(TILE_SIZE),
                },
                wgpu::Extent3d {
                    width: TILE_SIZE,
                    height: TILE_SIZE,
                    depth_or_array_layers: 1,
                },
            );
        }

        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("block tiles view"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("block tiles sampler"),
            // Repeat is what makes merged-quad UVs tile correctly.
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            // Nearest keeps the blocky look and avoids blurring at edges.
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("tiles layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2Array,
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
            label: Some("tiles bind group"),
            layout: &bind_group_layout,
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

        TileTextures {
            view,
            sampler,
            bind_group_layout,
            bind_group,
            layer_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tile_generates_a_full_rgba_image() {
        for tile in 0..slot::COUNT {
            let pixels = generate_tile(tile);
            assert_eq!(pixels.len(), (TILE_SIZE * TILE_SIZE * 4) as usize);
        }
    }

    #[test]
    fn tile_generation_is_deterministic() {
        // Terrain and textures must look the same on every machine and run.
        assert_eq!(generate_tile(slot::STONE), generate_tile(slot::STONE));
    }

    #[test]
    fn different_tiles_look_different() {
        assert_ne!(generate_tile(slot::STONE), generate_tile(slot::DIRT));
        assert_ne!(generate_tile(slot::GRASS_TOP), generate_tile(slot::GRASS_SIDE));
    }

    #[test]
    fn tiles_are_textured_rather_than_flat_colour() {
        // A constant tile would mean the jitter is broken.
        let pixels = generate_tile(slot::STONE);
        let first = &pixels[0..3];
        assert!(
            pixels.chunks_exact(4).any(|texel| texel[0..3] != *first),
            "stone tile is a flat colour"
        );
    }

    #[test]
    fn only_water_is_transparent() {
        for tile in 0..slot::COUNT {
            let pixels = generate_tile(tile);
            let alphas: Vec<u8> = pixels.chunks_exact(4).map(|texel| texel[3]).collect();
            if tile == slot::WATER {
                assert!(alphas.iter().all(|&a| a < 255), "water should be see-through");
            } else {
                assert!(alphas.iter().all(|&a| a == 255), "tile {tile} should be opaque");
            }
        }
    }

    #[test]
    fn grass_side_has_grass_above_dirt() {
        // The top row should be greener than the bottom row, or the block
        // reads upside down in game.
        let pixels = generate_tile(slot::GRASS_SIDE);
        let row = |y: u32| -> (u32, u32) {
            let mut green = 0;
            let mut red = 0;
            for x in 0..TILE_SIZE {
                let at = ((y * TILE_SIZE + x) * 4) as usize;
                red += pixels[at] as u32;
                green += pixels[at + 1] as u32;
            }
            (red, green)
        };

        let (top_red, top_green) = row(0);
        let (bottom_red, bottom_green) = row(TILE_SIZE - 1);
        assert!(top_green > top_red, "top of grass_side is not green");
        assert!(bottom_red > bottom_green, "bottom of grass_side is not dirt");
    }
}
