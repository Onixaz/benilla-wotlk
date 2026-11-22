//! The companion lists behind `GetNumCompanions` (`0x53c2a0`), `GetCompanionInfo` (`0x53e2c0`), `GetCompanionCooldown` (`0x53e490`), `CallCompanion` (`0x53e5e0`), `DismissCompanion` (`0x53e6b0`) and `SummonRandomCritter` (`0x53e7e0`), filled by the spell handlers' known-spell add (`0x5426a5`) with spell ids the verbs resolve when called and sorted by creature name once no record is pending (`0x53cec0`); each event fires from the handler that causes it, and the active test's own-vehicle arm (`0x802cf3`) is #1070's.

use bevy::prelude::*;

use benilla_formats::{CompanionKind, SpellCatalog, SummonPropertiesCatalog, SPELL_AURA_MOUNTED};
use benilla_protocol::ObjectFields;
use benilla_ui::script::{CompanionIntent, CompanionType, CompanionView, ScriptValue};

use crate::names::{NameCache, NameKey};
use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::spell::SummonSlots;
use crate::ui_action::{Spells, UiError};
use crate::ui_script::live::UiEvents;
use crate::ui_script::UiDrain;
use crate::unit_auras::UnitAuras;

/// `SkillLine.dbc` "Companion Pets": `SummonRandomCritter`'s pool (`0x6e7bc4`).
pub(crate) const SKILL_LINE_COMPANIONS: u32 = 778;

pub(crate) struct UiCompanionPlugin;

impl Plugin for UiCompanionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CompanionLists>()
            .init_resource::<KnownSpellBuffer>()
            .add_systems(
                Update,
                (
                    crate::ui_script::raises::then_fire(drain_companions).in_set(UiDrain),
                    // After the VM ticks: a click's `SetCreature` lands in `UiPress`, before it. Before the display build that loads its model and the booth that draws it.
                    feed_companion_booth
                        .in_set(UiDrain)
                        .in_set(crate::portrait::CreatureBoothFeed)
                        .before(crate::entities::DisplayBuildSet),
                ),
            );
        net::register(app);
    }
}

/// The companion lists (`0xbe8df4`), their pending records and the random pool (`0x6e7bc4`).
#[derive(Resource, Default)]
pub(crate) struct CompanionLists {
    /// Critters, then mounts: spell ids in add order until sorted.
    lists: [Vec<u32>; 2],
    /// One `(creature, notify)` per add whose record was asked: the callback's count and argument (`0x53ebc0`).
    pending: Vec<(u32, bool)>,
    /// The known spells of skill line 778, in add order.
    random_pool: Vec<u32>,
}

/// A list's slot in [`CompanionLists`], the type parse's order (`0x53b600`).
fn slot_of(kind: CompanionKind) -> usize {
    match kind {
        CompanionKind::Critter => 0,
        CompanionKind::Mount => 1,
    }
}

fn type_slot(ty: CompanionType) -> usize {
    match ty {
        CompanionType::Critter => 0,
        CompanionType::Mount => 1,
    }
}

impl CompanionLists {
    /// List `ty`'s spells in order.
    pub(crate) fn list(&self, ty: CompanionType) -> &[u32] {
        &self.lists[type_slot(ty)]
    }

    /// The random-critter pool.
    pub(crate) fn random_pool(&self) -> &[u32] {
        &self.random_pool
    }

    /// The pool's add for a known spell of skill line 778 (`0x6e7bc4`).
    fn pool(&mut self, spell_id: u32, line: Option<u32>) {
        if line == Some(SKILL_LINE_COMPANIONS) && !self.random_pool.contains(&spell_id) {
            self.random_pool.push(spell_id);
        }
    }

    /// The companion list update (`0x53cec0`): with no record pending each non-empty list sorts by its creature's name (`0x53c7c0`); whether one sorted, which fires `COMPANION_UPDATE`.
    fn update(&mut self, name_of: &dyn Fn(u32) -> String) -> bool {
        if !self.pending.is_empty() {
            return false;
        }
        let mut sorted = false;
        for list in &mut self.lists {
            if list.is_empty() {
                continue;
            }
            list.sort_by_cached_key(|&spell_id| (name_of(spell_id).to_lowercase(), spell_id));
            sorted = true;
        }
        sorted
    }
}

