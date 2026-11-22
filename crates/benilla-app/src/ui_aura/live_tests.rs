//! The aura verbs read the tables at the call, and `UNIT_AURA` waits for the frame's unique-signal
//! flush (`0x614760`), queued by the aura handler (`0x72fde8`), the party stats handler
//! (`0x6cf808`, `0x6cf941`) and the aura vision watch (`0x727bed`).

use benilla_protocol::field::{FIELD_OBJECT_GUID, FIELD_OBJECT_TYPE};
use benilla_protocol::messages::{
    member_status, AuraRecord, AuraUpdate, GroupMemberEntry, ObjectFields, PartyMemberStatsInfo,
    RecordAura, AURA_UPDATE_FLAG_HARMFUL,
};
use benilla_protocol::SessionEvent;
use benilla_ui::script::{TrackingSpell, UiScript};
use bevy::prelude::*;

use crate::net::{Guid, GuidIndex, NetCommands, ObjectStore, Reputations, SelfGuid, SelfPlayer};
use crate::target::Selection;
use crate::ui_party::GroupState;
use crate::ui_script::live::{flush_unique_signals, with_live_world, UniqueSignals};

const ME: u64 = 0x10;
const MOB: u64 = 0xF130_0000_0000_0001;
const MEMBER: u64 = 0x1234;
const MEMBER_PET: u64 = 0xF140_0000_0000_0077;
const HARM: u8 = AURA_UPDATE_FLAG_HARMFUL | 1;
const FIND_MINERALS: u32 = 2580;

/// Us, a held mob and a party of one, `MEMBER`, out of range: the aura handler, the stats handler
/// and an observer recording each `UNIT_AURA` with the debuffs its handler reads.
fn app() -> App {
    let (tx, _) = crossbeam_channel::unbounded();
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
        .insert_state(crate::char_select::ClientState::InWorld)
        .init_resource::<Selection>()
        .init_resource::<GuidIndex>()
        .init_resource::<Reputations>()
        .init_resource::<GroupState>()
        .insert_resource(SelfGuid(Some(ME)))
        .insert_resource(NetCommands(tx))
        .add_plugins(crate::unit_auras::UnitAurasPlugin);
    // The group's handlers, the stats handler among them.
    crate::ui_party::net::register(&mut app);
    app.insert_non_send_resource(UiScript::new().unwrap());
    app.world_mut()
        .non_send_resource::<UiScript>()
        .run(
            r#"
            SEEN = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_AURA")
            f:RegisterEvent("MINIMAP_UPDATE_TRACKING")
            f:SetScript("OnEvent", function(self, event, unit)
                if event == "UNIT_AURA" then
                    local n = 0
                    -- No catalog here, so no names: count by the spell id, the eleventh return.
                    while select(11, UnitDebuff(unit, n + 1)) do n = n + 1 end
                    table.insert(SEEN, unit .. "=" .. n)
                else
                    table.insert(SEEN, event)
                end
            end)
        "#,
        )
        .unwrap();
    spawn(&mut app, ME, 0x19, true);
    spawn(&mut app, MOB, 0x9, false);
    let member = GroupMemberEntry {
        name: "Brisca".into(),
        guid: MEMBER,
        status: member_status::ONLINE,
        flags: 0,
        roles: 0,
    };
    app.world_mut()
        .resource_mut::<GroupState>()
        .apply_list(0, 0, vec![member], ME, None, Some(ME));
    app
}

/// A held unit of typemask `ty`, ours with the player marker.
fn spawn(app: &mut App, guid: u64, ty: u32, ours: bool) {
    let fields = ObjectFields::from_pairs(&[
        (FIELD_OBJECT_TYPE, ty),
        (FIELD_OBJECT_GUID, guid as u32),
        (FIELD_OBJECT_GUID + 1, (guid >> 32) as u32),
    ]);
    let mut e = app.world_mut().spawn((ObjectStore(fields), Guid(guid)));
    if ours {
        e.insert(SelfPlayer);
    }
    let e = e.id();
    app.world_mut()
        .resource_mut::<GuidIndex>()
        .0
        .insert(guid, e);
}

fn aura(spell_id: u32, flags: u8) -> Option<AuraRecord> {
    Some(AuraRecord {
        spell_id,
        flags,
        level: 80,
        stacks: 1,
        caster: Some(MOB),
        duration_ms: None,
    })
}

fn auras(unit: u64, slots: Vec<(u8, Option<AuraRecord>)>) -> SessionEvent {
    SessionEvent::AuraUpdate(AuraUpdate {
        unit,
        all: false,
        slots,
    })
}

fn stats(guid: u64, info: PartyMemberStatsInfo) -> SessionEvent {
    SessionEvent::PartyMemberStats {
        guid,
        full: false,
        info: Box::new(info),
    }
}

/// A record block of `(slot, spell)` debuffs.
fn block(debuffs: &[(u8, u32)]) -> Option<Vec<RecordAura>> {
    Some(
        debuffs
            .iter()
            .map(|&(slot, spell_id)| RecordAura {
                slot,
                spell_id,
                flags: 0x80,
            })
            .collect(),
    )
}

fn packets(app: &mut App, events: Vec<SessionEvent>) {
    crate::net::handlers::dispatch(app.world_mut(), events);
}

fn flush(app: &mut App) {
    flush_unique_signals(app.world_mut());
}

fn seen(app: &mut App) -> Vec<String> {
    app.world_mut()
        .non_send_resource::<UiScript>()
        .eval::<Vec<String>>("local s = SEEN SEEN = {} return s")
        .unwrap()
}

/// The debuff spell ids `token` lists at the call, with the `World` lent.
fn debuffs(app: &mut App, token: &str) -> Vec<u32> {
    with_live_world(app.world_mut(), |s| s.aura_spell_ids(token, false)).expect("a VM")
}

