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
    // A zero guid in `SMSG_BINDER_CONFIRM` is the talk with no guid (`0x6d2137` -> `0x6d2257`): it sends for the latch, no question.
    let seen = rig.packets(vec![SessionEvent::BinderConfirm { binder: 0 }], 2);
    assert!(pin(&seen, &["CONFIRM_BINDER"], &[]).is_empty());
    assert_eq!(sends(&rx), 1, "the zero-guid confirm activates the latch");
    rig.packets(vec![SessionEvent::ObjectDestroyed(NPC)], 2);
    call(&mut rig, "ConfirmBinder()");
    rig.packets(vec![SessionEvent::BinderConfirm { binder: 0 }], 2);
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

/// Our own player's init runs its guild change (`0x6e82c0`) before the active player's init (`0x6e8378`), which signals the whole bar (`0x6e8046`) and then fires its UI world entry (`0x6e8186`); a worldport's fresh create of ours.
#[test]
fn our_create_fires_the_cansave_event_before_the_active_players_init() {
    let Some(mut rig) = Rig::login(&guild_master()) else {
        return;
    };
    let events = &[
        "TABARD_CANSAVE_CHANGED",
        "PLAYER_CONTROL_GAINED",
        "PLAYER_ENTERING_WORLD",
        "ACTIONBAR_SLOT_CHANGED",
        "UPDATE_SHAPESHIFT_FORM",
    ];
    rig.packets(
        vec![SessionEvent::ObjectDestroyed(
            super::characterization_tests::ME,
        )],
        2,
    );
    let seen = rig.packets(
        vec![super::characterization_tests::self_create(&guild_master())],
        3,
    );
    let fired: Vec<_> = seen
        .iter()
        .filter(|s| events.contains(&s.event.as_str()))
        .map(|s| s.event.clone())
        .collect();
    assert_eq!(
        fired,
        [
            "TABARD_CANSAVE_CHANGED",
            "ACTIONBAR_SLOT_CHANGED",
            "PLAYER_ENTERING_WORLD"
        ]
    );
}

/// One values update moving our guild id and its timestamp: the guild id's watch runs first (`0x6e1bb0` before `0x6e1c60`), so `CanSaveTabardNow` reads the new guild's cached record at its `TABARD_CANSAVE_CHANGED`; the stamp's drop follows (`0x6e1d47`).
#[test]
fn our_guild_id_watch_runs_before_the_stamp_drops_the_record() {
    let Some(mut rig) = Rig::login(&guild_master()) else {
        return;
    };
    rig.packets(vec![guild_record(8, "New Guild")], 2);
    let seen = rig.packets(
        vec![self_values(&[
            (benilla_protocol::field::FIELD_PLAYER_GUILDID, 8),
            (benilla_protocol::field::FIELD_PLAYER_GUILD_TIMESTAMP, 77),
        ])],
        2,
    );
    assert_eq!(
        pin(&seen, &["TABARD_CANSAVE_CHANGED"], &["cansave"]),
        ["TABARD_CANSAVE_CHANGED | cansave=1"]
    );
    assert_eq!(
        rig.live_now().verbs["cansave"],
        "nil",
        "the stamp dropped it"
    );
}

/// Our guild id's watch runs the guild-recruitment cascade after its guild change and in the same update (`0x6e1bd9`, `0x6e1beb`): leaving the guild under AUTO with the chat ready signals `UPDATE_CHAT_WINDOWS` after `TABARD_CANSAVE_CHANGED`; another player's watch runs no cascade (`0x6e1be5`).
#[test]
fn our_guild_id_watch_runs_the_recruitment_cascade_after_its_guild_change() {
    let Some(mut rig) = Rig::login(&guild_master()) else {
        return;
    };
    let events = &["TABARD_CANSAVE_CHANGED", "UPDATE_CHAT_WINDOWS"];
    rig.app
        .world_mut()
        .insert_resource(crate::ui_chat::ZoneChannelWalk::ready_chat());
    const OTHER: u64 = 0x11;
    rig.packets(vec![player_in(OTHER, 7)], 2);
    let seen = rig.packets(
        vec![SessionEvent::ObjectValues {
            guid: OTHER,
            fields: ObjectFields::from_pairs(&[(benilla_protocol::field::FIELD_PLAYER_GUILDID, 8)]),
        }],
        2,
    );
    assert_eq!(pin(&seen, events, &[]), ["TABARD_CANSAVE_CHANGED"]);
    // The next packet's watch fires after it: the cascade ran in the update's own turn.
    let seen = rig.packets(
        vec![
            self_values(&[(benilla_protocol::field::FIELD_PLAYER_GUILDID, 0)]),
            self_values(&[(
                benilla_protocol::field::FIELD_PLAYER_WATCHED_FACTION_INDEX,
                3,
            )]),
        ],
        2,
    );
    assert_eq!(
        pin(
            &seen,
            &[
                "TABARD_CANSAVE_CHANGED",
                "UPDATE_CHAT_WINDOWS",
                "UPDATE_FACTION"
            ],
            &[]
        ),
        [
            "TABARD_CANSAVE_CHANGED",
            "UPDATE_CHAT_WINDOWS",
            "UPDATE_FACTION player"
        ]
    );
}

/// The player-flags watch writes the AFK mirror at the packet's step (`0x6e0fe7`, `0x6e1026`): a handler of the next packet's event that sends sees the server's clear, so no `CLEARED_AFK` and no empty AFK packet; a move of `0x8000` alone resyncs a refused optimistic `/afk`, a move outside `0x800e` (resting) does not.
#[test]
fn the_player_flags_watch_writes_the_afk_mirror_at_its_step() {
    use crate::ui_chat::AfkMirror;
    use benilla_protocol::field::{FIELD_PLAYER_FLAGS, FIELD_PLAYER_WATCHED_FACTION_INDEX};

    let Some(mut rig) = Rig::login(&guild_master()) else {
        return;
    };
    rig.packets(vec![self_values(&[(FIELD_PLAYER_FLAGS, 0x2)])], 2);
    assert_eq!(rig.app.world().resource::<AfkMirror>().0, 2);
    call(
        &mut rig,
        "local f = CreateFrame('Frame') f:RegisterEvent('UPDATE_FACTION')          f:SetScript('OnEvent', function() SendChatMessage('hi', 'SAY') end)",
    );
    let rx = tap(&mut rig);
    rig.packets(
        vec![
            self_values(&[(FIELD_PLAYER_FLAGS, 0)]),
            self_values(&[(FIELD_PLAYER_WATCHED_FACTION_INDEX, 3)]),
        ],
        2,
    );
    let kinds: Vec<_> = rx
        .try_iter()
        .filter_map(|c| match c {
            ClientCommand::Chat { kind, .. } => Some(kind),
            _ => None,
        })
        .collect();
    assert_eq!(kinds, [crate::net::ChatKind::Say], "no auto clear");
    // A refused `/afk`: the mirror set with the bit clear; `0x8000` moving alone resyncs it.
    rig.app.world_mut().resource_mut::<AfkMirror>().0 = 1;
    rig.packets(vec![self_values(&[(FIELD_PLAYER_FLAGS, 0x20)])], 2);
    assert_eq!(
        rig.app.world().resource::<AfkMirror>().0,
        1,
        "resting is outside the mask"
    );
    rig.packets(vec![self_values(&[(FIELD_PLAYER_FLAGS, 0x8020)])], 2);
    assert_eq!(rig.app.world().resource::<AfkMirror>().0, 0);
}

/// A stamp moving twice before its guild's answer: the second drop runs the first ask's callback unfound (`0x67a610` -> `0x67a61f` -> `0x67a533`), which the walk skips (`0x6e1b93`), so the answer walks once.
#[test]
fn a_pending_stamp_ask_dropped_with_its_record_never_walks() {
    let Some(mut rig) = Rig::login(&guild_master()) else {
        return;
    };
    const OTHER: u64 = 0x11;
    rig.packets(vec![player_in(OTHER, 8)], 2);
    let stamp = |s| SessionEvent::ObjectValues {
        guid: OTHER,
        fields: ObjectFields::from_pairs(&[(
            benilla_protocol::field::FIELD_PLAYER_GUILD_TIMESTAMP,
            s,
        )]),
    };
    rig.packets(vec![stamp(1)], 2);
    rig.packets(vec![stamp(2)], 2);
    let seen = rig.packets(vec![guild_record(8, "Other Guild")], 2);
    assert_eq!(
        pin(&seen, &["TABARD_CANSAVE_CHANGED"], &[]).len(),
        1,
        "one walk, one visible member"
    );
}

