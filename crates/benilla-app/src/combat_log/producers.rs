//! The 12340 combat-log producers: each builds its entry and fires what the reference fires beside
//! it, the centre text (`COMBAT_TEXT_UPDATE`), `UNIT_COMBAT` and the floating word or number.

use benilla_formats::SpellDisplay;
use benilla_protocol::messages::power_display_scale;
use benilla_ui::script::{
    CombatLogEvent, CombatLogInfo, CombatLogItem, CombatLogSpell, Damage, Drain, Heal, MissType,
    ScriptValue, UiScript,
};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::entries::{self, CombatLogClock, FlagInputs, NewEntry, PendingEntries};
use crate::combat_text::{damage_color, miss_word, CombatTextSpawn, DamageSource, DamageTextGates};
use crate::names::NameCache;
use crate::net::{GuidIndex, NetCommands, ObjectStore, Reputations, SelfGuid};
use crate::target::ring::Factions;
use crate::ui_action::Spells;
use crate::ui_party::GroupState;
use crate::ui_unit::{CombatTextEvent, UnitCombatFeedback};

/// `Attributes & 0x180`: the spell is neither displayed nor logged.
const ATTR_NOT_LOGGED: u32 = 0x180;
/// `Attributes & 0x100`: no feedback, no floating text.
const ATTR_NO_FEEDBACK: u32 = 0x100;
/// `Attributes & 0x40`: a passive spell, whose auras do not log (`0x752710`).
const ATTR_PASSIVE: u32 = 0x40;
/// `AttributesExC & 0x8000`: a ranged basic shot, logged as `RANGE_*`.
const EX3_RANGED: u32 = 0x8000;
/// `AttributesExC & 0x2000000`: a heal or energize packet logs as periodic.
const EX3_PERIODIC_LOG: u32 = 0x200_0000;
/// `AttributesExD & 0x1`: hidden from the cast log (`0x74d3f0`).
const EX4_HIDDEN: u32 = 0x1;
/// `AttributesExD & 0x2000`: the entry names no source (`0x750400`).
const EX4_NO_SOURCE: u32 = 0x2000;
/// `AttributesExF & 0x40`: the cast's name floats in the combat text.
const EX6_CAST_TEXT: u32 = 0x40;
/// `AttributesExF & 0x400`: the aura does not log.
const EX6_AURA_NOT_LOGGED: u32 = 0x400;
/// `AttributesExF & 0x200000`: the spell's mechanics float as words (`0x74da03`).
const EX6_MECHANIC_TEXT: u32 = 0x20_0000;
/// `SPELL_EFFECT_OPEN_LOCK`: a spell carrying it floats no mechanic (`0x74db80`).
const EFFECT_OPEN_LOCK: u32 = 33;
/// `SpellMechanic.dbc` 26, "interrupted": the interrupt's word, never an aura's.
const MECHANIC_INTERRUPT: u32 = 26;

/// A spell record logs: present, `Attributes & 0x180` clear and named.
fn logged(d: &SpellDisplay) -> bool {
    d.attributes & ATTR_NOT_LOGGED == 0 && !d.name.is_empty()
}

/// The hidden-in-log check (`0x74d3f0`): unlogged, a learn effect (36), or `AttributesExD & 1`.
fn hidden_in_log(d: &SpellDisplay) -> bool {
    !logged(d) || d.effects.contains(&36) || d.attributes_ex4 & EX4_HIDDEN != 0
}

/// The power token (`0x7fe4b0`) the energize text carries.
fn power_word(power: u32) -> &'static str {
    match power {
        0 => "MANA",
        1 => "RAGE",
        2 => "FOCUS",
        3 => "ENERGY",
        4 => "HAPPINESS",
        5 => "RUNES",
        6 => "RUNIC_POWER",
        _ => "",
    }
}

/// The miss feedback's action word, `0xadbfac` by miss code.
fn miss_action(code: u8) -> Option<&'static str> {
    Some(match code {
        0 => "NONE",
        _ => MissType::from_code(code)?.word(),
    })
}

/// The read side of every producer.
#[derive(SystemParam)]
pub(crate) struct LogCtx<'w, 's> {
    self_guid: Res<'w, SelfGuid>,
    index: Res<'w, GuidIndex>,
    stores: Query<'w, 's, &'static ObjectStore>,
    spells: Option<Res<'w, Spells>>,
    group: Res<'w, GroupState>,
    factions: Option<Res<'w, Factions>>,
    reputations: Res<'w, Reputations>,
    selection: Option<Res<'w, crate::target::Selection>>,
    script: Option<NonSend<'w, UiScript>>,
    spell_mods: Option<Res<'w, crate::spell::SpellModifiers>>,
    enchants: Option<Res<'w, crate::items::Enchants>>,
    objects: Option<Res<'w, crate::go_templates::GameObjectTemplates>>,
    names: Res<'w, NameCache>,
    net: Res<'w, NetCommands>,
    mechanics: Option<Res<'w, crate::ui_action::SpellMechanics>>,
}

impl LogCtx<'_, '_> {
    /// A mechanic's `SpellMechanic.dbc` state name.
    fn mechanic_name(&self, id: u32) -> Option<&str> {
        self.mechanics.as_deref()?.catalog.name(id)
    }

    pub(crate) fn me(&self) -> Option<u64> {
        self.self_guid.0
    }

    pub(crate) fn spell(&self, id: u32) -> Option<&SpellDisplay> {
        self.spells.as_ref()?.catalog.get(id)
    }

    pub(crate) fn store(&self, guid: u64) -> Option<&ObjectStore> {
        self.index
            .0
            .get(&guid)
            .and_then(|&e| self.stores.get(e).ok())
    }

    pub(crate) fn entity(&self, guid: u64) -> Option<Entity> {
        self.index.0.get(&guid).copied()
    }

    /// A streamed unit (an object lookup with the unit mask).
    fn unit(&self, guid: u64) -> bool {
        self.store(guid).is_some_and(ObjectStore::is_unit)
    }

    /// The combat-text unit (`0x74d430`): the player.
    fn is_text_unit(&self, guid: u64) -> bool {
        self.me() == Some(guid)
    }

    /// The name by GUID: the name now, or empty while it is asked.
    fn name_of(&self, guid: u64) -> String {
        self.names
            .resolve_unit(guid, self.store(guid), &self.net)
            .unwrap_or_default()
            .to_owned()
    }

    fn flag_inputs(&self) -> FlagInputs<'_> {
        FlagInputs {
            me: self.me(),
            group: &self.group,
            target: self.selection.as_ref().and_then(|s| s.guid),
            focus: self
                .script
                .as_ref()
                .map(|s| s.focus_guid())
                .filter(|&g| g != 0),
            factions: self.factions.as_deref(),
            reputations: &self.reputations,
        }
    }

    /// The spell's cast time (`0x71ad20`, the player's `0x6d68d0`) is nonzero: a ranged spell
    /// always, else the level-scaled row, the player's own after its cast-time modifiers.
    fn has_cast_time(&self, caster: u64, d: &SpellDisplay) -> bool {
        if d.attributes & 0x2 != 0 {
            return true;
        }
        let Some(spells) = self.spells.as_deref() else {
            return false;
        };
        let Some(row) = spells.cast_times.get(d.casting_time_index) else {
            return false;
        };
        let level = self
            .store(caster)
            .and_then(|s| s.0.unit_level())
            .unwrap_or(0);
        let mut time = row.resolved_ms(level, d.base_level);
        if self.me() == Some(caster) {
            if let Some(mods) = self.spell_mods.as_deref() {
                time = mods.apply(d, crate::spell::OP_CAST_TIME, time);
            }
        }
        time > 0
    }

    /// A periodic heal's duration past 30 s floats nothing over the player (`0x750860`).
    fn long_duration(&self, d: &SpellDisplay, caster: u64) -> bool {
        let Some(row) = self
            .spells
            .as_deref()
            .and_then(|s| s.durations.get(d.duration_index))
        else {
            return false;
        };
        let level = self
            .store(caster)
            .and_then(|s| s.0.unit_level())
            .unwrap_or(0) as i32;
        let scaled = row.base_ms + row.per_level_ms * (level - d.base_level as i32).max(0);
        scaled.min(row.max_ms) > 30_000
    }

    /// Whether the active player owns the source (`0x7190a0`).
    fn owner_class(&self, source: u64) -> Option<DamageSource> {
        crate::combat_text::owned_by_active_player(source, self.me(), |g| self.store(g))
    }
}

