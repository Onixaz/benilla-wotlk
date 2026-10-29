//! The channel pane's engine, the social window's Chat tab (`ChannelFrame.lua`): the display list
//! of category headers over the group sessions and the joined channels (`0x504840`), the selected
//! entry (`0xb743dc`/`0xb743e0`) and its roster (`0xbcfb68`). A channel's roster is the server's
//! display `SMSG_CHANNEL_LIST` (`0x503990`), named by the app off its name cache; a party's or
//! raid's is built from the group roster (`0x5045c0`, `0x504720`). benilla runs no voice chat, so
//! no entry carries voice and no voice session is active.

use std::cmp::Ordering;

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{flag, ftol};
use super::channel::ChannelCommand;
use super::get_text::global_text;
use super::{Model, ScriptValue};

/// The category table (`0xac7a48`): the header tokens, also the entries' category returns.
const CATEGORIES: [&str; 3] = [
    "CHANNEL_CATEGORY_GROUP",
    "CHANNEL_CATEGORY_WORLD",
    "CHANNEL_CATEGORY_CUSTOM",
];

/// `Map.dbc` `InstanceType` 3, a battleground, and 4, an arena: the PvP-raid test (`0x572f76`).
const PVP_INSTANCE_TYPES: [u32; 2] = [3, 4];

/// One joined channel slot as the pane lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelSlotInfo {
    /// The channel's full name (`+0x04`), which the selection and the roster ask carry.
    pub name: String,
    /// A `ChatChannels.dbc` channel's Shortcut (`0x4fdfa5`): it lists under the world header by
    /// that name. `None` for a custom channel.
    pub shortcut: Option<String>,
    /// The slot holds the channel (`+0x9c == 0`, `0x504bb4`); a suspended zone channel lists grey.
    pub active: bool,
}

/// A display entry's type: a channel or a group session; a header's 4 is `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Channel,
    Battleground,
    Party,
    Raid,
}

/// One display entry (`0xb74468`, stride `0x20`).
#[derive(Clone, Debug)]
struct Entry {
    /// `+0x00`: a channel's slot index; -1 the party, -2 the raid, -3 the battleground.
    channel: i32,
    /// `+0x04`: 0..=2 for a header, -1 under one.
    header: i32,
    /// `+0x08`.
    kind: Option<Kind>,
    /// `+0x0c`.
    collapsed: bool,
    /// `+0x10`: `GetChannelDisplayInfo`'s `active`.
    enabled: bool,
    /// `+0x14`: a voice session on the entry, which never exists here.
    voice: bool,
    /// `+0x18 & 1`: a `ChatChannels.dbc` channel, the world category.
    world: bool,
    /// `+0x1c`: a header's children, a group's size, a channel's roster size once listed.
    count: u32,
}

impl Entry {
    fn header(header: i32, collapsed: bool) -> Self {
        Entry {
            channel: 0,
            header,
            kind: None,
            collapsed,
            enabled: false,
            voice: false,
            world: false,
            count: 0,
        }
    }

    fn under(channel: i32, kind: Kind) -> Self {
        Entry {
            channel,
            header: -1,
            kind: Some(kind),
            collapsed: false,
            enabled: true,
            voice: false,
            world: false,
            count: 0,
        }
    }
}

/// One pending-user row.
#[derive(Clone, Debug)]
struct Member {
    guid: u64,
    /// `0x1` owner, `0x2` moderator, `0x4` voiced; `0x10` a group member not silenced
    /// (`0x4fcac0`); `0x20`, `0x40` and `0x80` are voice bits, never set here.
    flags: u8,
    /// The cached name; `None` before the answer and for a negative one, which reads `UNKNOWN`.
    name: Option<String>,
    /// The name query is still out (`0x503bf1`).
    asked: bool,
}

/// A joined slot, with the channel flags its display lists carry (the channel record's `+0x90`).
#[derive(Clone, Debug)]
struct PaneSlot {
    info: ChannelSlotInfo,
    flags: u8,
}

