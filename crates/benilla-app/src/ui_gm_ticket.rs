//! The GM ticket flow behind the stock Help window, as the 12340 client runs it: one handler
//! (`0x5ad240`) for the eight answers, and the senders' gates on the client's two ids,
//! the ticket id (`[0xc1e8d0]`) and the response id (`[0xc1e8d4]`), zeroed at each world entry
//! (`0x5ac160`). Every `SMSG_GMTICKET_GETTICKET` fires `UPDATE_TICKET`, an unchanged one too, as
//! the window re-polls `GetGMTicket()` every 10 minutes; no ticket fires it with no arguments,
//! which the stock `HelpFrame.lua` reads as "no ticket".

use benilla_protocol::messages::GmTicket;
use benilla_ui::script::{GmTicketIntent, ScriptValue, UiScript};
use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;

use crate::net::{ClientCommand, NetCommands};
use crate::player::Player;
use crate::ui_script::{UiFeed, UiInput, VmMemo};

/// Spell 7355 "Stuck", what `Stuck()` casts: the only spell with `SPELL_EFFECT_STUCK` (84), which
/// moves the player to their last safe position.
const SPELL_STUCK: u32 = 7355;

/// The longest ticket text, in UTF-8 characters (the ticket text check, `0x5ac170`).
const TICKET_TEXT_MAX: usize = 500;

/// One thing the handler tells Lua, in arrival order.
#[derive(Debug, Clone, PartialEq)]
enum Signal {
    /// `UPDATE_TICKET`, with the ticket's seven arguments or none.
    Ticket(Option<Box<GmTicket>>),
    /// `UPDATE_GM_STATUS(status)`.
    QueueStatus(i32),
    /// `GMSURVEY_DISPLAY`, argless.
    Survey,
    /// `GMRESPONSE_RECEIVED(ticketDescription, response)`.
    Response { text: String, response: String },
    /// A `GlobalStrings` error line, as `GameUI_DisplayMessage` shows it.
    Message(&'static str),
}

/// The client's ticket state and what the handler owes Lua.
#[derive(Resource, Default)]
pub(crate) struct GmTicketState {
    /// The ticket id: the open ticket's id, nonzero while one is held; gates edit and delete.
    ticket_id: u32,
    /// The response id: a GM's answer awaiting resolve; gates the follow-up and the resolve.
    response_id: u32,
    /// Signals not yet fired, oldest first.
    pending: Vec<Signal>,
    /// Re-asks the engine owes (`0x5ad2a1`), sent by [`drain_gm_ticket`].
    engine_reasks: u32,
}

impl GmTicketState {
    /// `SMSG_GMTICKET_GETTICKET` (`0x5ad2f9`): status 6 holds the ticket and drops any answer;
    /// any other fires `UPDATE_TICKET` bare, status 0 after `ERR_TICKET_DB_ERROR`. The ids stand.
    pub(crate) fn answer(&mut self, status: u32, ticket: Option<Box<GmTicket>>) {
        match ticket {
            Some(t) if status == benilla_protocol::messages::GMTICKET_STATUS_HASTEXT => {
                self.response_id = 0;
                self.ticket_id = t.id;
                self.pending.push(Signal::Ticket(Some(t)));
            }
            _ => {
                if status == benilla_protocol::messages::GMTICKET_STATUS_DBERROR {
                    self.pending.push(Signal::Message("ERR_TICKET_DB_ERROR"));
                }
                self.pending.push(Signal::Ticket(None));
            }
        }
    }

    /// `SMSG_GMTICKET_CREATE` and `_UPDATETEXT` share one switch (`0x5ad27f`): 2 and 4 re-ask,
    /// 1, 3 and 5 show their errors, anything else is silent.
    pub(crate) fn write_response(&mut self, code: u32) {
        match code {
            1 => self
                .pending
                .push(Signal::Message("ERR_TICKET_ALREADY_EXISTS")),
            2 | 4 => self.engine_reasks = self.engine_reasks.saturating_add(1),
            3 => self
                .pending
                .push(Signal::Message("ERR_TICKET_CREATE_ERROR")),
            5 => self
                .pending
                .push(Signal::Message("ERR_TICKET_UPDATE_ERROR")),
            _ => debug!("gm ticket: write answered {code}"),
        }
    }

