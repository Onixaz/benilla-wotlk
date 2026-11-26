//! The chat send types and the joined-channel slots, beside the stock `ChatEdit_*` machine
//! (`ChatFrame.lua:1782-2242`) that owns the edit box.

use bevy::prelude::*;

use benilla_ui::script::sstr_to_int;

use crate::net::ChatKind;

/// The sendable chat types, `SendChatMessage`'s name table (`0x4fbb20`), as the wire kind each
/// token maps to; the leader lanes are the verb's own pick, never a token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SendType {
    Say,
    Yell,
    Emote,
    Whisper,
    Party,
    Raid,
    RaidWarning,
    Guild,
    Officer,
    Battleground,
    Afk,
    Dnd,
    Channel,
    Bn,
    BnWhisper,
    BnWhisperInform,
    BnConversation,
}

impl SendType {
    /// The `SendChatMessage` chat-type token; `None` for a made-up one, reported rather than sent
    /// as SAY. `AFK` and `DND` are sends: types `0x14`/`0x15` carrying the away message, which the
    /// server turns into the flag (vmangos `ChatHandler.cpp:611-648`).
    pub(crate) fn from_token(token: &str) -> Option<SendType> {
        Some(match token {
            "SAY" => SendType::Say,
            "YELL" => SendType::Yell,
            "EMOTE" => SendType::Emote,
            "WHISPER" => SendType::Whisper,
            "PARTY" => SendType::Party,
            "RAID" => SendType::Raid,
            "RAID_WARNING" => SendType::RaidWarning,
            "GUILD" => SendType::Guild,
            "OFFICER" => SendType::Officer,
            "BATTLEGROUND" => SendType::Battleground,
            "AFK" => SendType::Afk,
            "DND" => SendType::Dnd,
            "CHANNEL" => SendType::Channel,
            "BN" => SendType::Bn,
            "BN_WHISPER" => SendType::BnWhisper,
            "BN_WHISPER_INFORM" => SendType::BnWhisperInform,
            "BN_CONVERSATION" => SendType::BnConversation,
            _ => return None,
        })
    }

    /// The wire kind this type sends as.
    pub(crate) fn wire(self) -> ChatKind {
        match self {
            SendType::Say => ChatKind::Say,
            SendType::Yell => ChatKind::Yell,
            SendType::Emote => ChatKind::Emote,
            SendType::Whisper => ChatKind::Whisper,
            SendType::Party => ChatKind::Party,
            SendType::Raid => ChatKind::Raid,
            SendType::RaidWarning => ChatKind::RaidWarning,
            SendType::Guild => ChatKind::Guild,
            SendType::Officer => ChatKind::Officer,
            SendType::Battleground => ChatKind::Battleground,
            SendType::Afk => ChatKind::Afk,
            SendType::Dnd => ChatKind::Dnd,
            SendType::Channel => ChatKind::Channel,
            SendType::Bn => ChatKind::Bn,
            SendType::BnWhisper => ChatKind::BnWhisper,
            SendType::BnWhisperInform => ChatKind::BnWhisperInform,
            SendType::BnConversation => ChatKind::BnConversation,
        }
    }
}

/// The client refuses an eleventh channel; the boot-seeded `CHANNEL1`-`CHANNEL10` colour rows are
/// the same ten.
pub(crate) const MAX_CHANNELS: usize = 10;

/// The joined channels as the reference's slot array (stride `0xa0`, number at `+0x00`): `/N` is
/// slot N. The allocator reuses the first free entry before growing, and a leave zeroes the number
/// in place, so no other channel is renumbered and a hole answers "not joined".
#[derive(Resource, Default)]
pub(crate) struct ChannelState {
    /// Slot `i` is channel number `i + 1`; `None` is a freed slot, kept so the numbers above it
    /// do not move. Never longer than [`MAX_CHANNELS`].
    pub joined: Vec<Option<ChannelSlot>>,
    /// `ChatChannels.dbc`: it composes the auto-join names and resolves a chat event's arg7 from a
    /// name, as the server does. Empty without an install.
    pub channels: benilla_formats::ChatChannelsCatalog,
    /// The `ZONECHANNELS` mask, the reference's `ds:`, bit `1 << (ChannelID - 1)`: seeded from the
    /// chat cache or the `INITIAL` rows, set by a confirmed join and cleared only by an explicit
    /// leave, never the walk's LEAVE. Never derive it from the roster: each saved window's channel
    /// bits are ANDed with it, so an empty roster would save every window without its channels.
    ///
    /// `None` until the login reads the file: the reference's ready flag `ds:`, set by the cache
    /// loader and tested first by the walk; the saver writes nothing from `None`.
    pub zone_mask: Option<u32>,
    /// The custom channels to re-join at the next login, the chat cache's `CHANNELS` list. The
    /// reference writes it from its live slots at teardown; ours are cleared before the flush, so
    /// the list is kept here: seated from the file, grown by a confirmed custom join, shrunk by an
    /// explicit leave. A channel whose re-join is refused, or that kicks the player, stays listed
    /// until a `/leave`, where the reference's file drops it (#1518).
    pub custom: Vec<SavedChannel>,
    /// The channel pane's display list, selection, collapsed headers and roster: the game's, so a
    /// reload keeps them, as `GetSelectedDisplayChannel` reads a game global (`0x4ffa10`).
    pub pane: benilla_ui::script::ChannelPane,
}

