//! Characterization of the NPC service windows (#1279): their events fire from the packet handlers in wire order, and their verbs read the `World` at the call.

use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_LEVEL, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::ObjectType;
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};

use super::characterization_tests::{pin, self_values, Rig, Seen, OBJECT_TYPE_PLAYER};
use crate::net::ClientCommand;
use bevy::prelude::{Entity, Transform};

/// The NPC's guid.
const NPC: u64 = 0xF130_0000_0300_0001;
/// A second NPC.
const NPC2: u64 = 0xF130_0000_0300_0002;

/// A creature with `npc_flags` standing at `position`; ours is `[-8949.95, -132.493, 83.5312]`.
fn npc_at(guid: u64, npc_flags: u32, position: [f32; 3]) -> SessionEvent {
    npc_with(guid, npc_flags, position, &[])
}

/// [`npc_at`] with `extra` fields.
fn npc_with(guid: u64, npc_flags: u32, position: [f32; 3], extra: &[(u16, u32)]) -> SessionEvent {
    let mut pairs = vec![
        (2, 0x9),
        (FIELD_UNIT_HEALTH, 500),
        (FIELD_UNIT_MAXHEALTH, 500),
        (FIELD_UNIT_LEVEL, 30),
        (benilla_protocol::field::FIELD_UNIT_NPC_FLAGS, npc_flags),
    ];
    pairs.extend_from_slice(extra);
    SessionEvent::ObjectCreate {
        guid,
        kind: EntityKind::Unit,
        display_id: None,
        position,
        orientation: 0.0,
        scale: 1.0,
        speeds: None,
        mover: None,
        transport_progress: None,
        transport: None,
        spline: None,
        vehicle: None,
        attack_target: None,
        spawned: true,
        fields: ObjectFields::from_pairs(&pairs).into_created(ObjectType::Unit),
    }
}

/// Beside us, inside the 4 yd a zero combat reach leaves.
const NEAR: [f32; 3] = [-8949.0, -132.0, 83.5];
/// 20 yd off.
const FAR: [f32; 3] = [-8929.0, -132.0, 83.5];

/// The commands sent from here on.
fn tap(rig: &mut Rig) -> crossbeam_channel::Receiver<ClientCommand> {
    let (tx, rx) = crossbeam_channel::unbounded();
    rig.app.insert_resource(crate::net::NetCommands(tx));
    rx
}

/// Run `chunk` with the `World` lent, as a click's script call; the error lines it showed, then `returned`.
fn call(rig: &mut Rig, chunk: &str) -> String {
    super::live::with_live_world(rig.app.world_mut(), |s| {
        s.eval::<String>(&format!(
            "local before = #(CHARZ_LOG or {{}}) {chunk} local t = {{}} \
             for i = before + 1, #(CHARZ_LOG or {{}}) do \
               if CHARZ_LOG[i]:find('UI_ERROR_MESSAGE', 1, true) then \
                 t[#t + 1] = CHARZ_LOG[i]:match('\t([^\t]*)\t') end end \
             t[#t + 1] = 'returned' return table.concat(t, '|')"
        ))
    })
    .expect("a VM")
    .expect("the chunk runs")
}

