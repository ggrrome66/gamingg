//! The simulation clock and the scheduled-tick queue.
//!
//! Simulation runs at a fixed rate, decoupled from the frame rate, so world
//! behaviour is identical on a 30 fps laptop and a 240 fps desktop. A renderer
//! that skips a frame looks briefly worse; a simulation that skips a step
//! diverges.
//!
//! # Everything here is bounded on purpose
//!
//! A tick system is the engine's most inviting denial-of-service surface,
//! because the work it does is driven by world content rather than by anything
//! the player directly asks for. Three failure modes are designed against, and
//! each has a test that shows the bound holding:
//!
//! - **Runaway catch-up.** Suspend the process — sleep, a debugger, a long GC
//!   pause — and the next frame reports an enormous elapsed time. Naively that
//!   demands thousands of steps at once, which takes longer than the frame it
//!   is trying to catch up on, so the next frame owes even more. The classic
//!   spiral of death. [`TickClock::advance`] caps the steps and *discards* the
//!   remaining debt rather than carrying it.
//! - **Queue flooding.** A tick handler may schedule more ticks, so a chain
//!   reaction is amplification: one edit becoming exponentially many. The queue
//!   has a hard ceiling and refuses work past it rather than allocating.
//! - **Overflow.** A due time is `now + delay`. Wrap that and the tick is due
//!   in the past, so it fires immediately, forever. Delays are capped and the
//!   arithmetic saturates.
//!
//! Refusals are counted rather than silently swallowed, so the condition is
//! visible instead of just being survived.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::time::Duration;

use vx_core::BlockPos;

/// Simulation steps per second.
pub const TICKS_PER_SECOND: u32 = 20;

/// Ceilings on the work one step may create or perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickLimits {
    /// Scheduled ticks that may be outstanding at once.
    pub max_pending: usize,
    /// Scheduled ticks executed in one step. Work past this waits for the
    /// next step rather than extending this one.
    pub max_per_step: usize,
    /// Steps run in one frame, bounding catch-up.
    pub max_catchup_steps: u32,
    /// Furthest ahead anything may be scheduled, in ticks.
    pub max_delay: u64,
    /// Block updates processed in one step.
    pub max_updates_per_step: usize,
    /// Block updates that may be outstanding at once.
    pub max_pending_updates: usize,
}

impl Default for TickLimits {
    fn default() -> Self {
        TickLimits {
            // Roughly a large machine room's worth of pending work. Reaching
            // this means something is wrong, not that the player is busy.
            max_pending: 65_536,
            max_per_step: 4_096,
            // Half a second of catch-up. Beyond that the world is better off
            // skipping time than freezing to simulate it.
            max_catchup_steps: 10,
            // An hour at 20 TPS. Nothing legitimate schedules further out.
            max_delay: 72_000,
            max_updates_per_step: 8_192,
            max_pending_updates: 131_072,
        }
    }
}

/// A fixed-step clock driven by real elapsed time.
#[derive(Debug, Clone)]
pub struct TickClock {
    step: Duration,
    accumulator: Duration,
    elapsed_ticks: u64,
    /// Steps dropped to stay inside the catch-up cap.
    skipped: u64,
}

impl TickClock {
    pub fn new(ticks_per_second: u32) -> Self {
        let rate = ticks_per_second.max(1);
        TickClock {
            step: Duration::from_nanos(1_000_000_000 / rate as u64),
            accumulator: Duration::ZERO,
            elapsed_ticks: 0,
            skipped: 0,
        }
    }

    pub fn step(&self) -> Duration {
        self.step
    }

    pub fn elapsed_ticks(&self) -> u64 {
        self.elapsed_ticks
    }

    /// Steps abandoned rather than run, for the diagnostics readout.
    pub fn skipped(&self) -> u64 {
        self.skipped
    }

    /// Fraction through the current step, for interpolating rendering later.
    pub fn alpha(&self) -> f32 {
        self.accumulator.as_secs_f32() / self.step.as_secs_f32()
    }

    /// Take `elapsed` real time and report how many steps to run.
    ///
    /// Never returns more than `max_steps`. Whatever is still owed past that
    /// is thrown away, not banked: carrying it forward is exactly the spiral
    /// this exists to prevent, because the next frame would owe that debt plus
    /// its own.
    pub fn advance(&mut self, elapsed: Duration, max_steps: u32) -> u32 {
        self.accumulator = self.accumulator.saturating_add(elapsed);

        // Nanoseconds are counted in u128, and a long enough stall divides out
        // to more steps than a u64 can hold. `as u64` would wrap that to a
        // small number and quietly run the wrong amount of simulation, so
        // saturate instead.
        let owed = u64::try_from(self.accumulator.as_nanos() / self.step.as_nanos())
            .unwrap_or(u64::MAX);
        let steps = owed.min(max_steps as u64);

        self.accumulator -= self.step * steps as u32;
        if owed > steps {
            self.skipped = self.skipped.saturating_add(owed - steps);
            // Drop the rest of the debt outright.
            self.accumulator = Duration::ZERO;
        }

        self.elapsed_ticks += steps;
        steps as u32
    }
}

