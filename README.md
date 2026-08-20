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
there is a binary you can walk, jump, mine, craft and build in, with flight
one key away. Mining runs through a drill you upgrade for the whole game, and
everything else runs through the deck — a handheld that is the game's UI.

| Crate | What it does | State |
|---|---|---|
| `vx-core` | Block registry, coordinate spaces, event bus | Done |
| `vx-world` | Chunk storage, worldgen, state, tick, lighting | Done |
| `vx-mesh` | Greedy meshing | Done |
| `vx-render` | wgpu renderer, camera, tiles, UI overlay, capture | Done |
| `vx-platform` | Input state, XDG paths | Done |
| `vx-save` | Region files, chunk format, world metadata | Done |
| `vx-app` | Window, fly controls, streaming, building, HUD, `gamingg` | Done |
| `vx-mod-api` / `vx-mod` | Mod ABI, manifests, WASM host | Later |
| `vx-steam` | Steam Workshop mod source | Later |

The near-term focus is the engine core — simulation tick, lighting, world
richness, player physics, items. Third-party mod loading comes after that
foundation is solid rather than being designed around up front.

### Known rough edges

- Terrain has relief but little character: no biomes, so the whole world is
  one palette of grass, dirt and sand, and there are no trees or structures.
- Caves are noise-carved rather than tunnelled, so they wander without ever
  leading anywhere. Nothing generates an entrance, either — you find one by
  digging or by luck.
- Water is alpha-blended without depth sorting. Fine while water is the only
  translucent block; looks wrong the moment two transparent surfaces overlap.
- Chunks are edited and meshed on the main thread. Meshing is throttled per
  frame to hide it, but it belongs on a worker pool.
- Saved chunks are stored uncompressed. A modified chunk costs about 8 KiB and
  a region file has an 8 KiB header floor, which is fine at this scale but is
  the obvious thing to compress later.
- Scheduled ticks are not persisted. Nothing schedules work far enough ahead
  for it to matter yet — a falling block resolves within a few steps — but
  anything with a long-running internal state will need the chunk format to
  carry its pending ticks, which is a version bump.
- Lighting is flat per face, not smoothed per vertex. It suits the blocky look,
  but there is no ambient occlusion softening the corners.
- There is no day/night cycle yet, so sky light is always full strength. The
  channel is kept separate ready for one.
- A block edit relights its whole chunk on the next tick rather than doing an
  incremental update around the change. Correct, and far more work than needed.
- Falling blocks are instant per step rather than animated, and they do not
  fall into unloaded chunks, so a column at the edge of the loaded world waits
  until its neighbour streams in.
- There is no backup or rollback. The atomic rename means a crash cannot leave
  a half-written region, but nothing keeps the previous version of one.
- The DRILL tab installs the first module kind you carry into the chosen
  slot; there is no picker yet. With three kinds this is predictable.
- The bar clips its tail on very long item labels rather than shrinking.
- A full inventory loses the drop when you mine. There are no dropped-item
  entities yet for it to fall into, and refusing the break would trap the
  player in a hole they cannot dig out of.
- The inventory cannot be rearranged: stacks sit where insertion put them,
  and there is no way to move an item into a specific hotbar slot.
- Walking cannot step up a full block automatically; jumping is the mechanism.
  There are no stairs or slabs yet for a lower step to matter (a 0.6 m step
  height is inert while every block is 1.0 tall; it returns with slabs).
- Prone has no combat purpose yet — it earns its keep in one-block tunnels,
  but nothing shoots at you. It stays because removing a stance later is more
  disruptive than leaving one under-used.
- The mantle is a fixed interpolation, not an animation: the body glides up
  the ledge. Slide friction is one constant for every surface. Neither view
  bob nor a sprint FOV kick exists yet — deliberately, since they are the
  fastest route to motion sickness and will arrive with individual toggles.
- Mining progress still accumulates per frame rather than per tick; it is
  world-side and speed-clamped, but it is the one interactive system not yet
  on the command path.
- Fall damage does not exist, so the punishment for a long drop is the climb
  back up.
- Breaking is instant and reach is a flat 6 blocks. `BlockDef::hardness` is
  respected only as breakable/unbreakable; nothing consumes the value itself.
- The deck is screen-space presentation: a drawn shell around the UI, raised
  and lowered with a 2D transform, not a modelled object in a hand. The 3D
  version arrives with the player model and third person.
- The menus are keyboard-only. The mouse is ignored while one is open rather
  than moving a pointer over the entries.
- The overlay is rebuilt from scratch every frame. Fine at a few hundred
  quads; it would want caching before the UI grows much past this.

## Design notes

