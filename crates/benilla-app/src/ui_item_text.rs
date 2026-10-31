//! The item-text reader (`ItemTextFrame.xml`) a bag letter, a book or a readable world object opens
//! in, locally with no packet as in the reference: every route ends at one read call,
//! `(guid, flag)`, which looks the guid up as any object and asks it for its text (`vtbl+0x74`), a
//! page chain from an item template's `PageText` or a `GAMEOBJECT_TYPE_TEXT` object's `data[0]`, or
//! else a copied mail letter's own text (`ITEM_FIELD_FLAGS & 0x200`), asked by the item's guid
//! (`0x58a1a0`).
//!
//! A book item the player has not read yet (no `ITEM_FIELD_FLAGS` `0x2` translated bit) asks the
//! server first: `ITEM_TEXT_BEGIN` fires, `CMSG_READ_ITEM` goes out, and `SMSG_READ_ITEM_OK` or
//! `_FAILED` decides (`0x58a1a0`, `0x6d5d90`). A used GOOBER's page arrives as
//! `SMSG_GAMEOBJECT_PAGETEXT` (`0x70be30`), the same read call.
//!
//! The events follow `ItemTextFrame.lua:10-96`. Unread pages pass the chat's translator in the
//! book's or object's language (`0x589f60`).

use bevy::prelude::*;

use benilla_ui::script::{ItemTextState, UiScript};

use crate::go_templates::GameObjectTemplates;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, Objects};
use crate::query_cache::QueryCache;
use crate::ui_script::{UiFeed, UiInput};

/// `ITEM_FIELD_FLAGS & 0x2`: the item was read once, so it shows with no read check.
const ITEM_FIELD_FLAG_TRANSLATED: u32 = 0x2;

/// One page of a book as the wire gave it (`SMSG_PAGE_TEXT_QUERY_RESPONSE`).
pub(crate) struct PageText {
    pub(crate) text: String,
    /// The next page's id; 0 on the last page.
    pub(crate) next: u32,
}

/// The ask-once page cache, the client's `PageText` WDB cache, keyed by page id. vmangos answers
/// one query with the whole chain; a missing page is still asked for alone.
#[derive(Resource, Default)]
pub(crate) struct PageTexts {
    pages: QueryCache<u32, PageText>,
}

/// `pagetextcache.wdb`.
impl crate::wdb::Persisted for PageTexts {
    fn drop_persisted(&mut self) {
        self.pages.clear();
    }
}

impl crate::query_cache::AskOnce for PageTexts {
    fn clear_pending(&mut self) {
        self.pages.clear_pending();
    }
}

impl PageTexts {
    pub(crate) fn insert(&mut self, page_id: u32, text: String, next: u32) {
        self.pages.insert(page_id, Some(PageText { text, next }));
    }

    /// The cached page, asked for once; `guid` is the reading object, which vmangos discards.
    fn get_or_ask(&self, page_id: u32, guid: u64, commands: &NetCommands) -> Option<&PageText> {
        self.pages.get_or_ask(page_id, || {
            debug!("page text: asking page {page_id} (object {guid:#x})");
            let _ = commands
                .0
                .send(ClientCommand::PageTextQuery { page_id, guid });
        })
    }
}

/// The ask-once item-text cache (`itemtextcache.wdb`), a copied letter's text by item guid.
#[derive(Resource, Default)]
pub(crate) struct LetterTexts {
    texts: QueryCache<u64, String>,
}

/// `itemtextcache.wdb`.
impl crate::wdb::Persisted for LetterTexts {
    fn drop_persisted(&mut self) {
        self.texts.clear();
    }
}

impl crate::query_cache::AskOnce for LetterTexts {
    fn clear_pending(&mut self) {
        self.texts.clear_pending();
    }
}

impl LetterTexts {
    pub(crate) fn insert(&mut self, item_guid: u64, text: String) {
        self.texts.insert(item_guid, Some(text));
    }