/// The write side: the entry queue, its clock and every event a producer fires.
#[derive(SystemParam)]
pub(crate) struct LogOut<'w> {
    pending: ResMut<'w, PendingEntries>,
    clock: Res<'w, CombatLogClock>,
    time: Option<Res<'w, Time<bevy::time::Real>>>,
    gates: Res<'w, DamageTextGates>,
    text: MessageWriter<'w, CombatTextSpawn>,
    feedback: MessageWriter<'w, UnitCombatFeedback>,
    center: MessageWriter<'w, CombatTextEvent>,
    pub(crate) chat: ResMut<'w, crate::ui_chat::ChatLog>,
}

impl LogOut<'_> {
    fn now_ms(&self) -> u64 {
        self.time
            .as_ref()
            .map_or(0, |t| t.elapsed().as_millis() as u64)
    }

    fn center(&mut self, message_type: &'static str, args: Vec<ScriptValue>) {
        self.center.write(CombatTextEvent { message_type, args });
    }
}

/// `%d`.
fn int(n: i32) -> ScriptValue {
    ScriptValue::Int(i64::from(n))
}

/// `%s`.
fn text(s: impl Into<String>) -> ScriptValue {
    ScriptValue::Str(s.into())
}

/// The combat log's producers, one per 12340 function.
#[derive(SystemParam)]
pub(crate) struct Log<'w, 's> {
    pub(crate) ctx: LogCtx<'w, 's>,
    pub(crate) out: LogOut<'w>,
}

impl Log<'_, '_> {
    /// The entry's allocation and queueing (`0x750400`, `0x74ff20`).
    pub(crate) fn queue(
        &mut self,
        event: CombatLogEvent,
        source: u64,
        dest: u64,
        spell: Option<u32>,
        info: CombatLogInfo,
    ) {
        let rec = spell.and_then(|id| self.ctx.spell(id));
        let hide_source = rec.is_some_and(|d| d.attributes_ex4 & EX4_NO_SOURCE != 0);
        let spell = spell.map(|id| CombatLogSpell {
            id,
            name: rec.map(|d| d.name.clone()),
            school: rec.map_or(0, |d| d.school_mask),
        });
        let now = (self.out.now_ms(), *self.out.clock);
        let inputs = self.ctx.flag_inputs();
        let ctx = &self.ctx;
        self.out.pending.queue(
            NewEntry {
                event,
                source,
                dest,
                spell,
                hide_source,
                info,
            },
            now,
            &inputs,
            |g| ctx.store(g),
        );
    }

    /// The spell's record when it logs (`Attributes & 0x180` clear and named).
    fn logged(&self, spell: u32) -> Option<&SpellDisplay> {
        self.ctx.spell(spell).filter(|d| logged(d))
    }

    /// The combat feedback and its `UNIT_COMBAT` signal (`0x513380`, `0x5132c0`).
    pub(crate) fn combat_feedback(&mut self, guid: u64, amount: u32, school: u32, flags: u32) {
        self.unit_combat(guid, "WOUND", amount as i32, school, flags);
    }

    /// The healing feedback (`0x513480`): physical school, the crit bit.
    fn healing_feedback(&mut self, guid: u64, amount: u32, crit: bool) {
        if amount != 0 {
            self.unit_combat(guid, "HEAL", amount as i32, 1, if crit { 0x200 } else { 0 });
        }
    }

    /// The `UNIT_COMBAT` signal (`0x5132c0`): the descriptor by the amount's sign and the flags.
    fn unit_combat(&mut self, guid: u64, action: &'static str, amount: i32, school: u32, f: u32) {
        let flags = if amount <= 0 {
            if f & 0x20 != 0 {
                "ABSORB"
            } else if f & 0x2000 != 0 {
                "BLOCK"
            } else if f & 0x80 != 0 {
                "RESIST"
            } else {
                ""
            }
        } else if f & 0x200 != 0 {
            "CRITICAL"
        } else if f & 0x10000 != 0 {
            "GLANCING"
        } else if f & 0x20000 != 0 {
            "CRUSHING"
        } else {
            ""
        };
        self.out.feedback.write(UnitCombatFeedback {
            guid,
            action,
            flags,
            amount,
            school,
        });
    }

    /// The spell-miss feedback (`0x513410`): the miss word, no amount, physical school.
    pub(crate) fn miss_feedback(&mut self, guid: u64, code: u8) {
        if let Some(action) = miss_action(code) {
            self.out.feedback.write(UnitCombatFeedback {
                guid,
                action,
                flags: "",
                amount: 0,
                school: 1,
            });
        }
    }

    /// The emitters' shared gates and colour (`0x71f990`, `0x722340`): never over the player,
    /// only from the player or what it owns, under `CombatDamage`, and a periodic tick under
    /// `CombatLogPeriodicSpells`.
    fn world_color(
        &self,
        target: u64,
        source: u64,
        melee: bool,
        periodic: bool,
    ) -> Option<(Entity, Option<u32>)> {
        if self.ctx.me() == Some(target) || (periodic && !self.out.gates.periodic) {
            return None;
        }
        let anchor = self.ctx.entity(target).filter(|_| self.ctx.unit(target))?;
        let class = self.ctx.owner_class(source)?;
        Some((anchor, damage_color(*self.out.gates, class, melee)?))
    }

    /// The floating word (`0x71f990`): the outcome word over `target`.
    pub(crate) fn world_word(
        &mut self,
        target: u64,
        source: u64,
        code: u8,
        spell: u32,
        periodic: bool,
    ) {
        let melee = crate::combat_text::melee_styled(self.ctx.spell(spell));
        let Some((anchor, color)) = self.world_color(target, source, melee, periodic) else {
            return;
        };
        if let Some((word, category)) = miss_word(code) {
            self.out.text.write(CombatTextSpawn {
                anchor,
                text: word.to_string(),
                category,
                color,
            });
        }
    }

    /// The floating damage number (`0x722340`): the number over `target`, category 2 on a crit; a
    /// damage shield passes no record and colours as a spell.
    fn world_damage(
        &mut self,
        target: u64,
        source: u64,
        amount: u32,
        spell: Option<u32>,
        crit: bool,
        periodic: bool,
    ) {
        if amount == 0 {
            return;
        }
        let melee = match spell {
            Some(id) => crate::combat_text::melee_styled(self.ctx.spell(id)),
            None => false,
        };
        let Some((anchor, color)) = self.world_color(target, source, melee, periodic) else {
            return;
        };
        if benilla_assets::trace::enabled() {
            benilla_assets::trace::line(
                "fct",
                &format!("damage {amount} over {target:#x} from {source:#x} crit={crit}"),
            );
        }
        self.out.text.write(CombatTextSpawn {
            anchor,
            text: amount.to_string(),
            category: if crit { 2 } else { 0 },
            color,
        });
    }

    /// The floating heal number (`0x722440`): `+%d` over `target`, the player's own heals only,
    /// under `CombatHealing`, category 7 on a crit.
    fn world_heal(&mut self, target: u64, source: u64, amount: u32, crit: bool) {
        if amount == 0 || self.ctx.me() == Some(target) || !self.out.gates.healing {
            return;
        }
        if self.ctx.owner_class(source) != Some(DamageSource::Player) {
            return;
        }
        let Some(anchor) = self.ctx.entity(target).filter(|_| self.ctx.unit(target)) else {
            return;
        };
        self.out.text.write(CombatTextSpawn {
            anchor,
            text: format!("+{amount}"),
            category: if crit { 7 } else { 6 },
            color: None,
        });
    }

    // ── melee ────────────────────────────────────────────────────────────────────────────────

