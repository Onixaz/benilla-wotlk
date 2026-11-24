//! `GetSpellInfo`'s answers (`0x540a30`) and the spell reads beside it: the app's spell tables,
//! and the casters (the player and its pet) and the player's cast state as the call is made. The
//! live view answers through [`Answers`] over its own reads; before a view is lent (the UI load,
//! where 12340's `GetSpellInfo(id)` already reads the spell DBC) the source installed on the VM
//! answers from the tables alone. The cost, cast time and range are the spell tooltip's numbers,
//! from the same fields and modifier tables.

use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;

use bevy::prelude::*;

use benilla_formats::{
    RangeTargets, RangeUnit, SkillLineCatalog, SummonPropertiesCatalog, TalentCatalog,
};
use benilla_ui::script::{SpellInfo, SpellInfoSource, SpellStatEffect, SpellStatEffects, UiScript};

use crate::net::ObjectStore;
use crate::spell::{usable, SpellModifiers};
use crate::ui_action::Spells;

/// One caster as the cost, cast and range terms read it at the call.
pub(crate) struct Caster<'a> {
    pub(crate) guid: u64,
    pub(crate) pos: Vec3,
    pub(crate) store: &'a ObjectStore,
    /// The level the cost and cast terms scale by.
    pub(crate) level: u32,
    pub(crate) unit: RangeUnit,
}

/// The player's cast state the current-spell test (`0x806030`) reads, at the call.
pub(crate) struct CastNow<'a> {
    /// Every pending spell, the running cast and the queued strike (`0x805f30`).
    pub(crate) pending: Vec<u32>,
    /// The item entries whose uses are pending (`0x805fc0`).
    pub(crate) items: Vec<u32>,
    /// The auto-repeating spell (`0x7fe130`); benilla keeps the auto-ranged-combat spell
    /// (`0x7fe180`), Auto Shot, in the same record.
    pub(crate) auto_repeat: Option<u32>,
    /// Auto-attacking (`[player+0xa20]`).
    pub(crate) attacking: bool,
    /// The open trade-skill line (`[0xc235a4]`); a linked book is never open here.
    pub(crate) trade_line: Option<u32>,
    pub(crate) race_class: (u8, u8),
    pub(crate) critter: Option<u64>,
    /// The player's auras.
    pub(crate) auras: Option<&'a crate::unit_auras::UnitAuras>,
    /// The player's guid and known spells, a trade link's reads.
    pub(crate) guid: u64,
    pub(crate) known: &'a BTreeSet<u32>,
    pub(crate) summons: &'a SummonPropertiesCatalog,
}

/// No known spells, for a read with no action store.
pub(crate) static NO_SPELLS: BTreeSet<u32> = BTreeSet::new();

impl CastNow<'_> {
    /// No player: nothing pending, known or worn.
    pub(crate) fn none() -> CastNow<'static> {
        CastNow {
            pending: Vec::new(),
            items: Vec::new(),
            auto_repeat: None,
            attacking: false,
            trade_line: None,
            race_class: (0, 0),
            critter: None,
            auras: None,
            guid: 0,
            known: &NO_SPELLS,
            summons: &crate::ui_companion::EMPTY_SUMMONS,
        }
    }
}

/// The spells the current-spell test finds (`0x805f30`): the pending list alone, not the unit's cast state, so a held cast whose pending cast was released is not current.
pub(crate) fn pending_spells(
    pending: &crate::spell::PendingCast,
    queued_melee: &crate::spell::QueuedMeleeSpell,
    casting: Option<&crate::creature_anim::Casting>,
) -> Vec<u32> {
    let started = casting.filter(|c| c.listed).map(|c| c.spell_id);
    pending
        .records()
        .iter()
        .map(|r| r.spell_id)
        .chain(started)
        .chain(queued_melee.current())
        .collect()
}

/// What a spell answer reads of the world as the call is made.
pub(crate) trait CallReads {
    /// The player, or with `pet` its pet, `None` while it is not streamed.
    fn caster(&self, pet: bool) -> Option<Caster<'_>>;
    fn mods(&self) -> Option<&SpellModifiers>;
    /// The weapon face the action bar's attack read (`0x5a84d0`) picks, or with `ranged` its
    /// ranged read (`0x5a85e0`); `None` without one.
    fn weapon_icon(&self, ranged: bool) -> Option<String>;
    fn cast_now(&self) -> CastNow<'_>;
    /// A unit as the range check reads it, `None` before it streams.
    fn unit_facts(&self, guid: u64) -> Option<crate::spell::RangeFacts>;
    /// The usable walk with no target and the power check (`0x541680`), on the player or with
    /// `pet` its pet; `None` without the walk's state.
    fn spell_usable(&self, _spell_id: u32, _pet: bool) -> Option<(bool, bool)> {
        None
    }
}

