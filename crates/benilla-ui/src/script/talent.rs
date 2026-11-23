//! The talent bindings over 3.3.5a's talent state ([`super::TalentInfo`]), read and written at the
//! call through the live view: the player's tabs and groups, the pet's and the inspected player's.
//! A verb answers from the state as it stands when called (`0x5c5880` registers them), and a
//! preview move or reset raises its event inside the call (`0x5c9590`, `0x5c7130`, `0x5c7200`).
//!
//! Ranks are 0-based with -1 for none, as 12340 stores them (`0x5c9310`); the Lua sees rank + 1.
//! Tiers and columns are 1-based, as the stock frame indexes `TALENT_BRANCH_ARRAY[tier][column]`.
//! A group is the caller's `groupIndex`, else the player's active one, for the pet and the
//! inspected player too (`0x5c57d0`). A verb's `isPet` picks the pet's set, whose tiers take 3
//! points (`0x5c73d0`, `0x6226ea`); `isInspect` the inspected player's, and with `isPet` too
//! nothing (`0x5c5c60`, `0x5c6080`).
//!
//! `GameTooltip:SetTalent` (`0x62f740` → `0x626e20`) shows the group's rank, or its preview rank
//! with `isPreview`: the rank line (`TOOLTIP_TALENT_RANK`), the current rank's spell body, the next
//! rank's under `TOOLTIP_TALENT_NEXT_RANK`, the red requirement lines and "Click to learn".

use mlua::{Lua, MultiValue, Table, Value};

use super::talent_info::{TalentGroup, TalentInfo, TalentSide};
use super::tooltip_spell::{TalentHint, TalentLines};
use super::{LiveState, ScriptValue};

/// The dual-spec activation spells `SetActiveTalentGroup` casts, by group (`0xad05e8`).
const ACTIVATION_SPELLS: [u32; 2] = [63645, 63644];

/// One talent page, a `TalentTab.dbc` row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TalentTabView {
    /// The row's id, the group's tab key (`0x5c9960`).
    pub id: u32,
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

/// One prerequisite slot of a `Talent.dbc` row (`0x5c7ed0`): the talent it names and the 0-based
/// rank it needs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TalentPrereqView {
    pub talent: u32,
    pub rank: u32,
    /// The named record's 1-based tier and column, `None` when the id names no record (the slot
    /// is skipped, `0x5c7feb`; a zero id is skipped at `0x5c7fd6`).
    pub seat: Option<(u32, u32)>,
    /// The named talent's spell at the needed rank, or the first later rank that has one; 0 for
    /// none, and the tooltip shows no line for the slot (`0x622585`, `0x6225a5`).
    pub spell: u32,
    /// That spell's name, `None` when it names no spell record (`0x6225c4`).
    pub name: Option<String>,
}

/// One talent button's record.
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
    /// `requiredSpellID` (`0x5c7b30`), 0 for none; `meetsPrereq` asks whether we know it (`0x5c7b67`).
    pub required_spell: u32,
    /// The required spell's name, for the `ITEM_REQ_SKILL` line while it is unknown.
    pub required_spell_name: Option<String>,
    /// The pet-family categories that may take it, both words 0 for every pet (`0x5c5a20`).
    pub category_mask: [u32; 2],
}

impl TalentView {
    fn max_rank(&self) -> i32 {
        self.ranks.len() as i32
    }

    /// The rank spell shown for a rank, rank 1's when unlearned (`0x5c79c6`).
    fn rank_view(&self, rank: i32) -> Option<&TalentRankView> {
        self.ranks.get(rank.max(0) as usize)
    }

    /// Whether a pet of family category `category` may take it (`0x5c5a20`): a mask of two 0
    /// words or a negative category admits it, a category past the words never.
    fn available_to(&self, category: i32) -> bool {
        let (word, bit) = if category >= 0 {
            ((category / 32) as usize, category % 32)
        } else {
            (0, 0)
        };
        if word > 2 {
            return false;
        }
        if self.category_mask == [0, 0] {
            return true;
        }
        category < 0
            || self
                .category_mask
                .get(word)
                .is_some_and(|w| w & (1 << bit) != 0)
    }
}

/// A verb's `(isInspect, isPet)` pair as a side.
fn side_of(inspect: bool, pet: bool) -> TalentSide {
    match (inspect, pet) {
        (true, _) => TalentSide::Inspect,
        (false, true) => TalentSide::Pet,
        (false, false) => TalentSide::Player,
    }
}

