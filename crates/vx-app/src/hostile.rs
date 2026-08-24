//! Deputies: what the warrant sends after you, and how they behave.
//!
//! # Composure is the interesting variable, not aim
//!
//! The Ready or Not lesson, taken whole: what the player reads and plays
//! against is not marksmanship, it is *nerve*. Each deputy holds a scalar
//! composure; events push it down; thresholds move them between modes. Death
//! is one ending — flight, hiding and surrender are the others, and the last
//! one lands in a game that already has warrants, a bounty counter and a
//! fine waiting for it.
//!
//! # One derivation, spent twice
//!
//! Their nerve is the `nerve` byte [`crate::people::Temperament`] derived
//! back when the townsfolk shipped, and their archetype is the same one that
//! decides how chatty they are at a counter. A `Proud` deputy will not
//! surrender and a `Craven` one never bothers repositioning. Character was
//! rolled once, at creation, and this is the second thing to read it — which
//! is why "unpredictable" hostiles stay perfectly deterministic.
//!
//! # Intelligence the player cannot perceive is wasted
//!
//! Every mode transition lands a *bark* through the same toast-and-terminal
//! channel the townsfolk talk on. A squad that sweeps a room in silence may
//! as well be moving at random; the same squad saying "check the far side"
//! is the cleverest thing in the game. Half of AI quality is theatre, and
//! theatre is cheap.
//!
//! # Live-only, and the law does not shoot the scenery
//!
//! Like villagers, the roost and contact marks, none of this reaches the
//! replay oracle: deputies react to where the player is, and reactions are
//! not orders. Their rounds damage *you* and never the world — which is also
//! the honest fiction, since property damage is precisely what they are
//! billing you for.

use glam::Vec3;

use vx_world::World;

use crate::belief::Belief;
use crate::people::{Archetype, Temperament};

/// Composure a deputy starts a callout with.
pub const FULL_COMPOSURE: f32 = 100.0;

// What events cost, from the note's table.
pub const SHOT_HEARD: f32 = 10.0;
pub const ROUND_NEAR_COVER: f32 = 8.0;
pub const ALLY_DOWN: f32 = 20.0;
pub const WOUNDED: f32 = 25.0;
pub const OUTNUMBERED_EACH: f32 = 5.0;
pub const CHALLENGED_UNSEEN: f32 = 15.0;

/// Composure recovered per second while quiet and behind cover.
pub const STEADY_PER_SECOND: f32 = 3.0;

/// Metres within which a round counts as landing "near" somebody.
pub const SUPPRESS_NEAR: f32 = 2.0;

/// How wide an ally's firing lane is, for the cover scorer to avoid.
pub const LANE_WIDTH: f32 = 1.5;

/// Ticks of no progress before an approach is replanned.
pub const STUCK_TICKS: u32 = 40;

/// How far a deputy can shoot, and how often.
pub const ENGAGE_RANGE: f32 = 22.0;
pub const SHOT_INTERVAL: f32 = 1.6;

/// What a deputy is doing about you.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Nothing is wrong yet.
    Patrol,
    /// Something was heard; heading for it.
    Investigate,
    /// Eyes on, and shooting.
    Engage,
    /// Breaking line of sight and holding.
    Hide,
    /// Leaving.
    Flee,
    /// Hands up.
    Surrender,
    /// Out of the fight.
    Down,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Patrol => "PATROL",
            Mode::Investigate => "SEARCHING",
            Mode::Engage => "ENGAGING",
            Mode::Hide => "IN COVER",
            Mode::Flee => "RUNNING",
            Mode::Surrender => "SURRENDERED",
            Mode::Down => "DOWN",
        }
    }
}