/// A channel the chat cache re-joins at login: its header line `<name> <voice> <number>`
/// (`0x4fd455`), the slot's name, voice flag (`+0xa4`) and number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SavedChannel {
    pub(crate) name: String,
    pub(crate) voice: bool,
    pub(crate) number: u32,
}

/// One slot, the reference's `+ n*0xa0` record, in the fields this client uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChannelSlot {
    /// `+0x04`, the current name; the zone walk renames it in place.
    pub name: String,
    /// `+0x94`, the ChannelID the claim stored (`0x507795`), 0 for a custom channel: the join and
    /// the leave find a zone channel's record by it (`0x507b56`, `0x509581`).
    pub id: u32,
    /// `+0x9c`, the slot's state.
    pub state: SlotState,
    /// `+0xa0`: we asked to leave (`0x509628`); a join request clears it (`0x5077b4`).
    pub leave_asked: bool,
    /// `+0x90`, the channel flags its confirmed join (`0x5078eb`) or its last display list or
    /// user-list notice (`0x503a69`) carried.
    pub flags: u8,
    /// `+0x98`, the confirmed join's second word (`0x5078e5`).
    pub instance: u32,
    /// `+0xa4`, the join's voice flag (`0x5077c8`), saved with a permanent custom channel.
    pub voice: bool,
    /// `+0xa5`, the join's permanent flag (`0x5077d9`): the chat cache saves a custom record only
    /// with it (`0x4fd432`).
    pub permanent: bool,
}

/// The reference's per-slot state (`+0x9c`), in the values whose notice token differs: the stock
/// `ChatFrame_OnEvent`'s `YOU_LEFT` branch deletes the window's channel registration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SlotState {
    /// `0`, confirmed by the server's `YOU_JOINED` (`0x5078b3`): the only state the number legs,
    /// the channel list, a `CHANNEL` send and `ListChannels` read (`0x4fe8a3`, `0x4fe681`,
    /// `0x50d565`, `0x50db1a`).
    #[default]
    Joined,
    /// `1`, the join's claim (`0x5077a7`) awaiting its `YOU_JOINED`: found by name (`0x4fe8e4`)
    /// and by ChannelID, read as joined by nothing.
    Pending,
    /// `2`, renamed with its re-join pending: the confirming `YOU_JOINED` carries the `YOU_CHANGED`
    /// token, "Changed Channel: [%s]".
    Renamed,
    /// `3`, suspended: the row stopped applying, in 1.12 data a city-only channel outside a
    /// capital. The record and number survive, and `YOU_LEFT` carries the `SUSPENDED` token, which
    /// keeps the window's registration.
    Suspended,
}

impl ChannelSlot {
    /// A new slot in state `Joined`: claimed by the walk at send time or by a confirmed join.
    pub(crate) fn joined(name: &str) -> Self {
        ChannelSlot {
            name: name.to_string(),
            id: 0,
            state: SlotState::Joined,
            leave_asked: false,
            flags: 0,
            instance: 0,
            voice: false,
            permanent: false,
        }
    }

    /// The join's new record for ChannelID `id`, in state 1 until its `YOU_JOINED` (`0x5077a7`).
    pub(crate) fn pending(name: &str, id: u32) -> Self {
        ChannelSlot {
            state: SlotState::Pending,
            id,
            ..Self::joined(name)
        }
    }