/// A verb's `(isInspect, isPet)` pair at `inspect_at`.
fn flags_at(args: &MultiValue, inspect_at: usize) -> (bool, bool) {
    (
        flag_arg(args.get(inspect_at)),
        flag_arg(args.get(inspect_at + 1)),
    )
}

/// The tab `index` (0-based, as the reference's unsigned wrap reads it) of a side (`0x5c5c60`):
/// an inspected pet has none.
fn tab_of(
    info: &TalentInfo,
    inspect: bool,
    pet: bool,
    index: i64,
) -> Option<(usize, &TalentTabView, &[TalentView])> {
    if inspect && pet {
        return None;
    }
    let index = usize::try_from(index).ok()?;
    let (view, list) = info.pages(side_of(inspect, pet)).tab(index)?;
    Some((index, view, list))
}

/// The group a verb's `groupIndex` names (`0x5c57d0`): its integer less one, else the player's
/// active group.
pub(super) fn script_group(info: &TalentInfo, v: Option<&Value>) -> u32 {
    match v.and_then(number_of) {
        Some(n) => (n as i64 - 1) as u32,
        None => info.active_group(),
    }
}

/// A verb's group (`0x5c6080`): the inspected player's or, else, the player's or the pet's.
fn group_of(info: &TalentInfo, inspect: bool, pet: bool, index: u32) -> Option<&TalentGroup> {
    if inspect && pet {
        return None;
    }
    info.group(side_of(inspect, pet), index)
}

fn number_of(v: &Value) -> Option<f64> {
    match v {
        Value::Integer(i) => Some(*i as f64),
        Value::Number(n) => Some(*n),
        Value::String(s) => s.to_str().ok()?.trim().parse().ok(),
        _ => None,
    }
}

/// A verb's `isInspect`, `isPet` or `isPreview` through the bool-or-default reader with default 0
/// (`0x815500`; `0x5c5cce`, `0x5c7886`): `0`, `"0"`, `"false"` and `"off"` are false.
fn flag_arg(v: Option<&Value>) -> bool {
    super::binding_abi::bool_or_default(v, false)
}

fn int_arg(args: &MultiValue, i: usize) -> Option<i64> {
    args.get(i).and_then(number_of).map(|n| n as i64)
}

fn one_or_nil(b: bool) -> Value {
    if b {
        Value::Integer(1)
    } else {
        Value::Nil
    }
}

/// `f` over the talent state and the view lent now; `None` where no view lends it.
fn with_info<R>(f: impl FnOnce(&TalentInfo, &dyn LiveState) -> R) -> Option<R> {
    super::read_live(|l| l.talent_info().map(|info| f(info, l))).flatten()
}

/// Whether the pet head may take `t` (`0x5c5a20`): no pet or no family row, never.
fn pet_may_take(live: &dyn LiveState, t: &TalentView) -> bool {
    live.pet_family_category()
        .is_some_and(|category| t.available_to(category))
}

/// `GetTalentInfo`'s absent-talent placeholder, ten values (`0x5c7c49`).
fn placeholder() -> MultiValue {
    MultiValue::from_vec(vec![
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
    ])
}

/// The values `GetTalentInfo` pushes for one talent, read before any string is made.
struct InfoRow {
    name: String,
    icon: Option<String>,
    tier: u32,
    column: u32,
    rank: i32,
    max: i32,
    exceptional: bool,
    meets: bool,
    preview: i32,
    meets_preview: bool,
}

/// `GetTalentInfo`'s read (`0x5c7800`): `None` for the placeholder — an inspected pet, a talent not
/// there, a pet talent the pet may not take (`0x5c7902`) or a rank naming no spell. `meetsPrereq`
/// is, for an inspected talent, a rank held (`0x5c7b93`); else a unit to read and,
/// for the player, the required spell known or a higher rank of it (`0x5c7b67`, `0x5c7b76`).
fn talent_info_row(args: &MultiValue) -> Option<InfoRow> {
    let (tab, index) = (int_arg(args, 0)? - 1, int_arg(args, 1)? - 1);
    let (inspect, pet) = flags_at(args, 2);
    with_info(|info, live| {
        let (_, _, list) = tab_of(info, inspect, pet, tab)?;
        let t = list.get(usize::try_from(index).ok()?)?;
        let unit = live.talent_unit(pet);
        if pet && !pet_may_take(live, t) {
            return None;
        }
        let item = group_of(info, inspect, pet, script_group(info, args.get(4)))
            .and_then(|g| g.item(t.id));
        let (rank, preview) = item.map_or((-1, -1), |i| (i.rank, i.preview));
        let shown = t.rank_view(rank)?;
        let known = |held: i32| {
            if inspect {
                !pet && held >= 0
            } else {
                unit.is_some()
                    && (pet
                        || t.required_spell == 0
                        || live.spell_known_or_higher(t.required_spell))
            }
        };
        Some(InfoRow {
            name: shown.name.clone(),
            icon: shown.icon.clone(),
            tier: t.tier,
            column: t.column,
            rank,
            max: t.max_rank(),
            exceptional: t.exceptional,
            meets: known(rank),
            preview,
            meets_preview: known(preview),
        })
    })
    .flatten()
}

