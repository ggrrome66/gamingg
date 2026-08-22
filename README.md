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

M3 stage 10a is in, and it started by finding that the game's opening loop was
not implemented. **Machines are earned now.** A flier used to arrive the moment
the world opened and a whole crew of drones appeared, free, on every dispatch —
so there was nothing to mine *for* and the economy had no sink. You keep one
free starter flier; every drone after that is bought with credits at a shop
counter, and each one costs half again what the last did. Mine by hand, sell
your ore, buy your first drone: that is the opening.

**And the maps read like paper maps.** Pick a trade run at a beacon console and
it draws a map of the route — this town, the destination pinned, live caravans
crawling between them, and everything you have never walked pitch black — with
a bearing under it: `IRONREACH - NW 206M`. If you have not been there, it is a
pin sitting in the dark and you have to go and find it. `Tab` on the handheld
turns to the same map centred on you, with your machines, the towns you have
found and the traffic in the air.

Before that, stage 9b — **the towns are in business.** Every town keeps books now:
a mining camp pulls ore and stone out of the ground and burns through timber, a
refinery eats that ore and turns out **copper bars** worth far more than what
went into them, and a depot chews through finished goods and ships them onward.
So somewhere always has too much of something and somewhere else always wants
it.

Prices move with that. Ore is cheap at a mine tripping over it and dear at a
refinery that needs it — and a counter now knows which town it stands in, so
walk in and dump forty loads on a small market and you will watch the price fall
out from under you. That is not a random walk; it is what happens when you flood
a place.

**The towns haul to each other on their own**, whether you are watching or not,
which slowly closes the very gap you were making money on. And you can put your
own drones on the same routes: walk up to the mast, pick a run, send a load out
of your pile, and get paid at the far end when it lands.

A load in the air is not simulated — it left here at this tick and lands there
at that one, so where it is right now is a sum. A hundred of them crossing the
map cost nothing, and when one passes near you it is drawn as a real machine.
Same story for the towns: one two kilometres away is not ticking away in the
background, it just knows when it was last looked at and works out the gap in
one go when you finally walk in the door.

Before that, stage 9a — engine work rather than new toys, and it started by finding
a real bug. `World::block` reports unloaded chunks as air, and drones read the
world through it, so a machine working near the edge of the streamed-in set saw
air where there was rock. That made its decisions depend on which chunks were
resident, and residency follows *your camera* — so the same dispatch could dig
a different hole depending on where you happened to stand. Operations now pin
the ground they will read, and a test digs the same body with four chunks
resident and with none and holds the results identical. It fails without the
pins.

That fix unlocked the round's real idea. Ask who edits this world: you break a
few hundred blocks by hand; one drone moves tens of thousands, and moves exactly
the same ones every time. Writing that to disk is writing down the output of a
deterministic function whose input was one line. So the game now keeps a
**command journal** — mine that area this way, run this many ticks — and
replaying it re-derives every block those orders produced. A fixture mine of
several hundred blocks is described by fewer than a dozen entries.

The prize is `--replay`, which rebuilds a saved world from its journal and
compares content hashes against the ground on disk. It needs no GPU and no
window, and it covers worldgen, the agents and the editing path at once.

Also in: a **crew** of drones instead of one — the job board was built for
contention and had never run any of it — plus the seed tree and a body id on
every chunk, so a planet is additive later rather than a rewrite.

And the geometry got twenty-one times smaller. A merged quad used to cost four
36-byte vertices and six indices; it is now **eight bytes**, with the six
vertices of its two triangles synthesised in the vertex shader. Positions are
chunk-local, which also pulls the `f32` precision wall out of the mesh data —
it was baked into every vertex, so walking far enough would have degraded the
geometry itself and not just the camera. Verified the only way this can be
verified: a real town capture through the new path is byte-identical to the
same capture taken before it.

Before that, stage 8: The sun moves. A day runs twenty real minutes and the light,
the sky and the fog all turn with it — golden hour, dusk, real dark, dawn —
with the hour on the HUD and saved beside the world, so you come back to the
time you left. The townsfolk keep those hours: at sundown they walk home,
stand down inside for the night, and are back out on the plaza in the morning.

The village became a **frontier**. Towns are built from shipping containers
and corrugated metal now, rusted and patched, with grated catwalks and a radio
mast in the middle of each one. And there is more than one of them: towns sit
on a 512-block lattice across the whole world, each with its own name, size and
trade — depot, mine or refinery. Your hometown is still pinned at the origin,
authored, and byte-identical in every seed you ever roll; everything past its
skirt is derived from the seed.

The mast does something. Press `E` at the console at its foot and the town
posts work: haul so much stone to Redfork, sweep a sector out at such-and-such.
Take a job and it pins the target on your map **even if you have never been
there and the ground around it is still black** — and freight is signed for at
the far end, so the walk is the job. Nothing about that is pre-generated:
worldgen is a pure function of the seed, so "which towns are within two
kilometres" is a few hundred hashes and loads no chunks. A beacon can name a
town that does not exist as a single block until you walk over and it builds
itself exactly where the posting said it would.

