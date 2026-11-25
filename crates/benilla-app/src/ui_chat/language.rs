//! The language gate, the chat display's share of garbling: the exemptions, and how well the
//! character knows the language, the second argument of the substitution
//! [`benilla_formats::garble`]. Fluency is a client-side chain; the server sends no language list:
//! language → the known spell whose effect 39 declares it → its `SkillLineAbility` row → the
//! `PLAYER_SKILL_INFO` value. At 300 a line passes whole; below, each word passes when
//! `hash % 300 < skill`. The fold runs spell → language over known spells only (on spell add): five
//! spells declare Common, so the inverse would map Common to Old Tongue.

use benilla_formats::LanguageWords;
use bevy::prelude::*;

use crate::net::{ObjectStore, SelfPlayer};
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_spellbook::SkillLines;

/// The GM bit of `PLAYER_FLAGS` (descriptor 190), as the reference reads it.
const PLAYER_FLAGS_GM: u32 = 0x8;

/// Chat types forced to language 0 whatever the wire says, which drops both the garble and the
/// `[Language]` header.
const ALWAYS_UNIVERSAL: [u8; 4] = [
    benilla_protocol::messages::CHAT_MSG_EMOTE,
    benilla_protocol::messages::CHAT_MSG_SYSTEM,
    benilla_protocol::messages::CHAT_MSG_MONSTER_EMOTE,
    benilla_protocol::messages::CHAT_MSG_RAID_BOSS_EMOTE,
];

/// The garble word pool, `LanguageWords.dbc`.
#[derive(Resource, Default)]
pub(crate) struct ChatLanguages {
    /// While `None`, nothing garbles.
    words: Option<LanguageWords>,
}

impl ChatLanguages {
    /// A readable's text in `language` as a reader at `skill` sees it (`0x505930` with both flags
    /// set, as `0x589e90` calls it): `<…>` spans and punctuation stay.
    pub(crate) fn translate(&self, language: u32, skill: u32, text: &str) -> String {
        match self.words.as_ref() {
            Some(words) => benilla_formats::garble(
                words,
                language,
                skill,
                text,
                benilla_formats::Garble::ITEM_TEXT,
            ),
            None => text.to_string(),
        }
    }

    /// The line as a hearer at `skill` sees it: unchanged for language 0, fluency, or no word pool.
    fn garble(&self, language: u32, skill: u32, text: &str) -> String {
        let Some(words) = self.words.as_ref() else {
            return text.to_string();
        };
        benilla_formats::garble_chat(words, language, skill, text)
    }
}

/// The add's reads of the active player, made at its call: the GM bit of its descriptor and its
/// skill in a line's language, never a copy kept between frames.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct Hearer<'w, 's> {
    langs: Res<'w, ChatLanguages>,
    player: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    auras: Query<'w, 's, &'static crate::unit_auras::UnitAuras, With<SelfPlayer>>,
    actions: Option<Res<'w, PlayerActions>>,
    spells: Option<Res<'w, Spells>>,
    lines: Option<Res<'w, SkillLines>>,
}

impl Hearer<'_, '_> {
    /// The active player's descriptor, `None` with none (`0x50a3ad`).
    fn player(&self) -> Option<&ObjectStore> {
        self.player.iter().next()
    }

    /// The GM bit of the active player's `PLAYER_FLAGS`, read at the call: it drops the garble
    /// (`0x50a3bd`-`0x50a3cc`) and exempts the spam filter (`0x50a68a`-`0x50a699`).
    pub(crate) fn is_gm(&self) -> bool {
        self.player()
            .is_some_and(|p| p.0.player_flags() & PLAYER_FLAGS_GM != 0)
    }

    /// The language a line renders as: 0 for the narration types (`0x50a273`-`0x50a28a`) and for a
    /// GM hearer (`0x50a3d2`); any other keeps the wire's, with no active player too (`0x50a465`).
    pub(crate) fn effective_language(&self, chat_type: u8, language: u32) -> u32 {
        if ALWAYS_UNIVERSAL.contains(&chat_type) || self.is_gm() {
            return 0;
        }
        language
    }

