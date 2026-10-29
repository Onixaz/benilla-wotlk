//! The social feed and drain: the snapshot the FriendsFrame reads, the list events, the result
//! lines, and the sends for the Lua-side [`SocialRequest`] intents. The reference resolves names,
//! races, classes and zones engine-side too, before Lua sees a row (`GetWhoInfo` `0x6b4a80`).

use benilla_formats::{AreaTableCatalog, ChrClassNames, CreateRaceRow};
use benilla_protocol::messages::{contact_flag, friend_status, WhoEntry};
use benilla_ui::script::{
    FriendInfo, SocialRequest, SocialState as VmSocial, UiScript, WhoInfo, WhoRecords,
};
use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::chr_classes::ChrClassTable;
use crate::entities::CharCreate;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_action::{MessageSink, Shown, UiError};

use super::{name_pushes, result_key, row_guid, status_flag_key, Friend, SocialState};

/// The chat-routed `/who` templates (`0x6b5030`, the count at `0x6b8912`). FrameXML never names
/// them: the engine prints these lines itself. None is a message-catalog row, so they go out
/// through `Shown::unkeyed`, since `Shown::keyed` turns an unknown key's line red.
const WHO_KEYS: [&str; 3] = [
    "WHO_LIST_FORMAT",
    "WHO_LIST_GUILD_FORMAT",
    "WHO_NUM_RESULTS",
];

/// The most rows that print to chat when the Who frame does not claim an answer. `SMSG_WHO`
/// (`0x6b8766`-`0x6b8795`) prints the rows and the count with no event when the wire count is at
/// most a limit CVar's value, else fires `WHO_LIST_UPDATE` (`0x6b8960`); with `SetWhoToUI` set the
/// event always fires. Nothing in 12340 sets that CVar's pointer (`0xc7bfe8`), so its fallback 3
/// (`0x6b8788`) is the limit. It compares the raw wire count, not the 50-capped copy.
const WHO_CHAT_MAX: u32 = 3;

/// The tables a `/who` row resolves its ids through, and `UNKNOWN`'s text for an id with no row.
#[derive(Default)]
struct WhoNames<'a> {
    areas: Option<&'a AreaTableCatalog>,
    races: &'a [CreateRaceRow],
    classes: &'a [ChrClassNames],
    unknown: String,
}

/// What the feed last announced, so the list events fire on edges.
#[derive(Default)]
pub(crate) struct FedSocial {
    /// Whether the VM has had a snapshot; the first push fires the list events regardless.
    seeded: bool,
    /// The name cache's generation: an ask through `ResMut` marks the cache every frame.
    names: crate::ui_script::gate::Watch,
}