/// The pane's state, the reference's statics around `0xb743cc`.
#[derive(Debug, Default)]
pub(crate) struct ChannelPane {
    slots: Vec<Option<PaneSlot>>,
    /// The display list (`0xb74468`), its length the count (`0xb74460`).
    entries: Vec<Entry>,
    /// The collapsed headers (`0xb743d0`).
    collapsed: [bool; 3],
    /// The selected entry's type and name (`0xb743dc`, `0xb743e0`); `None` is type 4. A group's
    /// name is empty.
    selected: Option<(Kind, String)>,
    /// The selection's flags (`0xb743d4`): the player's own row's flags.
    own_flags: u8,
    /// The pending users (`0xbcfb68`): the selected entry's roster.
    roster: Vec<Member>,
    /// The member count (`0xb743d8`): the name answers the roster waits for.
    unnamed: u32,
    /// The active voice channel (`0xb743cc`): with no voice session, the first entry (`0x504d42`).
    active_voice: usize,
}

/// A case-insensitive UTF-8 compare's equality.
fn names_equal(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

impl ChannelPane {
    /// The entry's display string (`0x4fe1c0`): its kind with a channel's full name or a group's
    /// empty one; `None` for a header or an index past the list.
    fn target(&self, index: i64) -> Option<(Kind, String)> {
        let entry = self.entries.get(usize::try_from(index).ok()?)?;
        match entry.kind? {
            Kind::Channel => {
                let slot = self
                    .slots
                    .get(usize::try_from(entry.channel).ok()?)?
                    .as_ref()?;
                Some((Kind::Channel, slot.info.name.clone()))
            }
            kind => Some((kind, String::new())),
        }
    }

    /// The channel-name compare (`0x4fca90`) against the selection.
    fn is_selected(&self, kind: Kind, name: &str) -> bool {
        self.selected
            .as_ref()
            .is_some_and(|(k, n)| *k == kind && (kind != Kind::Channel || names_equal(n, name)))
    }

    /// The entry lookup (`0x4ff4f0`): the 1-based entry naming `kind` and, for a channel, `name`; 0
    /// for none.
    fn find(&self, kind: Kind, name: &str) -> usize {
        (0..self.entries.len())
            .find(|&i| {
                self.entries[i].header < 0
                    && self.target(i as i64).is_some_and(|(k, n)| {
                        k == kind && (k != Kind::Channel || names_equal(&n, name))
                    })
            })
            .map_or(0, |i| i + 1)
    }

    /// The selection's 1-based entry, 0 for none.
    fn selected_index(&self) -> usize {
        match &self.selected {
            Some((kind, name)) => self.find(*kind, name),
            None => 0,
        }
    }
}

/// The group legs (`0x504874`-`0x5048fa`): a raid in a battleground or an arena lists as the
/// battleground alone, its raid entry shown only by a voice session.
struct Group {
    party: bool,
    raid: bool,
    battleground: bool,
}

fn group(model: &Model) -> Group {
    // The party's first member (`0xbd1948`) and the raid count (`0xbeb608`).
    let party = model.party.members.first().is_some_and(|m| m.guid != 0);
    let raid = !model.party.raid.is_empty();
    if raid
        && model
            .instance_type
            .is_some_and(|t| PVP_INSTANCE_TYPES.contains(&t))
    {
        Group {
            party,
            raid: false,
            battleground: true,
        }
    } else {
        Group {
            party,
            raid,
            battleground: false,
        }
    }
}

/// The list rebuild (`0x504840`) without its signal. Every entry is rebuilt, so a
/// channel's listed roster size lasts only until the next rebuild, as in the reference.
fn rebuild(model: &mut Model) {
    let g = group(model);
    let party_count = model.party.members.len() as u32 + 1;
    let raid_count = model.party.raid.len() as u32;
    let pane = &mut model.channel_pane;
    let collapsed = pane.collapsed;
    let mut entries = vec![Entry {
        count: u32::from(g.party) + u32::from(g.raid) + u32::from(g.battleground),
        ..Entry::header(0, collapsed[0])
    }];
    if !collapsed[0] {
        // `0x504956`-`0x5049c2`: the raid's entry wins over the party's.
        if g.raid {
            entries.push(Entry {
                count: raid_count,
                ..Entry::under(-2, Kind::Raid)
            });
        } else if g.party {
            entries.push(Entry {
                count: party_count,
                ..Entry::under(-1, Kind::Party)
            });
        }
        if g.battleground {
            entries.push(Entry {
                count: raid_count,
                ..Entry::under(-3, Kind::Battleground)
            });
        }
    }
    // The world header lists the `ChatChannels.dbc` slots, the custom header the rest, each in
    // slot order; a collapsed header still counts its channels (`0x504c1b`, `0x504cf2`).
    for (header, &folded) in collapsed.iter().enumerate().skip(1) {
        let at = entries.len();
        entries.push(Entry::header(header as i32, folded));
        for (i, slot) in pane.slots.iter().enumerate() {
            let Some(slot) = slot else {
                continue;
            };
            let zone = slot.info.shortcut.is_some();
            if (header == 1) != zone {
                continue;
            }
            if !folded {
                entries.push(Entry {
                    // `0x504bb4` reads the slot's state; a custom slot is always live (`0x504c81`).
                    enabled: !zone || slot.info.active,
                    world: zone,
                    ..Entry::under(i as i32, Kind::Channel)
                });
            }
            entries[at].count += 1;
        }
    }
    pane.entries = entries;
    pane.active_voice = 0;
    let selected = pane.selected.as_ref().map(|(k, _)| *k);
    // `0x504dd0`: a selected group's roster is rebuilt with the list.
    match selected {
        Some(Kind::Party) => {
            build_party(model);
        }
        Some(Kind::Raid | Kind::Battleground) => {
            build_raid(model);
        }
        _ => {}
    }
}

fn non_empty(name: &str) -> Option<String> {
    (!name.is_empty()).then(|| name.to_string())
}

/// The roster sort (`0x4fb0f0`): unnamed rows first, the rest by name, case-folded.
fn sort_members(roster: &mut [Member]) {
    roster.sort_by(|a, b| match (&a.name, &b.name) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(a), Some(b)) => a.to_lowercase().cmp(&b.to_lowercase()),
    });
}

