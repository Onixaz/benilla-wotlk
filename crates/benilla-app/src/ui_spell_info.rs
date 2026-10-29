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
    cast: CastNow,
}

/// The player's cast state `Spell_C_IsCurrentSpell` (`0x806030`) reads.
#[derive(Default)]
struct CastNow {
    /// The pending or running cast, or the queued strike (`0x805f30`).
    pending: Option<u32>,
    /// Auto-attacking (`[player+0xa20]`).
    attacking: bool,
    /// The open trade-skill line (`[0xc235a4]`); a linked book is never open here.
    trade_line: Option<u32>,
    race_class: (u8, u8),
    critter: Option<u64>,
    /// The player's auras, `(spell, wire flags)`.
    auras: Vec<(u32, u8)>,
    summons: benilla_formats::SummonPropertiesCatalog,
}

struct Source {
    spells: Spells,
    skill_lines: Option<std::sync::Arc<SkillLineCatalog>>,
    snapshot: Arc<Mutex<Snapshot>>,
}

/// `SPELL_EFFECT_ATTACK`, the melee auto-attack's first effect.
const EFFECT_ATTACK: u32 = 78;
/// `SPELL_AURA_MOD_SHAPESHIFT`.
const AURA_SHAPESHIFT: u32 = 36;

impl Source {
    /// `Spell_C_IsActiveSpell` (`0x802cb0`) for the player: a critter summon while its creature is
    /// out, a spell whose own cancelable aura (flag `0x10`) is up, or a mount while any `MOUNTED`
    /// aura rides its creature.
    fn is_active_spell(
        &self,
        spell_id: u32,
        d: &benilla_formats::SpellDisplay,
        now: &CastNow,
    ) -> bool {
        let critter =
            d.companion_kind(&now.summons) == Some(benilla_formats::CompanionKind::Critter);
        if d.active_icon.is_none() && d.companion_kind(&now.summons).is_none() {
            return false;
        }
        if critter {
            let entry = now.critter.map(|g| ((g >> 24) & 0x0fff_ffff) as i32);
            return entry == Some(d.effect_misc_value[0]);
        }
        if now.auras.iter().any(|&(id, flags)| {
            id == spell_id && flags & benilla_protocol::messages::AURA_UPDATE_FLAG_CANCELABLE != 0
        }) {
            return true;
        }
        d.effect_apply_aura[0] == benilla_formats::SPELL_AURA_MOUNTED
            && now.auras.iter().any(|&(id, _)| {
                self.spells.catalog.get(id).is_some_and(|a| {
                    a.effect_apply_aura[0] == benilla_formats::SPELL_AURA_MOUNTED
                        && a.effect_misc_value[0] == d.effect_misc_value[0]
                })
            })
    }
}

impl SpellInfoSource for Source {
    /// `Spell_C_IsCurrentSpell` (`0x806030`): no row is nil; the pending cast; the auto-attack
    /// while attacking; a trade-skill opener while its line's book is open; a critter, mount or
    /// shapeshift by [`Source::is_active_spell`].
    fn is_current_spell(&self, spell_id: u32) -> bool {
        let Some(d) = self.spells.catalog.get(spell_id) else {
            return false;
        };
        let snap = self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = &snap.cast;
        if now.pending == Some(spell_id) {
            return true;
        }
        if snap.player.is_none() {
            return false;
        }
        match d.effects[0] {
            EFFECT_ATTACK => return now.attacking,
            benilla_formats::SPELL_EFFECT_TRADE_SKILL => {
                let (race, class) = now.race_class;
                let line = self
                    .skill_lines
                    .as_deref()
                    .and_then(|c| c.spell_to_line(spell_id, race, class));
                return line.is_some() && line == now.trade_line;
            }
            _ => {}
        }
        if d.companion_kind(&now.summons).is_none()
            && !d.effect_apply_aura.contains(&AURA_SHAPESHIFT)
        {
            return false;
        }
        self.is_active_spell(spell_id, d, now)
    }

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
        // display divisor; a positive per-second cost is the funnel's.
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
    cast_now: CastNowInputs,
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
    cast_now.refresh(&mut snap.cast, reinstall);
}

/// The self player's store, running cast, attack lock and auras.
type SelfCastState = (
    &'static ObjectStore,
    Option<&'static crate::creature_anim::Casting>,
    Has<crate::creature_anim::Engaged>,
    Option<Ref<'static, crate::unit_auras::UnitAuras>>,
);

/// What [`CastNow`] reads off the world.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CastNowInputs<'w, 's> {
    pending: Res<'w, crate::spell::PendingCast>,
    queued_melee: Res<'w, crate::spell::QueuedMeleeSpell>,
    trade_skill: Option<Res<'w, crate::ui_tradeskill::TradeSkillOpen>>,
    summons: Res<'w, crate::spell::SummonSlots>,
    me: Query<'w, 's, SelfCastState, With<SelfPlayer>>,
}

impl CastNowInputs<'_, '_> {
    fn refresh(&self, cast: &mut CastNow, reinstall: bool) {
        let (store, casting, engaged, auras) = match self.me.iter().next() {
            Some((s, c, e, a)) => (Some(s), c, e, a),
            None => (None, None, false, None),
        };
        cast.pending = crate::spell::inflight(
            &self.pending,
            &self.queued_melee,
            casting.map(|c| c.spell_id),
            std::time::Instant::now(),
        )
        .map(|i| i.spell_id());
        cast.attacking = engaged;
        cast.trade_line = self.trade_skill.as_ref().and_then(|t| t.line);
        cast.race_class = crate::spell::race_class(store);
        cast.critter = store.and_then(|s| s.0.unit_critter());
        if reinstall || self.summons.is_changed() {
            cast.summons = self.summons.0.clone();
        }
        match auras {
            Some(auras) if reinstall || auras.is_changed() => {
                cast.auras = auras.iter().map(|a| (a.spell_id, a.flags)).collect();
            }
            Some(_) => {}
            None => cast.auras.clear(),
        }
    }
}
