//! The `Unit*` global registrations, over the per-token [`UnitState`](super::UnitState) snapshots.
//! Every predicate answers the number 1 or nil through [`super::unit_predicate`] or
//! [`flag`](super::super::binding_abi::flag): no binding in the reference's unit table (`0x850438`)
//! calls `lua_pushboolean` (`0x6f39f0`), though `IsPetAttackActive` (`0x4be0e0`) outside it does.

use mlua::{Lua, MultiValue, Value};

use super::super::binding_abi::flag;
use super::super::calls::ScriptCall;
use super::{
    classification_word, grey_band, level_reads_unknown, pair_reaction, parse_unit_token,
    pick_unit_token, unit_predicate, unknownobject, with_unit, PlayerRecord, SelectionRequest,
    UnitBase, UnitTokenParse,
};

/// The type `UnitPower` and `UnitPowerMax` read with no type argument, the unit's own display
/// power (`0x60ed72`, `0x60effd`).
const OWN_POWER_TYPE: i64 = 7;

/// `UnitPower`/`UnitPowerMax`: the token must pass `lua_isstring`, else the usage line raises. A
/// numeric type goes through `lua_tointeger` (`0x84e070`, `fistp` at the default round-to-nearest)
/// and outside `0..=7` answers 0; 7 or no type is the unit's display type. A held unit answers
/// its own type, and another type only for the local player's current power (`0x4cee50`); the
/// maximum of any type answers for any held unit. A unit the client holds no object for answers
/// its party record's type alone (`0x513c30`), and an unknown token reads nobody, 0.
fn unit_power(
    lua: &Lua,
    token: Value,
    ty: Value,
    usage: &'static str,
    max: bool,
) -> mlua::Result<i64> {
    let token = super::super::binding_abi::string_arg(lua, token, usage)?;
    let ty = match ty {
        Value::Number(_) | Value::Integer(_) | Value::String(_) => lua
            .coerce_number(ty)?
            .map_or(OWN_POWER_TYPE, |n| n.round_ties_even() as i64),
        _ => OWN_POWER_TYPE,
    };
    if !(0..=OWN_POWER_TYPE).contains(&ty) {
        return Ok(0);
    }
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    let player = model.unit_guids.player;
    let Some(u) = model.unit(&token) else {
        return Ok(0);
    };
    let own = i64::from(u.power_type);
    let ty = if ty == OWN_POWER_TYPE { own } else { ty };
    let shown = if ty == own {
        if max {
            u.max_power
        } else {
            u.power
        }
    } else if !u.has_object || (!max && (player == 0 || u.guid != player)) {
        0
    } else {
        let table = if max {
            &u.shown_max_powers
        } else {
            &u.shown_powers
        };
        usize::try_from(ty)
            .ok()
            .and_then(|i| table.get(i))
            .copied()
            .unwrap_or(0)
    };
    Ok(i64::from(shown))
}

/// The `"player"` fast path of `UnitName`, `UnitRace`, `UnitClass` and `UnitSex`, never
/// `UnitLevel`: the record's answer when the token equals `"player"` (`0x847894`) whole and
/// case-insensitively; any other token, `"playerfoo"` included, goes to the resolver.
fn player_record_arm<T>(
    lua: &Lua,
    token: &Option<String>,
    f: impl FnOnce(&PlayerRecord) -> T,
) -> Option<T> {
    if !token
        .as_deref()
        .is_some_and(|t| t.eq_ignore_ascii_case("player"))
    {
        return None;
    }
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    Some(f(&model.player_record))
}

/// The class ids `GetComboPoints` (`0x51a190`) accepts: it compares the class, `UNIT_FIELD_BYTES_0`
/// byte 1, with 4 and 0xb.
const CLASS_ROGUE: u32 = 4;
const CLASS_DRUID: u32 = 11;

