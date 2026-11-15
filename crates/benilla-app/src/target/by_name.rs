//! Selection by name: the reference's one resolver behind `/target`, `/assist` and `/follow`,
//! parameterised per binding.
//!
//! | binding | typemask | filter mode | exact-only |
//! |---|---|---|---|
//! | `TargetByName` | 8, creatures and players | 0 | Lua arg 2 |
//! | `AssistByName` | 0x10, players only | 0 | 0 |
//! | `FollowByName` | 0x10, players only | 2, assistable and alive | Lua arg 2 |
//!
//! Filter mode 0 underflows the reference's jump table into accept-all: no range, facing, reaction,
//! liveness, visibility or self test. A name matches when it starts with the whole query, compared
//! in UTF-8 without case at the query's length (`0x52029d`, `0x52016d`); exact-only compares the
//! whole name (`0x520293`). A query holding `-` compares a player from another realm as
//! `name-realm` (`0x5202a8`, `0x52012d`). The strictly nearest match wins (`0x520151`), whole name
//! or not, and a dead unit counts 10000 yd farther (`0x5200dd`).
//!
//! `/target` and `TargetUnit` also take the reference's group legs ([`target_named`]): a group
//! member by the whole name first, and a party or raid member the object manager does not hold
//! behind every visible match. The active player scans at the group's distance (`0x52006a`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_protocol::guid;
use benilla_ui::script::{cmp_utf8i, starts_with_utf8i};

use crate::names::NameCache;
use crate::net::{Guid, GuidIndex, ObjectStore, SelfPlayer};

use super::{scan, Selection};

/// Which guid families a search accepts, the reference's typemask argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameSearch {
    /// Typemask 8, creatures and players (a player carries the UNIT bit): `/target`.
    AnyUnit,
    /// Typemask 0x10, players only: `/assist <name>` and `/follow <name>`.
    PlayerOnly,
}

impl NameSearch {
    fn accepts(self, guid_val: u64) -> bool {
        match self {
            Self::AnyUnit => guid::is_player(guid_val) || guid::is_creature_or_pet(guid_val),
            Self::PlayerOnly => guid::is_player(guid_val),
        }
    }
}

/// `/target <name>`: resolve and select (`TargetByName`).
#[derive(bevy::ecs::message::Message, Clone, Debug)]
pub(crate) struct TargetByNameRequest {
    pub(crate) name: String,
}

/// `/assist [name]`: select the basis unit's target (`AssistByName`, or bare
/// `AssistUnit("target")`, whose basis is the current selection).
#[derive(bevy::ecs::message::Message, Clone, Debug)]
pub(crate) struct AssistRequest {
    pub(crate) name: Option<String>,
}

/// Whether `dist` displaces the incumbent's: strictly nearer, against a best seeded at `FLT_MAX`
/// (`0x9ea8fc`), so a tie or a NaN keeps the incumbent, as the `fcomp`/`test ah,0x41` does
/// (`0x520151`).
fn nearer(dist: f32, best: Option<f32>) -> bool {
    dist < best.unwrap_or(f32::MAX)
}

/// A candidate's distance for the walk (`0x520050`): 3D from the active player, plus 10000 for a
/// unit dead, a ghost, feigning or lying dead (`0x5200a8`-`0x5200e3`).
fn match_distance(dist: f32, store: Option<&ObjectStore>) -> f32 {
    if store.is_some_and(|s| s.0.is_dead_or_ghost() || s.0.unit_reads_dead()) {
        DEAD_EXTRA + dist
    } else {
        dist
    }
}

/// The follow start gate (the reference's mode-3 chain) as a pure function, its order pinned by
/// test:
///
/// | check | failure |
/// |---|---|
/// | followee is a player (typemask `0x10`) | `0x128` `ERR_INVALID_FOLLOW_TARGET` |
/// | the followee passes the cooperation check | `0x128`, the same line |
/// | we are alive | `0x7e` `ERR_PLAYER_DEAD` |
/// | we are not stunned (`UNIT_FIELD_FLAGS & 0x40000`) | `0x191` `ERR_GENERIC_STUNNED` |
/// | we are not casting | `0x134` `ERR_TOOBUSYTOFOLLOW` |
/// | the followee is inside the row's 30 yd (`0x715c70`, 3D) | `0x126` `ERR_AUTOFOLLOW_TOO_FAR` |
///
/// There is no followee-alive check; the distance is the arm's, past its canceller, which ran.
fn follow_refusal_key(
    followee_is_player: bool,
    can_assist_followee: bool,
    we_are_dead: bool,
    we_are_stunned: bool,
    we_are_casting: bool,
    within_leash: bool,
) -> Option<&'static str> {
    if !followee_is_player || !can_assist_followee {
        return Some("ERR_INVALID_FOLLOW_TARGET");
    }
    if we_are_dead {
        return Some("ERR_PLAYER_DEAD");
    }
    if we_are_stunned {
        return Some("ERR_GENERIC_STUNNED");
    }
    if we_are_casting {
        return Some("ERR_TOOBUSYTOFOLLOW");
    }
    if !within_leash {
        return Some(FOLLOW_TOO_FAR);
    }
    None
}

/// The arm's refusal, the one past its canceller.
const FOLLOW_TOO_FAR: &str = "ERR_AUTOFOLLOW_TOO_FAR";

/// The compare length, the resolver's exact-only argument. The unit popup's Target and Follow pass
/// 1 (`UnitPopup.lua:557`, `UnitPopup.lua:623`), so a menu naming its unit never lands on a
/// bystander whose name extends it; the slash commands pass nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Match {
    /// The name starts with the whole query (`0x52029d`, the query's length in letters).
    PrefixOk,
    /// The whole name (`0x520293`, length `0x7fffffff`).
    ExactOnly,
}

