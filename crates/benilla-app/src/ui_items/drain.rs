//! The per-frame drains that send the queued Lua container intents as `ClientCommand`s, locking
//! the items each send touches in [`crate::pending_item_ops::PendingItemOps`].

use bevy::prelude::*;

use crate::ui_action::signals::SignalSink;

use benilla_protocol::messages::BAG_PLAYER_INVENTORY;
use benilla_ui::script::{item_lock_args, UiScript, EQUIPMENT_BAG};

use crate::items::Items;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfPlayer};
#[cfg(test)]
use crate::pending_item_ops::PendingItemOps;

use super::watch::LockStep;
use super::{slot_guid, slot_guid_count, wire_pos, INVTYPE_AMMO};

/// The one auto-equip sender, as in the reference (`AutoEquipCursorItem`); answers whether it sent.
/// An ammo item loads by entry with `CMSG_SET_AMMO` and stays in its bag. An unbound, equippable
/// `bonding == 2` item raises `AUTOEQUIP_BIND_CONFIRM` and sends nothing unless `suppress`, the
/// reference's flag, which the accept's re-issue sets.
pub(crate) fn send_auto_equip(
    script: &mut UiScript,
    gate: &mut crate::ui_bind_confirm::BindGate,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
    bag_index: u8,
    slot: u8,
    guid: Option<u64>,
    suppress: bool,
) -> bool {
    if !suppress {
        if let Some(guid) = guid {
            if gate.equip_binds(script, objects, items, commands, guid) {
                gate.defer_equip(
                    script,
                    crate::ui_bind_confirm::PendingEquip::AutoEquip {
                        bag_index,
                        slot,
                        guid,
                    },
                );
                return false;
            }
        }
    }
    let ammo_entry = guid.and_then(|guid| {
        let entry = objects.object(guid)?.object_entry()?;
        let t = items.template(entry, guid, commands)?;
        (t.inventory_type == INVTYPE_AMMO).then_some(entry)
    });
    if let Some(entry) = ammo_entry {
        debug!("ui_items: set ammo entry {entry} (wire {bag_index}/{slot})");
        let _ = commands.0.send(ClientCommand::SetAmmo { entry });
    } else {
        debug!("ui_items: autoequip wire {bag_index}/{slot}");
        let _ = commands
            .0
            .send(ClientCommand::AutoEquipItem { bag_index, slot });
    }
    true
}

/// Sends the auto-equips `AutoEquipCursorItem` queued through [`send_auto_equip`]. It takes no
/// pending lock, so the source slot does not dim while the equip is in flight.
pub(super) fn drain_container_autoequips(
    mut script: crate::ui_script::InTurn,
    items: Res<Items>,
    objects: Objects,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
    mut gate: crate::ui_bind_confirm::BindGate,
) {
    let Some(mut script) = script.turn() else {
        return;
    };
    // `EquipCursorItem`'s go out with the bind question off (`0x51a51a`).
    let asked = script.take_container_autoequips();
    let unasked = script.take_container_autoequips_unasked();
    let all = asked
        .into_iter()
        .map(|at| (at, false))
        .chain(unasked.into_iter().map(|at| (at, true)));
    for ((bag, slot), suppress) in all {
        let Some((bag_index, wire_slot)) = wire_pos(bag, slot) else {
            debug!("ui_items: autoequip ({bag}, {slot}) out of range — ignored");
            continue;
        };
        let slot0 = u8::try_from(slot.saturating_sub(1)).unwrap_or(0);
        let guid = self_q
            .iter()
            .next()
            .and_then(|store| slot_guid(&store.0, bag, slot0, &objects));
        send_auto_equip(
            &mut script,
            &mut gate,
            &objects,
            &items,
            &commands,
            bag_index,
            wire_slot,
            guid,
            suppress,
        );
    }
}

/// Sends the auto-stores `PutItemInBag`/`PutItemInBackpack` queued. The destination is a bag, not a
/// slot: `CMSG_AUTOSTORE_BAG_ITEM` names `(srcbag, srcslot, dstbag)` and the server picks the slot.
/// A split carry sends `CMSG_SPLIT_ITEM` instead. Takes no pending lock.
pub(super) fn drain_bag_autostores(
    mut script: crate::ui_script::InTurn,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script.turn() else {
        return;
    };
    for a in script.take_bag_autostores() {
        // A target no container id names goes out as the player slot holding it (`0x6dbb30`).
        let dst = match a.dst_player_slot {
            Some(slot) => Some((slot, 0)),
            None => wire_pos(a.dst_bag, 1),
        };
        let (Some((src_bag, src_slot)), Some((dst_bag, _))) =
            (wire_pos(a.src_bag, a.src_slot), dst)
        else {
            debug!("ui_items: bag autostore {a:?} out of range — ignored");
            continue;
        };
        match a.count {
            None => {
                debug!(
                    "ui_items: autostore lua {}/{} → bag {} (wire {src_bag}/{src_slot} → {dst_bag})",
                    a.src_bag, a.src_slot, a.dst_bag
                );
                let _ = commands.0.send(ClientCommand::AutoStoreBagItem {
                    src_bag,
                    src_slot,
                    dst_bag,
                });
            }
            Some(n) => {
                let count = n;
                debug!(
                    "ui_items: autostore split lua {}/{} → bag {} × {count} (wire {src_bag}/{src_slot} → {dst_bag})",
                    a.src_bag, a.src_slot, a.dst_bag
                );
                let _ = commands.0.send(ClientCommand::SplitItem {
                    src_bag,
                    src_slot,
                    dst_bag,
                    // The reference's literal for no destination slot.
                    dst_slot: 0xFF,
                    count,
                });
            }
        }
    }
}

/// What a `UseContainerItem` or `UseInventoryItem` call reads and writes.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ScriptItemUse<'w, 's> {
    /// `UseItemByName`'s target for the use this call runs, `None` the selection.
    aim: Local<'s, Option<crate::spell::cast_target::Aim>>,
    trade: Option<Res<'w, crate::ui_trade::TradeSession>>,
    self_q: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    equip_sound: MessageWriter<'w, crate::sound::AutoEquipSound>,
    /// The read call's queue, which runs it with the world once the use returns.
    queue: Commands<'w, 's>,
    targeting: crate::spell::cast_target::CastTargeting<'w, 's>,
    // The latch the open arm's `CloseLoot(1, 0, 0)` clears (`0x6dcd9c`).
    loot_latch: ResMut<'w, crate::ui_loot::LootLatch>,
    // The open arm's `CloseLoot(1, 0, 0)` (`0x6dcd9c`).
    loot: ResMut<'w, crate::ui_loot::LootState>,
    deselect: MessageWriter<'w, crate::target::DeselectGuid>,
    ladder: crate::spell::CastLadder<'w, 's>,
    gate: crate::ui_bind_confirm::BindGate<'w>,
    /// The banker, the bank flag's (`0xbeb9a0`).
    bank: Option<NonSend<'w, crate::ui_bank::BankOpen>>,
}

/// The context a use runs in: the selection, or the unit `UseItemByName` named.
fn aimed<'a>(
    targeting: &'a crate::spell::cast_target::CastTargeting,
    aim: Option<crate::spell::cast_target::Aim>,
) -> crate::spell::cast_target::CastContext<'a> {
    aim.map_or_else(|| targeting.context(), |a| targeting.context_at(a))
}

/// `EquipItemByName`'s slotted send (`0x6dff90`): the soulbind deferral, then the lock and its
/// edge (`0x6e0136` -> `0x513740`) and `CMSG_AUTOEQUIP_ITEM_SLOT` (`0x6e016e`), in the caller's turn.
pub(crate) fn send_equip_to_slot(
    script: &mut UiScript,
    gate: &mut crate::ui_bind_confirm::BindGate,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
    store: Option<&ObjectStore>,
    (bag, slot, dst): (i64, u32, u8),
) {
    let (guid, _) = slot_guid_count(store, bag, slot, objects);
    if guid == 0 {
        return;
    }
    if gate.equip_binds(script, objects, items, commands, guid) {
        gate.defer_equip(
            script,
            crate::ui_bind_confirm::PendingEquip::Slot { bag, slot, dst },
        );
        return;
    }
    super::watch::lock_then_send_in_turn(
        script,
        LockStep {
            guid,
            locked: true,
            args: Some(item_lock_args(bag, slot)),
        },
        ClientCommand::AutoEquipItemSlot {
            item: guid,
            slot: dst,
        },
    );
}

impl ScriptItemUse<'_, '_> {
    /// `UseItemByName`'s use (`0x51cd00`): the item's own use, at `aim`.
    pub(crate) fn use_item_at(
        &mut self,
        script: &mut UiScript,
        bag: i64,
        slot: u32,
        aim: crate::spell::cast_target::Aim,
    ) {
        *self.aim = Some(aim);
        if bag == benilla_ui::script::EQUIPMENT_BAG {
            self.use_inventory_item(script, slot);
        } else {
            self.use_item(script, bag, slot);
        }
        *self.aim = None;
    }

    /// `EquipItemByName`'s send (`0x51cdb0`), an item in our trade offer refused (`0x6dffb8`).
    pub(crate) fn equip_item(
        &mut self,
        script: &mut UiScript,
        bag: i64,
        slot: u32,
        dst: Option<u8>,
    ) {
        let store = self.self_q.iter().next();
        let Some(dst) = dst else {
            let Some((bag_index, wire_slot)) = wire_pos(bag, slot) else {
                return;
            };
            let slot0 = u8::try_from(slot.saturating_sub(1)).unwrap_or(0);
            let guid = store.and_then(|s| slot_guid(&s.0, bag, slot0, &self.ladder.objects));
            send_auto_equip(
                script,
                &mut self.gate,
                &self.ladder.objects,
                &self.ladder.items,
                &self.ladder.commands,
                bag_index,
                wire_slot,
                guid,
                false,
            );
            return;
        };
        if self.trade.as_deref().is_some_and(|t| t.offers(bag, slot)) {
            self.ladder
                .press
                .line(crate::ui_action::UiError::key("ERR_EQUIP_TRADE_ITEM"));
            return;
        }
        send_equip_to_slot(
            script,
            &mut self.gate,
            &self.ladder.objects,
            &self.ladder.items,
            &self.ladder.commands,
            store,
            (bag, slot, dst),
        );
    }

    /// `UseInventoryItem(id)`, a doll right-click, sent through [`super::send_item_use`], as the
    /// reference's doll click reaches the same item use a bag click does: a worn quest-starter
    /// offers its quest. Ids outside 1..=19 are refused.
    pub(crate) fn use_inventory_item(&mut self, script: &mut UiScript, id: u32) {
        let Self {
            aim,
            self_q,
            targeting,
            ladder,
            gate,
            ..
        } = self;
        if !(1..=19).contains(&id) {
            debug!("ui_items: UseInventoryItem({id}) out of range — ignored");
            return;
        }
        let slot = (id - 1) as u8;
        let (guid, start_quest, use_spell, entry, is_charter) = self_q
            .iter()
            .next()
            .and_then(|store| slot_guid(&store.0, EQUIPMENT_BAG, slot, &ladder.objects))
            .and_then(|guid| {
                let entry = ladder.objects.object(guid)?.object_entry()?;
                let t = ladder.items.template(entry, guid, &ladder.commands)?;
                Some((
                    Some(guid),
                    t.start_quest,
                    t.use_spell.map(|u| u.spell_id),
                    Some(entry),
                    t.flags & benilla_protocol::messages::ITEM_FLAG_CHARTER != 0,
                ))
            })
            .unwrap_or((None, 0, None, None, false));
        debug!("ui_items: use equipped item, lua slot {id} (wire 255/{slot})");
        super::send_item_use(
            super::ItemUse {
                guid,
                start_quest,
                bag_index: BAG_PLAYER_INVENTORY,
                slot,
                entry: entry.unwrap_or(0),
                use_spell,
                on_object: None,
                is_charter,
            },
            &aimed(targeting, **aim),
            ladder,
            script,
            gate,
            false,
        );
    }

    /// `UseContainerItem(bag, slot)` outside repair mode: a sale with a merchant open, a bank
    /// move with the bank open, else the reference's equip-vs-use fork and use dispatcher.
    pub(crate) fn use_container_item(
        &mut self,
        script: &mut UiScript,
        bag: i64,
        slot: u32,
        vendor: Option<u64>,
    ) {
        self.use_at(script, bag, slot, vendor, false);
    }

    /// The direct item use (`0x708c20`) on a bag item, `UseQuestLogSpecialItem`'s path: the use
    /// dispatcher alone, with no sale, bank move or equip.
    pub(crate) fn use_item(&mut self, script: &mut UiScript, bag: i64, slot: u32) {
        self.use_at(script, bag, slot, None, true);
    }

