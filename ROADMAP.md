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
| 10c | `67aa3a6` | Pregenerated spawn, streaming off the frame thread, the player's house, mail-order, the welcome panel |
| 11 | `289a143` | Permits: ranked claims, three grades of lockbox, witnesses and sneaking, bounty, breaching and lock-picking |
| 12 | `bbe6756` | The gold panel: journaled admin orders, live tuning, the operator's console compiled out of shipped builds |
| 13 | `4f22cd4` | The arsenal: the slug launcher, synthesized sound, recoil and shake, town warnings, witnessed property bounty, panic, caravan interception |
| 14 | `6b56c46` | The kestrel and the roost: a pack scout with standing orders and decaying contact marks, and the town's own watcher on the security office roof |
| 15 | `7e8d4f1` | Hacking through machines: spoofer coils, drone-borne intrusion on a leash, the watch box blinded, silenced or tapped, the impound, and a watch box for your own roof |
| 16 | `75bbb1b` | The fabricator: every block is stock, and a printer that turns raw material into ammunition, cells, building goods, modules and whole machines |
| 17 | `1643f59` | Caves: the first true 3D carve — tunnel galleries and deep chambers, pure in the seed, mouths in hillsides, ore showing in the cut faces |
| 18 | `8bbc730` | Lights in the dark: baked skylight makes the world below genuinely black, the suit's hand lamp cuts a warm cone through it, and the fabricator prints a high beam, night vision and thermal |
| 19 | `94f1ed1` | Bunkers: sacred-geometry layouts on their own lattice — three proportion systems, golden BSP on a Fibonacci vocabulary, golden-angle bearings, a 400-hardness shell, and supply caches to strip |
| 20 | `60b53e8` | The fuel loop: the fleet burns oxyhydrogen, an electrolyser on a shore splits water into it, and HHO joins the trade network as the first shortage that can stop you |
| 21 | `38024dc` | Star forts and banks: bastioned traces per town tier with gates, ditches and deterministic breaches, a strongroom in every town behind the first Tier Three lock ever stamped, and foundations under everything |
| 22 | `9148a77` | The terminal: the font's third user — typed commands, a caret and history, and four hundred lines of scrollback the toasts also land in |
| 23 | `58bc6ac` | The townsfolk: names, trades and temperaments per person, pure schedules with market days, a friendship ledger with gift tables and tier unlocks, and speech templated over the live simulation |
| 24 | `89aa6b3` | The controller: native gamepad play synthesized into the keyboard and mouse seams, a SELECT-key control scheme overlay, and a one-command Steam Deck installer |
| 25 | `c235ac7` | The workshop: upgrades printable in materials as well as bought in credits, three new lines (pack, press, lamp), and the rule that an upgrade may not change arithmetic the journal re-runs |
| 26 | `23d8bea` | Wear and recovery: machines age by the tick they work, the worst one sets the crew's pace, and spare parts printed at the workshop mend them — the first machine state that had to live inside the replayed simulation |
| 27 | `6a2eeef` | Micro-on-damage: a block gains a 4³ interior only when violence touches it, one `u64` a wound, so walls chew where they are shot, rays pass through the holes and feet never do |
| 28 | _this_ | Hostiles and health: a warrant sends deputies who believe rather than know, search an occupancy map, take cover, never fire through each other, and break or surrender on nerve |

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

## Shipped — Stage 11: permits

You could walk up to anybody's house and pull the walls off, and not one soul
said a word. This round the town grew a spine — and three ways past a locked
door, priced so the honest one is the best one.

**The bus finally has a subscriber.** `break_block` and `place_block` have
emitted cancellable events since stage 2.x with the module doc promising "mods
will, without any of these call sites changing". Eight stages later, that is
exactly what happened: the permission gate hooks the events, so the player's
drill, a drone's cutter and anything added later are all covered, and not one
caller changed.

**No actor identity was needed, and that was the finding.** The obvious design
threads an `Actor` through `break_block`'s seven call sites. It turned out the
only distinction that matters is live-versus-replay, and that already existed
structurally: `journal::replay` is handed a fresh `EventBus` by every caller, so
the oracle never sees the gate. `the_replay_oracle_runs_without_the_gate` is the
tripwire for anyone who later "helpfully" passes the live bus in.

**Claims are derived, never stored.** A town's buildings are a pure function of
its site, so `plan::buildings` gives every building a role and an AABB — one
below the floor and one above the roof, because a claim you can tunnel under is
not a claim. What is *stored* is only what cannot be derived: who has been let
in, who holds office, what you have been caught doing, and how far through a
lock you have drilled.

The ranking is **Sheriff > Mayor > owner > guest > everyone**. The sheriff opens
anything, because a lock the law cannot pass is a lock that makes crime safe.
The mayor is not an override — they own the streets, the paving, the tower and
every scrap of open ground inside the town line, which is status, not a skeleton
key. Outside the town line nothing is claimed and you build where you like.

**Stealth was already in the engine; nothing was reading it.**
`awareness::PLAYER_EYE` was a flat 1.62 and the villagers' sight ray aimed there
no matter what the player was doing. It now carries the player's *live stance*
eye height — 0.35 m prone — so going flat behind a one-block wall is occluded by
the raycast that was already running. No visibility stat, no detection meter:
stealth is geometry. `Stance::Prone`, which the stage 10b notes flagged as dead
weight, now has a job. Villager sight also gained a cone about the heading they
already track, because before this there was no *behind* to get to.

**Seen is the rule.** Prying at a claimed wall, picking a lock, smashing one —
all of it costs bounty only if somebody can actually see you, checked with a
fresh cast at the moment of the crime rather than the round-robin cache, which
is several frames stale. The HUD grew an eye, because without it the stealth
rules are invisible mechanics and the only teacher is being arrested.

**Three ways through a door.** Locks come in three grades and are deliberately
*breakable* — a lock you cannot attack is a wall with extra steps.
- **Authenticate**: free, permanent, no risk. Trust through trade is stage 12;
  this round only the owner qualifies.
- **Pick it**: needs the new `SECURITY` skill, leaves the lock standing so
  nobody need ever know. Deterministic — no roll — because what levelling buys
  is *speed*, and the real cost is standing there exposed while it runs.
- **Drill it**: each grade carries a hard `min_power` floor as well as a
  hardness, because "impossible for a new player" cannot be said in seconds
  (drilling is `hardness / power`, so any hardness eventually gives). Breach
  progress *persists* where ordinary drilling resets on a wobble — a breach is a
  project you come back to; a pick is not, and that asymmetry is what makes
  choosing between loud and quiet a decision.

Breaking a lock leaves the building open until the town puts a new box up. The
rebuild is recorded as an ordinary `Command::Place`, so a replay puts the lock
back at the same tick — a world edit the journal cannot see is a world edit that
makes the oracle lie.

**Stated plainly rather than hidden:** the player holds no office in ordinary
play, so the sheriff override is built and tested but exercised only through the
`--sheriff` development flag until stage 13's ballot box. And with town land
claimed, your chest and your base container can no longer go anywhere in town
except inside your own house — the toast says so rather than leaving you jabbing
at the dirt.

Journal VERSION 5: the lockboxes and the security office changed generated
ground, so an older log would replay against terrain that no longer generates.

---

## Shipped — Stage 12: the gold panel

The operator's console: spawn, tweak, inspect — a Garry's-Mod-shaped surface
built without breaking the one thing this codebase cannot afford to break.

**A cheat is an order like any other.** The panel owns no mutation path. Every
button resolves to a `Command::Admin(..)` in the same journal as movement and
dispatches — Give, SpawnMachine, Teleport, SetStat, SetStock, SetTuning — so a
session full of cheats still replays to a hash. Set up a situation through the
panel and the journal *is* that scenario: committed, a regression fixture;
handed over, a perfect reproduction case. Replay applies what `Rebuilt`
carries (the player, the tuning, the base pile); machines and town books are
no-ops on replay, the same honest line `run_replay` has always drawn for the
economy.

**Tuning constants are orders too — that was the subtle one.** Dragging
`friction_slide` feels like editing a file, but it changes how every later
command is *interpreted*; replayed under different constants, the same log
diverges. So the movement tranche moved into a name-keyed `Tuning` struct
riding on `Movement` (the shipped constants are its defaults), every slider
commit records `SetTuning { key, value }` — the f32 crossing the wire as raw
bits, exact — and a journal now carries its physics with it. Proven by test:
a mid-log tuning change lands the same sprint somewhere else, reproducibly.
Physics constants (`GRAVITY`, `STEP_HEIGHT`…) stay `const`: they cross the
crate boundary, and threading them is a bigger cut than tuning has yet earned.

**Fiction never sees gold — with one honest retrofit.** The gold *hue* was
already the house accent in all seven diegetic panels, so what marks the
console is chrome none of them have: the double gold border. If a capture
shows the border, the session was touched. The panel is the repository's
first cargo feature (`gold`, default-on for dev; the shipped Deck build
compiles it out with `--no-default-features`, verified by the marker string
being absent from the binary), and opens only behind `--gold` + F10.