/// Whether `name` answers `query` under `mode`, through the client's UTF-8 case-insensitive
/// compare (`0x52016d`, `0x5131ad`).
fn name_matches(query: &str, name: &str, mode: Match) -> bool {
    match mode {
        Match::PrefixOk => starts_with_utf8i(name, query),
        Match::ExactOnly => cmp_utf8i(name, query).is_eq(),
    }
}

/// Everything a by-name search reads.
#[derive(SystemParam)]
#[allow(clippy::type_complexity)] // one bundled param, the app's convention for big query sets
pub(crate) struct ByNameScan<'w, 's> {
    /// Every known unit, our own avatar included: the reference has no self-exclusion here.
    units: UnitQuery<'w, 's>,
    self_q: Query<
        'w,
        's,
        (
            Entity,
            &'static Guid,
            &'static Transform,
            Option<&'static ObjectStore>,
        ),
        With<SelfPlayer>,
    >,
    /// Read-only: a sweep fires no name query, so an uncached name does not match, as in the
    /// reference.
    names: Res<'w, NameCache>,
    /// The reaction inputs behind the assist check, for [`Filter::AssistableAlive`] and the follow
    /// gate.
    factions: Option<Res<'w, super::Factions>>,
    reputations: Res<'w, crate::net::Reputations>,
    /// The group, for the reaction's party rung; none without the party plugin.
    group: Option<Res<'w, crate::ui_party::GroupState>>,
    /// The form table behind a unit's creature type, for `StartAttack`'s totem skip.
    spells: Option<Res<'w, crate::ui_action::Spells>>,
}

/// The per-candidate filter, the mode argument the reference hands its filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filter {
    /// Mode 0, which underflows the jump table's bound into accept-all: `/target`, `/assist`.
    AcceptAll,
    /// Mode 2, the assist check and `UNIT_FIELD_HEALTH > 0`: `/follow`.
    AssistableAlive,
    /// Mode 1, the attack check and alive: `StartAttack`'s name.
    AttackableAlive,
}

impl ByNameScan<'_, '_> {
    /// The distance origin, the active player's position; without one the reference resolves
    /// nothing.
    fn origin(&self) -> Option<Vec3> {
        self.self_q
            .single()
            .ok()
            .map(|(_, _, tf, _)| tf.translation)
    }

    /// The active player's guid.
    fn me(&self) -> Option<u64> {
        self.self_q.single().ok().map(|(_, g, _, _)| g.0)
    }

    /// The mode-2 arm: alive, and the assist check as a friendly reaction (4+).
    fn assistable_alive(&self, store: Option<&ObjectStore>) -> bool {
        if store.is_some_and(|s| s.0.unit_is_dead()) {
            return false;
        }
        let self_store = self.self_q.single().ok().and_then(|(_, _, _, s)| s);
        super::ring_reaction(
            self.factions.as_deref(),
            &self.reputations,
            super::party_of(self.group.as_deref()),
            store,
            self_store,
        ) >= 4
    }

    /// The mode-1 arm: the reads-dead triple, then the attack check.
    fn attackable_alive(&self, store: Option<&ObjectStore>) -> bool {
        if store.is_some_and(|s| s.0.unit_reads_dead()) {
            return false;
        }
        let self_store = self.self_q.single().ok().and_then(|(_, _, _, s)| s);
        super::can_attack(
            store,
            self.factions.as_deref(),
            &self.reputations,
            super::party_of(self.group.as_deref()),
            self_store,
        )
    }

    /// `StartAttack`'s totem skip, its last resolver argument (`0x5230f8`): a totem (creature type
    /// 11) not friendly to us, reaction under 4 (`0x52008a`-`0x5200a2`, `0x514050`).
    fn hostile_totem(&self, store: Option<&ObjectStore>) -> bool {
        let Some(store) = store else {
            return false;
        };
        let sources = crate::creature_type::CreatureTypeSources::of_resources(
            &self.names,
            self.spells.as_deref(),
        );
        let self_store = self.self_q.single().ok().and_then(|(_, _, _, s)| s);
        skips_as_hostile_totem(sources.of(store), || {
            super::ring_reaction(
                self.factions.as_deref(),
                &self.reputations,
                super::party_of(self.group.as_deref()),
                Some(store),
                self_store,
            )
        })
    }

    /// The closest attackable unit by name, `StartAttack`'s search (`0x523090`).
    pub(super) fn attackable(&self, name: &str, exact: bool) -> Option<(Entity, u64)> {
        let mode = if exact {
            Match::ExactOnly
        } else {
            Match::PrefixOk
        };
        self.resolve(name, NameSearch::AnyUnit, Filter::AttackableAlive, mode)
            .map(|(entity, guid, _)| (entity, guid))
    }

