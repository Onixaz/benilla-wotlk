//! The social family: the contact list (friends, ignores and voice mutes in one packet) and `/who`
//! (0x62-0x6d). The 3.3.5a client holds 100 friends, 50 ignores and 50 mutes (its friend list;
//! `GetNumFriends` `0x6b34a0`, `GetNumIgnores` `0x6b3630`, `GetNumMutes` `0x6b3680`).

use std::io::{self, Read};

use crate::wire::{capacity_hint, read_cstring, read_u32_le, read_u64_le, read_u8};

/// `SMSG_FRIEND_STATUS`'s result byte, as the friend list's status handler (`0x6b73a0`) switches on
/// it. Every result prints through the error display to the chat frame, not the red error line.
pub mod friend_result {
    /// `ERR_FRIEND_NOT_FOUND`, as [`NOT_FOUND`] (the emulators' `FRIEND_DB_ERROR`).
    pub const DB_ERROR: u8 = 0x00;
    /// `ERR_FRIEND_LIST_FULL`.
    pub const LIST_FULL: u8 = 0x01;
    /// `ERR_FRIEND_ONLINE_SS`: a friend logged in; the presence tail follows.
    pub const ONLINE: u8 = 0x02;
    /// `ERR_FRIEND_OFFLINE_S`: a friend logged out; no tail.
    pub const OFFLINE: u8 = 0x03;
    /// `ERR_FRIEND_NOT_FOUND`.
    pub const NOT_FOUND: u8 = 0x04;
    /// `ERR_FRIEND_REMOVED_S`: the ack for `CMSG_DEL_FRIEND`.
    pub const REMOVED: u8 = 0x05;
    /// `ERR_FRIEND_ADDED_S`, friend online: the note, then the presence tail.
    pub const ADDED_ONLINE: u8 = 0x06;
    /// `ERR_FRIEND_ADDED_S`, friend offline: the note alone.
    pub const ADDED_OFFLINE: u8 = 0x07;
    /// `ERR_FRIEND_ALREADY_S`.
    pub const ALREADY: u8 = 0x08;
    /// `ERR_FRIEND_SELF`.
    pub const SELF: u8 = 0x09;
    /// `ERR_FRIEND_WRONG_FACTION`.
    pub const ENEMY: u8 = 0x0A;
    /// `ERR_IGNORE_FULL`.
    pub const IGNORE_FULL: u8 = 0x0B;
    /// `ERR_IGNORE_SELF`.
    pub const IGNORE_SELF: u8 = 0x0C;
    /// `ERR_IGNORE_NOT_FOUND`.
    pub const IGNORE_NOT_FOUND: u8 = 0x0D;
    /// `ERR_IGNORE_ALREADY_S`.
    pub const IGNORE_ALREADY: u8 = 0x0E;
    /// `ERR_IGNORE_ADDED_S`: the ack for `CMSG_ADD_IGNORE`.
    pub const IGNORE_ADDED: u8 = 0x0F;
    /// `ERR_IGNORE_REMOVED_S`: the ack for `CMSG_DEL_IGNORE`.
    pub const IGNORE_REMOVED: u8 = 0x10;
    /// `ERR_IGNORE_AMBIGUOUS`.
    pub const IGNORE_AMBIGUOUS: u8 = 0x11;
    /// `ERR_VOICE_IGNORE_FULL`; 0x12-0x18 are the mute list's twins of 0x0b-0x11.
    pub const MUTE_FULL: u8 = 0x12;
    /// `ERR_VOICE_IGNORE_SELF`.
    pub const MUTE_SELF: u8 = 0x13;
    /// `ERR_VOICE_IGNORE_NOT_FOUND`.
    pub const MUTE_NOT_FOUND: u8 = 0x14;
    /// `ERR_VOICE_IGNORE_ALREADY_S`.
    pub const MUTE_ALREADY: u8 = 0x15;
    /// `ERR_VOICE_IGNORE_ADDED_S`: the ack for `CMSG_ADD_VOICE_IGNORE`.
    pub const MUTE_ADDED: u8 = 0x16;
    /// `ERR_VOICE_IGNORE_REMOVED_S`: the ack for `CMSG_DEL_VOICE_IGNORE`.
    pub const MUTE_REMOVED: u8 = 0x17;
    /// `ERR_VOICE_IGNORE_AMBIGUOUS`.
    pub const MUTE_AMBIGUOUS: u8 = 0x18;
    /// Shows nothing (the error display of the no-message id).
    pub const SILENT: u8 = 0x19;
    /// A friend's away flags changed: a `u8` presence follows; `FRIENDLIST_UPDATE`, no line.
    pub const FLAGS_CHANGED: u8 = 0x1A;
    /// A friend changed zone: a `u32` area follows; `FRIENDLIST_UPDATE`, no line.
    pub const AREA_CHANGED: u8 = 0x1B;
}

