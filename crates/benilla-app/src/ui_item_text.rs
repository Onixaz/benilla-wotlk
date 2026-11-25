//! The item-text reader (`ItemTextFrame.xml`) a bag letter, a book or a readable world object opens in, with no packet: every route ends at the read call (`0x58a1a0`), which holds the object, takes the interact target and fires `ITEM_TEXT_BEGIN` once its first page (an item record's `PageText`, a text GameObject's page property) or a copied letter's text (`ITEM_FIELD_FLAGS & 0x200`) is cached, and else leaves its callback on the miss (`0x58a450`).
//!
//! The draw (`0x589e90`) sets the text and fires `ITEM_TEXT_READY`; an unread book asks the server first, `SMSG_READ_ITEM_OK` or `_FAILED` drawing it (`0x6d5d90`), and a page not yet read passes the chat's translator in its language. The verbs read the globals at the call; a page turn draws, and `CloseItemText` closes, inside its call.

use bevy::prelude::*;

use crate::go_templates::GameObjectTemplates;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, Objects};
use crate::query_cache::QueryCache;

/// `ITEM_FIELD_FLAGS & 0x2`: the item was read once, so it shows with no read check.
const ITEM_FIELD_FLAG_TRANSLATED: u32 = 0x2;

/// `ITEM_FIELD_FLAGS & 0x200`: a copied mail letter, its text asked by item guid (`0x58a2bc`).
const ITEM_FIELD_FLAG_LETTER: u32 = 0x200;

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

    /// The server has no such page: its callback does nothing (`0x58a453`), so the page stays
    /// blank, and the negative keeps the per-frame read from asking again.
    pub(crate) fn insert_miss(&mut self, page_id: u32) {
        self.pages.insert(page_id, None);
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

/// The reader's globals: the read object (`0xbfc948`), the letter flag (`0xbfa9f8`), the page index (`0xbfc940`), the page ids (`0xbfc958`) and the drawn text (`0xbfaa00`), set up as the game starts (`0x589dc0`).
#[derive(Resource)]
pub(crate) struct ItemText {
    pub(crate) guid: u64,
    has_item_text: bool,
    current_page: usize,
    /// The page ids from the first: the entry past the current one is its next page, 0 for none.
    pages: Vec<u32>,
    text: String,
    /// The reader's callbacks a cache miss left (`0x58a450`), each with the object it asked for.
    waits: Vec<(Wait, u64)>,
}

impl Default for ItemText {
    fn default() -> Self {
        Self {
            guid: 0,
            has_item_text: false,
            current_page: 0,
            pages: vec![0, 0],
            text: String::new(),
            waits: Vec::new(),
        }
    }
}

/// A record a reader's callback waits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wait {
    /// A page in the page cache.
    Page(u32),
    /// A copied letter's text in the item-text cache, by item guid.
    Letter(u64),
    /// A creator's name in the name cache.
    Name(u64),
}

impl ItemText {
    fn wait(&mut self, wait: Wait, guid: u64) {
        if !self.waits.contains(&(wait, guid)) {
            self.waits.push((wait, guid));
        }
    }

    /// The objects whose callbacks waited on `wait`, in ask order, unlinked.
    fn take(&mut self, wait: Wait) -> Vec<u64> {
        let mut taken = Vec::new();
        self.waits.retain(|&(w, g)| {
            let hit = w == wait;
            if hit {
                taken.push(g);
            }
            !hit
        });
        taken
    }

    /// A new object: page 0, no pages, no text (`0x58a20c`-`0x58a247`).
    fn reset(&mut self, guid: u64) {
        self.guid = guid;
        self.has_item_text = false;
        self.current_page = 0;
        self.pages[0] = 0;
        self.pages[1] = 0;
        self.text.clear();
    }
}

