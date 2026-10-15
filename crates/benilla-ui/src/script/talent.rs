//! The talent bindings over 3.3.5a's talent groups. The app pushes the class's pages and talents
//! (`Talent.dbc`, `TalentTab.dbc`, `Spell.dbc`) with each group's learned ranks from
//! `SMSG_TALENTS_INFO` ([`UiScript::set_talents`]); the VM keeps the preview ranks, which
//! `AddPreviewTalentPoints` moves and every push resets, as the packet re-seeds them in 12340
//! (`0x5c9310` sets the preview rank `+0x20` to the learned `+0x1c`).
//!
//! Ranks are 0-based with -1 for none, as 12340 stores them (`TalentRec+0x1c`/`+0x20`); the Lua
//! sees rank + 1. Tiers and columns are 1-based, as the stock frame indexes
//! `TALENT_BRANCH_ARRAY[tier][column]`. A group is the caller's `groupIndex`, else the active one
//! (`0x5c57d0`). Inspect and pet talents are not modelled: their counts answer 0 and their tabs
//! nothing, which hides the pet spec tab (`PlayerSpecTab_Update`).
//!
//! `GameTooltip:SetTalent` (`0x62f740` → `0x626e20`) shows the group's rank, or its preview rank
//! with `isPreview`: the rank line (`TOOLTIP_TALENT_RANK`), the current rank's spell body, the next
//! rank's under `TOOLTIP_TALENT_NEXT_RANK`, the red requirement lines and "Click to learn".

use mlua::{Lua, MultiValue, Table, Value};

use super::tooltip_spell::{spell_view_of, TalentHint, TalentLines};
use super::{Model, ScriptValue};

/// Points per tier a tab needs before the next unlocks (`PLAYER_TALENTS_PER_TIER`, `0x5c7461`).
const POINTS_PER_TIER: i32 = 5;

/// The dual-spec activation spells `SetActiveTalentGroup` casts, by group (`0xad05e8`).
const ACTIVATION_SPELLS: [u32; 2] = [63645, 63644];

/// One talent page, a `TalentTab.dbc` row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TalentTabView {
    pub name: String,
    /// `SpellIconID` through `SpellIcon.dbc`, the tuple's `iconTexture`.
    pub icon: Option<String>,
    /// The `Interface\TalentFrame\<base>-` art base, the tuple's `fileName`.
    pub background: String,
}

/// One rank of a talent: the spell it teaches and that spell's name and icon.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TalentRankView {
    pub spell: u32,
    pub name: String,
    pub icon: Option<String>,
}

/// One prerequisite edge: a talent in the same tab and the 0-based rank it needs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TalentPrereqView {
    /// The prerequisite's 0-based index in the tab.
    pub index: usize,
    pub rank: u32,
}

/// One talent button's static data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TalentView {
    /// The `Talent.dbc` row, sent by the learn verbs and named in the link.
    pub id: u32,
    pub tier: u32,
    pub column: u32,
    /// The rank spells, in rank order; the talent's max rank is their count.
    pub ranks: Vec<TalentRankView>,
    /// `Talent.dbc` flags bit 0 (`TalentRec+0x4c`), the tuple's `isExceptional`.
    pub exceptional: bool,
    pub prereqs: Vec<TalentPrereqView>,
    /// The `requiredSpell` is known (or there is none): `meetsPrereq` (`0x5c7a39`).
    pub required_spell_known: bool,
    /// The required spell's name, for the `ITEM_REQ_SKILL` line while it is unknown.
    pub required_spell_name: Option<String>,
}

impl TalentView {
    fn max_rank(&self) -> i32 {
        self.ranks.len() as i32
    }

    /// The rank spell shown for a rank, rank 1's when unlearned (`0x5c79c6`).
    fn rank_view(&self, rank: i32) -> Option<&TalentRankView> {
        self.ranks.get(rank.max(0) as usize)
    }
}

/// One talent group (spec): its learned ranks and unspent points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TalentGroupView {
    /// `ranks[tab][index]`, 0-based, -1 for unlearned; shaped as [`TalentUiState::talents`].
    pub ranks: Vec<Vec<i32>>,
    /// Unspent points: the packet's for the active group, the total less this group's spent
    /// for the others (`0x5c9ca0`).
    pub unspent: u32,
}

/// The pushed snapshot: the player's pages, talents and groups.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TalentUiState {
    pub tabs: Vec<TalentTabView>,
    /// `talents[t]` holds tab `t+1`'s talents in `Talent.dbc` row order.
    pub talents: Vec<Vec<TalentView>>,
    pub groups: Vec<TalentGroupView>,
    /// 0-based.
    pub active_group: usize,
    /// `UnitCharacterPoints("player")`: unspent talent points, free primary professions.
    pub points: (u32, u32),
}

impl super::UiScript {
    /// Push the whole talent snapshot; the preview ranks restart from the learned ones.
    pub fn set_talents(&mut self, state: TalentUiState) {
        let mut model = self.model_mut();
        model.talent_preview = state.groups.iter().map(|g| g.ranks.clone()).collect();
        model.talents = state;
    }

    /// Drain the `LearnTalent` sends as `(talentId, rank)`, the 0-based rank to learn
    /// (`CMSG_LEARN_TALENT`, `0x5c8288`).
    pub fn take_talent_learns(&mut self) -> Vec<(u32, u32)> {
        drain_field!(self, talent_learns)
    }

