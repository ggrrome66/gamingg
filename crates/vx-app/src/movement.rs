//! Player movement: the command, the stance machine, stamina and weight.
//!
//! Movement is a **command consumed on the fixed tick**, not a function of the
//! frame. Held keys become one `MoveCommand` per frame; the simulation eats
//! exactly one per tick at `TICKS_PER_SECOND`. Rendering interpolates between
//! the last two tick snapshots. The reason is determinism: a recorded command
//! sequence must replay to the same positions bit-for-bit, and anything keyed
//! to frame `dt` makes position a property of the machine it ran on.
//!
//! Two consequences shape the file:
//!
//! - **Look angles are quantised into the command** and the simulation reads
//!   only the quantised value. The camera itself keeps turning per-frame —
//!   that is cosmetic and touches nothing a replay measures.
//! - **No transcendentals in the tick path.** Yaw resolves through a table
//!   built once; the only rounding-sensitive op left is `sqrt`, which IEEE 754
//!   requires to be correctly rounded. (The table is built with `sin`/`cos`
//!   at startup, so replay is bit-exact per platform/libm — cross-platform
//!   identity would need the table baked into source, noted in the README.)
//!
//! The collision itself stays in `vx_world::Body`, which knows nothing about
//! stamina, stances or cargo. Everything fictional lives here.

use glam::{Vec2, Vec3};
use vx_core::BlockPos;
use vx_render::Camera;
use vx_world::{Body, World};

/// Everything tunable, in one place, so tuning is a diff to one table.
pub mod tune {
    /// Seconds per simulation tick (20 Hz, matching the world tick).
    pub const TICK_DT: f32 = 1.0 / vx_world::TICKS_PER_SECOND as f32;

    // Speeds, metres per second.
    pub const WALK: f32 = 4.3;
    pub const SPRINT: f32 = 6.5;
    pub const CROUCH: f32 = 1.9;
    pub const PRONE: f32 = 0.7;
    pub const SWIM: f32 = 2.1;

    // How quickly horizontal velocity converges on intent, per second.
    pub const ACCEL_GROUND: f32 = 10.0;
    pub const ACCEL_AIR: f32 = 2.5;

    /// Slide decay per second — the whole difference between sliding and
    /// crouching. Ordinary ground friction is `ACCEL_GROUND` converging on a
    /// slow target; a slide converges on nothing and barely decays.
    pub const FRICTION_SLIDE: f32 = 1.6;
    /// Weak steering while sliding, same authority as air control.
    pub const SLIDE_STEER: f32 = 2.5;

    pub const GRAVITY: f32 = 30.0;
    /// ~1.25-block apex: a one-block hop clears with margin.
    pub const JUMP_IMPULSE: f32 = 8.4;

    pub const SLIDE_ENTRY: f32 = 5.0;
    pub const SLIDE_BOOST: f32 = 1.4;
    pub const SLIDE_CAP: f32 = 9.0;
    pub const SLIDE_EXIT: f32 = 3.0;

    pub const COYOTE_TICKS: u8 = 2;
    pub const JUMP_BUFFER_TICKS: u8 = 3;

    pub const VAULT_TICKS: u8 = 6;
    pub const MANTLE_TICKS: u8 = 12;

    pub const STAM_MAX: f32 = 100.0;
    /// Per second, multiplied by (1 + fullness) so a heavy load drains faster.
    pub const STAM_SPRINT: f32 = 8.0;
    pub const STAM_SLIDE: f32 = 12.0;
    pub const STAM_MANTLE: f32 = 15.0;
    /// Vaulting a single block is free: it replaces a jump you could make
    /// anyway, and taxing it would just make players jump instead.
    pub const STAM_VAULT: f32 = 0.0;
    pub const STAM_REGEN: f32 = 20.0;
    pub const REGEN_DELAY_TICKS: u16 = 30; // 1.5 s

    /// Speed multiplier runs 1.0 empty down to this floor fully loaded.
    pub const MASS_FLOOR: f32 = 0.55;

    // Box heights per stance, metres.
    pub const HEIGHT_STAND: f32 = 1.8;
    pub const HEIGHT_CROUCH: f32 = 1.2;
    pub const HEIGHT_SLIDE: f32 = 1.0;
    pub const HEIGHT_PRONE: f32 = 0.6;
}

/// Command bit assignments. Unknown bits are ignored on consumption.
pub mod bits {
    pub const FWD: u16 = 1 << 0;
    pub const BACK: u16 = 1 << 1;
    pub const LEFT: u16 = 1 << 2;
    pub const RIGHT: u16 = 1 << 3;
    pub const JUMP: u16 = 1 << 4;
    pub const SPRINT: u16 = 1 << 5;
    pub const CROUCH: u16 = 1 << 6;
    pub const PRONE: u16 = 1 << 7;
}

/// Steps in a full yaw turn. 4096 is 0.09° — far below perception, small
/// enough that the table is 32 KiB.
const YAW_STEPS: usize = 4096;

/// One frame of intent. The simulation consumes exactly one per tick.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MoveCommand {
    pub bits: u16,
    /// Quantised yaw, `0..YAW_STEPS` over a full turn.
    pub yaw_q: i16,
    /// Quantised pitch. Recorded for replay completeness; the walker does not
    /// steer by pitch.
    pub pitch_q: i16,
}

impl MoveCommand {
    pub fn has(self, bit: u16) -> bool {
        self.bits & bit != 0
    }

    /// Build this frame's command from held keys and the camera's yaw.
    pub fn sample(input: &vx_platform::InputState, camera: &Camera) -> MoveCommand {
        use winit::keyboard::KeyCode;
        let mut command_bits = 0u16;
        let mut set = |key: KeyCode, bit: u16| {
            if input.is_down(key) {
                command_bits |= bit;
            }
        };
        set(KeyCode::KeyW, bits::FWD);
        set(KeyCode::KeyS, bits::BACK);
        set(KeyCode::KeyA, bits::LEFT);
        set(KeyCode::KeyD, bits::RIGHT);
        set(KeyCode::Space, bits::JUMP);
        set(KeyCode::ControlLeft, bits::SPRINT);
        set(KeyCode::KeyC, bits::CROUCH);
        set(KeyCode::KeyZ, bits::PRONE);

        MoveCommand {
            bits: command_bits,
            yaw_q: quantise_yaw(camera.yaw),
            pitch_q: quantise_pitch(camera.pitch),
        }
    }
}