    /// `SMSG_GMTICKET_DELETETICKET` (`0x5ad403`): 9 drops both ids and fires `UPDATE_TICKET`
    /// bare; anything else is `ERR_TICKET_DB_ERROR`.
    pub(crate) fn deleted(&mut self, code: u32) {
        if code == benilla_protocol::messages::GMTICKET_RESPONSE_TICKET_DELETED {
            self.forget_ids();
            self.pending.push(Signal::Ticket(None));
        } else {
            self.pending.push(Signal::Message("ERR_TICKET_DB_ERROR"));
        }
    }

    /// `SMSG_GM_TICKET_STATUS_UPDATE` (`0x5ad42d`): 1 and 4 re-ask, 2 closes the ticket, 3 closes
    /// it and offers the survey.
    pub(crate) fn status_update(&mut self, status: u32) {
        match status {
            1 | 4 => self.engine_reasks = self.engine_reasks.saturating_add(1),
            2 => {
                self.forget_ids();
                self.pending.push(Signal::Ticket(None));
            }
            3 => {
                self.forget_ids();
                self.pending.push(Signal::Survey);
            }
            other => debug!("gm ticket: status update {other}"),
        }
    }

    /// `SMSG_GMRESPONSE_RECEIVED` (`0x5ad4ef`): both ids come from the packet.
    pub(crate) fn response(&mut self, response: benilla_protocol::messages::GmResponse) {
        self.response_id = response.response_id;
        self.ticket_id = response.ticket_id;
        self.pending.push(Signal::Response {
            text: response.text,
            response: response.response,
        });
    }

    /// `SMSG_GMRESPONSE_STATUS_UPDATE` (`0x5ad4b2`): both ids drop, then the survey or a bare
    /// `UPDATE_TICKET`.
    pub(crate) fn response_closed(&mut self, show_survey: bool) {
        self.forget_ids();
        self.pending.push(if show_survey {
            Signal::Survey
        } else {
            Signal::Ticket(None)
        });
    }

    /// `SMSG_GMRESPONSE_DB_ERROR`.
    pub(crate) fn response_db_error(&mut self) {
        self.pending
            .push(Signal::Message("ERR_GMRESPONSE_DB_ERROR"));
    }

    /// `SMSG_GMTICKETSYSTEMSTATUS`.
    pub(crate) fn answer_queue(&mut self, status: i32) {
        self.pending.push(Signal::QueueStatus(status));
    }

    fn forget_ids(&mut self) {
        self.ticket_id = 0;
        self.response_id = 0;
    }

    fn take_reasks(&mut self) -> u32 {
        std::mem::take(&mut self.engine_reasks)
    }

    /// Session end: the next login is a fresh world entry (`0x5ac160`).
    pub(crate) fn clear_session(&mut self) {
        *self = Self::default();
    }

    /// The gate of one write, as its sender runs it (`0x5ac8f0`, `0x5acb50`, `0x5acc60`,
    /// `0x5acce0`): `Err` is the error line to show, `Ok(None)` a silent refusal.
    fn gate(&self, intent: &GmTicketIntent, has_player: bool) -> Result<Option<()>, &'static str> {
        let held = self.ticket_id != 0;
        let answered = self.response_id != 0;
        match intent {
            GmTicketIntent::Create {
                text, more_help, ..
            } => {
                let allowed = if *more_help { answered } else { !held };
                if !allowed || !has_player {
                    return Ok(None);
                }
                ticket_text_valid(text).map(Some)
            }
            GmTicketIntent::Update { text } => {
                if !held || answered {
                    return Ok(None);
                }
                ticket_text_valid(text).map(Some)
            }
            GmTicketIntent::Delete => Ok((held && !answered).then_some(())),
            GmTicketIntent::Resolve => Ok(answered.then_some(())),
            GmTicketIntent::Ask | GmTicketIntent::AskStatus => Ok(Some(())),
        }
    }
}

/// The ticket text check (`0x5ac170`): at most 500 UTF-8 characters, and one token that is not
/// whitespace.
fn ticket_text_valid(text: &str) -> Result<(), &'static str> {
    if text.chars().count() > TICKET_TEXT_MAX {
        return Err("ERR_TICKET_TEXT_TOO_LONG");
    }
    if text.split([' ', '\t', '\r', '\n']).all(str::is_empty) {
        return Err("ERR_TICKET_NO_TEXT");
    }
    Ok(())
}