    /// Drain the `LearnPreviewTalents` sends, each a `CMSG_LEARN_PREVIEW_TALENTS` body's
    /// `(talentId, previewRank)` list.
    pub fn take_talent_preview_learns(&mut self) -> Vec<Vec<(u32, u32)>> {
        drain_field!(self, talent_preview_learns)
    }

    /// Drain the count of `ConfirmTalentWipe()` calls, each an outbound `MSG_TALENT_WIPE_CONFIRM`;
    /// a count, since the app holds the trainer's guid.
    pub fn take_talent_wipe_confirms(&mut self) -> u32 {
        drain_field!(self, talent_wipe_confirms)
    }

    /// Push whether a trainer's respec question is live and in range, which
    /// `CheckTalentMasterDist()` answers; false for none pending or walked away.
    pub fn set_talent_master_pending(&mut self, pending: bool) {
        if self.model_ref().talent_master_pending == pending {
            return;
        }
        self.model_mut().talent_master_pending = pending;
    }
}

/// One talent by its 1-based (tab, index).
fn talent_at(model: &Model, tab: i64, index: i64) -> Option<&TalentView> {
    let tab = usize::try_from(tab.checked_sub(1)?).ok()?;
    let index = usize::try_from(index.checked_sub(1)?).ok()?;
    model.talents.talents.get(tab)?.get(index)
}

/// A `groupIndex` argument (1-based) or the active group, 0-based (`0x5c57d0`).
fn group_arg(model: &Model, v: &Value) -> usize {
    match number_of(v) {
        Some(n) => (n as i64 - 1).max(-1) as usize,
        None => model.talents.active_group,
    }
}

fn number_of(v: &Value) -> Option<f64> {
    match v {
        Value::Integer(i) => Some(*i as f64),
        Value::Number(n) => Some(*n),
        Value::String(s) => s.to_str().ok()?.trim().parse().ok(),
        _ => None,
    }
}

/// A boolean argument as `0x815500` reads it: nil and false are false, anything else true.
fn flag_arg(v: Option<&Value>) -> bool {
    !matches!(v, None | Some(Value::Nil) | Some(Value::Boolean(false)))
}

fn arg(args: &MultiValue, i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Nil)
}

fn int_arg(args: &MultiValue, i: usize) -> Option<i64> {
    number_of(&arg(args, i)).map(|n| n as i64)
}

fn one_or_nil(b: bool) -> Value {
    if b {
        Value::Integer(1)
    } else {
        Value::Nil
    }
}

/// A group's learned rank of a talent, -1 when the group or talent is absent.
fn learned(model: &Model, group: usize, tab: usize, index: usize) -> i32 {
    model
        .talents
        .groups
        .get(group)
        .and_then(|g| g.ranks.get(tab)?.get(index).copied())
        .unwrap_or(-1)
}

/// A group's preview rank of a talent, -1 when absent.
fn preview(model: &Model, group: usize, tab: usize, index: usize) -> i32 {
    model
        .talent_preview
        .get(group)
        .and_then(|g| g.get(tab)?.get(index).copied())
        .unwrap_or(-1)
}

/// Points a group has in a tab: learned (`TabInfo+4`) and previewed on top (`TabInfo+8`).
fn tab_spent(model: &Model, group: usize, tab: usize) -> (i32, i32) {
    let Some(ranks) = model
        .talents
        .groups
        .get(group)
        .and_then(|g| g.ranks.get(tab))
    else {
        return (0, 0);
    };
    let spent = ranks.iter().map(|&r| r + 1).sum();
    let previewed = (0..ranks.len())
        .map(|i| preview(model, group, tab, i) - ranks[i])
        .sum();
    (spent, previewed)
}

/// The group's preview points across every tab (`0x5c6010`).
fn group_preview_spent(model: &Model, group: usize) -> i32 {
    (0..model.talents.tabs.len())
        .map(|t| tab_spent(model, group, t).1)
        .sum()
}

/// The group's unspent points (`GroupInfo+0x38`), 0 for a group not there.
fn group_unspent(model: &Model, group: usize) -> i32 {
    model
        .talents
        .groups
        .get(group)
        .map_or(0, |g| g.unspent as i32)
}

