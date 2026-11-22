//! The player's spellbook: the book update (`0x53ca70`) builds the sorted slots and tabs from the
//! known spells and the `SpellCatalog` × `SkillLineCatalog` join, and the verbs read it through
//! the live view. A known spell is booked when
//! [`SpellDisplay::in_spellbook`](benilla_formats::SpellDisplay::in_spellbook) passes (`0x542030`);
//! a companion, a mount or an `AttributesEx4 & 0x8000` spell is named but listed in no tab.
//!
//! A spell's tab is its skill line, or General (key 0) when the `SkillRaceClassInfo.dbc` lookup
//! finds no row for the player's race and class, or one with `SKILL_FLAG_DISPLAY_SORTED` (`0x80`).
//! General comes first, then tabs by `SkillLine.dbc` name (byte order standing in for the enUS
//! collator); within a tab, name then parsed rank.
//!
//! Not modelled: which of several admitting `SkillRaceClassInfo` rows picks (the first wins here),
//! and the `SpellLevel` fallback for a digit-less rank (the rank string breaks ties).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use bevy::prelude::*;

use benilla_formats::{SkillLineCatalog, SpellCatalog, SummonPropertiesCatalog};
use benilla_ui::script::{PlayerBook, ScriptValue, SpellBookState, SpellSlotView, SpellTabView};

use crate::ui_action::Spells;
use crate::ui_script::live::UiEvents;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};

/// `SkillLine.dbc` × `SkillLineAbility.dbc`; absent when the data failed to load.
#[derive(Resource)]
pub(crate) struct SkillLines {
    /// Shared with `GetSpellInfo`'s source.
    pub(crate) catalog: std::sync::Arc<SkillLineCatalog>,
}

/// The General tab, key 0: pinned first, named from the `GENERAL` GlobalString with a fixed icon
/// (`GetSpellTabInfo`); vmangos's `SKILL_NONE` is 0, so no line collides.
const NO_LINE: u32 = 0;

/// Hardcoded in the reference, extensionless for the loader; key 0 has no DBC row.
const GENERAL_TAB_ICON: &str = "Interface\\Icons\\Ability_Kick";

/// The spells a handler's learn reached the book update with, for `LEARNED_SPELL_IN_TAB`, which
/// follows the update's `SPELLS_CHANGED` (`0x542cd4`, `0x542d16`). The `SMSG_INITIAL_SPELLS` load
/// and a learn before our object exists pass the registrar with its live flag clear and reach
/// neither.
#[derive(Default)]
pub(crate) struct LearnedInTab(pub(crate) Vec<u32>);

/// The player's book as the last book update built it (`0x53ca70`), which the verbs read.
#[derive(Resource, Default)]
pub(crate) struct SpellBook {
    pub(crate) live: Arc<PlayerBook>,
    /// The skill line behind each tab, in order, which `LEARNED_SPELL_IN_TAB` indexes.
    tab_lines: Vec<u32>,
    /// Our race and class, from our object's init on, which the General collapse reads.
    race_class: (u8, u8),
}

/// What a book update reads besides the book.
pub(crate) struct BookInputs<'a> {
    pub(crate) known: &'a BTreeSet<u32>,
    pub(crate) spells: Option<&'a Spells>,
    pub(crate) summons: &'a SummonPropertiesCatalog,
    pub(crate) skill_lines: Option<&'a SkillLineCatalog>,
}

impl SpellBook {
    /// The book update (`0x53ca70`) with its re-sort and its school-mask rebuild, as the learn,
    /// the removal and our object's init run it: the book rebuilt, the masks (`0x53cd48`), then
    /// `SPELLS_CHANGED` (`0x53ceae`).
    pub(crate) fn update(
        &mut self,
        inputs: &BookInputs,
        schools: &mut crate::ui_cast::InterruptSchools,
        ui: &mut UiEvents,
    ) {
        if let Some(spells) = inputs.spells {
            let (race, class) = self.race_class;
            let (book, tab_lines) = build_book(
                inputs.known,
                &spells.catalog,
                inputs.summons,
                inputs.skill_lines,
                race,
                class,
            );
            let tracking = tracking_of(&book, &spells.catalog);
            debug!(
                "ui_spellbook: book update, {} tab(s), {} spell(s)",
                book.tabs.len(),
                book.slots.len()
            );
            self.live = Arc::new(PlayerBook::new(book, tracking));
            self.tab_lines = tab_lines;
        }
        crate::ui_cast::book_updated(schools, inputs.known, inputs.spells);
        ui.fire("SPELLS_CHANGED", Vec::new());
    }

