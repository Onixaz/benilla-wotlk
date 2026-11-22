//! The aura bindings, over the unit's aura slots as the app's live view reads them at the call
//! (`0x6147c0`): a held unit's display order through the display filter, or a roster record's
//! block for a unit the object manager does not hold. A token resolves to its guid as 12340's
//! resolver `0x60abf0` does, so `"mouseover"`, `"party1target"` and `"PLAYER"` read the auras of
//! the unit they name.
//!
//! `UnitAura`, `UnitBuff`, `UnitDebuff`, `CancelUnitBuff` and `GameTooltip:SetUnitAura` and its two
//! siblings share one argument reader (`0x608390`): a unit, then a 1-based index or a spell name
//! and rank, then a filter string ([`AuraQuery`]). One finder (`0x72c9b0`) walks the unit's list
//! under that filter ([`find`]).

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// One aura `UnitAura` answers with, as the live view builds it for the aura the finder takes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuraState {
    /// `Spell.dbc` id, what `CMSG_CANCEL_AURA` names and the app's icon lookup keys on.
    pub spell_id: u32,
    /// The spell's name; `None` when the catalog lacks the id.
    pub name: Option<String>,
    /// The spell's rank text (`Spell.dbc` `Rank`), `UnitAura`'s second return; `None` reads as
    /// the empty rank.
    pub rank: Option<String>,
    /// The icon's extensionless MPQ path (`Interface\Icons\…`).
    pub icon: Option<String>,
    /// Stack count; the reference shows it only above 1.
    pub count: u8,
    /// `"Magic"`, `"Curse"`, `"Disease"`, `"Poison"` or `None`; the debuff border tints by it.
    pub debuff_type: Option<String>,
    /// Seconds, from the last apply or refresh; 0 when permanent or unknown.
    pub duration: f64,
    /// The `GetTime()` instant it runs out; 0 when [`Self::duration`] is 0.
    pub expiration_time: f64,
    /// A buff rather than a debuff: the `HELPFUL`/`HARMFUL` filter's test, the aura record's
    /// flag `0x80` clear (`0x72ca9e`).
    pub helpful: bool,
    /// The aura record's flag `0x10`: the `CANCELABLE`/`NOT_CANCELABLE` filter's test
    /// (`0x72cad0`) and `CancelUnitBuff`'s first gate (`0x8042ec`).
    pub cancelable: bool,
    /// `GetPlayerBuff`'s second return, cache record `+0xc`: no finite duration to show. Derived
    /// from the spell, not from `expiration_time`, so it holds before any
    /// `SMSG_UPDATE_AURA_DURATION` and a permanent aura never shows a `0 s` timer.
    pub until_cancelled: bool,
    /// `AttributesEx & 0x4` (`SPELL_ATTR_EX_IS_CHANNELED`), [`cancel_authorized`]'s second arm.
    pub channeled: bool,
    /// The caster's guid, 0 when unknown: `UnitAura`'s eighth return names it as a token
    /// ([`caster_token`]), and the `PLAYER` filter keeps the player's own (`0x53bd70`).
    pub caster: u64,
    /// `UnitAura`'s ninth return, which the view decides as `0x53d680` does: the player can
    /// steal it (Spellsteal's dispel mask `[0xbe5d68]`) from a hostile unit's helpful aura.
    pub stealable: bool,
    /// `AttributesEx7 & 0x10000000`, the stock buff frame's consolidation flag, `UnitAura`'s
    /// tenth return (`0x6149c1`).
    pub consolidate: bool,
    /// In the finder's second list (`0x72c510`), the one `RAID` reads: a buff the player or its
    /// pet can cast, or a debuff whose dispel type the player can dispel.
    pub raid: bool,
}

