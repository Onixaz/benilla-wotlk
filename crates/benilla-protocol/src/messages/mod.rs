//! World (`mangosd`) packet bodies: server packets decode into [`ServerPacket`] via
//! [`parse_server`], client bodies are built here; the header framing lives in [`crate::world`].

mod achievement;
mod action_bar;
pub mod addons;
mod area_trigger;
mod arena_team;
mod attack;
mod auction;
mod aura;
mod bank;
mod battlefield;
mod binder;
mod broadcast;
mod channel;
mod chat;
mod client;
mod combat_log;
mod death;
mod duel;
mod equipment_set;
mod gameobject;
mod gm_ticket;
mod gossip;
mod group;
mod guild;
mod guild_bank;
mod instance;
mod items;
mod lfg;
mod loot;
mod mail;
mod meeting_stone;
mod mirror_timer;
mod monster_move;
mod movement;
pub mod opcode;
mod opcode_names;
mod packet;
mod page_text;
mod parse;
mod pet;
mod petition;
mod pose;
mod progression;
mod pvp;
mod quest;
mod reputation;
mod roster;
mod rune;
mod skills;
mod social;
mod spellbook;
mod spells;
mod stable;
mod summon;
mod tabard;
mod talents;
mod taxi;
mod totem;
mod trade;
mod trainer;
mod tutorial;
mod update_object;
mod vendor;
mod world_state;
pub use achievement::{
    query_inspect_achievements, AchievementEarned, AllAchievementData, CriteriaProgress,
    InspectAchievements,
};
pub use action_bar::{
    set_action_button, set_actionbar_toggles, ActionButton, ACTION_KIND_EQUIPMENT_SET,
    ACTION_KIND_ITEM, ACTION_KIND_MACRO, ACTION_KIND_SPELL,
};
pub use addons::{
    hidden_from_reply, SecureAddon, STANDARD_MODULUS_CRC, STOCK_SECURE_ADDONS,
    STOCK_SECURE_ADDONS_WOTLK,
};
pub use area_trigger::area_trigger;
pub use arena_team::{
    arena_team_named, ArenaTeamCommandResult, ArenaTeamEvent, ArenaTeamMember, ArenaTeamRecord,
    ArenaTeamRoster, ArenaTeamStats,
};
pub use attack::{attack_swing, hit_info, AttackSwingError, AttackerState};
pub use auction::{
    auction_action, auction_duration, auction_error, auction_filter, auction_hello,
    auction_list_bidder_items, auction_list_items, auction_list_owner_items,
    auction_list_pending_sales, auction_place_bid, auction_remove_item, auction_sell_item,
    auction_sort, AuctionBidderNotification, AuctionCommandTail, AuctionEnchant, AuctionListEntry,
    AuctionListPage, AuctionListQuery, AuctionOwnerNotification, AuctionPendingSale,
    AUCTION_ENCHANT_SLOTS, AUCTION_PAGE_SIZE, AUCTION_RECORD_BYTES,
};
pub use aura::{
    AuraRecord, AuraUpdate, AURA_UPDATE_EFF_INDEX_MASK, AURA_UPDATE_FLAG_CANCELABLE,
    AURA_UPDATE_FLAG_DURATION, AURA_UPDATE_FLAG_HARMFUL, AURA_UPDATE_FLAG_SELF_CAST,
};
pub use bank::{
    autobank_item, autostore_bank_item, bank_slot_result, banker_activate, buy_bank_slot,
};
pub use battlefield::{
    battlefield_list, battlefield_port, battlemaster_join, battlemaster_join_arena,
    bf_mgr_response, leave_battlefield, queue_bg_type, queue_team_size, BattlefieldActive,
    BattlefieldInvite, BattlefieldList, BattlefieldMgrNotice, BattlefieldPosition,
    BattlefieldPositions, BattlefieldReward, BattlefieldStatus, GroupJoinedBattleground,
    PvpLogData, PvpLogRow, PvpLogTeam, BATTLEFIELD_POSITIONS_MAX,
};
pub use binder::{binder_activate, PlayerBound};
pub use channel::{channel_notice, ChannelNoticeTail, ChannelNotify};
pub use chat::{
    chat_tag, ChatMessage, CHAT_MSG_ACHIEVEMENT, CHAT_MSG_AFK, CHAT_MSG_BATTLEGROUND,
    CHAT_MSG_BATTLEGROUND_LEADER, CHAT_MSG_BG_SYSTEM_ALLIANCE, CHAT_MSG_BG_SYSTEM_HORDE,
    CHAT_MSG_BG_SYSTEM_NEUTRAL, CHAT_MSG_CHANNEL, CHAT_MSG_DND, CHAT_MSG_EMOTE, CHAT_MSG_FILTERED,
    CHAT_MSG_GUILD, CHAT_MSG_GUILD_ACHIEVEMENT, CHAT_MSG_IGNORED, CHAT_MSG_MONSTER_EMOTE,
    CHAT_MSG_MONSTER_SAY, CHAT_MSG_MONSTER_WHISPER, CHAT_MSG_MONSTER_YELL, CHAT_MSG_OFFICER,
    CHAT_MSG_PARTY, CHAT_MSG_RAID, CHAT_MSG_RAID_BOSS_EMOTE, CHAT_MSG_RAID_BOSS_WHISPER,
    CHAT_MSG_RAID_LEADER, CHAT_MSG_RAID_WARNING, CHAT_MSG_SAY, CHAT_MSG_SYSTEM, CHAT_MSG_WHISPER,
    CHAT_MSG_WHISPER_INFORM, CHAT_MSG_YELL, MACRO_EXPANDED_TYPES,
};
pub use client::{
    auth_session, auth_session_wotlk, channel_announcements, channel_ban, channel_invite,
    channel_kick, channel_list, channel_moderate, channel_moderator, channel_mute, channel_owner,
    channel_password, channel_set_owner, channel_unban, channel_unmoderator, channel_unmute,
    char_create, creature_query, force_speed_ack, full_guid, join_channel, join_channel_wotlk,
    knock_back_ack, leave_channel, leave_channel_wotlk, messagechat, messagechat_channel,
    messagechat_kind, messagechat_whisper, move_flag_ack, move_spline_done, move_time_skipped,
    movement, pet_name_query, ping, played_time, query_time, random_roll, teleport_ack, text_emote,
};
pub use combat_log::{
    spell_hit_info, BuildingDamage, DamageShield, DispelFailed, EnchantmentLog,
    EnvironmentalDamageLog, ExecuteLog, PartyKillLog, PeriodicAuraLog, PeriodicTick,
    SpellDamageLog, SpellEnergizeLog, SpellHealLog, SpellInstaKillLog, SpellLogExecute,
    SpellLogMiss, SpellOutcomeLog, SpellRemoveKind, SpellRemoveLog,
};
pub use death::{
    area_spirit_healer, reclaim_corpse, repop_request, resurrect_response, spirit_healer_activate,
    AreaSpiritHealerTime, CorpseLocation, ResurrectRequestBody,
};
pub use duel::{
    duel_accepted, duel_cancelled, read_duel_complete, read_duel_countdown, read_duel_requested,
    read_duel_winner, DuelRequested, DuelWinner,
};
pub use equipment_set::{EquipmentSet, EQUIPMENT_SET_SLOTS, MAX_EQUIPMENT_SETS};
pub use gameobject::{gameobj_use, gameobject_query, GameObjectQueryInfo};
pub use gm_ticket::{
    gm_ticket_create, gm_ticket_updatetext, GmResponse, GmTicket, GMTICKET_QUEUE_ENABLED,
    GMTICKET_RESPONSE_TICKET_DELETED, GMTICKET_STATUS_DBERROR, GMTICKET_STATUS_DEFAULT,
    GMTICKET_STATUS_HASTEXT,
};
pub use gossip::{
    gossip_hello, gossip_select_option, npc_text_query, select_greeting, GossipMenu, GossipOption,
    GossipPoi, NpcTextBlock, QuestOption, GOSSIP_MAX_ITEMS, NPC_TEXT_BLOCKS,
};
pub use group::{
    group_accept, group_assistant_leader, group_change_sub_group, group_decline, group_disband,
    group_invite, group_raid_convert, group_set_leader, group_swap_sub_group, group_uninvite,
    group_uninvite_guid, loot_method, member_status, minimap_ping, party_member_mask,
    party_operation, party_result, patch_auras, raid_target_request, raid_target_set,
    ready_check_answer, ready_check_start, request_party_member_stats, request_raid_info,
    DifficultySet, GroupDifficulties, GroupLootInfo, GroupMemberEntry, PartyMemberStatsInfo,
    RaidInstanceEntry, RaidTargetUpdate, ReadyCheck, RecordAura, GROUP_MEMBER_ASSISTANT,
    GROUP_MEMBER_MAIN_ASSIST, GROUP_MEMBER_MAIN_TANK,
};
pub use guild::{
    guild_accept, guild_add_rank, guild_bank_money_param, guild_bank_money_withdrawn,
    guild_command, guild_command_error, guild_create, guild_decline, guild_default_rank,
    guild_del_rank, guild_demote, guild_disband, guild_event, guild_info, guild_info_text,
    guild_invite, guild_leader, guild_leave, guild_motd, guild_permissions, guild_presence,
    guild_promote, guild_query, guild_rank, guild_rank_right, guild_remove, guild_roster,
    guild_set_officer_note, guild_set_public_note, GuildCommandResult, GuildEventNotice, GuildInfo,
    GuildPermissions, GuildQueryResponse, GuildRankBank, GuildRoster, GuildRosterMember,
    GUILD_BANK_TABS, GUILD_INFO_MAX_LENGTH, GUILD_MOTD_MAX_LENGTH, GUILD_NAME_MAX_LENGTH,
    GUILD_NOTE_MAX_LENGTH, GUILD_RANKS_MAX_COUNT, GUILD_RANKS_MIN_COUNT, GUILD_RANK_MAX_LENGTH,
    GUILD_RANK_RIGHT_ORDER,
};
pub use guild_bank::{
    guild_bank_activate, guild_bank_buy_tab, guild_bank_log, guild_bank_money,
    guild_bank_query_tab, guild_bank_set_text, guild_bank_swap_items, guild_bank_tab_query,
    guild_bank_update_tab, GuildBankList, GuildBankLog, GuildBankLogEntry, GuildBankSend,
    GuildBankSlot, GuildBankSwap, GuildBankTab, GUILD_BANK_MONEY_LOG_TAB, GUILD_BANK_TAB_SLOTS,
};
pub use instance::{
    reset_instances, InstanceResetFailed, InstanceResetFailure, RaidGroupOnly, RaidInstanceMessage,
    RaidInstanceWarning,
};
pub use items::{
    auto_equip_item, auto_store_bag_item, decode_item_query_response, destroy_item, item_query,
    open_item, set_ammo, split_item, swap_inv_item, swap_item, use_item, wrap_item, ItemDamage,
    ItemInfo, ItemSpellEntry, ItemUseSpell, UseItemTarget, BAG_PLAYER_INVENTORY, BANK_BAGS,
    BANK_SLOTS, ITEM_DYNFLAG_UNLOCKED, ITEM_DYNFLAG_WRAPPED, ITEM_FLAG_LOOTABLE, ITEM_FLAG_WRAPPER,
    KEYRING_SLOTS, SLOT_BAG_FIRST, SLOT_BANK_BAG_FIRST, SLOT_BANK_FIRST, SLOT_KEYRING_FIRST,
    SLOT_PACK_FIRST,
};
pub use lfg::{
    lfg_comment, lfg_join, lfg_proposal_result, LfgBootProposal, LfgCompletionReward,
    LfgDungeonReward, LfgJoin, LfgMemberLocks, LfgPacket, LfgProposal, LfgProposalMember,
    LfgQueueStatus, LfgRewardItem, LfgRoleCheck, LfgRoleCheckMember, LfgUpdate,
};
pub use loot::{
    autostore_loot_item, loot, loot_error, loot_master_give, loot_money, loot_release, loot_roll,
    loot_type, roll_vote, slot_type, ItemPushResult, LootAllPassed, LootItem, LootResponseBody,
    LootRoll, LootRollWon, LootStartRoll,
};
pub use mail::{
    get_mail_list, item_text_query, mail_action, mail_create_text_item, mail_delete,
    mail_delete_reason, mail_error, mail_flags, mail_mark_as_read, mail_message_type,
    mail_return_to_sender, mail_take_item, mail_take_money, send_mail, MailAttachment, MailEnchant,
    MailList, MailListEntry, MailResult, MailSender, MAIL_ATTACHMENTS_MAX, MAIL_RECENT_SENDERS,
};
pub use meeting_stone::{
    meeting_stone_join, meeting_stone_leave, MeetingStoneNotice, MeetingStoneSetQueue,
};
pub use mirror_timer::{
    read_pause_mirror_timer, read_start_mirror_timer, read_stop_mirror_timer, MirrorTimerKind,
    MirrorTimerStart,
};
pub use movement::{
    JumpInfo, MoveMode, MovementInfo, RelayVerb, SpeedKind, SplineMode, TransportPose,
    CLASSIC_ASCENDING, CLASSIC_CAN_FLY, CLASSIC_DESCENDING, CLASSIC_FLYING,
};
pub use opcode_names::opcode_name;
pub use packet::{CreatureQueryInfo, MonsterMoveFacing, ServerPacket};
pub use page_text::page_text_query;
pub use parse::{parse_auth_response_wotlk, parse_server, parse_server_with_tail};
pub use pet::{
    pet_abandon, pet_action, pet_cancel_aura, pet_rename, pet_set_action, pet_spell_autocast,
    pet_stop_attack, pet_tame_failure_key, pet_unlearn, PetActionEntry, PetMode, PetSpellCooldown,
    PetSpells, PetUnlearnConfirm, PET_ACTION_SLOTS, PET_ACT_COMMAND, PET_ACT_DISABLED,
    PET_ACT_ENABLED, PET_ACT_PASSIVE, PET_ACT_REACTION, PET_AUTOCAST_ALLOWED, PET_AUTOCAST_ON,
    PET_COMMAND_ATTACK, PET_COMMAND_DISMISS, PET_COMMAND_FOLLOW, PET_COMMAND_STAY,
    PET_COOLDOWN_PERMANENT, PET_REACT_AGGRESSIVE, PET_REACT_DEFENSIVE, PET_REACT_PASSIVE,
    PET_STATE_VEHICLE, PET_TALK_ATTACK, PET_TALK_ORDER, PET_TYPE_SPELL, PET_TYPE_SPELL_EXTRA,
    PET_UNUSABLE_UNIT_FLAGS,
};
pub use petition::{
    offer_petition, petition_buy, petition_decline, petition_query, petition_rename,
    petition_result, petition_show_list, petition_show_signatures, petition_sign,
    turn_in_arena_petition, turn_in_petition, ArenaBanner, PetitionQueryResponse, PetitionRename,
    PetitionShowList, PetitionShowListEntry, PetitionShowSignatures, PetitionSignResults,
    PetitionSignature, CHARTER_DISPLAY_ID, CHARTER_ITEM_ENTRY, CHARTER_NAME_MAX_LENGTH,
    ITEM_FLAG_CHARTER, MAX_PETITION_SIGNATURES, PETITION_TYPE_ARENA,
};
pub use pose::{set_sheathed, stand_state_change};
pub use progression::{
    learn_talent, talent_wipe_confirm, ExplorationXp, LevelUpInfo, TalentWipeConfirm, XpGain,
};
pub use pvp::{inspect_honor_stats, InspectArenaTeam, InspectHonorStats, PvpCredit};
pub use quest::{
    dialog_status, push_quest_to_party, quest_confirm_accept, quest_flags, quest_poi_query,
    quest_push_result, quest_query, questgiver_accept_quest, questgiver_choose_reward,
    questgiver_complete_quest, questgiver_hello, questgiver_query_quest, questgiver_request_reward,
    questgiver_status_query, questlog_remove_quest, questlog_swap_quest, QuestComplete,
    QuestConfirmAccept, QuestDetails, QuestExtraRewards, QuestGiverList, QuestListEntry,
    QuestObjective, QuestOfferReward, QuestPoi, QuestPoiBlob, QuestPushResult,
    QuestReputationRewards, QuestRequestItems, QuestRequiredItem, QuestRewardItem, QuestShareMsg,
    QuestTemplate, QUEST_EMOTE_COUNT, QUEST_OBJECTIVES_COUNT, QUEST_REPUTATIONS_COUNT,
    QUEST_REQUIRED_ITEMS_COUNT, QUEST_REWARDS_COUNT, QUEST_REWARD_CHOICES_COUNT,
};
pub use reputation::{
    set_faction_at_war, set_faction_inactive, set_watched_faction, FACTION_LIST_LEN,
    WATCHED_FACTION_NONE,
};
pub use roster::{
    CharCreateReq, CharEnumItem, Character, CHARACTER_FLAG_GHOST, CHARACTER_FLAG_HIDE_CLOAK,
    CHARACTER_FLAG_HIDE_HELM, CHARACTER_FLAG_RENAME, CHAR_CREATE_NAME_IN_USE,
    CHAR_CREATE_SERVER_LIMIT, CHAR_CREATE_SUCCESS, CHAR_DELETE_SUCCESS, CLASS_WARRIOR, GENDER_MALE,
    RACE_HUMAN,
};
pub use rune::{ConvertRune, ResyncRunes};
pub use skills::unlearn_skill;
pub use social::{
    add_friend, add_ignore, del_friend, del_ignore, friend_list, friend_result, friend_status,
    read_friend_list, read_friend_status, read_ignore_list, read_who, set_looking_for_group, who,
    FriendEntry, FriendOnline, FriendStatusUpdate, WhoEntry, WhoRequest, WhoResults,
    WHO_MAX_SEARCH_TERMS, WHO_MAX_ZONES,
};
pub use spellbook::SpellCooldown;
pub use spells::{
    cancel_aura, cast_spell, cast_spell_at_dest, cast_spell_at_source, cast_spell_corpse,
    cast_spell_gameobject, cast_spell_glyph, cast_spell_item, dismiss_critter,
    wotlk_cast_fail_shared, CastCounter, CastOutcome, CastRunes, CorpseTarget, SpellCastTargets,
    SpellChainTargets, SpellGo, SpellStart, WOTLK_CAST_FAIL_KEYS,
};
pub use stable::{
    buy_stable_slot, list_stabled_pets, stable_pet, stable_result, stable_swap_pet, unstable_pet,
    StabledPet,
};
pub use summon::{summon_response, SummonRequest};
pub use tabard::{
    battlemaster_hello, save_guild_emblem, tabard_vendor_activate, GUILD_EMBLEM_RESULT_MESSAGES,
};
pub use talents::{
    learn_preview_talents, learn_preview_talents_pet, pet_learn_talent, remove_glyph,
    LearnedTalent, TalentGroupInfo, TalentsInfo,
};
pub use taxi::{
    activate_taxi, activate_taxi_express, taxi_node_status_query, taxi_query_available_nodes,
    taxi_reply, TaxiMask,
};
pub use totem::{totem_destroyed, TotemCreated};
pub use trade::{
    accept_trade, clear_trade_item, initiate_trade, set_trade_gold, set_trade_item, TradeItem,
    TradeStatus, TradeStatusExtended, TRADE_SLOT_COUNT, TRADE_SLOT_NONTRADED,
    TRADE_SLOT_TRADED_COUNT,
};
pub use trainer::{train_fail, trainer_buy_spell, trainer_list, trainer_spell_state, TrainerSpell};
pub use tutorial::{tutorial_flag, TutorialFlags};
pub use update_object::field;
pub use update_object::{
    power_display_scale, quest_slot_state, CorpseLook, CreateSpline, MovementBlock, MoverState,
    Object, ObjectFields, ObjectType, OwnerFallback, PlayerSkillSlot, QuestLogSlot, UnitAuraSlot,
    AURA_FLAG_CANCELABLE, AURA_FLAG_EFF_INDEX_MASK, FIELD_PLAYER_SKILL_INFO_1_1,
    PLAYER_COMBAT_RATINGS, PLAYER_EXPLORED_ZONES_SLOTS, PLAYER_GLYPH_SOCKETS,
    PLAYER_QUEST_LOG_SLOTS, PLAYER_SKILL_SLOTS, QUEST_LOG_SLOT_FIELDS, UNIT_AURA_POSITIVE_SLOTS,
    UNIT_AURA_SLOTS,
};
pub use vendor::{
    buy_item, buy_item_in_slot, buy_result, buyback_item, list_inventory, repair_item, sell_item,
    sell_result, VendorItem, VendorList, VENDOR_ROWS_MAX,
};
pub use world_state::InitWorldStates;

