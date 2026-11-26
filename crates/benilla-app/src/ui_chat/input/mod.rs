//! The submitted-line side of the chat: [`drain_chat_input`] executes what [`parse`] resolved and
//! the verbs the stock `ChatFrame.lua` handlers queue in the VM; the addon drains send
//! `SendChatMessage` and `SendAddonMessage`. The emote arm is `DoEmote`: the eligibility gate, the
//! asleep gate, the stow, the posture, then `CMSG_TEXT_EMOTE`. Own sends are never echoed locally;
//! the server echoes them.

use crate::ui_chat::ChatSink as _;
use bevy::prelude::*;

mod parse;
mod send_text;
#[cfg(test)]
pub(super) use parse::lua_quoted_string;
pub(super) use parse::{parse_line, ParsedChat};

use crate::creature_anim::{move_flags, MovementState};
use crate::net::{ClientCommand, NetCommands, SelfPlayer};
use crate::target::Selection;

/// The target half of `GetSlashCmdTarget` (`ChatFrame.lua:650-658`): a bare command falls back
/// to the selection only when it is a player. A streamed player's name is always cached.
/// A streamed player by name, case-insensitive, for `/reaction`'s typed argument.
fn streamed_player_named(
    name: &str,
    index: &crate::net::GuidIndex,
    names: &crate::names::NameCache,
) -> Option<u64> {
    index
        .0
        .keys()
        .copied()
        .filter(|g| benilla_protocol::guid::is_player(*g))
        .find(|g| names.peek(*g).is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

fn target_player_name(
    selection: &Selection,
    names: &crate::names::NameCache,
    commands: &NetCommands,
) -> Option<String> {
    let guid = selection.guid?;
    if !benilla_protocol::guid::is_player(guid) {
        return None;
    }
    names.resolve(guid, commands).map(str::to_string)
}

/// `DoEmote`'s second argument through the VM's resolver with the `World` lent (`0x520190`);
/// `None` with no VM. A streamed object that is no player is the type test's miss.
fn emote_token(world: &mut World, arg: Option<&str>) -> Option<benilla_ui::script::EmoteTarget> {
    crate::ui_script::safe_world::lend(world, |s| {
        s.emote_target(arg, |guid| {
            let streamed = crate::ui_script::safe_world::with_world(|w| {
                w.get_resource::<crate::net::GuidIndex>()
                    .is_some_and(|i| i.0.contains_key(&guid))
            });
            streamed.unwrap_or(false) && !benilla_protocol::guid::is_player(guid)
        })
    })
}

/// The target guid a `CMSG_TEXT_EMOTE` carries (`DoEmote`'s caller): the selection, a token's guid
/// or the name search's, 0 on a miss. Either way an emote at yourself goes out untargeted
/// (`0x6ddaa0`).
pub(super) fn emote_target(
    target: benilla_ui::script::EmoteTarget,
    selection: &Selection,
    named: impl FnOnce(&str) -> Option<u64>,
    me: Option<u64>,
) -> u64 {
    use benilla_ui::script::EmoteTarget as T;
    let guid = match target {
        T::Selection => selection.guid,
        T::Guid(guid) => Some(guid),
        T::Name(name) => named(&name),
    };
    guid.filter(|&guid| Some(guid) != me).unwrap_or(0)
}

/// `/partytest raid`'s writes, applied off the VM's systems so the lent view reads the group and the name cache: the 25 synthetic rows, each member's name record, and the roster lines.
fn party_test_raid(world: &mut World, me: Option<u64>) {
    let lines = world.resource_scope(|world, mut names: Mut<crate::names::NameCache>| {
        crate::ui_party::synthetic_raid(
            &mut world.resource_mut::<crate::ui_party::GroupState>(),
            &mut names,
            me,
        )
    });
    for line in lines {
        crate::ui_action::show_line_in_world(world, "partytest", line);
    }
}

/// The inputs of `/shot`, `/liquid` and `/reaction`, bundled because [`drain_chat_input`] is at
/// Bevy's 16-parameter ceiling.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct ChatProbes<'w, 's> {
    camera: Query<
        'w,
        's,
        &'static GlobalTransform,
        (With<benilla_world::view::WorldCamera>, Without<SelfPlayer>),
    >,
    clock: Option<Res<'w, benilla_world::lighting::GameClock>>,
    world: benilla_world::world_point::WorldPoint<'w, 's>,
    stores: Query<'w, 's, &'static crate::net::ObjectStore>,
    self_store: Query<'w, 's, &'static crate::net::ObjectStore, With<SelfPlayer>>,
    factions: Option<Res<'w, crate::target::Factions>>,
    reputations: Res<'w, crate::net::Reputations>,
    /// `/reaction <name>`'s resolve, for a player a scripted probe cannot click.
    guids: Res<'w, crate::net::GuidIndex>,
    kinds: Query<'w, 's, &'static crate::net::NetEntity>,
    /// `/partytest raid` seats us as the leader, and the wire names a leader by guid.
    self_guid: Res<'w, crate::net::SelfGuid>,
    /// Our own server spline, flying on a taxi: the emote gates read it live.
    self_spline: Query<'w, 's, &'static crate::net::Spline, With<SelfPlayer>>,
    /// Our own auto-attack target, a gate of `DoEmote`'s local play.
    engaged: Query<'w, 's, (), (With<SelfPlayer>, With<crate::creature_anim::Engaged>)>,
    /// Our own held cast anim, the same gate's.
    casting: Query<'w, 's, (), (With<SelfPlayer>, With<crate::creature_anim::CastHold>)>,
    loot_kneel: Option<Res<'w, crate::ui_loot::LootKneel>>,
}

/// What the drain hands to another subsystem's setter, bundled for the same ceiling. Chat never
/// writes [`Selection`]: [`crate::target`]'s resolver commits through the click's path.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct ChatOut<'w, 's> {
    /// A `/console` line runs against the world at the next sync point.
    console: Commands<'w, 's>,
    stand: MessageWriter<'w, crate::player::StandStateRequest>,
    sheath: MessageWriter<'w, crate::creature_anim::SheathRequest>,
    /// `DoEmote`'s local play.
    anim: MessageWriter<'w, crate::creature_anim::EmoteAnim>,
    target: MessageWriter<'w, crate::target::TargetByNameRequest>,
    assist: MessageWriter<'w, crate::target::AssistRequest>,
    follow: MessageWriter<'w, crate::player::FollowRequest>,
    /// The voice file `CMSG_TEXT_EMOTE` names: the kits, and our display's race and sex.
    sound_kits: Option<ResMut<'w, crate::sound::SoundKits>>,
    creatures: Option<Res<'w, crate::entities::Creatures>>,
    /// `/wave <name>`'s player search, which selects nothing.
    players: crate::target::PlayerLookup<'w, 's>,
    /// The party and raid `/wave <name>` searches before the visible objects.
    roster: Option<Res<'w, crate::spell::group_relation::GroupRoster>>,
    /// `/partytest ping` seats a group member's ping through the wire arm's `seat`.
    ping: ResMut<'w, crate::minimap::MinimapPing>,
}

/// The verbs the stock `ChatFrame.lua` handlers call once they have parsed a line (`DoEmote`,
/// `RandomRoll`, `ConsoleExec`, the channel verbs), drained from the VM as
/// [`ParsedChat`] values, so one executor serves them and the lines the Lua never sees.
fn engine_verbs(
    script: &mut benilla_ui::script::UiScript,
    emotes: Option<&crate::sound::EmoteSounds>,
) -> Vec<(String, ParsedChat)> {
    let mut out = Vec::new();
    for e in script.take_emote_requests() {
        // The token is the `EmotesText.dbc` name (`EMOTE<i>_TOKEN`, "WAVE").
        match emotes.and_then(|c| c.text_id(&e.token)) {
            Some(text_id) => out.push((
                String::new(),
                ParsedChat::TextEmote {
                    text_id,
                    arg: e.target,
                },
            )),
            None => warn!(
                "chat: DoEmote({:?}): no EmotesText row for that token",
                e.token
            ),
        }
    }
    for (min, max) in script.take_roll_requests() {
        out.push((String::new(), ParsedChat::Random { min, max }));
    }
    for line in script.take_console_lines() {
        // `ConsoleExec` already applied a valued CVar line; what reaches here is a console command
        // (`detailDoodadAlpha`, never persists) or a bare CVar name to print.
        out.push((line.clone(), ParsedChat::Console { line }));
    }
    for cmd in script.take_channel_commands() {
        out.push((String::new(), ParsedChat::Channel(cmd)));
    }
    out
}

/// The lines a [`chat_arm`] adds, which [`drain_chat_input`] adds in its turn once the arm returns.
#[derive(Resource, Default)]
pub(crate) struct ArmLines(Vec<super::ChatLine>);

/// The VM's model for one arm, which calls no Lua and raises nothing: its lines go to [`ArmLines`].
struct ArmVm<'a> {
    vm: &'a mut benilla_ui::script::UiScript,
    lines: &'a mut Vec<super::ChatLine>,
}

impl std::ops::Deref for ArmVm<'_> {
    type Target = benilla_ui::script::UiScript;
    fn deref(&self) -> &Self::Target {
        self.vm
    }
}

impl std::ops::DerefMut for ArmVm<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.vm
    }
}

impl super::ChatSink for ArmVm<'_> {
    fn push_line(&mut self, line: super::ChatLine) {
        self.lines.push(line);
    }
}