    fn use_at(
        &mut self,
        script: &mut UiScript,
        bag: i64,
        slot: u32,
        vendor: Option<u64>,
        direct: bool,
    ) {
        let Self {
            aim,
            trade: _,
            self_q,
            equip_sound,
            queue,
            targeting,
            loot_latch,
            loot,
            deselect,
            ladder,
            gate,
            bank,
        } = self;
        let slot0 = u8::try_from(slot.saturating_sub(1)).ok();
        // With a merchant open, the click sells the whole stack (`CMSG_SELL_ITEM`, count 0).
        if let (false, Some(vendor)) = (direct, vendor) {
            let item_guid = self_q
                .iter()
                .next()
                .and_then(|store| slot_guid(&store.0, bag, slot0.unwrap_or(0), &ladder.objects));
            match item_guid {
                Some(guid) => {
                    debug!("ui_items: sell lua bag {bag} slot {slot} (item {guid:#x})");
                    let _ = ladder.commands.0.send(ClientCommand::SellItem {
                        vendor,
                        item_guid: guid,
                        count: 0,
                    });
                }
                None => debug!("ui_items: sell on empty slot (bag {bag} slot {slot}) — ignored"),
            }
            return;
        }
        let Some((bag_index, wire_slot)) = wire_pos(bag, slot) else {
            debug!("ui_items: UseContainerItem({bag}, {slot}) out of range — ignored");
            return;
        };
        // With the bank open, a vault or bank-bag item withdraws (`CMSG_AUTOSTORE_BANK_ITEM`) and
        // any other deposits (`CMSG_AUTOBANK_ITEM`): the reference's bank flag, set for bags -1
        // and 5..11, picks the opcode. Doll clicks come through `use_inventory_item`, so worn gear
        // keeps its plain use.
        let bank_open = bank.as_ref().is_some_and(|b| b.banker.is_some());
        if !direct && bank_open {
            let withdrawing = bag == super::BANK_CONTAINER
                || (super::BANK_BAG_ID_FIRST..=super::BANK_BAG_ID_LAST).contains(&bag);
            if withdrawing {
                debug!("ui_items: withdraw (lua bag {bag} → wire {bag_index}/{wire_slot})");
                let _ = ladder.commands.0.send(ClientCommand::AutoStoreBankItem {
                    bag: bag_index,
                    slot: wire_slot,
                });
            } else {
                debug!("ui_items: deposit (lua bag {bag} → wire {bag_index}/{wire_slot})");
                let _ = ladder.commands.0.send(ClientCommand::AutoBankItem {
                    bag: bag_index,
                    slot: wire_slot,
                });
            }
            return;
        }
        // `None` (empty, or the template in flight) falls through to a plain use.
        let clicked = self_q
            .iter()
            .next()
            .and_then(|store| slot_guid(&store.0, bag, slot0.unwrap_or(0), &ladder.objects))
            .and_then(|guid| {
                let obj = ladder.objects.object(guid)?;
                let inst_flags = obj.item_flags().unwrap_or(0);
                let has_text = obj.item_has_text();
                let entry = obj.object_entry()?;
                let t = ladder.items.template(entry, guid, &ladder.commands)?;
                Some(Clicked {
                    guid,
                    entry,
                    class: t.class,
                    has_text,
                    inventory_type: t.inventory_type,
                    display_info_id: t.display_info_id,
                    start_quest: t.start_quest,
                    use_spell: t.use_spell.map(|u| u.spell_id),
                    unwraps_gift: t.unwraps_gift(inst_flags),
                    begins_gift_wrap: t.begins_gift_wrap(inst_flags),
                    opens_loot: t.opens_loot(),
                    page_text: t.page_text,
                    is_charter: t.flags & benilla_protocol::messages::ITEM_FLAG_CHARTER != 0,
                })
            });

        // The reference's equip-vs-use fork: an equippable item auto-equips unless its `StartQuest`
        // (`[rec+0x1a8]`) is set. Everything below is the use dispatcher, in its own order.
        if let Some(c) = clicked.filter(|c| !direct && c.start_quest == 0 && c.inventory_type != 0)
        {
            // This path never moves the cursor, so it plays the equip sound itself; a deferred
            // equip plays none, as the reference's sound rides the arm that sends.
            if send_auto_equip(
                script,
                gate,
                &ladder.objects,
                &ladder.items,
                &ladder.commands,
                bag_index,
                wire_slot,
                Some(c.guid),
                false,
            ) {
                equip_sound.write(crate::sound::AutoEquipSound {
                    display_id: c.display_info_id,
                });
            }
            return;
        }
        // A Glyph-class template (16) fires `USE_GLYPH` (`0x5d8ad3`), then the direct use runs after its listeners at its own step (`0x5d8ae3` -> `0x708c20`).
        if !direct && clicked.is_some_and(|c| c.class == ITEM_CLASS_GLYPH) {
            script.fire_event("USE_GLYPH", Vec::new());
            let aim = **aim;
            super::watch::world_in_turn(script, move |w| {
                crate::script_calls::press_step(w, direct_use_step, (bag, slot, aim));
            });
            return;
        }
        // A wrapped gift unwraps with `CMSG_OPEN_ITEM`, first in the dispatcher; vmangos swaps the
        // entry back from `character_gifts`.
        if let Some(c) = clicked.filter(|c| c.unwraps_gift) {
            debug!(
                "ui_items: unwrap gift {:#x} (lua bag {bag} → wire {bag_index}/{wire_slot})",
                c.guid
            );
            let _ = ladder.commands.0.send(ClientCommand::OpenItem {
                bag_index,
                slot: wire_slot,
            });
            return;
        }
        // Wrapping paper sends nothing: its slot locks and the cursor becomes mode 2 until a
        // left-click on a container slot sends `CMSG_WRAP_ITEM`.
        if let Some(c) = clicked.filter(|c| c.begins_gift_wrap) {
            debug!(
                "ui_items: arm gift wrap with {:#x} (lua bag {bag} slot {slot})",
                c.guid
            );
            script.arm_gift_wrap(bag, slot);
            return;
        }
        // A quest-starter offers its quest, ahead of the readable and open arms.
        if let Some(c) = clicked.filter(|c| c.start_quest != 0) {
            debug!(
                "ui_items: quest-starter {:#x} offers quest {}",
                c.guid, c.start_quest
            );
            super::send_item_use(
                super::ItemUse {
                    guid: Some(c.guid),
                    start_quest: c.start_quest,
                    bag_index,
                    slot: wire_slot,
                    entry: c.entry,
                    use_spell: c.use_spell,
                    on_object: None,
                    is_charter: c.is_charter,
                },
                &aimed(targeting, **aim),
                ladder,
                script,
                gate,
                false,
            );
            return;
        }
        // The requirement check follows the quest-starter arm and precedes the readable, open and
        // spell arms (`0x708eb9`).
        if let Some(c) = clicked {
            let enchant_skill = ladder
                .objects
                .object(c.guid)
                .and_then(|f| crate::items::use_enchant_skill(f, ladder.trade_window.1.as_deref()));
            if super::refuse_item_use(&ladder.use_reqs, c.entry, enchant_skill, &mut ladder.press) {
                return;
            }
        }
        // A book (a template with `PageText`) or a copied letter (`ITEM_FIELD_FLAGS & 0x200`) runs the read call with flag 0, so the open one closes, with no packet (`0x708f60`). It precedes the open arm, the inverse of the tooltip: an item both readable and lootable shows `<Right Click to Open>` and reads on click.
        if let Some(c) = clicked.filter(|c| c.page_text != 0 || c.has_text) {
            debug!("ui_items: read {:#x} (lua bag {bag} slot {slot})", c.guid);
            crate::ui_item_text::read_from_click(queue, c.guid);
            return;
        }
        // A lootable template sends `CMSG_OPEN_ITEM`, which the server answers with a loot window
        // on the item (`SpellHandler.cpp:227`). Unlike the tooltip line, the send tests neither the
        // lock id nor the unlocked bit, so a locked box sends and the server's
        // `EQUIP_ERR_ITEM_LOCKED` is the "Item is locked" line.
        if let Some(c) = clicked.filter(|c| c.opens_loot) {
            debug!(
                "ui_items: open item {:#x} (lua bag {bag} → wire {bag_index}/{wire_slot})",
                c.guid
            );
            // The item open (`0x6dcd70`): the open loot closes and the auto-loot flag is set first.
            let closed = crate::ui_loot::close_interaction(
                loot,
                loot_latch,
                &ladder.commands,
                true,
                &mut *script,
            );
            if let Some(g) =
                closed.filter(|&g| crate::ui_loot::dead_unit_source(&ladder.objects, g))
            {
                deselect.write(crate::target::DeselectGuid(g));
            }
            let (guid, _) = self_q
                .iter()
                .next()
                .map(|store| slot_guid_count(Some(store), bag, slot, &ladder.objects))
                .unwrap_or((0, 0));
            let lock = LockStep {
                guid,
                locked: true,
                args: Some(item_lock_args(bag, slot)),
            };
            let item = c.guid;
            // After the close's unlock in turn: the auto-loot flag and latch, the grey lock
            // (`0x6dcded`) and the send (`0x6dce3b`), in order in the applier's turn.
            super::watch::world_in_turn(script, move |w| {
                arm_item_loot(w, item);
                super::watch::lock_now(w, lock);
                super::watch::send_now(
                    w,
                    ClientCommand::OpenItem {
                        bag_index,
                        slot: wire_slot,
                    },
                );
            });
            return;
        }
        // A plain use (food, potions, the hearthstone); quest-starters took the arm above.
        debug!("ui_items: use item (lua bag {bag} → wire {bag_index}/{wire_slot})");
        super::send_item_use(
            super::ItemUse {
                guid: clicked.map(|c| c.guid),
                start_quest: 0,
                bag_index,
                slot: wire_slot,
                entry: clicked.map_or(0, |c| c.entry),
                use_spell: clicked.and_then(|c| c.use_spell),
                on_object: None,
                is_charter: clicked.is_some_and(|c| c.is_charter),
            },
            &aimed(targeting, **aim),
            ladder,
            script,
            gate,
            false,
        );
    }
}

/// The direct item use (`0x708c20`) on a bag item, at `aim` or the selection: the glyph arm's use after `USE_GLYPH`'s listeners.
fn direct_use_step(
    In((bag, slot, aim)): In<(i64, u32, Option<crate::spell::cast_target::Aim>)>,
    mut script: NonSendMut<UiScript>,
    mut items: ScriptItemUse,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
) {
    *items.aim = aim;
    items.use_item(&mut script, bag, slot);
    *items.aim = None;
    items.ladder.press.defer(&mut ui);
}

/// The item open's auto-loot flag and sweep (`0x6dcdc7`) and the latch on the item (`0x6dcdd4`), so its
/// type-1 `SMSG_LOOT_RESPONSE` passes.
fn arm_item_loot(world: &mut World, item: u64) {
    let auto = world
        .get_resource::<crate::ui_loot::LootConfig>()
        .is_some_and(|cfg| {
            crate::ui_loot::is_auto_looting(cfg, world.get_non_send_resource::<UiScript>())
        });
    // The open loot closed ahead of this, so the setter's sweep finds no window and raises nothing.
    let _ = bevy::ecs::system::RunSystemOnce::run_system_once(
        &mut *world,
        move |mut loot: ResMut<crate::ui_loot::LootState>,
              mut latch: ResMut<crate::ui_loot::LootLatch>,
              commands: Res<NetCommands>,
              ui: Option<ResMut<crate::ui_script::live::UiEvents>>| {
            let mut nobody = Vec::new();
            let mut ui = ui;
            let sink: &mut dyn crate::ui_action::signals::SignalSink = match ui.as_deref_mut() {
                Some(ui) => ui,
                None => &mut nobody,
            };
            loot.set_auto_loot(auto, &mut latch, &commands, sink);
        },
    );
    if let Some(mut latch) = world.get_resource_mut::<crate::ui_loot::LootLatch>() {
        latch.0 = Some(item);
    }
}