Before that, stage 7: Press `C` and the camera swings over your shoulder — there
is a body there now, hi-vis jacket and hard hat, and the camera slides in
close rather than through the rock when you back into a wall. Press `V` and
the handheld lists the fleet: pick a machine, look through its eyes while it
carries on working, or take the master override and drive it yourself, cutter
and all. Its mining job goes back on the board while you have the wheel and it
picks the work up again when you hand it back. The world streams around
whichever eyes you are using, so you can drive a drone clear across the map
and watch the whole trip. And the townsfolk can see you now — properly see
you, with the rock in between counting — so they turn to watch, stop their
strolling, greet only someone actually in view, and keep looking where you
*were* for a few seconds after you slip out of sight.

Before that, stage 6: Every world now starts in the **same village**: an authored
town at the origin, identical whatever the seed — plaza, paths, three houses
and a supply shop — blended into whatever terrain the seed grew around it.
Villagers stroll the plaza on deterministic rounds and greet you when you
walk up. Press E at the shop counter to trade for real: sell the ore your
flier ferried home for credits, buy drill-power and cargo upgrades that take
effect immediately and stack on top of your skill levels. Credits and
upgrades persist in their own small save file. The wilderness grew greenery
too — trees (felled whole when a drone cuts any part of one) and grass
tufts, the engine's first non-cube shape.

Before that, stage 5: The world has real relief, meshes across all cores, draws
only what the camera can see, and hides copper ore in the rock with occasional
outcrops breaking the surface. Mark a body and the game works out how to mine
it — a level adit into a hillside, a diagonal decline, or a benched open pit —
and a ground drone cuts the excavation, drives in, digs the ore out and hauls
it to the mine mouth. A flying drone sweeps sectors and drops pings on ore
buried underground, and once you place a container it ferries every pile home
automatically. A fog-of-war minimap paints in as you walk — and as the flier
sweeps — with live dots for you, the drones, the pings and the base.

The machines now look like machines: the ground drone is a squat rust-orange
digger rig with a spinning nose drill and chunky treads, the flier carries a
rotor, and both glide between ticks and turn to face their travel. You hold a
compact boring drill instead of breaking blocks by clicking — hold the button
and it chews through by hardness, faster as your Mining level climbs. Skills
(Mining, Prospecting, Logistics) level RuneScape-style: prospecting deepens
the flier's scanner, logistics grows every cargo hold, and it is all drawn on
a bitmap-font HUD with XP bars and a level-up shout. You can walk on it,
build in it, and it survives quitting — skills included.

| Crate | What it does | State |
|---|---|---|
| `vx-core` | Block registry, coordinate spaces, event bus | Done |
| `vx-world` | Chunk storage, worldgen, ore, town lattice, flora, raycast, line of sight, physics, editing, content hashes, saves | Done |
| `vx-mesh` | Greedy meshing + crossed-quad plants, packed into 8-byte quads | Done |
| `vx-render` | wgpu renderer, camera, frustum culling, instanced objects, 2D overlays, bitmap font, offscreen capture | Done |
| `vx-platform` | Input state, XDG paths | Done |
| `vx-app` | Window, walk/fly/third-person camera, tick-based player movement, streaming, day/night clock, HUD, rigs, skills, villagers, awareness, shop, wallet, garage, handheld, beacon board, town economy, maps, command journal, `gamingg` binary | Done |
| `vx-agent` | Job board, flow fields, mine planning, scanner, flier + fleet, manual piloting | Done |
| `vx-mod-api` / `vx-mod` | Mod ABI, manifests, WASM host | later |
| `vx-steam` | Steam Workshop mod source | M4 |

### Known rough edges

- Underground light is column depth, not light transport: a lit shaft does not
  spill light into the gallery beside it, and the lamp casts no shadows. Cheap,
  pure, and wrong in ways that mostly read as atmosphere.
- The visors are full-screen transforms with no battery, no grain and no wear;
  thermal paints every machine warm, including your own cold wreckage.
- A cave mouth on flat ground is a pothole you notice at your feet, not a
  landmark; the mouths that read from a distance are the ones in hillsides.
- Bounty never decays. The town's memory is perfect and permanent, which will
  want a statute of limitations once the warrant chain exists.
- The player cannot hold an office in ordinary play, so the sheriff's override
  is exercised only by the `--sheriff` development flag until the ballot box
  lands. It is built and tested; it is simply not reachable yet.
- Villager sight is a cone about their heading with a close-range exception, and
  the cone width is a guess that has had no real playtesting.
- A drone caught digging somebody's wall on the far side of town is not traced
  back to you: the witness check is on the player's position. Convenient, and
  arguably correct, but it is a hole a patient player could drive through.
- The mantle arc is an interpolation, not an animation: the body slides up a
  ledge rather than climbing it, the same way villagers glide rather than walk.
  Stance poses are the standing rig scaled vertically — the parts are boxes and
  there are no joints to bend.
- Swimming slows you and stops you sprinting, and that is all it does. There is
  nothing to do underwater and no aquatic machine to do it with.
