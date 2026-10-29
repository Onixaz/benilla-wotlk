//! The login engine under the stock `AccountLogin` glue: the credential policy, the glue
//! manager's asks, the quit and the saved account.
//!
//! The credential policy drives the IO thread's pre-logon park: `WOW_USER` plus `WOW_PASS`
//! auto-submit, pending credentials resubmit every 3 s (app-side; the IO thread never sleeps),
//! and the screen's own submit ([`GlueLogin::Submit`]). A refusal clears the credentials, never a
//! retry; the stock screen shows it ([`crate::ui_script::glue_mgr`]).
//!
//! A lost session is over, as in `GlueParent.lua`: `DISCONNECTED_FROM_SERVER` returns to login.
//! Only an unattended run ([`crate::run_mode::unattended`]) reconnects, because a client that
//! re-authenticates takes the account back from whoever displaced it.

pub(crate) mod queue;

use std::sync::atomic::Ordering;

use bevy::prelude::*;

use crate::char_select::ClientState;
use crate::net::{
    DisconnectedMessage, LoginAbandon, LoginFailedMessage, LoginQueuedMessage, LoginRequest,
    LoginSubmit, WorldAuthMessage,
};

/// The silent resubmit's pacing after a transport failure or a lost session.
const RETRY_DELAY_SECS: f32 = 3.0;

pub(crate) struct LoginPlugin;

impl Plugin for LoginPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LoginIntent>()
            .add_message::<GlueLogin>()
            .add_message::<crate::ui_script::server_alert::LaunchUrl>()
            .add_systems(
                Update,
                // Every state: the reconnect resubmit fires while `InWorld`, and the
                // authentication edge lands wherever it lands. The glue manager first, after the
                // tick whose Lua called its verbs, so its asks reach the policy in the same frame.
                (
                    crate::ui_script::glue_mgr::drive_glue_mgr
                        .after(crate::ui_script::UiInput)
                        .after(crate::realm_select::RealmSelectSet),
                    // Off the roster and the screen the manager and the tick just moved.
                    crate::ui_script::glue_scene::feed_stock_glue_preview
                        .in_set(crate::portrait::GluePreviewFeed),
                    crate::ui_script::server_alert::open_launched
                        // Reads only its message, so no state orders it but the sets.
                        .after(benilla_world::schedule::WorldStage::Stream),
                    answer_glue,
                    drive_policy,
                    to_select_on_auth,
                    drive_quit,
                )
                    .chain()
                    .after(benilla_world::schedule::WorldStage::Net),
            );
    }
}

// ── The credential policy ────────────────────────────────────────────────────────────────────────

/// Where the IO thread's read loop is, as far as the app can tell; credentials are submitted
/// only while it is parked pre-logon.
#[derive(Default, PartialEq, Eq, Clone, Copy)]
enum IoPark {
    /// Parked pre-logon (boot, a failure, a disconnect, a Back).
    #[default]
    AtLogin,
    /// Past logon: at select or in the world.
    Active,
}

/// The credential policy's memory: the session's credentials, the in-flight and park state, and
/// the resubmit timer.
#[derive(Resource, Default)]
pub(crate) struct LoginIntent {
    /// Kept in-world so an unattended reconnect re-authenticates silently; cleared by select's
    /// Back, a refusal, a Cancel and a lost session.
    creds: Option<(String, String)>,
    /// Between our send and its LoginFailed or CharacterList answer.
    in_flight: bool,
    /// The in-flight submit came from the screen, which shows its connecting dialog and failure.
    announced: bool,
    park: IoPark,
    /// `Time::elapsed_secs` deadline for the next silent resubmit.
    retry_at: Option<f32>,
    /// The env fast path is read once, on the first policy run.
    env_read: bool,
}

impl LoginIntent {
    /// Forget the session's credentials and any scheduled retry.
    pub(crate) fn clear(&mut self) {
        self.creds = None;
        self.retry_at = None;
    }

    /// The account this session authenticated as, from the env fast path or the screen; it
    /// decides whether the probe shield applies.
    pub(crate) fn account(&self) -> Option<&str> {
        self.creds.as_ref().map(|(user, _)| user.as_str())
    }
}

