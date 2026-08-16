//! Turning input into world edits, through the drill and the inventory.
//!
//! The bar has two halves. Positions one and two hold the **deck** and the
//! **drill** — equipment, permanently carried, never in the inventory, so
//! neither can be dropped, overwritten or lost. Positions three to nine are
//! the seven item slots.
//!
//! Mining is no longer a click. The app reports intent every frame — "the
//! drill is on this block" — and `World::mine` decides when it yields. The
//! drill is the only tool that breaks anything: there is deliberately no
//! hand-mining path in this file or anywhere else.
//!
//! Module install/remove and tier upgrades are transactions against the
//! inventory with the same conservation discipline as crafting: check first,
//! then commit both sides, so the half-done state cannot exist.

use vx_core::{BlockPos, ItemRegistry};
use vx_render::Camera;
use vx_world::{
    Drill, EditError, Inventory, MineOutcome, ModuleKind, RayHit, World, HOTBAR_SLOTS,
};
use winit::keyboard::KeyCode;

/// What the player has in hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// The portable computer: right-click or `E` opens it.
    Deck,
    /// The mining tool, and the only one.
    Drill,
    /// One of the seven item slots, by index.
    Item(usize),
}

impl Held {
    /// Position on the nine-wide bar, for drawing the selection.
    pub fn bar_index(self) -> usize {
        match self {
            Held::Deck => 0,
            Held::Drill => 1,
            Held::Item(slot) => 2 + slot.min(HOTBAR_SLOTS - 1),
        }
    }

    /// The bar position a number key selects: `1` and `2` are the devices,
    /// `3`–`9` the item slots.
    pub fn from_key(code: KeyCode) -> Option<Held> {
        let held = match code {
            KeyCode::Digit1 => Held::Deck,
            KeyCode::Digit2 => Held::Drill,
            KeyCode::Digit3 => Held::Item(0),
            KeyCode::Digit4 => Held::Item(1),
            KeyCode::Digit5 => Held::Item(2),
            KeyCode::Digit6 => Held::Item(3),
            KeyCode::Digit7 => Held::Item(4),
            KeyCode::Digit8 => Held::Item(5),
            KeyCode::Digit9 => Held::Item(6),
            _ => return None,
        };
        Some(held)
    }

    /// Step along the bar, wrapping, for the scroll wheel.
    pub fn cycled(self, delta: i32) -> Held {
        let width = (2 + HOTBAR_SLOTS) as i32;
        let next = (self.bar_index() as i32 + delta).rem_euclid(width) as usize;
        match next {
            0 => Held::Deck,
            1 => Held::Drill,
            slot => Held::Item(slot - 2),
        }
    }
}

/// The block the camera is pointing at, within `reach`.
pub fn target(world: &World, camera: &Camera, reach: f32) -> Option<RayHit> {
    world.raycast_solid(camera.position, camera.forward(), reach)
}

/// One frame of drilling: raycast at drill reach, report intent to the world,
/// and bank the drop when the block yields.
///
/// Returns what happened, or `None` when nothing was in reach — in which case
/// any accumulated progress has been discarded.
pub fn mine_tick(
    world: &mut World,
    camera: &Camera,
    drill: &Drill,
    inventory: &mut Inventory,
    dt: f32,
) -> Option<MineOutcome> {
    let Some(hit) = target(world, camera, drill.reach()) else {
        world.stop_mining();
        return None;
    };

    let outcome = world.mine(hit.block, drill.speed(), dt);
    if let MineOutcome::Broke(block) = outcome {
        if let Some(drop) = world.drop_for(block) {
            // Fortune modules add whole extra drops.
            let count = drop.count.saturating_add(drill.bonus_drops());
            let leftover = inventory.insert(drop.item, count, world.items());
            if leftover > 0 {
                // Nowhere for it to go until dropped-item entities exist;
                // refusing the break would trap the player in a hole.
                log::debug!("inventory full; {leftover} of the drop was lost");
            }
        }
    }
    Some(outcome)
}

/// Place from an item slot against the targeted face.
pub fn place(
    world: &mut World,
    camera: &Camera,
    inventory: &mut Inventory,
    slot: usize,
    reach: f32,
) -> Result<BlockPos, EditError> {
    let hit = target(world, camera, reach).ok_or(EditError::OutOfReach)?;
    let held = inventory.slot(slot).ok_or(EditError::NothingHeld)?;
    let block = world
        .items()
        .block_of(held.item)
        .ok_or(EditError::NothingHeld)?;
    let pos = hit.placement().ok_or(EditError::Occupied)?;
    world.place_block(pos, block)?;

    // Deduct only after the world accepted the block, from the exact slot on
    // screen: the two sides of the trade commit together or not at all.
    let taken = inventory.take_one(slot);
    debug_assert_eq!(taken, Some(held.item), "the slot changed mid-place");
    Ok(pos)
}