/// One parsed line that runs no Lua: its sends go out in the call, its lines wait in [`ArmLines`].
fn chat_arm(
    In((msg, parsed, emote)): In<(String, ParsedChat, Option<benilla_ui::script::EmoteTarget>)>,
    mut vm: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut arm_lines: ResMut<ArmLines>,
    commands: Res<NetCommands>,
    emotes: Option<Res<crate::sound::EmoteSounds>>,
    selection: Res<Selection>,
    group: Res<crate::ui_party::GroupState>,
    names: Res<crate::names::NameCache>,
    // Our stand state and move flags for the emote gates; the entity is `/castvis`'s fallback.
    self_player: Query<(Entity, &MovementState, &GlobalTransform), With<SelfPlayer>>,
    mut cast_events: MessageWriter<crate::creature_anim::CastEvent>,
    mut play_seq: ResMut<crate::creature_anim::PlaySeq>,
    mut go_targets: MessageWriter<crate::creature_anim::SpellGoTargets>,
    mut chat_out: ChatOut,
    probes: ChatProbes,
) {
    let ChatProbes {
        camera: world_camera,
        clock,
        world,
        stores,
        self_store,
        self_spline,
        factions,
        reputations,
        guids,
        kinds,
        self_guid,
        engaged,
        casting,
        loot_kneel,
    } = &probes;
    let Some(vm) = vm.as_deref_mut() else {
        return;
    };
    let mut script = ArmVm {
        vm,
        lines: &mut arm_lines.0,
    };
    let msg = msg.as_str();
    match parsed {
        // `/r` is the stock `ChatEdit_ParseText`'s REPLY arm; a typed one never gets here.
        ParsedChat::Reply { .. } => {}
        ParsedChat::ChatList { name } => {
            let _ = commands.0.send(ClientCommand::ChannelList { name });
        }
        ParsedChat::Random { min, max } => {
            let _ = commands.0.send(ClientCommand::RandomRoll { min, max });
        }
        ParsedChat::Played => {
            let _ = commands.0.send(ClientCommand::PlayedTime { show: true });
        }
        ParsedChat::Shot => {
            // The camera pose in a capture `Scenario`'s raw WoW coords, to chat and a file.
            let Ok(cam) = world_camera.single() else {
                return;
            };
            let (_, rot, eye_bevy) = cam.to_scale_rotation_translation();
            let eye = benilla_assets::coords::bevy_to_wow(eye_bevy);
            let look = benilla_assets::coords::bevy_to_wow(eye_bevy + rot * Vec3::NEG_Z * 50.0);
            let minute = clock.as_deref().map(|c| c.minute).unwrap_or(720);
            let snippet = format!(
                "eye: [{:.1}, {:.1}, {:.1}], look: [{:.1}, {:.1}, {:.1}], minute: {minute}",
                eye[0], eye[1], eye[2], look[0], look[1], look[2]
            );
            script.push_event(super::event::ChatEvent::text_only(
                super::event::ChatEventKind::System,
                format!("shot: {snippet}"),
            ));
            if let Some(path) = crate::local_state::shots_path() {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let line = format!("{snippet}\n");
                let appended = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
                if appended.is_ok() {
                    script.push_event(super::event::ChatEvent::text_only(
                        super::event::ChatEventKind::System,
                        format!("shot: appended to {}", path.display()),
                    ));
                }
            }
        }
        ParsedChat::Liquid => {
            // Every candidate footprint, so a wrong surface shows beside the right one.
            let Ok((_, feet)) = self_player.single().map(|(_, _, t)| ((), t.translation())) else {
                script.push_event(super::event::ChatEvent::text_only(
                    super::event::ChatEventKind::System,
                    "liquid: no player yet".into(),
                ));
                return;
            };
            let wow = benilla_assets::coords::bevy_to_wow(feet);
            let claim = world.claim(benilla_world::world_point::Subject::Player);
            let eye = world.claim(benilla_world::world_point::Subject::Eye);
            let verdict = world.liquid_at(benilla_world::world_point::Subject::Player, wow);
            let mut lines = vec![
                format!(
                    "liquid: feet [{:.2}, {:.2}, {:.2}] · claim {claim:?} ({})",
                    wow[0],
                    wow[1],
                    wow[2],
                    match world.interior() {
                        Some(k) => format!(
                            "wmo {} nameSet {} group {}",
                            k.wmo_id, k.name_set, k.group_area_id
                        ),
                        None => "no WMO interior claim".into(),
                    }
                ),
                // The camera eye has its own claim, and it decides the underwater filter.
                format!("liquid: EYE claim {eye:?}"),
                match verdict {
                    Some(h) => format!(
                        "liquid: VERDICT {:?} surface z {:.2} ({:+.2} over feet)",
                        h.kind,
                        h.surface_z,
                        h.surface_z - wow[2]
                    ),
                    None => "liquid: VERDICT none — not in liquid".into(),
                },
            ];
            let candidates =
                world.describe_liquid_at(benilla_world::world_point::Subject::Player, wow);
            if candidates.is_empty() {
                lines.push("liquid: no footprint covers this XY at all".into());
            }
            lines.extend(candidates.into_iter().map(|c| format!("liquid:   {c}")));
            for line in lines {
                script.push_event(super::event::ChatEvent::text_only(
                    super::event::ChatEventKind::System,
                    line,
                ));
            }
        }
        ParsedChat::Reaction { name } => {
            // In the ladder's order: the duel gates, the faction work, then `can_attack`.
            let subject_guid = match &name {
                Some(n) => streamed_player_named(n, guids, &names),
                None => selection.guid,
            };
            let subject_entity = subject_guid.and_then(|g| guids.0.get(&g).copied());
            let target_store = subject_entity.and_then(|e| stores.get(e).ok());
            let own_store = self_store.iter().next();
            let is_player = subject_entity
                .and_then(|e| kinds.get(e).ok())
                .is_some_and(|n| n.kind == benilla_protocol::EntityKind::Player);
            let mut lines = Vec::new();
            let describe = |label: &str, s: Option<&crate::net::ObjectStore>| match s {
                Some(s) => format!(
                    "reaction: {label} flags 0x{:08x} (player-controlled {}) faction_tpl {:?} \
                         duel_team {} duel_arbiter 0x{:016x}",
                    s.0.unit_flags(),
                    s.0.unit_flags() & (1 << 3) != 0,
                    s.0.unit_faction_template(),
                    s.0.player_duel_team(),
                    s.0.player_duel_arbiter(),
                ),
                None => format!("reaction: {label} — no ObjectStore"),
            };
            lines.push(format!(
                "reaction: subject {} guid {} · self store present {}",
                name.as_deref().unwrap_or("<current target>"),
                subject_guid.map_or("none".to_string(), |g| format!("0x{g:016x}")),
                own_store.is_some(),
            ));
            lines.push(describe("target", target_store));
            lines.push(describe("self  ", own_store));
            match (target_store, own_store) {
                (Some(t), Some(o)) => {
                    lines.push(format!(
                        "reaction: duel rung {:?}",
                        crate::target::duel_rung(&t.0, &o.0)
                    ));
                }
                _ => lines.push("reaction: duel rung not evaluated (a store is missing)".into()),
            }
            let rank = crate::target::ring_reaction(
                factions.as_deref(),
                reputations,
                &*group,
                target_store,
                own_store,
            );
            lines.push(format!(
                "reaction: RANK {rank} → can_attack {}",
                crate::target::can_attack(
                    target_store,
                    factions.as_deref(),
                    reputations,
                    &*group,
                    own_store
                )
            ));
            // The at-war bit: the whole of the player-to-unit reaction's leg 3 for a faction
            // with a reputation slot, which the cursor, the plate and `UnitCanAttack` turn on.
            let war = (|| {
                let catalog = factions.as_deref()?.catalog();
                let tpl = catalog.template(target_store?.0.unit_faction_template()?)?;
                let at_war = crate::target::ring::at_war_with(catalog, reputations, tpl.faction);
                Some(match at_war {
                    Some(at_war) => format!(
                        "faction {} {:?} owns reputation slot {} → leg 3 answers with AT WAR \
                             = {at_war} (reaction {}); the standing is never read",
                        tpl.faction,
                        catalog.faction_name(tpl.faction).unwrap_or("<unnamed>"),
                        catalog
                            .reputation_faction(tpl.faction)
                            .map_or(-1, |i| i.rep_index),
                        if at_war { "hostile" } else { "friendly" },
                    ),
                    None => format!(
                        "faction {} {:?} has NO reputation slot → the template comparator \
                             answers; at-war does not apply",
                        tpl.faction,
                        catalog.faction_name(tpl.faction).unwrap_or("<unnamed>"),
                    ),
                })
            })()
            .unwrap_or_else(|| "at-war not evaluated (no catalog, store or template)".into());
            lines.push(format!("reaction: {war}"));
            // The world cursor's two predicates, in order: the interact check picks the
            // service ladder, then the attack check the sword. Neither is a reaction threshold.
            let interactable = crate::target::can_interact(
                target_store,
                factions.as_deref(),
                reputations,
                &*group,
                own_store,
            );
            let attackable = crate::target::can_attack(
                target_store,
                factions.as_deref(),
                reputations,
                &*group,
                own_store,
            );
            lines.push(format!(
                "reaction: CURSOR npc_flags 0x{:04x} · can_interact {interactable} · \
                     can_attack {attackable} → {}",
                target_store.map_or(0, |s| s.0.unit_npc_flags()),
                if interactable {
                    "the NPC service ladder (a bit it consults → its cursor; none → Point)"
                } else if attackable {
                    "the ATTACK sword (grayed past 10.45 yd)"
                } else {
                    "nothing matched → Point"
                },
            ));
            // The V-plate category is its own predicate (the attack check player-to-unit, plus
            // the cooperate check for a player), printed with the faction-group masks it
            // compares. A GM-mode character is mask 0 on vmangos (`.gm on` sets faction
            // template 35), so a friendly player can plate as an enemy.
            let mask = |s| {
                crate::target::ring::faction_group_mask(factions.as_deref(), s)
                    .map_or("none".to_string(), |m| m.to_string())
            };
            let friendly = crate::target::ring::plate_is_friendly(
                factions.as_deref(),
                reputations,
                &*group,
                target_store,
                own_store,
                is_player,
            );
            lines.push(format!(
                "reaction: plate is_player {is_player} (OBJECT_FIELD_TYPE says {:?}; the two \
                     must agree — the predicates read the field) · faction-group mask self {} \
                     target {} · can_cooperate {} · can_attack(player→unit) {}",
                target_store.and_then(|s| s.0.object_type()),
                mask(own_store),
                mask(target_store),
                crate::target::ring::can_cooperate_with_player(
                    factions.as_deref(),
                    target_store,
                    own_store
                ),
                crate::target::ring::can_attack_from_player(
                    factions.as_deref(),
                    reputations,
                    &*group,
                    target_store,
                    own_store,
                    is_player,
                ),
            ));
            lines.push(format!(
                "reaction: PLATE CATEGORY {} — this unit plates under {}",
                if friendly { "FRIENDLY" } else { "ENEMY" },
                if friendly { "SHIFT-V" } else { "V" },
            ));
            for line in lines {
                // Also to the log: a scripted two-client run reads stdout, not the feed.
                info!("{line}");
                script.push_event(super::event::ChatEvent::text_only(
                    super::event::ChatEventKind::System,
                    line,
                ));
            }
        }
        // Group writes go through the console queue: the lent live view reads the group.
        ParsedChat::PartyTest { arg } => match arg.as_str() {
            "off" => chat_out.console.queue(|world: &mut World| {
                world
                    .resource_mut::<crate::ui_party::GroupState>()
                    .clear_session();
            }),
            // 25 synthetic raid rows, us leading.
            "raid" => {
                // The wire arm's by-key queue, so the lines resolve from GlobalStrings with
                // each row's surface and sound.
                let me = self_guid.0;
                chat_out
                    .console
                    .queue(move |world: &mut World| party_test_raid(world, me));
            }
            "invite" => chat_out.console.queue(|world: &mut World| {
                world
                    .resource_mut::<crate::ui_party::GroupState>()
                    .pending_invite = Some("Partner".to_string());
            }),
            // A group member's ping 35 yd north-east: off both axes, so a mirrored sign
            // shows, and inside every outdoor view radius (the tightest is 66.7 yd).
            "ping" => {
                let line = if let Ok((_, _, tf)) = self_player.single() {
                    let w = benilla_assets::coords::bevy_to_wow(tf.translation());
                    // +x is north, -y is east.
                    let at = (w[0] + 24.75, w[1] - 24.75);
                    chat_out.ping.seat(at, 0xF001);
                    format!(
                        "partytest: Alice pinged ({:.0}, {:.0}) — 35 yd NE, party1, 5 s",
                        at.0, at.1
                    )
                } else {
                    "partytest: no player position — cannot place a ping".to_string()
                };
                script.push_event(super::event::ChatEvent::text_only(
                    super::event::ChatEventKind::System,
                    line,
                ));
            }
            // Skull the target on the local board; a real mark waits for the server's echo.
            "mark" => {
                if let Some(guid) = selection.guid {
                    chat_out.console.queue(move |world: &mut World| {
                        world
                            .resource_mut::<crate::ui_party::GroupState>()
                            .apply_raid_target(7, guid);
                    });
                }
            }
            arg => {
                let player_xy = self_player.single().ok().map(|(_, _, tf)| {
                    let w = benilla_assets::coords::bevy_to_wow(tf.translation());
                    (w[0], w[1])
                });
                let lead = arg == "lead";
                chat_out.console.queue(move |world: &mut World| {
                    let mut group = world.resource_mut::<crate::ui_party::GroupState>();
                    let lines = crate::ui_party::synthetic_roster(&mut group, player_xy);
                    if lead {
                        // An unmatched leader guid reads as `leader_index` 0, which shows the leader-only rows.
                        group.leader = 0xF000;
                    }
                    for line in lines {
                        crate::ui_action::show_line_in_world(world, "partytest", line);
                    }
                });
            }
        },
        ParsedChat::Invite { name } => {
            if let Some(name) = name.or_else(|| target_player_name(&selection, &names, &commands)) {
                let _ = commands
                    .0
                    .send(ClientCommand::GroupInvite { name, flags: 0 });
            }
        }
        ParsedChat::Uninvite { name } => {
            if let Some(name) = name.or_else(|| target_player_name(&selection, &names, &commands)) {
                let _ = commands.0.send(ClientCommand::GroupUninvite { name });
            }
        }
        ParsedChat::Promote { name } => {
            if let Some(name) = name.or_else(|| target_player_name(&selection, &names, &commands)) {
                // `CMSG_GROUP_SET_LEADER` takes a guid, resolved against the roster. A miss
                // prints the server's error wording; the reference's promote-by-name miss is
                // untraced.
                match group
                    .members
                    .iter()
                    .find(|m| m.name.eq_ignore_ascii_case(&name))
                {
                    Some(m) => {
                        let _ = commands
                            .0
                            .send(ClientCommand::GroupSetLeader { guid: m.guid });
                    }
                    None => {
                        // `ERR_TARGET_NOT_IN_GROUP_S` (`GlobalStrings.lua:1861`).
                        script.push_event(super::event::ChatEvent::text_only(
                            super::event::ChatEventKind::System,
                            format!("{name} is not in your party."),
                        ));
                    }
                }
            }
        }
        // The duel verbs join the intent queue `StartDuel` and `CancelDuel` feed: the stock
        // handlers are one-line calls to them, and `StartDuel` reads no name as the target.
        // Run with the `World` lent by the drain.
        ParsedChat::Duel { .. } => {}
        ParsedChat::Forfeit => {
            script.queue_duel_request(benilla_ui::script::DuelRequest::Cancel);
        }
        // `crate::target`'s resolver commits through the one SetSelection path. A bare
        // `/target` takes `GetSlashCmdTarget`'s fallback: a no-op re-select of a player target,
        // nothing for a creature.
        ParsedChat::Target { name } => {
            if let Some(name) = name.or_else(|| target_player_name(&selection, &names, &commands)) {
                chat_out
                    .target
                    .write(crate::target::TargetByNameRequest { name });
            }
        }
        // Deviation: a probe's bare `/assist` assists the live selection, where the stock
        // handler (`ChatFrame.lua:740`) resolves a selected player's name through
        // `AssistByName`, because a name resolve can land on a different player.
        ParsedChat::Assist { name } => {
            chat_out.assist.write(crate::target::AssistRequest { name });
        }
        // The stock handler's `FollowUnit(msg)` (`ChatFrame.lua:1507`): bare is the empty
        // string, the target, and a name has no exact flag, so prefix matching.
        ParsedChat::Follow { name } => {
            chat_out.follow.write(crate::player::FollowRequest {
                name: name.unwrap_or_default(),
                exact: false,
            });
        }
        // `SlashCmdList["PVP"]` is `TogglePVP()`, the popup row's intent queue.
        ParsedChat::Pvp => script.queue_pvp_toggle(),
        ParsedChat::Help => {
            // benilla's own summary, for a line that skipped the edit box; a typed `/help`
            // runs the stock `ChatFrame_DisplayHelpText`.
            for line in [
                "Chat: /s /y /p /g /o /raid /rw /bg, /w <name>, /r, /e",
                "Channels: /join <name> [pw], /leave <name>, /chatlist <name>",
                "Party: /invite /uninvite /promote [name — bare uses your target]",
                "Loot: /ffa /roundrobin /master <name>",
                "Duel: /duel [name — bare uses your target], /forfeit (/concede /yield)",
                "Social: /who [filter], /friends, /ignore, /trade, /inspect",
                "Emotes: /sit /stand /sleep /kneel and every /wave-style emote",
                "Spells: /cast <name> [(Rank N)]",
                "Macros: /macro (/m) opens the window, /macrohelp explains them",
                "Misc: /afk, /dnd, /random [min] [max], /played, /logout, /quit",
                "Instruments: /shot, /liquid, /reaction, /castvis, /chattest, /partytest",
            ] {
                script.push_event(super::event::ChatEvent::text_only(
                    super::event::ChatEventKind::System,
                    line.to_string(),
                ));
            }
        }
        // `ChatFrame_DisplayMacroHelpText` (`ChatFrame.lua:1695`), off the VM's GlobalStrings.
        ParsedChat::MacroHelp => {
            for i in 1..=5 {
                let key = format!("MACRO_HELP_TEXT_LINE{i}");
                let Ok(line) = script.lua().globals().get::<String>(key.as_str()) else {
                    continue;
                };
                if !line.is_empty() {
                    script.push_event(super::event::ChatEvent::text_only(
                        super::event::ChatEventKind::System,
                        line,
                    ));
                }
            }
        }
        // `DoEmote`: the gates in the client's order, then posture and packet.
        ParsedChat::TextEmote { text_id, .. } => {
            let (stand_state, flags) = self_player
                .single()
                .map_or((0, 0), |(_, m, _)| (m.stand_state, m.flags));
            let swimming = flags & move_flags::SWIMMING != 0;
            let flying_spline = self_spline.single().is_ok_and(|s| !s.grounded);
            let emote_id = emotes.as_deref().and_then(|e| e.text_emote(text_id));
            let posture = emote_id.and_then(|id| emotes.as_deref()?.posture_state(id));
            // `DoEmote`'s gates: a possessed unit, then the `Emotes.dbc` row, then the emote
            // eligibility check; only its `0x4000` arm refuses out loud. A chat-only text emote
            // (`EmoteID` 0) reads row 0.
            let self_unit_flags = self_store.single().map_or(0, |s| s.0.unit_flags());
            let gate = match emotes.as_deref() {
                Some(e) => emote_gate(
                    e.emote_flags(emote_id.unwrap_or(0)),
                    self_unit_flags,
                    stand_state,
                    swimming,
                    flags,
                    flying_spline,
                ),
                None => EmoteGate::Send,
            };
            // `DoEmote`'s half of the `0x4000` arm: the red line only while self-controlled, so
            // a feared or confused player emotes normally.
            if gate == EmoteGate::Moving {
                let controlled = self_store
                    .single()
                    .is_ok_and(|s| crate::player::self_controlled(s.0.unit_flags()));
                if controlled {
                    debug!("chat: emote {text_id} refused — moving (ERR_NOEMOTEWHILERUNNING)");
                    // The line at the display (`0x5216f0`) once the drain's commands apply.
                    chat_out.console.queue(|world: &mut World| {
                        crate::ui_action::show_line_in_world(
                            world,
                            "chat",
                            crate::ui_action::UiError::key("ERR_NOEMOTEWHILERUNNING"),
                        );
                    });
                    return;
                }
            }
            let eligible = gate != EmoteGate::Suppressed;
            // Gate B: asleep, only a posture emote gets through, so `/stand` ends `/sleep` and
            // a `/wave` in bed does nothing.
            let awake_or_posture = stand_state != 3 || posture.is_some();
            if !eligible || !awake_or_posture {
                // `DoEmote` returns before the packet: no send, no anim.
                debug!(
                    "chat: emote {text_id} suppressed (eligible {eligible}, \
                         awake-or-posture {awake_or_posture}, stand {stand_state})"
                );
                return;
            }
            // The stow, unconditional here: every emote past the gates sheathes instantly,
            // mid-fight too.
            if let Ok((entity, _, _)) = self_player.single() {
                chat_out.sheath.write(crate::creature_anim::SheathRequest {
                    entity,
                    state: 0,
                    ceremony: false,
                });
            }
            // The posture branch (`EmoteSpecProc == 1` → a stand-state change) is the whole
            // visible effect: the server plays nothing for SIT, SLEEP and KNEEL
            // (`ChatHandler.cpp:728`).
            if let Some(state) = posture {
                chat_out
                    .stand
                    .write(crate::player::StandStateRequest { state: state as u8 });
            }
            // The play is the posture branch's else: a posture emote plays no clip, and the
            // rest play here only while moving. Its `SMSG_EMOTE` echo then finds the id armed
            // and is skipped.
            else if let (Some(anim), Ok((entity, m, _)), Ok(store)) = (
                emote_id.and_then(|id| emotes.as_deref()?.anim(id)),
                self_player.single(),
                self_store.single(),
            ) {
                let fields = &store.0;
                let gate = crate::creature_anim::LocalPlay {
                    reads_dead: fields.unit_reads_dead(),
                    stand_state: m.stand_state,
                    move_flags: m.flags,
                    vertical_speed: m.vertical_speed,
                    flying: flying_spline,
                    loot_kneel: loot_kneel.as_deref().is_some_and(|k| k.0),
                    channeling: fields.unit_channel_spell() != 0,
                    casting: !casting.is_empty(),
                    engaged: !engaged.is_empty(),
                };
                if crate::creature_anim::local_play_eligible(&gate) {
                    chat_out.anim.write(crate::creature_anim::EmoteAnim {
                        entity,
                        anim_id: anim as u16,
                        seq: play_seq.next(),
                        via_player: false,
                        play: Default::default(),
                    });
                }
            }
            let is_streamed = |guid: u64| guids.0.contains_key(&guid);
            let target = emote.unwrap_or(benilla_ui::script::EmoteTarget::Selection);
            let no_roster = crate::spell::group_relation::GroupRoster::default();
            let roster = chat_out.roster.as_deref().unwrap_or(&no_roster);
            let target = emote_target(
                target,
                &selection,
                |name| {
                    let streamed = chat_out.players.find_other(&names, name, self_guid.0);
                    crate::target::emote_name_match(
                        &names,
                        roster,
                        self_guid.0,
                        streamed,
                        is_streamed,
                        name,
                    )
                },
                self_guid.0,
            );
            let file_index = match (emotes.as_deref(), self_store.single()) {
                (Some(e), Ok(store)) => e.pick_file_index(
                    text_id,
                    store,
                    chat_out.creatures.as_deref(),
                    chat_out.sound_kits.as_deref_mut(),
                ),
                _ => -1,
            };
            if benilla_assets::trace::enabled() {
                benilla_assets::trace::line(
                    "emote",
                    &format!("send text={text_id} index={file_index} target={target:#x}"),
                );
            }
            match commands.0.send(ClientCommand::TextEmote {
                text_id,
                file_index,
                target,
            }) {
                Ok(()) => info!("chat: sent {msg:?}"),
                Err(_) => warn!("chat: not connected; dropped {msg:?}"),
            }
        }
        ParsedChat::CastVis {
            spell_id,
            kind,
            ground,
        } => {
            // The net bridge's own message, on the selection or else self.
            let me = self_player.single().ok().map(|(e, _, _)| e);
            let subject = selection.target.or(me);
            match subject {
                Some(entity) => {
                    info!("castvis: spell {spell_id} {kind:?} ground={ground} on {entity}");
                    cast_events.write(crate::creature_anim::CastEvent {
                        entity,
                        spell_id,
                        kind,
                        cast_count: 0,
                        seq: play_seq.next(),
                    });
                    // A selected caster's GO hits us, so a missile flies end to end; a
                    // self-cast has no target. `ground` sends the pure dest shape instead, a
                    // point 15 yd ahead at the player's height.
                    if matches!(kind, crate::creature_anim::CastEventKind::Go { .. }) {
                        let dest = ground.then(|| {
                            self_player
                                .single()
                                .ok()
                                .map(|(_, _, tf)| tf.translation() + tf.forward().as_vec3() * 15.0)
                        });
                        if let Some(dest) = dest {
                            go_targets.write(crate::creature_anim::SpellGoTargets {
                                list: Vec::new(),
                                target_mask: 0x40,
                                src: None,
                                src_deck: None,
                                caster: entity,
                                spell_id,
                                hits: Vec::new(),
                                misses: Vec::new(),
                                dest,
                                dest_deck: None,
                                points: Vec::new(),
                                ammo: None,
                                seq: play_seq.next(),
                                trajectory: None,
                                reflected: 0,
                                reflects: Vec::new(),
                                caster_guid: 0,
                                server_time: 0,
                            });
                        } else if let Some(me) = me.filter(|&m| m != entity) {
                            go_targets.write(crate::creature_anim::SpellGoTargets {
                                list: Vec::new(),
                                target_mask: 0x2,
                                src: None,
                                src_deck: None,
                                caster: entity,
                                spell_id,
                                hits: vec![me],
                                misses: Vec::new(),
                                dest: None,
                                dest_deck: None,
                                points: Vec::new(),
                                // A real GO carries the caster's ammo; this flies the
                                // Rough Arrow, display id 5996.
                                ammo: Some(crate::creature_anim::ItemModel {
                                    display_id: 5996,
                                    inventory_type: 24,
                                }),
                                seq: play_seq.next(),
                                trajectory: None,
                                reflected: 0,
                                reflects: Vec::new(),
                                caster_guid: 0,
                                server_time: 0,
                            });
                        }
                    }
                }
                None => warn!("castvis: no selection and no self avatar — dropped"),
            }
        }
        ParsedChat::ChatTest => chattest_battery(&mut script),
        // `SlashCmdList["LOGOUT"]` is `Logout()`, the game menu's route: `crate::ui_logout`
        // sends the request and narrates the answer with the CAMP countdown.
        ParsedChat::Logout => {
            script.queue_session_request(benilla_ui::script::SessionRequest::Logout)
        }
        // `Quit()`, the game menu Exit button's queue, countdown and confirmation.
        ParsedChat::Quit => script.queue_session_request(benilla_ui::script::SessionRequest::Quit),
        // Deferred to the world, since a command is `fn(&mut World, &str)`; its lines print
        // as system text.
        ParsedChat::Console { line } => {
            chat_out.console.queue(move |world: &mut World| {
                let lines = crate::console::execute(world, &line);
                for text in lines {
                    system_line(world, text);
                }
            });
        }
        // Sent inside its verb's call ([`channel_arm`]).
        ParsedChat::Channel(_) => {}
        // Run with the `World` lent by [`drain_chat_input`].
        ParsedChat::Join { .. }
        | ParsedChat::Leave { .. }
        | ParsedChat::Lua { .. }
        | ParsedChat::Unknown => {}
    }
}

