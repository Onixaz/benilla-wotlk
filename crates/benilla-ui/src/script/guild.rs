//! The guild bindings: roster, ranks, notes, MOTD and the membership verbs. The app pushes a
//! display-ready [`GuildState`] ([`UiScript::set_guild`]) that joins two caches: the roster (MOTD,
//! info text, rank rights, members) and the guild query (the name and ten rank names). Each verb
//! queues a [`GuildRequest`] the app drains ([`UiScript::take_guild_requests`]). 12340's table is
//! `GuildInfoRegisterScriptFunctions` (`0x5ca380`, registrar `0xad0840`).
//!
//! Two index bases: a roster `rankIndex` is 0-based with 0 the guild master, while the
//! `GuildControl*` family is 1-based, driven by the dropdown IDs (`FriendsFrame.lua:1760-1780`).

use mlua::{Lua, MultiValue, Value};

use super::binding_abi;
use super::Model;

/// The seventeen rank rights in `GuildControlGetRankFlags`' return order, which is also
/// `GuildControlSetRankFlag`'s index and the `GuildControlPopupFrameCheckbox<i>` order
/// (`FriendsFrame.lua:1782-1793`): 12340's `{mask, enabled}` table at `0xad07b8`. Not bit order:
/// Promote and Demote (5, 6) are `0x80` and `0x100`, above Invite and Remove (7, 8).
pub const RANK_RIGHT_BITS: [u32; 17] = [
    0x0000_0001, // 1  Guildchat Listen
    0x0000_0002, // 2  Guildchat Speak
    0x0000_0004, // 3  Officerchat Listen
    0x0000_0008, // 4  Officerchat Speak
    0x0000_0080, // 5  Promote
    0x0000_0100, // 6  Demote
    0x0000_0010, // 7  Invite Member
    0x0000_0020, // 8  Remove Member
    0x0000_1000, // 9  Set MOTD
    0x0000_2000, // 10 Edit Public Note
    0x0000_4000, // 11 View Officer Note
    0x0000_8000, // 12 Edit Officer Note
    0x0001_0000, // 13 Modify Guild Info
    0x0002_0000, // 14 the deprecated flag, which has no checkbox
    0x0004_0000, // 15 Withdraw Repair
    0x0008_0000, // 16 Withdraw Gold
    0x0010_0000, // 17 Create Guild Event
];

/// The three bank-tab rights `SetGuildBankTabPermissions` indexes, 1-based (`0xad07ac`): view,
/// deposit, update text.
const BANK_TAB_RIGHT_BITS: [u32; 3] = [0x1, 0x2, 0x4];

/// The bank tabs a rank record carries (`0x5cb285`'s loop of 6).
pub const BANK_TABS: usize = 6;

/// One-based indices into [`RANK_RIGHT_BITS`], numbered as `GUILDCONTROL_OPTION<n>`.
mod right {
    pub(super) const PROMOTE: usize = 5;
    pub(super) const DEMOTE: usize = 6;
    pub(super) const INVITE: usize = 7;
    pub(super) const REMOVE: usize = 8;
    pub(super) const SET_MOTD: usize = 9;
    pub(super) const EDIT_PUBLIC_NOTE: usize = 10;
    pub(super) const VIEW_OFFICER_NOTE: usize = 11;
    pub(super) const EDIT_OFFICER_NOTE: usize = 12;
    pub(super) const MODIFY_GUILD_INFO: usize = 13;
    pub(super) const WITHDRAW_REPAIR: usize = 15;
}

/// The fewest ranks: `GuildControlDelRank` sends only above it (`0x5cb3f0`).
pub const MIN_RANKS: usize = 5;
/// The most ranks: `GuildControlAddRank` sends only below it (`0x5cb330`).
pub const MAX_RANKS: usize = 10;

/// The rank indices `GuildControlSetRank` loads, 0-based; past them it clears the selection
/// (`0x5ca090`, `rank > 16`).
const CONTROL_RANKS: u32 = 17;

/// `GetGuildRosterLastOnline`'s four units, decomposed by the app from the wire's one `f32` of
/// days offline (`0x5cabb0`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LastOnline {
    pub years: u32,
    pub months: u32,
    pub days: u32,
    pub hours: u32,
}

/// A unit's guild as `GetGuildInfo(unit)` reports it. `PLAYER_GUILDID` and `PLAYER_GUILDRANK` are
/// public descriptor fields, so every visible player carries one; the app names them through its
/// guild query cache.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnitGuild {
    /// Never empty: an unresolved guild is `None`, as the binding takes the same nil path on a
    /// cache miss as for no guild.
    pub name: String,
    pub rank_name: String,
    /// 0-based, 0 the guild master.
    pub rank_index: u32,
}

/// One guild rank: its name from the guild query, its rights and bank half from the roster
/// (12340's 56-byte record at `0xc21e60`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildRankInfo {
    /// Empty until the guild query names it.
    pub name: String,
    /// Tested with [`RANK_RIGHT_BITS`].
    pub rights: u32,
    /// The daily gold withdrawal: copper off the roster, and the gold amount
    /// `SetGuildBankWithdrawLimit` stores in place until the next roster (`0x5cb6f4`).
    pub bank_money_per_day: u32,
    /// Per bank tab: its rights and its daily item withdrawals.
    pub bank_tabs: Vec<(u32, u32)>,
}

impl GuildRankInfo {
    /// Bank tab `tab` (0-based), zeros past the record.
    fn tab(&self, tab: usize) -> (u32, u32) {
        self.bank_tabs.get(tab).copied().unwrap_or_default()
    }

    /// A mutable bank tab, growing the record to [`BANK_TABS`] first.
    fn tab_mut(&mut self, tab: usize) -> &mut (u32, u32) {
        if self.bank_tabs.len() < BANK_TABS {
            self.bank_tabs.resize(BANK_TABS, (0, 0));
        }
        &mut self.bank_tabs[tab]
    }
}

/// One roster row resolved for display: `GetGuildRosterInfo`'s eleven returns in order, then the
/// last-online time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildMemberInfo {
    /// On the wire in the roster itself, so it never waits on a name query.
    pub name: String,
    /// The rank's name.
    pub rank: String,
    /// 0-based, 0 the guild master; the `GuildControl*` family is 1-based.
    pub rank_index: u32,
    pub level: u32,
    /// The localized class name in the member's gender; empty, answered nil, without a
    /// `ChrClasses` row.
    pub class: String,
    /// The zone name; empty, answered nil, without an `AreaTable` row.
    pub zone: String,
    /// The public note.
    pub note: String,
    /// Empty both when unset and when the server blanks it for a viewer without View Officer
    /// Note; the frame gates on `CanViewOfficerNote` instead.
    pub officer_note: String,
    pub online: bool,
    /// The tenth return: `""`, `CHAT_FLAG_AFK` or `CHAT_FLAG_DND`.
    pub status: String,
    /// The eleventh return, `ChrClasses`' file name (`"WARRIOR"`); empty, answered nil, without a
    /// row.
    pub class_file: String,
    /// Meaningful only offline; the wire omits it for online members.
    pub last_online: LastOnline,
}

