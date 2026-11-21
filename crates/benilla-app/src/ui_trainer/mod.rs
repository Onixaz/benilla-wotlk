//! The trainer window's app side: `SMSG_TRAINER_LIST` runs the setter and the list build in its
//! handler (`0x6d12c0` -> `0x5940e0`, `0x596450`), which takes the interact target, builds
//! [`TrainerOpen`]'s window and fires `TRAINER_SHOW`; the re-evaluator (`0x596b00`) runs in each of
//! its triggers' turns and fires `TRAINER_UPDATE`. The verbs read the window through
//! [`TrainerLive`] at their call, resolve a service's gates and description there (`0x595470`,
//! `0x5955e0`, `0x5959d0`), and buy or close inside it (`0x594da0`, `0x5940e0`). A refusal goes
//! only to the log.

use std::collections::BTreeSet;

use benilla_formats::{SkillLineCatalog, SpellCatalog, SPELL_EFFECT_SKILL_STEP};
use benilla_protocol::messages::TrainerSpell;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{
    TrainerService, TrainerServiceLive, TrainerState, TrainerWindow, UiScript,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use benilla_protocol::messages::PLAYER_SKILL_SLOTS;

use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_script::live::UiEvents;
use crate::ui_session::NpcSession;
use crate::ui_spellbook::SkillLines;

/// `SMSG_TRAINER_LIST`'s `trainer_type` at a tradeskill trainer (`m_trainerType == 2`).
const TRAINER_TYPE_TRADESKILL: u32 = 2;

/// The trainer the last `CMSG_TRAINER_LIST` asked (written at `0x6d1f2f`); a list lands only for
/// it or for the gossip NPC.
#[derive(Resource, Default)]
pub(crate) struct TrainerAsked(pub(crate) Option<u64>);

/// The trainer window's globals: the trainer it is open on, the wire records the re-evaluator
/// re-derives, and the window the verbs read.
#[derive(Resource, Default)]
pub(crate) struct TrainerOpen {
    pub(crate) trainer: Option<u64>,
    /// In wire order, which is the window's service order.
    pub(crate) services: Vec<TrainerSpell>,
    pub(crate) trainer_type: u32,
    /// The greeting as the list build parsed it.
    pub(crate) greeting: String,
    /// The window the verbs read; it outlives a close as the reference's records do, emptied.
    pub(crate) window: TrainerWindow,
}

impl TrainerOpen {
    /// Store a trainer's freshly-listed records, before the window is built over them.
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
    }

    /// The setter's zero leg (`0x59411b`): the trainer zeroed and its records emptied; the
    /// greeting, the type, the selection and the visible count stay.
    pub(crate) fn clear(&mut self) {
        self.trainer = None;
        self.services.clear();
        self.window.state.services.clear();
        self.window.state.groups.clear();
    }

    /// Disconnect: drop the open window.
    pub(crate) fn clear_session(&mut self) {
        *self = Self::default();
    }
}

mod net;
mod reeval;

pub(crate) struct UiTrainerPlugin;

impl Plugin for UiTrainerPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<TrainerOpen>()
            .init_resource::<TrainerAsked>()
            .init_resource::<UnlearnedSpells>();
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
}

/// Resolve one wire [`TrainerSpell`] into the Lua [`TrainerService`] as the list build leaves it;
/// `get` is the VM's own `GlobalStrings.lua`, for the header labels and the step text. The gates,
/// the description and the product link are read at the call ([`TrainerLive`]).
fn resolve_service(
    wire: &TrainerSpell,
    cx: &ResolveCtx,
    get: &dyn Fn(&str) -> Option<String>,
) -> TrainerService {
    let spells = cx.spells;
    let display = spells.get(wire.spell);
    let (group_key, group_name) = service_group(
        wire.spell,
        cx.trainer_type,
        spells,
        cx.skill_lines,
        cx.race_class,
        get,
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
        texture: service_icon(wire.spell, cx.trainer_type, spells, cx.icons),
        description: None,
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
        ability_reqs: ability_reqs(
            wire.req_spells,
            spells,
            cx.skill_lines,
            &cx.player.known,
            cx.race_class,
        ),
        skill_step: display.is_some_and(|d| d.effects.contains(&SPELL_EFFECT_SKILL_STEP)),
        step_increase: service_step_increase(wire.spell, spells, cx.skill_lines, get),
        skill_line: service_skill_line(wire.spell, spells, cx.skill_lines, cx.race_class),
        item_link: None,
        group_key,
        group_name,
        tooltip: service_tooltip(wire.spell, spells),
    }
}

