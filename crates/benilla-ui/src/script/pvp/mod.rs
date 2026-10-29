//! `TogglePVP`, `SetPVP` and the honor bindings: the PvP window's honor pane, the inspect PvP
//! tab, and the rank any unit frame can read off another player.
//!
//! The local player's counters are private descriptor fields the app pushes as a [`HonorState`];
//! another player's arrive as a `MSG_INSPECT_HONOR_STATS` reply ([`InspectHonorData`]), whose
//! presence is `HasInspectHonorData`. With nothing held every getter answers zeros at full width,
//! as the reference reads zero-initialised slots ungated. A foreign player's current rank is
//! public and rides [`UnitState::pvp_rank`](super::UnitState::pvp_rank).
//!
//! The internal rank (0 none, 1..=4 dishonorable, 5..=18 Scout or Private to High Warlord or
//! Grand Marshal, 19 the racial "Leader") keys the `PVP_RANK_<rank>_<team>` GlobalStrings; the
//! visual rank indexes the badge texture, and every conversion goes through [`visual_rank`].
//!
//! Rank titles are read off the VM's globals, which the install's `GlobalStrings.lua` fills: the
//! key is `"PVP_RANK_%d_%d"` with team 0 Horde and 1 Alliance. There is no `PVP_RANK_0_*`, and the
//! panes fall back to `NONE` on its nil.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{number_arg, string_arg};

/// The local player's honor snapshot, decoded by the app from the private 3.3.5a honor fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HonorState {
    /// `PLAYER_FIELD_KILLS` low half: today's honorable kills (`GetPVPSessionStats`, `0x611a20`).
    pub today_hk: u16,
    /// `PLAYER_FIELD_TODAY_CONTRIBUTION`: today's honor.
    pub today_honor: u32,
    /// `PLAYER_FIELD_KILLS` high half: yesterday's honorable kills (`0x611ad0`).
    pub yesterday_hk: u16,
    /// `PLAYER_FIELD_YESTERDAY_CONTRIBUTION`.
    pub yesterday_honor: u32,
    /// `PLAYER_FIELD_LIFETIME_HONORABLE_KILLS` (`0x611b80`).
    pub lifetime_hk: u32,
    /// `PLAYER_FIELD_BYTES` byte 3: the highest lifetime rank, internal scale. Carried raw: only
    /// `GetPVPLifetimeStats` hides a value below 5.
    pub highest_rank: u8,
    /// The current rank, internal scale, 0 for none; the same public byte as
    /// [`UnitState::pvp_rank`](super::UnitState).
    pub rank: u8,
    /// `PLAYER_FIELD_HONOR_CURRENCY` and `_ARENA_CURRENCY`, the point balances.
    pub honor_currency: u32,
    pub arena_currency: u32,
}

/// The honor-point cap `GetHonorCurrency` answers second (`0x60fc40`).
const HONOR_CURRENCY_MAX: f64 = 75_000.0;
/// The arena-point cap `GetArenaCurrency` answers second (`0x60fcc0`).
const ARENA_CURRENCY_MAX: f64 = 10_000.0;

/// One `MSG_INSPECT_HONOR_STATS` reply as the app decoded it (`0x5e7a50`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InspectHonorData {
    /// The player the reply was for, the app's key; never returned to Lua.
    pub guid: u64,
    pub today_hk: u16,
    pub today_honor: u32,
    pub yesterday_hk: u16,
    pub yesterday_honor: u32,
    pub lifetime_hk: u32,
    /// The reply's highest lifetime rank, `GetInspectHonorData`'s sixth return, unfiltered.
    pub highest_rank: u8,
}

/// One of the inspected player's arena teams, `GetInspectArenaTeamData`'s eighteen (`0x5e8030`):
/// the reply's six words and the team cache's name, size and banner.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InspectArenaTeamView {
    pub name: String,
    pub team_size: u32,
    pub rating: u32,
    /// The reply's third to sixth words, pushed in their order.
    pub games: u32,
    pub wins: u32,
    pub played: u32,
    pub personal_rating: u32,
    /// The banner: background colour, emblem style and colour, border style and colour, each
    /// colour three 0..1 channels.
    pub background: [f64; 3],
    pub emblem: u32,
    pub emblem_color: [f64; 3],
    pub border: u32,
    pub border_color: [f64; 3],
}