/// The guild snapshot the app pushes whole when it changes ([`UiScript::set_guild`]); the default
/// is guildless.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildState {
    /// `IsInGuild`, from the player's own `PLAYER_GUILDID`, so true before any roster.
    pub in_guild: bool,
    /// Only `SMSG_GUILD_QUERY_RESPONSE` carries it, so it is empty until that query answers.
    pub name: String,
    pub rank_name: String,
    /// The player's own rank, 0-based, 0 the guild master.
    pub rank_index: u32,
    /// `IsGuildLeader`: the player is the guild master.
    pub is_leader: bool,
    /// The player's own rank's rights, which every `Can*` predicate tests.
    pub rights: u32,
    pub motd: String,
    /// The `GuildInfoFrame` body.
    pub info_text: String,
    /// The whole roster, sorted and never filtered: `GetGuildRosterInfo`,
    /// `GetGuildRosterLastOnline` and `SetGuildRosterSelection` index it 1-based.
    pub roster: Vec<GuildMemberInfo>,
    /// `GetNumGuildMembers()`, the UI's loop bound, which respects [`Self::show_offline`]. The
    /// index-taking bindings bound against the full count (`[0xc22ab0]`), so an index past this
    /// still returns a real offline member.
    pub num_members: usize,
    /// Every rank, 0 the guild master; the length is `GuildControlGetNumRanks`.
    pub ranks: Vec<GuildRankInfo>,
    /// The selected row, 1-based into [`Self::roster`], 0 for none.
    pub selection: u32,
    /// Changes [`Self::num_members`] only, never [`Self::roster`].
    pub show_offline: bool,
    /// `GetGuildBankMoney`: the guild bank's copper, 0 before any bank-money event.
    pub bank_money: u64,
    /// `GetGuildBankWithdrawMoney`: today's remaining withdrawal in copper, -1 unlimited.
    pub bank_withdraw_money: i32,
    /// The tabard as emblem style and colour, border style and colour, and background; `-1` is
    /// undesigned. `None` until the guild query answers.
    pub tabard: Option<[i32; 5]>,
}

/// The rank-control popup's staging buffer, kept out of [`GuildState`] so a push mid-edit does not
/// discard unsaved checkbox clicks: 12340's `s_guildControlRank` (`0xad07a8`) and the `enabled`
/// halves of the flag table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildRankEdit {
    /// The 1-based rank `GuildControlSetRank` last loaded, 0 for none.
    pub rank: u32,
    /// The seventeen staged checkboxes in [`RANK_RIGHT_BITS`] order, one slot per flag, so the
    /// save leaves every other bit of the live rights alone.
    pub staged: [bool; 17],
}

/// Outbound guild intents, drained by the app ([`UiScript::take_guild_requests`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuildRequest {
    /// `GuildRoster()` (`CMSG_GUILD_ROSTER`). The app throttles it to one per 10 s as the reference
    /// does; the `GUILD_ROSTER_UPDATE` arg1 path clears the throttle first, so the FrameXML's own
    /// request is never the one dropped.
    Roster,
    /// `GuildInfo()`, the `/ginfo` query (`CMSG_GUILD_INFO`), answered as a chat line.
    Info,
    /// `GuildInvite(name)` (`CMSG_GUILD_INVITE`).
    Invite(String),
    /// `GuildUninvite(name)` (`CMSG_GUILD_REMOVE`).
    Uninvite(String),
    /// `GuildPromote(name)`.
    Promote(String),
    /// `GuildDemote(name)`.
    Demote(String),
    /// `GuildSetLeader(name)`.
    SetLeader(String),
    /// `AcceptGuild()`.
    Accept,
    /// `DeclineGuild()`.
    Decline,
    /// `GuildLeave()`.
    Leave,
    /// `GuildDisband()`.
    Disband,
    /// `GuildSetMOTD(text)`; an empty string clears it.
    SetMotd(String),
    /// `SetGuildInfoText(text)`.
    SetInfoText(String),
    /// `GuildRosterSetPublicNote(index, note)`: a 1-based display row the app resolves to the name
    /// the wire takes.
    SetPublicNote { index: u32, note: String },
    /// `GuildRosterSetOfficerNote(index, note)`.
    SetOfficerNote { index: u32, note: String },
    /// `GuildControlSaveRank(name)` as one `CMSG_GUILD_RANK` for the loaded rank (`0x5cb170`):
    /// the rank 0-based, the folded rights, the scrubbed name, the withdraw limit times 10000 and
    /// the six bank tabs' rights and withdrawals.
    SaveRank {
        rank_index: u32,
        rights: u32,
        name: String,
        money_per_day: u32,
        tabs: [(u32, u32); BANK_TABS],
    },
    /// `GuildControlAddRank(name)`.
    AddRank(String),
    /// `GuildControlDelRank()`: no argument and an empty `0x233`; the server deletes the last rank.
    DelRank,
    /// `SetGuildRosterSelection(index)`, mirrored so the next push agrees.
    Select(u32),
    /// `SetGuildRosterShowOffline(flag)`, mirrored so the next push agrees.
    SetShowOffline(bool),
    /// `SortGuildRoster(field)`: `"name"`, `"zone"`, `"level"`, `"class"`, `"rank"`, `"note"` or
    /// `"online"`, the headers' `sortType`s. Sorting is client-side: the app re-orders its roster
    /// and pushes it back.
    Sort(String),
}

impl super::UiScript {
    /// Push the guild snapshot, replacing it. The app fires `GUILD_ROSTER_UPDATE`,
    /// `PLAYER_GUILD_UPDATE` and `GUILD_MOTD` on the edges.
    pub fn set_guild(&mut self, state: GuildState) {
        set_field!(self, guild, state);
    }

    /// Drain the guild intents queued since the last call.
    pub fn take_guild_requests(&mut self) -> Vec<GuildRequest> {
        drain_field!(self, guild_requests)
    }

    /// Queue an intent from a slash command (`/ginvite`, `/gquit`, `/gpromote`, ...). The
    /// reference's slash commands are Lua calling the same bindings; benilla parses them in Rust.
    pub fn queue_guild_request(&mut self, request: GuildRequest) {
        self.model_mut().guild_requests.push(request);
    }
}

