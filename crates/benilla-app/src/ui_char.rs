//! The character window's reads: the [`UnitCombatStats`] figures and the slot views the paper
//! doll's Lua bindings read at the call, built from our avatar's descriptor and item stores; the
//! skill list; and the booth's yaw from the stock `CharacterModelFrame`. [`unit_combat_stats`] is
//! the half any unit shares: a creature has no player fields, so those read absent and keep their
//! defaults.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_formats::{GameTableId, GameTables};
use benilla_protocol::messages::{PLAYER_COMBAT_RATINGS, PLAYER_SKILL_SLOTS};
use benilla_ui::script::{
    weapon_subclass_skill, InvSlotView, ScriptValue, SkillEntry, SkillLive, SkillsState, UiScript,
    UnitCombatStats, SKILL_DEFENSE, SKILL_UNARMED,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::pending_item_ops::PendingItemOps;
use crate::portrait::PaperDollBooth;
use crate::ui_items::{find_equip_slot, item_link};
use crate::ui_script::UiFeed;

/// Equipment slots, 0-based (`EQUIPMENT_SLOT_*`): main hand, off hand, ranged.
const SLOT_MAIN_HAND: u8 = 15;
const SLOT_OFF_HAND: u8 = 16;
const SLOT_RANGED: u8 = 17;
/// Item class 2, weapon (vmangos `ItemPrototype.h:139`).
const ITEM_CLASS_WEAPON: u32 = 2;
/// Weapon subclass 19, wand (`ItemPrototype.h:211`), for `HasWandEquipped`.
const SUBCLASS_WAND: u32 = 19;

/// The gt tables and form flags behind the sheet's derived figures; absent, those read 0.
#[derive(Resource)]
pub(crate) struct SheetTables {
    pub(crate) gt: GameTables,
    /// `SpellShapeshiftForm.dbc` `flags1` by form id; bit `0x20` lets agility add attack power.
    pub(crate) form_flags: HashMap<u32, u32>,
}

/// The spirit health-regen rate's cap on the spirit `gtOCTRegenHP` prices (`[0xadaa10]`).
const SPIRIT_REGEN_CAP: i32 = 50;

/// The figures 12340 derives from a unit's level, class and stats through the gt tables, for the
/// player or a pet: crit from agility (`0x71bae0`), spell crit from intellect (`0x71bb70`), and
/// health and mana regen from spirit (`0x71ba60`, `0x71b9f0`).
fn gt_derived(store: &ObjectStore, gt: &GameTables) -> [f32; 4] {
    let f = &store.0;
    let row = f.unit_level().unwrap_or(0) as i32 - 1;
    let col = ((f.unit_bytes_0().unwrap_or(0) >> 8) & 0xff) as i32 - 1;
    // Each stat floored at 0, as the reads' `(x < 0) - 1 & x`.
    let stat = |i: u8| (f.unit_stat(i).unwrap_or(0) as i32).max(0);
    let chance = |per: GameTableId, base: GameTableId, points: i32| {
        let per = gt.lookup(per, row, col);
        if per == 0.0 {
            return 0.0;
        }
        ((f64::from(per) * f64::from(points) + f64::from(gt.lookup(base, 0, col))) * 100.0) as f32
    };
    let crit_from_agility = chance(
        GameTableId::ChanceToMeleeCrit,
        GameTableId::ChanceToMeleeCritBase,
        stat(1),
    );
    let spell_crit_from_intellect = chance(
        GameTableId::ChanceToSpellCrit,
        GameTableId::ChanceToSpellCritBase,
        stat(3),
    );
    let spirit = stat(4);
    let capped = spirit.min(SPIRIT_REGEN_CAP);
    let health_regen = f64::from(capped) * f64::from(gt.lookup(GameTableId::OctRegenHp, row, col))
        + f64::from(gt.lookup(GameTableId::RegenHpPerSpt, row, col)) * f64::from(spirit - capped);
    let mana_regen = f64::from(stat(3)).sqrt()
        * f64::from(gt.lookup(GameTableId::RegenMpPerSpt, row, col))
        * f64::from(spirit);
    [
        crit_from_agility,
        spell_crit_from_intellect,
        health_regen as f32,
        mana_regen as f32,
    ]
}

/// The player's combat rating percentage (`0x6de410`) for each rating: the points over the level's
/// `gtCombatRatings` price, times the class's `gtOCTClassCombatRatingScalar`; 0 when either is 0.
fn combat_rating_bonus(store: &ObjectStore, gt: &GameTables) -> [f32; 25] {
    let f = &store.0;
    let level_row = f.unit_level().unwrap_or(0) as i32 - 1;
    let class_col = ((f.unit_bytes_0().unwrap_or(0) >> 8) & 0xff) as i32 - 1;
    std::array::from_fn(|i| {
        let price = gt.lookup(GameTableId::CombatRatings, level_row, i as i32);
        let scalar = gt.lookup(
            GameTableId::OctClassCombatRatingScalar,
            i as i32 + 1,
            class_col,
        );
        if price == 0.0 || scalar == 0.0 {
            return 0.0;
        }
        let rating = f.player_combat_rating(i as u8).unwrap_or(0);
        (f64::from(rating) / f64::from(price) * f64::from(scalar)) as f32
    })
}

/// An f32 snapshot field as a diff key, quantized to 1/32 so equal wire values compare equal.
fn fixed(v: f32) -> i64 {
    (v * 32.0).round() as i64
}

/// The client's skill list (`0xc22ae8`-`0xc22b18`): set up by the game start (`0x52b81e` -> `0x5ce880`), written by the rebuild (`0x5cd9b0`), kept across a UI reload, freed by the game shutdown (`0x5291c2` -> `0x5cd3a0`).
#[derive(Resource, Default)]
pub(crate) struct SkillInfo(pub(crate) benilla_ui::script::SkillList);

/// The character window's booth and the skill list; the bindings are `benilla-ui`'s `char_stats` and `skills`.
pub(crate) struct UiCharPlugin;

impl Plugin for UiCharPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SkillInfo>()
            .add_systems(Update, sync_char_booth.in_set(UiFeed))
            // After the UI shutdown's `PLAYER_LEAVING_WORLD` and `PLAYER_LOGOUT` (`0x529160`: `0x528f00`, then `0x5291c2`).
            .add_systems(
                OnExit(crate::char_select::ClientState::InWorld),
                free_skills.after(crate::ui_script::end_ui_session),
            );
    }
}

/// The game shutdown's free of the skill list (`0x5cd3a0`).
fn free_skills(mut list: ResMut<SkillInfo>) {
    *list = SkillInfo::default();
}

/// The yaw `CharacterModelFrame` draws onto the booth, read each frame as the model frame renders (`0x960240`), the facing its rotate buttons write (`0x9606e0`); written only on change, so the booth is not marked changed every frame.
fn sync_char_booth(script: Option<NonSend<UiScript>>, mut booth: ResMut<PaperDollBooth>) {
    let Some(script) = script else {
        return;
    };
    let yaw = script.model_pane_facing("CharacterModelFrame");
    if booth.yaw != yaw {
        booth.yaw = yaw;
    }
}

/// The character page's world entry (`0x5281bd` -> `0x5eaf30`): the alert recompute with its events (`0x5eaf5e` -> `0x5e90d0`), in turn after the bar's entry (`0x5281b8`).
pub(crate) fn enter_world(world: &mut World) {
    crate::ui_script::live::in_turn(world, update_inventory_alerts);
}

/// The alert recompute (`0x5e90d0`) over the slots read at the step, `UPDATE_INVENTORY_ALERTS` on a changed status then `UPDATE_INVENTORY_DURABILITY`.
pub(crate) fn update_inventory_alerts(world: &mut World) {
    crate::ui_script::live::with_live_world(world, |script| script.update_inventory_alerts());
}