/// Sends the repair-mode clicks, bag and paper doll, as `CMSG_REPAIR_ITEM`.
pub(super) fn drain_container_repairs(
    mut vm: crate::ui_script::InTurn,
    merchant: Option<Res<crate::ui_merchant::MerchantOpen>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    objects: Objects,
    commands: Res<NetCommands>,
    mut purse: crate::ui_merchant::RepairPurse,
) {
    let vendor = merchant.as_ref().and_then(|m| m.vendor);
    let Some(mut script) = vm.turn() else {
        return;
    };
    let me = self_q.iter().next();
    // Repair-mode clicks repair the one item: the purse test first, then `ITEM_REPAIR` as it goes
    // out.
    for (bag, slot) in script.take_container_repairs() {
        let (Some(vendor), Some(me)) = (vendor, me) else {
            continue;
        };
        let slot0 = u8::try_from(slot.saturating_sub(1)).unwrap_or(0);
        match slot_guid(&me.0, bag, slot0, &objects) {
            Some(guid) => {
                if purse.refuses(&mut script, Some(vendor), me, [guid]) {
                    continue;
                }
                debug!("ui_items: repair lua bag {bag} slot {slot} (item {guid:#x})");
                script.queue_sound_kit("ITEM_REPAIR");
                let _ = commands.0.send(ClientCommand::RepairItem {
                    vendor,
                    item_guid: guid,
                    guild_bank: false,
                });
            }
            None => debug!("ui_items: repair on empty slot (bag {bag} slot {slot}) — ignored"),
        }
    }
    // The paper doll's repair clicks, the same arm on a worn item.
    for id in script.take_inventory_repairs() {
        let (Some(vendor), Some(me)) = (vendor, me) else {
            continue;
        };
        let item_guid = u8::try_from(id.wrapping_sub(1))
            .ok()
            .and_then(|slot0| slot_guid(&me.0, EQUIPMENT_BAG, slot0, &objects));
        match item_guid {
            Some(guid) if (1..=19).contains(&id) => {
                if purse.refuses(&mut script, Some(vendor), me, [guid]) {
                    continue;
                }
                debug!("ui_items: repair worn lua slot {id} (item {guid:#x})");
                script.queue_sound_kit("ITEM_REPAIR");
                let _ = commands.0.send(ClientCommand::RepairItem {
                    vendor,
                    item_guid: guid,
                    guild_bank: false,
                });
            }
            _ => debug!("ui_items: repair on worn lua slot {id}, empty or out of range — ignored"),
        }
    }
}

/// `ItemClass.dbc`'s Glyph class.
const ITEM_CLASS_GLYPH: u32 = 16;

/// One clicked bag slot, resolved once: the fields the reference's fork chain tests.
#[derive(Clone, Copy)]
struct Clicked {
    guid: u64,
    entry: u32,
    /// The template's item class; 16 is Glyph.
    class: u32,
    /// Instance `ITEM_FIELD_FLAGS & 0x200`, set on a letter copied from mail.
    has_text: bool,
    inventory_type: u32,
    display_info_id: u32,
    start_quest: u32,
    /// The template's on-use spell, the key of the cast tail's in-flight guard.
    use_spell: Option<u32>,
    unwraps_gift: bool,
    begins_gift_wrap: bool,
    opens_loot: bool,
    /// The template's `PageText`, nonzero on a book.
    page_text: u32,
    is_charter: bool,
}

/// Sends the gift wraps and the moves `PickupContainerItem`/`SplitContainerItem` queued, each move
/// at a step of its own through [`send_container_move`]; the verb locked their items at its steps.
pub(super) fn drain_container_moves(world: &mut World) {
    let _ = world.run_system_cached(send_container_wraps);
    let (asks, moves) = world
        .get_non_send_resource_mut::<UiScript>()
        .map(|mut s| (s.take_container_bind_asks(), s.take_container_moves()))
        .unwrap_or_default();
    let asked = asks.into_iter().map(|mv| (mv, true));
    for step in asked.chain(moves.into_iter().map(|mv| (mv, false))) {
        let _ = world.run_system_cached_with(move_step, step);
        crate::ui_script::live::fire_handler_events(world);
    }
}

/// The wraps a left-click with an armed paper queued: paper first, then the target. Eligibility is
/// the server's, refused with an `ERR_CANT_WRAP_*` reason.
fn send_container_wraps(mut vm: crate::ui_script::InTurn, commands: Res<NetCommands>) {
    let Some(mut script) = vm.turn() else {
        return;
    };
    for (gift_bag, gift_slot, item_bag, item_slot) in script.take_container_wraps() {
        let (Some((gift_bag_index, gift_wire_slot)), Some((item_bag_index, item_wire_slot))) =
            (wire_pos(gift_bag, gift_slot), wire_pos(item_bag, item_slot))
        else {
            debug!(
                "ui_items: WrapItem({gift_bag}/{gift_slot} → {item_bag}/{item_slot}) out of \
                 range — ignored"
            );
            continue;
        };
        debug!(
            "ui_items: wrap {item_bag}/{item_slot} with the paper at {gift_bag}/{gift_slot} \
             (wire {gift_bag_index}/{gift_wire_slot} → {item_bag_index}/{item_wire_slot})"
        );
        let _ = commands.0.send(ClientCommand::WrapItem {
            gift_bag: gift_bag_index,
            gift_slot: gift_wire_slot,
            item_bag: item_bag_index,
            item_slot: item_wire_slot,
        });
    }
}

/// One move: a swap that asked in its call is filed and its question raised (`0x6dface`,
/// `0x6dfb3d`), nothing going out; any other is sent.
fn move_step(
    In((mv, asked)): In<(benilla_ui::script::ContainerMove, bool)>,
    mut vm: crate::ui_script::InTurn,
    commands: Res<NetCommands>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    objects: Objects,
    items: Res<Items>,
    mut gate: crate::ui_bind_confirm::BindGate,
) {
    let Some(mut script) = vm.turn() else {
        return;
    };
    if asked {
        gate.defer_equip(
            &mut script,
            crate::ui_bind_confirm::PendingEquip::Swap {
                src_bag: mv.src_bag,
                src_slot: mv.src_slot,
                dst_bag: mv.dst_bag,
                dst_slot: mv.dst_slot,
            },
        );
        return;
    }
    send_container_move(
        &mut script,
        &mut gate,
        &objects,
        &items,
        &commands,
        self_q.iter().next(),
        mv,
        false,
    );
}

/// The equip deferral's positions: slots 0..=22 (equipment and equipped bags) and 67..=73 (bank
/// bags) of the player's own array; the backpack and keyring are not.
fn is_equip_position(bag_index: u8, slot: u8) -> bool {
    let bank_bags = super::BANK_BAG_SLOT_FIRST..super::BANK_BAG_SLOT_FIRST + super::BANK_BAGS;
    bag_index == BAG_PLAYER_INVENTORY && (slot <= 22 || bank_bags.contains(&slot))
}

/// The item swap's gate over its two ends before its send (`0x6df890`): with exactly one end an equip position, the other end's bag must be held (`0x6df991`, `0x6df9a2`) and its unbound item's record decides, a miss keeping the swap (`0x6dfa1d`) and a bind-on-equip one asking (`0x6dfaac`).
pub(crate) fn swap_gate(
    me: &benilla_protocol::ObjectFields,
    objects: &Objects,
    items: &Items,
    enchants: Option<&crate::items::Enchants>,
    src: (i64, u32),
    dst: (i64, u32),
) -> benilla_ui::script::SwapGate {
    use benilla_ui::script::SwapGate;
    let (Some(s), Some(d)) = (wire_pos(src.0, src.1), wire_pos(dst.0, dst.1)) else {
        return SwapGate::Refuse;
    };
    if is_equip_position(d.0, d.1) == is_equip_position(s.0, s.1) {
        return SwapGate::Send;
    }
    let ((bag, slot), (bag_index, _)) = if is_equip_position(d.0, d.1) {
        (src, s)
    } else {
        (dst, d)
    };
    // A worn or bank bag's object, where the end is inside one.
    if bag_index != BAG_PLAYER_INVENTORY {
        let held = me
            .player_slot(bag_index)
            .filter(|&g| g != 0)
            .is_some_and(|g| objects.object(g).is_some());
        if !held {
            return SwapGate::Refuse;
        }
    }
    let slot0 = u8::try_from(slot.saturating_sub(1)).unwrap_or(0);
    let Some(fields) = slot_guid(me, bag, slot0, objects).and_then(|g| objects.object(g)) else {
        return SwapGate::Send;
    };
    if crate::items::already_bound(fields, enchants) {
        return SwapGate::Send;
    }
    let Some(entry) = fields.object_entry() else {
        return SwapGate::Send;
    };
    match items.template_cached(entry) {
        None => SwapGate::Wait(entry),
        Some(t) if t.bonding == crate::ui_bind_confirm::BIND_WHEN_EQUIPPED => SwapGate::Ask(entry),
        Some(_) => SwapGate::Send,
    }
}

/// `AutoEquipCursorItem`'s gate over the held item (`0x6dfc40`): no item object refuses (`0x6dfd45`), an unbound item's record decides as the swap's does (`0x6dfd99`, `0x6dfda6`), and the class picks the ammunition arm (`0x6dfe51`).
pub(crate) fn auto_equip_gate(
    me: &benilla_protocol::ObjectFields,
    objects: &Objects,
    items: &Items,
    enchants: Option<&crate::items::Enchants>,
    bag: i64,
    slot: u32,
) -> (benilla_ui::script::SwapGate, bool) {
    use benilla_ui::script::SwapGate;
    let slot0 = u8::try_from(slot.saturating_sub(1)).unwrap_or(0);
    let Some(fields) = slot_guid(me, bag, slot0, objects).and_then(|g| objects.object(g)) else {
        return (SwapGate::Refuse, false);
    };
    let template = fields
        .object_entry()
        .map(|entry| (entry, items.template_cached(entry)));
    if !crate::items::already_bound(fields, enchants) {
        match template {
            Some((entry, None)) => return (SwapGate::Wait(entry), false),
            Some((entry, Some(t))) if t.bonding == crate::ui_bind_confirm::BIND_WHEN_EQUIPPED => {
                return (SwapGate::Ask(entry), false);
            }
            _ => {}
        }
    }
    let ammo = template.is_some_and(|(_, t)| t.is_some_and(|t| t.class == 6));
    (SwapGate::Send, ammo)
}

/// One container move: the soulbind deferral, then the send; the verb locked its items at its own steps (`0x5d82af`, `0x5d822f`). A whole stack
/// is `CMSG_SWAP_INV_ITEM` when both ends are in the player's own array, else `CMSG_SWAP_ITEM`
/// (destination first, `Server/Packets/Item.cpp:30-36`); a split is `CMSG_SPLIT_ITEM`. The
/// accept's re-issue runs it with `suppress`. False when deferred behind `EQUIP_BIND_CONFIRM`.
pub(crate) fn send_container_move(
    script: &mut UiScript,
    gate: &mut crate::ui_bind_confirm::BindGate,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
    store: Option<&ObjectStore>,
    mv: benilla_ui::script::ContainerMove,
    suppress: bool,
) -> bool {
    {
        let (Some(src), Some(dst)) = (
            wire_pos(mv.src_bag, mv.src_slot),
            wire_pos(mv.dst_bag, mv.dst_slot),
        ) else {
            debug!("ui_items: container move {mv:?} out of range — ignored");
            return false;
        };
        // A swap no verb gated runs the swap's gate here (`0x6df890`): a record not cached keeps the swap for its callback and asks for it (`0x6dfa1d`), a bind-on-equip item the player can use is asked about (`0x6dfac1`), and an end whose bag is not held sends nothing (`0x6df98f`).
        if let Some(me) = store.filter(|_| !suppress && mv.count.is_none()) {
            use benilla_ui::script::SwapGate;
            let decided = swap_gate(
                &me.0,
                objects,
                items,
                gate.enchants.as_deref(),
                (mv.src_bag, mv.src_slot),
                (mv.dst_bag, mv.dst_slot),
            );
            match decided {
                SwapGate::Wait(entry) => {
                    items.template(entry, 0, commands);
                    // The wait is left on the record's store, read with the `World` lent, in the caller's turn.
                    benilla_ui::script::world_loan::in_turn_or_now(script, move |s| {
                        s.keep_swap_for_record(entry, mv)
                    });
                    return false;
                }
                SwapGate::Ask(entry) if script.item_usable(entry) => {
                    gate.defer_equip(
                        script,
                        crate::ui_bind_confirm::PendingEquip::Swap {
                            src_bag: mv.src_bag,
                            src_slot: mv.src_slot,
                            dst_bag: mv.dst_bag,
                            dst_slot: mv.dst_slot,
                        },
                    );
                    return false;
                }
                SwapGate::Refuse => return false,
                SwapGate::Ask(_) | SwapGate::Send => {}
            }
        }
        match mv.count {
            None => {
                let (src_wire_bag, src_slot) = src;
                let (dst_wire_bag, dst_slot) = dst;
                if src_wire_bag == BAG_PLAYER_INVENTORY && dst_wire_bag == BAG_PLAYER_INVENTORY {
                    debug!(
                        "ui_items: swap backpack lua {}→{} (wire 255 slot {src_slot}↔{dst_slot})",
                        mv.src_slot, mv.dst_slot
                    );
                    let _ = commands
                        .0
                        .send(ClientCommand::SwapInvItem { src_slot, dst_slot });
                } else {
                    debug!(
                        "ui_items: swap whole-space lua {}/{}→{}/{} (wire {src_wire_bag}/{src_slot}→{dst_wire_bag}/{dst_slot})",
                        mv.src_bag, mv.src_slot, mv.dst_bag, mv.dst_slot
                    );
                    let _ = commands.0.send(ClientCommand::SwapItem {
                        dst_bag: dst_wire_bag,
                        dst_slot,
                        src_bag: src_wire_bag,
                        src_slot,
                    });
                }
            }
            Some(n) => {
                let (src_bag, src_slot) = src;
                let (dst_bag, dst_slot) = dst;
                let count = n;
                debug!(
                    "ui_items: split lua {}/{}→{}/{} × {count} (wire {src_bag}/{src_slot}→{dst_bag}/{dst_slot})",
                    mv.src_bag, mv.src_slot, mv.dst_bag, mv.dst_slot
                );
                let _ = commands.0.send(ClientCommand::SplitItem {
                    src_bag,
                    src_slot,
                    dst_bag,
                    dst_slot,
                    count,
                });
            }
        }
    }
    true
}