/// Per VM ([`VmMemo`]): a `/reload`'s new VM is fed the categories afresh.
#[derive(Resource, Default)]
struct GmTicketFeedState {
    vm: VmMemo<bool>,
}

/// Fire the handler's signals in arrival order; push the categories once per VM.
fn feed_gm_ticket(
    script: Option<NonSendMut<UiScript>>,
    mut state: ResMut<GmTicketState>,
    categories: Option<Res<GmTicketCategories>>,
    mut feed: ResMut<GmTicketFeedState>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    let pushed = feed.vm.get(&script);
    if !*pushed {
        if let Some(categories) = categories.as_deref() {
            script.set_gm_ticket_categories(categories.0.clone());
            *pushed = true;
        }
    }
    if state.pending.is_empty() {
        return;
    }
    for signal in std::mem::take(&mut state.pending) {
        match signal {
            Signal::Ticket(ticket) => {
                script.fire_event("UPDATE_TICKET", update_ticket_args(ticket.as_deref()))
            }
            Signal::QueueStatus(status) => script.fire_event(
                "UPDATE_GM_STATUS",
                vec![ScriptValue::Int(i64::from(status))],
            ),
            Signal::Survey => script.fire_event("GMSURVEY_DISPLAY", vec![]),
            Signal::Response { text, response } => script.fire_event(
                "GMRESPONSE_RECEIVED",
                vec![ScriptValue::Str(text), ScriptValue::Str(response)],
            ),
            Signal::Message(key) => {
                let line = crate::ui_action::keyed_line(&script, key);
                crate::ui_action::show_messages(&mut script, &mut sink, "gm ticket", line);
            }
        }
    }
}

/// `UPDATE_TICKET`'s arguments, `"%d%s%f%f%f%d%d"` (`0x5ad3e6`): category, text, the three day
/// figures, assigned, opened. No ticket is no arguments at all (`0x5ad322`).
fn update_ticket_args(ticket: Option<&GmTicket>) -> Vec<ScriptValue> {
    let Some(t) = ticket else {
        return Vec::new();
    };
    vec![
        ScriptValue::Int(i64::from(t.category)),
        ScriptValue::Str(t.text.clone()),
        ScriptValue::Number(f64::from(t.ticket_age)),
        ScriptValue::Number(f64::from(t.oldest_ticket_age)),
        ScriptValue::Number(f64::from(t.update_time)),
        ScriptValue::Int(i64::from(t.assigned_to_gm)),
        ScriptValue::Int(i64::from(t.opened_by_gm)),
    ]
}

/// Turn the window's verbs into packets, each through its sender's gate.
fn drain_gm_ticket(
    script: Option<NonSendMut<UiScript>>,
    mut state: ResMut<GmTicketState>,
    commands: Res<NetCommands>,
    player: Option<Res<Player>>,
    map: Option<Res<benilla_world::world_map::CurrentMap>>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };

    // The engine's re-asks first: they answer a push that arrived before this frame's input.
    for _ in 0..state.take_reasks() {
        let _ = commands.0.send(ClientCommand::GmTicketGet);
    }

    for _ in 0..script.take_stuck_casts() {
        let _ = commands.0.send(ClientCommand::CastSpell {
            spell_id: SPELL_STUCK,
            target: None,
        });
    }

    let intents = script.take_gm_ticket_intents();
    if intents.is_empty() {
        return;
    }
    // Where the player stands at send time, in WoW coordinates.
    let pos = player.as_ref().map(|p| bevy_to_wow(p.pos));
    let map = map.map(|m| m.0).unwrap_or(0);
    // In call order: `DeleteGMTicket(); GetGMTicket()` must not become get-then-delete.
    for intent in intents {
        match state.gate(&intent, pos.is_some()) {
            Ok(Some(())) => {}
            Ok(None) => {
                debug!("gm ticket: {intent:?} refused by the client's gate");
                continue;
            }
            Err(key) => {
                let line = crate::ui_action::keyed_line(&script, key);
                crate::ui_action::show_messages(&mut script, &mut sink, "gm ticket", line);
                continue;
            }
        }
        let _ = commands.0.send(match intent {
            GmTicketIntent::Ask => ClientCommand::GmTicketGet,
            GmTicketIntent::AskStatus => ClientCommand::GmTicketSystemStatus,
            GmTicketIntent::Delete => ClientCommand::GmTicketDelete,
            GmTicketIntent::Resolve => ClientCommand::GmResponseResolve,
            GmTicketIntent::Update { text } => ClientCommand::GmTicketUpdate { text },
            GmTicketIntent::Create {
                text,
                need_response,
                more_help,
            } => ClientCommand::GmTicketCreate {
                map,
                pos: pos.unwrap_or([0.0; 3]),
                text,
                need_response,
                more_help,
            },
        });
    }
}

