//! The transfer packets' client side: `SMSG_TRANSFER_PENDING`'s control clear, the refusal line of
//! `SMSG_TRANSFER_ABORTED` (12340 `0x403910`, a `TRANSFER_ABORT_*` GlobalString for the reason,
//! filled with the map's name and shown as `CHAT_MSG_SYSTEM`), and the steps of a dynamic-difficulty
//! change (`SMSG_CHANGEPLAYER_DIFFICULTY_RESULT`, `0x52c460`).

use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_formats::MapDifficulty;
use benilla_protocol::messages::DifficultyChangeResult;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::UiScript;

use crate::net::NetHandlerApp;
use crate::ui_action::{FillArg, UiError, UiErrorKeys};
use crate::ui_chat::{ChatEvent, ChatEventKind, ChatLog};

/// The refusals waiting for the feed: `(map, reason, arg)`.
#[derive(Resource, Default)]
struct TransferAborts(Vec<(u32, u8, Option<u8>)>);

/// What a difficulty change owes the interface, in wire order.
#[derive(Debug, PartialEq, Eq)]
enum DifficultyNotice {
    /// Result 8's `MapDifficulty.dbc` message, a system line (`0x52c641`).
    Line(String),
    /// Result 9: `PLAYER_DIFFICULTY_CHANGED`, then the success line for the stored difficulty.
    Changed(u32),
}

#[derive(Resource, Default)]
struct DifficultyNotices(Vec<DifficultyNotice>);

/// `MapDifficulty.dbc`, for the difficulty refusal's own message; empty without client data.
#[derive(Resource, Default)]
pub(crate) struct MapDifficulties(pub(crate) Vec<MapDifficulty>);

pub(crate) struct UiTransferPlugin;

impl Plugin for UiTransferPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TransferAborts>()
            .init_resource::<DifficultyNotices>()
            .init_resource::<MapDifficulties>()
            .net_handler(SessionEventKind::TransferPending, on_transfer_pending)
            .net_handler(SessionEventKind::TransferAborted, on_transfer_aborted)
            .net_handler(
                SessionEventKind::DifficultyChangeResult,
                on_difficulty_change,
            )
            .add_systems(Startup, load_map_difficulties.after(AssetSet::Open))
            .add_systems(
                Update,
                // A refusal before the interface is up waits for it, as the chat line needs it.
                (feed_transfer_aborts, feed_difficulty_notices)
                    .in_set(crate::ui_script::UiFeed)
                    .run_if(crate::ui_script::ingame_ui_up),
            );
    }
}

fn load_map_difficulties(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_map_difficulties(&mut chain)
    };
    match loaded {
        Ok(rows) => commands.insert_resource(MapDifficulties(rows)),
        Err(e) => {
            warn!("ui_transfer: MapDifficulty.dbc failed — difficulty refusals fall back: {e:#}")
        }
    }
}

/// `SMSG_TRANSFER_PENDING` clears the client controls first (`0x4014ce` → `0x5194c0`): the cursor
/// (`ClearCursor(1, 1)`, `0x5194c4`) and our casts (`0x5194cc`). The screen is
/// [`crate::loading_screen`]'s, off the same latch.
fn on_transfer_pending(
    In(ev): In<SessionEvent>,
    script: Option<NonSendMut<UiScript>>,
    mut cancel: crate::spell::SelfCancel,
) {
    if !matches!(ev, SessionEvent::TransferPending { .. }) {
        return;
    }
    if let Some(mut script) = script {
        script.clear_cursor_payload();
    }
    cancel.cancel_player_spells();
}

/// `SMSG_CHANGEPLAYER_DIFFICULTY_RESULT` by its jump table (`0x52c484`). Not kept: result 6's next
/// allowed change time (`[0xbd1984]`), which only `CanChangePlayerDifficulty` reads, unbuilt.
#[allow(clippy::too_many_arguments)]
fn on_difficulty_change(
    In(ev): In<SessionEvent>,
    mut group: ResMut<crate::ui_party::GroupState>,
    mut lfg: ResMut<crate::ui_lfg::LfgFeed>,
    mut errors: ResMut<UiErrorKeys>,
    mut notices: ResMut<DifficultyNotices>,
    mut screen: ResMut<crate::loading_screen::LoadingScreen>,
    map: Option<Res<benilla_world::world_map::CurrentMap>>,
    difficulties: Res<MapDifficulties>,
    time: Res<Time<Real>>,
) {
    let SessionEvent::DifficultyChangeResult(result) = ev else {
        return;
    };
    debug!("ui_transfer: difficulty change result {result:?}");
    match result {
        DifficultyChangeResult::Stored(dynamic) => {
            group.difficulties.dynamic = dynamic;
            lfg.set_dynamic_difficulty(u32::from(dynamic));
        }
        DifficultyChangeResult::Started(_) => {
            screen.raise_for_difficulty_change(map.map(|m| m.0), time.elapsed_secs());
        }
        DifficultyChangeResult::Done => {
            screen.release_difficulty_change();
            notices.0.push(DifficultyNotice::Changed(u32::from(
                group.difficulties.dynamic,
            )));
        }
        DifficultyChangeResult::MapMessage(id) => {
            let message = difficulties
                .0
                .iter()
                .find(|row| row.id as u32 == id)
                .map(|row| row.message.clone())
                .filter(|m| !m.is_empty());
            if let Some(message) = message {
                notices.0.push(DifficultyNotice::Line(message));
            }
        }
        other => errors.0.extend(difficulty_refusal(other)),
    }
}