/// `SMSG_BINDER_CONFIRM` runs the binder talk in its handler (`0x6e3db9` -> `0x6d2120`): an innkeeper not held, or out of its reach, latches nothing and asks nothing; one in reach latches and fires `CONFIRM_BINDER`, `CheckBinderDist` reading the latch at the event (`0x6d2290`). The create after the confirm in one frame finds nothing held at the confirm.
#[test]
fn the_binder_confirm_asks_only_for_a_held_innkeeper_in_reach() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let events = &["CONFIRM_BINDER"];
    let verbs = &["binderdist"];
    let seen = rig.packets(
        vec![
            SessionEvent::BinderConfirm { binder: NPC },
            npc_at(NPC, 0x10000, NEAR),
        ],
        2,
    );
    assert!(
        pin(&seen, events, verbs).is_empty(),
        "not held at the confirm"
    );
    assert_eq!(rig.live_now().verbs["binderdist"], "nil");
    let seen = rig.packets(vec![SessionEvent::BinderConfirm { binder: NPC }], 2);
    let pinned = pin(&seen, events, verbs);
    assert_eq!(pinned.len(), 1, "{pinned:?}");
    assert!(pinned[0].ends_with("| binderdist=1"), "{pinned:?}");
    // An innkeeper out of reach asks nothing and keeps the old latch.
    let seen = rig.packets(
        vec![
            npc_at(NPC2, 0x10000, FAR),
            SessionEvent::BinderConfirm { binder: NPC2 },
        ],
        2,
    );
    assert!(pin(&seen, events, verbs).is_empty(), "out of reach");
    assert_eq!(rig.live_now().verbs["binderdist"], "1");
    // The innkeeper leaves: the latch stays, the distance reads it live.
    rig.packets(vec![SessionEvent::ObjectDestroyed(NPC)], 2);
    assert_eq!(rig.live_now().verbs["binderdist"], "nil");
}

/// `ConfirmBinder` is the talk with no guid (`0x51bfc3`): the latched innkeeper in reach sends `CMSG_BINDER_ACTIVATE` inside the call (`0x6d2257`), out of reach nothing; the latch is never cleared, so a second accept sends again.
#[test]
fn confirm_binder_sends_inside_its_call_while_in_reach() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    rig.packets(
        vec![
            npc_at(NPC, 0x10000, NEAR),
            SessionEvent::BinderConfirm { binder: NPC },
        ],
        2,
    );
    let rx = tap(&mut rig);
    let sends = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
        rx.try_iter()
            .filter(|c| matches!(c, ClientCommand::BinderActivate { binder } if *binder == NPC))
            .count()
    };
    call(&mut rig, "ConfirmBinder()");
    assert_eq!(sends(&rx), 1);
    call(&mut rig, "ConfirmBinder()");
    assert_eq!(sends(&rx), 1, "the latch outlives the accept");
    rig.packets(vec![SessionEvent::ObjectDestroyed(NPC)], 2);
    call(&mut rig, "ConfirmBinder()");
    assert_eq!(sends(&rx), 0, "not held");
}

/// `MSG_TALENT_WIPE_CONFIRM`'s handler (`0x6e3d4a`): a zero guid shows `ERR_TALENT_WIPE_ERROR` in its turn (`0x6e3d82`); a trainer in reach latches and fires `CONFIRM_TALENT_WIPE` with the cost (`0x6db28d`), `CheckTalentMasterDist` reading the latch at the event (`0x6d2070`).
#[test]
fn the_talent_wipe_confirm_asks_from_its_handler() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let events = &["CONFIRM_TALENT_WIPE", "UI_ERROR_MESSAGE"];
    let verbs = &["talentdist"];
    let seen = rig.packets(
        vec![
            SessionEvent::TalentWipeConfirm {
                trainer: 0,
                cost: 0,
            },
            npc_at(NPC, 0x10, NEAR),
            SessionEvent::TalentWipeConfirm {
                trainer: NPC,
                cost: 10_000,
            },
        ],
        2,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        [
            "UI_ERROR_MESSAGE You have not spent any talent points. | talentdist=nil",
            "CONFIRM_TALENT_WIPE 10000 | talentdist=1",
        ]
    );
}

