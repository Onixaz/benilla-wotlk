//! The 1.12.1 world opcode numbers (vmangos `Opcodes_1_12_1.h`). A client packet's header holds
//! the opcode in 4 bytes, but every value fits `u16`; the senders widen it.

pub const SMSG_CHAR_CREATE: u16 = 0x003A;
pub const SMSG_CHAR_ENUM: u16 = 0x003B;
// Character delete: CMSG a full u64 guid, SMSG one result byte (`CHAR_DELETE_SUCCESS` = 0x39).
pub const CMSG_CHAR_DELETE: u16 = 0x0038;
pub const SMSG_CHAR_DELETE: u16 = 0x003C;
/// The server's refusal of a `CMSG_PLAYER_LOGIN`: one result byte.
pub const SMSG_CHARACTER_LOGIN_FAILED: u16 = 0x0041;
pub const SMSG_NAME_QUERY_RESPONSE: u16 = 0x0051; // 81
pub const SMSG_CREATURE_QUERY_RESPONSE: u16 = 0x0061; // 97
/// Answers `CMSG_PET_NAME_QUERY`: `u32 petNumber`, cstring name, `u32 nameTimestamp`. No reply
/// comes unless the guid is a live pet with that number (`PetHandler.cpp:190-192`).
pub const SMSG_PET_NAME_QUERY_RESPONSE: u16 = 0x0053; // 83
/// Answers `CMSG_GAMEOBJECT_QUERY` with the GameObject's template.
pub const SMSG_GAMEOBJECT_QUERY_RESPONSE: u16 = 0x005F; // 95
/// Answers `CMSG_QUEST_QUERY` with the quest's template.
pub const SMSG_QUEST_QUERY_RESPONSE: u16 = 0x005D; // 93
/// Answers `CMSG_QUEST_POI_QUERY` with each quest's POI regions.
pub const SMSG_QUEST_POI_QUERY_RESPONSE: u16 = 0x01E4; // 484

// Page text: the ask-once `PageText.wdb` cache behind readable items and book GameObjects; an id
// names one page, chained by `nextPageId`. Copied mail letters use `CMSG_ITEM_TEXT_QUERY` instead.
pub const CMSG_PAGE_TEXT_QUERY: u16 = 0x005A; // 90
pub const SMSG_PAGE_TEXT_QUERY_RESPONSE: u16 = 0x005B; // 91

pub const SMSG_NEW_WORLD: u16 = 0x003E;
// The far-teleport preamble: with `SMSG_TRANSFER_PENDING`'s optional transport block, the next
// `SMSG_NEW_WORLD`'s coordinates are transport-local, else world (`Player.cpp:2065-2068`).
pub const SMSG_TRANSFER_PENDING: u16 = 0x003F;
pub const SMSG_TRANSFER_ABORTED: u16 = 0x0040;
pub const SMSG_LOGIN_SETTIMESPEED: u16 = 0x0042;
pub const SMSG_BINDPOINTUPDATE: u16 = 0x0155;
/// The proficiency mask for one item class (vmangos `Skill.h`).
pub const SMSG_SET_PROFICIENCY: u16 = 0x0127;
pub const SMSG_LOGOUT_RESPONSE: u16 = 0x004C;
pub const SMSG_LOGOUT_COMPLETE: u16 = 0x004D;
/// The ack for [`CMSG_LOGOUT_CANCEL`]; empty body.
pub const SMSG_LOGOUT_CANCEL_ACK: u16 = 0x004F;
pub const SMSG_UPDATE_OBJECT: u16 = 0x00A9;
pub const SMSG_DESTROY_OBJECT: u16 = 0x00AA;
// Cinematics: a first login's race intro, or a type-13 GameObject camera. Until the COMPLETE ack,
// vmangos sees from the camera and despawns all around the body (`Player::UpdateCinematic`).
/// Lua's `OpeningCinematic()` (`0x48c8c0`); empty body. vmangos does not handle it.
pub const CMSG_OPENING_CINEMATIC: u16 = 0x00F9;
pub const SMSG_TRIGGER_CINEMATIC: u16 = 0x00FA;
/// The next camera of a multi-camera `CinematicSequences` row began; empty body (`0x48efe0`).
pub const CMSG_NEXT_CINEMATIC_CAMERA: u16 = 0x00FB;
pub const CMSG_COMPLETE_CINEMATIC: u16 = 0x00FC;
pub const SMSG_MONSTER_MOVE: u16 = 0x00DD;
/// [`SMSG_MONSTER_MOVE`] for a unit on a transport: the transport's packed guid follows the
/// mover's, and every coordinate is deck-local (`MoveSplineInit.cpp:146-154`).
pub const SMSG_MONSTER_MOVE_TRANSPORT: u16 = 0x02AE; // 686
pub const SMSG_INITIALIZE_FACTIONS: u16 = 0x0122;
/// A faction became visible in the reputation pane: one `u32` list slot. It is the only notice of
/// `FACTION_FLAG_VISIBLE`; no fresh standing comes with it (`ReputationMgr::SendVisible`).
pub const SMSG_SET_FACTION_VISIBLE: u16 = 0x0123;
pub const SMSG_SET_FACTION_STANDING: u16 = 0x0124;
/// Set a faction's at-war flag from the reputation pane; none of the pane's three verbs is acked.
pub const CMSG_SET_FACTION_ATWAR: u16 = 0x0125; // 293
pub const CMSG_SET_FACTION_INACTIVE: u16 = 0x0317; // 791
pub const CMSG_SET_WATCHED_FACTION: u16 = 0x0318; // 792
pub const SMSG_AUTH_CHALLENGE: u16 = 0x01EC;
pub const SMSG_AUTH_RESPONSE: u16 = 0x01EE;
/// The Warden anticheat challenge; vmangos kicks a client that leaves it unanswered for 30 s
/// (`Warden::Update`). Deviation: benilla has no Warden, so the connect refuses such a server
/// ([`crate::WardenRequired`]) because a session there would only be kicked.
pub const SMSG_WARDEN_DATA: u16 = 0x02E6;
/// The answer to `CMSG_AUTH_SESSION`'s addon block: one record per `## Secure:` addon, in order,
/// with no count or names.
pub const SMSG_ADDON_INFO: u16 = 0x02EF;
pub const SMSG_COMPRESSED_UPDATE_OBJECT: u16 = 0x01F6;
/// A zlib envelope of whole movement packets. Routine: vmangos switches a session to it after 300
/// movement packets in ten seconds (`Compression.Movement.Count`).
pub const SMSG_COMPRESSED_MOVES: u16 = 0x02FB;
pub const SMSG_LOGIN_VERIFY_WORLD: u16 = 0x0236;
/// The account's tutorial bits: the whole body (32 bytes from vmangos after login), copied into
/// both of the client's banks. No tutorial fires before it arrives.
pub const SMSG_TUTORIAL_FLAGS: u16 = 0x00FD; // 253
/// `FlagTutorial(n)` and the client's six auto-acknowledge sites: one `u32`, the 0-based id.
pub const CMSG_TUTORIAL_FLAG: u16 = 0x00FE; // 254
/// `ClearTutorials()`: empty body; every bit is set locally and on the server.
pub const CMSG_TUTORIAL_CLEAR: u16 = 0x00FF; // 255
/// `ResetTutorials()`: empty body; every bit is cleared locally and on the server.
pub const CMSG_TUTORIAL_RESET: u16 = 0x0100; // 256
pub const SMSG_PLAY_MUSIC: u16 = 0x0277;
pub const SMSG_PLAY_OBJECT_SOUND: u16 = 0x0278;
pub const SMSG_PLAY_SOUND: u16 = 0x02D2;
/// The zone's weather, pushed by the server.
pub const SMSG_WEATHER: u16 = 0x02F4;
pub const SMSG_EMOTE: u16 = 0x0103;
pub const CMSG_TEXT_EMOTE: u16 = 0x0104;
/// An `Emotes.dbc` animation: one `u32` emote id.
pub const CMSG_EMOTE: u16 = 0x0102;
pub const SMSG_TEXT_EMOTE: u16 = 0x0105;
/// Answers `CMSG_ITEM_QUERY_SINGLE` with the item's template.
pub const SMSG_ITEM_QUERY_SINGLE_RESPONSE: u16 = 0x0058;
pub const CMSG_AUTOEQUIP_ITEM: u16 = 0x010A; // 266
pub const CMSG_AUTOSTORE_BAG_ITEM: u16 = 0x010B; // 267
pub const CMSG_SWAP_ITEM: u16 = 0x010C; // 268
pub const CMSG_SWAP_INV_ITEM: u16 = 0x010D; // 269
pub const CMSG_SPLIT_ITEM: u16 = 0x010E; // 270
pub const CMSG_DESTROYITEM: u16 = 0x0111; // 273, the popup-confirmed world-drop delete
pub const SMSG_INVENTORY_CHANGE_FAILURE: u16 = 0x0112; // 274
/// A bag was auto-equipped: one raw `u64` guid (`ItemHandler.cpp:227`). The reference raises
/// `BAG_OPEN` for it (`0x4f9410`): our own guid is container 0, a cached bag 1..10, else nothing.
pub const SMSG_OPEN_CONTAINER: u16 = 0x0113; // 275

// Trade. CMSG bodies: INITIATE u64 target, ACCEPT u32 (ignored), SET_ITEM 3×u8 (tradeSlot, bag,
// slot), CLEAR_ITEM u8, SET_GOLD u32 copper, the rest empty. SMSG: TRADE_STATUS u32 plus a
// per-status tail; TRADE_STATUS_EXTENDED a 21-byte header and 73-byte slot records.
pub const CMSG_INITIATE_TRADE: u16 = 0x0116; // 278
pub const CMSG_BEGIN_TRADE: u16 = 0x0117; // 279
pub const CMSG_BUSY_TRADE: u16 = 0x0118; // 280
pub const CMSG_IGNORE_TRADE: u16 = 0x0119; // 281
pub const CMSG_ACCEPT_TRADE: u16 = 0x011A; // 282
pub const CMSG_UNACCEPT_TRADE: u16 = 0x011B; // 283
pub const CMSG_CANCEL_TRADE: u16 = 0x011C; // 284
pub const CMSG_SET_TRADE_ITEM: u16 = 0x011D; // 285
pub const CMSG_CLEAR_TRADE_ITEM: u16 = 0x011E; // 286
pub const CMSG_SET_TRADE_GOLD: u16 = 0x011F; // 287
pub const SMSG_TRADE_STATUS: u16 = 0x0120; // 288
pub const SMSG_TRADE_STATUS_EXTENDED: u16 = 0x0121; // 289
/// Load ammo (`PLAYER_AMMO_ID`); the auto-equip sender `0x5e1480` sends ammo here. Trap: `0x0268`
/// is not decimal 268, which is `CMSG_SWAP_ITEM` (`0x010C`).
pub const CMSG_SET_AMMO: u16 = 0x0268; // 616
/// Set one action-bar button (`HandleSetActionButtonOpcode`, `MiscHandler.cpp:885`).
pub const CMSG_SET_ACTION_BUTTON: u16 = 0x0128; // 296
/// The four extra action bars' visibility: one `u8`, stored as `PLAYER_FIELD_BYTES` byte 2
/// (sent only from `0x4e771d`; `MiscHandler.cpp:923-932`).
pub const CMSG_SET_ACTIONBAR_TOGGLES: u16 = 0x02BF; // 703
pub const SMSG_ACTION_BUTTONS: u16 = 0x0129; // 297
pub const SMSG_INITIAL_SPELLS: u16 = 0x012A; // 298

// Spell-book changes after login: a spell learned, and a rank swapped in place.
pub const SMSG_LEARNED_SPELL: u16 = 0x012B; // 299
pub const SMSG_SUPERCEDED_SPELL: u16 = 0x012C; // 300

/// A spell left the book (`Player::RemoveSpell`): one `u16` id; a talent wipe sends one per rank.
pub const SMSG_REMOVED_SPELL: u16 = 0x0203; // 515
pub const SMSG_CAST_RESULT: u16 = 0x0130; // 304

/// Whether we may drive one unit: packed guid, `u8 allowMove` (`Misc.cpp:677-682`). Each packet
/// is about one unit, not a swap: ending Mind Control sends the caster `(self, 1)` then
/// `(victim, 0)`. The server drops `MSG_MOVE_*` for a new mover until [`CMSG_SET_ACTIVE_MOVER`]
/// answers, and never stops a possessed player: `allowMove = 0` on our guid is ours to enforce.
pub const SMSG_CLIENT_CONTROL_UPDATE: u16 = 0x0159; // 345
/// The whole pet bar: ten slots, react/command state, spell list, cooldowns. The 8-byte
/// guid-only form is the teardown, the only sign the bar is gone (`Player::RemovePetActionBar`).
pub const SMSG_PET_SPELLS: u16 = 0x0179; // 377
/// The state-only refresh: the same four state bytes, with no bar behind them.
pub const SMSG_PET_MODE: u16 = 0x017A; // 378
/// One reason byte for a refused pet order (the red error line).
pub const SMSG_PET_ACTION_FEEDBACK: u16 = 0x02C6; // 710
/// [`SMSG_CAST_RESULT`] for a failed pet cast, with the same `SpellCastResult` codes.
pub const SMSG_PET_CAST_FAILED: u16 = 0x0138; // 312