/// The page-text and item-text replies' handlers.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{ItemText, LetterTexts, PageTexts, Wait};
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::PageText, on_page_text)
            .net_handler(SessionEventKind::PageTextMiss, on_page_text_miss)
            .net_handler(SessionEventKind::ItemText, on_item_text)
            .net_handler(SessionEventKind::ReadItemOk, on_read_item_ok)
            .net_handler(SessionEventKind::ReadItemFailed, on_read_item_failed)
            .net_handler(
                SessionEventKind::GameObjectPageText,
                on_gameobject_page_text,
            )
            .net_handler(SessionEventKind::Disconnected, on_session_end);
    }

    /// `SMSG_READ_ITEM_OK` (`0x6d5d90`): the held item's translated bit goes on locally (`0x707060`) and the text draws.
    fn on_read_item_ok(In(ev): In<SessionEvent>, world: &mut World) {
        let SessionEvent::ReadItemOk { item } = ev else {
            return;
        };
        debug!("item text: read check passed for {item:#x}");
        let entity = world
            .get_resource::<crate::net::GuidIndex>()
            .and_then(|i| i.0.get(&item).copied());
        let Some(entity) = entity else {
            return;
        };
        let mut state = bevy::ecs::system::SystemState::<(
            Query<&mut crate::net::ObjectStore>,
            MessageWriter<crate::net::FieldChanged>,
        )>::new(world);
        let (mut stores, mut edges) = state.get_mut(world);
        let Ok(mut store) = stores.get_mut(entity) else {
            return;
        };
        let flags = store.0.item_flags().unwrap_or(0) | super::ITEM_FIELD_FLAG_TRANSLATED;
        let delta = benilla_protocol::messages::ObjectFields::from_pairs(&[(
            benilla_protocol::field::FIELD_ITEM_FLAGS,
            flags,
        )]);
        crate::net::merge_store_fields(&mut store.0, delta, entity, item, |e| {
            edges.write(e);
        });
        state.apply(world);
        super::display_text(world, item, true);
    }

    /// `SMSG_READ_ITEM_FAILED` (`0x6d5d90`): reason 0 draws the text, 1 draws it untranslated and starts the translation bar, any other fires the close for the held object and leaves the reader set.
    fn on_read_item_failed(In(ev): In<SessionEvent>, world: &mut World) {
        let SessionEvent::ReadItemFailed { item, failure } = ev else {
            return;
        };
        debug!("item text: read check failed for {item:#x}: {failure:?}");
        use benilla_protocol::messages::ReadItemFailure as F;
        match failure {
            F::Show => super::display_text(world, item, true),
            F::Untranslated { ms } => {
                super::display_text(world, item, false);
                super::fire(
                    world,
                    "ITEM_TEXT_TRANSLATION",
                    vec![benilla_ui::script::ScriptValue::Number(
                        f64::from(ms) * 0.001,
                    )],
                );
            }
            F::Close => {
                if world.resource::<ItemText>().guid == item {
                    super::fire(world, "ITEM_TEXT_CLOSED", Vec::new());
                }
            }
        }
    }

    /// `SMSG_GAMEOBJECT_PAGETEXT` (`0x70be30`): the read call with flag 0 on a held GameObject, so a page already open closes.
    fn on_gameobject_page_text(In(ev): In<SessionEvent>, world: &mut World) {
        let SessionEvent::GameObjectPageText { guid } = ev else {
            return;
        };
        let held = world
            .get_resource::<crate::net::GuidIndex>()
            .is_some_and(|i| i.0.contains_key(&guid));
        if benilla_protocol::guid::is_gameobject(guid) && held {
            debug!("item text: read gameobject page {guid:#x}");
            super::set_item(world, guid, false);
        }
    }

    /// `SMSG_ITEM_TEXT_QUERY_RESPONSE` (`0x635a40`): the letter's text lands in the cache, then its callbacks run.
    fn on_item_text(In(ev): In<SessionEvent>, world: &mut World) {
        if let SessionEvent::ItemText { item_guid, text } = ev {
            debug!(
                "item text: letter {item_guid:#x} answered, {} byte(s)",
                text.len()
            );
            world.resource_mut::<LetterTexts>().insert(item_guid, text);
            super::callbacks(world, Wait::Letter(item_guid), true);
        }
    }

    /// A page the server lacks: its callbacks run with no record, so do nothing (`0x58a453`).
    fn on_page_text_miss(In(ev): In<SessionEvent>, world: &mut World) {
        if let SessionEvent::PageTextMiss { page_id } = ev {
            debug!("page text: no page {page_id}");
            world.resource_mut::<PageTexts>().insert_miss(page_id);
            super::callbacks(world, Wait::Page(page_id), false);
        }
    }

    /// One page per packet; the page's callbacks run once it is cached.
    fn on_page_text(In(ev): In<SessionEvent>, world: &mut World) {
        if let SessionEvent::PageText {
            page_id,
            text,
            next_page_id,
        } = ev
        {
            world
                .resource_mut::<PageTexts>()
                .insert(page_id, text, next_page_id);
            super::callbacks(world, Wait::Page(page_id), true);
        }
    }

    /// The disconnect leaves the world (`0x58a430`); the asks in flight die with the socket.
    fn on_session_end(In(_): In<SessionEvent>, world: &mut World) {
        super::set_item(world, 0, false);
        if let Some(mut text) = world.get_resource_mut::<ItemText>() {
            text.waits.clear();
        }
    }
}

