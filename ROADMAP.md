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
| 9a | `b77aedc` | Residency pinning, content hashes, the command journal, seed tree, body ids, a real crew, 8-byte packed quads |
| 9b | `dbfe091` | Town books, moving prices, inter-town freight, player trade runs, copper bars |
| 10a | `63da370` | Machines cost credits, trade map on the console, handheld map page |
| 10b | `b12b1e2` | Player movement on a fixed clock: stance, sprint, slide, vault, mantle, stamina, carried mass |
| 10c | _this_ | Pregenerated spawn, streaming off the frame thread, the player's house, mail-order, the welcome panel |

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
and editing at once. Also: `SeedPath`, `BodyId` on every chunk key, a crew of
drones finally exercising the job board's contention paths, and geometry packed
from 168 bytes a quad down to 8 — chunk-local, with the vertices synthesised in
the shader, verified by a byte-identical capture.

**9b — The trade network.** What stage 8's masts were for. Towns keep books,
derived from their site until something touches them; they produce, consume and
price by speciality, and they ship surpluses to neighbours that are short. A
counter now asks its own town what it pays, so ore is cheap at a mine and dear
at a refinery, and a big sale moves the price. `engine:copper_bar` makes it a
chain rather than a gradient. One shipment record serves town freight and the
player's alike, and a load in the air is a lerp rather than a simulation — free
by the hundred, and drawable as a real machine when one passes close.

Both the books and the network catch up in fixed quantised windows, which is the
round's central care: these flows clamp, and once a clamp binds, one big step
and many small ones disagree — so the world would otherwise depend on when the
player happened to look. Two tests hold that line, and both were verified by
breaking the code and watching them fail.

**10a — The opening loop, and maps worth reading.** Machines were free: a flier
arrived the moment the world opened and a crew of drones appeared unpaid on
every dispatch, while the shop sold two capped upgrade lines and nothing else.
So the economy had no sink and there was nothing to mine *for*. `garage.rs` is
that sink — machines are name-keyed, cost credits, and each one costs half again
what the last did. You keep one free starter flier so an empty garage is still
playable.

The maps turned out to be half built: the minimap has always drawn unexplored
ground as a dark pane and always stamped markers over it with no visibility
test. `render_map_sized` makes that picture drawable at panel size, so the
beacon console can inset a trade map — this town, the destination pinned, live
caravans, black where nobody has walked — with a bearing under it, and the
handheld gains a map page on `Tab`.

---

## Shipped — Stage 10b: player movement

The player was the last actor in the world still integrated off the frame clock.
Everything else took ticks; the player took `dt`, which meant where you ended up
depended on how fast your machine drew. House rule 2 says agents are
bit-identical given the same inputs, and the player is an agent.

**Movement is a command now.** Held keys become a `MoveCommand` once per frame;
the simulation consumes one per tick at 64 Hz. Same idiom as `PilotCommand`,
same reason. Look angles are quantised into the command as `i16` and resolved
through a direction table, so nothing in the integration path calls `sin` every
tick on the value the whole simulation keys off.

**Sixty-four hertz, not sixty.** The journal speaks in mining ticks and mining
runs at 8 Hz. Sixty over eight is seven and a half; sixty-four over eight is
exactly eight. The integer ratio means `Advance { ticks }` keeps the meaning it
already had and replay runs eight movement sub-ticks per journal tick.

**The verbs.** Sprint, crouch, prone, slide, vault, mantle, coyote time, jump
buffering. `STEP_HEIGHT` dropped from 1.05 to 0.6, so a full block can no longer
be strolled up — but vaulting is **automatic on contact**, which is what keeps
the benched pits the mine planner cuts pleasant to walk through rather than four
hundred keypresses an hour. Above 2.2 m there is nothing to do but go around.

**Sliding is an impulse and a decay curve.** Entering one kicks horizontal speed
to 1.4× and installs low friction; a slide-jump keeps every bit of the speed it
built. The design note wanted gravity projected onto the surface normal so a
slide would carry downhill — but there are no ramps in a voxel world. A decline
is a staircase and every normal is exactly `+Y`, so the projection yields
nothing. What actually differs downhill is that the run is spent falling off
one-block steps, so a slide converts a slice of each drop into forward speed.
Same behaviour, honest mechanism.

**Carried mass is the movement stat.** Speed scales from 1.0 empty down to a
floor of 0.55 fully laden, and mass raises the sprint drain. This wires three
systems that did not touch: `Stockpile`, the `Logistics` skill and the wallet's
cargo upgrades all became movement upgrades without a single new item — every
upgrade that lets you carry more is also a tax on using it. The load rides in
the command as a quantised byte, so a replay reproduces the laden player rather
than a lighter, faster one.