/// The tooltip's talent part for one talent (`0x626e20`): the rank shown, the requirement lines
/// and the closing hints, read in one view.
fn tooltip_lines(lua: &Lua, args: &MultiValue) -> Option<(u32, TalentLines)> {
    let (tab, index) = (
        int_arg(args, 0).unwrap_or(0) - 1,
        int_arg(args, 1).unwrap_or(0) - 1,
    );
    let (inspect, pet) = flags_at(args, 2);
    let use_preview = flag_arg(args.get(5));
    let get = |key: &str| crate::strings::global(lua, key);
    with_info(|info, live| {
        let (t_index, view, list) = tab_of(info, inspect, pet, tab)?;
        let t = list.get(usize::try_from(index).ok()?)?;
        let group_index = script_group(info, args.get(4));
        let group = group_of(info, inspect, pet, group_index);
        let rank_of = |id: u32| {
            group
                .and_then(|g| g.item(id))
                .map_or(-1, |i| if use_preview { i.preview } else { i.rank })
        };
        let rank = rank_of(t.id);
        let max = t.max_rank();
        let display = t.rank_view(rank).map_or(0, |r| r.spell);
        let next_spell = if rank >= 0 && rank + 1 < max {
            t.ranks[(rank + 1) as usize].spell
        } else {
            0
        };
        let rank_line = get("TOOLTIP_TALENT_RANK").map(|tmpl| {
            crate::strings::fill(
                &tmpl,
                &[
                    crate::strings::Arg::D((rank + 1).into()),
                    crate::strings::Arg::D(max.into()),
                ],
            )
        });
        // The requirement lines (`0x6224f0`), only while no rank is held (`0x627085`, `0x623b64`);
        // `met` is its answer, cleared by a slot whose spell is found even with no line shown.
        let mut reqs = Vec::new();
        let mut met = true;
        let held = rank >= 0;
        // The prerequisite slots first (`0x622520`-`0x62267d`): a slot held at its rank, the
        // preview's under isPreview (`0x62256d`), shows nothing; without a group every slot shows.
        for p in t.prereqs.iter().filter(|_| !held) {
            if p.seat.is_none() || p.spell == 0 || rank_of(p.talent) >= p.rank as i32 {
                continue;
            }
            met = false;
            let Some(name) = &p.name else { continue };
            // The key is read with no ordinal (`0x622626`), as `GetText` reads -1.
            let tmpl = super::get_text::get_text(lua, "TOOLTIP_TALENT_PREREQ", -1, 0)
                .ok()
                .map(|s| s.to_string_lossy().to_string())
                .filter(|s| !s.is_empty());
            reqs.extend(tmpl.map(|f| {
                crate::strings::fill(
                    &f,
                    &[
                        crate::strings::Arg::D((p.rank + 1).into()),
                        crate::strings::Arg::S(name),
                    ],
                )
            }));
        }
        // Then the tier line, only with a group (`0x62268b`): the group tab's spent plus preview
        // points whatever isPreview (`0x6226d6`, `0x6226d9`), 5 a tier or 3 for the pet (`0x6226ea`).
        if let Some(g) = group.filter(|_| !held) {
            let tab_points = g.tab(t_index).map_or(0, |gt| gt.spent + gt.preview_spent);
            let per_tier = if pet { 3 } else { 5 };
            let need = per_tier * (t.tier as i32 - 1);
            if need > tab_points {
                met = false;
                reqs.extend(get("TOOLTIP_TALENT_TIER_POINTS").map(|f| {
                    crate::strings::fill(
                        &f,
                        &[
                            crate::strings::Arg::D(need.into()),
                            crate::strings::Arg::S(&view.name),
                        ],
                    )
                }));
            }
        }
        // The required spell (`0x622759` into `0x5c60c0`): an inspected player knows it; else our
        // player or the pet, asked at the call (`0x5c6128`, `0x5c6134`).
        let knows = |spell| {
            if pet {
                live.pet_spell_known_or_higher(spell)
            } else {
                live.spell_known_or_higher(spell)
            }
        };
        if !held && !inspect && t.required_spell != 0 && !knows(t.required_spell) {
            if let Some(name) = &t.required_spell_name {
                met = false;
                reqs.extend(
                    get("ITEM_REQ_SKILL")
                        .map(|f| crate::strings::fill(&f, &[crate::strings::Arg::S(name)])),
                );
            }
        }
        // The closing hints (`0x622800`), for the learning group alone: the talent can be learned
        // when it holds a rank or every requirement is met (`0x6224f0`); then, without
        // preview, points unspent and a rank to go show the learn line, and with preview each move
        // that would pass.
        let learning = !inspect && group_index == if pet { 0 } else { info.active_group() };
        let mut hints = Vec::new();
        if (held || met) && learning {
            if let Some(g) = group {
                if !use_preview {
                    if g.unspent != 0 && rank + 1 < max {
                        hints.push(TalentHint::Learn);
                    }
                } else {
                    let find = |id| info.find(side_of(false, pet), id);
                    if g.validate_preview(t_index, t, 1, pet, &find).is_some() {
                        hints.push(TalentHint::AddPreview);
                    }
                    if g.validate_preview(t_index, t, -1, pet, &find).is_some() {
                        hints.push(TalentHint::RemovePreview);
                    }
                }
            }
        }
        Some((
            display,
            TalentLines {
                exceptional: t.exceptional,
                rank_line,
                reqs,
                next_spell,
                next_desc: None,
                hints,
            },
        ))
    })
    .flatten()
}

