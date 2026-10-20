//! The charter VM feed and drain: the snapshot the charter windows read, their events, the
//! composed lines, and the sends for the Lua [`PetitionRequest`]s.
//!
//! `PETITION_SHOW` waits for the record and every signer name (`0x4f419b`-`0x4f41ad`), then fires
//! once (`0x4f4320` fires as the pending count reaches zero); the owner's name is not counted
//! (`0x4f4186`/`0x4f42e8`). The getters answer at any time, nil where nothing has landed.

use benilla_protocol::messages::{ArenaBanner, ITEM_FLAG_CHARTER};
use benilla_ui::script::{
    petition_type_name, PetitionRecordView, PetitionRequest, PetitionState as VmPetition,
    PetitionVendorItem, PetitionVendorState as VmVendor, UiScript,
};
use bevy::prelude::*;

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfGuid, SelfPlayer};
use crate::ui_items::{find_carried, find_item, ItemSearch};
use crate::ui_session::{npc_switched, NpcSession};

use super::{lines, GuildRegistrarState, PetitionState, PetitionVendorState};

/// The carried arena charter `TurnInArenaCharter` (`0x6dd680`) finds: a charter-flagged item
/// whose cached record is an arena one, of `team_size` when given.
fn arena_charter(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    petition: &PetitionState,
    team_size: Option<u32>,
    commands: &NetCommands,
) -> Option<u64> {
    find_carried(&store.0, objects, |guid| {
        let Some(f) = objects.object(guid) else {
            return false;
        };
        let charter = f
            .object_entry()
            .and_then(|e| items.template(e, guid, commands))
            .is_some_and(|t| t.flags & ITEM_FLAG_CHARTER != 0);
        charter
            && petition.is_arena_charter(
                f.item_enchant(0).unwrap_or(0) as u32,
                guid,
                team_size,
                commands,
            )
    })
}

/// What the feed last announced, so the events fire on edges rather than every frame.
#[derive(Default)]
pub(super) struct FedPetition {
    registrar: Option<u64>,
    vendor: Option<u64>,
    vendor_items: Vec<PetitionVendorItem>,
    has_filled: bool,
    /// The charter shown last frame; `None` while one is still resolving, so the wait is an edge.
    shown_charter: Option<u64>,
    /// The last pushed view and names, so a change while shown re-fires `PETITION_SHOW`.
    shown: Option<PetitionRecordView>,
    shown_signers: Vec<Option<String>>,
}

