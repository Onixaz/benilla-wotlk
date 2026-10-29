//! The pet's temporary action bar (`CGPetInfo::UpdatePetTempBar`, `0x5d4ad0`): while our far
//! sight is on our pet and the pet bar is in use (`0x5d35b0`), the main bar's slots 121-132 take
//! the pet bar's ten actions as pet-action slots (`0x5ab800` with `0x5d3240`), and the bar pages
//! to the bonus page. A vehicle we ride as the pet does this only where our seat shows its
//! abilities on the main bar (`vehicleAbilityDisplay` 1); a seat of 0 keeps the pet bar off. The
//! slots stay as written when the bar ends, as the reference leaves them.

use bevy::prelude::*;

use benilla_formats::SpellCatalog;
use benilla_protocol::messages::{PET_ACT_COMMAND, PET_ACT_REACTION};
use benilla_ui::script::UiScript;

use crate::net::{GuidIndex, ObjectStore, SelfPlayer};
use crate::ui_action::{
    PlayerActions, TempActionBar, ACTION_KIND_PET, TEMP_BAR_FIRST_SLOT, TEMP_BAR_PET,
};

use super::{bar, PetBar};

/// The pet bar's slots the temporary bar shows (`0x5d3240`: indices under 10).
const PET_BAR_SLOTS: u32 = 10;

/// `CreatureTypeFlags & 0x2000`: a creature whose bar never shows (`0x7226b0`).
const TYPE_FLAG_NO_PET_BAR: u32 = 0x2000;

/// A pet-action slot's texture (`0x5a97f0`): a spell's icon, or a mode or order's
/// `PET_%s_TEXTURE` global through `global`. A spell draws its own icon; the active-icon swap of
/// a running spell is the pet bar's alone.
pub(crate) fn temp_slot_texture(
    bar: &PetBar,
    index: u32,
    spells: Option<&SpellCatalog>,
    global: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let entry = *bar.spells.bar.get(usize::try_from(index).ok()?)?;
    if entry.is_empty() {
        return None;
    }
    let token = match entry.kind() {
        PET_ACT_REACTION => bar::reaction_token(entry.action()),
        PET_ACT_COMMAND => bar::command_token(entry.action()),
        _ if entry.is_spell() => return spells?.get(entry.action())?.icon.clone(),
        _ => None,
    };
    global(token?.1)
}

/// `0x7226b0`: the pet's bar is in use unless its template forbids it, or we ride it in a seat
/// that shows no abilities.
fn unit_pet_bar_used(
    pet: &ObjectStore,
    names: Option<&crate::names::NameCache>,
    riding_seat: Option<i32>,
) -> bool {
    let forbidden = pet
        .0
        .object_entry()
        .and_then(|e| names?.creature_record(e))
        .is_some_and(|r| r.type_flags & TYPE_FLAG_NO_PET_BAR != 0);
    !forbidden && riding_seat.is_none_or(|d| d != 0)
}

/// Write the temporary bar and its flag when the pet's bar takes it or leaves it.
#[allow(clippy::too_many_arguments)] // a Bevy system's full input set
pub(super) fn update_pet_temp_bar(
    script: Option<NonSendMut<UiScript>>,
    bar: Res<PetBar>,
    me: Query<(&ObjectStore, Option<&crate::vehicle::Passenger>), With<SelfPlayer>>,
    stores: Query<(&ObjectStore, Option<&crate::vehicle::Vehicle>)>,
    index: Res<GuidIndex>,
    names: Option<Res<crate::names::NameCache>>,
    tables: Option<Res<crate::vehicle::VehicleTables>>,
    actions: Option<ResMut<PlayerActions>>,
    temp: Option<ResMut<TempActionBar>>,
) {
    // A harness without the action bar's plugin has no slots to write.
    let (Some(mut actions), Some(mut temp)) = (actions, temp) else {
        return;
    };
    let pet_guid = bar.spells.pet_guid;
    let Ok((player, seated)) = me.single() else {
        return;
    };
    let pet = (pet_guid != 0)
        .then(|| index.0.get(&pet_guid))
        .flatten()
        .and_then(|&e| stores.get(e).ok());
    // Our seat on the pet, when we ride it: its `vehicleAbilityDisplay` (`0x5d4b20`).
    let riding_seat = seated
        .filter(|p| p.vehicle == pet_guid && pet_guid != 0)
        .and_then(|p| {
            let (_, vehicle) = pet?;
            tables.as_deref()?.seat(vehicle?.id, usize::from(p.seat))
        })
        .map(|s| s.vehicle_ability_display);
    // `0x5d35b0`: an unheld pet counts as used; a held one by its own test and its health.
    let used = pet_guid != 0
        && pet.is_none_or(|(store, _)| {
            unit_pet_bar_used(store, names.as_deref(), riding_seat)
                && store.0.unit_health().unwrap_or(0) > 0
        });
    let far_sight = player.0.player_farsight().unwrap_or(0);
    let mut active = far_sight != 0 && far_sight == pet_guid && used;
    let mut write = active;
    if active {
        if let Some(display) = riding_seat {
            active = display == 1;
            write = active;
        }
    }
    if write {
        let mut moved = false;
        for i in 0..12u8 {
            let packed = if u32::from(i) < PET_BAR_SLOTS {
                (u32::from(ACTION_KIND_PET) << 24) | u32::from(i)
            } else {
                0
            };
            moved |= actions.set_local(TEMP_BAR_FIRST_SLOT + i, packed);
        }
        // The slots' icons follow the pet bar's actions.
        if !moved && bar.is_changed() {
            actions.dirty = true;
        }
    }
    let flags = if active {
        temp.flags | TEMP_BAR_PET
    } else {
        temp.flags & !TEMP_BAR_PET
    };
    if flags != temp.flags {
        debug!(
            "ui_pet: temporary bar {}",
            if active { "on" } else { "off" }
        );
        temp.flags = flags;
        if let Some(mut script) = script {
            script.fire_event("PET_BAR_UPDATE", vec![]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use benilla_protocol::messages::PetActionEntry;

    #[test]
    fn a_slot_shows_its_action_s_texture() {
        let mut bar = PetBar::default();
        bar.spells.bar[0] = PetActionEntry::from(0x0700_0002); // Attack
        bar.spells.bar[1] = PetActionEntry::from(0x0600_0000); // Passive
        let global = |k: &str| Some(format!("tex:{k}"));
        assert_eq!(
            temp_slot_texture(&bar, 0, None, &global).as_deref(),
            Some("tex:PET_ATTACK_TEXTURE")
        );
        assert_eq!(
            temp_slot_texture(&bar, 1, None, &global).as_deref(),
            Some("tex:PET_PASSIVE_TEXTURE")
        );
        assert_eq!(temp_slot_texture(&bar, 2, None, &global), None, "empty");
        assert_eq!(
            temp_slot_texture(&bar, 12, None, &global),
            None,
            "past the bar"
        );
    }
}