/// The party roster (`0x5045c0`): the player and the party, the leader as owner (`5`, else `4`),
/// each with the `0x10` of a member not silenced; the count, 0 outside a party.
fn build_party(model: &mut Model) -> u32 {
    let own = model.unit_guids.player;
    if own == 0 {
        return 0;
    }
    model.channel_pane.roster.clear();
    if !model.party.members.first().is_some_and(|m| m.guid != 0) {
        return 0;
    }
    let leader = model.party.leader_guid;
    let flags = |guid: u64| if guid == leader { 0x15 } else { 0x14 };
    let mut roster = vec![Member {
        guid: own,
        flags: flags(own),
        name: non_empty(&model.player_record.name),
        asked: false,
    }];
    roster.extend(
        model
            .party
            .members
            .iter()
            .filter(|m| m.guid != 0)
            .map(|m| Member {
                guid: m.guid,
                flags: flags(m.guid),
                name: non_empty(&m.name),
                asked: false,
            }),
    );
    sort_members(&mut roster);
    let count = roster.len() as u32;
    let pane = &mut model.channel_pane;
    pane.own_flags = flags(own);
    pane.roster = roster;
    count
}

/// The raid roster (`0x504720`), for the raid and the battleground: the leader as owner
/// and moderator (`7`), an assistant as moderator (`6`), others `4`, each with the not-silenced
/// `0x10`; the count, 0 outside a raid.
fn build_raid(model: &mut Model) -> u32 {
    let own = model.unit_guids.player;
    if own == 0 {
        return 0;
    }
    model.channel_pane.roster.clear();
    if model.party.raid.is_empty() {
        return 0;
    }
    let mut own_flags = None;
    let mut roster: Vec<Member> = model
        .party
        .raid
        .iter()
        .map(|m| {
            let flags = 0x10
                | match m.rank {
                    2 => 7,
                    1 => 6,
                    _ => 4,
                };
            if m.guid == own {
                own_flags = Some(flags);
            }
            Member {
                guid: m.guid,
                flags,
                name: non_empty(&m.name),
                asked: false,
            }
        })
        .collect();
    sort_members(&mut roster);
    let count = roster.len() as u32;
    let pane = &mut model.channel_pane;
    if let Some(flags) = own_flags {
        pane.own_flags = flags;
    }
    pane.roster = roster;
    count
}

/// The selection setter: a new selection drops the roster and the own flags and, for a channel,
/// watches it; the current one does nothing. The pending name count stays, as in the reference.
fn select(model: &mut Model, kind: Kind, name: &str) {
    let pane = &mut model.channel_pane;
    if pane.is_selected(kind, name) {
        return;
    }
    pane.selected = Some((kind, name.to_string()));
    pane.roster.clear();
    pane.own_flags = 0;
    if kind == Kind::Channel {
        model.channel_commands.push(ChannelCommand::Watch {
            name: name.to_string(),
        });
    }
}

