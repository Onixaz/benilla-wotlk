//! The account data, host side: [`store::AccountData`] opened at the account login and the world
//! entry, fed by the server's times and bytes, its requests and uploads sent through the world
//! writer in the world and through the character park outside it.
//!
//! As 12340: the account scope opens at the world server's authentication
//! (`AccountDataInitialize(true)`, `0x6b1fcd`), the character scope at the world entry
//! (`ClientInitializeGame` `0x4055cf`), and the character scope closes after the logout's flush
//! (`AccountDataDestroy(false)` `0x406665`). The character screen asks for the account's times
//! (`ReadyForAccountDataTimes`, `CMSG_READY_FOR_ACCOUNT_DATA_TIMES`); the server sends the
//! character's after the world entry.
//!
//! The CVar slots 0 and 1 load before the in-game UI (`LoadAccountVariables` `0x518c80`, called in
//! `CGGameUI::InitializeFrameXML`) and save at its shutdown (`SaveGameCVarsCallback` `0x512890`),
//! after which `Config.wtf` is saved, the character's rows dropped and the pending uploads sent
//! (`CGGameUI::Shutdown` `0x5290a0`-`0x5290bb`).

pub(crate) mod store;

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use crate::cvars::{Cvars, Scope};
use crate::net::NetHandlerApp;
pub(crate) use store::{AccountData, SlotDef, Wire};

pub(crate) struct AccountDataPlugin;

impl Plugin for AccountDataPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AccountData>()
            .init_resource::<ActiveCharacter>()
            .net_handler(SessionEventKind::AccountDataTimes, on_times)
            .net_handler(SessionEventKind::AccountDataUpdate, on_update)
            .net_handler(SessionEventKind::WorldAuthenticated, open_account_scope)
            .net_handler(SessionEventKind::Connected, open_character_scope)
            .add_systems(
                Update,
                tick_and_send
                    .in_set(benilla_world::schedule::WorldStage::Net)
                    .after(crate::net::apply_net_updates),
            );
        // A quit from the world never leaves `InWorld`: the shutdown's saves run on the exit
        // edge, after the UI's own exit tail fired `PLAYER_LOGOUT`.
        crate::shutdown::on_app_exit(
            app,
            save_on_exit
                .after(crate::ui_script::shutdown_on_exit)
                .into_configs(),
        );
    }
}

/// The character entering the world, whose slots a server update may name.
#[derive(Resource, Default)]
pub(crate) struct ActiveCharacter(pub(crate) u64);

/// The sync gate as the CVars stand: `synchronizeSettings`, then the slot's own.
pub(crate) fn gate(cvars: &Cvars) -> impl Fn(&SlotDef) -> bool + '_ {
    move |slot: &SlotDef| {
        cvars.flag("synchronizeSettings").unwrap_or(true)
            && slot.gate.is_none_or(|g| cvars.flag(g).unwrap_or(true))
    }
}

/// The world server took the account: its scope opens, waiting for the times, and the account's
/// rows go back to their defaults (`CVar::DeleteAccountCVars` `0x7673a0`).
fn open_account_scope(
    In(ev): In<SessionEvent>,
    intent: Res<crate::login::LoginIntent>,
    mut data: ResMut<AccountData>,
    mut cvars: ResMut<Cvars>,
) {
    if !matches!(ev, SessionEvent::WorldAuthenticated { .. }) {
        return;
    }
    if let Some(account) = intent.account() {
        crate::local_state::set_account(account);
    }
    let dir = crate::local_state::account().and_then(|a| crate::local_state::account_dir(&a));
    data.initialize(true, dir);
    cvars.reset_scope(Scope::Account);
}

/// The world entry: the character's scope opens, under its realm and name.
fn open_character_scope(
    In(ev): In<SessionEvent>,
    roster: Res<crate::char_select::Roster>,
    mut active: ResMut<ActiveCharacter>,
    mut data: ResMut<AccountData>,
) {
    let SessionEvent::Connected {
        self_guid, name, ..
    } = ev
    else {
        return;
    };
    active.0 = self_guid;
    let realm = roster.realm.as_ref().map(|r| r.name.as_str()).unwrap_or("");
    let dir = crate::local_state::account()
        .and_then(|a| crate::local_state::character_dir(&a, realm, &name));
    data.initialize(false, dir);
}

fn on_times(In(ev): In<SessionEvent>, cvars: Res<Cvars>, mut data: ResMut<AccountData>) {
    if let SessionEvent::AccountDataTimes(times) = ev {
        data.on_times(&times, &gate(&cvars));
    }
}

fn on_update(
    In(ev): In<SessionEvent>,
    active: Res<ActiveCharacter>,
    mut data: ResMut<AccountData>,
) {
    if let SessionEvent::AccountDataUpdate(update) = ev {
        data.on_update(&update, active.0);
    }
}

/// The save timers, and what the store queued for the server: through the world writer in the
/// world, else through the character park, which holds the socket there.
fn tick_and_send(
    time: Res<Time>,
    state: Res<State<crate::char_select::ClientState>>,
    net: Res<crate::net::NetCommands>,
    park: Res<crate::net::CharPick>,
    mut data: ResMut<AccountData>,
) {
    data.tick(time.delta_secs());
    if !data.has_outbox() {
        return;
    }
    let in_world = *state.get() == crate::char_select::ClientState::InWorld;
    for wire in data.take_outbox() {
        if in_world {
            let _ = net.0.send(crate::net::ClientCommand::AccountData(wire));
        } else {
            let _ = park.0.send(crate::net::CharRequest::AccountData(wire));
        }
    }
}

