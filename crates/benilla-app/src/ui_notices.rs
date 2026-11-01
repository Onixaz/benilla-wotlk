//! Small server notices and what they drive: the light override, the server's UI clock, the arena
//! and refer-a-friend errors, the level grant offer and the inebriation lines. The dance moves are
//! read and dropped: nothing built reads them, as `PlayDance` is not.

use std::time::{Duration, Instant};

use benilla_protocol::messages::{InebriationThreshold, ObjectType};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{ScriptValue, UiScript};
use bevy::prelude::*;

use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp, NetStatus, SelfGuid};
use crate::ui_action::{FillArg, UiError, UiErrorKeys};
use crate::ui_chat::{ChatEvent, ChatEventKind, ChatLog};
use crate::ui_script::{UiFeed, UiInput};

/// How often the UI clock is asked for in world (`0x5488f0`: `now + 20000`).
const UI_TIME_REQUEST_EVERY: Duration = Duration::from_secs(20);

/// Frames an inebriation line waits for its name or item before it is dropped.
const LINE_MAX_TRIES: u16 = 120;

/// The server's UI clock less the local clock, and when it is asked for next; reset with the
/// session, as `0x548570` zeroes both at world init.
#[derive(Resource, Default)]
pub(crate) struct UiClock {
    offset: i32,
    next_request: Option<Instant>,
}

impl UiClock {
    /// The UI clock now, Unix seconds (`0x548700`): `time()` plus the offset.
    pub(crate) fn now(&self) -> i32 {
        unix_now().wrapping_add(self.offset)
    }
}

fn unix_now() -> i32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i32)
}

/// The pending refer-a-friend granter (`[CGGameUI::m_pendingLevelGranter]`) and whether
/// `LEVEL_GRANT_PROPOSED` has gone out for it.
#[derive(Resource, Default)]
struct LevelGrant {
    granter: Option<u64>,
    announced: bool,
}

/// Inebriation lines waiting for their roller's name or item template.
#[derive(Resource, Default)]
struct InebriationLines(Vec<(InebriationThreshold, u16)>);

pub(crate) struct NoticesPlugin;

impl Plugin for NoticesPlugin {
    fn build(&self, app: &mut App) {
        use SessionEventKind as K;
        app.init_resource::<UiClock>()
            .init_resource::<LevelGrant>()
            .init_resource::<InebriationLines>()
            .net_handler(K::OverrideLight, on_override_light)
            .net_handler(K::UiTime, on_ui_time)
            .net_handler(K::ArenaError, on_arena_error)
            .net_handler(K::ReferAFriendFailure, on_refer_a_friend_failure)
            .net_handler(K::ProposeLevelGrant, on_propose_level_grant)
            .net_handler(K::InebriationThreshold, on_inebriation)
            .net_handler(K::LearnedDanceMoves, on_dance_moves)
            .net_handler(K::Disconnected, on_session_end)
            .add_systems(
                Update,
                (
                    request_ui_time
                        .after(benilla_world::schedule::WorldStage::Net)
                        .before(UiFeed),
                    feed_notices
                        .in_set(UiFeed)
                        .run_if(crate::ui_script::ingame_ui_up),
                    drain_level_grant.after(UiInput),
                ),
            );
    }
}

/// `SMSG_OVERRIDE_LIGHT` (`0x526530`): the lighting blend takes it over the transition.
fn on_override_light(
    In(ev): In<SessionEvent>,
    mut overrides: ResMut<benilla_world::lighting::LightOverrides>,
) {
    if let SessionEvent::OverrideLight(p) = ev {
        debug!(
            "net: light {} overridden by {} over {} ms",
            p.light, p.override_id, p.transition_ms
        );
        overrides.apply(p.light, p.override_id, p.transition_ms);
    }
}

/// `SMSG_WORLD_STATE_UI_TIMER_UPDATE` (`0x5486e0`): the offset to the local clock.
fn on_ui_time(In(ev): In<SessionEvent>, mut clock: ResMut<UiClock>) {
    if let SessionEvent::UiTime(time) = ev {
        clock.offset = (time as i32).wrapping_sub(unix_now());
    }
}

