//! Composite rigs: machines built from a handful of cuboids.
//!
//! A rig is data — parts with a centre, a size, a tile and maybe a spin axis
//! — turned into [`Object`]s each frame. Every part rides the existing
//! instanced path, so per-object frustum culling and the shared depth buffer
//! keep working with zero new render code, and the inverse-transpose normal
//! fix is what makes these rotated, stretched parts light correctly.
//!
//! The digger deliberately reads like the classic dig-and-sell flash games:
//! squat rust-orange hull, chunky dark wheels, pale cab glass, and a tapered
//! steel drill on the nose that spins while she is cutting. The *vibe* is
//! borrowed; every pixel is ours.
//!
//! Rigs face local +X; [`yaw_towards`] turns a movement delta into the yaw
//! that points the nose along it.

use glam::{Mat4, Vec3};
use vx_render::tiles::slot;
use vx_render::Object;

/// Which local axis a part spins around.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spin {
    /// Along the nose — drill bits.
    Roll,
    /// Around vertical — rotors.
    Yaw,
}

/// One cuboid of a rig, in rig-local coordinates: +X forward, +Y up, origin
/// at ground level under the rig's centre.
#[derive(Debug, Clone, Copy)]
pub struct Part {
    pub centre: Vec3,
    pub size: Vec3,
    pub tile: u32,
    pub spin: Option<Spin>,
}

impl Part {
    const fn fixed(centre: Vec3, size: Vec3, tile: u32) -> Self {
        Part {
            centre,
            size,
            tile,
            spin: None,
        }
    }

    const fn spinning(centre: Vec3, size: Vec3, tile: u32, spin: Spin) -> Self {
        Part {
            centre,
            size,
            tile,
            spin: Some(spin),
        }
    }
}

/// A machine's shape.
#[derive(Debug, Clone)]
pub struct Rig {
    pub parts: Vec<Part>,
}

impl Rig {
    /// The ground digger, Motherload-flavoured and original.
    pub fn digger() -> Self {
        let hull = slot::HULL;
        let tread = slot::TREAD;
        let steel = slot::STEEL;
        let cab = slot::CAB;
        Rig {
            parts: vec![
                // Hull and cab.
                Part::fixed(Vec3::new(-0.05, 0.45, 0.0), Vec3::new(0.95, 0.5, 0.68), hull),
                Part::fixed(Vec3::new(-0.14, 0.79, 0.0), Vec3::new(0.44, 0.26, 0.5), cab),
                // The nose drill, tapering, both stages spin.
                Part::spinning(Vec3::new(0.58, 0.45, 0.0), Vec3::new(0.34, 0.3, 0.3), steel, Spin::Roll),
                Part::spinning(Vec3::new(0.85, 0.45, 0.0), Vec3::new(0.22, 0.16, 0.16), steel, Spin::Roll),
                // Four chunky wheels.
                Part::fixed(Vec3::new(0.28, 0.16, 0.38), Vec3::new(0.32, 0.32, 0.14), tread),
                Part::fixed(Vec3::new(0.28, 0.16, -0.38), Vec3::new(0.32, 0.32, 0.14), tread),
                Part::fixed(Vec3::new(-0.34, 0.16, 0.38), Vec3::new(0.32, 0.32, 0.14), tread),
                Part::fixed(Vec3::new(-0.34, 0.16, -0.38), Vec3::new(0.32, 0.32, 0.14), tread),
            ],
        }
    }

    /// The flying drone: hull, skids, and a rotor that spins overhead.
    pub fn flier() -> Self {
        Rig {
            parts: vec![
                Part::fixed(Vec3::new(0.0, 0.38, 0.0), Vec3::new(0.9, 0.4, 0.6), slot::HULL),
                Part::fixed(Vec3::new(0.22, 0.64, 0.0), Vec3::new(0.34, 0.2, 0.42), slot::CAB),
                Part::spinning(Vec3::new(0.0, 0.86, 0.0), Vec3::new(1.5, 0.06, 0.16), slot::STEEL, Spin::Yaw),
                Part::fixed(Vec3::new(0.0, 0.05, 0.3), Vec3::new(0.85, 0.08, 0.1), slot::TREAD),
                Part::fixed(Vec3::new(0.0, 0.05, -0.3), Vec3::new(0.85, 0.08, 0.1), slot::TREAD),
            ],
        }
    }