/// The composure band a deputy is in, given their nerve.
///
/// `nerve` shifts every threshold: a steady deputy fights on through what
/// would break a nervous one. Archetype then overrides the floor, which is
/// how temperament becomes visible in behaviour rather than in data.
pub fn band(composure: f32, temperament: &Temperament) -> Mode {
    // Nerve runs 0..255; read as a ±20 point shift on every threshold.
    // Subtracted, not added: a steady deputy keeps fighting on *less*
    // composure, so their thresholds sit lower. Adding it makes the most
    // nervous deputy in the county the bravest, which is how this first
    // read of the byte was wrong.
    let steady = (temperament.nerve as f32 / 255.0 - 0.5) * 40.0;
    let mode = if composure > 60.0 - steady {
        Mode::Engage
    } else if composure > 35.0 - steady {
        Mode::Hide
    } else if composure > 15.0 - steady {
        Mode::Flee
    } else {
        Mode::Surrender
    };
    match temperament.archetype {
        // The proud do not put their hands up.
        Archetype::Proud if mode == Mode::Surrender => Mode::Flee,
        // The craven do not bother with cover; they are already leaving.
        Archetype::Craven if mode == Mode::Hide => Mode::Flee,
        other_mode => {
            let _ = other_mode;
            mode
        }
    }
}

/// One deputy.
#[derive(Debug, Clone)]
pub struct Deputy {
    pub position: Vec3,
    pub yaw: f32,
    pub temperament: Temperament,
    pub composure: f32,
    pub mode: Mode,
    /// Seconds until this deputy may fire again.
    pub reload: f32,
    /// Where they are trying to get to.
    pub going: Option<Vec3>,
    /// Ticks since they last closed any distance on `going`.
    stuck: u32,
    /// Distance to `going` when it was last checked.
    last_gap: f32,
    /// Which rig to draw them with.
    pub variant: usize,
    /// Rounds this deputy can still take.
    pub hits: u8,
}

/// Rounds it takes to put a deputy down.
pub const DEPUTY_HITS: u8 = 3;

/// Metres from a passing round that count as a hit on a person.
pub const HIT_RADIUS: f32 = 0.6;

impl Deputy {
    fn new(position: Vec3, temperament: Temperament, variant: usize) -> Self {
        Deputy {
            position,
            yaw: 0.0,
            temperament,
            composure: FULL_COMPOSURE,
            mode: Mode::Patrol,
            reload: 0.0,
            going: None,
            stuck: 0,
            last_gap: f32::MAX,
            variant,
            hits: DEPUTY_HITS,
        }
    }

    /// Knock composure down, and say whether the band changed.
    pub fn rattle(&mut self, cost: f32) -> Option<Mode> {
        let before = band(self.composure, &self.temperament);
        self.composure = (self.composure - cost).max(0.0);
        let after = band(self.composure, &self.temperament);
        (before != after).then_some(after)
    }

    /// Is this deputy still in the fight?
    pub fn active(&self) -> bool {
        !matches!(self.mode, Mode::Down | Mode::Surrender)
    }

    /// Their eye, for line-of-sight questions.
    pub fn eye(&self) -> Vec3 {
        self.position + Vec3::Y * crate::awareness::VILLAGER_EYE
    }
}

/// How good a stance is against the threats a deputy knows about.
///
/// Cover is a query over geometry that already answers questions, not a new
/// annotation layer: occlusion sampled at three eye heights. Blocked
/// standing is a wall to fight from; blocked crouched but open standing is
/// waist-high cover you can peek over; blocked only prone is a last resort.
/// Voxels make this cheap, and it is the very same call the roost uses to
/// witness a crime.
pub fn cover_score(world: &World, at: Vec3, threat: Vec3) -> u32 {
    let registry = world.registry();
    let heights = [
        crate::movement::Stance::Grounded.eye_cm(),
        crate::movement::Stance::Crouched.eye_cm(),
        crate::movement::Stance::Prone.eye_cm(),
    ];
    heights
        .into_iter()
        .filter(|centimetres| {
            let eye = at + Vec3::Y * (*centimetres as f32 / 100.0);
            vx_world::sight::obstruction(world, registry, eye, threat).is_some()
        })
        .count() as u32
}