    /// The attack-round entry (`0x7512b0`): the swing's entry, and its centre text over the player.
    pub(crate) fn swing(&mut self, s: &benilla_protocol::messages::AttackerState) {
        use benilla_protocol::messages::hit_info as h;
        let Some((event, info)) = entries::swing_entry(s) else {
            return;
        };
        self.queue(event, s.attacker, s.victim, None, info);
        if !self.ctx.is_text_unit(s.victim) {
            return;
        }
        let damage = s.damage as i32;
        if event == CombatLogEvent::SwingDamage {
            let (ty, args) = if s.resist > 0 {
                ("RESIST", vec![int(damage), int(s.resist)])
            } else if s.blocked != 0 {
                ("BLOCK", vec![int(damage), int(s.blocked as i32)])
            } else if s.absorb != 0 {
                ("ABSORB", vec![int(damage), int(s.absorb as i32)])
            } else if s.hit_info & h::CRIT != 0 {
                ("DAMAGE_CRIT", vec![int(damage)])
            } else {
                ("DAMAGE", vec![int(damage)])
            };
            self.out.center(ty, args);
            return;
        }
        let ty = if s.hit_info & h::MISS != 0 {
            "MISS"
        } else if s.hit_info & h::FULL_ABSORB != 0 {
            "ABSORB"
        } else if s.hit_info & h::FULL_RESIST != 0 {
            "RESIST"
        } else {
            match s.victim_state {
                2 => "DODGE",
                3 => "PARRY",
                5 => "BLOCK",
                6 => "EVADE",
                7 => "IMMUNE",
                8 => "DEFLECT",
                _ => return,
            }
        };
        self.out.center(ty, Vec::new());
    }

    // ── damage ───────────────────────────────────────────────────────────────────────────────

    /// The damage event (`0x750660`): the damage entry, and its centre text over the player in
    /// the spell words for `SPELL_DAMAGE` alone.
    fn damage_event(
        &mut self,
        event: CombatLogEvent,
        source: u64,
        dest: u64,
        spell: Option<u32>,
        d: Damage,
    ) {
        self.queue(
            event,
            source,
            dest,
            spell,
            CombatLogInfo {
                damage: Some(d),
                ..CombatLogInfo::default()
            },
        );
        if !self.ctx.is_text_unit(dest) {
            return;
        }
        let spelled = event == CombatLogEvent::SpellDamage;
        let pick = |spell_word: &'static str, word: &'static str| {
            if spelled {
                spell_word
            } else {
                word
            }
        };
        let (ty, args) = if d.resisted > 0 {
            (
                pick("SPELL_RESIST", "RESIST"),
                vec![int(d.amount), int(d.resisted)],
            )
        } else if d.blocked != 0 {
            (
                pick("SPELL_BLOCK", "BLOCK"),
                vec![int(d.amount), int(d.blocked)],
            )
        } else if d.absorbed != 0 {
            (
                pick("SPELL_ABSORB", "ABSORB"),
                vec![int(d.amount), int(d.absorbed)],
            )
        } else if d.critical {
            (
                pick("SPELL_DAMAGE_CRIT", "DAMAGE_CRIT"),
                vec![int(d.amount)],
            )
        } else if event == CombatLogEvent::DamageShield {
            ("DAMAGE_SHIELD", vec![int(d.amount)])
        } else {
            (pick("SPELL_DAMAGE", "DAMAGE"), vec![int(d.amount)])
        };
        self.out.center(ty, args);
    }

    /// The spell-damage entry (`0x751c40`): a landed hit, else its absorbed, blocked or
    /// resisted miss.
    pub(crate) fn spell_damage(
        &mut self,
        source: u64,
        dest: u64,
        spell: u32,
        damage: u32,
        overkill: u32,
        school: u32,
        absorbed: u32,
        resisted: u32,
        blocked: u32,
        crit: bool,
        periodic: bool,
    ) {
        let Some(ranged) = self
            .logged(spell)
            .map(|d| d.attributes_ex3 & EX3_RANGED != 0)
        else {
            return;
        };
        if damage != 0 {
            let event = if periodic {
                CombatLogEvent::SpellPeriodicDamage
            } else if ranged {
                CombatLogEvent::RangeDamage
            } else {
                CombatLogEvent::SpellDamage
            };
            let d = Damage {
                amount: damage as i32,
                overkill: overkill as i32,
                school,
                resisted: resisted as i32,
                blocked: blocked as i32,
                absorbed: absorbed as i32,
                critical: crit,
                ..Damage::default()
            };
            self.damage_event(event, source, dest, Some(spell), d);
            return;
        }
        let event = if periodic {
            CombatLogEvent::SpellPeriodicMissed
        } else if ranged {
            CombatLogEvent::RangeMissed
        } else {
            CombatLogEvent::SpellMissed
        };
        let (kind, amount, spell_word, word) = if absorbed != 0 {
            (MissType::Absorb, absorbed, "SPELL_ABSORB", "ABSORB")
        } else if blocked != 0 {
            (MissType::Block, blocked, "SPELL_BLOCK", "BLOCK")
        } else if resisted != 0 {
            (MissType::Resist, resisted, "SPELL_RESIST", "RESIST")
        } else {
            return;
        };
        if self.ctx.is_text_unit(dest) {
            let ty = if event == CombatLogEvent::SpellMissed {
                spell_word
            } else {
                word
            };
            self.out.center(ty, Vec::new());
        }
        self.queue(
            event,
            source,
            dest,
            Some(spell),
            CombatLogInfo {
                miss: Some(entries::miss(kind, amount as i32)),
                ..CombatLogInfo::default()
            },
        );
    }

    /// The split-damage entry (`0x752de0`): `DAMAGE_SPLIT`, then `SPLIT_DAMAGE` over the player.
    pub(crate) fn split_damage(
        &mut self,
        source: u64,
        dest: u64,
        spell: u32,
        damage: u32,
        overkill: u32,
        school: u32,
        absorbed: u32,
        resisted: u32,
        blocked: u32,
        crit: bool,
    ) {
        if self.logged(spell).is_none() {
            return;
        }
        let d = Damage {
            amount: damage as i32,
            overkill: overkill as i32,
            school,
            resisted: resisted as i32,
            blocked: blocked as i32,
            absorbed: absorbed as i32,
            critical: crit,
            ..Damage::default()
        };
        self.damage_event(CombatLogEvent::DamageSplit, source, dest, Some(spell), d);
        if self.ctx.is_text_unit(dest) {
            self.out.center("SPLIT_DAMAGE", vec![int(damage as i32)]);
        }
    }

    /// `SMSG_SPELLNONMELEEDAMAGELOG` (`0x6d3840`).
    pub(crate) fn spell_damage_log(&mut self, s: &benilla_protocol::messages::SpellDamageLog) {
        use benilla_protocol::messages::spell_hit_info as h;
        let crit = s.hit_info & h::CRIT != 0;
        let flags = (if crit { 0x200 } else { 0 })
            | (if s.absorbed != 0 { 0x20 } else { 0 })
            | (if s.blocked != 0 { 0x2000 } else { 0 })
            | (if s.resisted != 0 { 0x80 } else { 0 });
        let school = u32::from(s.school);
        let feedback = self
            .ctx
            .spell(s.spell_id)
            .is_some_and(|d| d.attributes & ATTR_NO_FEEDBACK == 0);
        if s.hit_info & h::SPLIT != 0 {
            self.split_damage(
                s.caster, s.target, s.spell_id, s.damage, s.overkill, school, s.absorbed,
                s.resisted, s.blocked, crit,
            );
            if feedback {
                self.combat_feedback(s.target, s.damage, school, flags);
            }
            return;
        }
        let source = if s.hit_info & h::SOURCE_IS_TARGET != 0 {
            s.target
        } else {
            s.caster
        };
        self.spell_damage(
            source, s.target, s.spell_id, s.damage, s.overkill, school, s.absorbed, s.resisted,
            s.blocked, crit, s.periodic,
        );
        if !feedback {
            return;
        }
        self.combat_feedback(s.target, s.damage, school, flags);
        if s.damage != 0 {
            self.world_damage(s.target, s.caster, s.damage, Some(s.spell_id), crit, false);
        } else if s.absorbed != 0 {
            self.world_word(s.target, s.caster, 10, s.spell_id, false);
        } else if s.blocked != 0 {
            self.world_word(s.target, s.caster, 5, s.spell_id, false);
        } else if s.resisted != 0 {
            self.world_word(s.target, s.caster, 2, s.spell_id, false);
        }
    }

