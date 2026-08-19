//! Inventories: fixed slots of item stacks.
//!
//! Everything here is deliberately bounded and conserving. The slot count is
//! fixed at construction — an inventory never grows — and every operation
//! either moves items or reports what would not fit; nothing is silently
//! created or destroyed. Item duplication is the classic inventory exploit,
//! and the arithmetic that would permit it (a stack past its ceiling, a
//! remove that under-counts, an insert that both keeps and returns items) is
//! what the tests below pin.

use vx_core::{ItemId, ItemRegistry};

/// Slots in a player inventory: seven on the bar, twenty-nine behind them.
/// Total capacity is unchanged from the old nine-slot bar — the first two bar
/// positions now belong to the deck and drill, which are equipment rather
/// than inventory, and the backpack absorbed the difference.
pub const PLAYER_SLOTS: usize = 36;
/// The first seven slots are the item half of the bar.
pub const HOTBAR_SLOTS: usize = 7;

/// Some of one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemStack {
    pub item: ItemId,
    pub count: u32,
}

impl ItemStack {
    pub fn new(item: ItemId, count: u32) -> Self {
        ItemStack { item, count }
    }
}

/// A fixed row of slots, each empty or holding one stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inventory {
    slots: Vec<Option<ItemStack>>,
}

impl Inventory {
    pub fn new(slots: usize) -> Self {
        Inventory {
            slots: vec![None; slots],
        }
    }

    pub fn player() -> Self {
        Self::new(PLAYER_SLOTS)
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(|slot| slot.is_none())
    }

    pub fn slot(&self, index: usize) -> Option<ItemStack> {
        self.slots.get(index).copied().flatten()
    }

    /// Total held of one item, wide enough that no inventory can overflow it.
    pub fn count_of(&self, item: ItemId) -> u64 {
        self.slots
            .iter()
            .flatten()
            .filter(|stack| stack.item == item)
            .map(|stack| stack.count as u64)
            .sum()
    }

    /// Insert up to `count` of `item`, filling existing stacks before empty
    /// slots. Returns how many did **not** fit; the caller decides whether
    /// that overflow is dropped, refused, or reported.
    pub fn insert(&mut self, item: ItemId, count: u32, registry: &ItemRegistry) -> u32 {
        let max_stack = registry.max_stack(item);
        let mut remaining = count;

        // Top up existing stacks first, so items consolidate.
        for slot in self.slots.iter_mut().flatten() {
            if remaining == 0 {
                break;
            }
            if slot.item == item && slot.count < max_stack {
                let space = max_stack - slot.count;
                let moved = space.min(remaining);
                slot.count += moved;
                remaining -= moved;
            }
        }

        // Then open new stacks in empty slots.
        for slot in self.slots.iter_mut() {
            if remaining == 0 {
                break;
            }
            if slot.is_none() {
                let moved = remaining.min(max_stack);
                *slot = Some(ItemStack::new(item, moved));
                remaining -= moved;
            }
        }

        remaining
    }

    /// True when `insert` would place all `count` items.
    ///
    /// Used before crafting: consuming ingredients and then discovering the
    /// output does not fit would need the ingredients handed back, and any
    /// bug in that hand-back is an item dupe or an item shredder.
    pub fn can_accept(&self, item: ItemId, count: u32, registry: &ItemRegistry) -> bool {
        let max_stack = registry.max_stack(item);
        let mut space: u64 = 0;
        for slot in &self.slots {
            match slot {
                Some(stack) if stack.item == item => {
                    space += (max_stack.saturating_sub(stack.count)) as u64;
                }
                None => space += max_stack as u64,
                Some(_) => {}
            }
        }
        space >= count as u64
    }

    /// Remove exactly `count` of `item`, or nothing at all.
    ///
    /// All-or-nothing so a caller can never half-pay for something: either
    /// the full price leaves the inventory or the transaction never started.
    pub fn remove(&mut self, item: ItemId, count: u32) -> bool {
        if self.count_of(item) < count as u64 {
            return false;
        }
        let mut remaining = count;
        for slot in self.slots.iter_mut() {
            if remaining == 0 {
                break;
            }
            if let Some(stack) = slot {
                if stack.item == item {
                    let taken = stack.count.min(remaining);
                    stack.count -= taken;
                    remaining -= taken;
                    if stack.count == 0 {
                        *slot = None;
                    }
                }
            }
        }
        debug_assert_eq!(remaining, 0, "count_of said the items were there");
        true
    }

