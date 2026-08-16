//! The engine's built-in items, what blocks drop, and what can be crafted.
//!
//! Items are data against the registries, following the same pattern as
//! blocks: when mods arrive they register into the same tables and everything
//! downstream — drops, recipes, the inventory UI — picks them up for free.

use vx_core::{BlockId, BlockRegistry, ItemDef, ItemId, ItemRegistry};

use crate::gen::TerrainBlocks;
use crate::inventory::{ItemStack, Recipe};

/// The item ids gameplay needs, resolved once.
#[derive(Debug, Clone, Copy)]
pub struct GameItems {
    pub stone: ItemId,
    pub dirt: ItemId,
    pub grass: ItemId,
    pub sand: ItemId,
    pub lamp: ItemId,
    pub coal: ItemId,
    pub raw_iron: ItemId,
    pub raw_gold: ItemId,
    pub module_speed: ItemId,
    pub module_reach: ItemId,
    pub module_fortune: ItemId,
    /// Consumed to raise the drill's tier.
    pub drill_upgrade: ItemId,
}

impl GameItems {
    /// Register the built-in items against the built-in blocks.
    pub fn register_builtins(items: &mut ItemRegistry, blocks: &TerrainBlocks) -> Self {
        let mut register = |def: ItemDef| {
            items
                .register(def)
                .expect("built-in items register exactly once into a fresh registry")
        };

        GameItems {
            // Placeable forms of the placeable blocks. Water and bedrock have
            // no item: one needs fluid handling, the other is the world floor.
            stone: register(ItemDef::for_block("engine:stone", blocks.stone)),
            dirt: register(ItemDef::for_block("engine:dirt", blocks.dirt)),
            grass: register(ItemDef::for_block("engine:grass", blocks.grass)),
            sand: register(ItemDef::for_block("engine:sand", blocks.sand)),
            lamp: register(ItemDef::for_block("engine:lamp", blocks.lamp)),
            // Materials: things you carry but cannot place.
            coal: register(ItemDef::material("engine:coal")),
            raw_iron: register(ItemDef::material("engine:raw_iron")),
            raw_gold: register(ItemDef::material("engine:raw_gold")),
            // Drill parts. Small stacks: these are milestones, not bulk goods.
            module_speed: register(
                ItemDef::material("engine:module_speed")
                    .with_display_name("Speed Mod")
                    .with_max_stack(4),
            ),
            module_reach: register(
                ItemDef::material("engine:module_reach")
                    .with_display_name("Reach Mod")
                    .with_max_stack(4),
            ),
            module_fortune: register(
                ItemDef::material("engine:module_fortune")
                    .with_display_name("Fortune Mod")
                    .with_max_stack(4),
            ),
            drill_upgrade: register(
                ItemDef::material("engine:drill_upgrade")
                    .with_display_name("Tier Kit")
                    .with_max_stack(2),
            ),
        }
    }

    /// What breaking `block` yields, or `None` for blocks that drop nothing.
    ///
    /// Ores drop their mineral rather than themselves — the block is the
    /// container, the material is the reward. Grass drops dirt, as the grass
    /// itself needs sunlight it will not have in a pocket.
    pub fn drop_for(&self, block: BlockId, blocks: &TerrainBlocks) -> Option<ItemStack> {
        let stack = |item| Some(ItemStack::new(item, 1));
        if block == blocks.stone {
            stack(self.stone)
        } else if block == blocks.dirt || block == blocks.grass {
            stack(self.dirt)
        } else if block == blocks.sand {
            stack(self.sand)
        } else if block == blocks.lamp {
            stack(self.lamp)
        } else if block == blocks.coal_ore {
            stack(self.coal)
        } else if block == blocks.iron_ore {
            stack(self.raw_iron)
        } else if block == blocks.gold_ore {
            stack(self.raw_gold)
        } else {
            None
        }
    }