**The journal covers the player now.** `Command::Move` is recorded on *change* —
`Advance` already folds, so holding W for a minute is two entries — and
`--replay` reports where the walk ended. The regression test is the determinism
oracle that already existed rather than a fixture built beside it.

Three bugs worth recording, because each was found by a test rather than by
looking:

- **Ground speed was capped below sprint.** With exponential drag and capped
  acceleration the fastest a body holds is `accel / friction`. The note's 60 and
  10 put that ceiling at 6.0 m/s, making `SPRINT` at 6.5 a number you could
  never reach. `ACCEL_GROUND` is 100.
- **The inset made penetration worse, not better.** Shrinking the sweep box
  guarantees only the *shrunken* hull is clear, which lets the real hull
  penetrate by exactly the amount you shrank it — and the error compounded across
  sub-steps until the body was embedded in a wall. It is a tolerance in the block
  query now, ordered `INSET < SKIN` on purpose.
- **A slide died on the first step it fell off.** Leaving the ground ended the
  stance, and going downhill *is* leaving the ground, so the one case the verb
  exists for was the one case it could not do.

### Deferred from this round

The drone-traversability warning. A 2.2 m mantle means a hole the player can
climb out of is not necessarily one a drone can drive out of; breaking that
invariant is fine, breaking it *quietly* is what strands machines. Surfacing it
on the handheld is a mine-planner change and waits.

---

## Shipped — Stage 10c: home

The player woke up on a road while the world assembled itself around them, and
every walk across it hitched. Both had the same shape of cause: work done at
the wrong time, on the wrong thread, or over and over.

**The lag had three causes, and the biggest was the dumbest.** Every chunk
loaded from disk re-read and re-decoded its *entire* 32×32-chunk region file —
up to eight whole-file decodes per frame while walking through saved ground.
`WorldSave` now holds decoded regions in a small cache, invalidated by its own
writes; sixteen chunks from one region cost one read, by test. Second:
generation ran synchronously on the main thread. It now runs on a background
worker holding a *clone* of the generator — terrain is a pure function of
`(seed, position)`, so the clone is bit-identical by contract and by test, and
anything the simulation needs immediately still comes through the synchronous
path, so the stage-9a pinning story is untouched. A result arriving for a chunk
already resident is discarded: the resident copy may carry edits. Third: the
wanted-list sort and the dirty scan ran every frame; both are now gated.
Meshing stays a budgeted fork-join on purpose — it was never the hitch.

**The spawn area pregenerates before the first playable frame** — the whole
render distance, nearest-first, with progress in the title bar — and then the
hometown footprint is **pinned resident forever** with `pin_span`, the
"spawn chunks" idea Minecraft ships and stage 9a happened to have already
built. Coming home never hitches on regeneration. `--view-distance` picks the
radius (4–16, default 8).

**You wake up in your own house.** STONEHAVEN gained one building on its
south-west quarter — hometown only, by plan split on `is_home()`, so your house
is singular as a property of the data. Inside: a **chest**, your home
warehouse. Breaking it packs its contents into thin air and carrying the block
somewhere else unpacks them — the contents were never in the world, the block
is a marker and the side table is the truth, the pattern the fleet's base
container established. One chest may stand at a time; the place hook enforces
it. Outside, beside the door: a **mailbox**, unbreakable town furniture like
the counter, which deletes every "what if the mailbox is gone when the mail
lands" edge case in one line of registry data.

**Mail-order.** The shop counter now lists, for each good, a parcel offer from
the cheapest other town in radio range with stock to spare. Pay the source
price plus freight — freight priced by the same `travel_ticks` the caravan
flies by — and a real `Shipment` crosses the map with `Owner::Mail`, visible on
the trade map like any caravan, landing in your mailbox whichever counter took
the order. Mail never touches the destination's books: it was bought at the
source, and it lands in a mailbox, not a market. The order *does* move the
source town's books, honestly. Machines stay instant at the counter — the
opening loop's first-drone moment is not getting a shipping delay.

**The welcome panel.** First boot opens an in-world panel: where your stuff
is, and the whole story — parsed out of *this file* at compile time via
`include_str!`, so the changelog cannot drift from the truth. A test insists
the stage that built the panel appears in it. Dismissing it writes a zero-byte
marker in the config directory (player metadata, not world state) and it never
shows again. `--changelog` prints the same lines headless; `--welcome` draws
the panel over a capture.

