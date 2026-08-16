//! Turning a click into a world edit.
//!
//! Kept out of the event loop so it can be tested without a window: picking a
//! target and applying an edit are both pure functions of (world, camera).
//!
//! The split against `vx-world` is deliberate. The world owns the rules — what
//! may be broken, what may be built over — and this module owns only the
//! policy of *where the player is pointing*. A future server enforces the
//! former and would not trust the latter.

use vx_core::{BlockId, BlockPos, BlockRegistry};
use vx_render::Camera;
use vx_world::{EditError, RayHit, World};
use winit::keyboard::KeyCode;

/// How far the player can reach, in blocks.
pub const REACH: f32 = 6.0;

/// What a click does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Remove the targeted block.
    Break,
    /// Put the held block against the targeted face.
    Place,
}

/// The block the camera is pointing at, if anything is in range.
pub fn target(world: &World, camera: &Camera) -> Option<RayHit> {
    world.raycast_solid(camera.position, camera.forward(), REACH)
}

/// Apply `action` to whatever the camera is looking at.
///
/// Returns the position that changed. A refusal is a normal outcome — clicking
/// at the sky, or at bedrock — and is reported rather than logged as an error.
pub fn apply(
    world: &mut World,
    camera: &Camera,
    action: Action,
    holding: BlockId,
) -> Result<BlockPos, EditError> {
    let hit = target(world, camera).ok_or(EditError::OutOfReach)?;

    match action {
        Action::Break => {
            world.break_block(hit.block)?;
            Ok(hit.block)
        }
        Action::Place => {
            // No entry face means the camera is inside the block, so there is
            // no side to build against.
            let pos = hit.placement().ok_or(EditError::Occupied)?;
            world.place_block(pos, holding)?;
            Ok(pos)
        }
    }
}

/// The blocks the player can place, and which one is selected.
///
/// Built from the registry rather than a fixed list, so blocks a mod registers
/// appear here for free once mods land.
#[derive(Debug, Clone)]
pub struct Hotbar {
    slots: Vec<BlockId>,
    selected: usize,
}

impl Hotbar {
    /// Every block worth holding: solid, and breakable once placed.
    ///
    /// Excluding unbreakable blocks is what stops the player from walling
    /// themselves in with bedrock they cannot dig back out of.
    pub fn from_registry(registry: &BlockRegistry) -> Self {
        let slots = registry
            .iter()
            .filter(|(_, def)| def.solid && def.hardness.is_some())
            .map(|(id, _)| id)
            .collect();
        Hotbar { slots, selected: 0 }
    }

    /// The held block, or `None` if the registry offered nothing placeable.
    pub fn selected(&self) -> Option<BlockId> {
        self.slots.get(self.selected).copied()
    }

    /// Index of the held slot.
    pub fn selected_slot(&self) -> usize {
        self.selected
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Select a slot by index. Out-of-range slots are ignored, so pressing `7`
    /// with four blocks available keeps the current selection.
    pub fn select(&mut self, slot: usize) {
        if slot < self.slots.len() {
            self.selected = slot;
        }
    }

    /// Step through the slots, wrapping at both ends. For the scroll wheel.
    pub fn cycle(&mut self, delta: i32) {
        if self.is_empty() {
            return;
        }
        let len = self.len() as i32;
        let next = (self.selected as i32 + delta).rem_euclid(len);
        self.selected = next as usize;
    }

    /// Display names of every slot, in order, for the on-screen selector.
    pub fn names(&self, registry: &BlockRegistry) -> Vec<String> {
        self.slots
            .iter()
            .map(|id| {
                registry
                    .get(*id)
                    .map_or("?", |def| def.display_name.as_str())
                    .to_string()
            })
            .collect()
    }

    /// Display name of the held block, for the title bar.
    pub fn selected_name<'a>(&self, registry: &'a BlockRegistry) -> &'a str {
        self.selected()
            .and_then(|id| registry.get(id))
            .map_or("nothing", |def| def.display_name.as_str())
    }
}