- Slide friction is one constant for every surface, so gravel, metal grating and
  a wet town street all feel the same. Per-block friction is a small registry
  change and a large tuning job.
- No view bob, camera roll on strafe, sprint FOV shift or landing dip. The
  cheapest way to make movement feel good and the fastest way to make people
  motion sick, so they want individual toggles rather than arriving on by
  default.
- Toggle-versus-hold for sprint, crouch and prone is not configurable, and input
  is not remappable. There are no gamepad bindings at all.
- The third-person camera does not swing clear of the body during a slide, so a
  slide into a wall can fill the frame with your own back. An existing rough
  edge, now reachable at higher speed.
- A 2.2 m mantle lets the player leave a hole a ground drone cannot drive out
  of. The planner does not warn about it yet.
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
- Rig parts are lit like everything else but cast no shadows and have no
  animation beyond spin and yaw — no suspension, no articulation. Villagers
  and the third-person player glide rather than walk: no leg animation yet.
- The third-person body is a static pose that yaws with the camera, and the
  camera does not swing round to avoid it — you can stand nose-first against a
  wall and watch your own back fill the frame.
- One machine can be piloted at a time, by design. No squad orders, no
  waypoint autopilot, no saved view or pilot state: quitting returns you to
  first person with nothing driven.
- Villager awareness is sight only — no hearing, and nothing reacts to a
  machine cutting rock next to it beyond turning to look.
- Villagers are not persisted — their deterministic stroll restarts each
  run. Cosmetic only, since nothing about them accumulates yet.
- A town ignores its surroundings beyond height blending and the sea/slope
  siting gates: a plateau can still abut open water, and no road leads from one
  town to the next.
- Only the town you are standing in has people in it. Distant towns are
  architecture until you arrive — deliberate, to keep the frame budget flat
  however many exist, but it means nothing happens in a town you are not in.
- **The town books are not covered by `--replay`.** The network's reach follows
  the player, and where the player stood is not in the journal, so a replay
  rebuilds the ground but not the markets. Journalling the network's windows
  would close it and is not done.
- The economy only runs for towns within radio range of the player. That is the
  level-of-detail — the simulation is as wide as it needs to be — but it does
  mean a far corner of the frontier is frozen until somebody goes near it.
- Routing is greedy nearest-deficit matching rather than a real min-cost flow.
  Fine at a few dozen towns; revisit if the traffic disappoints.
- One load in the air at a time per dispatch window, and no contract can fail,
  expire, or be robbed.
- Villagers do not react to their town's fortunes — a boom town and a starving
  one look identical on the ground.
- Nothing can be intercepted and nothing shoots: caravans pass overhead
  untouchable. Weapons, interception and bounty are stage 10b, designed in
  ROADMAP.
- The starter flier is free and unlosable, so you can always scan and ferry even
  with an empty garage and no credits.
- A damaged `garage.dat` costs you your whole bought fleet. It is the harshest
  of the tolerant loaders, and the alternative — refusing to open the world — is
  worse.
- The console's trade map picks its zoom to fit the route, so a very long run
  draws a very coarse map.
- There is no artificial light. Night is genuinely dark outside, and a lamp
  block is the obvious next thing the sun uniform wants.
- Leaves do not decay when a trunk is felled by the player's drill (drones
  fell whole trees; the handheld drill still breaks one block at a time).
  Felled wood has nowhere to go but the pile until stage 9's crafting.
- The viewmodel drill is drawn in world space at a camera offset, so it can
  clip into a wall you stand against. A real fix renders it in a separate
  near-scaled pass; cosmetic until then.
- Skill effects apply on the next frame's `apply_skills`, so a capacity raise
  reaches existing drones mid-run. Deliberate — retroactive upgrades feel
  good — but it means capacity is not per-drone state anywhere.
- A drone's grade shapes the ramps it cuts but is not yet a limit on what it can
  drive: the flow field allows one block of climb per step, so a 1:1 staircase is
  traversable by anything. Making grade a real constraint needs a field that
  tracks how far a drone has run since it last climbed.
- The command journal records world edits and dispatches, not the whole session:
  the fleet, surveys, the base and piloting are not in it yet, so `--replay`
  rebuilds the *ground* rather than the entire game state.
- Region files are still written on every save, so the journal does not yet make
  saves smaller — it earns its keep as a determinism oracle first. Letting it
  replace region writes is where the disk win lives, and the keyframe machinery
  is in place for it.
- Replay is deterministic on the same binary and platform. Agent movement uses
  `f32`, so **cross-platform replay is not promised** and should not be claimed.
- Chunk pinning holds an operation's whole working span resident for the life of
  the job. Correct, but a very large marked area pins a lot of chunks.
- A running excavation is not saved, and neither is the fleet. The holes are —
  block edits go through the same path a player's do — but quitting mid-dig
  loses the drone, the job board, the surveys and the base declaration. The
  journal is the obvious way to fix this and does not do it yet.
- Chunk geometry is chunk-local now, but nothing yet *rebases* the origin as the
  player travels, so the camera still walks into `f32` trouble eventually. The
  seam is open; the rebase is not written.