/// `SMSG_ARENA_ERROR` (`0x6cce90`): code 0 names the team type twice, 1 and 2 are bare, any other
/// shows nothing.
fn arena_error(error: u32, team_type: Option<u8>) -> Option<UiError> {
    match error {
        0 => {
            let t = i64::from(team_type.unwrap_or(0));
            Some(UiError::args(
                "ERR_ARENA_NO_TEAM_II",
                vec![FillArg::D(t), FillArg::D(t)],
            ))
        }
        1 => Some(UiError::key("ERR_ARENA_EXPIRED_CAIS")),
        2 => Some(UiError::key("ERR_LFG_CANT_USE_BATTLEGROUND")),
        _ => None,
    }
}

fn on_arena_error(In(ev): In<SessionEvent>, mut errors: ResMut<UiErrorKeys>) {
    if let SessionEvent::ArenaError { error, team_type } = ev {
        errors.0.extend(arena_error(error, team_type));
    }
}

/// `SMSG_REFER_A_FRIEND_FAILURE` (`0x526530` -> `0x5219e0`): codes 1-13; the two level caps read
/// 60, and 9 and 13 name a player, the wire carrying a name for 9 only.
fn refer_a_friend_error(error: u32, name: &str) -> Option<UiError> {
    let level_cap = |key| UiError::args(key, vec![FillArg::D(60)]);
    Some(match error {
        1 => UiError::key("ERR_REFER_A_FRIEND_NOT_REFERRED_BY"),
        2 => UiError::key("ERR_REFER_A_FRIEND_TARGET_TOO_HIGH"),
        3 => UiError::key("ERR_REFER_A_FRIEND_INSUFFICIENT_GRANTABLE_LEVELS"),
        4 => UiError::key("ERR_REFER_A_FRIEND_TOO_FAR"),
        5 => UiError::key("ERR_REFER_A_FRIEND_DIFFERENT_FACTION"),
        6 => UiError::key("ERR_REFER_A_FRIEND_NOT_NOW"),
        7 => level_cap("ERR_REFER_A_FRIEND_GRANT_LEVEL_MAX_I"),
        8 => UiError::key("ERR_GENERIC_NO_TARGET"),
        9 => UiError::s("ERR_TARGET_NOT_IN_GROUP_S", name),
        10 => level_cap("ERR_REFER_A_FRIEND_SUMMON_LEVEL_MAX_I"),
        11 => UiError::key("ERR_REFER_A_FRIEND_SUMMON_COOLDOWN"),
        12 => UiError::key("ERR_REFER_A_FRIEND_INSUF_EXPAN_LVL"),
        13 => UiError::s("ERR_REFER_A_FRIEND_SUMMON_OFFLINE_S", name),
        _ => return None,
    })
}

fn on_refer_a_friend_failure(In(ev): In<SessionEvent>, mut errors: ResMut<UiErrorKeys>) {
    if let SessionEvent::ReferAFriendFailure { error, name } = ev {
        errors.0.extend(refer_a_friend_error(error, &name));
    }
}

/// `SMSG_PROPOSE_LEVEL_GRANT` (`0x513fd0`): an offer while one is pending is ignored.
fn on_propose_level_grant(In(ev): In<SessionEvent>, mut grant: ResMut<LevelGrant>) {
    if let SessionEvent::ProposeLevelGrant { granter } = ev {
        if grant.granter.is_none() {
            *grant = LevelGrant {
                granter: Some(granter),
                announced: false,
            };
        }
    }
}

fn on_inebriation(In(ev): In<SessionEvent>, mut lines: ResMut<InebriationLines>) {
    if let SessionEvent::InebriationThreshold(p) = ev {
        lines.0.push((p, 0));
    }
}

/// `SMSG_LEARNED_DANCE_MOVES` (`0x5758a0`) stores the mask only the unbuilt dance studio reads.
fn on_dance_moves(In(ev): In<SessionEvent>) {
    if let SessionEvent::LearnedDanceMoves(mask) = ev {
        debug!("net: learned dance moves {mask:#x} — no dance studio, dropped");
    }
}

/// The session's end zeroes the clock (`0x548570`) and drops the offer (`0x528c30`).
fn on_session_end(
    In(_): In<SessionEvent>,
    mut clock: ResMut<UiClock>,
    mut grant: ResMut<LevelGrant>,
    mut lines: ResMut<InebriationLines>,
) {
    *clock = UiClock::default();
    *grant = LevelGrant::default();
    lines.0.clear();
}