/// Whether the UI may load: both scopes have heard from the server, as the reference's loads wait
/// (`LoadAccountData` `0x6b9050` defers while a slot downloads).
pub(crate) fn ready_for_ui(world: &World) -> bool {
    world
        .get_resource::<AccountData>()
        .is_none_or(|d| d.scope_ready(true) && d.scope_ready(false))
}

/// `LoadAccountVariables` (`0x518c80`): the account's rows from slot 0, the character's from slot
/// 1, then the camera from its two rows (`0x5ff3e0`). A slot not ready loads nothing.
pub(crate) fn load_config_slots(world: &mut World) {
    let texts: Vec<(Scope, String)> = {
        let Some(data) = world.get_resource::<AccountData>() else {
            return;
        };
        [
            (store::CONFIG_ACCOUNT, Scope::Account),
            (store::CONFIG_CHARACTER, Scope::Character),
        ]
        .into_iter()
        .filter_map(|(slot, scope)| Some((scope, data.read(slot)?)))
        .map(|(scope, b)| (scope, String::from_utf8_lossy(&b).into_owned()))
        .collect::<Vec<_>>()
    };
    let events = {
        let Some(mut cvars) = world.get_resource_mut::<Cvars>() else {
            return;
        };
        for (scope, text) in &texts {
            cvars.load_scope(*scope, text);
        }
        cvars.take_events()
    };
    for event in events {
        world.trigger(event);
    }
    crate::player::restore_camera_from_cvars(world);
}

/// `SaveAccountData` (`0x6b9be0`) for each `(slot, bytes)`: written now, uploaded per the gate.
pub(crate) fn save_slots(world: &mut World, slots: &[(usize, Vec<u8>)]) {
    let Some(cvars) = world.get_resource::<Cvars>() else {
        return;
    };
    let allowed: Vec<bool> = slots
        .iter()
        .map(|(slot, _)| gate(cvars)(&store::SLOTS[*slot]))
        .collect();
    let Some(mut data) = world.get_resource_mut::<AccountData>() else {
        return;
    };
    for ((slot, bytes), allow) in slots.iter().zip(allowed) {
        data.save(*slot, bytes, &move |_: &SlotDef| allow);
    }
}

/// `CGGameUI::Shutdown`'s writes (`0x528f00`), in its order: the chat cache, the macros, the
/// bindings, the placed frames, then the CVar slots, `Config.wtf` and the flush ([`shutdown`]).
pub(crate) fn save_ui(world: &mut World) {
    crate::ui_chat::settings::save_to_account_data(world);
    crate::ui_macro::save_to_account_data(world);
    crate::bindings::save_to_account_data(world);
    crate::ui_layout::save_to_account_data(world);
    shutdown(world);
}

/// The CVar half of `CGGameUI::Shutdown` (`0x5290a0`-`0x5290bb`): the camera into its rows, the
/// account's and the character's `config-cache.wtf`, `Config.wtf`, the character's rows dropped,
/// and every pending upload sent now (`AccountDataFlush` `0x6b9b90`).
pub(crate) fn shutdown(world: &mut World) {
    crate::player::save_camera_to_cvars(world);
    let bodies = world.get_resource::<Cvars>().map(|c| {
        (
            c.compose_scope(Scope::Account),
            c.compose_scope(Scope::Character),
        )
    });
    if let Some((account, character)) = bodies {
        world.resource_scope(|world, mut data: Mut<AccountData>| {
            let cvars = world.resource::<Cvars>();
            let gate = gate(cvars);
            data.save(store::CONFIG_ACCOUNT, account.as_bytes(), &gate);
            data.save(store::CONFIG_CHARACTER, character.as_bytes(), &gate);
        });
    }
    crate::cvars::save_config_now(world);
    crate::cvars::drop_character_cvars(world);
    if let Some(mut data) = world.get_resource_mut::<AccountData>() {
        data.flush();
    }
}

/// The logout's end: the character's slots close, after [`shutdown`] sent their uploads.
pub(crate) fn release_character(world: &mut World) {
    if let Some(mut data) = world.get_resource_mut::<AccountData>() {
        data.release_character();
    }
    if let Some(mut active) = world.get_resource_mut::<ActiveCharacter>() {
        active.0 = 0;
    }
}

/// The exit edge from the world: the dying VM's last writes folded, then [`shutdown`]; from the
/// glue only `Config.wtf`, which the CVar plugin's own exit save writes.
fn save_on_exit(world: &mut World) {
    let in_ui = world
        .get_resource::<crate::ui_script::AddOnIdentity>()
        .is_some_and(|id| id.0.is_some());
    if !in_ui {
        return;
    }
    crate::cvars::fold_dying_vm_cvars(world);
    save_ui(world);
    // The uploads the flush queued go out before the process ends: its threads are not joined, so
    // the exit waits on a barrier behind them, bounded for a link that is gone.
    let wires = world.resource_mut::<AccountData>().take_outbox();
    if wires.is_empty() {
        return;
    }
    let Some(net) = world.get_resource::<crate::net::NetCommands>() else {
        return;
    };
    for wire in wires {
        let _ = net.0.send(crate::net::ClientCommand::AccountData(wire));
    }
    let (done, written) = crossbeam_channel::bounded(1);
    if net.0.send(crate::net::ClientCommand::Flush(done)).is_ok()
        && written.recv_timeout(EXIT_FLUSH_WAIT).is_err()
    {
        warn!("account data: the exit's uploads were not confirmed written");
    }
}

/// How long the exit waits for its uploads to reach the socket.
const EXIT_FLUSH_WAIT: std::time::Duration = std::time::Duration::from_secs(1);