/// One login attempt's inputs as a single [`SystemParam`]: the policy's memory, the channel to
/// the parked IO thread, the abandon generation a Cancel bumps, and the realmlist it dials. A
/// bundle keeps [`login_input`] under Bevy's sixteen-parameter ceiling.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct Attempt<'w> {
    pub(super) intent: ResMut<'w, LoginIntent>,
    submit: Res<'w, LoginSubmit>,
    abandon: Res<'w, LoginAbandon>,
    realmlist: Res<'w, crate::realmlist::Realmlist>,
}

impl Attempt<'_> {
    /// Send one login attempt to the parked IO thread, stamped with the abandon generation. The
    /// realmlist is read at submit time, so an attempt keeps the server it started with.
    fn send(&mut self, user: &str, pass: &str, announced: bool) {
        self.intent.creds = Some((user.to_string(), pass.to_string()));
        self.intent.in_flight = true;
        self.intent.announced = announced;
        self.intent.retry_at = None;
        let _ = self.submit.0.send(LoginRequest {
            user: user.to_string(),
            pass: pass.to_string(),
            host: self.realmlist.address().to_string(),
            generation: self.abandon.0.load(Ordering::SeqCst),
        });
    }
}

/// What the glue screens ask of the login engine: the stock ones past the glue manager's own
/// guards ([`crate::ui_script::glue_mgr`]), and the native realm list's Cancel over login.
#[derive(Message)]
pub(crate) enum GlueLogin {
    /// `CGlueMgr::Login` reaching `ClientServices`' logon: one attempt, typed, with the screen's
    /// credentials. It supersedes a logon still parked at the realm list, as the reference starts
    /// the next logon whatever link the last one left.
    Submit { account: String, password: String },
    /// `CLoginMgr::Cancel`: the status dialog's Cancel during the logon, and
    /// `RealmListDialogCancelled` on the login screen. The attempt in flight is dropped, the realmd
    /// link closes, and nothing resubmits.
    Abandon,
    /// The status dialog's Cancel during the world connect, which completes it
    /// `RESPONSE_CANCELLED`; the failure arm then drops the world socket alone, so the attempt in
    /// flight ends and the realmd link stays up.
    CancelWorld,
    /// `DisconnectFromServer` (`0x4dda10`): the world session dropped if there is one, then the
    /// logon cancelled, as select's Back drops the parked session.
    Disconnect,
    /// `QuitGame` (`0x4dc050`): the process ends, with no grace for a sound.
    Quit,
}

/// The glue manager's asks: a submit as the screen's own Login press makes it (the account saving
/// is the stock Lua's `SetSavedAccountName`), a cancel as the native Cancel, a disconnect as
/// select's Back, a quit as the native quit with no grace. The screen moves and the quit go
/// through `Commands`, applied before [`drive_quit`] in this chain.
pub(crate) fn answer_glue(
    mut asks: MessageReader<GlueLogin>,
    mut attempt: Attempt,
    pick: Option<Res<crate::net::CharPick>>,
    realm: Option<Res<crate::net::RealmChoice>>,
    state: Res<State<ClientState>>,
    time: Res<Time>,
    mut commands: Commands,
) {
    for ask in asks.read() {
        match ask {
            GlueLogin::Submit { account, password } => attempt.send(account, password, true),
            GlueLogin::Abandon => {
                close_logon(&attempt.abandon, &mut attempt.intent, realm.as_deref());
            }
            GlueLogin::CancelWorld => {
                attempt.abandon.0.fetch_add(1, Ordering::SeqCst);
            }
            GlueLogin::Disconnect => {
                close_logon(&attempt.abandon, &mut attempt.intent, realm.as_deref());
                if *state.get() == ClientState::CharSelect {
                    if let Some(pick) = pick.as_deref() {
                        let _ = pick.0.send(crate::net::CharRequest::Abandon);
                    }
                    commands.set_state(ClientState::Login);
                }
            }
            GlueLogin::Quit => {
                info!("login: QuitGame");
                commands.insert_resource(QuitArm(Some(time.elapsed_secs())));
            }
        }
    }
}

/// `CLoginMgr::Cancel`: the attempt in flight is dropped at its next stage boundary (a dial asked
/// for under the old generation included), the realmd link closes, and nothing resubmits.
fn close_logon(
    abandon: &LoginAbandon,
    intent: &mut LoginIntent,
    realm: Option<&crate::net::RealmChoice>,
) {
    abandon.0.fetch_add(1, Ordering::SeqCst);
    intent.in_flight = false;
    intent.clear();
    if let Some(realm) = realm {
        let _ = realm.0.send(crate::net::RealmRequest::Close);
    }
}

