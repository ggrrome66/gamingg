//! The sensations the movement round deferred.
//!
//! The mechanics shipped and the feel did not, and stiffness is what a
//! playtester feels when a body accelerates correctly and tells them nothing
//! about it. This module is the layer that talks back: a sprint widens the
//! lens, a landing drives the eye down and springs it back, a slide leans into
//! its own direction.
//!
//! # Presentation, and nothing but
//!
//! Everything here is an *offset*. Nothing in this module is read by the
//! simulation, and nothing it produces is ever written back into a body, a
//! velocity or a stance — [`Feel::advance`] takes sim state by value and
//! returns nothing. That is the property the whole round rests on: turning
//! every effect off changes what the frame looks like and cannot change where
//! the player ends up. It is also why the bob and the roll are handed to the
//! renderer as view-only angles rather than folded into `Camera::yaw` and
//! `Camera::pitch` — those two are the aim vector, and a weapon that drifts
//! with the walk cycle is a bug, not a sensation.
//!
//! # Deterministic, so captures stay byte-identical
//!
//! [`Feel::advance`] is called once per *movement tick* with the same fixed
//! `dt` the simulation uses, from sim state alone. No wall clock, no frame
//! time, no random source: the same journal replays to the same offsets, so a
//! capture taken with every effect on matches one taken from the same tick in
//! another run, bit for bit. Effects that eased on frame time — the tempting
//! shape — would make every capture a race against the frame rate.
//!
//! # Every one of them is someone's motion sickness
//!
//! Hence [`FeelSettings`]: a toggle per effect, and strafe roll off by default
//! because a horizon that tilts when you sidestep is the single most reliable
//! way to make someone put the game down.

use glam::Vec3;

use crate::movement::Stance;

// --- how big, and how fast -------------------------------------------------

/// How much wider the lens goes at a sprint.
pub const SPRINT_FOV: f32 = 8.0;
/// How long the sprint kick takes to arrive, and to leave.
pub const SPRINT_FOV_EASE: f32 = 0.150;
/// How much wider the lens goes in a slide, at [`SLIDE_FOV_AT`] and above.
pub const SLIDE_FOV: f32 = 12.0;
/// The slide speed that earns the full [`SLIDE_FOV`]; below it the kick
/// decays with the slide, which is what makes a slide read as *losing* speed.
pub const SLIDE_FOV_AT: f32 = 9.0;
/// How far the eye drops on the hardest survivable landing.
pub const LANDING_DIP: f32 = 0.28;
/// Fall speed that earns the full [`LANDING_DIP`].
pub const LANDING_DIP_AT: f32 = 18.0;
/// Below this there was no impact worth showing — a walked-down step.
pub const LANDING_DIP_FLOOR: f32 = 3.5;
/// Stiffness and damping of the spring the eye comes back up on.
pub const DIP_STIFFNESS: f32 = 190.0;
pub const DIP_DAMPING: f32 = 21.0;
/// How far the view swings at each end of a step.
pub const BOB_DEGREES: f32 = 1.2;
/// Radians of walk cycle per metre travelled. A stride is a little under a
/// metre, and a cycle is two strides.
pub const BOB_CADENCE: f32 = 3.6;
/// How far the horizon tilts into a full-speed sidestep.
pub const STRAFE_ROLL: f32 = 1.5;
/// How far it tilts into a slide.
pub const SLIDE_TILT: f32 = 2.0;
/// Lateral speed earning the full [`STRAFE_ROLL`].
pub const STRAFE_ROLL_AT: f32 = 4.3;
/// How quickly roll and bob amplitude chase their targets.
pub const ROLL_EASE: f32 = 0.120;
pub const BOB_EASE: f32 = 0.200;