/// The open sets the interact target before `OPEN_TABARD_FRAME` (`0x598e9e`, `0x598eaa`), and `Save` names it at the call (`0x599438`): a save from the event's own handler names the vendor. A second activate on the same vendor clears the held target through the interaction close first (`0x51fd1c` -> `0x51300e`): `CLOSE_TABARD_FRAME`, then `OPEN_TABARD_FRAME`; the closing then fires once, its vendor zeroed by that close while the target holds.
#[test]
fn the_save_names_the_interact_target_the_open_set() {
    let Some(mut rig) = Rig::login(&guild_master()) else {
        return;
    };
    rig.packets(
        vec![npc_at(NPC, 0x80000, NEAR), guild_record(7, "Probe Guild")],
        2,
    );
    call(
        &mut rig,
        "local f = CreateFrame('Frame') f:RegisterEvent('OPEN_TABARD_FRAME') \
         f:SetScript('OnEvent', function() if not CHARZ_SAVED then CHARZ_SAVED = 1 \
         TabardModel:InitializeTabardColors() TabardModel:CycleVariation(1, 1) TabardModel:Save() end end)",
    );
    let rx = tap(&mut rig);
    rig.packets(vec![SessionEvent::TabardVendorActivate(NPC)], 2);
    let sent: Vec<_> = rx
        .try_iter()
        .filter_map(|c| match c {
            ClientCommand::SaveGuildEmblem { vendor, .. } => Some(vendor),
            _ => None,
        })
        .collect();
    assert_eq!(sent, vec![NPC], "the handler's save names the vendor");
    let seen = rig.packets(vec![SessionEvent::TabardVendorActivate(NPC)], 2);
    assert_eq!(
        pin(&seen, TABARD_EVENTS, &[]),
        vec!["CLOSE_TABARD_FRAME", "OPEN_TABARD_FRAME"]
    );
    let pinned = fired_in_call(&mut rig, "CloseTabardCreation()", TABARD_EVENTS, &[]);
    assert!(pinned.is_empty(), "the vendor was zeroed: {pinned:?}");
}

/// `SMSG_ENABLE_BARBER_SHOP` fires `BARBER_SHOP_OPEN` from its handler with the shop open at the event (`0x52fbac`), and a result before it shows its line first (`0x52e5b0`); our hair moving after the open lands on the body and queues `BARBER_SHOP_APPEARANCE_APPLIED` for the frame's flush (`0x6df530` -> `0x52e4d0`), after the open.
#[test]
fn the_barber_events_fire_from_their_handlers_in_wire_order() {
    use benilla_protocol::field::FIELD_PLAYER_BYTES;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let events = &[
        "BARBER_SHOP_OPEN",
        "BARBER_SHOP_APPEARANCE_APPLIED",
        "BARBER_SHOP_SUCCESS",
        "UI_ERROR_MESSAGE",
    ];
    let verbs = &["barberinfo", "barbercost"];
    let seen = rig.packets(
        vec![
            SessionEvent::BarberShopResult(1),
            SessionEvent::EnableBarberShop,
            self_values(&[(FIELD_PLAYER_BYTES, 2 << 16)]),
            SessionEvent::BarberShopResult(0),
        ],
        2,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        [
            "UI_ERROR_MESSAGE You don't have enough money. | barberinfo=0 barbercost=0",
            "BARBER_SHOP_OPEN | barberinfo=4 barbercost=0",
            "BARBER_SHOP_SUCCESS | barberinfo=4 barbercost=0",
            "BARBER_SHOP_APPEARANCE_APPLIED | barberinfo=4 barbercost=0",
        ]
    );
}

/// `CancelBarberShop` closes and fires `BARBER_SHOP_CLOSE` inside its call (`0x52ed80` -> `0x52e9f0`), the shop closed at the event; a second cancel fires nothing. A display change closes from our display id's watch in its turn (`0x716860`).
#[test]
fn the_barber_closes_inside_the_cancel_and_from_the_display_watch() {
    use benilla_protocol::field::FIELD_UNIT_DISPLAYID;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    rig.packets(vec![SessionEvent::EnableBarberShop], 2);
    rig.take();
    let shown = super::live::with_live_world(rig.app.world_mut(), |s| {
        s.eval::<String>(
            "local before = #CHARZ_LOG CancelBarberShop() local t = {} \
             for i = before + 1, #CHARZ_LOG do t[#t + 1] = CHARZ_LOG[i]:match('^[^\t]*') end \
             CancelBarberShop() t[#t + 1] = tostring(#CHARZ_LOG - before) .. ' ' .. select('#', GetBarberShopStyleInfo(2)) \
             return table.concat(t, ' ')",
        )
    })
    .expect("a VM")
    .expect("the chunk runs");
    assert_eq!(shown, "BARBER_SHOP_CLOSE 1 0", "closed in the call, once");
    rig.run(1);
    rig.take();
    let events = &["BARBER_SHOP_OPEN", "BARBER_SHOP_CLOSE", "UI_ERROR_MESSAGE"];
    let verbs = &["barberinfo"];
    let seen = rig.packets(
        vec![
            SessionEvent::EnableBarberShop,
            self_values(&[(FIELD_UNIT_DISPLAYID, 4242)]),
            SessionEvent::BarberShopResult(2),
        ],
        2,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        [
            "BARBER_SHOP_OPEN | barberinfo=4",
            "BARBER_SHOP_CLOSE | barberinfo=0",
            "UI_ERROR_MESSAGE You must be sitting in a barber chair | barberinfo=0",
        ]
    );
}

/// `SMSG_STANDSTATE_UPDATE` reaches the stand-state setter in its handler (`0x73f57a` -> `0x6e2b30`): a chair keeps the shop open, any other new state closes it with `BARBER_SHOP_CLOSE` in the packet's turn, ahead of the next packet's line (`0x6e2bb6`); a sit closes it and stands us up (`0x52ea5f`).
#[test]
fn a_stand_state_update_closes_the_barber_in_its_turn() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let events = &["BARBER_SHOP_OPEN", "BARBER_SHOP_CLOSE", "UI_ERROR_MESSAGE"];
    let verbs = &["barberinfo"];
    let seen = rig.packets(
        vec![
            SessionEvent::EnableBarberShop,
            SessionEvent::StandStateUpdate { state: 5 },
            SessionEvent::BarberShopResult(2),
            SessionEvent::StandStateUpdate { state: 0 },
            SessionEvent::BarberShopResult(2),
        ],
        2,
    );
    let error = "UI_ERROR_MESSAGE You must be sitting in a barber chair";
    assert_eq!(
        pin(&seen, events, verbs),
        [
            "BARBER_SHOP_OPEN | barberinfo=4".to_string(),
            format!("{error} | barberinfo=4"),
            "BARBER_SHOP_CLOSE | barberinfo=0".to_string(),
            format!("{error} | barberinfo=0"),
        ]
    );
    let rx = tap(&mut rig);
    let seen = rig.packets(
        vec![
            SessionEvent::EnableBarberShop,
            SessionEvent::StandStateUpdate { state: 1 },
            SessionEvent::BarberShopResult(2),
        ],
        2,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        [
            "BARBER_SHOP_OPEN | barberinfo=4".to_string(),
            "BARBER_SHOP_CLOSE | barberinfo=0".to_string(),
            format!("{error} | barberinfo=0"),
        ]
    );
    let stands: Vec<u32> = rx
        .try_iter()
        .filter_map(|c| match c {
            ClientCommand::StandStateChange { state } => Some(state),
            _ => None,
        })
        .collect();
    assert_eq!(stands, [0], "the close stood us up from the sit");
}

/// A new map's load closes the barber shop (`0x403ba2`), then its object manager's teardown leaves the UI's world, whose reader part closes the reader (`0x403ba7` -> `0x528ce2` -> `0x58a430`), twice as the reader still holds the interact target (`0x58a207` -> `0x512e60`), all in the worldport's turn ahead of the next packet's line; a login verify and our create again leave the barber open, the reader closed by the watchdog once the purge takes its object.
#[test]
fn a_new_map_closes_the_barber_then_the_reader_in_its_turn() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let _rx = tap(&mut rig);
    rig.packets(plaque(false), 2);
    // Landing beside the plaque, so the reread stays in the GameObject's reach (`0x51fe86`).
    let port = |needs_ack| SessionEvent::Worldport {
        map_id: 1,
        position: NEAR,
        orientation: 0.0,
        needs_ack,
    };
    let events = &["BARBER_SHOP_CLOSE", "ITEM_TEXT_CLOSED", "UI_ERROR_MESSAGE"];
    let verbs = &["barberinfo", "ittext"];
    rig.packets(
        vec![
            SessionEvent::GameObjectPageText { guid: PLAQUE },
            SessionEvent::EnableBarberShop,
        ],
        2,
    );
    let seen = rig.packets(
        vec![
            port(false),
            super::characterization_tests::self_create(&[OBJECT_TYPE_PLAYER]),
            SessionEvent::BarberShopResult(2),
        ],
        2,
    );
    let error = "UI_ERROR_MESSAGE You must be sitting in a barber chair";
    // The verify's own handler closes neither; its purge takes the plaque, so the interact target's watchdog, run by the world frame's update (`0x4fa34a`), clears the reader's hold (`0x51fe2e` -> `0x512ecf`) with our create of the same frame already in the object table (`0x51fe55` -> `0x4038f0`), so the open shop answers; `ui_session::tests::the_watchdog_runs_after_our_create_is_tagged` holds the order.
    assert_eq!(
        pin(&seen, events, verbs),
        [
            "ITEM_TEXT_CLOSED | barberinfo=4 ittext=".to_string(),
            format!("{error} | barberinfo=4 ittext="),
        ],
        "a login verify leaves the barber open"
    );
    let mut reread = plaque(false);
    reread.push(SessionEvent::GameObjectPageText { guid: PLAQUE });
    rig.packets(reread, 2);
    let seen = rig.packets(vec![port(true), SessionEvent::BarberShopResult(2)], 2);
    assert_eq!(
        pin(&seen, events, verbs),
        [
            "BARBER_SHOP_CLOSE | barberinfo=0 ittext=Page one".to_string(),
            "ITEM_TEXT_CLOSED | barberinfo=0 ittext=".to_string(),
            "ITEM_TEXT_CLOSED | barberinfo=0 ittext=".to_string(),
            format!("{error} | barberinfo=0 ittext="),
        ]
    );
}