/// `AddPreviewTalentPoints`'s gate (`0x5c73d0`): the preview delta a move would apply, 0 when
/// refused. Raising needs a point left; any move needs the tier unlocked and the prerequisites
/// met in preview; the rank clamps to [learned, max]; lowering refuses when a dependent or a
/// higher tier's points would lose their footing.
fn preview_step(model: &Model, group: usize, tab: usize, index: usize, delta: i32) -> i32 {
    if delta == 0 || group >= model.talents.groups.len() {
        return 0;
    }
    let Some(t) = model
        .talents
        .talents
        .get(tab)
        .and_then(|ts| ts.get(index))
        .cloned()
    else {
        return 0;
    };
    let previewed_total = group_preview_spent(model, group);
    let unspent = group_unspent(model, group);
    if delta >= 1 && unspent - previewed_total <= 0 {
        return 0;
    }
    let (spent, previewed) = tab_spent(model, group, tab);
    if POINTS_PER_TIER * (t.tier as i32 - 1) > spent + previewed {
        return 0;
    }
    for p in &t.prereqs {
        if preview(model, group, tab, p.index) < p.rank as i32 {
            return 0;
        }
    }
    let base = learned(model, group, tab, index);
    let current = preview(model, group, tab, index);
    let target = (current + delta).clamp(base, (t.max_rank() - 1).max(base));
    let applied = target - current;
    if applied == 0 {
        return 0;
    }
    if applied < 0 {
        // A dependent with preview points must keep its prerequisite's rank (`0x5c7505`).
        for (i, other) in model.talents.talents[tab].iter().enumerate() {
            let needs = other
                .prereqs
                .iter()
                .any(|p| p.index == index && current + applied < p.rank as i32);
            if needs && preview(model, group, tab, i) >= 0 {
                return 0;
            }
        }
        // Every higher tier holding points keeps the tiers under it at five per tier
        // (`0x5c7631`).
        let talents = &model.talents.talents[tab];
        let points_in = |tier: u32| -> i32 {
            talents
                .iter()
                .enumerate()
                .filter(|(_, o)| o.tier == tier)
                .map(|(i, _)| preview(model, group, tab, i) + 1)
                .sum()
        };
        let top = talents.iter().map(|o| o.tier).max().unwrap_or(0);
        let mut below = 0;
        for tier in 1..=top {
            let here = points_in(tier);
            if tier > t.tier && here > 0 && below + applied < POINTS_PER_TIER * (tier as i32 - 1) {
                return 0;
            }
            below += here;
        }
    }
    applied
}

/// Apply [`preview_step`] (`0x5c9450`), returning the delta applied.
fn add_preview(model: &mut Model, group: usize, tab: usize, index: usize, delta: i32) -> i32 {
    let applied = preview_step(model, group, tab, index, delta);
    if applied != 0 {
        model.talent_preview[group][tab][index] += applied;
    }
    applied
}

/// `GetTalentInfo`'s ten values for one talent, or its absent-talent placeholder (`0x5c7c49`).
fn talent_info(lua: &Lua, model: &Model, args: &MultiValue) -> mlua::Result<MultiValue> {
    let (tab, index) = (int_arg(args, 0).unwrap_or(0), int_arg(args, 1).unwrap_or(0));
    let inspect = flag_arg(args.get(2));
    let pet = flag_arg(args.get(3));
    let t = (!inspect && !pet)
        .then(|| talent_at(model, tab, index))
        .flatten();
    let Some(t) = t else {
        return Ok(MultiValue::from_vec(vec![
            Value::Nil,
            Value::Nil,
            Value::Integer(1),
            Value::Integer(1),
            Value::Integer(0),
            Value::Integer(0),
            Value::Integer(0),
            Value::Nil,
            Value::Integer(0),
            Value::Nil,
        ]));
    };
    let (tab, index) = (tab as usize - 1, index as usize - 1);
    let group = group_arg(model, &arg(args, 4));
    let rank = learned(model, group, tab, index);
    let preview_rank = preview(model, group, tab, index);
    let shown = t.rank_view(rank);
    Ok(MultiValue::from_vec(vec![
        match shown {
            Some(r) => Value::String(lua.create_string(&r.name)?),
            None => Value::Nil,
        },
        match shown.and_then(|r| r.icon.as_deref()) {
            Some(icon) => Value::String(lua.create_string(icon)?),
            None => Value::Nil,
        },
        Value::Integer(i64::from(t.tier)),
        Value::Integer(i64::from(t.column)),
        Value::Integer(i64::from(rank + 1)),
        Value::Integer(i64::from(t.max_rank())),
        one_or_nil(t.exceptional),
        one_or_nil(t.required_spell_known),
        Value::Integer(i64::from(preview_rank + 1)),
        one_or_nil(t.required_spell_known),
    ]))
}