/// Push the display snapshot to the VM, fire the list events, and print the owed result lines.
pub(crate) fn feed_social(
    script: Option<NonSendMut<UiScript>>,
    mut social: ResMut<SocialState>,
    names: Res<NameCache>,
    areas: Option<Res<AreaTableRes>>,
    (classes, races): (Option<Res<ChrClassTable>>, Option<Res<CharCreate>>),
    commands: Res<NetCommands>,
    mut sink: MessageSink,
    mut fed: Local<crate::ui_script::VmMemo<FedSocial>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let fed = fed.get(&script);
    // The snapshot reads these alone: the store (written by the wire and the VM's drains), the
    // names and the three tables.
    let names_moved = fed.names.moved(names.generation());
    let inputs = [
        ("first", !fed.seeded),
        ("social", social.is_changed()),
        ("names", names_moved),
        ("areas", areas.as_ref().is_some_and(|a| a.is_changed())),
        ("classes", classes.as_ref().is_some_and(|c| c.is_changed())),
        ("races", races.as_ref().is_some_and(|r| r.is_changed())),
    ];
    crate::ui_script::gate::trace("feed_social", &inputs);
    if crate::ui_script::gate::Gate::new(inputs.iter().any(|&(_, open)| open)).skip() {
        return;
    }
    let areas = areas.as_deref().map(|a| &a.0);
    let who_names = WhoNames {
        areas,
        races: races.as_deref().map_or(&[], |r| r.0.race_rows()),
        classes: classes.as_deref().map_or(&[], |c| c.0.names()),
        unknown: script.engine_text("UNKNOWN"),
    };

    // Everything that reads the VM's own string table, resolved under one borrow of it.
    let (owed, (friends, display_order, friends_named)) = {
        let get = |key: &str| script.lua().globals().get::<String>(key).ok();
        // Keyed, so the surface and the sound come from the catalog row (`ERR_FRIEND_ONLINE_SS`
        // plays `FRIENDJOINGAME`).
        let owed: Vec<Shown> = drain_result_lines(&mut social, &names, &commands)
            .iter()
            .filter_map(|e| {
                crate::ui_action::ui_error_text(e, &get).map(|text| Shown::keyed(e.key, text))
            })
            .collect();
        let away = |connected: bool, status: u8| {
            status_flag_key(connected, status)
                .and_then(&get)
                .unwrap_or_default()
        };
        let friends = friend_rows(
            &social,
            &names,
            &commands,
            (areas, who_names.classes),
            &away,
        );
        (owed, friends)
    };

    // The owed lines first, so a friend's offline line lands before the update that clears their
    // zone.
    crate::ui_action::show_messages(&mut script, &mut sink, "ui_social", owed);

    let (ignores, ignore_order, ignores_named) = list_rows(&social.ignores, &names, &commands);
    let (mutes, mute_order, mutes_named) = list_rows(&social.mutes, &names, &commands);
    let who = who_rows(&social, &who_names);

    let selected_friend = index_of(&display_order, social.selected_friend);
    let selected_ignore = index_of(&ignore_order, social.selected_ignore);
    let selected_mute = index_of(&mute_order, social.selected_mute);
    social.display_order = display_order;
    social.ignore_display_order = ignore_order;
    social.mute_display_order = mute_order;

    script.set_social(VmSocial {
        friends,
        selected_friend,
        ignores,
        selected_ignore,
        mutes,
        selected_mute,
        who,
        who_total: social.who_total,
        // The chain rides along: `SortWho` re-sorts inside the binding.
        who_sort: social.who_sort.clone(),
    });

    // A changed list announces itself once every name on it has resolved, as the packet read
    // (`0x6b7dd0`) holds each event until its pending-name count drains (`0x6b3d90`).
    let first = !fed.seeded;
    fed.seeded = true;
    let lists = &mut *social;
    for (dirty, named, event) in [
        (&mut lists.friends_dirty, friends_named, "FRIENDLIST_UPDATE"),
        (&mut lists.ignores_dirty, ignores_named, "IGNORELIST_UPDATE"),
        (&mut lists.mutes_dirty, mutes_named, "MUTELIST_UPDATE"),
    ] {
        if first || (*dirty && named) {
            *dirty = false;
            script.fire_event(event, Vec::new());
        }
    }
    // `SMSG_WHO`'s two exits ([`WHO_CHAT_MAX`]); a sort fires its own event inside `SortWho`.
    if social.who_dirty {
        social.who_dirty = false;
        if answer_goes_to_the_frame(social.who_to_ui, social.who_sent) {
            script.fire_event("WHO_LIST_UPDATE", Vec::new());
        } else {
            // In wire order: each line is composed inside the parse loop (`0x6b887a`), before the
            // `qsort` at `0x6b88fe` orders the array `GetWhoInfo` reads.
            let printed = {
                let wire_order: Vec<WhoInfo> =
                    social.who.iter().map(|e| who_row(e, &who_names)).collect();
                who_lines(&wire_order, social.who_sent, &|key| script.engine_text(key))
            };
            crate::ui_action::show_messages(&mut script, &mut sink, "ui_social", printed);
        }
    }
}

/// Whether an `SMSG_WHO` answer fires `WHO_LIST_UPDATE` (true) or prints chat lines (false).
fn answer_goes_to_the_frame(to_ui: bool, sent: u32) -> bool {
    to_ui || sent > WHO_CHAT_MAX
}

/// Name every result line whose subject has resolved: a line that needs a name waits for the name
/// query, as the reference resolves before it composes.
fn drain_result_lines(
    social: &mut SocialState,
    names: &NameCache,
    commands: &NetCommands,
) -> Vec<UiError> {
    let mut still_pending = Vec::new();
    let mut ready = Vec::new();
    for update in std::mem::take(&mut social.pending_lines) {
        let Some(key) = result_key(update.result) else {
            continue; // an unknown code shows nothing
        };
        let pushes = name_pushes(key);
        if pushes == 0 {
            ready.push(UiError::key(key));
            continue;
        }
        // A named line with guid 0 (the server's answer to a failed lookup) can never resolve.
        if update.guid == 0 {
            continue;
        }
        match names.resolve(update.guid, commands).map(str::to_string) {
            // The name, once per `%s` the key's arity names (twice for the player link).
            Some(name) => ready.push(UiError::strings(key, &vec![name.as_str(); pushes])),
            None => still_pending.push(update),
        }
    }
    social.pending_lines = still_pending;
    ready
}

