//! Where the object update fields benilla reads live. An update field's index counts from the
//! start of its object's descriptor, so the same index names different fields on a unit and a
//! corpse; each place names its [`Block`]. A [`Field`] names a value's meaning, not its encoding.
//! [`WIRE`] is 3.3.5a's layout as the server sends it; [`SHARED`] is the index space
//! [`super::ObjectFields`]' getters read, every field benilla reads, into which the decoder moves
//! and converts each wire value. A field missing from [`WIRE`] is not read.

/// A descriptor block. An object's descriptor is its blocks in order: a player is
/// `Object, Unit, Player`, a container `Object, Item, Container`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Block {
    Object,
    Item,
    Container,
    Unit,
    Player,
    GameObject,
    DynamicObject,
    Corpse,
}

/// An update field benilla reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    ObjectGuid,
    ObjectType,
    ObjectEntry,
    ObjectScaleX,

    ItemOwner,
    ItemContained,
    ItemCreator,
    ItemGiftCreator,
    ItemStackCount,
    ItemDuration,
    ItemSpellCharges,
    ItemFlags,
    /// The permanent and temporary enchant triples (id, duration, charges).
    ItemEnchantPermTemp,
    /// The random-property enchant triples.
    ItemEnchantProps,
    ItemPropertySeed,
    ItemRandomPropertiesId,
    ItemTextId,
    ItemDurability,
    ItemMaxDurability,
    ContainerNumSlots,
    ContainerSlots,

    UnitCharm,
    UnitSummon,
    /// 3.3.5a's summoned companion (`UNIT_FIELD_CRITTER`).
    UnitCritter,
    UnitCharmedBy,
    UnitSummonedBy,
    UnitCreatedBy,
    UnitTarget,
    UnitChannelObject,
    UnitHealth,
    /// Mana, rage, focus, energy and happiness.
    UnitPower,
    /// 3.3.5a's sixth and seventh power: runes and runic power.
    UnitRunePower,
    UnitMaxHealth,
    UnitMaxPower,
    UnitMaxRunePower,
    UnitLevel,
    UnitFactionTemplate,
    UnitBytes0,
    /// A creature's weapon display ids.
    UnitVirtualItemDisplay,
    UnitFlags,
    UnitFlags2,
    UnitAura,
    UnitAuraFlags,
    UnitAuraLevels,
    UnitAuraApplications,
    UnitAuraState,
    UnitBaseAttackTime,
    UnitRangedAttackTime,
    UnitBoundingRadius,
    UnitCombatReach,
    UnitDisplayId,
    UnitNativeDisplayId,
    UnitMountDisplayId,
    UnitMinDamage,
    UnitMaxDamage,
    UnitMinOffhandDamage,
    UnitMaxOffhandDamage,
    UnitBytes1,
    UnitPetNumber,
    UnitPetNameTimestamp,
    UnitPetExperience,
    UnitPetNextLevelExp,
    UnitDynamicFlags,
    UnitChannelSpell,
    UnitModCastSpeed,
    UnitCreatedBySpell,
    UnitNpcFlags,
    UnitNpcEmoteState,
    UnitTrainingPoints,
    UnitStat,
    UnitResistances,
    UnitBaseMana,
    UnitBaseHealth,
    UnitBytes2,
    UnitAttackPower,
    UnitAttackPowerMods,
    UnitAttackPowerMultiplier,
    UnitRangedAttackPower,
    UnitRangedAttackPowerMods,
    UnitRangedAttackPowerMultiplier,
    UnitMinRangedDamage,
    UnitMaxRangedDamage,
    /// Per-power regeneration per second out of combat and while casting, floats.
    UnitPowerRegenFlatModifier,
    UnitPowerRegenInterruptedFlatModifier,
    /// The max-health multiplier less one, a float.
    UnitMaxHealthModifier,
    /// The positive and negative stat and resistance buffs: a unit block on the wire, a player
    /// block in [`SHARED`].
    PosStat,
    NegStat,
    ResistanceBuffModsPositive,
    ResistanceBuffModsNegative,

    PlayerDuelArbiter,
    PlayerFlags,
    PlayerGuildId,
    PlayerGuildRank,
    PlayerBytes,
    PlayerBytes2,
    PlayerBytes3,
    PlayerDuelTeam,
    PlayerQuestLog,
    /// 3.3.5a's daily quests done today, a quest id per field.
    PlayerDailyQuests,
    /// 3.3.5a's level cap for the account's expansion.
    PlayerMaxLevel,
    /// Each equipment slot's broadcast item entry.
    PlayerVisibleItemEntry,
    /// Each equipment slot's broadcast enchants.
    PlayerVisibleItemEnchants,
    PlayerVisibleItemProperties,
    PlayerInvSlots,
    PlayerPackSlots,
    PlayerBankSlots,
    PlayerBankBagSlots,
    PlayerVendorBuybackSlots,
    PlayerKeyringSlots,
    /// 3.3.5a's 32 currency-token slots (player slots 118..149), item guids.
    PlayerCurrencyTokenSlots,
    PlayerFarsight,
    /// 3.3.5a's chosen title, a `CharTitles.dbc` mask id, 0 for none.
    PlayerChosenTitle,
    /// 3.3.5a's known-titles bitmask, 192 bits indexed by mask id.
    PlayerKnownTitles,
    /// 3.3.5a's known-currencies bitmask, 64 bits indexed by `CurrencyTypes.dbc` bit index - 1.
    PlayerKnownCurrencies,
    PlayerComboTarget,
    PlayerXp,
    PlayerNextLevelXp,
    PlayerSkillInfo,
    PlayerCharacterPoints1,
    PlayerCharacterPoints2,
    PlayerTrackCreatures,
    PlayerTrackResources,
    PlayerBlockPercentage,
    PlayerDodgePercentage,
    PlayerParryPercentage,
    PlayerCritPercentage,
    PlayerRangedCritPercentage,
    /// Main-hand and off-hand expertise, integers.
    PlayerExpertise,
    PlayerOffhandExpertise,
    /// Spell crit chance per school, percent floats, `[0]` physical.
    PlayerSpellCritPercentage,
    PlayerShieldBlock,
    PlayerModHealingDonePos,
    PlayerModHealingPct,
    PlayerModHealingDonePct,
    /// The spell penetration, a non-positive integer.
    PlayerModTargetResistance,
    /// `PLAYER_PET_SPELL_POWER`, the pet's spell bonus `GetPetSpellBonusDamage` reads (`0x60e43b`).
    PlayerPetSpellPower,
    /// 3.3.5a's Refer-a-Friend levels left to grant, `PLAYER_FIELD_BYTES` byte 1.
    PlayerRafGrantableLevel,
    /// 3.3.5a's `OverrideSpellData` id, `PLAYER_FIELD_BYTES2` bytes 0-1.
    PlayerOverrideSpellsId,
    /// 3.3.5a's honorable kills, today's in the low half and yesterday's in the high.
    PlayerKills,
    /// 3.3.5a's honor earned today.
    PlayerTodayContribution,
    /// 3.3.5a's three arena-team slots of seven words: team id, type, captain flag, games this
    /// week, games this season, wins this season and personal rating.
    PlayerArenaTeamInfo,
    /// 3.3.5a's honor and arena-point balances.
    PlayerHonorCurrency,
    PlayerArenaCurrency,
    /// 3.3.5a's rune regeneration per rune type, runes per second as floats.
    PlayerRuneRegen,
    /// 3.3.5a's six glyph sockets: each a `GlyphSlot.dbc` id, 0 for a socket not there.
    PlayerGlyphSlots,
    /// 3.3.5a's six inscribed glyphs, `GlyphProperties.dbc` ids, 0 for an empty socket.
    PlayerGlyphs,
    /// 3.3.5a's unlocked-socket mask, bit `i` for socket `i`.
    PlayerGlyphsEnabled,
    /// The 25 combat ratings.
    PlayerCombatRating,
    PlayerExploredZones,
    PlayerRestStateExperience,
    PlayerCoinage,
    PlayerModDamageDonePos,
    PlayerModDamageDoneNeg,
    PlayerModDamageDonePct,
    PlayerFieldBytes,
    PlayerAmmoId,
    PlayerSelfResSpell,
    PlayerPvpMedals,
    PlayerBuybackPrice,
    PlayerBuybackTimestamp,
    PlayerSessionKills,
    PlayerYesterdayKills,
    PlayerLastWeekKills,
    PlayerThisWeekKills,
    PlayerThisWeekContribution,
    PlayerLifetimeHonorableKills,
    PlayerLifetimeDishonorableKills,
    PlayerYesterdayContribution,
    PlayerLastWeekContribution,
    PlayerLastWeekRank,
    PlayerFieldBytes2,
    PlayerWatchedFactionIndex,

    GameObjectCreatedBy,
    GameObjectDisplayId,
    GameObjectFlags,
    /// The spawn's own rotation quaternion `(x, y, z, w)`, a [`SHARED`] place only: 3.3.5a sends
    /// it packed in the movement block (`UPDATEFLAG_ROTATION`).
    GameObjectRotation,
    GameObjectState,
    GameObjectPosition,
    GameObjectDynFlags,
    GameObjectFaction,
    GameObjectTypeId,
    GameObjectLevel,
    GameObjectArtKit,
    GameObjectAnimProgress,
    /// `GAMEOBJECT_DYNAMIC`: the dynamic flags in the low half, path progress above.
    GameObjectDynamic,
    /// `GAMEOBJECT_BYTES_1`: state, type, art kit and anim progress, bytes 0 to 3.
    GameObjectBytes1,

    DynamicObjectCaster,
    DynamicObjectBytes,
    DynamicObjectSpellId,
    DynamicObjectRadius,
    /// x, y, z and facing, a [`SHARED`] place only: 3.3.5a sends them in the movement block
    /// (`UPDATEFLAG_STATIONARY_POSITION`).
    DynamicObjectPosition,

    CorpseOwner,
    CorpseDisplayId,
    CorpseItems,
    CorpseBytes1,
    CorpseBytes2,
    CorpseGuild,
    CorpseFlags,
    CorpseDynamicFlags,
}