    /// Take one item from a specific slot — placement's path. Returns what
    /// was taken.
    pub fn take_one(&mut self, index: usize) -> Option<ItemId> {
        let slot = self.slots.get_mut(index)?;
        let stack = slot.as_mut()?;
        let item = stack.item;
        stack.count -= 1;
        if stack.count == 0 {
            *slot = None;
        }
        Some(item)
    }

    /// Put a stack into a specific empty slot, for restoring a saved layout.
    ///
    /// This is the only way to write a slot directly, and it validates so the
    /// caller cannot break the invariants the rest of the file maintains: the
    /// slot must exist and be empty, the count must be non-zero, and it is
    /// clamped to the item's ceiling — a save file claiming a stack of four
    /// billion loads as one full stack, not an overflow.
    ///
    /// Returns whether the stack was placed.
    pub fn place_stack(&mut self, index: usize, stack: ItemStack, registry: &ItemRegistry) -> bool {
        if stack.count == 0 {
            return false;
        }
        let Some(slot) = self.slots.get_mut(index) else {
            return false;
        };
        if slot.is_some() {
            return false;
        }
        let count = stack.count.min(registry.max_stack(stack.item));
        *slot = Some(ItemStack::new(stack.item, count));
        true
    }

    /// Every occupied slot, for the UI.
    pub fn occupied(&self) -> impl Iterator<Item = (usize, ItemStack)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.map(|stack| (index, stack)))
    }
}

/// One shapeless crafting recipe: consume the inputs, produce the output.
#[derive(Debug, Clone)]
pub struct Recipe {
    pub inputs: Vec<ItemStack>,
    pub output: ItemStack,
}

impl Recipe {
    /// True when `inventory` holds every input.
    pub fn craftable_from(&self, inventory: &Inventory) -> bool {
        self.inputs
            .iter()
            .all(|input| inventory.count_of(input.item) >= input.count as u64)
    }

