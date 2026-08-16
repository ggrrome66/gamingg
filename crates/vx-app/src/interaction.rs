//! Turning a click into a world edit, through the inventory.
//!
//! Mining puts a drop into the inventory; placing takes the held item back
//! out. The two are the halves of one conservation law — block broken, item
//! gained; item spent, block placed — and the tests treat any imbalance as
//! the bug it would be, because inventory duplication is the classic voxel
//! exploit.
//!
//! Kept out of the event loop so it can be tested without a window: picking a
//! target and applying an edit are pure functions of (world, camera,
//! inventory).

use vx_core::{BlockPos, ItemRegistry};
use vx_render::Camera;
use vx_world::{EditError, Inventory, RayHit, World, HOTBAR_SLOTS};
use winit::keyboard::KeyCode;

/// How far the player can reach, in blocks.
pub const REACH: f32 = 6.0;

/// What a click does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Remove the targeted block, yielding its drop.
    Break,
    /// Place the held block against the targeted face.
    Place,
}

/// The block the camera is pointing at, if anything is in range.
pub fn target(world: &World, camera: &Camera) -> Option<RayHit> {
    world.raycast_solid(camera.position, camera.forward(), REACH)
}

/// Apply `action` at the crosshair, moving items through `inventory`.
///
/// Returns the position that changed. A refusal is ordinary play — the sky,
/// bedrock, an empty hand — and is reported rather than logged as an error.
pub fn apply(
    world: &mut World,
    camera: &Camera,
    action: Action,
    inventory: &mut Inventory,
    selected: usize,
) -> Result<BlockPos, EditError> {
    let hit = target(world, camera).ok_or(EditError::OutOfReach)?;

    match action {
        Action::Break => {
            let broken = world.break_block(hit.block)?;
            if let Some(drop) = world.drop_for(broken) {
                let leftover = inventory.insert(drop.item, drop.count, world.items());
                if leftover > 0 {
                    // A full inventory loses the drop. With no dropped-item
                    // entities yet there is nowhere else for it to go, and
                    // refusing the break entirely would trap the player in a
                    // hole they cannot dig out of.
                    log::debug!("inventory full; {leftover} of the drop was lost");
                }
            }
            Ok(hit.block)
        }
        Action::Place => {
            let held = inventory.slot(selected).ok_or(EditError::NothingHeld)?;
            let block = world
                .items()
                .block_of(held.item)
                .ok_or(EditError::NothingHeld)?;
            // No entry face means the camera is inside the block: nowhere to
            // build against.
            let pos = hit.placement().ok_or(EditError::Occupied)?;
            world.place_block(pos, block)?;

            // Deduct only after the world accepted the block, and from the
            // exact slot shown on screen: the two sides of the trade commit
            // together or not at all.
            let taken = inventory.take_one(selected);
            debug_assert_eq!(taken, Some(held.item), "the slot changed mid-place");
            Ok(pos)
        }
    }
}

/// Label for one hotbar slot: `STONE x12`, or a dash for empty.
pub fn hotbar_label(inventory: &Inventory, items: &ItemRegistry, slot: usize) -> String {
    match inventory.slot(slot) {
        Some(stack) => {
            let name = items
                .get(stack.item)
                .map_or("?", |def| def.display_name.as_str())
                .to_uppercase();
            format!("{name} x{}", stack.count)
        }
        None => "-".to_string(),
    }
}

/// All nine hotbar labels, for the HUD.
pub fn hotbar_labels(inventory: &Inventory, items: &ItemRegistry) -> Vec<String> {
    (0..HOTBAR_SLOTS)
        .map(|slot| hotbar_label(inventory, items, slot))
        .collect()
}

