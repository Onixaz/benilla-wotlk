//! The pet action bar as its verbs read it at the call: the ten packed words of the last `SMSG_PET_SPELLS`, which the server owns whole, each resolved into a `PetActionView` when a verb asks (`0x5d4ec0`).

use std::time::Instant;

use bevy::prelude::*;

use benilla_protocol::messages::{
    PetActionEntry, PetSpells, PET_COMMAND_ATTACK, PET_COMMAND_DISMISS, PET_COMMAND_FOLLOW,
    PET_COMMAND_STAY, PET_REACT_AGGRESSIVE, PET_REACT_DEFENSIVE, PET_REACT_PASSIVE,
};
use benilla_ui::script::{PetActionView, PetBarLive};

use crate::target::UNIT_FLAG_POSSESSED;
use crate::ui_action::Spells;

use super::{PetBar, PetUnit};

/// What the reference's action usability test (`0x80b5d0`) reads for a pet spell slot besides the
/// pet; with any of it absent the walk is skipped and the slot reads usable.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PetSlotWalk<'w, 's> {
    objects: crate::net::Objects<'w, 's>,
    factions: Option<Res<'w, crate::target::Factions>>,
    reputations: Option<Res<'w, crate::net::Reputations>>,
    /// The group, for the reaction's party rung; a harness without it has no group.
    group: Option<Res<'w, crate::ui_party::GroupState>>,
    items: Option<Res<'w, crate::items::Items>>,
    commands: Option<Res<'w, crate::net::NetCommands>>,
    combo: Option<Res<'w, crate::spell::ComboPoints>>,
    spell_mods: Option<Res<'w, crate::spell::SpellModifiers>>,
    /// The map, for the walk's place legs.
    map: Option<Res<'w, benilla_world::world_map::CurrentMap>>,
}

/// The walk's inputs a VM holder writes, read through the lent view at the call.
#[derive(Clone, Copy)]
pub(crate) struct WalkLent<'a> {
    pub(crate) selection: Option<&'a crate::target::Selection>,
    /// Where the player stands, for the walk's place legs.
    pub(crate) place: Option<&'a crate::area::PlayerPlace>,
}

impl PetSlotWalk<'_, '_> {
    /// The usable walk with the pet as caster: no bags, its own cooldowns, the current target.
    #[allow(clippy::too_many_arguments)] // the walk's inputs, each read at the call
    fn usable(
        &self,
        lent: WalkLent,
        spell_id: u32,
        d: &benilla_formats::SpellDisplay,
        spells: Option<&Spells>,
        pet: &crate::net::ObjectStore,
        pet_unit: &PetUnit,
        cooldowns: &crate::spell::Cooldowns,
    ) -> bool {
        let (Some(spells), Some(reputations), Some(items), Some(commands), Some(spell_mods)) = (
            spells,
            self.reputations.as_deref(),
            self.items.as_deref(),
            self.commands.as_deref(),
            self.spell_mods.as_deref(),
        ) else {
            return true;
        };
        let target = lent.selection.and_then(|s| s.guid);
        let target_store = target.and_then(|guid| pet_unit.store(guid));
        let ctx = crate::spell::usable::UsableCtx {
            self_auras: None,
            target_auras: None,
            store: pet,
            target_store,
            factions: self.factions.as_deref(),
            reputations,
            party: crate::target::party_of(self.group.as_deref()),
            cooldowns,
            spell_mods,
            carried: &std::collections::HashMap::new(),
            // The pet is not the active player: no rune spell is usable on it (`0x80182d`).
            runes: None,
            combo_points: self
                .combo
                .as_ref()
                .map_or(0, |c| c.pet.on(0, target.unwrap_or(0))),
            // The auto-unshift check asks for the active player (`0x71b130`).
            auto_unshift: false,
            place: crate::spell::usable::UsablePlace::of(
                lent.place,
                self.map.as_deref().map(|m| m.0),
                None,
            ),
        };
        crate::spell::usable::spell_usable(
            spell_id,
            d,
            spells,
            &ctx,
            &self.objects,
            items,
            commands,
        )
        .0
    }
}