/// Sends the destroys `DeleteCursorItem` queued as `CMSG_DESTROYITEM` (count 0 is the whole stack
/// on both sides), each at a step of its own; the verb locked the item after its clear (`0x52244a`).
pub(super) fn drain_container_destroys(world: &mut World) {
    let destroys = world
        .get_non_send_resource_mut::<UiScript>()
        .map(|mut s| s.take_container_destroys())
        .unwrap_or_default();
    for destroy in destroys {
        let _ = world.run_system_cached_with(destroy_step, destroy);
    }
}

/// One destroy's send.
fn destroy_step(In((bag, slot, count)): In<(i64, u32, u32)>, commands: Res<NetCommands>) {
    let Some((bag_index, wire_slot)) = wire_pos(bag, slot) else {
        debug!("ui_items: destroy ({bag}, {slot}) out of range — ignored");
        return;
    };
    let count = count.min(u32::from(u8::MAX)) as u8;
    debug!("ui_items: destroy lua {bag}/{slot} × {count} (wire {bag_index}/{wire_slot})");
    let _ = commands.0.send(ClientCommand::DestroyItem {
        bag_index,
        slot: wire_slot,
        count,
    });
}

/// The bag clicks as the frame applies them: the repair-mode drain, then the queued
/// `UseContainerItem` calls through their applier.
#[cfg(test)]
fn run_container_clicks(world: &mut World) {
    use benilla_ui::script::ScriptCall;
    use bevy::ecs::system::RunSystemOnce;
    world.run_system_once(drain_container_repairs).unwrap();
    let Some(mut script) = world
        .get_non_send_resource::<UiScript>()
        .map(UiScript::share)
    else {
        return;
    };
    let lua = script.lua().clone();
    crate::script_calls::in_call_order(&mut script, |_, call| {
        let ScriptCall::UseContainerItem { bag, slot } = call else {
            return;
        };
        // The applier's raises in turn, fired as it returns with the `World` lent.
        let (_, raised) = benilla_ui::script::world_loan::raises_in_turn(&lua, || {
            world.run_system_once_with(
                |In((bag, slot)): In<(i64, u32)>,
                 mut script: NonSendMut<UiScript>,
                 mut items: ScriptItemUse| {
                    items.use_container_item(&mut script, bag, slot, None);
                },
                (bag, slot),
            )
        });
        crate::ui_script::safe_world::lend(world, |s| raised.into_iter().for_each(|r| r.run(s)));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{ItemInfo, ObjectFields, ITEM_FLAG_LOOTABLE};

    /// Small Barnacled Clam.
    const CLAM_ENTRY: u32 = 7973;
    const CLAM: u64 = 0x4000_0000_0000_1939;
    /// `PLAYER_FIELD_PACK_SLOT_1`, backpack slot 1's guid pair.
    const F_PACK_SLOT_1: u16 = benilla_protocol::field::FIELD_PLAYER_PACK_SLOT_1;
    /// `OBJECT_FIELD_ENTRY` on the item object.
    const F_OBJECT_ENTRY: u16 = benilla_protocol::field::FIELD_OBJECT_ENTRY;

    /// Right-clicks backpack slot 1, holding a lootable template, and runs the click dispatcher.
    fn open_the_clam() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_message::<crate::sound::AutoEquipSound>()
            .add_message::<crate::creature_anim::SheathRequest>()
            .init_resource::<crate::ui_merchant::MerchantOpen>()
            .init_non_send_resource::<crate::ui_bank::BankOpen>()
            .init_resource::<crate::ui_item_text::ItemText>()
            .init_resource::<PendingItemOps>()
            .init_resource::<crate::ui_bind_confirm::PendingEquips>()
            .init_resource::<crate::ui_bind_confirm::PendingBindOnUse>()
            .init_resource::<crate::ui_loot::LootLatch>()
            .init_resource::<crate::ui_loot::LootState>()
            .init_resource::<crate::ui_loot::LootConfig>()
            .add_message::<crate::target::DeselectGuid>()
            .init_resource::<crate::target::Selection>()
            .init_resource::<crate::net::SelfGuid>()
            .init_resource::<crate::spell::cast_target::AutoSelfCast>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::player::Player>()
            .init_resource::<crate::spell::PendingCast>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_action::RefusalsSeen>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::spell::AutoRepeatActive>()
            .init_resource::<crate::spell::targeting::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx));

        app.world_mut().spawn((
            SelfPlayer,
            ObjectStore(ObjectFields::from_pairs(&[
                (F_PACK_SLOT_1, CLAM as u32),
                (F_PACK_SLOT_1 + 1, (CLAM >> 32) as u32),
            ])),
        ));
        // The item object and its lootable template, so the open arm claims the click.
        crate::items::test_spawn_item(
            app.world_mut(),
            CLAM,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, CLAM_ENTRY)]),
            false,
        );
        let mut items = app.world_mut().resource_mut::<Items>();
        items.insert_template(
            CLAM_ENTRY,
            Some(ItemInfo {
                flags: ITEM_FLAG_LOOTABLE,
                ..crate::items::test_template("Small Barnacled Clam")
            }),
        );

        let script = UiScript::new().unwrap();
        script.run("UseContainerItem(0, 1)").unwrap();
        app.insert_non_send_resource(script);
        run_container_clicks(app.world_mut());
        (app, rx)
    }

    /// vmangos answers with a type-1 `SMSG_LOOT_RESPONSE` on the item's guid, refused against a
    /// cold latch: without the latch the clam greys and no window opens.
    #[test]
    fn the_open_item_send_arms_the_loot_latch_on_the_items_own_guid() {
        let (app, rx) = open_the_clam();
        assert!(
            matches!(
                rx.try_recv(),
                Ok(ClientCommand::OpenItem {
                    bag_index: 255,
                    slot: 23
                })
            ),
            "the open arm ships CMSG_OPEN_ITEM for backpack slot 1"
        );
        assert_eq!(
            app.world().resource::<crate::ui_loot::LootLatch>().0,
            Some(CLAM),
            "…having first latched the item's own guid, or the type-1 answer is refused"
        );
        // The grey lock, the reference's other pre-send write.
        assert!(
            app.world().resource::<PendingItemOps>().is_locked(CLAM),
            "the slot greys at the click"
        );
    }

    /// A repair-mode right-click on a bag item: `ITEM_REPAIR` as it goes out, then
    /// `CMSG_REPAIR_ITEM` with the vendor's and the item's guid, and no use or sale.
    #[test]
    fn a_repair_click_sounds_and_ships_the_items_guid() {
        const VENDOR: u64 = 0xF130_0000_0000_0042;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        app.world_mut()
            .resource_mut::<crate::ui_merchant::MerchantOpen>()
            .vendor = Some(VENDOR);
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.take_sounds();
            let mut bag = benilla_ui::script::ContainerState {
                num_slots: 16,
                ..Default::default()
            };
            bag.slots.insert(
                1,
                benilla_ui::script::ContainerSlot {
                    item_id: CLAM_ENTRY,
                    count: 1,
                    ..Default::default()
                },
            );
            script.set_container(0, Some(bag));
            script.set_merchant(Some(benilla_ui::script::MerchantState {
                can_repair: true,
                ..Default::default()
            }));
            script
                .run("ShowRepairCursor() UseContainerItem(0, 1)")
                .unwrap();
        }
        run_container_clicks(app.world_mut());

        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::RepairItem {
                vendor: VENDOR,
                item_guid: CLAM,
                guild_bank: false,
            })
        ));
        assert!(rx.try_recv().is_err(), "the repair alone: no use or sale");
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        assert_eq!(
            script.take_sounds(),
            vec![benilla_ui::script::SoundRequest::KitName(
                "ITEM_REPAIR".into()
            )]
        );
    }

    /// A repair-mode click on a worn item: `ITEM_REPAIR` as it goes out, then `CMSG_REPAIR_ITEM`
    /// with the vendor's and the worn item's guid.
    #[test]
    fn a_worn_repair_click_sounds_and_ships_the_items_guid() {
        const VENDOR: u64 = 0xF130_0000_0000_0042;
        const HELM: u64 = 0x4000_0000_0000_0777;
        let head = benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        app.world_mut()
            .resource_mut::<crate::ui_merchant::MerchantOpen>()
            .vendor = Some(VENDOR);
        let me = app
            .world_mut()
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(me)
            .insert(ObjectStore(ObjectFields::from_pairs(&[
                (head, HELM as u32),
                (head + 1, (HELM >> 32) as u32),
            ])));
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.take_sounds();
            let mut slots: benilla_ui::script::InventorySlots = Default::default();
            slots[1] = Some(benilla_ui::script::InvSlotView {
                item_id: 7,
                ..Default::default()
            });
            script.set_inventory_slots(slots);
            script.set_merchant(Some(benilla_ui::script::MerchantState {
                can_repair: true,
                ..Default::default()
            }));
            script
                .run("ShowRepairCursor() PickupInventoryItem(1)")
                .unwrap();
        }
        run_container_clicks(app.world_mut());

        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::RepairItem {
                vendor: VENDOR,
                item_guid: HELM,
                guild_bank: false,
            })
        ));
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        assert_eq!(
            script.take_sounds(),
            vec![benilla_ui::script::SoundRequest::KitName(
                "ITEM_REPAIR".into()
            )]
        );
    }

    /// What one repair click did: the packets, the VM's sounds, the `UI_ERROR_MESSAGE` lines and
    /// the message rows queued for their sound.
    struct RepairClick {
        sent: Vec<ClientCommand>,
        sounds: Vec<benilla_ui::script::SoundRequest>,
        shown: Vec<String>,
        sounded: Vec<&'static str>,
    }

    /// A repair-mode click on [`crate::ui_merchant::purse_fixture`]'s sword with `money` in the
    /// purse: in backpack slot 1 through `UseContainerItem`, or `worn` in the main hand (slot 15)
    /// through `PickupInventoryItem`.
    fn click_to_repair_the_sword(money: u32, worn: bool) -> RepairClick {
        use crate::ui_merchant::purse_fixture::{player, record_errors, seat, SWORD_ENTRY};
        use benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        seat(app.world_mut());
        let slot_field = if worn {
            FIELD_PLAYER_INV_SLOT_HEAD + 2 * 15
        } else {
            F_PACK_SLOT_1
        };
        let me = app
            .world_mut()
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(me)
            .insert(player(slot_field, money));
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.take_sounds();
            record_errors(&script);
            script.set_merchant(Some(benilla_ui::script::MerchantState {
                can_repair: true,
                ..Default::default()
            }));
            let click = if worn {
                let mut slots: benilla_ui::script::InventorySlots = Default::default();
                slots[16] = Some(benilla_ui::script::InvSlotView {
                    item_id: SWORD_ENTRY,
                    ..Default::default()
                });
                script.set_inventory_slots(slots);
                "ShowRepairCursor() PickupInventoryItem(16)"
            } else {
                let mut bag = benilla_ui::script::ContainerState {
                    num_slots: 16,
                    ..Default::default()
                };
                bag.slots.insert(
                    1,
                    benilla_ui::script::ContainerSlot {
                        item_id: SWORD_ENTRY,
                        count: 1,
                        ..Default::default()
                    },
                );
                script.set_container(0, Some(bag));
                "ShowRepairCursor() UseContainerItem(0, 1)"
            };
            script.run(click).unwrap();
        }
        run_container_clicks(app.world_mut());
        let sounded = crate::ui_merchant::purse_fixture::sounded(app.world());
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        RepairClick {
            sent: rx.try_iter().collect(),
            sounds: script.take_sounds(),
            shown: crate::ui_merchant::purse_fixture::shown(&script),
            sounded,
        }
    }

    /// The repair arm's purse test, unsigned `cost > money` on the discounted cost: one copper
    /// short shows `ERR_NOT_ENOUGH_MONEY` with its speech row, plays no `ITEM_REPAIR` and sends
    /// nothing; exactly the cost sounds and sends.
    fn the_purse_test_refuses_one_copper_short_and_pays_the_exact_cost(worn: bool) {
        use crate::ui_merchant::purse_fixture::{sword_price, NOT_ENOUGH_MONEY, SWORD, VENDOR};
        let price = sword_price();

        let short = click_to_repair_the_sword(price - 1, worn);
        assert!(short.sent.is_empty(), "short of {price}: nothing goes out");
        assert!(short.sounds.is_empty(), "…no ITEM_REPAIR");
        assert_eq!(short.shown, [NOT_ENOUGH_MONEY], "…the red line");
        assert_eq!(
            short.sounded,
            ["ERR_NOT_ENOUGH_MONEY"],
            "…and its row, which speaks line 0x28"
        );

        let exact = click_to_repair_the_sword(price, worn);
        assert!(
            matches!(
                exact.sent.as_slice(),
                [ClientCommand::RepairItem {
                    vendor: VENDOR,
                    item_guid: SWORD,
                    guild_bank: false,
                }]
            ),
            "{price} in the purse pays the {price}"
        );
        assert_eq!(
            exact.sounds,
            vec![benilla_ui::script::SoundRequest::KitName(
                "ITEM_REPAIR".into()
            )]
        );
        assert!(exact.shown.is_empty() && exact.sounded.is_empty());
    }

    /// `UseContainerItem`'s repair arm.
    #[test]
    fn a_bag_repair_the_purse_cannot_cover_is_refused_and_one_it_covers_is_sent() {
        the_purse_test_refuses_one_copper_short_and_pays_the_exact_cost(false);
    }

    /// `PickupInventoryItem`'s repair arm.
    #[test]
    fn a_worn_repair_the_purse_cannot_cover_is_refused_and_one_it_covers_is_sent() {
        the_purse_test_refuses_one_copper_short_and_pays_the_exact_cost(true);
    }

    /// The unwrap arm's other side: wrapping paper arms the wrap cursor and sends nothing.
    #[test]
    fn a_wrapper_right_click_arms_the_cursor_and_ships_no_packet() {
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
                                       // Slot 1 becomes wrapping paper: WRAPPER on the
                                       // template, no WRAPPED bit on the instance.
        let mut items = app.world_mut().resource_mut::<Items>();
        items.insert_template(
            CLAM_ENTRY,
            Some(ItemInfo {
                flags: benilla_protocol::messages::ITEM_FLAG_WRAPPER,
                ..crate::items::test_template("Red Ribboned Wrapping Paper")
            }),
        );
        {
            let script = app.world().non_send_resource::<UiScript>();
            script.run("UseContainerItem(0, 1)").unwrap();
        }
        run_container_clicks(app.world_mut());

        assert!(
            rx.try_recv().is_err(),
            "the begin-wrap arm is purely local — nothing goes on the wire until a container \
             click spends it"
        );
        let script = app.world().non_send_resource::<UiScript>();
        assert_eq!(
            script.gift_wrap_armed(),
            Some(benilla_ui::script::PendingWrap { bag: 0, slot: 1 }),
            "…and the paper is armed"
        );
    }
}

