//! The hunter pet's paper-doll stat block (`GetPetHappiness`, `GetPetExperience`, `HasPetUI`, `GetPetIcon`, `GetPetTalentTree`, `GetPetFoodTypes`) read live for the pet list's head (`0x5d3a24`), each gated on the pet gaining experience (`0x71b630`: a pet number and a held Hunter creator), and `UnitCreatureFamily` for any unit, ungated.

use bevy::prelude::*;

use benilla_protocol::ObjectFields;
use benilla_ui::script::PetStats;

use crate::names::NameCache;
use crate::net::NetCommands;

/// `UNIT_FIELD_BYTES_0` byte 1, Hunter: the class the stat gate tests. The enum is pinned by
/// `GetComboPoints`, which tests 4 and 0xB, Rogue and Druid, on that byte.
const CLASS_HUNTER: u8 = 3;

/// Power index 4, the happiness `GetPetHappiness` buckets: read by index, as a pet's displayed
/// power is focus.
const POWER_HAPPINESS: u8 = 4;

/// `PetPersonality.dbc`, loaded once; without it happiness answers the binding's failure form.
#[derive(Resource)]
pub(crate) struct PetStatTables {
    pub(crate) personalities: benilla_formats::PetPersonalities,
}

/// `CreatureFamily.dbc` (the word and icon) and `ItemPetFood.dbc` (the diet a family's food mask
/// names), apart from [`PetStatTables`] so each half degrades alone.
#[derive(Resource)]
pub(crate) struct PetFamilyTables {
    pub(crate) families: benilla_formats::CreatureFamilies,
    pub(crate) foods: benilla_formats::PetFoodNames,
}

/// The pet's family word, icon and diet, from its cached creature template. The template entry is
/// the descriptor's `OBJECT_FIELD_ENTRY`, not the guid's entry slot, which holds the pet number
/// (vmangos `Pet::Create`); `Creature::InitEntry` writes the real entry (`Creature.cpp:376`). A
/// miss sends the ask-once creature query and answers all-absent until it lands. The icon is
/// `CreatureFamily.dbc`'s own column: `GetPetIcon`'s answer and the stable's slot art.
fn family_for(
    pet: Option<&ObjectFields>,
    names: &NameCache,
    commands: &NetCommands,
    tables: Option<&PetFamilyTables>,
) -> (Option<String>, Option<String>, Vec<String>) {
    let Some(entry) = pet.and_then(|s| s.object_entry()).filter(|&e| e != 0) else {
        return (None, None, Vec::new());
    };
    // vmangos answers a creature query off the entry alone (`HandleCreatureQueryOpcode`), so the
    // guid is 0.
    let _ = names.resolve_creature(entry, 0, commands);
    let Some(tables) = tables else {
        return (None, None, Vec::new());
    };
    let Some(family_id) = names.creature_record(entry).map(|r| r.pet_family) else {
        return (None, None, Vec::new());
    };
    let Some(family) = tables.families.get(family_id) else {
        return (None, None, Vec::new());
    };
    (
        Some(family.name.clone()),
        tables.families.icon(family_id).map(str::to_string),
        tables
            .foods
            .for_mask(family.pet_food_mask)
            .into_iter()
            .map(str::to_string)
            .collect(),
    )
}

/// The pet's talent tree name (`0x5d3d80`): its template's family, that family's talent type, and
/// the first `TalentTab.dbc` row whose pet mask admits it; `None` without any of them.
fn tree_for(
    pet: Option<&ObjectFields>,
    names: &NameCache,
    tables: Option<&PetFamilyTables>,
    talents: Option<&crate::ui_talent::Talents>,
) -> Option<String> {
    let entry = pet?.object_entry().filter(|&e| e != 0)?;
    let family = names.creature_record(entry)?.pet_family;
    let tree = tables?.families.get(family)?.pet_talent_type;
    Some(talents?.catalog.tabs_for_pet(tree).first()?.name.clone())
}

