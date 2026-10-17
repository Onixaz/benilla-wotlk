//! The 3.3.5a combat log (`UnitCombatLog_C.cpp`): entry store, filters, verbs and events.

use std::collections::VecDeque;

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::bool_or_default;
use super::{Model, ScriptValue};

/// The event names, indexed by the entry kind (`0xadb758`).
pub const EVENTS: [&str; 50] = [
    "ENVIRONMENTAL_DAMAGE",
    "SWING_DAMAGE",
    "SWING_MISSED",
    "RANGE_DAMAGE",
    "RANGE_MISSED",
    "SPELL_CAST_START",
    "SPELL_CAST_SUCCESS",
    "SPELL_CAST_FAILED",
    "SPELL_MISSED",
    "SPELL_DAMAGE",
    "SPELL_HEAL",
    "SPELL_ENERGIZE",
    "SPELL_DRAIN",
    "SPELL_LEECH",
    "SPELL_INSTAKILL",
    "SPELL_SUMMON",
    "SPELL_CREATE",
    "SPELL_INTERRUPT",
    "SPELL_EXTRA_ATTACKS",
    "SPELL_DURABILITY_DAMAGE",
    "SPELL_DURABILITY_DAMAGE_ALL",
    "SPELL_AURA_APPLIED",
    "SPELL_AURA_APPLIED_DOSE",
    "SPELL_AURA_REMOVED_DOSE",
    "SPELL_AURA_REMOVED",
    "SPELL_AURA_REFRESH",
    "SPELL_DISPEL",
    "SPELL_STOLEN",
    "SPELL_AURA_BROKEN",
    "SPELL_AURA_BROKEN_SPELL",
    "DAMAGE_AURA_BROKEN",
    "ENCHANT_APPLIED",
    "ENCHANT_REMOVED",
    "SPELL_PERIODIC_MISSED",
    "SPELL_PERIODIC_DAMAGE",
    "SPELL_PERIODIC_HEAL",
    "SPELL_PERIODIC_ENERGIZE",
    "SPELL_PERIODIC_DRAIN",
    "SPELL_PERIODIC_LEECH",
    "SPELL_DISPEL_FAILED",
    "DAMAGE_SHIELD",
    "DAMAGE_SHIELD_MISSED",
    "DAMAGE_SPLIT",
    "PARTY_KILL",
    "UNIT_DIED",
    "UNIT_DESTROYED",
    "SPELL_RESURRECT",
    "SPELL_BUILDING_DAMAGE",
    "SPELL_BUILDING_HEAL",
    "UNIT_DISSIPATES",
];

/// The CVar that bounds how long an entry is kept, in seconds (`0x51efed`).
pub const RETENTION_CVAR: &str = "combatLogRetentionTime";

/// The flags of an absent endpoint, `COMBATLOG_OBJECT_NONE` (`0x74dcb0`, `0x74d920`).
pub const OBJECT_NONE: u32 = 0x8000_0000;

/// One end of an entry: its guid, its name once known, and its `COMBATLOG_OBJECT_*` flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CombatLogUnit {
    pub guid: u64,
    pub name: Option<String>,
    pub flags: u32,
}

impl CombatLogUnit {
    /// No unit: guid 0, no name, [`OBJECT_NONE`].
    pub fn none() -> Self {
        Self {
            guid: 0,
            name: None,
            flags: OBJECT_NONE,
        }
    }
}

/// A miss type, by its index in the reference's word table (`0xadbfac`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissType {
    Miss = 1,
    Resist = 2,
    Dodge = 3,
    Parry = 4,
    Block = 5,
    Evade = 6,
    Immune = 7,
    Deflect = 9,
    Absorb = 10,
    Reflect = 11,
}

impl MissType {
    /// The word the event carries.
    pub fn word(self) -> &'static str {
        match self {
            MissType::Miss => "MISS",
            MissType::Resist => "RESIST",
            MissType::Dodge => "DODGE",
            MissType::Parry => "PARRY",
            MissType::Block => "BLOCK",
            MissType::Evade => "EVADE",
            MissType::Immune => "IMMUNE",
            MissType::Deflect => "DEFLECT",
            MissType::Absorb => "ABSORB",
            MissType::Reflect => "REFLECT",
        }
    }
}

