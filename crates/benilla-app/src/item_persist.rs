//! The item templates, kept across sessions: benilla's `itemcache.wdb` (`'WIDB'`, `0x9cb722`).
//!
//! Deviation: the file is `benilla-config/cache/<realm>-items.tsv`, per realm, as the creature
//! cache is ([`crate::name_persist`]), and it keeps each answer's wire body, decoded again on load.

use std::path::PathBuf;

use bevy::prelude::*;

use crate::char_select::Roster;
use crate::items::Items;

/// The header's first fields; a file with another header is discarded, as `DBCache` does.
const CACHE_MAGIC: &str = "benilla-itemcache";
const CACHE_FORMAT: u32 = 1;
const CACHE_BUILD: u32 = 12340;
const CACHE_LOCALE: &str = "enUS";
/// Seconds between writes, as the creature cache waits.
const SAVE_DEBOUNCE: f32 = 10.0;

pub(crate) struct ItemPersistPlugin;

impl Plugin for ItemPersistPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ItemCacheFile>()
            // Ahead of the frame's net events and feeds, which ask for and read templates.
            .add_systems(
                Update,
                (load_item_cache, save_item_cache)
                    .chain()
                    .before(benilla_world::schedule::WorldStage::Net),
            );
        crate::shutdown::on_app_exit(app, save_on_exit.into_configs());
    }
}

/// Where this realm's cache lives and what we last wrote there.
#[derive(Resource, Default)]
pub(crate) struct ItemCacheFile {
    realm: Option<String>,
    path: Option<PathBuf>,
    /// [`Items::template_epoch`] at the last write.
    saved_generation: u64,
    since_save: f32,
}

fn header(realm: &str) -> String {
    format!("{CACHE_MAGIC}\t{CACHE_FORMAT}\t{CACHE_BUILD}\t{CACHE_LOCALE}\t{realm}")
}

/// The file: the header, then `I <entry> <hex body>` per template and `i <entry>` per unknown.
fn to_tsv(items: &Items, realm: &str) -> String {
    let mut rows: Vec<(u32, Option<&[u8]>)> = items.cache_rows().collect();
    rows.sort_unstable_by_key(|(entry, _)| *entry);
    let mut out = header(realm);
    out.push('\n');
    for (entry, body) in rows {
        match body {
            Some(body) => {
                out.push_str(&format!("I\t{entry}\t"));
                for b in body {
                    out.push_str(&format!("{b:02x}"));
                }
                out.push('\n');
            }
            None => out.push_str(&format!("i\t{entry}\n")),
        }
    }
    out
}

/// One cached answer: the entry, its template (`None` unknown) and the body it came in.
type CachedRow = (
    u32,
    Option<benilla_protocol::messages::ItemInfo>,
    Option<Vec<u8>>,
);

