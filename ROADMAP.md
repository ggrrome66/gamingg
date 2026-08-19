# gamingg — roadmap

Where this project has got to, and where it is going. Written to be picked up
cold: if you have never seen the repository, start here, then read `README.md`
for how to build and run it.

**What it is.** An original voxel engine and game in Rust, Linux-first, aimed
at a commercial release with mods distributed through Steam Workshop. A
frontier mining game: you prospect, you send machines into the ground, you haul
what they find back to town and sell it.

**Branch.** All work lands on `claude/custom-mcp940-build-18pfok`.

---

## House rules

Three invariants constrain every stage. Most of the hard bugs so far have been
violations of one of them.

1. **Worldgen is a pure function of `(seed, position)`.** No sequence state, no
   global RNG. Chunks generate in parallel in any order and a saved world
   regenerates identically — which is also why saves only store chunks a player
   has *changed*. It is why the flying drone's surveys, the ore lattice, the
   forest and (as of stage 8) the map of towns can all be *derived* rather than
   stored.
2. **Agents are bit-identical given the same inputs.** Villager strolls,
   perception and drone routes come from hashing, never from wall-clock time or
   an RNG the caller cannot reproduce. Anything time-dependent takes the time
   as a parameter.
3. **No wall-clock reads inside render paths.** The renderer draws what it was
   explicitly pushed. That is what keeps the headless captures reproducible and
   the pixel-equality tests honest.

Two more that matter at the seams:

- **Persisted data is keyed by name, never by numeric id.** Block ids are
  assigned in registration order, so a mod shifts them; a save keyed on numbers
  would silently reload as the wrong blocks. Same reasoning drives name-keyed
  skills, stockpiles and upgrades.
- **Crate boundaries hold.** `vx-world` knows nothing of rendering. `vx-agent`
  knows nothing of rendering *or* game fiction — no quests, no economy. Game
  fiction lives in `vx-app`.

## Licensing constraints

Written down because they are easy to forget and expensive to get wrong.

- **This is original code.** No Minecraft source, no decompiled game code, no
  Mojang assets. It is not a fork of `mcp940` or any decompiled-source
  repository — redistributing decompiled game code violates both the MCP
  licence and the Minecraft EULA. The 1.12-era conventions borrowed (16×16
  chunk columns, 256-block height, ~1 m voxels) are design reference points.
- **Motherload is a *vibe*, not a source.** The digger rig deliberately evokes
  the classic dig-and-sell flash games. Every pixel is procedurally generated
  here; no sprites, names or trade dress.
- **The handheld drill has no brand.** The tunnel-boring company whose
  silhouette it evokes is a live trademark. In game it is "a compact boring
  drill" and it stays that way.
- **The pocket arcade will never be a Doom port.** WAD assets are not
  redistributable and a GPL engine would encumber the binary. When that stage
  arrives it is an original mini-FPS.
- **RuneScape lent the *shape* of the XP curve**, not its numbers. Curves are
  not copyrightable; names are, and ours are generic.

---

## Shipped

| Stage | Commit | What landed |
|---|---|---|
| 1 | `9deecc5` | Core scaffold |
| 2 | `bc9a8ef` | Renderer and a playable binary |
| 2.x | `e3e4ad7` | Block editing, physics, persistence |
| 2.5 | `4a88348` | Spline terrain, parallel meshing, frustum culling |
| 3 | `c2c8c6a` | Ore geology |
| 4 | `def13db` | Instanced objects, drone swarm, three mining methods |
| 4.5 | `39e9cc7` | Eight review fixes |
| 5 | `6834e8a` | Flying drone, sector scanning, ferry loop, minimap |
| 6 | `2d263c2` | Rigs, handheld drill, skills, bitmap-font HUD |
| 6.5 | `229a299` | Starting village, villagers, trading shop, trees |
| 7 | `42fb8ab` | Third person, drone piloting, NPC senses |
| 8 | `f050c16` | Day/night, container towns on a lattice, the beacon network |
| 9a | `7dcff15` | Residency pinning, content hashes, the command journal, seed tree, body ids, a real crew |

**1 — Core scaffold.** Block registry, palette-compressed chunk storage,
worldgen, greedy meshing. A chunk is 65 536 blocks; storing a `BlockId` each
would cost 128 KiB, so blocks index a per-chunk palette packed at the narrowest
bit width that fits.

**2 — Renderer and binary.** wgpu pipeline, camera, chunk streaming, and the
`--screenshot` path that renders offscreen with no display — which is how the
whole stack gets smoke-tested against a software Vulkan driver in CI.

**2.x — Editing, physics, persistence.** Voxel raycast (DDA), block break and
place through *cancellable events* (nothing listens yet; the point is that mods
will be able to veto an edit without any call site changing), AABB player
physics resolving one axis at a time, and region saves.

**2.5 — Terrain that means something.** Height comes from three independent
fields through splines rather than summed octaves — summing drives everything
toward the mean and produces flat terraces. Continentalness, erosion and
peaks/valleys are sampled separately and each mapped through its own curve, so
"high but flat" and "low but jagged" are both expressible. Relief went from ~22
blocks to ~100. Plus parallel meshing and frustum culling, the latter with a
test that renders with culling on and off and asserts the pixels are identical.

**3 — Ore geology.** Copper in irregular buried blobs from a jittered lattice.
Outcrops are a *consequence*, not a feature: a body sitting high enough pushes
through the soil. Every visible outcrop has ore continuing beneath it, so
prospecting means something.

**4 — Machines that dig.** The `vx-agent` crate: job board, BFS flow fields,
and three mining methods — adit, decline, open pit — ranked on *cost* rather
than dug volume, because the excavation is dug once and the haul is made
forever. Bodies are mined in benches, not as boxes, so a drone can always climb
back out of its own hole.

