//! The command journal: what the player told the world to do, in order.
//!
//! # Why a log and not a picture
//!
//! Ask who actually edits this world. The player breaks a few hundred blocks by
//! hand over a session. A single drone excavating an open pit moves *tens of
//! thousands* — and it moves exactly the same ones every time, because
//! `digging_is_deterministic` says so and the chunk-pinning in
//! [`vx_agent::working_span`] is what makes that true regardless of where the
//! player was standing.
//!
//! So writing the drone's output to disk is writing down the output of a
//! deterministic function whose input was one line. A whole mine — every bench,
//! every haul, every block — is recoverable from `Dispatch { area, method }`
//! plus the number of ticks it was allowed to run.
//!
//! That is what this records. Region files still hold the ground (they are the
//! keyframes); the log holds everything since the last one, and replaying it
//! reconstructs the rest.
//!
//! # Ticks, never seconds
//!
//! [`crate::mining::Mining::update`] turns wall-clock elapsed time into a tick
//! count, and that conversion depends on frame rate, stalls, and how long
//! somebody dragged the window. None of it is reproducible. The tick count is.
//! So the log records `Advance { ticks }` and replays through
//! [`crate::mining::Mining::advance`], which means a session recorded at nine
//! frames a second replays identically at three hundred.
//!
//! # What it is worth beyond small saves
//!
//! A log plus a content hash is a *determinism oracle*. Replay it and check the
//! world came out the same, and you have a test that covers worldgen, agents
//! and editing all at once — one that catches divergence no unit test is shaped
//! to find. A save becomes a perfect reproduction case. Rewind becomes possible.
//! None of that is built here; all of it is opened up by recording orders
//! instead of outcomes.

use std::io::{Read, Write};
use std::path::Path;

use vx_agent::{MineMethod, VoxelAabb};
use vx_core::{BlockPos, EventBus};
use vx_world::{PlayerBody, World};

use crate::mining::Mining;
use crate::movement::{self, MoveCommand, Movement};

const MAGIC: &[u8; 4] = b"VXLG";
// Bumped to 2 when a dispatch gained its crew size and to 3 when the player's
// own movement joined the log — both format changes.
//
// Four and five are *world* changes: the player's house joined the hometown,
// then the lockboxes and the security office did. An older log replays against
// ground that no longer generates, so it would report a divergence that is
// nobody's fault. Refusing it lets the oracle restart honestly instead. A
// journal is an oracle rather than a load path — region files are still written
// every save — so an old one being rejected costs a determinism check, not a
// world.
//
// Six added the gold panel's admin orders. A cheat is an order like any other:
// the moment an admin action mutated state outside this log, the oracle would
// be blind to it and every replay of that session would report a divergence
// that is nobody's fault.
//
// Seven added `Fire`. Shots break blocks, so a firefight is part of the hash.
//
// Eight is both kinds at once: the roost box joined the security office roof
// (a world change, like four and five) and the kestrel's standing orders
// joined the log (a format change). The scout itself never touches ground —
// its orders replay as documented no-ops — but the log stays the complete
// record of what was ordered.
//
// Nine is both kinds again: the watch box became its own block (a world
// change) and intrusion orders joined the log (a format change). An
// intrusion moves claims and grants, never ground, so it replays as a
// no-op like the scout's orders.
const VERSION: u32 = 9;

/// How many entries may pile up before a keyframe is worth writing.
///
/// Replay is fast — it is the same code the game runs — but not free, and this
/// bounds load time by the log tail rather than by how long a world has been
/// played.
pub const KEYFRAME_ENTRIES: usize = 4_096;

/// One thing the player told the world to do.
///
/// Deliberately *intent*, not outcome: `Dispatch` is "mine that, that way", not
/// the forty thousand blocks that follow from it.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// The handheld drill finished a block.
    Break { at: BlockPos },
    /// A block placed by hand. Named, never numbered — block ids shift when a
    /// mod is installed, exactly as the save format has always said.
    Place { at: BlockPos, block: String },
    /// An excavation ordered: the marked area, the method, and how many drones
    /// were put on it.
    ///
    /// The crew is recorded because it changes the hole. Machines cost credits
    /// now, so how many you own varies over a session, and a replay that
    /// guessed would dig a different excavation from the one that happened.
    Dispatch {
        area: VoxelAabb,
        method: MineMethod,
        crew: u32,
    },
    /// The plan abandoned.
    Cancel,
    /// The player's held input changed.
    ///
    /// Recorded on *change*, not per tick. `Advance` already folds, so holding
    /// W for a minute is one `Move` and one `Advance` rather than four thousand
    /// entries — run-length encoding for free, out of machinery that was
    /// already there.
    ///
    /// This is what extends the oracle to cover where the player walked. It did
    /// not before: the log knew every block a drone cut and nothing at all
    /// about the person who ordered it.
    Move {
        bits: u16,
        yaw_q: i16,
        pitch_q: i16,
        load: u8,
    },
    /// Simulation ticks run. The unit of time the log speaks in.
    Advance { ticks: u32 },
    /// An operator's order from the gold panel. In the same log as everything
    /// else on purpose: a journal with cheats in it still replays to a hash,
    /// which is what makes the panel a scenario editor rather than a taint.
    Admin(Admin),
    /// A machine sent to work a lock or the town's watch box. Journalled
    /// for the record; replayed as a no-op, because what an intrusion moves
    /// is claims and grants — permits state, kept in its own file — and not
    /// one block of the ground the hash covers.
    Intrude(IntrudeOrder),
    /// A standing order for the kestrel. Journalled because the log is the
    /// complete record of what the player ordered; replayed as a no-op
    /// because the scout reveals contacts, never terrain — nothing it does
    /// can reach the hash. The same honest line the economy draws.
    Scout(ScoutOrder),
    /// The launcher fired: the muzzle, and the quantised aim.
    ///
    /// The muzzle position is recorded — the one deliberate exception to
    /// "intent, not outcome". Live movement runs on a real-time 64 Hz clock
    /// and journal ticks on the drones' 8 Hz clock, so the body a replay
    /// rebuilds stands a few subticks from where the live body stood when the
    /// trigger was pulled. A shot breaks blocks, blocks are in the hash, and
    /// a hash must not depend on which clock you asked. The floats cross the
    /// wire as raw bits, like `SetTuning`.
    Fire {
        muzzle: [f32; 3],
        yaw_q: i16,
        pitch_q: i16,
    },
}

/// What the gold panel may order.
///
/// Replay applies what [`Rebuilt`] carries — the player, the movement tuning,
/// the fleet's base pile. Machines, credits, skills and town books are not in
/// `Rebuilt`, so those arms are no-ops on replay, the same honest line
/// `run_replay` already draws for the economy: recorded, visible in the log,
/// not part of the hash the oracle checks.
#[derive(Debug, Clone, PartialEq)]
pub enum Admin {
    /// Goods into the fleet's base pile. No base means nothing happens, live
    /// and replayed alike — the rule must match or the oracle lies.
    Give { good: String, amount: u64 },
    /// Machines into the garage. Replay no-op: dispatches record their crew.
    SpawnMachine { kind: String, count: u32 },
    /// The player, moved. Block-quantised so the landing spot survives the
    /// wire exactly.
    Teleport { x: i32, y: i32, z: i32 },
    /// A named stat. "stamina" is movement state and replays; "credits" and
    /// "xp:<skill>" are live-only.
    SetStat { key: String, value: u64 },
    /// A town's shelf. Live-only: the books are outside the oracle.
    SetStock { x: i32, z: i32, good: String, amount: u64 },
    /// A tunable, by name. This one is the subtle one: it changes how every
    /// later command is *interpreted*, so a journal now carries its physics.
    /// The f32 crosses the wire as raw bits — exact, not printed-and-parsed.
    SetTuning { key: String, value: f32 },
}

