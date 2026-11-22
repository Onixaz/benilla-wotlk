//! The pet system: [`PetBar`], the pet action bar's state, and the app side of
//! `benilla_ui::script::pet`. The usual three commands, four spells and three reactions are server
//! data (vmangos `CharmInfo::InitPetActionBar`), not a layout: a possessed or charmed unit fills
//! the same ten words differently, so nothing here assumes them.

use std::time::Instant;

use bevy::prelude::*;

use benilla_protocol::messages::PetSpells;

use crate::net::{GuidIndex, ObjectStore};
use crate::spell::Cooldowns;
use crate::ui_script::UiDrain;
use crate::ui_unit::UnitFeed;

mod bar;
mod drain;
mod menu;
mod net;
pub(crate) mod possess;
mod temp_bar;
mod unit;
mod usable;

pub(crate) use bar::{action_live, bar_live, PetSlotWalk, WalkLent};
// A pet bar press, applied in call order by `crate::script_calls`.
pub(crate) use drain::pet_stop_on_old_target_clear;
pub(crate) use drain::{
    plain_press_word, press_plain, send_pet_action, stop_pet_attack, toggle_slot_autocast, PetCast,
    PetPress,
};
use menu::drain_pet_menu;
#[cfg(test)]
pub(crate) use menu::PET_FLAG_ABANDON;
pub(crate) use menu::{menu_predicates, run_menu_verb, summon_bars_dismiss, MenuCx};
pub(crate) use net::{set_pet_tail, tame_failure_line};
pub(crate) use temp_bar::{temp_slot_icon, update_pet_temp_bar_step};
pub(crate) use unit::{feed_pet_unit, pet_combat_flag, pet_renamed, push_pet_token};

#[cfg(test)]
mod flush_tests;
#[cfg(test)]
mod possess_tests;
#[cfg(test)]
mod press_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod usable_tests;

/// The pet action bar's state. A zero `spells.pet_guid` means no bar: the teardown packet carries
/// only that guid.
#[derive(Resource, Default)]
pub(crate) struct PetBar {
    /// The last `SMSG_PET_SPELLS`, with `SMSG_PET_MODE` and local presses folded in.
    pub(crate) spells: PetSpells,
    /// The setter's cached guid `[0xc234d0]`, read only by its change test (`0x5d6903`) and never written by `SMSG_PET_GUIDS` (`0x5d6550`).
    pub(crate) cached: u64,
    /// The pet's own cooldowns, the list. Seeded from `SMSG_PET_SPELLS`, topped up by `SMSG_SPELL_COOLDOWN` for the pet's guid, by the pet's own `SMSG_SPELL_GO` and by a press's GCD. Each mutation but the seed signals where it lands ([`crate::ui_action::signals`]).
    pub(crate) cooldowns: Cooldowns,
    /// The pet-attacking global `[0xc234dc]`: all of `IsPetAttackActive` and the only thing that lights Attack. Written by the flags field's pet arm after its event (`0x5d30ba`), a possessed Attack press (`0x5d4509`), the stop (`0x5d46c2`) and a new pet's combat test (`0x5d6a80`).
    pub(crate) attacking: bool,
    /// The `PET_BAR_UPDATE`s a press's latch raised, counted (`0x5d300c`, `0x5d3041`): its applier fires one per count as the press returns ([`bar_signals_in_turn`]). Wrapping.
    pub(crate) bar_signals: u32,
    /// The charm or possess expiry, the packet's duration past its arrival; `None` when the
    /// duration is 0, as for a hunter's or warlock's own pet.
    pub(crate) expires: Option<Instant>,
}

/// The controlled-pet list (`[0xc23500]`), which the pet verbs read at call time (`0x5d3a24`).
#[derive(Resource, Default)]
pub(crate) struct PetList(pub(crate) Vec<u64>);

impl PetList {
    /// The head the pet verbs read, `None` for an empty list (`0x5d3a2b`).
    pub(crate) fn head(&self) -> Option<u64> {
        self.0.first().copied()
    }
}

/// The cascade's pet step (`0x5d6e60`): its ask sent here in the cascade's send order (`0x5281fe` between `0x5281db` and `0x52822b`), on the gate read inside our create when the step ran there, else while the drain waits for the entry, so no answer lands before the step's clear; the rest owed for after `PLAYER_ENTERING_WORLD`.
pub(crate) fn request_pet_info_on_world_enter(
    mut cascades: MessageReader<crate::net::WorldEnterCascadeMessage>,
    hold: Option<Res<crate::net::EntryHold>>,
    player: Query<&ObjectStore, With<crate::net::SelfPlayer>>,
    commands: Res<crate::net::NetCommands>,
    step: Option<ResMut<possess::WorldEntryPetStep>>,
) {
    if cascades.read().next().is_none() {
        return;
    }
    let Some(mut step) = step else {
        return;
    };
    let (owed, asked) = match step.at_create {
        Some(gated) => (false, gated),
        None => {
            let held = hold.is_some_and(|h| h.0);
            let gated = player
                .iter()
                .next()
                .and_then(|s| s.0.unit_pet_guid())
                .is_some();
            (true, held && gated)
        }
    };
    if asked {
        debug!("ui_pet: world entry with a pet out, asking its bar");
        let _ = commands.0.send(crate::net::ClientCommand::RequestPetInfo);
    }
    *step = possess::WorldEntryPetStep {
        owed,
        asked,
        at_create: None,
    };
}

