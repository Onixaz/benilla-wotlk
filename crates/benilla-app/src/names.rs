//! Unit-name resolution. Descriptors carry no names: a player's answers `CMSG_NAME_QUERY` (by
//! guid), a creature's `CMSG_CREATURE_QUERY` (by template entry), a pet's or charm's
//! `CMSG_PET_NAME_QUERY` (by pet number), and which one applies is read off the descriptor as the
//! reference's `GetUnitName` reads it. Each key is asked once while in flight, and a negative
//! answer is cached so a bad id never loops; for a player that is an empty name, which the
//! reference evicts instead, so its next read asks again.
//!
//! Creature templates persist, as the reference's `creaturecache.wdb` does; player and pet names
//! are never written to disk and are cleared at world-session start, as the reference does.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_protocol::guid;

use crate::net::{ClientCommand, NetCommands, ObjectStore};
use crate::query_cache::QueryCache;

/// The creature template's `type_flags` dword from `SMSG_CREATURE_QUERY_RESPONSE`, cached by the
/// reference at `[CGUnit+0xb30] + 0x14`. Adjacent bits mean unrelated things, and the reference
/// gives each its own getter, so every consumer names the bit it reads:
///
/// | bit | meaning |
/// |---|---|
/// | `0x1` | TAMEABLE |
/// | `0x2` | VISIBLE_TO_GHOSTS |
/// | `0x4` | BOSS_MOB |
/// | `0x8` | [`DO_NOT_PLAY_WOUND_ANIM`] |
/// | `0x10` | [`NO_FACTION_TOOLTIP`], its getter inverted |
/// | `0x20` | [`MORE_AUDIBLE`] |
/// | `0x40` | SPELL_ATTACKABLE / no harmful vertex colouring |
/// | `0x80` | INTERACT_WHILE_DEAD |
///
/// The names of `0x1`-`0x40` follow vmangos `CreatureDefines.h:146-152`.
pub(crate) mod type_flags {
    /// `0x8`: the reference skips the victim wound flinch for it; the blood spurt and the floating
    /// combat text still play.
    pub(crate) const DO_NOT_PLAY_WOUND_ANIM: u32 = 0x8;

    /// `0x10`: the unit tooltip drops its faction line (which returns it inverted).
    pub(crate) const NO_FACTION_TOOLTIP: u32 = 0x10;

    /// `0x20`: the pass-2 election keeps an off-screen creature ticking so its combat stays
    /// audible.
    pub(crate) const MORE_AUDIBLE: u32 = 0x20;
}

/// The name cache: players by guid, creatures by template entry, pets by pet number.
#[derive(Resource, Default)]
pub(crate) struct NameCache {
    /// A `None` answer is an empty wire name: the server does not know the guid.
    players: QueryCache<u64, String>,
    /// `(race, class, gender)` from `SMSG_NAME_QUERY_RESPONSE`, the `$`-macro expander's fallback
    /// for an unstreamed subject. Present only when the server answered: a name learned otherwise
    /// (our own, seeded at login) has no entry, never a zero triple.
    player_traits: HashMap<u64, (u8, u8, u8)>,
    /// A `None` answer is an entry the server flagged unknown.
    creatures: QueryCache<u32, CreatureRecord>,
    /// No negative entry: the server answers for a live pet or says nothing.
    pets: QueryCache<u32, String>,
    /// Bumped by every landed answer and the pet-rename eviction, never by an ask: the gated
    /// feeds re-run when it moves.
    generation: u64,
}

/// One cached creature template head.
#[derive(Clone, Debug, Default)]
pub(crate) struct CreatureRecord {
    pub(crate) name: String,
    pub(crate) subname: Option<String>,
    /// The template's cursor (`IconName`), which the unit cursor `0x715ea0` resolves by name.
    pub(crate) cursor_name: Option<String>,
    pub(crate) creature_type: u32,
    /// `CreatureFamily.dbc` id, 0 for anything not a tameable beast or warlock minion (the table
    /// has no row 0).
    pub(crate) pet_family: u32,
    /// Elite rank 0..4 as the template declares it; read it through [`gated_rank`].
    pub(crate) rank: u32,
    /// Read bit by bit through [`type_flags`].
    pub(crate) type_flags: u32,
    pub(crate) civilian: bool,
    /// The tooltip's LEADER line.
    pub(crate) racial_leader: bool,
    /// `CreatureDisplayInfo.dbc` id, the first of the template's display ids, 0 when it has none.
    /// The only model for a unit with no world object, such as a stabled pet; an on-screen unit's
    /// own `UNIT_FIELD_DISPLAYID` wins.
    pub(crate) display_id: u32,
    /// The template's health and power multipliers (`0x71b8d0`, `0x71b960`).
    pub(crate) health_modifier: f32,
    pub(crate) power_modifier: f32,
}