/// Register the `Unit*` globals.
pub(in crate::script) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "UnitExists",
        lua.create_function(|lua, token: Option<String>| {
            unit_predicate(lua, &token, |u| u.exists)
        })?,
    )?;

    // `UnitIsVisible(unit)` (`0x516030`): whether the client holds the unit's object, nothing else.
    // Not `UnitExists`: an out-of-range party member exists through its roster entry, unseen.
    g.set(
        "UnitIsVisible",
        lua.create_function(|lua, token: Option<String>| {
            unit_predicate(lua, &token, |u| u.has_object)
        })?,
    )?;

    // `UnitIsTapped`/`UnitIsTappedByPlayer` (`0x519c90`/`0x519d00`): the object is present and its
    // `UNIT_DYNAMIC_FLAGS` has the mask bit, nothing more. Unlike `UnitIsVisible` they gate with
    // `lua_isstring`: another type raises `Usage:`, and a number raises `Unknown unit name`.
    for (name, usage, by_player) in [
        ("UnitIsTapped", r#"Usage: UnitIsTapped("unit")"#, false),
        (
            "UnitIsTappedByPlayer",
            r#"Usage: UnitIsTappedByPlayer("unit")"#,
            true,
        ),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, token: Value| {
                let token = Some(crate::script::binding_abi::string_arg(lua, token, usage)?);
                let hit = with_unit(lua, &token, false, |u| {
                    if by_player {
                        u.tapped_by_player
                    } else {
                        u.tapped
                    }
                })?;
                Ok(flag(hit))
            })?,
        )?;
    }
    // `UnitIsTappedByAllThreatList(unit)` (`0x611230`): 1 or nil, the held object's
    // `UNIT_DYNAMIC_FLAGS` bit `0x80` (`0x611276`), every attacker credited; the same
    // `lua_isstring` gate.
    g.set(
        "UnitIsTappedByAllThreatList",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitIsTappedByAllThreatList("unit")"#,
            )?);
            let hit = with_unit(lua, &token, false, |u| {
                u.has_object && u.dynamic_flags & 0x80 != 0
            })?;
            Ok(flag(hit))
        })?,
    )?;

    // `UnitHasVehicleUI(unit)` (`0x613740`): a Lua boolean, the flag `0x20000000` of the seat the
    // unit rides (`0x613600`: the held unit's vehicle seat, else a party member's seat id). The
    // token goes through `lua_tolstring` and the non-raising resolver (`0x60abf0`), so any
    // argument answers. benilla tracks no vehicle, so every unit sits on no seat and answers
    // false, as the macro conditions `vehicleui` and `unithasvehicleui` do.
    g.set(
        "UnitHasVehicleUI",
        lua.create_function(|_, _token: Value| Ok(false))?,
    )?;
    // `UnitControllingVehicle(unit)` (`0x613570`): a Lua boolean, true for a unit on a seat of
    // kind 3 riding its own charm (`0x6135b8`-`0x6135db`); the same non-raising token path, so
    // false on no seat, as above.
    g.set(
        "UnitControllingVehicle",
        lua.create_function(|_, _token: Value| Ok(false))?,
    )?;
    // `UnitTargetsVehicleInRaidUI(unit)` (`0x613780`): the flag `0x8` of the same seat
    // (`0x613798`), so false on no seat, as above.
    g.set(
        "UnitTargetsVehicleInRaidUI",
        lua.create_function(|_, _token: Value| Ok(false))?,
    )?;
    // `UnitInVehicle(unit)` (`0x6133d0`): 1 or nil, a held unit's passenger state, else the party
    // record's status bit `0x200` (`0x61347b`).
    g.set(
        "UnitInVehicle",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitInVehicle("unit")"#,
            )?);
            unit_predicate(lua, &token, |u| u.in_vehicle)
        })?,
    )?;

    // `IsReferAFriendLinked(unit)` (`0x51d1b0`): 1 or nil, never raising, and nil with no player
    // object; the token resolves to a guid (`0x60c1c0`) that `IsRAFPlayer` (`0x6d7590`) asks.
    g.set(
        "IsReferAFriendLinked",
        lua.create_function(|lua, token: Value| {
            let token = crate::script::binding_abi::optional_string(lua, &token);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            if model.unit_guids.player == 0 {
                return Ok(Value::Nil);
            }
            let unit = token.as_deref().and_then(|t| model.unit(t));
            Ok(flag(crate::script::unit_menu::is_raf_player(&model, unit)))
        })?,
    )?;

    // `UnitIsPartyLeader(unit)` (`0x516210`): a held player's `PLAYER_FLAGS` bit 0x1, for a
    // stranger leading their own party, or a GUID equal to the group leader's, for a member not
    // held. No zero guard (unlike `IsPartyLeader`, `0x4e9130`): GUID 0 matches the zeroed leader,
    // so `UnitIsPartyLeader(nil)` answers 1 while ungrouped. No `Usage:` gate; a bad token or a
    // number raises `Unknown unit name`.
    g.set(
        "UnitIsPartyLeader",
        lua.create_function(|lua, token: Option<String>| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let by_flag = token
                .as_ref()
                .and_then(|t| model.unit(t))
                .is_some_and(|u| u.group_leader);
            // A token with no snapshot is GUID 0, the reference's `0:0`.
            let guid = token
                .as_ref()
                .and_then(|t| model.unit(t))
                .map_or(0, |u| u.guid);
            Ok(flag(by_flag || guid == model.party.leader_guid))
        })?,
    )?;

    g.set(
        "UnitName",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitName("unit")"#,
            )?);
            // Two values (`0x5170ae`, `0x517289`): the name, and the realm, nil for a same-realm
            // player or a non-player (`0x609210`), so always nil in single-realm benilla. A second
            // argument (stock `UnitPopup.lua:106` passes `true`) is not read. The name is nil only
            // for an empty `"player"` buffer (`0x51708c`, `0x6f3895`) or GUID 0 (`0x5170c0`);
            // otherwise a string, `UNKNOWNOBJECT` until the name cache answers (`0x517220`,
            // `0x609324`), as for a new pet. A seated snapshot answers even where `exists` is
            // false; the `"player"` arm reads the local name buffer (`0xc27d88`), never the unit.
            if let Some(seeded) = player_record_arm(lua, &token, |r| r.name.clone()) {
                let name = if seeded.is_empty() {
                    Value::Nil
                } else {
                    Value::String(lua.create_string(&seeded)?)
                };
                return Ok((name, Value::Nil));
            }
            let name = with_unit(lua, &token, None, |u| Some(u.name.clone()))?;
            let name = match name {
                None => Value::Nil,
                Some(Some(n)) => Value::String(lua.create_string(&n)?),
                Some(None) => Value::String(unknownobject(lua)?),
            };
            Ok((name, Value::Nil))
        })?,
    )?;

    g.set(
        "UnitHealth",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitHealth("unit")"#,
            )?);
            with_unit(lua, &token, 0i64, |u| i64::from(u.health))
        })?,
    )?;

    g.set(
        "UnitHealthMax",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitHealthMax("unit")"#,
            )?);
            with_unit(lua, &token, 0i64, |u| i64::from(u.max_health))
        })?,
    )?;

    // UnitLevel (`0x517fc0`): the raw level (a raw 0 stays 0), or -1 for a world boss or a hostile
    // unit 10 or more levels above the player. The reference's attackable-decay override
    // (`max(1, raw - round(min(b, 100) * 0.05))`, `b` an untraced byte not seen set) is not built.
    g.set(
        "UnitLevel",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitLevel("unit")"#,
            )?);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let Some(u) = token.as_ref().and_then(|t| model.unit(t)) else {
                return Ok(0i64);
            };
            Ok(if u.level == 0 {
                0
            } else if level_reads_unknown(u, model.player_req.level) {
                -1
            } else {
                i64::from(u.level)
            })
        })?,
    )?;

    // UnitIsCorpse (`0x5161c0`): the token names a corpse object, a released player's remains; a
    // dead unit is not one. No feed sets `corpse_object`, so this answers nil.
    g.set(
        "UnitIsCorpse",
        lua.create_function(|lua, token: Option<String>| {
            unit_predicate(lua, &token, |u| u.corpse_object)
        })?,
    )?;

    // UnitCanAttack (`0x516c50`) delegates to `CanAttack` (`0x606980`). The snapshot holds only
    // whether the player can attack the unit, fed for `target`, `targettarget` and `npc`, and that
    // answers both argument orders; the reverse direction stock FrameXML also asks
    // (`TargetFrame.lua:147`) is not built.
    g.set(
        "UnitCanAttack",
        lua.create_function(|lua, (a, b): (Value, Value)| {
            // Both arguments are `lua_isstring`-gated, so either one nil raises.
            let a = Some(crate::script::binding_abi::string_arg(
                lua,
                a,
                r#"Usage: UnitCanAttack("unit", "otherUnit")"#,
            )?);
            let b = Some(crate::script::binding_abi::string_arg(
                lua,
                b,
                r#"Usage: UnitCanAttack("unit", "otherUnit")"#,
            )?);
            let token = pick_unit_token(&a, &b);
            unit_predicate(lua, &token, |u| u.can_attack)
        })?,
    )?;

    // GetQuestGreenRange (`0x4e17d0`): the green-to-grey boundary `QuestLogFrame.lua:593` buckets
    // by, from the table at `0x8076c0`. The reference answers 0 with no player object (`0x4e17f8`);
    // here a level of 0 reads the table's first entry, 4.
    g.set(
        "GetQuestGreenRange",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(grey_band(model.player_req.level)))
        })?,
    )?;

    g.set(
        "UnitIsDead",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitIsDead("unit")"#,
            )?);
            unit_predicate(lua, &token, |u| u.dead)
        })?,
    )?;

    // A released ghost has health 1, so `UnitIsDead` is false for it.
    g.set(
        "UnitIsGhost",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitIsGhost("unit")"#,
            )?);
            unit_predicate(lua, &token, |u| u.ghost)
        })?,
    )?;
    g.set(
        "UnitIsDeadOrGhost",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitIsDeadOrGhost("unit")"#,
            )?);
            unit_predicate(lua, &token, |u| u.dead || u.ghost)
        })?,
    )?;

    // UnitReaction(unit, other) (12340 `0x60d280`): both tokens resolved (`0x60c1f0`), then
    // `0x7251c0(unit, other)` plus one (`0x60d2dc`), so 1-based, 5 for a unit toward itself; nil
    // (`0x60d300`) when either names no unit. Our reaction 0 means "not fed" and also answers nil,
    // where the reference answers a number. [`pair_reaction`] reads the pair.
    g.set(
        "UnitReaction",
        lua.create_function(|lua, (a, b): (Value, Value)| {
            // Both arguments are `lua_isstring`-gated, so either one nil raises.
            let usage = r#"Usage: UnitReaction("unit", "otherUnit")"#;
            let a = crate::script::binding_abi::string_arg(lua, a, usage)?;
            let b = crate::script::binding_abi::string_arg(lua, b, usage)?;
            Ok(pair_reaction(lua, &a, &b).map_or(Value::Nil, |r| Value::Integer(i64::from(r))))
        })?,
    )?;

    // UnitIsEnemy (12340 `0x60d330`) asks the same `0x7251c0(unit, other)` and answers 1 at 0 or 1
    // (`0x60d38b`), 1 or 2 on `UnitReaction`'s scale; UnitIsFriend (`0x60d3d0`) through `0x514050`
    // at 4 and up, 5 on that scale. An unfed or unresolved pair is neither. Not built:
    // UnitIsFriend's creature-template leg (`0x715df0`, type flag bit 26) and its party-roster
    // leg for a unit with no object (`0x512a30`).
    g.set(
        "UnitIsEnemy",
        lua.create_function(|lua, (a, b): (Value, Value)| {
            // Both arguments are `lua_isstring`-gated, so either one nil raises.
            let usage = r#"Usage: UnitIsEnemy("unit", "otherUnit")"#;
            let a = crate::script::binding_abi::string_arg(lua, a, usage)?;
            let b = crate::script::binding_abi::string_arg(lua, b, usage)?;
            Ok(flag(pair_reaction(lua, &a, &b).is_some_and(|r| r <= 2)))
        })?,
    )?;
    g.set(
        "UnitIsFriend",
        lua.create_function(|lua, (a, b): (Value, Value)| {
            // Both arguments are `lua_isstring`-gated, so either one nil raises.
            let usage = r#"Usage: UnitIsFriend("unit", "otherUnit")"#;
            let a = crate::script::binding_abi::string_arg(lua, a, usage)?;
            let b = crate::script::binding_abi::string_arg(lua, b, usage)?;
            Ok(flag(pair_reaction(lua, &a, &b).is_some_and(|r| r >= 5)))
        })?,
    )?;

    // UnitSelectionColor(unit) (12340 `0x60da00`): `lua_isstring`-gated, then the ARGB dword
    // `0x521bf0` resolves for the unit's guid, pushed as r, g, b, a, each byte times 1/255
    // (`[0x9ead78]`). The app resolves the dword ([`UnitState::selection_color`]); a token naming
    // no fed unit reads the resolver's no-object answer, blue `0xFF0000FF` (`0x522093`).
    g.set(
        "UnitSelectionColor",
        lua.create_function(|lua, token: Value| {
            let token = crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitSelectionColor("unit")"#,
            )?;
            let argb = with_unit(lua, &Some(token), 0, |u| u.selection_color)?;
            let argb = if argb == 0 { 0xFF00_00FF } else { argb };
            let channel = |shift: u32| f64::from((argb >> shift) & 0xff) / 255.0;
            Ok((channel(16), channel(8), channel(0), channel(24)))
        })?,
    )?;

    // UnitIsCivilian(unit): killing the unit would be dishonorable, the predicate `0x612550`. It
    // shares `is_civilian_kill` with the tooltip's civilian line and `UnitPVPName`, so all agree.
    g.set(
        "UnitIsCivilian",
        lua.create_function(|lua, unit: Value| {
            let unit = Some(crate::script::binding_abi::string_arg(
                lua,
                unit,
                r#"Usage: UnitIsCivilian("unit")"#,
            )?);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let player_level = model.player_req.level;
            Ok(flag(
                unit.and_then(|u| model.unit(&u))
                    .is_some_and(|u| super::is_civilian_kill(u, player_level)),
            ))
        })?,
    )?;
    // UnitPlayerControlled(unit) (`0x516410`): `UNIT_FIELD_FLAGS` bit 0x8, vmangos
    // `UNIT_FLAG_PLAYER_CONTROLLED`, so a player's pet or a charmed creature answers 1 where
    // `UnitIsPlayer` answers nil. An unrecognised token answers nil here; the reference raises.
    g.set(
        "UnitPlayerControlled",
        lua.create_function(|lua, unit: Option<String>| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(
                unit.and_then(|u| model.unit(&u))
                    .is_some_and(|u| u.player_controlled),
            ))
        })?,
    )?;
    g.set(
        "UnitIsPlayer",
        lua.create_function(|lua, token: Option<String>| {
            unit_predicate(lua, &token, |u| u.is_player)
        })?,
    )?;
    // UnitIsPlusMob(unit) (`0x516d40`): `UNIT_FIELD_FLAGS` bit 0x40, never the creature rank
    // (`0x605620`). The server sets it for a non-pet above normal rank (`Creature.cpp:637`), so a
    // rare answers 1, and an uncached creature still answers from its flags.
    g.set(
        "UnitIsPlusMob",
        lua.create_function(|lua, token: Option<String>| {
            // No `lua_isstring` gate, so nil answers nil; an unrecognised token still raises in
            // the resolver (`0x515940`).
            unit_predicate(lua, &token, |u| u.flags & 0x40 != 0)
        })?,
    )?;

    // UnitIsUnit(a, b): both units exist and are the same token or share a nonzero GUID.
    g.set(
        "UnitIsUnit",
        lua.create_function(|lua, (a, b): (Value, Value)| {
            // Both arguments are `lua_isstring`-gated, so either one nil raises.
            let a = Some(crate::script::binding_abi::string_arg(
                lua,
                a,
                r#"Usage: UnitIsUnit("unit", "otherUnit")"#,
            )?);
            let b = Some(crate::script::binding_abi::string_arg(
                lua,
                b,
                r#"Usage: UnitIsUnit("unit", "otherUnit")"#,
            )?);
            // Both go through the resolver, so either one unrecognised raises.
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let (Some(a), Some(b)) = (a, b) else {
                return Ok(Value::Nil);
            };
            let (Some(ua), Some(ub)) = (model.unit(&a), model.unit(&b)) else {
                return Ok(Value::Nil);
            };
            if !ua.exists || !ub.exists {
                return Ok(Value::Nil);
            }
            Ok(flag(a == b || (ua.guid != 0 && ua.guid == ub.guid)))
        })?,
    )?;

    // UnitAffectingCombat(unit) (`0x517e10`): 1 in combat, else nil. No such unit takes the same
    // arm as a unit out of combat (`0x517e48`, `0x517e5c`), so it cannot probe existence. A missing
    // argument, or one neither string nor number, raises `Usage:` (`0x6f3510`, `0x6f4940`), unlike
    // `UnitInRaid`; a number reaches the resolver, which raises `Unknown unit name` (`0x515c14`).
    g.set(
        "UnitAffectingCombat",
        lua.create_function(|lua, token: Value| {
            let token = super::super::binding_abi::string_arg(
                lua,
                token,
                "Usage: UnitAffectingCombat(\"unit\")",
            )?;
            let hot = with_unit(lua, &Some(token), false, |u| u.exists && u.in_combat)?;
            Ok(flag(hot))
        })?,
    )?;

    // HasFullControl(): the flag `SMSG_CLIENT_CONTROL_UPDATE` writes for the local player
    // (`0xb4b3e4`, read at `0x51a158`), which the stock unit menu greys Trade and Duel on
    // (`UnitPopup.lua:474-477`, `:502-505`).
    g.set(
        "HasFullControl",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(model.player_control))
        })?,
    )?;

    // UnitPlayerOrPetInParty/InRaid(unit): the unit or its owner (`UnitState::owner`) is in the
    // group. The reference's predicate behind `0x5162f0`/`0x5163b0` is untraced; the owner reading
    // is benilla's.
    for (name, raid) in [
        ("UnitPlayerOrPetInParty", false),
        ("UnitPlayerOrPetInRaid", true),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, token: Option<String>| {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let Some(u) = token.as_ref().and_then(|t| model.unit(t)) else {
                    return Ok(Value::Nil);
                };
                if !u.exists || u.guid == 0 {
                    return Ok(Value::Nil);
                }
                let me = model.unit("player").map(|p| p.guid).unwrap_or(0);
                let in_group = |guid: u64| {
                    guid != 0
                        && if raid {
                            model.party.raid.iter().any(|m| m.guid == guid)
                        } else {
                            guid == me || model.party.members.iter().any(|m| m.guid == guid)
                        }
                };
                let grouped = if raid {
                    !model.party.raid.is_empty()
                } else {
                    !model.party.members.is_empty()
                };
                let hit = grouped && (in_group(u.guid) || in_group(u.owner));
                Ok(flag(hit))
            })?,
        )?;
    }

    // UnitInRaid(unit) (`0x516350`): the constant 1 on a hit (`0x51637e`), never an index. The
    // reference never raises `Usage:`: a missing or uncoercible argument is GUID 0 (`0x6f3690`,
    // `0x515970`), a miss (`0x4baee0`). The roster holds the player, unlike `UnitInParty`'s.
    g.set(
        "UnitInRaid",
        lua.create_function(|lua, token: Option<String>| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let hit = token
                .as_ref()
                .and_then(|t| model.unit(t))
                .filter(|u| u.exists && u.guid != 0)
                .is_some_and(|u| {
                    model
                        .party
                        .raid
                        .iter()
                        .any(|m| m.guid != 0 && m.guid == u.guid)
                });
            Ok(flag(hit))
        })?,
    )?;

    // UnitInBattleground(unit) (`0x60caa0`): the unit's 0-based raid roster row (`0x608130`), but
    // only on a battleground or arena map (`InstanceType` 3 or 4, `0x572f50`); nil otherwise. The
    // token is never checked: a missing or unknown one is GUID 0, a miss.
    g.set(
        "UnitInBattleground",
        lua.create_function(|lua, token: Option<String>| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            if !matches!(model.instance_type, Some(3 | 4)) {
                return Ok(Value::Nil);
            }
            let row = token
                .as_ref()
                .and_then(|t| model.unit(t))
                .filter(|u| u.exists && u.guid != 0)
                .and_then(|u| model.party.raid.iter().position(|m| m.guid == u.guid));
            Ok(row.map_or(Value::Nil, |i| Value::Number(i as f64)))
        })?,
    )?;

    // UnitInParty(unit): the grouped player or a party member, by token or by roster GUID.
    g.set(
        "UnitInParty",
        lua.create_function(|lua, token: Option<String>| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let Some(t) = token else {
                return Ok(Value::Nil);
            };
            let Some(u) = model.unit(&t) else {
                return Ok(Value::Nil);
            };
            let grouped = !model.party.members.is_empty();
            if !u.exists || !grouped {
                return Ok(Value::Nil);
            }
            // A `partyN` token itself names a member; a chain off one (`party1target`) names
            // whoever that member targets, which the guid arms below judge.
            let party_slot = matches!(
                parse_unit_token(&t),
                UnitTokenParse::Unit {
                    base: UnitBase::Party(_),
                    hops: 0
                }
            );
            let hit = party_slot
                || t.eq_ignore_ascii_case("player")
                || (u.guid != 0
                    && (model.unit("player").is_some_and(|p| p.guid == u.guid)
                        || model
                            .party
                            .members
                            .iter()
                            .any(|m| m.guid != 0 && m.guid == u.guid)));
            Ok(flag(hit))
        })?,
    )?;

    // UnitCanAssist("unit", "otherUnit") (`0x60d690` -> `0x7293d0`): the player toward the other
    // token, as `UnitCanAttack`
    // pairs them; the stock secure click asks it of the clicked unit (`SecureTemplates.lua:491`).
    g.set(
        "UnitCanAssist",
        lua.create_function(|lua, (a, b): (Value, Value)| {
            let usage = r#"Usage: UnitCanAssist("unit", "otherUnit")"#;
            let a = Some(crate::script::binding_abi::string_arg(lua, a, usage)?);
            let b = Some(crate::script::binding_abi::string_arg(lua, b, usage)?);
            let token = pick_unit_token(&a, &b);
            unit_predicate(lua, &token, |u| u.can_assist)
        })?,
    )?;

    // Deviation: UnitCanCooperate answers 1 for a friendly (5 and up) player, because the engine
    // carries no faction templates; the reference resolves their cooperation masks. The verdict
    // matches for the unit popup's invite and whisper gates.
    g.set(
        "UnitCanCooperate",
        lua.create_function(|lua, (a, b): (Value, Value)| {
            // Both arguments are `lua_isstring`-gated, so either one nil raises.
            let a = Some(crate::script::binding_abi::string_arg(
                lua,
                a,
                r#"Usage: UnitCanCooperate("unit", "otherUnit")"#,
            )?);
            let b = Some(crate::script::binding_abi::string_arg(
                lua,
                b,
                r#"Usage: UnitCanCooperate("unit", "otherUnit")"#,
            )?);
            let token = pick_unit_token(&a, &b);
            let ok = with_unit(lua, &token, false, |u| {
                u.exists && u.is_player && u.reaction >= 5
            })?;
            Ok(flag(ok))
        })?,
    )?;

    // GetRaidTargetIndex(unit): the mark, 1 to 8, or nil.
    g.set(
        "GetRaidTargetIndex",
        lua.create_function(|lua, token: Value| {
            // Gated (`0x4bb4be`, `0x4bb4cd`); the unquoted `unit` is the reference's spelling.
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                "Usage: GetRaidTargetIndex(unit)",
            )?);
            let idx = with_unit(lua, &token, 0u8, |u| u.raid_target)?;
            Ok(if idx > 0 {
                Value::Integer(i64::from(idx))
            } else {
                Value::Nil
            })
        })?,
    )?;

    // The party frame's status predicates; 1.12 has no unit AFK or DND predicate.
    g.set(
        "UnitIsConnected",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitIsConnected("unit")"#,
            )?);
            unit_predicate(lua, &token, |u| u.is_connected)
        })?,
    )?;
    // UnitIsAFK(unit) (`0x60cc30`) answers nil for a held player the player can attack, before
    // the flag (`0x60cccd`); UnitIsDND(unit) (`0x60cd50`) has no such gate.
    g.set(
        "UnitIsAFK",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitIsAFK("unit")"#,
            )?);
            unit_predicate(lua, &token, |u| u.afk && !u.can_attack)
        })?,
    )?;
    g.set(
        "UnitIsDND",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitIsDND("unit")"#,
            )?);
            unit_predicate(lua, &token, |u| u.dnd)
        })?,
    )?;
    // UnitGroupRolesAssigned(unit) (`0x60c810`): three booleans off the role bits, tank, healer,
    // damage (`0x60c869` on). No usage gate: a bad token is GUID 0, no role.
    g.set(
        "UnitGroupRolesAssigned",
        lua.create_function(|lua, token: Option<String>| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let roles = token
                .as_deref()
                .and_then(|t| model.unit(t))
                .map_or(0, |u| u.group_roles);
            Ok((roles & 0x2 != 0, roles & 0x4 != 0, roles & 0x8 != 0))
        })?,
    )?;
    g.set(
        "UnitIsPVP",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitIsPVP("unit")"#,
            )?);
            unit_predicate(lua, &token, |u| u.pvp)
        })?,
    )?;
    g.set(
        "UnitIsPVPFreeForAll",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitIsPVPFreeForAll("unit")"#,
            )?);
            unit_predicate(lua, &token, |u| u.is_pvp_ffa)
        })?,
    )?;
    // UnitFactionGroup(unit) → FactionGroup.dbc's `InternalName` (field 2), which stock FrameXML
    // builds texture paths from (`PlayerFrame.lua:68`), and its localized `Name0` (field 3); nil,
    // nil for a unit with no side or no snapshot.
    g.set(
        "UnitFactionGroup",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitFactionGroup("unit")"#,
            )?);
            let pair = with_unit(lua, &token, (None, None), |u| {
                (u.faction_group.clone(), u.faction_group_localized.clone())
            })?;
            match pair {
                (Some(english), localized) => {
                    let a = Value::String(lua.create_string(&english)?);
                    let b = match localized {
                        Some(l) if !l.is_empty() => Value::String(lua.create_string(&l)?),
                        _ => a.clone(),
                    };
                    Ok((a, b))
                }
                (None, _) => Ok((Value::Nil, Value::Nil)),
            }
        })?,
    )?;

    // UnitRace(unit) → localized, raceFile; UnitClass(unit) → localized, classFileName; nil, nil
    // for a unit without one.
    g.set(
        "UnitRace",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitRace("unit")"#,
            )?);
            // The `"player"` arm reads the character-select record's race byte (`0x518269`,
            // `0xc27e80`), never the unit resolver.
            if let Some(pair) = player_record_arm(lua, &token, |r| r.race.clone()) {
                return match pair {
                    Some((loc, file)) => Ok((
                        Value::String(lua.create_string(&loc)?),
                        Value::String(lua.create_string(&file)?),
                    )),
                    None => Ok((Value::Nil, Value::Nil)),
                };
            }
            let pair = with_unit(lua, &token, None, |u| {
                u.race.clone().zip(u.race_file.clone())
            })?;
            match pair {
                Some((loc, file)) => Ok((
                    Value::String(lua.create_string(&loc)?),
                    Value::String(lua.create_string(&file)?),
                )),
                None => Ok((Value::Nil, Value::Nil)),
            }
        })?,
    )?;
    // UnitClassBase(unit) (`0x610040`): `ChrClasses.dbc`'s name and file for the class byte,
    // the player's from the record (`0x610094`); the pair `UnitClass` answers.
    g.set(
        "UnitClassBase",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitClassBase("unit")"#,
            )?);
            class_pair(lua, &token)
        })?,
    )?;
    g.set(
        "UnitClass",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitClass("unit")"#,
            )?);
            class_pair(lua, &token)
        })?,
    )?;
    // UnitHasRelicSlot(unit) (`0x519e50`): a player unit's class has a relic slot, `ChrClasses.dbc`
    // field 16 (`ds:0xc0def4`), which the app resolves; no class id is compared in code. Field 16
    // is in the `patch.MPQ` copy only, set for Paladin, Shaman and Druid. No `"player"` fast path,
    // so any token answers, as stock `InspectPaperDollFrame.lua:123` needs.
    g.set(
        "UnitHasRelicSlot",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitHasRelicSlot("unit")"#,
            )?);
            let has = with_unit(lua, &token, false, |u| u.has_relic_slot)?;
            Ok(flag(has))
        })?,
    )?;

    // GetDamageBonusStat() (`0x48b520`): the active player's `ChrClasses.dbc` field 2 plus one,
    // the 1-based `UnitStat` index its melee damage scales with; 0 with no player or no class row
    // (`0x48b58a`). No stock caller.
    g.set(
        "GetDamageBonusStat",
        lua.create_function(|lua, ()| {
            let stat = with_unit(lua, &Some("player".to_string()), None, |u| {
                u.damage_bonus_stat
            })?;
            Ok(stat.map_or(0, |s| i64::from(s) + 1))
        })?,
    )?;

    // UnitSex(unit): 2 male, 3 female, or 1 neuter, which no feed sends. A unit that does not
    // resolve answers 2, never nil (`0x517f9f`).
    g.set(
        "UnitSex",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitSex("unit")"#,
            )?);
            // The `"player"` arm reads the record's byte (`0x517ef9`, `0xc27e82`) into
            // `{2, 3, 1, 6}` unchecked (`0x808be4`), so an unset record answers 2 there too.
            let sex = match player_record_arm(lua, &token, |r| r.sex) {
                Some(sex) => sex,
                None => with_unit(lua, &token, 0u8, |u| u.sex)?,
            };
            Ok(Value::Integer(i64::from(if sex == 0 { 2 } else { sex })))
        })?,
    )?;

    // UnitCreatureType(unit) (`0x51a280`): one string, nil for a token naming no unit (`0x51a2b8`)
    // and for a type of 0 (`0x51a2c3`). The type is the snapshot's, which the app resolved as the
    // reference's `0x605570` does: the shapeshift form's (`SpellShapeshiftForm.dbc` column 12, only
    // above 0), else the cached creature record's, else the race's (`ChrRaces.dbc` column 9). A
    // snapshot with no descriptor behind it carries none, as the reference's token resolver finds
    // no object for it. A missing argument, or one neither string nor number, raises `Usage:`
    // (`0x6f3510`, `0x6f4940`).
    g.set(
        "UnitCreatureType",
        lua.create_function(|lua, token: Value| {
            let token = match &token {
                Value::String(s) => Some(s.to_str()?.to_string()),
                // A number passes `lua_isstring`; here it answers nil, where the reference's
                // resolver raises `Unknown unit name` for it.
                Value::Number(_) | Value::Integer(_) => Some(String::new()),
                _ => return Err(mlua::Error::runtime("Usage: UnitCreatureType(\"unit\")")),
            };
            let word = with_unit(lua, &token, None, |u| u.creature_type_name.clone())?;
            match word {
                Some(w) => Ok(Value::String(lua.create_string(&w)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // UnitClassification(unit) (`0x516d90`): the rank's word, and "normal" for a token that
    // resolves to no unit, never nil.
    g.set(
        "UnitClassification",
        lua.create_function(|lua, token: Option<String>| {
            let rank = with_unit(lua, &token, 0u32, |u| u.rank)?;
            Ok(classification_word(rank).to_string())
        })?,
    )?;

    // UnitPowerType(unit) (`0x517940`): one number, the power index (0 mana, 1 rage, ...), with no
    // token string. A unit that does not resolve answers 0, never nil: `UnitFrame.lua:122` indexes
    // `ManaBarColor` with it.
    g.set(
        "UnitPowerType",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitPowerType("unit")"#,
            )?);
            // (type, token) as `0x60f100` pushes them: the token is `GetPowerString` (`0x7fe4b0`),
            // and a unit that resolves nothing answers 0 and "" (`0x60f32a`).
            let ty = with_unit(lua, &token, None, |u| Some(u.power_type))?;
            Ok(match ty {
                Some(ty) => (i64::from(ty), power_token(ty)),
                None => (0, ""),
            })
        })?,
    )?;

    // UnitPower(unit [, type]) (`0x60ed40`) and UnitPowerMax (`0x60ef40`); `UnitMana` and
    // `UnitManaMax` are the same two functions in 12340.
    for (names, usage, max) in [
        (
            ["UnitPower", "UnitMana"],
            r#"Usage: UnitPower("unit"[, type])"#,
            false,
        ),
        (
            ["UnitPowerMax", "UnitManaMax"],
            r#"Usage: UnitPowerMax("unit"[, type])"#,
            true,
        ),
    ] {
        let f = lua.create_function(move |lua, (token, ty): (Value, Value)| {
            unit_power(lua, token, ty, usage, max)
        })?;
        for name in names {
            g.set(name, f.clone())?;
        }
    }

    // UnitThreatSituation(unit [, mob]) (`0x613a60`): each token through the resolver, an unknown
    // one raising the usage line; then the status byte, `status - 1` when nonzero and nothing
    // otherwise. benilla keeps no threat table, so every status reads 0, as a unit with no
    // threat entry does in the reference (`0x52a1d7`).
    g.set(
        "UnitThreatSituation",
        lua.create_function(|lua, (unit, mob): (Value, Value)| {
            const USAGE: &str = r#"Usage: UnitThreatSituation("unit" [, "mob"])"#;
            let known = |v: &Value| -> bool {
                crate::script::binding_abi::optional_string(lua, v)
                    .is_some_and(|t| parse_unit_token(&t) != UnitTokenParse::Unknown)
            };
            if !known(&unit) {
                return Err(mlua::Error::runtime(USAGE));
            }
            // The mob goes through the resolver only when `lua_isstring` passes (`0x613aae`).
            if matches!(mob, Value::String(_) | Value::Number(_) | Value::Integer(_))
                && !known(&mob)
            {
                return Err(mlua::Error::runtime(USAGE));
            }
            Ok(mlua::MultiValue::new())
        })?,
    )?;

    // UnitDetailedThreatSituation(unit, mob) (`0x613b40`): both tokens through the resolver, the
    // mob too, so a missing or unknown one raises the usage line; then nothing for a mob not held
    // or a status of 0, which every status is without a threat table.
    g.set(
        "UnitDetailedThreatSituation",
        lua.create_function(|lua, (unit, mob): (Value, Value)| {
            const USAGE: &str = r#"Usage: UnitDetailedThreatSituation("unit" [, "mob"])"#;
            for v in [&unit, &mob] {
                let known = crate::script::binding_abi::optional_string(lua, v)
                    .is_some_and(|t| parse_unit_token(&t) != UnitTokenParse::Unknown);
                if !known {
                    return Err(mlua::Error::runtime(USAGE));
                }
            }
            Ok(mlua::MultiValue::new())
        })?,
    )?;

    // GetThreatStatusColor(status) (`0x511fe0`): `trunc(tonumber(status) + 1)` indexes the colour
    // table `0xad2d70`, entry 1 for anything outside `0..5` (`0x61a4f6`); each channel byte times
    // the single-precision `0.00392156` at `0x9ead78`, not quite `1/255`. A NaN status stores the
    // integer-indefinite pattern, past the table: entry 1.
    g.set(
        "GetThreatStatusColor",
        lua.create_function(|lua, status: Option<Value>| {
            const COLORS: [u32; 5] = [0xffffff, 0xb0b0b0, 0xffff77, 0xff9900, 0xff0000];
            const SCALE: f32 = 0.003_921_56;
            let status = crate::script::binding_abi::coerced_number(lua, status) + 1.0;
            let index = if status.is_finite() {
                usize::try_from(status as i64).unwrap_or(1)
            } else {
                1
            };
            let rgb = COLORS.get(index).copied().unwrap_or(COLORS[1]);
            let channel = |shift: u32| f64::from((rgb >> shift) & 0xff) * f64::from(SCALE);
            Ok((channel(16), channel(8), channel(0)))
        })?,
    )?;

    // UnitXP/UnitXPMax(unit): `PLAYER_XP` is a private field, so only `"player"` has a value and
    // any other token answers 0.
    let is_player = |token: &Option<String>| token.as_deref() == Some("player");
    g.set(
        "UnitXP",
        lua.create_function(move |lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitXP("unit")"#,
            )?);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(if is_player(&token) {
                i64::from(model.player_xp)
            } else {
                0
            })
        })?,
    )?;
    g.set(
        "UnitXPMax",
        lua.create_function(move |lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitXPMax("unit")"#,
            )?);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(if is_player(&token) {
                i64::from(model.player_next_level_xp)
            } else {
                0
            })
        })?,
    )?;

    // UnitIsPossessed(unit) (`0x60d860`): a held unit whose `UNIT_FIELD_FLAGS` carries
    // `UNIT_FLAG_POSSESSED` (`0x01000000`, byte 3 bit 0 at `0x60d8ad`).
    g.set(
        "UnitIsPossessed",
        lua.create_function(|lua, token: Option<String>| {
            unit_predicate(lua, &token, |u| u.has_object && u.flags & 0x0100_0000 != 0)
        })?,
    )?;

    // UnitIsCharmed(unit) (`0x516cf0`): `UNIT_FIELD_CHARMEDBY` is nonzero, not a flags bit, so the
    // charmed unit answers 1 and its charmer nil.
    g.set(
        "UnitIsCharmed",
        lua.create_function(|lua, token: Option<String>| {
            unit_predicate(lua, &token, |u| u.charmed)
        })?,
    )?;

    g.set(
        "GetMoney",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.money as i64)
        })?,
    )?;

    // GetComboPoints() (`0x51a190`): `PLAYER_FIELD_BYTES` byte 1, behind two gates stock
    // `ComboFrame.lua` does not repeat: the class is rogue or druid (`0x51a1cc`), so a warrior's
    // dodge point never shows, and `PLAYER_FIELD_COMBO_TARGET` equals the current target
    // (`0x51a205`), so the points read per target. Both are plain equality, GUID 0 included.
    g.set(
        "GetComboPoints",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let class = model.player_req.class_id;
            if class != CLASS_ROGUE && class != CLASS_DRUID {
                return Ok(0_i64);
            }
            let target = model.unit("target").map_or(0, |u| u.guid);
            if model.combo_target != target {
                return Ok(0_i64);
            }
            Ok(i64::from(model.combo_points))
        })?,
    )?;

    // GetRestState() (`0x51b8b0`) → the Exhaustion.dbc row's ID, name and factor, the records at
    // `[0xad3838]` indexed directly by `PLAYER_BYTES_2` byte 3; nil, nil, nil when there is no row.
    g.set(
        "GetRestState",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(match model.exhaustion.get(&model.rest_state) {
                Some((name, factor)) => (
                    Value::Number(f64::from(model.rest_state)),
                    Value::String(lua.create_string(name)?),
                    Value::Number(*factor),
                ),
                None => (Value::Nil, Value::Nil, Value::Nil),
            })
        })?,
    )?;
    // GetXPExhaustion() (`0x51b960`): the rested pool times the factor of Exhaustion.dbc row 1
    // whatever the state (2.0 shipped). Nil unless the rest byte is 1 (`0x51b9c7`), whatever the
    // pool holds, so a rested byte with an empty pool answers 0.
    g.set(
        "GetXPExhaustion",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(if model.rest_state != 1 {
                Value::Nil
            } else {
                let factor = model.exhaustion.get(&1).map_or(2.0, |(_, f)| *f);
                Value::Number(f64::from(model.rest_pool) * factor)
            })
        })?,
    )?;
    // IsResting() (`0x60dd40`): `PLAYER_FLAGS` bit 0x20, in a rest area.
    g.set(
        "IsResting",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(model.resting))
        })?,
    )?;
    // IsXPUserDisabled() (`0x614140`): `PLAYER_FLAGS` bit 25 as a boolean; nothing without a player.
    g.set(
        "IsXPUserDisabled",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            if !model.unit("player").is_some_and(|u| u.exists) {
                return Ok(MultiValue::new());
            }
            Ok(MultiValue::from_vec(vec![Value::Boolean(
                model.xp_user_disabled,
            )]))
        })?,
    )?;
    // PartialPlayTime()/NoPlayTime(): the play-time limit flags, `PLAYER_FLAGS` bits 0x1000
    // (`0x51cb30`) and 0x2000 (`0x51cba0`).
    g.set(
        "PartialPlayTime",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(model.partial_play_time))
        })?,
    )?;
    g.set(
        "NoPlayTime",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(model.no_play_time))
        })?,
    )?;
    // GetBillingTimeRested() (`0x511c40`): always one number, unconverted (`0x511c57`); stock
    // `PlayerFrame.lua:246` reads it as minutes. vmangos always sends 0 (`World.cpp:331`).
    g.set(
        "GetBillingTimeRested",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(Value::Number(f64::from(model.billing_time_rested)))
        })?,
    )?;
    // GetTimeToWellRested(): always nil; the reference's binding (`0x510b90`) only pushes nil.
    g.set(
        "GetTimeToWellRested",
        lua.create_function(|_, ()| Ok(Value::Nil))?,
    )?;

    // TargetUnit(unit): queue the token for the app to resolve and select; a token naming no unit
    // is a no-op, as in the reference.
    g.set(
        "TargetUnit",
        lua.create_function(|lua, token: Option<String>| {
            if let Some(token) = token {
                let mut model = crate::script::model_mut_of(lua);
                model
                    .script_calls
                    .push(ScriptCall::Select(SelectionRequest::Unit(token)));
            }
            Ok(())
        })?,
    )?;

    // AssistUnit(unit) (`0x489b80`): select the unit's `UNIT_FIELD_TARGET`. The shared tail
    // (`0x489bb2`-`0x489c07`) returns silently on 0 and selects through `0x489a40`, which leaves
    // the selection alone when nothing resolves; no `CanAssist` gate (`0x6066f0`), any unit, and a
    // swing only with `assistAttack` set (default "0", `0x48fc50`). A token that resolves nothing
    // is silent here, where the reference shows `0xb8` ERR_GENERIC_NO_TARGET (`0x489c0e`).
    g.set(
        "AssistUnit",
        lua.create_function(|lua, token: Option<String>| {
            if let Some(token) = token {
                let mut model = crate::script::model_mut_of(lua);
                model
                    .script_calls
                    .push(ScriptCall::Select(SelectionRequest::Assist(token)));
            }
            Ok(())
        })?,
    )?;

    // TargetLastEnemy(): re-select the last attackable target (`0x489b45` reads `0xb4e2e8`, which
    // `SetSelection` stamps at `0x49377d`), a no-op once it has streamed out. The app remembers it.
    g.set(
        "TargetLastEnemy",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model
                .script_calls
                .push(ScriptCall::Select(SelectionRequest::LastEnemy));
            Ok(())
        })?,
    )?;

    // TargetByName(name [, exactMatch]) (`0x489d60`, resolver `0x493aa0`, shared with `/target`): a
    // case-insensitive whole-name match wins, else without `exactMatch` the longest common prefix,
    // so "Rag" selects Ragnaros. Any unit (typemask 8), no dead, reaction, range or self gate. A
    // missing name, or one neither string nor number, raises `Usage:` (`0x489d69`, `0x6f4940`); a
    // number is taken as its string. Deviation: among whole-name matches ours picks the nearest,
    // because the reference's first-walked pick is order-dependent and reads as a bug. A miss is
    // silent here, where the reference shows `0x127` ERR_UNIT_NOT_FOUND, or `0xb8`
    // ERR_GENERIC_NO_TARGET for an empty name.
    g.set(
        "TargetByName",
        lua.create_function(|lua, (name, exact): (Value, Option<Value>)| {
            let name =
                super::super::binding_abi::string_arg(lua, name, "Usage: TargetByName(\"name\")")?;
            // Argument 2 comes from `0x6f1c10` with default 0: it never raises, and 0 is absent.
            let exact = match exact {
                None | Some(Value::Nil) | Some(Value::Boolean(false)) => false,
                Some(Value::Integer(n)) => n != 0,
                Some(Value::Number(n)) => n != 0.0,
                Some(_) => true,
            };
            let mut model = crate::script::model_mut_of(lua);
            model
                .script_calls
                .push(ScriptCall::TargetByName { name, exact });
            Ok(())
        })?,
    )?;

    // DropItemOnUnit(unit) (`0x48d960`): drop the cursor's item on a unit, feeding a pet or
    // opening a trade with a player. Only the token is queued; the app runs every gate.
    g.set(
        "DropItemOnUnit",
        lua.create_function(|lua, token: Option<String>| {
            if let Some(token) = token {
                let mut model = crate::script::model_mut_of(lua);
                model.drop_item_on_unit.push(token);
            }
            Ok(())
        })?,
    )?;

    // SpellTargetUnit(unit) (`0x6e6d90`): an argument that is not a string or a number raises
    // `Usage:`; while not targeting the call is a silent no-op, before the token is read; an
    // unknown token then raises `Unknown unit name`. The token queues for the host, which ends
    // targeting with "Out of range." when it names no unit and otherwise runs `BindTarget`
    // (`0x6e5b10` → `0x6e5b40`).
    g.set(
        "SpellTargetUnit",
        lua.create_function(|lua, token: Value| {
            let token = crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: SpellTargetUnit("unit")"#,
            )?;
            if !lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data")
                .spell_targeting
            {
                return Ok(());
            }
            let mut model = crate::script::model_mut_of(lua);
            model.script_calls.push(ScriptCall::SpellTargetUnit(token));
            Ok(())
        })?,
    )?;

    // ClearTarget() (`0x489ff0`): 1 when it cleared a target, nil when there was none, which
    // `ToggleGameMenu`'s Escape chain needs to fall through to the menu. The deselect is queued
    // either way: it reads the selection as the calls before it leave it (`0x489ff0`-`0x489fff`),
    // so `TargetUnit("player") ClearTarget()` ends with nothing selected.
    g.set(
        "ClearTarget",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            let had = model.unit("target").is_some_and(|u| u.exists);
            model.script_calls.push(ScriptCall::ClearTarget);
            Ok(flag(had))
        })?,
    )?;

    Ok(())
}

/// `GetPowerString` (`0x7fe4b0`): the power type's token, "" past the seven.
fn power_token(ty: u8) -> &'static str {
    const TOKENS: [&str; 7] = [
        "MANA",
        "RAGE",
        "FOCUS",
        "ENERGY",
        "HAPPINESS",
        "RUNES",
        "RUNIC_POWER",
    ];
    TOKENS.get(usize::from(ty)).copied().unwrap_or("")
}

/// `UnitClass`'s two returns, the display name and the uppercase token. The `"player"` arm reads
/// the record's class byte (`0x5183b9`, `0xc27e81`).
fn class_pair(lua: &Lua, token: &Option<String>) -> mlua::Result<(Value, Value)> {
    let pair = match player_record_arm(lua, token, |r| r.class.clone()) {
        Some(pair) => pair,
        None => with_unit(lua, token, None, |u| {
            u.class.clone().zip(u.class_file.clone())
        })?,
    };
    match pair {
        Some((loc, file)) => Ok((
            Value::String(lua.create_string(&loc)?),
            Value::String(lua.create_string(&file)?),
        )),
        None => Ok((Value::Nil, Value::Nil)),
    }
}