/// `ConfirmTalentWipe` is the talk with no guid (`0x51bf84`): our coinage below the latched cost shows `ERR_NOT_ENOUGH_MONEY` inside the call (`0x6db2ae`); at the cost the wipe goes out naming the latch.
#[test]
fn confirm_talent_wipe_refuses_or_sends_inside_its_call() {
    use benilla_protocol::field::FIELD_PLAYER_FIELD_COINAGE;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    rig.packets(
        vec![
            npc_at(NPC, 0x10, NEAR),
            SessionEvent::TalentWipeConfirm {
                trainer: NPC,
                cost: 10_000,
            },
            self_values(&[(FIELD_PLAYER_FIELD_COINAGE, 9_999)]),
        ],
        2,
    );
    let rx = tap(&mut rig);
    let sends = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
        rx.try_iter()
            .filter(
                |c| matches!(c, ClientCommand::TalentWipeConfirm { trainer } if *trainer == NPC),
            )
            .count()
    };
    assert_eq!(
        call(&mut rig, "ConfirmTalentWipe()"),
        "You don't have enough money.|returned"
    );
    assert_eq!(sends(&rx), 0);
    rig.packets(
        vec![self_values(&[(FIELD_PLAYER_FIELD_COINAGE, 10_000)])],
        1,
    );
    assert_eq!(call(&mut rig, "ConfirmTalentWipe()"), "returned");
    assert_eq!(sends(&rx), 1);
}

/// `SMSG_RAID_GROUP_ONLY`: a zero delay shows its reason's line first (`0x6e3be3`), then the boot setter clears or arms the deadline and fires its event in the handler's turn (`0x6e3c10` -> `0x513ad0`), `GetInstanceBootTimeRemaining` reading the deadline just set (`0x5162e0`).
#[test]
fn the_instance_boot_fires_from_its_handler_with_the_deadline_set() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let events = &[
        "INSTANCE_BOOT_START",
        "INSTANCE_BOOT_STOP",
        "UI_ERROR_MESSAGE",
    ];
    let verbs = &["boot"];
    let seen = rig.packets(
        vec![
            SessionEvent::RaidGroupOnly {
                delay_ms: 60_500,
                reason: 0,
            },
            SessionEvent::RaidGroupOnly {
                delay_ms: 0,
                reason: 2,
            },
            SessionEvent::RaidGroupOnly {
                delay_ms: 30_500,
                reason: 0,
            },
        ],
        2,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        [
            "INSTANCE_BOOT_START | boot=60",
            "UI_ERROR_MESSAGE You must be in a raid group to enter this instance. | boot=60",
            "INSTANCE_BOOT_STOP | boot=0",
            "INSTANCE_BOOT_START | boot=30",
        ]
    );
}

/// `SMSG_AREA_SPIRIT_HEALER_TIME` runs the healer poll first (`0x524b10` -> `0x524010`), so a guide streamed in the same frame is adopted before the time is matched; the held healer's positive time arms the wave and fires `AREA_SPIRIT_HEALER_IN_RANGE` with `GetAreaSpiritHealerTime` reading it (`0x516b90`).
#[test]
fn the_spirit_healer_time_polls_first_and_fires_in_range() {
    use benilla_protocol::field::{
        FIELD_PLAYER_FLAGS, FIELD_UNIT_FACTIONTEMPLATE, FIELD_UNIT_FLAGS,
    };
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER, (FIELD_UNIT_FACTIONTEMPLATE, 1)]) else {
        return;
    };
    let guide = || {
        npc_with(
            NPC,
            0x8000,
            NEAR,
            &[(FIELD_UNIT_FACTIONTEMPLATE, 35), (FIELD_UNIT_FLAGS, 0x1000)],
        )
    };
    let events = &[
        "AREA_SPIRIT_HEALER_IN_RANGE",
        "AREA_SPIRIT_HEALER_OUT_OF_RANGE",
    ];
    let verbs = &["spirittime"];
    let time = SessionEvent::AreaSpiritHealerTime {
        healer: NPC,
        ms: 30_500,
    };
    // Alive: the poll holds no healer, so the time arms nothing.
    let seen = rig.packets(vec![guide(), time.clone()], 2);
    assert!(pin(&seen, events, verbs).is_empty(), "{seen:?}");
    // A ghost: the guide, our ghost bit and the time in one frame.
    let seen = rig.packets(
        vec![self_values(&[(FIELD_PLAYER_FLAGS, 0x10)]), time.clone()],
        2,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        ["AREA_SPIRIT_HEALER_IN_RANGE | spirittime=30"]
    );
    // Alive again: the frame's poll drops the healer, its cancel firing while the wave still
    // reads (`0x523eea` before `0x523efa` zeroes it), and the clock reads 0 after.
    let seen = rig.packets(vec![self_values(&[(FIELD_PLAYER_FLAGS, 0)])], 2);
    assert_eq!(
        pin(&seen, events, verbs),
        ["AREA_SPIRIT_HEALER_OUT_OF_RANGE | spirittime=30"]
    );
    assert_eq!(rig.live_now().line(verbs), "NOW | spirittime=0");
}