/// The preview total a verb compares around its write (`0x5c6010`).
fn preview_total(pet: bool, group: u32) -> i32 {
    with_info(|info, _| {
        info.group(side_of(false, pet), group)
            .map_or(0, TalentGroup::preview_spent)
    })
    .unwrap_or(0)
}

/// Raise a side's preview event, the pet's being the next id (`0x5c7200`).
fn raise_preview(lua: &Lua, pet: bool, args: [i64; 4]) {
    let event = if pet {
        "PREVIEW_PET_TALENT_POINTS_CHANGED"
    } else {
        "PREVIEW_TALENT_POINTS_CHANGED"
    };
    super::tick::fire_event_into(lua, event, args.map(ScriptValue::Int).to_vec());
}

/// The `previewTalents` callback (`0x5127d0`): every player group's preview back to its ranks
/// (`0x5c70d0`), `PREVIEW_TALENT_POINTS_CHANGED(0, 0, 0, -spent)`, then the pet's and its event.
pub(super) fn preview_talents_changed(lua: &Lua) {
    for pet in [false, true] {
        let spent = super::write_live(|l| {
            let info = l.talent_info_mut()?;
            let spent = info.sum_preview_spent(pet);
            info.recalculate_preview(pet);
            Some(spent)
        })
        .flatten()
        .unwrap_or(0);
        raise_preview(lua, pet, [0, 0, 0, -i64::from(spent)]);
    }
}