/// The item at a wire position: the player's slot under bag 255, else the slot of the bag in that player slot.
fn wire_item(store: Option<&ObjectStore>, objects: &Objects, (bag, slot): (u8, u8)) -> u64 {
    let Some(me) = store else {
        return 0;
    };
    if bag == BAG_PLAYER_INVENTORY {
        return me.0.player_slot(slot).unwrap_or(0);
    }
    me.0.player_slot(bag)
        .and_then(|g| objects.object(g))
        .and_then(|b| b.container_slot(slot))
        .unwrap_or(0)
}

/// The cancel's two unlocks (`0x6e0273`, `0x6e02cd`): the record's source and destination occupants; a slotted equip files its equip slot as the source slot in the item's own container (`0x6e00d7`), and an auto-equip has no destination.
fn cancel_occupants(
    store: Option<&ObjectStore>,
    objects: &Objects,
    rec: &crate::ui_bind_confirm::PendingEquip,
) -> Vec<u64> {
    use crate::ui_bind_confirm::PendingEquip;
    let at = |pos: Option<(u8, u8)>| pos.map_or(0, |p| wire_item(store, objects, p));
    match *rec {
        PendingEquip::Swap {
            src_bag,
            src_slot,
            dst_bag,
            dst_slot,
        } => vec![
            at(wire_pos(src_bag, src_slot)),
            at(wire_pos(dst_bag, dst_slot)),
        ],
        PendingEquip::Slot { bag, slot, dst } => {
            vec![at(
                wire_pos(bag, slot).map(|(container, _)| (container, dst))
            )]
        }
        PendingEquip::AutoEquip {
            bag_index, slot, ..
        } => vec![at(Some((bag_index, slot)))],
    }
}

/// One step of a pending equip's answer (`0x6e01a0`), each ending where the reference raises.
#[derive(Clone, Copy)]
pub(super) enum AnswerStep {
    /// The answers given so far, taken.
    Take,
    /// The cancel's first or second occupant, read off the record as it stands (`0x6e0227`, `0x6e027b`).
    Occupant(u32, usize),
    /// The accept's re-run of the record's sender with `suppress` set (`0x6e01d4`, `0x6e01fa`, `0x6e021d`).
    Accept(u32),
    /// The record zeroed, after either arm (`0x6e02e2`).
    Free(u32),
}

/// What a step hands back: the answers taken, or the occupant to unlock.
#[derive(Default)]
pub(super) struct AnswerOut {
    answers: Vec<benilla_ui::script::PendingEquipAnswer>,
    unlock: Option<u64>,
}

/// One [`AnswerStep`] over the record at its index; an unfiled index does nothing, as the reference bounds-checks it and a zeroed record names no item.
#[allow(clippy::too_many_arguments)] // the senders' state, one parameter each
pub(super) fn equip_answer_step(
    In(step): In<AnswerStep>,
    mut vm: crate::ui_script::InTurn,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    objects: Objects,
    items: Res<Items>,
    commands: Res<NetCommands>,
    mut gate: crate::ui_bind_confirm::BindGate,
    trade: Option<Res<crate::ui_trade::TradeSession>>,
) -> AnswerOut {
    use crate::ui_bind_confirm::PendingEquip;
    let mut out = AnswerOut::default();
    let Some(mut script) = vm.turn() else {
        return out;
    };
    let store = self_q.iter().next();
    match step {
        AnswerStep::Take => out.answers = script.take_pending_equip_answers(),
        AnswerStep::Occupant(index, which) => {
            out.unlock = gate
                .equips
                .get(index)
                .and_then(|rec| cancel_occupants(store, &objects, &rec).get(which).copied());
        }
        AnswerStep::Free(index) => {
            gate.equips.take(index);
        }
        AnswerStep::Accept(index) => match gate.equips.get(index) {
            None => debug!("ui_items: bind answer for index {index} — no such pending equip"),
            Some(PendingEquip::Swap {
                src_bag,
                src_slot,
                dst_bag,
                dst_slot,
            }) => {
                send_container_move(
                    &mut script,
                    &mut gate,
                    &objects,
                    &items,
                    &commands,
                    store,
                    benilla_ui::script::ContainerMove {
                        src_bag,
                        src_slot,
                        dst_bag,
                        dst_slot,
                        count: None,
                    },
                    true,
                );
            }
            Some(PendingEquip::Slot { bag, slot, dst }) => {
                let (guid, _) = slot_guid_count(store, bag, slot, &objects);
                if guid == 0 {
                    return out;
                }
                // An item in our trade offer is refused on the re-run too (`0x6dffb8`); the action gate `0x5191c0` after it is #1227.
                if trade.as_deref().is_some_and(|t| t.offers(bag, slot)) {
                    if let Some(ui) = script.queue() {
                        crate::ui_action::show_in_turn(
                            ui,
                            "ui_items",
                            crate::ui_action::UiError::key("ERR_EQUIP_TRADE_ITEM"),
                        );
                    }
                    return out;
                }
                // The lock, its edge and the usable update (`0x6e0136` -> `0x513740`), each at its place ahead of the send (`0x6e016e`).
                super::watch::lock_then_send_in_turn(
                    &mut script,
                    LockStep {
                        guid,
                        locked: true,
                        args: Some(item_lock_args(bag, slot)),
                    },
                    ClientCommand::AutoEquipItemSlot {
                        item: guid,
                        slot: dst,
                    },
                );
            }
            Some(PendingEquip::AutoEquip {
                bag_index,
                slot,
                guid,
            }) => {
                send_auto_equip(
                    &mut script,
                    &mut gate,
                    &objects,
                    &items,
                    &commands,
                    bag_index,
                    slot,
                    Some(guid),
                    true,
                );
            }
        },
    }
    out
}

/// The answers to `EquipPendingItem`/`CancelPendingEquip` (`0x6e01a0`), each raise fired at its step with the `World` lent: accept re-runs the record's sender with `suppress` set, a slot that changed under the dialog judged afresh; cancel unlocks the first occupant (`0x6e0273`) and then the second (`0x6e02cd`); the record is zeroed after either (`0x6e02e2`). `false` in a world without the senders' state, where the answers wait.
pub(super) fn run_equip_answers(world: &mut World) -> bool {
    use crate::script_calls::in_call_step;
    fn step(world: &mut World, step: AnswerStep) -> Option<AnswerOut> {
        in_call_step(world, |world| {
            world.run_system_cached_with(equip_answer_step, step).ok()
        })
    }
    let Some(taken) = step(world, AnswerStep::Take) else {
        return false;
    };
    for answer in taken.answers {
        if answer.accept {
            step(world, AnswerStep::Accept(answer.index));
        } else {
            debug!("ui_items: pending equip {} cancelled", answer.index);
            for which in 0..2 {
                in_call_step(world, |world| {
                    let occupant = world
                        .run_system_cached_with(
                            equip_answer_step,
                            AnswerStep::Occupant(answer.index, which),
                        )
                        .ok()
                        .and_then(|out| out.unlock);
                    // The unlock (`0x513770`): a locked item's edge, then the usable update either way.
                    if let Some(guid) = occupant {
                        super::watch::unlock_item(world, guid);
                    }
                });
            }
        }
        step(world, AnswerStep::Free(answer.index));
    }
    true
}

/// The answers given where no `World` was lent to the verb, run as [`run_equip_answers`] runs them inside one.
pub(crate) fn drain_bind_confirm_answers(world: &mut World) {
    let waiting = world
        .get_non_send_resource::<UiScript>()
        .is_some_and(UiScript::has_pending_equip_answers);
    if waiting {
        run_equip_answers(world);
    }
}

/// `ConfirmBindOnUse()`: re-issues the pending use through the cast ladder with `suppress` set.
/// The pending state is one cell, taken on the first accept, so a doubled accept does nothing.
pub(super) fn drain_bind_on_use_confirms(
    mut vm: crate::ui_script::InTurn,
    targeting: crate::spell::cast_target::CastTargeting,
    mut ladder: crate::spell::CastLadder,
    mut gate: crate::ui_bind_confirm::BindGate,
) {
    let Some(mut script) = vm.turn() else {
        return;
    };
    if script.take_bind_on_use_confirms() == 0 {
        return;
    }
    let Some(it) = gate.on_use.0.take() else {
        return;
    };
    debug!(
        "ui_items: bind-on-use confirmed for wire {}/{}",
        it.bag_index, it.slot
    );
    super::send_item_use(
        it,
        &targeting.context(),
        &mut ladder,
        &mut script,
        &mut gate,
        true,
    );
    // The use's cast-bar edges, in turn as it returns (`0x80b556`).
    ladder.press.defer_in_turn(&mut script);
}

#[cfg(test)]
mod bind_confirm_tests {
    use super::*;
    use benilla_protocol::messages::{ItemInfo, ObjectFields};
    use benilla_ui::script::{ContainerSlot, ContainerState};
    use bevy::ecs::system::RunSystemOnce;