impl super::UiScript {
    /// Drain the queued `TogglePVP` calls, one `CMSG_TOGGLE_PVP` each; the packet is empty, so
    /// two toggles in a frame are two sends.
    pub fn take_pvp_toggles(&mut self) -> u32 {
        drain_field!(self, pvp_toggles)
    }

    /// Push `GetPVPToggleTimer` (`0x6de7a0`): 300000 without the PvP timer flag, else the ms left
    /// on the flag watch's five-minute stamp, or -1 once it passed.
    pub fn set_pvp_toggle_timer(&mut self, ms: i32) {
        set_field!(self, pvp_toggle_timer, ms);
    }

    /// Drain the queued `SetPVP` calls, the one-byte `CMSG_TOGGLE_PVP` each sends.
    pub fn take_pvp_sets(&mut self) -> Vec<u8> {
        drain_field!(self, pvp_sets)
    }

    /// Push the inspected player's arena teams by slot; the app fires `INSPECT_HONOR_UPDATE`.
    pub fn set_inspect_arena_teams(&mut self, teams: [Option<InspectArenaTeamView>; 3]) {
        set_field!(self, inspect_arena_teams, teams);
    }

    /// Queue a toggle for `/pvp`, which benilla parses in Rust; the reference's `/pvp` is Lua over
    /// `TogglePVP`.
    pub fn queue_pvp_toggle(&mut self) {
        self.model_mut().pvp_toggles += 1;
    }

    /// Push or clear the local player's honor snapshot, the self getters' only source. It fires
    /// no event: the app fires `PLAYER_PVP_KILLS_CHANGED`/`PLAYER_PVP_RANK_CHANGED` alongside.
    pub fn set_honor(&mut self, honor: Option<HonorState>) {
        set_field!(self, honor, honor);
    }

    /// Push or clear the last `MSG_INSPECT_HONOR_STATS` reply, paired with the pane's
    /// `INSPECT_HONOR_UPDATE`. Its presence is `HasInspectHonorData`, so the app clears it on
    /// `ClearInspectPlayer` and the pane's `OnShow` asks again for the next player. Either way the
    /// in-flight latch clears, as the engine's reply and re-key do.
    pub fn set_inspect_honor(&mut self, data: Option<InspectHonorData>) {
        let mut model = self.model_mut();
        model.inspect_honor = data;
        model.inspect_honor_pending = false;
    }

    /// Drain the queued `RequestInspectHonorData` calls, each a `MSG_INSPECT_HONOR_STATS` and a
    /// `MSG_INSPECT_ARENA_TEAMS` send for the app's inspect target (`0x5e9140`). At most 1: the
    /// `pending` latch refuses another until [`Self::set_inspect_honor`] resolves the first.
    pub fn take_inspect_honor_requests(&mut self) -> u32 {
        drain_field!(self, inspect_honor_requests)
    }

    /// A rank's localized title for the app's `SMSG_PVP_CREDIT` chat line, `None` if missing or
    /// empty. Gendered, unlike the pane's: the credit formatter resolves. No range check, so rank
    /// 19 names "Leader"; `rank` is the internal rank the credit carries (`HonorMgr::SendPVPCredit`
    /// sends `GetRank().rank`).
    ///
    /// The caller owns two facts: the gender is the local player's while the team is the victim's,
    /// and a victim whose team is -1 gets no line at all, hence `team: u8`.
    pub fn pvp_rank_title(&self, rank: u8, team: u8, female: bool) -> Option<String> {
        rank_title_gendered(
            self.lua(),
            RankTitle {
                rank,
                // Past `i8` names no key, the lookup's own miss.
                team: i8::try_from(team).ok()?,
                female,
            },
        )
    }
}

