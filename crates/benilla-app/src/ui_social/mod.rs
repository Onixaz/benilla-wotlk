//! The social session: the friend, ignore and mute lists, `/who`, and the system lines they
//! print. The reference's `FriendList` (`0xc28168`) holds guids, not names: a row's name comes from
//! the name cache when it is read, and each list's selection is a guid too.
//!
//! Every friend, ignore and mute result prints to the chat frame through `DisplayError`, the line
//! `HandleStatus` (`0x6b73a0`) picks per result code.

use benilla_protocol::messages::{
    contact_flag, friend_result, friend_status, ContactList, FriendStatusUpdate,
};
use bevy::prelude::*;

use crate::ui_script::{UiFeed, UiInput};

mod feed;
mod query;

pub(crate) use feed::feed_social;
pub(crate) use query::parse as who_query;

/// The friend array's slots (`FriendList` `+0x0`).
const MAX_FRIENDS: usize = 100;
/// The ignore and mute arrays' slots (`+0xd490`, `+0xd7c0`).
const MAX_IGNORES: usize = 50;

/// One friend as the reference's `FriendInfo` holds it. A logout clears only `connected`: the
/// level, class and zone stay as last sent (`SetConnected` `0x6b3d10`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Friend {
    pub(crate) guid: u64,
    /// The [`friend_status`] set last sent: away flags and Recruit-A-Friend.
    pub(crate) status: u8,
    pub(crate) connected: bool,
    pub(crate) note: String,
    pub(crate) area: u32,
    pub(crate) level: u32,
    pub(crate) class: u32,
}

/// The social session mirror: filled by the social handlers, read by the feed, cleared on
/// disconnect.
#[derive(Resource, Default)]
pub(crate) struct SocialState {
    /// The friend list in wire order; the feed sorts for display.
    friends: Vec<Friend>,
    ignores: Vec<u64>,
    /// The voice mute list.
    mutes: Vec<u64>,
    /// Each list's selection as a guid (`+0xd488`, `+0xd7b8`, `+0xdae8`); 0 is none.
    selected_friend: u64,
    selected_ignore: u64,
    selected_mute: u64,
    /// Each list's guids in the order the feed last showed them, for Lua's row indices.
    display_order: Vec<u64>,
    ignore_display_order: Vec<u64>,
    mute_display_order: Vec<u64>,
    /// The last `/who` answer's first 50 rows (`0x6b875c`); `who_total` is the server's full match
    /// count and `who_sent` the rows the wire carried, which picks chat or the frame.
    who: Vec<benilla_protocol::messages::WhoEntry>,
    who_total: u32,
    who_sent: u32,
    /// The `/who` sort chain (`SortWho`'s seven `{key, dir}` slots at `0xc79f9c`), pushed with the
    /// rows so the binding can re-sort synchronously.
    who_sort: benilla_ui::script::WhoSortChain,
    /// `SetWhoToUI`, which the Who frame sets on show and clears on hide.
    who_to_ui: bool,
    /// Results whose line waits on a name query (the reference formats from the name cache).
    pending_lines: Vec<FriendStatusUpdate>,
    /// Set when a list changed, so the feed fires its event once every name has resolved.
    friends_dirty: bool,
    ignores_dirty: bool,
    mutes_dirty: bool,
    who_dirty: bool,
}

impl SocialState {
    /// Whether `guid` is on the friend list, online or not: the highlight colour's friend test.
    pub(crate) fn is_friend(&self, guid: u64) -> bool {
        self.friends.iter().any(|f| f.guid == guid)
    }

    /// Drop the session's state: the server re-sends every list at login, and a stale ignore list
    /// would silence the wrong guids. The `/who` rows and sort chain go too, as the reference
    /// reseeds both at each world entry (`FriendList::Initialize` `0x6b8990`, from `0x405653`).
    pub(crate) fn clear_session(&mut self) {
        *self = Self::default();
    }