/// `GMTicketCategory.dbc` as the `(id, name)` pairs `GetGMTicketCategories()` returns.
#[derive(Resource)]
pub(crate) struct GmTicketCategories(pub(crate) Vec<(u32, String)>);

/// Startup, after the MPQ chain opens: a bare `Startup` races the open and can leave the category
/// list empty for the whole session.
fn load_gm_ticket_dbc(
    mut commands: Commands,
    world_assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    use benilla_assets::LockRecover;
    let Some(world_assets) = world_assets else {
        return;
    };
    let mut chain = world_assets.chain.lock_recover();
    match benilla_formats::load_gm_ticket_categories(&mut chain) {
        Ok(cat) => {
            info!(
                "gm ticket: GMTicketCategory.dbc loaded ({} categories)",
                cat.len()
            );
            commands.insert_resource(GmTicketCategories(
                cat.categories()
                    .iter()
                    .map(|c| (c.id, c.name.clone()))
                    .collect(),
            ));
        }
        Err(e) => warn!("gm ticket: GMTicketCategory.dbc failed to load: {e:#}"),
    }
}

/// The GM ticket's packet handlers.
pub(crate) mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use crate::net::NetHandlerApp;

    use super::GmTicketState;

    /// Register the ticket's handlers, called from [`super::UiGmTicketPlugin`].
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::GmTicket, on_answer)
            .net_handler(K::GmTicketSystemStatus, on_answer)
            .net_handler(K::GmTicketStatusUpdate, on_answer)
            .net_handler(K::GmTicketCreated, on_answer)
            .net_handler(K::GmTicketUpdated, on_answer)
            .net_handler(K::GmTicketDeleted, on_answer)
            .net_handler(K::GmResponseDbError, on_answer)
            .net_handler(K::GmResponseReceived, on_answer)
            .net_handler(K::GmResponseStatusUpdate, on_answer)
            .net_handler(K::Disconnected, on_session_end);
    }

    /// The handler's eight opcodes and the queue status, into [`GmTicketState`].
    fn on_answer(In(ev): In<SessionEvent>, mut ticket: ResMut<GmTicketState>) {
        match ev {
            SessionEvent::GmTicket { status, ticket: t } => ticket.answer(status, t),
            SessionEvent::GmTicketSystemStatus { status } => ticket.answer_queue(status),
            SessionEvent::GmTicketStatusUpdate { status } => ticket.status_update(status),
            SessionEvent::GmTicketCreated { response }
            | SessionEvent::GmTicketUpdated { response } => ticket.write_response(response),
            SessionEvent::GmTicketDeleted { response } => ticket.deleted(response),
            SessionEvent::GmResponseDbError => ticket.response_db_error(),
            SessionEvent::GmResponseReceived { response } => ticket.response(*response),
            SessionEvent::GmResponseStatusUpdate { show_survey } => {
                ticket.response_closed(show_survey)
            }
            _ => {}
        }
    }

    /// The ids are the character's, and the next login may be another; a logout ends the session
    /// as a disconnect.
    fn on_session_end(In(_): In<SessionEvent>, mut ticket: ResMut<GmTicketState>) {
        ticket.clear_session();
    }
}

/// The GM trouble-ticket flow: the answer feed and the window's sends.
pub(crate) struct UiGmTicketPlugin;