/// A roster row by 1-based index.
fn member_at(roster: &[GuildMemberInfo], index: i32) -> Option<&GuildMemberInfo> {
    usize::try_from(index)
        .ok()
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| roster.get(i))
}

/// A string return, nil when empty: the bindings push nil for a missing DBC row.
fn string_or_nil(lua: &Lua, s: &str) -> mlua::Result<Value> {
    Ok(if s.is_empty() {
        Value::Nil
    } else {
        Value::String(lua.create_string(s)?)
    })
}

/// Whether the player's own rank holds right `index`, 1-based into [`RANK_RIGHT_BITS`].
fn has_right(model: &Model, index: usize) -> bool {
    model.guild.in_guild && model.guild.rights & RANK_RIGHT_BITS[index - 1] != 0
}

/// The rank record the control popup has loaded, `None` for none or past the guild's ranks.
fn control_rank(model: &Model) -> Option<&GuildRankInfo> {
    (model.guild_control.rank as usize)
        .checked_sub(1)
        .and_then(|i| model.guild.ranks.get(i))
}

/// The mutable rank record the control popup has loaded.
fn control_rank_mut(model: &mut Model) -> Option<&mut GuildRankInfo> {
    let rank = model.guild_control.rank as usize;
    rank.checked_sub(1)
        .and_then(move |i| model.guild.ranks.get_mut(i))
}

/// A rank name as the save and add verbs send it: `ScrubString(name, buf, 16, 64, 1)`, which
/// strips pipes, cuts at a line break and keeps fifteen characters (`0x7e1830`).
fn scrubbed_rank_name(name: &str) -> String {
    super::calendar::scrub(name, 16, 64, true)
}

/// `TruncateNewLines` (`0x7e1640`): a CR, an LF, or a `\n` or `|n` escape ends the text.
fn truncate_new_lines(text: &str) -> &str {
    let bytes = text.as_bytes();
    let cut = (0..bytes.len()).find(|&i| match bytes[i] {
        b'\r' | b'\n' => true,
        b'\\' | b'|' => bytes.get(i + 1) == Some(&b'n'),
        _ => false,
    });
    cut.map_or(text, |i| &text[..i])
}