/// Which effects are switched on. One flag each, because every one of them is
/// someone's motion sickness and a single "screen shake" master toggle is how
/// you end up with players turning off all of it to be rid of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeelSettings {
    pub sprint_fov: bool,
    pub slide_fov: bool,
    pub landing_dip: bool,
    pub view_bob: bool,
    /// Off by default. A tilting horizon on every sidestep is the effect most
    /// likely to make someone feel ill.
    pub strafe_roll: bool,
    pub slide_tilt: bool,
}

impl Default for FeelSettings {
    fn default() -> Self {
        FeelSettings {
            sprint_fov: true,
            slide_fov: true,
            landing_dip: true,
            view_bob: true,
            strafe_roll: false,
            slide_tilt: true,
        }
    }
}

// The two presets and `is_settled` are the shape the Settings screen will
// reach for — an "all off" button, an "everything on" button, and a way to ask
// whether the view is currently displaced. The pause menu that hosts them is
// its own piece of the polish round, so for now only the tests call them.
#[allow(dead_code)]
impl FeelSettings {
    /// Every effect off. The reference state: a capture taken with this must
    /// match one taken before the feel pass existed.
    pub const NONE: FeelSettings = FeelSettings {
        sprint_fov: false,
        slide_fov: false,
        landing_dip: false,
        view_bob: false,
        strafe_roll: false,
        slide_tilt: false,
    };

    /// Every effect on, strafe roll included. What the toggles can reach, and
    /// what the determinism tests run with.
    pub const ALL: FeelSettings = FeelSettings {
        sprint_fov: true,
        slide_fov: true,
        landing_dip: true,
        view_bob: true,
        strafe_roll: true,
        slide_tilt: true,
    };
}

/// What one tick of feel adds to the camera.
///
/// Angles in radians, the drop in metres. All four are *additions* to a camera
/// the simulation already placed; none of them replaces anything.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FeelOffsets {
    /// Added to `Camera::fov_y`.
    pub fov: f32,
    /// Subtracted from the eye's height. Positive means the eye is down.
    pub eye_drop: f32,
    /// Roll about the view axis: strafe lean and slide tilt together.
    pub roll: f32,
    /// The walk cycle's vertical swing. A *view* offset, never added to the
    /// pitch that aims.
    pub pitch: f32,
}

/// The live state behind the offsets.
///
/// Advanced once per movement tick and read once per frame. Cheap to copy so
/// it can ride in the same struct as the rest of the session state.
#[derive(Debug, Clone, Copy)]
pub struct Feel {
    pub settings: FeelSettings,
    /// Degrees of lens kick currently applied, eased toward its target.
    fov: f32,
    /// Where in the walk cycle the last step left us, in radians.
    bob_phase: f32,
    /// How much of the bob is currently expressed. Eased so that stopping
    /// dead settles the view instead of freezing it mid-swing.
    bob_amount: f32,
    /// The eye's displacement from the landing spring, in metres, and its
    /// velocity. Positive displacement is downward.
    dip: f32,
    dip_rate: f32,
    /// Degrees of roll currently applied, tracked per effect.
    ///
    /// Two values rather than one sum, because the two have separate toggles:
    /// a single eased total cannot be split back into its parts (a sidestep
    /// with slide tilt on and strafe roll off would still lean), and the
    /// arithmetic that pretends otherwise is wrong in exactly the case the
    /// toggles exist for.
    roll_strafe: f32,
    roll_slide: f32,
    /// Whether the previous tick was off the ground, so a landing can be seen.
    airborne: bool,
    /// The previous tick's vertical velocity. Read on the landing tick, when
    /// the sweep has already zeroed the body's own copy.
    fall: f32,
}

impl Default for Feel {
    fn default() -> Self {
        Feel::new(FeelSettings::default())
    }
}

impl Feel {
    pub fn new(settings: FeelSettings) -> Self {
        Feel {
            settings,
            fov: 0.0,
            bob_phase: 0.0,
            bob_amount: 0.0,
            dip: 0.0,
            dip_rate: 0.0,
            roll_strafe: 0.0,
            roll_slide: 0.0,
            airborne: true,
            fall: 0.0,
        }
    }

