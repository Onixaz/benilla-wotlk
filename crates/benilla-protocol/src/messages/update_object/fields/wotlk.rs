//! 3.3.5a's update fields read into [`ObjectFields`]' shared index space ([`layout::SHARED`]).
//! Each wire field the shared set reads lands at its shared index; a value the wire packs another
//! way is converted here; a field with no shared place is dropped.

use std::sync::OnceLock;

use super::layout::{self, Block, Field, FieldPlace, UpdateFieldLayout};
use super::ObjectFields;
use crate::messages::update_object::ObjectType;
use crate::wire::Vector3d;

/// A 3.3.5a field's element at one wire index.
type Slot = Option<(&'static FieldPlace, u16)>;

fn wotlk() -> &'static UpdateFieldLayout {
    &layout::WIRE
}

fn shared() -> &'static UpdateFieldLayout {
    &layout::SHARED
}

/// The blocks of an object's descriptor; a container's item block is the item's.
fn blocks(object_type: ObjectType) -> &'static [Block] {
    match object_type {
        ObjectType::Object => &[Block::Object],
        ObjectType::Item | ObjectType::Container => &[Block::Object, Block::Item, Block::Container],
        ObjectType::Unit => &[Block::Object, Block::Unit],
        ObjectType::Player => &[Block::Object, Block::Unit, Block::Player],
        ObjectType::GameObject => &[Block::Object, Block::GameObject],
        ObjectType::DynamicObject => &[Block::Object, Block::DynamicObject],
        ObjectType::Corpse => &[Block::Object, Block::Corpse],
    }
}

/// Each 3.3.5a wire index of `object_type`'s descriptor, the field element it holds.
fn wire_slots(object_type: ObjectType) -> &'static [Slot] {
    static TABLES: [OnceLock<Vec<Slot>>; 8] = [const { OnceLock::new() }; 8];
    let table = match object_type {
        ObjectType::Object => 0,
        ObjectType::Item => 1,
        ObjectType::Container => 2,
        ObjectType::Unit => 3,
        ObjectType::Player => 4,
        ObjectType::GameObject => 5,
        ObjectType::DynamicObject => 6,
        ObjectType::Corpse => 7,
    };
    TABLES[table].get_or_init(|| {
        let layout = wotlk();
        let mut slots: Vec<Slot> = vec![None; usize::from(layout.ends.player)];
        for place in layout
            .places
            .iter()
            .filter(|p| blocks(object_type).contains(&p.block))
        {
            for i in 0..place.count {
                let index = usize::from(place.element(i).expect("in count"));
                if index >= slots.len() {
                    slots.resize(index + 1, None);
                }
                slots[index] = Some((place, i));
            }
        }
        slots
    })
}

impl ObjectFields {
    /// `raw`, read off a 3.3.5a wire for an `object_type`, in the shared indices.
    pub(in crate::messages::update_object) fn from_wotlk(
        raw: ObjectFields,
        object_type: ObjectType,
    ) -> ObjectFields {
        let slots = wire_slots(object_type);
        let mut out = ObjectFields::default();
        for (index, value) in raw.raw_fields() {
            if let Some(Some((place, element))) = slots.get(usize::from(index)) {
                out.insert_wotlk(place.field, *element, value);
            }
        }
        out
    }

