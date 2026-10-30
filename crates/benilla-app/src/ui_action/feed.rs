//! The action bar's identity feed: each of the action slots' icon and count, pushed to the VM with
//! `ACTIONBAR_SLOT_CHANGED` per changed slot. [`feed_actions`] also drains the UI error queues and
//! feeds the stance page (`GetBonusBarOffset`: the form's `SpellShapeshiftForm.dbc` BonusActionBar
//! column).

use crate::ui_items::{count_of, InventoryScope};
use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use benilla_protocol::messages::{
    ACTION_KIND_EQUIPMENT_SET, ACTION_KIND_ITEM, ACTION_KIND_MACRO, ACTION_KIND_SPELL,
};
use benilla_ui::script::{ActionSlot, ScriptValue, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, SelfPlayer};

use super::errors::{first_missing_totem, first_short_reagent, mount_result_key};
use super::weapon_icon::{auto_attack_icon, substitutes_weapon_icon};
use super::{
    cast_fail, show_messages, ui_error_text, CastErrors, MessageSink, MountErrors, PetTameFailures,
    PlayerActions, Shown, Spells, UiError, UiErrorKeys, UiErrorTexts,
};

/// An item action's icon when its template has not answered or its display id has no
/// `ItemDisplayInfo` row: the literal the reference's icon resolver falls back to.
pub(super) const MISSING_ITEM_ICON: &str = "Interface\\Icons\\INV_Misc_QuestionMark";

/// The feed's memory of what it last pushed, for per-slot change events.
#[derive(Default)]
pub(super) struct FeedMemory {
    pushed: HashMap<u32, ActionSlot>,
    bonus_offset: u8,
    /// Whether the identity resolve has run against this VM: a new VM resets this memory, while
    /// every other input to the gate is host-side and may still match.
    resolved: bool,
    /// The [`Items::template_epoch`] of the last resolve; an advance re-resolves.
    template_epoch: u64,
    /// The macro-table generation of the last resolve: a macro edit changes a bar icon while
    /// touching neither the action table nor any item template.
    macro_generation: u64,
}

/// The DBC name tables the cast-fail argument arms read, as one parameter to stay within Bevy's
/// parameter limit; each is absent without game data. The shapeshift forms ride [`Spells`].
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct FailNameTables<'w> {
    /// `SpellFocusObject.dbc`, for `0x5e`.
    focus: Option<Res<'w, crate::ui_tradeskill::SpellFocus>>,
    /// `AreaTable.dbc`, for `0x5d`.
    areas: Option<Res<'w, crate::area::AreaTableRes>>,
    /// `SpellMechanic.dbc`, for `0x8d`.
    mechanics: Option<Res<'w, super::SpellMechanics>>,
    /// `SkillLine.dbc`, for `0x90`.
    skill_lines: Option<Res<'w, crate::ui_spellbook::SkillLines>>,
    /// `TotemCategory.dbc`, for `0x9c`.
    totems: Option<Res<'w, crate::ui_totem::TotemCategories>>,
}

impl FailNameTables<'_> {
    /// A `SkillLine.dbc` row's display name.
    fn skill_name(&self, line_id: u32) -> Option<&str> {
        let lines = self.skill_lines.as_deref()?;
        lines.catalog.line(line_id).map(|l| l.name.as_str())
    }

    fn args<'a>(&'a self, spells: Option<&'a Spells>) -> cast_fail::FailArgs<'a> {
        cast_fail::FailArgs {
            arg: None,
            focus: self.focus.as_deref().map(|f| &f.catalog),
            areas: self.areas.as_deref().map(|a| &a.0),
            mechanics: self.mechanics.as_deref().map(|m| &m.catalog),
            forms: spells.map(|s| &s.forms),
            totems: self.totems.as_deref().map(|t| &t.0),
        }
    }
}