/// The whole stat block for the pet's fields, `creator` its `UNIT_FIELD_CREATEDBY` held as a player, or [`PetStats::default`] with no pet.
fn stats_for(
    pet: Option<&ObjectFields>,
    creator: Option<&ObjectFields>,
    tables: Option<&PetStatTables>,
    family: (Option<String>, Option<String>, Vec<String>),
) -> (bool, PetStats) {
    let (family, icon, food_types) = family;
    let Some(fields) = pet else {
        return (false, PetStats::default());
    };
    // `HasPetUI`'s first return: no player, with a pet number (`0x5d3960`). A possessed creature has a bar and no pet number, so no paper doll.
    let has_ui = fields.object_type() != Some(benilla_protocol::messages::ObjectType::Player)
        && fields.unit_is_pet_or_charm();
    // The pet can gain experience: a pet number and a Hunter creator (`0x71b630`).
    let hunter =
        fields.unit_is_pet_or_charm() && creator.and_then(|c| c.unit_class()) == Some(CLASS_HUNTER);
    if !hunter {
        // The family word has no gate; the icon, diet and stats do (`0x5d3d1c`, `0x5d3c18`).
        return (
            has_ui,
            PetStats {
                family,
                icon,
                ..PetStats::default()
            },
        );
    }
    let (happiness, damage_percentage) = tables
        .and_then(|t| t.personalities.for_pet(None))
        .map(|p| {
            let h = p.happiness(fields.unit_power(POWER_HAPPINESS).unwrap_or(0));
            (Some(h.bucket), h.damage_percentage)
        })
        // No DBC: the binding's gate-failure numbers and a nil bucket.
        .unwrap_or((None, 100.0));
    (
        has_ui,
        PetStats {
            hunter_pet: true,
            happiness,
            damage_percentage,
            experience: fields.unit_pet_experience(),
            family,
            icon,
            food_types,
            talent_tree: None,
        },
    )
}

/// What the pet verbs read at call time, part of the VM's live view: the pet list, the tables and the creature cache.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PetView<'w> {
    list: Option<Res<'w, crate::ui_pet::PetList>>,
    tables: Option<Res<'w, PetStatTables>>,
    families: Option<Res<'w, PetFamilyTables>>,
    talents: Option<Res<'w, crate::ui_talent::Talents>>,
    names: Option<Res<'w, NameCache>>,
    commands: Option<Res<'w, NetCommands>>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    summons: Option<Res<'w, crate::spell::SummonSlots>>,
}