    /// `SMSG_PERIODICAURALOG` (`0x7531f0`): nothing without the record or a streamed target.
    pub(crate) fn periodic_aura_log(&mut self, l: &benilla_protocol::messages::PeriodicAuraLog) {
        use benilla_protocol::messages::PeriodicTick as T;
        let Some(feedback) = self
            .ctx
            .spell(l.spell_id)
            .map(|d| d.attributes & ATTR_NO_FEEDBACK == 0)
        else {
            return;
        };
        if !self.ctx.unit(l.target) {
            return;
        }
        let (target, caster, spell) = (l.target, l.caster, l.spell_id);
        for &tick in &l.ticks {
            match tick {
                T::Damage {
                    amount,
                    overkill,
                    school,
                    absorbed,
                    resisted,
                    crit,
                } => {
                    if feedback {
                        if amount != 0 {
                            self.world_damage(target, caster, amount, Some(spell), crit, true);
                        } else if absorbed != 0 {
                            self.world_word(target, caster, 10, spell, true);
                        } else if resisted != 0 {
                            self.world_word(target, caster, 2, spell, true);
                        }
                        let flags = (if absorbed != 0 { 0x20 } else { 0 })
                            | (if resisted != 0 { 0x80 } else { 0 })
                            | (if crit { 0x200 } else { 0 });
                        self.combat_feedback(target, amount, school, flags);
                    }
                    self.spell_damage(
                        caster, target, spell, amount, overkill, school, absorbed, resisted, 0,
                        crit, true,
                    );
                }
                T::Heal {
                    amount,
                    overheal,
                    absorbed,
                    crit,
                } => {
                    if feedback {
                        if amount == 0 && absorbed != 0 {
                            self.world_word(target, caster, 10, spell, true);
                        }
                        self.healing_feedback(target, amount, crit);
                    }
                    self.heal(
                        caster, target, spell, amount, overheal, absorbed, crit, true,
                    );
                }
                T::Energize { power, amount } => {
                    self.energize(caster, target, spell, power, amount, true);
                }
                T::Leech {
                    power,
                    amount,
                    multiplier,
                } => {
                    self.power_drain(caster, target, spell, power, amount, multiplier, true);
                }
            }
        }
    }

    /// The damage-shield entry (`0x752c90`) and the packet's tail (`0x6d3750`): the bearer
    /// strikes the attacker, whose number floats as a spell's.
    pub(crate) fn damage_shield(&mut self, s: &benilla_protocol::messages::DamageShield) {
        if self.logged(s.spell_id).is_some() {
            let d = Damage {
                amount: s.damage as i32,
                overkill: s.overkill as i32,
                school: s.school,
                ..Damage::default()
            };
            self.queue(
                CombatLogEvent::DamageShield,
                s.victim,
                s.attacker,
                Some(s.spell_id),
                CombatLogInfo {
                    damage: Some(d),
                    ..CombatLogInfo::default()
                },
            );
            if self.ctx.is_text_unit(s.attacker) {
                self.out.center("DAMAGE_SHIELD", vec![int(s.damage as i32)]);
            }
        }
        self.world_damage(s.attacker, s.victim, s.damage, None, false, false);
        self.combat_feedback(s.attacker, s.damage, s.school, 0);
    }

    /// The environmental entry (`0x751150`) behind `SMSG_ENVIRONMENTALDAMAGELOG` (`0x755270`): a
    /// streamed unit, some amount, a known type.
    pub(crate) fn environmental(&mut self, e: &benilla_protocol::messages::EnvironmentalDamageLog) {
        // `0xa2d394`: fatigue, drowning and falling are physical, lava and fire fire, slime nature.
        const SCHOOLS: [u32; 6] = [0, 0, 0, 2, 3, 2];
        let Some(&shift) = SCHOOLS.get(usize::from(e.damage_type)) else {
            return;
        };
        if !self.ctx.unit(e.victim) || (e.damage == 0 && e.absorb == 0 && e.resist == 0) {
            return;
        }
        let school = 1 << shift;
        let (damage, absorbed, resisted) = (e.damage as i32, e.absorb as i32, e.resist as i32);
        self.queue(
            CombatLogEvent::EnvironmentalDamage,
            0,
            e.victim,
            None,
            CombatLogInfo {
                environmental: Some(e.damage_type),
                damage: Some(Damage {
                    amount: damage,
                    school,
                    resisted,
                    absorbed,
                    ..Damage::default()
                }),
                ..CombatLogInfo::default()
            },
        );
        if self.ctx.is_text_unit(e.victim) {
            let (ty, args) = if resisted != 0 {
                ("RESIST", vec![int(damage), int(resisted)])
            } else if absorbed != 0 {
                ("ABSORB", vec![int(damage), int(absorbed)])
            } else {
                ("DAMAGE", vec![int(damage)])
            };
            self.out.center(ty, args);
        }
        if self.ctx.me() == Some(e.victim) {
            let flags =
                (if absorbed != 0 { 0x20 } else { 0 }) | (if resisted != 0 { 0x80 } else { 0 });
            self.combat_feedback(e.victim, e.damage, school, flags);
        }
    }

    // ── misses ───────────────────────────────────────────────────────────────────────────────

    /// The missed-spell text (`0x750500`): the miss entry, and its word over the player.
    fn missed_spell_text(
        &mut self,
        source: u64,
        dest: u64,
        event: CombatLogEvent,
        spell: u32,
        kind: MissType,
    ) {
        if self.ctx.is_text_unit(dest) {
            let spelled = event == CombatLogEvent::SpellMissed;
            let (spell_word, word) = match kind {
                MissType::Miss => ("SPELL_MISS", "MISS"),
                MissType::Resist => ("SPELL_RESIST", "RESIST"),
                MissType::Dodge => ("SPELL_DODGE", "DODGE"),
                MissType::Parry => ("SPELL_PARRY", "PARRY"),
                MissType::Block => ("SPELL_BLOCK", "BLOCK"),
                MissType::Evade => ("SPELL_EVADE", "EVADE"),
                MissType::Immune | MissType::Immune2 => ("SPELL_IMMUNE", "IMMUNE"),
                MissType::Deflect => ("SPELL_DEFLECT", "DEFLECT"),
                MissType::Absorb => ("SPELL_ABSORB", "ABSORB"),
                MissType::Reflect => ("SPELL_REFLECT", "REFLECT"),
            };
            self.out
                .center(if spelled { spell_word } else { word }, Vec::new());
        }
        self.queue(
            event,
            source,
            dest,
            Some(spell),
            CombatLogInfo {
                miss: Some(entries::miss(kind, 0)),
                ..CombatLogInfo::default()
            },
        );
    }

    /// The spell-missed entry (`0x751b80`): `shield` a damage shield's miss, `periodic` a
    /// periodic immunity, else `SPELL_MISSED` or `RANGE_MISSED`.
    pub(crate) fn spell_missed(
        &mut self,
        source: u64,
        dest: u64,
        spell: u32,
        code: u8,
        shield: bool,
        periodic: bool,
    ) {
        let Some(kind) = MissType::from_code(code) else {
            return;
        };
        let Some(ranged) = self
            .logged(spell)
            .map(|d| d.attributes_ex3 & EX3_RANGED != 0)
        else {
            return;
        };
        let event = if periodic {
            CombatLogEvent::SpellPeriodicMissed
        } else if shield {
            CombatLogEvent::DamageShieldMissed
        } else if ranged {
            CombatLogEvent::RangeMissed
        } else {
            CombatLogEvent::SpellMissed
        };
        self.missed_spell_text(source, dest, event, spell, kind);
    }

