//! The Lua side of instances: `IsInInstance`, `GetInstanceInfo`, the reset and lock-warning verbs,
//! the difficulty setters and `IsThreatWarningEnabled`, which keys on the map's instance type. The
//! readers answer off state the app pushes from `Map.dbc` and the instance packets; the lockout
//! lines arrive as `CHAT_MSG_SYSTEM`.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::number_arg;

/// The instance lock clock as `GetInstanceLockInfo` (`0x513b90`) reads it: all zero once the
/// deadline has passed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InstanceLock {
    /// The deadline has not passed: what the encounter verb tests.
    pub running: bool,
    /// The time left in whole seconds, the reference's unsigned millisecond quotient.
    pub secs: u32,
    /// `DungeonEncounter.dbc` `Bit`s done.
    pub completed_mask: u32,
    pub extending: bool,
}

/// `IsInInstance()`'s type word for a `Map.dbc` `InstanceType`: the reference's table, and `"none"`
/// from 4 up.
pub fn instance_type_name(instance_type: u32) -> &'static str {
    match instance_type {
        1 => "party",
        2 => "raid",
        3 => "pvp",
        _ => "none",
    }
}

impl super::UiScript {
    /// Push the current map's `Map.dbc` `InstanceType`, `None` for a map with no row. Deviation:
    /// `IsInInstance()` then answers `nil, "none"`, because the reference returns two results
    /// without pushing either, a stack bug.
    pub fn set_instance_type(&mut self, instance_type: Option<u32>) {
        if self.model_ref().instance_type == instance_type {
            return;
        }
        let mut model = self.model_mut();
        model.instance_type = instance_type;
    }

    /// Push the current map's `MapName_lang` and whether its difficulty is dynamic (`Flags &
    /// 0x100`), `GetInstanceInfo`'s first and seventh returns; `None` for a map with no row.
    pub fn set_map_info(&mut self, info: Option<(String, bool)>) {
        if self.model_ref().map_info == info {
            return;
        }
        self.model_mut().map_info = info;
    }

    /// Push `CanShowResetInstances()`'s answer; the app owns its terms (the ownership latch, the
    /// last dungeon, its age and the current map).
    pub fn set_can_reset_instances(&mut self, can: bool) {
        if self.model_ref().can_reset_instances == can {
            return;
        }
        self.model_mut().can_reset_instances = can;
    }

    /// `ResetInstances()` calls since the last drain, each a `CMSG_RESET_INSTANCES`.
    pub fn take_reset_instance_asks(&mut self) -> u32 {
        drain_field!(self, reset_instance_asks)
    }

    /// Push the instance lock clock, as it reads this frame.
    pub fn set_instance_lock(&mut self, lock: InstanceLock) {
        set_field!(self, instance_lock, lock);
    }

    /// `RespondInstanceLock(accept)` answers since the last drain, each a
    /// `CMSG_INSTANCE_LOCK_RESPONSE`.
    pub fn take_instance_lock_responses(&mut self) -> Vec<bool> {
        drain_field!(self, instance_lock_responses)
    }

    /// Push the dynamic-difficulty maps (`Map.dbc` `Flags & 0x100`), once per catalog.
    pub fn set_dynamic_difficulty_maps(&mut self, maps: Vec<i32>) {
        set_field!(self, dynamic_difficulty_maps, maps);
    }
}

/// The flag `0x519df0` recomputes off the `threatWarning` CVar, the map and the group, read on
/// demand: mode 0 never, 1 a dungeon or raid map (`InstanceType` 1 or 2, `0x519e4b`), 2 in a party
/// or raid (`0x519e08`), any other value always; the CVar is read as `atoi` reads it.
pub(crate) fn threat_warning_enabled(model: &super::Model) -> bool {
    let mode = model
        .cvars
        .get("threatwarning")
        .map_or(3, |c| atoi(&c.value));
    match mode {
        0 => false,
        1 => matches!(model.instance_type, Some(1 | 2)),
        2 => {
            let g = &model.unit_guids;
            g.party.iter().any(|&p| p != 0) || g.raid.iter().any(|&r| r != 0)
        }
        _ => true,
    }
}

