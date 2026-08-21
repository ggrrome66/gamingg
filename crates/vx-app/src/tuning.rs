//! Live-tunable constants: the numbers a designer drags, made draggable.
//!
//! # Why a struct and not the consts
//!
//! Spawning a drone is obviously state. Dragging `FRICTION_SLIDE` from 1.6 to
//! 1.2 *feels* like editing a file, but it changes how every subsequent command
//! is interpreted — replay the same journal under different constants and the
//! world diverges. So the tunables move here, the journal records
//! `SetTuning { key, value }` at its tick, and a journal now carries its
//! physics with it. The constants in `movement.rs` become this struct's
//! *defaults*, which is also what the gold panel shows you against.
//!
//! # Keys are names, never indices
//!
//! The same rule as every save format in this repository: a numeric field
//! index would silently retarget when a field is added. `set`/`get` go through
//! the name, and an unknown name is refused loudly rather than absorbed.
//!
//! # Scope
//!
//! The movement tranche, plus the arsenal's numbers when they land. The
//! physics constants (`GRAVITY`, `STEP_HEIGHT`…) stay `const`: they cross the
//! crate boundary into `vx-world`, and threading a runtime value through
//! `step_aabb` is a bigger cut than tuning has yet earned. Economy and mining
//! tranches join the same way when someone actually needs to drag them.

use crate::arsenal;
use crate::movement;

/// Every tunable, with the shipped constants as its defaults.
///
/// `Copy`, deliberately: it rides inside [`crate::movement::Movement`], which
/// is `Copy`, and a map would quietly take that away.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tuning {
    pub walk: f32,
    pub sprint_speed: f32,
    pub crouch_speed: f32,
    pub prone_speed: f32,
    pub accel_ground: f32,
    pub accel_air: f32,
    pub accel_slide: f32,
    pub friction: f32,
    pub friction_slide: f32,
    pub friction_air: f32,
    pub slide_entry: f32,
    pub slide_boost: f32,
    pub slide_cap: f32,
    pub slide_exit: f32,
    pub slide_landing_transfer: f32,
    pub stam_max: f32,
    pub stam_sprint: f32,
    pub stam_slide: f32,
    pub stam_mantle: f32,
    pub stam_regen: f32,
    pub stam_regen_delay: f32,
    pub winded: f32,
    pub slug_speed: f32,
    pub slug_gravity: f32,
    pub slug_punch: f32,
    pub slug_kick: f32,
    pub slug_rate: f32,
    pub shake_power: f32,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning {
            walk: movement::WALK,
            sprint_speed: movement::SPRINT_SPEED,
            crouch_speed: movement::CROUCH_SPEED,
            prone_speed: movement::PRONE_SPEED,
            accel_ground: movement::ACCEL_GROUND,
            accel_air: movement::ACCEL_AIR,
            accel_slide: movement::ACCEL_SLIDE,
            friction: movement::FRICTION,
            friction_slide: movement::FRICTION_SLIDE,
            friction_air: movement::FRICTION_AIR,
            slide_entry: movement::SLIDE_ENTRY,
            slide_boost: movement::SLIDE_BOOST,
            slide_cap: movement::SLIDE_CAP,
            slide_exit: movement::SLIDE_EXIT,
            slide_landing_transfer: movement::SLIDE_LANDING_TRANSFER,
            stam_max: movement::STAM_MAX,
            stam_sprint: movement::STAM_SPRINT,
            stam_slide: movement::STAM_SLIDE,
            stam_mantle: movement::STAM_MANTLE,
            stam_regen: movement::STAM_REGEN,
            stam_regen_delay: movement::STAM_REGEN_DELAY,
            winded: movement::WINDED,
            slug_speed: arsenal::SLUG_SPEED,
            slug_gravity: arsenal::SLUG_GRAVITY,
            slug_punch: arsenal::SLUG_PUNCH,
            slug_kick: arsenal::SLUG_KICK,
            slug_rate: arsenal::SLUG_RATE,
            shake_power: arsenal::SHAKE_POWER,
        }
    }
}

