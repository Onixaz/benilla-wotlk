//! The trainer bindings the stock `Blizzard_TrainerUI` window calls, over the snapshot the app
//! pushes ([`UiScript::set_trainer`]). The reference's trainer keeps one array of service and
//! header records, re-sorted by its filter-and-sort (`0x594ba0`) with the visible rows first; every
//! Lua index is 1-based into that whole array, and `GetNumTrainerServices` (`0x593d10`) counts only
//! the visible rows.

use std::collections::HashSet;

use mlua::{Lua, MultiValue, Value};

use super::binding_abi;
use super::Model;

/// A service's state byte (`0x594260`): green learnable, red gated, gray known.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrainerServiceCategory {
    #[default]
    Available,
    Unavailable,
    Used,
}

impl TrainerServiceCategory {
    /// `GetTrainerServiceInfo`'s type string (`0x594260`).
    fn type_str(self) -> &'static str {
        match self {
            TrainerServiceCategory::Available => "available",
            TrainerServiceCategory::Unavailable => "unavailable",
            TrainerServiceCategory::Used => "used",
        }
    }

    /// The state byte, which is also the service's bit in `serviceTypeFilter`.
    fn state(self) -> u32 {
        match self {
            TrainerServiceCategory::Available => 0,
            TrainerServiceCategory::Unavailable => 1,
            TrainerServiceCategory::Used => 2,
        }
    }
}

/// The service type parse (`0x593db0`): the filter bit a type string names.
fn type_from_str(s: &str) -> Option<u32> {
    match s.to_ascii_lowercase().as_str() {
        "available" => Some(0),
        "unavailable" => Some(1),
        "used" => Some(2),
        _ => None,
    }
}

/// A service's skill gate (`GetTrainerServiceSkillReq` `0x595470`); `met` is the player's skill
/// value against the rank, no slot for the line failing it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrainerSkillReq {
    pub name: String,
    pub rank: u32,
    pub met: bool,
}

/// One nonzero ability slot (`GetTrainerServiceAbilityReq` `0x5955e0`): "name (rank)", `None` for
/// a spell with no record, and whether the player knows it or a higher rank.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrainerAbilityReq {
    pub name: Option<String>,
    pub met: bool,
}

/// What `GameTooltip:SetTrainerService` describes for a row, resolved by the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrainerTooltip {
    /// The item builder on this item id; 0 or an uncached template renders empty.
    Item(u32),
    /// The spell builder on the taught spell, or on the wire spell when no learn slot resolved.
    Spell {
        spell_id: u32,
        /// The builder's altCaster argument, set for a `SPELL_EFFECT_LEARN_PET_SPELL` slot.
        alt_caster: bool,
    },
}

impl Default for TrainerTooltip {
    fn default() -> Self {
        TrainerTooltip::Spell {
            spell_id: 0,
            alt_caster: false,
        }
    }
}

/// One service, resolved by the app from the wire record; its place in
/// [`TrainerState::services`] is wire order, not display order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrainerService {
    /// The wire spell id, which `CMSG_TRAINER_BUY_SPELL` names.
    pub spell_id: u32,
    /// The wire spell's name; `None` when it has no `Spell.dbc` record, which answers nil.
    pub name: Option<String>,
    /// The wire spell's rank text, such as "Rank 2".
    pub subtext: Option<String>,
    pub texture: Option<String>,
    /// `GetTrainerServiceDescription` (`0x5959d0`); `None` answers nil.
    pub description: Option<String>,
    /// In copper, already reputation-discounted by the server.
    pub cost: u32,
    /// The wire's talent-point cost word (`0x6d12c0`), returned raw.
    pub talent_cost: u32,
    /// The wire's profession-point cost word; the stock window confirms a service that has one.
    pub profession_cost: u32,
    pub category: TrainerServiceCategory,
    pub level_req: u32,
    pub skill_req: Option<TrainerSkillReq>,
    /// The wire's three ability slots, `None` where the slot is 0.
    pub ability_reqs: [Option<TrainerAbilityReq>; 3],
    /// The wire spell has a `SKILL_STEP` (44) effect (`IsTrainerServiceSkillStep` `0x595b60`).
    pub skill_step: bool,
    /// `GetTrainerServiceStepIncrease` (`0x595c40`) for a step not yet known.
    pub step_increase: Option<String>,
    /// `GetTrainerServiceSkillLine` (`0x5952f0`): the line the first resolvable effect names.
    pub skill_line: Option<String>,
    /// `GetTrainerServiceItemLink` (`0x596290`): a tradeskill recipe's cached product.
    pub item_link: Option<String>,
    /// The tree group: the taught spell's `SkillLine` (0 drops the service); at a tradeskill
    /// trainer 3 passive, 1 skill step, else 2 (`0x596450`).
    pub group_key: u32,
    /// The header text: the skill line's name, or a tradeskill trainer's
    /// `TRADESKILL_SERVICE_*` string (`0x594e80`).
    pub group_name: String,
    pub tooltip: TrainerTooltip,
}

