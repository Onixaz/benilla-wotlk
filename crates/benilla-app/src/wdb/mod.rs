//! The query caches on disk: 12340's nine persisted `DBCache`s as `Cache\WDB\<locale>\*.wdb`
//! ([`crate::local_state::wdb_dir`]), in the reference's own bytes ([`codec`]).
//!
//! As the reference: every file loads once at start (`LoadDBCaches` `0x635060`) and is rewritten
//! whole once at exit (`DBCache_Destroy` `0x635540`); each answer the server sends lands in its
//! file's records; a server cache version other than the one a file carries drops its records
//! (`SMSG_CLIENTCACHE_VERSION`, `0x66f940`). No realm is checked: one set serves the install.
//!
//! A loaded record reaches the game as the answer it was: its response is rebuilt and run through
//! the same handler table as the wire's, so nothing asks again for what the file already holds.
//! The item names and the Warden modules, which benilla never asks for, ride along untouched.

pub(crate) mod codec;

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use crate::net::NetHandlerApp;
use codec::{Kind, Refused, WdbFile, KINDS};

pub(crate) struct WdbPlugin;

impl Plugin for WdbPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(load())
            .net_handler(SessionEventKind::QueryAnswer, on_query_answer)
            .net_handler(SessionEventKind::ClientCacheVersion, on_cache_version)
            // Before the first frame: the files answer before the wire does.
            .add_systems(PostStartup, replay);
        crate::shutdown::on_app_exit(app, save.into_configs());
    }
}

/// A cache owner holding answers the reference persists, which a moved server cache version takes.
pub(crate) trait Persisted: Resource {
    fn drop_persisted(&mut self);
}

/// The nine files' records, in [`KINDS`] order, and where they live.
#[derive(Resource, Default)]
pub(crate) struct WdbCaches {
    /// The mounted locale, which names the folder and fills each header; `None` with no install.
    locale: Option<String>,
    files: [WdbFile; KINDS.len()],
}

impl WdbCaches {
    fn file_mut(&mut self, kind: &Kind) -> &mut WdbFile {
        let i = KINDS.iter().position(|k| k == kind).unwrap_or(0);
        &mut self.files[i]
    }
}

/// Startup: every file, each refused one loading nothing (`0x67bb2c`, `0x67bd1b`).
fn load() -> WdbCaches {
    let locale = benilla_formats::wow_data().and_then(|d| benilla_formats::data_locale(&d).ok());
    let mut caches = WdbCaches {
        locale,
        ..WdbCaches::default()
    };
    let Some(locale) = caches.locale.clone() else {
        return caches;
    };
    let Some(dir) = crate::local_state::wdb_dir(&locale) else {
        return caches;
    };
    let code = codec::locale_code(&locale);
    for (kind, file) in KINDS.iter().zip(caches.files.iter_mut()) {
        let path = dir.join(kind.file);
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        match WdbFile::parse(kind, code, &bytes) {
            Ok(loaded) => *file = loaded,
            Err(Refused::Header) => info!(
                "wdb: {} is another build's or locale's; it is rewritten at exit",
                path.display()
            ),
            Err(Refused::Corrupt) => {
                warn!(
                    "wdb: {} is cut short; nothing loaded from it",
                    path.display()
                )
            }
        }
    }
    let total: usize = caches.files.iter().map(WdbFile::len).sum();
    info!("wdb: {total} cached answers from {}", dir.display());
    caches
}

/// The loaded records, as the responses they answered, through the wire's handlers.
fn replay(world: &mut World) {
    let events: Vec<SessionEvent> = {
        let caches = world.resource::<WdbCaches>();
        KINDS
            .iter()
            .zip(caches.files.iter())
            .filter(|(kind, _)| benilla_protocol::PERSISTED_RESPONSES.contains(&kind.response))
            .flat_map(|(kind, file)| {
                file.entries().filter_map(move |(key, body)| {
                    let wire = codec::wire_of(kind, key, body);
                    match benilla_protocol::messages::parse_server(kind.response, &wire) {
                        Ok(packet) => Some(benilla_protocol::decode(packet)),
                        Err(e) => {
                            debug!("wdb: a {} record does not decode ({e})", kind.file);
                            None
                        }
                    }
                })
            })
            .flatten()
            .collect()
    };
    crate::net::handlers::dispatch(world, events);
}

