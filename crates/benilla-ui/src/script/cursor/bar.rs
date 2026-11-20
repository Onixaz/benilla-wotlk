//! The action-bar cursor verbs. The bar is client-authoritative: placing onto an occupied slot hops
//! the displaced action onto the cursor (`PlaceAction`). Each write goes to the app's bar inside
//! the call, which stores and sends it (one `CMSG_SET_ACTION_BUTTON`, so a drag-swap is two sends),
//! and the verb signals `ACTIONBAR_SLOT_CHANGED` for each slot it wrote before it returns.

use mlua::Lua;

use crate::script::action::{ACTION_KIND_ITEM, ACTION_KIND_MACRO, ACTION_KIND_SPELL};
use crate::script::action_slots::{self, Word};
use crate::script::ui_errors::Line;

use super::{clear_cursor, set_cursor, CursorAction, CursorPayload};

/// The passive-spell refusal's error, errorId `0x9e`: entry of the table (stride `0x14`) names this
/// key.
const PASSIVE_ON_BAR_ERROR: &str = "ERR_PASSIVE_ABILITY";

/// The temporary bar's Lua actions, which a pickup leaves alone (`0x5abe9d`).
const TEMP_BAR: std::ops::RangeInclusive<u32> = 121..=132;

/// `PickupAction(id)` (`0x5abe70`), the slot's shift-click and drag start
/// (`ActionBarFrame.xml:12-38`). With a spell, an item, a macro or a set held it places instead
/// (`0x5abeae`-`0x5abeec`); else the slot's action goes onto the cursor through its setter, which
/// clears whatever else was held, and then the slot is removed, sent and signalled (`0x5abf44`),
/// which a locked bar refuses while the cursor keeps the action. The temporary bar's slots are
/// refused. Returns whether to repaint; each slot written is pushed to `changed`; a place's
/// refusal is handed back as [`place_action`]'s.
pub(super) fn pickup_action(
    model: &mut crate::script::ModelGuard<'_>,
    id: u32,
    changed: &mut Vec<u32>,
) -> Result<bool, Line> {
    if TEMP_BAR.contains(&id) {
        return Ok(false);
    }
    if places(&model.cursor) {
        return place_action(model, id, changed);
    }
    let Some(slot) = action_slots::word(model, id) else {
        return Ok(false);
    };
    let payload = CursorAction {
        src_slot: id,
        kind: slot.kind,
        action: slot.action,
        texture: action_slots::texture(model, id, &|_| None),
    };
    set_cursor(model, CursorPayload::Action(payload));
    if action_slots::write(model, id, None, None) {
        changed.push(id);
    }
    Ok(true)
}

/// Whether the cursor holds what the bar takes: a spell, an item, a macro, a bar item or a set.
fn places(cursor: &Option<CursorPayload>) -> bool {
    matches!(
        cursor,
        Some(
            CursorPayload::Spell(_)
                | CursorPayload::Item(_)
                | CursorPayload::Macro(_)
                | CursorPayload::EquipmentSet(_)
                | CursorPayload::Action(_)
        )
    )
}