const GOSSIP_EVENTS: &[&str] = &["GOSSIP_SHOW", "GOSSIP_CLOSED"];

/// `SMSG_GOSSIP_MESSAGE` for `npc` asking text `text_id`, with one option per label.
fn gossip_menu(npc: u64, text_id: u32, labels: &[&str]) -> SessionEvent {
    SessionEvent::GossipMenu(benilla_protocol::messages::GossipMenu {
        npc,
        menu_id: 1,
        text_id,
        options: labels
            .iter()
            .enumerate()
            .map(|(i, l)| benilla_protocol::messages::GossipOption {
                index: i as u32,
                icon: 0,
                coded: false,
                box_money: 0,
                message: (*l).into(),
                box_text: None,
            })
            .collect(),
        quests: Vec::new(),
    })
}

/// `SMSG_NPC_TEXT_UPDATE` answering `text_id` with one male line.
fn npc_text(text_id: u32, line: &str) -> SessionEvent {
    SessionEvent::NpcGreeting {
        text_id,
        blocks: vec![benilla_protocol::messages::NpcTextBlock {
            probability: 1.0,
            male: line.into(),
            ..Default::default()
        }],
    }
}

/// A first-visit menu reads its options at the packet and holds the text draw: nothing fires and the old greeting stands (`0x58ac5e`); the answer draws and fires `GOSSIP_SHOW` bare (`0x58aaf1` -> `0x58aef6`). A cached text draws in the menu's own handler.
#[test]
fn gossip_show_fires_from_the_text_draw_with_the_menu_read_at_the_packet() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let _rx = tap(&mut rig);
    let verbs = &["gossiptext", "gossipopts"];
    rig.packets(vec![npc_at(NPC, 1, NEAR)], 2);
    let seen = rig.packets(vec![gossip_menu(NPC, 7, &["Browse"])], 2);
    assert!(
        pin(&seen, GOSSIP_EVENTS, verbs).is_empty(),
        "the hold fires nothing"
    );
    let now = rig.live_now();
    assert_eq!(
        (
            now.verbs["gossiptext"].as_str(),
            now.verbs["gossipopts"].as_str()
        ),
        ("", "1")
    );
    let seen = rig.packets(vec![npc_text(7, "Well met.")], 2);
    assert_eq!(
        pin(&seen, GOSSIP_EVENTS, verbs),
        ["GOSSIP_SHOW | gossiptext=Well met. gossipopts=1"]
    );
    let seen = rig.packets(vec![gossip_menu(NPC, 7, &["Browse", "Train"])], 2);
    assert_eq!(
        pin(&seen, GOSSIP_EVENTS, verbs),
        ["GOSSIP_SHOW | gossiptext=Well met. gossipopts=2"],
        "the same NPC again closes nothing (`0x58a56a`)"
    );
}

/// `CloseGossip` fires `GOSSIP_CLOSED` twice inside its call while the NPC holds the interact target: the target's clear re-enters the zero set through the interaction close (`0x58a588` -> `0x512ef6`); the globals stay. A second close, and `SMSG_GOSSIP_COMPLETE` with nothing held, fire nothing; the packet on a held NPC runs the same two.
#[test]
fn close_gossip_fires_inside_its_call_through_the_interaction_close() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let _rx = tap(&mut rig);
    let verbs = &["gossiptext", "gossipopts"];
    rig.packets(vec![npc_at(NPC, 1, NEAR)], 2);
    rig.packets(
        vec![gossip_menu(NPC, 7, &["Browse"]), npc_text(7, "Hi.")],
        2,
    );
    let pinned = fired_in_call(&mut rig, "CloseGossip()", GOSSIP_EVENTS, verbs);
    assert_eq!(
        pinned,
        [
            "GOSSIP_CLOSED | gossiptext=Hi. gossipopts=1",
            "GOSSIP_CLOSED | gossiptext=Hi. gossipopts=1"
        ]
    );
    assert!(fired_in_call(&mut rig, "CloseGossip()", GOSSIP_EVENTS, &[]).is_empty());
    let seen = rig.packets(vec![SessionEvent::GossipComplete], 2);
    assert!(pin(&seen, GOSSIP_EVENTS, &[]).is_empty());
    let seen = rig.packets(
        vec![
            gossip_menu(NPC, 7, &["Browse"]),
            SessionEvent::GossipComplete,
        ],
        2,
    );
    assert_eq!(
        pin(&seen, GOSSIP_EVENTS, &[]),
        ["GOSSIP_SHOW", "GOSSIP_CLOSED", "GOSSIP_CLOSED"]
    );
}

/// A menu from another NPC takes the interact target, whose clear closes the held menu once in the walk (`0x58a5b4` -> `0x51fce0` -> `0x512ef6`), then draws: `GOSSIP_CLOSED` before `GOSSIP_SHOW`, in either text order.
#[test]
fn a_menu_from_another_npc_closes_the_held_one_through_the_target() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let _rx = tap(&mut rig);
    let verbs = &["gossiptext"];
    rig.packets(vec![npc_at(NPC, 1, NEAR), npc_at(NPC2, 1, NEAR)], 2);
    rig.packets(vec![gossip_menu(NPC, 7, &["A"]), npc_text(7, "One.")], 2);
    let seen = rig.packets(vec![gossip_menu(NPC2, 8, &["B"]), npc_text(8, "Two.")], 2);
    assert_eq!(
        pin(&seen, GOSSIP_EVENTS, verbs),
        [
            "GOSSIP_CLOSED | gossiptext=One.",
            "GOSSIP_SHOW | gossiptext=Two."
        ]
    );
    // The cached text draws inside the menu's handler, after the close.
    let seen = rig.packets(vec![gossip_menu(NPC, 7, &["A"])], 2);
    assert_eq!(
        pin(&seen, GOSSIP_EVENTS, verbs),
        [
            "GOSSIP_CLOSED | gossiptext=Two.",
            "GOSSIP_SHOW | gossiptext=One."
        ]
    );
}

const ITEM_TEXT_EVENTS: &[&str] = &["ITEM_TEXT_BEGIN", "ITEM_TEXT_READY", "ITEM_TEXT_CLOSED"];

/// A text GameObject (type 9) whose template reads page 100.
const PLAQUE: u64 = 0xF110_0000_0BEE_0001;

/// The plaque's create, its type-9 record on page 100, and pages 100 and 101 unless `uncached`.
fn plaque(uncached: bool) -> Vec<SessionEvent> {
    let mut data = [0i32; 24];
    data[0] = 100;
    let mut events = vec![
        SessionEvent::ObjectCreate {
            guid: PLAQUE,
            kind: EntityKind::GameObject,
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
            spawned: true,
            fields: ObjectFields::from_pairs(&[
                (2, 0x21),
                (benilla_protocol::field::FIELD_OBJECT_ENTRY, 0xBEE),
            ])
            .into_created(ObjectType::GameObject),
        },
        SessionEvent::GameObjectInfo {
            entry: 0xBEE,
            type_id: 9,
            display_id: 0,
            name: "Plaque".into(),
            data,
        },
    ];
    if !uncached {
        events.extend(pages());
    }
    events
}

/// Pages 100 and 101, the chain's end.
fn pages() -> Vec<SessionEvent> {
    vec![
        SessionEvent::PageText {
            page_id: 100,
            text: "Page one".into(),
            next_page_id: 101,
        },
        SessionEvent::PageText {
            page_id: 101,
            text: "Page two".into(),
            next_page_id: 0,
        },
    ]
}

/// `SMSG_GAMEOBJECT_PAGETEXT` runs the read call in its handler (`0x70be73` -> `0x58a1a0`): `ITEM_TEXT_BEGIN`, then the draw's `ITEM_TEXT_READY` with the text set (`0x58a345`, `0x589e90`). `ItemTextNextPage` and `ItemTextPrevPage` step and draw inside their calls (`0x58a187`); `CloseItemText` fires `ITEM_TEXT_CLOSED` twice inside its call, the interact target's clear re-entering the read call (`0x58a207` -> `0x512ecf`).
#[test]
fn the_reader_opens_from_its_handler_and_turns_and_closes_inside_its_calls() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let _rx = tap(&mut rig);
    let verbs = &["ittext", "itpage", "itnext"];
    rig.packets(plaque(false), 2);
    let seen = rig.packets(vec![SessionEvent::GameObjectPageText { guid: PLAQUE }], 2);
    assert_eq!(
        pin(&seen, ITEM_TEXT_EVENTS, verbs),
        [
            "ITEM_TEXT_BEGIN | ittext= itpage=1 itnext=nil",
            "ITEM_TEXT_READY | ittext=Page one itpage=1 itnext=1"
        ]
    );
    assert_eq!(
        fired_in_call(&mut rig, "ItemTextNextPage()", ITEM_TEXT_EVENTS, verbs),
        ["ITEM_TEXT_READY | ittext=Page two itpage=2 itnext=nil"]
    );
    assert!(
        fired_in_call(&mut rig, "ItemTextNextPage()", ITEM_TEXT_EVENTS, verbs).is_empty(),
        "the last page has no next"
    );
    assert_eq!(
        fired_in_call(&mut rig, "ItemTextPrevPage()", ITEM_TEXT_EVENTS, verbs),
        ["ITEM_TEXT_READY | ittext=Page one itpage=1 itnext=1"]
    );
    assert_eq!(
        fired_in_call(&mut rig, "CloseItemText()", ITEM_TEXT_EVENTS, verbs),
        [
            "ITEM_TEXT_CLOSED | ittext= itpage=1 itnext=nil",
            "ITEM_TEXT_CLOSED | ittext= itpage=1 itnext=nil"
        ]
    );
    assert!(
        fired_in_call(&mut rig, "CloseItemText()", ITEM_TEXT_EVENTS, &[]).is_empty(),
        "no reader, nothing to close"
    );
}