    /// Flurry Axe, a real `item_template` row: quality 4, bonding 2 (bind on equip).
    const FLURRY_AXE: u32 = 871;
    const AXE_GUID: u64 = 0x4000_0000_0000_0871;
    /// `ITEM_FIELD_FLAGS`; bit 0 is soulbound.
    const F_ITEM_FLAGS: u16 = benilla_protocol::field::FIELD_ITEM_FLAGS;
    /// `PLAYER_FIELD_PACK_SLOT_1`, backpack slot 1's guid pair.
    const F_PACK_SLOT_1: u16 = benilla_protocol::field::FIELD_PLAYER_PACK_SLOT_1;
    const F_OBJECT_ENTRY: u16 = benilla_protocol::field::FIELD_OBJECT_ENTRY;

    /// A world holding the axe in backpack slot 1, its record cached or not, and a bare VM.
    fn axe_world(
        bonding: Option<u32>,
        bound: bool,
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<PendingItemOps>()
            .init_resource::<crate::ui_bind_confirm::PendingEquips>()
            .init_resource::<crate::ui_bind_confirm::PendingBindOnUse>()
            .init_resource::<Items>()
            .init_resource::<crate::names::NameCache>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(crate::net::SelfGuid(Some(0x42)))
            .insert_resource(NetCommands(tx));
        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                ObjectStore(ObjectFields::from_pairs(&[
                    (F_PACK_SLOT_1, AXE_GUID as u32),
                    (F_PACK_SLOT_1 + 1, (AXE_GUID >> 32) as u32),
                ])),
            ))
            .id();
        app.world_mut()
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(0x42, me);
        crate::items::test_spawn_item(
            app.world_mut(),
            AXE_GUID,
            ObjectFields::from_pairs(&[
                (F_OBJECT_ENTRY, FLURRY_AXE),
                (F_ITEM_FLAGS, u32::from(bound)),
            ]),
            false,
        );
        if let Some(bonding) = bonding {
            app.world_mut().resource_mut::<Items>().insert_template(
                FLURRY_AXE,
                Some(ItemInfo {
                    bonding,
                    inventory_type: 17,
                    ..crate::items::test_template("Flurry Axe")
                }),
            );
        }
        app.insert_non_send_resource(UiScript::new().unwrap());
        (app, rx)
    }

    fn gate_of(app: &mut App, dst: (i64, u32)) -> benilla_ui::script::SwapGate {
        app.world_mut()
            .run_system_once(move |objects: Objects, items: Res<Items>| {
                swap_gate(
                    objects.player_object().expect("the player"),
                    &objects,
                    &items,
                    None,
                    (0, 1),
                    dst,
                )
            })
            .unwrap()
    }

    /// The swap's gate before its send (`0x6df890`): only an equip of an unbound item reads its record, a miss keeping the swap (`0x6dfa1d`), bonding 2 asking (`0x6dfaac`); a bag end whose bag is not held refuses (`0x6df991`).
    #[test]
    fn the_swap_gate_reads_the_equipped_items_record() {
        use benilla_ui::script::SwapGate;
        let main_hand = (benilla_ui::script::EQUIPMENT_BAG, 16);
        let (mut app, _rx) = axe_world(Some(2), false);
        assert_eq!(gate_of(&mut app, main_hand), SwapGate::Ask(FLURRY_AXE));
        assert_eq!(gate_of(&mut app, (0, 2)), SwapGate::Send, "no equip end");
        for bonding in [0, 1, 3, 4] {
            let (mut app, _rx) = axe_world(Some(bonding), false);
            assert_eq!(gate_of(&mut app, main_hand), SwapGate::Send, "{bonding}");
        }
        let (mut app, _rx) = axe_world(Some(2), true);
        assert_eq!(gate_of(&mut app, main_hand), SwapGate::Send, "bound");
        let (mut app, _rx) = axe_world(None, false);
        assert_eq!(gate_of(&mut app, main_hand), SwapGate::Wait(FLURRY_AXE));
        // The worn item put down into bag 1, which holds no bag object.
        let (mut app, _rx) = axe_world(Some(2), false);
        let gate = app
            .world_mut()
            .run_system_once(|objects: Objects, items: Res<Items>| {
                swap_gate(
                    objects.player_object().expect("the player"),
                    &objects,
                    &items,
                    None,
                    (benilla_ui::script::EQUIPMENT_BAG, 16),
                    (1, 1),
                )
            })
            .unwrap();
        assert_eq!(gate, SwapGate::Refuse);
    }

    /// `AutoEquipCursorItem`'s gate (`0x6dfc40`): no item refuses (`0x6dfd45`), a record miss waits (`0x6dfd99`), bonding 2 asks (`0x6dfda6`), and ammunition takes its own arm (`0x6dfe51`).
    #[test]
    fn the_auto_equip_gate_reads_the_held_items_record() {
        use benilla_ui::script::SwapGate;
        let gate = |app: &mut App, slot: u32| {
            app.world_mut()
                .run_system_once(move |objects: Objects, items: Res<Items>| {
                    auto_equip_gate(
                        objects.player_object().expect("the player"),
                        &objects,
                        &items,
                        None,
                        0,
                        slot,
                    )
                })
                .unwrap()
        };
        let (mut app, _rx) = axe_world(Some(2), false);
        assert_eq!(gate(&mut app, 1), (SwapGate::Ask(FLURRY_AXE), false));
        assert_eq!(gate(&mut app, 2), (SwapGate::Refuse, false));
        let (mut app, _rx) = axe_world(Some(2), true);
        assert_eq!(gate(&mut app, 1), (SwapGate::Send, false));
        let (mut app, _rx) = axe_world(None, false);
        assert_eq!(gate(&mut app, 1), (SwapGate::Wait(FLURRY_AXE), false));
        let (mut app, _rx) = axe_world(Some(0), false);
        app.world_mut().resource_mut::<Items>().insert_template(
            FLURRY_AXE,
            Some(ItemInfo {
                class: 6,
                ..crate::items::test_template("Arrow")
            }),
        );
        assert_eq!(gate(&mut app, 1), (SwapGate::Send, true));
    }

    /// A bind-on-equip item put on the doll files its record and signals `EQUIP_BIND_CONFIRM` inside the call (`0x6dface`, `0x6dfb3d`) and returns with the cursor still holding it (`0x6dfb45`); the drain files nothing more and sends nothing, and the accept's re-run sends the swap.
    #[test]
    fn placing_a_bind_on_equip_item_keeps_it_on_the_cursor_and_sends_nothing() {
        let (mut app, rx) = axe_world(Some(2), false);
        let (held, asked) = crate::ui_script::live::with_live_world(app.world_mut(), |script| {
            script
                .run(
                    r#"local f = CreateFrame("Frame")
                       f:RegisterEvent("EQUIP_BIND_CONFIRM")
                       f:SetScript("OnEvent", function(_, _, index)
                           ASKED = { index, CursorHasItem(), AFTER }
                       end)
                       PickupContainerItem(0, 1) PickupInventoryItem(16) AFTER = true"#,
                )
                .unwrap();
            let asked = script
                .eval::<bool>(
                    "return ASKED and ASKED[1] == 0 and ASKED[2] == 1 and ASKED[3] == nil",
                )
                .unwrap();
            (script.cursor_item(), asked)
        })
        .unwrap();
        assert_eq!(
            held.map(|i| (i.bag, i.slot, i.item_id)),
            Some((0, 1, FLURRY_AXE)),
            "the question leaves the cursor as it was"
        );
        assert!(
            asked,
            "the question signals inside the call, with its index"
        );
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            1,
            "filed inside the call"
        );
        app.world_mut()
            .run_system_once(drain_container_moves)
            .unwrap();
        assert!(rx.try_iter().next().is_none(), "nothing goes out");
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            1
        );
        assert!(
            app.world().resource::<PendingItemOps>().is_locked(AXE_GUID),
            "the held item is still locked"
        );

        // The accept re-runs the swap suppressed (`0x6e021d`), which sends.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("EquipPendingItem(0)")
            .unwrap();
        app.world_mut()
            .run_system_once(drain_bind_confirm_answers)
            .unwrap();
        assert!(matches!(
            rx.try_iter().next(),
            Some(ClientCommand::SwapInvItem { .. })
        ));
    }

    /// The dialog's cancel unlocks the swap's items inside the call (`0x6e0273`, `0x6e02cd`), and its accept re-runs the swap, which sends before the verb returns (`0x6e021d`, `0x6dfc04`).
    #[test]
    fn the_bind_answers_run_inside_the_call() {
        let (mut app, rx) = axe_world(Some(2), false);
        let log = crate::ui_script::live::with_live_world(app.world_mut(), |script| {
            script
                .run(
                    r#"local f = CreateFrame("Frame")
                       f:RegisterEvent("ITEM_UNLOCKED")
                       f:SetScript("OnEvent", function(_, e) Log[#Log + 1] = e end)
                       PickupContainerItem(0, 1) PickupInventoryItem(16)
                       Log = {}
                       CancelPendingEquip(0) Log[#Log + 1] = "after cancel""#,
                )
                .unwrap();
            script.eval::<Vec<String>>("return Log").unwrap()
        })
        .unwrap();
        assert_eq!(log, ["ITEM_UNLOCKED", "after cancel"]);
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            0
        );
        assert!(!app.world().resource::<PendingItemOps>().is_locked(AXE_GUID));
        assert!(rx.try_iter().next().is_none());

        // The question again, then the accept: the swap goes out inside the call.
        let rx = &rx;
        let sent = crate::ui_script::live::with_live_world(app.world_mut(), |script| {
            script
                .run("PickupInventoryItem(16) EquipPendingItem(0)")
                .unwrap();
            rx.try_iter().next()
        })
        .unwrap();
        assert!(matches!(sent, Some(ClientCommand::SwapInvItem { .. })));
        assert!(app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .take_pending_equip_answers()
            .is_empty());
    }

    const SWORD_GUID: u64 = 0x4000_0000_0000_0872;

    /// The axe's world with a sword in the main hand, both locked under a filed swap, and `Locked(guid)` reading an item's lock off the lent `World`.
    fn a_filed_swap_with_both_ends_locked() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (mut app, rx) = axe_world(Some(2), false);
        let main_hand = benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD + 2 * 15;
        let me = app
            .world_mut()
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(me)
            .insert(ObjectStore(ObjectFields::from_pairs(&[
                (F_PACK_SLOT_1, AXE_GUID as u32),
                (F_PACK_SLOT_1 + 1, (AXE_GUID >> 32) as u32),
                (main_hand, SWORD_GUID as u32),
                (main_hand + 1, (SWORD_GUID >> 32) as u32),
            ])));
        crate::items::test_spawn_item(
            app.world_mut(),
            SWORD_GUID,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, 25)]),
            false,
        );
        let lua = app.world().non_send_resource::<UiScript>().lua().clone();
        let locked = lua
            .create_function(|_, which: String| {
                let guid = if which == "axe" { AXE_GUID } else { SWORD_GUID };
                Ok(benilla_ui::script::read_live(|l| l.item_locked(guid)).flatten())
            })
            .unwrap();
        lua.globals().set("Locked", locked).unwrap();
        crate::ui_script::live::with_live_world(app.world_mut(), |script| {
            script
                .run("PickupContainerItem(0, 1) PickupInventoryItem(16)")
                .unwrap();
        })
        .unwrap();
        let mut pending = app.world_mut().resource_mut::<PendingItemOps>();
        pending.lock(AXE_GUID);
        pending.lock(SWORD_GUID);
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            1
        );
        (app, rx)
    }

    /// The cancel unlocks its first occupant and signals it (`0x6e0273` -> `0x707030`, `0x707041`) with the second still locked, then unlocks the second (`0x6e02cd`), and frees the record after both (`0x6e02e2`).
    #[test]
    fn a_cancel_signals_each_unlock_before_the_next() {
        let (mut app, rx) = a_filed_swap_with_both_ends_locked();
        let log = crate::ui_script::live::with_live_world(app.world_mut(), |script| {
            script
                .run(
                    r#"Log = {}
                       local f = CreateFrame("Frame")
                       f:RegisterEvent("ITEM_LOCK_CHANGED")
                       f:RegisterEvent("ITEM_UNLOCKED")
                       f:SetScript("OnEvent", function(_, e, a, b)
                           Log[#Log + 1] = table.concat({ e, tostring(a), tostring(b),
                               tostring(Locked("axe")), tostring(Locked("sword")) }, " ")
                       end)
                       CancelPendingEquip(0) Log[#Log + 1] = "after""#,
                )
                .unwrap();
            script.eval::<Vec<String>>("return Log").unwrap()
        })
        .unwrap();
        assert_eq!(
            log,
            [
                "ITEM_LOCK_CHANGED 0 1 false true",
                "ITEM_UNLOCKED 0 1 false true",
                "ITEM_LOCK_CHANGED 16 nil false false",
                "ITEM_UNLOCKED 16 nil false false",
                "after",
            ]
        );
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            0
        );
        assert!(rx.try_iter().next().is_none());
    }

    /// The axe's world with a slotted equip filed for it and `Sent()` counting what has gone out.
    fn a_filed_slotted_equip() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (mut app, rx) = axe_world(Some(2), false);
        let index = app
            .world_mut()
            .resource_mut::<crate::ui_bind_confirm::PendingEquips>()
            .add(crate::ui_bind_confirm::PendingEquip::Slot {
                bag: 0,
                slot: 1,
                dst: 15,
            });
        assert_eq!(index, 0);
        let lua = app.world().non_send_resource::<UiScript>().lua().clone();
        let out = rx.clone();
        let sent = lua.create_function(move |_, ()| Ok(out.len())).unwrap();
        lua.globals().set("Sent", sent).unwrap();
        (app, rx)
    }

    const LOG_THE_LOCK: &str = r#"Log = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("ITEM_LOCK_CHANGED")
        f:RegisterEvent("ITEM_LOCKED")
        f:SetScript("OnEvent", function(_, e) Log[#Log + 1] = e .. " " .. Sent() end)
        EquipPendingItem(0) Log[#Log + 1] = "after " .. Sent()"#;

    /// A slotted equip's accept locks the item and signals the edge (`0x6e0136`) before its packet goes out (`0x6e016e`), and frees the record (`0x6e02e2`).
    #[test]
    fn a_slotted_accept_signals_its_lock_before_it_sends() {
        let (mut app, rx) = a_filed_slotted_equip();
        let log = crate::ui_script::live::with_live_world(app.world_mut(), |script| {
            script.run(LOG_THE_LOCK).unwrap();
            script.eval::<Vec<String>>("return Log").unwrap()
        })
        .unwrap();
        assert_eq!(log, ["ITEM_LOCK_CHANGED 0", "ITEM_LOCKED 0", "after 1"]);
        assert!(matches!(
            rx.try_iter().next(),
            Some(ClientCommand::AutoEquipItemSlot {
                item: AXE_GUID,
                slot: 15
            })
        ));
        assert!(app.world().resource::<PendingItemOps>().is_locked(AXE_GUID));
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            0
        );
    }

    /// The accept's re-run refuses an item put in our trade offer since the question (`0x6dffb8`): no lock, no packet, the record freed.
    #[test]
    fn a_slotted_accept_refuses_an_item_in_our_trade_offer() {
        let (mut app, rx) = a_filed_slotted_equip();
        let mut trade = crate::ui_trade::TradeSession::default();
        trade.offer_from_for_test(0, 1);
        app.insert_resource(trade);
        let log = crate::ui_script::live::with_live_world(app.world_mut(), |script| {
            script.run(LOG_THE_LOCK).unwrap();
            script.eval::<Vec<String>>("return Log").unwrap()
        })
        .unwrap();
        assert_eq!(log, ["after 0"]);
        assert!(rx.try_iter().next().is_none());
        assert!(!app.world().resource::<PendingItemOps>().is_locked(AXE_GUID));
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            0
        );
    }

    /// A listener that cancels the same record inside the first unlock's signal finds it still filed (`0x6e02e2` comes last): its own cancel unlocks the second occupant, and the outer one then finds the record zeroed.
    #[test]
    fn a_cancel_from_the_first_unlocks_listener_finds_the_record_filed() {
        let (mut app, _rx) = a_filed_swap_with_both_ends_locked();
        let log = crate::ui_script::live::with_live_world(app.world_mut(), |script| {
            script
                .run(
                    r#"Log = {}
                       local f = CreateFrame("Frame")
                       f:RegisterEvent("ITEM_UNLOCKED")
                       f:SetScript("OnEvent", function(_, e, a)
                           Log[#Log + 1] = "in " .. a
                           if a == 0 then CancelPendingEquip(0) end
                           Log[#Log + 1] = "out " .. a
                       end)
                       CancelPendingEquip(0) Log[#Log + 1] = "after""#,
                )
                .unwrap();
            script.eval::<Vec<String>>("return Log").unwrap()
        })
        .unwrap();
        assert_eq!(log, ["in 0", "in 16", "out 16", "out 0", "after"]);
        assert!(!app
            .world()
            .resource::<PendingItemOps>()
            .is_locked(SWORD_GUID));
    }

    /// The same put-down of an item that binds nothing sends and clears the cursor, the item locked until the answer (`0x6dfc04`, `0x6dfc0d`).
    #[test]
    fn placing_an_item_that_binds_nothing_sends_and_clears_the_cursor() {
        let (mut app, rx) = axe_world(Some(0), false);
        let held = crate::ui_script::live::with_live_world(app.world_mut(), |script| {
            script
                .run("PickupContainerItem(0, 1) PickupInventoryItem(16)")
                .unwrap();
            script.cursor_payload()
        })
        .unwrap();
        assert!(held.is_none());
        app.world_mut()
            .run_system_once(drain_container_moves)
            .unwrap();
        assert!(matches!(
            rx.try_iter().next(),
            Some(ClientCommand::SwapInvItem { .. })
        ));
        assert!(app.world().resource::<PendingItemOps>().is_locked(AXE_GUID));
    }

    /// A swap no verb gated, its item's record not cached: the drain's gate keeps it on the record's callback and asks for the record, sending no swap (`0x6dfa1d`); the callback runs it again and it goes out (`0x6db6f0`).
    #[test]
    fn a_drained_swap_whose_record_is_not_cached_waits_for_it() {
        let (mut app, rx) = axe_world(None, false);
        app.init_non_send_resource::<crate::items::RecordWaits>();
        // The drain's steps fire their raises off the handler queue, with the `World` lent.
        app.init_resource::<crate::ui_script::live::UiEvents>();
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.set_container(0, Some(bag_with_the_axe(1)));
            script.set_inventory_slots(Default::default());
            // No `World` is lent: the verb's own gate reads nothing and the move waits for the drain.
            script
                .run("PickupContainerItem(0, 1) PickupInventoryItem(16)")
                .unwrap();
        }
        app.world_mut()
            .run_system_once(drain_container_moves)
            .unwrap();
        let sent: Vec<String> = rx.try_iter().map(|c| format!("{c:?}")).collect();
        assert!(
            sent.iter().all(|c| !c.starts_with("Swap")) && sent.iter().any(|c| c.contains("871")),
            "the record is asked for and no swap goes out: {sent:?}"
        );
        let waits = app
            .world()
            .non_send_resource::<crate::items::RecordWaits>()
            .waiting()
            .to_vec();
        let [(FLURRY_AXE, crate::items::RecordWait::Ui(wait))] = waits[..] else {
            panic!("one wait on the axe's record: {waits:?}");
        };
        assert!(matches!(wait, benilla_ui::script::ItemRecordWait::Swap(_)));

        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(FLURRY_AXE, Some(crate::items::test_template("Flurry Axe")));
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .item_record_callback(FLURRY_AXE, true, wait);
        app.world_mut()
            .run_system_once(drain_container_moves)
            .unwrap();
        let sent: Vec<String> = rx.try_iter().map(|c| format!("{c:?}")).collect();
        assert_eq!(sent, ["SwapInvItem { src_slot: 23, dst_slot: 15 }"]);
    }

    fn load_ui(s: &UiScript) {
        // Chain files, so through the chain-aware reader.
        for file in [
            "Interface\\FrameXML\\GlobalStrings.lua",
            "Interface\\FrameXML\\Fonts.xml",
            "Interface\\FrameXML\\BasicControls.xml",
            "Interface\\FrameXML\\LocaleProperties.lua",
            r"Interface\FrameXML\UIParent.xml",
            r"Interface\FrameXML\MoneyFrame.lua",
            r"Interface\FrameXML\MoneyFrame.xml",
            "Interface\\FrameXML\\StaticPopup.xml",
        ] {
            crate::ui_script::load_ui_for_test(s, file);
        }
    }

    fn bag_with_the_axe(quality: u32) -> ContainerState {
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1,
            ContainerSlot {
                guid: 0,
                gems: [0; 3],
                quest_item: false,
                start_quest: 0,
                duration_ms: None,
                petition: None,
                already_bound: false,
                bar_placeable: true,
                durability: None,
                texture: Some("Interface\\Icons\\INV_Axe_01".into()),
                count: 1,
                quality: Some(quality),
                item_id: FLURRY_AXE,
                link: Some("|cffa335ee|Hitem:871:0:0:0:0:0:0:0:0|h[Flurry Axe]|h|r".into()),
                locked: false,
                equip_slots: vec![16], // MainHandSlot
                cooldown: None,
                readable: false,
                creator: None,
                flags: 0,
                enchants: Vec::new(),
            },
        );
        ContainerState {
            bag_family: 0,
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }
    }

    /// Places the axe from backpack slot 1 on the empty main hand and runs the move drain.
    fn place_the_axe_on_the_doll() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        place_the_axe_with(2, 4, false)
    }

    fn place_the_axe_with_bonding(
        bonding: u32,
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        place_the_axe_with(bonding, 4, false)
    }

    /// The place, over everything the gate reads; `quality` must not matter.
    fn place_the_axe_with(
        bonding: u32,
        quality: u32,
        already_bound: bool,
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<PendingItemOps>()
            .init_resource::<crate::ui_bind_confirm::PendingEquips>()
            .init_resource::<crate::ui_bind_confirm::PendingBindOnUse>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx));

        app.world_mut().spawn((
            SelfPlayer,
            ObjectStore(ObjectFields::from_pairs(&[
                (F_PACK_SLOT_1, AXE_GUID as u32),
                (F_PACK_SLOT_1 + 1, (AXE_GUID >> 32) as u32),
            ])),
        ));
        crate::items::test_spawn_item(
            app.world_mut(),
            AXE_GUID,
            ObjectFields::from_pairs(&[
                (F_OBJECT_ENTRY, FLURRY_AXE),
                (F_ITEM_FLAGS, u32::from(already_bound)),
            ]),
            false,
        );
        let mut items = app.world_mut().resource_mut::<Items>();
        items.insert_template(
            FLURRY_AXE,
            Some(ItemInfo {
                quality,
                bonding,
                ..crate::items::test_template("Flurry Axe")
            }),
        );

        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        load_ui(&script);
        script.set_container(0, Some(bag_with_the_axe(quality)));
        let doll: benilla_ui::script::InventorySlots = Default::default();
        script.set_inventory_slots(doll);
        // Pick up and drop on the main hand: a queued move and a pending CURSOR_UPDATE.
        script.run("PickupContainerItem(0, 1)").unwrap();
        script.run("PickupInventoryItem(16)").unwrap();
        app.insert_non_send_resource(script);
        app.world_mut()
            .run_system_once(drain_container_moves)
            .unwrap();
        (app, rx)
    }

    fn shown(app: &mut App, which: &str) -> bool {
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .eval::<bool>(&format!(
                "return StaticPopup_FindVisible(\"{which}\") ~= nil"
            ))
            .unwrap()
    }

    /// The question is queued behind the place's own `CURSOR_UPDATE`, whose `StaticPopup_Hide`
    /// (`UIParent.lua:357-359`) would otherwise cancel it unseen: benilla drains a step after the
    /// input pass, where the reference defers inside the call. A later cursor change retires it.
    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn placing_a_boe_on_the_doll_asks_and_survives_its_own_cursor_update() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = place_the_axe_on_the_doll();
        assert!(
            rx.try_iter().next().is_none(),
            "the deferred place sends nothing"
        );
        assert!(
            !shown(&mut app, "EQUIP_BIND"),
            "the question is queued, so it is not up in the drain's own frame"
        );

        // The next tick flushes the place's CURSOR_UPDATE, then the question.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(
            shown(&mut app, "EQUIP_BIND"),
            "the dialog survives its own place's CURSOR_UPDATE and shows"
        );
        assert_eq!(
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<i64>("return StaticPopup_FindVisible(\"EQUIP_BIND\").data")
                .unwrap(),
            0,
            "carrying the pending-array index, which is 0 for the first record"
        );

        // A quiet frame changes nothing.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(shown(&mut app, "EQUIP_BIND"), "and stays up");

        // A later cursor change retires it, and its OnHide cancels the record.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .fire_event("CURSOR_UPDATE", vec![]);
        assert!(
            !shown(&mut app, "EQUIP_BIND"),
            "a later cursor change still retires the stale question"
        );
        app.world_mut()
            .run_system_once(drain_bind_confirm_answers)
            .unwrap();
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            0,
            "and the record it was holding is freed, not leaked"
        );
        assert!(
            rx.try_iter().next().is_none(),
            "a retired question never sends"
        );
    }

    /// The accept re-issues the place (1.12 has no confirm opcode); `suppress` stops a second ask.
    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn accepting_re_issues_the_place_and_does_not_ask_again() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = place_the_axe_on_the_doll();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(shown(&mut app, "EQUIP_BIND"));

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("StaticPopup_OnClick(StaticPopup_FindVisible(\"EQUIP_BIND\"), 1)")
            .unwrap();
        app.world_mut()
            .run_system_once(drain_bind_confirm_answers)
            .unwrap();

        // Backpack slot 1 is 255/23 and the main hand 255/15, both in the player's own array.
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [ClientCommand::SwapInvItem {
                    src_slot: 23,
                    dst_slot: 15
                }]
            ),
            "the accept sends the original swap, once: {sent:?}"
        );
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            0,
            "and frees its record — the OnHide's cancel finds nothing left to cancel"
        );
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(
            !shown(&mut app, "EQUIP_BIND"),
            "and no second question is queued: the re-issue was suppressed"
        );
    }

    /// The stock popup cancels twice, from `OnCancel` and `OnHide`; the second finds the record
    /// already free.
    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn cancelling_sends_nothing_and_the_doubled_cancel_is_harmless() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = place_the_axe_on_the_doll();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("StaticPopup_OnClick(StaticPopup_FindVisible(\"EQUIP_BIND\"), 2)")
            .unwrap();
        assert_eq!(
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .take_pending_equip_answers()
                .len(),
            2,
            "OnCancel and OnHide both fire"
        );
        app.world_mut()
            .run_system_once(drain_bind_confirm_answers)
            .unwrap();
        assert!(rx.try_iter().next().is_none(), "a cancel never sends");
    }

    /// The equip predicate is `bonding == 2` alone, with no quality leg.
    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn only_bind_on_equip_defers_the_place() {
        benilla_formats::wow_data_or_skip!();
        for (bonding, why) in [
            (0u32, "no bind"),
            (1, "bind on PICKUP is the loot arm's value, not this one"),
            (3, "bind on USE is the use arm's"),
            (4, "quest item"),
        ] {
            let (mut app, rx) = place_the_axe_with_bonding(bonding);
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .tick(0.01);
            assert!(
                !shown(&mut app, "EQUIP_BIND"),
                "bonding {bonding} must not ask ({why})"
            );
            assert!(
                rx.try_iter().next().is_some(),
                "bonding {bonding} places straight through ({why})"
            );
        }
    }

    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn a_white_bind_on_equip_item_still_asks() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = place_the_axe_with(2, 1, false);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(
            shown(&mut app, "EQUIP_BIND"),
            "quality 1 is irrelevant to the equip arm"
        );
        assert!(rx.try_iter().next().is_none());
    }

    /// The predicate the tooltip's Soulbound override also uses.
    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn an_already_bound_item_places_without_asking() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = place_the_axe_with(2, 4, true);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(
            !shown(&mut app, "EQUIP_BIND"),
            "there is nothing left to bind"
        );
        assert!(rx.try_iter().next().is_some(), "and it places");
    }

    /// The auto-equip arm (event 289): the same predicate and re-issue, its own dialog.
    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn auto_equipping_a_boe_asks_and_the_accept_re_issues() {
        benilla_formats::wow_data_or_skip!();
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<PendingItemOps>()
            .init_resource::<crate::ui_bind_confirm::PendingEquips>()
            .init_resource::<crate::ui_bind_confirm::PendingBindOnUse>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx));
        app.world_mut().spawn((
            SelfPlayer,
            ObjectStore(ObjectFields::from_pairs(&[
                (F_PACK_SLOT_1, AXE_GUID as u32),
                (F_PACK_SLOT_1 + 1, (AXE_GUID >> 32) as u32),
            ])),
        ));
        crate::items::test_spawn_item(
            app.world_mut(),
            AXE_GUID,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, FLURRY_AXE)]),
            false,
        );
        let mut items = app.world_mut().resource_mut::<Items>();
        items.insert_template(
            FLURRY_AXE,
            Some(ItemInfo {
                quality: 4,
                bonding: 2,
                ..crate::items::test_template("Flurry Axe")
            }),
        );
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        load_ui(&script);
        script.set_container(0, Some(bag_with_the_axe(4)));
        script.run("PickupContainerItem(0, 1)").unwrap();
        script.run("AutoEquipCursorItem()").unwrap();
        app.insert_non_send_resource(script);
        app.world_mut()
            .run_system_once(drain_container_autoequips)
            .unwrap();

        assert!(
            rx.try_iter().next().is_none(),
            "the deferred equip sends nothing"
        );
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(
            shown(&mut app, "AUTOEQUIP_BIND"),
            "the auto-equip arm raises its OWN dialog, not the placement one"
        );
        assert!(
            !shown(&mut app, "EQUIP_BIND"),
            "and the two are distinct entries"
        );

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("StaticPopup_OnClick(StaticPopup_FindVisible(\"AUTOEQUIP_BIND\"), 1)")
            .unwrap();
        app.world_mut()
            .run_system_once(drain_bind_confirm_answers)
            .unwrap();
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [ClientCommand::AutoEquipItem {
                    bag_index: 255,
                    slot: 23
                }]
            ),
            "the accept re-issues the auto-equip on the item's wire position: {sent:?}"
        );
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            0
        );
    }

    /// The conjunct, driven by its level leg.
    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn an_unusable_item_is_never_asked_about() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = place_the_axe_with(2, 4, false);
        // A real player level (the fixture's 0 declines to judge) and a template it refuses.
        {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.set_player_req_state(benilla_ui::script::PlayerReqState {
                level: 10,
                class_id: 1,
                race_id: 1,
                ..Default::default()
            });
            s.set_item_template(
                FLURRY_AXE,
                benilla_ui::script::ItemTemplateView {
                    required_level: 60,
                    allowable_class: -1,
                    allowable_race: -1,
                    ..Default::default()
                },
            );
        }
        // Re-run the place now that the item is out of reach.
        {
            let s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.run("PickupContainerItem(0, 1)").unwrap();
            s.run("PickupInventoryItem(16)").unwrap();
        }
        let _ = rx.try_iter().count();
        app.world_mut()
            .run_system_once(drain_container_moves)
            .unwrap();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(
            !shown(&mut app, "EQUIP_BIND"),
            "a level-60 axe on a level-10 player asks nothing"
        );
        assert!(
            rx.try_iter().next().is_some(),
            "it places, and the server refuses it — which is where that refusal belongs"
        );
    }

    /// Right-clicks a bind-on-use item in a bag (the use arm, event 290); `no_use_spell` drops its
    /// on-use spell, since four of the five paths into the reference's question are failed spell
    /// lookups and it still asks.
    fn right_click_a_bind_on_use_item(
        no_use_spell: bool,
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_message::<crate::sound::AutoEquipSound>()
            .add_message::<crate::creature_anim::SheathRequest>()
            .init_resource::<crate::ui_merchant::MerchantOpen>()
            .init_non_send_resource::<crate::ui_bank::BankOpen>()
            .init_resource::<crate::ui_item_text::ItemText>()
            .init_resource::<PendingItemOps>()
            .init_resource::<crate::ui_bind_confirm::PendingEquips>()
            .init_resource::<crate::ui_bind_confirm::PendingBindOnUse>()
            .init_resource::<crate::ui_loot::LootLatch>()
            .init_resource::<crate::ui_loot::LootState>()
            .init_resource::<crate::ui_loot::LootConfig>()
            .add_message::<crate::target::DeselectGuid>()
            .init_resource::<crate::target::Selection>()
            .init_resource::<crate::net::SelfGuid>()
            .init_resource::<crate::spell::cast_target::AutoSelfCast>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::player::Player>()
            .init_resource::<crate::spell::PendingCast>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_action::RefusalsSeen>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::spell::AutoRepeatActive>()
            .init_resource::<crate::spell::targeting::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx));
        app.world_mut().spawn((
            SelfPlayer,
            ObjectStore(ObjectFields::from_pairs(&[
                (F_PACK_SLOT_1, AXE_GUID as u32),
                (F_PACK_SLOT_1 + 1, (AXE_GUID >> 32) as u32),
            ])),
        ));
        crate::items::test_spawn_item(
            app.world_mut(),
            AXE_GUID,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, FLURRY_AXE)]),
            false,
        );
        let mut items = app.world_mut().resource_mut::<Items>();
        let mut template = crate::items::test_template("A Bind-On-Use Thing");
        template.bonding = 3;
        template.quality = 3;
        template.inventory_type = 0; // not equippable, so the equip fork above cannot claim it
        if !no_use_spell {
            template.spells = vec![benilla_protocol::messages::ItemSpellEntry {
                index: 0,
                spell_id: 4321,
                trigger: 0, // ON_USE
                charges: 0,
                cooldown_ms: -1,
                category: 0,
                category_cooldown_ms: -1,
            }];
            template.use_spell = Some(benilla_protocol::messages::ItemUseSpell {
                spell_id: 4321,
                cooldown_ms: -1,
                category: 0,
                category_cooldown_ms: -1,
            });
        }
        items.insert_template(FLURRY_AXE, Some(template));

        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        load_ui(&script);
        script.set_container(0, Some(bag_with_the_axe(3)));
        script.run("UseContainerItem(0, 1)").unwrap();
        app.insert_non_send_resource(script);
        run_container_clicks(app.world_mut());
        (app, rx)
    }

    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn right_clicking_a_bind_on_use_item_asks_before_using_it() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = right_click_a_bind_on_use_item(false);
        assert!(
            rx.try_iter().next().is_none(),
            "the deferred use sends nothing"
        );
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(shown(&mut app, "USE_BIND"), "USE_BIND is raised");
        assert_eq!(
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<String>(
                    "return getglobal(StaticPopup_FindVisible(\"USE_BIND\"):GetName() .. \"Text\"):GetText()"
                )
                .unwrap(),
            "Using this item will bind it to you.",
            "the real GlobalStrings USE_NO_DROP"
        );

        // Accept → ConfirmBindOnUse() → the use is re-issued with suppress set.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("StaticPopup_OnClick(StaticPopup_FindVisible(\"USE_BIND\"), 1)")
            .unwrap();
        app.world_mut()
            .run_system_once(drain_bind_on_use_confirms)
            .unwrap();
        assert!(
            rx.try_iter().count() > 0,
            "the accept re-issues the use rather than sending a confirm packet"
        );
    }

    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn a_bind_on_use_item_with_no_on_use_spell_still_asks() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = right_click_a_bind_on_use_item(true);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(
            shown(&mut app, "USE_BIND"),
            "the reference's bind arm sits BELOW the on-use lookup's failure exits, not above them"
        );
        assert!(rx.try_iter().next().is_none());
    }
}