**Deck-shaped, keyboard-driven.** The focus model is a controller's — tab
cycling, directional focus, one activate button, slider fields, X-to-reset —
but no gamepad backend exists anywhere yet, so the keys stand in (arrows,
Tab, Enter, X). When a gamepad crate lands, with hardware to test it on, it
maps onto this model without the panel changing. Journal VERSION 6.

Deliberately not built: spawning villagers (the roster is hardcoded at three
and rigs index by variant — a real refactor, waiting for a round that needs
it), search in the spawn grid, undo (every order is journalled, so replay-to-
tick-N is sitting right there when someone builds the button).

---

## Shipped — Stage 13: the arsenal, and what robbing costs

The brief was three words — heavy, loud, scary — and a fourth that goes
without saying in this repository: *replayable*.

**Heavy.** The launcher rides the same carried-mass system as ore: owning it
folds a fixed heft into the movement load byte *before* the command is
journalled, so the weight slows the same sprint on both sides of the oracle
without the journal ever learning what a weapon is. Firing queues a recoil
impulse through the exact one-shot slot the slide-entry kick uses — applied
before friction runs, or the stagger would be partly thrown away — and the
aim physically climbs, because the pitch is real input state and rides the
next `Move` command for free.

**Loud.** The repository grew its first audio: `rodio`, playback features
only, because every sound is *synthesized* — a sub-bass sweep under a hard
noise burst, soft-clipped so it reads overdriven rather than polite. There
are no audio assets and there will be none for the base game: nothing to
license, nothing to load, nothing for a mod to be missing. A machine with no
output device gets a silent player and the game does not care. The camera
shakes on a decaying trauma curve, applied to the *pivot* only — in third
person the orbit's wall raycast runs from the shaken anchor, so the kick can
never punch the camera into rock — and the whole felt layer is tunable from
the gold panel (`shake_power`, `slug_kick`, and friends joined the tuning
table, which is exactly why the panel shipped first).

**Scary.** Villagers understand the muzzle. Pointing it at somebody who can
see you — same fresh line-of-sight cast as witnessing — panics them, and each
of them settles it with their own deterministic coin (the same salted hash
streams the wander runs on): *de-escalate*, running home to hide, or
*escalate*, running for the security office. An alarm that reaches the office
is a signed report and costs bounty with no further witnesses needed. Blasts
and impacts panic close bystanders outright — hearing needs no line of sight
— and turn every head in earshot.

**The projectile steps on the journal clock.** The design note said "a sum,
not a simulation"; what shipped is stepped — one integration per journal
tick, fixed step, gravity sag and all — because slugs break blocks, blocks
are in the world hash, and per-tick segment sweeps through `raycast_solid`
were needed for hit detection anyway. Eight visible steps a second is also
what makes *leading* a caravan a skill. The `Fire` order (VERSION 6 → 7)
records the quantised aim **and the muzzle position** — the one deliberate
exception to "intent, not outcome": live movement runs on a real-time 64 Hz
clock and journal ticks on the drones' 8 Hz clock, so a replay-derived muzzle
would stand a few subticks from where the trigger was actually pulled, and a
hash must not depend on which clock you asked. `the firefight replays
crater-for-crater` is a test, not a hope.

**A slug does not ask permission.** Ballistic damage bypasses the cancellable
break event (the gate exists to refuse an edit; a fired slug is past
refusing) and the bill arrives by consequence instead: the first harmless
shot inside a town's line gets one warning, once, per town, and property
broken on somebody's claim costs `damage × (1 + 0.5 × (witnesses − 1))` —
seen is the rule for gunfire exactly as for lockpicks. What a slug can break
is capped between sheet metal and mast steel: buildings are vulnerable, ore
bodies, masts and every grade of lockbox are not, and at shop prices per
round it is the worst drill money can buy.

**Interception.** A slug sweep is tested against every caravan's hull —
`position_at` is pure in the tick, so the test is arithmetic. A downed load
falls where it was hit, waits as a crash site, and is salvaged onto the base
pile by walking up to it. The network bills half the cargo against your name
whether or not anyone watched: the manifest is its own witness, the one
crime in the game that needs no eyes. The destination town simply never
receives the goods, and the shortage working through its price curve is the
town noticing. Shooting down your own delivery is legal, free, and
its own punishment.

### What is deliberately not in 13

Player health, hostile escorts, death, and the rest of the weapon table
(scattergun, mining charge, beam cutter, rail lance, EMP burst, guided
missile — named for what they do, never for anybody's trademark). Ammunition
as a *trade good* with its own market line also waits; the counter sells
slugs for credits meanwhile. Nothing shoots back yet — that is stage 17's
health model — and the bounty contracts the mast should post wait for
somebody to take them.

---

## Shipped — Stage 14: the kestrel and the roost

One machine, two owners — the decision everything else followed from. The
player's scout and the town's watcher are the same airframe with the same
endurance and the same battery problem, so the player learns exactly what the
law can see by owning the thing that sees it, and surveillance stays a
mechanic instead of a punishment.

**The kestrel** rides the pack: bought once at the counter (one per person —
the fleet is for swarms, the pack is for one), tethered by recharge rather
than fuel. Flight and cooldown are one budget — the recharge is proportional
to what the flight spent, and the cell upgrade line shortens it. Its standing
orders are journalled commands (`Command::Scout`, VERSION 7 → 8) set from the
handheld's third page: orbit overhead, sortie where you look, perch as a
sentry at a quarter drain, fly vanguard ahead, dock. Piloting it is the
existing take-control path — `MachineRef::Kestrel` — and its movement is the
flier's, reused whole: same terrain-safe stepping, same never-inside-terrain
rule. It is deliberately **not** in the fleet roster, so `dispatch_scan` can
never grab it and the survey layer stays the paid flier's trade by
construction — the scout reveals *contacts*, never terrain, which is the line
that keeps it from quietly obsoleting the machine you pay for.

**A mark is a report, not a tracking beacon.** Contacts scanned within range
and line of sight get a hovering cube over where they *were* seen and a pin
on both maps, dimming as the report ages and gone after thirty seconds
unsighted. The occlusion raycast that already ran everywhere does the work,
which is what makes a roof — or tree canopy — real cover from above with no
new stealth system: cover now splits into cover from people and cover from
the sky, and the sky kind is geometry.

**The roost** is the same machine in a box on the security office roof (a
fifth blueprint layer — the worldgen half of the version bump). It hears
loud *reports* — a lock breached, a shot fired in town — pops out over three
readable seconds, flies to the noise, and watches. The first time it gets
eyes on you, you are *observed*: the drone overhead, visibly, is the
warning. Crimes committed while observed count it as one more witness in
the arithmetic stage 11 built. It never attacks. And every counter to it is
honest mechanics: break sky line of sight until the mark decays, or bait it
aloft and outlast its endurance — while it recharges in the box the town is
blind, and `the heist window exists` is a test, not a promise.

### What is deliberately not in 14

The homestead's own roost (stage 15, where hardening it against intrusion
has stakes); hacking anything (stage 15); contact classification beyond
person/machine; a second kestrel; per-kind mark decay; kestrel numbers in
the tuning table. Known rough edges, expected: the kestrel's cell resets on
a reload (endurance is not persisted); a perched kestrel under an overhang
sees nothing and says nothing about why; the roost watches but nothing yet
*acts* on what it witnesses beyond the ledger — the sheriff who serves a
warrant is a later tenant.

---

## Shipped — Stage 15: hacking through machines

The Security line's work, done at arm's length — and the town's own machines
made into targets worth doing it to.

**Not a minigame, on purpose.** Stage 11 set the rule that locks are gated by
hard floors, not dice. Moving the work onto a drone changes *who is exposed
and where the operator stands*; it does not change the odds, because there
are none. `refuse` is one pure function taking one `Attempt` — frame, coil,
Security, reach, link, target — so the whole rule set is testable in a line
and identical whether the machine flew there under orders or under your
thumb. Position is the only input, which is the strongest possible form of
"a piloted machine can do nothing an autonomous one could not."

**The tool is a module, the ceiling is the skill.** The counter starts
stocking spoofer gear at Security 10 — well before any of it is useful,
which is the right way round. A light coil rides the kestrel and opens
houses and shops; a heavy coil rides a real airframe and opens anything. The
grade of lock is still gated by the floors stage 11 set, so the shop cannot
sell what practice has not earned. The **hardened link** is on the shelf too:
nothing hacks *you* until factions land, so it is bought early and needed
late — which is exactly how the player's own conduct teaches the rules they
will later be on the wrong end of.

**The machine is exposed; the owner is billed.** A machine at a lock is
watched by the same eyes as a body: `Villagers::watchers_of` is the witness
query pointed at a point in the world rather than at the player, and the
roost counts too. Caught, the bill lands on the name in the garage papers —
and an *unattended* machine is seized where one you are personally flying can
be flown off, which is the honest difference between leaving a tool
somewhere and holding it. The pound wants a flat fee plus a tenth of the
machine's worth, payable at the counter, and it outranks every other row on
the shelf while it is owed.

**Range is the link, and the link is the leash.** The operator must stay
within 120 m of the machine and the machine within reach of the target, and
every condition is re-judged every tick — walk away and the job stops where
it stands. An intrusion is a planned act you stay committed to, not
fire-and-forget, and that number is what stage 25's jammers will exist to
shrink.