/// A damage block, the nine values `0x74e290` pushes for its `0x10` arm.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Damage {
    pub amount: i32,
    pub overkill: i32,
    /// The school mask, 1 physical.
    pub school: u32,
    pub resisted: i32,
    pub blocked: i32,
    pub absorbed: i32,
    pub critical: bool,
    pub glancing: bool,
    pub crushing: bool,
}

/// What an entry says past its two endpoints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CombatLogPayload {
    /// `SWING_DAMAGE`.
    SwingDamage(Damage),
    /// `SWING_MISSED`; an absorb, resist or block also carries the amount it stopped.
    SwingMissed { miss: MissType, amount: Option<i32> },
}

impl CombatLogPayload {
    /// The entry kind, the index into [`EVENTS`].
    fn kind(&self) -> usize {
        match self {
            CombatLogPayload::SwingDamage(_) => 1,
            CombatLogPayload::SwingMissed { .. } => 2,
        }
    }

    /// The spell id a filter's spell test reads; swings have none.
    fn spell_id(&self) -> u32 {
        0
    }

    /// The spell name a filter's spell test reads; swings have none.
    fn spell_name(&self) -> Option<&str> {
        None
    }
}

/// One combat-log entry as the app hands it over, names and flags resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct CombatLogEntry {
    /// The client clock at the event, in milliseconds, which the retention time is measured on.
    pub time_ms: u64,
    /// The event's first value: `time()` at world entry plus the seconds since (`0x74e290`).
    pub timestamp: f64,
    pub source: CombatLogUnit,
    pub dest: CombatLogUnit,
    pub payload: CombatLogPayload,
}

/// One `CombatLogAddFilter` row (`0x74e050`).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Filter {
    /// Bit `kind` set for each event the filter passes.
    events: u64,
    src_guid: u64,
    src_mask: u32,
    dst_guid: u64,
    dst_mask: u32,
    spell_id: u32,
    spell_name: Option<String>,
}

impl Filter {
    /// `CombatLogFilter::MatchesFilter` (`0x74e1a0`).
    fn matches(&self, e: &CombatLogEntry) -> bool {
        if self.events & (1 << e.payload.kind()) == 0 {
            return false;
        }
        let end = |guid: u64, mask: u32, unit: &CombatLogUnit| {
            if guid == 0 {
                is_a(unit.flags, mask)
            } else {
                unit.guid == guid
            }
        };
        if !end(self.src_guid, self.src_mask, &e.source)
            || !end(self.dst_guid, self.dst_mask, &e.dest)
        {
            return false;
        }
        if self.spell_id != 0 {
            return e.payload.spell_id() == self.spell_id;
        }
        match &self.spell_name {
            Some(want) => e
                .payload
                .spell_name()
                .is_some_and(|n| n.eq_ignore_ascii_case(want)),
            None => true,
        }
    }
}

/// The entries oldest first, the Lua cursor and the filters; each VM starts empty (`0x7538a0`).
#[derive(Debug, Default)]
pub struct CombatLogStore {
    entries: VecDeque<CombatLogEntry>,
    /// The id of `entries[0]`; ids grow by one per entry and never repeat.
    first_id: u64,
    /// The entry `CombatLogGetCurrentEntry` reads (`[0xca1390]`).
    current: Option<u64>,
    filters: Vec<Filter>,
}

impl CombatLogStore {
    /// `CombatLogEntry::MatchesFilter` (`0x74f6c0`): no filter passes everything.
    fn passes(&self, e: &CombatLogEntry) -> bool {
        self.filters.is_empty() || self.filters.iter().any(|f| f.matches(e))
    }

    fn get(&self, id: u64) -> Option<&CombatLogEntry> {
        let at = usize::try_from(id.checked_sub(self.first_id)?).ok()?;
        self.entries.get(at)
    }

    fn last_id(&self) -> Option<u64> {
        (!self.entries.is_empty()).then(|| self.first_id + self.entries.len() as u64 - 1)
    }