/// `CMSG_WORLD_STATE_UI_TIMER_UPDATE` every 20 s in world, the first at once (`0x5488f0`).
fn request_ui_time(
    mut clock: ResMut<UiClock>,
    status: Res<NetStatus>,
    self_guid: Res<SelfGuid>,
    net: Res<NetCommands>,
) {
    if !status.connected || self_guid.0.is_none() {
        return;
    }
    let now = Instant::now();
    if clock.next_request.is_none_or(|at| now >= at) {
        clock.next_request = Some(now + UI_TIME_REQUEST_EVERY);
        let _ = net.0.send(ClientCommand::UiTimeRequest);
    }
}

/// The inebriation line (`0x6cfff0`), `CHAT_MSG_SYSTEM`: with an item, `DRUNK_MESSAGE_ITEM_*` and
/// its link, else `DRUNK_MESSAGE_*`; ours takes `_SELF`, another's `_OTHER` with its name. The
/// suffix is the threshold plus one.
fn inebriation_line(
    get: &dyn Fn(&str) -> Option<String>,
    threshold: u32,
    is_self: bool,
    name: &str,
    link: Option<&str>,
) -> Option<String> {
    use benilla_ui::strings::{fill, Arg};
    let n = threshold + 1;
    let (key, args): (String, Vec<Arg>) = match (link, is_self) {
        (Some(link), true) => (format!("DRUNK_MESSAGE_ITEM_SELF{n}"), vec![Arg::S(link)]),
        (Some(link), false) => (
            format!("DRUNK_MESSAGE_ITEM_OTHER{n}"),
            vec![Arg::S(name), Arg::S(link)],
        ),
        (None, true) => (format!("DRUNK_MESSAGE_SELF{n}"), vec![Arg::S(name)]),
        (None, false) => (format!("DRUNK_MESSAGE_OTHER{n}"), vec![Arg::S(name)]),
    };
    get(&key).map(|t| fill(&t, &args))
}

/// Push the UI clock offset, announce a pending level grant once its granter is named, and post
/// the inebriation lines whose inputs have landed.
#[allow(clippy::too_many_arguments)]
fn feed_notices(
    script: Option<NonSendMut<UiScript>>,
    clock: Res<UiClock>,
    mut grant: ResMut<LevelGrant>,
    mut lines: ResMut<InebriationLines>,
    names: Res<NameCache>,
    items: Res<Items>,
    net: Res<NetCommands>,
    self_guid: Res<SelfGuid>,
    objects: crate::net::Objects,
    mut chat: ResMut<ChatLog>,
    mut pushed: Local<crate::ui_script::VmMemo<Option<i32>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let pushed = pushed.get(&script);
    if *pushed != Some(clock.offset) {
        *pushed = Some(clock.offset);
        script.set_ui_time_offset(clock.offset);
    }

    // `0x513fd0`: the event carries the granter's name, from its query's callback on a miss.
    if let Some(granter) = grant.granter.filter(|_| !grant.announced) {
        if let Some(name) = names.resolve(granter, &net) {
            let name = name.to_string();
            grant.announced = true;
            script.fire_event("LEVEL_GRANT_PROPOSED", vec![ScriptValue::Str(name)]);
        }
    }

    if lines.0.is_empty() {
        return;
    }
    let pending = std::mem::take(&mut lines.0);
    for (p, tries) in pending {
        // The line needs the player object; one we do not hold drops it (`0x6cfff0`).
        let is_player = objects
            .object(p.player)
            .is_some_and(|o| o.object_type() == Some(ObjectType::Player));
        if !is_player {
            continue;
        }
        let is_self = self_guid.0 == Some(p.player);
        let template = (p.item != 0)
            .then(|| items.template(p.item, 0, &net).cloned())
            .flatten();
        let name = names.resolve(p.player, &net).map(str::to_string);
        let ready = name.is_some() && (p.item == 0 || template.is_some());
        if !ready {
            if tries + 1 < LINE_MAX_TRIES {
                lines.0.push((p, tries + 1));
            }
            continue;
        }
        let link = template
            .as_ref()
            .map(|t| crate::ui_items::item_link(p.item, &t.name, t.quality));
        let text = {
            let lua = script.lua();
            let get = |key: &str| benilla_ui::strings::global(lua, key);
            inebriation_line(
                &get,
                p.threshold,
                is_self,
                name.as_deref().unwrap_or_default(),
                link.as_deref(),
            )
        };
        if let Some(text) = text {
            chat.push_event(ChatEvent::text_only(ChatEventKind::System, text));
        }
    }
}

