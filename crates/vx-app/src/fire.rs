//! Lightning, and what it starts.
//!
//! The chain the forest note argues for, end to end: a storm picks somewhere
//! tall and lonely, most strikes do nothing, a dry one lights, and what
//! catches spreads **uphill and downwind** until it runs out of fuel or the
//! country turns wet. Then the ground remembers it burned, and
//! [`crate::succession`] brings the stand back.
//!
//! **The spread rule is Rothermel's shape, not his parameter set.** The
//! surface-fire model multiplies a no-wind rate by `(1 + φ_w + φ_s)` — wind
//! and slope as intensification factors, so the direction of maximum spread
//! is the vector sum of the two. That is the part worth having: a fire that
//! runs up a hill and away downwind is a fire people recognise. The full
//! fuel-particle model behind those coefficients is a reference here, not a
//! runtime requirement.
//!
//! **Per forest, from the note.** Hardwood coves resist and act as refugia —
//! mesic, high fuel moisture, sparse fine fuels. Black spruce *explodes*:
//! branches to the ground, resinous foliage, an aerial seed bank that opens
//! *because* of the fire. Subalpine burns rarely and totally. And ancient
//! wood does not burn at all, which is the promise stage 36 made when it put
//! ancients at the top of the hardness ladder.
//!
//! **Wood burns wherever it stands.** Planks, roofs, log walls, the
//! fabricator you left in a clearing, your own house. Stone, metal and the
//! bunker shell do not. A firebreak is a thing you cut, and an ancient grove
//! is the safest ground on the map.
//!
//! **No order records any of this.** Lightning is a pure function of
//! `(seed, tick)` and the player's own replayed position, so both sides
//! compute the same strike on the same tick — the module is shaped like
//! [`crate::felling`] and [`crate::arsenal`] before it: the world edits happen
//! inside [`advance_fire`], and the reports carry the live-only half.

use vx_core::{BlockId, BlockPos, Face};
use vx_world::weather::{self, Conditions};
use vx_world::World;

/// Ticks between one step of the fire and the next. Fire is not a
/// sixty-fourth-of-a-second business.
pub const EVERY: u32 = 8;

/// How long a block burns before it is gone, in fire steps.
const BURN_STEPS: u32 = 6;

/// How often a storm tries a strike, in ticks.
pub const STRIKE_EVERY: u64 = 64 * 11;

/// How far from the player a strike may land. Beyond this the ground is not
/// loaded and a strike would be a rumour.
pub const STRIKE_REACH: i32 = 96;

/// Fires per discharge, before moisture and fuel have their say.
///
/// The literature's spread on this is enormous — about one fire per fifty
/// strikes in the wetter half of one study area and one per fourteen hundred
/// on the drier side of the same border. Fifty is the generous end, because a
/// storm nobody ever sees light anything is a storm with no teeth.
pub const IGNITION_BASE: f32 = 1.0 / 50.0;

/// A dry strike is thirty to fifty per cent likelier to light than one that
/// comes with its own rain. Here it is the top of a range the moisture term
/// walks.
const DRY_BONUS: f32 = 1.5;

/// The no-wind, no-slope chance one burning block lights a neighbour.
const P0: f32 = 0.16;

/// How much a full gale multiplies the spread downwind.
const WIND_GAIN: f32 = 1.9;

/// And how much a steep slope multiplies it uphill. Fire runs uphill harder
/// than it runs downwind, which is why the number is larger.
const SLOPE_GAIN: f32 = 2.4;

/// How far a fire may run from where it started.
pub const REACH: i32 = 64;

/// Steps a fire may go without lighting anything before it is called out.
const PATIENCE: u32 = 12;

/// What a block is worth to a fire: how readily it lights, or `None` if it
/// does not burn at all.
///
/// The table is the note's per-species ecology, written as one number each.
/// Black spruce is the highest thing here for the reason the note gives — it
/// is a tree shaped like kindling — and ancient wood is absent, which is how
/// a block says "no".
pub fn fuel(name: &str) -> Option<f32> {
    Some(match name {
        // Fine fuels: what actually carries a fire between stems.
        "engine:bog_needles" => 1.00,
        "engine:needles" => 0.80,
        "engine:leaves" => 0.55,
        "engine:tall_grass" => 0.70,
        // Stems. A bog spruce is resinous and thin; a hardwood is neither.
        "engine:bog_log" => 0.85,
        "engine:spruce_log" => 0.55,
        "engine:log" => 0.34,
        // Milled wood burns like milled wood, wherever somebody put it.
        "engine:plank" => 0.50,
        "engine:roof" => 0.50,
        "engine:prime_timber" => 0.30,
        // Peat smoulders. Slowly, and it is the reason a bog fire is a
        // different animal from a forest fire.
        "engine:sphagnum" => 0.22,
        // Everything else — stone, metal, the bunker shell, and the ancient
        // wood that was promised it would never burn.
        _ => return None,
    })
}