    /// `0x750390` on the oldest entry: a cursor on it moves to the next one.
    fn pop_oldest(&mut self) {
        if self.entries.pop_front().is_none() {
            return;
        }
        let gone = self.first_id;
        self.first_id += 1;
        if self.current == Some(gone) {
            self.current = (!self.entries.is_empty()).then_some(self.first_id);
        }
    }

    fn counted(&self, id: u64, all: bool) -> bool {
        all || self.get(id).is_some_and(|e| self.passes(e))
    }

    /// `CombatLogSetCurrentEntry` (`0x74fae0`): 1 the oldest passing entry, 0 the newest.
    fn seek(&self, index: i64, all: bool) -> Option<u64> {
        if index >= 1 {
            let mut left = index;
            (self.first_id..=self.last_id()?)
                .filter(|&id| self.counted(id, all))
                .find(|_| {
                    left -= 1;
                    left == 0
                })
        } else {
            let mut left = index;
            (self.first_id..=self.last_id()?)
                .rev()
                .filter(|&id| self.counted(id, all))
                .find(|_| {
                    let hit = left == 0;
                    left += 1;
                    hit
                })
        }
    }

    /// `CombatLogAdvanceEntry` (`0x74fc20`): the current entry spends one step when it passes.
    fn advance(&self, count: i64, all: bool) -> Option<u64> {
        let from = self.current?;
        let last = self.last_id()?;
        let mut left = count;
        if count > 0 {
            (from..=last).filter(|&id| self.counted(id, all)).find(|_| {
                let hit = left == 0;
                left -= 1;
                hit
            })
        } else {
            (self.first_id..=from)
                .rev()
                .filter(|&id| self.counted(id, all))
                .find(|_| {
                    let hit = left == 0;
                    left += 1;
                    hit
                })
        }
    }

    /// The values `0x74e290` pushes for an entry.
    fn args(&self, e: &CombatLogEntry) -> Vec<ScriptValue> {
        let mut out = vec![
            ScriptValue::Number(e.timestamp),
            ScriptValue::Str(EVENTS[e.payload.kind()].to_string()),
        ];
        for unit in [&e.source, &e.dest] {
            out.push(ScriptValue::Str(guid_text(unit.guid)));
            out.push(match &unit.name {
                Some(n) if !n.is_empty() => ScriptValue::Str(n.clone()),
                _ => ScriptValue::Nil,
            });
            // `lua_pushinteger` of the int: `OBJECT_NONE` reads negative.
            out.push(ScriptValue::Int(i64::from(unit.flags as i32)));
        }
        let or_nil = |n: i32| {
            if n == 0 {
                ScriptValue::Nil
            } else {
                ScriptValue::Number(f64::from(n))
            }
        };
        let one = |b: bool| {
            if b {
                ScriptValue::Number(1.0)
            } else {
                ScriptValue::Nil
            }
        };
        match &e.payload {
            CombatLogPayload::SwingDamage(d) => out.extend([
                ScriptValue::Number(f64::from(d.amount)),
                ScriptValue::Number(f64::from(d.overkill)),
                ScriptValue::Int(i64::from(d.school as i32)),
                or_nil(d.resisted),
                or_nil(d.blocked),
                or_nil(d.absorbed),
                one(d.critical),
                one(d.glancing),
                one(d.crushing),
            ]),
            CombatLogPayload::SwingMissed { miss, amount } => {
                out.push(ScriptValue::Str(miss.word().to_string()));
                if let Some(n) = amount {
                    out.push(ScriptValue::Int(i64::from(*n)));
                }
            }
        }
        out
    }
}

/// `0x74d0d0`: `0x` and sixteen upper-case hex digits.
fn guid_text(guid: u64) -> String {
    format!("0x{guid:016X}")
}

/// `0x74d1a0`, also `CombatLog_Object_IsA`: a special bit, or every one of the four groups.
fn is_a(flags: u32, mask: u32) -> bool {
    let m = flags & mask;
    m & 0xffff_0000 != 0 || (m & 0xf != 0 && m & 0xf0 != 0 && m & 0x300 != 0 && m & 0xfc00 != 0)
}

