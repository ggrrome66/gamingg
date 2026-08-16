# gamingg

A modular voxel engine in Rust, Linux-first, built for mods distributed through
Steam Workshop.

## Provenance

This is **original code**. It contains no Minecraft source, no decompiled game
code, and no Mojang assets, and it is not a fork of `mcp940` or any other
decompiled-source repository — redistributing decompiled game code violates
both the MCP license and the Minecraft EULA.

The 1.12-era conventions this borrows are design *reference points* only
(16×16 chunk columns, 256-block world height, ~1m cubic voxels). All textures
and content are original; placeholder art is generated procedurally until real
art exists.

## Status

Milestone 1 is complete: the world generates, meshes, streams and renders, and
there is a binary you can fly around in.

| Crate | What it does | State |
|---|---|---|
| `vx-core` | Block registry, coordinate spaces, event bus | Done |
| `vx-world` | Paletted chunk storage, terrain generation, world state | Done |
| `vx-mesh` | Greedy meshing | Done |
| `vx-render` | wgpu renderer, camera, tile textures, offscreen capture | Done |
| `vx-platform` | Input state, XDG paths | Done |
| `vx-app` | Window, fly controls, chunk streaming, `gamingg` binary | Done |
| `vx-mod-api` / `vx-mod` | Mod ABI, manifests, WASM host | M3 |
| `vx-steam` | Steam Workshop mod source | M4 |

### Known rough edges

- Terrain reads as broad flat terraces. The noise stack clusters near its mean,
  so relief is only ~22 blocks spread over a wide area. Needs shaping work.
- Water is alpha-blended without depth sorting. Fine while water is the only
  translucent block; looks wrong the moment two transparent surfaces overlap.
- Chunks are edited and meshed on the main thread. Meshing is throttled per
  frame to hide it, but it belongs on a worker pool.
- No block placement or breaking yet, and nothing persists to disk.

## Design notes

**The simulation/presentation split is the networking seam.** `vx-world` owns
all authoritative state and never references rendering or windowing. Multiplayer
is not implemented, but adding it later means transporting the existing command
interface over a socket rather than restructuring world logic.

**Chunk storage is palette-compressed.** A chunk is 65 536 blocks; storing a
`BlockId` each would cost 128 KiB. Instead blocks index a per-chunk palette,
packed at the narrowest bit width that fits it. An untouched air chunk collapses
to zero bits and an empty buffer.

**Block ids are not stable across runs.** They are assigned in registration
order, so the mod set changes them. Anything persisted to disk must key on the
namespaced string name (`engine:stone`), never the numeric id.

**All `BlockView` coordinates are absolute world coordinates**, never
chunk-local. Mixing the two makes geometry silently vanish.

**Worldgen is a pure function of `(seed, position)`.** It uses an integer hash
rather than a seeded RNG, so there is no sequence state to desynchronise:
chunks can generate in parallel in any order, and a saved world regenerates
identically.

## Building and running

Requires Rust 1.94+.

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run --release -p vx-app          # opens a window
```

Controls: `WASD` to move, `Space`/`Left Shift` for up and down, `Left Ctrl` to
sprint, click to capture the mouse for looking around, `Escape` to release it.

Running windowed needs a Vulkan loader and drivers plus X11 and/or Wayland
client libraries.

### Rendering without a display

`--screenshot` renders one frame offscreen and exits, so it works over SSH and
in CI:

```sh
cargo run --release -p vx-app -- --screenshot frame.ppm --width 640 --height 360
```

The render tests do the same thing and assert on the pixels. Both run against a
software Vulkan driver, so no GPU is required:

```sh
sudo apt-get install mesa-vulkan-drivers
VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json cargo test --workspace
```

Tests skip themselves rather than failing when no Vulkan adapter exists at all.

## Licence

MIT OR Apache-2.0.
