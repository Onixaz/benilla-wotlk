//! The trainer feed: [`TrainerOpen`] from `SMSG_TRAINER_LIST`, [`feed_trainer`] resolving it into
//! Lua [`TrainerService`]s and firing the trainer events, and [`drain_trainer`] sending the buys;
//! `CloseTrainer` sends nothing. A bought row repaints through the re-evaluator ([`reeval`]),
//! never through `SMSG_TRAINER_BUY_SUCCEEDED`, and a refusal goes only to the log.

use std::collections::{BTreeSet, HashMap};

use benilla_formats::{SkillLineCatalog, SpellCatalog, SPELL_EFFECT_SKILL_STEP};
use benilla_protocol::messages::TrainerSpell;
use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, TrainerService, TrainerState, TrainerTooltip, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use benilla_protocol::field::{
    FIELD_PLAYER_FIELD_COINAGE, FIELD_PLAYER_SKILL_INFO_1_1, FIELD_UNIT_LEVEL,
};
use benilla_protocol::messages::PLAYER_SKILL_SLOTS;
use bevy::ecs::system::SystemParam;

use crate::net::{ClientCommand, FieldChanged, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::{close_npc_session_out_of_range, npc_switched, NpcSession};
use crate::ui_spellbook::SkillLines;

/// `SMSG_TRAINER_LIST`'s `trainer_type` at a tradeskill trainer (`m_trainerType == 2`).
const TRAINER_TYPE_TRADESKILL: u32 = 2;

/// The open trainer as `SMSG_TRAINER_LIST` delivered it.
#[derive(Resource, Default)]
pub(crate) struct TrainerOpen {
    pub(crate) trainer: Option<u64>,
    /// In wire order, which is the 1-based display order.
    pub(crate) services: Vec<TrainerSpell>,
    pub(crate) trainer_type: u32,
    pub(crate) greeting: String,
    /// A list landed and the engine's collapse set and line filter are not yet reset, as
    /// `AddServices` (`0x596450`) resets them per list; an open window repaints through
    /// [`reeval`], never a second list.
    pub(crate) fresh_list: bool,
    /// One of the re-evaluator's triggers fired with the window open; the feed re-derives and
    /// fires `TRAINER_UPDATE` even if no state moved, as `0x596b00`'s tail does (`0x597098`), and
    /// the stock window re-reads `GetMoney()` on it.
    pub(crate) re_derive: bool,
}

impl TrainerOpen {
    /// Open (or replace) the window with a trainer's freshly-listed services.
    pub(crate) fn open(
        &mut self,
        trainer: u64,
        trainer_type: u32,
        services: Vec<TrainerSpell>,
        greeting: String,
    ) {
        self.trainer = Some(trainer);
        self.trainer_type = trainer_type;
        self.services = services;
        self.greeting = greeting;
        // A fresh list carries the server's own states, so nothing is pending against it.
        self.fresh_list = true;
        self.re_derive = false;
    }

    /// A trigger fired: re-derive on the next feed, if a window is open (`0x596b14`).
    pub(crate) fn trigger_re_derive(&mut self) {
        if self.trainer.is_some() {
            self.re_derive = true;
        }
    }

    /// A client-side close; a re-open re-lists.
    pub(crate) fn clear(&mut self) {
        self.trainer = None;
        self.services.clear();
        self.trainer_type = 0;
        self.greeting.clear();
        self.fresh_list = false;
        self.re_derive = false;
    }

    /// Disconnect: drop the open window.
    pub(crate) fn clear_session(&mut self) {
        self.clear();
    }
}

/// `SMSG_TRAINER_BUY_FAILED` codes, queued for the feed to log.
#[derive(Resource, Default)]
pub(crate) struct TrainerErrors(pub Vec<u32>);

/// The spells the open trainer's services show as tooltips, which the spell-tooltip feed pushes: a
/// pet-learn service's view is built against the pet (the builder's unit selector).
#[derive(Resource, Default, PartialEq)]
pub(crate) struct TrainerTooltipSubjects {
    pub(crate) player: Vec<u32>,
    pub(crate) pet: Vec<u32>,
}

/// The trainer feed, which the spell-tooltip feed runs after.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TrainerFeed;

mod net;
mod reeval;

pub(crate) struct UiTrainerPlugin;

impl Plugin for UiTrainerPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<TrainerOpen>()
            .init_resource::<TrainerErrors>()
            .init_resource::<TrainerTooltipSubjects>()
            .add_systems(
                Update,
                (
                    // Range-close before the feed so the clear fires TRAINER_CLOSED the same frame.
                    close_npc_session_out_of_range::<TrainerOpen>.before(feed_trainer),
                    // The unit feeds share the VM and the player's stores with it; either order
                    // reads the same state, but the schedule check wants one declared.
                    feed_trainer
                        .in_set(UiFeed)
                        .in_set(TrainerFeed)
                        .after(crate::ui_unit::UnitFeed),
                    drain_trainer.after(UiInput),
                ),
            );
    }
}