/// One row of the name table: the key, and where it lives.
macro_rules! tunables {
    ($( $key:literal => $field:ident ),+ $(,)?) => {
        /// Every key, in panel order. Read by the gold panel and the tests;
        /// a build with neither has no reader, which is fine and said here so
        /// dead-code analysis is being overridden knowingly.
        #[allow(dead_code)]
        pub const KEYS: &[&str] = &[ $( $key ),+ ];

        impl Tuning {
            /// Read a tunable by name. The gold panel and the tests are the
            /// readers; a build with neither has none, knowingly.
            #[allow(dead_code)]
            pub fn get(&self, key: &str) -> Option<f32> {
                match key {
                    $( $key => Some(self.$field), )+
                    _ => None,
                }
            }

            /// Write a tunable by name. `false` means the name is unknown,
            /// which the caller must surface — a silently-absorbed typo would
            /// mean a journal that claims a tuning it never applied.
            pub fn set(&mut self, key: &str, value: f32) -> bool {
                match key {
                    $( $key => { self.$field = value; true } )+
                    _ => false,
                }
            }
        }
    };
}

tunables! {
    "walk" => walk,
    "sprint_speed" => sprint_speed,
    "crouch_speed" => crouch_speed,
    "prone_speed" => prone_speed,
    "accel_ground" => accel_ground,
    "accel_air" => accel_air,
    "accel_slide" => accel_slide,
    "friction" => friction,
    "friction_slide" => friction_slide,
    "friction_air" => friction_air,
    "slide_entry" => slide_entry,
    "slide_boost" => slide_boost,
    "slide_cap" => slide_cap,
    "slide_exit" => slide_exit,
    "slide_landing_transfer" => slide_landing_transfer,
    "stam_max" => stam_max,
    "stam_sprint" => stam_sprint,
    "stam_slide" => stam_slide,
    "stam_mantle" => stam_mantle,
    "stam_regen" => stam_regen,
    "stam_regen_delay" => stam_regen_delay,
    "winded" => winded,
    "slug_speed" => slug_speed,
    "slug_gravity" => slug_gravity,
    "slug_punch" => slug_punch,
    "slug_kick" => slug_kick,
    "slug_rate" => slug_rate,
    "shake_power" => shake_power,
}

/// How a key reads on the panel: uppercase, underscores as spaces (the bitmap
/// font has no underscore glyph).
#[allow(dead_code)]
pub fn label(key: &str) -> String {
    key.to_uppercase().replace('_', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_reads_and_writes_its_own_field() {
        let mut tuning = Tuning::default();
        for (index, key) in KEYS.iter().enumerate() {
            let sentinel = 1000.0 + index as f32;
            assert!(tuning.set(key, sentinel), "{key} refused a write");
            assert_eq!(tuning.get(key), Some(sentinel), "{key} read back wrong");
        }
        // And no two keys alias the same field.
        for (index, key) in KEYS.iter().enumerate() {
            assert_eq!(tuning.get(key), Some(1000.0 + index as f32), "{key} was aliased");
        }
    }

    #[test]
    fn an_unknown_key_is_refused_loudly() {
        let mut tuning = Tuning::default();
        assert!(!tuning.set("gravity_but_misspelt", 1.0));
        assert_eq!(tuning.get("gravity_but_misspelt"), None);
        assert_eq!(tuning, Tuning::default(), "a refused write changed something");
    }

    #[test]
    fn the_defaults_are_the_shipped_constants() {
        let tuning = Tuning::default();
        assert_eq!(tuning.walk, movement::WALK);
        assert_eq!(tuning.friction_slide, movement::FRICTION_SLIDE);
        assert_eq!(tuning.stam_max, movement::STAM_MAX);
    }

    #[test]
    fn every_key_is_drawable_on_the_panel() {
        // The font has no underscore, so the panel shows keys with spaces —
        // `label()` is that softening, and this holds it honest.
        for key in KEYS {
            for character in label(key).chars() {
                assert!(
                    vx_render::font::knows(character),
                    "undrawable {character:?} in {key}"
                );
            }
        }
    }
}