/// `0x74d120`: an optional `0x`, then up to sixteen hex digits to the first other byte.
fn parse_guid(s: &str) -> u64 {
    let s = s
        .get(..2)
        .filter(|p| p.eq_ignore_ascii_case("0x"))
        .map_or(s, |_| &s[2..]);
    s.bytes()
        .take(16)
        .map_while(|b| (b as char).to_digit(16))
        .fold(0, |n, d| (n << 4) | u64::from(d))
}

/// `lua_tointeger`: the x87 store at its default round-to-nearest-even.
fn rounded(n: f64) -> i64 {
    n.round_ties_even() as i64
}

fn retention_secs(model: &Model) -> i64 {
    model
        .cvars
        .get(&RETENTION_CVAR.to_ascii_lowercase())
        .map_or(0, |slot| i64::from(super::cvars::sstr_to_int(&slot.value)))
}

impl super::UiScript {
    /// Take the store out of this VM, for a `ReloadUI`, which the reference's store outlives.
    pub fn take_combat_log(&mut self) -> CombatLogStore {
        std::mem::take(&mut self.model_mut().combat_log)
    }

    /// Put a store taken from the previous VM back.
    pub fn restore_combat_log(&mut self, store: CombatLogStore) {
        self.model_mut().combat_log = store;
    }

    /// Store and fire an entry (`0x750400`, `0x74f910`), first dropping one expired oldest entry.
    pub fn push_combat_log(&mut self, entry: CombatLogEntry) {
        let (args, passes) = {
            let mut model = self.model_mut();
            let retention_ms = retention_secs(&model) * 1000;
            let log = &mut model.combat_log;
            let expired = log.entries.front().is_some_and(|oldest| {
                entry.time_ms as i64 - retention_ms - oldest.time_ms as i64 >= 0
            });
            if expired {
                log.pop_oldest();
            }
            let out = (log.args(&entry), log.passes(&entry));
            log.entries.push_back(entry);
            out
        };
        if passes {
            self.fire_event("COMBAT_LOG_EVENT", args.clone());
        }
        self.fire_event("COMBAT_LOG_EVENT_UNFILTERED", args);
    }
}

/// `1.0` or nil, as these verbs push them.
fn one_or_nil(b: bool) -> Value {
    if b {
        Value::Number(1.0)
    } else {
        Value::Nil
    }
}

/// A `srcMask`/`dstMask` (`0x74ff70`): a number is a mask, a string a guid, else every unit.
fn endpoint(lua: &Lua, v: &Value, which: &str) -> mlua::Result<(u64, u32)> {
    match v {
        Value::Integer(_) | Value::Number(_) => {
            let mask = rounded(lua.coerce_number(v.clone())?.unwrap_or(0.0)) as u32;
            let incomplete = mask & 0xffff_0000 == 0
                && (mask & 0xf == 0 || mask & 0xf0 == 0 || mask & 0x300 == 0 || mask & 0xfc00 == 0);
            if incomplete {
                return Err(mlua::Error::RuntimeError(format!(
                    "CombatLogAddFilter(): incomplete filter for {which}"
                )));
            }
            Ok((0, mask))
        }
        Value::String(s) => Ok((parse_guid(&s.to_string_lossy()), 0)),
        _ => Ok((0, u32::MAX)),
    }
}

