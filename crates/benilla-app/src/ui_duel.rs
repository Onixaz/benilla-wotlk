//! Duels: the challenge, the countdown, the bounds and the outcome line, after 12340's handlers
//! (registered at `0x5cfdd0`), each firing its events and adding its lines as it applies its
//! packet. The challenge (`0x5cfcd0`) reaches both players, and the challenger's own copy accepts
//! at once (`0x5cfd12`). Bounds (`0x5cf910`, `0x5cf930`) only fire events: the server enforces them.

use benilla_ui::script::{DuelRequest, ScriptValue, UiScript};
use bevy::prelude::*;

use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::target::{NameMatch, PlayerLookup};
use crate::ui_action::{show_in_turn, Spells, UiError};
use crate::ui_script::live::{LineArg, UiEvents};
use crate::ui_script::UiDrain;

/// The `Effect[0]` that marks the duel spell: the spell-learned walk keeps such a spell in
/// `[0xbe7d90]` (`0x5420ac`), which `StartDuel` casts (`0x5cf9a1`).
const SPELL_EFFECT_DUEL: u32 = 83;

/// The duel session the handlers write.
#[derive(Resource, Default)]
pub(crate) struct DuelState {
    /// The duel arbiter (`0xc22b58`), cleared by completion or an ignored challenge; 0 is no duel.
    pub(crate) arbiter: u64,
    /// Another player's challenge (`0xc22b60`), stored after `DUEL_REQUESTED` (`0x5cfd99`).
    dueler: Option<u64>,
    countdown: Option<Countdown>,
}

/// The duel countdown timer (`0x5cf870`): the seconds its next line shows.
struct Countdown {
    remaining: u32,
    tick: Timer,
}

impl DuelState {
    /// The duel's world leave (`0x5cfa10`): the arbiter and the countdown timer go.
    pub(crate) fn leave_world(&mut self) {
        self.arbiter = 0;
        self.countdown = None;
    }

    /// The partner probe's accept hook (`WOW_PROBE=partner`): the challenge it answers.
    pub(crate) fn take_challenger(&mut self) -> Option<u64> {
        self.dueler.take()
    }
}

/// The countdown's line (`0x5cf870`): `DUEL_COUNTDOWN` ("Duel starting: %d") as a system line.
fn countdown_line(ui: &mut UiEvents, seconds: u32) {
    ui.system_line("DUEL_COUNTDOWN", vec![LineArg::D(i64::from(seconds))]);
}