/// What one add, unlearn or answer reads: the catalogs, the creature cache and the queue the calling handler fires into.
pub(crate) struct CompanionCx<'a> {
    pub(crate) catalog: &'a SpellCatalog,
    pub(crate) summons: &'a SummonPropertiesCatalog,
    pub(crate) names: &'a NameCache,
    pub(crate) commands: &'a NetCommands,
    pub(crate) ui: &'a mut UiEvents,
}

impl CompanionCx<'_> {
    /// A listed spell's creature name for the sort, `answered` standing in for the record the calling handler carries.
    fn sort_name(&self, spell_id: u32, answered: Option<(u32, &str)>) -> String {
        let creature = self
            .catalog
            .get(spell_id)
            .map_or(0, |d| d.companion_creature());
        match answered {
            Some((entry, name)) if entry == creature => name.to_string(),
            _ => self
                .names
                .creature_record(creature)
                .map(|r| r.name.clone())
                .unwrap_or_default(),
        }
    }

    /// The update and its `COMPANION_UPDATE`.
    fn update(&mut self, lists: &mut CompanionLists, answered: Option<(u32, &str)>) {
        let sorted = {
            let names = |spell_id| self.sort_name(spell_id, answered);
            lists.update(&names)
        };
        if sorted {
            self.ui.fire("COMPANION_UPDATE", Vec::new());
        }
    }

    /// The learn line, shown in turn (`0x5427be`).
    fn learn_line(&mut self, name: &str) {
        crate::ui_action::show_in_turn(
            self.ui,
            "ui_companion",
            UiError::s("ERR_LEARN_COMPANION_S", name.to_string()),
        );
    }
}

/// The known-spell add's companion arm (`0x5426a5`): the spell joins its list and its creature's record is asked; a live learn (`notify`) with the record cached shows the learn line, updates and fires `COMPANION_LEARNED` (`0x5427be`), and a miss waits on the answer.
pub(crate) fn add_known(
    lists: &mut CompanionLists,
    spell_id: u32,
    notify: bool,
    line: Option<u32>,
    cx: &mut CompanionCx,
) -> Option<u32> {
    lists.pool(spell_id, line);
    let d = cx.catalog.get(spell_id).filter(|d| d.in_name_cache())?;
    let kind = d.companion_kind(cx.summons)?;
    lists.lists[slot_of(kind)].push(spell_id);
    let creature = d.companion_creature();
    match cx.names.answer(NameKey::Creature(creature), 0, cx.commands) {
        None => lists.pending.push((creature, notify)),
        Some(name) if notify => {
            let name = name.unwrap_or_default().to_string();
            cx.learn_line(&name);
            cx.update(lists, None);
            cx.ui.fire("COMPANION_LEARNED", Vec::new());
        }
        Some(_) => {}
    }
    Some(tutorial_of(kind))
}

/// The tutorial an add to `kind`'s list triggers, before its record is asked (`0x542751`, `0x54275a`).
fn tutorial_of(kind: CompanionKind) -> u32 {
    match kind {
        CompanionKind::Critter => crate::tutorial::id::COMPANION_CRITTER,
        CompanionKind::Mount => crate::tutorial::id::COMPANION_MOUNT,
    }
}

/// The known-spell buffer the active player's init replays (`0xc9eb3c`): `SMSG_INITIAL_SPELLS` sets it (`0x6df0c0`), a learn, removal or supersede before our object exists edits it (`0x6e7de0`, `0x6e7888`, `0x6e7ef4`), and leaving the world empties it (`0x4065c3`).
#[derive(Resource, Default)]
pub(crate) struct KnownSpellBuffer(pub(crate) Vec<u32>);

