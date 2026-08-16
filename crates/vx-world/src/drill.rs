//! The drill: the only way to break a block, and the thing progression is
//! measured in.
//!
//! Two axes rather than one number going up. **Tier** raises the base speed
//! and unlocks module slots; **modules** are found, bought or crafted, and
//! slot into what the tier has opened. Both are visible on the deck, so an
//! upgrade is something you can point at.
//!
//! The drill is player equipment, not an inventory item. That avoids inventing
//! per-item instance data for one object — modules live here, on a plain
//! struct — and it means the drill cannot be dropped or lost.
//!
//! Every multiplier here is bounded. An unbounded speed stack is instant
//! mining, which would delete the mechanic this exists to create.

use vx_core::{ItemId, ItemRegistry};

/// Hard ceiling on module slots, independent of tier.
///
/// Tier decides how many are *unlocked*, but the array is this long whatever
/// tier says, so a progression bug cannot produce an unbounded module list.
pub const MAX_MODULE_SLOTS: usize = 4;

/// Highest tier the drill can reach.
pub const MAX_TIER: u8 = 3;

/// Ceiling on the total speed multiplier, however many modules are fitted.
///
/// Mining has to stay something you wait for. Without this, four speed
/// modules on a tier-three drill is a click again.
pub const MAX_SPEED: f32 = 12.0;

/// What a module does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModuleKind {
    /// Digs faster.
    Speed,
    /// Reaches further.
    Reach,
    /// Sometimes yields an extra drop.
    Fortune,
}

impl ModuleKind {
    pub const ALL: [ModuleKind; 3] = [ModuleKind::Speed, ModuleKind::Reach, ModuleKind::Fortune];

    /// The item that installs this module.
    pub fn item_name(self) -> &'static str {
        match self {
            ModuleKind::Speed => "engine:module_speed",
            ModuleKind::Reach => "engine:module_reach",
            ModuleKind::Fortune => "engine:module_fortune",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ModuleKind::Speed => "SPEED",
            ModuleKind::Reach => "REACH",
            ModuleKind::Fortune => "FORTUNE",
        }
    }

    /// Resolve the installing item against a registry.
    pub fn item(self, items: &ItemRegistry) -> Option<ItemId> {
        items.id_of(self.item_name())
    }

    /// Which module an item installs, if any.
    pub fn from_item(item: ItemId, items: &ItemRegistry) -> Option<ModuleKind> {
        let name = &items.get(item)?.name;
        ModuleKind::ALL
            .into_iter()
            .find(|kind| kind.item_name() == name)
    }
}

/// Why a module could not be fitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ModuleError {
    #[error("that slot does not exist on this drill")]
    NoSuchSlot,
    #[error("this drill's tier has not unlocked that slot")]
    SlotLocked,
    #[error("that slot already holds a module")]
    SlotFilled,
    #[error("that slot is empty")]
    SlotEmpty,
}

/// The player's drill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drill {
    tier: u8,
    modules: [Option<ModuleKind>; MAX_MODULE_SLOTS],
}

impl Default for Drill {
    fn default() -> Self {
        Drill::new()
    }
}

impl Drill {
    /// A fresh drill: tier one, nothing fitted. Deliberately slow.
    pub fn new() -> Self {
        Drill {
            tier: 1,
            modules: [None; MAX_MODULE_SLOTS],
        }
    }

    pub fn tier(&self) -> u8 {
        self.tier
    }

    /// Raise the tier by one, up to the ceiling. Returns whether it moved.
    pub fn upgrade(&mut self) -> bool {
        if self.tier >= MAX_TIER {
            return false;
        }
        self.tier += 1;
        true
    }

    /// How many module slots this tier has unlocked.
    ///
    /// Tier one has one, and each tier adds another, never past the array.
    pub fn unlocked_slots(&self) -> usize {
        (self.tier as usize).min(MAX_MODULE_SLOTS)
    }

    pub fn module(&self, slot: usize) -> Option<ModuleKind> {
        self.modules.get(slot).copied().flatten()
    }