    /// A villager: boots, trousers, jacket, arms and a head, with the
    /// proportions nudged per variant so the town is not staffed by clones.
    pub fn villager(variant: usize) -> Self {
        // Deterministic small nudges, one per variant.
        let stretch = match variant % 3 {
            1 => 1.08,
            2 => 0.92,
            _ => 1.0,
        };
        let girth = match variant % 3 {
            2 => 1.18,
            _ => 1.0,
        };
        let jacket = match variant % 3 {
            1 => slot::HULL, // one of them wears work orange
            _ => slot::CLOTH,
        };
        let leg = 0.62 * stretch;
        let torso_h = 0.52 * stretch;
        let torso_top = leg + torso_h;
        Rig {
            parts: vec![
                // Legs.
                Part::fixed(
                    Vec3::new(0.0, leg * 0.5, 0.09),
                    Vec3::new(0.15, leg, 0.13) * Vec3::new(girth, 1.0, 1.0),
                    slot::TREAD,
                ),
                Part::fixed(
                    Vec3::new(0.0, leg * 0.5, -0.09),
                    Vec3::new(0.15, leg, 0.13) * Vec3::new(girth, 1.0, 1.0),
                    slot::TREAD,
                ),
                // Torso.
                Part::fixed(
                    Vec3::new(0.0, leg + torso_h * 0.5, 0.0),
                    Vec3::new(0.24, torso_h, 0.4) * Vec3::new(girth, 1.0, girth),
                    jacket,
                ),
                // Arms, hanging at the sides.
                Part::fixed(
                    Vec3::new(0.0, leg + torso_h * 0.55, 0.26 * girth),
                    Vec3::new(0.11, torso_h * 0.9, 0.1),
                    jacket,
                ),
                Part::fixed(
                    Vec3::new(0.0, leg + torso_h * 0.55, -0.26 * girth),
                    Vec3::new(0.11, torso_h * 0.9, 0.1),
                    jacket,
                ),
                // Head.
                Part::fixed(
                    Vec3::new(0.0, torso_top + 0.15, 0.0),
                    Vec3::new(0.24, 0.28, 0.24),
                    slot::SKIN,
                ),
            ],
        }
    }

    /// The player, seen from behind in third person.
    ///
    /// Deliberately not a villager: same build, but a hi-vis work jacket, a
    /// hard hat, and the boring drill slung at the hip, so at a glance you
    /// can tell yourself from the townsfolk in a crowd.
    pub fn player() -> Self {
        let leg = 0.64;
        let torso_h = 0.56;
        let torso_top = leg + torso_h;
        Rig {
            parts: vec![
                // Legs.
                Part::fixed(Vec3::new(0.0, leg * 0.5, 0.10), Vec3::new(0.16, leg, 0.14), slot::TREAD),
                Part::fixed(Vec3::new(0.0, leg * 0.5, -0.10), Vec3::new(0.16, leg, 0.14), slot::TREAD),
                // Hi-vis torso.
                Part::fixed(
                    Vec3::new(0.0, leg + torso_h * 0.5, 0.0),
                    Vec3::new(0.26, torso_h, 0.42),
                    slot::HULL,
                ),
                // Arms.
                Part::fixed(
                    Vec3::new(0.0, leg + torso_h * 0.55, 0.27),
                    Vec3::new(0.12, torso_h * 0.9, 0.11),
                    slot::HULL,
                ),
                Part::fixed(
                    Vec3::new(0.0, leg + torso_h * 0.55, -0.27),
                    Vec3::new(0.12, torso_h * 0.9, 0.11),
                    slot::HULL,
                ),
                // Head, then the hard hat on top of it.
                Part::fixed(Vec3::new(0.0, torso_top + 0.14, 0.0), Vec3::new(0.24, 0.26, 0.24), slot::SKIN),
                Part::fixed(Vec3::new(0.0, torso_top + 0.30, 0.0), Vec3::new(0.30, 0.10, 0.30), slot::HULL),
                // The drill on the hip, so the silhouette says "miner".
                Part::fixed(
                    Vec3::new(0.10, leg + 0.06, 0.26),
                    Vec3::new(0.30, 0.13, 0.12),
                    slot::STEEL,
                ),
            ],
        }
    }