    /// The follow start gate ([`follow_refusal_key`]): the key of the first refusal, for the UI's
    /// error display. `FollowUnit` skips the resolver but not this gate.
    fn follow_refusal(
        &self,
        followee: u64,
        followee_store: Option<&ObjectStore>,
        casting: bool,
        at: Option<Vec3>,
    ) -> Option<&'static str> {
        let self_store = self.self_q.single().ok().and_then(|(_, _, _, s)| s);
        let within_leash = match (at, self.origin()) {
            (Some(at), Some(me)) => at.distance_squared(me) < crate::player::FOLLOW_LEASH_SQ,
            _ => true,
        };
        follow_refusal_key(
            guid::is_player(followee),
            super::ring_reaction(
                self.factions.as_deref(),
                &self.reputations,
                super::party_of(self.group.as_deref()),
                followee_store,
                self_store,
            ) >= 4,
            self_store.is_some_and(|s| s.0.unit_is_dead()),
            self_store.is_some_and(|s| s.0.unit_flags() & crate::player::UNIT_FLAG_STUNNED != 0),
            casting,
            within_leash,
        )
    }

    /// Resolve a name to a unit through [`scan_units`], the mode argument deciding the filter.
    fn resolve(
        &self,
        query: &str,
        search: NameSearch,
        filter: Filter,
        mode: Match,
    ) -> Option<(Entity, u64, String)> {
        scan_units(
            self.units
                .iter()
                .map(|(e, g, tf, store)| (e, g.0, tf.translation, store)),
            self.origin(),
            self.me(),
            &self.names,
            query,
            search,
            mode,
            |store| match filter {
                Filter::AcceptAll => true,
                Filter::AssistableAlive => self.assistable_alive(store),
                Filter::AttackableAlive => {
                    self.attackable_alive(store) && !self.hostile_totem(store)
                }
            },
        )
    }
}

/// The distance the reference gives a group member out of view and the active player, the float at
/// `0xa02a3c` (`0x520331`, `0x52006a`).
const GROUP_DIST: f32 = 2.5e9;

/// What a dead unit adds to its distance, the float at `0xac8a74` (`0x5200dd`).
const DEAD_EXTRA: f32 = 10_000.0;

/// `CreatureType.dbc`'s Totem.
const CREATURE_TYPE_TOTEM: u32 = 11;

/// The totem skip's test: a totem whose reaction to us is under friendly.
fn skips_as_hostile_totem(creature_type: u32, reaction: impl FnOnce() -> u8) -> bool {
    creature_type == CREATURE_TYPE_TOTEM && reaction() < 4
}

/// One unit query's candidates, shared by [`ByNameScan`] and [`PlayerLookup`].
type UnitQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Guid,
        &'static Transform,
        Option<&'static ObjectStore>,
    ),
>;

/// The resolver's walk: a name to a unit, logging one `by-name:` line per call, ungated since it
/// runs per command: the candidates counted, the winner and on what. `keep` is the per-candidate
/// filter mode; `origin` is the active player's position, without which the reference resolves
/// nothing. The active player `me` ranks at the group's distance, behind every other living unit.
#[allow(clippy::too_many_arguments)] // the resolver's own argument list
fn scan_units<'a>(
    units: impl Iterator<Item = (Entity, u64, Vec3, Option<&'a ObjectStore>)>,
    origin: Option<Vec3>,
    me: Option<u64>,
    names: &NameCache,
    query: &str,
    search: NameSearch,
    mode: Match,
    keep: impl Fn(Option<&ObjectStore>) -> bool,
) -> Option<(Entity, u64, String)> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    let Some(origin) = origin else {
        info!("by-name: \"{query}\" — no active player object; nothing resolves");
        return None;
    };
    let realm_form = query.contains('-');
    let mut considered = 0usize;
    let mut nameless = 0usize;
    let mut best: Option<(Entity, u64, f32, String)> = None;
    for (entity, guid, pos, store) in units {
        if !search.accepts(guid) || !keep(store) {
            continue;
        }
        let Some(name) = names.peek(guid) else {
            nameless += 1;
            continue;
        };
        considered += 1;
        let dist = if Some(guid) == me {
            GROUP_DIST
        } else {
            match_distance(pos.distance(origin), store)
        };
        let full;
        let name = match names.player_realm(guid).filter(|_| realm_form) {
            Some(realm) => {
                full = format!("{name}-{realm}");
                full.as_str()
            }
            None => name,
        };
        if nearer(dist, best.as_ref().map(|b| b.2)) && name_matches(query, name, mode) {
            best = Some((entity, guid, dist, name.to_string()));
        }
    }
    match &best {
        Some((_, guid, dist, name)) => info!(
            "by-name: \"{query}\" ({search:?}) -> \"{name}\" guid {guid:#x} ({dist:.1} yd) \
             over {considered} named candidates ({nameless} unnamed)"
        ),
        None => info!(
            "by-name: \"{query}\" ({search:?}) -> NO MATCH over {considered} named candidates \
             ({nameless} unnamed, {mode:?}); target left untouched"
        ),
    }
    best.map(|(e, g, _, name)| (e, g, name))
}

/// The player-only name search for a caller that holds the [`NameCache`] mutably and so cannot take
/// [`ByNameScan`]: `DoEmote`'s target.
#[derive(SystemParam)]
pub(crate) struct PlayerLookup<'w, 's> {
    /// World positions, [`GlobalTransform`]: the chat drain already reads it, where `Transform`
    /// would put it in an undeclared order against every system that moves a unit.
    units: Query<
        'w,
        's,
        (
            Entity,
            &'static Guid,
            &'static GlobalTransform,
            Option<&'static ObjectStore>,
        ),
    >,
    self_q: Query<'w, 's, &'static GlobalTransform, With<SelfPlayer>>,
}

impl PlayerLookup<'_, '_> {
    /// The resolver's object walk with typemask `0x10`, filter mode 0 and no exact-only flag, past
    /// the active player, who scans at the group's distance (`0x51ffe0`): any other visible match or
    /// group member comes first, and a hit on ourselves is untargeted anyway. Selects nothing.
    pub(crate) fn find_other(&self, names: &NameCache, name: &str, me: Option<u64>) -> Option<u64> {
        let origin = self.self_q.single().ok().map(|tf| tf.translation());
        scan_units(
            self.units
                .iter()
                .filter(|(_, g, _, _)| Some(g.0) != me)
                .map(|(e, g, tf, store)| (e, g.0, tf.translation(), store)),
            origin,
            None,
            names,
            name,
            NameSearch::PlayerOnly,
            Match::PrefixOk,
            |_| true,
        )
        .map(|(_, g, _)| g)
    }

