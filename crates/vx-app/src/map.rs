//! The minimap: where you have been, and where everything is right now.
//!
//! # The explored set is the only state
//!
//! Worldgen is a pure function of `(seed, position)`, so the surface of an
//! explored-but-unloaded chunk can be recomputed from the generator's height
//! field on demand. The map therefore stores no terrain thumbnails at all —
//! just the set of chunks you (or the flier) have seen. Accepted consequence:
//! edits and mine holes show only while their chunks are loaded; distant
//! explored land draws as generated.
//!
//! # Player knowledge, not world truth
//!
//! The explored set persists in its own small file beside the region files
//! rather than inside them, because it belongs to the player rather than the
//! world. Losing it is cosmetic, so a damaged file logs and starts empty —
//! it must never take the world down with it.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::Path;

use vx_core::{BlockPos, ChunkPos, CHUNK_SIZE};
use vx_world::{World, SEA_LEVEL};

/// Side length of the map image, in pixels.
pub const MAP_SIZE: u32 = 192;

/// Redraw the picture at most this often, in frames. The terrain sampling is
/// the whole cost of the map, and 6 Hz is indistinguishable from live for
/// something that scrolls at walking speed.
pub const REDRAW_INTERVAL: u32 = 10;

const MAGIC: &[u8; 4] = b"VXMP";
const VERSION: u32 = 1;

/// A dot on the map, in world column coordinates.
#[derive(Debug, Clone, Copy)]
pub struct Marker {
    pub x: i32,
    pub z: i32,
    pub colour: [u8; 4],
    /// Half-width in pixels, so important dots survive zooming out.
    pub radius: i32,
}

/// Marker colours, kept together so the legend stays consistent.
pub mod colour {
    pub const PLAYER: [u8; 4] = [255, 255, 255, 255];
    pub const DRONE: [u8; 4] = [25, 25, 30, 255];
    pub const FLIER: [u8; 4] = [120, 200, 255, 255];
    pub const PING: [u8; 4] = [255, 140, 40, 255];
    pub const BASE: [u8; 4] = [80, 255, 170, 255];
}

/// The map's persistent and per-session state.
#[derive(Debug, Default)]
pub struct MapState {
    /// Chunks the player has been near or the flier has swept.
    explored: HashSet<ChunkPos>,
    pub visible: bool,
    /// Blocks per map pixel: 1, 2 or 4.
    pub zoom: i32,
    /// Frames until the next redraw is allowed.
    cooldown: u32,
}

impl MapState {
    pub fn new() -> Self {
        MapState {
            explored: HashSet::new(),
            visible: true,
            zoom: 2,
            cooldown: 0,
        }
    }

    /// Mark every chunk within `radius` of `centre` as explored.
    pub fn explore_around(&mut self, centre: ChunkPos, radius: i32) {
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                self.explored.insert(ChunkPos::new(centre.x + dx, centre.z + dz));
            }
        }
    }

    pub fn explore(&mut self, chunk: ChunkPos) {
        self.explored.insert(chunk);
    }

    pub fn is_explored(&self, chunk: ChunkPos) -> bool {
        self.explored.contains(&chunk)
    }

    pub fn explored_count(&self) -> usize {
        self.explored.len()
    }

    /// Step the zoom through 1, 2, 4 blocks per pixel.
    pub fn zoom_in(&mut self) {
        self.zoom = (self.zoom / 2).max(1);
    }

    pub fn zoom_out(&mut self) {
        self.zoom = (self.zoom * 2).min(4);
    }

    /// Count down the redraw throttle; true when a redraw is due.
    pub fn should_redraw(&mut self) -> bool {
        if self.cooldown == 0 {
            // Minus one so "every REDRAW_INTERVAL frames" means what it says:
            // one fire per interval of calls, not per interval-plus-one.
            self.cooldown = REDRAW_INTERVAL - 1;
            true
        } else {
            self.cooldown -= 1;
            false
        }
    }

    /// Force the next `should_redraw` to fire — after a zoom change, say.
    pub fn invalidate(&mut self) {
        self.cooldown = 0;
    }

    /// Write the explored set beside the world save.
    pub fn save(&self, directory: &Path) -> std::io::Result<()> {
        // Written atomically enough for a cosmetic file: a torn write is
        // caught by the tolerant loader and simply forgotten.
        let mut file = std::io::BufWriter::new(std::fs::File::create(directory.join("explored.dat"))?);
        file.write_all(MAGIC)?;
        file.write_all(&VERSION.to_le_bytes())?;
        file.write_all(&(self.explored.len() as u64).to_le_bytes())?;
        // Sorted, so the file is deterministic for a given set.
        let mut chunks: Vec<ChunkPos> = self.explored.iter().copied().collect();
        chunks.sort_by_key(|chunk| (chunk.x, chunk.z));
        for chunk in chunks {
            file.write_all(&chunk.x.to_le_bytes())?;
            file.write_all(&chunk.z.to_le_bytes())?;
        }
        file.flush()
    }

    /// Load the explored set, tolerating absence and damage.
    ///
    /// Anything wrong with the file — missing, truncated, wrong magic — logs
    /// and yields an empty set. The map is player knowledge; a corrupted
    /// memory is forgotten ground, not a failed world.
    pub fn load(&mut self, directory: &Path) {
        let path = directory.join("explored.dat");
        match read_explored(&path) {
            Ok(Some(chunks)) => {
                log::info!("loaded {} explored chunks", chunks.len());
                self.explored = chunks;
            }
            Ok(None) => {}
            Err(error) => {
                log::warn!("could not read {}: {error}; starting unexplored", path.display());
                self.explored.clear();
            }
        }
    }
}

