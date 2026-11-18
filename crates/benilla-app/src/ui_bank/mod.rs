//! The bank window's app side: the open banker the verbs read live and the events its changes fire where 12340 fires them (`0x57b8d0`).

use benilla_protocol::messages::bank_slot_result;
use bevy::prelude::*;

use crate::net::{GuidIndex, SelfPlayer};
use crate::ui_script::live::UiEvents;
use crate::ui_session::npc_out_of_range;

/// `BankBagSlotPrices.dbc`, the client's own slot prices; absent, the purchase row shows 0 and
/// the server still charges its own price.
#[derive(Resource)]
pub(crate) struct BankPrices(pub(crate) benilla_formats::BankBagSlotPrices);

/// The open bank: the banker (`0xbeb9a0`), and whether the interact target is it (`0xbd07a8`). Main-thread state the bank verbs write in their call ([`crate::ui_script::live::Lent`]).
#[derive(Default)]
pub(crate) struct BankOpen {
    pub(crate) banker: Option<u64>,
    interacting: bool,
}

impl BankOpen {
    pub(crate) fn is_open(&self) -> bool {
        self.banker.is_some()
    }

    /// The interact target while the banker holds it (`0xbd07a8`).
    pub(crate) fn interact_target(&self) -> Option<u64> {
        self.banker.filter(|_| self.interacting)
    }

    /// The interaction close's bank leg with the interact target the banker (`0x518d50`), whether it was.
    pub(crate) fn close_interaction(&mut self) -> bool {
        std::mem::take(&mut self.interacting) && self.banker.is_some()
    }

    /// The banker setter's clear with no banker (`0x57b8f9`).
    pub(crate) fn clear(&mut self) {
        self.banker = None;
        self.interacting = false;
    }
}

mod net;

pub(crate) struct UiBankPlugin;

impl Plugin for UiBankPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_non_send_resource::<BankOpen>().add_systems(
            Update,
            // Range-close ahead of the press, as the walk-away watchdog runs before input.
            close_bank_out_of_range.before(crate::ui_script::UiPress),
        );
    }
}

/// The `ERR_BANKSLOT_*` line for a `SMSG_BUY_BANK_SLOT_RESULT` code. The reference's handler
/// ignores any code `>= 3`, `OK` included, and maps the rest through the three-entry table, message
/// ids `0x100..=0x102`.
fn bank_slot_error_key(result: u32) -> Option<&'static str> {
    Some(match result {
        bank_slot_result::FAILED_TOO_MANY => "ERR_BANKSLOT_FAILED_TOO_MANY", // 0x100
        bank_slot_result::INSUFFICIENT_FUNDS => "ERR_BANKSLOT_INSUFFICIENT_FUNDS", // 0x101
        bank_slot_result::NOTBANKER => "ERR_BANKSLOT_NOTBANKER",             // 0x102
        _ => return None,
    })
}

/// `SMSG_SHOW_BANK`'s banker setter call (`0x57b908`): the interact target's change closes an open bank through the interaction close's bank leg first (`0x51fce0` -> `0x512fa7`), then the banker is set and `BANKFRAME_OPENED` fires (`0x57b926`).
pub(crate) fn open_bank(world: &mut World, banker: u64) {
    if world
        .get_non_send_resource::<BankOpen>()
        .is_some_and(|b| b.interacting && b.is_open())
    {
        close_by_interaction(world);
    }
    let Some(mut ui) = world.get_resource_mut::<UiEvents>() else {
        return;
    };
    ui.world(move |world| {
        let Some(mut bank) = world.get_non_send_resource_mut::<BankOpen>() else {
            return;
        };
        debug!("net: bank opened at {banker:#x}");
        bank.banker = Some(banker);
        bank.interacting = true;
        if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
            ui.fire("BANKFRAME_OPENED", Vec::new());
        }
    });
}

/// The interaction close's bank leg (`0x512fa7`), taken only while the interact target is the banker (`0x512f8a`): the target zeroed, `BANKFRAME_CLOSED` fires with the banker still set (`0x57b8e6`), then the banker clears (`0x57b8f9`). Queued on [`UiEvents`] for the caller's flush.
pub(crate) fn close_by_interaction(world: &mut World) {
    let Some(mut bank) = world.get_non_send_resource_mut::<BankOpen>() else {
        return;
    };
    if !bank.interacting || !bank.is_open() {
        return;
    }
    bank.interacting = false;
    let Some(mut ui) = world.get_resource_mut::<UiEvents>() else {
        world.non_send_resource_mut::<BankOpen>().clear();
        return;
    };
    ui.fire("BANKFRAME_CLOSED", Vec::new());
    ui.world(clear_banker);
}

