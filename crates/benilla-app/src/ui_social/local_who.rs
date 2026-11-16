//! The `/who` answer's local tail on a battleground or arena map, the reference's `DoLocalWho`
//! (`0x6b8290`), which `SMSG_WHO`'s handler calls after the server's rows (`0x6b88d7`).
//!
//! It walks the raid roster, not every streamed player, and keeps a member only from another realm
//! of the battlegroup: one whose cached realm equals ours is skipped (`0x6b84da`), as the server
//! already listed it. Each kept member must pass the last query as `SendWho` stored it: race and
//! class masks, the name up to its first `-`, the guild, the level range and every search word
//! (`MatchesFilter` `0x6b5ca0`: name, race, class, our zone or guild). Its row reads `name-Realm`
//! with the realm's spaces dropped, and our own zone (`[0xbd080c]`), while the array holds under
//! 50 rows.

use benilla_protocol::messages::{WhoEntry, WhoRequest};
use benilla_ui::script::contains_utf8i;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::chr_classes::ChrClassTable;
use crate::entities::CharCreate;
use crate::names::NameCache;
use crate::net::{NetCommands, Objects, SelfGuid};
use crate::ui_guild::GuildState;
use crate::ui_party::GroupState;

/// `Map.dbc` instance types that run the local pass (`0x6b88cb`, `0x6b88d1`).
const INSTANCE_TYPE_BATTLEGROUND: u32 = 3;
const INSTANCE_TYPE_ARENA: u32 = 4;

/// One raid member as the pass reads it: its cached name and realm and its descriptor.
pub(crate) struct LocalMember<'a> {
    pub(crate) name: &'a str,
    /// Empty for our own realm, as the name cache files it.
    pub(crate) realm: &'a str,
    pub(crate) race: u8,
    pub(crate) class: u8,
    pub(crate) gender: u8,
    pub(crate) level: u32,
    /// `PLAYER_GUILDID` is set.
    pub(crate) guilded: bool,
    /// The guild's name once its query answered.
    pub(crate) guild: Option<&'a str>,
}

/// What every member is matched against besides itself.
pub(crate) struct LocalScope<'a> {
    pub(crate) query: &'a WhoRequest,
    pub(crate) own_realm: &'a str,
    /// Our zone id and its name.
    pub(crate) zone: u32,
    pub(crate) zone_name: Option<&'a str>,
    pub(crate) race_name: &'a dyn Fn(u8) -> Option<&'a str>,
    pub(crate) class_name: &'a dyn Fn(u8) -> Option<&'a str>,
}

/// The rows the pass appends, at most `room` of them, in roster order.
pub(crate) fn local_rows<'a>(
    scope: &LocalScope<'a>,
    members: impl IntoIterator<Item = LocalMember<'a>>,
    room: usize,
) -> Vec<WhoEntry> {
    let query = scope.query;
    // The stored name is cut at its first `-` (`0x6b8408`), the realm half of `name-Realm`.
    let name_filter = query
        .player_name
        .split_once('-')
        .map_or(query.player_name.as_str(), |(name, _)| name);
    let mut rows = Vec::new();
    for m in members {
        if rows.len() >= room {
            break;
        }
        // `shl` takes the count mod 32 (`0x6b838e`).
        if query.race_mask & 1u32.wrapping_shl(u32::from(m.race)) == 0
            || query.class_mask & 1u32.wrapping_shl(u32::from(m.class)) == 0
        {
            continue;
        }
        if !name_filter.is_empty() && !contains_utf8i(m.name, name_filter) {
            continue;
        }
        if !query.guild_name.is_empty()
            && !(m.guilded
                && m.guild
                    .is_some_and(|g| contains_utf8i(g, &query.guild_name)))
        {
            continue;
        }
        // Signed compares (`jl`/`jg` at `0x6b8491`), so a range typed past 2^31 wraps.
        let level = m.level as i32;
        if level < query.level_min as i32 || level > query.level_max as i32 {
            continue;
        }
        if m.realm == scope.own_realm {
            continue;
        }
        if !query
            .search_terms
            .iter()
            .all(|w| matches_word(scope, &m, w))
        {
            continue;
        }
        let realm: String = m.realm.chars().filter(|&c| c != ' ').collect();
        rows.push(WhoEntry {
            name: format!("{}-{realm}", m.name),
            guild: m.guild.unwrap_or_default().to_string(),
            level: m.level,
            class: u32::from(m.class),
            race: u32::from(m.race),
            gender: m.gender,
            zone: scope.zone,
        });
    }
    rows
}