    /// Is `guid` on the ignore list? The reference's `IsIgnored` (`0x6b52e0`): an ignored player's
    /// chat and text emotes drop silently, and their duel is declined.
    pub(crate) fn is_ignored(&self, guid: u64) -> bool {
        guid != 0 && self.ignores.contains(&guid)
    }

    /// The ignore list as a set, the calendar's filter (`0x5c0104`).
    pub(crate) fn ignored_set(&self) -> std::collections::HashSet<u64> {
        self.ignores.iter().copied().collect()
    }

    /// The friend list's guids, which the calendar's complaint gate excludes (`0x5bca34`).
    pub(crate) fn friend_set(&self) -> std::collections::HashSet<u64> {
        self.friends.iter().map(|f| f.guid).collect()
    }

    /// Is `guid` on the friend list? The reference's `IsFriend` (`0x6b3920`), which keeps the
    /// guild sign-on line from announcing a friend twice.
    pub(crate) fn is_friend(&self, guid: u64) -> bool {
        guid != 0 && self.friends.iter().any(|f| f.guid == guid)
    }

    /// `SMSG_CONTACT_LIST` (`ReadPacket` `0x6b7dd0`): each list the mask names is replaced by the
    /// entries carrying its bit, up to its slot count; the other lists stand.
    fn apply_contact_list(&mut self, list: ContactList) {
        let on = |flag: u32| list.mask & flag != 0;
        let guids = |flag: u32| -> Vec<u64> {
            list.contacts
                .iter()
                .filter(|c| c.flags & flag != 0)
                .map(|c| c.guid)
                .take(MAX_IGNORES)
                .collect()
        };
        if on(contact_flag::FRIEND) {
            self.friends = list
                .contacts
                .iter()
                .filter(|c| c.flags & contact_flag::FRIEND != 0)
                .take(MAX_FRIENDS)
                .map(|c| Friend {
                    guid: c.guid,
                    status: c.status,
                    connected: c.status != friend_status::OFFLINE,
                    note: c.note.clone(),
                    area: c.area,
                    level: c.level,
                    class: c.class,
                })
                .collect();
            self.friends_dirty = true;
        }
        if on(contact_flag::IGNORED) {
            self.ignores = guids(contact_flag::IGNORED);
            self.ignores_dirty = true;
        }
        if on(contact_flag::MUTED) {
            self.mutes = guids(contact_flag::MUTED);
            self.mutes_dirty = true;
        }
    }

    /// Seat the ignore list directly, for tests of the code that consults it.
    #[cfg(test)]
    pub(crate) fn set_ignores_for_test(&mut self, guids: Vec<u64>) {
        self.ignores = guids;
        self.ignores_dirty = true;
    }

    /// `SMSG_FRIEND_STATUS` (`HandleStatus` `0x6b73a0`): apply the result and queue its line. The
    /// server sends no fresh list after an add or a remove, so the result codes maintain the lists.
    fn apply_friend_status(&mut self, update: FriendStatusUpdate) {
        use friend_result as R;
        match update.result {
            R::ONLINE => {
                if let Some(friend) = self.friend_mut(update.guid) {
                    if let Some(online) = update.online {
                        friend.status = online.status;
                        friend.area = online.area;
                        friend.level = online.level;
                        friend.class = online.class;
                    }
                    friend.connected = true;
                }
                self.friends_dirty = true;
            }
            R::OFFLINE => {
                if let Some(friend) = self.friend_mut(update.guid) {
                    friend.connected = false;
                }
                self.friends_dirty = true;
            }
            R::REMOVED => {
                self.friends.retain(|f| f.guid != update.guid);
                self.friends_dirty = true;
            }
            // `GetFriendIndexByGuid` (`0x6b3c10`) seats an unlisted guid: an add, and also the
            // already-listed refusal.
            R::ADDED_ONLINE | R::ADDED_OFFLINE | R::ALREADY => {
                let friend = self.seat_friend(update.guid);
                if let Some(friend) = friend {
                    if let Some(note) = &update.note {
                        friend.note = note.clone();
                    }
                    if let Some(online) = update.online {
                        friend.status = online.status;
                        friend.area = online.area;
                        friend.level = online.level;
                        friend.class = online.class;
                    }
                    match update.result {
                        R::ADDED_ONLINE => friend.connected = true,
                        R::ADDED_OFFLINE => friend.connected = false,
                        _ => {}
                    }
                }
                self.friends_dirty = true;
            }
            R::FLAGS_CHANGED => {
                if let (Some(friend), Some(status)) = (self.friend_mut(update.guid), update.status)
                {
                    friend.status = status;
                }
                self.friends_dirty = true;
            }
            R::AREA_CHANGED => {
                if let (Some(friend), Some(area)) = (self.friend_mut(update.guid), update.area) {
                    friend.area = area;
                }
                self.friends_dirty = true;
            }
            R::IGNORE_ADDED => {
                seat(&mut self.ignores, update.guid);
                self.ignores_dirty = true;
            }
            R::IGNORE_REMOVED => {
                self.ignores.retain(|g| *g != update.guid);
                self.ignores_dirty = true;
            }
            R::MUTE_ADDED => {
                seat(&mut self.mutes, update.guid);
                self.mutes_dirty = true;
            }
            R::MUTE_REMOVED => {
                self.mutes.retain(|g| *g != update.guid);
                self.mutes_dirty = true;
            }
            // Every other code is a refusal: a line, no change.
            _ => {}
        }
        self.pending_lines.push(update);
    }

