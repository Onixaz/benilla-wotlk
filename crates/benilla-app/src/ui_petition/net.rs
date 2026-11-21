//! The charter packet handlers: each applies its change and fires its events in its turn.

use benilla_protocol::messages::{petition_result, PetitionShowList};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{PetitionButtonItem, PetitionStep, UiScript};
use bevy::prelude::*;

use super::live::{step, Walk};
use super::{lines, GuildRegistrarState, PetitionState, PetitionVendorState, Record, Wait};
use crate::net::{
    GuidIndex, NetCommands, NetHandlerApp, ObjectStore, Objects, SelfGuid, SelfPlayer,
};
use crate::ui_script::live::{in_turn, UiEvents};

pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::PetitionShowList, on_show_list)
        .net_handler(K::PetitionShowSignatures, on_show_signatures)
        .net_handler(K::PetitionQueryResponse, on_query_response)
        .net_handler(K::PetitionSignResults, on_sign_results)
        .net_handler(K::TurnInPetitionResults, on_turn_in_results)
        .net_handler(K::PetitionDeclined, on_declined)
        .net_handler(K::PetitionRenamed, on_renamed)
        .net_handler(K::PlayerName, on_name_answer)
        .net_handler(K::PlayerNameMiss, on_name_answer)
        .net_handler(K::Disconnected, on_session_end);
}

fn commands(world: &World) -> Option<NetCommands> {
    world
        .get_resource::<NetCommands>()
        .map(|c| NetCommands(c.0.clone()))
}

fn fire(world: &mut World, event: &'static str) {
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.fire(event, vec![]);
    }
}

/// The charter windows end with the session, silently; the asks in flight died with the socket.
fn on_session_end(
    In(_): In<SessionEvent>,
    mut petition: ResMut<PetitionState>,
    mut registrar: ResMut<GuildRegistrarState>,
    mut vendor: ResMut<PetitionVendorState>,
) {
    petition.petition = None;
    petition.signed = false;
    petition.held = false;
    petition.clear_signatures();
    petition.waits.clear();
    registrar.registrar = None;
    vendor.vendor = None;
}

/// `SMSG_PETITION_SHOWLIST` (`0x6d69f0`): an NPC not held as a unit drops the list (`0x6d6aa0`); both flags set the registrar (`0x6d6ab7` -> `0x599e00`), else the vendor's list shows (`0x5a22a0`).
fn on_show_list(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::PetitionShowList(list) = ev else {
        return;
    };
    let flags = world
        .get_resource::<GuidIndex>()
        .and_then(|i| i.0.get(&list.npc).copied())
        .and_then(|e| world.get::<ObjectStore>(e))
        .filter(|s| s.0.is_unit())
        .map(|s| s.0.unit_npc_flags());
    let Some(flags) = flags else {
        debug!(
            "ui_petition: showlist from unheld {:#x} — dropped",
            list.npc
        );
        return;
    };
    if flags & super::REGISTRAR_NPC_FLAGS == super::REGISTRAR_NPC_FLAGS {
        set_registrar(world, list);
    } else {
        show_petition_list(world, list);
    }
}

/// The registrar's setter (`0x599e00`): the interact target taken, the row copied, `GUILD_REGISTRAR_SHOW`.
fn set_registrar(world: &mut World, list: PetitionShowList) {
    let npc = list.npc;
    crate::ui_session::set_interact_target(world, npc);
    let row = list.entries.into_iter().next().unwrap_or_default();
    in_turn(world, move |w| {
        if let Some(mut r) = w.get_resource_mut::<GuildRegistrarState>() {
            r.registrar = Some(npc);
            r.row = row;
        }
        fire(w, "GUILD_REGISTRAR_SHOW");
    });
}