    /// Advance one movement tick from the state the simulation just produced.
    ///
    /// Takes everything by value and returns nothing: there is no path from
    /// here back into the body. `yaw` is the look angle the tick was commanded
    /// with — quantised before it ever reaches here, which is what keeps the
    /// lateral split replay-exact.
    pub fn advance(&mut self, stance: Stance, velocity: Vec3, yaw: f32, dt: f32) {
        let grounded = stance.is_grounded();
        let sliding = matches!(stance, Stance::Sliding { .. });
        let horizontal = Vec3::new(velocity.x, 0.0, velocity.z);
        let speed = horizontal.length();

        self.advance_fov(stance, sliding, speed, dt);
        self.advance_dip(grounded, velocity.y, dt);
        self.advance_bob(stance, grounded, sliding, speed, dt);
        self.advance_roll(sliding, horizontal, yaw, dt);

        self.airborne = !grounded;
        self.fall = velocity.y;
    }

    /// The lens. A sprint widens it; a slide widens it further and gives the
    /// width back as the slide dies, which is what sells a slide as speed
    /// being spent rather than held.
    fn advance_fov(&mut self, stance: Stance, sliding: bool, speed: f32, dt: f32) {
        let target = if sliding {
            SLIDE_FOV * (speed / SLIDE_FOV_AT).clamp(0.0, 1.0)
        } else if matches!(stance, Stance::Sprinting) {
            SPRINT_FOV
        } else {
            0.0
        };
        // A fixed rate rather than an exponential chase: "eased over 150 ms"
        // should mean 150 ms, and an exponential never actually arrives.
        let step = (SPRINT_FOV.max(SLIDE_FOV) / SPRINT_FOV_EASE) * dt;
        self.fov = approach(self.fov, target, step);
    }

    /// The landing. An impact kicks the eye down proportionally to the fall it
    /// survived, and a spring brings it back — critically enough damped that
    /// it never bounces twice.
    fn advance_dip(&mut self, grounded: bool, vertical: f32, dt: f32) {
        if grounded && self.airborne {
            // `self.fall` is the previous tick's velocity: on this tick the
            // sweep has already zeroed the body's, so reading it here would
            // report every landing as feather-light.
            let fell = (-self.fall).max(0.0);
            if fell > LANDING_DIP_FLOOR {
                let hardness = ((fell - LANDING_DIP_FLOOR)
                    / (LANDING_DIP_AT - LANDING_DIP_FLOOR))
                    .clamp(0.0, 1.0);
                self.dip_rate += LANDING_DIP * hardness * DIP_STIFFNESS.sqrt();
            }
        }
        let _ = vertical;

        // A damped spring back toward the eye's true height.
        let accel = -DIP_STIFFNESS * self.dip - DIP_DAMPING * self.dip_rate;
        self.dip_rate += accel * dt;
        self.dip += self.dip_rate * dt;
        if self.dip.abs() < 1.0e-5 && self.dip_rate.abs() < 1.0e-4 {
            self.dip = 0.0;
            self.dip_rate = 0.0;
        }
    }

    /// The walk cycle. Phase advances with *distance*, not time, so the view
    /// keeps step with the feet at any speed and stops dead when you do.
    fn advance_bob(&mut self, stance: Stance, grounded: bool, sliding: bool, speed: f32, dt: f32) {
        let walking = grounded
            && !sliding
            && matches!(stance, Stance::Grounded | Stance::Sprinting)
            && speed > 0.1;
        self.bob_phase = (self.bob_phase + speed * BOB_CADENCE * dt) % std::f32::consts::TAU;
        let target = if walking {
            (speed / crate::movement::SPRINT_SPEED).clamp(0.0, 1.2)
        } else {
            0.0
        };
        self.bob_amount = approach(self.bob_amount, target, dt / BOB_EASE);
    }