/// No summon rows, before the catalog loads.
pub(crate) static EMPTY_SUMMONS: std::sync::LazyLock<benilla_formats::SummonPropertiesCatalog> =
    std::sync::LazyLock::new(Default::default);

/// What the active player's init reads to replay the buffer into the lists.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CompanionInit<'w> {
    buffer: Res<'w, KnownSpellBuffer>,
    lists: ResMut<'w, CompanionLists>,
    spells: Option<Res<'w, Spells>>,
    summons: Option<Res<'w, SummonSlots>>,
    names: Option<Res<'w, NameCache>>,
    commands: Option<Res<'w, NetCommands>>,
    skill_lines: Option<Res<'w, crate::ui_spellbook::SkillLines>>,
    ui: ResMut<'w, UiEvents>,
    tutorials: Option<ResMut<'w, Messages<crate::tutorial::TutorialEvent>>>,
    /// The book its update rebuilds between the adds and the companion update (`0x6e7fdb`).
    book: ResMut<'w, crate::ui_spellbook::SpellBook>,
    schools: ResMut<'w, crate::ui_cast::InterruptSchools>,
    actions: Res<'w, crate::ui_action::PlayerActions>,
}

/// The active player's init (`0x6e7f50`), run as our object is created with its race, class and form byte: the lists cleared (`0x6e7f9d`), each buffered spell added without a line, the stance list among them, the book update with its re-sort and masks (`0x6e7fdb`), then one companion update (`0x6e7fe3`); shapeshifted, the usable update (`0x6e8059`), the selection update (`0x6e805e`), the form's preset bar and the bonus update (`0x6e8063`, `0x6e8068`), then `UPDATE_SHAPESHIFT_FORM` (`0x6e8074`).
pub(crate) fn active_player_init(In((race, class, form)): In<(u8, u8, u8)>, init: CompanionInit) {
    let CompanionInit {
        buffer,
        mut lists,
        spells,
        summons,
        names,
        commands,
        skill_lines,
        mut ui,
        mut tutorials,
        mut book,
        mut schools,
        actions,
    } = init;
    book.set_race_class(race, class);
    book.clear_registered();
    {
        use crate::ui_action::usable_cache::{edit_step, ListEdit};
        ui.world(edit_step(ListEdit::Rebuild));
    }
    // Shapeshifted only (`0x6e8050`-`0x6e8057`): a form row that is no stance; a form the table
    // lacks is none.
    let shifted = spells
        .as_deref()
        .and_then(|s| s.forms.get(&u32::from(form)))
        .is_some_and(|row| crate::spell::validator::is_shapeshifted(form, row.flags));
    // The stored action table goes on the bar (`0x6e8000`-`0x6e8046`), then the shapeshifted tail.
    let tail = move |ui: &mut UiEvents| {
        ui.world(crate::spell::net::put_action_table);
        if shifted {
            ui.world(crate::ui_action::usable_cache::step(
                crate::ui_action::usable_cache::Pass::Full,
            ));
            crate::ui_action::signals::selection(ui);
            // The form's preset bar and the bonus update (`0x6e8063`, `0x6e8068`).
            ui.world(crate::ui_action::bonus::form_changed);
            ui.fire("UPDATE_SHAPESHIFT_FORM", Vec::new());
        }
    };
    let mut update_book = |ui: &mut UiEvents| {
        let inputs = crate::ui_spellbook::BookInputs {
            known: &actions.spells,
            spells: spells.as_deref(),
            summons: summons.as_deref().map_or(&EMPTY_SUMMONS, |s| &s.0),
            skill_lines: skill_lines.as_deref().map(|s| &*s.catalog),
        };
        // Our object's init passes its first argument set (`0x6e7fd5`).
        book.update(&inputs, &mut schools, ui, true);
    };
    let (Some(spells), Some(summons), Some(names), Some(commands)) = (
        spells.as_deref(),
        summons.as_deref(),
        names.as_deref(),
        commands.as_deref(),
    ) else {
        *lists = CompanionLists::default();
        update_book(&mut ui);
        tail(&mut ui);
        return;
    };
    let line_of = |id: u32| {
        skill_lines
            .as_deref()
            .and_then(|s| s.catalog.spell_to_line(id, race, class))
    };
    let known: Vec<_> = buffer.0.iter().map(|&id| (id, line_of(id))).collect();
    let mut cx = CompanionCx {
        catalog: &spells.catalog,
        summons: &summons.0,
        names,
        commands,
        ui: &mut ui,
    };
    let tutorial_ids = add_initial(&mut lists, known, &mut cx, update_book);
    tail(&mut ui);
    for id in tutorial_ids {
        if let Some(t) = tutorials.as_mut() {
            t.write(crate::tutorial::TutorialEvent::trigger(id));
        }
    }
}

