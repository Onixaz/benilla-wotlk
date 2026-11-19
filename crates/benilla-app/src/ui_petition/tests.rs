use super::*;
use benilla_protocol::messages::{PetitionQueryResponse, PetitionRename};
use crossbeam_channel::unbounded;

fn commands() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
    let (tx, rx) = unbounded();
    (NetCommands(tx), rx)
}

fn record(id: u32, owner: u64, name: &str) -> PetitionQueryResponse {
    PetitionQueryResponse {
        petition_id: id,
        owner,
        name: name.into(),
        min_signatures: 9,
        max_signatures: 9,
        ..Default::default()
    }
}

fn cache(p: &mut PetitionState, r: PetitionQueryResponse) {
    p.records.insert(
        r.petition_id,
        Some(Record {
            owner: r.owner,
            name: r.name,
            ..Default::default()
        }),
    );
}

/// The window's store asks a record once per petition id, leaving its callback; a cached record is held at once (`0x5cefe6`).
#[test]
fn the_store_asks_the_record_once_and_holds_a_cached_one() {
    let (commands, rx) = commands();
    let mut p = PetitionState::default();
    p.store(Some(0x99), 7, 0xaa, &commands);
    p.store(Some(0x99), 7, 0xaa, &commands);
    let sent: Vec<_> = rx.try_iter().collect();
    assert!(
        matches!(
            sent.as_slice(),
            [ClientCommand::PetitionQuery {
                petition_id: 7,
                item: 0x99
            }]
        ),
        "{sent:?}"
    );
    assert!(!p.held);
    assert_eq!(
        p.waits,
        [(7, Wait::Open), (7, Wait::Open)],
        "one callback per ask"
    );
    cache(&mut p, record(7, 0xaa, "Legacy"));
    p.store(Some(0x99), 7, 0xaa, &commands);
    assert!(p.held, "a cached record is held at the store");
}

/// The setter's close leg declines only an unsigned petition whose held record another owns (`0x5cef0d`-`0x5cef36`).
#[test]
fn the_close_declines_only_an_unsigned_held_record_of_another() {
    const ME: u64 = 0x1;
    let run = |signed: bool, cached: bool, owner: u64| {
        let (commands, rx) = commands();
        let mut p = PetitionState::default();
        if cached {
            cache(&mut p, record(7, owner, "Legacy"));
        }
        p.store(Some(0x99), 7, owner, &commands);
        p.signed = signed;
        let _ = rx.try_iter().count();
        let closed = p.close_open(ME, &commands);
        assert!(closed && p.petition.is_none() && p.signers.is_empty());
        rx.try_iter()
            .any(|c| matches!(c, ClientCommand::PetitionDecline { item: 0x99 }))
    };
    assert!(
        run(false, true, 0xaa),
        "another's charter, unsigned: declined"
    );
    assert!(!run(true, true, 0xaa), "our sign went out: no decline");
    assert!(!run(false, false, 0xaa), "no record held: no decline");
    assert!(!run(false, true, ME), "our own charter: no decline");
    let (commands, _rx) = commands();
    assert!(
        !PetitionState::default().close_open(ME, &commands),
        "nothing open closes nothing"
    );
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(UiPetitionPlugin);
    app.init_resource::<UiEvents>();
    let (commands, _rx) = commands();
    app.insert_resource(commands);
    app
}

fn dispatch(app: &mut App, ev: benilla_protocol::SessionEvent) {
    crate::net::handlers::dispatch(app.world_mut(), vec![ev]);
}

/// A rename patches the cached record, open or not, and fires `PETITION_SHOW` (`0x6dd33f`); an empty name, an item naming no petition or an uncached record does nothing.
#[test]
fn a_rename_patches_a_cached_record_and_shows() {
    let mut app = app();
    cache(
        &mut app.world_mut().resource_mut::<PetitionState>(),
        record(9, 0xaa, "Old"),
    );
    for name in ["", "New"] {
        dispatch(
            &mut app,
            benilla_protocol::SessionEvent::PetitionRenamed(PetitionRename {
                item: 0x55,
                name: name.into(),
            }),
        );
    }
    let p = app.world().resource::<PetitionState>();
    assert_eq!(
        p.records.get(9).map(|r| r.name.as_str()),
        Some("Old"),
        "an item that is not held names no petition"
    );
}

/// Through the real registration: a new login's VM never runs the old one's `OnHide`.
#[test]
fn the_session_end_closes_the_charter_and_forgets_the_sign() {
    let mut app = app();
    {
        let (commands, _rx) = commands();
        let mut p = app.world_mut().resource_mut::<PetitionState>();
        p.store(Some(0x99), 7, 0xaa, &commands);
        p.signed = true;
    }
    app.world_mut()
        .resource_mut::<GuildRegistrarState>()
        .registrar = Some(0x2a1f);
    dispatch(
        &mut app,
        benilla_protocol::SessionEvent::Disconnected {
            reason: "socket".into(),
            end: benilla_protocol::SessionEnd::Lost,
        },
    );
    assert_eq!(
        app.world().resource::<GuildRegistrarState>().registrar,
        None
    );
    let p = app.world().resource::<PetitionState>();
    assert_eq!(p.petition, None, "no charter carried into the next login");
    assert!(!p.signed && p.waits.is_empty());
}