/// Yaw in radians to a table index. Wraps, so unbounded camera yaw is fine.
pub fn quantise_yaw(yaw: f32) -> i16 {
    let turns = yaw.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
    ((turns * YAW_STEPS as f32) as i32 & (YAW_STEPS as i32 - 1)) as i16
}

/// Pitch to a signed step count. Pitch is clamped by the camera already.
pub fn quantise_pitch(pitch: f32) -> i16 {
    (pitch / std::f32::consts::FRAC_PI_2 * 2047.0).clamp(-2047.0, 2047.0) as i16
}

/// (sin, cos) of a quantised yaw, from the startup-built table. This is the
/// only route from an angle to a direction inside the tick path.
fn yaw_sincos(yaw_q: i16) -> (f32, f32) {
    use std::sync::OnceLock;
    static TABLE: OnceLock<Vec<[f32; 2]>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        (0..YAW_STEPS)
            .map(|i| {
                let angle = i as f32 / YAW_STEPS as f32 * std::f32::consts::TAU;
                [angle.sin(), angle.cos()]
            })
            .collect()
    });
    let entry = table[(yaw_q as usize) & (YAW_STEPS - 1)];
    (entry[0], entry[1])
}

/// 1.0 with an empty inventory, floored at `MASS_FLOOR` fully loaded.
pub fn mass_multiplier(fullness: f32) -> f32 {
    (1.0 - (1.0 - tune::MASS_FLOOR) * fullness.clamp(0.0, 1.0)).max(tune::MASS_FLOOR)
}

/// What the player's body is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stance {
    Grounded,
    Sprinting,
    Sliding { ticks_held: u16 },
    Crouched,
    Prone,
    Airborne { coyote: u8 },
    /// Physics suspended; the box interpolates toward `to` over `total` ticks.
    Mantling { t: u8, total: u8, to: BlockPos },
    Swimming,
}

impl Stance {
    /// Eye height above the feet, in centimetres. Integer so the value that
    /// reaches the camera interpolates identically every run.
    pub fn eye_cm(self) -> u16 {
        match self {
            Stance::Grounded | Stance::Sprinting | Stance::Airborne { .. } => 165,
            Stance::Crouched => 110,
            Stance::Sliding { .. } => 70,
            Stance::Prone => 35,
            Stance::Mantling { .. } => 140,
            Stance::Swimming => 150,
        }
    }

    /// Collision box height for the stance.
    pub fn height(self) -> f32 {
        match self {
            Stance::Grounded | Stance::Sprinting | Stance::Airborne { .. } | Stance::Swimming => {
                tune::HEIGHT_STAND
            }
            Stance::Crouched | Stance::Mantling { .. } => tune::HEIGHT_CROUCH,
            Stance::Sliding { .. } => tune::HEIGHT_SLIDE,
            Stance::Prone => tune::HEIGHT_PRONE,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Stance::Grounded => "STAND",
            Stance::Sprinting => "SPRINT",
            Stance::Sliding { .. } => "SLIDE",
            Stance::Crouched => "CROUCH",
            Stance::Prone => "PRONE",
            Stance::Airborne { .. } => "AIR",
            Stance::Mantling { .. } => "MANTLE",
            Stance::Swimming => "SWIM",
        }
    }
}

/// One tick's snapshot, for render interpolation.
#[derive(Debug, Clone, Copy)]
pub struct Snapshot {
    pub position: Vec3,
    pub eye_cm: u16,
}

/// The player's moving self: body, stance, stamina, and the two snapshots the
/// renderer lerps between.
#[derive(Debug, Clone)]
pub struct Movement {
    pub body: Body,
    pub stance: Stance,
    pub stamina: f32,
    /// Ticks since stamina was last spent; regen starts past the delay.
    idle_ticks: u16,
    jump_buffer: u8,
    /// Where a mantle started, for the interpolation and the abort path.
    mantle_from: Vec3,
    last_bits: u16,
    prev: Snapshot,
    curr: Snapshot,
}

impl Movement {
    pub fn new(position: Vec3) -> Self {
        let body = Body::player(position);
        let snap = Snapshot {
            position,
            eye_cm: Stance::Grounded.eye_cm(),
        };
        Movement {
            body,
            stance: Stance::Airborne { coyote: 0 },
            stamina: tune::STAM_MAX,
            idle_ticks: 0,
            jump_buffer: 0,
            mantle_from: position,
            last_bits: 0,
            prev: snap,
            curr: snap,
        }
    }

    /// Teleport-class reposition: spawn, restore, fly-to-walk. Snapshots
    /// collapse so the camera does not lerp across the jump.
    pub fn reset_at(&mut self, position: Vec3) {
        self.body.position = position;
        self.body.velocity = Vec3::ZERO;
        self.body.height = tune::HEIGHT_STAND;
        self.stance = Stance::Airborne { coyote: 0 };
        let snap = Snapshot {
            position,
            eye_cm: self.stance.eye_cm(),
        };
        self.prev = snap;
        self.curr = snap;
    }

    /// Camera position for rendering: tick snapshots lerped by `alpha`.
    pub fn camera_position(&self, alpha: f32) -> Vec3 {
        // clamp() propagates NaN; a broken clock must not shake the camera.
        let alpha = if alpha.is_finite() { alpha.clamp(0.0, 1.0) } else { 0.0 };
        let position = self.prev.position.lerp(self.curr.position, alpha);
        let eye = (self.prev.eye_cm as f32 + (self.curr.eye_cm as f32 - self.prev.eye_cm as f32) * alpha)
            / 100.0;
        position + Vec3::new(0.0, eye, 0.0)
    }