/// The init's adds: the lists cleared (`0x6e7f9d`), each known spell added without a line, then one update.
pub(crate) fn add_initial(
    lists: &mut CompanionLists,
    known: impl IntoIterator<Item = (u32, Option<u32>)>,
    cx: &mut CompanionCx,
    before_update: impl FnOnce(&mut UiEvents),
) -> Vec<u32> {
    *lists = CompanionLists::default();
    let tutorials = known
        .into_iter()
        .filter_map(|(spell_id, line)| add_known(lists, spell_id, false, line, cx))
        .collect();
    before_update(cx.ui);
    cx.update(lists, None);
    tutorials
}

/// The known-spell removal's companion arm (`0x53fad0`): a listed spell leaves its list, then the update and `COMPANION_UNLEARNED`.
pub(crate) fn remove_known(lists: &mut CompanionLists, spell_id: u32, cx: &mut CompanionCx) {
    lists.random_pool.retain(|&s| s != spell_id);
    let Some(kind) = cx
        .catalog
        .get(spell_id)
        .and_then(|d| d.companion_kind(cx.summons))
    else {
        return;
    };
    let list = &mut lists.lists[slot_of(kind)];
    let Some(at) = list.iter().position(|&s| s == spell_id) else {
        return;
    };
    list.remove(at);
    cx.update(lists, None);
    cx.ui.fire("COMPANION_UNLEARNED", Vec::new());
}

/// The creature-record callback (`0x53ebc0`), once per add that waited on `entry`: a live learn's answered record shows the learn line, updates and fires `COMPANION_LEARNED`; otherwise the last answer updates.
pub(crate) fn record_answered(
    lists: &mut CompanionLists,
    entry: u32,
    name: Option<&str>,
    cx: &mut CompanionCx,
) {
    while let Some(at) = lists.pending.iter().position(|&(c, _)| c == entry) {
        let (_, notify) = lists.pending.remove(at);
        let answered = name.map(|n| (entry, n));
        match name {
            Some(name) if notify => {
                cx.learn_line(name);
                // The update, `COMPANION_LEARNED` and the later callbacks after the line's listeners (`0x5427be`).
                let name = name.to_string();
                cx.ui
                    .world(move |world| learned_after_line(world, entry, name));
                return;
            }
            _ if lists.pending.is_empty() => cx.update(lists, answered),
            _ => {}
        }
    }
}

/// A learn line's tail in the creature-record callback (`0x53ebc0`): the update and `COMPANION_LEARNED`, then the callbacks still filed on `entry` as a step after its listeners (`0x53ec20` returns before the next callback).
fn learned_after_line(world: &mut World, entry: u32, name: String) {
    with_cx(
        world,
        entry,
        name,
        "a companion learn's tail",
        |lists, entry, name, cx| {
            cx.update(lists, Some((entry, name)));
            cx.ui.fire("COMPANION_LEARNED", Vec::new());
            let name = name.to_string();
            cx.ui
                .world(move |world| later_callbacks(world, entry, name));
        },
    );
}

/// The callbacks still filed on `entry` once a learn's signal has run (`0x53ebc0`).
fn later_callbacks(world: &mut World, entry: u32, name: String) {
    with_cx(
        world,
        entry,
        name,
        "a companion record's later callbacks",
        |lists, entry, name, cx| {
            record_answered(lists, entry, Some(name), cx);
        },
    );
}