    /// `LEARNED_SPELL_IN_TAB` with the 1-based index of each learned spell's tab in the book the
    /// update just built (`0x542d16`), which `SpellBookFrame.lua` flashes.
    pub(crate) fn flash(&self, learned: &LearnedInTab, inputs: &BookInputs, ui: &mut UiEvents) {
        let Some(spells) = inputs.spells else {
            return;
        };
        let (race, class) = self.race_class;
        for &spell_id in &learned.0 {
            let Some(index) = tab_flash_index(
                spell_id,
                &self.tab_lines,
                spells,
                inputs.summons,
                inputs.skill_lines,
                race,
                class,
            ) else {
                continue;
            };
            debug!("ui_spellbook: spell {spell_id} landed in tab {index} — flash");
            ui.fire("LEARNED_SPELL_IN_TAB", vec![ScriptValue::Int(index)]);
        }
    }

    /// A book built from `book`, for a test.
    #[cfg(test)]
    pub(crate) fn of(book: SpellBookState) -> Self {
        Self {
            live: Arc::new(PlayerBook::new(book, Vec::new())),
            ..Default::default()
        }
    }

    /// Our object's race and class, set by its init before its book update (`0x6e7f50`).
    pub(crate) fn set_race_class(&mut self, race: u8, class: u8) {
        self.race_class = (race, class);
    }
}

/// The book's known tracking spells in display order (`0x53cab5`, filled by `0x7fdf60`).
fn tracking_of(
    book: &SpellBookState,
    catalog: &SpellCatalog,
) -> Vec<benilla_ui::script::TrackingSpell> {
    book.named()
        .filter_map(|slot| {
            let d = catalog.get(slot.spell_id)?;
            d.tracking_aura()
                .then(|| benilla_ui::script::TrackingSpell {
                    spell_id: slot.spell_id,
                    name: d.name.clone(),
                    icon: d.icon.clone(),
                    active_icon: d.active_icon.clone(),
                })
        })
        .collect()
}

/// A book spell's texture at the call (`0x53dc20`): the melee auto-attack's face alone
/// (`Effect[0] == SPELL_EFFECT_ATTACK`, `0x53dc8c`), nil with no player; a ranged shot's
/// (`Attributes & 0x2` and `AttributesEx2 & 0x20`, `0x53dcb7`) while there is a ranged weapon;
/// else the record's icon.
pub(crate) fn book_texture(
    d: &benilla_formats::SpellDisplay,
    attack_icon: impl FnOnce() -> Option<String>,
    ranged_icon: impl FnOnce() -> Option<String>,
) -> Option<String> {
    if d.is_melee_auto_attack() {
        return attack_icon();
    }
    if d.ranged_icon_substitution() {
        if let Some(icon) = ranged_icon() {
            return Some(icon);
        }
    }
    d.icon.clone()
}

pub(crate) struct UiSpellbookPlugin;

impl Plugin for UiSpellbookPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpellBook>()
            .add_systems(Startup, load_skill_lines.after(AssetSet::Open));
    }
}

fn load_skill_lines(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_skill_line_catalog(&mut chain)
    };
    match loaded {
        Ok(catalog) => {
            info!(
                "ui_spellbook: {} skill line(s) in the tab catalog",
                catalog.len()
            );
            commands.insert_resource(SkillLines {
                catalog: catalog.into(),
            });
        }
        Err(e) => warn!(
            "ui_spellbook: SkillLine.dbc failed to load — every spell falls into one \
             fallback tab: {e:#}"
        ),
    }
}