/// `PlaceAction(id)` (`0x5ab120`), the slot's `OnReceiveDrag` and `UseAction(id, 1)`'s place fork.
/// The slot's own action dropped on it just clears the cursor (`0x5ab32c`, `0x5ab3bd`). Else the
/// cursor changes first: an occupied slot's action goes up through its setter with `id` as its
/// source, an empty slot clears it; then the slot is written, sent and signalled (`0x5ab7da`),
/// which a locked bar refuses with the cursor already changed. An item stays in its bag, since a
/// bar item is a reference, not a move. Returns whether to repaint; each slot written is pushed to
/// `changed`. A refusal with a line is handed back for the caller to show at once; nothing has
/// changed by then.
pub(crate) fn place_action(
    model: &mut crate::script::ModelGuard<'_>,
    id: u32,
    changed: &mut Vec<u32>,
) -> Result<bool, Line> {
    // The two accept filters refuse with a bare return: nothing stored or sent, and the payload
    // stays held. An item needs an on-use spell or an equip slot, so a bag is placeable; a passive
    // spell (`Attributes & 0x40`) also raises errorId `0x9e` through the error display, where the
    // item refusal is mute. A totem slot takes only a spell of its element, else refuses and keeps
    // it held (`0x5ab24b`).
    if model.cursor.is_some() {
        let spell = match &model.cursor {
            Some(CursorPayload::Spell(s)) => Some(s.spell_id),
            Some(CursorPayload::Action(a)) if a.kind == ACTION_KIND_SPELL => Some(a.action),
            _ => None,
        };
        let fits = spell.and_then(|s| crate::script::multicast::totem_fits(id, s));
        let on_totem = crate::script::multicast::action_category(id).is_some();
        if on_totem && fits != Some(true) {
            return match crate::script::multicast::totem_refusal(id) {
                Some(line) => Err(line),
                None => Ok(false),
            };
        }
    }
    match &model.cursor {
        Some(CursorPayload::Item(i)) if !i.bar_placeable => return Ok(false),
        Some(CursorPayload::Spell(s)) if s.passive => {
            return Err((PASSIVE_ON_BAR_ERROR, None));
        }
        _ => {}
    }
    let placeable = match &model.cursor {
        None => return Ok(false),
        Some(CursorPayload::Action(a)) => Some((a.kind, a.action, a.texture.clone())),
        Some(CursorPayload::Item(i)) => Some((ACTION_KIND_ITEM, i.item_id, i.texture.clone())),
        Some(CursorPayload::Spell(s)) => Some((ACTION_KIND_SPELL, s.spell_id, s.texture.clone())),
        // `PlaceAction` accepts modes 1, 3, 7 and 8: a macro, mode 8, packs its id under the macro
        // tag. Pet actions (4), coins (2), stabled pets (10) and vendor rows (5) are refused and
        // stay held.
        Some(CursorPayload::Macro(m)) => Some((ACTION_KIND_MACRO, m.index, m.texture.clone())),
        // Mode 13, a set, packs its id under the set tag.
        Some(CursorPayload::EquipmentSet(e)) => Some((
            crate::script::action::ACTION_KIND_EQUIPMENT_SET,
            e.id,
            e.texture.clone(),
        )),
        Some(
            CursorPayload::PetAction(_)
            | CursorPayload::StablePet(_)
            | CursorPayload::Money(_)
            | CursorPayload::Merchant(_)
            // Modes 11 and 12, guild bank items and money, are refused too.
            | CursorPayload::GuildBankItem(_)
            | CursorPayload::GuildBankMoney(_),
        ) => None,
    };
    let Some((kind, action, texture)) = placeable else {
        return Ok(false);
    };
    let displaced =
        action_slots::word(model, id).map(|d| (d, action_slots::texture(model, id, &|_| None)));
    match displaced {
        Some((d, _)) if d.kind == kind && d.action == action => {
            clear_cursor(model);
            return Ok(true);
        }
        Some((d, texture)) => set_cursor(
            model,
            CursorPayload::Action(CursorAction {
                src_slot: id,
                kind: d.kind,
                action: d.action,
                texture,
            }),
        ),
        None => clear_cursor(model),
    }
    if action_slots::write(model, id, Some(Word { kind, action }), texture) {
        changed.push(id);
    }
    Ok(true)
}

/// `UseAction`'s place fork (`0x5abbe1`-`0x5abc2c`): a press outside a key binding, on a slot
/// outside 121-132, with a spell, an item, a macro or a picked-up action held places it there
/// instead of using the slot, refused or not. Whether the fork took the press, with the place's
/// refusal if it had one.
pub(crate) fn use_action_places(
    model: &mut crate::script::ModelGuard<'_>,
    id: u32,
    in_binding: bool,
    changed: &mut Vec<u32>,
) -> Option<Option<Line>> {
    if in_binding || TEMP_BAR.contains(&id) || !places(&model.cursor) {
        return None;
    }
    Some(place_action(model, id, changed).err())
}