/// The skill list as the rebuild makes it (`0x5cd9b0`): each held line its `SkillLine.dbc`, race and class row and category admit, in slot order.
pub(crate) fn skills_state(
    fields: &benilla_protocol::ObjectFields,
    catalog: Option<&benilla_formats::SkillLineCatalog>,
) -> SkillsState {
    let (race, class) = (
        fields.unit_race().unwrap_or(0),
        fields.unit_class().unwrap_or(0),
    );
    let level = fields.unit_level().unwrap_or(0);
    let entries = catalog.map_or_else(Vec::new, |catalog| {
        (0..PLAYER_SKILL_SLOTS)
            .filter_map(|i| fields.player_skill(i))
            .filter_map(|slot| skills_row(&slot, catalog, race, class, level))
            .collect()
    });
    SkillsState {
        entries,
        points: fields.player_free_professions().unwrap_or(0),
    }
}

/// The skill list's rebuild (`0x5cd9b0`): with the active player held, the list rebuilt off its fields, every group expanded, then `SKILL_LINES_CHANGED` (`0x5cd5e0`); run in turn with the step's other raises.
pub(crate) fn rebuild_skills(world: &mut World) {
    let state = {
        let mut me = world.query_filtered::<&ObjectStore, With<SelfPlayer>>();
        let Some(store) = me.iter(world).next() else {
            return;
        };
        let catalog = world
            .get_resource::<crate::ui_spellbook::SkillLines>()
            .map(|l| l.catalog.clone());
        skills_state(&store.0, catalog.as_deref())
    };
    if let Some(mut list) = world.get_resource_mut::<SkillInfo>() {
        list.0.rebuild(state);
    }
    match world.get_resource_mut::<crate::ui_script::live::UiEvents>() {
        Some(mut ui) => ui.fire("SKILL_LINES_CHANGED", Vec::new()),
        None => {
            crate::ui_script::live::with_live_world(world, |script| {
                script.fire_event("SKILL_LINES_CHANGED", Vec::new());
            });
        }
    }
}

/// The slot the skill readers index for line `skill_id` (`0x6dc1c0`): a miss turns into index -1 (`0x6dc218`-`0x6dc222`), whose reads land on the words before the array: id|step on `PLAYER_FIELD_KNOWN_CURRENCIES`' second word, value|max on `PLAYER_XP`, temp|perm on `PLAYER_NEXT_LEVEL_XP`.
pub(crate) fn found_skill_slot(
    fields: &benilla_protocol::ObjectFields,
    skill_id: u32,
) -> benilla_protocol::messages::PlayerSkillSlot {
    (0..PLAYER_SKILL_SLOTS)
        .filter_map(|i| fields.player_skill(i))
        .find(|s| u32::from(s.skill_id) == skill_id)
        .unwrap_or_else(|| {
            let halves = |w: u32| (w as u16, (w >> 16) as u16);
            let (id, step) = halves((fields.player_known_currencies() >> 32) as u32);
            let (value, max) = halves(fields.player_xp().unwrap_or(0));
            let (temp, perm) = halves(fields.player_next_level_xp().unwrap_or(0));
            benilla_protocol::messages::PlayerSkillSlot {
                skill_id: id,
                step,
                value,
                max,
                temp_bonus: temp as i16,
                perm_bonus: perm as i16,
            }
        })
}

/// Line `skill_id`'s slot as `GetSkillLineInfo` reads it at the call (`0x5cdfe7`-`0x5ce00a`): the ranks, the bonuses, the abandon flag off the step (`0x5ce147`) and the tier it opens (`0x5ce16d`); a line the player lacks reads slot -1 unchecked ([`found_skill_slot`]).
pub(crate) fn skill_slot(
    fields: &benilla_protocol::ObjectFields,
    catalog: Option<&benilla_formats::SkillLineCatalog>,
    skill_id: u32,
) -> SkillLive {
    let slot = found_skill_slot(fields, skill_id);
    let (race, class) = (
        fields.unit_race().unwrap_or(0),
        fields.unit_class().unwrap_or(0),
    );
    let rc = catalog.and_then(|c| c.race_class(skill_id, race, class).map(|rc| (c, rc)));
    let tier_cost = rc.map_or(0, |(c, rc)| {
        if rc.flags & 8 != 0 || (slot.step == 0 && rc.flags & 4 != 0) {
            c.tier_cost(rc.tier_id, u32::from(slot.step))
        } else {
            0
        }
    });
    SkillLive {
        value: u32::from(slot.value),
        max: u32::from(slot.max),
        temp_bonus: i32::from(slot.temp_bonus),
        perm_bonus: i32::from(slot.perm_bonus),
        abandonable: slot.step != 0 && rc.is_some_and(|(_, rc)| rc.unlearnable()),
        tier_cost,
    }
}

/// The three skill stat events (`0x6cef40`): `UNIT_ATTACK`, `UNIT_DEFENSE` and `UNIT_RANGED_ATTACK_POWER` with `"player"`.
pub(crate) fn fire_skill_stat_events(ui: &mut crate::ui_script::live::UiEvents) {
    for event in ["UNIT_ATTACK", "UNIT_DEFENSE", "UNIT_RANGED_ATTACK_POWER"] {
        ui.fire(event, vec![ScriptValue::Str("player".into())]);
    }
}

/// A line's `(rank, max)` as the trade-link builder reads them (`0x5db120`-`0x5db134`), a missing line's slot -1 passed on unchecked ([`found_skill_slot`]): the value (`0x51a250`) and the max (`0x594710`) each add the permanent bonus when positive.
pub(crate) fn link_ranks(fields: &benilla_protocol::ObjectFields, skill_id: u32) -> (u32, u32) {
    let slot = found_skill_slot(fields, skill_id);
    let perm = slot.perm_bonus as u16;
    let read = |half: u16| u32::from(half) + if half > 0 { u32::from(perm) } else { 0 };
    (read(slot.value), read(slot.max))
}

/// The skill value reader (`0x51a250`): the value half (`0x51a286`), plus the permanent-bonus half read unsigned when the value is positive (`0x51a291`-`0x51a299`).
pub(crate) fn skill_value_read(slot: &benilla_protocol::messages::PlayerSkillSlot) -> i32 {
    let value = i32::from(slot.value);
    if value > 0 {
        value + i32::from(slot.perm_bonus as u16)
    } else {
        value
    }
}

/// The skill value watch (`0x6da4a0`) on slot `slot`, its old raw value half `old`: a read (`0x6da4fa`) that differs from `old` fires the stat events (`0x6da509`), rebuilds the list (`0x6da531`) and raises the gain or rank line (`0x6da59f`); then the trainer's refresh (`0x6da5f3`), the trade-skill list's refresh (`0x6da5fa`, the trade-skill module's, #1282) and the merchant's (`0x6da602`).
pub(crate) fn skill_value_watch(world: &mut World, slot: u8, old: u16) {
    let moved = {
        let mut me = world.query_filtered::<&ObjectStore, With<SelfPlayer>>();
        me.iter(world).next().and_then(|s| {
            let skill = s.0.player_skill(slot)?;
            let read = skill_value_read(&skill);
            let race_class = (s.0.unit_race().unwrap_or(0), s.0.unit_class().unwrap_or(0));
            (read != i32::from(old)).then_some((u32::from(skill.skill_id), read, race_class))
        })
    };
    if let Some((skill_id, read, (race, class))) = moved {
        if let Some(mut ui) = world.get_resource_mut::<crate::ui_script::live::UiEvents>() {
            fire_skill_stat_events(&mut ui);
        }
        crate::ui_script::live::in_turn(world, rebuild_skills);
        // The display at its step (`0x6da5b4`, `0x6da5d9`); its `CHAT_MSG_SKILL` still fires from the chat feed (#1416).
        if let Some(line) = skill_line(world, skill_id, race, class, old, read) {
            match world.get_resource_mut::<crate::ui_script::live::UiEvents>() {
                Some(mut ui) => crate::ui_action::show_in_turn(&mut ui, "skills", line),
                None => crate::ui_action::show_line_in_world(world, "skills", line),
            }
        }
    }
    crate::ui_script::live::in_turn(world, crate::ui_trainer::refresh);
    crate::ui_script::live::in_turn(world, crate::ui_merchant::refresh);
}