/// The 1.12 client's creature-rank getter: 0 before the record is queried and 0 for any unit with a
/// `UNIT_FIELD_PETNUMBER` (a charmed or enslaved unit), else the template rank.
/// `UnitClassification`, the tooltip's ELITE/BOSS word and `UnitLevel`'s world-boss -1 all read
/// through it, so every rank read here goes through this function. No store reads as not-a-pet.
pub(crate) fn gated_rank(rec: Option<&CreatureRecord>, store: Option<&ObjectStore>) -> u32 {
    match rec {
        Some(rec) if !store.is_some_and(|s| s.0.unit_is_pet_or_charm()) => rec.rank,
        _ => 0,
    }
}

/// Which cache names a unit and under which key: the branch of the reference's `GetUnitName`, which
/// the combat log's `GetObjectName` also calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NameKey {
    /// The `OBJECT_FIELD_TYPE` player bit: the name cache by guid.
    Player(u64),
    /// `UNIT_FIELD_PETNUMBER` non-zero: the pet-name cache under that number.
    Pet(u32),
    /// `UNIT_FIELD_PETNUMBER` zero: the creature record under the descriptor's
    /// `OBJECT_FIELD_ENTRY`.
    Creature(u32),
}

impl NameKey {
    /// With a descriptor this is exactly: the pet number and entry are the unit's fields, not the
    /// guid's bits (a vmangos companion pet has a `HIGHGUID_PET` guid but `PETNUMBER` 0; a charmed
    /// creature has a `HIGHGUID_UNIT` guid and a pet number). Without one, a `HIGHGUID_PET` guid's
    /// entry slot is taken as the pet number, right only for a permanent pet.
    pub(crate) fn of(guid_val: u64, unit: Option<&ObjectStore>) -> Option<Self> {
        if guid::is_player(guid_val) {
            return Some(Self::Player(guid_val));
        }
        if !guid::is_creature_or_pet(guid_val) {
            return None;
        }
        match unit {
            Some(unit) => match unit.0.unit_pet_number() {
                0 => unit
                    .0
                    .object_entry()
                    .filter(|&e| e != 0)
                    .or_else(|| guid::entry(guid_val))
                    .map(Self::Creature),
                n => Some(Self::Pet(n)),
            },
            None => guid::pet_number(guid_val)
                .map(Self::Pet)
                .or_else(|| guid::entry(guid_val).map(Self::Creature)),
        }
    }
}

impl NameCache {
    /// The name for a guid with no descriptor in hand; on a miss, asks once and returns `None`.
    /// A caller holding the unit's [`ObjectStore`] uses [`Self::resolve_unit`]: the guid alone
    /// cannot name a companion pet or a guardian. A game object is `None` without a query.
    pub(crate) fn resolve(&self, guid_val: u64, commands: &NetCommands) -> Option<&str> {
        self.resolve_unit(guid_val, None, commands)
    }

    /// The name for a unit keyed off its descriptor, as the reference keys it; asks once on a miss.
    pub(crate) fn resolve_unit(
        &self,
        guid_val: u64,
        unit: Option<&ObjectStore>,
        commands: &NetCommands,
    ) -> Option<&str> {
        self.resolve_key(NameKey::of(guid_val, unit)?, guid_val, commands)
    }

    fn resolve_key(&self, key: NameKey, guid_val: u64, commands: &NetCommands) -> Option<&str> {
        match key {
            NameKey::Player(guid_val) => self
                .players
                .get_or_ask(guid_val, || {
                    debug!("names: asking player name (guid {guid_val})");
                    let _ = commands.0.send(ClientCommand::NameQuery { guid: guid_val });
                })
                .map(String::as_str),
            NameKey::Pet(pet_number) => self.resolve_pet(pet_number, guid_val, commands),
            NameKey::Creature(entry) => self.resolve_creature(entry, guid_val, commands),
        }
    }