/// The tooltip's talent part for one group and rank view (`0x626e20`).
fn tooltip_lines(
    lua: &Lua,
    model: &Model,
    t: &TalentView,
    tab: usize,
    index: usize,
    group: usize,
    use_preview: bool,
) -> (u32, TalentLines) {
    let rank_of = |i: usize| {
        if use_preview {
            preview(model, group, tab, i)
        } else {
            learned(model, group, tab, i)
        }
    };
    let rank = rank_of(index);
    let max = t.max_rank();
    let display = t.rank_view(rank).map_or(0, |r| r.spell);
    let next_spell = if rank >= 0 && rank + 1 < max {
        t.ranks[(rank + 1) as usize].spell
    } else {
        0
    };
    let next_desc = (next_spell != 0)
        .then(|| {
            model
                .spell_tooltips
                .get(&next_spell)
                .map(|v| v.description.clone())
        })
        .flatten();
    let get = |key: &str| crate::strings::global(lua, key);
    let rank_line = get("TOOLTIP_TALENT_RANK").map(|tmpl| {
        crate::strings::fill(
            &tmpl,
            &[
                crate::strings::Arg::D((rank + 1).into()),
                crate::strings::Arg::D(max.into()),
            ],
        )
    });
    let mut reqs = Vec::new();
    let (spent, previewed) = tab_spent(model, group, tab);
    let tab_points = if use_preview {
        spent + previewed
    } else {
        spent
    };
    let need = POINTS_PER_TIER * (t.tier as i32 - 1);
    if need > tab_points {
        let tab_name = model
            .talents
            .tabs
            .get(tab)
            .map(|v| v.name.as_str())
            .unwrap_or("");
        reqs.extend(get("TOOLTIP_TALENT_TIER_POINTS").map(|f| {
            crate::strings::fill(
                &f,
                &[
                    crate::strings::Arg::D(need.into()),
                    crate::strings::Arg::S(tab_name),
                ],
            )
        }));
    }
    for p in &t.prereqs {
        if rank_of(p.index) >= p.rank as i32 {
            continue;
        }
        let Some(other) = model.talents.talents[tab].get(p.index) else {
            continue;
        };
        let name = other.rank_view(0).map_or("", |r| r.name.as_str());
        let need = p.rank + 1;
        reqs.extend(
            crate::strings::plural("TOOLTIP_TALENT_PREREQ", Some(need), &get).map(|f| {
                crate::strings::fill(
                    &f,
                    &[
                        crate::strings::Arg::D(need.into()),
                        crate::strings::Arg::S(name),
                    ],
                )
            }),
        );
    }
    if !t.required_spell_known {
        if let Some(name) = &t.required_spell_name {
            reqs.extend(
                get("ITEM_REQ_SKILL")
                    .map(|f| crate::strings::fill(&f, &[crate::strings::Arg::S(name)])),
            );
        }
    }
    // The closing hints (`0x622800`): the talent can be learned when it holds a rank or every
    // requirement line is met (`0x6224f0`); then, without preview, the active group with points
    // unspent and a rank to go shows the learn line, and with preview each move that would pass.
    let can_learn = rank >= 0 || reqs.is_empty();
    let mut hints = Vec::new();
    if can_learn && !use_preview {
        if group == model.talents.active_group && group_unspent(model, group) != 0 && rank + 1 < max
        {
            hints.push(TalentHint::Learn);
        }
    } else if can_learn {
        if preview_step(model, group, tab, index, 1) != 0 {
            hints.push(TalentHint::AddPreview);
        }
        if preview_step(model, group, tab, index, -1) != 0 {
            hints.push(TalentHint::RemovePreview);
        }
    }
    (
        display,
        TalentLines {
            exceptional: t.exceptional,
            rank_line,
            reqs,
            next_spell,
            next_desc,
            hints,
        },
    )
}

