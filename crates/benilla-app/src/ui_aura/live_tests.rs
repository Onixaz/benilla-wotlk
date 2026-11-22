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
const DEVOTION_AURA: u32 = 465;

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

/// A book knowing Find Minerals, with its icon.
fn knows_find_minerals(app: &mut App) {
    app.insert_resource(crate::ui_spellbook::SpellBook::with_tracking(vec![
        TrackingSpell {
            spell_id: FIND_MINERALS,
            name: "Find Minerals".into(),
            icon: Some("Interface\\Icons\\Spell_Nature_Earthquake".into()),
            active_icon: None,
        },
    ]));
}

/// Devotion Aura on the stance bar.
fn devotion_on_the_stance_bar(app: &mut App) {
    use crate::ui_action::usable_cache::{SpellUsable, Usable, UsableCaches};
    let mut caches = UsableCaches::default();
    caches.stances.push(SpellUsable {
        spell: DEVOTION_AURA,
        usable: Usable::default(),
    });
    app.insert_resource(caches);
    app.world_mut()
        .non_send_resource::<UiScript>()
        .run(
            r#"local f = CreateFrame("Frame") f:RegisterEvent("UPDATE_SHAPESHIFT_FORM")
                f:SetScript("OnEvent", function(self, event) table.insert(SEEN, event) end)"#,
        )
        .unwrap();
}

/// The engine's tracking global (`0xbeba68`).
fn tracking(app: &mut App) -> u32 {
    app.world().resource::<crate::unit_auras::TrackingSpell>().0
}

/// A tracking aura's add sets the tracking spell in the handler and its removal clears it, each
/// queueing `MINIMAP_UPDATE_TRACKING` for the flush ahead of the packet's `UNIT_AURA`.
#[test]
fn a_tracking_aura_sets_and_clears_the_tracking_spell() {
    let mut app = app();
    knows_find_minerals(&mut app);
    packets(&mut app, vec![auras(ME, vec![(5, aura(FIND_MINERALS, 1))])]);
    assert_eq!(tracking(&mut app), FIND_MINERALS);
    flush(&mut app);
    assert_eq!(seen(&mut app), ["MINIMAP_UPDATE_TRACKING", "player=0"]);
    packets(&mut app, vec![auras(ME, vec![(5, None)])]);
    assert_eq!(tracking(&mut app), 0);
    flush(&mut app);
    assert_eq!(seen(&mut app), ["MINIMAP_UPDATE_TRACKING", "player=0"]);
}

/// A stance-bar aura with no shapeshift effect, a paladin's aura, moves no form byte: its add and
/// removal on us fire `UPDATE_SHAPESHIFT_FORM` in the handler's turn (`0x727835`, `0x722144`); an
/// aura off the stance bar fires nothing.
#[test]
fn a_stance_aura_signals_the_form_where_it_is_added_and_removed() {
    let mut app = app();
    devotion_on_the_stance_bar(&mut app);
    packets(&mut app, vec![auras(ME, vec![(3, aura(DEVOTION_AURA, 1))])]);
    assert_eq!(
        seen(&mut app),
        ["UPDATE_SHAPESHIFT_FORM"],
        "before the flush"
    );
    flush(&mut app);
    assert_eq!(seen(&mut app), ["player=0"]);
    packets(&mut app, vec![auras(ME, vec![(3, None)])]);
    assert_eq!(seen(&mut app), ["UPDATE_SHAPESHIFT_FORM"]);
    flush(&mut app);
    let _ = seen(&mut app);
    packets(&mut app, vec![auras(ME, vec![(4, aura(6673, 1))])]);
    assert!(seen(&mut app).is_empty(), "off the stance bar");
}

/// An `ALL` packet that drops our tracking and stance auras runs the removal for each
/// (`0x722090`): the tracking spell cleared (`0x722113`) and the form signalled (`0x722144`), as an
/// update packet's removal does; one that keeps them re-sets the spell (`0x72f9b5`) and signals no
/// form.
#[test]
fn an_all_packet_that_drops_a_tracking_or_stance_aura_runs_their_removal() {
    let mut app = app();
    knows_find_minerals(&mut app);
    devotion_on_the_stance_bar(&mut app);
    let all = |slots| {
        SessionEvent::AuraUpdate(AuraUpdate {
            unit: ME,
            all: true,
            slots,
        })
    };
    packets(
        &mut app,
        vec![all(vec![
            (3, aura(DEVOTION_AURA, 1)),
            (5, aura(FIND_MINERALS, 1)),
        ])],
    );
    assert_eq!(
        tracking(&mut app),
        FIND_MINERALS,
        "a live tracking aura sets it"
    );
    assert!(
        seen(&mut app).is_empty(),
        "an ALL packet adds through no add site"
    );
    flush(&mut app);
    let _ = seen(&mut app);
    packets(&mut app, vec![all(vec![(7, aura(6673, 1))])]);
    assert_eq!(tracking(&mut app), 0, "the dropped tracking aura clears it");
    assert_eq!(
        seen(&mut app),
        ["UPDATE_SHAPESHIFT_FORM"],
        "the dropped stance aura"
    );
    flush(&mut app);
    assert_eq!(seen(&mut app), ["MINIMAP_UPDATE_TRACKING", "player=0"]);
}