/// Internal rank to visual rank: 5..=18 subtract 4, and 1..=4 subtract 5 to give -4..=-1. The
/// server's `visualRank` negates instead (`HonorMgr.cpp:991`), so the two disagree on ranks 1..=4;
/// this is the client's. Only 1..=18 reach it, as `GetPVPRankInfo` gates first.
fn visual_rank(rank: i64) -> i64 {
    if rank >= 5 {
        rank - 4
    } else {
        rank - 5
    }
}

fn honor(lua: &Lua) -> Option<HonorState> {
    lua.app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data")
        .honor
}

fn read(lua: &Lua) -> mlua::AppDataRef<'_, crate::script::ModelCell> {
    lua.app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data")
}

fn inspect_honor(lua: &Lua) -> Option<InspectHonorData> {
    lua.app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data")
        .inspect_honor
}

/// A GlobalString off the VM's globals, `None` when absent or empty, as `GetPVPRankInfo` tests
/// both.
fn global_string(lua: &Lua, key: &str) -> Option<String> {
    lua.globals()
        .get::<Option<String>>(key)
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
}

/// A unit's PvP team digit, the reference's tri-state from its race (`ChrRaces` to
/// `FactionTemplate`, group mask `& 4` then `& 2`): 0 Horde, 1 Alliance, -1 no side; resolved by
/// the app as [`UnitState::pvp_team`](super::UnitState). Not `faction_group`, the live faction
/// template: a vmangos GM's template 35 has no side while his race keeps one. -1 misses every key.
fn team_of(u: &super::UnitState) -> i64 {
    i64::from(u.pvp_team)
}

/// `GetPVPRankInfo`'s second argument as a team digit, the engine's three-way dispatch: a number is
/// the digit itself, truncated; a string is that unit's [`team_of`], or 0 if it names nothing or a
/// non-player; anything else is the local player's, or 0 with no player. That 0 is the team
/// register's initial value, so a miss names off the Horde list, as in the reference. An unknown
/// token raises, as the reference does.
fn team_arg(lua: &Lua, v: Value) -> mlua::Result<i64> {
    // `lua_isnumber` before `lua_isstring`, which is what puts a numeric string on the number arm.
    if let Some(n) = lua.coerce_number(v.clone())? {
        return Ok(i64::from(n as i64 as i32));
    }
    let Some(token) = lua.coerce_string(v)? else {
        // Absent, boolean or table: the local player.
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model app_data");
        return Ok(model.unit("player").map_or(0, team_of));
    };
    let token = Some(token.to_str()?.to_owned());
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    Ok(token
        .as_ref()
        .and_then(|t| model.unit(t))
        // The reference's player gate (`shr edx,4; test dl,1`), applied here unlike in
        // `UnitPVPRank`: on this arm nothing else tells a creature, which has a race too, from a
        // player.
        .filter(|u| u.is_player)
        .map_or(0, team_of))
}

/// `PVP_RANK_<internal>_<team>`, ungendered: `GetPVPRankInfo`'s lookup, `(key, -1, 0)`, so the
/// panes show a female character the default title. Do not gender it: that would diverge from the
/// reference.
fn rank_title_ungendered(lua: &Lua, rank: i64, team: i64) -> Option<String> {
    global_string(lua, &format!("PVP_RANK_{rank}_{team}"))
}

/// The `PVP_RANK_<rank>_<team>[_FEMALE]` inputs, kept together because the live lookup and the
/// world-text pass's builder key on the same three.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RankTitle {
    /// `PLAYER_BYTES_3` byte 3 (`UnitPVPRank`), the internal rank 0..=18; 19 is the racial
    /// "Leader" the keys still name.
    pub rank: u8,
    /// The team digit from the race: 0 Horde, 1 Alliance, -1 no side.
    pub team: i8,
    /// Female takes the `_FEMALE` twin and falls back to the base key.
    pub female: bool,
}

/// The same title gendered: it passes 2 (male) or 3 (female), which asks FrameXML's `GetText` to
/// append `_FEMALE` for 3 (the engine never builds that key), and retries ungendered on a miss. The
/// credit line's lookup.
fn rank_title_gendered(lua: &Lua, title: RankTitle) -> Option<String> {
    rank_title(&|key| global_string(lua, key), title)
}