fn read_explored(path: &Path) -> std::io::Result<Option<HashSet<ChunkPos>>> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => std::io::BufReader::new(file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };

    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(std::io::Error::other("bad magic"));
    }
    let mut word = [0u8; 4];
    file.read_exact(&mut word)?;
    if u32::from_le_bytes(word) != VERSION {
        return Err(std::io::Error::other("unknown version"));
    }
    let mut count_bytes = [0u8; 8];
    file.read_exact(&mut count_bytes)?;
    let count = u64::from_le_bytes(count_bytes);

    let mut chunks = HashSet::new();
    for _ in 0..count {
        let mut x = [0u8; 4];
        let mut z = [0u8; 4];
        file.read_exact(&mut x)?;
        file.read_exact(&mut z)?;
        chunks.insert(ChunkPos::new(i32::from_le_bytes(x), i32::from_le_bytes(z)));
    }
    Ok(Some(chunks))
}

/// The colour class of one column, before height shading.
fn column_colour(world: &World, state: &MapState, x: i32, z: i32) -> Option<([f32; 3], i32)> {
    let chunk = BlockPos::new(x, 0, z).chunk();

    if world.is_loaded(chunk) {
        let clear = world.surface_y(x, z)?;
        let top = clear - 1;
        let name = world
            .registry()
            .get(world.block(BlockPos::new(x, top, z)))
            .map(|def| def.name.as_str())
            .unwrap_or("");
        let colour = if name.ends_with("_ore") {
            [0.90, 0.48, 0.18]
        } else {
            match name {
                "engine:water" => [0.24, 0.36, 0.65],
                "engine:grass" => [0.28, 0.52, 0.24],
                "engine:sand" => [0.78, 0.72, 0.51],
                "engine:dirt" => [0.44, 0.32, 0.22],
                "engine:container" => [0.63, 0.67, 0.73],
                _ => [0.51, 0.51, 0.53],
            }
        };
        return Some((colour, top));
    }

    if state.is_explored(chunk) {
        // Unloaded but seen: recompute the generated surface. Edits there are
        // not shown — the accepted trade for storing nothing.
        let height = world.generator().height_at(x, z);
        let colour = if height < SEA_LEVEL {
            [0.24, 0.36, 0.65]
        } else if height < SEA_LEVEL + 2 {
            [0.78, 0.72, 0.51]
        } else {
            [0.28, 0.52, 0.24]
        };
        return Some((colour, height.max(SEA_LEVEL)));
    }

    None
}