- Each chunk carries its own small origin uniform and bind group. Fine at a few
  hundred chunks; a single buffer with dynamic offsets is the tidier answer if
  view distances grow.
- The minimap draws explored-but-unloaded ground from the generator, so your
  edits and mine holes only show on it while their chunks are loaded. The
  trade is that the map stores nothing but the explored set.
- One flier. The fleet is shaped for more; nothing exercises it.
- No combat, health or hostiles. Piloting and NPC senses are the
  foundation they will stand on; neither is exercised by anything yet.
- No inventory, no audio, and no gamepad support.

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

**The scanner samples real blocks, not the deposit function.** For each surface
column it walks up to 24 blocks down (deeper as Prospecting levels), then clusters adjacent
hits into one ping per body. Reading the pure deposit lattice would have been
cheaper and always right about generated terrain — and wrong about everything
else: a mined-out body must honestly stop pinging, and player-placed ore must
ping. Depth-0 pings are outcrops anyone can eyeball; the depth-1-to-24 band is
what the scanner is for, and in practice that band matters most on gentle
terrain — in steep country shallow bodies usually breach a hillside somewhere,
so your eyes really are nearly as good as the instrument there.

**The flier's movement is trivial by design.** It flies at a fixed clearance
over whichever column it is over, one block per tick horizontally, climbing
first before entering a column whose ground is too close — so it is never
inside terrain, and a cliff costs climb time instead of a collision. No flow
fields, no reachability: not needing them is the fantasy of the flier, and it
is also what makes it nearly free to simulate.

**The overlay draws only when set.** The minimap goes through a generic 2D
pass — any RGBA image at any screen rectangle, no depth test — and headless
captures and the culling tests never set one, so every pixel-equality
guarantee stands byte-for-byte. The same pass is deliberately the first brick
of the terminal screen and anything else that is a flat picture.

**One owner for where the camera sits.** `Camera` has no look target — the
view is built from position plus yaw/pitch alone — so third person needed no
renderer change at all, only a different position chosen after movement. The
controllers own orientation and the body; `view::camera_placement` owns
placement, which is why first person, over-the-shoulder and a drone's feed
differ in exactly one place instead of being threaded through every
controller. The pull-in casts back along `-forward` and clamps to a minimum:
a ray starting inside a solid reports distance zero, and without the floor the
camera would collapse onto the player's own head.

**A piloted machine can do nothing an autonomous one could not.** The override
is a change of driver, not of physics. Held keys become a `PilotCommand`
sampled once per frame and re-issued on every simulation tick, so a driven
drone moves one cell per tick obeying the same standability and one-block-step
rule the flow field enforces, falls when it cuts its own floor, cannot reach
through rock, and cannot undermine anywhere the planner would refuse to. A
piloted flier still climbs before it enters a column. Break that and "every
route in is a route out" — the invariant every mine plan rests on — stops
being true the moment a player touches a machine.

**Jobs are shared; surveys are not.** Taking a drone *releases* its claimed job
back to the board, so another drone can pick the work up while you joyride, and
hand-back needs no bookkeeping at all — the drone goes idle and claims lazily
on its next tick. A flier's survey lives only in that flier, with no board to
hand it back to, so taking one **stashes** its state and restores it on
hand-back instead. Releasing it the way a job is released would silently throw
the whole sweep away.

**The eye and the body come from one interpolation.** Machines move a whole
cell per tick and are drawn gliding between ticks; the FPV camera reads the
*same* interpolated point the rig is drawn at, lifted to eye height, so the
view can never drift or stutter against the hull it is bolted to.

**Piloting far away unloads your own ground.** Streaming follows the camera,
which on a feed is the machine — that is what makes driving across the map
work, and it needed no new code. The consequence needed care: `unload_beyond`
drops the player's own chunks while they are away, so hanging up does *not*
resume physics until `world.is_loaded` says their ground is back. Without it
the first step after a long-range flight drops the body through a world that
has not arrived yet. The HUD says `RECONNECTING` while it waits.

**Exploration is earned by being somewhere.** Driving a drone around does not
paint the fog-of-war map. A remote camera is not the player, and letting a
driven digger reveal terrain would quietly take the scouting job away from the
flier, whose swept sectors still count.

**NPC memory is the feature, not an optimisation.** An observer that forgets
the instant line of sight breaks snaps its head away the moment you step behind
a tree. One that keeps watching where you *were* for a few seconds reads as
somebody who saw something — and it is the shape a hostile will need to hunt
rather than twitch. The cost control falls out of the same mechanism: one
villager re-casts per update on a round robin keyed to an update counter (not
wall time, so the bit-identical determinism survives), and between turns they
face the remembered spot. Three villagers today and thirty later cost the same.

**The hometown is a drawing; the frontier is a dice roll.** `town::home_site()`
returns before any hashing happens, so the starting town is seed-*independent*
by construction rather than merely seed-stable — same plot, same name, same
plan, every world. Every other town comes off a jittered 512-block lattice with
one splitmix64 stream per property, the same idiom ore deposits and trees
already use. Buildings are origin-relative ASCII layer blueprints, so one
static set of plans stamps at any site; they are never saved, because they
regenerate.