/// The skill max and permanent-bonus watch (`0x6cddb0`): the trainer's refresh, the trade-skill list's refresh (`0x6cddb7`, the trade-skill module's, #1282), then the rebuild (`0x6cddbf`).
pub(crate) fn skill_max_watch(world: &mut World) {
    crate::ui_script::live::in_turn(world, crate::ui_trainer::refresh);
    crate::ui_script::live::in_turn(world, rebuild_skills);
}

/// The skill temporary-bonus watch (`0x6d1bc0`): with the player held, the three stat events (`0x6d1bf2`-`0x6d1c17`), then the rebuild (`0x6d1c1f`).
pub(crate) fn skill_temp_watch(world: &mut World) {
    let held = world
        .query_filtered::<(), With<SelfPlayer>>()
        .iter(world)
        .next()
        .is_some();
    if held {
        if let Some(mut ui) = world.get_resource_mut::<crate::ui_script::live::UiEvents>() {
            fire_skill_stat_events(&mut ui);
        }
    }
    crate::ui_script::live::in_turn(world, rebuild_skills);
}

/// The value watch's line for the message display (`0x5216f0`): `ERR_SKILL_GAINED_S` from a raw 0 (`0x6da5b4`), else `ERR_SKILL_UP_SI` with the read value when positive (`0x6da5c6`-`0x6da5d9`); none for a line with no record, or whose race and class row is missing or carries `0x402` (`0x6da597`-`0x6da5a6`).
fn skill_line(
    world: &World,
    skill_id: u32,
    race: u8,
    class: u8,
    old: u16,
    read: i32,
) -> Option<crate::ui_action::UiError> {
    use crate::ui_action::{FillArg, UiError};
    let lines = world.get_resource::<crate::ui_spellbook::SkillLines>()?;
    let name = lines.catalog.line(skill_id)?.name.clone();
    if !lines.catalog.announces_skill_ups(skill_id, race, class) {
        debug!("chat: skill line {skill_id} silenced ({old} -> {read}, the 0x402 gate)");
        return None;
    }
    match (old, read) {
        (0, _) => Some(UiError::s("ERR_SKILL_GAINED_S", name)),
        (_, v) if v > 0 => Some(UiError::args(
            "ERR_SKILL_UP_SI",
            vec![FillArg::S(name), FillArg::D(i64::from(v))],
        )),
        _ => None,
    }
}

/// One skill slot as a Skills-tab row, or `None` where the reference's list build skips it; the
/// checks run in that build's order.
pub(crate) fn skills_row(
    slot: &benilla_protocol::messages::PlayerSkillSlot,
    catalog: &benilla_formats::SkillLineCatalog,
    race: u8,
    class: u8,
    level: u32,
) -> Option<SkillEntry> {
    if slot.skill_id == 0 {
        return None;
    }
    let id = u32::from(slot.skill_id);
    let line = catalog.line(id)?;
    let rc = catalog.race_class(id, race, class)?;
    // The category row must exist; no category filters, `Not Displayed` (12) included.
    let (category_name, category_order) = catalog.category(line.category_id)?;
    // Flag `0x2` hides `Dual Wield`, the racials, the mount lines and `GENERIC (DND)`.
    if rc.hidden() {
        return None;
    }
    // An untrained line shows only where the flags admit it at this level.
    if slot.value == 0 && !rc.displays_untrained(level) {
        return None;
    }
    Some(SkillEntry {
        skill_id: id,
        name: line.name.clone(),
        value: u32::from(slot.value),
        max: u32::from(slot.max),
        temp_bonus: i32::from(slot.temp_bonus),
        perm_bonus: i32::from(slot.perm_bonus),
        min_level: rc.min_level,
        cost_index: rc.cost_index,
        category_id: line.category_id,
        category_name: category_name.to_string(),
        category_order,
        description: line.description.clone(),
        // The unlearnable bit and a nonzero skill step.
        abandonable: slot.step != 0 && rc.unlearnable(),
        rank_costs: catalog.rank_costs(id, rc.cost_index),
        // The tier opens on flag 8, or 4 before the first step (`0x5ce6f0`).
        tier_cost: if rc.flags & 8 != 0 || (slot.step == 0 && rc.flags & 4 != 0) {
            catalog.tier_cost(rc.tier_id, u32::from(slot.step))
        } else {
            0
        },
        mono: rc.mono(),
    })
}

/// The item entry in an equipped slot; `None` for an empty slot or an item not yet streamed.
fn slot_entry(store: &ObjectStore, objects: &Objects, slot0: u8) -> Option<u32> {
    let guid = store.0.player_inv_slot(slot0)?;
    objects.object(guid)?.object_entry()
}

/// The skill line of the hand at `slot0` (`0x6d6660`): Unarmed when empty, a weapon's subclass line, 0 for anything else.
fn weapon_skill_id(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
    slot0: u8,
) -> u32 {
    // The skill readers ask the weapon getter with the disarm on (`0x6d6671`): a hidden hand is
    // Unarmed.
    if crate::items::disarmed_equipment_slots(store, objects, items, commands) & (1 << slot0) != 0 {
        return SKILL_UNARMED;
    }
    let Some(entry) = slot_entry(store, objects, slot0) else {
        return SKILL_UNARMED;
    };
    let Some(t) = items.template(entry, 0, commands) else {
        return SKILL_UNARMED; // in flight: refined when the template lands
    };
    if t.class != ITEM_CLASS_WEAPON {
        return 0;
    }
    weapon_subclass_skill(t.subclass).unwrap_or(0)
}

/// A skill's `(value + permanent bonus, temporary bonus)`, split as the reference splits it; value
/// 0 skips the permanent add. Signed: a malus can exceed the value.
fn skill_pair(store: &ObjectStore, skill_id: u32) -> (i32, i32) {
    for i in 0..PLAYER_SKILL_SLOTS {
        if let Some(s) = store.0.player_skill(i) {
            if u32::from(s.skill_id) == skill_id {
                let value = i32::from(s.value);
                let base = if value == 0 {
                    0
                } else {
                    value + i32::from(s.perm_bonus)
                };
                return (base, i32::from(s.temp_bonus));
            }
        }
    }
    (0, 0)
}

/// A swing time in ms, with 0 or absent read as not yet streamed: the vanilla 2000, never zero,
/// as the reference divides damage by it.
fn base_swing(streamed: Option<u32>) -> u32 {
    streamed.filter(|&ms| ms != 0).unwrap_or(2000)
}

