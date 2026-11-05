//! The sound layer's packet handlers: the three server audio triggers, each a
//! [`ServerSoundMessage`] the zone mixer ([`super::zone`]) plays.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use crate::net::{GuidIndex, NetHandlerApp, ServerSoundKind, ServerSoundMessage};

/// Register the handlers, from [`super::SoundPlugin`].
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::PlaySound, on_play)
        .net_handler(K::PlayMusic, on_play)
        .net_handler(K::PlayObjectSound, on_play);
}

fn on_play(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    mut out: MessageWriter<ServerSoundMessage>,
) {
    match ev {
        SessionEvent::PlaySound { sound_id } => play_sound(sound_id, &mut out),
        SessionEvent::PlayMusic { music_id } => play_music(music_id, &mut out),
        SessionEvent::PlayObjectSound { sound_id, guid } => {
            play_object_sound(sound_id, guid, &index, &mut out)
        }
        _ => {}
    }
}

/// `SMSG_PLAY_SOUND`: a 2D one-shot.
fn play_sound(sound_id: u32, out: &mut MessageWriter<ServerSoundMessage>) {
    out.write(ServerSoundMessage {
        kind: ServerSoundKind::Sound2d,
        sound_id,
        source: None,
    });
}

/// `SMSG_PLAY_MUSIC`: the zone or event music track.
fn play_music(music_id: u32, out: &mut MessageWriter<ServerSoundMessage>) {
    out.write(ServerSoundMessage {
        kind: ServerSoundKind::Music,
        sound_id: music_id,
        source: None,
    });
}

/// `SMSG_PLAY_OBJECT_SOUND` (`0x526852`): a one-shot at a streamed object; an object the client
/// does not hold plays nothing (`0x52688b`).
fn play_object_sound(
    sound_id: u32,
    guid: u64,
    index: &GuidIndex,
    out: &mut MessageWriter<ServerSoundMessage>,
) {
    let Some(&source) = index.0.get(&guid) else {
        debug!("net: object sound {sound_id} on {guid:#x}, not held — not played");
        return;
    };
    out.write(ServerSoundMessage {
        kind: ServerSoundKind::ObjectSound,
        sound_id,
        source: Some(source),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// `0x52688b`: an object the client does not hold plays nothing, not a flat sound.
    #[test]
    fn an_unheld_object_sound_plays_nothing() {
        let mut app = App::new();
        app.add_message::<ServerSoundMessage>()
            .init_resource::<GuidIndex>();
        let held = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(0x42, held);
        app.world_mut()
            .run_system_once(
                |index: Res<GuidIndex>, mut out: MessageWriter<ServerSoundMessage>| {
                    play_object_sound(7, 0x99, &index, &mut out);
                    play_object_sound(8, 0x42, &index, &mut out);
                },
            )
            .unwrap();
        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<ServerSoundMessage>>()
            .drain()
            .map(|m| (m.sound_id, m.source))
            .collect();
        assert_eq!(sent, vec![(8, Some(held))]);
    }
}