pub(crate) struct UiItemTextPlugin;

impl Plugin for UiItemTextPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        crate::query_cache::register::<PageTexts>(app);
        crate::query_cache::register::<LetterTexts>(app);
        app.init_resource::<ItemText>()
            .init_resource::<PageTexts>()
            .init_resource::<LetterTexts>();
    }
}

/// The world leave's reader part (`0x528ce2` -> `0x58a430`): the reader closes.
pub(crate) fn leave_world(world: &mut World) {
    set_item(world, 0, false);
}

/// The read call from a click or a use (`0x70c86d`, `0x708f60`), its events fired once it runs.
pub(crate) fn read_from_click(commands: &mut Commands, guid: u64) {
    commands.queue(move |world: &mut World| {
        set_item(world, guid, false);
        crate::ui_script::live::fire_handler_events(world);
    });
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
                go_templates.request(guid, None, commands);
                None
            }
        };
    }
    let entry = objects.object(guid)?.object_entry()?;
    let t = items.template(entry, guid, commands)?;
    name(t.page_material)
}

/// What the reader's calls read of the world: the objects, the record caches, our body and the translator.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ItemTextCtx<'w, 's> {
    objects: Option<Objects<'w, 's>>,
    me: Query<
        'w,
        's,
        (&'static crate::net::ObjectStore, &'static crate::net::Guid),
        With<crate::net::SelfPlayer>,
    >,
    my_auras: Query<'w, 's, &'static crate::unit_auras::UnitAuras, With<crate::net::SelfPlayer>>,
    items: Option<Res<'w, Items>>,
    go_templates: Option<Res<'w, GameObjectTemplates>>,
    pages: Option<Res<'w, PageTexts>>,
    letters: Option<Res<'w, LetterTexts>>,
    names: Option<Res<'w, NameCache>>,
    states: Option<Res<'w, crate::world_state::WorldStates>>,
    languages: Option<Res<'w, crate::ui_chat::ChatLanguages>>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    skill_lines: Option<Res<'w, crate::ui_spellbook::SkillLines>>,
    materials: Option<Res<'w, PageMaterials>>,
    commands: Option<Res<'w, NetCommands>>,
}

/// An object's page head as its page-id call answers (`vtbl+0xdc`).
enum PageHead {
    Page(u32),
    /// An item whose record is in flight, asked with the reader's callback on it (`0x707180`).
    ItemRecord(u32),
}

impl ItemTextCtx<'_, '_> {
    fn object(&self, guid: u64) -> Option<&benilla_protocol::ObjectFields> {
        self.objects.as_ref()?.object(guid)
    }

    fn is_item(&self, guid: u64) -> bool {
        benilla_protocol::guid::is_item(guid) && self.object(guid).is_some()
    }

    /// An item's cached record, asked on a miss.
    fn item_record(&self, guid: u64) -> Option<&benilla_protocol::messages::ItemInfo> {
        let entry = self.object(guid)?.object_entry()?;
        self.items
            .as_deref()?
            .template(entry, guid, self.commands.as_deref()?)
    }

