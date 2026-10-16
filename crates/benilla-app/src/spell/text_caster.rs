//! The active player as the spell-text parser reads it ([`benilla_formats::Caster`]): its known
//! spells, its auras, the expression operands off its fields ([`Operands`]), its power costs and
//! its cast speed.

use std::collections::BTreeSet;

use benilla_formats::{PlayerValue, SpellDisplay};
use bevy::ecs::change_detection::DetectChanges;

use crate::items::Items;
use crate::net::ObjectStore;
use crate::unit_auras::UnitAuras;

use super::SpellModifiers;

/// One player input the parser read, for a caller that rebuilds what read it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum CasterRead {
    Spell(u32),
    Aura(u32),
    Operand(PlayerValue),
    CastSpeed,
}

const OPERANDS: usize = PlayerValue::ALL.len();

/// Every [`PlayerValue`] as the player's fields give it, in [`PlayerValue::ALL`] order; all 0
/// without a player.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Operands([f32; OPERANDS]);

impl Default for Operands {
    fn default() -> Self {
        Self::ZERO
    }
}

impl Operands {
    /// No player's operands.
    pub(crate) const ZERO: Self = Self([0.0; OPERANDS]);

    pub(crate) fn of(store: Option<&ObjectStore>, items: &Items) -> Self {
        match store {
            Some(store) => Self(PlayerValue::ALL.map(|v| operand(store, items, v))),
            None => Self::default(),
        }
    }

    pub(crate) fn get(&self, v: PlayerValue) -> f32 {
        self.0[v.index()]
    }

    /// The operands that differ from `prev`, a bit per [`PlayerValue::index`].
    pub(crate) fn diff(&self, prev: &Self) -> u64 {
        self.0
            .iter()
            .zip(&prev.0)
            .enumerate()
            .fold(0, |mask, (i, (now, before))| {
                mask | u64::from(now.to_bits() != before.to_bits()) << i
            })
    }
}

/// The bit [`Operands::diff`] keeps `v` at.
pub(crate) fn operand_bit(v: PlayerValue) -> u64 {
    1 << v.index()
}

/// `IsTwoHanded` `0x714c80`'s weapon subclasses: two-handed axe, mace and sword, polearm, staff,
/// the second exotic, spear and fishing pole.
fn two_handed(class: u32, subclass: u32) -> bool {
    class == 2 && matches!(subclass, 1 | 5 | 6 | 8 | 10 | 12 | 17 | 20)
}

/// `GetVariableValue` `0x5782d0` for a value off the player's fields.
fn operand(store: &ObjectStore, items: &Items, v: PlayerValue) -> f32 {
    let p = &store.0;
    let stat = |i: u8| p.unit_stat(i).unwrap_or(0) as i32;
    let weapon = |hand: u8| {
        // The visible weapon of slot 15 + hand, and its first damage range when physical.
        p.player_visible_item_entry(15 + hand)
            .and_then(|entry| items.template_cached(entry))
            .filter(|t| t.dmg_type == 0)
    };
    let school_power = |s: u8| {
        p.player_mod_damage_done_pos(s).unwrap_or(0) + p.player_mod_damage_done_neg(s).unwrap_or(0)
    };
    let (armor, armor_pos, armor_neg) = (
        p.unit_resistance(0).unwrap_or(0),
        p.player_resistance_buff_pos(0).unwrap_or(0),
        p.player_resistance_buff_neg(0).unwrap_or(0),
    );
    match v {
        PlayerValue::Stat(i) => stat(i).max(0) as f32,
        PlayerValue::BaseStat(i) => {
            stat(i) as f32
                - p.player_posstat(i).unwrap_or(0) as f32
                - p.player_negstat(i).unwrap_or(0) as f32
        }
        PlayerValue::AttackPower => (p.unit_attack_power().unwrap_or(0) as f32).max(0.0),
        PlayerValue::AttackPowerTotal => {
            let (pos, neg) = p.unit_attack_power_mods();
            let sum = p.unit_attack_power().unwrap_or(0) + i32::from(pos) + i32::from(neg);
            ((p.unit_attack_power_multiplier().unwrap_or(0.0) + 1.0) * sum as f32).max(0.0)
        }
        PlayerValue::RangedAttackPower => {
            (p.unit_ranged_attack_power().unwrap_or(0) as f32).max(0.0)
        }
        PlayerValue::RangedAttackPowerTotal => {
            let (pos, neg) = p.unit_ranged_attack_power_mods();
            let sum = p.unit_ranged_attack_power().unwrap_or(0) + i32::from(pos) + i32::from(neg);
            ((p.unit_ranged_attack_power_multiplier().unwrap_or(0.0) + 1.0) * sum as f32).max(0.0)
        }
        PlayerValue::WeaponMin(hand) => weapon(hand).map_or(0.0, |t| t.dmg_min),
        PlayerValue::WeaponMax(hand) => weapon(hand).map_or(0.0, |t| t.dmg_max),
        PlayerValue::DamageMin(hand) => {
            let d = match hand {
                0 => p.unit_min_damage(),
                1 => p.unit_min_offhand_damage(),
                _ => p.unit_min_ranged_damage(),
            };
            d.unwrap_or(0.0).floor().max(1.0)
        }
        PlayerValue::DamageMax(hand) => {
            let d = match hand {
                0 => p.unit_max_damage(),
                1 => p.unit_max_offhand_damage(),
                _ => p.unit_max_ranged_damage(),
            };
            d.unwrap_or(0.0).ceil().max(1.0)
        }
        // `GetResistanceAndBuffs` school 0: the base less both buffs, the effective clamped.
        PlayerValue::BaseArmor => (armor - armor_pos - armor_neg) as f32,
        PlayerValue::Armor => armor.max(0) as f32,
        PlayerValue::AttackSpeed(hand) => {
            let ms = match hand {
                0 | 1 => p.unit_base_attack_time(hand),
                _ => p.unit_ranged_attack_time(),
            };
            (f64::from(ms.unwrap_or(0)) * 0.001) as f32
        }
        PlayerValue::Level => p.unit_level().unwrap_or(0) as i32 as f32,
        // The main hand's virtual item (`0x71f440`), hidden while a disarm holds a weapon.
        PlayerValue::Hands => {
            let main = p
                .player_visible_item_entry(15)
                .and_then(|entry| items.template_cached(entry));
            let held = main.filter(|t| !(crate::items::is_disarmed(store) && t.class == 2));
            if held.is_some_and(|t| two_handed(t.class, t.subclass)) {
                2.0
            } else {
                1.0
            }
        }
        PlayerValue::SpellPower => (2..=6).map(school_power).fold(school_power(1), i32::min) as f32,
        PlayerValue::SchoolSpellPower(s) => school_power(s) as f32,
        PlayerValue::HealingPower => p.player_mod_healing_done_pos().unwrap_or(0) as f32,
        // The multipliers the server starts at 1.0, read so before they stream.
        PlayerValue::SchoolDamagePct(s) => p.player_mod_damage_done_pct(s).unwrap_or(1.0),
        PlayerValue::HealingPct => p.player_mod_healing_pct().unwrap_or(1.0),
        PlayerValue::HealingDonePct => p.player_mod_healing_done_pct().unwrap_or(1.0),
    }
}

