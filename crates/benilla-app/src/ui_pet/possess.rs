//! The possess spell `[0xc234e0]`, which the stock possess bar (`BonusActionBarFrame.lua`) reads
//! through `IsPossessBarVisible` and `GetPossessInfo` when called. The possess-spell lookup
//! (`0x5d62a0`) takes it from the active player's own auras inside the handler that runs it: the
//! UI's world entry with a pet (`0x5d6ede`), our `UNIT_FIELD_CHARM` or `UNIT_FIELD_SUMMON` watch
//! (`0x6d1ab1`) and our far-sight watch when its subject resolves (`0x6e5034`); a far sight with no
//! subject clears it (`0x6e5042`), as does leaving the world. Every change fires
//! `UPDATE_BONUS_ACTIONBAR` (`0x5d30e0`).

use bevy::prelude::*;

use benilla_formats::{SpellCatalog, SummonPropertiesCatalog, SUMMON_CONTROL_POSSESSED};
use benilla_protocol::ObjectFields;
use benilla_ui::script::PossessView;

use crate::ui_script::live::UiEvents;
use crate::unit_auras::UnitAuras;

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

/// The possess spell `[0xc234e0]`, 0 for none, written by the handlers alone.
#[derive(Resource, Default)]
pub(crate) struct PossessSpell(pub(crate) u32);

/// `0x5d30e0`: hold `spell`, firing `UPDATE_BONUS_ACTIONBAR` when it changed.
pub(crate) fn set_possess_spell(held: &mut PossessSpell, spell: u32, ui: &mut UiEvents) {
    if std::mem::replace(&mut held.0, spell) != spell {
        debug!("ui_pet: possess spell -> {spell}");
        ui.fire("UPDATE_BONUS_ACTIONBAR", Vec::new());
    }
}

/// `0x5d62a0` for the active player's fields and auras, `riding` its transport: a found spell is
/// held, no find keeps the held one.
pub(crate) fn find_possessed(
    me: &ObjectFields,
    auras: Option<&UnitAuras>,
    riding: Option<u64>,
    catalogs: Option<(&SpellCatalog, &SummonPropertiesCatalog)>,
    held: &mut PossessSpell,
    ui: &mut UiEvents,
) {
    let Some((spells, summons)) = catalogs else {
        return;
    };
    // We ride our charm, else our summon, while our transport is it (`0x5d62f9`).
    let on_charm_vehicle = riding.is_some() && me.unit_pet_guid() == riding;
    if let Some(found) = find_possessed_spell(auras, on_charm_vehicle, spells, summons) {
        set_possess_spell(held, found, ui);
    }
}

/// The world entry's lookup owed by the pet ask that went out (`0x5d6ede`, behind the send at
/// `0x5d6ed6`), run after `PLAYER_ENTERING_WORLD` (`0x5281a6` before `0x5281fe`).
#[derive(Resource, Default)]
pub(crate) struct PossessLookupOwed(pub(crate) bool);

/// The world entry's pet step after `PLAYER_ENTERING_WORLD` (`0x5281fe`): the owed lookup, its
/// event fired in turn, ahead of the reputation's entry (`0x528208`).
pub(crate) fn world_entry_step(world: &mut World) {
    let owed = world
        .get_resource_mut::<PossessLookupOwed>()
        .is_some_and(|mut o| std::mem::take(&mut o.0));
    if !owed {
        return;
    }
    if let Err(e) = world.run_system_cached(world_entry_lookup) {
        warn!("ui_pet: the world entry's possess lookup did not run: {e}");
    }
    crate::ui_script::live::fire_handler_events(world);
}

/// Our fields, auras and transport seat, as the lookup reads them.
type OurPossessRead = (
    &'static crate::net::ObjectStore,
    Option<&'static UnitAuras>,
    Option<&'static crate::transport::TransportRider>,
);

fn world_entry_lookup(
    me: Query<OurPossessRead, With<crate::net::SelfPlayer>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    summons: Option<Res<crate::spell::SummonSlots>>,
    mut held: ResMut<PossessSpell>,
    mut ui: ResMut<UiEvents>,
) {
    let Ok((store, auras, rider)) = me.single() else {
        return;
    };
    let catalogs = spells
        .as_deref()
        .map(|s| &s.catalog)
        .zip(summons.as_deref().map(|s| &s.0));
    find_possessed(
        &store.0,
        auras,
        rider.map(|r| r.transport_guid),
        catalogs,
        &mut held,
        &mut ui,
    );
}

/// What the possess verbs read when called: the held spell and its record.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PossessLive<'w> {
    held: Option<Res<'w, PossessSpell>>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
}

impl PossessLive<'_> {
    /// The held spell's view, `None` while none is held.
    pub(crate) fn view(&self) -> Option<PossessView> {
        let spell = self.held.as_ref()?.0;
        let spells = self.spells.as_deref()?;
        (spell != 0).then(|| possess_view(spell, &spells.catalog))
    }
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
