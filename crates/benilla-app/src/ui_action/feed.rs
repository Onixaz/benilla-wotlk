//! [`feed_actions`] drains the UI error queues and feeds the stance page (`GetBonusBarOffset`: the
//! form's `SpellShapeshiftForm.dbc` BonusActionBar column).

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, SelfPlayer};

use super::errors::{first_missing_totem, first_short_reagent, mount_result_key};
use super::{
    cast_fail, show_messages, ui_error_text, CastErrors, MessageSink, MountErrors, PetTameFailures,
    PlayerActions, Shown, Spells, UiError, UiErrorKeys, UiErrorTexts,
};

/// The feed's memory of the stance page it last pushed.
#[derive(Default)]
pub(super) struct FeedMemory {
    bonus_offset: u8,
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
            power_displays: spells.map(|s| &s.power_displays),
        }
    }
}

#[allow(clippy::type_complexity)] // a Bevy system's full input set
pub(super) fn feed_actions(
    (script, _live): (
        Option<NonSendMut<UiScript>>,
        crate::ui_script::LendLive,
    ),
    action_store: crate::ui_script::LentRef<PlayerActions>,
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
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    objects: crate::net::Objects,
    items: Res<Items>,
    // With the temporary bar's owners.
    bars: (
        Option<Res<crate::ui_items::ItemSubClasses>>,
        Option<Res<super::TempActionBar>>,
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
    let (sub_classes, temp_bar) = bars;
    let temp_flags = temp_bar.map_or(0, |t| t.flags);
    // Cold Weather Flying, what `flyable` needs in Northrend (`0x7261b0`).
    let knows = action_store
        .get()
        .is_some_and(|a| a.spells.contains(&54197));
    script.set_knows_cold_weather_flying(knows);
    let memory = memory.get(&script);

    // Cast failures become error lines through [`cast_fail`], all resolved before any event fires.
    // The drain owns the arms that need our bags or the item cache: `0x78` and `0x5c` name the
    // failing slot's item, found as the pre-send check finds it. On a cache miss the entry stays
    // queued, as the reference's callback redisplays on the answer.
    let self_store = self_q.iter().next();
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
            // The log entry from the shown text; nothing for a redisplay, the pet or a custom line.
            if let (Some(shown), false, false) =
                (line.as_ref().filter(|l| l.logs), pet, fail.redisplay)
            {
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

    // The sound half of the lines verbs displayed inline, their events already fired in the call.
    for key in script.take_message_sounds() {
        if let Some(record) = benilla_ui::messages::by_key(key) {
            sink.sounds.push(record);
        }
    }

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

    let store = self_q.iter().next();

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
}