/// Render the map image: `MAP_SIZE` square, centred on `centre`, one column
/// sampled per pixel at `zoom` blocks per pixel. Markers are stamped on top.
pub fn render_map(
    world: &World,
    state: &MapState,
    centre: (i32, i32),
    markers: &[Marker],
) -> Vec<u8> {
    let size = MAP_SIZE as i32;
    let zoom = state.zoom.max(1);
    let mut pixels = vec![0u8; (MAP_SIZE * MAP_SIZE * 4) as usize];

    for py in 0..size {
        for px in 0..size {
            let x = centre.0 + (px - size / 2) * zoom;
            let z = centre.1 + (py - size / 2) * zoom;
            let at = ((py * size + px) * 4) as usize;

            match column_colour(world, state, x, z) {
                Some((base, height)) => {
                    // Height shading: high ground bright, valleys dark, so
                    // relief reads at a glance.
                    let shade = 0.55 + 0.45 * ((height - 40).clamp(0, 100) as f32 / 100.0);
                    pixels[at] = (base[0] * shade * 255.0) as u8;
                    pixels[at + 1] = (base[1] * shade * 255.0) as u8;
                    pixels[at + 2] = (base[2] * shade * 255.0) as u8;
                    pixels[at + 3] = 235;
                }
                None => {
                    // Unexplored: a dark pane, translucent enough to hint at
                    // the world behind the map.
                    pixels[at] = 10;
                    pixels[at + 1] = 12;
                    pixels[at + 2] = 16;
                    pixels[at + 3] = 170;
                }
            }
        }
    }

    for marker in markers {
        let px = (marker.x - centre.0) / zoom + size / 2;
        let py = (marker.z - centre.1) / zoom + size / 2;
        for dy in -marker.radius..=marker.radius {
            for dx in -marker.radius..=marker.radius {
                let (sx, sy) = (px + dx, py + dy);
                if (0..size).contains(&sx) && (0..size).contains(&sy) {
                    let at = ((sy * size + sx) * 4) as usize;
                    pixels[at..at + 4].copy_from_slice(&marker.colour);
                }
            }
        }
    }

    pixels
}