/// The window's records from [`TrainerOpen`]'s, in wire order; the tree is built by
/// [`TrainerWindow::listed`] from each service's group key.
fn snapshot(
    open: &TrainerOpen,
    cx: &ResolveCtx,
    get: &dyn Fn(&str) -> Option<String>,
) -> TrainerState {
    TrainerState {
        greeting: open.greeting.clone(),
        trainer_type: open.trainer_type,
        services: open
            .services
            .iter()
            .map(|w| resolve_service(w, cx, get))
            .collect(),
        groups: Vec::new(),
    }
}

/// The player as the re-evaluator and the requirement getters read it.
fn player_view(
    store: Option<&ObjectStore>,
    known: &BTreeSet<u32>,
    unlearned: &[u32],
) -> reeval::PlayerView {
    reeval::PlayerView {
        known: known.clone(),
        race_class: crate::spell::race_class(store),
        unlearned: unlearned.to_vec(),
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

/// The `SMSG_SEND_UNLEARN_SPELLS` list (`0x6e2240`), which the re-evaluator grays.
#[derive(Resource, Default)]
pub(crate) struct UnlearnedSpells(pub(crate) Vec<u32>);

/// `serviceTypeFilter` as the VM holds it now (`0x594ba0`), its registered default with no VM.
fn type_filter(world: &World) -> u32 {
    world
        .get_non_send_resource::<UiScript>()
        .map_or(3, benilla_ui::script::trainer_type_filter)
}

/// The player as the world holds it now, for the re-evaluator and the list build.
fn world_player(world: &mut World) -> Option<reeval::PlayerView> {
    let known = world
        .get_resource::<PlayerActions>()
        .map(|a| a.spells.clone())
        .unwrap_or_default();
    let unlearned = world
        .get_resource::<UnlearnedSpells>()
        .map(|u| u.0.clone())
        .unwrap_or_default();
    let mut me = world.query_filtered::<&ObjectStore, With<SelfPlayer>>();
    let store = me.iter(world).next()?;
    Some(player_view(Some(store), &known, &unlearned))
}

/// `SMSG_TRAINER_LIST`'s setter and list build (`0x6d14ed`, `0x6d151e` -> `0x5940e0`, `0x596450`): a
/// zero trainer runs the setter's zero leg; another than the open one takes the interact target,
/// the active player skipping it (`0x59413f`), the same one skipping the setter (`0x5940fa`). The
/// window is built once the target's close has fired.
pub(crate) fn list(
    world: &mut World,
    trainer: u64,
    trainer_type: u32,
    services: Vec<TrainerSpell>,
    greeting: String,
) {
    let held = world.get_resource::<TrainerOpen>().and_then(|t| t.trainer);
    let me = world
        .get_resource::<crate::net::SelfGuid>()
        .and_then(|s| s.0);
    if trainer == 0 {
        set_none(world);
    } else if held != Some(trainer) && me != Some(trainer) {
        crate::ui_session::set_interact_target(world, trainer);
    }
    crate::ui_script::live::in_turn(world, move |w| {
        build_list(w, trainer, trainer_type, services, greeting);
    });
}

/// The list build (`0x596450`): the records stored, the tree built with every line passing and
/// expanded, the filter-and-sort run, the greeting parsed against the active player (`0x596ad3`),
/// then `TRAINER_SHOW` (`0x596ae2`).
fn build_list(
    world: &mut World,
    trainer: u64,
    trainer_type: u32,
    services: Vec<TrainerSpell>,
    greeting: String,
) {
    let greeting = parse_greeting(world, &greeting);
    let filter = type_filter(world);
    let player = world_player(world).unwrap_or_default();
    let race_class = player.race_class;
    let state = {
        let get = |key: &str| {
            world
                .get_non_send_resource::<UiScript>()
                .and_then(|s| s.lua().globals().get::<String>(key).ok())
                .filter(|t| !t.is_empty())
        };
        let spells = world.get_resource::<Spells>();
        let skill_lines = world.get_resource::<SkillLines>();
        let icons = world.get_resource::<ItemDisplays>();
        let empty = SpellCatalog::from_displays(Default::default());
        let catalog = spells.map_or(&empty, |s| &s.catalog);
        let mut open = TrainerOpen::default();
        open.open(trainer, trainer_type, services.clone(), greeting.clone());
        snapshot(
            &open,
            &ResolveCtx {
                trainer_type,
                spells: catalog,
                skill_lines: skill_lines.map(|s| &*s.catalog),
                race_class,
                player: &player,
                icons,
            },
            &get,
        )
    };
    if let Some(mut open) = world.get_resource_mut::<TrainerOpen>() {
        open.open(trainer, trainer_type, services, greeting);
        open.trainer = (trainer != 0).then_some(trainer);
        open.window = TrainerWindow::listed(state, filter);
    }
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.fire("TRAINER_SHOW", Vec::new());
    }
}

/// The greeting with its `$` tokens expanded against the active player (`0x596ad3`).
fn parse_greeting(world: &mut World, greeting: &str) -> String {
    let mut state = bevy::ecs::system::SystemState::<(
        Query<(&ObjectStore, &crate::net::Guid), With<SelfPlayer>>,
        Option<Res<NameCache>>,
        Option<Res<NetCommands>>,
        Option<Res<crate::world_state::WorldStates>>,
    )>::new(world);
    let (me, names, commands, states) = state.get(world);
    let (Some(names), Some(commands), Some(states)) = (names, commands, states) else {
        return greeting.to_string();
    };
    let player = crate::npc_text::player_identity(&me, &names, &commands);
    crate::npc_text::substitute(
        greeting,
        &crate::npc_text::MacroContext {
            subject: player.as_ref(),
            states: &states,
            achievement: None,
        },
    )
}

/// The re-evaluator (`0x596b00`) in its trigger's turn: with a trainer open and an active player,
/// every record's state re-derived, the filter-and-sort run (`0x59708c`) and `TRAINER_UPDATE`
/// fired (`0x597098`).
pub(crate) fn refresh(world: &mut World) {
    if world
        .get_resource::<TrainerOpen>()
        .and_then(|t| t.trainer)
        .is_none()
    {
        return;
    }
    let Some(player) = world_player(world) else {
        return;
    };
    let filter = type_filter(world);
    world.resource_scope::<TrainerOpen, _>(|world, mut open| {
        let (Some(spells), Some(lines)) = (
            world.get_resource::<Spells>(),
            world.get_resource::<SkillLines>(),
        ) else {
            return;
        };
        let open = &mut *open;
        reeval::re_derive_all(&mut open.services, &spells.catalog, &lines.catalog, &player);
        for (service, wire) in open.window.state.services.iter_mut().zip(&open.services) {
            service.category = law::category(wire.state);
        }
        open.window.filter_and_sort(filter);
    });
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.fire("TRAINER_UPDATE", Vec::new());
    }
}