/// The chat-routed `/who` output: one line per row in the order given, then the count of rows the
/// wire carried (`[ebp-0x4]` at `0x6b890b`), not the server's total. Each template is the engine's
/// `GetText` (`0x819d40`), so the count line takes `WHO_NUM_RESULTS` and its `|4` grammar.
fn who_lines(rows: &[WhoInfo], sent: u32, text: &dyn Fn(&str) -> String) -> Vec<Shown> {
    use benilla_ui::strings::{fill, Arg};

    let chat = |text: String| {
        (!text.is_empty()).then(|| Shown::unkeyed(benilla_ui::messages::MsgKind::Chat, text))
    };
    let mut lines = Vec::with_capacity(rows.len() + 1);
    for row in rows {
        let key = if row.guild.is_empty() {
            WHO_KEYS[0]
        } else {
            WHO_KEYS[1]
        };
        let template = text(key);
        // The templates fill in order, `%s` and `%d` mixed: name, name, level, race, class,
        // [guild,] zone.
        let mut args = vec![
            Arg::S(&row.name),
            Arg::S(&row.name),
            Arg::D(i64::from(row.level)),
            Arg::S(&row.race),
            Arg::S(&row.class),
        ];
        if !row.guild.is_empty() {
            args.push(Arg::S(&row.guild));
        }
        args.push(Arg::S(&row.zone));
        lines.extend(chat(fill(&template, &args)));
    }
    lines.extend(chat(fill(&text(WHO_KEYS[2]), &[Arg::D(i64::from(sent))])));
    lines
}

/// The friend rows in the reference's sort order (`0x6b3960`): online first, then by name with
/// unnamed rows last; their guids in the same order for the drain; and whether every name resolved.
fn friend_rows(
    social: &SocialState,
    names: &NameCache,
    commands: &NetCommands,
    (areas, classes): (Option<&AreaTableCatalog>, &[ChrClassNames]),
    away: &dyn Fn(bool, u8) -> String,
) -> (Vec<FriendInfo>, Vec<u64>, bool) {
    let mut rows: Vec<(u64, FriendInfo)> = social
        .friends
        .iter()
        .map(|friend| {
            let name = names.resolve(friend.guid, commands).map(str::to_string);
            (
                friend.guid,
                FriendInfo {
                    guid: friend.guid,
                    class: friend_class(friend, names, classes),
                    area: areas.and_then(|a| friend_area(a, friend.area)),
                    name,
                    level: friend.level,
                    connected: friend.connected,
                    status: away(friend.connected, friend.status),
                    note: (!friend.note.is_empty()).then(|| friend.note.clone()),
                    raf: friend.status & friend_status::RAF != 0,
                },
            )
        })
        .collect();
    let named = rows.iter().all(|(_, row)| row.name.is_some());

    rows.sort_by(|(_, a), (_, b)| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| match (&a.name, &b.name) {
                (Some(a), Some(b)) => a.to_lowercase().cmp(&b.to_lowercase()),
                (a, b) => b.is_some().cmp(&a.is_some()),
            })
    });
    let (rows, order) = rows.into_iter().map(|(guid, row)| (row, guid)).unzip();
    (rows, order, named)
}

/// `GetFriendInfo`'s class (`0x6b4130`): with a `ChrClasses` row for the friend's class, the
/// name-cache record's class in its gender, else the row's name; no row reads `UNKNOWN`.
fn friend_class(friend: &Friend, names: &NameCache, classes: &[ChrClassNames]) -> Option<String> {
    let row = classes.iter().find(|c| c.id == friend.class)?;
    let shown = names
        .player_traits(friend.guid)
        .and_then(|(_, class, gender)| {
            let cached = classes.iter().find(|c| c.id == u32::from(class))?;
            Some(gendered(&cached.name, &cached.female, &cached.male, gender))
        })
        .unwrap_or(&row.name);
    Some(shown.to_string())
}