/// A channel verb's send inside its call (`0x507a80` the join, `0x5096d0` the leave): the slot and
/// mask writes, then the packet; a line it prints waits in [`ArmLines`]. `vm` is the calling VM's
/// handle, which the world-entry load holds outside the `World` while its addons run.
pub(crate) fn channel_arm(
    In((cmd, mut vm)): In<(
        benilla_ui::script::ChannelCommand,
        benilla_ui::script::UiScript,
    )>,
    mut arm_lines: ResMut<ArmLines>,
    mut channels: ResMut<super::edit::ChannelState>,
    commands: Res<NetCommands>,
) -> ArmOutcome {
    let mut script = ArmVm {
        vm: &mut vm,
        lines: &mut arm_lines.0,
    };
    use benilla_ui::script::ChannelCommand as C;
    let cmd = match cmd {
        C::Join {
            id,
            name,
            password,
            voice,
            permanent,
        } => {
            // The record is found or claimed before the send, with no relist (`0x507a80`).
            match channels.claim_for_join(&name, id, 0) {
                super::edit::JoinRecord::Claimed(n) => {
                    debug!("chat: joining {name:?} as slot {n}");
                    channels.set_join_flags(n, voice, permanent);
                }
                super::edit::JoinRecord::Held => return ArmOutcome::Done,
                // The verb is not quiet: its line, then no send (`0x507755`, `0x507c51`).
                super::edit::JoinRecord::Full => return ArmOutcome::TooManyChannels,
                super::edit::JoinRecord::Refused => return ArmOutcome::Declined,
            }
            channels.join_command(id, name, password, false, voice)
        }
        C::Leave { id, name } => {
            // `LeaveChannelByName` past its name (the binding resolved a number and composed a
            // Shortcut, the guild-recruitment latch's reset with it). Only this path clears the
            // zone mask and the custom re-join list.
            // The verb's leave sends only for a record in state 0, under the record's own name
            // (`0x5095b0`, `0x5095b8`-`0x5095c4`, `0x509600`).
            let Some(name) = channels
                .leave_record(&name, id)
                .and_then(|n| channels.slot(n))
                .filter(|s| s.state == super::edit::SlotState::Joined)
                .map(|s| s.name.clone())
            else {
                return ArmOutcome::Done;
            };
            channels.note_zone_channel_left(&name);
            channels.note_custom_channel_left(&name);
            channels.note_leave_asked(&name);
            channels.leave_command(name)
        }
        C::List { name } => ClientCommand::ChannelList { name },
        // `ListChannels()` (`0x50dae0`): each joined record as `[%d. %s] ` (`0x50db31`), packed
        // into one `CHAT_MSG_CHANNEL_LIST` line added in the call, empty or not (`0x50dbf2`).
        C::ListAll => {
            let mut text = String::new();
            for (i, slot) in channels.joined.iter().enumerate() {
                let Some(slot) = slot
                    .as_ref()
                    .filter(|s| s.state == super::edit::SlotState::Joined)
                else {
                    continue;
                };
                let entry = format!("[{}. {}] ", i + 1, slot.name);
                // A full buffer goes out first (`0x50db63`-`0x50db8d`). A 12340 bug, fixed: it
                // measures the text from the entry's length on (`0x50db56`), past its end on a
                // short text; this measures the text's length plus the entry's.
                if text.len() + entry.len() >= LIST_CHANNELS_BUFFER {
                    script.push_event(super::event::ChatEvent::text_only(
                        super::event::ChatEventKind::ChannelList,
                        std::mem::take(&mut text),
                    ));
                }
                text.push_str(&entry);
            }
            script.push_event(super::event::ChatEvent::text_only(
                super::event::ChatEventKind::ChannelList,
                text,
            ));
            return ArmOutcome::Done;
        }
        C::DisplayOwner { name } => ClientCommand::ChannelOwner { name },
        C::SetOwner { name, player } => ClientCommand::ChannelSetOwner { name, player },
        C::SetPassword { name, password } => ClientCommand::ChannelPassword { name, password },
        C::Ban { name, player } => ClientCommand::ChannelBan { name, player },
        C::Invite { name, player } => ClientCommand::ChannelInvite { name, player },
        C::Kick { name, player } => ClientCommand::ChannelKick { name, player },
        C::Moderator { name, player } => ClientCommand::ChannelModerator { name, player },
        C::Unmoderator { name, player } => ClientCommand::ChannelUnmoderator { name, player },
        C::Mute { name, player } => ClientCommand::ChannelMute { name, player },
        C::Unmute { name, player } => ClientCommand::ChannelUnmute { name, player },
        C::Unban { name, player } => ClientCommand::ChannelUnban { name, player },
        C::Moderate { name } => ClientCommand::ChannelModerate { name },
        C::ToggleAnnouncements { name } => ClientCommand::ChannelAnnouncements { name },
        C::DeclineInvite { name } => ClientCommand::DeclineChannelInvite { name },
        C::Watch { name } => ClientCommand::SetChannelWatch { name },
        C::DisplayList { name } => ClientCommand::ChannelDisplayList { name },
    };
    let _ = commands.0.send(cmd);
    ArmOutcome::Done
}

