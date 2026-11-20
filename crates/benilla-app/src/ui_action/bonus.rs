//! The bar's pages: the temporary bar's owners `[0xc1e5a0]` and the bonus offset `[0xc1e59c]`
//! that `GetBonusBarOffset` (`0x5a7f20`) and the macro option `bonusbar` read at the call. A form
//! listing preset spells takes the temporary bar under flag `0x2` (`0x5abac0`); the bonus update
//! (`0x5a83c0`) stores 5 while any owner holds the bar or the form has preset spells, else the
//! form's `BonusActionBar`, and signals a change. Both run in the turn of the handler that moved
//! them: world entry (`0x5ac0e0`), the pet's bar (`0x5d4c0f`), the override field (`0x6d1c62`),
//! the form field (`0x6da38e`) and the player's init (`0x6e8063`).

use bevy::prelude::*;

use crate::net::{ObjectStore, SelfPlayer};
use crate::ui_script::live::UiEvents;

use super::{ActionSlots, Spells, TempActionBar, TEMP_BAR_FIRST_SLOT};

/// A form's preset bar's temporary-bar flag.
pub(crate) const TEMP_BAR_FORM: u32 = 0x2;

/// The page every temporary bar shows.
const TEMP_PAGE: u8 = 5;

/// Our form byte, `None` with no player.
fn own_form(world: &mut World) -> Option<u8> {
    let mut q = world.query_filtered::<&ObjectStore, With<SelfPlayer>>();
    q.iter(world).next().map(|s| s.0.unit_shapeshift_form())
}

/// The form's row, `None` for no form or one the table lacks.
fn form_row(world: &World, form: u8) -> Option<&benilla_formats::ShapeshiftForm> {
    world.get_resource::<Spells>()?.forms.get(&u32::from(form))
}

/// Write the temporary bar's twelve slots, each set without a send and signalled, moved or not
/// (`0x5ab800`).
pub(crate) fn write_temp_bar(world: &mut World, words: [u32; 12]) {
    for (i, packed) in words.into_iter().enumerate() {
        let slot = TEMP_BAR_FIRST_SLOT + i as u8;
        if let Some(mut actions) = world.get_resource_mut::<ActionSlots>() {
            actions.set_local(slot, packed);
        }
        super::slots::slot_changed(world, slot);
    }
}

/// `0x5abac0`: a form listing preset spells writes them to the temporary bar, then four empty
/// slots (`0x5a7cd0`), and takes flag `0x2`; any other form, or none, gives the flag back and
/// leaves the slots as written.
pub(crate) fn update_form_bar(world: &mut World) {
    let preset = own_form(world)
        .and_then(|form| form_row(world, form))
        .filter(|row| row.has_preset_bar())
        .map(|row| row.preset_spells);
    if let Some(spells) = preset {
        write_temp_bar(
            world,
            std::array::from_fn(|i| spells.get(i).copied().unwrap_or(0)),
        );
    }
    if let Some(mut temp) = world.get_resource_mut::<TempActionBar>() {
        match preset {
            Some(_) => temp.flags |= TEMP_BAR_FORM,
            None => temp.flags &= !TEMP_BAR_FORM,
        }
    }
}

/// The offset `0x5a83c0` stores: 0 with no player, 5 while the temporary bar has an owner, else 5
/// for a form with preset spells and the form's `BonusActionBar` for any other row.
fn offset(world: &mut World, flags: u32) -> u8 {
    let Some(form) = own_form(world) else {
        return 0;
    };
    if flags != 0 {
        return TEMP_PAGE;
    }
    match form_row(world, form) {
        Some(row) if row.has_preset_bar() => TEMP_PAGE,
        Some(row) => u8::try_from(row.bonus_bar).unwrap_or(0),
        None => 0,
    }
}

/// `0x5a83c0`: store the offset, and on a change fire `ACTIONBAR_PAGE_CHANGED` when it leaves 5,
/// then `UPDATE_BONUS_ACTIONBAR`, in the calling handler's turn.
pub(crate) fn update_bonus_bar(world: &mut World) {
    let Some(temp) = world.get_resource::<TempActionBar>().copied() else {
        return;
    };
    let offset = offset(world, temp.flags);
    if offset == temp.bonus_offset {
        return;
    }
    debug!("ui_action: bonus bar offset {offset}");
    if let Some(mut stored) = world.get_resource_mut::<TempActionBar>() {
        stored.bonus_offset = offset;
    }
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        if temp.bonus_offset == TEMP_PAGE {
            ui.fire("ACTIONBAR_PAGE_CHANGED", Vec::new());
        }
        ui.fire("UPDATE_BONUS_ACTIONBAR", Vec::new());
    }
}

/// The bar's world entry (`0x5ac0e0`): the usable pass, the form's preset bar, the override bar,
/// then the bonus update.
pub(crate) fn enter_world(world: &mut World) {
    super::usable_cache::step(super::usable_cache::Pass::Bar)(world);
    update_form_bar(world);
    crate::vehicle::update_override_bar(world);
    update_bonus_bar(world);
}

/// The form field's and the init's bar tail (`0x6da38e`, `0x6e8063`): the form's preset bar, then
/// the bonus update.
pub(crate) fn form_changed(world: &mut World) {
    update_form_bar(world);
    update_bonus_bar(world);
}

