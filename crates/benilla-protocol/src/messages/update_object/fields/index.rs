//! 3.3.5a (12340) update-field indices, as the server sends them and the client mirrors them: each
//! counts from its object's descriptor start, so a block's offset in the reference's descriptor
//! tables (`0xac5c48` object, `0xac5ce8` item, `0xac5cac` container, `0xac5fe0` unit, `0xac66d8`
//! player, `0xac77a8` gameobject, `0xac7848` dynamic object, `0xac78b0` corpse) is added to the
//! `*_END` of the blocks before it. The same index names different fields on different blocks.

/// `OBJECT_END`: the object block is 6 dwords.
pub const OBJECT_END: u16 = 6;
/// `ITEM_END`: 58 item dwords.
pub const ITEM_END: u16 = OBJECT_END + 58;
/// `CONTAINER_END`: 74 container dwords.
pub const CONTAINER_END: u16 = ITEM_END + 74;
/// `UNIT_END`: 142 unit dwords.
pub const UNIT_END: u16 = OBJECT_END + 142;
/// `PLAYER_END`: 1178 player dwords.
pub const PLAYER_END: u16 = UNIT_END + 1178;
/// `GAMEOBJECT_END`: 12 gameobject dwords.
pub const GAMEOBJECT_END: u16 = OBJECT_END + 12;
/// `DYNAMICOBJECT_END`: 6 dynamic-object dwords.
pub const DYNAMICOBJECT_END: u16 = OBJECT_END + 6;
/// `CORPSE_END`: 30 corpse dwords.
pub const CORPSE_END: u16 = OBJECT_END + 30;

pub const FIELD_OBJECT_GUID: u16 = 0;
pub const FIELD_OBJECT_TYPE: u16 = 2;
pub const FIELD_OBJECT_ENTRY: u16 = 3;
pub const FIELD_OBJECT_SCALE_X: u16 = 4;

pub const FIELD_ITEM_OWNER: u16 = OBJECT_END;
pub const FIELD_ITEM_CONTAINED: u16 = OBJECT_END + 0x2;
pub const FIELD_ITEM_CREATOR: u16 = OBJECT_END + 0x4;
pub const FIELD_ITEM_GIFTCREATOR: u16 = OBJECT_END + 0x6;
pub const FIELD_ITEM_STACK_COUNT: u16 = OBJECT_END + 0x8;
pub const FIELD_ITEM_DURATION: u16 = OBJECT_END + 0x9;
/// Five signed charge counts, one per item spell.
pub const FIELD_ITEM_SPELL_CHARGES: u16 = OBJECT_END + 0xa;
/// `0x1` bound, `0x8` wrapped, `0x100` trade window open, `0x200` carries text.
pub const FIELD_ITEM_FLAGS: u16 = OBJECT_END + 0xf;
/// Twelve (id, duration, charges) triples: permanent, temporary, three sockets, socket bonus,
/// prismatic, five random-property slots.
pub const FIELD_ITEM_ENCHANTMENT_1_1: u16 = OBJECT_END + 0x10;
pub const FIELD_ITEM_PROPERTY_SEED: u16 = OBJECT_END + 0x34;
pub const FIELD_ITEM_RANDOM_PROPERTIES_ID: u16 = OBJECT_END + 0x35;
pub const FIELD_ITEM_DURABILITY: u16 = OBJECT_END + 0x36;
pub const FIELD_ITEM_MAXDURABILITY: u16 = OBJECT_END + 0x37;
pub const FIELD_ITEM_CREATE_PLAYED_TIME: u16 = OBJECT_END + 0x38;

pub const FIELD_CONTAINER_NUM_SLOTS: u16 = ITEM_END;
/// 36 item guids.
pub const FIELD_CONTAINER_SLOT_1: u16 = ITEM_END + 0x2;