/// The log line for a `SMSG_TRAINER_BUY_FAILED` code: the reference shows the player nothing, its
/// handler (`0x6d8410`) only writes these lines to the console.
fn train_error_log_line(code: u32) -> &'static str {
    use benilla_protocol::messages::train_fail;
    match code {
        train_fail::NOT_ENOUGH_MONEY => "Not enough money for trainer service",
        train_fail::NOT_ENOUGH_SKILL => "Not enough skill points for trainer service",
        _ => "Trainer service unavailable",
    }
}

/// What resolving a service reads besides its wire record.
struct ResolveCtx<'a> {
    trainer_type: u32,
    spells: &'a SpellCatalog,
    skill_lines: Option<&'a SkillLineCatalog>,
    /// The player's race and class, which the skill-line lookups key on.
    race_class: (u8, u8),
    /// The player's book, skills and level, which the requirement getters test.
    player: &'a reeval::PlayerView,
    icons: Option<&'a ItemDisplays>,
    items: &'a Items,
    commands: &'a NetCommands,
    /// Each wire spell's substituted `Spell.dbc` description, `None` when it has none.
    descriptions: &'a HashMap<u32, Option<String>>,
    /// The VM's own `GlobalStrings.lua`, for the header labels and the step text.
    get: &'a dyn Fn(&str) -> Option<String>,
}

/// Resolve one wire [`TrainerSpell`] into the Lua [`TrainerService`].
fn resolve_service(wire: &TrainerSpell, cx: &ResolveCtx) -> TrainerService {
    let spells = cx.spells;
    let display = spells.get(wire.spell);
    let (group_key, group_name) = service_group(
        wire.spell,
        cx.trainer_type,
        spells,
        cx.skill_lines,
        cx.race_class,
        cx.get,
    );
    if group_key == 0 && cx.skill_lines.is_some() {
        debug!(
            "ui_trainer: trainer spell {} has no skill line — dropped from the tree",
            wire.spell
        );
    }
    TrainerService {
        spell_id: wire.spell,
        name: display.map(|d| d.name.clone()),
        subtext: display.and_then(|d| d.rank.clone()),
        texture: service_icon(
            wire.spell,
            cx.trainer_type,
            spells,
            cx.icons,
            cx.items,
            cx.commands,
        ),
        // `0x5959d0`: the spell's description, else a recipe's cached product description.
        description: cx
            .descriptions
            .get(&wire.spell)
            .cloned()
            .flatten()
            .or_else(|| service_item_description(wire.spell, spells, cx.items, cx.commands)),
        cost: wire.cost,
        talent_cost: wire.talent_cost,
        profession_cost: wire.profession_cost,
        category: category(wire.state),
        level_req: u32::from(wire.req_level),
        skill_req: skill_req(
            wire.req_skill,
            wire.req_skill_value,
            cx.skill_lines,
            cx.player,
        ),
        ability_reqs: ability_reqs(wire.req_spells, spells, cx.skill_lines, &cx.player.known),
        skill_step: display.is_some_and(|d| d.effects.contains(&SPELL_EFFECT_SKILL_STEP)),
        step_increase: service_step_increase(wire.spell, spells, cx.skill_lines, cx.get),
        skill_line: service_skill_line(wire.spell, spells, cx.skill_lines, cx.race_class),
        item_link: service_item_link(wire.spell, cx.trainer_type, spells, cx.items, cx.commands),
        group_key,
        group_name,
        tooltip: service_tooltip(wire.spell, spells),
    }
}

/// Build the Lua snapshot from [`TrainerOpen`].
fn snapshot(open: &TrainerOpen, cx: &ResolveCtx) -> Option<TrainerState> {
    open.trainer?;
    Some(TrainerState {
        greeting: open.greeting.clone(),
        trainer_type: open.trainer_type,
        services: open
            .services
            .iter()
            .map(|w| resolve_service(w, cx))
            .collect(),
        // The engine builds the tree in `set_trainer` from each service's group key.
        groups: Vec::new(),
    })
}