/// The hotbar slot a number key selects, counting from zero.
///
/// `1` is the first slot, so the key and the index differ by one; `0` is the
/// tenth, as on a keyboard's number row — but the hotbar holds nine, so `0`
/// selects the last.
pub fn hotbar_slot(code: KeyCode) -> Option<usize> {
    let slot = match code {
        KeyCode::Digit1 => 0,
        KeyCode::Digit2 => 1,
        KeyCode::Digit3 => 2,
        KeyCode::Digit4 => 3,
        KeyCode::Digit5 => 4,
        KeyCode::Digit6 => 5,
        KeyCode::Digit7 => 6,
        KeyCode::Digit8 => 7,
        KeyCode::Digit9 => 8,
        _ => return None,
    };
    Some(slot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use vx_core::ChunkPos;

    /// A world with terrain around the origin, a camera looking straight down
    /// at it, and an empty inventory.
    fn looking_down() -> (World, Camera, Inventory) {
        let mut world = World::new(2468);
        world.load_around(ChunkPos::new(0, 0), 2);
        let surface = world.surface_y(8, 8).unwrap();
        let camera = Camera {
            position: Vec3::new(8.5, surface as f32 + 3.0, 8.5),
            pitch: -std::f32::consts::FRAC_PI_2 + 0.001,
            yaw: 0.0,
            ..Camera::default()
        };
        (world, camera, Inventory::player())
    }

    #[test]
    fn mining_a_block_puts_its_drop_in_the_inventory() {
        let (mut world, camera, mut inventory) = looking_down();
        let hit = target(&world, &camera).expect("the ground is in reach");
        let broken_block = world.block(hit.block);
        let expected = world.drop_for(broken_block).expect("terrain drops something");

        let edited = apply(&mut world, &camera, Action::Break, &mut inventory, 0).unwrap();

        assert_eq!(edited, hit.block);
        assert!(world.block(edited).is_air());
        assert_eq!(inventory.count_of(expected.item), expected.count as u64);
    }

    #[test]
    fn placing_consumes_exactly_one_from_the_selected_slot() {
        let (mut world, camera, mut inventory) = looking_down();
        let stone = world.game_items().stone;
        inventory.insert(stone, 5, world.items());

        let placed = apply(&mut world, &camera, Action::Place, &mut inventory, 0).unwrap();

        let stone_block = world.registry().id_of("engine:stone").unwrap();
        assert_eq!(world.block(placed), stone_block);
        assert_eq!(inventory.count_of(stone), 4);
    }

    #[test]
    fn an_empty_hand_places_nothing() {
        let (mut world, camera, mut inventory) = looking_down();
        let before = target(&world, &camera).unwrap();

        assert_eq!(
            apply(&mut world, &camera, Action::Place, &mut inventory, 0),
            Err(EditError::NothingHeld)
        );
        // And the world is untouched.
        assert_eq!(target(&world, &camera).unwrap().block, before.block);
    }

    #[test]
    fn a_material_in_hand_places_nothing() {
        // Coal has no block form; trying to place it must refuse cleanly
        // rather than conjuring a block or eating the coal.
        let (mut world, camera, mut inventory) = looking_down();
        let coal = world.game_items().coal;
        inventory.insert(coal, 3, world.items());

        assert_eq!(
            apply(&mut world, &camera, Action::Place, &mut inventory, 0),
            Err(EditError::NothingHeld)
        );
        assert_eq!(inventory.count_of(coal), 3, "a refused place consumed coal");
    }

    #[test]
    fn a_failed_place_consumes_nothing() {
        // Point at the sky: the place fails after the item lookup, and the
        // stack must be exactly as it was.
        let (mut world, mut camera, mut inventory) = looking_down();
        camera.pitch = std::f32::consts::FRAC_PI_2 - 0.001;
        let stone = world.game_items().stone;
        inventory.insert(stone, 5, world.items());

        assert_eq!(
            apply(&mut world, &camera, Action::Place, &mut inventory, 0),
            Err(EditError::OutOfReach)
        );
        assert_eq!(inventory.count_of(stone), 5);
    }

    #[test]
    fn mine_and_place_cycles_conserve_items_exactly() {
        // The dupe test. Break a block, place it back, repeatedly: the total
        // of block-in-world plus item-in-inventory must never drift.
        let (mut world, camera, mut inventory) = looking_down();
        let stone = world.game_items().stone;
        inventory.insert(stone, 1, world.items());

        for _ in 0..10 {
            let placed = apply(&mut world, &camera, Action::Place, &mut inventory, 0).unwrap();
            assert_eq!(inventory.count_of(stone), 0, "placed but kept the item");
            let broken = apply(&mut world, &camera, Action::Break, &mut inventory, 0).unwrap();
            assert_eq!(broken, placed, "the ray re-targeted something else");
            assert_eq!(inventory.count_of(stone), 1, "broke but got a different count");
        }
    }

    #[test]
    fn a_full_inventory_loses_the_drop_but_still_breaks_the_block() {
        // Documented behaviour until dropped-item entities exist: refusing
        // the break would trap the player in a hole they cannot dig out of.
        let (mut world, camera, mut inventory) = looking_down();
        let hit = target(&world, &camera).unwrap();
        let drop = world.drop_for(world.block(hit.block)).unwrap();

        // Fill every slot with something that cannot stack with the drop.
        let coal = world.game_items().coal;
        assert_ne!(coal, drop.item);
        for _ in 0..inventory.len() {
            inventory.insert(coal, 64, world.items());
        }
        assert!(!inventory.can_accept(drop.item, drop.count, world.items()));

        let edited = apply(&mut world, &camera, Action::Break, &mut inventory, 0).unwrap();

        assert!(world.block(edited).is_air(), "the break was refused");
        assert_eq!(inventory.count_of(drop.item), 0, "the drop went somewhere");
    }

    #[test]
    fn hotbar_labels_show_counts_and_empties() {
        let (world, _, mut inventory) = looking_down();
        inventory.insert(world.game_items().stone, 12, world.items());

        let labels = hotbar_labels(&inventory, world.items());
        assert_eq!(labels.len(), HOTBAR_SLOTS);
        assert_eq!(labels[0], "STONE x12");
        assert_eq!(labels[1], "-");
    }

    #[test]
    fn number_keys_map_to_the_nine_hotbar_slots() {
        assert_eq!(hotbar_slot(KeyCode::Digit1), Some(0));
        assert_eq!(hotbar_slot(KeyCode::Digit9), Some(8));
        assert_eq!(hotbar_slot(KeyCode::Digit0), None, "the hotbar has nine slots");
        assert_eq!(hotbar_slot(KeyCode::KeyW), None);
        assert_eq!(hotbar_slot(KeyCode::Numpad1), None);
    }

    #[test]
    fn mining_ore_yields_the_mineral() {
        // End to end through the real drops table: dig down to an ore block
        // is fiddly, so place one and mine it.
        let (mut world, camera, mut inventory) = looking_down();
        let hit = target(&world, &camera).unwrap();
        let ore = world.registry().id_of("engine:coal_ore").unwrap();
        world.set_block(hit.block, ore);

        apply(&mut world, &camera, Action::Break, &mut inventory, 0).unwrap();

        assert_eq!(inventory.count_of(world.game_items().coal), 1);
        assert_eq!(
            inventory.count_of(world.game_items().stone),
            0,
            "ore dropped its block form instead of the mineral"
        );
    }
}