**The height field must not feed itself.** `height_at` used to *be* the village
override — its last line blended the plateau in. With one town that is fine.
With a lattice of them, siting town N+1 would measure ground that town N had
already flattened, and the world would depend on the order towns were
considered in. So the field is split: `natural_height_at` is the seed's own
terrain and the only height a siting test may consult, and `height_at` gathers
the towns that could reach a column and blends their plateaus on top. Sites are
gathered **once per chunk** and threaded through column filling, flora and
stamping; a test asserts siting never reads the blended field.

**Town separation falls out of the constants, not out of a rejection pass.**
Jitter is clamped to the middle half of each 512-block cell, so two neighbouring
towns are at least 256 blocks apart, against a maximum reach of 58. No
cross-cell comparison is needed, which is what lets a site be answered from one
cell's hash alone — and that in turn is what makes enumerating three kilometres
of frontier arithmetic instead of a search.

**Nothing about the frontier is pre-generated.** Because worldgen is pure in the
seed, "where are the towns within two kilometres?" is a few hundred hashes and
loads no chunks, no disk, no memory. A beacon can post a job at a town that does
not exist as a single block; walk over and it builds itself exactly where the
posting said. The only thing *stored* is which towns the player has actually
stood in — player knowledge, in its own tolerant file, not world truth.

**A map marker needs no visibility check, and that is the hook.** Markers are
stamped in a pass after the terrain, with no test against the explored set, so
a contract pin draws over blacked-out ground for free. Accepting a delivery
pins a town you have never seen and the black around it stays black until you
walk there.

**The sun is pushed, never read.** Time of day is a uniform at
`@group(2) @binding(0)` holding direction, sky and light, written through
`Renderer::set_sun` beside `update_camera`. No render path reads a clock, which
is what keeps headless captures byte-identical and the culling pixel tests
honest — and it is why `--time`, `--dawn` and `--night` can light a capture
exactly. The fog colour *is* the sky colour, one value, so the horizon cannot
disagree with the clear colour the way two matching constants eventually would.

**The clock is an input to the town, not a global.** `Villagers::update` takes
the hour as an argument, so the bit-identical determinism test still passes a
fixed noon while sundown genuinely sends everyone indoors.

**Vegetation is not ground, and machines know it.** Trees are solid blocks,
so the planners had to learn that a canopy is not a hilltop: `ground_height`
walks down through vegetation, or a pit surveys its rim six blocks up in the
branches. And a crown hanging into a bench from a tree rooted elsewhere has
no cell to stand and prune it from — so cutting *any* part of a tree fells
the connected whole into the cargo bed (through the same cancellable event
path as every edit), and standing vegetation never counts as outstanding
work. Take the trunk, take the tree.

**The cross shape is the engine's first non-cube.** A grass tuft is two
diagonal quads, each emitted twice with reversed winding because the terrain
pipeline culls back faces — a plant must carry its own back or vanish from
half the compass. Cross blocks never merge into the greedy sweep and never
cull a neighbour's face; they are non-solid, so you walk through them and
the raycast ignores them.

**Selling drains the fleet's base pile.** The shop buys whatever the flier
ferried home, which closes the loop the whole game is about: scan → mine →
ferry → sell → upgrade → dig deeper. There is deliberately no separate
player inventory yet (stage 7); the pile *is* the wealth. Prices and
upgrade lines are name-keyed tables, so the shelf grows by adding rows.

**Bought upgrades multiply on top of skill effects.** `wallet.dat` is its
own small file beside `player.dat` (one concern per file, no migrations,
same tolerant loader — a corrupt wallet is an empty wallet, never a failed
world). A fresh wallet is the exact identity, and upgrades reach machines
already in the field on the next frame. Retroactive upgrades feel good;
that is deliberate. The shop counter itself has no hardness: you cannot
drill the economy out of the town.

**Villagers are deterministic clockwork.** Waypoints and pauses come from
hashing the villager's index and stroll leg — no RNG state, nothing saved,
two runs fed the same frame times are bit-identical. Greetings use
hysteresis (speak at 3 m, re-arm at 5 m), so walking up gets one line, not
a line per frame.

**The font is data, the HUD is pixels.** Text is a hand-set 5×7 bitmap —
A–Z, digits, punctuation, seven bytes a glyph — stamped into any RGBA buffer.
The HUD composites a small panel on the CPU exactly the way the minimap does
and ships it through a second overlay slot; the renderer never learns what a
glyph is. The same font is the terminal screen's and the shop's, later.

**A rig is data too: a handful of cuboids.** Machines are parts — centre,
size, tile, maybe a spin axis — turned into instanced objects each frame, so
they ride the existing one-draw-call path and per-object frustum culling with
zero new render code. The digger reads Motherload-ish on purpose: squat
rust-orange hull, pale cab, chunky treads, tapered steel drill that spins
while she cuts. The *vibe* is borrowed; every pixel is procedural and ours.
Rigs face +X and yaw to their travel; positions interpolate between
simulation ticks so machines glide instead of teleporting block to block.
The inverse-transpose normal fix from the review round is what keeps these
rotated, stretched parts lit correctly — this is the consumer it was fixed
for.

