//! The 3.3.5a combat log: entry store, filters, verbs and events.

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

/// An entry's event, its index into [`EVENTS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CombatLogEvent {
    EnvironmentalDamage,
    SwingDamage,
    SwingMissed,
    RangeDamage,
    RangeMissed,
    SpellCastStart,
    SpellCastSuccess,
    SpellCastFailed,
    SpellMissed,
    SpellDamage,
    SpellHeal,
    SpellEnergize,
    SpellDrain,
    SpellLeech,
    SpellInstakill,
    SpellSummon,
    SpellCreate,
    SpellInterrupt,
    SpellExtraAttacks,
    SpellDurabilityDamage,
    SpellDurabilityDamageAll,
    SpellAuraApplied,
    SpellAuraAppliedDose,
    SpellAuraRemovedDose,
    SpellAuraRemoved,
    SpellAuraRefresh,
    SpellDispel,
    SpellStolen,
    SpellAuraBroken,
    SpellAuraBrokenSpell,
    DamageAuraBroken,
    EnchantApplied,
    EnchantRemoved,
    SpellPeriodicMissed,
    SpellPeriodicDamage,
    SpellPeriodicHeal,
    SpellPeriodicEnergize,
    SpellPeriodicDrain,
    SpellPeriodicLeech,
    SpellDispelFailed,
    DamageShield,
    DamageShieldMissed,
    DamageSplit,
    PartyKill,
    UnitDied,
    UnitDestroyed,
    SpellResurrect,
    SpellBuildingDamage,
    SpellBuildingHeal,
    UnitDissipates,
}

impl CombatLogEvent {
    /// The event's name, the entry's second value.
    pub fn name(self) -> &'static str {
        EVENTS[self as usize]
    }
}

/// The CVar that bounds how long an entry is kept, in seconds (`0x51efed`).
pub const RETENTION_CVAR: &str = "combatLogRetentionTime";

/// The flags of an absent endpoint, `COMBATLOG_OBJECT_NONE` (`0x74dcb0`, `0x74d920`).
pub const OBJECT_NONE: u32 = 0x8000_0000;

/// The text an unresolved name reads as (`0x74f720`, `0x74fd40`).
const UNKNOWN_OBJECT: &str = "UNKNOWNOBJECT";

/// One end of an entry: its guid, its name once known, and its `COMBATLOG_OBJECT_*` flags. A name
/// still `None` on a nonzero guid reads as `UNKNOWNOBJECT`, as an unresolved one does (`0x74f720`).
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

/// A spell as an entry carries it: the id, and the record's name and school mask, `None` and 0
/// when the record is missing (`0x74e290`, `0x750400`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CombatLogSpell {
    pub id: u32,
    pub name: Option<String>,
    pub school: u32,
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
    /// The second immune code, worded alike.
    Immune2 = 8,
    Deflect = 9,
    Absorb = 10,
    Reflect = 11,
}

impl MissType {
    /// The wire's miss code, 1 to 11; 0 and anything past the table is none.
    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::Miss,
            2 => Self::Resist,
            3 => Self::Dodge,
            4 => Self::Parry,
            5 => Self::Block,
            6 => Self::Evade,
            7 => Self::Immune,
            8 => Self::Immune2,
            9 => Self::Deflect,
            10 => Self::Absorb,
            11 => Self::Reflect,
            _ => return None,
        })
    }

    /// The word the event carries.
    pub fn word(self) -> &'static str {
        match self {
            MissType::Miss => "MISS",
            MissType::Resist => "RESIST",
            MissType::Dodge => "DODGE",
            MissType::Parry => "PARRY",
            MissType::Block => "BLOCK",
            MissType::Evade => "EVADE",
            MissType::Immune | MissType::Immune2 => "IMMUNE",
            MissType::Deflect => "DEFLECT",
            MissType::Absorb => "ABSORB",
            MissType::Reflect => "REFLECT",
        }
    }
}

/// A miss, and the amount an absorb, resist or block carries (`0x74d980`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Miss {
    pub kind: MissType,
    pub amount: Option<i32>,
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