/// The override field's bar tail (`0x6d1c5d`): the override bar, then the bonus update.
pub(crate) fn override_changed(world: &mut World) {
    crate::vehicle::update_override_bar(world);
    update_bonus_bar(world);
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::field::FIELD_UNIT_BYTES_2;

    const BATTLE_STANCE: u8 = 17;
    const PRESET_FORM: u8 = 40;

    /// Our player in `form`, the stance (bonus bar 1) and a form listing three preset spells.
    fn world(form: u8) -> World {
        let mut world = World::new();
        let mut spells = Spells::empty_for_tests();
        spells.forms.insert(
            u32::from(BATTLE_STANCE),
            benilla_formats::ShapeshiftForm {
                bonus_bar: 1,
                flags: 1,
                ..Default::default()
            },
        );
        spells.forms.insert(
            u32::from(PRESET_FORM),
            benilla_formats::ShapeshiftForm {
                bonus_bar: 2,
                preset_spells: [101, 102, 103, 0, 0, 0, 0, 0],
                ..Default::default()
            },
        );
        world.insert_resource(spells);
        world.init_resource::<TempActionBar>();
        world.init_resource::<crate::ui_action::ActionSlots>();
        world.init_resource::<UiEvents>();
        world.spawn((
            SelfPlayer,
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                FIELD_UNIT_BYTES_2,
                u32::from(form) << 24,
            )])),
        ));
        world
    }

    /// The events queued, emptied.
    fn fired(world: &mut World) -> Vec<&'static str> {
        let mut ui = world.resource_mut::<UiEvents>();
        let names = ui.names();
        *ui = UiEvents::default();
        names
    }

    fn offset_of(world: &World) -> u8 {
        world.resource::<TempActionBar>().bonus_offset
    }

    /// A stance pages the bar to its `BonusActionBar` and signals once; the same form again moves
    /// nothing and fires nothing (`0x5a8433`).
    #[test]
    fn a_stance_pages_the_bar_and_signals_only_a_change() {
        let mut world = world(BATTLE_STANCE);
        form_changed(&mut world);
        assert_eq!(offset_of(&world), 1);
        assert_eq!(fired(&mut world), ["UPDATE_BONUS_ACTIONBAR"]);
        form_changed(&mut world);
        assert!(fired(&mut world).is_empty());
    }

    /// An owner of the temporary bar pages it to 5 over the form; leaving 5 fires
    /// `ACTIONBAR_PAGE_CHANGED` before `UPDATE_BONUS_ACTIONBAR` (`0x5a844e`, `0x5a845d`).
    #[test]
    fn leaving_the_temporary_page_fires_the_page_change_first() {
        let mut world = world(BATTLE_STANCE);
        world.resource_mut::<TempActionBar>().flags = 0x1;
        update_bonus_bar(&mut world);
        assert_eq!(offset_of(&world), 5);
        assert_eq!(fired(&mut world), ["UPDATE_BONUS_ACTIONBAR"]);
        world.resource_mut::<TempActionBar>().flags = 0;
        update_bonus_bar(&mut world);
        assert_eq!(offset_of(&world), 1);
        assert_eq!(
            fired(&mut world),
            ["ACTIONBAR_PAGE_CHANGED", "UPDATE_BONUS_ACTIONBAR"]
        );
    }

    /// A form listing preset spells takes the temporary bar: its spells, then nothing, each of the
    /// twelve slots signalled, flag `0x2`, and the page 5 over its own `BonusActionBar`
    /// (`0x5abac0`, `0x5a841a`); leaving it gives the flag back and leaves the slots.
    #[test]
    fn a_form_with_preset_spells_takes_the_temporary_bar() {
        let mut world = world(PRESET_FORM);
        form_changed(&mut world);
        let slots = world.resource::<crate::ui_action::ActionSlots>();
        let words: Vec<u32> = (0..12)
            .map(|i| {
                slots
                    .buttons
                    .get(&(TEMP_BAR_FIRST_SLOT + i))
                    .map_or(0, |b| b.action)
            })
            .collect();
        assert_eq!(words, [101, 102, 103, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(world.resource::<TempActionBar>().flags, TEMP_BAR_FORM);
        assert_eq!(offset_of(&world), 5);
        let names = fired(&mut world);
        assert_eq!(
            names
                .iter()
                .filter(|&&n| n == "ACTIONBAR_SLOT_CHANGED")
                .count(),
            12
        );
        assert_eq!(names.last(), Some(&"UPDATE_BONUS_ACTIONBAR"));

        *world
            .query_filtered::<&mut ObjectStore, With<SelfPlayer>>()
            .single_mut(&mut world)
            .unwrap() = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
            FIELD_UNIT_BYTES_2,
            0,
        )]));
        form_changed(&mut world);
        assert_eq!(world.resource::<TempActionBar>().flags, 0);
        assert_eq!(offset_of(&world), 0);
        assert_eq!(
            fired(&mut world),
            ["ACTIONBAR_PAGE_CHANGED", "UPDATE_BONUS_ACTIONBAR"]
        );
        let slots = world.resource::<crate::ui_action::ActionSlots>();
        assert!(
            slots.buttons.contains_key(&TEMP_BAR_FIRST_SLOT),
            "left as written"
        );
    }

    /// No player stores 0 (`0x5a83e0`).
    #[test]
    fn no_player_pages_to_zero() {
        let mut world = world(BATTLE_STANCE);
        form_changed(&mut world);
        let me = world
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(&world)
            .unwrap();
        world.despawn(me);
        fired(&mut world);
        update_bonus_bar(&mut world);
        assert_eq!(offset_of(&world), 0);
    }
}