    /// The same walk over every player, with the exact-only flag as `StartDuel`'s second argument
    /// sets it.
    pub(crate) fn find_as(
        &self,
        names: &NameCache,
        name: &str,
        mode: Match,
    ) -> Option<(Entity, u64)> {
        let origin = self.self_q.single().ok().map(|tf| tf.translation());
        scan_units(
            self.units
                .iter()
                .map(|(e, g, tf, store)| (e, g.0, tf.translation(), store)),
            origin,
            None,
            names,
            name,
            NameSearch::PlayerOnly,
            mode,
            |_| true,
        )
        .map(|(e, g, _)| (e, g))
    }
}

/// `DoEmote`'s name search past the token words, which needs only a guid. The token resolver's name
/// half (`0x60a630`) takes the active player, then the raid or else the party, by the whole name;
/// the scan (`0x52027e`) walks the party slots, then the raid, each member with no object and a
/// cached name, then the visible objects, whose `streamed` hit is [`PlayerLookup::find_other`]'s and
/// outbids the group's distance (`0x520331`).
pub(crate) fn emote_name_match(
    names: &NameCache,
    roster: &crate::spell::group_relation::GroupRoster,
    me: Option<u64>,
    streamed: Option<u64>,
    held: impl Fn(u64) -> bool,
    query: &str,
) -> Option<u64> {
    whole_name_in_group(names, roster, me, query).or_else(|| {
        streamed
            .filter(|&g| {
                names
                    .peek(g)
                    .is_some_and(|n| name_matches(query, n, Match::PrefixOk))
            })
            .or_else(|| unheld_group_match(names, roster, held, query, Match::PrefixOk))
    })
}

/// The token resolver's name half (`0x60a630`): the active player, then the raid or else the
/// party, by the whole name, held or not.
fn whole_name_in_group(
    names: &NameCache,
    roster: &crate::spell::group_relation::GroupRoster,
    me: Option<u64>,
    query: &str,
) -> Option<u64> {
    let group = if roster.raid.is_empty() {
        &roster.party
    } else {
        &roster.raid
    };
    me.into_iter().chain(group.iter().copied()).find(|&g| {
        names
            .peek(g)
            .is_some_and(|n| name_matches(query, n, Match::ExactOnly))
    })
}

/// The group scans (`0x5202e1`, `0x52035d`): the party slots, then the raid rows, each member the
/// object manager does not hold with a cached name matching over the compare length; all share one
/// distance (`0x520331`), so the first wins.
fn unheld_group_match(
    names: &NameCache,
    roster: &crate::spell::group_relation::GroupRoster,
    held: impl Fn(u64) -> bool,
    query: &str,
    mode: Match,
) -> Option<u64> {
    let matches = |name: &str| name_matches(query, name, mode);
    roster
        .party
        .iter()
        .chain(&roster.raid)
        .copied()
        .find(|&g| g != 0 && !held(g) && names.peek(g).is_some_and(matches))
}

/// `/target <name>` and the Lua `TargetByName(name, exactMatch)` through [`scan::commit`], the
/// stop, select, re-swing path a click takes. `exact` is the Lua second argument, which the
/// reference reads with default 0 as the resolver's exact-only flag. A miss leaves the target
/// alone, as neither failure edge reaches the selection set, and prints nothing; the reference
/// prints message `0x127` `ERR_UNIT_NOT_FOUND` there, or `0xb8` `ERR_GENERIC_NO_TARGET` for an
/// empty name.
///
/// The group legs (`0x520190`, filter mode 0): a group member or ourselves by the whole name first
/// (`0x5201e5`), then the visible units, then a party or raid member out of view, which every
/// other visible match outbids. A group pick goes through the script select helper (`0x5259e0`).
/// True when an unheld guid was newly selected, which the caller sounds (`0x524eef`).
pub(super) fn target_named(
    scan_params: &ByNameScan,
    commit: &mut SelectCommit,
    name: &str,
    exact: bool,
) -> bool {
    let query = name.trim();
    if query.is_empty() {
        return false;
    }
    let Some(me) = scan_params.me() else {
        info!("by-name: \"{query}\" — no active player object; nothing resolves");
        return false;
    };
    let mode = if exact {
        Match::ExactOnly
    } else {
        Match::PrefixOk
    };
    let roster = scan_params
        .group
        .as_deref()
        .map(|g| crate::spell::group_relation::GroupRoster::of(g, Some(me)))
        .unwrap_or_default();
    if let Some(guid) = whole_name_in_group(&scan_params.names, &roster, Some(me), query) {
        info!("by-name: \"{query}\" -> the group's whole name, guid {guid:#x}");
        return commit.target_if_valid(guid);
    }
    let visible = scan_params.resolve(query, NameSearch::AnyUnit, Filter::AcceptAll, mode);
    if let Some((entity, guid, _)) = visible.as_ref().filter(|(_, g, _)| *g != me) {
        commit.commit(*entity, *guid);
        return false;
    }
    let held = |g: u64| commit.index.0.contains_key(&g);
    if let Some(guid) = unheld_group_match(&scan_params.names, &roster, held, query, mode) {
        info!("by-name: \"{query}\" -> group member guid {guid:#x}, not held");
        return commit.target_if_valid(guid);
    }
    if let Some((entity, guid, _)) = visible {
        commit.commit(entity, guid);
    }
    false
}