    /// One missed target off a cast or a missile (`0x80e1b0`, `0x702fc0`): the word, the
    /// feedback and the entry.
    pub(crate) fn cast_missed(&mut self, source: u64, target: u64, spell: u32, code: u8) {
        let feedback = self
            .ctx
            .spell(spell)
            .is_some_and(|d| d.attributes & ATTR_NO_FEEDBACK == 0);
        if feedback {
            self.world_word(target, source, code, spell, false);
            self.miss_feedback(target, code);
        }
        self.spell_missed(source, target, spell, code, false, false);
    }

    /// `SMSG_SPELLLOGMISS` (`0x6d3090`): a damage shield's misses; a debug log logs nothing.
    pub(crate) fn spell_log_miss(&mut self, m: &benilla_protocol::messages::SpellLogMiss) {
        let shown = self
            .ctx
            .spell(m.spell_id)
            .is_some_and(|d| d.attributes & ATTR_NO_FEEDBACK == 0);
        if m.debug || !shown {
            return;
        }
        for &(target, code) in &m.misses {
            self.miss_feedback(target, code);
            self.spell_missed(m.caster, target, m.spell_id, code, true, false);
            self.world_word(target, m.caster, code, m.spell_id, false);
        }
    }

    /// `SMSG_PROCRESIST` (`0x750d40`): a resist with a zero amount.
    pub(crate) fn proc_resist(&mut self, o: &benilla_protocol::messages::SpellOutcomeLog) {
        if self.logged(o.spell_id).is_none() {
            return;
        }
        self.queue(
            CombatLogEvent::SpellMissed,
            o.caster,
            o.target,
            Some(o.spell_id),
            CombatLogInfo {
                miss: Some(entries::miss(MissType::Resist, 0)),
                ..CombatLogInfo::default()
            },
        );
        if self.ctx.is_text_unit(o.target) {
            self.out.center("SPELL_RESIST", Vec::new());
        }
    }

    // ── heals and power ──────────────────────────────────────────────────────────────────────

    /// The heal entry (`0x751f70`) and the heal event (`0x750860`).
    pub(crate) fn heal(
        &mut self,
        source: u64,
        dest: u64,
        spell: u32,
        amount: u32,
        overheal: u32,
        absorbed: u32,
        crit: bool,
        periodic: bool,
    ) {
        if amount == 0 && overheal == 0 && absorbed == 0 {
            return;
        }
        let Some(long) = self
            .logged(spell)
            .map(|d| periodic && self.ctx.long_duration(d, source))
        else {
            return;
        };
        let event = if periodic {
            CombatLogEvent::SpellPeriodicHeal
        } else {
            CombatLogEvent::SpellHeal
        };
        self.queue(
            event,
            source,
            dest,
            Some(spell),
            CombatLogInfo {
                heal: Some(Heal {
                    amount: amount as i32,
                    overheal: overheal as i32,
                    absorbed: absorbed as i32,
                    critical: crit,
                }),
                ..CombatLogInfo::default()
            },
        );
        if long {
            return;
        }
        if self.ctx.is_text_unit(dest) {
            let name = text(self.ctx.name_of(source));
            let (ty, args) = match (absorbed != 0, periodic, crit) {
                (true, true, _) => (
                    "PERIODIC_HEAL_ABSORB",
                    vec![name, int(amount as i32), int(absorbed as i32)],
                ),
                (true, false, true) => (
                    "HEAL_CRIT_ABSORB",
                    vec![name, int(amount as i32), int(absorbed as i32)],
                ),
                (true, false, false) => (
                    "HEAL_ABSORB",
                    vec![name, int(amount as i32), int(absorbed as i32)],
                ),
                (false, true, _) => ("PERIODIC_HEAL", vec![name, int(amount as i32)]),
                (false, false, true) => ("HEAL_CRIT", vec![name, int(amount as i32)]),
                (false, false, false) => ("HEAL", vec![name, int(amount as i32)]),
            };
            self.out.center(ty, args);
        } else {
            self.world_heal(dest, source, amount, crit);
        }
    }

    /// `SMSG_SPELLHEALLOG` (`0x6d3c30`).
    pub(crate) fn spell_heal_log(&mut self, h: &benilla_protocol::messages::SpellHealLog) {
        let Some((feedback, periodic)) = self.ctx.spell(h.spell_id).map(|d| {
            (
                d.attributes & ATTR_NO_FEEDBACK == 0,
                d.attributes_ex3 & EX3_PERIODIC_LOG != 0,
            )
        }) else {
            return;
        };
        if feedback {
            if h.amount == 0 && h.absorbed != 0 {
                self.world_word(h.target, h.caster, 10, h.spell_id, true);
            }
            self.healing_feedback(h.target, h.amount, h.crit);
        }
        self.heal(
            h.caster, h.target, h.spell_id, h.amount, h.overheal, h.absorbed, h.crit, periodic,
        );
    }

    /// The energize entry (`0x752040`) and the energize event (`0x750a90`): the shown amount, and
    /// the power gain over the player.
    pub(crate) fn energize(
        &mut self,
        source: u64,
        dest: u64,
        spell: u32,
        power: u32,
        amount: u32,
        periodic: bool,
    ) {
        if amount == 0 || self.logged(spell).is_none() {
            return;
        }
        let shown = (amount / power_display_scale(power)) as i32;
        let event = if periodic {
            CombatLogEvent::SpellPeriodicEnergize
        } else {
            CombatLogEvent::SpellEnergize
        };
        self.queue(
            event,
            source,
            dest,
            Some(spell),
            CombatLogInfo {
                energize: Some((shown, power as i32)),
                ..CombatLogInfo::default()
            },
        );
        if self.ctx.is_text_unit(dest) {
            let ty = if periodic {
                "PERIODIC_ENERGIZE"
            } else {
                "ENERGIZE"
            };
            self.out
                .center(ty, vec![int(shown), text(power_word(power))]);
        }
    }

    /// `SMSG_SPELLENERGIZELOG` (`0x6d3df0`).
    pub(crate) fn spell_energize_log(&mut self, e: &benilla_protocol::messages::SpellEnergizeLog) {
        let Some(periodic) = self
            .ctx
            .spell(e.spell_id)
            .map(|d| d.attributes_ex3 & EX3_PERIODIC_LOG != 0)
        else {
            return;
        };
        self.energize(e.caster, e.target, e.spell_id, e.power, e.amount, periodic);
    }

    /// The power-drain entry and its queueing (`0x752100`, `0x750b60`): a happiness drain is a pet
    /// line instead.
    pub(crate) fn power_drain(
        &mut self,
        source: u64,
        dest: u64,
        spell: u32,
        power: u32,
        amount: u32,
        multiplier: f32,
        periodic: bool,
    ) {
        if amount == 0 {
            return;
        }
        if power == 4 {
            let shown = amount / power_display_scale(4);
            super::chat::happiness_drain(&mut self.out.chat, self.ctx.me(), source, dest, shown);
            return;
        }
        if self.logged(spell).is_none() {
            return;
        }
        let extra = (amount as f32 * multiplier) as i32;
        let event = match (extra != 0, periodic) {
            (true, true) => CombatLogEvent::SpellPeriodicLeech,
            (true, false) => CombatLogEvent::SpellLeech,
            (false, true) => CombatLogEvent::SpellPeriodicDrain,
            (false, false) => CombatLogEvent::SpellDrain,
        };
        let scale = power_display_scale(power) as i32;
        let (value, extra) = (amount as i32 / scale, extra / scale);
        self.queue(
            event,
            source,
            dest,
            Some(spell),
            CombatLogInfo {
                drain: Some(Drain {
                    amount: value,
                    power: power as i32,
                    extra,
                }),
                ..CombatLogInfo::default()
            },
        );
        if self.ctx.is_text_unit(dest) && value != 0 {
            self.out
                .center("ENERGIZE", vec![int(-value), text(power_word(power))]);
        }
        if self.ctx.is_text_unit(source) && extra != 0 {
            let ty = if event == CombatLogEvent::SpellPeriodicLeech {
                "PERIODIC_ENERGIZE"
            } else {
                "ENERGIZE"
            };
            self.out
                .center(ty, vec![int(extra), text(power_word(power))]);
        }
    }

