//! The possess spell `[0xc234e0]`, which the stock possess bar (`BonusActionBarFrame.lua`) reads
//! through `IsPossessBarVisible` and `GetPossessInfo`. `CGPetInfo::FindPossessedSpell`
//! (`0x5d62a0`) takes it from the active player's own auras; it runs at world entry
//! (`SendRequestPetInfo`, `0x5d6ede`), on a move of our `UNIT_FIELD_CHARM` or `UNIT_FIELD_SUMMON`
//! (`0x6d1ab1`) and on a far-sight change whose subject resolves (`0x6e5034`); a far sight with no
//! subject clears it (`0x6e5042`), as does leaving the world (`0x5d3360`). Every change fires
//! `UPDATE_BONUS_ACTIONBAR` (`0x5d30e0`).

use bevy::prelude::*;

use benilla_formats::{SpellCatalog, SummonPropertiesCatalog, SUMMON_CONTROL_POSSESSED};
use benilla_ui::script::{PossessView, UiScript};

use crate::unit_auras::UnitAuras;

use super::PetUnit;

/// The aura types that make an aura's spell the possess spell (`0x5d63a7`-`0x5d63c4`):
/// `MOD_POSSESS`, `MOD_POSSESS_PET`, `MOD_CHARM`.
const POSSESS_AURAS: [u32; 3] = [2, 128, 6];
/// `CONTROL_VEHICLE`, which counts only while we ride our own charm (`0x5d63c4`-`0x5d63cb`), and
/// hides the bar (`IsPossessBarVisible`, `0x5a8872`).
const SPELL_AURA_CONTROL_VEHICLE: u32 = 236;
/// `SPELL_EFFECT_SUMMON`, whose `SummonProperties` row may name a possession (`0x5d63f1`).
const SPELL_EFFECT_SUMMON: u32 = 28;
/// The aura flag that skips an aura (`0x5d6378`): the harmful bit.
const AURA_FLAG_HARMFUL: u8 = benilla_protocol::messages::AURA_UPDATE_FLAG_HARMFUL;
/// `SpellIcon.dbc` row of the possess bar's cancel button (`0x5d5973`).
const POSSESS_CANCEL_ICON: u32 = 693;
/// `Attributes`' sign bit, which disables the cancel button (`0x5d59c7`).
const ATTR_CANCEL_DISABLED: u32 = 0x8000_0000;

/// What the feed holds between frames.
#[derive(Default)]
pub(super) struct PossessMemory {
    /// `[0xc234e0]`, 0 for none.
    spell: u32,
    /// The active player's object was in the world last frame: its arrival is the world entry.
    in_world: bool,
}

/// `0x5d62a0`'s walk: the first non-harmful aura, by slot, whose spell applies a possess aura,
/// or a vehicle's control aura while `on_charm_vehicle`, or summons through a possessed row.
/// `None` leaves the held spell as it is: the walk sets, it never clears.
pub(super) fn find_possessed_spell(
    auras: Option<&UnitAuras>,
    on_charm_vehicle: bool,
    spells: &SpellCatalog,
    summons: &SummonPropertiesCatalog,
) -> Option<u32> {
    auras?
        .iter()
        .filter(|a| a.flags & AURA_FLAG_HARMFUL == 0)
        .find(|a| {
            let Some(spell) = spells.get(a.spell_id) else {
                return false;
            };
            let aura = spell.effect_apply_aura.iter().any(|&t| {
                POSSESS_AURAS.contains(&t) || (t == SPELL_AURA_CONTROL_VEHICLE && on_charm_vehicle)
            });
            aura || spell
                .effects
                .iter()
                .zip(spell.effect_misc_value_b)
                .any(|(&e, row)| {
                    e == SPELL_EFFECT_SUMMON
                        && u32::try_from(row)
                            .is_ok_and(|row| summons.control(row) == Some(SUMMON_CONTROL_POSSESSED))
                })
        })
        .map(|a| a.spell_id)
}

/// The two verbs' reads of the held spell's record.
pub(super) fn possess_view(spell_id: u32, spells: &SpellCatalog) -> PossessView {
    let spell = spells.get(spell_id);
    PossessView {
        spell_id,
        bar_visible: !spell
            .is_some_and(|s| s.effect_apply_aura.contains(&SPELL_AURA_CONTROL_VEHICLE)),
        name: spell.map(|s| s.name.clone()),
        icon: spell.and_then(|s| {
            if s.active_icon_id != 0 {
                s.active_icon.clone()
            } else {
                s.icon.clone()
            }
        }),
        cancel_icon: spells.icon(POSSESS_CANCEL_ICON).map(str::to_string),
        cancel_enabled: !spell.is_some_and(|s| s.attributes & ATTR_CANCEL_DISABLED != 0),
    }
}

/// Follow the possess spell and push it, firing `UPDATE_BONUS_ACTIONBAR` on a change.
pub(super) fn feed_possess_spell(
    script: Option<NonSendMut<UiScript>>,
    pet: PetUnit,
    mut field_edges: MessageReader<crate::net::FieldChanged>,
    spells: Option<Res<crate::ui_action::Spells>>,
    summons: Option<Res<crate::spell::SummonSlots>>,
    mut memory: Local<crate::ui_script::VmMemo<PossessMemory>>,
) {
    let Some(mut script) = script else {
        field_edges.clear();
        return;
    };
    let (memory, vm_reset) = memory.get_reset(&script);
    let edges = crate::net::FieldEdges::collect(&mut field_edges);
    let me = pet.self_guid.0;
    let player = pet.player_store();
    let in_world = player.is_some();
    let entered = in_world && (!memory.in_world || vm_reset);
    let left = !in_world && memory.in_world;
    memory.in_world = in_world;

    let moved =
        |field: u16| me.is_some_and(|me| edges.moved(me, field) || edges.moved(me, field + 1));
    use benilla_protocol::field::{FIELD_PLAYER_FARSIGHT, FIELD_UNIT_CHARM, FIELD_UNIT_SUMMON};
    let pet_moved = moved(FIELD_UNIT_CHARM) || moved(FIELD_UNIT_SUMMON);
    let farsight_moved = moved(FIELD_PLAYER_FARSIGHT);

    let mut next = memory.spell;
    if left {
        next = 0;
    }
    let mut find = entered || pet_moved;
    if farsight_moved {
        // The subject must resolve to an object (`0x6e5020`); a far sight cleared or not yet
        // streamed clears the spell instead.
        let subject = player.and_then(|p| p.0.player_farsight());
        if subject.is_some_and(|g| pet.store(g).is_some()) {
            find = true;
        } else {
            next = 0;
        }
    }
    if find {
        if let (Some(spells), Some(summons)) = (spells.as_deref(), summons.as_deref()) {
            // benilla seats no vehicle (#235), so we never ride our charm.
            let auras = me.and_then(|me| pet.auras(me));
            if let Some(found) = find_possessed_spell(auras, false, &spells.catalog, &summons.0) {
                next = found;
            }
        }
    }
    if next == memory.spell && !vm_reset {
        return;
    }
    let changed = next != memory.spell;
    memory.spell = next;
    let view = (next != 0)
        .then(|| spells.as_deref().map(|s| possess_view(next, &s.catalog)))
        .flatten();
    script.set_possess_spell(view);
    if changed {
        debug!("ui_pet: possess spell -> {next}");
        script.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    }
}