/// The spell tables an answer reads.
#[derive(Clone, Copy)]
pub(crate) struct Tables<'a> {
    pub(crate) spells: &'a Spells,
    pub(crate) skill_lines: Option<&'a SkillLineCatalog>,
    pub(crate) talents: Option<&'a TalentCatalog>,
}

/// The VM's spell answers over `tables` and the world as `reads` sees it.
pub(crate) struct Answers<'a, R: ?Sized> {
    pub(crate) tables: Tables<'a>,
    pub(crate) reads: &'a R,
}

/// `SPELL_EFFECT_ATTACK`, the melee auto-attack's first effect.
const EFFECT_ATTACK: u32 = 78;
/// `SPELL_AURA_MOD_SHAPESHIFT`.
const AURA_SHAPESHIFT: u32 = 36;

/// The active-spell test (`0x802cb0`) for the player: a critter summon while its creature is out,
/// a spell whose own cancelable aura (flag `0x10`) is up, or a mount while any `MOUNTED` aura rides
/// its creature; a spell with no `ActiveIconID` that summons no companion is never active.
pub(crate) fn active_spell(
    spell_id: u32,
    d: &benilla_formats::SpellDisplay,
    now: &CastNow,
    catalog: &benilla_formats::SpellCatalog,
) -> bool {
    let kind = d.companion_kind(now.summons);
    if d.active_icon.is_none() && kind.is_none() {
        return false;
    }
    if kind == Some(benilla_formats::CompanionKind::Critter) {
        let entry = now.critter.map(|g| ((g >> 24) & 0x0fff_ffff) as i32);
        return entry == Some(d.effect_misc_value[0]);
    }
    let auras = || now.auras.into_iter().flat_map(|a| a.iter());
    if auras().any(|a| {
        a.spell_id == spell_id
            && a.flags & benilla_protocol::messages::AURA_UPDATE_FLAG_CANCELABLE != 0
    }) {
        return true;
    }
    d.effect_apply_aura[0] == benilla_formats::SPELL_AURA_MOUNTED
        && auras().any(|a| {
            catalog.get(a.spell_id).is_some_and(|m| {
                m.effect_apply_aura[0] == benilla_formats::SPELL_AURA_MOUNTED
                    && m.effect_misc_value[0] == d.effect_misc_value[0]
            })
        })
}