/// Register the talent globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumTalentTabs([isInspect[, isPet]]) (`0x5c5cc0`).
    g.set(
        "GetNumTalentTabs",
        lua.create_function(|lua, args: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            if flag_arg(args.front()) || flag_arg(args.get(1)) {
                return Ok(0);
            }
            Ok(model.talents.tabs.len() as i64)
        })?,
    )?;

    // GetTalentTabInfo(tab[, isInspect[, isPet[, groupIndex]]]) (`0x5c6150`): name, iconTexture,
    // pointsSpent, background, previewPointsSpent; an absent tab answers nil, nil, 0, nil, 0.
    g.set(
        "GetTalentTabInfo",
        lua.create_function(|lua, args: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let Some(tab) = int_arg(&args, 0) else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: GetTalentTabInfo(tabIndex[, isInspect[, isPet[, groupIndex]]])".into(),
                ));
            };
            let inspect = flag_arg(args.get(1));
            let pet = flag_arg(args.get(2));
            let view = (!inspect && !pet)
                .then(|| {
                    usize::try_from(tab - 1)
                        .ok()
                        .and_then(|t| model.talents.tabs.get(t).map(|v| (t, v)))
                })
                .flatten();
            let Some((t, view)) = view else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Integer(0),
                    Value::Nil,
                    Value::Integer(0),
                ]));
            };
            let group = group_arg(&model, &arg(&args, 3));
            let (spent, previewed) = tab_spent(&model, group, t);
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&view.name)?),
                match &view.icon {
                    Some(icon) => Value::String(lua.create_string(icon)?),
                    None => Value::Nil,
                },
                Value::Integer(spent.into()),
                Value::String(lua.create_string(&view.background)?),
                Value::Integer(previewed.into()),
            ]))
        })?,
    )?;

    // GetNumTalents(tab[, isInspect[, isPet]]) (`0x5c5d40`).
    g.set(
        "GetNumTalents",
        lua.create_function(|lua, args: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let Some(tab) = int_arg(&args, 0) else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: GetNumTalents(tabIndex[, isInspect[, isPet]])".into(),
                ));
            };
            if flag_arg(args.get(1)) || flag_arg(args.get(2)) {
                return Ok(0);
            }
            let n = usize::try_from(tab - 1)
                .ok()
                .and_then(|t| model.talents.talents.get(t))
                .map_or(0, Vec::len);
            Ok(n as i64)
        })?,
    )?;

    // GetTalentInfo(tab, index[, isInspect[, isPet[, groupIndex]]]) (`0x5c7800`): name,
    // iconTexture, tier, column, rank, maxRank, isExceptional, meetsPrereq, previewRank,
    // meetsPreviewPrereq.
    g.set(
        "GetTalentInfo",
        lua.create_function(|lua, args: MultiValue| {
            if int_arg(&args, 0).is_none() || int_arg(&args, 1).is_none() {
                return Err(mlua::Error::RuntimeError(
                    "Usage: GetTalentInfo(tabIndex, talentIndex[, isInspect[, isPet[, groupIndex]]])"
                        .into(),
                ));
            }
            let model = lua.app_data_ref::<crate::script::ModelCell>().expect("model app_data");
            talent_info(lua, &model, &args)
        })?,
    )?;

    // GetTalentPrereqs(tab, index[, isInspect[, isPet[, groupIndex]]]) (`0x5c7ed0`): flat
    // (tier, column, isLearnable, isPreviewLearnable) quadruplets, as
    // `TalentFrame_SetPrereqs` walks them.
    g.set(
        "GetTalentPrereqs",
        lua.create_function(|lua, args: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let (tab, index) = (
                int_arg(&args, 0).unwrap_or(0),
                int_arg(&args, 1).unwrap_or(0),
            );
            let mut out = Vec::new();
            if flag_arg(args.get(2)) || flag_arg(args.get(3)) {
                return Ok(MultiValue::from_vec(out));
            }
            let Some(t) = talent_at(&model, tab, index) else {
                return Ok(MultiValue::from_vec(out));
            };
            let tab = tab as usize - 1;
            let group = group_arg(&model, &arg(&args, 4));
            let in_group = group < model.talents.groups.len();
            for p in &t.prereqs {
                let Some(other) = model.talents.talents[tab].get(p.index) else {
                    continue;
                };
                out.push(Value::Integer(i64::from(other.tier)));
                out.push(Value::Integer(i64::from(other.column)));
                if in_group {
                    let need = p.rank as i32;
                    out.push(one_or_nil(learned(&model, group, tab, p.index) >= need));
                    out.push(one_or_nil(preview(&model, group, tab, p.index) >= need));
                } else {
                    out.push(Value::Nil);
                    out.push(Value::Nil);
                }
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetTalentLink(tab, index[, isInspect[, isPet[, groupIndex[, isPreview]]]]) (`0x5c7ce0`):
    // `|cff4e96f7|Htalent:<id>:<rank>|h[<name>]|h|r` (`0x61a550`), the rank 0-based or -1.
    g.set(
        "GetTalentLink",
        lua.create_function(|lua, args: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let (tab, index) = (
                int_arg(&args, 0).unwrap_or(0),
                int_arg(&args, 1).unwrap_or(0),
            );
            if flag_arg(args.get(2)) || flag_arg(args.get(3)) {
                return Ok(Value::Nil);
            }
            let Some(t) = talent_at(&model, tab, index) else {
                return Ok(Value::Nil);
            };
            let (tab, index) = (tab as usize - 1, index as usize - 1);
            let group = group_arg(&model, &arg(&args, 4));
            let rank = if flag_arg(args.get(5)) {
                preview(&model, group, tab, index)
            } else {
                learned(&model, group, tab, index)
            };
            let Some(view) = t.rank_view(rank) else {
                return Ok(Value::Nil);
            };
            let link = format!("|cff4e96f7|Htalent:{}:{}|h[{}]|h|r", t.id, rank, view.name);
            Ok(Value::String(lua.create_string(&link)?))
        })?,
    )?;

    // GetActiveTalentGroup([isInspect[, isPet]]) (`0x5c5810`): the pet's is always its one.
    g.set(
        "GetActiveTalentGroup",
        lua.create_function(|lua, args: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            if flag_arg(args.front()) || flag_arg(args.get(1)) {
                return Ok(1);
            }
            Ok(model.talents.active_group as i64 + 1)
        })?,
    )?;

    // GetNumTalentGroups([isInspect[, isPet]]) (`0x5c5df0`).
    g.set(
        "GetNumTalentGroups",
        lua.create_function(|lua, args: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            if flag_arg(args.front()) || flag_arg(args.get(1)) {
                return Ok(0);
            }
            Ok(model.talents.groups.len() as i64)
        })?,
    )?;

    // SetActiveTalentGroup(group) (`0x5c5e70`): another existing group casts its activation
    // spell on the player; the server's `SMSG_TALENTS_INFO` then names the new active group.
    g.set(
        "SetActiveTalentGroup",
        lua.create_function(|lua, group: Value| {
            let Some(group) = number_of(&group) else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: SetActiveTalentGroup(groupIndex)".into(),
                ));
            };
            let mut model = crate::script::model_mut_of(lua);
            let g = group as i64 - 1;
            if g >= 0
                && (g as usize) != model.talents.active_group
                && (g as usize) < model.talents.groups.len()
                && (g as usize) < ACTIVATION_SPELLS.len()
            {
                model.script_calls.push(super::ScriptCall::CastSpell {
                    spell_id: ACTIVATION_SPELLS[g as usize],
                    at: super::calls::CastAt::Player,
                    via_book: false,
                });
            }
            Ok(())
        })?,
    )?;

    // GetUnspentTalentPoints([isInspect[, isPet[, groupIndex]]]) (`0x5c6310`).
    g.set(
        "GetUnspentTalentPoints",
        lua.create_function(|lua, args: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            if flag_arg(args.front()) || flag_arg(args.get(1)) {
                return Ok(0);
            }
            let group = group_arg(&model, &arg(&args, 2));
            Ok(i64::from(group_unspent(&model, group)))
        })?,
    )?;

    // GetGroupPreviewTalentPointsSpent([isPet[, groupIndex]]) (`0x5c6420`).
    g.set(
        "GetGroupPreviewTalentPointsSpent",
        lua.create_function(|lua, args: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            if flag_arg(args.front()) {
                return Ok(0);
            }
            let group = group_arg(&model, &arg(&args, 1));
            Ok(i64::from(group_preview_spent(&model, group)))
        })?,
    )?;

    // GetPreviewTalentPointsSpent(tab[, isPet[, groupIndex]]) (`0x5c63b0`): 12340 reads its
    // arguments and answers 0, whatever the preview holds.
    g.set(
        "GetPreviewTalentPointsSpent",
        lua.create_function(|_, args: MultiValue| {
            if int_arg(&args, 0).is_none() {
                return Err(mlua::Error::RuntimeError(
                    "Usage: GetPreviewTalentPointsSpent(tabIndex[, isPet[, groupIndex]])".into(),
                ));
            }
            Ok(0)
        })?,
    )?;

    // AddPreviewTalentPoints(tab, index, points[, isPet[, groupIndex]]) (`0x5c9590`): moves the
    // preview rank and fires `PREVIEW_TALENT_POINTS_CHANGED` (talentIndex, tabIndex, group,
    // delta) when the group's preview total moved.
    g.set(
        "AddPreviewTalentPoints",
        lua.create_function(|lua, args: MultiValue| {
            let (Some(tab), Some(index), Some(points)) =
                (int_arg(&args, 0), int_arg(&args, 1), int_arg(&args, 2))
            else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: AddPreviewTalentPoints(tabIndex, talentIndex, points[, isPet[, groupIndex]])"
                        .into(),
                ));
            };
            if flag_arg(args.get(3)) {
                return Ok(());
            }
            let mut model = crate::script::model_mut_of(lua);
            let group = group_arg(&model, &arg(&args, 4));
            let (Ok(t), Ok(i)) = (usize::try_from(tab - 1), usize::try_from(index - 1)) else {
                return Ok(());
            };
            let delta = add_preview(&mut model, group, t, i, points as i32);
            if delta != 0 {
                model.pending_events.push((
                    "PREVIEW_TALENT_POINTS_CHANGED".into(),
                    vec![
                        ScriptValue::Int(index),
                        ScriptValue::Int(tab),
                        ScriptValue::Int(group as i64 + 1),
                        ScriptValue::Int(delta.into()),
                    ],
                ));
            }
            Ok(())
        })?,
    )?;

    // ResetPreviewTalentPoints(tab[, isPet[, groupIndex]]) (`0x5c7130`) and
    // ResetGroupPreviewTalentPoints([isPet[, groupIndex]]) (`0x5c7200`): the preview ranks back
    // to the learned ones, the event's index 0 for "every talent" (and tab 0 for every tab).
    g.set(
        "ResetPreviewTalentPoints",
        lua.create_function(|lua, args: MultiValue| {
            let Some(tab) = int_arg(&args, 0) else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: ResetPreviewTalentPoints(tabIndex[, isPet[, groupIndex]])".into(),
                ));
            };
            if flag_arg(args.get(1)) {
                return Ok(());
            }
            let mut model = crate::script::model_mut_of(lua);
            let group = group_arg(&model, &arg(&args, 2));
            reset_preview(&mut model, group, usize::try_from(tab - 1).ok(), tab);
            Ok(())
        })?,
    )?;
    g.set(
        "ResetGroupPreviewTalentPoints",
        lua.create_function(|lua, args: MultiValue| {
            if flag_arg(args.front()) {
                return Ok(());
            }
            let mut model = crate::script::model_mut_of(lua);
            let group = group_arg(&model, &arg(&args, 1));
            reset_preview(&mut model, group, None, 0);
            Ok(())
        })?,
    )?;

    // LearnPreviewTalents([isPet]) (`0x5c6a10`): the active group's previewed ranks in one send,
    // when any preview point is spent.
    g.set(
        "LearnPreviewTalents",
        lua.create_function(|lua, args: MultiValue| {
            if flag_arg(args.front()) {
                return Ok(());
            }
            let mut model = crate::script::model_mut_of(lua);
            let group = model.talents.active_group;
            if group_preview_spent(&model, group) <= 0 {
                return Ok(());
            }
            let mut sends = Vec::new();
            for (t, talents) in model.talents.talents.iter().enumerate() {
                for (i, talent) in talents.iter().enumerate() {
                    let p = preview(&model, group, t, i);
                    if learned(&model, group, t, i) < p {
                        sends.push((talent.id, p as u32));
                    }
                }
            }
            if !sends.is_empty() {
                model.talent_preview_learns.push(sends);
            }
            Ok(())
        })?,
    )?;

    // LearnTalent(tab, index[, isPet]) (`0x5c80e0`): the active group's next rank, below the
    // max; points and prerequisites are the server's to enforce.
    g.set(
        "LearnTalent",
        lua.create_function(|lua, args: MultiValue| {
            let (Some(tab), Some(index)) = (int_arg(&args, 0), int_arg(&args, 1)) else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: LearnTalent(tabIndex, talentIndex[, isPet])".into(),
                ));
            };
            if flag_arg(args.get(2)) {
                return Ok(());
            }
            let mut model = crate::script::model_mut_of(lua);
            let Some(t) = talent_at(&model, tab, index) else {
                return Ok(());
            };
            let (id, max) = (t.id, t.max_rank());
            let group = model.talents.active_group;
            let rank = learned(&model, group, tab as usize - 1, index as usize - 1);
            if rank < max - 1 {
                model.talent_learns.push((id, (rank + 1) as u32));
            }
            Ok(())
        })?,
    )?;

    // ConfirmTalentWipe(): the `CONFIRM_TALENT_WIPE` Accept, the one call that unlearns talents.
    // No argument: the reference sends the trainer guid it latched; the app holds it.
    g.set(
        "ConfirmTalentWipe",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.talent_wipe_confirms += 1;
            Ok(())
        })?,
    )?;

    // CheckTalentMasterDist(): polled by that dialog's OnUpdate, false hides it; no packet.
    g.set(
        "CheckTalentMasterDist",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.talent_master_pending)
        })?,
    )?;

    // Every unit token answers the player's own pair: `PLAYER_CHARACTER_POINTS1/2` are private
    // fields only we receive.
    g.set(
        "UnitCharacterPoints",
        lua.create_function(|lua, _unit: String| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let (cp1, cp2) = model.talents.points;
            Ok((i64::from(cp1), i64::from(cp2)))
        })?,
    )?;

    Ok(())
}