**The simulation/presentation split is the networking seam.** `vx-world` owns
all authoritative state and never references rendering or windowing. Multiplayer
is not implemented, but adding it later means transporting the existing command
interface over a socket rather than restructuring world logic.

**Chunk storage is palette-compressed.** A chunk is 65 536 blocks; storing a
`BlockId` each would cost 128 KiB. Instead blocks index a per-chunk palette,
packed at the narrowest bit width that fits it. An untouched air chunk collapses
to zero bits and an empty buffer.

**Terrain shaping is deliberately gentle.** Raw fbm clusters around its mean,
which is what made the first terrain a set of broad terraces spanning barely
twenty blocks. A redistribution curve spreads it across the full height range
— but only mildly: a strong curve flattens the middle of the distribution into
literal plateaus joined by cliffs, trading one kind of terracing for a worse
one. Ridged noise adds mountains, weighted by the continent value so they rise
out of highland instead of erupting from the sea.

**Cave thresholds are far higher than they look.** Value noise clusters around
its mean and the ridge fold peaks exactly there, so a threshold that reads as
selective is not: the first attempt carved a sixth of the underground into
swiss cheese and multiplied the triangle count twenty-fold. The calibration
test reports the real carve fraction, and asserts it stays in a sane band.

**The world records which generator shaped it.** Saves store only modified
chunks, so changing generation silently rewrites the untouched parts of every
existing world, leaving cliffs where old edits meet new ground. `level.dat`
carries a generator version and a mismatch is reported rather than left to be
discovered as a seam through somebody's house.

**Simulation runs at a fixed rate, decoupled from the frame rate.** A renderer
that skips a frame looks briefly worse; a simulation that skips a step
diverges. `TickClock` accumulates real time and hands out whole steps.

**The tick system is bounded everywhere, on purpose.** It is the engine's most
inviting denial-of-service surface, because the work it does is driven by world
content rather than by anything the player asks for directly. Three failure
modes are designed against, each with a test that shows the bound holding.
*Runaway catch-up*: a suspended process reports an enormous elapsed time, and
naively catching up takes longer than the frame it is catching up on, so the
next frame owes more — the spiral of death. Catch-up is capped and the
remaining debt is discarded rather than carried. *Queue flooding*: a tick
handler may schedule more ticks, so a cascade is amplification; the queue has a
hard ceiling, refuses past it, and de-duplicates by position so repeatedly
scheduling one block cannot fill it. *Overflow*: a due time is `now + delay`,
and wrapping it would leave a tick permanently overdue, firing every step
forever; delays are capped and the arithmetic saturates. Refusals are counted
and surfaced on the HUD, because a limit that is silently absorbed looks
exactly like one that is never reached.

**Light is derived state, so it is recomputed rather than saved.** Two
channels of four bits each — sky and block — kept apart because a day/night
cycle will dim one and not the other. Recomputing on load keeps saves smaller
and means there is no lighting data on disk for a corrupt file to lie about;
the nibble packing makes an out-of-range level unrepresentable rather than
merely unlikely. Propagation is a flood fill with a hard work ceiling, since
one edit can in principle relight a whole cavern.

**Light is part of the greedy mesher's merge key.** Faces are lit per-quad, and
merging compares the whole facet rather than just the block, so a lit floor and
its shadow never fold into one flat quad. Getting that wrong does not crash —
it silently smears shadows across whole chunks.

**A save is the diff against what generation would produce.** Worldgen is a
pure function of `(seed, position)`, so an untouched chunk can be recreated
exactly and is never written. Only chunks somebody actually modified reach the
disk, which is why streaming across a world you have not built in produces an
empty save. `Chunk` tracks that separately from its mesh-dirty flag.

**The player saves to its own file, with its own failure posture.**
`player.dat` carries pose, mode, inventory and drill — items and modules by
namespaced name, like everything persisted. A chunk that fails to decode is
regenerated from the seed; a player cannot be, so semantic oddities are
sanitised rather than fatal: hostile stack counts load clamped, unknown items
are dropped with a warning, a non-finite position flags a respawn while the
inventory and drill — the parts a player would actually miss — survive. Only
structural damage rejects the file, and then the world is untouched.

**Reading a save is a trust boundary.** Save files can be truncated by a crash,
corrupted on disk, or crafted deliberately, so `vx-save` treats every byte as
hostile: lengths are checked against the bytes actually present before anything
is allocated, offsets use overflow-checked arithmetic, and every packed block
index is verified to lie inside its palette — `PalettedStorage::get` indexes the
palette directly, so an out-of-range index would panic in the middle of meshing.
World names are matched against an allowed character set rather than filtered
for dangerous sequences, because filtering invites being outsmarted. Malformed
input produces an error; it never panics.

