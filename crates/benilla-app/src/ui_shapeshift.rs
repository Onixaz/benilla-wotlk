//! The stance bar's casts behind `CastShapeshiftForm`, `CancelShapeshiftForm` and `Dismount`.
//! The bar itself is the stance list the registrar keeps (`[0xbe8e2c]`,
//! [`crate::ui_action::usable_cache`]), read at the call through the live view.

use bevy::prelude::*;

use crate::net::ClientCommand;
use crate::spell::CastCommit;

/// `Dismount()` (`0x51d170`, `0x7412e0`): alive and not on a taxi, `CMSG_CANCEL_MOUNT_AURA`.
pub(crate) fn dismount(cast: &mut crate::spell::ScriptCast) {
    let store = cast.targeting.self_store.iter().next();
    let on_taxi =
        store.is_some_and(|s| s.0.unit_flags() & crate::player::UNIT_FLAG_TAXI_FLIGHT != 0);
    if store.is_some_and(|s| !s.0.is_dead_or_ghost()) && !on_taxi {
        let _ = cast.ladder.commands.0.send(ClientCommand::CancelMountAura);
    }
}

/// The cancelable auras holding `form`, as the auto-unshift's cancel walks them.
pub(crate) fn form_auras(
    form: u8,
    auras: &crate::unit_auras::UnitAuras,
    catalog: &benilla_formats::SpellCatalog,
) -> Vec<u32> {
    auras
        .iter()
        .filter(|a| {
            a.is_cancelable()
                && catalog
                    .get(a.spell_id)
                    .is_some_and(|d| d.shapeshift_form == Some(u32::from(form)))
        })
        .map(|a| a.spell_id)
        .collect()
}

/// `CancelShapeshiftForm()` (`0x726ce0`): every cancelable aura of the active form's spell, unless
/// charmed or the form's row forbids it (`0x802f80`).
pub(crate) fn cancel_form(cast: &mut crate::spell::ScriptCast) {
    let crate::spell::ScriptCast {
        targeting, ladder, ..
    } = cast;
    let Some(store) = targeting.self_store.iter().next() else {
        return;
    };
    let form_byte = store.0.unit_shapeshift_form();
    if form_byte == 0 || store.0.unit_charmed_by().is_some() {
        return;
    }
    let Some(spells) = ladder.spells.as_ref() else {
        return;
    };
    let row = spells.forms.get(&u32::from(form_byte));
    let Some(auras) = targeting.self_auras.iter().next() else {
        return;
    };
    for aura in auras.iter().filter(|a| a.is_cancelable()) {
        let Some(d) = spells.catalog.get(aura.spell_id) else {
            continue;
        };
        if d.shapeshift_form.unwrap_or(0) == u32::from(form_byte)
            && crate::ui_action::toggle::form_recast_disposition(d, form_byte, row) == Some(true)
        {
            let _ = ladder.commands.0.send(ClientCommand::CancelAura {
                spell_id: aura.spell_id,
            });
        }
    }
}

/// `CastShapeshiftForm`, forked on the form id like the info call: the active form cancels unless
/// `SpellShapeshiftForm.dbc` `flags1 & 0x2` makes it a silent no-op (warrior stances); a
/// force-admitted spell whose aura is up cancels, with no DBC guard since it has no form id.
/// Anything else casts.
pub(crate) fn cast_form(cast: &mut crate::spell::ScriptCast, spell_id: u32) {
    let crate::spell::ScriptCast {
        targeting, ladder, ..
    } = cast;
    let store = targeting.self_store.iter().next();
    let form_byte = store.map(|s| s.0.unit_shapeshift_form()).unwrap_or(0);
    let d = ladder.spells.as_ref().and_then(|s| s.catalog.get(spell_id));
    // The active form's row, for the guard (shared with `crate::ui_action::toggle`).
    let row = ladder
        .spells
        .as_ref()
        .and_then(|s| s.forms.get(&u32::from(form_byte)));
    // The force-admit arm uses `form_active`'s predicate, so a lit button always cancels.
    let disposition = d.and_then(|d| {
        if d.shapeshift_form.unwrap_or(0) == 0 {
            let auras = targeting.self_auras.iter().next();
            crate::ui_action::toggle::active_action_toggle(spell_id, d, auras).then_some(true)
        } else {
            crate::ui_action::toggle::form_recast_disposition(d, form_byte, row)
        }
    });
    match disposition {
        Some(true) if ladder.keeps_fresh_toggle(spell_id) => {
            debug!("ui_shapeshift: form {spell_id} was just cast, kept");
        }
        Some(true) => {
            debug!("ui_shapeshift: cancel form aura {spell_id}");
            let _ = ladder
                .commands
                .0
                .send(ClientCommand::CancelAura { spell_id });
        }
        Some(false) => {}
        None => {
            debug!("ui_shapeshift: cast form {spell_id}");
            ladder.send(spell_id, &targeting.context(), CastCommit::Spell);
        }
    }
}
