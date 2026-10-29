//! The dungeon finder's Lua globals, the registrar's (`0xacd668`) but the raid browser's search
//! list (`SearchLFG*`, `RefreshLFGList`), plus `IsPartyLFG` (`0x52c310`). They push real Lua
//! booleans (`lua_pushboolean 0x84e4d0`), not the 1/nil of the older predicates.

use mlua::{Lua, MultiValue, Table, Value};

use super::cvar_array;
use super::engine::{class_roles, slot_of, Entry, LfgModel, FLAG_SEASONAL, TYPE_RANDOM};
use super::LfgRequest;
use crate::script::binding_abi::{bool_or_default, number_arg};
use crate::script::{Model, ModelCell};

/// The type token table (`0xacd644`), `GetLFGTypes`' seven `GlobalStrings` keys.
const TYPE_TOKENS: [&str; 7] = [
    "LFG_TYPE_NONE",
    "LFG_TYPE_DUNGEON",
    "LFG_TYPE_RAID",
    "LFG_TYPE_QUEST",
    "LFG_TYPE_ZONE",
    "LFG_TYPE_HEROIC_DUNGEON",
    "LFG_TYPE_RANDOM_DUNGEON",
];

/// `GetLFGRandomCooldownExpiration` and `GetLFGDeserterExpiration`'s auras.
const RANDOM_COOLDOWN_SPELL: u32 = 71328;
const DESERTER_SPELL: u32 = 71041;

/// The three CVars the selection lives in, registered per character (`0x51f25c`-`0x51f2aa`).
const ROLES_CVAR: &str = "lfgSelectedRoles";
const COLLAPSED_CVAR: &str = "lfdCollapsedHeaders";
const SELECTED_CVAR: &str = "lfdSelectedDungeons";

fn read<R>(lua: &Lua, f: impl FnOnce(&Model) -> R) -> R {
    let model = lua.app_data_ref::<ModelCell>().expect("model app_data");
    f(&model)
}

fn num(v: impl Into<f64>) -> Value {
    Value::Number(v.into())
}

fn text(lua: &Lua, s: &str) -> mlua::Result<Value> {
    Ok(Value::String(lua.create_string(s)?))
}

fn values(v: Vec<Value>) -> MultiValue {
    MultiValue::from_vec(v)
}

fn raise(msg: &str) -> mlua::Error {
    mlua::Error::RuntimeError(msg.into())
}

/// A registered CVar's value, `None` for one the host never registered (a null CVar pointer).
fn cvar(model: &Model, name: &str) -> Option<String> {
    model
        .cvars
        .get(&name.to_ascii_lowercase())
        .map(|c| c.value.clone())
}

fn roles_cvar(model: &Model) -> Option<u32> {
    cvar(model, ROLES_CVAR).map(|v| crate::script::cvars::sstr_to_int(&v) as u32)
}

/// A bit of an array CVar; false for an unregistered one.
fn array_bit(model: &Model, name: &str, index: i32) -> bool {
    cvar(model, name).is_some_and(|v| u32::try_from(index).is_ok_and(|i| cvar_array::get(&v, i)))
}

fn set_array_bit(lua: &Lua, name: &str, index: i32, on: bool) {
    let mut model = crate::script::model_mut_of(lua);
    let Some(value) = cvar(&model, name) else {
        return;
    };
    let Ok(index) = u32::try_from(index) else {
        return;
    };
    if let Some(next) = cvar_array::set(&value, index, on) {
        crate::script::cvars::set_from_engine(&mut model, name, next);
    }
}

/// The roles filtered by class (`0x553b50`).
fn filter_roles(lfg: &LfgModel, roles: u32) -> u32 {
    roles & u32::from(class_roles(lfg.class_id))
}

/// The optional table argument of the `GetLFDChoice*` getters: cleared and refilled, or a new one.
fn out_table(lua: &Lua, arg: Value, usage: &'static str) -> mlua::Result<Table> {
    match arg {
        Value::Table(t) => {
            let keys: Vec<Value> = t
                .pairs::<Value, Value>()
                .filter_map(|p| p.ok().map(|(k, _)| k))
                .collect();
            for k in keys {
                t.raw_set(k, Value::Nil)?;
            }
            Ok(t)
        }
        Value::Nil => lua.create_table(),
        _ => Err(raise(usage)),
    }
}

/// The dungeon info push (`0x5553f0`): fourteen values, or none for an id with no row.
fn dungeon_info(lua: &Lua, lfg: &LfgModel, id: i32) -> mlua::Result<Vec<Value>> {
    let Some(d) = lfg.catalog.dungeon(id) else {
        return Ok(Vec::new());
    };
    let exp = lfg.catalog.expansion(d.id, lfg.account_expansion);
    let size = lfg.catalog.map_difficulty(d.map_id, d.difficulty);
    Ok(vec![
        text(lua, &d.name)?,
        num(d.type_id),
        num(exp.map_or(d.min_level, |e| e.hard_level_min)),
        num(exp.map_or(d.max_level, |e| e.hard_level_max)),
        num(d.target_level),
        num(exp.map_or(d.target_level_min, |e| e.target_level_min)),
        num(exp.map_or(d.target_level_max, |e| e.target_level_max)),
        num(d.expansion_level),
        num(d.group_id),
        text(lua, &d.texture)?,
        num(d.difficulty),
        size.map_or(Value::Nil, |s| num(s.max_players)),
        text(lua, &d.description)?,
        Value::Boolean(d.flags & FLAG_SEASONAL != 0),
    ])
}

/// The choice info push (`0x554e60`): a header's name and type, or a dungeon's fourteen fields
/// with its group negated, into a fresh table.
fn choice_info(lua: &Lua, lfg: &LfgModel, id: i32) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    if id < 0 {
        if let Some(g) = lfg.catalog.group(-id) {
            t.raw_set(1, g.name.as_str())?;
            t.raw_set(2, g.type_id)?;
        }
        return Ok(t);
    }
    let mut v = dungeon_info(lua, lfg, id)?;
    if let Some(Value::Number(g)) = v.get_mut(8) {
        *g = -*g;
    }
    for (i, value) in v.into_iter().enumerate() {
        t.raw_set(i + 1, value)?;
    }
    Ok(t)
}