**The honest cost: old journals restart.** Adding the house changes generated
ground, so a v3 journal would replay against terrain that no longer generates
and report divergence that is nobody's fault. The journal is now VERSION 4; an
old one is refused with a warning and the fresh log declines genesis coverage
(`keyframe_tick` nonzero), so `--replay` says "nothing to compare against"
instead of lying either way. Old worlds stay fully playable — region snapshots
load as ever, and player edits win over regeneration, which is correct.

---

## Planned — Stage 10d: the arsenal, and what robbing costs

Caravans can be intercepted. Doing it makes you wanted.

### The weapon system

Three decisions hold the whole arsenal up, and each one is a pattern this engine
already uses somewhere else.

**Weapons are name-keyed data, not code.** `engine:slug_launcher` with fire
rate, muzzle speed, damage, spread, ammunition kind and recoil as fields — the
same shape blocks, skills, upgrade lines and machines all have. Adding a weapon
is a row and a tile, which is the only way an arsenal stays affordable.

**A projectile is a sum, not a simulation.** Fired from a point at a tick with a
velocity, so where it is at any later tick is arithmetic — exactly the trick
trade caravans use. Nothing is stepped, a hundred rounds in the air cost
nothing, and it replays exactly. Hit detection is the existing `raycast_solid`
over the segment a round swept since the last frame, so the physics needs
nothing new.

**Ammunition is a trade good**, which means the economy already knows how to
make it scarce. A firefight becomes a supply problem, and that is the game this
wants to be.

### The arsenal to grow into

Named for what they do rather than for anybody's trademark, per the licensing
constraints above.

| Weapon | Role | Rests on |
|---|---|---|
| Slug launcher | The baseline: one kinetic round, slow, punchy | projectiles, damage |
| Scattergun | Close work, wide spread, falls off hard | the spread field |
| Mining charge | Thrown, timed, breaks blocks | `break_block`, an area query |
| Beam cutter | Continuous, no travel time, drains power | a hitscan path, power draw |
| Rail lance | Long charge, pierces several targets | charge-up state |
| EMP burst | Drops a drone *without* wrecking its cargo — the thief's weapon | machine state rather than damage |
| Guided missile | Slow, tracking, expensive | steering, and a reason for countermeasures |

10d ships the slug launcher and the EMP burst. The rest are rows.

### Interception and bounty

A caravan is already drawn as a real machine when one passes within sight, and
its position is already a pure function of the clock — so shooting one needs no
new state. Knocked down, its load falls to you.

The town that sent it remembers. Bounty is **economic first**: its market pays
you less, then refuses to trade, and the shortage you caused drives its own
prices up. Then its mast starts **posting a contract on your head** through the
board that already exists — groundwork that bites properly once there is
somebody to take it.

### What is deliberately not in 10d

Player health, hostile escorts, and death. Nothing shoots back yet. That keeps
10d to a weapon system and a consequence rather than a whole combat model, and
the data-driven shape means hostiles slot in later without a rewrite.

---

## Planned — the civic layer: permits, offices, elections

The town-law arc, in three rounds. The user's design, recorded whole so none of
it gets lost between rounds.

### B1 — Build permissions and the bounty ledger

Each building carries an **authorization container** — tool-cupboard-shaped,
with **no upkeep and no decay**: houses persist indefinitely. Editing a
building's blocks requires being authorized on its container. Outside town
limits, free building everywhere.

Authorization is **ranked, not binary**. The town priority order, top down:
**Sheriff > Mayor > building owner > authorized guests > everyone**. The
sheriff's rank is the point: a sheriff can enter — and later, when doors and
locks exist, unlock — buildings they were never authorized on, because
enforcement has to reach where bounties hide. Shops and the **security office**
(where the sheriff and deputies live) sit at a much higher authorization
difficulty than houses.

**Bounty points**: playing the loop legally — mine, haul, trade — accrues zero.
Every attempted edit of a building you are not authorized on adds bounty points
to your ledger; attempts count, the wall does not have to fall. This is the
ledger the 10d arsenal's bounty ("economic plus a contract on your head")
reads.

The mechanism is already waiting: `BlockBreakEvent`/`BlockPlaceEvent` are
`Cancellable`, the bus has priority subscriptions and `emit_cancellable`, and
there are **zero subscribers today** — B1 is the first, with no call-site
changes. Prerequisites: `Blueprint` gains names/roles and exposed per-building
bounds; the 10c chest is the prototype authorization container.

### B2 — Towns that work: offices and the warrant chain