/// One queued tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Scheduled {
    due: u64,
    pos: BlockPos,
}

// Ordered by due time, then by position. The position tiebreak is not
// cosmetic: a heap gives no order among equal keys, so without it two ticks
// due on the same step could run in either order and the simulation would stop
// being reproducible from a seed.
impl Ord for Scheduled {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.due.cmp(&other.due).then_with(|| self.pos.cmp(&other.pos))
    }
}

impl PartialOrd for Scheduled {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Why a schedule request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// That position already has a tick pending.
    AlreadyQueued,
    /// The queue is at its ceiling.
    QueueFull,
    /// The delay was beyond [`TickLimits::max_delay`].
    DelayTooLong,
}

/// Pending scheduled ticks, ordered by when they are due.
#[derive(Debug, Clone, Default)]
pub struct TickScheduler {
    now: u64,
    queue: BinaryHeap<Reverse<Scheduled>>,
    /// Positions already queued. Without this, repeatedly scheduling one
    /// position fills the queue on its own — no amplification needed.
    queued: HashSet<BlockPos>,
    refused: u64,
}

impl TickScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn now(&self) -> u64 {
        self.now
    }

    pub fn pending(&self) -> usize {
        self.queue.len()
    }

    /// How many schedule requests have been turned away. Non-zero means a
    /// limit is being hit and is worth surfacing.
    pub fn refused(&self) -> u64 {
        self.refused
    }

    pub fn is_queued(&self, pos: BlockPos) -> bool {
        self.queued.contains(&pos)
    }

    /// Queue `pos` to tick in `delay` steps.
    ///
    /// Returns the refusal reason rather than a bare bool so a caller can tell
    /// "already handled" from "the engine is out of room".
    pub fn schedule(
        &mut self,
        pos: BlockPos,
        delay: u64,
        limits: &TickLimits,
    ) -> Result<(), Refusal> {
        if delay > limits.max_delay {
            self.refused += 1;
            return Err(Refusal::DelayTooLong);
        }
        if self.queued.contains(&pos) {
            // Not counted as a refusal: asking twice for the same work is
            // ordinary, not a symptom.
            return Err(Refusal::AlreadyQueued);
        }
        if self.queue.len() >= limits.max_pending {
            self.refused += 1;
            return Err(Refusal::QueueFull);
        }

        // Saturating, so a due time can never wrap into the past and re-fire
        // for the rest of the session.
        let due = self.now.saturating_add(delay);
        self.queue.push(Reverse(Scheduled { due, pos }));
        self.queued.insert(pos);
        Ok(())
    }

    /// Move to the next step and take up to `max` ticks now due.
    ///
    /// Anything due but over budget stays queued and comes back next step, so
    /// a backlog slows the world down rather than stalling the frame.
    pub fn advance(&mut self, max: usize) -> Vec<BlockPos> {
        self.now = self.now.saturating_add(1);
        self.take_due(max)
    }

    /// Ticks due at the current step, without advancing.
    pub fn take_due(&mut self, max: usize) -> Vec<BlockPos> {
        let mut ready = Vec::new();
        while ready.len() < max {
            match self.queue.peek() {
                Some(Reverse(next)) if next.due <= self.now => {
                    let Reverse(scheduled) = self.queue.pop().expect("just peeked");
                    self.queued.remove(&scheduled.pos);
                    ready.push(scheduled.pos);
                }
                _ => break,
            }
        }
        ready
    }

    /// Forget everything queued inside a chunk, for when it unloads.
    ///
    /// Without this the queue keeps growing as the player travels, holding
    /// work for chunks that are no longer resident.
    pub fn forget_chunk(&mut self, chunk: vx_core::ChunkPos) {
        if self.queued.is_empty() {
            return;
        }
        self.queued.retain(|pos| pos.chunk() != chunk);
        // The heap cannot be filtered in place; rebuild it from what survived.
        let kept: Vec<Reverse<Scheduled>> = self
            .queue
            .drain()
            .filter(|Reverse(scheduled)| self.queued.contains(&scheduled.pos))
            .collect();
        self.queue = kept.into_iter().collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::ChunkPos;

    fn limits() -> TickLimits {
        TickLimits::default()
    }

    #[test]
    fn the_clock_runs_one_step_per_interval() {
        let mut clock = TickClock::new(20);
        assert_eq!(clock.advance(Duration::from_millis(50), 10), 1);
        assert_eq!(clock.advance(Duration::from_millis(100), 10), 2);
        assert_eq!(clock.elapsed_ticks(), 3);
    }

    #[test]
    fn time_shorter_than_a_step_accumulates_instead_of_being_lost() {
        let mut clock = TickClock::new(20);
        // Four 20 ms frames are 80 ms, which is one whole step and a bit.
        let mut steps = 0;
        for _ in 0..4 {
            steps += clock.advance(Duration::from_millis(20), 10);
        }
        assert_eq!(steps, 1);
        assert!(clock.alpha() > 0.0, "the remainder was discarded");
    }

    #[test]
    fn a_long_stall_is_capped_and_the_debt_is_dropped() {
        // The spiral of death: a suspended process reports a huge elapsed
        // time. Running it all would take longer than the frame it is catching
        // up on, so the next frame owes even more.
        let mut clock = TickClock::new(20);

        let steps = clock.advance(Duration::from_secs(60), 10);

        assert_eq!(steps, 10, "catch-up was not capped");
        assert!(clock.skipped() > 1_000, "the debt was not dropped");

        // The decisive part: the very next frame must be back to normal, not
        // still working off an hour of backlog.
        assert_eq!(clock.advance(Duration::from_millis(50), 10), 1);
    }

    #[test]
    fn an_absurd_elapsed_time_cannot_overflow_the_accumulator() {
        let mut clock = TickClock::new(20);
        assert_eq!(clock.advance(Duration::MAX, 10), 10);
        assert_eq!(clock.advance(Duration::MAX, 10), 10);
        // Still responsive afterwards.
        assert_eq!(clock.advance(Duration::from_millis(50), 10), 1);
    }

    #[test]
    fn a_zero_rate_does_not_divide_by_zero() {
        let mut clock = TickClock::new(0);
        assert!(clock.step() > Duration::ZERO);
        assert!(clock.advance(Duration::from_secs(1), 10) <= 10);
    }

    #[test]
    fn a_scheduled_tick_comes_back_when_it_is_due() {
        let mut scheduler = TickScheduler::new();
        let pos = BlockPos::new(1, 2, 3);
        scheduler.schedule(pos, 2, &limits()).unwrap();

        assert!(scheduler.advance(16).is_empty(), "fired a step early");
        assert_eq!(scheduler.advance(16), vec![pos]);
        assert_eq!(scheduler.pending(), 0);
    }

    #[test]
    fn a_zero_delay_tick_fires_on_the_next_step() {
        let mut scheduler = TickScheduler::new();
        let pos = BlockPos::new(0, 0, 0);
        scheduler.schedule(pos, 0, &limits()).unwrap();
        assert_eq!(scheduler.advance(16), vec![pos]);
    }

    #[test]
    fn ticks_come_back_in_a_deterministic_order() {
        // A heap gives no order among equal keys, so without the position
        // tiebreak two ticks due on the same step could run either way round
        // and the simulation would stop being reproducible.
        let order = |seed: &[BlockPos]| {
            let mut scheduler = TickScheduler::new();
            for pos in seed {
                scheduler.schedule(*pos, 1, &limits()).unwrap();
            }
            scheduler.advance(16)
        };

        let forwards = [
            BlockPos::new(0, 0, 0),
            BlockPos::new(5, 0, 0),
            BlockPos::new(0, 9, 0),
        ];
        let mut backwards = forwards;
        backwards.reverse();

        assert_eq!(order(&forwards), order(&backwards));
    }

    #[test]
    fn earlier_ticks_come_back_before_later_ones() {
        let mut scheduler = TickScheduler::new();
        let soon = BlockPos::new(1, 0, 0);
        let later = BlockPos::new(2, 0, 0);
        scheduler.schedule(later, 5, &limits()).unwrap();
        scheduler.schedule(soon, 1, &limits()).unwrap();

        assert_eq!(scheduler.advance(16), vec![soon]);
        for _ in 0..3 {
            assert!(scheduler.advance(16).is_empty());
        }
        assert_eq!(scheduler.advance(16), vec![later]);
    }

    #[test]
    fn scheduling_the_same_position_twice_queues_it_once() {
        // Without the dedup set, a loop scheduling one position fills the
        // queue with no amplification needed at all.
        let mut scheduler = TickScheduler::new();
        let pos = BlockPos::new(4, 4, 4);

        scheduler.schedule(pos, 3, &limits()).unwrap();
        for _ in 0..10_000 {
            assert_eq!(scheduler.schedule(pos, 3, &limits()), Err(Refusal::AlreadyQueued));
        }

        assert_eq!(scheduler.pending(), 1);
        // Repeats are ordinary, so they are not counted as a symptom.
        assert_eq!(scheduler.refused(), 0);
    }

    #[test]
    fn a_full_queue_refuses_work_rather_than_growing() {
        let limits = TickLimits {
            max_pending: 64,
            ..TickLimits::default()
        };
        let mut scheduler = TickScheduler::new();

        for x in 0..64 {
            scheduler.schedule(BlockPos::new(x, 0, 0), 1, &limits).unwrap();
        }
        for x in 64..2_000 {
            assert_eq!(
                scheduler.schedule(BlockPos::new(x, 0, 0), 1, &limits),
                Err(Refusal::QueueFull)
            );
        }

        assert_eq!(scheduler.pending(), 64, "the ceiling did not hold");
        assert!(scheduler.refused() > 0, "refusals were not counted");
    }

    #[test]
    fn an_overlong_delay_is_refused() {
        let mut scheduler = TickScheduler::new();
        let limits = limits();

        assert_eq!(
            scheduler.schedule(BlockPos::new(0, 0, 0), limits.max_delay + 1, &limits),
            Err(Refusal::DelayTooLong)
        );
        assert_eq!(scheduler.pending(), 0);
    }

    #[test]
    fn a_due_time_cannot_wrap_into_the_past() {
        // Wrapping would make the tick permanently overdue, so it would fire
        // every single step for the rest of the session.
        let mut scheduler = TickScheduler::new();
        scheduler.now = u64::MAX / 2;

        let limits = TickLimits {
            max_delay: u64::MAX,
            ..TickLimits::default()
        };
        scheduler
            .schedule(BlockPos::new(0, 0, 0), u64::MAX, &limits)
            .unwrap();

        // Wrapping would land this at roughly `now - 1`, permanently overdue,
        // so it would fire every step forever. Saturating puts it at the end
        // of time instead, where it simply never comes due.
        for _ in 0..8 {
            assert!(scheduler.advance(16).is_empty());
        }
        assert_eq!(scheduler.pending(), 1);
    }

    #[test]
    fn the_step_budget_defers_work_instead_of_dropping_it() {
        // A backlog should slow the world down, not lose it.
        let mut scheduler = TickScheduler::new();
        for x in 0..100 {
            scheduler.schedule(BlockPos::new(x, 0, 0), 1, &limits()).unwrap();
        }

        let first = scheduler.advance(30);
        assert_eq!(first.len(), 30);
        assert_eq!(scheduler.pending(), 70, "the rest was discarded");

        let second = scheduler.take_due(1_000);
        assert_eq!(second.len(), 70);
        assert_eq!(scheduler.pending(), 0);
    }

    #[test]
    fn a_position_can_be_rescheduled_once_it_has_fired() {
        // Falling blocks depend on this: each step of a fall schedules the
        // next one from the same handler.
        let mut scheduler = TickScheduler::new();
        let pos = BlockPos::new(0, 100, 0);

        scheduler.schedule(pos, 1, &limits()).unwrap();
        assert_eq!(scheduler.advance(16), vec![pos]);
        assert!(!scheduler.is_queued(pos));
        scheduler.schedule(pos, 1, &limits()).unwrap();
        assert_eq!(scheduler.advance(16), vec![pos]);
    }

    #[test]
    fn unloading_a_chunk_forgets_its_pending_work() {
        // Otherwise the queue grows without bound as the player travels, full
        // of work for chunks that are no longer resident.
        let mut scheduler = TickScheduler::new();
        let staying = BlockPos::new(0, 0, 0);
        let leaving = BlockPos::new(100, 0, 100);

        scheduler.schedule(staying, 5, &limits()).unwrap();
        scheduler.schedule(leaving, 5, &limits()).unwrap();

        scheduler.forget_chunk(leaving.chunk());

        assert_eq!(scheduler.pending(), 1);
        assert!(scheduler.is_queued(staying));
        assert!(!scheduler.is_queued(leaving));

        // And the survivor still fires on time.
        for _ in 0..4 {
            assert!(scheduler.advance(16).is_empty());
        }
        assert_eq!(scheduler.advance(16), vec![staying]);
    }

    #[test]
    fn forgetting_a_chunk_with_nothing_in_it_is_harmless() {
        let mut scheduler = TickScheduler::new();
        scheduler.forget_chunk(ChunkPos::new(3, 3));
        assert_eq!(scheduler.pending(), 0);

        scheduler.schedule(BlockPos::new(0, 0, 0), 1, &limits()).unwrap();
        scheduler.forget_chunk(ChunkPos::new(9, 9));
        assert_eq!(scheduler.pending(), 1);
    }
}