/// The setter's zero leg (`0x5940e0`): with a trainer open, `TRAINER_CLOSED` (`0x594108`), then the
/// interact target cleared on it, whose interaction close runs this leg again while it held it
/// (`0x594113` -> `0x512f80`), then the zero (`0x59411b`).
pub(crate) fn set_none(world: &mut World) {
    let Some(trainer) = world.get_resource::<TrainerOpen>().and_then(|t| t.trainer) else {
        return;
    };
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.fire("TRAINER_CLOSED", Vec::new());
    }
    crate::ui_script::live::in_turn(world, move |w| {
        crate::ui_session::clear_interact_target(w, trainer);
        crate::ui_script::live::in_turn(w, |w| {
            if let Some(mut open) = w.get_resource_mut::<TrainerOpen>() {
                open.clear();
            }
        });
    });
}

/// The world leave's trainer part (`0x528cfb` -> `0x5947b0`): the setter's zero leg.
pub(crate) fn leave_world(world: &mut World) {
    set_none(world);
}

/// The range guard and the interaction walk read the trainer open on.
impl NpcSession for TrainerOpen {
    fn npc(&self) -> Option<u64> {
        self.trainer
    }

    fn close(&mut self) {
        self.clear();
    }
}

/// The trainer window the verbs read and act on at their call.
#[derive(SystemParam)]
pub(crate) struct TrainerLive<'w, 's> {
    open: ResMut<'w, TrainerOpen>,
    spells: Option<Res<'w, Spells>>,
    skill_lines: Option<Res<'w, SkillLines>>,
    items: Res<'w, Items>,
    commands: Res<'w, NetCommands>,
    me: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    spell_mods: Option<Res<'w, crate::spell::SpellModifiers>>,
    auras: Query<'w, 's, &'static crate::unit_auras::UnitAuras, With<SelfPlayer>>,
    /// `$z`: the bind point's area (`SMSG_BINDPOINTUPDATE`), named through `AreaTable`.
    home_bind: Option<Res<'w, crate::net::HomeBind>>,
    area_names: Option<Res<'w, crate::ui_quest_log::QuestHeaderNamesRes>>,
}

impl TrainerLive<'_, '_> {
    pub(crate) fn window(&self) -> &TrainerWindow {
        &self.open.window
    }

    pub(crate) fn window_mut(&mut self) -> &mut TrainerWindow {
        &mut self.open.window
    }

    /// Whether a trainer is open (the setter stored a nonzero guid).
    pub(crate) fn is_open(&self) -> bool {
        self.open.trainer.is_some()
    }