**The player's drill is the pickaxe role without the pickaxe.** Hold to dig:
progress accrues at `drill_power / hardness` per second against the block
under the crosshair, resets when you look away, and the break at 100% goes
through the same cancellable event path as a click ever did — a mod's veto
still holds. It is an original *compact boring drill*: the tunnel-company
name on the real one is a live trademark, so ours has none. Bedrock has no
hardness and stays undrillable.

**Skills are name-keyed entries, not an enum.** `"mining"`, `"prospecting"`,
`"logistics"` today; the Skyrim/Fallout-breadth combat and faction skills
planned for the village era are *rows in the same table*, not code changes —
the same reasoning as namespaced block names. The XP curve is the classic
each-level-costs-~10%-more shape with our own constants, levels 1–99,
precomputed once. Effects are small pure functions: Mining speeds the hand
drill, Prospecting deepens the flier's scan, Logistics grows every cargo
hold. `player.dat` persists XP by name and a corrupt file logs and starts
fresh — progress must never take the world down with it.

**The map's only state is the explored set.** Worldgen is a pure function of
`(seed, position)`, so explored-but-unloaded terrain is recomputed from the
height field on demand rather than cached as thumbnails. `explored.dat` is a
list of chunk coordinates and nothing else, and a corrupt one logs and starts
empty — player knowledge must never take the world down with it.

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

Every building in a town carries a lockbox saying who may edit it, ranked
sheriff over mayor over owner over guest. Your own house is yours; the streets
are the town's; past the town line you build where you like. Getting into
somebody else's means earning it, picking the lock, or drilling the box out —
and any of it only costs you a bounty if somebody actually sees you, which is
what crouching and going prone are for.

New players wake up inside their own house in the starting village, with a
storage chest, a mailbox for mail orders, and a one-time welcome panel whose
changelog is parsed straight out of `ROADMAP.md` (`--changelog` prints it).
The spawn area is pregenerated before the first frame and the hometown is held
resident permanently; `--view-distance <n>` (4–16) picks the streaming radius.

### A build you can just run

`dist/` holds a stripped release binary for x86-64 Linux, for testing on a Steam
Deck without a toolchain. One file, no assets, no installer — the shaders are
compiled in. It needs **glibc 2.39 or newer** (SteamOS 3.7+ is fine, 3.6 is not);
`dist/README.md` has the check and the rest of the Deck notes.

```sh
./dist/gamingg-linux-x86_64
```

Controls:

| Input | Action |
|---|---|
| `WASD` | Move |
| `Space` | Jump (walk) / rise (fly). Held into a ledge above waist height, mantle it |
| `Left Shift` | Crouch (walk) / descend (fly) |
| `Left Ctrl` | Sprint |
| `Left Ctrl` + `Left Shift` | Slide, from a sprint. A slide-jump keeps the speed it built |
| `Z` | Go prone |
| — | Vaulting a waist-high ledge is automatic; you do not press anything |
| Click | Capture the mouse |
| Hold left button | Run the drill — harder rock takes longer |
| Right click | Place the selected block |
| `1`–`6` | Choose stone, dirt, grass, sand, the base container or your chest |
| `8` | Choose the fabricator, to place it (buy one at the counter first) |
| `7` | Take out or sling the slug launcher (once you own one) |
| Hold left button | (launcher out) Fire — slow, heavy rounds on a visible arc |
| `C` | Toggle first person and over-the-shoulder |
| `V` | Open the handheld fleet uplink (arrows pick) |
| `Tab` | (in the handheld) Turn the page: fleet roster, map, kestrel command |
| `Enter` | (on the kestrel page) Give the selected standing order — orbit, sortie, perch, vanguard, dock — or, when the machine is standing at a lock or the town's watch box, set the coil on it |
| `Enter` | (in the uplink) Look through the selected machine |
| `R` | Take or release the master override of what you are watching |
| `Escape` | (on a feed) Hang up and return to your body |
| `E` | Trade at the shop counter: sell at that town's prices, buy drones and fliers, or order goods by mail from another town (arrows pick, `Enter` trades, `E` leaves) |
| `E` | Open your chest at home (arrows pick, `Enter` moves goods to the base pile), or collect the mailbox outside |
| `E` | Read a lockbox: who owns the building, where you stand with them, and the grade of the lock |
| `Enter` | (at a lockbox you have no right to) Hold to pick it. Needs the Security skill; you stand still and exposed while it runs |
| `Z` / `Left Shift` | Go prone or crouch — cover and a low profile are what stop anyone seeing what you are up to |
| `E` | Read the beacon console at the foot of a radio mast — take work, or sign for a delivery (arrows pick, `Enter` acts, `E` leaves) |
| `M` | Mark a corner of an ore body (two marks make an area) |
| `Tab` | Cycle the proposed mining method |
| `Enter` | Send a drone to dig it |
| `Backspace` | Cancel the marked plan, releasing the ground it held |
| `G` | Send the flier to scan the sector you are standing in |
| `N` | Toggle the minimap |
| `[` / `]` | Zoom the minimap out / in |
| `L` | Turn the optics dial: lamp, then any printed visor, then off |
| `F` | Toggle walking and flying |
| `F5` | Save |
| `F10` | The gold panel — the operator's console (dev builds only, see below) |
| `Escape` | Release the mouse |