pub(crate) fn unit_combat_stats(
    store: &ObjectStore,
    tables: Option<&SheetTables>,
) -> UnitCombatStats {
    let f = &store.0;
    let [crit_from_agility, spell_crit_from_intellect, health_regen_from_spirit, mana_regen_from_spirit] =
        tables.map_or([0.0; 4], |t| gt_derived(store, &t.gt));

    let mut stats = [0i32; 5];
    let mut stat_pos = [0i32; 5];
    let mut stat_neg = [0i32; 5];
    for i in 0..5u8 {
        stats[usize::from(i)] = f.unit_stat(i).unwrap_or(0) as i32;
        // The buff-split arrays are integers on the wire, not floats.
        stat_pos[usize::from(i)] = f.player_posstat(i).unwrap_or(0);
        stat_neg[usize::from(i)] = f.player_negstat(i).unwrap_or(0);
    }
    let mut resistances = [0i32; 7];
    let mut resistance_pos = [0i32; 7];
    let mut resistance_neg = [0i32; 7];
    for i in 0..7u8 {
        resistances[usize::from(i)] = f.unit_resistance(i).unwrap_or(0);
        resistance_pos[usize::from(i)] = f.player_resistance_buff_pos(i).unwrap_or(0);
        resistance_neg[usize::from(i)] = f.player_resistance_buff_neg(i).unwrap_or(0);
    }

    let (ap_pos, ap_neg) = f.unit_attack_power_mods();
    let (rap_pos, rap_neg) = f.unit_ranged_attack_power_mods();

    UnitCombatStats {
        stats,
        stat_pos,
        stat_neg,
        resistances,
        resistance_pos,
        resistance_neg,
        min_damage: f.unit_min_damage().unwrap_or(0.0),
        max_damage: f.unit_max_damage().unwrap_or(0.0),
        min_offhand_damage: f.unit_min_offhand_damage().unwrap_or(0.0),
        max_offhand_damage: f.unit_max_offhand_damage().unwrap_or(0.0),
        physical_bonus_pos: f.player_mod_damage_done_pos(0).unwrap_or(0),
        physical_bonus_neg: f.player_mod_damage_done_neg(0).unwrap_or(0),
        damage_percent: f.player_mod_damage_done_pct(0).unwrap_or(1.0),
        // `0` counts as unstreamed: a created store reads an absent in-block field as `Some(0)`,
        // so a plain `unwrap_or` would leave `damage / speed` infinite.
        main_attack_time_ms: base_swing(f.unit_base_attack_time(0)),
        offhand_attack_time_ms: base_swing(f.unit_base_attack_time(1)),
        attack_power: f.unit_attack_power().unwrap_or(0),
        attack_power_pos: i32::from(ap_pos),
        attack_power_neg: i32::from(ap_neg),
        ranged_attack_power: f.unit_ranged_attack_power().unwrap_or(0),
        ranged_attack_power_pos: i32::from(rap_pos),
        ranged_attack_power_neg: i32::from(rap_neg),
        ranged_attack_time_ms: base_swing(f.unit_ranged_attack_time()),
        ranged_min_damage: f.unit_min_ranged_damage().unwrap_or(0.0),
        ranged_max_damage: f.unit_max_ranged_damage().unwrap_or(0.0),
        // The equipment and skill half is `combat_stats`'s; any other unit keeps these defaults.
        has_offhand: false,
        has_wand: false,
        main_weapon_skill: (0, 0),
        offhand_weapon_skill: (0, 0),
        ranged_weapon_skill: (0, 0),
        defense_skill: (0, 0),
        crit_from_agility,
        spell_crit_from_intellect,
        health_regen_from_spirit,
        mana_regen_from_spirit,
        max_health_modifier: f.unit_max_health_modifier().unwrap_or(0.0),
        // The player's sheet half is `combat_stats`'s.
        ..UnitCombatStats::default()
    }
}

/// The player's snapshot: [`unit_combat_stats`] plus the equipment flags, the skill pairs and the
/// 3.3.5a sheet's player fields.
pub(crate) fn combat_stats(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
    tables: Option<&SheetTables>,
    classes: Option<&benilla_formats::ChrClasses>,
) -> UnitCombatStats {
    let f = &store.0;
    let class = (f.unit_bytes_0().unwrap_or(0) >> 8) & 0xff;
    // The off hand counts only as a weapon (a shield does not swing); both flags read false
    // until the template lands.
    let has_offhand = slot_entry(store, objects, SLOT_OFF_HAND)
        .and_then(|e| items.template(e, 0, commands))
        .is_some_and(|t| t.class == ITEM_CLASS_WEAPON);
    let has_wand = slot_entry(store, objects, SLOT_RANGED)
        .and_then(|e| items.template(e, 0, commands))
        .is_some_and(|t| t.class == ITEM_CLASS_WEAPON && t.subclass == SUBCLASS_WAND);

    let main_skill = weapon_skill_id(store, objects, items, commands, SLOT_MAIN_HAND);
    let offhand_skill = weapon_skill_id(store, objects, items, commands, SLOT_OFF_HAND);
    let ranged_skill = weapon_skill_id(store, objects, items, commands, SLOT_RANGED);

    UnitCombatStats {
        has_offhand,
        has_wand,
        main_weapon_skill: skill_pair(store, main_skill),
        // `UnitAttackBothHands`'s second pair.
        offhand_weapon_skill: skill_pair(store, offhand_skill),
        ranged_weapon_skill: skill_pair(store, ranged_skill),
        // `UnitDefense`'s pair.
        defense_skill: skill_pair(store, SKILL_DEFENSE),
        hand_skill_lines: [main_skill, offhand_skill, ranged_skill],
        // `GetDodgeChance`/`GetParryChance`/`GetBlockChance`: player fields, already percentages.
        dodge_percent: store.0.player_dodge_percentage().unwrap_or(0.0),
        parry_percent: store.0.player_parry_percentage().unwrap_or(0.0),
        block_percent: store.0.player_block_percentage().unwrap_or(0.0),
        crit_percent: f.player_crit_percentage().unwrap_or(0.0),
        ranged_crit_percent: f.player_ranged_crit_percentage().unwrap_or(0.0),
        spell_crit_percent: std::array::from_fn(|i| {
            f.player_spell_crit_percentage(i as u8).unwrap_or(0.0)
        }),
        // `GetModDamageDonePos + GetModDamageDoneNeg` (`0x578210`, `0x578250`).
        spell_damage_bonus: std::array::from_fn(|i| {
            f.player_mod_damage_done_pos(i as u8).unwrap_or(0)
                + f.player_mod_damage_done_neg(i as u8).unwrap_or(0)
        }),
        spell_healing_bonus: f.player_mod_healing_done_pos().unwrap_or(0),
        target_resistance_mod: f.player_mod_target_resistance().unwrap_or(0),
        pet_spell_power: f.player_pet_spell_power(),
        shield_block: f.player_shield_block().unwrap_or(0),
        expertise: [
            f.player_expertise(0).unwrap_or(0),
            f.player_expertise(1).unwrap_or(0),
        ],
        combat_ratings: std::array::from_fn(|i| {
            (i < usize::from(PLAYER_COMBAT_RATINGS))
                .then(|| f.player_combat_rating(i as u8))
                .flatten()
                .unwrap_or(0)
        }),
        combat_rating_bonus: tables.map_or([0.0; 25], |t| combat_rating_bonus(store, &t.gt)),
        // `GetManaRegen` answers only a mana user (power type byte 0, `0x612abb`); the base table
        // the unit's power-regen base adds is 0 for mana.
        mana_regen: (f.unit_power_type() == 0).then(|| {
            [
                f.unit_power_regen(0, false).unwrap_or(0.0),
                f.unit_power_regen(0, true).unwrap_or(0.0),
            ]
        }),
        power_regen: {
            let power = f.unit_power_type();
            let base = power_regen_base(power);
            [
                base[0] + f.unit_power_regen(power, false).unwrap_or(0.0),
                base[1] + f.unit_power_regen(power, true).unwrap_or(0.0),
            ]
        },
        damage_bonus_stat: classes.and_then(|c| c.damage_bonus_stat(class)),
        form_agility_attack_power: tables.is_some_and(|t| {
            t.form_flags
                .get(&u32::from(f.unit_shapeshift_form()))
                .is_some_and(|flags| flags & 0x20 != 0)
        }),
        ..unit_combat_stats(store, tables)
    }
}