/// The refusal arms' error-display messages (`0x52c4e9`-`0x52c5e9`).
fn difficulty_refusal(result: DifficultyChangeResult) -> Option<UiError> {
    let key = match result {
        DifficultyChangeResult::Cooldown(seconds) => {
            return Some(UiError::args(
                "ERR_DIFFICULTY_CHANGE_COOLDOWN_S",
                vec![FillArg::Duration(u64::from(seconds) * 1000)],
            ));
        }
        DifficultyChangeResult::Refused(2) => "ERR_DIFFICULTY_CHANGE_WORLDSTATE",
        DifficultyChangeResult::Refused(3) => "ERR_DIFFICULTY_CHANGE_ENCOUNTER",
        DifficultyChangeResult::Refused(4) => "ERR_DIFFICULTY_CHANGE_COMBAT",
        DifficultyChangeResult::Refused(5) => "ERR_DIFFICULTY_CHANGE_PLAYER_BUSY",
        DifficultyChangeResult::Refused(7) => "ERR_DIFFICULTY_CHANGE_ALREADY_STARTED",
        _ => return None,
    };
    Some(UiError::key(key))
}

/// Result 9 fires `PLAYER_DIFFICULTY_CHANGED` (`0x52c5a5`), then `ERR_PLAYER_DIFFICULTY_CHANGED_S`
/// with `PLAYER_DIFFICULTYn`, one past the stored difficulty (`0x52c5d7`); result 8's line is
/// system chat.
fn feed_difficulty_notices(
    script: Option<NonSendMut<UiScript>>,
    mut notices: ResMut<DifficultyNotices>,
    mut errors: ResMut<UiErrorKeys>,
    mut chat: ResMut<ChatLog>,
) {
    let Some(mut script) = script else { return };
    for notice in std::mem::take(&mut notices.0) {
        match notice {
            DifficultyNotice::Line(text) => {
                chat.push_event(ChatEvent::text_only(ChatEventKind::System, text));
            }
            DifficultyNotice::Changed(dynamic) => {
                script.fire_event("PLAYER_DIFFICULTY_CHANGED", Vec::new());
                errors.0.push(UiError::args(
                    "ERR_PLAYER_DIFFICULTY_CHANGED_S",
                    vec![FillArg::Global(format!("PLAYER_DIFFICULTY{}", dynamic + 1))],
                ));
            }
        }
    }
}

fn on_transfer_aborted(In(ev): In<SessionEvent>, mut aborts: ResMut<TransferAborts>) {
    if let SessionEvent::TransferAborted { map, reason, arg } = ev {
        aborts.0.push((map, reason, arg));
    }
}

/// What a reason shows: a GlobalString token, or `MapDifficulty.dbc`'s own message.
#[derive(Debug, PartialEq, Eq)]
enum AbortText {
    Token(String),
    Literal(String),
}

/// The reason's text by the handler's jump table (`0x403974`, table `0x403b2c`); 0, 5 and past 16
/// show nothing. A difficulty refusal takes the `(map, difficulty)` row's message, else
/// `TRANSFER_ABORT_DIFFICULTY<difficulty + 1>` (`0x403a2d`-`0x403a65`).
fn abort_text(
    reason: u8,
    arg: Option<u8>,
    difficulty_message: impl FnOnce(u8) -> Option<String>,
) -> Option<AbortText> {
    let token = |t: &str| Some(AbortText::Token(t.to_string()));
    let arg = arg.unwrap_or(0);
    match reason {
        1 => token("TRANSFER_ABORT_ERROR"),
        2 => token("TRANSFER_ABORT_MAX_PLAYERS"),
        3 | 12..=14 => token("TRANSFER_ABORT_NOT_FOUND"),
        4 => token("TRANSFER_ABORT_TOO_MANY_INSTANCES"),
        6 => token("TRANSFER_ABORT_ZONE_IN_COMBAT"),
        7 => token(&format!("TRANSFER_ABORT_INSUF_EXPAN_LVL{arg}")),
        8 => match difficulty_message(arg).filter(|m| !m.is_empty()) {
            Some(message) => Some(AbortText::Literal(message)),
            None => token(&format!("TRANSFER_ABORT_DIFFICULTY{}", u32::from(arg) + 1)),
        },
        9 => token(&format!("TRANSFER_ABORT_UNIQUE_MESSAGE{arg}")),
        10 => token("TRANSFER_ABORT_TOO_MANY_REALM_INSTANCES"),
        11 => token("TRANSFER_ABORT_NEED_GROUP"),
        15 => token("TRANSFER_ABORT_REALM_ONLY"),
        16 => token("TRANSFER_ABORT_MAP_NOT_ALLOWED"),
        _ => None,
    }
}