impl TrainerService {
    /// Costs talent or profession points, the line key `unk_0024` sums (`0x596450`).
    fn costs_points(&self) -> bool {
        self.talent_cost != 0 || self.profession_cost != 0
    }
}

/// One header and its services, built by [`UiScript::set_trainer`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrainerGroup {
    /// Also the collapse and line-filter key.
    pub key: u32,
    pub name: String,
    /// Positions into [`TrainerState::services`], in display order.
    pub services: Vec<usize>,
}

/// The open trainer, pushed whole by the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrainerState {
    pub services: Vec<TrainerService>,
    /// `SMSG_TRAINER_LIST`'s trailing string.
    pub greeting: String,
    /// `SMSG_TRAINER_LIST`'s trainer type: 2 is a tradeskill trainer.
    pub trainer_type: u32,
    /// Built by [`UiScript::set_trainer`], not pushed by the app.
    pub groups: Vec<TrainerGroup>,
}

impl super::UiScript {
    /// Push the open trainer, or clear it with `None`. A push builds the tree ([`build_groups`]);
    /// one whose services or states moved re-runs the filter-and-sort's selection pick, as the
    /// list builder and the re-evaluator (`0x596b00`) both do.
    pub fn set_trainer(&mut self, state: Option<TrainerState>) {
        let mut model = self.model_mut();
        match state {
            None => {
                model.trainer_selection = None;
                model.trainer_collapsed.clear();
                model.trainer_lines_off.clear();
                model.trainer = None;
            }
            Some(mut s) => {
                s.groups = build_groups(&s.services, s.trainer_type);
                let key = |t: &TrainerState| -> Vec<(u32, TrainerServiceCategory)> {
                    t.services
                        .iter()
                        .map(|v| (v.spell_id, v.category))
                        .collect()
                };
                let moved = model.trainer.as_ref().is_none_or(|old| key(old) != key(&s));
                model.trainer = Some(s);
                if moved {
                    pick_selection(&mut model);
                }
            }
        }
    }

    /// Reset what each `SMSG_TRAINER_LIST` resets (`0x596450`): every skill line
    /// expanded and passing the line filter. The state filter is the `serviceTypeFilter` CVar,
    /// which no list touches. Call it per packet, never per snapshot.
    pub fn reset_trainer_list_state(&mut self) {
        let mut model = self.model_mut();
        model.trainer_collapsed.clear();
        model.trainer_lines_off.clear();
    }

    /// Drain the spell ids `BuyTrainerService` queued, each already resolved from its row.
    pub fn take_trainer_buys(&mut self) -> Vec<u32> {
        drain_field!(self, trainer_buys)
    }

    /// Whether `CloseTrainer` was called since the last drain; the close sends no packet.
    pub fn take_trainer_close(&mut self) -> bool {
        drain_field!(self, trainer_close)
    }
}

/// One record: a header by group index, or a service by its position in
/// [`TrainerState::services`].
#[derive(Clone, Copy)]
enum Row {
    Header(usize),
    Service(usize),
}