**Regions are rewritten whole, through a temporary file and a rename.** Simpler
than an in-place allocator with a free list, and a torn write is impossible: the
old file stays intact until the rename succeeds. Viable only because saves hold
just the modified chunks; if regions ever grow large this is the thing to
revisit.

**Block ids are not stable across runs.** They are assigned in registration
order, so the mod set changes them. Anything persisted to disk must key on the
namespaced string name (`engine:stone`), never the numeric id.

**All `BlockView` coordinates are absolute world coordinates**, never
chunk-local. Mixing the two makes geometry silently vanish.

**The UI is laid out in pixels and owns no GPU types.** `vx-app::hud` takes
state and an `OverlayBuilder` and emits quads; `vx-render::overlay` turns those
into clip space and draws them. Because layout is a pure function of state, the
crosshair, HUD and every menu screen are tested by inspecting the geometry they
produce rather than by looking at them.

**Text is a bitmap face defined in the source.** `vx-render::font` is a 5×7
pixel font written as ASCII art, rasterised into an atlas at startup and
sampled with a nearest filter. No font dependency, no licensing question, and
filtering it would ruin the look anyway.

**The block outline is twelve boxes, not a line list.** wgpu gives no control
over line width — every line is one pixel at any resolution — so a wireframe
cage would be nearly invisible. Solid edges also shrink with distance the way
the rest of the world does.

**The outline is drawn twice, by complementary depth tests.** A single
depth-tested pass shows only the edges of the block's exposed face, so a block
buried in terrain gets no outline at all and two stacked blocks are
indistinguishable. Instead one pass uses `Less` and draws the edges standing
clear of the world; a second uses `Greater`, which passes exactly where
something nearer has already written depth, and draws the buried remainder
faintly. The tests are complements, so nothing is drawn twice. Both passes
carry the same negative depth bias — inflation alone leaves edges within
depth-precision noise of the face they lie on, and they break into dashes at
distance. Biasing the two passes differently would stop them being complements
and put a double-drawn band along every edge.

**The deck and drill are equipment, not items.** They live on the player, not
in the inventory, so they cannot be dropped, overwritten or lost — and since
the deck is the only crafting interface, that turns a possible softlock into
an unrepresentable state. It also means modules live on a plain `Drill` struct
instead of forcing per-item instance data into the item system for one object.

**Mining progress belongs to the world, not the client.** The app reports
intent — "the drill is on this block" — and `World::mine` accumulates progress
and decides when the block yields, clamping the claimed speed and dt because
the caller is untrusted by construction. A protocol where the client announces
"I broke it" cannot be made multiplayer-safe afterwards; this one never says
it. Progress resets when the target changes, so it cannot be charged on soft
dirt and spent on hard ore.

**Inventory arithmetic is conservation law first.** Fixed slots that never
grow, stacks that cannot pass their ceiling, all-or-nothing removal so nothing
can be half-paid, and inserts that return their overflow instead of vanishing
it. Crafting checks that the output fits **before** consuming the inputs —
checking after would need the ingredients handed back on failure, and any bug
in that hand-back is an item dupe or an item shredder. The refusal is
conservative: a craft that would only fit in the space its own inputs free is
refused, costing a rare retry to make the half-done state unrepresentable.

**Player movement is a command consumed on the fixed tick.** Held keys
become one `MoveCommand` per frame; the simulation eats exactly one per tick
at 20 Hz and rendering interpolates between tick snapshots via the clock's
`alpha()`. Look angles are quantised into the command and the simulation reads
only the quantised value — the camera turns per-frame, but that is cosmetic:
positions depend on nothing but the command stream, which is what makes a
recorded sequence replay bit-for-bit. No transcendentals run in the tick path;
yaw resolves through a table built once at startup (bit-exact per platform —
cross-platform identity would need the table baked into source).

**Slides obey voxel physics, not slope physics.** There are no slopes for
gravity to project onto, so the downhill/uphill asymmetry is emergent: slide
friction only bites while grounded, and landing a drop mid-slide converts
some of the impact into more slide — so benched declines feed a slide while
block faces kill one going up. The asymmetry is tested on real stepped
terrain, not asserted about a normal that cannot exist.

**Carried mass is the movement stat.** Speed multiplies by inventory
fullness, floored at 0.55, and stamina drains faster loaded. Everything that
already makes you carry more makes you slower using it — the tension that
makes the number interesting rather than a straight buff.