/// Register the talent globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumTalentTabs([isInspect[, isPet]]) (`0x5c5cc0`): the inspected pet has none.
    g.set(
        "GetNumTalentTabs",
        lua.create_function(|_, args: MultiValue| {
            let (inspect, pet) = flags_at(&args, 0);
            if inspect && pet {
                return Ok(0);
            }
            Ok(
                with_info(|info, _| info.pages(side_of(inspect, pet)).tabs.len() as i64)
                    .unwrap_or(0),
            )
        })?,
    )?;

    // GetTalentTabInfo(tab[, isInspect[, isPet[, groupIndex]]]) (`0x5c6150`): name, iconTexture,
    // the group tab's pointsSpent, background, its previewPointsSpent; an absent tab answers nil,
    // nil, 0, nil, 0, and a group without the tab 0 and 0 (`0x5c6291`).
    g.set(
        "GetTalentTabInfo",
        lua.create_function(|lua, args: MultiValue| {
            let Some(tab) = int_arg(&args, 0) else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: GetTalentTabInfo(tabIndex[, isInspect[, isPet[, groupIndex]]])".into(),
                ));
            };
            let (inspect, pet) = flags_at(&args, 1);
            let row = with_info(|info, _| {
                let (t, view, _) = tab_of(info, inspect, pet, tab - 1)?;
                let points = group_of(info, inspect, pet, script_group(info, args.get(3)))
                    .and_then(|g| g.tab(t))
                    .map_or((0, 0), |gt| (gt.spent, gt.preview_spent));
                Some((view.clone(), points))
            })
            .flatten();
            let Some((view, (spent, previewed))) = row else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Integer(0),
                    Value::Nil,
                    Value::Integer(0),
                ]));
            };
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
        lua.create_function(|_, args: MultiValue| {
            let Some(tab) = int_arg(&args, 0) else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: GetNumTalents(tabIndex[, isInspect[, isPet]])".into(),
                ));
            };
            let (inspect, pet) = flags_at(&args, 1);
            Ok(with_info(|info, _| {
                tab_of(info, inspect, pet, tab - 1).map_or(0, |(_, _, list)| list.len() as i64)
            })
            .unwrap_or(0))
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
            let Some(r) = talent_info_row(&args) else {
                return Ok(placeholder());
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&r.name)?),
                match r.icon {
                    Some(icon) => Value::String(lua.create_string(&icon)?),
                    None => Value::Nil,
                },
                Value::Integer(i64::from(r.tier)),
                Value::Integer(i64::from(r.column)),
                Value::Integer(i64::from(r.rank + 1)),
                Value::Integer(i64::from(r.max)),
                one_or_nil(r.exceptional),
                one_or_nil(r.meets),
                Value::Integer(i64::from(r.preview + 1)),
                one_or_nil(r.meets_preview),
            ]))
        })?,
    )?;

    // GetTalentPrereqs(tab, index[, isInspect[, isPet[, groupIndex]]]) (`0x5c7ed0`): per
    // prerequisite slot naming a record, its tier and column, then whether the group's rank and
    // preview rank reach the needed one (nil, nil for a talent the group does not hold).
    g.set(
        "GetTalentPrereqs",
        lua.create_function(|_, args: MultiValue| {
            let (Some(tab), Some(index)) = (int_arg(&args, 0), int_arg(&args, 1)) else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: GetTalentPrereqs(tabIndex, talentIndex[, isInspect[, isPet[, groupIndex]]])"
                        .into(),
                ));
            };
            let (inspect, pet) = flags_at(&args, 2);
            let out = with_info(|info, _| {
                let mut out = Vec::new();
                let Some((_, _, list)) = tab_of(info, inspect, pet, tab - 1) else {
                    return out;
                };
                let Some(t) = usize::try_from(index - 1).ok().and_then(|i| list.get(i)) else {
                    return out;
                };
                let group = group_of(info, inspect, pet, script_group(info, args.get(4)));
                for p in &t.prereqs {
                    let Some((tier, column)) = p.seat else {
                        continue;
                    };
                    out.push(Value::Integer(i64::from(tier)));
                    out.push(Value::Integer(i64::from(column)));
                    match group.and_then(|g| g.item(p.talent)) {
                        Some(item) => {
                            out.push(one_or_nil(item.rank >= p.rank as i32));
                            out.push(one_or_nil(item.preview >= p.rank as i32));
                        }
                        None => {
                            out.push(Value::Nil);
                            out.push(Value::Nil);
                        }
                    }
                }
                out
            })
            .unwrap_or_default();
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetTalentLink(tab, index[, isInspect[, isPet[, groupIndex]]]) (`0x5c7ce0`):
    // `|cff4e96f7|Htalent:<id>:<rank>|h[<name>]|h|r` (`0x61a550`), the rank 0-based or -1; a pet
    // talent the pet may not take has none (`0x5c7ddf`). The usage text names a sixth isPreview,
    // but the preview-over-rank flag is argument 5 read again as a boolean (`0x5c7e13`).
    g.set(
        "GetTalentLink",
        lua.create_function(|lua, args: MultiValue| {
            let (Some(tab), Some(index)) = (int_arg(&args, 0), int_arg(&args, 1)) else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: GetTalentLink(tabIndex, talentIndex[, isInspect[, isPet[, groupIndex[, isPreview]]]])"
                        .into(),
                ));
            };
            let (inspect, pet) = flags_at(&args, 2);
            let link = with_info(|info, live| {
                let (_, _, list) = tab_of(info, inspect, pet, tab - 1)?;
                let t = list.get(usize::try_from(index - 1).ok()?)?;
                if pet && !pet_may_take(live, t) {
                    return None;
                }
                let rank = group_of(info, inspect, pet, script_group(info, args.get(4)))
                    .and_then(|g| g.item(t.id))
                    .map_or(-1, |i| {
                        if super::binding_abi::bool_or_default(args.get(4), false) {
                            i.preview
                        } else {
                            i.rank
                        }
                    });
                let view = t.rank_view(rank)?;
                Some(format!("|cff4e96f7|Htalent:{}:{}|h[{}]|h|r", t.id, rank, view.name))
            })
            .flatten();
            match link {
                Some(link) => Ok(Value::String(lua.create_string(&link)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetActiveTalentGroup([isInspect[, isPet]]) (`0x5c5810`): the inspected player's whatever
    // `isPet`, the pet's always its one.
    g.set(
        "GetActiveTalentGroup",
        lua.create_function(|_, args: MultiValue| {
            let (inspect, pet) = flags_at(&args, 0);
            let group = with_info(|info, _| {
                if inspect {
                    info.inspect_active_group()
                } else if pet {
                    0
                } else {
                    info.active_group()
                }
            })
            .unwrap_or(0);
            Ok(i64::from(group) + 1)
        })?,
    )?;

    // GetNumTalentGroups([isInspect[, isPet]]) (`0x5c5df0`): the inspected player's whatever
    // `isPet`, the pet's 1 while it has a family and tabs.
    g.set(
        "GetNumTalentGroups",
        lua.create_function(|_, args: MultiValue| {
            let (inspect, pet) = flags_at(&args, 0);
            Ok(with_info(|info, _| {
                if inspect {
                    info.num_inspect_groups()
                } else if pet {
                    u32::from(info.pet_has_tree())
                } else {
                    info.num_groups()
                }
            })
            .map_or(0, i64::from))
        })?,
    )?;

    // SetActiveTalentGroup(group) (`0x5c5e70`): another group below the count and two casts its
    // activation spell on the player (`0x5c5edc`); the server's `SMSG_TALENTS_INFO` then names it.
    g.set(
        "SetActiveTalentGroup",
        lua.create_function(|lua, group: Value| {
            let Some(group) = number_of(&group) else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: SetActiveTalentGroup(groupIndex)".into(),
                ));
            };
            let g = (group as i64 - 1) as u32;
            let (active, count) =
                with_info(|info, _| (info.active_group(), info.num_groups())).unwrap_or((0, 0));
            if g != active && g < count && (g as usize) < ACTIVATION_SPELLS.len() {
                super::calls::make(
                    lua,
                    super::ScriptCall::CastSpell {
                        spell_id: ACTIVATION_SPELLS[g as usize],
                        at: super::calls::CastAt::Player,
                        via_book: false,
                    },
                );
            }
            Ok(())
        })?,
    )?;

    // GetUnspentTalentPoints([isInspect[, isPet[, groupIndex]]]) (`0x5c6310`).
    g.set(
        "GetUnspentTalentPoints",
        lua.create_function(|_, args: MultiValue| {
            let (inspect, pet) = flags_at(&args, 0);
            Ok(with_info(|info, _| {
                group_of(info, inspect, pet, script_group(info, args.get(2)))
                    .map_or(0, |g| g.unspent)
            })
            .map_or(0, i64::from))
        })?,
    )?;

    // GetGroupPreviewTalentPointsSpent([isPet[, groupIndex]]) (`0x5c6420`).
    g.set(
        "GetGroupPreviewTalentPointsSpent",
        lua.create_function(|_, args: MultiValue| {
            let pet = flag_arg(args.front());
            let group = with_info(|info, _| script_group(info, args.get(1))).unwrap_or(0);
            Ok(i64::from(preview_total(pet, group)))
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

    // AddPreviewTalentPoints(tab, index, points[, isPet[, groupIndex]]) (`0x5c9590`): the move
    // (`0x5c94f0`), then `PREVIEW_TALENT_POINTS_CHANGED`, or the pet's, (talentIndex, tabIndex,
    // group, delta) inside the call when the group's preview total moved.
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
            let pet = flag_arg(args.get(3));
            let group = with_info(|info, _| script_group(info, args.get(4))).unwrap_or(0);
            let before = preview_total(pet, group);
            super::write_live(|l| {
                let info = l.talent_info_mut()?;
                let (t, _, list) = tab_of(info, false, pet, tab - 1)?;
                let talent = list.get(usize::try_from(index - 1).ok()?)?.clone();
                let side = side_of(false, pet);
                let pages = info.pages_shared(side);
                let find = |id| pages.talents.iter().flatten().find(|v| v.id == id).cloned();
                info.group_mut(pet, group)?
                    .add_preview(t, &talent, points as i32, pet, &find)
            });
            let after = preview_total(pet, group);
            if after != before {
                raise_preview(
                    lua,
                    pet,
                    [index, tab, i64::from(group) + 1, i64::from(after - before)],
                );
            }
            Ok(())
        })?,
    )?;

    // ResetPreviewTalentPoints(tab[, isPet[, groupIndex]]) (`0x5c7130`) and
    // ResetGroupPreviewTalentPoints([isPet[, groupIndex]]) (`0x5c7200`): the preview ranks back
    // to the learned ones, then the event inside the call when the total moved, its index 0 for
    // "every talent" (and its tab 0 for every tab).
    g.set(
        "ResetPreviewTalentPoints",
        lua.create_function(|lua, args: MultiValue| {
            let Some(tab) = int_arg(&args, 0) else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: ResetPreviewTalentPoints(tabIndex[, isPet[, groupIndex]])".into(),
                ));
            };
            let pet = flag_arg(args.get(1));
            let group = with_info(|info, _| script_group(info, args.get(2))).unwrap_or(0);
            let before = preview_total(pet, group);
            super::write_live(|l| {
                let g = l.talent_info_mut()?.group_mut(pet, group)?;
                g.reset_preview(usize::try_from(tab - 1).ok()?);
                Some(())
            });
            let after = preview_total(pet, group);
            if after != before {
                raise_preview(
                    lua,
                    pet,
                    [0, tab, i64::from(group) + 1, i64::from(after - before)],
                );
            }
            Ok(())
        })?,
    )?;
    g.set(
        "ResetGroupPreviewTalentPoints",
        lua.create_function(|lua, args: MultiValue| {
            let pet = flag_arg(args.front());
            let group = with_info(|info, _| script_group(info, args.get(1))).unwrap_or(0);
            let before = preview_total(pet, group);
            super::write_live(|l| {
                l.talent_info_mut()?
                    .group_mut(pet, group)?
                    .reset_all_preview();
                Some(())
            });
            let after = preview_total(pet, group);
            if after != before {
                raise_preview(
                    lua,
                    pet,
                    [0, 0, i64::from(group) + 1, i64::from(after - before)],
                );
            }
            Ok(())
        })?,
    )?;

    // LearnPreviewTalents([isPet]) (`0x5c6a10`): the active group's (the pet's one) previewed
    // ranks, sent in the call when the tabs' preview total is above 0 (`0x5c6aa1`) and the unit
    // resolves (`0x5c6ae4`): `CMSG_LEARN_PREVIEW_TALENTS` (`0x5c6b2b`), or the pet's form
    // (`0x5c6b0d`) with the pet's guid (`0x5c6b24`).
    g.set(
        "LearnPreviewTalents",
        lua.create_function(|_, args: MultiValue| {
            let pet = flag_arg(args.front());
            super::write_live(|l| {
                let unit = l.talent_unit(pet)?;
                let info = l.talent_info()?;
                let active = if pet { 0 } else { info.active_group() };
                let g = info.group(side_of(false, pet), active)?;
                if g.preview_spent() <= 0 {
                    return None;
                }
                let sends = g.preview_learns();
                if !sends.is_empty() {
                    l.send_learn_preview_talents(pet.then_some(unit), sends);
                }
                Some(())
            });
            Ok(())
        })?,
    )?;

    // LearnTalent(tab, index[, isPet]) (`0x5c80e0`): with the unit resolved (`0x5c81df`), the
    // active group's next rank below the max, sent in the call: `CMSG_LEARN_TALENT`, or
    // `CMSG_PET_LEARN_TALENT` with the pet's guid (`0x5c8288`); a pet talent the pet
    // may not take sends nothing (`0x5c820c`). Points and prerequisites are the server's to
    // enforce.
    g.set(
        "LearnTalent",
        lua.create_function(|_, args: MultiValue| {
            let (Some(tab), Some(index)) = (int_arg(&args, 0), int_arg(&args, 1)) else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: LearnTalent(tabIndex, talentIndex[, isPet])".into(),
                ));
            };
            let pet = flag_arg(args.get(2));
            super::write_live(|l| {
                let unit = l.talent_unit(pet)?;
                let info = l.talent_info()?;
                let (_, _, list) = tab_of(info, false, pet, tab - 1)?;
                let t = list.get(usize::try_from(index - 1).ok()?)?;
                if pet && !pet_may_take(&*l, t) {
                    return None;
                }
                let active = if pet { 0 } else { info.active_group() };
                let rank = info
                    .group(side_of(false, pet), active)
                    .and_then(|g| g.item(t.id))
                    .map_or(-1, |i| i.rank);
                let (id, max) = (t.id, t.max_rank() - 1);
                if rank < max {
                    l.send_learn_talent(pet.then_some(unit), id, (rank + 1) as u32);
                }
                Some(())
            });
            Ok(())
        })?,
    )?;

    // ConfirmTalentWipe(): the dialog's Accept, the talent master talk with no guid (`0x51bf84`); its refusal line shows from the call (`0x6db2b0`).
    g.set(
        "ConfirmTalentWipe",
        lua.create_function(|lua, ()| {
            if let Some(key) = super::read_live(|l| l.confirm_talent_wipe()).flatten() {
                super::ui_errors::show_now(lua, key);
            }
            Ok(())
        })?,
    )?;

    // CheckTalentMasterDist(): polled by that dialog's OnUpdate, 1 or nil (`0x51b740`).
    g.set(
        "CheckTalentMasterDist",
        lua.create_function(|_, ()| {
            Ok(super::binding_abi::flag(
                super::read_live(|l| l.talent_master_in_reach()).unwrap_or(false),
            ))
        })?,
    )?;

    // UnitCharacterPoints(unit) (`0x610fb0`): a string or number token (`0x610fbd`); the active
    // player's `PLAYER_CHARACTER_POINTS1/2` at the call, signed (`0x61102e`), 0 and 0 for any
    // other unit.
    g.set(
        "UnitCharacterPoints",
        lua.create_function(|lua, unit: Value| {
            let token = match &unit {
                Value::String(s) => s.to_str()?.to_string(),
                Value::Integer(i) => i.to_string(),
                Value::Number(n) => n.to_string(),
                _ => {
                    return Err(mlua::Error::RuntimeError(
                        "Usage: UnitCharacterPoints(\"unit\")".into(),
                    ))
                }
            };
            let guid = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.token_guid(&token)
            };
            let points = super::read_live(|l| {
                let me = l.active_player()?;
                (guid == Some(me)).then(|| l.character_points()).flatten()
            })
            .flatten()
            .unwrap_or((0, 0));
            Ok((i64::from(points.0 as i32), i64::from(points.1 as i32)))
        })?,
    )?;

    Ok(())
}