/// One block alight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Burning {
    pub at: BlockPos,
    /// Steps of burning left before it is ash.
    pub left: u32,
}

/// A fire, from the strike that lit it to the last ember.
#[derive(Debug, Clone, PartialEq)]
pub struct Fire {
    /// Where it started. Nothing runs further than [`REACH`] from here, for
    /// the reason the water has the same rule: a system that wanders out of
    /// the ground a replay loaded is a system the two sides disagree about.
    pub origin: BlockPos,
    /// Blocks alight, kept sorted so the sweep has one canonical order.
    burning: Vec<Burning>,
    /// Steps since anything new caught.
    quiet: u32,
    step: u32,
    /// How many blocks this fire has taken, for the line it earns.
    pub eaten: u32,
}

impl Fire {
    pub fn new(origin: BlockPos) -> Self {
        Fire {
            origin,
            burning: Vec::new(),
            quiet: 0,
            step: 0,
            eaten: 0,
        }
    }

    /// Is anything still alight?
    pub fn alive(&self) -> bool {
        !self.burning.is_empty() && self.quiet < PATIENCE
    }

    pub fn burning(&self) -> &[Burning] {
        &self.burning
    }

    /// Set one block alight, if it is inside the fire's reach and will take.
    pub fn light(&mut self, world: &mut World, at: BlockPos) -> bool {
        if !within(self.origin, at) {
            return false;
        }
        let name = &world.registry().get_or_air(world.block(at)).name;
        if fuel(name).is_none() {
            return false;
        }
        let Some(ember) = world.registry().id_of("engine:ember") else {
            return false;
        };
        if let Err(index) = self
            .burning
            .binary_search_by(|other| order(&other.at, &at))
        {
            world.set_block(at, ember);
            self.burning.insert(
                index,
                Burning {
                    at,
                    left: BURN_STEPS,
                },
            );
            self.quiet = 0;
            return true;
        }
        false
    }
}

/// What one step of a fire did, for the caller to spend.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    /// Blocks that caught this step.
    pub caught: u32,
    /// Blocks that finished burning.
    pub spent: u32,
    /// Somewhere in the fire — one of the blocks alight, or the last one to
    /// go to ash if this step took the rest. The live game reads it for the
    /// noise and the light; both sides read it for the disturbance ledger.
    pub at: Option<BlockPos>,
    /// The fire is out.
    pub out: bool,
}

/// Step every fire: spread it, burn it down, and leave what it leaves.
///
/// Every edit happens in here, so the live game and a replay of it change the
/// same blocks in the same order.
pub fn advance_fire(
    fires: &mut Vec<Fire>,
    world: &mut World,
    seed: u64,
    tick: u64,
    sky: &Conditions,
) -> Vec<Report> {
    let mut reports = Vec::with_capacity(fires.len());
    for fire in fires.iter_mut() {
        reports.push(step_fire(fire, world, seed, tick, sky));
    }
    fires.retain(|fire| fire.alive());
    reports
}

