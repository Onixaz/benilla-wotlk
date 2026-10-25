//! The app's lifecycle state ([`ClientState`]) and the roster under the stock character screens:
//! the parked IO thread emits the roster ([`CharListMessage`]), which a reconnect answers with
//! the pending pick; the first pick is the stock screen's ([`crate::ui_script::glue_mgr`]).

use benilla_protocol::Character;
use bevy::prelude::*;

use crate::net::{
    CharListMessage, CharPick, CharRequest, CharacterLoginFailedMessage, EnteredWorldMessage,
    LoggedOutMessage,
};

/// The app's lifecycle: which screen owns the session.
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) enum ClientState {
    /// Parked pre-logon at the login screen, the IO thread waiting for credentials. The realm list
    /// is no state: `GlueParent.lua`'s `GlueScreenInfo` omits it, a `DIALOG` frame over any
    /// screen.
    #[default]
    Login,
    /// Parked at the character screens, select and create, the IO thread waiting for a pick.
    CharSelect,
    /// A character is in (or entering) the world.
    InWorld,
}

/// The one "only while in the world" gate; a member's own `run_if` ANDs with it. Configured for
/// `Update` only: a set's conditions are per schedule, so a member elsewhere needs the set
/// configured there too.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct InWorldGated;

/// The lifecycle state and the roster.
pub(crate) struct CharSelectPlugin {
    /// The screen this session opens on: `Login` when connected, `InWorld` for a world capture.
    pub(crate) start: ClientState,
}

/// Mirror the session's screen onto the engine's one-bit "is there a world" fact, in one place so
/// no transition can forget it.
fn publish_world_live(
    state: Res<State<ClientState>>,
    mut live: ResMut<benilla_world::schedule::WorldLive>,
) {
    let now = benilla_world::schedule::WorldLive(*state.get() == ClientState::InWorld);
    if *live != now {
        *live = now;
    }
}

impl Plugin for CharSelectPlugin {
    fn build(&self, app: &mut App) {
        app.insert_state(self.start)
            .configure_sets(Update, InWorldGated.run_if(in_state(ClientState::InWorld)))
            .init_resource::<Roster>()
            // Ahead of every world stage, so `WorldLive` and its falling edge are this frame's.
            .add_systems(
                Update,
                publish_world_live.before(benilla_world::schedule::WorldStage::Net),
            )
            .add_systems(Update, debug_logout_smoke)
            .add_systems(
                Update,
                session_edges().after(benilla_world::schedule::WorldStage::Net),
            );
    }
}

/// The session's edges, in every state: the reconnect auto-answer and the logout edge arrive
/// `InWorld`. A logout or a refused login clears the pick before the roster policy reads it: the
/// roster comes right behind them on the kept link, often in the same drain, and must be shown,
/// not answered with the pick just left. `back_on_disconnect` runs after the entry: a session that
/// died during entry queues both edges into one frame, and the dead one must win.
fn session_edges() -> bevy::ecs::schedule::ScheduleConfigs<bevy::ecs::system::ScheduleSystem> {
    (
        back_on_logout,
        back_on_login_refused,
        apply_roster_policy,
        enter_on_connected,
        back_on_disconnect,
        // Last: it mirrors the pick the frame ended with.
        persist_last_character,
    )
        .chain()
        .into_configs()
}

// ── The roster ─────────────────────────────────────────────────────────────────────────────────

/// The account roster and the pick in flight.
#[derive(Resource, Default)]
pub(crate) struct Roster {
    pub(super) chars: Vec<Character>,
    /// The guid picked; kept in-world so a reconnect's roster is answered with it, cleared by a
    /// logout so the roster is shown.
    pub(crate) pending_pick: Option<u64>,
    /// The realm-list entry this session connected to, refreshed with each roster.
    pub(super) realm: Option<benilla_protocol::RealmInfo>,
}

