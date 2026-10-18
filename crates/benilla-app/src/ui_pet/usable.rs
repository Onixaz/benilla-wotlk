//! The pet-usability predicate `CGPetInfo::PetActionsUsable` (`0x5d3e80`) with control required,
//! asked once by everything the reference lets it gate: `GetPetActionsUsable` (`0x5d53c3`), the
//! dispatcher behind `CastPetAction` and the orders, both autocast toggles and the bar write the
//! drag makes (`0x5d5ad0`). A false answer leaves each of them before anything is latched, armed,
//! cancelled or sent.

use benilla_formats::SpellDisplay;
use benilla_protocol::messages::{OwnerFallback, PetActionEntry, PET_UNUSABLE_UNIT_FLAGS};

use crate::net::ObjectStore;

use super::{PetBar, PetUnit};

/// `0x5d3e80` with control required, every exit in the order the bytes take them. `me` is the
/// client's active player guid; `player` is that guid's object and `pet` the bar's pet's, each
/// `None` while it is not streamed; `riding` is the transport the player rides.
#[cfg(test)]
pub(super) fn actions_usable(
    bar: &PetBar,
    me: Option<u64>,
    player: Option<&ObjectStore>,
    pet: Option<&ObjectStore>,
    riding: Option<u64>,
) -> bool {
    actions_usable_with(bar, me, player, pet, riding, false)
}

/// `0x5d3e80(ignoreControl)`: a spell slot's ask (`0x5d4018`) skips the control test.
pub(super) fn actions_usable_with(
    bar: &PetBar,
    me: Option<u64>,
    player: Option<&ObjectStore>,
    pet: Option<&ObjectStore>,
    riding: Option<u64>,
    ignore_control: bool,
) -> bool {
    // `0x5d3ea4`: the active player must resolve.
    let (Some(me), Some(player)) = (me, player) else {
        return false;
    };
    // `0x5d3eb0`: a charmed player, whose `UNIT_FIELD_CHARMEDBY` is nonzero, has no pet bar.
    if player.0.unit_charmed_by().is_some() {
        return false;
    }
    // `0x5d3ec0`-`0x5d3ef3`: a far-sight view held on some unit must be on the pet: Eyes of the
    // Beast keeps the bar, Far Sight or Mind Vision on anything else takes it.
    if player
        .0
        .player_farsight()
        .is_some_and(|held| held != bar.spells.pet_guid)
    {
        return false;
    }
    // `0x5d3f3c`: the pet must resolve to a unit.
    let Some(pet) = pet else {
        return false;
    };
    // `0x5d3f3e`-`0x5d3f64`: the pet's charmed-by guid, or its summoned-by guid while that is
    // zero, must be the active player's.
    if pet.0.unit_owner(OwnerFallback::SummonedBy) != Some(me) {
        return false;
    }
    // `0x5d3f6e`: `CGUnit_C::IsControllable` (`0x5d3310`), not stunned, fleeing or confused.
    if !ignore_control && pet.0.unit_flags() & PET_UNUSABLE_UNIT_FLAGS != 0 {
        return false;
    }
    // `0x5d3f77`-`0x5d3f9a`: a vehicle's bar works only while we ride the pet.
    !bar.spells.vehicle() || riding == Some(bar.spells.pet_guid)
}

/// `AttributesEx5` exemptions from the pet's state (`0x5d40bb`-`0x5d4110`): usable while stunned,
/// feared and confused.
const ATTR_EX5_USABLE_WHILE_STUNNED: u32 = 0x8;
const ATTR_EX5_USABLE_WHILE_FEARED: u32 = 0x2_0000;
const ATTR_EX5_USABLE_WHILE_CONFUSED: u32 = 0x4_0000;
/// The pet's `UNIT_FIELD_FLAGS` the exemptions answer: stunned, fleeing, confused.
const UNIT_FLAG_STUNNED: u32 = 0x4_0000;
const UNIT_FLAG_FLEEING: u32 = 0x80_0000;
const UNIT_FLAG_CONFUSED: u32 = 0x40_0000;

/// `CGPetInfo::PetActionSlotUsable` (`0x5d3fd0`) for one slot word. An order, a mode or an empty
/// word asks the bar's predicate; a spell asks it without the control test (`usable_uncontrolled`
/// holds the pet it resolved), needs its record, is refused by a stun, fear or confusion its
/// `AttributesEx5` does not exempt, then by `Spell_C_IsUsableAction` (`0x80b5d0`, `walk`).
pub(super) fn slot_usable(
    entry: PetActionEntry,
    display: Option<&SpellDisplay>,
    usable: bool,
    usable_uncontrolled: Option<&ObjectStore>,
    walk: impl FnOnce(&SpellDisplay, &ObjectStore) -> bool,
) -> bool {
    if !entry.is_spell() {
        return usable;
    }
    let (Some(pet), Some(d)) = (usable_uncontrolled, display) else {
        return false;
    };
    let flags = pet.0.unit_flags();
    let barred = |state: u32, exempt: u32| flags & state != 0 && d.attributes_ex5 & exempt == 0;
    let barred = barred(UNIT_FLAG_STUNNED, ATTR_EX5_USABLE_WHILE_STUNNED)
        || barred(UNIT_FLAG_FLEEING, ATTR_EX5_USABLE_WHILE_FEARED)
        || barred(UNIT_FLAG_CONFUSED, ATTR_EX5_USABLE_WHILE_CONFUSED);
    !barred && walk(d, pet)
}

impl PetUnit<'_, '_> {
    /// [`actions_usable`] for the bar as it stands: the pet's object when the predicate holds,
    /// `None` when it does not. A caller past the gate holds the pet the predicate resolved.
    pub(crate) fn usable_pet(&self, bar: &PetBar) -> Option<&ObjectStore> {
        self.usable_pet_with(bar, false)
    }

    /// [`Self::usable_pet`], skipping the control test when `ignore_control` is set.
    pub(crate) fn usable_pet_with(
        &self,
        bar: &PetBar,
        ignore_control: bool,
    ) -> Option<&ObjectStore> {
        let pet = self.store(bar.spells.pet_guid);
        pet.filter(|_| {
            actions_usable_with(
                bar,
                self.self_guid.0,
                self.player_store(),
                pet,
                // benilla seats no vehicle (#235), so no ride is ever the pet.
                None,
                ignore_control,
            )
        })
    }

    /// `GetPetActionsUsable()`, and the gate every pet-bar verb the reference guards asks.
    pub(crate) fn actions_usable(&self, bar: &PetBar) -> bool {
        self.usable_pet(bar).is_some()
    }
}