fn step_fire(
    fire: &mut Fire,
    world: &mut World,
    seed: u64,
    tick: u64,
    sky: &Conditions,
) -> Report {
    fire.step += 1;
    if !fire.step.is_multiple_of(EVERY) || !fire.alive() {
        return Report {
            out: !fire.alive(),
            at: fire.burning.first().map(|block| block.at),
            ..Report::default()
        };
    }

    let ash = world.registry().id_of("engine:ash");
    let mut caught = Vec::new();
    let mut spent = 0;

    // Spread first, from the state at the top of the step: a block that
    // catches now does not get to light its own neighbours until the next
    // one, which is what keeps a fire a front rather than a flood fill.
    let alight: Vec<BlockPos> = fire.burning.iter().map(|block| block.at).collect();
    for from in &alight {
        for face in Face::ALL {
            let into = from.neighbour(face);
            if !within(fire.origin, into) {
                continue;
            }
            let name = world.registry().get_or_air(world.block(into)).name.clone();
            let Some(fuel) = fuel(&name) else { continue };
            let chance = spread_chance(seed, tick, *from, into, face, fuel, sky);
            if roll(seed, tick, into, face) < chance {
                caught.push(into);
            }
        }
    }

    // Then burn down what was already alight.
    let mut last_ash = None;
    fire.burning.retain_mut(|block| {
        block.left -= 1;
        if block.left > 0 {
            return true;
        }
        // Gone. What is under it is scorched, which is what a burn looks
        // like until the stand comes back.
        world.set_block(block.at, BlockId::AIR);
        if let Some(ash) = ash {
            let under = block.at.neighbour(Face::NegY);
            let below = world.registry().get_or_air(world.block(under)).name.clone();
            if matches!(
                below.as_str(),
                "engine:grass" | "engine:dirt" | "engine:sphagnum"
            ) {
                world.set_block(under, ash);
            }
        }
        spent += 1;
        last_ash = Some(block.at);
        false
    });

    let mut lit = 0;
    for at in caught {
        if fire.light(world, at) {
            lit += 1;
        }
    }
    fire.eaten += spent;
    if lit == 0 {
        fire.quiet += 1;
    } else {
        fire.quiet = 0;
    }

    Report {
        caught: lit,
        spent,
        // Where the fire is — or, on the step that takes the last of it,
        // where it was. The ledger reads this, and a burn whose final step
        // reported nowhere would be a burn the country never remembered.
        at: fire
            .burning
            .first()
            .map(|block| block.at)
            .or(last_ash),
        out: !fire.alive(),
    }
}

/// The chance one burning block lights a neighbour this step.
///
/// `p0 · fuel · (1 + φ_w·cosθ_wind + φ_s·cosθ_slope)`: the wind and the slope
/// enter as intensification factors, exactly as they do in the surface-fire
/// model, so the direction of maximum spread is the sum of the two and a fire
/// climbs a hill faster than it crosses a flat.
pub fn spread_chance(
    seed: u64,
    tick: u64,
    from: BlockPos,
    into: BlockPos,
    face: Face,
    fuel: f32,
    sky: &Conditions,
) -> f32 {
    let (dx, dz) = ((into.x - from.x) as f32, (into.z - from.z) as f32);
    let flat = (dx * dx + dz * dz).sqrt();

    // Downwind: the cosine between where the fire is going and where the wind
    // is blowing.
    let wind_speed = sky.wind_speed();
    let phi_w = if flat > 0.0 && wind_speed > 0.0 {
        let (wx, wz) = (sky.wind.0 / wind_speed, sky.wind.1 / wind_speed);
        let along = (dx / flat) * wx + (dz / flat) * wz;
        WIND_GAIN * (wind_speed / weather::WIND_MAX) * along.max(0.0)
    } else {
        0.0
    };

    // Uphill: a step upward is the steepest slope this grid can offer, and it
    // is the one fire likes most. Downward is a step it takes reluctantly.
    let phi_s = match face {
        Face::PosY => SLOPE_GAIN,
        Face::NegY => -0.55,
        _ => 0.0,
    };

    // How dry it is here and now. A sodden hillside does not carry a fire
    // however hard the wind blows.
    let dryness = weather::fuel_moisture(seed, tick, into.x, into.z);
    let damp = if sky.state.wet() { 0.25 } else { 1.0 };

    (P0 * fuel * (1.0 + phi_w + phi_s) * dryness * damp).clamp(0.0, 0.95)
}