/// What the kestrel may be told to do. Mirrors `vx_agent::KestrelMode`
/// minus the states only flight itself can enter (`Manual`, `Returning`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoutOrder {
    /// Come home and dock.
    Dock,
    /// Circle the owner.
    Orbit,
    /// Fly to a column, look, come back.
    Sortie { x: i32, z: i32 },
    /// Land there and watch.
    Perch { x: i32, z: i32 },
    /// Hold ahead along the owner's heading.
    Vanguard,
}

/// What a machine may be told to work on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntrudeOrder {
    /// A lockbox at this block.
    Lock { x: i32, y: i32, z: i32 },
    /// The watch box at this block, to the given grade.
    Roost {
        x: i32,
        y: i32,
        z: i32,
        grade: crate::intrusion::Grade,
    },
    /// Called off.
    Abort,
}

impl Command {
    /// A `Move` carrying this command.
    pub fn moving(command: MoveCommand) -> Self {
        Command::Move {
            bits: command.bits,
            yaw_q: command.yaw_q,
            pitch_q: command.pitch_q,
            load: command.load,
        }
    }
}

/// A command and when it happened, in ticks since the world began.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub tick: u64,
    pub command: Command,
}

/// Everything ordered since the last keyframe.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct CommandLog {
    entries: Vec<Entry>,
    /// The tick the keyframe on disk was taken at.
    pub keyframe_tick: u64,
    /// The world hash at that keyframe, so a replay can say whether it landed
    /// where it was supposed to.
    pub keyframe_hash: u64,
    /// Ticks since the world began.
    tick: u64,
}