/// Would a shot from `from` at `at` cross an ally?
///
/// Two cheap checks and an invariant rather than a tuning goal: no deputy
/// ever fires through another. Its absence is the thing that reads as
/// contempt in other games, and enforcing it *at the shot* means the failure
/// cannot exist.
pub fn lane_is_clear(from: Vec3, at: Vec3, allies: &[Vec3]) -> bool {
    let along = at - from;
    let distance = along.length();
    if distance < 1.0e-3 {
        return false;
    }
    let direction = along / distance;
    !allies.iter().any(|ally| {
        let to_ally = *ally - from;
        let ahead = to_ally.dot(direction);
        // Only allies actually between the muzzle and the target matter.
        if !(0.0..distance).contains(&ahead) {
            return false;
        }
        (to_ally - direction * ahead).length() < LANE_WIDTH
    })
}

/// What the posse did this frame that the rest of the game must know about.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    /// Rounds that landed on the player.
    pub hits: u8,
    /// Lines to say, in order.
    pub barks: Vec<String>,
    /// A deputy reached a downed player: the arrest is made.
    pub arrested: bool,
}

/// The squad the warrant sent.
#[derive(Debug, Default)]
pub struct Posse {
    pub deputies: Vec<Deputy>,
    /// What the squad believes about where you are. One board between them:
    /// they are a squad, and squads share what they saw.
    pub belief: Belief,
    /// Seconds of continuous contact, for the back-off that keeps a callout
    /// a rhythm rather than a grind.
    pressure: f32,
    /// Seconds left of a forced stand-down.
    backing_off: f32,
    /// Whether the squad has already said the trail went cold.
    went_cold: bool,
    /// Whether anybody had eyes on the player last frame. Being shot at by
    /// somebody you cannot see is its own line in the composure table, and
    /// this is how `under_fire` knows which one applies.
    exposed: bool,
}

/// Seconds of sustained contact before the squad is made to back off.
pub const PRESSURE_LIMIT: f32 = 90.0;
/// How long that back-off lasts.
pub const BACK_OFF: f32 = 60.0;
/// Deputies a warrant sends.
pub const SQUAD: usize = 3;

impl Posse {
    /// Is anybody out there?
    pub fn called_out(&self) -> bool {
        !self.deputies.is_empty()
    }

    /// Send a squad, spread around `near` at arm's length from the player.
    pub fn call_out(&mut self, near: Vec3, ground: impl Fn(f32, f32) -> f32, seed: u64) {
        if self.called_out() {
            return;
        }
        self.deputies = (0..SQUAD)
            .map(|index| {
                // Fanned out on an arc, never on top of the player: the note's
                // floor distance, so nobody materialises in your face.
                let angle = index as f32 * std::f32::consts::TAU / SQUAD as f32;
                let x = near.x + angle.cos() * 26.0;
                let z = near.z + angle.sin() * 26.0;
                let temperament = crate::people::temperament_from(seed ^ index as u64);
                Deputy::new(
                    Vec3::new(x, ground(x, z), z),
                    temperament,
                    index % 3,
                )
            })
            .collect();
        self.belief = Belief::default();
        self.belief.seen(near);
        self.pressure = 0.0;
        self.backing_off = 0.0;
        self.went_cold = false;
    }

    /// Stand everybody down and forget the whole thing.
    pub fn stand_down(&mut self) {
        self.deputies.clear();
        self.belief.abandon();
        self.pressure = 0.0;
        self.backing_off = 0.0;
    }

    /// What each deputy is doing, for the terminal's `law` verb. Naming
    /// what a squad is up to is half of what makes it look clever.
    pub fn roll_call(&self) -> Vec<String> {
        self.deputies
            .iter()
            .enumerate()
            .map(|(index, deputy)| {
                format!(
                    "DEPUTY {}  {:<12} NERVE {:>3}  {}",
                    index + 1,
                    deputy.mode.name(),
                    deputy.composure as i32,
                    deputy.temperament.archetype.name()
                )
            })
            .collect()
    }