/// World `SMSG_AUTH_RESPONSE` result codes, not realmd's `AuthLogonResult` (`crate::AuthReject`):
/// the ranges overlap, and 0x0C is `AUTH_OK` here but `AUTH_LOGON_FAILED_SUSPENDED` there. Each
/// name is the `GlueStrings` key the client shows (cmangos `SharedDefines.h:1721`).
pub const AUTH_OK: u8 = 0x0C;
pub const AUTH_FAILED: u8 = 0x0D;
pub const AUTH_REJECT: u8 = 0x0E;
pub const AUTH_BAD_SERVER_PROOF: u8 = 0x0F;
pub const AUTH_UNAVAILABLE: u8 = 0x10;
pub const AUTH_SYSTEM_ERROR: u8 = 0x11;
pub const AUTH_BILLING_ERROR: u8 = 0x12;
pub const AUTH_BILLING_EXPIRED: u8 = 0x13;
pub const AUTH_VERSION_MISMATCH: u8 = 0x14;
pub const AUTH_UNKNOWN_ACCOUNT: u8 = 0x15;
pub const AUTH_INCORRECT_PASSWORD: u8 = 0x16;
pub const AUTH_SESSION_EXPIRED: u8 = 0x17;
pub const AUTH_SERVER_SHUTTING_DOWN: u8 = 0x18;
pub const AUTH_ALREADY_LOGGING_IN: u8 = 0x19;
pub const AUTH_LOGIN_SERVER_NOT_FOUND: u8 = 0x1A;
/// The realm is full and we are queued, not refused: the one code here that is not an ending.
pub const AUTH_WAIT_QUEUE: u8 = 0x1B;
// The 1.12 client's `OKAY_WITH_URL` table (`0x803740`) keys on banned, no-time, db-busy,
// suspended and parental, so the URL dialog is reachable only from this enum, never from realmd.
pub const AUTH_BANNED: u8 = 0x1C;
pub const AUTH_ALREADY_ONLINE: u8 = 0x1D;
pub const AUTH_NO_TIME: u8 = 0x1E;
pub const AUTH_DB_BUSY: u8 = 0x1F;
pub const AUTH_SUSPENDED: u8 = 0x20;
pub const AUTH_PARENTAL_CONTROL: u8 = 0x21;
/// `LogoutResult::Success` (`SMSG_LOGOUT_RESPONSE`).
pub const LOGOUT_SUCCESS: u32 = 0x0;
/// Faction-tongue `Language` ids (vmangos `SharedDefines.h:256-261`); vmangos rejects
/// `Universal` (0) from clients in ordinary chat.
pub const LANGUAGE_COMMON: u32 = 0x7;
pub const LANGUAGE_ORCISH: u32 = 0x1;