/// An answer off the wire lands in its file's records; a refusal is never kept.
fn on_query_answer(In(ev): In<SessionEvent>, mut caches: ResMut<WdbCaches>) {
    let SessionEvent::QueryAnswer { opcode, body } = ev else {
        return;
    };
    if let Some((kind, key, record)) = codec::record_of(opcode, &body) {
        caches.file_mut(&kind).insert(key, record);
    }
}

/// `SetClientCacheVersion` (`0x635710`): each file whose version differs drops its records and
/// takes the server's; the game's caches drop theirs with it.
fn on_cache_version(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::ClientCacheVersion { version } = ev else {
        return;
    };
    let mut dropped = false;
    if let Some(mut caches) = world.get_resource_mut::<WdbCaches>() {
        for file in &mut caches.files {
            dropped |= file.set_server_version(version);
        }
    }
    if !dropped {
        return;
    }
    info!("wdb: the server's cache version is {version}; the cached answers are dropped");
    drop_persisted::<crate::names::NameCache>(world);
    drop_persisted::<crate::go_templates::GameObjectTemplates>(world);
    drop_persisted::<crate::items::Items>(world);
    drop_persisted::<crate::ui_gossip::GossipState>(world);
    drop_persisted::<crate::ui_quest_log::QuestLog>(world);
    drop_persisted::<crate::ui_item_text::PageTexts>(world);
    drop_persisted::<crate::ui_item_text::LetterTexts>(world);
}

fn drop_persisted<T: Persisted>(world: &mut World) {
    if let Some(mut owner) = world.get_resource_mut::<T>() {
        owner.drop_persisted();
    }
}

/// Exit: every file rewritten whole from what the session holds (`CREATE_ALWAYS`, `0x66a0dd`).
fn save(caches: Option<Res<WdbCaches>>) {
    let Some(caches) = caches else {
        return;
    };
    let Some(locale) = caches.locale.as_deref() else {
        return;
    };
    let Some(dir) = crate::local_state::wdb_dir(locale) else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn!("wdb: cannot create {}: {e}", dir.display());
        return;
    }
    let code = codec::locale_code(locale);
    for (kind, file) in KINDS.iter().zip(caches.files.iter()) {
        let path = dir.join(kind.file);
        if let Err(e) = std::fs::write(&path, file.encode(kind, code)) {
            warn!("wdb: cannot write {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A creature answer as 12340 stores it: four names, subname, cursor name, ten u32, two f32,
    /// the leader byte, six quest items and the movement id.
    fn wolf_body() -> Vec<u8> {
        let mut b = b"Wolf\0\0\0\0\0\0".to_vec();
        b.extend(std::iter::repeat_n(0u8, 10 * 4));
        b.extend_from_slice(&1.0f32.to_le_bytes());
        b.extend_from_slice(&1.0f32.to_le_bytes());
        b.push(0);
        b.extend(std::iter::repeat_n(0u8, 7 * 4));
        b
    }

    /// The file answers before the wire: a loaded record reaches the game's cache through the
    /// handlers on the first frame, and an answer off the wire lands in the file's records.
    #[test]
    fn a_cached_answer_reaches_the_game_and_a_new_one_reaches_the_file() {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        {
            let mut caches = app.world_mut().resource_mut::<WdbCaches>();
            caches
                .file_mut(&codec::CREATURE)
                .insert(69u32.to_le_bytes().to_vec(), wolf_body());
        }
        replay(app.world_mut());
        let names = app.world().resource::<crate::names::NameCache>();
        assert_eq!(
            names.creature_record(69).map(|r| r.name.as_str()),
            Some("Wolf"),
            "the record answered with no query sent"
        );

        let mut wire = 70u32.to_le_bytes().to_vec();
        wire.extend(wolf_body());
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::QueryAnswer {
                opcode: benilla_protocol::messages::opcode::SMSG_CREATURE_QUERY_RESPONSE,
                body: wire,
            }],
        );
        let caches = app.world().resource::<WdbCaches>();
        let keys: Vec<u32> = caches.files[0]
            .entries()
            .map(|(k, _)| u32::from_le_bytes(k.try_into().unwrap()))
            .collect();
        assert_eq!(keys, [69, 70]);
    }
}
