//! Town markets: what each place makes, wants, holds and charges.
//!
//! # The first thing here that is not a pure function of the seed
//!
//! Everything else in this world can be recomputed: terrain, ore, trees, town
//! sites, the work a beacon posts. A market cannot, because the player changes
//! it — sell forty loads of ore and the price moves, and it has to still be
//! moved tomorrow. So this is the first state that genuinely has to be *kept*.
//!
//! It is kept as cheaply as the rest: a town's opening books are derived from
//! its site, so the save holds an entry only for a town something has actually
//! touched. A frontier of a thousand untouched towns costs nothing.
//!
//! # Fast-forward on read
//!
//! Nothing ticks. A market carries the tick it was last brought up to date, and
//! reading it integrates production and consumption forward to now. A town three
//! kilometres away costs nothing until something asks about it, and then costs
//! one short loop.
//!
//! ## Why the loop is quantised rather than closed-form
//!
//! The obvious implementation is `stock += rate * elapsed`, in one multiply. It
//! is wrong, and wrong in a way that would have been very hard to see later.
//!
//! Flows here are *clamped* — a mine's ore fills up and stops, a refinery stops
//! when it runs out of ore to melt. Once a clamp binds, one big step and many
//! small steps stop agreeing: in one big step the mine saturates and the
//! refinery then eats a full day's worth all at once, while in small steps the
//! refinery keeps making room for the mine to refill into. The books would then
//! depend on *when the player happened to look*, which is exactly the class of
//! bug stage 9a spent its whole round removing from the drones.
//!
//! So catch-up runs in fixed [`STEP`]-tick chunks, always landing on a step
//! boundary. Integrating to tick 5 000 and then to 10 000 gives bit-for-bit
//! what integrating straight to 10 000 gives, clamps or no clamps — and there
//! is a test that says so. A whole in-game day of neglect is forty steps.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;

use vx_world::town::{Speciality, TownSite};

const MAGIC: &[u8; 4] = b"VXEC";
// Version 2 added `Owner::Mail`. The record layout is unchanged — a v1 file
// simply never contains the new owner byte — so both versions are accepted on
// read and nothing is discarded on upgrade. An older build refuses a v2 file
// cleanly, which is the tolerant-loader contract doing its job.
const VERSION: u32 = 2;

/// What the network moves.
///
/// Three raw goods that come out of the ground and one that is made. Ordered,
/// because a market's books are an array indexed by this — names are for disk
/// and for the player, indices are for the arithmetic.
pub const GOODS: [&str; 4] = [
    "engine:copper_ore",
    "engine:log",
    "engine:stone",
    "engine:copper_bar",
];

pub const ORE: usize = 0;
pub const LOG: usize = 1;
pub const STONE: usize = 2;
pub const BAR: usize = 3;

/// Ticks per catch-up step. Thirty seconds of game time at eight ticks a
/// second, so a full day of neglect is forty steps.
pub const STEP: u64 = 240;

/// What a town aims to hold of a good. Prices sit at their reference here.
const TARGET: f32 = 400.0;

/// The most a town will hold. Past this a producer simply stops.
const CAPACITY: f32 = 1_200.0;

/// What each good is worth to a town holding exactly [`TARGET`] of it.
const BASE_PRICE: [f32; GOODS.len()] = [10.0, 4.0, 3.0, 60.0];

/// How far price may swing from its reference. A glutted market still pays
/// something and a desperate one is still affordable — a shock should be
/// legible, not a cliff.
const MIN_FACTOR: f32 = 0.35;
const MAX_FACTOR: f32 = 2.50;

/// Look up a good by its namespaced name.
pub fn good_index(name: &str) -> Option<usize> {
    GOODS.iter().position(|good| *good == name)
}

/// One town's books.
#[derive(Debug, Clone, PartialEq)]
pub struct Market {
    stock: [f32; GOODS.len()],
    /// The last step boundary this was brought up to date at.
    last_tick: u64,
}

impl Market {
    pub fn stock(&self, good: usize) -> f32 {
        self.stock[good]
    }

    /// What this town pays for one unit, given what it is holding.
    ///
    /// Monotonically decreasing in stock and bounded at both ends: cheap where
    /// a thing is made, dear where it is wanted, never free and never infinite.
    pub fn price(&self, good: usize) -> u64 {
        let ratio = self.stock[good] / TARGET;
        let factor = (2.0 - ratio).clamp(MIN_FACTOR, MAX_FACTOR);
        (BASE_PRICE[good] * factor).round().max(1.0) as u64
    }

    /// Is this town short of a good — the thing that makes it worth shipping to?
    pub fn wants(&self, good: usize) -> bool {
        self.stock[good] < TARGET * 0.6
    }

    /// Has it more than it can use?
    pub fn surplus(&self, good: usize) -> f32 {
        (self.stock[good] - TARGET * 1.2).max(0.0)
    }

    /// Move goods in or out, clamped to what is actually there.
    /// Returns the amount that really moved.
    pub fn deposit(&mut self, good: usize, amount: f32) -> f32 {
        let room = CAPACITY - self.stock[good];
        let moved = amount.min(room).max(0.0);
        self.stock[good] += moved;
        moved
    }