**Player collision is axis-separated AABB sweeps, substepped.** Each step
resolves y before x and z, so landing happens before sliding can clip a block
corner. No single substep moves further than a fraction of a block, which is
what stops one frame of terminal-velocity fall passing through a floor — and
the substep count is capped, with excess motion discarded, so a hostile dt
buys bounded work rather than either tunnelling or a stalled frame. Unloaded
chunks collide as solid: the edge of the streamed world is ground, not a hole.

**Block picking walks the voxel grid, it does not sample along the ray.**
Marching in fixed steps and testing each point either tunnels through blocks
met at an angle or wastes most of its samples; `vx-world::raycast` uses a grid
traversal that visits every voxel the ray touches, in order, and no others. It
reports the face it entered through, which is what makes "place against the
side you clicked" land outside the block rather than inside it.

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
cargo run --release -p vx-app -- --world myworld
```

Without `--world` nothing is written to disk. With it, the world lives under
`$XDG_DATA_HOME/gamingg/saves/<name>`; modified chunks are written when they
unload, every thirty seconds, and on exit. An existing world keeps its own
seed, so `--seed` only applies when creating one — generating against a
different seed would seam against whatever is already saved.

Controls: `WASD` to move, `Space` to jump (hold to hop; held against a low
ledge it vaults, against a two-block one it mantles), `Left Ctrl` to sprint,
`C` to crouch — pressed at sprint speed it starts a **slide**, and jumping out
of a slide keeps the speed — `Z` to go prone, which fits one-block tunnels.
Sprint, slide and mantle draw stamina; running dry slows you to a walk, never
stops you. Carrying more slows everything: speed scales with inventory
fullness down to 55% at a full pack. Click to capture the mouse. `F` switches
between walking and flying; in flight, `Space`/`Left Shift` move up and down.
`E` raises the deck — the handheld swings up into frame and its screen is the
UI; `E` again stows it. `Escape` opens the menu.

The bar's first two positions are permanent equipment: the **deck** (a
handheld computer — crafting today, drones, supply drops and quests on its
greyed tabs later) and the **drill**, the only thing that breaks blocks.
`1`/`2` select them, `3`–`9` the seven item slots. Digging is hold-to-mine:
hardness finally matters, a progress bar fills under the crosshair, and
looking away discards the dig.

You start with nothing: mining fills the inventory, placing drains the held
stack, and ores drop their mineral rather than themselves. The drill starts
slow on purpose. Speed, reach and fortune modules — and the tier kits that
unlock more module slots — are crafted from mined ore on the deck's DRILL
tab, so the first upgrade is felt on every block after it. Vendors, dungeon
loot and supply drops will later be alternative routes to these same items.

Once the mouse is captured, **left click breaks** the block you are looking at
and **right click places** the held one against the face you are pointing at.
The targeted block is outlined and named on screen. Number keys and the scroll
wheel change what you are holding.

`Escape` opens the menu — `W`/`S` to move, `Enter` to choose, `Escape` to back
out. The world keeps streaming behind it but the camera is frozen, and clicks
do not reach the world while a panel is up.

Running windowed needs a Vulkan loader and drivers plus X11 and/or Wayland
client libraries.

### Rendering without a display

`--screenshot` renders one frame offscreen and exits, so it works over SSH and
in CI:

```sh
cargo run --release -p vx-app -- --screenshot frame.ppm --width 640 --height 360
```

`--ui` picks what the capture draws over the world — `hud` (the default),
`none`, or a menu screen by name (`main`, `controls`, `world`). That is how the
menus get reviewed without someone sitting in front of a window:

```sh
cargo run --release -p vx-app -- --screenshot menu.ppm --ui main
```

The render tests do the same thing and assert on the pixels. Both run against a
software Vulkan driver, so no GPU is required:

```sh
sudo apt-get install mesa-vulkan-drivers
VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json cargo test --workspace
```

Debian and Ubuntu install that ICD arch-suffixed, as above. Point the variable
at whatever `ls /usr/share/vulkan/icd.d/` actually shows — if the path is
wrong the loader reports no driver, and the render tests quietly skip
themselves instead of failing.

### Building in a container

`docker/Dockerfile` is that environment ready made — the toolchain, the X11 and
Wayland headers, and lavapipe already wired up. It is the easy path on a
machine with no Rust installed, and on Windows or macOS, where the local
graphics stack is not the one this targets.

```sh
docker build -t vx-build docker/
docker run --rm -v "$PWD:/work" -v vx-target:/target vx-build cargo test --workspace
```

The named volume holds `target/`. Building onto the bind mount instead works
but is substantially slower.

Tests skip themselves rather than failing when no Vulkan adapter exists at all.

## Licence

MIT OR Apache-2.0.