/// A page not cached holds the read call: nothing fires until the page's answer runs the reader's callback, which re-runs the read call (`0x58a450`), `ITEM_TEXT_BEGIN` then `ITEM_TEXT_READY` in that handler. The same object again closes it through the interact target, whose leg empties the reader before both `ITEM_TEXT_CLOSED`s (`0x58a1cd` -> `0x512ecf`, `0x58a1de`).
#[test]
fn an_uncached_page_opens_from_its_answer_and_a_reread_closes() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let _rx = tap(&mut rig);
    let verbs = &["ittext"];
    rig.packets(plaque(true), 2);
    let seen = rig.packets(vec![SessionEvent::GameObjectPageText { guid: PLAQUE }], 2);
    assert!(
        pin(&seen, ITEM_TEXT_EVENTS, verbs).is_empty(),
        "held on the page"
    );
    let mut answer = pages();
    answer.push(SessionEvent::GameObjectPageText { guid: PLAQUE });
    let seen = rig.packets(answer, 2);
    assert_eq!(
        pin(&seen, ITEM_TEXT_EVENTS, verbs),
        [
            "ITEM_TEXT_BEGIN | ittext=",
            "ITEM_TEXT_READY | ittext=Page one",
            "ITEM_TEXT_CLOSED | ittext=",
            "ITEM_TEXT_CLOSED | ittext="
        ]
    );
}

/// Only an item's disable has a reader leg (`0x70b036` -> `0x58a1a0`): a read GameObject's destroy leaves the reader open in its handler; the interact target's watchdog then finds the object gone and its clear closes the reader through the walk's reader leg (`0x51fe2e` -> `0x51ff05` -> `0x512ecf`).
#[test]
fn a_read_gameobject_leaving_closes_the_reader_from_the_watchdog() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let _rx = tap(&mut rig);
    rig.packets(plaque(false), 2);
    rig.packets(vec![SessionEvent::GameObjectPageText { guid: PLAQUE }], 2);
    let seen = rig.packets(vec![SessionEvent::ObjectDestroyed(PLAQUE)], 2);
    assert_eq!(
        pin(&seen, ITEM_TEXT_EVENTS, &["ittext"]),
        ["ITEM_TEXT_CLOSED | ittext="],
        "the watchdog's clear alone closes it, once"
    );
    assert_eq!(
        rig.app
            .world()
            .resource::<crate::ui_session::InteractTarget>()
            .0,
        None
    );
}

const MERCHANT_EVENTS: &[&str] = &[
    "MERCHANT_SHOW",
    "MERCHANT_UPDATE",
    "MERCHANT_CLOSED",
    "UI_ERROR_MESSAGE",
    "PLAYER_MONEY",
];

/// A vendor row: `entry` in wire slot `slot` with `count` in stock.
fn vendor_row(entry: u32, slot: u32, count: u32) -> benilla_protocol::messages::VendorItem {
    benilla_protocol::messages::VendorItem {
        slot,
        entry,
        display_id: 0,
        current_count: count,
        price: 10,
        max_durability: 0,
        buy_count: 1,
        extended_cost: 0,
    }
}

/// `vendor`'s `SMSG_LIST_INVENTORY` with `rows`.
fn vendor_list(vendor: u64, rows: Vec<benilla_protocol::messages::VendorItem>) -> SessionEvent {
    SessionEvent::VendorInventory {
        vendor,
        items: rows,
        error: None,
    }
}

/// A Lua expression's value at the call, `tostring`ed.
fn eval(rig: &mut Rig, expr: &str) -> String {
    super::live::with_live_world(rig.app.world_mut(), |s| {
        s.eval::<String>(&format!("return tostring({expr})"))
    })
    .expect("a VM")
    .expect("the chunk runs")
}

/// `SMSG_LIST_INVENTORY` sets the merchant and fires `MERCHANT_SHOW` with no argument (`0x6d9af8` -> `0x584596`); `SMSG_BUY_ITEM` moves the row and fires `MERCHANT_UPDATE` for the open vendor only (`0x6cdd67` -> `0x58411b`); a sold-out refusal zeroes the row with its `MERCHANT_UPDATE` before its line (`0x6cdc06`, `0x6cdc23`); another vendor's list closes the first through the interact target's interaction close, `MERCHANT_CLOSED` before `MERCHANT_SHOW` (`0x584540` -> `0x512f3a`). The rows read at the call.
#[test]
fn the_merchant_fires_from_its_handlers_with_the_rows_read_at_the_call() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let _rx = tap(&mut rig);
    let verbs = &["merchantn"];
    rig.packets(vec![npc_at(NPC, 0x80, NEAR), npc_at(NPC2, 0x80, NEAR)], 2);
    let seen = rig.packets(
        vec![vendor_list(
            NPC,
            vec![vendor_row(11, 1, 3), vendor_row(22, 2, 5)],
        )],
        2,
    );
    // The show reads the vendor the setter just stored (`0x60af67`).
    assert_eq!(
        pin(&seen, MERCHANT_EVENTS, &["merchantn", "npc"]),
        vec![format!("MERCHANT_SHOW | merchantn=2 npc=0x{NPC:016X}")]
    );
    let seen = rig.packets(
        vec![
            SessionEvent::VendorBuyResult {
                vendor: NPC2,
                slot: 2,
                new_count: 1,
                purchase_count: 1,
            },
            SessionEvent::VendorBuyResult {
                vendor: NPC,
                slot: 2,
                new_count: 4,
                purchase_count: 1,
            },
        ],
        2,
    );
    assert_eq!(
        pin(&seen, MERCHANT_EVENTS, verbs),
        vec!["MERCHANT_UPDATE | merchantn=2"],
        "another vendor's count is dropped"
    );
    assert_eq!(eval(&mut rig, "select(5, GetMerchantItemInfo(2))"), "4");
    let seen = rig.packets(
        vec![SessionEvent::VendorBuyFailed {
            vendor: NPC,
            item_entry: 11,
            reason: benilla_protocol::messages::buy_result::ITEM_ALREADY_SOLD,
        }],
        2,
    );
    let pinned = pin(&seen, MERCHANT_EVENTS, &[]);
    assert_eq!(pinned.first().map(String::as_str), Some("MERCHANT_UPDATE"));
    assert!(
        pinned.len() == 2 && pinned[1].starts_with("UI_ERROR_MESSAGE"),
        "{pinned:?}"
    );
    assert_eq!(eval(&mut rig, "select(5, GetMerchantItemInfo(1))"), "0");
    let seen = rig.packets(vec![vendor_list(NPC2, vec![vendor_row(33, 1, 1)])], 2);
    // The setter's walk runs with the target zeroed (`0x51fd1c`, `0x512e89`), then stores the next.
    assert_eq!(
        pin(&seen, MERCHANT_EVENTS, &["merchantn", "npc"]),
        vec![
            "MERCHANT_CLOSED | merchantn=0 npc=nil".to_string(),
            format!("MERCHANT_SHOW | merchantn=1 npc=0x{NPC2:016X}")
        ]
    );
}

/// `CloseMerchant` closes inside its call: the vendor holds the interact target, whose clear runs the interaction close's merchant leg first, so `MERCHANT_CLOSED` fires twice (`0x584614` -> `0x512f3a`, `0x58463e`), with `"npc"` already nobody; a second call finds no vendor.
#[test]
fn close_merchant_fires_twice_inside_its_call() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let _rx = tap(&mut rig);
    let verbs = &["merchantn", "npc", "npcname"];
    rig.packets(vec![npc_at(NPC, 0x80, NEAR)], 2);
    rig.packets(vec![vendor_list(NPC, vec![vendor_row(11, 1, 3)])], 2);
    assert_eq!(
        eval(&mut rig, r#"UnitGUID("npc")"#),
        format!("0x{NPC:016X}"),
        "the vendor holds the token while open"
    );
    // The clear zeroes the target before its walk (`0x512e89`), so the token reads nobody at both, by guid and by every unit getter (`0x60af67`).
    let pinned = fired_in_call(&mut rig, "CloseMerchant()", MERCHANT_EVENTS, verbs);
    assert_eq!(
        pinned,
        vec![
            "MERCHANT_CLOSED | merchantn=0 npc=nil npcname=nil",
            "MERCHANT_CLOSED | merchantn=0 npc=nil npcname=nil"
        ]
    );
    let pinned = fired_in_call(&mut rig, "CloseMerchant()", MERCHANT_EVENTS, verbs);
    assert!(pinned.is_empty(), "{pinned:?}");
}