    // ── kills ────────────────────────────────────────────────────────────────────────────────

    /// `SMSG_PARTYKILLLOG` (`0x750c90`).
    pub(crate) fn party_kill(&mut self, k: &benilla_protocol::messages::PartyKillLog) {
        self.queue(
            CombatLogEvent::PartyKill,
            k.killer,
            k.victim,
            None,
            CombatLogInfo::default(),
        );
    }

    /// The instakill entry (`0x752210`).
    pub(crate) fn instakill(&mut self, k: &benilla_protocol::messages::SpellInstaKillLog) {
        if self.logged(k.spell_id).is_some() {
            self.queue(
                CombatLogEvent::SpellInstakill,
                k.caster,
                k.victim,
                Some(k.spell_id),
                CombatLogInfo::default(),
            );
        }
    }

    /// The unit-dead entry (`0x752ed0`) from the death animation (`0x7561e0`): a creature that
    /// does not log its death (`0x718a90`: type flag `0x400`, or no template yet) logs nothing,
    /// and a gas cloud dissipates.
    pub(crate) fn unit_dead(&mut self, guid: u64) {
        let Some(store) = self.ctx.store(guid) else {
            return;
        };
        let is_player =
            store.0.object_type() == Some(benilla_protocol::messages::ObjectType::Player);
        let record = store
            .0
            .object_entry()
            .and_then(|e| self.ctx.names.creature_record(e));
        if !is_player && record.is_none_or(|r| r.type_flags & 0x400 != 0) {
            return;
        }
        // 12340 tests the summon slot only when the spell lookup fails (`0x752f3e`), so a summon
        // never reads `UNIT_DESTROYED`.
        const GAS_CLOUD: u32 = 13;
        let event = if record.is_some_and(|r| r.creature_type == GAS_CLOUD) {
            CombatLogEvent::UnitDissipates
        } else {
            CombatLogEvent::UnitDied
        };
        self.queue(event, 0, guid, None, CombatLogInfo::default());
    }

    // ── casts ────────────────────────────────────────────────────────────────────────────────

    /// The cast-start entry (`0x751920`): a streamed caster's spell with a cast time.
    pub(crate) fn cast_start(&mut self, caster: u64, spell: u32) {
        if !self.ctx.unit(caster) {
            return;
        }
        let logs = self
            .ctx
            .spell(spell)
            .is_some_and(|d| !hidden_in_log(d) && self.ctx.has_cast_time(caster, d));
        if logs {
            self.queue(
                CombatLogEvent::SpellCastStart,
                caster,
                0,
                Some(spell),
                CombatLogInfo::default(),
            );
        }
    }

    /// The cast-success entry (`0x7519e0`): an instant spell's success, and its name over the
    /// player for an `AttributesExF & 0x40` spell.
    pub(crate) fn cast_go(&mut self, caster: u64, target: u64, spell: u32) {
        if !self.ctx.unit(caster) {
            return;
        }
        let Some(d) = self.ctx.spell(spell) else {
            return;
        };
        if hidden_in_log(d) || d.effects.contains(&33) {
            return;
        }
        let instant = !self.ctx.has_cast_time(caster, d);
        let cast_text = (d.attributes_ex6 & EX6_CAST_TEXT != 0).then(|| d.name.clone());
        if instant {
            self.queue(
                CombatLogEvent::SpellCastSuccess,
                caster,
                target,
                Some(spell),
                CombatLogInfo::default(),
            );
        }
        if let (true, Some(name)) = (self.ctx.is_text_unit(caster), cast_text) {
            self.out.center("SPELL_CAST", vec![text(name)]);
        }
    }

    /// The proc entry (`0x74d700`): a hidden proc only floats its name over the player.
    pub(crate) fn proc_go(&mut self, caster: u64, spell: u32) {
        let name = self
            .ctx
            .spell(spell)
            .filter(|d| d.attributes_ex6 & EX6_CAST_TEXT != 0)
            .map(|d| d.name.clone());
        if let (true, Some(name)) = (self.ctx.is_text_unit(caster), name) {
            self.out.center("SPELL_CAST", vec![text(name)]);
        }
    }

    /// `SMSG_SPELL_GO`'s log (`0x80e1b0`): the success entry, then each miss and its reflect.
    pub(crate) fn spell_go(
        &mut self,
        caster: u64,
        item_caster: Option<u64>,
        spell: u32,
        cast_flags: u32,
        target: u64,
        misses: &[(u64, u8)],
        reflects: &[(u64, u8)],
    ) {
        let Some(d) = self.ctx.spell(spell) else {
            return;
        };
        let (log_hidden, proc_text) = (
            d.attributes_ex7 & 0x8000_0000 != 0,
            d.attributes_ex6 & EX6_CAST_TEXT != 0,
        );
        let inline_misses = d.speed == 0.0 && d.attributes_ex2 & 0x80 == 0;
        let hidden = cast_flags & 0x1 != 0;
        if !hidden || log_hidden {
            self.cast_go(caster, target, spell);
        } else if proc_text {
            self.proc_go(item_caster.unwrap_or(caster), spell);
        }
        if !inline_misses {
            return;
        }
        // The cast item, when it is a unit or game object, is the source.
        let source = item_caster
            .filter(|&g| {
                self.ctx.store(g).is_some_and(|s| {
                    s.is_unit()
                        || s.0.object_type()
                            == Some(benilla_protocol::messages::ObjectType::GameObject)
                })
            })
            .unwrap_or(caster);
        for &(victim, code) in misses {
            self.cast_missed(source, victim, spell, code);
            if code == MissType::Reflect as u8 {
                if let Some(&(_, result)) = reflects.iter().find(|(g, _)| *g == victim) {
                    if result != 0 {
                        self.cast_missed(source, caster, spell, result);
                    }
                }
            }
        }
    }

    /// The spell-failed entry (`0x751ad0`): the player's own failure, with the error shown.
    pub(crate) fn spell_fail(&mut self, spell: u32, reason: &str) {
        if reason.is_empty() || self.logged(spell).is_none() {
            return;
        }
        let Some(me) = self.ctx.me() else {
            return;
        };
        self.queue(
            CombatLogEvent::SpellCastFailed,
            me,
            0,
            Some(spell),
            CombatLogInfo {
                fail_reason: Some(reason.to_string()),
                ..CombatLogInfo::default()
            },
        );
    }

    // ── auras ────────────────────────────────────────────────────────────────────────────────

    /// The aura gates of `0x752710` and `0x752ba0`: logged, not passive, `AttributesExF & 0x400`
    /// clear.
    fn aura_logs(&self, spell: u32) -> Option<&SpellDisplay> {
        self.logged(spell).filter(|d| {
            d.attributes & ATTR_PASSIVE == 0 && d.attributes_ex6 & EX6_AURA_NOT_LOGGED == 0
        })
    }

    /// The aura applied-or-removed entry (`0x752710`): a dead unit's removals do not log.
    pub(crate) fn aura_added_or_removed(
        &mut self,
        unit: u64,
        caster: u64,
        spell: u32,
        harmful: bool,
        added: bool,
    ) {
        let alive = self
            .ctx
            .store(unit)
            .and_then(|s| s.0.unit_health())
            .is_some_and(|h| h > 0);
        if !alive && !added {
            return;
        }
        let Some(d) = self.aura_logs(spell) else {
            return;
        };
        let (name, start_text) = (d.name.clone(), d.attributes_ex6 & EX6_CAST_TEXT == 0);
        let event = if added {
            CombatLogEvent::SpellAuraApplied
        } else {
            CombatLogEvent::SpellAuraRemoved
        };
        self.queue(
            event,
            caster,
            unit,
            Some(spell),
            CombatLogInfo {
                harmful: Some(harmful),
                ..CombatLogInfo::default()
            },
        );
        if added {
            self.spell_mechanic_text(unit, caster, spell);
        }
        if !self.ctx.is_text_unit(unit) {
            return;
        }
        let ty = match (added, harmful) {
            (true, true) => "SPELL_AURA_START_HARMFUL",
            (true, false) if start_text => "SPELL_AURA_START",
            (true, false) => return,
            (false, true) => "SPELL_AURA_END_HARMFUL",
            (false, false) => "SPELL_AURA_END",
        };
        self.out.center(ty, vec![text(name)]);
    }

