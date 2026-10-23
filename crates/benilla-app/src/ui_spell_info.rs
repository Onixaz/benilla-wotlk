//! `GetSpellInfo`'s answers (`0x540a30`): the app's spell tables and a snapshot of the casters,
//! the player and its pet, which the VM's [`SpellInfoSource`] reads at the call. The snapshot is
//! refreshed before the input pass, from the same fields and modifier tables the spell tooltip's
//! cells read, so the cost, cast time and range are the tooltip's numbers.

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use bevy::prelude::*;

use benilla_formats::{RangeTargets, RangeUnit, SkillLineCatalog};
use benilla_ui::script::{SpellInfo, SpellInfoSource, UiScript};

use crate::net::{ObjectStore, SelfPlayer};
use crate::spell::usable;
use crate::ui_action::Spells;
use crate::ui_script::VmMemo;
use crate::ui_unit::UnitFeed;

/// The book's weapon icons, which `GetSpellInfo` substitutes as `GetSpellTexture` does: the main
/// hand's for the melee auto-attack (`CGActionBar::GetAttackTexture`) and the ranged weapon's for
/// a ranged shot (`GetRangedTexture`), `None` without one.
#[derive(Resource, Clone, Default, PartialEq)]
pub(crate) struct WeaponActionIcons {
    pub(crate) attack: Option<String>,
    pub(crate) ranged: Option<String>,
}

/// One caster as the cost, cast and range terms read it.
#[derive(Clone)]
struct Caster {
    store: ObjectStore,
    /// The level the cost and cast terms scale by.
    level: u32,
    unit: RangeUnit,
}

#[derive(Default)]
struct Snapshot {
    player: Option<Caster>,
    pet: Option<Caster>,
    mods: Option<crate::spell::SpellModifiers>,
    icons: WeaponActionIcons,
}

struct Source {
    spells: Spells,
    skill_lines: Option<std::sync::Arc<SkillLineCatalog>>,
    snapshot: Arc<Mutex<Snapshot>>,
}

impl SpellInfoSource for Source {
    fn spell_info(&self, spell_id: u32, pet: bool) -> Option<SpellInfo> {
        let d = self.spells.catalog.get(spell_id)?;
        let snap = self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let caster = if pet {
            snap.pet.as_ref()
        } else {
            snap.player.as_ref()
        };
        let no_mods = crate::spell::SpellModifiers::default();
        let mods = snap.mods.as_ref().unwrap_or(&no_mods);
        let texture = if d.is_melee_auto_attack() {
            snap.icons.attack.clone()
        } else if d.ranged_icon_substitution() {
            snap.icons.ranged.clone()
        } else {
            Some(d.icon.clone().unwrap_or_default())
        };
        // `Spell_C_GetPowerCost` and `Spell_C_GetPowerCostPerSecond` (`0x7ff100`) over the shared
        // display divisor (`0x6e7130`); a positive per-second cost is the funnel's.
        let cost = caster.map_or(d.mana_cost, |c| {
            usable::power_cost_at(d, &c.store.0, c.level, mods)
        });
        let skill = match (pet, caster) {
            (false, Some(c)) => crate::spell::spell_skill_value(
                Some(&c.store),
                self.skill_lines.as_deref(),
                spell_id,
            ),
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
        // `GetCastTime(spell, pet, 0, 1)` as a dword, 0 for an `Attributes & 0x40` spell.
        let cast_ms = if d.attributes & 0x40 != 0 {
            0
        } else {
            let level = caster.map_or(0, |c| c.level);
            self.spells.cast_time_unclamped_ms(d, level, mods) as u32
        };
        // The range needs the player and the caster: a melee row is 0, 0; else `GetMinMaxRange`
        // with no target, and a minimum-range row's floor `5 + min` (`0x7ff400`).
        let range = snap.player.as_ref().and(caster).map(|c| {
            let row = self.spells.ranges.get(d.range_index);
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

pub(crate) struct UiSpellInfoPlugin;

impl Plugin for UiSpellInfoPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WeaponActionIcons>().add_systems(
            Update,
            // After the book feed, whose weapon icons it copies.
            feed_spell_info
                .in_set(UnitFeed)
                .after(crate::ui_spellbook::BookFeed),
        );
    }
}

/// The source's snapshot, kept across frames for the VM it was installed in.
#[derive(Default)]
struct Installed(Option<Arc<Mutex<Snapshot>>>);

#[allow(clippy::too_many_arguments)]
fn feed_spell_info(
    script: Option<NonSendMut<UiScript>>,
    spells: Option<Res<Spells>>,
    skill_lines: Option<Res<crate::ui_spellbook::SkillLines>>,
    mods: Res<crate::spell::SpellModifiers>,
    icons: Res<WeaponActionIcons>,
    self_q: Query<(Entity, Ref<ObjectStore>), With<SelfPlayer>>,
    pet_bar: Option<Res<crate::ui_pet::PetBar>>,
    index: Res<crate::net::GuidIndex>,
    stores: Query<Ref<ObjectStore>>,
    range_units: crate::spell::RangeUnits,
    mut memory: Local<VmMemo<Installed>>,
) {
    let (Some(mut script), Some(spells)) = (script, spells) else {
        return;
    };
    let (installed, vm_reset) = memory.get_reset(&script);
    let reinstall = vm_reset
        || installed.0.is_none()
        || spells.is_changed()
        || skill_lines.as_ref().is_some_and(|s| s.is_changed());
    if reinstall {
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        script.set_spell_info_source(Rc::new(Source {
            spells: spells.clone(),
            skill_lines: skill_lines.as_ref().map(|s| s.catalog.clone()),
            snapshot: snapshot.clone(),
        }));
        installed.0 = Some(snapshot);
    }
    let Some(snapshot) = installed.0.as_ref() else {
        return;
    };
    let mut snap = snapshot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if reinstall || mods.is_changed() {
        snap.mods = Some(mods.clone());
    }
    if reinstall || icons.is_changed() {
        snap.icons = icons.clone();
    }
    // The units' motion moves every frame; their fields only when their stores do.
    let caster = |entity: Entity, store: &Ref<ObjectStore>, old: Option<Caster>, level: u32| {
        let unit = range_units.unit(entity)?;
        let store = match old {
            Some(c) if !reinstall && !store.is_changed() => c.store,
            _ => ObjectStore::clone(store),
        };
        Some(Caster { store, level, unit })
    };
    snap.player = self_q.iter().next().and_then(|(entity, store)| {
        let level = store.0.unit_level().unwrap_or(0);
        caster(entity, &store, snap.player.take(), level)
    });
    let pet = pet_bar
        .as_ref()
        .map(|b| b.spells.pet_guid)
        .filter(|&g| g != 0)
        .and_then(|g| index.0.get(&g).copied())
        .and_then(|e| Some((e, stores.get(e).ok()?)));
    snap.pet = pet.and_then(|(entity, store)| {
        let level = store.0.unit_level().unwrap_or(0);
        caster(entity, &store, snap.pet.take(), level)
    });
}