/// The hotbar slot a number key selects, counting from zero.
///
/// `1` is the first slot, so the key and the index differ by one; `0` is the
/// tenth, as on a keyboard's number row.
pub fn hotbar_slot(code: KeyCode) -> Option<usize> {
    match code {
        KeyCode::Digit1 => Some(0),
        KeyCode::Digit2 => Some(1),
        KeyCode::Digit3 => Some(2),
        KeyCode::Digit4 => Some(3),
        KeyCode::Digit5 => Some(4),
        KeyCode::Digit6 => Some(5),
        KeyCode::Digit7 => Some(6),
        KeyCode::Digit8 => Some(7),
        KeyCode::Digit9 => Some(8),
        KeyCode::Digit0 => Some(9),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use vx_core::ChunkPos;

    /// A world with terrain around the origin, and a camera above it looking
    /// straight down.
    fn looking_down() -> (World, Camera) {
        let mut world = World::new(2468);
        world.load_around(ChunkPos::new(0, 0), 2);
        let surface = world.surface_y(8, 8).unwrap();
        let camera = Camera {
            position: Vec3::new(8.5, surface as f32 + 3.0, 8.5),
            // `pitch` is positive looking up, so this looks at the ground.
            pitch: -std::f32::consts::FRAC_PI_2 + 0.001,
            yaw: 0.0,
            ..Camera::default()
        };
        (world, camera)
    }

    fn stone(world: &World) -> BlockId {
        world.registry().id_of("engine:stone").unwrap()
    }

    #[test]
    fn breaking_removes_the_targeted_block() {
        let (mut world, camera) = looking_down();
        let hit = target(&world, &camera).expect("the ground should be in reach");
        let held = stone(&world);

        let edited = apply(&mut world, &camera, Action::Break, held).unwrap();

        assert_eq!(edited, hit.block);
        assert!(world.block(edited).is_air(), "the block is still there");
    }

    #[test]
    fn placing_builds_against_the_targeted_face_not_inside_it() {
        // The classic off-by-one: placing into the block you clicked instead of
        // the empty space in front of it.
        let (mut world, camera) = looking_down();
        let hit = target(&world, &camera).unwrap();
        let held = stone(&world);

        let edited = apply(&mut world, &camera, Action::Place, held).unwrap();

        assert_ne!(edited, hit.block, "placed inside the block that was clicked");
        assert_eq!(edited, hit.placement().unwrap());
        assert_eq!(world.block(edited), held);
        // Looking down, the new block sits on top of the old one.
        assert_eq!(edited, hit.block.offset([0, 1, 0]));
    }

    #[test]
    fn a_placed_block_can_be_broken_again() {
        let (mut world, camera) = looking_down();
        let held = stone(&world);

        let placed = apply(&mut world, &camera, Action::Place, held).unwrap();
        // The new block is now the nearest thing under the camera.
        let broken = apply(&mut world, &camera, Action::Break, held).unwrap();

        assert_eq!(broken, placed, "the ray did not re-target the new block");
        assert!(world.block(placed).is_air());
    }

    #[test]
    fn looking_at_the_sky_reaches_nothing() {
        let (mut world, mut camera) = looking_down();
        camera.pitch = std::f32::consts::FRAC_PI_2 - 0.001;
        let held = stone(&world);

        assert!(target(&world, &camera).is_none());
        for action in [Action::Break, Action::Place] {
            assert_eq!(
                apply(&mut world, &camera, action, held),
                Err(EditError::OutOfReach)
            );
        }
    }

    #[test]
    fn terrain_further_than_reach_cannot_be_touched() {
        let (mut world, mut camera) = looking_down();
        camera.position.y += REACH * 2.0;
        let held = stone(&world);

        assert!(target(&world, &camera).is_none(), "reach is not being applied");
        assert_eq!(
            apply(&mut world, &camera, Action::Break, held),
            Err(EditError::OutOfReach)
        );
    }

    #[test]
    fn an_edit_only_touches_the_one_block() {
        let (mut world, camera) = looking_down();
        let held = stone(&world);
        let before: Vec<BlockId> = (0..16)
            .map(|y| world.block(BlockPos::new(8, 60 + y, 8)))
            .collect();

        let edited = apply(&mut world, &camera, Action::Break, held).unwrap();

        for (offset, was) in before.iter().enumerate() {
            let pos = BlockPos::new(8, 60 + offset as i32, 8);
            if pos == edited {
                continue;
            }
            assert_eq!(world.block(pos), *was, "{pos:?} changed as a side effect");
        }
    }

    #[test]
    fn the_hotbar_offers_placeable_blocks_only() {
        let world = World::new(1);
        let hotbar = Hotbar::from_registry(world.registry());

        assert!(!hotbar.is_empty());
        for slot in 0..hotbar.len() {
            let mut probe = hotbar.clone();
            probe.select(slot);
            let id = probe.selected().unwrap();
            assert!(world.registry().is_solid(id), "slot {slot} is not solid");
            assert!(world.registry().is_breakable(id), "slot {slot} is unbreakable");
        }

        // Air, water and bedrock must not be on offer.
        let registry = world.registry();
        let excluded = ["engine:air", "engine:water", "engine:bedrock"];
        let offered: Vec<BlockId> = (0..hotbar.len())
            .map(|slot| {
                let mut probe = hotbar.clone();
                probe.select(slot);
                probe.selected().unwrap()
            })
            .collect();
        for name in excluded {
            let id = registry.id_of(name).unwrap();
            assert!(!offered.contains(&id), "{name} should not be placeable");
        }
    }

    #[test]
    fn selecting_out_of_range_keeps_the_current_slot() {
        let world = World::new(1);
        let mut hotbar = Hotbar::from_registry(world.registry());
        hotbar.select(1);
        let held = hotbar.selected();

        hotbar.select(999);

        assert_eq!(hotbar.selected(), held, "an unusable slot changed the selection");
    }

    #[test]
    fn cycling_wraps_around_in_both_directions() {
        let world = World::new(1);
        let mut hotbar = Hotbar::from_registry(world.registry());
        let first = hotbar.selected();

        for _ in 0..hotbar.len() {
            hotbar.cycle(1);
        }
        assert_eq!(hotbar.selected(), first, "a full cycle did not return home");

        hotbar.cycle(-1);
        assert_ne!(hotbar.selected(), first);
        hotbar.cycle(1);
        assert_eq!(hotbar.selected(), first);
    }

    #[test]
    fn an_empty_hotbar_holds_nothing_rather_than_panicking() {
        // A registry with no placeable blocks is what a stripped-down mod set
        // would produce.
        let mut hotbar = Hotbar::from_registry(&BlockRegistry::new());
        assert!(hotbar.is_empty());
        assert_eq!(hotbar.selected(), None);

        hotbar.select(0);
        hotbar.cycle(1);
        assert_eq!(hotbar.selected(), None);
        assert_eq!(hotbar.selected_name(&BlockRegistry::new()), "nothing");
    }

    #[test]
    fn the_held_block_is_named_for_the_title_bar() {
        let world = World::new(1);
        let hotbar = Hotbar::from_registry(world.registry());
        assert_eq!(hotbar.selected_name(world.registry()), "Stone");
    }

    #[test]
    fn number_keys_map_to_slots_counting_from_zero() {
        // `1` selects the first slot, not the second.
        assert_eq!(hotbar_slot(KeyCode::Digit1), Some(0));
        assert_eq!(hotbar_slot(KeyCode::Digit9), Some(8));
        // `0` sits at the end of the row, so it is the tenth slot.
        assert_eq!(hotbar_slot(KeyCode::Digit0), Some(9));
        assert_eq!(hotbar_slot(KeyCode::KeyW), None);
        assert_eq!(hotbar_slot(KeyCode::Numpad1), None);
    }
}
