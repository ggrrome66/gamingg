//! Encoding the player to bytes and back: pose, mode, inventory, drill.
//!
//! Without this file the drill resets to tier one and the inventory empties
//! on every restart, which guts the attachment the drill exists to build —
//! progression only means something if it survives quitting.
//!
//! Items and modules are stored by **namespaced name**, never numeric id, for
//! the same reason chunks store block names: ids are assigned in registration
//! order and shift with the mod set.
//!
//! Decoding is a trust boundary, like every reader in this crate — but with a
//! different failure posture than chunks. A chunk that fails structurally is
//! regenerated from the seed; a player cannot be regenerated, so *semantic*
//! oddities are sanitised rather than fatal: a hostile stack count loads
//! clamped, an unknown item is skipped with a warning, a non-finite position
//! flags a respawn. Only structural damage — wrong magic, truncation,
//! trailing bytes — rejects the file, and the caller starts a fresh player
//! while the world itself is untouched.

use vx_core::ItemRegistry;
use vx_world::inventory::{Inventory, ItemStack};
use vx_world::{Drill, ModuleKind, MAX_MODULE_SLOTS, PLAYER_SLOTS};

use crate::cursor::{Cursor, CursorError};

const MAGIC: [u8; 4] = *b"VXPL";
pub const PLAYER_FORMAT_VERSION: u16 = 1;

/// Longest item name accepted, matching the chunk format's bound.
const MAX_NAME: usize = 256;

/// Bit in the flags byte: the player was flying.
const FLAG_FLYING: u8 = 1;

#[derive(Debug, thiserror::Error)]
pub enum PlayerFormatError {
    #[error("not a player record: bad magic")]
    BadMagic,
    #[error("player format version {found} is not supported (this build reads {supported})")]
    UnsupportedVersion { found: u16, supported: u16 },
    #[error("malformed player record: {0}")]
    Malformed(#[from] CursorError),
    #[error("{0} trailing bytes after the player record")]
    TrailingBytes(usize),
}

/// Everything about the player that outlives a session.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerRecord {
    /// Feet position, as the body defines it.
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub flying: bool,
    pub inventory: Inventory,
    pub drill: Drill,
    /// Set by the decoder when the saved pose was unusable — non-finite
    /// floats — so the caller should place the player at spawn instead.
    /// Never serialised.
    pub respawn: bool,
}

/// Serialise a player, resolving item ids to names through `items`.
pub fn encode_player(record: &PlayerRecord, items: &ItemRegistry) -> Vec<u8> {
    let mut out = Vec::with_capacity(128);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&PLAYER_FORMAT_VERSION.to_le_bytes());

    let mut flags = 0u8;
    if record.flying {
        flags |= FLAG_FLYING;
    }
    out.push(flags);

    for value in record.position {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&record.yaw.to_le_bytes());
    out.extend_from_slice(&record.pitch.to_le_bytes());

    // Drill: tier, then each fitted module as (slot, item name).
    out.push(record.drill.tier());
    let fitted: Vec<(usize, ModuleKind)> = record
        .drill
        .slots()
        .filter_map(|(slot, module, _)| module.map(|kind| (slot, kind)))
        .collect();
    out.push(fitted.len() as u8);
    for (slot, kind) in fitted {
        out.push(slot as u8);
        let name = kind.item_name().as_bytes();
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name);
    }

    // Inventory: each occupied slot as (slot, item name, count). An item the
    // registry no longer knows is skipped — it cannot be named, and a nameless
    // entry could never load.
    let occupied: Vec<(usize, ItemStack, &str)> = record
        .inventory
        .occupied()
        .filter_map(|(slot, stack)| {
            items
                .get(stack.item)
                .map(|def| (slot, stack, def.name.as_str()))
        })
        .collect();
    out.push(occupied.len() as u8);
    for (slot, stack, name) in occupied {
        out.push(slot as u8);
        let bytes = name.as_bytes();
        out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(bytes);
        out.extend_from_slice(&stack.count.to_le_bytes());
    }

    out
}