    /// The line as this character hears it: garbled at its skill in the language read at the call
    /// (`0x50a472` -> `0x6e0640`), whole for language 0 or with no active player (`0x50a45b`-`0x50a465`).
    pub(crate) fn garble(&self, language: u32, text: &str) -> String {
        let Some(player) = self.player().filter(|_| language != 0) else {
            return text.to_string();
        };
        let skill = match (
            self.actions.as_deref(),
            self.spells.as_deref(),
            self.lines.as_deref(),
        ) {
            (Some(actions), Some(spells), Some(lines)) => language_skill(
                player,
                self.auras.iter().next(),
                &actions.spells,
                spells,
                lines,
                language,
            )
            .unwrap_or(0),
            _ => 0,
        };
        self.langs.garble(language, skill, text)
    }
}

/// Load `LanguageWords.dbc` once at startup.
pub(crate) fn load_language_words(
    mut langs: ResMut<ChatLanguages>,
    assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    if langs.words.is_some() {
        return;
    }
    let Some(assets) = assets else { return };
    let loaded = {
        use benilla_assets::LockRecover;
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_language_words(&mut chain)
    };
    match loaded {
        Ok(words) => {
            info!("ui_chat: {} language word pools", words.len());
            langs.words = Some(words);
        }
        // Not fatal: with no pool every line renders plain.
        Err(e) => warn!("ui_chat: language word pools unavailable — {e:#}"),
    }
}

/// The active player's default language id read at the call (`0x6d5e70`: the race byte of the
/// player's descriptor, its `ChrRaces` base language, 0 with no record); `None` with no active
/// player, which the log's header test reads at `0x50abbe`.
pub(crate) fn default_language_id(world: &mut World) -> Option<u32> {
    let race = world
        .query_filtered::<&ObjectStore, With<SelfPlayer>>()
        .iter(world)
        .next()
        .map(|store| store.0.unit_race())?;
    Some(
        match (
            race,
            world.get_resource::<crate::ui_unit::DefaultLanguagesRes>(),
        ) {
            (Some(race), Some(langs)) => langs.0.id(u32::from(race)),
            _ => 0,
        },
    )
}

/// The comprehend-language aura type, `EffectApplyAuraName` 244 (`0x6e0700`).
const AURA_COMPREHEND_LANGUAGE: u32 = 244;

/// Our skill in `language` at the call (`0x6e0640`), `None` where it answers 0: the last known spell
/// declaring it (`0x6e067d`), that spell's ability row for our race and class (`0x6e0775`), the row's
/// line in `PLAYER_SKILL_INFO` (`0x6e07c0`) with a `SkillLine.dbc` record (`0x6e07e4`-`0x6e080d`),
/// and its value. With no spell, the comprehend flag (`UNIT_FIELD_FLAGS_2 & 0x8`, `0x6e0699`) and a
/// held aura of type 244 on the language (`0x6e06c0`-`0x6e0717`) answer 300 (`0x6e0751`).
pub(crate) fn language_skill(
    store: &ObjectStore,
    auras: Option<&crate::unit_auras::UnitAuras>,
    known: &std::collections::BTreeSet<u32>,
    spells: &Spells,
    lines: &SkillLines,
    language: u32,
) -> Option<u32> {
    let declaring = known
        .iter()
        .rfind(|&&s| spells.catalog.declared_language(s) == Some(language));
    let Some(&spell) = declaring else {
        if store.0.unit_flags2() & 0x8 == 0 {
            return None;
        }
        let comprehends = auras.is_some_and(|a| {
            a.slot_spells().iter().any(|&(id, _)| {
                spells.catalog.get(id).is_some_and(|d| {
                    (0..3).any(|e| {
                        d.effect_apply_aura[e] == AURA_COMPREHEND_LANGUAGE
                            && d.effect_misc_value[e] == language as i32
                    })
                })
            })
        });
        return comprehends.then_some(300);
    };
    let (race, class) = crate::spell::race_class(Some(store));
    let line = lines.catalog.spell_to_line(spell, race, class)?;
    lines.catalog.line(line)?;
    skill_value(store, line)
}