/// Why an equipment transaction was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EquipError {
    #[error("no module of that kind is carried")]
    NotCarried,
    #[error("the inventory has no room for the removed module")]
    NoRoom,
    #[error("{0}")]
    Module(#[from] vx_world::ModuleError),
    #[error("the drill is already at its highest tier")]
    AtMaxTier,
    #[error("no drill upgrade is carried")]
    NoUpgrade,
}

/// Fit a carried module into a drill slot, consuming the item.
///
/// Order matters for conservation: the drill slot is claimed first — it fails
/// without side effects — and only then is the item removed, which cannot
/// fail because the count was checked while nothing else could touch the
/// inventory. No hand-back path exists, so no bug in one.
pub fn install_module(
    drill: &mut Drill,
    inventory: &mut Inventory,
    items: &ItemRegistry,
    slot: usize,
    kind: ModuleKind,
) -> Result<(), EquipError> {
    let item = kind.item(items).ok_or(EquipError::NotCarried)?;
    if inventory.count_of(item) < 1 {
        return Err(EquipError::NotCarried);
    }
    drill.install(slot, kind)?;
    let removed = inventory.remove(item, 1);
    debug_assert!(removed, "the count was checked above");
    Ok(())
}

/// Take a module out of the drill and back into the inventory.
///
/// The room check comes first: pulling the module and then discovering the
/// inventory is full would need it pushed back, and a bug there shreds it.
pub fn remove_module(
    drill: &mut Drill,
    inventory: &mut Inventory,
    items: &ItemRegistry,
    slot: usize,
) -> Result<ModuleKind, EquipError> {
    let Some(kind) = drill.module(slot) else {
        // Delegate to the drill for the precise refusal.
        return Err(drill.remove(slot).expect_err("slot was empty").into());
    };
    let item = kind.item(items).ok_or(EquipError::NoRoom)?;
    if !inventory.can_accept(item, 1, items) {
        return Err(EquipError::NoRoom);
    }
    let removed = drill.remove(slot)?;
    debug_assert_eq!(removed, kind);
    let leftover = inventory.insert(item, 1, items);
    debug_assert_eq!(leftover, 0, "can_accept said there was room");
    Ok(kind)
}

/// Spend a carried drill upgrade to raise the tier.
pub fn upgrade_drill(
    drill: &mut Drill,
    inventory: &mut Inventory,
    upgrade_item: vx_core::ItemId,
) -> Result<u8, EquipError> {
    // Tier first: at the ceiling the item must not be consumed.
    if drill.tier() >= vx_world::MAX_TIER {
        return Err(EquipError::AtMaxTier);
    }
    if !inventory.remove(upgrade_item, 1) {
        return Err(EquipError::NoUpgrade);
    }
    let upgraded = drill.upgrade();
    debug_assert!(upgraded, "tier was checked above");
    Ok(drill.tier())
}

/// Label for one item slot: `STONE x12`, or a dash for empty.
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