/// Every chunk covered by a scan sector, for marking swept ground explored.
pub fn sector_chunks(sector: vx_agent::Sector) -> impl Iterator<Item = ChunkPos> {
    let (min_x, min_z) = sector.min_column();
    let chunks = vx_agent::SECTOR_SIZE / CHUNK_SIZE;
    let base = ChunkPos::new(min_x.div_euclid(CHUNK_SIZE), min_z.div_euclid(CHUNK_SIZE));
    (0..chunks).flat_map(move |dx| (0..chunks).map(move |dz| ChunkPos::new(base.x + dx, base.z + dz)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::ChunkPos;

    fn world_with_origin() -> World {
        let mut world = World::new(2024);
        world.load_around(ChunkPos::new(0, 0), 2);
        world
    }

    #[test]
    fn exploring_grows_and_never_shrinks() {
        let mut state = MapState::new();
        state.explore_around(ChunkPos::new(0, 0), 2);
        let first = state.explored_count();
        assert_eq!(first, 25);

        state.explore_around(ChunkPos::new(1, 1), 2);
        assert!(state.explored_count() >= first, "exploring lost ground");
        // Re-exploring the same ground adds nothing.
        let again = state.explored_count();
        state.explore_around(ChunkPos::new(0, 0), 2);
        assert_eq!(state.explored_count(), again);
    }

    #[test]
    fn the_explored_set_round_trips_through_disk() {
        let directory = std::env::temp_dir().join(format!("vx-map-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();

        let mut state = MapState::new();
        state.explore(ChunkPos::new(3, -7));
        state.explore(ChunkPos::new(-100, 42));
        state.save(&directory).unwrap();

        let mut loaded = MapState::new();
        loaded.load(&directory);
        assert_eq!(loaded.explored_count(), 2);
        assert!(loaded.is_explored(ChunkPos::new(3, -7)));
        assert!(loaded.is_explored(ChunkPos::new(-100, 42)));

        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn a_corrupt_explored_file_yields_empty_not_failure() {
        let directory = std::env::temp_dir().join(format!("vx-map-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("explored.dat"), b"garbage").unwrap();

        let mut state = MapState::new();
        state.explore(ChunkPos::new(1, 1));
        state.load(&directory);
        assert_eq!(state.explored_count(), 0, "corrupt file should reset, not crash");

        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn loaded_terrain_colours_match_their_surface_blocks() {
        // The map must tell the truth about ground it can see directly.
        let world = world_with_origin();
        let mut state = MapState::new();
        state.explore_around(ChunkPos::new(0, 0), 2);

        let pixels = render_map(&world, &state, (0, 0), &[]);
        let size = MAP_SIZE as i32;
        let zoom = state.zoom;

        let mut checked = 0;
        for (px, py) in [(96, 96), (60, 96), (96, 60), (120, 120)] {
            let x = (px - size / 2) * zoom;
            let z = (py - size / 2) * zoom;
            let Some(clear) = world.surface_y(x, z) else { continue };
            let name = world
                .registry()
                .get(world.block(vx_core::BlockPos::new(x, clear - 1, z)))
                .map(|def| def.name.clone())
                .unwrap_or_default();
            let at = ((py * size + px) * 4) as usize;
            let (r, g, b) = (pixels[at], pixels[at + 1], pixels[at + 2]);

            match name.as_str() {
                "engine:water" => assert!(b > r && b > g, "water pixel is not blue: {r},{g},{b}"),
                "engine:grass" => assert!(g > r && g > b, "grass pixel is not green: {r},{g},{b}"),
                "engine:sand" => assert!(r > b && g > b, "sand pixel is not sandy: {r},{g},{b}"),
                _ => continue,
            }
            checked += 1;
        }
        assert!(checked > 0, "no classifiable columns landed under the probes");
    }

    #[test]
    fn unexplored_ground_is_dark_and_explored_ground_is_not() {
        let world = world_with_origin();
        let mut state = MapState::new();
        state.explore(ChunkPos::new(0, 0)); // only the origin chunk

        let pixels = render_map(&world, &state, (8, 8), &[]);
        let size = MAP_SIZE as i32;
        let zoom = state.zoom;

        // Centre of the map is over the explored origin chunk.
        let centre_at = ((size / 2 * size + size / 2) * 4) as usize;
        assert!(pixels[centre_at + 3] > 200, "explored ground is see-through");

        // A far corner maps to columns hundreds of blocks away: unexplored.
        let corner_x = 8 + (0 - size / 2) * zoom;
        assert!(!state.is_explored(vx_core::BlockPos::new(corner_x, 0, corner_x).chunk()));
        let corner_at = 0;
        assert!(
            pixels[corner_at] < 30 && pixels[corner_at + 1] < 30,
            "unexplored ground is not dark"
        );
    }

    #[test]
    fn a_marker_lands_on_its_column_and_moves_with_it() {
        let world = world_with_origin();
        let mut state = MapState::new();
        state.explore_around(ChunkPos::new(0, 0), 3);

        let size = MAP_SIZE as i32;
        let zoom = state.zoom;
        let dot = |pixels: &[u8], px: i32, py: i32| -> [u8; 4] {
            let at = ((py * size + px) * 4) as usize;
            [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
        };

        let marker = |x: i32, z: i32| Marker {
            x,
            z,
            colour: colour::DRONE,
            radius: 1,
        };

        let here = render_map(&world, &state, (0, 0), &[marker(8, 8)]);
        assert_eq!(dot(&here, size / 2 + 8 / zoom, size / 2 + 8 / zoom), colour::DRONE);

        // Move the drone: the dot moves, the old spot does not keep it.
        let moved = render_map(&world, &state, (0, 0), &[marker(16, 8)]);
        assert_eq!(dot(&moved, size / 2 + 16 / zoom, size / 2 + 8 / zoom), colour::DRONE);
        assert_ne!(dot(&moved, size / 2 + 8 / zoom, size / 2 + 8 / zoom), colour::DRONE);
    }

    #[test]
    fn the_redraw_throttle_fires_once_per_interval() {
        let mut state = MapState::new();
        assert!(state.should_redraw(), "the first frame should draw");
        let mut fired = 0;
        for _ in 0..REDRAW_INTERVAL {
            if state.should_redraw() {
                fired += 1;
            }
        }
        assert_eq!(fired, 1, "the throttle fired {fired} times in one interval");
    }
}