    /// The same rig squashed vertically.
    ///
    /// Stance poses without an animation system: a crouching body is a standing
    /// body with its height scaled, which is crude but honest — the parts are
    /// boxes and there are no joints to bend. It reads correctly in third
    /// person, which is the only place anyone sees it.
    pub fn compressed(&self, factor: f32) -> Self {
        let factor = factor.clamp(0.05, 1.0);
        Rig {
            parts: self
                .parts
                .iter()
                .map(|part| {
                    let mut part = *part;
                    part.centre.y *= factor;
                    part.size.y *= factor;
                    part
                })
                .collect(),
        }
    }

    /// The player's handheld compact boring drill, sized for the viewmodel.
    pub fn hand_drill() -> Self {
        Rig {
            parts: vec![
                Part::fixed(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.34, 0.2, 0.16), slot::HULL),
                Part::fixed(Vec3::new(-0.08, -0.17, 0.0), Vec3::new(0.1, 0.18, 0.1), slot::TREAD),
                Part::spinning(Vec3::new(0.26, 0.0, 0.0), Vec3::new(0.2, 0.1, 0.1), slot::STEEL, Spin::Roll),
                Part::spinning(Vec3::new(0.41, 0.0, 0.0), Vec3::new(0.12, 0.05, 0.05), slot::STEEL, Spin::Roll),
            ],
        }
    }

    /// The slug launcher, sized for the viewmodel: a fat barrel over a boxy
    /// receiver, all mass and no grace. Like the drill it hangs off the
    /// camera, so parts sit at negative Y and the ground-origin convention
    /// does not apply.
    pub fn launcher() -> Self {
        Rig {
            parts: vec![
                // The receiver, shoulder-height and square.
                Part::fixed(Vec3::new(-0.02, -0.02, 0.0), Vec3::new(0.36, 0.16, 0.14), slot::HULL),
                // The barrel: wide bore, short throw.
                Part::fixed(Vec3::new(0.28, 0.01, 0.0), Vec3::new(0.3, 0.11, 0.11), slot::STEEL),
                // The muzzle ring, a shade wider than the barrel.
                Part::fixed(Vec3::new(0.44, 0.01, 0.0), Vec3::new(0.05, 0.14, 0.14), slot::STEEL),
                // The grip.
                Part::fixed(Vec3::new(-0.1, -0.18, 0.0), Vec3::new(0.09, 0.16, 0.09), slot::TREAD),
                // The satchel feed under the receiver.
                Part::fixed(Vec3::new(0.06, -0.14, 0.0), Vec3::new(0.14, 0.1, 0.12), slot::TREAD),
            ],
        }
    }

    /// Build this frame's objects: the rig at `position` (ground point under
    /// its centre), nose yawed by `yaw`, spinning parts rolled by `spin`.
    pub fn objects(&self, position: Vec3, yaw: f32, spin: f32) -> Vec<Object> {
        self.objects_pitched(position, yaw, 0.0, spin)
    }

    /// [`Rig::objects`] with the nose also pitched up (positive) or down.
    /// Ground rigs stay level; the handheld drill follows the player's gaze.
    pub fn objects_pitched(&self, position: Vec3, yaw: f32, pitch: f32, spin: f32) -> Vec<Object> {
        let place = Mat4::from_translation(position)
            * Mat4::from_rotation_y(yaw)
            * Mat4::from_rotation_z(pitch);
        self.parts
            .iter()
            .map(|part| {
                let turn = match part.spin {
                    Some(Spin::Roll) => Mat4::from_rotation_x(spin),
                    Some(Spin::Yaw) => Mat4::from_rotation_y(spin),
                    None => Mat4::IDENTITY,
                };
                // Unit cube spans 0..1: centre it, scale it, spin it, place it.
                let model = place
                    * Mat4::from_translation(part.centre)
                    * turn
                    * Mat4::from_scale(part.size)
                    * Mat4::from_translation(Vec3::splat(-0.5));
                Object::new(model, part.tile)
            })
            .collect()
    }
}

