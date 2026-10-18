//! Client packet body builders: plain body bytes, framed and encrypted elsewhere.

use super::addons::{addon_tail, addon_tail_wotlk, SecureAddon};
use super::{CharCreateReq, MovementInfo};

/// `CMSG_AUTH_SESSION` body: build, server id, account, seed, proof, then the addon block's
/// uncompressed size and zlib stream (`0x51d910`); stock sends [`super::STOCK_SECURE_ADDONS`].
pub fn auth_session(
    build: u32,
    username: &str,
    client_seed: u32,
    client_proof: &[u8; 20],
    addons: &[SecureAddon],
) -> Vec<u8> {
    let tail = addon_tail(addons);
    let mut body = Vec::with_capacity(34 + username.len() + tail.len());
    body.extend_from_slice(&build.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // server_id
    body.extend_from_slice(username.as_bytes());
    body.push(0);
    body.extend_from_slice(&client_seed.to_le_bytes());
    body.extend_from_slice(client_proof);
    body.extend_from_slice(&tail);
    body
}

/// 3.3.5a's `CMSG_AUTH_SESSION` body: build, login server id, account, login server type, seed,
/// region, battlegroup and realm ids, the `u64` DoS response, proof, then the count-framed addon
/// block (AzerothCore `WorldSocket::HandleAuthSession`). The realm id is the realm list's; the
/// server refuses one that is not its own. Login server id and type, region, battlegroup and DoS
/// response are 0, as the emulators ignore them; the reference's values are not established.
pub fn auth_session_wotlk(
    build: u32,
    username: &str,
    client_seed: u32,
    realm_id: u32,
    client_proof: &[u8; 20],
    addons: &[SecureAddon],
) -> Vec<u8> {
    let tail = addon_tail_wotlk(addons, 0);
    let mut body = Vec::with_capacity(58 + username.len() + tail.len());
    body.extend_from_slice(&build.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // login server id
    body.extend_from_slice(username.as_bytes());
    body.push(0);
    body.extend_from_slice(&0u32.to_le_bytes()); // login server type
    body.extend_from_slice(&client_seed.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // region id
    body.extend_from_slice(&0u32.to_le_bytes()); // battlegroup id
    body.extend_from_slice(&realm_id.to_le_bytes());
    body.extend_from_slice(&0u64.to_le_bytes()); // DoS response
    body.extend_from_slice(client_proof);
    body.extend_from_slice(&tail);
    body
}

/// `CMSG_CHAR_CREATE` body: the name cstring then nine bytes (`Packets/Character.cpp:4-19`); the
/// last, `outfit_id`, is 0 because the server ignores it (`CharacterHandler.cpp:310`).
pub fn char_create(req: &CharCreateReq) -> Vec<u8> {
    let mut body = Vec::with_capacity(req.name.len() + 10);
    body.extend_from_slice(req.name.as_bytes());
    body.push(0);
    body.extend_from_slice(&[
        req.race,
        req.class,
        req.gender,
        req.skin,
        req.face,
        req.hair_style,
        req.hair_color,
        req.facial_hair,
        0,
    ]);
    body
}

/// `CMSG_MESSAGECHAT` body: `u32` type and language, the target or channel name for whisper and
/// channel only (`None` otherwise), then the message (vmangos `Server/Packets/Chat.cpp:3-12`).
pub fn messagechat_kind(
    chat_type: u32,
    language: u32,
    target: Option<&str>,
    message: &str,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(message.len() + target.map_or(0, str::len) + 10);
    body.extend_from_slice(&chat_type.to_le_bytes());
    body.extend_from_slice(&language.to_le_bytes());
    if let Some(target) = target {
        body.extend_from_slice(target.as_bytes());
        body.push(0);
    }
    body.extend_from_slice(message.as_bytes());
    body.push(0);
    body
}

/// `CMSG_MESSAGECHAT` body for every type without a target or channel name.
pub fn messagechat(chat_type: u32, language: u32, message: &str) -> Vec<u8> {
    messagechat_kind(chat_type, language, None, message)
}

/// `CMSG_MESSAGECHAT` body for a whisper: the target's player name precedes the message.
pub fn messagechat_whisper(language: u32, target: &str, message: &str) -> Vec<u8> {
    messagechat_kind(super::CHAT_TYPE_WHISPER, language, Some(target), message)
}

/// `CMSG_MESSAGECHAT` body for a channel line: the channel name precedes the message.
pub fn messagechat_channel(language: u32, channel: &str, message: &str) -> Vec<u8> {
    messagechat_kind(super::CHAT_TYPE_CHANNEL, language, Some(channel), message)
}

/// Back-to-back cstrings, the shape of every `CMSG_CHANNEL_*` body (`Packets/Channel.cpp`).
fn cstrings_body(strings: &[&str]) -> Vec<u8> {
    let mut body = Vec::with_capacity(strings.iter().map(|s| s.len() + 1).sum());
    for s in strings {
        body.extend_from_slice(s.as_bytes());
        body.push(0);
    }
    body
}

/// `CMSG_JOIN_CHANNEL` body: channel name, then password (empty for none); 1.12 sends no channel
/// id (`Channel.cpp:3-7`).
pub fn join_channel(name: &str, password: &str) -> Vec<u8> {
    cstrings_body(&[name, password])
}

/// `CMSG_LEAVE_CHANNEL` body: the channel name (`Channel.cpp:9-12`).
pub fn leave_channel(name: &str) -> Vec<u8> {
    cstrings_body(&[name])
}

/// 3.3.5a's `CMSG_JOIN_CHANNEL` body, as the 12340 client builds it (`0x507c58`): `u32` the
/// `ChatChannels.dbc` id of a built-in channel (0 for a custom one), `u8` 1 when the client joins
/// on its own (the zone walk and the login re-join, `0x508139`, `0x50819c`) and 0 for a Lua join,
/// `u8` the voice flag, then the name and password.
pub fn join_channel_wotlk(
    channel_id: u32,
    auto_joined: bool,
    voice: bool,
    name: &str,
    password: &str,
) -> Vec<u8> {
    let mut body = channel_id.to_le_bytes().to_vec();
    body.push(u8::from(auto_joined));
    body.push(u8::from(voice));
    body.extend(cstrings_body(&[name, password]));
    body
}

/// 3.3.5a's `CMSG_LEAVE_CHANNEL` body (`0x5095ca`): `u32` the channel's id (0 for a custom one),
/// then its name.
pub fn leave_channel_wotlk(channel_id: u32, name: &str) -> Vec<u8> {
    let mut body = channel_id.to_le_bytes().to_vec();
    body.extend(cstrings_body(&[name]));
    body
}

/// `CMSG_CHANNEL_LIST` body, the member-roster request: the channel name (`Channel.cpp:14-17`).
pub fn channel_list(name: &str) -> Vec<u8> {
    cstrings_body(&[name])
}

/// `CMSG_CHANNEL_PASSWORD` body: channel name, then the new password (`Channel.cpp:19-23`).
pub fn channel_password(name: &str, password: &str) -> Vec<u8> {
    cstrings_body(&[name, password])
}

/// `CMSG_CHANNEL_SET_OWNER` body: channel name, then the new owner's name (`Channel.cpp:25-29`).
pub fn channel_set_owner(name: &str, player: &str) -> Vec<u8> {
    cstrings_body(&[name, player])
}

/// `CMSG_CHANNEL_OWNER` body: the channel name (`Channel.cpp:31-34`); the answer is a
/// `SMSG_CHANNEL_NOTIFY` owner notice.
pub fn channel_owner(name: &str) -> Vec<u8> {
    cstrings_body(&[name])
}

/// `CMSG_CHANNEL_MODERATOR` body: channel name, then the player's name (`Channel.cpp:36-40`).
pub fn channel_moderator(name: &str, player: &str) -> Vec<u8> {
    cstrings_body(&[name, player])
}

/// `CMSG_CHANNEL_UNMODERATOR` body: channel name, then the player's name (`Channel.cpp:42-46`).
pub fn channel_unmoderator(name: &str, player: &str) -> Vec<u8> {
    cstrings_body(&[name, player])
}

/// `CMSG_CHANNEL_MUTE` body: channel name, then the player's name (`Channel.cpp:48-52`).
pub fn channel_mute(name: &str, player: &str) -> Vec<u8> {
    cstrings_body(&[name, player])
}

/// `CMSG_CHANNEL_UNMUTE` body: channel name, then the player's name (`Channel.cpp:54-58`).
pub fn channel_unmute(name: &str, player: &str) -> Vec<u8> {
    cstrings_body(&[name, player])
}

/// `CMSG_CHANNEL_INVITE` body: channel name, then the invitee's name (`Channel.cpp:60-64`).
pub fn channel_invite(name: &str, player: &str) -> Vec<u8> {
    cstrings_body(&[name, player])
}

/// `CMSG_CHANNEL_KICK` body: channel name, then the player's name (`Channel.cpp:66-70`).
pub fn channel_kick(name: &str, player: &str) -> Vec<u8> {
    cstrings_body(&[name, player])
}

/// `CMSG_CHANNEL_BAN` body: channel name, then the player's name (`Channel.cpp:72-76`).
pub fn channel_ban(name: &str, player: &str) -> Vec<u8> {
    cstrings_body(&[name, player])
}

/// `CMSG_CHANNEL_UNBAN` body: channel name, then the player's name (`Channel.cpp:78-82`).
pub fn channel_unban(name: &str, player: &str) -> Vec<u8> {
    cstrings_body(&[name, player])
}

/// `CMSG_CHANNEL_ANNOUNCEMENTS` body, the join/leave notice toggle (`Channel.cpp:84-87`).
pub fn channel_announcements(name: &str) -> Vec<u8> {
    cstrings_body(&[name])
}

/// `CMSG_CHANNEL_MODERATE` body, the moderation toggle (`Channel.cpp:89-92`).
pub fn channel_moderate(name: &str) -> Vec<u8> {
    cstrings_body(&[name])
}

/// `CMSG_PLAYED_TIME` body, the `/played` request: empty (`Handlers/MiscHandler.cpp:935`).
pub fn played_time() -> Vec<u8> {
    Vec::new()
}

/// `CMSG_QUERY_TIME` body: empty (`Handlers/QueryHandler.cpp:107`). The answer is the server's
/// clock in unix seconds, the epoch timed-quest deadlines are stamped in.
pub fn query_time() -> Vec<u8> {
    Vec::new()
}

/// `MSG_RANDOM_ROLL` request body (`Server/Packets/Group.cpp:39-43`); the server silently drops
/// one outside `minimum <= maximum <= 10000`.
pub fn random_roll(min: u32, max: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(8);
    body.extend_from_slice(&min.to_le_bytes());
    body.extend_from_slice(&max.to_le_bytes());
    body
}

/// `CMSG_TEXT_EMOTE` body: `u32` EmotesText.dbc id, `u32` text variation (only relayed), `u64`
/// target or 0 (`Misc.cpp:60-65`). The server echoes `SMSG_TEXT_EMOTE` to us as well.
pub fn text_emote(text_id: u32, target: u64) -> Vec<u8> {
    let mut b = Vec::with_capacity(16);
    b.extend_from_slice(&text_id.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes()); // emoteNum: text-variation 0
    b.extend_from_slice(&target.to_le_bytes());
    b
}

pub fn full_guid(guid: u64) -> Vec<u8> {
    guid.to_le_bytes().to_vec()
}

/// `CMSG_CREATURE_QUERY` body: the template entry, then a full guid (vmangos `QueryCreature`).
pub fn creature_query(entry: u32, guid: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&entry.to_le_bytes());
    body.extend_from_slice(&guid.to_le_bytes());
    body
}

/// `CMSG_PET_NAME_QUERY` body: the pet number, then its full guid (`Server/Packets/Pet.cpp:3-7`);
/// the server stays silent unless the number matches the live pet (`PetHandler.cpp:190-192`).
pub fn pet_name_query(pet_number: u32, guid: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&pet_number.to_le_bytes());
    body.extend_from_slice(&guid.to_le_bytes());
    body
}

/// Body of a `MSG_MOVE_*` message: the mover's packed guid, then its `MovementInfo`.
pub fn movement(mover: u64, info: &MovementInfo) -> Vec<u8> {
    let mut body = Vec::with_capacity(40);
    info.write(mover, &mut body);
    body
}

/// The mover's packed guid, as the 3.3.5a acks open (AzerothCore `MovementHandler.cpp`).
fn packed(guid: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(9);
    crate::wire::write_packed_guid(guid, &mut body).expect("a Vec write cannot fail");
    body
}

/// `CMSG_MOVE_TIME_SKIPPED` body: the mover's packed guid, then the skipped ms
/// (`HandleMoveTimeSkippedOpcode`).
pub fn move_time_skipped(guid: u64, lag_ms: u32) -> Vec<u8> {
    let mut body = packed(guid);
    body.extend_from_slice(&lag_ms.to_le_bytes());
    body
}

/// `CMSG_MOVE_SPLINE_DONE` body: the mover's packed guid and `MovementInfo` at the ride's end,
/// then the acked spline id (`HandleMoveSplineDoneOpcode`).
pub fn move_spline_done(mover: u64, info: &MovementInfo, spline_id: u32) -> Vec<u8> {
    let mut body = movement(mover, info);
    body.extend_from_slice(&spline_id.to_le_bytes());
    body
}

/// `MSG_MOVE_TELEPORT_ACK` body: the mover's packed guid, the counter, the client time
/// (`HandleMoveTeleportAck`).
pub fn teleport_ack(guid: u64, counter: u32, time_ms: u32) -> Vec<u8> {
    let mut body = packed(guid);
    body.extend_from_slice(&counter.to_le_bytes());
    body.extend_from_slice(&time_ms.to_le_bytes());
    body
}

/// Body of all six `CMSG_FORCE_*_SPEED_CHANGE_ACK`s: the mover's packed guid, the echoed
/// counter, our live `MovementInfo`, the echoed speed (`HandleForceSpeedChangeAck`).
pub fn force_speed_ack(guid: u64, counter: u32, info: &MovementInfo, speed: f32) -> Vec<u8> {
    let mut body = packed(guid);
    body.extend_from_slice(&counter.to_le_bytes());
    info.write_body(&mut body);
    body.extend_from_slice(&speed.to_le_bytes());
    body
}

/// `CMSG_PING` body, the ~30 s keepalive (`0x537e10`): the sequence, then the last round trip in
/// ms, which the server stores as our latency; it echoes the sequence in `SMSG_PONG`.
pub fn ping(sequence: u32, last_rtt_ms: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(8);
    body.extend_from_slice(&sequence.to_le_bytes());
    body.extend_from_slice(&last_rtt_ms.to_le_bytes());
    body
}

/// Body of a movement-mode ack: the mover's packed guid, the echoed counter, our `MovementInfo`,
/// then for every mode but root the apply as an `f32` 1.0 or 0.0 (`0x71ef80`, the value the event
/// arm passes, `0x6ef860`). `info.flags` must carry the applied mode bit.
pub fn move_flag_ack(guid: u64, counter: u32, info: &MovementInfo, apply: Option<bool>) -> Vec<u8> {
    let mut body = packed(guid);
    body.extend_from_slice(&counter.to_le_bytes());
    info.write_body(&mut body);
    if let Some(apply) = apply {
        let value: f32 = if apply { 1.0 } else { 0.0 };
        body.extend_from_slice(&value.to_le_bytes());
    }
    body
}

/// `CMSG_MOVE_KNOCK_BACK_ACK` body, a movement-mode ack without `apply` (`HandleMoveKnockBackAck`).
/// `info` must carry `MOVEFLAG_JUMPING` and the launch quad, which the server matches.
pub fn knock_back_ack(guid: u64, counter: u32, info: &MovementInfo) -> Vec<u8> {
    move_flag_ack(guid, counter, info, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Vector3d;

    /// AzerothCore `HandleMoveSplineDoneOpcode` reads the packed guid, the info, then the id.
    #[test]
    fn move_spline_done_body_golden() {
        let info = MovementInfo {
            flags: 0, // no conditional MovementInfo tails
            timestamp: 0x1122_3344,
            position: Vector3d {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            },
            orientation: 0.5,
            transport: None,
            pitch: 0.0,
            fall_time: 0x5566_7788,
            jump: None,
        };
        let body = move_spline_done(0x3e9, &info, 0xAABB_CCDD);
        #[rustfmt::skip]
        let want: &[u8] = &[
            0x03, 0xE9, 0x03,       // packed guid 0x3e9
            0x00, 0x00, 0x00, 0x00, // flags
            0x00, 0x00,             // extra flags
            0x44, 0x33, 0x22, 0x11, // timestamp
            0x00, 0x00, 0x80, 0x3F, // pos.x = 1.0
            0x00, 0x00, 0x00, 0x40, // pos.y = 2.0
            0x00, 0x00, 0x40, 0x40, // pos.z = 3.0
            0x00, 0x00, 0x00, 0x3F, // orientation = 0.5
            0x88, 0x77, 0x66, 0x55, // fall_time
            0xDD, 0xCC, 0xBB, 0xAA, // splineId
        ];
        assert_eq!(body, want, "CMSG_MOVE_SPLINE_DONE body");
    }

    /// An ack's guid comes once, before its counter: the info that follows carries none.
    #[test]
    fn an_ack_packs_its_guid_once_before_the_counter() {
        let info = MovementInfo {
            flags: 0,
            timestamp: 0,
            position: Vector3d {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            orientation: 0.0,
            transport: None,
            pitch: 0.0,
            fall_time: 0,
            jump: None,
        };
        let ack = move_flag_ack(0x3e9, 7, &info, Some(true));
        assert_eq!(&ack[..7], &[0x03, 0xE9, 0x03, 7, 0, 0, 0]);
        let mut bare = Vec::new();
        info.write_body(&mut bare);
        assert_eq!(&ack[7..ack.len() - 4], &bare[..]);
        assert_eq!(&ack[ack.len() - 4..], &1.0f32.to_le_bytes());
    }
}
