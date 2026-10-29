//! The bar's click intents onto the wire, and the state a press latches locally: vmangos answers
//! no command or reaction press (`PetHandler.cpp:81-99`) and no autocast toggle
//! (`PetHandler.cpp:203-300`), and sends `SMSG_PET_MODE` only from `Pet::SetEnabled`
//! (`Pet.cpp:2362-2377`), so the client applies them itself until the next `SMSG_PET_SPELLS`.

use std::time::Instant;

use bevy::prelude::*;

use benilla_protocol::messages::{
    PetActionEntry, PET_COMMAND_ATTACK, PET_COMMAND_FOLLOW, PET_COMMAND_STAY,
};
use benilla_ui::script::UiScript;

use crate::net::{ClientCommand, NetCommands, ObjectStore};
use crate::target::{is_possessed_by, Selection};
use crate::ui_action::Spells;

use super::bar::active_aura_press;
use super::{PetBar, PetUnit};

/// What a pet bar press reads and writes: `CastPetAction`, `PetAttack` and the other orders, and
/// the pet book's `CastSpell(id, "pet")`.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PetPress<'w, 's> {
    pub(crate) bar: ResMut<'w, PetBar>,
    pub(crate) selection: ResMut<'w, Selection>,
    pub(crate) commands: Res<'w, NetCommands>,
    pub(crate) pet: PetUnit<'w, 's>,
    pub(crate) spells: Option<Res<'w, Spells>>,
    /// The talent modifiers the pet GCD's op 21 goes through: the player's tables, which gate on
    /// the spell's family and never on the caster.
    spell_mods: Res<'w, crate::spell::SpellModifiers>,
    ui_errors: ResMut<'w, crate::ui_action::UiErrorKeys>,
    pick: crate::target::AttackPick<'w, 's>,
    seam: crate::creature_anim::AttackSeam<'w, 's>,
    /// `secureAbilityToggle`, the recently-used guard on an aura cancel.
    cvars: Option<Res<'w, crate::cvars::Cvars>>,
}

/// The pet's control state against a spell (`0x5d437f`-`0x5d43dc`): stunned unless
/// `AttributesEx5 & 0x8`, else fleeing unless `& 0x20000`, else confused unless `& 0x40000`.
fn pet_cannot_cast(unit_flags: u32, spell: &benilla_formats::SpellDisplay) -> bool {
    const STUNNED: u32 = 0x0004_0000;
    const FLEEING: u32 = 0x0080_0000;
    const CONFUSED: u32 = 0x0040_0000;
    let ex5 = spell.attributes_ex5;
    (ex5 & 0x8 == 0 && unit_flags & STUNNED != 0)
        || (ex5 & 0x2_0000 == 0 && unit_flags & FLEEING != 0)
        || (ex5 & 0x4_0000 == 0 && unit_flags & CONFUSED != 0)
}

/// Where a press went: [`PetCast::CastAsPet`] leaves the spell to the cast ladder with the pet as
/// caster (`Spell_C_CastSpell` `0x80cce0`), which [`crate::script_calls`] runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PetCast {
    Done,
    CastAsPet {
        pet_guid: u64,
        spell_id: u32,
        target_guid: u64,
    },
}

impl PetPress<'_, '_> {
    /// `CastPetAction(slot)`, a 1-based bar slot.
    pub(crate) fn press_slot(&mut self, slot: u32) -> PetCast {
        match slot_entry(&self.bar, slot) {
            Some(entry) => self.press(slot, entry, None),
            None => PetCast::Done,
        }
    }

    /// The guid the pet book's `CastSpell(id, "pet", onSelf)` passes: the active player with
    /// `onSelf`, else none, which `SendPetAction` reads as the selection.
    pub(crate) fn spell_target(&self, on_self: bool) -> Option<u64> {
        if on_self {
            self.pet.self_guid.0
        } else {
            None
        }
    }

    /// `PetAttack` and the other one-shot orders, as their slot's packed word. A slot press, an
    /// order and a pet book cast are one press: each reaches `CGPetInfo::SendPetAction`
    /// (`0x5d4210`). The slot number only labels logs.
    pub(crate) fn order(&mut self, packed: u32) -> PetCast {
        self.press(0, PetActionEntry::from(packed), None)
    }

    /// The pet book's `CastSpell(id, "pet")` (`CGSpellBook::CastSpell` `0x53bc40`): the spell
    /// word `id | 0x1000000` through the bar's dispatcher, at `target` if one is passed.
    pub(crate) fn cast_book_spell(&mut self, spell_id: u32, target: Option<u64>) -> PetCast {
        let entry = PetActionEntry::from(0x0100_0000 | (spell_id & 0x00FF_FFFF));
        self.press(0, entry, target)
    }