/// The first argument as `lua_isnumber` and `lua_tointeger` read it, or the usage error.
fn count_arg(lua: &Lua, args: &MultiValue, usage: &'static str) -> mlua::Result<i64> {
    let n = args
        .front()
        .cloned()
        .map(|v| lua.coerce_number(v))
        .transpose()?
        .flatten();
    n.map(rounded)
        .ok_or_else(|| mlua::Error::RuntimeError(usage.into()))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `CombatTextSetActiveUnit(unit)` (`0x74d580`) keys `COMBAT_TEXT_UPDATE` to the unit's guid
    // `[0xca1380]` and returns nothing; benilla's feed reports the player, the stock default.
    g.set(
        "CombatTextSetActiveUnit",
        lua.create_function(|_, _: mlua::MultiValue| Ok(()))?,
    )?;

    g.set(
        "CombatLogAddFilter",
        lua.create_function(|lua, args: MultiValue| {
            let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Nil);
            let events = match arg(0) {
                Value::Nil => None,
                v => lua.coerce_string(v)?.map(|s| s.to_string_lossy()),
            };
            let (src_guid, src_mask) = endpoint(lua, &arg(1), "srcMask")?;
            let (dst_guid, dst_mask) = endpoint(lua, &arg(2), "dstMask")?;
            let (spell_id, spell_name) = match arg(3) {
                v @ (Value::Integer(_) | Value::Number(_)) => {
                    (rounded(lua.coerce_number(v)?.unwrap_or(0.0)) as u32, None)
                }
                Value::String(s) => (0, Some(s.to_string_lossy())),
                _ => (0, None),
            };
            let events = match events {
                None => u64::MAX,
                Some(list) => list
                    .split([' ', ','])
                    .filter(|t| !t.is_empty())
                    .filter_map(|t| EVENTS.iter().position(|e| e.eq_ignore_ascii_case(t)))
                    .fold(0, |bits, i| bits | 1 << i),
            };
            super::model_mut_of(lua).combat_log.filters.push(Filter {
                events,
                src_guid,
                src_mask,
                dst_guid,
                dst_mask,
                spell_id,
                spell_name,
            });
            Ok(())
        })?,
    )?;

    g.set(
        "CombatLogResetFilter",
        lua.create_function(|lua, ()| {
            super::model_mut_of(lua).combat_log.filters.clear();
            Ok(())
        })?,
    )?;

    g.set(
        "CombatLogGetNumEntries",
        lua.create_function(|lua, all: Option<Value>| {
            let all = bool_or_default(all.as_ref(), false);
            let model = super::model_mut_of(lua);
            let log = &model.combat_log;
            Ok(log.entries.iter().filter(|e| all || log.passes(e)).count() as i64)
        })?,
    )?;

    g.set(
        "CombatLogSetCurrentEntry",
        lua.create_function(|lua, args: MultiValue| {
            let index = count_arg(
                lua,
                &args,
                "Usage: CombatLogSetCurrentEntry(index [,ignoreFilter])",
            )?;
            let all = bool_or_default(args.get(1), false);
            let mut model = super::model_mut_of(lua);
            let log = &mut model.combat_log;
            log.current = log.seek(index, all);
            Ok(one_or_nil(log.current.is_some()))
        })?,
    )?;

    g.set(
        "CombatLogAdvanceEntry",
        lua.create_function(|lua, args: MultiValue| {
            let count = count_arg(
                lua,
                &args,
                "Usage: CombatLogAdvanceEntry(count [,ignoreFilter])",
            )?;
            let all = bool_or_default(args.get(1), false);
            let mut model = super::model_mut_of(lua);
            let log = &mut model.combat_log;
            if count != 0 {
                log.current = log.advance(count, all);
            }
            Ok(one_or_nil(log.current.is_some()))
        })?,
    )?;

    g.set(
        "CombatLogGetCurrentEntry",
        lua.create_function(|lua, ()| {
            let args = {
                let model = super::model_mut_of(lua);
                let log = &model.combat_log;
                log.current
                    .and_then(|id| log.get(id))
                    .map(|e| log.args(e))
                    .unwrap_or_default()
            };
            args.into_iter()
                .map(|v| v.into_lua(lua))
                .collect::<mlua::Result<MultiValue>>()
        })?,
    )?;

    g.set(
        "CombatLogClearEntries",
        lua.create_function(|lua, ()| {
            let mut model = super::model_mut_of(lua);
            let log = &mut model.combat_log;
            while !log.entries.is_empty() {
                log.pop_oldest();
            }
            Ok(())
        })?,
    )?;

    g.set(
        "CombatLog_Object_IsA",
        lua.create_function(|lua, (flags, mask): (Option<Value>, Option<Value>)| {
            let num = |v: Option<Value>| rounded(super::binding_abi::coerced_number(lua, v)) as u32;
            Ok(one_or_nil(is_a(num(flags), num(mask))))
        })?,
    )?;

    g.set(
        "CombatLogGetRetentionTime",
        lua.create_function(|lua, ()| Ok(retention_secs(&super::model_mut_of(lua))))?,
    )?;

    g.set(
        "CombatLogSetRetentionTime",
        lua.create_function(|lua, args: MultiValue| {
            let secs = count_arg(lua, &args, "Usage: CombatLogSetRetentionTime(seconds)")?;
            let mut model = super::model_mut_of(lua);
            super::cvars::write_cvar(&mut model, RETENTION_CVAR, (secs as i32).to_string());
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    /// Mine, friendly, player-controlled player.
    const ME: u32 = 0x511;
    /// Outsider, hostile, NPC-controlled NPC.
    const MOB: u32 = 0xa48;
    const ME_GUID: u64 = 0x3ee;
    const MOB_GUID: u64 = 0xf130_0000_060c_281e;

    fn unit(guid: u64, name: &str, flags: u32) -> CombatLogUnit {
        CombatLogUnit {
            guid,
            name: Some(name.into()),
            flags,
        }
    }

    fn hit(time_ms: u64, amount: i32) -> CombatLogEntry {
        CombatLogEntry {
            time_ms,
            timestamp: 1_000_000.0 + time_ms as f64 * 0.001,
            source: unit(ME_GUID, "Claude", ME),
            dest: unit(MOB_GUID, "Kobold Vermin", MOB),
            payload: CombatLogPayload::SwingDamage(Damage {
                amount,
                school: 1,
                ..Damage::default()
            }),
        }
    }

    fn dodge(time_ms: u64) -> CombatLogEntry {
        CombatLogEntry {
            time_ms,
            timestamp: 1_000_000.0 + time_ms as f64 * 0.001,
            source: unit(MOB_GUID, "Kobold Vermin", MOB),
            dest: unit(ME_GUID, "Claude", ME),
            payload: CombatLogPayload::SwingMissed {
                miss: MissType::Dodge,
                amount: None,
            },
        }
    }

    fn script() -> UiScript {
        let s = UiScript::new().unwrap();
        s.register_cvars([(RETENTION_CVAR, "300")]);
        s
    }

    /// The values `0x74e290` pushes for a landed swing, nil where a count is zero.
    #[test]
    fn a_swing_reads_back_in_the_12340_shape() {
        let mut s = script();
        s.push_combat_log(CombatLogEntry {
            payload: CombatLogPayload::SwingDamage(Damage {
                amount: 17,
                overkill: 3,
                school: 1,
                critical: true,
                ..Damage::default()
            }),
            ..hit(7_500, 0)
        });
        let got: String = s
            .eval(
                "CombatLogSetCurrentEntry(0)
                 local t = {CombatLogGetCurrentEntry()}
                 local out = {}
                 for i = 1, 17 do out[i] = tostring(t[i]) end
                 return table.concat(out, ',')",
            )
            .unwrap();
        assert_eq!(
            got,
            "1000007.5,SWING_DAMAGE,0x00000000000003EE,Claude,1297,0xF1300000060C281E,\
             Kobold Vermin,2632,17,3,1,nil,nil,nil,1,nil,nil"
        );
    }

    #[test]
    fn a_miss_carries_its_word_and_a_stopped_amount() {
        let mut s = script();
        s.push_combat_log(dodge(6_000));
        s.push_combat_log(CombatLogEntry {
            payload: CombatLogPayload::SwingMissed {
                miss: MissType::Absorb,
                amount: Some(40),
            },
            ..dodge(6_100)
        });
        let (n, word, amount): (i64, String, i64) = s
            .eval(
                "CombatLogSetCurrentEntry(0)
                 local t = {CombatLogGetCurrentEntry()}
                 return select('#', CombatLogGetCurrentEntry()), t[9], t[10]",
            )
            .unwrap();
        assert_eq!((n, word.as_str(), amount), (10, "ABSORB", 40));
        let (n, word): (i64, String) = s
            .eval(
                "CombatLogSetCurrentEntry(1)
                 return select('#', CombatLogGetCurrentEntry()), select(9, CombatLogGetCurrentEntry())",
            )
            .unwrap();
        assert_eq!((n, word.as_str()), (9, "DODGE"));
    }

    /// The filtered event fires only for a passing entry; the unfiltered one always does.
    #[test]
    fn the_filter_gates_combat_log_event_only() {
        let mut s = script();
        s.run(
            "seen = {}
             local f = CreateFrame('Frame')
             f:RegisterEvent('COMBAT_LOG_EVENT')
             f:RegisterEvent('COMBAT_LOG_EVENT_UNFILTERED')
             f:SetScript('OnEvent', function(self, event, _, kind)
               seen[#seen + 1] = event .. ':' .. kind
             end)
             CombatLogAddFilter('SWING_MISSED', nil, nil)",
        )
        .unwrap();
        s.push_combat_log(hit(6_000, 17));
        s.push_combat_log(dodge(6_100));
        let seen: String = s.eval("return table.concat(seen, ' ')").unwrap();
        assert_eq!(
            seen,
            "COMBAT_LOG_EVENT_UNFILTERED:SWING_DAMAGE COMBAT_LOG_EVENT:SWING_MISSED \
             COMBAT_LOG_EVENT_UNFILTERED:SWING_MISSED"
        );
        assert_eq!(s.eval::<i64>("return CombatLogGetNumEntries()").unwrap(), 1);
        assert_eq!(
            s.eval::<i64>("return CombatLogGetNumEntries(1)").unwrap(),
            2
        );
        s.run("CombatLogResetFilter()").unwrap();
        assert_eq!(s.eval::<i64>("return CombatLogGetNumEntries()").unwrap(), 2);
    }

    #[test]
    fn a_filter_matches_by_mask_or_guid() {
        let mut s = script();
        s.push_combat_log(hit(6_000, 17));
        s.push_combat_log(dodge(6_100));
        // `COMBATLOG_FILTER_MINE`: my own outgoing swing alone.
        s.run("CombatLogAddFilter(nil, 0x7d11, nil)").unwrap();
        assert_eq!(s.eval::<i64>("return CombatLogGetNumEntries()").unwrap(), 1);
        s.run("CombatLogResetFilter() CombatLogAddFilter(nil, nil, '0x00000000000003ee')")
            .unwrap();
        assert_eq!(s.eval::<i64>("return CombatLogGetNumEntries()").unwrap(), 1);
        // An empty event list passes nothing.
        s.run("CombatLogResetFilter() CombatLogAddFilter('', nil, nil)")
            .unwrap();
        assert_eq!(s.eval::<i64>("return CombatLogGetNumEntries()").unwrap(), 0);
        // A spell filter never passes a swing.
        s.run("CombatLogResetFilter() CombatLogAddFilter(nil, nil, nil, 78)")
            .unwrap();
        assert_eq!(s.eval::<i64>("return CombatLogGetNumEntries()").unwrap(), 0);
    }

    #[test]
    fn an_incomplete_mask_raises() {
        let s = script();
        let err = s.run("CombatLogAddFilter(nil, 0x11, nil)").unwrap_err();
        assert!(err.to_string().contains("incomplete filter for srcMask"));
        let err = s.run("CombatLogAddFilter(nil, nil, 0x700)").unwrap_err();
        assert!(err.to_string().contains("incomplete filter for dstMask"));
        // A special bit alone is complete.
        s.run("CombatLogAddFilter(nil, 0x10000, nil)").unwrap();
    }

    #[test]
    fn object_is_a_tests_every_group_or_a_special_bit() {
        let s = script();
        let is_a = |flags: u32, mask: u32| {
            s.eval::<Option<f64>>(&format!("return CombatLog_Object_IsA({flags}, {mask})"))
                .unwrap()
        };
        assert_eq!(is_a(ME, 0x7d11), Some(1.0));
        assert_eq!(is_a(MOB, 0x7d11), None);
        assert_eq!(is_a(MOB | 0x10000, 0x10000), Some(1.0));
    }

    /// 1 is the oldest, 0 the newest, -1 the one before; an advance steps from the current entry.
    #[test]
    fn the_cursor_walks_both_ways() {
        let mut s = script();
        for (i, t) in [6_000, 6_100, 6_200].into_iter().enumerate() {
            s.push_combat_log(hit(t, 10 + i as i32));
        }
        let amount = "return select(9, CombatLogGetCurrentEntry())";
        s.run("CombatLogSetCurrentEntry(1)").unwrap();
        assert_eq!(s.eval::<i64>(amount).unwrap(), 10);
        s.run("CombatLogSetCurrentEntry(0)").unwrap();
        assert_eq!(s.eval::<i64>(amount).unwrap(), 12);
        s.run("CombatLogSetCurrentEntry(-1)").unwrap();
        assert_eq!(s.eval::<i64>(amount).unwrap(), 11);
        assert_eq!(
            s.eval::<Option<f64>>("return CombatLogAdvanceEntry(-1)")
                .unwrap(),
            Some(1.0)
        );
        assert_eq!(s.eval::<i64>(amount).unwrap(), 10);
        assert_eq!(
            s.eval::<Option<f64>>("return CombatLogAdvanceEntry(-1)")
                .unwrap(),
            None
        );
        assert_eq!(
            s.eval::<i64>("return select('#', CombatLogGetCurrentEntry())")
                .unwrap(),
            0
        );
        s.run("CombatLogSetCurrentEntry(1) CombatLogAdvanceEntry(2)")
            .unwrap();
        assert_eq!(s.eval::<i64>(amount).unwrap(), 12);
        assert_eq!(
            s.eval::<Option<f64>>("return CombatLogSetCurrentEntry(4)")
                .unwrap(),
            None
        );
        assert!(s.run("CombatLogSetCurrentEntry()").is_err());
    }

    /// One expired entry goes per new entry, and a cursor on it moves to the next.
    #[test]
    fn retention_drops_the_oldest_and_moves_the_cursor() {
        let mut s = script();
        s.run("CombatLogSetRetentionTime(10)").unwrap();
        assert_eq!(
            s.take_cvar_changes(),
            [(RETENTION_CVAR.to_string(), "10".to_string())]
        );
        assert_eq!(
            s.eval::<i64>("return CombatLogGetRetentionTime()").unwrap(),
            10
        );
        s.push_combat_log(hit(6_000, 10));
        s.push_combat_log(hit(7_000, 11));
        s.run("CombatLogSetCurrentEntry(1)").unwrap();
        s.push_combat_log(hit(16_000, 12));
        assert_eq!(s.eval::<i64>("return CombatLogGetNumEntries()").unwrap(), 2);
        assert_eq!(
            s.eval::<i64>("return select(9, CombatLogGetCurrentEntry())")
                .unwrap(),
            11
        );
        s.run("CombatLogClearEntries()").unwrap();
        assert_eq!(s.eval::<i64>("return CombatLogGetNumEntries()").unwrap(), 0);
        assert_eq!(
            s.eval::<i64>("return select('#', CombatLogGetCurrentEntry())")
                .unwrap(),
            0
        );
    }

    /// A `ReloadUI` carries the entries, cursor and filters into the new VM.
    #[test]
    fn the_store_survives_a_reload() {
        let mut s = script();
        s.push_combat_log(hit(6_000, 10));
        s.push_combat_log(dodge(6_100));
        s.run("CombatLogAddFilter('SWING_DAMAGE', nil, nil) CombatLogSetCurrentEntry(0)")
            .unwrap();
        let carried = s.take_combat_log();
        let mut fresh = script();
        assert_eq!(
            fresh
                .eval::<i64>("return CombatLogGetNumEntries(1)")
                .unwrap(),
            0
        );
        fresh.restore_combat_log(carried);
        let (n, all, amount): (i64, i64, i64) = fresh
            .eval("return CombatLogGetNumEntries(), CombatLogGetNumEntries(1), select(9, CombatLogGetCurrentEntry())")
            .unwrap();
        assert_eq!((n, all, amount), (1, 2, 10));
    }

    #[test]
    fn a_guid_parses_with_or_without_its_prefix() {
        assert_eq!(parse_guid("0xF1300000060C281E"), MOB_GUID);
        assert_eq!(parse_guid("f1300000060c281e"), MOB_GUID);
        assert_eq!(parse_guid("0x3eeZZ"), 0x3ee);
    }
}