/// What a channel verb's send left for its binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArmOutcome {
    /// The verb went on: a join answers its values and registers its window (`0x507cea`).
    Done,
    /// The join returned before its send, answering nothing (`0x507bf7`).
    Declined,
    /// [`Self::Declined`] after `ERR_TOO_MANY_CHAT_CHANNELS` at the display (`0x507755`).
    TooManyChannels,
}

/// `ListChannels`' line buffer, 3000 bytes (`0x50db63`).
const LIST_CHANNELS_BUFFER: usize = 0xbb8;

/// [`channel_arm`] with the `World` and the calling VM's handle, its lines added in the call's turn;
/// whether the verb went on.
pub(crate) fn run_channel_command(
    world: &mut World,
    vm: benilla_ui::script::UiScript,
    cmd: benilla_ui::script::ChannelCommand,
) -> bool {
    world.get_resource_or_init::<ArmLines>();
    let mut caller = vm.share();
    let outcome = world
        .run_system_cached_with(channel_arm, (cmd, vm))
        .unwrap_or(ArmOutcome::Done);
    let lines = std::mem::take(&mut world.resource_mut::<ArmLines>().0);
    for line in lines {
        super::feed::add(world, line);
    }
    // The line shows inside the call on the calling VM (`0x507755` -> `0x5216f0`), which the
    // world-entry load and a reload hold outside the `World`.
    if outcome == ArmOutcome::TooManyChannels {
        crate::ui_action::show_line_with_vm(
            world,
            &mut caller,
            "chat",
            crate::ui_action::UiError::key("ERR_TOO_MANY_CHAT_CHANNELS"),
        );
    }
    outcome == ArmOutcome::Done
}