/// The LFD entries, then the raid browser's not already listed.
fn all_entries(lfg: &LfgModel) -> impl Iterator<Item = Entry> + '_ {
    lfg.lfd_entries.iter().copied().chain(
        lfg.lfr_entries
            .iter()
            .copied()
            .filter(|e| !lfg.lfd_entries.iter().any(|l| l.0 == e.0)),
    )
}

/// The encounter info push (`0x5538b0`): the `index`-th encounter of the map and difficulty.
fn encounter_info(
    lua: &Lua,
    lfg: &LfgModel,
    map: i32,
    difficulty: i32,
    index: i64,
    mask: u32,
) -> mlua::Result<MultiValue> {
    let found = usize::try_from(index).ok().and_then(|i| {
        lfg.catalog
            .view
            .encounters
            .iter()
            .filter(|e| e.map_id == map && e.difficulty == difficulty)
            .nth(i)
    });
    Ok(match found {
        Some(e) => values(vec![
            text(lua, &e.name)?,
            match &e.icon {
                Some(icon) => text(lua, icon)?,
                None => Value::Nil,
            },
            Value::Boolean(mask & 1u32.wrapping_shl(e.bit as u32) != 0),
        ]),
        None => values(vec![Value::Nil; 3]),
    })
}

fn role_token(roles: u32) -> &'static str {
    if roles & 0x2 != 0 {
        "TANK"
    } else if roles & 0x4 != 0 {
        "HEALER"
    } else if roles & 0x8 != 0 {
        "DAMAGER"
    } else {
        "UNKNOWN"
    }
}

/// An aura check for a unit token: nothing for nobody or a unit with no aura list.
fn unit_has_aura(model: &Model, token: &str, spell: u32) -> Option<bool> {
    model.unit_guids.guid_of(token).filter(|&g| g != 0)?;
    crate::script::aura::auras_of(model, token).map(|list| list.iter().any(|a| a.spell_id == spell))
}

fn player_aura_end(model: &Model, spell: u32) -> MultiValue {
    match model.player_auras.iter().find(|a| a.spell_id == spell) {
        Some(a) => values(vec![num(a.expiration_time)]),
        None => MultiValue::new(),
    }
}

/// A reward item's name, asking for its template on a miss; the arrival fires
/// `LFG_UPDATE_RANDOM_INFO` (`0x552950`).
fn reward_name(lua: &Lua, item: u32) -> String {
    let mut model = crate::script::model_mut_of(lua);
    if let Some(t) = model.item_templates.get(&item) {
        return t.name.clone();
    }
    model.item_stat_asks.insert(item);
    model.lfg.reward_item_waits.insert(item);
    String::new()
}

/// `JoinLFG` (`0x5572d0`): nothing while joined and unchanged, past the throttle or with no
/// role but leader.
fn join(lua: &Lua) {
    let mut model = crate::script::model_mut_of(lua);
    let Some(selected) = roles_cvar(&model) else {
        return;
    };
    let now = model.now;
    let lfg = &mut model.lfg;
    if lfg.local.joined && !lfg.local.dirty {
        return;
    }
    if !lfg.throttles.join.allow(now, 10.0) {
        return;
    }
    let roles = filter_roles(lfg, selected);
    if roles & !0x1 == 0 {
        return;
    }
    let request = LfgRequest::Join {
        roles,
        no_partial_clear: lfg.local.no_partial_clear,
        achievements: lfg.local.achievements,
        slots: lfg.local.dungeons.clone(),
        needs: lfg.local.needs,
        comment: lfg.local.comment.clone(),
    };
    lfg.local.joined = true;
    lfg.local.dirty = false;
    lfg.requests.push(request);
}

fn ui_error(lua: &Lua, key: &'static str) {
    crate::script::model_mut_of(lua).ui_errors.push(key);
}

fn fire(lua: &Lua, event: &str) {
    crate::script::tick::fire_event_into(lua, event, Vec::new());
}