/// Folds case, raw bytes breaking ties, as the reference's case-insensitive UTF-8 sort.
fn collate(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

const TRAINER_TYPE_TRADESKILL: u32 = 2;

/// The record's required-skill value, 0 with no skill gate.
fn skill_value(s: &TrainerService) -> u32 {
    s.skill_req.as_ref().map_or(0, |r| r.rank)
}

/// The class trainer's service order (`0x5947d0`): required level, skill value, name, then rank.
fn class_order(a: &TrainerService, b: &TrainerService) -> std::cmp::Ordering {
    let name = |s: &TrainerService| s.name.clone().unwrap_or_default();
    let rank = |s: &TrainerService| s.subtext.clone().unwrap_or_default();
    a.level_req
        .cmp(&b.level_req)
        .then_with(|| skill_value(a).cmp(&skill_value(b)))
        .then_with(|| collate(&name(a), &name(b)))
        .then_with(|| collate(&rank(a), &rank(b)))
}

/// The tradeskill trainer's service order (`0x5949a0`): required skill value, then name.
fn tradeskill_order(a: &TrainerService, b: &TrainerService) -> std::cmp::Ordering {
    let name = |s: &TrainerService| s.name.clone().unwrap_or_default();
    skill_value(a)
        .cmp(&skill_value(b))
        .then_with(|| collate(&name(a), &name(b)))
}

/// Group the services by [`TrainerService::group_key`], dropping key 0, and sort each group's rows
/// by the type's comparator. Lines sort by raw key at a tradeskill trainer (`0x593ce0`); elsewhere
/// a line whose every service costs points goes last, then by name (`0x5941b0`).
fn build_groups(services: &[TrainerService], trainer_type: u32) -> Vec<TrainerGroup> {
    let mut map: std::collections::HashMap<u32, (TrainerGroup, bool)> =
        std::collections::HashMap::new();
    for (i, s) in services.iter().enumerate() {
        if s.group_key == 0 {
            continue;
        }
        let (g, all_points) = map.entry(s.group_key).or_insert_with(|| {
            (
                TrainerGroup {
                    key: s.group_key,
                    name: s.group_name.clone(),
                    services: Vec::new(),
                },
                true,
            )
        });
        g.services.push(i);
        *all_points &= s.costs_points();
    }
    let mut groups: Vec<(TrainerGroup, bool)> = map.into_values().collect();
    let order = if trainer_type == TRAINER_TYPE_TRADESKILL {
        tradeskill_order
    } else {
        class_order
    };
    for (g, _) in &mut groups {
        g.services
            .sort_by(|&a, &b| order(&services[a], &services[b]));
    }
    if trainer_type == TRAINER_TYPE_TRADESKILL {
        groups.sort_by_key(|(g, _)| g.key);
    } else {
        groups.sort_by(|(a, pa), (b, pb)| {
            pa.cmp(pb)
                .then_with(|| collate(&a.name, &b.name))
                .then_with(|| a.key.cmp(&b.key))
        });
    }
    groups.into_iter().map(|(g, _)| g).collect()
}

/// The `serviceTypeFilter` CVar, which `0x594ba0` reads; its registered default is 3.
fn type_filter(model: &Model) -> u32 {
    model
        .cvars
        .get("servicetypefilter")
        .map_or(3, |c| super::cvars::sstr_to_int(&c.value) as u32)
}

/// The whole record array and its visible count, as the filter-and-sort (`0x594ba0`) leaves
/// it: a line shows when the line filter passes it and it holds a service in a filtered state; a
/// service shows when its state passes and its line shows and is expanded; visible records sort
/// first, by line, header first. The hidden tail's order is tree order, which `qsort` leaves
/// unspecified.
fn rows(model: &Model) -> (Vec<Row>, usize) {
    let Some(t) = model.trainer.as_ref() else {
        return (Vec::new(), 0);
    };
    let filter = type_filter(model);
    let passes = |si: usize| filter & (1 << t.services[si].category.state()) != 0;
    let mut shown = Vec::new();
    let mut tail = Vec::new();
    for (gi, g) in t.groups.iter().enumerate() {
        let line_shown =
            !model.trainer_lines_off.contains(&g.key) && g.services.iter().any(|&si| passes(si));
        let expanded = !model.trainer_collapsed.contains(&g.key);
        if line_shown {
            shown.push(Row::Header(gi));
        } else {
            tail.push(Row::Header(gi));
        }
        for &si in &g.services {
            if passes(si) && line_shown && expanded {
                shown.push(Row::Service(si));
            } else {
                tail.push(Row::Service(si));
            }
        }
    }
    let visible = shown.len();
    shown.extend(tail);
    (shown, visible)
}

/// The filter-and-sort's selection (`0x594ba0`): the first visible available service, else
/// the record at array index 1, none when that is a header.
fn pick_selection(model: &mut Model) {
    let (all, visible) = rows(model);
    let Some(t) = model.trainer.as_ref() else {
        model.trainer_selection = None;
        return;
    };
    let service = |r: &Row| match r {
        Row::Service(si) => Some(&t.services[*si]),
        Row::Header(_) => None,
    };
    let first = all[..visible]
        .iter()
        .filter_map(service)
        .find(|s| s.category == TrainerServiceCategory::Available && s.spell_id > 0);
    let pick = first.or_else(|| all.get(1).and_then(service));
    model.trainer_selection = pick.map(|s| s.spell_id);
}

/// The service at a 1-based record index; `None` for a header or past the end.
/// `GameTooltip:SetTrainerService` resolves its index through this too.
pub(super) fn service(model: &Model, index: i64) -> Option<&TrainerService> {
    let n = usize::try_from(index.checked_sub(1)?).ok()?;
    match rows(model).0.get(n)? {
        Row::Service(si) => model.trainer.as_ref()?.services.get(*si),
        Row::Header(_) => None,
    }
}

/// `GetTrainerSelectionIndex` (`0x594430`): the selected spell's 1-based record index, which may
/// sit in the hidden tail; `None` when nothing is selected or it left the list.
fn selected_row(model: &Model) -> Option<usize> {
    let want = model.trainer_selection?;
    let t = model.trainer.as_ref()?;
    rows(model)
        .0
        .iter()
        .position(|r| match r {
            Row::Service(si) => t.services.get(*si).is_some_and(|s| s.spell_id == want),
            Row::Header(_) => false,
        })?
        .checked_add(1)
}

/// Re-sort and signal `TRAINER_UPDATE`, as every mask commit does (`0x594fe0`, `0x595010`, the
/// collapse verbs).
fn filter_and_sort(model: &mut Model) {
    pick_selection(model);
    model
        .pending_events
        .push(("TRAINER_UPDATE".to_string(), Vec::new()));
}

/// A number argument read with `lua_tonumber`, raising `usage` on anything else.
fn number_arg(v: &Value, usage: &'static str) -> mlua::Result<i64> {
    match v {
        Value::Integer(i) => Ok(*i),
        Value::Number(n) => Ok(*n as i64),
        Value::String(s) => s
            .to_str()
            .ok()
            .and_then(|t| t.trim().parse::<f64>().ok())
            .map(|n| n as i64)
            .ok_or_else(|| mlua::Error::runtime(usage)),
        _ => Err(mlua::Error::runtime(usage)),
    }
}

/// The optional exclusive argument: a nonzero number.
fn exclusive_arg(v: &Value) -> bool {
    number_arg(v, "").is_ok_and(|e| e != 0)
}

/// The group key of the header at a 1-based record index, `None` when the record is no header.
fn header_key(model: &Model, index: i64) -> Option<u32> {
    let n = usize::try_from(index.checked_sub(1)?).ok()?;
    match rows(model).0.get(n)? {
        Row::Header(gi) => model.trainer.as_ref()?.groups.get(*gi).map(|g| g.key),
        Row::Service(_) => None,
    }
}

/// A skill line's key by its 1-based line index, in skill-line order.
fn line_key(model: &Model, index: i64) -> Option<u32> {
    let n = usize::try_from(index.checked_sub(1)?).ok()?;
    model.trainer.as_ref()?.groups.get(n).map(|g| g.key)
}

/// Every line key but `keep`.
fn other_lines(model: &Model, keep: Option<u32>) -> HashSet<u32> {
    model
        .trainer
        .as_ref()
        .map(|t| {
            t.groups
                .iter()
                .map(|g| g.key)
                .filter(|&k| Some(k) != keep)
                .collect()
        })
        .unwrap_or_default()
}

fn opt_str(lua: &Lua, s: Option<&str>) -> mlua::Result<Value> {
    Ok(match s {
        Some(s) => Value::String(lua.create_string(s)?),
        None => Value::Nil,
    })
}

fn read(lua: &Lua) -> mlua::AppDataRef<'_, crate::script::ModelCell> {
    lua.app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data")
}