impl<R: CallReads + ?Sized> SpellInfoSource for Answers<'_, R> {
    fn spell_usable(&self, spell_id: u32, pet: bool) -> Option<(bool, bool)> {
        self.reads.spell_usable(spell_id, pet)
    }

    fn spell_has_range(&self, spell_id: u32, pet: bool) -> bool {
        let spells = self.tables.spells;
        let (Some(caster), Some(mods), Some(d)) = (
            self.reads.caster(pet),
            self.reads.mods(),
            spells.catalog.get(spell_id),
        ) else {
            return false;
        };
        let row = spells.ranges.get(d.range_index);
        mods.min_max_range(d, row, caster.unit, RangeTargets::default())
            .is_some_and(|(min, max)| min.abs() > f32::EPSILON || max.abs() > f32::EPSILON)
    }

    fn spell_in_range(&self, spell_id: u32, pet: bool, target: u64) -> Option<bool> {
        let spells = self.tables.spells;
        let caster = self.reads.caster(pet)?;
        let mods = self.reads.mods()?;
        let d = spells.catalog.get(spell_id)?;
        let pet_guid = if pet {
            Some(caster.guid)
        } else {
            self.reads.caster(true).map(|p| p.guid)
        };
        crate::spell::range_check(
            d,
            spells.ranges.get(d.range_index),
            mods,
            (caster.unit, caster.pos),
            target,
            pet_guid,
            |guid| self.reads.unit_facts(guid),
        )
    }

    fn spell_stat_effects(&self, spell_id: u32) -> Option<SpellStatEffects> {
        let d = self.tables.spells.catalog.get(spell_id)?;
        Some(SpellStatEffects {
            attributes: d.attributes,
            effects: std::array::from_fn(|i| SpellStatEffect {
                effect: d.effects[i],
                aura: d.effect_apply_aura[i],
                misc_value: d.effect_misc_value[i],
                base_points: d.effect_base_points[i],
            }),
            has_reagents: d.reagents.iter().any(|&(id, n)| id != 0 && n > 0),
            reagents: d.reagents,
            item_type: d.effect_item_type[0],
            helpful: d.is_helpful(),
            harmful: d.is_harmful(),
        })
    }

    fn is_auto_repeat_spell(&self, spell_id: u32) -> bool {
        self.reads.cast_now().auto_repeat == Some(spell_id)
    }

    fn is_current_item(&self, item_id: u32) -> bool {
        self.reads.cast_now().items.contains(&item_id)
    }

    fn talent_rank_spell(&self, talent_id: u32, rank: i32) -> Option<(u32, u32)> {
        self.tables
            .talents?
            .rank_spell(talent_id, usize::try_from(rank).ok()?)
    }

    /// `GetSpellLink` (`0x5408e0`): a row hidden by attributes `0x80` and `0x100` together has
    /// none; the trade link needs a linkable line and an active player (`0x5db060`).
    fn spell_link(
        &self,
        spell_id: u32,
        skill: &dyn Fn(u32) -> Option<(u32, u32)>,
    ) -> Option<(String, Option<String>)> {
        let d = self.tables.spells.catalog.get(spell_id)?;
        if d.attributes & 0x180 == 0x180 {
            return None;
        }
        let now = self.reads.cast_now();
        let (race, class) = now.race_class;
        let trade = self.tables.skill_lines.and_then(|lines| {
            let line = lines.spell_to_line(spell_id, race, class)?;
            crate::ui_tradeskill::list_link(
                d,
                line,
                lines,
                now.known,
                (race, class, now.guid),
                skill(line)?,
            )
        });
        Some((d.name.clone(), trade))
    }

    /// The current-spell test (`0x806030`): no row is nil; the pending cast; the auto-attack
    /// while attacking; a trade-skill opener while its line's book is open; a critter, mount or
    /// shapeshift by [`active_spell`].
    fn is_current_spell(&self, spell_id: u32) -> bool {
        let Some(d) = self.tables.spells.catalog.get(spell_id) else {
            return false;
        };
        let now = self.reads.cast_now();
        if now.pending.contains(&spell_id) {
            return true;
        }
        if self.reads.caster(false).is_none() {
            return false;
        }
        match d.effects[0] {
            EFFECT_ATTACK => return now.attacking,
            benilla_formats::SPELL_EFFECT_TRADE_SKILL => {
                let (race, class) = now.race_class;
                let line = self
                    .tables
                    .skill_lines
                    .and_then(|c| c.spell_to_line(spell_id, race, class));
                return line.is_some() && line == now.trade_line;
            }
            _ => {}
        }
        if d.companion_kind(now.summons).is_none()
            && !d.effect_apply_aura.contains(&AURA_SHAPESHIFT)
        {
            return false;
        }
        active_spell(spell_id, d, &now, &self.tables.spells.catalog)
    }

    fn spell_info(&self, spell_id: u32, pet: bool) -> Option<SpellInfo> {
        let spells = self.tables.spells;
        let d = spells.catalog.get(spell_id)?;
        let caster = self.reads.caster(pet);
        let no_mods = SpellModifiers::default();
        let mods = self.reads.mods().unwrap_or(&no_mods);
        let texture = if d.is_melee_auto_attack() {
            self.reads.weapon_icon(false)
        } else if d.ranged_icon_substitution() {
            self.reads.weapon_icon(true)
        } else {
            Some(d.icon.clone().unwrap_or_default())
        };
        // The power-cost and per-second-cost reads (`0x7ff100`) over the shared display divisor; a
        // positive per-second cost is the funnel's.
        let cost = caster.as_ref().map_or(d.mana_cost, |c| {
            usable::power_cost_at(d, &c.store.0, c.level, mods)
        });
        let skill = match (pet, caster.as_ref()) {
            (false, Some(c)) => {
                crate::spell::spell_skill_value(Some(c.store), self.tables.skill_lines, spell_id)
            }
            (true, Some(c)) => c.level.saturating_mul(5),
            _ => 0,
        };
        let level = d.skill_level(skill) as i32;
        let per_second = (d.mana_per_second as i32).wrapping_add(
            (d.mana_per_second_per_level as i32)
                .wrapping_mul(level.wrapping_sub(d.base_level as i32)),
        );
        let per_second = match mods.pct_of(d, crate::spell::OP_COST) {
            Some(pct) if per_second > 0 => per_second.wrapping_mul(pct) / 100,
            _ => per_second,
        };
        let div = benilla_protocol::messages::power_display_scale(d.power_type);
        let per_second = per_second / div as i32;
        let (cost, funnel) = if per_second <= 0 {
            (i64::from(cost / div), false)
        } else {
            (i64::from(per_second), true)
        };
        // The cast-time read, `(spell, pet, 0, 1)`, as a dword, 0 for an `Attributes & 0x40` spell.
        let cast_ms = if d.attributes & 0x40 != 0 {
            0
        } else {
            let level = caster.as_ref().map_or(0, |c| c.level);
            spells.cast_time_unclamped_ms(d, level, mods) as u32
        };
        // The range needs the player and the caster: a melee row is 0, 0; else the min/max range
        // read with no target, and a minimum-range row's floor `5 + min` (`0x7ff400`).
        let player_too = !pet || self.reads.caster(false).is_some();
        let range = caster.as_ref().filter(|_| player_too).map(|c| {
            let row = spells.ranges.get(d.range_index);
            if row.is_some_and(|r| r.is_melee()) {
                return (0.0, 0.0);
            }
            let targets = RangeTargets {
                target: None,
                attack_target: None,
            };
            let (mut min, max) = mods
                .min_max_range(d, row, c.unit, targets)
                .unwrap_or((0.0, 0.0));
            if let Some(r) = row.filter(|r| r.flags & 2 != 0) {
                min = 5.0 + r.pair(d.is_helpful()).0;
            }
            (min, max)
        });
        Some(SpellInfo {
            name: d.name.clone(),
            rank: d.rank.clone().unwrap_or_default(),
            texture,
            cost,
            funnel,
            power_type: i64::from(d.power_type as i32),
            cast_ms,
            range,
        })
    }
}