/// Reset a group's preview, one tab or all, and fire the event when its total moved.
fn reset_preview(model: &mut Model, group: usize, tab: Option<usize>, tab_arg: i64) {
    if group >= model.talents.groups.len() {
        return;
    }
    let before = group_preview_spent(model, group);
    let learned = model.talents.groups[group].ranks.clone();
    for (t, ranks) in learned.into_iter().enumerate() {
        if tab.is_none_or(|only| only == t) {
            model.talent_preview[group][t] = ranks;
        }
    }
    let after = group_preview_spent(model, group);
    if after != before {
        model.pending_events.push((
            "PREVIEW_TALENT_POINTS_CHANGED".into(),
            vec![
                ScriptValue::Int(0),
                ScriptValue::Int(tab_arg),
                ScriptValue::Int(group as i64 + 1),
                ScriptValue::Int((after - before).into()),
            ],
        ));
    }
}

/// Register `GameTooltip:SetTalent(tab, index[, isInspect[, isPet[, groupIndex[, isPreview]]]])`
/// into the tooltip method table.
pub(super) fn install_tooltip_method(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.set(
        "SetTalent",
        lua.create_function(|lua, (this, args): (Table, MultiValue)| {
            let (display, lines) = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let (tab, index) = (
                    int_arg(&args, 0).unwrap_or(0),
                    int_arg(&args, 1).unwrap_or(0),
                );
                if flag_arg(args.get(2)) || flag_arg(args.get(3)) {
                    return Ok(());
                }
                let Some(t) = talent_at(&model, tab, index) else {
                    return Ok(());
                };
                let group = group_arg(&model, &arg(&args, 4));
                tooltip_lines(
                    lua,
                    &model,
                    t,
                    tab as usize - 1,
                    index as usize - 1,
                    group,
                    flag_arg(args.get(5)),
                )
            };
            super::tooltip_spell::set_spell_with_talent(lua, &this, display, lines)
        })?,
    )?;
    Ok(())
}

