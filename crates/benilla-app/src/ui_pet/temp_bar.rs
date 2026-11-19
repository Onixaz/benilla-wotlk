//! The pet's temporary action bar (`0x5d4ad0`): while our far sight is on our pet and the pet bar is in use (`0x5d35b0`), the main bar's slots 121-132 take the pet bar's ten actions as pet-action slots (`0x5ab800` with `0x5d3240`), and the bar pages to the bonus page. A vehicle we ride as the pet does this only where our seat shows its abilities on the main bar (`vehicleAbilityDisplay` 1); a seat of 0 keeps the pet bar off. The slots stay as written when the bar ends, as the reference leaves them.

use bevy::prelude::*;

use benilla_formats::SpellCatalog;
use benilla_protocol::messages::{PET_ACT_COMMAND, PET_ACT_REACTION};

use crate::net::{GuidIndex, ObjectStore, SelfPlayer};
use crate::ui_action::{
    ActionSlots, TempActionBar, ACTION_KIND_PET, TEMP_BAR_FIRST_SLOT, TEMP_BAR_PET,
};

use super::{bar, PetBar};

/// The pet bar's slots the temporary bar shows (`0x5d3240`: indices under 10).
const PET_BAR_SLOTS: u32 = 10;

/// `CreatureTypeFlags & 0x2000`: a creature whose bar never shows (`0x7226b0`).
const TYPE_FLAG_NO_PET_BAR: u32 = 0x2000;

/// A pet-action slot's icon (`0x5a97f0`): a spell's, or a mode or order's `PET_%s_TEXTURE` global.
/// A spell draws its own icon; the active-icon swap of a running spell is the pet bar's alone.
pub(crate) fn temp_slot_icon(
    bar: &PetBar,
    index: u32,
    spells: Option<&SpellCatalog>,
) -> Option<benilla_ui::script::ActionIcon> {
    use benilla_ui::script::ActionIcon;
    let entry = *bar.spells.bar.get(usize::try_from(index).ok()?)?;
    if entry.is_empty() {
        return None;
    }
    let token = match entry.kind() {
        PET_ACT_REACTION => bar::reaction_token(entry.action()),
        PET_ACT_COMMAND => bar::command_token(entry.action()),
        _ if entry.is_spell() => {
            return spells?
                .get(entry.action())?
                .icon
                .clone()
                .map(ActionIcon::Path)
        }
        _ => None,
    };
    Some(ActionIcon::Global(token?.1))
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

/// `0x5d4ad0` as a step in its caller's turn: the pet setter's (`0x5d6ad2`) and the far-sight field's (`0x6e504a`).
pub(crate) fn update_pet_temp_bar_step(world: &mut World) {
    if let Err(e) = world.run_system_cached(update_pet_temp_bar) {
        warn!("ui_pet: the temporary bar's update did not run: {e}");
    }
}

/// The pet can gain experience (`0x71b630`): a pet number and a Hunter creator held as a player.
pub(super) fn can_gain_xp(pet: &ObjectStore, creator: Option<&ObjectStore>) -> bool {
    const CLASS_HUNTER: u8 = 3;
    pet.0.unit_is_pet_or_charm() && creator.and_then(|c| c.0.unit_class()) == Some(CLASS_HUNTER)
}

/// The action store, the temporary bar and the possess latch the bar's update writes and reads.
type TempBarState<'w> = (
    Option<ResMut<'w, ActionSlots>>,
    Option<ResMut<'w, TempActionBar>>,
    Option<Res<'w, super::possess::PossessSpell>>,
);

