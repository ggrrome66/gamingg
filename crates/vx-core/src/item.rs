//! Item definitions and the registry that owns them.
//!
//! Items are what sits in inventories: the placeable form of a block, or a
//! material that exists only as a thing you carry — coal, raw metal. The
//! registry mirrors [`crate::block::BlockRegistry`] deliberately: dense ids
//! assigned in registration order, **unstable across runs**, so anything that
//! reaches disk must be keyed by the namespaced name.

use std::collections::HashMap;

use crate::block::BlockId;

/// A dense runtime handle for an item type. Not stable across runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u16);

impl ItemId {
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// What an item is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemDef {
    /// Namespaced unique name, e.g. `engine:coal`.
    pub name: String,
    /// Shown to players.
    pub display_name: String,
    /// The block this item places, for block items. `None` for materials.
    pub block: Option<BlockId>,
    /// Most of this item one inventory slot may hold.
    pub max_stack: u32,
}

impl ItemDef {
    /// A carryable material with no block form.
    pub fn material(name: impl Into<String>) -> Self {
        let name = name.into();
        let display_name = crate::block::default_display_name(&name);
        ItemDef {
            name,
            display_name,
            block: None,
            max_stack: 64,
        }
    }

    /// The placeable form of a block.
    pub fn for_block(name: impl Into<String>, block: BlockId) -> Self {
        let mut def = ItemDef::material(name);
        def.block = Some(block);
        def
    }

    pub fn with_display_name(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = display_name.into();
        self
    }

    pub fn with_max_stack(mut self, max_stack: u32) -> Self {
        // A zero max stack would make the item impossible to hold at all.
        self.max_stack = max_stack.max(1);
        self
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ItemRegistryError {
    #[error("item '{0}' is already registered")]
    DuplicateName(String),
    #[error("item registry is full (limit {0} items)")]
    Full(usize),
}

/// Owns every known [`ItemDef`] and maps names to ids.
#[derive(Debug, Clone, Default)]
pub struct ItemRegistry {
    defs: Vec<ItemDef>,
    by_name: HashMap<String, ItemId>,
}

impl ItemRegistry {
    /// Maximum registrable items, bounded by `ItemId`'s u16.
    pub const CAPACITY: usize = u16::MAX as usize + 1;

    pub fn new() -> Self {
        Self::default()
    }

    /// Append an item, returning its freshly assigned id.
    pub fn register(&mut self, def: ItemDef) -> Result<ItemId, ItemRegistryError> {
        if self.by_name.contains_key(&def.name) {
            return Err(ItemRegistryError::DuplicateName(def.name));
        }
        if self.defs.len() >= Self::CAPACITY {
            return Err(ItemRegistryError::Full(Self::CAPACITY));
        }
        let id = ItemId(self.defs.len() as u16);
        self.by_name.insert(def.name.clone(), id);
        self.defs.push(def);
        Ok(id)
    }

    pub fn get(&self, id: ItemId) -> Option<&ItemDef> {
        self.defs.get(id.index())
    }

    pub fn id_of(&self, name: &str) -> Option<ItemId> {
        self.by_name.get(name).copied()
    }

    /// The block an item places, or `None` for materials and unknown ids.
    pub fn block_of(&self, id: ItemId) -> Option<BlockId> {
        self.get(id).and_then(|def| def.block)
    }

    /// Stack ceiling for an item. Unknown ids stack to 1, so a stale id
    /// cannot be piled into an oversized stack.
    pub fn max_stack(&self, id: ItemId) -> u32 {
        self.get(id).map_or(1, |def| def.max_stack)
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (ItemId, &ItemDef)> {
        self.defs
            .iter()
            .enumerate()
            .map(|(i, def)| (ItemId(i as u16), def))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_register_and_resolve_by_name() {
        let mut registry = ItemRegistry::new();
        let coal = registry.register(ItemDef::material("engine:coal")).unwrap();
        let stone = registry
            .register(ItemDef::for_block("engine:stone", BlockId(1)))
            .unwrap();

        assert_eq!(registry.id_of("engine:coal"), Some(coal));
        assert_eq!(registry.get(coal).unwrap().display_name, "Coal");
        assert_eq!(registry.block_of(coal), None, "a material places nothing");
        assert_eq!(registry.block_of(stone), Some(BlockId(1)));
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn duplicate_names_are_refused() {
        let mut registry = ItemRegistry::new();
        registry.register(ItemDef::material("engine:coal")).unwrap();
        assert_eq!(
            registry.register(ItemDef::material("engine:coal")),
            Err(ItemRegistryError::DuplicateName("engine:coal".into()))
        );
    }

    #[test]
    fn unknown_ids_resolve_to_nothing_and_stack_to_one() {
        let registry = ItemRegistry::new();
        let stale = ItemId(999);
        assert!(registry.get(stale).is_none());
        assert_eq!(registry.block_of(stale), None);
        // Not 64: an id the registry does not know must not be pile-able.
        assert_eq!(registry.max_stack(stale), 1);
    }

    #[test]
    fn a_zero_max_stack_is_bumped_to_one() {
        // Zero would make the item impossible to hold and divide-by-zero
        // adjacent in stacking arithmetic.
        let def = ItemDef::material("engine:oddity").with_max_stack(0);
        assert_eq!(def.max_stack, 1);
    }
}