/// A contact entry's list bits (the list read, `0x6b7dd0`), also the list mask `SMSG_CONTACT_LIST`
/// opens with and `CMSG_CONTACT_LIST` asks for.
pub mod contact_flag {
    /// On the friend list: the entry carries the presence tail.
    pub const FRIEND: u32 = 0x1;
    pub const IGNORED: u32 = 0x2;
    /// On the voice mute list.
    pub const MUTED: u32 = 0x4;
}

/// A friend's presence byte, a flag set: any bit is online (the connected flag is `status != 0`).
pub mod friend_status {
    pub const OFFLINE: u8 = 0;
    pub const ONLINE: u8 = 0x1;
    /// `GetFriendInfo` shows `CHAT_FLAG_AFK` (`0x6b4130`).
    pub const AFK: u8 = 0x2;
    /// `CHAT_FLAG_DND`, which outranks AFK.
    pub const DND: u8 = 0x4;
    /// Recruit-A-Friend: `GetFriendInfo`'s eighth return.
    pub const RAF: u8 = 0x8;
}

/// One `SMSG_CONTACT_LIST` entry. The name is not on the wire: the client resolves it through the
/// name cache when it reads the entry.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContactEntry {
    pub guid: u64,
    /// The [`contact_flag`] lists this player is on.
    pub flags: u32,
    /// The friend note, up to 511 bytes; empty for none.
    pub note: String,
    /// A [`friend_status`] set, read only for a friend; 0 offline.
    pub status: u8,
    /// The zone id (`AreaTable.dbc`), level and class, sent only for an online friend.
    pub area: u32,
    pub level: u32,
    pub class: u32,
}

/// `SMSG_CONTACT_LIST`: the lists `mask` names replaced by the entries on them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContactList {
    /// The [`contact_flag`] lists this packet replaces; an entry's bits outside it are skipped.
    pub mask: u32,
    pub contacts: Vec<ContactEntry>,
}

/// Read `SMSG_CONTACT_LIST` (the list read, `0x6b7dd0`): `u32` list mask, `u32` count, then per
/// entry `u64` guid, `u32` flags, the note cstring and, for a friend, a `u8` status with `u32`
/// area/level/class when it is non-zero. A trailing 16-byte module key is left unread.
pub fn read_contact_list(r: &mut impl Read) -> io::Result<ContactList> {
    let mask = read_u32_le(r)?;
    let count = read_u32_le(r)?;
    // 100 friends and 50 each of ignores and mutes.
    let mut contacts = Vec::with_capacity(capacity_hint(count, 200));
    for _ in 0..count {
        let mut entry = ContactEntry {
            guid: read_u64_le(r)?,
            flags: read_u32_le(r)?,
            note: read_cstring(r)?,
            ..Default::default()
        };
        if entry.flags & contact_flag::FRIEND != 0 {
            entry.status = read_u8(r)?;
            if entry.status != friend_status::OFFLINE {
                entry.area = read_u32_le(r)?;
                entry.level = read_u32_le(r)?;
                entry.class = read_u32_le(r)?;
            }
        }
        contacts.push(entry);
    }
    Ok(ContactList { mask, contacts })
}

/// `SMSG_FRIEND_STATUS`: one result about one player, with whatever tail its code carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FriendStatusUpdate {
    /// A [`friend_result`] code, either an ack or a broadcast; the code says which.
    pub result: u8,
    /// The player the result is about; 0 for `NOT_FOUND`.
    pub guid: u64,
    /// The friend note, for the two `ADDED` codes.
    pub note: Option<String>,
    /// The presence tail, for [`friend_result::ONLINE`] and [`friend_result::ADDED_ONLINE`].
    pub online: Option<FriendOnline>,
    /// The new presence byte of [`friend_result::FLAGS_CHANGED`].
    pub status: Option<u8>,
    /// The new zone of [`friend_result::AREA_CHANGED`].
    pub area: Option<u32>,
}