    /// The lean. A slide tilts into its own direction; a sidestep tilts into
    /// the strafe, for anyone who has asked for it.
    fn advance_roll(&mut self, sliding: bool, horizontal: Vec3, yaw: f32, dt: f32) {
        // Right, level with the horizon — the same construction `Camera::right`
        // uses, rebuilt here so this module needs no camera.
        let (sin_yaw, cos_yaw) = yaw.sin_cos();
        let right = Vec3::new(cos_yaw, 0.0, sin_yaw);
        let lateral = horizontal.dot(right);

        let lean = (lateral / STRAFE_ROLL_AT).clamp(-1.0, 1.0);

        // Leaning *into* the turn, so the horizon tilts the way a motorcyclist
        // would: sidestepping right drops the right side of the view.
        let strafe_target = -STRAFE_ROLL * lean;
        let slide_target = if sliding { SLIDE_TILT * lean } else { 0.0 };

        self.roll_strafe = approach(
            self.roll_strafe,
            strafe_target,
            (STRAFE_ROLL / ROLL_EASE) * dt,
        );
        self.roll_slide = approach(
            self.roll_slide,
            slide_target,
            (SLIDE_TILT / ROLL_EASE) * dt,
        );
    }

    /// What to add to the camera this frame, with the switched-off effects
    /// left out. The state keeps advancing either way, so a toggle flipped
    /// mid-run takes effect on the next frame rather than on the next landing.
    pub fn offsets(&self) -> FeelOffsets {
        let sliding_fov = self.settings.slide_fov;
        let sprint_fov = self.settings.sprint_fov;
        // One eased value serves both kicks, so a toggle picks whether the
        // value is *shown*, not whether it is tracked.
        let fov = if sprint_fov || sliding_fov { self.fov } else { 0.0 };

        // Each toggle drops exactly its own contribution, which is only
        // possible because the two are eased separately.
        let mut roll = 0.0;
        if self.settings.strafe_roll {
            roll += self.roll_strafe;
        }
        if self.settings.slide_tilt {
            roll += self.roll_slide;
        }

        FeelOffsets {
            fov: fov.to_radians(),
            eye_drop: if self.settings.landing_dip { self.dip } else { 0.0 },
            roll: roll.to_radians(),
            pitch: if self.settings.view_bob {
                (BOB_DEGREES * self.bob_amount * self.bob_phase.sin()).to_radians()
            } else {
                0.0
            },
        }
    }

    /// True while nothing is displaced — the state a fresh `Feel` is in, and
    /// the one a still player settles back to.
    #[allow(dead_code)]
    pub fn is_settled(&self) -> bool {
        self.offsets() == FeelOffsets::default()
    }
}