fn target(app: &mut App, guid: u64) {
    let e = app.world().resource::<GuidIndex>().0[&guid];
    let mut selection = app.world_mut().resource_mut::<Selection>();
    (selection.target, selection.guid) = (Some(e), Some(guid));
}

/// A packet's auras are read at once, before the frame's flush, with nothing fired yet.
#[test]
fn an_aura_packet_is_read_at_once_and_signalled_at_the_flush() {
    let mut app = app();
    target(&mut app, MOB);
    packets(&mut app, vec![auras(MOB, vec![(32, aura(589, HARM))])]);
    assert_eq!(debuffs(&mut app, "target"), [589]);
    assert!(seen(&mut app).is_empty(), "the handler only queues");
    assert_eq!(
        app.world().non_send_resource::<UniqueSignals>().queued(),
        [(MOB, "UNIT_AURA")]
    );
    flush(&mut app);
    assert_eq!(seen(&mut app), ["target=1"]);
    flush(&mut app);
    assert!(seen(&mut app).is_empty(), "the flush empties the queue");
}

/// Two packets for one unit in a frame signal once; the tokens are read at the flush, so a target
/// taken after the packets in the same frame hears it, and a token on no unit hears nothing.
#[test]
fn two_packets_signal_once_for_the_tokens_naming_the_unit_at_the_flush() {
    let mut app = app();
    packets(
        &mut app,
        vec![
            auras(MOB, vec![(32, aura(589, HARM))]),
            auras(MOB, vec![(33, aura(11976, HARM))]),
        ],
    );
    target(&mut app, MOB);
    flush(&mut app);
    assert_eq!(seen(&mut app), ["target=2"]);
    // A packet that moves no slot still signals its unit (`0x72f5d0`).
    packets(&mut app, vec![auras(MOB, Vec::new())]);
    flush(&mut app);
    assert_eq!(seen(&mut app), ["target=2"]);
}

/// Our own packet signals `"player"`, and a unit no token names is queued but heard by nobody.
#[test]
fn our_packet_signals_the_player_and_an_unnamed_unit_nobody() {
    let mut app = app();
    packets(
        &mut app,
        vec![
            auras(ME, vec![(0, aura(6788, HARM))]),
            auras(MOB, vec![(0, aura(589, HARM))]),
        ],
    );
    flush(&mut app);
    assert_eq!(seen(&mut app), ["player=1"]);
}

/// An out-of-range member's aura block moving signals its token at the flush, read off the record;
/// a stats packet that moves no aura signals nothing; the pet's block signals the pet.
#[test]
fn a_members_aura_block_signals_its_token_and_its_pets() {
    let mut app = app();
    let online = PartyMemberStatsInfo {
        status: Some(member_status::ONLINE.into()),
        pet_guid: Some(MEMBER_PET),
        auras: block(&[(0, 589), (1, 770)]),
        ..Default::default()
    };
    packets(&mut app, vec![stats(MEMBER, online)]);
    assert_eq!(debuffs(&mut app, "party1"), [589, 770]);
    flush(&mut app);
    assert_eq!(seen(&mut app), ["party1=2"]);

    let health = PartyMemberStatsInfo {
        cur_hp: Some(7),
        ..Default::default()
    };
    packets(&mut app, vec![stats(MEMBER, health)]);
    flush(&mut app);
    assert!(seen(&mut app).is_empty(), "no aura moved");

    let pet = PartyMemberStatsInfo {
        pet_auras: block(&[(0, 11976)]),
        ..Default::default()
    };
    packets(&mut app, vec![stats(MEMBER, pet)]);
    flush(&mut app);
    assert_eq!(seen(&mut app), ["partypet1=1"]);
}

/// The record walk stops at the first empty slot (`0x614a83`): a hole hides every aura past it.
#[test]
fn the_record_walk_ends_at_the_first_empty_slot() {
    let mut app = app();
    let online = PartyMemberStatsInfo {
        status: Some(member_status::ONLINE.into()),
        auras: block(&[(0, 589), (2, 770)]),
        ..Default::default()
    };
    packets(&mut app, vec![stats(MEMBER, online)]);
    assert_eq!(debuffs(&mut app, "party1"), [589]);
    let fill = PartyMemberStatsInfo {
        auras: block(&[(1, 11976)]),
        ..Default::default()
    };
    packets(&mut app, vec![stats(MEMBER, fill)]);
    assert_eq!(debuffs(&mut app, "party1"), [589, 11976, 770]);
}

/// A tracking aura's add sets the tracking spell in the handler and its removal clears it, each
/// queueing `MINIMAP_UPDATE_TRACKING` for the flush ahead of the packet's `UNIT_AURA`.
#[test]
fn a_tracking_aura_sets_and_clears_the_tracking_spell() {
    let mut app = app();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_tracking_spells(
            vec![TrackingSpell {
                spell_id: FIND_MINERALS,
                name: "Find Minerals".into(),
                icon: None,
                active_icon: None,
            }],
            3,
        );
    let tracking = |app: &mut App| app.world().non_send_resource::<UiScript>().tracking_spell();
    packets(&mut app, vec![auras(ME, vec![(5, aura(FIND_MINERALS, 1))])]);
    assert_eq!(tracking(&mut app), FIND_MINERALS);
    flush(&mut app);
    assert_eq!(seen(&mut app), ["MINIMAP_UPDATE_TRACKING", "player=0"]);
    packets(&mut app, vec![auras(ME, vec![(5, None)])]);
    assert_eq!(tracking(&mut app), 0);
    flush(&mut app);
    assert_eq!(seen(&mut app), ["MINIMAP_UPDATE_TRACKING", "player=0"]);
}