pub const FIELD_UNIT_CHARM: u16 = OBJECT_END;
/// On our own descriptor, the `"pet"` unit.
pub const FIELD_UNIT_SUMMON: u16 = OBJECT_END + 0x2;
/// The summoned companion.
pub const FIELD_UNIT_CRITTER: u16 = OBJECT_END + 0x4;
pub const FIELD_UNIT_CHARMEDBY: u16 = OBJECT_END + 0x6;
pub const FIELD_UNIT_SUMMONEDBY: u16 = OBJECT_END + 0x8;
pub const FIELD_UNIT_CREATEDBY: u16 = OBJECT_END + 0xa;
/// The client turns an idle unit to face it; no packet carries that facing.
pub const FIELD_UNIT_TARGET: u16 = OBJECT_END + 0xc;
/// A channel's target, possibly the caster; public, so another unit's channel renders from it.
pub const FIELD_UNIT_CHANNEL_OBJECT: u16 = OBJECT_END + 0xe;
pub const FIELD_UNIT_CHANNEL_SPELL: u16 = OBJECT_END + 0x10;
/// Race, class, gender and power type, bytes 0 to 3.
pub const FIELD_UNIT_BYTES_0: u16 = OBJECT_END + 0x11;
pub const FIELD_UNIT_HEALTH: u16 = OBJECT_END + 0x12;
/// Seven powers, each read at `POWER1 + type` (`0x71c2e0`).
pub const FIELD_UNIT_POWER1: u16 = OBJECT_END + 0x13;
pub const FIELD_UNIT_MAXHEALTH: u16 = OBJECT_END + 0x1a;
/// Seven maxima, read at `MAXPOWER1 + type` (`0x60ef40`).
pub const FIELD_UNIT_MAXPOWER1: u16 = OBJECT_END + 0x1b;
/// Seven floats; `GetManaRegen` reads power 0 at unit block `+0x88` (`0x4f5390`).
pub const FIELD_UNIT_POWER_REGEN_FLAT_MODIFIER: u16 = OBJECT_END + 0x22;
/// Seven floats, read at unit block `+0xa4` (`0x4f5390`).
pub const FIELD_UNIT_POWER_REGEN_INTERRUPTED_FLAT_MODIFIER: u16 = OBJECT_END + 0x29;
pub const FIELD_UNIT_LEVEL: u16 = OBJECT_END + 0x30;
pub const FIELD_UNIT_FACTIONTEMPLATE: u16 = OBJECT_END + 0x31;
/// A creature's three `Item.dbc` ids, main hand, off hand, ranged (`0x725010`).
pub const FIELD_UNIT_VIRTUAL_ITEM_SLOT_ID: u16 = OBJECT_END + 0x32;
pub const FIELD_UNIT_FLAGS: u16 = OBJECT_END + 0x35;
pub const FIELD_UNIT_FLAGS_2: u16 = OBJECT_END + 0x36;
/// Aura-state bits, tested as `1 << (state - 1)` by the usable check.
pub const FIELD_UNIT_AURASTATE: u16 = OBJECT_END + 0x37;
/// Main hand, off hand, in ms.
pub const FIELD_UNIT_BASEATTACKTIME: u16 = OBJECT_END + 0x38;
pub const FIELD_UNIT_RANGEDATTACKTIME: u16 = OBJECT_END + 0x3a;
pub const FIELD_UNIT_BOUNDINGRADIUS: u16 = OBJECT_END + 0x3b;
pub const FIELD_UNIT_COMBATREACH: u16 = OBJECT_END + 0x3c;
pub const FIELD_UNIT_DISPLAYID: u16 = OBJECT_END + 0x3d;
/// The appearance no form or morph touches; the client sizes the mover collision box from it.
pub const FIELD_UNIT_NATIVEDISPLAYID: u16 = OBJECT_END + 0x3e;
/// The ridden mount's `CreatureDisplayInfo` id, 0 when unmounted.
pub const FIELD_UNIT_MOUNTDISPLAYID: u16 = OBJECT_END + 0x3f;
pub const FIELD_UNIT_MINDAMAGE: u16 = OBJECT_END + 0x40;
pub const FIELD_UNIT_MAXDAMAGE: u16 = OBJECT_END + 0x41;
pub const FIELD_UNIT_MINOFFHANDDAMAGE: u16 = OBJECT_END + 0x42;
pub const FIELD_UNIT_MAXOFFHANDDAMAGE: u16 = OBJECT_END + 0x43;
/// Stand state, pet talents, visibility flags and anim tier, bytes 0 to 3.
pub const FIELD_UNIT_BYTES_1: u16 = OBJECT_END + 0x44;
/// Nonzero on a pet or charm.
pub const FIELD_UNIT_PETNUMBER: u16 = OBJECT_END + 0x45;
/// Changes on a pet rename, staling the name cache.
pub const FIELD_UNIT_PET_NAME_TIMESTAMP: u16 = OBJECT_END + 0x46;
pub const FIELD_UNIT_PETEXPERIENCE: u16 = OBJECT_END + 0x47;
pub const FIELD_UNIT_PETNEXTLEVELEXP: u16 = OBJECT_END + 0x48;
/// Per viewer: `0x1` lootable, `0x4` tapped, `0x8` tapped by me, `0x20` reads dead.
pub const FIELD_UNIT_DYNAMIC_FLAGS: u16 = OBJECT_END + 0x49;
pub const FIELD_UNIT_MOD_CAST_SPEED: u16 = OBJECT_END + 0x4a;
pub const FIELD_UNIT_CREATED_BY_SPELL: u16 = OBJECT_END + 0x4b;
pub const FIELD_UNIT_NPC_FLAGS: u16 = OBJECT_END + 0x4c;
pub const FIELD_UNIT_NPC_EMOTESTATE: u16 = OBJECT_END + 0x4d;
/// Five primary stats after buffs.
pub const FIELD_UNIT_STAT0: u16 = OBJECT_END + 0x4e;
/// Five positive stat buff totals, integers.
pub const FIELD_UNIT_POSSTAT0: u16 = OBJECT_END + 0x53;
/// Five negative stat buff totals, integers at or below 0.
pub const FIELD_UNIT_NEGSTAT0: u16 = OBJECT_END + 0x58;
/// Seven resistances, `[0]` armor.
pub const FIELD_UNIT_RESISTANCES: u16 = OBJECT_END + 0x5d;
pub const FIELD_UNIT_RESISTANCEBUFFMODSPOSITIVE: u16 = OBJECT_END + 0x64;
pub const FIELD_UNIT_RESISTANCEBUFFMODSNEGATIVE: u16 = OBJECT_END + 0x6b;
pub const FIELD_UNIT_BASE_MANA: u16 = OBJECT_END + 0x72;
pub const FIELD_UNIT_BASE_HEALTH: u16 = OBJECT_END + 0x73;
/// Sheath state, `UNIT_BYTE2_FLAG`s, pet flags and shapeshift form, bytes 0 to 3 (`0x71af70`).
pub const FIELD_UNIT_BYTES_2: u16 = OBJECT_END + 0x74;
pub const FIELD_UNIT_ATTACK_POWER: u16 = OBJECT_END + 0x75;
/// `TWO_SHORT`: the positive mod in the low half, the negative in the high.
pub const FIELD_UNIT_ATTACK_POWER_MODS: u16 = OBJECT_END + 0x76;
pub const FIELD_UNIT_ATTACK_POWER_MULTIPLIER: u16 = OBJECT_END + 0x77;
pub const FIELD_UNIT_RANGED_ATTACK_POWER: u16 = OBJECT_END + 0x78;
pub const FIELD_UNIT_RANGED_ATTACK_POWER_MODS: u16 = OBJECT_END + 0x79;
pub const FIELD_UNIT_RANGED_ATTACK_POWER_MULTIPLIER: u16 = OBJECT_END + 0x7a;
pub const FIELD_UNIT_MINRANGEDDAMAGE: u16 = OBJECT_END + 0x7b;
pub const FIELD_UNIT_MAXRANGEDDAMAGE: u16 = OBJECT_END + 0x7c;
/// Seven schools, integers; the cost lookup reads unit block `+0x1f4` (`0x5ee050`).
pub const FIELD_UNIT_POWER_COST_MODIFIER: u16 = OBJECT_END + 0x7d;
/// Seven schools, floats less one; read at unit block `+0x210` (`0x5ee0b0`).
pub const FIELD_UNIT_POWER_COST_MULTIPLIER: u16 = OBJECT_END + 0x84;
/// Read at unit block `+0x22c` (`0x612870`).
pub const FIELD_UNIT_MAXHEALTHMODIFIER: u16 = OBJECT_END + 0x8b;
pub const FIELD_UNIT_HOVERHEIGHT: u16 = OBJECT_END + 0x8c;