    /// The aura-dose entry (`0x752860`): a stacking aura's count moved.
    pub(crate) fn aura_dose(
        &mut self,
        unit: u64,
        caster: u64,
        spell: u32,
        old: u8,
        new: u8,
        harmful: bool,
    ) {
        if !self.logged(spell).is_some_and(|d| d.stack_amount > 1) {
            return;
        }
        let event = if new <= old {
            CombatLogEvent::SpellAuraRemovedDose
        } else {
            CombatLogEvent::SpellAuraAppliedDose
        };
        self.queue(
            event,
            caster,
            unit,
            Some(spell),
            CombatLogInfo {
                harmful: Some(harmful),
                dose: Some(i32::from(new)),
                ..CombatLogInfo::default()
            },
        );
    }

    /// The aura-refresh entry (`0x752ba0`): the same aura, a new expiry.
    pub(crate) fn aura_refreshed(&mut self, unit: u64, caster: u64, spell: u32, harmful: bool) {
        if self.aura_logs(spell).is_none() {
            return;
        }
        self.queue(
            CombatLogEvent::SpellAuraRefresh,
            caster,
            unit,
            Some(spell),
            CombatLogInfo {
                harmful: Some(harmful),
                ..CombatLogInfo::default()
            },
        );
        self.spell_mechanic_text(unit, caster, spell);
    }

    /// An aura's mechanic words (`0x74db80`): each distinct effect mechanic, then the spell's own
    /// when no effect names it; an open-lock spell and the interrupt mechanic float none.
    fn spell_mechanic_text(&mut self, unit: u64, caster: u64, spell: u32) {
        let Some(d) = self.ctx.spell(spell).filter(|d| logged(d)) else {
            return;
        };
        if d.effects.contains(&EFFECT_OPEN_LOCK) {
            return;
        }
        let mut ids: Vec<u32> = Vec::with_capacity(4);
        for &m in &d.effect_mechanic {
            if m != 0 && !ids.contains(&m) {
                ids.push(m);
            }
        }
        if d.mechanic != 0 && !d.effect_mechanic.contains(&d.mechanic) {
            ids.push(d.mechanic);
        }
        for m in ids.into_iter().filter(|&m| m != MECHANIC_INTERRUPT) {
            self.mechanic_effect(unit, caster, spell, m);
        }
    }

    /// `0x74da70`: the mechanic's state name, first letter upper-cased, floats over `unit`, with
    /// the caster's name after it unless the caster is the player. `0x74da00` gates it: the
    /// spell's `AttributesExF` bit, then `fctAllSpellMechanics`, or `fctSpellMechanics` with the
    /// player as the caster or `fctSpellMechanicsOther`.
    fn mechanic_effect(&mut self, unit: u64, caster: u64, spell: u32, mechanic: u32) {
        let flagged = self
            .ctx
            .spell(spell)
            .is_some_and(|d| d.attributes_ex6 & EX6_MECHANIC_TEXT != 0);
        let mine = self.ctx.is_text_unit(caster);
        let g = *self.out.gates;
        if !flagged || !(g.all_mechanics || g.mechanics && (mine || g.mechanics_other)) {
            return;
        }
        let Some(state) = self.ctx.mechanic_name(mechanic) else {
            return;
        };
        let mut chars = state.chars();
        let state: String = chars
            .next()
            .map(|c| c.to_uppercase().chain(chars).collect())
            .unwrap_or_default();
        let text = if mine {
            state
        } else {
            format!("{state} ({})", self.ctx.name_of(caster))
        };
        let Some(anchor) = self.ctx.entity(unit).filter(|_| self.ctx.unit(unit)) else {
            return;
        };
        if benilla_assets::trace::enabled() {
            benilla_assets::trace::line(
                "fct",
                &format!("mechanic {text:?} over {unit:#x} from {caster:#x}"),
            );
        }
        self.out.text.write(CombatTextSpawn {
            anchor,
            text,
            category: 0,
            color: Some(crate::combat_text::COLOR_MECHANIC),
        });
    }

    /// `SMSG_SPELLBREAKLOG`, `SMSG_SPELLDISPELLOG` and `SMSG_SPELLSTEALLOG` (`0x6ce2b0`): a break
    /// removes, a dispel or steal replaces.
    pub(crate) fn spell_remove_log(&mut self, l: &benilla_protocol::messages::SpellRemoveLog) {
        use benilla_protocol::messages::SpellRemoveKind as K;
        for &(aura, harmful) in &l.auras {
            match l.kind {
                K::Break => self.aura_broken(l.caster, l.target, l.spell_id, aura, harmful),
                K::Dispel | K::Steal => self.aura_replaced(
                    l.caster,
                    l.target,
                    l.spell_id,
                    aura,
                    harmful,
                    l.kind == K::Steal,
                ),
            }
        }
    }

    /// The aura-replaced entry (`0x752930`): `SPELL_DISPEL` or `SPELL_STOLEN`, the removed aura as
    /// the second spell.
    fn aura_replaced(
        &mut self,
        source: u64,
        dest: u64,
        spell: u32,
        aura: u32,
        harmful: bool,
        stolen: bool,
    ) {
        if self.logged(spell).is_none() {
            return;
        }
        let Some(name) = self.logged(aura).map(|d| d.name.clone()) else {
            return;
        };
        let extra = self.extra_spell(aura);
        let event = if stolen {
            CombatLogEvent::SpellStolen
        } else {
            CombatLogEvent::SpellDispel
        };
        self.queue(
            event,
            source,
            dest,
            Some(spell),
            CombatLogInfo {
                extra_spell: Some(extra),
                harmful: Some(harmful),
                ..CombatLogInfo::default()
            },
        );
        if self.ctx.is_text_unit(dest) {
            self.out.center("SPELL_DISPELLED", vec![text(name)]);
        }
    }

    /// The aura-removed entry (`0x752a40`): `SPELL_AURA_BROKEN`, or `_BROKEN_SPELL` naming the
    /// breaking spell; a breaker spell with `AttributesExD & 0x2000` names no source.
    fn aura_broken(&mut self, source: u64, dest: u64, breaker: u32, aura: u32, harmful: bool) {
        let mut remover = source;
        if breaker != 0 {
            let Some(d) = self.logged(breaker) else {
                return;
            };
            if d.attributes_ex4 & EX4_NO_SOURCE != 0 {
                remover = 0;
            }
        }
        let Some(name) = self.logged(aura).map(|d| d.name.clone()) else {
            return;
        };
        let (event, extra_spell) = if breaker != 0 {
            (
                CombatLogEvent::SpellAuraBrokenSpell,
                Some(self.extra_spell(breaker)),
            )
        } else {
            (CombatLogEvent::SpellAuraBroken, None)
        };
        self.queue(
            event,
            remover,
            dest,
            Some(aura),
            CombatLogInfo {
                extra_spell,
                harmful: Some(harmful),
                ..CombatLogInfo::default()
            },
        );
        if self.ctx.is_text_unit(dest) {
            self.out.center("SPELL_DISPELLED", vec![text(name)]);
        }
    }

    /// A second spell as the `0x100` block names it.
    fn extra_spell(&self, id: u32) -> CombatLogSpell {
        let rec = self.ctx.spell(id);
        CombatLogSpell {
            id,
            name: rec.map(|d| d.name.clone()),
            school: rec.map_or(0, |d| d.school_mask),
        }
    }

    /// `SMSG_DISPEL_FAILED` (`0x750ea0`): one entry per aura that stayed.
    pub(crate) fn dispel_failed(&mut self, f: &benilla_protocol::messages::DispelFailed) {
        if self.logged(f.spell_id).is_none() {
            return;
        }
        for &aura in &f.failed {
            if self.logged(aura).is_none() {
                continue;
            }
            let extra = self.extra_spell(aura);
            self.queue(
                CombatLogEvent::SpellDispelFailed,
                f.caster,
                f.target,
                Some(f.spell_id),
                CombatLogInfo {
                    extra_spell: Some(extra),
                    ..CombatLogInfo::default()
                },
            );
        }
    }