/// Where one [`Field`] lives: `count` elements, element `i` at `index + i * stride`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldPlace {
    pub field: Field,
    pub block: Block,
    pub index: u16,
    pub count: u16,
    pub stride: u16,
}

impl FieldPlace {
    /// The index of element `i`, `None` past the last.
    pub fn element(&self, i: u16) -> Option<u16> {
        (i < self.count).then(|| self.index + i * self.stride)
    }
}

/// Each object type's descriptor length in dwords, the `*_END` of its innermost block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DescriptorEnds {
    pub object: u16,
    pub item: u16,
    pub container: u16,
    pub unit: u16,
    pub player: u16,
    pub gameobject: u16,
    pub dynamicobject: u16,
    pub corpse: u16,
}

/// An update-field layout.
#[derive(Debug)]
pub struct UpdateFieldLayout {
    pub ends: DescriptorEnds,
    pub places: &'static [FieldPlace],
}

impl UpdateFieldLayout {
    /// Where this layout keeps `field`, `None` when it has no such field.
    pub fn place(&self, field: Field) -> Option<&FieldPlace> {
        self.places.iter().find(|p| p.field == field)
    }
}

const fn run(field: Field, block: Block, index: u16, count: u16) -> FieldPlace {
    FieldPlace {
        field,
        block,
        index,
        count,
        stride: 1,
    }
}