    /// `CGPetInfo::SendPetAction` (`0x5d4210`), under [`PetBar`]'s pet guid, never one from the VM,
    /// so a press queued as the pet leaves dies here. `CMSG_PET_ACTION` echoes the slot's word,
    /// which the server dispatches on its type, with `passed` or else the selection as the target.
    /// The usability predicate comes first (`0x5d4285`), and a refusal sends nothing.
    fn press(&mut self, slot: u32, entry: PetActionEntry, passed: Option<u64>) -> PetCast {
        let Self {
            bar,
            selection,
            commands,
            pet,
            spells,
            spell_mods,
            ui_errors,
            pick,
            seam,
            cvars,
        } = self;
        // The spell arm asks with `ignoreControl` set (`0x5d4283 push 1`), the others without.
        let Some(pet_store) = pet.usable_pet_with(bar, entry.is_spell()) else {
            debug!("ui_pet: slot {slot} refused, the pet's actions are not usable — no packet");
            return PetCast::Done;
        };
        let pet_guid = bar.spells.pet_guid;
        let possessing = possessing(Some(pet_store), pet.self_guid.0);
        // A press on a running spell cancels its aura (`0x5d4325`), kept if cast under 1.5 s ago.
        let display = entry
            .is_spell()
            .then(|| spells.as_ref().and_then(|s| s.catalog.get(entry.action())))
            .flatten();
        if let Some(spell_id) = active_aura_press(entry, pet.auras(pet_guid), display) {
            let secure = cvars
                .as_ref()
                .and_then(|c| c.num("secureAbilityToggle"))
                .is_some_and(|v| v != 0.0);
            if secure && bar.cooldowns.recently_cast(spell_id, 0, Instant::now()) {
                debug!("ui_pet: slot {slot} (spell {spell_id}) was just cast — the aura stays");
                return PetCast::Done;
            }
            debug!("ui_pet: slot {slot} cancels its own aura (spell {spell_id}) — no PetAction");
            let _ = commands
                .0
                .send(ClientCommand::PetCancelAura { pet_guid, spell_id });
            return PetCast::Done;
        }
        // The spell arm leaves without a `Spell.dbc` record (`0x5d42b4`).
        let spell = if entry.is_spell() {
            let Some(spell) = display else {
                debug!(
                    "ui_pet: slot {slot} (spell {}) has no record — no packet",
                    entry.action()
                );
                return PetCast::Done;
            };
            Some(spell)
        } else {
            None
        };
        // The passed target, else the selection (`0x5d4240`).
        let mut target_guid = passed.filter(|&g| g != 0).or(selection.guid).unwrap_or(0);
        // `AttributesEx4 & 0x20` (`0x5d4407`) or a possessed pet casts as the pet (`0x5d445a`).
        if let Some(spell) = spell {
            if pet_cannot_cast(pet_store.0.unit_flags(), spell) {
                debug!(
                    "ui_pet: slot {slot} (spell {}) refused, the pet is stunned, fleeing or \
                     confused — no packet",
                    entry.action()
                );
                return PetCast::Done;
            }
            if spell.allows_client_targeting() || possessing {
                debug!(
                    "ui_pet: slot {slot} (spell {}) casts as the pet at {target_guid:#x}",
                    entry.action()
                );
                return PetCast::CastAsPet {
                    pet_guid,
                    spell_id: entry.action(),
                    target_guid,
                };
            }
        }
        // Only Attack runs the attack validator, whose pick can move the selection (`0x5d4500`).
        let refused = if is_attack_order(entry) {
            crate::ui_action::attack_actor_refusal(Some(pet_store), pet.self_guid.0, ui_errors)
                || match pick.target(None, selection, seam, ui_errors) {
                    Some(guid) => {
                        target_guid = guid;
                        false
                    }
                    None => true,
                }
        } else {
            false
        };
        if !commit_press(bar, entry, refused, possessing) {
            debug!("ui_pet: slot {slot} refused by the attack validator — no packet");
            return PetCast::Done;
        }
        if let Some(spell) = spell {
            arm_pet_gcd(
                bar,
                entry.action(),
                spell,
                possessing,
                spell_mods,
                Instant::now(),
            );
        }
        debug!(
            "ui_pet: press slot {slot} (action {} kind {:#04x}) at {target_guid:#x}",
            entry.action(),
            entry.kind()
        );
        let _ = commands.0.send(ClientCommand::PetAction {
            pet_guid,
            packed: entry.packed,
            target_guid,
        });
        PetCast::Done
    }
}

