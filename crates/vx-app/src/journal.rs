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
use vx_world::World;

use crate::mining::Mining;

const MAGIC: &[u8; 4] = b"VXLG";
const VERSION: u32 = 1;

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
    /// An excavation ordered: the marked area and the chosen method.
    Dispatch { area: VoxelAabb, method: MineMethod },
    /// The plan abandoned.
    Cancel,
    /// Simulation ticks run. The unit of time the log speaks in.
    Advance { ticks: u32 },
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
                    "could not read {}: {error}; the world will load from its last keyframe \
                     and anything ordered since is lost",
                    path.display()
                );
                CommandLog::new()
            }
        }
    }
}

/// Replay a log over a world, returning the mining state it rebuilt.
///
/// The world must already be at the log's keyframe — region files loaded, or a
/// bare generated world when `keyframe_tick` is zero.
pub fn replay(log: &CommandLog, world: &mut World, events: &EventBus) -> Mining {
    let mut mining = Mining::default();
    for entry in log.entries() {
        apply(&entry.command, world, events, &mut mining);
    }
    mining
}

fn apply(command: &Command, world: &mut World, events: &EventBus, mining: &mut Mining) {
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
        Command::Dispatch { area, method } => {
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
            mining.start(world);
        }
        Command::Cancel => mining.cancel(world),
        Command::Advance { ticks } => {
            mining.advance(world, events, *ticks);
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
        Command::Dispatch { area, method } => {
            file.write_all(&[2u8])?;
            write_pos(file, area.min)?;
            write_pos(file, area.max)?;
            file.write_all(&[match method {
                MineMethod::Adit => 0,
                MineMethod::Decline => 1,
                MineMethod::Pit => 2,
            }])?;
        }
        Command::Cancel => file.write_all(&[3u8])?,
        Command::Advance { ticks } => {
            file.write_all(&[4u8])?;
            file.write_all(&ticks.to_le_bytes())?;
        }
    }
    Ok(())
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
            }
        }
        3 => Command::Cancel,
        4 => Command::Advance {
            ticks: read_u32(file)?,
        },
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
        let mut mining = Mining::default();

        let dispatch = Command::Dispatch {
            area,
            method: MineMethod::Pit,
        };
        apply(&dispatch, &mut world, &events, &mut mining);
        journal.record(dispatch);

        // Frames of wildly different lengths, as a real session has.
        for ticks in [1u32, 7, 3, 16, 2, 11, 16, 16, 9, 4] {
            mining.advance(&mut world, &events, ticks);
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
        let mut mining = Mining::default();

        let dispatch = Command::Dispatch {
            area,
            method: MineMethod::Pit,
        };
        apply(&dispatch, &mut world, &events, &mut mining);
        journal.record(dispatch);
        for ticks in [5u32, 16, 16, 16, 11] {
            mining.advance(&mut world, &events, ticks);
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
        let mut mining = Mining::default();

        let dispatch = Command::Dispatch {
            area,
            method: MineMethod::Pit,
        };
        apply(&dispatch, &mut world, &events, &mut mining);
        journal.record(dispatch);
        // Frames of wildly different lengths, as a real session has.
        for _ in 0..200 {
            for ticks in [1u32, 7, 3, 16, 2, 11, 16, 9] {
                mining.advance(&mut world, &events, ticks);
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
        let mut mining = Mining::default();

        let dispatch = Command::Dispatch {
            area,
            method: MineMethod::Pit,
        };
        apply(&dispatch, &mut world, &events, &mut mining);
        journal.record(dispatch);
        for _ in 0..40 {
            mining.advance(&mut world, &events, 16);
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