/// A skill's `PLAYER_SKILL_INFO` value as the reference sums it: base, plus the permanent bonus
/// only when base is non-zero, plus the signed temporary bonus; `None` with no row.
fn skill_value(store: &ObjectStore, line: u32) -> Option<u32> {
    for i in 0..benilla_protocol::messages::PLAYER_SKILL_SLOTS {
        let Some(s) = store.0.player_skill(i) else {
            continue;
        };
        if u32::from(s.skill_id) != line {
            continue;
        }
        let mut base = i32::from(s.value);
        if base != 0 {
            base += i32::from(s.perm_bonus);
        }
        return Some((base + i32::from(s.temp_bonus)).max(0) as u32);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{
        CHAT_MSG_EMOTE, CHAT_MSG_MONSTER_EMOTE, CHAT_MSG_MONSTER_SAY, CHAT_MSG_RAID_BOSS_EMOTE,
        CHAT_MSG_SAY, CHAT_MSG_SYSTEM,
    };

    /// Run `f` over the add's reads of the active player, as the add makes them at its call.
    fn hear<T: Send + Sync + 'static>(
        world: &mut World,
        f: impl Fn(&Hearer) -> T + Send + Sync + 'static,
    ) -> T {
        use bevy::ecs::system::RunSystemOnce;
        world.init_resource::<ChatLanguages>();
        world.run_system_once(move |h: Hearer| f(&h)).unwrap()
    }

    fn player(world: &mut World, flags: u32) -> Entity {
        world
            .spawn((
                SelfPlayer,
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                    benilla_protocol::field::FIELD_PLAYER_FLAGS,
                    flags,
                )])),
            ))
            .id()
    }

    /// With no active player the wire's language stays (`0x50a465` skips only the garble), so
    /// arg3 and the log still name it; nothing garbles and no GM bit is read.
    #[test]
    fn no_active_player_keeps_the_language_and_skips_the_garble() {
        let mut world = World::new();
        assert_eq!(
            hear(&mut world, |h| h.effective_language(CHAT_MSG_SAY, 1)),
            1
        );
        assert!(!hear(&mut world, |h| h.is_gm()));
        assert_eq!(
            hear(&mut world, |h| h.garble(1, "hello there")),
            "hello there"
        );
    }

    #[test]
    fn the_narration_types_are_always_universal() {
        let mut world = World::new();
        player(&mut world, 0);
        // Speech keeps its language...
        assert_eq!(
            hear(&mut world, |h| h.effective_language(CHAT_MSG_SAY, 1)),
            1
        );
        assert_eq!(
            hear(&mut world, |h| h
                .effective_language(CHAT_MSG_MONSTER_SAY, 1)),
            1
        );
        // ...narration does not, whatever the wire said.
        for t in [
            CHAT_MSG_EMOTE,
            CHAT_MSG_SYSTEM,
            CHAT_MSG_MONSTER_EMOTE,
            CHAT_MSG_RAID_BOSS_EMOTE,
        ] {
            assert_eq!(
                hear(&mut world, move |h| h.effective_language(t, 1)),
                0,
                "chat type {t:#x}"
            );
        }
    }

    /// The GM bit is read off the descriptor at the call (`0x50a3bd`-`0x50a3cc`): a flags write
    /// earlier in the same frame is what the next line sees, with no feed between.
    #[test]
    fn the_gm_bit_is_read_from_the_active_player_at_the_call() {
        let mut world = World::new();
        let me = player(&mut world, 0);
        assert_eq!(
            hear(&mut world, |h| h.effective_language(CHAT_MSG_SAY, 1)),
            1
        );
        world.get_mut::<ObjectStore>(me).unwrap().0 = benilla_protocol::ObjectFields::from_pairs(
            &[(benilla_protocol::field::FIELD_PLAYER_FLAGS, PLAYER_FLAGS_GM)],
        );
        assert!(hear(&mut world, |h| h.is_gm()));
        assert_eq!(
            hear(&mut world, |h| h.effective_language(CHAT_MSG_SAY, 1)),
            0
        );
        assert_eq!(
            hear(&mut world, |h| h
                .effective_language(CHAT_MSG_MONSTER_SAY, 7)),
            0
        );
    }

    /// With no known spell declaring the language the skill is 0 (`0x6e0640` finds no entry);
    /// with no word pool the line still renders whole rather than blank.
    #[test]
    fn a_missing_pool_renders_plainly_rather_than_blanking_chat() {
        let mut world = World::new();
        player(&mut world, 0);
        assert_eq!(
            hear(&mut world, |h| h.garble(1, "hello there")),
            "hello there"
        );
    }

    /// The log header's default language is read at the call (`0x50abc2` -> `0x6d5e70`): no
    /// active player is `None`, a player created this frame answers its race's base language at
    /// once, and a race with no `ChrRaces` record answers 0.
    #[test]
    fn the_default_language_is_read_from_the_active_player_at_the_call() {
        let mut world = World::new();
        world.insert_resource(crate::ui_unit::DefaultLanguagesRes(
            benilla_formats::DefaultLanguages::from_rows(&[(1, 7, "Common"), (2, 1, "Orcish")]),
        ));
        assert_eq!(default_language_id(&mut world), None);
        // `UNIT_FIELD_BYTES_0` byte 0 is the race.
        const BYTES_0: u16 = benilla_protocol::field::FIELD_UNIT_BYTES_0;
        let player = world
            .spawn((
                SelfPlayer,
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(BYTES_0, 2)])),
            ))
            .id();
        assert_eq!(default_language_id(&mut world), Some(1));
        // Another character on the same session: the new race, with no frame between.
        world.entity_mut(player).despawn();
        let human = world
            .spawn((
                SelfPlayer,
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(BYTES_0, 1)])),
            ))
            .id();
        assert_eq!(default_language_id(&mut world), Some(7));
        world.entity_mut(human).despawn();
        world.spawn((
            SelfPlayer,
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(BYTES_0, 99)])),
        ));
        assert_eq!(default_language_id(&mut world), Some(0));
    }

    /// The comprehend leg (`0x6e0690`-`0x6e0751`): with no known spell for the language, the
    /// `UNIT_FIELD_FLAGS_2` bit `0x8` and an aura effect 244 naming it, in any of the three
    /// effects, give 300; a clear bit or no matching effect gives nothing.
    #[test]
    fn a_comprehend_aura_speaks_its_language_under_the_flag() {
        const TONGUE: u32 = 90_000;
        let lines = SkillLines {
            catalog: std::sync::Arc::new(benilla_formats::SkillLineCatalog::from_lines([])),
        };
        let known = std::collections::BTreeSet::new();
        let auras = crate::unit_auras::UnitAuras::of(0, &[(0, TONGUE, 1)]);
        let store = |flags2: u32| {
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                benilla_protocol::field::FIELD_UNIT_FLAGS_2,
                flags2,
            )]))
        };
        for effect in 0..3 {
            let mut d = benilla_formats::SpellDisplay::default();
            d.effect_apply_aura[effect] = 244;
            d.effect_misc_value[effect] = 2;
            let mut spells = Spells::empty_for_tests();
            spells.catalog =
                benilla_formats::SpellCatalog::from_displays([(TONGUE, d)].into_iter().collect());
            let skill = |flags2: u32, language: u32| {
                language_skill(
                    &store(flags2),
                    Some(&auras),
                    &known,
                    &spells,
                    &lines,
                    language,
                )
            };
            assert_eq!(skill(0x8, 2), Some(300), "effect {effect}");
            assert_eq!(skill(0, 2), None, "the flag clear");
            assert_eq!(skill(0x8, 1), None, "another language");
            assert_eq!(
                language_skill(&store(0x8), None, &known, &spells, &lines, 2),
                None,
                "no aura held"
            );
        }
    }
}