    /// Put 3.3.5a's `value` for element `element` of `field` where the shared set reads it.
    fn insert_wotlk(&mut self, field: Field, element: u16, value: u32) {
        let at =
            |field: Field, element: u16| shared().place(field).and_then(|p| p.element(element));
        let mut put = |field: Field, element: u16, value: u32| {
            if let Some(index) = at(field, element) {
                self.insert(index, value);
            }
        };
        match field {
            // Stand state stays in byte 0; 3.3.5a's visibility flags (ghost, creep, untrackable,
            // AzerothCore `UnitStandFlags`) move from byte 2 to 1.12.1's byte 3, and its anim tier
            // from byte 3 to byte 2, where 1.12.1 keeps the form 3.3.5a carries elsewhere. Pet
            // talents in byte 1 have no 1.12.1 place.
            Field::UnitBytes1 => put(
                field,
                element,
                (value & 0xff) | ((value >> 16) & 0xff) << 24 | ((value >> 24) & 0xff) << 16,
            ),
            // Sheath byte 0, flags byte 1 (`+0x1d1`), pet flags byte 2 (`+0x1d2`) and form byte 3
            // (`+0x1d3`, `0x71af70`).
            Field::UnitBytes2 => put(field, element, value),
            // Gender and drunkenness; bytes 2 and 3 are the PvP title and arena faction, where
            // 1.12.1 keeps the city protector and the honor rank.
            Field::PlayerBytes3 => put(field, element, value & 0xffff),
            // Byte 1 is the refer-a-friend level, where 1.12.1 keeps the combo points.
            Field::PlayerFieldBytes => {
                put(field, element, value & !0xff00);
                put(Field::PlayerRafGrantableLevel, 0, (value >> 8) & 0xff);
            }
            // Byte 3 is the aura vision; bytes 0-2 are the override-spells id and the regen
            // prediction mask, where 1.12.1 keeps the honor rank bar. The id rides its own field.
            Field::PlayerFieldBytes2 => {
                put(field, element, value & 0xff00_0000);
                put(Field::PlayerOverrideSpellsId, 0, value & 0xffff);
            }
            // The permanent and temporary enchant as two `u16`, which 1.12.1 sends as the first
            // two of seven dwords (AzerothCore `Player::SetVisibleItemSlot`).
            Field::PlayerVisibleItemEnchants => {
                if let Some(index) = at(field, element) {
                    self.insert(index, value & 0xffff);
                    self.insert(index + 1, value >> 16);
                }
            }
            // The dynamic flags in the low half; the high half is path progress.
            Field::GameObjectDynamic => put(Field::GameObjectDynFlags, 0, value & 0xffff),
            Field::GameObjectBytes1 => {
                put(Field::GameObjectState, 0, value & 0xff);
                put(Field::GameObjectTypeId, 0, (value >> 8) & 0xff);
                put(Field::GameObjectArtKit, 0, (value >> 16) & 0xff);
                put(Field::GameObjectAnimProgress, 0, value >> 24);
            }
            _ => put(field, element, value),
        }
    }

