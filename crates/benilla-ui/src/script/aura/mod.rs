//! The aura bindings, over the lists the app pushes in display order: the player's, kept by
//! `benilla::ui_aura`, and every other unit's by guid. A token resolves to its guid as 12340's
//! resolver `0x60abf0` does ([`super::UnitGuids`]), so `"mouseover"`, `"party1target"` and
//! `"PLAYER"` read the list of the unit they name, and a token naming the player reads the
//! player's list.
//!
//! `UnitAura`, `UnitBuff`, `UnitDebuff`, `CancelUnitBuff` and `GameTooltip:SetUnitAura` and its two
//! siblings share one argument reader (`0x6110a0`, `0x608390`): a unit, then a 1-based index or a
//! spell name and rank, then a filter string ([`AuraQuery`]). One finder (`0x72c9b0`) walks the
//! unit's list under that filter ([`find`]). The 1.12 `GetPlayerBuff` family ([`player_buff`])
//! is still installed beside them, though 3.3.5a's stock UI no longer calls it (#52).

use mlua::{Lua, MultiValue, Value};

use super::Model;

mod player_buff;

/// One aura on one unit, as the app's feed pushes it ([`crate::script::UiScript::set_unit_auras`],
/// [`crate::script::UiScript::set_player_auras`]).
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
    /// from the spell (`0x4e452e`-`0x4e45c5`), not from `expiration_time`, so it holds before any
    /// `SMSG_UPDATE_AURA_DURATION` and a permanent aura never shows a `0 s` timer.
    pub until_cancelled: bool,
    /// `AttributesEx & 0x4` (`SPELL_ATTR_EX_IS_CHANNELED`), [`cancel_authorized`]'s second arm.
    pub channeled: bool,
    /// The caster's guid, 0 when unknown: `UnitAura`'s eighth return names it as a token
    /// ([`caster_token`]), and the `PLAYER` filter keeps the player's own (`0x53bd70`).
    pub caster: u64,
    /// `UnitAura`'s ninth return, which the feed decides as `0x53d680` does: the player can
    /// steal it (Spellsteal's dispel mask `[0xbe5d68]`) from a hostile unit's helpful aura.
    pub stealable: bool,
    /// `AttributesEx7 & 0x10000000`, the stock buff frame's consolidation flag, `UnitAura`'s
    /// tenth return (`0x6149c1`).
    pub consolidate: bool,
}

/// The player's active tracking aura, the reference's tracking global (`0xbc6378`): the cache
/// rebuild records a spell with a tracking effect (`0x2c`, `0x2d`, `0x97`) there instead of in
/// the display cache, and `GetTrackingTexture` (`0x4e4a20`) reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackingState {
    /// `Spell.dbc` id, the `CMSG_CANCEL_AURA` payload of `CancelTrackingBuff`.
    pub spell_id: u32,
    /// The spell's name; `None` when the catalog lacks the id.
    pub name: Option<String>,
    /// The icon's extensionless MPQ path, `GetTrackingTexture`'s return.
    pub icon: Option<String>,
    /// The wire's `AFLAG_CANCELABLE` on the aura's slot, `CancelTrackingBuff`'s gate.
    pub cancelable: bool,
}

/// The cancel gate of `CancelPlayerBuff` (`0x4e49a0`, `0x4e49fb`-`0x4e4a14`):
/// a helpful aura with `AFLAG_CANCELABLE` (vmangos: unless `SPELL_ATTR_NO_AURA_CANCEL`), or any
/// aura of a channeled spell, the only negative aura that cancels (breaking a channel on you).
pub(super) fn cancel_authorized(a: &AuraState) -> bool {
    (a.helpful && a.cancelable) || a.channeled
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

    /// The finder's per-aura test (`0x72ca9e`-`0x72cae3`): the sign against `HELPFUL` alone, the
    /// caster under `PLAYER`, then the cancelable flag. `RAID` picks the finder's second ordering
    /// of the unit's list (`+0xdd4 + 0x84`); the pushed order stands for both.
    fn admits(self, a: &AuraState, player: u64) -> bool {
        let helpful = self.0 & Self::HELPFUL != 0;
        if a.helpful != helpful {
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

/// What the aura readers' shared argument list selects (`0x6110a0` into a `0x18`-byte record):
/// the unit's token, then a 0-based index (`-1` when a name is given, `0x6083f3`) or a name and
/// optional rank, and the filter.
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
    /// Read `args` as `0x6110a0` does: a string unit and a number or string second argument, or
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
        model.unit_guids.guid_of(&self.token)
    }
}

/// `SStrCmpUTF8I`'s fold (`0x76ea40`): the whole string, case-insensitively.
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
    let player = model.unit_guids.player;
    let list = auras_of(model, &q.token)?;
    let mut admitted = list.iter().filter(|a| q.filter.admits(a, player));
    if q.index >= 0 {
        return admitted.nth(usize::try_from(q.index).ok()?).cloned();
    }
    let name = q.name.as_deref()?;
    admitted
        .find(|a| {
            a.name.as_deref().is_some_and(|n| same_text(n, name))
                && q.rank
                    .as_deref()
                    .is_none_or(|r| same_text(a.rank.as_deref().unwrap_or(""), r))
        })
        .cloned()
}