/// `MatchesFilter` (`0x6b5ca0`): the word is in the name, the race or class name, our zone's
/// name, or the guild's.
fn matches_word(scope: &LocalScope, m: &LocalMember, word: &str) -> bool {
    contains_utf8i(m.name, word)
        || (scope.race_name)(m.race).is_some_and(|n| contains_utf8i(n, word))
        || (scope.class_name)(m.class).is_some_and(|n| contains_utf8i(n, word))
        || scope.zone_name.is_some_and(|n| contains_utf8i(n, word))
        || m.guild.is_some_and(|g| contains_utf8i(g, word))
}

/// The world the pass reads, one system parameter for the `SMSG_WHO` handler.
#[derive(SystemParam)]
pub(crate) struct LocalWho<'w, 's> {
    maps: Option<Res<'w, benilla_assets::MapCatalogRes>>,
    here: Option<Res<'w, benilla_world::world_map::CurrentMap>>,
    area: Option<Res<'w, benilla_world::terrain_stream::CurrentArea>>,
    areas: Option<Res<'w, AreaTableRes>>,
    classes: Option<Res<'w, ChrClassTable>>,
    races: Option<Res<'w, CharCreate>>,
    group: Res<'w, GroupState>,
    me: Res<'w, SelfGuid>,
    names: Res<'w, NameCache>,
    guild: Res<'w, GuildState>,
    commands: Res<'w, NetCommands>,
    objects: Objects<'w, 's>,
}

impl LocalWho<'_, '_> {
    /// Whether the current map is a battleground or an arena.
    fn on_battlefield(&self) -> bool {
        let (Some(maps), Some(here)) = (self.maps.as_deref(), self.here.as_deref()) else {
            return false;
        };
        matches!(
            maps.0.instance_type(here.0),
            Some(INSTANCE_TYPE_BATTLEGROUND | INSTANCE_TYPE_ARENA)
        )
    }

    /// The local rows for `query`, at most `room`; none off a battlefield or before our own name
    /// is cached.
    pub(crate) fn rows(&self, query: &WhoRequest, room: usize) -> Vec<WhoEntry> {
        if room == 0 || !self.on_battlefield() {
            return Vec::new();
        }
        let Some(me) = self.me.0 else {
            return Vec::new();
        };
        // The reference reads our name record unchecked; without it there is no realm to compare.
        if self.names.peek(me).is_none() {
            return Vec::new();
        }
        let own_realm = self.names.player_realm(me).unwrap_or("");
        let areas = self.areas.as_deref().map(|a| &a.0);
        let zone = self
            .area
            .as_deref()
            .and_then(|a| a.0)
            .and_then(|id| areas?.top_zone(id))
            .unwrap_or(0);
        let race_name = |id: u8| -> Option<&str> {
            let rows = self.races.as_deref()?.0.race_rows();
            rows.iter().find(|r| r.id == id).map(|r| r.name.as_str())
        };
        let class_name =
            |id: u8| -> Option<&str> { self.classes.as_deref()?.0.name(u32::from(id)) };
        let scope = LocalScope {
            query,
            own_realm,
            zone,
            zone_name: areas.and_then(|a| a.name(zone)),
            race_name: &race_name,
            class_name: &class_name,
        };
        let members = crate::ui_party::raid_row_guids(&self.group, Some(me))
            .into_iter()
            .filter_map(|guid| {
                let fields = self.objects.object(guid)?;
                if fields.object_type() != Some(benilla_protocol::messages::ObjectType::Player) {
                    return None;
                }
                Some(LocalMember {
                    name: self.names.peek(guid)?,
                    realm: self.names.player_realm(guid).unwrap_or(""),
                    race: fields.unit_race()?,
                    class: fields.unit_class()?,
                    gender: fields.unit_gender()?,
                    level: fields.unit_level()?,
                    guilded: fields.player_guild_id() != 0,
                    guild: crate::ui_guild::player_guild_name(fields, &self.guild, &self.commands),
                })
            })
            .collect::<Vec<_>>();
        local_rows(&scope, members, room)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member<'a>(name: &'a str, realm: &'a str) -> LocalMember<'a> {
        LocalMember {
            name,
            realm,
            race: 1,
            class: 1,
            gender: 0,
            level: 80,
            guilded: false,
            guild: None,
        }
    }

    fn names(id: u8) -> Option<&'static str> {
        match id {
            1 => Some("Human"),
            _ => None,
        }
    }