    /// The answer under `key`, asking once: `None` in flight, `Some(None)` for an empty answer.
    pub(crate) fn answer(
        &self,
        key: NameKey,
        guid_val: u64,
        commands: &NetCommands,
    ) -> Option<Option<&str>> {
        if let Some(name) = self.resolve_key(key, guid_val, commands) {
            return Some(Some(name));
        }
        let answered = match key {
            NameKey::Player(guid_val) => self.players.answered(guid_val),
            NameKey::Pet(pet_number) => self.pets.answered(pet_number),
            NameKey::Creature(entry) => self.creatures.answered(entry),
        };
        answered.then_some(None)
    }

    /// A pet name asked once per pet number; an empty answer is cached as negative.
    fn resolve_pet(&self, pet_number: u32, guid: u64, commands: &NetCommands) -> Option<&str> {
        self.pets
            .get_or_ask(pet_number, || {
                debug!("names: asking pet name (pet {pet_number})");
                let _ = commands
                    .0
                    .send(ClientCommand::PetNameQuery { pet_number, guid });
            })
            .map(String::as_str)
    }

    /// The name for a creature template `entry`; `guid` is 0 when no spawn is known, since the
    /// server answers by entry alone.
    pub(crate) fn resolve_creature(
        &self,
        entry: u32,
        guid: u64,
        commands: &NetCommands,
    ) -> Option<&str> {
        self.creatures
            .get_or_ask(entry, || {
                debug!("names: asking creature name (entry {entry})");
                let _ = commands
                    .0
                    .send(ClientCommand::CreatureQuery { entry, guid });
            })
            .map(|r| r.name.as_str())
    }

    /// The cached name for `guid`, with no query on a miss.
    pub(crate) fn peek(&self, guid_val: u64) -> Option<&str> {
        self.peek_unit(guid_val, None)
    }

    /// The cached name for a unit keyed off its descriptor, with no query on a miss.
    pub(crate) fn peek_unit(&self, guid_val: u64, unit: Option<&ObjectStore>) -> Option<&str> {
        match NameKey::of(guid_val, unit)? {
            NameKey::Player(guid_val) => self.players.get(guid_val).map(String::as_str),
            NameKey::Pet(pet_number) => self.pets.get(pet_number).map(String::as_str),
            NameKey::Creature(entry) => self.creatures.get(entry).map(|r| r.name.as_str()),
        }
    }

    /// Record a player-name answer; an empty name is a negative answer. `traits` is `None` for a
    /// name learned without `SMSG_NAME_QUERY_RESPONSE`, and then clears any traits filed for the
    /// guid, since they belonged to the name being replaced.
    pub(crate) fn insert_player(&mut self, guid: u64, name: String, traits: Option<(u8, u8, u8)>) {
        match traits.filter(|_| !name.is_empty()) {
            Some(t) => self.player_traits.insert(guid, t),
            None => self.player_traits.remove(&guid),
        };
        self.players
            .insert(guid, (!name.is_empty()).then_some(name));
        self.generation = self.generation.wrapping_add(1);
    }

