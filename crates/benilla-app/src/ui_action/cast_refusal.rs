//! A cast refusal's line, shown in its caller's turn: the display (`0x808200`) runs inside the refusing press or handler, so a press fires it at its step with the `World` lent, a handler in its turn.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, SelfPlayer};
use crate::sound::MessageSounds;
use crate::ui_chat::ChatLog;

use super::errors::{first_missing_totem, first_short_reagent, show_one};
use super::{cast_fail, CastFail, Caster, Shown, Spells};

/// Where a cast refusal goes: into the caller's ordered stream, shown in turn with the signals
/// around it.
pub(crate) trait CastRefusals {
    fn refuse(&mut self, fail: CastFail);

    /// A [`CastFail::local`] refusal.
    fn push_local(&mut self, spell_id: u32, reason: u8) {
        self.refuse(CastFail::local(spell_id, reason));
    }

    /// A client-local refusal with an argument word: the crowd-control `0x8d`, the missing tool
    /// category `0x9c`.
    fn push_local_arg(&mut self, spell_id: u32, reason: u8, arg: u32) {
        self.refuse(CastFail {
            arg: Some(arg),
            ..CastFail::local(spell_id, reason)
        });
    }

    /// The pet's `SMSG_PET_CAST_FAILED` refusal; its handler (`0x806c30`) passes the argument
    /// words to the message as the player's does.
    fn push_pet(&mut self, spell_id: u32, reason: u8, arg: Option<u32>) {
        self.refuse(CastFail {
            arg,
            caster: Caster::Pet,
            ..CastFail::local(spell_id, reason)
        });
    }
}

impl CastRefusals for crate::ui_script::live::UiEvents {
    fn refuse(&mut self, fail: CastFail) {
        self.cast_edge(crate::ui_cast::CastBarEdge::Refused(fail));
    }
}

/// A test's record of the refusals raised, in order.
impl CastRefusals for Vec<CastFail> {
    fn refuse(&mut self, fail: CastFail) {
        self.push(fail);
    }
}

/// Where a refusal that names an uncached item links its callback on the item's record, in ask
/// order with every subsystem's: the record waits the caller holds.
pub(crate) trait WaitRecord {
    fn wait_record(&mut self, entry: u32, wait: crate::items::RecordWait);
}

impl WaitRecord for crate::items::RecordWaits {
    fn wait_record(&mut self, entry: u32, wait: crate::items::RecordWait) {
        self.wait(entry, wait, false);
    }
}

/// A harness with no raise queue and no record store links nothing.
impl WaitRecord for () {
    fn wait_record(&mut self, _: u32, _: crate::items::RecordWait) {}
}

/// A test's record of the links, for a rig with no loan.
#[cfg(test)]
impl WaitRecord for Vec<(u32, crate::items::RecordWait)> {
    fn wait_record(&mut self, entry: u32, wait: crate::items::RecordWait) {
        self.push((entry, wait));
    }
}

/// The DBC name tables the cast-fail argument arms read, each absent without game data. The
/// shapeshift forms ride [`Spells`].
#[derive(SystemParam)]
pub(crate) struct FailNameTables<'w> {
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

/// The refusals shown, in order, for tests that run without the VM.
#[derive(Resource, Default)]
pub(crate) struct RefusalsSeen(pub(crate) Vec<CastFail>);

/// Everything a refusal's line reads and where it lands.
#[derive(SystemParam)]
pub(crate) struct CastFailShow<'w, 's> {
    spells: Option<Res<'w, Spells>>,
    self_q: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    objects: crate::net::Objects<'w, 's>,
    items: Res<'w, Items>,
    commands: Res<'w, NetCommands>,
    sub_classes: Option<Res<'w, crate::ui_items::ItemSubClasses>>,
    name_tables: FailNameTables<'w>,
    chat: Option<ResMut<'w, ChatLog>>,
    sounds: Option<ResMut<'w, MessageSounds>>,
    /// The combat log's line: `0x808200` calls the error display, then the log's spell-failure
    /// line (`0x751ad0`).
    log: Option<ResMut<'w, Messages<crate::combat_log::SpellCastFailedLog>>>,
}