/// Register `PickupAction` and `PlaceAction` as globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "PickupAction",
        lua.create_function(|lua, id: u32| {
            let mut changed = Vec::new();
            let placed = pickup_action(&mut crate::script::model_mut_of(lua), id, &mut changed);
            let repaint = match placed {
                Ok(repaint) => repaint,
                Err(line) => {
                    crate::script::ui_errors::show_line_now(lua, line);
                    false
                }
            };
            action_slots::signal_all(lua, changed);
            Ok(repaint)
        })?,
    )?;
    g.set(
        "PlaceAction",
        lua.create_function(|lua, id: u32| {
            let mut changed = Vec::new();
            let placed = place_action(&mut crate::script::model_mut_of(lua), id, &mut changed);
            let repaint = match placed {
                Ok(repaint) => repaint,
                Err(line) => {
                    crate::script::ui_errors::show_line_now(lua, line);
                    false
                }
            };
            action_slots::signal_all(lua, changed);
            Ok(repaint)
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::cursor::{CursorAction, CursorItem, CursorPayload, CursorSpell};
    use crate::script::{ActionSlot, UiScript};

    fn action_slot(texture: &str, kind: u8, action: u32) -> ActionSlot {
        ActionSlot {
            texture: Some(texture.into()),
            kind,
            action,
            count: 0,
            consumable: false,
            stackable: false,
            pet_spell: None,
        }
    }

    #[test]
    fn pickup_action_empties_the_slot_and_queues_a_clear() {
        let mut s = UiScript::new().unwrap();
        s.set_action(1, Some(action_slot("Interface\\Icons\\Spell_A", 0x00, 133)));

        assert!(s.eval::<bool>("return PickupAction(1)").unwrap());
        assert!(
            !s.eval::<bool>("return HasAction(1)").unwrap(),
            "removed from the engine's optimistic mirror"
        );
        assert!(
            matches!(s.cursor_payload(), Some(CursorPayload::Action(c)) if c.src_slot == 1),
            "{:?}",
            s.cursor_payload()
        );
        assert_eq!(s.take_action_sets(), vec![(1, 0)]);
    }

    #[test]
    fn pickup_action_on_an_empty_slot_is_a_no_op() {
        let mut s = UiScript::new().unwrap();
        assert!(!s.eval::<bool>("return PickupAction(5)").unwrap());
        assert!(s.cursor_payload().is_none());
        assert!(s.take_action_sets().is_empty());
    }

    #[test]
    fn place_action_onto_empty_writes_the_slot_and_clears_cursor() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 3,
            kind: 0x00,
            action: 133,
            texture: Some("Interface\\Icons\\Spell_A".into()),
        }));

        assert!(s.eval::<bool>("return PlaceAction(7)").unwrap());
        assert!(s.cursor_payload().is_none(), "empty destination clears");
        assert!(s.eval::<bool>("return HasAction(7)").unwrap());
        assert_eq!(
            s.eval::<String>("return GetActionTexture(7)").unwrap(),
            "Interface\\Icons\\Spell_A"
        );
        // 0x00<<24 | 133 == 133.
        assert_eq!(s.take_action_sets(), vec![(7, 133)]);
    }

    #[test]
    fn place_action_onto_occupied_hops_the_displaced_action() {
        let mut s = UiScript::new().unwrap();
        s.set_action(1, Some(action_slot("Interface\\Icons\\Spell_A", 0x00, 111)));
        s.set_action(2, Some(action_slot("Interface\\Icons\\Spell_B", 0x00, 222)));

        assert!(s.eval::<bool>("return PickupAction(1)").unwrap());
        assert_eq!(s.take_action_sets(), vec![(1, 0)]);

        assert!(s.eval::<bool>("return PlaceAction(2)").unwrap());
        assert_eq!(
            s.eval::<String>("return GetActionTexture(2)").unwrap(),
            "Interface\\Icons\\Spell_A"
        );
        assert_eq!(
            s.cursor_payload(),
            Some(CursorPayload::Action(CursorAction {
                src_slot: 2,
                kind: 0x00,
                action: 222,
                texture: Some("Interface\\Icons\\Spell_B".into()),
            }))
        );
        assert_eq!(s.take_action_sets(), vec![(2, 111)]);
    }

    #[test]
    fn place_action_item_payload_packs_the_item_kind_and_leaves_the_bag_untouched() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Item(CursorItem {
            bar_placeable: true,
            bag: 0,
            slot: 1,
            item_id: 117,
            texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
            link: None,
            count: None,
            quality: Some(1),
            equip_slots: Vec::new(),
        }));

        assert!(s.eval::<bool>("return PlaceAction(4)").unwrap());
        assert!(s.cursor_payload().is_none());
        // 0x80<<24 | 117.
        assert_eq!(s.take_action_sets(), vec![(4, 0x8000_0000 | 117)]);
        assert!(
            s.take_container_moves().is_empty(),
            "a bar item action is a reference, not a move"
        );
    }

    #[test]
    fn place_action_spell_payload_packs_the_spell_kind() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            passive: false,
            book_slot: 3,
            book_type: "spell".into(),
            spell_id: 133,
            texture: Some("Interface\\Icons\\Spell_A".into()),
        }));

        assert!(s.eval::<bool>("return PlaceAction(9)").unwrap());
        assert_eq!(s.take_action_sets(), vec![(9, 133)]); // 0x00<<24 | 133
    }

    #[test]
    fn place_action_with_an_empty_cursor_is_a_no_op() {
        let mut s = UiScript::new().unwrap();
        s.set_action(1, Some(action_slot("Interface\\Icons\\Spell_A", 0x00, 111)));
        assert!(!s.eval::<bool>("return PlaceAction(1)").unwrap());
        assert!(s.eval::<bool>("return HasAction(1)").unwrap(), "untouched");
        assert!(s.take_action_sets().is_empty());
    }

    #[test]
    fn pickup_action_with_a_payload_held_falls_through_to_place() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 1,
            kind: 0x00,
            action: 111,
            texture: Some("Interface\\Icons\\Spell_A".into()),
        }));
        assert!(s.eval::<bool>("return PickupAction(5)").unwrap());
        assert!(s.cursor_payload().is_none(), "placed onto the empty slot 5");
        assert!(s.eval::<bool>("return HasAction(5)").unwrap());
        assert_eq!(s.take_action_sets(), vec![(5, 111)]);
    }

    /// The grid shows from the spell setter and hides from the clear, a hop's setter clearing first: hide, then show (`0x519447`-`0x51945f`, `0x520a38`).
    #[test]
    fn showgrid_hidegrid_fire_from_the_setter_and_the_clear() {
        let mut s = UiScript::new().unwrap();
        s.set_action(1, Some(action_slot("Interface\\Icons\\Spell_A", 0x00, 111)));
        s.set_action(2, Some(action_slot("Interface\\Icons\\Spell_B", 0x00, 222)));
        s.run(
            r#"
            shows, hides = 0, 0
            local f = CreateFrame("Frame", "GridListener")
            f:RegisterEvent("ACTIONBAR_SHOWGRID")
            f:RegisterEvent("ACTIONBAR_HIDEGRID")
            f:SetScript("OnEvent", function()
                if event == "ACTIONBAR_SHOWGRID" then shows = shows + 1 end
                if event == "ACTIONBAR_HIDEGRID" then hides = hides + 1 end
            end)
            "#,
        )
        .unwrap();

        s.run("PickupAction(1)").unwrap(); // None -> Some: SHOW
        assert_eq!(s.eval::<i64>("return shows").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return hides").unwrap(), 0);

        s.run("PlaceAction(2)").unwrap(); // a hop: the setter's clear hides, the setter shows
        assert_eq!(s.eval::<i64>("return shows").unwrap(), 2);
        assert_eq!(s.eval::<i64>("return hides").unwrap(), 1);

        s.run("ClearCursor()").unwrap(); // Some -> None: HIDE
        assert_eq!(s.eval::<i64>("return shows").unwrap(), 2);
        assert_eq!(s.eval::<i64>("return hides").unwrap(), 2);
    }

    #[test]
    fn a_non_usable_non_equippable_item_is_refused_and_stays_held() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Item(CursorItem {
            bar_placeable: false, // no on-use spell, InventoryType 0
            bag: 0,
            slot: 1,
            item_id: 2589, // Linen Cloth
            texture: Some("Interface\\Icons\\INV_Fabric_Linen_01".into()),
            link: None,
            count: None,
            quality: Some(1),
            equip_slots: Vec::new(),
        }));

        assert!(!s.eval::<bool>("return PlaceAction(4)").unwrap());
        assert!(!s.eval::<bool>("return HasAction(4)").unwrap(), "no store");
        assert!(s.take_action_sets().is_empty(), "no packet");
        assert!(
            matches!(s.cursor_payload(), Some(CursorPayload::Item(_))),
            "a refused payload stays on the cursor — the reference never clears it"
        );
        assert!(
            s.take_shown().is_empty(),
            "the ITEM refusal is MUTE — only the SPELL arm raises an error; a \
             toast here would be ours, not the reference's"
        );
    }

    #[test]
    fn a_passive_spell_is_refused_with_the_refs_error_and_stays_held() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 674, // Dual Wield, a passive
            texture: Some("Interface\\Icons\\Ability_DualWield".into()),
            passive: true,
        }));

        assert!(!s.eval::<bool>("return PlaceAction(4)").unwrap());
        assert!(!s.eval::<bool>("return HasAction(4)").unwrap());
        assert!(s.take_action_sets().is_empty());
        assert!(matches!(s.cursor_payload(), Some(CursorPayload::Spell(_))));
        assert_eq!(
            s.take_shown(),
            vec![super::PASSIVE_ON_BAR_ERROR],
            "the ref's errorId 0x9e toast — 'You can't put a passive ability in the action bar.'"
        );
        assert!(
            s.take_shown().is_empty(),
            "…and the queue drains, so the toast fires once per refusal"
        );
        // The refusal is raised while the verb holds the bar, and fires before it returns.
        s.run(
            "Log = {} local f = CreateFrame('Frame') f:RegisterEvent('UI_ERROR_MESSAGE') \
             f:SetScript('OnEvent', function(_, _, t) Log[#Log + 1] = t end)",
        )
        .unwrap();
        s.lua()
            .globals()
            .set(super::PASSIVE_ON_BAR_ERROR, "passive")
            .unwrap();
        s.run("PlaceAction(4) Log[#Log + 1] = 'after'").unwrap();
        assert_eq!(
            s.eval::<Vec<String>>("return Log").unwrap(),
            ["passive", "after"]
        );
    }

    #[test]
    fn an_active_spell_places_without_an_error() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 133, // Fireball
            texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
            passive: false,
        }));

        assert!(s.eval::<bool>("return PlaceAction(4)").unwrap());
        assert!(s.eval::<bool>("return HasAction(4)").unwrap());
        assert!(s.take_shown().is_empty());
    }

    #[test]
    fn a_bag_is_placeable() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Item(CursorItem {
            bar_placeable: true, // equippable (BAG), even with no on-use spell
            bag: 0,
            slot: 1,
            item_id: 4496, // Small Brown Pouch
            texture: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
            link: None,
            count: None,
            quality: Some(1),
            equip_slots: vec![20, 21, 22, 23],
        }));

        assert!(s.eval::<bool>("return PlaceAction(4)").unwrap());
        assert_eq!(s.take_action_sets(), vec![(4, 0x8000_0000u32 | 4496)]);
    }
}