/// The petition list's show (`0x5a22a0`): the interact target taken and the vendor set; with a vendor the button forgets its item and `PETITION_VENDOR_UPDATE` fires (`0x5a22cb`), then the rows, the banner reset and `PETITION_VENDOR_SHOW`.
fn show_petition_list(world: &mut World, list: PetitionShowList) {
    let npc = list.npc;
    crate::ui_session::set_interact_target(world, npc);
    let mut rows = list.entries;
    rows.truncate(super::VENDOR_ROWS);
    in_turn(world, move |w| {
        if let Some(mut v) = w.get_resource_mut::<PetitionVendorState>() {
            v.vendor = (npc != 0).then_some(npc);
        }
        if npc != 0 {
            forget_button(w);
            fire(w, "PETITION_VENDOR_UPDATE");
        }
        // The rows once the update has fired over the last list's (`0x5a22e8`, `0x5a22eb`).
        in_turn(w, move |w| {
            if let Some(mut v) = w.get_resource_mut::<PetitionVendorState>() {
                v.rows = rows;
                v.banner = super::live::unset_banner();
            }
            fire(w, "PETITION_VENDOR_SHOW");
        });
    });
}

/// The button forgets its item without unlocking it (`0x5a22cb`, `0x5a1f57`): the item keeps the pickup's lock until its slot's or count's watch unlocks it (`0x5d9790`, `0x707720`).
pub(super) fn forget_button(world: &mut World) {
    let held = world
        .get_non_send_resource_mut::<UiScript>()
        .and_then(|mut s| s.clear_petition_button());
    if let Some(held) = held {
        if let Err(e) = world.run_system_cached_with(keep_locked, held) {
            warn!("ui_petition: the button's lock was not kept: {e}");
        }
    }
}

/// The forgotten item's lock, while it is still in the slot it was carried from.
pub(super) fn keep_locked(
    In(held): In<PetitionButtonItem>,
    me: Query<&ObjectStore, With<SelfPlayer>>,
    objects: Objects,
    mut pending: ResMut<crate::pending_item_ops::PendingItemOps>,
) {
    let (guid, _) =
        crate::ui_items::slot_guid_count(me.iter().next(), held.bag, held.slot, &objects);
    if guid == held.item {
        pending.lock(guid);
    }
}

/// `SMSG_PETITION_SHOW_SIGNATURES` (`0x6d6d20`): an ignored owner drops it; else the window's setter (`0x5ceef0`), then the signatures (`0x5cf650`).
fn on_show_signatures(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::PetitionShowSignatures(sigs) = ev else {
        return;
    };
    let ignored = world
        .get_resource::<crate::ui_social::SocialState>()
        .is_some_and(|s| s.is_ignored(sigs.owner));
    if ignored {
        debug!(
            "ui_petition: charter from ignored {:#x} — dropped",
            sigs.owner
        );
        return;
    }
    let Some(commands) = commands(world) else {
        return;
    };
    let me = world
        .get_resource::<SelfGuid>()
        .and_then(|g| g.0)
        .unwrap_or(0);
    let closed = world
        .get_resource_mut::<PetitionState>()
        .is_some_and(|mut p| p.close_open(me, &commands));
    // The close's event fires before the new petition is stored (`0x5cefae`).
    if closed {
        fire(world, "PETITION_CLOSED");
    }
    let signers: Vec<u64> = sigs.signatures.iter().map(|s| s.signer).collect();
    in_turn(world, move |w| {
        if let Some(mut p) = w.get_resource_mut::<PetitionState>() {
            p.store(Some(sigs.item), sigs.petition_id, sigs.owner, &commands);
        }
        set_signatures(w, signers, &commands);
    });
}

/// The signer list's store (`0x5cf650`): the signers stored, each name not cached asked with the window's callback; with none pending and a record held, `PETITION_SHOW`.
fn set_signatures(world: &mut World, signers: Vec<u64>, commands: &NetCommands) {
    world.resource_scope(|w, mut p: Mut<PetitionState>| {
        p.clear_signatures();
        if let Some(names) = w.get_resource::<crate::names::NameCache>() {
            for &g in &signers {
                if names.resolve(g, commands).is_none() {
                    p.pending.push(g);
                }
            }
        }
        p.signers = signers;
    });
    let show = world
        .get_resource::<PetitionState>()
        .is_some_and(|p| p.pending.is_empty() && p.held);
    if show {
        fire(world, "PETITION_SHOW");
    }
}

