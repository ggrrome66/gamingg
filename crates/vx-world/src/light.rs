//! Per-block light levels.
//!
//! Two channels, four bits each. **Sky** is daylight reaching a block from
//! above; **block** is light emitted by the world itself, torches and the like.
//! They are kept apart rather than summed because a day/night cycle dims one
//! and not the other, and merging them now would make that impossible later
//! without recomputing every chunk.
//!
//! Light is *derived* state: a pure function of the blocks around it. It is
//! recomputed when a chunk loads rather than written to disk, which keeps
//! saves smaller and — more usefully — means there is no lighting data on disk
//! for a corrupt or hostile file to lie about. The nibble packing also makes
//! an out-of-range level unrepresentable rather than merely unlikely.
//!
//! Propagation is a breadth-first flood fill with a hard work ceiling. One
//! edit can in principle relight an enormous volume — knock a hole in a cave
//! roof and daylight pours through a cavern system — so the budget bounds what
//! any single operation can cost.

use std::collections::VecDeque;

use vx_core::{BlockPos, CHUNK_VOLUME};

/// Brightest a channel can be. Four bits, so this is also the ceiling the
/// packing itself enforces.
pub const MAX_LIGHT: u8 = 15;

/// Most blocks one relight may visit.
///
/// Reached only by something pathological — the ordinary case for a single
/// block edit is a few hundred. Hitting it leaves lighting subtly wrong in a
/// large cavern rather than stalling the frame, which is the right way round.
pub const RELIGHT_BUDGET: usize = 96_000;

/// Packed light for one chunk: one byte per block, sky in the high nibble and
/// block light in the low.
///
/// A byte per block rather than a shared nibble array. It doubles the memory
/// against the tightest possible packing, but every read and write is a single
/// indexed byte with no shifting between neighbours, and lighting is read far
/// more often than it is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightGrid {
    data: Vec<u8>,
}

impl Default for LightGrid {
    fn default() -> Self {
        Self::dark()
    }
}

impl LightGrid {
    /// A fully unlit chunk.
    pub fn dark() -> Self {
        LightGrid {
            data: vec![0; CHUNK_VOLUME],
        }
    }

    /// Reset everything to darkness, keeping the allocation.
    pub fn clear(&mut self) {
        self.data.fill(0);
    }

    /// Daylight reaching this block.
    pub fn sky(&self, index: usize) -> u8 {
        self.data[index] >> 4
    }

    /// Light emitted by the world at this block.
    pub fn block(&self, index: usize) -> u8 {
        self.data[index] & 0x0f
    }

    /// The brighter of the two channels, which is what shading uses.
    pub fn brightest(&self, index: usize) -> u8 {
        self.sky(index).max(self.block(index))
    }

    /// Both channels packed as they are stored.
    pub fn packed(&self, index: usize) -> u8 {
        self.data[index]
    }

    /// Clamping, so no caller can store a value the nibble cannot hold.
    pub fn set_sky(&mut self, index: usize, value: u8) {
        let value = value.min(MAX_LIGHT);
        self.data[index] = (self.data[index] & 0x0f) | (value << 4);
    }

    pub fn set_block(&mut self, index: usize, value: u8) {
        let value = value.min(MAX_LIGHT);
        self.data[index] = (self.data[index] & 0xf0) | value;
    }

    /// True when nothing is lit, so an all-dark chunk can skip work.
    pub fn is_dark(&self) -> bool {
        self.data.iter().all(|packed| *packed == 0)
    }
}

/// Which channel a propagation pass is filling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Sky,
    Block,
}

/// A block waiting to spread its light to its neighbours.
#[derive(Debug, Clone, Copy)]
pub struct Pending {
    pub pos: BlockPos,
    pub level: u8,
}

/// The frontier of a flood fill, plus the budget it is allowed to spend.
#[derive(Debug, Default)]
pub struct LightQueue {
    queue: VecDeque<Pending>,
    visited: usize,
}