    /// Craft once: check the inputs, check the output fits, then and only
    /// then consume and produce.
    ///
    /// The output check happens **before** anything is consumed. Doing it
    /// after would need the inputs handed back on failure, and a bug in that
    /// path is either an item dupe or an item shredder — better that the
    /// failure mode cannot exist.
    pub fn craft(&self, inventory: &mut Inventory, registry: &ItemRegistry) -> bool {
        if !self.craftable_from(inventory) {
            return false;
        }
        // Conservative: checked against the inventory as it stands, without
        // credit for the space the inputs will free. A craft that would only
        // fit in that freed space is refused, which costs a rare retry and
        // buys the guarantee that nothing is ever half-done.
        if !inventory.can_accept(self.output.item, self.output.count, registry) {
            return false;
        }

        for input in &self.inputs {
            let removed = inventory.remove(input.item, input.count);
            debug_assert!(removed, "craftable_from said the inputs were there");
        }
        let leftover = inventory.insert(self.output.item, self.output.count, registry);
        debug_assert_eq!(leftover, 0, "can_accept said the output would fit");
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::ItemDef;

    fn registry() -> (ItemRegistry, ItemId, ItemId) {
        let mut registry = ItemRegistry::new();
        let stone = registry.register(ItemDef::material("engine:stone")).unwrap();
        let coal = registry.register(ItemDef::material("engine:coal")).unwrap();
        (registry, stone, coal)
    }

    #[test]
    fn a_new_inventory_is_empty() {
        let inventory = Inventory::player();
        assert_eq!(inventory.len(), PLAYER_SLOTS);
        assert!(inventory.is_empty());
        assert_eq!(inventory.count_of(ItemId(0)), 0);
        assert_eq!(inventory.occupied().count(), 0);
    }

    #[test]
    fn inserting_stacks_up_before_opening_new_slots() {
        let (registry, stone, _) = registry();
        let mut inventory = Inventory::player();

        assert_eq!(inventory.insert(stone, 40, &registry), 0);
        assert_eq!(inventory.insert(stone, 40, &registry), 0);

        // 80 stone at a stack size of 64: one full stack, one of 16.
        assert_eq!(inventory.slot(0), Some(ItemStack::new(stone, 64)));
        assert_eq!(inventory.slot(1), Some(ItemStack::new(stone, 16)));
        assert_eq!(inventory.count_of(stone), 80);
        assert_eq!(inventory.occupied().count(), 2);
    }

    #[test]
    fn no_stack_ever_exceeds_its_ceiling() {
        let (registry, stone, _) = registry();
        let mut inventory = Inventory::player();
        inventory.insert(stone, 10_000, &registry);

        for (_, stack) in inventory.occupied() {
            assert!(
                stack.count <= registry.max_stack(stone),
                "a stack reached {}",
                stack.count
            );
        }
    }

    #[test]
    fn overflow_is_returned_not_vanished() {
        let (registry, stone, _) = registry();
        let mut inventory = Inventory::new(2); // room for 128 stone

        let leftover = inventory.insert(stone, 200, &registry);

        assert_eq!(leftover, 72, "128 fit, so 72 must come back");
        assert_eq!(inventory.count_of(stone), 128);
        // Conservation: held plus returned equals what went in.
        assert_eq!(inventory.count_of(stone) + leftover as u64, 200);
    }

    #[test]
    fn removal_is_all_or_nothing() {
        let (registry, stone, _) = registry();
        let mut inventory = Inventory::player();
        inventory.insert(stone, 10, &registry);

        assert!(!inventory.remove(stone, 11), "removed more than was held");
        assert_eq!(inventory.count_of(stone), 10, "a failed remove took items");

        assert!(inventory.remove(stone, 10));
        assert_eq!(inventory.count_of(stone), 0);
        assert!(inventory.is_empty(), "an emptied slot was left occupied");
    }

    #[test]
    fn removal_spans_stacks() {
        let (registry, stone, _) = registry();
        let mut inventory = Inventory::player();
        inventory.insert(stone, 100, &registry); // 64 + 36

        assert!(inventory.remove(stone, 70));
        assert_eq!(inventory.count_of(stone), 30);
    }

    #[test]
    fn take_one_decrements_and_clears_the_slot_at_zero() {
        let (registry, stone, _) = registry();
        let mut inventory = Inventory::player();
        inventory.insert(stone, 2, &registry);

        assert_eq!(inventory.take_one(0), Some(stone));
        assert_eq!(inventory.take_one(0), Some(stone));
        assert_eq!(inventory.take_one(0), None, "took from an empty slot");
        assert_eq!(inventory.slot(0), None);
        // Out-of-range slots are a None, not a panic.
        assert_eq!(inventory.take_one(999), None);
    }

    #[test]
    fn place_stack_validates_everything_a_save_file_could_lie_about() {
        let (registry, stone, coal) = registry();
        let mut inventory = Inventory::player();

        // A hostile count is clamped to the ceiling, not trusted.
        assert!(inventory.place_stack(0, ItemStack::new(stone, u32::MAX), &registry));
        assert_eq!(inventory.slot(0), Some(ItemStack::new(stone, 64)));

        // Occupied, out-of-range and zero-count are all refused.
        assert!(!inventory.place_stack(0, ItemStack::new(coal, 1), &registry));
        assert_eq!(inventory.slot(0), Some(ItemStack::new(stone, 64)), "overwritten");
        assert!(!inventory.place_stack(999, ItemStack::new(coal, 1), &registry));
        assert!(!inventory.place_stack(1, ItemStack::new(coal, 0), &registry));
        assert_eq!(inventory.count_of(coal), 0);

        // An unknown item id stacks to one, so a stale save cannot pile it.
        let stale = vx_core::ItemId(9_999);
        assert!(inventory.place_stack(2, ItemStack::new(stale, 50), &registry));
        assert_eq!(inventory.slot(2), Some(ItemStack::new(stale, 1)));
    }

    #[test]
    fn different_items_never_share_a_stack() {
        let (registry, stone, coal) = registry();
        let mut inventory = Inventory::player();
        inventory.insert(stone, 10, &registry);
        inventory.insert(coal, 10, &registry);

        assert_eq!(inventory.slot(0), Some(ItemStack::new(stone, 10)));
        assert_eq!(inventory.slot(1), Some(ItemStack::new(coal, 10)));
    }

    #[test]
    fn can_accept_agrees_with_what_insert_actually_does() {
        let (registry, stone, _) = registry();
        let mut inventory = Inventory::new(2);
        inventory.insert(stone, 100, &registry); // 64 + 36; 28 space left

        assert!(inventory.can_accept(stone, 28, &registry));
        assert!(!inventory.can_accept(stone, 29, &registry));

        // And the promise holds: inserting what was accepted leaves nothing.
        assert_eq!(inventory.insert(stone, 28, &registry), 0);
        assert_eq!(inventory.insert(stone, 1, &registry), 1);
    }

    #[test]
    fn crafting_consumes_inputs_and_produces_the_output() {
        let (registry, stone, coal) = registry();
        let mut lamp_registry = registry.clone();
        let lamp = lamp_registry.register(ItemDef::material("engine:lamp")).unwrap();

        let recipe = Recipe {
            inputs: vec![ItemStack::new(stone, 3), ItemStack::new(coal, 1)],
            output: ItemStack::new(lamp, 1),
        };

        let mut inventory = Inventory::player();
        inventory.insert(stone, 5, &lamp_registry);
        inventory.insert(coal, 2, &lamp_registry);

        assert!(recipe.craftable_from(&inventory));
        assert!(recipe.craft(&mut inventory, &lamp_registry));

        assert_eq!(inventory.count_of(stone), 2);
        assert_eq!(inventory.count_of(coal), 1);
        assert_eq!(inventory.count_of(lamp), 1);
    }

    #[test]
    fn crafting_without_the_inputs_changes_nothing() {
        let (registry, stone, coal) = registry();
        let recipe = Recipe {
            inputs: vec![ItemStack::new(stone, 3), ItemStack::new(coal, 1)],
            output: ItemStack::new(coal, 5),
        };

        let mut inventory = Inventory::player();
        inventory.insert(stone, 3, &registry); // no coal

        assert!(!recipe.craftable_from(&inventory));
        assert!(!recipe.craft(&mut inventory, &registry));
        assert_eq!(inventory.count_of(stone), 3, "a failed craft took items");
    }

    #[test]
    fn crafting_into_a_full_inventory_refuses_before_consuming() {
        // The dupe-or-shredder hazard: consume inputs, fail to place the
        // output. The pre-check means that state cannot be reached.
        let (registry, stone, coal) = registry();
        let recipe = Recipe {
            inputs: vec![ItemStack::new(coal, 1)],
            output: ItemStack::new(stone, 1),
        };

        let mut inventory = Inventory::new(1);
        inventory.insert(coal, 64, &registry); // the only slot is full of coal

        // Inputs are present, but the output has nowhere to go... except the
        // slot the coal frees. The conservative check refuses that case.
        assert!(!recipe.craft(&mut inventory, &registry));
        assert_eq!(inventory.count_of(coal), 64, "a refused craft took items");
        assert_eq!(inventory.count_of(stone), 0, "a refused craft produced items");
    }

    #[test]
    fn repeated_crafting_conserves_totals_exactly() {
        // The dupe test: over many crafts, input consumed and output produced
        // stay in exact ratio.
        let (registry, stone, coal) = registry();
        let recipe = Recipe {
            inputs: vec![ItemStack::new(stone, 3), ItemStack::new(coal, 1)],
            output: ItemStack::new(coal, 2), // deliberately feeds back an input
        };

        let mut inventory = Inventory::player();
        inventory.insert(stone, 60, &registry);
        inventory.insert(coal, 5, &registry);

        let mut crafts = 0;
        while recipe.craft(&mut inventory, &registry) {
            crafts += 1;
            assert!(crafts <= 20, "crafting never stopped");
        }

        // Each craft: -3 stone, net +1 coal. 20 crafts empties the stone.
        assert_eq!(crafts, 20);
        assert_eq!(inventory.count_of(stone), 0);
        assert_eq!(inventory.count_of(coal), 25);
    }
}