/// A step over the lists and a [`CompanionCx`], with the answered entry and name.
type CxStep = fn(&mut CompanionLists, u32, &str, &mut CompanionCx);

/// `f` over the lists and a [`CompanionCx`] built from the `World`; nothing without the spells.
fn with_cx(world: &mut World, entry: u32, name: String, what: &'static str, f: CxStep) {
    let ran = world.run_system_cached_with(
        |In((entry, name, f)): In<(u32, String, CxStep)>,
         mut lists: ResMut<CompanionLists>,
         spells: Option<Res<crate::ui_action::Spells>>,
         summons: Res<crate::spell::SummonSlots>,
         names: Res<NameCache>,
         commands: Res<NetCommands>,
         mut ui: ResMut<UiEvents>| {
            let Some(spells) = spells.as_deref() else {
                return;
            };
            let mut cx = CompanionCx {
                catalog: &spells.catalog,
                summons: &summons.0,
                names: &names,
                commands: &commands,
                ui: &mut ui,
            };
            f(&mut lists, entry, &name, &mut cx);
        },
        (entry, name, f),
    );
    crate::ui_cast::step_ran(ran, what);
}

/// One `COMPANION_UPDATE "MOUNT"` per `MOUNTED` effect of `spell_id` as an aura of it comes or goes on us (`0x724a6b`, `0x71e9b2`).
pub(crate) fn mount_updates(spell_id: u32, catalog: &SpellCatalog, ui: &mut UiEvents) {
    let effects = catalog.get(spell_id).map_or(0, |d| {
        d.effect_apply_aura
            .iter()
            .filter(|&&a| a == SPELL_AURA_MOUNTED)
            .count()
    });
    for _ in 0..effects {
        ui.fire("COMPANION_UPDATE", vec![ScriptValue::Str("MOUNT".into())]);
    }
}

/// `COMPANION_UPDATE "CRITTER"`, from our `UNIT_FIELD_CRITTER` watch (`0x6cf722`).
pub(crate) fn critter_update(ui: &mut UiEvents) {
    ui.fire("COMPANION_UPDATE", vec![ScriptValue::Str("CRITTER".into())]);
}

/// A critter guid's entry bits, as `0x802dd8` takes them: bits 24..51, whatever the high word.
pub(crate) fn critter_entry(guid: u64) -> u32 {
    ((guid >> 24) & 0x0fff_ffff) as u32
}

/// The active-spell test (`0x802cb0`) for a companion: a slot-5 summon is active while the critter
/// out is its creature (`0x802dcc`, the guid's entry bits); a mount while an aura of its spell is
/// up, or any `MOUNTED` aura riding the same creature (the walk's tail).
fn companion_active(
    spell_id: u32,
    d: &benilla_formats::SpellDisplay,
    kind: CompanionKind,
    catalog: &SpellCatalog,
    critter: Option<u64>,
    auras: Option<&UnitAuras>,
) -> bool {
    match kind {
        CompanionKind::Critter => critter.map(critter_entry) == Some(d.companion_creature()),
        CompanionKind::Mount => auras.is_some_and(|auras| {
            auras.iter().any(|a| {
                a.spell_id == spell_id
                    || catalog.get(a.spell_id).is_some_and(|m| {
                        m.effect_apply_aura[0] == SPELL_AURA_MOUNTED
                            && m.effect_misc_value[0] == d.effect_misc_value[0]
                    })
            })
        }),
    }
}