/// Tame, Call Pet or Revive Pet refused: one `u8` reason. The reference (`0x6e97e0`) shows
/// `ERR_TAME_FAILED` with the reason's `PETTAME_*` string, `PETTAME_UNKNOWNERROR` outside 1..=12.
pub const SMSG_PET_TAME_FAILURE: u16 = 0x0173; // 371
/// A refused pet rename: a `u32` name code, the name, and a declension flag with its forms
/// (`0x6e2fb1`).
pub const SMSG_PET_NAME_INVALID: u16 = 0x0178; // 376
/// The pet ran away at zero loyalty (`Pet.cpp:822`): empty body. The reference (`0x4bdc00`) only
/// shows `ERR_PET_BROKEN`; the bar's teardown still comes as an `SMSG_PET_SPELLS`.
pub const SMSG_PET_BROKEN: u16 = 0x02AF; // 687
/// The pet's voice: `u64 petGuid`, `u32` talk selector (`Pet.h:98`). The reference (`0x6040c0`)
/// barks `CreatureSoundData` column 28 (`PetOrder`) for 0, 27 (`PetAttack`) for 1, else nothing,
/// through the unit's one-shot voice slot.
pub const SMSG_PET_ACTION_SOUND: u16 = 0x0324; // 804
/// A dismissed pet's parting sound: `u32 creatureModelDataId`, `f32` x, y, z. The reference
/// (`0x604140`) plays that model's `CreatureSoundData` column 29 there at `z + 1.0`, with no unit.
/// vmangos never sends it.
pub const SMSG_PET_DISMISS_SOUND: u16 = 0x0325; // 805

/// Our own server-driven spline (Charge, taxi) finished: `MovementInfo` at the endpoint, the
/// `splineId`, and a float the server skips (`MoveSplineDone::ReadFromWorldPacket`).
pub const CMSG_MOVE_SPLINE_DONE: u16 = 0x02C9; // 713
/// Our movement clock skipped (a stall, a long frame): the mover and the missing milliseconds.
/// vmangos shifts its clock, and re-creates a just-boarded transport (`MovementHandler.cpp:989`).
pub const CMSG_MOVE_TIME_SKIPPED: u16 = 0x02CE; // 718
/// Another mover's clock skip, relayed: packed guid, `u32` lag (`MovementHandler.cpp:1005-1011`).
/// The reference (`0x603b40`) adds it to that unit's last wire timestamp (`CMovement+0xac`); if
/// dropped, the unit's next packet lands `lag` ms late.
pub const MSG_MOVE_TIME_SKIPPED: u16 = 0x0319; // 793
pub const SMSG_ATTACKSTART: u16 = 0x0143; // 323
pub const SMSG_ATTACKSTOP: u16 = 0x0144; // 324
pub const SMSG_ATTACKERSTATEUPDATE: u16 = 0x014A; // 330

// The server's refusals of a `CMSG_ATTACKSWING`, all with empty bodies (`Combat.cpp`). `0x147`
// NOTSTANDING is absent on purpose: the reference registers no handler for it (`0x6255b0`).
pub const SMSG_ATTACKSWING_NOTINRANGE: u16 = 0x0145; // 325
pub const SMSG_ATTACKSWING_BADFACING: u16 = 0x0146; // 326
pub const SMSG_ATTACKSWING_DEADTARGET: u16 = 0x0148; // 328
pub const SMSG_ATTACKSWING_CANT_ATTACK: u16 = 0x0149; // 329

/// The forced attack cancel: empty body (`Player::SendAttackSwingCancelAttack`). The reference
/// (`0x5e7dd0`) stops our attack and shows nothing.
pub const SMSG_CANCEL_COMBAT: u16 = 0x014E; // 334

/// A Feign Death was resisted: empty body (`Unit.cpp:9465-9471`). The reference (`0x6e9800`) only
/// shows `ERR_FEIGN_DEATH_RESISTED`.
pub const SMSG_FEIGN_DEATH_RESISTED: u16 = 0x02B4; // 692
/// A creature's aggro or alert flare.
pub const SMSG_AI_REACTION: u16 = 0x013C; // 316

pub const SMSG_SPELL_START: u16 = 0x0131; // 305
pub const SMSG_SPELL_GO: u16 = 0x0132; // 306
/// A cast failed or was interrupted, sent to the caster and its observers (handler `0x809c70`).
pub const SMSG_SPELL_FAILURE: u16 = 0x0133; // 307
pub const SMSG_PLAY_SPELL_VISUAL: u16 = 0x01F3; // 499
/// Save the tabard design, both ways; the reply is one of six results.
pub const MSG_SAVE_GUILD_EMBLEM: u16 = 0x01F1; // 497
pub const MSG_TABARDVENDOR_ACTIVATE: u16 = 0x01F2; // 498
pub const SMSG_CANCEL_AUTO_REPEAT: u16 = 0x029C; // 668
pub const SMSG_SPELL_FAILED_OTHER: u16 = 0x02A6; // 678

/// The only source of a chain beam's extra hops: the reference (`0x6e9820`) fills `unit+0xd44`,
/// which the chain `CharProc` consumes once and zeroes.
pub const SMSG_SPELL_UPDATE_CHAIN_TARGETS: u16 = 0x0330; // 816

/// A talent's flat spell modifier; the next opcode is the percent one. Both carry the same 6-byte
/// body, the absolute value of one table cell, never a delta (reference `0x6e9950`).
pub const SMSG_SET_FLAT_SPELL_MODIFIER: u16 = 0x0266; // 614
pub const SMSG_SET_PCT_SPELL_MODIFIER: u16 = 0x0267; // 615

pub const SMSG_SPELL_COOLDOWN: u16 = 0x0134; // 308
pub const SMSG_ITEM_COOLDOWN: u16 = 0x00B0; // 176
pub const SMSG_COOLDOWN_EVENT: u16 = 0x0135; // 309
pub const SMSG_CLEAR_COOLDOWN: u16 = 0x01DE; // 478
pub const SMSG_COOLDOWN_CHEAT: u16 = 0x01E1; // 481

/// A timed item's remaining life in seconds; the display reads this, not `ITEM_FIELD_DURATION`
/// (`Item.cpp:1094`). Its reference handler is the enchant update's, forking on the opcode
/// (`0x5e4f69`).
pub const SMSG_ITEM_TIME_UPDATE: u16 = 0x01EA; // 490

/// The only source of a temporary enchant's remaining time: the reference tooltip reads a
/// per-slot deadline (`obj + slot*4 + 0x324`) that this handler (`0x5e4f82`) writes.
pub const SMSG_ITEM_ENCHANT_TIME_UPDATE: u16 = 0x01EB; // 491

/// Cast pushback, to the caster: raw `u64` guid, `u32` delay ms (`Spell.cpp:7472`), and the cast
/// bar slides by it. A hit only pushes a cast back, never interrupts it.
pub const SMSG_SPELL_DELAYED: u16 = 0x01E2; // 482

// The channel bar, to the caster only: START is `u32 spellId`, `u32 duration_ms`; UPDATE is
// `u32 remaining_ms`, 0 when the channel is over.
pub const MSG_CHANNEL_START: u16 = 0x0139; // 313
pub const MSG_CHANNEL_UPDATE: u16 = 0x013A; // 314

/// Our own aura's remaining time: `u8 slot`, `u32 remaining_ms`; never for others' or permanent
/// auras (`SpellAuras.cpp:7511-7523`). It arrives before the `UNIT_FIELD_AURA` delta for the slot.
/// 3.3.5a only: a saved set's server guid, `u32 id` and a packed guid (`0x5af6c0`).
pub const SMSG_EQUIPMENT_SET_SAVED: u16 = 0x0137; // 311
/// 3.3.5a only: a unit's whole aura array; slots it does not name are emptied (`0x7300a0`).
pub const SMSG_AURA_UPDATE_ALL: u16 = 0x0495;
/// 3.3.5a only: some of a unit's aura slots, the same body as `SMSG_AURA_UPDATE_ALL`.
pub const SMSG_AURA_UPDATE: u16 = 0x0496;

/// Our own level-up, with no guid and sent only to the leveling player (`Player::GiveLevel`).
pub const SMSG_LEVELUP_INFO: u16 = 0x01D4; // 468
/// A newly explored area: the area id and the XP it granted (0 at max level).
pub const SMSG_EXPLORATION_EXPERIENCE: u16 = 0x01F8; // 504
/// An honor award, the only attributed notice of one; dishonorable kills carry negative honor.
pub const SMSG_PVP_CREDIT: u16 = 0x028C; // 652

pub const SMSG_ENVIRONMENTALDAMAGELOG: u16 = 0x01FC; // 508
pub const SMSG_LOG_XPGAIN: u16 = 0x01D0; // 464
pub const SMSG_SPELLLOGMISS: u16 = 0x024B; // 587
pub const SMSG_PERIODICAURALOG: u16 = 0x024E; // 590
pub const SMSG_SPELLDAMAGESHIELD: u16 = 0x024F; // 591
pub const SMSG_SPELLNONMELEEDAMAGELOG: u16 = 0x0250; // 592

pub const SMSG_ENCHANTMENTLOG: u16 = 0x01D7; // 471
pub const SMSG_PARTYKILLLOG: u16 = 0x01F5; // 501
pub const SMSG_SPELLLOGEXECUTE: u16 = 0x024C; // 588
pub const SMSG_PROCRESIST: u16 = 0x0260; // 608
pub const SMSG_DISPEL_FAILED: u16 = 0x0262; // 610
pub const SMSG_SPELLORDAMAGE_IMMUNE: u16 = 0x0263; // 611
pub const SMSG_SPELLDISPELLOG: u16 = 0x027B; // 635
pub const SMSG_SPELLSTEALLOG: u16 = 0x0333; // 819
pub const SMSG_SPELLBREAKLOG: u16 = 0x014F; // 335
pub const SMSG_DESTRUCTIBLE_BUILDING_DAMAGE: u16 = 0x0032; // 50
pub const SMSG_SPELLINSTAKILLLOG: u16 = 0x032F; // 815

pub const SMSG_SPELLHEALLOG: u16 = 0x0150; // 336
pub const SMSG_SPELLENERGIZELOG: u16 = 0x0151; // 337

pub const CMSG_CHAR_CREATE: u16 = 0x0036;
pub const CMSG_CHAR_ENUM: u16 = 0x0037;
pub const CMSG_PLAYER_LOGIN: u16 = 0x003D;
pub const CMSG_LOGOUT_REQUEST: u16 = 0x004B;
/// The forced logout (`ForceLogout`, `0x48ab50`): empty body, sent in place of
/// `CMSG_LOGOUT_REQUEST` and past the pending-logout latch.
pub const CMSG_PLAYER_LOGOUT: u16 = 0x004A; // 74
/// The CAMP/QUIT dialog's Cancel: empty body, answered by [`SMSG_LOGOUT_CANCEL_ACK`].
pub const CMSG_LOGOUT_CANCEL: u16 = 0x004E;
pub const CMSG_NAME_QUERY: u16 = 0x0050; // 80
pub const CMSG_CREATURE_QUERY: u16 = 0x0060; // 96
/// Ask a pet's name; answered by [`SMSG_PET_NAME_QUERY_RESPONSE`].
pub const CMSG_PET_NAME_QUERY: u16 = 0x0052; // 82

/// A pet bar press: the slot's packed word and a target guid, dispatched on its type byte.
pub const CMSG_PET_ACTION: u16 = 0x0175; // 373
/// A pet bar drag: one or two `(position, packed)` pairs, told apart by body size alone.
pub const CMSG_PET_SET_ACTION: u16 = 0x0174; // 372
/// Set a pet spell's autocast bit to a given value (the right-click).
pub const CMSG_PET_SPELL_AUTOCAST: u16 = 0x02F3; // 755
/// Call the pet off: the Attack button's second press.
pub const CMSG_PET_STOP_ATTACK: u16 = 0x02EA; // 746
/// Cancel one of the pet's auras; `CMSG_CANCEL_AURA` carries only a spell id, no unit.
pub const CMSG_PET_CANCEL_AURA: u16 = 0x026B; // 619

/// The pet menu's Abandon (`PetAbandon`, `0x4be4c0`): deletes a hunter pet, unsummons anything
/// else (`PetHandler.cpp:347-374`). Dismiss sends [`CMSG_PET_ACTION`] word `0x07000003` instead.
pub const CMSG_PET_ABANDON: u16 = 0x0176; // 374
/// Rename the pet (`PetRename`, `0x4be4e0`). One-shot: the server clears `UNIT_FLAG_PET_RENAME`
/// on success (`PetHandler.cpp:302-345`); a refusal is [`SMSG_PET_NAME_INVALID`].
pub const CMSG_PET_RENAME: u16 = 0x0177; // 375

