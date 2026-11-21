//! The dressing room: each `DressUpModel` pane's own model ([`DressUpRoom`]), which its verbs edit
//! inside their call through the live view ([`DressUpLive`]) as 12340's do, and the booth's
//! render sync, which composes the shown pane's look ([`DressUpPreview`]): the player's own gear
//! with each substitution in its [`equip_slot`], the held pair through [`held_lanes`]. A try-on
//! whose record misses asks inside the call and lands through its record callback (`0x5980b0`); a
//! recipe stands in for its product and an item with no visual of its own glows with the first
//! enchant that has one. There is no fit check: the reference previews whatever it is handed
//! (`0x5980d0`).

use std::collections::HashMap;

use benilla_protocol::CharEnumItem;
use benilla_ui::script::{DressUpIntent, UiScript};
use bevy::ecs::system::{SystemParam, SystemState};
use bevy::prelude::*;

use crate::entities::equip_slot;
use crate::items::{Items, RecordWait, RecordWaits};
use crate::net::{NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::portrait::{DressUpLook, DressUpPreview};
use crate::ui_script::UiFeed;

/// Every rendered equipment slot, the set [`equip_slot`] can map an item into.
const LOOK_SLOTS: [u8; 14] = [0, 2, 3, 4, 5, 6, 7, 8, 9, 14, 15, 16, 17, 18];

/// Main hand, off hand and ranged: three slots for the widget's two held lanes ([`held_lanes`]).
const HELD_SLOTS: [usize; 3] = [15, 16, 17];

/// The widget's two held lanes. `TryOn` installs into lane `0x0f` (HandRight) or `0x10` (HandLeft,
/// or the Shield point): a bow (15) takes the off lane, a gun, crossbow, wand (26) or thrown (25)
/// the main one. An item the other lane cannot coexist with evicts it, and a ranged weapon coexists
/// with nothing.
///
/// Deviation: the base is the player's melee pair, held drawn, because the booth shares the select
/// screen's assembly; the reference clones the live model with its attachments, drawn or sheathed.
/// A worn ranged weapon, shown only while ranged-drawn, appears once tried on.
fn held_lanes(equipment: &mut [CharEnumItem; 19], room: &DressUpRoom) {
    // The player's own melee pair; substitutions replay below.
    let base = |slot: usize| {
        (room.worn[slot].is_none() && equipment[slot].display_id != 0).then_some(equipment[slot])
    };
    let (mut main, mut off) = (base(15), base(16));

    for &slot in &room.held_order {
        let Some(item) = room.worn[slot] else {
            continue;
        };
        // A bow is the one ranged type that takes the off lane.
        let to_off = slot == 16 || (slot == 17 && item.inventory_type == 15);
        let (mine, other) = if to_off {
            (&mut off, &mut main)
        } else {
            (&mut main, &mut off)
        };
        if let Some(evicted) = *other {
            let (m, o) = if to_off {
                (evicted.inventory_type, item.inventory_type)
            } else {
                (item.inventory_type, evicted.inventory_type)
            };
            if !lanes_coexist(m, o) {
                *other = None;
            }
        }
        *mine = Some(item);
    }

    // Write back by lane, never through `equip_slot`: the index picks the held law `held_wants`
    // asks (15 and 16 melee-drawn, 17 ranged-drawn with its own bow-left split), and an off-hand
    // one-hander is `INVTYPE_WEAPON` 13, which `equip_slot` maps to the main hand.
    for slot in HELD_SLOTS {
        equipment[slot] = CharEnumItem::default();
    }
    for (lane, item) in [(15usize, main), (16, off)] {
        let Some(item) = item else { continue };
        let ranged = matches!(item.inventory_type, 15 | 25 | 26);
        equipment[if ranged { 17 } else { lane }] = item;
    }
}

/// `(main, off, dualWield)`: a one-hand main (13, 21) coexists with a shield or holdable (14, 23),
/// and with a one-hand off (13, 22) under dual-wield; anything else evicts. Dual-wield is taken as
/// allowed here; the reference reads it, whose source is untraced.
fn lanes_coexist(main: u8, off: u8) -> bool {
    matches!(main, 13 | 21) && matches!(off, 13 | 14 | 22 | 23)
}

/// One `DressUpModel` pane's model: its substitutions over the player's own gear.
#[derive(Default)]
pub(crate) struct DressUpRoom {
    /// Set by the first rebuild from the unit; before it the pane has no model, which `Undress`
    /// and `TryOn` leave alone (`0x597ba3`, `0x598112`).
    open: bool,
    /// `Undress()` has taken off the player's own pieces, the hand lanes included, until the next
    /// `Dress` (`0x597bc7`, `0x597bfa`, `0x597c35`).
    bare: bool,
    /// Resolved substitutions by equipment slot.
    worn: [Option<CharEnumItem>; 19],
    /// Held substitutions (slots 15-17) in landing order, latest last: a held try-on can evict the
    /// other lane, so the order decides what survives.
    held_order: Vec<usize>,
}

/// What a try-on reads at the call: the item cache, the wire its ask leaves on, and the catalogs
/// a recipe's product and an enchant's glow resolve through.
pub(crate) struct DressUpCatalogs<'a> {
    pub(crate) items: &'a Items,
    pub(crate) commands: &'a NetCommands,
    pub(crate) spells: Option<&'a crate::ui_action::Spells>,
    pub(crate) enchants: Option<&'a crate::items::Enchants>,
    pub(crate) icons: Option<&'a crate::entities::ItemDisplays>,
}

