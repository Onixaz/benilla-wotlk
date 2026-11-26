//! The 3.3.5a combat log: every combat packet's handler, as 12340 reads it, and the producers the
//! client runs itself (casts, auras, deaths, missile arrivals). Each builds its entries
//! ([`entries`]) and fires what the reference fires beside them: the centre text, `UNIT_COMBAT`
//! and the floating word or number ([`producers`]). The few chat lines 3.3.5a still prints are
//! [`chat`]'s.
//!
//! The completed swing and the environmental-damage log also drive animation; those handlers
//! ([`crate::creature_anim::net`]) register after these.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use crate::combat_text::CombatTextSpawn;
use crate::net::{GuidIndex, NetHandlerApp, ObjectStore, SelfGuid, ServerSoundMessage};
use crate::ui_chat::ChatSink as _;
use crate::ui_unit::CombatTextEvent;

pub(crate) mod chat;
pub(crate) mod entries;
mod producers;
mod progress;
mod store;
#[cfg(test)]
mod tests;

pub(crate) use producers::Log;
pub(crate) use store::{cancel_waits, name_landed, record_landed, CombatLog, Landed};

/// The combat log's handler registration. Added ahead of [`crate::creature_anim::CreatureAnimPlugin`]
/// and [`crate::unit_auras::UnitAurasPlugin`], so the aura handler reads the table before the packet
/// lands in it.
pub(crate) struct CombatLogPlugin;

impl Plugin for CombatLogPlugin {
    fn build(&self, app: &mut App) {
        use SessionEventKind as K;
        app.init_resource::<CombatLog>()
            .init_resource::<entries::CombatLogClock>()
            .init_resource::<entries::CombatLogBackdate>()
            .add_message::<crate::entities::MissileMiss>()
            // The arrivals' misses log in the world frame's paint (`0x47f163`), behind the idle's
            // runners (`0x47f15b`): the world update moves the missiles (`0x4fb031` -> `0x703b00`),
            // ahead of the name text (`0x4fb036`) and the frames' render (`0x4fb090`).
            .add_systems(
                Update,
                log_missile_misses
                    .after(crate::entities::EntityVisualsSet)
                    .after(crate::area::ZoneEventsSet)
                    .after(crate::ui_dialog_verbs::poll_area_spirit_healer)
                    .before(crate::vplates::VPlateSet)
                    .before(crate::ui_script::extract::paint_script)
                    .in_set(crate::ui_script::UiPaint),
            );
        app.net_handler(K::Connected, on_login)
            .net_handler(K::CombatLogBackdate, on_backdate)
            .net_handler(K::AttackerState, on_attacker_state)
            .net_handler(K::SpellDamageLog, on_spell_damage_log)
            .net_handler(K::PeriodicAuraLog, on_periodic_aura_log)
            .net_handler(K::SpellHealLog, on_spell_heal_log)
            .net_handler(K::SpellEnergizeLog, on_spell_energize_log)
            .net_handler(K::DamageShield, on_damage_shield)
            .net_handler(K::SpellLogMiss, on_spell_log_miss)
            .net_handler(K::PartyKillLog, on_party_kill_log)
            .net_handler(K::SpellInstaKillLog, on_spell_insta_kill_log)
            .net_handler(K::ProcResist, on_proc_resist)
            .net_handler(K::SpellOrDamageImmune, on_spell_or_damage_immune)
            .net_handler(K::SpellRemoveLog, on_spell_remove_log)
            .net_handler(K::DispelFailed, on_dispel_failed)
            .net_handler(K::EnchantmentLog, on_enchantment_log)
            .net_handler(K::SpellLogExecute, on_spell_log_execute)
            .net_handler(K::EnvironmentalDamageLog, on_environmental_damage_log)
            .net_handler(K::BuildingDamage, on_building_damage)
            .net_handler(K::DestroyedOnDeath, on_destroyed_on_death)
            .net_handler(K::SpellStart, on_spell_start)
            .net_handler(K::SpellGo, on_spell_go)
            .net_handler(K::AuraUpdate, on_aura_update)
            .net_handler(K::XpGain, on_xp_gain)
            .net_handler(K::ExplorationXp, on_exploration_xp)
            .net_handler(K::LevelUp, on_level_up)
            .net_handler(K::PvpCredit, on_pvp_credit);
    }
}