/// The duel's packet handlers, each firing its events once its changes apply.
pub(crate) mod net {
    use super::*;
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::DuelRequested, on_requested)
            .net_handler(K::DuelOutOfBounds, on_bounds)
            .net_handler(K::DuelInBounds, on_bounds)
            .net_handler(K::DuelComplete, on_complete)
            .net_handler(K::DuelWinner, on_winner)
            .net_handler(K::DuelCountdown, on_countdown)
            .net_handler(K::Disconnected, on_session_end);
    }

    #[allow(clippy::too_many_arguments)] // a handler's full input set
    fn on_requested(
        In(ev): In<SessionEvent>,
        mut duel: ResMut<DuelState>,
        mut ui: ResMut<UiEvents>,
        commands: Res<NetCommands>,
        self_guid: Res<crate::net::SelfGuid>,
        social: Res<crate::ui_social::SocialState>,
        players: Players,
    ) {
        if let SessionEvent::DuelRequested {
            arbiter,
            challenger,
        } = ev
        {
            let held = players.player_name(challenger);
            requested(
                &mut duel,
                &mut ui,
                &commands,
                (arbiter, challenger),
                self_guid.0,
                social.is_ignored(challenger),
                held,
            );
        }
    }

    /// Held players as the challenge's lookup finds them (`0x5cfd69`), and the cached names.
    #[derive(bevy::ecs::system::SystemParam)]
    pub(crate) struct Players<'w, 's> {
        index: Res<'w, crate::net::GuidIndex>,
        stores: Query<'w, 's, &'static crate::net::ObjectStore>,
        names: Res<'w, NameCache>,
        commands: Res<'w, NetCommands>,
    }

    impl Players<'_, '_> {
        /// `None` for no held player; else its name, `None` inside while uncached.
        fn player_name(&self, guid: u64) -> Option<Option<String>> {
            let store = self
                .index
                .0
                .get(&guid)
                .and_then(|&e| self.stores.get(e).ok())?;
            (store.0.object_type() == Some(benilla_protocol::messages::ObjectType::Player))
                .then(|| self.names.resolve(guid, &self.commands).map(str::to_string))
        }
    }

    fn on_bounds(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
        match ev {
            SessionEvent::DuelOutOfBounds => bounds(&mut ui, true),
            SessionEvent::DuelInBounds => bounds(&mut ui, false),
            _ => {}
        }
    }

    fn on_complete(
        In(ev): In<SessionEvent>,
        mut duel: ResMut<DuelState>,
        mut ui: ResMut<UiEvents>,
    ) {
        if let SessionEvent::DuelComplete { started } = ev {
            complete(&mut duel, &mut ui, started);
        }
    }

    fn on_winner(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
        if let SessionEvent::DuelWinner {
            fled,
            winner: won,
            loser,
        } = ev
        {
            winner(&mut ui, fled, &won, &loser);
        }
    }

    fn on_countdown(
        In(ev): In<SessionEvent>,
        mut duel: ResMut<DuelState>,
        mut ui: ResMut<UiEvents>,
    ) {
        if let SessionEvent::DuelCountdown { seconds } = ev {
            countdown(&mut duel, &mut ui, seconds);
        }
    }

    /// The duel dies with the socket, as on the server (`Player::DuelComplete(DUEL_FLED)` on
    /// logout); a stale arbiter would make the next `AcceptDuel` echo a dead object.
    fn on_session_end(In(_): In<SessionEvent>, mut duel: ResMut<DuelState>) {
        *duel = DuelState::default();
    }

    /// `SMSG_DUEL_REQUESTED` (`0x5cfcd0`): ours accepts (`0x5cfd0a`), an ignored one declines (`0x5cfd33`), a held one fires `DUEL_REQUESTED` (`0x5cfd8b`).
    #[allow(clippy::too_many_arguments)] // the handler's inputs, passed through for its tests
    pub(crate) fn requested(
        duel: &mut DuelState,
        ui: &mut UiEvents,
        commands: &NetCommands,
        (arbiter, challenger): (u64, u64),
        own: Option<u64>,
        ignored: bool,
        held: Option<Option<String>>,
    ) {
        duel.arbiter = arbiter;
        if Some(challenger) == own {
            // `0x5cfd0a`, ahead of the accept (`0x5cfd12`).
            show_in_turn(ui, "ui_duel", UiError::key("ERR_DUEL_REQUESTED"));
            let _ = commands.0.send(ClientCommand::DuelAccepted { arbiter });
            return;
        }
        if ignored {
            let _ = commands.0.send(ClientCommand::DuelCancelled { arbiter });
            duel.arbiter = 0;
            return;
        }
        if let Some(name) = held {
            ui.write(move |s| {
                let name = name
                    .or_else(|| crate::ui_chat::combat::global_string(s, "UNKNOWNOBJECT"))
                    .unwrap_or_default();
                s.fire_event("DUEL_REQUESTED", vec![ScriptValue::Str(name)]);
            });
        }
        duel.dueler = Some(challenger);
    }

    /// `SMSG_DUEL_COMPLETE` (`0x5cfa90`): a held arbiter clears and fires `DUEL_FINISHED` (`0x5cfad8`); the countdown ends either way.
    pub(crate) fn complete(duel: &mut DuelState, ui: &mut UiEvents, started: bool) {
        if duel.arbiter != 0 {
            // `0x5cfabe`, ahead of `DUEL_FINISHED`.
            if !started {
                show_in_turn(ui, "ui_duel", UiError::key("ERR_DUEL_CANCELLED"));
            }
            duel.arbiter = 0;
            ui.fire("DUEL_FINISHED", Vec::new());
        }
        duel.countdown = None;
        duel.dueler = None;
    }

    /// `SMSG_DUEL_WINNER`'s line, composed client-side (`0x5cfb20`), winner then loser.
    pub(crate) fn winner(ui: &mut UiEvents, fled: bool, winner: &str, loser: &str) {
        let key = if fled {
            "DUEL_WINNER_RETREAT"
        } else {
            "DUEL_WINNER_KNOCKOUT"
        };
        ui.system_line(
            key,
            vec![
                LineArg::S(winner.to_string()),
                LineArg::S(loser.to_string()),
            ],
        );
    }

    /// `SMSG_DUEL_COUNTDOWN` (`0x5cfa50`); deviation: zero arms nothing, where 12340's `dec` wraps and re-arms forever.
    pub(crate) fn countdown(duel: &mut DuelState, ui: &mut UiEvents, seconds: u32) {
        duel.countdown = None;
        if seconds == 0 {
            return;
        }
        countdown_line(ui, seconds);
        duel.countdown = (seconds > 1).then(|| Countdown {
            remaining: seconds - 1,
            tick: Timer::from_seconds(1.0, TimerMode::Repeating),
        });
    }

    /// Each packet fires its event and nothing else (`0x5cf910`, `0x5cf930`).
    pub(crate) fn bounds(ui: &mut UiEvents, outside: bool) {
        if outside {
            ui.fire("DUEL_OUTOFBOUNDS", Vec::new());
        } else {
            ui.fire("DUEL_INBOUNDS", Vec::new());
        }
    }
}