/// The 1-based index of `spell_id`'s tab in `tab_lines`; `None` for an unbooked spell.
fn tab_flash_index(
    spell_id: u32,
    tab_lines: &[u32],
    spells: &Spells,
    summons: &SummonPropertiesCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    race: u8,
    class: u8,
) -> Option<i64> {
    if !spells
        .catalog
        .get(spell_id)
        .is_some_and(|d| d.in_spellbook(summons))
    {
        return None;
    }
    let tab = skill_lines.map_or(NO_LINE, |c| c.spell_tab(spell_id, race, class));
    let index = tab_lines.iter().position(|&l| l == tab)?;
    Some(index as i64 + 1)
}

/// Build the book, and the skill-line id behind each published tab in order, for
/// [`LearnedInTab`]'s index: one tab ordering, never recomputed.
fn build_book(
    known: &BTreeSet<u32>,
    catalog: &SpellCatalog,
    summons: &SummonPropertiesCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    race: u8,
    class: u8,
) -> (SpellBookState, Vec<u32>) {
    let mut by_line: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut unlisted = Vec::new();
    for &spell_id in known {
        // The add-gate (module doc); a spell missing from `Spell.dbc` is dropped too.
        let Some(d) = catalog.get(spell_id).filter(|d| d.in_name_cache()) else {
            continue;
        };
        if !d.in_spellbook(summons) {
            unlisted.push(SpellSlotView {
                spell_id,
                name: d.name.clone(),
                rank: d.rank.clone(),
                texture: d.icon.clone(),
                passive: d.passive,
                ..SpellSlotView::default()
            });
            continue;
        }
        // The tab after the General collapse; with no skill-line catalog, General.
        let tab = skill_lines
            .map(|c| c.spell_tab(spell_id, race, class))
            .unwrap_or(NO_LINE);
        by_line.entry(tab).or_default().push(spell_id);
    }

    // General first with its fixed name and icon, then by `SkillLine.dbc` name; an unresolved
    // line reads "General" with no icon.
    let mut lines: Vec<(u32, String, Option<String>, Vec<u32>)> = by_line
        .into_iter()
        .map(|(line_id, spell_ids)| {
            let (name, texture) = if line_id == NO_LINE {
                ("General".to_string(), Some(GENERAL_TAB_ICON.to_string()))
            } else {
                match skill_lines.and_then(|c| c.line(line_id)) {
                    Some(info) => (info.name.clone(), info.icon.clone()),
                    None => ("General".to_string(), None),
                }
            };
            (line_id, name, texture, spell_ids)
        })
        .collect();
    lines.sort_by(|a, b| (a.0 != NO_LINE).cmp(&(b.0 != NO_LINE)).then(a.1.cmp(&b.1)));

    let mut tabs = Vec::with_capacity(lines.len());
    let mut tab_lines = Vec::with_capacity(lines.len());
    let mut slots = Vec::new();
    for (line_id, name, texture, mut spell_ids) in lines {
        tab_lines.push(line_id);
        spell_ids.sort_by_key(|&a| spell_sort_key(catalog, a));
        let offset = slots.len() as u32;
        let num_spells = spell_ids.len() as u32;
        for spell_id in spell_ids {
            let d = catalog.get(spell_id);
            // The record's icon; the weapon faces are swapped in at the call ([`book_texture`]).
            let texture = d.and_then(|d| d.icon.clone());
            slots.push(SpellSlotView {
                spell_id,
                name: d.map(|d| d.name.clone()).unwrap_or_default(),
                rank: d.and_then(|d| d.rank.clone()),
                texture,
                passive: d.is_some_and(|d| d.passive),
                // Read at the call (`0x53d350`, `0x809000`).
                current: false,
                cooldown: None,
                // `GetSpellAutocast` answers (nil, nil) for the player book, and a known spell has
                // no packed word.
                autocast: None,
                packed: 0,
            });
        }
        tabs.push(SpellTabView {
            name,
            texture,
            offset,
            num_spells,
        });
    }
    (
        SpellBookState {
            tabs,
            slots,
            unlisted,
        },
        tab_lines,
    )
}

/// The reference's book sort's tail: name, then the parsed rank number, then the rank string. The
/// rank is the first digit run of `NameSubtext`, so any localized prefix parses.
fn spell_sort_key(catalog: &SpellCatalog, spell_id: u32) -> (String, u32, String) {
    let d = catalog.get(spell_id);
    spell_sort_key_of(
        d.map(|d| d.name.as_str()).unwrap_or_default(),
        d.and_then(|d| d.rank.as_deref()),
    )
}