/// `/assist [name]` and the Lua `AssistByName`: the basis is a named player, or bare the current
/// selection (`AssistUnit("target")`, `ChatFrame.lua:744`), and [`SelectCommit::assist`] selects
/// its target. True when an unheld guid was newly selected.
pub(super) fn assist_named(
    scan_params: &ByNameScan,
    commit: &mut SelectCommit,
    name: Option<&str>,
    how: &str,
) -> bool {
    let basis = match name {
        Some(name) => scan_params
            .resolve(
                name,
                NameSearch::PlayerOnly,
                Filter::AcceptAll,
                Match::PrefixOk,
            )
            .map(|(e, _, _)| e),
        None => commit.selection.target,
    };
    let Some(basis) = basis else {
        info!("assist ({how}): no basis unit; nothing to assist");
        return false;
    };
    // From here on this is `AssistUnit`'s tail too, one function as in the reference.
    commit.assist(basis, how)
}

/// Drain `/follow [name]` into [`crate::player`]; follow sends nothing on the wire. The followee's
/// name is latched here for `AUTOFOLLOW_BEGIN` ([`crate::ui_follow`]).
pub(super) fn follow_requests(
    mut requests: MessageReader<crate::player::FollowRequest>,
    scan_params: ByNameScan,
    selection: Res<Selection>,
    group: Res<crate::ui_party::GroupState>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    places: Query<&Transform>,
    cast: Res<crate::spell::PendingCast>,
    mut errors: ResMut<crate::ui_action::UiErrorKeys>,
    mut follow: ResMut<crate::player::FollowState>,
    mut approach: ResMut<crate::player::Approach>,
    mut stand: MessageWriter<crate::player::StandStateRequest>,
) {
    for request in requests.read() {
        let resolved = match request {
            crate::player::FollowRequest::Name { name, exact } => scan_params
                .resolve(
                    name,
                    NameSearch::PlayerOnly,
                    Filter::AssistableAlive,
                    if *exact {
                        Match::ExactOnly
                    } else {
                        Match::PrefixOk
                    },
                )
                .map(|(_, guid, name)| (guid, name)),
            // `"target"` skips `player_token_guid`, whose players-only filter would make a
            // creature a silent miss: the start gate must refuse it with its error line.
            crate::player::FollowRequest::Unit(token) => match token.as_str() {
                "target" => selection.guid,
                tok => crate::ui_unit::player_token_guid(tok, &selection, &group),
            }
            .map(|guid| {
                let name = scan_params.names.peek(guid).unwrap_or_default().to_string();
                (guid, name)
            }),
        };
        // The start gate, whichever way the subject was found.
        if let Some((guid, _)) = &resolved {
            let entity = index.0.get(guid);
            let followee = entity.and_then(|e| stores.get(*e).ok());
            let at = entity.and_then(|e| places.get(*e).ok().map(|t| t.translation));
            if let Some(key) = scan_params.follow_refusal(
                *guid,
                followee,
                cast.in_flight(std::time::Instant::now()),
                at,
            ) {
                info!("follow: refused — {key}");
                errors.0.push(crate::ui_action::UiError::key(key));
                if key == FOLLOW_TOO_FAR {
                    approach.stop();
                    follow.stop();
                }
                continue;
            }
        }
        match resolved {
            Some((guid, name)) => {
                info!("follow: now following \"{name}\" guid {guid:#x}");
                // The arm cancels what runs first, one auto-move cell, and stands us up.
                approach.stop();
                follow.start(guid, name);
                let seated = scan_params
                    .self_q
                    .single()
                    .is_ok_and(|(_, _, _, s)| s.is_some_and(|s| s.0.unit_stand_state() != 0));
                if seated {
                    stand.write(crate::player::StandStateRequest { state: 0 });
                }
            }
            None => {
                // No subject by name or token: a follow in progress is left alone.
                info!("follow: nothing to follow");
            }
        }
    }
}

/// Everything [`scan::commit`], the one selection-set path, needs.
#[derive(SystemParam)]
#[allow(clippy::type_complexity)] // one bundled param, the app's convention for big query sets
pub(crate) struct SelectCommit<'w, 's> {
    pub(super) selection: ResMut<'w, Selection>,
    pub(crate) seam: crate::creature_anim::AttackSeam<'w, 's>,
    // Our own body: the guid, the store `can_attack` reads, and whether we are mid-swing.
    me: Query<
        'w,
        's,
        (
            &'static Guid,
            Option<&'static ObjectStore>,
            Has<crate::creature_anim::Engaged>,
        ),
        With<SelfPlayer>,
    >,
    pub(super) stores: Query<'w, 's, &'static ObjectStore>,
    /// The guid to entity map, the reference's lookup.
    index: Res<'w, GuidIndex>,
    factions: Option<Res<'w, super::Factions>>,
    reputations: Res<'w, crate::net::Reputations>,
    /// The group, for the reaction's party rung; none without the party plugin.
    group: Option<Res<'w, crate::ui_party::GroupState>>,
    /// `assistAttack`, the assist tail's second leg.
    assist_attack: Res<'w, super::AssistAttack>,
}