/// The frame's submitted lines: the engine verbs' queue, then the typed and probe lines, parsed;
/// a line with no slash goes out as a SAY here.
fn take_chat_lines(
    mut script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    emotes: Option<Res<crate::sound::EmoteSounds>>,
    commands: Res<NetCommands>,
    table: Res<super::commands::SlashCommands>,
) -> Vec<(String, ParsedChat)> {
    let Some(script) = script.as_deref_mut() else {
        return Vec::new();
    };
    let mut queue = engine_verbs(script, emotes.as_deref());
    // The dev instruments, from their host `SlashCmdList` rows once the stock `ChatEdit_ParseText`
    // has found no built-in (dev builds), and probe lines.
    for raw in script.take_chat_input() {
        let msg = raw.trim();
        if msg.is_empty() {
            continue;
        }
        // A probe's line with no slash is a SAY, which is also how a `.gm`-style server command
        // travels; a typed one goes out through the stock `ChatEdit_SendText` instead.
        if !msg.starts_with('/') {
            let cmd = ClientCommand::Chat {
                kind: super::edit::SendType::Say.wire(),
                target: None,
                text: msg.as_bytes().to_vec(),
                language: None,
            };
            if commands.0.send(cmd).is_err() {
                warn!("chat: not connected; line dropped");
            }
            continue;
        }
        queue.push((msg.to_string(), parse_line(&table, msg)));
    }
    queue
}

/// A system line printed to the chat log.
fn system_line(world: &mut World, text: String) {
    super::feed::add(
        world,
        super::event::ChatEvent::text_only(super::event::ChatEventKind::System, text).into(),
    );
}

/// Run `body` in the VM with the `World` lent; its error, if any.
fn run_lent(world: &mut World, body: &str) -> Option<mlua::Error> {
    crate::ui_script::safe_world::lend(world, |s| s.run(body).err()).flatten()
}