/// Register a getter over the service at its 1-based index argument.
fn getter(
    lua: &Lua,
    name: &str,
    usage: &'static str,
    f: fn(&Lua, Option<&TrainerService>) -> mlua::Result<Value>,
) -> mlua::Result<()> {
    lua.globals().set(
        name,
        lua.create_function(move |lua, index: Value| {
            let index = number_arg(&index, usage)?;
            let model = read(lua);
            f(lua, service(&model, index))
        })?,
    )
}

/// Register the trainer globals: `s_ScriptFunctions` (`0xacf3e8`) less `OpenTrainer`.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetNumTrainerServices",
        lua.create_function(|lua, ()| Ok(rows(&read(lua)).1 as i64))?,
    )?;

    // GetTrainerServiceInfo(index) → name, subText, serviceType, isExpanded (`0x595090`): a header
    // reads "" for its subtext, and isExpanded is 1 for every record but a collapsed header.
    g.set(
        "GetTrainerServiceInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(&index, "Usage: GetTrainerServiceInfo(index)")?;
            let model = read(lua);
            let row = usize::try_from(index - 1)
                .ok()
                .and_then(|n| rows(&model).0.get(n).copied());
            let (name, sub, kind, expanded) = match (row, model.trainer.as_ref()) {
                (Some(Row::Header(gi)), Some(t)) => {
                    let g = &t.groups[gi];
                    let expanded = !model.trainer_collapsed.contains(&g.key);
                    (Some(g.name.as_str()), Some(""), Some("header"), expanded)
                }
                (Some(Row::Service(si)), Some(t)) => {
                    let s = &t.services[si];
                    let sub = s.name.as_ref().map(|_| s.subtext.as_deref().unwrap_or(""));
                    (s.name.as_deref(), sub, Some(s.category.type_str()), true)
                }
                _ => (None, None, None, true),
            };
            Ok(MultiValue::from_vec(vec![
                opt_str(lua, name)?,
                opt_str(lua, sub)?,
                opt_str(lua, kind)?,
                binding_abi::flag(expanded),
            ]))
        })?,
    )?;

    getter(
        lua,
        "GetTrainerServiceSkillLine",
        "Usage: GetTrainerServiceSkillLine(index)",
        |lua, s| opt_str(lua, s.and_then(|s| s.skill_line.as_deref())),
    )?;
    getter(
        lua,
        "GetTrainerServiceIcon",
        "Usage: GetTrainerServiceIcon(index)",
        |lua, s| opt_str(lua, s.and_then(|s| s.texture.as_deref())),
    )?;
    getter(
        lua,
        "GetTrainerServiceDescription",
        "Usage: GetTrainerServiceDescription(index)",
        |lua, s| opt_str(lua, s.and_then(|s| s.description.as_deref())),
    )?;
    getter(
        lua,
        "GetTrainerServiceLevelReq",
        "Usage: GetTrainerServiceLevelReq(index)",
        |_, s| Ok(Value::Integer(s.map_or(0, |s| i64::from(s.level_req)))),
    )?;
    getter(
        lua,
        "GetTrainerServiceNumAbilityReq",
        "Usage: GetTrainerServiceAbilityReq(index)",
        |_, s| {
            let n = s.map_or(0, |s| s.ability_reqs.iter().flatten().count());
            Ok(Value::Integer(n as i64))
        },
    )?;
    getter(
        lua,
        "IsTrainerServiceSkillStep",
        "Usage: IsTrainerServiceSkillStep(index)",
        |_, s| Ok(binding_abi::flag(s.is_some_and(|s| s.skill_step))),
    )?;
    // Nil for a known step: `0x595c40` gates on state 2.
    getter(
        lua,
        "GetTrainerServiceStepIncrease",
        "Usage: GetTrainerServiceStepIncrease(index)",
        |lua, s| {
            let text = s
                .filter(|s| s.category != TrainerServiceCategory::Used)
                .and_then(|s| s.step_increase.as_deref());
            opt_str(lua, text)
        },
    )?;
    getter(
        lua,
        "GetTrainerServiceItemLink",
        "Usage: GetTrainerServiceItemLink(index)",
        |lua, s| opt_str(lua, s.and_then(|s| s.item_link.as_deref())),
    )?;

    // GetTrainerServiceCost(index) → money, talentCost, professionCost, the wire's words raw.
    g.set(
        "GetTrainerServiceCost",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(&index, "Usage: GetTrainerServiceCost(index)")?;
            let model = read(lua);
            let (money, talent, prof) = service(&model, index)
                .map(|s| (s.cost, s.talent_cost, s.profession_cost))
                .unwrap_or((0, 0, 0));
            Ok(MultiValue::from_vec(vec![
                Value::Integer(i64::from(money)),
                Value::Integer(i64::from(talent)),
                Value::Integer(i64::from(prof)),
            ]))
        })?,
    )?;

    // GetTrainerServiceSkillReq(index) → skill, rank, hasReq; (nil, 0, 1) with no gate.
    g.set(
        "GetTrainerServiceSkillReq",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(&index, "Usage: GetTrainerServiceSkillReq(index)")?;
            let model = read(lua);
            let req = service(&model, index).and_then(|s| s.skill_req.as_ref());
            Ok(MultiValue::from_vec(match req {
                Some(r) if !r.name.is_empty() => vec![
                    Value::String(lua.create_string(&r.name)?),
                    Value::Integer(i64::from(r.rank)),
                    binding_abi::flag(r.met),
                ],
                r => vec![
                    Value::Nil,
                    Value::Integer(0),
                    binding_abi::flag(r.is_none_or(|r| r.met)),
                ],
            }))
        })?,
    )?;

    // GetTrainerServiceAbilityReq(index, slot) → name, hasReq over the raw wire slot, so an empty
    // slot reads (nil, 1).
    g.set(
        "GetTrainerServiceAbilityReq",
        lua.create_function(|lua, (index, slot): (Value, Value)| {
            const USAGE: &str = "Usage: GetTrainerServiceAbilityReq(index)";
            let index = number_arg(&index, USAGE)?;
            let slot = number_arg(&slot, USAGE)?;
            let model = read(lua);
            let req = service(&model, index).and_then(|s| {
                usize::try_from(slot - 1)
                    .ok()
                    .and_then(|n| s.ability_reqs.get(n))
                    .and_then(Option::as_ref)
            });
            Ok(MultiValue::from_vec(vec![
                opt_str(lua, req.and_then(|r| r.name.as_deref()))?,
                binding_abi::flag(req.is_none_or(|r| r.met)),
            ]))
        })?,
    )?;

    // GetTrainerServiceStepReq(index) → (nil, 1): the list handler zeroes every step spell
    // (`0x6d12c0`), so `0x5957d0` never finds one.
    g.set(
        "GetTrainerServiceStepReq",
        lua.create_function(|_, index: Value| {
            number_arg(&index, "Usage: GetTrainerServiceStepReq(index)")?;
            Ok(MultiValue::from_vec(vec![
                Value::Nil,
                binding_abi::flag(true),
            ]))
        })?,
    )?;

    g.set(
        "IsTradeskillTrainer",
        lua.create_function(|lua, ()| {
            let model = read(lua);
            Ok(binding_abi::flag(model.trainer.as_ref().is_some_and(|t| {
                t.trainer_type == TRAINER_TYPE_TRADESKILL
            })))
        })?,
    )?;

    g.set(
        "GetTrainerGreetingText",
        lua.create_function(|lua, ()| {
            let model = read(lua);
            lua.create_string(model.trainer.as_ref().map_or("", |t| t.greeting.as_str()))
        })?,
    )?;

    g.set(
        "GetTrainerServiceTypeFilter",
        lua.create_function(|lua, kind: Value| {
            let Value::String(kind) = kind else {
                return Err(mlua::Error::runtime(
                    "Usage: GetTrainerServiceTypeFilter(\"type\")",
                ));
            };
            let bit = type_from_str(&kind.to_string_lossy()).ok_or_else(|| {
                mlua::Error::runtime("Bad service type in GetTrainerServiceTypeFilter")
            })?;
            Ok(binding_abi::flag(type_filter(&read(lua)) & (1 << bit) != 0))
        })?,
    )?;

    // SetTrainerServiceTypeFilter("type", on [, exclusive]) (`0x595ec0`): "all" stores 7; off
    // clears the bit, exclusive stores the bit alone, else it is added. Every call commits.
    g.set(
        "SetTrainerServiceTypeFilter",
        lua.create_function(|lua, (kind, on, exclusive): (Value, Value, Value)| {
            let Value::String(kind) = kind else {
                return Err(mlua::Error::runtime(
                    "Usage: SetTrainerServiceTypeFilter(\"type\", on/off [, exclusive])",
                ));
            };
            let kind = kind.to_string_lossy();
            let mut model = crate::script::model_mut_of(lua);
            let value = if kind.eq_ignore_ascii_case("all") {
                7
            } else {
                let bit = type_from_str(&kind).ok_or_else(|| {
                    mlua::Error::runtime("Bad service type in SetTrainerServiceTypeFilter")
                })?;
                let on = number_arg(
                    &on,
                    "Missing on/off parameter in SetTrainerServiceTypeFilter",
                )?;
                let filter = type_filter(&model);
                if on == 0 {
                    filter & !(1 << bit)
                } else if exclusive_arg(&exclusive) {
                    1 << bit
                } else {
                    filter | (1 << bit)
                }
            };
            super::cvars::set_from_engine(&mut model, "serviceTypeFilter", value.to_string());
            filter_and_sort(&mut model);
            Ok(())
        })?,
    )?;

    // GetTrainerSkillLines() → each line's `SkillLine` name (`0x594650`); a tradeskill trainer's
    // keys 1-3 name no skill line, so they answer nil.
    g.set(
        "GetTrainerSkillLines",
        lua.create_function(|lua, ()| {
            let model = read(lua);
            let Some(t) = model.trainer.as_ref() else {
                return Ok(MultiValue::new());
            };
            let tradeskill = t.trainer_type == TRAINER_TYPE_TRADESKILL;
            t.groups
                .iter()
                .map(|g| opt_str(lua, (!tradeskill).then_some(g.name.as_str())))
                .collect::<mlua::Result<Vec<_>>>()
                .map(MultiValue::from_vec)
        })?,
    )?;

    // GetTrainerSkillLineFilter(index) (`0x593eb0`): 0 or below asks whether every line passes.
    g.set(
        "GetTrainerSkillLineFilter",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(&index, "Usage: GetTrainerSkillLineFilter(index)")?;
            let model = read(lua);
            if index < 1 {
                return Ok(binding_abi::flag(model.trainer_lines_off.is_empty()));
            }
            let key = line_key(&model, index).ok_or_else(|| {
                mlua::Error::runtime("Bad skill line in GetTrainerSkillLineFilter")
            })?;
            Ok(binding_abi::flag(!model.trainer_lines_off.contains(&key)))
        })?,
    )?;

    // SetTrainerSkillLineFilter(index [, on, exclusive]) (`0x596010`): 0 or below passes every
    // line; otherwise the type filter's on and exclusive legs.
    g.set(
        "SetTrainerSkillLineFilter",
        lua.create_function(|lua, (index, on, exclusive): (Value, Value, Value)| {
            let index = number_arg(
                &index,
                "Usage: SetTrainerSkillLineFilter(index [, on\\off, exclusive])",
            )?;
            let mut model = crate::script::model_mut_of(lua);
            if index < 1 {
                model.trainer_lines_off.clear();
            } else {
                let key = line_key(&model, index).ok_or_else(|| {
                    mlua::Error::runtime("Bad skill line in SetTrainerSkillLineFilter")
                })?;
                let on = number_arg(
                    &on,
                    "Missing on//off parameter in SetTrainerSkillLineFilter",
                )?;
                if on == 0 {
                    model.trainer_lines_off.insert(key);
                } else if exclusive_arg(&exclusive) {
                    model.trainer_lines_off = other_lines(&model, Some(key));
                } else {
                    model.trainer_lines_off.remove(&key);
                }
            }
            filter_and_sort(&mut model);
            Ok(())
        })?,
    )?;

    // Collapse/ExpandTrainerSkillLine(index) (`0x596150`, `0x5961f0`): the line headed by the
    // record at `index`, raising on any other record; 0 or below folds or opens every line.
    for (name, collapse, usage, bad) in [
        (
            "CollapseTrainerSkillLine",
            true,
            "Usage: CollapseTrainerSkillLine(index)",
            "Bad skill line in CollapseTrainerSkillLine",
        ),
        (
            "ExpandTrainerSkillLine",
            false,
            "Usage: ExpandTrainerSkillLine(index)",
            "Bad skill line in ExpandTrainerSkillLine",
        ),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, index: Value| {
                let index = number_arg(&index, usage)?;
                let mut model = crate::script::model_mut_of(lua);
                if index < 1 {
                    model.trainer_collapsed = if collapse {
                        other_lines(&model, None)
                    } else {
                        HashSet::new()
                    };
                } else {
                    let key = header_key(&model, index).ok_or_else(|| mlua::Error::runtime(bad))?;
                    if collapse {
                        model.trainer_collapsed.insert(key);
                    } else {
                        model.trainer_collapsed.remove(&key);
                    }
                }
                filter_and_sort(&mut model);
                Ok(())
            })?,
        )?;
    }

    // SelectTrainerService(index) (`0x5943a0`): a header or past the end selects nothing.
    g.set(
        "SelectTrainerService",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(&index, "Usage: SelectTrainerService(index)")?;
            let mut model = crate::script::model_mut_of(lua);
            model.trainer_selection = service(&model, index).map(|s| s.spell_id);
            Ok(())
        })?,
    )?;

    g.set(
        "GetTrainerSelectionIndex",
        lua.create_function(|lua, ()| Ok(selected_row(&read(lua)).map(|r| r as i64)))?,
    )?;

    // BuyTrainerService(index): queue the record's spell id when its state is 0
    // (`TrainerBuySpell` `0x594da0`). Deviation: 0 or below is a no-op, because one stray call
    // would spend the purse; the reference buys every visible service (`0x594e50`).
    g.set(
        "BuyTrainerService",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(&index, "Usage: BuyTrainerService(index)")?;
            let mut model = crate::script::model_mut_of(lua);
            let buyable = service(&model, index)
                .filter(|s| s.category == TrainerServiceCategory::Available)
                .map(|s| s.spell_id);
            if let Some(spell_id) = buyable {
                model.trainer_buys.push(spell_id);
            }
            Ok(())
        })?,
    )?;

    // CloseTrainer(): sends no packet; the app clears its state on the flag.
    g.set(
        "CloseTrainer",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.trainer_close = true;
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests;