/// The unit-event fan-out names `"npc"` by the interact target itself (`0x60bece`): a hit on the open vendor fires `UNIT_COMBAT` for `"npc"` as the vendor's show stored it, and none once `CloseMerchant` clears the target; the aura walk holds the vendor's unit for `UnitAura("npc")`.
#[test]
fn a_hit_on_the_open_vendor_fires_unit_combat_for_npc() {
    use crate::ui_unit::UnitCombatFeedback;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let _rx = tap(&mut rig);
    rig.packets(vec![npc_at(NPC, 0x80, NEAR)], 2);
    rig.packets(vec![vendor_list(NPC, vec![vendor_row(11, 1, 3)])], 2);
    let hit = |rig: &mut Rig| {
        rig.take();
        rig.app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<UnitCombatFeedback>>()
            .write(UnitCombatFeedback {
                guid: NPC,
                action: "WOUND",
                flags: "CRITICAL",
                amount: 7,
                school: 1,
            });
        rig.run(1);
        pin(&rig.take(), &["UNIT_COMBAT"], &[])
    };
    assert_eq!(hit(&mut rig), ["UNIT_COMBAT npc WOUND CRITICAL 7 1"]);
    let focus = rig
        .app
        .world()
        .non_send_resource::<benilla_ui::script::UiScript>()
        .focus_guid();
    let walked = rig
        .app
        .world_mut()
        .run_system_cached_with(
            |bevy::prelude::In(focus): bevy::prelude::In<u64>,
             units: crate::ui_unit::TokenView,
             selection: bevy::prelude::Res<crate::target::Selection>,
             interact: bevy::prelude::Res<crate::ui_session::InteractTarget>| {
                units
                    .walk(focus, selection.guid, interact.0.unwrap_or(0))
                    .held
            },
            focus,
        )
        .unwrap();
    assert!(
        walked.contains(&NPC),
        "the walk holds the interact target's unit"
    );
    call(&mut rig, "CloseMerchant()");
    assert!(hit(&mut rig).is_empty(), "no interact target, no \"npc\"");
}

/// `BuyMerchantItem` and `BuybackItem` send inside their call (`0x5854c0`, `0x5855c0`); our coinage's watch fires `PLAYER_MONEY` from the handler with `GetMoney` read at the call (`0x6da091`), and the buyback fields' watch fires `MERCHANT_UPDATE` once per update (`0x585f60`).
#[test]
fn the_merchant_sends_in_its_calls_and_its_watches_fire_from_the_handler() {
    use benilla_protocol::field::{
        FIELD_PLAYER_BUYBACK_PRICE_1, FIELD_PLAYER_BUYBACK_TIMESTAMP_1, FIELD_PLAYER_FIELD_COINAGE,
    };
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let rx = tap(&mut rig);
    rig.packets(vec![npc_at(NPC, 0x80, NEAR)], 2);
    rig.packets(vec![vendor_list(NPC, vec![vendor_row(11, 4, 3)])], 2);
    let seen = rig.packets(vec![self_values(&[(FIELD_PLAYER_FIELD_COINAGE, 500)])], 2);
    assert_eq!(
        pin(&seen, MERCHANT_EVENTS, &["money"]),
        vec!["PLAYER_MONEY | money=500"]
    );
    let seen = rig.packets(
        vec![self_values(&[
            (FIELD_PLAYER_BUYBACK_PRICE_1, 7),
            (FIELD_PLAYER_BUYBACK_TIMESTAMP_1, 9),
        ])],
        2,
    );
    assert_eq!(
        pin(&seen, MERCHANT_EVENTS, &[]),
        vec!["MERCHANT_UPDATE"],
        "one handler over the range"
    );
    while rx.try_recv().is_ok() {}
    call(&mut rig, "BuyMerchantItem(1, 300)");
    let sent: Vec<_> = rx
        .try_iter()
        .filter_map(|c| match c {
            ClientCommand::BuyItem {
                vendor,
                entry,
                slot,
                count,
            } => Some((vendor, entry, slot, count)),
            _ => None,
        })
        .collect();
    assert_eq!(sent, vec![(NPC, 11, 4, 255)], "sent inside the call");
}

const TAXI_EVENTS: &[&str] = &[
    "TAXIMAP_OPENED",
    "TAXIMAP_CLOSED",
    "GOSSIP_CLOSED",
    "UI_ERROR_MESSAGE",
    "UI_INFO_MESSAGE",
];

/// `SMSG_SHOWTAXINODES` for `master` at Stormwind (2) knowing Sentinel Hill (4), Ironforge (6) and Thelsamar (8).
fn stormwind_nodes(master: u64) -> SessionEvent {
    let mut known_mask = benilla_protocol::messages::TaxiMask::default();
    for id in [2, 4, 6, 8] {
        known_mask.set(id);
    }
    SessionEvent::TaxiNodesShown {
        flightmaster: master,
        nearest_node: 2,
        known_mask,
    }
}

/// The 1-based slot of the node named `name`, read at the call.
fn taxi_slot(rig: &mut Rig, name: &str) -> String {
    super::live::with_live_world(rig.app.world_mut(), |s| {
        s.eval::<String>(&format!(
            "for i = 1, NumTaxiNodes() do if TaxiNodeName(i) == {name:?} then return tostring(i) end end return 'none'"
        ))
    })
    .expect("a VM")
    .expect("the chunk runs")
}

/// `SMSG_SHOWTAXINODES` runs the map's setup in its handler (`0x6d61e7` -> `0x5932e0`): the table listed, the interact target taken and `TAXIMAP_OPENED` fired in the packet's turn, ahead of the next packet's line (`0x59378b`), with `NumTaxiNodes` and `TaxiNodeCost` read at the call (`0x590710`, `0x591350`). `TakeTaxiNode` sends inside its call (`0x5914b0`): Sentinel Hill's direct hop, Thelsamar's chain once the coinage covers its fare, `ERR_TAXINOTENOUGHMONEY` before; `CloseTaxiMap` fires `TAXIMAP_CLOSED` twice inside its call, the flight master holding the interact target (`0x590bb7` -> `0x512f58`), and the table stays listed (`0x590bc7`).
#[test]
fn the_taxi_map_opens_from_its_handler_and_acts_inside_its_calls() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let rx = tap(&mut rig);
    rig.packets(vec![npc_at(NPC, 0x2000, NEAR)], 2);
    let verbs = &["taxin", "taxicost"];
    let seen = rig.packets(
        vec![stormwind_nodes(NPC), SessionEvent::BarberShopResult(2)],
        2,
    );
    assert_eq!(
        pin(&seen, TAXI_EVENTS, verbs),
        [
            "TAXIMAP_OPENED | taxin=4 taxicost=110",
            "UI_ERROR_MESSAGE You must be sitting in a barber chair | taxin=4 taxicost=110",
        ]
    );
    let _ = rx.try_iter().count();
    let thelsamar = taxi_slot(&mut rig, "Thelsamar, Loch Modan");
    let hill = taxi_slot(&mut rig, "Sentinel Hill, Westfall");
    assert_eq!(
        fired_in_call(
            &mut rig,
            &format!("TakeTaxiNode({thelsamar})"),
            TAXI_EVENTS,
            &[]
        ),
        ["UI_INFO_MESSAGE You don't have enough money!"],
        "the refusal shows inside the call"
    );
    rig.packets(
        vec![self_values(&[(
            benilla_protocol::field::FIELD_PLAYER_FIELD_COINAGE,
            1000,
        )])],
        1,
    );
    let _ = rx.try_iter().count();
    call(
        &mut rig,
        &format!("TakeTaxiNode({thelsamar}) TakeTaxiNode({hill})"),
    );
    let sent: Vec<String> = rx
        .try_iter()
        .filter_map(|c| match c {
            ClientCommand::ActivateTaxiExpress { guid, nodes } => {
                Some(format!("express {guid:#x} {nodes:?}"))
            }
            ClientCommand::ActivateTaxi {
                guid,
                source_node,
                dest_node,
            } => Some(format!("direct {guid:#x} {source_node}->{dest_node}")),
            _ => None,
        })
        .collect();
    assert_eq!(
        sent,
        [
            format!("express {NPC:#x} [2, 6, 8]"),
            format!("direct {NPC:#x} 2->4"),
        ],
        "sent inside the calls, in call order"
    );
    assert_eq!(
        fired_in_call(&mut rig, "CloseTaxiMap()", TAXI_EVENTS, verbs),
        [
            "TAXIMAP_CLOSED | taxin=4 taxicost=0",
            "TAXIMAP_CLOSED | taxin=4 taxicost=0",
        ]
    );
    assert!(fired_in_call(&mut rig, "CloseTaxiMap()", TAXI_EVENTS, verbs).is_empty());
}

/// An info-kind refusal fires `UI_INFO_MESSAGE` inside the verb's call (`0x5216f0` -> `0x521863`): `TakeTaxiNode` past our coinage (`0x591680` -> `0x5914b0`, the line at `0x591572`) logs its line ahead of the chunk's marker, and the frame's feed shows it no second time (#1306).
#[test]
fn an_info_refusal_fires_in_its_call_before_the_marker() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let _rx = tap(&mut rig);
    rig.packets(vec![npc_at(NPC, 0x2000, NEAR)], 2);
    rig.packets(vec![stormwind_nodes(NPC)], 2);
    let thelsamar = taxi_slot(&mut rig, "Thelsamar, Loch Modan");
    let read = |rig: &mut Rig| {
        super::live::with_live_world(rig.app.world_mut(), |s| {
            s.eval::<Vec<String>>("return Log").unwrap()
        })
        .expect("a VM")
    };
    super::live::with_live_world(rig.app.world_mut(), |s| {
        s.run(&format!(
            "Log = {{}} local f = CreateFrame('Frame') f:RegisterEvent('UI_INFO_MESSAGE') \
             f:SetScript('OnEvent', function(_, e, t) Log[#Log + 1] = e .. ':' .. t end) \
             TakeTaxiNode({thelsamar}) Log[#Log + 1] = 'after'"
        ))
    })
    .expect("a VM")
    .expect("the chunk runs");
    let expected = ["UI_INFO_MESSAGE:You don't have enough money!", "after"];
    assert_eq!(read(&mut rig), expected);
    rig.packets(Vec::new(), 2);
    assert_eq!(read(&mut rig), expected, "the feed shows it no second time");
}

