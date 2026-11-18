//! The player's runes (12340's rune table): the three rune packets and the rune block of our own
//! casts apply in their handlers and fire `RUNE_POWER_UPDATE` and `RUNE_TYPE_UPDATE` there; the
//! regen tick runs each frame as the player's update does; the rune verbs read the table and the
//! player's rates through the live view when called.

use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_formats::SpellRuneCostCatalog;
use benilla_protocol::{ObjectFields, SessionEvent, SessionEventKind};
use benilla_ui::script::{RuneEvent, RuneRegen, RuneState, ScriptValue};

use crate::net::{NetHandlerApp, ObjectStore, SelfGuid, SelfPlayer};
use crate::ui_action::Spells;
use crate::ui_script::live::UiEvents;

/// `castFlags` the start and go handlers require before predicting runes (`0x806742`,
/// `0x806766`): the caster's power left, and the rune block.
const CAST_FLAG_POWER_LEFT_SELF: u32 = 0x800;
const CAST_FLAG_RUNE_LIST: u32 = 0x20_0000;

/// `POWER_RUNE`: a spell of this power type spends runes (`0x8068c0`, `0x80e6ad`).
const POWER_RUNE: u32 = 5;

/// `POWER_RUNIC_POWER`: the display power the regen tick checks for (`0x728aa1`).
const POWER_RUNIC_POWER: u8 = 6;

/// The rune table, written by the handlers and the regen tick alone.
#[derive(Resource, Default)]
pub(crate) struct Runes {
    pub(crate) state: RuneState,
    /// Moves whenever a packet or the regen tick changed the table, for gated feeds.
    pub(crate) epoch: u64,
}

/// `SpellRuneCost.dbc`; empty without client data.
#[derive(Resource, Default)]
pub(crate) struct RuneCosts(pub(crate) SpellRuneCostCatalog);

/// `PLAYER_RUNE_REGEN_1..4` off the active player, read where a rate is needed (`0x5ee140`).
pub(crate) fn regen_of(fields: &ObjectFields) -> RuneRegen {
    std::array::from_fn(|kind| fields.player_rune_regen(kind as u8))
}

pub(crate) struct UiRunePlugin;

impl Plugin for UiRunePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Runes>()
            .init_resource::<RuneCosts>()
            .net_handler(SessionEventKind::ConvertRune, on_rune_packet)
            .net_handler(SessionEventKind::ResyncRunes, on_rune_packet)
            .net_handler(SessionEventKind::AddRunePower, on_rune_packet)
            .net_handler(SessionEventKind::CastRunes, on_rune_packet)
            .add_systems(Startup, load_rune_costs.after(AssetSet::Open))
            // A game start runs the rune table's init (`0x52b550` -> `0x5edef0`).
            .add_systems(
                OnEnter(crate::char_select::ClientState::InWorld),
                |mut runes: ResMut<Runes>| runes.state.initialize_game(),
            )
            .add_systems(
                Update,
                // The player's update ticks the runes each frame (`0x728aa6`), after the packets.
                tick_runes.after(benilla_world::schedule::WorldStage::Net),
            );
    }
}

/// The world entry's rune step (`0x528294` -> `0x5edef0`), after `PLAYER_ENTERING_WORLD`
/// (`0x5281a6`): the game start's table again, with no event.
pub(crate) fn enter_world(world: &mut World) {
    if let Some(mut runes) = world.get_resource_mut::<Runes>() {
        runes.state.initialize_game();
        runes.epoch = runes.epoch.wrapping_add(1);
    }
}

fn load_rune_costs(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_spell_rune_costs(&mut chain)
    };
    match loaded {
        Ok(catalog) => commands.insert_resource(RuneCosts(catalog)),
        Err(e) => {
            warn!("ui_rune: SpellRuneCost.dbc failed — rune spells read as affordable: {e:#}")
        }
    }
}