/// Register the guild globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ── Membership and identity ──────────────────────────────────────────────────────────────
    // CanGuildBankRepair() (`0x5cc200`): 1 while in a guild whose rank table gives our rank
    // Withdraw Repair; it tests neither the bank's money nor the cost.
    g.set(
        "CanGuildBankRepair",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(binding_abi::flag(has_right(&model, right::WITHDRAW_REPAIR)))
        })?,
    )?;
    // GetGuildBankMoney() (`0x5a4440`) and GetGuildBankWithdrawMoney() (`0x5a44a0`): the two stored
    // values as numbers.
    g.set(
        "GetGuildBankMoney",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.guild.bank_money as f64)
        })?,
    )?;
    g.set(
        "GetGuildBankWithdrawMoney",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(model.guild.bank_withdraw_money))
        })?,
    )?;

    // IsInGuild() reads the player's own `PLAYER_GUILDID`, so it answers before any roster.
    g.set(
        "IsInGuild",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(binding_abi::flag(model.guild.in_guild))
        })?,
    )?;

    // IsGuildLeader() takes no argument: whether the player is the guild master.
    g.set(
        "IsGuildLeader",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(binding_abi::flag(model.guild.is_leader))
        })?,
    )?;

    // GetGuildInfo(unit) → guildName, rankName, rankIndex, per unit because the rank fields are
    // public: it reads the unit snapshot, not the player's guild. An unrecognised token raises; a
    // recognised but unresolvable token, a non-player, a guildless player and an unarrived guild
    // record all answer `nil, nil, 0`, the third the number 0.
    g.set(
        "GetGuildInfo",
        lua.create_function(|lua, unit: Option<String>| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let Some(state) = unit.as_deref().and_then(|u| model.unit(u)) else {
                return Ok((Value::Nil, Value::Nil, Value::Integer(0)));
            };
            let Some(guild) = state.guild.as_ref() else {
                return Ok((Value::Nil, Value::Nil, Value::Integer(0)));
            };
            Ok((
                Value::String(lua.create_string(&guild.name)?),
                Value::String(lua.create_string(&guild.rank_name)?),
                Value::Integer(i64::from(guild.rank_index)),
            ))
        })?,
    )?;

    // ── The roster ───────────────────────────────────────────────────────────────────────────
    // GetNumGuildMembers([all]) (`0x5ca130`): the online count unless show-offline or `all`
    // (`GetBoolOrDefault(1, 0)`) asks for every member.
    g.set(
        "GetNumGuildMembers",
        lua.create_function(|lua, args: MultiValue| {
            let all = binding_abi::bool_or_default(args.front(), false);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(if all {
                model.guild.roster.len()
            } else {
                model.guild.num_members
            } as i64)
        })?,
    )?;

    // GetGuildRosterInfo(index) → name, rank, rankIndex, level, class, zone, note, officernote,
    // online, status, classFileName (`0x5cc9c0`): eleven returns. Out of range answers
    // `nil, nil, 0, 0, nil, nil, nil, nil, nil, "", nil` (`0x5ccc22`).
    g.set(
        "GetGuildRosterInfo",
        lua.create_function(|lua, index: Value| {
            let index = binding_abi::number_arg(lua, index, "Usage: GetGuildRosterInfo(index)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let Some(m) = member_at(&model.guild.roster, index) else {
                let mut out = vec![Value::Nil; 11];
                out[2] = Value::Integer(0);
                out[3] = Value::Integer(0);
                out[9] = Value::String(lua.create_string("")?);
                return Ok(MultiValue::from_vec(out));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&m.name)?),
                Value::String(lua.create_string(&m.rank)?),
                Value::Integer(i64::from(m.rank_index)),
                Value::Integer(i64::from(m.level)),
                string_or_nil(lua, &m.class)?,
                string_or_nil(lua, &m.zone)?,
                Value::String(lua.create_string(&m.note)?),
                Value::String(lua.create_string(&m.officer_note)?),
                binding_abi::flag(m.online),
                Value::String(lua.create_string(&m.status)?),
                string_or_nil(lua, &m.class_file)?,
            ]))
        })?,
    )?;

    // GetGuildRosterLastOnline(index) → years, months, days, hours (`0x5cabb0`): four nils for an
    // online member or an index out of range.
    g.set(
        "GetGuildRosterLastOnline",
        lua.create_function(|lua, index: Value| {
            let index =
                binding_abi::number_arg(lua, index, "Usage: GetGuildRosterLastOnline(index)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let Some(m) = member_at(&model.guild.roster, index).filter(|m| !m.online) else {
                return Ok((Value::Nil, Value::Nil, Value::Nil, Value::Nil));
            };
            let last = m.last_online;
            Ok((
                Value::Integer(i64::from(last.years)),
                Value::Integer(i64::from(last.months)),
                Value::Integer(i64::from(last.days)),
                Value::Integer(i64::from(last.hours)),
            ))
        })?,
    )?;

    // GetGuildRosterMOTD() / GetGuildInfoText(): the two free-text blocks the roster carries.
    g.set(
        "GetGuildRosterMOTD",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            lua.create_string(&model.guild.motd)
        })?,
    )?;
    g.set(
        "GetGuildInfoText",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            lua.create_string(&model.guild.info_text)
        })?,
    )?;

    // GetGuildRosterSelection() (`0x5cafd0`) / SetGuildRosterSelection(index) (`0x5caf40`):
    // 1-based, 0 for none; an index past the roster clears it. The setter writes the snapshot and
    // queues the intent: the same Lua pass reads it back, and the app's state must follow.
    g.set(
        "GetGuildRosterSelection",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(model.guild.selection))
        })?,
    )?;
    g.set(
        "SetGuildRosterSelection",
        lua.create_function(|lua, index: Value| {
            let index =
                binding_abi::number_arg(lua, index, "Usage: SetGuildRosterSelection(index)")?;
            let mut model = crate::script::model_mut_of(lua);
            let len = model.guild.roster.len();
            let index = u32::try_from(index)
                .ok()
                .filter(|i| (1..=len as u32).contains(i))
                .unwrap_or(0);
            model.guild.selection = index;
            model.guild_requests.push(GuildRequest::Select(index));
            Ok(())
        })?,
    )?;

    // GetGuildRosterShowOffline() (`0x5ca1e0`) / SetGuildRosterShowOffline(flag) (`0x5ca1b0`,
    // `GetBoolOrDefault(1, 1)`): written in place and mirrored, as the selection is.
    g.set(
        "GetGuildRosterShowOffline",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(binding_abi::flag(model.guild.show_offline))
        })?,
    )?;
    g.set(
        "SetGuildRosterShowOffline",
        lua.create_function(|lua, args: MultiValue| {
            let on = binding_abi::bool_or_default(args.front(), true);
            let mut model = crate::script::model_mut_of(lua);
            model.guild.show_offline = on;
            model.guild_requests.push(GuildRequest::SetShowOffline(on));
            Ok(())
        })?,
    )?;

    // SortGuildRoster(field) (`0x5cb000`): the app re-orders and fires `GUILD_ROSTER_UPDATE` on
    // the drain, without asking the server.
    g.set(
        "SortGuildRoster",
        lua.create_function(|lua, field: String| {
            let mut model = crate::script::model_mut_of(lua);
            model.guild_requests.push(GuildRequest::Sort(field));
            Ok(())
        })?,
    )?;

    // CloseGuildRoster() (`0x8e5250`): a no-op that sends nothing.
    g.set("CloseGuildRoster", lua.create_function(|_, ()| Ok(()))?)?;

    // The argument-less verbs; the app throttles `GuildRoster()` (`GuildRequest::Roster`).
    for (global, request) in [
        ("GuildRoster", GuildRequest::Roster),
        ("GuildInfo", GuildRequest::Info),
        ("AcceptGuild", GuildRequest::Accept),
        ("DeclineGuild", GuildRequest::Decline),
        ("GuildLeave", GuildRequest::Leave),
        ("GuildDisband", GuildRequest::Disband),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, ()| {
                let mut model = crate::script::model_mut_of(lua);
                model.guild_requests.push(request.clone());
                Ok(())
            })?,
        )?;
    }

    // ── The by-name verbs ────────────────────────────────────────────────────────────────────
    // Five name-taking verbs (`0x522a20`-`0x522d60`): a missing name posts
    // `ERR_COMMAND_NEEDS_TARGET` (`0x522ace`), one over 48 bytes raises, and any other is sent,
    // the empty string included.
    for (global, make) in [
        ("GuildInvite", GuildRequest::Invite as fn(String) -> _),
        ("GuildUninvite", GuildRequest::Uninvite as fn(String) -> _),
        ("GuildPromote", GuildRequest::Promote as fn(String) -> _),
        ("GuildDemote", GuildRequest::Demote as fn(String) -> _),
        ("GuildSetLeader", GuildRequest::SetLeader as fn(String) -> _),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, name: Value| {
                let Some(name) = lua.coerce_string(name)? else {
                    crate::script::model_mut_of(lua)
                        .ui_errors
                        .push("ERR_COMMAND_NEEDS_TARGET");
                    return Ok(());
                };
                if name.as_bytes().len() > 48 {
                    return Err(mlua::Error::RuntimeError("Name too long".into()));
                }
                let name = name.to_string_lossy();
                crate::script::model_mut_of(lua)
                    .guild_requests
                    .push(make(name));
                Ok(())
            })?,
        )?;
    }

    // GuildSetMOTD(text) / SetGuildInfoText(text) (`0x5cb820`): an empty string is sent, and
    // clears the text.
    for (global, make) in [
        ("GuildSetMOTD", GuildRequest::SetMotd as fn(String) -> _),
        (
            "SetGuildInfoText",
            GuildRequest::SetInfoText as fn(String) -> _,
        ),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, text: String| {
                let mut model = crate::script::model_mut_of(lua);
                model.guild_requests.push(make(text));
                Ok(())
            })?,
        )?;
    }

    // GuildRosterSetPublicNote(index, note) (`0x5cad00`) / GuildRosterSetOfficerNote (`0x5cae20`)
    // take a roster row, which the app resolves to the name the wire carries.
    for (global, make) in [
        (
            "GuildRosterSetPublicNote",
            (|index, note| GuildRequest::SetPublicNote { index, note }) as fn(u32, String) -> _,
        ),
        (
            "GuildRosterSetOfficerNote",
            (|index, note| GuildRequest::SetOfficerNote { index, note }) as fn(u32, String) -> _,
        ),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, (index, note): (i64, String)| {
                let Ok(index) = u32::try_from(index) else {
                    return Ok(());
                };
                if index >= 1 {
                    let mut model = crate::script::model_mut_of(lua);
                    model.guild_requests.push(make(index, note));
                }
                Ok(())
            })?,
        )?;
    }

    // ── The permission predicates ────────────────────────────────────────────────────────────
    // Nine predicates, each one bit of the player's own rank's rights (`0x5cbcf0`-`0x5cc170`).
    for (global, index) in [
        ("CanGuildPromote", right::PROMOTE),
        ("CanGuildDemote", right::DEMOTE),
        ("CanGuildInvite", right::INVITE),
        ("CanGuildRemove", right::REMOVE),
        ("CanEditMOTD", right::SET_MOTD),
        ("CanEditPublicNote", right::EDIT_PUBLIC_NOTE),
        ("CanViewOfficerNote", right::VIEW_OFFICER_NOTE),
        ("CanEditOfficerNote", right::EDIT_OFFICER_NOTE),
        ("CanEditGuildInfo", right::MODIFY_GUILD_INFO),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, ()| {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                Ok(binding_abi::flag(has_right(&model, index)))
            })?,
        )?;
    }

    // CanEditGuildEvent() (`0x5cc360`): the rank's calendar right, bit 20; 1 or nil.
    g.set(
        "CanEditGuildEvent",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(binding_abi::flag(
                model.guild.in_guild
                    && model.guild.rights & super::calendar::RIGHT_EDIT_GUILD_EVENT != 0,
            ))
        })?,
    )?;

    // ── The rank-control popup ───────────────────────────────────────────────────────────────
    // GuildControlGetNumRanks() (`0x5ca220`): the guild's rank count.
    g.set(
        "GuildControlGetNumRanks",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.guild.ranks.len() as i64)
        })?,
    )?;

    // GuildControlGetRankName(index) (`0x5ccc90`): 1-based, the dropdown's ID; `""` past the
    // guild's ranks, and cut at a line break (`TruncateNewLines`).
    g.set(
        "GuildControlGetRankName",
        lua.create_function(|lua, index: Value| {
            let index =
                binding_abi::number_arg(lua, index, "Usage: GuildControlGetRankName(index)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let name = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| model.guild.ranks.get(i))
                .map(|r| truncate_new_lines(&r.name))
                .unwrap_or_default();
            lua.create_string(name)
        })?,
    )?;

    // GuildControlSetRank(index) (`0x5ca260` → `0x5ca090`): load a 1-based rank into the staging
    // buffer, each slot seeded from its live rights; past seventeen, or 0, clears the selection.
    g.set(
        "GuildControlSetRank",
        lua.create_function(|lua, index: Value| {
            let index = binding_abi::number_arg(lua, index, "Usage: GuildControlSetRank(rank)")?;
            let mut model = crate::script::model_mut_of(lua);
            let rank = u32::try_from(index)
                .ok()
                .filter(|i| (1..=CONTROL_RANKS).contains(i))
                .unwrap_or(0);
            model.guild_control.rank = rank;
            let rights = control_rank(&model).map(|r| r.rights).unwrap_or(0);
            let mut staged = [false; 17];
            for (slot, bit) in staged.iter_mut().zip(RANK_RIGHT_BITS) {
                *slot = rights & bit != 0;
            }
            model.guild_control.staged = staged;
            Ok(())
        })?,
    )?;

    // GuildControlGetRankFlags() (`0x5cc3e0`): seventeen returns in `RANK_RIGHT_BITS` order, from
    // the loaded rank's live rights, never the staging buffer, all nil outside a guild.
    g.set(
        "GuildControlGetRankFlags",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let rights = control_rank(&model)
                .filter(|_| model.guild.in_guild)
                .map(|r| r.rights)
                .unwrap_or(0);
            Ok(MultiValue::from_vec(
                RANK_RIGHT_BITS
                    .iter()
                    .map(|bit| binding_abi::flag(rights & bit != 0))
                    .collect(),
            ))
        })?,
    )?;

    // GuildControlSetRankFlag(index, enabled) (`0x5ca2b0`): stage one checkbox, 1 to 17, by
    // `GetBoolOrDefault(2, 0)`.
    g.set(
        "GuildControlSetRankFlag",
        lua.create_function(|lua, args: MultiValue| {
            let mut args = args.into_iter();
            let index = binding_abi::number_arg(
                lua,
                args.next().unwrap_or(Value::Nil),
                "Usage: GuildControlSetRankFlag(index, enabled)",
            )?;
            let enabled = binding_abi::bool_or_default(args.next().as_ref(), false);
            let Some(slot) = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .filter(|i| *i < RANK_RIGHT_BITS.len())
            else {
                return Ok(());
            };
            let mut model = crate::script::model_mut_of(lua);
            model.guild_control.staged[slot] = enabled;
            Ok(())
        })?,
    )?;

    // GuildControlSaveRank(name) (`0x5cb170`): an empty name is a silent no-op; otherwise the
    // scrubbed name, the staged flags folded over the live rights (`or mask` / `and ~mask`), the
    // withdraw limit times 10000 and the six bank tabs, as one `CMSG_GUILD_RANK` for the loaded
    // rank, if it is one of the guild's.
    g.set(
        "GuildControlSaveRank",
        lua.create_function(|lua, name: Value| {
            let name = binding_abi::string_arg(lua, name, "Usage: GuildControlSaveRank(name)")?;
            if name.is_empty() {
                return Ok(());
            }
            let mut model = crate::script::model_mut_of(lua);
            let edit = model.guild_control.clone();
            let Some(rank) = control_rank(&model) else {
                return Ok(());
            };
            let mut rights = rank.rights;
            for (staged, bit) in edit.staged.iter().zip(RANK_RIGHT_BITS) {
                if *staged {
                    rights |= bit;
                } else {
                    rights &= !bit;
                }
            }
            let money_per_day = rank.bank_money_per_day.wrapping_mul(10_000);
            let tabs = std::array::from_fn(|i| rank.tab(i));
            model.guild_requests.push(GuildRequest::SaveRank {
                rank_index: edit.rank - 1,
                rights,
                name: scrubbed_rank_name(&name),
                money_per_day,
                tabs,
            });
            Ok(())
        })?,
    )?;

    // GuildControlAddRank(name) (`0x5cb330`): an empty name is a silent no-op, and so is a guild
    // already at `MAX_RANKS`; the name is scrubbed as the save's is.
    g.set(
        "GuildControlAddRank",
        lua.create_function(|lua, name: Value| {
            let name = binding_abi::string_arg(lua, name, "Usage: GuildControlAddRank(name)")?;
            if name.is_empty() {
                return Ok(());
            }
            let mut model = crate::script::model_mut_of(lua);
            if model.guild.ranks.len() < MAX_RANKS {
                model
                    .guild_requests
                    .push(GuildRequest::AddRank(scrubbed_rank_name(&name)));
            }
            Ok(())
        })?,
    )?;

    // GuildControlDelRank() (`0x5cb3f0`) reads no argument and sends an empty `0x233`: the server
    // deletes the last rank. It refuses silently at `MIN_RANKS`, before any packet.
    g.set(
        "GuildControlDelRank",
        lua.create_function(|lua, _ignored: MultiValue| {
            let mut model = crate::script::model_mut_of(lua);
            if model.guild.ranks.len() > MIN_RANKS {
                model.guild_requests.push(GuildRequest::DelRank);
            }
            Ok(())
        })?,
    )?;

    // ── The loaded rank's bank half ──────────────────────────────────────────────────────────
    // Five verbs read and write the control rank's record in place, which the next roster
    // overwrites; the save sends what they left there.

    // GetGuildBankTabPermissions(tab) → view, deposit, updateText, withdrawals (`0x5cb560`):
    // three flags and a number, zeros past the six tabs.
    g.set(
        "GetGuildBankTabPermissions",
        lua.create_function(|lua, tab: Value| {
            let tab = binding_abi::number_arg(lua, tab, "Usage: GetGuildBankTabPermissions(tab)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let (rights, stacks) = usize::try_from(tab - 1)
                .ok()
                .filter(|t| *t < BANK_TABS)
                .and_then(|t| control_rank(&model).map(|r| r.tab(t)))
                .unwrap_or_default();
            Ok((
                binding_abi::flag(rights & BANK_TAB_RIGHT_BITS[0] != 0),
                binding_abi::flag(rights & BANK_TAB_RIGHT_BITS[1] != 0),
                binding_abi::flag(rights & BANK_TAB_RIGHT_BITS[2] != 0),
                i64::from(stacks),
            ))
        })?,
    )?;

    // SetGuildBankTabPermissions(tab, index, enabled) (`0x5cb470`): one of the tab's three rights
    // by `GetBoolOrDefault(3, 0)`.
    g.set(
        "SetGuildBankTabPermissions",
        lua.create_function(|lua, args: MultiValue| {
            const USAGE: &str = "Usage: SetGuildTabPermissions(tab, index, enabled)";
            let mut args = args.into_iter();
            let tab = binding_abi::number_arg(lua, args.next().unwrap_or(Value::Nil), USAGE)?;
            let index = binding_abi::number_arg(lua, args.next().unwrap_or(Value::Nil), USAGE)?;
            let enabled = binding_abi::bool_or_default(args.next().as_ref(), false);
            let (Some(tab), Some(&bit)) = (
                usize::try_from(tab - 1).ok().filter(|t| *t < BANK_TABS),
                usize::try_from(index - 1)
                    .ok()
                    .and_then(|i| BANK_TAB_RIGHT_BITS.get(i)),
            ) else {
                return Ok(());
            };
            let mut model = crate::script::model_mut_of(lua);
            if let Some(rank) = control_rank_mut(&mut model) {
                let (rights, _) = rank.tab_mut(tab);
                if enabled {
                    *rights |= bit;
                } else {
                    *rights &= !bit;
                }
            }
            Ok(())
        })?,
    )?;

    // SetGuildBankWithdrawLimit(amount) (`0x5cb6a0`): stores the gold amount, capped at 10^9, in
    // the copper slot the save multiplies by 10000.
    g.set(
        "SetGuildBankWithdrawLimit",
        lua.create_function(|lua, amount: Value| {
            let amount = binding_abi::number_arg(
                lua,
                amount,
                "Usage: Script_SetGuildBankWithdrawLimit(amount)",
            )?;
            let mut model = crate::script::model_mut_of(lua);
            if let Some(rank) = control_rank_mut(&mut model) {
                rank.bank_money_per_day = (amount as u32).min(1_000_000_000);
            }
            Ok(())
        })?,
    )?;

    // GetGuildBankWithdrawLimit() (`0x5cb700`): the slot over 10000 when it is not negative, so
    // the guild master's -1 stays -1.
    g.set(
        "GetGuildBankWithdrawLimit",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let limit = control_rank(&model)
                .map(|r| r.bank_money_per_day as i32)
                .unwrap_or(0);
            Ok(i64::from(if limit >= 0 { limit / 10_000 } else { limit }))
        })?,
    )?;

    // SetGuildBankTabWithdraw(tab, amount) (`0x5cb760`): a tab's daily withdrawals, capped at
    // 100000.
    g.set(
        "SetGuildBankTabWithdraw",
        lua.create_function(|lua, (tab, amount): (Value, Value)| {
            const USAGE: &str = "Usage: SetGuildBankTabWithdraw(tab, amount)";
            let tab = binding_abi::number_arg(lua, tab, USAGE)?;
            let amount = binding_abi::number_arg(lua, amount, USAGE)?;
            let Some(tab) = usize::try_from(tab - 1).ok().filter(|t| *t < BANK_TABS) else {
                return Ok(());
            };
            let mut model = crate::script::model_mut_of(lua);
            if let Some(rank) = control_rank_mut(&mut model) {
                rank.tab_mut(tab).1 = (amount as u32).min(100_000);
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only an unrecognised token raises; `"player"` with no unit behind it, the paper doll's case
    /// at world entry, answers `nil, nil, 0`.
    #[test]
    fn a_recognised_but_unresolved_unit_answers_nils_and_only_an_unknown_token_raises() {
        let s = crate::script::UiScript::new().unwrap();
        let (name, rank, idx): (Option<String>, Option<String>, i64) = s
            .eval(r#"return GetGuildInfo("player")"#)
            .expect("a recognised token must not raise");
        assert_eq!((name, rank, idx), (None, None, 0));
        assert!(s
            .eval::<bool>(r#"local a,b,c = GetGuildInfo("party3") return c == 0"#)
            .unwrap());
        assert!(s
            .eval::<i64>(r#"return GetGuildInfo("nosuchunit")"#)
            .is_err());
    }

    /// `CanGuildBankRepair` (`0x5cc200`) is our rank's Withdraw Repair while in a guild; the two
    /// money getters (`0x5a4440`, `0x5a44a0`) answer the stored values.
    #[test]
    fn the_guild_bank_repair_verbs_read_the_rank_and_the_stored_money() {
        let mut s = crate::script::UiScript::new().unwrap();
        assert!(s
            .eval::<bool>("return CanGuildBankRepair() == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetGuildBankMoney()").unwrap(), 0);
        assert_eq!(
            s.eval::<i64>("return GetGuildBankWithdrawMoney()").unwrap(),
            0
        );

        s.set_guild(GuildState {
            in_guild: true,
            rights: 0x0004_0003,
            bank_money: 12_345_678,
            bank_withdraw_money: -1,
            ..Default::default()
        });
        assert!(s.eval::<bool>("return CanGuildBankRepair() == 1").unwrap());
        assert_eq!(
            s.eval::<i64>("return GetGuildBankMoney()").unwrap(),
            12_345_678
        );
        assert_eq!(
            s.eval::<i64>("return GetGuildBankWithdrawMoney()").unwrap(),
            -1,
            "the guild master's unlimited"
        );

        s.set_guild(GuildState {
            in_guild: true,
            rights: 0x0000_0003,
            ..Default::default()
        });
        assert!(s
            .eval::<bool>("return CanGuildBankRepair() == nil")
            .unwrap());
        s.set_guild(GuildState {
            rights: 0x0004_0000,
            ..Default::default()
        });
        assert!(
            s.eval::<bool>("return CanGuildBankRepair() == nil")
                .unwrap(),
            "guildless"
        );
    }

    #[test]
    fn the_rank_right_table_is_twelve_thousand_three_hundred_forty_s_seventeen_slots() {
        assert_eq!(RANK_RIGHT_BITS[right::PROMOTE - 1], 0x80);
        assert_eq!(RANK_RIGHT_BITS[right::DEMOTE - 1], 0x100);
        assert_eq!(RANK_RIGHT_BITS[right::INVITE - 1], 0x10);
        assert_eq!(RANK_RIGHT_BITS[right::REMOVE - 1], 0x20);
        assert_eq!(RANK_RIGHT_BITS[right::WITHDRAW_REPAIR - 1], 0x4_0000);
        assert_eq!(
            RANK_RIGHT_BITS[16],
            super::super::calendar::RIGHT_EDIT_GUILD_EVENT,
            "the last slot is the calendar's right"
        );
        let mut seen = 0u32;
        for bit in RANK_RIGHT_BITS {
            assert_eq!(bit.count_ones(), 1, "{bit:#x} is not a single bit");
            assert_eq!(seen & bit, 0, "duplicate bit {bit:#x}");
            seen |= bit;
        }
    }

    fn guild_with_ranks(n: usize) -> crate::script::UiScript {
        let mut s = crate::script::UiScript::new().unwrap();
        s.set_guild(GuildState {
            in_guild: true,
            ranks: (0..n)
                .map(|i| GuildRankInfo {
                    name: format!("Rank{i}"),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        });
        s
    }

    fn saves(s: &mut crate::script::UiScript) -> Vec<GuildRequest> {
        s.take_guild_requests()
            .into_iter()
            .filter(|r| matches!(r, GuildRequest::SaveRank { .. }))
            .collect()
    }

    /// `0x5cb170` sends any non-empty name, scrubbed to fifteen characters with its pipes gone
    /// (`ScrubString`, `0x7e1830`), and refuses no rank loaded or one past the count.
    #[test]
    fn saving_a_rank_scrubs_the_name_and_refuses_what_the_reference_refuses() {
        let mut s = guild_with_ranks(5);
        let name = |r: &GuildRequest| match r {
            GuildRequest::SaveRank { name, .. } => name.clone(),
            _ => unreachable!(),
        };

        s.run(r#"GuildControlSetRank(2); GuildControlSaveRank("")"#)
            .unwrap();
        assert!(saves(&mut s).is_empty(), "an empty name is a silent no-op");

        s.run(r#"GuildControlSetRank(2); GuildControlSaveRank(string.rep("a", 20))"#)
            .unwrap();
        assert_eq!(
            saves(&mut s).iter().map(name).collect::<Vec<_>>(),
            vec!["a".repeat(15)],
            "the sixteenth character is where the scrub cuts"
        );
        s.run(r#"GuildControlSetRank(2); GuildControlSaveRank("Of|ficer\nnext")"#)
            .unwrap();
        assert_eq!(
            saves(&mut s).iter().map(name).collect::<Vec<_>>(),
            vec!["Officer".to_string()]
        );

        let mut fresh = guild_with_ranks(5);
        fresh.run(r#"GuildControlSaveRank("Officer")"#).unwrap();
        assert!(saves(&mut fresh).is_empty(), "nothing selected");
        s.run(r#"GuildControlSetRank(6); GuildControlSaveRank("Officer")"#)
            .unwrap();
        assert!(saves(&mut s).is_empty(), "rank index 5 >= 5 ranks");
    }

    /// The save folds the seventeen staged flags over the live rights and carries the rank's bank
    /// half as the popup's verbs left it: the gold limit times 10000 and six tab pairs.
    #[test]
    fn saving_a_rank_carries_the_staged_flags_and_the_bank_half() {
        let mut s = crate::script::UiScript::new().unwrap();
        s.set_guild(GuildState {
            in_guild: true,
            ranks: vec![
                GuildRankInfo {
                    name: "Master".into(),
                    rights: 0x001F_F1FF,
                    ..Default::default()
                },
                GuildRankInfo {
                    name: "Officer".into(),
                    // Listen, Speak, Set MOTD, and a bit outside the seventeen.
                    rights: 0x0000_1003 | 0x0000_0040,
                    bank_money_per_day: 50_000,
                    bank_tabs: vec![(0x1, 5); BANK_TABS],
                },
            ],
            ..Default::default()
        });
        s.run(
            r#"
            GuildControlSetRank(2)
            GuildControlSetRankFlag(9, nil)    -- Set MOTD off
            GuildControlSetRankFlag(16, 1)     -- Withdraw Gold on
            SetGuildBankWithdrawLimit(25)
            SetGuildBankTabPermissions(2, 2, 1)
            SetGuildBankTabWithdraw(3, 250000)
            GuildControlSaveRank("Officer")
        "#,
        )
        .unwrap();
        let mut tabs = [(0x1, 5); BANK_TABS];
        tabs[1].0 = 0x1 | 0x2;
        tabs[2].1 = 100_000;
        assert_eq!(
            saves(&mut s),
            vec![GuildRequest::SaveRank {
                rank_index: 1,
                rights: 0x0000_0003 | 0x0000_0040 | 0x0008_0000,
                name: "Officer".into(),
                money_per_day: 250_000,
                tabs,
            }]
        );
    }

    /// `0x5cb700` divides the stored slot by 10000 unless it is negative; `0x5cb560` answers three
    /// flags and the withdrawals, zeros past the tabs.
    #[test]
    fn the_bank_limit_getters_read_the_loaded_rank() {
        let mut s = crate::script::UiScript::new().unwrap();
        s.set_guild(GuildState {
            in_guild: true,
            ranks: vec![
                GuildRankInfo {
                    bank_money_per_day: u32::MAX,
                    ..Default::default()
                },
                GuildRankInfo {
                    bank_money_per_day: 1_234_567,
                    bank_tabs: vec![(0x5, 7)],
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        s.run("GuildControlSetRank(1)").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetGuildBankWithdrawLimit()").unwrap(),
            -1
        );
        s.run("GuildControlSetRank(2)").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetGuildBankWithdrawLimit()").unwrap(),
            123
        );
        assert!(s
            .eval::<bool>(
                "local v, d, u, n = GetGuildBankTabPermissions(1) \
                 return v == 1 and d == nil and u == 1 and n == 7"
            )
            .unwrap());
        assert!(s
            .eval::<bool>(
                "local v, d, u, n = GetGuildBankTabPermissions(7) \
                 return v == nil and d == nil and u == nil and n == 0"
            )
            .unwrap());
    }

    /// `0x5cc3e0` answers seventeen values, all nil before any rank is loaded.
    #[test]
    fn the_rank_flags_are_seventeen_values() {
        let s = guild_with_ranks(5);
        assert_eq!(
            s.eval::<i64>("return select('#', GuildControlGetRankFlags())")
                .unwrap(),
            17
        );
        assert!(s
            .eval::<bool>("return GuildControlGetRankFlags() == nil")
            .unwrap());
    }

    /// `0x5ccc90` answers `""` past the ranks and cuts a name at its line break.
    #[test]
    fn a_rank_name_past_the_count_is_empty() {
        let mut s = guild_with_ranks(5);
        s.set_guild(GuildState {
            in_guild: true,
            ranks: vec![GuildRankInfo {
                name: "Master\nof all".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        assert_eq!(
            s.eval::<String>("return GuildControlGetRankName(1)")
                .unwrap(),
            "Master"
        );
        assert_eq!(
            s.eval::<String>("return GuildControlGetRankName(2)")
                .unwrap(),
            ""
        );
    }

    /// `0x5cc9c0`: eleven returns, the eleventh the class file; out of range the fixed shape;
    /// `0x5cabb0`: nothing for an online member.
    #[test]
    fn the_roster_row_is_eleven_values_and_an_online_member_has_no_last_online() {
        let mut s = crate::script::UiScript::new().unwrap();
        s.set_guild(GuildState {
            in_guild: true,
            roster: vec![
                GuildMemberInfo {
                    name: "Tigole".into(),
                    rank: "Guild Master".into(),
                    level: 80,
                    class: "Warrior".into(),
                    class_file: "WARRIOR".into(),
                    online: true,
                    ..Default::default()
                },
                GuildMemberInfo {
                    name: "Kaplan".into(),
                    last_online: LastOnline {
                        days: 3,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ],
            num_members: 1,
            ..Default::default()
        });
        assert_eq!(
            s.eval::<i64>("return select('#', GetGuildRosterInfo(1))")
                .unwrap(),
            11
        );
        assert_eq!(
            s.eval::<String>("return select(11, GetGuildRosterInfo(1))")
                .unwrap(),
            "WARRIOR"
        );
        assert!(s
            .eval::<bool>(
                "local n, r, ri, l, c, z, no, o, on, st, cf = GetGuildRosterInfo(9) \
                 return n == nil and r == nil and ri == 0 and l == 0 and c == nil and z == nil \
                 and no == nil and o == nil and on == nil and st == '' and cf == nil"
            )
            .unwrap());
        assert!(s
            .eval::<bool>(
                "local n, r, ri, l, c, z = GetGuildRosterInfo(2) return c == nil and z == nil"
            )
            .unwrap());
        assert!(s
            .eval::<bool>("return GetGuildRosterLastOnline(1) == nil")
            .unwrap());
        assert_eq!(
            s.eval::<i64>("return select(3, GetGuildRosterLastOnline(2))")
                .unwrap(),
            3
        );
        assert_eq!(s.eval::<i64>("return GetNumGuildMembers()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return GetNumGuildMembers(1)").unwrap(), 2);
        s.run("SetGuildRosterSelection(3)").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetGuildRosterSelection()").unwrap(),
            0,
            "an index past the roster clears the selection"
        );
    }

    /// `0x5cb330` scrubs the name as the save does and sends only below ten ranks.
    #[test]
    fn adding_a_rank_scrubs_the_name() {
        let mut s = guild_with_ranks(5);
        s.run(r#"GuildControlAddRank(""); GuildControlAddRank(string.rep("a", 17))"#)
            .unwrap();
        assert_eq!(
            s.take_guild_requests(),
            vec![GuildRequest::AddRank("a".repeat(15))]
        );
        let mut full = guild_with_ranks(MAX_RANKS);
        full.run(r#"GuildControlAddRank("Peon")"#).unwrap();
        assert!(full.take_guild_requests().is_empty());
    }

    /// `GuildInvite` and its four siblings (`0x522a20`-`0x522d60`): no name posts the needs-target
    /// error, a name over 48 bytes raises, and an empty one is sent.
    #[test]
    fn the_by_name_verbs_send_any_name_up_to_48_bytes() {
        let mut s = crate::script::UiScript::new().unwrap();
        s.run(r#"GuildInvite("Thrall"); GuildSetLeader(""); GuildUninvite()"#)
            .unwrap();
        assert_eq!(
            s.take_guild_requests(),
            vec![
                GuildRequest::Invite("Thrall".into()),
                GuildRequest::SetLeader(String::new()),
            ]
        );
        assert_eq!(s.take_ui_errors(), vec!["ERR_COMMAND_NEEDS_TARGET"]);
        assert!(s.run(r#"GuildPromote(string.rep("a", 49))"#).is_err());
        s.run(r#"GuildDemote(string.rep("a", 48))"#).unwrap();
        assert_eq!(s.take_guild_requests().len(), 1);
        assert!(
            s.eval::<bool>("return GuildInviteByName == nil").unwrap(),
            "the 1.12 names are gone"
        );
    }

    /// `GuildStatus_Update` passes 0 before it tests the selection.
    #[test]
    fn member_lookup_is_one_based_and_tolerates_zero() {
        let roster = vec![
            GuildMemberInfo {
                name: "Alice".into(),
                ..Default::default()
            },
            GuildMemberInfo {
                name: "Bob".into(),
                ..Default::default()
            },
        ];
        assert_eq!(
            member_at(&roster, 1).map(|m| m.name.as_str()),
            Some("Alice")
        );
        assert_eq!(member_at(&roster, 2).map(|m| m.name.as_str()), Some("Bob"));
        assert!(member_at(&roster, 0).is_none(), "0 = nothing selected");
        assert!(member_at(&roster, 3).is_none(), "past the end");
        assert!(member_at(&roster, -1).is_none(), "negative");
    }
}