const fn one(field: Field, block: Block, index: u16) -> FieldPlace {
    run(field, block, index, 1)
}

const fn strided(field: Field, block: Block, index: u16, count: u16, stride: u16) -> FieldPlace {
    FieldPlace {
        field,
        block,
        index,
        count,
        stride,
    }
}

use Block::{Container, Corpse, DynamicObject, GameObject, Item, Object, Player, Unit};
use Field as F;

/// The index space [`super::ObjectFields`] keeps, 1.12.1's (vmangos `UpdateFields_1_12_1.h`)
/// numbering, with the places only the movement block fills on the wire and, past its player end,
/// the 3.3.5a quest log, which outgrew 1.12.1's 20 slots of 3.
pub(crate) static SHARED: UpdateFieldLayout = UpdateFieldLayout {
    ends: DescriptorEnds {
        object: 6,
        item: 48,
        container: 122,
        unit: 188,
        player: 1660,
        gameobject: 26,
        dynamicobject: 16,
        corpse: 38,
    },
    places: &[
        run(F::ObjectGuid, Object, 0, 2),
        one(F::ObjectType, Object, 2),
        one(F::ObjectEntry, Object, 3),
        one(F::ObjectScaleX, Object, 4),
        run(F::ItemOwner, Item, 6, 2),
        run(F::ItemContained, Item, 8, 2),
        run(F::ItemCreator, Item, 10, 2),
        run(F::ItemGiftCreator, Item, 12, 2),
        one(F::ItemStackCount, Item, 14),
        one(F::ItemDuration, Item, 15),
        run(F::ItemSpellCharges, Item, 16, 5),
        one(F::ItemFlags, Item, 21),
        // `ITEM_FIELD_ENCHANTMENT`, 7 triples: slots 0 and 1, then the props at 3..=6.
        run(F::ItemEnchantPermTemp, Item, 22, 6),
        run(F::ItemEnchantProps, Item, 31, 12),
        one(F::ItemPropertySeed, Item, 43),
        one(F::ItemRandomPropertiesId, Item, 44),
        one(F::ItemTextId, Item, 45),
        one(F::ItemDurability, Item, 46),
        one(F::ItemMaxDurability, Item, 47),
        one(F::ContainerNumSlots, Container, 48),
        run(F::ContainerSlots, Container, 50, 72),
        run(F::UnitCharm, Unit, 6, 2),
        run(F::UnitSummon, Unit, 8, 2),
        run(F::UnitCharmedBy, Unit, 10, 2),
        run(F::UnitSummonedBy, Unit, 12, 2),
        run(F::UnitCreatedBy, Unit, 14, 2),
        run(F::UnitTarget, Unit, 16, 2),
        // 3.3.5a's critter guid, in 1.12.1's unread `UNIT_FIELD_PERSUADED`.
        run(F::UnitCritter, Unit, 18, 2),
        run(F::UnitChannelObject, Unit, 20, 2),
        one(F::UnitHealth, Unit, 22),
        run(F::UnitPower, Unit, 23, 5),
        one(F::UnitMaxHealth, Unit, 28),
        run(F::UnitMaxPower, Unit, 29, 5),
        one(F::UnitLevel, Unit, 34),
        one(F::UnitFactionTemplate, Unit, 35),
        one(F::UnitBytes0, Unit, 36),
        run(F::UnitVirtualItemDisplay, Unit, 37, 3),
        // 3.3.5a's runes and runic power, in 1.12.1's `UNIT_VIRTUAL_ITEM_INFO`, which it lacks.
        run(F::UnitRunePower, Unit, 40, 2),
        run(F::UnitMaxRunePower, Unit, 42, 2),
        // 3.3.5a's second flags word, in the tail of the same 1.12.1 run.
        one(F::UnitFlags2, Unit, 44),
        one(F::UnitFlags, Unit, 46),
        run(F::UnitAura, Unit, 47, 48),
        run(F::UnitAuraFlags, Unit, 95, 6),
        run(F::UnitAuraLevels, Unit, 101, 12),
        run(F::UnitAuraApplications, Unit, 113, 12),
        one(F::UnitAuraState, Unit, 125),
        run(F::UnitBaseAttackTime, Unit, 126, 2),
        one(F::UnitRangedAttackTime, Unit, 128),
        one(F::UnitBoundingRadius, Unit, 129),
        one(F::UnitCombatReach, Unit, 130),
        one(F::UnitDisplayId, Unit, 131),
        one(F::UnitNativeDisplayId, Unit, 132),
        one(F::UnitMountDisplayId, Unit, 133),
        one(F::UnitMinDamage, Unit, 134),
        one(F::UnitMaxDamage, Unit, 135),
        one(F::UnitMinOffhandDamage, Unit, 136),
        one(F::UnitMaxOffhandDamage, Unit, 137),
        one(F::UnitBytes1, Unit, 138),
        one(F::UnitPetNumber, Unit, 139),
        one(F::UnitPetNameTimestamp, Unit, 140),
        one(F::UnitPetExperience, Unit, 141),
        one(F::UnitPetNextLevelExp, Unit, 142),
        one(F::UnitDynamicFlags, Unit, 143),
        one(F::UnitChannelSpell, Unit, 144),
        one(F::UnitModCastSpeed, Unit, 145),
        one(F::UnitCreatedBySpell, Unit, 146),
        one(F::UnitNpcFlags, Unit, 147),
        one(F::UnitNpcEmoteState, Unit, 148),
        one(F::UnitTrainingPoints, Unit, 149),
        run(F::UnitStat, Unit, 150, 5),
        run(F::UnitResistances, Unit, 155, 7),
        one(F::UnitBaseMana, Unit, 162),
        one(F::UnitBaseHealth, Unit, 163),
        one(F::UnitBytes2, Unit, 164),
        one(F::UnitAttackPower, Unit, 165),
        one(F::UnitAttackPowerMods, Unit, 166),
        one(F::UnitAttackPowerMultiplier, Unit, 167),
        one(F::UnitRangedAttackPower, Unit, 168),
        one(F::UnitRangedAttackPowerMods, Unit, 169),
        one(F::UnitRangedAttackPowerMultiplier, Unit, 170),
        one(F::UnitMinRangedDamage, Unit, 171),
        one(F::UnitMaxRangedDamage, Unit, 172),
        // 3.3.5a's power regen and max-health modifier, in 1.12.1's unread power-cost words.
        run(F::UnitPowerRegenFlatModifier, Unit, 173, 7),
        run(F::UnitPowerRegenInterruptedFlatModifier, Unit, 180, 7),
        one(F::UnitMaxHealthModifier, Unit, 187),
        run(F::PlayerDuelArbiter, Player, 188, 2),
        one(F::PlayerFlags, Player, 190),
        one(F::PlayerGuildId, Player, 191),
        one(F::PlayerGuildRank, Player, 192),
        one(F::PlayerBytes, Player, 193),
        one(F::PlayerBytes2, Player, 194),
        one(F::PlayerBytes3, Player, 195),
        one(F::PlayerDuelTeam, Player, 196),
        // 25 slots of (quest id, state, two words of four `u16` counters, timer).
        run(F::PlayerQuestLog, Player, 1282, 125),
        // 3.3.5a's daily quests and level cap, also kept past the 1.12.1 player end.
        run(F::PlayerDailyQuests, Player, 1407, 25),
        one(F::PlayerMaxLevel, Player, 1432),
        // 3.3.5a's character-sheet fields, past the daily quests.
        one(F::PlayerExpertise, Player, 1433),
        one(F::PlayerOffhandExpertise, Player, 1434),
        run(F::PlayerSpellCritPercentage, Player, 1435, 7),
        one(F::PlayerShieldBlock, Player, 1442),
        one(F::PlayerModHealingDonePos, Player, 1443),
        one(F::PlayerModTargetResistance, Player, 1444),
        run(F::PlayerCombatRating, Player, 1445, 25),
        // `+0x1054`/`+0x1058`, the spell-text operands `pbh` and `pbhd` (`0x5782d0`).
        one(F::PlayerModHealingPct, Player, 1470),
        one(F::PlayerModHealingDonePct, Player, 1471),
        one(F::PlayerPetSpellPower, Player, 1472),
        one(F::PlayerRafGrantableLevel, Player, 1473),
        // 3.3.5a's honor, arena-team and currency fields, past the character sheet.
        one(F::PlayerKills, Player, 1474),
        one(F::PlayerTodayContribution, Player, 1475),
        run(F::PlayerArenaTeamInfo, Player, 1476, 21),
        one(F::PlayerHonorCurrency, Player, 1497),
        one(F::PlayerArenaCurrency, Player, 1498),
        run(F::PlayerRuneRegen, Player, 1499, 4),
        run(F::PlayerGlyphSlots, Player, 1503, 6),
        run(F::PlayerGlyphs, Player, 1509, 6),
        one(F::PlayerGlyphsEnabled, Player, 1515),
        // 3.3.5a's 28 bank slots and 7 bank bags, which outgrew 1.12.1's 24 and 6.
        run(F::PlayerBankSlots, Player, 1516, 56),
        run(F::PlayerBankBagSlots, Player, 1572, 14),
        // 3.3.5a's title and currency fields.
        one(F::PlayerChosenTitle, Player, 1586),
        run(F::PlayerKnownTitles, Player, 1587, 6),
        run(F::PlayerKnownCurrencies, Player, 1593, 2),
        run(F::PlayerCurrencyTokenSlots, Player, 1595, 64),
        one(F::PlayerOverrideSpellsId, Player, 1659),
        // 19 slots of 12: creator guid, entry, 7 enchants, properties, pad.
        strided(F::PlayerVisibleItemEntry, Player, 260, 19, 12),
        strided(F::PlayerVisibleItemEnchants, Player, 261, 19, 12),
        strided(F::PlayerVisibleItemProperties, Player, 268, 19, 12),
        run(F::PlayerInvSlots, Player, 486, 46),
        run(F::PlayerPackSlots, Player, 532, 32),
        run(F::PlayerVendorBuybackSlots, Player, 624, 24),
        run(F::PlayerKeyringSlots, Player, 648, 64),
        run(F::PlayerFarsight, Player, 712, 2),
        run(F::PlayerComboTarget, Player, 714, 2),
        one(F::PlayerXp, Player, 716),
        one(F::PlayerNextLevelXp, Player, 717),
        run(F::PlayerSkillInfo, Player, 718, 384),
        one(F::PlayerCharacterPoints1, Player, 1102),
        one(F::PlayerCharacterPoints2, Player, 1103),
        one(F::PlayerTrackCreatures, Player, 1104),
        one(F::PlayerTrackResources, Player, 1105),
        one(F::PlayerBlockPercentage, Player, 1106),
        one(F::PlayerDodgePercentage, Player, 1107),
        one(F::PlayerParryPercentage, Player, 1108),
        one(F::PlayerCritPercentage, Player, 1109),
        one(F::PlayerRangedCritPercentage, Player, 1110),
        run(F::PlayerExploredZones, Player, 1111, 64),
        one(F::PlayerRestStateExperience, Player, 1175),
        one(F::PlayerCoinage, Player, 1176),
        run(F::PosStat, Player, 1177, 5),
        run(F::NegStat, Player, 1182, 5),
        run(F::ResistanceBuffModsPositive, Player, 1187, 7),
        run(F::ResistanceBuffModsNegative, Player, 1194, 7),
        run(F::PlayerModDamageDonePos, Player, 1201, 7),
        run(F::PlayerModDamageDoneNeg, Player, 1208, 7),
        run(F::PlayerModDamageDonePct, Player, 1215, 7),
        one(F::PlayerFieldBytes, Player, 1222),
        one(F::PlayerAmmoId, Player, 1223),
        one(F::PlayerSelfResSpell, Player, 1224),
        one(F::PlayerPvpMedals, Player, 1225),
        run(F::PlayerBuybackPrice, Player, 1226, 12),
        run(F::PlayerBuybackTimestamp, Player, 1238, 12),
        one(F::PlayerSessionKills, Player, 1250),
        one(F::PlayerYesterdayKills, Player, 1251),
        one(F::PlayerLastWeekKills, Player, 1252),
        one(F::PlayerThisWeekKills, Player, 1253),
        one(F::PlayerThisWeekContribution, Player, 1254),
        one(F::PlayerLifetimeHonorableKills, Player, 1255),
        one(F::PlayerLifetimeDishonorableKills, Player, 1256),
        one(F::PlayerYesterdayContribution, Player, 1257),
        one(F::PlayerLastWeekContribution, Player, 1258),
        one(F::PlayerLastWeekRank, Player, 1259),
        one(F::PlayerFieldBytes2, Player, 1260),
        one(F::PlayerWatchedFactionIndex, Player, 1261),
        run(F::GameObjectCreatedBy, GameObject, 6, 2),
        one(F::GameObjectDisplayId, GameObject, 8),
        one(F::GameObjectFlags, GameObject, 9),
        run(F::GameObjectRotation, GameObject, 10, 4),
        one(F::GameObjectState, GameObject, 14),
        run(F::GameObjectPosition, GameObject, 15, 4),
        one(F::GameObjectDynFlags, GameObject, 19),
        one(F::GameObjectFaction, GameObject, 20),
        one(F::GameObjectTypeId, GameObject, 21),
        one(F::GameObjectLevel, GameObject, 22),
        one(F::GameObjectArtKit, GameObject, 23),
        one(F::GameObjectAnimProgress, GameObject, 24),
        run(F::DynamicObjectCaster, DynamicObject, 6, 2),
        one(F::DynamicObjectBytes, DynamicObject, 8),
        one(F::DynamicObjectSpellId, DynamicObject, 9),
        one(F::DynamicObjectRadius, DynamicObject, 10),
        run(F::DynamicObjectPosition, DynamicObject, 11, 4),
        run(F::CorpseOwner, Corpse, 6, 2),
        one(F::CorpseDisplayId, Corpse, 12),
        run(F::CorpseItems, Corpse, 13, 19),
        one(F::CorpseBytes1, Corpse, 32),
        one(F::CorpseBytes2, Corpse, 33),
        one(F::CorpseGuild, Corpse, 34),
        one(F::CorpseFlags, Corpse, 35),
        one(F::CorpseDynamicFlags, Corpse, 36),
    ],
};