/// What the finder's filter and name match read off one listed aura (`0x72ca9e`-`0x72cae3`),
/// before the aura is built: the view hands each in list order to the walk.
#[derive(Clone, Copy, Debug)]
pub struct AuraFacts<'a> {
    pub spell_id: u32,
    /// The aura record's flag `0x80` clear.
    pub helpful: bool,
    /// The aura record's flag `0x10`.
    pub cancelable: bool,
    /// In the finder's second list ([`AuraState::raid`]); false unless the walk asked for it.
    pub raid: bool,
    /// The caster's guid, 0 when unknown.
    pub caster: u64,
    /// The spell's name and rank text, `None` when the catalog lacks the id.
    pub name: Option<&'a str>,
    pub rank: Option<&'a str>,
}

impl<'a> AuraFacts<'a> {
    /// A built aura's facts, for a fixture's list.
    fn of(a: &'a AuraState) -> Self {
        Self {
            spell_id: a.spell_id,
            helpful: a.helpful,
            cancelable: a.cancelable,
            raid: a.raid,
            caster: a.caster,
            name: a.name.as_deref(),
            rank: a.rank.as_deref(),
        }
    }
}

/// A unit's raw aura slots, as the spell-id checks read them, unfiltered.
#[derive(Clone, Debug, PartialEq)]
pub enum UnitAuraSlots {
    /// A held unit's slots in slot order, an effect applied or not (`0x7282a0`, `0x7289c0`): each
    /// spell with its expiry on the `GetTime()` clock, 0 untimed.
    Held(Vec<(u32, f64)>),
    /// A member's roster record (`+0x20`): its online bit and its occupied `(slot, spell)`s,
    /// ascending.
    Member { online: bool, slots: Vec<(u8, u32)> },
    /// The pet record naming the unit (`+0x1c8`).
    Pet(Vec<(u8, u32)>),
}

/// A record block as the record walks read it (`0x614a80`, `0x612ec6`): slots from 0 up to the
/// first empty one (`0x614a83`).
pub(crate) fn leading_slots(slots: &[(u8, u32)]) -> impl Iterator<Item = u32> + '_ {
    slots
        .iter()
        .enumerate()
        .take_while(|(i, (slot, spell))| usize::from(*slot) == *i && *spell != 0)
        .map(|(_, (_, spell))| *spell)
}

/// The filter byte `0x608390` builds at `+0x14`: `HELPFUL` alone until a filter string names
/// otherwise (`0x608419`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AuraFilter(u8);

impl AuraFilter {
    const HELPFUL: u8 = 0x1;
    const HARMFUL: u8 = 0x2;
    const PLAYER: u8 = 0x4;
    const RAID: u8 = 0x8;
    const CANCELABLE: u8 = 0x10;
    const NOT_CANCELABLE: u8 = 0x20;

    /// Each token of `spec`, split on space and `|` (`0xa1f7fc`) and matched case-insensitively
    /// (`0x76e780`): `HELPFUL` and `HARMFUL` each clear the other (`0x608477`, `0x60849d`), so the
    /// later one wins; an unknown token sets nothing.
    fn parse(spec: Option<&str>) -> Self {
        let mut bits = Self::HELPFUL;
        for token in spec
            .unwrap_or("")
            .split([' ', '|'])
            .filter(|t| !t.is_empty())
        {
            let is = |name: &str| token.eq_ignore_ascii_case(name);
            if is("HELPFUL") {
                bits = (bits & !Self::HARMFUL) | Self::HELPFUL;
            } else if is("HARMFUL") {
                bits = (bits & !Self::HELPFUL) | Self::HARMFUL;
            } else if is("PLAYER") {
                bits |= Self::PLAYER;
            } else if is("RAID") {
                bits |= Self::RAID;
            } else if is("CANCELABLE") {
                bits |= Self::CANCELABLE;
            } else if is("NOT_CANCELABLE") {
                bits |= Self::NOT_CANCELABLE;
            }
        }
        Self(bits)
    }