    pub fn withdraw(&mut self, good: usize, amount: f32) -> f32 {
        let moved = amount.min(self.stock[good]).max(0.0);
        self.stock[good] -= moved;
        moved
    }
}

/// What a town pulls out of the ground, per tick, needing nothing to do it.
fn extraction(speciality: Speciality) -> [f32; GOODS.len()] {
    match speciality {
        // A camp built around a hole: ore and the spoil that comes with it.
        Speciality::Mine => [0.030, 0.0, 0.020, 0.0],
        // Timber from the country around it.
        Speciality::Depot => [0.0, 0.018, 0.0, 0.0],
        Speciality::Refinery => [0.0, 0.004, 0.006, 0.0],
    }
}

/// What a town makes out of other goods: `(inputs per unit, output, rate)`.
fn conversion(speciality: Speciality) -> Option<([f32; GOODS.len()], usize, f32)> {
    match speciality {
        // Ore and stone in, bars out. The reason a refinery is worth visiting.
        Speciality::Refinery => Some(([0.030, 0.0, 0.012, 0.0], BAR, 0.010)),
        _ => None,
    }
}

/// What a town simply uses up, per tick.
fn consumption(speciality: Speciality) -> [f32; GOODS.len()] {
    match speciality {
        // Props and fires.
        Speciality::Mine => [0.0, 0.012, 0.0, 0.0],
        // A depot's whole business is moving finished goods onward.
        Speciality::Depot => [0.004, 0.006, 0.002, 0.012],
        Speciality::Refinery => [0.0, 0.008, 0.0, 0.002],
    }
}

/// A town's books before anything has touched them.
///
/// Derived from the site, like its name and size, so an untouched town costs
/// the save nothing. Biased by speciality — a mine opens sitting on ore — and
/// jittered per town so no two markets are identical.
pub fn opening_books(site: &TownSite) -> Market {
    let mut stock = [0.0f32; GOODS.len()];
    for (good, slot) in stock.iter_mut().enumerate() {
        let jitter = vx_world::seed::unit(vx_world::seed::finalise(
            site.seed ^ (good as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15),
        ));
        // 0.6 to 1.4 of target, then shifted by what the town does.
        let spread = 0.6 + jitter * 0.8;
        let bias = match (site.speciality, good) {
            (Speciality::Mine, ORE) | (Speciality::Mine, STONE) => 1.6,
            (Speciality::Refinery, BAR) => 1.5,
            (Speciality::Refinery, ORE) => 0.5,
            (Speciality::Depot, LOG) => 1.4,
            (Speciality::Depot, BAR) => 0.5,
            _ => 1.0,
        };
        *slot = (TARGET * spread * bias).min(CAPACITY);
    }
    Market {
        stock,
        last_tick: 0,
    }
}

/// Advance one market by exactly one [`STEP`].
fn step_market(market: &mut Market, speciality: Speciality) {
    let dt = STEP as f32;

    // Out of the ground first: unconditional, and stops at capacity.
    for (good, rate) in extraction(speciality).into_iter().enumerate() {
        if rate > 0.0 {
            market.stock[good] = (market.stock[good] + rate * dt).min(CAPACITY);
        }
    }

    // Then the works, which run only as long as their inputs last.
    if let Some((inputs, output, rate)) = conversion(speciality) {
        let mut run = dt;
        for (good, need) in inputs.into_iter().enumerate() {
            if need > 0.0 {
                run = run.min(market.stock[good] / need);
            }
        }
        for (good, need) in inputs.into_iter().enumerate() {
            if need > 0.0 {
                market.stock[good] = (market.stock[good] - need * run).max(0.0);
            }
        }
        market.stock[output] = (market.stock[output] + rate * run).min(CAPACITY);
    }

    // Then what the town simply burns through, down to nothing at worst.
    for (good, rate) in consumption(speciality).into_iter().enumerate() {
        if rate > 0.0 {
            market.stock[good] = (market.stock[good] - rate * dt).max(0.0);
        }
    }
}

/// Who a load belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// A town shipping to another town, on its own account.
    Town,
    /// The player's drone, paid out on arrival.
    Player,
    /// A mail order: bought and paid for at a counter, bound for the player's
    /// mailbox rather than any town's books.
    Mail,
}

/// A load in the air.
///
/// The same record for a town's freight and the player's, which is most of why
/// doing both halves of this round cost barely more than doing one.
///
/// Nothing about it is simulated. It has a departure, an arrival and two ends,
/// and *where it is right now is a sum* — see [`Shipment::position_at`]. That is
/// what makes a hundred loads crossing the map free, and what lets one be drawn
/// as a real drone when it happens to pass the player without any state
/// existing for it to be drawn from.
#[derive(Debug, Clone, PartialEq)]
pub struct Shipment {
    pub from: (i32, i32),
    pub to: (i32, i32),
    pub good: usize,
    pub amount: f32,
    pub depart: u64,
    pub arrive: u64,
    pub owner: Owner,
}

/// How many ticks a load takes per block travelled.
///
/// About twelve blocks a second, so a two-kilometre haul is a shade under three
/// in-game minutes — long enough that a run is a commitment, short enough that
/// the frontier feels connected.
const TICKS_PER_BLOCK: f32 = 0.65;