/// Move `value` toward `target` by at most `step`. The shape every eased
/// number here uses: it arrives, and it arrives when the constant says.
fn approach(value: f32, target: f32, step: f32) -> f32 {
    if value < target {
        (value + step).min(target)
    } else {
        (value - step).max(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movement::MOVE_TICK;

    fn sprinting() -> Stance {
        Stance::Sprinting
    }

    /// Run `ticks` of a held state and hand back the feel.
    fn run(feel: &mut Feel, stance: Stance, velocity: Vec3, ticks: u32) {
        for _ in 0..ticks {
            feel.advance(stance, velocity, 0.0, MOVE_TICK);
        }
    }

    #[test]
    fn a_fresh_feel_adds_nothing() {
        assert!(Feel::default().is_settled());
        assert_eq!(Feel::default().offsets(), FeelOffsets::default());
    }

    #[test]
    fn every_effect_off_is_exactly_the_camera_the_simulation_placed() {
        // The reference state: with the toggles down, the feel pass is not
        // merely small, it is absent. Anything else means a player who turned
        // the effects off is still being moved around.
        let mut feel = Feel::new(FeelSettings::NONE);
        run(&mut feel, sprinting(), Vec3::new(6.5, 0.0, 0.0), 64);
        feel.advance(Stance::Airborne { coyote: 0 }, Vec3::new(0.0, -20.0, 0.0), 0.0, MOVE_TICK);
        run(&mut feel, Stance::Grounded, Vec3::ZERO, 4);

        assert_eq!(feel.offsets(), FeelOffsets::default());
        assert!(feel.is_settled());
    }

    #[test]
    fn sprinting_widens_the_lens_and_letting_go_gives_it_back() {
        let mut feel = Feel::default();
        let quick = Vec3::new(0.0, 0.0, -crate::movement::SPRINT_SPEED);

        // Eased over 150 ms, so a tenth of a second in it is partway there.
        run(&mut feel, sprinting(), quick, 6);
        let partway = feel.offsets().fov;
        assert!(partway > 0.0, "the lens never opened");
        assert!(
            partway < SPRINT_FOV.to_radians(),
            "the kick arrived instantly rather than easing"
        );

        // Well past the ease, it is all the way open and stays there.
        run(&mut feel, sprinting(), quick, 30);
        let open = feel.offsets().fov;
        assert!(
            (open - SPRINT_FOV.to_radians()).abs() < 1.0e-4,
            "expected {SPRINT_FOV} degrees, got {}",
            open.to_degrees()
        );

        // Back to a walk and it closes again.
        run(&mut feel, Stance::Grounded, Vec3::new(0.0, 0.0, -4.3), 30);
        assert!(
            feel.offsets().fov.abs() < 1.0e-4,
            "the lens stayed open at a walk: {}",
            feel.offsets().fov.to_degrees()
        );
    }

    #[test]
    fn the_slide_kick_decays_with_the_slide() {
        // The point of the slide kick: it tracks the speed, so a slide that is
        // running out looks like one.
        let mut feel = Feel::default();
        let fast = Vec3::new(0.0, 0.0, -SLIDE_FOV_AT);
        run(&mut feel, Stance::Sliding { ticks: 4 }, fast, 40);
        let quick = feel.offsets().fov;
        assert!(
            (quick - SLIDE_FOV.to_radians()).abs() < 1.0e-4,
            "a full-speed slide should reach {SLIDE_FOV} degrees, got {}",
            quick.to_degrees()
        );

        let slow = Vec3::new(0.0, 0.0, -SLIDE_FOV_AT / 3.0);
        run(&mut feel, Stance::Sliding { ticks: 40 }, slow, 40);
        let dying = feel.offsets().fov;
        assert!(dying < quick, "the kick did not decay with the slide");
        assert!(dying > 0.0, "the kick vanished rather than decayed");
    }

    #[test]
    fn a_landing_drops_the_eye_and_springs_it_back() {
        let mut feel = Feel::default();
        // Falling, then caught.
        feel.advance(
            Stance::Airborne { coyote: 0 },
            Vec3::new(0.0, -14.0, 0.0),
            0.0,
            MOVE_TICK,
        );
        feel.advance(Stance::Grounded, Vec3::ZERO, 0.0, MOVE_TICK);

        let dipped = feel.offsets().eye_drop;
        assert!(dipped > 0.01, "the eye did not drop on landing: {dipped}");
        assert!(
            dipped < LANDING_DIP * 2.0,
            "the dip overshot wildly: {dipped}"
        );

        // The spring brings it home and leaves it there.
        run(&mut feel, Stance::Grounded, Vec3::ZERO, 128);
        assert!(
            feel.offsets().eye_drop.abs() < 1.0e-3,
            "the eye never came back up: {}",
            feel.offsets().eye_drop
        );
    }

    #[test]
    fn a_harder_landing_dips_further() {
        let drop_for = |speed: f32| {
            let mut feel = Feel::default();
            feel.advance(
                Stance::Airborne { coyote: 0 },
                Vec3::new(0.0, -speed, 0.0),
                0.0,
                MOVE_TICK,
            );
            // A few ticks in, so the spring has expressed the impulse.
            run(&mut feel, Stance::Grounded, Vec3::ZERO, 3);
            feel.offsets().eye_drop
        };

        let gentle = drop_for(6.0);
        let hard = drop_for(17.0);
        assert!(
            hard > gentle,
            "a 17 m/s landing ({hard}) should dip further than a 6 m/s one ({gentle})"
        );
    }

    #[test]
    fn stepping_off_a_kerb_does_not_dip_the_view() {
        // Below the floor there was no impact worth showing. Without this,
        // walking down a staircase bobs the eye on every step.
        let mut feel = Feel::default();
        feel.advance(
            Stance::Airborne { coyote: 0 },
            Vec3::new(0.0, -LANDING_DIP_FLOOR * 0.5, 0.0),
            0.0,
            MOVE_TICK,
        );
        run(&mut feel, Stance::Grounded, Vec3::ZERO, 2);
        assert!(
            feel.offsets().eye_drop.abs() < 1.0e-4,
            "a kerb dipped the view: {}",
            feel.offsets().eye_drop
        );
    }

    #[test]
    fn the_view_bobs_while_walking_and_settles_when_you_stop() {
        let mut feel = Feel::default();
        let walk = Vec3::new(0.0, 0.0, -4.3);

        let mut swing: f32 = 0.0;
        for _ in 0..96 {
            feel.advance(Stance::Grounded, walk, 0.0, MOVE_TICK);
            swing = swing.max(feel.offsets().pitch.abs());
        }
        assert!(swing > 0.0, "the view never bobbed");
        assert!(
            swing <= (BOB_DEGREES * 1.2).to_radians() + 1.0e-6,
            "the bob exceeded its amplitude: {} degrees",
            swing.to_degrees()
        );

        // Standing still: the amplitude eases out, so the view settles rather
        // than freezing mid-swing.
        run(&mut feel, Stance::Grounded, Vec3::ZERO, 64);
        assert!(
            feel.offsets().pitch.abs() < 1.0e-4,
            "the view kept bobbing at a standstill: {}",
            feel.offsets().pitch
        );
    }

    #[test]
    fn the_view_does_not_bob_in_the_air_or_in_a_slide() {
        // Bob is a walk cycle. Feet off the ground, no cycle.
        let mut feel = Feel::default();
        run(
            &mut feel,
            Stance::Airborne { coyote: 0 },
            Vec3::new(0.0, -2.0, -6.0),
            64,
        );
        assert!(
            feel.offsets().pitch.abs() < 1.0e-4,
            "the view bobbed in mid-air"
        );

        let mut feel = Feel::default();
        run(&mut feel, Stance::Sliding { ticks: 3 }, Vec3::new(0.0, 0.0, -8.0), 64);
        assert!(
            feel.offsets().pitch.abs() < 1.0e-4,
            "the view bobbed during a slide"
        );
    }

    #[test]
    fn strafe_roll_leans_into_the_sidestep_and_is_off_by_default() {
        // Moving right, looking down -Z: `right` is +X, so the lateral is
        // positive and the horizon leans the other way.
        let sidestep = Vec3::new(4.3, 0.0, 0.0);

        let mut stock = Feel::default();
        run(&mut stock, Stance::Grounded, sidestep, 40);
        assert!(
            stock.offsets().roll.abs() < 1.0e-6,
            "strafe roll is on by default; it is the one that makes people ill"
        );

        let mut opted_in = Feel::new(FeelSettings {
            strafe_roll: true,
            ..FeelSettings::default()
        });
        run(&mut opted_in, Stance::Grounded, sidestep, 40);
        let roll = opted_in.offsets().roll;
        assert!(roll.abs() > 0.0, "opting in did not lean the horizon");
        assert!(
            roll.abs() <= STRAFE_ROLL.to_radians() + 1.0e-6,
            "the lean exceeded its amplitude: {} degrees",
            roll.to_degrees()
        );

        // The other way leans the other way.
        let mut mirrored = Feel::new(FeelSettings {
            strafe_roll: true,
            ..FeelSettings::default()
        });
        run(&mut mirrored, Stance::Grounded, -sidestep, 40);
        assert!(
            mirrored.offsets().roll * roll < 0.0,
            "sidestepping the other way leaned the same way"
        );
    }

    #[test]
    fn a_toggle_removes_exactly_its_own_effect() {
        // The toggles are separate because the effects are separate. Turning
        // the bob off must not disturb the lens, and so on round the table.
        let sequence = |settings: FeelSettings| {
            let mut feel = Feel::new(settings);
            run(&mut feel, sprinting(), Vec3::new(2.0, 0.0, -6.0), 40);
            feel.advance(
                Stance::Airborne { coyote: 0 },
                Vec3::new(0.0, -12.0, 0.0),
                0.0,
                MOVE_TICK,
            );
            run(&mut feel, Stance::Grounded, Vec3::new(2.0, 0.0, -4.0), 3);
            feel.offsets()
        };

        let all = sequence(FeelSettings::ALL);
        assert!(all.fov > 0.0 && all.eye_drop > 0.0 && all.pitch != 0.0 && all.roll != 0.0);

        let no_bob = sequence(FeelSettings {
            view_bob: false,
            ..FeelSettings::ALL
        });
        assert_eq!(no_bob.pitch, 0.0, "the bob toggle did not remove the bob");
        assert_eq!(no_bob.fov, all.fov, "the bob toggle moved the lens");
        assert_eq!(
            no_bob.eye_drop, all.eye_drop,
            "the bob toggle moved the landing dip"
        );

        let no_dip = sequence(FeelSettings {
            landing_dip: false,
            ..FeelSettings::ALL
        });
        assert_eq!(no_dip.eye_drop, 0.0, "the dip toggle did not remove the dip");
        assert_eq!(no_dip.pitch, all.pitch, "the dip toggle moved the bob");
    }

    #[test]
    fn the_same_ticks_always_produce_the_same_offsets() {
        // The capture guarantee, in the small: feel is a function of sim state
        // and the tick count alone, so two runs of the same journal agree bit
        // for bit. Anything easing on frame time would fail this the moment
        // the two runs disagreed about how long a frame took.
        let play = || {
            let mut feel = Feel::new(FeelSettings::ALL);
            let mut samples = Vec::new();
            for tick in 0..240 {
                let (stance, velocity) = match tick % 60 {
                    0..=19 => (Stance::Sprinting, Vec3::new(1.0, 0.0, -6.5)),
                    20..=29 => (
                        Stance::Airborne { coyote: 0 },
                        Vec3::new(1.0, -3.0 - tick as f32 * 0.1, -6.0),
                    ),
                    30..=44 => (Stance::Sliding { ticks: 5 }, Vec3::new(2.0, 0.0, -8.0)),
                    _ => (Stance::Grounded, Vec3::new(0.5, 0.0, -4.3)),
                };
                feel.advance(stance, velocity, tick as f32 * 0.01, MOVE_TICK);
                samples.push(feel.offsets());
            }
            samples
        };

        let first = play();
        let second = play();
        assert_eq!(first, second, "the same ticks produced different offsets");
        // And it is not vacuously constant.
        assert!(
            first.iter().any(|o| *o != FeelOffsets::default()),
            "the run produced no offsets at all"
        );
    }

    #[test]
    fn approach_arrives_and_stops() {
        assert_eq!(approach(0.0, 1.0, 0.25), 0.25);
        assert_eq!(approach(0.9, 1.0, 0.25), 1.0, "overshot its target");
        assert_eq!(approach(-0.9, -1.0, 0.25), -1.0, "overshot downward");
        assert_eq!(approach(1.0, 1.0, 0.25), 1.0, "moved when already there");
    }
}