impl PetBar {
    /// A nonzero list head, which `spells.pet_guid` mirrors (`0x5d3723`-`0x5d3737`), with no alive or control check.
    pub(crate) fn has_bar(&self) -> bool {
        self.spells.pet_guid != 0
    }

    /// No pet: every field back to its default, the cooldown list emptied with its counters kept
    /// ([`Cooldowns::clear_silent`]), for the teardown packet and the session end.
    pub(crate) fn clear(&mut self) {
        let mut cooldowns = std::mem::take(&mut self.cooldowns);
        cooldowns.clear_silent();
        *self = Self {
            cooldowns,
            ..Default::default()
        };
    }
}

/// The `PET_BAR_UPDATE`s a press raised since `before`, the bar's signal count then, in turn on `ui`.
pub(crate) fn bar_signals_in_turn(
    ui: &mut crate::ui_script::live::UiEvents,
    before: u32,
    now: u32,
) {
    for _ in 0..now.wrapping_sub(before) {
        ui.fire("PET_BAR_UPDATE", Vec::new());
    }
}

pub(crate) struct UiPetPlugin;

impl Plugin for UiPetPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<PetBar>()
            .init_resource::<PetList>()
            .init_resource::<possess::PossessSpell>()
            .init_resource::<possess::WorldEntryPetStep>()
            .add_message::<crate::net::WorldEnterCascadeMessage>()
            .add_systems(
                Update,
                (
                    // The old-target clear after the press pass whose selects move the target, and before the feeds, as it writes the bar every lent view reads.
                    pet_stop_on_old_target_clear
                        .after(crate::ui_script::UiPress)
                        .before(crate::ui_script::UiFeed)
                        .before(crate::ui_action::SlotIdentityFeed),
                    feed_pet_unit.in_set(UnitFeed),
                    // The entry ask: after the time, before the mail (`0x528010`), owed before PEW.
                    request_pet_info_on_world_enter
                        .after(crate::net::send_query_time)
                        .before(crate::ui_mail::send_query_next_mail_time_on_enter)
                        .before(UnitFeed)
                        .before(crate::ui_script::UiPress),
                    // The menu verbs a queued script call held back, once the interface is up.
                    crate::ui_script::raises::then_fire(
                        drain_pet_menu.run_if(crate::ui_script::ingame_ui_up),
                    )
                    .in_set(UiDrain),
                ),
            );
    }
}

/// The unit behind [`PetBar`]'s cached guid, plus our own guid for the ownership tests the
/// reference makes before trusting the pet's fields, and the usability predicate every pet-bar verb
/// the reference guards asks of them ([`PetUnit::usable_pet`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PetUnit<'w, 's> {
    index: Res<'w, GuidIndex>,
    stores: Query<'w, 's, &'static ObjectStore>,
    auras: Query<'w, 's, &'static crate::unit_auras::UnitAuras>,
    self_guid: Res<'w, crate::net::SelfGuid>,
    /// Our seat, whose vehicle is the transport the predicate asks after.
    seat: Query<'w, 's, &'static crate::vehicle::Passenger, With<crate::net::SelfPlayer>>,
}

impl PetUnit<'_, '_> {
    /// The pet's descriptor, `None` while its object is not streamed: the reference's "no pet".
    pub(crate) fn store(&self, pet_guid: u64) -> Option<&ObjectStore> {
        let e = *self.index.0.get(&pet_guid)?;
        self.stores.get(e).ok()
    }

    /// The pet's aura table, `None` while its object is not streamed or holds no aura yet.
    pub(crate) fn auras(&self, pet_guid: u64) -> Option<&crate::unit_auras::UnitAuras> {
        let e = *self.index.0.get(&pet_guid)?;
        self.auras.get(e).ok()
    }

    /// The active player's descriptor, `None` while its object is not streamed: the lookup the
    /// usability predicate makes on the client's player guid (typemask `0x10`).
    pub(crate) fn player_store(&self) -> Option<&ObjectStore> {
        self.store(self.self_guid.0?)
    }

    /// The pet's entity under [`Self::store`]'s contract, for the pet paper doll's model.
    pub(crate) fn entity(&self, pet_guid: u64) -> Option<Entity> {
        let e = *self.index.0.get(&pet_guid)?;
        self.stores.contains(e).then_some(e)
    }
}