/// A second flight master's map closes the first through the setup's close (`0x593306`), `TAXIMAP_CLOSED` before `TAXIMAP_OPENED`; the same one again does nothing (`0x5932fc`); an `OK` activate reply closes the map in its turn (`0x6ceeb3`), and a refusal shows its line (`0x6ceea7`). Each close fires twice, the flight master holding the interact target (`0x590bb7` -> `0x512f58`).
#[test]
fn a_second_flight_master_closes_the_first_and_a_reply_closes_in_its_turn() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let _rx = tap(&mut rig);
    rig.packets(
        vec![npc_at(NPC, 0x2000, NEAR), npc_at(NPC2, 0x2000, NEAR)],
        2,
    );
    let verbs = &["taxin"];
    let seen = rig.packets(
        vec![
            stormwind_nodes(NPC),
            stormwind_nodes(NPC),
            stormwind_nodes(NPC2),
            SessionEvent::ActivateTaxiReply { code: 3 },
            SessionEvent::ActivateTaxiReply { code: 0 },
            SessionEvent::BarberShopResult(2),
        ],
        2,
    );
    assert_eq!(
        pin(&seen, TAXI_EVENTS, verbs),
        [
            "TAXIMAP_OPENED | taxin=4",
            "TAXIMAP_CLOSED | taxin=4",
            "TAXIMAP_CLOSED | taxin=4",
            "TAXIMAP_OPENED | taxin=4",
            "UI_INFO_MESSAGE You don't have enough money! | taxin=4",
            "TAXIMAP_CLOSED | taxin=4",
            "TAXIMAP_CLOSED | taxin=4",
            "UI_ERROR_MESSAGE You must be sitting in a barber chair | taxin=4",
        ]
    );
}

/// The trainer window's events, with the marker line and the money event the coinage watch fires after the trainer's re-evaluation.
const TRAINER_EVENTS: &[&str] = &[
    "TRAINER_SHOW",
    "TRAINER_UPDATE",
    "TRAINER_CLOSED",
    "PLAYER_MONEY",
    "UI_ERROR_MESSAGE",
];

/// A human warrior's trainer list for `trainer`: Heroic Strike (78) and Rend (772, level 4) under Arms, Battle Shout (6673) under Fury.
fn warrior_list(trainer: u64) -> SessionEvent {
    let row = |spell, cost, req_level| benilla_protocol::messages::TrainerSpell {
        spell,
        state: benilla_protocol::messages::trainer_spell_state::GREEN,
        cost,
        talent_cost: 0,
        profession_cost: 0,
        req_level,
        req_skill: 0,
        req_skill_value: 0,
        req_spells: [0, 0, 0],
    };
    SessionEvent::TrainerList {
        trainer,
        trainer_type: 0,
        services: vec![row(78, 10, 1), row(772, 100, 4), row(6673, 10, 1)],
        greeting: "Hello, $N.".into(),
    }
}

/// A human warrior at level 2 with a trainer `guid` asked.
fn warrior_rig(guid: u64) -> Option<Rig> {
    use benilla_protocol::field::FIELD_UNIT_BYTES_0;
    let mut rig = Rig::login(&[
        OBJECT_TYPE_PLAYER,
        (FIELD_UNIT_BYTES_0, 1 | 1 << 8),
        (FIELD_UNIT_LEVEL, 2),
    ])?;
    rig.packets(vec![npc_at(guid, 0x10, NEAR), npc_at(NPC2, 0x10, NEAR)], 2);
    rig.app
        .world_mut()
        .resource_mut::<crate::ui_trainer::TrainerAsked>()
        .0 = Some(guid);
    Some(rig)
}

/// `SMSG_TRAINER_LIST` runs the setter and the list build in its handler (`0x6d14ed`, `0x6d151e`): the interact target taken, the records sorted and `TRAINER_SHOW` fired in the packet's turn, ahead of the next packet's line (`0x596ae2`), with the visible count and the selection read at the call (`0x593d10`, `0x594430`). The coinage watch re-evaluates the trainer ahead of `PLAYER_MONEY` (`0x6da085`, `0x6da091`); `BuyTrainerService` sends inside its call (`0x594da0`); `CloseTrainer` fires `TRAINER_CLOSED` inside its call, again when the trainer held the interact target (`0x594113` -> `0x512f80`), and the records empty (`0x59411b`).
#[test]
fn the_trainer_opens_from_its_list_and_acts_inside_its_calls() {
    let Some(mut rig) = warrior_rig(NPC) else {
        return;
    };
    let rx = tap(&mut rig);
    let verbs = &["trainn", "trainsel"];
    let seen = rig.packets(
        vec![warrior_list(NPC), SessionEvent::BarberShopResult(2)],
        2,
    );
    assert_eq!(
        pin(&seen, TRAINER_EVENTS, verbs),
        [
            "TRAINER_SHOW | trainn=5 trainsel=2",
            "UI_ERROR_MESSAGE You must be sitting in a barber chair | trainn=5 trainsel=2",
        ]
    );
    let seen = rig.packets(
        vec![self_values(&[(
            benilla_protocol::field::FIELD_PLAYER_FIELD_COINAGE,
            1000,
        )])],
        1,
    );
    assert_eq!(
        pin(&seen, TRAINER_EVENTS, verbs),
        [
            "TRAINER_UPDATE | trainn=5 trainsel=2",
            "PLAYER_MONEY | trainn=5 trainsel=2",
        ],
        "the re-evaluation runs in the coinage watch's turn, ahead of its event"
    );
    let _ = rx.try_iter().count();
    call(&mut rig, "BuyTrainerService(GetTrainerSelectionIndex())");
    let sent: Vec<String> = rx
        .try_iter()
        .filter_map(|c| match c {
            ClientCommand::TrainerBuySpell { trainer, spell_id } => {
                Some(format!("buy {trainer:#x} {spell_id}"))
            }
            _ => None,
        })
        .collect();
    assert_eq!(sent, [format!("buy {NPC:#x} 78")], "sent inside the call");
    assert_eq!(
        fired_in_call(&mut rig, "CloseTrainer()", TRAINER_EVENTS, verbs),
        [
            "TRAINER_CLOSED | trainn=5 trainsel=2",
            "TRAINER_CLOSED | trainn=5 trainsel=2",
            "TRAINER_CLOSED | trainn=5 trainsel=nil",
        ],
        "the stock frame's hide calls CloseTrainer again inside the first event, whose clear of the          held target runs the leg a third time; the log line of the first lands last, after the zero"
    );
    let first = super::live::with_live_world(rig.app.world_mut(), |s| {
        s.eval::<String>("return tostring(GetTrainerServiceInfo(1))")
    })
    .expect("a VM")
    .expect("the chunk runs");
    assert_eq!(first, "nil", "the zero empties the records");
    assert!(fired_in_call(&mut rig, "CloseTrainer()", TRAINER_EVENTS, verbs).is_empty());
}

/// A second trainer's list takes the interact target, whose close runs the first's zero leg (`0x51fd1c` -> `0x512f80`), `TRAINER_CLOSED` before the second `TRAINER_SHOW`; the trainer bit lost on the open trainer closes it in the field's turn (`0x71a172`-`0x71a1a0`), twice with the target held.
#[test]
fn a_second_trainer_closes_the_first_and_a_lost_flag_closes_in_its_turn() {
    let Some(mut rig) = warrior_rig(NPC) else {
        return;
    };
    let _rx = tap(&mut rig);
    let verbs = &["trainn"];
    let seen = rig.packets(vec![warrior_list(NPC)], 1);
    assert_eq!(
        pin(&seen, TRAINER_EVENTS, verbs),
        ["TRAINER_SHOW | trainn=5"]
    );
    rig.app
        .world_mut()
        .resource_mut::<crate::ui_trainer::TrainerAsked>()
        .0 = Some(NPC2);
    let seen = rig.packets(
        vec![
            warrior_list(NPC2),
            npc_at(NPC2, 0, NEAR),
            SessionEvent::BarberShopResult(2),
        ],
        2,
    );
    assert_eq!(
        pin(&seen, TRAINER_EVENTS, verbs),
        [
            "TRAINER_CLOSED | trainn=5",
            "TRAINER_CLOSED | trainn=5",
            "TRAINER_SHOW | trainn=5",
            "TRAINER_CLOSED | trainn=5",
            "TRAINER_CLOSED | trainn=5",
            "TRAINER_CLOSED | trainn=5",
            "UI_ERROR_MESSAGE You must be sitting in a barber chair | trainn=5",
        ],
        "each close runs the stock frame's own CloseTrainer inside its event; the visible count          outlives the zero"
    );
}

const PETITION_EVENTS: &[&str] = &[
    "GUILD_REGISTRAR_SHOW",
    "GUILD_REGISTRAR_CLOSED",
    "PETITION_SHOW",
    "PETITION_CLOSED",
    "PETITION_VENDOR_SHOW",
    "PETITION_VENDOR_UPDATE",
    "PETITION_VENDOR_CLOSED",
    "UI_ERROR_MESSAGE",
];

/// A showlist row costing `cost`.
fn charter_row(
    index: u32,
    cost: u32,
    charter_type: u32,
) -> benilla_protocol::messages::PetitionShowListEntry {
    benilla_protocol::messages::PetitionShowListEntry {
        index,
        charter_entry: 5863,
        charter_display_id: 16161,
        charter_cost: cost,
        charter_type,
        required_signatures: 9,
    }
}