/// Rebuild a player from `bytes`, mapping names back through `items`.
pub fn decode_player(
    bytes: &[u8],
    items: &ItemRegistry,
) -> Result<PlayerRecord, PlayerFormatError> {
    let mut cursor = Cursor::new(bytes);

    if !cursor.expect_magic(&MAGIC)? {
        return Err(PlayerFormatError::BadMagic);
    }
    let version = cursor.take_u16()?;
    if version != PLAYER_FORMAT_VERSION {
        return Err(PlayerFormatError::UnsupportedVersion {
            found: version,
            supported: PLAYER_FORMAT_VERSION,
        });
    }

    let flags = cursor.take_u8()?;
    let position = [cursor.take_f32()?, cursor.take_f32()?, cursor.take_f32()?];
    let yaw = cursor.take_f32()?;
    let pitch = cursor.take_f32()?;

    // A pose the game cannot stand at flags a respawn rather than rejecting
    // the record: the inventory and drill are still perfectly recoverable,
    // and they are what the player would actually miss.
    let finite = position.iter().all(|v| v.is_finite()) && yaw.is_finite() && pitch.is_finite();

    // Drill. The count is bounded by the slot array, and `Drill::restore`
    // re-validates every fit, so a lying record degrades to a legal drill.
    let tier = cursor.take_u8()?;
    let module_count = (cursor.take_u8()? as usize).min(MAX_MODULE_SLOTS);
    let mut fitted = Vec::with_capacity(module_count);
    for _ in 0..module_count {
        let slot = cursor.take_u8()? as usize;
        let name = cursor.take_string("module name", MAX_NAME)?;
        match ModuleKind::from_name(&name) {
            Some(kind) => fitted.push((slot, kind)),
            None => log::warn!("saved drill module {name:?} is unknown; dropped"),
        }
    }
    let drill = Drill::restore(tier, &fitted);

    // Inventory. `place_stack` validates each entry — occupied, out of range,
    // zero or oversized counts — so nothing here needs trusting.
    let entry_count = (cursor.take_u8()? as usize).min(PLAYER_SLOTS);
    let mut inventory = Inventory::player();
    for _ in 0..entry_count {
        let slot = cursor.take_u8()? as usize;
        let name = cursor.take_string("item name", MAX_NAME)?;
        let count = cursor.take_u32()?;
        let Some(item) = items.id_of(&name) else {
            // A removed mod's item. The slot loads empty rather than the
            // whole record failing.
            log::warn!("saved item {name:?} is unknown to this build; dropped");
            continue;
        };
        if !inventory.place_stack(slot, ItemStack::new(item, count), items) {
            log::warn!("saved stack of {name:?} in slot {slot} was invalid; dropped");
        }
    }

    if !cursor.is_empty() {
        return Err(PlayerFormatError::TrailingBytes(cursor.remaining()));
    }

    Ok(PlayerRecord {
        position: if finite { position } else { [0.0; 3] },
        yaw: if finite { yaw } else { 0.0 },
        pitch: if finite { pitch } else { 0.0 },
        flying: flags & FLAG_FLYING != 0,
        inventory,
        drill,
        respawn: !finite,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use vx_core::BlockRegistry;
    use vx_world::items::{register_all, GameItems};

    fn setup() -> (ItemRegistry, GameItems) {
        let mut blocks = BlockRegistry::new();
        let (_, items, game_items) = register_all(&mut blocks);
        (items, game_items)
    }

    fn sample(items: &ItemRegistry, kit: &GameItems) -> PlayerRecord {
        let mut inventory = Inventory::player();
        inventory.insert(kit.stone, 40, items);
        inventory.insert(kit.coal, 7, items);
        inventory.insert(kit.module_speed, 1, items);

        let mut drill = Drill::new();
        drill.upgrade();
        drill.install(0, ModuleKind::Speed).unwrap();
        drill.install(1, ModuleKind::Fortune).unwrap();

        PlayerRecord {
            position: [12.5, 71.0, -3.5],
            yaw: 1.25,
            pitch: -0.4,
            flying: false,
            inventory,
            drill,
            respawn: false,
        }
    }

    #[test]
    fn a_player_survives_the_round_trip_exactly() {
        let (items, kit) = setup();
        let original = sample(&items, &kit);

        let decoded = decode_player(&encode_player(&original, &items), &items).unwrap();

        assert_eq!(decoded, original);
        // The parts the player would actually miss, spelled out.
        assert_eq!(decoded.drill.tier(), 2);
        assert_eq!(decoded.drill.module(0), Some(ModuleKind::Speed));
        assert_eq!(decoded.inventory.count_of(kit.stone), 40);
    }

    #[test]
    fn flying_and_walking_both_round_trip() {
        let (items, kit) = setup();
        let mut record = sample(&items, &kit);
        record.flying = true;
        let decoded = decode_player(&encode_player(&record, &items), &items).unwrap();
        assert!(decoded.flying);
    }

    #[test]
    fn a_non_finite_pose_respawns_but_keeps_the_loot() {
        // A corrupt position must not cost the player their drill and items —
        // those are recoverable, the pose is not.
        let (items, kit) = setup();
        let mut record = sample(&items, &kit);
        record.position[1] = f32::NAN;
        record.yaw = f32::INFINITY;

        let decoded = decode_player(&encode_player(&record, &items), &items).unwrap();

        assert!(decoded.respawn, "an unusable pose must flag a respawn");
        assert!(decoded.position.iter().all(|v| v.is_finite()));
        assert_eq!(decoded.drill, record.drill, "the drill was lost with the pose");
        assert_eq!(decoded.inventory.count_of(kit.stone), 40);
    }

    #[test]
    fn a_hostile_stack_count_loads_clamped() {
        // Crafted bytes claiming 4 billion stone: the entry loads at the
        // stack ceiling rather than overflowing anything downstream.
        let (items, kit) = setup();
        let mut record = sample(&items, &kit);
        let bytes = encode_player(&record, &items);

        // The count of the *last* inventory entry is the final 4 bytes.
        let mut hostile = bytes.clone();
        let end = hostile.len();
        hostile[end - 4..].copy_from_slice(&u32::MAX.to_le_bytes());

        let decoded = decode_player(&hostile, &items).unwrap();
        let ceiling = items.max_stack(kit.module_speed) as u64;
        assert_eq!(decoded.inventory.count_of(kit.module_speed), ceiling);

        // And for contrast: the legitimate record still round-trips.
        record.respawn = false;
        assert!(decode_player(&bytes, &items).is_ok());
    }

    #[test]
    fn an_unknown_item_is_dropped_and_the_rest_survives() {
        // The mod-removed case: one alien name in the record must not take
        // the whole player with it.
        let (items, kit) = setup();
        let record = sample(&items, &kit);
        let mut bytes = encode_player(&record, &items);

        // Splice an extra entry with an unknown name onto the inventory by
        // rebuilding: bump the entry count and append slot+name+count.
        let count_at = {
            // Entry count byte sits before the serialised entries; find it by
            // re-encoding an empty-inventory record and measuring.
            let mut empty = record.clone();
            empty.inventory = Inventory::player();
            encode_player(&empty, &items).len() - 1
        };
        bytes[count_at] += 1;
        bytes.push(30); // slot
        let name = b"gone_mod:widget";
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(&5u32.to_le_bytes());

        let decoded = decode_player(&bytes, &items).unwrap();
        assert_eq!(decoded.inventory.slot(30), None, "an unknown item materialised");
        assert_eq!(decoded.inventory.count_of(kit.stone), 40, "known items were lost");
    }

    #[test]
    fn a_lying_drill_loads_as_a_legal_drill() {
        let (items, kit) = setup();
        let record = sample(&items, &kit);
        let mut bytes = encode_player(&record, &items);

        // The tier byte follows magic(4) + version(2) + flags(1) + pose(20).
        bytes[27] = 200;
        let decoded = decode_player(&bytes, &items).unwrap();
        assert_eq!(decoded.drill.tier(), vx_world::MAX_TIER);
        // At max tier both saved modules still fit.
        assert_eq!(decoded.drill.module(0), Some(ModuleKind::Speed));
    }

    #[test]
    fn structural_damage_is_refused_but_never_panics() {
        let (items, kit) = setup();
        let full = encode_player(&sample(&items, &kit), &items);

        assert!(matches!(
            decode_player(b"NOPE", &items),
            Err(PlayerFormatError::Malformed(_) | PlayerFormatError::BadMagic)
        ));

        // Truncation at every offset: an error, never a panic.
        for cut in 0..full.len() {
            assert!(
                decode_player(&full[..cut], &items).is_err(),
                "a {cut}-byte prefix decoded successfully"
            );
        }

        // Trailing rubbish is refused.
        let mut padded = full.clone();
        padded.extend_from_slice(b"junk");
        assert!(matches!(
            decode_player(&padded, &items),
            Err(PlayerFormatError::TrailingBytes(4))
        ));

        // A future version is refused rather than guessed at.
        let mut future = full.clone();
        future[4..6].copy_from_slice(&(PLAYER_FORMAT_VERSION + 1).to_le_bytes());
        assert!(matches!(
            decode_player(&future, &items),
            Err(PlayerFormatError::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn every_single_byte_corruption_is_survivable() {
        // Not detected — much of the payload is position and counts, where a
        // flipped byte is just a different value — but never a panic, and
        // whatever loads still honours every invariant.
        let (items, kit) = setup();
        let full = encode_player(&sample(&items, &kit), &items);

        for at in 0..full.len() {
            let mut corrupted = full.clone();
            corrupted[at] ^= 0xff;
            if let Ok(decoded) = decode_player(&corrupted, &items) {
                assert!(decoded.drill.tier() >= 1 && decoded.drill.tier() <= vx_world::MAX_TIER);
                for (_, stack) in decoded.inventory.occupied() {
                    assert!(stack.count <= items.max_stack(stack.item));
                }
            }
        }
        // Keep the fixture honest.
        assert!(kit.stone != kit.coal);
    }
}