impl FriendStatusUpdate {
    /// A result with no tail.
    pub fn bare(result: u8, guid: u64) -> Self {
        Self {
            result,
            guid,
            note: None,
            online: None,
            status: None,
            area: None,
        }
    }
}

/// The presence tail of an online `SMSG_FRIEND_STATUS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FriendOnline {
    /// A [`friend_status`] set.
    pub status: u8,
    /// Zone id.
    pub area: u32,
    pub level: u32,
    pub class: u32,
}

/// Read `SMSG_FRIEND_STATUS` (`0x6b86b0`, tails per code in the status handler `0x6b73a0`): `u8`
/// result, `u64` guid, then the note for the two `ADDED` codes, the presence tail for `ONLINE` and
/// `ADDED_ONLINE`, a `u8` for `FLAGS_CHANGED` and a `u32` for `AREA_CHANGED`.
pub fn read_friend_status(r: &mut impl Read) -> io::Result<FriendStatusUpdate> {
    use friend_result as R;
    let mut update = FriendStatusUpdate::bare(read_u8(r)?, read_u64_le(r)?);
    if matches!(update.result, R::ADDED_ONLINE | R::ADDED_OFFLINE) {
        update.note = Some(read_cstring(r)?);
    }
    match update.result {
        R::ONLINE | R::ADDED_ONLINE => {
            update.online = Some(FriendOnline {
                status: read_u8(r)?,
                area: read_u32_le(r)?,
                level: read_u32_le(r)?,
                class: read_u32_le(r)?,
            });
        }
        R::FLAGS_CHANGED => update.status = Some(read_u8(r)?),
        R::AREA_CHANGED => update.area = Some(read_u32_le(r)?),
        _ => {}
    }
    Ok(update)
}

/// One `/who` hit; unlike a friend row, every field is on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WhoEntry {
    pub name: String,
    /// An empty cstring when unguilded.
    pub guild: String,
    pub level: u32,
    pub class: u32,
    pub race: u32,
    /// 0 male, 1 female; the client names race and class through it.
    pub gender: u8,
    /// `AreaTable.dbc` zone id; the client resolves the name.
    pub zone: u32,
}

/// `SMSG_WHO`: the results and the two counts of the "N players total (M displayed)" line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WhoResults {
    /// Entries that follow; the client keeps the first 50 (`0x6b8751`) but reads them all.
    pub displayed: u32,
    /// The server's full match count, which can exceed [`Self::displayed`].
    pub total: u32,
    /// In server order, every one on the wire.
    pub entries: Vec<WhoEntry>,
}

/// Read `SMSG_WHO` (12340 `0x6b8720`): two `u32` counts, then `{name, guild, level, class, race,
/// gender u8, zone}` per entry, the gender read between race and zone (`0x6b8801`).
pub fn read_who(r: &mut impl Read) -> io::Result<WhoResults> {
    let displayed = read_u32_le(r)?;
    let total = read_u32_le(r)?;
    let mut entries = Vec::with_capacity(capacity_hint(displayed, 64));
    for _ in 0..displayed {
        entries.push(WhoEntry {
            name: read_cstring(r)?,
            guild: read_cstring(r)?,
            level: read_u32_le(r)?,
            class: read_u32_le(r)?,
            race: read_u32_le(r)?,
            gender: read_u8(r)?,
            zone: read_u32_le(r)?,
        });
    }
    Ok(WhoResults {
        displayed,
        total,
        entries,
    })
}

/// A `/who` query parsed from the typed filter, e.g. `z-"Elwynn Forest" 1-10`, which the engine
/// (not FrameXML) parses for `SendWho` (12340 `0x6b5630`). [`Default`] is a bare `/who`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhoRequest {
    /// Inclusive minimum level.
    pub level_min: u32,
    /// Inclusive maximum level; the client sends 100 when unset, not 255.
    pub level_max: u32,
    /// `n-`: a name substring, empty for any.
    pub player_name: String,
    /// `g-`: a guild-name substring, empty for any.
    pub guild_name: String,
    /// `r-`: a bitmask over race ids (`1 << race`), all-ones for any.
    pub race_mask: u32,
    /// `c-`: a bitmask over class ids (`1 << class`), all-ones for any.
    pub class_mask: u32,
    /// `z-`: zone ids, resolved from names client-side.
    pub zones: Vec<u32>,
    /// Untagged words; the server matches each against name, guild and zone name.
    pub search_terms: Vec<String>,
}