/// A power type's regen base, out of casting and while casting (`0x4f5300`'s table at
/// `0xb6ba80`): rage and runic power decay 12.5 a second out of combat, focus regains 5 and energy
/// 10; every other type, and a type past the table, 0.
fn power_regen_base(power: u8) -> [f32; 2] {
    match power {
        1 | 6 => [-12.5, 0.0],
        2 => [5.0, 5.0],
        3 => [10.0, 10.0],
        _ => [0.0, 0.0],
    }
}

/// One slot's view from the item object and its ask-once template (no icon while in flight).
/// Keyed by guid and live id: the doll (`PLAYER_FIELD_INV_SLOT_*`, live id = slot + 1) and the
/// bank bags (`PLAYER_FIELD_BANK_BAG_SLOT_*`, 68..=74) read their guids from different arrays.
fn slot_view(
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
    commands: &NetCommands,
    pending: &PendingItemOps,
    names: Option<&crate::names::NameCache>,
    // `ItemSubClass.dbc`, for the count gate below; absent, every bag counts 0.
    sub_classes: Option<&crate::ui_items::ItemSubClasses>,
    // The player's `ChrClasses.dbc` relic flag, which decides `find_equip_slot`'s INVSLOT 17.
    has_relic_slot: bool,
    guid: u64,
) -> Option<InvSlotView> {
    let countdowns = objects.countdowns(guid);
    // The remaining times read at the call.
    let enchant_ms: [Option<u64>; crate::items::ENCHANT_SLOTS] =
        std::array::from_fn(|s| countdowns.and_then(|c| c.enchant_remaining_ms(s as u32)));
    let duration_ms = countdowns.and_then(|c| c.lifetime_remaining_ms());
    let obj = objects.object(guid)?;
    let entry = obj.object_entry()?;
    let count = obj.item_stack_count().unwrap_or(1).max(1);
    // `OBJECT_FIELD_TYPE & TYPEMASK_CONTAINER`, read off the object as `GetInventoryItemCount`'s
    // first fork reads it.
    let is_container = obj.is_container();
    let content_guids: Vec<u64> = {
        let slots = obj.container_num_slots().unwrap_or(0).min(36) as u8;
        (0..slots).filter_map(|s| obj.container_slot(s)).collect()
    };
    let durability = obj
        .item_durability()
        .zip(obj.item_max_durability())
        .filter(|&(_, max)| max > 0);
    // `ITEM_FIELD_FLAGS`, for its wrapped (0x08) and force-red (0x10) bits.
    let flags = obj.item_flags().unwrap_or(0);
    // Soulbound or carrying a binding enchant, read off the raw descriptor, never off the enchant
    // lines below.
    let already_bound = crate::items::already_bound(obj, rolls.enchants);
    let creator = obj
        .item_creator()
        .filter(|&g| g != 0)
        .and_then(|g| names?.resolve(g, commands).map(str::to_string));
    // All twelve enchant slots: our own gear streams as item objects, where an inspected player's
    // descriptor carries only two.
    let enchants = crate::items::enchant_lines(
        (0..crate::items::ENCHANT_SLOTS as u8).map(|s| {
            (
                s,
                obj.item_enchant(s).unwrap_or(0),
                obj.item_enchant_charges(s),
                enchant_ms[usize::from(s)],
            )
        }),
        rolls.enchants,
    );
    // `ITEM_FIELD_RANDOM_PROPERTIES_ID`, the roll behind an "of the Bear" name; its enchants are
    // already in the slots above.
    let roll = obj.item_random_properties_id();
    let t = items.template(entry, guid, commands);
    let (name, quality, display, link, equip_slots, kind, bar_placeable) = match t {
        Some(t) => {
            let name = rolls.name(&t.name, roll);
            (
                Some(name.clone()),
                t.quality as i32,
                t.display_info_id,
                Some(crate::ui_items::item_link_full(
                    entry,
                    &crate::ui_items::object_link_fields(obj, t.flags),
                    &name,
                    t.quality,
                )),
                find_equip_slot(t.inventory_type, has_relic_slot),
                (t.class, t.subclass),
                t.placeable_on_action_bar(),
            )
        }
        None => (None, 0, 0, None, Vec::new(), (0, 0), false),
    };
    // A container counts its contents' stacks when its subclass's `DisplayFlags` has 0x4, else 0
    // (`GetInventoryItemCount`); the bit is on Soul Bag (1/1) and the Quiver class (11/0..3) only,
    // so a plain bag shows no count.
    let counts_contents = sub_classes.is_some_and(|c| c.0.display_flags(kind.0, kind.1) & 0x4 != 0);
    let contents_count = is_container.then(|| {
        if counts_contents {
            content_guids
                .iter()
                .filter_map(|&g| objects.object(g))
                .map(|o| o.item_stack_count().unwrap_or(1).max(1))
                .sum()
        } else {
            0
        }
    });
    let icon = crate::ui_items::item_icon(icons, display);
    Some(InvSlotView {
        item_id: entry,
        guid,
        gems: crate::items::gems(obj, rolls.enchants),
        icon,
        count,
        contents_count,
        quality,
        name,
        link,
        durability,
        flags,
        already_bound,
        locked: pending.is_locked(guid),
        equip_slots,
        bar_placeable,
        creator,
        enchants,
        duration_ms,
    })
}

/// Carried ammo matching `ammo_id`, in the backpack and the four equipped bags; bank and buyback
/// never count.
fn ammo_count(store: &ObjectStore, objects: &Objects, ammo_id: u32) -> u32 {
    let mut n = 0;
    let mut add = |guid: Option<u64>| {
        if let Some(o) = guid.and_then(|g| objects.object(g)) {
            if o.object_entry() == Some(ammo_id) {
                n += o.item_stack_count().unwrap_or(1);
            }
        }
    };
    for i in 0..16u8 {
        add(store.0.player_pack_slot(i));
    }
    for b in 19..23u8 {
        if let Some(bag) = store.0.player_inv_slot(b).and_then(|g| objects.object(g)) {
            let slots = bag.container_num_slots().unwrap_or(0).min(36) as u8;
            for s in 0..slots {
                if let Some(o) = bag.container_slot(s).and_then(|g| objects.object(g)) {
                    if o.object_entry() == Some(ammo_id) {
                        n += o.item_stack_count().unwrap_or(1);
                    }
                }
            }
        }
    }
    n
}

/// The reads a slot's view takes beside the player's store and the held objects.
#[derive(Clone, Copy)]
pub(crate) struct SlotReads<'a> {
    pub(crate) items: &'a Items,
    pub(crate) icons: Option<&'a ItemDisplays>,
    pub(crate) rolls: crate::items::RollCatalogs<'a>,
    pub(crate) commands: &'a NetCommands,
    pub(crate) pending: &'a PendingItemOps,
    pub(crate) names: Option<&'a crate::names::NameCache>,
    pub(crate) sub_classes: Option<&'a crate::ui_items::ItemSubClasses>,
    pub(crate) classes: Option<&'a benilla_formats::ChrClasses>,
}