    /// A zone channel's record in state 0, ChannelID `id`.
    #[cfg(test)]
    pub(crate) fn zone(name: &str, id: u32) -> Self {
        ChannelSlot {
            id,
            ..Self::joined(name)
        }
    }
}

/// What the join found or made of its record (`0x507a80`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JoinRecord {
    /// A record holds the channel already: no claim and no send (`0x507c1b` -> `0x507cea`).
    Held,
    /// A new record under this number, the join sent.
    Claimed(u32),
    /// All ten records taken: the verb's `ERR_TOO_MANY_CHAT_CHANNELS` unless quiet (`0x507755`),
    /// and the join returns before its send (`0x507c51`).
    Full,
    /// The claim found the name held by another record (`0x5076c3`): the join returns.
    Refused,
}

/// `id`'s `ZONECHANNELS` bit, `1 << (ChannelID - 1)`; nothing outside `1..=32` has one.
pub(crate) fn zone_bit(id: u32) -> u32 {
    if id == 0 || id > 32 {
        0
    } else {
        1 << (id - 1)
    }
}

impl ChannelState {
    /// The `CMSG_JOIN_CHANNEL` ask for `name`, carrying the ChannelID its join matched
    /// (`0x507c7e`), 0 for a custom channel. `auto_joined` is the client's own join, the zone
    /// walk's and the login re-join's.
    pub(crate) fn join_command(
        &self,
        id: u32,
        name: String,
        password: String,
        auto_joined: bool,
        voice: bool,
    ) -> crate::net::ClientCommand {
        crate::net::ClientCommand::JoinChannel {
            channel_id: id,
            name,
            password,
            auto_joined,
            voice,
        }
    }

    /// The `CMSG_LEAVE_CHANNEL` ask for the record holding `name`, with the record's ChannelID
    /// (`0x5095f1`); no record sends id 0.
    pub(crate) fn leave_command(&self, name: String) -> crate::net::ClientCommand {
        crate::net::ClientCommand::LeaveChannel {
            channel_id: self.slot_id(&name),
            name,
        }
    }

    /// The ChannelID (`+0x94`) of the record holding `name`, 0 with none.
    pub(crate) fn slot_id(&self, name: &str) -> u32 {
        self.number_of(name)
            .and_then(|n| self.slot(n))
            .map_or(0, |s| s.id)
    }

    /// A confirmed join sets the channel's mask bit (in the `YOU_JOINED` arm), only on a held slot:
    /// a join the server made unasked finds no record and sets nothing (`0x507880`).
    pub(crate) fn note_zone_channel_joined(&mut self, name: &str) {
        if self.number_of(name).is_none() {
            return;
        }
        // The bit is the record's ChannelID's (`0x5078ab`).
        let bit = zone_bit(self.slot_id(name));
        match &mut self.zone_mask {
            Some(mask) => *mask |= bit,
            // No join precedes the cache loader, so this is a broken ordering.
            None if bit != 0 => warn!(
                "chat: {name:?} joined before the zone mask was seated — bit {bit:#x} dropped"
            ),
            None => {}
        }
    }

    /// An explicit leave clears the bit, found as the reference finds it: by the slot carrying the
    /// wire name.
    pub(crate) fn note_zone_channel_left(&mut self, name: &str) {
        if self.number_of(name).is_none() {
            return;
        }
        // The bit is the record's ChannelID's (`0x509618`-`0x509639`).
        let bit = zone_bit(self.slot_id(name));
        if let Some(mask) = &mut self.zone_mask {
            *mask &= !bit;
        }
    }

    /// A confirmed join of a channel with no `ChatChannels.dbc` id enters [`Self::custom`] under
    /// the number it holds, or moves an entry already there to it.
    pub(crate) fn note_custom_channel_joined(&mut self, name: &str) {
        let Some(number) = self.number_of(name) else {
            return;
        };
        if self.slot_id(name) != 0 {
            return;
        }
        // The save writes a custom record only with its permanent flag, and its voice flag
        // (`0x4fd432`, `0x4fd43e`).
        let Some((voice, true)) = self.slot(number).map(|s| (s.voice, s.permanent)) else {
            return;
        };
        match self
            .custom
            .iter_mut()
            .find(|c| c.name.eq_ignore_ascii_case(name))
        {
            Some(c) => {
                c.number = number;
                c.voice = voice;
            }
            None => self.custom.push(SavedChannel {
                name: name.to_string(),
                voice,
                number,
            }),
        }
    }