impl Default for WhoRequest {
    fn default() -> Self {
        Self {
            level_min: 0,
            level_max: 100,
            player_name: String::new(),
            guild_name: String::new(),
            race_mask: u32::MAX,
            class_mask: u32::MAX,
            zones: Vec::new(),
            search_terms: Vec::new(),
        }
    }
}

/// The client's own caps (`SendWho` `0x6b5630`); over them, the server drops the whole query.
pub const WHO_MAX_ZONES: usize = 10;
/// See [`WHO_MAX_ZONES`].
pub const WHO_MAX_SEARCH_TERMS: usize = 4;

/// Body of `CMSG_WHO`, in `SendWho`'s put order (`0x6b5630`). Both arrays are trimmed to the
/// caps, so an over-long query narrows instead of matching nobody.
pub fn who(request: &WhoRequest) -> Vec<u8> {
    let zones = &request.zones[..request.zones.len().min(WHO_MAX_ZONES)];
    let terms = &request.search_terms[..request.search_terms.len().min(WHO_MAX_SEARCH_TERMS)];

    let mut body = Vec::with_capacity(32 + request.player_name.len() + request.guild_name.len());
    body.extend_from_slice(&request.level_min.to_le_bytes());
    body.extend_from_slice(&request.level_max.to_le_bytes());
    push_cstring(&mut body, &request.player_name);
    push_cstring(&mut body, &request.guild_name);
    body.extend_from_slice(&request.race_mask.to_le_bytes());
    body.extend_from_slice(&request.class_mask.to_le_bytes());
    body.extend_from_slice(&(zones.len() as u32).to_le_bytes());
    for zone in zones {
        body.extend_from_slice(&zone.to_le_bytes());
    }
    body.extend_from_slice(&(terms.len() as u32).to_le_bytes());
    for term in terms {
        push_cstring(&mut body, term);
    }
    body
}

/// Body of `CMSG_CONTACT_LIST`: the [`contact_flag`] lists wanted; `ShowFriends()` asks for the
/// friends alone (`0x6b44f0`). The whole list also arrives unasked at login.
pub fn contact_list(mask: u32) -> Vec<u8> {
    mask.to_le_bytes().to_vec()
}

/// Body of `CMSG_ADD_FRIEND` (`0x6b5400`): the name, kept to 304 bytes, then the note, to 511.
pub fn add_friend(name: &str, note: &str) -> Vec<u8> {
    let name = truncate_bytes(name, 304);
    let note = truncate_bytes(note, 511);
    let mut body = Vec::with_capacity(name.len() + note.len() + 2);
    push_cstring(&mut body, name);
    push_cstring(&mut body, note);
    body
}

/// Body of `CMSG_DEL_FRIEND`: removal is by guid, not name.
pub fn del_friend(guid: u64) -> Vec<u8> {
    guid.to_le_bytes().to_vec()
}

/// Body of `CMSG_SET_CONTACT_NOTES` (`0x6b5540`): the guid, then the note as stored, to 511 bytes.
pub fn set_contact_notes(guid: u64, note: &str) -> Vec<u8> {
    let note = truncate_bytes(note, 511);
    let mut body = Vec::with_capacity(8 + note.len() + 1);
    body.extend_from_slice(&guid.to_le_bytes());
    push_cstring(&mut body, note);
    body
}

/// Body of `CMSG_ADD_IGNORE` (`0x6b5f70`): the name, kept to 255 bytes.
pub fn add_ignore(name: &str) -> Vec<u8> {
    cstring_body(truncate_bytes(name, 255))
}

/// Body of `CMSG_DEL_IGNORE`: removal is by guid.
pub fn del_ignore(guid: u64) -> Vec<u8> {
    guid.to_le_bytes().to_vec()
}