/// The policy tick and the net-message reactions; the screen's own submit is [`answer_glue`]'s.
fn drive_policy(
    mut attempt: Attempt,
    time: Res<Time>,
    mut queued: MessageReader<LoginQueuedMessage>,
    mut failures: MessageReader<LoginFailedMessage>,
    mut disconnects: MessageReader<DisconnectedMessage>,
    mut exit: MessageWriter<AppExit>,
) {
    let now = time.elapsed_secs();

    // The env fast path, once: `WOW_USER` and `WOW_PASS` both set, or `WOW_PROBE_AS`, auto-submit.
    // There is no default account.
    if !attempt.intent.env_read {
        attempt.intent.env_read = true;
        // A login kicks whoever holds the account, so the automated path logs in only as an
        // account the checkout declares (`.probe-identity`) and no other run on this machine
        // holds; a typed login is not gated.
        if let Some(decision) = crate::run_mode::fast_path() {
            match decision {
                Ok(fast) => {
                    let entry = fast
                        .identity
                        .map(|id| format!(" (.probe-identity entry `{}`)", id.name))
                        .unwrap_or_default();
                    info!(
                        "login: env fast path — auto-submitting as {}{entry}",
                        fast.user
                    );
                    attempt.intent.creds = Some((fast.user, fast.pass));
                    attempt.intent.retry_at = Some(now);
                }
                Err(why) => {
                    error!("login: REFUSING the env fast path — {why}");
                    // The refusal is deterministic, so the run can never get past this screen.
                    if crate::run_mode::fatal_when_driverless(
                        "the account guard refused the only credentials this run has",
                    ) {
                        exit.write(AppExit::error());
                    }
                }
            }
        }
    }

    // `in_flight` stays set under a queue, so nothing resubmits; the stock screen shows it.
    for msg in queued.read() {
        info!(
            "login: queued for {} at position {}",
            msg.realm.as_deref().unwrap_or("the realm"),
            msg.position
                .map_or_else(|| "unknown".to_string(), |p| p.to_string()),
        );
    }
    for msg in failures.read() {
        attempt.intent.in_flight = false;
        attempt.intent.park = IoPark::AtLogin;
        // `announced` is set only by the screen's submit, and `LoginIntent::clear` leaves it, so
        // it says a person typed this attempt; only an untyped attempt may end the run.
        let may_end_the_run = !attempt.intent.announced;
        if msg.terminal {
            // No resubmit can change it: drop the credentials.
            warn!("login: {}", msg.reason);
            attempt.intent.clear();
            if may_end_the_run
                && crate::run_mode::fatal_when_driverless(&format!(
                    "terminal login failure with nobody at the keyboard ({})",
                    msg.reason
                ))
            {
                exit.write(AppExit::error());
            }
            continue;
        }
        match msg.refusal {
            Some(refusal) => {
                // A refusal from either server: never retried.
                let byte = refusal.byte();
                warn!("login: refused ({refusal:?}, {byte:#04x}) — {}", msg.reason);
                attempt.intent.clear();
                if may_end_the_run
                    && crate::run_mode::fatal_when_driverless(&format!(
                        "refused ({refusal:?}, {byte:#04x}) and no resubmit can change it"
                    ))
                {
                    exit.write(AppExit::error());
                }
            }
            None if attempt.intent.announced => warn!("login: {}", msg.reason),
            None => {
                // A silent transport failure schedules the paced resubmit.
                debug!("login: transport failure ({}) — retrying", msg.reason);
                if attempt.intent.creds.is_some() {
                    attempt.intent.retry_at = Some(now + RETRY_DELAY_SECS);
                }
            }
        }
    }
    for msg in disconnects.read() {
        attempt.intent.in_flight = false;
        // A logout or a refused character login keeps the world link: the IO thread is back at
        // character select on it, with nothing to log on again.
        if msg.end == benilla_protocol::SessionEnd::LoggedOut {
            continue;
        }
        attempt.intent.park = IoPark::AtLogin;
        if msg.session_over {
            // `DISCONNECTED_FROM_SERVER`: the credentials go, so nothing takes the account back.
            warn!(
                "login: {} — session over, back to the login screen",
                msg.reason
            );
            attempt.intent.clear();
            continue;
        }
        // Otherwise an unattended run's silent re-auth, paced after the stream death.
        if attempt.intent.creds.is_some() {
            attempt.intent.retry_at = Some(now + RETRY_DELAY_SECS);
        }
    }

    if attempt.intent.park == IoPark::AtLogin
        && !attempt.intent.in_flight
        && attempt.intent.retry_at.is_some_and(|t| now >= t)
    {
        if let Some((user, pass)) = attempt.intent.creds.clone() {
            attempt.send(&user, &pass, false);
        } else {
            attempt.intent.retry_at = None;
        }
    }
}