/// [`spell_sort_key`] over a resolved `(name, rank)`, shared with the pet book: the reference sorts
/// both books with the same comparison.
pub(crate) fn spell_sort_key_of(name: &str, rank: Option<&str>) -> (String, u32, String) {
    let rank_str = rank.unwrap_or_default().to_string();
    (name.to_string(), leading_number(&rank_str), rank_str)
}

/// The first ASCII digit run as a number, else 0: the reference's rank parse.
fn leading_number(s: &str) -> u32 {
    s.chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .fold(None::<u32>, |acc, c| {
            Some(acc.unwrap_or(0) * 10 + c.to_digit(10).unwrap())
        })
        .unwrap_or(0)
}

/// `CastSpell`, `CastSpellByName` and `CastSpellByID`, through the cast tail the action bar uses,
/// at the unit the verb named, else the selection as the calls before it left it. `via_book`: the
/// book's dispatcher, with its cancel forks; a `!name` or an id goes past them.
pub(crate) fn cast_spell(
    cast: &mut crate::spell::ScriptCast,
    spell_id: u32,
    at: benilla_ui::script::CastAt,
    via_book: bool,
    vm: &benilla_ui::script::UiScript,
) {
    let crate::spell::ScriptCast {
        targeting,
        ladder,
        tokens,
    } = cast;
    // `CastSpell`'s two cancel forks, in order: the active-spell test, then the form match,
    // which `UseAction` lacks, with its silent no-op.
    let forks = via_book.then(|| (ladder.spells.as_ref(), targeting.self_store.iter().next()));
    if let Some((Some(sp), Some(store))) = forks {
        if let Some(d) = sp.catalog.get(spell_id) {
            let auras = targeting.self_auras.iter().next();
            if ladder.is_active_spell(spell_id, Some(store), auras) {
                if ladder.keeps_fresh_toggle(spell_id) {
                    debug!("ui_spellbook: cast {spell_id} re-pressed — just cast, kept");
                    return;
                }
                debug!("ui_spellbook: cast {spell_id} re-pressed — cancels");
                ladder.cancel_active_spell(spell_id, Some(store), auras);
                return;
            }
            let form = store.0.unit_shapeshift_form();
            let row = sp.forms.get(&u32::from(form));
            match crate::ui_action::toggle::form_recast_disposition(d, form, row) {
                Some(true) if ladder.keeps_fresh_toggle(spell_id) => {
                    debug!("ui_spellbook: cast {spell_id} — the form was just cast, kept");
                    return;
                }
                Some(true) => {
                    debug!("ui_spellbook: cast {spell_id} — the active form cancels");
                    ladder.cancel_aura(spell_id);
                    return;
                }
                Some(false) => {
                    debug!("ui_spellbook: cast {spell_id} — non-cancelable form, silent no-op");
                    return;
                }
                None => {}
            }
        }
    }
    debug!(
        "ui_spellbook: cast {spell_id} at {at:?} (selection {:?})",
        targeting.selected().1
    );
    let aim = targeting.aim(tokens, &at, crate::ui_unit::VmUnits::of(vm));
    ladder.send_spell(spell_id, &targeting.context_at(aim), aim.passed());
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::SpellDisplay;
    use std::collections::HashMap;

    #[test]
    fn leading_number_scans_the_first_digit_run() {
        assert_eq!(leading_number("Rank 1"), 1);
        assert_eq!(leading_number("Rank 10"), 10);
        assert_eq!(leading_number("Rank 2"), 2);
        assert_eq!(leading_number(""), 0);
        assert_eq!(leading_number("Racial"), 0); // digit-less: the reference uses SpellLevel
        assert_eq!(leading_number("Apprentice"), 0);
    }

    /// A minimal `SpellDisplay`: `attributes` drives the gate, `rank` the sort.
    fn spell(name: &str, rank: Option<&str>, attributes: u32) -> SpellDisplay {
        SpellDisplay {
            name: name.into(),
            rank: rank.map(Into::into),
            attributes,
            passive: attributes & 0x40 != 0,
            ..Default::default()
        }
    }

    /// A mount: aura 0 is `MOUNTED`, which `0x5426a5` files in the mount list, not a tab.
    fn mount(name: &str) -> SpellDisplay {
        SpellDisplay {
            effect_apply_aura: [benilla_formats::SPELL_AURA_MOUNTED, 0, 0],
            ..spell(name, None, 0)
        }
    }

    /// `AttributesEx4 & 0x8000`: known and named, filed in no tab (`0x542856`).
    fn unbooked(name: &str) -> SpellDisplay {
        SpellDisplay {
            attributes_ex4: 0x8000,
            ..spell(name, None, 0)
        }
    }

    /// With no skill-line catalog every booked spell is in General, index 1.
    #[test]
    fn the_flash_index_is_one_based_into_the_published_tab_strip() {
        let mut map = HashMap::new();
        map.insert(133, spell("Fireball", Some("Rank 2"), 0x10000));
        map.insert(668, spell("Language: Common", None, 0x80)); // DO_NOT_DISPLAY
        map.insert(818, spell("Cooking", None, 0x20)); // IS_TRADESKILL
        map.insert(458, mount("Brown Horse"));
        map.insert(1234, unbooked("Unbooked"));
        let catalog = SpellCatalog::from_displays(map);
        let known: BTreeSet<u32> = [133, 668, 818, 458, 1234].into_iter().collect();
        let (book, tab_lines) = build_book(
            &known,
            &catalog,
            &SummonPropertiesCatalog::default(),
            None,
            1,
            1,
        );

        assert_eq!(
            tab_lines.len(),
            book.tabs.len(),
            "one line id per published tab"
        );
        assert_eq!(tab_lines, vec![NO_LINE], "General is the only tab here");

        let spells = Spells {
            catalog,
            ..Spells::empty_for_tests()
        };
        assert_eq!(
            tab_flash_index(
                133,
                &tab_lines,
                &spells,
                &SummonPropertiesCatalog::default(),
                None,
                1,
                1
            ),
            Some(1),
            "a booked spell flashes its tab, 1-based"
        );

        // The registrar's returns before `LEARNED_SPELL_IN_TAB`, the book add-gate.
        for (id, why) in [
            (668u32, "DO_NOT_DISPLAY returns at 0x54265c"),
            (818, "IS_TRADESKILL returns before the name cache"),
            (458, "a mount takes the companion exit at 0x5426a5"),
            (1234, "AttributesEx4 0x8000 returns at 0x542856"),
        ] {
            assert_eq!(
                tab_flash_index(
                    id,
                    &tab_lines,
                    &spells,
                    &SummonPropertiesCatalog::default(),
                    None,
                    1,
                    1
                ),
                None,
                "{why}"
            );
        }

        assert_eq!(
            tab_flash_index(
                99999,
                &tab_lines,
                &spells,
                &SummonPropertiesCatalog::default(),
                None,
                1,
                1
            ),
            None,
            "an id with no Spell.dbc record flashes nothing"
        );
    }

    /// A mount and an `AttributesEx4 & 0x8000` spell are named but in no tab (`0x542699` before
    /// the exits): `unlisted`, which `CastSpellByName` finds and no book index reaches. A
    /// `0x80`/`0x20` spell is neither.
    #[test]
    fn companions_and_unbooked_spells_are_named_but_listed_in_no_tab() {
        let mut map = HashMap::new();
        map.insert(133, spell("Fireball", Some("Rank 1"), 0));
        map.insert(458, mount("Brown Horse"));
        map.insert(1234, unbooked("Unbooked"));
        map.insert(668, spell("Language: Common", None, 0x80));
        map.insert(818, spell("Cooking", None, 0x20));
        let catalog = SpellCatalog::from_displays(map);
        let known: BTreeSet<u32> = [133, 458, 1234, 668, 818].into_iter().collect();
        let (book, _) = build_book(
            &known,
            &catalog,
            &SummonPropertiesCatalog::default(),
            None,
            1,
            1,
        );
        let ids = |slots: &[SpellSlotView]| slots.iter().map(|s| s.spell_id).collect::<Vec<_>>();
        assert_eq!(ids(&book.slots), [133]);
        assert_eq!(book.tabs[0].num_spells, 1);
        assert_eq!(ids(&book.unlisted), [458, 1234]);
        assert_eq!(book.unlisted[0].name, "Brown Horse");
    }

    /// Shown spells sort name then rank, and General takes the fixed name and icon.
    #[test]
    fn build_book_gates_hidden_spells_and_pins_general() {
        let mut map = HashMap::new();
        map.insert(133, spell("Fireball", Some("Rank 2"), 0x10000));
        map.insert(145, spell("Fireball", Some("Rank 1"), 0x10000)); // out-of-order rank
        map.insert(2136, spell("Fire Blast", Some("Rank 1"), 0x0));
        map.insert(668, spell("Language: Common", None, 0x80)); // DO_NOT_DISPLAY
        map.insert(818, spell("Cooking", None, 0x20)); // IS_TRADESKILL
        let catalog = SpellCatalog::from_displays(map);
        let known: BTreeSet<u32> = [133, 145, 2136, 668, 818].into_iter().collect();

        // No skill-line catalog: every shown spell lands in General.
        let book = build_book(
            &known,
            &catalog,
            &SummonPropertiesCatalog::default(),
            None,
            1,
            1,
        )
        .0;

        assert_eq!(book.tabs.len(), 1);
        assert_eq!(book.tabs[0].name, "General");
        assert_eq!(book.tabs[0].texture.as_deref(), Some(GENERAL_TAB_ICON));
        assert_eq!(book.tabs[0].num_spells, 3);
        let order: Vec<(&str, Option<&str>)> = book
            .slots
            .iter()
            .map(|s| (s.name.as_str(), s.rank.as_deref()))
            .collect();
        assert_eq!(
            order,
            vec![
                ("Fire Blast", Some("Rank 1")),
                ("Fireball", Some("Rank 1")),
                ("Fireball", Some("Rank 2")),
            ]
        );
    }

    /// Keyed on `Effect[0] == SPELL_EFFECT_ATTACK`, not the id: the resolved face stands in for the
    /// record's `Temp`, and with no player there is none (`0x53dc8c`).
    #[test]
    fn the_attack_shows_the_resolved_face_by_effect() {
        let mut attack = spell("Attack", None, 0x10);
        attack.icon = Some("Interface\\Icons\\Temp".into()); // the real DBC placeholder
        attack.effects[0] = 78; // SPELL_EFFECT_ATTACK
        for resolved in [
            "Interface\\Icons\\INV_Sword_04",
            "Interface\\Buttons\\Spell-Reset",
        ] {
            let t = book_texture(&attack, || Some(resolved.into()), || None);
            assert_eq!(t.as_deref(), Some(resolved));
        }
        assert_eq!(
            book_texture(&attack, || None, || None),
            None,
            "no player answers no texture"
        );
    }

    /// Both bits (`Attributes & 0x2`, `AttributesEx2 & 0x20`) substitute; Throw's `0x2` alone does
    /// not, and without a ranged weapon the spell keeps its own icon (`0x53dcb7`).
    #[test]
    fn ranged_shots_borrow_the_ranged_weapon_face() {
        let mut auto_shot = spell("Auto Shot", None, 0x2);
        auto_shot.icon = Some("Interface\\Icons\\Ability_AutoShot".into());
        auto_shot.attributes_ex2 = 0x20;
        let mut throw = spell("Throw", None, 0x2);
        throw.icon = Some("Interface\\Icons\\Ability_Throw".into());
        let bow = "Interface\\Icons\\INV_Weapon_Bow_02";
        let with_bow = |d| book_texture(d, || None, || Some(bow.into()));
        assert_eq!(with_bow(&auto_shot).as_deref(), Some(bow));
        assert_eq!(
            with_bow(&throw).as_deref(),
            Some("Interface\\Icons\\Ability_Throw"),
            "ranged-slot alone (no auto-repeat bit) keeps the spell icon"
        );
        assert_eq!(
            book_texture(&auto_shot, || None, || None).as_deref(),
            Some("Interface\\Icons\\Ability_AutoShot")
        );
    }
}