    /// The reference's world-session wipe: every `DBCache` whose persistence byte `[cache+0x39]` is
    /// 0 is cleared, which is `'WNAM'` (player names), `'WGLD'`, `'WPNM'` (pet names) and `'WPTN'`.
    /// Called at world entry, before the login seeds our own name.
    pub(crate) fn clear_world_session(&mut self) {
        self.players.clear();
        self.player_traits.clear();
        self.pets.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    /// The landed-answer counter.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// `(race, class, gender)` for a player guid the name query has answered for.
    pub(crate) fn player_traits(&self, guid: u64) -> Option<(u8, u8, u8)> {
        self.player_traits.get(&guid).copied()
    }

    /// Record a pet-name answer, keyed by pet number; an empty name is a negative answer, which
    /// the reference denies rather than caching (`DBCache::DenyItem`, `0x635365`).
    pub(crate) fn insert_pet(&mut self, pet_number: u32, name: String) {
        self.pets
            .insert(pet_number, (!name.is_empty()).then_some(name));
        self.generation = self.generation.wrapping_add(1);
    }

    /// Forget a pet's name and its in-flight mark so the next resolve asks again: a rename
    /// arrives only as a bumped `UNIT_FIELD_PET_NAME_TIMESTAMP`, never as the name.
    pub(crate) fn forget_pet(&mut self, pet_number: u32) {
        self.pets.evict(pet_number);
        self.generation = self.generation.wrapping_add(1);
    }

    /// Record a creature-name answer; `None` is an unknown entry.
    pub(crate) fn insert_creature(&mut self, entry: u32, record: Option<CreatureRecord>) {
        self.creatures.insert(entry, record);
        self.generation = self.generation.wrapping_add(1);
    }

    /// The cached subname, the title line under a creature's name.
    pub(crate) fn creature_subname(&self, entry: u32) -> Option<&str> {
        self.creatures.get(entry)?.subname.as_deref()
    }

    /// The cached template record. Key it by the descriptor's `OBJECT_FIELD_ENTRY`, never the
    /// guid's entry slot, as the reference does. On `None` each consumer picks its own default as
    /// the reference's getters do: `MORE_AUDIBLE` fails closed, `DO_NOT_PLAY_WOUND_ANIM` fails
    /// open.
    pub(crate) fn creature_record(&self, entry: u32) -> Option<&CreatureRecord> {
        self.creatures.get(entry)
    }

    /// The cached `CreatureType.dbc` id; the TAB-target scan treats `None` as targetable.
    pub(crate) fn creature_type(&self, entry: u32) -> Option<u32> {
        Some(self.creatures.get(entry)?.creature_type)
    }

    /// Forget the in-flight asks a disconnect may have dropped; player and creature names stay,
    /// pet names go with the spawns they named.
    pub(crate) fn clear_pending(&mut self) {
        self.players.clear_pending();
        self.creatures.clear_pending();
        self.pets.clear();
    }
}

/// `creaturecache.wdb`: the templates; player and pet names never touch disk.
impl crate::wdb::Persisted for NameCache {
    fn drop_persisted(&mut self) {
        self.creatures.clear();
        self.generation = self.generation.wrapping_add(1);
    }
}

impl crate::query_cache::AskOnce for NameCache {
    fn clear_pending(&mut self) {
        NameCache::clear_pending(self);
    }
}

impl NameCache {
    /// Drop a player's cached name and traits so the next resolve re-asks: the reference's
    /// remove-by-key for `SMSG_INVALIDATE_PLAYER`.
    pub(crate) fn invalidate_player(&mut self, guid: u64) {
        self.players.evict(guid);
        self.player_traits.remove(&guid);
        self.generation = self.generation.wrapping_add(1);
    }
}

/// The three name-query answers and `SMSG_INVALIDATE_PLAYER`.
pub(crate) mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{CreatureRecord, NameCache};
    use crate::net::NetHandlerApp;