/// `LANG_ADDON` (vmangos `SharedDefines.h:270`): marks a chat line as addon data, not speech;
/// 1.12 has no addon opcode, and the client routes such a line to `CHAT_MSG_ADDON`. The server
/// skips language, flood and sanitize checks for it, never rewrites it, and allows it only on the
/// group, guild and channel lanes (`ChatHandler.cpp:49,84,172-219`). The 1.12 `SendAddonMessage`
/// (`0x49f920`) sends only PARTY, RAID, GUILD and BATTLEGROUND, and the receive side names any
/// other lane "UNKNOWN" (`0x49aff4`).
pub const LANGUAGE_ADDON: u32 = 0xFFFF_FFFF;

/// A race's faction tongue. vmangos drops a chat line, `.command`s included, in a language the
/// speaker does not know (`KnowsLanguage`, `Handlers/ChatHandler.cpp`); races 2/5/6/8 learn 669
/// Language Orcish, the rest 668 Language Common (`playercreateinfo_spell`).
pub fn faction_language(race: u8) -> u32 {
    match race {
        2 | 5 | 6 | 8 => LANGUAGE_ORCISH, // orc, undead, tauren, troll
        _ => LANGUAGE_COMMON,             // human, dwarf, night elf, gnome
    }
}
/// The `ChatMsg` lanes the client sends as `CMSG_MESSAGECHAT`'s `u32` `type`: `SendChatMessage`'s
/// name table (`0x4fbb20`), whose type defaults to SAY = 1 (`0x50d1d8`). It has no RAID_LEADER or
/// BATTLEGROUND_LEADER: the client never sends either. The inbound `u8` field has its own set,
/// [`CHAT_MSG_SAY`] etc.
pub const CHAT_TYPE_SAY: u32 = 0x01;
pub const CHAT_TYPE_PARTY: u32 = 0x02;
pub const CHAT_TYPE_RAID: u32 = 0x03;
pub const CHAT_TYPE_GUILD: u32 = 0x04;
pub const CHAT_TYPE_OFFICER: u32 = 0x05;
pub const CHAT_TYPE_YELL: u32 = 0x06;
pub const CHAT_TYPE_WHISPER: u32 = 0x07;
pub const CHAT_TYPE_EMOTE: u32 = 0x0A;
pub const CHAT_TYPE_CHANNEL: u32 = 0x11;
pub const CHAT_TYPE_AFK: u32 = 0x17;
pub const CHAT_TYPE_DND: u32 = 0x18;
pub const CHAT_TYPE_RAID_WARNING: u32 = 0x28;
pub const CHAT_TYPE_BATTLEGROUND: u32 = 0x2C;
