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
    pub const COPPER_ORE: u32 = 7;
    pub const CONTAINER: u32 = 8;
    pub const HULL: u32 = 9;
    pub const TREAD: u32 = 10;
    pub const STEEL: u32 = 11;
    pub const CAB: u32 = 12;
    pub const PLANK: u32 = 13;
    pub const ROOF: u32 = 14;
    pub const COUNTER: u32 = 15;
    pub const CLOTH: u32 = 16;
    pub const SKIN: u32 = 17;
    pub const LOG_SIDE: u32 = 18;
    pub const LOG_TOP: u32 = 19;
    pub const LEAVES: u32 = 20;
    pub const TUFT: u32 = 21;
    pub const METAL_WALL: u32 = 22;
    pub const RUSTED_METAL: u32 = 23;
    pub const CATWALK: u32 = 24;
    pub const MAST: u32 = 25;
    pub const BEACON: u32 = 26;
    pub const COPPER_BAR: u32 = 27;
    pub const CHEST: u32 = 28;
    pub const MAILBOX: u32 = 29;
    /// Total generated tiles.
    pub const COUNT: u32 = 30;
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
                slot::COPPER_ORE => {
                    // Veins of metal through host rock. The metal has to carry
                    // most of the tile: an outcrop is only useful if a player
                    // can pick it out of a hillside at distance, and a few
                    // flecks on grey read as plain stone from more than a
                    // dozen blocks away.
                    if jitter(tile ^ 0x51, x / 2, y / 2) > -0.12 {
                        shade([0.78, 0.40, 0.16], noise * 0.16)
                    } else {
                        shade([0.44, 0.44, 0.47], noise * 0.10)
                    }
                }
                slot::CONTAINER => {
                    // A steel crate: riveted border around brushed panels, so
                    // it reads as built rather than mined even at distance.
                    let edge = x.min(TILE_SIZE - 1 - x).min(y).min(TILE_SIZE - 1 - y);
                    if edge == 0 {
                        shade([0.30, 0.32, 0.36], noise * 0.05)
                    } else if edge == 1 && (x + y) % 3 == 0 {
                        shade([0.72, 0.74, 0.78], 0.0) // rivets
                    } else {
                        shade([0.52, 0.56, 0.62], noise * 0.08)
                    }
                }
                slot::HULL => {
                    // Rust-orange machine plate with a darker weld seam across
                    // the middle, so big flat rig sides do not read as one
                    // slab of paint.
                    if y == TILE_SIZE / 2 {
                        shade([0.55, 0.24, 0.10], noise * 0.05)
                    } else {
                        shade([0.76, 0.34, 0.13], noise * 0.10)
                    }
                }
                slot::TREAD => {
                    // Near-black rubber, ribbed.
                    if x % 4 < 2 {
                        shade([0.10, 0.10, 0.11], noise * 0.05)
                    } else {
                        shade([0.17, 0.17, 0.19], noise * 0.05)
                    }
                }
                slot::STEEL => {
                    // Bright brushed steel: horizontal grain.
                    shade([0.72, 0.74, 0.78], jitter(tile ^ 0x77, 0, y) * 0.18 + noise * 0.04)
                }
                slot::CAB => {
                    // Pale glassy blue-grey with a highlight streak.
                    if x + y < 8 {
                        shade([0.80, 0.88, 0.94], noise * 0.03)
                    } else {
                        shade([0.55, 0.66, 0.76], noise * 0.05)
                    }
                }
                slot::PLANK => {
                    // Warm boards with a dark seam every fourth row and the
                    // odd nail head, so a wall reads as carpentry.
                    if y % 4 == 3 {
                        shade([0.38, 0.26, 0.14], noise * 0.06)
                    } else if y % 4 == 1 && x % 8 == 2 {
                        shade([0.30, 0.28, 0.26], 0.0) // nails
                    } else {
                        shade([0.62, 0.44, 0.24], noise * 0.10)
                    }
                }
                slot::ROOF => {
                    // Staggered shingle courses in a weathered red-brown.
                    let course = y / 4;
                    let shifted = x + course * 2;
                    if y % 4 == 0 || shifted % 8 == 0 {
                        shade([0.30, 0.13, 0.10], noise * 0.06)
                    } else {
                        shade([0.50, 0.22, 0.15], noise * 0.10)
                    }
                }
                slot::COUNTER => {
                    // Dark shop wood under a brass strip: the one block in
                    // town you trade over, so it has to stand apart.
                    if y < 3 {
                        shade([0.72, 0.58, 0.22], noise * 0.06)
                    } else if y % 5 == 4 {
                        shade([0.24, 0.16, 0.10], noise * 0.05)
                    } else {
                        shade([0.40, 0.28, 0.16], noise * 0.08)
                    }
                }
                slot::CLOTH => {
                    // Woven fabric: a faint checked weave in a muted blue.
                    if (x + y) % 2 == 0 {
                        shade([0.28, 0.38, 0.55], noise * 0.05)
                    } else {
                        shade([0.24, 0.33, 0.49], noise * 0.05)
                    }
                }
                slot::SKIN => shade([0.82, 0.62, 0.46], noise * 0.05),
                slot::LOG_SIDE => {
                    // Bark: vertical ridges, the grain running with the trunk.
                    if jitter(tile ^ 0x2f, x, 0) > 0.1 {
                        shade([0.26, 0.17, 0.09], noise * 0.08)
                    } else {
                        shade([0.38, 0.26, 0.14], noise * 0.10)
                    }
                }
                slot::LOG_TOP => {
                    // End grain: rough growth rings around the centre.
                    let cx = x as f32 - 7.5;
                    let cy = y as f32 - 7.5;
                    let ring = (cx * cx + cy * cy).sqrt() as u32;
                    if ring.is_multiple_of(3) {
                        shade([0.44, 0.31, 0.17], noise * 0.06)
                    } else {
                        shade([0.60, 0.44, 0.24], noise * 0.08)
                    }
                }
                slot::LEAVES => {
                    // Mottled two-tone canopy. Opaque on purpose — the
                    // "fast graphics" look — so foliage never joins water in
                    // the unsorted alpha-blend pass.
                    if jitter(tile ^ 0x63, x / 2, y / 2) > 0.05 {
                        shade([0.16, 0.34, 0.12], noise * 0.10)
                    } else {
                        shade([0.24, 0.46, 0.16], noise * 0.12)
                    }
                }
                slot::TUFT => {
                    // A few grass blades on a transparent field; drawn as a
                    // crossed pair of quads in the world, not a cube.
                    let sway = (jitter(tile ^ 0x11, x, 0) * 3.0) as i32;
                    let blade = x % 5 == 2 && (y as i32) > 4 + sway && jitter(tile ^ 0x19, x, 0) > -0.3;
                    if blade {
                        shade([0.30, 0.52, 0.20], noise * 0.12)
                    } else {
                        [0, 0, 0, 0]
                    }
                }
                slot::METAL_WALL => {
                    // Corrugated container flank: vertical ribbing catching
                    // the light, so a wall reads as pressed steel rather than
                    // a flat panel.
                    let rib = x % 4;
                    let lift = match rib {
                        0 => 0.06,
                        1 => 0.02,
                        2 => -0.04,
                        _ => -0.02,
                    };
                    shade([0.55, 0.58, 0.60], lift + noise * 0.05)
                }
                slot::RUSTED_METAL => {
                    // The same ribbing, weathered. Patches of rust bloom
                    // through where the paint has given up.
                    let rib = x % 4;
                    let lift = if rib < 2 { 0.04 } else { -0.03 };
                    if jitter(tile ^ 0x3d, x / 3, y / 3) > 0.05 {
                        shade([0.55, 0.30, 0.16], lift + noise * 0.10)
                    } else {
                        shade([0.48, 0.47, 0.45], lift + noise * 0.06)
                    }
                }
                slot::CATWALK => {
                    // Open steel grating, drawn opaque: the mesher has no
                    // sorted-transparency pass, so a see-through walkway would
                    // cost far more than it looks.
                    if x % 4 == 0 || y % 4 == 0 {
                        shade([0.38, 0.40, 0.43], noise * 0.05)
                    } else {
                        shade([0.16, 0.17, 0.19], noise * 0.08)
                    }
                }
                slot::MAST => {
                    // Galvanised lattice: diagonal bracing over dark sky-grey,
                    // which reads as a tower from a distance without needing
                    // a single transparent texel.
                    let diagonal = (x + y) % 6 < 2 || (x + TILE_SIZE - y) % 6 < 2;
                    if diagonal {
                        shade([0.62, 0.65, 0.68], noise * 0.06)
                    } else {
                        shade([0.22, 0.24, 0.27], noise * 0.05)
                    }
                }
                slot::BEACON => {
                    // A console face: dark housing, an amber strip that reads
                    // as powered even at night.
                    if (5..9).contains(&y) && (2..14).contains(&x) {
                        shade([0.95, 0.62, 0.16], noise * 0.06)
                    } else if y < 3 {
                        shade([0.30, 0.32, 0.36], noise * 0.05)
                    } else {
                        shade([0.18, 0.19, 0.22], noise * 0.06)
                    }
                }
                slot::COPPER_BAR => {
                    // Stacked ingots: bright bar faces with dark seams between
                    // them, so a pallet of them reads as many rather than one.
                    let seam = y % 5 == 0 || (x + (y / 5) * 3) % 8 == 0;
                    if seam {
                        shade([0.28, 0.16, 0.10], noise * 0.05)
                    } else {
                        // Lighter along the top of each bar, for a rounded look.
                        let lift = if y % 5 == 1 { 0.10 } else { 0.0 };
                        shade(
                            [0.85 + lift, 0.48 + lift * 0.6, 0.22 + lift * 0.3],
                            noise * 0.07,
                        )
                    }
                }
                slot::CHEST => {
                    // A strongbox: plank body, a darker iron strap across the
                    // middle with a clasp — the strap is what says "chest"
                    // rather than "crate" at a glance.
                    let strap = (7..10).contains(&y);
                    let clasp = strap && (6..10).contains(&x);
                    if clasp {
                        shade([0.72, 0.70, 0.62], noise * 0.05)
                    } else if strap || !(1..TILE_SIZE - 1).contains(&x) {
                        shade([0.24, 0.20, 0.16], noise * 0.06)
                    } else {
                        shade([0.55, 0.40, 0.24], noise * 0.10)
                    }
                }
                slot::MAILBOX => {
                    // A postal panel: pale housing with one dark letter slot
                    // near the top. One strong horizontal is all it takes.
                    if (3..6).contains(&y) && (3..13).contains(&x) {
                        shade([0.10, 0.11, 0.13], noise * 0.03)
                    } else if y >= TILE_SIZE - 2 {
                        shade([0.35, 0.37, 0.40], noise * 0.05)
                    } else {
                        shade([0.70, 0.72, 0.75], noise * 0.06)
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
    fn only_water_and_tufts_are_transparent() {
        for tile in 0..slot::COUNT {
            let pixels = generate_tile(tile);
            let alphas: Vec<u8> = pixels.chunks_exact(4).map(|texel| texel[3]).collect();
            match tile {
                slot::WATER => {
                    assert!(alphas.iter().all(|&a| a < 255), "water should be see-through")
                }
                slot::TUFT => {
                    // Blades on empty air: some texels fully clear, some
                    // fully solid, nothing half-and-half to blend badly.
                    assert!(alphas.contains(&0), "tuft has no clear texels");
                    assert!(alphas.contains(&255), "tuft has no blades");
                    assert!(alphas.iter().all(|&a| a == 0 || a == 255));
                }
                _ => assert!(alphas.iter().all(|&a| a == 255), "tile {tile} should be opaque"),
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
