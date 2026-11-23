//! The world-enter sends wait for our own player's create, as the reference's do: none goes out on
//! the login edge alone, which fires before the server seats the player; the create sends
//! `CMSG_PLAYED_TIME` (`0x6e6e27`), `CMSG_SET_ACTIVE_MOVER`, then the cascade's `CMSG_QUERY_TIME`,
//! `CMSG_REQUEST_PET_INFO` (with a pet out, while the drain waits for the entry), `MSG_QUERY_NEXT_MAIL_TIME`
//! and the dungeon finder's `CMSG_LFG_GET_STATUS` and `CMSG_LFG_PLAYER_LOCK_INFO_REQUEST` (`0x557240`);
//! a worldport's fresh create sends them all again. `CMSG_BATTLEFIELD_STATUS` waits for the UI's
//! world entry, which this harness lacks (`0x54cc40`, after `0x5281a6`).

use bevy::prelude::*;
use crossbeam_channel::Receiver;

use crate::net::{
    ClientCommand, EnteredWorldMessage, Guid, NetCommands, NetStatus, SelfGuid, ServerWallClock,
    WorldEnterCascadeMessage,
};

const ME: u64 = 0x0000_0000_0000_0042;

/// The self-create edge and the cascade's four senders, in the order the client schedules them
/// (`the_client_schedules_the_sends_in_the_reference_order` pins that), logged in as [`ME`].
fn app() -> (App, Receiver<ClientCommand>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    app.insert_resource(NetCommands(tx))
        .insert_resource(SelfGuid(Some(ME)))
        .insert_resource(NetStatus {
            connected: true,
            last_reason: None,
        })
        .init_resource::<ServerWallClock>()
        .init_resource::<crate::net::PlayedClock>()
        .init_resource::<crate::ui_mail::MailPending>()
        .init_resource::<crate::ui_battlefield::Battlefield>()
        .init_resource::<crate::ui_battlefield::StatusRequestOwed>()
        .init_resource::<crate::ui_battlefield::BattlefieldQueue>()
        .init_resource::<crate::ui_lfg::LfgFeed>()
        .init_resource::<crate::ui_pet::PetBar>()
        .init_resource::<crate::ui_pet::possess::WorldEntryPetStep>()
        .add_message::<EnteredWorldMessage>()
        .add_message::<WorldEnterCascadeMessage>()
        .add_systems(
            Update,
            (
                super::tag_self_player,
                super::enter_world_on_self_create,
                crate::net::send_query_time,
                crate::ui_pet::request_pet_info_on_world_enter,
                crate::ui_mail::send_query_next_mail_time_on_enter,
                crate::ui_battlefield::request_status_on_world_enter,
                crate::ui_lfg::lfg_enter_world,
            )
                .chain(),
        );
    (app, rx)
}

/// The login edge `connected` writes at `CMSG_PLAYER_LOGIN`.
fn log_in(app: &mut App) {
    app.world_mut().write_message(EnteredWorldMessage {
        billing_time_rested: 0,
        tutorial_flags: None,
    });
}

/// What `object_create` spawns for our own create block.
fn self_create(app: &mut App) -> Entity {
    app.world_mut().spawn(Guid(ME)).id()
}

fn sent(rx: &Receiver<ClientCommand>) -> Vec<String> {
    rx.try_iter()
        .map(|c| match c {
            ClientCommand::SetActiveMover { guid } => format!("SetActiveMover {guid:#x}"),
            other => format!("{other:?}"),
        })
        .collect()
}

fn the_sends() -> Vec<String> {
    vec![
        "PlayedTime { show: false }".into(),
        format!("SetActiveMover {ME:#x}"),
        "QueryTime".into(),
        "QueryNextMailTime".into(),
        "Lfg(GetStatus)".into(),
        "Lfg(PlayerLockInfo)".into(),
    ]
}

/// cmangos drops each of them as "the player has not logged in yet" when it beats the load,
/// so the login edge sends none; with no clock answer yet, no re-ask is armed either.
#[test]
fn the_login_edge_alone_sends_nothing() {
    let (mut app, rx) = app();
    log_in(&mut app);
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(sent(&rx), Vec::<String>::new());
}

#[test]
fn the_self_create_sends_them_in_the_reference_order() {
    let (mut app, rx) = app();
    log_in(&mut app);
    app.update();
    self_create(&mut app);
    app.update();
    assert_eq!(sent(&rx), the_sends());
    // The same body on later frames is no new create.
    app.update();
    app.update();
    assert_eq!(sent(&rx), Vec::<String>::new());
}