    fn get_or_ask(&self, item_guid: u64, commands: &NetCommands) -> Option<&String> {
        self.texts.get_or_ask(item_guid, || {
            debug!("item text: asking letter {item_guid:#x}");
            let _ = commands.0.send(ClientCommand::ItemTextQuery { item_guid });
        })
    }
}

/// Which of the reference's two text sources a session shows; the object decides, not the click.
pub(crate) enum ReadSource {
    /// A copied mail letter's own text: one body and a creator line.
    Letter,
    /// A page chain. `visited` runs from the first page to the one shown, as pages link only
    /// forwards; it is empty until the object's template answers, since the reference asks for the
    /// page id at paint time.
    Pages { visited: Vec<u32> },
}

/// The read check a book item takes before its text shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadCheck {
    /// Not judged yet: judged once `ITEM_TEXT_BEGIN` fires.
    Unchecked,
    /// `CMSG_READ_ITEM` sent, the answer not in.
    Waiting,
    /// Show the text, with the translation bar's ms when the answer read it untranslated.
    Show { translation_ms: Option<u32> },
}

pub(crate) struct ReadSession {
    /// An item's or a GameObject's guid; reading the same object again closes the reader.
    pub(crate) object_guid: u64,
    pub(crate) source: ReadSource,
    check: ReadCheck,
    /// Keyed on the VM, so a `/reload` re-fires `ITEM_TEXT_BEGIN` for a reader still open.
    told: crate::ui_script::VmMemo<ItemTextTold>,
}

#[derive(Default)]
struct ItemTextTold {
    /// `ITEM_TEXT_BEGIN` fired: once per open, before the text lands.
    begun: bool,
    ready: bool,
}

#[derive(Resource, Default)]
pub(crate) struct ItemTextOpen {
    pub(crate) pending: Option<ReadSession>,
    /// A toggle closed the reader and the frame is not told yet; the feed holds the script.
    closing: bool,
}

impl ItemTextOpen {
    /// Open a bag letter; re-opening restarts from `ITEM_TEXT_BEGIN`.
    pub(crate) fn open_letter(&mut self, item_guid: u64) {
        self.open(item_guid, ReadSource::Letter);
    }

    /// Open a page chain; the feed asks the object's template for the first page.
    pub(crate) fn open_pages(&mut self, object_guid: u64) {
        self.open(
            object_guid,
            ReadSource::Pages {
                visited: Vec::new(),
            },
        );
    }

    /// The read check's answer for the open book: `Some` shows it, `None` closes it. An answer for
    /// another object is dropped (`0x589e90`).
    fn answer(&mut self, item: u64, check: Option<ReadCheck>) {
        let Some(sess) = self.pending.as_mut().filter(|s| s.object_guid == item) else {
            return;
        };
        match check {
            Some(c) => sess.check = c,
            None => {
                self.pending = None;
                self.closing = true;
            }
        }
    }

    /// The reference's toggle, the read call with flag 0 from both click routes: reading the open
    /// object again closes it. Returns whether it closed.
    pub(crate) fn toggle_closed(&mut self, object_guid: u64) -> bool {
        let open = self
            .pending
            .as_ref()
            .is_some_and(|s| s.object_guid == object_guid);
        if open {
            self.pending = None;
            self.closing = true;
        }
        open
    }

    fn take_closing(&mut self) -> bool {
        std::mem::take(&mut self.closing)
    }

    fn open(&mut self, object_guid: u64, source: ReadSource) {
        self.pending = Some(ReadSession {
            object_guid,
            source,
            check: ReadCheck::Unchecked,
            told: Default::default(),
        });
    }
}