/// A command token's `(name, texture)` pair, both global names (`PetActionBarFrame.lua:98-104`):
/// keys from `GlobalStrings.lua:3029-3032`, textures from `PetActionBarFrame.lua:6-12`.
pub(super) fn command_token(action: u32) -> Option<(&'static str, &'static str)> {
    Some(match action {
        PET_COMMAND_STAY => ("PET_ACTION_WAIT", "PET_WAIT_TEXTURE"),
        PET_COMMAND_FOLLOW => ("PET_ACTION_FOLLOW", "PET_FOLLOW_TEXTURE"),
        PET_COMMAND_ATTACK => ("PET_ACTION_ATTACK", "PET_ATTACK_TEXTURE"),
        PET_COMMAND_DISMISS => ("PET_ACTION_DISMISS", "PET_DISMISS_TEXTURE"),
        _ => return None,
    })
}

/// A reaction token's pair: the `PET_MODE_*` keys (`GlobalStrings.lua:3045-3047`), naming the
/// pet's state, not the menu's imperative `PET_AGGRESSIVE` family, though enUS reads them alike.
pub(super) fn reaction_token(action: u32) -> Option<(&'static str, &'static str)> {
    Some(match action {
        PET_REACT_PASSIVE => ("PET_MODE_PASSIVE", "PET_PASSIVE_TEXTURE"),
        PET_REACT_DEFENSIVE => ("PET_MODE_DEFENSIVE", "PET_DEFENSIVE_TEXTURE"),
        PET_REACT_AGGRESSIVE => ("PET_MODE_AGGRESSIVE", "PET_AGGRESSIVE_TEXTURE"),
        _ => return None,
    })
}

/// One packed slot word as the bar draws it. `showing_active` is [`active_aura_press`]'s answer,
/// the one call that both `GetPetActionInfo` and `CastPetAction` make, so the button showing active
/// art is the one whose press cancels.
pub(super) fn slot_view(
    entry: PetActionEntry,
    bar: &PetSpells,
    spell: Option<&benilla_formats::SpellDisplay>,
    cooldown: Option<(i64, u32, bool)>,
    pet_attacking: bool,
    showing_active: bool,
) -> PetActionView {
    PetActionView {
        // Every slot carries its raw word, empty ones too: the drag's relocation candidate is
        // vmangos's unused slot, `ACT_DISABLED` with spell 0 (type 1, low 16 bits zero).
        packed: entry.packed,
        passive: spell.is_some_and(|s| s.passive),
        ..slot_paint(entry, bar, spell, cooldown, pet_attacking, showing_active)
    }
}

/// [`slot_view`]'s painted half: everything but the raw `packed` and `passive`.
pub(super) fn slot_paint(
    entry: PetActionEntry,
    bar: &PetSpells,
    spell: Option<&benilla_formats::SpellDisplay>,
    cooldown: Option<(i64, u32, bool)>,
    pet_attacking: bool,
    showing_active: bool,
) -> PetActionView {
    let kind = entry.kind();
    let action = entry.action();

    if let Some((name, texture)) = (kind == benilla_protocol::messages::PET_ACT_COMMAND)
        .then(|| command_token(action))
        .flatten()
    {
        // Attack is lit by the pet-attacking global alone (`0x5d51ea`); the others when the unmasked `state >> 8` equals the action (`0x5d51fc`), so a disabled bar lights none.
        let attacking = pet_attacking && action == PET_COMMAND_ATTACK;
        return PetActionView {
            name: Some(name.to_string()),
            texture: Some(texture.to_string()),
            is_token: true,
            active: if action == PET_COMMAND_ATTACK {
                pet_attacking
            } else {
                bar.command_state() == action
            },
            attack_active: attacking,
            ..Default::default()
        };
    }

    if let Some((name, texture)) = (kind == benilla_protocol::messages::PET_ACT_REACTION)
        .then(|| reaction_token(action))
        .flatten()
    {
        // A vehicle's bar reads as Passive (the pet mode getter, `0x5a7980`).
        let showing = if bar.vehicle() {
            benilla_protocol::messages::PET_REACT_PASSIVE
        } else {
            bar.react_state()
        };
        return PetActionView {
            name: Some(name.to_string()),
            texture: Some(texture.to_string()),
            is_token: true,
            active: showing == action,
            ..Default::default()
        };
    }

    // A zero word is empty (the client tests the dword); vmangos's unused slots are spell 0, which
    // has no record and takes the next exit, as in the client.
    if !entry.is_spell() || entry.is_empty() {
        return PetActionView::default();
    }
    let Some(spell) = spell else {
        // No `Spell.dbc` record, or no catalog: the reference returns nil and the button hides.
        return PetActionView::default();
    };
    PetActionView {
        name: Some(spell.name.clone()),
        subtext: spell.rank.clone(),
        // A running spell draws its `ActiveIconID`, with no fallback: the reference pushes nil when
        // that icon does not resolve.
        texture: if showing_active {
            spell.active_icon.clone()
        } else {
            spell.icon.clone()
        },
        is_token: false,
        spell_id: Some(action),
        // Nil on every spell path: `isActive` is a token's, `autoCast*` a spell's.
        active: false,
        // Bits 31/30, not the type, gated on the record checked above.
        autocast_allowed: entry.autocast_allowed(),
        autocast_enabled: entry.autocast_on(),
        attack_active: false,
        cooldown,
        ..Default::default()
    }
}