Worlds are saved on quit to `$XDG_DATA_HOME/gamingg/saves/world`, selectable
with `--world <name>`. Reloading an existing world uses its stored seed, so
`--seed` only applies when creating a new one.

### The arsenal

The shop counter sells a compact slug launcher and boxes of slugs. It is
heavy (it rides your pack weight), loud (every sound in the game is
synthesized — a machine with no audio device simply plays nothing), and the
town treats it accordingly. The first shot inside a town's line gets you one
warning, once, per town. After that the rules are the rules: property you
break costs bounty scaled by how many people saw it; pointing the muzzle at
somebody panics them — each one either runs home or runs for the security
office, and an alarm that reaches the office is a signed report against you.
Caravans can be shot down mid-flight: the cargo drops where it fell and can
be salvaged, and the network logs the loss against your name whether or not
anyone watched — the manifest is its own witness. Every shot is journalled
with its muzzle and aim, so a firefight replays crater-for-crater under
`--replay`.

### The kestrel and the roost

The shop sells a palm-sized scout drone that rides your pack. It flies on a
small cell — endurance and cooldown are one budget, and the recharge costs
what the flight spent — and takes standing orders from the handheld's third
page: circle overhead, fly a sortie where you're looking, perch as a sentry
(a quarter of the drain), hold ahead of you, or come home. What it sees gets
a mark: a report of where a person or machine *was*, on both maps and over
the spot itself, fading after thirty seconds unless re-sighted. It reveals
contacts, never terrain — cartography stays the flier's paid trade.

The same machine sits in a box on the security office roof. Breach a lock or
fire a shot in town and it pops out, flies to the noise, and watches. The
first time it sees you, you are *observed* — the drone overhead is the
warning. Anything you do while observed is witnessed, on the same bounty
arithmetic as every other pair of eyes. It can be evaded (break its line of
sight until the mark fades), outlasted (its battery dies like yours, and
while it recharges the town is blind), or shot down — which is property
damage in front of the best witness in town. Cover now works in two
directions: walls hide you from people, roofs and canopy hide you from the
sky, and both are plain geometry.

### The fabricator

Everything you break is stock now. Every block yields itself onto the fleet's
base pile — the same pile the flier ferries into and the shop sells out of —
so the drill finally produces something, and a hillside is raw material.

Buy a fabricator at the counter, place it wherever you like (`8` on the belt,
broken to move it), and press `E` at it. It takes raw goods off the pile and
turns them into whatever the catalogue has a row for: slugs for the launcher,
copper bars, planks, metal wall panels, a charged cell that puts the kestrel
back in the air immediately, a spoofer coil, and — at the top of the ladder —
a kestrel or a ground drone. Buying a machine with credits and printing one
out of copper are two routes to the same machine: the counter is for people
with money, the fabricator is for people with a mine.

Materials come off the pile the moment you start, so a print is a decision
rather than a queue, and the Fabrication skill buys speed and unlocks the
harder patterns. Nothing here costs credits.

### The world below

The terrain used to be a height field: one surface height per column, solid
rock all the way down. Caves are the first genuinely three-dimensional thing
worldgen has ever done — winding tunnel galleries where two 3D noise fields
run near zero together, and rarer chambers deeper down, all of it a pure
function of the seed and the block position so chunks still generate in
parallel and a saved world still regenerates identically.

Tunnels thin as they approach the surface, so mouths exist — a hole in a
hillside you can walk into — but stay scarce enough to be worth marking on
the map. Nothing opens under a town, nothing opens into the sea, and the
bedrock floor keeps a margin of rock below the deepest gallery. Where a
tunnel cuts through an ore body it carves the vein with it, which leaves
copper showing in the cut faces: a cave is where hand-mining is pleasant,
ore at the surface of a wall instead of under twenty blocks of overburden.

Drones already cope: a machine over a void settles to the cave floor, and a
mine plan that meets a cavity simply finds part of its digging already done.

### Lights in the dark

The world below is genuinely dark now. Every face bakes how much sky its
column can see, so daylight dies a few blocks under a roof and is gone twenty
under rock — house interiors read as shade, caves as night. Machines and
people darken with the ground they stand on.

Against that: the suit's hand lamp, on `L` — a warm cone thrown from wherever
you are looking, short and honest. Everything better comes off the
fabricator, not the counter: a **high beam** that throws nearly twice as far,
a **night vision visor** that amplifies what little light there is into green
geometry, and a **thermal visor** that ignores light entirely and paints
warm machinery and warm bodies against cold rock. One key cycles the dial
through what you own; the HUD names what you are looking through.