/// The ask-once GameObject template lookup, shaped like `CMSG_CREATURE_QUERY`.
pub const CMSG_GAMEOBJECT_QUERY: u16 = 0x005E; // 94
/// Ask for a quest's template; answered by [`SMSG_QUEST_QUERY_RESPONSE`].
pub const CMSG_QUEST_QUERY: u16 = 0x005C; // 92
/// Ask for quests' POI regions; answered by [`SMSG_QUEST_POI_QUERY_RESPONSE`].
pub const CMSG_QUEST_POI_QUERY: u16 = 0x01E3; // 483
pub const CMSG_ITEM_QUERY_SINGLE: u16 = 0x0056; // 86
pub const CMSG_USE_ITEM: u16 = 0x00AB; // 171
/// Open an openable item (a clam, a lockbox, a gift) by bag position; the loot that answers names
/// the item's own guid (`HandleOpenItemOpcode`).
pub const CMSG_OPEN_ITEM: u16 = 0x00AC; // 172
/// Gift-wrap an item: `giftBag`, `giftSlot`, `itemBag`, `itemSlot`, paper first. Sent when the
/// wrap cursor, armed locally by right-clicking the paper (`0x5edea0`), clicks the item.
pub const CMSG_WRAP_ITEM: u16 = 0x01D3; // 467
/// Use a GameObject by full guid; chests loot through this, since `CMSG_LOOT` refuses their guids.
pub const CMSG_GAMEOBJ_USE: u16 = 0x00B1; // 177
/// A GameObject's one-shot custom animation: `u64 guid`, `u32 animId`. The client arms substate
/// `8 + animId` (AnimationData 153..156) and rejects `animId >= 4`; the bobber's bite is 0.
pub const SMSG_GAMEOBJECT_CUSTOM_ANIM: u16 = 0x00B3; // 179
/// The despawn animation: a bare `u64` guid, from `WorldObject` so totems and DynamicObjects too.
/// The client arms substate 12 (AnimationData 157) and the object outlives its
/// `SMSG_DESTROY_OBJECT` for that play.
pub const SMSG_GAMEOBJECT_DESPAWN_ANIM: u16 = 0x0215; // 533
/// We walked into an area trigger: its `AreaTrigger.dbc` id (reference `0x6dbe30`).
pub const CMSG_AREATRIGGER: u16 = 0x00B4; // 180
/// An area trigger's refusal (level, ghost, faction), shown like `SMSG_NOTIFICATION` (`0x4945b0`).
pub const SMSG_AREA_TRIGGER_MESSAGE: u16 = 0x02B8; // 696
pub const CMSG_MESSAGECHAT: u16 = 0x0095;
pub const SMSG_MESSAGECHAT: u16 = 0x0096;
/// 3.3.5a only: a chat line from a GM-tagged speaker, `SMSG_MESSAGECHAT`'s body with the sender's
/// name inline (handler `0x50ebc0`).
pub const SMSG_GM_MESSAGECHAT: u16 = 0x03B3;
// Channels: every CMSG is `cstring channelName` [+ `cstring playerName`]; 1.12 has no channel id.
pub const CMSG_JOIN_CHANNEL: u16 = 0x0097; // 151
pub const CMSG_LEAVE_CHANNEL: u16 = 0x0098; // 152
pub const SMSG_CHANNEL_NOTIFY: u16 = 0x0099; // 153
pub const CMSG_CHANNEL_LIST: u16 = 0x009A; // 154
pub const SMSG_CHANNEL_LIST: u16 = 0x009B; // 155
pub const CMSG_CHANNEL_PASSWORD: u16 = 0x009C; // 156
pub const CMSG_CHANNEL_SET_OWNER: u16 = 0x009D; // 157
pub const CMSG_CHANNEL_OWNER: u16 = 0x009E; // 158
pub const CMSG_CHANNEL_MODERATOR: u16 = 0x009F; // 159
pub const CMSG_CHANNEL_UNMODERATOR: u16 = 0x00A0; // 160
pub const CMSG_CHANNEL_MUTE: u16 = 0x00A1; // 161
pub const CMSG_CHANNEL_UNMUTE: u16 = 0x00A2; // 162
pub const CMSG_CHANNEL_INVITE: u16 = 0x00A3; // 163
pub const CMSG_CHANNEL_KICK: u16 = 0x00A4; // 164
pub const CMSG_CHANNEL_BAN: u16 = 0x00A5; // 165
pub const CMSG_CHANNEL_UNBAN: u16 = 0x00A6; // 166
pub const CMSG_CHANNEL_ANNOUNCEMENTS: u16 = 0x00A7; // 167
pub const CMSG_CHANNEL_MODERATE: u16 = 0x00A8; // 168
/// 3.3.5a only: the channel pane's roster ask (`0x504ee2`), `cstring channelName`, answered by an
/// `SMSG_CHANNEL_LIST` whose display byte is set.
pub const CMSG_CHANNEL_DISPLAY_LIST: u16 = 0x03D2; // 978
/// 3.3.5a only: the channel the pane now shows (`0x504555`), `cstring channelName`.
pub const CMSG_SET_CHANNEL_WATCH: u16 = 0x03EF; // 1007

/// The ignore notice ("Name is now ignoring you"): a raw `u64` guid (`Misc.cpp:127-130`).
pub const CMSG_CHAT_IGNORED: u16 = 0x0225; // 549
/// A whisper target is not online: one cstring name (`Chat.cpp:26-29`).
pub const SMSG_CHAT_PLAYER_NOT_FOUND: u16 = 0x02A9; // 681
/// A cross-faction whisper was refused; empty body (`Chat.cpp:16-18`).
pub const SMSG_CHAT_WRONG_FACTION: u16 = 0x0219; // 537
/// Nothing hooked (the bobber expired or was clicked early): empty body, `ERR_FISH_NOT_HOOKED`.
pub const SMSG_FISH_NOT_HOOKED: u16 = 0x01C8; // 456
/// The hooked fish got away (the skill roll failed); empty body, `ERR_FISH_ESCAPED`.
pub const SMSG_FISH_ESCAPED: u16 = 0x01C9; // 457
/// A server notice shown in the red UIErrorsFrame: one cstring (`WorldSession.cpp:900-915`).
pub const SMSG_NOTIFICATION: u16 = 0x01CB; // 459

// World broadcasts: to everyone, or to everyone in a zone.
/// An area is under attack: one `u32` `AreaTable.dbc` id. The client (`0x49dcc0`) fills
/// `ZONE_UNDER_ATTACK` and posts it on the joined defense channels, not as a system line.
pub const SMSG_ZONE_UNDER_ATTACK: u16 = 0x0254; // 596
/// A shutdown countdown or operator broadcast: a `u32` `ServerMessages.dbc` id, whose text formats
/// the cstring that follows; shown as `CHAT_MSG_SYSTEM` (`0x49df80`).
pub const SMSG_SERVER_MESSAGE: u16 = 0x0291; // 657
/// A trial account hit its whisper cap: empty body; the client (`0x5e4a09`) shows error `0x1c3`.
pub const SMSG_CHAT_RESTRICTED: u16 = 0x02FD; // 765
/// A defense broadcast (the Eastern Plaguelands towers): `u32 zoneId`, `u32 length`, the text
/// (`Map.cpp:1868-1884`); 12340 reads it at `0x50c850`.
pub const SMSG_DEFENSE_MESSAGE: u16 = 0x033A; // 826

/// `/played`: an empty request; the reply is `u32 total`, `u32 level` seconds (`Misc.cpp:278-282`).
pub const CMSG_PLAYED_TIME: u16 = 0x01CC; // 460
pub const SMSG_PLAYED_TIME: u16 = 0x01CD; // 461
/// Ask the server's clock: an empty request; the reply is one `u32` of unix seconds
/// (`QueryHandler.cpp:418-423`). Timed-quest deadlines are stamps on this clock.
pub const CMSG_QUERY_TIME: u16 = 0x01CE; // 462
pub const SMSG_QUERY_TIME_RESPONSE: u16 = 0x01CF; // 463
/// `/random`, both ways: we send `u32 min`, `u32 max`; the server broadcasts min, max,
/// `u32 roll`, `u64 guid` (`GroupHandler.cpp:394-422`).
pub const MSG_RANDOM_ROLL: u16 = 0x01FB; // 507

/// Ask for a new stand state.
pub const CMSG_STANDSTATECHANGE: u16 = 0x0101; // 257
/// A server-side change to our stand state (eat/drink sit, stand on damage): one `u8`, no guid
/// (`Unit.cpp:9541`). The reference (`0x603e50`) applies it to the local player, ungated.
pub const SMSG_STANDSTATE_UPDATE: u16 = 0x029D; // 669
pub const CMSG_CAST_SPELL: u16 = 0x012E; // 302
/// Cancel our cast: one `u32` spell id. The wand auto-repeat handoff sends it for the cached Shoot
/// before the local cancel (`0x6095b8`).
pub const CMSG_CANCEL_CAST: u16 = 0x012F; // 303
pub const CMSG_CANCEL_AURA: u16 = 0x0136; // 310
/// Send the summoned companion home (`Spell_C_CancelAura` `0x80303d`): the critter's guid.
pub const CMSG_DISMISS_CRITTER: u16 = 0x048D; // 1165
/// Empty a glyph socket (`RemoveGlyphFromSocket` `0x5b76ea`): the 0-based socket as a `u32`.
pub const CMSG_REMOVE_GLYPH: u16 = 0x048A; // 1162
/// Cancel our channel: a `u32` spell id the server ignores (`Spell.h:69`) and 1.12 still sends.
pub const CMSG_CANCEL_CHANNELLING: u16 = 0x013B; // 315
/// Inspect a player: a raw 8-byte guid. The server selects it, then answers `SMSG_INSPECT` unless
/// it is beyond 10 yd or attackable (`MiscHandler.cpp:943-960`).
pub const CMSG_INSPECT: u16 = 0x0114; // 276
/// The inspect reply, the echoed `u64` guid; the reference discards it (`0x5e7d70`), so we drop it.
pub const SMSG_INSPECT: u16 = 0x0115; // 277
/// The inspect Honor tab, both ways: we send a raw 8-byte guid, the reply is a 50-byte body.
/// Refused silently like [`CMSG_INSPECT`], but without selecting (`MiscHandler.cpp:962-972`).
pub const MSG_INSPECT_HONOR_STATS: u16 = 0x02D6; // 726
/// Another player's arena teams, both ways: we send the raw guid, each reply is one team slot
/// (`0x5e7b00`); it fires `INSPECT_HONOR_UPDATE`.
pub const MSG_INSPECT_ARENA_TEAMS: u16 = 0x0377; // 887
/// Right-click a battlemaster (`0x5e01a0`): a `u64` guid; answered by `SMSG_BATTLEFIELD_LIST`.
pub const CMSG_BATTLEMASTER_HELLO: u16 = 0x02D7; // 727
pub const CMSG_SET_SELECTION: u16 = 0x013D; // 317
pub const CMSG_ATTACKSWING: u16 = 0x0141; // 321
pub const CMSG_ATTACKSTOP: u16 = 0x0142; // 322
/// Set our weapon sheath state.
pub const CMSG_SETSHEATHED: u16 = 0x01E0; // 480
pub const CMSG_AUTH_SESSION: u16 = 0x01ED;
pub const CMSG_SET_ACTIVE_MOVER: u16 = 0x026A;
/// Give up a mover: its full u64 guid, then a `MovementInfo`. The server re-broadcasts a stop for
/// it; unsent, observers keep its last relayed pose (`MovementHandler.cpp:886-965`).
pub const CMSG_MOVE_NOT_ACTIVE_MOVER: u16 = 0x02D1; // 721
/// The far-sight toggle: one `u8`, 1 views through the `PLAYER_FARSIGHT` object, 0 through our
/// body (`MiscHandler.cpp:1138-1155`). Sending 0 moves visibility home while the camera stays on
/// the object, so it must be deliberate; the server expects no reply.
pub const CMSG_FAR_SIGHT: u16 = 0x027A; // 634
/// Stop auto-repeat: empty body, sent from the local cancel (`0x6ea080`), so on every cancel.
pub const CMSG_CANCEL_AUTO_REPEAT_SPELL: u16 = 0x026D; // 621

// Keepalive: the 1.12 client pings every 30 s (`0x537ff0`) with `u32 sequence`, `u32 lastRtt`,
// and the pong echoes the sequence. vmangos kicks after more than 2 pings under 27 s apart.
pub const CMSG_PING: u16 = 0x01DC; // 476
pub const SMSG_PONG: u16 = 0x01DD; // 477

// Speed changes to a unit's controller: `[packed guid][u32 counter][f32 yd/s]`. The client must
// ack with `[full u64 guid][u32 counter][MovementInfo][f32 speed]`, the speed within 0.01 and the
// counter pending; unacked, vmangos force-resolves after 4 s and flags it as cheating.
pub const SMSG_FORCE_RUN_SPEED_CHANGE: u16 = 0x00E2; // 226
pub const CMSG_FORCE_RUN_SPEED_CHANGE_ACK: u16 = 0x00E3; // 227
pub const SMSG_FORCE_RUN_BACK_SPEED_CHANGE: u16 = 0x00E4; // 228
pub const CMSG_FORCE_RUN_BACK_SPEED_CHANGE_ACK: u16 = 0x00E5; // 229
pub const SMSG_FORCE_SWIM_SPEED_CHANGE: u16 = 0x00E6; // 230
pub const CMSG_FORCE_SWIM_SPEED_CHANGE_ACK: u16 = 0x00E7; // 231
pub const SMSG_FORCE_WALK_SPEED_CHANGE: u16 = 0x02DA; // 730
pub const CMSG_FORCE_WALK_SPEED_CHANGE_ACK: u16 = 0x02DB; // 731
pub const SMSG_FORCE_SWIM_BACK_SPEED_CHANGE: u16 = 0x02DC; // 732
pub const CMSG_FORCE_SWIM_BACK_SPEED_CHANGE_ACK: u16 = 0x02DD; // 733
pub const SMSG_FORCE_TURN_RATE_CHANGE: u16 = 0x02DE; // 734
pub const CMSG_FORCE_TURN_RATE_CHANGE_ACK: u16 = 0x02DF; // 735
/// 3.3.5a only, as the five below: the flight speeds and the pitch rate, one handler `0x732450`.
pub const SMSG_FORCE_FLIGHT_SPEED_CHANGE: u16 = 0x0381; // 897
pub const CMSG_FORCE_FLIGHT_SPEED_CHANGE_ACK: u16 = 0x0382; // 898
pub const SMSG_FORCE_FLIGHT_BACK_SPEED_CHANGE: u16 = 0x0383; // 899
pub const CMSG_FORCE_FLIGHT_BACK_SPEED_CHANGE_ACK: u16 = 0x0384; // 900
pub const SMSG_FORCE_PITCH_RATE_CHANGE: u16 = 0x045C; // 1116
pub const CMSG_FORCE_PITCH_RATE_CHANGE_ACK: u16 = 0x045D; // 1117