/// The frame's submitted lines in order: a line that runs Lua does so with the `World` lent, so a
/// verb it calls applies inside it; the rest run as [`chat_arm`], whose raises fire after it.
pub(crate) fn drain_chat_input(world: &mut World) {
    let Ok(queue) = world.run_system_cached(take_chat_lines) else {
        return;
    };
    crate::ui_script::live::fire_handler_events(world);
    for (msg, parsed) in queue {
        match parsed {
            // Stock built-ins (`ChatFrame.lua:778`, `:803`), so only a probe's line gets here, and
            // it runs the same handler: sent verbatim, `/join General` would create a custom
            // channel of that name. The handlers queue their verbs for the `Channel` arm.
            ParsedChat::Join { name, password } => {
                let body = format!(
                    "SlashCmdList['JOIN']({:?})",
                    format!("{name} {password}").trim_end()
                );
                if let Some(e) = run_lent(world, &body) {
                    warn!("ui_chat: {body}: {e}");
                }
            }
            ParsedChat::Leave { name } => {
                let body = format!("SlashCmdList['LEAVE']({name:?})");
                if let Some(e) = run_lent(world, &body) {
                    warn!("ui_chat: {body}: {e}");
                }
            }
            // The reference's handler body, run in the VM.
            ParsedChat::Lua { body } => {
                if let Some(e) = run_lent(world, &body) {
                    // Logged and printed: the player sees a `/script` typo, the log a failing
                    // built-in.
                    warn!("ui_chat: {body:?}: {e}");
                    system_line(world, format!("{e}"));
                }
            }
            ParsedChat::Unknown => {
                // `SlashCmdList` gets the line before the help reply: an addon's command, or a
                // stock one with no arm here.
                if let Some(rest) = msg.strip_prefix('/') {
                    let rest = rest.trim();
                    let (cmd, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                    let ran = crate::ui_script::safe_world::lend(world, |s| {
                        s.run_slash_command(cmd, args.trim())
                    });
                    if ran == Some(true) {
                        continue;
                    }
                }
                // `HELP_TEXT_SIMPLE`, the reference's unknown-command reply (`ChatFrame.lua:2205`),
                // a plain chat line with no message-catalog surface or sound.
                let text = world
                    .get_non_send_resource::<benilla_ui::script::UiScript>()
                    .and_then(|s| s.lua().globals().get::<String>("HELP_TEXT_SIMPLE").ok())
                    .filter(|t| !t.is_empty());
                if let Some(text) = text {
                    system_line(world, text);
                }
            }
            // `StartDuel` resolves its token at the call (`0x60abf0`), with the `World` lent.
            ParsedChat::Duel { name } => {
                crate::ui_script::safe_world::lend(world, |s| {
                    s.start_duel(name.as_deref().unwrap_or(""));
                });
            }
            ParsedChat::Channel(cmd) => {
                if let Some(vm) = world
                    .get_non_send_resource::<benilla_ui::script::UiScript>()
                    .map(benilla_ui::script::UiScript::share)
                {
                    run_channel_command(world, vm, cmd);
                }
            }
            parsed => {
                // `DoEmote`'s token resolves with the `World` lent, before the arm reads the rest.
                let emote = match &parsed {
                    ParsedChat::TextEmote { arg, .. } => emote_token(world, arg.as_deref()),
                    _ => None,
                };
                world.get_resource_or_init::<ArmLines>();
                let _ = world.run_system_cached_with(chat_arm, (msg, parsed, emote));
                let lines = std::mem::take(&mut world.resource_mut::<ArmLines>().0);
                for line in lines {
                    super::feed::add(world, line);
                }
            }
        }
        crate::ui_script::live::fire_handler_events(world);
    }
}

/// `/chattest`: one synthetic line of every renderable form (kinds, flags, the language header,
/// channel prefixes, notices, item and player links) through the real event pipeline.
fn chattest_battery(log: &mut impl super::ChatSink) {
    use super::event::{ChatEvent, ChatEventKind as K};
    let player = |kind: K, text: &str, sender: &str| {
        let mut e = ChatEvent::text_only(kind, text.into());
        e.sender = sender.into();
        e
    };
    let mut battery: Vec<ChatEvent> = vec![
        player(K::Say, "the quick brown fox — say white.", "Testa"),
        player(K::Yell, "yell red!", "Testa"),
        player(
            K::Whisper,
            "whisper pink (chime + flash if Combat Log is selected).",
            "Testa",
        ),
        player(K::WhisperInform, "the To-echo.", "Testa"),
        player(K::Emote, "dances — bare name, orange.", "Testa"),
        player(K::Party, "party blue.", "Testa"),
        player(K::Guild, "guild green.", "Testa"),
        player(K::Officer, "officer deep green.", "Testa"),
        player(K::RaidWarning, "raid warning salmon.", "Testa"),
        player(K::MonsterSay, "monster say pale yellow.", "Grunt"),
        player(K::MonsterYell, "monster yell red.", "Grunt"),
        player(K::MonsterWhisper, "monster whisper gray.", "Grunt"),
        player(K::MonsterEmote, "%s looks around — emote orange.", "Grunt"),
        ChatEvent::text_only(K::System, "system yellow.".into()),
        ChatEvent::text_only(
            K::Skill,
            "Your skill in Testing has increased to 300.".into(),
        ),
        ChatEvent::text_only(
            K::Loot,
            "You receive loot: |cff1eff00|Hitem:2000:0:0:0|h[Test Blade]|h|r — click me.".into(),
        ),
        ChatEvent::text_only(K::Money, "You loot 1 Gold, 23 Silver, 45 Copper.".into()),
    ];
    // A GM-flagged line, an Orcish header, a numbered channel line, and the join/kick notices.
    let mut gm = player(K::Say, "a GM-tagged line.", "Testa");
    gm.flag = "GM".into();
    battery.push(gm);
    let mut orc = player(K::Say, "an Orcish-headered line.", "Grunk");
    orc.language = "Orcish".into();
    battery.push(orc);
    let mut chan = player(K::Channel, "channel pink.", "Testa");
    chan.channel = "1. General - Elwynn Forest".into();
    battery.push(chan);
    let mut join = player(K::ChannelJoin, "", "Testa");
    join.channel = "1. General - Elwynn Forest".into();
    battery.push(join);
    let mut notice = ChatEvent::text_only(K::ChannelNotice, String::new());
    notice.channel = "General - Elwynn Forest".into();
    notice.notice = "2".into(); // YOU_JOINED
    battery.push(notice);
    for e in battery {
        log.push_event(e);
    }
    info!("chattest: battery queued");
}

/// What the emote eligibility check decides about one emote.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum EmoteGate {
    /// Send it: packet, posture, animation.
    Send,
    /// Dropped silently: no packet, no animation, no line. A seated `/bow` is this.
    Suppressed,
    /// The `0x4000` "requires standing still" arm tripped. `DoEmote` raises
    /// `ERR_NOEMOTEWHILERUNNING` and aborts only when the player is self-controlled, false while
    /// `UNIT_FIELD_FLAGS & 0xc00004`, so a feared player emotes; that test is the caller's.
    Moving,
}

/// The emote eligibility check (`0x4f5410`), the only reader of `Emotes.dbc` `EmoteFlags`, in the
/// client's order; `DoEmote` calls it before building `CMSG_TEXT_EMOTE`. The `0x4000` arm tests
/// movement, not `SWIMMING`, so it is no water gate. `flying_spline` is a live spline with the
/// flying flag `0x2000` and not the done flag `0x400`.
pub(super) fn emote_send_eligible(
    emote_flags: u32,
    stand_state: u8,
    swimming: bool,
    move_flags: u32,
    flying_spline: bool,
) -> EmoteGate {
    // `0x0400`: unconditional suppress.
    if emote_flags & 0x0400 != 0 {
        return EmoteGate::Suppressed;
    }
    // `0x0001` + non-zero stand-state: "requires STAND".
    if emote_flags & 0x0001 != 0 && stand_state != 0 {
        return EmoteGate::Suppressed;
    }
    // `0x0080` while swimming.
    if emote_flags & 0x0080 != 0 && swimming {
        return EmoteGate::Suppressed;
    }
    // `0x8000` on a flying spline (`0x4f5462`) or with the flying bit (`0x4f547d`).
    if emote_flags & 0x8000 != 0
        && (flying_spline || move_flags & crate::creature_anim::move_flags::FLYING != 0)
    {
        return EmoteGate::Suppressed;
    }
    // `0x0200` ABSENT at SLEEP(3)/DEAD(7): the bit means "allowed while asleep/dead".
    if emote_flags & 0x0200 == 0 && matches!(stand_state, 3 | 7) {
        return EmoteGate::Suppressed;
    }
    // `0x4000` "requires standing still", against the live movement word's `0x20ff` (direction,
    // turn and pitch bits and FALLING), which is [`crate::creature_anim::move_flags::INTEGRATED`].
    // It only reports; `DoEmote` decides.
    if emote_flags & 0x4000 != 0 && move_flags & crate::creature_anim::move_flags::INTEGRATED != 0 {
        return EmoteGate::Moving;
    }
    EmoteGate::Send
}

/// `UNIT_FLAG_POSSESSED`, `UNIT_FIELD_FLAGS` bit 24: `DoEmote` returns silently for a unit that has
/// it (`descriptor+0xa3 & 1`).
const UNIT_FLAG_POSSESSED: u32 = 0x0100_0000;

/// `DoEmote`'s entry gates in the client's order: the unit's own `UNIT_FIELD_FLAGS`, the
/// `Emotes.dbc` row by `EmoteID` (`None` is a missing row, which returns), then the emote
/// eligibility check over the row's flags.
pub(super) fn emote_gate(
    row_flags: Option<u32>,
    unit_flags: u32,
    stand_state: u8,
    swimming: bool,
    move_flags: u32,
    flying_spline: bool,
) -> EmoteGate {
    match row_flags {
        Some(flags) if unit_flags & UNIT_FLAG_POSSESSED == 0 => {
            emote_send_eligible(flags, stand_state, swimming, move_flags, flying_spline)
        }
        _ => EmoteGate::Suppressed,
    }
}