    /// How many deputies are still coming for you, for the HUD.
    pub fn active(&self) -> usize {
        self.deputies.iter().filter(|deputy| deputy.active()).count()
    }

    /// A round of the player's went past. Work out who it hit, who it
    /// nearly hit, and what that does to their nerve.
    ///
    /// This is where the note's composure table actually lands: a hit
    /// wounds, a near miss suppresses, and an ally going down is the most
    /// expensive thing that can happen to a squad. Suppression is composure
    /// spent faster than cover restores it, which hands the player *pinning*
    /// as a verb without a single new system.
    pub fn under_fire(&mut self, from: Vec3, to: Vec3) -> Report {
        let mut report = Report::default();
        if !self.called_out() {
            return report;
        }
        let along = to - from;
        let travel = along.length();
        if travel < 1.0e-3 {
            return report;
        }
        let direction = along / travel;

        let mut downed = 0;
        for deputy in &mut self.deputies {
            if !deputy.active() {
                continue;
            }
            // Nearest approach of the round to this deputy's chest.
            let chest = deputy.position + Vec3::Y;
            let ahead = (chest - from).dot(direction).clamp(0.0, travel);
            let miss = (chest - (from + direction * ahead)).length();

            if miss <= HIT_RADIUS {
                deputy.hits = deputy.hits.saturating_sub(1);
                if deputy.hits == 0 {
                    deputy.mode = Mode::Down;
                    downed += 1;
                    report.barks.push("DEPUTY: MAN DOWN".into());
                } else if let Some(mode) = deputy.rattle(WOUNDED) {
                    if let Some(line) = bark_for(mode) {
                        report.barks.push(line);
                    }
                }
            } else if miss <= SUPPRESS_NEAR {
                // Pinned: near misses cost more nerve than cover gives back.
                if let Some(mode) = deputy.rattle(ROUND_NEAR_COVER) {
                    if let Some(line) = bark_for(mode) {
                        report.barks.push(line);
                    }
                }
            } else if (chest - from).length() < crate::awareness::SIGHT_RANGE * 2.0 {
                // Heard, not felt — and worse when it comes from somewhere
                // they cannot see, which is the note's "challenged from an
                // unseen position" and the reason shooting from cover is
                // worth more than shooting from the open.
                let cost = if self.exposed {
                    SHOT_HEARD * 0.25
                } else {
                    CHALLENGED_UNSEEN * 0.25
                };
                deputy.rattle(cost);
            }
        }

        // Watching a partner go down is the worst thing that happens to a
        // squad, and it happens to everyone still standing.
        for _ in 0..downed {
            for deputy in &mut self.deputies {
                if !deputy.active() {
                    continue;
                }
                if let Some(mode) = deputy.rattle(ALLY_DOWN) {
                    if let Some(line) = bark_for(mode) {
                        report.barks.push(line);
                    }
                }
            }
        }
        report
    }