    /// The claim's voice and permanent flags on record `n` (`0x5077c8`, `0x5077d9`).
    pub(crate) fn set_join_flags(&mut self, n: u32, voice: bool, permanent: bool) {
        let slot = (n as usize)
            .checked_sub(1)
            .and_then(|i| self.joined.get_mut(i));
        if let Some(Some(slot)) = slot {
            slot.voice = voice;
            slot.permanent = permanent;
        }
    }

    /// An explicit leave drops a custom channel, case-insensitively as the server's names are.
    pub(crate) fn note_custom_channel_left(&mut self, name: &str) {
        self.custom.retain(|c| !c.name.eq_ignore_ascii_case(name));
    }

    /// Whether the mask carries `id`'s bit, the walk's live predicate.
    pub(crate) fn zone_row_wanted(&self, id: u32) -> bool {
        self.zone_mask.is_some_and(|mask| mask & zone_bit(id) != 0)
    }

    /// `SendChatMessage`'s channel target: the integer parse straight, with no name leg, so only a
    /// number naming a confirmed slot sends, and the packet carries that slot's name. Anything
    /// else, a name included, sends nothing.
    pub(crate) fn send_target(&self, arg: &str) -> Option<String> {
        self.confirmed_slot_name(sstr_to_int(arg))
            .map(str::to_string)
    }

