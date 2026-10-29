//! Weather sound: the `SMSG_WEATHER` id's `Weather.dbc` ambience kit (12340 stores it at
//! `0x4c8610`; 0 is none) is an input to the one zone-ambience channel, not a loop of its own. The
//! selector returns the weather kit while the zonetext indoor bit is clear (written 1 by the indoor
//! area feeder) and the area's `AmbienceID` while set, each swap a 5.0 s crossfade: rain is
//! replaced indoors, not ducked. The packet's `grade` drives rendering only. The channel itself is
//! `super::zone`'s.

use bevy::prelude::*;

use benilla_world::schedule::WorldStage;
use benilla_world::weather::WeatherMessage;

/// The weather loop's SoundEntries kit from the last `SMSG_WEATHER`, 0 for none.
#[derive(Resource, Default)]
pub(super) struct WeatherAmbience(pub(super) u32);

/// Track the wire's weather sound kit.
fn track_weather_kit(mut kit: ResMut<WeatherAmbience>, mut msgs: MessageReader<WeatherMessage>) {
    for m in msgs.read() {
        if kit.0 != m.sound_id {
            info!("weather sound kit {} (grade {:.2})", m.sound_id, m.grade);
            kit.0 = m.sound_id;
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<WeatherAmbience>()
        .add_systems(Update, track_weather_kit.in_set(WorldStage::Present));
}