/// A login clears the log and takes its timestamp base (`0x756bd0` -> `0x7538a0`).
fn on_login(In(ev): In<SessionEvent>, mut ui: ResMut<crate::ui_script::live::UiEvents>) {
    if let SessionEvent::Connected { .. } = ev {
        ui.world(store::reset);
    }
}

fn on_backdate(In(ev): In<SessionEvent>, mut backdate: ResMut<entries::CombatLogBackdate>) {
    if let SessionEvent::CombatLogBackdate { ms } = ev {
        backdate.0 = ms;
    }
}

fn on_attacker_state(In(ev): In<SessionEvent>, mut log: Log) {
    if let SessionEvent::AttackerState(s) = ev {
        log.swing(&s);
    }
}

fn on_spell_damage_log(In(ev): In<SessionEvent>, mut log: Log) {
    if let SessionEvent::SpellDamageLog(s) = ev {
        if benilla_assets::trace::enabled() {
            benilla_assets::trace::line(
                "fct",
                &format!(
                    "recv spelldmg atk={:#x} target={:#x} dmg={} crit={}",
                    s.caster,
                    s.target,
                    s.damage,
                    s.hit_info & benilla_protocol::messages::spell_hit_info::CRIT != 0
                ),
            );
        }
        log.spell_damage_log(&s);
    }
}

fn on_periodic_aura_log(In(ev): In<SessionEvent>, mut log: Log) {
    if let SessionEvent::PeriodicAuraLog(l) = ev {
        log.periodic_aura_log(&l);
    }
}

fn on_spell_heal_log(In(ev): In<SessionEvent>, mut log: Log) {
    if let SessionEvent::SpellHealLog(h) = ev {
        log.spell_heal_log(&h);
    }
}

fn on_spell_energize_log(In(ev): In<SessionEvent>, mut log: Log) {
    if let SessionEvent::SpellEnergizeLog(e) = ev {
        log.spell_energize_log(&e);
    }
}

fn on_damage_shield(In(ev): In<SessionEvent>, mut log: Log) {
    if let SessionEvent::DamageShield(s) = ev {
        log.damage_shield(&s);
    }
}

fn on_spell_log_miss(In(ev): In<SessionEvent>, mut log: Log) {
    if let SessionEvent::SpellLogMiss(m) = ev {
        log.spell_log_miss(&m);
    }
}

fn on_party_kill_log(In(ev): In<SessionEvent>, mut log: Log) {
    if let SessionEvent::PartyKillLog(k) = ev {
        log.party_kill(&k);
    }
}

fn on_spell_insta_kill_log(In(ev): In<SessionEvent>, mut log: Log) {
    if let SessionEvent::SpellInstaKillLog(k) = ev {
        log.instakill(&k);
    }
}

fn on_proc_resist(In(ev): In<SessionEvent>, mut log: Log) {
    if let SessionEvent::ProcResist(o) = ev {
        log.proc_resist(&o);
    }
}

/// `0x6ce3d0`: an immunity, a periodic one when the byte says so, else a damage shield's.
fn on_spell_or_damage_immune(In(ev): In<SessionEvent>, mut log: Log) {
    if let SessionEvent::SpellOrDamageImmune(o) = ev {
        log.spell_missed(o.caster, o.target, o.spell_id, 7, !o.periodic, o.periodic);
    }
}

fn on_spell_remove_log(In(ev): In<SessionEvent>, mut log: Log) {
    if let SessionEvent::SpellRemoveLog(l) = ev {
        log.spell_remove_log(&l);
    }
}

fn on_dispel_failed(In(ev): In<SessionEvent>, mut log: Log) {
    if let SessionEvent::DispelFailed(f) = ev {
        log.dispel_failed(&f);
    }
}

fn on_enchantment_log(In(ev): In<SessionEvent>, mut log: Log) {
    if let SessionEvent::EnchantmentLog(e) = ev {
        log.enchantment(&e);
    }
}

fn on_spell_log_execute(In(ev): In<SessionEvent>, mut log: Log) {
    if let SessionEvent::SpellLogExecute(x) = ev {
        log.spell_log_execute(&x);
    }
}