/// Write the temporary bar's slots while the pet's bar takes it, then its flag, and fire `PET_BAR_UPDATE` on every call (`0x5d4c1b`).
#[allow(clippy::too_many_arguments)] // a Bevy system's full input set
fn update_pet_temp_bar(
    bar: Res<PetBar>,
    me: Query<(&ObjectStore, Option<&crate::vehicle::Passenger>), With<SelfPlayer>>,
    stores: Query<(&ObjectStore, Option<&crate::vehicle::Vehicle>)>,
    index: Res<GuidIndex>,
    names: Option<Res<crate::names::NameCache>>,
    tables: Option<Res<crate::vehicle::VehicleTables>>,
    (actions, temp, possess): TempBarState,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
) {
    // The flag write and the signal end every call (`0x5d4c07`-`0x5d4c1b`).
    let fire = |ui: &mut crate::ui_script::live::UiEvents| ui.fire("PET_BAR_UPDATE", Vec::new());
    // A harness without the action bar's plugin has no slots to write.
    let (Some(mut actions), Some(mut temp)) = (actions, temp) else {
        fire(&mut ui);
        return;
    };
    let pet_guid = bar.spells.pet_guid;
    let Ok((player, seated)) = me.single() else {
        fire(&mut ui);
        return;
    };
    let entity = (pet_guid != 0).then(|| index.0.get(&pet_guid)).flatten();
    let pet = entity.and_then(|&e| stores.get(e).ok());
    // Our seat on the pet, when we ride it: its `vehicleAbilityDisplay` (`0x5d4b20`).
    let riding_seat = seated
        .filter(|p| p.vehicle == pet_guid && pet_guid != 0)
        .and_then(|p| {
            let (_, vehicle) = pet?;
            tables.as_deref()?.seat(vehicle?.id, usize::from(p.seat))
        })
        .map(|s| s.vehicle_ability_display);
    // `0x5d35b0`: an unheld pet counts as used; a held one by its own test and its health, then by the bar latch: down, or a possess spell held, or a pet that can gain experience (`0x5d35ff`-`0x5d3613`).
    let (spell, latch) = possess.as_deref().map_or((0, false), |p| (p.0, p.1));
    let used = pet_guid != 0
        && pet.is_none_or(|(store, _)| {
            let creator = store
                .0
                .unit_created_by()
                .and_then(|g| index.0.get(&g))
                .and_then(|&e| stores.get(e).ok())
                .map(|(s, _)| s);
            unit_pet_bar_used(store, names.as_deref(), riding_seat)
                && store.0.unit_health().unwrap_or(0) > 0
                && (!latch || spell != 0 || can_gain_xp(store, creator))
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
        // Each of the twelve set without a send and signalled, moved or not (`0x5ab800`).
        for i in 0..12u8 {
            let packed = if u32::from(i) < PET_BAR_SLOTS {
                (u32::from(ACTION_KIND_PET) << 24) | u32::from(i)
            } else {
                0
            };
            let slot = TEMP_BAR_FIRST_SLOT + i;
            actions.set_local(slot, packed);
            crate::ui_action::slots::changed_in_turn(&mut ui, slot);
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
    }
    // The flag is set before the bonus update and the event (`0x5d4c0a`, `0x5d4c0f`), whose stock
    // handler asks `PetHasActionBar` whether the pet bar stays.
    ui.world(crate::ui_action::bonus::update_bonus_bar);
    fire(&mut ui);
}

#[cfg(test)]
mod tests {
    use super::*;

    use benilla_protocol::messages::PetActionEntry;

    #[test]
    fn a_slot_shows_its_action_s_texture() {
        use benilla_ui::script::ActionIcon;
        let mut bar = PetBar::default();
        bar.spells.bar[0] = PetActionEntry::from(0x0700_0002); // Attack
        bar.spells.bar[1] = PetActionEntry::from(0x0600_0000); // Passive
        assert_eq!(
            temp_slot_icon(&bar, 0, None),
            Some(ActionIcon::Global("PET_ATTACK_TEXTURE"))
        );
        assert_eq!(
            temp_slot_icon(&bar, 1, None),
            Some(ActionIcon::Global("PET_PASSIVE_TEXTURE"))
        );
        assert_eq!(temp_slot_icon(&bar, 2, None), None, "empty");
        assert_eq!(temp_slot_icon(&bar, 12, None), None, "past the bar");
    }
}