    /// One frame of the callout.
    ///
    /// `player` is where the body is; `player_down` says whether they are
    /// already on the floor, which turns shooting into arresting.
    pub fn update(
        &mut self,
        dt: f32,
        world: &World,
        player: Vec3,
        player_down: bool,
    ) -> Report {
        let mut report = Report::default();
        if !self.called_out() {
            return report;
        }

        // Pacing: sustained contact forces a stand-off, so a callout is a
        // rhythm rather than a grind.
        if self.backing_off > 0.0 {
            self.backing_off = (self.backing_off - dt).max(0.0);
            if self.backing_off == 0.0 {
                report.barks.push("DEPUTY: MOVING BACK IN".into());
            }
        } else {
            self.pressure += dt;
            if self.pressure >= PRESSURE_LIMIT {
                self.pressure = 0.0;
                self.backing_off = BACK_OFF;
                report.barks.push("DEPUTY: HOLD - FALL BACK AND REGROUP".into());
            }
        }

        let registry = world.registry();
        let eye_of_player = player + Vec3::Y * crate::awareness::PLAYER_EYE;

        // Who can see the player right now? Asked fresh, because sight is
        // the whole game here.
        let seen_by: Vec<bool> = self
            .deputies
            .iter()
            .map(|deputy| {
                deputy.active()
                    && (player - deputy.position).length() <= crate::awareness::SIGHT_RANGE * 1.5
                    && vx_world::sight::sees(
                        world,
                        registry,
                        deputy.eye(),
                        eye_of_player,
                        crate::awareness::SIGHT_RANGE * 1.5,
                    )
            })
            .collect();

        self.exposed = seen_by.iter().any(|seen| *seen);
        if self.exposed {
            // Eyes on: the trail is as warm as it gets, and may go cold
            // again later.
            self.went_cold = false;
            self.belief.seen(player);
        } else {
            self.belief.diffuse(dt, |x, z| {
                // The belief only spreads where a person could stand.
                world
                    .surface_y(x, z)
                    .is_some_and(|top| (top - player.y as i32).abs() < 6)
            });
            // Everything a deputy can see is everything the player is not.
            let eyes: Vec<Vec3> = self
                .deputies
                .iter()
                .filter(|deputy| deputy.active())
                .map(|deputy| deputy.eye())
                .collect();
            self.belief.clear_seen(|x, z| {
                eyes.iter().any(|eye| {
                    let at = Vec3::new(x as f32 + 0.5, eye.y, z as f32 + 0.5);
                    (at - *eye).length() <= crate::awareness::SIGHT_RANGE
                        && vx_world::sight::sees(
                            world,
                            registry,
                            *eye,
                            at,
                            crate::awareness::SIGHT_RANGE,
                        )
                })
            });
        }

        // The trail going cold is the decay constant made audible. Without
        // a line here `LKP_DECAY` would be a number nothing in the game
        // could ever show the player.
        let cold = self.belief.confidence() < 0.35 && self.belief.last_known().is_some();
        if cold && !self.went_cold {
            self.went_cold = true;
            report.barks.push("DEPUTY: TRAIL IS COLD - SPREAD OUT".into());
        }

        // Give up when there is nothing left to believe.
        if !self.belief.searching() && !seen_by.iter().any(|seen| *seen) {
            report.barks.push("DEPUTY: NOTHING HERE. STAND DOWN".into());
            self.stand_down();
            return report;
        }

        let positions: Vec<Vec3> = self.deputies.iter().map(|deputy| deputy.position).collect();
        let outnumbering = self.active();
        let target = self.belief.search_target(player);
        let holding = self.backing_off > 0.0;

        for (index, sees) in seen_by.iter().copied().enumerate() {
            let allies: Vec<Vec3> = positions
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
                .map(|(_, at)| *at)
                .collect();
            let step = Self::step_one(
                &mut self.deputies[index],
                dt,
                world,
                player,
                player_down,
                sees,
                &allies,
                outnumbering,
                target,
                holding,
                &mut report,
            );
            if step {
                report.hits += 1;
            }
        }
        report
    }