    /// `SMSG_ENCHANTMENTLOG` (`0x751050`): a casterless log is the fade; an enchantment whose
    /// `Flags & 0x2` hides it logs nothing.
    pub(crate) fn enchantment(&mut self, e: &benilla_protocol::messages::EnchantmentLog) {
        let Some(enchants) = self.ctx.enchants.as_deref() else {
            return;
        };
        if !enchants.0.has_row(e.enchant_id) || enchants.0.tooltip_hides_name(e.enchant_id) {
            return;
        }
        let name = enchants.0.name(e.enchant_id).unwrap_or("").to_string();
        let event = if e.caster != 0 {
            CombatLogEvent::EnchantApplied
        } else {
            CombatLogEvent::EnchantRemoved
        };
        self.queue(
            event,
            e.caster,
            e.target,
            None,
            CombatLogInfo {
                enchant: Some(name),
                item: Some(CombatLogItem {
                    id: e.item_entry,
                    name: None,
                }),
                ..CombatLogInfo::default()
            },
        );
    }

    // ── spell effects ────────────────────────────────────────────────────────────────────────

    /// `SMSG_SPELLLOGEXECUTE` (`0x6d3220`): one producer per effect arm.
    pub(crate) fn spell_log_execute(&mut self, x: &benilla_protocol::messages::SpellLogExecute) {
        use benilla_protocol::messages::ExecuteLog as E;
        let Some(periodic) = self
            .ctx
            .spell(x.spell_id)
            .map(|d| d.attributes_ex3 & EX3_PERIODIC_LOG != 0)
        else {
            return;
        };
        let (caster, spell) = (x.caster, x.spell_id);
        for (_, rows) in &x.effects {
            for &row in rows {
                match row {
                    E::PowerDrain {
                        target,
                        amount,
                        power,
                        multiplier,
                    } => {
                        self.power_drain(caster, target, spell, power, amount, multiplier, periodic)
                    }
                    E::Resurrect { target } => {
                        self.simple(CombatLogEvent::SpellResurrect, caster, target, spell)
                    }
                    E::ExtraAttacks { target, count } => {
                        self.extra_attacks(caster, target, spell, count)
                    }
                    E::CreateItem { item_entry } => super::chat::tradeskill_create(
                        &mut self.out.chat,
                        self.ctx.me(),
                        caster,
                        item_entry,
                    ),
                    E::Summon { target } => {
                        let event = if (target >> 32) as u32 & 0xf0f0_0000 == 0xf010_0000 {
                            CombatLogEvent::SpellCreate
                        } else {
                            CombatLogEvent::SpellSummon
                        };
                        self.simple(event, caster, target, spell);
                    }
                    E::OpenLock { target } => self.open_lock(caster, target, spell),
                    E::InterruptCast { target, spell_id } => {
                        self.interrupt(caster, target, spell, spell_id)
                    }
                    E::FeedPet { item_entry } => {
                        super::chat::feed_pet(&mut self.out.chat, self.ctx.me(), caster, item_entry)
                    }
                    E::DismissPet { target } => {
                        super::chat::dismiss_pet(&mut self.out.chat, self.ctx.me(), caster, target)
                    }
                    E::DurabilityDamage {
                        target,
                        item_entry,
                        slot,
                    } => {
                        if item_entry == -1 && slot == -1 {
                            self.simple(
                                CombatLogEvent::SpellDurabilityDamageAll,
                                caster,
                                target,
                                spell,
                            );
                        } else if self.logged(spell).is_some() {
                            self.queue(
                                CombatLogEvent::SpellDurabilityDamage,
                                caster,
                                target,
                                Some(spell),
                                CombatLogInfo {
                                    item: Some(CombatLogItem {
                                        id: item_entry as u32,
                                        name: None,
                                    }),
                                    ..CombatLogInfo::default()
                                },
                            );
                        }
                    }
                }
            }
        }
    }

    /// The spell-only entries: the cast, resurrect and all-items durability entries (`0x7522a0`,
    /// `0x752340`, `0x752680`).
    fn simple(&mut self, event: CombatLogEvent, source: u64, dest: u64, spell: u32) {
        if self.logged(spell).is_some() {
            self.queue(event, source, dest, Some(spell), CombatLogInfo::default());
        }
    }

    /// The extra-attacks entry (`0x752520`): the extra-attack count, and `EXTRA_ATTACKS` over the
    /// player.
    fn extra_attacks(&mut self, source: u64, dest: u64, spell: u32, count: u32) {
        if count == 0 || self.logged(spell).is_none() {
            return;
        }
        self.queue(
            CombatLogEvent::SpellExtraAttacks,
            source,
            dest,
            Some(spell),
            CombatLogInfo {
                amount: Some(count as i32),
                ..CombatLogInfo::default()
            },
        );
        if self.ctx.is_text_unit(dest) {
            self.out.center("EXTRA_ATTACKS", vec![int(count as i32)]);
        }
    }

    /// The interrupt entry (`0x7523d0`): both spells must log; `INTERRUPT` over the player
    /// names the interrupted one.
    fn interrupt(&mut self, source: u64, dest: u64, spell: u32, interrupted: u32) {
        if self.logged(spell).is_none() {
            return;
        }
        let Some(name) = self.logged(interrupted).map(|d| d.name.clone()) else {
            return;
        };
        let extra = self.extra_spell(interrupted);
        self.queue(
            CombatLogEvent::SpellInterrupt,
            source,
            dest,
            Some(spell),
            CombatLogInfo {
                extra_spell: Some(extra),
                ..CombatLogInfo::default()
            },
        );
        if self.ctx.is_text_unit(dest) {
            self.out.center("INTERRUPT", vec![text(name)]);
        }
        self.mechanic_effect(dest, source, spell, MECHANIC_INTERRUPT);
    }

    /// The log's link info (`0x50ebe0`): an opened item or game object's chat line.
    fn open_lock(&mut self, source: u64, target: u64, spell: u32) {
        let Some(spell_name) = self.logged(spell).map(|d| d.name.clone()) else {
            return;
        };
        let Some(store) = self.ctx.store(target) else {
            return;
        };
        use benilla_protocol::messages::ObjectType as O;
        let opened = match store.0.object_type() {
            Some(O::Item | O::Container) => store.0.object_entry().map(super::chat::Opened::Item),
            // An uncached template prints nothing (`0x50deb0`).
            Some(O::GameObject) => self
                .ctx
                .objects
                .as_deref()
                .and_then(|o| o.get(target))
                .map(|t| super::chat::Opened::GameObject(t.name.clone())),
            _ => None,
        };
        if let Some(opened) = opened {
            super::chat::open_lock(
                &mut self.out.chat,
                self.ctx.me(),
                source,
                spell_name,
                opened,
            );
        }
    }

    /// `SMSG_DESTRUCTIBLE_BUILDING_DAMAGE` (`0x753730`): a hit (`0x753000`) or, at zero or below, a
    /// repair (`0x753100`).
    pub(crate) fn building_damage(&mut self, b: &benilla_protocol::messages::BuildingDamage) {
        let Some(school) = self.logged(b.spell_id).map(|d| d.school_mask) else {
            return;
        };
        let info = if b.damage <= 0 {
            CombatLogInfo {
                heal: Some(Heal {
                    amount: -b.damage,
                    ..Heal::default()
                }),
                ..CombatLogInfo::default()
            }
        } else {
            CombatLogInfo {
                damage: Some(Damage {
                    amount: b.damage,
                    school,
                    ..Damage::default()
                }),
                ..CombatLogInfo::default()
            }
        };
        let event = if b.damage <= 0 {
            CombatLogEvent::SpellBuildingHeal
        } else {
            CombatLogEvent::SpellBuildingDamage
        };
        self.queue(event, b.caster, b.building, Some(b.spell_id), info);
    }
}
