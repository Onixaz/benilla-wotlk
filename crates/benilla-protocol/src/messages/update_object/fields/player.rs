//! Named accessors over [`ObjectFields`] for the item, container, player and GameObject blocks.

use super::*;

impl ObjectFields {
    /// `OBJECT_FIELD_ENTRY`: the template entry. The only route to a pet's template: its guid
    /// holds a pet number instead, but the server writes the entry here.
    pub fn object_entry(&self) -> Option<u32> {
        self.get_u32(FIELD_OBJECT_ENTRY)
    }
    /// `OBJECT_FIELD_GUID`: the object's own guid, `None` until it streams or when zero.
    pub fn object_guid(&self) -> Option<u64> {
        self.get_guid(FIELD_OBJECT_GUID).filter(|&g| g != 0)
    }
    /// `ITEM_FIELD_STACK_COUNT`: an item object's stack size.
    pub fn item_stack_count(&self) -> Option<u32> {
        self.get_u32(FIELD_ITEM_STACK_COUNT)
    }
    /// `ITEM_FIELD_SPELL_CHARGES + i` (the client's `[item+0x114]+0x28`): charges left on spell
    /// `i`, signed; negative is consumed when empty, 0 is spent.
    pub fn item_spell_charges(&self, i: u8) -> Option<i32> {
        (i < 5)
            .then(|| self.get_u32(FIELD_ITEM_SPELL_CHARGES + u16::from(i)))?
            .map(|v| v as i32)
    }
    /// `ITEM_FIELD_DURATION`: a timed item's remaining seconds, 0 for none.
    pub fn item_duration(&self) -> Option<u32> {
        self.get_u32(FIELD_ITEM_DURATION)
    }
    /// `ITEM_FIELD_FLAGS`: `0x08` wrapped, never alerts; `0x10` forces red status 4.
    pub fn item_flags(&self) -> Option<u32> {
        self.get_u32(FIELD_ITEM_FLAGS)
    }
    /// `ITEM_FIELD_OWNER`.
    pub fn item_owner(&self) -> Option<u64> {
        self.get_guid(FIELD_ITEM_OWNER).filter(|&g| g != 0)
    }
    /// `ITEM_FIELD_CONTAINED`: the bag or player whose slots hold the item.
    pub fn item_contained(&self) -> Option<u64> {
        self.get_guid(FIELD_ITEM_CONTAINED).filter(|&g| g != 0)
    }
    /// `ITEM_FIELD_CREATE_PLAYED_TIME`: the owner's played seconds when a bound item's trade window
    /// opened, read with `ITEM_FIELD_FLAGS & 0x100` (`0x708b40`).
    pub fn item_create_played_time(&self) -> Option<u32> {
        self.get_u32(FIELD_ITEM_CREATE_PLAYED_TIME)
    }
    /// `ITEM_FIELD_CREATOR`: crafter, or letter sender (`HandleMailCreateTextItem`).
    pub fn item_creator(&self) -> Option<u64> {
        self.get_guid(FIELD_ITEM_CREATOR).filter(|&g| g != 0)
    }
    /// `ITEM_FIELD_FLAGS & 0x200`: the item carries its own text, a copied mail letter's, which a
    /// right-click reads by `CMSG_ITEM_TEXT_QUERY` on the item's guid (`0x58a2bc`).
    pub fn item_has_text(&self) -> bool {
        self.item_flags().is_some_and(|f| f & 0x200 != 0)
    }
    /// `ITEM_FIELD_ENCHANTMENT + 3*slot`: the enchant id in one of 12 slots (0 permanent, 1
    /// temporary, 2-4 sockets, 5 socket bonus, 6 prismatic, 7-11 random properties), each an id,
    /// duration and charges. Signed: the tooltip looks up `abs(id)` and paints a negative one red.
    pub fn item_enchant(&self, slot: u8) -> Option<i32> {
        self.get_u32(item_enchant_index(slot)?)
            .map(|id| id as i32)
            .filter(|&id| id != 0)
    }
    /// `ITEM_FIELD_ENCHANTMENT + 3*slot + 2`: charges left, shown as `" (N Charges)"` if nonzero.
    /// The tooltip skips the duration; a countdown comes from `SMSG_ITEM_ENCHANT_TIME_UPDATE`.
    pub fn item_enchant_charges(&self, slot: u8) -> u32 {
        item_enchant_index(slot)
            .and_then(|i| self.get_u32(i + 2))
            .unwrap_or(0)
    }
    /// `ITEM_FIELD_RANDOM_PROPERTIES_ID`: the `ItemRandomProperties.dbc` suffix, 0 if
    /// none; only the name uses it, the roll's enchants fill enchant slots 7-11.
    pub fn item_random_properties_id(&self) -> u32 {
        self.get_u32(FIELD_ITEM_RANDOM_PROPERTIES_ID).unwrap_or(0)
    }
    /// `ITEM_FIELD_PROPERTY_SEED`: a random suffix's points, the link's seed.
    pub fn item_property_seed(&self) -> u32 {
        self.get_u32(FIELD_ITEM_PROPERTY_SEED).unwrap_or(0)
    }
    /// `ITEM_FIELD_DURABILITY`: current durability.
    pub fn item_durability(&self) -> Option<u32> {
        self.get_u32(FIELD_ITEM_DURABILITY)
    }
    /// `ITEM_FIELD_MAXDURABILITY`: the durability ceiling, 0 for an item without durability.
    pub fn item_max_durability(&self) -> Option<u32> {
        self.get_u32(FIELD_ITEM_MAXDURABILITY)
    }
    /// `CONTAINER_FIELD_NUM_SLOTS`: a bag object's capacity.
    pub fn container_num_slots(&self) -> Option<u32> {
        self.get_u32(FIELD_CONTAINER_NUM_SLOTS)
    }
    /// `CONTAINER_FIELD_SLOT_1 + 2i`: the item guid in bag slot `i`, `Some(0)` when empty.
    pub fn container_slot(&self, i: u8) -> Option<u64> {
        (i < 36).then(|| self.get_guid(FIELD_CONTAINER_SLOT_1 + 2 * u16::from(i)))?
    }
    /// `PLAYER_VISIBLE_ITEM_<i+1>_ENTRYID`: the public entry worn in equipment slot `i`, which
    /// other clients render from.
    pub fn player_visible_item_entry(&self, i: u8) -> Option<u32> {
        (i < 19)
            .then(|| {
                self.get_u32(field::player_visible_item_entry_field(i))
                    .filter(|&e| e != 0)
            })
            .flatten()
    }
    /// `PLAYER_VISIBLE_ITEM_<i+1>_ENCHANTMENT`'s `u16` half `j`: 0 the permanent enchant, 1 the
    /// temporary one, the only two the slot broadcasts. The reference's enchant visuals for every
    /// unit, itself included, come from these.
    pub fn player_visible_item_enchant(&self, i: u8, j: u8) -> Option<u32> {
        (i < 19 && j < 2)
            .then(|| {
                self.get_u16_pair(field::player_visible_item_entry_field(i) + 1)
                    .map(|(perm, temp)| u32::from(if j == 0 { perm } else { temp }))
                    .filter(|&e| e != 0)
            })
            .flatten()
    }
    /// `PLAYER_FIELD_INV_SLOT_HEAD + 2i`: our equipment (0-18) and equipped-bag (19-22) guids.
    pub fn player_inv_slot(&self, i: u8) -> Option<u64> {
        (i < 23).then(|| self.get_guid(FIELD_PLAYER_INV_SLOT_HEAD + 2 * u16::from(i)))?
    }
    /// `PLAYER_FIELD_PACK_SLOT_1 + 2i`: our backpack's 16 item guids.
    pub fn player_pack_slot(&self, i: u8) -> Option<u64> {
        (i < 16).then(|| self.get_guid(FIELD_PLAYER_PACK_SLOT_1 + 2 * u16::from(i)))?
    }
    /// `PLAYER_FIELD_BANK_SLOT_1 + 2i`: our bank's 28 item guids, player slots 39-66.
    pub fn player_bank_slot(&self, i: u8) -> Option<u64> {
        (i < crate::messages::BANK_SLOTS)
            .then(|| self.get_guid(FIELD_PLAYER_BANK_SLOT_1 + 2 * u16::from(i)))?
    }
    /// `PLAYER_FIELD_BANK_BAG_SLOT_1 + 2i`: bank bag `i`'s guid; its contents stream on the bag
    /// item's own `CONTAINER_FIELD_SLOT_*`, addressed as bag 67-73.
    pub fn player_bank_bag_slot(&self, i: u8) -> Option<u64> {
        (i < crate::messages::BANK_BAGS)
            .then(|| self.get_guid(FIELD_PLAYER_BANK_BAG_SLOT_1 + 2 * u16::from(i)))?
    }
    /// `PLAYER_FIELD_KEYRING_SLOT_1 + 2i`: the keyring's 32 guids, player slots 86-117.
    pub fn player_keyring_slot(&self, i: u8) -> Option<u64> {
        (i < crate::messages::KEYRING_SLOTS)
            .then(|| self.get_guid(FIELD_PLAYER_KEYRING_SLOT_1 + 2 * u16::from(i)))?
    }
    /// `PLAYER_FIELD_VENDORBUYBACK_SLOT_1 + 2i`: buyback slot `i`, player slot `74 + i`.
    pub fn player_buyback_slot(&self, i: u8) -> Option<u64> {
        (i < 12).then(|| self.get_guid(FIELD_PLAYER_VENDORBUYBACK_SLOT_1 + 2 * u16::from(i)))?
    }
    /// Player slot `slot`'s item guid (0-149), whichever slot array holds it.
    pub fn player_slot(&self, slot: u8) -> Option<u64> {
        match slot {
            0..=22 => self.player_inv_slot(slot),
            23..=38 => self.player_pack_slot(slot - 23),
            39..=66 => self.player_bank_slot(slot - 39),
            67..=73 => self.player_bank_bag_slot(slot - 67),
            74..=85 => self.player_buyback_slot(slot - 74),
            86..=117 => self.player_keyring_slot(slot - 86),
            _ => self.player_currency_token_slot(slot.checked_sub(118)?),
        }
    }
    /// `PLAYER_FIELD_BUYBACK_PRICE_1 + i`: what buying slot `i` back costs, in copper.
    pub fn player_buyback_price(&self, i: u8) -> Option<u32> {
        (i < 12).then(|| self.get_u32(FIELD_PLAYER_BUYBACK_PRICE_1 + u16::from(i)))?
    }
    /// `PLAYER_FIELD_BUYBACK_TIMESTAMP_1 + i`: expiry, used only to sort oldest first.
    pub fn player_buyback_timestamp(&self, i: u8) -> Option<u32> {
        (i < 12).then(|| self.get_u32(FIELD_PLAYER_BUYBACK_TIMESTAMP_1 + u16::from(i)))?
    }
    /// `PLAYER_FIELD_COINAGE`: our money, in copper.
    pub fn player_money(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_FIELD_COINAGE)
    }
    /// `PLAYER_FARSIGHT` (private): the view's anchor during Mind Vision and the like, or 0.
    pub fn player_farsight(&self) -> Option<u64> {
        self.get_guid(FIELD_PLAYER_FARSIGHT).filter(|&g| g != 0)
    }
    /// `PLAYER_XP` (private): experience within the current level, `UnitXP("player")`.
    pub fn player_xp(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_XP)
    }
    /// `PLAYER_NEXT_LEVEL_XP` (private): experience to the next level, `UnitXPMax("player")`.
    pub fn player_next_level_xp(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_NEXT_LEVEL_XP)
    }
    /// `PLAYER_FIELD_WATCHED_FACTION_INDEX` (private): the reputation slot on the main menu bar,
    /// signed: none is `-1`, since slot 0 is the Bloodsail Buccaneers.
    pub fn player_watched_faction(&self) -> Option<i32> {
        self.get_u32(FIELD_PLAYER_WATCHED_FACTION_INDEX)
            .map(|v| v as i32)
    }
    /// `PLAYER_CHARACTER_POINTS1` (private): unspent talent points, first of `UnitCharacterPoints`.
    pub fn player_talent_points(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_CHARACTER_POINTS1)
    }
    /// `PLAYER_CHARACTER_POINTS2` (private): free professions, second of `UnitCharacterPoints`.
    pub fn player_free_professions(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_CHARACTER_POINTS2)
    }
    /// `PLAYER_TRACK_CREATURES` (private): bit `1 << (creatureType - 1)` per creature tracking
    /// aura, the mask the minimap's red dots test.
    pub fn player_track_creatures(&self) -> u32 {
        self.get_u32(FIELD_PLAYER_TRACK_CREATURES).unwrap_or(0)
    }
    /// `PLAYER_TRACK_RESOURCES` (private): bit `1 << (lockType - 1)` per resource tracking aura
    /// (`LockType.dbc`), the mask the minimap's gold dots test.
    pub fn player_track_resources(&self) -> u32 {
        self.get_u32(FIELD_PLAYER_TRACK_RESOURCES).unwrap_or(0)
    }
    /// `PLAYER_FIELD_BYTES` byte 0 bit `0x2` (private): set by aura type 151, track stealthed;
    /// every creep-flagged unit then gets a minimap dot (the `+0x1028` read).
    pub fn player_track_stealthed(&self) -> bool {
        self.get_u32(FIELD_PLAYER_FIELD_BYTES).unwrap_or(0) & 0x2 != 0
    }
    /// Each `PLAYER_QUEST_LOG` slot's quest id, 0 for an empty slot or a unit with no player
    /// block, which `IsUnitOnQuest` (`0x5e4070`) scans.
    pub fn player_quest_ids(&self) -> [u32; PLAYER_QUEST_LOG_SLOTS as usize] {
        std::array::from_fn(|slot| {
            self.get_u32(FIELD_PLAYER_QUEST_LOG_1_1 + QUEST_LOG_SLOT_FIELDS * slot as u16)
                .unwrap_or(0)
        })
    }
    /// A fingerprint of the whole `PLAYER_QUEST_LOG` window, ids, states, counters and timers,
    /// which moves when any of its fields does.
    pub fn player_quest_log_stamp(&self) -> u64 {
        (0..QUEST_LOG_SLOT_FIELDS * u16::from(PLAYER_QUEST_LOG_SLOTS)).fold(
            0xcbf2_9ce4_8422_2325,
            |h, i| {
                let v = self.get_u32(FIELD_PLAYER_QUEST_LOG_1_1 + i).unwrap_or(0);
                (h ^ u64::from(v)).wrapping_mul(0x0100_0000_01b3)
            },
        )
    }
    /// `PLAYER_QUEST_LOG_<slot+1>_*`: one quest-log slot, `quest_id == 0` when the server cleared
    /// it; the state, counters and timer are written with the id.
    pub fn player_quest_log(&self, slot: u8) -> Option<QuestLogSlot> {
        if slot >= PLAYER_QUEST_LOG_SLOTS {
            return None;
        }
        let base = FIELD_PLAYER_QUEST_LOG_1_1 + QUEST_LOG_SLOT_FIELDS * u16::from(slot);
        let quest_id = self.get_u32(base)?;
        let state = self.get_u32(base + 1).unwrap_or(0);
        let counts = [
            self.get_u32(base + 2).unwrap_or(0),
            self.get_u32(base + 3).unwrap_or(0),
        ];
        let timer = self.get_u32(base + 4).unwrap_or(0);
        let mut counters = [0u16; 4];
        for (i, c) in counters.iter_mut().enumerate() {
            *c = (counts[i / 2] >> (16 * (i % 2))) as u16;
        }
        Some(QuestLogSlot {
            quest_id,
            counters,
            state: state as u8,
            timer,
        })
    }
    /// `PLAYER_FIELD_DAILY_QUESTS_1..25`, the day's completed daily quest ids, 0 for a free slot.
    pub fn player_daily_quests(&self) -> impl Iterator<Item = u32> + '_ {
        (0..25).map(|i| self.get_u32(FIELD_PLAYER_DAILY_QUESTS_1 + i).unwrap_or(0))
    }
    /// How many of the 25 daily-quest fields hold a quest: `GetDailyQuestsCompleted` (`0x58db30`).
    pub fn player_daily_quests_completed(&self) -> u32 {
        (0..25)
            .filter(|&i| self.get_u32(FIELD_PLAYER_DAILY_QUESTS_1 + i).unwrap_or(0) != 0)
            .count() as u32
    }
    /// `PLAYER_FIELD_MAX_LEVEL`: the level cap of the account's expansion.
    pub fn player_max_level(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_MAX_LEVEL)
    }
    /// `PLAYER_EXPLORED_ZONES_1 + i`: bitset dword `i`; bit `n` is `AreaTable` explore flag `n`.
    pub fn player_explored_zone_slot(&self, i: u16) -> u32 {
        if i >= PLAYER_EXPLORED_ZONES_SLOTS {
            return 0;
        }
        self.get_u32(FIELD_PLAYER_EXPLORED_ZONES_1 + i).unwrap_or(0)
    }
    /// `PLAYER_BLOCK_PERCENTAGE`: block chance, already a percent (2.62 means 2.62%).
    pub fn player_block_percentage(&self) -> Option<f32> {
        self.get_f32(FIELD_PLAYER_BLOCK_PERCENTAGE)
    }
    /// `PLAYER_DODGE_PERCENTAGE`: dodge chance, already a percent.
    pub fn player_dodge_percentage(&self) -> Option<f32> {
        self.get_f32(FIELD_PLAYER_DODGE_PERCENTAGE)
    }
    /// `PLAYER_PARRY_PERCENTAGE`: parry chance, already a percent.
    pub fn player_parry_percentage(&self) -> Option<f32> {
        self.get_f32(FIELD_PLAYER_PARRY_PERCENTAGE)
    }
    /// `PLAYER_CRIT_PERCENTAGE`: melee crit chance, already a percent.
    pub fn player_crit_percentage(&self) -> Option<f32> {
        self.get_f32(FIELD_PLAYER_CRIT_PERCENTAGE)
    }
    /// `PLAYER_RANGED_CRIT_PERCENTAGE`: ranged crit chance, already a percent.
    pub fn player_ranged_crit_percentage(&self) -> Option<f32> {
        self.get_f32(FIELD_PLAYER_RANGED_CRIT_PERCENTAGE)
    }
    /// `PLAYER_SPELL_CRIT_PERCENTAGE1 + school`: spell crit chance, already a percent.
    pub fn player_spell_crit_percentage(&self, school: u8) -> Option<f32> {
        (school < 7)
            .then(|| self.get_f32(FIELD_PLAYER_SPELL_CRIT_PERCENTAGE1 + u16::from(school)))?
    }
    /// `PLAYER_EXPERTISE` (`hand` 0) and `PLAYER_OFFHAND_EXPERTISE` (1).
    pub fn player_expertise(&self, hand: u8) -> Option<u32> {
        (hand < 2).then(|| self.get_u32(FIELD_PLAYER_EXPERTISE + u16::from(hand)))?
    }
    /// `PLAYER_SHIELD_BLOCK`: the shield's block value.
    pub fn player_shield_block(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_SHIELD_BLOCK)
    }
    /// `PLAYER_FIELD_MOD_HEALING_DONE_POS`: the healing bonus.
    pub fn player_mod_healing_done_pos(&self) -> Option<i32> {
        self.get_i32(FIELD_PLAYER_MOD_HEALING_DONE_POS)
    }
    /// `PLAYER_FIELD_MOD_HEALING_PCT`: the healing-taken multiplier.
    pub fn player_mod_healing_pct(&self) -> Option<f32> {
        self.get_f32(FIELD_PLAYER_MOD_HEALING_PCT)
    }
    /// `PLAYER_FIELD_MOD_HEALING_DONE_PCT`: the healing-done multiplier.
    pub fn player_mod_healing_done_pct(&self) -> Option<f32> {
        self.get_f32(FIELD_PLAYER_MOD_HEALING_DONE_PCT)
    }
    /// `PLAYER_FIELD_MOD_TARGET_RESISTANCE`: spell penetration as the resistance it removes, ≤ 0.
    pub fn player_mod_target_resistance(&self) -> Option<i32> {
        self.get_i32(FIELD_PLAYER_MOD_TARGET_RESISTANCE)
    }
    /// `PLAYER_PET_SPELL_POWER`: the pet's spell damage bonus, `GetPetSpellBonusDamage`'s answer.
    pub fn player_pet_spell_power(&self) -> i32 {
        self.get_i32(FIELD_PLAYER_PET_SPELL_POWER).unwrap_or(0)
    }
    /// `PLAYER_FIELD_BYTES` byte 1: the Refer-a-Friend levels left to grant (`0x6deb08`).
    pub fn player_raf_grantable_level(&self) -> u8 {
        (self.get_u32(FIELD_PLAYER_FIELD_BYTES).unwrap_or(0) >> 8) as u8
    }
    /// `PLAYER_FIELD_KILLS` (private): today's honorable kills, the low half.
    pub fn player_kills_today(&self) -> Option<u16> {
        self.get_u32(FIELD_PLAYER_FIELD_KILLS).map(|v| v as u16)
    }
    /// `PLAYER_FIELD_KILLS`: yesterday's honorable kills, the high half.
    pub fn player_kills_yesterday(&self) -> Option<u16> {
        self.get_u32(FIELD_PLAYER_FIELD_KILLS)
            .map(|v| (v >> 16) as u16)
    }
    /// `PLAYER_FIELD_TODAY_CONTRIBUTION` (private): honor earned today.
    pub fn player_today_contribution(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_FIELD_TODAY_CONTRIBUTION)
    }
    /// `PLAYER_FIELD_ARENA_TEAM_INFO_1_1 + 7 * slot + word`: arena-team slot 0..3's word
    /// 0..7 (team id, type, captain flag, week games, season games, season wins, rating).
    pub fn player_arena_team_info(&self, slot: u16, word: u16) -> Option<u32> {
        (slot < 3 && word < ARENA_TEAM_INFO_STRIDE).then(|| {
            self.get_u32(
                FIELD_PLAYER_FIELD_ARENA_TEAM_INFO_1_1 + slot * ARENA_TEAM_INFO_STRIDE + word,
            )
        })?
    }
    /// `PLAYER_FIELD_HONOR_CURRENCY` (private): the honor point balance.
    pub fn player_honor_currency(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_FIELD_HONOR_CURRENCY)
    }
    /// `PLAYER_FIELD_ARENA_CURRENCY` (private): the arena point balance.
    pub fn player_arena_currency(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_FIELD_ARENA_CURRENCY)
    }
    /// `PLAYER_RUNE_REGEN_1 + kind` (private): runes of `kind` (0 blood, 1 unholy, 2 frost,
    /// 3 death) regained per second; 0 when absent.
    pub fn player_rune_regen(&self, kind: u8) -> f32 {
        (kind < 4)
            .then(|| self.get_f32(FIELD_PLAYER_RUNE_REGEN_1 + u16::from(kind)))
            .flatten()
            .unwrap_or(0.0)
    }
    /// `PLAYER_FIELD_GLYPH_SLOTS_1 + socket` (private): the socket's `GlyphSlot.dbc` id, 0
    /// when absent.
    pub fn player_glyph_slot(&self, socket: u8) -> u32 {
        (socket < PLAYER_GLYPH_SOCKETS)
            .then(|| self.get_u32(FIELD_PLAYER_GLYPH_SLOTS_1 + u16::from(socket)))
            .flatten()
            .unwrap_or(0)
    }
    /// `PLAYER_FIELD_GLYPHS_1 + socket` (private): the inscribed `GlyphProperties.dbc` id, 0
    /// when empty.
    pub fn player_glyph(&self, socket: u8) -> u32 {
        (socket < PLAYER_GLYPH_SOCKETS)
            .then(|| self.get_u32(FIELD_PLAYER_GLYPHS_1 + u16::from(socket)))
            .flatten()
            .unwrap_or(0)
    }
    /// `PLAYER_CHOSEN_TITLE` (public): the shown title's `CharTitles.dbc` mask id, 0 for
    /// none; read signed, as the reference's title lookup (`0x72a290`) shows none at or below 0.
    pub fn player_chosen_title(&self) -> i32 {
        self.get_i32(FIELD_PLAYER_CHOSEN_TITLE).unwrap_or(0)
    }
    /// `PLAYER__FIELD_KNOWN_TITLES` (private): whether mask id `bit` is set
    /// (`IsTitleKnown` `0x51b410`); bits past the 192 answer false.
    pub fn player_knows_title(&self, bit: u32) -> bool {
        if bit >= PLAYER_KNOWN_TITLE_BITS {
            return false;
        }
        let word = self
            .get_u32(FIELD_PLAYER_KNOWN_TITLES + (bit / 32) as u16)
            .unwrap_or(0);
        word & (1 << (bit % 32)) != 0
    }
    /// The six known-titles words, for an edge test.
    pub fn player_known_titles(&self) -> [u32; 6] {
        std::array::from_fn(|i| {
            self.get_u32(FIELD_PLAYER_KNOWN_TITLES + i as u16)
                .unwrap_or(0)
        })
    }
    /// `PLAYER_FIELD_KNOWN_CURRENCIES` (private): bit `i` for `CurrencyTypes.dbc` bit index
    /// `i + 1`.
    pub fn player_known_currencies(&self) -> u64 {
        let lo = self.get_u32(FIELD_PLAYER_KNOWN_CURRENCIES).unwrap_or(0);
        let hi = self.get_u32(FIELD_PLAYER_KNOWN_CURRENCIES + 1).unwrap_or(0);
        u64::from(lo) | u64::from(hi) << 32
    }
    /// `PLAYER_FIELD_CURRENCYTOKEN_SLOT_1 + 2i`: the item guid in currency slot `i`
    /// (player slot `118 + i`), `Some(0)` when empty.
    pub fn player_currency_token_slot(&self, i: u8) -> Option<u64> {
        (i < CURRENCY_TOKEN_SLOTS)
            .then(|| self.get_guid(FIELD_PLAYER_CURRENCY_TOKEN_SLOT_1 + 2 * u16::from(i)))?
    }
    /// `PLAYER_GLYPHS_ENABLED` (private): bit `i` set while socket `i` is unlocked.
    pub fn player_glyphs_enabled(&self) -> u32 {
        self.get_u32(FIELD_PLAYER_GLYPHS_ENABLED).unwrap_or(0)
    }
    /// `PLAYER_FIELD_COMBAT_RATING_1 + i`: combat rating `i`, 0-based.
    pub fn player_combat_rating(&self, i: u8) -> Option<i32> {
        (i < PLAYER_COMBAT_RATINGS)
            .then(|| self.get_i32(FIELD_PLAYER_COMBAT_RATING_1 + u16::from(i)))?
    }
    /// `UNIT_FIELD_POSSTAT0 + i`: stat `i`'s positive buff total from gear, enchants and auras;
    /// an int on the wire, narrowed from the server's float.
    pub fn player_posstat(&self, i: u8) -> Option<i32> {
        (i < 5).then(|| self.get_i32(FIELD_UNIT_POSSTAT0 + u16::from(i)))?
    }
    /// `UNIT_FIELD_NEGSTAT0 + i`: stat `i`'s negative buff total, an int at or below 0. An x86
    /// server sends it in two's complement, an arm64 one saturates it to 0.
    pub fn player_negstat(&self, i: u8) -> Option<i32> {
        (i < 5).then(|| self.get_i32(FIELD_UNIT_NEGSTAT0 + u16::from(i)))?
    }
    /// `UNIT_FIELD_RESISTANCEBUFFMODSPOSITIVE + school`: the positive resistance buff, an int.
    pub fn player_resistance_buff_pos(&self, school: u8) -> Option<i32> {
        (school < 7)
            .then(|| self.get_i32(FIELD_UNIT_RESISTANCEBUFFMODSPOSITIVE + u16::from(school)))?
    }
    /// `UNIT_FIELD_RESISTANCEBUFFMODSNEGATIVE + school`: the negative resistance buff, at most 0.
    pub fn player_resistance_buff_neg(&self, school: u8) -> Option<i32> {
        (school < 7)
            .then(|| self.get_i32(FIELD_UNIT_RESISTANCEBUFFMODSNEGATIVE + u16::from(school)))?
    }
    /// `PLAYER_FIELD_MOD_DAMAGE_DONE_POS + school`: the positive damage bonus, school 0 physical.
    pub fn player_mod_damage_done_pos(&self, school: u8) -> Option<i32> {
        (school < 7).then(|| self.get_i32(FIELD_PLAYER_MOD_DAMAGE_DONE_POS + u16::from(school)))?
    }
    /// `PLAYER_FIELD_MOD_DAMAGE_DONE_NEG + school`: the negative damage bonus, at most 0.
    pub fn player_mod_damage_done_neg(&self, school: u8) -> Option<i32> {
        (school < 7).then(|| self.get_i32(FIELD_PLAYER_MOD_DAMAGE_DONE_NEG + u16::from(school)))?
    }
    /// `PLAYER_FIELD_MOD_DAMAGE_DONE_PCT + school`: the damage multiplier, a true float that the
    /// server starts at 1.0 (`Player.cpp:3336`); a caller reads `None` as 1.0.
    pub fn player_mod_damage_done_pct(&self, school: u8) -> Option<f32> {
        (school < 7).then(|| self.get_f32(FIELD_PLAYER_MOD_DAMAGE_DONE_PCT + u16::from(school)))?
    }
    /// `PLAYER_SKILL_INFO_<slot+1>_*`: one skill slot; vmangos writes the id and value together
    /// (`SetSkill`), so an absent value or bonus reads 0.
    pub fn player_skill(&self, slot: u8) -> Option<PlayerSkillSlot> {
        if slot >= PLAYER_SKILL_SLOTS {
            return None;
        }
        let base = FIELD_PLAYER_SKILL_INFO_1_1 + 3 * u16::from(slot);
        let (skill_id, step) = self.get_u16_pair(base)?;
        let (value, max) = self.get_u16_pair(base + 1).unwrap_or((0, 0));
        let (temp_bonus, perm_bonus) = self.get_u16_pair(base + 2).unwrap_or((0, 0));
        Some(PlayerSkillSlot {
            skill_id,
            step,
            value,
            max,
            temp_bonus: temp_bonus as i16,
            perm_bonus: perm_bonus as i16,
        })
    }
    /// `PLAYER_AMMO_ID`: the item id of the selected arrows or bullets, 0 for none.
    pub fn player_ammo_id(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_AMMO_ID)
    }
    /// `PLAYER_SELF_RES_SPELL`: the spell `CMSG_SELF_RES` casts (Soulstone 3026, 20758-20761,
    /// Reincarnation 21169, Twisting Nether 23700) and the whole condition for offering it; set
    /// one flush before health 0 at death (`Unit.cpp:1136-1143`), zeroed by any other resurrection.
    pub fn player_self_res_spell(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_SELF_RES_SPELL)
            .filter(|&s| s != 0)
    }
    /// `PLAYER_FIELD_PVP_MEDALS` (private): bit `n - 1` holds medal `n`, an item's
    /// `RequiredCityRank` (`0x61dcb0`).
    pub fn player_pvp_medals(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_FIELD_PVP_MEDALS)
    }
    /// `PLAYER_BYTES`: skinColor, faceType, hairStyle and hairColor, bytes 0 to 3.
    pub fn player_bytes(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_BYTES)
    }
    /// `PLAYER_BYTES_2`: facialHair, unknown, bank bag slots and rest state, bytes 0 to 3.
    pub fn player_bytes_2(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_BYTES_2)
    }
    /// `PLAYER_BYTES_2` byte 3: rest state, 1 rested, 2 normal (`Player.h:673`), set with
    /// hysteresis on the pool (`SetRestBonus`). Absent reads 0, which `GetRestState()` answers
    /// with nils; do not default it to 2, which would hide a read before the create landed.
    pub fn player_rest_state(&self) -> Option<u8> {
        self.player_bytes_2().map(|b| (b >> 24) as u8)
    }
    /// `PLAYER_REST_STATE_EXPERIENCE` (private): the rested pool in base kill XP,
    /// drained 1:1 while kills pay double (`GetXPRestBonus`), so the bar's rested span is twice it.
    pub fn player_rest_state_experience(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_REST_STATE_EXPERIENCE)
    }
    /// `PLAYER_BYTES_3` byte 0: the player's own gender copy, 1 female, which 12340's title verbs
    /// read for the active player (`0x51b546`).
    pub fn player_gender_byte(&self) -> Option<u8> {
        self.get_u32(FIELD_PLAYER_BYTES_3).map(|v| v as u8)
    }
    /// `PLAYER_BYTES_3` byte 1: drunkenness, the high byte of `drunk & 0xFFFE` in the low half
    /// (`SetDrunkValue`); the reference clamps it to 100 and scales by 0.01.
    pub fn player_drunk_byte(&self) -> Option<u8> {
        self.get_u32(FIELD_PLAYER_BYTES_3).map(|v| (v >> 8) as u8)
    }
    /// `PLAYER_FLAGS`: the player state flags.
    pub fn player_flags(&self) -> u32 {
        self.get_u32(FIELD_PLAYER_FLAGS).unwrap_or(0)
    }
    /// `PLAYER_FLAGS` bit `0x1`: leads their group, for a stranger's party too; the first leg of
    /// `UnitIsPartyLeader` and the bit's only test.
    pub fn player_is_group_leader(&self) -> bool {
        self.player_flags() & 0x1 != 0
    }
    /// `PLAYER_FLAGS_GHOST` (`0x10`), from aura 8326; a ghost has health 1, so not `unit_is_dead`.
    pub fn player_is_ghost(&self) -> bool {
        self.player_flags() & 0x10 != 0
    }
    /// Dead or a ghost, as the reference's dead-or-ghost test reads it: health 0, or for a player
    /// the ghost flag. Not feign death, which is `unit_reads_dead`; a creature has no
    /// `PLAYER_FLAGS`, so reads clear.
    pub fn is_dead_or_ghost(&self) -> bool {
        self.unit_is_dead() || self.player_is_ghost()
    }
    /// `PLAYER_FLAGS_HIDE_HELM` (`0x400`, `Player.h:325`), set by `CMSG_TOGGLE_HELM`; public.
    pub fn player_hides_helm(&self) -> bool {
        self.player_flags() & 0x400 != 0
    }
    /// `PLAYER_FLAGS_HIDE_CLOAK` (`0x800`), public like the helm bit.
    pub fn player_hides_cloak(&self) -> bool {
        self.player_flags() & 0x800 != 0
    }
    /// `PLAYER_FLAGS_RESTING` (`0x20`, `Player.h:320`): in an inn or city; `IsResting()` reads it.
    pub fn player_is_resting(&self) -> bool {
        self.player_flags() & 0x20 != 0
    }
    /// `PLAYER_DUEL_ARBITER` (guid, public): the duel flag GameObject, 0 for none;
    /// set on both duellists at the challenge (`Spell::EffectDuel`).
    pub fn player_duel_arbiter(&self) -> u64 {
        self.get_guid(FIELD_PLAYER_DUEL_ARBITER).unwrap_or(0)
    }
    /// `PLAYER_DUEL_TEAM`: 0 until the countdown ends, then 1 challenger, 2
    /// challenged (`Player::UpdateDuelFlag`); a duel is live only when both sides are nonzero.
    pub fn player_duel_team(&self) -> u32 {
        self.get_u32(FIELD_PLAYER_DUEL_TEAM).unwrap_or(0)
    }
    /// `PLAYER_GUILDID` (public): the guild, 0 for none; names come from
    /// `SMSG_GUILD_QUERY_RESPONSE`, and `GetGuildInfo` answers nil on 0 or a miss.
    pub fn player_guild_id(&self) -> u32 {
        self.get_u32(FIELD_PLAYER_GUILDID).unwrap_or(0)
    }
    /// `PLAYER_GUILDRANK`: the guild rank, 0 the guild master; the reference indexes
    /// its rank names with it unbounded, so bound it to the ten ranks.
    pub fn player_guild_rank(&self) -> u32 {
        self.get_u32(FIELD_PLAYER_GUILDRANK).unwrap_or(0)
    }
    /// `PLAYER_FIELD_BYTES` byte 0 bit `0x08`, `PLAYER_FIELD_BYTE_RELEASE_TIMER`: set at death
    /// outside instances, where the server releases the spirit after 6 minutes; the client arms
    /// its own `now + 360000` ms, and with the bit clear `GetReleaseTimeRemaining()` is -1.
    pub fn player_release_timer_running(&self) -> bool {
        self.get_u32(FIELD_PLAYER_FIELD_BYTES).unwrap_or(0) & 0x08 != 0
    }
    /// `PLAYER_FIELD_BYTES` byte 0 bit `0x10`: no release window, so the timer reads 0 at once
    /// (`0x6dc096`).
    pub fn player_no_release_window(&self) -> bool {
        self.get_u32(FIELD_PLAYER_FIELD_BYTES).unwrap_or(0) & 0x10 != 0
    }
    /// `PLAYER_FIELD_BYTES` byte 2 (private): the four extra action bars' toggles, bits
    /// `0x01..0x08`. The client never writes it and no event fires when it moves.
    pub fn player_action_bar_toggles(&self) -> Option<u8> {
        self.get_u32(FIELD_PLAYER_FIELD_BYTES)
            .map(|b| ((b >> 16) & 0xff) as u8)
    }
    /// `PLAYER_FIELD_BYTES` byte 3 (private): the lifetime highest honor rank held, 0 never
    /// ranked, which `RequiredHonorRank` checks.
    pub fn player_honor_rank(&self) -> Option<u8> {
        self.get_u32(FIELD_PLAYER_FIELD_BYTES)
            .map(|b| ((b >> 24) & 0xff) as u8)
    }
    /// `PLAYER_BYTES_3` byte 3 as 12340 reads it: the arena team side, which a player streamed
    /// into an arena is compared against our own to make it an opponent (`0x6e830f`).
    pub fn player_arena_faction(&self) -> Option<u8> {
        self.get_u32(FIELD_PLAYER_BYTES_3)
            .map(|b| ((b >> 24) & 0xff) as u8)
    }
    /// `PLAYER_FIELD_YESTERDAY_CONTRIBUTION` (private): yesterday's honor.
    pub fn player_yesterday_contribution(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_FIELD_YESTERDAY_CONTRIBUTION)
    }
    /// `PLAYER_FIELD_LIFETIME_HONORBALE_KILLS` (private, the reference's spelling): lifetime
    /// honorable kills.
    pub fn player_lifetime_honorable_kills(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_FIELD_LIFETIME_HONORABLE_KILLS)
    }
    /// `PLAYER_FIELD_BYTES2`'s low `i16` (private): the `OverrideSpellData` row the bar's
    /// override spells come from (`0x5abb40`).
    pub fn player_override_spells_id(&self) -> Option<i16> {
        self.get_u32(FIELD_PLAYER_FIELD_BYTES2)
            .map(|v| v as u16 as i16)
    }
    /// `PLAYER_FIELD_BYTES2` byte 3 (private): the aura vision, `0x20` stealth and `0x40`
    /// invisibility detection, which 12340 reads at `+0x10e7` to show or hide the detection
    /// auras' state kits on every unit (`0x724979`, `0x727b4e`).
    pub fn player_aura_vision(&self) -> Option<u8> {
        self.get_u32(FIELD_PLAYER_FIELD_BYTES2)
            .map(|b| (b >> 24) as u8)
    }
    /// `UNIT_FIELD_BYTES_0` byte 0: race.
    pub fn unit_race(&self) -> Option<u8> {
        self.unit_bytes_0().map(|b| (b & 0xff) as u8)
    }
    /// `UNIT_FIELD_BYTES_0` byte 1: class.
    pub fn unit_class(&self) -> Option<u8> {
        self.unit_bytes_0().map(|b| ((b >> 8) & 0xff) as u8)
    }
    /// `UNIT_FIELD_BYTES_0` byte 2: gender.
    pub fn unit_gender(&self) -> Option<u8> {
        self.unit_bytes_0().map(|b| ((b >> 16) & 0xff) as u8)
    }
    /// `PLAYER_BYTES` byte 0: skinColor.
    pub fn player_skin(&self) -> Option<u8> {
        self.player_bytes().map(|b| (b & 0xff) as u8)
    }
    /// `PLAYER_BYTES` byte 1: faceType.
    pub fn player_face(&self) -> Option<u8> {
        self.player_bytes().map(|b| ((b >> 8) & 0xff) as u8)
    }
    /// `PLAYER_BYTES` byte 2: hairStyle.
    pub fn player_hair_style(&self) -> Option<u8> {
        self.player_bytes().map(|b| ((b >> 16) & 0xff) as u8)
    }
    /// `PLAYER_BYTES` byte 3: hairColor.
    pub fn player_hair_color(&self) -> Option<u8> {
        self.player_bytes().map(|b| ((b >> 24) & 0xff) as u8)
    }
    /// `PLAYER_BYTES_2` byte 0: facialHair.
    pub fn player_facial_hair(&self) -> Option<u8> {
        self.player_bytes_2().map(|b| (b & 0xff) as u8)
    }
    /// `PLAYER_BYTES_2` byte 2 (`GetNumBankSlots`, `0x57bab0`): bank bag slots bought, of 7; this
    /// delta is the only reply to a successful `CMSG_BUY_BANK_SLOT` (`PLAYERBANKBAGSLOTS_CHANGED`).
    pub fn player_bank_bag_slots_purchased(&self) -> Option<u8> {
        self.player_bytes_2().map(|b| ((b >> 16) & 0xff) as u8)
    }
    /// `OBJECT_FIELD_CREATED_BY`: who summoned a spell-spawned object; the faction
    /// resolver prefers its reaction, but a bobber's use gate is our channel object.
    pub fn gameobject_created_by(&self) -> Option<u64> {
        self.get_guid(FIELD_GAMEOBJECT_CREATED_BY)
            .filter(|&g| g != 0)
    }
    /// `GAMEOBJECT_DISPLAYID`.
    pub fn gameobject_displayid(&self) -> Option<i32> {
        self.get_i32(FIELD_GAMEOBJECT_DISPLAYID)
    }
    /// `GAMEOBJECT_BYTES_1` byte 0, the state: 0 active (open), 1 ready (closed), 2 active-alt.
    pub fn gameobject_state(&self) -> Option<u32> {
        self.get_u32(FIELD_GAMEOBJECT_BYTES_1)
            .map(field::gameobject_state_of)
    }
    /// `GAMEOBJECT_BYTES_1` byte 1, the type: absent is 0, `GAMEOBJECT_TYPE_DOOR`, as a create
    /// omits zero fields.
    pub fn gameobject_type_id(&self) -> i32 {
        self.get_u32(FIELD_GAMEOBJECT_BYTES_1)
            .map_or(0, |b| i32::from((b >> 8) as u8))
    }
    /// `GAMEOBJECT_FLAGS`: `0x1` in use, `0x4` needs the quest activate bit, `0x10` no interact.
    pub fn gameobject_flags(&self) -> u32 {
        self.get_u32(FIELD_GAMEOBJECT_FLAGS).unwrap_or(0)
    }
    /// `GAMEOBJECT_FACTION`: the `FactionTemplate.dbc` id, 0 neutral; hostile to the player
    /// means not highlightable: no cursor, tooltip or hover.
    pub fn gameobject_faction(&self) -> u32 {
        self.get_u32(FIELD_GAMEOBJECT_FACTION).unwrap_or(0)
    }
    /// `GAMEOBJECT_DYNAMIC`'s low half (per player): `0x1` activate, read only under flag `0x4`;
    /// `0x8` the loot sparkle (`0x71121c`).
    pub fn gameobject_dynamic_flags(&self) -> u32 {
        self.get_u32(FIELD_GAMEOBJECT_DYNAMIC)
            .map_or(0, field::gameobject_dyn_flags_of)
    }
    /// `GAMEOBJECT_LEVEL`: a lock with `Skill[0]` 0 asks for `level * 5`.
    pub fn gameobject_level(&self) -> u32 {
        self.get_u32(FIELD_GAMEOBJECT_LEVEL).unwrap_or(0)
    }

    /// `OBJECT_FIELD_TYPE & TYPEMASK_CONTAINER`: a bag, whose mask carries the item bit too; the
    /// reference's test in `GetInventoryItemCount` and in `PutItemInBag`.
    pub fn is_container(&self) -> bool {
        self.get_u32(FIELD_OBJECT_TYPE).unwrap_or(0) & 0x4 != 0
    }

    /// `OBJECT_FIELD_TYPE`, the type mask the object-pointer lookups test.
    pub fn type_mask(&self) -> u32 {
        self.get_u32(FIELD_OBJECT_TYPE).unwrap_or(0)
    }

    /// `OBJECT_FIELD_TYPE`'s unit bit `0x8`, a player included: the object-pointer type mask 8.
    pub fn is_unit(&self) -> bool {
        self.get_u32(FIELD_OBJECT_TYPE).unwrap_or(0) & 0x8 != 0
    }

    /// The object's class from `OBJECT_FIELD_TYPE`, for updates with no `TypeId`; the player bit
    /// `0x10` is the reference's own is-player test (in its attack and interact checks).
    pub fn object_type(&self) -> Option<ObjectType> {
        self.get_u32(FIELD_OBJECT_TYPE).map(|bits| {
            if bits & 0x10 != 0 {
                ObjectType::Player
            } else if bits & 0x08 != 0 {
                ObjectType::Unit
            } else if bits & 0x20 != 0 {
                ObjectType::GameObject
            } else {
                ObjectType::Object
            }
        })
    }
}