/// A heal block, the `0x20` arm's four values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Heal {
    pub amount: i32,
    pub overheal: i32,
    pub absorbed: i32,
    pub critical: bool,
}

/// A drain block, the `0x80` arm: the amount, the power type and what the caster gained.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drain {
    pub amount: i32,
    pub power: i32,
    pub extra: i32,
}

/// An item an entry names, with its name once the item cache answers (`0x750210`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CombatLogItem {
    pub id: u32,
    pub name: Option<String>,
}

/// What an entry says past its spell, one field per info-flag bit, pushed in `0x74e290`'s
/// order: `0x1`, `0x2`, `0x4`, `0x8`, `0x10`, the miss amounts, `0x20`, `0x40`, `0x80`, `0x100`,
/// `0x1000`, `0x200`, `0x400`, `0x800`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CombatLogInfo {
    /// `0x1`: one integer, the extra-attack count.
    pub amount: Option<i32>,
    /// `0x2`: a failed cast's reason.
    pub fail_reason: Option<String>,
    /// `0x4`, with `0x2000`/`0x4000`/`0x8000` for its amount.
    pub miss: Option<Miss>,
    /// `0x8`: the environmental type, 0 fatigue to 5 fire (`0xad671c`).
    pub environmental: Option<u8>,
    /// `0x10`.
    pub damage: Option<Damage>,
    /// `0x20`.
    pub heal: Option<Heal>,
    /// `0x40`: amount and power type.
    pub energize: Option<(i32, i32)>,
    /// `0x80`.
    pub drain: Option<Drain>,
    /// `0x100`: the dispelled, stolen, interrupted or broken-by spell.
    pub extra_spell: Option<CombatLogSpell>,
    /// `0x1000`: an enchantment's name.
    pub enchant: Option<String>,
    /// `0x200`.
    pub item: Option<CombatLogItem>,
    /// `0x400`: true for a harmful aura, `"DEBUFF"`.
    pub harmful: Option<bool>,
    /// `0x800`: an aura's new stack count.
    pub dose: Option<i32>,
}

/// One combat-log entry as the app hands it over, names and flags resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct CombatLogEntry {
    /// The client clock at the event, in milliseconds, which the retention time is measured on.
    pub time_ms: u64,
    /// The event's first value: `time()` at world entry plus the seconds since (`0x74e290`).
    pub timestamp: f64,
    pub event: CombatLogEvent,
    pub source: CombatLogUnit,
    pub dest: CombatLogUnit,
    pub spell: Option<CombatLogSpell>,
    pub info: CombatLogInfo,
}

impl CombatLogEntry {
    /// The spell id a filter's spell test reads; 0 without a spell.
    fn spell_id(&self) -> u32 {
        self.spell.as_ref().map_or(0, |s| s.id)
    }