fn show_list(
    npc: u64,
    rows: Vec<benilla_protocol::messages::PetitionShowListEntry>,
) -> SessionEvent {
    SessionEvent::PetitionShowList(benilla_protocol::messages::PetitionShowList {
        npc,
        entries: rows,
    })
}

/// The petitioner and tabard designer flags (`0x6d6ab7`).
const REGISTRAR_FLAGS: u32 = 0x40000 | 0x80000;

/// `SMSG_PETITION_SHOWLIST` sets the registrar in its handler (`0x6d6ab7` -> `0x599e00`): the interact target taken, `GUILD_REGISTRAR_SHOW` in the packet's turn, ahead of the next packet's line; a second registrar's list closes the first through the target's interaction close (`0x51fd1c` -> `0x512ff1`). `CloseGuildRegistrar` fires `GUILD_REGISTRAR_CLOSED` twice inside its call, the registrar holding the target (`0x599f00` -> `0x512ff1`), and `GetGuildCharterCost` reads the row kept past the close (`0x599f40`).
#[test]
fn the_registrar_opens_from_its_list_and_closes_inside_its_call() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let _rx = tap(&mut rig);
    rig.packets(
        vec![
            npc_at(NPC, REGISTRAR_FLAGS, NEAR),
            npc_at(NPC2, REGISTRAR_FLAGS, NEAR),
        ],
        2,
    );
    let verbs = &["charter"];
    let seen = rig.packets(
        vec![
            show_list(NPC, vec![charter_row(1, 1000, 0)]),
            SessionEvent::BarberShopResult(2),
            show_list(NPC2, vec![charter_row(1, 2000, 0)]),
        ],
        2,
    );
    assert_eq!(
        pin(&seen, PETITION_EVENTS, verbs),
        [
            "GUILD_REGISTRAR_SHOW | charter=1000",
            "UI_ERROR_MESSAGE You must be sitting in a barber chair | charter=1000",
            "GUILD_REGISTRAR_CLOSED | charter=1000",
            "GUILD_REGISTRAR_SHOW | charter=2000",
        ]
    );
    assert_eq!(
        fired_in_call(&mut rig, "CloseGuildRegistrar()", PETITION_EVENTS, verbs),
        [
            "GUILD_REGISTRAR_CLOSED | charter=2000",
            "GUILD_REGISTRAR_CLOSED | charter=2000",
        ]
    );
    assert!(fired_in_call(&mut rig, "CloseGuildRegistrar()", PETITION_EVENTS, verbs).is_empty());
}

/// A charter's signatures.
fn signatures(item: u64, owner: u64, id: u32, signers: &[u64]) -> SessionEvent {
    SessionEvent::PetitionShowSignatures(benilla_protocol::messages::PetitionShowSignatures {
        item,
        owner,
        petition_id: id,
        signatures: signers
            .iter()
            .map(|&signer| benilla_protocol::messages::PetitionSignature { signer, unknown: 0 })
            .collect(),
    })
}

fn petition_record(id: u32, owner: u64, name: &str) -> SessionEvent {
    SessionEvent::PetitionQueryResponse(benilla_protocol::messages::PetitionQueryResponse {
        petition_id: id,
        owner,
        name: name.into(),
        min_signatures: 9,
        max_signatures: 9,
        ..Default::default()
    })
}

fn player_name(guid: u64, name: &str) -> SessionEvent {
    SessionEvent::PlayerName {
        guid,
        name: name.into(),
        realm: String::new(),
        race: 1,
        class: 1,
        gender: 0,
        declensions: None,
    }
}

/// `SMSG_PETITION_SHOW_SIGNATURES` stores the petition and its signers in its handler (`0x6d6d20` -> `0x5ceef0`, `0x5cf650`): `PETITION_SHOW` waits for the record (`0x5cea60`) and the last signer name (`0x5cebf0`), then fires in that answer's turn, ahead of the next packet's line. Our sign's success shows its line and fires `PETITION_CLOSED` with the window left as it is (`0x6cf036`, `0x6cf047`); the stock frame's close then runs `ClosePetition`, which declines nothing once our sign went out (`0x5cef0d`).
#[test]
fn the_petition_shows_once_its_record_and_names_land_and_a_sign_closes_in_its_turn() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    const ITEM: u64 = 0x4000_0000_0000_0099;
    const OWNER: u64 = 0x21;
    const SIGNER: u64 = 0x22;
    let rx = tap(&mut rig);
    let verbs = &["petnames", "pettitle"];
    let seen = rig.packets(
        vec![
            signatures(ITEM, OWNER, 7, &[SIGNER]),
            petition_record(7, OWNER, "Legacy"),
            SessionEvent::BarberShopResult(2),
        ],
        2,
    );
    assert_eq!(
        pin(&seen, PETITION_EVENTS, verbs),
        ["UI_ERROR_MESSAGE You must be sitting in a barber chair | petnames=1 pettitle=Legacy"],
        "the signer's name is still pending"
    );
    let seen = rig.packets(
        vec![
            player_name(SIGNER, "Signer"),
            SessionEvent::BarberShopResult(2),
        ],
        2,
    );
    assert_eq!(
        pin(&seen, PETITION_EVENTS, verbs),
        [
            "PETITION_SHOW | petnames=1 pettitle=Legacy",
            "UI_ERROR_MESSAGE You must be sitting in a barber chair | petnames=1 pettitle=Legacy",
        ]
    );
    let _ = rx.try_iter().count();
    call(&mut rig, "SignPetition()");
    let seen = rig.packets(
        vec![SessionEvent::PetitionSignResults(
            benilla_protocol::messages::PetitionSignResults {
                item: ITEM,
                player: super::characterization_tests::ME,
                result: 0,
            },
        )],
        2,
    );
    let closes = pin(&seen, PETITION_EVENTS, verbs);
    assert_eq!(
        closes.first().map(String::as_str),
        Some("PETITION_CLOSED | petnames=0 pettitle=nil"),
        "{closes:?}"
    );
    let sent: Vec<String> = rx
        .try_iter()
        .filter_map(|c| match c {
            ClientCommand::PetitionSign { item, byte } => Some(format!("sign {item:#x} {byte}")),
            ClientCommand::PetitionDecline { item } => Some(format!("decline {item:#x}")),
            _ => None,
        })
        .collect();
    assert_eq!(
        sent,
        [format!("sign {ITEM:#x} 1")],
        "no decline after our sign"
    );
}

/// `SMSG_PETITION_SHOWLIST` shows the vendor's list in its handler (`0x5a22a0`): the target taken, `PETITION_VENDOR_UPDATE` then `PETITION_VENDOR_SHOW` in the packet's turn; `ClosePetitionVendor` fires `PETITION_VENDOR_CLOSED` twice inside its call (`0x5a24f0` -> `0x513083`), and `GetNumPetitionItems` keeps the count past the close (`0x5a1f70`).
#[test]
fn the_petition_vendor_opens_from_its_list_and_closes_inside_its_call() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let _rx = tap(&mut rig);
    rig.packets(vec![npc_at(NPC, 0x40000, NEAR)], 2);
    let verbs = &["petitems"];
    let seen = rig.packets(
        vec![
            show_list(
                NPC,
                vec![charter_row(1, 800_000, 1), charter_row(2, 1_200_000, 1)],
            ),
            SessionEvent::BarberShopResult(2),
        ],
        2,
    );
    assert_eq!(
        pin(&seen, PETITION_EVENTS, verbs),
        [
            "PETITION_VENDOR_UPDATE | petitems=0",
            "PETITION_VENDOR_SHOW | petitems=2",
            "UI_ERROR_MESSAGE You must be sitting in a barber chair | petitems=2",
        ]
    );
    assert_eq!(
        fired_in_call(&mut rig, "ClosePetitionVendor()", PETITION_EVENTS, verbs),
        [
            "PETITION_VENDOR_CLOSED | petitems=2",
            "PETITION_VENDOR_CLOSED | petitems=2",
        ]
    );
    assert!(
        fired_in_call(&mut rig, "ClosePetitionVendor()", PETITION_EVENTS, verbs).is_empty(),
        "the vendor's zero gates a second close"
    );
}