/// The player slot (0-149) a slot array's guid word `index` holds, and whether it is the high word.
pub fn player_slot_of_field(index: u16) -> Option<(u8, bool)> {
    const RUNS: [(u16, u8, u8); 7] = [
        (FIELD_PLAYER_INV_SLOT_HEAD, 0, 23),
        (FIELD_PLAYER_PACK_SLOT_1, 23, 16),
        (FIELD_PLAYER_BANK_SLOT_1, 39, crate::messages::BANK_SLOTS),
        (FIELD_PLAYER_BANK_BAG_SLOT_1, 67, crate::messages::BANK_BAGS),
        (FIELD_PLAYER_VENDORBUYBACK_SLOT_1, 74, 12),
        (
            FIELD_PLAYER_KEYRING_SLOT_1,
            86,
            crate::messages::KEYRING_SLOTS,
        ),
        (
            FIELD_PLAYER_CURRENCY_TOKEN_SLOT_1,
            118,
            CURRENCY_TOKEN_SLOTS,
        ),
    ];
    RUNS.iter().find_map(|&(base, first, n)| {
        let off = index.checked_sub(base)?;
        (off < 2 * u16::from(n)).then(|| (first + (off / 2) as u8, off % 2 == 1))
    })
}

/// Where enchant slot `slot`'s triple starts, its id's field, for a local write; `None` past slot
/// 11.
pub fn item_enchant_field(slot: u8) -> Option<u16> {
    item_enchant_index(slot)
}

/// `ITEM_FIELD_ENCHANTMENT_<slot+1>_1`, twelve triples in a row; `None` past slot 11.
fn item_enchant_index(slot: u8) -> Option<u16> {
    (slot < 12).then(|| FIELD_ITEM_ENCHANTMENT_1_1 + 3 * u16::from(slot))
}