/// The tracking spell is the engine's: an aura packet before any UI stands sets it, and a new UI,
/// a login's or a reload's, reads it at the call (`0xbeba68`).
#[test]
fn the_tracking_spell_outlives_the_ui_and_a_new_one_reads_it() {
    let mut app = app();
    knows_find_minerals(&mut app);
    app.world_mut().remove_non_send_resource::<UiScript>();
    packets(&mut app, vec![auras(ME, vec![(5, aura(FIND_MINERALS, 1))])]);
    assert_eq!(tracking(&mut app), FIND_MINERALS);
    app.insert_non_send_resource(UiScript::new().unwrap());
    let texture = with_live_world(app.world_mut(), |s| {
        s.eval::<String>("return GetTrackingTexture()").unwrap()
    });
    assert_eq!(
        texture.as_deref(),
        Some("Interface\\Icons\\Spell_Nature_Earthquake")
    );
    assert_eq!(
        with_live_world(app.world_mut(), |s| s.tracking_spell()),
        Some(FIND_MINERALS)
    );
}

/// A `RAID` debuff read lists what the book update registered the dispel types for, read through
/// the live view: a Magic debuff for Dispel Magic, not a poison; an unlearn keeps the type, as
/// 12340 only appends at learn (`0x542170`), until the clear at our object's init (`0x6e7f9d`).
#[test]
fn a_raid_debuff_read_follows_the_dispel_types_the_book_registered() {
    let Some(spells) = crate::ui_action::real_spells() else {
        return;
    };
    const DISPEL_MAGIC: u32 = 527;
    const FROSTBOLT: u32 = 42842;
    const DEADLY_POISON: u32 = 57970;
    let mut app = app();
    let update = |app: &mut App, known: &[u32]| {
        let known: std::collections::BTreeSet<u32> = known.iter().copied().collect();
        let spells = app.world().resource::<crate::ui_action::Spells>().clone();
        let mut book = app
            .world_mut()
            .remove_resource::<crate::ui_spellbook::SpellBook>()
            .unwrap_or_default();
        book.update(
            &crate::ui_spellbook::BookInputs {
                known: &known,
                spells: Some(&spells),
                summons: &Default::default(),
                skill_lines: None,
            },
            &mut Default::default(),
            &mut Default::default(),
            false,
        );
        app.insert_resource(book);
        app.world_mut()
            .resource_mut::<crate::ui_action::PlayerActions>()
            .spells = known;
    };
    app.insert_resource(spells)
        .init_resource::<crate::ui_action::PlayerActions>();
    target(&mut app, MOB);
    packets(
        &mut app,
        vec![auras(
            MOB,
            vec![(1, aura(FROSTBOLT, HARM)), (2, aura(DEADLY_POISON, HARM))],
        )],
    );
    let raid = |app: &mut App| -> Vec<u32> {
        with_live_world(app.world_mut(), |s| {
            s.eval::<Vec<u32>>(
                "local t = {} local i = 1 while true do \
                 local id = select(11, UnitDebuff('target', i, 'RAID')) \
                 if not id then return t end t[#t + 1] = id i = i + 1 end",
            )
            .unwrap()
        })
        .expect("a VM")
    };
    // Display order, as the index rebuild sorts them (`0x72c510`).
    assert_eq!(debuffs(&mut app, "target"), [DEADLY_POISON, FROSTBOLT]);
    assert!(
        raid(&mut app).is_empty(),
        "nothing registered, nothing listed"
    );
    update(&mut app, &[DISPEL_MAGIC]);
    assert_eq!(raid(&mut app), [FROSTBOLT], "Magic, not the poison");
    update(&mut app, &[]);
    assert_eq!(raid(&mut app), [FROSTBOLT], "an unlearn keeps the type");
    app.world_mut()
        .resource_mut::<crate::ui_spellbook::SpellBook>()
        .clear_registered();
    update(&mut app, &[]);
    assert!(raid(&mut app).is_empty(), "the init's clear drops it");
}