/// 3.3.5a (12340): AzerothCore `UpdateFields.h`. Not here, so not read: the auras
/// (`SMSG_AURA_UPDATE` carries them), the virtual items (item entries, not display ids), the item
/// text id, training points, the combo target and the old honor fields, which 3.3.5a has not.
pub(crate) static WIRE: UpdateFieldLayout = UpdateFieldLayout {
    ends: DescriptorEnds {
        object: 6,
        item: 64,
        container: 138,
        unit: 148,
        player: 1326,
        gameobject: 18,
        dynamicobject: 12,
        corpse: 36,
    },
    places: &[
        run(F::ObjectGuid, Object, 0, 2),
        one(F::ObjectType, Object, 2),
        one(F::ObjectEntry, Object, 3),
        one(F::ObjectScaleX, Object, 4),
        run(F::ItemOwner, Item, 6, 2),
        run(F::ItemContained, Item, 8, 2),
        run(F::ItemCreator, Item, 10, 2),
        run(F::ItemGiftCreator, Item, 12, 2),
        one(F::ItemStackCount, Item, 14),
        one(F::ItemDuration, Item, 15),
        run(F::ItemSpellCharges, Item, 16, 5),
        one(F::ItemFlags, Item, 21),
        // `ITEM_FIELD_ENCHANTMENT`, 12 triples: permanent and temporary, three sockets, bonus,
        // prismatic, then five props at 7..=11 (`EnchantmentSlot`), the first four read.
        run(F::ItemEnchantPermTemp, Item, 22, 6),
        run(F::ItemEnchantProps, Item, 43, 12),
        one(F::ItemPropertySeed, Item, 58),
        one(F::ItemRandomPropertiesId, Item, 59),
        one(F::ItemDurability, Item, 60),
        one(F::ItemMaxDurability, Item, 61),
        one(F::ContainerNumSlots, Container, 64),
        run(F::ContainerSlots, Container, 66, 72),
        run(F::UnitCharm, Unit, 6, 2),
        run(F::UnitSummon, Unit, 8, 2),
        run(F::UnitCritter, Unit, 10, 2),
        run(F::UnitCharmedBy, Unit, 12, 2),
        run(F::UnitSummonedBy, Unit, 14, 2),
        run(F::UnitCreatedBy, Unit, 16, 2),
        run(F::UnitTarget, Unit, 18, 2),
        run(F::UnitChannelObject, Unit, 20, 2),
        one(F::UnitChannelSpell, Unit, 22),
        one(F::UnitBytes0, Unit, 23),
        one(F::UnitHealth, Unit, 24),
        // Seven powers, each read at `POWER1 + type` (`0x71c2e0`) and `MAXPOWER1 + type`
        // (`0x60ef40`); runes and runic power (5, 6) keep their own shared place.
        run(F::UnitPower, Unit, 25, 5),
        run(F::UnitRunePower, Unit, 30, 2),
        one(F::UnitMaxHealth, Unit, 32),
        run(F::UnitMaxPower, Unit, 33, 5),
        run(F::UnitMaxRunePower, Unit, 38, 2),
        one(F::UnitLevel, Unit, 54),
        one(F::UnitFactionTemplate, Unit, 55),
        one(F::UnitFlags, Unit, 59),
        one(F::UnitFlags2, Unit, 60),
        one(F::UnitAuraState, Unit, 61),
        run(F::UnitBaseAttackTime, Unit, 62, 2),
        one(F::UnitRangedAttackTime, Unit, 64),
        one(F::UnitBoundingRadius, Unit, 65),
        one(F::UnitCombatReach, Unit, 66),
        one(F::UnitDisplayId, Unit, 67),
        one(F::UnitNativeDisplayId, Unit, 68),
        one(F::UnitMountDisplayId, Unit, 69),
        one(F::UnitMinDamage, Unit, 70),
        one(F::UnitMaxDamage, Unit, 71),
        one(F::UnitMinOffhandDamage, Unit, 72),
        one(F::UnitMaxOffhandDamage, Unit, 73),
        one(F::UnitBytes1, Unit, 74),
        one(F::UnitPetNumber, Unit, 75),
        one(F::UnitPetNameTimestamp, Unit, 76),
        one(F::UnitPetExperience, Unit, 77),
        one(F::UnitPetNextLevelExp, Unit, 78),
        one(F::UnitDynamicFlags, Unit, 79),
        one(F::UnitModCastSpeed, Unit, 80),
        one(F::UnitCreatedBySpell, Unit, 81),
        one(F::UnitNpcFlags, Unit, 82),
        one(F::UnitNpcEmoteState, Unit, 83),
        run(F::UnitStat, Unit, 84, 5),
        run(F::PosStat, Unit, 89, 5),
        run(F::NegStat, Unit, 94, 5),
        run(F::UnitResistances, Unit, 99, 7),
        run(F::ResistanceBuffModsPositive, Unit, 106, 7),
        run(F::ResistanceBuffModsNegative, Unit, 113, 7),
        one(F::UnitBaseMana, Unit, 120),
        one(F::UnitBaseHealth, Unit, 121),
        one(F::UnitBytes2, Unit, 122),
        one(F::UnitAttackPower, Unit, 123),
        one(F::UnitAttackPowerMods, Unit, 124),
        one(F::UnitAttackPowerMultiplier, Unit, 125),
        one(F::UnitRangedAttackPower, Unit, 126),
        one(F::UnitRangedAttackPowerMods, Unit, 127),
        one(F::UnitRangedAttackPowerMultiplier, Unit, 128),
        one(F::UnitMinRangedDamage, Unit, 129),
        one(F::UnitMaxRangedDamage, Unit, 130),
        // `GetManaRegen` reads power 0 at unit block `+0x88` and `+0xa4` (`0x4f5390`).
        run(F::UnitPowerRegenFlatModifier, Unit, 40, 7),
        run(F::UnitPowerRegenInterruptedFlatModifier, Unit, 47, 7),
        // `GetUnitMaxHealthModifier` reads unit block `+0x22c` (`0x612870`).
        one(F::UnitMaxHealthModifier, Unit, 145),
        run(F::PlayerDuelArbiter, Player, 148, 2),
        one(F::PlayerFlags, Player, 150),
        one(F::PlayerGuildId, Player, 151),
        one(F::PlayerGuildRank, Player, 152),
        one(F::PlayerBytes, Player, 153),
        one(F::PlayerBytes2, Player, 154),
        one(F::PlayerBytes3, Player, 155),
        one(F::PlayerDuelTeam, Player, 156),
        // 25 slots of 5 at `UNIT_END + 0xA`: 12340 reads slot `i` at `(i*5 + 10)*4` into the player
        // block (`0x5e1561`), the counters as `u16` at `+8` (`0x5e161c`).
        run(F::PlayerQuestLog, Player, 158, 125),
        // `UNIT_END + 0x46b`, the cap `GetQuestLogRewardXP` tests (`+0x11ac`, `0x5e4a21`), and the
        // 25 daily quests `GetDailyQuestsCompleted` counts (`+0x11b0`, `0x58db5e`).
        one(F::PlayerMaxLevel, Player, 1279),
        run(F::PlayerDailyQuests, Player, 1280, 25),
        // 19 slots of 2: entry, then the permanent and temporary enchant as two `u16`.
        strided(F::PlayerVisibleItemEntry, Player, 283, 19, 2),
        strided(F::PlayerVisibleItemEnchants, Player, 284, 19, 2),
        run(F::PlayerInvSlots, Player, 324, 46),
        run(F::PlayerPackSlots, Player, 370, 32),
        // 28 bank slots and 7 bank bags.
        run(F::PlayerBankSlots, Player, 402, 56),
        run(F::PlayerBankBagSlots, Player, 458, 14),
        run(F::PlayerVendorBuybackSlots, Player, 472, 24),
        run(F::PlayerKeyringSlots, Player, 496, 64),
        // `UNIT_END + 0x19c`, player slots 118..149 (`GetCurrencyListInfo` `0x5b0680`).
        run(F::PlayerCurrencyTokenSlots, Player, 560, 64),
        run(F::PlayerFarsight, Player, 624, 2),
        // `UNIT_END + 0xad`, player block `+0x2b4` (`GetCurrentTitle` `0x51b3b0`).
        one(F::PlayerChosenTitle, Player, 321),
        // `UNIT_END + 0x1de`, `+0x778` as three u64 (`IsTitleKnown` `0x51b410`).
        run(F::PlayerKnownTitles, Player, 626, 6),
        // `UNIT_END + 0x1e4`, `+0x790` as one u64 (`0x6e5401`).
        run(F::PlayerKnownCurrencies, Player, 632, 2),
        one(F::PlayerXp, Player, 634),
        one(F::PlayerNextLevelXp, Player, 635),
        run(F::PlayerSkillInfo, Player, 636, 384),
        one(F::PlayerCharacterPoints1, Player, 1020),
        one(F::PlayerCharacterPoints2, Player, 1021),
        one(F::PlayerTrackCreatures, Player, 1022),
        one(F::PlayerTrackResources, Player, 1023),
        one(F::PlayerBlockPercentage, Player, 1024),
        one(F::PlayerDodgePercentage, Player, 1025),
        one(F::PlayerParryPercentage, Player, 1026),
        one(F::PlayerCritPercentage, Player, 1029),
        one(F::PlayerRangedCritPercentage, Player, 1030),
        // Player block `+0xdbc`/`+0xdc0` (`GetExpertise` `0x612bf0`).
        one(F::PlayerExpertise, Player, 1027),
        one(F::PlayerOffhandExpertise, Player, 1028),
        // `+0xdd0 + 4 * (school - 1)` (`GetSpellCritChance` `0x60e290`).
        run(F::PlayerSpellCritPercentage, Player, 1032, 7),
        // `+0xdec` (`GetShieldBlock` `0x60dff0`).
        one(F::PlayerShieldBlock, Player, 1039),
        // 128 words; the shared space keeps the first 64.
        run(F::PlayerExploredZones, Player, 1041, 64),
        one(F::PlayerRestStateExperience, Player, 1169),
        one(F::PlayerCoinage, Player, 1170),
        run(F::PlayerModDamageDonePos, Player, 1171, 7),
        run(F::PlayerModDamageDoneNeg, Player, 1178, 7),
        run(F::PlayerModDamageDonePct, Player, 1185, 7),
        // `+0x1050` (`GetSpellBonusHealing` `0x60e3b0`) and `+0x105c` (`GetSpellPenetration`
        // `0x60e470`).
        one(F::PlayerModHealingDonePos, Player, 1192),
        one(F::PlayerModHealingPct, Player, 1193),
        one(F::PlayerModHealingDonePct, Player, 1194),
        one(F::PlayerModTargetResistance, Player, 1195),
        // `UNIT_END + 0x499`, the last player field (`[player+0x1264]`).
        one(F::PlayerPetSpellPower, Player, 1325),
        one(F::PlayerFieldBytes, Player, 1197),
        one(F::PlayerAmmoId, Player, 1198),
        one(F::PlayerSelfResSpell, Player, 1199),
        one(F::PlayerPvpMedals, Player, 1200),
        run(F::PlayerBuybackPrice, Player, 1201, 12),
        run(F::PlayerBuybackTimestamp, Player, 1213, 12),
        // `+0x1080`/`+0x1084` (`GetPVPSessionStats` `0x611a20`).
        one(F::PlayerKills, Player, 1225),
        one(F::PlayerTodayContribution, Player, 1226),
        one(F::PlayerYesterdayContribution, Player, 1227),
        one(F::PlayerLifetimeHonorableKills, Player, 1228),
        one(F::PlayerFieldBytes2, Player, 1229),
        one(F::PlayerWatchedFactionIndex, Player, 1230),
        // `+0x10ec + 4 * index` (`GetCombatRating` `0x60ddb0`).
        run(F::PlayerCombatRating, Player, 1231, 25),
        // `+0x10ec + 0x64`, three slots of seven (`GetArenaTeam` `0x5a3860`).
        run(F::PlayerArenaTeamInfo, Player, 1256, 21),
        // `+0x1144`/`+0x1148` (`GetHonorCurrency` `0x60fc40`, `GetArenaCurrency` `0x60fcc0`).
        one(F::PlayerHonorCurrency, Player, 1277),
        one(F::PlayerArenaCurrency, Player, 1278),
        // `UNIT_END + 0x485`, read at `+0x1214 + 4 * type` (`GetRuneCooldown` `0x613092`).
        run(F::PlayerRuneRegen, Player, 1305, 4),
        // `UNIT_END + 0x48c`, `+0x492` and `+0x498`: the player block's `+0x1230`, `+0x1248` and
        // `+0x1260` (`Spell_C_HandleGlyphClick` `0x80c3e0`).
        run(F::PlayerGlyphSlots, Player, 1312, 6),
        run(F::PlayerGlyphs, Player, 1318, 6),
        one(F::PlayerGlyphsEnabled, Player, 1324),
        run(F::GameObjectCreatedBy, GameObject, 6, 2),
        one(F::GameObjectDisplayId, GameObject, 8),
        one(F::GameObjectFlags, GameObject, 9),
        one(F::GameObjectDynamic, GameObject, 14),
        one(F::GameObjectFaction, GameObject, 15),
        one(F::GameObjectLevel, GameObject, 16),
        one(F::GameObjectBytes1, GameObject, 17),
        run(F::DynamicObjectCaster, DynamicObject, 6, 2),
        one(F::DynamicObjectBytes, DynamicObject, 8),
        one(F::DynamicObjectSpellId, DynamicObject, 9),
        one(F::DynamicObjectRadius, DynamicObject, 10),
        run(F::CorpseOwner, Corpse, 6, 2),
        one(F::CorpseDisplayId, Corpse, 10),
        run(F::CorpseItems, Corpse, 11, 19),
        one(F::CorpseBytes1, Corpse, 30),
        one(F::CorpseBytes2, Corpse, 31),
        one(F::CorpseGuild, Corpse, 32),
        one(F::CorpseFlags, Corpse, 33),
        one(F::CorpseDynamicFlags, Corpse, 34),
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    fn block_end(layout: &UpdateFieldLayout, block: Block) -> u16 {
        let e = &layout.ends;
        match block {
            Block::Object => e.object,
            Block::Item => e.item,
            Block::Container => e.container,
            Block::Unit => e.unit,
            Block::Player => e.player,
            Block::GameObject => e.gameobject,
            Block::DynamicObject => e.dynamicobject,
            Block::Corpse => e.corpse,
        }
    }

    fn block_start(layout: &UpdateFieldLayout, block: Block) -> u16 {
        let e = &layout.ends;
        match block {
            Block::Object => 0,
            Block::Item | Block::Unit | Block::GameObject | Block::DynamicObject => e.object,
            Block::Corpse => e.object,
            Block::Container => e.item,
            Block::Player => e.unit,
        }
    }

    #[test]
    fn every_place_lies_inside_its_block_and_names_its_field_once() {
        for layout in [&SHARED, &WIRE] {
            for (i, p) in layout.places.iter().enumerate() {
                let last = p.element(p.count - 1).unwrap();
                assert!(
                    p.index >= block_start(layout, p.block) && last < block_end(layout, p.block),
                    "{:?} outside {:?}",
                    p.field,
                    p.block
                );
                assert!(
                    layout.places[..i].iter().all(|q| q.field != p.field),
                    "{:?} twice",
                    p.field
                );
            }
        }
    }

    #[test]
    fn no_two_places_of_one_block_overlap() {
        for layout in [&SHARED, &WIRE] {
            let mut taken = std::collections::HashMap::new();
            for p in layout.places {
                for i in 0..p.count {
                    let index = p.element(i).unwrap();
                    if let Some(other) = taken.insert((p.block, index), p.field) {
                        panic!("{:?} and {:?} share {index}", other, p.field);
                    }
                }
            }
        }
    }

    #[test]
    fn shared_fields_keep_their_element_count_where_the_meaning_does() {
        // A wire field maps element for element, so the counts must agree.
        for p in WIRE.places {
            if let Some(c) = SHARED.place(p.field) {
                assert_eq!(p.count, c.count, "{:?}", p.field);
            }
        }
    }

    #[test]
    fn each_layout_keeps_health_and_the_player_block_where_its_header_says() {
        assert_eq!(SHARED.place(Field::UnitHealth).unwrap().index, 22);
        assert_eq!(WIRE.place(Field::UnitHealth).unwrap().index, 24);
        assert_eq!(SHARED.ends.unit, 188);
        assert_eq!(WIRE.ends.unit, 148);
        assert_eq!(WIRE.ends.player, 1326);
        assert_eq!(
            WIRE.place(Field::PlayerVisibleItemEntry)
                .unwrap()
                .element(18),
            Some(319)
        );
        assert_eq!(
            SHARED
                .place(Field::PlayerVisibleItemEntry)
                .unwrap()
                .element(1),
            Some(272)
        );
    }

    #[test]
    fn fields_the_wire_does_not_carry_are_missing_not_borrowed() {
        for f in [
            Field::UnitAura,
            Field::UnitVirtualItemDisplay,
            Field::PlayerComboTarget,
            Field::GameObjectRotation,
            Field::GameObjectPosition,
            Field::DynamicObjectPosition,
        ] {
            assert!(SHARED.place(f).is_some(), "{f:?}");
            assert!(WIRE.place(f).is_none(), "{f:?}");
        }
        assert!(SHARED.place(Field::GameObjectBytes1).is_none());
        assert_eq!(WIRE.place(Field::GameObjectBytes1).unwrap().index, 17);
    }
}