    /// One deputy's frame. Returns whether they landed a round.
    #[allow(clippy::too_many_arguments)]
    fn step_one(
        deputy: &mut Deputy,
        dt: f32,
        world: &World,
        player: Vec3,
        player_down: bool,
        sees: bool,
        allies: &[Vec3],
        outnumbering: usize,
        search: Option<(i32, i32)>,
        holding: bool,
        report: &mut Report,
    ) -> bool {
        if !deputy.active() {
            return false;
        }
        deputy.reload = (deputy.reload - dt).max(0.0);

        // Quiet and unseen: nerve comes back, but never above where it
        // started — a callout wears people down over its whole length.
        if !sees {
            deputy.composure =
                (deputy.composure + STEADY_PER_SECOND * dt).min(FULL_COMPOSURE);
        } else if outnumbering < 1 {
            deputy.rattle(OUTNUMBERED_EACH * dt);
        }

        let was = deputy.mode;
        deputy.mode = if sees {
            band(deputy.composure, &deputy.temperament)
        } else if deputy.mode == Mode::Engage {
            Mode::Investigate
        } else {
            deputy.mode.max_investigate()
        };
        if deputy.mode != was {
            if let Some(line) = bark_for(deputy.mode) {
                report.barks.push(line);
            }
        }

        // A player already on the floor is arrested, not shot.
        if player_down {
            let gap = (player - deputy.position).length();
            if gap < 2.0 {
                report.arrested = true;
                return false;
            }
            Self::walk(deputy, dt, player, 3.4, world);
            return false;
        }

        match deputy.mode {
            Mode::Engage if !holding => {
                let gap = (player - deputy.position).length();
                deputy.face(player);
                if gap > ENGAGE_RANGE {
                    Self::walk(deputy, dt, player, 3.0, world);
                    return false;
                }
                // Fire discipline, enforced at the shot: never through an
                // ally, no exceptions, so the failure cannot exist.
                if deputy.reload > 0.0 || !lane_is_clear(deputy.eye(), player, allies) {
                    // Step aside rather than stand in a friend's lane.
                    Self::sidestep(deputy, dt, player);
                    return false;
                }
                deputy.reload = SHOT_INTERVAL;
                true
            }
            Mode::Hide | Mode::Engage => {
                // Find the best stance nearby against the threat we know
                // about. Re-scored every frame, so when the belief moves the
                // score is stale and they scramble — which *is* the flanking
                // mechanic, with no flanking code in it.
                if let Some(spot) = best_cover(world, deputy.position, player) {
                    Self::walk(deputy, dt, spot, 2.6, world);
                }
                deputy.face(player);
                false
            }
            Mode::Flee => {
                let away = deputy.position - player;
                if away.length() > 1.0e-3 {
                    let to = deputy.position + away.normalize() * 8.0;
                    Self::walk(deputy, dt, to, 3.8, world);
                }
                false
            }
            Mode::Investigate | Mode::Patrol => {
                if let Some((x, z)) = search {
                    let to = Vec3::new(x as f32 + 0.5, deputy.position.y, z as f32 + 0.5);
                    Self::walk(deputy, dt, to, 2.4, world);
                    deputy.face(to);
                }
                false
            }
            Mode::Surrender | Mode::Down => false,
        }
    }

    /// Walk toward a spot, with the progress watchdog running.
    ///
    /// An agent that has closed no distance in [`STUCK_TICKS`] gives the
    /// approach up rather than moonwalking into a wall — the one failure
    /// players never forgive, bounded here to well under a second.
    fn walk(deputy: &mut Deputy, dt: f32, to: Vec3, speed: f32, world: &World) {
        let along = Vec3::new(to.x - deputy.position.x, 0.0, to.z - deputy.position.z);
        let gap = along.length();
        if gap < 0.05 {
            deputy.stuck = 0;
            return;
        }
        if gap < deputy.last_gap - 0.01 {
            deputy.stuck = 0;
            deputy.last_gap = gap;
        } else {
            deputy.stuck += 1;
            if deputy.stuck > STUCK_TICKS {
                deputy.going = None;
                deputy.stuck = 0;
                deputy.last_gap = f32::MAX;
                return;
            }
        }
        deputy.going = Some(to);
        deputy.position += along / gap * speed * dt;
        // Stay on the ground. Walking in x and z alone leaves a deputy
        // hovering off a bank or buried in a rise the moment the terrain
        // stops being flat, which is the first thing anybody notices.
        if let Some(top) = world.surface_y(
            deputy.position.x.floor() as i32,
            deputy.position.z.floor() as i32,
        ) {
            deputy.position.y = (top + 1) as f32;
        }
        deputy.face(to);
    }