/// Ask the spell store for the next rank's description; a miss queues it as a primary view's does.
pub(super) fn ask_next_rank(lua: &Lua, next_spell: u32) {
    if next_spell != 0 {
        let _ = spell_view_of(lua, next_spell, false); // a miss records the ask as a side effect
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn rank(spell: u32, name: &str) -> TalentRankView {
        TalentRankView {
            spell,
            name: name.into(),
            icon: Some("Interface\\Icons\\Spell_Fire_Fireball".into()),
        }
    }

    /// One tab: A (tier 1, three ranks), B (tier 1, one rank) and C (tier 2, needs A at rank 3).
    /// Two groups; group 1 active with two points in A and three unspent.
    fn state() -> TalentUiState {
        let a = TalentView {
            id: 10,
            tier: 1,
            column: 1,
            ranks: vec![rank(100, "A"), rank(101, "A"), rank(102, "A")],
            required_spell_known: true,
            ..Default::default()
        };
        let b = TalentView {
            id: 11,
            tier: 1,
            column: 2,
            ranks: vec![rank(110, "B")],
            required_spell_known: true,
            ..Default::default()
        };
        let c = TalentView {
            id: 12,
            tier: 2,
            column: 1,
            ranks: vec![rank(120, "C")],
            prereqs: vec![TalentPrereqView { index: 0, rank: 2 }],
            required_spell_known: true,
            ..Default::default()
        };
        TalentUiState {
            tabs: vec![TalentTabView {
                name: "Fire".into(),
                icon: Some("Interface\\Icons\\Spell_Fire_FireBolt02".into()),
                background: "MageFire".into(),
            }],
            talents: vec![vec![a, b, c]],
            groups: vec![
                TalentGroupView {
                    ranks: vec![vec![1, -1, -1]],
                    unspent: 3,
                },
                TalentGroupView {
                    ranks: vec![vec![-1, -1, -1]],
                    unspent: 5,
                },
            ],
            active_group: 0,
            points: (3, 0),
        }
    }

    fn script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_talents(state());
        s
    }

    #[test]
    fn the_tab_and_talent_tuples_are_12340s() {
        let s = script();
        let tab: (String, String, i64, String, i64) = s.eval("return GetTalentTabInfo(1)").unwrap();
        assert_eq!(
            tab,
            (
                "Fire".into(),
                "Interface\\Icons\\Spell_Fire_FireBolt02".into(),
                2,
                "MageFire".into(),
                0
            )
        );
        let info: (String, i64, i64, i64, i64, Option<i64>, i64, i64) = s
            .eval("local n,_,t,c,r,m,e,p,pr,pp = GetTalentInfo(1, 1) return n,t,c,r,m,e,p,pr")
            .unwrap();
        assert_eq!(info, ("A".into(), 1, 1, 2, 3, None, 1, 2));
        // The other group holds nothing; its tab sum and ranks read 0.
        assert_eq!(
            s.eval::<i64>("return select(5, GetTalentInfo(1, 1, false, false, 2))")
                .unwrap(),
            0
        );
        assert_eq!(
            s.eval::<i64>("return select(3, GetTalentTabInfo(1, false, false, 2))")
                .unwrap(),
            0
        );
        // Ten values for an absent talent too.
        assert_eq!(
            s.eval::<i64>("return select('#', GetTalentInfo(1, 9))")
                .unwrap(),
            10
        );
        assert_eq!(s.eval::<i64>("return GetNumTalentGroups()").unwrap(), 2);
        assert_eq!(s.eval::<i64>("return GetActiveTalentGroup()").unwrap(), 1);
        assert_eq!(
            s.eval::<i64>("return GetNumTalentGroups(false, true)")
                .unwrap(),
            0,
            "no pet groups: the pet spec tab stays hidden"
        );
        assert_eq!(s.eval::<i64>("return GetUnspentTalentPoints()").unwrap(), 3);
        assert_eq!(
            s.eval::<i64>("return GetUnspentTalentPoints(false, false, 2)")
                .unwrap(),
            5
        );
    }

    #[test]
    fn prereqs_are_quadruplets_against_the_groups_ranks() {
        let s = script();
        let q: (i64, i64, Option<i64>, Option<i64>) =
            s.eval("return GetTalentPrereqs(1, 3)").unwrap();
        assert_eq!(q, (1, 1, None, None), "A is at rank 2 of the 3 C needs");
        assert_eq!(
            s.eval::<i64>("return select('#', GetTalentPrereqs(1, 1))")
                .unwrap(),
            0
        );
    }

    #[test]
    fn learn_talent_sends_the_next_zero_based_rank() {
        let mut s = script();
        s.run("LearnTalent(1, 1); LearnTalent(1, 2)").unwrap();
        assert_eq!(s.take_talent_learns(), vec![(10, 2), (11, 0)]);
        // Maxed: nothing.
        let mut st = state();
        st.groups[0].ranks[0][1] = 0;
        s.set_talents(st);
        s.run("LearnTalent(1, 2)").unwrap();
        assert!(s.take_talent_learns().is_empty());
    }

    #[test]
    fn preview_points_move_within_the_rules_and_learn_in_one_send() {
        let mut s = script();
        // C is locked: tier 2 needs five points in the tab, the tab has two.
        s.run("AddPreviewTalentPoints(1, 3, 1)").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetGroupPreviewTalentPointsSpent()")
                .unwrap(),
            0
        );
        // A to max (one more), then B; preview rank reads on top.
        s.run("AddPreviewTalentPoints(1, 1, 5); AddPreviewTalentPoints(1, 2, 1)")
            .unwrap();
        let pr: (i64, i64) = s
            .eval("return select(9, GetTalentInfo(1, 1)), select(9, GetTalentInfo(1, 2))")
            .unwrap();
        assert_eq!(pr, (3, 1), "A clamps at its max rank");
        assert_eq!(
            s.eval::<i64>("return GetGroupPreviewTalentPointsSpent()")
                .unwrap(),
            2
        );
        assert_eq!(
            s.eval::<i64>("return select(5, GetTalentTabInfo(1))")
                .unwrap(),
            2
        );
        // Lowering never goes under the learned rank.
        s.run("AddPreviewTalentPoints(1, 1, -5)").unwrap();
        assert_eq!(
            s.eval::<i64>("return select(9, GetTalentInfo(1, 1))")
                .unwrap(),
            2
        );
        s.run("AddPreviewTalentPoints(1, 1, 1); LearnPreviewTalents()")
            .unwrap();
        assert_eq!(s.take_talent_preview_learns(), vec![vec![(10, 2), (11, 0)]]);
        s.run("ResetGroupPreviewTalentPoints()").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetGroupPreviewTalentPointsSpent()")
                .unwrap(),
            0
        );
        assert_eq!(
            s.eval::<i64>("return GetPreviewTalentPointsSpent(1)")
                .unwrap(),
            0,
            "12340's stub"
        );
    }

    #[test]
    fn a_link_names_the_talent_and_its_zero_based_rank() {
        let s = script();
        assert_eq!(
            s.eval::<String>("return GetTalentLink(1, 1)").unwrap(),
            "|cff4e96f7|Htalent:10:1|h[A]|h|r"
        );
        assert_eq!(
            s.eval::<String>("return GetTalentLink(1, 2)").unwrap(),
            "|cff4e96f7|Htalent:11:-1|h[B]|h|r"
        );
    }

    #[test]
    fn switching_groups_casts_the_activation_spell() {
        let mut s = script();
        s.run("SetActiveTalentGroup(1); SetActiveTalentGroup(2); SetActiveTalentGroup(3)")
            .unwrap();
        let casts = s.take_spell_casts();
        assert_eq!(
            casts.len(),
            1,
            "only another existing group casts: {casts:?}"
        );
        assert_eq!(casts[0].0, 63644);
    }
}