    fn friend_mut(&mut self, guid: u64) -> Option<&mut Friend> {
        self.friends.iter_mut().find(|f| f.guid == guid)
    }

    /// The friend's row, seated in the first free slot when unlisted; `None` with the list full.
    fn seat_friend(&mut self, guid: u64) -> Option<&mut Friend> {
        match self.friends.iter().position(|f| f.guid == guid) {
            Some(i) => Some(&mut self.friends[i]),
            None if self.friends.len() < MAX_FRIENDS => {
                self.friends.push(Friend {
                    guid,
                    ..Default::default()
                });
                self.friends.last_mut()
            }
            None => None,
        }
    }

    /// A friend's guid at a 1-based row of the order last shown.
    fn friend_at_row(&self, row: u32) -> Option<u64> {
        row_guid(&self.display_order, row)
    }

    /// `SetNotes` (`0x6b5540`): the note the VM already stored, mirrored so the next push agrees.
    fn set_note(&mut self, guid: u64, note: &str) {
        if let Some(friend) = self.friend_mut(guid) {
            friend.note = note.to_string();
        }
    }

    /// `SMSG_WHO`: the answer to the last query, every row read and the first 50 kept.
    fn apply_who(&mut self, mut results: benilla_protocol::messages::WhoResults) {
        results.entries.truncate(WHO_MAX_RESULTS);
        self.who = results.entries;
        self.who_total = results.total;
        self.who_sent = results.displayed;
        self.who_dirty = true;
    }
}

/// `IgnoreAdded`/`MuteAdded` (`0x6b7200`, `0x6b72e0`): the guid takes the first free slot of 50,
/// unless it is listed already.
fn seat(list: &mut Vec<u64>, guid: u64) {
    if !list.contains(&guid) && list.len() < MAX_IGNORES {
        list.push(guid);
    }
}

/// The guid at a 1-based display row.
fn row_guid(order: &[u64], row: u32) -> Option<u64> {
    usize::try_from(row.checked_sub(1)?)
        .ok()
        .and_then(|i| order.get(i))
        .copied()
}

/// The rows the reference keeps of an answer (`cmp ecx,0x32` at `0x6b8751`).
const WHO_MAX_RESULTS: usize = 50;