impl SelectCommit<'_, '_> {
    /// The shared assist tail (byte-identical in `AssistByName`): the basis unit's
    /// `UNIT_FIELD_TARGET` goes to the script select helper (`0x525f25` calls `0x5259e0`), so a
    /// held unit, or a group member or pet the object manager does not hold, is selected. Every
    /// other miss is silent and deselects nothing: a basis targeting nothing, or a stranger out of
    /// view. With `assistAttack` set, the tail then opens the swing on an attackable unit. True when
    /// an unheld guid was newly selected, which the caller sounds (`0x524eef`).
    pub(super) fn assist(&mut self, basis: Entity, how: &str) -> bool {
        let Some(guid) = self
            .stores
            .get(basis)
            .ok()
            .and_then(|s| s.0.unit_target())
            .filter(|g| *g != 0)
        else {
            info!("assist ({how}): the basis unit is targeting nothing; silent no-op");
            return false;
        };
        let Some(entity) = self.index.0.get(&guid).copied() else {
            // An unheld guid: a group member or pet by guid alone, else a no-op; never attackable.
            info!("assist ({how}): the basis is targeting guid {guid:#x}, which is not held");
            return self.target_if_valid(guid);
        };
        info!("assist ({how}) -> the basis unit's target, guid {guid:#x}");
        // `engaged` and attackability are read before the selection: the selection set may re-point
        // a swing we already had, and the swing start then skips the send but still runs its tail,
        // a sheath snap; not yet fighting, the second leg opens a fresh swing.
        let me = self.me.single().ok();
        let engaged = me.is_some_and(|(_, _, engaged)| engaged);
        let attackable = super::relations::can_attack(
            self.stores.get(entity).ok(),
            self.factions.as_deref(),
            &self.reputations,
            super::party_of(self.group.as_deref()),
            me.and_then(|(_, store, _)| store),
        );
        self.commit(entity, guid);
        // The second leg. The swing start's attackability test is the caller's here, so a friendly
        // target is selected, not swung at.
        if self.assist_attack.0 && attackable {
            info!("assist ({how}): assistAttack is on -> opening the swing");
            self.seam.start(guid, engaged, false);
        }
        false
    }

    /// Whether we are mid-swing, which a deselect stops.
    pub(super) fn engaged(&self) -> bool {
        self.me.single().is_ok_and(|(_, _, engaged)| engaged)
    }

    /// Deselect, a no-op with nothing selected; see [`super::click::clear`].
    pub(super) fn clear(&mut self) {
        let engaged = self.engaged();
        super::click::deselect(&mut self.selection, &mut self.seam, engaged);
    }

    /// The reference's select on a looted object, which tests `TYPEMASK_UNIT` first.
    pub(crate) fn select_unit(&mut self, guid: u64) {
        let Some(&entity) = self.index.0.get(&guid) else {
            return;
        };
        if self.stores.get(entity).is_ok_and(ObjectStore::is_unit) {
            self.commit(entity, guid);
        }
    }

    /// The script verbs' select (`0x5259e0`): a held unit, else a group member or pet by guid alone;
    /// anything else is a no-op. True when an unheld guid was newly selected, which the caller
    /// sounds (`0x524eef`).
    pub(super) fn target_if_valid(&mut self, guid: u64) -> bool {
        if let Some(&entity) = self.index.0.get(&guid) {
            self.commit(entity, guid);
            return false;
        }
        let me = self.me.single().ok();
        let own = me.and_then(|(g, store, _)| Some((g.0, &store?.0)));
        let held = |member: u64| {
            let &entity = self.index.0.get(&member)?;
            self.stores.get(entity).ok().map(|s| &s.0)
        };
        if !self
            .group
            .as_deref()
            .is_some_and(|g| g.names_member_or_pet(guid, own, held))
        {
            return false;
        }
        scan::commit_at(
            &mut self.selection,
            &mut self.seam,
            None,
            guid,
            None,
            me.is_some_and(|(_, _, engaged)| engaged),
            me.map(|(g, _, _)| g.0),
            false,
        )
        .changed
    }

    /// Select a resolved guid, the reference's arm 1, through [`scan::commit`].
    pub(super) fn commit(&mut self, entity: Entity, guid: u64) {
        let me = self.me.single().ok();
        // The re-swing's gate is `StartAttack`'s on the new target: alive, then the attack check,
        // so a body is switched to and never swung at.
        let attackable = scan::attack_target_valid(
            self.stores.get(entity).ok(),
            self.factions.as_deref(),
            &self.reputations,
            super::party_of(self.group.as_deref()),
            me.and_then(|(_, store, _)| store),
        );
        scan::commit(
            &mut self.selection,
            &mut self.seam,
            entity,
            guid,
            self.stores.get(entity).ok(),
            me.is_some_and(|(_, _, engaged)| engaged),
            me.map(|(g, _, _)| g.0),
            attackable,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One player candidate for [`scan_units`]: guid, name, distance along x, store.
    type Candidate = (u64, &'static str, f32, Option<ObjectStore>);

    /// Run the walk over `candidates` from the origin, players only, and return the winner's guid.
    fn pick(
        query: &str,
        mode: Match,
        candidates: &[Candidate],
        realms: &[(u64, &str)],
    ) -> Option<u64> {
        let mut names = NameCache::default();
        for &(guid, name, _, _) in candidates {
            names.insert_player(guid, name.to_string(), None);
        }
        for &(guid, realm) in realms {
            names.set_player_realm(guid, realm.to_string());
        }
        scan_units(
            candidates.iter().map(|(guid, _, x, store)| {
                (
                    Entity::PLACEHOLDER,
                    *guid,
                    Vec3::new(*x, 0.0, 0.0),
                    store.as_ref(),
                )
            }),
            Some(Vec3::ZERO),
            Some(ME),
            &names,
            query,
            NameSearch::PlayerOnly,
            mode,
            |_| true,
        )
        .map(|(_, g, _)| g)
    }

    /// The active player's guid in [`pick`].
    const ME: u64 = 0x99;

    /// A unit at 0 health of 100.
    fn dead() -> Option<ObjectStore> {
        use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
        Some(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (FIELD_UNIT_HEALTH, 0),
            (FIELD_UNIT_MAXHEALTH, 100),
        ])))
    }

