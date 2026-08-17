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

M3 is under way. The world has real relief, meshes across all cores, draws only
what the camera can see, and hides copper ore in the rock with occasional
outcrops breaking the surface. Mark a body and the game works out how to mine
it — a level adit into a hillside, a diagonal decline, or a benched open pit —
and a drone cuts the excavation, drives in, digs the ore out and hauls it home.
You can walk on it, build in it, and it survives quitting.

| Crate | What it does | State |
|---|---|---|
| `vx-core` | Block registry, coordinate spaces, event bus | Done |
| `vx-world` | Chunk storage, worldgen, ore, raycast, physics, editing, saves | Done |
| `vx-mesh` | Greedy meshing | Done |
| `vx-render` | wgpu renderer, camera, frustum culling, instanced objects, offscreen capture | Done |
| `vx-platform` | Input state, XDG paths | Done |
| `vx-app` | Window, walk/fly controls, streaming, `gamingg` binary | Done |
| `vx-agent` | Job board, flow fields, mine planning, one drone | Done |
| `vx-mod-api` / `vx-mod` | Mod ABI, manifests, WASM host | later |
| `vx-steam` | Steam Workshop mod source | M4 |

### Known rough edges

- Saves store a full chunk snapshot per modified chunk — about 24 KiB whether
  one block changed or ten thousand. An edit journal would cost bytes instead.
  Harmless until worlds get large.
- Water is alpha-blended without depth sorting. Fine while water is the only
  translucent block; looks wrong the moment two transparent surfaces overlap.
- Saving happens on quit and on demand, not periodically. A crash loses the
  session.
- Chunk culling uses each chunk's full 256-block height. A tighter bound around
  the blocks actually present would cull more.
- Ore uses one tile for every block, so a large exposed body shows a visibly
  repeating pattern. Real art or per-block texture variation fixes it.
- Drones are drawn as plain cubes, and the mining readout goes to the log rather
  than the screen. Text rendering is a later milestone.
- A drone's grade shapes the ramps it cuts but is not yet a limit on what it can
  drive: the flow field allows one block of climb per step, so a 1:1 staircase is
  traversable by anything. Making grade a real constraint needs a field that
  tracks how far a drone has run since it last climbed.
- Only one drone. The job board is built for a swarm and nothing exercises it
  yet.
- A running excavation is not saved. The hole is — block edits go through the
  same path a player's do — but quitting mid-dig loses the drone and the job
  board, and the marked plan has to be set up again.
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

**Terrain height comes from three fields through splines, not summed octaves.**
Summing octaves drives the result toward its mean, so everything lands mid-range
and the world reads as flat terraces — the old version had ~22 blocks of relief.
Continentalness, erosion and peaks/valleys are sampled independently and each
mapped through a piecewise-linear curve, which can spend a wide output range on
a narrow input band. A cliff is a steep segment; a plain is a flat one, flat
because it was authored that way. Domain warping bends the sample space so
nothing lines up with the lattice. Relief is now ~100 blocks.

**Outcrops are a consequence, not a feature.** Ore deposits are irregular blobs
buried at depth. Most never reach the top of the rock. A few sit high enough that
the blob pushes up through the soil, and that is an outcrop — nothing generates
them directly. Deposit centres come from a jittered lattice, so they are a pure
function of the seed and the ones near a chunk can be gathered without a global
list. Ore only ever replaces stone or overburden, which is what stops it hanging
in mid-air.

**For a machine that drives, the mining method is the access problem.** A ground
drone can change height by one block per step, so how an excavation is shaped
decides whether the ore can be reached and hauled out at all — the same question
real operations answer when they choose between an adit, a decline and a pit. An
adit is a level tunnel into a hillside and only works where the body meets a
slope, which is exactly what the outcrop rule produces; a decline is the
general-purpose ramp; a pit's benches are its own haul road but its volume grows
with the cube of depth. A vertical shaft is deliberately absent: nothing that
drives can climb out of one, so it belongs to the flying drone as a later unlock.