    /// The page id call: an item's record's page (`0x707180`), a GameObject's page property (`0x710280`), else 0.
    fn page_head(&self, guid: u64) -> PageHead {
        if self.is_item(guid) {
            let entry = self
                .object(guid)
                .and_then(|o| o.object_entry())
                .unwrap_or(0);
            return match self.item_record(guid) {
                Some(t) => PageHead::Page(t.page_text),
                None => PageHead::ItemRecord(entry),
            };
        }
        PageHead::Page(
            self.go_templates
                .as_deref()
                .and_then(|g| g.get(guid))
                .and_then(|t| t.text_page)
                .map_or(0, |p| p.page_id),
        )
    }

    /// The cached page, asked on a miss.
    fn page(&self, page_id: u32, guid: u64) -> Option<&PageText> {
        self.pages
            .as_deref()?
            .get_or_ask(page_id, guid, self.commands.as_deref()?)
    }

    /// The cached letter, asked on a miss.
    fn letter(&self, guid: u64) -> Option<&String> {
        self.letters
            .as_deref()?
            .get_or_ask(guid, self.commands.as_deref()?)
    }

    /// `ItemTextGetItem` (`0x589c40`): the object's name, an item's from its record with no callback (`0x7071d0`).
    fn name(&self, guid: u64) -> Option<String> {
        if self.is_item(guid) {
            return self.item_record(guid).map(|t| t.name.clone());
        }
        self.object(guid)?;
        self.go_templates
            .as_deref()?
            .get(guid)
            .map(|t| t.name.clone())
    }

    /// `ItemTextGetMaterial` (`0x589c90`): an item's record's material, a GameObject's page material property, its `PageTextMaterial.dbc` name when above 0.
    fn material(&self, guid: u64) -> Option<String> {
        let material = if self.is_item(guid) {
            self.item_record(guid)?.page_material
        } else if benilla_protocol::guid::is_gameobject(guid) && self.object(guid).is_some() {
            self.go_templates.as_deref()?.get(guid)?.text_page?.material
        } else {
            return None;
        };
        if material == 0 {
            return None;
        }
        self.materials
            .as_deref()?
            .0
            .name(material)
            .map(str::to_string)
    }

    /// Our skill in `language` (`0x6e0640`), 0 with no body.
    fn language_skill(
        &self,
        language: u32,
        known: Option<&crate::ui_action::PlayerActions>,
    ) -> u32 {
        let (Ok((store, _)), Some(known), Some(spells), Some(lines)) = (
            self.me.single(),
            known,
            self.spells.as_deref(),
            self.skill_lines.as_deref(),
        ) else {
            return 0;
        };
        crate::ui_chat::language_skill(
            store,
            self.my_auras.iter().next(),
            &known.spells,
            spells,
            lines,
            language,
        )
        .unwrap_or(0)
    }

    /// The `$`-macro pass the reader runs on its text (`0x589ff6`), with us as the subject.
    fn parse(&self, text: &str) -> String {
        let subject = self
            .names
            .as_deref()
            .zip(self.commands.as_deref())
            .and_then(|(names, commands)| {
                crate::npc_text::player_identity(&self.me, names, commands)
            });
        let fallback = crate::world_state::WorldStates::default();
        crate::npc_text::substitute(
            text,
            &crate::npc_text::MacroContext {
                subject: subject.as_ref(),
                states: self.states.as_deref().unwrap_or(&fallback),
                achievement: None,
            },
        )
    }

    /// `text` in `language` as a reader at `skill` reads it (`0x505930`).
    fn translate(&self, language: u32, skill: u32, text: &str) -> String {
        match self.languages.as_deref() {
            Some(l) => l.translate(language, skill, text),
            None => text.to_string(),
        }
    }
}

/// A draw's outcome.
enum Drawn {
    /// Not drawn: another object, the object gone, or a record in flight.
    No,
    /// A letter's text, still to pass the profanity mask.
    Letter,
    Page,
}