// Speed changes on units we do not control, never acked. A creature's, or a player's mid-spline,
// come as `SMSG_SPLINE_SET_*` `[packed guid][f32 speed]`; a freely moving player's come as
// `MSG_MOVE_SET_*` `[packed guid][MovementInfo][f32 speed]`, which also carries a fresh pose.
pub const SMSG_SPLINE_SET_RUN_SPEED: u16 = 0x02FE; // 766
pub const SMSG_SPLINE_SET_RUN_BACK_SPEED: u16 = 0x02FF; // 767
pub const SMSG_SPLINE_SET_SWIM_SPEED: u16 = 0x0300; // 768
pub const SMSG_SPLINE_SET_WALK_SPEED: u16 = 0x0301; // 769
pub const SMSG_SPLINE_SET_SWIM_BACK_SPEED: u16 = 0x0302; // 770
pub const SMSG_SPLINE_SET_TURN_RATE: u16 = 0x0303; // 771
/// 3.3.5a only, as the two below (`0x741bc0`).
pub const SMSG_SPLINE_SET_FLIGHT_SPEED: u16 = 0x0385; // 901
pub const SMSG_SPLINE_SET_FLIGHT_BACK_SPEED: u16 = 0x0386; // 902
pub const SMSG_SPLINE_SET_PITCH_RATE: u16 = 0x045E; // 1118

// Mode changes on any unit, for observers: a bare packed guid, no counter, no ack
// (`MovementPacketSender.cpp:399-462`). The reference handles all twelve at `0x603c80`, drops an
// unresolvable guid, and re-picks the unit's animation after each. Trap: SET_RUN_MODE clears
// `MOVEFLAG_WALK_MODE` and SET_WALK_MODE sets it (`0x617e80`).
pub const SMSG_SPLINE_MOVE_UNROOT: u16 = 0x0304; // 772
pub const SMSG_SPLINE_MOVE_FEATHER_FALL: u16 = 0x0305; // 773
pub const SMSG_SPLINE_MOVE_NORMAL_FALL: u16 = 0x0306; // 774
pub const SMSG_SPLINE_MOVE_SET_HOVER: u16 = 0x0307; // 775
pub const SMSG_SPLINE_MOVE_UNSET_HOVER: u16 = 0x0308; // 776
pub const SMSG_SPLINE_MOVE_WATER_WALK: u16 = 0x0309; // 777
pub const SMSG_SPLINE_MOVE_LAND_WALK: u16 = 0x030A; // 778
pub const SMSG_SPLINE_MOVE_START_SWIM: u16 = 0x030B; // 779
pub const SMSG_SPLINE_MOVE_STOP_SWIM: u16 = 0x030C; // 780
pub const SMSG_SPLINE_MOVE_SET_RUN_MODE: u16 = 0x030D; // 781
pub const SMSG_SPLINE_MOVE_SET_WALK_MODE: u16 = 0x030E; // 782
pub const SMSG_SPLINE_MOVE_ROOT: u16 = 0x031A; // 794
pub const MSG_MOVE_SET_RUN_SPEED: u16 = 0x00CD; // 205
pub const MSG_MOVE_SET_RUN_BACK_SPEED: u16 = 0x00CF; // 207
pub const MSG_MOVE_SET_WALK_SPEED: u16 = 0x00D1; // 209
pub const MSG_MOVE_SET_SWIM_SPEED: u16 = 0x00D3; // 211
pub const MSG_MOVE_SET_SWIM_BACK_SPEED: u16 = 0x00D5; // 213
pub const MSG_MOVE_SET_TURN_RATE: u16 = 0x00D8; // 216
/// 3.3.5a only, as the two below (`0x741b00`).
pub const MSG_MOVE_SET_FLIGHT_SPEED: u16 = 0x037E; // 894
pub const MSG_MOVE_SET_FLIGHT_BACK_SPEED: u16 = 0x0380; // 896
pub const MSG_MOVE_SET_PITCH_RATE: u16 = 0x045B; // 1115

// Mount results: one `u32` code (`Player::SendMountResult`). Success (`MOUNTRESULT_OK` = 10,
// `DISMOUNTRESULT_OK` = 3) is silent in the reference; any other code is a red error line.
pub const SMSG_MOUNTRESULT: u16 = 0x016E; // 366
pub const SMSG_DISMOUNTRESULT: u16 = 0x016F; // 367

// The mounted flourish, an empty CMSG; who gets the echo: `ServerPacket::MountSpecialAnim`.
pub const CMSG_MOUNTSPECIAL_ANIM: u16 = 0x0171; // 369
pub const SMSG_MOUNTSPECIAL_ANIM: u16 = 0x0172; // 370

// Movement, both ways: vmangos relays each `HandleMovementOpcodes` opcode to nearby clients as
// `[packed mover guid][MovementInfo]` (`Opcodes.cpp`); the TELEPORT and WORLDPORT acks differ.
pub const MSG_MOVE_START_FORWARD: u16 = 0x00B5; // 181
pub const MSG_MOVE_START_BACKWARD: u16 = 0x00B6; // 182
pub const MSG_MOVE_STOP: u16 = 0x00B7; // 183
pub const MSG_MOVE_START_STRAFE_LEFT: u16 = 0x00B8; // 184
pub const MSG_MOVE_START_STRAFE_RIGHT: u16 = 0x00B9; // 185
pub const MSG_MOVE_STOP_STRAFE: u16 = 0x00BA; // 186
pub const MSG_MOVE_JUMP: u16 = 0x00BB; // 187
pub const MSG_MOVE_START_TURN_LEFT: u16 = 0x00BC; // 188
pub const MSG_MOVE_START_TURN_RIGHT: u16 = 0x00BD; // 189
pub const MSG_MOVE_STOP_TURN: u16 = 0x00BE; // 190
pub const MSG_MOVE_START_PITCH_UP: u16 = 0x00BF; // 191
pub const MSG_MOVE_START_PITCH_DOWN: u16 = 0x00C0; // 192
pub const MSG_MOVE_STOP_PITCH: u16 = 0x00C1; // 193
pub const MSG_MOVE_SET_RUN_MODE: u16 = 0x00C2; // 194
pub const MSG_MOVE_SET_WALK_MODE: u16 = 0x00C3; // 195
pub const MSG_MOVE_TELEPORT_ACK: u16 = 0x00C7; // 199
pub const MSG_MOVE_FALL_LAND: u16 = 0x00C9; // 201
pub const MSG_MOVE_START_SWIM: u16 = 0x00CA; // 202
pub const MSG_MOVE_STOP_SWIM: u16 = 0x00CB; // 203
pub const MSG_MOVE_WORLDPORT_ACK: u16 = 0x00DC; // 220
pub const MSG_MOVE_SET_FACING: u16 = 0x00DA; // 218
pub const MSG_MOVE_SET_PITCH: u16 = 0x00DB; // 219
pub const MSG_MOVE_HEARTBEAT: u16 = 0x00EE; // 238
/// 3.3.5a only: the vertical channel's three verbs, sent by the move-command executor `0x6ef860`
/// (cases 6, 7 and 8) and relayed through the shared move handler.
pub const MSG_MOVE_START_ASCEND: u16 = 0x0359; // 857
/// 3.3.5a only: the one stop for either direction (`0x989940` clears both bits).
pub const MSG_MOVE_STOP_ASCEND: u16 = 0x035A; // 858
/// 3.3.5a only.
pub const MSG_MOVE_START_DESCEND: u16 = 0x03A7; // 935

pub const CMSG_GOSSIP_HELLO: u16 = 0x017B; // 379
pub const CMSG_GOSSIP_SELECT_OPTION: u16 = 0x017C; // 380
pub const SMSG_GOSSIP_MESSAGE: u16 = 0x017D; // 381
pub const SMSG_GOSSIP_COMPLETE: u16 = 0x017E; // 382
pub const CMSG_NPC_TEXT_QUERY: u16 = 0x017F; // 383
pub const SMSG_NPC_TEXT_UPDATE: u16 = 0x0180; // 384

// A guard's map marker, pushed unasked by a gossip option's `action_poi_id` (`GossipDef.cpp:253`).
pub const SMSG_GOSSIP_POI: u16 = 0x0224; // 548

pub const CMSG_QUESTGIVER_STATUS_QUERY: u16 = 0x0182; // 386
pub const SMSG_QUESTGIVER_STATUS: u16 = 0x0183; // 387
pub const CMSG_QUESTGIVER_HELLO: u16 = 0x0184; // 388
pub const SMSG_QUESTGIVER_QUEST_LIST: u16 = 0x0185; // 389
pub const CMSG_QUESTGIVER_QUERY_QUEST: u16 = 0x0186; // 390
pub const SMSG_QUESTGIVER_QUEST_DETAILS: u16 = 0x0188; // 392
pub const CMSG_QUESTGIVER_ACCEPT_QUEST: u16 = 0x0189; // 393
pub const CMSG_QUESTGIVER_COMPLETE_QUEST: u16 = 0x018A; // 394
pub const SMSG_QUESTGIVER_REQUEST_ITEMS: u16 = 0x018B; // 395
pub const CMSG_QUESTGIVER_REQUEST_REWARD: u16 = 0x018C; // 396
pub const SMSG_QUESTGIVER_OFFER_REWARD: u16 = 0x018D; // 397
pub const CMSG_QUESTGIVER_CHOOSE_REWARD: u16 = 0x018E; // 398
pub const SMSG_QUESTGIVER_QUEST_INVALID: u16 = 0x018F; // 399
pub const SMSG_QUESTGIVER_QUEST_COMPLETE: u16 = 0x0191; // 401
pub const SMSG_QUESTGIVER_QUEST_FAILED: u16 = 0x0192; // 402
/// 3.3.5a only: `u32 count`, then `{u64 guid, u8 status}` per NPC; 12340 routes it with 0x183.
pub const SMSG_QUESTGIVER_STATUS_MULTIPLE: u16 = 0x0418;

pub const CMSG_QUESTLOG_SWAP_QUEST: u16 = 0x0193; // 403
pub const CMSG_QUESTLOG_REMOVE_QUEST: u16 = 0x0194; // 404
pub const SMSG_QUESTLOG_FULL: u16 = 0x0195; // 405
pub const SMSG_QUESTUPDATE_FAILED: u16 = 0x0196; // 406
pub const SMSG_QUESTUPDATE_FAILEDTIMER: u16 = 0x0197; // 407
pub const SMSG_QUESTUPDATE_COMPLETE: u16 = 0x0198; // 408
pub const SMSG_QUESTUPDATE_ADD_KILL: u16 = 0x0199; // 409
pub const SMSG_QUESTUPDATE_ADD_ITEM: u16 = 0x019A; // 410

// Quest sharing. `MSG_QUEST_PUSH_RESULT` runs both ways: the receiver sends its verdict, and the
// server relays every verdict, its own and the receiver's, to the sharer.
pub const CMSG_QUEST_CONFIRM_ACCEPT: u16 = 0x019B; // 411
pub const SMSG_QUEST_CONFIRM_ACCEPT: u16 = 0x019C; // 412
pub const CMSG_PUSHQUESTTOPARTY: u16 = 0x019D; // 413
pub const MSG_QUEST_PUSH_RESULT: u16 = 0x0276; // 630

pub const CMSG_LIST_INVENTORY: u16 = 0x019E; // 414
pub const SMSG_LIST_INVENTORY: u16 = 0x019F; // 415
pub const CMSG_SELL_ITEM: u16 = 0x01A0; // 416
pub const SMSG_SELL_ITEM: u16 = 0x01A1; // 417
pub const CMSG_BUY_ITEM: u16 = 0x01A2; // 418
/// Buy into a given bag slot (a vendor row dropped from the cursor); a click sends `CMSG_BUY_ITEM`.
pub const CMSG_BUY_ITEM_IN_SLOT: u16 = 0x01A3; // 419
pub const SMSG_BUY_ITEM: u16 = 0x01A4; // 420
pub const SMSG_BUY_FAILED: u16 = 0x01A5; // 421
/// Buy a sold item back (`BuybackItem`, `0x4fb950`); outside the 0x19E vendor run.
pub const CMSG_BUYBACK_ITEM: u16 = 0x0290; // 656
/// Repair one item by guid, or everything with guid 0, at a repair vendor.
pub const CMSG_REPAIR_ITEM: u16 = 0x02A8; // 680

// Taxi: every guid in these, both ways, is a plain u64, never packed (`ObjectGuid.cpp:174-186`).
pub const SMSG_SHOWTAXINODES: u16 = 0x01A9; // 425
pub const CMSG_TAXINODE_STATUS_QUERY: u16 = 0x01AA; // 426
pub const SMSG_TAXINODE_STATUS: u16 = 0x01AB; // 427
pub const CMSG_TAXIQUERYAVAILABLENODES: u16 = 0x01AC; // 428
pub const CMSG_ACTIVATETAXI: u16 = 0x01AD; // 429
pub const SMSG_ACTIVATETAXIREPLY: u16 = 0x01AE; // 430
pub const SMSG_NEW_TAXI_PATH: u16 = 0x01AF; // 431
/// A multi-hop flight: guid, total cost, node list; a single hop uses `CMSG_ACTIVATETAXI`.
pub const CMSG_ACTIVATETAXIEXPRESS: u16 = 0x0312; // 786

// Trainers, class and profession alike, open from `GOSSIP_OPTION_TRAINER`; there is no open verb.
pub const CMSG_TRAINER_LIST: u16 = 0x01B0; // 432
pub const SMSG_TRAINER_LIST: u16 = 0x01B1; // 433
pub const CMSG_TRAINER_BUY_SPELL: u16 = 0x01B2; // 434
pub const SMSG_TRAINER_BUY_SUCCEEDED: u16 = 0x01B3; // 435
pub const SMSG_TRAINER_BUY_FAILED: u16 = 0x01B4; // 436