/// Doll id `live` (`GetInventorySlotInfo`'s: 0 ammo, 1..=19 the equipment, 20..=23 the equipped
/// bags, inv slots 19..22) read off the player's slot field and its item at the call (`0x5e7c00`).
fn inventory_slot(
    store: &ObjectStore,
    objects: &Objects,
    r: SlotReads<'_>,
    live: usize,
) -> Option<InvSlotView> {
    if live == 0 {
        let ammo_id = store.0.player_ammo_id().unwrap_or(0);
        if ammo_id == 0 {
            return None;
        }
        let t = r.items.template(ammo_id, 0, r.commands);
        let (name, quality, display, link) = match t {
            Some(t) => (
                Some(t.name.clone()),
                t.quality as i32,
                t.display_info_id,
                Some(item_link(ammo_id, &t.name, t.quality)),
            ),
            None => (None, 0, 0, None),
        };
        let icon = t.and(crate::ui_items::item_icon(r.icons, display));
        return Some(InvSlotView {
            guid: 0,
            gems: [0; 3],
            durability: None,
            flags: 0,
            // Ammo has no item object, only its template, so there is no bind to read.
            already_bound: false,
            item_id: ammo_id,
            icon,
            count: ammo_count(store, objects, ammo_id),
            // `GetInventoryItemCount`'s `PLAYER_AMMO_ID` leg, before the container fork.
            contents_count: None,
            quality,
            name,
            link,
            // No click or drag path reaches the ammo slot: nothing locks it, it fits nowhere.
            locked: false,
            equip_slots: Vec::new(),
            bar_placeable: false,
            creator: None,
            // No ammo instance streams, only its template id: no enchants, no countdown.
            enchants: Vec::new(),
            duration_ms: None,
        });
    }
    if live > 23 {
        return None;
    }
    let guid = store
        .0
        .player_inv_slot(live as u8 - 1)
        .filter(|g| *g != 0)?;
    let has_relic_slot = store
        .0
        .unit_class()
        .is_some_and(|c| r.classes.is_some_and(|t| t.has_relic_slot(u32::from(c))));
    slot_view(
        objects,
        r.items,
        r.icons,
        r.rolls,
        r.commands,
        r.pending,
        r.names,
        r.sub_classes,
        has_relic_slot,
        guid,
    )
}

/// Bank bag slot `i` (live ids 68..=74) read at the call: a player slot streamed whether or not a
/// banker is open, which the stock bank reads through `BankButtonIDToInvSlotID(i, 1)`
/// (`BankFrame.lua:28-35, 199-204`).
fn bank_bag_slot(
    store: &ObjectStore,
    objects: &Objects,
    r: SlotReads<'_>,
    i: usize,
) -> Option<InvSlotView> {
    let guid = store
        .0
        .player_bank_bag_slot(u8::try_from(i).ok()?)
        .filter(|g| *g != 0)?;
    slot_view(
        objects,
        r.items,
        r.icons,
        r.rolls,
        r.commands,
        r.pending,
        r.names,
        r.sub_classes,
        // A bank bag slot holds a bag; the relic flag decides INVSLOT 17 alone.
        false,
        guid,
    )
}

/// Fire the unit-field stat events for one unit's snapshot transition, `arg1 = token`, each when
/// a unit field behind it moved: the unit watch's events by field (`UNIT_STATS` 78, `UNIT_DAMAGE`
/// 64, `UNIT_ATTACK_SPEED` 56, `UNIT_RESISTANCES` 93, `UNIT_ATTACK_POWER` 117,
/// `UNIT_RANGED_ATTACK_POWER` 120, `UNIT_RANGEDDAMAGE` 58, each its field's index less 6). A first
/// snapshot fires them all. The player's own watches fire the rest from the values handler.
pub(crate) fn fire_stat_transitions(
    script: &mut UiScript,
    token: &str,
    prev: Option<&UnitCombatStats>,
    stats: &UnitCombatStats,
) {
    let tok = || ScriptValue::Str(token.to_string());
    let changed = |sel: fn(&UnitCombatStats) -> Vec<i64>| prev.is_none_or(|p| sel(p) != sel(stats));
    if changed(|s| {
        let mut v: Vec<i64> = s.stats.iter().map(|&x| i64::from(x)).collect();
        v.extend(
            s.stat_pos
                .iter()
                .chain(s.stat_neg.iter())
                .map(|&x| i64::from(x)),
        );
        v
    }) {
        script.fire_event("UNIT_STATS", vec![tok()]);
    }
    if changed(|s| {
        s.resistances
            .iter()
            .chain(s.resistance_pos.iter())
            .chain(s.resistance_neg.iter())
            .map(|&x| i64::from(x))
            .collect()
    }) {
        script.fire_event("UNIT_RESISTANCES", vec![tok()]);
    }
    if changed(|s| {
        vec![
            fixed(s.min_damage),
            fixed(s.max_damage),
            fixed(s.min_offhand_damage),
            fixed(s.max_offhand_damage),
        ]
    }) {
        script.fire_event("UNIT_DAMAGE", vec![tok()]);
    }
    if changed(|s| {
        vec![
            i64::from(s.main_attack_time_ms),
            i64::from(s.offhand_attack_time_ms),
        ]
    }) {
        script.fire_event("UNIT_ATTACK_SPEED", vec![tok()]);
    }
    if changed(|s| {
        vec![
            i64::from(s.attack_power),
            i64::from(s.attack_power_pos),
            i64::from(s.attack_power_neg),
        ]
    }) {
        script.fire_event("UNIT_ATTACK_POWER", vec![tok()]);
    }
    if changed(|s| {
        vec![
            i64::from(s.ranged_attack_power),
            i64::from(s.ranged_attack_power_pos),
            i64::from(s.ranged_attack_power_neg),
        ]
    }) {
        script.fire_event("UNIT_RANGED_ATTACK_POWER", vec![tok()]);
    }
    if changed(|s| {
        vec![
            fixed(s.ranged_min_damage),
            fixed(s.ranged_max_damage),
            i64::from(s.ranged_attack_time_ms),
        ]
    }) {
        script.fire_event("UNIT_RANGEDDAMAGE", vec![tok()]);
    }
}

/// The 0-based weapon slots `GetWeaponEnchantInfo` reads; the frames' live ids are one higher.
const EQUIPMENT_SLOT_MAINHAND: u8 = 15;
const EQUIPMENT_SLOT_OFFHAND: u8 = 16;

/// The temporary enchantment slot (0 permanent, 7..11 random properties), as the reference's
/// remaining-time read passes it.
const TEMP_ENCHANTMENT_SLOT: u8 = 1;

/// One weapon's temporary enchantment, read off the raw descriptor rather than
/// [`InvSlotView::enchants`], which drops unnamed ids and the totem imbues; the reference's only
/// gate is a nonzero id (`[descriptor+0x4c]`).
fn weapon_enchant(
    store: &ObjectStore,
    objects: &Objects,
    slot0: u8,
) -> Option<benilla_ui::script::WeaponEnchant> {
    let guid = store.0.player_inv_slot(slot0)?;
    // The deadline read, not the tooltip's: an elapsed timer answers 0, not "no timer".
    let remaining_ms = objects
        .countdowns(guid)
        .and_then(|c| c.enchant_deadline_ms(u32::from(TEMP_ENCHANTMENT_SLOT)));
    let obj = objects.object(guid)?;
    obj.item_enchant(TEMP_ENCHANTMENT_SLOT)?;
    Some(benilla_ui::script::WeaponEnchant {
        remaining_ms,
        charges: obj.item_enchant_charges(TEMP_ENCHANTMENT_SLOT),
    })
}

/// What the sheet verbs read at the call beside the live view's own tables: the held objects the worn weapons resolve through, the gt tables, and the catalogs a slot's view reads.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CharLive<'w, 's> {
    objects: Objects<'w, 's>,
    tables: Option<Res<'w, SheetTables>>,
    icons: Option<Res<'w, ItemDisplays>>,
    props: Option<Res<'w, crate::items::RandomProperties>>,
    names: Option<Res<'w, crate::names::NameCache>>,
    sub_classes: Option<Res<'w, crate::ui_items::ItemSubClasses>>,
}