    /// `UnitBuff` and `SetUnitBuff` force `HELPFUL` over whatever the string named (`0x614cd2`),
    /// `UnitDebuff` and `SetUnitDebuff` force `HARMFUL` (`0x614d22`).
    fn forced(self, helpful: bool) -> Self {
        if helpful {
            Self((self.0 & !Self::HARMFUL) | Self::HELPFUL)
        } else {
            Self((self.0 & !Self::HELPFUL) | Self::HARMFUL)
        }
    }

    /// Whether the walk must decide each aura's [`AuraFacts::raid`].
    fn raid(self) -> bool {
        self.0 & Self::RAID != 0
    }

    /// The finder's per-aura test (`0x72ca9e`-`0x72cae3`): the sign against `HELPFUL` alone, the
    /// caster under `PLAYER`, then the cancelable flag. `RAID` walks the finder's second list
    /// (`+0xdd4 + 0x84`, `0x72c9b0`), whose auras keep the main list's order, so here it keeps the
    /// auras that list holds ([`AuraFacts::raid`]).
    fn admits(self, a: &AuraFacts, player: u64) -> bool {
        let helpful = self.0 & Self::HELPFUL != 0;
        if a.helpful != helpful {
            return false;
        }
        if self.0 & Self::RAID != 0 && !a.raid {
            return false;
        }
        // `0x53bd70`: the caster is the player. Its second arm, a caster the player created or
        // summoned, is not modelled: `AuraState` holds the caster's guid alone.
        if self.0 & Self::PLAYER != 0 && (player == 0 || a.caster != player) {
            return false;
        }
        let refused = if a.cancelable {
            Self::NOT_CANCELABLE
        } else {
            Self::CANCELABLE
        };
        self.0 & refused == 0
    }
}

/// What the aura readers' shared argument list selects (into a `0x18`-byte record): the unit's
/// token, then a 0-based index (`-1` when a name is given, `0x6083f3`) or a name and optional rank,
/// and the filter.
pub(crate) struct AuraQuery {
    token: String,
    index: i64,
    name: Option<String>,
    rank: Option<String>,
    filter: AuraFilter,
}

/// `lua_isstring`: a string or a number.
fn is_string(v: Option<&Value>) -> bool {
    matches!(
        v,
        Some(Value::String(_) | Value::Integer(_) | Value::Number(_))
    )
}