/// The active player for one parse.
pub(crate) struct TextCaster<'a> {
    pub(crate) store: &'a ObjectStore,
    /// The known spells (`SMSG_INITIAL_SPELLS` and its learns).
    pub(crate) known: &'a BTreeSet<u32>,
    pub(crate) auras: Option<&'a UnitAuras>,
    pub(crate) operands: &'a Operands,
    pub(crate) mods: &'a SpellModifiers,
    /// The player's skill in a spell's line, the per-level cost term's level.
    pub(crate) skill: &'a dyn Fn(u32) -> u32,
    /// The view's resolved power cost, the player's or its pet's.
    pub(crate) power_cost: &'a dyn Fn(&SpellDisplay) -> i32,
    /// Told of each read, for a caller that tracks what a parse depended on.
    pub(crate) on_read: &'a dyn Fn(CasterRead),
}

impl benilla_formats::Caster for TextCaster<'_> {
    fn knows_spell(&self, spell: u32) -> bool {
        (self.on_read)(CasterRead::Spell(spell));
        self.known.contains(&spell)
    }

    fn has_aura(&self, spell: u32) -> bool {
        (self.on_read)(CasterRead::Aura(spell));
        self.auras
            .is_some_and(|a| a.iter().any(|aura| aura.spell_id == spell))
    }

    fn value(&self, value: PlayerValue) -> f32 {
        (self.on_read)(CasterRead::Operand(value));
        self.operands.get(value)
    }

    fn power_cost(&self, d: &SpellDisplay) -> i32 {
        (self.power_cost)(d)
    }

    /// `Spell_C_GetPowerCostPerSecond` `0x7ff100`: the per-second cost plus its per-level term at
    /// the spell's level less `baseLevel`, then op 14's percentage alone when positive.
    fn power_cost_per_second(&self, d: &SpellDisplay) -> i32 {
        let level = d.skill_level((self.skill)(d.id)) as i32;
        let cost = (d.mana_per_second as i32).wrapping_add(
            (d.mana_per_second_per_level as i32)
                .wrapping_mul(level.wrapping_sub(d.base_level as i32)),
        );
        match self.mods.pct_of(d, super::OP_COST) {
            Some(pct) if cost > 0 => cost.wrapping_mul(pct) / 100,
            _ => cost,
        }
    }

    fn cast_speed(&self) -> f32 {
        (self.on_read)(CasterRead::CastSpeed);
        self.store.0.unit_mod_cast_speed().unwrap_or(1.0)
    }
}

/// The known set and the player's auras for a system that parses spell text, and whether either
/// moved since that system last ran.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CasterSources<'w, 's> {
    actions: Option<bevy::prelude::Res<'w, crate::ui_action::PlayerActions>>,
    auras: bevy::prelude::Query<
        'w,
        's,
        bevy::prelude::Ref<'static, UnitAuras>,
        bevy::prelude::With<crate::net::SelfPlayer>,
    >,
}

static NO_SPELLS: BTreeSet<u32> = BTreeSet::new();

impl CasterSources<'_, '_> {
    pub(crate) fn known(&self) -> &BTreeSet<u32> {
        self.actions.as_deref().map_or(&NO_SPELLS, |a| &a.spells)
    }

    pub(crate) fn auras(&self) -> Option<&UnitAuras> {
        self.auras.single().ok().map(|a| a.into_inner())
    }

    /// A learn, an unlearn or an aura change since the system last ran.
    pub(crate) fn changed(&self) -> bool {
        self.actions.as_ref().is_some_and(|a| a.is_changed())
            || self.auras.single().is_ok_and(|a| a.is_changed())
    }
}

/// A [`TextCaster`] that tracks nothing, its cost the player's own.
pub(crate) fn untracked<'a>(
    store: &'a ObjectStore,
    sources: (&'a BTreeSet<u32>, Option<&'a UnitAuras>),
    operands: &'a Operands,
    mods: &'a SpellModifiers,
    skill: &'a dyn Fn(u32) -> u32,
    power_cost: &'a dyn Fn(&SpellDisplay) -> i32,
) -> TextCaster<'a> {
    TextCaster {
        store,
        known: sources.0,
        auras: sources.1,
        operands,
        mods,
        skill,
        power_cost,
        on_read: &|_| {},
    }
}