**The watch box, three ways.** Blind it (Security 15) and it stands down —
loud in its own way, because the town notices a dark box soonest. Silence it
(35) and it flies its patrols, sees everything, and files nothing; nobody
notices until an offence it should have witnessed goes strangely unpunished.
Tap it (70, heavy coil only) and its sightings mirror onto your handheld
through the same `Marks` the kestrel fills — the same eye, the same radius,
the same occlusion. The tap grants the sheriff's eyes, never better ones, and
it lasts longest because nothing about a tapped box looks wrong. The grades
were re-scaled onto the ladder the locks already use (1 / 20 / 60) rather
than the note's own 3 / 5 / 7, which was written against a scale this game
does not have.

**And the loud way still works.** The box is its own block now, so you can
read it across the plaza; it joins the lockboxes' exemption from the permits
gate for the same reason they have it — it is the attack surface, never the
thing defended — and drilling it out blinds the town until it is re-boxed, at
the breach price if anybody saw. **Your own roof** takes the same box for the
same price the sheriff paid: bought at the counter, mounted with a journalled
`Place`, and it watches your yard without ever flying, because it has nothing
to respond to.

### What is deliberately not in 15

Anything hacking *you* — the hardened link has no adversary until factions.
Targets beyond locks and the watch box: a rival's hauler, a faction digger.
The impound as a *place* — it is a counter and a fee, not a fortified
jailhouse holding your best machine, which is self-writing content
deliberately left unwritten. Unmarked chassis: every machine traces to its
owner, and stolen serials wait for a market to sell them in. And the Security
milestones still gate cleanly while teaching nothing; the skill line needs
its manuals.

---

## Shipped — Stage 16: the fabricator

**Why a printer and not a crafting table.** A grid of shaped recipes is a
memory game about where to put the sticks. This codebase already has a better
vocabulary for the same idea: everything it owns is a name-keyed row — blocks,
skills, upgrade lines, machines, goods. So the fabricator takes named goods
off a pile and puts named things back, and "it can make anything" means
exactly "adding a thing is adding a row". That is a promise the code can keep,
and the round is the proof: ammunition, a charged cell, planks, wall panels, a
spoofer coil and two whole machines are all one table.

**Every block is stock.** The player's drill used to be the only tool in the
game that produced nothing — you cut a block and it vanished. Now every block
yields *itself*, by name, onto the fleet's base pile: the same pile the flier
ferries into and the shop sells out of. One pile, three doors, and no
transfer minigame, because there is still no player-carried inventory and this
round deliberately did not invent one. It also means a town is feedstock if
you are willing to be that sort of person, and the permits gate is what has an
opinion about it.

**Materials and time, never credits.** Buying a drone with money and printing
one out of copper are two routes to the same machine, which is the point: the
counter is for people with money, the fabricator is for people with a mine.
Materials leave the pile *at the start* — queue three drones on one bar and
the pile would be lying — so a cancelled print costs you, and starting one is
a decision. The Fabrication skill buys speed and unlocks the harder patterns
through the same hard floors every other gate in this game uses; it never buys
certainty, because there are no rolls anywhere in this game.

**The ladder.** Slugs and copper bars at the bottom, so a fabricator pays for
itself the day you place it — and the ammunition loop finally closes, since
slugs were credits-only. Planks and wall panels next, which is the first time
the frontier's own building materials have been makeable. A charged cell in
the middle: swap it in and the kestrel flies *now* rather than after its
recharge. Then a spoofer coil, then the kestrel and the ground drone
themselves.

**It is a machine, so you buy one and place it.** Bought once at the counter,
placed from the belt wherever you want it, moved by breaking it — the chest's
own rule. Placing one you have not bought is refused, or the palette would
quietly hand out free drone factories. And while fixing the belt, slot six
turned out to have been unreachable since the chest was added, which had
quietly made the chest unplaceable; the belt now runs blocks on one to six,
the launcher on seven and the fabricator on eight.

**The oracle keeps up.** `Command::Print` is the first order since `Advance`
that is *not* a replay no-op: a print eats the base pile, and the pile is
something `Rebuilt` carries, so replay spends the same materials at the same
tick and finishes holding the same stock. Only the outputs that live outside
the pile — slugs, cells, machines, modules — are the honest no-ops, the same
line `Give` and `SpawnMachine` draw. VERSION 9 → 10.

### What is deliberately not in 16

Fuel: printing costs time and materials, and when the fuel loop lands it
plugs in as a cost on the *time* rather than a change to what a recipe means
(logs stand in for smelting fuel meanwhile). A print queue longer than one.
Recipes as moddable content — the catalogue is code, which is why the journal
records a print by index rather than by name, and the version bump that makes
it content will say so. And still no player inventory: the pile remains the
only place goods live.

---

## Shipped — Stage 17: caves

**The first hole in a height field.** `fill_column` fills one column from
bedrock to a surface height, and until this round nothing anywhere carved a
hole in the middle of it. Caves are the first genuinely **3D** thing worldgen
has ever done, and the cost was exactly what the plan predicted: not the caves
but the carve — a volumetric field that stays pure in `(seed, x, y, z)` so
chunks keep generating in parallel with no cross-chunk context, and a saved
world keeps regenerating identically. `noise.rs` grew the trilinear sibling of
its 2D value noise; `caves.rs` is the field.

**Tunnels are an intersection, not a threshold.** One noise field thresholded
into air gives bubbles. Two independent signed fields, carved where *both* run
near zero at once, give the intersection of two surfaces — a long winding tube.
That is the whole tunnel algorithm: `a² + b² < r²`, with the y-frequency
squashed so galleries run wide and low rather than as chimneys. A third,
slower field opens chambers where it peaks, gated `CHAMBER_COVER` below the
surface where a room cannot crater a hillside. Girth tapers toward the surface
(`MOUTH_FACTOR`), so mouths exist and stay scarce: at depth the rock is about
a tenth hollow, at the skin far less.

**What is never carved:** town footprints (`fill_column` masks them — a plaza
must not open into a void); the bedrock floor plus a margin (`CAVE_FLOOR`);
and the top `SEA_BED_COVER` blocks of any column ending at or below the
waterline, because there is no fluid simulation and a mouth under the sea
would be a hole the ocean visibly fails to pour into. Where a tunnel meets an
ore body it carves the vein with it — the carve wins — which both guarantees
no copper ever floats in the middle of a gallery and leaves the rest of the
body showing in the cut faces. That is the stage's opening-loop payoff: ore
at the surface of a wall, where hand-mining is pleasant.

**The agents already coped.** The plan flagged `flow::settle`, the mine
planners and span pinning for a pass. Settle was built for "the ground under
me vanished" the day drones learned to dig, so a machine over a void drops to
the gallery floor and carries on; a mine plan that meets a cavity finds part
of its digging already done and the whole agent suite runs green over the new
ground. Flora needed the one real fix: a tree whose base column got carved
into a mouth would have floated, so trees filter on the same pure field the
carve uses — every chunk a canopy reaches agrees, and the stamping stays
seamless.

**Journal VERSION 10 → 11**, a world change alone: every hash a version-ten
journal recorded was taken over terrain that no longer exists, and pretending
the old hashes still bound would make every old session replay as a divergence
that is really this bump. `--cave` joined the capture flags: it hunts the
roomiest pocket of underground air near `--at` and stands the camera in it,
facing down the longest gallery — the capture reads the generated world, not
the field, so what it frames is what generation actually built.

**Deliberately not in 17:** darkness — caves are lit by the same sun as the
surface, and an underground lighting model is its own round; bunkers (next,
now the 3D carve is paid for); anything living down there (hostiles wait for
health); water in caves; per-biome cave styles.

---

## Shipped — Stage 18: lights in the dark

**Darkness first, then the tools that cut it.** Stage 17 left one honest gap:
caves were lit by the same sun as the surface. This round closes it without a
light-transport system. Every face the mesher emits now bakes a 4-bit **sky
exposure** — how deep the air it faces sits beneath the topmost opaque block
of its column. The curve is gentle then steep: two blocks of roof reads as
shade, twenty reads as night. It is an approximation of enclosure, not
radiosity — cheap, pure, and rebaked for free exactly when digging changes
what the sky reaches, because that is when a chunk remeshes anyway. The
packed quad had seven spare bits; light took four of them, and it joined the
greedy merge key so a rectangle never smears one value across a light change.
Machines and people darken by the same column rule, per instance.

**The lamp is a light; the visors are ways of seeing.** The suit's hand lamp
(`L`) is a warm spot cone in the fragment shader, thrown from the active eye —
strength, reach and aim ride the sun uniform, which had reserved its `zw`
lanes for exactly this since the day/night round. Everything better is
printed, not bought: a **high beam** with nearly twice the throw, a **night
vision visor** (an intensifier: it amplifies received light plus a floor read
off the surface, shaped by facing and fading with range, so black galleries
resolve into green geometry rather than one flat wash), and a **thermal
visor** that ignores light entirely — terrain graded cold, objects rendered
as warm bodies, which is what the terrain/object flag in the shared fragment
shader is for. The sky is a clear colour, not fragments, so the visors tint
it on the CPU or a green cave would open onto a blue day.

