//! The titles: `CharTitles.dbc` and the active player's title fields, read by the title verbs at
//! the call through the live view; the known-titles watch's `KNOWN_TITLES_UPDATE`, which 12340
//! registers once per known-titles u64 (`0x778`, `0x780`, `0x788`), each firing the event with
//! `"player"` (`0x6cddd0`); and `SMSG_TITLE_EARNED`'s chat line.

use bevy::prelude::*;

use benilla_formats::QuestRewardTables;
use benilla_protocol::field::FIELD_PLAYER_KNOWN_TITLES;
use benilla_protocol::{ObjectFields, SessionEvent, SessionEventKind};
use benilla_ui::script::{PlayerTitles, TitleRow};

use crate::names::NameCache;
use crate::net::{NetHandlerApp, ObjectStore, SelfPlayer};
use crate::ui_quest::rewards::QuestRewardTablesRes;

pub(crate) struct UiTitlePlugin;

impl Plugin for UiTitlePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, feed_overhead_rows)
            .net_handler(SessionEventKind::TitleEarned, on_title_earned);
    }
}

/// `CharTitles.dbc`'s rows in file order, as the overhead name reads them.
fn title_rows(tables: &QuestRewardTables) -> Vec<TitleRow> {
    tables.char_titles().iter().map(row_of).collect()
}

fn row_of(r: &benilla_formats::CharTitle) -> TitleRow {
    TitleRow {
        mask_id: r.mask_id,
        name: r.name.clone(),
        name1: r.name1.clone(),
    }
}

/// The row whose mask id is `mask`, the first in file order (`0x719900`).
pub(crate) fn title_row(tables: &QuestRewardTables, mask: i64) -> Option<TitleRow> {
    tables
        .char_titles()
        .iter()
        .find(|r| i64::from(r.mask_id) == mask)
        .map(row_of)
}

/// The player's title fields; `PLAYER_BYTES_3` byte 0 is the gender `GetTitleName` reads (`0x51b546`).
pub(crate) fn player_titles(fields: &ObjectFields) -> PlayerTitles {
    PlayerTitles {
        chosen: fields.player_chosen_title(),
        known: fields.player_known_titles(),
        female: fields.player_gender_byte() == Some(1),
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
        strings.set_rows(title_rows(&tables.0));
    }
}

/// The known-titles watch an edge at `index` belongs to, 0..3; `None` off the field.
pub(crate) fn known_titles_watch(index: u16) -> Option<u16> {
    index
        .checked_sub(FIELD_PLAYER_KNOWN_TITLES)
        .filter(|&i| i < 6)
        .map(|i| i / 2)
}

/// `SMSG_TITLE_EARNED` (`0x50c520`): `NEW_TITLE_EARNED` or `OLD_TITLE_LOST` around the title filled with the player's name, added to the chat log in the handler (`0x50c70c` -> `0x509dd0`). Nothing for a mask no row has or with no player. The earn effect plays in `creature_anim`.
fn on_title_earned(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::TitleEarned { mask_id, earned } = ev else {
        return;
    };
    let Ok(Some(title)) = world.run_system_cached_with(earned_title, mask_id) else {
        return;
    };
    let key = if earned {
        "NEW_TITLE_EARNED"
    } else {
        "OLD_TITLE_LOST"
    };
    let Some(template) = crate::ui_script::live::with_live_world(world, |s| {
        crate::ui_chat::combat::global_string(s, key).unwrap_or_default()
    }) else {
        return;
    };
    crate::ui_chat::add_chat_line_now(world, 0, template.replacen("%s", &title, 1));
}

/// The title `mask_id` names for our player, filled with our name (`0x50c567`, `0x72a000`).
fn earned_title(
    In(mask_id): In<u32>,
    tables: Option<Res<QuestRewardTablesRes>>,
    names: Res<NameCache>,
    self_q: Query<(&crate::net::Guid, &ObjectStore), With<SelfPlayer>>,
) -> Option<String> {
    let tables = tables?;
    let (guid, store) = self_q.iter().next()?;
    let name = names.peek(guid.0).unwrap_or_default();
    let female = store.0.player_gender_byte() == Some(1);
    benilla_ui::script::chosen_title_name(
        |mask| title_row(&tables.0, mask),
        mask_id as i32,
        female,
        0,
        name,
        |_| None,
    )
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