/// The least a run can take, so neighbouring towns do not trade instantly.
const MIN_TRAVEL: u64 = 200;

impl Shipment {
    /// How long a run between two towns takes.
    pub fn travel_ticks(from: (i32, i32), to: (i32, i32)) -> u64 {
        let dx = (from.0 - to.0) as f32;
        let dz = (from.1 - to.1) as f32;
        ((dx * dx + dz * dz).sqrt() * TICKS_PER_BLOCK) as u64 + MIN_TRAVEL
    }

    /// Where the load is at `now`, as a column. Pure in the clock: same tick,
    /// same place, however often it is asked.
    pub fn position_at(&self, now: u64) -> (f32, f32) {
        let span = self.arrive.saturating_sub(self.depart).max(1) as f32;
        let travelled = now.saturating_sub(self.depart) as f32;
        let t = (travelled / span).clamp(0.0, 1.0);
        (
            self.from.0 as f32 + (self.to.0 - self.from.0) as f32 * t,
            self.from.1 as f32 + (self.to.1 - self.from.1) as f32 * t,
        )
    }
}

/// How much of a surplus a town will part with in one load.
const LOAD: f32 = 90.0;

/// How much the player's drone carries in one run. Whole units, because it
/// comes out of a block pile rather than a town's books.
pub const PLAYER_LOAD: u64 = 60;

/// How much one mail order brings. Smaller than a trade run on purpose:
/// ordering is convenience, hauling is the business.
pub const PARCEL: u64 = 20;

/// Most goods a mailbox holds, counting loads still in the air. A cap rather
/// than a queue: the refusal happens at the counter, where you can do
/// something about it.
pub const MAILBOX_CAP: u64 = 120;

/// How often the network looks for a run worth making. Four in-game minutes.
pub const DISPATCH_EVERY: u64 = STEP * 8;

/// Every town whose books have moved away from what its site implies, and
/// every load currently in the air.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Economy {
    towns: HashMap<(i32, i32), Market>,
    /// Ordered by arrival, so settling is a walk off the front.
    shipments: Vec<Shipment>,
    /// The last tick the network was asked to look for work.
    last_dispatch: u64,
}

impl Economy {
    pub fn new() -> Self {
        Economy::default()
    }

    pub fn tracked(&self) -> usize {
        self.towns.len()
    }

    /// A town's books, brought up to date.
    ///
    /// The only way to read a market. Everything else goes through here so that
    /// nothing can ever see a stale one.
    pub fn market(&mut self, site: &TownSite, now: u64) -> &Market {
        self.market_mut(site, now)
    }

    pub fn market_mut(&mut self, site: &TownSite, now: u64) -> &mut Market {
        let market = self
            .towns
            .entry(site.centre)
            .or_insert_with(|| opening_books(site));

        // Catch up in whole steps, landing on a boundary. This is what makes
        // "integrate to 5 000 then to 10 000" identical to "integrate to
        // 10 000" even where a clamp binds.
        let due = now / STEP;
        let done = market.last_tick / STEP;
        for _ in done..due {
            step_market(market, site.speciality);
        }
        market.last_tick = due * STEP;
        market
    }

    /// Loads currently in the air.
    pub fn shipments(&self) -> &[Shipment] {
        &self.shipments
    }

    /// Put a load in the air. Goods must already have left wherever they came
    /// from — this only carries them.
    pub fn ship(&mut self, shipment: Shipment) {
        // Kept in arrival order so settling is a walk off the front rather than
        // a scan of everything in flight.
        let at = self
            .shipments
            .partition_point(|other| other.arrive <= shipment.arrive);
        self.shipments.insert(at, shipment);
    }

    /// Run the network up to `now`: land everything that has arrived, then look
    /// for new runs worth making.
    ///
    /// `reachable` is the towns the network can see — pass what
    /// [`vx_world::town::towns_near`] returns for the player's neighbourhood.
    /// Towns beyond it carry on being derived and simply do not trade until
    /// somebody is near enough to care, which is the level-of-detail: the
    /// simulation is only ever as wide as it needs to be.
    ///
    /// Returns the player-owned loads that landed, so the caller can pay for
    /// them — this module knows nothing about wallets.
    pub fn run(&mut self, reachable: &[TownSite], now: u64) -> Vec<Shipment> {
        let mut delivered = Vec::new();

        // Catch up one dispatch window at a time, exactly as a market catches
        // up one step at a time and for the same reason: a network that only
        // looked for work when somebody happened to be watching would move
        // different goods depending on how often it was watched.
        let due = now / DISPATCH_EVERY;
        let done = self.last_dispatch / DISPATCH_EVERY;
        for window in done..due {
            let at = (window + 1) * DISPATCH_EVERY;
            self.settle(reachable, at, &mut delivered);
            self.dispatch(reachable, at);
        }
        self.last_dispatch = due * DISPATCH_EVERY;

        // And anything that has landed since the last whole window.
        self.settle(reachable, now, &mut delivered);
        delivered
    }