/// The page-text and item-text replies' handlers.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{LetterTexts, PageTexts};
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::PageText, on_page_text)
            .net_handler(SessionEventKind::ItemText, on_item_text)
            .net_handler(SessionEventKind::ReadItemOk, on_read_item_ok)
            .net_handler(SessionEventKind::ReadItemFailed, on_read_item_failed)
            .net_handler(
                SessionEventKind::GameObjectPageText,
                on_gameobject_page_text,
            );
    }

    /// `SMSG_READ_ITEM_OK`: the item's translated bit goes on locally (`0x707060`) and the text
    /// shows.
    fn on_read_item_ok(
        In(ev): In<SessionEvent>,
        mut open: ResMut<super::ItemTextOpen>,
        index: Res<crate::net::GuidIndex>,
        mut stores: Query<&mut crate::net::ObjectStore>,
        mut edges: MessageWriter<crate::net::FieldChanged>,
    ) {
        let SessionEvent::ReadItemOk { item } = ev else {
            return;
        };
        debug!("item text: read check passed for {item:#x}");
        if let Some((entity, mut store)) = index
            .0
            .get(&item)
            .and_then(|&e| Some((e, stores.get_mut(e).ok()?)))
        {
            let flags = store.0.item_flags().unwrap_or(0) | super::ITEM_FIELD_FLAG_TRANSLATED;
            let delta = benilla_protocol::messages::ObjectFields::from_pairs(&[(
                benilla_protocol::field::FIELD_ITEM_FLAGS,
                flags,
            )]);
            crate::net::merge_store_fields(&mut store.0, delta, entity, item, |e| {
                edges.write(e);
            });
        }
        open.answer(
            item,
            Some(super::ReadCheck::Show {
                translation_ms: None,
            }),
        );
    }

    /// `SMSG_READ_ITEM_FAILED`: reason 0 shows the text, 1 shows it untranslated with the
    /// translation bar, any other closes the reader.
    fn on_read_item_failed(In(ev): In<SessionEvent>, mut open: ResMut<super::ItemTextOpen>) {
        let SessionEvent::ReadItemFailed { item, failure } = ev else {
            return;
        };
        debug!("item text: read check failed for {item:#x}: {failure:?}");
        use benilla_protocol::messages::ReadItemFailure as F;
        let check = match failure {
            F::Show => Some(super::ReadCheck::Show {
                translation_ms: None,
            }),
            F::Untranslated { ms } => Some(super::ReadCheck::Show {
                translation_ms: Some(ms),
            }),
            F::Close => None,
        };
        open.answer(item, check);
    }

    /// `SMSG_GAMEOBJECT_PAGETEXT` (`0x70be30`): the read call with flag 0 on the GameObject, so a
    /// page already open closes.
    fn on_gameobject_page_text(In(ev): In<SessionEvent>, mut open: ResMut<super::ItemTextOpen>) {
        let SessionEvent::GameObjectPageText { guid } = ev else {
            return;
        };
        if !benilla_protocol::guid::is_gameobject(guid) {
            return;
        }
        if !open.toggle_closed(guid) {
            debug!("item text: read gameobject page {guid:#x}");
            open.open_pages(guid);
        }
    }

    /// `SMSG_ITEM_TEXT_QUERY_RESPONSE` (`0x635a40`): the letter's text lands in the cache.
    fn on_item_text(In(ev): In<SessionEvent>, mut texts: ResMut<LetterTexts>) {
        if let SessionEvent::ItemText { item_guid, text } = ev {
            debug!(
                "item text: letter {item_guid:#x} answered, {} byte(s)",
                text.len()
            );
            texts.insert(item_guid, text);
        }
    }

    /// One page per packet; the reader repaints off the cache on the next feed.
    fn on_page_text(In(ev): In<SessionEvent>, mut pages: ResMut<PageTexts>) {
        if let SessionEvent::PageText {
            page_id,
            text,
            next_page_id,
        } = ev
        {
            pages.insert(page_id, text, next_page_id);
        }
    }
}

pub(crate) struct UiItemTextPlugin;