/// [`rank_title_gendered`]'s key construction over a string lookup, so a caller with no VM (the
/// world-text pass) resolves the same `_FEMALE` twin and fallback.
fn rank_title(lookup: &impl Fn(&str) -> Option<String>, title: RankTitle) -> Option<String> {
    title
        .female
        .then(|| lookup(&format!("PVP_RANK_{}_{}_FEMALE", title.rank, title.team)))
        .flatten()
        .or_else(|| lookup(&format!("PVP_RANK_{}_{}", title.rank, title.team)))
}

/// Fill a two-`%s` C format string, the install's template (enUS `UNIT_PVP_NAME` is `"%s %s"`);
/// `%%` is a literal `%` and any other specifier passes through. Deviation: a third `%s` gets an
/// empty string, because the reference pushes two varargs and would read uninitialised stack.
pub(super) fn format_two_strings(fmt: &str, a: &str, b: &str) -> String {
    let mut out = String::with_capacity(fmt.len() + a.len() + b.len());
    let mut args = [a, b].into_iter();
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push_str(args.next().unwrap_or("")),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// The install keys the overhead name's title slot can resolve beside `CharTitles.dbc`: the
/// `PVP_MEDAL<n>` line under a chosen title, for every value of `PLAYER_BYTES_3` byte 2
/// (`0x72a35d`). The world-text pass, which runs without the VM, reads them off each VM once.
pub fn pvp_name_global_keys() -> Vec<String> {
    (1..=u8::MAX).map(|n| format!("PVP_MEDAL{n}")).collect()
}

/// `GetUnitChosenTitle` (`0x72a290`) with titles on, the name `UnitPVPName` and the unit tooltip's
/// first line (`0x6219a0`) show: a player's chosen title filled with `name`, else `name`.
pub(super) fn pvp_name(lua: &Lua, u: &super::UnitState, name: &str) -> String {
    if !u.is_player {
        return name.to_owned();
    }
    let model = read(lua);
    super::title::chosen_title_name(
        &model.title_rows,
        u.chosen_title,
        u.sex == 3,
        u.pvp_medal,
        name,
        |key| global_string(lua, key),
    )
    .unwrap_or_else(|| name.to_owned())
}

/// Register the PvP + honor globals.
/// `PLAYER_FLAGS_PVP_TIMER`: the server's five-minute flag after PvP is turned off.
pub const PLAYER_FLAGS_PVP_TIMER: u32 = 0x0004_0000;

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // TogglePVP() (`0x516840`): the empty `CMSG_TOGGLE_PVP`; `SlashCmdList["PVP"]` calls it.
    g.set(
        "TogglePVP",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.pvp_toggles += 1;
            Ok(())
        })?,
    )?;

    // GetPVPTimer() (`0x51bd00`): the toggle timer plus 1000 ms.
    g.set(
        "GetPVPTimer",
        lua.create_function(|lua, ()| {
            let model = read(lua);
            Ok(f64::from(model.pvp_toggle_timer.wrapping_add(1000)))
        })?,
    )?;

    // IsPVPTimerRunning() (`0x51bd60`): `PLAYER_FLAGS_PVP_TIMER` (`0x40000`), 1 or nil.
    g.set(
        "IsPVPTimerRunning",
        lua.create_function(|lua, ()| {
            let model = read(lua);
            let flags = model.unit("player").map_or(0, |u| u.player_flags);
            Ok(super::binding_abi::flag(
                flags & PLAYER_FLAGS_PVP_TIMER != 0,
            ))
        })?,
    )?;

    // SetPVP(state) (`0x5168b0`): the one-byte form, the number truncated to a byte and 0 for
    // anything else; the unit menu's Enable/Disable PvP pass 1 and nil.
    g.set(
        "SetPVP",
        lua.create_function(|lua, state: Option<Value>| {
            let byte = match state.and_then(|v| lua.coerce_number(v).ok().flatten()) {
                Some(n) => super::binding_abi::ftol(n) as u8,
                None => 0,
            };
            crate::script::model_mut_of(lua).pvp_sets.push(byte);
            Ok(())
        })?,
    )?;

    // ── The self getters ─────────────────────────────────────────────────────────────────────
    // Two values each and zeros with no player (`0x611a20`, `0x611ad0`, `0x611b80`).

    g.set(
        "GetPVPSessionStats",
        lua.create_function(|lua, ()| {
            let h = honor(lua).unwrap_or_default();
            Ok((i64::from(h.today_hk), i64::from(h.today_honor)))
        })?,
    )?;

    g.set(
        "GetPVPYesterdayStats",
        lua.create_function(|lua, ()| {
            let h = honor(lua).unwrap_or_default();
            Ok((i64::from(h.yesterday_hk), i64::from(h.yesterday_honor)))
        })?,
    )?;

    // GetPVPLifetimeStats() → hk, highestRank; a rank below 5 answers 0 (`0x611bd3`).
    g.set(
        "GetPVPLifetimeStats",
        lua.create_function(|lua, ()| {
            let h = honor(lua).unwrap_or_default();
            let highest = if h.highest_rank >= 5 {
                h.highest_rank
            } else {
                0
            };
            Ok((i64::from(h.lifetime_hk), i64::from(highest)))
        })?,
    )?;

    // GetPVPRankProgress() (`0x608560`): a constant 0 in 3.3.5a.
    g.set("GetPVPRankProgress", lua.create_function(|_, ()| Ok(0.0))?)?;

    // GetHonorCurrency() and GetArenaCurrency() → the balance and the cap, 0 with no player.
    g.set(
        "GetHonorCurrency",
        lua.create_function(|lua, ()| {
            let h = honor(lua).unwrap_or_default();
            Ok((f64::from(h.honor_currency), HONOR_CURRENCY_MAX))
        })?,
    )?;
    g.set(
        "GetArenaCurrency",
        lua.create_function(|lua, ()| {
            let h = honor(lua).unwrap_or_default();
            Ok((f64::from(h.arena_currency), ARENA_CURRENCY_MAX))
        })?,
    )?;

    // GetPVPRankInfo(rank [, unit]) → rankName, rankNumber: the title and the visual rank for an
    // internal rank, always two values; every failure answers `nil, 0`, which both panes turn into
    // `NONE`. The reference accepts [1, 18], signed, so rank 19, "Leader", is refused although its
    // GlobalStrings exist and the server sends it. The second argument is `team_arg`'s three-way
    // dispatch.
    g.set(
        "GetPVPRankInfo",
        lua.create_function(|lua, (rank, unit): (Value, Value)| {
            let rank = i64::from(number_arg(
                lua,
                rank,
                "Usage: GetPVPRankInfo(rank [, unit])",
            )?);
            let team = team_arg(lua, unit)?;
            // The range gate first: outside [1, 18] no key is even built.
            let name = if (1..=18).contains(&rank) {
                rank_title_ungendered(lua, rank, team)
            } else {
                None
            };
            let Some(name) = name else {
                return Ok(MultiValue::from_vec(vec![Value::Nil, Value::Integer(0)]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&name)?),
                Value::Integer(visual_rank(rank)),
            ]))
        })?,
    )?;

    // UnitPVPRank(unit) → the unit's current rank, internal, 0 for none. The reference reads the
    // named object's own byte (`[+0xe68]+0x1f`, in public `PLAYER_BYTES_3`), which is how the
    // inspect pane reads a foreign rank. Anything unresolved reads 0, never nil; a non-string
    // argument raises.
    //
    // The engine's player gate is not re-applied: it tests the type mask, while our `is_player`
    // comes from the app's later guid-keyed enrichment, so a snapshot pushed before it would zero a
    // real player's rank. A creature has no player block, so its byte is 0.
    g.set(
        "UnitPVPRank",
        lua.create_function(|lua, token: Value| {
            let token = Some(string_arg(lua, token, "Usage: UnitPVPRank(\"unit\")")?);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(token
                .as_ref()
                .and_then(|t| model.unit(t))
                .map_or(0i64, |u| i64::from(u.pvp_rank)))
        })?,
    )?;

    // UnitPVPName(unit) (`0x60e9a0`): `GetUnitChosenTitle` with titles on (`0x72a290`), a
    // player's chosen title around the name and a medal line, else the plain name. No snapshot or
    // an unresolved name answers nil.
    g.set(
        "UnitPVPName",
        lua.create_function(|lua, token: Value| {
            let token = Some(string_arg(lua, token, "Usage: UnitPVPName(\"unit\")")?);
            let unit = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                token.as_ref().and_then(|t| model.unit(t)).cloned()
            };
            let Some(u) = unit else {
                return Ok(Value::Nil);
            };
            let Some(name) = u.name.clone() else {
                return Ok(Value::Nil);
            };
            let decorated = pvp_name(lua, &u, &name);
            Ok(Value::String(lua.create_string(&decorated)?))
        })?,
    )?;

    // ── The inspect half ─────────────────────────────────────────────────────────────────────

    // RequestInspectHonorData(): `InspectHonorFrame_OnShow` calls it while `HasInspectHonorData()`
    // is false; the app sends `MSG_INSPECT_HONOR_STATS` for its inspect target. Zero return values.
    //
    // The reference bails silently if data is held, if a query is in flight (`pending`) or if the
    // target guid is 0. The first two are here; the guid is the app's, which drops a request with
    // no target. Our `pending` latches on queue, not send, so a dropped request holds it until the
    // next `set_inspect_honor`, which `ClearInspectPlayer` calls as the reference clears the
    // engine's.
    g.set(
        "RequestInspectHonorData",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            if model.inspect_honor.is_some() || model.inspect_honor_pending {
                return Ok(());
            }
            model.inspect_honor_requests += 1;
            model.inspect_honor_pending = true;
            Ok(())
        })?,
    )?;

    // HasInspectHonorData() → 1 or nil: whether a reply is held.
    g.set(
        "HasInspectHonorData",
        lua.create_function(|lua, ()| {
            Ok(if inspect_honor(lua).is_some() {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // GetInspectHonorData() (`0x5e77c0`) → todayHK, todayHonor, yesterdayHK, yesterdayHonor,
    // lifetimeHK, lifetimeHighestRank; six on every path, zeros with nothing held.
    g.set(
        "GetInspectHonorData",
        lua.create_function(|lua, ()| {
            let d = inspect_honor(lua).unwrap_or_default();
            Ok(MultiValue::from_vec(vec![
                Value::Number(f64::from(d.today_hk)),
                Value::Number(f64::from(d.today_honor)),
                Value::Number(f64::from(d.yesterday_hk)),
                Value::Number(f64::from(d.yesterday_honor)),
                Value::Number(f64::from(d.lifetime_hk)),
                Value::Number(f64::from(d.highest_rank)),
            ]))
        })?,
    )?;

    // GetInspectArenaTeamData(index) (`0x5e8030`): eighteen values, nothing for an empty slot or
    // a team the cache has not answered.
    g.set(
        "GetInspectArenaTeamData",
        lua.create_function(|lua, index: Value| {
            let index = super::binding_abi::number_arg(
                lua,
                index,
                "Usage: GetInspectArenaTeamData(index)",
            )?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let team = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| model.inspect_arena_teams.get(i))
                .and_then(Option::as_ref);
            let Some(t) = team else {
                return Ok(MultiValue::new());
            };
            let n = |v: u32| Value::Number(f64::from(v as i32));
            let mut out = vec![
                Value::String(lua.create_string(&t.name)?),
                n(t.team_size),
                n(t.rating),
                n(t.games),
                n(t.wins),
                n(t.played),
                n(t.personal_rating),
            ];
            out.extend(t.background.iter().map(|&c| Value::Number(c)));
            out.push(n(t.emblem));
            out.extend(t.emblem_color.iter().map(|&c| Value::Number(c)));
            out.push(n(t.border));
            out.extend(t.border_color.iter().map(|&c| Value::Number(c)));
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests;