/// The login's success edge, the world server's authentication (`SMSG_AUTH_RESPONSE`): go to
/// CharSelect, but only from `Login`, since an unattended reconnect's lands while `InWorld`. The
/// roster is the stock screen's own ask, and comes once the screen is up.
fn to_select_on_auth(
    mut auths: MessageReader<WorldAuthMessage>,
    mut intent: ResMut<LoginIntent>,
    state: Res<State<ClientState>>,
    mut next: ResMut<NextState<ClientState>>,
    abandon: Option<Res<LoginAbandon>>,
    pick: Option<Res<crate::net::CharPick>>,
) {
    // A message of an attempt a Cancel or a disconnect has passed since is not an edge: the
    // reference drops the socket there, so the session that parked behind the screen's back is
    // abandoned.
    let now = LoginAbandon::now(abandon.as_deref());
    let (mut edge, mut stale) = (false, false);
    for msg in auths.read() {
        if msg.generation == now {
            edge = true;
        } else {
            stale = true;
        }
    }
    if stale && *state.get() == ClientState::Login {
        if let Some(pick) = pick.as_deref() {
            let _ = pick.0.send(crate::net::CharRequest::Abandon);
        }
    }
    if !edge {
        return;
    }
    intent.in_flight = false;
    intent.park = IoPark::Active;
    intent.retry_at = None;
    if *state.get() == ClientState::Login {
        next.set(ClientState::CharSelect);
    }
}

/// The armed quit, `QuitGame`'s, fired by [`drive_quit`] at the time it holds.
#[derive(Resource, Default)]
struct QuitArm(Option<f32>);

/// Fire the armed quit once its time has come.
fn drive_quit(arm: Option<Res<QuitArm>>, time: Res<Time>, mut exit: MessageWriter<AppExit>) {
    if let Some(arm) = arm {
        if arm.0.is_some_and(|t| time.elapsed_secs() >= t) {
            exit.write(AppExit::Success);
        }
    }
}

// ── The saved account name ────────────────────────────────────────────────────

/// The remembered login (`GetSavedAccountName`/`SetSavedAccountName`): `Config.wtf`'s
/// `accountName` (`0x401ba7`), hidden from Lua.
pub(crate) const CVAR_ACCOUNT_NAME: &str = "accountName";

#[cfg(test)]
mod tests {
    use benilla_protocol::LoginRefusal;

    use super::*;

    /// `drive_policy` alone with the env fast path spent, so an ambient `WOW_USER` cannot seed
    /// credentials.
    fn policy_app() -> (App, crossbeam_channel::Receiver<LoginRequest>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<LoginIntent>()
            // Not `Realmlist::default()`, which reads `$WOW_HOST` from the shell.
            .insert_resource(crate::realmlist::Realmlist::unpinned(
                crate::realmlist::DEFAULT_REALMLIST,
            ))
            .insert_resource(LoginSubmit(tx))
            .insert_resource(LoginAbandon(std::sync::Arc::new(
                std::sync::atomic::AtomicU64::new(0),
            )))
            .add_message::<LoginQueuedMessage>()
            .add_message::<LoginFailedMessage>()
            .add_message::<DisconnectedMessage>()
            .add_systems(Update, drive_policy);
        app.world_mut().resource_mut::<LoginIntent>().env_read = true;
        // Returned, not dropped: a dropped receiver turns every submit into an `Err`.
        (app, rx)
    }