/// `lua_tolstring`: a string, or a number's text; anything else none.
fn to_string(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.to_string_lossy()),
        Value::Integer(i) => Some(i.to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

impl AuraQuery {
    /// Read `args` as the reference does: a string unit and a number or string second argument, or
    /// `None`, where every caller raises its usage line. A numeric second argument is the index,
    /// truncated (`0x6083d1`), with the filter next; any other string is the name, the rank and
    /// then the filter.
    pub(crate) fn read(lua: &Lua, args: &[Value]) -> mlua::Result<Option<Self>> {
        if !is_string(args.first()) || !is_string(args.get(1)) {
            return Ok(None);
        }
        let token = to_string(args.first()).unwrap_or_default();
        let (index, name, rank, next) = match lua.coerce_number(args[1].clone())? {
            Some(n) => ((n as i64) - 1, None, None, 2),
            None => (-1, to_string(args.get(1)), to_string(args.get(2)), 3),
        };
        let filter = AuraFilter::parse(to_string(args.get(next)).as_deref());
        Ok(Some(Self {
            token,
            index,
            name,
            rank,
            filter,
        }))
    }

    /// [`Self::read`] for a `UnitBuff`/`UnitDebuff`-shaped verb, the sign forced.
    pub(crate) fn read_signed(
        lua: &Lua,
        args: &[Value],
        helpful: Option<bool>,
    ) -> mlua::Result<Option<Self>> {
        Ok(Self::read(lua, args)?.map(|mut q| {
            if let Some(h) = helpful {
                q.filter = q.filter.forced(h);
            }
            q
        }))
    }

    /// The guid the token names, nobody for an unknown token (`0x60abf0` raises for none).
    pub(crate) fn guid(&self, model: &Model) -> Option<u64> {
        model.token_guid(&self.token)
    }
}

/// The reference's UTF-8 case-insensitive compare: the whole string, case-insensitively.
fn same_text(a: &str, b: &str) -> bool {
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}

/// The aura `q` selects, as the finder `0x72c9b0` walks the unit's list: the `index`-th aura
/// passing the filter, or the first passing it whose name, and rank when one is given, match.
/// An index below 0 with no name finds nothing (`0x72c9c2`).
pub(crate) fn find(model: &Model, q: &AuraQuery) -> Option<AuraState> {
    if q.index < 0 && q.name.is_none() {
        return None;
    }
    let guid = q.guid(model)?;
    let player = model.player_guid();
    let mut found = 0i64;
    let mut pick = |a: &AuraFacts| {
        if !q.filter.admits(a, player) {
            return false;
        }
        if q.index >= 0 {
            found += 1;
            return found - 1 == q.index;
        }
        q.name.as_deref().is_some_and(|name| {
            a.name.is_some_and(|n| same_text(n, name))
                && q.rank
                    .as_deref()
                    .is_none_or(|r| same_text(a.rank.unwrap_or(""), r))
        })
    };
    walk(model, guid, q.filter.raid(), &mut pick)
}

/// Walk `guid`'s listed auras in order through the lent view, building the first `pick` takes;
/// where no view lends the game's, a fixture's list.
fn walk(
    model: &Model,
    guid: u64,
    raid: bool,
    pick: &mut dyn FnMut(&AuraFacts) -> bool,
) -> Option<AuraState> {
    let now = model.now;
    if let Some(found) =
        super::live::read_live(|l| l.find_unit_aura(guid, now, raid, pick)).flatten()
    {
        return found;
    }
    fixture_list(model, guid)?
        .iter()
        .find(|a| pick(&AuraFacts::of(a)))
        .cloned()
}

/// A fixture's list for `guid`: the player's for the player, else that guid's.
#[cfg(any(test, feature = "fixtures"))]
fn fixture_list(model: &Model, guid: u64) -> Option<&[AuraState]> {
    if guid == model.player_guid() {
        Some(model.player_auras.as_slice())
    } else {
        model.unit_auras.get(&guid).map(Vec::as_slice)
    }
}

#[cfg(not(any(test, feature = "fixtures")))]
fn fixture_list(_model: &Model, _guid: u64) -> Option<&[AuraState]> {
    None
}

/// `guid`'s raw aura slots at the call; `None` for nobody. Where no view lends the game's, a
/// fixture's list stands in as a held unit's.
pub(crate) fn slots_of(model: &Model, guid: u64) -> Option<UnitAuraSlots> {
    let now = model.now;
    if let Some(slots) = super::live::read_live(|l| l.unit_aura_slots(guid, now)).flatten() {
        return slots;
    }
    fixture_list(model, guid).map(|list| {
        UnitAuraSlots::Held(
            list.iter()
                .map(|a| (a.spell_id, a.expiration_time))
                .collect(),
        )
    })
}

/// The token names a caster by: the first token the unit-event fan-out lists for it
/// (`0x60bb70`), else `mouseover`; nil for none.
fn caster_token(model: &Model, caster: u64) -> Option<String> {
    let tokens = model.tokens();
    tokens.tokens_naming(caster).into_iter().next().or_else(|| {
        (caster != 0 && tokens.base_guid(super::UnitBase::Mouseover) == caster)
            .then(|| "mouseover".into())
    })
}

/// The eleven values `0x6147c0` pushes for a found aura: `name, rank, icon, count, debuffType,
/// duration, expirationTime, caster, isStealable, shouldConsolidate, spellId`. A timed aura's
/// two times are seconds on the `GetTime()` clock (`0x61493d`, `0x9e56b0` = 0.001), and both
/// are 0 for an untimed one; the two flags are 1 or nil.
fn returns(lua: &Lua, a: &AuraState, caster: Option<String>) -> mlua::Result<MultiValue> {
    let text = |t: Option<&str>| -> mlua::Result<Value> {
        Ok(match t {
            Some(t) => Value::String(lua.create_string(t)?),
            None => Value::Nil,
        })
    };
    let flag = |b: bool| if b { Value::Integer(1) } else { Value::Nil };
    let (duration, expiration) = if a.duration > 0.0 {
        (a.duration, a.expiration_time)
    } else {
        (0.0, 0.0)
    };
    Ok(MultiValue::from_vec(vec![
        text(a.name.as_deref())?,
        text(Some(a.rank.as_deref().unwrap_or("")))?,
        text(a.icon.as_deref())?,
        Value::Integer(i64::from(a.count)),
        text(a.debuff_type.as_deref())?,
        Value::Number(duration),
        Value::Number(expiration),
        text(caster.as_deref())?,
        flag(a.stealable),
        flag(a.consolidate),
        Value::Integer(i64::from(a.spell_id)),
    ]))
}

/// `UnitAura` (`0x614d40`) and its two shorthands: `helpful` forces the sign. A bad argument list
/// raises `usage`; a miss answers nothing, the loop terminator.
fn unit_aura(
    lua: &Lua,
    args: MultiValue,
    helpful: Option<bool>,
    usage: &'static str,
) -> mlua::Result<MultiValue> {
    let args = args.into_vec();
    let Some(q) = AuraQuery::read_signed(lua, &args, helpful)? else {
        return Err(mlua::Error::RuntimeError(usage.into()));
    };
    let hit = {
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model app_data");
        find(&model, &q).map(|a| {
            let caster = caster_token(&model, a.caster);
            (a, caster)
        })
    };
    match hit {
        Some((a, caster)) => returns(lua, &a, caster),
        None => Ok(MultiValue::new()),
    }
}

impl super::UiScript {
    /// A fixture's stand-in player aura list, in display order, which a call reads while no view
    /// lends the game's.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn set_player_auras(&mut self, auras: Vec<AuraState>) {
        set_field!(self, player_auras, auras);
    }

    /// A fixture's stand-in aura list for a unit by guid ([`Self::set_player_auras`]).
    #[cfg(any(test, feature = "fixtures"))]
    pub fn set_unit_auras(&mut self, guid: u64, auras: Option<Vec<AuraState>>) {
        let mut model = self.model_mut();
        match auras {
            Some(a) => {
                model.unit_auras.insert(guid, a);
            }
            None => {
                model.unit_auras.remove(&guid);
            }
        }
    }

    /// The resolver's tables a call reads while no view lends the game's: a test's or a capture
    /// fixture's, never the game's, which every call reads live.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn set_unit_guids(&mut self, guids: &super::UnitGuids) {
        self.model_mut().unit_fixture = Some(guids.clone());
    }

    /// The selection the last `PLAYER_TARGET_CHANGED` this VM heard announced; `None` in a new VM.
    pub fn target_signalled(&self) -> Option<u64> {
        self.model_ref().target_signalled
    }

    /// Record that `PLAYER_TARGET_CHANGED` announced `selection`.
    pub fn set_target_signalled(&mut self, selection: Option<u64>) {
        self.model_mut().target_signalled = selection;
    }

    /// A fixture's arena slots alone ([`Self::set_unit_guids`]).
    #[cfg(any(test, feature = "fixtures"))]
    pub fn set_arena_unit_guids(&mut self, arena: [u64; 5], pets: [u64; 5], count: usize) {
        let mut model = self.model_mut();
        let g = model.unit_fixture.get_or_insert_with(Default::default);
        (g.arena, g.arena_pets, g.arena_count) = (arena, pets, count);
    }

    /// The name lookup alone (`0x60a630`), for the host's resolver, which reads every other part
    /// of a token live and hands its own read of the bases and rows (`bases`, `counts`): the guid
    /// a group member's or pet's name in `token` names, and the hops after it.
    pub fn unit_by_name<'t>(
        &self,
        token: &'t str,
        bases: super::HostBases<'_>,
        counts: super::UnitCounts,
    ) -> Option<(u64, super::Hops<'t>)> {
        let model = self.model_ref();
        let tokens = super::unit::Tokens::host(bases, counts, model.focus);
        tokens.by_name(token, |ask| model.unit_name(ask))
    }

    /// Every token naming `guid` at the call, for a per-token unit event (`0x60bb70`).
    pub fn unit_tokens(&self, guid: u64) -> Vec<String> {
        self.model_ref().tokens().tokens_naming(guid)
    }

    /// The spell ids of the buffs (`helpful`) or debuffs of the unit `token` names, in the order
    /// `UnitBuff`/`UnitDebuff` enumerate them with no filter string; empty for nobody.
    pub fn aura_spell_ids(&self, token: &str, helpful: bool) -> Vec<u32> {
        let model = self.model_ref();
        let mut ids = Vec::new();
        if let Some(guid) = model.token_guid(token) {
            walk(&model, guid, false, &mut |a| {
                if a.helpful == helpful {
                    ids.push(a.spell_id);
                }
                false
            });
        }
        ids
    }

    /// Drain the queued cancels, one `CMSG_CANCEL_AURA` per spell id: the server cancels by spell,
    /// never by slot.
    pub fn take_cancel_aura_requests(&mut self) -> Vec<u32> {
        drain_field!(self, cancel_aura_requests)
    }
}