/// The draw (`0x589e90`): for the held object, its letter or current page into the text, the next page's id noted, the page translated at our skill or at none.
fn draw(
    text: &mut ItemText,
    ctx: &ItemTextCtx,
    known: Option<&crate::ui_action::PlayerActions>,
    guid: u64,
    translate: bool,
) -> Drawn {
    if text.guid != guid || guid == 0 {
        return Drawn::No;
    }
    let Some(object) = ctx.object(guid) else {
        return Drawn::No;
    };
    if text.has_item_text {
        let Some(letter) = ctx.letter(guid) else {
            text.wait(Wait::Letter(guid), guid);
            return Drawn::No;
        };
        text.text = ctx.parse(letter);
        return Drawn::Letter;
    }
    let page_id = text.pages[text.current_page];
    let Some(page) = ctx.page(page_id, guid) else {
        text.wait(Wait::Page(page_id), guid);
        return Drawn::No;
    };
    // The page list doubles when the next entry would run off its end (`0x589fbd`).
    if text.pages.len() == text.current_page + 2 {
        let len = text.pages.len();
        text.pages.resize(2 * len, 0);
    }
    text.pages[text.current_page + 1] = page.next;
    let body = ctx.parse(&page.text);
    if ctx.is_item(guid) {
        if object.item_flags().unwrap_or(0) & ITEM_FIELD_FLAG_TRANSLATED != 0 {
            text.text = body;
        } else {
            let language = ctx.item_record(guid).map_or(0, |t| t.language_id);
            let skill = if translate {
                ctx.language_skill(language, known)
            } else {
                0
            };
            text.text = ctx.translate(language, skill, &body);
        }
    } else if benilla_protocol::guid::is_gameobject(guid) {
        let language = ctx
            .go_templates
            .as_deref()
            .and_then(|g| g.get(guid))
            .and_then(|t| t.text_page)
            .map_or(0, |p| p.language);
        let skill = if translate {
            ctx.language_skill(language, known)
        } else {
            0
        };
        text.text = ctx.translate(language, skill, &body);
    }
    Drawn::Page
}

/// Queue `event` from a handler.
fn fire(world: &mut World, event: &'static str, args: Vec<benilla_ui::script::ScriptValue>) {
    if let Some(mut ui) = world.get_resource_mut::<crate::ui_script::live::UiEvents>() {
        ui.fire(event, args);
    }
}

/// The draw from a handler (`0x589e90`): `ITEM_TEXT_READY` once drawn, a letter masked first (`0x589f4b`).
pub(crate) fn display_text(world: &mut World, guid: u64, translate: bool) {
    let mut state = bevy::ecs::system::SystemState::<(
        Option<ResMut<ItemText>>,
        ItemTextCtx,
        Option<Res<crate::ui_action::PlayerActions>>,
        Option<ResMut<crate::text_filter::TextFilter>>,
        Option<Res<crate::text_filter::TextFilterSwitches>>,
    )>::new(world);
    let (text, ctx, known, filter, switches) = state.get_mut(world);
    let Some(mut text) = text else {
        return;
    };
    let drawn = draw(&mut text, &ctx, known.as_deref(), guid, translate);
    if let (Drawn::Letter, Some(mut filter)) = (&drawn, filter) {
        let enabled = switches.is_none_or(|s| s.profanity);
        filter.mask(enabled, &mut text.text);
    }
    state.apply(world);
    if !matches!(drawn, Drawn::No) {
        fire(world, "ITEM_TEXT_READY", Vec::new());
    }
}

