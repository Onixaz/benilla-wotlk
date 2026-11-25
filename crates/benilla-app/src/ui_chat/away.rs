//! `/afk` and `/dnd`, which the reference's `SendChatMessage` handles differently. DND reads
//! `PLAYER_FLAGS` bit `0x4` live and falls through to the plain `CMSG_MESSAGECHAT` send. AFK reads
//! the client-side [`AfkMirror`] and hands off to the player's AFK set or clear, which send their
//! own packets. Both echo before the send, never from the descriptor: the `PLAYER_FLAGS` delta arm
//! prints nothing. The four lines are `CHAT_MSG_SYSTEM` (type `0xA`); the `CHAT_MSG_AFK`/`DND`
//! events are other players' auto-replies.

use bevy::prelude::*;

use super::event::{ChatEvent, ChatEventKind};

/// The reference's optimistic mirror of our `PLAYER_FLAGS` AFK bit (`[0xbcefec]`): written at
/// command time, by the player-flags watch ([`flags_watch`]) and at world entry
/// ([`seed_on_enter_world`]). A `u32` tested for non-zero: the watch stores `PLAYER_FLAGS & 2`, so a
/// set mirror can read 2. DND has no mirror, so a second
/// `/dnd` before the server's update re-marks where a second `/afk` clears.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AfkMirror(pub(crate) u32);

impl AfkMirror {
    /// Non-zero, as all six of the reference's readers test it.
    pub(crate) fn is_afk(self) -> bool {
        self.0 != 0
    }
}

/// What a `/dnd` resolves to: the line to print and the wire body; DND has no mirror.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AwayOutcome {
    /// The `CHAT_MSG_SYSTEM` line.
    pub(crate) line: String,
    /// The `CMSG_MESSAGECHAT` body's bytes, after the client's own default-message substitution.
    pub(crate) body: Vec<u8>,
}

/// `/dnd <msg>` against the live `PLAYER_FLAGS` bit `0x4`, the DND arm; unlike `/afk`, a repeat
/// with a message re-prints. `MARKED_DND` carries its period inside the format string, so it is
/// filled, never composed.
pub(crate) fn dnd_line(
    msg: &[u8],
    is_dnd: bool,
    strings: &impl Fn(&str) -> Option<String>,
) -> AwayOutcome {
    let text = |key: &str| strings(key).unwrap_or_default();
    if is_dnd && msg.is_empty() {
        return AwayOutcome {
            line: text("CLEARED_DND"),
            body: Vec::new(),
        };
    }
    let body = if msg.is_empty() {
        text("DEFAULT_DND_MESSAGE").into_bytes()
    } else {
        msg.to_vec()
    };
    AwayOutcome {
        line: shown(&fill_one_bytes(text("MARKED_DND").as_bytes(), &body)),
        body,
    }
}