/// The list of the unit `token` names: the player's for the player, else that guid's; `None` for
/// nobody or a unit with no list.
pub(crate) fn auras_of<'m>(model: &'m Model, token: &str) -> Option<&'m [AuraState]> {
    let guids = &model.unit_guids;
    guids.guid_of(token).and_then(|g| {
        if g == guids.player {
            Some(model.player_auras.as_slice())
        } else {
            model.unit_auras.get(&g).map(Vec::as_slice)
        }
    })
}

/// The token `0x60b420` names a caster by: the first of `player`, `pet`, `partyN`, `partypetN`,
/// `raidN`, `raidpetN`, `target`, `npc` and `mouseover` that is the caster, nil for none. The
/// reference also tries `vehicle`, `arenaN`, `arenapetN` and `focus`, which the resolver has no
/// guid for yet (#90).
fn caster_token(model: &Model, caster: u64) -> Option<String> {
    let guids = &model.unit_guids;
    guids
        .tokens_naming(caster)
        .into_iter()
        .next()
        .or_else(|| (caster != 0 && guids.mouseover == caster).then(|| "mouseover".into()))
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
    /// Push the player's aura list in display order, durations joined.
    pub fn set_player_auras(&mut self, auras: Vec<AuraState>) {
        set_field!(self, player_auras, auras);
    }

    /// Push or clear a unit's aura list by guid, in display order; any token naming the unit
    /// reads it. The player's is [`Self::set_player_auras`].
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

    /// Drop every aura list, the player's with the rest: the session's end.
    pub fn clear_auras(&mut self) {
        let mut model = self.model_mut();
        model.player_auras.clear();
        model.unit_auras.clear();
    }

    /// Push the unit-token resolver's inputs, copied only when they moved.
    pub fn set_unit_guids(&mut self, guids: &super::UnitGuids) {
        if self.model_ref().unit_guids.same_push(guids) {
            return;
        }
        let mut model = self.model_mut();
        // The focus is the VM's own: `FocusUnit` sets it, never a push.
        let focus = model.unit_guids.focus;
        model.unit_guids.clone_from(guids);
        model.unit_guids.focus = focus;
    }

    /// Every token naming `guid` against the last push, for a per-token unit event (`0x60bb70`).
    pub fn unit_tokens(&self, guid: u64) -> Vec<String> {
        self.model_ref().unit_guids.tokens_naming(guid)
    }

    /// Push or clear the player's active [`TrackingState`].
    pub fn set_tracking(&mut self, tracking: Option<TrackingState>) {
        set_field!(self, tracking, tracking);
    }

    /// The spell ids of the buffs (`helpful`) or debuffs of the unit `token` names, in the order
    /// `UnitBuff`/`UnitDebuff` enumerate them with no filter string; empty for nobody.
    pub fn aura_spell_ids(&self, token: &str, helpful: bool) -> Vec<u32> {
        let model = self.model_ref();
        auras_of(&model, token)
            .map(|list| {
                list.iter()
                    .filter(|a| a.helpful == helpful)
                    .map(|a| a.spell_id)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Drain the queued cancels, one `CMSG_CANCEL_AURA` per spell id: the server cancels by spell,
    /// never by slot.
    pub fn take_cancel_aura_requests(&mut self) -> Vec<u32> {
        drain_field!(self, cancel_aura_requests)
    }
}

/// Register the aura and tracking globals, the `GetPlayerBuff` family from [`player_buff`].
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
            let player = model.unit_guids.player;
            if player == 0 || q.guid(&model) != Some(player) {
                return Ok(());
            }
            if let Some(a) = find(&model, &q).filter(|a| a.cancelable || a.channeled) {
                model.cancel_aura_requests.push(a.spell_id);
            }
            Ok(())
        })?,
    )?;

    player_buff::install(lua)?;

    // GetTrackingTexture() (`0x4e4a20`): the tracking spell's icon or nil, which shows or hides
    // the stock `MiniMapTrackingFrame` (`Minimap.xml:150-159`).
    g.set(
        "GetTrackingTexture",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.tracking.as_ref().and_then(|t| t.icon.clone()))
        })?,
    )?;

    // CancelTrackingBuff() (`0x4e4a80`), the tracking frame's right-click: queues the tracking
    // spell's id behind its `AFLAG_CANCELABLE`; with no tracking it is a no-op.
    g.set(
        "CancelTrackingBuff",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            if let Some(t) = model.tracking.as_ref().filter(|t| t.cancelable) {
                let id = t.spell_id;
                model.cancel_aura_requests.push(id);
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests;