impl DressUpRoom {
    /// One edit of the pane's model, inside its verb's call; a try-on's record miss leaves `pane`'s
    /// callback on `waits`.
    pub(crate) fn apply(
        &mut self,
        pane: Pane,
        intent: DressUpIntent,
        cat: &DressUpCatalogs<'_>,
        waits: Option<&mut RecordWaits>,
    ) {
        match intent {
            // The rebuild from the unit drops every substitution (`0x5977e0`).
            DressUpIntent::Dress => {
                self.open = true;
                self.bare = false;
                self.worn = Default::default();
                self.held_order.clear();
            }
            // Bodyslots `0..0xc` and both hand lanes off, worn and tried-on alike (`0x597bc0`-`0x597c45`).
            DressUpIntent::Undress => {
                if !self.open {
                    return;
                }
                self.bare = true;
                self.worn = Default::default();
                self.held_order.clear();
            }
            DressUpIntent::TryOn(item, enchants) => self.try_on(pane, item, enchants, cat, waits),
        }
    }

    /// The try-on (`0x597fc0`): the record read with no ask (`0x597fe6`), whose own visual wins
    /// over the enchants' (`0x598045`), then the try-on with that glow.
    fn try_on(
        &mut self,
        pane: Pane,
        item: u32,
        enchants: [i32; 4],
        cat: &DressUpCatalogs<'_>,
        waits: Option<&mut RecordWaits>,
    ) {
        let own = cat
            .items
            .template_cached(item)
            .and_then(|t| cat.icons?.catalog.get(t.display_info_id))
            .is_some_and(|d| d.item_visual != 0);
        let visual = if own {
            0
        } else {
            enchants
                .iter()
                .filter_map(|&id| cat.enchants?.0.visual(u32::try_from(id).ok()?))
                .find(|&v| v != 0)
                .unwrap_or(0)
        };
        self.try_on_with_visual(pane, item, visual, cat, waits);
    }