impl Plugin for UiItemTextPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        crate::query_cache::register::<PageTexts>(app);
        crate::query_cache::register::<LetterTexts>(app);
        app.init_resource::<ItemTextOpen>()
            .init_resource::<PageTexts>()
            .init_resource::<LetterTexts>()
            .add_systems(
                Update,
                (
                    // Feed before the input pass so an open paints this frame; drain after it
                    // so a close clears this frame.
                    // After the language feeds, whose skills and words the translator reads.
                    feed_item_text
                        .in_set(UiFeed)
                        .after(crate::ui_chat::load_language_words)
                        .after(crate::ui_chat::feed_language_skills),
                    drain_item_text.after(UiInput),
                ),
            );
    }
}

/// What the read object answers: its name for the title (`ItemTextGetItem`), its material
/// (`ItemTextGetMaterial`: an item's `PageMaterial`, a GameObject's `data[2]`) and its first page.
struct Readable {
    title: String,
    /// The `PageTextMaterial.dbc` basename; `None` is the Lua's Parchment default.
    material: Option<String>,
    /// The first `PageText` id; 0 for a letter or a template with no page.
    page_head: u32,
    /// The pages' language: an item template's `LanguageID`, a GameObject's prop `0x10`.
    language: u32,
    /// An item already read (`ITEM_FIELD_FLAGS & 0x2`) shows its pages as written.
    translated: bool,
}

/// The material a quest from `guid` paints on, `GetQuestBackgroundMaterial`, the body of
/// `ItemTextGetMaterial`; `None` is FrameXML's Parchment. Asks for an uncached GameObject.
pub(crate) fn object_material(
    guid: u64,
    objects: &Objects,
    items: &Items,
    go_templates: &GameObjectTemplates,
    materials: Option<&PageMaterials>,
    commands: &NetCommands,
) -> Option<String> {
    let name = |id: u32| materials.and_then(|m| m.0.name(id)).map(str::to_string);
    if benilla_protocol::guid::is_gameobject(guid) {
        return match go_templates.get(guid) {
            Some(go) => go.quest_material.and_then(name),
            None => {
                go_templates.request(guid, commands);
                None
            }
        };
    }
    let entry = objects.object(guid)?.object_entry()?;
    let t = items.template(entry, guid, commands)?;
    name(t.page_material)
}

/// The object's [`Readable`], item or GameObject alike; `None` while its template is in flight.
fn readable(
    guid: u64,
    objects: &Objects,
    items: &Items,
    go_templates: &GameObjectTemplates,
    materials: Option<&PageMaterials>,
    commands: &NetCommands,
) -> Option<Readable> {
    let name = |id: u32| materials.and_then(|m| m.0.name(id)).map(str::to_string);
    if let Some(go) = go_templates.get(guid) {
        return Some(Readable {
            title: go.name.clone(),
            material: go.text_page.and_then(|p| name(p.material)),
            page_head: go.text_page.map_or(0, |p| p.page_id),
            language: go.text_page.map_or(0, |p| p.language),
            translated: false,
        });
    }
    let fields = objects.object(guid)?;
    let entry = fields.object_entry()?;
    let t = items.template(entry, guid, commands)?;
    Some(Readable {
        title: t.name.clone(),
        material: name(t.page_material),
        page_head: t.page_text,
        language: t.language_id,
        translated: fields.item_flags().unwrap_or(0) & ITEM_FIELD_FLAG_TRANSLATED != 0,
    })
}

