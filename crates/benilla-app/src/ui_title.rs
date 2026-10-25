//! The title feed: `CharTitles.dbc` and the player's title fields into the VM, the known-titles
//! watch's `KNOWN_TITLES_UPDATE`, `SetCurrentTitle`'s `CMSG_SET_TITLE` and `SMSG_TITLE_EARNED`'s
//! chat line. 12340 registers one watch per known-titles u64 (`0x778`, `0x780`, `0x788`), each
//! firing the event with `"player"` (`OnUpdatePlayerScriptEvent` `0x6cddd0`).

use bevy::prelude::*;

use benilla_protocol::field::FIELD_PLAYER_KNOWN_TITLES;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{PlayerTitles, ScriptValue, TitleRow, UiScript};

use crate::names::NameCache;
use crate::net::{
    ClientCommand, FieldChanged, NetCommands, NetHandlerApp, ObjectStore, SelfPlayer,
};
use crate::ui_chat::ChatLog;
use crate::ui_quest::rewards::QuestRewardTablesRes;
use crate::ui_script::{UiInput, VmMemo};
use crate::ui_unit::UnitFeed;

pub(crate) struct UiTitlePlugin;

impl Plugin for UiTitlePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                feed_titles.in_set(UnitFeed),
                drain_title_sets.after(UiInput),
                feed_overhead_rows,
            ),
        )
        .net_handler(SessionEventKind::TitleEarned, on_title_earned);
    }
}

/// `CharTitles.dbc`'s rows in file order, as the VM reads them.
fn title_rows(tables: &QuestRewardTablesRes) -> Vec<TitleRow> {
    tables
        .0
        .char_titles()
        .iter()
        .map(|r| TitleRow {
            mask_id: r.mask_id,
            name: r.name.clone(),
            name1: r.name1.clone(),
        })
        .collect()
}

/// The player's title fields; `PLAYER_BYTES_3` byte 0 is the gender `GetTitleName` reads.
fn player_titles(store: &ObjectStore) -> PlayerTitles {
    PlayerTitles {
        chosen: store.0.player_chosen_title(),
        known: store.0.player_known_titles(),
        female: store.0.player_gender_byte() == Some(1),
    }
}

/// The overhead name's copy of the rows, for the world-text pass that runs without the VM.
fn feed_overhead_rows(
    tables: Option<Res<QuestRewardTablesRes>>,
    strings: Option<ResMut<crate::nameplates::PvpNameStrings>>,
) {
    let (Some(tables), Some(mut strings)) = (tables, strings) else {
        return;
    };
    if tables.is_changed() {
        strings.set_rows(title_rows(&tables));
    }
}

/// The u64 watches a known-titles edge at `index` belongs to, 0..3; `None` off the field.
fn known_titles_watch(index: u16) -> Option<u16> {
    index
        .checked_sub(FIELD_PLAYER_KNOWN_TITLES)
        .filter(|&i| i < 6)
        .map(|i| i / 2)
}

fn feed_titles(
    script: Option<NonSendMut<UiScript>>,
    tables: Option<Res<QuestRewardTablesRes>>,
    self_q: Query<(Entity, &ObjectStore), With<SelfPlayer>>,
    changed_self: Query<(), (With<SelfPlayer>, Changed<ObjectStore>)>,
    mut edges: MessageReader<FieldChanged>,
    mut memory: Local<VmMemo<()>>,
) {
    let Some(mut script) = script else {
        edges.clear();
        return;
    };
    let (_, vm_reset) = memory.get_reset(&script);
    if let Some(tables) = tables {
        if vm_reset || tables.is_changed() {
            script.set_title_rows(title_rows(&tables));
        }
    }
    let me = self_q.iter().next();
    if vm_reset || !changed_self.is_empty() {
        script.set_player_titles(me.map(|(_, store)| player_titles(store)));
    }
    let Some((me, _)) = me else {
        edges.clear();
        return;
    };
    let mut fired = [false; 3];
    for edge in edges.read().filter(|e| e.entity == me) {
        if let Some(watch) = known_titles_watch(edge.index) {
            if !std::mem::replace(&mut fired[usize::from(watch)], true) {
                script.fire_event(
                    "KNOWN_TITLES_UPDATE",
                    vec![ScriptValue::Str("player".into())],
                );
            }
        }
    }
}

/// `SetCurrentTitle`'s sends; the server answers with `PLAYER_CHOSEN_TITLE`.
fn drain_title_sets(script: Option<NonSendMut<UiScript>>, commands: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for mask_id in script.take_title_sets() {
        debug!("ui_title: set title {mask_id}");
        let _ = commands.0.send(ClientCommand::SetTitle { mask_id });
    }
}

/// `SMSG_TITLE_EARNED` (`CGChat::OnTitleEarned` `0x50c520`): `NEW_TITLE_EARNED` or
/// `OLD_TITLE_LOST` around the title filled with the player's name, as a system line. Nothing
/// for a mask no row has or with no player. The earn's spell effect (kind 9) is not built.
fn on_title_earned(
    In(ev): In<SessionEvent>,
    script: Option<NonSend<UiScript>>,
    tables: Option<Res<QuestRewardTablesRes>>,
    names: Res<NameCache>,
    self_q: Query<(&crate::net::Guid, &ObjectStore), With<SelfPlayer>>,
    mut chat: ResMut<ChatLog>,
) {
    let SessionEvent::TitleEarned { mask_id, earned } = ev else {
        return;
    };
    let (Some(script), Some(tables), Some((guid, store))) = (script, tables, self_q.iter().next())
    else {
        return;
    };
    let rows = title_rows(&tables);
    let name = names.peek(guid.0).unwrap_or_default();
    let female = store.0.player_gender_byte() == Some(1);
    let Some(title) =
        benilla_ui::script::chosen_title_name(&rows, mask_id as i32, female, 0, name, |_| None)
    else {
        return;
    };
    let key = if earned {
        "NEW_TITLE_EARNED"
    } else {
        "OLD_TITLE_LOST"
    };
    let template = crate::ui_chat::combat::global_string(&script, key).unwrap_or_default();
    crate::ui_chat::away::push_system(&mut chat, template.replacen("%s", &title, 1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_known_titles_u64_is_one_watch() {
        assert_eq!(known_titles_watch(FIELD_PLAYER_KNOWN_TITLES - 1), None);
        assert_eq!(known_titles_watch(FIELD_PLAYER_KNOWN_TITLES), Some(0));
        assert_eq!(known_titles_watch(FIELD_PLAYER_KNOWN_TITLES + 1), Some(0));
        assert_eq!(known_titles_watch(FIELD_PLAYER_KNOWN_TITLES + 5), Some(2));
        assert_eq!(known_titles_watch(FIELD_PLAYER_KNOWN_TITLES + 6), None);
    }
}