    /// Advance one tick. `fullness` is the inventory's 0..=1 load fraction.
    pub fn tick(&mut self, cmd: MoveCommand, world: &World, fullness: f32) {
        self.prev = self.curr;

        let solid = |pos: BlockPos| !world.is_loaded(pos.chunk()) || world.is_solid(pos);
        let water = world.generator().blocks().water;
        let pressed_jump = cmd.has(bits::JUMP) && self.last_bits & bits::JUMP == 0;
        let pressed_crouch = cmd.has(bits::CROUCH) && self.last_bits & bits::CROUCH == 0;
        self.last_bits = cmd.bits;

        let mass = mass_multiplier(fullness);
        let drain_scale = 1.0 + fullness.clamp(0.0, 1.0);

        // Wish direction from the quantised yaw only.
        let (sin_yaw, cos_yaw) = yaw_sincos(cmd.yaw_q);
        let forward = Vec2::new(sin_yaw, -cos_yaw);
        let right = Vec2::new(cos_yaw, sin_yaw);
        let mut wish = Vec2::ZERO;
        if cmd.has(bits::FWD) {
            wish += forward;
        }
        if cmd.has(bits::BACK) {
            wish -= forward;
        }
        if cmd.has(bits::RIGHT) {
            wish += right;
        }
        if cmd.has(bits::LEFT) {
            wish -= right;
        }
        if wish.length_squared() > 1.0 {
            wish = wish.normalize();
        }

        // Mantling suspends everything else.
        if let Stance::Mantling { t, total, to } = self.stance {
            self.tick_mantle(t, total, to, &solid);
            self.finish_tick(cmd, false);
            return;
        }

        // Water overrides the ground states.
        let feet = BlockPos::new(
            self.body.position.x.floor() as i32,
            (self.body.position.y + 0.4).floor() as i32,
            self.body.position.z.floor() as i32,
        );
        let in_water = world.block(feet) == water;
        if in_water && !matches!(self.stance, Stance::Swimming) {
            self.set_stance(Stance::Swimming, &solid);
        } else if !in_water && matches!(self.stance, Stance::Swimming) {
            self.set_stance(Stance::Airborne { coyote: 0 }, &solid);
        }

        let mut spent = false;

        match self.stance {
            Stance::Swimming => {
                let target = wish * tune::SWIM * mass;
                self.accelerate(target, tune::ACCEL_GROUND);
                // Buoyancy: slow sink, Space paddles up.
                self.body.velocity.y -= tune::GRAVITY * 0.25 * tune::TICK_DT;
                self.body.velocity.y = self.body.velocity.y.max(-3.0);
                if cmd.has(bits::JUMP) {
                    self.body.velocity.y = (self.body.velocity.y + 24.0 * tune::TICK_DT).min(3.5);
                }
                self.body.step(tune::TICK_DT, solid);
            }

            Stance::Sliding { ticks_held } => {
                // Slide-jump first, before any decay touches the velocity:
                // horizontal speed is preserved exactly, which is the whole
                // reason the verb exists.
                if pressed_jump && self.body.on_ground && self.try_stand(&solid) {
                    self.body.velocity.y = tune::JUMP_IMPULSE;
                    self.stance = Stance::Airborne { coyote: 0 };
                    self.body.step(tune::TICK_DT, solid);
                    self.stamina = self.stamina.clamp(0.0, tune::STAM_MAX);
                    self.finish_tick(cmd, false);
                    return;
                }

                // Slide: no target to converge on, just weak decay and steer.
                // Friction only bites while grounded - a slide that carries
                // off a bench edge free-falls without losing its speed.
                if self.body.on_ground {
                    let decay = (1.0 - tune::FRICTION_SLIDE * tune::TICK_DT).max(0.0);
                    self.body.velocity.x *= decay;
                    self.body.velocity.z *= decay;
                }
                let steer = wish * tune::SLIDE_STEER * tune::TICK_DT;
                self.body.velocity.x += steer.x;
                self.body.velocity.z += steer.y;

                let falling = self.body.velocity.y - tune::GRAVITY * tune::TICK_DT;
                self.body.velocity.y = falling;
                let result = self.body.step(tune::TICK_DT, solid);

                // Landing a drop mid-slide converts some of the impact into
                // more slide. There are no slopes in a voxel world for
                // gravity to project onto; this is the honest equivalent,
                // and it is why a slide down benched terrain runs away while
                // a flat one dies.
                if result.on_ground && falling < -3.0 {
                    let speed = horizontal_speed(&self.body);
                    if speed > 0.1 {
                        let boosted = (speed + -falling * 0.35).min(tune::SLIDE_CAP);
                        let scale = boosted / speed;
                        self.body.velocity.x *= scale;
                        self.body.velocity.z *= scale;
                    }
                }

                let speed = horizontal_speed(&self.body);
                if !cmd.has(bits::CROUCH) && self.try_stand(&solid) {
                    self.stance = if speed > tune::WALK { Stance::Sprinting } else { Stance::Grounded };
                } else if speed < tune::SLIDE_EXIT {
                    self.set_stance(Stance::Crouched, &solid);
                } else {
                    self.stance = Stance::Sliding {
                        ticks_held: ticks_held.saturating_add(1),
                    };
                }
            }

            Stance::Airborne { coyote } => {
                // Air control steers only while there is input: converging on
                // a zero wish would be air drag, and air drag is what makes a
                // slide-jump pointless.
                if wish != Vec2::ZERO {
                    self.accelerate(wish * tune::WALK * mass, tune::ACCEL_AIR);
                }
                self.body.velocity.y -= tune::GRAVITY * tune::TICK_DT;

                // Coyote jump: forgiveness after walking off an edge.
                if pressed_jump && coyote > 0 {
                    self.body.velocity.y = tune::JUMP_IMPULSE;
                    self.stance = Stance::Airborne { coyote: 0 };
                } else if pressed_jump {
                    // Try a ledge first; otherwise bank the press.
                    if !self.try_ledge(wish, &solid) {
                        self.jump_buffer = tune::JUMP_BUFFER_TICKS;
                    }
                }

                if !matches!(self.stance, Stance::Mantling { .. }) {
                    let result = self.body.step(tune::TICK_DT, solid);
                    if result.on_ground {
                        if self.jump_buffer > 0 {
                            // Buffered jump fires the tick you land.
                            self.jump_buffer = 0;
                            self.body.velocity.y = tune::JUMP_IMPULSE;
                            self.stance = Stance::Airborne { coyote: 0 };
                        } else {
                            self.stance = Stance::Grounded;
                            self.resolve_ground_stance(cmd, mass, &solid);
                        }
                    } else if let Stance::Airborne { coyote } = &mut self.stance {
                        *coyote = coyote.saturating_sub(1);
                    }
                }
            }

            Stance::Grounded | Stance::Sprinting | Stance::Crouched | Stance::Prone => {
                // Slide entry: crouch pressed at sprint speed, with stamina.
                if pressed_crouch
                    && matches!(self.stance, Stance::Sprinting)
                    && horizontal_speed(&self.body) >= tune::SLIDE_ENTRY
                    && self.stamina >= tune::STAM_SLIDE
                {
                    self.stamina -= tune::STAM_SLIDE;
                    spent = true;
                    let speed = horizontal_speed(&self.body);
                    let boosted = (speed * tune::SLIDE_BOOST).min(tune::SLIDE_CAP);
                    if speed > 0.0 {
                        let scale = boosted / speed;
                        self.body.velocity.x *= scale;
                        self.body.velocity.z *= scale;
                    }
                    self.set_stance(Stance::Sliding { ticks_held: 0 }, &solid);
                } else {
                    self.resolve_ground_stance(cmd, mass, &solid);
                }

                if let Stance::Sliding { .. } = self.stance {
                    // Entered the slide this tick; its physics starts next.
                    self.body.velocity.y -= tune::GRAVITY * tune::TICK_DT;
                    self.body.step(tune::TICK_DT, solid);
                } else {
                    let target_speed = match self.stance {
                        Stance::Sprinting => tune::SPRINT,
                        Stance::Crouched => tune::CROUCH,
                        Stance::Prone => tune::PRONE,
                        _ => tune::WALK,
                    } * mass;
                    self.accelerate(wish * target_speed, tune::ACCEL_GROUND);

                    if matches!(self.stance, Stance::Sprinting) && wish != Vec2::ZERO {
                        self.stamina -= tune::STAM_SPRINT * drain_scale * tune::TICK_DT;
                        spent = true;
                    }

                    // Jumps, vaults and mantles, from the ground. Held
                    // Space hops repeatedly, which is also what re-triggers
                    // the ledge check while pushing against a wall.
                    if (cmd.has(bits::JUMP) || self.jump_buffer > 0) && self.body.on_ground {
                        self.jump_buffer = 0;
                        if !self.try_ledge(wish, &solid) && self.try_stand(&solid) {
                            self.body.velocity.y = tune::JUMP_IMPULSE;
                            self.stance = Stance::Airborne { coyote: 0 };
                        }
                    }

                    if !matches!(self.stance, Stance::Mantling { .. }) {
                        self.body.velocity.y -= tune::GRAVITY * tune::TICK_DT;
                        let result = self.body.step(tune::TICK_DT, solid);
                        if !result.on_ground
                            && !matches!(self.stance, Stance::Airborne { .. })
                        {
                            self.stance = Stance::Airborne {
                                coyote: tune::COYOTE_TICKS,
                            };
                        }
                    }
                }
            }

            Stance::Mantling { .. } => unreachable!("handled above"),
        }

        self.stamina = self.stamina.clamp(0.0, tune::STAM_MAX);
        self.finish_tick(cmd, spent);
    }