    /// Every slot, unlocked or not, so the deck can draw the locked ones.
    pub fn slots(&self) -> impl Iterator<Item = (usize, Option<ModuleKind>, bool)> + '_ {
        (0..MAX_MODULE_SLOTS).map(move |slot| (slot, self.modules[slot], slot < self.unlocked_slots()))
    }

    /// How many of `kind` are fitted.
    pub fn count_of(&self, kind: ModuleKind) -> usize {
        self.modules.iter().flatten().filter(|m| **m == kind).count()
    }

    /// Fit a module. Fails without changing anything, so the caller can treat
    /// success as the sole signal to consume the item.
    pub fn install(&mut self, slot: usize, kind: ModuleKind) -> Result<(), ModuleError> {
        if slot >= MAX_MODULE_SLOTS {
            return Err(ModuleError::NoSuchSlot);
        }
        if slot >= self.unlocked_slots() {
            return Err(ModuleError::SlotLocked);
        }
        if self.modules[slot].is_some() {
            return Err(ModuleError::SlotFilled);
        }
        self.modules[slot] = Some(kind);
        Ok(())
    }

    /// Take a module back out, returning what it was.
    pub fn remove(&mut self, slot: usize) -> Result<ModuleKind, ModuleError> {
        if slot >= MAX_MODULE_SLOTS {
            return Err(ModuleError::NoSuchSlot);
        }
        self.modules[slot].take().ok_or(ModuleError::SlotEmpty)
    }

    /// Base speed from the tier alone, in hardness-units per second.
    ///
    /// Tier one is slow on purpose: a stone block at hardness 1.0 takes about
    /// two and a half seconds, which is long enough that the first speed
    /// module is felt immediately.
    pub fn base_speed(&self) -> f32 {
        match self.tier {
            0 | 1 => 0.4,
            2 => 0.9,
            _ => 1.6,
        }
    }

    /// Speed after modules, clamped.
    ///
    /// Each speed module is a flat proportional bonus rather than a
    /// multiplier stack, so the fourth is worth the same as the first and the
    /// curve stays readable.
    pub fn speed(&self) -> f32 {
        let bonus = 0.6 * self.count_of(ModuleKind::Speed) as f32;
        (self.base_speed() * (1.0 + bonus)).clamp(0.0, MAX_SPEED)
    }

    /// Reach in blocks, extended by reach modules and clamped.
    ///
    /// The clamp is not cosmetic: reach is how far a player may edit the
    /// world from, so an unbounded one is an unbounded edit radius.
    pub fn reach(&self) -> f32 {
        const BASE_REACH: f32 = 4.5;
        const MAX_REACH: f32 = 9.0;
        (BASE_REACH + 1.5 * self.count_of(ModuleKind::Reach) as f32).clamp(1.0, MAX_REACH)
    }

    /// Extra drops per broken block, from fortune modules.
    pub fn bonus_drops(&self) -> u32 {
        self.count_of(ModuleKind::Fortune) as u32
    }

    /// Seconds to break a block of `hardness` at the current speed.
    pub fn seconds_to_break(&self, hardness: f32) -> f32 {
        let speed = self.speed();
        if speed <= 0.0 {
            return f32::INFINITY;
        }
        hardness.max(0.0) / speed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::ItemDef;

    fn registry() -> ItemRegistry {
        let mut items = ItemRegistry::new();
        for kind in ModuleKind::ALL {
            items.register(ItemDef::material(kind.item_name())).unwrap();
        }
        items
    }

    #[test]
    fn a_fresh_drill_is_tier_one_and_slow() {
        let drill = Drill::new();
        assert_eq!(drill.tier(), 1);
        assert_eq!(drill.unlocked_slots(), 1);
        assert!(drill.module(0).is_none());

        // Slow enough to be worth upgrading: stone should take seconds.
        let stone = drill.seconds_to_break(1.0);
        assert!(
            (2.0..4.0).contains(&stone),
            "a tier-one drill breaks stone in {stone}s"
        );
    }

    #[test]
    fn tiers_raise_speed_and_unlock_slots_up_to_the_ceiling() {
        let mut drill = Drill::new();
        let mut speeds = vec![drill.speed()];
        let mut slots = vec![drill.unlocked_slots()];

        while drill.upgrade() {
            speeds.push(drill.speed());
            slots.push(drill.unlocked_slots());
        }

        assert_eq!(drill.tier(), MAX_TIER);
        assert!(!drill.upgrade(), "upgraded past the ceiling");
        assert!(
            speeds.windows(2).all(|pair| pair[1] > pair[0]),
            "speed did not rise with tier: {speeds:?}"
        );
        assert!(slots.windows(2).all(|pair| pair[1] >= pair[0]));
        assert!(slots.iter().all(|count| *count <= MAX_MODULE_SLOTS));
    }

    #[test]
    fn a_speed_module_measurably_shortens_a_dig() {
        let mut drill = Drill::new();
        let before = drill.seconds_to_break(1.0);

        drill.install(0, ModuleKind::Speed).unwrap();
        let after = drill.seconds_to_break(1.0);

        assert!(after < before, "{before}s then {after}s");
    }

    #[test]
    fn a_full_load_of_speed_modules_does_not_make_mining_instant() {
        // The mechanic this file exists to create is waiting for a block.
        let mut drill = Drill::new();
        while drill.upgrade() {}
        for slot in 0..drill.unlocked_slots() {
            drill.install(slot, ModuleKind::Speed).unwrap();
        }

        assert!(drill.speed() <= MAX_SPEED);
        assert!(
            drill.seconds_to_break(1.0) > 0.05,
            "a fully loaded drill breaks stone in {}s",
            drill.seconds_to_break(1.0)
        );
    }

    #[test]
    fn locked_slots_refuse_modules() {
        let mut drill = Drill::new(); // tier 1, one slot
        assert_eq!(drill.install(1, ModuleKind::Speed), Err(ModuleError::SlotLocked));
        assert!(drill.module(1).is_none());

        drill.upgrade();
        assert!(drill.install(1, ModuleKind::Speed).is_ok());
    }

    #[test]
    fn a_slot_holds_one_module_and_gives_it_back() {
        let mut drill = Drill::new();
        drill.install(0, ModuleKind::Fortune).unwrap();

        assert_eq!(
            drill.install(0, ModuleKind::Speed),
            Err(ModuleError::SlotFilled),
            "a filled slot accepted a second module"
        );
        assert_eq!(drill.module(0), Some(ModuleKind::Fortune), "the slot changed");

        assert_eq!(drill.remove(0), Ok(ModuleKind::Fortune));
        assert_eq!(drill.remove(0), Err(ModuleError::SlotEmpty));
        assert!(drill.module(0).is_none());
    }

    #[test]
    fn slots_past_the_array_are_refused_rather_than_panicking() {
        let mut drill = Drill::new();
        assert_eq!(drill.install(99, ModuleKind::Speed), Err(ModuleError::NoSuchSlot));
        assert_eq!(drill.remove(99), Err(ModuleError::NoSuchSlot));
        assert_eq!(drill.module(99), None);
    }

    #[test]
    fn reach_grows_with_modules_but_is_bounded() {
        // Reach is the radius a player may edit the world from, so an
        // unbounded one is an unbounded edit radius.
        let mut drill = Drill::new();
        let base = drill.reach();
        while drill.upgrade() {}
        for slot in 0..drill.unlocked_slots() {
            drill.install(slot, ModuleKind::Reach).unwrap();
        }

        assert!(drill.reach() > base);
        assert!(drill.reach() <= 9.0, "reach reached {}", drill.reach());
    }

    #[test]
    fn modules_map_to_their_items_and_back() {
        let items = registry();
        for kind in ModuleKind::ALL {
            let item = kind.item(&items).expect("module item is registered");
            assert_eq!(ModuleKind::from_item(item, &items), Some(kind));
        }

        // Something that is not a module resolves to nothing.
        let mut other = items.clone();
        let coal = other.register(ItemDef::material("engine:coal")).unwrap();
        assert_eq!(ModuleKind::from_item(coal, &other), None);
    }

    #[test]
    fn every_slot_is_reported_with_its_locked_state() {
        let drill = Drill::new();
        let slots: Vec<_> = drill.slots().collect();

        assert_eq!(slots.len(), MAX_MODULE_SLOTS, "the deck must see every slot");
        assert!(slots[0].2, "the first slot is unlocked at tier one");
        assert!(!slots[MAX_MODULE_SLOTS - 1].2, "the last slot starts locked");
    }

    #[test]
    fn harder_blocks_take_longer_at_any_configuration() {
        let mut drill = Drill::new();
        for _ in 0..2 {
            assert!(
                drill.seconds_to_break(3.0) > drill.seconds_to_break(1.0),
                "hardness is not being read at tier {}",
                drill.tier()
            );
            drill.upgrade();
        }
    }
}