    /// The built-in recipes.
    pub fn recipes(&self) -> Vec<Recipe> {
        vec![
            // The lamp is otherwise unobtainable: nothing generates it, so
            // crafting is the only road to portable light.
            Recipe {
                inputs: vec![ItemStack::new(self.stone, 3), ItemStack::new(self.coal, 1)],
                output: ItemStack::new(self.lamp, 1),
            },
            // Sand is only worth carrying near beaches; let spare dirt become
            // buildable ground anywhere.
            Recipe {
                inputs: vec![ItemStack::new(self.sand, 2), ItemStack::new(self.dirt, 2)],
                output: ItemStack::new(self.grass, 1),
            },
            // Drill progression, craftable from the ore the drill mines.
            // Vendors, dungeons and supply drops become alternative routes to
            // these same items later, not separate systems.
            Recipe {
                inputs: vec![ItemStack::new(self.raw_iron, 3), ItemStack::new(self.coal, 2)],
                output: ItemStack::new(self.module_speed, 1),
            },
            Recipe {
                inputs: vec![ItemStack::new(self.raw_iron, 2), ItemStack::new(self.stone, 8)],
                output: ItemStack::new(self.module_reach, 1),
            },
            Recipe {
                inputs: vec![ItemStack::new(self.raw_gold, 2), ItemStack::new(self.coal, 3)],
                output: ItemStack::new(self.module_fortune, 1),
            },
            Recipe {
                inputs: vec![ItemStack::new(self.raw_iron, 5), ItemStack::new(self.raw_gold, 2)],
                output: ItemStack::new(self.drill_upgrade, 1),
            },
        ]
    }
}

/// A human-readable label for a recipe, for the crafting list.
pub fn recipe_label(recipe: &Recipe, items: &ItemRegistry) -> String {
    let name = |id: ItemId| {
        items
            .get(id)
            .map_or("?", |def| def.display_name.as_str())
            .to_uppercase()
    };
    let inputs = recipe
        .inputs
        .iter()
        .map(|input| format!("{} {}", input.count, name(input.item)))
        .collect::<Vec<_>>()
        .join(" + ");
    format!("{inputs} = {} {}", recipe.output.count, name(recipe.output.item))
}

/// Register blocks and items together, for worlds and tests alike.
pub fn register_all(
    block_registry: &mut BlockRegistry,
) -> (TerrainBlocks, ItemRegistry, GameItems) {
    let blocks = TerrainBlocks::register_builtins(block_registry);
    let mut items = ItemRegistry::new();
    let game_items = GameItems::register_builtins(&mut items, &blocks);
    (blocks, items, game_items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (BlockRegistry, TerrainBlocks, ItemRegistry, GameItems) {
        let mut registry = BlockRegistry::new();
        let (blocks, items, game_items) = register_all(&mut registry);
        (registry, blocks, items, game_items)
    }

    #[test]
    fn block_items_place_the_block_they_are_named_for() {
        let (_, blocks, items, game_items) = setup();
        assert_eq!(items.block_of(game_items.stone), Some(blocks.stone));
        assert_eq!(items.block_of(game_items.lamp), Some(blocks.lamp));
        // Materials place nothing.
        assert_eq!(items.block_of(game_items.coal), None);
        assert_eq!(items.block_of(game_items.raw_iron), None);
    }

    #[test]
    fn ores_drop_their_mineral_not_themselves() {
        let (_, blocks, _, game_items) = setup();
        assert_eq!(
            game_items.drop_for(blocks.coal_ore, &blocks).unwrap().item,
            game_items.coal
        );
        assert_eq!(
            game_items.drop_for(blocks.iron_ore, &blocks).unwrap().item,
            game_items.raw_iron
        );
        assert_eq!(
            game_items.drop_for(blocks.gold_ore, &blocks).unwrap().item,
            game_items.raw_gold
        );
    }

    #[test]
    fn grass_drops_dirt_and_the_floor_drops_nothing() {
        let (_, blocks, _, game_items) = setup();
        assert_eq!(
            game_items.drop_for(blocks.grass, &blocks).unwrap().item,
            game_items.dirt
        );
        assert!(game_items.drop_for(blocks.bedrock, &blocks).is_none());
        assert!(game_items.drop_for(blocks.water, &blocks).is_none());
        assert!(game_items.drop_for(BlockId::AIR, &blocks).is_none());
    }

    #[test]
    fn every_recipe_is_labelled_and_produces_something_reachable() {
        let (_, _, items, game_items) = setup();
        let recipes = game_items.recipes();
        assert!(!recipes.is_empty());

        for recipe in &recipes {
            let label = recipe_label(recipe, &items);
            assert!(label.contains('='), "unlabelled recipe: {label}");
            assert!(!label.contains('?'), "recipe names an unknown item: {label}");
            assert!(recipe.output.count > 0);
            assert!(!recipe.inputs.is_empty(), "a recipe from nothing is free items");
        }
    }

    #[test]
    fn the_lamp_recipe_reads_as_expected() {
        let (_, _, items, game_items) = setup();
        let recipes = game_items.recipes();
        assert_eq!(recipe_label(&recipes[0], &items), "3 STONE + 1 COAL = 1 LAMP");
    }
}