    /// Service `position`'s gates and product link at the call (`0x595470`, `0x5955e0`, `0x596290`).
    pub(crate) fn service_live(
        &self,
        position: usize,
        known: &BTreeSet<u32>,
    ) -> Option<TrainerServiceLive> {
        let wire = self.open.services.get(position)?;
        let spells = self.spells.as_deref()?;
        let skill_lines = self.skill_lines.as_deref().map(|s| &*s.catalog);
        let store = self.me.iter().next();
        let player = player_view(store, &BTreeSet::new(), &[]);
        Some(TrainerServiceLive {
            skill_req: law::skill_req(wire.req_skill, wire.req_skill_value, skill_lines, &player),
            ability_reqs: law::ability_reqs(
                wire.req_spells,
                &spells.catalog,
                skill_lines,
                known,
                player.race_class,
            ),
            item_link: law::service_item_link(
                wire.spell,
                self.open.trainer_type,
                &spells.catalog,
                &self.items,
                &self.commands,
            ),
        })
    }

    /// `GetTrainerServiceDescription` (`0x5959d0`): the spell's description parsed now against the
    /// active player, else a recipe's product description off the item cache, a miss asked with
    /// the trainer's record callback (`0x595b02`).
    pub(crate) fn description(
        &self,
        position: usize,
        known: &BTreeSet<u32>,
        waits: Option<&mut crate::items::RecordWaits>,
        global: &dyn Fn(&str) -> Option<String>,
    ) -> Option<String> {
        let wire = self.open.services.get(position)?;
        let spells = self.spells.as_deref()?;
        let d = spells.catalog.get(wire.spell)?;
        if let Some(text) = d.description.as_deref().filter(|t| !t.is_empty()) {
            let store = self.me.iter().next()?;
            let skill_catalog = self.skill_lines.as_deref().map(|s| &*s.catalog);
            let skill = |id| crate::spell::spell_skill_value(Some(store), skill_catalog, id);
            let operands = crate::spell::text_caster::Operands::of(Some(store), &self.items);
            let fallback = crate::spell::SpellModifiers::default();
            let mods = self.spell_mods.as_deref().unwrap_or(&fallback);
            let cost = |d: &benilla_formats::SpellDisplay| {
                crate::spell::usable::power_cost(d, store, mods) as i32
            };
            let caster = crate::spell::text_caster::untracked(
                store,
                (known, self.auras.single().ok()),
                &operands,
                mods,
                &skill,
                &cost,
            );
            let home_area: Option<String> = self
                .home_bind
                .as_deref()
                .and_then(|b| b.0)
                .and_then(|id| self.area_names.as_deref()?.0.resolve(id as i32))
                .map(str::to_string);
            let ctx = benilla_formats::TokenContext {
                durations: &spells.durations,
                radii: &spells.radii,
                ranges: Some(&spells.ranges),
                variables: Some(spells.catalog.description_variables()),
                skill: &skill,
                lookup: &|id| spells.catalog.get(spells.for_difficulty(id)),
                mods: Some(mods as &dyn benilla_formats::SpellMods),
                unmodified_points: false,
                gender: &|| store.0.unit_gender().unwrap_or(0),
                home_area: &|| home_area.as_deref(),
                caster: Some(&caster),
                global,
                printf: &crate::ui_script::token_printf,
            };
            return Some(benilla_formats::substitute(text, d, &ctx));
        }
        let product = law::description_product(wire.spell, &spells.catalog)?;
        if let Some(t) = self.items.template_cached(product) {
            return Some(t.description.clone()).filter(|t| !t.is_empty());
        }
        let _ = self.items.template(product, 0, &self.commands);
        if let Some(waits) = waits {
            waits.wait(product, crate::items::RecordWait::Trainer, false);
        }
        None
    }

    /// The trainer's buy (`0x594da0`): `CMSG_TRAINER_BUY_SPELL` naming the trainer, sent now.
    pub(crate) fn buy(&self, spell_id: u32) {
        let trainer = self.open.trainer.unwrap_or(0);
        debug!("ui_trainer: buy service (trainer {trainer:#x}, spell {spell_id})");
        let _ = self
            .commands
            .0
            .send(ClientCommand::TrainerBuySpell { trainer, spell_id });
    }

    /// The setter's interact target clear (`0x594113`): true when the trainer held it, whose
    /// interaction close runs the zero leg again inside it (`0x512f80`).
    pub(crate) fn release_target(
        &mut self,
        target: Option<&mut crate::ui_session::InteractTarget>,
    ) -> bool {
        let Some(trainer) = self.open.trainer else {
            return false;
        };
        target.is_some_and(|t| t.clear_in_call(trainer))
    }

    /// The setter's zero (`0x59411b`).
    pub(crate) fn zero(&mut self) {
        self.open.clear();
    }
}

mod law;
use law::{
    ability_reqs, category, service_group, service_icon, service_skill_line, service_step_increase,
    service_tooltip, skill_req,
};

#[cfg(test)]
mod tests;