    /// The by-number getter: slot `n`'s name when it is in range, not a hole, and in state 0 (here
    /// [`SlotState::Joined`]); below 1, past the end, a hole or another state is `None`.
    fn confirmed_slot_name(&self, n: i32) -> Option<&str> {
        usize::try_from(n)
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|i| self.joined.get(i))
            .and_then(|slot| slot.as_ref())
            .filter(|slot| slot.state == SlotState::Joined)
            .map(|slot| slot.name.as_str())
    }

    /// Rename a slot in place at send time, the reference's from the walk: the server's `YOU_LEFT`
    /// for the old name then finds no slot, its args default, and the stock `ChatFrame_OnEvent`
    /// returns before its `YOU_LEFT` arm deletes the window's channel registration
    /// (`ChatFrame.lua:1382-1384`), which nothing re-adds.
    pub(crate) fn rename_slot(&mut self, old: &str, new: &str) -> Option<u32> {
        let n = self.number_of(old)?;
        let slot = self.joined[n as usize - 1].as_mut()?;
        slot.name = new.to_string();
        // `(old == 3) ? 1 : 2`. A suspended slot comes back as a pending join, so its notice reads
        // `YOU_JOINED`; any other is a rename awaiting its re-join.
        slot.state = if slot.state == SlotState::Suspended {
            SlotState::Pending
        } else {
            SlotState::Renamed
        };
        Some(n)
    }

    /// Suspend the slot holding `name`: the LEAVE has gone, and keeping the record makes the
    /// returning notice `SUSPENDED`, so the window keeps its registration.
    pub(crate) fn suspend_slot(&mut self, name: &str) -> Option<u32> {
        let n = self.number_of(name)?;
        self.joined[n as usize - 1].as_mut()?.state = SlotState::Suspended;
        Some(n)
    }

    /// The name the slot of `name`'s zone channel holds when it is another one: the confirm finds
    /// a zone channel's record by its ChannelID (`0x507880`). `None` for a custom channel, a held
    /// name or no slot.
    pub(crate) fn zone_slot_held_as(&self, name: &str) -> Option<String> {
        let id = self.channels.zone_channel_id(name);
        if id == 0 || self.number_of(name).is_some() {
            return None;
        }
        self.joined
            .iter()
            .flatten()
            .find(|s| s.id == id)
            .map(|s| s.name.clone())
    }

    /// The confirming `YOU_JOINED` writes state 0 unconditionally; separate from
    /// [`Self::claim_slot`], as a renamed slot is already numbered.
    pub(crate) fn confirm_slot(&mut self, name: &str) {
        if let Some(n) = self.number_of(name) {
            if let Some(slot) = self.joined[n as usize - 1].as_mut() {
                slot.state = SlotState::Joined;
            }
        }
    }

    /// The confirmed join's second word and flags byte into the record holding `name` (`0x5078e5`,
    /// `0x5078eb`).
    pub(crate) fn set_slot_join(&mut self, name: &str, instance: u32, flags: u8) {
        if let Some(n) = self.number_of(name) {
            if let Some(slot) = self.joined[n as usize - 1].as_mut() {
                slot.instance = instance;
                slot.flags = flags;
            }
        }
    }

    /// Mark the slot holding `name` as left by our own ask, the explicit leave's (`0x509628`).
    pub(crate) fn note_leave_asked(&mut self, name: &str) {
        if let Some(n) = self.number_of(name) {
            if let Some(slot) = self.joined[n as usize - 1].as_mut() {
                slot.leave_asked = true;
            }
        }
    }

    /// A voice notice's write to the record holding `name`, found by name (`0x4fdfc0`): `on` sets
    /// the flags byte's `0x80` (`0x50e9b0`, `0x50e9f2`), else clears it (`0x50ea4d`). Whether a
    /// record held it.
    pub(crate) fn set_voice(&mut self, name: &str, on: bool) -> bool {
        let Some(slot) = self
            .number_of(name)
            .and_then(|n| self.joined[n as usize - 1].as_mut())
        else {
            return false;
        };
        if on {
            slot.flags |= 0x80;
        } else {
            slot.flags &= 0x7f;
        }
        true
    }

    /// Whether we asked to leave the slot holding `name`; `None` when no slot holds it.
    pub(crate) fn leave_asked(&self, name: &str) -> Option<bool> {
        self.number_of(name)
            .and_then(|n| self.joined[n as usize - 1].as_ref())
            .map(|s| s.leave_asked)
    }

    /// The state of the slot holding `name`; `None` is the reference's no-record leg.
    pub(crate) fn slot_state(&self, name: &str) -> Option<SlotState> {
        self.number_of(name)
            .and_then(|n| self.joined[n as usize - 1].as_ref())
            .map(|s| s.state)
    }

    /// The slots' names, holes kept so `/N` addresses the right channel.
    #[cfg(test)]
    pub(crate) fn names(&self) -> Vec<Option<String>> {
        self.joined
            .iter()
            .map(|s| s.as_ref().map(|s| s.name.clone()))
            .collect()
    }

    /// The records as the verbs read them at the call, holes kept: a zone channel carries its
    /// row's Shortcut, which the pane lists it by (`0x4fdfa5`), and a suspended slot lists grey.
    pub(crate) fn ui_slots(&self) -> Vec<Option<benilla_ui::script::ChannelSlotInfo>> {
        self.joined
            .iter()
            .map(|s| {
                s.as_ref().map(|s| benilla_ui::script::ChannelSlotInfo {
                    name: s.name.clone(),
                    // The record's ChannelID's row (`0x4fdf8f`-`0x4fdfa5`).
                    shortcut: (s.id != 0)
                        .then(|| self.channels.rows().iter().find(|r| r.id == s.id))
                        .flatten()
                        .map(|r| r.shortcut.clone()),
                    active: s.state == SlotState::Joined,
                    flags: s.flags,
                    instance: s.instance,
                })
            })
            .collect()
    }

    /// Every joined channel's name, holes skipped.
    pub(crate) fn iter_names(&self) -> impl Iterator<Item = &str> {
        self.joined.iter().flatten().map(|s| s.name.as_str())
    }

    /// The 1-based number of `name` (case-insensitive), if joined.
    pub(crate) fn number_of(&self, name: &str) -> Option<u32> {
        self.joined
            .iter()
            .position(|c| {
                c.as_ref()
                    .is_some_and(|c| c.name.eq_ignore_ascii_case(name))
            })
            .map(|i| i as u32 + 1)
    }

    /// Give `name` the first free slot, else a new one under [`MAX_CHANNELS`], for a test: the
    /// ChannelID is the one the catalog resolves the name to.
    #[cfg(test)]
    pub(crate) fn claim_slot(&mut self, name: &str) -> Option<u32> {
        let id = self.channels.zone_channel_id(name);
        self.claim_walk_slot(name, id)
    }

    /// The zone walk's registration of row `id` under `name`: the first free slot, else a new one
    /// under [`MAX_CHANNELS`]; a name already held keeps its number. A join claims through
    /// [`Self::claim_for_join`].
    pub(crate) fn claim_walk_slot(&mut self, name: &str, id: u32) -> Option<u32> {
        if let Some(n) = self.number_of(name) {
            // A confirmed join on a held slot clears its suspension: the other half of the state-3
            // bypass.
            if let Some(slot) = self.joined[n as usize - 1].as_mut() {
                slot.state = SlotState::Joined;
                slot.leave_asked = false;
            }
            return Some(n);
        }
        let slot = ChannelSlot {
            id,
            ..ChannelSlot::joined(name)
        };
        if let Some(i) = self.joined.iter().position(Option::is_none) {
            self.joined[i] = Some(slot);
            return Some(i as u32 + 1);
        }
        if self.joined.len() >= MAX_CHANNELS {
            return None;
        }
        self.joined.push(Some(slot));
        Some(self.joined.len() as u32)
    }

    /// The record of ChannelID `id`, else for a custom channel (`id` 0) the one holding `name`:
    /// the join's and the leave's lookup (`0x507b3a`-`0x507c11`, `0x509562`-`0x5095ae`).
    fn record_for(&self, name: &str, id: u32) -> Option<u32> {
        if id == 0 {
            return self.number_of(name);
        }
        self.joined
            .iter()
            .position(|s| s.as_ref().is_some_and(|s| s.id == id))
            .map(|i| i as u32 + 1)
    }

    /// The join's record (`0x507a80`) for `name` and the ChannelID its verb matched (`0x507baa`),
    /// 0 for a custom channel: a zone channel's found by that id, a custom one's by name
    /// (`0x507b3a`-`0x507c11`); one already held skips the claim and the send (`0x507c1b`).
    /// Otherwise a new record is claimed (`0x507c47` -> `0x5076b0`), at `number` when that slot is
    /// free (`0x5076f6`-`0x507727`), else at the first free one or a new one under ten.
    pub(crate) fn claim_for_join(&mut self, name: &str, id: u32, number: u32) -> JoinRecord {
        if self.record_for(name, id).is_some() {
            return JoinRecord::Held;
        }
        // The claim refuses a name another record holds (`0x5076b7`).
        if self.number_of(name).is_some() {
            return JoinRecord::Refused;
        }
        let at = usize::try_from(number)
            .ok()
            .filter(|n| (1..=MAX_CHANNELS).contains(n))
            .and_then(|n| {
                if n > self.joined.len() {
                    self.joined.resize(n, None);
                    Some(n - 1)
                } else {
                    self.joined[n - 1].is_none().then_some(n - 1)
                }
            })
            .or_else(|| self.joined.iter().position(Option::is_none));
        let i = match at {
            Some(i) => i,
            // All ten taken (`0x507746`).
            None if self.joined.len() >= MAX_CHANNELS => return JoinRecord::Full,
            None => {
                self.joined.push(None);
                self.joined.len() - 1
            }
        };
        // State 1 until the server's `YOU_JOINED` (`0x5077a7`, `0x5078b3`), the id stored
        // (`0x507795`).
        self.joined[i] = Some(ChannelSlot::pending(name, id));
        JoinRecord::Claimed(i as u32 + 1)
    }

    /// The leave's record (`0x509562`-`0x5095ae`) for `name` and the ChannelID its verb matched
    /// (`0x509502`): a zone channel's found by that id, a custom one's by name; its 1-based number.
    pub(crate) fn leave_record(&self, name: &str, id: u32) -> Option<u32> {
        self.record_for(name, id)
    }

    /// The record numbered `n` (1-based).
    pub(crate) fn slot(&self, n: u32) -> Option<&ChannelSlot> {
        self.joined
            .get(usize::try_from(n).ok()?.checked_sub(1)?)?
            .as_ref()
    }

    /// Free the slot holding `name` in place, so no other channel renumbers.
    pub(crate) fn free_slot(&mut self, name: &str) -> Option<u32> {
        let n = self.number_of(name)?;
        self.joined[n as usize - 1] = None;
        Some(n)
    }

    /// Stamp an event's channel args from our slot, as the reference reads them off one record
    /// (`+0x00` number, `+0x04` name, `+0x94` ChannelID, `+0x98` split index): held, arg4 becomes
    /// `"N. Name"`, arg8 the number, arg9 the name with its zone tail and arg7 the ChannelID; not
    /// held, nothing is stamped.
    pub(crate) fn stamp_channel(&self, event: &mut super::event::ChatEvent) {
        // A miss stamps nothing: the four args are one record.
        let Some(n) = self
            .number_of(&event.channel)
            .filter(|_| !event.channel.is_empty())
        else {
            return;
        };
        event.channel_base = event.channel.clone();
        event.zone_channel_id = self.slot(n).map_or(0, |s| s.id);
        event.channel_number = n;
        event.channel = format!("{n}. {}", event.channel_base);
    }
}