/// The client's player block base `[player+0xe68]`: the arbiter at `+0x0`, the flags at `+0x8`
/// and the duel team at `+0x20`, the three reads of the reaction's duel leg.
pub const FIELD_PLAYER_DUEL_ARBITER: u16 = UNIT_END;
/// Bit `0x10` is the ghost, `0x400`/`0x800` hide helm and cloak.
pub const FIELD_PLAYER_FLAGS: u16 = UNIT_END + 0x2;
pub const FIELD_PLAYER_GUILDID: u16 = UNIT_END + 0x3;
pub const FIELD_PLAYER_GUILDRANK: u16 = UNIT_END + 0x4;
/// Skin, face, hair style and hair colour, bytes 0 to 3.
pub const FIELD_PLAYER_BYTES: u16 = UNIT_END + 0x5;
/// Facial hair, unused, bank bag slots bought and rest state, bytes 0 to 3.
pub const FIELD_PLAYER_BYTES_2: u16 = UNIT_END + 0x6;
/// Gender, drunkenness, PvP title and arena faction, bytes 0 to 3.
pub const FIELD_PLAYER_BYTES_3: u16 = UNIT_END + 0x7;
pub const FIELD_PLAYER_DUEL_TEAM: u16 = UNIT_END + 0x8;
/// Read at player block `+0x24` by its watch (`0x6e1c90`).
pub const FIELD_PLAYER_GUILD_TIMESTAMP: u16 = UNIT_END + 0x9;
/// 25 slots of [`QUEST_LOG_SLOT_FIELDS`]: 12340 reads slot `i` at `(i*5 + 10)*4` into the player
/// block (`0x5e1561`), the counters as `u16` at `+8` (`0x5e161c`).
pub const FIELD_PLAYER_QUEST_LOG_1_1: u16 = UNIT_END + 0xa;
/// 19 slots of [`VISIBLE_ITEM_SLOT_FIELDS`]: the item entry, then the permanent and temporary
/// enchant as two `u16`.
pub const FIELD_PLAYER_VISIBLE_ITEM_1_ENTRYID: u16 = UNIT_END + 0x87;
/// Read at player block `+0x2b4` (`0x51b3b0`).
pub const FIELD_PLAYER_CHOSEN_TITLE: u16 = UNIT_END + 0xad;
/// Read at player block `+0x2b8` beside the drunk byte (`0x4ffd88`, `0x4f7290`).
pub const FIELD_PLAYER_FAKE_INEBRIATION: u16 = UNIT_END + 0xae;
/// 23 guids: equipment 0-18, bags 19-22.
pub const FIELD_PLAYER_INV_SLOT_HEAD: u16 = UNIT_END + 0xb0;
/// 16 backpack guids, player slots 23-38.
pub const FIELD_PLAYER_PACK_SLOT_1: u16 = UNIT_END + 0xde;
/// 28 bank guids, player slots 39-66.
pub const FIELD_PLAYER_BANK_SLOT_1: u16 = UNIT_END + 0xfe;
/// 7 bank bag guids, player slots 67-73.
pub const FIELD_PLAYER_BANK_BAG_SLOT_1: u16 = UNIT_END + 0x136;
/// 12 buyback guids, player slots 74-85.
pub const FIELD_PLAYER_VENDORBUYBACK_SLOT_1: u16 = UNIT_END + 0x144;
/// 32 keyring guids, player slots 86-117.
pub const FIELD_PLAYER_KEYRING_SLOT_1: u16 = UNIT_END + 0x15c;
/// 32 currency-token guids, player slots 118-149 (`0x5b0680`).
pub const FIELD_PLAYER_CURRENCY_TOKEN_SLOT_1: u16 = UNIT_END + 0x19c;
/// Our view's anchor (Mind Vision, Sentry Totem), or 0.
pub const FIELD_PLAYER_FARSIGHT: u16 = UNIT_END + 0x1dc;
/// Six words, read as three `u64` at `+0x778` (`0x51b410`).
pub const FIELD_PLAYER_KNOWN_TITLES: u16 = UNIT_END + 0x1de;
/// Two words, read as one `u64` at `+0x790` (`0x6e5401`).
pub const FIELD_PLAYER_KNOWN_CURRENCIES: u16 = UNIT_END + 0x1e4;
pub const FIELD_PLAYER_XP: u16 = UNIT_END + 0x1e6;
pub const FIELD_PLAYER_NEXT_LEVEL_XP: u16 = UNIT_END + 0x1e7;
/// 128 skills × 3 dwords.
pub const FIELD_PLAYER_SKILL_INFO_1_1: u16 = UNIT_END + 0x1e8;
/// Unspent talent points, then free primary professions.
pub const FIELD_PLAYER_CHARACTER_POINTS1: u16 = UNIT_END + 0x368;
pub const FIELD_PLAYER_CHARACTER_POINTS2: u16 = UNIT_END + 0x369;
/// Bit `1 << (n - 1)` per creature type or `LockType.dbc` id `n` tracked.
pub const FIELD_PLAYER_TRACK_CREATURES: u16 = UNIT_END + 0x36a;
pub const FIELD_PLAYER_TRACK_RESOURCES: u16 = UNIT_END + 0x36b;
pub const FIELD_PLAYER_BLOCK_PERCENTAGE: u16 = UNIT_END + 0x36c;
pub const FIELD_PLAYER_DODGE_PERCENTAGE: u16 = UNIT_END + 0x36d;
pub const FIELD_PLAYER_PARRY_PERCENTAGE: u16 = UNIT_END + 0x36e;
/// Then the off hand's; read at player block `+0xdbc` (`0x612bf0`).
pub const FIELD_PLAYER_EXPERTISE: u16 = UNIT_END + 0x36f;
pub const FIELD_PLAYER_CRIT_PERCENTAGE: u16 = UNIT_END + 0x371;
pub const FIELD_PLAYER_RANGED_CRIT_PERCENTAGE: u16 = UNIT_END + 0x372;
/// Seven schools, `[0]` physical; read at `+0xdd0 + 4 * (school - 1)` (`0x60e290`).
pub const FIELD_PLAYER_SPELL_CRIT_PERCENTAGE1: u16 = UNIT_END + 0x374;
/// Read at `+0xdec` (`0x60dff0`).
pub const FIELD_PLAYER_SHIELD_BLOCK: u16 = UNIT_END + 0x37b;
/// The discovery bitset, 128 words indexed by `AreaTable.dbc` exploreFlag.
pub const FIELD_PLAYER_EXPLORED_ZONES_1: u16 = UNIT_END + 0x37d;
/// The rested pool in base kill-XP units.
pub const FIELD_PLAYER_REST_STATE_EXPERIENCE: u16 = UNIT_END + 0x3fd;
pub const FIELD_PLAYER_FIELD_COINAGE: u16 = UNIT_END + 0x3fe;
pub const FIELD_PLAYER_MOD_DAMAGE_DONE_POS: u16 = UNIT_END + 0x3ff;
pub const FIELD_PLAYER_MOD_DAMAGE_DONE_NEG: u16 = UNIT_END + 0x406;
pub const FIELD_PLAYER_MOD_DAMAGE_DONE_PCT: u16 = UNIT_END + 0x40d;
/// Read at `+0x1050` (`0x60e3b0`).
pub const FIELD_PLAYER_MOD_HEALING_DONE_POS: u16 = UNIT_END + 0x414;
/// `+0x1054`/`+0x1058`, the spell-text operands `pbh` and `pbhd` (`0x5782d0`).
pub const FIELD_PLAYER_MOD_HEALING_PCT: u16 = UNIT_END + 0x415;
pub const FIELD_PLAYER_MOD_HEALING_DONE_PCT: u16 = UNIT_END + 0x416;
/// Read at `+0x105c` (`0x60e470`).
pub const FIELD_PLAYER_MOD_TARGET_RESISTANCE: u16 = UNIT_END + 0x417;
/// Flags, Refer-a-Friend levels, action bar toggles and the lifetime highest honor rank, bytes 0
/// to 3.
pub const FIELD_PLAYER_FIELD_BYTES: u16 = UNIT_END + 0x419;
pub const FIELD_PLAYER_AMMO_ID: u16 = UNIT_END + 0x41a;
pub const FIELD_PLAYER_SELF_RES_SPELL: u16 = UNIT_END + 0x41b;
pub const FIELD_PLAYER_FIELD_PVP_MEDALS: u16 = UNIT_END + 0x41c;
/// 12 prices in copper, indexed by buyback slot.
pub const FIELD_PLAYER_BUYBACK_PRICE_1: u16 = UNIT_END + 0x41d;
/// 12 expiries, the client's sort key only.
pub const FIELD_PLAYER_BUYBACK_TIMESTAMP_1: u16 = UNIT_END + 0x429;
/// `TWO_SHORT`: today's honorable kills, then yesterday's; read at `+0x1080` (`0x611a20`).
pub const FIELD_PLAYER_FIELD_KILLS: u16 = UNIT_END + 0x435;
pub const FIELD_PLAYER_FIELD_TODAY_CONTRIBUTION: u16 = UNIT_END + 0x436;
pub const FIELD_PLAYER_FIELD_YESTERDAY_CONTRIBUTION: u16 = UNIT_END + 0x437;
pub const FIELD_PLAYER_FIELD_LIFETIME_HONORABLE_KILLS: u16 = UNIT_END + 0x438;
/// The `OverrideSpellData` id as the low `i16` (`0x5abb40`), then the regen prediction mask and
/// the aura vision (`+0x10e7`), bytes 2 and 3.
pub const FIELD_PLAYER_FIELD_BYTES2: u16 = UNIT_END + 0x439;
/// Signed: slot 0 is a real faction, so only -1 means none.
pub const FIELD_PLAYER_WATCHED_FACTION_INDEX: u16 = UNIT_END + 0x43a;
/// 25 ratings; read at `+0x10ec + 4 * index` (`0x60ddb0`).
pub const FIELD_PLAYER_COMBAT_RATING_1: u16 = UNIT_END + 0x43b;
/// Three slots of [`ARENA_TEAM_INFO_STRIDE`] words (`0x5a3860`).
pub const FIELD_PLAYER_FIELD_ARENA_TEAM_INFO_1_1: u16 = UNIT_END + 0x454;
/// Read at `+0x1144` (`0x60fc40`).
pub const FIELD_PLAYER_FIELD_HONOR_CURRENCY: u16 = UNIT_END + 0x469;
/// Read at `+0x1148` (`0x60fcc0`).
pub const FIELD_PLAYER_FIELD_ARENA_CURRENCY: u16 = UNIT_END + 0x46a;
/// The cap `GetQuestLogRewardXP` tests (`+0x11ac`, `0x5e4a21`).
pub const FIELD_PLAYER_MAX_LEVEL: u16 = UNIT_END + 0x46b;
/// 25 quest ids `GetDailyQuestsCompleted` counts (`+0x11b0`, `0x58db5e`).
pub const FIELD_PLAYER_DAILY_QUESTS_1: u16 = UNIT_END + 0x46c;
/// Four floats, read at `+0x1214 + 4 * type` (`0x613092`).
pub const FIELD_PLAYER_RUNE_REGEN_1: u16 = UNIT_END + 0x485;
/// Six `GlyphSlot.dbc` ids, at player block `+0x1230` (`0x80c3e0`).
pub const FIELD_PLAYER_GLYPH_SLOTS_1: u16 = UNIT_END + 0x48c;
/// Six `GlyphProperties.dbc` ids, at `+0x1248` (`0x80c3e0`).
pub const FIELD_PLAYER_GLYPHS_1: u16 = UNIT_END + 0x492;
/// Bit `i` while socket `i` is unlocked, at `+0x1260` (`0x80c3e0`).
pub const FIELD_PLAYER_GLYPHS_ENABLED: u16 = UNIT_END + 0x498;
/// The last player field, `[player+0x1264]`.
pub const FIELD_PLAYER_PET_SPELL_POWER: u16 = UNIT_END + 0x499;

