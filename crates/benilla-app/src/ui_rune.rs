//! The player's runes (12340 `CGRuneInfo`): the three rune packets, the rune block of our own
//! casts, the regen tick, and the table the rune verbs read, with `RUNE_POWER_UPDATE` and
//! `RUNE_TYPE_UPDATE`.

use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_formats::SpellRuneCostCatalog;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{RuneEvent, RuneState, ScriptValue, UiScript};

use crate::net::{EnteredWorldMessage, NetHandlerApp, ObjectStore, SelfGuid, SelfPlayer};
use crate::ui_action::Spells;
use crate::ui_unit::UnitFeed;

/// `castFlags` the start and go handlers require before predicting runes (`0x806742`,
/// `0x806766`): the caster's power left, and the rune block.
const CAST_FLAG_POWER_LEFT_SELF: u32 = 0x800;
const CAST_FLAG_RUNE_LIST: u32 = 0x20_0000;

/// `POWER_RUNE`: a spell of this power type spends runes (`0x8068c0`, `0x80e6ad`).
const POWER_RUNE: u32 = 5;

/// `POWER_RUNIC_POWER`: the display power the regen tick checks for (`0x728aa1`).
const POWER_RUNIC_POWER: u8 = 6;

/// The rune table and the packets waiting for the feed.
#[derive(Resource, Default)]
pub(crate) struct Runes {
    pub(crate) state: RuneState,
    pending: Vec<SessionEvent>,
    /// Moves whenever a packet or the regen tick changed the table, for gated feeds.
    pub(crate) epoch: u64,
}

/// `SpellRuneCost.dbc`; empty without client data.
#[derive(Resource, Default)]
pub(crate) struct RuneCosts(pub(crate) SpellRuneCostCatalog);

pub(crate) struct UiRunePlugin;

impl Plugin for UiRunePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Runes>()
            .init_resource::<RuneCosts>()
            .net_handler(SessionEventKind::ConvertRune, queue_rune_packet)
            .net_handler(SessionEventKind::ResyncRunes, queue_rune_packet)
            .net_handler(SessionEventKind::AddRunePower, queue_rune_packet)
            .net_handler(SessionEventKind::CastRunes, queue_rune_packet)
            .add_systems(Startup, load_rune_costs.after(AssetSet::Open))
            .add_systems(
                Update,
                feed_runes
                    .in_set(UnitFeed)
                    .after(benilla_world::schedule::WorldStage::Net)
                    // Before `PLAYER_ENTERING_WORLD`, whose stock handler reads every rune's kind,
                    // and the cooldown readers, whose rune leg reads the table.
                    .before(crate::ui_unit::EnterWorldFeed)
                    .before(crate::ui_spellbook::BookFeed)
                    .before(crate::ui_action::CooldownEvents),
            );
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

fn queue_rune_packet(In(ev): In<SessionEvent>, mut runes: ResMut<Runes>) {
    if matches!(
        ev,
        SessionEvent::ConvertRune(_)
            | SessionEvent::ResyncRunes(_)
            | SessionEvent::AddRunePower { .. }
            | SessionEvent::CastRunes { .. }
    ) {
        runes.pending.push(ev);
    }
}

/// The packets in arrival order, the regen tick, the push and the events.
#[allow(clippy::too_many_arguments)] // a Bevy system's full input set
fn feed_runes(
    script: Option<NonSendMut<UiScript>>,
    mut runes: ResMut<Runes>,
    mut entered: MessageReader<EnteredWorldMessage>,
    self_guid: Res<SelfGuid>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    spells: Option<Res<Spells>>,
    clock: Res<crate::ui_script::UiClock>,
    mut pushed: Local<crate::ui_script::VmMemo<Option<RuneState>>>,
) {
    // `CGGameUI::EnterWorld` starts a fresh table (`0x528294` into `0x5edef0`).
    if entered.read().count() > 0 {
        runes.state = RuneState::default();
    }
    let me = self_q.iter().next();
    // The milliseconds of this frame's `GetTime`, the clock the reference stamps runes on.
    let now_ms = (clock.ui_now * 1000.0) as u32;
    let Runes { state, pending, .. } = &mut *runes;
    if let Some(me) = me {
        state.regen = std::array::from_fn(|kind| me.0.player_rune_regen(kind as u8));
    }
    let mut events = Vec::new();
    for ev in pending.drain(..) {
        match ev {
            SessionEvent::ConvertRune(c) => events.extend(state.convert(c.rune, c.kind)),
            SessionEvent::ResyncRunes(r) => events.extend(state.resync(r.count, &r.runes, now_ms)),
            SessionEvent::AddRunePower { mask } => events.extend(state.add_power(mask, now_ms)),
            // Our own rune spell's start or go (`0x8068c0`-`0x8068f6`, `0x80e6ad`-`0x80e712`).
            // The start's `UNIT_FLAG2` `0x100` gate is not built: the layout has no `FLAGS_2`.
            SessionEvent::CastRunes {
                caster,
                spell_id,
                cast_flags,
                runes,
            } => {
                let ours = me.is_some() && Some(caster) == self_guid.0;
                let flagged = cast_flags & CAST_FLAG_POWER_LEFT_SELF != 0
                    && cast_flags & CAST_FLAG_RUNE_LIST != 0;
                let rune_spell = spells
                    .as_deref()
                    .and_then(|s| s.catalog.get(spell_id))
                    .is_some_and(|d| d.power_type == POWER_RUNE);
                if ours && flagged && rune_spell {
                    events.extend(state.set_predicted(
                        u32::from(runes.before),
                        u32::from(runes.after),
                        Some(&runes.progress),
                        now_ms,
                    ));
                }
            }
            _ => {}
        }
    }
    // The regen tick runs for the active player while its display power is runic power
    // (`0x728aa1`); its `UNIT_FLAG2_REGENERATE_POWER` gate (`0x728a45`) is not built.
    if me.is_some_and(|me| me.0.unit_power_type() == POWER_RUNIC_POWER) {
        events.extend(state.tick(now_ms));
    }

    if !events.is_empty() {
        runes.epoch = runes.epoch.wrapping_add(1);
    }
    let Some(mut script) = script else {
        return;
    };
    let last = pushed.get(&script);
    if last.as_ref() != Some(&runes.state) {
        script.set_runes(runes.state.clone());
        *last = Some(runes.state.clone());
    }
    for event in events {
        match event {
            RuneEvent::Power { rune, usable } => script.fire_event(
                "RUNE_POWER_UPDATE",
                vec![ScriptValue::Int(i64::from(rune)), ScriptValue::Bool(usable)],
            ),
            RuneEvent::Type { rune } => {
                script.fire_event("RUNE_TYPE_UPDATE", vec![ScriptValue::Int(i64::from(rune))])
            }
            RuneEvent::ActionBarCooldown => script.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]),
            // The action feed reads the table every frame and fires its own usable events.
            RuneEvent::Usable => {}
        }
    }
}