/// The rune packets and our casts' rune blocks, applied as each handler runs, their events fired
/// once it returns: `SMSG_CONVERT_RUNE` (`0x5ee020`), `SMSG_RESYNC_RUNES` (`0x5ee440`),
/// `SMSG_ADD_RUNE_POWER` (`0x5ee520`), and our own rune spell's start or go (`0x8068c0`-`0x8068f6`,
/// `0x80e6ad`-`0x80e712`). The start's `UNIT_FLAG2` `0x100` gate is not built: the layout has no
/// `FLAGS_2`.
#[allow(clippy::too_many_arguments)] // a Bevy system's full input set
fn on_rune_packet(
    In(ev): In<SessionEvent>,
    mut runes: ResMut<Runes>,
    self_guid: Res<SelfGuid>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    spells: Option<Res<Spells>>,
    clock: Res<crate::ui_script::UiClock>,
    mut ui: ResMut<UiEvents>,
) {
    let me = self_q.iter().next();
    let regen = me.map_or([0.0; benilla_ui::script::RUNE_KINDS], |m| regen_of(&m.0));
    // The milliseconds of this frame's `GetTime`, the clock the reference stamps runes on.
    let now_ms = (clock.ui_now * 1000.0) as u32;
    let state = &mut runes.state;
    let events = match ev {
        SessionEvent::ConvertRune(c) => state.convert(c.rune, c.kind),
        SessionEvent::ResyncRunes(r) => state.resync(r.count, &r.runes, now_ms, &regen),
        SessionEvent::AddRunePower { mask } => state.add_power(mask, now_ms, &regen),
        SessionEvent::CastRunes {
            caster,
            spell_id,
            cast_flags,
            runes: wire,
        } => {
            let ours = me.is_some() && Some(caster) == self_guid.0;
            let flagged = cast_flags & CAST_FLAG_POWER_LEFT_SELF != 0
                && cast_flags & CAST_FLAG_RUNE_LIST != 0;
            let rune_spell = spells
                .as_deref()
                .and_then(|s| s.catalog.get(spell_id))
                .is_some_and(|d| d.power_type == POWER_RUNE);
            if !(ours && flagged && rune_spell) {
                return;
            }
            state.set_predicted(
                u32::from(wire.before),
                u32::from(wire.after),
                Some(&wire.progress),
                now_ms,
                &regen,
            )
        }
        _ => return,
    };
    if !events.is_empty() {
        runes.epoch = runes.epoch.wrapping_add(1);
    }
    fire(&mut ui, events);
}

/// The regen tick, which the player's update runs for the active player while its display power is
/// runic power (`0x728aa1`); its `UNIT_FLAG2_REGENERATE_POWER` gate (`0x728a45`) is not built. A
/// rune coming ready fires from the tick, as the reference's update signals it.
fn tick_runes(
    mut runes: ResMut<Runes>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    clock: Res<crate::ui_script::UiClock>,
    mut ui: ResMut<UiEvents>,
    mut commands: Commands,
) {
    let Some(me) = self_q
        .iter()
        .next()
        .filter(|me| me.0.unit_power_type() == POWER_RUNIC_POWER)
    else {
        return;
    };
    let now_ms = (clock.ui_now * 1000.0) as u32;
    // A quiet tick leaves the table unmarked.
    let events = runes
        .bypass_change_detection()
        .state
        .tick(now_ms, &regen_of(&me.0));
    if events.is_empty() {
        return;
    }
    runes.set_changed();
    runes.epoch = runes.epoch.wrapping_add(1);
    fire(&mut ui, events);
    commands.queue(crate::ui_script::live::fire_handler_events);
}

/// Queue a rune change's events in the reference's order.
fn fire(ui: &mut UiEvents, events: Vec<RuneEvent>) {
    for event in events {
        match event {
            RuneEvent::Power { rune, usable } => ui.fire(
                "RUNE_POWER_UPDATE",
                vec![ScriptValue::Int(i64::from(rune)), ScriptValue::Bool(usable)],
            ),
            RuneEvent::Type { rune } => {
                ui.fire("RUNE_TYPE_UPDATE", vec![ScriptValue::Int(i64::from(rune))])
            }
            RuneEvent::ActionBarCooldown => ui.fire("ACTIONBAR_UPDATE_COOLDOWN", vec![]),
            // The action feed reads the table every frame and fires its own usable events.
            RuneEvent::Usable => {}
        }
    }
}