/// `DressUpModel` edits its pane's model inside each verb's call: a cached record lands before the call returns, an uncached one sends `CMSG_ITEM_QUERY` inside the call and leaves the pane's callback (`0x5980fa`), which the answer's handler runs (`0x5980b0`); hiding the window keeps the model and empties only the booth.
#[test]
fn dressup_verbs_edit_the_pane_in_their_call_and_a_miss_lands_by_its_callback() {
    use benilla_protocol::field::{FIELD_UNIT_BYTES_0, FIELD_UNIT_DISPLAYID};
    // A human warrior's body, which the booth's look is built on.
    let body = [
        OBJECT_TYPE_PLAYER,
        (FIELD_UNIT_BYTES_0, 1 | 1 << 8),
        (FIELD_UNIT_DISPLAYID, 49),
    ];
    let Some(mut rig) = Rig::login(&body) else {
        return;
    };
    let worn = |name: &str, display_info_id: u32, inventory_type: u32| {
        Some(Box::new(benilla_protocol::ItemInfo {
            display_info_id,
            inventory_type,
            ..crate::items::test_template(name)
        }))
    };
    rig.packets(
        vec![SessionEvent::ItemTemplate {
            entry: 2000,
            info: worn("Charz Chest", 7000, 5),
        }],
        2,
    );
    let rx = tap(&mut rig);
    call(
        &mut rig,
        "ShowUIPanel(DressUpFrame) DressUpModel:SetUnit('player') DressUpModel:TryOn(2000)",
    );
    rig.run(1);
    let pane = rig
        .app
        .world()
        .non_send_resource::<benilla_ui::script::UiScript>()
        .shown_model_pane("DressUpModel")
        .expect("the dressing room is shown")
        .0
        .bits();
    let slot = |rig: &Rig, slot: usize| {
        rig.app
            .world()
            .resource::<crate::ui_dressup::DressUpRooms>()
            .worn_display(pane, slot)
    };
    assert_eq!(
        slot(&rig, 4),
        Some(7000),
        "a cached record lands in the call"
    );
    while rx.try_recv().is_ok() {}

    call(&mut rig, "DressUpModel:TryOn(3000)");
    let asks: Vec<u32> = rx
        .try_iter()
        .filter_map(|c| match c {
            ClientCommand::ItemQuery { entry, .. } => Some(entry),
            _ => None,
        })
        .collect();
    assert_eq!(asks, vec![3000], "the miss asks inside the call");
    assert_eq!(slot(&rig, 0), None, "and lands nothing yet");
    rig.packets(
        vec![SessionEvent::ItemTemplate {
            entry: 3000,
            info: worn("Charz Helm", 8000, 1),
        }],
        1,
    );
    assert_eq!(
        slot(&rig, 0),
        Some(8000),
        "the answer runs the pane's callback"
    );
    let look = rig
        .app
        .world()
        .resource::<crate::portrait::DressUpPreview>()
        .look
        .expect("the shown pane draws its model");
    assert_eq!(
        (look.equipment[0].display_id, look.equipment[4].display_id),
        (8000, 7000)
    );

    call(&mut rig, "HideUIPanel(DressUpFrame)");
    rig.run(1);
    assert_eq!(slot(&rig, 4), Some(7000), "a hidden pane keeps its model");
    assert!(
        rig.app
            .world()
            .resource::<crate::portrait::DressUpPreview>()
            .look
            .is_none(),
        "and the booth draws none"
    );
}

/// The stock popup showing `which`, `nil` with none (`StaticPopup.lua`'s own lookup).
fn popup(rig: &mut Rig, which: &str) -> String {
    eval(rig, &format!("StaticPopup_Visible(\"{which}\")"))
}

/// The stock `CONFIRM_BINDER` popup over the live reach: it shows from the handler's event, Cancel and Escape send nothing (there is no decline opcode), and its `OnUpdate` takes it down once `CheckBinderDist()` reads nil (`StaticPopup.lua:1329`).
#[test]
fn the_binder_popup_declines_silently_and_its_update_hides_it_out_of_reach() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    rig.packets(vec![npc_at(NPC, 0x10000, NEAR)], 2);
    let rx = tap(&mut rig);
    rig.packets(vec![SessionEvent::BinderConfirm { binder: NPC }], 2);
    let shown = popup(&mut rig, "CONFIRM_BINDER");
    assert_ne!(shown, "nil", "the handler's event shows the popup");
    call(&mut rig, &format!("StaticPopup_OnClick({shown}, 2)"));
    assert_eq!(popup(&mut rig, "CONFIRM_BINDER"), "nil", "Cancel hides it");
    rig.packets(vec![SessionEvent::BinderConfirm { binder: NPC }], 2);
    call(&mut rig, "StaticPopup_EscapePressed()");
    assert_eq!(popup(&mut rig, "CONFIRM_BINDER"), "nil", "Escape hides it");
    assert!(
        !rx.try_iter()
            .any(|c| matches!(c, ClientCommand::BinderActivate { .. })),
        "neither sends"
    );

    rig.packets(vec![SessionEvent::BinderConfirm { binder: NPC }], 2);
    rig.run(3);
    assert_ne!(
        popup(&mut rig, "CONFIRM_BINDER"),
        "nil",
        "in reach, the update leaves it up"
    );
    rig.packets(vec![SessionEvent::ObjectDestroyed(NPC)], 3);
    assert_eq!(
        popup(&mut rig, "CONFIRM_BINDER"),
        "nil",
        "the innkeeper gone, the update takes it down"
    );
    assert!(
        !rx.try_iter()
            .any(|c| matches!(c, ClientCommand::BinderActivate { .. })),
        "and sends nothing"
    );
}

/// The stock `CONFIRM_TALENT_WIPE` popup: the cost goes to its money frame (`UIParent.lua:536`), Cancel and Escape send nothing, and its `OnUpdate` takes it down once `CheckTalentMasterDist()` reads nil (`StaticPopup.lua:1297`).
#[test]
fn the_talent_wipe_popup_shows_its_cost_declines_silently_and_hides_out_of_reach() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let ask = |cost| SessionEvent::TalentWipeConfirm { trainer: NPC, cost };
    rig.packets(vec![npc_at(NPC, 0x10, NEAR)], 2);
    let rx = tap(&mut rig);
    rig.packets(vec![ask(15_000)], 2);
    let shown = popup(&mut rig, "CONFIRM_TALENT_WIPE");
    assert_ne!(shown, "nil");
    assert_eq!(
        (
            eval(&mut rig, &format!("{shown}MoneyFrame:IsShown()")),
            eval(&mut rig, &format!("{shown}MoneyFrameGoldButton:GetText()")),
            eval(
                &mut rig,
                &format!("{shown}MoneyFrameSilverButton:GetText()")
            ),
        ),
        ("1".into(), "1".into(), "50".into()),
        "15000 copper on the coin row"
    );
    call(&mut rig, &format!("StaticPopup_OnClick({shown}, 2)"));
    assert_eq!(
        popup(&mut rig, "CONFIRM_TALENT_WIPE"),
        "nil",
        "Cancel hides it"
    );
    rig.packets(vec![ask(50_000)], 2);
    let shown = popup(&mut rig, "CONFIRM_TALENT_WIPE");
    assert_eq!(
        eval(&mut rig, &format!("{shown}MoneyFrameGoldButton:GetText()")),
        "5",
        "a second question repaints the row"
    );
    call(&mut rig, "StaticPopup_EscapePressed()");
    assert_eq!(
        popup(&mut rig, "CONFIRM_TALENT_WIPE"),
        "nil",
        "Escape hides it"
    );
    assert!(
        !rx.try_iter()
            .any(|c| matches!(c, ClientCommand::TalentWipeConfirm { .. })),
        "neither sends"
    );

    rig.packets(vec![ask(15_000)], 2);
    rig.run(3);
    assert_ne!(popup(&mut rig, "CONFIRM_TALENT_WIPE"), "nil", "in reach");
    rig.packets(vec![SessionEvent::ObjectDestroyed(NPC)], 3);
    assert_eq!(
        popup(&mut rig, "CONFIRM_TALENT_WIPE"),
        "nil",
        "the trainer gone, the update takes it down"
    );
}

/// The stock designer on `OPEN_TABARD_FRAME` (`TabardFrame.lua:25`): a guild master with our record cached sees it seeded and painted with Save enabled; Cancel hides the window.
#[test]
fn the_tabard_window_seeds_and_paints_for_a_guild_master() {
    let Some(mut rig) = Rig::login(&guild_master()) else {
        return;
    };
    rig.packets(
        vec![npc_at(NPC, 0x80000, NEAR), guild_record(7, "Probe Guild")],
        2,
    );
    rig.packets(vec![SessionEvent::TabardVendorActivate(NPC)], 2);
    assert_eq!(eval(&mut rig, "TabardFrame:IsShown()"), "1");
    assert_eq!(
        eval(&mut rig, "TabardFrameAcceptButton:IsEnabled()"),
        "1",
        "a master with a cached record can save"
    );
    assert_eq!(
        eval(
            &mut rig,
            "TabardFrameGreetingText:GetText() == TABARDVENDORGREETING"
        ),
        "true"
    );
    let cell = eval(&mut rig, "TabardFrameEmblemTopLeft:GetTexture()");
    assert!(
        cell.contains("Emblem_03_") && cell.ends_with("_TU_U"),
        "the seeded emblem 3 painted: {cell}"
    );
    call(&mut rig, "TabardFrameCancelButton:Click()");
    rig.run(1);
    assert_ne!(
        eval(&mut rig, "TabardFrame:IsShown()"),
        "1",
        "Cancel hides it"
    );
}

/// A member below rank 0 gets the no-guild greeting and no Save (`TabardFrame.lua:97`).
#[test]
fn the_tabard_window_refuses_a_non_master() {
    let mut fields = guild_master();
    fields.retain(|(f, _)| *f != benilla_protocol::field::FIELD_PLAYER_GUILDRANK);
    fields.push((benilla_protocol::field::FIELD_PLAYER_GUILDRANK, 3));
    let Some(mut rig) = Rig::login(&fields) else {
        return;
    };
    rig.packets(
        vec![npc_at(NPC, 0x80000, NEAR), guild_record(7, "Probe Guild")],
        2,
    );
    rig.packets(vec![SessionEvent::TabardVendorActivate(NPC)], 2);
    assert_eq!(eval(&mut rig, "TabardFrame:IsShown()"), "1");
    assert_ne!(eval(&mut rig, "TabardFrameAcceptButton:IsEnabled()"), "1");
    assert_eq!(
        eval(
            &mut rig,
            "TabardFrameGreetingText:GetText() == TABARDVENDORNOGUILDGREETING"
        ),
        "true"
    );
}