/// `AcceptLevelGrant` sends `CMSG_ACCEPT_LEVEL_GRANT` to the pending granter and forgets it
/// (`0x521b00`); `DeclineLevelGrant` only forgets it (`0x517d20`).
fn drain_level_grant(
    script: Option<NonSendMut<UiScript>>,
    mut grant: ResMut<LevelGrant>,
    net: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    for accept in script.take_level_grant_answers() {
        let Some(granter) = grant.granter.take() else {
            continue;
        };
        grant.announced = false;
        if accept {
            let _ = net.0.send(ClientCommand::AcceptLevelGrant { granter });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(e: &UiError) -> Option<String> {
        let get = |key: &str| {
            Some(
                match key {
                    "ERR_ARENA_NO_TEAM_II" => "You are not in a %dv%d arena team.",
                    "ERR_REFER_A_FRIEND_GRANT_LEVEL_MAX_I" => {
                        "You cannot grant levels to players level %d or higher"
                    }
                    "ERR_TARGET_NOT_IN_GROUP_S" => "%s is not in your party.",
                    _ => return None,
                }
                .to_string(),
            )
        };
        crate::ui_action::ui_error_text(e, &get)
    }

    /// `0x6cce90` and `0x5219e0`: each code's message and arguments.
    #[test]
    fn the_error_codes_take_their_messages() {
        assert_eq!(
            text(&arena_error(0, Some(3)).unwrap()).as_deref(),
            Some("You are not in a 3v3 arena team.")
        );
        assert_eq!(arena_error(1, None).unwrap().key, "ERR_ARENA_EXPIRED_CAIS");
        assert!(arena_error(7, None).is_none());
        assert_eq!(
            text(&refer_a_friend_error(7, "").unwrap()).as_deref(),
            Some("You cannot grant levels to players level 60 or higher")
        );
        assert_eq!(
            text(&refer_a_friend_error(9, "Bob").unwrap()).as_deref(),
            Some("Bob is not in your party.")
        );
        assert!(refer_a_friend_error(0, "").is_none());
        assert!(refer_a_friend_error(14, "").is_none());
    }

    /// `0x6cfff0`'s four forms.
    #[test]
    fn the_inebriation_line_picks_its_form() {
        let get = |key: &str| {
            Some(
                match key {
                    "DRUNK_MESSAGE_SELF2" => "You feel tipsy.  Whee!",
                    "DRUNK_MESSAGE_OTHER3" => "%s looks drunk.",
                    "DRUNK_MESSAGE_ITEM_SELF4" => "You feel completely smashed after that %s.",
                    "DRUNK_MESSAGE_ITEM_OTHER1" => "%s is looking sober from the %s.",
                    _ => return None,
                }
                .to_string(),
            )
        };
        assert_eq!(
            inebriation_line(&get, 1, true, "Me", None).as_deref(),
            Some("You feel tipsy.  Whee!")
        );
        assert_eq!(
            inebriation_line(&get, 2, false, "Bob", None).as_deref(),
            Some("Bob looks drunk.")
        );
        assert_eq!(
            inebriation_line(&get, 3, true, "Me", Some("[Ale]")).as_deref(),
            Some("You feel completely smashed after that [Ale].")
        );
        assert_eq!(
            inebriation_line(&get, 0, false, "Bob", Some("[Ale]")).as_deref(),
            Some("Bob is looking sober from the [Ale].")
        );
    }

    /// `0x513fd0` keeps the first offer; the answer sends only an accept, to that granter.
    #[test]
    fn the_first_offer_holds_and_only_an_accept_sends() {
        use bevy::ecs::system::RunSystemOnce;
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut w = World::new();
        w.init_resource::<LevelGrant>();
        w.insert_resource(NetCommands(tx));
        for granter in [0x2A, 0x2B] {
            w.run_system_once_with(
                on_propose_level_grant,
                SessionEvent::ProposeLevelGrant { granter },
            )
            .unwrap();
        }
        assert_eq!(w.resource::<LevelGrant>().granter, Some(0x2A));
        let script = UiScript::new().unwrap();
        script
            .run("DeclineLevelGrant() AcceptLevelGrant()")
            .unwrap();
        w.insert_non_send_resource(script);
        w.run_system_once(drain_level_grant).unwrap();
        assert!(
            rx.try_iter().next().is_none(),
            "declined first, then none pending"
        );
        assert_eq!(w.resource::<LevelGrant>().granter, None);
    }
}