/// One companion as `GetCompanionInfo` reads it when called: the creature's cached name, the active icon while active (`0x53e3d3`), and the active test against our own fields and auras.
pub(crate) fn companion_view(
    spell_id: u32,
    catalog: &SpellCatalog,
    summons: &SummonPropertiesCatalog,
    name_of: &dyn Fn(u32) -> Option<String>,
    me: Option<(&ObjectFields, Option<&UnitAuras>)>,
) -> Option<CompanionView> {
    let d = catalog.get(spell_id)?;
    let kind = d.companion_kind(summons)?;
    let creature_id = d.companion_creature();
    let active = companion_active(
        spell_id,
        d,
        kind,
        catalog,
        me.and_then(|(f, _)| f.unit_critter()),
        me.and_then(|(_, a)| a),
    );
    let icon = if active {
        d.active_icon.clone().or_else(|| d.icon.clone())
    } else {
        d.icon.clone()
    };
    Some(CompanionView {
        creature_id,
        name: name_of(creature_id),
        spell_id,
        icon,
        active,
    })
}

/// What the companion verbs read at call time, part of the VM's live view.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CompanionLive<'w, 's> {
    lists: Option<Res<'w, CompanionLists>>,
    spells: Option<Res<'w, Spells>>,
    summons: Option<Res<'w, SummonSlots>>,
    names: Option<Res<'w, NameCache>>,
    commands: Option<Res<'w, NetCommands>>,
    me: Query<'w, 's, (&'static ObjectStore, Option<&'static UnitAuras>), With<SelfPlayer>>,
}

impl CompanionLive<'_, '_> {
    pub(crate) fn spells(&self, ty: CompanionType) -> Vec<u32> {
        self.lists
            .as_ref()
            .map_or_else(Vec::new, |l| l.list(ty).to_vec())
    }

    pub(crate) fn random_pool(&self) -> Vec<u32> {
        self.lists
            .as_ref()
            .map_or_else(Vec::new, |l| l.random_pool().to_vec())
    }

    /// List `ty`'s companion at `index`, its name asked on a miss (`0x53e2c0`).
    pub(crate) fn info(&self, ty: CompanionType, index: usize) -> Option<CompanionView> {
        let spell_id = *self.lists.as_ref()?.list(ty).get(index)?;
        let (spells, summons) = (self.spells.as_deref()?, self.summons.as_deref()?);
        let name_of = |entry: u32| {
            let (names, commands) = (self.names.as_deref()?, self.commands.as_deref()?);
            names
                .answer(NameKey::Creature(entry), 0, commands)
                .flatten()
                .map(str::to_string)
        };
        let me = self.me.iter().next().map(|(s, a)| (&s.0, a));
        companion_view(spell_id, &spells.catalog, &summons.0, &name_of, me)
    }
}

/// The companion handlers: the creature answers the lists wait on.
pub(crate) mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{CompanionCx, CompanionLists};
    use crate::names::NameCache;
    use crate::net::{NetCommands, NetHandlerApp};
    use crate::ui_script::live::UiEvents;

    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::CreatureName, on_creature_answer)
            .net_handler(SessionEventKind::LoggedOut, on_game_end)
            .net_handler(SessionEventKind::Disconnected, on_game_end);
    }

    /// Leaving the world empties the known-spell buffer (`0x4065c3` -> `0x6e47d1`).
    fn on_game_end(In(ev): In<SessionEvent>, mut buffer: ResMut<super::KnownSpellBuffer>) {
        if matches!(
            ev,
            SessionEvent::LoggedOut | SessionEvent::Disconnected { .. }
        ) {
            buffer.0.clear();
        }
    }

    /// `SMSG_CREATURE_QUERY_RESPONSE` for a record the lists wait on (`0x53ebc0`).
    fn on_creature_answer(
        In(ev): In<SessionEvent>,
        mut lists: ResMut<CompanionLists>,
        spells: Option<Res<crate::ui_action::Spells>>,
        summons: Res<crate::spell::SummonSlots>,
        names: Res<NameCache>,
        commands: Res<NetCommands>,
        mut ui: ResMut<UiEvents>,
    ) {
        let SessionEvent::CreatureName { entry, name, .. } = ev else {
            return;
        };
        let Some(spells) = spells.as_deref() else {
            return;
        };
        if !lists.pending.iter().any(|&(c, _)| c == entry) {
            return;
        }
        let mut cx = CompanionCx {
            catalog: &spells.catalog,
            summons: &summons.0,
            names: &names,
            commands: &commands,
            ui: &mut ui,
        };
        super::record_answered(&mut lists, entry, name.as_deref(), &mut cx);
    }
}