/// Drive the open session to `ITEM_TEXT_BEGIN` once the template resolves, as the stock handler
/// takes the text colour from the material (`ItemTextFrame.lua:18-23`), then to
/// `ITEM_TEXT_READY`, which shows the window, once the body and any creator line are in.
fn feed_item_text(
    script: Option<NonSendMut<UiScript>>,
    mut open: ResMut<ItemTextOpen>,
    letters: Res<LetterTexts>,
    pages: Res<PageTexts>,
    objects: Objects,
    items: Res<Items>,
    names: Res<NameCache>,
    go_templates: Res<GameObjectTemplates>,
    materials: Option<Res<PageMaterials>>,
    commands: Res<NetCommands>,
    // The `$`-macro subject for the page body: the local player.
    self_q: Query<(&crate::net::ObjectStore, &crate::net::Guid), With<crate::net::SelfPlayer>>,
    states: Res<crate::world_state::WorldStates>,
    // The reader's language skill and the garble word pool, the chat's own.
    languages: Option<Res<crate::ui_chat::ChatLanguages>>,
) {
    let Some(mut script) = script else {
        return;
    };
    // A toggle closed the reader; the frame is told here, where the script is.
    if open.take_closing() {
        script.set_item_text(None);
        script.fire_event("ITEM_TEXT_CLOSED", vec![]);
    }
    let Some(sess) = open.pending.as_mut() else {
        return;
    };
    if sess.told.get(&script).ready {
        return;
    }
    let Some(readable) = readable(
        sess.object_guid,
        &objects,
        &items,
        &go_templates,
        materials.as_deref(),
        &commands,
    ) else {
        return; // template in flight; the reference re-enters the open when it lands
    };

    // No page to show: the reference bails before firing anything.
    if matches!(sess.source, ReadSource::Pages { .. }) && readable.page_head == 0 {
        debug!(
            "item text: {:#x} has no page to read — closing",
            sess.object_guid
        );
        open.pending = None;
        return;
    }

    if !sess.told.get(&script).begun {
        script.set_item_text(Some(ItemTextState {
            item: readable.title.clone(),
            creator: None,
            text: String::new(),
            page: 1,
            has_next: false,
            material: readable.material.clone(),
        }));
        script.fire_event("ITEM_TEXT_BEGIN", vec![]);
        sess.told.get(&script).begun = true;
    }

    // The read check (`0x58a2a0`): a book item not yet read asks the server by its bag position;
    // a letter, a read book and a GameObject show at once. An item with no bag position sends
    // nothing and waits.
    if sess.check == ReadCheck::Unchecked {
        let unread = !benilla_protocol::guid::is_gameobject(sess.object_guid)
            && matches!(sess.source, ReadSource::Pages { .. })
            && objects.object(sess.object_guid).is_some_and(|f| {
                f.item_flags().unwrap_or(0) & (ITEM_FIELD_FLAG_TRANSLATED | 0x200) == 0
            });
        sess.check = if unread {
            let at = self_q.iter().next().and_then(|(store, _)| {
                crate::ui_items::collect_inventory(
                    &store.0,
                    &objects,
                    crate::ui_items::InventoryScope::DEFAULT,
                )
                .into_iter()
                .find(|&(_, _, g)| g == sess.object_guid)
            });
            if let Some((bag, slot, _)) = at {
                debug!(
                    "item text: read check for {:#x} at bag {bag} slot {slot}",
                    sess.object_guid
                );
                let _ = commands.0.send(ClientCommand::ReadItem { bag, slot });
            }
            ReadCheck::Waiting
        } else {
            ReadCheck::Show {
                translation_ms: None,
            }
        };
    }
    let ReadCheck::Show { translation_ms } = sess.check else {
        return; // the answer is in flight
    };

    let (creator, text, page, has_next) = match &mut sess.source {
        ReadSource::Letter => {
            let creator_guid = objects
                .object(sess.object_guid)
                .and_then(|o| o.item_creator());
            let creator = match creator_guid {
                None => None, // authorless
                Some(guid) => match names.resolve(guid, &commands) {
                    Some(name) => Some(name.to_string()),
                    None => return, // name query in flight
                },
            };
            let body = letters.get_or_ask(sess.object_guid, &commands).cloned();
            let Some(text) = body else {
                return; // body query in flight
            };
            (creator, text, 1, false)
        }
        ReadSource::Pages { visited } => {
            if visited.is_empty() {
                visited.push(readable.page_head);
            }
            let page_id = *visited.last().expect("seeded just above");
            let Some(page) = pages.get_or_ask(page_id, sess.object_guid, &commands) else {
                return; // page query in flight
            };
            // A book has no creator line.
            (
                None,
                page.text.clone(),
                visited.len() as u32,
                page.next != 0,
            )
        }
    };

    // Server-authored text runs the `$`-macro expander, as in the reference's item-text frame.
    let subject = crate::npc_text::player_identity(&self_q, &names, &commands);
    let text = crate::npc_text::substitute(
        &text,
        &crate::npc_text::MacroContext {
            subject: subject.as_ref(),
            states: &states,
            achievement: None,
        },
    );
    // A page not yet read shows through the translator (`0x589f60`): at the reader's skill, or
    // none when the check read it untranslated. A letter and a read book show as written.
    let text = match languages.as_deref() {
        Some(langs) if matches!(sess.source, ReadSource::Pages { .. }) && !readable.translated => {
            let skill = match translation_ms {
                Some(_) => 0,
                None => langs.skill(readable.language),
            };
            langs.translate(readable.language, skill, &text)
        }
        _ => text,
    };
    script.set_item_text(Some(ItemTextState {
        item: readable.title,
        creator,
        text,
        page,
        has_next,
        material: readable.material,
    }));
    script.fire_event("ITEM_TEXT_READY", vec![]);
    sess.told.get(&script).ready = true;
    // An untranslated read starts the translation bar, its seconds the answer's ms (`0x6d5e2a`).
    if let Some(ms) = translation_ms {
        sess.check = ReadCheck::Show {
            translation_ms: None,
        };
        script.fire_event(
            "ITEM_TEXT_TRANSLATION",
            vec![benilla_ui::script::ScriptValue::Number(
                f64::from(ms) * 0.001,
            )],
        );
    }
}