/// A spell-spawned object's creator (bobber, ritual portal); 0 for a world spawn.
pub const FIELD_GAMEOBJECT_CREATED_BY: u16 = OBJECT_END;
pub const FIELD_GAMEOBJECT_DISPLAYID: u16 = OBJECT_END + 0x2;
pub const FIELD_GAMEOBJECT_FLAGS: u16 = OBJECT_END + 0x3;
/// The transport parent's rotation, four floats; not the spawn's own, which rides the movement
/// block.
pub const FIELD_GAMEOBJECT_PARENTROTATION: u16 = OBJECT_END + 0x4;
/// `TWO_SHORT`: the per-viewer dynamic flags in the low half, path progress in the high.
pub const FIELD_GAMEOBJECT_DYNAMIC: u16 = OBJECT_END + 0x8;
/// The `FactionTemplate.dbc` id; hostile to the player means not highlightable.
pub const FIELD_GAMEOBJECT_FACTION: u16 = OBJECT_END + 0x9;
pub const FIELD_GAMEOBJECT_LEVEL: u16 = OBJECT_END + 0xa;
/// State, type, art kit and anim progress, bytes 0 to 3.
pub const FIELD_GAMEOBJECT_BYTES_1: u16 = OBJECT_END + 0xb;

pub const FIELD_DYNAMICOBJECT_CASTER: u16 = OBJECT_END;
pub const FIELD_DYNAMICOBJECT_BYTES: u16 = OBJECT_END + 0x2;
pub const FIELD_DYNAMICOBJECT_SPELLID: u16 = OBJECT_END + 0x3;
pub const FIELD_DYNAMICOBJECT_RADIUS: u16 = OBJECT_END + 0x4;
/// The server clock of the cast that made it (`0x804cc0`).
pub const FIELD_DYNAMICOBJECT_CASTTIME: u16 = OBJECT_END + 0x5;