/// The selection's roster ask: a channel's display list (`0x4fe530`), which answers later, or the
/// group roster built now; the group legs answer its count.
fn fetch(model: &mut Model, kind: Kind, name: &str) -> u32 {
    match kind {
        Kind::Channel => {
            model.channel_commands.push(ChannelCommand::DisplayList {
                name: name.to_string(),
            });
            0
        }
        Kind::Party => build_party(model),
        Kind::Raid | Kind::Battleground => build_raid(model),
    }
}

/// The verbs' display index: `lua_isnumber`, then the truncating `lua_tonumber` less one;
/// `None` for a non-number.
fn index_arg(lua: &Lua, v: Value) -> mlua::Result<Option<i64>> {
    Ok(lua.coerce_number(v)?.map(|n| i64::from(ftol(n)) - 1))
}

fn count_value(count: u32) -> Value {
    if count == 0 {
        Value::Nil
    } else {
        Value::Integer(i64::from(count))
    }
}

impl super::UiScript {
    /// Push the joined slots in slot order, holes kept: the pane relists them and fires
    /// `CHANNEL_UI_UPDATE`, as each join, leave and drop does (`0x505d40`). A selected channel no
    /// longer held live is unselected, as its leave does (`0x507900`).
    pub fn set_channel_slots(&mut self, slots: Vec<Option<ChannelSlotInfo>>) {
        {
            let mut model = self.model_mut();
            let model: &mut Model = &mut model;
            let names: Vec<Option<String>> = slots
                .iter()
                .map(|s| s.as_ref().map(|s| s.name.clone()))
                .collect();
            model.joined_channels = names;
            let old = std::mem::take(&mut model.channel_pane.slots);
            model.channel_pane.slots = slots
                .into_iter()
                .enumerate()
                .map(|(i, s)| {
                    s.map(|info| {
                        // A slot keeps the channel flags its last display list carried.
                        let flags = old
                            .get(i)
                            .and_then(Option::as_ref)
                            .filter(|o| names_equal(&o.info.name, &info.name))
                            .map_or(0, |o| o.flags);
                        PaneSlot { info, flags }
                    })
                })
                .collect();
            let pane = &mut model.channel_pane;
            let dropped = match &pane.selected {
                Some((Kind::Channel, name)) => !pane
                    .slots
                    .iter()
                    .flatten()
                    .any(|s| s.info.active && names_equal(&s.info.name, name)),
                _ => false,
            };
            if dropped {
                pane.selected = None;
                pane.roster.clear();
                pane.own_flags = 0;
            }
            rebuild(model);
        }
        self.fire_event("CHANNEL_UI_UPDATE", Vec::new());
    }

    /// A display `SMSG_CHANNEL_LIST` (`0x503990`), read only while it names the selected channel.
    /// `name_of` answers the name cache for a member: `Some` held (`Some(None)` a negative answer),
    /// `None` asked. The player's own row seats the owner and moderator gates, and a roster with
    /// every name fires `CHANNEL_COUNT_UPDATE` and `CHANNEL_ROSTER_UPDATE`; the events fire after
    /// the parse, in the reference's order.
    pub fn display_channel_list(
        &mut self,
        channel: &str,
        flags: u8,
        members: &[(u64, u8)],
        name_of: impl Fn(u64) -> Option<Option<String>>,
    ) {
        let mut events: Vec<(&str, Vec<ScriptValue>)> = Vec::new();
        {
            let mut model = self.model_mut();
            let model: &mut Model = &mut model;
            let own = model.unit_guids.player;
            let pane = &mut model.channel_pane;
            if !pane.is_selected(Kind::Channel, channel) {
                return;
            }
            pane.unnamed = 0;
            pane.roster.clear();
            pane.own_flags = 0;
            let index = pane.find(Kind::Channel, channel);
            let at = ScriptValue::Int(index as i64);
            if index != 0 {
                let slot = usize::try_from(pane.entries[index - 1].channel)
                    .ok()
                    .and_then(|i| pane.slots.get_mut(i))
                    .and_then(Option::as_mut);
                if let Some(slot) = slot.filter(|s| s.flags != flags) {
                    slot.flags = flags;
                    events.push(("CHANNEL_FLAGS_UPDATED", vec![at.clone()]));
                }
            }
            for &(guid, member_flags) in members {
                if own != 0 && guid == own {
                    pane.own_flags = member_flags;
                    if index != 0 {
                        events.push(("CHANNEL_FLAGS_UPDATED", vec![at.clone()]));
                    }
                }
                let answer = name_of(guid);
                let asked = answer.is_none();
                if asked {
                    pane.unnamed += 1;
                }
                pane.roster.push(Member {
                    guid,
                    flags: member_flags,
                    name: answer.flatten(),
                    asked,
                });
            }
            if pane.unnamed == 0 {
                sort_members(&mut pane.roster);
                if index != 0 {
                    let count = pane.roster.len() as u32;
                    pane.entries[index - 1].count = count;
                    let args = vec![at, ScriptValue::Int(i64::from(count))];
                    events.push(("CHANNEL_COUNT_UPDATE", args.clone()));
                    events.push(("CHANNEL_ROSTER_UPDATE", args));
                }
            }
        }
        for (event, args) in events {
            self.fire_event(event, args);
        }
    }