#[allow(clippy::type_complexity)] // a Bevy system's full input set
pub(super) fn feed_actions(
    script: Option<NonSendMut<UiScript>>,
    mut actions: ResMut<PlayerActions>,
    mut cast_errors: ResMut<CastErrors>,
    mut mount_errors: ResMut<MountErrors>,
    mut pet_tame_failures: ResMut<PetTameFailures>,
    // The by-key and resolved-text queues and the combat log's failure feed as one parameter, at
    // Bevy's 16-parameter limit.
    ui_errors: (
        ResMut<UiErrorKeys>,
        ResMut<UiErrorTexts>,
        Option<ResMut<Messages<crate::combat_log::SpellCastFailedLog>>>,
    ),
    spells: Option<Res<Spells>>,
    // With the aura table, the `ActiveIconID` toggle's input.
    self_q: Query<(&ObjectStore, Option<&crate::unit_auras::UnitAuras>), With<SelfPlayer>>,
    objects: crate::net::Objects,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    // With the temporary bar's owners and the pet bar its pet-action slots show.
    bars: (
        Option<Res<crate::ui_items::ItemSubClasses>>,
        Option<Res<super::TempActionBar>>,
        Option<Res<crate::ui_pet::PetBar>>,
    ),
    name_tables: FailNameTables,
    commands: Res<NetCommands>,
    mut memory: Local<crate::ui_script::VmMemo<FeedMemory>>,
    mut sink: MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    let (mut ui_error_keys, mut ui_error_texts, mut cast_fail_log) = ui_errors;
    let (sub_classes, temp_bar, pet_bar) = bars;
    let temp_flags = temp_bar.map_or(0, |t| t.flags);
    let memory = memory.get(&script);

    // Cast failures become error lines through [`cast_fail`], all resolved before any event fires.
    // The drain owns the arms that need our bags or the item cache: `0x78` and `0x5c` name the
    // failing slot's item, found as the pre-send check finds it. On a cache miss the entry stays
    // queued, as the reference's callback redisplays on the answer.
    let self_store = self_q.iter().next().map(|(s, _)| s);
    let mut await_template: Vec<crate::ui_action::CastFail> = Vec::new();
    // The same failures for the combat log: `0x808200` calls both the error display and the combat
    // log's spell-failure line (`0x751ad0`).
    let mut fail_lines: Vec<crate::combat_log::SpellCastFailedLog> = Vec::new();
    let fail_args = name_tables.args(spells.as_deref());
    let texts: Vec<cast_fail::CastFailLine> = cast_errors
        .0
        .drain(..)
        .filter_map(|fail| {
            let crate::ui_action::CastFail {
                spell_id,
                reason,
                caster,
                ..
            } = fail;
            let pet = caster == crate::ui_action::Caster::Pet;
            let d = spells.as_ref().and_then(|s| s.catalog.get(spell_id));
            let get = |key: &str| script.lua().globals().get::<String>(key).ok();
            // The displayed line first and the log line from it, the reference's order.
            let line = (|| -> Option<cast_fail::CastFailLine> {
                // `0x19`-`0x1b`: the singular subclass name, "Must have a Wand equipped"; a
                // multi-bit mask names its `ItemSubClassMask.dbc` group, else the first subclass,
                // where the tooltip uses the plural and joins. The pet's table fills it too.
                if let (0x19..=0x1b, Some(d), Some(subs)) = (reason, d, sub_classes.as_deref()) {
                    if let Some(name) = (d.equipped_item_class >= 0)
                        .then(|| {
                            subs.0.requirement_display_name(
                                d.equipped_item_class as u32,
                                d.equipped_item_subclass_mask,
                            )
                        })
                        .flatten()
                    {
                        let key = cast_fail::CAST_FAIL_KEYS[reason as usize];
                        return get(key)
                            .filter(|s| !s.is_empty())
                            .map(|t| cast_fail::CastFailLine::passthrough(t.replace("%s", &name)));
                    }
                }
                // `0x31`: the same helper for class 6 (Projectile), mask `1 << arg`, the shift
                // masked to five bits as x86 `shl` masks it. The player's arm only: the pet's table
                // has no `0x31` entry. vmangos never sends the word (`Spell.cpp:4419-4443`), so
                // against it this declines to the stem.
                if let (false, 0x31, Some(arg), Some(subs)) =
                    (pet, reason, fail.arg, sub_classes.as_deref())
                {
                    const ITEM_CLASS_PROJECTILE: u32 = 6;
                    if let Some(name) = subs
                        .0
                        .requirement_display_name(ITEM_CLASS_PROJECTILE, 1u32 << (arg & 31))
                    {
                        let key = cast_fail::CAST_FAIL_KEYS[reason as usize];
                        return get(key)
                            .filter(|s| !s.is_empty())
                            .map(|t| cast_fail::CastFailLine::passthrough(t.replace("%s", &name)));
                    }
                }
                // `0x78` and `0x5c`, the player's arms only: the pet's table sends both to its
                // generic arm.
                if !pet && (reason == 0x78 || reason == 0x5c) {
                    let d = d?;
                    let failing = if reason == 0x78 {
                        self_store
                            .and_then(|s| first_missing_totem(d, s, &objects))
                            // No self store yet: name the first tool.
                            .or_else(|| d.totems.iter().copied().find(|&t| t != 0))
                    } else {
                        self_store
                            .and_then(|s| first_short_reagent(d, s, &objects))
                            .or_else(|| d.reagents.iter().map(|&(id, _)| id).find(|&id| id != 0))
                    }?;
                    let cached = items
                        .template(failing, 0, &commands)
                        .map(|i| i.name.clone());
                    let name = match cached {
                        Some(name) => name,
                        // Answered unknown: the reference's callback literal.
                        None if items.template_answered_unknown(failing) => "UNKNOWN".to_string(),
                        None => {
                            // Pending: requeued as a redisplay, which is not logged.
                            await_template.push(fail.requeued());
                            return None;
                        }
                    };
                    let key = if reason == 0x78 {
                        "SPELL_FAILED_TOTEMS"
                    } else {
                        "SPELL_FAILED_REAGENTS"
                    };
                    return get(key)
                        .filter(|s| !s.is_empty())
                        .map(|t| cast_fail::CastFailLine::passthrough(t.replace("%s", &name)));
                }
                // `0x97` NEED_MORE_ITEMS and `0x90` MIN_SKILL (12340's 55 and 150, `0x808200`),
                // the player's arms only: the first word names the item or skill line, the second
                // the count or the value, "Requires %d %s." and "Requires %s (%d).". A pending
                // item waits as `0x5c` does; an unknown one prints 0 "UNKNOWN" (`0x7feb50`).
                if !pet && (reason == 0x97 || reason == 0x90) {
                    let key = cast_fail::CAST_FAIL_KEYS[reason as usize];
                    let second = fail.arg2.map_or(-1, |n| n as i32);
                    let (count, name) = if reason == 0x90 {
                        let name = fail.arg.and_then(|id| name_tables.skill_name(id));
                        (second, name.unwrap_or("UNKNOWN").to_string())
                    } else {
                        let cached = fail.arg.and_then(|id| {
                            items.template(id, 0, &commands).map(|i| i.name.clone())
                        });
                        match (cached, fail.arg) {
                            (Some(name), _) => (second, name),
                            (None, Some(id)) if !items.template_answered_unknown(id) => {
                                await_template.push(fail.requeued());
                                return None;
                            }
                            (None, _) => (0, "UNKNOWN".to_string()),
                        }
                    };
                    return get(key).filter(|s| !s.is_empty()).map(|t| {
                        cast_fail::CastFailLine::passthrough(t.replacen("%s", &name, 1).replacen(
                            "%d",
                            &count.to_string(),
                            1,
                        ))
                    });
                }
                cast_fail::cast_fail_text(
                    caster,
                    reason,
                    d,
                    cast_fail::FailArgs {
                        arg: fail.arg,
                        ..fail_args
                    },
                    &get,
                )
            })();
            // The reason, its wire word and both resolved buffers, so a declined arm is readable
            // from a probe run.
            debug!(
                "ui_action: cast fail — {caster:?} spell {spell_id} reason {reason:#04x} \
                 arg {:?} → {:?}",
                fail.arg, line
            );
            // The log entry from the shown text; nothing for a redisplay or for the pet.
            if let (Some(shown), false, false) = (line.as_ref(), pet, fail.redisplay) {
                fail_lines.push(crate::combat_log::SpellCastFailedLog {
                    spell_id,
                    reason: shown.logged().to_string(),
                });
            }
            line
        })
        .collect();
    cast_errors.0.extend(await_template);
    if let Some(log) = cast_fail_log.as_deref_mut() {
        for line in fail_lines {
            log.write(line);
        }
    }
    show_messages(
        &mut script,
        &mut sink,
        "ui_action",
        texts.into_iter().map(|l| Shown::keyed(l.key, l.text)),
    );

    // (Dis)mount refusals by key ([`mount_result_key`]); none of these strings takes arguments.
    let mount_texts: Vec<(&'static str, String)> = mount_errors
        .0
        .drain(..)
        .filter_map(|(mount, code)| {
            let key = mount_result_key(mount, code)?;
            Some((key, script.lua().globals().get::<String>(key).ok()?))
        })
        .collect();
    show_messages(
        &mut script,
        &mut sink,
        "ui_action",
        mount_texts
            .into_iter()
            .map(|(key, text)| Shown::keyed(key, text)),
    );

    // Taming refusals ([`PetTameFailures`]): the reason's `PETTAME_*` string fills
    // `ERR_TAME_FAILED` ("%s."), two lookups in that order.
    let tame_texts: Vec<Shown> = pet_tame_failures
        .0
        .drain(..)
        .filter_map(|reason| {
            let reason_key = benilla_protocol::messages::pet_tame_failure_key(reason);
            let reason_text = script.lua().globals().get::<String>(reason_key).ok()?;
            super::keyed_line_s(&script, "ERR_TAME_FAILED", &[&reason_text])
        })
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", tame_texts);

    // Client-local refusals by key ([`UiErrorKeys`]); the key also names the catalog record.
    let key_lines: Vec<Shown> = ui_error_keys
        .0
        .drain(..)
        .filter_map(|e| {
            ui_error_text(&e, &|key| script.lua().globals().get::<String>(key).ok())
                .map(|t| Shown::keyed(e.key, t))
        })
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", key_lines);

    // Already-resolved lines ([`UiErrorTexts`]); the queued kind is the message sink's flag.
    let resolved: Vec<Shown> = ui_error_texts
        .0
        .drain(..)
        .map(|(text, kind)| Shown::unkeyed(kind, text))
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", resolved);

    // The engine's own by-key refusals (`ERR_PASSIVE_ABILITY`): `benilla_ui` cannot reach
    // [`UiErrorKeys`], so it queues keys that show here, one frame late.
    let engine_keys = script.take_ui_errors();
    let engine_lines: Vec<Shown> = engine_keys
        .into_iter()
        .filter_map(|key| {
            let e = UiError::key(key);
            ui_error_text(&e, &|k| script.lua().globals().get::<String>(k).ok())
                .map(|t| Shown::keyed(e.key, t))
        })
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", engine_lines);
    // The same with one `%s` (`ERR_MULTI_CAST_ACTION_TOTEM_S`).
    let engine_s: Vec<Shown> = script
        .take_ui_errors_s()
        .into_iter()
        .filter_map(|(key, arg)| {
            let e = UiError::s(key, arg);
            ui_error_text(&e, &|k| script.lua().globals().get::<String>(k).ok())
                .map(|t| Shown::keyed(e.key, t))
        })
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", engine_s);

    // `SecureCmdOptionParse`'s unknown condition names (`0x5f011a`, message 574).
    let unknown = script.take_unknown_macro_options();
    let unknown_lines: Vec<Shown> = unknown
        .iter()
        .filter_map(|name| {
            super::errors::keyed_line_s(&script, "ERR_UNKNOWN_MACRO_OPTION_S", &[name])
        })
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", unknown_lines);

    let (store, auras) = self_q
        .iter()
        .next()
        .map_or((None, None), |(s, a)| (Some(s), a));

    // Stance page: the temporary bar's 5 while it has an owner, else our form's bonus bar offset,
    // with `UPDATE_BONUS_ACTIONBAR` on change and `ACTIONBAR_PAGE_CHANGED` first when leaving 5
    // (`0x5a83c0`).
    let form = store.map(|s| s.0.unit_shapeshift_form()).unwrap_or(0);
    let offset = if temp_flags != 0 {
        5
    } else {
        spells
            .as_ref()
            .and_then(|s| s.forms.get(&u32::from(form)))
            .map(|f| f.bonus_bar)
            .unwrap_or(0) as u8
    };
    script.set_temp_action_bar(temp_flags);
    if offset != memory.bonus_offset {
        debug!("ui_action: bonus bar offset {} (form {form})", offset);
        let left_temp = memory.bonus_offset == 5;
        memory.bonus_offset = offset;
        script.set_bonus_bar_offset(offset);
        if left_temp {
            script.fire_event("ACTIONBAR_PAGE_CHANGED", vec![]);
        }
        script.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    }

    // The identity resolve reruns on a new VM, a bar edit, a macro edit or a landed item
    // template: a template is fetched ask-once, so a cold slot's first resolve only issues the
    // query, and the epoch advance redisplays it, like the reference's cache callback.
    let template_epoch = items.template_epoch();
    let macro_generation = script.macros_generation();
    let macros_moved = macro_generation != memory.macro_generation;
    if !memory.resolved || actions.dirty || template_epoch != memory.template_epoch || macros_moved
    {
        actions.dirty = false;
        memory.resolved = true;
        memory.template_epoch = template_epoch;
        memory.macro_generation = macro_generation;
        let macros = script.macros();
        script.set_equipment_sets(
            actions
                .equipment_sets
                .iter()
                .map(|s| (s.id, s.name.clone()))
                .collect(),
        );

        // Each occupied slot's display, diffed against what the VM holds; a changed slot is
        // pushed and fires `ACTIONBAR_SLOT_CHANGED` with its Lua action id.
        let mut fresh: HashMap<u32, ActionSlot> = HashMap::new();
        for (slot, button) in &actions.buttons {
            let (texture, count, consumable, stackable) = match button.kind {
                ACTION_KIND_SPELL => {
                    let icon = spells.as_ref().and_then(|sp| {
                        let d = sp.catalog.get(button.action)?;
                        spell_action_icon(
                            button.action,
                            d,
                            sp,
                            store,
                            auras,
                            &objects,
                            &items,
                            icons.as_deref(),
                            &commands,
                        )
                    });
                    (icon, 0, false, false)
                }
                ACTION_KIND_ITEM => {
                    // Never nil: the reference's resolver returns the question mark, and the stock
                    // `ActionButton.lua:158` hides the icon on a nil texture.
                    let template = items.template(button.action, 0, &commands).cloned();
                    let texture = template
                        .as_ref()
                        .and_then(|t| icons.as_ref()?.catalog.get(t.display_info_id)?.icon.clone())
                        .unwrap_or_else(|| MISSING_ITEM_ICON.to_string());
                    let count = store
                        .map(|s| count_of(&s.0, &objects, button.action, InventoryScope::CARRIED))
                        .unwrap_or(0);
                    // The Count gate ([`ItemInfo::is_consumable`]) reads the icon's template, so it
                    // rides the same push.
                    let consumable = template.as_ref().is_some_and(|t| t.is_consumable());
                    // `IsStackableAction` (`0x5a8470`): the same template stacks past one.
                    let stackable = template.as_ref().is_some_and(|t| t.stackable > 1);
                    (Some(texture), count, consumable, stackable)
                }
                // A macro slot shows the macro's own icon, never its bound spell's, though its
                // state follows the bound spell.
                ACTION_KIND_MACRO => (
                    macros
                        .get(button.action as usize)
                        .and_then(|m| m.texture.clone()),
                    0,
                    false,
                    false,
                ),
                // A set slot shows `Interface\Icons\` and the set's icon, nothing for a set the
                // list lacks or an empty icon (the equipment manager's icon path, `0x5ae6b0`).
                ACTION_KIND_EQUIPMENT_SET => (
                    actions
                        .equipment_sets
                        .iter()
                        .find(|s| s.id == button.action)
                        .filter(|s| !s.icon.is_empty())
                        .map(|s| format!("Interface\\Icons\\{}", s.icon)),
                    0,
                    false,
                    false,
                ),
                // The pet bar's action at the slot's index (`0x5a97f0`): a spell's icon, or a mode
                // or order's `PET_%s_TEXTURE` global.
                super::ACTION_KIND_PET => (
                    pet_bar.as_deref().and_then(|bar| {
                        crate::ui_pet::temp_slot_texture(
                            bar,
                            button.action,
                            spells.as_ref().map(|s| &s.catalog),
                            &|key| script.lua().globals().get::<String>(key).ok(),
                        )
                    }),
                    0,
                    false,
                    false,
                ),
                _ => (None, 0, false, false),
            };
            fresh.insert(
                u32::from(*slot) + 1,
                ActionSlot {
                    texture,
                    kind: button.kind,
                    action: button.action,
                    count,
                    consumable,
                    stackable,
                    // `0x5a80e0`: the pet bar's spell at the slot's index.
                    pet_spell: (button.kind == super::ACTION_KIND_PET)
                        .then(|| {
                            let entry =
                                *pet_bar.as_deref()?.spells.bar.get(button.action as usize)?;
                            (entry.is_spell() && !entry.is_empty()).then(|| entry.action())
                        })
                        .flatten(),
                },
            );
        }
        // A macro rename changes the name line (`GetActionText`) but not the slot value, so a
        // macro edit re-fires every macro slot.
        let changed: Vec<u32> = fresh
            .keys()
            .chain(memory.pushed.keys())
            .copied()
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|a| {
                fresh.get(a) != memory.pushed.get(a)
                    || (macros_moved && fresh.get(a).is_some_and(|s| s.kind == ACTION_KIND_MACRO))
            })
            .collect();
        for &action in &changed {
            script.set_action(action, fresh.get(&action).cloned());
        }
        memory.pushed = fresh;
        debug!(
            "ui_action: fed {} changed slot(s) ({} occupied)",
            changed.len(),
            memory.pushed.len()
        );
        for action in changed {
            script.fire_event(
                "ACTIONBAR_SLOT_CHANGED",
                vec![ScriptValue::Int(i64::from(action))],
            );
        }
    }

    // An item slot's count and a live icon change without an action-table edit (eating a stack
    // sends no `SMSG_ACTION_BUTTONS`), so the pushed slots refresh every frame.
    if let Some(store) = store {
        for (&action, slot) in memory.pushed.iter_mut() {
            let changed = match slot.kind {
                ACTION_KIND_ITEM => {
                    let fresh = count_of(&store.0, &objects, slot.action, InventoryScope::CARRIED);
                    let changed = fresh != slot.count;
                    if changed {
                        slot.count = fresh;
                    }
                    changed
                }
                // Live icons: a weapon face follows the equipped weapon and form, and a toggle's
                // `ActiveIconID` follows its own aura.
                ACTION_KIND_SPELL => {
                    let d = spells
                        .as_ref()
                        .and_then(|s| s.catalog.get(slot.action))
                        .filter(|d| substitutes_weapon_icon(d) || d.active_icon_id != 0);
                    match (d, spells.as_ref()) {
                        (Some(d), Some(sp)) => {
                            let fresh = spell_action_icon(
                                slot.action,
                                d,
                                sp,
                                Some(store),
                                auras,
                                &objects,
                                &items,
                                icons.as_deref(),
                                &commands,
                            );
                            let changed = fresh != slot.texture;
                            if changed {
                                debug!(
                                    "ui_action: live icon swap slot {action} ({}) -> {fresh:?}",
                                    slot.action
                                );
                                slot.texture = fresh;
                            }
                            changed
                        }
                        _ => false,
                    }
                }
                _ => false,
            };
            if changed {
                script.set_action(action, Some(slot.clone()));
                script.fire_event(
                    "ACTIONBAR_SLOT_CHANGED",
                    vec![ScriptValue::Int(i64::from(action))],
                );
            }
        }
    }
}

/// A spell slot's icon, by the reference's `GetActionTexture` resolver in order: the auto-attack
/// faces ([`auto_attack_icon`]), then `ActiveIconID` while the spell's own aura is live and
/// cancelable (the predicate of [`super::toggle::active_action_toggle`]), then the spell's icon.
/// The spellbook's `GetSpellTexture` never serves `ActiveIconID`.
fn spell_action_icon(
    spell_id: u32,
    d: &benilla_formats::SpellDisplay,
    spells: &super::Spells,
    store: Option<&crate::net::ObjectStore>,
    auras: Option<&crate::unit_auras::UnitAuras>,
    objects: &crate::net::Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> Option<String> {
    auto_attack_icon(d, store, &spells.forms, objects, items, icons, commands)
        .or_else(|| {
            super::toggle::active_action_toggle(spell_id, d, auras)
                .then(|| d.active_icon.clone())
                .flatten()
        })
        .or_else(|| d.icon.clone())
}