/// The answers in `text`, `None` when its header is not this one; a line that does not decode is
/// skipped.
fn from_tsv(text: &str, realm: &str) -> Option<Vec<CachedRow>> {
    let mut lines = text.lines();
    if lines.next()? != header(realm) {
        return None;
    }
    let rows = lines.filter_map(|line| {
        let mut f = line.split('\t');
        let kind = f.next()?;
        let entry: u32 = f.next()?.parse().ok()?;
        match kind {
            "i" => Some((entry, None, None)),
            "I" => {
                let hex = f.next()?;
                let body = (0..hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
                    .collect::<Option<Vec<u8>>>()?;
                let (decoded, info) =
                    benilla_protocol::messages::decode_item_query_response(&body).ok()?;
                (decoded == entry && info.is_some()).then_some((entry, info, Some(body)))
            }
            _ => None,
        }
    });
    Some(rows.collect())
}

/// Loads the realm's cache the first frame its identity is known, ahead of the world's asks.
fn load_item_cache(roster: Res<Roster>, mut file: ResMut<ItemCacheFile>, mut items: ResMut<Items>) {
    let Some((realm, _)) = crate::ui_macro::identity(&roster) else {
        return;
    };
    if file.realm.as_deref() == Some(realm.as_str()) {
        return;
    }
    let path = crate::local_state::item_cache_path(&realm);
    file.realm = Some(realm.clone());
    file.path = path.clone();
    file.saved_generation = items.template_epoch();
    let Some(path) = path else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    match from_tsv(&text, &realm) {
        Some(rows) => {
            let n = rows.len();
            for (entry, info, body) in rows {
                items.install_cached(entry, info, body);
            }
            file.saved_generation = items.template_epoch();
            debug!(
                "items: loaded {n} cached templates for {realm} from {}",
                path.display()
            );
        }
        None => debug!(
            "items: discarding {} — header is not this build/locale/format",
            path.display()
        ),
    }
}

fn save_item_cache(time: Res<Time>, mut file: ResMut<ItemCacheFile>, items: Res<Items>) {
    file.since_save += time.delta_secs();
    if file.since_save < SAVE_DEBOUNCE {
        return;
    }
    file.since_save = 0.0;
    write_now(&mut file, &items);
}

fn save_on_exit(mut file: ResMut<ItemCacheFile>, items: Res<Items>) {
    write_now(&mut file, &items);
}

fn write_now(file: &mut ItemCacheFile, items: &Items) {
    if items.template_epoch() == file.saved_generation {
        return;
    }
    let (Some(path), Some(realm)) = (file.path.clone(), file.realm.clone()) else {
        return;
    };
    if let Some(dir) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            warn!("items: cannot create {}: {e}", dir.display());
            return;
        }
    }
    match crate::local_state::write_atomic(&path, &to_tsv(items, &realm)) {
        Ok(()) => {
            file.saved_generation = items.template_epoch();
            debug!("items: wrote the item cache to {}", path.display());
        }
        Err(e) => warn!("items: cannot write {}: {e}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal answer body: entry, class, subclass, sound, name, three empty names, then zeros.
    fn body(entry: u32, name: &str) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&entry.to_le_bytes());
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&(-1i32).to_le_bytes());
        b.extend_from_slice(name.as_bytes());
        b.extend_from_slice(&[0, 0, 0, 0]);
        b.resize(b.len() + 4096, 0);
        b
    }

    #[test]
    fn templates_and_unknown_entries_survive_a_write_and_a_load() {
        let mut items = Items::default();
        let b = body(6125, "Brawler's Harness");
        let (_, info) = benilla_protocol::messages::decode_item_query_response(&b).unwrap();
        items.insert_answer(6125, info, b.clone());
        items.insert_answer(99_999, None, Vec::new());
        let text = to_tsv(&items, "Realm");

        let rows = from_tsv(&text, "Realm").expect("same header");
        assert_eq!(rows.len(), 2);
        let (entry, info, body) = rows.iter().find(|r| r.0 == 6125).unwrap();
        assert_eq!(*entry, 6125);
        assert_eq!(info.as_ref().unwrap().name, "Brawler's Harness");
        assert_eq!(body.as_deref(), Some(b.as_slice()));
        assert!(rows.iter().any(|r| r.0 == 99_999 && r.1.is_none()));

        let mut loaded = Items::default();
        for (entry, info, body) in rows {
            loaded.install_cached(entry, info, body);
        }
        assert_eq!(
            loaded.template_cached(6125).map(|i| i.name.as_str()),
            Some("Brawler's Harness")
        );
        assert!(loaded.template_answered_unknown(99_999));
        assert_eq!(
            to_tsv(&loaded, "Realm"),
            text,
            "a load writes the same file back"
        );
    }

    #[test]
    fn another_realm_build_or_format_discards_the_file() {
        let items = Items::default();
        let text = to_tsv(&items, "Realm");
        assert!(from_tsv(&text, "Other").is_none());
        assert!(from_tsv(&text.replace("12340", "5875"), "Realm").is_none());
        assert!(from_tsv("", "Realm").is_none());
    }
}