/// `SetHyperlink`'s `talent:` arm (`0x62dae0`): the link's rank of a talent of any class. Rank -1
/// reads the first, with nothing learned.
pub(super) fn set_talent_link(lua: &Lua, this: &Table, id: u32, rank: i32) -> mlua::Result<()> {
    let source = crate::script::model_mut_of(lua).spell_info.clone();
    let Some((spell, ranks)) = source.and_then(|s| s.talent_rank_spell(id, rank.max(0))) else {
        return Ok(());
    };
    let shown = rank.max(-1) + 1;
    let rank_line = crate::strings::global(lua, "TOOLTIP_TALENT_RANK").map(|t| {
        crate::strings::fill(
            &t,
            &[
                crate::strings::Arg::D(i64::from(shown)),
                crate::strings::Arg::D(i64::from(ranks)),
            ],
        )
    });
    super::tooltip_spell::set_spell_with_talent(
        lua,
        this,
        spell,
        super::tooltip_spell::TalentLines {
            rank_line,
            ..Default::default()
        },
    )
}

/// Register `GameTooltip:SetTalent(tab, index[, isInspect[, isPet[, groupIndex[, isPreview]]]])`
/// into the tooltip method table.
pub(super) fn install_tooltip_method(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.set(
        "SetTalent",
        lua.create_function(|lua, (this, args): (Table, MultiValue)| {
            let Some((display, mut lines)) = tooltip_lines(lua, &args) else {
                return Ok(());
            };
            if lines.next_spell != 0 {
                lines.next_desc = super::tooltip_spell::spell_view_of(lua, lines.next_spell, false)
                    .map(|v| v.description);
            }
            super::tooltip_spell::set_spell_with_talent(lua, &this, display, lines)
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