impl CastFailShow<'_, '_> {
    /// Show `fail`'s line now, then log it; a line naming an item not yet cached waits for the
    /// answer.
    pub(crate) fn show(
        &mut self,
        script: Option<&mut UiScript>,
        fail: CastFail,
        records: &mut dyn WaitRecord,
    ) {
        let Some(script) = script else {
            return;
        };
        let mut wait = None;
        let line = self.resolve(script, fail, &mut wait);
        // The first pass shows nothing; the record's answer shows it (`0x808200`'s callbacks,
        // linked each time).
        if let Some(entry) = wait {
            records.wait_record(
                entry,
                crate::items::RecordWait::CastFailure(fail.requeued()),
            );
            return;
        }
        // The reason, its wire word and the resolved buffers, so a declined arm is readable from
        // a probe run.
        debug!(
            "ui_action: cast fail — {:?} spell {} reason {:#04x} arg {:?} → {line:?}",
            fail.caster, fail.spell_id, fail.reason, fail.arg
        );
        let Some(line) = line else {
            return;
        };
        let logged = (line.logs && fail.caster == Caster::Player && !fail.redisplay)
            .then(|| line.logged().to_string());
        show_one(
            script,
            self.chat.as_deref_mut(),
            self.sounds.as_deref_mut(),
            "ui_action",
            Shown::keyed(line.key, line.text),
        );
        // The log entry from the shown text; nothing for a redisplay, the pet or a custom line.
        if let (Some(reason), Some(log)) = (logged, self.log.as_mut()) {
            log.write(crate::combat_log::SpellCastFailedLog {
                spell_id: fail.spell_id,
                reason,
            });
        }
    }

    /// The line for `fail` through [`cast_fail`]. The arms that need our bags or the item cache
    /// live here: `0x78` and `0x5c` name the failing slot's item, found as the pre-send check
    /// finds it; on a cache miss `wait` takes the entry.
    fn resolve(
        &self,
        script: &UiScript,
        fail: CastFail,
        wait: &mut Option<u32>,
    ) -> Option<cast_fail::CastFailLine> {
        let CastFail {
            spell_id,
            reason,
            caster,
            ..
        } = fail;
        let pet = caster == Caster::Pet;
        let spells = self.spells.as_deref();
        let d = spells.and_then(|s| s.catalog.get(spell_id));
        let get = |key: &str| script.lua().globals().get::<String>(key).ok();
        let sub_classes = &self.sub_classes;
        let self_store = self.self_q.iter().next();
        let objects = &self.objects;
        let items = &*self.items;
        let commands = &*self.commands;
        let name_tables = &self.name_tables;
        let fail_args = name_tables.args(spells);
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
                    .and_then(|s| first_missing_totem(d, s, objects))
                    // No self store yet: name the first tool.
                    .or_else(|| d.totems.iter().copied().find(|&t| t != 0))
            } else {
                self_store
                    .and_then(|s| first_short_reagent(d, s, objects))
                    .or_else(|| d.reagents.iter().map(|&(id, _)| id).find(|&id| id != 0))
            }?;
            let cached = items.template(failing, 0, commands).map(|i| i.name.clone());
            let name = match cached {
                Some(name) => name,
                // Answered unknown: the reference's callback literal.
                None if items.template_answered_unknown(failing) => "UNKNOWN".to_string(),
                None => {
                    // Pending: the answer's callback redisplays it, unlogged.
                    *wait = Some(failing);
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
                let cached = fail
                    .arg
                    .and_then(|id| items.template(id, 0, commands).map(|i| i.name.clone()));
                match (cached, fail.arg) {
                    (Some(name), _) => (second, name),
                    (None, Some(id)) if !items.template_answered_unknown(id) => {
                        *wait = Some(id);
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
    }
}

/// Show `fail` from a world step: a handler's refusal in its turn.
pub(crate) fn show_refusal(world: &mut World, fail: CastFail) {
    if let Some(mut seen) = world.get_resource_mut::<RefusalsSeen>() {
        seen.0.push(fail);
    }
    if let Err(e) = world.run_system_cached_with(show_refusal_now, fail) {
        warn!("ui_action: a cast refusal was not shown: {e}");
    }
}

fn show_refusal_now(
    In(fail): In<CastFail>,
    mut vm: crate::ui_script::InTurn,
    mut waits: Option<NonSendMut<crate::items::RecordWaits>>,
    show: Option<CastFailShow>,
) {
    let Some(mut show) = show else {
        return;
    };
    let mut script = vm.turn();
    match waits.as_deref_mut() {
        Some(waits) => show.show(script.as_deref_mut(), fail, waits),
        None => show.show(script.as_deref_mut(), fail, &mut ()),
    }
}

/// The refusals raised and not yet shown or shown so far, in order, taken: the waiting press
/// edges, the handler queue and the record of shown ones.
#[cfg(test)]
pub(crate) fn take_refusals(world: &mut World) -> Vec<CastFail> {
    let mut out = world
        .get_resource_mut::<RefusalsSeen>()
        .map(|mut s| std::mem::take(&mut s.0))
        .unwrap_or_default();
    if let Some(mut edges) = world.get_resource_mut::<crate::ui_cast::PressEdges>() {
        out.extend(edges.take_refusals());
    }
    if let Some(mut ui) = world.get_resource_mut::<crate::ui_script::live::UiEvents>() {
        out.extend(ui.take_refusals());
    }
    out
}
