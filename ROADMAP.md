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
| 14 | _this_ | The kestrel and the roost: a pack scout with standing orders and decaying contact marks, and the town's own watcher on the security office roof |

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

## Planned — Stage 15: hacking through machines

The intrusion round, recorded from the design note whole. The Security
line's work moves *through drones* — a machine perches at the lock while
the operator stands anywhere the link reaches — and the town's machines,
the roost first among them, become the targets worth doing it to.

- **Not a minigame.** Stage 11's rule holds: locks are gated by hard
  floors, not dice. Moving the work onto a drone changes who is exposed
  and where the operator stands, not the odds — which is what keeps a hack
  journallable and honest on a Deck with no mouse to wiggle.
- **A hack is a job, so a drone can carry it.** `Intrude { target }` is a
  dispatched order like a dig; piloting the machine to the box by hand and
  dispatching it are one code path, one journal entry, and a test that says
  the two produce the same world.
- **The machine is exposed; the owner is billed.** A witnessed machine
  marks its owner on the bounty ledger the same as a witnessed hand, and a
  machine caught mid-hack can be seized — impound fine at the counter.
  Remote intrusion buys distance from the *scene*, not the *consequence*.
- **The tool is a module, the ceiling is the skill.** The garage fits a
  spoofer coil (light for the kestrel, heavy for ground frames); what it
  may attempt is capped by the operator's Security level — hardware sets
  where the work can happen, skill sets what work is possible.
- **The roost is the first target that matters.** Three grades up the
  Security line: *blind* it (it stands down, the town notices within a
  day), *silence* it (it patrols and files nothing), *tap* it (its marks
  mirror to your handheld — the strongest intelligence in the game, and
  the view from the other side of the lens).
- **Symmetry, on purpose.** The same spoofers arrive in faction hands
  later, and the garage sells the counter — a hardened link that raises a
  machine's effective lock tier. The player who has tapped a roost knows
  exactly why their own kestrel deserves one. This round also brings the
  **homestead's own roost box** — the sheriff's watcher on your roof, for
  the same price the sheriff paid.
- **Range is the link, and the link is the leash**: operator within link
  of the machine, machine at the target; out-of-link machines finish their
  standing order and return. The number stage 16+'s jammers will shrink.

---

## Planned — star forts: walls with the receipts to justify them

A worldgen round, recorded from the design note whole. Towns grow bastioned
traces — the star-fort geometry that exists because cannon exist: low, thick,
angled walls with no dead ground, every face covered by another face's guns.

- **The trace is a polygon, the wall is a signed distance.** A town's tier
  picks a trace (none / palisade / four-point star / six-point with ravelins),
  authored as a loop of points around the footprint; the wall, walk, parapet
  and ditch are bands of signed distance from that loop, so generation stays
  pure in (seed, position) like every terrain feature.
- **Tiered like everything else.** Hamlets stay open; the county seat earns
  the full six-point trace with ravelins covering its gates. Gates sit where
  the roads already run, and a gate is a claim with a lockbox like any other
  door — the permits system needs nothing new.
- **Some forts are ruins.** A deterministic ruin pass drops wall segments so
  breaches exist to find, because a perfect wall is a worse story than a
  broken one.

Sits after the fuel loop and before hostiles: the walls should exist — and
have gaps — before anything arrives that makes them matter, and the arsenal
(stage 13) is what makes a town honestly want them.

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
- *Military.* The same movement, carrying the arsenal's weapons, and aligned to
  something — which is what makes them a faction problem rather than a monster
  problem.

**The honest sequencing problem:** both flavours attack you, and stage 13
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

Renumbered again after the scout (14) and intrusion (15) rounds took their
slots: the plans kept their order, the stages moved down to make room.

| Stage | What | Why here |
|---|---|---|
| 16 | Caves | The first true 3D carve in a height-field world, and the thing that makes hand-mining pleasant — so it serves the opening loop 10a just built, not only the late game |
| 17 | Bunkers, built and lootable | Rests on caves paying for the 3D carve. Sited on the same lattice as towns, shelled with a very high `hardness`, laid out by the jigsaw generator deferred since stage 8, and looted — a *source* to match 10a's sink |
| 18 | Fuel loop | Machines stop being perpetual. Markets price goods and the network hauls them, so a fuel is one more good on an economy that already knows how to make shortages — and a fuel shortage is the first one that can stop you |
| 19 | Star forts | Bastioned traces per town tier, gates on the roads, deterministic ruins. After the fuel loop and before hostiles: walls should exist — and have gaps — before anything arrives that makes them matter, and the arsenal is what makes a town want them |
| 20 | Text + terminal | The font exists; the terminal is its third user after the HUD and the panels |
| 21 | Crafting + upgrades | Needs the fuel and trade economies to have something to feed |
| 22 | Wear, breakdowns, recovery | Machines that can fail need machines you can reach — piloting shipped in 7 |
| 23 | Hostiles and health | The half of combat stage 13 leaves out. `Perception` (stage 7) is already the shape a hostile needs, and stage 13's bounty contracts are already something for one to take |
| 24 | Bunkers, occupied | Mobs and military. Held until here because both attack you, and that needs the health model stage 23 brings |
| 25 | Factions and reputation | Bounty (stage 13) is per-town standing; factions are that standing shared between towns — and what a bunker's military garrison belongs to. The spoofers stage 15 taught arrive in their hands |
| 26 | Uranium, oil, gas | New resource *kinds* (fluids, wells) — a bigger worldgen change than more ore |
| 27 | The pocket arcade | Endgame toy: an original mini-FPS on a craftable handheld |

## The feature map

The whole game at a glance, as of stage 14.

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
window); a Steam Deck dist build every round.

**Planned, in arc order:** hacking through machines (stage 15: drone-borne
intrusion, spoofer coils, the roost blinded/silenced/tapped, impound, the
homestead's own roost); caves; bunkers built-then-occupied; fuel loop;
star forts; terminal; crafting; wear and breakdowns; hostiles and health;
the civic layer B2 (offices, the NPC economic loop, the warrant chain) and
B3 (elections on trade goodwill); factions; uranium/oil/gas; the pocket
arcade.

**Outstanding engineering:** floating-origin rebase; journal-shrunk saves;
real min-cost flow for freight; ammunition as a trade good; the rest of the
weapon table; gamepad input for the gold panel and the game; the kestrel's
cell state surviving a reload.

## Known rough edges

Tracked in `README.md` under "Known rough edges" — currently ~25 entries, the
notable ones being: saves store a whole chunk snapshot per modified chunk;
water is alpha-blended without depth sorting; a running excavation is not
persisted; only one drone and one flier are ever created; and there is no
player-carried inventory, so everything routes through the fleet's base pile —
which is also what the movement system now weighs you down by, for want of a
real backpack.