impl CommandLog {
    pub fn new() -> Self {
        CommandLog::default()
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Record a command at the current tick.
    ///
    /// `Advance` folds into a preceding `Advance` rather than appending: a
    /// running game issues one per frame, and sixty entries a second would
    /// bury the handful that carry meaning. Folding is exact — ticks add.
    pub fn record(&mut self, command: Command) {
        if let Command::Advance { ticks } = command {
            if ticks == 0 {
                return;
            }
            self.tick += u64::from(ticks);
            if let Some(Entry {
                command: Command::Advance { ticks: last },
                ..
            }) = self.entries.last_mut()
            {
                *last += ticks;
                return;
            }
        }
        self.entries.push(Entry {
            tick: self.tick,
            command,
        });
    }

    /// Is the tail long enough that a keyframe would pay for itself?
    pub fn wants_keyframe(&self) -> bool {
        self.entries.len() >= KEYFRAME_ENTRIES
    }

    /// Start a fresh tail from a keyframe taken now.
    pub fn keyframed(&mut self, hash: u64) {
        self.entries.clear();
        self.keyframe_tick = self.tick;
        self.keyframe_hash = hash;
    }

    pub fn save(&self, directory: &Path) -> std::io::Result<()> {
        let mut file = std::io::BufWriter::new(std::fs::File::create(directory.join("log.dat"))?);
        file.write_all(MAGIC)?;
        file.write_all(&VERSION.to_le_bytes())?;
        file.write_all(&self.keyframe_tick.to_le_bytes())?;
        file.write_all(&self.keyframe_hash.to_le_bytes())?;
        file.write_all(&self.tick.to_le_bytes())?;
        file.write_all(&(self.entries.len() as u32).to_le_bytes())?;
        for entry in &self.entries {
            write_entry(&mut file, entry)?;
        }
        file.flush()
    }

    /// Read a log back, tolerating absence and damage.
    ///
    /// A damaged log is an *empty* log, not a failed world: the region files
    /// are the keyframe and they still hold the ground. What is lost is
    /// whatever happened since — bounded by [`KEYFRAME_ENTRIES`], and said out
    /// loud rather than swallowed.
    pub fn load(directory: &Path) -> CommandLog {
        let path = directory.join("log.dat");
        match read_log(&path) {
            Ok(Some(log)) => log,
            Ok(None) => CommandLog::new(),
            Err(error) => {
                log::warn!(
                    "could not read {}: {error}; the world still loads from its region files, \
                     but the determinism oracle restarts here",
                    path.display()
                );
                CommandLog::restarted()
            }
        }
    }

    /// A log that begins mid-history, after an old or damaged one was refused.
    ///
    /// The nonzero keyframe tick is the load-bearing part: it says "this log
    /// does not reach back to genesis", so `--replay` declines to compare
    /// hashes instead of reporting a divergence that is really a version bump.
    /// One specific tick would be a lie — the refused file's clock is exactly
    /// what could not be read.
    pub fn restarted() -> CommandLog {
        CommandLog {
            keyframe_tick: 1,
            ..CommandLog::default()
        }
    }
}

/// Everything a replay reconstructs.
pub struct Rebuilt {
    pub mining: Mining,
    pub player: PlayerBody,
    pub movement: Movement,
    /// The command in force, carried across every `Advance` that follows it.
    held: MoveCommand,
    /// Slugs in the air, stepped one integration per `Advance` tick.
    pub shots: Vec<crate::arsenal::Shot>,
}

/// Replay a log over a world.
///
/// The world must already be at the log's keyframe — region files loaded, or a
/// bare generated world when `keyframe_tick` is zero. `start` is where the
/// player's feet begin: the log records what they *did*, not where they were
/// standing when they started, so the caller supplies the origin the path is
/// measured from.
impl Default for Rebuilt {
    fn default() -> Self {
        Rebuilt {
            mining: Mining::default(),
            player: PlayerBody::default(),
            movement: Movement::default(),
            held: MoveCommand::default(),
            shots: Vec::new(),
        }
    }
}

pub fn replay_from(
    log: &CommandLog,
    world: &mut World,
    events: &EventBus,
    start: glam::Vec3,
) -> Rebuilt {
    let mut state = Rebuilt {
        player: PlayerBody {
            position: start,
            ..PlayerBody::default()
        },
        ..Rebuilt::default()
    };
    for entry in log.entries() {
        apply(&entry.command, world, events, &mut state);
    }
    state
}

/// Replay from the surface at the world origin.
///
/// Deterministic from the seed, which is what an oracle needs; it is not
/// necessarily where the player actually stood.
pub fn replay(log: &CommandLog, world: &mut World, events: &EventBus) -> Rebuilt {
    let ground = world.surface_y(0, 0).unwrap_or(64) as f32;
    replay_from(log, world, events, glam::Vec3::new(0.5, ground, 0.5))
}

fn apply(command: &Command, world: &mut World, events: &EventBus, state: &mut Rebuilt) {
    let mining = &mut state.mining;
    match command {
        Command::Break { at } => {
            let _ = vx_world::break_block(world, events, *at);
        }
        Command::Place { at, block } => {
            if let Some(id) = world.registry().id_of(block) {
                world.set_block(*at, id);
            } else {
                // A block whose mod is gone decodes to nothing rather than to
                // whatever now occupies its number — the same rule the region
                // format follows.
                log::warn!("replay skipped an unknown block {block} at {at:?}");
            }
        }
        Command::Dispatch { area, method, crew } => {
            mining.mark(world, area.min);
            mining.mark(world, area.max);
            // Select the recorded method rather than trusting the ranking:
            // ranking is deterministic, but it reads the world, and pinning the
            // *choice* means a later tweak to the cost model cannot silently
            // re-cut an old mine a different way.
            for _ in 0..MineMethod::ALL.len() {
                if mining.selected_plan().is_some_and(|plan| plan.method == *method) {
                    break;
                }
                mining.cycle_method();
            }
            mining.start(world, *crew);
        }
        Command::Cancel => mining.cancel(world),
        Command::Move {
            bits,
            yaw_q,
            pitch_q,
            load,
        } => {
            state.held = MoveCommand {
                bits: *bits,
                yaw_q: *yaw_q,
                pitch_q: *pitch_q,
                load: *load,
            };
        }
        Command::Admin(order) => match order {
            Admin::Give { good, amount } => {
                // Same rule as live: no base, nothing happens. The rule must
                // match on both sides or the oracle lies.
                if let Some(base) = state.mining.fleet.base.as_mut() {
                    base.stockpile.add(good.clone(), *amount);
                }
            }
            Admin::Teleport { x, y, z } => {
                state.player.position = glam::Vec3::new(*x as f32 + 0.5, *y as f32, *z as f32 + 0.5);
                state.player.velocity = glam::Vec3::ZERO;
                state.player.on_ground = false;
            }
            Admin::SetStat { key, value } => {
                // Only what `Rebuilt` carries. Everything else is live-only,
                // the same line `run_replay` draws for the economy.
                if key == "stamina" {
                    state.movement.stamina =
                        (*value as f32).min(state.movement.tuning.stam_max);
                }
            }
            Admin::SetTuning { key, value } => {
                if !state.movement.tuning.set(key, *value) {
                    log::warn!("replay met an unknown tunable '{key}'");
                }
            }
            // Machines and town books are outside the oracle: crews are
            // recorded on each dispatch, and the books were never replayed.
            Admin::SpawnMachine { .. } | Admin::SetStock { .. } => {}
        },
        // The scout reveals contacts, never terrain: nothing it does can
        // reach the world hash, so replay only needs to decode the order and
        // move on — the same honest line run_replay draws for the economy.
        // An intrusion moves grants and claims, which live in their own file
        // outside the hash, so it draws the same line.
        Command::Scout(_) | Command::Intrude(_) => {}
        Command::Fire {
            muzzle,
            yaw_q,
            pitch_q,
        } => {
            crate::arsenal::launch(
                &mut state.shots,
                &mut state.movement,
                glam::Vec3::from(*muzzle),
                *yaw_q,
                *pitch_q,
            );
        }
        Command::Advance { ticks } => {
            mining.advance(world, events, *ticks);
            // The held command is re-issued for every tick it covers, at
            // `SUBTICKS` movement steps each. This is the same call the live
            // game makes, which is what makes the log an oracle for the
            // player's path rather than a second implementation of it.
            for _ in 0..*ticks {
                movement::advance_journal_tick(
                    &mut state.movement,
                    &mut state.player,
                    world,
                    state.held,
                );
                // Shots step on the same clock. The sweeps are dropped: the
                // craters are the part the hash checks, the bills were the
                // live game's business.
                let _ = crate::arsenal::advance_shots(
                    &mut state.shots,
                    world,
                    &state.movement.tuning,
                );
            }
        }
    }
}

fn write_entry(file: &mut impl Write, entry: &Entry) -> std::io::Result<()> {
    file.write_all(&entry.tick.to_le_bytes())?;
    match &entry.command {
        Command::Break { at } => {
            file.write_all(&[0u8])?;
            write_pos(file, *at)?;
        }
        Command::Place { at, block } => {
            file.write_all(&[1u8])?;
            write_pos(file, *at)?;
            file.write_all(&(block.len() as u32).to_le_bytes())?;
            file.write_all(block.as_bytes())?;
        }
        Command::Dispatch { area, method, crew } => {
            file.write_all(&[2u8])?;
            write_pos(file, area.min)?;
            write_pos(file, area.max)?;
            file.write_all(&[match method {
                MineMethod::Adit => 0,
                MineMethod::Decline => 1,
                MineMethod::Pit => 2,
            }])?;
            file.write_all(&crew.to_le_bytes())?;
        }
        Command::Cancel => file.write_all(&[3u8])?,
        Command::Advance { ticks } => {
            file.write_all(&[4u8])?;
            file.write_all(&ticks.to_le_bytes())?;
        }
        Command::Move {
            bits,
            yaw_q,
            pitch_q,
            load,
        } => {
            file.write_all(&[5u8])?;
            file.write_all(&bits.to_le_bytes())?;
            file.write_all(&yaw_q.to_le_bytes())?;
            file.write_all(&pitch_q.to_le_bytes())?;
            file.write_all(&[*load])?;
        }
        Command::Admin(order) => match order {
            Admin::Give { good, amount } => {
                file.write_all(&[6u8])?;
                write_name(file, good)?;
                file.write_all(&amount.to_le_bytes())?;
            }
            Admin::SpawnMachine { kind, count } => {
                file.write_all(&[7u8])?;
                write_name(file, kind)?;
                file.write_all(&count.to_le_bytes())?;
            }
            Admin::Teleport { x, y, z } => {
                file.write_all(&[8u8])?;
                file.write_all(&x.to_le_bytes())?;
                file.write_all(&y.to_le_bytes())?;
                file.write_all(&z.to_le_bytes())?;
            }
            Admin::SetStat { key, value } => {
                file.write_all(&[9u8])?;
                write_name(file, key)?;
                file.write_all(&value.to_le_bytes())?;
            }
            Admin::SetStock { x, z, good, amount } => {
                file.write_all(&[10u8])?;
                file.write_all(&x.to_le_bytes())?;
                file.write_all(&z.to_le_bytes())?;
                write_name(file, good)?;
                file.write_all(&amount.to_le_bytes())?;
            }
            Admin::SetTuning { key, value } => {
                file.write_all(&[11u8])?;
                write_name(file, key)?;
                // Raw bits: exact, not printed-and-parsed.
                file.write_all(&value.to_bits().to_le_bytes())?;
            }
        },
        Command::Fire {
            muzzle,
            yaw_q,
            pitch_q,
        } => {
            file.write_all(&[12u8])?;
            for part in muzzle {
                file.write_all(&part.to_bits().to_le_bytes())?;
            }
            file.write_all(&yaw_q.to_le_bytes())?;
            file.write_all(&pitch_q.to_le_bytes())?;
        }
        Command::Intrude(order) => {
            file.write_all(&[14u8])?;
            match order {
                IntrudeOrder::Abort => file.write_all(&[0u8])?,
                IntrudeOrder::Lock { x, y, z } => {
                    file.write_all(&[1u8])?;
                    file.write_all(&x.to_le_bytes())?;
                    file.write_all(&y.to_le_bytes())?;
                    file.write_all(&z.to_le_bytes())?;
                }
                IntrudeOrder::Roost { x, y, z, grade } => {
                    file.write_all(&[2u8])?;
                    file.write_all(&x.to_le_bytes())?;
                    file.write_all(&y.to_le_bytes())?;
                    file.write_all(&z.to_le_bytes())?;
                    file.write_all(&[match grade {
                        crate::intrusion::Grade::Blind => 0u8,
                        crate::intrusion::Grade::Silence => 1u8,
                        crate::intrusion::Grade::Tap => 2u8,
                    }])?;
                }
            }
        }
        Command::Scout(order) => {
            file.write_all(&[13u8])?;
            match order {
                ScoutOrder::Dock => file.write_all(&[0u8])?,
                ScoutOrder::Orbit => file.write_all(&[1u8])?,
                ScoutOrder::Sortie { x, z } => {
                    file.write_all(&[2u8])?;
                    file.write_all(&x.to_le_bytes())?;
                    file.write_all(&z.to_le_bytes())?;
                }
                ScoutOrder::Perch { x, z } => {
                    file.write_all(&[3u8])?;
                    file.write_all(&x.to_le_bytes())?;
                    file.write_all(&z.to_le_bytes())?;
                }
                ScoutOrder::Vanguard => file.write_all(&[4u8])?,
            }
        }
    }
    Ok(())
}

/// A length-prefixed name on the wire, capped like every other name here.
fn write_name(file: &mut impl Write, name: &str) -> std::io::Result<()> {
    file.write_all(&(name.len() as u32).to_le_bytes())?;
    file.write_all(name.as_bytes())
}

fn read_name(file: &mut impl Read) -> std::io::Result<String> {
    let length = read_u32(file)? as usize;
    if length > 256 {
        return Err(std::io::Error::other("implausible name length"));
    }
    let mut bytes = vec![0u8; length];
    file.read_exact(&mut bytes)?;
    String::from_utf8(bytes).map_err(|_| std::io::Error::other("name is not text"))
}

fn write_pos(file: &mut impl Write, at: BlockPos) -> std::io::Result<()> {
    file.write_all(&at.x.to_le_bytes())?;
    file.write_all(&at.y.to_le_bytes())?;
    file.write_all(&at.z.to_le_bytes())
}

fn read_u32(file: &mut impl Read) -> std::io::Result<u32> {
    let mut word = [0u8; 4];
    file.read_exact(&mut word)?;
    Ok(u32::from_le_bytes(word))
}

fn read_u64(file: &mut impl Read) -> std::io::Result<u64> {
    let mut word = [0u8; 8];
    file.read_exact(&mut word)?;
    Ok(u64::from_le_bytes(word))
}

fn read_pos(file: &mut impl Read) -> std::io::Result<BlockPos> {
    Ok(BlockPos::new(
        read_u32(file)? as i32,
        read_u32(file)? as i32,
        read_u32(file)? as i32,
    ))
}

fn read_entry(file: &mut impl Read) -> std::io::Result<Entry> {
    let tick = read_u64(file)?;
    let mut tag = [0u8; 1];
    file.read_exact(&mut tag)?;
    let command = match tag[0] {
        0 => Command::Break { at: read_pos(file)? },
        1 => {
            let at = read_pos(file)?;
            let length = read_u32(file)? as usize;
            if length > 256 {
                return Err(std::io::Error::other("implausible block name"));
            }
            let mut bytes = vec![0u8; length];
            file.read_exact(&mut bytes)?;
            let block =
                String::from_utf8(bytes).map_err(|_| std::io::Error::other("name is not text"))?;
            Command::Place { at, block }
        }
        2 => {
            let min = read_pos(file)?;
            let max = read_pos(file)?;
            let mut method = [0u8; 1];
            file.read_exact(&mut method)?;
            Command::Dispatch {
                area: VoxelAabb::new(min, max),
                method: match method[0] {
                    0 => MineMethod::Adit,
                    1 => MineMethod::Decline,
                    2 => MineMethod::Pit,
                    other => {
                        return Err(std::io::Error::other(format!("unknown method {other}")))
                    }
                },
                crew: read_u32(file)?,
            }
        }
        3 => Command::Cancel,
        4 => Command::Advance {
            ticks: read_u32(file)?,
        },
        5 => {
            let mut bits = [0u8; 2];
            file.read_exact(&mut bits)?;
            let mut yaw = [0u8; 2];
            file.read_exact(&mut yaw)?;
            let mut pitch = [0u8; 2];
            file.read_exact(&mut pitch)?;
            let mut load = [0u8; 1];
            file.read_exact(&mut load)?;
            Command::Move {
                bits: u16::from_le_bytes(bits),
                yaw_q: i16::from_le_bytes(yaw),
                pitch_q: i16::from_le_bytes(pitch),
                load: load[0],
            }
        }
        6 => Command::Admin(Admin::Give {
            good: read_name(file)?,
            amount: read_u64(file)?,
        }),
        7 => Command::Admin(Admin::SpawnMachine {
            kind: read_name(file)?,
            count: read_u32(file)?,
        }),
        8 => Command::Admin(Admin::Teleport {
            x: read_u32(file)? as i32,
            y: read_u32(file)? as i32,
            z: read_u32(file)? as i32,
        }),
        9 => Command::Admin(Admin::SetStat {
            key: read_name(file)?,
            value: read_u64(file)?,
        }),
        10 => Command::Admin(Admin::SetStock {
            x: read_u32(file)? as i32,
            z: read_u32(file)? as i32,
            good: read_name(file)?,
            amount: read_u64(file)?,
        }),
        11 => Command::Admin(Admin::SetTuning {
            key: read_name(file)?,
            value: f32::from_bits(read_u32(file)?),
        }),
        12 => {
            let mut muzzle = [0f32; 3];
            for part in &mut muzzle {
                *part = f32::from_bits(read_u32(file)?);
            }
            let mut yaw = [0u8; 2];
            file.read_exact(&mut yaw)?;
            let mut pitch = [0u8; 2];
            file.read_exact(&mut pitch)?;
            Command::Fire {
                muzzle,
                yaw_q: i16::from_le_bytes(yaw),
                pitch_q: i16::from_le_bytes(pitch),
            }
        }
        13 => {
            let mut which = [0u8; 1];
            file.read_exact(&mut which)?;
            let order = match which[0] {
                0 => ScoutOrder::Dock,
                1 => ScoutOrder::Orbit,
                2 => ScoutOrder::Sortie {
                    x: read_u32(file)? as i32,
                    z: read_u32(file)? as i32,
                },
                3 => ScoutOrder::Perch {
                    x: read_u32(file)? as i32,
                    z: read_u32(file)? as i32,
                },
                4 => ScoutOrder::Vanguard,
                other => {
                    return Err(std::io::Error::other(format!(
                        "unknown scout order {other}"
                    )))
                }
            };
            Command::Scout(order)
        }
        14 => {
            let mut which = [0u8; 1];
            file.read_exact(&mut which)?;
            let order = match which[0] {
                0 => IntrudeOrder::Abort,
                1 => IntrudeOrder::Lock {
                    x: read_u32(file)? as i32,
                    y: read_u32(file)? as i32,
                    z: read_u32(file)? as i32,
                },
                2 => {
                    let (x, y, z) = (
                        read_u32(file)? as i32,
                        read_u32(file)? as i32,
                        read_u32(file)? as i32,
                    );
                    let mut grade = [0u8; 1];
                    file.read_exact(&mut grade)?;
                    let grade = match grade[0] {
                        0 => crate::intrusion::Grade::Blind,
                        1 => crate::intrusion::Grade::Silence,
                        2 => crate::intrusion::Grade::Tap,
                        other => {
                            return Err(std::io::Error::other(format!(
                                "unknown roost grade {other}"
                            )))
                        }
                    };
                    IntrudeOrder::Roost { x, y, z, grade }
                }
                other => {
                    return Err(std::io::Error::other(format!(
                        "unknown intrusion order {other}"
                    )))
                }
            };
            Command::Intrude(order)
        }
        other => return Err(std::io::Error::other(format!("unknown command {other}"))),
    };
    Ok(Entry { tick, command })
}

fn read_log(path: &Path) -> std::io::Result<Option<CommandLog>> {
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
    if read_u32(&mut file)? != VERSION {
        return Err(std::io::Error::other("unknown version"));
    }

    let keyframe_tick = read_u64(&mut file)?;
    let keyframe_hash = read_u64(&mut file)?;
    let tick = read_u64(&mut file)?;
    let count = read_u32(&mut file)?;

    let mut entries = Vec::new();
    for _ in 0..count {
        entries.push(read_entry(&mut file)?);
    }
    Ok(Some(CommandLog {
        entries,
        keyframe_tick,
        keyframe_hash,
        tick,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::ChunkPos;
    use vx_world::region_hash;

    /// A world with a body worth mining a few blocks under the surface.
    fn site() -> (World, VoxelAabb) {
        let mut world = World::new(2024);
        world.load_around(ChunkPos::new(0, 0), 2);
        let ground = world.generator().height_at(4, 4);
        let area = VoxelAabb::new(
            BlockPos::new(2, ground - 7, 2),
            BlockPos::new(5, ground - 5, 5),
        );
        (world, area)
    }

    /// Order a mine and let it run, recording as we go — what the running game
    /// does, minus the window.
    fn record_a_session() -> CommandLog {
        let (mut world, area) = site();
        let events = EventBus::new();
        let mut journal = CommandLog::new();
        let mut rebuilt = Rebuilt::default();

        let dispatch = Command::Dispatch {
            area,
            method: MineMethod::Pit,
            crew: 3,
        };
        apply(&dispatch, &mut world, &events, &mut rebuilt);
        journal.record(dispatch);

        // Frames of wildly different lengths, as a real session has.
        for ticks in [1u32, 7, 3, 16, 2, 11, 16, 16, 9, 4] {
            rebuilt.mining.advance(&mut world, &events, ticks);
            journal.record(Command::Advance { ticks });
        }

        // And some work by hand on top.
        let ground = world.generator().height_at(20, 20);
        for offset in 0..4 {
            let at = BlockPos::new(20, ground - 1 - offset, 20);
            let _ = vx_world::break_block(&mut world, &events, at);
            journal.record(Command::Break { at });
        }
        journal
    }

    /// Replay a log from a fresh world and report the ground it produced.
    fn replay_to_hash(journal: &CommandLog, area: VoxelAabb) -> u64 {
        let (mut world, _) = site();
        let events = EventBus::new();
        replay(journal, &mut world, &events);
        let span = vx_agent::working_span(area, area.min);
        region_hash(&world, span.min, span.max)
    }

    #[test]
    fn a_recorded_session_replays_to_the_same_world() {
        // The oracle. If this ever fails, something in worldgen, the agents or
        // the editing path stopped being deterministic — and it will fail here
        // long before anyone notices it in a save.
        let (mut world, area) = site();
        let events = EventBus::new();
        let mut journal = CommandLog::new();
        let mut rebuilt = Rebuilt::default();

        let dispatch = Command::Dispatch {
            area,
            method: MineMethod::Pit,
            crew: 3,
        };
        apply(&dispatch, &mut world, &events, &mut rebuilt);
        journal.record(dispatch);
        for ticks in [5u32, 16, 16, 16, 11] {
            rebuilt.mining.advance(&mut world, &events, ticks);
            journal.record(Command::Advance { ticks });
        }

        let span = vx_agent::working_span(area, area.min);
        let played = region_hash(&world, span.min, span.max);
        assert_eq!(
            played,
            replay_to_hash(&journal, area),
            "replaying the session produced a different world"
        );
    }

    #[test]
    fn the_journal_is_far_smaller_than_the_ground_it_describes() {
        // The whole argument for recording orders instead of outcomes: a mine
        // is thousands of block changes and one line of intent.
        let (mut world, area) = site();
        let events = EventBus::new();
        let mut journal = CommandLog::new();
        let mut rebuilt = Rebuilt::default();

        let dispatch = Command::Dispatch {
            area,
            method: MineMethod::Pit,
            crew: 3,
        };
        apply(&dispatch, &mut world, &events, &mut rebuilt);
        journal.record(dispatch);
        // Frames of wildly different lengths, as a real session has.
        for _ in 0..200 {
            for ticks in [1u32, 7, 3, 16, 2, 11, 16, 9] {
                rebuilt.mining.advance(&mut world, &events, ticks);
                journal.record(Command::Advance { ticks });
            }
        }

        // Count what actually moved, against a pristine copy of the same seed.
        let span = vx_agent::working_span(area, area.min);
        let mut pristine = World::new(2024);
        pristine.pin_span(span.min, span.max);

        let mut dug = 0usize;
        for y in span.min.y..=span.max.y {
            for z in span.min.z..=span.max.z {
                for x in span.min.x..=span.max.x {
                    let at = BlockPos::new(x, y, z);
                    if pristine.block(at) != world.block(at) {
                        dug += 1;
                    }
                }
            }
        }

        assert!(dug > 300, "the fixture mine only moved {dug} blocks");
        assert!(
            journal.len() < 32,
            "{} entries described {dug} changed blocks",
            journal.len()
        );
        // And it still replays to exactly that ground.
        assert_eq!(
            region_hash(&world, span.min, span.max),
            replay_to_hash(&journal, area),
            "the compact journal did not rebuild the mine"
        );
    }

    #[test]
    fn ticks_fold_so_a_running_game_does_not_bury_the_log() {
        // Sixty entries a second would drown the handful that carry meaning.
        let mut journal = CommandLog::new();
        for _ in 0..1_000 {
            journal.record(Command::Advance { ticks: 3 });
        }
        assert_eq!(journal.len(), 1, "per-frame ticks were not folded");
        assert_eq!(journal.tick(), 3_000, "folding lost time");
        assert_eq!(
            journal.entries()[0].command,
            Command::Advance { ticks: 3_000 }
        );

        // A zero-tick frame is not an event.
        journal.record(Command::Advance { ticks: 0 });
        assert_eq!(journal.len(), 1);
        assert_eq!(journal.tick(), 3_000);

        // Anything else breaks the run.
        journal.record(Command::Cancel);
        journal.record(Command::Advance { ticks: 2 });
        assert_eq!(journal.len(), 3);
    }

    #[test]
    fn commands_are_stamped_with_the_tick_they_happened_on() {
        let mut journal = CommandLog::new();
        journal.record(Command::Advance { ticks: 40 });
        journal.record(Command::Break {
            at: BlockPos::new(1, 2, 3),
        });
        assert_eq!(journal.entries()[1].tick, 40);
    }

    #[test]
    fn a_keyframe_clears_the_tail_but_not_the_clock() {
        let mut journal = CommandLog::new();
        journal.record(Command::Advance { ticks: 100 });
        journal.record(Command::Cancel);
        journal.keyframed(0xdead_beef);

        assert!(journal.is_empty(), "the tail survived a keyframe");
        assert_eq!(journal.keyframe_tick, 100);
        assert_eq!(journal.keyframe_hash, 0xdead_beef);
        assert_eq!(journal.tick(), 100, "the keyframe rewound the clock");
    }

    #[test]
    fn the_journal_round_trips_and_tolerates_damage() {
        let directory = std::env::temp_dir().join(format!("vx-journal-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();

        let mut journal = record_a_session();
        journal.keyframed(0x1234_5678);
        journal.record(Command::Place {
            at: BlockPos::new(-3, 70, 8),
            block: "engine:stone".into(),
        });
        journal.record(Command::Advance { ticks: 12 });
        journal.save(&directory).unwrap();

        let read = CommandLog::load(&directory);
        assert_eq!(read, journal, "the journal did not survive the trip");

        std::fs::write(directory.join("log.dat"), b"NOT A JOURNAL").unwrap();
        assert!(
            CommandLog::load(&directory).is_empty(),
            "a damaged journal invented commands"
        );

        std::fs::remove_dir_all(&directory).ok();
        assert!(CommandLog::load(&directory).is_empty());
    }

    #[test]
    fn negative_coordinates_survive_the_encoding() {
        // The encoder writes positions through u32; a sign bug here would move
        // every edit west of the origin somewhere else entirely.
        let directory = std::env::temp_dir().join(format!("vx-journal-neg-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();

        let mut journal = CommandLog::new();
        let at = BlockPos::new(-1_000, 5, -7);
        journal.record(Command::Break { at });
        journal.save(&directory).unwrap();

        assert_eq!(
            CommandLog::load(&directory).entries()[0].command,
            Command::Break { at }
        );
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn a_saved_world_and_its_journal_agree_across_disk() {
        // What `--replay` does, end to end: play a session, save the ground and
        // the orders, then rebuild the ground from the orders alone and check
        // the two hash the same.
        let directory = std::env::temp_dir().join(format!("vx-replay-{}", std::process::id()));
        std::fs::remove_dir_all(&directory).ok();
        let save = vx_world::WorldSave::create(&directory).unwrap();

        let (mut world, area) = site();
        let events = EventBus::new();
        let mut journal = CommandLog::new();
        let mut rebuilt = Rebuilt::default();

        let dispatch = Command::Dispatch {
            area,
            method: MineMethod::Pit,
            crew: 3,
        };
        apply(&dispatch, &mut world, &events, &mut rebuilt);
        journal.record(dispatch);
        for _ in 0..40 {
            rebuilt.mining.advance(&mut world, &events, 16);
            journal.record(Command::Advance { ticks: 16 });
        }
        let ground = world.generator().height_at(20, 20);
        let by_hand = BlockPos::new(20, ground - 1, 20);
        vx_world::break_block(&mut world, &events, by_hand).unwrap();
        journal.record(Command::Break { at: by_hand });

        save.write_meta(world.seed()).unwrap();
        save.save_world(&mut world).unwrap();
        journal.save(&directory).unwrap();

        // Now the replay side, from nothing but the seed and the orders.
        let seed = save.read_meta().unwrap();
        let read = CommandLog::load(&directory);
        let mut rebuilt = World::new(seed);
        replay(&read, &mut rebuilt, &EventBus::new());

        // And the saved side, chunk by chunk, exactly as `--replay` reloads it.
        let mut restored = World::new(seed);
        for pos in rebuilt.loaded_chunks().collect::<Vec<_>>() {
            match save.load_chunk(pos, restored.registry()).unwrap() {
                Some(chunk) => restored.insert_chunk(chunk),
                None => {
                    restored.load_chunk(pos);
                }
            }
        }

        assert_eq!(
            vx_world::world_hash(&restored),
            vx_world::world_hash(&rebuilt),
            "the journal did not rebuild the world that was saved beside it"
        );
        // And the hand-dug block is really in both, so the comparison is not
        // two empty worlds agreeing.
        assert_eq!(rebuilt.block(by_hand), vx_core::BlockId::AIR);
        assert_eq!(restored.block(by_hand), vx_core::BlockId::AIR);

        std::fs::remove_dir_all(&directory).ok();
    }
}

#[cfg(test)]
mod version_tests {
    use super::*;

    #[test]
    fn an_old_journal_restarts_the_oracle_instead_of_lying() {
        // A v3 log was recorded against a hometown without the player's house.
        // Replaying it over post-house terrain would diverge through no fault
        // of the log, so the loader refuses it and the fresh log declines
        // genesis coverage rather than claiming it.
        let directory = std::env::temp_dir().join(format!(
            "vx-journal-version-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();

        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 24]); // keyframe tick/hash/clock
        std::fs::write(directory.join("log.dat"), &bytes).unwrap();

        let log = CommandLog::load(&directory);
        std::fs::remove_dir_all(&directory).ok();

        assert!(log.is_empty(), "salvaged entries from a refused version");
        assert_ne!(
            log.keyframe_tick, 0,
            "a restarted log must not claim to reach back to genesis"
        );
    }
}

#[cfg(test)]
mod movement_replay_tests {
    use super::*;
    use crate::movement::{self, MoveCommand};
    use vx_core::ChunkPos;

    fn world() -> World {
        let mut world = World::new(2024);
        world.load_around(ChunkPos::new(0, 0), 2);
        world
    }

    /// A short recorded run: walk, sprint, jump, slide, stop.
    fn recorded() -> CommandLog {
        let mut journal = CommandLog::default();
        let east = std::f32::consts::FRAC_PI_2;
        let script: [(u16, u32); 5] = [
            (movement::FWD, 12),
            (movement::FWD | movement::SPRINT, 20),
            (movement::FWD | movement::SPRINT | movement::JUMP, 4),
            (movement::FWD | movement::SPRINT | movement::CROUCH, 16),
            (0, 8),
        ];
        for (bits, ticks) in script {
            let command = MoveCommand::looking(bits, east, 0.0).laden(40);
            journal.record(Command::moving(command));
            journal.record(Command::Advance { ticks });
        }
        journal
    }

    #[test]
    fn the_same_journal_walks_the_same_walk() {
        // The regression test the design note asked for, run against the log
        // that already existed rather than a fixture built beside it.
        let journal = recorded();
        let events = EventBus::new();

        let once = replay(&journal, &mut world(), &events);
        let twice = replay(&journal, &mut world(), &events);

        assert_eq!(once.player.position, twice.player.position);
        assert_eq!(once.player.velocity, twice.player.velocity);
        assert_eq!(once.movement.stance, twice.movement.stance);
        assert_eq!(once.movement.stamina, twice.movement.stamina);
    }

    #[test]
    fn a_journal_that_records_movement_actually_moves_the_player() {
        // Guards against the whole thing passing by moving nobody at all.
        let journal = recorded();
        let events = EventBus::new();
        let mut world = world();
        let start = {
            let ground = world.surface_y(0, 0).unwrap_or(64) as f32;
            glam::Vec3::new(0.5, ground, 0.5)
        };

        let walked = replay(&journal, &mut world, &events);

        let travelled = (walked.player.position - start).length();
        assert!(travelled > 3.0, "the replayed player only moved {travelled}");
    }

    #[test]
    fn splitting_an_advance_does_not_change_where_the_player_ends_up() {
        // `Advance` folds when recorded, so the same run can be logged as one
        // long entry or several short ones. Both must replay identically, or
        // the fold is silently lossy.
        let east = std::f32::consts::FRAC_PI_2;
        let command = MoveCommand::looking(movement::FWD | movement::SPRINT, east, 0.0);

        let mut whole = CommandLog::default();
        whole.record(Command::moving(command));
        whole.record(Command::Advance { ticks: 60 });

        let mut split = CommandLog::default();
        split.record(Command::moving(command));
        for _ in 0..12 {
            split.record(Command::Advance { ticks: 5 });
        }

        let events = EventBus::new();
        let a = replay(&whole, &mut world(), &events);
        let b = replay(&split, &mut world(), &events);
        assert_eq!(a.player.position, b.player.position);
    }

    #[test]
    fn a_move_survives_a_round_trip_through_the_file() {
        let directory = std::env::temp_dir().join(format!(
            "vx-journal-move-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();

        let journal = recorded();
        journal.save(&directory).unwrap();
        let read_back = CommandLog::load(&directory);
        std::fs::remove_dir_all(&directory).ok();

        let moves: Vec<_> = read_back
            .entries()
            .iter()
            .filter(|entry| matches!(entry.command, Command::Move { .. }))
            .map(|entry| entry.command.clone())
            .collect();
        assert_eq!(moves.len(), 5, "movement entries did not survive the file");
        assert!(
            matches!(moves[0], Command::Move { load: 40, .. }),
            "the load did not survive: {:?}",
            moves[0]
        );
    }
}

#[cfg(test)]
mod admin_tests {
    use super::*;
    use vx_core::ChunkPos;

    /// Radius four: the teleport fixtures land near the town's west edge, and
    /// an unloaded chunk reads as air — a body teleported into one falls
    /// forever instead of sprinting.
    fn world() -> World {
        let mut world = World::new(2024);
        world.load_around(ChunkPos::new(0, 0), 4);
        world
    }

    /// A session with cheats in it: teleport, a tuning drag, movement after.
    fn cheated() -> CommandLog {
        let mut journal = CommandLog::default();
        let east = std::f32::consts::FRAC_PI_2;
        journal.record(Command::Admin(Admin::Teleport { x: 8, y: 74, z: 8 }));
        journal.record(Command::Admin(Admin::SetTuning {
            key: "sprint_speed".into(),
            value: 9.0,
        }));
        journal.record(Command::moving(MoveCommand::looking(
            movement::FWD | movement::SPRINT,
            east,
            0.0,
        )));
        journal.record(Command::Advance { ticks: 40 });
        journal.record(Command::Admin(Admin::SetStat {
            key: "stamina".into(),
            value: 100,
        }));
        journal.record(Command::Advance { ticks: 8 });
        journal
    }

    #[test]
    fn a_journal_with_admin_commands_replays_to_the_same_place() {
        // The entire premise: the oracle covers cheated sessions.
        let journal = cheated();
        let events = EventBus::new();
        let once = replay(&journal, &mut world(), &events);
        let twice = replay(&journal, &mut world(), &events);
        assert_eq!(once.player.position, twice.player.position);
        assert_eq!(once.movement.stamina, twice.movement.stamina);
        assert_eq!(once.movement.tuning, twice.movement.tuning);
    }

    #[test]
    fn a_tuning_change_mid_log_changes_the_outcome_and_still_replays() {
        // The physics travels with the journal: the same held sprint under a
        // different sprint_speed lands somewhere else, reproducibly.
        let east = std::f32::consts::FRAC_PI_2;
        let with_tuning = |speed: Option<f32>| {
            let mut journal = CommandLog::default();
            // The lane at z = 20 crosses the whole flat town platform with no
            // building in it — a wall would clamp both runs to the same spot
            // and hide the difference this test exists to see.
            journal.record(Command::Admin(Admin::Teleport { x: -20, y: 74, z: 20 }));
            if let Some(value) = speed {
                journal.record(Command::Admin(Admin::SetTuning {
                    key: "sprint_speed".into(),
                    value,
                }));
            }
            journal.record(Command::moving(MoveCommand::looking(
                movement::FWD | movement::SPRINT,
                east,
                0.0,
            )));
            journal.record(Command::Advance { ticks: 20 });
            let events = EventBus::new();
            replay(&journal, &mut world(), &events).player.position
        };

        let stock = with_tuning(None);
        let tuned = with_tuning(Some(9.5));
        assert_ne!(stock, tuned, "the tuning order changed nothing");
        assert_eq!(
            with_tuning(Some(9.5)),
            tuned,
            "the tuned run is not reproducible"
        );
    }

    #[test]
    fn a_give_lands_in_the_base_pile_only_when_a_base_exists() {
        // The no-base rule must match live behaviour exactly, or the oracle
        // and the session disagree about how much ore you own.
        let events = EventBus::new();
        let mut journal = CommandLog::default();
        journal.record(Command::Admin(Admin::Give {
            good: "engine:copper_ore".into(),
            amount: 500,
        }));
        let rebuilt = replay(&journal, &mut world(), &events);
        assert!(
            rebuilt.mining.fleet.base.is_none(),
            "a base appeared from nowhere"
        );
    }

    #[test]
    fn admin_orders_survive_the_wire() {
        let directory = std::env::temp_dir().join(format!(
            "vx-journal-admin-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();

        let mut journal = CommandLog::default();
        let orders = [
            Admin::Give { good: "engine:log".into(), amount: 42 },
            Admin::SpawnMachine { kind: "drone".into(), count: 3 },
            Admin::Teleport { x: -14, y: 73, z: 9 },
            Admin::SetStat { key: "credits".into(), value: 9_999 },
            Admin::SetStock { x: 512, z: 0, good: "engine:stone".into(), amount: 700 },
            Admin::SetTuning { key: "friction_slide".into(), value: 1.234_567 },
        ];
        for order in &orders {
            journal.record(Command::Admin(order.clone()));
        }
        journal.save(&directory).unwrap();
        let read_back = CommandLog::load(&directory);
        std::fs::remove_dir_all(&directory).ok();

        let recovered: Vec<&Command> =
            read_back.entries().iter().map(|entry| &entry.command).collect();
        assert_eq!(recovered.len(), orders.len(), "orders were lost on the wire");
        for (found, sent) in recovered.iter().zip(&orders) {
            assert_eq!(**found, Command::Admin((*sent).clone()), "an order mutated in transit");
        }
    }

    #[test]
    fn a_teleport_is_block_exact_after_the_wire() {
        // f32 positions would drift through a file; blocks cannot.
        let events = EventBus::new();
        let mut journal = CommandLog::default();
        journal.record(Command::Admin(Admin::Teleport { x: 100, y: 80, z: -40 }));
        let rebuilt = replay(&journal, &mut world(), &events);
        assert_eq!(
            rebuilt.player.position,
            glam::Vec3::new(100.5, 80.0, -39.5),
            "the landing spot moved"
        );
    }
}

#[cfg(test)]
mod fire_tests {
    use super::*;
    use vx_core::ChunkPos;
    use vx_world::region_hash;

    fn world() -> World {
        let mut world = World::new(2024);
        world.load_around(ChunkPos::new(0, 0), 4);
        world
    }

    /// A `Fire` survives the wire bit-exactly, muzzle floats included.
    #[test]
    fn a_shot_survives_the_wire() {
        let directory = std::env::temp_dir().join(format!(
            "vx-journal-fire-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();

        let mut journal = CommandLog::default();
        let shot = Command::Fire {
            muzzle: [12.375, 81.62, -3.25],
            yaw_q: 1023,
            pitch_q: -77,
        };
        journal.record(shot.clone());
        journal.save(&directory).unwrap();
        let read_back = CommandLog::load(&directory);
        std::fs::remove_dir_all(&directory).ok();

        assert_eq!(read_back.entries().len(), 1);
        assert_eq!(read_back.entries()[0].command, shot, "the shot mutated in transit");
    }

    /// The oracle covers gunfire: a session that fires into the ground twice
    /// replays to the same craters, from the recorded orders alone.
    #[test]
    fn a_firefight_replays_to_the_same_ground() {
        let fight = |world: &mut World| -> u64 {
            let events = EventBus::new();
            let mut journal = CommandLog::default();
            let mut rebuilt = Rebuilt::default();

            let ground = world.generator().height_at(10, 10) as f32;
            // Stand above open ground and fire down at an angle, twice, with
            // ticks between: each shot arcs into the dirt and craters it.
            let muzzle = [10.5, ground + 8.0, 10.5];
            for (yaw_q, pitch_q, wait) in [(0i16, -700i16, 6u32), (1024, -650, 8)] {
                let shot = Command::Fire { muzzle, yaw_q, pitch_q };
                apply(&shot, world, &events, &mut rebuilt);
                journal.record(shot);
                let advance = Command::Advance { ticks: wait };
                apply(&advance, world, &events, &mut rebuilt);
                journal.record(advance);
            }
            assert!(
                rebuilt.shots.is_empty(),
                "both rounds should have landed inside the waits"
            );
            let hash_live = region_hash(
                world,
                vx_core::BlockPos::new(-20, 40, -20),
                vx_core::BlockPos::new(40, 100, 40),
            );

            // Now the other side of the oracle: a fresh world, orders only.
            let mut fresh = super::fire_tests::world();
            replay(&journal, &mut fresh, &EventBus::new());
            let hash_replayed = region_hash(
                &fresh,
                vx_core::BlockPos::new(-20, 40, -20),
                vx_core::BlockPos::new(40, 100, 40),
            );
            assert_eq!(hash_live, hash_replayed, "the craters diverged");
            hash_live
        };

        let first = fight(&mut world());
        let second = fight(&mut world());
        assert_eq!(first, second, "the firefight itself is not deterministic");
    }

    /// Every scout order survives the wire, and a log full of them replays
    /// to the same ground as a log with none — the scout charts nothing and
    /// breaks nothing, so the oracle must not see it.
    #[test]
    fn scout_orders_round_trip_and_replay_to_unchanged_ground() {
        let directory = std::env::temp_dir().join(format!(
            "vx-journal-scout-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();

        let orders = [
            ScoutOrder::Orbit,
            ScoutOrder::Sortie { x: -40, z: 120 },
            ScoutOrder::Perch { x: 7, z: -3 },
            ScoutOrder::Vanguard,
            ScoutOrder::Dock,
        ];
        let mut journal = CommandLog::default();
        for order in orders {
            journal.record(Command::Scout(order));
            journal.record(Command::Advance { ticks: 5 });
        }
        journal.save(&directory).unwrap();
        let read_back = CommandLog::load(&directory);
        std::fs::remove_dir_all(&directory).ok();

        let recovered: Vec<&Command> = read_back
            .entries()
            .iter()
            .map(|entry| &entry.command)
            .filter(|command| matches!(command, Command::Scout(_)))
            .collect();
        assert_eq!(recovered.len(), orders.len(), "orders were lost on the wire");
        for (found, sent) in recovered.iter().zip(&orders) {
            assert_eq!(**found, Command::Scout(*sent), "an order mutated in transit");
        }

        // Replay it against fresh ground, and replay the same ticks with no
        // scout orders at all: identical hashes, or the scout lied.
        let span = (
            vx_core::BlockPos::new(-20, 40, -20),
            vx_core::BlockPos::new(40, 100, 40),
        );
        let events = EventBus::new();
        let mut scouted = world();
        replay(&read_back, &mut scouted, &events);
        let mut quiet_log = CommandLog::default();
        quiet_log.record(Command::Advance { ticks: 25 });
        let mut quiet = world();
        replay(&quiet_log, &mut quiet, &events);
        assert_eq!(
            region_hash(&scouted, span.0, span.1),
            region_hash(&quiet, span.0, span.1),
            "scout orders changed the ground"
        );
    }

    /// Intrusion orders survive the wire, and a log full of them replays to
    /// the same ground as a log with none. What a hack moves is grants and
    /// claims — permits state, its own file — so if this ever fails, an
    /// intrusion has started touching blocks and the oracle needs telling.
    #[test]
    fn intrusion_orders_round_trip_and_replay_to_unchanged_ground() {
        let directory = std::env::temp_dir().join(format!(
            "vx-journal-intrude-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();

        let orders = [
            IntrudeOrder::Lock { x: 12, y: 74, z: -3 },
            IntrudeOrder::Roost {
                x: 14,
                y: 76,
                z: 9,
                grade: crate::intrusion::Grade::Tap,
            },
            IntrudeOrder::Roost {
                x: 14,
                y: 76,
                z: 9,
                grade: crate::intrusion::Grade::Blind,
            },
            IntrudeOrder::Abort,
        ];
        let mut journal = CommandLog::default();
        for order in orders {
            journal.record(Command::Intrude(order));
            journal.record(Command::Advance { ticks: 4 });
        }
        journal.save(&directory).unwrap();
        let read_back = CommandLog::load(&directory);
        std::fs::remove_dir_all(&directory).ok();

        let recovered: Vec<&Command> = read_back
            .entries()
            .iter()
            .map(|entry| &entry.command)
            .filter(|command| matches!(command, Command::Intrude(_)))
            .collect();
        assert_eq!(recovered.len(), orders.len(), "orders were lost on the wire");
        for (found, sent) in recovered.iter().zip(&orders) {
            assert_eq!(**found, Command::Intrude(*sent), "an order mutated in transit");
        }

        let span = (
            vx_core::BlockPos::new(-20, 40, -20),
            vx_core::BlockPos::new(40, 100, 40),
        );
        let events = EventBus::new();
        let mut hacked = world();
        replay(&read_back, &mut hacked, &events);
        let mut quiet_log = CommandLog::default();
        quiet_log.record(Command::Advance { ticks: 16 });
        let mut quiet = world();
        replay(&quiet_log, &mut quiet, &events);
        assert_eq!(
            region_hash(&hacked, span.0, span.1),
            region_hash(&quiet, span.0, span.1),
            "an intrusion changed the ground"
        );
    }

    /// Firing queues recoil, and the next `Advance` applies it: the replayed
    /// body ends up shoved off where an unfired one stands.
    #[test]
    fn recoil_replays_into_the_body() {
        let events = EventBus::new();

        let mut quiet = CommandLog::default();
        quiet.record(Command::Advance { ticks: 8 });
        let stood = replay(&quiet, &mut world(), &events);

        let mut loud = CommandLog::default();
        // Fire level, due south (+Z is yaw_q of half the circle).
        loud.record(Command::Fire {
            muzzle: [0.5, 80.0, 0.5],
            yaw_q: 2048,
            pitch_q: 0,
        });
        loud.record(Command::Advance { ticks: 8 });
        let shoved = replay(&loud, &mut world(), &events);

        assert_ne!(
            stood.player.position, shoved.player.position,
            "recoil never reached the replayed body"
        );
    }
}