/// Frames never ask the time: the world entry asks it (`0x5e6fa9`), the log's rebuild re-asks (`0x5e6d79`).
#[test]
fn a_due_reask_waits_for_the_logs_rebuild() {
    let (mut app, rx) = app();
    log_in(&mut app);
    self_create(&mut app);
    app.update();
    assert_eq!(sent(&rx), the_sends());
    app.world_mut()
        .resource_mut::<ServerWallClock>()
        .sample(1_000, 0);
    app.update();
    app.update();
    assert_eq!(sent(&rx), Vec::<String>::new());
}

/// A cross-map worldport purges the streamed world and the server creates us afresh (the reference
/// rebuilds the object manager; the create takes the login's route).
#[test]
fn a_worldport_create_sends_them_again() {
    let (mut app, rx) = app();
    log_in(&mut app);
    let first = self_create(&mut app);
    app.update();
    assert_eq!(sent(&rx), the_sends());

    app.world_mut().despawn(first);
    app.update();
    assert_eq!(sent(&rx), Vec::<String>::new(), "nothing between the two");
    self_create(&mut app);
    app.update();
    assert_eq!(sent(&rx), the_sends());
}

/// The clock lives on the player: each create unseeds it (`0x6e6dfe`), so another character, or the
/// same one after a worldport, reads 0 until its own answer (`0x6cf443`), never the last player's
/// total.
#[test]
fn a_create_unseeds_the_played_clock() {
    let (mut app, _rx) = app();
    log_in(&mut app);
    let first = self_create(&mut app);
    app.update();
    app.world_mut().resource_mut::<crate::net::PlayedClock>().0 =
        Some((360_000, std::time::Instant::now()));
    app.update();
    assert_eq!(
        app.world().resource::<crate::net::PlayedClock>().now(),
        360_000
    );
    app.world_mut().despawn(first);
    self_create(&mut app);
    app.update();
    assert_eq!(app.world().resource::<crate::net::PlayedClock>().now(), 0);
}

/// Our create with a pet out, the drain waiting for the UI's entry: the ask goes out after the time query and before the mail and dungeon finder asks (`0x5281db`, `0x5281fe` -> `0x5d6ed6`, `0x52822b`, `0x528267`), and the step's clear stays owed, so the bar stands for `PLAYER_ENTERING_WORLD`.
#[test]
fn a_create_with_a_pet_out_asks_its_bar_in_the_cascades_order() {
    use benilla_protocol::field::FIELD_UNIT_SUMMON;
    let (mut app, rx) = app();
    app.world_mut()
        .resource_mut::<crate::ui_pet::PetBar>()
        .spells
        .pet_guid = 0x2A;
    app.insert_resource(crate::net::EntryHold(true));
    log_in(&mut app);
    app.world_mut().spawn((
        Guid(ME),
        crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
            FIELD_UNIT_SUMMON,
            0x2A,
        )])),
    ));
    app.update();
    let mut want = the_sends();
    want.insert(3, "RequestPetInfo".into());
    assert_eq!(sent(&rx), want);
    assert!(app.world().resource::<crate::ui_pet::PetBar>().has_bar());
    assert_eq!(
        *app.world()
            .resource::<crate::ui_pet::possess::WorldEntryPetStep>(),
        crate::ui_pet::possess::WorldEntryPetStep {
            owed: true,
            asked: true,
            at_create: None
        }
    );
}

/// With no charm or summon nothing is asked (`0x5d6ea6`), but the setter's clear ahead of that gate (`0x5d6e6d`) is owed all the same.
#[test]
fn a_create_with_no_pet_field_owes_the_clear_but_asks_nothing() {
    let (mut app, rx) = app();
    app.insert_resource(crate::net::EntryHold(true));
    log_in(&mut app);
    self_create(&mut app);
    app.update();
    assert_eq!(sent(&rx), the_sends());
    assert_eq!(
        *app.world()
            .resource::<crate::ui_pet::possess::WorldEntryPetStep>(),
        crate::ui_pet::possess::WorldEntryPetStep {
            owed: true,
            asked: false,
            at_create: None
        }
    );
}