// The innkeeper bind: `SMSG_BINDER_CONFIRM` only asks; Accept sends `CMSG_BINDER_ACTIVATE`, then
// spell 3286 binds and sends `SMSG_BINDPOINTUPDATE` and `SMSG_PLAYERBOUND` (`EffectBind`).
pub const CMSG_BINDER_ACTIVATE: u16 = 0x01B5; // 437
pub const SMSG_PLAYERBOUND: u16 = 0x0158; // 344
pub const SMSG_BINDER_CONFIRM: u16 = 0x02EB; // 747

// GM tickets: nothing is pushed at login, so the Help window asks `CMSG_GMTICKET_GETTICKET`. GM
// commands push GETTICKET, DELETETICKET and status answers unasked, so nothing waits on a reply.
// 12340 reads all eight answers in one handler (`0x5ad240`).
pub const CMSG_GMTICKET_CREATE: u16 = 0x0205; // 517
pub const SMSG_GMTICKET_CREATE: u16 = 0x0206; // 518
pub const CMSG_GMTICKET_UPDATETEXT: u16 = 0x0207; // 519
pub const SMSG_GMTICKET_UPDATETEXT: u16 = 0x0208; // 520
pub const CMSG_GMTICKET_GETTICKET: u16 = 0x0211; // 529
pub const SMSG_GMTICKET_GETTICKET: u16 = 0x0212; // 530
pub const CMSG_GMTICKET_DELETETICKET: u16 = 0x0217; // 535
pub const SMSG_GMTICKET_DELETETICKET: u16 = 0x0218; // 536
pub const CMSG_GMTICKET_SYSTEMSTATUS: u16 = 0x021A; // 538
pub const SMSG_GMTICKET_SYSTEMSTATUS: u16 = 0x021B; // 539

/// A ticket-state push: `u32` 1 or 4 re-asks, 2 closes the ticket, 3 closes it and offers the
/// survey (`0x5ad42d`).
pub const SMSG_GM_TICKET_STATUS_UPDATE: u16 = 0x0328; // 808

// A GM's answer to a ticket, its database error, the player's resolve and the answer's closing
// push (`0x5ad499`); `CMSG_GMRESPONSE_RESOLVE` is empty (`0x5acce0`).
pub const SMSG_GMRESPONSE_DB_ERROR: u16 = 0x04EE; // 1262
pub const SMSG_GMRESPONSE_RECEIVED: u16 = 0x04EF; // 1263
pub const CMSG_GMRESPONSE_RESOLVE: u16 = 0x04F0; // 1264
pub const SMSG_GMRESPONSE_STATUS_UPDATE: u16 = 0x04F1; // 1265

// The bank: `CMSG_BANKER_ACTIVATE` opens a pure banker; `SMSG_SHOW_BANK` also arrives unasked
// from the `GOSSIP_OPTION_BANKER` option.
pub const CMSG_BANKER_ACTIVATE: u16 = 0x01B7; // 439
pub const SMSG_SHOW_BANK: u16 = 0x01B8; // 440
pub const CMSG_BUY_BANK_SLOT: u16 = 0x01B9; // 441
pub const SMSG_BUY_BANK_SLOT_RESULT: u16 = 0x01BA; // 442
pub const CMSG_AUTOSTORE_BANK_ITEM: u16 = 0x0282; // 642
pub const CMSG_AUTOBANK_ITEM: u16 = 0x0283; // 643

/// Forget a cached player name: `u64 guid`, the cache's only eviction for a player (it has no
/// TTL; `0x556ff0`). vmangos never sends it.
pub const SMSG_INVALIDATE_PLAYER: u16 = 0x031C; // 796

// The pet stable. `MSG_LIST_STABLED_PETS` runs both ways: the server sends the list to open the
// window, the client sends one guid to refresh. Mutations get only one `SMSG_STABLE_RESULT` byte.
pub const MSG_LIST_STABLED_PETS: u16 = 0x026F; // 623
pub const CMSG_STABLE_PET: u16 = 0x0270; // 624
pub const CMSG_UNSTABLE_PET: u16 = 0x0271; // 625
pub const CMSG_BUY_STABLE_SLOT: u16 = 0x0272; // 626
pub const SMSG_STABLE_RESULT: u16 = 0x0273; // 627
pub const CMSG_STABLE_SWAP_PET: u16 = 0x0275; // 629

/// Spend a talent point; the rank's `SMSG_LEARNED_SPELL` and `PLAYER_CHARACTER_POINTS1` answer it.
pub const CMSG_LEARN_TALENT: u16 = 0x0251; // 593
/// 3.3.5a only: the pet's `LearnTalent`, `u64 pet`, `u32 talentId`, `u32 rank` (`0x5c8291`).
pub const CMSG_PET_LEARN_TALENT: u16 = 0x047A;
/// 3.3.5a only: a unit earned an achievement (`0x5b3020`).
pub const SMSG_ACHIEVEMENT_EARNED: u16 = 0x0468;
/// 3.3.5a only: one criterion's progress (`0x5b3160`).
pub const SMSG_CRITERIA_UPDATE: u16 = 0x046A;
/// 3.3.5a only: ask for a player's achievements, `SetAchievementComparisonUnit` (`0x5b1b1f`).
pub const CMSG_QUERY_INSPECT_ACHIEVEMENTS: u16 = 0x046B;
/// 3.3.5a only: the inspected player's achievements and criteria (`0x5b34a0`).
pub const SMSG_RESPOND_INSPECT_ACHIEVEMENTS: u16 = 0x046C;
/// 3.3.5a only: drop one criterion's progress (`0x5b36f0`).
pub const SMSG_CRITERIA_DELETED: u16 = 0x049E;
/// 3.3.5a only: the server's cache version, one u32 sent after the auth (handler `0x464730`); a
/// value other than the one the `Cache\WDB\` files carry drops their records (`0x66f940`).
pub const SMSG_CLIENTCACHE_VERSION: u16 = 0x04AB;
/// The server's time per account-data slot (`0x6b94c0`); 1.12's `SMSG_ACCOUNT_DATA_MD5` number.
pub const SMSG_ACCOUNT_DATA_TIMES: u16 = 0x0209;
/// Ask for one account-data slot's bytes (`DownloadAccountData` `0x6b92c0`).
pub const CMSG_REQUEST_ACCOUNT_DATA: u16 = 0x020A;
/// One account-data slot's bytes up (`UploadAccountData` `0x6b9100`).
pub const CMSG_UPDATE_ACCOUNT_DATA: u16 = 0x020B;
/// One account-data slot's bytes down (`0x6b9730`).
pub const SMSG_UPDATE_ACCOUNT_DATA: u16 = 0x020C;
/// The server stored an upload: slot and result, read and ignored (`0x6b8fc0`).
pub const SMSG_UPDATE_ACCOUNT_DATA_COMPLETE: u16 = 0x0463;
/// The character screen is up: send the account's times (`ReadyForAccountDataTimes` `0x4de390`).
pub const CMSG_READY_FOR_ACCOUNT_DATA_TIMES: u16 = 0x04FF;
/// 3.3.5a only: drop one completed achievement (`0x5b3610`).
pub const SMSG_ACHIEVEMENT_DELETED: u16 = 0x049F;
/// 3.3.5a only: the player's completed achievements and criteria at login (`0x5b32f0`).
pub const SMSG_ALL_ACHIEVEMENT_DATA: u16 = 0x047D;
/// 3.3.5a only: every talent group's ranks, or the pet's (`0x6cd770`).
pub const SMSG_TALENTS_INFO: u16 = 0x04C0;
/// 3.3.5a only: the player's saved equipment sets (`0x5af490`).
pub const SMSG_EQUIPMENT_SET_LIST: u16 = 0x04BC;
/// 3.3.5a only: a set swap's verdict, one `u8` (`0x5af710`).
pub const SMSG_EQUIPMENT_SET_USE_RESULT: u16 = 0x04D6;
/// 3.3.5a only: save a set (`CGEquipmentManager::SaveSet` `0x5ade50`).
pub const CMSG_EQUIPMENT_SET_SAVE: u16 = 0x04BD;
/// 3.3.5a only: delete a set by its guid (`0x5ae260`).
pub const CMSG_EQUIPMENT_SET_DELETE: u16 = 0x013E;
/// 3.3.5a only: wear a set, 19 slots of where each item is (`0x5ada20`).
pub const CMSG_EQUIPMENT_SET_USE: u16 = 0x04D5;
/// 3.3.5a only: a title mask id gained or lost, `u32 mask`, `u32 earned` (`0x50c520`).
pub const SMSG_TITLE_EARNED: u16 = 0x0373;
/// 3.3.5a only: show a title, `u32` mask id (`SetCurrentTitle` `0x522e30`).
pub const CMSG_SET_TITLE: u16 = 0x0374;
/// 3.3.5a only: a totem took a slot (`0x526530` arm `0x413`).
pub const SMSG_TOTEM_CREATED: u16 = 0x0413;
/// 3.3.5a only: one rune changed kind, `u8 rune`, `u8 kind` (`0x6e2e90` arm `0x486`).
pub const SMSG_CONVERT_RUNE: u16 = 0x0486;
/// 3.3.5a only: every rune's kind and recharge progress (`0x6e2e90` arm `0x487`).
pub const SMSG_RESYNC_RUNES: u16 = 0x0487;
/// 3.3.5a only: a mask of runes made ready (`0x6e2e90` arm `0x488`).
pub const SMSG_ADD_RUNE_POWER: u16 = 0x0488;
/// 3.3.5a only: dismiss the totem in a slot, `u8 slot` (`DestroyTotem` `0x5197d0`).
pub const CMSG_TOTEM_DESTROYED: u16 = 0x0414;
/// 3.3.5a only: the preview mode's learn, `u32 count` × (`u32 talentId`, `u32 rank`) (`0x5c6a10`).
pub const CMSG_LEARN_PREVIEW_TALENTS: u16 = 0x04C1;
/// 3.3.5a only: the pet's preview learn, the pet guid before the same body (`0x5c6ab4`).
pub const CMSG_LEARN_PREVIEW_TALENTS_PET: u16 = 0x04C2;

/// The respec, both ways: the trainer's gossip line (`Player.cpp:12406`) makes the server ask with
/// the trainer's guid and the cost; the `CONFIRM_TALENT_WIPE` Accept sends the latched guid back.
pub const MSG_TALENT_WIPE_CONFIRM: u16 = 0x02AA; // 682
/// The instance boot timer: `u32 delayMs`, `u32 reason`; boot START for a positive delay, STOP for
/// zero, and with reason 1 or 2 a zero delay also shows `ERR_RAID_GROUP_ONLY` or `_FULL`.
pub const SMSG_RAID_GROUP_ONLY: u16 = 0x0286; // 646
/// A queue slot (`0x54ae40`): `u32 slot`, `u64 queue`, then unless the word is 0 `u8` min and
/// max level, `u32 instance`, `u8 rated`, `u32 status` and the status's tail.
pub const SMSG_BATTLEFIELD_STATUS: u16 = 0x02D4; // 724
/// `AcceptBattlefieldPort(index, accept)` (`0x54da10`): the slot's `u64 queue`, `u8 accept`.
pub const CMSG_BATTLEFIELD_PORT: u16 = 0x02D5; // 725
/// The scoreboard, both ways: an empty request (5000 ms throttle, once only after the end or in
/// an arena, `0x54ce30`); the reply is read by `0x54d280`.
pub const MSG_PVP_LOG_DATA: u16 = 0x02E0; // 736
/// `LeaveBattlefield()` (`0x54c250`): the active slot's `u64 queue`, 0 with none active.
pub const CMSG_LEAVE_BATTLEFIELD: u16 = 0x02E1; // 737
/// Teammate positions, both ways: an empty request (5000 ms throttle, an active slot only); the
/// reply is `u32 count` × `(u64, f32, f32)`, then `u32 carriers` × the same (`0x54b3f0`).
pub const MSG_BATTLEGROUND_PLAYER_POSITIONS: u16 = 0x02E9; // 745
/// `RequestBattlegroundInstanceInfo(index)` (`0x54e5b0`): `u32 type`, `u8 1`, `u8 xpDisabled`.
pub const CMSG_BATTLEFIELD_LIST: u16 = 0x023C; // 572
/// One battleground type's instance list and rewards (`0x54e390`).
pub const SMSG_BATTLEFIELD_LIST: u16 = 0x023D; // 573
/// `JoinBattlefield` for a battleground (`0x54cc70`): `u64 battlemaster` (0 from the PvP window),
/// `u32 type`, `u32 instance` (0 first available), `u8 asGroup`.
pub const CMSG_BATTLEMASTER_JOIN: u16 = 0x02EE; // 750
/// `JoinBattlefield` for an arena (`0x54cc70`): `u64 battlemaster`, `u8 slot`, `u8 asGroup`,
/// `u8 rated`.
pub const CMSG_BATTLEMASTER_JOIN_ARENA: u16 = 0x0358; // 856
/// The world-entry status request (`0x54ca50`): empty; one `SMSG_BATTLEFIELD_STATUS` per slot.
pub const CMSG_BATTLEFIELD_STATUS: u16 = 0x02D3; // 723
/// A join's verdict (`0x54b1c0`): `i32`, a `BattlemasterList.dbc` id or a negative error, the two
/// timed-out codes followed by the `u64` player at fault.
pub const SMSG_GROUP_JOINED_BATTLEGROUND: u16 = 0x02E8; // 744
/// A player joined the battleground: `u64 guid`, printed once the name resolves (`0x54b510`).
pub const SMSG_BATTLEGROUND_PLAYER_JOINED: u16 = 0x02EC; // 748
/// A player left the battleground: `u64 guid` (`0x54b510`).
pub const SMSG_BATTLEGROUND_PLAYER_LEFT: u16 = 0x02ED; // 749
/// The port was refused because the player is dead: empty (`0x54d4b0`).
pub const SMSG_BATTLEFIELD_PORT_DENIED: u16 = 0x014B; // 331
/// An arena opponent's unit left the world: `u64 guid` (`0x54b5e0`).
pub const SMSG_ARENA_UNIT_DESTROYED: u16 = 0x04C7; // 1223
/// `HearthAndResurrectFromArea()` (`0x54c7a0`): empty.
pub const CMSG_HEARTH_AND_RESURRECT: u16 = 0x049C; // 1180
/// `ReportPlayerIsPVPAFK(unit)`: `u64 guid`.
pub const CMSG_REPORT_PVP_AFK: u16 = 0x03E4; // 996
/// The world-PvP battle manager (Wintergrasp), handlers `0x54b610`..`0x54b7f0`.
pub const SMSG_BATTLEFIELD_MGR_ENTRY_INVITE: u16 = 0x04DE; // 1246
pub const CMSG_BATTLEFIELD_MGR_ENTRY_INVITE_RESPONSE: u16 = 0x04DF; // 1247
pub const SMSG_BATTLEFIELD_MGR_ENTERED: u16 = 0x04E0; // 1248
pub const SMSG_BATTLEFIELD_MGR_QUEUE_INVITE: u16 = 0x04E1; // 1249
pub const CMSG_BATTLEFIELD_MGR_QUEUE_INVITE_RESPONSE: u16 = 0x04E2; // 1250
pub const CMSG_BATTLEFIELD_MGR_QUEUE_REQUEST: u16 = 0x04E3; // 1251
pub const SMSG_BATTLEFIELD_MGR_QUEUE_REQUEST_RESPONSE: u16 = 0x04E4; // 1252
pub const SMSG_BATTLEFIELD_MGR_EJECT_PENDING: u16 = 0x04E5; // 1253
pub const SMSG_BATTLEFIELD_MGR_EJECTED: u16 = 0x04E6; // 1254
pub const CMSG_BATTLEFIELD_MGR_EXIT_REQUEST: u16 = 0x04E7; // 1255
pub const SMSG_BATTLEFIELD_MGR_STATE_CHANGE: u16 = 0x04E8; // 1256