/// C's `atoi`, which `CVar` keeps as its integer: leading blanks, an optional sign, then digits up
/// to the first other byte; nothing parsed is 0.
fn atoi(s: &str) -> i64 {
    let s = s.trim_start();
    let (neg, rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let n = rest
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i64, |n, d| {
            n.saturating_mul(10).saturating_add(i64::from(d - b'0'))
        });
    if neg {
        -n
    } else {
        n
    }
}

/// The model, read-only.
fn model_of(lua: &Lua) -> mlua::AppDataRef<'_, super::ModelCell> {
    lua.app_data_ref::<super::ModelCell>()
        .expect("model app_data")
}

/// Register the instance globals and `IsThreatWarningEnabled`.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `IsInInstance()`: 1 if `InstanceType` is nonzero, else nil; then the type word.
    g.set(
        "IsInInstance",
        lua.create_function(|lua, ()| {
            let ty = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.instance_type
            };
            let name = instance_type_name(ty.unwrap_or(0));
            let inside = match ty {
                Some(t) if t != 0 => Value::Number(1.0),
                _ => Value::Nil,
            };
            Ok(MultiValue::from_vec(vec![
                inside,
                Value::String(lua.create_string(name)?),
            ]))
        })?,
    )?;

    // `CanShowResetInstances()`: 1 or nil, the gate on the SELF menu's reset row.
    g.set(
        "CanShowResetInstances",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(if model.can_reset_instances {
                Value::Number(1.0)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // `IsThreatWarningEnabled()` (`0x517d30`): 1 or nil off [`threat_warning_enabled`].
    g.set(
        "IsThreatWarningEnabled",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(super::binding_abi::flag(threat_warning_enabled(&model)))
        })?,
    )?;

    // `GetInstanceInfo()` (`0x51a8c0`): name, type word (`0xac8074`, "none" from 5 up),
    // `SMSG_INSTANCE_DIFFICULTY`'s difficulty + 1, its `MapDifficulty` row's name and size ("" and
    // 0 with no row), the dynamic difficulty, and whether the map's difficulty is dynamic.
    // Deviation: with no map row the reference pushes six values for its seven, a stack bug,
    // where this answers the six and a nil.
    g.set(
        "GetInstanceInfo",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let ty = model.instance_type.unwrap_or(0);
            let word = ["none", "party", "raid", "pvp", "arena"]
                .get(ty as usize)
                .copied()
                .unwrap_or("none");
            let (name, dynamic) = match &model.map_info {
                Some((name, dynamic)) => (
                    Value::String(lua.create_string(name)?),
                    Value::Boolean(*dynamic),
                ),
                None => (Value::Nil, Value::Nil),
            };
            let (map, difficulty, dynamic_difficulty) = model.lfg.world;
            let (size_name, size) = match model.lfg.catalog.map_difficulty(map, difficulty as i32) {
                Some(row) => (
                    crate::strings::global(lua, &row.difficulty_string)
                        .unwrap_or_else(|| row.difficulty_string.clone()),
                    row.max_players,
                ),
                None => (String::new(), 0),
            };
            Ok(MultiValue::from_vec(vec![
                name,
                Value::String(lua.create_string(word)?),
                Value::Number(f64::from(difficulty) + 1.0),
                Value::String(lua.create_string(&size_name)?),
                Value::Number(f64::from(size)),
                Value::Number(f64::from(dynamic_difficulty)),
                dynamic,
            ]))
        })?,
    )?;

    // `ResetInstances()`: the `CONFIRM_RESET_INSTANCES` dialog's Yes.
    g.set(
        "ResetInstances",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.reset_instance_asks += 1;
            Ok(())
        })?,
    )?;

    // `GetInstanceDifficulty()` (`0x515750`): `SMSG_INSTANCE_DIFFICULTY`'s difficulty + 1.
    g.set(
        "GetInstanceDifficulty",
        lua.create_function(|lua, ()| {
            let model = model_of(lua);
            Ok(f64::from(model.lfg.world.1) + 1.0)
        })?,
    )?;

    // `CanMapChangeDifficulty([mapId])` (`0x517d70`): 1 when the map, else the current one, has
    // `Map.dbc` flag `0x100`, else nil.
    g.set(
        "CanMapChangeDifficulty",
        lua.create_function(|lua, map: Value| {
            let model = model_of(lua);
            let map = match lua.coerce_number(map)? {
                Some(n) => super::binding_abi::ftol(n),
                None => model.lfg.world.0,
            };
            Ok(super::binding_abi::flag(
                model.dynamic_difficulty_maps.contains(&map),
            ))
        })?,
    )?;

    // `GetInstanceLockTimeRemaining()` (`0x516340`): whole seconds left, extending, and the
    // current map's encounter total and completed count at the instance difficulty (`0x553830`).
    g.set(
        "GetInstanceLockTimeRemaining",
        lua.create_function(|lua, ()| {
            let model = model_of(lua);
            let lock = model.instance_lock;
            let (map, difficulty, _) = model.lfg.world;
            let (total, done) =
                model
                    .lfg
                    .catalog
                    .encounter_counts(map, difficulty as i32, lock.completed_mask);
            Ok(MultiValue::from_vec(vec![
                Value::Number(f64::from(lock.secs)),
                Value::Boolean(lock.extending),
                Value::Number(f64::from(total)),
                Value::Number(f64::from(done)),
            ]))
        })?,
    )?;

    // `GetInstanceLockTimeRemainingEncounter(index)` (`0x516410`): the current map's index-th
    // encounter at the instance difficulty while the clock runs, else the first of map 0's
    // difficulty 0, which no row holds.
    g.set(
        "GetInstanceLockTimeRemainingEncounter",
        lua.create_function(|lua, index: Value| {
            let index = match lua.coerce_number(index)? {
                Some(n) => n,
                None => {
                    return Err(mlua::Error::RuntimeError(
                        "Usage: GetInstanceLockTimeRemainingEncounter(encounterIndex)".into(),
                    ))
                }
            };
            let model = model_of(lua);
            let lock = model.instance_lock;
            let (map, difficulty, _) = model.lfg.world;
            if lock.running {
                super::lfg::encounter_info(
                    lua,
                    &model.lfg,
                    map,
                    difficulty as i32,
                    i64::from(super::binding_abi::ftol(index - 1.0)),
                    lock.completed_mask,
                )
            } else {
                super::lfg::encounter_info(lua, &model.lfg, 0, 0, 0, 0)
            }
        })?,
    )?;

    // `RespondInstanceLock(accept)` (`0x613ed0`): the reference's boolean coercion, then
    // `CMSG_INSTANCE_LOCK_RESPONSE`.
    g.set(
        "RespondInstanceLock",
        lua.create_function(|lua, args: MultiValue| {
            let accept = super::binding_abi::bool_or_default(args.front(), false);
            crate::script::model_mut_of(lua)
                .instance_lock_responses
                .push(accept);
            Ok(())
        })?,
    )?;

    // `SetDungeonDifficulty(n)` (`0x526050`) and `SetRaidDifficulty(n)` (`0x5261a0`): 1-based,
    // past the range (1-2 dungeon, 1-4 raid) nothing; in a group, only its leader, else
    // `ERR_NOT_LEADER`. The app applies the choice and sends it.
    for (binding, usage, max, raid) in [
        (
            "SetDungeonDifficulty",
            "Usage: SetDungeonDifficulty(difficulty)",
            1u32,
            false,
        ),
        (
            "SetRaidDifficulty",
            "Usage: SetRaidDifficulty(difficulty)",
            3,
            true,
        ),
    ] {
        g.set(
            binding,
            lua.create_function(move |lua, n: Value| {
                // `(uint)(n - 1.0)`, so a 0 wraps past the range.
                let difficulty = (number_arg(lua, n, usage)? as u32).wrapping_sub(1);
                if difficulty > max {
                    return Ok(());
                }
                let mut model = crate::script::model_mut_of(lua);
                let party = &model.party;
                let grouped = !party.members.is_empty() || !party.raid.is_empty();
                if grouped && party.leader_guid != party.own_guid {
                    model.ui_errors.push("ERR_NOT_LEADER");
                    return Ok(());
                }
                model.party_requests.push(if raid {
                    super::PartyRequest::SetRaidDifficulty(difficulty)
                } else {
                    super::PartyRequest::SetDungeonDifficulty(difficulty)
                });
                Ok(())
            })?,
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{UiScript, UnitGuids};

    /// `0x519df0`'s four modes, off the `threatWarning` CVar, the map type and the group.
    #[test]
    fn the_threat_warning_follows_its_mode() {
        let mut s = UiScript::new().unwrap();
        s.register_cvars([("threatWarning", "3")]);
        let on = |s: &mut UiScript| {
            s.eval::<bool>("return IsThreatWarningEnabled() == 1")
                .unwrap()
        };
        assert!(on(&mut s), "3, the default, is always");

        s.set_cvar_host("threatWarning", "0");
        assert!(!on(&mut s), "0 is never");

        s.set_cvar_host("threatWarning", "1");
        s.set_instance_type(Some(0));
        assert!(!on(&mut s), "1 outside a dungeon");
        s.set_instance_type(Some(1));
        assert!(on(&mut s), "1 in a dungeon");
        s.set_instance_type(Some(3));
        assert!(!on(&mut s), "a battleground is neither dungeon nor raid");

        s.set_cvar_host("threatWarning", "2");
        assert!(!on(&mut s), "2 alone");
        s.set_unit_guids(&UnitGuids {
            player: 1,
            party: [2, 0, 0, 0],
            ..Default::default()
        });
        assert!(on(&mut s), "2 in a party");

        s.set_cvar_host("threatWarning", "7");
        assert!(on(&mut s), "any other value is always");
    }

    /// `0x51a8c0`'s seven returns off the pushed map; no row answers the six and a nil.
    #[test]
    fn get_instance_info_reads_the_current_map() {
        let mut s = UiScript::new().unwrap();
        let info = |s: &mut UiScript| {
            s.eval::<String>(
                "local n, t, d, dn, m, dd, dyn = GetInstanceInfo()                  return table.concat({tostring(n), t, d, '[' .. dn .. ']', m, dd, tostring(dyn)}, ',')",
            )
            .unwrap()
        };
        assert_eq!(info(&mut s), "nil,none,1,[],0,0,nil");
        s.set_instance_type(Some(2));
        s.set_map_info(Some(("Icecrown Citadel".into(), true)));
        assert_eq!(info(&mut s), "Icecrown Citadel,raid,1,[],0,0,true");
        s.set_instance_type(Some(0));
        s.set_map_info(Some(("Eastern Kingdoms".into(), false)));
        assert_eq!(info(&mut s), "Eastern Kingdoms,none,1,[],0,0,false");
    }

    /// The lock verbs (`0x516340`, `0x516410`) over the pushed clock and the current map's
    /// encounters at the instance difficulty.
    #[test]
    fn the_lock_verbs_read_the_clock_and_the_encounters() {
        use crate::script::{EncounterView, InstanceLock, LfgCatalogView};
        let encounter = |bit, name: &str| EncounterView {
            map_id: 631,
            difficulty: 1,
            bit,
            name: name.into(),
            icon: None,
        };
        let mut s = UiScript::new().unwrap();
        s.set_lfg_catalog(LfgCatalogView {
            encounters: vec![
                encounter(0, "Lord Marrowgar"),
                encounter(1, "Lady Deathwhisper"),
                encounter(2, "Gunship Battle"),
            ],
            ..Default::default()
        });
        s.set_lfg_world(631, 1, 0);
        let remaining = |s: &mut UiScript| {
            s.eval::<String>(
                "local t, e, n, d = GetInstanceLockTimeRemaining() \
                 return table.concat({t, tostring(e), n, d}, ',')",
            )
            .unwrap()
        };
        assert_eq!(remaining(&mut s), "0,false,3,0", "no clock: nothing done");
        assert!(
            s.eval::<bool>("return GetInstanceLockTimeRemainingEncounter(1) == nil")
                .unwrap(),
            "no clock: map 0's first encounter, which none is"
        );
        s.set_instance_lock(InstanceLock {
            running: true,
            secs: 59,
            completed_mask: 0b101,
            extending: true,
        });
        assert_eq!(remaining(&mut s), "59,true,3,2");
        assert_eq!(
            s.eval::<String>(
                "local n, i, done = GetInstanceLockTimeRemainingEncounter(2) \
                 return n .. ',' .. tostring(i) .. ',' .. tostring(done)"
            )
            .unwrap(),
            "Lady Deathwhisper,nil,false"
        );
        assert!(s
            .eval::<String>(
                "return tostring(select(2, pcall(GetInstanceLockTimeRemainingEncounter)))"
            )
            .unwrap()
            .contains("Usage: GetInstanceLockTimeRemainingEncounter(encounterIndex)"));
        assert_eq!(
            s.eval::<f64>("return GetInstanceDifficulty()").unwrap(),
            2.0
        );
    }

    /// `RespondInstanceLock` coerces as the reference's boolean helper does.
    #[test]
    fn respond_instance_lock_queues_the_answer() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "RespondInstanceLock(true) RespondInstanceLock(false) RespondInstanceLock('1') \
             RespondInstanceLock()",
        )
        .unwrap();
        assert_eq!(s.take_instance_lock_responses(), [true, false, true, false]);
        assert!(s.take_instance_lock_responses().is_empty());
    }

    /// `CanMapChangeDifficulty([map])` (`0x517d70`): the given map, else the current one.
    #[test]
    fn can_map_change_difficulty_reads_the_flag() {
        let mut s = UiScript::new().unwrap();
        s.set_dynamic_difficulty_maps(vec![631, 724]);
        s.set_lfg_world(631, 0, 0);
        assert_eq!(
            s.eval::<String>(
                "return tostring(CanMapChangeDifficulty()) .. ',' .. \
                 tostring(CanMapChangeDifficulty(36)) .. ',' .. \
                 tostring(CanMapChangeDifficulty(724))"
            )
            .unwrap(),
            "1,nil,1"
        );
    }

    /// The difficulty setters (`0x526050`, `0x5261a0`): the range, then the leader gate.
    #[test]
    fn the_difficulty_setters_gate_on_range_and_leader() {
        use crate::script::{PartyMemberInfo, PartyRequest, PartyState};
        let mut s = UiScript::new().unwrap();
        s.run(
            "SetDungeonDifficulty(2) SetDungeonDifficulty(3) SetDungeonDifficulty(0) \
             SetRaidDifficulty(4) SetRaidDifficulty(5)",
        )
        .unwrap();
        assert_eq!(
            s.take_party_requests(),
            [
                PartyRequest::SetDungeonDifficulty(1),
                PartyRequest::SetRaidDifficulty(3),
            ],
            "solo: in range only"
        );
        assert!(s
            .eval::<String>("return tostring(select(2, pcall(SetRaidDifficulty, 'x')))")
            .unwrap()
            .contains("Usage: SetRaidDifficulty(difficulty)"));
        s.set_party(PartyState {
            members: vec![PartyMemberInfo::default()],
            leader_guid: 7,
            own_guid: 1,
            ..Default::default()
        });
        s.run("SetDungeonDifficulty(1)").unwrap();
        assert!(s.take_party_requests().is_empty());
        assert_eq!(s.take_ui_errors(), ["ERR_NOT_LEADER"]);
    }
}