    /// The create fields 3.3.5a sends in the movement block instead: a gameobject's rotation and
    /// a dynamic object's position.
    pub(in crate::messages::update_object) fn place_wotlk_movement_fields(
        &mut self,
        object_type: ObjectType,
        position: Option<(Vector3d, f32)>,
        rotation: Option<[f32; 4]>,
    ) {
        let (field, values) = match (object_type, position, rotation) {
            (ObjectType::GameObject, _, Some(q)) => (Field::GameObjectRotation, q),
            (ObjectType::DynamicObject, Some((p, o)), _) => {
                (Field::DynamicObjectPosition, [p.x, p.y, p.z, o])
            }
            _ => return,
        };
        let place = shared().place(field).expect("a 1.12.1 field");
        for (i, v) in (0..).zip(values) {
            self.insert(place.element(i).expect("four elements"), v.to_bits());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::update_object::{quest_slot_state, QuestLogSlot};

    fn wire(pairs: &[(u16, u32)]) -> ObjectFields {
        ObjectFields::from_pairs(pairs)
    }

    #[test]
    fn a_unit_s_3_3_5a_fields_land_at_their_1_12_1_indices() {
        // Health 24, level 54, faction 55, display 67, target 18..19, bytes_0 23.
        let f = ObjectFields::from_wotlk(
            wire(&[
                (24, 1500),
                (54, 12),
                (55, 35),
                (67, 4_321),
                (18, 0xAA),
                (19, 0xF130_0000),
                (23, 0x0001_0101),
            ]),
            ObjectType::Unit,
        );
        assert_eq!(f.unit_health(), Some(1500));
        assert_eq!(f.unit_level(), Some(12));
        assert_eq!(f.unit_faction_template(), Some(35));
        assert_eq!(f.unit_displayid(), Some(4_321));
        assert_eq!(f.unit_target(), Some(0xF130_0000_0000_00AA));
        assert_eq!(f.unit_race(), Some(1));
        assert_eq!(f.unit_class(), Some(1));
        // 1.12.1's health index 22 is 3.3.5a's channel spell: nothing lands there by accident.
        assert_eq!(
            ObjectFields::from_wotlk(wire(&[(22, 7)]), ObjectType::Unit).unit_health(),
            None
        );
    }

    /// A death knight's runes and runic power ride `POWER6`/`POWER7` (30, 31) and
    /// `MAXPOWER6`/`MAXPOWER7` (38, 39); the rune regen floats `UNIT_END + 0x485` (1305).
    #[test]
    fn a_death_knight_s_seven_powers_and_rune_regen_all_land() {
        let f = ObjectFields::from_wotlk(
            wire(&[
                (25, 100),
                (30, 8),
                (31, 450),
                (33, 200),
                (38, 8),
                (39, 1000),
                (1305, 0.1f32.to_bits()),
                (1308, 0.2f32.to_bits()),
            ]),
            ObjectType::Player,
        );
        assert_eq!(f.unit_power(0), Some(100));
        assert_eq!(f.unit_max_power(0), Some(200));
        assert_eq!((f.unit_power(5), f.unit_max_power(5)), (Some(8), Some(8)));
        assert_eq!(
            (f.unit_power(6), f.unit_max_power(6)),
            (Some(450), Some(1000))
        );
        assert_eq!(
            (f.unit_shown_power(6), f.unit_shown_max_power(6)),
            (Some(45), Some(100)),
            "runic power divides by 10"
        );
        assert_eq!(f.unit_power(7), None);
        assert_eq!(f.player_rune_regen(0), 0.1);
        assert_eq!(f.player_rune_regen(3), 0.2);
        assert_eq!(f.player_rune_regen(1), 0.0);
        assert_eq!(f.player_rune_regen(4), 0.0);
    }

    #[test]
    fn a_player_s_appearance_and_gear_read_as_on_1_12_1() {
        let f = ObjectFields::from_wotlk(
            wire(&[
                (153, 0x0403_0201),
                (154, 0x0200_0005),
                (155, 0x0300_6401),
                // Visible item slot 4 (chest): entry, permanent 1897 and temporary 25.
                (283 + 2 * 4, 6_096),
                (284 + 2 * 4, 1_897 | 25 << 16),
                // Inventory slot 0's guid.
                (324, 0x55),
                (1170, 12_345),
            ]),
            ObjectType::Player,
        );
        assert_eq!(f.player_skin(), Some(1));
        assert_eq!(f.player_hair_color(), Some(4));
        assert_eq!(f.player_facial_hair(), Some(5));
        assert_eq!(f.player_rest_state(), Some(2));
        assert_eq!(f.player_drunk_byte(), Some(0x64));
        // The arena faction in byte 3 is not 1.12.1's honor rank.
        assert_eq!(f.player_pvp_rank(), Some(0));
        assert_eq!(f.player_visible_item_entry(4), Some(6_096));
        assert_eq!(f.player_visible_item_enchant(4, 0), Some(1_897));
        assert_eq!(f.player_visible_item_enchant(4, 1), Some(25));
        assert_eq!(f.player_inv_slot(0), Some(0x55));
        assert_eq!(f.player_money(), Some(12_345));
    }

    #[test]
    fn a_quest_log_slot_reads_from_3_3_5a_s_25_by_5_block() {
        // Slot 2 at 158 + 2*5: id, state COMPLETE, counters 5/300 then 63/0 as `u16`, a timer.
        let base = 158 + 2 * 5;
        let f = ObjectFields::from_wotlk(
            wire(&[
                (base, 783),
                (base + 1, 1),
                (base + 2, 5 | 300 << 16),
                (base + 3, 63),
                (base + 4, 4_242),
                // Slot 24, the last, and the first visible item right after the block.
                (158 + 24 * 5, 99),
                (283, 6_096),
            ]),
            ObjectType::Player,
        );
        assert_eq!(
            f.player_quest_log(2),
            Some(QuestLogSlot {
                quest_id: 783,
                counters: [5, 300, 63, 0],
                state: quest_slot_state::COMPLETE,
                timer: 4_242,
            })
        );
        assert_eq!(f.player_quest_log(24).map(|s| s.quest_id), Some(99));
        assert_eq!(f.player_visible_item_entry(0), Some(6_096));
        assert_eq!(f.player_quest_log(1), None);
    }

    #[test]
    fn unit_bytes_take_1_12_1_s_bits_and_npc_flags_stay_3_3_5a_s() {
        // Sitting (1), pet talents 3, creep (0x02) in byte 2, anim tier 1.
        let f = ObjectFields::from_wotlk(
            wire(&[(74, 0x0102_0301), (82, 0x80 | 0x1000 | 0x2000 | 0x1)]),
            ObjectType::Unit,
        );
        assert_eq!(f.unit_stand_state(), 1);
        assert_eq!(f.unit_loyalty_level(), 0);
        assert!(f.unit_is_stealthed());
        assert_eq!(f.unit_anim_tier(), 1);
        assert!(!f.unit_is_ghost_visual());
        assert_eq!(f.unit_npc_flags(), 0x80 | 0x1000 | 0x2000 | 0x1);
        // Sheathed melee, PvP and sanctuary flags, pet flags 0x3, form 0x10.
        let f = ObjectFields::from_wotlk(wire(&[(122, 0x1003_0901)]), ObjectType::Unit);
        assert_eq!(f.unit_sheath_state(), Some(1));
        assert_eq!(f.unit_byte2_flags(), 0x09);
        assert_eq!(
            f.unit_shapeshift_form(),
            0x10,
            "byte 3 is the form (`+0x1d3`)"
        );
        // Battle Stance; `UNIT_FIELD_BYTES_1` does not carry the form on 3.3.5a.
        let f = ObjectFields::from_wotlk(
            wire(&[(122, 17 << 24), (74, 0x00ff_0000)]),
            ObjectType::Unit,
        );
        assert_eq!(f.unit_shapeshift_form(), 17);
    }

    #[test]
    fn player_field_bytes2_keeps_the_aura_vision_and_drops_the_override_spells() {
        // Override spells 0x1234, regen mask 0x56, stealth and invisibility vision 0x60.
        let f = ObjectFields::from_wotlk(wire(&[(1229, 0x6056_1234)]), ObjectType::Player);
        assert_eq!(f.player_aura_vision(), Some(0x60));
        assert_eq!(f.player_honor_rank_bar(), Some(0));
        assert_eq!(f.player_override_spells_id(), Some(0x1234));
    }

    #[test]
    fn the_character_sheet_fields_land_where_the_12340_stat_verbs_read_them() {
        let f = ObjectFields::from_wotlk(
            wire(&[
                (40, 12.5f32.to_bits()),
                (47, 3.25f32.to_bits()),
                (145, 0.1f32.to_bits()),
                (1027, 12),
                (1028, 7),
                (1030, 4.5f32.to_bits()),
                (1033, 6.75f32.to_bits()),
                (1039, 95),
                (1192, 410),
                (1195, (-35i32) as u32),
                (1231, 18),
                (1255, 40),
            ]),
            ObjectType::Player,
        );
        assert_eq!(f.unit_power_regen(0, false), Some(12.5));
        assert_eq!(f.unit_power_regen(0, true), Some(3.25));
        assert_eq!(f.unit_max_health_modifier(), Some(0.1));
        assert_eq!(f.player_expertise(0), Some(12));
        assert_eq!(f.player_expertise(1), Some(7));
        assert_eq!(f.player_ranged_crit_percentage(), Some(4.5));
        assert_eq!(f.player_spell_crit_percentage(1), Some(6.75));
        assert_eq!(f.player_shield_block(), Some(95));
        assert_eq!(f.player_mod_healing_done_pos(), Some(410));
        assert_eq!(f.player_mod_target_resistance(), Some(-35));
        assert_eq!(f.player_combat_rating(0), Some(18));
        assert_eq!(f.player_combat_rating(24), Some(40));
        assert_eq!(f.player_combat_rating(25), None);
    }

    #[test]
    fn a_gameobject_s_bytes_split_into_state_type_and_its_rotation_comes_from_movement() {
        // State 1, type 19 (mailbox), art kit 0, anim progress 255; dynamic flags 0x4 under a
        // path progress.
        let mut f = ObjectFields::from_wotlk(
            wire(&[(17, 0xFF00_1301), (14, 0x1234_0004), (8, 5_000), (15, 35)]),
            ObjectType::GameObject,
        );
        f.place_wotlk_movement_fields(ObjectType::GameObject, None, Some([0.0, 0.0, 0.6, 0.8]));
        let f = f.into_created(ObjectType::GameObject);
        assert_eq!(f.gameobject_state(), Some(1));
        assert_eq!(f.gameobject_type_id(), 19);
        assert_eq!(f.gameobject_dynamic_flags(), 0x4);
        assert_eq!(f.gameobject_displayid(), Some(5_000));
        assert_eq!(f.gameobject_faction(), 35);
        assert_eq!(f.gameobject_rotation(), Some([0.0, 0.0, 0.6, 0.8]));
        // 3.3.5a's parent rotation at 10..=13 is not the spawn's rotation.
        let parent =
            ObjectFields::from_wotlk(wire(&[(13, 1.0f32.to_bits())]), ObjectType::GameObject);
        assert_eq!(parent.gameobject_rotation(), None);
    }

    #[test]
    fn a_dynamic_object_s_position_comes_from_its_movement_block() {
        let mut f = ObjectFields::from_wotlk(wire(&[(9, 10)]), ObjectType::DynamicObject);
        f.place_wotlk_movement_fields(
            ObjectType::DynamicObject,
            Some((
                Vector3d {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                },
                0.5,
            )),
            None,
        );
        assert_eq!(f.dynamicobject_spell_id(), Some(10));
        assert_eq!(f.dynamicobject_position(), Some(([1.0, 2.0, 3.0], 0.5)));
    }

    #[test]
    fn a_container_s_slots_and_a_corpse_s_owner_move_with_their_blocks() {
        let bag = ObjectFields::from_wotlk(wire(&[(64, 16), (66, 0x77)]), ObjectType::Container);
        assert_eq!(bag.container_num_slots(), Some(16));
        assert_eq!(bag.container_slot(0), Some(0x77));
        let corpse = ObjectFields::from_wotlk(
            wire(&[(6, 0x9), (10, 55), (30, 0x0001_0100)]),
            ObjectType::Corpse,
        );
        assert_eq!(corpse.corpse_owner(), Some(0x9));
        assert_eq!(corpse.corpse_display_id(), Some(55));
        assert_eq!(corpse.corpse_race(), 1);
    }

    #[test]
    fn the_shared_layout_is_the_index_space_the_1_12_1_accessors_read() {
        use super::super::*;
        // Each accessor's constant, and the indices the accessors write out, against the layout
        // 3.3.5a's fields translate into.
        let pairs: &[(Field, u16)] = &[
            (Field::ObjectType, FIELD_OBJECT_TYPE),
            (Field::ObjectEntry, 3),
            (Field::ObjectScaleX, FIELD_OBJECT_SCALE_X),
            (Field::ItemStackCount, FIELD_ITEM_STACK_COUNT),
            (Field::ItemEnchantPermTemp, FIELD_ITEM_ENCHANTMENT),
            (Field::ItemEnchantProps, FIELD_ITEM_ENCHANTMENT + 3 * 3),
            (Field::ItemTextId, 45),
            (Field::ContainerNumSlots, FIELD_CONTAINER_NUM_SLOTS),
            (Field::ContainerSlots, FIELD_CONTAINER_SLOT_1),
            (Field::UnitCharm, FIELD_UNIT_CHARM),
            (Field::UnitSummon, FIELD_UNIT_SUMMON),
            (Field::UnitCritter, FIELD_UNIT_CRITTER),
            (Field::UnitCharmedBy, FIELD_UNIT_CHARMEDBY),
            (Field::UnitSummonedBy, FIELD_UNIT_SUMMONEDBY),
            (Field::UnitCreatedBy, FIELD_UNIT_CREATEDBY),
            (Field::UnitTarget, FIELD_UNIT_TARGET),
            (Field::UnitChannelObject, FIELD_UNIT_CHANNEL_OBJECT),
            (Field::UnitHealth, FIELD_UNIT_HEALTH),
            (Field::UnitPower, FIELD_UNIT_POWER1),
            (Field::UnitMaxHealth, FIELD_UNIT_MAXHEALTH),
            (Field::UnitMaxPower, FIELD_UNIT_MAXPOWER1),
            (Field::UnitRunePower, FIELD_UNIT_POWER6),
            (Field::UnitMaxRunePower, FIELD_UNIT_MAXPOWER6),
            (Field::PlayerRuneRegen, FIELD_PLAYER_RUNE_REGEN_1),
            (Field::PlayerGlyphSlots, FIELD_PLAYER_GLYPH_SLOTS_1),
            (Field::PlayerGlyphs, FIELD_PLAYER_GLYPHS_1),
            (Field::PlayerGlyphsEnabled, FIELD_PLAYER_GLYPHS_ENABLED),
            (Field::PlayerChosenTitle, FIELD_PLAYER_CHOSEN_TITLE),
            (Field::PlayerKnownTitles, FIELD_PLAYER_KNOWN_TITLES),
            (Field::PlayerKnownCurrencies, FIELD_PLAYER_KNOWN_CURRENCIES),
            (
                Field::PlayerCurrencyTokenSlots,
                FIELD_PLAYER_CURRENCY_TOKEN_SLOT_1,
            ),
            (Field::UnitLevel, FIELD_UNIT_LEVEL),
            (Field::UnitFactionTemplate, FIELD_UNIT_FACTIONTEMPLATE),
            (Field::UnitBytes0, FIELD_UNIT_BYTES_0),
            (
                Field::UnitVirtualItemDisplay,
                FIELD_UNIT_VIRTUAL_ITEM_SLOT_DISPLAY,
            ),
            (Field::UnitFlags, FIELD_UNIT_FLAGS),
            (Field::UnitAura, FIELD_UNIT_AURA),
            (Field::UnitAuraFlags, FIELD_UNIT_AURAFLAGS),
            (Field::UnitAuraLevels, FIELD_UNIT_AURALEVELS),
            (Field::UnitAuraApplications, FIELD_UNIT_AURAAPPLICATIONS),
            (Field::UnitAuraState, FIELD_UNIT_AURASTATE),
            (Field::UnitBaseAttackTime, FIELD_UNIT_BASEATTACKTIME),
            (Field::UnitRangedAttackTime, FIELD_UNIT_RANGEDATTACKTIME),
            (Field::UnitBoundingRadius, FIELD_UNIT_BOUNDINGRADIUS),
            (Field::UnitCombatReach, FIELD_UNIT_COMBATREACH),
            (Field::UnitDisplayId, FIELD_UNIT_DISPLAYID),
            (Field::UnitNativeDisplayId, FIELD_UNIT_NATIVEDISPLAYID),
            (Field::UnitMountDisplayId, FIELD_UNIT_MOUNTDISPLAYID),
            (Field::UnitMinDamage, FIELD_UNIT_MINDAMAGE),
            (Field::UnitMaxDamage, FIELD_UNIT_MAXDAMAGE),
            (Field::UnitMinOffhandDamage, FIELD_UNIT_MINOFFHANDDAMAGE),
            (Field::UnitMaxOffhandDamage, FIELD_UNIT_MAXOFFHANDDAMAGE),
            (Field::UnitBytes1, FIELD_UNIT_BYTES_1),
            (Field::UnitPetNumber, FIELD_UNIT_PETNUMBER),
            (Field::UnitPetNameTimestamp, FIELD_UNIT_PET_NAME_TIMESTAMP),
            (Field::UnitPetExperience, FIELD_UNIT_PETEXPERIENCE),
            (Field::UnitPetNextLevelExp, FIELD_UNIT_PETNEXTLEVELEXP),
            (Field::UnitDynamicFlags, FIELD_UNIT_DYNAMIC_FLAGS),
            (Field::UnitChannelSpell, FIELD_UNIT_CHANNEL_SPELL),
            (Field::UnitFlags2, FIELD_UNIT_FLAGS_2),
            (Field::UnitCreatedBySpell, FIELD_UNIT_CREATED_BY_SPELL),
            (Field::UnitNpcFlags, FIELD_UNIT_NPC_FLAGS),
            (Field::UnitNpcEmoteState, FIELD_UNIT_NPC_EMOTESTATE),
            (Field::UnitTrainingPoints, FIELD_UNIT_TRAINING_POINTS),
            (Field::UnitStat, FIELD_UNIT_STAT0),
            (Field::UnitResistances, FIELD_UNIT_RESISTANCES),
            (Field::UnitBaseMana, FIELD_UNIT_BASE_MANA),
            (Field::UnitBaseHealth, FIELD_UNIT_BASE_HEALTH),
            (Field::UnitBytes2, FIELD_UNIT_BYTES_2),
            (Field::UnitAttackPower, FIELD_UNIT_ATTACK_POWER),
            (Field::UnitAttackPowerMods, FIELD_UNIT_ATTACK_POWER_MODS),
            (
                Field::UnitAttackPowerMultiplier,
                FIELD_UNIT_ATTACK_POWER_MULTIPLIER,
            ),
            (Field::UnitRangedAttackPower, FIELD_UNIT_RANGED_ATTACK_POWER),
            (
                Field::UnitRangedAttackPowerMods,
                FIELD_UNIT_RANGED_ATTACK_POWER_MODS,
            ),
            (
                Field::UnitRangedAttackPowerMultiplier,
                FIELD_UNIT_RANGED_ATTACK_POWER_MULTIPLIER,
            ),
            (Field::UnitMinRangedDamage, FIELD_UNIT_MINRANGEDDAMAGE),
            (Field::UnitMaxRangedDamage, FIELD_UNIT_MAXRANGEDDAMAGE),
            (Field::PosStat, FIELD_PLAYER_POSSTAT0),
            (Field::NegStat, FIELD_PLAYER_NEGSTAT0),
            (
                Field::ResistanceBuffModsPositive,
                FIELD_PLAYER_RESISTANCEBUFFMODSPOSITIVE,
            ),
            (
                Field::ResistanceBuffModsNegative,
                FIELD_PLAYER_RESISTANCEBUFFMODSNEGATIVE,
            ),
            (Field::PlayerDuelArbiter, FIELD_PLAYER_DUEL_ARBITER),
            (Field::PlayerFlags, FIELD_PLAYER_FLAGS),
            (Field::PlayerGuildId, FIELD_PLAYER_GUILDID),
            (Field::PlayerGuildRank, FIELD_PLAYER_GUILDRANK),
            (Field::PlayerBytes, FIELD_PLAYER_BYTES),
            (Field::PlayerBytes2, FIELD_PLAYER_BYTES_2),
            (Field::PlayerBytes3, FIELD_PLAYER_BYTES_3),
            (Field::PlayerDuelTeam, FIELD_PLAYER_DUEL_TEAM),
            (Field::PlayerQuestLog, FIELD_PLAYER_QUEST_LOG_1_1),
            (
                Field::PlayerVisibleItemEntry,
                FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 2,
            ),
            (
                Field::PlayerVisibleItemEnchants,
                FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 3,
            ),
            (
                Field::PlayerVisibleItemProperties,
                FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 10,
            ),
            (Field::PlayerInvSlots, FIELD_PLAYER_INV_SLOT_HEAD),
            (Field::PlayerPackSlots, FIELD_PLAYER_PACK_SLOT_1),
            (Field::PlayerBankSlots, FIELD_PLAYER_BANK_SLOT_1),
            (Field::PlayerBankBagSlots, FIELD_PLAYER_BANK_BAG_SLOT_1),
            (
                Field::PlayerVendorBuybackSlots,
                FIELD_PLAYER_VENDORBUYBACK_SLOT_1,
            ),
            (Field::PlayerKeyringSlots, FIELD_PLAYER_KEYRING_SLOT_1),
            (Field::PlayerFarsight, FIELD_PLAYER_FARSIGHT),
            (Field::PlayerComboTarget, FIELD_PLAYER_FIELD_COMBO_TARGET),
            (Field::PlayerXp, FIELD_PLAYER_XP),
            (Field::PlayerNextLevelXp, FIELD_PLAYER_NEXT_LEVEL_XP),
            (Field::PlayerSkillInfo, FIELD_PLAYER_SKILL_INFO_1_1),
            (
                Field::PlayerCharacterPoints1,
                FIELD_PLAYER_CHARACTER_POINTS1,
            ),
            (
                Field::PlayerCharacterPoints2,
                FIELD_PLAYER_CHARACTER_POINTS2,
            ),
            (Field::PlayerTrackCreatures, FIELD_PLAYER_TRACK_CREATURES),
            (Field::PlayerTrackResources, FIELD_PLAYER_TRACK_RESOURCES),
            (Field::PlayerBlockPercentage, FIELD_PLAYER_BLOCK_PERCENTAGE),
            (Field::PlayerDodgePercentage, FIELD_PLAYER_DODGE_PERCENTAGE),
            (Field::PlayerParryPercentage, FIELD_PLAYER_PARRY_PERCENTAGE),
            (Field::PlayerCritPercentage, FIELD_PLAYER_CRIT_PERCENTAGE),
            (Field::PlayerExploredZones, FIELD_PLAYER_EXPLORED_ZONES_1),
            (
                Field::PlayerRestStateExperience,
                FIELD_PLAYER_REST_STATE_EXPERIENCE,
            ),
            (Field::PlayerCoinage, FIELD_PLAYER_FIELD_COINAGE),
            (
                Field::PlayerModDamageDonePos,
                FIELD_PLAYER_MOD_DAMAGE_DONE_POS,
            ),
            (
                Field::PlayerModDamageDoneNeg,
                FIELD_PLAYER_MOD_DAMAGE_DONE_NEG,
            ),
            (
                Field::PlayerModDamageDonePct,
                FIELD_PLAYER_MOD_DAMAGE_DONE_PCT,
            ),
            (Field::PlayerFieldBytes, FIELD_PLAYER_FIELD_BYTES),
            (Field::PlayerAmmoId, FIELD_PLAYER_AMMO_ID),
            (Field::PlayerSelfResSpell, FIELD_PLAYER_SELF_RES_SPELL),
            (Field::PlayerBuybackPrice, FIELD_PLAYER_BUYBACK_PRICE_1),
            (
                Field::PlayerBuybackTimestamp,
                FIELD_PLAYER_BUYBACK_TIMESTAMP_1,
            ),
            (Field::PlayerSessionKills, FIELD_PLAYER_FIELD_SESSION_KILLS),
            (
                Field::PlayerYesterdayKills,
                FIELD_PLAYER_FIELD_YESTERDAY_KILLS,
            ),
            (
                Field::PlayerLastWeekKills,
                FIELD_PLAYER_FIELD_LAST_WEEK_KILLS,
            ),
            (
                Field::PlayerThisWeekKills,
                FIELD_PLAYER_FIELD_THIS_WEEK_KILLS,
            ),
            (
                Field::PlayerThisWeekContribution,
                FIELD_PLAYER_FIELD_THIS_WEEK_CONTRIBUTION,
            ),
            (
                Field::PlayerLifetimeHonorableKills,
                FIELD_PLAYER_FIELD_LIFETIME_HONORABLE_KILLS,
            ),
            (
                Field::PlayerLifetimeDishonorableKills,
                FIELD_PLAYER_FIELD_LIFETIME_DISHONORABLE_KILLS,
            ),
            (
                Field::PlayerYesterdayContribution,
                FIELD_PLAYER_FIELD_YESTERDAY_CONTRIBUTION,
            ),
            (
                Field::PlayerLastWeekContribution,
                FIELD_PLAYER_FIELD_LAST_WEEK_CONTRIBUTION,
            ),
            (Field::PlayerLastWeekRank, FIELD_PLAYER_FIELD_LAST_WEEK_RANK),
            (Field::PlayerFieldBytes2, FIELD_PLAYER_FIELD_BYTES2),
            (
                Field::PlayerWatchedFactionIndex,
                FIELD_PLAYER_WATCHED_FACTION_INDEX,
            ),
            (Field::GameObjectCreatedBy, FIELD_GAMEOBJECT_CREATED_BY),
            (Field::GameObjectDisplayId, FIELD_GAMEOBJECT_DISPLAYID),
            (Field::GameObjectFlags, FIELD_GAMEOBJECT_FLAGS),
            (Field::GameObjectRotation, FIELD_GAMEOBJECT_ROTATION),
            (Field::GameObjectState, FIELD_GAMEOBJECT_STATE),
            (Field::GameObjectPosition, FIELD_GAMEOBJECT_POS_X),
            (Field::GameObjectDynFlags, FIELD_GAMEOBJECT_DYN_FLAGS),
            (Field::GameObjectFaction, FIELD_GAMEOBJECT_FACTION),
            (Field::GameObjectTypeId, FIELD_GAMEOBJECT_TYPE_ID),
            (Field::GameObjectLevel, FIELD_GAMEOBJECT_LEVEL),
            (Field::DynamicObjectCaster, 6),
            (Field::DynamicObjectPosition, 11),
            (Field::CorpseOwner, 6),
            (Field::CorpseDisplayId, 12),
            (Field::CorpseItems, 13),
            (Field::CorpseDynamicFlags, FIELD_CORPSE_DYNAMIC_FLAGS),
        ];
        for &(field, index) in pairs {
            assert_eq!(
                shared().place(field).map(|p| p.index),
                Some(index),
                "{field:?}"
            );
        }
        assert_eq!(
            shared()
                .place(Field::PlayerVisibleItemEntry)
                .unwrap()
                .stride,
            12
        );
        let e = shared().ends;
        for (t, end) in [
            (ObjectType::Object, e.object),
            (ObjectType::Item, e.item),
            (ObjectType::Container, e.container),
            (ObjectType::Unit, e.unit),
            (ObjectType::Player, e.player),
            (ObjectType::GameObject, e.gameobject),
            (ObjectType::DynamicObject, e.dynamicobject),
            (ObjectType::Corpse, e.corpse),
        ] {
            assert_eq!(descriptor_len(t), end, "{t:?}");
        }
    }
}