**Methods are ranked on cost, not on dug blocks.** Ranking on volume alone would
never choose an adit — a decline can always start part-way down a slope and cut a
shorter tunnel. But the excavation is dug once and the haul is made on every load
forever, so a plan is charged for the height its loaded drone has to climb. On
the reference body that puts the adit ahead at 2160 against the decline's 2355,
even though the decline moves 66 fewer blocks of rock. The player can override.

**A body is mined in benches, not as a box.** Clearing a box layer by layer
leaves vertical walls, and a drone that has worked to the floor of one cannot
climb four blocks back out — it ends up standing on its own ore with a full load
and nowhere to take it. Each layer of the stope reaches one block further toward
the access than the layer beneath, which leaves a staircase down one wall. The
bottom bench is the body's true footprint, so all the ore still comes out; the
extra is waste cut to make the ramp.

**A drone cuts level with itself and above, never diagonally below.** Cutting
down-and-across looks like the obvious way to descend and is what strands it: it
carves a full-height notch and destroys the floor of every cell in it, including
the ones needed to advance. Descending is by undermining — cutting the block
directly underfoot, allowed only when there is solid ground under *that*, so the
drop is one block and the step back up exists. Every hole it makes for itself, it
can climb out of.

**Tunnels are two blocks tall for the same reason.** Two is the tallest a drone
can cut standing on its own floor. A three-block corridor needs somebody a block
off the ground to cut the roof, and on a level tunnel there is nowhere to stand.

**Every visible outcrop has ore continuing beneath it.** A surface block only
shows ore when the body also fills the blocks below it. Without that rule a body
could graze the surface and leave a single speck leading nowhere, players would
learn that outcrops mean nothing, and the prospecting loop would die with them.
A test pins it.

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

**Meshing runs on a worker pool, reading the world immutably.** `World` is plain
data with no interior mutability, so parallel reads need no locking — the shared
borrow the mesher was already written against was all that was required. A test
asserts the parallel result is byte-identical to the serial one.

**Objects are instanced from the start and share the terrain's fragment shader.**
The design calls for swarms, so there is one cube mesh, one instance buffer and
one draw call however many drones exist; per-object draws would be rewritten at
the first swarm. They share the terrain pass's depth buffer so drones and hills
occlude each other, and the *same* fragment entry point, so a drone can never
drift out of step with the light on the ground it is standing on.

**Frustum culling is derived from the same view-projection matrix the GPU uses**,
so it can never disagree with what actually gets clipped. A chunk's bounding box
comes from its `ChunkPos`, which relies on the mesher never emitting geometry
outside the chunk it was asked for. The test that matters renders a frame with
culling on and off and asserts the pixels are identical: skipping something
visible would show up immediately.

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
| `M` | Mark a corner of an ore body (two marks make an area) |
| `Tab` | Cycle the proposed mining method |
| `Enter` | Send a drone to dig it |
| `Backspace` | Cancel the marked plan |
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

# look at a specific place, which is how ore outcrops get eyeballed
cargo run --release -p vx-app -- --screenshot ore.ppm --at 146,30

# find the nearest outcrop, mine it out, and photograph the workings
cargo run --release -p vx-app -- --screenshot mine.ppm --at 146,30 --dig auto
cargo run --release -p vx-app -- --screenshot pit.ppm --at 146,30 --dig pit
```

`--dig` runs a whole excavation headlessly against generated terrain, so it is
both a screenshot tool and the fastest way to see whether a change to the
planners still produces a mine a drone can drive.

The render tests do the same thing and assert on the pixels. Both run against a
software Vulkan driver, so no GPU is required:

```sh
sudo apt-get install mesa-vulkan-drivers
VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json cargo test --workspace
```

Tests skip themselves rather than failing when no Vulkan adapter exists at all.

## Licence

MIT OR Apache-2.0.