impl Roster {
    /// The connected realm's type, `Cfg_Configs.dbc`'s `RealmType` key.
    pub(crate) fn realm_type(&self) -> Option<u32> {
        self.realm.as_ref().map(|r| r.realm_type)
    }

    /// A roster with a pick in flight, for tests outside this module.
    #[cfg(test)]
    pub(crate) fn with_pending_pick(chars: Vec<Character>, guid: u64) -> Self {
        Self {
            chars,
            pending_pick: Some(guid),
            ..Self::default()
        }
    }

    /// The pending pick's map, for the loading screen's art before `SMSG_LOGIN_VERIFY_WORLD`.
    pub(crate) fn pending_map(&self) -> Option<u32> {
        self.pending_row().map(|c| c.map)
    }

    /// The pending pick's level: the reference suppresses the loading tip when it arrived as 0 in
    /// `SMSG_CHAR_ENUM` (`0x5b42a0` sets `[selChar+0x10a]`); vmangos always sends a real level.
    pub(crate) fn pending_level(&self) -> Option<u8> {
        self.pending_row().map(|c| c.level)
    }

    /// The picked character's `(map, wow xyz)`, a round trip before `SMSG_LOGIN_VERIFY_WORLD`; the
    /// streamers aim at it during world entry.
    pub(crate) fn pending_entry(&self) -> Option<(u32, [f32; 3])> {
        self.pending_row()
            .map(|c| (c.map, [c.position.x, c.position.y, c.position.z]))
    }

    /// The roster row for the pick in flight: the only description of the character until the
    /// self descriptor streams in, so the in-game UI seats `UnitName("player")` from it.
    pub(crate) fn pending_row(&self) -> Option<&Character> {
        self.pending_pick
            .and_then(|g| self.chars.iter().find(|c| c.guid == g))
    }

    /// The roster position of the pick in flight, by guid rather than the live selection: what
    /// the reference persists as `lastCharacterIndex` at Enter World.
    pub(super) fn pending_index(&self) -> Option<usize> {
        let guid = self.pending_pick?;
        self.chars.iter().position(|c| c.guid == guid)
    }
}

// ── The remembered character (`lastCharacterIndex`) ───────────────────────────────

/// The 1.12 CVar the select screen remembers by: registered at `0x402d93` (name `0x82e8f8`,
/// default `"0"`, pointer at `[0x882674]`), written engine-side, named by no GlueXML.
pub(crate) const CVAR_LAST_CHARACTER: &str = "lastCharacterIndex";

/// The stored index is 0-based, `"0"` the first character: the engine's selection cell
/// `[0x83856c]` printed with `"%d"` (`0x473470` decrements the Lua's 1-based row).
/// `CVar::SaveConfig` (`0x63d980`) skips a value equal to the default, so row 0 writes no line.
fn last_character_value(row: usize) -> String {
    row.to_string()
}

/// Mirror the character entering the world into [`CVAR_LAST_CHARACTER`], at Enter World, not on
/// a click: `CGlueMgr::EnterWorld` (`0x46b500`) writes the CVar at `0x46b5fa`. The reference
/// flushes `Config.wtf` in the same call (`0x46b6f6`); this reaches disk on exit with every CVar.
fn persist_last_character(
    roster: Res<Roster>,
    mut cvars: ResMut<crate::cvars::Cvars>,
    // The registry outlives every VM, so a row written once stays written.
    mut mirrored: Local<Option<usize>>,
) {
    let Some(row) = roster.pending_index() else {
        return;
    };
    if *mirrored == Some(row) {
        return;
    }
    cvars.set(CVAR_LAST_CHARACTER, &last_character_value(row));
    *mirrored = Some(row);
}

/// Ask the parked IO thread to log in as `guid` and remember it as pending; the probe rig picks
/// through here too, so a reconnect re-answers with its character.
pub(crate) fn send_pick(roster: &mut Roster, pick: &CharPick, guid: u64) {
    roster.pending_pick = Some(guid);
    let _ = pick.0.send(CharRequest::Enter(guid));
}

