//! Server-started movies. `SMSG_TRIGGER_MOVIE` (`0x526c73`) runs `StartMovie` (`0x5235b0`): a
//! `Movie.dbc` row with a file for the screen's width becomes the playing movie, and with our
//! player in the world `PlayMovie` (`0x51d710`) fires `PLAY_MOVIE` with the file's name and the
//! row's volume. `GameMovieFinished()` stops it (`0x518870`), which sends `CMSG_COMPLETE_MOVIE`.
//!
//! Deviation: benilla plays no pre-rendered movie (#130). The stock `MovieFrame`'s `StartMovie`
//! answers nil, so the frame hides and calls `GameMovieFinished()` at once, and the server hears
//! the end. Nothing enters the movie's input mode (`0x523604`), as no movie takes the screen.

use benilla_formats::MovieCatalog;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{ScriptValue, UiScript};
use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands, NetHandlerApp, SelfPlayer};

/// The movie tables; empty without client data.
#[derive(Resource, Default)]
pub(crate) struct Movies(pub(crate) MovieCatalog);

/// The playing movie (`[0xbd0800]`-`[0xbd0808]`): the file `PLAY_MOVIE` names and the row's
/// volume, and whether that event is still owed.
#[derive(Resource, Default)]
pub(crate) struct MoviePlaying {
    movie: Option<(String, u32)>,
    owed: bool,
}

impl MoviePlaying {
    /// `StartMovie`: nothing for an id with no row or no file (`0x5235be`-`0x5235e5`).
    fn start(&mut self, movie_id: u32, catalog: &MovieCatalog, width: u32) {
        let (Some(volume), Some(file)) =
            (catalog.volume(movie_id), catalog.file_for(movie_id, width))
        else {
            debug!("movie: {movie_id} has no Movie.dbc row or file — not started");
            return;
        };
        debug!("movie: {movie_id} started ({}, volume {volume})", file.name);
        self.movie = Some((file.name, volume));
        self.owed = true;
    }

    /// `StopMovie`: a playing movie ends, `true` when the server is owed its end.
    fn stop(&mut self) -> bool {
        self.owed = false;
        self.movie.take().is_some()
    }
}

pub(crate) struct MoviePlugin;

impl Plugin for MoviePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Movies>()
            .init_resource::<MoviePlaying>()
            .net_handler(SessionEventKind::TriggerMovie, on_trigger_movie)
            .net_handler(SessionEventKind::Disconnected, on_session_end)
            .add_systems(Startup, load_movies.after(benilla_assets::AssetSet::Open))
            .add_systems(
                Update,
                feed_movie
                    .in_set(crate::ui_script::UiFeed)
                    .run_if(crate::ui_script::ingame_ui_up),
            );
    }
}

fn load_movies(mut commands: Commands, assets: Option<Res<benilla_assets::WorldAssets>>) {
    let Some(assets) = assets else {
        return;
    };
    let loaded = {
        use benilla_assets::LockRecover;
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_movies(&mut chain)
    };
    match loaded {
        Ok(catalog) => commands.insert_resource(Movies(catalog)),
        Err(e) => warn!("movie: the movie tables failed — no movie starts: {e:#}"),
    }
}

/// The screen width `FindMovieFileDataID` reads from `gxResolution`'s `"%dx%d"` (`0x518765`).
fn screen_width(resolution: Option<&str>) -> u32 {
    resolution
        .and_then(|r| r.split('x').next())
        .and_then(|w| w.trim().parse().ok())
        .unwrap_or(0)
}

fn on_trigger_movie(
    In(ev): In<SessionEvent>,
    mut playing: ResMut<MoviePlaying>,
    movies: Res<Movies>,
    cvars: Option<Res<crate::cvars::Cvars>>,
) {
    if let SessionEvent::TriggerMovie { movie_id } = ev {
        let resolution = cvars.as_deref().and_then(|c| c.get("gxResolution"));
        playing.start(movie_id, &movies.0, screen_width(resolution));
    }
}

/// The movie ends with the connection, as the reference's world teardown leaves none playing.
fn on_session_end(In(_): In<SessionEvent>, mut playing: ResMut<MoviePlaying>) {
    playing.stop();
}

/// `PLAY_MOVIE` once our player is in the world (`0x523632`), then the stops its handlers ask for.
fn feed_movie(
    script: Option<NonSendMut<UiScript>>,
    mut playing: ResMut<MoviePlaying>,
    me: Query<(), With<SelfPlayer>>,
    net: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    if playing.owed && !me.is_empty() {
        playing.owed = false;
        if let Some((name, volume)) = playing.movie.clone() {
            script.fire_event(
                "PLAY_MOVIE",
                vec![ScriptValue::Str(name), ScriptValue::Int(i64::from(volume))],
            );
        }
    }
    for _ in 0..script.take_game_movie_finished_calls() {
        if playing.stop() {
            debug!("movie: finished — CMSG_COMPLETE_MOVIE");
            let _ = net.0.send(ClientCommand::CompleteMovie);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> MovieCatalog {
        MovieCatalog::from_rows(
            &[(2, 120)],
            &[(2, 10)],
            &[(10, 1024)],
            &[(10, "WotLK.avi", "Interface\\Cinematics\\")],
        )
    }

    /// `0x5235be`: an unknown id starts nothing; a known one is owed `PLAY_MOVIE` and its stop
    /// owes the server the end once.
    #[test]
    fn a_known_movie_starts_and_stops_once() {
        let mut playing = MoviePlaying::default();
        playing.start(9, &catalog(), 1024);
        assert!(playing.movie.is_none() && !playing.owed);
        playing.start(2, &catalog(), 1024);
        assert_eq!(
            playing.movie,
            Some(("Interface\\Cinematics\\WotLK".into(), 120))
        );
        assert!(playing.owed);
        assert!(playing.stop());
        assert!(!playing.stop(), "a second stop sends nothing (`0x518870`)");
    }

    /// The stock in-game `MovieFrame` takes `PLAY_MOVIE`, cannot start the file, and ends with one
    /// `GameMovieFinished()`.
    #[test]
    fn the_stock_movie_frame_finishes_a_movie_it_cannot_play() {
        let _data = benilla_formats::wow_data_or_skip!();
        let mut s = crate::ui_script::test_ui::stock_ui(1024.0, 768.0);
        s.fire_event(
            "PLAY_MOVIE",
            vec![
                ScriptValue::Str("Interface\\Cinematics\\WOW_Wrathgate".into()),
                ScriptValue::Int(150),
            ],
        );
        assert_eq!(s.take_game_movie_finished_calls(), 1, "{:?}", s.errors());
        assert!(s.eval::<bool>("return MovieFrame:IsShown() ~= 1").unwrap());
    }

    #[test]
    fn the_width_is_gx_resolution_s_first_number() {
        assert_eq!(screen_width(Some("1920x1080")), 1920);
        assert_eq!(screen_width(None), 0);
    }
}