/// The aura cancel (`0x802f80`) on our own player for one companion: nothing while charmed;
/// a slot-5 summon whose creature is out sends `CMSG_DISMISS_CRITTER` with the critter's guid
/// (`0x80303d`); an aura spell cancels its aura, a mount ridden through another spell of the same
/// creature that one (`0x8030bc`).
pub(crate) fn dismiss(
    spell_id: u32,
    catalog: &SpellCatalog,
    summons: &SummonPropertiesCatalog,
    store: &ObjectStore,
    auras: Option<&UnitAuras>,
) -> Vec<ClientCommand> {
    let Some(d) = catalog.get(spell_id) else {
        return Vec::new();
    };
    if store.0.unit_charmed_by().is_some() {
        return Vec::new();
    }
    let mut sends = Vec::new();
    if d.companion_kind(summons) == Some(CompanionKind::Critter) {
        if let Some(guid) = store.0.unit_critter() {
            if critter_entry(guid) == d.companion_creature() {
                sends.push(ClientCommand::DismissCritter { guid });
            }
        }
    }
    if d.effect_apply_aura.iter().all(|&a| a == 0) {
        return sends;
    }
    let ridden = (store.0.unit_mount_display_id() > 0
        && d.effect_apply_aura.contains(&SPELL_AURA_MOUNTED))
    .then(|| {
        auras?.iter().find_map(|a| {
            let m = catalog.get(a.spell_id)?;
            (m.effect_apply_aura[0] == SPELL_AURA_MOUNTED
                && m.effect_misc_value[0] == d.effect_misc_value[0]
                && a.spell_id != spell_id)
                .then_some(a.spell_id)
        })
    })
    .flatten();
    sends.push(ClientCommand::CancelAura {
        spell_id: ridden.unwrap_or(spell_id),
    });
    sends
}

/// The dismissals `DismissCompanion` queued.
fn drain_companions(
    mut script: crate::ui_script::InTurn,
    spells: Option<Res<Spells>>,
    summons: Res<SummonSlots>,
    commands: Res<NetCommands>,
    self_q: Query<(&ObjectStore, Option<&UnitAuras>), With<SelfPlayer>>,
) {
    let Some(mut script) = script.turn() else {
        return;
    };
    let intents = script.take_companion_intents();
    if intents.is_empty() {
        return;
    }
    let (Some(spells), Some((store, auras))) = (spells.as_deref(), self_q.iter().next()) else {
        return;
    };
    for CompanionIntent::Dismiss(spell_id) in intents {
        for send in dismiss(spell_id, &spells.catalog, &summons.0, store, auras) {
            debug!("ui_companion: dismiss {spell_id} — {send:?}");
            let _ = commands.0.send(send);
        }
    }
}

/// Point the preview booth at the pane's `SetCreature` creature by its cached record's display
/// (`0x597960`, the record's first display id), asking for a record not yet cached. The pane's
/// rotate buttons own the yaw.
fn feed_companion_booth(
    mut script: crate::ui_script::InTurn,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut booth: ResMut<crate::portrait::CompanionBooth>,
) {
    let Some(script) = script.turn() else {
        return;
    };
    let pane = script.model_pane("CompanionModelFrame");
    let yaw = pane.as_ref().map_or(0.61, |m| m.facing);
    let display = pane.and_then(|m| m.creature).and_then(|entry| {
        let record = names.creature_record(entry);
        if record.is_none() {
            names.resolve_creature(entry, 0, &commands);
        }
        record.map(|r| r.display_id).filter(|&d| d != 0)
    });
    if booth.yaw != yaw {
        booth.yaw = yaw;
    }
    if booth.display_id != display {
        booth.display_id = display;
    }
}

#[cfg(test)]
#[path = "ui_companion_tests.rs"]
mod tests;