/// The read call (`0x58a1a0`): `guid` 0 closes; the held object again with flag 0 closes it; another object, or the held one from a callback, opens once its page or letter is cached, taking the interact target and firing `ITEM_TEXT_BEGIN`, then draws, or asks the server to read an unread book first.
pub(crate) fn set_item(world: &mut World, guid: u64, again: bool) {
    let Some(held) = world.get_resource::<ItemText>().map(|t| t.guid) else {
        return;
    };
    if !again && guid != 0 && guid == held {
        crate::ui_session::clear_interact_target(world, held);
        fire(world, "ITEM_TEXT_CLOSED", Vec::new());
        return;
    }
    if guid != held {
        crate::ui_session::clear_interact_target(world, held);
        world.resource_mut::<ItemText>().reset(guid);
        if guid == 0 {
            fire(world, "ITEM_TEXT_CLOSED", Vec::new());
            return;
        }
    }
    let mut state = bevy::ecs::system::SystemState::<(ResMut<ItemText>, ItemTextCtx)>::new(world);
    let (mut text, ctx) = state.get_mut(world);
    let Some(object) = ctx.object(guid) else {
        text.guid = 0;
        return;
    };
    let is_item = ctx.is_item(guid);
    let flags = object.item_flags().unwrap_or(0);
    let head = ctx.page_head(guid);
    let page = match head {
        PageHead::Page(p) => p,
        PageHead::ItemRecord(_) => 0,
    };
    text.has_item_text = is_item && flags & ITEM_FIELD_FLAG_LETTER != 0;
    let has_item_text = text.has_item_text;
    let cached = if page == 0 && !has_item_text {
        None
    } else {
        text.pages[0] = page;
        if page != 0 {
            let id = text.pages[text.current_page];
            let hit = ctx.page(id, guid).is_some();
            if !hit {
                text.wait(Wait::Page(id), guid);
            }
            Some(hit)
        } else {
            let hit = ctx.letter(guid).is_some();
            if !hit {
                text.wait(Wait::Letter(guid), guid);
            }
            Some(hit)
        }
    };
    state.apply(world);
    if let PageHead::ItemRecord(entry) = head {
        if let Some(mut waits) = world.get_non_send_resource_mut::<crate::items::RecordWaits>() {
            waits.wait(entry, crate::items::RecordWait::ItemText(guid), false);
        }
    }
    if cached != Some(true) {
        return;
    }
    crate::ui_session::set_interact_target(world, guid);
    fire(world, "ITEM_TEXT_BEGIN", Vec::new());
    // The draw or the read check runs once `ITEM_TEXT_BEGIN` has fired (`0x58a345`).
    crate::ui_script::live::in_turn(world, move |w| {
        if !is_item || has_item_text || flags & ITEM_FIELD_FLAG_TRANSLATED != 0 {
            display_text(w, guid, true);
        } else {
            read_check(w, guid);
        }
    });
}

/// An unread book's read check: `CMSG_READ_ITEM` with its bag and slot (`0x58a3d5` -> `0x6d5d10`); none found sends nothing.
fn read_check(world: &mut World, guid: u64) {
    let mut state = bevy::ecs::system::SystemState::<(
        Option<Objects>,
        Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
        Option<Res<NetCommands>>,
    )>::new(world);
    let (objects, me, commands) = state.get(world);
    let (Some(objects), Ok(me), Some(commands)) = (objects, me.single(), commands) else {
        return;
    };
    let at = crate::ui_items::collect_inventory(
        &me.0,
        &objects,
        crate::ui_items::InventoryScope::DEFAULT,
    )
    .into_iter()
    .find(|&(_, _, g)| g == guid);
    if let Some((bag, slot, _)) = at {
        debug!("item text: read check for {guid:#x} at bag {bag} slot {slot}");
        let _ = commands.0.send(ClientCommand::ReadItem { bag, slot });
    }
}

/// The reader's callback (`0x58a450`): a found record for the held object re-runs the read call with flag 1.
fn callback(world: &mut World, guid: u64, success: bool) {
    let held = world.get_resource::<ItemText>().map(|t| t.guid);
    if success && held == Some(guid) && guid != 0 {
        set_item(world, guid, true);
    }
}

/// The callbacks waiting on `wait`, in ask order.
fn callbacks(world: &mut World, wait: Wait, success: bool) {
    let Some(taken) = world
        .get_resource_mut::<ItemText>()
        .map(|mut t| t.take(wait))
    else {
        return;
    };
    for guid in taken {
        callback(world, guid, success);
    }
}

/// An item record's callback the page-id call left (`0x707180`).
pub(crate) fn item_record_callback(world: &mut World, guid: u64, landed: bool) {
    callback(world, guid, landed);
}

/// A name answered: the creator reads' callbacks on it (`0x58a4e1`).
pub(crate) fn name_landed(world: &mut World, guid: u64) {
    let landed = world
        .get_resource::<NameCache>()
        .is_some_and(|n| n.peek(guid).is_some());
    callbacks(world, Wait::Name(guid), landed);
}

/// A GameObject's record landed: the text type's post-init re-runs the read call for the held object (`0x70c880`).
pub(crate) fn go_template_landed(world: &mut World, entry: u32) {
    let held = world.get_resource::<ItemText>().map_or(0, |t| t.guid);
    if benilla_protocol::guid::is_gameobject(held)
        && benilla_protocol::guid::entry(held) == Some(entry)
    {
        set_item(world, held, true);
    }
}