    /// A lost session does not log itself back in: a displaced client's resubmit would kick the
    /// client that displaced it. `GlueParent.lua` retries nothing.
    #[test]
    fn a_lost_session_clears_the_credentials() {
        let (mut app, _requests) = policy_app();
        app.world_mut().resource_mut::<LoginIntent>().creds =
            Some(("player".into(), "secret".into()));
        app.world_mut().write_message(DisconnectedMessage {
            reason: "disconnected: world stream closed: failed to fill whole buffer".into(),
            end: benilla_protocol::SessionEnd::Lost,
            session_over: true,
        });
        app.update();

        let intent = app.world().resource::<LoginIntent>();
        assert!(
            intent.creds.is_none(),
            "the session's credentials die with the session — keeping them is what won the \
             account back off the client that displaced us",
        );
        assert!(intent.retry_at.is_none(), "and nothing is scheduled");
    }

    /// A logout rides the same message but keeps the world link, so nothing logs on again: the
    /// IO thread is at character select on that link, and the roster comes on it.
    #[test]
    fn a_logout_teardown_logs_on_no_more() {
        let (mut app, requests) = policy_app();
        {
            let mut intent = app.world_mut().resource_mut::<LoginIntent>();
            intent.creds = Some(("player".into(), "secret".into()));
            intent.park = IoPark::Active;
        }
        app.world_mut().write_message(DisconnectedMessage {
            reason: "logged out".into(),
            end: benilla_protocol::SessionEnd::LoggedOut,
            session_over: false,
        });
        app.update();
        app.update();

        let intent = app.world().resource::<LoginIntent>();
        assert!(
            intent.creds.is_some(),
            "a logout keeps the account signed in"
        );
        assert!(
            intent.park == IoPark::Active && !intent.in_flight && intent.retry_at.is_none(),
            "and submits nothing: the link it entered by is still up",
        );
        assert!(
            requests.try_recv().is_err(),
            "no login request reached the IO thread"
        );
    }

    fn auth(generation: u64) -> WorldAuthMessage {
        WorldAuthMessage {
            realm: benilla_protocol::RealmInfo {
                name: "Fake".into(),
                address: "127.0.0.1:8085".into(),
                realm_type: 0,
                flags: 0,
                population: 0.0,
                characters: 0,
                category: 1,
                id: 1,
            },
            epoch: 1,
            generation,
        }
    }