fn on_environmental_damage_log(In(ev): In<SessionEvent>, mut log: Log) {
    if let SessionEvent::EnvironmentalDamageLog(e) = ev {
        log.environmental(&e);
    }
}

fn on_building_damage(In(ev): In<SessionEvent>, mut log: Log) {
    if let SessionEvent::BuildingDamage(b) = ev {
        log.building_damage(&b);
    }
}

fn on_destroyed_on_death(
    In(ev): In<SessionEvent>,
    mut log: Log,
    passengers: Query<&crate::vehicle::Passenger>,
) {
    if let SessionEvent::DestroyedOnDeath(guid) = ev {
        // The passenger record's seated state (`[unit+0xf60]` == 3, read at `0x729277`).
        let seated = log
            .ctx
            .entity(guid)
            .and_then(|e| passengers.get(e).ok())
            .is_some_and(|p| p.state == crate::vehicle::SEATED);
        log.destroyed_on_death(guid, seated);
    }
}

/// `OnSpellStartPacket` (`0x806700`): a shown cast logs its start.
fn on_spell_start(In(ev): In<SessionEvent>, mut log: Log) {
    if let SessionEvent::SpellStart {
        caster,
        spell_id,
        cast_flags,
        ..
    } = ev
    {
        if cast_flags & 0x1 == 0 {
            log.cast_start(caster, spell_id);
        }
    }
}

fn on_spell_go(In(ev): In<SessionEvent>, mut log: Log) {
    if let SessionEvent::SpellGo {
        caster,
        spell_id,
        cast_flags,
        misses,
        reflects,
        target,
        go_target,
        item_caster,
        ..
    } = ev
    {
        let object = target.or(go_target).unwrap_or(0);
        log.spell_go(
            caster,
            item_caster,
            spell_id,
            cast_flags,
            object,
            &misses,
            &reflects,
        );
    }
}

/// `OnAuraUpdate`'s log (`0x72f5d0`): the unit's table before the packet, diffed against it.
fn on_aura_update(
    In(ev): In<SessionEvent>,
    mut log: Log,
    auras: Query<&crate::unit_auras::UnitAuras>,
    clock: Res<Time<bevy::time::Real>>,
) {
    use crate::unit_auras::{AuraEdge, UnitAuras};
    let SessionEvent::AuraUpdate(update) = ev else {
        return;
    };
    let Some(entity) = log.ctx.entity(update.unit) else {
        return;
    };
    if !log.ctx.store(update.unit).is_some_and(ObjectStore::is_unit) {
        return;
    }
    let empty = UnitAuras::default();
    let table = auras.get(entity).unwrap_or(&empty);
    let unit = update.unit;
    for edge in table.edges(&update, clock.elapsed_secs_f64()) {
        match edge {
            AuraEdge::Added(a) => {
                log.aura_added_or_removed(unit, a.caster, a.spell_id, a.is_harmful(), true)
            }
            AuraEdge::Removed(a) => {
                log.aura_added_or_removed(unit, a.caster, a.spell_id, a.is_harmful(), false)
            }
            AuraEdge::Dose { old, aura } => log.aura_dose(
                unit,
                aura.caster,
                aura.spell_id,
                old,
                aura.stacks,
                aura.is_harmful(),
            ),
            AuraEdge::Refreshed(a) => {
                log.aura_refreshed(unit, a.caster, a.spell_id, a.is_harmful())
            }
        }
    }
}

/// A unit's death log (`0x7561ed` -> `0x752ed0`), at its death's step in the update's field watch.
pub(crate) fn unit_dead(world: &mut World, guid: u64) {
    run_producer(world, guid, |log, guid| log.unit_dead(guid));
}

/// The player's spell failure (`0x751ad0`), logged by the refusal display behind its line
/// (`0x808200`).
pub(crate) fn spell_fail(world: &mut World, spell: u32, reason: String) {
    run_producer(world, (spell, reason), |log, (spell, reason)| {
        log.spell_fail(spell, &reason);
    });
}

/// A producer and its input.
type Producer<I> = (I, fn(&mut Log, I));

