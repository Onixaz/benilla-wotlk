//! The guild event log: the rows `MSG_GUILD_EVENT_LOG_QUERY` last answered (`0x5ca6a0`), their
//! names resolved through the name cache and pushed to the VM. `GUILD_EVENT_LOG_UPDATE` fires once
//! every name is cached: at once when all were, else from the last name's callback (`0x5ca460`).

use benilla_protocol::messages::GuildEventLogEntry;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{GuildEventView, UiScript};
use bevy::prelude::*;

use crate::names::NameCache;
use crate::net::{NetCommands, NetHandlerApp};

/// The last answer's rows, each with the Unix second it happened: arrival less its age.
#[derive(Resource, Default)]
pub(crate) struct GuildEventLog {
    rows: Vec<(GuildEventLogEntry, u32)>,
    /// The rows changed since the last push.
    dirty: bool,
    /// A row's name was not cached at the last push.
    unnamed: bool,
    /// `GUILD_EVENT_LOG_UPDATE` waits for the names.
    owed: bool,
}

impl GuildEventLog {
    fn apply(&mut self, rows: Vec<GuildEventLogEntry>, now: u32) {
        self.rows = rows
            .into_iter()
            .map(|e| (e, now.wrapping_sub(e.age)))
            .collect();
        self.dirty = true;
        self.owed = true;
    }

    /// The rows as the VM reads them, and whether every name was cached; a miss asks once.
    fn views(&self, names: &NameCache, commands: &NetCommands) -> (Vec<GuildEventView>, bool) {
        let mut named = true;
        let mut name = |guid: u64| {
            if guid == 0 {
                return None;
            }
            let n = names.resolve(guid, commands).map(str::to_string);
            named &= n.is_some();
            n
        };
        let views = self
            .rows
            .iter()
            .map(|(e, written_at)| GuildEventView {
                kind: e.kind,
                player1: name(e.player1),
                player2: name(e.player2),
                rank: e.rank,
                written_at: *written_at,
            })
            .collect();
        (views, named)
    }
}

pub(super) fn register(app: &mut App) {
    app.init_resource::<GuildEventLog>()
        .net_handler(SessionEventKind::GuildEventLog, on_log)
        .net_handler(SessionEventKind::Disconnected, on_session_end);
}

fn on_log(In(ev): In<SessionEvent>, mut log: ResMut<GuildEventLog>) {
    if let SessionEvent::GuildEventLog(rows) = ev {
        debug!("guild: event log, {} row(s)", rows.len());
        log.apply(rows, unix_now());
    }
}

fn on_session_end(In(_): In<SessionEvent>, mut log: ResMut<GuildEventLog>) {
    *log = GuildEventLog::default();
}

/// Push the log on a change, again as missing names land, and fire `GUILD_EVENT_LOG_UPDATE` once
/// the last name is in.
pub(super) fn feed_guild_event_log(
    script: Option<NonSendMut<UiScript>>,
    mut log: ResMut<GuildEventLog>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut seeded: Local<crate::ui_script::VmMemo<bool>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let seeded = seeded.get(&script);
    if *seeded && !log.dirty && !(log.unnamed && names.is_changed()) {
        return;
    }
    *seeded = true;
    let (views, named) = log.views(&names, &commands);
    script.set_guild_event_log(views);
    log.dirty = false;
    log.unnamed = !named;
    if named && std::mem::take(&mut log.owed) {
        script.fire_event("GUILD_EVENT_LOG_UPDATE", Vec::new());
    }
}

fn unix_now() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: u8, player1: u64, player2: u64, age: u32) -> GuildEventLogEntry {
        GuildEventLogEntry {
            kind,
            player1,
            player2,
            rank: 0,
            age,
        }
    }

    #[test]
    fn a_row_happened_its_age_before_arrival_and_the_answer_owes_the_event() {
        let mut log = GuildEventLog::default();
        log.apply(vec![row(1, 1, 2, 60), row(2, 2, 0, 3600)], 10_000);
        assert_eq!(
            log.rows.iter().map(|(_, at)| *at).collect::<Vec<_>>(),
            [9_940, 6_400]
        );
        assert!(log.dirty && log.owed);
    }

    #[test]
    fn the_event_waits_for_every_name() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut names = NameCache::default();
        let mut log = GuildEventLog::default();
        log.apply(vec![row(1, 1, 2, 0), row(2, 2, 0, 0)], 0);
        let (views, named) = log.views(&names, &commands);
        assert!(!named);
        assert_eq!(views[0].player1, None);
        names.insert_player(1, "Alice".into(), None);
        names.insert_player(2, "Bob".into(), None);
        let (views, named) = log.views(&names, &commands);
        assert!(named);
        assert_eq!(views[0].player1.as_deref(), Some("Alice"));
        assert_eq!(views[0].player2.as_deref(), Some("Bob"));
        // A join carries no second guid, which waits on nothing.
        assert_eq!(views[1].player2, None);
    }
}