/// `GetFriendInfo`'s zone: the area's name, or its parent zone's for a subzone, one hop.
fn friend_area(areas: &AreaTableCatalog, area: u32) -> Option<String> {
    let row = areas.get(area)?;
    let shown = match row.zone_id {
        0 => row,
        parent => areas.get(parent).unwrap_or(row),
    };
    Some(shown.name.clone())
}

/// An ignore or mute list's names in the reference's sort order (`0x6b3a60`): resolved names first,
/// then by byte order (a case-sensitive compare); unresolved names are empty. Also the guids in
/// that order and whether every name resolved.
fn list_rows(
    guids: &[u64],
    names: &NameCache,
    commands: &NetCommands,
) -> (Vec<String>, Vec<u64>, bool) {
    let mut rows: Vec<(u64, Option<String>)> = guids
        .iter()
        .map(|guid| (*guid, names.resolve(*guid, commands).map(str::to_string)))
        .collect();
    let named = rows.iter().all(|(_, name)| name.is_some());
    rows.sort_by(|(_, a), (_, b)| match (a, b) {
        (Some(a), Some(b)) => a.cmp(b),
        (a, b) => b.is_some().cmp(&a.is_some()),
    });
    let (rows, order) = rows
        .into_iter()
        .map(|(guid, name)| (name.unwrap_or_default(), guid))
        .unzip();
    (rows, order, named)
}

/// The `/who` rows, resolved, then sorted by the chain as the reference sorts every answer
/// (`0x6b88fe`); the comparator compares record names, so the sort runs after [`who_row`].
fn who_rows(social: &SocialState, names: &WhoNames) -> Vec<WhoInfo> {
    let mut rows: Vec<WhoInfo> = social.who.iter().map(|e| who_row(e, names)).collect();
    social.who_sort.sort(&mut rows);
    rows
}

/// The race and class display names (`0x715970`, `0x7159e0`): the gender's own column, else the
/// other gender's, else `Name_lang`; a gender past 1 takes `Name_lang`.
fn gendered<'a>(name: &'a str, female: &'a str, male: &'a str, gender: u8) -> &'a str {
    let order = match gender {
        0 => [male, female],
        1 => [female, male],
        _ => return name,
    };
    order.into_iter().find(|s| !s.is_empty()).unwrap_or(name)
}

/// One `/who` row, ids resolved as `GetWhoInfo` does (`0x6b4a80`): race and class in the row's
/// gender, `UNKNOWN` for an id with no row; the wire carries class before race, the Lua API race
/// first.
fn who_row(entry: &WhoEntry, names: &WhoNames) -> WhoInfo {
    let race = names.races.iter().find(|r| u32::from(r.id) == entry.race);
    let class = names.classes.iter().find(|c| c.id == entry.class);
    let zone = names.areas.and_then(|a| a.name(entry.zone));
    let shown = |s: Option<&str>| s.unwrap_or(&names.unknown).to_string();
    WhoInfo {
        name: entry.name.clone(),
        guild: entry.guild.clone(),
        level: entry.level,
        race: shown(race.map(|r| gendered(&r.name, &r.name_female, &r.name_male, entry.gender))),
        class: shown(class.map(|c| gendered(&c.name, &c.female, &c.male, entry.gender))),
        zone: shown(zone),
        class_file: class.map(|c| c.file.clone()),
        records: WhoRecords {
            race: race.map(|r| r.name.clone()),
            class: class.map(|c| c.name.clone()),
            zone: zone.map(str::to_string),
        },
    }
}

/// The 1-based row of a guid in the shown order, 0 when absent, as `GetSelectedFriend` converts it.
fn index_of(order: &[u64], guid: u64) -> u32 {
    if guid == 0 {
        return 0;
    }
    order
        .iter()
        .position(|g| *g == guid)
        .map_or(0, |i| i as u32 + 1)
}

