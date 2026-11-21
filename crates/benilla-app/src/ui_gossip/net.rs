//! The gossip window's packet handlers: each writes the gossip globals and fires its events in its turn.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::GossipState;
use crate::net::NetHandlerApp;

/// Register the gossip handlers and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::GossipMenu, on_gossip_menu)
        .net_handler(K::NpcGreeting, on_npc_greeting)
        .net_handler(K::NpcTextMiss, on_npc_text_miss)
        .net_handler(K::GossipComplete, on_gossip_complete)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_gossip_menu(In(ev): In<SessionEvent>, world: &mut World) {
    if let SessionEvent::GossipMenu(menu) = ev {
        super::gossip_menu(world, menu);
    }
}

/// `SMSG_NPC_TEXT_UPDATE` with a record: cached, and each draw waiting on it runs (`0x58aaf1`).
fn on_npc_greeting(In(ev): In<SessionEvent>, world: &mut World) {
    if let SessionEvent::NpcGreeting { text_id, blocks } = ev {
        super::text_answered(world, text_id, Some(blocks));
    }
}

/// `SMSG_NPC_TEXT_UPDATE`'s miss: each draw waiting on it closes the gossip (`0x58ab07`).
fn on_npc_text_miss(In(ev): In<SessionEvent>, world: &mut World) {
    if let SessionEvent::NpcTextMiss { text_id } = ev {
        super::text_answered(world, text_id, None);
    }
}

/// `SMSG_GOSSIP_COMPLETE` runs the zero gossip set (`0x58a852`): a held quest giver keeps the interact target and its own dialog.
fn on_gossip_complete(In(ev): In<SessionEvent>, world: &mut World) {
    if let SessionEvent::GossipComplete = ev {
        debug!("net: gossip complete — closing the menu");
        super::set_npc(world, 0);
    }
}

/// The NPC is dropped with the connection; the globals and the records stay.
fn on_session_end(In(_): In<SessionEvent>, mut gossip: ResMut<GossipState>) {
    gossip.npc = None;
    gossip.menu_id = 0;
}