impl Plugin for UiGmTicketPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<GmTicketState>()
            .init_resource::<GmTicketFeedState>()
            .add_systems(
                Startup,
                load_gm_ticket_dbc.after(benilla_assets::AssetSet::Open),
            )
            .add_systems(
                Update,
                (
                    feed_gm_ticket.in_set(UiFeed),
                    drain_gm_ticket.after(UiInput),
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{
        GmResponse, GMTICKET_STATUS_DEFAULT, GMTICKET_STATUS_HASTEXT,
    };

    fn ticket(id: u32) -> Box<GmTicket> {
        Box::new(GmTicket {
            id,
            text: "Stuck in a rock.".into(),
            category: 1,
            ticket_age: 0.25,
            oldest_ticket_age: 2.5,
            update_time: 0.01,
            assigned_to_gm: 2,
            opened_by_gm: 1,
        })
    }

    fn create(more_help: bool) -> GmTicketIntent {
        GmTicketIntent::Create {
            text: "help".into(),
            need_response: true,
            more_help,
        }
    }

    /// The event leads with the category, the packet with the id and text.
    #[test]
    fn the_event_args_lead_with_the_category_not_the_text() {
        let t = ticket(7);
        let args = update_ticket_args(Some(&t));
        assert_eq!(args[0], ScriptValue::Int(1), "arg1 is the category");
        assert_eq!(args[1], ScriptValue::Str("Stuck in a rock.".into()));
        assert_eq!(args.len(), 7);
        assert_eq!(args[5], ScriptValue::Int(2), "arg6 assignedToGM");
        assert_eq!(args[6], ScriptValue::Int(1), "arg7 openedByGM");
    }

    /// `HelpFrame.lua` tests `if ( category )`: a `0` would read as a ticket, so no ticket is no
    /// arguments (#188).
    #[test]
    fn the_no_ticket_answer_carries_no_arguments() {
        assert!(update_ticket_args(None).is_empty());
        let mut state = GmTicketState::default();
        state.answer(GMTICKET_STATUS_DEFAULT, None);
        assert_eq!(state.pending, vec![Signal::Ticket(None)]);
    }

    /// Status 0 shows the error first; the ids survive any no-ticket answer.
    #[test]
    fn a_db_error_answer_shows_its_line_then_fires_bare_and_keeps_the_ids() {
        let mut state = GmTicketState::default();
        state.answer(GMTICKET_STATUS_HASTEXT, Some(ticket(9)));
        state.pending.clear();
        state.answer(0, None);
        assert_eq!(
            state.pending,
            vec![Signal::Message("ERR_TICKET_DB_ERROR"), Signal::Ticket(None)]
        );
        assert_eq!(state.ticket_id, 9, "0x5ad322 leaves the ticket id alone");
    }

    /// The window's 10-minute poll must be re-fed each time.
    #[test]
    fn a_repeated_identical_answer_fires_again() {
        let mut state = GmTicketState::default();
        state.answer(GMTICKET_STATUS_HASTEXT, Some(ticket(3)));
        state.answer(GMTICKET_STATUS_HASTEXT, Some(ticket(3)));
        assert_eq!(state.pending.len(), 2);
    }

    #[test]
    fn create_and_update_answers_reask_or_show_their_errors() {
        let mut state = GmTicketState::default();
        for code in [2, 4] {
            state.write_response(code);
        }
        assert_eq!(state.take_reasks(), 2);
        for code in [1, 3, 5, 7] {
            state.write_response(code);
        }
        assert_eq!(
            state.pending,
            vec![
                Signal::Message("ERR_TICKET_ALREADY_EXISTS"),
                Signal::Message("ERR_TICKET_CREATE_ERROR"),
                Signal::Message("ERR_TICKET_UPDATE_ERROR"),
            ]
        );
        assert_eq!(state.take_reasks(), 0);
    }

    #[test]
    fn a_delete_clears_the_ids_and_fires_bare_and_a_refusal_is_a_db_error() {
        let mut state = GmTicketState::default();
        state.answer(GMTICKET_STATUS_HASTEXT, Some(ticket(4)));
        state.pending.clear();
        state.deleted(9);
        assert_eq!(state.pending, vec![Signal::Ticket(None)]);
        assert_eq!((state.ticket_id, state.response_id), (0, 0));
        state.pending.clear();
        state.deleted(1);
        assert_eq!(state.pending, vec![Signal::Message("ERR_TICKET_DB_ERROR")]);
    }

    #[test]
    fn the_status_push_reasks_closes_or_offers_the_survey() {
        let mut state = GmTicketState::default();
        state.status_update(1);
        state.status_update(4);
        assert_eq!(state.take_reasks(), 2);
        state.ticket_id = 5;
        state.status_update(2);
        assert_eq!(state.ticket_id, 0);
        state.ticket_id = 5;
        state.status_update(3);
        assert_eq!(state.pending, vec![Signal::Ticket(None), Signal::Survey]);
    }

    /// A GM's answer sets both ids; its close drops them and fires the survey or a bare update.
    #[test]
    fn a_gm_response_arms_the_follow_up_and_its_close_disarms_it() {
        let mut state = GmTicketState::default();
        state.response(GmResponse {
            response_id: 1,
            ticket_id: 8,
            text: "Stuck.".into(),
            response: "Hearth.".into(),
        });
        assert_eq!((state.ticket_id, state.response_id), (8, 1));
        assert_eq!(state.gate(&create(true), true), Ok(Some(())));
        assert_eq!(state.gate(&GmTicketIntent::Resolve, true), Ok(Some(())));
        assert_eq!(
            state.gate(&create(false), true),
            Ok(None),
            "a ticket is held"
        );
        assert_eq!(
            state.gate(&GmTicketIntent::Delete, true),
            Ok(None),
            "an answered ticket cannot be abandoned"
        );
        state.response_closed(false);
        assert_eq!(state.gate(&GmTicketIntent::Resolve, true), Ok(None));
        assert_eq!(state.pending.last(), Some(&Signal::Ticket(None)));
    }

    /// The senders' gates (`0x5ac8f0`, `0x5acb50`, `0x5acc60`): no edit or delete without a ticket,
    /// no second create while one is open, no create without a player.
    #[test]
    fn the_writes_are_gated_on_the_clients_ticket_id() {
        let mut state = GmTicketState::default();
        let update = GmTicketIntent::Update { text: "x".into() };
        assert_eq!(state.gate(&update, true), Ok(None));
        assert_eq!(state.gate(&GmTicketIntent::Delete, true), Ok(None));
        assert_eq!(state.gate(&create(false), true), Ok(Some(())));
        assert_eq!(state.gate(&create(false), false), Ok(None));
        assert_eq!(
            state.gate(&create(true), true),
            Ok(None),
            "no answer to follow"
        );

        state.answer(GMTICKET_STATUS_HASTEXT, Some(ticket(2)));
        assert_eq!(state.gate(&update, true), Ok(Some(())));
        assert_eq!(state.gate(&GmTicketIntent::Delete, true), Ok(Some(())));
        assert_eq!(state.gate(&create(false), true), Ok(None));
        assert_eq!(state.gate(&GmTicketIntent::Ask, true), Ok(Some(())));
    }

    /// The ticket text check (`0x5ac170`): 500 UTF-8 characters, and something besides whitespace.
    #[test]
    fn ticket_text_is_bounded_in_characters_and_must_not_be_blank() {
        assert_eq!(ticket_text_valid(&"é".repeat(500)), Ok(()));
        assert_eq!(
            ticket_text_valid(&"a".repeat(501)),
            Err("ERR_TICKET_TEXT_TOO_LONG")
        );
        assert_eq!(ticket_text_valid(" \t\r\n "), Err("ERR_TICKET_NO_TEXT"));
        assert_eq!(ticket_text_valid(""), Err("ERR_TICKET_NO_TEXT"));
        let state = GmTicketState::default();
        let blank = GmTicketIntent::Create {
            text: "  ".into(),
            need_response: false,
            more_help: false,
        };
        assert_eq!(state.gate(&blank, true), Err("ERR_TICKET_NO_TEXT"));
    }

    #[test]
    fn a_dead_session_forgets_the_ids_and_the_queue() {
        let mut state = GmTicketState::default();
        state.answer(GMTICKET_STATUS_HASTEXT, Some(ticket(6)));
        state.answer_queue(1);
        state.write_response(2);
        state.clear_session();
        assert_eq!(state.ticket_id, 0);
        assert!(state.pending.is_empty());
        assert_eq!(state.take_reasks(), 0);
    }
}