/// `SpellShapeshiftForm` `flags1`: a stance, which the shapeshift test does not count (`0x721cdd`).
const FORM_STANCE: u32 = 0x1;
/// `SpellShapeshiftForm` `flags1`: the form keeps NPC interaction (`0x6da1cf`).
const FORM_NPC_INTERACT: u32 = 0x8;

/// The form byte's watch (`0x6da130`): shapeshifted into a form without the NPC-interact bit, the interact target clears (`0x6da1dc` -> `0x512e60`), the bank's leg with it.
pub(crate) fn form_entered(world: &mut World, form: u8) {
    let clears = form != 0
        && world
            .get_resource::<crate::ui_action::Spells>()
            .and_then(|s| s.forms.get(&u32::from(form)))
            .is_some_and(|f| f.flags & (FORM_STANCE | FORM_NPC_INTERACT) == 0);
    if clears {
        close_by_interaction(world);
    }
}

/// The banker setter with no banker from the NPC-flag watch's bank leg (`0x71a1a8`-`0x71a1db`): `BANKFRAME_CLOSED`, then with the interact target the banker the interaction close fires it again (`0x57b8f1` -> `0x512fa7`), then the banker clears.
pub(crate) fn set_banker_none(world: &mut World) {
    let Some(mut ui) = world.get_resource_mut::<UiEvents>() else {
        return;
    };
    ui.fire("BANKFRAME_CLOSED", Vec::new());
    ui.world(|world| {
        let again = world
            .get_non_send_resource_mut::<BankOpen>()
            .is_some_and(|mut b| b.close_interaction());
        if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
            if again {
                ui.fire("BANKFRAME_CLOSED", Vec::new());
            }
            ui.world(clear_banker);
        }
    });
}

fn clear_banker(world: &mut World) {
    if let Some(mut bank) = world.get_non_send_resource_mut::<BankOpen>() {
        bank.clear();
    }
}

/// The walk-away watchdog for the bank: past its leash or with its banker gone, the interaction closes (`0x512e60`), its events fired as it runs.
fn close_bank_out_of_range(
    bank: NonSend<BankOpen>,
    index: Res<GuidIndex>,
    self_q: Query<&Transform, With<SelfPlayer>>,
    transforms: Query<&Transform>,
    mut commands: Commands,
) {
    let Some(banker) = bank.banker.filter(|_| bank.interacting) else {
        return;
    };
    if npc_out_of_range(banker, &index, &self_q, &transforms) == Some(true) {
        debug!("ui_bank: banker {banker:#x} out of range/gone — closing the interaction");
        commands.queue(|world: &mut World| {
            close_by_interaction(world);
            crate::ui_script::live::fire_handler_events(world);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bank_slot_error_keys() {
        assert_eq!(
            bank_slot_error_key(bank_slot_result::FAILED_TOO_MANY),
            Some("ERR_BANKSLOT_FAILED_TOO_MANY")
        );
        assert_eq!(
            bank_slot_error_key(bank_slot_result::INSUFFICIENT_FUNDS),
            Some("ERR_BANKSLOT_INSUFFICIENT_FUNDS")
        );
        assert_eq!(
            bank_slot_error_key(bank_slot_result::NOTBANKER),
            Some("ERR_BANKSLOT_NOTBANKER")
        );
        assert_eq!(bank_slot_error_key(bank_slot_result::OK), None);
        assert_eq!(
            bank_slot_error_key(99),
            None,
            "past the reference's own bound"
        );
        // The catalog's rows; insufficient funds also speaks.
        for (key, id) in [
            ("ERR_BANKSLOT_FAILED_TOO_MANY", 0x10eu16),
            ("ERR_BANKSLOT_INSUFFICIENT_FUNDS", 0x10f),
            ("ERR_BANKSLOT_NOTBANKER", 0x110),
        ] {
            let r = benilla_ui::messages::by_key(key).expect("catalog row");
            assert_eq!(r.id, id, "{key}");
            assert_eq!(r.kind, benilla_ui::messages::MsgKind::Error, "{key}");
        }
        assert_eq!(
            benilla_ui::messages::by_key("ERR_BANKSLOT_INSUFFICIENT_FUNDS")
                .unwrap()
                .type_tag,
            0x16
        );
    }
}