**Optics are possessions, not upgrades.** `optics.rs` keeps a name-keyed
owned set and the dial, saved as `VXOL` (a dial restored onto gear that is
not owned falls back to Off). One key cycles Off → lamp → night vision →
thermal, skipping what you do not own; the HUD names what you are looking
through. The rows sit in the fabricator's ladder in floor order — beam at 6,
night vision at 12, thermal at 18 — which renumbered the recipes after them,
and `Print` records recipes by index: **journal VERSION 11 → 12**, the
"recipe indices are content" bump stage 16 promised by name. One-per-person
is enforced before the journal ever hears about a duplicate print. Rendering
touches no world state, so nothing else in the log changed.

**Deliberately not in 18:** placeable lights — a torch or floodlight block
needs flood-fill light propagation, which is the real lighting round;
batteries or wear on the visors; light bleed around corners (a lit shaft
does not illuminate the gallery beside it); villagers reacting to a lamp
beam sweeping over them, which wants the perception work hostiles will need
anyway.

---

## Shipped — Stage 19: bunkers, and the geometry that makes each one unique

**The claim, and why it is not mysticism.** The golden ratio is the *most
irrational* number — its continued fraction is all ones, so no fraction
approximates it well — and by the equidistribution theorem `{n·φ}` never
repeats, never clusters and spreads as evenly as a sequence can. That is the
property uniqueness actually needs. Uniform randomness makes every dungeon
feel like every other dungeon because noise has no grammar; quasirandomness is
deterministic, non-repeating and evenly varied. It is also plain arithmetic on
a site hash — no rejection loops, no stored state — which is the only kind of
mathematics the worldgen rules here admit.

**Variation between sites, invariance within one.** A bunker where every room
rolled its own dice is unique the way static is unique. Instead the site hash
picks a handful of numbers — a proportion system, a tier, a footprint, a
bearing — and everything inside derives from those. Each bunker is governed by
one geometry the way each town is governed by one plan.

| System | Ratio | Splits | Reads as |
|---|---|---|---|
| φ | 1.618 | two at `1/φ`, recursing deeper into the *smaller* child | a coil: rooms shrinking inward |
| √2 | 1.414 | two at `1/√2`, both children equally | the barracks grid |
| √3 | 1.732 | three at a time on the long axis | the industrial hive |

Tier weights the draw: shelters coil, works branch. **Golden BSP** places the
rooms — splits at `1/P` of the parent, jittered ±4%, on the longer axis, which
for a P-proportioned rectangle alternates by itself and never yields the
slivers a uniform split does. **Fibonacci is how an irrational proportion
lives on a block grid**: every room dimension snaps to `{3, 5, 8, 13, 21, 34}`,
adjacent pairs approximate φ, and they land exactly on voxels. Orientation is
the **golden angle**: bunker `k` faces `k · 137.507…°`, so no two hatches on a
ridge point the same way and the sequence never repeats world-wide, with not
one byte stored.

**Where the note's plan bent to the block grid** — two deviations, both
documented in the module and both because voxels are not paper. Orientation
is the *entry bearing* rather than a rotation of the plan, because rotating a
voxel BSP by an irrational angle aliases every wall into a staircase; and the
pool *furnishes* generated rooms rather than replacing them, because the
vocabulary cross-product is over thirty room shapes per system and the shell
is identical in all of them. The guarantee the note asked for survives whole:
every legal room size has at least one piece that fits, checked at authoring
time by test.

**The shell is one number, not a new mechanic.** Bunker concrete has hardness
400 against stone's 1 — cutting in is possible and almost never worth it — and
it is `Some(400)`, never `None`, because the point is a choice you can make
and usually regret rather than a wall. Caves are masked over the works so a
tunnel cannot open a sealed shell, and bunkers are refused under towns for the
same reason a plaza may not open into a void.

**Three bugs the tests caught, all of the same family.** Connectivity is the
one failure invisible from the surface — a sealed room is loot the ledger
promises and nothing can collect — so it is asserted, not argued: every room,
every level, every tier, reachable on foot from the hatch. Getting there found
(1) furnishings that ringed their own footprint and sealed an air pocket, now
a flood-fill test over every piece; (2) a stairwell plated per level, which
sealed the very column the levels are threaded on, now one open shaft with
landings beside the run; and (3) the entry corridor paving its floor straight
over the staircase's air — fixed by a rule worth stating plainly, that the way
in is cut *into* finished works and never paves over open space it finds. A
fourth, subtler one: the golden angle computed in `f32` collapsed distinct
bearings onto each other, because `k · 137.5°` runs to tens of millions of
degrees before the modulo and an `f32` that size has an ulp of about four
degrees. The whole irrational-rotation argument was quietly dead until it was
computed in `f64`.

**Loot is the first source to match the sinks.** Supply caches hold goods
derived from where they stand — so two visits agree and nothing is rolled
twice — and *that a crate was opened* is remembered by the crate not being
there any more, which is the one thing this engine already stores. Prising one
open with the drill pays out the same haul as opening it by hand, through one
shared path, so neither can start paying something the other does not.
Prospecting scales the haul rather than re-rolling it. **Journal VERSION 12 →
13**, a world change: the ground now holds works that were not there.

**Deliberately not in 19:** occupants — mobs and military both attack you, and
that needs the health model stage 25 brings; ruin and partial collapse; new
goods (rations, spirits, oil) which are a trade round wearing a bunker's
clothes; pressure, damp or anything else that makes a room at level three
different from the same room at level one; and the geodesic dome and
phyllotaxis silo columns of the large tier's concept art, which want a
rasterisation pass of their own.

---

## Shipped — Stage 20: the fuel loop, and what it is made of

**Machines stop being perpetual.** Every cost in this game had been a one-off:
buy the machine and it works free forever. A running cost is what turns a
stockpile into a supply line — the difference between "can I afford this" and
"can I keep this going" — and this round adds the first one.

**The fuel is oxyhydrogen**, water split back into the two gases it is made of
and burned back into water in the cutting gear. Which means the feedstock is
something the world has an ocean of, so the cost had to move somewhere honest:
to the **electrodes** (copper, dissolved into the bath), the **time** (real
minutes of a machine running), and the **place** — an electrolyser only works
within two blocks of water. That last one is the first machine in this game
whose *position* decides whether it works at all, and it is the reason a lake
shore is now somewhere worth building. The refusal is at placement, not at the
panel, because a machine that looks built and quietly does nothing is the
worse lie.

**How the burn stays inside the oracle.** Fuel decides how much ground gets
dug, and ground is what the world hash covers, so a tank that behaved
differently on replay would be a divergence with the fleet's name on it. The
tank therefore lives on `Mining` — which replay carries — and is drawn and
burned inside `Mining::advance`, which is the very call `Command::Advance`
replays. That is the whole trick: **no fuelling order exists at all**, because
both sides run the same code over the same pile for the same number of ticks
and arrive at the same tank. The only new order is `Electrolyse`, which moves
goods exactly the way `Print` does. The tank is persisted, and has to be:
replay re-derives it from tick zero, so a session that reloaded with an empty
one would run dry at a different tick than its own journal says it did, and
the ground would differ by exactly the digging that bought.

**Measured in machine-ticks, not canisters.** A crew of six burns six times as
fast as a lone drone, and counting whole cells would make that unrepresentable
without fractions — which are precisely what a determinism argument does not
want. One canister is 2,400 machine-ticks: five minutes of one machine, or
seventy-five seconds of a crew of four. The kestrel is not among the burners;
it runs on a charged cell and a cooldown, which was the point of that design,
and charging for the same wing twice would be a tax rather than a mechanic.

**HHO is a traded good, not a private resource.** It joins the goods table
with a price, and the specialities take sides: a depot banks and sells it, a
mine and a refinery burn through it. So the network hauls fuel the way it
hauls ore, prices move when somebody floods a market with it, and a mining
camp is now a town that stops when the fuel does. Adding a fifth good is a
format change to a market's books — an old file's four numbers cannot be read
as five without inventing the missing one — so the economy save bumps to
version three and re-derives a town's books from its site instead of guessing.

**Journal VERSION 13 → 14.** `Electrolyse` reaches the hash by a longer road
than most orders: it only moves goods, but the fleet burns those goods to dig,
so a log replayed without it would run the crew dry at a different tick and
leave a different hole.

**Deliberately not in 20:** a tank per machine, so a drone cannot yet be
stranded far from base with the rest still working; tanker runs, since
machines refuel from the pile at any distance; fuel for the player or the
kestrel; and the oxygen half doing anything of its own — the mix burns as one
good, and splitting it would be a chemistry round rather than a fuel one.

---

## Shipped — Stage 21: star forts, and somewhere to put your things

**A bastioned trace is an argument, not a decoration.** Tall thin walls exist
to stop ladders; once cannon exist they fall down, so the answer is low, thick
and *angled* — every face of the wall covered by the guns of another face, and
no dead ground where an attacker can stand unseen. The star is what falls out
of "no re-entrant angle may go unwatched", and it belongs here because stage 13
gave the frontier something to shoot with.