Named town offices: **Mayor, Sheriff (plus deputies), Shop clerk, Residents**.
Every individual in town runs the same loop the player does — mine or gather,
trade at the market, accumulate credits — because credits are the end goal of
every individual, NPC and player alike. Villagers become deterministic economic
agents trading against the town books on the same tick clock; the economy's
quantised catch-up already makes that replay-safe.

**Trust through trade** is the per-resident stat that unlocks guest
authorization on their building.

**The warrant chain**: when an individual's bounty crosses a threshold, the
sheriff cannot act alone — they must obtain a **warrant from the mayor**, and
only then may dispatch the offender. Enforcement-by-force lands with stage 17
(hostiles and health); until then the chain carries consequences short of force
— fines, revoked market access.

### B3 — Elections and goodwill

The town's main computer — the beacon console — gains a **voting page**:
residents elect the mayor and sheriff. Votes are cast on **goodwill points**,
earned through trade interactions — the resident who profits from trading with
you votes your way, which reuses the trade ledger rather than inventing a
reputation system from nothing.

**The player can hold office**: win the sheriff's badge at the ballot box,
found a new town and hold its offices by default, or take a town over — the
hostile path, priced by the bounty system itself. Offices held across towns tie
into stage 19 (factions).

---

## Planned — the world below: caves and bunkers

From concept art supplied for the project: a small civilian bunker interior
(one room, reinforced doors onto a stairwell, generator and exhaust, breaker
box, radio bench, a supply shelf and a cot — lived in, untidy) and a large
industrial complex (silo hall, dome, conveyor ramps, catwalks, a surface
canopy over a multi-level base). Those are the small and large tiers.

Everything below stays inside the house rules: sited as a pure function of the
seed, stamped rather than stored, and original in every name and shape.

### Caves

The engine's terrain is a **height field**. `fill_column` (`gen.rs:480`) fills
one column from bedrock to a surface height, and nothing anywhere carves a hole
in the middle of it. Caves are therefore the first genuinely **3D** thing
worldgen has ever done, and that — not the caves themselves — is the cost of
the stage.

The shape: 3D ridged or Worley noise thresholded into air below the surface,
evaluated per block, pure in `(seed, x, y, z)` so it stays chunk-parallel and
needs no cross-chunk context. Masked out of every town footprint
(`town::footprint_contains`) so a plaza cannot open into a void, and floored
above bedrock so the world keeps its bottom.

**Why caves are worth doing early:** stage 10a made you hand-mine your way to a
first drone. A cave is where hand-mining is *pleasant* — ore at the surface of
a wall rather than under twenty blocks of overburden — so it directly serves the
opening loop rather than only the late game.

**What caves break, precisely.** `flow::settle` (`flow.rs:56`) walks a drone
down until it finds solid ground. Carve a void under one and it falls in. The
mine planners assume a solid column to cut through, and `working_span` pinning
assumes the ground stays where it was surveyed. None of that is fatal, but all
of it needs a pass, and pretending otherwise would be how a stage overruns.
`World::surface_y` is safe — it reads the topmost solid block, so a cave beneath
does not confuse it.

### Bunkers

**Siting** is the lattice idiom used three times already — ore deposits, trees,
towns: a jittered cell, splitmix64 per property, a large cell because bunkers
should be rare. Tier from the same hash, weighted so the big ones are rarer and
further out. Derived, so a bunker three kilometres away costs nothing until
somebody goes there, exactly like a town.

**The shell is the interesting part, and the engine already has it.** Blocks
carry `hardness: Option<f32>` and the drill spends `dt * power / hardness`
(`main.rs:1535`). Stone is `1.0`. A bunker shell at, say, `400.0` is four
hundred times slower to cut — perfectly possible, and almost never worth it.
That is the soft gate asked for, with **no new mechanic at all**: just a block
with a big number.

It must be `Some(large)` and never `None`. `None` means bedrock — genuinely
unbreakable — and that would be the wrong answer: the point is that digging in
is a *choice you can make and usually regret*, not a wall.

**Getting in** is meant to be the door. A bunker breaks the surface with a hatch
or a stair head, which makes it findable by walking, by the flier's scanner (it
already sweeps sectors and drops pings — a structure ping is the same
machinery), and pinnable on the maps 10a just built. A large one is partly
above ground by design, so it reads as a landmark from a distance; a small one
you stumble on.

**Interiors** are the reason to finally build the **jigsaw / template-pool**
generator that has been deferred since stage 8: a start room plus modules
attached at connection points, drawn from a pool, deterministic from the site
hash. Rooms are the existing origin-relative ASCII layer blueprints
(`town/plan.rs`) stamped at a negative height instead of a positive one — the
same code, a different Y.