    /// The spell name a filter's spell test reads.
    fn spell_name(&self) -> Option<&str> {
        self.spell.as_ref()?.name.as_deref()
    }
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
    /// The reference's filter match (`0x74e1a0`).
    fn matches(&self, e: &CombatLogEntry) -> bool {
        if self.events & (1 << e.event as u8) == 0 {
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
            return e.spell_id() == self.spell_id;
        }
        match &self.spell_name {
            Some(want) => e
                .spell_name()
                .is_some_and(|n| n.to_lowercase() == want.to_lowercase()),
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
    /// The reference's entry match (`0x74f6c0`): no filter passes everything.
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
}

/// The values `0x74e290` pushes for an entry; `unknown` is the `UNKNOWNOBJECT` text.
fn args(e: &CombatLogEntry, unknown: &str) -> Vec<ScriptValue> {
    let num = |n: i32| ScriptValue::Number(f64::from(n));
    let int = |n: i32| ScriptValue::Int(i64::from(n));
    let or_nil = |n: i32| if n == 0 { ScriptValue::Nil } else { num(n) };
    let one = |b: bool| {
        if b {
            ScriptValue::Number(1.0)
        } else {
            ScriptValue::Nil
        }
    };
    let text = |s: &Option<String>| match s {
        Some(s) if s.is_empty() => ScriptValue::Nil,
        Some(s) => ScriptValue::Str(s.clone()),
        None => ScriptValue::Str(unknown.to_string()),
    };
    let mut out = vec![
        ScriptValue::Number(e.timestamp),
        ScriptValue::Str(e.event.name().to_string()),
    ];
    for unit in [&e.source, &e.dest] {
        out.push(ScriptValue::Str(guid_text(unit.guid)));
        out.push(if unit.guid == 0 {
            ScriptValue::Nil
        } else {
            text(&unit.name)
        });
        // `lua_pushinteger` of the int: `OBJECT_NONE` reads negative.
        out.push(int(unit.flags as i32));
    }
    let spell = |s: &CombatLogSpell, out: &mut Vec<ScriptValue>| {
        out.push(ScriptValue::Int(i64::from(s.id)));
        match &s.name {
            Some(name) => {
                out.push(ScriptValue::Str(name.clone()));
                out.push(int(s.school as i32));
            }
            None => out.extend([ScriptValue::Nil, ScriptValue::Int(0)]),
        }
    };
    if let Some(s) = &e.spell {
        spell(s, &mut out);
    }
    let i = &e.info;
    if let Some(n) = i.amount {
        out.push(int(n));
    }
    if let Some(reason) = &i.fail_reason {
        out.push(ScriptValue::Str(reason.clone()));
    }
    if let Some(m) = i.miss {
        out.push(ScriptValue::Str(m.kind.word().to_string()));
    }
    if let Some(ty) = i.environmental {
        const TYPES: [&str; 6] = ["FATIGUE", "DROWNING", "FALLING", "LAVA", "SLIME", "FIRE"];
        out.push(ScriptValue::Str(
            TYPES
                .get(usize::from(ty))
                .copied()
                .unwrap_or("")
                .to_string(),
        ));
    }
    if let Some(d) = i.damage {
        out.extend([
            num(d.amount),
            num(d.overkill),
            int(d.school as i32),
            or_nil(d.resisted),
            or_nil(d.blocked),
            or_nil(d.absorbed),
            one(d.critical),
            one(d.glancing),
            one(d.crushing),
        ]);
    }
    if let Some(Miss {
        amount: Some(n), ..
    }) = i.miss
    {
        out.push(int(n));
    }
    if let Some(h) = i.heal {
        out.extend([
            num(h.amount),
            num(h.overheal),
            num(h.absorbed),
            one(h.critical),
        ]);
    }
    if let Some((amount, power)) = i.energize {
        out.extend([num(amount), num(power)]);
    }
    if let Some(d) = i.drain {
        out.extend([num(d.amount), num(d.power), or_nil(d.extra)]);
    }
    if let Some(s) = &i.extra_spell {
        out.push(ScriptValue::Int(i64::from(s.id)));
        match &s.name {
            Some(name) => {
                out.push(ScriptValue::Str(name.clone()));
                out.push(int(s.school as i32));
            }
            None => out.extend([ScriptValue::Nil, ScriptValue::Number(0.0)]),
        }
    }
    if let Some(name) = &i.enchant {
        out.push(ScriptValue::Str(name.clone()));
    }
    if let Some(item) = &i.item {
        out.push(ScriptValue::Int(i64::from(item.id)));
        out.push(match &item.name {
            Some(name) => ScriptValue::Str(name.clone()),
            None => ScriptValue::Str(unknown.to_string()),
        });
    }
    if let Some(harmful) = i.harmful {
        out.push(ScriptValue::Str(
            if harmful { "DEBUFF" } else { "BUFF" }.to_string(),
        ));
    }
    if let Some(n) = i.dose {
        out.push(int(n));
    }
    out
}

/// The `UNKNOWNOBJECT` global, as the reference's string lookup reads it.
fn unknown_text(lua: &Lua) -> String {
    lua.globals()
        .get::<Option<String>>(UNKNOWN_OBJECT)
        .ok()
        .flatten()
        .unwrap_or_else(|| UNKNOWN_OBJECT.to_string())
}

/// `0x` and sixteen upper-case hex digits.
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
        let unknown = unknown_text(self.lua());
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
            let out = (args(&entry, &unknown), log.passes(&entry));
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
            let unknown = unknown_text(lua);
            let args = {
                let model = super::model_mut_of(lua);
                let log = &model.combat_log;
                log.current
                    .and_then(|id| log.get(id))
                    .map(|e| args(e, &unknown))
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
            event: CombatLogEvent::SwingDamage,
            source: unit(ME_GUID, "Claude", ME),
            dest: unit(MOB_GUID, "Kobold Vermin", MOB),
            spell: None,
            info: CombatLogInfo {
                damage: Some(Damage {
                    amount,
                    school: 1,
                    ..Damage::default()
                }),
                ..CombatLogInfo::default()
            },
        }
    }