/// The arena-team family (`CGArenaTeamInfo`, `0x5a40e0`, and the player's `0x6cc910`..).
pub const SMSG_ARENA_TEAM_COMMAND_RESULT: u16 = 0x0349; // 841
/// The team cache's ask (`DBCache<ArenaTeamCache>`): `u32 teamId`.
pub const CMSG_ARENA_TEAM_QUERY: u16 = 0x034B; // 843
pub const SMSG_ARENA_TEAM_QUERY_RESPONSE: u16 = 0x034C; // 844
/// `ArenaTeamRoster(team)` (`0x5a3600`): `u32 teamId`, 10 s apart per team.
pub const CMSG_ARENA_TEAM_ROSTER: u16 = 0x034D; // 845
pub const SMSG_ARENA_TEAM_ROSTER: u16 = 0x034E; // 846
pub const CMSG_ARENA_TEAM_INVITE: u16 = 0x034F; // 847
pub const SMSG_ARENA_TEAM_INVITE: u16 = 0x0350; // 848
pub const CMSG_ARENA_TEAM_ACCEPT: u16 = 0x0351; // 849
pub const CMSG_ARENA_TEAM_DECLINE: u16 = 0x0352; // 850
pub const CMSG_ARENA_TEAM_LEAVE: u16 = 0x0353; // 851
pub const CMSG_ARENA_TEAM_REMOVE: u16 = 0x0354; // 852
pub const CMSG_ARENA_TEAM_DISBAND: u16 = 0x0355; // 853
pub const CMSG_ARENA_TEAM_LEADER: u16 = 0x0356; // 854
pub const SMSG_ARENA_TEAM_EVENT: u16 = 0x0357; // 855
pub const SMSG_ARENA_TEAM_STATS: u16 = 0x035B; // 859
/// Sent on adopting a new area spirit healer: `u64 guid`; [`SMSG_AREA_SPIRIT_HEALER_TIME`] answers.
pub const CMSG_AREA_SPIRIT_HEALER_QUERY: u16 = 0x02E2; // 738
/// `AcceptAreaSpiritHeal()`: the cached area healer's `u64 guid`, never an argument.
pub const CMSG_AREA_SPIRIT_HEALER_QUEUE: u16 = 0x02E3; // 739
/// The healer's next resurrection: `u64 guid`, `u32 ms`. For the cached healer and a positive
/// time it arms the deadline and fires `AREA_SPIRIT_HEALER_IN_RANGE`.
pub const SMSG_AREA_SPIRIT_HEALER_TIME: u16 = 0x02E4; // 740
/// Right-click a meeting stone (GameObject type 23): `u64` guid, sent by `0x4c9ff0`. A stone never
/// sends [`CMSG_GAMEOBJ_USE`], which vmangos ignores for type 23 (`GameObject.cpp:1836`).
pub const CMSG_MEETINGSTONE_JOIN: u16 = 0x0292; // 658
/// `CancelMeetingStoneRequest()`: empty, party leader only; the server's `0x295` reply clears the
/// state. The name is the emulators'; the number is the client's.
pub const CMSG_MEETINGSTONE_LEAVE: u16 = 0x0293; // 659
/// The meeting-stone queue state: `u32 areaId`, `u8 status`; always stored, a message per
/// status, then `MEETINGSTONE_CHANGED`.
pub const SMSG_MEETINGSTONE_SETQUEUE: u16 = 0x0295; // 661
/// `u64` mailbox guid: the server opens the mail window on it (`0x56dbc0`).
pub const SMSG_SHOW_MAILBOX: u16 = 0x0297; // 663
/// Display only: empty; `ERR_MEETING_STONE_IN_PROGRESS`.
pub const SMSG_MEETINGSTONE_IN_PROGRESS: u16 = 0x0298; // 664
/// Display only: `u64 guid`; `ERR_MEETING_STONE_MEMBER_ADDED_S` once the name resolves.
pub const SMSG_MEETINGSTONE_MEMBER_ADDED: u16 = 0x0299; // 665
/// `ConfirmPetUnlearn()`: the latched `u64 trainerGuid`, never an argument.
pub const CMSG_PET_UNLEARN: u16 = 0x02F0; // 752
/// The pet trainer asks: `u64 trainerGuid`, `u32 costCopper`, latched for
/// `CONFIRM_PET_UNLEARN`; a zero guid shows `ERR_TALENT_WIPE_ERROR` instead.
pub const SMSG_PET_UNLEARN_CONFIRM: u16 = 0x02F1; // 753

/// A summon offer (warlock, meeting stone, GM). The client can only accept, with
/// `CMSG_SUMMON_RESPONSE` (`0x48b770`); a decline is silence until the two-minute timeout.
pub const SMSG_SUMMON_REQUEST: u16 = 0x02AB; // 683
pub const CMSG_SUMMON_RESPONSE: u16 = 0x02AC; // 684

/// Abandon a skill line; no ack, the removal arrives as a `PLAYER_SKILL_INFO` update.
pub const CMSG_UNLEARN_SKILL: u16 = 0x0202; // 514

/// Our PvP flag: an empty body toggles it (`TogglePVP`, `0x516840`), one byte sets a state
/// (`SetPVP`, `0x5168b0`). No ack; the flag arrives in `UNIT_FIELD_FLAGS`, and turning it off
/// waits out the server's timer.
pub const CMSG_TOGGLE_PVP: u16 = 0x0253; // 595

/// Toggle `PLAYER_FLAGS_HIDE_HELM`: an empty body and a pure toggle, so send only when the held
/// flag differs (`CharacterHandler.cpp:753-761`). No ack; the public flag hides it for everyone.
pub const CMSG_TOGGLE_HELM: u16 = 0x02B9; // 697
/// [`CMSG_TOGGLE_HELM`] for `PLAYER_FLAGS_HIDE_CLOAK`.
pub const CMSG_TOGGLE_CLOAK: u16 = 0x02BA; // 698

pub const CMSG_AUTOSTORE_LOOT_ITEM: u16 = 0x0108; // 264
pub const CMSG_LOOT: u16 = 0x015D; // 349
pub const CMSG_LOOT_MONEY: u16 = 0x015E; // 350
pub const CMSG_LOOT_RELEASE: u16 = 0x015F; // 351
pub const SMSG_LOOT_RESPONSE: u16 = 0x0160; // 352
pub const SMSG_LOOT_RELEASE_RESPONSE: u16 = 0x0161; // 353
pub const SMSG_LOOT_REMOVED: u16 = 0x0162; // 354
pub const SMSG_LOOT_MONEY_NOTIFY: u16 = 0x0163; // 355
pub const SMSG_LOOT_CLEAR_MONEY: u16 = 0x0165; // 357
pub const SMSG_ITEM_PUSH_RESULT: u16 = 0x0166; // 358

// Need/Greed/Pass rolls: group or need-before-greed loot on a drop at or above the threshold.
pub const SMSG_LOOT_ALL_PASSED: u16 = 0x029E; // 670
pub const SMSG_LOOT_ROLL_WON: u16 = 0x029F; // 671
pub const CMSG_LOOT_ROLL: u16 = 0x02A0; // 672
pub const SMSG_LOOT_START_ROLL: u16 = 0x02A1; // 673
pub const SMSG_LOOT_ROLL: u16 = 0x02A2; // 674

// Master loot: no roll; the master looter gets the eligible members when the window opens.
pub const CMSG_LOOT_MASTER_GIVE: u16 = 0x02A3; // 675
pub const SMSG_LOOT_MASTER_LIST: u16 = 0x02A4; // 676

// Death: `CMSG_REPOP_REQUEST` is one byte; our `MSG_CORPSE_QUERY` request is empty.
pub const CMSG_REPOP_REQUEST: u16 = 0x015A; // 346
pub const SMSG_RESURRECT_REQUEST: u16 = 0x015B; // 347
pub const CMSG_RESURRECT_RESPONSE: u16 = 0x015C; // 348
pub const CMSG_RECLAIM_CORPSE: u16 = 0x01D2; // 466
pub const MSG_CORPSE_QUERY: u16 = 0x0216; // 534
pub const CMSG_SPIRIT_HEALER_ACTIVATE: u16 = 0x021C; // 540
pub const SMSG_SPIRIT_HEALER_CONFIRM: u16 = 0x0222; // 546
pub const SMSG_CORPSE_RECLAIM_DELAY: u16 = 0x0269; // 617
/// `UseSoulstone()`: empty body; the server casts our `PLAYER_SELF_RES_SPELL` and zeroes it
/// (`SpellHandler.cpp:461`). No answer packet, only descriptor deltas.
pub const CMSG_SELF_RES: u16 = 0x02B3; // 691
/// A non-PvP death's 10% durability loss: empty body, a red error line (`Unit.cpp:1170-1182`).
pub const SMSG_DURABILITY_DAMAGE_DEATH: u16 = 0x02BD; // 701

// Mover modes, acked (`IsFlagAckOpcode`): the SMSG is a packed guid and `u32 counter`; the ack
// echoes the packed guid, the counter and a `MovementInfo`, plus an `f32` apply for all but root
// (`0x71ef80`). An unacked change never reaches observers.
pub const SMSG_MOVE_WATER_WALK: u16 = 0x00DE; // 222
pub const SMSG_MOVE_LAND_WALK: u16 = 0x00DF; // 223
pub const CMSG_MOVE_WATER_WALK_ACK: u16 = 0x02D0; // 720
pub const SMSG_FORCE_MOVE_ROOT: u16 = 0x00E8; // 232
pub const CMSG_FORCE_MOVE_ROOT_ACK: u16 = 0x00E9; // 233
pub const SMSG_FORCE_MOVE_UNROOT: u16 = 0x00EA; // 234
pub const CMSG_FORCE_MOVE_UNROOT_ACK: u16 = 0x00EB; // 235
pub const SMSG_MOVE_FEATHER_FALL: u16 = 0x00F2; // 242
pub const SMSG_MOVE_NORMAL_FALL: u16 = 0x00F3; // 243
pub const SMSG_MOVE_SET_HOVER: u16 = 0x00F4; // 244
pub const SMSG_MOVE_UNSET_HOVER: u16 = 0x00F5; // 245
pub const CMSG_MOVE_HOVER_ACK: u16 = 0x00F6; // 246
pub const CMSG_MOVE_FEATHER_FALL_ACK: u16 = 0x02CF; // 719
/// 3.3.5a only: `MOVEFLAG_CAN_FLY` granted and revoked (handler `0x732450`), one ack for both.
pub const SMSG_MOVE_SET_CAN_FLY: u16 = 0x0343; // 835
pub const SMSG_MOVE_UNSET_CAN_FLY: u16 = 0x0344; // 836
pub const CMSG_MOVE_SET_CAN_FLY_ACK: u16 = 0x0345; // 837
/// 3.3.5a only: our mover's flight start or stop, a plain `MovementInfo` report (`0x6ef860`).
pub const CMSG_MOVE_SET_FLY: u16 = 0x0346; // 838

// The observers' leg of those modes and of teleports (`MovementPacketSender.h:30-60`): the relay
// shape `[packed guid][MovementInfo]`, handled like the relay opcodes (`0x603bb0`). Only root has
// two opcodes; apply is in the flags word. `MSG_MOVE_TELEPORT` has no counter and comes twice per
// near teleport, both with the destination; a creature blink arrives the same way.
pub const MSG_MOVE_TELEPORT: u16 = 0x00C5; // 197
pub const MSG_MOVE_ROOT: u16 = 0x00EC; // 236
pub const MSG_MOVE_UNROOT: u16 = 0x00ED; // 237
pub const MSG_MOVE_HOVER: u16 = 0x00F7; // 247
pub const MSG_MOVE_FEATHER_FALL: u16 = 0x02B0; // 688
pub const MSG_MOVE_WATER_WALK: u16 = 0x02B1; // 689
/// 3.3.5a only: another unit's can-fly change, the relay shape (`0x741b60`).
pub const MSG_MOVE_UPDATE_CAN_FLY: u16 = 0x03AD; // 941