/// A strike, if this storm makes one at this tick.
///
/// Biased toward the tall and the isolated the way lightning is: the column
/// picked is the highest thing in a small neighbourhood, so an emergent giant
/// or a ridgeline spruce takes it and the stand below does not.
pub fn strike(seed: u64, tick: u64, near: BlockPos, world: &World, sky: &Conditions) -> Option<BlockPos> {
    if sky.state != weather::State::Storm || !tick.is_multiple_of(STRIKE_EVERY) {
        return None;
    }
    // Somewhere in reach, hashed off the tick so both sides pick the same
    // place without anybody recording it.
    let pick = |salt: u64| {
        let hash = vx_world::seed::finalise(seed ^ salt ^ tick.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        vx_world::seed::unit(hash)
    };
    let base_x = near.x + ((pick(0xa1) * 2.0 - 1.0) * STRIKE_REACH as f32) as i32;
    let base_z = near.z + ((pick(0xb2) * 2.0 - 1.0) * STRIKE_REACH as f32) as i32;

    // The tall *and the lonely*, which is not the same as the highest column:
    // on a hillside the highest column is just the top of the hill. What
    // lightning actually takes is whatever stands proud of the ground around
    // it — an emergent giant over a cove, a ridgeline spruce, a lone snag —
    // so the column is scored by how far its top rises above its neighbours
    // three blocks out, and only ties are settled by raw height.
    let mut best: Option<(i32, i32, BlockPos)> = None;
    for dx in -6..=6 {
        for dz in -6..=6 {
            let (x, z) = (base_x + dx, base_z + dz);
            let Some(top) = world.surface_y(x, z) else {
                continue;
            };
            let around = [(3, 0), (-3, 0), (0, 3), (0, -3)]
                .iter()
                .filter_map(|(ox, oz)| world.surface_y(x + ox, z + oz))
                .min()
                .unwrap_or(top);
            let prominence = top - around;
            if best.is_none_or(|(stands, highest, _)| {
                prominence > stands || (prominence == stands && top > highest)
            }) {
                // `surface_y` is the first clear block *above* the ground, so
                // the thing that takes the strike is one below it.
                best = Some((prominence, top, BlockPos::new(x, top - 1, z)));
            }
        }
    }
    let (_, _, at) = best?;

    // Most strikes do nothing. What decides is how dry it is and what it hit.
    let name = world.registry().get_or_air(world.block(at)).name.clone();
    let fuel = fuel(&name).unwrap_or(0.0);
    let dryness = weather::fuel_moisture(seed, tick, at.x, at.z);
    // About one in fifty overall, the note's figure: a dry strike is roughly
    // two and a half times likelier than a sodden one, and a strike into
    // something that does not burn is not a fire however dry the month was.
    let chance = IGNITION_BASE * (1.0 + DRY_BONUS * dryness) * (0.15 + fuel * 0.85);
    (pick(0xc3) < chance).then_some(at)
}

/// Inside the fire's reach?
fn within(origin: BlockPos, at: BlockPos) -> bool {
    (at.x - origin.x).abs() <= REACH
        && (at.y - origin.y).abs() <= REACH
        && (at.z - origin.z).abs() <= REACH
}

/// The canonical order fires burn in: y, then z, then x.
fn order(left: &BlockPos, right: &BlockPos) -> std::cmp::Ordering {
    left.y
        .cmp(&right.y)
        .then(left.z.cmp(&right.z))
        .then(left.x.cmp(&right.x))
}

/// The one die this module rolls, and it is a hash rather than a generator.
fn roll(seed: u64, tick: u64, at: BlockPos, face: Face) -> f32 {
    let hash = vx_world::seed::finalise(
        seed ^ tick.wrapping_mul(0xff51_afd7_ed55_8ccd)
            ^ (at.x as i64 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ (at.y as i64 as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f)
            ^ (at.z as i64 as u64).wrapping_mul(0x1656_67b1_9e37_79f9)
            ^ (face as u64).wrapping_mul(0x94d0_49bb_1331_11eb),
    );
    vx_world::seed::unit(hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::ChunkPos;

    fn dry_sky() -> Conditions {
        Conditions {
            temperature: 0.8,
            humidity: 0.1,
            wind: (weather::WIND_MAX, 0.0),
            rain: 0.0,
            state: weather::State::Clear,
        }
    }

    /// A stand of one species planted in the air, well above the sea so
    /// nothing here is wet.
    fn stand(name: &str) -> (World, BlockPos) {
        let mut world = World::new(2024);
        world.load_around(ChunkPos::new(0, 0), 1);
        let block = world.registry().id_of(name).expect("no such block");
        let floor = 150;
        for x in 0..10 {
            for z in 0..10 {
                for y in floor..floor + 4 {
                    world.set_block(BlockPos::new(x, y, z), block);
                }
            }
        }
        (world, BlockPos::new(5, floor, 5))
    }

    /// Burn until it goes out, and say how much it ate.
    fn burn(world: &mut World, at: BlockPos, sky: &Conditions, tick: u64) -> u32 {
        let mut fire = Fire::new(at);
        fire.light(world, at);
        let mut fires = vec![fire];
        for step in 0..4_000u64 {
            advance_fire(&mut fires, world, 2024, tick + step, sky);
            if fires.is_empty() {
                break;
            }
        }
        // What is left of the stand tells the story: count what burned.
        let mut gone = 0;
        for x in 0..10 {
            for z in 0..10 {
                for y in 150..154 {
                    if world.block(BlockPos::new(x, y, z)).is_air() {
                        gone += 1;
                    }
                }
            }
        }
        gone
    }

    #[test]
    fn the_fuel_table_says_what_the_note_says() {
        // Black spruce is kindling with cones on; a hardwood is not.
        assert!(fuel("engine:bog_needles") > fuel("engine:needles"));
        assert!(fuel("engine:needles") > fuel("engine:leaves"));
        assert!(fuel("engine:bog_log") > fuel("engine:spruce_log"));
        assert!(fuel("engine:spruce_log") > fuel("engine:log"));
        // And the things that do not burn, do not burn.
        for solid in [
            "engine:ancient_log",
            "engine:stone",
            "engine:bunker_shell",
            "engine:water",
            "engine:metal_wall",
        ] {
            assert_eq!(fuel(solid), None, "{solid} caught fire");
        }
    }

    #[test]
    fn an_ancient_grove_does_not_burn() {
        let (mut world, at) = stand("engine:ancient_log");
        let mut fire = Fire::new(at);
        assert!(!fire.light(&mut world, at), "an ancient took a light");
        assert!(!fire.alive());
    }

    #[test]
    fn a_bog_goes_up_and_a_cove_barely_catches() {
        let mut bog = stand("engine:bog_needles");
        let mut cove = stand("engine:leaves");
        let sky = dry_sky();
        let burned_bog = burn(&mut bog.0, bog.1, &sky, 0);
        let burned_cove = burn(&mut cove.0, cove.1, &sky, 0);
        assert!(burned_bog > 20, "the bog barely burned: {burned_bog}");
        assert!(
            burned_bog > burned_cove,
            "a cove burned as hard as a bog: {burned_cove} against {burned_bog}"
        );
    }

    #[test]
    fn fire_runs_uphill_and_downwind_and_not_the_other_way() {
        let sky = dry_sky(); // blowing hard toward +x
        let at = BlockPos::new(0, 100, 0);
        let into_wind = spread_chance(
            2024,
            0,
            at,
            at.neighbour(Face::PosX),
            Face::PosX,
            1.0,
            &sky,
        );
        let against = spread_chance(
            2024,
            0,
            at,
            at.neighbour(Face::NegX),
            Face::NegX,
            1.0,
            &sky,
        );
        assert!(
            into_wind > against,
            "fire spread upwind as easily as down: {into_wind} against {against}"
        );

        let up = spread_chance(2024, 0, at, at.neighbour(Face::PosY), Face::PosY, 1.0, &sky);
        let down = spread_chance(2024, 0, at, at.neighbour(Face::NegY), Face::NegY, 1.0, &sky);
        assert!(up > down, "fire fell downhill faster than it climbed");
        assert!(up > into_wind, "slope should beat wind on this grid");
    }

    #[test]
    fn rain_puts_it_out() {
        let wet = Conditions {
            rain: 0.9,
            state: weather::State::Storm,
            humidity: 1.0,
            ..dry_sky()
        };
        let at = BlockPos::new(0, 100, 0);
        let dry_chance = spread_chance(2024, 0, at, at.neighbour(Face::PosX), Face::PosX, 1.0, &dry_sky());
        let wet_chance = spread_chance(2024, 0, at, at.neighbour(Face::PosX), Face::PosX, 1.0, &wet);
        assert!(
            wet_chance < dry_chance * 0.5,
            "a downpour barely slowed it: {wet_chance} against {dry_chance}"
        );
    }

    #[test]
    fn two_identical_fires_burn_the_same_stand() {
        let sky = dry_sky();
        let run = || {
            let (mut world, at) = stand("engine:needles");
            burn(&mut world, at, &sky, 4_096);
            let mut shape = Vec::new();
            for x in 0..10 {
                for z in 0..10 {
                    for y in 150..154 {
                        shape.push(world.block(BlockPos::new(x, y, z)));
                    }
                }
            }
            shape
        };
        assert_eq!(run(), run(), "the same fire burned two different shapes");
    }

    #[test]
    fn a_fire_with_nothing_to_eat_goes_out() {
        let (mut world, at) = stand("engine:needles");
        // One block of fuel in a world of stone.
        let stone = world.registry().id_of("engine:stone").unwrap();
        for x in 0..10 {
            for z in 0..10 {
                for y in 150..154 {
                    world.set_block(BlockPos::new(x, y, z), stone);
                }
            }
        }
        let needles = world.registry().id_of("engine:needles").unwrap();
        world.set_block(at, needles);

        let mut fire = Fire::new(at);
        assert!(fire.light(&mut world, at));
        let mut fires = vec![fire];
        let mut steps = 0;
        while !fires.is_empty() && steps < 4_000 {
            advance_fire(&mut fires, &mut world, 2024, steps, &dry_sky());
            steps += 1;
        }
        assert!(fires.is_empty(), "it is still burning a stone");
        assert!(world.block(at).is_air(), "the one block of fuel survived");
    }
}