/// The spell arm's tail for a press that sends `CMSG_PET_ACTION`: `Spell_C_StartSpellCooldown`
/// (`0x5d4427`) before the send, the spell's own `StartRecovery*` pair into the pet's list. The
/// insert moves the list's generation, which carries its flush to
/// `bar::fire_pet_cooldown_events`. The client-targeted and possessed presses start it at their
/// cast's send instead (`Spell_C::SendPendingCast`, `0x80ac90`).
pub(super) fn arm_pet_gcd(
    bar: &mut PetBar,
    spell_id: u32,
    spell: &benilla_formats::SpellDisplay,
    possessing: bool,
    mods: &crate::spell::SpellModifiers,
    now: Instant,
) {
    if !possessing && !spell.allows_client_targeting() {
        bar.cooldowns.start_gcd(spell_id, spell, now, mods);
    }
}

/// Send the bar's other intents under [`PetBar`]'s pet guid: the autocast toggles, the stops and
/// the drag writes. A toggle asks the usability predicate first, so on an unusable pet it flips
/// nothing and sends nothing. The stops do not; the drag's writes were gated where the VM applied
/// them, on the answer the bar last pushed.
pub(super) fn drain_pet_actions(
    script: Option<NonSendMut<UiScript>>,
    mut bar: ResMut<PetBar>,
    commands: Res<NetCommands>,
    pet: PetUnit,
) {
    let Some(mut script) = script else {
        return;
    };
    let toggles = script.take_pet_autocast_toggles();
    let stops = script.take_pet_stop_attacks();
    let writes = script.take_pet_set_actions();
    if toggles.is_empty() && stops == 0 && writes.is_empty() {
        return;
    }
    let pet_guid = bar.spells.pet_guid;
    if pet_guid == 0 {
        debug!("ui_pet: dropping queued pet intents — the bar is gone");
        return;
    }
    // The repaint is `latch_press`'s alone, not every click's: a refused `TogglePetAutocast` (: any
    // right-click on a token) signals nothing, so after `OnClick`'s `SetChecked(0)` that button
    // stays unlit until the next repaint, as in the reference.
    for slot in toggles {
        if !pet.actions_usable(&bar) {
            debug!(
                "ui_pet: autocast toggle on slot {slot} refused, the pet's actions are not usable"
            );
            continue;
        }
        let Some(flipped) = toggle_slot_autocast(&mut bar, slot) else {
            continue;
        };
        debug!(
            "ui_pet: autocast {} for spell {} (slot {slot})",
            flipped.autocast_on(),
            flipped.action()
        );
        let _ = commands.0.send(ClientCommand::PetSetAction {
            pet_guid,
            // Lua slots are 1-based, the wire's 0-based; `slot_entry` already refused 0.
            entries: vec![(slot - 1, flipped.packed)],
        });
    }
    for _ in 0..stops {
        if stop_pet_attack(&mut bar, &commands) {
            debug!("ui_pet: stop attack");
        }
    }
    // The drag's `(0-based position, word)` pairs: mirrored here and sent as one batch, since the
    // server tells one pair from two by body size.
    for entries in writes {
        for &(position, packed) in &entries {
            if let Some(e) = bar.spells.bar.get_mut(position as usize) {
                *e = PetActionEntry::from(packed);
            }
        }
        debug!("ui_pet: bar write {entries:?}");
        let _ = commands
            .0
            .send(ClientCommand::PetSetAction { pet_guid, entries });
    }
}

/// `TogglePetAutocast`'s local half: flip bit 30 in place and return the whole word to send, or
/// `None` for a slot out of range or without bit 31. The reference then calls, which copies a spell
/// slot's word over the last spellbook entry equal to it under `& 0x3FFFFFFF`.
pub(super) fn toggle_slot_autocast(bar: &mut PetBar, slot: u32) -> Option<PetActionEntry> {
    let entry = slot_entry(bar, slot).filter(|e| e.autocast_allowed())?;
    let flipped = entry.with_autocast(!entry.autocast_on());
    *slot_entry_mut(bar, slot)? = flipped;
    if flipped.kind() == 1 {
        let key = flipped.packed & 0x3FFF_FFFF;
        if let Some(book) = bar
            .spells
            .spells
            .iter_mut()
            .rev()
            .find(|w| w.packed & 0x3FFF_FFFF == key)
        {
            *book = flipped;
        }
    }
    Some(flipped)
}