/// Each queued refusal as one system line, the text's `%s` the map's `Map.dbc` name
/// (`[map+0x14]`, `0x403acc`). A map with no row, or text that resolves empty, shows nothing
/// (`0x40395e`, `0x403ac5`-`0x403aca`).
fn feed_transfer_aborts(
    script: Option<NonSend<UiScript>>,
    mut aborts: ResMut<TransferAborts>,
    mut chat: ResMut<ChatLog>,
    maps: Option<Res<benilla_assets::MapCatalogRes>>,
    difficulties: Res<MapDifficulties>,
) {
    let Some(script) = script else { return };
    if aborts.0.is_empty() {
        return;
    }
    let pending = std::mem::take(&mut aborts.0);
    for (map, reason, arg) in pending {
        let Some(name) = maps.as_ref().and_then(|c| c.0.name(map)) else {
            debug!("ui_transfer: map {map} has no Map.dbc row — no line");
            continue;
        };
        let message = |difficulty: u8| {
            benilla_formats::find_map_difficulty(&difficulties.0, map as i32, i32::from(difficulty))
                .map(|row| row.message.clone())
        };
        let template = match abort_text(reason, arg, message) {
            Some(AbortText::Token(key)) => script.lua().globals().get::<String>(key).ok(),
            Some(AbortText::Literal(text)) => Some(text),
            None => None,
        };
        let Some(template) = template.filter(|t| !t.is_empty()) else {
            debug!("ui_transfer: reason {reason} on map {map} resolves to nothing — no line");
            continue;
        };
        let text = benilla_ui::strings::fill(&template, &[benilla_ui::strings::Arg::S(name)]);
        chat.push_event(ChatEvent::text_only(ChatEventKind::System, text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(t: &str) -> Option<AbortText> {
        Some(AbortText::Token(t.into()))
    }

    /// The jump table's arms (`0x403b2c`): the shared not-found text, the numbered families, and
    /// silence for 0, 5 and past 16.
    #[test]
    fn each_reason_names_its_global_string() {
        let none = |_| None;
        assert_eq!(
            abort_text(2, None, none),
            token("TRANSFER_ABORT_MAX_PLAYERS")
        );
        for reason in [3, 12, 13, 14] {
            assert_eq!(
                abort_text(reason, None, none),
                token("TRANSFER_ABORT_NOT_FOUND")
            );
        }
        assert_eq!(
            abort_text(7, Some(2), none),
            token("TRANSFER_ABORT_INSUF_EXPAN_LVL2")
        );
        assert_eq!(
            abort_text(9, Some(1), none),
            token("TRANSFER_ABORT_UNIQUE_MESSAGE1")
        );
        assert_eq!(
            abort_text(16, None, none),
            token("TRANSFER_ABORT_MAP_NOT_ALLOWED")
        );
        for reason in [0, 5, 17, 255] {
            assert_eq!(abort_text(reason, None, none), None, "{reason}");
        }
    }

    /// The refusal arms name their texts; the cooldown fills a duration, the steps none.
    #[test]
    fn each_difficulty_refusal_names_its_message() {
        use DifficultyChangeResult as R;
        let key = |r| difficulty_refusal(r).map(|e| e.key);
        assert_eq!(key(R::Refused(2)), Some("ERR_DIFFICULTY_CHANGE_WORLDSTATE"));
        assert_eq!(key(R::Refused(3)), Some("ERR_DIFFICULTY_CHANGE_ENCOUNTER"));
        assert_eq!(key(R::Refused(4)), Some("ERR_DIFFICULTY_CHANGE_COMBAT"));
        assert_eq!(
            key(R::Refused(5)),
            Some("ERR_DIFFICULTY_CHANGE_PLAYER_BUSY")
        );
        assert_eq!(
            key(R::Refused(7)),
            Some("ERR_DIFFICULTY_CHANGE_ALREADY_STARTED")
        );
        assert_eq!(
            difficulty_refusal(R::Cooldown(90)),
            Some(UiError::args(
                "ERR_DIFFICULTY_CHANGE_COOLDOWN_S",
                vec![FillArg::Duration(90_000)]
            ))
        );
        for step in [
            R::Stored(1),
            R::Started(5),
            R::Done,
            R::MapMessage(3),
            R::Other(10),
        ] {
            assert_eq!(key(step), None, "{step:?}");
        }
    }

    /// A difficulty refusal shows the row's message when it has one, else the numbered text one
    /// past the 0-based difficulty.
    #[test]
    fn a_difficulty_refusal_prefers_the_rows_message() {
        assert_eq!(
            abort_text(8, Some(1), |d| (d == 1).then(|| "row text".to_string())),
            Some(AbortText::Literal("row text".into()))
        );
        assert_eq!(
            abort_text(8, Some(1), |_| Some(String::new())),
            token("TRANSFER_ABORT_DIFFICULTY2")
        );
        assert_eq!(
            abort_text(8, Some(0), |_| None),
            token("TRANSFER_ABORT_DIFFICULTY1")
        );
    }
}
