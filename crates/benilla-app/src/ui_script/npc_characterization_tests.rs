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

/// Our own player's init runs its guild change (`0x6e82c0`) before the active player's init (`0x6e8378`), whose UI world entry fires after it (`0x6e8186`); a worldport's fresh create of ours.
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
    assert_eq!(fired, ["TABARD_CANSAVE_CHANGED", "PLAYER_ENTERING_WORLD"]);
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

/// A new map's load closes the barber shop (`0x403ba2`), then its object manager's teardown leaves the UI's world, whose reader part closes the reader (`0x403ba7` -> `0x528ce2` -> `0x58a430`), twice as the reader still holds the interact target (`0x58a207` -> `0x512e60`), all in the worldport's turn ahead of the next packet's line; a login verify and our create again leave both open.
#[test]
fn a_new_map_closes_the_barber_then_the_reader_in_its_turn() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let _rx = tap(&mut rig);
    rig.packets(plaque(false), 2);
    let port = |needs_ack| SessionEvent::Worldport {
        map_id: 1,
        position: [16226.2, 16257.0, 13.2],
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
    assert_eq!(
        pin(&seen, events, verbs),
        [format!("{error} | barberinfo=4 ittext=Page one")],
        "a login verify leaves both open"
    );
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

/// Only an item's disable has a reader leg (`0x70b036` -> `0x58a1a0`): a read GameObject's destroy leaves the reader open.
#[test]
fn a_read_gameobject_leaving_leaves_the_reader_open() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let _rx = tap(&mut rig);
    rig.packets(plaque(false), 2);
    rig.packets(vec![SessionEvent::GameObjectPageText { guid: PLAQUE }], 2);
    let seen = rig.packets(vec![SessionEvent::ObjectDestroyed(PLAQUE)], 2);
    assert!(
        pin(&seen, ITEM_TEXT_EVENTS, &[]).is_empty(),
        "a GameObject's disable has no reader leg"
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
    assert_eq!(
        pin(&seen, MERCHANT_EVENTS, verbs),
        vec!["MERCHANT_SHOW | merchantn=2"]
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
    assert_eq!(
        pin(&seen, MERCHANT_EVENTS, verbs),
        vec![
            "MERCHANT_CLOSED | merchantn=0",
            "MERCHANT_SHOW | merchantn=1"
        ]
    );
}

/// `CloseMerchant` closes inside its call: the vendor holds the interact target, whose clear runs the interaction close's merchant leg first, so `MERCHANT_CLOSED` fires twice (`0x584614` -> `0x512f3a`, `0x58463e`); a second call finds no vendor.
#[test]
fn close_merchant_fires_twice_inside_its_call() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let _rx = tap(&mut rig);
    let verbs = &["merchantn"];
    rig.packets(vec![npc_at(NPC, 0x80, NEAR)], 2);
    rig.packets(vec![vendor_list(NPC, vec![vendor_row(11, 1, 3)])], 2);
    let pinned = fired_in_call(&mut rig, "CloseMerchant()", MERCHANT_EVENTS, verbs);
    assert_eq!(
        pinned,
        vec![
            "MERCHANT_CLOSED | merchantn=0",
            "MERCHANT_CLOSED | merchantn=0"
        ]
    );
    let pinned = fired_in_call(&mut rig, "CloseMerchant()", MERCHANT_EVENTS, verbs);
    assert!(pinned.is_empty(), "{pinned:?}");
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