#[cfg(test)]
mod cancel_tests {
    use super::*;
    use crate::ui_bind_confirm::PendingEquip;
    use benilla_protocol::field::{FIELD_PLAYER_INV_SLOT_HEAD, FIELD_PLAYER_PACK_SLOT_1};
    use benilla_protocol::ObjectFields;
    use bevy::ecs::system::RunSystemOnce;

    const ITEM: u64 = 0x4000_0000_0000_0101;
    const SWORD: u64 = 0x4000_0000_0000_0102;

    /// The cancel unlocks the record's occupants as 12340 files them: a swap's two ends, a slotted equip's equip slot in the item's own container, an auto-equip's source.
    #[test]
    fn a_cancel_names_the_records_occupants() {
        let mut world = World::new();
        world.init_resource::<crate::net::GuidIndex>();
        let pair = |f: u16, g: u64| [(f, g as u32), (f + 1, (g >> 32) as u32)];
        let mut fields = pair(FIELD_PLAYER_PACK_SLOT_1, ITEM).to_vec();
        fields.extend(pair(FIELD_PLAYER_INV_SLOT_HEAD + 2 * 15, SWORD));
        world.spawn((SelfPlayer, ObjectStore(ObjectFields::from_pairs(&fields))));
        let occupants = |world: &mut World, rec: PendingEquip| {
            world
                .run_system_once(
                    move |q: Query<&ObjectStore, With<SelfPlayer>>, objects: Objects| {
                        cancel_occupants(q.iter().next(), &objects, &rec)
                    },
                )
                .unwrap()
        };
        let swap = PendingEquip::Swap {
            src_bag: 0,
            src_slot: 1,
            dst_bag: EQUIPMENT_BAG,
            dst_slot: 16,
        };
        assert_eq!(occupants(&mut world, swap), [ITEM, SWORD]);
        let slotted = PendingEquip::Slot {
            bag: 0,
            slot: 1,
            dst: 15,
        };
        assert_eq!(occupants(&mut world, slotted), [SWORD]);
        let auto = PendingEquip::AutoEquip {
            bag_index: BAG_PLAYER_INVENTORY,
            slot: 23,
            guid: ITEM,
        };
        assert_eq!(occupants(&mut world, auto), [ITEM]);
    }
}