    /// Register the four handlers.
    pub(crate) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::PlayerName, on_player_name)
            .net_handler(K::PetName, on_pet_name)
            .net_handler(K::CreatureName, on_creature_name)
            .net_handler(K::InvalidatePlayer, on_invalidate_player);
    }

    fn on_player_name(In(ev): In<SessionEvent>, mut names: ResMut<NameCache>) {
        if let SessionEvent::PlayerName {
            guid,
            name,
            race,
            class,
            gender,
        } = ev
        {
            player_name(guid, name, race, class, gender, &mut names);
        }
    }

    fn on_pet_name(In(ev): In<SessionEvent>, mut names: ResMut<NameCache>) {
        if let SessionEvent::PetName { pet_number, name } = ev {
            pet_name(pet_number, name, &mut names);
        }
    }

    fn on_creature_name(In(ev): In<SessionEvent>, mut names: ResMut<NameCache>) {
        if let SessionEvent::CreatureName {
            entry,
            name,
            subname,
            cursor_name,
            creature_type,
            pet_family,
            rank,
            type_flags,
            display_id,
            civilian,
            racial_leader,
            modifiers,
        } = ev
        {
            creature_name(
                entry,
                name,
                subname,
                cursor_name,
                creature_type,
                pet_family,
                rank,
                type_flags,
                civilian,
                racial_leader,
                (display_id, modifiers),
                &mut names,
            );
        }
    }

    fn on_invalidate_player(In(ev): In<SessionEvent>, mut names: ResMut<NameCache>) {
        if let SessionEvent::InvalidatePlayer { guid } = ev {
            invalidate_player(guid, &mut names);
        }
    }

    /// `SMSG_NAME_QUERY_RESPONSE`: the name plus race, class and gender.
    fn player_name(
        guid: u64,
        name: String,
        race: u32,
        class: u32,
        gender: u32,
        names: &mut NameCache,
    ) {
        names.insert_player(guid, name, Some((race as u8, class as u8, gender as u8)));
    }

    /// `SMSG_PET_NAME_QUERY_RESPONSE`, keyed by pet number.
    fn pet_name(pet_number: u32, name: String, names: &mut NameCache) {
        names.insert_pet(pet_number, name);
    }

    /// `SMSG_INVALIDATE_PLAYER`, which vmangos sends after a rename (`CharacterHandler.cpp:839`):
    /// the reference's eviction in a name cache with no TTL; its other, an empty-name answer, is a
    /// cached negative here.
    fn invalidate_player(guid: u64, names: &mut NameCache) {
        debug!("net: invalidating cached name for player {guid:#x}");
        names.invalidate_player(guid);
    }

    /// `SMSG_CREATURE_QUERY_RESPONSE`; a `None` name is the server's "no such entry".
    fn creature_name(
        entry: u32,
        name: Option<String>,
        subname: Option<String>,
        cursor_name: Option<String>,
        creature_type: Option<u32>,
        pet_family: u32,
        rank: u32,
        type_flags: u32,
        civilian: bool,
        racial_leader: bool,
        (display_id, (health_modifier, power_modifier)): (u32, (f32, f32)),
        names: &mut NameCache,
    ) {
        names.insert_creature(
            entry,
            name.map(|n| CreatureRecord {
                name: n,
                subname,
                cursor_name,
                creature_type: creature_type.unwrap_or(0),
                pet_family,
                rank,
                type_flags,
                civilian,
                racial_leader,
                display_id,
                health_modifier,
                power_modifier,
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::TryRecvError;

    /// Compose a guid the way the server does: `counter | (entry << 24) | (high << 48)`.
    fn compose(high: u16, entry: u32, counter: u32) -> u64 {
        u64::from(counter) | (u64::from(entry) << 24) | (u64::from(high) << 48)
    }

    fn commands() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (NetCommands(tx), rx)
    }

    /// An ask must not move the counter, or an outstanding query would hold every gated feed open.
    #[test]
    fn the_generation_counts_landings_never_asks() {
        let (cmds, _rx) = commands();
        let mut cache = NameCache::default();
        let g0 = cache.generation();

        let player = compose(0, 0, 7);
        assert_eq!(cache.resolve(player, &cmds), None);
        assert_eq!(
            cache.generation(),
            g0,
            "the miss asked, and asking is not a landing"
        );

        cache.insert_player(player, "Benilla".into(), None);
        let landed = cache.generation();
        assert_ne!(g0, landed, "the answer landing is the edge");

        assert_eq!(cache.resolve(player, &cmds), Some("Benilla"));
        assert_eq!(cache.generation(), landed, "a cache hit moves nothing");

        cache.insert_pet(137, "Voidwalker".into());
        let pet = cache.generation();
        assert_ne!(landed, pet);
        cache.forget_pet(137);
        assert_ne!(
            pet,
            cache.generation(),
            "the rename eviction is a landing-shaped edge"
        );
    }

    /// A summoned pet asks `CMSG_PET_NAME_QUERY` by the pet number in its guid; its entry slot
    /// names no creature template.
    #[test]
    fn a_pet_asks_the_pet_query_not_the_creature_query() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        let voidwalker = compose(guid::HIGH_PET, 137, 9);

        assert_eq!(cache.resolve(voidwalker, &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::PetNameQuery {
                pet_number: 137,
                ..
            })
        ));
        assert_eq!(cache.resolve(voidwalker, &cmds), None);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));

        cache.insert_pet(137, "Voidwalker".into());
        assert_eq!(cache.resolve(voidwalker, &cmds), Some("Voidwalker"));
        assert_eq!(cache.peek(voidwalker), Some("Voidwalker"));
        // Pet names are per-spawn, so a disconnect drops them.
        cache.clear_pending();
        assert_eq!(cache.peek(voidwalker), None);
    }

    /// An empty answer is a negative (`DBCache::DenyItem`, `0x635365`): no `""` name, no re-ask.
    #[test]
    fn an_empty_pet_name_is_a_negative_answer() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        let voidwalker = compose(guid::HIGH_PET, 137, 9);

        assert_eq!(cache.resolve(voidwalker, &cmds), None);
        assert!(rx.try_recv().is_ok());
        cache.insert_pet(137, String::new());

        assert_eq!(cache.peek(voidwalker), None);
        assert_eq!(
            cache.answer(NameKey::Pet(137), voidwalker, &cmds),
            Some(None)
        );
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    /// A stable list seeds the same `(pet_number, name)` pair the pet-name query answers, so an
    /// unstabled pet is named on arrival: `PetStable.lua:163` stores `UnitName("pet")` unguarded
    /// and `:178` hands it to `GameTooltip:SetText`, which raises on nil. vmangos packs the pet
    /// number into the guid's entry slot (`Pet.cpp:2250`).
    #[test]
    fn a_stable_list_names_the_pet_before_it_is_ever_summoned() {
        let (cmds, rx) = commands();
        let cache = NameCache::default();
        let rex = compose(guid::HIGH_PET, 7, 42);

        assert_eq!(cache.resolve(rex, &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::PetNameQuery { pet_number: 7, .. })
        ));

        let mut warm = NameCache::default();
        warm.insert_pet(7, "Rex".into());
        assert_eq!(warm.resolve(rex, &cmds), Some("Rex"));
        assert!(
            matches!(rx.try_recv(), Err(TryRecvError::Empty)),
            "a seeded pet must not re-ask"
        );
    }

    /// The reference's clears the stores whose `[cache+0x39]` byte is 0.
    #[test]
    fn world_entry_clears_the_identities_and_keeps_the_templates() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        cache.insert_player(0x11, "Moamtester".into(), Some((1, 2, 0)));
        cache.insert_pet(7, "Rex".into());
        cache.insert_creature(69, None);

        cache.clear_world_session();

        assert_eq!(cache.peek(0x11), None);
        assert_eq!(cache.player_traits(0x11), None);
        assert!(
            cache.creatures.answered(69),
            "templates outlive the world session"
        );
        // The in-flight marks went too.
        assert_eq!(cache.resolve(0x11, &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::NameQuery { guid: 0x11 })
        ));
    }

    #[test]
    fn a_name_replaced_without_traits_does_not_keep_the_old_ones() {
        let mut cache = NameCache::default();
        cache.insert_player(0x7, "Moamtester".into(), Some((1, 1, 0)));
        cache.insert_player(0x7, "Corances".into(), None);
        assert_eq!(cache.player_traits(0x7), None);
    }

    #[test]
    fn invalidating_a_player_makes_the_next_resolve_ask_again() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        cache.insert_player(0x11, "Aldric".into(), Some((1, 2, 0)));
        assert_eq!(cache.resolve(0x11, &cmds), Some("Aldric"));
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)), "no ask");

        let before = cache.generation();
        cache.invalidate_player(0x11);
        assert!(
            cache.generation() != before,
            "an eviction is a landed change"
        );
        assert_eq!(
            cache.player_traits(0x11),
            None,
            "the traits go with the name"
        );

        assert_eq!(cache.resolve(0x11, &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::NameQuery { guid: 0x11 })
        ));
    }

    #[test]
    fn creature_miss_queries_once_then_serves_the_answer() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        let wolf_a = compose(guid::HIGH_UNIT, 69, 1);
        let wolf_b = compose(guid::HIGH_UNIT, 69, 2);

        assert_eq!(cache.resolve(wolf_a, &cmds), None);
        // Same entry, different spawn: no second query.
        assert_eq!(cache.resolve(wolf_b, &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CreatureQuery { entry: 69, .. })
        ));
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));

        cache.insert_creature(
            69,
            Some(CreatureRecord {
                cursor_name: None,
                name: "Young Wolf".into(),
                subname: None,
                creature_type: 0,
                pet_family: 0,
                rank: 0,
                type_flags: 0,
                civilian: false,
                racial_leader: false,
                display_id: 0,
                ..Default::default()
            }),
        );
        assert_eq!(cache.resolve(wolf_b, &cmds), Some("Young Wolf"));
    }

    #[test]
    fn player_negative_answer_is_cached() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        let g = compose(guid::HIGH_PLAYER, 0, 7);

        assert_eq!(cache.resolve(g, &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::NameQuery { guid }) if guid == g
        ));
        cache.insert_player(g, String::new(), None); // server: unknown guid
        assert_eq!(cache.resolve(g, &cmds), None);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn clear_pending_allows_a_reconnect_reask() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        let g = compose(guid::HIGH_UNIT, 100, 1);

        assert_eq!(cache.resolve(g, &cmds), None);
        let _ = rx.try_recv();
        // A disconnect dropped the answer.
        cache.clear_pending();
        assert_eq!(cache.resolve(g, &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CreatureQuery { entry: 100, .. })
        ));
    }

    /// `OBJECT_FIELD_ENTRY` and `UNIT_FIELD_PETNUMBER`, absolute descriptor indices.
    const ENTRY: u16 = 3;
    const PETNUMBER: u16 = 139;

    fn unit(fields: &[(u16, u32)]) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(fields))
    }

    fn record(name: &str) -> CreatureRecord {
        CreatureRecord {
            cursor_name: None,
            name: name.into(),
            subname: None,
            creature_type: 0,
            pet_family: 0,
            rank: 0,
            type_flags: 0,
            civilian: false,
            racial_leader: false,
            display_id: 0,
            ..Default::default()
        }
    }

    /// The reference's `UNIT_FIELD_PETNUMBER == 0` leg: a vmangos companion pet's guid holds a pet
    /// number the server never answers for (`PetHandler.cpp:190-192`).
    #[test]
    fn a_companion_pet_is_named_by_its_descriptor_entry() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        let squirrel = compose(guid::HIGH_PET, 5_021, 3);
        let store = unit(&[(ENTRY, 2_671), (PETNUMBER, 0)]);
        cache.insert_creature(2_671, Some(record("Mechanical Squirrel")));

        assert_eq!(
            cache.resolve_unit(squirrel, Some(&store), &cmds),
            Some("Mechanical Squirrel")
        );
        assert_eq!(
            cache.peek_unit(squirrel, Some(&store)),
            Some("Mechanical Squirrel")
        );
        assert!(
            matches!(rx.try_recv(), Err(TryRecvError::Empty)),
            "no pet-name query for a unit whose PETNUMBER is 0"
        );
    }

    #[test]
    fn a_cold_companion_pet_asks_the_creature_query_for_its_entry() {
        let (cmds, rx) = commands();
        let cache = NameCache::default();
        let squirrel = compose(guid::HIGH_PET, 5_021, 3);
        let store = unit(&[(ENTRY, 2_671)]);

        assert_eq!(cache.resolve_unit(squirrel, Some(&store), &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CreatureQuery { entry: 2_671, guid }) if guid == squirrel
        ));
        assert_eq!(cache.resolve_unit(squirrel, Some(&store), &cmds), None);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn a_permanent_pet_asks_the_pet_query_for_its_descriptor_number() {
        let (cmds, rx) = commands();
        let mut cache = NameCache::default();
        let rex = compose(guid::HIGH_PET, 7, 42);
        let store = unit(&[(ENTRY, 3_122), (PETNUMBER, 7)]);
        cache.insert_creature(3_122, Some(record("Bloodtalon Taillasher")));

        assert_eq!(cache.resolve_unit(rex, Some(&store), &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::PetNameQuery { pet_number: 7, guid }) if guid == rex
        ));
        cache.insert_pet(7, "Rex".into());
        assert_eq!(cache.resolve_unit(rex, Some(&store), &cmds), Some("Rex"));
        assert_eq!(cache.peek_unit(rex, Some(&store)), Some("Rex"));
    }

    /// A charmed creature gets a pet number (`SpellAuras.cpp:3248`) and is named under it.
    #[test]
    fn a_charmed_creature_is_named_by_its_pet_number() {
        let (cmds, rx) = commands();
        let cache = NameCache::default();
        let ogre = compose(guid::HIGH_UNIT, 1_000, 9);
        let store = unit(&[(ENTRY, 1_000), (PETNUMBER, 55)]);

        assert_eq!(cache.resolve_unit(ogre, Some(&store), &cmds), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::PetNameQuery { pet_number: 55, .. })
        ));
    }
}