**4.5 — The review round.** Eight verified findings fixed, mostly swarm stalls:
asymmetric flow-field edges under overhangs, an unbreakable-block wedge, a
haul livelock, and a tick backlog that fast-forwarded a drone across the map
after a stall.

**5 — The air, and the map.** A flying drone that sweeps sectors and pings ore
up to 24 blocks down, ferries piles home once you place a base container, and a
fog-of-war minimap that stores nothing but the set of explored chunks —
unloaded terrain is recomputed from the height field on demand.

**6 — The game in your hands.** Composite cuboid rigs (the machines finally
look like machines), a handheld drill with hold-to-dig, name-keyed skills on a
RuneScape-shaped curve, and a 5×7 bitmap font with a HUD — the font that every
later panel is built on.

**6.5 — The hometown.** A static village at the origin, identical in every
seed, blended into the terrain over a smoothstep skirt; villagers; a supply
shop that really trades. Plus trees and grass, which needed the engine's first
non-cube geometry (crossed quads).

**7 — Eyes and hands.** Third-person camera with a player body and terrain
pull-in; the handheld fleet uplink that lets you look through any machine's
eyes and take the master override, driving and cutting yourself; and NPC
perception with line of sight and memory. The piloting layer is deliberately
the foundation for drone-assisted combat later.

**8 — The frontier.** A day/night cycle pushed to the renderer as a uniform
(never read from a clock in a render path, so captures stay byte-identical);
towns rebuilt in shipping containers and corrugated metal with a radio mast at
the centre; **many** towns on a jittered 512-block lattice, varied by size and
speciality; and a beacon network whose derived job postings can name a town the
player has never seen, pinning it on a map that is still black around it. The
structural half of the round was splitting `height_at` into `natural_height_at`
plus a blend, so town siting could stop reading its own output.

**9a — Determinism, and cashing it in.** An external design report prompted this
round; the audit of it is in the commit history. It began by finding a real bug:
`World::block` reports unloaded chunks as air and agents read through it, so a
drone's decisions depended on which chunks the camera had streamed in. Fixed by
pinning an operation's working span, and gated by a test that digs the same body
at two residencies and compares content hashes — verified sensitive by removing
the pins and watching it fail.

That made the round's real idea safe: agents author almost every block change in
this game, and they do it deterministically, so the save should record *orders*
rather than outcomes. The command journal does, and `--replay` rebuilds a world
from it and checks the result — a determinism oracle covering worldgen, agents
and editing at once. Also: `SeedPath`, `BodyId` on every chunk key, and a crew
of drones finally exercising the job board's contention paths.

---

## In flight — Stage 9b: the trade network

Towns get stockpiles and prices that differ between them; they broadcast what
they need over the network stage 8 built; trade drones run routes. The
game-of-life economy — a shortage in one town becoming a run on another — is
the point, and it is the first thing in the game where the world changes while
the player is not looking.

The approach is settled and comes from the design report's economy chapter,
which is genuinely excellent: **fast-forward-on-read** town state (integrate
only when something asks), a **discrete-event queue** so idle towns cost
nothing, **damped tâtonnement** with capped iterations to solve clearing prices
rather than nudge them, **net buy/sell pressure** pricing so shocks are legible,
**Leontief input-output** for what a town must import, and **min-cost flow** for
who ships to whom. Region-rollup LOD from the start: detail near the player,
aggregates far away.

Groundwork already in place: `postings_for` derives a board from a town and its
neighbours, `Ledger` records what the player did about it, `towns_near` answers
"who can this mast hear" without loading a chunk, and the command journal is the
shape the economy's own event log wants. What 9b adds is state that *persists
per town* — the first thing in the world that is not a pure function of the seed
— and the rule for how it changes.

---

## Also outstanding from 9a

**Packed chunk-local quads.** Mesh vertices are 36 bytes and carry absolute
world positions as `f32`, so a merged quad costs 168 bytes and the precision
wall is baked into the mesh data rather than just the camera. Storing one `u64`
per quad and synthesising the six vertices in the vertex shader is ~21x less
geometry memory, removes a buffer per chunk, and opens the floating-origin seam.
Deferred out of 9a because cross blocks emit diagonal quads that need their own
encoding, and because it is a renderer change that wants the byte-identical
frame tests watched closely rather than rushed.

**Letting the journal shrink saves.** Region files are still written every save,
so the journal is currently an oracle rather than a disk win. The keyframe
machinery is in place; turning it on is worth doing after the oracle has run
against real sessions for a while.

## The arc beyond

| Stage | What | Why here |
|---|---|---|
| 10 | Fuel loop | Rods drain machines; the shop already sells, so this closes a loop rather than opening one |
| 11 | Text + terminal | The font exists; the terminal is its third user after the HUD and the panels |
| 12 | Crafting + upgrades | Needs the fuel and trade economies to have something to feed |
| 13 | Wear, breakdowns, recovery | Machines that can fail need machines you can reach — piloting shipped in 7 |
| 14 | Factions, reputation, PvE | Towns and contracts exist by then; `Perception` (stage 7) is already the shape a hostile needs |
| 15 | Uranium, oil, gas | New resource *kinds* (fluids, wells) — a bigger worldgen change than more ore |
| 16 | The pocket arcade | Endgame toy: an original mini-FPS on a craftable handheld |

## Known rough edges

Tracked in `README.md` under "Known rough edges" — currently ~25 entries, the
notable ones being: saves store a whole chunk snapshot per modified chunk;
water is alpha-blended without depth sorting; a running excavation is not
persisted; only one drone and one flier are ever created; and there is no
player-carried inventory, so everything routes through the fleet's base pile.