    /// The guids whose name the pane's roster still waits for.
    pub fn channel_roster_unnamed(&self) -> Vec<u64> {
        let model = self.model_ref();
        let pane = &model.channel_pane;
        if pane.unnamed == 0 {
            return Vec::new();
        }
        pane.roster
            .iter()
            .filter(|m| m.asked)
            .map(|m| m.guid)
            .collect()
    }

    /// The name cache answered for `guid` (`0x4ffe30`), `None` a negative answer; the last awaited
    /// one sorts the roster and fires its count and roster.
    pub fn channel_roster_named(&mut self, guid: u64, name: Option<String>) {
        let mut events: Vec<(&str, Vec<ScriptValue>)> = Vec::new();
        {
            let mut model = self.model_mut();
            let pane = &mut model.channel_pane;
            let mut found = false;
            for m in pane.roster.iter_mut().filter(|m| m.guid == guid) {
                m.name = name.clone();
                m.asked = false;
                found = true;
            }
            if pane.unnamed == 0 || !found {
                return;
            }
            pane.unnamed -= 1;
            if pane.unnamed != 0 {
                return;
            }
            sort_members(&mut pane.roster);
            let index = pane.selected_index();
            if index != 0 {
                let count = pane.roster.len() as u32;
                pane.entries[index - 1].count = count;
                let args = vec![
                    ScriptValue::Int(index as i64),
                    ScriptValue::Int(i64::from(count)),
                ];
                events.push(("CHANNEL_COUNT_UPDATE", args.clone()));
                events.push(("CHANNEL_ROSTER_UPDATE", args));
            }
        }
        for (event, args) in events {
            self.fire_event(event, args);
        }
    }
}