/// `PetStopAttack`'s core: send `CMSG_PET_STOP_ATTACK` and lower the latch, or do nothing when it
/// is already down. Returns whether it sent.
pub(super) fn stop_pet_attack(bar: &mut PetBar, commands: &NetCommands) -> bool {
    let pet_guid = bar.spells.pet_guid;
    if !bar.attacking || pet_guid == 0 {
        return false;
    }
    let _ = commands.0.send(ClientCommand::PetStopAttack { pet_guid });
    bar.attacking = false;
    true
}

/// The reference's entry gate: the old-target clear runs only when a selection existed and is being
/// replaced or dropped.
pub(super) fn old_target_cleared(previous: Option<u64>, now: Option<u64>) -> bool {
    previous.is_some() && previous != now
}

/// The old-target clear's `PetStopAttack` (at the reference when the reference finds a possessed
/// unit). It sits before the notify branch, so every selection writer runs it; a re-select runs no
/// clear.
pub(crate) fn pet_stop_on_old_target_clear(
    selection: Res<Selection>,
    mut previous: Local<Option<u64>>,
    mut bar: ResMut<PetBar>,
    commands: Res<NetCommands>,
) {
    let now = selection.guid;
    if *previous == now {
        return;
    }
    let cleared = old_target_cleared(*previous, now);
    *previous = now;
    if cleared && stop_pet_attack(&mut bar, &commands) {
        debug!("ui_pet: the old-target clear called the pet off");
    }
}

/// The unit the player possesses, which must be the bar's unit for an Attack press to latch. It
/// needs `UNIT_FLAG_POSSESSED` and charmed-by, else created-by, us, [`is_possessed_by`]; vmangos
/// sets that flag only with possession (Mind Control, Eyes of the Beast), never for an ordinary
/// pet. The reference also tests the active mover, not modelled here: vmangos sets the mover with
/// the flag.
pub(super) fn possessing(store: Option<&ObjectStore>, self_guid: Option<u64>) -> bool {
    is_possessed_by(store, self_guid)
}

/// The Attack order, the one press validated before it sends: the type-7 arm branches only on
/// `action <= 1` and `action == 2`, so every other command sends as it is.
pub(super) fn is_attack_order(entry: PetActionEntry) -> bool {
    entry.kind() == benilla_protocol::messages::PET_ACT_COMMAND
        && entry.action() == PET_COMMAND_ATTACK
}

/// Apply a press and answer whether it is sent. A veto from the validator (the pet as actor) jumps
/// past the send: no packet, no latch, no repaint. Failing the possession compare after it costs
/// only the latch.
pub(super) fn commit_press(
    bar: &mut PetBar,
    entry: PetActionEntry,
    refused: bool,
    possessing: bool,
) -> bool {
    if refused {
        return false;
    }
    latch_press(bar, entry, possessing);
    true
}

/// Latch a press's state, the half the server never confirms, with the client's masks. A reaction
/// writes byte 0. Stay and Follow, the only commands to reach the write (`action <= 1`), set bits
/// 8-15 and keep byte 0 and bit 27, which only the server writes. Attack raises a possessed unit's
/// attack latch instead.
pub(super) fn latch_press(bar: &mut PetBar, entry: PetActionEntry, possessing: bool) {
    let action = entry.action();
    match entry.kind() {
        benilla_protocol::messages::PET_ACT_COMMAND
            if action == PET_COMMAND_STAY || action == PET_COMMAND_FOLLOW =>
        {
            // The reference signals `PET_BAR_UPDATE` even when the command is unchanged.
            bar.spells.state = (bar.spells.state & 0x0800_00FF) | (action << 8);
            bar.bar_signals = bar.bar_signals.wrapping_add(1);
        }
        benilla_protocol::messages::PET_ACT_COMMAND
            if action == PET_COMMAND_ATTACK && possessing =>
        {
            // The reference signals too; an ordinary pet never gets here.
            bar.attacking = true;
            bar.bar_signals = bar.bar_signals.wrapping_add(1);
        }
        benilla_protocol::messages::PET_ACT_REACTION => {
            // The same, on byte 0.
            bar.spells.state = (bar.spells.state & 0xFFFF_FF00) | action;
            bar.bar_signals = bar.bar_signals.wrapping_add(1);
        }
        // Dismiss, spells and unknown types reach the send without a signal.
        _ => {}
    }
}

/// The word held for a 1-based Lua slot.
pub(super) fn slot_entry(bar: &PetBar, slot: u32) -> Option<PetActionEntry> {
    let index = usize::try_from(slot.checked_sub(1)?).ok()?;
    bar.spells.bar.get(index).copied()
}

pub(super) fn slot_entry_mut(bar: &mut PetBar, slot: u32) -> Option<&mut PetActionEntry> {
    let index = usize::try_from(slot.checked_sub(1)?).ok()?;
    bar.spells.bar.get_mut(index)
}