/// A signer name's answer runs the window's callbacks filed for it (`0x5cebf0`): the last one pending, with a record held, fires `PETITION_SHOW`.
fn on_name_answer(
    In(ev): In<SessionEvent>,
    mut petition: ResMut<PetitionState>,
    mut ui: ResMut<UiEvents>,
) {
    use benilla_protocol::messages::NameQueryMiss;
    let guid = match ev {
        SessionEvent::PlayerName { guid, .. } => guid,
        SessionEvent::PlayerNameMiss {
            miss: NameQueryMiss::Retry,
            ..
        } => return,
        SessionEvent::PlayerNameMiss { guid, .. } => guid,
        _ => return,
    };
    if !petition.pending.contains(&guid) {
        return;
    }
    petition.pending.retain(|g| *g != guid);
    if petition.pending.is_empty() && petition.held {
        ui.fire("PETITION_SHOW", vec![]);
    }
}

/// `SMSG_PETITION_QUERY_RESPONSE`: the record cached, then the callbacks waiting on it in ask order: the window's (`0x5cec20` -> `0x5cea60`), the guild turn-in's (`0x6dd360`) and the arena walk's (`0x6dd630`).
fn on_query_response(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::PetitionQueryResponse(r) = ev else {
        return;
    };
    let id = r.petition_id;
    let waits = {
        let Some(mut p) = world.get_resource_mut::<PetitionState>() else {
            return;
        };
        p.records.insert(
            id,
            Some(Record {
                owner: r.owner,
                name: r.name,
                body_text: r.body_text,
                min_signatures: r.min_signatures as i32,
                max_signatures: r.max_signatures as i32,
                static_type: r.static_type,
                min_level: r.allowed_min_level,
                max_level: r.allowed_max_level,
            }),
        );
        let mut taken = Vec::new();
        p.waits.retain(|&(i, w)| {
            let hit = i == id;
            if hit {
                taken.push(w);
            }
            !hit
        });
        taken
    };
    for wait in waits {
        match wait {
            Wait::Open => {
                let show = world
                    .get_resource_mut::<PetitionState>()
                    .is_some_and(|mut p| {
                        if p.petition_id != id {
                            return false;
                        }
                        p.held = true;
                        p.pending.is_empty()
                    });
                if show {
                    fire(world, "PETITION_SHOW");
                }
            }
            Wait::GuildTurnIn => run_walk(world, WalkKind::Guild),
            Wait::ArenaTurnIn => run_walk(world, WalkKind::Arena),
        }
    }
}

#[derive(Clone, Copy)]
enum WalkKind {
    Guild,
    Arena,
}

/// A record callback's walk again, its steps in its turn; the arena one fires `PETITION_VENDOR_UPDATE` on a find (`0x6dd66a`).
fn run_walk(world: &mut World, kind: WalkKind) {
    match world.run_system_cached_with(walk_system, kind) {
        Ok(steps) => super::fire_steps(world, steps),
        Err(e) => warn!("ui_petition: the charter walk did not run: {e}"),
    }
}

fn walk_system(
    In(kind): In<WalkKind>,
    mut petition: ResMut<PetitionState>,
    mut vendor: ResMut<PetitionVendorState>,
    objects: Objects,
    items: Res<crate::items::Items>,
    me: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
) -> Vec<PetitionStep> {
    let Some(me) = me.iter().next() else {
        return Vec::new();
    };
    let walk = Walk {
        me,
        objects: &objects,
        items: &items,
        commands: &commands,
    };
    match kind {
        WalkKind::Guild => walk.turn_in_guild(&mut petition),
        WalkKind::Arena => {
            let turn_in = vendor.turn_in;
            let (found, mut steps) = walk.turn_in_arena(&mut petition, &mut vendor, turn_in);
            if found {
                steps.push(PetitionStep::Fire("PETITION_VENDOR_UPDATE"));
            }
            steps
        }
    }
}