impl LightQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, pos: BlockPos, level: u8) {
        self.queue.push_back(Pending {
            pos,
            level: level.min(MAX_LIGHT),
        });
    }

    /// Take the next block to spread from, or `None` when the fill is done or
    /// has spent its budget.
    pub fn pop(&mut self, budget: usize) -> Option<Pending> {
        if self.visited >= budget {
            return None;
        }
        let next = self.queue.pop_front()?;
        self.visited += 1;
        Some(next)
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn visited(&self) -> usize {
        self.visited
    }

    /// True when the fill stopped because it ran out of budget rather than
    /// because it finished. Worth surfacing: the result is incomplete.
    pub fn exhausted(&self, budget: usize) -> bool {
        self.visited >= budget && !self.queue.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_grid_is_completely_dark() {
        let grid = LightGrid::dark();
        assert!(grid.is_dark());
        for index in [0, 1, CHUNK_VOLUME / 2, CHUNK_VOLUME - 1] {
            assert_eq!(grid.sky(index), 0);
            assert_eq!(grid.block(index), 0);
            assert_eq!(grid.brightest(index), 0);
        }
    }

    #[test]
    fn the_two_channels_are_independent() {
        // Sharing a byte must not let one channel corrupt the other.
        let mut grid = LightGrid::dark();
        grid.set_sky(10, 15);
        grid.set_block(10, 3);

        assert_eq!(grid.sky(10), 15);
        assert_eq!(grid.block(10), 3);

        grid.set_sky(10, 0);
        assert_eq!(grid.block(10), 3, "clearing sky wiped the block channel");

        grid.set_block(10, 0);
        grid.set_sky(10, 7);
        assert_eq!(grid.block(10), 0, "setting sky refilled the block channel");
    }

    #[test]
    fn brightest_reports_whichever_channel_wins() {
        let mut grid = LightGrid::dark();
        grid.set_sky(0, 4);
        grid.set_block(0, 9);
        assert_eq!(grid.brightest(0), 9);

        grid.set_sky(0, 12);
        assert_eq!(grid.brightest(0), 12);
    }

    #[test]
    fn levels_above_the_maximum_are_clamped_not_wrapped() {
        // The nibble cannot hold more than 15; wrapping would turn a bright
        // light into a dark one, which is the wrong failure.
        let mut grid = LightGrid::dark();
        grid.set_sky(0, 200);
        grid.set_block(0, u8::MAX);

        assert_eq!(grid.sky(0), MAX_LIGHT);
        assert_eq!(grid.block(0), MAX_LIGHT);
        // And nothing leaked between the nibbles doing it.
        assert_eq!(grid.packed(0), 0xff);
    }

    #[test]
    fn clearing_returns_the_grid_to_darkness() {
        let mut grid = LightGrid::dark();
        grid.set_sky(5, 15);
        grid.set_block(9, 7);
        assert!(!grid.is_dark());

        grid.clear();
        assert!(grid.is_dark());
    }

    #[test]
    fn a_queue_hands_back_what_was_pushed_in_order() {
        // Breadth-first order is what makes the fill correct: a nearer,
        // brighter source must be processed before a further, dimmer one.
        let mut queue = LightQueue::new();
        for x in 0..4 {
            queue.push(BlockPos::new(x, 0, 0), 15);
        }

        let mut seen = Vec::new();
        while let Some(pending) = queue.pop(100) {
            seen.push(pending.pos.x);
        }
        assert_eq!(seen, vec![0, 1, 2, 3]);
    }

    #[test]
    fn queued_levels_are_clamped_on_the_way_in() {
        let mut queue = LightQueue::new();
        queue.push(BlockPos::new(0, 0, 0), 99);
        assert_eq!(queue.pop(10).unwrap().level, MAX_LIGHT);
    }

    #[test]
    fn a_fill_stops_at_its_budget_rather_than_running_to_completion() {
        // One edit can in principle relight a whole cavern system. The budget
        // is what stops that being unbounded work inside a single frame.
        let mut queue = LightQueue::new();
        for x in 0..1_000 {
            queue.push(BlockPos::new(x, 0, 0), 15);
        }

        let budget = 50;
        let mut popped = 0;
        while queue.pop(budget).is_some() {
            popped += 1;
        }

        assert_eq!(popped, budget);
        assert!(queue.exhausted(budget), "the fill did not report being cut short");
        assert!(!queue.is_empty(), "the remainder was discarded");
    }

    #[test]
    fn a_fill_that_finishes_is_not_reported_as_exhausted() {
        let mut queue = LightQueue::new();
        queue.push(BlockPos::new(0, 0, 0), 15);
        while queue.pop(100).is_some() {}

        assert!(!queue.exhausted(100));
        assert!(queue.is_empty());
        assert_eq!(queue.visited(), 1);
    }

    #[test]
    fn an_empty_queue_pops_nothing_and_spends_nothing() {
        let mut queue = LightQueue::new();
        assert!(queue.pop(100).is_none());
        assert_eq!(queue.visited(), 0);
        assert!(!queue.exhausted(100));
    }
}