    /// Move out of an ally's line rather than standing in it.
    fn sidestep(deputy: &mut Deputy, dt: f32, player: Vec3) {
        let along = player - deputy.position;
        let across = Vec3::new(-along.z, 0.0, along.x);
        if across.length() > 1.0e-3 {
            deputy.position += across.normalize() * 2.0 * dt;
        }
    }
}

impl Mode {
    /// Anything short of a fight becomes a search once contact is lost.
    fn max_investigate(self) -> Mode {
        match self {
            Mode::Patrol => Mode::Investigate,
            other => other,
        }
    }
}

impl Deputy {
    fn face(&mut self, at: Vec3) {
        if let Some(yaw) = crate::rig::yaw_towards(at.x - self.position.x, at.z - self.position.z) {
            self.yaw = yaw;
        }
    }
}

/// The line a deputy says on entering a mode. Deterministic, like
/// everything else — the same transition always sounds the same.
fn bark_for(mode: Mode) -> Option<String> {
    match mode {
        Mode::Engage => Some("DEPUTY: CONTACT - HOLD IT RIGHT THERE".into()),
        Mode::Hide => Some("DEPUTY: TAKING COVER".into()),
        Mode::Flee => Some("DEPUTY: FALL BACK, FALL BACK".into()),
        Mode::Surrender => Some("DEPUTY: DONT SHOOT - IM DONE".into()),
        Mode::Investigate => Some("DEPUTY: CHECK THE FAR SIDE".into()),
        Mode::Patrol | Mode::Down => None,
    }
}