/// A ghost holding a guide with a 30 s wave armed, the guide at `NEAR`.
fn armed_ghost() -> Option<Rig> {
    use benilla_protocol::field::{
        FIELD_PLAYER_FLAGS, FIELD_UNIT_FACTIONTEMPLATE, FIELD_UNIT_FLAGS,
    };
    let mut rig = Rig::login(&[OBJECT_TYPE_PLAYER, (FIELD_UNIT_FACTIONTEMPLATE, 1)])?;
    let seen = rig.packets(
        vec![
            npc_with(
                NPC,
                0x8000,
                NEAR,
                &[(FIELD_UNIT_FACTIONTEMPLATE, 35), (FIELD_UNIT_FLAGS, 0x1000)],
            ),
            self_values(&[(FIELD_PLAYER_FLAGS, 0x10)]),
            SessionEvent::AreaSpiritHealerTime {
                healer: NPC,
                ms: 30_500,
            },
        ],
        2,
    );
    assert_eq!(
        pin(&seen, &["AREA_SPIRIT_HEALER_IN_RANGE"], &["spirittime"]),
        ["AREA_SPIRIT_HEALER_IN_RANGE | spirittime=30"]
    );
    Some(rig)
}

/// The sends of `rx` by name, the healer's guid as `npc`.
fn spirit_sends(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<String> {
    rx.try_iter()
        .map(|c| match c {
            ClientCommand::CancelAura { spell_id } => format!("cancel {spell_id}"),
            ClientCommand::AreaSpiritHealerQuery { healer } if healer == NPC => "query npc".into(),
            ClientCommand::AreaSpiritHealerQueue { healer } if healer == NPC => "queue npc".into(),
            other => format!("{other:?}"),
        })
        .collect()
}

/// `AcceptAreaSpiritHeal` polls inside its call (`0x524b60` -> `0x524010`): a guide past 22 yd is
/// dropped, the cancel's event firing inside the call while `GetAreaSpiritHealerTime` still reads
/// the wave (`0x523eea` before `0x523efa`), then the aura cancel goes out and, no healer left,
/// no queue.
#[test]
fn accept_area_spirit_heal_drops_a_far_guide_inside_its_call_with_the_wave_read() {
    let Some(mut rig) = armed_ghost() else {
        return;
    };
    let rx = tap(&mut rig);
    // The guide steps off between frames, so only the verb's own poll sees it.
    let world = rig.app.world_mut();
    let guide = world
        .query::<(Entity, &crate::net::Guid)>()
        .iter(world)
        .find(|(_, g)| g.0 == NPC)
        .map(|(e, _)| e)
        .expect("the guide");
    // 30 yd along one horizontal axis: past the 22 yd retain radius.
    world
        .entity_mut(guide)
        .get_mut::<Transform>()
        .expect("the guide stands somewhere")
        .translation
        .x += 30.0;
    assert_eq!(call(&mut rig, "AcceptAreaSpiritHeal()"), "returned");
    assert_eq!(spirit_sends(&rx), ["cancel 2584"]);
    rig.run(1);
    assert_eq!(
        pin(
            &rig.take(),
            &["AREA_SPIRIT_HEALER_OUT_OF_RANGE"],
            &["spirittime"]
        ),
        ["AREA_SPIRIT_HEALER_OUT_OF_RANGE | spirittime=30"]
    );
    assert_eq!(rig.live_now().line(&["spirittime"]), "NOW | spirittime=0");
}

/// In reach, `AcceptAreaSpiritHeal` keeps the guide, fires nothing and queues for its wave.
#[test]
fn accept_area_spirit_heal_queues_for_a_guide_in_reach() {
    let Some(mut rig) = armed_ghost() else {
        return;
    };
    let rx = tap(&mut rig);
    assert_eq!(call(&mut rig, "AcceptAreaSpiritHeal()"), "returned");
    assert_eq!(spirit_sends(&rx), ["queue npc"]);
    rig.run(1);
    assert!(pin(&rig.take(), &["AREA_SPIRIT_HEALER_OUT_OF_RANGE"], &[]).is_empty());
    assert_eq!(rig.live_now().line(&["spirittime"]), "NOW | spirittime=30");
}

/// `CancelAreaSpiritHeal` runs the aura cancel alone (`0x522fa0` -> `0x51f710`): the event inside
/// the call, then the cancel's send, and the wave deadline left as it was.
#[test]
fn cancel_area_spirit_heal_fires_in_its_call_and_keeps_the_wave() {
    let Some(mut rig) = armed_ghost() else {
        return;
    };
    let rx = tap(&mut rig);
    assert_eq!(call(&mut rig, "CancelAreaSpiritHeal()"), "returned");
    assert_eq!(spirit_sends(&rx), ["cancel 2584"]);
    rig.run(1);
    assert_eq!(
        pin(
            &rig.take(),
            &["AREA_SPIRIT_HEALER_OUT_OF_RANGE"],
            &["spirittime"]
        ),
        ["AREA_SPIRIT_HEALER_OUT_OF_RANGE | spirittime=30"]
    );
    assert_eq!(rig.live_now().line(&["spirittime"]), "NOW | spirittime=30");
}

/// Our guild's record as its query answers it: a designed record.
fn guild_record(guild_id: u32, name: &str) -> SessionEvent {
    SessionEvent::GuildQueryResponse(benilla_protocol::messages::GuildQueryResponse {
        guild_id,
        name: name.into(),
        emblem_style: 3,
        emblem_color: 4,
        border_style: 1,
        border_color: 2,
        background_color: 9,
        ..Default::default()
    })
}

/// Another player beside us, in guild `guild_id`.
fn player_in(guid: u64, guild_id: u32) -> SessionEvent {
    SessionEvent::ObjectCreate {
        guid,
        kind: EntityKind::Player,
        display_id: None,
        position: NEAR,
        orientation: 0.0,
        scale: 1.0,
        speeds: None,
        mover: None,
        transport_progress: None,
        transport: None,
        spline: None,
        vehicle: None,
        attack_target: None,
        spawned: false,
        fields: ObjectFields::from_pairs(&[
            (2, 0x19),
            (FIELD_UNIT_HEALTH, 500),
            (FIELD_UNIT_MAXHEALTH, 500),
            (benilla_protocol::field::FIELD_PLAYER_GUILDID, guild_id),
        ])
        .into_created(ObjectType::Player),
    }
}

/// A guild master of guild 7 with the designer's price.
fn guild_master() -> Vec<(u16, u32)> {
    vec![
        OBJECT_TYPE_PLAYER,
        (benilla_protocol::field::FIELD_PLAYER_GUILDID, 7),
        (benilla_protocol::field::FIELD_PLAYER_GUILDRANK, 0),
        (benilla_protocol::field::FIELD_PLAYER_FIELD_COINAGE, 200_000),
    ]
}

/// The events a script call fired inside it, as the observer logged them.
fn fired_in_call(rig: &mut Rig, chunk: &str, events: &[&str], verbs: &[&str]) -> Vec<String> {
    rig.take();
    call(rig, chunk);
    let lines = super::live::with_live_world(rig.app.world_mut(), |s| {
        s.eval::<Vec<String>>("local t = CHARZ_LOG or {}; CHARZ_LOG = {}; return t")
    })
    .expect("a VM")
    .expect("the log reads");
    let seen: Vec<_> = lines.iter().map(|l| Seen::parse(l)).collect();
    pin(&seen, events, verbs)
}

const TABARD_EVENTS: &[&str] = &[
    "OPEN_TABARD_FRAME",
    "CLOSE_TABARD_FRAME",
    "TABARD_SAVE_PENDING",
    "GUILDTABARD_UPDATE",
    "TABARD_CANSAVE_CHANGED",
];

/// `MSG_TABARDVENDOR_ACTIVATE` fires `OPEN_TABARD_FRAME` from its handler (`0x6cc576` -> `0x598e60`), and `CanSaveTabardNow` reads our guild's record at the call (`0x6de1c0`); a record landing fires no `TABARD_CANSAVE_CHANGED` of its own. `Save` sends, latches and fires `TABARD_SAVE_PENDING` inside its call (`0x6db4cc`, `0x6db4d3`); the result clears the latch, drops the record and fires `GUILDTABARD_UPDATE` (`0x6d9264`, `0x6d929b`, `0x6d92a7`); `CloseTabardCreation` fires `CLOSE_TABARD_FRAME` twice inside its call (`0x598ef5`, `0x51300e`).
#[test]
fn the_tabard_designer_reads_its_gate_at_the_call_and_fires_from_its_handlers() {
    let Some(mut rig) = Rig::login(&guild_master()) else {
        return;
    };
    let verbs = &["cansave"];
    // The vendor stands beside us: an unheld one is closed by the range guard.
    rig.packets(vec![npc_at(NPC, 0x80000, NEAR)], 2);
    let seen = rig.packets(vec![SessionEvent::TabardVendorActivate(NPC)], 2);
    assert_eq!(
        pin(&seen, TABARD_EVENTS, verbs),
        vec!["OPEN_TABARD_FRAME | cansave=nil"],
        "no record cached yet, and none asked by the read"
    );
    let seen = rig.packets(vec![guild_record(7, "Probe Guild")], 2);
    assert!(
        pin(&seen, TABARD_EVENTS, verbs).is_empty(),
        "a record's arrival alone fires nothing: {:?}",
        pin(&seen, TABARD_EVENTS, verbs)
    );
    assert_eq!(rig.live_now().verbs["cansave"], "1");
    let rx = tap(&mut rig);
    // The seeded record saved as it is: the same-design check refuses inside the call.
    let lines = call(
        &mut rig,
        "TabardModel:InitializeTabardColors() TabardModel:Save()",
    );
    assert!(
        lines.ends_with("|returned") && lines != "returned",
        "{lines}"
    );
    let pinned = fired_in_call(
        &mut rig,
        "TabardModel:CycleVariation(1, 1) TabardModel:Save()",
        TABARD_EVENTS,
        verbs,
    );
    assert_eq!(pinned, vec!["TABARD_SAVE_PENDING | cansave=nil"]);
    let sent: Vec<_> = rx
        .try_iter()
        .filter_map(|c| match c {
            ClientCommand::SaveGuildEmblem { vendor, design } => Some((vendor, design)),
            _ => None,
        })
        .collect();
    assert_eq!(sent, vec![(NPC, [4, 4, 1, 2, 9])]);
    let seen = rig.packets(vec![SessionEvent::SaveGuildEmblemResult(0)], 2);
    assert_eq!(
        pin(&seen, TABARD_EVENTS, verbs),
        vec!["GUILDTABARD_UPDATE | cansave=nil"],
        "the latch is clear but the record is dropped"
    );
    let pinned = fired_in_call(&mut rig, "CloseTabardCreation()", TABARD_EVENTS, verbs);
    assert_eq!(
        pinned,
        vec![
            "CLOSE_TABARD_FRAME | cansave=nil",
            "CLOSE_TABARD_FRAME | cansave=nil"
        ]
    );
    let pinned = fired_in_call(&mut rig, "CloseTabardCreation()", TABARD_EVENTS, verbs);
    assert!(pinned.is_empty(), "no vendor held: {pinned:?}");
}

/// A failed save's result clears the latch and fires `TABARD_SAVE_PENDING` again (`0x6d92bf`); a code past the table is dropped whole, the latch kept (`0x6d9246`).
#[test]
fn a_failed_save_refires_pending_and_an_unknown_code_keeps_the_latch() {
    let Some(mut rig) = Rig::login(&guild_master()) else {
        return;
    };
    let verbs = &["cansave"];
    rig.packets(
        vec![
            npc_at(NPC, 0x80000, NEAR),
            guild_record(7, "Probe Guild"),
            SessionEvent::TabardVendorActivate(NPC),
        ],
        2,
    );
    let pinned = fired_in_call(
        &mut rig,
        "TabardModel:InitializeTabardColors() TabardModel:CycleVariation(2, 1) TabardModel:Save()",
        TABARD_EVENTS,
        verbs,
    );
    assert_eq!(pinned, vec!["TABARD_SAVE_PENDING | cansave=nil"], "latched");
    let seen = rig.packets(vec![SessionEvent::SaveGuildEmblemResult(6)], 2);
    let pinned = pin(&seen, TABARD_EVENTS, verbs);
    assert!(pinned.is_empty(), "{pinned:?}");
    assert_eq!(rig.live_now().verbs["cansave"], "nil", "still latched");
    let seen = rig.packets(vec![SessionEvent::SaveGuildEmblemResult(3)], 2);
    assert_eq!(
        pin(&seen, TABARD_EVENTS, verbs),
        vec!["TABARD_SAVE_PENDING | cansave=1"]
    );
}

/// `TABARD_CANSAVE_CHANGED` fires from a player's guild change (`0x6df7a0`): another player's create (`0x6e82c0`) and guild id watch (`0x6e1bd9`), our own guild id watch, and a guild timestamp's ask answered, once per visible player in that guild (`0x6e1d5f`, `0x6e1b83`); nothing else fires it.
#[test]
fn the_cansave_event_fires_from_the_guild_change_handlers() {
    let Some(mut rig) = Rig::login(&guild_master()) else {
        return;
    };
    let events = &["TABARD_CANSAVE_CHANGED"];
    const OTHER: u64 = 0x11;
    let seen = rig.packets(vec![player_in(OTHER, 7)], 2);
    assert_eq!(pin(&seen, events, &[]).len(), 1, "the create's init");
    let seen = rig.packets(
        vec![SessionEvent::ObjectValues {
            guid: OTHER,
            fields: ObjectFields::from_pairs(&[(benilla_protocol::field::FIELD_PLAYER_GUILDID, 8)]),
        }],
        2,
    );
    assert_eq!(pin(&seen, events, &[]).len(), 1, "its guild id's watch");
    let seen = rig.packets(
        vec![self_values(&[(
            benilla_protocol::field::FIELD_PLAYER_GUILDID,
            9,
        )])],
        2,
    );
    assert_eq!(pin(&seen, events, &[]).len(), 1, "our guild id's watch");
    let rx = tap(&mut rig);
    let seen = rig.packets(
        vec![SessionEvent::ObjectValues {
            guid: OTHER,
            fields: ObjectFields::from_pairs(&[(
                benilla_protocol::field::FIELD_PLAYER_GUILD_TIMESTAMP,
                1234,
            )]),
        }],
        2,
    );
    assert!(pin(&seen, events, &[]).is_empty(), "the stamp only asks");
    assert!(rx
        .try_iter()
        .any(|c| matches!(c, ClientCommand::GuildQuery { guild_id: 8 })));
    let seen = rig.packets(vec![guild_record(8, "Other Guild")], 2);
    assert_eq!(
        pin(&seen, events, &[]).len(),
        1,
        "the answer's walk: one visible player in guild 8"
    );
    let seen = rig.packets(vec![guild_record(8, "Other Guild")], 2);
    assert!(pin(&seen, events, &[]).is_empty(), "no ask waits on it now");
}
