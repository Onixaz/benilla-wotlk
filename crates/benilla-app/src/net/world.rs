//! The bridge's world feed: weather goes to the world crate as a [`WeatherMessage`], world states
//! to the [`WorldStates`] table the UI's `$<n>w` tokens read.

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_formats::WeatherCatalog;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_world::weather::WeatherMessage;
use bevy::prelude::*;

use super::NetHandlerApp;
use crate::world_state::WorldStates;

/// `Weather.dbc`, which names what an `SMSG_WEATHER` id means.
#[derive(Resource)]
struct WeatherTable(WeatherCatalog);

/// Registers the two handlers.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.add_systems(Startup, load_weather_table.after(AssetSet::Open))
        .net_handler(K::Weather, on_weather)
        .net_handler(K::WorldStates, on_world_states);
}

fn load_weather_table(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = benilla_formats::load_weather_catalog(&mut assets.chain.lock_recover());
    match loaded {
        Ok(cat) => commands.insert_resource(WeatherTable(cat)),
        Err(e) => warn!("Weather.dbc failed to load, weather stays fine: {e:#}"),
    }
}

fn on_weather(
    In(ev): In<SessionEvent>,
    table: Option<Res<WeatherTable>>,
    mut out: MessageWriter<WeatherMessage>,
) {
    if let SessionEvent::Weather {
        weather_id,
        grade,
        instant,
    } = ev
    {
        weather(
            table.as_deref().map(|t| &t.0),
            weather_id,
            grade,
            instant,
            &mut out,
        );
    }
}

fn on_world_states(In(ev): In<SessionEvent>, mut states: ResMut<WorldStates>) {
    if let SessionEvent::WorldStates {
        scope,
        states: pairs,
    } = ev
    {
        world_states(scope, pairs, &mut states);
    }
}

/// `SMSG_WEATHER`: the zone's weather change, `instant` on zone entry, a ramp otherwise. The id
/// is a `Weather.dbc` row whose effect type and ambience kit the reference applies (`0x526a77`);
/// an id the table lacks is type 0 with no kit.
fn weather(
    table: Option<&WeatherCatalog>,
    weather_id: u32,
    grade: f32,
    instant: bool,
    out: &mut MessageWriter<WeatherMessage>,
) {
    let row = table.and_then(|t| t.get(weather_id));
    debug!("weather id {weather_id} grade {grade:.2} instant {instant}: {row:?}");
    out.write(WeatherMessage {
        weather_type: row.map_or(0, |r| r.effect_type),
        grade,
        sound_id: row.map_or(0, |r| r.ambience_kit),
        instant,
    });
}

/// `SMSG_INIT_WORLD_STATES` and `SMSG_UPDATE_WORLD_STATE` share one setter, as in the reference.
/// An init first records `(map, zone, area)` as the UI's display filter, then writes its pairs
/// (`0x526530`).
fn world_states(
    scope: Option<(u32, u32, u32)>,
    states: Vec<(u32, u32)>,
    world_states: &mut WorldStates,
) {
    if let Some((map, zone, area)) = scope {
        debug!(
            "world states: map {map} zone {zone} area {area}, {} entries",
            states.len()
        );
        world_states.init_scope(map, zone, area);
    }
    world_states.write(&states);
}
