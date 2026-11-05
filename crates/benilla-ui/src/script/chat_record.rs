//! The chat lines a complaint can name: 12340 keeps the last 60 recorded lines (`0x4fdd80`), and
//! `CanComplainChat` and `ComplainChat` pick the best one by name, text or line id (`0x4fb300`).

use mlua::{Lua, Value};

use super::model::Model;

/// The ring's slots (`0x4fb300` walks 60).
const RECORDS: usize = 60;

/// The 3.3.5a chat types a record keeps (`0x4fb1a0`, the Battle.net lanes aside).
const RECORDED: [u32; 18] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x09, 0x0a, 0x0b, 0x11, 0x17, 0x18, 0x27, 0x28, 0x2c,
    0x2d, 0x33,
];

/// One recorded line, as the app saw it reach the chat event.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatRecord {
    /// The 3.3.5a chat type.
    pub chat_type: u32,
    pub sender_guid: u64,
    /// The `u32` after the sender in `SMSG_MESSAGECHAT`.
    pub sender_word: u32,
    pub sender_name: String,
    /// The channel's name, empty off a channel.
    pub channel: String,
    /// The zone channel id, 0 off a zone channel.
    pub zone_channel: u32,
    pub text: String,
    pub line_id: u32,
    /// Unix seconds when the line arrived.
    pub time: u32,
}

/// The complaint the verb filed, for the app to check against the complained-about list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatComplaint {
    pub sender: u64,
    pub sender_word: u32,
    pub chat_type: u32,
    pub zone_channel: u32,
    /// Unix seconds the line arrived; the packet carries the age at send.
    pub time: u32,
    pub description: String,
}

/// The ring and what the complaint gate reads.
#[derive(Debug, Clone, Default)]
pub(crate) struct ChatRecords {
    /// `(record, description)` per slot, oldest overwritten first.
    slots: Vec<(ChatRecord, String)>,
    /// The next slot to write.
    next: usize,
    /// The complaint system's status from `SMSG_FEATURE_SYSTEM_STATUS`, 0 off.
    pub(crate) complaint_status: u8,
    pub(crate) complaints: Vec<ChatComplaint>,
}