/// Turn the Lua API's social intents into their sends. A row resolves through the order the feed
/// published, since the wire removes by guid.
pub(super) fn drain_social(
    script: Option<NonSendMut<UiScript>>,
    mut social: ResMut<SocialState>,
    commands: Res<NetCommands>,
    areas: Option<Res<AreaTableRes>>,
    mut tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let requests = script.take_social_requests();
    if requests.is_empty() {
        return;
    }
    let send = |command| {
        let _ = commands.0.send(command);
    };
    for request in requests {
        match request {
            SocialRequest::RefreshFriends => send(ClientCommand::ContactList {
                mask: contact_flag::FRIEND,
            }),
            SocialRequest::AddFriend { name, note } => {
                // `0x6b540c`: the Friends tutorial is flagged just before the send.
                if let Some(t) = tutorials.as_mut() {
                    t.write(crate::tutorial::TutorialEvent::Acknowledge {
                        id: crate::tutorial::id::FRIENDS,
                    });
                }
                send(ClientCommand::AddFriend { name, note });
            }
            SocialRequest::RemoveFriend(row) => {
                if let Some(guid) = social.friend_at_row(row) {
                    send(ClientCommand::DelFriend { guid });
                }
            }
            SocialRequest::SetFriendNotes { row, note } => {
                if let Some(guid) = social.friend_at_row(row) {
                    social.set_note(guid, &note);
                    send(ClientCommand::SetContactNotes { guid, note });
                }
            }
            SocialRequest::AddIgnore(name) => send(ClientCommand::AddIgnore { name }),
            SocialRequest::DelIgnore(row) => {
                if let Some(guid) = row_guid(&social.ignore_display_order, row) {
                    send(ClientCommand::DelIgnore { guid });
                }
            }
            SocialRequest::AddMute(name) => send(ClientCommand::AddMute { name }),
            SocialRequest::DelMute(row) => {
                if let Some(guid) = row_guid(&social.mute_display_order, row) {
                    send(ClientCommand::DelMute { guid });
                }
            }
            SocialRequest::SelectFriend(row) => {
                social.selected_friend = social.friend_at_row(row).unwrap_or(0);
            }
            SocialRequest::SelectIgnore(row) => {
                social.selected_ignore = row_guid(&social.ignore_display_order, row).unwrap_or(0);
            }
            SocialRequest::SelectMute(row) => {
                social.selected_mute = row_guid(&social.mute_display_order, row).unwrap_or(0);
            }
            SocialRequest::Who(filter) => {
                let request = super::who_query(&filter, areas.as_deref().map(|a| &a.0));
                send(ClientCommand::Who {
                    request: Box::new(request),
                });
            }
            // Mirror the click the binding applied to the VM's chain. No `who_dirty`: `SortWho`
            // already fired `WHO_LIST_UPDATE`, once per click as the reference does.
            SocialRequest::SortWho(sort_type) => social.who_sort.promote(&sort_type),
            SocialRequest::SetWhoToUi(on) => social.who_to_ui = on,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::WhoResults;

    /// The snapshot rebuilds only when an input moves: a list edit that marks nothing is not read
    /// (the gate holds), and the same edit marked is.
    #[test]
    fn a_quiet_frame_rebuilds_no_social_snapshot() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_non_send_resource(UiScript::new().unwrap())
            .insert_resource(NetCommands(tx))
            .init_resource::<SocialState>()
            .init_resource::<NameCache>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .add_systems(Update, feed_social);
        let ignores = |app: &App| {
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<i64>("return GetNumIgnores()")
                .unwrap()
        };
        app.update();
        assert_eq!(ignores(&app), 0);
        app.world_mut()
            .resource_mut::<SocialState>()
            .bypass_change_detection()
            .ignores
            .push(7);
        app.update();
        assert_eq!(ignores(&app), 0, "an unmarked edit is not read");
        app.world_mut().resource_mut::<SocialState>().set_changed();
        app.update();
        assert_eq!(ignores(&app), 1, "a marked edit is");
    }

    fn entry(name: &str, level: u32, guild: &str) -> WhoEntry {
        WhoEntry {
            name: name.to_string(),
            guild: guild.to_string(),
            level,
            class: 1,
            race: 1,
            gender: 0,
            zone: 12,
        }
    }

    fn no_names() -> WhoNames<'static> {
        WhoNames {
            unknown: "UNKNOWN".into(),
            ..Default::default()
        }
    }

    fn names(rows: &[WhoInfo]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    /// The reference sorts inside the parse (`0x6b88fe`), so a list sorted by level stays sorted
    /// when the next `/who` lands.
    #[test]
    fn the_answer_is_presented_in_the_sort_chains_order() {
        let mut social = SocialState::default();
        social.apply_who(WhoResults {
            displayed: 3,
            total: 3,
            entries: vec![
                entry("Galas", 60, ""),
                entry("erdrin", 12, ""),
                entry("Bruk", 60, ""),
            ],
        });

        // The seeded chain is zone → level → class → group → name → …: one zone and one class
        // here, so it resolves on level, then name.
        assert_eq!(
            names(&who_rows(&social, &no_names())),
            ["erdrin", "Bruk", "Galas"],
            "wire order was Galas, erdrin, Bruk"
        );

        // A Name click: ascending and case-folded, so `erdrin` sorts among the capitals.
        social.who_sort.promote("name");
        assert_eq!(
            names(&who_rows(&social, &no_names())),
            ["Bruk", "erdrin", "Galas"]
        );

        // The same click again reverses.
        social.who_sort.promote("name");
        assert_eq!(
            names(&who_rows(&social, &no_names())),
            ["Galas", "erdrin", "Bruk"]
        );

        // A fresh answer arrives into that same chain.
        social.apply_who(WhoResults {
            displayed: 2,
            total: 2,
            entries: vec![entry("Aaa", 1, ""), entry("Zzz", 1, "")],
        });
        assert_eq!(names(&who_rows(&social, &no_names())), ["Zzz", "Aaa"]);
    }

    #[test]
    fn a_small_answer_goes_to_chat_and_a_large_one_to_the_frame() {
        // The wire count routes: 57 rows sent but 50 kept still go to the frame.
        assert!(answer_goes_to_the_frame(false, 57));
        assert!(!answer_goes_to_the_frame(false, 0));
        assert!(!answer_goes_to_the_frame(false, 3), "three still print");
        assert!(answer_goes_to_the_frame(false, 4), "four go to the frame");
        // With the frame open, every answer is the frame's.
        for shown in 0..=4 {
            assert!(answer_goes_to_the_frame(true, shown));
        }
    }

    #[test]
    fn the_chat_lines_come_out_in_wire_order_with_the_count_last() {
        let mut social = SocialState::default();
        social.apply_who(WhoResults {
            displayed: 2,
            total: 9,
            entries: vec![entry("Zzz", 60, ""), entry("Aaa", 12, "")],
        });
        // A chain that would order them the other way round, to prove it is not consulted.
        social.who_sort.promote("name");
        assert_eq!(
            names(&who_rows(&social, &no_names())),
            ["Aaa", "Zzz"],
            "the frame's order"
        );

        let wire: Vec<WhoInfo> = social.who.iter().map(|e| who_row(e, &no_names())).collect();
        // The shipped templates: FrameXML never names these keys, so a stub would echo a guess.
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let vm = benilla_ui::script::UiScript::new().expect("VM");
        vm.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let lines: Vec<String> = who_lines(&wire, social.who_sent, &|key| vm.engine_text(key))
            .iter()
            .map(|s| s.text().to_string())
            .collect();

        assert_eq!(lines.len(), 3, "one per row plus the count: {lines:?}");
        assert!(
            lines[0].contains("Zzz"),
            "wire order, not the chain's: {lines:?}"
        );
        assert!(lines[1].contains("Aaa"), "{lines:?}");
        // 3.3.5a has no `_P1` twin: the engine's `GetText` falls back to the bare token, and the
        // count is the wire's 2, not the total 9.
        assert!(vm.global_text("WHO_NUM_RESULTS_P1").is_empty());
        let count = benilla_ui::strings::fill(
            &vm.global_text("WHO_NUM_RESULTS"),
            &[benilla_ui::strings::Arg::D(2)],
        );
        assert!(!count.is_empty());
        assert_eq!(lines[2], count, "the count comes last");
    }

    /// An id with no row shows `UNKNOWN` (`0x6b4b56`), has no class file name (`0x6b4c2b`), and
    /// carries no record name for the comparator.
    #[test]
    fn an_unresolvable_id_shows_unknown_and_has_no_record() {
        let row = who_row(
            &WhoEntry {
                name: "Nobody".into(),
                level: 1,
                class: 200,
                race: 200,
                zone: 999_999,
                ..Default::default()
            },
            &no_names(),
        );
        assert_eq!(
            (row.class.as_str(), row.race.as_str(), row.zone.as_str()),
            ("UNKNOWN", "UNKNOWN", "UNKNOWN")
        );
        assert_eq!(row.class_file, None);
        assert_eq!(row.records, WhoRecords::default());
    }

    /// A female row takes the female column, falling back to the male one, then `Name_lang`.
    #[test]
    fn race_and_class_follow_the_rows_gender() {
        let races = [CreateRaceRow {
            id: 10,
            name: "Neutral".into(),
            name_female: "She".into(),
            name_male: "He".into(),
            ..Default::default()
        }];
        let classes = [ChrClassNames {
            id: 6,
            file: "DEATHKNIGHT".into(),
            name: "Plain".into(),
            male: "Only male".into(),
            ..Default::default()
        }];
        let names = WhoNames {
            races: &races,
            classes: &classes,
            ..no_names()
        };
        let row = |gender| {
            who_row(
                &WhoEntry {
                    class: 6,
                    race: 10,
                    gender,
                    ..Default::default()
                },
                &names,
            )
        };
        let female = row(1);
        assert_eq!(
            (female.race.as_str(), female.class.as_str()),
            ("She", "Only male")
        );
        assert_eq!(female.class_file.as_deref(), Some("DEATHKNIGHT"));
        assert_eq!(female.records.race.as_deref(), Some("Neutral"));
        assert_eq!(female.records.class.as_deref(), Some("Plain"));
        let male = row(0);
        assert_eq!(
            (male.race.as_str(), male.class.as_str()),
            ("He", "Only male")
        );
        let other = row(2);
        assert_eq!(
            (other.race.as_str(), other.class.as_str()),
            ("Neutral", "Plain")
        );
    }

    #[test]
    fn row_indices_map_through_the_shown_order() {
        let order = [11u64, 22, 33];
        assert_eq!(row_guid(&order, 1), Some(11));
        assert_eq!(row_guid(&order, 3), Some(33));
        assert_eq!(row_guid(&order, 0), None);
        assert_eq!(row_guid(&order, 4), None);
    }

    #[test]
    fn selection_follows_the_player_not_the_row() {
        assert_eq!(index_of(&[11, 22, 33], 22), 2);
        assert_eq!(index_of(&[22, 11, 33], 22), 1, "same player, new row");
        assert_eq!(index_of(&[11, 33], 22), 0, "no longer listed");
        assert_eq!(index_of(&[11, 22], 0), 0, "nothing selected");
    }

    /// Against the shipped strings: the guilded template takes a seventh fill.
    #[test]
    fn who_lines_fill_both_specifiers_in_wire_order() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let vm = benilla_ui::script::UiScript::new().expect("VM");
        vm.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let get = |key: &str| vm.engine_text(key);

        let row =
            |name: &str, level: u32, guild: &str, race: &str, class: &str, zone: &str| WhoInfo {
                name: name.into(),
                level,
                guild: guild.into(),
                race: race.into(),
                class: class.into(),
                zone: zone.into(),
                ..Default::default()
            };
        let lines: Vec<String> = who_lines(
            &[
                row("Tigole", 40, "Legacy", "Human", "Rogue", "Westfall"),
                row("Solo", 5, "", "Dwarf", "Priest", "Coldridge Valley"),
            ],
            2,
            &get,
        )
        .iter()
        .map(|s| s.text().to_string())
        .collect();

        assert_eq!(
            lines[0],
            "|Hplayer:Tigole|h[Tigole]|h: Level 40 Human Rogue <Legacy> - Westfall"
        );
        assert_eq!(
            lines[1], "|Hplayer:Solo|h[Solo]|h: Level 5 Dwarf Priest - Coldridge Valley",
            "the unguilded template skips the guild fill"
        );
    }

    #[test]
    fn the_away_tag_and_zone_follow_get_friend_info() {
        let areas = AreaTableCatalog::from_rows(
            [
                (1519, "Stormwind City", 0),
                (5148, "Trade District", 1519),
                (77, "Orphan", 9),
            ]
            .into_iter()
            .map(|(id, name, zone_id)| {
                (
                    id,
                    benilla_formats::AreaTableRow {
                        map_id: 0,
                        zone_id,
                        explore_flag: 0,
                        flags: 0,
                        faction_group_mask: 0,
                        exploration_level: 0,
                        name: name.to_string(),
                    },
                )
            })
            .collect(),
        );
        assert_eq!(friend_area(&areas, 5148).as_deref(), Some("Stormwind City"));
        assert_eq!(friend_area(&areas, 1519).as_deref(), Some("Stormwind City"));
        assert_eq!(
            friend_area(&areas, 77).as_deref(),
            Some("Orphan"),
            "a missing parent"
        );
        assert_eq!(friend_area(&areas, 0), None, "no row reads UNKNOWN");
    }
}