/// `PageTextMaterial.dbc`; absent without client data, leaving every reader on Parchment.
#[derive(Resource)]
pub(crate) struct PageMaterials(pub(crate) benilla_formats::PageTextMaterialCatalog);

/// Drain the reader's intents: `CloseItemText()` clears the session and fires `ITEM_TEXT_CLOSED`;
/// a page turn re-runs the feed from `ITEM_TEXT_READY`, as the reference does not re-fire BEGIN.
fn drain_item_text(
    script: Option<NonSendMut<UiScript>>,
    mut open: ResMut<ItemTextOpen>,
    pages: Res<PageTexts>,
) {
    let Some(mut script) = script else {
        return;
    };
    for delta in script.take_item_text_page_turns() {
        let Some(sess) = open.pending.as_mut() else {
            continue;
        };
        let ReadSource::Pages { visited } = &mut sess.source else {
            continue; // a letter has no pages; its buttons never show
        };
        if turn_page(visited, delta, |id| pages.pages.get(id).map(|p| p.next)) {
            // Repaint on the next feed without re-firing BEGIN.
            sess.told.get(&script).ready = false;
        }
    }
    if script.take_item_text_close() && open.pending.take().is_some() {
        script.set_item_text(None);
        script.fire_event("ITEM_TEXT_CLOSED", vec![]);
    }
}