    /// The scan's order: a member's whole name first, a visible player over an unstreamed member,
    /// an unstreamed member over nobody, party slots before the raid, and a member with an object or
    /// no cached name not at all.
    #[test]
    fn the_emote_search_finds_a_group_member_out_of_streaming_range() {
        use crate::spell::group_relation::GroupRoster;

        const ME: u64 = 1;
        const FAR: u64 = 2;
        const NEAR: u64 = 3;
        const NAMELESS: u64 = 4;
        const STRANGER: u64 = 5;
        const RAIDER: u64 = 6;

        let mut names = NameCache::default();
        for (guid, name) in [
            (ME, "Claude"),
            (FAR, "Farley"),
            (NEAR, "Nearby"),
            (STRANGER, "Farmer"),
            (RAIDER, "Raidley"),
        ] {
            names.insert_player(guid, name.to_string(), None);
        }
        let roster = GroupRoster {
            party: vec![NAMELESS, FAR, NEAR],
            raid: Vec::new(),
        };
        let held = |g| g == NEAR || g == ME || g == STRANGER;
        let find =
            |streamed, query| emote_name_match(&names, &roster, Some(ME), streamed, held, query);
        assert_eq!(
            find(None, "Farley"),
            Some(FAR),
            "the whole name, unstreamed"
        );
        assert_eq!(find(None, "farl"), Some(FAR), "a prefix, unstreamed");
        assert_eq!(
            find(Some(STRANGER), "Far"),
            Some(STRANGER),
            "visible outbids"
        );
        assert_eq!(find(None, "Claude"), Some(ME), "the whole own name");
        assert_eq!(
            find(None, "Near"),
            None,
            "a member with an object is the scan's"
        );
        assert_eq!(find(Some(NEAR), "Near"), Some(NEAR));
        assert_eq!(find(None, "Zed"), None);
        assert_eq!(
            find(Some(STRANGER), "Farz"),
            None,
            "a hit must start with the whole query"
        );
        let raid = GroupRoster {
            party: Vec::new(),
            raid: vec![ME, RAIDER],
        };
        assert_eq!(
            emote_name_match(&names, &raid, Some(ME), None, held, "Raid"),
            Some(RAIDER),
            "the raid roster, when the party slots miss"
        );
    }

    /// Filter modes 1 (`StartAttack`) and 2 (`/follow`) skip the group scans (`0x5202c3`), so a
    /// member out of view never resolves for them; the unheld scan alone takes them.
    #[test]
    fn the_attack_and_follow_modes_never_take_a_member_out_of_view() {
        use crate::spell::group_relation::GroupRoster;
        use bevy::ecs::system::RunSystemOnce;

        const ME: u64 = 1;
        const MEMBER: u64 = 0x1234;
        let mut world = World::new();
        world.init_resource::<NameCache>();
        world.init_resource::<crate::net::Reputations>();
        world.init_resource::<crate::ui_party::GroupState>();
        world
            .resource_mut::<crate::ui_party::GroupState>()
            .apply_list(
                0,
                0,
                vec![benilla_protocol::messages::GroupMemberEntry {
                    name: "Brisca".into(),
                    guid: MEMBER,
                    status: benilla_protocol::messages::member_status::ONLINE,
                    flags: 0,
                    roles: 0,
                }],
                ME,
                None,
                Some(ME),
            );
        world
            .resource_mut::<NameCache>()
            .insert_player(MEMBER, "Brisca".into(), None);
        world.spawn((SelfPlayer, Guid(ME), Transform::default()));
        for filter in [Filter::AttackableAlive, Filter::AssistableAlive] {
            let hit = world
                .run_system_once(move |scan: ByNameScan| {
                    scan.resolve("Brisca", NameSearch::PlayerOnly, filter, Match::PrefixOk)
                })
                .expect("the resolve runs as a one-shot system");
            assert_eq!(hit, None, "{filter:?}");
        }
        let roster = GroupRoster {
            party: vec![MEMBER],
            raid: Vec::new(),
        };
        let names = world.resource::<NameCache>();
        assert_eq!(
            unheld_group_match(names, &roster, |_| false, "bris", Match::PrefixOk),
            Some(MEMBER)
        );
        assert_eq!(
            unheld_group_match(names, &roster, |_| false, "bris", Match::ExactOnly),
            None
        );
        assert_eq!(
            unheld_group_match(names, &roster, |_| true, "Brisca", Match::ExactOnly),
            None,
            "a held member is the visible scan's"
        );
    }

    /// The compare runs to the query's length: a shared first letter is no match, and a query
    /// longer than the name fails.
    #[test]
    fn a_prefix_match_must_cover_the_whole_query() {
        assert!(!name_matches("Bob", "Brian", Match::PrefixOk));
        assert!(name_matches("Bob", "Bobby", Match::PrefixOk));
        assert!(name_matches("bOB", "Bobby", Match::PrefixOk), "case folded");
        assert!(!name_matches("Bobby", "Bob", Match::PrefixOk));
        assert!(!name_matches("zzz", "Ragnaros", Match::PrefixOk));
    }

    /// The fold is the client's UTF-8 one, not ASCII's: a Latin-1 capital matches its small letter.
    #[test]
    fn the_compare_folds_accented_letters() {
        assert!(name_matches("élu", "Élune", Match::PrefixOk));
        assert!(name_matches("ÉLUNE", "élune", Match::ExactOnly));
    }