/// Drain each [`CharListMessage`] into the roster, then answer it with the pending pick: a
/// reconnect, or a pick that raced a dying socket, re-answers without the screen.
fn apply_roster_policy(
    mut msgs: MessageReader<CharListMessage>,
    mut roster: ResMut<Roster>,
    pick: Res<CharPick>,
) {
    for msg in msgs.read() {
        roster.chars = msg.characters.clone();
        roster.realm = msg.realm.clone();
        if let Some(guid) = roster.pending_pick {
            send_pick(&mut roster, &pick, guid);
        }
    }
}

/// `Connected` ([`EnteredWorldMessage`]): the world owns the session.
fn enter_on_connected(
    mut msgs: MessageReader<EnteredWorldMessage>,
    mut next: ResMut<NextState<ClientState>>,
) {
    if msgs.read().next().is_some() {
        next.set(ClientState::InWorld);
    }
}

/// The session died: back to the login screen, as `GlueParent.lua`'s `DISCONNECTED_FROM_SERVER`
/// does. Chained after [`enter_on_connected`]: a stream dying during entry drains `Connected` and
/// `Disconnected` in one frame, and the dead session must be the last word.
fn back_on_disconnect(
    mut msgs: MessageReader<crate::net::DisconnectedMessage>,
    mut roster: ResMut<Roster>,
    mut next: ResMut<NextState<ClientState>>,
) {
    if !msgs.read().any(|m| m.session_over) {
        return;
    }
    // With no reconnect coming, a kept pick would auto-answer the next roster.
    roster.pending_pick = None;
    next.set(ClientState::Login);
}

/// `SMSG_CHARACTER_LOGIN_FAILED`: back to select, where the stock screen shows the refusal. The IO
/// thread announces the entry with the pick, so this undoes a half-built one. The pick must be
/// cleared, or the roster behind the refusal is auto-answered and the refused login loops.
fn back_on_login_refused(
    mut msgs: MessageReader<CharacterLoginFailedMessage>,
    mut roster: ResMut<Roster>,
    mut next: ResMut<NextState<ClientState>>,
) {
    if msgs.read().last().is_none() {
        return;
    }
    roster.pending_pick = None;
    next.set(ClientState::CharSelect);
}

/// A confirmed `/logout`: back to select, the pick cleared so the next roster is shown.
fn back_on_logout(
    mut msgs: MessageReader<LoggedOutMessage>,
    mut roster: ResMut<Roster>,
    mut next: ResMut<NextState<ClientState>>,
) {
    if msgs.read().next().is_some() {
        roster.pending_pick = None;
        next.set(ClientState::CharSelect);
    }
}