**The trace is a polar radius, which makes the wall a signed distance.**
`r(θ) = base + bastion · cos(points · θ + phase)`, so "how far is this column
from the wall" is `hypot(dx, dz) - r(θ)`: one cheap expression, pure in
`(seed, position)`, needing no cross-chunk context. Wall, walkway, parapet and
ditch are bands of that one number — exactly the shape the design note asked
for, and the reason a fort costs a chunk almost nothing.

**Tiered, gated, and sometimes ruined.** A hamlet stays open; middling towns
get a palisade or a four-point trace; only the largest earn six points with
re-entrant angles deep enough to read as ravelins. A refinery is likelier to
be walled than a depot, for the obvious reason. Gates sit on the four cardinal
axes because that is where the roads already ran, and each carries a lockbox —
the permits system needed nothing new. A quarter of walled towns have let
theirs go, and a deterministic segment pass drops the wall in places, because
a breach is the thing a player actually remembers.

**One geometry bug worth naming.** The first cut pinned the wall to the
plaza's level and put the trace ten blocks past the core. Both were wrong for
the same reason: a fort's curtain runs a long way out, and out there the
town's plateau has already blended most of the way back to natural ground —
so the wall hung in the air downhill and buried itself uphill. And at ten
blocks the *re-entrant* angles of a six-point trace cut back inside the market
square. The wall now rides each column's own ground, and the standoff is
wider than a bastion plus the curtain's half-thickness. A test pins both:
nothing is ever placed below the plateau, nothing is ever cut above it, and
the inner face never crosses the core.

**And the banks.** Every town now has one, carrying the **Tier Three lockbox**
— the grade stage 11 built and left with a comment saying "endgame; nothing
stamps one yet". It took a building worth robbing to give it a subject.
Inside is a vault: a per-town strongroom, keyed by town centre exactly as the
economy keys its markets, so a vault and a market can never disagree about
which town they belong to.

It answers two things at once. Staging a trade run stopped meaning "carry
everything and sell it in one lump" — leave a load where you mean to sell it
and come back when the price moves. And there is finally somewhere to put what
you cannot afford to lose. Unlike nearly everything else in this world a
vault's contents are **not derived**: they are exactly what somebody put
there, which is what a bank is.

**Robbing one is the loudest thing in the game.** A breach bills by the grade
of what was breached, and Tier Three is stamped on exactly one building in a
town — so a vault door costs `BOUNTY_VAULT`, several times any other crime and
past the warrant threshold on its own. Picking the same lock quietly still
costs the quiet price: that asymmetry *is* the permits round, and Security 60
is the toll on the quiet road.

`Command::Bank` records **the amount that actually moved**, not the amount
asked for — a vault's capacity can bite mid-deposit, and a log saying "all of
it" while the world took two thirds is a divergence waiting for the next
replay. **Journal VERSION 14 → 15**: both kinds of change at once, since the
ground now carries forts and bank buildings as well.

**And everything got foundations.** Buildings had been sitting on dirt, which
made every lock in the game decoration: the way past a Tier Three vault door
was a hole in the floor. They are founded now the way real ones are — a deep
strip under each load-bearing wall, a shallow slab under the floor between,
depth set by what the building protects rather than how tall it is (a shed
runs two courses, the tower four, the bank's strongroom five). Paving runs
none: a plaza is a surface, not a structure, and four hundred hardness under
the market square would wall the town's own ground off from anybody who ever
wanted a cellar.

The footing shares one `FORTIFIED_HARDNESS` constant with the bunker shell
and the heaviest lockbox, because all three are making the same promise —
`Some(400.0)`, never `None`, so it is a cost rather than a wall. A test holds
them to it. Building claims now reach the bottom of their own footing, so
undermining is a permit crime as well as a long afternoon. Fort curtains are
founded along their whole circuit including the gateways *and* the fallen
segments, so a ruin leaves its foundation in the ground where the wall used to
be — which reads right and also means a breach is a gap you walk through
rather than a hole you dig under. **Journal VERSION 15 → 16**, since all of
that is generated ground.

**Deliberately not in 21:** anybody manning the walls, which waits for
hostiles; rubble where a wall fell, or any state between whole and dropped;
fees or interest on a deposit, so banking is convenience rather than a priced
decision; and towns reacting to their own gates being locked or breached.

---

## Shipped — Stage 22: the terminal

**The font's third user, and the game's first typed input.** Every readout so
far answers one question in one place: the HUD says how you are, the handheld
says where your machines are, a shop panel says what is on the shelf. None of
them answer *"what just happened"* — because the answer has been a toast that
fades in three seconds. That is fine for "you levelled up" and useless for
"the crew ran dry while you were down a cave". The terminal keeps four hundred
lines of scrollback, and every toast the game raises is written into it, so
the message you missed is still there when you come up.

**Typing was its own round's worth of work.** A caret that edits *behind*
itself, a command history the arrows walk in both directions, page-up
scrollback, and word wrapping at the panel's width. Characters come from the
platform's own text for the key rather than from the scancode — a scancode is
a *position*, and reading letters off positions is how a console ends up
unusable on half the world's keyboards. Anything the bitmap font cannot draw
is refused at the point of typing rather than stored, so no line can ever
render as a hole; a test caught the first casualty of that rule, which was the
`|` in the help text for `scout`.

**Orders go the long way round on purpose.** The parser is pure — it returns
data, not effects — and an order typed at the prompt is handed to the very
same call the keys use. The journal therefore only ever sees one kind of
dispatch. A console that recorded its own orders would be a second
implementation of every rule in this game and the first one to drift, which is
the same argument that keeps the replay oracle honest everywhere else.

It answers `status`, `fleet`, `where`, `pile` and `bank` from live state, and
takes `dig`, `cancel`, `survey`, `lights`, `save` and the scout's whole order
set. `help` lists them, and a test holds the help and the parser to each
other: the help may not list a verb the parser has never heard of.

**Deliberately not in 22:** scripting, aliases or anything that would let the
terminal replay itself; a command that does something no key can already do,
which would make the console mandatory rather than convenient; and completion,
which wants a wider panel than the font is comfortable in.

---

## Shipped — Stage 23: the townsfolk

**One agent, two policies — this round builds the first.** The people design
note describes a population whose civic half lives in the Stardew Valley and
Animal Crossing tradition and whose combat half (nerve, cover, surrender)
lands with the hostiles rounds. Both halves spend the same derivation: a
`Temperament` — archetype, nerve, warmth, voice — rolled once per person from
the site hash. The archetype that makes a clerk chatty at the counter is the
archetype that will make her slow to panic in a fight; nerve is derived now
and read two stages from now, so nothing about a person ever re-rolls.

**Identity is derived; the hometown is authored.** Every town seats three
named people — one per dwelling, the same beds the permits round mapped — with
trades in the town's speciality (a mine keeps a foreman, a powderman and an
assayer), two loved goods biased by trade, one hated good never loved, and a
birthday in a 28-day year. The hometown trio are people, not arithmetic: THE
MAYOR (proud, loves bars), THE SHERIFF (steady, loves fuel), and OLD PRAT
(chatty, loves logs, hates being handed bars).

**A schedule is worldgen for people.** `where_is(site, person, day, hour)` is
a pure rule stack — alarm, night, market day, work hours, an evening shaped
by archetype, a dawn stroll — with every boundary jittered ±20 minutes by the
person's own hash so the town never moves in lockstep. The villagers' wander
machinery is untouched: a schedule swaps the *patch* it wanders and drives
the same home routes the day/night cycle used to. Each town holds market one
weekday of its own; at noon that day the square fills, and a stakeout, the
kestrel and the roost all observe the same consistent lives.

**Friendship is a ledger, like loot.** What was given and said is remembered
as entries; the number is derived. Loved +60, liked +30, neutral +8, hated
−50; two gifts a week count and a third is taken politely and scored zero;
birthday gifts triple; a first conversation each day is +2; witnessed crimes
against a town push a negative entry to everyone who lives there, scaled off
the same bill the bounty board charged — law and disposition linked but
distinct. Tiers open things that already exist: prices talk at Acquainted,
bunker intel at Trusted (a bearing into stage 19's loot loop, earned by
talking), and at Close a key — an authenticate-rung grant to that person's
own door through the permit system, not around it.

**Gossip is telemetry wearing a coat.** Speech pools key on archetype and
context, and templates fill from live systems: the ore price is the market's
real price, the bounty line quotes the board, "the fleet is dry" is the
tank's own flag. The terminal (stage 22, and this is why it shipped first)
gives it all a surface: `who` lists the roster with tier and current
whereabouts, `talk` has a word with whoever is nearest, `gift <good>` hands
over one good off the base pile — and E with nothing solid in reach talks to
the neighbour in front of you.

**The oracle's line, drawn again.** `Talk` is journalled and replays as a
no-op — disposition lives in its own file (`friends.dat`, VXDS), like permits
grants. `Gift` is *not* a no-op: the good comes off the base pile, which is
state replay carries, so both sides run the same take. Journal VERSION 17.

**Deliberately not in 23:** the rain rule from the note's schedule stack
(this world has no weather to look up); person-policies emitting
`MoveCommand` through the player integrator, composure, cover scoring and
surrender — the note's combat half, recorded whole below for the hostiles
rounds; roster sizes that grow with the books (three per town until the
plans grow more dwellings).

---

## Shipped — Stage 24: the controller, and the Deck package