    /// `/target Bob` with Brian alone in view leaves the target alone.
    #[test]
    fn a_shared_first_letter_resolves_nobody() {
        assert_eq!(
            pick("Bob", Match::PrefixOk, &[(1, "Brian", 2.0, None)], &[]),
            None
        );
        assert_eq!(
            pick("Bob", Match::PrefixOk, &[(1, "Bobby", 2.0, None)], &[]),
            Some(1)
        );
    }

    /// One tier: the nearest match wins, a whole-name hit included (`0x520151`).
    #[test]
    fn the_nearest_match_wins_over_a_farther_whole_name() {
        let near_prefix = (1, "Bobby", 5.0, None);
        let far_exact = (2, "Bob", 20.0, None);
        assert_eq!(
            pick(
                "bob",
                Match::PrefixOk,
                &[far_exact.clone(), near_prefix.clone()],
                &[]
            ),
            Some(1)
        );
        assert_eq!(
            pick("bob", Match::ExactOnly, &[far_exact, near_prefix], &[]),
            Some(2),
            "exact-only skips the nearer prefix"
        );
    }

    /// A dead unit counts 10000 yd farther (`0x5200dd`): behind any living match, ahead of the
    /// active player at the group's distance.
    #[test]
    fn a_dead_match_ranks_behind_the_living() {
        let corpse = (1, "Bobby", 1.0, dead());
        let living = (2, "Bobbins", 900.0, None);
        assert_eq!(
            pick("bob", Match::PrefixOk, &[corpse.clone(), living], &[]),
            Some(2)
        );
        let me = (ME, "Bobo", 0.0, None);
        assert_eq!(pick("bob", Match::PrefixOk, &[me, corpse], &[]), Some(1));
    }

    /// A tie keeps the incumbent, and a NaN never displaces.
    #[test]
    fn ties_keep_the_incumbent_and_nan_never_displaces() {
        assert_eq!(
            pick(
                "bob",
                Match::PrefixOk,
                &[(1, "Bob", 4.0, None), (2, "Bob", 4.0, None)],
                &[]
            ),
            Some(1)
        );
        assert!(!nearer(4.0, Some(4.0)));
        assert!(!nearer(f32::NAN, Some(4.0)));
        assert!(!nearer(f32::NAN, None));
        assert!(nearer(1.0e30, None));
    }

    /// A query holding `-` matches a player from another realm by `name-realm`, and only then.
    #[test]
    fn a_dashed_query_matches_the_realm_form() {
        let visitor = [(1, "Bob", 3.0, None)];
        let realms = [(1, "Outland")];
        assert_eq!(pick("bob-out", Match::PrefixOk, &visitor, &realms), Some(1));
        assert_eq!(
            pick("Bob-Outland", Match::ExactOnly, &visitor, &realms),
            Some(1)
        );
        assert_eq!(pick("Bob", Match::ExactOnly, &visitor, &realms), Some(1));
        assert_eq!(pick("Bob-Outland", Match::PrefixOk, &visitor, &[]), None);
    }

    /// `StartAttack` skips a totem not friendly to us, and only a totem.
    #[test]
    fn the_attack_search_skips_a_hostile_totem() {
        assert!(skips_as_hostile_totem(CREATURE_TYPE_TOTEM, || 1));
        assert!(skips_as_hostile_totem(CREATURE_TYPE_TOTEM, || 3));
        assert!(!skips_as_hostile_totem(CREATURE_TYPE_TOTEM, || 4));
        assert!(!skips_as_hostile_totem(7, || 1));
    }

    /// A dead player following a creature sees `ERR_INVALID_FOLLOW_TARGET`, not `ERR_PLAYER_DEAD`:
    /// the two followee checks come first and share message `0x128`.
    #[test]
    fn the_follow_gate_refuses_in_the_references_own_order() {
        // The clean case.
        assert_eq!(
            follow_refusal_key(true, true, false, false, false, true),
            None
        );
        // A creature refuses, however it was found.
        assert_eq!(
            follow_refusal_key(false, true, false, false, false, true),
            Some("ERR_INVALID_FOLLOW_TARGET")
        );
        // An enemy player fails the assist check, and shares the creature's line.
        assert_eq!(
            follow_refusal_key(true, false, false, false, false, true),
            Some("ERR_INVALID_FOLLOW_TARGET")
        );
        // Our own three, each shadowed by the followee checks above it.
        assert_eq!(
            follow_refusal_key(true, true, true, false, false, true),
            Some("ERR_PLAYER_DEAD")
        );
        assert_eq!(
            follow_refusal_key(true, true, false, true, false, true),
            Some("ERR_GENERIC_STUNNED")
        );
        assert_eq!(
            follow_refusal_key(true, true, false, false, true, true),
            Some("ERR_TOOBUSYTOFOLLOW")
        );
        // Everything wrong at once: the followee's line wins.
        assert_eq!(
            follow_refusal_key(false, false, true, true, true, true),
            Some("ERR_INVALID_FOLLOW_TARGET")
        );
        // Dead and stunned: dead is checked first.
        assert_eq!(
            follow_refusal_key(true, true, true, true, true, true),
            Some("ERR_PLAYER_DEAD")
        );
        // The arm's distance comes last, past every gate above.
        assert_eq!(
            follow_refusal_key(true, true, false, false, false, false),
            Some("ERR_AUTOFOLLOW_TOO_FAR")
        );
        assert_eq!(
            follow_refusal_key(true, true, false, false, true, false),
            Some("ERR_TOOBUSYTOFOLLOW")
        );
    }

    #[test]
    fn exact_only_compares_the_whole_name_but_keeps_the_case_fold() {
        assert!(!name_matches("rag", "Ragnaros", Match::ExactOnly));
        assert!(name_matches("RAGNAROS", "Ragnaros", Match::ExactOnly));
    }
}