/// Register the channel pane's globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumDisplayChannels() (`0x504e10`): rebuilds the list (`0x504e16`, unsignalled) and
    // answers its length.
    g.set(
        "GetNumDisplayChannels",
        lua.create_function(|lua, _: MultiValue| {
            let mut model = crate::script::model_mut_of(lua);
            let model: &mut Model = &mut model;
            rebuild(model);
            Ok(model.channel_pane.entries.len() as i64)
        })?,
    )?;

    // GetChannelDisplayInfo(index) (`0x4ff580`) → name, header, collapsed, channelNumber, count,
    // active, category, voiceEnabled, voiceActive: nine values, or none past the list. A header
    // answers its category's text, 1 and its collapse; a group its chat type's text and the group
    // category; a channel its Shortcut or name, its slot and its category token.
    g.set(
        "GetChannelDisplayInfo",
        lua.create_function(|lua, index: Value| {
            let Some(index) = index_arg(lua, index)? else {
                return Err(mlua::Error::runtime("Usage: GetChannelDisplayInfo(index)"));
            };
            let (entry, label, active_voice) = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let pane = &model.channel_pane;
                let Some(entry) = usize::try_from(index)
                    .ok()
                    .and_then(|i| pane.entries.get(i))
                    .cloned()
                else {
                    return Ok(MultiValue::new());
                };
                // The channel's short name (`0x4fdf50`): a DBC channel's Shortcut, else its name.
                let label = usize::try_from(entry.channel)
                    .ok()
                    .and_then(|i| pane.slots.get(i))
                    .and_then(Option::as_ref)
                    .map(|s| {
                        s.info
                            .shortcut
                            .clone()
                            .unwrap_or_else(|| s.info.name.clone())
                    });
                (entry, label, pane.active_voice)
            };
            let count = count_value(entry.count);
            if let Ok(header) = usize::try_from(entry.header) {
                let name = global_text(lua, CATEGORIES[header]);
                return Ok(MultiValue::from_vec(vec![
                    Value::String(lua.create_string(name)?),
                    Value::Integer(1),
                    flag(entry.collapsed),
                    Value::Nil,
                    count,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                ]));
            }
            let (name, number, category) = match entry.channel {
                -1 => (
                    global_text(lua, "CHAT_MSG_PARTY"),
                    Value::Nil,
                    CATEGORIES[0],
                ),
                -2 => (global_text(lua, "CHAT_MSG_RAID"), Value::Nil, CATEGORIES[0]),
                -3 => (
                    global_text(lua, "CHAT_MSG_BATTLEGROUND"),
                    Value::Nil,
                    CATEGORIES[0],
                ),
                c if c < 0 => return Ok(MultiValue::from_vec(vec![Value::Nil; 9])),
                c => (
                    label.unwrap_or_default(),
                    Value::Integer(i64::from(c) + 1),
                    if entry.world {
                        CATEGORIES[1]
                    } else {
                        CATEGORIES[2]
                    },
                ),
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(name)?),
                Value::Nil,
                Value::Nil,
                number,
                count,
                flag(entry.enabled),
                Value::String(lua.create_string(category)?),
                flag(entry.voice),
                flag(usize::try_from(index).ok() == Some(active_voice)),
            ]))
        })?,
    )?;

    // GetSelectedDisplayChannel() (`0x4ffa10`): the selection's entry, nil until one is selected
    // or once it leaves the list.
    g.set(
        "GetSelectedDisplayChannel",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(match model.channel_pane.selected_index() {
                0 => Value::Nil,
                i => Value::Integer(i as i64),
            })
        })?,
    )?;

    // SetSelectedDisplayChannel(index) (`0x504e50`): a new channel or group selects and asks its
    // roster; a header, the current selection or a non-number does nothing. Returns nothing.
    g.set(
        "SetSelectedDisplayChannel",
        lua.create_function(|lua, index: Value| {
            let Some(index) = index_arg(lua, index)? else {
                return Ok(());
            };
            let mut model = crate::script::model_mut_of(lua);
            let model: &mut Model = &mut model;
            let Some((kind, name)) = model.channel_pane.target(index) else {
                return Ok(());
            };
            if model.channel_pane.is_selected(kind, &name) {
                return Ok(());
            }
            select(model, kind, &name);
            fetch(model, kind, &name);
            Ok(())
        })?,
    )?;

    // CollapseChannelHeader(index) (`0x5052e0`) and ExpandChannelHeader(index) (`0x505360`): a
    // header entry's flag, then the signalled rebuild, `CHANNEL_UI_UPDATE` (`0x504dfe`).
    for (verb, collapse) in [
        ("CollapseChannelHeader", true),
        ("ExpandChannelHeader", false),
    ] {
        g.set(
            verb,
            lua.create_function(move |lua, index: Value| {
                let Some(index) = index_arg(lua, index)? else {
                    return Ok(());
                };
                {
                    let mut model = crate::script::model_mut_of(lua);
                    let model: &mut Model = &mut model;
                    let header = usize::try_from(index)
                        .ok()
                        .and_then(|i| model.channel_pane.entries.get(i))
                        .and_then(|e| usize::try_from(e.header).ok())
                        .filter(|h| *h < 3);
                    let Some(header) = header else {
                        return Ok(());
                    };
                    model.channel_pane.collapsed[header] = collapse;
                    rebuild(model);
                }
                super::tick::fire_event_into(lua, "CHANNEL_UI_UPDATE", Vec::new());
                Ok(())
            })?,
        )?;
    }

    // GetNumChannelMembers(index) (`0x505190`): a new selection selects and asks, answering nil
    // for a channel and the built size for a group; the current one answers its roster's size
    // once every name is in. nil for none, a header or a non-number.
    g.set(
        "GetNumChannelMembers",
        lua.create_function(|lua, index: Value| {
            let Some(index) = index_arg(lua, index)? else {
                return Ok(Value::Nil);
            };
            let mut model = crate::script::model_mut_of(lua);
            let model: &mut Model = &mut model;
            let Some((kind, name)) = model.channel_pane.target(index) else {
                return Ok(Value::Nil);
            };
            let count = if !model.channel_pane.is_selected(kind, &name) {
                select(model, kind, &name);
                fetch(model, kind, &name)
            } else if model.channel_pane.unnamed != 0 {
                0
            } else {
                model.channel_pane.roster.len() as u32
            };
            Ok(count_value(count))
        })?,
    )?;

    // GetChannelRosterInfo(index, member) (`0x504f30`) → name, owner, moderator, muted, active,
    // voiceEnabled, from the row's flags `0x1`, `0x2`, `0x20`, `0x40`, `0x80`. An entry not
    // selected selects and asks, answering nothing, as do a pending roster, a row past it and a
    // non-number. A name the cache lacks reads `UNKNOWN` (`0x50505a`).
    g.set(
        "GetChannelRosterInfo",
        lua.create_function(|lua, (index, member): (Value, Value)| {
            let Some(index) = index_arg(lua, index)? else {
                return Ok(MultiValue::new());
            };
            let Some(member) = index_arg(lua, member)? else {
                return Ok(MultiValue::new());
            };
            let (name, flags) = {
                let mut model = crate::script::model_mut_of(lua);
                let model: &mut Model = &mut model;
                let Some((kind, target)) = model.channel_pane.target(index) else {
                    return Ok(MultiValue::new());
                };
                if !model.channel_pane.is_selected(kind, &target) {
                    select(model, kind, &target);
                    fetch(model, kind, &target);
                    return Ok(MultiValue::new());
                }
                if model.channel_pane.unnamed != 0 {
                    return Ok(MultiValue::new());
                }
                let Some(row) = usize::try_from(member)
                    .ok()
                    .and_then(|i| model.channel_pane.roster.get(i))
                else {
                    return Ok(MultiValue::new());
                };
                (row.name.clone(), row.flags)
            };
            let name = name.unwrap_or_else(|| global_text(lua, "UNKNOWN"));
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(name)?),
                flag(flags & 0x1 != 0),
                flag(flags & 0x2 != 0),
                flag(flags & 0x20 != 0),
                flag(flags & 0x40 != 0),
                flag(flags & 0x80 != 0),
            ]))
        })?,
    )?;

    // IsDisplayChannelOwner() (`0x4ffa70`) and IsDisplayChannelModerator() (`0x4ffae0`): the
    // selection is listed and the player's own row carries `0x1`, or `0x1`/`0x2`.
    for (verb, mask) in [
        ("IsDisplayChannelOwner", 0x1u8),
        ("IsDisplayChannelModerator", 0x3),
    ] {
        g.set(
            verb,
            lua.create_function(move |lua, _: MultiValue| {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let pane = &model.channel_pane;
                let index = pane.selected_index();
                Ok(flag(
                    index != 0 && pane.entries[index - 1].header < 0 && pane.own_flags & mask != 0,
                ))
            })?,
        )?;
    }

    // SetActiveVoiceChannel(index) (`0x500ae0`): a non-zero index opens the entry's voice session,
    // which none carries here; 0 leaves the active one, firing `CHANNEL_VOICE_UPDATE` with its
    // 1-based entry (and its voice flag when set) and resetting it to the first entry.
    g.set(
        "SetActiveVoiceChannel",
        lua.create_function(|lua, index: Value| {
            let Some(n) = lua.coerce_number(index)? else {
                return Ok(());
            };
            if ftol(n) != 0 {
                return Ok(());
            }
            let (active, voice) = {
                let mut model = crate::script::model_mut_of(lua);
                let pane = &mut model.channel_pane;
                let active = pane.active_voice;
                let voice = pane.entries.get(active).is_some_and(|e| e.voice);
                pane.active_voice = 0;
                (active, voice)
            };
            let mut args = vec![ScriptValue::Int(active as i64 + 1)];
            if voice {
                args.push(ScriptValue::Int(1));
            }
            super::tick::fire_event_into(lua, "CHANNEL_VOICE_UPDATE", args);
            Ok(())
        })?,
    )?;

    Ok(())
}