**Synthesis, not a second input system.** The game has exactly one
implementation of every rule input can reach: keys route through
`handle_press` and `InputState`, the mouse feeds a look accumulator and two
buttons. The pad plugs into those seams and nothing else — buttons resolve
to the `KeyCode` the same action is already bound to and go down the very
same dispatch, the left stick merges into the same movement axes the keys
drive (summed, then capped, so pad and keyboard cannot fight), the right
stick fills the same accumulator the mouse fills in the mouse's own units,
and the triggers mirror the mouse buttons. Every panel, the shop, the map,
the handheld and the terminal's scrollback gained pad support without one
of them changing, which was the entire design argument.

**Context is one bit.** A pad has fewer buttons than a keyboard has keys,
so the face buttons follow the console convention when a panel owns the
screen: south confirms, east backs out, the d-pad becomes the arrows. The
mapping asks a single question — is any panel open — and everything finer
stays downstream, where the keyboard already routes it. Releases look up
what the button meant *at press time*, so a panel closing mid-hold can
never leak a stuck key.

**Honest analog.** A radial deadzone with rescale: rest drift dies at the
threshold, and the live range re-spans from zero so a barely-tilted stick
is a genuine creep rather than a dead spot followed by a lurch. Tested
monotonic. Disconnecting a pad mid-stride releases everything it held.

**SELECT is the manual.** The control scheme renders on the bitmap font as
a panel in the game, from the same table the tests hold drawable — not a
wiki page. A pad connecting says so in a toast and points at it.

**The Deck package.** `dist/install-steamdeck.sh` verifies the binary
against `SHA256SUMS`, installs to `~/.local/share/gamingg/` (no root,
nothing system-wide — SteamOS's root filesystem is read-only and stays
untouched), writes a desktop launcher, and prints the *Add a Non-Steam
Game* steps that put it in Game Mode's library. `--uninstall` reverses it
and keeps saves. The pad backend links `libudev.so.1` at runtime (shipped
everywhere, SteamOS included); building from source now wants `libudev-dev`.

**Deliberately not in 24:** rumble; gyro aim (Steam Input can lend it as
mouse input, which the game already reads); remappable bindings (one good
layout first, a settings file when someone actually wants a second); pad
text entry for the terminal (an on-screen keyboard is its own round);
journal changes of any kind — input synthesis happens above the seams the
journal records, so a pad session records the same orders a keyboard one
does.

---

## Shipped — Stage 25: the workshop

**Two doors onto one upgrade.** The fabricator has always argued that "the
counter is for people with money, the printer is for people with a mine".
Upgrades were the last thing that ignored it: three lines, credits only,
bought at a shelf. Now every line worth fitting has a *part* — DRILL HEAD,
CARGO RACK, PACK FRAME, LAMP REFLECTOR — printable out of ore and time, and
raising the same line the counter raises. Not a second upgrade system: the
same `wallet` entry, the same retroactive effect on machines already in the
field, one number.

**The rule this round discovered.** Adding printable upgrades turned up a
constraint worth writing on the wall: **a recipe's inputs are oracle state;
its refusal is not.** Replay re-runs `Command::Print` by taking
`recipe.inputs` off the pile, so a price that scaled with what you already
own — a wallet level, live-only state a replay does not carry — would have
the two sides take different amounts and the pile would drift apart within
one print. So the parts cost a flat price forever, and the *gate* stiffens
instead: each mark on a line demands `FLOOR_STEP` more Fabrication than the
last, and `refuse()` is only ever asked live. Two tests pin it — one proves
the charge does not move when the wallet does, one walks a line from zero
to five and checks the floor rises every time and that a sixth is refused
at any skill.

**Three new lines, chosen by the same rule.** Every effect had to be
something the journal does not re-derive:

- **PACK** — carry more before the weight tells on you. Legal because the
  player's load reaches the log as a *byte recorded in the `MoveCommand`*,
  not as something replay recomputes.
- **PRESS** — every print finishes sooner. Legal because print *timing* is
  live-only; the journal records the order and its replay arm moves the pile
  in one go.
- **LAMP** — a longer, stronger throw on whichever lamp you carry. Legal
  because it is a shader uniform and reaches nothing else.

A fourth candidate was cut for failing the same test: fuel efficiency.
Burning happens *inside* `Mining::advance`, which is exactly the call
`Command::Advance` replays, so a wallet-dependent burn rate would run the
crew dry at a different tick and leave a different hole. The rule earned its
first refusal before it earned its first feature.

**The press is the one line the counter will not sell.** Rollers for the
fabricator come out of the fabricator, which gives the machine a reason to
be stood in front of rather than ordered from. The panel now shows marks
per row (`3/5`), and the terminal's new `kit` verb prints the character
sheet the game never had: every line, what is fitted, what the next mark
costs in credits, and what it actually does.

Journal **VERSION 18** — no command changed, but five rows joined the
catalogue in ladder order and that renumbers the indices `Print` records.
The same reason twelve exists: recipe indices are content.

**Deliberately not in 25:** per-machine fitments (a drone that is *this*
drone, with its own parts) — that wants machines to be distinguishable
first, which is the wear round's job; recycling a printed thing back into
materials; upgrade lines that touch replayed arithmetic, now formally out of
bounds rather than merely absent; and a sixth mark on any line.

---

## Shipped — Stage 26: wear, and what mends it

**The first machine state that had to be oracle state.** Every other number
a player accumulates in this game — credits, upgrade marks, optics,
friendships — is live-only by design, deliberately outside the replay hash.
Wear cannot be, and the reason is the whole round: a worn crew digs slower,
a seized crew stops, and *how long a crew turned* is exactly what decides
where the hole ends up. So the ledger lives inside `Mining`, beside the fuel
tank, which is the struct `Rebuilt` carries — replay re-runs the same ticks,
re-derives the same wear, and the two sides cannot disagree about how much
ground got cut. Stage 25 said an upgrade may not touch arithmetic the
journal re-runs; this round is the other half of that rule, a system that
*must*, and so had to be built where the oracle can see it.

**A seized tick is a tick nobody works** — the same sentence the fuel loop
wrote, deliberately. Wear plugs into `Mining::advance` beside `fuelled()`,
one gate below the other, and reads as its sibling: fuel decides whether the
crew *can* turn, wear decides whether it *does*.

**Ticks worked, never seconds elapsed.** A machine parked in the garage ages
not at all; one that dug all night is ruined. A session recorded at nine
frames a second wears identically to one at three hundred, for the same
reason the journal counts ticks rather than time.

**Fresh → Worn → Failing → Seized**, at 4 400 / 8 800 / 13 200 worked ticks,
losing one tick in five, then one in two, then all of them. A pleasing
consequence falls out of charging wear only on ticks that *turn*: a failing
machine wears more slowly than a fresh one, because it is working less. The
last stretch of a machine's life is the longest, decay curves rather than
falls off a cliff, and a test pins the ordering.

**The worst machine sets the pace.** A crew works as a crew, and the duty
cycle comes off the worst condition in it rather than an average — an
average is something a player must be *told*, a worst is something they can
*see*. The roster names the bad machine, the HUD says so out loud when one
starts failing, and mending that one machine visibly hands the dig back.

**Recovery closes the loop the workshop opened.** Spare parts are a printed
good, low on the fabricator's ladder on purpose: a fleet that cannot be
mended is a fleet that dies of old age, and nobody should meet that wall
before they can print their way past it. `repair` at the terminal mends the
worst machine, or a named one. `Command::Repair` is journalled — the most
oracle-entangled order in the log — and both the live game and the replay
arm run one `Wear::repair`, so there is no second implementation to drift.
Journal **VERSION 19**.

**Deliberately not in 26:** per-machine fitments and parts (a drone that is
*this* drone); wear on the kestrel, which runs on a cell and a cooldown and
would be charged twice; machines that break *catastrophically* rather than
seizing; and repair as a thing you must physically travel to — you already
can pilot to a machine, and making it mandatory is a fetch quest wearing a
maintenance costume.

---

## Shipped — Stage 27: micro-on-damage

**Detail allocated by damage.** The world is still one metre. Worldgen, the
lattice, blueprints, forts, bunkers, flow fields, footings and every
block-denominated constant are untouched — a *composite* is a wound on the
existing grid, not a resolution increase. A block gains a 4³ interior the
first time something hits it and loses exactly the cells the hit removed, so
detail costs nothing anywhere the player has not shot and is spent precisely
where they are looking hardest.

**A damaged block is one `u64`.** Sixty-four cells, one bit each, at bit
`x + 4z + 16y` — each vertical layer a sixteen-bit slice, the whole wound one
register. Single-material by decree: chewed stone is stone with pieces
missing, because that is what damage *is*. Every operation is register
arithmetic — a carve is one `AND`, the death check is one `popcnt`, the
mesher's face set is six shifts and six ANDs, and even connectivity runs in
registers (dilate-and-mask to a fixpoint, ten iterations being a proof
rather than a guess, because the longest path in a 4³ is nine steps).

**SWAR, not `PEXT` — the Deck is Zen 2.** The obvious instructions for
bit-plane surgery are microcoded on Van Gogh, tens to hundreds of cycles,
fixed only in Zen 3. Everything here is plain shifts, ANDs and `popcnt`,
single-cycle everywhere this game will run — and, being integer arithmetic,
identical on every machine, which is what lets wounds ride the replay oracle
for free.