/// The best spot within reach to stand against one threat.
///
/// Scores a ring of candidates by how many stances the geometry covers,
/// nearest winning ties, so the answer is deterministic.
pub fn best_cover(world: &World, from: Vec3, threat: Vec3) -> Option<Vec3> {
    let mut best: Option<(Vec3, u32, f32)> = None;
    for step in 0..12 {
        let angle = step as f32 * std::f32::consts::TAU / 12.0;
        for reach in [2.0f32, 4.0, 6.0] {
            let at = Vec3::new(
                from.x + angle.cos() * reach,
                from.y,
                from.z + angle.sin() * reach,
            );
            let score = cover_score(world, at, threat);
            if score == 0 {
                continue;
            }
            let away = (at - from).length();
            let better = match &best {
                None => true,
                Some((_, top, closest)) => score > *top || (score == *top && away < *closest),
            };
            if better {
                best = Some((at, score, away));
            }
        }
    }
    best.map(|(at, _, _)| at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::people::Archetype;

    fn temperament(archetype: Archetype, nerve: u8) -> Temperament {
        Temperament {
            archetype,
            nerve,
            warmth: 128,
            voice: 7,
        }
    }

    #[test]
    fn composure_walks_down_the_ladder_in_order() {
        let steady = temperament(Archetype::Steady, 128);
        assert_eq!(band(100.0, &steady), Mode::Engage);
        assert_eq!(band(50.0, &steady), Mode::Hide);
        assert_eq!(band(25.0, &steady), Mode::Flee);
        assert_eq!(band(5.0, &steady), Mode::Surrender);
    }

    #[test]
    fn nerve_moves_every_threshold() {
        // The same blow that breaks a nervous deputy leaves a steady one
        // fighting. This is stage 23's derived byte, finally read.
        let nervous = temperament(Archetype::Steady, 0);
        let stoic = temperament(Archetype::Steady, 255);
        let under_fire = 60.0;
        assert_eq!(band(under_fire, &nervous), Mode::Hide);
        assert_eq!(band(under_fire, &stoic), Mode::Engage);
        // And all the way down: what makes a stoic deputy run would have a
        // nervous one already surrendering.
        assert_eq!(band(20.0, &nervous), Mode::Surrender);
        assert_eq!(band(20.0, &stoic), Mode::Hide);
    }

    #[test]
    fn a_proud_deputy_runs_and_a_craven_one_never_takes_cover() {
        // Temperament observable in outcome, not just in data — the note
        // asks for exactly this pair.
        let proud = temperament(Archetype::Proud, 128);
        let craven = temperament(Archetype::Craven, 128);
        assert_eq!(band(1.0, &proud), Mode::Flee, "the proud surrendered");
        assert_ne!(band(1.0, &craven), Mode::Hide);
        assert_eq!(band(50.0, &craven), Mode::Flee, "the craven took cover");
        // And a steady one in the same spot does put their hands up.
        assert_eq!(band(1.0, &temperament(Archetype::Steady, 128)), Mode::Surrender);
    }

    #[test]
    fn rattling_reports_only_the_transitions() {
        let mut deputy = Deputy::new(Vec3::ZERO, temperament(Archetype::Steady, 128), 0);
        assert_eq!(deputy.rattle(1.0), None, "a scratch changed the mode");
        assert_eq!(deputy.rattle(45.0), Some(Mode::Hide));
        assert_eq!(deputy.rattle(1.0), None);
        assert_eq!(deputy.rattle(25.0), Some(Mode::Flee));
        // Composure floors at zero rather than going negative.
        deputy.rattle(1_000.0);
        assert_eq!(deputy.composure, 0.0);
    }

    #[test]
    fn nobody_ever_fires_through_a_friend() {
        // An invariant, not a tuning goal. An ally anywhere in the lane
        // blocks the shot; one behind you or off to the side does not.
        let muzzle = Vec3::new(0.0, 70.0, 0.0);
        let target = Vec3::new(10.0, 70.0, 0.0);
        assert!(lane_is_clear(muzzle, target, &[]));
        assert!(
            !lane_is_clear(muzzle, target, &[Vec3::new(5.0, 70.0, 0.4)]),
            "fired straight through a deputy"
        );
        assert!(
            !lane_is_clear(muzzle, target, &[Vec3::new(9.0, 70.0, -1.0)]),
            "fired past a deputy inside the lane"
        );
        // Behind the muzzle, past the target, and well off to one side are
        // all fine.
        assert!(lane_is_clear(muzzle, target, &[Vec3::new(-3.0, 70.0, 0.0)]));
        assert!(lane_is_clear(muzzle, target, &[Vec3::new(14.0, 70.0, 0.0)]));
        assert!(lane_is_clear(muzzle, target, &[Vec3::new(5.0, 70.0, 3.0)]));
    }

    #[test]
    fn the_lane_check_is_symmetric_in_the_only_way_that_matters() {
        // A crossfire from many angles, thousands of samples, and never once
        // a round through a friend.
        let target = Vec3::new(0.0, 70.0, 0.0);
        for step in 0..360 {
            let angle = step as f32 * std::f32::consts::TAU / 360.0;
            let muzzle = Vec3::new(angle.cos() * 12.0, 70.0, angle.sin() * 12.0);
            // An ally placed exactly halfway must always block.
            let ally = (muzzle + target) * 0.5;
            assert!(
                !lane_is_clear(muzzle, target, &[ally]),
                "a shot from {step} degrees crossed an ally"
            );
        }
    }

    #[test]
    fn every_bark_is_drawable() {
        for mode in [
            Mode::Patrol,
            Mode::Investigate,
            Mode::Engage,
            Mode::Hide,
            Mode::Flee,
            Mode::Surrender,
            Mode::Down,
        ] {
            for line in bark_for(mode).into_iter().chain([mode.name().to_string()]) {
                for character in line.chars() {
                    assert!(vx_render::font::knows(character), "undrawable {character:?}");
                }
            }
        }
    }
}