/// Each listed spell's own description expanded through `ctx` (`0x5959d0`), `None` when it has
/// none; a text already cached stands.
fn describe_services(
    texts: &mut HashMap<u32, Option<String>>,
    services: &[TrainerSpell],
    catalog: &SpellCatalog,
    ctx: &benilla_formats::TokenContext,
) {
    for w in services {
        texts.entry(w.spell).or_insert_with(|| {
            let d = catalog.get(w.spell)?;
            let text = d.description.as_deref().filter(|t| !t.is_empty())?;
            Some(benilla_formats::substitute(text, d, ctx))
        });
    }
}

/// Whether two snapshots differ only in what an item record fills: a recipe's icon, item link and
/// product description, all read by the detail pane alone.
fn only_pane_moved(old: &TrainerState, new: &TrainerState) -> bool {
    let strip = |t: &TrainerState| {
        let mut t = t.clone();
        for s in &mut t.services {
            s.texture = None;
            s.item_link = None;
            s.description = None;
        }
        t
    };
    old != new && strip(old) == strip(new)
}

/// The player as the re-evaluator and the requirement getters read it.
fn player_view(store: Option<&ObjectStore>, known: &BTreeSet<u32>) -> reeval::PlayerView {
    reeval::PlayerView {
        known: known.clone(),
        level: store.and_then(|s| s.0.unit_level()).unwrap_or(0),
        skills: store
            .map(|store| {
                (0..PLAYER_SKILL_SLOTS)
                    .filter_map(|i| store.0.player_skill(i))
                    .filter(|s| s.skill_id != 0)
                    .map(|s| reeval::SkillSlot {
                        skill_id: u32::from(s.skill_id),
                        step: u32::from(s.step),
                        // The permanent bonus counts only on a non-zero value (`0x51a28c`).
                        value_plus_perm: if s.value == 0 {
                            0
                        } else {
                            i32::from(s.value) + i32::from(s.perm_bonus)
                        },
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// What the description `$`-tokens read of the player beside its spell modifiers, known spells and
/// auras: its skills, gender byte, expression operands and the bind point's area (`$z`).
type TokenReads = (
    crate::spell::SkillSnapshot,
    u8,
    crate::spell::text_caster::Operands,
    Option<String>,
);

/// `GetTrainerServiceDescription`'s parser inputs (`SpellParserParseText` over the player with its
/// modifiers, `0x5959d0`) and the per-spell texts, rebuilt when a token input moves.
#[derive(SystemParam)]
pub(crate) struct DescriptionInputs<'w, 's> {
    spell_mods: Res<'w, crate::spell::SpellModifiers>,
    caster_sources: crate::spell::text_caster::CasterSources<'w, 's>,
    texts: Local<'s, crate::ui_script::VmMemo<HashMap<u32, Option<String>>>>,
    last_reads: Local<'s, crate::ui_script::VmMemo<Option<TokenReads>>>,
    // `$z`: the bind point's area (`SMSG_BINDPOINTUPDATE`), named through `AreaTable`.
    home_bind: Option<Res<'w, crate::net::HomeBind>>,
    area_names: Option<Res<'w, crate::ui_quest_log::QuestHeaderNamesRes>>,
}

/// The re-evaluator's inputs and its descriptor triggers: the player's descriptor and its field
/// edges (money, level, a skill slot).
#[derive(SystemParam)]
pub(crate) struct ReEvalInputs<'w, 's> {
    self_player: Query<'w, 's, (Entity, &'static ObjectStore), With<SelfPlayer>>,
    edges: MessageReader<'w, 's, FieldChanged>,
}

/// Push the current trainer into the VM and fire its events on a change. Another trainer while
/// open is a close then an open: `ShowUIPanel` returns early on a visible frame.
#[allow(clippy::too_many_arguments)] // one Bevy system's full input set
pub(crate) fn feed_trainer(
    script: Option<NonSendMut<UiScript>>,
    // Writes the re-derived states, as `0x596b00` overwrites its own records.
    mut open: ResMut<TrainerOpen>,
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    skill_lines: Option<Res<SkillLines>>,
    mut re_eval: ReEvalInputs,
    // A tradeskill row shows its created item's icon: the template cache and `ItemDisplayInfo.dbc`.
    icons: Option<Res<ItemDisplays>>,
    items: Res<Items>,
    mut errors: ResMut<TrainerErrors>,
    commands: Res<NetCommands>,
    names: Res<NameCache>,
    mut tooltip_subjects: ResMut<TrainerTooltipSubjects>,
    mut desc: DescriptionInputs,
    mut last: Local<crate::ui_script::VmMemo<Option<TrainerState>>>,
    mut last_trainer: Local<crate::ui_script::VmMemo<Option<u64>>>,
    mut last_name: Local<crate::ui_script::VmMemo<Option<String>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let last_trainer = last_trainer.get(&script);
    let last_name = last_name.get(&script);
    // Refusals go to the log only, as the reference prints them only to its console.
    for code in errors.0.drain(..) {
        info!("ui_trainer: {} (code {code})", train_error_log_line(code));
    }
    // Nothing to resolve names or icons from until Spell.dbc loads.
    let Some(spells) = spells.as_deref() else {
        return;
    };
    // Without the skill-line catalog every service would group to 0 and drop.
    let Some(skill_lines) = skill_lines.as_deref() else {
        return;
    };
    // The per-list reset goes in before the snapshot, so `TRAINER_SHOW` finds it.
    if open.fresh_list {
        script.reset_trainer_list_state();
        open.fresh_list = false;
    }
    // The player's money, level and skill triggers, read every frame so the reader never backs up.
    let self_player = re_eval.self_player.iter().next();
    for edge in re_eval.edges.read() {
        let Some((me, _)) = self_player else {
            continue;
        };
        if edge.entity != me {
            continue;
        }
        // The reference watches each slot's value, max and permanent bonus: the second dword
        // always, the third only when its high half moved.
        let skill_edge = edge
            .index
            .checked_sub(FIELD_PLAYER_SKILL_INFO_1_1)
            .filter(|&r| r < 3 * u16::from(PLAYER_SKILL_SLOTS))
            .is_some_and(|r| match r % 3 {
                1 => true,
                2 => (edge.old ^ edge.new) & 0xFFFF_0000 != 0,
                _ => false,
            });
        let player_field = edge.kind == benilla_protocol::messages::ObjectType::Player
            && (edge.index == FIELD_PLAYER_FIELD_COINAGE || skill_edge);
        if player_field || edge.unit_field(FIELD_UNIT_LEVEL) {
            open.trigger_re_derive();
        }
    }
    // `0x596b00` over every row, then `TRAINER_UPDATE` unconditionally, as its tail fires it.
    let player = player_view(self_player.map(|(_, s)| s), &actions.spells);
    let forced = open.re_derive && open.trainer.is_some();
    if forced {
        open.re_derive = false;
        reeval::re_derive_all(
            &mut open.services,
            &spells.catalog,
            &skill_lines.catalog,
            &player,
        );
    }
    let get = |key: &str| {
        script
            .lua()
            .globals()
            .get::<String>(key)
            .ok()
            .filter(|t| !t.is_empty())
    };
    let store = self_player.map(|(_, s)| s);
    let home_area: Option<String> = desc
        .home_bind
        .as_deref()
        .and_then(|b| b.0)
        .and_then(|id| desc.area_names.as_deref()?.0.resolve(id as i32))
        .map(str::to_string);
    let reads: Option<TokenReads> = store.map(|s| {
        (
            crate::spell::skill_snapshot(Some(s)),
            s.0.unit_gender().unwrap_or(0),
            crate::spell::text_caster::Operands::of(Some(s), &items),
            home_area.clone(),
        )
    });
    let moved = *desc.last_reads.get(&script) != reads
        || desc.spell_mods.is_changed()
        || desc.caster_sources.changed();
    *desc.last_reads.get(&script) = reads;
    let texts = desc.texts.get(&script);
    if moved {
        texts.clear();
    }
    if let Some(store) = store {
        let global = |key: &str| benilla_ui::strings::global(script.lua(), key);
        let skill =
            |id| crate::spell::spell_skill_value(Some(store), Some(&skill_lines.catalog), id);
        let operands = crate::spell::text_caster::Operands::of(Some(store), &items);
        let cost = |d: &benilla_formats::SpellDisplay| {
            crate::spell::usable::power_cost(d, store, &desc.spell_mods) as i32
        };
        let caster = crate::spell::text_caster::untracked(
            store,
            (&actions.spells, desc.caster_sources.auras()),
            &operands,
            &desc.spell_mods,
            &skill,
            &cost,
        );
        let ctx = benilla_formats::TokenContext {
            durations: &spells.durations,
            radii: &spells.radii,
            ranges: Some(&spells.ranges),
            variables: Some(spells.catalog.description_variables()),
            skill: &skill,
            lookup: &|id| spells.catalog.get(id),
            mods: Some(&*desc.spell_mods as &dyn benilla_formats::SpellMods),
            unmodified_points: false,
            gender: &|| store.0.unit_gender().unwrap_or(0),
            home_area: &|| home_area.as_deref(),
            caster: Some(&caster),
            global: &global,
            printf: &crate::ui_script::token_printf,
        };
        describe_services(texts, &open.services, &spells.catalog, &ctx);
    }
    let fresh = snapshot(
        &open,
        &ResolveCtx {
            trainer_type: open.trainer_type,
            spells: &spells.catalog,
            skill_lines: Some(&skill_lines.catalog),
            race_class: crate::spell::race_class(self_player.map(|(_, s)| s)),
            player: &player,
            icons: icons.as_deref(),
            items: &items,
            commands: &commands,
            descriptions: texts,
            get: &get,
        },
    );
    // A spell subject's view must be in the store before the detail icon is hovered.
    let mut fresh_tooltip_subjects = TrainerTooltipSubjects::default();
    for service in fresh.iter().flat_map(|state| &state.services) {
        if let TrainerTooltip::Spell {
            spell_id,
            alt_caster,
        } = service.tooltip
        {
            let subjects = if alt_caster {
                &mut fresh_tooltip_subjects.pet
            } else {
                &mut fresh_tooltip_subjects.player
            };
            subjects.push(spell_id);
        }
    }
    for subjects in [
        &mut fresh_tooltip_subjects.player,
        &mut fresh_tooltip_subjects.pet,
    ] {
        subjects.sort_unstable();
        subjects.dedup();
    }
    if *tooltip_subjects != fresh_tooltip_subjects {
        *tooltip_subjects = fresh_tooltip_subjects;
    }
    // A name-only change re-fires `TRAINER_UPDATE`, so the title's `UnitName("npc")` repaints.
    // The name rides as arg1, which the 1.12 trainer events do not carry.
    let trainer_name = open
        .trainer
        .and_then(|g| names.resolve(g, &commands).map(str::to_string));
    let name_changed = *last_name != trainer_name;
    let switched = npc_switched(*last_trainer, open.trainer);
    if fresh == *last && !name_changed && !switched && !forced {
        return;
    }
    script.set_trainer(fresh.clone());
    let name_arg = || vec![ScriptValue::Str(trainer_name.clone().unwrap_or_default())];
    if switched {
        // Drop the close intent `OnHide` queues, or the drain would clear the new trainer.
        script.fire_event("TRAINER_CLOSED", vec![]);
        script.fire_event("TRAINER_SHOW", name_arg());
        let _ = script.take_trainer_close();
    } else {
        match (&*last, &fresh) {
            (None, Some(_)) => script.fire_event("TRAINER_SHOW", name_arg()),
            // A landed item record changes only the detail pane: the cache callback `0x593ca0`
            // fires `TRAINER_DESCRIPTION_UPDATE`, which re-selects without re-scrolling.
            (Some(old), Some(new)) if !name_changed && !forced && only_pane_moved(old, new) => {
                script.fire_event("TRAINER_DESCRIPTION_UPDATE", vec![])
            }
            (Some(_), Some(_)) => script.fire_event("TRAINER_UPDATE", name_arg()),
            (Some(_), None) => script.fire_event("TRAINER_CLOSED", vec![]),
            (None, None) => {}
        }
    }
    *last = fresh;
    *last_trainer = open.trainer;
    *last_name = trainer_name;
}

/// The range guard closes the window, as `CloseTrainer` would, on walk-away or despawn.
impl NpcSession for TrainerOpen {
    fn npc(&self) -> Option<u64> {
        self.trainer
    }

    fn close(&mut self) {
        self.clear();
    }
}

/// Each `BuyTrainerService` becomes `CMSG_TRAINER_BUY_SPELL`; a close sends nothing.
fn drain_trainer(
    script: Option<NonSendMut<UiScript>>,
    mut open: ResMut<TrainerOpen>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    for spell_id in script.take_trainer_buys() {
        let Some(trainer) = open.trainer else {
            debug!(
                "ui_trainer: BuyTrainerService(spell {spell_id}) with no open trainer — ignored"
            );
            continue;
        };
        debug!("ui_trainer: buy service (trainer {trainer:#x}, spell {spell_id})");
        let _ = commands
            .0
            .send(ClientCommand::TrainerBuySpell { trainer, spell_id });
    }
    if script.take_trainer_close() {
        debug!("ui_trainer: client-side close (no packet)");
        open.clear();
    }
}

mod law;
use law::{
    ability_reqs, category, service_group, service_icon, service_item_description,
    service_item_link, service_skill_line, service_step_increase, service_tooltip, skill_req,
};

#[cfg(test)]
mod tests;