/// The yaw that points a rig's nose (+X) along a movement delta, or `None`
/// when the delta is too small to mean anything — callers keep the last yaw,
/// so a parked rig does not twitch.
pub fn yaw_towards(dx: f32, dz: f32) -> Option<f32> {
    if dx * dx + dz * dz < 1.0e-6 {
        return None;
    }
    Some((-dz).atan2(dx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rig_shapes_are_pinned() {
        // Part counts and spin counts are the cheap regression net for
        // "someone deleted a wheel".
        let digger = Rig::digger();
        assert_eq!(digger.parts.len(), 8);
        assert_eq!(digger.parts.iter().filter(|part| part.spin.is_some()).count(), 2);

        let flier = Rig::flier();
        assert_eq!(flier.parts.len(), 5);
        assert_eq!(flier.parts.iter().filter(|part| part.spin == Some(Spin::Yaw)).count(), 1);

        assert_eq!(Rig::hand_drill().parts.len(), 4);
        assert_eq!(Rig::launcher().parts.len(), 5);

        let player = Rig::player();
        assert_eq!(player.parts.len(), 8);
        assert!(player.parts.iter().all(|part| part.spin.is_none()), "the player does not spin");

        for variant in 0..3 {
            let villager = Rig::villager(variant);
            assert_eq!(villager.parts.len(), 6);
            assert!(villager.parts.iter().all(|part| part.spin.is_none()));
        }
        // The variants must actually look different.
        let a = Rig::villager(0).objects(Vec3::ZERO, 0.0, 0.0);
        let b = Rig::villager(1).objects(Vec3::ZERO, 0.0, 0.0);
        assert_ne!(a[2].model, b[2].model, "variant 1 is a clone of variant 0");
    }

    #[test]
    fn objects_track_position_yaw_and_spin() {
        let rig = Rig::digger();
        let here = rig.objects(Vec3::new(10.0, 60.0, 5.0), 0.0, 0.0);
        assert_eq!(here.len(), rig.parts.len());

        // Moving the rig moves every part.
        let there = rig.objects(Vec3::new(11.0, 60.0, 5.0), 0.0, 0.0);
        for (a, b) in here.iter().zip(&there) {
            assert!((b.bounds_min.x - a.bounds_min.x - 1.0).abs() < 1.0e-4);
        }

        // Yaw and spin each change the transforms.
        let yawed = rig.objects(Vec3::new(10.0, 60.0, 5.0), 1.0, 0.0);
        assert_ne!(here[2].model, yawed[2].model);
        let spun = rig.objects(Vec3::new(10.0, 60.0, 5.0), 0.0, 1.0);
        assert_ne!(here[2].model, spun[2].model, "the drill part should spin");
        assert_eq!(here[0].model, spun[0].model, "the hull must not spin");
    }

    #[test]
    fn every_part_sits_above_the_ground_point() {
        // position.y is the ground under the rig; nothing may dip below it,
        // or machines look buried in flat terrain.
        for rig in [
            Rig::digger(),
            Rig::flier(),
            Rig::player(),
            Rig::villager(0),
            Rig::villager(1),
            Rig::villager(2),
        ] {
            for object in rig.objects(Vec3::new(0.0, 50.0, 0.0), 0.7, 0.3) {
                assert!(
                    object.bounds_min.y >= 50.0 - 0.02,
                    "a part dips to {} below the ground point",
                    object.bounds_min.y
                );
            }
        }
    }

    #[test]
    fn pitch_tips_the_nose_toward_the_gaze() {
        let rig = Rig::hand_drill();
        let level = rig.objects_pitched(Vec3::ZERO, 0.0, 0.0, 0.0);
        let down = rig.objects_pitched(Vec3::ZERO, 0.0, -0.8, 0.0);
        // The bit is the last part and sits furthest along +X; pitching down
        // must carry it below its level height.
        let bit = rig.parts.len() - 1;
        assert!(
            down[bit].bounds_max.y < level[bit].bounds_min.y,
            "a downward pitch left the bit at {} (level bottom {})",
            down[bit].bounds_max.y,
            level[bit].bounds_min.y
        );
    }

    #[test]
    fn yaw_points_the_nose_along_travel() {
        // Rig forward is +X; check the four cardinals through the actual
        // rotation rather than trusting the trig by eye.
        for (dx, dz) in [(1.0f32, 0.0f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            let yaw = yaw_towards(dx, dz).expect("a unit delta has a yaw");
            let nose = Mat4::from_rotation_y(yaw).transform_vector3(Vec3::X);
            assert!(
                (nose.x - dx).abs() < 1.0e-5 && (nose.z - dz).abs() < 1.0e-5,
                "delta ({dx},{dz}) yawed the nose to {nose:?}"
            );
        }
        assert!(yaw_towards(0.0, 0.0).is_none(), "a parked rig must keep its yaw");
    }
}