    /// The try-on with its glow (`0x5980d0`): the record asked with the pane's callback, a miss
    /// sending the ask now and returning (`0x5980fa`); no display or no model is a no-op
    /// (`0x598109`, `0x598112`); a pattern tries on its product (`0x59822e`).
    fn try_on_with_visual(
        &mut self,
        pane: Pane,
        item: u32,
        visual: i32,
        cat: &DressUpCatalogs<'_>,
        waits: Option<&mut RecordWaits>,
    ) {
        let Some(t) = cat.items.ask_template(item, 0, cat.commands) else {
            if let Some(w) = waits {
                w.wait(item, RecordWait::DressUp(pane), false);
            }
            return;
        };
        if t.display_info_id == 0 || !self.open {
            return;
        }
        if t.inventory_type == 0 {
            let product = crate::ui_items::feed::recipe_creates(t, cat.spells);
            if product != 0 {
                self.try_on(pane, product, [0; 4], cat, waits);
            }
            return;
        }
        let (display_id, inventory_type) = (t.display_info_id, t.inventory_type as u8);
        // A non-worn item maps to no slot and previews nothing.
        let Some(slot) = equip_slot(inventory_type) else {
            return;
        };
        self.worn[slot] = Some(CharEnumItem {
            display_id,
            inventory_type,
            enchant_visual: visual.max(0) as u32,
        });
        // Held slots record their order, which eviction reads.
        if HELD_SLOTS.contains(&slot) {
            self.held_order.retain(|&s| s != slot);
            self.held_order.push(slot);
        }
    }
}

/// A `DressUpModel` pane, by its handle's [`benilla_ui::widget::FrameHandle::bits`].
pub(crate) type Pane = u64;

/// Every `DressUpModel` pane's model, by pane: each widget holds its own.
#[derive(Resource, Default)]
pub(crate) struct DressUpRooms(HashMap<Pane, DressUpRoom>);

impl DressUpRooms {
    /// `pane`'s substitution in `slot`, its display id.
    #[cfg(test)]
    pub(crate) fn worn_display(&self, pane: Pane, slot: usize) -> Option<u32> {
        Some(self.0.get(&pane)?.worn[slot]?.display_id)
    }
}

/// The dressing room as its verbs reach it at the call.
#[derive(SystemParam)]
pub(crate) struct DressUpLive<'w> {
    rooms: Option<ResMut<'w, DressUpRooms>>,
    items: Option<Res<'w, Items>>,
    commands: Option<Res<'w, NetCommands>>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    enchants: Option<Res<'w, crate::items::Enchants>>,
    icons: Option<Res<'w, crate::entities::ItemDisplays>>,
}

impl DressUpLive<'_> {
    /// `pane`'s model edited now, a try-on's miss leaving its callback on `waits`.
    pub(crate) fn dress_up(
        &mut self,
        pane: Pane,
        intent: DressUpIntent,
        waits: Option<&mut RecordWaits>,
    ) {
        let (Some(rooms), Some(items), Some(commands)) = (
            self.rooms.as_mut(),
            self.items.as_deref(),
            self.commands.as_deref(),
        ) else {
            return;
        };
        let cat = DressUpCatalogs {
            items,
            commands,
            spells: self.spells.as_deref(),
            enchants: self.enchants.as_deref(),
            icons: self.icons.as_deref(),
        };
        rooms
            .0
            .entry(pane)
            .or_default()
            .apply(pane, intent, &cat, waits);
    }
}

/// The pane's record callback (`0x5980b0`): a found record tries the item on again with no enchants
/// (`0x5980b3`, `0x5980c4`), so an item first seen uncached glows with none.
pub(crate) fn record_callback(world: &mut World, entry: u32, landed: bool, pane: Pane) {
    if !landed {
        return;
    }
    let mut state = SystemState::<(DressUpLive, Option<NonSendMut<RecordWaits>>)>::new(world);
    let (mut live, mut waits) = state.get_mut(world);
    live.dress_up(
        pane,
        DressUpIntent::TryOn(entry, [0; 4]),
        waits.as_deref_mut(),
    );
    state.apply(world);
}

pub(crate) struct DressUpUiPlugin;

impl Plugin for DressUpUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DressUpRooms>()
            .add_systems(Update, sync_dressup_booth.in_set(UiFeed))
            .add_systems(OnExit(crate::char_select::ClientState::InWorld), drop_rooms);
    }
}

/// The panes go with the in-game UI, so a later VM's pane never inherits a model.
fn drop_rooms(mut rooms: ResMut<DressUpRooms>) {
    rooms.0.clear();
}

/// The panes the booth draws, the dressing room's and the auction house's, in the order it picks.
const BOOTH_PANES: [&str; 2] = ["DressUpModel", "AuctionDressUpModel"];