/// The logout smoke (`WOW_LOGOUT_SMOKE=1`, with `WOW_CHAR`): once in the world, `/logout`, return
/// to CharSelect, wait for the roster the logout brings on the link the world was entered by,
/// enter the world again and close the window. The second entry streams a map
/// `terrain_stream::release_world` tore down. It ends by a window close, not an `AppExit`, since
/// only the close reaches the shutdown tail that saves variables and addon files.
fn debug_logout_smoke(
    state: Res<State<ClientState>>,
    player: Res<crate::player::Player>,
    commands: Res<crate::net::NetCommands>,
    mut roster: ResMut<Roster>,
    pick: Res<CharPick>,
    streamer: Res<benilla_world::terrain_stream::TerrainStreamer>,
    time: Res<Time>,
    mut exit: MessageWriter<AppExit>,
    mut close: MessageWriter<bevy::window::WindowCloseRequested>,
    windows: Query<Entity, With<bevy::window::PrimaryWindow>>,
    mut lists: MessageReader<CharListMessage>,
    // The world session's number at the entry, and whether a roster has come since the logout.
    mut link: Local<Option<u64>>,
    mut relisted: Local<bool>,
    mut phase: Local<u8>,
    mut mark: Local<f32>,
) {
    if std::env::var("WOW_LOGOUT_SMOKE").is_err() {
        return;
    }
    for list in lists.read() {
        if *phase < 2 {
            *link = Some(list.epoch);
        } else if !*relisted {
            *relisted = true;
            if Some(list.epoch) == *link {
                info!(
                    "logout-smoke: roster after the logout on the same link — {} char(s)",
                    list.characters.len()
                );
            } else {
                error!(
                    "logout-smoke: FAILED — the roster after the logout came on world session {} \
                     where the world was entered on {:?}",
                    list.epoch, *link
                );
            }
        }
    }
    let now = time.elapsed_secs();
    match *phase {
        0 if *state.get() == ClientState::InWorld && player.active => {
            info!("logout-smoke: seated in world — lingering");
            (*phase, *mark) = (1, now);
        }
        1 if now - *mark > 3.0 => {
            info!("logout-smoke: requesting logout");
            let _ = commands.0.send(crate::net::ClientCommand::Logout);
            *phase = 2;
        }
        2 if *state.get() == ClientState::CharSelect => {
            info!("logout-smoke: back at character select — lingering");
            (*phase, *mark) = (3, now);
        }
        // Residency is read here, not on arrival: `release_world` runs about 150 ms after the
        // state edge, on the world-live falling edge.
        // The roster the logout brings, not the one that survived it, decides the re-entry.
        3 if now - *mark > 4.0 && *relisted => {
            match roster.chars.first().map(|c| (c.guid, c.name.clone())) {
                Some((guid, name)) => {
                    info!(
                        "logout-smoke: {} tiles resident after the release (must be 0)",
                        streamer.residency().1
                    );
                    info!("logout-smoke: re-entering the world as {name}");
                    send_pick(&mut roster, &pick, guid);
                    (*phase, *mark) = (4, now);
                }
                None => {
                    warn!("logout-smoke: empty roster — cannot test re-entry");
                    *phase = 5;
                }
            }
        }
        4 if *state.get() == ClientState::InWorld && player.active && now - *mark > 3.0 => {
            // The suppressor reading catches a character that re-entered unable to move;
            // `scripts/smoke.sh` fails on anything but `none`. A GM account's logout is instant,
            // not rooted, so it does not exercise the rooted path.
            info!(
                "logout-smoke: re-entered — {} tiles resident, suppressors: {}, done",
                streamer.residency().1,
                player.movement_suppressors()
            );
            *phase = 5;
        }
        5 => {
            info!("logout-smoke: done");
            // Close the window as a player does; a headless run has none and exits directly.
            match windows.single() {
                Ok(window) => {
                    close.write(bevy::window::WindowCloseRequested { window });
                }
                Err(_) => {
                    warn!("logout-smoke: no primary window — exiting by AppExit instead");
                    exit.write(AppExit::Success);
                }
            }
            *phase = 6;
        }
        _ => {}
    }
}

/// The WoW UI font, straight off the patch chain (Bevy's TTF loader over the `mpq://` source).
pub(crate) fn wow_font(assets: &AssetServer) -> Handle<Font> {
    assets.load("mpq://Fonts/FRIZQT__.ttf")
}