    fn dodge(time_ms: u64) -> CombatLogEntry {
        CombatLogEntry {
            time_ms,
            timestamp: 1_000_000.0 + time_ms as f64 * 0.001,
            event: CombatLogEvent::SwingMissed,
            source: unit(MOB_GUID, "Kobold Vermin", MOB),
            dest: unit(ME_GUID, "Claude", ME),
            spell: None,
            info: CombatLogInfo {
                miss: Some(Miss {
                    kind: MissType::Dodge,
                    amount: None,
                }),
                ..CombatLogInfo::default()
            },
        }
    }

    fn fireball() -> CombatLogSpell {
        CombatLogSpell {
            id: 133,
            name: Some("Fireball".into()),
            school: 4,
        }
    }

    /// Every value an entry pushes, as `tostring` prints it.
    fn pushed(s: &mut UiScript, entry: CombatLogEntry) -> String {
        s.push_combat_log(entry);
        s.eval(
            "CombatLogSetCurrentEntry(0)
             local t = {CombatLogGetCurrentEntry()}
             local out = {}
             for i = 1, select('#', CombatLogGetCurrentEntry()) do out[i] = tostring(t[i]) end
             return table.concat(out, ',')",
        )
        .unwrap()
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
        let got = pushed(
            &mut s,
            CombatLogEntry {
                info: CombatLogInfo {
                    damage: Some(Damage {
                        amount: 17,
                        overkill: 3,
                        school: 1,
                        critical: true,
                        ..Damage::default()
                    }),
                    ..CombatLogInfo::default()
                },
                ..hit(7_500, 0)
            },
        );
        assert_eq!(
            got,
            "1000007.5,SWING_DAMAGE,0x00000000000003EE,Claude,1297,0xF1300000060C281E,\
             Kobold Vermin,2632,17,3,1,nil,nil,nil,1,nil,nil"
        );
    }

    /// A spell entry pushes the id, name and school before its damage block (`0x74e290`).
    #[test]
    fn a_spell_hit_carries_its_spell_before_the_damage() {
        let mut s = script();
        let got = pushed(
            &mut s,
            CombatLogEntry {
                event: CombatLogEvent::SpellDamage,
                spell: Some(fireball()),
                info: CombatLogInfo {
                    damage: Some(Damage {
                        amount: 300,
                        school: 4,
                        resisted: 20,
                        critical: true,
                        ..Damage::default()
                    }),
                    ..CombatLogInfo::default()
                },
                ..hit(7_000, 0)
            },
        );
        assert_eq!(
            got,
            "1000007,SPELL_DAMAGE,0x00000000000003EE,Claude,1297,0xF1300000060C281E,\
             Kobold Vermin,2632,133,Fireball,4,300,0,4,20,nil,nil,1,nil,nil"
        );
    }

    /// A heal's block is amount, overheal, absorbed and the crit flag; a missing record pushes a
    /// nil name and school 0.
    #[test]
    fn a_heal_pushes_its_four_values() {
        let mut s = script();
        let got = pushed(
            &mut s,
            CombatLogEntry {
                event: CombatLogEvent::SpellHeal,
                dest: unit(ME_GUID, "Claude", ME),
                spell: Some(CombatLogSpell {
                    id: 9999,
                    name: None,
                    school: 0,
                }),
                info: CombatLogInfo {
                    heal: Some(Heal {
                        amount: 120,
                        overheal: 30,
                        absorbed: 0,
                        critical: false,
                    }),
                    ..CombatLogInfo::default()
                },
                ..hit(7_000, 0)
            },
        );
        assert!(
            got.ends_with(",9999,nil,0,120,30,0,nil"),
            "heal tail, got {got}"
        );
    }