/// Register the dungeon finder globals.
pub(in crate::script) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // SetLFGDungeon(id) (`0x559f50`): the row's slot into the local list.
    g.set(
        "SetLFGDungeon",
        lua.create_function(|lua, id: Value| {
            let id = number_arg(lua, id, "Usage: SetLFGDungeon(type)")?;
            let mut model = crate::script::model_mut_of(lua);
            if let Some(slot) = model.lfg.catalog.dungeon(id).map(slot_of) {
                model.lfg.set_dungeon(slot);
            }
            Ok(())
        })?,
    )?;

    // ClearLFGDungeon(type, id) (`0x559500`): raises only when neither is a number; a
    // non-number reads 0.
    g.set(
        "ClearLFGDungeon",
        lua.create_function(|lua, (ty, id): (Value, Value)| {
            let (ty, id) = (lua.coerce_number(ty)?, lua.coerce_number(id)?);
            if ty.is_none() && id.is_none() {
                return Err(raise("Usage: ClearLFGDungeon(type, id)"));
            }
            let (ty, id) = (
                ty.unwrap_or(0.0) as i64 as u32,
                id.unwrap_or(0.0) as i64 as i32,
            );
            let mut model = crate::script::model_mut_of(lua);
            if ty < 7 && model.lfg.catalog.dungeon(id).is_some() {
                model.lfg.clear_dungeon(ty << 24 | (id as u32 & 0xff_ffff));
            }
            Ok(())
        })?,
    )?;

    g.set(
        "ClearAllLFGDungeons",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua).lfg.clear_dungeon_list();
            Ok(())
        })?,
    )?;

    // GetLFGInfoLocal() (`0x552e90`): joined, the two options, comment, count, the three needs.
    g.set(
        "GetLFGInfoLocal",
        lua.create_function(|lua, ()| {
            read(lua, |m| {
                let l = &m.lfg.local;
                Ok(values(vec![
                    Value::Boolean(l.joined),
                    Value::Boolean(l.no_partial_clear),
                    Value::Boolean(l.achievements),
                    text(lua, &l.comment)?,
                    num(l.dungeons.len() as u32),
                    num(l.needs[0]),
                    num(l.needs[1]),
                    num(l.needs[2]),
                ]))
            })
        })?,
    )?;

    // GetLFGInfoServer() (`0x552f30`): in party, then the server info's fields.
    g.set(
        "GetLFGInfoServer",
        lua.create_function(|lua, ()| {
            read(lua, |m| {
                let grouped = m.lfg.party.grouped();
                let i = m.lfg.server_info();
                Ok(values(vec![
                    Value::Boolean(grouped),
                    Value::Boolean(i.joined),
                    Value::Boolean(i.queued),
                    Value::Boolean(i.no_partial_clear),
                    Value::Boolean(i.achievements),
                    text(lua, &i.comment)?,
                    num(i.dungeons.len() as u32),
                    num(i.needs[0]),
                    num(i.needs[1]),
                    num(i.needs[2]),
                ]))
            })
        })?,
    )?;

    // GetLFGQueuedList([table]) (`0x557520`): `[id] = true` for each queued dungeon.
    g.set(
        "GetLFGQueuedList",
        lua.create_function(|lua, arg: Value| {
            let t = out_table(lua, arg, "Usage: GetLFDQueuedList([table])")?;
            let ids: Vec<u32> = read(lua, |m| {
                m.lfg
                    .server_info()
                    .dungeons
                    .iter()
                    .map(|s| s & 0xff_ffff)
                    .collect()
            });
            for id in ids {
                t.raw_set(id, true)?;
            }
            Ok(t)
        })?,
    )?;

    // SetLFGComment(text) (`0x553f50` → `0x5539a0`): kept locally; sent while joined, two per
    // two seconds. Deviation: the reference scrubs it to 64 characters first; this cuts it at 64.
    g.set(
        "SetLFGComment",
        lua.create_function(|lua, comment: Value| {
            let comment: String = crate::script::binding_abi::optional_string(lua, &comment)
                .unwrap_or_default()
                .chars()
                .take(64)
                .collect();
            let mut model = crate::script::model_mut_of(lua);
            let now = model.now;
            let lfg = &mut model.lfg;
            lfg.local.comment = comment.clone();
            if !lfg.local.joined {
                lfg.local.dirty = true;
                return Ok(());
            }
            if lfg.throttles.comment.allow(now, 2.0) {
                lfg.requests.push(LfgRequest::SetComment(comment));
            }
            Ok(())
        })?,
    )?;

    // LFGTeleport([out]) (`0x556bc0`): the combat and death refusals, then the send; true when
    // sent. Deviation: the falling, transport and fatigue refusals are left to the server, which
    // answers them with `SMSG_LFG_TELEPORT_DENIED`.
    g.set(
        "LFGTeleport",
        lua.create_function(|lua, out: MultiValue| {
            let out = bool_or_default(out.front(), false);
            let refusal = {
                let mut model = crate::script::model_mut_of(lua);
                let Some(player) = model.unit("player").filter(|u| u.exists).cloned() else {
                    return Ok(MultiValue::new());
                };
                let now = model.now;
                if !model.lfg.throttles.teleport.allow(now, 10.0) {
                    return Ok(values(vec![Value::Boolean(false)]));
                }
                if player.in_combat {
                    Some("ERR_NOT_IN_COMBAT")
                } else if player.dead || player.ghost {
                    Some("ERR_PLAYER_DEAD")
                } else {
                    model.lfg.requests.push(LfgRequest::Teleport(out));
                    None
                }
            };
            if let Some(key) = refusal {
                ui_error(lua, key);
            }
            Ok(values(vec![Value::Boolean(refusal.is_none())]))
        })?,
    )?;

    // JoinLFG() (`0x557660`): no dungeon or no role but leader refuses with a line.
    g.set(
        "JoinLFG",
        lua.create_function(|lua, ()| {
            let refusal = read(lua, |m| {
                if m.lfg.local.dungeons.is_empty() {
                    Some("ERR_LFG_NO_SLOTS_SELECTED")
                } else if roles_cvar(m).is_none_or(|r| r & !0x1 == 0) {
                    Some("ERR_LFG_NO_ROLES_SELECTED")
                } else {
                    None
                }
            });
            match refusal {
                Some(key) => ui_error(lua, key),
                None => join(lua),
            }
            Ok(())
        })?,
    )?;

    // LeaveLFG() (`0x553ab0`): sent while the local or the server info is joined.
    g.set(
        "LeaveLFG",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            let lfg = &mut model.lfg;
            if !lfg.local.joined && !lfg.server_info().joined {
                return Ok(());
            }
            lfg.local.joined = false;
            lfg.local.dirty = false;
            lfg.requests.push(LfgRequest::Leave);
            Ok(())
        })?,
    )?;

    // GetLFGTypes() (`0x552dd0`): the seven type names.
    g.set(
        "GetLFGTypes",
        lua.create_function(|lua, ()| {
            let mut v = Vec::with_capacity(TYPE_TOKENS.len());
            for key in TYPE_TOKENS {
                let s = crate::strings::global(lua, key).unwrap_or_else(|| key.to_string());
                v.push(text(lua, &s)?);
            }
            Ok(values(v))
        })?,
    )?;

    // GetLFGRoleUpdate() (`0x554610`): in progress, slot count, member count.
    g.set(
        "GetLFGRoleUpdate",
        lua.create_function(|lua, ()| {
            read(lua, |m| {
                let l = &m.lfg;
                Ok(values(vec![
                    Value::Boolean(l.role_check_state == 2),
                    num(l.role_check_slots.len() as u32),
                    num(l.role_check_members.len() as u32),
                ]))
            })
        })?,
    )?;

    // GetLFGRoleUpdateSlot(index) (`0x554690`): the slot's type and id.
    g.set(
        "GetLFGRoleUpdateSlot",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetLFGRoleUpdateSlot(index)")?;
            read(lua, |m| {
                let Some(&slot) = usize::try_from(index - 1)
                    .ok()
                    .and_then(|i| m.lfg.role_check_slots.get(i))
                else {
                    return Ok(MultiValue::new());
                };
                Ok(values(vec![num(slot >> 24), num(slot & 0xff_ffff)]))
            })
        })?,
    )?;

    // GetLFGRoleUpdateMember(index) (`0x554750`): confirmed, level, name, class, then the four
    // role bits once confirmed.
    g.set(
        "GetLFGRoleUpdateMember",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetLFGRoleUpdateMember(index)")?;
            read(lua, |m| {
                let Some(member) = usize::try_from(index - 1)
                    .ok()
                    .and_then(|i| m.lfg.role_check_members.get(i))
                else {
                    return Ok(MultiValue::new());
                };
                let mut v = vec![Value::Boolean(member.confirmed), num(member.level)];
                match m.lfg.names.get(&member.guid) {
                    Some(n) => {
                        v.push(text(lua, &n.name)?);
                        v.push(text(lua, &n.class_name)?);
                    }
                    None => v.extend([Value::Nil, Value::Nil]),
                }
                for bit in [0x1, 0x2, 0x4, 0x8] {
                    v.push(if member.confirmed {
                        Value::Boolean(member.roles & bit != 0)
                    } else {
                        Value::Nil
                    });
                }
                Ok(values(v))
            })
        })?,
    )?;

    // GetLFGRoles() (`0x552e10`): leader, tank, healer, damage from `lfgSelectedRoles`.
    g.set(
        "GetLFGRoles",
        lua.create_function(|lua, ()| {
            Ok(read(lua, |m| match roles_cvar(m) {
                Some(r) => values(
                    [0x1, 0x2, 0x4, 0x8]
                        .map(|b| Value::Boolean(r & b != 0))
                        .to_vec(),
                ),
                None => values(vec![Value::Nil; 4]),
            }))
        })?,
    )?;

    // SetLFGRoles(leader, tank, healer, damage) (`0x553d60`): the class's roles kept, then
    // `LFG_ROLE_UPDATE`.
    g.set(
        "SetLFGRoles",
        lua.create_function(|lua, args: MultiValue| {
            let arg = |i: usize| bool_or_default(args.get(i), false);
            let mut roles = u32::from(arg(0));
            for (i, bit) in [(1, 0x2), (2, 0x4), (3, 0x8)] {
                if arg(i) {
                    roles |= bit;
                }
            }
            {
                let mut model = crate::script::model_mut_of(lua);
                let roles = filter_roles(&model.lfg, roles);
                crate::script::cvars::set_from_engine(&mut model, ROLES_CVAR, roles.to_string());
            }
            fire(lua, "LFG_ROLE_UPDATE");
            Ok(())
        })?,
    )?;

    // CompleteLFGRoleCheck(accept) (`0x553e20`): the class-filtered roles, or 0 to decline;
    // accepting with no role refuses with a line.
    g.set(
        "CompleteLFGRoleCheck",
        lua.create_function(|lua, args: MultiValue| {
            let accept = bool_or_default(args.front(), false);
            let mut model = crate::script::model_mut_of(lua);
            let now = model.now;
            if !model.lfg.throttles.role_check.allow(now, 10.0) {
                return Ok(MultiValue::new());
            }
            let Some(selected) = roles_cvar(&model) else {
                return Ok(MultiValue::new());
            };
            let mut roles = 0;
            if accept {
                roles = filter_roles(&model.lfg, selected) as u8;
                if roles & 0xe == 0 {
                    model.ui_errors.push("ERR_LFG_NO_ROLES_SELECTED");
                    return Ok(values(vec![Value::Boolean(false)]));
                }
            }
            model.lfg.requests.push(LfgRequest::SetRoles(roles));
            Ok(values(vec![Value::Boolean(true)]))
        })?,
    )?;

    // GetLFGProposal() (`0x554170`): twelve values for a proposal on a known dungeon.
    g.set(
        "GetLFGProposal",
        lua.create_function(|lua, ()| {
            read(lua, |m| {
                let l = &m.lfg;
                let p = &l.proposal;
                if p.slot == 0 {
                    return Ok(MultiValue::new());
                }
                let Some(d) = l.catalog.dungeon(p.slot as i32 & 0xff_ffff) else {
                    return Ok(MultiValue::new());
                };
                let me = p
                    .members
                    .iter()
                    .find(|m| m.is_local)
                    .copied()
                    .unwrap_or_default();
                let (total, done) = if d.type_id == TYPE_RANDOM {
                    (0, u32::from(p.completed_mask != 0))
                } else if d.flags & FLAG_SEASONAL != 0 {
                    (1, u32::from(p.completed_mask != 0))
                } else {
                    l.catalog
                        .encounter_counts(d.map_id, d.difficulty, p.completed_mask)
                };
                Ok(values(vec![
                    Value::Boolean(true),
                    num(p.slot >> 24),
                    num(p.slot & 0xff_ffff),
                    text(lua, &d.name)?,
                    text(lua, &d.texture)?,
                    text(lua, role_token(me.roles))?,
                    Value::Boolean(me.answered),
                    num(total),
                    num(done),
                    num(5),
                    Value::Boolean(me.roles & 0x1 != 0),
                    Value::Boolean(d.flags & FLAG_SEASONAL != 0),
                ]))
            })
        })?,
    )?;

    // GetLFGProposalMember(index) (`0x553000`): leader, role, level 0, responded, accepted.
    g.set(
        "GetLFGProposalMember",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetLFGProposalMember(index)")?;
            read(lua, |m| {
                let p = &m.lfg.proposal;
                let i = (index as u32).wrapping_sub(1);
                if p.slot == 0 || i > 4 {
                    return Ok(MultiValue::new());
                }
                let member = p.members[i as usize];
                Ok(values(vec![
                    Value::Boolean(member.roles & 0x1 != 0),
                    text(lua, role_token(member.roles))?,
                    num(0),
                    Value::Boolean(member.answered),
                    Value::Boolean(member.accepted),
                    Value::Nil,
                    Value::Nil,
                ]))
            })
        })?,
    )?;

    // GetLFGProposalEncounter(index) (`0x554380`).
    g.set(
        "GetLFGProposalEncounter",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetLFGProposalEncounter(index)")?;
            read(lua, |m| {
                let l = &m.lfg;
                let p = &l.proposal;
                match (p.slot != 0)
                    .then(|| l.catalog.dungeon(p.slot as i32 & 0xff_ffff))
                    .flatten()
                {
                    Some(d) => encounter_info(
                        lua,
                        l,
                        d.map_id,
                        d.difficulty,
                        i64::from(index) - 1,
                        p.completed_mask,
                    ),
                    None => encounter_info(lua, l, 0, 0, 0, 0),
                }
            })
        })?,
    )?;

    // AcceptProposal()/RejectProposal() (`0x5545e0`/`0x554600` → `0x554450`): an accept marks
    // our row answered, a reject drops the proposal; both repaint. `CanPerformAction(9)` only
    // stops tainted code, which benilla does not track.
    for (name, accept) in [("AcceptProposal", true), ("RejectProposal", false)] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                let fired = {
                    let mut model = crate::script::model_mut_of(lua);
                    let now = model.now;
                    let lfg = &mut model.lfg;
                    if lfg.proposal.slot == 0 || !lfg.throttles.proposal.allow(now, 10.0) {
                        false
                    } else {
                        lfg.requests.push(LfgRequest::ProposalResult {
                            id: lfg.proposal.id,
                            accept,
                        });
                        if accept {
                            for m in lfg.proposal.members.iter_mut().filter(|m| m.is_local) {
                                m.answered = true;
                                m.accepted = true;
                            }
                        } else {
                            lfg.proposal = Default::default();
                        }
                        true
                    }
                };
                if fired {
                    fire(lua, "LFG_PROPOSAL_UPDATE");
                }
                Ok(())
            })?,
        )?;
    }

    // GetLFGBootProposal() (`0x554a60`): in progress, then voted, my vote, target, the counts
    // and the reason.
    g.set(
        "GetLFGBootProposal",
        lua.create_function(|lua, ()| {
            read(lua, |m| {
                let b = &m.lfg.boot;
                if !b.in_progress {
                    return Ok(values(vec![Value::Boolean(false)]));
                }
                let target = match m.lfg.names.get(&b.target) {
                    Some(n) if n.realm.is_empty() => text(lua, &n.name)?,
                    Some(n) => text(lua, &format!("{}-{}", n.name, n.realm))?,
                    None => Value::Nil,
                };
                Ok(values(vec![
                    Value::Boolean(true),
                    Value::Boolean(b.did_vote),
                    Value::Boolean(b.my_vote),
                    target,
                    num(b.total_votes),
                    num(b.boot_votes),
                    num(b.time_left),
                    text(lua, &b.reason)?,
                ]))
            })
        })?,
    )?;

    // SetLFGBootVote(vote) (`0x554960`).
    g.set(
        "SetLFGBootVote",
        lua.create_function(|lua, args: MultiValue| {
            let vote = bool_or_default(args.front(), false);
            let mut model = crate::script::model_mut_of(lua);
            let now = model.now;
            let lfg = &mut model.lfg;
            if lfg.boot.in_progress && lfg.throttles.boot_vote.allow(now, 10.0) {
                lfg.boot.my_vote = vote;
                lfg.requests.push(LfgRequest::BootVote(vote));
            }
            Ok(())
        })?,
    )?;

    // GetLFGQueueStats() (`0x553f90`): nothing unless queued with a status; then the needs, the
    // dungeon, the waits (`[0xbeaa30..3c]`, then `[0xbeaa2c]`) and the queue start.
    g.set(
        "GetLFGQueueStats",
        lua.create_function(|lua, ()| {
            read(lua, |m| {
                let l = &m.lfg;
                let info = l.server_info();
                let q = &l.queue;
                if !info.joined || !info.queued || q.lfg_id == 0 {
                    return Ok(MultiValue::new());
                }
                let mut v = vec![
                    Value::Boolean(true),
                    num(0),
                    num(q.tanks_needed),
                    num(q.healers_needed),
                    num(q.damage_needed),
                ];
                match l.catalog.dungeon(q.lfg_id as i32) {
                    Some(d) => v.extend([
                        num(d.type_id),
                        text(lua, &d.name)?,
                        num(q.average_wait),
                        num(q.tank_wait),
                        num(q.healer_wait),
                        num(q.damage_wait),
                        num(q.my_wait),
                        num(q.queued_at),
                    ]),
                    None => v.extend(std::iter::repeat_n(Value::Nil, 8)),
                }
                Ok(values(v))
            })
        })?,
    )?;

    // GetLastQueueStatusIndex() (`0x559ff0`): the last status slot's place in the queued list.
    // The last queue slot (`[0xbeaa0c]`) has no writer in 12340, so it stays 0 and this answers nil
    // unless slot 0 is queued.
    g.set(
        "GetLastQueueStatusIndex",
        lua.create_function(|lua, ()| {
            Ok(read(lua, |m| {
                m.lfg
                    .server_info()
                    .dungeons
                    .iter()
                    .position(|&s| s == 0)
                    .map_or(Value::Nil, |i| num(i as u32 + 1))
            }))
        })?,
    )?;

    // GetLFGDungeonInfo(id) (`0x555590`).
    g.set(
        "GetLFGDungeonInfo",
        lua.create_function(|lua, id: Value| {
            let id = number_arg(lua, id, "Usage: GetLFGDungeonInfo(dungeonID)")?;
            Ok(values(read(lua, |m| dungeon_info(lua, &m.lfg, id))?))
        })?,
    )?;

    // GetLFGRandomDungeonInfo(index) (`0x5555d0`): the id, then its fourteen fields.
    g.set(
        "GetLFGRandomDungeonInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetLFGRandomDungeonInfo(index)")?;
            read(lua, |m| {
                let Some(&slot) = (index as u32)
                    .checked_sub(1)
                    .and_then(|i| m.lfg.random_dungeons.get(i as usize))
                else {
                    return Ok(MultiValue::new());
                };
                let id = (slot & 0xff_ffff) as i32;
                let mut v = vec![num(id)];
                v.extend(dungeon_info(lua, &m.lfg, id)?);
                Ok(values(v))
            })
        })?,
    )?;

    // IsLFGDungeonJoinable(id) (`0x55a040`).
    g.set(
        "IsLFGDungeonJoinable",
        lua.create_function(|lua, id: Value| {
            let id = number_arg(lua, id, "Usage: IsLFGDungeonJoinable(dungeonID)")?;
            Ok(read(lua, |m| match m.lfg.catalog.dungeon(id) {
                Some(d) => values(vec![Value::Boolean(m.lfg.slot_valid_for_all(slot_of(d)))]),
                None => MultiValue::new(),
            }))
        })?,
    )?;

    // SetLFGHeaderCollapsed(headerID, collapsed) (`0x556ad0`): a negated group id.
    g.set(
        "SetLFGHeaderCollapsed",
        lua.create_function(|lua, args: MultiValue| {
            let header = number_arg(
                lua,
                args.front().cloned().unwrap_or(Value::Nil),
                "Usage: SetLFGHeaderCollapsed(headerID, isCollapsed)",
            )?;
            let collapsed = bool_or_default(args.get(1), false);
            let known = header < 0 && read(lua, |m| m.lfg.catalog.group(-header).is_some());
            if known {
                set_array_bit(lua, COLLAPSED_CVAR, -header, collapsed);
            }
            Ok(())
        })?,
    )?;

    // SetLFGDungeonEnabled(dungeonID[, enabled]) (`0x556b60`): enabled by default.
    g.set(
        "SetLFGDungeonEnabled",
        lua.create_function(|lua, args: MultiValue| {
            let id = number_arg(
                lua,
                args.front().cloned().unwrap_or(Value::Nil),
                "Usage: SetLFGDungeonEnabled(dungeonID, isEnabled)",
            )?;
            let enabled = bool_or_default(args.get(1), true);
            if id > 0 {
                set_array_bit(lua, SELECTED_CVAR, id, enabled);
            }
            Ok(())
        })?,
    )?;

    // GetLFGCompletionReward() (`0x557e40`): the finished dungeon and its reward totals.
    g.set(
        "GetLFGCompletionReward",
        lua.create_function(|lua, ()| {
            read(lua, |m| {
                let r = &m.lfg.reward;
                if r.actual_slot == 0 {
                    return Ok(MultiValue::new());
                }
                let Some(d) = m.lfg.catalog.dungeon(r.actual_slot as i32 & 0xff_ffff) else {
                    return Ok(MultiValue::new());
                };
                Ok(values(vec![
                    text(lua, &d.name)?,
                    num(d.type_id),
                    text(lua, &d.texture)?,
                    num(r.base_money),
                    num(r.variable_money),
                    num(r.base_xp),
                    num(r.variable_xp),
                    num(r.strangers),
                    num(r.items.len() as u32),
                ]))
            })
        })?,
    )?;

    // GetLFGCompletionRewardItem(index) (`0x557f70`): texture and count.
    g.set(
        "GetLFGCompletionRewardItem",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetLFGCompletionRewardItem(index)")?;
            read(lua, |m| {
                let Some(item) = usize::try_from(index - 1)
                    .ok()
                    .and_then(|i| m.lfg.reward.items.get(i))
                else {
                    return Ok(MultiValue::new());
                };
                let icon = m
                    .lfg
                    .display_icons
                    .get(&item.display)
                    .cloned()
                    .unwrap_or_default();
                Ok(values(vec![text(lua, &icon)?, num(item.count)]))
            })
        })?,
    )?;

    // IsInLFGDungeon() (`0x555660`): the group's LFG dungeon is the map and difficulty we are in.
    g.set(
        "IsInLFGDungeon",
        lua.create_function(|lua, ()| {
            Ok(read(lua, |m| {
                let l = &m.lfg;
                if l.party.lfg_slot == 0 {
                    return MultiValue::new();
                }
                match l.catalog.dungeon(l.party.lfg_slot as i32 & 0xff_ffff) {
                    Some(d) => values(vec![Value::Boolean(
                        d.map_id == l.world.0 && d.difficulty == l.world.1 as i32,
                    )]),
                    None => MultiValue::new(),
                }
            }))
        })?,
    )?;

    // IsPartyLFG() (`0x52c310`): 1 or nil, the group's LFG slot.
    g.set(
        "IsPartyLFG",
        lua.create_function(|lua, ()| {
            Ok(read(lua, |m| {
                crate::script::binding_abi::flag(m.lfg.party.lfg_slot != 0)
            }))
        })?,
    )?;

    // CanPartyLFGBackfill() (`0x553170`): an LFG party of two to four, not finished.
    g.set(
        "CanPartyLFGBackfill",
        lua.create_function(|lua, ()| Ok(read(lua, |m| Value::Boolean(can_backfill(&m.lfg)))))?,
    )?;

    // GetPartyLFGBackfillInfo() (`0x5556d0`): the dungeon's name, id and type.
    g.set(
        "GetPartyLFGBackfillInfo",
        lua.create_function(|lua, ()| {
            read(lua, |m| {
                let l = &m.lfg;
                if !can_backfill(l) {
                    return Ok(MultiValue::new());
                }
                let Some(d) = l.catalog.dungeon(l.party.lfg_slot as i32 & 0xff_ffff) else {
                    return Ok(MultiValue::new());
                };
                Ok(values(vec![text(lua, &d.name)?, num(d.id), num(d.type_id)]))
            })
        })?,
    )?;

    // PartyLFGStartBackfill() (`0x55a0c0`): the group's dungeon as the only choice, then a join.
    g.set(
        "PartyLFGStartBackfill",
        lua.create_function(|lua, ()| {
            let has_role = read(lua, |m| roles_cvar(m).is_some_and(|r| r & !0x1 != 0));
            if !has_role {
                ui_error(lua, "ERR_LFG_NO_ROLES_SELECTED");
                return Ok(MultiValue::new());
            }
            {
                let mut model = crate::script::model_mut_of(lua);
                if !can_backfill(&model.lfg) {
                    return Ok(MultiValue::new());
                }
                let slot = model.lfg.party.lfg_slot;
                model.lfg.clear_dungeon_list();
                model.lfg.set_dungeon(slot);
            }
            join(lua);
            Ok(values(vec![Value::Boolean(true)]))
        })?,
    )?;

    // GetLFGRandomCooldownExpiration()/GetLFGDeserterExpiration() (`0x558060`/`0x5580e0`): the
    // aura's expiry on the `GetTime()` clock.
    g.set(
        "GetLFGRandomCooldownExpiration",
        lua.create_function(|lua, ()| {
            Ok(read(lua, |m| player_aura_end(m, RANDOM_COOLDOWN_SPELL)))
        })?,
    )?;
    g.set(
        "GetLFGDeserterExpiration",
        lua.create_function(|lua, ()| Ok(read(lua, |m| player_aura_end(m, DESERTER_SPELL))))?,
    )?;

    // UnitHasLFGRandomCooldown(unit)/UnitHasLFGDeserter(unit) (`0x555760`/`0x555840`).
    for (name, spell, usage) in [
        (
            "UnitHasLFGRandomCooldown",
            RANDOM_COOLDOWN_SPELL,
            "Usage: UnitHasLFGRandomCooldown(\"unit\")",
        ),
        (
            "UnitHasLFGDeserter",
            DESERTER_SPELL,
            "Usage: UnitHasLFGDeserter(\"unit\")",
        ),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, unit: Value| {
                let Some(token) = crate::script::binding_abi::optional_string(lua, &unit) else {
                    return Err(raise(usage));
                };
                Ok(read(lua, |m| match unit_has_aura(m, &token, spell) {
                    Some(b) => values(vec![Value::Boolean(b)]),
                    None => MultiValue::new(),
                }))
            })?,
        )?;
    }

    // GetAvailableRoles() (`0x5548f0`): the class's tank, healer and damage bits.
    g.set(
        "GetAvailableRoles",
        lua.create_function(|lua, ()| {
            let roles = read(lua, |m| class_roles(m.lfg.class_id));
            Ok(values(
                [0x2, 0x4, 0x8]
                    .map(|b| Value::Boolean(roles & b != 0))
                    .to_vec(),
            ))
        })?,
    )?;

    // GetLFDChoiceOrder([table]) / GetLFRChoiceOrder([table]) (`0x554be0`/`0x554d20`): the
    // sorted ids, 1-based.
    for (name, usage, raid) in [
        (
            "GetLFDChoiceOrder",
            "Usage: GetLFDChoiceOrder([table])",
            false,
        ),
        (
            "GetLFRChoiceOrder",
            "Usage: GetLFRChoiceOrder([table])",
            true,
        ),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, arg: Value| {
                let t = out_table(lua, arg, usage)?;
                let ids: Vec<i32> = read(lua, |m| {
                    let list = if raid {
                        &m.lfg.lfr_entries
                    } else {
                        &m.lfg.lfd_entries
                    };
                    list.iter().map(|e| e.0).collect()
                });
                for (i, id) in ids.into_iter().enumerate() {
                    t.raw_set(i + 1, id)?;
                }
                Ok(t)
            })?,
        )?;
    }

    // GetLFDChoiceInfo([table]) (`0x5551e0`): `[id] = info` for every LFD entry, then the raid
    // browser's not already in.
    g.set(
        "GetLFDChoiceInfo",
        lua.create_function(|lua, arg: Value| {
            let t = out_table(lua, arg, "Usage: GetLFDChoiceInfo([table])")?;
            read(lua, |m| -> mlua::Result<()> {
                for (id, _) in all_entries(&m.lfg) {
                    t.raw_set(id, choice_info(lua, &m.lfg, id)?)?;
                }
                Ok(())
            })?;
            Ok(t)
        })?,
    )?;

    // GetLFDChoiceCollapseState([table]) (`0x5564d0`): a dungeon reads its group's bit, a
    // header its own, of `lfdCollapsedHeaders`.
    g.set(
        "GetLFDChoiceCollapseState",
        lua.create_function(|lua, arg: Value| {
            let t = out_table(lua, arg, "Usage: GetLFDChoiceCollapseState([table])")?;
            let rows: Vec<(i32, bool)> = read(lua, |m| {
                all_entries(&m.lfg)
                    .map(|(id, _)| {
                        let header = if id >= 0 {
                            m.lfg.catalog.dungeon(id).map_or(0, |d| d.group_id)
                        } else {
                            -id
                        };
                        (id, array_bit(m, COLLAPSED_CVAR, header))
                    })
                    .collect()
            });
            for (id, collapsed) in rows {
                t.raw_set(id, collapsed)?;
            }
            Ok(t)
        })?,
    )?;

    // GetLFDChoiceEnabledState([table]) (`0x5567e0`): `[id] = true` for each selected dungeon.
    g.set(
        "GetLFDChoiceEnabledState",
        lua.create_function(|lua, arg: Value| {
            let t = out_table(lua, arg, "Usage: GetLFDChoiceEnabledState([table])")?;
            let ids: Vec<i32> = read(lua, |m| {
                all_entries(&m.lfg)
                    .map(|e| e.0)
                    .filter(|&id| id > 0 && array_bit(m, SELECTED_CVAR, id))
                    .collect()
            });
            for id in ids {
                t.raw_set(id, true)?;
            }
            Ok(t)
        })?,
    )?;

    // GetLFDChoiceLockedState([table]) (`0x5576b0`): `[id] = reason`, the party's locks but on
    // raids first, then ours over them.
    g.set(
        "GetLFDChoiceLockedState",
        lua.create_function(|lua, arg: Value| {
            let t = out_table(lua, arg, "Usage: GetLFDChoiceLockedState([table])")?;
            let rows: Vec<(u32, u32)> = read(lua, |m| {
                let l = &m.lfg;
                let mut rows = Vec::new();
                if l.party_lock_valid {
                    for member in &l.party_lock {
                        rows.extend(
                            member
                                .dungeons
                                .iter()
                                .filter(|e| e.slot >> 24 != 2 && e.locked)
                                .map(|e| (e.slot & 0xff_ffff, e.lock_reason)),
                        );
                    }
                }
                if l.player_lock.valid {
                    rows.extend(
                        l.player_lock
                            .dungeons
                            .iter()
                            .filter(|e| e.locked)
                            .map(|e| (e.slot & 0xff_ffff, e.lock_reason)),
                    );
                }
                rows
            });
            for (id, reason) in rows {
                t.raw_set(id, reason)?;
            }
            Ok(t)
        })?,
    )?;

    // GetLFDLockPlayerCount() (`0x557890`): 0 without our locks, 1 without the party's.
    g.set(
        "GetLFDLockPlayerCount",
        lua.create_function(|lua, ()| {
            Ok(read(lua, |m| {
                let l = &m.lfg;
                if !l.player_lock.valid {
                    0
                } else if !l.party_lock_valid {
                    1
                } else {
                    l.party_lock.len() + 1
                }
            }))
        })?,
    )?;

    // GetLFDLockInfo(dungeonID, playerIndex) (`0x557930`): the player's name and the lock
    // reason, 0 for none; index 1 is us, then the party in list order.
    g.set(
        "GetLFDLockInfo",
        lua.create_function(|lua, (id, index): (Value, Value)| {
            let usage = "Usage: GetLFDLockInfo(dungeonID, playerIndex)";
            let id = number_arg(lua, id, usage)?;
            let index = number_arg(lua, index, usage)?;
            read(lua, |m| {
                let l = &m.lfg;
                if id < 0 {
                    return Ok(MultiValue::new());
                }
                let info = if index == 1 {
                    l.player_lock.valid.then_some(&l.player_lock)
                } else if !l.party_lock_valid {
                    None
                } else {
                    usize::try_from(index - 2)
                        .ok()
                        .and_then(|i| l.party_lock.get(i))
                };
                let Some(info) = info else {
                    return Ok(MultiValue::new());
                };
                let name = match l.names.get(&info.guid) {
                    Some(n) => n.name.clone(),
                    None => crate::strings::global(lua, "UNKNOWN").unwrap_or_default(),
                };
                let reason = info
                    .dungeons
                    .iter()
                    .find(|e| (e.slot & 0xff_ffff) as i32 == id)
                    .map_or(0, |e| if e.locked { e.lock_reason } else { 0 });
                Ok(values(vec![text(lua, &name)?, num(reason)]))
            })
        })?,
    )?;

    // RequestLFDPlayerLockInfo()/RequestLFDPartyLockInfo() (`0x5569d0`/`0x556a50`): three per
    // ten seconds; whether it went.
    for (name, party) in [
        ("RequestLFDPlayerLockInfo", false),
        ("RequestLFDPartyLockInfo", true),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                let mut model = crate::script::model_mut_of(lua);
                let now = model.now;
                let lfg = &mut model.lfg;
                let throttle = if party {
                    &mut lfg.throttles.party_lock
                } else {
                    &mut lfg.throttles.player_lock
                };
                if !throttle.allow(now, 10.0) {
                    return Ok(false);
                }
                lfg.requests.push(if party {
                    LfgRequest::PartyLockInfo
                } else {
                    LfgRequest::PlayerLockInfo
                });
                Ok(true)
            })?,
        )?;
    }

    g.set(
        "GetNumRandomDungeons",
        lua.create_function(|lua, ()| Ok(read(lua, |m| m.lfg.random_dungeons.len())))?,
    )?;

    // GetLFGDungeonRewards(dungeonID) (`0x557aa0`): done, base money, variable money, base and
    // variable experience, item count; zeros for a dungeon the server listed no rewards for.
    g.set(
        "GetLFGDungeonRewards",
        lua.create_function(|lua, id: Value| {
            let id = number_arg(lua, id, "Usage: GetLFGDungeonRewards(dungeonID)")?;
            Ok(read(lua, |m| {
                let l = &m.lfg;
                let Some(d) = l.catalog.dungeon(id) else {
                    return MultiValue::new();
                };
                let entry = l
                    .player_lock
                    .valid
                    .then(|| l.player_lock.find(slot_of(d)))
                    .flatten();
                match entry {
                    Some(e) => values(vec![
                        Value::Boolean(e.reward_done),
                        num(e.base_money),
                        num(e.variable_money),
                        num(e.base_xp),
                        num(e.variable_xp),
                        num(e.rewards.len() as u32),
                    ]),
                    None => values(vec![
                        Value::Boolean(false),
                        num(0),
                        num(0),
                        num(0),
                        num(0),
                        num(0),
                    ]),
                }
            }))
        })?,
    )?;

    // GetLFGDungeonRewardInfo(dungeonID, index) (`0x557be0`): a random or seasonal dungeon's
    // item: name, texture, count.
    g.set(
        "GetLFGDungeonRewardInfo",
        lua.create_function(|lua, (id, index): (Value, Value)| {
            let usage = "Usage: GetLFGDungeonRewardInfo(dungeonID, lootIndex)";
            let id = number_arg(lua, id, usage)?;
            let index = number_arg(lua, index, usage)?;
            let found = read(lua, |m| {
                let l = &m.lfg;
                let d = l.catalog.dungeon(id)?;
                if d.type_id != TYPE_RANDOM && d.flags & FLAG_SEASONAL == 0 {
                    return None;
                }
                let item = l.reward_item(slot_of(d), i64::from(index) - 1)?;
                Some((
                    item,
                    l.display_icons
                        .get(&item.display)
                        .cloned()
                        .unwrap_or_default(),
                ))
            });
            let Some((item, icon)) = found else {
                return Ok(MultiValue::new());
            };
            let name = reward_name(lua, item.item);
            Ok(values(vec![
                text(lua, &name)?,
                text(lua, &icon)?,
                num(item.count),
            ]))
        })?,
    )?;

    // GetLFGDungeonRewardLink(dungeonID, index) (`0x557d60`): the item's link once its template
    // is cached; the slot is always the random type's.
    g.set(
        "GetLFGDungeonRewardLink",
        lua.create_function(|lua, (id, index): (Value, Value)| {
            let usage = "Usage: GetLFGDungeonRewardLink(dungeonID, lootIndex)";
            let id = number_arg(lua, id, usage)?;
            let index = number_arg(lua, index, usage)?;
            let slot = id as u32 & 0xff_ffff | 0x0600_0000;
            let Some(item) = read(lua, |m| m.lfg.reward_item(slot, i64::from(index) - 1)) else {
                return Ok(MultiValue::new());
            };
            let link = {
                let mut model = crate::script::model_mut_of(lua);
                match model.item_templates.get(&item.item) {
                    Some(t) => Some(crate::script::item_stats::item_link(
                        item.item, &t.name, t.quality,
                    )),
                    None => {
                        model.item_stat_asks.insert(item.item);
                        None
                    }
                }
            };
            Ok(match link {
                Some(link) => values(vec![text(lua, &link)?]),
                None => MultiValue::new(),
            })
        })?,
    )?;

    // GetRandomDungeonBestChoice() (`0x553120`): the best random dungeon's id.
    g.set(
        "GetRandomDungeonBestChoice",
        lua.create_function(|lua, ()| {
            Ok(read(lua, |m| match m.lfg.best_choice {
                0 => MultiValue::new(),
                s => values(vec![num(s & 0xff_ffff)]),
            }))
        })?,
    )?;

    // IsListedInLFR() (`0x5586d0`): the server info holds a raid.
    g.set(
        "IsListedInLFR",
        lua.create_function(|lua, ()| {
            Ok(read(lua, |m| {
                let l = &m.lfg;
                let info = l.server_info();
                info.joined
                    && info.dungeons.iter().any(|&s| {
                        l.catalog
                            .dungeon(s as i32 & 0xff_ffff)
                            .is_some_and(|d| d.type_id == 2)
                    })
            }))
        })?,
    )?;

    Ok(())
}

/// `CanPartyLFGBackfill`'s test (`0x553170`): one to three others, an LFG slot, not finished.
fn can_backfill(lfg: &LfgModel) -> bool {
    let p = &lfg.party;
    p.real_members != 0 && p.real_members < 4 && p.lfg_slot != 0 && p.lfg_flags & 0x2 == 0
}