/// Walk the trail one step: Next pushes the page's `nextPageId` from `next_of`, the cache, and Prev
/// pops. A page not landed or a last page refuses; returns whether the page changed.
fn turn_page(visited: &mut Vec<u32>, delta: i32, next_of: impl Fn(u32) -> Option<u32>) -> bool {
    let Some(&current) = visited.last() else {
        return false; // head not resolved yet
    };
    if delta > 0 {
        match next_of(current) {
            Some(next) if next != 0 => visited.push(next),
            _ => return false,
        }
    } else if visited.len() > 1 {
        visited.pop();
    } else {
        return false; // already on page 1
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_turns_walk_the_chain_both_ways() {
        let chain = |id: u32| match id {
            100 => Some(101),
            101 => Some(102),
            102 => Some(0), // last page
            _ => None,
        };
        let mut visited = vec![100];
        assert!(!turn_page(&mut visited, -1, chain), "page 1 has no Prev");
        assert_eq!(visited, [100]);

        assert!(turn_page(&mut visited, 1, chain));
        assert!(turn_page(&mut visited, 1, chain));
        assert_eq!(visited, [100, 101, 102]);
        assert!(
            !turn_page(&mut visited, 1, chain),
            "the last page has no Next"
        );

        assert!(turn_page(&mut visited, -1, chain));
        assert_eq!(visited, [100, 101], "Prev pops the forward-only trail");
    }

    #[test]
    fn a_page_still_in_flight_refuses_the_turn() {
        let mut visited = vec![7];
        assert!(!turn_page(&mut visited, 1, |_| None));
        assert_eq!(visited, [7]);
    }

    #[test]
    fn re_reading_the_same_object_toggles_closed() {
        let mut open = ItemTextOpen::default();
        open.open_pages(0xF110_0000_0000_0001);
        assert!(!open.toggle_closed(0xF110_0000_0000_0002), "another book");
        assert!(open.pending.is_some());
        assert!(open.toggle_closed(0xF110_0000_0000_0001));
        assert!(open.pending.is_none());
        assert!(
            open.take_closing(),
            "the frame must be told — a cleared session alone leaves the window on screen"
        );
        assert!(!open.take_closing(), "drained");
        assert!(!open.toggle_closed(0xF110_0000_0000_0001), "nothing open");
        assert!(!open.take_closing(), "a no-op toggle closes nothing");
    }

    #[test]
    fn a_page_read_opens_before_its_template_lands() {
        let mut open = ItemTextOpen::default();
        open.open_pages(0xF110_0000_0000_0001);
        let sess = open.pending.as_ref().expect("open");
        assert!(matches!(&sess.source, ReadSource::Pages { visited } if visited.is_empty()));
    }
}

#[cfg(test)]
mod quest_material_tests {
    use super::*;

    fn go_guid(entry: u64) -> u64 {
        (u64::from(benilla_protocol::guid::HIGH_GAMEOBJECT) << 48) | (entry << 24) | 1
    }

    /// A GameObject's material is its template data by type; a creature's is Parchment.
    #[test]
    fn a_quest_sourced_from_an_object_paints_the_objects_material() {
        let items = Items::default();
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let mut gos = GameObjectTemplates::default();
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let catalog = PageMaterials(benilla_formats::PageTextMaterialCatalog::from_rows(&[
            (1, "Parchment"),
            (2, "Stone"),
            (3, "Marble"),
        ]));
        let mut data = [0i32; 24];
        data[2] = 2;
        data[9] = 3;
        gos.insert(100, 2, "Wanted Poster".into(), &data);
        gos.insert(102, 10, "Chest".into(), &data);
        assert_eq!(
            object_material(
                go_guid(100),
                &objects,
                &items,
                &gos,
                Some(&catalog),
                &commands
            )
            .as_deref(),
            Some("Stone")
        );
        assert_eq!(
            object_material(
                go_guid(102),
                &objects,
                &items,
                &gos,
                Some(&catalog),
                &commands
            )
            .as_deref(),
            Some("Marble"),
            "a chest reads data[9]"
        );
        // A creature quest-giver: neither template, nothing asked.
        let creature = (u64::from(benilla_protocol::guid::HIGH_UNIT) << 48) | (6 << 24) | 1;
        assert_eq!(
            object_material(creature, &objects, &items, &gos, Some(&catalog), &commands),
            None
        );
        assert!(rx.try_recv().is_err(), "a creature source asks for nothing");
        // An uncached object: nothing yet, and the template is asked for.
        assert_eq!(
            object_material(
                go_guid(777),
                &objects,
                &items,
                &gos,
                Some(&catalog),
                &commands
            ),
            None
        );
        assert!(
            rx.try_recv().is_ok(),
            "an uncached object's template is asked for"
        );
    }
}
