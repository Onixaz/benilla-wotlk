//! The class trainer's respec question: `MSG_TALENT_WIPE_CONFIRM`'s handler runs the talent master talk (`0x6e3d71` -> `0x6db1c0`), which latches the trainer and cost and fires `CONFIRM_TALENT_WIPE`; `ConfirmTalentWipe` and `CheckTalentMasterDist` read the latch live at the call ([`crate::ui_binder::TalkView`]).

use benilla_ui::script::ScriptValue;
use bevy::prelude::*;

use crate::net::{GuidIndex, ObjectStore, SelfPlayer};
use crate::ui_script::live::UiEvents;

/// The latched trainer and cost (`0xc9eb18`, `0xc9eb0c`), set by the talk and never cleared.
#[derive(Resource, Default)]
pub(crate) struct TalentWipeState {
    npc: Option<u64>,
    cost: u32,
}

impl TalentWipeState {
    /// The latched trainer.
    pub(crate) fn npc(&self) -> Option<u64> {
        self.npc
    }

    /// The latched cost in copper.
    pub(crate) fn cost(&self) -> u32 {
        self.cost
    }
}

/// The talent master talk with a guid (`0x6db1c0`): a held trainer in reach latches with its cost (`0x6db26f`-`0x6db288`) and `CONFIRM_TALENT_WIPE` fires with the cost (`0x6db28d`).
fn talk_to_talent_master(
    In((npc, cost)): In<(u64, u32)>,
    mut wipe: ResMut<TalentWipeState>,
    me: Query<&Transform, With<SelfPlayer>>,
    index: Option<Res<GuidIndex>>,
    units: Query<(&ObjectStore, &Transform)>,
    mut ui: ResMut<UiEvents>,
) {
    if !crate::ui_binder::in_talk_reach(npc, me.single().ok(), index.as_deref(), &units) {
        debug!("ui_talent_wipe: trainer {npc:#x} not held in reach, no question");
        return;
    }
    wipe.npc = Some(npc);
    wipe.cost = cost;
    ui.fire(
        "CONFIRM_TALENT_WIPE",
        vec![ScriptValue::Int(i64::from(cost))],
    );
}

/// The talent-wipe question's packet handler.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::TalentWipeConfirm, on_confirm);
    }

    /// `MSG_TALENT_WIPE_CONFIRM`: a trainer guid runs the talk (`0x6e3d71`); a zero one shows `ERR_TALENT_WIPE_ERROR` (`0x6e3d82`, message `0x1c5`).
    fn on_confirm(In(ev): In<SessionEvent>, world: &mut World) {
        let SessionEvent::TalentWipeConfirm { trainer, cost } = ev else {
            return;
        };
        if trainer == 0 {
            if let Some(mut ui) = world.get_resource_mut::<crate::ui_script::live::UiEvents>() {
                crate::ui_action::show_in_turn(
                    &mut ui,
                    "ui_talent_wipe",
                    crate::ui_action::UiError::key("ERR_TALENT_WIPE_ERROR"),
                );
            }
            return;
        }
        debug!("net: trainer {trainer:#x} asks to wipe talents for {cost} copper");
        if let Err(e) = world.run_system_cached_with(super::talk_to_talent_master, (trainer, cost))
        {
            warn!("ui_talent_wipe: the talent master talk did not run: {e}");
        }
    }
}

pub(crate) struct UiTalentWipePlugin;

impl Plugin for UiTalentWipePlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<TalentWipeState>();
    }
}