### Hacking through machines

Once Security 10, the counter stocks spoofer coils. Fitted to a machine, a
coil lets that machine do the lock work while you stand somewhere else: a
light coil rides the kestrel and opens houses and shops, a heavy one rides a
real airframe and opens anything. The Security floors from the permits round
still apply — hardware says *where* the work can happen, your skill says
*what* work is possible, and neither substitutes for the other.

What you have bought is distance from the scene, never from the consequence.
The machine at the lock is watched by the same eyes a body would be, and if
it is spotted the bounty lands on your name. Worse: a machine working
unattended gets seized, and signing it out of the pound costs a fee at the
counter. One you are personally flying can at least be flown away. And the
link is a leash — stay within 120 m of your machine and the machine within
reach of the target, or the job stops where it stands.

The town's watch box is a target too, and there are three things to do to
it. **Blind** it and it stands down — though the town notices a dark box
soonest. **Silence** it and it flies its patrols, sees everything and files
nothing, which nobody notices until an offence goes strangely unpunished.
**Tap** it and its sightings mirror onto your handheld: the sheriff's eyes,
exactly, never better ones. Each grade wants more Security than the last, and
the tap needs a heavy coil. If subtlety is not your line, the box is still a
block — drill it out and the town is blind until it is re-boxed, at the same
price every other loud thing costs.

You can buy the same box for your own roof, at the price the sheriff paid. It
does not fly, because it has nothing to answer; it just watches your yard and
files what it sees to you.

### The gold panel

Development builds carry an operator's console behind the repo's first cargo
feature, `gold` (on by default; the `dist/` build is compiled with
`--no-default-features` and does not contain it). `F10` opens it, or launch
with `--gold` to start with it open. Five tabs — Player, Spawn, Town, World,
Tuning — driven Deck-shaped: `Tab` cycles tabs, arrows move the cursor and
adjust a held slider, `Enter` acts and commits, `X` resets a tunable to its
shipped default. Every mutation it makes is an ordinary journaled order
(`Command::Admin`), so a cheated session still replays to the same world hash
— a cheat is an order like any other. It is a keyboard console today; the
gamepad backend waits for real hardware.

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

# same, but framed close on the digger rig itself
cargo run --release -p vx-app -- --screenshot rig.ppm --at 146,30 --dig auto --close

# the starting village (identical in every world), villagers included
cargo run --release -p vx-app -- --screenshot village.ppm --at 0,0

# the same view with the supply shop panel open over it
cargo run --release -p vx-app -- --screenshot shop.ppm --at 0,4 --shop

# over the shoulder, with the player's body in shot
cargo run --release -p vx-app -- --screenshot third.ppm --at 0,10 --third-person

# the container town at dawn, mast against the sky (--night, --noon, --dusk too)
cargo run --release -p vx-app -- --screenshot town.ppm --at 0,22 --dawn

# from inside the roomiest cave pocket near a spot, facing down the gallery
cargo run --release -p vx-app -- --screenshot cave.ppm --at -244,-14 --cave

# the same gallery by hand lamp, night vision or thermal
cargo run --release -p vx-app -- --screenshot lamp.ppm --at -244,-14 --cave --optic lamp
cargo run --release -p vx-app -- --screenshot nvg.ppm --at -244,-14 --cave --optic nvg
cargo run --release -p vx-app -- --screenshot heat.ppm --at 0,8 --time 0.95 --optic thermal

# a fabricator standing in frame with its panel open mid-print
cargo run --release -p vx-app -- --screenshot fab.ppm --at 13,9 --fab

# the beacon console, work posted and one contract already taken
cargo run --release -p vx-app -- --screenshot board.ppm --at 0,4 --board

# the console with its trade map: a pin in the dark and a bearing under it
cargo run --release -p vx-app -- --screenshot console.ppm --at 0,4 --board --height 620

# the handheld's map page
cargo run --release -p vx-app -- --screenshot handheld.ppm --at 0,10 --handheld-map

# the beacon console, now with the town's prices and what it is short of
cargo run --release -p vx-app -- --screenshot market.ppm --at 0,4 --board

# rebuild a saved world from its journal and check it against the ground on disk
cargo run --release -p vx-app -- --replay --world myworld

# put a bigger crew on the next dispatch
cargo run --release -p vx-app -- --drones 8

# the handheld's fleet roster and a live feed banner
cargo run --release -p vx-app -- --screenshot uplink.ppm --at 0,10 --device
```

`--dig` runs a whole excavation headlessly against generated terrain — then
sweeps the surrounding sector with the flier and draws the pings, the aircraft
and the minimap into the frame. It is both a screenshot tool and the fastest
way to see whether a change still produces a mine a drone can drive and a
survey the scanner can fly.

The render tests do the same thing and assert on the pixels. Both run against a
software Vulkan driver, so no GPU is required:

```sh
sudo apt-get install mesa-vulkan-drivers
VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json cargo test --workspace
```

Tests skip themselves rather than failing when no Vulkan adapter exists at all.

## Licence

MIT OR Apache-2.0.