/// Body of `CMSG_ADD_VOICE_IGNORE` (`0x6b62c0`): the name, kept to 304 bytes.
pub fn add_mute(name: &str) -> Vec<u8> {
    cstring_body(truncate_bytes(name, 304))
}

/// Body of `CMSG_DEL_VOICE_IGNORE` (`0x6b6410`): by guid.
pub fn del_mute(guid: u64) -> Vec<u8> {
    guid.to_le_bytes().to_vec()
}

/// The reference's string copy into a buffer of `max + 1`: at most `max` bytes, cut on a character
/// boundary.
fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn cstring_body(s: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(s.len() + 1);
    push_cstring(&mut body, s);
    body
}

fn push_cstring(body: &mut Vec<u8>, s: &str) {
    body.extend_from_slice(s.as_bytes());
    body.push(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One packet, three lists: an offline friend with a note, an online AFK friend, an ignore and
    /// a mute, each entry's tail keyed on its own flags.
    #[test]
    fn the_contact_list_reads_each_entrys_tail_from_its_flags() {
        let mut body = Vec::new();
        body.extend_from_slice(&7u32.to_le_bytes()); // mask: all three lists
        body.extend_from_slice(&4u32.to_le_bytes()); // count
        body.extend_from_slice(&7u64.to_le_bytes());
        body.extend_from_slice(&contact_flag::FRIEND.to_le_bytes());
        body.extend_from_slice(b"raid healer\0");
        body.push(friend_status::OFFLINE);
        body.extend_from_slice(&9u64.to_le_bytes());
        body.extend_from_slice(&contact_flag::FRIEND.to_le_bytes());
        body.push(0);
        body.push(friend_status::ONLINE | friend_status::AFK);
        body.extend_from_slice(&1519u32.to_le_bytes());
        body.extend_from_slice(&80u32.to_le_bytes());
        body.extend_from_slice(&6u32.to_le_bytes());
        body.extend_from_slice(&11u64.to_le_bytes());
        body.extend_from_slice(&contact_flag::IGNORED.to_le_bytes());
        body.push(0);
        body.extend_from_slice(&12u64.to_le_bytes());
        body.extend_from_slice(&contact_flag::MUTED.to_le_bytes());
        body.push(0);

        let mut r = &body[..];
        let list = read_contact_list(&mut r).unwrap();
        assert!(r.is_empty(), "every byte read");
        assert_eq!(list.mask, 7);
        assert_eq!(
            list.contacts,
            vec![
                ContactEntry {
                    guid: 7,
                    flags: contact_flag::FRIEND,
                    note: "raid healer".into(),
                    ..Default::default()
                },
                ContactEntry {
                    guid: 9,
                    flags: contact_flag::FRIEND,
                    status: friend_status::ONLINE | friend_status::AFK,
                    area: 1519,
                    level: 80,
                    class: 6,
                    ..Default::default()
                },
                ContactEntry {
                    guid: 11,
                    flags: contact_flag::IGNORED,
                    ..Default::default()
                },
                ContactEntry {
                    guid: 12,
                    flags: contact_flag::MUTED,
                    ..Default::default()
                },
            ]
        );
    }

    #[test]
    fn friend_status_tails_are_keyed_on_the_result_code() {
        let mut online = vec![friend_result::ONLINE];
        online.extend_from_slice(&9u64.to_le_bytes());
        online.push(friend_status::ONLINE | friend_status::DND);
        online.extend_from_slice(&1519u32.to_le_bytes());
        online.extend_from_slice(&60u32.to_le_bytes());
        online.extend_from_slice(&8u32.to_le_bytes());
        assert_eq!(
            read_friend_status(&mut &online[..]).unwrap(),
            FriendStatusUpdate {
                online: Some(FriendOnline {
                    status: friend_status::ONLINE | friend_status::DND,
                    area: 1519,
                    level: 60,
                    class: 8,
                }),
                ..FriendStatusUpdate::bare(friend_result::ONLINE, 9)
            }
        );

        // ADDED_ONLINE: the note comes before the presence tail.
        let mut added = vec![friend_result::ADDED_ONLINE];
        added.extend_from_slice(&9u64.to_le_bytes());
        added.extend_from_slice(b"tank\0");
        added.push(friend_status::ONLINE);
        for v in [12u32, 10, 1] {
            added.extend_from_slice(&v.to_le_bytes());
        }
        let mut r = &added[..];
        let status = read_friend_status(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(status.note.as_deref(), Some("tank"));
        assert_eq!(status.online.map(|o| o.level), Some(10));

        let mut offline = vec![friend_result::ADDED_OFFLINE];
        offline.extend_from_slice(&9u64.to_le_bytes());
        offline.extend_from_slice(b"\0");
        let status = read_friend_status(&mut &offline[..]).unwrap();
        assert_eq!(status.note.as_deref(), Some(""));
        assert_eq!(status.online, None);

        let mut flags = vec![friend_result::FLAGS_CHANGED];
        flags.extend_from_slice(&9u64.to_le_bytes());
        flags.push(friend_status::ONLINE | friend_status::AFK);
        assert_eq!(
            read_friend_status(&mut &flags[..]).unwrap().status,
            Some(friend_status::ONLINE | friend_status::AFK)
        );

        let mut area = vec![friend_result::AREA_CHANGED];
        area.extend_from_slice(&9u64.to_le_bytes());
        area.extend_from_slice(&4395u32.to_le_bytes());
        assert_eq!(read_friend_status(&mut &area[..]).unwrap().area, Some(4395));

        let mut removed = vec![friend_result::REMOVED];
        removed.extend_from_slice(&9u64.to_le_bytes());
        removed.extend_from_slice(&[0xFF; 13]); // must be ignored, not parsed
        assert_eq!(
            read_friend_status(&mut &removed[..]).unwrap(),
            FriendStatusUpdate::bare(friend_result::REMOVED, 9)
        );
    }

    #[test]
    fn who_results_carry_both_counts() {
        let mut body = Vec::new();
        body.extend_from_slice(&1u32.to_le_bytes()); // displayed
        body.extend_from_slice(&57u32.to_le_bytes()); // total
        body.extend_from_slice(b"Tigole\0");
        body.extend_from_slice(b"Legacy of Steel\0");
        body.extend_from_slice(&40u32.to_le_bytes()); // level
        body.extend_from_slice(&4u32.to_le_bytes()); // class
        body.extend_from_slice(&1u32.to_le_bytes()); // race
        body.push(1); // gender
        body.extend_from_slice(&40u32.to_le_bytes()); // zone

        assert_eq!(
            read_who(&mut &body[..]).unwrap(),
            WhoResults {
                displayed: 1,
                total: 57,
                entries: vec![WhoEntry {
                    name: "Tigole".into(),
                    guild: "Legacy of Steel".into(),
                    level: 40,
                    class: 4,
                    race: 1,
                    gender: 1,
                    zone: 40,
                }],
            }
        );
    }

    #[test]
    fn who_reads_the_empty_guild_string() {
        let mut body = Vec::new();
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(b"Solo\0");
        body.extend_from_slice(b"\0");
        body.extend_from_slice(&5u32.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&3u32.to_le_bytes());
        body.push(0);
        body.extend_from_slice(&1u32.to_le_bytes());
        let who = read_who(&mut &body[..]).unwrap();
        assert_eq!(who.entries[0].guild, "");
        assert_eq!(who.entries[0].level, 5);
    }

    /// Two rows back to back: a missed gender byte shifts the second row's name by one.
    #[test]
    fn who_reads_each_rows_gender_so_the_next_row_stays_aligned() {
        let mut body = Vec::new();
        body.extend_from_slice(&2u32.to_le_bytes());
        body.extend_from_slice(&57u32.to_le_bytes());
        for (name, guild, level, class, race, gender, zone) in [
            ("Belinda", "", 80u32, 6u32, 10u32, 1u8, 4395u32),
            ("Thrall", "Horde", 80, 7, 2, 0, 1637),
        ] {
            body.extend_from_slice(name.as_bytes());
            body.push(0);
            body.extend_from_slice(guild.as_bytes());
            body.push(0);
            for v in [level, class, race] {
                body.extend_from_slice(&v.to_le_bytes());
            }
            body.push(gender);
            body.extend_from_slice(&zone.to_le_bytes());
        }
        let mut r = &body[..];
        let who = read_who(&mut r).unwrap();
        assert!(r.is_empty(), "every byte read");
        assert_eq!(
            who.entries,
            [
                WhoEntry {
                    name: "Belinda".into(),
                    guild: String::new(),
                    level: 80,
                    class: 6,
                    race: 10,
                    gender: 1,
                    zone: 4395,
                },
                WhoEntry {
                    name: "Thrall".into(),
                    guild: "Horde".into(),
                    level: 80,
                    class: 7,
                    race: 2,
                    gender: 0,
                    zone: 1637,
                },
            ]
        );
    }

    #[test]
    fn who_request_body_is_byte_exact() {
        let request = WhoRequest {
            level_min: 1,
            level_max: 10,
            player_name: "bob".into(),
            guild_name: String::new(),
            race_mask: 0xFFFF_FFFF,
            class_mask: 0x0000_0002,
            zones: vec![12],
            search_terms: vec!["elw".into()],
        };
        let mut want = Vec::new();
        want.extend_from_slice(&1u32.to_le_bytes());
        want.extend_from_slice(&10u32.to_le_bytes());
        want.extend_from_slice(b"bob\0");
        want.extend_from_slice(b"\0");
        want.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        want.extend_from_slice(&2u32.to_le_bytes());
        want.extend_from_slice(&1u32.to_le_bytes());
        want.extend_from_slice(&12u32.to_le_bytes());
        want.extend_from_slice(&1u32.to_le_bytes());
        want.extend_from_slice(b"elw\0");
        assert_eq!(who(&request), want);
    }

    /// A bare `/who` as bytes: levels 0 to 100, both names empty, every race and class, no lists.
    #[test]
    fn the_default_who_query_encodes_levels_0_to_100_with_every_filter_open() {
        let body = who(&WhoRequest::default());
        assert_eq!(&body[0..4], &0u32.to_le_bytes(), "levelMin");
        assert_eq!(&body[4..8], &100u32.to_le_bytes(), "levelMax — not 255");
        assert_eq!(&body[8..10], b"\0\0", "both name filters empty");
        assert_eq!(&body[10..14], &u32::MAX.to_le_bytes(), "every race");
        assert_eq!(&body[14..18], &u32::MAX.to_le_bytes(), "every class");
        assert_eq!(&body[18..22], &0u32.to_le_bytes(), "no zones");
        assert_eq!(&body[22..26], &0u32.to_le_bytes(), "no terms");
        assert_eq!(body.len(), 26);
    }

    #[test]
    fn who_trims_to_the_servers_caps() {
        let request = WhoRequest {
            zones: (0..15).collect(),
            search_terms: (0..7).map(|i| i.to_string()).collect(),
            ..Default::default()
        };
        let body = who(&request);
        let zone_count = u32::from_le_bytes(body[18..22].try_into().unwrap());
        assert_eq!(zone_count as usize, WHO_MAX_ZONES);
        let after_zones = 22 + WHO_MAX_ZONES * 4;
        let term_count = u32::from_le_bytes(body[after_zones..after_zones + 4].try_into().unwrap());
        assert_eq!(term_count as usize, WHO_MAX_SEARCH_TERMS);
    }

    #[test]
    fn the_contact_sends() {
        assert_eq!(contact_list(contact_flag::FRIEND), 1u32.to_le_bytes());
        assert_eq!(add_friend("Bob", ""), b"Bob\0\0");
        assert_eq!(add_friend("Bob", "tank"), b"Bob\0tank\0");
        assert_eq!(add_friend("Bob", &"n".repeat(600)).len(), 4 + 511 + 1);
        assert_eq!(add_ignore("Bob"), b"Bob\0");
        assert_eq!(add_mute("Bob"), b"Bob\0");
        assert_eq!(del_friend(0xDEAD_BEEF), 0xDEAD_BEEFu64.to_le_bytes());
        assert_eq!(del_ignore(0xDEAD_BEEF), 0xDEAD_BEEFu64.to_le_bytes());
        assert_eq!(del_mute(0xDEAD_BEEF), 0xDEAD_BEEFu64.to_le_bytes());
        let mut notes = 9u64.to_le_bytes().to_vec();
        notes.extend_from_slice(b"healer\0");
        assert_eq!(set_contact_notes(9, "healer"), notes);
        // A cut never splits a character: 510 bytes of `n` then a two-byte `é`.
        let long = format!("{}é", "n".repeat(510));
        assert_eq!(set_contact_notes(9, &long).len(), 8 + 510 + 1);
    }
}