/// `SMSG_PETITION_SIGN_RESULTS` (`0x6cefd0`): another's signature is added (`0x5cf710`); ours shows its line, and on success fires `PETITION_CLOSED` with the window left as it is (`0x6cf047`).
fn on_sign_results(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::PetitionSignResults(results) = ev else {
        return;
    };
    let me = world
        .get_resource::<SelfGuid>()
        .and_then(|g| g.0)
        .unwrap_or(0);
    let mut steps = Vec::new();
    if results.player != me {
        let Some(commands) = commands(world) else {
            return;
        };
        world.resource_scope(|w, mut p: Mut<PetitionState>| {
            if p.signers.contains(&results.player) {
                return;
            }
            p.signers.push(results.player);
            let name = w
                .get_resource::<crate::names::NameCache>()
                .and_then(|n| n.resolve(results.player, &commands).map(str::to_string));
            match name {
                Some(name) => steps.push(step(lines::signed_by_other(&name))),
                None => p.pending.push(results.player),
            }
            if p.pending.is_empty() && p.held {
                steps.push(PetitionStep::Fire("PETITION_SHOW"));
            }
        });
    } else {
        if let Some(line) = lines::my_sign_line(results.result) {
            steps.push(step(line));
        }
        if results.result == petition_result::OK {
            steps.push(PetitionStep::Fire("PETITION_CLOSED"));
        }
    }
    super::fire_steps(world, steps);
}

/// `SMSG_TURN_IN_PETITION_RESULTS` (`0x6cf1a0`): success fires `GUILD_REGISTRAR_CLOSED` alone (`0x6cf1bc`), the refusals show their line.
fn on_turn_in_results(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::TurnInPetitionResults { result } = ev else {
        return;
    };
    if result == petition_result::OK {
        fire(world, "GUILD_REGISTRAR_CLOSED");
    } else if let Some(line) = lines::turn_in_line(result) {
        super::fire_steps(world, vec![step(line)]);
    }
}

/// `MSG_PETITION_DECLINE` inbound (`0x6cf140`): a line only if the decliner's name is already cached.
fn on_declined(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::PetitionDeclined { player } = ev else {
        return;
    };
    let name = world
        .get_resource::<crate::names::NameCache>()
        .and_then(|n| n.peek(player).map(str::to_string));
    if let Some(name) = name {
        super::fire_steps(world, vec![step(lines::declined_line(&name))]);
    }
}

/// `MSG_PETITION_RENAME` inbound (`0x6dd270`): an empty name (`0x6dd292`), an item that is no charter or names no petition, or an uncached record (`0x6dd31d`) does nothing; else the record's name is patched and `PETITION_SHOW` fires (`0x6dd33f`).
fn on_renamed(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::PetitionRenamed(rename) = ev else {
        return;
    };
    if rename.name.is_empty() {
        return;
    }
    let id = world
        .run_system_cached_with(charter_id, rename.item)
        .ok()
        .flatten();
    let patched = id.is_some_and(|id| {
        world
            .get_resource_mut::<PetitionState>()
            .is_some_and(|mut p| match p.records.get_mut(id) {
                Some(r) => {
                    r.name = rename.name.clone();
                    true
                }
                None => false,
            })
    });
    if patched {
        fire(world, "PETITION_SHOW");
    }
}

/// A held item with the charter flag (`0x6dd2d4`) names its petition in enchantment slot 0 (`0x6dd2de`).
fn charter_id(In(item): In<u64>, objects: Objects, items: Res<crate::items::Items>) -> Option<u32> {
    let f = objects.object(item)?;
    let charter = f
        .object_entry()
        .and_then(|e| items.template_cached(e))
        .is_some_and(|t| t.flags & benilla_protocol::messages::ITEM_FLAG_CHARTER != 0);
    let id = f.item_enchant(0).unwrap_or(0) as u32;
    (charter && id != 0).then_some(id)
}