    /// The login's success edge is the authentication: the roster comes only after the screen is
    /// up and asks. Until the edge the state stays `Login`, so the screen's Exit still abandons
    /// the attempt.
    #[test]
    fn the_login_ends_at_the_authentication() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::Login)
            .init_resource::<LoginIntent>()
            .add_message::<WorldAuthMessage>()
            .add_systems(Update, to_select_on_auth);
        app.world_mut().write_message(auth(0));
        app.update();
        app.update();
        assert_eq!(
            *app.world().resource::<State<ClientState>>().get(),
            ClientState::CharSelect
        );
    }

    /// An attempt a Cancel or a disconnect has passed since is no edge: the screen stays on login
    /// and the session that parked behind its back is abandoned.
    #[test]
    fn a_login_edge_behind_the_abandon_generation_abandons_the_session() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::Login)
            .insert_resource(LoginAbandon(std::sync::Arc::new(
                std::sync::atomic::AtomicU64::new(1),
            )))
            .insert_resource(crate::net::CharPick(tx.into()))
            .init_resource::<LoginIntent>()
            .add_message::<WorldAuthMessage>()
            .add_systems(Update, to_select_on_auth);
        app.world_mut().write_message(auth(0));
        app.update();
        app.update();
        assert_eq!(
            *app.world().resource::<State<ClientState>>().get(),
            ClientState::Login,
            "a stale edge"
        );
        assert!(
            matches!(rx.try_recv(), Ok(crate::net::CharRequest::Abandon)),
            "the parked session is abandoned"
        );
        app.world_mut().write_message(auth(1));
        app.update();
        app.update();
        assert_eq!(
            *app.world().resource::<State<ClientState>>().get(),
            ClientState::CharSelect,
            "the current attempt's edge"
        );
        assert!(rx.try_recv().is_err());
    }

    /// An unattended run still reconnects on a lost session; the verdict rides the message.
    #[test]
    fn an_unattended_run_still_reconnects_on_its_own() {
        let (mut app, _requests) = policy_app();
        app.world_mut().resource_mut::<LoginIntent>().creds =
            Some(("probe1".into(), "secret".into()));
        app.world_mut().write_message(DisconnectedMessage {
            reason: "disconnected: connection reset".into(),
            end: benilla_protocol::SessionEnd::Lost,
            session_over: false,
        });
        app.update();

        let intent = app.world().resource::<LoginIntent>();
        assert!(intent.creds.is_some());
        assert_eq!(
            intent.retry_at,
            Some(RETRY_DELAY_SECS),
            "paced by the flat 3 s off a zeroed clock, not fired on the spot",
        );
    }

    /// Each attempt dials the realmlist current at its submit.
    #[test]
    fn a_submitted_attempt_carries_the_configured_realmlist() {
        let (mut app, requests) = policy_app();
        app.world_mut()
            .insert_resource(crate::realmlist::Realmlist::unpinned(
                "logon.example.org:3725",
            ));
        {
            let mut intent = app.world_mut().resource_mut::<LoginIntent>();
            intent.creds = Some(("player".into(), "secret".into()));
            intent.retry_at = Some(0.0);
        }
        app.update();

        let sent = requests.try_recv().expect("the policy submitted");
        assert_eq!(sent.user, "player");
        assert_eq!(
            sent.host, "logon.example.org:3725",
            "the attempt dials what the realmlist says, not a value latched at spawn",
        );

        app.world_mut()
            .insert_resource(crate::realmlist::Realmlist::unpinned(
                "elsewhere.example.org",
            ));
        {
            let mut intent = app.world_mut().resource_mut::<LoginIntent>();
            intent.in_flight = false;
            intent.retry_at = Some(0.0);
        }
        app.update();
        assert_eq!(
            requests.try_recv().expect("resubmitted").host,
            "elsewhere.example.org",
        );
    }

    fn exited(app: &mut App, cursor: &mut bevy::ecs::message::MessageCursor<AppExit>) -> bool {
        let msgs = app.world().resource::<Messages<AppExit>>();
        cursor.read(msgs).next().is_some()
    }

    /// Drive one refusal through the policy; whether it exited.
    fn refusal_exits(typed: bool) -> bool {
        let (mut app, _requests) = policy_app();
        let mut cursor = bevy::ecs::message::MessageCursor::<AppExit>::default();
        app.update();
        let _ = exited(&mut app, &mut cursor);
        app.world_mut().resource_mut::<LoginIntent>().announced = typed;
        app.world_mut().write_message(LoginFailedMessage {
            refusal: Some(LoginRefusal::Logon(0x05)),
            reason: "server rejected logon: result 0x05".into(),
            terminal: false,
        });
        app.update();
        exited(&mut app, &mut cursor)
    }

    /// Neither a typed refusal nor env credentials alone end the run: only an untyped attempt in
    /// a run that declares `WOW_UNATTENDED` does.
    #[test]
    fn a_typed_password_refusal_never_exits() {
        let _lock = crate::local_state::test_env::ENV_LOCK.lock();

        // A person's own launch line (as `.cargo/config.toml`'s example), declaring nobody absent.
        let _user = crate::local_state::test_env::EnvGuard::set("WOW_USER", "player");
        let _pass = crate::local_state::test_env::EnvGuard::set("WOW_PASS", "secret");
        let _char = crate::local_state::test_env::EnvGuard::set("WOW_CHAR", "Hero");
        let _decl = crate::local_state::test_env::EnvGuard::unset("WOW_UNATTENDED");
        let _cap = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let _rig = crate::local_state::test_env::EnvGuard::unset("WOW_RIG");
        assert!(
            crate::run_mode::env_login() && !crate::run_mode::unattended(),
            "the fixture must be a player's env: credentials, nobody declared away",
        );
        assert!(
            !refusal_exits(false),
            "an env-credentialled run with a person in it does not exit",
        );

        // A run that declares itself driverless.
        let _decl = crate::local_state::test_env::EnvGuard::set("WOW_UNATTENDED", "1");
        assert!(
            refusal_exits(false),
            "an attempt nobody typed, in a run nobody is in, still exits non-zero",
        );
        assert!(
            !refusal_exits(true),
            "a password typed at the screen is attended by direct evidence",
        );
    }
}
