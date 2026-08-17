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

Milestone 2 is complete: the world generates, meshes, streams and renders, you
can walk on it, build in it, and it survives quitting.

| Crate | What it does | State |
|---|---|---|
| `vx-core` | Block registry, coordinate spaces, event bus | Done |
| `vx-world` | Chunk storage, worldgen, raycast, physics, editing, saves | Done |
| `vx-mesh` | Greedy meshing | Done |
| `vx-render` | wgpu renderer, camera, tile textures, offscreen capture | Done |
| `vx-platform` | Input state, XDG paths | Done |
| `vx-app` | Window, walk/fly controls, streaming, `gamingg` binary | Done |
| `vx-mod-api` / `vx-mod` | Mod ABI, manifests, WASM host | M3 |
| `vx-steam` | Steam Workshop mod source | M4 |

### Known rough edges

- Terrain reads as broad flat terraces. The noise stack clusters near its mean,
  so relief is only ~22 blocks spread over a wide area. Needs shaping work.
- Water is alpha-blended without depth sorting. Fine while water is the only
  translucent block; looks wrong the moment two transparent surfaces overlap.
- Chunks are meshed on the main thread. Meshing is throttled per frame to hide
  it, but it belongs on a worker pool — `rayon` is already a dependency.
- **No frustum culling.** Every loaded chunk is drawn every frame, even the
  two-thirds behind the camera. The cheapest large performance win available.
- Saving happens on quit and on demand, not periodically. A crash loses the
  session.
- No inventory, no UI, no audio, and no gamepad support.

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
identically. This is also why saves only store chunks a player has *changed* —
everything else comes back from the seed for free.

**Saves key blocks by namespaced name, never by numeric id.** Ids are assigned
in registration order, so installing one mod shifts all of them; a save keyed on
numbers would silently reload as the wrong blocks entirely.

**Collision resolves one axis at a time.** Resolving all three together and
pushing out along the shallowest overlap is the usual shortcut, and it is what
makes sliding along a wall snag on every block seam.

**Block edits are announced on the event bus before they happen**, through
`emit_cancellable`. Nothing listens yet — the point is that M3 mods will be able
to veto or alter an edit without any call site changing.

## Building and running

Requires Rust 1.94+.

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run --release -p vx-app          # opens a window
```

Controls:

| Input | Action |
|---|---|
| `WASD` | Move |
| `Space` | Jump (walk) / rise (fly) |
| `Left Shift` | Descend (fly) |
| `Left Ctrl` | Sprint |
| Click | Capture the mouse; once captured, break a block |
| Right click | Place the selected block |
| `1`–`4` | Choose stone, dirt, grass or sand |
| `F` | Toggle walking and flying |
| `F5` | Save |
| `Escape` | Release the mouse |

Worlds are saved on quit to `$XDG_DATA_HOME/gamingg/saves/world`, selectable
with `--world <name>`. Reloading an existing world uses its stored seed, so
`--seed` only applies when creating a new one.

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