    /// The `0x400`/`0x800` pair: aura type, then the new stack count.
    #[test]
    fn an_aura_dose_pushes_its_type_and_count() {
        let mut s = script();
        let got = pushed(
            &mut s,
            CombatLogEntry {
                event: CombatLogEvent::SpellAuraAppliedDose,
                spell: Some(fireball()),
                info: CombatLogInfo {
                    harmful: Some(true),
                    dose: Some(3),
                    ..CombatLogInfo::default()
                },
                ..hit(7_000, 0)
            },
        );
        assert!(got.ends_with(",133,Fireball,4,DEBUFF,3"), "dose, got {got}");
    }

    /// A dispel names the removed aura as a second spell, then its type (`0x500`).
    #[test]
    fn a_dispel_pushes_the_removed_spell_then_its_type() {
        let mut s = script();
        let got = pushed(
            &mut s,
            CombatLogEntry {
                event: CombatLogEvent::SpellDispel,
                spell: Some(CombatLogSpell {
                    id: 527,
                    name: Some("Dispel Magic".into()),
                    school: 2,
                }),
                info: CombatLogInfo {
                    extra_spell: Some(fireball()),
                    harmful: Some(false),
                    ..CombatLogInfo::default()
                },
                ..hit(7_000, 0)
            },
        );
        assert!(
            got.ends_with(",527,Dispel Magic,2,133,Fireball,4,BUFF"),
            "dispel, got {got}"
        );
    }

    /// An enchant pushes the enchantment's name before the item (`0x1000` before `0x200`), and
    /// an unanswered name reads as `UNKNOWNOBJECT`.
    #[test]
    fn an_enchant_names_the_enchantment_before_the_item() {
        let mut s = script();
        s.run("UNKNOWNOBJECT = 'Unknown'").unwrap();
        let got = pushed(
            &mut s,
            CombatLogEntry {
                event: CombatLogEvent::EnchantApplied,
                source: CombatLogUnit {
                    guid: ME_GUID,
                    name: None,
                    flags: ME,
                },
                dest: CombatLogUnit::none(),
                info: CombatLogInfo {
                    enchant: Some("An enchant".into()),
                    item: Some(CombatLogItem {
                        id: 2504,
                        name: None,
                    }),
                    ..CombatLogInfo::default()
                },
                ..hit(7_000, 0)
            },
        );
        assert_eq!(
            got,
            "1000007,ENCHANT_APPLIED,0x00000000000003EE,Unknown,1297,0x0000000000000000,nil,\
             -2147483648,An enchant,2504,Unknown"
        );
    }

    /// Environmental damage pushes its type word, then the damage block.
    #[test]
    fn environmental_damage_words_its_type() {
        let mut s = script();
        let got = pushed(
            &mut s,
            CombatLogEntry {
                event: CombatLogEvent::EnvironmentalDamage,
                source: CombatLogUnit::none(),
                info: CombatLogInfo {
                    environmental: Some(2),
                    damage: Some(Damage {
                        amount: 23,
                        school: 1,
                        ..Damage::default()
                    }),
                    ..CombatLogInfo::default()
                },
                ..hit(7_000, 0)
            },
        );
        assert!(
            got.contains(",2632,FALLING,23,0,1,nil,nil,nil,nil,nil,nil"),
            "environmental, got {got}"
        );
    }

    #[test]
    fn a_miss_carries_its_word_and_a_stopped_amount() {
        let mut s = script();
        s.push_combat_log(dodge(6_000));
        s.push_combat_log(CombatLogEntry {
            info: CombatLogInfo {
                miss: Some(Miss {
                    kind: MissType::Absorb,
                    amount: Some(40),
                }),
                ..CombatLogInfo::default()
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