/// The `GlobalStrings.lua` key a friend, ignore or mute result prints (`HandleStatus`
/// `0x6b73a0`), resolved at the feed; `None` prints nothing.
fn result_key(result: u8) -> Option<&'static str> {
    use friend_result as R;
    Some(match result {
        R::DB_ERROR | R::NOT_FOUND => "ERR_FRIEND_NOT_FOUND",
        R::LIST_FULL => "ERR_FRIEND_LIST_FULL",
        R::ONLINE => "ERR_FRIEND_ONLINE_SS",
        R::OFFLINE => "ERR_FRIEND_OFFLINE_S",
        R::REMOVED => "ERR_FRIEND_REMOVED_S",
        R::ADDED_ONLINE | R::ADDED_OFFLINE => "ERR_FRIEND_ADDED_S",
        R::ALREADY => "ERR_FRIEND_ALREADY_S",
        R::SELF => "ERR_FRIEND_SELF",
        R::ENEMY => "ERR_FRIEND_WRONG_FACTION",
        R::IGNORE_FULL => "ERR_IGNORE_FULL",
        R::IGNORE_SELF => "ERR_IGNORE_SELF",
        R::IGNORE_NOT_FOUND => "ERR_IGNORE_NOT_FOUND",
        R::IGNORE_ALREADY => "ERR_IGNORE_ALREADY_S",
        R::IGNORE_ADDED => "ERR_IGNORE_ADDED_S",
        R::IGNORE_REMOVED => "ERR_IGNORE_REMOVED_S",
        R::IGNORE_AMBIGUOUS => "ERR_IGNORE_AMBIGUOUS",
        R::MUTE_FULL => "ERR_VOICE_IGNORE_FULL",
        R::MUTE_SELF => "ERR_VOICE_IGNORE_SELF",
        R::MUTE_NOT_FOUND => "ERR_VOICE_IGNORE_NOT_FOUND",
        R::MUTE_ALREADY => "ERR_VOICE_IGNORE_ALREADY_S",
        R::MUTE_ADDED => "ERR_VOICE_IGNORE_ADDED_S",
        R::MUTE_REMOVED => "ERR_VOICE_IGNORE_REMOVED_S",
        R::MUTE_AMBIGUOUS => "ERR_VOICE_IGNORE_AMBIGUOUS",
        R::SILENT | R::FLAGS_CHANGED | R::AREA_CHANGED => return None,
        // The switch's default arm.
        _ => "ERR_FRIEND_ERROR",
    })
}

/// How many times the subject's name fills a result's message, as the key's suffix names it: none
/// for a bare key, one for `_S`, two for `_SS` (the `|Hplayer:%s|h[%s]|h` link). Read off the key,
/// not the text, so a string not yet resolved never passes for one that takes no name.
fn name_pushes(key: &str) -> usize {
    if key.ends_with("_SS") {
        2
    } else if key.ends_with("_S") {
        1
    } else {
        0
    }
}

/// The key a friend row's away tag resolves through (`GetFriendInfo` `0x6b4130`): DND outranks
/// AFK, and an offline friend has none.
fn status_flag_key(connected: bool, status: u8) -> Option<&'static str> {
    if !connected {
        None
    } else if status & friend_status::DND != 0 {
        Some("CHAT_FLAG_DND")
    } else if status & friend_status::AFK != 0 {
        Some("CHAT_FLAG_AFK")
    } else {
        None
    }
}

/// The social packet handlers: the contact lists, the `/who` answer and the result codes. The
/// lines and the list events fire from the feed, which resolves the names they need.
pub(crate) mod net {
    use super::*;
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;

    /// Register the handlers, one per kind, and the session-end listener.
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::ContactList, on_contact_list)
            .net_handler(K::FriendStatus, on_friend_status)
            .net_handler(K::WhoResults, on_who)
            .net_handler(K::Disconnected, on_session_end);
    }

    fn on_contact_list(In(ev): In<SessionEvent>, mut social: ResMut<SocialState>) {
        if let SessionEvent::ContactList(list) = ev {
            contact_list(&mut social, list);
        }
    }

    fn on_friend_status(In(ev): In<SessionEvent>, mut social: ResMut<SocialState>) {
        if let SessionEvent::FriendStatus(update) = ev {
            friend_status(&mut social, update);
        }
    }

    fn on_who(In(ev): In<SessionEvent>, mut social: ResMut<SocialState>) {
        if let SessionEvent::WhoResults(results) = ev {
            who(&mut social, results);
        }
    }

    /// A second handler on the session end, after the bridge's own teardown; `clear_session`, not
    /// `default()`, so the per-process sort chain survives.
    fn on_session_end(In(_): In<SessionEvent>, mut social: ResMut<SocialState>) {
        social.clear_session();
    }

    /// `SMSG_CONTACT_LIST`.
    pub(crate) fn contact_list(social: &mut SocialState, list: ContactList) {
        social.apply_contact_list(list);
    }

    /// `SMSG_FRIEND_STATUS`.
    pub(crate) fn friend_status(social: &mut SocialState, update: FriendStatusUpdate) {
        social.apply_friend_status(update);
    }

    /// `SMSG_WHO`.
    pub(crate) fn who(social: &mut SocialState, results: benilla_protocol::messages::WhoResults) {
        social.apply_who(results);
    }
}

