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

/// `UNIT_FIELD_FLAGS_2` bit the start handler skips its power prediction under (`0x8068ad`).
const UNIT_FLAG2_NO_POWER_PREDICT: u32 = 0x100;

/// `UNIT_FIELD_FLAGS_2` bit the power regen runs under (`0x728a45`).
const UNIT_FLAG2_REGENERATE_POWER: u32 = 0x800;

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
                // The player's update ticks the runes each frame (`0x728aa6`), after the packets and
                // before the UI's first VM holder, as the object update precedes the UI's in the frame.
                tick_runes
                    .after(benilla_world::schedule::WorldStage::Net)
                    .before(crate::ui_script::UiPress),
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
/// `0x80e6ad`-`0x80e712`); a start predicts only while our `UNIT_FIELD_FLAGS_2 & 0x100` is clear
/// (`0x8068a1`-`0x8068b3`).
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
    // The rates off our object; without it a rune's recharge time stays as it was (`0x5ee163`).
    let regen = me.map(|m| regen_of(&m.0));
    // The milliseconds of this frame's `GetTime`, the clock the reference stamps runes on.
    let now_ms = (clock.ui_now * 1000.0) as u32;
    let state = &mut runes.state;
    let events = match ev {
        SessionEvent::ConvertRune(c) => state.convert(c.rune, c.kind),
        SessionEvent::ResyncRunes(r) => state.resync(r.count, &r.runes, now_ms, regen.as_ref()),
        SessionEvent::AddRunePower { mask } => state.add_power(mask, now_ms, regen.as_ref()),
        SessionEvent::CastRunes {
            caster,
            spell_id,
            cast_flags,
            runes: wire,
            start,
        } => {
            let held =
                me.filter(|m| !(start && m.0.unit_flags2() & UNIT_FLAG2_NO_POWER_PREDICT != 0));
            let ours = held.is_some() && Some(caster) == self_guid.0;
            let flagged = cast_flags & CAST_FLAG_POWER_LEFT_SELF != 0
                && cast_flags & CAST_FLAG_RUNE_LIST != 0;
            let rune_spell = spells
                .as_deref()
                .and_then(|s| s.catalog.get(spell_id))
                .is_some_and(|d| d.power_type == POWER_RUNE);
            if !(ours && flagged && rune_spell) {
                return;
            }
            // `ours` holds our object, so its rates are there.
            let Some(regen) = regen.as_ref() else {
                return;
            };
            state.set_predicted(
                u32::from(wire.before),
                u32::from(wire.after),
                Some(&wire.progress),
                now_ms,
                regen,
            )
        }
        _ => return,
    };
    if !events.is_empty() {
        runes.epoch = runes.epoch.wrapping_add(1);
    }
    fire(&mut ui, events);
}

/// The regen tick, which the player's power regen runs for the active player, alive
/// (`0x728a2f`-`0x728a33`) with `UNIT_FIELD_FLAGS_2 & 0x800` (`0x728a45`-`0x728a4b`), while its
/// display power is runic power (`0x728aa1`). A rune coming ready fires from the tick, as the
/// reference's update signals it.
fn tick_runes(
    mut runes: ResMut<Runes>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    clock: Res<crate::ui_script::UiClock>,
    mut ui: ResMut<UiEvents>,
    mut commands: Commands,
) {
    let Some(me) = self_q.iter().next().filter(|me| {
        !me.0.is_dead_or_ghost()
            && me.0.unit_flags2() & UNIT_FLAG2_REGENERATE_POWER != 0
            && me.0.unit_power_type() == POWER_RUNIC_POWER
    }) else {
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
            // The usable update (`0x5ee047`, `0x5ee3c2`).
            RuneEvent::Usable => ui.world(crate::ui_action::usable_cache::step(
                crate::ui_action::usable_cache::Pass::Full,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::field::{
        FIELD_PLAYER_RUNE_REGEN_1, FIELD_UNIT_BYTES_0, FIELD_UNIT_FLAGS_2, FIELD_UNIT_HEALTH,
        FIELD_UNIT_MAXHEALTH,
    };
    use bevy::ecs::system::RunSystemOnce;

    const ME: u64 = 0x10;

    /// A death knight with `health` and `UNIT_FIELD_FLAGS_2` = `flags2`, rune 1 spent at 0 s and due
    /// at 10 s, the clock at 20 s.
    fn world(health: u32, flags2: u32) -> World {
        let mut world = World::new();
        let mut pairs = vec![
            (FIELD_UNIT_HEALTH, health),
            (FIELD_UNIT_MAXHEALTH, 1000),
            (FIELD_UNIT_BYTES_0, u32::from(POWER_RUNIC_POWER) << 24),
            (FIELD_UNIT_FLAGS_2, flags2),
        ];
        pairs.extend((0..4).map(|k| (FIELD_PLAYER_RUNE_REGEN_1 + k, 0.1f32.to_bits())));
        world.spawn((
            SelfPlayer,
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&pairs)),
        ));
        let mut runes = Runes::default();
        runes
            .state
            .set_predicted(0x3f, 0x3e, None, 1, &[0.1; benilla_ui::script::RUNE_KINDS]);
        world.insert_resource(runes);
        world.insert_resource(crate::ui_script::UiClock { ui_now: 20.0 });
        world.init_resource::<UiEvents>();
        world.insert_resource(SelfGuid(Some(ME)));
        world
    }

    /// The regen tick runs alive (`0x728a33`) with `UNIT_FIELD_FLAGS_2 & 0x800` (`0x728a45`): only
    /// then does the due rune come ready.
    #[test]
    fn the_regen_tick_runs_alive_with_the_regenerate_bit() {
        for (health, flags2, readies) in [(1000, 0x800, true), (1000, 0, false), (0, 0x800, false)]
        {
            let mut world = world(health, flags2);
            world.run_system_once(tick_runes).unwrap();
            let ready = world.resource::<Runes>().state.ready;
            assert_eq!(
                ready & 1 != 0,
                readies,
                "health {health} flags2 {flags2:#x}: ready {ready:#x}"
            );
        }
    }

    /// A start predicts our runes only while `UNIT_FIELD_FLAGS_2 & 0x100` is clear
    /// (`0x8068a1`-`0x8068b3`); a go predicts them whatever the bit.
    #[test]
    fn the_start_alone_gates_its_prediction_on_flags_2() {
        let cast = |start| SessionEvent::CastRunes {
            caster: ME,
            spell_id: 45477,
            cast_flags: CAST_FLAG_POWER_LEFT_SELF | CAST_FLAG_RUNE_LIST,
            runes: benilla_protocol::messages::CastRunes {
                before: 0x3f,
                after: 0x3d,
                progress: [0; 6],
            },
            start,
        };
        for (start, flags2, predicts) in
            [(true, 0, true), (true, 0x100, false), (false, 0x100, true)]
        {
            let mut world = world(1000, flags2);
            world.insert_resource(Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(
                        45477,
                        benilla_formats::SpellDisplay {
                            power_type: POWER_RUNE,
                            ..Default::default()
                        },
                    )]
                    .into_iter()
                    .collect(),
                ),
                ..Spells::empty_for_tests()
            });
            world
                .run_system_once_with(on_rune_packet, cast(start))
                .unwrap();
            let ready = world.resource::<Runes>().state.ready;
            assert_eq!(
                ready == 0x3d,
                predicts,
                "start {start} flags2 {flags2:#x}: ready {ready:#x}"
            );
        }
    }
}