/// A `Character` with only its identity filled in.
#[cfg(test)]
pub(crate) fn test_character(guid: u64, name: &str) -> Character {
    Character {
        guid,
        name: name.to_string(),
        race: 1,
        class: 1,
        gender: 0,
        skin: 0,
        face: 0,
        hair_style: 0,
        hair_color: 0,
        facial_hair: 0,
        level: 1,
        zone: 0,
        map: 0,
        position: benilla_protocol::wire::Vector3d {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        flags: 0,
        customize_flags: 0,
        equipment: [benilla_protocol::CharEnumItem::default(); 19],
        bags: [benilla_protocol::CharEnumItem::default(); 4],
        pet_display_id: 0,
        pet_level: 0,
        pet_family: 0,
    }
}

/// The roster policy and its CVar mirror without the screen, for tests over a real CVar host.
#[cfg(test)]
pub(crate) fn add_test_systems(app: &mut App, pick: crossbeam_channel::Sender<CharRequest>) {
    app.insert_state(ClientState::CharSelect)
        .init_resource::<Roster>()
        .insert_resource(CharPick(pick.into()))
        .add_message::<CharListMessage>()
        .add_systems(
            Update,
            (apply_roster_policy, persist_last_character).chain(),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::test_character as character;

    /// A socket dying mid-entry (a displacement kick) drains `Connected` and `Disconnected` in one
    /// frame; the ordering alone makes the disconnect win.
    #[test]
    fn a_session_lost_during_entry_beats_the_entry() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::CharSelect)
            .init_resource::<Roster>()
            .init_resource::<crate::ui_script::PlayerUiHover>()
            .init_resource::<crate::ui_script::UiKeyboardCapture>()
            .add_message::<EnteredWorldMessage>()
            .add_message::<crate::net::DisconnectedMessage>()
            .add_systems(Update, (enter_on_connected, back_on_disconnect).chain());
        app.world_mut().resource_mut::<Roster>().pending_pick = Some(7);

        app.world_mut().write_message(EnteredWorldMessage {
            billing_time_rested: 0,
            tutorial_flags: None,
        });
        app.world_mut()
            .write_message(crate::net::DisconnectedMessage {
                reason: "disconnected: world stream closed: failed to fill whole buffer".into(),
                end: benilla_protocol::SessionEnd::Lost,
                session_over: true,
            });
        app.update();
        app.update(); // `StateTransition` applies the pending state at the next frame

        assert_eq!(
            *app.world().resource::<State<ClientState>>().get(),
            ClientState::Login,
            "the dead session must be the last word — the reference's DISCONNECTED_FROM_SERVER \
             puts the client back on the account screen, not into a world with no session",
        );
        assert_eq!(
            app.world().resource::<Roster>().pending_pick,
            None,
            "and the reconnect's pending pick goes with it, or the next roster walks straight \
             back into the world the player was just thrown out of",
        );
    }

    /// A teardown whose session is not over (a logout, or a reconnect) still enters the world.
    #[test]
    fn a_teardown_that_is_not_the_end_leaves_the_entry_alone() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::CharSelect)
            .init_resource::<Roster>()
            .init_resource::<crate::ui_script::PlayerUiHover>()
            .init_resource::<crate::ui_script::UiKeyboardCapture>()
            .add_message::<EnteredWorldMessage>()
            .add_message::<crate::net::DisconnectedMessage>()
            .add_systems(Update, (enter_on_connected, back_on_disconnect).chain());

        app.world_mut().write_message(EnteredWorldMessage {
            billing_time_rested: 0,
            tutorial_flags: None,
        });
        app.world_mut()
            .write_message(crate::net::DisconnectedMessage {
                reason: "logged out".into(),
                end: benilla_protocol::SessionEnd::LoggedOut,
                session_over: false,
            });
        app.update();
        app.update();

        assert_eq!(
            *app.world().resource::<State<ClientState>>().get(),
            ClientState::InWorld,
        );
    }

    /// `Connected` is emitted with `CMSG_PLAYER_LOGIN`, and vmangos refuses before touching the
    /// database, so the refusal lands in the same drain and must be the last word.
    #[test]
    fn a_refused_login_beats_the_entry_it_revokes() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::CharSelect)
            .init_resource::<Roster>()
            .init_resource::<crate::ui_script::PlayerUiHover>()
            .init_resource::<crate::ui_script::UiKeyboardCapture>()
            .add_message::<EnteredWorldMessage>()
            .add_message::<CharacterLoginFailedMessage>()
            .add_systems(Update, (enter_on_connected, back_on_login_refused).chain());
        app.world_mut().resource_mut::<Roster>().pending_pick = Some(7);

        app.world_mut().write_message(EnteredWorldMessage {
            billing_time_rested: 0,
            tutorial_flags: None,
        });
        app.world_mut()
            .write_message(CharacterLoginFailedMessage { result: 0x01 });
        app.update();
        app.update(); // `StateTransition` applies the pending state at the next frame

        assert_eq!(
            *app.world().resource::<State<ClientState>>().get(),
            ClientState::CharSelect,
            "the refusal must win — the reference leaves the player on the select screen it \
             never actually took them off",
        );
        assert_eq!(
            app.world().resource::<Roster>().pending_pick,
            None,
            "and the pick goes with it: the relist behind the refusal is auto-answered with \
             `pending_pick`, so keeping it would re-enter the character just refused, forever",
        );
    }

    /// A logout, or a refused login, and the roster the kept link sends right behind it can land
    /// in one drain: the pick the world was entered with goes first, so the roster is shown and
    /// not answered with that pick again.
    #[test]
    fn a_return_and_its_roster_in_one_drain_show_the_roster() {
        for refused in [false, true] {
            let (tx, rx) = crossbeam_channel::unbounded();
            let mut app = App::new();
            app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
                .insert_state(ClientState::InWorld)
                .insert_resource(Roster::with_pending_pick(vec![character(7, "Seven")], 7))
                .insert_resource(CharPick(tx.into()))
                .init_resource::<crate::cvars::Cvars>()
                .add_message::<CharListMessage>()
                .add_message::<EnteredWorldMessage>()
                .add_message::<LoggedOutMessage>()
                .add_message::<CharacterLoginFailedMessage>()
                .add_message::<crate::net::DisconnectedMessage>()
                .add_systems(Update, session_edges());
            if refused {
                app.world_mut()
                    .write_message(CharacterLoginFailedMessage { result: 0x01 });
            } else {
                app.world_mut().write_message(LoggedOutMessage);
            }
            app.world_mut().write_message(CharListMessage {
                characters: vec![character(7, "Seven")],
                realm: None,
                cause: benilla_protocol::RosterCause::Park,
                epoch: 1,
            });
            app.update();
            app.update();

            assert_eq!(
                *app.world().resource::<State<ClientState>>().get(),
                ClientState::CharSelect,
                "refused: {refused}"
            );
            assert!(
                rx.try_recv().is_err(),
                "refused: {refused} — the roster was answered with the pick just left"
            );
            assert_eq!(app.world().resource::<Roster>().pending_pick, None);
        }
    }

    /// The write is at Enter World, not at selection (`0x46b500`).
    #[test]
    fn only_entering_the_world_writes_the_cvar() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Roster>()
            .init_resource::<crate::cvars::Cvars>()
            .insert_resource(CharPick(tx.into()))
            .add_systems(Update, persist_last_character);
        {
            let mut roster = app.world_mut().resource_mut::<Roster>();
            roster.chars = vec![
                character(1, "Kerwind"),
                character(2, "Xero"),
                character(3, "Zz"),
            ];
        }
        app.update();
        assert!(
            !app.world().resource::<crate::cvars::Cvars>().has_events(),
            "a roster alone must NOT be remembered — the reference writes nothing here",
        );

        app.world_mut().resource_mut::<Roster>().pending_pick = Some(2);
        app.update();
        let moved: Vec<(String, String)> = app
            .world_mut()
            .resource_mut::<crate::cvars::Cvars>()
            .take_events()
            .into_iter()
            .map(|e| (e.name, e.new))
            .collect();
        assert_eq!(
            moved,
            vec![(CVAR_LAST_CHARACTER.to_string(), "1".to_string())],
            "guid 2 sits at row 1, and the row is what the registry took",
        );
        // Steady frames stay quiet; the next VM is seeded from the registry.
        app.update();
        let cvars = app.world().resource::<crate::cvars::Cvars>();
        assert!(!cvars.has_events());
        assert!(cvars
            .vm_seed()
            .iter()
            .any(|r| r.name == CVAR_LAST_CHARACTER && r.value == "1"));
    }
}
