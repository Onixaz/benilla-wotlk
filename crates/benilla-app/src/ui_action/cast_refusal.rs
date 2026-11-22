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

    /// The pet's `SMSG_PET_CAST_FAILED` refusal; its handler (`0x806c30`) passes both argument words to the pet's display (`0x806d60`).
    fn push_pet(&mut self, spell_id: u32, reason: u8, arg: Option<u32>, arg2: Option<u32>) {
        self.refuse(CastFail {
            arg,
            arg2,
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
    /// `ItemLimitCategory.dbc`, for `0x77`.
    limits: Option<Res<'w, crate::ui_trade::ItemLimitCategories>>,
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
            own_power: None,
            item_name: None,
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
    /// answer. Whether the display took its silent exit (`0x8086ff`), which ends the cast quietly.
    pub(crate) fn show(
        &mut self,
        script: Option<&mut UiScript>,
        fail: CastFail,
        records: &mut dyn WaitRecord,
    ) -> bool {
        let Some(script) = script else {
            return false;
        };
        let mut wait = None;
        let line = self.resolve(script, fail, &mut wait);
        // The first pass shows nothing; the record's answer shows it (`0x808200`'s callbacks,
        // linked each time), and the cast ends by the silent exit (`0x8086db`, `0x80876f`).
        if let Some(entry) = wait {
            records.wait_record(
                entry,
                crate::items::RecordWait::CastFailure(fail.requeued()),
            );
            return true;
        }
        let quiet = fail.caster == Caster::Player && !fail.redisplay && self.quiet(script, fail);
        // The reason, its wire word and the resolved buffers, so a declined arm is readable from
        // a probe run.
        debug!(
            "ui_action: cast fail — {:?} spell {} reason {:#04x} arg {:?} → {line:?}",
            fail.caster, fail.spell_id, fail.reason, fail.arg
        );
        let Some(line) = line else {
            return quiet;
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
        false
    }

    /// The player's arms that end in the silent exit with no line ([`cast_fail::exits_quietly`]).
    fn quiet(&self, script: &UiScript, fail: CastFail) -> bool {
        let spells = self.spells.as_deref();
        let d = spells.and_then(|s| s.catalog.get(fail.spell_id));
        let get = |key: &str| script.lua().globals().get::<String>(key).ok();
        let args = cast_fail::FailArgs {
            arg: fail.arg,
            own_power: self.self_q.iter().next().map(|s| s.0.unit_power_type()),
            ..self.name_tables.args(spells)
        };
        cast_fail::exits_quietly(fail.reason, d, args, &get)
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
        // 176's casting item (`0x808a0a`): an item object's template name, never waited on, else the spell's.
        let item_name = (!pet)
            .then_some(fail.item)
            .flatten()
            .and_then(|guid| objects.object(guid))
            .filter(|o| o.type_mask() & 0x2 != 0)
            .and_then(|o| o.object_entry())
            .and_then(|entry| items.template(entry, 0, commands))
            .map(|i| i.name.clone());
        let fail_args = cast_fail::FailArgs {
            own_power: self_store.map(|s| s.0.unit_power_type()),
            item_name: item_name.as_deref(),
            ..name_tables.args(spells)
        };
        let template = |key: &str| get(key).filter(|s| !s.is_empty());
        // 29-31 (`0x8085dd`): the class and mask the wire's two words name, through `ItemSubClassMask.dbc` on the exact pair, else the first masked subclass (`0x7fea30`); a local refusal passes the spell's own. No name shows the template as is.
        if let (0x19..=0x1b, Some(subs)) = (reason, sub_classes.as_deref()) {
            let pair = match (fail.arg, fail.arg2) {
                (Some(class), Some(mask)) => Some((class, mask)),
                _ => d
                    .filter(|d| d.equipped_item_class >= 0)
                    .map(|d| (d.equipped_item_class as u32, d.equipped_item_subclass_mask)),
            };
            if let Some(name) = pair.and_then(|(c, m)| subs.0.requirement_display_name(c, m)) {
                let key = cast_fail::CAST_FAIL_KEYS[reason as usize];
                return template(key)
                    .map(|t| cast_fail::CastFailLine::passthrough(t.replace("%s", &name)));
            }
            // The pet's arm shows nothing with no name (`0x8057bb` -> `0x80581c`).
            if pet {
                return None;
            }
        }
        // 54 (`0x80865d`): the first subclass of class 6 (Projectile) under `1 << word`, the shift masked to five bits as x86 `shl` masks it, with no mask-group stage. The player's arm only.
        if let (false, 0x31, Some(arg), Some(subs)) =
            (pet, reason, fail.arg, sub_classes.as_deref())
        {
            const ITEM_CLASS_PROJECTILE: u32 = 6;
            if let Some(name) = subs
                .0
                .first_masked_display_name(ITEM_CLASS_PROJECTILE, 1u32 << (arg & 31))
            {
                let key = cast_fail::CAST_FAIL_KEYS[reason as usize];
                return template(key)
                    .map(|t| cast_fail::CastFailLine::passthrough(t.replace("%s", name)));
            }
        }
        // 100's word `-2` (`0x80869a`): message 0, `ERR_INV_FULL`, with an empty argText, so the log reads the shown line (`0x808b23`).
        if !pet && reason == 0x5c && fail.arg == Some(0xFFFF_FFFE) {
            return template("ERR_INV_FULL").map(|text| cast_fail::CastFailLine {
                key: "ERR_INV_FULL",
                text,
                arg_text: String::new(),
                logs: true,
            });
        }
        // 129 (`0x808a86`): a positive word names an `ItemLimitCategory.dbc` row, shown on message `0x272` with its quantity and name (`0x8080b1`, `0x808aa5`), the log reading the shown line.
        if let (false, 0x77, Some(id)) = (pet, reason, fail.arg.filter(|&a| a as i32 > 0)) {
            let row = name_tables.limits.as_deref().and_then(|l| l.0.get(id));
            if let Some(row) = row {
                const KEY: &str = "ERR_ITEM_MAX_LIMIT_CATEGORY_COUNT_EXCEEDED_IS";
                return template(KEY).map(|t| cast_fail::CastFailLine {
                    key: KEY,
                    text: benilla_ui::strings::fill(
                        &t,
                        &[
                            benilla_ui::strings::Arg::D(i64::from(row.quantity)),
                            benilla_ui::strings::Arg::S(&row.name),
                        ],
                    ),
                    arg_text: String::new(),
                    logs: true,
                });
            }
        }
        // 131 and 100 (`0x808695`), the player's arms only: the item the wire's word names; a local refusal names the first missing tool or short reagent, as the pre-send check finds it.
        if !pet && (reason == 0x78 || reason == 0x5c) {
            let d = d?;
            let failing = match fail.arg {
                Some(item) => item,
                None if reason == 0x78 => self_store
                    .and_then(|s| first_missing_totem(d, s, objects))
                    // No self store yet: name the first tool.
                    .or_else(|| d.totems.iter().copied().find(|&t| t != 0))?,
                None => self_store
                    .and_then(|s| first_short_reagent(d, s, objects))
                    .or_else(|| d.reagents.iter().map(|&(id, _)| id).find(|&id| id != 0))?,
            };
            let cached = items.template(failing, 0, commands).map(|i| i.name.clone());
            let name = match cached {
                Some(name) => name,
                // Answered unknown: the reference's callback literal.
                None if items.template_answered_unknown(failing) => "UNKNOWN".to_string(),
                None => {
                    // Pending: the silent exit (`0x8086db`); the answer's callback redisplays it, unlogged.
                    *wait = Some(failing);
                    return None;
                }
            };
            let key = if reason == 0x78 {
                "SPELL_FAILED_TOTEMS"
            } else {
                "SPELL_FAILED_REAGENTS"
            };
            return template(key)
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

/// Show `fail` from a world step: a handler's refusal in its turn. Whether the display took its silent exit (`0x8086ff`).
pub(crate) fn show_refusal(world: &mut World, fail: CastFail) -> bool {
    if let Some(mut seen) = world.get_resource_mut::<RefusalsSeen>() {
        seen.0.push(fail);
    }
    match world.run_system_cached_with(show_refusal_now, fail) {
        Ok(quiet) => quiet,
        Err(e) => {
            warn!("ui_action: a cast refusal was not shown: {e}");
            false
        }
    }
}

fn show_refusal_now(
    In(fail): In<CastFail>,
    mut vm: crate::ui_script::InTurn,
    mut waits: Option<NonSendMut<crate::items::RecordWaits>>,
    show: Option<CastFailShow>,
) -> bool {
    let Some(mut show) = show else {
        return false;
    };
    let mut script = vm.turn();
    match waits.as_deref_mut() {
        Some(waits) => show.show(script.as_deref_mut(), fail, waits),
        None => show.show(script.as_deref_mut(), fail, &mut ()),
    }
}

/// `0x808200` for our own failure: the line, then the cast's end on the wire reason, or on `DONT_REPORT` (wire 27) when the display took its silent exit (`0x808705`, `0x808b6c`).
pub(crate) fn refuse_then_end(world: &mut World, fail: CastFail, caster: u64, cast_count: u8) {
    const DONT_REPORT: u8 = 0x17;
    let quiet = show_refusal(world, fail);
    let edge = crate::ui_cast::CastBarEdge::Failed {
        caster,
        spell_id: fail.spell_id,
        cast_count,
        reason: if quiet { DONT_REPORT } else { fail.reason },
    };
    match world.get_resource_mut::<crate::ui_script::live::UiEvents>() {
        Some(mut ui) => ui.cast_edge(edge),
        None => crate::ui_cast::apply_edge(world, edge),
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