/// The seven item-slot labels, for the HUD.
pub fn hotbar_labels(inventory: &Inventory, items: &ItemRegistry) -> Vec<String> {
    (0..HOTBAR_SLOTS)
        .map(|slot| hotbar_label(inventory, items, slot))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use vx_core::ChunkPos;

    /// A world with terrain around the origin, a camera looking straight down
    /// at it, and the player's starting kit.
    fn looking_down() -> (World, Camera, Inventory, Drill) {
        let mut world = World::new(2468);
        world.load_around(ChunkPos::new(0, 0), 2);
        let surface = world.surface_y(8, 8).unwrap();
        let camera = Camera {
            position: Vec3::new(8.5, surface as f32 + 3.0, 8.5),
            pitch: -std::f32::consts::FRAC_PI_2 + 0.001,
            yaw: 0.0,
            ..Camera::default()
        };
        (world, camera, Inventory::player(), Drill::new())
    }

    /// Hold the drill on whatever is targeted until it breaks.
    fn mine_until_broken(
        world: &mut World,
        camera: &Camera,
        drill: &Drill,
        inventory: &mut Inventory,
    ) -> (BlockPos, u32) {
        let mut frames = 0;
        loop {
            frames += 1;
            assert!(frames < 10_000, "mining never completed");
            match mine_tick(world, camera, drill, inventory, 0.016) {
                Some(MineOutcome::Broke(_)) => {
                    // The hit position is gone now; report frames only.
                    return (BlockPos::new(0, 0, 0), frames);
                }
                Some(MineOutcome::Progressing(_)) => {}
                Some(MineOutcome::Refused(refusal)) => panic!("refused: {refusal}"),
                None => panic!("nothing in reach"),
            }
        }
    }

    #[test]
    fn mining_takes_frames_and_banks_the_drop() {
        let (mut world, camera, mut inventory, drill) = looking_down();
        let expected = world
            .drop_for(world.block(target(&world, &camera, drill.reach()).unwrap().block))
            .unwrap();

        let (_, frames) = mine_until_broken(&mut world, &camera, &drill, &mut inventory);

        assert!(frames > 5, "a tier-one drill broke ground in {frames} frames");
        assert_eq!(inventory.count_of(expected.item), expected.count as u64);
    }

    #[test]
    fn a_speed_module_shortens_the_dig() {
        let (mut world, camera, mut inventory, mut drill) = looking_down();
        let (_, slow) = mine_until_broken(&mut world, &camera, &drill, &mut inventory);

        drill.install(0, ModuleKind::Speed).unwrap();
        let (_, fast) = mine_until_broken(&mut world, &camera, &drill, &mut inventory);

        assert!(fast < slow, "{slow} frames then {fast} frames");
    }

    #[test]
    fn fortune_modules_add_extra_drops() {
        let (mut world, camera, mut inventory, mut drill) = looking_down();
        while drill.upgrade() {}
        drill.install(0, ModuleKind::Fortune).unwrap();
        drill.install(1, ModuleKind::Fortune).unwrap();

        let hit = target(&world, &camera, drill.reach()).unwrap();
        let expected = world.drop_for(world.block(hit.block)).unwrap();

        mine_until_broken(&mut world, &camera, &drill, &mut inventory);

        assert_eq!(
            inventory.count_of(expected.item),
            expected.count as u64 + 2,
            "two fortune modules should add two drops"
        );
    }

    #[test]
    fn looking_at_nothing_stops_the_dig() {
        let (mut world, mut camera, mut inventory, drill) = looking_down();
        // Start a dig...
        mine_tick(&mut world, &camera, &drill, &mut inventory, 0.016);
        assert!(world.mining_progress().is_some());

        // ...then look at the sky.
        camera.pitch = std::f32::consts::FRAC_PI_2 - 0.001;
        let outcome = mine_tick(&mut world, &camera, &drill, &mut inventory, 0.016);

        assert!(outcome.is_none());
        assert!(world.mining_progress().is_none(), "progress survived");
    }

    #[test]
    fn placing_consumes_exactly_one_from_the_selected_slot() {
        let (mut world, camera, mut inventory, drill) = looking_down();
        let stone = world.game_items().stone;
        inventory.insert(stone, 5, world.items());

        let placed = place(&mut world, &camera, &mut inventory, 0, drill.reach()).unwrap();

        let stone_block = world.registry().id_of("engine:stone").unwrap();
        assert_eq!(world.block(placed), stone_block);
        assert_eq!(inventory.count_of(stone), 4);
    }

    #[test]
    fn an_empty_hand_or_a_material_places_nothing() {
        let (mut world, camera, mut inventory, drill) = looking_down();
        assert_eq!(
            place(&mut world, &camera, &mut inventory, 0, drill.reach()),
            Err(EditError::NothingHeld)
        );

        let coal = world.game_items().coal;
        inventory.insert(coal, 3, world.items());
        assert_eq!(
            place(&mut world, &camera, &mut inventory, 0, drill.reach()),
            Err(EditError::NothingHeld)
        );
        assert_eq!(inventory.count_of(coal), 3, "a refused place consumed coal");
    }

    #[test]
    fn install_and_remove_cycles_conserve_modules_exactly() {
        // The dupe test for equipment: item and fitted module must always sum
        // to one, through many cycles.
        let (world, _, mut inventory, mut drill) = looking_down();
        let items = world.items().clone();
        let speed_item = ModuleKind::Speed.item(&items).unwrap();
        inventory.insert(speed_item, 1, &items);

        for _ in 0..20 {
            install_module(&mut drill, &mut inventory, &items, 0, ModuleKind::Speed).unwrap();
            assert_eq!(inventory.count_of(speed_item), 0, "installed but kept the item");
            assert_eq!(drill.count_of(ModuleKind::Speed), 1);

            remove_module(&mut drill, &mut inventory, &items, 0).unwrap();
            assert_eq!(inventory.count_of(speed_item), 1, "removed but count drifted");
            assert_eq!(drill.count_of(ModuleKind::Speed), 0);
        }
    }

    #[test]
    fn installing_without_the_item_changes_nothing() {
        let (world, _, mut inventory, mut drill) = looking_down();
        let items = world.items().clone();

        assert_eq!(
            install_module(&mut drill, &mut inventory, &items, 0, ModuleKind::Speed),
            Err(EquipError::NotCarried)
        );
        assert!(drill.module(0).is_none(), "a module appeared from nowhere");
    }

    #[test]
    fn installing_into_a_locked_slot_keeps_the_item() {
        let (world, _, mut inventory, mut drill) = looking_down();
        let items = world.items().clone();
        let speed_item = ModuleKind::Speed.item(&items).unwrap();
        inventory.insert(speed_item, 1, &items);

        // Tier one only unlocks slot zero.
        let refused =
            install_module(&mut drill, &mut inventory, &items, 1, ModuleKind::Speed);
        assert!(refused.is_err());
        assert_eq!(
            inventory.count_of(speed_item),
            1,
            "a refused install consumed the module"
        );
    }

    #[test]
    fn removing_into_a_full_inventory_leaves_the_module_fitted() {
        let (world, _, mut inventory, mut drill) = looking_down();
        let items = world.items().clone();
        let speed_item = ModuleKind::Speed.item(&items).unwrap();
        inventory.insert(speed_item, 1, &items);
        install_module(&mut drill, &mut inventory, &items, 0, ModuleKind::Speed).unwrap();

        // Pack the inventory with something the module cannot stack onto.
        let coal = world.game_items().coal;
        for _ in 0..inventory.len() {
            inventory.insert(coal, 64, &items);
        }

        assert_eq!(
            remove_module(&mut drill, &mut inventory, &items, 0),
            Err(EquipError::NoRoom)
        );
        assert_eq!(
            drill.module(0),
            Some(ModuleKind::Speed),
            "the module was shredded"
        );
    }

    #[test]
    fn tier_upgrades_consume_the_item_and_stop_at_the_ceiling() {
        let (world, _, mut inventory, mut drill) = looking_down();
        let items = world.items().clone();
        let upgrade = world.game_items().drill_upgrade;
        inventory.insert(upgrade, 2, &items);

        assert_eq!(upgrade_drill(&mut drill, &mut inventory, upgrade), Ok(2));
        assert_eq!(upgrade_drill(&mut drill, &mut inventory, upgrade), Ok(3));
        assert_eq!(inventory.count_of(upgrade), 0);

        // At the ceiling with none left: refused for the right reason, and a
        // fresh upgrade item is not consumed either.
        inventory.insert(upgrade, 1, &items);
        assert_eq!(
            upgrade_drill(&mut drill, &mut inventory, upgrade),
            Err(EquipError::AtMaxTier)
        );
        assert_eq!(inventory.count_of(upgrade), 1, "consumed at the ceiling");
    }

    #[test]
    fn the_bar_maps_keys_devices_first_then_items() {
        assert_eq!(Held::from_key(KeyCode::Digit1), Some(Held::Deck));
        assert_eq!(Held::from_key(KeyCode::Digit2), Some(Held::Drill));
        assert_eq!(Held::from_key(KeyCode::Digit3), Some(Held::Item(0)));
        assert_eq!(Held::from_key(KeyCode::Digit9), Some(Held::Item(6)));
        assert_eq!(Held::from_key(KeyCode::Digit0), None);
        assert_eq!(Held::from_key(KeyCode::KeyW), None);
    }

    #[test]
    fn cycling_the_bar_wraps_through_devices_and_items() {
        let mut held = Held::Deck;
        for _ in 0..(2 + HOTBAR_SLOTS) {
            held = held.cycled(1);
        }
        assert_eq!(held, Held::Deck, "a full cycle did not return home");

        assert_eq!(Held::Deck.cycled(-1), Held::Item(HOTBAR_SLOTS - 1));
        assert_eq!(Held::Drill.cycled(1), Held::Item(0));
        assert_eq!(Held::Item(0).bar_index(), 2);
    }

    #[test]
    fn hotbar_labels_cover_exactly_the_seven_item_slots() {
        let (world, _, mut inventory, _) = looking_down();
        inventory.insert(world.game_items().stone, 12, world.items());

        let labels = hotbar_labels(&inventory, world.items());
        assert_eq!(labels.len(), HOTBAR_SLOTS);
        assert_eq!(labels[0], "STONE x12");
        assert_eq!(labels[6], "-");
    }
}