/// The booth's render sync: the shown pane's model and facing, which 12340's model frame draws on
/// every frame render (`0x960240`); it reads the pane's model and writes no game state.
fn sync_dressup_booth(
    script: Option<NonSend<UiScript>>,
    rooms: Option<Res<DressUpRooms>>,
    mut preview: ResMut<DressUpPreview>,
    items: Res<Items>,
    self_q: Query<(&ObjectStore, &NetEntity), With<SelfPlayer>>,
    guilds: Res<crate::ui_guild::GuildCache>,
) {
    let Some(script) = script else {
        return;
    };
    let shown = BOOTH_PANES
        .iter()
        .find_map(|name| script.shown_model_pane(name));
    if let Some((_, yaw)) = shown {
        // The stock rotate buttons set the yaw with `SetRotation`; the booth mirrors it.
        preview.yaw = yaw;
    }
    // A hidden pane draws nothing; its model stays as the verbs left it.
    let look = shown.and_then(|(pane, _)| {
        let room = rooms.as_ref()?.0.get(&pane.bits()).filter(|r| r.open)?;
        let (store, net) = self_q.single().ok()?;
        // The crest comes off the guild cache, read with no ask, not the outfit `player_look` builds.
        player_look(store, net, room, &items).map(|l| DressUpLook {
            emblem: guilds.cached_emblem(store.0.player_guild_id()),
            ..l
        })
    });
    if preview.look != look {
        preview.look = look;
    }
}