**Rays read micro; feet never do.** The DDA descends into a composite and
walks its cells, so `sight::obstruction`, projectiles and every cover query
see the hole: chip a wall long enough and a firing hole opens. Movement
collision stays coarse — a composite is a full box until it dies. You can
shoot through a peephole; you can never fall through one. That single
asymmetry is what leaves physics, `supported`, the flow fields and every
footing byte-for-byte untouched, and it is the reason this was one round
rather than four. Both halves are pinned by tests.

**The arsenal grew the mechanic by itself.** A slug bites instead of
deleting, and which cell it bites comes from where the round actually
stopped. Nobody wrote "make a hole": fire twice down one line and the second
round flies through the cells the first one took. That behaviour is now an
arsenal test, and so is its complement — spread fire still demolishes, so
the launcher stays a weapon rather than a chisel. Drilling carves the layer
nearest the bit each quarter of the way through: the same total work, the
same finishing tick, visible at last.

**No second vertex stream was needed.** The note budgeted for one, at
quarter-metre scale with its own uniform. It turned out `PackedQuad`'s
`kind` field had six unused values and micro faces need only three of `w`
and `h`'s nine bits, so cell faces ride the terrain buffer with their
sub-cell offsets in the freed bits — no new pipeline, no new buffer, and
every quad that existed before this round packs to the identical eight
bytes. A test mirrors the shader's own arithmetic in Rust and checks each
face lands on the cell it belongs to, which is a class of bug no test on the
mesher alone can see.

**Wounds converge; battlefields do not accumulate.** Below `DEATH_CELLS` the
block becomes air by the same path every other break takes; a mask above
`HEAL_CELLS` is nearly whole. Carving drops anything it knocks loose, so a
wound is always one connected component — asserted exhaustively, every shape
at every cell and face. Region saves carry the masks (**format version 2**),
raw rather than interned, because the note is explicit that saved masks are
a cache of what replay would recompute and a cache gets pragmatic
compression, not optimal compression.

**Found and fixed on the way:** the gold panel indexed a four-entry stock
array with five goods and had done since HHO landed in stage 20 — it only
crashes under `--features gold`, and recent rounds ran the no-default-feature
suite. The array is now sized by the catalogue, so the next good cannot
repeat it.

**Deliberately not in 27:** the intern table and its quad cache (the note's
own advice is measure first); consolidation's seasonal repair; the distant
LOD majority vote, which is written and tested (`lod_solid`) but not yet
wired to a range; multi-material wounds at a two-block seam; and gravel that
tumbles, which is physics this round refuses to buy.

---

## Shipped — Stage 28: hostiles, health, and the warrant

**Combat had a reason to exist before it had any code.** Crime has raised
bounty since stage 11, and bounty has crossed a warrant threshold that
nothing ever answered. Now it does: cross it and the town sends deputies.
That closes a loop five stages in the making — crime, bounty, warrant,
posse, arrest — and it needed no new worldgen, which is why the occupied
bunkers can still be their own round.

**They believe rather than know.** Each squad holds a last-known position
whose confidence decays, and an occupancy map that spreads probability
across walkable ground from there. Searchers walk to the likeliest cell they
can reach; everything they can see is zeroed. Sweeping a room, covering
ground and doubling back all fall out of the arithmetic — the map cannot
send anybody where the player provably is not, and when the mass runs out
they say so and stand down. It is the same model the kestrel's marks give
the *player*, which is the point: their intelligence about you ages exactly
as fast as yours about them.

**Nerve, not aim.** Composure is the variable the player reads and plays
against. Hits wound it, near misses suppress it, and watching a partner go
down costs more than either — so pinning a deputy is a verb with no new
system behind it. Thresholds are shifted by the `nerve` byte derived back in
stage 23, and archetype overrides the floor: a `Proud` deputy will not
surrender and a `Craven` one never bothers with cover. Character was rolled
once, at creation; this is the second thing to read it, exactly as the
people note promised, and two tests watch temperament show up in *outcome*
rather than in data.

**Cover is a query, not an annotation layer.** Occlusion sampled at three
eye heights — the same `sight::obstruction` call the roost uses to witness a
crime. Blocked standing is a wall to fight from, blocked crouched is
waist-high cover, blocked only prone is a last resort. It is re-scored every
frame, so when the belief moves the old score is stale and they scramble:
that *is* the flanking behaviour, with no flanking code in it.

**Nobody fires through a friend.** One check at the shot, allies counted as
blockers, and a deputy who finds themselves in a partner's lane sidesteps
instead. It is an invariant rather than a tuning goal — a test fires from
three hundred and sixty angles with an ally at the midpoint and never once
gets a round through. Its absence is the thing that reads as contempt in
other games.

**Health is quiet-then-mend, and down is arrested.** Six hits, no medkit
economy: break contact for eight seconds and the count climbs back, which
makes *disengaging* the heal. Fall to zero in front of the law and the
bounty is settled out of credits, the rest written off, and you wake at the
homestead. Dying to something that is not the law waits for a round with
something else in it.

**Live-only, and the law does not shoot the scenery.** None of this reaches
the replay oracle — deputies react to where the player is, and reactions are
not orders, the same line villagers, the roost and contact marks have always
drawn. Their rounds damage you and never the world, which is also the honest
fiction, since property damage is precisely what they are billing you for.
Combat therefore needed no journal version of its own.

**Found on the way:** the composure constants were still unused after the
first pass, which was clippy pointing out that the player could not shoot
*back*. Wiring the return fire is what turned a chase into a fight. The
deputies also walked in x and z alone and hovered off every bank until they
were snapped to the ground.

**Deliberately not in 28:** flow-field pathing, so deputies walk straight
lines and the watchdog abandons blocked approaches rather than going around
— visible the moment a callout happens inside a town, and the first thing
stage 29 fixes; the note's paired movement (one moves while one watches);
the arrest verb applied to *them*, since a surrendered deputy currently just
stops; and the hunt note's stronger search claim, which is kept as a named
ignored test because with one searcher it is not true, and its not being
true is what makes running and hiding a real option.

---

## Planned — the hunt: how hostiles will search, shoot and stalk

A design note arrived extending the combat half of the people note, and it
is recorded whole here — as every note before it has been, because this file
is the one record and a second copy is a second thing to drift. It is the
*engine* the next two rounds assemble rather than invent. Its thesis: the famous failures of game
AI — enemies that do not search, do not survive, do not check their fire,
cannot walk, and never hunt — are pipeline rot, not intelligence problems.
Baked navmeshes desync from streamed worlds; hand-annotated cover goes
stale; the AI reads a copy of a world that moved on. **This engine has none
of those failure modes available to it**: the world *is* the grid, cover and
navigation are derived from live voxels on demand, and derived data cannot
rot. Most of the work is the discipline of not reintroducing by habit the
problems the architecture already solved.

**Belief, not position.** Each squad holds a last-known position with a
confidence that decays — exactly the model the kestrel's marks already give
the player, and the symmetry is the point. Their intelligence about you goes
stale the way yours goes stale about them, breaking line of sight means the
same thing on both sides, and stealth becomes a system rather than a stat.

**Search is an occupancy map, and the map is the behaviour.** Probability
diffuses from the last-known position across walkable cells each tick;
searchers move to the highest-probability cell they can reach; every cell
they see is zeroed. Sweeping rooms, covering exits and doubling back all
fall out of the arithmetic, with no scripted search points and no cheating —
the map literally cannot send a searcher where the player provably is not.
Below a mass threshold they give up, visibly: weapon lowered, patrol
resumed, a line landed.

**Cover is only cover against a belief**, so re-scoring when the belief
moves *is* the flanking mechanic; suppression is composure spent faster than
cover restores it, which hands the player pinning as a verb without a new
system. **Nobody fires through a friend**: two `sight::obstruction` checks —
allies count as blockers at the shot, and the cover scorer charges for
standing in an ally's lane — enforced as an invariant rather than tuned.
Allies extend the same courtesy to the player, because the player's yaw is
already in the command stream.

**A progress watchdog bounds the one failure players never forgive.** An
agent that closes no distance in `STUCK_TICKS` replans; twice stuck, it
abandons the approach. Flow fields already derive from the live grid, so a
dug wall is in the next field rather than in a mesh three patches stale;
people-shaped traversal costs reuse the player's own movement
classification, so a route a hostile takes is a route a player could.

**The stalker is two brains, and it hunts by ear.** A director that knows
the truth but feeds the creature only *zone-grade hints* — a 32-metre cell,
never a position — and a creature that must close the rest with the same
occupancy search as anyone else, so it genuinely hunts and can genuinely be
evaded. What makes it belong to *this* game: hints are weighted by noise,
**and machines are loud**. Drills, crews, the fuel loop's burners — every
report the roost can hear, the deep thing hears better. A dig site in
stalker country is a dinner bell, so the tension attaches to the core mining
loop rather than to a haunted corridor: run the swarm loud and rich, run it
slow and quiet, or dig decoy noise a valley over and work in the shadow of
your own diversion. Pacing is the director's other job — a pressure budget,
forced back-off, a floor distance it never spawns inside.