/// Register the aura and tracking globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // UnitAura("unit", [index] or ["name", "rank"][, "filter"]) (`0x614d40`), and UnitBuff
    // (`0x614ca0`) and UnitDebuff (`0x614cf0`), which read the same list with the sign forced.
    for (verb, helpful, usage) in [
        (
            "UnitAura",
            None,
            r#"Usage: UnitAura("unit", [index] or ["name", "rank"][, "filter"])"#,
        ),
        (
            "UnitBuff",
            Some(true),
            r#"Usage: UnitBuff("unit", [index] or ["name", "rank"][, "filter"])"#,
        ),
        (
            "UnitDebuff",
            Some(false),
            r#"Usage: UnitDebuff("unit", [index] or ["name", "rank"][, "filter"])"#,
        ),
    ] {
        g.set(
            verb,
            lua.create_function(move |lua, args: MultiValue| unit_aura(lua, args, helpful, usage))?,
        )?;
    }

    // CancelUnitBuff("unit", [index] or ["name", "rank"][, "filter"]) (`0x804220`), the stock
    // buff button's right-click (`BuffFrame.lua:272`): only the player's own auras cancel
    // (`0x80427c`), those flagged cancelable or of a channeled spell (`0x8042ec`-`0x8042f7`), by
    // queueing the spell id for `CMSG_CANCEL_AURA` (`0x802f80`). Anything else is a silent no-op,
    // with no return. The reference's other arm, leaving a vehicle the player controls by its
    // `0xec` aura (`0x804322`), is not modelled.
    g.set(
        "CancelUnitBuff",
        lua.create_function(|lua, args: MultiValue| {
            let args = args.into_vec();
            let Some(q) = AuraQuery::read(lua, &args)? else {
                return Err(mlua::Error::RuntimeError(
                    r#"Usage: CancelUnitBuff("unit", [index] or ["name", "rank"][, "filter"])"#
                        .into(),
                ));
            };
            let mut model = crate::script::model_mut_of(lua);
            let player = model.player_guid();
            if player == 0 || q.guid(&model) != Some(player) {
                return Ok(());
            }
            if let Some(a) = find(&model, &q).filter(|a| a.cancelable || a.channeled) {
                model.cancel_aura_requests.push(a.spell_id);
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests;