impl CharLive<'_, '_> {
    /// The player's sheet figures off `store` at the call (`0x610300`, `0x60ddb0`).
    pub(crate) fn combat_stats(
        &self,
        store: &ObjectStore,
        items: &Items,
        commands: &NetCommands,
        classes: Option<&benilla_formats::ChrClasses>,
    ) -> UnitCombatStats {
        combat_stats(
            store,
            &self.objects,
            items,
            commands,
            self.tables.as_deref(),
            classes,
        )
    }

    /// The reads a slot's view takes, this side's catalogs beside the live view's.
    fn slot_reads<'a>(
        &'a self,
        items: &'a Items,
        enchants: Option<&'a crate::items::Enchants>,
        commands: &'a NetCommands,
        pending: &'a PendingItemOps,
        classes: Option<&'a benilla_formats::ChrClasses>,
    ) -> SlotReads<'a> {
        SlotReads {
            items,
            icons: self.icons.as_deref(),
            rolls: crate::items::RollCatalogs {
                enchants,
                props: self.props.as_deref(),
            },
            commands,
            pending,
            names: self.names.as_deref(),
            sub_classes: self.sub_classes.as_deref(),
            classes,
        }
    }

    /// Doll id `live` of the player off `store` at the call (`0x5e7c00`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn inventory_slot(
        &self,
        store: &ObjectStore,
        items: &Items,
        enchants: Option<&crate::items::Enchants>,
        commands: &NetCommands,
        pending: &PendingItemOps,
        classes: Option<&benilla_formats::ChrClasses>,
        live: usize,
    ) -> Option<InvSlotView> {
        let r = self.slot_reads(items, enchants, commands, pending, classes);
        inventory_slot(store, &self.objects, r, live)
    }

    /// Bank bag slot `i` of the player off `store` at the call (`0x5e7c00`).
    pub(crate) fn bank_bag_slot(
        &self,
        store: &ObjectStore,
        items: &Items,
        enchants: Option<&crate::items::Enchants>,
        commands: &NetCommands,
        pending: &PendingItemOps,
        i: usize,
    ) -> Option<InvSlotView> {
        let r = self.slot_reads(items, enchants, commands, pending, None);
        bank_bag_slot(store, &self.objects, r, i)
    }

    /// Both weapons' temporary enchantment off `store` at the call, main hand then off hand (`0x5eaca0`).
    pub(crate) fn weapon_enchants(
        &self,
        store: &ObjectStore,
    ) -> [Option<benilla_ui::script::WeaponEnchant>; 2] {
        [
            weapon_enchant(store, &self.objects, EQUIPMENT_SLOT_MAINHAND),
            weapon_enchant(store, &self.objects, EQUIPMENT_SLOT_OFFHAND),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::ObjectFields;
    use bevy::ecs::system::RunSystemOnce;

    /// The trade link's ranks (`0x5db120`-`0x5db134`): each half adds the permanent bonus when positive, and a line the player lacks reads slot -1 (`0x6dc222`), the words before the array.
    #[test]
    fn the_trade_link_reads_the_slot_at_the_call_and_slot_minus_one_for_a_missing_line() {
        use benilla_protocol::field::{
            FIELD_PLAYER_CHARACTER_POINTS2, FIELD_PLAYER_KNOWN_CURRENCIES,
            FIELD_PLAYER_NEXT_LEVEL_XP, FIELD_PLAYER_XP,
        };
        const F_SKILL: u16 = benilla_protocol::field::FIELD_PLAYER_SKILL_INFO_1_1;
        let pair = |lo: u16, hi: u16| u32::from(lo) | (u32::from(hi) << 16);
        // Slot 0: Tailoring (197) 150/225, +5 permanent; slot 1: Mining (186) 0/75, +7 permanent.
        let mut pairs = vec![
            (F_SKILL, pair(197, 2)),
            (F_SKILL + 1, pair(150, 225)),
            (F_SKILL + 2, pair(0, 5)),
            (F_SKILL + 3, pair(186, 0)),
            (F_SKILL + 4, pair(0, 75)),
            (F_SKILL + 5, pair(0, 7)),
            // Slot 128's words, which the miss never reads.
            (FIELD_PLAYER_CHARACTER_POINTS2, 2),
            // Slot -1: value|max on PLAYER_XP.
            (FIELD_PLAYER_XP, pair(1234, 3)),
        ];
        let fields = ObjectFields::from_pairs(&pairs);
        assert_eq!(link_ranks(&fields, 197), (155, 230));
        assert_eq!(link_ranks(&fields, 186), (0, 82));
        assert_eq!(link_ranks(&fields, 171), (1234, 3));
        // PLAYER_NEXT_LEVEL_XP's high half is slot -1's permanent bonus.
        pairs.push((FIELD_PLAYER_NEXT_LEVEL_XP, pair(0, 4)));
        let fields = ObjectFields::from_pairs(&pairs);
        assert_eq!(link_ranks(&fields, 171), (1238, 7));
        assert_eq!(F_SKILL - 3 + 1, FIELD_PLAYER_XP);
        assert_eq!(F_SKILL - 3 + 2, FIELD_PLAYER_NEXT_LEVEL_XP);
        assert_eq!(F_SKILL - 3, FIELD_PLAYER_KNOWN_CURRENCIES + 1);
    }

    /// `GetSkillLineInfo`'s slot (`0x5cdfe7`) for a line the player lacks: slot -1 unchecked (`0x6dc222`), the step off `PLAYER_FIELD_KNOWN_CURRENCIES`' high half (`0x594794`), the bonuses off `PLAYER_NEXT_LEVEL_XP`.
    #[test]
    fn a_missing_line_reads_slot_minus_one_at_the_call() {
        use benilla_protocol::field::{
            FIELD_PLAYER_KNOWN_CURRENCIES, FIELD_PLAYER_NEXT_LEVEL_XP, FIELD_PLAYER_XP,
        };
        let pair = |lo: u16, hi: u16| u32::from(lo) | (u32::from(hi) << 16);
        let fields = ObjectFields::from_pairs(&[
            (FIELD_PLAYER_KNOWN_CURRENCIES + 1, pair(0, 1)),
            (FIELD_PLAYER_XP, pair(500, 9)),
            (FIELD_PLAYER_NEXT_LEVEL_XP, pair(0xfffe, 6)),
        ]);
        let slot = found_skill_slot(&fields, 171);
        assert_eq!(
            (
                slot.step,
                slot.value,
                slot.max,
                slot.temp_bonus,
                slot.perm_bonus
            ),
            (1, 500, 9, -2, 6)
        );
        let live = skill_slot(&fields, None, 171);
        assert_eq!(
            (live.value, live.max, live.temp_bonus, live.perm_bonus),
            (500, 9, -2, 6)
        );
    }

    /// The reference's split; a value-0 line skips the permanent add.
    #[test]
    fn a_skill_pair_folds_the_permanent_bonus_into_the_base() {
        /// `PLAYER_SKILL_INFO_1_1`: id|step, value|max, temp|perm per slot.
        const F_SKILL: u16 = benilla_protocol::field::FIELD_PLAYER_SKILL_INFO_1_1;
        let pair = |lo: u16, hi: u16| u32::from(lo) | (u32::from(hi) << 16);
        // Slot 0: Defense (95) at 300, +10 temporary, +5 permanent.
        // Slot 1: Swords (43) at 0, with a +7 permanent that must not surface.
        let store = ObjectStore(ObjectFields::from_pairs(&[
            (F_SKILL, pair(95, 0)),
            (F_SKILL + 1, pair(300, 300)),
            (F_SKILL + 2, pair(10, 5)),
            (F_SKILL + 3, pair(43, 0)),
            (F_SKILL + 4, pair(0, 300)),
            (F_SKILL + 5, pair(0, 7)),
        ]));
        assert_eq!(
            skill_pair(&store, 95),
            (305, 10),
            "the permanent bonus belongs to the base and the temporary one is the modifier"
        );
        assert_eq!(
            skill_pair(&store, 43),
            (0, 0),
            "a line at value 0 skips the perm add — a bare talent bonus is not a skill"
        );
        assert_eq!(
            skill_pair(&store, 162),
            (0, 0),
            "a line the player does not have at all"
        );
        // A malus deeper than the skill is representable, which is why the pair is signed.
        let cursed = ObjectStore(ObjectFields::from_pairs(&[
            (F_SKILL, pair(95, 0)),
            (F_SKILL + 1, pair(5, 300)),
            (F_SKILL + 2, pair(0, u16::MAX - 9)), // perm = -10
        ]));
        assert_eq!(skill_pair(&cursed, 95), (-5, 0));
    }

    /// A hand's line: Unarmed when empty, a weapon's subclass line, 0 for a shield or a relic.
    #[test]
    fn a_hands_skill_line_follows_the_item_in_it() {
        /// `PLAYER_FIELD_INV_SLOT_HEAD`, one guid pair per slot.
        const F_INV_SLOT_HEAD: u16 = benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        const SWORD: u64 = 0x4000_0000_0000_0001;
        const SHIELD: u64 = 0x4000_0000_0000_0002;
        let mut app = App::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx));
        let inv = |slot: u8| F_INV_SLOT_HEAD + 2 * u16::from(slot);
        let world = app.world_mut();
        world.spawn((
            SelfPlayer,
            ObjectStore(ObjectFields::from_pairs(&[
                (inv(SLOT_MAIN_HAND), SWORD as u32),
                (inv(SLOT_MAIN_HAND) + 1, (SWORD >> 32) as u32),
                (inv(SLOT_OFF_HAND), SHIELD as u32),
                (inv(SLOT_OFF_HAND) + 1, (SHIELD >> 32) as u32),
                (inv(SLOT_RANGED), 0),
                (inv(SLOT_RANGED) + 1, 0),
            ])),
        ));
        for (guid, entry, class, subclass) in [(SWORD, 1, ITEM_CLASS_WEAPON, 7), (SHIELD, 2, 4, 6)]
        {
            let fields = ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, entry)]);
            crate::items::test_spawn_item(world, guid, fields, false);
            let template = benilla_protocol::ItemInfo {
                class,
                subclass,
                ..crate::items::test_template("Hand item")
            };
            world
                .resource_mut::<Items>()
                .insert_template(entry, Some(template));
        }
        let lines = world
            .run_system_once(
                |me: Query<&ObjectStore, With<SelfPlayer>>,
                 objects: Objects,
                 items: Res<Items>,
                 commands: Res<NetCommands>| {
                    let store = me.single().unwrap();
                    [SLOT_MAIN_HAND, SLOT_OFF_HAND, SLOT_RANGED]
                        .map(|slot| weapon_skill_id(store, &objects, &items, &commands, slot))
                },
            )
            .unwrap();
        assert_eq!(lines, [43, 0, SKILL_UNARMED]);
    }

    /// `PLAYER_FIELD_BANK_BAG_SLOT_1`, bank bag slot 0's guid pair.
    const F_BANK_BAG_1: u16 = benilla_protocol::field::FIELD_PLAYER_BANK_BAG_SLOT_1;
    /// `OBJECT_FIELD_ENTRY` on the item object.
    const F_OBJECT_ENTRY: u16 = benilla_protocol::field::FIELD_OBJECT_ENTRY;
    /// Any container entry ("Traveler's Backpack"); the feed only needs it to resolve.
    const BAG_ENTRY: u32 = 4500;
    const BAG: u64 = 0x4000_0000_0000_0abc;

    /// Our player's guid in the test world.
    const ME: u64 = 0x1;

    /// A world with `bag`, or nothing, in the first bank bag slot of our held player, and a frame recording events.
    fn world_with_bank_bag(bag: Option<u64>) -> App {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<PaperDollBooth>()
            .init_resource::<PendingItemOps>()
            .init_resource::<Items>()
            .init_resource::<crate::names::NameCache>()
            .insert_resource(crate::net::SelfGuid(Some(ME)))
            .insert_resource(NetCommands(tx));
        // `OBJECT_FIELD_TYPE` with the player bit, as the live view's player lookup reads it.
        let mut fields = match bag {
            Some(g) => ObjectFields::from_pairs(&[
                (2, 0x19),
                (F_BANK_BAG_1, g as u32),
                (F_BANK_BAG_1 + 1, (g >> 32) as u32),
            ]),
            None => {
                ObjectFields::from_pairs(&[(2, 0x19), (F_BANK_BAG_1, 0), (F_BANK_BAG_1 + 1, 0)])
            }
        };
        fields = fields.into_created(benilla_protocol::messages::ObjectType::Player);
        let me = app
            .world_mut()
            .spawn((SelfPlayer, crate::net::Guid(ME), ObjectStore(fields)))
            .id();
        let mut index = crate::net::GuidIndex::default();
        index.0.insert(ME, me);
        app.insert_resource(index);
        if let Some(g) = bag {
            // The bag is an item entity; its template lives on `Items`.
            crate::items::test_spawn_item(
                app.world_mut(),
                g,
                ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, BAG_ENTRY)]),
                true,
            );
            app.world_mut().resource_mut::<Items>().insert_template(
                BAG_ENTRY,
                Some(crate::items::test_template("Traveler's Backpack")),
            );
        }
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("PLAYERBANKSLOTS_CHANGED")
                f:RegisterEvent("UNIT_INVENTORY_CHANGED")
                f:SetScript("OnEvent", function(self, event, arg1)
                    table.insert(SEEN, event .. " " .. tostring(arg1))
                end)
            "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app
    }

    fn seen(app: &mut App) -> Vec<String> {
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        let out = script.eval::<Vec<String>>("return SEEN").unwrap();
        script.run("SEEN = {}").unwrap();
        out
    }

    /// `IsInventoryItemLocked` with the `World` lent, read at the call.
    fn locked_at_call(app: &mut App, slot: u32) -> bool {
        crate::ui_script::live::with_live_world(app.world_mut(), |script| {
            script
                .eval::<bool>(&format!(
                    "return IsInventoryItemLocked({slot}) and true or false"
                ))
                .unwrap()
        })
        .unwrap()
    }

    /// The slot verbs read the lock at the call (`0x5e7c00`): a lock and its clear show at once, and neither fires an inventory event.
    #[test]
    fn a_bank_bag_lock_reads_at_the_call_and_fires_nothing() {
        let mut app = world_with_bank_bag(Some(BAG));
        assert!(!locked_at_call(&mut app, 68), "nothing in flight");
        app.world_mut().resource_mut::<PendingItemOps>().lock(BAG);
        assert!(
            locked_at_call(&mut app, 68),
            "in flight, the slot reads locked"
        );
        // A refusal: the lock clears without any object moving.
        assert!(app.world_mut().resource_mut::<PendingItemOps>().unlock(BAG));
        assert!(
            !locked_at_call(&mut app, 68),
            "the unlock reads at the next call, no frame between"
        );
        let events = seen(&mut app);
        assert!(
            events.is_empty(),
            "a lock flip is `ITEM_LOCK_CHANGED`'s, not a slot change, got {events:?}"
        );
    }

    /// An empty bank bag slot reads empty at the call.
    #[test]
    fn an_empty_bank_bag_slot_reads_empty() {
        let mut app = world_with_bank_bag(None);
        let link = crate::ui_script::live::with_live_world(app.world_mut(), |script| {
            script
                .eval::<Option<String>>("return GetInventoryItemLink('player', 68)")
                .unwrap()
        })
        .unwrap();
        assert_eq!(link, None);
    }
}
