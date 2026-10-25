//! The calendar's one login-time feed: `SMSG_CALENDAR_SEND_NUM_PENDING`'s count, which the stock
//! clock button reads through `CalendarGetNumPendingInvites` on every world entry. The date
//! `CalendarGetDate` answers rides the game clock ([`crate::minimap`]'s minute feed).

use bevy::prelude::*;

use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::UiScript;

use crate::net::NetHandlerApp;
use crate::ui_script::{UiFeed, VmMemo};

/// The server's latest pending-invite count this session; `None` before its packet.
#[derive(Resource, Default)]
struct PendingInvites(Option<u32>);

fn on_net(In(ev): In<SessionEvent>, mut pending: ResMut<PendingInvites>) {
    match ev {
        SessionEvent::CalendarNumPending { count } => pending.0 = Some(count),
        SessionEvent::LoggedOut => pending.0 = None,
        _ => {}
    }
}

/// Push a new count into the VM, and the session's count into a VM built after it arrived.
fn feed_pending_invites(
    script: Option<NonSendMut<UiScript>>,
    pending: Res<PendingInvites>,
    mut last: Local<VmMemo<Option<u32>>>,
) {
    let Some(mut script) = script else { return };
    let last = last.get(&script);
    if *last == pending.0 {
        return;
    }
    *last = pending.0;
    if let Some(count) = pending.0 {
        script.set_calendar_pending_invites(count);
    }
}

pub(crate) struct UiCalendarPlugin;

impl Plugin for UiCalendarPlugin {
    fn build(&self, app: &mut App) {
        app.net_handler(SessionEventKind::CalendarNumPending, on_net)
            .net_handler(SessionEventKind::LoggedOut, on_net)
            .init_resource::<PendingInvites>()
            .add_systems(
                Update,
                // Ahead of the world-entry fire, whose stock clock button reads the count.
                feed_pending_invites
                    .in_set(UiFeed)
                    .before(crate::ui_unit::EnterWorldFeed),
            );
    }
}