// Knockback. `SMSG_MOVE_KNOCK_BACK` to the mover: packed guid, `u32 counter`, `f32` vcos, vsin,
// speedXY, speedZ, with speedZ down-positive like the jump tail. The mandatory ack is full u64
// guid, counter, `MovementInfo`, whose jump tail must echo the four floats within 0.01 with
// `MOVEFLAG_JUMPING` set; a knockback is never re-sent (`Unit.cpp:6912`). Observers get
// `MSG_MOVE_KNOCK_BACK`: the relay shape plus the four floats, re-launched by `0x6026f0`.
pub const SMSG_MOVE_KNOCK_BACK: u16 = 0x00EF; // 239
pub const CMSG_MOVE_KNOCK_BACK_ACK: u16 = 0x00F0; // 240
pub const MSG_MOVE_KNOCK_BACK: u16 = 0x00F1; // 241

pub const CMSG_WHO: u16 = 0x0062; // 98
pub const SMSG_WHO: u16 = 0x0063; // 99
pub const CMSG_CONTACT_LIST: u16 = 0x0066; // 102
pub const SMSG_CONTACT_LIST: u16 = 0x0067; // 103
pub const SMSG_FRIEND_STATUS: u16 = 0x0068; // 104
pub const CMSG_ADD_FRIEND: u16 = 0x0069; // 105
pub const CMSG_DEL_FRIEND: u16 = 0x006A; // 106
pub const CMSG_SET_CONTACT_NOTES: u16 = 0x006B; // 107
pub const CMSG_ADD_IGNORE: u16 = 0x006C; // 108
pub const CMSG_DEL_IGNORE: u16 = 0x006D; // 109
/// The voice mute list's add and remove (`0x6b62c0`, `0x6b6410`).
pub const CMSG_ADD_VOICE_IGNORE: u16 = 0x03DB; // 987
pub const CMSG_DEL_VOICE_IGNORE: u16 = 0x03DC; // 988

pub const CMSG_GUILD_QUERY: u16 = 0x0054; // 84
pub const SMSG_GUILD_QUERY_RESPONSE: u16 = 0x0055; // 85
/// Never handled (`STATUS_NEVER`, `Opcodes.cpp:210`): 1.12 guilds are founded by petition.
pub const CMSG_GUILD_CREATE: u16 = 0x0081; // 129
pub const CMSG_GUILD_INVITE: u16 = 0x0082; // 130
pub const SMSG_GUILD_INVITE: u16 = 0x0083; // 131
pub const CMSG_GUILD_ACCEPT: u16 = 0x0084; // 132
pub const CMSG_GUILD_DECLINE: u16 = 0x0085; // 133
pub const SMSG_GUILD_DECLINE: u16 = 0x0086; // 134
pub const CMSG_GUILD_INFO: u16 = 0x0087; // 135
pub const SMSG_GUILD_INFO: u16 = 0x0088; // 136
pub const CMSG_GUILD_ROSTER: u16 = 0x0089; // 137
/// The roster, cut by the sender at `0x8000 - 4` bytes; one field is conditional (see
/// `super::guild::read_guild_roster`).
pub const SMSG_GUILD_ROSTER: u16 = 0x008A; // 138
/// Both ways: our empty ask, and the reply's `i32` of the day's remaining guild-bank withdrawal
/// (`0x5a4ab0`), -1 unlimited.
pub const MSG_GUILD_BANK_MONEY_WITHDRAWN: u16 = 0x03FE; // 1022
/// Both ways: our empty ask, and our rank's rights and bank rights (`0x5cb9f0`).
pub const MSG_GUILD_PERMISSIONS: u16 = 0x03FD; // 1021
/// The guild bank (`super::guild_bank`): the banker hello, one tab's slots, the moves.
pub const CMSG_GUILD_BANKER_ACTIVATE: u16 = 0x03E6; // 998
pub const CMSG_GUILD_BANK_QUERY_TAB: u16 = 0x03E7; // 999
pub const SMSG_GUILD_BANK_LIST: u16 = 0x03E8; // 1000
pub const CMSG_GUILD_BANK_SWAP_ITEMS: u16 = 0x03E9; // 1001
pub const CMSG_GUILD_BANK_BUY_TAB: u16 = 0x03EA; // 1002
pub const CMSG_GUILD_BANK_UPDATE_TAB: u16 = 0x03EB; // 1003
pub const CMSG_GUILD_BANK_DEPOSIT_MONEY: u16 = 0x03EC; // 1004
pub const CMSG_GUILD_BANK_WITHDRAW_MONEY: u16 = 0x03ED; // 1005
/// Both ways: our tab ask, and the tab's log (`0x5a4800`).
pub const MSG_GUILD_BANK_LOG_QUERY: u16 = 0x03EE; // 1006
/// Both ways: our tab ask, and the tab's text (`0x5a4ae0`).
pub const MSG_QUERY_GUILD_BANK_TEXT: u16 = 0x040A; // 1034
pub const CMSG_SET_GUILD_BANK_TEXT: u16 = 0x040B; // 1035
pub const CMSG_GUILD_PROMOTE: u16 = 0x008B; // 139
pub const CMSG_GUILD_DEMOTE: u16 = 0x008C; // 140
pub const CMSG_GUILD_LEAVE: u16 = 0x008D; // 141
pub const CMSG_GUILD_REMOVE: u16 = 0x008E; // 142
pub const CMSG_GUILD_DISBAND: u16 = 0x008F; // 143
pub const CMSG_GUILD_LEADER: u16 = 0x0090; // 144
pub const CMSG_GUILD_MOTD: u16 = 0x0091; // 145
pub const SMSG_GUILD_EVENT: u16 = 0x0092; // 146
pub const SMSG_GUILD_COMMAND_RESULT: u16 = 0x0093; // 147
pub const CMSG_GUILD_RANK: u16 = 0x0231; // 561
pub const CMSG_GUILD_ADD_RANK: u16 = 0x0232; // 562
pub const CMSG_GUILD_DEL_RANK: u16 = 0x0233; // 563
pub const CMSG_GUILD_SET_PUBLIC_NOTE: u16 = 0x0234; // 564
pub const CMSG_GUILD_SET_OFFICER_NOTE: u16 = 0x0235; // 565
pub const CMSG_GUILD_INFO_TEXT: u16 = 0x02FC; // 764

pub const CMSG_PETITION_SHOWLIST: u16 = 0x01BB; // 443
pub const SMSG_PETITION_SHOWLIST: u16 = 0x01BC; // 444
pub const CMSG_PETITION_BUY: u16 = 0x01BD; // 445
pub const CMSG_PETITION_SHOW_SIGNATURES: u16 = 0x01BE; // 446
/// Answers our `CMSG_PETITION_SHOW_SIGNATURES` and another player's `CMSG_OFFER_PETITION` alike;
/// only `owner` tells them apart.
pub const SMSG_PETITION_SHOW_SIGNATURES: u16 = 0x01BF; // 447
pub const CMSG_PETITION_SIGN: u16 = 0x01C0; // 448
pub const SMSG_PETITION_SIGN_RESULTS: u16 = 0x01C1; // 449
/// Both ways, different bodies: we send the charter item's guid, the owner gets the decliner's.
pub const MSG_PETITION_DECLINE: u16 = 0x01C2; // 450
pub const CMSG_OFFER_PETITION: u16 = 0x01C3; // 451
pub const CMSG_TURN_IN_PETITION: u16 = 0x01C4; // 452
pub const SMSG_TURN_IN_PETITION_RESULTS: u16 = 0x01C5; // 453
pub const CMSG_PETITION_QUERY: u16 = 0x01C6; // 454
pub const SMSG_PETITION_QUERY_RESPONSE: u16 = 0x01C7; // 455
/// Both ways, one body: `u64 item` and the new name; echoed back only on success.
pub const MSG_PETITION_RENAME: u16 = 0x02C1; // 705

pub const CMSG_GROUP_INVITE: u16 = 0x006E; // 110
pub const SMSG_GROUP_INVITE: u16 = 0x006F; // 111
pub const CMSG_GROUP_ACCEPT: u16 = 0x0072; // 114
pub const CMSG_GROUP_DECLINE: u16 = 0x0073; // 115
pub const SMSG_GROUP_DECLINE: u16 = 0x0074; // 116
pub const CMSG_GROUP_UNINVITE: u16 = 0x0075; // 117
pub const CMSG_GROUP_UNINVITE_GUID: u16 = 0x0076; // 118
pub const SMSG_GROUP_UNINVITE: u16 = 0x0077; // 119
pub const CMSG_GROUP_SET_LEADER: u16 = 0x0078; // 120
pub const SMSG_GROUP_SET_LEADER: u16 = 0x0079; // 121
pub const CMSG_LOOT_METHOD: u16 = 0x007A; // 122
pub const CMSG_GROUP_DISBAND: u16 = 0x007B; // 123
pub const SMSG_GROUP_DESTROYED: u16 = 0x007C; // 124
pub const SMSG_GROUP_LIST: u16 = 0x007D; // 125
pub const SMSG_PARTY_MEMBER_STATS: u16 = 0x007E; // 126
pub const SMSG_PARTY_COMMAND_RESULT: u16 = 0x007F; // 127
/// Both ways: our ping has no guid; the server adds ours and relays it (`GroupHandler.cpp:382`).
pub const MSG_MINIMAP_PING: u16 = 0x01D5; // 469
pub const CMSG_GROUP_CHANGE_SUB_GROUP: u16 = 0x027E; // 638
pub const CMSG_REQUEST_PARTY_MEMBER_STATS: u16 = 0x027F; // 639
pub const CMSG_GROUP_SWAP_SUB_GROUP: u16 = 0x0280; // 640
pub const CMSG_GROUP_RAID_CONVERT: u16 = 0x028E; // 654
pub const CMSG_GROUP_ASSISTANT_LEADER: u16 = 0x028F; // 655
pub const SMSG_PARTY_MEMBER_STATS_FULL: u16 = 0x02F2; // 754
/// Both ways; the server's side is mode-prefixed (`Packets/Group.cpp:77-82` read, `:132-147`
/// write).
pub const MSG_RAID_TARGET_UPDATE: u16 = 0x0321; // 801
/// Both ways: an empty body starts a check, a non-empty one answers (`Packets/Group.cpp:84-96`).
pub const MSG_RAID_READY_CHECK: u16 = 0x0322; // 802
/// Ask for our raid lockouts: empty body, sent by `RequestRaidInfo()` on every RaidFrame show.
pub const CMSG_REQUEST_RAID_INFO: u16 = 0x02CD; // 717
/// Our permanent binds: `u32 count`, count × `{u32 mapId, u32 secondsUntilReset, u32 instanceId}`.
pub const SMSG_RAID_INSTANCE_INFO: u16 = 0x02CC; // 716

// Duels (reference handlers registered at `0x4d4710`). A challenge is a `CMSG_CAST_SPELL` of the
// `SPELL_EFFECT_DUEL` (83) spell; there is no opcode to start one.
pub const SMSG_DUEL_REQUESTED: u16 = 0x0167; // 359
pub const SMSG_DUEL_OUTOFBOUNDS: u16 = 0x0168; // 360
pub const SMSG_DUEL_INBOUNDS: u16 = 0x0169; // 361
pub const SMSG_DUEL_COMPLETE: u16 = 0x016A; // 362
pub const SMSG_DUEL_WINNER: u16 = 0x016B; // 363
pub const CMSG_DUEL_ACCEPTED: u16 = 0x016C; // 364
pub const CMSG_DUEL_CANCELLED: u16 = 0x016D; // 365
/// The duel countdown in milliseconds; the client divides by 1000 (`0x4d4aef`). Registered with
/// the 0x167 block (`0x4d474a`) despite its number.
pub const SMSG_DUEL_COUNTDOWN: u16 = 0x02B7; // 695

// Instance difficulty, both `u32 difficulty, u32 set the player's, u32 set the group's` (`0x526b04`).
pub const MSG_SET_DUNGEON_DIFFICULTY: u16 = 0x0329; // 809
pub const MSG_SET_RAID_DIFFICULTY: u16 = 0x04EB; // 1259
/// A packed guid the client stops targeting and focusing (`0x526586`).
pub const SMSG_BREAK_TARGET: u16 = 0x0152; // 338

// Mirror timers (breath, fatigue, feign death): server countdowns; any change re-sends a START.
pub const SMSG_START_MIRROR_TIMER: u16 = 0x01D9; // 473
/// Never sent by vmangos, which re-sends a START instead: the stock `MirrorTimer.lua` errors on
/// a real pause (`Player::SendMirrorTimers`).
pub const SMSG_PAUSE_MIRROR_TIMER: u16 = 0x01DA; // 474
pub const SMSG_STOP_MIRROR_TIMER: u16 = 0x01DB; // 475