impl PetView<'_> {
    /// The pet list's head (`0x5d3a24`).
    pub(crate) fn head(&self) -> Option<u64> {
        self.list.as_ref()?.head()
    }

    /// The pet stat block for the head's fields, `unit_of` resolving its creator.
    pub(crate) fn stats<'a>(
        &self,
        pet: Option<&'a ObjectFields>,
        unit_of: impl Fn(u64) -> Option<&'a ObjectFields>,
    ) -> (bool, PetStats) {
        let (Some(names), Some(commands)) = (self.names.as_deref(), self.commands.as_deref())
        else {
            return (false, PetStats::default());
        };
        // The creator, looked up as a player (`0x71b656`).
        let creator = pet
            .and_then(|p| p.unit_created_by())
            .and_then(&unit_of)
            .filter(|f| f.object_type() == Some(benilla_protocol::messages::ObjectType::Player));
        let family = family_for(pet, names, commands, self.families.as_deref());
        let tree = tree_for(
            pet,
            names,
            self.families.as_deref(),
            self.talents.as_deref(),
        );
        let mut fresh = stats_for(pet, creator, self.tables.as_deref(), family);
        if fresh.1.hunter_pet {
            fresh.1.talent_tree = tree;
        }
        fresh
    }

    /// `(PetCanBeAbandoned, PetCanBeRenamed, PetCanBeDismissed)` for the head's fields: the first two need us as `UNIT_FIELD_SUMMONEDBY` (`0x5d37cb`), the refusal us as `UNIT_FIELD_CREATEDBY` (`0x5d386b`); with no pet it answers `(nil, nil, 1)`.
    pub(crate) fn menu(&self, pet: Option<&ObjectFields>, me: Option<u64>) -> (bool, bool, bool) {
        let flags = pet
            .filter(|f| f.unit_summoned_by() == me)
            .map_or(0, |f| f.unit_pet_flags());
        let (abandon, rename) = crate::ui_pet::menu_predicates(flags);
        let dismiss = !pet
            .filter(|f| f.unit_created_by() == me)
            .zip(self.spells.as_deref().zip(self.summons.as_deref()))
            .is_some_and(|(f, (spells, summons))| {
                crate::ui_pet::summon_bars_dismiss(f, &spells.catalog, &summons.0)
            });
        (abandon, rename, dismiss)
    }

    /// A unit's `CreatureFamily.dbc` name off its cached creature record (`0x7153e0`), asking a record not yet cached.
    pub(crate) fn creature_family(&self, unit: &ObjectFields) -> Option<String> {
        let (Some(names), Some(commands)) = (self.names.as_deref(), self.commands.as_deref())
        else {
            return None;
        };
        if unit.object_type() != Some(benilla_protocol::messages::ObjectType::Unit) {
            return None;
        }
        family_for(Some(unit), names, commands, self.families.as_deref()).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::ObjectStore;
    use benilla_protocol::ObjectFields;
    use benilla_ui::script::UiScript;

    const BYTES_0: u16 = benilla_protocol::field::FIELD_UNIT_BYTES_0;
    const BYTES_1: u16 = benilla_protocol::field::FIELD_UNIT_BYTES_1;
    const POWER5: u16 = benilla_protocol::field::FIELD_UNIT_POWER1 + 4;
    const PETNUMBER: u16 = benilla_protocol::field::FIELD_UNIT_PETNUMBER;
    const PETXP: u16 = benilla_protocol::field::FIELD_UNIT_PETEXPERIENCE;
    const PETNEXTXP: u16 = benilla_protocol::field::FIELD_UNIT_PETNEXTLEVELEXP;
    /// `OBJECT_FIELD_ENTRY`, the pet's creature-template id.
    const ENTRY: u16 = benilla_protocol::field::FIELD_OBJECT_ENTRY;
    /// vmangos `creature_template` entries: Imp (`pet_family` 23) and Stonetusk Boar (5).
    const IMP_ENTRY: u32 = 416;
    const BOAR_ENTRY: u32 = 113;

    fn hunter() -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[(
            BYTES_0,
            u32::from(CLASS_HUNTER) << 8,
        )]))
    }

    fn warlock() -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[(BYTES_0, 9 << 8)]))
    }

    /// A boar with byte 1 set, full happiness and mid-XP.
    fn boar() -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[
            (ENTRY, BOAR_ENTRY),
            (PETNUMBER, 42),
            (BYTES_1, 6 << 8),
            (POWER5, 1_000_000), // maximum happiness
            (PETXP, 4200),
            (PETNEXTXP, 8000),
        ]))
    }

    fn chain() -> Option<benilla_formats::Chain> {
        let data = benilla_formats::wow_data_or_skip!(None);
        Some(benilla_formats::open_chain(&data).expect("open chain"))
    }

    fn tables() -> Option<PetStatTables> {
        let mut chain = chain()?;
        Some(PetStatTables {
            personalities: benilla_formats::load_pet_personalities(&mut chain)
                .expect("personality"),
        })
    }

    fn family_tables() -> Option<PetFamilyTables> {
        let mut chain = chain()?;
        Some(PetFamilyTables {
            families: benilla_formats::load_creature_families(&mut chain).expect("families"),
            foods: benilla_formats::load_pet_food_names(&mut chain).expect("foods"),
        })
    }

    fn no_family() -> (Option<String>, Option<String>, Vec<String>) {
        (None, None, Vec::new())
    }

    /// A `NameCache` as after `SMSG_CREATURE_QUERY_RESPONSE` for `entry` landed.
    fn cache_with(entry: u32, pet_family: u32) -> NameCache {
        let mut names = NameCache::default();
        names.insert_creature(
            entry,
            Some(crate::names::CreatureRecord {
                cursor_name: None,
                name: "Snarl".into(),
                subname: None,
                creature_type: 1,
                pet_family,
                rank: 0,
                type_flags: 0,
                civilian: false,
                racial_leader: false,
                display_id: 0,
                ..Default::default()
            }),
        );
        names
    }

    fn commands() -> (
        NetCommands,
        crossbeam_channel::Receiver<crate::net::ClientCommand>,
    ) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (NetCommands(tx), rx)
    }

    /// On the real DBC data: a maxed pet's bucket with 125% damage and its experience.
    #[test]
    fn a_hunters_boar_reads_every_field() {
        let Some(t) = tables() else { return };
        let (has_ui, s) = stats_for(Some(&boar().0), Some(&hunter().0), Some(&t), no_family());
        assert!(has_ui && s.hunter_pet);
        assert_eq!(s.happiness, Some(3));
        assert_eq!(s.damage_percentage, 125.0);
        assert_eq!(s.experience, (4200, 8000));
    }

    #[test]
    fn happiness_buckets_the_raw_power() {
        let Some(t) = tables() else { return };
        let at = |raw: u32| {
            let store = ObjectStore(ObjectFields::from_pairs(&[
                (PETNUMBER, 42),
                (BYTES_1, 0),
                (POWER5, raw),
            ]));
            let (_, s) = stats_for(Some(&store.0), Some(&hunter().0), Some(&t), no_family());
            (s.happiness, s.damage_percentage)
        };
        assert_eq!(at(0), (Some(1), 75.0), "an unhappy pet deals 75%");
        assert_eq!(at(500_000), (Some(2), 100.0));
        assert_eq!(at(1_000_000), (Some(3), 125.0));
    }

    /// A warlock's imp has a pet number, so `HasPetUI`'s first return holds, and no stats.
    #[test]
    fn a_warlocks_minion_has_a_ui_and_no_stats() {
        let Some(t) = tables() else { return };
        let (has_ui, s) = stats_for(Some(&boar().0), Some(&warlock().0), Some(&t), no_family());
        assert!(
            has_ui,
            "the pet number is what HasPetUI's first return reads"
        );
        assert!(!s.hunter_pet);
        assert_eq!(s.happiness, None);
        assert_eq!(s.experience, (0, 0));
    }

    /// `PetHasActionBar` is the cached guid alone; the paper doll also needs a pet number.
    #[test]
    fn no_pet_number_means_no_pet_ui() {
        let Some(t) = tables() else { return };
        let possessed = ObjectStore(ObjectFields::from_pairs(&[(POWER5, 1_000_000)]));
        let (has_ui, s) = stats_for(Some(&possessed.0), Some(&hunter().0), Some(&t), no_family());
        assert!(!has_ui);
        assert!(!s.hunter_pet);
    }

    /// A zero byte 1 leaves the happiness bucket answering.
    #[test]
    fn a_zero_byte_one_still_answers_happiness() {
        let Some(t) = tables() else { return };
        let fresh = ObjectStore(ObjectFields::from_pairs(&[
            (PETNUMBER, 42),
            (BYTES_1, 0),
            (POWER5, 1_000_000),
        ]));
        let (_, s) = stats_for(Some(&fresh.0), Some(&hunter().0), Some(&t), no_family());
        assert_eq!(s.happiness, Some(3), "a fresh pet still answers");
    }

    #[test]
    fn absent_dbc_data_degrades_to_the_failure_numbers() {
        let (has_ui, s) = stats_for(Some(&boar().0), Some(&hunter().0), None, no_family());
        assert!(has_ui && s.hunter_pet);
        assert_eq!(s.happiness, None);
        assert_eq!(s.damage_percentage, 100.0);
        // The descriptor values need no table.
        assert_eq!(s.experience, (4200, 8000));
    }

    /// A missing tooltip key is silent: `getglobal` answers nil and the tooltip loses a line.
    #[test]
    fn every_happiness_string_resolves_in_the_real_global_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| {
            s.lua()
                .globals()
                .get::<String>(key)
                .ok()
                .unwrap_or_default()
        };

        // The bucket names are built by concatenation (`"PET_HAPPINESS"..happiness`).
        assert_eq!(g("PET_HAPPINESS1"), "Unhappy");
        assert_eq!(g("PET_HAPPINESS2"), "Content");
        assert_eq!(g("PET_HAPPINESS3"), "Happy");
        assert!(
            g("PET_DAMAGE_PERCENTAGE").contains("%d"),
            "the damage line formats the percentage in: {:?}",
            g("PET_DAMAGE_PERCENTAGE")
        );
    }

    #[test]
    fn no_pet_is_no_ui() {
        let (has_ui, s) = stats_for(None, Some(&hunter().0), None, no_family());
        assert!(!has_ui);
        assert_eq!(s, PetStats::default());
    }

    /// On the real DBC data, through the binding's three nil sources; asking by the guid's entry
    /// slot, a pet number, would answer nil forever and silently.
    #[test]
    fn the_pets_family_resolves_off_its_descriptor_entry() {
        let Some(t) = family_tables() else { return };
        let (cmds, rx) = commands();

        // 1. No pet.
        let names = NameCache::default();
        assert_eq!(family_for(None, &names, &cmds, Some(&t)), no_family());
        assert!(rx.try_recv().is_err(), "nothing to ask about");

        // 2. Unanswered: nil, and the ask goes out once.
        let pet = ObjectStore(ObjectFields::from_pairs(&[
            (ENTRY, IMP_ENTRY),
            (PETNUMBER, 7),
        ]));
        assert_eq!(
            family_for(Some(&pet.0), &names, &cmds, Some(&t)),
            no_family(),
            "un-queried is nil, not a guess"
        );
        assert!(
            matches!(
                rx.try_recv(),
                Ok(crate::net::ClientCommand::CreatureQuery { entry, .. }) if entry == IMP_ENTRY
            ),
            "the pet's DESCRIPTOR entry is what gets queried"
        );
        assert!(rx.try_recv().is_err(), "ask-once");

        // 3. Family 0, a template with no family: nil.
        let names = cache_with(IMP_ENTRY, 0);
        assert_eq!(
            family_for(Some(&pet.0), &names, &cmds, Some(&t)),
            no_family()
        );

        // 4. The Imp's family 23: a word, no icon (12340's row leaves it empty) and an empty diet.
        let names = cache_with(IMP_ENTRY, 23);
        assert_eq!(
            family_for(Some(&pet.0), &names, &cmds, Some(&t)),
            (Some("Imp".into()), None, Vec::new())
        );

        // 5. A boar (family 5): the word and six diets, in bit order.
        let names = cache_with(BOAR_ENTRY, 5);
        let (name, icon, diet) = family_for(Some(&boar().0), &names, &cmds, Some(&t));
        assert_eq!(name.as_deref(), Some("Boar"));
        assert_eq!(
            icon.as_deref(),
            Some("Interface\\Icons\\Ability_Hunter_Pet_Boar")
        );
        assert_eq!(diet, ["Meat", "Fish", "Cheese", "Bread", "Fungus", "Fruit"]);

        // 6. No DBC tables: nil.
        let names = cache_with(BOAR_ENTRY, 5);
        assert_eq!(
            family_for(Some(&boar().0), &names, &cmds, None),
            no_family()
        );
    }

    /// A boar under a non-hunter (family 5, food mask 63) is the one case that shows the split, as
    /// every warlock minion family ships mask 0.
    #[test]
    fn a_charmed_beast_keeps_its_family_word_and_loses_its_diet() {
        let Some(t) = tables() else { return };
        let boar_diet: Vec<String> = ["Meat", "Fish", "Cheese", "Bread", "Fungus", "Fruit"]
            .map(String::from)
            .to_vec();
        let (_, s) = stats_for(
            Some(&boar().0),
            Some(&warlock().0),
            Some(&t),
            (Some("Boar".into()), None, boar_diet.clone()),
        );
        assert!(!s.hunter_pet);
        assert_eq!(s.family.as_deref(), Some("Boar"), "the word is ungated");
        assert!(
            s.food_types.is_empty(),
            "…but the diet shares with the stats"
        );
        assert_eq!(s.happiness, None, "…as does the hunter machinery");

        // Under a hunter it gets both.
        let (_, s) = stats_for(
            Some(&boar().0),
            Some(&hunter().0),
            Some(&t),
            (Some("Boar".into()), None, boar_diet.clone()),
        );
        assert_eq!(s.family.as_deref(), Some("Boar"));
        assert_eq!(s.food_types, boar_diet);
    }

    #[test]
    fn no_pet_drops_the_family_too() {
        let (_, s) = stats_for(
            None,
            Some(&hunter().0),
            None,
            (Some("Imp".into()), None, vec![]),
        );
        assert_eq!(s.family, None);
        assert!(s.food_types.is_empty());
    }

    /// `UNIT_PET` must reach Lua with `HasPetUI()` already answering, as `PetTab_Update` reads it: the verb reads the pet list's head live, so no feed order decides it.
    #[test]
    fn unit_pet_reaches_lua_with_has_pet_ui_already_true() {
        use crate::char_select::ClientState;
        use crate::net::GuidIndex;
        use crate::ui_pet::UiPetPlugin;

        const PET_GUID: u64 = 0xf140_0000_0000_002a;

        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::InWorld)
            .init_resource::<NameCache>()
            .init_resource::<GuidIndex>()
            .init_resource::<crate::target::Selection>()
            .init_resource::<crate::net::SelfGuid>()
            .init_resource::<crate::ui_script::UiClock>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::AutoRepeatActive>()
            .init_resource::<crate::ui_loot::LootState>()
            .init_resource::<crate::ui_loot::LootLatch>()
            .add_message::<crate::creature_anim::SheathRequest>()
            .add_message::<crate::net::FieldChanged>()
            .insert_resource(NetCommands(tx))
            .add_plugins(UiPetPlugin);

        // The pet is in the world when the bar's guid lands: a cold summon.
        let pet = app
            .world_mut()
            // `OBJECT_FIELD_TYPE` with the unit bit: the verbs' lookup is unit-typed.
            .spawn(ObjectStore(ObjectFields::from_pairs(&[
                (benilla_protocol::field::FIELD_OBJECT_TYPE, 0x9),
                (ENTRY, BOAR_ENTRY),
                (PETNUMBER, 42),
            ])))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(PET_GUID, pet);
        // A frame standing in for the pet page records what `HasPetUI()` answers at dispatch.
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                BENILLA_SAW = "never fired"
                local f = CreateFrame("Frame")
                f:RegisterEvent("UNIT_PET")
                f:SetScript("OnEvent", function()
                    BENILLA_SAW = HasPetUI() and "has pet UI" or "no pet UI"
                end)
                "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);

        // The setter fires `UNIT_PET` once the guid and the list are written (`0x5d6aca`).
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::PetSpells(Box::new(
                benilla_protocol::messages::PetSpells {
                    pet_guid: PET_GUID,
                    ..Default::default()
                },
            ))],
        );

        let saw: String = app
            .world_mut()
            .non_send_resource::<UiScript>()
            .eval("return BENILLA_SAW")
            .unwrap();
        assert_eq!(
            saw, "has pet UI",
            "UNIT_PET must not reach Lua ahead of the snapshot its handlers read — unordered, \
             this answered \"no pet UI\" and the Pet tab stayed down for the session"
        );
    }
}