    fn classes(id: u8) -> Option<&'static str> {
        match id {
            1 => Some("Warrior"),
            8 => Some("Mage"),
            _ => None,
        }
    }

    fn rows(query: &WhoRequest, members: Vec<LocalMember>, room: usize) -> Vec<String> {
        let scope = LocalScope {
            query,
            own_realm: "",
            zone: 3277,
            zone_name: Some("Warsong Gulch"),
            race_name: &|id| names(id),
            class_name: &|id| classes(id),
        };
        local_rows(&scope, members, room)
            .into_iter()
            .map(|r| r.name)
            .collect()
    }

    /// Our own realm's members are the server's to list; another realm's come back as
    /// `name-Realm`, spaces dropped, in our zone.
    #[test]
    fn only_another_realms_members_are_appended() {
        let query = WhoRequest::default();
        let members = vec![member("Ours", ""), member("Theirs", "Burning Legion")];
        assert_eq!(rows(&query, members, 50), ["Theirs-BurningLegion"]);
        let scope = LocalScope {
            query: &query,
            own_realm: "",
            zone: 3277,
            zone_name: None,
            race_name: &|id| names(id),
            class_name: &|id| classes(id),
        };
        let row = &local_rows(&scope, vec![member("Theirs", "X")], 50)[0];
        assert_eq!((row.zone, row.level, row.race, row.class), (3277, 80, 1, 1));
    }

    /// The masks, the name before its `-`, the level range and every search word filter.
    #[test]
    fn the_stored_query_filters_the_members() {
        let query = WhoRequest {
            class_mask: 1 << 8,
            ..Default::default()
        };
        let mut mage = member("Zap", "Far");
        mage.class = 8;
        assert_eq!(
            rows(&query, vec![member("Brick", "Far"), mage], 50),
            ["Zap-Far"]
        );

        let query = WhoRequest {
            player_name: "bri-Far".into(),
            ..Default::default()
        };
        assert_eq!(
            rows(
                &query,
                vec![member("Brick", "Far"), member("Zap", "Far")],
                50
            ),
            ["Brick-Far"],
            "the name filter stops at its `-`"
        );

        let query = WhoRequest {
            level_min: 70,
            level_max: 100,
            ..Default::default()
        };
        let mut low = member("Low", "Far");
        low.level = 10;
        assert_eq!(
            rows(&query, vec![low, member("High", "Far")], 50),
            ["High-Far"]
        );

        // Every word must match one of name, race, class, zone or guild.
        let query = WhoRequest {
            search_terms: vec!["warsong".into(), "warr".into()],
            ..Default::default()
        };
        let mut mage = member("Zap", "Far");
        mage.class = 8;
        assert_eq!(
            rows(&query, vec![member("Brick", "Far"), mage], 50),
            ["Brick-Far"]
        );
    }

    /// A guild word needs a guild whose name has landed.
    #[test]
    fn a_guild_filter_needs_a_named_guild() {
        let query = WhoRequest {
            guild_name: "steel".into(),
            ..Default::default()
        };
        let mut named = member("Named", "Far");
        named.guilded = true;
        named.guild = Some("Legacy of Steel");
        let mut pending = member("Pending", "Far");
        pending.guilded = true;
        assert_eq!(
            rows(&query, vec![named, pending, member("None", "Far")], 50),
            ["Named-Far"]
        );
    }

    /// The array holds 50 rows; the pass stops at the room left.
    #[test]
    fn the_pass_fills_only_the_room_left() {
        let query = WhoRequest::default();
        let members = vec![member("A", "Far"), member("B", "Far"), member("C", "Far")];
        assert_eq!(rows(&query, members, 2), ["A-Far", "B-Far"]);
    }
}
