//! `/afk` and `/dnd`, which the reference's `SendChatMessage` handles differently. DND reads
//! `PLAYER_FLAGS` bit `0x4` live and falls through to the plain `CMSG_MESSAGECHAT` send. AFK reads
//! the client-side [`AfkMirror`] and hands off to the player's AFK set or clear, which send their
//! own packets. Both echo before the send, never from the descriptor: the `PLAYER_FLAGS` delta arm
//! prints nothing. The four lines are `CHAT_MSG_SYSTEM` (type `0xA`); the `CHAT_MSG_AFK`/`DND`
//! events are other players' auto-replies.

use bevy::prelude::*;

use super::event::{ChatEvent, ChatEventKind};

/// The reference's optimistic mirror of our `PLAYER_FLAGS` AFK bit: written at command time,
/// reconciled from the descriptor, reseeded at world enter. A `u32` tested for non-zero: the
/// reconcile stores `PLAYER_FLAGS & 2`, so a set mirror can read 2. DND has no mirror, so a second
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
    /// The `CMSG_MESSAGECHAT` body, after the client's own default-message substitution.
    pub(crate) body: String,
}

/// `/dnd <msg>` against the live `PLAYER_FLAGS` bit `0x4`, the DND arm; unlike `/afk`, a repeat
/// with a message re-prints. `MARKED_DND` carries its period inside the format string, so it is
/// filled, never composed.
pub(crate) fn dnd_line(
    msg: &str,
    is_dnd: bool,
    strings: &impl Fn(&str) -> Option<String>,
) -> AwayOutcome {
    let text = |key: &str| strings(key).unwrap_or_default();
    if is_dnd && msg.is_empty() {
        return AwayOutcome {
            line: text("CLEARED_DND"),
            body: String::new(),
        };
    }
    let body = if msg.is_empty() {
        text("DEFAULT_DND_MESSAGE")
    } else {
        msg.to_string()
    };
    AwayOutcome {
        line: fill_one(&text("MARKED_DND"), &body),
        body,
    }
}

/// Fill the first `%s` of a GlobalString, which the reference `snprintf`s; `%%` is a percent.
pub(crate) fn fill_one(template: &str, arg: &str) -> String {
    let mut out = String::with_capacity(template.len() + arg.len());
    let mut chars = template.chars().peekable();
    let mut used = false;
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('s') if !used => {
                out.push_str(arg);
                used = true;
            }
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
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

/// The last `PLAYER_FLAGS` the reconcile saw; `None` with no local player in the world.
#[derive(Resource, Default)]
pub(crate) struct AfkMirrorMemo(Option<u32>);

/// Reconcile the mirror from our descriptor: the `PLAYER_FLAGS` delta arm and the world-enter
/// reseed. The delta arm writes `flags & 2` only when AFK, DND or GM moved; writing on any delta
/// would let an unrelated bit, such as resting, clear an optimistic AFK a round trip early.
pub(crate) fn reconcile_afk_mirror(
    self_q: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
    mut mirror: ResMut<AfkMirror>,
    mut memo: ResMut<AfkMirrorMemo>,
) {
    let Some(store) = self_q.iter().next() else {
        // Forgetting the memo makes the next world enter reseed rather than diff.
        memo.0 = None;
        return;
    };
    let flags = store.0.player_flags();
    match memo.0.replace(flags) {
        // World enter: seed outright.
        None => mirror.0 = flags & 0x2,
        // The delta arm: only an AFK/DND/GM move reconciles.
        Some(prev) if (prev ^ flags) & 0xE != 0 => mirror.0 = flags & 0x2,
        Some(_) => {}
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

    #[test]
    fn the_truth_table_row_for_row() {
        let dnd = |msg: &str, is: bool| dnd_line(msg, is, &strings);

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
                body: String::new(),
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
        let out = dnd_line("brb", false, &none);
        assert_eq!(out.line, "");
        assert_eq!(
            out.body, "brb",
            "the wire half never depends on the strings"
        );
    }
}