    /// Land every load that has arrived by `at`.
    fn settle(&mut self, reachable: &[TownSite], at: u64, delivered: &mut Vec<Shipment>) {
        // The list is arrival-ordered, so this stops at the first load still in
        // the air rather than scanning everything in flight.
        while self.shipments.first().is_some_and(|first| first.arrive <= at) {
            let landed = self.shipments.remove(0);
            // Mail never touches the destination's books: it was bought and
            // paid for at the source, and it lands in a mailbox, not a market.
            if landed.owner == Owner::Mail {
                delivered.push(landed);
                continue;
            }
            match reachable.iter().find(|site| site.centre == landed.to) {
                Some(site) => {
                    // Brought up to the moment the load *arrived*, not to now:
                    // a load that landed an hour ago should have been on the
                    // shelves for that hour.
                    let market = self.market_mut(site, landed.arrive);
                    market.deposit(landed.good, landed.amount);
                }
                // Nowhere to put it. The goods are gone either way, and a load
                // that cannot be landed is better lost than quietly duplicated.
                None => log::warn!("a load arrived at {:?}, which is not a town", landed.to),
            }
            if landed.owner == Owner::Player {
                delivered.push(landed);
            }
        }
    }

    /// Look for a run worth making and put one load in the air.
    ///
    /// Greedy nearest-deficit matching: for the town with the biggest surplus,
    /// ship to the nearest town within reach that is short of that good. This
    /// is a deliberate simplification of the min-cost flow a full treatment
    /// would use — at a few dozen towns the two agree on the runs that matter,
    /// and the greedy version is a fraction of the code. Worth revisiting only
    /// if the traffic it produces disappoints.
    fn dispatch(&mut self, reachable: &[TownSite], now: u64) {
        let mut best: Option<(f32, usize, usize, usize)> = None; // surplus, from, to, good

        for (from_index, from) in reachable.iter().enumerate() {
            for good in 0..GOODS.len() {
                let surplus = self.market(from, now).surplus(good);
                if surplus < LOAD {
                    continue;
                }
                // Nearest town that actually wants it.
                let mut nearest: Option<(i64, usize)> = None;
                for (to_index, to) in reachable.iter().enumerate() {
                    if to.centre == from.centre || !self.market(to, now).wants(good) {
                        continue;
                    }
                    let dx = (from.centre.0 - to.centre.0) as i64;
                    let dz = (from.centre.1 - to.centre.1) as i64;
                    let distance = dx * dx + dz * dz;
                    if nearest.is_none_or(|(best, _)| distance < best) {
                        nearest = Some((distance, to_index));
                    }
                }
                if let Some((_, to_index)) = nearest {
                    if best.is_none_or(|(most, ..)| surplus > most) {
                        best = Some((surplus, from_index, to_index, good));
                    }
                }
            }
        }

        let Some((_, from_index, to_index, good)) = best else {
            return;
        };
        let from = reachable[from_index];
        let to = reachable[to_index];

        // Take the goods out before they go in the air, or the network would
        // conjure freight out of nothing.
        let loaded = self.market_mut(&from, now).withdraw(good, LOAD);
        if loaded <= 0.0 {
            return;
        }
        self.ship(Shipment {
            from: from.centre,
            to: to.centre,
            good,
            amount: loaded,
            depart: now,
            arrive: now + Shipment::travel_ticks(from.centre, to.centre),
            owner: Owner::Town,
        });
    }

    /// A summary for hashing, so the replay oracle can cover the economy.
    ///
    /// Rounded to whole units on purpose: the books are `f32`, and a hash over
    /// raw bits would report divergence over a fraction of a log nobody could
    /// ever see.
    pub fn books_hash(&self) -> u64 {
        let mut towns: Vec<(&(i32, i32), &Market)> = self.towns.iter().collect();
        towns.sort_by_key(|(centre, _)| **centre);
        let mut hash = 0u64;
        for (centre, market) in towns {
            let mut town = vx_world::seed::finalise(
                (centre.0 as i64 as u64).wrapping_mul(0x2545_f491_4f6c_dd1d)
                    ^ (centre.1 as i64 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15),
            );
            for good in 0..GOODS.len() {
                town = vx_world::seed::finalise(
                    town ^ (market.stock[good].round() as i64 as u64)
                        .wrapping_mul(0xbf58_476d_1ce4_e5b9),
                );
            }
            hash ^= town;
        }
        hash
    }

    pub fn save(&self, directory: &Path) -> std::io::Result<()> {
        let mut file =
            std::io::BufWriter::new(std::fs::File::create(directory.join("economy.dat"))?);
        file.write_all(MAGIC)?;
        file.write_all(&VERSION.to_le_bytes())?;

        let mut towns: Vec<(&(i32, i32), &Market)> = self.towns.iter().collect();
        towns.sort_by_key(|(centre, _)| **centre);
        file.write_all(&(towns.len() as u32).to_le_bytes())?;
        for (centre, market) in towns {
            file.write_all(&centre.0.to_le_bytes())?;
            file.write_all(&centre.1.to_le_bytes())?;
            file.write_all(&market.last_tick.to_le_bytes())?;
            for good in 0..GOODS.len() {
                file.write_all(&market.stock[good].to_le_bytes())?;
            }
        }

        file.write_all(&self.last_dispatch.to_le_bytes())?;
        file.write_all(&(self.shipments.len() as u32).to_le_bytes())?;
        for load in &self.shipments {
            file.write_all(&load.from.0.to_le_bytes())?;
            file.write_all(&load.from.1.to_le_bytes())?;
            file.write_all(&load.to.0.to_le_bytes())?;
            file.write_all(&load.to.1.to_le_bytes())?;
            file.write_all(&(load.good as u32).to_le_bytes())?;
            file.write_all(&load.amount.to_le_bytes())?;
            file.write_all(&load.depart.to_le_bytes())?;
            file.write_all(&load.arrive.to_le_bytes())?;
            file.write_all(&[match load.owner {
                Owner::Town => 0,
                Owner::Player => 1,
                Owner::Mail => 2,
            }])?;
        }
        file.flush()
    }

