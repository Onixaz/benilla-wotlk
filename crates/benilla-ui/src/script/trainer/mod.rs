//! The trainer bindings the stock `Blizzard_TrainerUI` window calls. The app's list handler builds
//! the window (`0x596450`) and its re-evaluator re-sorts it (`0x596b00`); the verbs read it through
//! the lent view at the call, the player-side answers too (`0x595470`, `0x5955e0`, `0x5959d0`),
//! and the filter, collapse, select, buy and close verbs act inside their call. The window keeps
//! one array of service and header records, re-sorted by its filter-and-sort (`0x594ba0`) with the
//! visible rows first; every Lua index is 1-based into that whole array, and
//! `GetNumTrainerServices` (`0x593d10`) counts only the visible rows.

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

/// What a service's getters read off the player and the item cache at the call: the skill and
/// ability gates' met flags (`0x595470`, `0x5955e0`) and a recipe's cached product link
/// (`0x596290`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrainerServiceLive {
    pub skill_req: Option<TrainerSkillReq>,
    pub ability_reqs: [Option<TrainerAbilityReq>; 3],
    pub item_link: Option<String>,
}

/// The trainer window's globals: the records and their tree, the line filter and collapse masks,
/// the `serviceTypeFilter` value the last filter-and-sort read (`0x594ba0`), the visible count it
/// left, which `GetNumTrainerServices` answers (`0x593d10`) and the setter's zero keeps, and the
/// selection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrainerWindow {
    pub state: TrainerState,
    pub selection: Option<u32>,
    pub collapsed: HashSet<u32>,
    pub lines_off: HashSet<u32>,
    pub filter: u32,
    pub filtered: usize,
}

impl TrainerWindow {
    /// The list build (`0x596450`): the tree from each service's group key, every line passing the
    /// line filter and expanded, then the filter-and-sort over `filter`.
    pub fn listed(mut state: TrainerState, filter: u32) -> Self {
        state.groups = build_groups(&state.services, state.trainer_type);
        let mut w = TrainerWindow {
            state,
            ..Default::default()
        };
        w.filter_and_sort(filter);
        w
    }

    /// The filter-and-sort (`0x594ba0`): the type filter read now, then the selection's pick, which
    /// every list, re-evaluation and mask commit runs.
    pub fn filter_and_sort(&mut self, filter: u32) {
        self.filter = filter;
        pick_selection(self);
    }
}

/// `serviceTypeFilter` as the filter-and-sort reads it (`0x594ba0`).
pub fn trainer_type_filter(script: &super::UiScript) -> u32 {
    type_filter(&script.model_ref())
}

impl super::UiScript {
    /// A test's trainer, which the verbs read where the lent view does not answer it: a push builds
    /// the tree ([`build_groups`]) and keeps the masks; one whose services or states moved re-runs
    /// the selection pick.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn set_trainer(&mut self, state: Option<TrainerState>) {
        let filter = type_filter(&self.model_ref());
        let mut model = self.model_mut();
        match state {
            None => model.trainer = None,
            Some(mut s) => {
                s.groups = build_groups(&s.services, s.trainer_type);
                let key = |t: &TrainerState| -> Vec<(u32, TrainerServiceCategory)> {
                    t.services
                        .iter()
                        .map(|v| (v.spell_id, v.category))
                        .collect()
                };
                let fresh = model.trainer.is_none();
                let w = model.trainer.get_or_insert_with(TrainerWindow::default);
                let moved = fresh || key(&w.state) != key(&s);
                w.state = s;
                w.filter = filter;
                if moved {
                    pick_selection(w);
                }
                w.filtered = rows(w).1;
            }
        }
    }

    /// A test's list reset (`0x596450`): every skill line expanded and passing the line filter.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn reset_trainer_list_state(&mut self) {
        if let Some(w) = self.model_mut().trainer.as_mut() {
            w.collapsed.clear();
            w.lines_off.clear();
        }
    }
}

/// The window: the lent view's at the call; with none, a test's, else none.
fn with_window<R>(lua: &Lua, f: impl FnOnce(Option<&TrainerWindow>) -> R) -> R {
    let mut f = Some(f);
    if let Some(Some(r)) =
        super::live::read_live(|l| l.trainer().map(|w| (f.take().expect("once"))(w)))
    {
        return r;
    }
    #[cfg(any(test, feature = "fixtures"))]
    {
        let model = read(lua);
        (f.take().expect("once"))(model.trainer.as_ref())
    }
    #[cfg(not(any(test, feature = "fixtures")))]
    {
        let _ = lua;
        (f.take().expect("once"))(None)
    }
}