/// The dead player.
pub const FIELD_CORPSE_OWNER: u16 = OBJECT_END;
pub const FIELD_CORPSE_PARTY: u16 = OBJECT_END + 0x2;
pub const FIELD_CORPSE_DISPLAY_ID: u16 = OBJECT_END + 0x4;
/// 19 equipment slots, each `display | inventory_type << 24`.
pub const FIELD_CORPSE_ITEM: u16 = OBJECT_END + 0x5;
/// Unused, race, gender and skin, bytes 0 to 3.
pub const FIELD_CORPSE_BYTES_1: u16 = OBJECT_END + 0x18;
/// Face, hair style, hair colour and facial hair, bytes 0 to 3.
pub const FIELD_CORPSE_BYTES_2: u16 = OBJECT_END + 0x19;
pub const FIELD_CORPSE_GUILD: u16 = OBJECT_END + 0x1a;
/// `0x1` bones, `0x8`/`0x10` hide helm and cloak, `0x20`
/// insignia takeable.
pub const FIELD_CORPSE_FLAGS: u16 = OBJECT_END + 0x1b;
/// Bit 0 is lootable insignia. On a unit this index is `UNIT_FIELD_MAXPOWER2`, so a field edge
/// carries its object class.
pub const FIELD_CORPSE_DYNAMIC_FLAGS: u16 = OBJECT_END + 0x1c;

/// The fields of one quest-log slot: id, state, two words of counters, timer.
pub const QUEST_LOG_SLOT_FIELDS: u16 = 5;
/// The fields of one visible-item slot: entry, enchants.
pub const VISIBLE_ITEM_SLOT_FIELDS: u16 = 2;
/// The words per arena-team slot in `PLAYER_FIELD_ARENA_TEAM_INFO_1_1`.
pub const ARENA_TEAM_INFO_STRIDE: u16 = 7;

/// `PLAYER_VISIBLE_ITEM_<slot+1>_ENTRYID`, the public item entry worn in equipment slot `slot`.
pub const fn player_visible_item_entry_field(slot: u8) -> u16 {
    FIELD_PLAYER_VISIBLE_ITEM_1_ENTRYID + VISIBLE_ITEM_SLOT_FIELDS * slot as u16
}

/// `PLAYER_FIELD_INV_SLOT_HEAD + 2 * slot`, the low word of player slot `slot`'s item guid.
pub const fn player_inv_slot_field(slot: u8) -> u16 {
    FIELD_PLAYER_INV_SLOT_HEAD + 2 * slot as u16
}