    /// Read the books back, tolerating absence and damage.
    ///
    /// A damaged file is an *empty* economy, not a failed world: every town
    /// falls back to the books its site implies. What is lost is trading
    /// history, which is a shame rather than a disaster.
    pub fn load(&mut self, directory: &Path) {
        let path = directory.join("economy.dat");
        match read_economy(&path) {
            Ok(Some(economy)) => *self = economy,
            Ok(None) => {}
            Err(error) => {
                log::warn!(
                    "could not read {}: {error}; every town reopens on its site's books",
                    path.display()
                );
            }
        }
    }
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

fn read_f32(file: &mut impl Read) -> std::io::Result<f32> {
    let mut word = [0u8; 4];
    file.read_exact(&mut word)?;
    let value = f32::from_le_bytes(word);
    if !value.is_finite() || value < 0.0 {
        return Err(std::io::Error::other("stock is not a number"));
    }
    Ok(value)
}

fn read_economy(path: &Path) -> std::io::Result<Option<Economy>> {
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
    let version = read_u32(&mut file)?;
    // Version 1 is readable as-is: the layout never changed, only which owner
    // bytes can appear. Anything newer than this build writes is refused.
    if version == 0 || version > VERSION {
        return Err(std::io::Error::other("unknown version"));
    }

    let mut economy = Economy::new();
    let count = read_u32(&mut file)?;
    for _ in 0..count {
        let centre = (read_u32(&mut file)? as i32, read_u32(&mut file)? as i32);
        let last_tick = read_u64(&mut file)?;
        let mut stock = [0.0f32; GOODS.len()];
        for slot in stock.iter_mut() {
            *slot = read_f32(&mut file)?;
        }
        economy.towns.insert(centre, Market { stock, last_tick });
    }

    economy.last_dispatch = read_u64(&mut file)?;
    let loads = read_u32(&mut file)?;
    for _ in 0..loads {
        let from = (read_u32(&mut file)? as i32, read_u32(&mut file)? as i32);
        let to = (read_u32(&mut file)? as i32, read_u32(&mut file)? as i32);
        let good = read_u32(&mut file)? as usize;
        if good >= GOODS.len() {
            return Err(std::io::Error::other("unknown good in a load"));
        }
        let amount = read_f32(&mut file)?;
        let depart = read_u64(&mut file)?;
        let arrive = read_u64(&mut file)?;
        let mut owner = [0u8; 1];
        file.read_exact(&mut owner)?;
        economy.shipments.push(Shipment {
            from,
            to,
            good,
            amount,
            depart,
            arrive,
            owner: match owner[0] {
                0 => Owner::Town,
                1 => Owner::Player,
                2 => Owner::Mail,
                other => return Err(std::io::Error::other(format!("unknown owner {other}"))),
            },
        });
    }
    Ok(Some(economy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_world::town;

    fn frontier() -> Vec<TownSite> {
        town::towns_near(7, (0, 0), 2_000, &|_, _| 90)
    }

    fn of(speciality: Speciality) -> TownSite {
        frontier()
            .into_iter()
            .find(|site| site.speciality == speciality)
            .unwrap_or_else(|| panic!("no {} on the fixture frontier", speciality.name()))
    }

    #[test]
    fn catching_up_in_one_go_matches_catching_up_in_pieces() {
        // The load-bearing property of the whole model. If reading a market
        // late gives a different answer from reading it often, then the world
        // depends on when the player happened to look — which is the bug stage
        // 9a spent a round removing from the drones, reappearing in the books.
        //
        // It has to hold *through* the clamps: long enough that a mine fills
        // up and a refinery runs its inputs dry, because that is exactly where
        // a naive `stock += rate * elapsed` stops being splittable.
        for speciality in [Speciality::Mine, Speciality::Refinery, Speciality::Depot] {
            let site = of(speciality);
            let far = 200_000; // a good twenty in-game days

            let mut all_at_once = Economy::new();
            all_at_once.market(&site, far);

            let mut in_pieces = Economy::new();
            for at in (0..=far).step_by(STEP as usize * 7) {
                in_pieces.market(&site, at);
            }
            in_pieces.market(&site, far);

            assert_eq!(
                all_at_once.market(&site, far),
                in_pieces.market(&site, far),
                "{} books depend on when they were read",
                speciality.name()
            );
        }
    }

    #[test]
    fn reading_a_market_between_steps_does_not_advance_it() {
        // Catch-up lands on step boundaries, which is what makes the property
        // above exact rather than approximate.
        let site = of(Speciality::Mine);
        let mut economy = Economy::new();
        let opening = economy.market(&site, 0).clone();
        assert_eq!(economy.market(&site, STEP - 1), &opening);
        assert_ne!(economy.market(&site, STEP), &opening);
        assert_eq!(economy.market(&site, STEP).last_tick, STEP);
    }

    #[test]
    fn a_town_makes_what_it_is_for_and_burns_what_it_needs() {
        let mine = of(Speciality::Mine);
        let mut economy = Economy::new();
        let before = economy.market(&mine, 0).clone();
        let after = economy.market(&mine, STEP * 40).clone();
        assert!(
            after.stock(ORE) > before.stock(ORE),
            "a mine did not produce ore"
        );
        assert!(
            after.stock(LOG) < before.stock(LOG),
            "a mine did not burn any timber"
        );

        // A refinery turns ore and stone into bars.
        let refinery = of(Speciality::Refinery);
        let before = economy.market(&refinery, 0).clone();
        let after = economy.market(&refinery, STEP * 40).clone();
        assert!(
            after.stock(BAR) > before.stock(BAR),
            "a refinery made no bars"
        );
        assert!(after.stock(ORE) < before.stock(ORE), "it ate no ore");
    }

    #[test]
    fn a_refinery_with_no_ore_makes_no_bars() {
        // The conversion is limited by its inputs, which is what stops a
        // refinery conjuring metal out of an empty yard.
        let site = of(Speciality::Refinery);
        let mut economy = Economy::new();
        {
            let market = economy.market_mut(&site, 0);
            market.withdraw(ORE, 1e9);
            market.withdraw(STONE, 1e9);
        }
        let bars = economy.market(&site, 0).stock(BAR);
        let after = economy.market(&site, STEP * 10).stock(BAR);
        assert!(after <= bars, "bars appeared from an empty yard");
        assert_eq!(economy.market(&site, STEP * 10).stock(ORE), 0.0);
    }

    #[test]
    fn stock_never_leaves_its_bounds_however_long_it_runs() {
        for speciality in [Speciality::Mine, Speciality::Refinery, Speciality::Depot] {
            let site = of(speciality);
            let mut economy = Economy::new();
            let market = economy.market(&site, 1_000_000);
            for (good, name) in GOODS.iter().enumerate() {
                let held = market.stock(good);
                assert!(
                    (0.0..=CAPACITY).contains(&held),
                    "{} holds {held} of {name}",
                    speciality.name()
                );
            }
        }
    }

    #[test]
    fn a_glutted_market_pays_less_than_a_starving_one() {
        let site = of(Speciality::Depot);
        let mut economy = Economy::new();

        let starving = {
            let market = economy.market_mut(&site, 0);
            market.withdraw(ORE, 1e9);
            market.price(ORE)
        };
        let glutted = {
            let market = economy.market_mut(&site, 0);
            market.deposit(ORE, CAPACITY);
            market.price(ORE)
        };

        assert!(
            starving > glutted,
            "a starving market paid {starving} and a glutted one {glutted}"
        );
        // Bounded at both ends: never free, never runaway.
        assert!(glutted >= 1, "a glutted market paid nothing at all");
        let ceiling = (BASE_PRICE[ORE] * MAX_FACTOR).round() as u64;
        assert!(starving <= ceiling, "a starving market paid {starving}");
    }

    #[test]
    fn price_falls_the_more_a_market_is_holding() {
        // Monotonic, so a player dumping a load can predict what it does.
        let site = of(Speciality::Depot);
        let mut economy = Economy::new();
        let market = economy.market_mut(&site, 0);
        market.withdraw(ORE, 1e9);

        let mut last = u64::MAX;
        for _ in 0..20 {
            let price = market.price(ORE);
            assert!(price <= last, "price rose from {last} to {price} on a sale");
            last = price;
            market.deposit(ORE, 60.0);
        }
    }

    #[test]
    fn moving_goods_reports_what_actually_moved() {
        let site = of(Speciality::Depot);
        let mut economy = Economy::new();
        let market = economy.market_mut(&site, 0);

        market.withdraw(BAR, 1e9);
        assert_eq!(market.stock(BAR), 0.0);
        assert_eq!(market.withdraw(BAR, 50.0), 0.0, "took from an empty shelf");

        assert_eq!(market.deposit(BAR, 100.0), 100.0);
        assert_eq!(market.stock(BAR), 100.0);
        // And a market cannot be filled past its capacity.
        let taken = market.deposit(BAR, CAPACITY * 2.0);
        assert_eq!(taken, CAPACITY - 100.0);
        assert_eq!(market.stock(BAR), CAPACITY);
    }

    #[test]
    fn towns_open_on_different_books_but_the_same_ones_every_time() {
        let sites = frontier();
        assert!(sites.len() > 3, "the fixture frontier is too small");

        for site in &sites {
            assert_eq!(
                opening_books(site),
                opening_books(site),
                "a town's opening books are not reproducible"
            );
        }
        // Two towns should not read identically, or the map has no gradient
        // for anyone to trade across.
        assert!(
            sites
                .windows(2)
                .any(|pair| opening_books(&pair[0]) != opening_books(&pair[1])),
            "every town opened on identical books"
        );
    }

    #[test]
    fn an_untouched_frontier_costs_the_save_nothing() {
        // A town nobody has visited is derived, not stored.
        let sites = frontier();
        let mut economy = Economy::new();
        assert_eq!(economy.tracked(), 0);
        economy.market(&sites[0], 5_000);
        assert_eq!(economy.tracked(), 1, "reading one town tracked {} ", economy.tracked());
    }

    #[test]
    fn the_books_round_trip_and_tolerate_damage() {
        let directory = std::env::temp_dir().join(format!("vx-economy-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();

        let sites = frontier();
        let mut economy = Economy::new();
        for site in sites.iter().take(4) {
            economy.market_mut(site, 12_000).deposit(BAR, 37.0);
        }
        economy.save(&directory).unwrap();

        let mut read = Economy::new();
        read.load(&directory);
        assert_eq!(read, economy, "the books did not survive the trip");
        assert_eq!(read.books_hash(), economy.books_hash());

        std::fs::write(directory.join("economy.dat"), b"NOT A LEDGER").unwrap();
        let mut damaged = Economy::new();
        damaged.load(&directory);
        assert_eq!(damaged.tracked(), 0, "a damaged file invented markets");

        std::fs::remove_dir_all(&directory).ok();
        let mut missing = Economy::new();
        missing.load(&directory);
        assert_eq!(missing.tracked(), 0);
    }

    #[test]
    fn negative_town_coordinates_survive_the_encoding() {
        let directory = std::env::temp_dir().join(format!("vx-economy-neg-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();

        let site = frontier()
            .into_iter()
            .find(|site| site.centre.0 < 0 || site.centre.1 < 0)
            .expect("the fixture frontier has no town west or north of the origin");
        let mut economy = Economy::new();
        economy.market(&site, 3_000);
        economy.save(&directory).unwrap();

        let mut read = Economy::new();
        read.load(&directory);
        assert_eq!(read, economy);
        std::fs::remove_dir_all(&directory).ok();
    }

    /// Total units of a good held across every tracked town plus everything in
    /// the air. What conservation is measured against.
    fn afloat_and_ashore(economy: &Economy, good: usize) -> f32 {
        let ashore: f32 = economy
            .towns
            .values()
            .map(|market| market.stock[good])
            .sum();
        let afloat: f32 = economy
            .shipments
            .iter()
            .filter(|load| load.good == good)
            .map(|load| load.amount)
            .sum();
        ashore + afloat
    }

    #[test]
    fn a_load_leaves_one_town_and_lands_whole_at_the_other() {
        // Nothing may be created or destroyed by the act of carrying it.
        let sites = frontier();
        // Just the two towns, so nothing else's books enter the sum.
        let pair = [sites[0], sites[1]];
        let (from, to) = (pair[0], pair[1]);
        let arrive = Shipment::travel_ticks(from.centre, to.centre);

        let mut economy = Economy::new();
        // Both caught up to the landing tick first, so the only thing that
        // happens across the measurement is the load being carried — not a
        // day's production either side of it.
        economy.market(&from, arrive);
        economy.market(&to, arrive);

        let before = afloat_and_ashore(&economy, ORE);
        let carried = economy.market_mut(&from, arrive).withdraw(ORE, 120.0);
        assert!(carried > 0.0);
        economy.ship(Shipment {
            from: from.centre,
            to: to.centre,
            good: ORE,
            amount: carried,
            depart: 0,
            arrive,
            owner: Owner::Town,
        });

        // In the air: still accounted for, and not yet on anyone's shelves.
        assert_eq!(economy.shipments().len(), 1);
        assert!((afloat_and_ashore(&economy, ORE) - before).abs() < 0.01);

        // Landed: the destination has it, nothing is in the air.
        economy.settle(&pair, arrive, &mut Vec::new());
        assert!(economy.shipments().is_empty(), "the load never landed");
        assert!(
            (afloat_and_ashore(&economy, ORE) - before).abs() < 0.01,
            "goods were created or lost in transit"
        );
    }

    #[test]
    fn a_load_is_where_the_clock_says_and_nowhere_else() {
        // Position is a sum, not a simulation — which is what lets one be drawn
        // as a real drone without any state existing for it.
        let load = Shipment {
            from: (0, 0),
            to: (1_000, -500),
            good: ORE,
            amount: 50.0,
            depart: 1_000,
            arrive: 2_000,
            owner: Owner::Town,
        };
        assert_eq!(load.position_at(1_000), (0.0, 0.0));
        assert_eq!(load.position_at(2_000), (1_000.0, -500.0));
        assert_eq!(load.position_at(1_500), (500.0, -250.0));

        // Asking twice gives the same answer, and it never runs off either end.
        assert_eq!(load.position_at(1_500), load.position_at(1_500));
        assert_eq!(load.position_at(0), (0.0, 0.0));
        assert_eq!(load.position_at(9_999), (1_000.0, -500.0));
    }

    #[test]
    fn the_network_ships_from_glut_to_shortage() {
        // The game-of-life bit: left alone, towns move goods to where they are
        // wanted, and the run happens whether anyone is watching or not.
        let sites = frontier();
        let mut economy = Economy::new();

        // Long enough for specialities to pull the books apart.
        let mut now = 0;
        let mut seen = 0;
        for _ in 0..40 {
            now += DISPATCH_EVERY;
            economy.run(&sites, now);
            seen = seen.max(economy.shipments().len());
        }
        assert!(seen > 0, "the network never sent anything anywhere");

        // And every load it sent went somewhere that wanted it.
        for load in economy.shipments() {
            assert_ne!(load.from, load.to, "a town shipped to itself");
            assert!(load.amount > 0.0, "an empty load went out");
            assert!(load.arrive > load.depart, "a load arrived before it left");
        }
    }

    #[test]
    fn the_network_is_the_same_however_often_it_is_run() {
        // Same guarantee as the books themselves: dispatch is quantised, so
        // running the network every step and running it rarely land in the
        // same place.
        let sites = frontier();
        let far = DISPATCH_EVERY * 30;

        let mut often = Economy::new();
        for at in (0..=far).step_by(DISPATCH_EVERY as usize) {
            often.run(&sites, at);
        }

        let mut rarely = Economy::new();
        for at in (0..=far).step_by(DISPATCH_EVERY as usize * 5) {
            rarely.run(&sites, at);
        }
        rarely.run(&sites, far);
        often.run(&sites, far);

        assert_eq!(
            often.books_hash(),
            rarely.books_hash(),
            "the network's books depend on how often it was looked at"
        );
    }

    #[test]
    fn loads_in_the_air_survive_a_save() {
        let directory = std::env::temp_dir().join(format!("vx-loads-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();

        let sites = frontier();
        let mut economy = Economy::new();
        for _ in 0..30 {
            economy.run(&sites, economy.last_dispatch + DISPATCH_EVERY);
        }
        economy.ship(Shipment {
            from: sites[0].centre,
            to: sites[1].centre,
            good: BAR,
            amount: 17.5,
            depart: 10,
            arrive: 9_000,
            owner: Owner::Player,
        });
        economy.save(&directory).unwrap();

        let mut read = Economy::new();
        read.load(&directory);
        assert_eq!(read, economy, "loads in the air did not survive the trip");
        assert!(
            read.shipments().iter().any(|load| load.owner == Owner::Player),
            "the player's load was lost"
        );

        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn running_the_network_loads_no_chunks() {
        // Same promise the beacon board makes: the frontier is arithmetic.
        let probes = std::cell::Cell::new(0usize);
        let sites = town::towns_near(11, (0, 0), 3_000, &|_, _| {
            probes.set(probes.get() + 1);
            90
        });
        let after_siting = probes.get();

        let mut economy = Economy::new();
        for _ in 0..20 {
            economy.run(&sites, economy.last_dispatch + DISPATCH_EVERY);
        }
        assert_eq!(
            probes.get(),
            after_siting,
            "running the network went back to the terrain"
        );
        assert!(economy.tracked() > 0, "the network touched nothing at all");
    }
}

#[cfg(test)]
mod mail_tests {
    use super::*;
    use vx_world::town::home_site;

    fn far_town() -> TownSite {
        TownSite {
            centre: (1024, 512),
            ..home_site()
        }
    }

    #[test]
    fn a_mail_load_lands_in_the_delivered_list_not_the_towns_books() {
        let mut economy = Economy::new();
        let home = home_site();
        let source = far_town();

        let before = economy.market(&home, 0).stock(ORE);
        economy.ship(Shipment {
            from: source.centre,
            to: home.centre,
            good: ORE,
            amount: PARCEL as f32,
            depart: 0,
            arrive: 10,
            owner: Owner::Mail,
        });

        let landed = economy.run(&[home, source], 20);

        assert_eq!(landed.len(), 1, "the mail never landed");
        assert_eq!(landed[0].owner, Owner::Mail);
        let after = economy.market(&home, 20).stock(ORE);
        assert!(
            (after - before).abs() < f32::EPSILON,
            "mail leaked into the town's books: {before} became {after}"
        );
    }

    #[test]
    fn an_economy_file_from_before_mail_still_loads() {
        // The layout never changed; v1 files simply predate the Mail owner
        // byte. Write a real file, stamp it v1, and it must load whole.
        let directory = std::env::temp_dir().join(format!(
            "vx-economy-v1-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();

        let mut economy = Economy::new();
        economy.ship(Shipment {
            from: far_town().centre,
            to: home_site().centre,
            good: LOG,
            amount: 30.0,
            depart: 5,
            arrive: 500,
            owner: Owner::Player,
        });
        economy.save(&directory).unwrap();

        let path = directory.join("economy.dat");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();

        let mut read_back = Economy::new();
        read_back.load(&directory);
        std::fs::remove_dir_all(&directory).ok();

        assert_eq!(read_back.shipments().len(), 1, "the v1 file was refused");
        assert_eq!(read_back.shipments()[0].owner, Owner::Player);
    }

    #[test]
    fn a_future_economy_version_is_still_refused() {
        let directory = std::env::temp_dir().join(format!(
            "vx-economy-v9-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();

        let economy = Economy::new();
        economy.save(&directory).unwrap();
        let path = directory.join("economy.dat");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[4..8].copy_from_slice(&9u32.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();

        let mut fresh = Economy::new();
        fresh.load(&directory); // must warn and start fresh, not panic
        std::fs::remove_dir_all(&directory).ok();
        assert!(fresh.shipments().is_empty());
    }
}