    /// Stance for the grounded family from held bits, respecting clearances.
    fn resolve_ground_stance(
        &mut self,
        cmd: MoveCommand,
        _mass: f32,
        solid: &impl Fn(BlockPos) -> bool,
    ) {
        let desired = if cmd.has(bits::PRONE) {
            Stance::Prone
        } else if cmd.has(bits::CROUCH) {
            Stance::Crouched
        } else if cmd.has(bits::SPRINT) && cmd.has(bits::FWD) && self.stamina > 0.0 {
            Stance::Sprinting
        } else {
            Stance::Grounded
        };
        if desired != self.stance {
            self.set_stance(desired, solid);
        }
    }

    /// Move to a stance, growing the box only if there is room. A refused
    /// grow keeps the current stance — you stay prone under the ceiling.
    fn set_stance(&mut self, desired: Stance, solid: &impl Fn(BlockPos) -> bool) {
        if self.body.try_resize(desired.height(), solid) {
            self.stance = desired;
        }
    }

    /// Try to stand at full height; true when the box made it.
    fn try_stand(&mut self, solid: &impl Fn(BlockPos) -> bool) -> bool {
        self.body.try_resize(tune::HEIGHT_STAND, solid)
    }

    /// Converge horizontal velocity on `target` at `rate` per second.
    fn accelerate(&mut self, target: Vec2, rate: f32) {
        let blend = (rate * tune::TICK_DT).min(1.0);
        self.body.velocity.x += (target.x - self.body.velocity.x) * blend;
        self.body.velocity.z += (target.y - self.body.velocity.z) * blend;
    }

    /// Look for a ledge ahead and start a vault or mantle. True if started.
    fn try_ledge(&mut self, wish: Vec2, solid: &impl Fn(BlockPos) -> bool) -> bool {
        if wish == Vec2::ZERO {
            return false;
        }
        let Some((to, height)) = classify_ledge(self.body.position, self.body.half_width, wish, solid)
        else {
            return false;
        };

        let (total, cost) = match height {
            1 => (tune::VAULT_TICKS, tune::STAM_VAULT),
            _ => (tune::MANTLE_TICKS, tune::STAM_MANTLE),
        };
        if self.stamina < cost {
            return false;
        }
        self.stamina -= cost;
        self.idle_ticks = 0;

        self.mantle_from = self.body.position;
        self.body.velocity = Vec3::ZERO;
        // Compact for the climb; shrinking always succeeds.
        let _ = self.body.try_resize(tune::HEIGHT_CROUCH, solid);
        self.stance = Stance::Mantling { t: 0, total, to };
        true
    }

    /// One tick of a mantle: advance the interpolation; at the end, land only
    /// if the destination is still real.
    fn tick_mantle(&mut self, t: u8, total: u8, to: BlockPos, solid: &impl Fn(BlockPos) -> bool) {
        let t = t.saturating_add(1);
        let to_feet = Vec3::new(to.x as f32 + 0.5, to.y as f32, to.z as f32 + 0.5);

        if t < total {
            // Rise first, then translate: integer tick fractions, so the
            // curve replays identically.
            let f = t as f32 / total as f32;
            let vertical = (f * 1.5).min(1.0);
            let horizontal = ((f - 0.33) * 1.5).clamp(0.0, 1.0);
            self.body.position.y = self.mantle_from.y + (to_feet.y - self.mantle_from.y) * vertical;
            self.body.position.x =
                self.mantle_from.x + (to_feet.x - self.mantle_from.x) * horizontal;
            self.body.position.z =
                self.mantle_from.z + (to_feet.z - self.mantle_from.z) * horizontal;
            self.stance = Stance::Mantling { t, total, to };
            return;
        }

        // Completion: the world may have changed mid-climb — sand can fall
        // into the destination. Re-validate before landing; on failure the
        // body returns to where it started rather than embedding.
        self.body.position = to_feet;
        if self.try_stand(solid) && !solid(to) {
            self.stance = Stance::Grounded;
        } else {
            self.body.position = self.mantle_from;
            let _ = self.body.try_resize(tune::HEIGHT_STAND, solid);
            self.stance = Stance::Airborne { coyote: 0 };
        }
        self.body.velocity = Vec3::ZERO;
    }