// Mail (`CGMailInfo`): a mailbox opens client-side or by `SMSG_SHOW_MAILBOX`; every CMSG leads
// with the mailbox guid.
pub const CMSG_SEND_MAIL: u16 = 0x0238; // 568
pub const SMSG_SEND_MAIL_RESULT: u16 = 0x0239; // 569
pub const CMSG_GET_MAIL_LIST: u16 = 0x023A; // 570
pub const SMSG_MAIL_LIST_RESULT: u16 = 0x023B; // 571
/// A copied letter's text, ask-once by the letter item's guid; the mail list carries bodies inline.
pub const CMSG_ITEM_TEXT_QUERY: u16 = 0x0243; // 579
pub const SMSG_ITEM_TEXT_QUERY_RESPONSE: u16 = 0x0244; // 580
pub const CMSG_MAIL_TAKE_MONEY: u16 = 0x0245; // 581
pub const CMSG_MAIL_TAKE_ITEM: u16 = 0x0246; // 582
/// No response; the client sets the letter's read bit itself.
pub const CMSG_MAIL_MARK_AS_READ: u16 = 0x0247; // 583
pub const CMSG_MAIL_RETURN_TO_SENDER: u16 = 0x0248; // 584
pub const CMSG_MAIL_DELETE: u16 = 0x0249; // 585
pub const CMSG_MAIL_CREATE_TEXT_ITEM: u16 = 0x024A; // 586
/// Both ways: an empty request; the reply is an `f32` (`0.0` unread mail, `-86400.0` none), a `u32`
/// count and that many sender records.
pub const MSG_QUERY_NEXT_MAIL_TIME: u16 = 0x0284; // 644
/// A mail arrived: an `f32` delay until it is delivered.
pub const SMSG_RECEIVED_MAIL: u16 = 0x0285; // 645

// The auction house (`CGAuctionHouse`): every CMSG leads with the auctioneer guid.
/// Both ways: we send the auctioneer guid; the reply adds `u32 houseId` and `u8 enabled`.
pub const MSG_AUCTION_HELLO: u16 = 0x0255; // 597
pub const CMSG_AUCTION_SELL_ITEM: u16 = 0x0256; // 598
pub const CMSG_AUCTION_REMOVE_ITEM: u16 = 0x0257; // 599
/// The Browse search: the filters, `getAll`, then the Browse list's sort criteria (`0x59bcf0`).
pub const CMSG_AUCTION_LIST_ITEMS: u16 = 0x0258; // 600
pub const CMSG_AUCTION_LIST_OWNER_ITEMS: u16 = 0x0259; // 601
pub const CMSG_AUCTION_PLACE_BID: u16 = 0x025A; // 602
/// The sell, cancel or bid verdict, with a tail keyed on its error (`0x59ffb0`).
pub const SMSG_AUCTION_COMMAND_RESULT: u16 = 0x025B; // 603
/// Shared with the owner and bidder list results: `u32 count`, 148-byte records, `u32 totalCount`
/// and the `u32` query delay (`0x59e160`).
pub const SMSG_AUCTION_LIST_RESULT: u16 = 0x025C; // 604
pub const SMSG_AUCTION_OWNER_LIST_RESULT: u16 = 0x025D; // 605
/// To the bidder, won or outbid: `bidOrZero == 0` means won (`0x5a0480`).
pub const SMSG_AUCTION_BIDDER_NOTIFICATION: u16 = 0x025E; // 606
/// To the seller: a new bid, a sale or an expiry (`0x5a0790`).
pub const SMSG_AUCTION_OWNER_NOTIFICATION: u16 = 0x025F; // 607
pub const CMSG_AUCTION_LIST_BIDDER_ITEMS: u16 = 0x0264; // 612
pub const SMSG_AUCTION_BIDDER_LIST_RESULT: u16 = 0x0265; // 613
/// Pushed to a bidder whose auction the seller cancelled (`0x5a0ac0`).
pub const SMSG_AUCTION_REMOVED_NOTIFICATION: u16 = 0x028D; // 653
/// Sold auctions whose money mail is still in transit, asked beside the owner list (`0x59cc50`).
pub const CMSG_AUCTION_LIST_PENDING_SALES: u16 = 0x048F; // 1167
pub const SMSG_AUCTION_LIST_PENDING_SALES: u16 = 0x0490; // 1168

// The world-state table; these two are its only writers (reference `0x48f690`). NPC text's
// `$<n>w`/`$<n>e` tokens read it.
pub const SMSG_INIT_WORLD_STATES: u16 = 0x02C2; // 706
pub const SMSG_UPDATE_WORLD_STATE: u16 = 0x02C3; // 707

// ── Instance and raid lockouts ──

/// "You are now saved to this instance": a `u32` flag, 0 from vmangos (`0x4e7e60`; 1 adds a debug
/// prefix). Deviation: 2 or more prints nothing, because the reference prints an uninitialized
/// buffer.
pub const SMSG_INSTANCE_SAVE_CREATED: u16 = 0x02CB; // 715
/// A raid-lockout warning: `u32 type`, `u32 mapId`, `u32 secondsUntilReset` (`0x49e1c0`).
pub const SMSG_RAID_INSTANCE_MESSAGE: u16 = 0x02FA; // 762
/// "Reset all instances": empty body (`ResetInstances`, `0x48a6b0`).
pub const CMSG_RESET_INSTANCES: u16 = 0x031D; // 797
/// "%s has been reset.": `u32 mapId`; `0x49e470` clears the last-instance latch before reading it.
pub const SMSG_INSTANCE_RESET: u16 = 0x031E; // 798
/// The reset refusal: `u32 reason`, `u32 mapId` (`0x49e540`).
pub const SMSG_INSTANCE_RESET_FAILED: u16 = 0x031F; // 799
/// The dungeon we were last in: `u32 mapId` (`0x49e670`), half of what
/// `CanShowResetInstances()` reads; it shows no line.
pub const SMSG_UPDATE_LAST_INSTANCE: u16 = 0x0320; // 800
/// Whether we hold any permanent bind: `u32` flag (`0x49e6c0`), half of `CanShowResetInstances()`.
pub const SMSG_UPDATE_INSTANCE_OWNERSHIP: u16 = 0x032B; // 811

// ── The dungeon finder (`CGLookingForGroup`, handlers set by `InitializeGame 0x55d440`) ──

/// The world-enter status request (`RequestLFGStatus 0x555920`): empty; answered by
/// `SMSG_LFG_UPDATE_PLAYER` and `SMSG_LFG_UPDATE_PARTY`.
pub const CMSG_LFG_GET_STATUS: u16 = 0x0296; // 662
/// `JoinLFG` (`0x5572d0`): see [`super::lfg_join`].
pub const CMSG_LFG_JOIN: u16 = 0x035C; // 860
/// `LeaveLFG` (`0x553ab0`): empty.
pub const CMSG_LFG_LEAVE: u16 = 0x035D; // 861
/// `SearchLFGJoin` (`0x558ed0`): the raid browser's `u32` slot.
pub const CMSG_LFG_LFR_JOIN: u16 = 0x035E; // 862
/// `SearchLFGLeave` (`0x558f90`): the joined `u32` slot.
pub const CMSG_LFG_LFR_LEAVE: u16 = 0x035F; // 863
/// `AcceptProposal`/`RejectProposal` (`0x554450`): `u32` proposal id, `u8` answer.
pub const CMSG_LFG_PROPOSAL_RESULT: u16 = 0x0362; // 866
/// `SetLFGComment` while joined (`0x5539a0`): the cstring.
pub const CMSG_SET_LFG_COMMENT: u16 = 0x0366; // 870
/// `CompleteLFGRoleCheck` (`0x553e20`): the `u8` roles, 0 to decline.
pub const CMSG_LFG_SET_ROLES: u16 = 0x036A; // 874
/// `SetLFGBootVote` (`0x554960`): the `u8` vote.
pub const CMSG_LFG_SET_BOOT_VOTE: u16 = 0x036C; // 876
/// `RequestLFDPlayerLockInfo` (`0x555990`): empty; answered by `SMSG_LFG_PLAYER_INFO`.
pub const CMSG_LFG_PLAYER_LOCK_INFO_REQUEST: u16 = 0x036E; // 878
/// `LFGTeleport` (`0x556bc0`): `u8` out of the dungeon.
pub const CMSG_LFG_TELEPORT: u16 = 0x0370; // 880
/// `RequestLFDPartyLockInfo` (`0x555a00`): empty; answered by `SMSG_LFG_PARTY_INFO`.
pub const CMSG_LFG_PARTY_LOCK_INFO_REQUEST: u16 = 0x0371; // 881

pub const SMSG_LFG_PLAYER_REWARD: u16 = 0x01FF; // 511
pub const SMSG_LFG_TELEPORT_DENIED: u16 = 0x0200; // 512
pub const SMSG_LFG_OFFER_CONTINUE: u16 = 0x0293; // 659
pub const SMSG_LFG_ROLE_CHOSEN: u16 = 0x02BB; // 699
pub const SMSG_LFG_PROPOSAL_UPDATE: u16 = 0x0361; // 865
pub const SMSG_LFG_ROLE_CHECK_UPDATE: u16 = 0x0363; // 867
pub const SMSG_LFG_JOIN_RESULT: u16 = 0x0364; // 868
pub const SMSG_LFG_QUEUE_STATUS: u16 = 0x0365; // 869
pub const SMSG_LFG_UPDATE_PLAYER: u16 = 0x0367; // 871
pub const SMSG_LFG_UPDATE_PARTY: u16 = 0x0368; // 872
pub const SMSG_LFG_UPDATE_SEARCH: u16 = 0x0369; // 873
pub const SMSG_LFG_BOOT_PROPOSAL_UPDATE: u16 = 0x036D; // 877
pub const SMSG_LFG_PLAYER_INFO: u16 = 0x036F; // 879
pub const SMSG_LFG_PARTY_INFO: u16 = 0x0372; // 882
pub const SMSG_LFG_DISABLED: u16 = 0x0398; // 920
pub const SMSG_OPEN_LFG_DUNGEON_FINDER: u16 = 0x0515; // 1301
/// The map's difficulty on entry: `u32` difficulty, `u32` dynamic difficulty (`0x526530`).
pub const SMSG_INSTANCE_DIFFICULTY: u16 = 0x033B; // 827

// The calendar (`CGCalendar`, `0x5b7820`-`0x5c43b0`): every server message routes through
// `PH_CALENDAR_GROUP` (`0x5c3fe0`) except the command result (`0x6cd380`).
pub const CMSG_CALENDAR_GET_CALENDAR: u16 = 0x0429; // 1065
pub const CMSG_CALENDAR_GET_EVENT: u16 = 0x042A; // 1066
pub const CMSG_CALENDAR_GUILD_FILTER: u16 = 0x042B; // 1067
pub const CMSG_CALENDAR_ARENA_TEAM: u16 = 0x042C; // 1068
pub const CMSG_CALENDAR_ADD_EVENT: u16 = 0x042D; // 1069
pub const CMSG_CALENDAR_UPDATE_EVENT: u16 = 0x042E; // 1070
pub const CMSG_CALENDAR_REMOVE_EVENT: u16 = 0x042F; // 1071
pub const CMSG_CALENDAR_COPY_EVENT: u16 = 0x0430; // 1072
pub const CMSG_CALENDAR_EVENT_INVITE: u16 = 0x0431; // 1073
pub const CMSG_CALENDAR_EVENT_RSVP: u16 = 0x0432; // 1074
pub const CMSG_CALENDAR_EVENT_REMOVE_INVITE: u16 = 0x0433; // 1075
pub const CMSG_CALENDAR_EVENT_STATUS: u16 = 0x0434; // 1076
pub const CMSG_CALENDAR_EVENT_MODERATOR_STATUS: u16 = 0x0435; // 1077
pub const SMSG_CALENDAR_SEND_CALENDAR: u16 = 0x0436; // 1078
pub const SMSG_CALENDAR_SEND_EVENT: u16 = 0x0437; // 1079
pub const SMSG_CALENDAR_FILTER_GUILD: u16 = 0x0438; // 1080
pub const SMSG_CALENDAR_ARENA_TEAM: u16 = 0x0439; // 1081
pub const SMSG_CALENDAR_EVENT_INVITE: u16 = 0x043A; // 1082
pub const SMSG_CALENDAR_EVENT_INVITE_REMOVED: u16 = 0x043B; // 1083
pub const SMSG_CALENDAR_EVENT_STATUS: u16 = 0x043C; // 1084
pub const SMSG_CALENDAR_COMMAND_RESULT: u16 = 0x043D; // 1085
pub const SMSG_CALENDAR_RAID_LOCKOUT_ADDED: u16 = 0x043E; // 1086
pub const SMSG_CALENDAR_RAID_LOCKOUT_REMOVED: u16 = 0x043F; // 1087
pub const SMSG_CALENDAR_EVENT_INVITE_ALERT: u16 = 0x0440; // 1088
pub const SMSG_CALENDAR_EVENT_INVITE_REMOVED_ALERT: u16 = 0x0441; // 1089
pub const SMSG_CALENDAR_EVENT_INVITE_STATUS_ALERT: u16 = 0x0442; // 1090
pub const SMSG_CALENDAR_EVENT_REMOVED_ALERT: u16 = 0x0443; // 1091
pub const SMSG_CALENDAR_EVENT_UPDATED_ALERT: u16 = 0x0444; // 1092
pub const SMSG_CALENDAR_EVENT_MODERATOR_STATUS_ALERT: u16 = 0x0445; // 1093
pub const CMSG_CALENDAR_COMPLAIN: u16 = 0x0446; // 1094
pub const CMSG_CALENDAR_GET_NUM_PENDING: u16 = 0x0447; // 1095
pub const SMSG_CALENDAR_SEND_NUM_PENDING: u16 = 0x0448; // 1096
pub const SMSG_CALENDAR_EVENT_INVITE_NOTES: u16 = 0x0460; // 1120
pub const SMSG_CALENDAR_EVENT_INVITE_NOTES_ALERT: u16 = 0x0461; // 1121
pub const SMSG_CALENDAR_RAID_LOCKOUT_UPDATED: u16 = 0x0471; // 1137
pub const CMSG_CALENDAR_EVENT_SIGN_UP: u16 = 0x04BA; // 1210
pub const SMSG_CALENDAR_CLEAR_PENDING_ACTION: u16 = 0x04BB; // 1211

/// The group outside a battleground, sent while in one (`Packet_SMSG_REAL_GROUP_UPDATE` `0x6cc300`).
pub const SMSG_REAL_GROUP_UPDATE: u16 = 0x0397; // 919
