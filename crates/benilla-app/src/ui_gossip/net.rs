//! The gossip window's packet handlers: they fill [`GossipState`] and never touch the VM.

use benilla_protocol::messages::{GossipMenu, NpcTextBlock};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::GossipState;
use crate::net::{ClientCommand, GuidIndex, NetCommands, NetHandlerApp, ObjectStore};
use crate::ui_quest::QuestGiver;

/// Register the gossip handlers and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::GossipMenu, on_gossip_menu)
        .net_handler(K::NpcGreeting, on_npc_greeting)
        .net_handler(K::NpcTextMiss, on_npc_text_miss)
        .net_handler(K::GossipComplete, on_gossip_complete)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_gossip_menu(
    In(ev): In<SessionEvent>,
    mut gossip: ResMut<GossipState>,
    commands: Res<NetCommands>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
) {
    if let SessionEvent::GossipMenu(menu) = ev {
        gossip_menu(menu, &mut gossip, &commands, &index, &stores);
    }
}

fn on_npc_greeting(
    In(ev): In<SessionEvent>,
    mut gossip: ResMut<GossipState>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
) {
    if let SessionEvent::NpcGreeting { text_id, blocks } = ev {
        npc_greeting(text_id, blocks, &mut gossip, &index, &stores);
    }
}

fn on_npc_text_miss(In(ev): In<SessionEvent>, mut gossip: ResMut<GossipState>) {
    if let SessionEvent::NpcTextMiss { text_id } = ev {
        gossip.text_missing(text_id);
    }
}

fn on_gossip_complete(In(ev): In<SessionEvent>, mut gossip: ResMut<GossipState>) {
    if let SessionEvent::GossipComplete = ev {
        gossip_complete(&mut gossip);
    }
}

/// The menu closes with the connection.
fn on_session_end(In(_): In<SessionEvent>, mut gossip: ResMut<GossipState>) {
    gossip.clear_session();
}

/// A streamed unit's gender (`UNIT_FIELD_BYTES_0` byte 2), which picks the greeting's column: the
/// reference tests `== 1` for female (`0x58ace0`), so genderless 2 reads male. `0` when the guid
/// is not a streamed unit; the reference's non-unit arm takes the female column only when block
/// 0 has no male text.
fn npc_gender(guid: u64, index: &GuidIndex, stores: &Query<&ObjectStore>) -> u8 {
    index
        .0
        .get(&guid)
        .and_then(|&e| stores.get(e).ok())
        .and_then(|s| s.0.unit_gender())
        .unwrap_or(0)
}

/// `SMSG_GOSSIP_MESSAGE`: latch the menu, and on a first visit to its text send the query. The
/// greeting is drawn at menu open, as the reference's `0x58ac30` draws it.
fn gossip_menu(
    menu: GossipMenu,
    gossip: &mut GossipState,
    net_commands: &NetCommands,
    index: &GuidIndex,
    stores: &Query<&ObjectStore>,
) {
    let (npc, text_id) = (menu.npc, menu.text_id);
    let npc_gender = npc_gender(npc, index, stores);
    debug!(
        "net: gossip menu {} on {npc:#x} — {} options, {} quests",
        menu.menu_id,
        menu.options.len(),
        menu.quests.len()
    );
    if gossip.open_menu(menu, npc_gender) {
        let _ = net_commands
            .0
            .send(ClientCommand::NpcTextQuery { text_id, guid: npc });
    }
}

/// `SMSG_NPC_TEXT_UPDATE`: cache the record and open the menu waiting on it, whose NPC's gender
/// picks the column.
fn npc_greeting(
    text_id: u32,
    blocks: Vec<NpcTextBlock>,
    gossip: &mut GossipState,
    index: &GuidIndex,
    stores: &Query<&ObjectStore>,
) {
    let npc_gender = gossip.npc.map_or(0, |npc| npc_gender(npc, index, stores));
    gossip.text_arrived(text_id, blocks, npc_gender);
}

/// `SMSG_GOSSIP_COMPLETE` closes the menu alone (`0x58a852`): the zero-guid gossip setter leaves an
/// open quest-giver dialog standing (`0x58a570`).
pub(crate) fn gossip_complete(gossip: &mut GossipState) {
    debug!("net: gossip complete — closing the menu");
    gossip.clear();
}

/// The menu and the quest-giver dialog both close.
pub(crate) fn end_interaction(gossip: &mut GossipState, quest: &mut QuestGiver) {
    gossip.clear();
    quest.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gossip_complete_leaves_the_quest_dialog_open() {
        let mut gossip = GossipState {
            npc: Some(0x42),
            ..Default::default()
        };
        let mut quest = QuestGiver {
            npc: Some(0x42),
            ..Default::default()
        };
        gossip_complete(&mut gossip);
        assert_eq!(gossip.npc, None);
        assert_eq!(quest.npc, Some(0x42), "the quest-giver dialog stays");

        gossip.npc = Some(0x42);
        end_interaction(&mut gossip, &mut quest);
        assert_eq!((gossip.npc, quest.npc), (None, None));
    }
}