/// `SStrCopy` into a `size`-byte buffer: at most `size - 1` bytes, cut on a character boundary.
fn clipped(s: &str, size: usize) -> String {
    let mut end = s.len().min(size - 1);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

impl ChatRecords {
    /// Record a line of a recorded type (`0x4fdd80`): the description names the type, channel,
    /// sender and both guids.
    fn record(&mut self, r: ChatRecord, player: u64) {
        if !RECORDED.contains(&r.chat_type) {
            return;
        }
        let description = clipped(
            &format!(
                "Type: [{}], Channel: [{}], Player Name: [{}], Sender GUID: [{:016X}], Active player: [{:016X}], Text: [{}]",
                r.chat_type, r.channel, r.sender_name, r.sender_guid, player, r.text
            ),
            3000,
        );
        let r = ChatRecord {
            sender_name: clipped(&r.sender_name, 48),
            text: clipped(&r.text, 3000),
            ..r
        };
        if self.slots.len() < RECORDS {
            self.slots.push((r, description));
        } else {
            self.slots[self.next] = (r, description);
        }
        self.next = (self.next + 1) % RECORDS;
    }

    /// The best line for a complaint (`0x4fb300`): of those matching every given filter, the one
    /// of the most urgent type, the oldest on a tie.
    fn find(
        &self,
        name: Option<&str>,
        text: Option<&str>,
        line_id: u32,
    ) -> Option<&(ChatRecord, String)> {
        let mut best = None;
        let mut best_priority = u32::MAX;
        let len = self.slots.len();
        for i in 0..len {
            let slot = &self.slots[(i + if len < RECORDS { 0 } else { self.next }) % len];
            let (r, _) = slot;
            let priority = priority(r.chat_type, r.zone_channel);
            if priority < best_priority
                && name.is_none_or(|n| n.eq_ignore_ascii_case(&r.sender_name))
                && text.is_none_or(|t| t.eq_ignore_ascii_case(&r.text))
                && (line_id == 0 || line_id == r.line_id)
            {
                best = Some(slot);
                best_priority = priority;
            }
        }
        best
    }
}

/// A line's complaint priority, lowest first (`0x4fb240`): whispers, then zone channels, custom
/// channels, battleground, raid, party, yell, say, emote, AFK and DND; any other type never wins.
fn priority(chat_type: u32, zone_channel: u32) -> u32 {
    match chat_type {
        0x07 => 0,
        0x11 => {
            if zone_channel != 0 {
                1
            } else {
                2
            }
        }
        0x2d => 3,
        0x2c => 4,
        0x27 => 5,
        0x28 => 6,
        0x03 => 7,
        0x33 => 8,
        0x02 => 9,
        0x06 => 10,
        0x01 => 11,
        0x0a => 12,
        0x17 => 13,
        0x18 => 14,
        _ => u32::MAX,
    }
}

/// Whether the player may complain about `guid` (`0x4ffdc0`): in the world, the system on, and
/// neither themself nor a friend.
fn can_complain(model: &Model, guid: u64) -> bool {
    let Some(me) = model.unit("player").filter(|u| u.exists) else {
        return false;
    };
    model.chat_records.complaint_status != 0
        && guid != me.guid
        && !model.social.friends.iter().any(|f| f.guid == guid)
}

impl super::UiScript {
    /// Record a chat line as its event fires.
    pub fn record_chat(&mut self, record: ChatRecord) {
        let mut model = self.model_mut();
        let player = model.unit("player").map_or(0, |u| u.guid);
        model.chat_records.record(record, player);
    }

    /// Push the complaint system's status.
    pub fn set_complaint_status(&mut self, status: u8) {
        set_field!(self, chat_records.complaint_status, status);
    }

    /// Drain the chat complaints filed since the last call.
    pub fn take_chat_complaints(&mut self) -> Vec<ChatComplaint> {
        drain_field!(self, chat_records.complaints)
    }
}

/// The verbs' shared argument read: a number is a line id, else a name and an optional text.
fn record_args(
    lua: &Lua,
    first: Value,
    second: Value,
    usage: &'static str,
) -> mlua::Result<(Option<String>, Option<String>, u32)> {
    if let Some(n) = lua.coerce_number(first.clone())? {
        return Ok((None, None, n as i64 as u32));
    }
    let Some(name) = super::binding_abi::optional_string(lua, &first) else {
        return Err(mlua::Error::runtime(usage));
    };
    Ok((
        Some(name),
        super::binding_abi::optional_string(lua, &second),
        0,
    ))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // CanComplainChat(lineID | name [, text]) (`0x500c80`): 1 when the best line's sender may be
    // complained about, else nil.
    g.set(
        "CanComplainChat",
        lua.create_function(|lua, (first, second): (Value, Value)| {
            let (name, text, line) = record_args(
                lua,
                first,
                second,
                "Usage: CanComplainChat(lineID) or CanComplainChat(name) or CanComplainChat(name, text)",
            )?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let ok = model
                .chat_records
                .find(name.as_deref(), text.as_deref(), line)
                .is_some_and(|(r, _)| can_complain(&model, r.sender_guid));
            Ok(super::binding_abi::flag(ok))
        })?,
    )?;

    // ComplainChat(lineID | name [, text]) (`0x50ed00`): files the best line's sender (`0x50dfd0`)
    // when `CanComplainChat` would pass.
    g.set(
        "ComplainChat",
        lua.create_function(|lua, (first, second): (Value, Value)| {
            let (name, text, line) = record_args(
                lua,
                first,
                second,
                "Usage: ComplainChat(lineID) or ComplainChat(name) or ComplainChat(name, text)",
            )?;
            let mut model = crate::script::model_mut_of(lua);
            let Some((r, description)) = model
                .chat_records
                .find(name.as_deref(), text.as_deref(), line)
                .filter(|(r, _)| can_complain(&model, r.sender_guid))
                .cloned()
            else {
                return Ok(());
            };
            model.chat_records.complaints.push(ChatComplaint {
                sender: r.sender_guid,
                sender_word: r.sender_word,
                chat_type: r.chat_type,
                zone_channel: r.zone_channel,
                time: r.time,
                description,
            });
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn line(chat_type: u32, guid: u64, name: &str, text: &str, line_id: u32) -> ChatRecord {
        ChatRecord {
            chat_type,
            sender_guid: guid,
            sender_name: name.into(),
            text: text.into(),
            line_id,
            time: 100,
            ..Default::default()
        }
    }

    /// `0x4fb240`: a whisper outranks a say from the same sender; a line id picks its own line.
    #[test]
    fn the_best_line_is_the_most_urgent_type() {
        let mut r = ChatRecords::default();
        r.record(line(0x01, 7, "Bob", "hi", 1), 1);
        r.record(line(0x07, 7, "Bob", "psst", 2), 1);
        r.record(line(0x00, 7, "Bob", "system", 3), 1);
        assert_eq!(r.find(Some("bob"), None, 0).unwrap().0.line_id, 2);
        assert_eq!(r.find(None, None, 1).unwrap().0.line_id, 1);
        assert!(
            r.find(None, None, 3).is_none(),
            "a system line is not recorded"
        );
        assert_eq!(
            r.find(None, None, 2).unwrap().1,
            "Type: [7], Channel: [], Player Name: [Bob], Sender GUID: [0000000000000007], Active player: [0000000000000001], Text: [psst]"
        );
    }

    /// The ring holds the last 60 lines.
    #[test]
    fn the_ring_keeps_sixty_lines() {
        let mut r = ChatRecords::default();
        for i in 1..=61 {
            r.record(line(0x01, 7, "Bob", "hi", i), 1);
        }
        assert!(r.find(None, None, 1).is_none());
        assert!(r.find(None, None, 61).is_some());
    }

    /// `0x4ffdc0`: the complaint system must be on; nothing files with it off.
    #[test]
    fn complaints_need_the_system_on() {
        let mut s = UiScript::new().unwrap();
        s.record_chat(line(0x01, 7, "Bob", "hi", 1));
        let can: Option<i32> = s.eval("return CanComplainChat(1)").unwrap();
        assert_eq!(can, None);
        assert!(s.eval::<()>("CanComplainChat()").is_err());
        assert!(s.eval::<()>("ComplainChat()").is_err());
    }
}