/// The social window's session: the wire mirror, the VM feed, and the outbound intents.
pub(crate) struct UiSocialPlugin;

impl Plugin for UiSocialPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<SocialState>().add_systems(
            Update,
            (
                feed::feed_social.in_set(UiFeed),
                feed::drain_social.after(UiInput),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{ContactEntry, FriendOnline};

    fn status(result: u8, guid: u64) -> FriendStatusUpdate {
        FriendStatusUpdate::bare(result, guid)
    }

    fn online(status: u8, area: u32, level: u32, class: u32) -> Option<FriendOnline> {
        Some(FriendOnline {
            status,
            area,
            level,
            class,
        })
    }

    fn contact(guid: u64, flags: u32) -> ContactEntry {
        ContactEntry {
            guid,
            flags,
            ..Default::default()
        }
    }

    /// The login packet carries all three lists; a later friends-only refresh replaces the friends
    /// and leaves the ignores and mutes alone.
    #[test]
    fn a_contact_list_replaces_only_the_lists_its_mask_names() {
        let mut social = SocialState::default();
        social.apply_contact_list(ContactList {
            mask: contact_flag::FRIEND | contact_flag::IGNORED | contact_flag::MUTED,
            contacts: vec![
                ContactEntry {
                    note: "healer".into(),
                    status: friend_status::ONLINE | friend_status::AFK,
                    level: 80,
                    ..contact(7, contact_flag::FRIEND)
                },
                contact(9, contact_flag::IGNORED | contact_flag::MUTED),
            ],
        });
        assert_eq!(social.friends.len(), 1);
        assert!(social.friends[0].connected);
        assert_eq!(social.friends[0].note, "healer");
        assert_eq!(
            (social.ignores.clone(), social.mutes.clone()),
            (vec![9], vec![9])
        );
        assert!(social.friends_dirty && social.ignores_dirty && social.mutes_dirty);

        social.friends_dirty = false;
        social.ignores_dirty = false;
        social.apply_contact_list(ContactList {
            mask: contact_flag::FRIEND,
            contacts: vec![contact(8, contact_flag::FRIEND)],
        });
        assert_eq!(
            social.friends.iter().map(|f| f.guid).collect::<Vec<_>>(),
            [8]
        );
        assert!(!social.friends[0].connected, "status 0 is offline");
        assert_eq!(social.ignores, [9], "the ignores stand");
        assert!(social.friends_dirty && !social.ignores_dirty);
    }

    #[test]
    fn add_and_remove_results_maintain_the_list() {
        let mut social = SocialState::default();
        social.apply_friend_status(FriendStatusUpdate {
            note: Some("tank".into()),
            online: online(friend_status::ONLINE, 12, 60, 4),
            ..status(friend_result::ADDED_ONLINE, 7)
        });
        assert_eq!(social.friends.len(), 1);
        assert_eq!(social.friends[0].level, 60);
        assert_eq!(social.friends[0].note, "tank");
        assert!(social.friends[0].connected);

        // The same add arriving again must not duplicate the row.
        social.apply_friend_status(FriendStatusUpdate {
            note: Some(String::new()),
            ..status(friend_result::ADDED_OFFLINE, 7)
        });
        assert_eq!(social.friends.len(), 1);
        assert!(!social.friends[0].connected);

        social.apply_friend_status(status(friend_result::REMOVED, 7));
        assert!(social.friends.is_empty());

        // `ALREADY` seats a guid the client did not hold (`GetFriendIndexByGuid` `0x6b3c10`).
        social.apply_friend_status(status(friend_result::ALREADY, 8));
        assert_eq!(social.friends.len(), 1);
    }

    /// A logout clears only the connected bit (`SetConnected` `0x6b3d10`): the row keeps the last
    /// level, class and zone it was sent.
    #[test]
    fn presence_broadcasts_patch_the_row() {
        let mut social = SocialState::default();
        social.apply_contact_list(ContactList {
            mask: contact_flag::FRIEND,
            contacts: vec![contact(7, contact_flag::FRIEND)],
        });
        social.apply_friend_status(FriendStatusUpdate {
            online: online(friend_status::ONLINE | friend_status::AFK, 1519, 42, 8),
            ..status(friend_result::ONLINE, 7)
        });
        assert!(social.friends[0].connected);
        assert_eq!(social.friends[0].area, 1519);

        social.apply_friend_status(FriendStatusUpdate {
            status: Some(friend_status::ONLINE),
            ..status(friend_result::FLAGS_CHANGED, 7)
        });
        assert_eq!(social.friends[0].status, friend_status::ONLINE);
        social.apply_friend_status(FriendStatusUpdate {
            area: Some(4395),
            ..status(friend_result::AREA_CHANGED, 7)
        });
        assert_eq!(social.friends[0].area, 4395);

        social.apply_friend_status(status(friend_result::OFFLINE, 7));
        assert!(!social.friends[0].connected);
        assert_eq!(
            (social.friends[0].area, social.friends[0].level),
            (4395, 42),
            "the last zone and level stay"
        );
    }

    #[test]
    fn ignore_and_mute_results_maintain_their_lists() {
        let mut social = SocialState::default();
        assert!(!social.is_ignored(9));
        social.apply_friend_status(status(friend_result::IGNORE_ADDED, 9));
        assert!(social.is_ignored(9));
        assert!(!social.is_ignored(0), "guid 0 is never ignored");
        social.apply_friend_status(status(friend_result::IGNORE_REMOVED, 9));
        assert!(!social.is_ignored(9));

        social.apply_friend_status(status(friend_result::MUTE_ADDED, 9));
        social.apply_friend_status(status(friend_result::MUTE_ADDED, 9));
        assert_eq!(social.mutes, [9]);
        assert!(!social.is_ignored(9), "a mute is not an ignore");
        social.apply_friend_status(status(friend_result::MUTE_REMOVED, 9));
        assert!(social.mutes.is_empty());
    }

    #[test]
    fn a_refusal_changes_no_state_but_prints() {
        let mut social = SocialState::default();
        social.apply_friend_status(status(friend_result::SELF, 7));
        assert!(social.friends.is_empty());
        assert_eq!(social.pending_lines.len(), 1);
    }

    /// `HandleStatus` (`0x6b73a0`): codes 0 and 4 share the not-found line, 0x12-0x18 are the
    /// voice twins, 0x19-0x1b print nothing and anything past them is `ERR_FRIEND_ERROR`.
    #[test]
    fn every_result_code_maps_to_its_line() {
        assert_eq!(
            result_key(friend_result::DB_ERROR),
            Some("ERR_FRIEND_NOT_FOUND")
        );
        assert_eq!(
            result_key(friend_result::NOT_FOUND),
            Some("ERR_FRIEND_NOT_FOUND")
        );
        assert_eq!(
            result_key(friend_result::ADDED_ONLINE),
            result_key(friend_result::ADDED_OFFLINE),
        );
        assert_eq!(
            result_key(friend_result::MUTE_ALREADY),
            Some("ERR_VOICE_IGNORE_ALREADY_S")
        );
        for silent in [
            friend_result::SILENT,
            friend_result::FLAGS_CHANGED,
            friend_result::AREA_CHANGED,
        ] {
            assert_eq!(result_key(silent), None);
        }
        assert_eq!(result_key(0x1C), Some("ERR_FRIEND_ERROR"));
        assert_eq!(result_key(0x77), Some("ERR_FRIEND_ERROR"));
    }

    #[test]
    fn a_logout_drops_the_lists_the_answer_and_the_sort_chain() {
        let mut social = SocialState::default();
        social.apply_contact_list(ContactList {
            mask: 7,
            contacts: vec![
                contact(7, contact_flag::FRIEND),
                contact(9, contact_flag::IGNORED),
                contact(10, contact_flag::MUTED),
            ],
        });
        social.who_to_ui = true;
        social.who_sort.promote("level");
        social.who_sort.promote("level"); // descending
        assert_ne!(social.who_sort, benilla_ui::script::WhoSortChain::default());

        social.clear_session();
        assert!(social.friends.is_empty());
        assert!(social.ignores.is_empty());
        assert!(social.mutes.is_empty());
        assert!(social.who.is_empty());
        assert!(!social.who_to_ui, "the frame is closed after a logout");
        assert_eq!(
            social.who_sort,
            benilla_ui::script::WhoSortChain::default(),
            "reseeded at the next world entry"
        );
    }

    /// 57 rows on the wire: all are read, 50 kept, and the wire count still routes the answer.
    #[test]
    fn an_answer_keeps_its_first_fifty_rows() {
        let mut social = SocialState::default();
        let entries: Vec<_> = (0..57)
            .map(|i| benilla_protocol::messages::WhoEntry {
                name: format!("N{i:02}"),
                ..Default::default()
            })
            .collect();
        social.apply_who(benilla_protocol::messages::WhoResults {
            displayed: 57,
            total: 120,
            entries,
        });
        assert_eq!(social.who.len(), 50);
        assert_eq!(social.who[49].name, "N49");
        assert_eq!((social.who_sent, social.who_total), (57, 120));
    }

    #[test]
    fn the_key_suffix_is_the_name_arity() {
        assert_eq!(name_pushes("ERR_FRIEND_ONLINE_SS"), 2);
        assert_eq!(name_pushes("ERR_FRIEND_ADDED_S"), 1);
        assert_eq!(name_pushes("ERR_FRIEND_SELF"), 0, "not an _S despite the S");
    }

    #[test]
    fn the_away_tag_is_dnd_first_and_only_online() {
        let both = friend_status::ONLINE | friend_status::AFK | friend_status::DND;
        assert_eq!(status_flag_key(true, both), Some("CHAT_FLAG_DND"));
        assert_eq!(
            status_flag_key(true, friend_status::ONLINE | friend_status::AFK),
            Some("CHAT_FLAG_AFK")
        );
        assert_eq!(status_flag_key(false, both), None);
        assert_eq!(status_flag_key(true, friend_status::ONLINE), None);
    }

    #[test]
    fn every_key_resolves_and_its_arity_matches_the_string() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");

        let mut keys: Vec<&str> = (0x00..=0x1Cu8).filter_map(result_key).collect();
        keys.extend(["CHAT_FLAG_AFK", "CHAT_FLAG_DND"]);
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 26, "24 result lines plus the two away tags");

        for key in keys {
            let text: String = s.lua().globals().get(key).expect(key);
            assert!(!text.is_empty(), "{key} resolves empty");
            // The away tags are a row field, not a `DisplayError` message: no catalog row.
            if !key.starts_with("CHAT_FLAG_") {
                assert!(
                    benilla_ui::messages::by_key(key).is_some(),
                    "{key} is not a catalog row, so its surface and sound would be a guess"
                );
                assert_eq!(
                    text.matches("%s").count(),
                    name_pushes(key),
                    "{key}: the string's holes vs the arity its suffix promises"
                );
            }
        }
        assert_eq!(
            benilla_ui::messages::by_key("ERR_FRIEND_ONLINE_SS").and_then(|r| r.sound),
            Some("FRIENDJOINGAME"),
            "the cue the chat-log path was dropping"
        );
    }
}