/// The window mutably inside the call: the lent view's; with none, a test's, else none.
fn with_window_mut<R>(lua: &Lua, f: impl FnOnce(Option<&mut TrainerWindow>) -> R) -> R {
    let mut f = Some(f);
    if let Some(Some(r)) =
        super::live::write_live(|l| l.trainer_mut().map(|w| (f.take().expect("once"))(w)))
    {
        return r;
    }
    #[cfg(any(test, feature = "fixtures"))]
    {
        let mut model = crate::script::model_mut_of(lua);
        (f.take().expect("once"))(model.trainer.as_mut())
    }
    #[cfg(not(any(test, feature = "fixtures")))]
    {
        let _ = lua;
        (f.take().expect("once"))(None)
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
fn rows(w: &TrainerWindow) -> (Vec<Row>, usize) {
    let t = &w.state;
    let filter = w.filter;
    let passes = |si: usize| filter & (1 << t.services[si].category.state()) != 0;
    let mut shown = Vec::new();
    let mut tail = Vec::new();
    for (gi, g) in t.groups.iter().enumerate() {
        let line_shown = !w.lines_off.contains(&g.key) && g.services.iter().any(|&si| passes(si));
        let expanded = !w.collapsed.contains(&g.key);
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
fn pick_selection(w: &mut TrainerWindow) {
    let (all, visible) = rows(w);
    w.filtered = visible;
    let t = &w.state;
    let service = |r: &Row| match r {
        Row::Service(si) => Some(&t.services[*si]),
        Row::Header(_) => None,
    };
    let first = all[..visible]
        .iter()
        .filter_map(service)
        .find(|s| s.category == TrainerServiceCategory::Available && s.spell_id > 0);
    let pick = first.or_else(|| all.get(1).and_then(service));
    w.selection = pick.map(|s| s.spell_id);
}

/// The service and its place in [`TrainerState::services`] at a 1-based record index; `None` for
/// a header or past the end.
fn service(w: &TrainerWindow, index: i64) -> Option<(usize, &TrainerService)> {
    let n = usize::try_from(index.checked_sub(1)?).ok()?;
    match rows(w).0.get(n)? {
        Row::Service(si) => Some((*si, w.state.services.get(*si)?)),
        Row::Header(_) => None,
    }
}

/// The service at a 1-based record index, read at the call, with its place.
fn service_of(lua: &Lua, index: i64) -> Option<(usize, TrainerService)> {
    with_window(lua, |w| service(w?, index).map(|(si, s)| (si, s.clone())))
}

/// `GameTooltip:SetTrainerService`'s subject at a 1-based record index (`0x62e900`).
pub(super) fn service_tooltip(lua: &Lua, index: i64) -> Option<TrainerTooltip> {
    service_of(lua, index).map(|(_, s)| s.tooltip)
}

/// A service's player-side answers at the call: the lent view's, else a test's resolved fields.
fn service_live(si: usize, s: &TrainerService) -> TrainerServiceLive {
    super::live::read_live(|l| l.trainer_service_live(si))
        .flatten()
        .flatten()
        .unwrap_or_else(|| TrainerServiceLive {
            skill_req: s.skill_req.clone(),
            ability_reqs: s.ability_reqs.clone(),
            item_link: s.item_link.clone(),
        })
}

/// `GetTrainerSelectionIndex` (`0x594430`): the selected spell's 1-based record index, which may
/// sit in the hidden tail; `None` when nothing is selected or it left the list.
fn selected_row(w: &TrainerWindow) -> Option<usize> {
    let want = w.selection?;
    let t = &w.state;
    rows(w)
        .0
        .iter()
        .position(|r| match r {
            Row::Service(si) => t.services.get(*si).is_some_and(|s| s.spell_id == want),
            Row::Header(_) => false,
        })?
        .checked_add(1)
}

/// A mask commit's tail (`0x594fe0`, `0x595010`, the collapse verbs): `edit` on the window, the
/// filter-and-sort over the type filter now, then `TRAINER_UPDATE` inside the call.
fn commit(
    lua: &Lua,
    edit: impl FnOnce(&mut TrainerWindow) -> mlua::Result<()>,
) -> mlua::Result<()> {
    let filter = type_filter(&read(lua));
    with_window_mut(lua, |w| match w {
        Some(w) => {
            edit(w)?;
            w.filter_and_sort(filter);
            Ok(())
        }
        None => edit(&mut TrainerWindow::default()),
    })?;
    super::tick::fire_event_into(lua, "TRAINER_UPDATE", Vec::new());
    Ok(())
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
fn header_key(w: &TrainerWindow, index: i64) -> Option<u32> {
    let n = usize::try_from(index.checked_sub(1)?).ok()?;
    match rows(w).0.get(n)? {
        Row::Header(gi) => w.state.groups.get(*gi).map(|g| g.key),
        Row::Service(_) => None,
    }
}

/// A skill line's key by its 1-based line index, in skill-line order.
fn line_key(w: &TrainerWindow, index: i64) -> Option<u32> {
    let n = usize::try_from(index.checked_sub(1)?).ok()?;
    w.state.groups.get(n).map(|g| g.key)
}

/// Every line key but `keep`.
fn other_lines(w: &TrainerWindow, keep: Option<u32>) -> HashSet<u32> {
    w.state
        .groups
        .iter()
        .map(|g| g.key)
        .filter(|&k| Some(k) != keep)
        .collect()
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

/// The setter's zero leg (`0x5940e0`) on an open trainer: `TRAINER_CLOSED` (`0x594108`), the
/// interact target cleared, whose interaction close runs this leg again when the trainer held it
/// (`0x594113` -> `0x512f80`), then the zero (`0x59411b`).
fn set_none(lua: &Lua, open: bool) {
    if !open {
        return;
    }
    super::tick::fire_event_into(lua, "TRAINER_CLOSED", Vec::new());
    if super::live::write_live(|l| l.trainer_release_target()) == Some(true) {
        let open = super::live::read_live(|l| l.trainer_open())
            .flatten()
            .unwrap_or(false);
        set_none(lua, open);
    }
    super::live::write_live(|l| l.trainer_zero());
}

/// A getter over a service and its place in [`TrainerState::services`].
type Getter = fn(&Lua, Option<(usize, &TrainerService)>) -> mlua::Result<Value>;

/// Register a getter over the service at its 1-based index argument, read at the call.
fn getter(lua: &Lua, name: &str, usage: &'static str, f: Getter) -> mlua::Result<()> {
    lua.globals().set(
        name,
        lua.create_function(move |lua, index: Value| {
            let index = number_arg(&index, usage)?;
            let s = service_of(lua, index);
            f(lua, s.as_ref().map(|(si, s)| (*si, s)))
        })?,
    )
}

/// Register the trainer globals: `s_ScriptFunctions` (`0xacf3e8`) less `OpenTrainer`.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetNumTrainerServices",
        lua.create_function(
            |lua, ()| Ok(with_window(lua, |w| w.map_or(0, |w| w.filtered)) as i64),
        )?,
    )?;

    // GetTrainerServiceInfo(index) → name, subText, serviceType, isExpanded (`0x595090`): a header
    // reads "" for its subtext, and isExpanded is 1 for every record but a collapsed header.
    g.set(
        "GetTrainerServiceInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(&index, "Usage: GetTrainerServiceInfo(index)")?;
            type Info = (Option<String>, Option<String>, Option<&'static str>, bool);
            let (name, sub, kind, expanded): Info = with_window(lua, |w| {
                let Some(w) = w else {
                    return (None, None, None, true);
                };
                let row = usize::try_from(index - 1)
                    .ok()
                    .and_then(|n| rows(w).0.get(n).copied());
                match row {
                    Some(Row::Header(gi)) => {
                        let g = &w.state.groups[gi];
                        let expanded = !w.collapsed.contains(&g.key);
                        (
                            Some(g.name.clone()),
                            Some(String::new()),
                            Some("header"),
                            expanded,
                        )
                    }
                    Some(Row::Service(si)) => {
                        let s = &w.state.services[si];
                        let sub = s
                            .name
                            .as_ref()
                            .map(|_| s.subtext.clone().unwrap_or_default());
                        (s.name.clone(), sub, Some(s.category.type_str()), true)
                    }
                    None => (None, None, None, true),
                }
            });
            Ok(MultiValue::from_vec(vec![
                opt_str(lua, name.as_deref())?,
                opt_str(lua, sub.as_deref())?,
                opt_str(lua, kind)?,
                binding_abi::flag(expanded),
            ]))
        })?,
    )?;

    getter(
        lua,
        "GetTrainerServiceSkillLine",
        "Usage: GetTrainerServiceSkillLine(index)",
        |lua, s| opt_str(lua, s.and_then(|(_, s)| s.skill_line.as_deref())),
    )?;
    getter(
        lua,
        "GetTrainerServiceIcon",
        "Usage: GetTrainerServiceIcon(index)",
        |lua, s| opt_str(lua, s.and_then(|(_, s)| s.texture.as_deref())),
    )?;
    // `0x5959d0`: the description parsed at the call, a recipe's product asked with the trainer's
    // record callback.
    getter(
        lua,
        "GetTrainerServiceDescription",
        "Usage: GetTrainerServiceDescription(index)",
        |lua, s| {
            let global = |key: &str| crate::strings::global(lua, key);
            let text = s.and_then(|(si, s)| {
                super::live::write_live(|l| l.trainer_description(si, &global))
                    .flatten()
                    .unwrap_or_else(|| s.description.clone())
            });
            opt_str(lua, text.as_deref())
        },
    )?;
    getter(
        lua,
        "GetTrainerServiceLevelReq",
        "Usage: GetTrainerServiceLevelReq(index)",
        |_, s| Ok(Value::Integer(s.map_or(0, |(_, s)| i64::from(s.level_req)))),
    )?;
    getter(
        lua,
        "GetTrainerServiceNumAbilityReq",
        "Usage: GetTrainerServiceAbilityReq(index)",
        |_, s| {
            let n = s.map_or(0, |(_, s)| s.ability_reqs.iter().flatten().count());
            Ok(Value::Integer(n as i64))
        },
    )?;
    getter(
        lua,
        "IsTrainerServiceSkillStep",
        "Usage: IsTrainerServiceSkillStep(index)",
        |_, s| Ok(binding_abi::flag(s.is_some_and(|(_, s)| s.skill_step))),
    )?;
    // Nil for a known step: `0x595c40` gates on state 2.
    getter(
        lua,
        "GetTrainerServiceStepIncrease",
        "Usage: GetTrainerServiceStepIncrease(index)",
        |lua, s| {
            let text = s
                .filter(|(_, s)| s.category != TrainerServiceCategory::Used)
                .and_then(|(_, s)| s.step_increase.as_deref());
            opt_str(lua, text)
        },
    )?;
    // `0x596290`: a recipe's product link off the item cache at the call.
    getter(
        lua,
        "GetTrainerServiceItemLink",
        "Usage: GetTrainerServiceItemLink(index)",
        |lua, s| {
            let link = s.and_then(|(si, s)| service_live(si, s).item_link);
            opt_str(lua, link.as_deref())
        },
    )?;

    // GetTrainerServiceCost(index) → money, talentCost, professionCost, the wire's words raw.
    g.set(
        "GetTrainerServiceCost",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(&index, "Usage: GetTrainerServiceCost(index)")?;
            let (money, talent, prof) = service_of(lua, index)
                .map(|(_, s)| (s.cost, s.talent_cost, s.profession_cost))
                .unwrap_or((0, 0, 0));
            Ok(MultiValue::from_vec(vec![
                Value::Integer(i64::from(money)),
                Value::Integer(i64::from(talent)),
                Value::Integer(i64::from(prof)),
            ]))
        })?,
    )?;

    // GetTrainerServiceSkillReq(index) → skill, rank, hasReq; (nil, 0, 1) with no gate; met read
    // off our skill slot at the call (`0x595470`).
    g.set(
        "GetTrainerServiceSkillReq",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(&index, "Usage: GetTrainerServiceSkillReq(index)")?;
            let req = service_of(lua, index).and_then(|(si, s)| service_live(si, &s).skill_req);
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
    // slot reads (nil, 1); met read off our book at the call (`0x5955e0`).
    g.set(
        "GetTrainerServiceAbilityReq",
        lua.create_function(|lua, (index, slot): (Value, Value)| {
            const USAGE: &str = "Usage: GetTrainerServiceAbilityReq(index)";
            let index = number_arg(&index, USAGE)?;
            let slot = number_arg(&slot, USAGE)?;
            let req = service_of(lua, index).and_then(|(si, s)| {
                let n = usize::try_from(slot - 1).ok()?;
                service_live(si, &s).ability_reqs.get(n).cloned().flatten()
            });
            Ok(MultiValue::from_vec(vec![
                opt_str(lua, req.as_ref().and_then(|r| r.name.as_deref()))?,
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
            Ok(binding_abi::flag(with_window(lua, |w| {
                w.is_some_and(|w| w.state.trainer_type == TRAINER_TYPE_TRADESKILL)
            })))
        })?,
    )?;

    g.set(
        "GetTrainerGreetingText",
        lua.create_function(|lua, ()| {
            let text = with_window(lua, |w| w.map(|w| w.state.greeting.clone()));
            lua.create_string(text.unwrap_or_default())
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
            {
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
            }
            commit(lua, |_| Ok(()))
        })?,
    )?;

    // GetTrainerSkillLines() → each line's `SkillLine` name (`0x594650`); a tradeskill trainer's
    // keys 1-3 name no skill line, so they answer nil.
    g.set(
        "GetTrainerSkillLines",
        lua.create_function(|lua, ()| {
            let names: Vec<Option<String>> = with_window(lua, |w| {
                let Some(w) = w else {
                    return Vec::new();
                };
                let tradeskill = w.state.trainer_type == TRAINER_TYPE_TRADESKILL;
                w.state
                    .groups
                    .iter()
                    .map(|g| (!tradeskill).then(|| g.name.clone()))
                    .collect()
            });
            names
                .iter()
                .map(|n| opt_str(lua, n.as_deref()))
                .collect::<mlua::Result<Vec<_>>>()
                .map(MultiValue::from_vec)
        })?,
    )?;

    // GetTrainerSkillLineFilter(index) (`0x593eb0`): 0 or below asks whether every line passes.
    g.set(
        "GetTrainerSkillLineFilter",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(&index, "Usage: GetTrainerSkillLineFilter(index)")?;
            let empty = TrainerWindow::default();
            with_window(lua, |w| {
                let w = w.unwrap_or(&empty);
                if index < 1 {
                    return Ok(binding_abi::flag(w.lines_off.is_empty()));
                }
                let key = line_key(w, index).ok_or_else(|| {
                    mlua::Error::runtime("Bad skill line in GetTrainerSkillLineFilter")
                })?;
                Ok(binding_abi::flag(!w.lines_off.contains(&key)))
            })
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
            commit(lua, |w| {
                if index < 1 {
                    w.lines_off.clear();
                    return Ok(());
                }
                let key = line_key(w, index).ok_or_else(|| {
                    mlua::Error::runtime("Bad skill line in SetTrainerSkillLineFilter")
                })?;
                let on = number_arg(
                    &on,
                    "Missing on//off parameter in SetTrainerSkillLineFilter",
                )?;
                if on == 0 {
                    w.lines_off.insert(key);
                } else if exclusive_arg(&exclusive) {
                    w.lines_off = other_lines(w, Some(key));
                } else {
                    w.lines_off.remove(&key);
                }
                Ok(())
            })
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
                commit(lua, |w| {
                    if index < 1 {
                        w.collapsed = if collapse {
                            other_lines(w, None)
                        } else {
                            HashSet::new()
                        };
                        return Ok(());
                    }
                    let key = header_key(w, index).ok_or_else(|| mlua::Error::runtime(bad))?;
                    if collapse {
                        w.collapsed.insert(key);
                    } else {
                        w.collapsed.remove(&key);
                    }
                    Ok(())
                })
            })?,
        )?;
    }

    // SelectTrainerService(index) (`0x5943a0`): a header or past the end selects nothing.
    g.set(
        "SelectTrainerService",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(&index, "Usage: SelectTrainerService(index)")?;
            with_window_mut(lua, |w| {
                if let Some(w) = w {
                    w.selection = service(w, index).map(|(_, s)| s.spell_id);
                }
            });
            Ok(())
        })?,
    )?;

    g.set(
        "GetTrainerSelectionIndex",
        lua.create_function(|lua, ()| {
            Ok(with_window(lua, |w| w.and_then(selected_row)).map(|r| r as i64))
        })?,
    )?;

    // BuyTrainerService(index): `CMSG_TRAINER_BUY_SPELL` inside the call for a record in state 0
    // (`0x594da0`). Deviation: 0 or below is a no-op, because one stray call would spend the
    // purse; the reference buys every visible service (`0x594e50`).
    g.set(
        "BuyTrainerService",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(&index, "Usage: BuyTrainerService(index)")?;
            let buyable = service_of(lua, index)
                .filter(|(_, s)| s.category == TrainerServiceCategory::Available)
                .map(|(_, s)| s.spell_id);
            if let Some(spell_id) = buyable {
                super::live::write_live(|l| l.trainer_buy(spell_id));
            }
            Ok(())
        })?,
    )?;

    // CloseTrainer() (`0x594370`): the setter's zero leg inside the call.
    g.set(
        "CloseTrainer",
        lua.create_function(|lua, ()| {
            if let Some(open) = super::live::read_live(|l| l.trainer_open()).flatten() {
                set_none(lua, open);
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests;