/// Run one producer at the caller's step; what it queues runs in the caller's turn.
fn run_producer<I: Send + Sync + 'static>(world: &mut World, input: I, producer: fn(&mut Log, I)) {
    let ran = world.run_system_cached_with(
        |In((input, producer)): In<Producer<I>>, mut log: Log| producer(&mut log, input),
        (input, producer),
    );
    if let Err(e) = ran {
        warn!("combat_log: a producer did not run: {e}");
    }
}

/// The frame's missile arrivals that missed, each logged at its arrival (`0x702fc0`): the word,
/// the feedback and the entry, whose events fire before the next arrival's.
pub(crate) fn log_missile_misses(world: &mut World) {
    let misses = world
        .run_system_cached(
            |mut arrivals: MessageReader<crate::entities::MissileMiss>| {
                arrivals.read().copied().collect::<Vec<_>>()
            },
        )
        .unwrap_or_default();
    for m in misses {
        if let Err(e) = world.run_system_cached_with(missile_missed, m) {
            warn!("combat_log: a missile miss was not logged: {e}");
        }
        crate::ui_script::live::fire_handler_events(world);
    }
}

fn missile_missed(
    In(m): In<crate::entities::MissileMiss>,
    guids: Query<&crate::net::Guid>,
    mut log: Log,
) {
    let (Ok(caster), Ok(target)) = (guids.get(m.caster), guids.get(m.anchor)) else {
        return;
    };
    log.cast_missed(caster.0, target.0, m.spell_id, m.code);
}

/// The XP number and line, off their own sinks.
#[derive(bevy::ecs::system::SystemParam)]
struct Progress<'w, 's> {
    index: Res<'w, GuidIndex>,
    self_guid: Res<'w, SelfGuid>,
    stores: Query<'w, 's, &'static ObjectStore>,
    text: MessageWriter<'w, CombatTextSpawn>,
    chat: ResMut<'w, crate::ui_script::live::UiEvents>,
    center: MessageWriter<'w, CombatTextEvent>,
}

fn on_xp_gain(
    In(ev): In<SessionEvent>,
    mut p: Progress,
    rest_states: Option<Res<crate::ui_unit::RestStates>>,
) {
    if let SessionEvent::XpGain(x) = ev {
        let me = p.self_guid.0.and_then(|g| p.index.0.get(&g).copied());
        let line = progress::xp_line(
            &x,
            p.self_guid.0,
            me.and_then(|e| p.stores.get(e).ok()),
            |guid| {
                p.index
                    .0
                    .get(&guid)
                    .and_then(|&e| p.stores.get(e).ok())
                    .is_some_and(|s| s.0.is_unit())
            },
            rest_states.as_deref(),
        );
        progress::xp_gain(x, me, line, &mut p.text, &mut p.chat);
    }
}

fn on_exploration_xp(
    In(ev): In<SessionEvent>,
    mut p: Progress,
    area_table: Option<Res<crate::area::AreaTableRes>>,
    exploration_sounds: Option<Res<crate::sound::ExplorationSounds>>,
    mut server_sounds: MessageWriter<ServerSoundMessage>,
) {
    if let SessionEvent::ExplorationXp(x) = ev {
        progress::exploration_xp(
            x,
            area_table.as_deref(),
            exploration_sounds.as_deref(),
            &p.index,
            &p.self_guid,
            &p.stores,
            &mut server_sounds,
            &mut p.chat,
        );
    }
}

fn on_level_up(
    In(ev): In<SessionEvent>,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
    mut tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
) {
    if let SessionEvent::LevelUp(lv) = ev {
        progress::level_tutorials(lv.level, tutorials.as_mut());
        progress::level_up(lv, &mut ui);
    }
}

/// An honor award: the line (its victim's name read in the call) and the `HONOR_GAINED` number.
/// A dishonorable kill carries negative honor, passed signed.
fn on_pvp_credit(In(ev): In<SessionEvent>, mut p: Progress) {
    if let SessionEvent::PvpCredit(credit) = ev {
        p.chat
            .push_pvp_credit(credit.honor, credit.victim_guid, credit.victim_rank);
        p.center.write(CombatTextEvent {
            message_type: "HONOR_GAINED",
            args: vec![benilla_ui::script::ScriptValue::Int(i64::from(
                credit.honor,
            ))],
        });
    }
}