/// The countdown timer (`0x5cf870`): one `DUEL_COUNTDOWN` line a second after the handler's.
fn tick_countdown(
    time: Res<Time>,
    mut duel: ResMut<DuelState>,
    mut ui: ResMut<UiEvents>,
    mut commands: Commands,
) {
    let Some(countdown) = duel.countdown.as_mut() else {
        return;
    };
    if !countdown.tick.tick(time.delta()).just_finished() {
        return;
    }
    let remaining = countdown.remaining;
    countdown.remaining -= 1;
    if countdown.remaining == 0 {
        duel.countdown = None;
    }
    countdown_line(&mut ui, remaining);
    commands.queue(crate::ui_script::live::fire_handler_events);
}

/// Drain the duel verbs' intents into their sends.
fn drain_duel(
    (script, _live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    duel: Res<DuelState>,
    spells: Option<Res<Spells>>,
    actions: Res<crate::ui_action::PlayerActions>,
    players: PlayerLookup,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    let requests = script.take_duel_requests();
    if requests.is_empty() {
        return;
    }
    for req in requests {
        match req {
            // `AcceptDuel` and `CancelDuel` send the held arbiter, whatever it is (`0x5cfbd0`,
            // `0x5cfc50`).
            DuelRequest::Accept => {
                let _ = commands.0.send(ClientCommand::DuelAccepted {
                    arbiter: duel.arbiter,
                });
            }
            DuelRequest::Cancel => {
                let _ = commands.0.send(ClientCommand::DuelCancelled {
                    arbiter: duel.arbiter,
                });
            }
            DuelRequest::ChallengeGuid(guid) => {
                challenge(Some(guid), &spells, &actions, &commands);
            }
            // The name walk with typemask `0x10`, any distance (`0x5cf953`).
            DuelRequest::ChallengeName { name, exact } => {
                let mode = if exact {
                    NameMatch::ExactOnly
                } else {
                    NameMatch::PrefixOk
                };
                let guid = players.find_as(&names, &name, mode).map(|(_, g)| g);
                if guid.is_none() {
                    info!(target: "duel", "StartDuel({name:?}): no player matches; nothing cast");
                }
                challenge(guid, &spells, &actions, &commands);
            }
        }
    }
}

/// Cast the duel spell at `guid`, a plain spell cast as `StartDuel` makes it (`0x5cf9a1`), not a
/// duel opcode. Deviation: with no duel spell learned nothing is sent, where the reference casts
/// spell 0, which its own cast check refuses.
fn challenge(
    guid: Option<u64>,
    spells: &Option<Res<Spells>>,
    actions: &crate::ui_action::PlayerActions,
    commands: &NetCommands,
) {
    let Some(guid) = guid else {
        return;
    };
    let Some(spell_id) = spells
        .as_deref()
        .and_then(|s| s.learned_with_effect(&actions.spells, SPELL_EFFECT_DUEL))
    else {
        info!(target: "duel", "StartDuel at {guid:#x}: no duel spell learned; nothing cast");
        return;
    };
    info!(target: "duel", "StartDuel at {guid:#x}: casting duel spell {spell_id}");
    let _ = commands.0.send(ClientCommand::CastSpell {
        spell_id,
        target: Some(guid),
    });
}

/// Duels: the wire session, the countdown tick, the events and the outbound intents.
pub(crate) struct UiDuelPlugin;

impl Plugin for UiDuelPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<DuelState>().add_systems(
            Update,
            (
                tick_countdown.after(benilla_world::schedule::WorldStage::Net),
                drain_duel.in_set(UiDrain),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requested(
        duel: &mut DuelState,
        challenger: u64,
        ignored: bool,
        held: Option<Option<String>>,
    ) -> (Vec<&'static str>, Vec<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut ui = UiEvents::default();
        net::requested(
            duel,
            &mut ui,
            &NetCommands(tx),
            (0xA1, challenger),
            Some(7),
            ignored,
            held,
        );
        (ui.names(), rx.try_iter().collect())
    }

    /// Our own challenge accepts at once (`0x5cfd0a`, `0x5cfd12`); both sides store the arbiter.
    #[test]
    fn our_own_challenge_takes_the_error_branch() {
        let mut duel = DuelState::default();
        let (events, sent) = requested(&mut duel, 7, false, Some(None));
        assert_eq!(events, ["<world>"], "the line, shown in the handler's turn");
        assert!(matches!(
            sent[..],
            [ClientCommand::DuelAccepted { arbiter: 0xA1 }]
        ));
        assert_eq!((duel.arbiter, duel.dueler), (0xA1, None));
    }

    /// An ignored challenger is declined (`0x5cfd33`); only a held one fires `DUEL_REQUESTED` (`0x5cfd73`), and either is stored (`0x5cfd99`).
    #[test]
    fn a_challenge_fires_only_for_a_held_player_and_never_for_an_ignored_one() {
        let mut duel = DuelState::default();
        let (events, sent) = requested(&mut duel, 9, true, Some(Some("Two".into())));
        assert!(events.is_empty());
        assert!(matches!(
            sent[..],
            [ClientCommand::DuelCancelled { arbiter: 0xA1 }]
        ));
        assert_eq!((duel.arbiter, duel.dueler), (0, None));

        let (events, sent) = requested(&mut duel, 9, false, Some(Some("Two".into())));
        assert_eq!(
            events,
            ["<write>"],
            "the event, fired with its name off the VM"
        );
        assert!(sent.is_empty());
        assert_eq!((duel.arbiter, duel.dueler), (0xA1, Some(9)));

        let mut duel = DuelState::default();
        let (events, _) = requested(&mut duel, 9, false, None);
        assert!(events.is_empty(), "no held player, no popup");
        assert_eq!(duel.dueler, Some(9));
    }

    /// Only a held arbiter finishes, an unstarted duel cancelled; the countdown ends either way (`0x5cfa90`).
    #[test]
    fn completion_is_gated_on_holding_an_arbiter() {
        let mut duel = DuelState::default();
        net::countdown(&mut duel, &mut UiEvents::default(), 3);
        let mut ui = UiEvents::default();
        net::complete(&mut duel, &mut ui, false);
        assert!(ui.is_empty(), "no duel held: silent");
        assert!(duel.countdown.is_none(), "the countdown ends anyway");

        duel.arbiter = 0xA1;
        net::complete(&mut duel, &mut ui, false);
        assert_eq!(ui.names(), ["<world>", "DUEL_FINISHED"], "the line first");
        assert_eq!(duel.arbiter, 0);

        let mut ui = UiEvents::default();
        duel.arbiter = 0xA1;
        net::complete(&mut duel, &mut ui, true);
        assert_eq!(
            ui.names(),
            ["DUEL_FINISHED"],
            "a real duel ending shows no line here"
        );
    }

    /// Each bounds packet fires its own event, repeats included (`0x5cf917`, `0x5cf937`).
    #[test]
    fn bounds_fire_once_per_packet() {
        let mut ui = UiEvents::default();
        net::bounds(&mut ui, true);
        net::bounds(&mut ui, true);
        net::bounds(&mut ui, false);
        assert_eq!(
            ui.names(),
            ["DUEL_OUTOFBOUNDS", "DUEL_OUTOFBOUNDS", "DUEL_INBOUNDS"]
        );
    }

    /// The countdown's first line comes from the handler (`0x5cfa50` -> `0x5cf870`); zero arms nothing.
    #[test]
    fn the_countdown_shows_its_first_line_from_the_handler() {
        let mut duel = DuelState::default();
        let mut ui = UiEvents::default();
        net::countdown(&mut duel, &mut ui, 0);
        assert!(duel.countdown.is_none() && ui.is_empty());
        net::countdown(&mut duel, &mut ui, 3);
        assert_eq!(ui.names(), ["DUEL_COUNTDOWN"]);
        assert_eq!(duel.countdown.as_ref().map(|c| c.remaining), Some(2));
    }

    /// 7266 "Duel", which every character starts with, carries `SPELL_EFFECT_DUEL`, so a starting
    /// book resolves the duel spell to it.
    #[test]
    fn the_duel_spell_resolves_to_7266_on_real_data() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let mut spells = Spells::empty_for_tests();
        spells.catalog = benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let book = std::collections::BTreeSet::from([6603, 7266, 8386]);
        assert_eq!(
            spells.learned_with_effect(&book, SPELL_EFFECT_DUEL),
            Some(7266)
        );
        assert_eq!(
            spells.learned_with_effect(&book, 152),
            None,
            "no summon-friend spell"
        );
    }

    /// Against the shipped `GlobalStrings.lua`: the retreat template names the loser first.
    #[test]
    fn the_outcome_line_places_both_names_positionally() {
        use benilla_ui::strings::{fill, Arg};
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let get = |key: &str| s.lua().globals().get::<String>(key).unwrap();
        let names = [Arg::S("Onerogue"), Arg::S("Twomage")];
        assert_eq!(
            fill(&get("DUEL_WINNER_KNOCKOUT"), &names),
            "Onerogue has defeated Twomage in a duel"
        );
        assert_eq!(
            fill(&get("DUEL_WINNER_RETREAT"), &names),
            "Twomage has fled from Onerogue in a duel"
        );
        assert_eq!(
            fill(&get("DUEL_COUNTDOWN"), &[Arg::D(3)]),
            "Duel starting: 3"
        );
    }

    /// `ERR_DUEL_CANCELLED` is the yellow info line and `ERR_DUEL_REQUESTED` plays `LEVELUP`; the
    /// three templates are not rows, so they go out as system lines.
    #[test]
    fn the_error_pair_are_catalog_rows_and_the_templates_are_not() {
        use benilla_ui::messages::{by_key, MsgKind};
        let requested = by_key("ERR_DUEL_REQUESTED").expect("a catalog row");
        assert_eq!(requested.kind, MsgKind::Chat);
        assert_eq!(requested.sound, Some("LEVELUP"));
        let cancelled = by_key("ERR_DUEL_CANCELLED").expect("a catalog row");
        assert_eq!(
            cancelled.kind,
            MsgKind::Info,
            "the yellow info line, not chat"
        );
        for key in [
            "DUEL_COUNTDOWN",
            "DUEL_WINNER_KNOCKOUT",
            "DUEL_WINNER_RETREAT",
        ] {
            assert!(by_key(key).is_none(), "{key} is engine-composed");
        }
    }
}
