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
/// (`0x526530`). An update owes `UPDATE_WORLD_STATES` whatever it wrote (`0x526a17`); an init
/// only when its filter or any pair moved (`0x5269bd`).
fn world_states(
    scope: Option<(u32, u32, u32)>,
    states: Vec<(u32, u32)>,
    world_states: &mut WorldStates,
) {
    let mut moved = scope.is_none();
    if let Some((map, zone, area)) = scope {
        debug!(
            "world states: map {map} zone {zone} area {area}, {} entries",
            states.len()
        );
        moved |= world_states.init_scope(map, zone, area);
    }
    moved |= world_states.write(&states);
    if moved {
        world_states.owe_update();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_state::WorldStateEvent::{ArenaSeason, Update};

    /// `0x526530`: an update owes `UPDATE_WORLD_STATES` every time, an init only when its filter or
    /// a value moved, and a moved arena season fires its own event from the setter first.
    #[test]
    fn updates_always_fire_and_inits_fire_on_a_move() {
        let mut t = WorldStates::default();
        world_states(Some((489, 3277, 3277)), vec![(1581, 0)], &mut t);
        assert_eq!(t.take_owed(), [Update], "a first init moves the filter");
        world_states(Some((489, 3277, 3277)), vec![(1581, 0)], &mut t);
        assert_eq!(t.take_owed(), [], "the same init again moves nothing");
        world_states(Some((489, 3277, 3277)), vec![(1581, 2)], &mut t);
        assert_eq!(t.take_owed(), [Update], "a moved value");
        world_states(None, vec![(1581, 2)], &mut t);
        world_states(None, vec![(1581, 2)], &mut t);
        assert_eq!(
            t.take_owed(),
            [Update, Update],
            "one per update, moved or not"
        );
        world_states(None, vec![(0xc77, 8)], &mut t);
        world_states(None, vec![(0xc77, 8)], &mut t);
        assert_eq!(
            t.take_owed(),
            [ArenaSeason, Update, Update],
            "the season fires only when it moves"
        );
    }
}