/// Build the snapshot, push it, drain the queued lines, and fire the events on their edges.
#[allow(clippy::too_many_arguments)]
pub(super) fn feed_petition(
    script: Option<NonSendMut<UiScript>>,
    registrar: Res<GuildRegistrarState>,
    mut vendor: ResMut<PetitionVendorState>,
    mut petition: ResMut<PetitionState>,
    names: Res<NameCache>,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    objects: Objects,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    self_guid: Res<SelfGuid>,
    commands: Res<NetCommands>,
    mut sink: crate::ui_action::MessageSink,
    mut fed: Local<crate::ui_script::VmMemo<FedPetition>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let fed = fed.get(&script);

    // Shown before the snapshot, so a refusal lands the same frame as its state change; a key the
    // VM's `GlobalStrings.lua` lacks shows nothing, as in the reference.
    let composed = std::mem::take(&mut petition.lines);
    let resolved: Vec<crate::ui_action::Shown> = composed
        .iter()
        .filter_map(|e| {
            crate::ui_action::ui_error_text(e, &|key| {
                script.lua().globals().get::<String>(key).ok()
            })
            .map(|text| crate::ui_action::Shown::keyed(e.key, text))
        })
        .collect();
    crate::ui_action::show_messages(&mut script, &mut sink, "ui_petition", resolved);

    // Rebuilt every frame: reading the caches issues their queries, so a landing sets no flag and
    // only a rebuild sees it.
    let open = petition.open.clone();
    let me = self_guid.0.unwrap_or(0);

    let signers: Vec<Option<String>> = open
        .as_ref()
        .map(|o| {
            o.signers
                .iter()
                .map(|s| names.resolve(*s, &commands).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let record = open.as_ref().and_then(|o| {
        let r = petition.records.get(o.petition_id)?;
        Some(PetitionRecordView {
            petition_type: petition_type_name(r.static_type).to_string(),
            title: r.name.clone(),
            body_text: r.body_text.clone(),
            max_signatures: r.max_signatures,
            // Nil until the name lands, and not waited for.
            originator: names.resolve(o.owner, &commands).map(str::to_string),
            // The record's owner, which the binding compares, not the packet's.
            is_originator: r.owner == me,
            min_signatures: r.min_signatures,
        })
    });

    // `CanSignPetition` (`0x5cf040`). With no record the reference runs only the signer scan.
    let in_guild = self_q
        .iter()
        .next()
        .is_some_and(|s| s.0.player_guild_id() != 0);
    let can_sign = match (open.as_ref(), record.as_ref()) {
        (Some(o), Some(r)) => {
            // A guild or arena charter refuses a full list, and a guild one a guild member.
            let kind = r.petition_type.as_str();
            let full = matches!(kind, "guild" | "arena")
                && (o.signers.len() as i32 >= r.max_signatures || (kind == "guild" && in_guild));
            !full && !r.is_originator && !o.signers.contains(&me)
        }
        // No record: only the signer scan runs.
        (Some(o), None) => !o.signers.contains(&me),
        (None, _) => true,
    };

    script.set_petition(VmPetition {
        charter_cost: registrar.cost(),
        signers: signers.clone(),
        record: record.clone(),
        can_sign,
    });

    // ── The registrar's edges. Switching registrars is a close then an open: `ShowUIPanel`
    //    returns early on a visible frame, so the open sound would not replay.
    let now_registrar = registrar.npc();
    if npc_switched(fed.registrar, now_registrar) {
        script.fire_event("GUILD_REGISTRAR_CLOSED", vec![]);
        script.fire_event("GUILD_REGISTRAR_SHOW", vec![]);
        // Drop the close intent `OnHide` just queued, or the drain would close the new registrar.
        let _ = script.drop_petition_close_intents();
    } else {
        match (fed.registrar, now_registrar) {
            (None, Some(_)) => script.fire_event("GUILD_REGISTRAR_SHOW", vec![]),
            (Some(_), None) => script.fire_event("GUILD_REGISTRAR_CLOSED", vec![]),
            _ => {}
        }
    }
    fed.registrar = now_registrar;

    // ── The petition window's edges, once the record and every signer name have landed.
    let ready = record.is_some() && signers.iter().all(Option::is_some);
    let now_charter = open.as_ref().map(|o| o.item).filter(|_| ready);
    match (fed.shown_charter, now_charter) {
        (Some(a), Some(b)) if a != b => {
            // Another charter while one is shown: drop the queued close intent, or the drain would
            // close the new charter and send a `MSG_PETITION_DECLINE` for it.
            script.fire_event("PETITION_CLOSED", vec![]);
            script.fire_event("PETITION_SHOW", vec![]);
            let _ = script.drop_petition_close_intents();
        }
        // The wait's end: the record landed or the last name resolved; fires once.
        (None, Some(_)) => script.fire_event("PETITION_SHOW", vec![]),
        (Some(_), None) => script.fire_event("PETITION_CLOSED", vec![]),
        // A rename echo, or a name resolving after the window was already up.
        (Some(_), Some(_)) if fed.shown != record || fed.shown_signers != signers => {
            script.fire_event("PETITION_SHOW", vec![])
        }
        _ => {}
    }
    fed.shown_charter = now_charter;
    fed.shown = record;
    fed.shown_signers = signers;

    // ── The petition vendor. `GetPetitionItemInfo` (`0x5a1fb0`) answers once the item cache has
    //    the row, the icon by the showlist's display id.
    let vendor_items: Vec<PetitionVendorItem> = vendor
        .rows()
        .iter()
        .map(|row| {
            let name = items
                .template(row.charter_entry, 0, &commands)
                .map(|t| t.name.clone());
            PetitionVendorItem {
                texture: name.as_ref().and_then(|_| {
                    icons
                        .as_deref()
                        .and_then(|i| i.catalog.get(row.charter_display_id))
                        .and_then(|d| d.icon.clone())
                }),
                name,
                cost: row.charter_cost,
            }
        })
        .collect();
    // `HasFilledPetition` (`0x5a2820`), read only while a vendor is open, its one reader.
    let has_filled = vendor.npc().is_some()
        && self_q.iter().next().is_some_and(|store| {
            arena_charter(store, &objects, &items, &petition, None, &commands).is_some()
        });
    script.set_petition_vendor(VmVendor {
        open: vendor.npc().is_some(),
        items: vendor_items.clone(),
        has_filled_petition: has_filled,
    });
    let now_vendor = vendor.npc();
    if std::mem::take(&mut vendor.update) {
        // `ShowPetitionList` (`0x5a22a0`): the update, then the show, with no close between two.
        script.fire_event("PETITION_VENDOR_UPDATE", vec![]);
        script.fire_event("PETITION_VENDOR_SHOW", vec![]);
    } else if fed.vendor.is_some() && now_vendor.is_none() {
        // `ClosePetitionVendor` fired its own close in the VM.
        if !std::mem::take(&mut vendor.closed_by_vm) {
            script.fire_event("PETITION_VENDOR_CLOSED", vec![]);
        }
    } else if now_vendor.is_some()
        && (fed.vendor_items != vendor_items || (has_filled && !fed.has_filled))
    {
        // The item cache's callback (`0x5a1f30`) and a charter record landing (`0x6dd630`).
        script.fire_event("PETITION_VENDOR_UPDATE", vec![]);
    }
    vendor.closed_by_vm = false;
    fed.vendor = now_vendor;
    fed.vendor_items = vendor_items;
    fed.has_filled = has_filled;
}

/// Turn the Lua charter intents into sends. `ClosePetition` can send `MSG_PETITION_DECLINE`
/// (`0x4f3f60`, `decline_on_close`); `CloseGuildRegistrar` sends nothing.
#[allow(clippy::too_many_arguments)]
pub(super) fn drain_petition(
    script: Option<NonSendMut<UiScript>>,
    mut registrar: ResMut<GuildRegistrarState>,
    mut vendor: ResMut<PetitionVendorState>,
    mut petition: ResMut<PetitionState>,
    items: Res<Items>,
    arena_teams: Res<crate::ui_arena_team::ArenaTeams>,
    names: Res<NameCache>,
    objects: Objects,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    self_guid: Res<SelfGuid>,
    selection: Res<crate::target::Selection>,
    commands: Res<NetCommands>,
    tabard: Res<crate::ui_tabard::TabardOpen>,
    mut errors: ResMut<crate::ui_action::UiErrorKeys>,
) {
    let Some(mut script) = script else {
        return;
    };
    let requests = script.take_petition_requests();
    if requests.is_empty() {
        return;
    }
    let me = self_guid.0.unwrap_or(0);
    for request in requests {
        match request {
            PetitionRequest::Buy(name) => {
                // The registrar's own latch, not the target or `CGGameUI`'s NPC.
                let Some(npc) = registrar.npc() else {
                    debug!("ui_petition: BuyGuildCharter with no registrar open — dropped");
                    continue;
                };
                let index = registrar.index();
                let _ = commands
                    .0
                    .send(ClientCommand::PetitionBuy { npc, name, index });
            }
            PetitionRequest::VendorBuy { index, name } => {
                let (Some(npc), Some(store)) = (vendor.npc(), self_q.iter().next()) else {
                    continue;
                };
                let Some(row) = vendor.rows().get(index as usize).cloned() else {
                    continue;
                };
                if let Some(line) = vendor_buy_refusal(&row, &store.0, &arena_teams) {
                    petition.lines.push(line);
                    continue;
                }
                // A level off 60, 70 and 80 prints its line and buys anyway (`0x5a2460`).
                if let Some(line) = arena_level_line(&row, &store.0) {
                    petition.lines.push(line);
                }
                if store.0.player_money().unwrap_or(0) < row.charter_cost {
                    petition
                        .lines
                        .push(crate::ui_action::UiError::key("ERR_NOT_ENOUGH_MONEY"));
                    continue;
                }
                let _ = commands.0.send(ClientCommand::PetitionBuy {
                    npc,
                    name,
                    index: row.index,
                });
            }
            PetitionRequest::VendorTurnInArena(choice) => {
                let Some(store) = self_q.iter().next() else {
                    continue;
                };
                let charter = arena_charter(
                    store,
                    &objects,
                    &items,
                    &petition,
                    Some(choice.team_size),
                    &commands,
                );
                let Some(item) = charter else {
                    petition.lines.push(lines::no_arena_charter_line());
                    continue;
                };
                let banner = ArenaBanner {
                    background_color: choice.background_color,
                    emblem_style: choice.emblem_style,
                    emblem_color: choice.emblem_color,
                    border_style: choice.border_style,
                    border_color: choice.border_color,
                };
                let _ = commands
                    .0
                    .send(ClientCommand::TurnInArenaPetition { item, banner });
            }
            // The petition button's held item: the cursor drop onto it is not built, so the
            // button is always empty and `TurnInPetition` sends nothing (`0x5a2600`).
            PetitionRequest::VendorTurnIn | PetitionRequest::VendorClickButton => {
                debug!("ui_petition: petition button item not built — {request:?} dropped");
            }
            PetitionRequest::CloseVendor => {
                vendor.close();
                vendor.closed_by_vm = true;
            }
            PetitionRequest::TurnIn => {
                // A fresh bag scan per call (`0x5ef2b0`); the charter's `max_count` is 1.
                let charter = self_q.iter().next().and_then(|store| {
                    find_item(
                        &store.0,
                        &objects,
                        benilla_protocol::messages::CHARTER_ITEM_ENTRY,
                        ItemSearch::default(),
                    )
                });
                match charter {
                    Some((_, _, item)) => {
                        let _ = commands.0.send(ClientCommand::TurnInPetition { item });
                    }
                    // No charter, no packet: a red line (`0x5ef49a`).
                    None => petition.lines.push(lines::no_charter_line()),
                }
            }
            // Sends nothing.
            PetitionRequest::CloseRegistrar => registrar.close(),
            PetitionRequest::TabardInfo => {
                let npc = registrar.npc().unwrap_or(0);
                if let Some(store) = self_q.iter().next() {
                    tabard_vendor_activate(
                        npc,
                        tabard.npc().unwrap_or(0),
                        store,
                        &commands,
                        &mut errors,
                    );
                }
                registrar.close();
            }
            PetitionRequest::ClosePetition => {
                petition.decline_on_close(me, &commands);
                petition.close();
            }
            PetitionRequest::Sign(byte) => {
                let Some(item) = petition.open_item() else {
                    continue;
                };
                let _ = commands.0.send(ClientCommand::PetitionSign { item, byte });
                // The in-flight latch: a close while our signature is outstanding is no decline.
                petition.signing = true;
            }
            PetitionRequest::Offer => {
                let Some(item) = petition.open_item() else {
                    continue; // guard 1, silent
                };
                // `OfferPetition()` offers to the current target; with none it is silent (guard 2).
                let Some(player) = selection.guid else {
                    debug!("ui_petition: OfferPetition with no target — dropped");
                    continue;
                };
                if player == me {
                    // Guard 6; guards 3-5, 7 and 8 are not built, and the server refuses those.
                    petition.lines.push(lines::self_offer_line());
                    continue;
                }
                let team_size = petition.offer_team_size();
                let _ = commands.0.send(ClientCommand::OfferPetition {
                    team_size,
                    item,
                    player,
                });
                // Printed on the send, unconfirmed: the server answers only the target.
                let name = names
                    .resolve(player, &commands)
                    .unwrap_or_default()
                    .to_string();
                petition.lines.push(lines::offered_line(&name));
            }
            PetitionRequest::Rename(name) => {
                let Some(item) = petition.open_item() else {
                    continue;
                };
                let _ = commands
                    .0
                    .send(ClientCommand::PetitionRename { item, name });
            }
            // The client's own name validator refused it; no packet is built.
            PetitionRequest::NameRefused(key) => {
                petition.lines.push(lines::name_refused_line(key));
            }
        }
    }
}

/// `CGPetitionVendorInfo::BuyPetition`'s refusals (`0x5a2350`): a guild charter for a guild
/// member, an arena one for a size the player already holds a team of.
fn vendor_buy_refusal(
    row: &benilla_protocol::messages::PetitionShowListEntry,
    player: &benilla_protocol::ObjectFields,
    teams: &crate::ui_arena_team::ArenaTeams,
) -> Option<lines::Line> {
    use crate::ui_action::UiError;
    match row.charter_type {
        0 if player.player_guild_id() != 0 => Some(UiError::key("ERR_ALREADY_IN_GUILD")),
        1 if teams.holds_team_of_size(row.required_signatures + 1) => {
            Some(UiError::key("ERR_ALREADY_IN_ARENA_TEAM"))
        }
        _ => None,
    }
}

/// An arena charter bought off level 60, 70 or 80: the level it needs, 80 between 71 and 79, else
/// 70 (`0x5a2431`).
fn arena_level_line(
    row: &benilla_protocol::messages::PetitionShowListEntry,
    player: &benilla_protocol::ObjectFields,
) -> Option<lines::Line> {
    let level = player.unit_level().unwrap_or(0);
    if row.charter_type != 1 || matches!(level, 60 | 70 | 80) {
        return None;
    }
    let needed = if (71..80).contains(&level) { 80 } else { 70 };
    Some(crate::ui_action::UiError::args(
        "ERR_ARENA_TEAM_LEVEL_TOO_LOW_I",
        vec![crate::ui_action::FillArg::D(needed)],
    ))
}

/// `0x5e00e0`, the designer's opener, for `GetTabardInfo` with the registrar's guid `npc`
/// (`0x4f5338`): nothing while the designer is open on that guid (`0x5e00e6`-`0x5e00fc`, so two
/// zero guids send nothing), the refusal while the player's `UNIT_FIELD_DISPLAYID` is not its
/// `UNIT_FIELD_NATIVEDISPLAYID` (`0x5e0102`-`0x5e0116`, error `0x130`), else the packet.
fn tabard_vendor_activate(
    npc: u64,
    designer: u64,
    player: &ObjectStore,
    commands: &NetCommands,
    errors: &mut crate::ui_action::UiErrorKeys,
) {
    if designer == npc {
        return;
    }
    if player.0.unit_displayid() != player.0.unit_native_displayid() {
        errors.0.push(crate::ui_action::UiError::key(
            "ERR_EMBLEMERROR_NOTABARDGEOSET",
        ));
        return;
    }
    let _ = commands.0.send(ClientCommand::TabardVendorActivate { npc });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_action::UiErrorKeys;
    use benilla_protocol::ObjectFields;

    /// The player's display ids, `UNIT_FIELD_DISPLAYID` (131) and `UNIT_FIELD_NATIVEDISPLAYID`.
    fn player(display: u32, native: u32) -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[(131, display), (132, native)]))
    }

    fn run(npc: u64, designer: u64, me: &ObjectStore) -> (Vec<ClientCommand>, Vec<&'static str>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut errors = UiErrorKeys::default();
        tabard_vendor_activate(npc, designer, me, &NetCommands(tx), &mut errors);
        (
            rx.try_iter().collect(),
            errors.0.iter().map(|e| e.key).collect(),
        )
    }

    #[test]
    fn get_tabard_info_opens_the_designer_for_the_registrar() {
        let (sent, errors) = run(0x42, 0, &player(49, 49));
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::TabardVendorActivate { npc: 0x42 }]
            ),
            "{sent:?}"
        );
        assert!(errors.is_empty());
    }

    #[test]
    fn get_tabard_info_sends_nothing_to_a_designer_already_open_on_that_npc() {
        let (sent, errors) = run(0x42, 0x42, &player(49, 49));
        assert!(sent.is_empty() && errors.is_empty());
        // With no registrar and no designer both guids are 0: equal, so nothing.
        let (sent, _) = run(0, 0, &player(49, 49));
        assert!(sent.is_empty());
    }

    #[test]
    fn get_tabard_info_refuses_a_shapeshifted_player() {
        let (sent, errors) = run(0x42, 0, &player(2281, 49));
        assert!(sent.is_empty());
        assert_eq!(errors, ["ERR_EMBLEMERROR_NOTABARDGEOSET"]);
    }
}