/// A create the drain does not wait on (the in-game UI not yet up, a state 12340's synchronous UI load never reaches) leaves the ask to the step, after its clear (`0x5d6e6d` before `0x5d6ed6`), so an answer can only land on the cleared bar; the step is spent.
#[test]
fn an_unheld_create_asks_after_the_steps_clear() {
    use benilla_protocol::field::FIELD_UNIT_SUMMON;
    let (mut app, rx) = app();
    app.init_resource::<crate::ui_pet::possess::PossessSpell>()
        .init_resource::<crate::ui_script::live::UiEvents>();
    app.world_mut()
        .resource_mut::<crate::ui_pet::PetBar>()
        .spells
        .pet_guid = 0x2A;
    log_in(&mut app);
    app.world_mut().spawn((
        Guid(ME),
        crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
            FIELD_UNIT_SUMMON,
            0x2A,
        )])),
    ));
    app.update();
    assert_eq!(sent(&rx), the_sends());
    crate::ui_pet::possess::world_entry_step(app.world_mut());
    assert_eq!(sent(&rx), vec!["RequestPetInfo".to_string()]);
    assert!(!app.world().resource::<crate::ui_pet::PetBar>().has_bar());
    assert!(
        !app.world()
            .resource::<crate::ui_pet::possess::WorldEntryPetStep>()
            .owed
    );
    // Spent: a second run neither clears nor asks.
    crate::ui_pet::possess::world_entry_step(app.world_mut());
    assert_eq!(sent(&rx), Vec::<String>::new());
}

/// The step after a held create's ask clears and looks up but sends no second ask (`0x5d6e60` runs once per entry).
#[test]
fn the_step_after_a_held_ask_does_not_ask_again() {
    use benilla_protocol::field::FIELD_UNIT_SUMMON;
    let (mut app, rx) = app();
    app.init_resource::<crate::ui_pet::possess::PossessSpell>()
        .init_resource::<crate::ui_script::live::UiEvents>();
    app.insert_resource(crate::net::EntryHold(true));
    log_in(&mut app);
    app.world_mut().spawn((
        Guid(ME),
        crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
            FIELD_UNIT_SUMMON,
            0x2A,
        )])),
    ));
    app.update();
    let _ = sent(&rx);
    crate::ui_pet::possess::world_entry_step(app.world_mut());
    assert_eq!(sent(&rx), Vec::<String>::new());
    assert!(
        !app.world()
            .resource::<crate::ui_pet::possess::WorldEntryPetStep>()
            .owed
    );
}

/// A step that ran inside our create (no UI up to hold the drain for) leaves the cascade only its ask, sent in its order on the gate the create read (`0x5d6ea6`, `0x5d6ed6`), and nothing owed after `PLAYER_ENTERING_WORLD`.
#[test]
fn a_step_run_at_the_create_leaves_only_its_ask() {
    let (mut app, rx) = app();
    app.world_mut()
        .resource_mut::<crate::ui_pet::possess::WorldEntryPetStep>()
        .at_create = Some(true);
    log_in(&mut app);
    self_create(&mut app);
    app.update();
    let mut want = the_sends();
    want.insert(3, "RequestPetInfo".into());
    assert_eq!(sent(&rx), want);
    assert_eq!(
        *app.world()
            .resource::<crate::ui_pet::possess::WorldEntryPetStep>(),
        crate::ui_pet::possess::WorldEntryPetStep {
            owed: false,
            asked: true,
            at_create: None
        }
    );
}

/// The production schedule: the self-create edge, then the time, pet, mail, battlefield, dungeon
/// finder and arena-team sends (the UI's world entry `0x528010` runs them in that order); and a
/// `/reload` frame's sweep drops the stone's text before its cascade resets it.
#[test]
fn the_client_schedules_the_sends_in_the_reference_order() {
    use crate::test_support::runs_before;
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    assert!(runs_before(
        &mut app,
        super::tag_self_player,
        super::enter_world_on_self_create
    ));
    assert!(runs_before(
        &mut app,
        super::enter_world_on_self_create,
        crate::net::send_query_time
    ));
    assert!(runs_before(
        &mut app,
        crate::net::send_query_time,
        crate::ui_pet::request_pet_info_on_world_enter
    ));
    assert!(runs_before(
        &mut app,
        crate::ui_pet::request_pet_info_on_world_enter,
        crate::ui_mail::send_query_next_mail_time_on_enter
    ));
    assert!(runs_before(
        &mut app,
        crate::ui_mail::send_query_next_mail_time_on_enter,
        crate::ui_battlefield::request_status_on_world_enter
    ));
    assert!(runs_before(
        &mut app,
        crate::ui_battlefield::request_status_on_world_enter,
        crate::ui_lfg::lfg_enter_world
    ));
    // The arena-team update's roster asks follow the dungeon finder's (`0x528267`, `0x52826c`).
    assert!(runs_before(
        &mut app,
        crate::ui_lfg::lfg_enter_world,
        crate::ui_arena_team::enter_world
    ));
}