/// A composed line as the chat event's text: the line's bytes, a sequence that is no UTF-8 read
/// as U+FFFD, since the event carries a `String` (#1517).
pub(crate) fn shown(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Fill the first `%s` of a GlobalString, as the reference's `snprintf` fills it with any bytes;
/// `%%` is a percent.
pub(crate) fn fill_one_bytes(template: &[u8], arg: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(template.len() + arg.len());
    let mut used = false;
    let mut i = 0;
    while i < template.len() {
        let c = template[i];
        i += 1;
        if c != b'%' {
            out.push(c);
            continue;
        }
        match template.get(i).copied() {
            Some(b'%') => out.push(b'%'),
            Some(b's') if !used => {
                out.extend_from_slice(arg);
                used = true;
            }
            Some(other) => {
                out.push(b'%');
                out.push(other);
            }
            None => {
                out.push(b'%');
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Add one of the four lines as `CHAT_MSG_SYSTEM`, in the caller's turn.
pub(crate) fn push_system(chat: &mut impl super::ChatSink, line: String) {
    chat.push_event(ChatEvent::text_only(ChatEventKind::System, line));
}

/// Add one of the four lines as `CHAT_MSG_SYSTEM` through the chat add, in the caller's turn
/// (`0x509dd0`).
pub(crate) fn add_system(world: &mut World, line: String) {
    super::feed::add(
        world,
        ChatEvent::text_only(ChatEventKind::System, line).into(),
    );
}

/// The player-flags watch's mirror write on the active player (`0x6e0fd0`): a move in AFK, DND,
/// GM or `0x8000` (`0x6e0fe7`) stores `flags & 2` (`0x6e1023`), at the packet's step.
pub(crate) fn flags_watch(world: &mut World, old: u32, new: u32) {
    if (old ^ new) & 0x800e == 0 {
        return;
    }
    if let Some(mut mirror) = world.get_resource_mut::<AfkMirror>() {
        mirror.0 = new & 0x2;
    }
}

/// The chat's world-entry seed (`0x50df80`): 1 when the active player's flags carry AFK
/// (`0x50dfa7`), else 0, also with no active player (`0x50dfb7`).
pub(crate) fn seed_on_enter_world(world: &mut World) {
    let afk = world
        .query_filtered::<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>()
        .iter(world)
        .next()
        .is_some_and(|store| store.0.player_flags() & 0x2 != 0);
    if let Some(mut mirror) = world.get_resource_mut::<AfkMirror>() {
        mirror.0 = u32::from(afk);
    }
}

/// The movement AFK clears: jump, forward/back, strafe and keyboard turn, each calling the AFK
/// clear unforced in its own turn (`0x5fae0c`, `0x5faf49`, `0x5fb056`, `0x5fb14e`, `0x5fc069`).
/// They sit in the movement emitters, so only a key press clears; mouse-look turning never reaches
/// the keyboard-turn emitter, and knockback, fear and splines press nothing. Jump is here but not
/// in `player::follow`'s cancel set: two separate reference tables.
pub(crate) fn movement_clears_afk(world: &mut World) {
    use crate::bindings::Input;
    let moved = world
        .get_resource::<crate::bindings::BindingsState>()
        .is_some_and(|binds| {
            [
                Input::Jump,
                Input::MoveForward,
                Input::MoveBackward,
                Input::StrafeLeft,
                Input::StrafeRight,
                Input::TurnLeft,
                Input::TurnRight,
            ]
            .iter()
            .any(|&c| binds.just_pressed(c))
        });
    if moved {
        super::input::clear_afk(world, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The enUS `GlobalStrings.lua` values; `MARKED_DND` carries its period inside the format.
    fn strings(key: &str) -> Option<String> {
        Some(
            match key {
                "MARKED_AFK_MESSAGE" => "You are now AFK: %s",
                "CLEARED_AFK" => "You are no longer AFK.",
                "MARKED_DND" => "You are now DND: %s.",
                "CLEARED_DND" => "You are no longer marked DND.",
                "DEFAULT_AFK_MESSAGE" => "Away from Keyboard",
                "DEFAULT_DND_MESSAGE" => "Do not Disturb",
                _ => return None,
            }
            .to_string(),
        )
    }

    /// The world-entry seed stores 1 for a set AFK bit and 0 otherwise or with no active player
    /// (`0x50dfab`, `0x50dfb7`); the watch stores `flags & 2` on a `0x800e` move only (`0x6e0fe7`).
    #[test]
    fn the_mirror_is_seeded_at_entry_and_written_by_the_watch() {
        use benilla_protocol::field::FIELD_PLAYER_FLAGS;
        let mut world = World::new();
        world.insert_resource(AfkMirror(2));
        super::super::feed::enter_world(&mut world);
        assert_eq!(world.resource::<AfkMirror>().0, 0, "no active player");
        let me = world
            .spawn((
                crate::net::SelfPlayer,
                crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                    FIELD_PLAYER_FLAGS,
                    0x22,
                )])),
            ))
            .id();
        super::super::feed::enter_world(&mut world);
        assert_eq!(world.resource::<AfkMirror>().0, 1);
        world.entity_mut(me).despawn();
        flags_watch(&mut world, 0x2, 0x22);
        assert_eq!(world.resource::<AfkMirror>().0, 1, "outside the mask");
        flags_watch(&mut world, 0x0, 0x8002);
        assert_eq!(world.resource::<AfkMirror>().0, 2);
        flags_watch(&mut world, 0x8002, 0x2);
        assert_eq!(world.resource::<AfkMirror>().0, 2);
        world.resource_mut::<AfkMirror>().0 = 1;
        flags_watch(&mut world, 0x0, 0x8000);
        assert_eq!(world.resource::<AfkMirror>().0, 0, "0x8000 alone resyncs");
    }

    #[test]
    fn the_truth_table_row_for_row() {
        let dnd = |msg: &str, is: bool| dnd_line(msg.as_bytes(), is, &strings);

        assert_eq!(
            dnd("", false),
            AwayOutcome {
                line: "You are now DND: Do not Disturb.".into(),
                body: "Do not Disturb".into(),
            }
        );
        assert_eq!(
            dnd("", true),
            AwayOutcome {
                line: "You are no longer marked DND.".into(),
                body: Vec::new(),
            }
        );
        assert_eq!(
            dnd("busy", false),
            AwayOutcome {
                line: "You are now DND: busy.".into(),
                body: "busy".into(),
            }
        );
        // `/dnd M` while DND re-prints: DND reads the live bit and has no mirror.
        assert_eq!(
            dnd("busy", true),
            AwayOutcome {
                line: "You are now DND: busy.".into(),
                body: "busy".into(),
            },
            "DND re-marks where AFK stays silent — the reference's own asymmetry"
        );
    }

    fn fill_one(template: &str, arg: &str) -> String {
        shown(&fill_one_bytes(template.as_bytes(), arg.as_bytes()))
    }

    #[test]
    fn the_fill_handles_the_shapes_globalstrings_can_carry() {
        assert_eq!(
            fill_one("You are now AFK: %s", "brb"),
            "You are now AFK: brb"
        );
        assert_eq!(fill_one("%s.", "x"), "x.");
        assert_eq!(fill_one("no slot", "x"), "no slot");
        assert_eq!(fill_one("100%% sure", "x"), "100% sure");
        assert_eq!(
            fill_one("%s and %s", "x"),
            "x and %s",
            "only the first slot is ours"
        );
    }

    #[test]
    fn a_missing_string_table_does_not_invent_english() {
        let none = |_: &str| None;
        let out = dnd_line(b"brb", false, &none);
        assert_eq!(out.line, "");
        assert_eq!(
            out.body, b"brb",
            "the wire half never depends on the strings"
        );
    }
}
