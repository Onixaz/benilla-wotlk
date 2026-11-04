//! Small server notices and what they drive: the light override, the server's UI clock, the arena
//! and refer-a-friend errors, the level grant offer, the inebriation lines, the message of the day,
//! a GM's whois and rwhois answers, realm-first awards and the one-line [`WorldNotice`]s. The dance
//! moves and dance-cache answers are read and dropped: nothing built reads them, as `PlayDance` is
//! not.

use std::time::{Duration, Instant};

use benilla_protocol::messages::{
    ChatMessage, InebriationThreshold, ObjectType, RWhois, ServerFirstAchievement, WorldNotice,
    CHAT_MSG_ACHIEVEMENT,
};
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

/// The message of the day (`0xbea7fc`), its lines joined by `\n`, bumped by each `SMSG_MOTD`, and
/// the `KNOWLEDGE_BASE_SYSTEM_MOTD_UPDATED` fires owed.
#[derive(Resource, Default)]
struct KbMotd {
    text: String,
    generation: u64,
    owed: u32,
}

/// Realm-first awards waiting for the VM's line templates.
#[derive(Resource, Default)]
struct ServerFirsts(Vec<ServerFirstAchievement>);

pub(crate) struct NoticesPlugin;

impl Plugin for NoticesPlugin {
    fn build(&self, app: &mut App) {
        use SessionEventKind as K;
        app.init_resource::<UiClock>()
            .init_resource::<LevelGrant>()
            .init_resource::<InebriationLines>()
            .init_resource::<KbMotd>()
            .init_resource::<ServerFirsts>()
            .net_handler(K::OverrideLight, on_override_light)
            .net_handler(K::UiTime, on_ui_time)
            .net_handler(K::ArenaError, on_arena_error)
            .net_handler(K::ReferAFriendFailure, on_refer_a_friend_failure)
            .net_handler(K::ProposeLevelGrant, on_propose_level_grant)
            .net_handler(K::InebriationThreshold, on_inebriation)
            .net_handler(K::LearnedDanceMoves, on_dance_moves)
            .net_handler(K::DanceRecord, on_dance_cache)
            .net_handler(K::InvalidateDance, on_dance_cache)
            .net_handler(K::Motd, on_motd)
            .net_handler(K::Whois, on_whois)
            .net_handler(K::RWhois, on_rwhois)
            .net_handler(K::ServerFirstAchievement, on_server_first)
            .net_handler(K::Notice, on_notice)
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

/// The line a [`WorldNotice`] shows, if any. The petition offer's name comes from the cache alone,
/// never asked for (`0x5cea37`); the AFK report shows none here ([`afk_report`]).
fn notice_line(notice: WorldNotice, names: &NameCache) -> Option<UiError> {
    Some(match notice {
        WorldNotice::PlayerBindError => UiError::key("ERR_DEATHBINDALREADYBOUND"),
        // `0x522180`: reasons 0-2, any other silent.
        WorldNotice::RemovedFromPvpQueue { reason } => UiError::key(match reason {
            0 => "ERR_REMOVE_FROM_PVP_QUEUE_GRANT_LEVEL",
            1 => "ERR_REMOVE_FROM_PVP_QUEUE_XP_GAIN",
            2 => "ERR_REMOVE_FROM_PVP_QUEUE_FACTION_CHANGE",
            _ => return None,
        }),
        WorldNotice::InvalidPromotionCode => UiError::key("ERR_INVALID_PROMOTION_CODE"),
        WorldNotice::OfferPetitionError { player } => {
            UiError::s("ERR_ALREADY_IN_ARENA_TEAM_S", names.peek(player)?)
        }
        WorldNotice::ArenaTeamChangeFailedQueued { failed } => {
            if failed == 0 {
                return None;
            }
            UiError::key("ERR_ARENA_TEAM_CHANGE_FAILED_QUEUED")
        }
        WorldNotice::BattlegroundInfoThrottled => UiError::key("ERR_BATTLEGROUND_INFO_THROTTLED"),
        WorldNotice::PlayTimeWarning { flags, seconds } => play_time_warning(flags, seconds)?,
        WorldNotice::ReportPvpAfkResult { .. } => return None,
    })
}

/// `SMSG_PLAY_TIME_WARNING` (`0x6cd2de`-`0x6cd367`): the first warning flag set names the time left
/// as `"%2d:%02d"` hours and minutes (`0x6cd2d3`); with none, the sign bit is the bare unhealthy
/// line.
fn play_time_warning(flags: u32, seconds: i32) -> Option<UiError> {
    let minutes = seconds / 60;
    let left = format!("{:2}:{:02}", minutes / 60, minutes % 60);
    for (bit, key) in [
        (0x1000, "ERR_APPROACHING_PARTIAL_PLAY_TIME"),
        (0x4000_0000, "ERR_APPROACHING_PARTIAL_PLAY_TIME_2"),
        (0x2000, "ERR_APPROACHING_NO_PLAY_TIME"),
        (0x2000_0000, "ERR_APPROACHING_NO_PLAY_TIME_2"),
    ] {
        if flags & bit != 0 {
            return Some(UiError::s(key, left));
        }
    }
    ((flags as i32) < 0).then(|| UiError::key("ERR_UNHEALTHY_TIME"))
}

/// `SMSG_REPORT_PVP_AFK_RESULT` (`0x6e3fe5`): result 5 turns `enablePVPNotifyAFK` on and 6 off,
/// each only from the other state, and the switch prints its system line's key.
fn afk_report(result: u8, enabled: bool) -> Option<(&'static str, &'static str)> {
    match (result, enabled) {
        (5, false) => Some(("1", "PVP_REPORT_AFK_SYSTEM_ENABLED")),
        (6, true) => Some(("0", "PVP_REPORT_AFK_SYSTEM_DISABLED")),
        _ => None,
    }
}

fn on_notice(
    In(ev): In<SessionEvent>,
    mut errors: ResMut<UiErrorKeys>,
    names: Res<NameCache>,
    cvars: Option<ResMut<crate::cvars::Cvars>>,
    script: Option<NonSend<UiScript>>,
    mut chat: ResMut<ChatLog>,
) {
    let SessionEvent::Notice(notice) = ev else {
        return;
    };
    debug!("net: {notice:?}");
    if let WorldNotice::ReportPvpAfkResult { result } = notice {
        let Some(mut cvars) = cvars else {
            return;
        };
        let enabled = cvars.flag("enablePVPNotifyAFK") == Some(true);
        if let Some((value, key)) = afk_report(result, enabled) {
            cvars.set("enablePVPNotifyAFK", value);
            // The VM's text lookup shows a missing key as itself (`0x819d40`).
            let text = script
                .as_deref()
                .and_then(|s| crate::ui_chat::combat::global_string(s, key))
                .unwrap_or_else(|| key.to_string());
            crate::ui_chat::away::push_system(&mut chat, text);
        }
        return;
    }
    errors.0.extend(notice_line(notice, &names));
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

/// `SMSG_DANCE_QUERY_RESPONSE` (`0x635ab0`) and `SMSG_INVALIDATE_DANCE` (`0x6354f0`) fill and trim
/// the cache only the unbuilt dance studio reads.
fn on_dance_cache(In(ev): In<SessionEvent>) {
    match ev {
        SessionEvent::DanceRecord { id, record } => {
            debug!(
                "net: dance {id} {} — no dance studio, dropped",
                if record.is_some() {
                    "answered"
                } else {
                    "unknown"
                }
            );
        }
        SessionEvent::InvalidateDance { id } => {
            debug!("net: dance {id} invalidated — no dance studio, nothing cached");
        }
        _ => {}
    }
}

/// `SMSG_MOTD` (`0x551660`): each line a `CHAT_MSG_SYSTEM` line whose arg6 is `"MOTD"`, the lines
/// kept for `KBSystem_GetMOTD`, then `KNOWLEDGE_BASE_SYSTEM_MOTD_UPDATED`.
fn on_motd(In(ev): In<SessionEvent>, mut motd: ResMut<KbMotd>, mut chat: ResMut<ChatLog>) {
    if let SessionEvent::Motd(lines) = ev {
        debug!("net: message of the day, {} line(s)", lines.len());
        for line in &lines {
            chat.push_event(motd_line(line));
        }
        motd.text = lines.join("\n");
        motd.generation += 1;
        motd.owed += 1;
    }
}

fn motd_line(line: &str) -> ChatEvent {
    ChatEvent {
        flag: "MOTD".into(),
        ..ChatEvent::text_only(ChatEventKind::System, line.to_string())
    }
}

/// `SMSG_WHOIS` (`0x6b3280`) writes its text to the console, which prints to the log here.
fn on_whois(In(ev): In<SessionEvent>, echo: Option<ResMut<crate::console::ConsoleEcho>>) {
    if let SessionEvent::Whois(text) = ev {
        match echo {
            Some(mut echo) => echo.print(text),
            None => info!("console: {text}"),
        }
    }
}

/// `SMSG_RWHOIS` (`0x6b32c0`) goes to the console alone.
fn on_rwhois(In(ev): In<SessionEvent>, mut echo: Option<ResMut<crate::console::ConsoleEcho>>) {
    if let SessionEvent::RWhois(answer) = ev {
        for line in rwhois_lines(&answer) {
            match echo.as_mut() {
                Some(echo) => echo.print(line),
                None => info!("console: {line}"),
            }
        }
    }
}

/// The rwhois console lines: one for a failure or no match, else per account its name, then each
/// line. Deviation: the wording is benilla's own, and the highlighted line (`0x6b33a5`) is marked
/// with `>` where the reference colours it, the echo having no colour.
fn rwhois_lines(answer: &RWhois) -> Vec<String> {
    match answer {
        RWhois::Failed => vec!["rwhois: the lookup failed".into()],
        RWhois::NotFound => vec!["rwhois: no account found".into()],
        RWhois::Accounts(accounts) => accounts
            .iter()
            .flat_map(|a| {
                let lines = a.lines.iter().enumerate().map(|(i, line)| {
                    let mark = if i as u32 == a.highlight { '>' } else { ' ' };
                    format!("{mark} {line}")
                });
                std::iter::once(format!("rwhois: account {}", a.account)).chain(lines)
            })
            .collect(),
    }
}

fn on_server_first(In(ev): In<SessionEvent>, mut firsts: ResMut<ServerFirsts>) {
    if let SessionEvent::ServerFirstAchievement(a) = ev {
        debug!(
            "net: server first {} by {:?} (linked {})",
            a.achievement, a.name, a.linked
        );
        firsts.0.push(a);
    }
}

/// The realm-first line (`0x50b010`): `PLAYER_SERVER_FIRST_ACHIEVEMENT` with the name twice when
/// it links, else `SERVER_FIRST_ACHIEVEMENT` with it once, as `CHAT_MSG_ACHIEVEMENT` from the
/// earner to the earner, whose `$a` names the award.
fn server_first_line(
    get: &dyn Fn(&str) -> Option<String>,
    a: &ServerFirstAchievement,
) -> Option<ChatMessage> {
    use benilla_ui::strings::{fill, Arg};
    let name = Arg::S(&a.name);
    let text = if a.linked {
        fill(&get("PLAYER_SERVER_FIRST_ACHIEVEMENT")?, &[name, name])
    } else {
        fill(&get("SERVER_FIRST_ACHIEVEMENT")?, &[name])
    };
    Some(ChatMessage {
        chat_type: CHAT_MSG_ACHIEVEMENT,
        language: 0,
        sender_guid: a.guid,
        target_guid: a.guid,
        sender_name: Some(a.name.clone()),
        channel: None,
        text,
        chat_tag: 0,
        achievement: a.achievement,
    })
}

/// The session's end zeroes the clock (`0x548570`) and drops the offer (`0x528c30`).
fn on_session_end(
    In(_): In<SessionEvent>,
    mut clock: ResMut<UiClock>,
    mut grant: ResMut<LevelGrant>,
    mut lines: ResMut<InebriationLines>,
    mut firsts: ResMut<ServerFirsts>,
) {
    *clock = UiClock::default();
    *grant = LevelGrant::default();
    lines.0.clear();
    firsts.0.clear();
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

/// Push the UI clock offset and the message of the day, announce a pending level grant once its
/// granter is named, and post the realm-first lines and the inebriation lines whose inputs have
/// landed.
#[allow(clippy::too_many_arguments)]
fn feed_notices(
    script: Option<NonSendMut<UiScript>>,
    clock: Res<UiClock>,
    mut grant: ResMut<LevelGrant>,
    mut lines: ResMut<InebriationLines>,
    (mut motd, mut firsts): (ResMut<KbMotd>, ResMut<ServerFirsts>),
    names: Res<NameCache>,
    items: Res<Items>,
    net: Res<NetCommands>,
    self_guid: Res<SelfGuid>,
    objects: crate::net::Objects,
    mut chat: ResMut<ChatLog>,
    mut pushed: Local<crate::ui_script::VmMemo<(Option<i32>, u64)>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let pushed = pushed.get(&script);
    if pushed.0 != Some(clock.offset) {
        pushed.0 = Some(clock.offset);
        script.set_ui_time_offset(clock.offset);
    }
    if pushed.1 != motd.generation {
        pushed.1 = motd.generation;
        script.set_kb_motd(&motd.text);
    }
    for _ in 0..std::mem::take(&mut motd.owed) {
        script.fire_event("KNOWLEDGE_BASE_SYSTEM_MOTD_UPDATED", Vec::new());
    }
    if !firsts.0.is_empty() {
        let lua = script.lua();
        let get = |key: &str| benilla_ui::strings::global(lua, key);
        for a in std::mem::take(&mut firsts.0) {
            if let Some(msg) = server_first_line(&get, &a) {
                chat.push_wire(msg);
            }
        }
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

    #[test]
    fn rwhois_prints_its_arms_and_marks_the_highlighted_line() {
        assert_eq!(rwhois_lines(&RWhois::Failed).len(), 1);
        assert_eq!(rwhois_lines(&RWhois::NotFound).len(), 1);
        assert!(rwhois_lines(&RWhois::Accounts(Vec::new())).is_empty());
        let account = benilla_protocol::messages::RWhoisAccount {
            account: "ACC".into(),
            lines: vec!["one".into(), "two".into()],
            highlight: 1,
        };
        assert_eq!(
            rwhois_lines(&RWhois::Accounts(vec![account])),
            ["rwhois: account ACC", "  one", "> two"]
        );
    }

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

    /// `0x6e3492`, `0x522180`, `0x526656`, `0x5cea46`, `0x6e40b7`, `0x6cbea0`: each notice's line,
    /// the petition offer's only with a cached name and the queue removal's only for reasons 0-2.
    #[test]
    fn each_world_notice_shows_its_line() {
        let mut names = NameCache::default();
        names.insert_player(0x2A, "Brisca".into(), None);
        let line = |n| notice_line(n, &names);
        use WorldNotice as N;
        assert_eq!(
            line(N::PlayerBindError),
            Some(UiError::key("ERR_DEATHBINDALREADYBOUND"))
        );
        for (reason, key) in [
            (0, "ERR_REMOVE_FROM_PVP_QUEUE_GRANT_LEVEL"),
            (1, "ERR_REMOVE_FROM_PVP_QUEUE_XP_GAIN"),
            (2, "ERR_REMOVE_FROM_PVP_QUEUE_FACTION_CHANGE"),
        ] {
            assert_eq!(
                line(N::RemovedFromPvpQueue { reason }),
                Some(UiError::key(key))
            );
        }
        assert_eq!(line(N::RemovedFromPvpQueue { reason: 3 }), None);
        assert_eq!(
            line(N::InvalidPromotionCode),
            Some(UiError::key("ERR_INVALID_PROMOTION_CODE"))
        );
        assert_eq!(
            line(N::OfferPetitionError { player: 0x2A }),
            Some(UiError::s("ERR_ALREADY_IN_ARENA_TEAM_S", "Brisca"))
        );
        assert_eq!(line(N::OfferPetitionError { player: 0x2B }), None);
        assert_eq!(
            line(N::ArenaTeamChangeFailedQueued { failed: 1 }),
            Some(UiError::key("ERR_ARENA_TEAM_CHANGE_FAILED_QUEUED"))
        );
        assert_eq!(line(N::ArenaTeamChangeFailedQueued { failed: 0 }), None);
        assert_eq!(
            line(N::BattlegroundInfoThrottled),
            Some(UiError::key("ERR_BATTLEGROUND_INFO_THROTTLED"))
        );
        assert_eq!(line(N::ReportPvpAfkResult { result: 5 }), None);
    }

    /// `0x6cd2de`-`0x6cd367`: the first flag in the handler's order wins, the seconds read as
    /// `"%2d:%02d"`, and the sign bit alone is the bare unhealthy line.
    #[test]
    fn a_play_time_warning_names_the_time_left_by_its_first_flag() {
        assert_eq!(
            play_time_warning(0x1000 | 0x2000, 3_900),
            Some(UiError::s("ERR_APPROACHING_PARTIAL_PLAY_TIME", " 1:05"))
        );
        assert_eq!(
            play_time_warning(0x4000_0000, 45_000),
            Some(UiError::s("ERR_APPROACHING_PARTIAL_PLAY_TIME_2", "12:30"))
        );
        assert_eq!(
            play_time_warning(0x2000, 59),
            Some(UiError::s("ERR_APPROACHING_NO_PLAY_TIME", " 0:00"))
        );
        assert_eq!(
            play_time_warning(0x2000_0000 | 0x8000_0000, 600),
            Some(UiError::s("ERR_APPROACHING_NO_PLAY_TIME_2", " 0:10"))
        );
        assert_eq!(
            play_time_warning(0x8000_0000, 600),
            Some(UiError::key("ERR_UNHEALTHY_TIME"))
        );
        assert_eq!(play_time_warning(0x1, 600), None);
    }

    /// `0x6e3fe5`-`0x6e4040`: each result flips the CVar only from the other state.
    #[test]
    fn the_afk_report_result_switches_the_cvar_once() {
        assert_eq!(
            afk_report(5, false),
            Some(("1", "PVP_REPORT_AFK_SYSTEM_ENABLED"))
        );
        assert_eq!(afk_report(5, true), None);
        assert_eq!(
            afk_report(6, true),
            Some(("0", "PVP_REPORT_AFK_SYSTEM_DISABLED"))
        );
        assert_eq!(afk_report(6, false), None);
        assert_eq!(afk_report(4, false), None);
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

    /// `0x50b010`: the linked form fills the name twice, the plain form once; both keep `$a` for
    /// the achievement lane's expansion, from and about the earner.
    #[test]
    fn a_server_first_formats_by_its_link_flag() {
        let get = |key: &str| {
            Some(
                match key {
                    "PLAYER_SERVER_FIRST_ACHIEVEMENT" => "[%s|%s] $a",
                    "SERVER_FIRST_ACHIEVEMENT" => "<%s> $a",
                    _ => return None,
                }
                .to_string(),
            )
        };
        let mut a = ServerFirstAchievement {
            name: "Bob".into(),
            guid: 0x2A,
            achievement: 457,
            linked: true,
        };
        let msg = server_first_line(&get, &a).unwrap();
        assert_eq!(msg.text, "[Bob|Bob] $a");
        assert_eq!(msg.chat_type, CHAT_MSG_ACHIEVEMENT);
        assert_eq!((msg.sender_guid, msg.target_guid), (0x2A, 0x2A));
        assert_eq!(msg.sender_name.as_deref(), Some("Bob"));
        assert_eq!(msg.achievement, 457);
        a.linked = false;
        assert_eq!(server_first_line(&get, &a).unwrap().text, "<Bob> $a");
        assert!(server_first_line(&|_: &str| None, &a).is_none());
    }

    /// `0x551660`: a system line flagged `MOTD`, and the joined message for `KBSystem_GetMOTD`.
    #[test]
    fn the_motd_prints_each_line_and_keeps_the_whole() {
        use bevy::ecs::system::RunSystemOnce;
        let line = motd_line("Hello");
        assert_eq!(line.kind, Some(ChatEventKind::System));
        assert_eq!((line.text.as_str(), line.flag.as_str()), ("Hello", "MOTD"));
        let mut w = World::new();
        w.init_resource::<KbMotd>();
        w.init_resource::<ChatLog>();
        w.run_system_once_with(on_motd, SessionEvent::Motd(vec!["a".into(), "b".into()]))
            .unwrap();
        let motd = w.resource::<KbMotd>();
        assert_eq!(
            (motd.text.as_str(), motd.generation, motd.owed),
            ("a\nb", 1, 1)
        );
    }
}