    /// Bookkeeping shared by every tick exit: stamina regen and snapshots.
    fn finish_tick(&mut self, _cmd: MoveCommand, spent: bool) {
        if spent {
            self.idle_ticks = 0;
        } else {
            self.idle_ticks = self.idle_ticks.saturating_add(1);
            if self.idle_ticks > tune::REGEN_DELAY_TICKS {
                self.stamina = (self.stamina + tune::STAM_REGEN * tune::TICK_DT).min(tune::STAM_MAX);
            }
        }
        self.curr = Snapshot {
            position: self.body.position,
            eye_cm: self.stance.eye_cm(),
        };
    }
}

fn horizontal_speed(body: &Body) -> f32 {
    Vec2::new(body.velocity.x, body.velocity.z).length()
}

/// Classify the ledge ahead: `Some((standing position, height in blocks))`.
///
/// Integer scans, not raycasts: on unit voxels a ledge is one or two solid
/// blocks with clear air above, and the classification is exact. Unloaded
/// chunks read as solid through the caller's closure, so a mantle can never
/// carry the player into terrain that has not streamed in.
fn classify_ledge(
    position: Vec3,
    half_width: f32,
    wish: Vec2,
    solid: &impl Fn(BlockPos) -> bool,
) -> Option<(BlockPos, u8)> {
    let probe = position + Vec3::new(wish.x, 0.0, wish.y) * (half_width + 0.45);
    let front_x = probe.x.floor() as i32;
    let front_z = probe.z.floor() as i32;
    let feet_y = (position.y + 0.01).floor() as i32;

    let at = |dy: i32| solid(BlockPos::new(front_x, feet_y + dy, front_z));

    if at(0) && !at(1) && !at(2) {
        // One block: vault. Destination needs the two clear blocks it has.
        Some((BlockPos::new(front_x, feet_y + 1, front_z), 1))
    } else if at(0) && at(1) && !at(2) && !at(3) {
        // Two blocks: mantle.
        Some((BlockPos::new(front_x, feet_y + 2, front_z), 2))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::ChunkPos;

    /// A world with a big flat stone platform in the sky at y = 200, well
    /// clear of terrain, plus whatever the test builds on it.
    fn platform_world() -> World {
        let mut world = World::new(7);
        world.load_around(ChunkPos::new(1, 0), 3);
        let stone = world.registry().id_of("engine:stone").unwrap();
        for x in -10..70 {
            for z in -10..10 {
                world.set_block(BlockPos::new(x, 199, z), stone);
            }
        }
        world
    }

    fn stone(world: &World) -> vx_core::BlockId {
        world.registry().id_of("engine:stone").unwrap()
    }

    /// Stand a settled body on the platform at `(x, z)`.
    fn spawn(world: &World, x: f32, z: f32) -> Movement {
        let mut movement = Movement::new(Vec3::new(x, 200.0, z));
        // Settle onto the ground.
        for _ in 0..10 {
            movement.tick(MoveCommand::default(), world, 0.0);
        }
        movement
    }

    /// A command holding the given bits, facing +X.
    fn cmd(bits_held: u16) -> MoveCommand {
        MoveCommand {
            bits: bits_held,
            yaw_q: quantise_yaw(std::f32::consts::FRAC_PI_2), // forward = +X
            pitch_q: 0,
        }
    }

    #[test]
    fn two_runs_of_the_same_commands_are_bit_identical() {
        let script: Vec<MoveCommand> = (0..120)
            .map(|i| match i {
                0..=39 => cmd(bits::FWD),
                40..=79 => cmd(bits::FWD | bits::SPRINT),
                80 => cmd(bits::FWD | bits::SPRINT | bits::CROUCH),
                81..=99 => cmd(bits::CROUCH),
                _ => cmd(bits::FWD | bits::JUMP),
            })
            .collect();

        let run = || {
            let world = platform_world();
            let mut movement = spawn(&world, 0.5, 0.5);
            for command in &script {
                movement.tick(*command, &world, 0.25);
            }
            movement.body.position
        };

        let (a, b) = (run(), run());
        assert_eq!(a.x.to_bits(), b.x.to_bits(), "x diverged");
        assert_eq!(a.y.to_bits(), b.y.to_bits(), "y diverged");
        assert_eq!(a.z.to_bits(), b.z.to_bits(), "z diverged");
    }

    #[test]
    fn consumption_chunking_does_not_change_the_trace() {
        // The frame-rate independence property, structurally: whether ticks
        // are consumed one at a time or three at a time between renders makes
        // no difference, because rendering reads only snapshots.
        let script: Vec<MoveCommand> =
            (0..90).map(|i| if i % 7 == 0 { cmd(bits::FWD | bits::JUMP) } else { cmd(bits::FWD) }).collect();

        let world = platform_world();
        let mut one = spawn(&world, 0.5, 0.5);
        for command in &script {
            one.tick(*command, &world, 0.0);
            let _ = one.camera_position(0.4); // renders interleaved
        }

        let world2 = platform_world();
        let mut chunked = spawn(&world2, 0.5, 0.5);
        for group in script.chunks(3) {
            for command in group {
                chunked.tick(*command, &world2, 0.0);
            }
            let _ = chunked.camera_position(0.9);
        }

        assert_eq!(one.body.position, chunked.body.position);
    }

    #[test]
    fn sprint_outruns_walk_and_mass_slows_both() {
        let world = platform_world();
        let distance = |bits_held: u16, fullness: f32| {
            let mut movement = spawn(&world, 0.5, 0.5);
            let start = movement.body.position.x;
            for _ in 0..60 {
                movement.tick(cmd(bits_held), &world, fullness);
            }
            movement.body.position.x - start
        };

        let walk = distance(bits::FWD, 0.0);
        let sprint = distance(bits::FWD | bits::SPRINT, 0.0);
        let loaded_walk = distance(bits::FWD, 1.0);

        assert!(sprint > walk * 1.3, "sprint {sprint} vs walk {walk}");
        assert!(
            loaded_walk < walk * 0.65,
            "full load walked {loaded_walk} vs empty {walk}"
        );
        // The floor holds: even fully loaded you move.
        assert!(loaded_walk > walk * 0.4);
    }

    #[test]
    fn crouch_and_prone_are_progressively_slower_with_lower_eyes() {
        let world = platform_world();
        let outcome = |bits_held: u16| {
            let mut movement = spawn(&world, 0.5, 0.5);
            let start = movement.body.position.x;
            for _ in 0..60 {
                movement.tick(cmd(bits_held), &world, 0.0);
            }
            (movement.body.position.x - start, movement.stance.eye_cm())
        };

        let (walk, eye_walk) = outcome(bits::FWD);
        let (crouch, eye_crouch) = outcome(bits::FWD | bits::CROUCH);
        let (prone, eye_prone) = outcome(bits::FWD | bits::PRONE);

        assert!(walk > crouch && crouch > prone, "{walk} / {crouch} / {prone}");
        assert!(eye_walk > eye_crouch && eye_crouch > eye_prone);
    }

    #[test]
    fn slide_boosts_then_decays_to_a_crouch() {
        let world = platform_world();
        let mut movement = spawn(&world, 0.5, 0.5);

        // Sprint up to speed.
        for _ in 0..40 {
            movement.tick(cmd(bits::FWD | bits::SPRINT), &world, 0.0);
        }
        let sprint_speed = horizontal_speed(&movement.body);
        assert!(sprint_speed >= tune::SLIDE_ENTRY);

        // Crouch press enters the slide with a boost.
        movement.tick(cmd(bits::FWD | bits::SPRINT | bits::CROUCH), &world, 0.0);
        assert!(matches!(movement.stance, Stance::Sliding { .. }));
        assert!(horizontal_speed(&movement.body) > sprint_speed);
        assert!(horizontal_speed(&movement.body) <= tune::SLIDE_CAP + 0.01);

        // Hold crouch: the slide decays and ends in a crouch, not a stand.
        for _ in 0..200 {
            movement.tick(cmd(bits::CROUCH), &world, 0.0);
        }
        assert!(
            matches!(movement.stance, Stance::Crouched),
            "slide ended in {:?}",
            movement.stance
        );
    }

    #[test]
    fn slide_jump_preserves_horizontal_velocity() {
        let world = platform_world();
        let mut movement = spawn(&world, 0.5, 0.5);
        for _ in 0..40 {
            movement.tick(cmd(bits::FWD | bits::SPRINT), &world, 0.0);
        }
        movement.tick(cmd(bits::FWD | bits::SPRINT | bits::CROUCH), &world, 0.0);
        let sliding_speed = horizontal_speed(&movement.body);
        assert!(matches!(movement.stance, Stance::Sliding { .. }));

        // Release jump bit first so the next press is an edge.
        movement.tick(cmd(bits::CROUCH), &world, 0.0);
        let before_jump = horizontal_speed(&movement.body);
        movement.tick(cmd(bits::CROUCH | bits::JUMP), &world, 0.0);

        assert!(matches!(movement.stance, Stance::Airborne { .. }));
        let after = horizontal_speed(&movement.body);
        assert!(
            after > before_jump * 0.95,
            "slide-jump lost the speed: {sliding_speed} -> {after}"
        );
        assert!(movement.body.velocity.y > 0.0, "did not actually jump");
    }

    #[test]
    fn slide_needs_sprint_speed_and_stamina() {
        let world = platform_world();

        // Walking speed: crouch is just a crouch.
        let mut movement = spawn(&world, 0.5, 0.5);
        for _ in 0..40 {
            movement.tick(cmd(bits::FWD), &world, 0.0);
        }
        movement.tick(cmd(bits::FWD | bits::CROUCH), &world, 0.0);
        assert!(matches!(movement.stance, Stance::Crouched));

        // No stamina: no slide either.
        let mut movement = spawn(&world, 0.5, 0.5);
        movement.stamina = tune::STAM_SLIDE - 1.0;
        for _ in 0..40 {
            movement.tick(cmd(bits::FWD | bits::SPRINT), &world, 0.0);
            movement.stamina = tune::STAM_SLIDE - 1.0; // hold it low
        }
        movement.tick(cmd(bits::FWD | bits::SPRINT | bits::CROUCH), &world, 0.0);
        assert!(
            !matches!(movement.stance, Stance::Sliding { .. }),
            "slid without the stamina to pay for it"
        );
    }

    #[test]
    fn slides_run_longer_downhill_and_die_uphill() {
        // No slopes exist in a voxel world, so there is no normal to project
        // gravity onto. The asymmetry is emergent: downhill, each bench drop
        // feeds the slide through the landing conversion; uphill, block
        // faces kill the speed before a slide can even start.
        let mut world = platform_world();
        let rock = stone(&world);

        // A staircase in the z -8..-2 lane only: from x = 20, drop one block
        // every two, for fifteen steps. z >= 0 stays flat for the control.
        for i in 0..15 {
            for dx in 0..2 {
                for z in -8..-2 {
                    let x = 16 + i * 2 + dx;
                    world.set_block(BlockPos::new(x, 198 - i, z), rock);
                    world.set_block(BlockPos::new(x, 199, z), vx_core::BlockId::AIR);
                }
            }
        }

        let slide_distance = |start: Vec3, yaw: f32| {
            let mut movement = Movement::new(start);
            for _ in 0..10 {
                movement.tick(MoveCommand::default(), &world, 0.0);
            }
            let facing = |bits_held| MoveCommand {
                bits: bits_held,
                yaw_q: quantise_yaw(yaw),
                pitch_q: 0,
            };
            // A short sprint runway that stays on flat ground.
            for _ in 0..16 {
                movement.tick(facing(bits::FWD | bits::SPRINT), &world, 0.0);
            }
            let begin = movement.body.position;
            movement.tick(facing(bits::FWD | bits::SPRINT | bits::CROUCH), &world, 0.0);
            if !matches!(movement.stance, Stance::Sliding { .. }) {
                return 0.0;
            }
            for _ in 0..300 {
                movement.tick(facing(bits::CROUCH), &world, 0.0);
                if !matches!(movement.stance, Stance::Sliding { .. }) {
                    break;
                }
            }
            (movement.body.position - begin).length()
        };

        let east = std::f32::consts::FRAC_PI_2;
        // Flat control, in the untouched z = 4 lane.
        let flat = slide_distance(Vec3::new(8.5, 200.0, 4.5), east);
        // Downhill: same runway, but in the staircase lane.
        let downhill = slide_distance(Vec3::new(8.5, 200.0, -5.5), east);
        // Uphill: at the stair bottom facing back up the risers.
        let uphill = slide_distance(Vec3::new(43.0, 186.0, -5.5), -east);

        assert!(flat > 3.0, "the flat control slide only ran {flat}");
        assert!(
            downhill > flat * 1.15,
            "downhill {downhill} did not outrun flat {flat}"
        );
        assert!(uphill < flat * 0.6, "uphill {uphill} vs flat {flat}");
    }


    #[test]
    fn vault_takes_one_block_and_mantle_takes_two_but_not_three() {
        let mut world = platform_world();
        let rock = stone(&world);

        // Walls across +X at x=10: heights 1, 2 and 3 in separate z lanes.
        for (z, height) in [(-6i32, 1i32), (0, 2), (6, 3)] {
            for dz in -1..2 {
                for h in 0..height {
                    world.set_block(BlockPos::new(10, 200 + h, z + dz), rock);
                }
            }
        }

        // Track how far and how high the runner ever got: after a clean
        // climb they keep running and drop off the far side of the wall.
        let attempt = |z: f32| {
            let mut movement = spawn(&world, 7.5, z);
            let mut peak_x = movement.body.position.x;
            let mut peak_y = movement.body.position.y;
            for _ in 0..120 {
                movement.tick(cmd(bits::FWD | bits::JUMP), &world, 0.0);
                peak_x = peak_x.max(movement.body.position.x);
                peak_y = peak_y.max(movement.body.position.y);
            }
            (peak_x, peak_y)
        };

        let (vault_x, _) = attempt(-6.0);
        assert!(vault_x > 10.0, "did not vault the 1-block wall: x={vault_x}");

        let (mantle_x, mantle_y) = attempt(0.0);
        assert!(
            mantle_x > 10.0 && mantle_y >= 201.9,
            "did not mantle the 2-block wall: peak ({mantle_x}, {mantle_y})"
        );

        let (stop_x, stop_y) = attempt(6.0);
        assert!(
            stop_x < 10.0 && stop_y < 202.0,
            "climbed a 3-block wall: peak ({stop_x}, {stop_y})"
        );
    }

    #[test]
    fn a_block_falling_into_the_mantle_destination_aborts_the_climb() {
        let mut world = platform_world();
        let rock = stone(&world);
        for dz in -1..2 {
            for h in 0..2 {
                world.set_block(BlockPos::new(10, 200 + h, dz), rock);
            }
        }

        let mut movement = spawn(&world, 8.5, 0.5);
        // Walk into the wall and start the mantle.
        for _ in 0..60 {
            movement.tick(cmd(bits::FWD | bits::JUMP), &world, 0.0);
            if matches!(movement.stance, Stance::Mantling { .. }) {
                break;
            }
        }
        let Stance::Mantling { to, .. } = movement.stance else {
            panic!("never started the mantle");
        };
        let from = movement.mantle_from;

        // Sand-in-the-destination, as a direct world edit mid-climb.
        world.set_block(to, rock);
        world.set_block(to.offset([0, 1, 0]), rock);

        for _ in 0..40 {
            movement.tick(cmd(bits::FWD | bits::JUMP), &world, 0.0);
            if !matches!(movement.stance, Stance::Mantling { .. }) {
                break;
            }
        }

        assert!(
            (movement.body.position - from).length() < 1.0,
            "the climb ended {:?}, not back at {from:?}",
            movement.body.position
        );
        // And wherever we are, we are not inside rock.
        let feet = movement.body.position;
        assert!(!world.is_solid(BlockPos::new(
            feet.x.floor() as i32,
            (feet.y + 0.3).floor() as i32,
            feet.z.floor() as i32
        )));
    }

    #[test]
    fn prone_fits_a_one_block_tunnel_and_cannot_stand_inside_it() {
        let mut world = platform_world();
        let rock = stone(&world);
        // A slab roof one block above the platform over x 12..24.
        for x in 12..24 {
            for z in -4..4 {
                world.set_block(BlockPos::new(x, 201, z), rock);
            }
        }

        let mut movement = spawn(&world, 9.5, 0.5);
        // Go prone, crawl in.
        for _ in 0..160 {
            movement.tick(cmd(bits::FWD | bits::PRONE), &world, 0.0);
        }
        assert!(
            movement.body.position.x > 13.0,
            "never got into the tunnel: x={}",
            movement.body.position.x
        );
        assert!(matches!(movement.stance, Stance::Prone));

        // Release prone inside: the ceiling keeps you prone.
        movement.tick(cmd(0), &world, 0.0);
        assert!(
            matches!(movement.stance, Stance::Prone),
            "stood up inside a one-block tunnel: {:?}",
            movement.stance
        );

        // Crawl out the far side and stand freely.
        for _ in 0..500 {
            movement.tick(cmd(bits::FWD | bits::PRONE), &world, 0.0);
        }
        assert!(movement.body.position.x > 24.5, "x={}", movement.body.position.x);
        movement.tick(cmd(0), &world, 0.0);
        assert!(matches!(movement.stance, Stance::Grounded));
    }

    #[test]
    fn coyote_forgives_two_ticks_and_no_more() {
        let world = platform_world();
        // The platform ends at x = 69 (built to x < 70). Walk off the edge.
        let run_off = |ticks_late: u8| {
            let mut movement = spawn(&world, 68.0, 0.5);
            // Walk east until airborne.
            let mut airborne_at = None;
            for i in 0..200 {
                movement.tick(cmd(bits::FWD), &world, 0.0);
                if matches!(movement.stance, Stance::Airborne { .. }) {
                    airborne_at = Some(i);
                    break;
                }
            }
            airborne_at.expect("never left the platform");
            for _ in 0..ticks_late {
                movement.tick(cmd(bits::FWD), &world, 0.0);
            }
            movement.tick(cmd(bits::FWD | bits::JUMP), &world, 0.0);
            movement.body.velocity.y > 1.0
        };

        assert!(run_off(0), "coyote jump on the first airborne tick failed");
        assert!(!run_off(tune::COYOTE_TICKS + 1), "jumped well past the window");
    }

    #[test]
    fn a_jump_pressed_early_fires_on_landing() {
        let world = platform_world();
        let mut movement = spawn(&world, 0.5, 0.5);
        // Jump, and press jump again on the way down (edge: release first).
        movement.tick(cmd(bits::JUMP), &world, 0.0);
        assert!(matches!(movement.stance, Stance::Airborne { .. }));
        let mut fired_second = false;
        let mut released = false;
        for _ in 0..60 {
            let held = if !released {
                released = true;
                cmd(0)
            } else if movement.body.velocity.y < -2.0 && !fired_second {
                fired_second = true;
                cmd(bits::JUMP)
            } else {
                cmd(0)
            };
            movement.tick(held, &world, 0.0);
            if fired_second && movement.body.velocity.y > 1.0 {
                // Buffered jump fired on landing.
                return;
            }
        }
        panic!("the buffered jump never fired");
    }

    #[test]
    fn stamina_drains_regens_and_never_hard_stops() {
        let world = platform_world();
        let mut movement = spawn(&world, 0.5, 0.5);

        // Sprint until empty (loaded, so it drains fast).
        for _ in 0..2000 {
            movement.tick(cmd(bits::FWD | bits::SPRINT), &world, 1.0);
            if movement.stamina <= 0.0 {
                break;
            }
        }
        assert!(movement.stamina <= 0.0, "stamina never emptied");

        // Empty: still moving, at walk pace, not stopped.
        let before = movement.body.position.x;
        for _ in 0..20 {
            movement.tick(cmd(bits::FWD | bits::SPRINT), &world, 1.0);
        }
        assert!(
            movement.body.position.x - before > 1.0,
            "empty stamina stopped the player"
        );
        assert!(!matches!(movement.stance, Stance::Sprinting));

        // Rest: it comes back.
        for _ in 0..(tune::REGEN_DELAY_TICKS + 100) {
            movement.tick(cmd(0), &world, 1.0);
        }
        assert!(movement.stamina > 30.0, "stamina never regenerated");
    }

    #[test]
    fn mass_multiplier_bounds_and_monotonicity() {
        assert_eq!(mass_multiplier(0.0), 1.0);
        assert!((mass_multiplier(1.0) - tune::MASS_FLOOR).abs() < 1e-6);
        let mut last = 2.0;
        for i in 0..=10 {
            let m = mass_multiplier(i as f32 / 10.0);
            assert!(m <= last);
            last = m;
        }
        // Hostile inputs clamp rather than extrapolate.
        assert_eq!(mass_multiplier(99.0), tune::MASS_FLOOR);
        assert_eq!(mass_multiplier(-5.0), 1.0);
    }

    #[test]
    fn command_hygiene_holds_under_hostile_input() {
        let world = platform_world();
        let mut movement = spawn(&world, 0.5, 0.5);

        // Unknown bits are ignored; every yaw_q resolves without NaN.
        for i in 0..64 {
            let hostile = MoveCommand {
                bits: 0xff00 | (i as u16),
                yaw_q: (i as i16).wrapping_mul(-1021),
                pitch_q: i16::MIN + i as i16,
            };
            movement.tick(hostile, &world, 0.5);
            assert!(movement.body.position.is_finite());
            assert!(movement.body.velocity.is_finite());
            assert!(movement.stamina.is_finite());
        }

        // The full quantised range maps to unit-ish directions.
        for q in [i16::MIN, -1, 0, 1, 4095, 4096, i16::MAX] {
            let (s, c) = yaw_sincos(q);
            assert!(s.is_finite() && c.is_finite());
            assert!(((s * s + c * c) - 1.0).abs() < 1e-5);
        }

        // Quantisation wraps unbounded camera yaw.
        for yaw in [-1000.0f32, -0.1, 0.0, 7.0, 12345.0] {
            let q = quantise_yaw(yaw);
            assert!((0..YAW_STEPS as i16).contains(&q), "yaw {yaw} -> {q}");
        }
    }

    #[test]
    fn no_escape_across_a_rough_script() {
        // Property: whatever the commands, the body never ends a tick inside
        // solid rock. Deterministic pseudo-random script from an LCG.
        let mut world = platform_world();
        let rock = stone(&world);
        // Scatter obstacles.
        let mut seed = 0x12345678u32;
        let mut rand = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            seed
        };
        for _ in 0..60 {
            let x = 5 + (rand() % 40) as i32;
            let z = (rand() % 12) as i32 - 6;
            let h = (rand() % 3) as i32;
            for dy in 0..=h {
                world.set_block(BlockPos::new(x, 200 + dy, z), rock);
            }
        }

        let mut movement = spawn(&world, 0.5, 0.5);
        for _ in 0..600 {
            let command = MoveCommand {
                bits: (rand() % 256) as u16,
                yaw_q: (rand() % YAW_STEPS as u32) as i16,
                pitch_q: 0,
            };
            movement.tick(command, &world, 0.3);

            let (min, max) = movement.body.bounds();
            // Sample the box corners and centre against the world.
            for corner in [
                Vec3::new(min.x + 0.01, min.y + 0.01, min.z + 0.01),
                Vec3::new(max.x - 0.01, min.y + 0.01, max.z - 0.01),
                Vec3::new(min.x + 0.01, max.y - 0.01, max.z - 0.01),
                (min + max) * 0.5,
            ] {
                let block = BlockPos::new(
                    corner.x.floor() as i32,
                    corner.y.floor() as i32,
                    corner.z.floor() as i32,
                );
                assert!(
                    !world.is_solid(block),
                    "body inside rock at {block:?} (stance {:?})",
                    movement.stance
                );
            }
        }
    }

    #[test]
    fn swimming_engages_in_water_and_space_surfaces() {
        let mut world = platform_world();
        let rock = stone(&world);
        let water = world.generator().blocks().water;
        // A pool: basin at x 30..36, water 2 deep on a stone bed.
        for x in 30..36 {
            for z in -3..3 {
                world.set_block(BlockPos::new(x, 199, z), rock); // bed (replaces platform)
                world.set_block(BlockPos::new(x, 200, z), water);
                world.set_block(BlockPos::new(x, 201, z), water);
            }
        }

        let mut movement = Movement::new(Vec3::new(32.5, 203.0, 0.5));
        // Fall in.
        for _ in 0..30 {
            movement.tick(MoveCommand::default(), &world, 0.0);
        }
        assert!(
            matches!(movement.stance, Stance::Swimming),
            "in the pool but {:?}",
            movement.stance
        );

        // Sinks slowly, never faster than the clamp.
        assert!(movement.body.velocity.y >= -3.0);

        // Space swims up.
        let depth = movement.body.position.y;
        for _ in 0..20 {
            movement.tick(cmd(bits::JUMP), &world, 0.0);
        }
        assert!(movement.body.position.y > depth, "space did not surface");
    }

    #[test]
    fn interpolation_stays_between_snapshots_and_clamps_alpha() {
        let world = platform_world();
        let mut movement = spawn(&world, 0.5, 0.5);
        movement.tick(cmd(bits::FWD), &world, 0.0);
        movement.tick(cmd(bits::FWD), &world, 0.0);

        let a = movement.camera_position(0.0);
        let b = movement.camera_position(1.0);
        let mid = movement.camera_position(0.5);
        assert!(mid.x >= a.x.min(b.x) && mid.x <= a.x.max(b.x));

        // Hostile alphas clamp instead of extrapolating.
        assert_eq!(movement.camera_position(-5.0), a);
        assert_eq!(movement.camera_position(99.0), b);
        assert!(movement.camera_position(f32::NAN).is_finite());
    }
}