**Loot** is a name-keyed table over goods that already exist, plus the ones the
concept sheet lists: rations, water, spirits, cigarettes, oil, rope, blankets,
a first aid kit, tools, mechanical and electrical parts. Derived from the site
hash and consumed through the ledger, exactly as `postings_for` derives a board
and `Ledger` remembers what was taken — so two visits agree and nothing is
rolled twice. This also gives the economy a **source** to match the sink stage
10a added: things you cannot buy, only take.

**Occupants**, the two flavours asked for:

- *Mobs.* A hostile is a drone that walks toward you instead of toward a job.
  `FlowField`, `is_standable` and `settle` already move a machine through a
  world, and `Perception` with `sight::obstruction` (stage 7) is already the
  "can it see you, with rock in between counting" primitive. The movement half
  is close to free.
- *Military.* The same movement, carrying the stage 10d weapons, and aligned to
  something — which is what makes them a faction problem rather than a monster
  problem.

**The honest sequencing problem:** both flavours attack you, and stage 10d
deliberately leaves out player health, hostiles and death. So bunkers split in
two. The **built** half — sited, shelled, entered, laid out, looted — can ship
as soon as caves have paid for the 3D carve. The **occupied** half waits for
health, and a bunker without occupants is still worth entering, because the loot
is the point.

### The three tiers

| Tier | Shape | Reads as |
|---|---|---|
| Small | One room off a stairwell: reinforced doors, generator and exhaust, breaker box, radio bench, supply shelf, a cot. Somebody's private shelter, lived in and untidy | A find. Modest loot, a story told by what is on the shelf |
| Medium | Several rooms off a corridor, its own stairwell and generator room. Military or civilian | A dungeon. Worth clearing, worth coming back for |
| Large | Multiple levels, a silo hall, a dome, conveyor ramps and catwalks, part of it breaking the surface under a canopy | A landmark you can see from a ridge and plan an expedition to |

---

## Also outstanding

**Rebasing the floating origin.** Chunk geometry is chunk-local now, so the
precision wall is out of the mesh data — but nothing shifts the origin as the
player travels, so the camera still meets it eventually. The seam is open and
the rebase is a small change on top of it.

**Letting the journal shrink saves.** Region files are still written every save,
so the journal is currently an oracle rather than a disk win. The keyframe
machinery is in place; turning it on is worth doing after the oracle has run
against real sessions for a while.

**A real min-cost flow.** Trade routing is greedy nearest-deficit matching. At a
few dozen towns it picks the runs a proper solver would; worth revisiting only
if the traffic it produces reads as dull.

## The arc beyond

| Stage | What | Why here |
|---|---|---|
| 11 | Caves | The first true 3D carve in a height-field world, and the thing that makes hand-mining pleasant — so it serves the opening loop 10a just built, not only the late game |
| 12 | Bunkers, built and lootable | Rests on caves paying for the 3D carve. Sited on the same lattice as towns, shelled with a very high `hardness`, laid out by the jigsaw generator deferred since stage 8, and looted — a *source* to match 10a's sink |
| 13 | Fuel loop | Machines stop being perpetual. Markets price goods and the network hauls them, so a fuel is one more good on an economy that already knows how to make shortages — and a fuel shortage is the first one that can stop you |
| 14 | Text + terminal | The font exists; the terminal is its third user after the HUD and the panels |
| 15 | Crafting + upgrades | Needs the fuel and trade economies to have something to feed |
| 16 | Wear, breakdowns, recovery | Machines that can fail need machines you can reach — piloting shipped in 7 |
| 17 | Hostiles and health | The half of combat 10d leaves out. `Perception` (stage 7) is already the shape a hostile needs, and 10d's bounty contracts are already something for one to take |
| 18 | Bunkers, occupied | Mobs and military. Held until here because both attack you, and that needs the health model stage 17 brings |
| 19 | Factions and reputation | Bounty (10d) is per-town standing; factions are that standing shared between towns — and what a bunker's military garrison belongs to |
| 20 | Uranium, oil, gas | New resource *kinds* (fluids, wells) — a bigger worldgen change than more ore |
| 21 | The pocket arcade | Endgame toy: an original mini-FPS on a craftable handheld |

## Known rough edges

Tracked in `README.md` under "Known rough edges" — currently ~25 entries, the
notable ones being: saves store a whole chunk snapshot per modified chunk;
water is alpha-blended without depth sorting; a running excavation is not
persisted; only one drone and one flier are ever created; and there is no
player-carried inventory, so everything routes through the fleet's base pile —
which is also what the movement system now weighs you down by, for want of a
real backpack.