/// `PLAYER_FLAGS_DND` (`0x4`) off our live descriptor. Live, not mirrored, as in the reference: AFK
/// keeps an optimistic global and DND nothing, so a second `/dnd` before the server's update
/// re-marks where a second `/afk` clears.
fn is_dnd(self_q: &Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>) -> bool {
    self_q
        .iter()
        .next()
        .is_some_and(|s| s.0.player_flags() & 0x4 != 0)
}

/// `autoClearAFK`, registered default `"1"`. Off, the clear is a total no-op: no echo, no mirror
/// write, no packet.
fn auto_clear_afk(cvars: &crate::cvars::Cvars) -> bool {
    cvars.flag("autoClearAFK").unwrap_or(true)
}

/// The leader's lanes: a party line goes out as `0x33` (`0x50d516`), a raid line as `0x27`
/// (`0x50d412`) and a battleground line as `0x2d` (`0x50d4e9`) when we lead the party, each past
/// its group gate.
pub(super) fn party_lane(wire: crate::net::ChatKind, leads: bool) -> crate::net::ChatKind {
    use crate::net::ChatKind as K;
    if !leads {
        return wire;
    }
    match wire {
        K::Party => K::PartyLeader,
        K::Raid => K::RaidLeader,
        K::Battleground => K::BattlegroundLeader,
        other => other,
    }
}

/// The join's profanity test (`0x50d9c7` -> `0x509a50`): a copy of the typed name through the
/// filter forced on; a match shows `ERR_PROFANE_CHAT_NAME` on the calling VM (`0x50d9d8`).
pub(crate) fn refuse_profane_channel_name(
    world: &mut World,
    vm: &benilla_ui::script::UiScript,
    name: &str,
) -> bool {
    let mut copy = name.to_string();
    let profane = world
        .get_resource_mut::<crate::text_filter::TextFilter>()
        .is_some_and(|mut f| f.mask(true, &mut copy));
    if profane {
        refuse(world, vm, "ERR_PROFANE_CHAT_NAME");
    }
    profane
}

/// `SendChatMessage`'s refusal line, shown in its call on the calling VM (`0x5216f0`).
fn refuse(world: &mut World, vm: &benilla_ui::script::UiScript, key: &'static str) {
    crate::ui_action::show_line_with_vm(
        world,
        &mut vm.share(),
        "chat",
        crate::ui_action::UiError::key(key),
    );
}

/// The battleground raid test (`0x572f50`): the current map's `InstanceType` is 3 or 4.
fn pvp_raid(world: &World) -> bool {
    // 12340 dereferences a null record for a map outside `Map.dbc` (`0x572f88` -> `0x572f7c`);
    // that map is no battleground here.
    let map = world
        .get_resource::<benilla_world::world_map::CurrentMap>()
        .map(|m| m.0);
    map.and_then(|m| {
        world
            .get_resource::<benilla_assets::MapCatalogRes>()
            .and_then(|c| c.0.instance_type(m))
    })
    .is_some_and(|t| matches!(t, 3 | 4))
}

/// Whether we speak `language` at the call (`0x6e0640` answering 1).
fn speaks_language(world: &mut World, language: u32) -> bool {
    world
        .run_system_cached_with(
            |In(language): In<u32>,
             me: Query<
                (
                    &crate::net::ObjectStore,
                    Option<&crate::unit_auras::UnitAuras>,
                ),
                With<crate::net::SelfPlayer>,
            >,
             actions: Option<Res<crate::ui_action::PlayerActions>>,
             spells: Option<Res<crate::ui_action::Spells>>,
             lines: Option<Res<crate::ui_spellbook::SkillLines>>| {
                let (Some((store, auras)), Some(actions), Some(spells), Some(lines)) = (
                    me.iter().next(),
                    actions.as_deref(),
                    spells.as_deref(),
                    lines.as_deref(),
                ) else {
                    return false;
                };
                crate::ui_chat::language_skill(
                    store,
                    auras,
                    &actions.spells,
                    spells,
                    lines,
                    language,
                )
                .is_some()
            },
            language,
        )
        .unwrap_or(false)
}

/// A global string off `vm`, read with no call into Lua.
fn vm_string(vm: Option<&benilla_ui::script::UiScript>, key: &str) -> Option<String> {
    vm.and_then(|s| crate::ui_chat::combat::global_string(s, key))
}

/// A handle on the `World`'s VM, for a caller outside any Lua call.
fn world_vm(world: &World) -> Option<benilla_ui::script::UiScript> {
    world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .map(benilla_ui::script::UiScript::share)
}

/// Send `text` on `kind`, the verb's packet out in its turn.
fn send_chat(
    world: &World,
    kind: crate::net::ChatKind,
    target: Option<String>,
    text: Vec<u8>,
    language: Option<u32>,
) {
    let Some(commands) = world.get_resource::<NetCommands>() else {
        return;
    };
    let cmd = ClientCommand::Chat {
        kind,
        target,
        text,
        language,
    };
    if commands.0.send(cmd).is_err() {
        warn!("chat: not connected; addon line dropped");
    }
}

/// The AFK clear (`0x6d52d0`): with the mirror set (`0x6d52d9`) and, called by a send,
/// `autoClearAFK` on (`0x6d52ea`), the mirror zeroes (`0x6d530e`), `CLEARED_AFK` is added in the
/// call (`0x6d531d`) and the empty AFK message goes out (`0x6d5373`).
pub(crate) fn clear_afk(world: &mut World, forced: bool) -> bool {
    let vm = world_vm(world);
    clear_afk_with(world, vm.as_ref(), forced)
}

/// [`clear_afk`] reading its line off `vm`, the calling VM inside a verb's call.
fn clear_afk_with(
    world: &mut World,
    vm: Option<&benilla_ui::script::UiScript>,
    forced: bool,
) -> bool {
    let set = world
        .get_resource::<super::away::AfkMirror>()
        .is_some_and(|m| m.is_afk());
    let gated = forced
        || world
            .get_resource::<crate::cvars::Cvars>()
            .is_none_or(auto_clear_afk);
    if !set || !gated {
        return false;
    }
    world.resource_mut::<super::away::AfkMirror>().0 = 0;
    let line = vm_string(vm, "CLEARED_AFK").unwrap_or_default();
    super::away::add_system(world, line);
    // Language 0 (`0x6d5356`), never the speaker's tongue.
    send_chat(world, crate::net::ChatKind::Afk, None, Vec::new(), Some(0));
    true
}

/// The AFK mark (`0x6dc640`): an empty message takes `DEFAULT_AFK_MESSAGE` (`0x6dc65b`). With the
/// mirror clear (`0x6dc66d`) it adds `CLEARED_DND` under a live DND bit (`0x6dc675`-`0x6dc6a0`), then
/// `MARKED_AFK_MESSAGE` (`0x6dc6e0`), then sets the mirror (`0x6dc6e8`); set, it adds nothing. The
/// AFK packet always goes out with the message (`0x6dc73d`).
pub(crate) fn set_afk(world: &mut World, msg: &[u8]) {
    let vm = world_vm(world);
    set_afk_with(world, vm.as_ref(), msg);
}

/// [`set_afk`] reading its strings off `vm`, the calling VM inside a verb's call.
fn set_afk_with(world: &mut World, vm: Option<&benilla_ui::script::UiScript>, msg: &[u8]) {
    let body = if msg.is_empty() {
        vm_string(vm, "DEFAULT_AFK_MESSAGE")
            .unwrap_or_default()
            .into_bytes()
    } else {
        msg.to_vec()
    };
    let mirror = world
        .get_resource::<super::away::AfkMirror>()
        .copied()
        .unwrap_or_default();
    if !mirror.is_afk() {
        let dnd = world
            .run_system_cached(
                |q: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>| is_dnd(&q),
            )
            .unwrap_or(false);
        if dnd {
            let line = vm_string(vm, "CLEARED_DND").unwrap_or_default();
            super::away::add_system(world, line);
        }
        let format = vm_string(vm, "MARKED_AFK_MESSAGE").unwrap_or_default();
        let line = super::away::fill_one_bytes(format.as_bytes(), &body);
        super::away::add_system(world, super::away::shown(&line));
        world.resource_mut::<super::away::AfkMirror>().0 = 1;
    }
    // Language 0 (`0x6dc724`), never the speaker's tongue.
    send_chat(world, crate::net::ChatKind::Afk, None, body, Some(0));
}