/// The player's own look with the room's substitutions written in.
fn player_look(
    store: &ObjectStore,
    net: &NetEntity,
    room: &DressUpRoom,
    items: &Items,
) -> Option<DressUpLook> {
    let s = &store.0;
    let mut equipment = [CharEnumItem::default(); 19];
    for slot in LOOK_SLOTS {
        let idx = slot as usize;
        // A substitution wins over the player's own piece.
        if let Some(worn) = room.worn[idx] {
            equipment[idx] = worn;
            continue;
        }
        // Undressed: the player's own pieces are off, the hand lanes too (`0x597bfa`, `0x597c35`).
        if room.bare {
            continue;
        }
        // Hidden in the world, hidden here: `SetUnit` clones the live player's bodyslot display
        // pointers verbatim. A try-on, handled above, gates on nothing.
        if (idx == 0 && s.player_hides_helm()) || (idx == 14 && s.player_hides_cloak()) {
            continue;
        }
        let Some(entry) = s.player_visible_item_entry(slot).filter(|e| *e != 0) else {
            continue;
        };
        // The visible-item field carries an item entry; the display id is on its template, read
        // with no ask: the pane clones the live model's own (`0x597e10`).
        if let Some(t) = items.template_cached(entry) {
            equipment[idx] = CharEnumItem {
                display_id: t.display_info_id,
                inventory_type: t.inventory_type as u8,
                enchant_visual: 0,
            };
        }
    }
    held_lanes(&mut equipment, room);
    Some(DressUpLook {
        display_id: net.display_id?,
        race: s.unit_race()?,
        sex: s.unit_gender()?,
        class: s.unit_class().unwrap_or(0),
        skin: s.player_skin().unwrap_or(0),
        face: s.player_face().unwrap_or(0),
        hair_style: s.player_hair_style().unwrap_or(0),
        hair_color: s.player_hair_color().unwrap_or(0),
        facial_hair: s.player_facial_hair().unwrap_or(0),
        equipment,
        // `sync_dressup_booth` stamps the crest in.
        emblem: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use benilla_protocol::{EntityKind, ItemInfo, ObjectFields};

    use crate::items::test_template;
    use crate::net::ClientCommand;

    /// A human male self-player wearing `entries` (slot, item entry).
    fn player(entries: &[(u8, u32)]) -> ObjectStore {
        use benilla_protocol::field::{FIELD_PLAYER_BYTES, FIELD_UNIT_BYTES_0};
        let mut pairs = vec![
            (FIELD_UNIT_BYTES_0, 1 | 1 << 8), // race 1 (human), class 1, gender 0 (male)
            (FIELD_PLAYER_BYTES, 3 | 4 << 8), // skin 3, face 4, hair 0, hair colour 0
        ];
        for (slot, entry) in entries {
            pairs.push((
                benilla_protocol::field::player_visible_item_entry_field(*slot),
                *entry,
            ));
        }
        ObjectStore(ObjectFields::from_pairs(&pairs))
    }

    fn net() -> NetEntity {
        NetEntity {
            kind: EntityKind::Player,
            display_id: Some(49),
            scale: 1.0,
        }
    }

    fn commands() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (NetCommands(tx), rx)
    }

    const PANE: Pane = 7;

    /// One edit of [`PANE`]'s model with no catalogs and no callback list.
    fn edit(room: &mut DressUpRoom, items: &Items, cmds: &NetCommands, intent: DressUpIntent) {
        let cat = DressUpCatalogs {
            items,
            commands: cmds,
            spells: None,
            enchants: None,
            icons: None,
        };
        room.apply(PANE, intent, &cat, None);
    }

    fn worn(name: &str, display_info_id: u32, inventory_type: u32) -> ItemInfo {
        ItemInfo {
            display_info_id,
            inventory_type,
            ..test_template(name)
        }
    }

    #[test]
    fn a_try_on_substitutes_only_its_own_slot() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        // Worn: a chest (slot 4) and a sword in the main hand (slot 15).
        items.insert_template(1000, Some(worn("Worn Chest", 5000, 5)));
        items.insert_template(1500, Some(worn("Worn Sword", 5500, 21)));
        // Tried on: a different chest.
        items.insert_template(2000, Some(worn("Shiny Chest", 7000, 5)));
        let store = player(&[(4, 1000), (15, 1500)]);

        let mut room = DressUpRoom::default();
        edit(&mut room, &items, &cmds, DressUpIntent::Dress);
        edit(&mut room, &items, &cmds, DressUpIntent::TryOn(2000, [0; 4]));

        let look = player_look(&store, &net(), &room, &items).expect("a look");
        assert_eq!(
            look.equipment[4].display_id, 7000,
            "the tried-on chest shows"
        );
        assert_eq!(
            look.equipment[15].display_id, 5500,
            "the sword the player is actually holding is untouched"
        );
        assert_eq!(look.race, 1);
        assert_eq!(look.sex, 0);
        assert_eq!((look.skin, look.face), (3, 4));
        assert_eq!(look.display_id, 49, "the player's own body");
    }

    /// Reset is `DressUpModel:Dress()`; a pane with no model yet takes no try-on (`0x598112`).
    #[test]
    fn reset_drops_substitutions_and_a_pane_with_no_model_takes_nothing() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1000, Some(worn("Worn Chest", 5000, 5)));
        items.insert_template(2000, Some(worn("Shiny Chest", 7000, 5)));
        let store = player(&[(4, 1000)]);

        let mut room = DressUpRoom::default();
        edit(&mut room, &items, &cmds, DressUpIntent::TryOn(2000, [0; 4]));
        edit(&mut room, &items, &cmds, DressUpIntent::Undress);
        assert!(
            !room.open && !room.bare && room.worn.iter().all(Option::is_none),
            "no model: the try-on and the undress are no-ops"
        );

        edit(&mut room, &items, &cmds, DressUpIntent::Dress);
        edit(&mut room, &items, &cmds, DressUpIntent::TryOn(2000, [0; 4]));
        assert_eq!(
            player_look(&store, &net(), &room, &items)
                .unwrap()
                .equipment[4]
                .display_id,
            7000
        );

        edit(&mut room, &items, &cmds, DressUpIntent::Dress);
        assert_eq!(
            player_look(&store, &net(), &room, &items)
                .unwrap()
                .equipment[4]
                .display_id,
            5000,
            "Reset puts the player's own chest back on"
        );
    }

    #[test]
    fn undress_strips_the_body_and_the_hands_and_a_try_on_lands_on_the_bare_body() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1000, Some(worn("Worn Chest", 5000, 5)));
        items.insert_template(2000, Some(worn("Shiny Chest", 7000, 5)));
        items.insert_template(3000, Some(worn("Sword", 6000, 13)));
        items.insert_template(4000, Some(worn("Worn Helm", 8000, 1)));
        let store = player(&[(4, 1000), (15, 3000), (0, 4000)]);

        let mut room = DressUpRoom::default();
        edit(&mut room, &items, &cmds, DressUpIntent::Dress);
        edit(&mut room, &items, &cmds, DressUpIntent::TryOn(2000, [0; 4]));
        edit(&mut room, &items, &cmds, DressUpIntent::Undress);
        let look = player_look(&store, &net(), &room, &items).unwrap();
        assert_eq!(look.equipment[4].display_id, 0, "the tried-on chest is off");
        assert_eq!(look.equipment[0].display_id, 0, "the worn helm is off");
        assert_eq!(
            look.equipment[15].display_id, 0,
            "the sword leaves the hand with the lanes (`0x597bfa`)"
        );

        edit(&mut room, &items, &cmds, DressUpIntent::TryOn(2000, [0; 4]));
        let look = player_look(&store, &net(), &room, &items).unwrap();
        assert_eq!(
            look.equipment[4].display_id, 7000,
            "a try-on lands on the bare body"
        );
        assert_eq!(
            look.equipment[0].display_id, 0,
            "…which stays bare elsewhere"
        );

        edit(&mut room, &items, &cmds, DressUpIntent::Dress);
        let look = player_look(&store, &net(), &room, &items).unwrap();
        assert_eq!(
            look.equipment[4].display_id, 5000,
            "Dress puts the player's own chest back"
        );
        assert_eq!(look.equipment[0].display_id, 8000);
    }

    /// A chat-linked item usually misses the cache on the first click: the ask leaves inside the
    /// call with the pane's callback (`0x5980fa`), a `Dress` in between cancels nothing, and the
    /// callback's try-on carries no enchants (`0x5980c4`).
    #[test]
    fn an_unknown_item_asks_in_the_call_and_lands_by_its_callback() {
        let (cmds, rx) = commands();
        let mut items = Items::default();
        items.insert_template(1000, Some(worn("Worn Chest", 5000, 5)));
        let store = player(&[(4, 1000)]);
        fn cat<'a>(items: &'a Items, commands: &'a NetCommands) -> DressUpCatalogs<'a> {
            DressUpCatalogs {
                items,
                commands,
                spells: None,
                enchants: None,
                icons: None,
            }
        }

        let mut room = DressUpRoom::default();
        let mut waits = RecordWaits::default();
        edit(&mut room, &items, &cmds, DressUpIntent::Dress);
        room.apply(
            PANE,
            DressUpIntent::TryOn(2000, [5, 0, 0, 0]),
            &cat(&items, &cmds),
            Some(&mut waits),
        );
        let asks = rx
            .try_iter()
            .filter(|c| matches!(c, ClientCommand::ItemQuery { entry: 2000, .. }))
            .count();
        assert_eq!(asks, 1, "the ask left inside the call");
        assert_eq!(waits.waiting(), &[(2000, RecordWait::DressUp(PANE))]);
        assert_eq!(
            player_look(&store, &net(), &room, &items)
                .unwrap()
                .equipment[4]
                .display_id,
            5000,
            "until it lands, the player's own gear is what shows"
        );
        edit(&mut room, &items, &cmds, DressUpIntent::Dress);
        assert_eq!(waits.waiting().len(), 1, "a rebuild cancels no callback");

        items.insert_template(2000, Some(worn("Shiny Chest", 7000, 5)));
        for wait in waits.take(2000) {
            assert_eq!(wait, RecordWait::DressUp(PANE));
            room.apply(
                PANE,
                DressUpIntent::TryOn(2000, [0; 4]),
                &cat(&items, &cmds),
                Some(&mut waits),
            );
        }
        assert_eq!(
            player_look(&store, &net(), &room, &items)
                .unwrap()
                .equipment[4],
            CharEnumItem {
                display_id: 7000,
                inventory_type: 5,
                enchant_visual: 0,
            },
            "the answer landing is what makes it show"
        );
        assert!(waits.waiting().is_empty());
    }

    #[test]
    fn a_non_worn_item_previews_nothing_and_does_not_linger() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(3000, Some(worn("Healing Potion", 9000, 0)));
        let mut room = DressUpRoom::default();
        edit(&mut room, &items, &cmds, DressUpIntent::Dress);
        edit(&mut room, &items, &cmds, DressUpIntent::TryOn(3000, [0; 4]));
        assert!(
            room.worn.iter().all(Option::is_none),
            "resolved, just not worn anywhere"
        );
    }

    /// A bow takes the off lane and clears the main; a crossbow takes the main and clears the off.
    #[test]
    fn a_tried_on_ranged_weapon_takes_a_hand_and_clears_the_melee_pair() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1500, Some(worn("Worn Sword", 5500, 21))); // WEAPONMAINHAND
        items.insert_template(1600, Some(worn("Worn Shield", 5600, 14))); // SHIELD
        items.insert_template(2500, Some(worn("Short Bow", 8500, 15))); // RANGED
        items.insert_template(2600, Some(worn("Crossbow", 8600, 26))); // RANGEDRIGHT
        let store = player(&[(15, 1500), (16, 1600)]);

        for (item, display, what) in [(2500u32, 8500u32, "bow"), (2600, 8600, "crossbow")] {
            let mut room = DressUpRoom::default();
            edit(&mut room, &items, &cmds, DressUpIntent::Dress);
            edit(&mut room, &items, &cmds, DressUpIntent::TryOn(item, [0; 4]));

            let look = player_look(&store, &net(), &room, &items).expect("a look");
            assert_eq!(
                look.equipment[17].display_id, display,
                "the {what} is in the ranged slot, which is what puts it in a hand"
            );
            assert_eq!(
                (look.equipment[15].display_id, look.equipment[16].display_id),
                (0, 0),
                "…and a ranged weapon coexists with neither melee lane"
            );
        }
    }

    /// The booth is melee-drawn ([`held_lanes`]); a ranged weapon shows only while ranged-drawn.
    #[test]
    fn a_worn_ranged_weapon_is_not_shown_until_it_is_tried_on() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1500, Some(worn("Worn Sword", 5500, 21)));
        items.insert_template(1700, Some(worn("Worn Bow", 5700, 15)));
        let store = player(&[(15, 1500), (17, 1700)]);

        let mut room = DressUpRoom::default();
        edit(&mut room, &items, &cmds, DressUpIntent::Dress);
        let look = player_look(&store, &net(), &room, &items).expect("a look");
        assert_eq!(
            look.equipment[17].display_id, 0,
            "the worn bow stays stowed"
        );
        assert_eq!(look.equipment[15].display_id, 5500, "the sword is in hand");
    }

    /// Eviction follows try-on order: a two-hander after a bow evicts the bow.
    #[test]
    fn a_later_melee_try_on_evicts_the_ranged_one() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(2500, Some(worn("Short Bow", 8500, 15)));
        items.insert_template(2700, Some(worn("Great Axe", 8700, 17))); // TWOHAND
        let store = player(&[]);

        let mut room = DressUpRoom::default();
        edit(&mut room, &items, &cmds, DressUpIntent::Dress);
        edit(&mut room, &items, &cmds, DressUpIntent::TryOn(2500, [0; 4]));
        edit(&mut room, &items, &cmds, DressUpIntent::TryOn(2700, [0; 4]));

        let look = player_look(&store, &net(), &room, &items).expect("a look");
        assert_eq!(look.equipment[15].display_id, 8700, "the axe is in hand");
        assert_eq!(look.equipment[17].display_id, 0, "…and the bow is gone");
    }

    #[test]
    fn a_shield_coexists_with_a_one_hander_but_not_with_a_two_hander() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1500, Some(worn("Worn Sword", 5500, 21)));
        items.insert_template(1550, Some(worn("Worn Axe", 5550, 17))); // TWOHAND
        items.insert_template(1600, Some(worn("Bright Shield", 5600, 14)));

        let mut room = DressUpRoom::default();
        edit(&mut room, &items, &cmds, DressUpIntent::Dress);
        edit(&mut room, &items, &cmds, DressUpIntent::TryOn(1600, [0; 4]));

        let look = player_look(&player(&[(15, 1500)]), &net(), &room, &items).unwrap();
        assert_eq!(look.equipment[15].display_id, 5500, "the one-hander stays");
        assert_eq!(look.equipment[16].display_id, 5600, "…beside the shield");

        let look = player_look(&player(&[(15, 1550)]), &net(), &room, &items).unwrap();
        assert_eq!(
            look.equipment[15].display_id, 0,
            "the two-hander is evicted"
        );
        assert_eq!(look.equipment[16].display_id, 5600);
    }

    /// An off-hand one-hander is `INVTYPE_WEAPON` 13, which [`equip_slot`] maps to the main hand;
    /// each lane keeps its own item beside an unrelated try-on, for every off-lane type.
    #[test]
    fn a_dual_wielded_pair_keeps_both_hands() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1500, Some(worn("Main Sword", 5500, 13))); // INVTYPE_WEAPON
        items.insert_template(2000, Some(worn("Shiny Boots", 7000, 8))); // FEET, the try-on

        for (entry, display, inv, what) in [
            (1600u32, 5600u32, 13u32, "a second one-hander"),
            (1601, 5601, 22, "an off-hand-only weapon"),
            (1602, 5602, 23, "a held-in-off-hand"),
            (1603, 5603, 14, "a shield"),
        ] {
            items.insert_template(entry, Some(worn("Off Hand", display, inv)));
            let store = player(&[(15, 1500), (16, entry)]);

            let mut room = DressUpRoom::default();
            edit(&mut room, &items, &cmds, DressUpIntent::Dress);
            edit(&mut room, &items, &cmds, DressUpIntent::TryOn(2000, [0; 4]));

            let look = player_look(&store, &net(), &room, &items).expect("a look");
            assert_eq!(look.equipment[7].display_id, 7000, "the boots went on");
            assert_eq!(
                (look.equipment[15].display_id, look.equipment[16].display_id),
                (5500, display),
                "the main hand keeps its sword beside {what}"
            );
        }
    }

    /// The room inherits hide-helm and hide-cloak through the clone (`SetUnit`, copying display
    /// pointers); a try-on overrides them.
    #[test]
    fn a_hidden_helm_stays_hidden_on_the_mannequin_until_one_is_tried_on() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1000, Some(worn("Worn Helm", 5000, 1)));
        items.insert_template(1100, Some(worn("Worn Cloak", 5100, 16)));
        items.insert_template(1200, Some(worn("Worn Chest", 5200, 5)));
        items.insert_template(2000, Some(worn("Shiny Helm", 7000, 1)));
        // `PLAYER_FLAGS` carrying HIDE_HELM 0x400 | HIDE_CLOAK 0x800.
        let mut store = player(&[(0, 1000), (14, 1100), (4, 1200)]);
        store.0.merge(ObjectFields::from_pairs(&[(
            benilla_protocol::field::FIELD_PLAYER_FLAGS,
            0x400 | 0x800,
        )]));

        let mut room = DressUpRoom::default();
        edit(&mut room, &items, &cmds, DressUpIntent::Dress);
        let look = player_look(&store, &net(), &room, &items).expect("a look");
        assert_eq!(
            look.equipment[0].display_id, 0,
            "the player's own helm is hidden"
        );
        assert_eq!(look.equipment[14].display_id, 0, "and so is their cloak");
        assert_eq!(
            look.equipment[4].display_id, 5200,
            "everything else they are wearing is untouched"
        );

        // The reference's `TryOn` path never tests the flag.
        edit(&mut room, &items, &cmds, DressUpIntent::TryOn(2000, [0; 4]));
        let look = player_look(&store, &net(), &room, &items).expect("a look");
        assert_eq!(
            look.equipment[0].display_id, 7000,
            "the tried-on helm previews regardless of the preference"
        );

        // Reset drops the substitution: bare-headed again.
        edit(&mut room, &items, &cmds, DressUpIntent::Dress);
        let look = player_look(&store, &net(), &room, &items).expect("a look");
        assert_eq!(
            look.equipment[0].display_id, 0,
            "Reset re-clones the hidden helm away"
        );
    }
}