/// An item leaving the object manager: the held one closes the reader (`0x70b036` -> `0x58a1a0`).
pub(crate) fn item_disabled(world: &mut World, guid: u64) {
    let held = world.get_resource::<ItemText>().map_or(0, |t| t.guid);
    if benilla_protocol::guid::is_item(guid) && held == guid {
        set_item(world, 0, false);
    }
}

/// The interaction close's reader leg (`0x512ecf` -> `0x58a1a0`): the held object on the zeroed target closes.
pub(crate) fn close_by_interaction(world: &mut World, target: u64) -> bool {
    let held = world.get_resource::<ItemText>().map_or(0, |t| t.guid);
    if held == 0 || held != target {
        return false;
    }
    set_item(world, 0, false);
    true
}

/// The reader as the live view lends it.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ItemTextLive<'w, 's> {
    text: Option<ResMut<'w, ItemText>>,
    ctx: ItemTextCtx<'w, 's>,
}

impl ItemTextLive<'_, '_> {
    /// The reader at the call.
    pub(crate) fn read(&self) -> Option<benilla_ui::script::ItemTextRead> {
        let t = self.text.as_deref()?;
        Some(benilla_ui::script::ItemTextRead {
            item: self.ctx.name(t.guid),
            text: t.text.clone(),
            page: t.current_page as u32 + 1,
            has_next: !t.has_item_text && t.pages.get(t.current_page + 1).is_some_and(|&p| p != 0),
            material: self.ctx.material(t.guid),
        })
    }

    /// `ItemTextGetCreator` (`0x58a480`): the held item's creator name, a miss asked with the reader's callback on it.
    pub(crate) fn creator(&mut self) -> Option<String> {
        let t = self.text.as_mut()?;
        let item = t.guid;
        if !self.ctx.is_item(item) {
            return None;
        }
        let creator = self.ctx.object(item)?.item_creator()?;
        let name = self
            .ctx
            .names
            .as_deref()?
            .resolve(creator, self.ctx.commands.as_deref()?)
            .map(str::to_string);
        if name.is_none() {
            t.wait(Wait::Name(creator), item);
        }
        name
    }

    /// A page step and its redraw (`0x58a110`, `0x58a150` -> `0x589e90`): whether it drew.
    pub(crate) fn turn(
        &mut self,
        next: bool,
        known: Option<&crate::ui_action::PlayerActions>,
    ) -> bool {
        let Some(t) = self.text.as_mut() else {
            return false;
        };
        if t.guid == 0 || t.has_item_text {
            return false;
        }
        if next {
            // The next id read signed (`0x58a177`).
            if t.pages
                .get(t.current_page + 1)
                .is_none_or(|&p| p as i32 <= 0)
            {
                return false;
            }
            t.current_page += 1;
        } else {
            if t.current_page == 0 {
                return false;
            }
            t.current_page -= 1;
        }
        let guid = t.guid;
        matches!(draw(t, &self.ctx, known, guid, true), Drawn::Page)
    }

    /// `CloseItemText`'s read call with no object (`0x58a400`): two closes while the reader holds the interact target, whose clear zeroes it (`0x58a207` -> `0x512e60` -> `0x512ecf`), one otherwise, none with no reader.
    pub(crate) fn close(&mut self, target: Option<&mut crate::ui_session::InteractTarget>) -> u32 {
        let Some(t) = self.text.as_mut() else {
            return 0;
        };
        let held = t.guid;
        if held == 0 {
            return 0;
        }
        t.reset(0);
        if target.is_some_and(|target| target.clear_in_call(held)) {
            2
        } else {
            1
        }
    }
}

/// `PageTextMaterial.dbc`; absent without client data, leaving every reader on Parchment.
#[derive(Resource)]
pub(crate) struct PageMaterials(pub(crate) benilla_formats::PageTextMaterialCatalog);

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
        gos.insert(100, 2, 0, "Wanted Poster".into(), &data);
        gos.insert(102, 10, 0, "Chest".into(), &data);
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