/// `SendChatMessage`'s body inside the call (`0x50d170`), past the binding's type, empty-line and
/// language reads. No slash grammar runs here: an addon sending `"/dance"` says six characters.
/// Every line it adds is added in the call; a `Some` is the verb's Lua error. `vm` is the calling
/// VM's handle.
pub(crate) fn send_chat_message(
    world: &mut World,
    vm: &benilla_ui::script::UiScript,
    mut send: benilla_ui::script::ChatSend,
) -> Option<&'static str> {
    use super::edit::SendType as T;
    let Some(kind) = T::from_token(&send.chat_type) else {
        return Some("SendChatMessage(): Unknown chat type");
    };
    if send.ends_at_empty_line() {
        return None;
    }
    // A named language needs our skill in it (`0x50d2b0`), else `ERR_CANT_SPEAK_LANGAGE` and the
    // call returns (`0x50d2b9`).
    if send.language.is_some_and(|l| !speaks_language(world, l)) {
        refuse(world, vm, "ERR_CANT_SPEAK_LANGAGE");
        return None;
    }
    let target_empty = send.target.as_deref().is_none_or(str::is_empty);
    let me = world
        .get_resource::<crate::net::SelfGuid>()
        .and_then(|g| g.0)
        .unwrap_or(0);
    // The group facts read at the call: party (`0x4fb0b0`), raid count (`[0xbeb608]`), officer
    // (`0x572740`), and the leader lanes' compare of the party leader with us (`0x50d4f9`,
    // `0x50d3fd`, `0x50d4cc`).
    let (in_party, in_raid, officer, leads) = world
        .get_resource::<crate::ui_party::GroupState>()
        .map_or((false, false, false, false), |g| {
            (
                g.in_party(),
                g.in_raid(),
                g.is_raid_officer(me),
                me != 0 && g.party_leader() == me,
            )
        });
    let mut wire = kind.wire();
    match kind {
        // A whisper with no target is the verb's error (`0x50d302`).
        T::Whisper if target_empty => {
            return Some("SendChatMessage(): Whisper message missing target player!");
        }
        // PARTY needs a party (`0x50d31e`) or a battleground raid (`0x50d32b`-`0x50d333`), else
        // `ERR_NOT_IN_GROUP` (`0x50d340`); our lead picks `0x33` (`0x50d516`).
        T::Party => {
            if !in_party && !(in_raid && pvp_raid(world)) {
                refuse(world, vm, "ERR_NOT_IN_GROUP");
                return None;
            }
            wire = party_lane(wire, leads);
        }
        // RAID_WARNING in a raid needs an officer (`0x50d376`), else `ERR_NOT_LEADER` (`0x50d386`);
        // outside one, a party (`0x50d399`), else `ERR_NOT_IN_GROUP` (`0x50d3a2`).
        T::RaidWarning => {
            if in_raid {
                if !officer {
                    refuse(world, vm, "ERR_NOT_LEADER");
                    return None;
                }
            } else if !in_party {
                refuse(world, vm, "ERR_NOT_IN_GROUP");
                return None;
            }
        }
        // RAID and BATTLEGROUND need a raid (`0x50d3c3`), else `ERR_NOT_IN_RAID` or
        // `ERR_NOT_IN_BATTLEGROUND` (`0x50d3cc`-`0x50d3dd`); our lead picks `0x27` (`0x50d412`) or
        // `0x2d` (`0x50d4e9`).
        T::Raid | T::Battleground => {
            if !in_raid {
                refuse(
                    world,
                    vm,
                    if kind == T::Raid {
                        "ERR_NOT_IN_RAID"
                    } else {
                        "ERR_NOT_IN_BATTLEGROUND"
                    },
                );
                return None;
            }
            wire = party_lane(wire, leads);
        }
        // A channel send with no target is the verb's error (`0x50d587`); a target naming no
        // joined record ends the call (`0x50d52f`-`0x50d569`), else it sends the record's name.
        T::Channel if target_empty => {
            return Some("SendChatMessage(): Channel send missing channel number");
        }
        T::Channel => {
            let name = world
                .get_resource::<super::edit::ChannelState>()
                .and_then(|c| send.target.as_deref().and_then(|t| c.send_target(t)));
            if name.is_none() {
                debug!(
                    "chat: SendChatMessage names no joined channel {:?}; not sent",
                    send.target
                );
                return None;
            }
            send.target = name;
        }
        _ => {}
    }
    // Every string reads the calling VM: the world-entry load holds the `World`'s VM outside it.
    let strings = |key: &str| vm_string(Some(vm), key);
    // AFK marks or clears and returns (`0x50d5a7`-`0x50d5ee`): no AFK clear, dead gate or tutorial.
    if wire == crate::net::ChatKind::Afk {
        let mirror = world
            .get_resource::<super::away::AfkMirror>()
            .copied()
            .unwrap_or_default();
        // A clear mirror's empty line takes `DEFAULT_AFK_MESSAGE` (`0x50d5b8`); a line still empty
        // is the forced AFK clear (`0x50d5e9`-`0x50d5ee`), any other the AFK mark (`0x50d5db`). The
        // AFK mark's own packet never reads the language argument.
        let text = if !mirror.is_afk() && send.text.is_empty() {
            strings("DEFAULT_AFK_MESSAGE")
                .unwrap_or_default()
                .into_bytes()
        } else {
            send.text
        };
        if text.is_empty() {
            clear_afk_with(world, Some(vm), true);
        } else {
            set_afk_with(world, Some(vm), &text);
        }
        return None;
    }
    // Every other type first clears a standing AFK (`0x50d417`-`0x50d425`), `/dnd` included.
    clear_afk_with(world, Some(vm), false);
    let text = if wire == crate::net::ChatKind::Dnd {
        // DND has no mirror: the live descriptor bit (`0x50d43c`).
        let dnd = world
            .run_system_cached(
                |q: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>| is_dnd(&q),
            )
            .unwrap_or(false);
        let out = super::away::dnd_line(&send.text, dnd, &strings);
        super::away::add_system(world, out.line);
        out.body
    } else {
        send.text
    };
    // Dead or a ghost, only the group, guild, whisper, channel and DND lanes speak; any other shows
    // `ERR_CHAT_WHILE_DEAD` and ends the call (`0x50d632`-`0x50d684`).
    if !speaks_while_dead(wire) && self_dead_or_ghost(world) {
        refuse(world, vm, "ERR_CHAT_WHILE_DEAD");
        return None;
    }
    // Every type that reaches the send flags the chatting tutorial (`0x50d695`).
    if let Some(mut t) = world.get_resource_mut::<Messages<crate::tutorial::TutorialEvent>>() {
        t.write(crate::tutorial::TutorialEvent::Acknowledge {
            id: crate::tutorial::id::CHATTING,
        });
    }
    // The line's passes (`0x50d6ac`-`0x50d7ae`); a bad escape is the verb's error (`0x50d7b1`).
    let Some(text) = send_text::sent_line(world, vm, &text) else {
        return Some(send_text::INVALID_ESCAPE);
    };
    send_chat(world, wire, send.target, text, send.language);
    None
}

/// The lanes `SendChatMessage` sends while dead or a ghost (`0x50d643`-`0x50d67d`).
fn speaks_while_dead(wire: crate::net::ChatKind) -> bool {
    use crate::net::ChatKind as K;
    matches!(
        wire,
        K::Whisper
            | K::Party
            | K::PartyLeader
            | K::Raid
            | K::RaidLeader
            | K::RaidWarning
            | K::Guild
            | K::Officer
            | K::Channel
            | K::Dnd
            | K::Battleground
            | K::BattlegroundLeader
    )
}

/// The active player's dead-or-ghost test, read at the call (`0x50d637`).
fn self_dead_or_ghost(world: &mut World) -> bool {
    world
        .run_system_cached(
            |q: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>| {
                q.iter().next().is_some_and(|s| s.0.is_dead_or_ghost())
            },
        )
        .unwrap_or(false)
}

/// `SendAddonMessage`'s gates and packet inside the call (`0x500678`-`0x50071f`): RAID outside a
/// raid goes as PARTY (`0x500663`-`0x50066c`), and PARTY with no party sends nothing (`0x500678`).
/// This writes the outbound half of the `addon` trace tag and `ui_chat::net` the inbound:
/// `WOW_MOVE_TRACE=<path> WOW_MOVE_TRACE_TAGS=addon` logs both.
pub(crate) fn send_addon_message(world: &World, mut send: benilla_ui::script::AddonSend) {
    use benilla_ui::script::AddonDistribution as D;
    // The party (`0x4fb0b0`'s word) and raid count (`[0xbeb608]`) read at the call.
    let (in_party, in_raid) = world
        .get_resource::<crate::ui_party::GroupState>()
        .map_or((false, false), |g| (g.in_party(), g.in_raid()));
    if send.distribution == D::Raid && !in_raid {
        send.distribution = D::Party;
    }
    if send.distribution == D::Party && !in_party {
        debug!("chat: addon message on PARTY with no party; not sent");
        return;
    }
    let text = String::from_utf8_lossy(&send.text);
    debug!(
        "chat: addon message on {} — {text:?}",
        send.distribution.token()
    );
    if benilla_assets::trace::enabled() {
        benilla_assets::trace::line(
            "addon",
            &format!("-> {} {text:?}", send.distribution.token()),
        );
    }
    let Some(commands) = world.get_resource::<NetCommands>() else {
        return;
    };
    let cmd = ClientCommand::AddonMessage {
        distribution: send.distribution,
        target: send.target,
        text: send.text,
    };
    if commands.0.send(cmd).is_err() {
        warn!("chat: not connected; addon message dropped");
    }
}

#[cfg(test)]
mod party_test_raid_tests {
    use super::*;

    /// The queued `/partytest raid` seats every synthetic member with its name record, which the raid rows read from the name cache (#1304).
    #[test]
    fn the_party_test_raid_rows_carry_their_names() {
        let mut world = World::new();
        world.init_resource::<crate::ui_party::GroupState>();
        world.init_resource::<crate::names::NameCache>();
        party_test_raid(&mut world, Some(0x19));
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = crate::net::NetCommands(tx);
        let group = world.resource::<crate::ui_party::GroupState>();
        let names = world.resource::<crate::names::NameCache>();
        assert_eq!(group.members.len(), 24, "every synthetic member, us aside");
        for m in &group.members {
            assert_eq!(
                names.resolve(m.guid, &commands),
                Some(m.name.as_str()),
                "{:#x}",
                m.guid
            );
        }
    }
}