**Intelligence the player cannot perceive is intelligence wasted.** Every
mode transition lands a tell through the toast-and-terminal channel the
townsfolk round built. The search that sweeps a room in silence may as well
be random; the same search behind "check the far side" is the smartest enemy
the player has ever met.

| Constant | Value |
|---|---|
| `LKP_DECAY` | confidence 100 → 0 over 45 s unseen (matches the kestrel's marks, on purpose) |
| `DIFFUSE_RATE` | 0.18 per tick to open neighbours |
| `GIVE_UP` | total mass < 0.15 |
| `LANE_W` | 1.5 m ally firing lane, charged in the cover score |
| `SUPPRESS_NEAR` | impact within 2 m, composure −8 |
| `STUCK_TICKS` | 40 (~0.6 s) |
| `HINT_GRADE` | 32 m zone — the director never says more |
| `HINT_NOISE_W` | report loudness × 3 |
| `BACK_OFF` | forced ≥ 60 s after 90 s pressure |
| `NO_SPAWN_R` | 48 m |

**How it lands against this roadmap.** The note sequences itself by what a
part has to attach to, which maps onto the arc as it now stands:

| Part | Stage | Why there |
|---|---|---|
| `belief.rs`, the occupancy search, the watchdog | 28 — shipped | lands with the first hostile — there is nothing to hold a belief until then |
| Fire discipline and lane costs | 28 — shipped | needs a *pair* of hostiles before "do not shoot your friend" can be violated |
| The director and its pacing budget | 29 | arrives with the occupied bunkers it paces |
| The stalker, and noise-weighted hints | 31 | waits until the deep caves have somewhere worth being afraid of |

Its tests come with it, and one is a house rule restated: **all of it
replays** — a firefight's journal reproduces every mode transition at the
same tick, hunting AI covered by the oracle like everything else. Nothing in
the note blocks the civic half, which shipped in stage 23.

---

## Planned — the civic layer: permits, offices, elections

The town-law arc, in three rounds. The user's design, recorded whole so none of
it gets lost between rounds.

### B1 — Build permissions and the bounty ledger — **shipped as stage 11**

See the stage 11 section above.

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

Shipped as stage 17 — see the shipped section above. What remains below is
the bunkers half, which rests on the carve the caves round paid for.

### Bunkers

The built half shipped as stage 19 — see the shipped section above. What
remains is the *occupied* half: mobs and military, held until the health
model exists, because both of them attack you.

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

Renumbered again after the townsfolk (23), the controller (24), the
workshop (25), wear (26) and micro-on-damage (27) took their slots: the
plans kept their order, the stages moved down to make room. The hunt note
above supplied the engine for 28, which has shipped; its director lands with
29 and the stalker waits for 31 — and it
now arrives into a world where cover already degrades, which is exactly why
the micro note asked to land before the hostiles rather than after them.

| Stage | What | Why here |
|---|---|---|
| 29 | Bunkers, occupied, and hostiles that path | Mobs and military, and the hunt note's director with its pacing budget and zone-grade hints. Held until here because both attack you, and that needs the health model stage 28 brought. Flow-field pathing for hostiles lands here too: stage 28's deputies walk straight lines, which is fine in the open and obvious in a town. Squads bounded on purpose: pairs move alternately, a pair that loses its partner takes the ally-down composure hit — suppression and command layers explicitly out of scope. Surrender feeds the arrest verb, the jailhouse and capture contracts |
| 30 | Factions and reputation | Bounty (stage 13) is per-town standing; factions are that standing shared between towns — and what a bunker's military garrison belongs to. The spoofers stage 15 taught arrive in their hands |
| 31 | Uranium, oil, gas | New resource *kinds* (fluids, wells) — a bigger worldgen change than more ore. The hunt note's stalker lands here too: it waits until the deep places are worth being afraid of, and hunts by the noise a working mine makes |
| 32 | The pocket arcade | Endgame toy: an original mini-FPS on a craftable handheld |

## The feature map

The whole game at a glance, as of stage 28.

**Shipped:** core scaffold; wgpu renderer + headless capture; block editing
through cancellable events; AABB physics; region saves (name-keyed, cached);
spline terrain; parallel meshing; frustum culling; ore geology with honest
outcrops; drone swarm and three mining methods (adit / decline / pit); flying
drone, sector scanning and the ferry loop; fog-of-war minimap; composite
rigs; handheld drill; RS-curve skills (mining / prospecting / logistics /
security); bitmap font + HUD; starting village, villagers and perception;
day/night; container towns on a lattice; beacon network and contracts; third
person and drone piloting; world hashes and the command journal (the replay
oracle); seed tree; body ids; packed quads; town economies with moving
prices, inter-town freight and player trade runs; machines cost credits;
trade and handheld maps with bearings; tick-based player movement (stance /
sprint / slide / vault / mantle / stamina / carried mass); pregenerated
pinned spawn, background chunk generation, region cache; the player's house,
chest, mailbox and mail-order; the welcome panel with its self-updating
changelog; permits (ranked claims, three lock grades, witnesses and sneaking
by stance eye height, bounty, breaching, lock-picking); the gold panel
(journaled admin orders, live tuning, compiled out of shipped builds); the
arsenal (slug launcher, synthesized audio, recoil and screenshake, town
warnings, witnessed property bounty, panic with flee-or-alarm, caravan
interception and salvage); the kestrel (pack scout, standing orders from
the handheld, decaying contact marks, cell upgrade line) and the roost (the
town's watcher on the office roof, observed-then-witnessed, the heist
window); intrusion through machines (spoofer coils, the leash, machine
witnesses and the impound, the watch box blinded/silenced/tapped, a watch
box for your own roof); the fabricator (every broken block is stock on the
one pile, and a printer that turns named goods into ammunition, bars, planks,
wall panels, charged cells, spoofer coils and whole machines, with the
Fabrication skill buying speed); caves (the first true 3D carve: tunnel
galleries, deep chambers, hillside mouths, ore in the cut faces, nothing
under towns or into the sea); lights in the dark (baked per-face skylight,
a genuinely black underground, the suit's hand lamp, and printed optics —
high beam, night vision, thermal); bunkers (a lattice of their own, three
proportion systems, golden BSP on a Fibonacci vocabulary, golden-angle
bearings, a 400-hardness shell, an authored furnishing pool, and supply
caches whose contents are derived from where they stand); the fuel loop (the
fleet burns oxyhydrogen and stops when it runs out, an electrolyser splits
water into it on any shore, and HHO trades on the network like any other
good); star forts (bastioned traces by town tier, gates on the road axes,
ditches, and deterministic breaches) and town banks (a strongroom per town
behind the first Tier Three lock, for staging trade or keeping what you
cannot lose); foundations under every building and fort wall; the terminal
(typed commands, a caret and history, and a scrollback the toasts land in);
the townsfolk (names, trades and temperaments derived once per person, pure
schedules with per-town market days and ±20-minute personal jitter, a
friendship ledger with gift tables, weekly caps, birthdays and crime
spillover, tier unlocks up to bunker intel and a door key, and speech
templated over live prices, bounty and fuel state);
native gamepad play (buttons synthesized into the keyboard seams,
context-sensitive face buttons, analog sticks with a rescaled deadzone, a
SELECT control-scheme overlay) and a one-command Steam Deck installer;
the workshop (upgrade parts printed from ore as a second route to the same
lines the counter sells, the pack, press and lamp lines, and the rule that
an upgrade may not touch arithmetic the journal re-runs);
wear and recovery (machines age by the tick they work, the worst one sets
the crew's pace, printed spare parts mend them, and the ledger lives inside
the replayed simulation because it decides how much ground gets cut);
micro-on-damage (blocks gain a 4³ interior only where violence touches them,
one `u64` a wound, carved with register arithmetic, drawn as quarter-metre
faces riding the existing quad stream, with rays reading the cells and feet
never doing); hostiles and health (a warrant musters a posse who hold a
decaying belief and search an occupancy map for you, score cover as
occlusion at three eye heights, never fire through each other, and fight,
hide, run or surrender on nerve derived back at the townsfolk round — with
six hits, quiet-then-mend recovery, and an arrest that settles the bounty);
a Steam Deck dist build every round.

**Planned, in arc order:** bunkers occupied, and hostiles that path around
a building instead of into it; the civic layer B2 (offices, the NPC economic loop, the warrant chain) and
B3 (elections on trade goodwill); factions; uranium/oil/gas; the pocket
arcade.

**Outstanding engineering:** floating-origin rebase; journal-shrunk saves;
real min-cost flow for freight; ammunition as a trade good; the rest of the
weapon table; the kestrel's
cell state surviving a reload; pad text entry for the terminal; anything that hacks *you* (the hardened link
has no adversary until factions).

## Known rough edges

Tracked in `README.md` under "Known rough edges" — currently ~25 entries, the
notable ones being: saves store a whole chunk snapshot per modified chunk;
water is alpha-blended without depth sorting; a running excavation is not
persisted; only one drone and one flier are ever created; and there is no
player-carried inventory, so everything routes through the fleet's base pile —
which is also what the movement system now weighs you down by, for want of a
real backpack.