/// The source installed on the VM, for a call with no view lent: the tables alone, no caster.
struct Installed {
    spells: Spells,
    skill_lines: Option<Arc<SkillLineCatalog>>,
    talents: Option<Arc<TalentCatalog>>,
    mods: Option<SpellModifiers>,
}

impl CallReads for Installed {
    fn caster(&self, _: bool) -> Option<Caster<'_>> {
        None
    }

    fn mods(&self) -> Option<&SpellModifiers> {
        self.mods.as_ref()
    }

    fn weapon_icon(&self, _: bool) -> Option<String> {
        None
    }

    fn cast_now(&self) -> CastNow<'_> {
        CastNow::none()
    }

    fn unit_facts(&self, _: u64) -> Option<crate::spell::RangeFacts> {
        None
    }
}

impl Installed {
    fn answers(&self) -> Answers<'_, Self> {
        Answers {
            tables: Tables {
                spells: &self.spells,
                skill_lines: self.skill_lines.as_deref(),
                talents: self.talents.as_deref(),
            },
            reads: self,
        }
    }
}

impl SpellInfoSource for Installed {
    fn spell_info(&self, spell_id: u32, pet: bool) -> Option<SpellInfo> {
        self.answers().spell_info(spell_id, pet)
    }

    fn is_current_spell(&self, spell_id: u32) -> bool {
        self.answers().is_current_spell(spell_id)
    }

    fn spell_has_range(&self, spell_id: u32, pet: bool) -> bool {
        self.answers().spell_has_range(spell_id, pet)
    }

    fn spell_stat_effects(&self, spell_id: u32) -> Option<SpellStatEffects> {
        self.answers().spell_stat_effects(spell_id)
    }

    fn talent_rank_spell(&self, talent_id: u32, rank: i32) -> Option<(u32, u32)> {
        self.answers().talent_rank_spell(talent_id, rank)
    }

    fn spell_link(
        &self,
        spell_id: u32,
        skill: &dyn Fn(u32) -> Option<(u32, u32)>,
    ) -> Option<(String, Option<String>)> {
        self.answers().spell_link(spell_id, skill)
    }
}

/// 12340's `GetSpellInfo(id)` reads the spell DBC during the UI load, before any view is lent.
pub(crate) fn seed_spell_info_for_vm(world: &World, script: &mut UiScript) {
    let Some(spells) = world.get_resource::<Spells>() else {
        return;
    };
    script.set_spell_info_source(Rc::new(Installed {
        spells: spells.clone(),
        skill_lines: world
            .get_resource::<crate::ui_spellbook::SkillLines>()
            .map(|s| s.catalog.clone()),
        talents: world
            .get_resource::<crate::ui_talent::Talents>()
            .map(|t| t.catalog.clone()),
        mods: world.get_resource::<SpellModifiers>().cloned(),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creature_anim::Casting;

    /// The current-spell test reads the pending list alone (`0x805f30`): a held cast whose pending
    /// cast was released is not current.
    #[test]
    fn a_held_cast_is_current_only_while_listed() {
        for (listed, want) in [(true, vec![133]), (false, vec![])] {
            let casting = Casting {
                spell_id: 133,
                cast_count: 0,
                until: None,
                listed,
                hidden: false,
            };
            let got = pending_spells(
                &crate::spell::PendingCast::default(),
                &crate::spell::QueuedMeleeSpell::default(),
                Some(&casting),
            );
            assert_eq!(got, want);
        }
    }
}