/// The spell id when the slot shows active, the pet-side twin of the action bar's
/// ([`crate::ui_action::toggle::active_action_toggle`]). It tests `ActiveIconID != 0` first, so a
/// press on a spell without one re-casts rather than cancels.
pub(super) fn active_aura_press(
    entry: PetActionEntry,
    pet: Option<&crate::unit_auras::UnitAuras>,
    spell: Option<&benilla_formats::SpellDisplay>,
) -> Option<u32> {
    if !entry.is_spell() || entry.is_empty() {
        return None;
    }
    let spell_id = entry.action();
    crate::ui_action::toggle::active_action_toggle(spell_id, spell?, pet).then_some(spell_id)
}

/// The bar-wide answers at the call: `PetHasActionBar`, the usability predicate, the drag's gate, the attacking global and the expiry's tick difference (`0x5d3185`-`0x5d31a5`).
pub(crate) fn bar_live(bar: &PetBar, pet: Option<&PetUnit>) -> PetBarLive {
    // `PickupPetAction`'s gate alone: the pet's object must resolve and not be possessed. A possessed unit's bar cannot be rearranged but its buttons work, so possession stays out of `actions_usable`; an unstreamed pet has no flags to test and lifts nothing.
    let pickup_allowed = pet
        .and_then(|p| p.store(bar.spells.pet_guid))
        .is_some_and(|s| s.0.unit_flags() & UNIT_FLAG_POSSESSED == 0);
    let now = Instant::now();
    PetBarLive {
        has_bar: bar.has_bar(),
        actions_usable: pet.is_some_and(|p| p.actions_usable(bar)),
        pickup_allowed,
        attacking: bar.attacking,
        // The expiry less the tick count as an unsigned 32-bit difference, wrapping once passed.
        #[allow(clippy::cast_possible_truncation)] // the reference's dword difference
        time_remaining_ms: bar.expires.map(|t| {
            let ms = match t.checked_duration_since(now) {
                Some(ahead) => ahead.as_secs_f64() * 1000.0,
                None => -(now.duration_since(t).as_secs_f64() * 1000.0),
            };
            ms.round() as i64 as u32
        }),
    }
}

/// Slot `index`'s view at the call: the word as `GetPetActionInfo` paints it, its cooldown from the pet's history on the VM clock `(anchor, ui_now)` names, and `GetPetActionSlotUsable`'s answer. `None` with no bar or past the ten words.
pub(crate) fn action_live(
    bar: &PetBar,
    index: usize,
    spells: Option<&Spells>,
    pet: Option<&PetUnit>,
    walk: Option<(&PetSlotWalk, WalkLent)>,
    (anchor, ui_now): (Instant, f64),
) -> Option<PetActionView> {
    if !bar.has_bar() {
        return None;
    }
    let entry = *bar.spells.bar.get(index)?;
    let now = Instant::now();
    let display = entry
        .is_spell()
        .then(|| spells.and_then(|s| s.catalog.get(entry.action())))
        .flatten();
    let cooldown = display.and_then(|d| {
        bar.cooldowns
            .info(entry.action(), 0, Some(d), now)
            .ui_triple(anchor, ui_now)
    });
    let auras = pet.and_then(|p| p.auras(bar.spells.pet_guid));
    let mut view = slot_view(
        entry,
        &bar.spells,
        display,
        cooldown,
        bar.attacking,
        active_aura_press(entry, auras, display).is_some(),
    );
    let usable = pet.is_some_and(|p| p.actions_usable(bar));
    let uncontrolled = pet.and_then(|p| p.usable_pet_with(bar, true));
    view.slot_usable =
        super::usable::slot_usable(entry, display, usable, uncontrolled, |d, store| {
            match (walk, pet) {
                (Some((walk, lent)), Some(pet)) => {
                    walk.usable(lent, entry.action(), d, spells, store, pet, &bar.cooldowns)
                }
                // Without the walk's inputs the walk is skipped and the slot reads usable.
                _ => true,
            }
        });
    Some(view)
}
