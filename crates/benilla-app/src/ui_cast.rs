//! The unit-cast feed: every unit's cast and channel edges, off its packets, become the
//! `UNIT_SPELLCAST_*` events 3.3.5a's `CastingBarFrame.lua:6-15` registers for, fanned out over the
//! tokens naming the caster (`0x60bf60`: unit, name, rank, castCount), and the cast state
//! `UnitCastingInfo` and `UnitChannelInfo` read. The VM's casting flag is pushed each frame. The
//! own-cast lifecycle it reads is `crate::spell`'s.
//!
//! A cast's end runs one path in the reference (`0x7fecc0`): `SUCCEEDED` for a launch (result
//! `0xbb`), else `FAILED_QUIET`, `INTERRUPTED` or `FAILED` by reason, then `STOP` when it was the
//! unit's running cast (`castCount` and spell, `+0xa5c`/`+0xa6c`).

use std::collections::HashMap;
use std::time::Instant;

use benilla_formats::SpellDisplay;
use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript, UnitCast};

use crate::creature_anim::Casting;
use crate::net::{GuidIndex, NetCommands, ObjectStore, SelfGuid};
use crate::spell::{inflight, PendingCast, QueuedMeleeSpell};
use crate::ui_action::Spells;
use crate::ui_unit::UnitFeed;

/// One edge of a unit's cast, queued by the spell's packet handlers, the cast send and the
/// local cancel.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CastBarEdge {
    /// Our `CMSG_CAST_SPELL` went out (`UNIT_SPELLCAST_SENT`, `0x80b556`), at a unit or none.
    Sent { spell_id: u32, target: Option<u64> },
    /// `SMSG_SPELL_START`; an instant (`cast_time_ms == 0`) opens no cast (`0x805409`).
    Start {
        caster: u64,
        spell_id: u32,
        cast_count: u8,
        cast_time_ms: u32,
    },
    /// `SMSG_SPELL_GO`: the cast launched; `cast_flags & 1` fires nothing (`0x80e747`).
    Go {
        caster: u64,
        spell_id: u32,
        cast_count: u8,
        cast_flags: u32,
    },
    /// `SMSG_CAST_FAILED`, `SMSG_SPELL_FAILURE`, `SMSG_SPELL_FAILED_OTHER`, or our own cancel;
    /// `reason` in the shared numbering.
    Failed {
        caster: u64,
        spell_id: u32,
        cast_count: u8,
        reason: u8,
    },
    /// Our own move or Esc cancel (`0x806200`, reason `0x28`): the running cast ends interrupted.
    Cancelled { caster: u64 },
    /// `SMSG_SPELL_DELAYED`: pushback moves the running cast's end out by `delay_ms`.
    Delayed { caster: u64, delay_ms: u32 },
    /// `MSG_CHANNEL_START`.
    ChannelStart {
        caster: u64,
        spell_id: u32,
        duration_ms: u32,
    },
    /// `MSG_CHANNEL_UPDATE`: the time left, 0 when the channel is over.
    ChannelUpdate { caster: u64, remaining_ms: u32 },
}

/// The queue of [`CastBarEdge`]s, drained by [`feed_cast_bar`].
#[derive(Resource, Default)]
pub(crate) struct CastBarFeed(pub(crate) Vec<CastBarEdge>);

/// Queue `UNIT_SPELLCAST_SENT` from a cast send, which holds no feed borrow: the reference fires it
/// beside `CMSG_CAST_SPELL` (`0x80b556`).
pub(crate) fn queue_sent(ecs: &mut Commands, spell_id: u32, target: Option<u64>) {
    ecs.queue(move |world: &mut World| {
        if let Some(mut feed) = world.get_resource_mut::<CastBarFeed>() {
            feed.0.push(CastBarEdge::Sent { spell_id, target });
        }
    });
}

/// Each unit's running cast and channel as the feed tracks them: `(spell, castCount)` and the
/// window on the `GetTime` clock in ms.
#[derive(Resource, Default)]
pub(crate) struct UnitCasts {
    casts: HashMap<u64, Running>,
    channels: HashMap<u64, Running>,
}

#[derive(Clone, Copy, Debug)]
struct Running {
    spell_id: u32,
    cast_count: u8,
    start_ms: f64,
    end_ms: f64,
}

/// Whether a cast or channel raises its events, the gate both the start (`0x80540f`) and the
/// channel handlers (`0x801cf3`, `0x801e27`) apply: `AttributesEx4 & 0x8000000` always passes,
/// `Attributes & 2` (a ranged-slot spell) never does, the active player passes, and anyone else
/// fails on `Attributes & 0x180` or `AttributesEx4 & 1`, and a channel also on
/// `AttributesEx3 & 0x2000`.
fn raises_events(d: &SpellDisplay, is_self: bool, channel: bool) -> bool {
    if d.attributes_ex4 & 0x0800_0000 != 0 {
        return true;
    }
    if d.attributes & 0x2 != 0 {
        return false;
    }
    if is_self {
        return true;
    }
    d.attributes & 0x180 == 0
        && d.attributes_ex4 & 0x1 == 0
        && !(channel && d.attributes_ex3 & 0x2000 != 0)
}

/// The event an ended cast raises by its reason (`0x7fedd0`-`0x7fee0f`): `DONT_REPORT`,
/// `SPELL_IN_PROGRESS` and `CHARMED` are quiet, the two interrupts are `INTERRUPTED`, the rest
/// `FAILED`.
fn end_event(reason: u8) -> &'static str {
    match crate::ui_action::cast_fail::CAST_FAIL_KEYS
        .get(usize::from(reason))
        .copied()
        .unwrap_or("")
    {
        "SPELL_FAILED_DONT_REPORT" | "SPELL_FAILED_SPELL_IN_PROGRESS" | "SPELL_FAILED_CHARMED" => {
            "UNIT_SPELLCAST_FAILED_QUIET"
        }
        "SPELL_FAILED_INTERRUPTED" | "SPELL_FAILED_INTERRUPTED_COMBAT" => {
            "UNIT_SPELLCAST_INTERRUPTED"
        }
        _ => "UNIT_SPELLCAST_FAILED",
    }
}

/// The spell's `(name, rank)`, `("", "")` for an id with no row.
fn name_rank(spells: Option<&Spells>, id: u32) -> (String, String) {
    spells
        .and_then(|s| s.catalog.get(id))
        .map(|d| (d.name.clone(), d.rank.clone().unwrap_or_default()))
        .unwrap_or_default()
}

/// The event arguments after the token: name, rank, castCount.
fn spell_args(spells: Option<&Spells>, id: u32, cast_count: u8) -> Vec<ScriptValue> {
    let (name, rank) = name_rank(spells, id);
    vec![
        ScriptValue::Str(name),
        ScriptValue::Str(rank),
        ScriptValue::Int(i64::from(cast_count)),
    ]
}

/// The cast as `UnitCastingInfo`/`UnitChannelInfo` read it. A cast's text is its name, empty under
/// `AttributesEx3 & 4` (`0x611e81`); a channel's is its name only with `AttributesEx` bit 29
/// (`0x612151`), else the Lua global `CHANNELING`.
fn unit_cast(script: &UiScript, spells: Option<&Spells>, run: Running, channel: bool) -> UnitCast {
    let display = spells.and_then(|s| s.catalog.get(run.spell_id));
    let (name, rank) = name_rank(spells, run.spell_id);
    let text = match display {
        Some(d) if channel && !d.channel_bar_own_name() => script
            .lua()
            .globals()
            .get::<String>("CHANNELING")
            .unwrap_or_default(),
        Some(d) if !channel && d.no_casting_bar_text() => String::new(),
        _ => name.clone(),
    };
    UnitCast {
        name,
        rank,
        text,
        texture: display.and_then(|d| d.icon.clone()),
        start_ms: run.start_ms,
        end_ms: run.end_ms,
        // `Attributes & 0x20` (`0x611fa8`).
        trade_skill: display.is_some_and(|d| d.attributes & 0x20 != 0),
        cast_count: run.cast_count,
    }
}

/// What the feed reads to name `UNIT_SPELLCAST_SENT`'s target.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SentTarget<'w, 's> {
    names: Res<'w, crate::names::NameCache>,
    commands: Res<'w, NetCommands>,
    stores: Query<'w, 's, &'static ObjectStore>,
}

/// Drain the queue into the `UNIT_SPELLCAST_*` events and the VM's cast state, then push the flag
/// `SpellStopCasting` answers from: auto-repeat or the in-flight slot, never a channel, taken
/// after the local cancel. A queued strike counts, so the first Esc stops it and
/// `ToggleGameMenu` skips `ClearTarget()` (`UIParent.lua:1489-1492`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn feed_cast_bar(
    script: Option<NonSendMut<UiScript>>,
    mut feed: ResMut<CastBarFeed>,
    mut units: ResMut<UnitCasts>,
    spells: Option<Res<Spells>>,
    pending: Res<PendingCast>,
    queued_melee: Res<QueuedMeleeSpell>,
    auto_repeat: Res<crate::spell::AutoRepeatActive>,
    self_guid: Res<SelfGuid>,
    index: Res<GuidIndex>,
    casting: Query<&Casting>,
    sent: SentTarget,
) {
    let Some(mut script) = script else {
        return;
    };
    let now = Instant::now();
    // The auto-repeat key (`0xceac30`), which the reference reads first, then the in-flight slot
    // the cancel resolves, so the flag and the cancel agree.
    let started = self_guid
        .0
        .as_ref()
        .and_then(|g| index.0.get(g))
        .and_then(|&e| casting.get(e).ok())
        .map(|c| c.spell_id);
    script.set_casting(
        auto_repeat.0.is_some() || inflight(&pending, &queued_melee, started, now).is_some(),
    );
    let spells = spells.as_deref();
    let now_ms = script.now() * 1000.0;
    let is_self = |guid: u64| self_guid.0 == Some(guid);
    for edge in feed.0.drain(..) {
        if *crate::net::CAST_TRACE {
            info!("cast-trace: UNIT_SPELLCAST edge {edge:?}");
        }
        match edge {
            CastBarEdge::Sent { spell_id, target } => {
                // `("player", name, rank, target)`, the target's name or `""` (`0x80b533`).
                let target = target
                    .and_then(|g| {
                        let store = index.0.get(&g).and_then(|&e| sent.stores.get(e).ok());
                        sent.names.resolve_unit(g, store, &sent.commands)
                    })
                    .unwrap_or_default()
                    .to_string();
                let (name, rank) = name_rank(spells, spell_id);
                script.fire_event(
                    "UNIT_SPELLCAST_SENT",
                    vec![
                        ScriptValue::Str("player".into()),
                        ScriptValue::Str(name),
                        ScriptValue::Str(rank),
                        ScriptValue::Str(target),
                    ],
                );
            }
            CastBarEdge::Start {
                caster,
                spell_id,
                cast_count,
                cast_time_ms,
            } => {
                let raises = spells
                    .and_then(|s| s.catalog.get(spell_id))
                    .is_none_or(|d| raises_events(d, is_self(caster), false));
                if cast_time_ms == 0 || !raises {
                    continue;
                }
                let run = Running {
                    spell_id,
                    cast_count,
                    start_ms: now_ms,
                    end_ms: now_ms + f64::from(cast_time_ms),
                };
                units.casts.insert(caster, run);
                let cast = unit_cast(&script, spells, run, false);
                script.set_unit_cast(caster, false, Some(cast));
                script.fire_unit_event(
                    caster,
                    "UNIT_SPELLCAST_START",
                    spell_args(spells, spell_id, cast_count),
                );
            }
            CastBarEdge::Go {
                caster,
                spell_id,
                cast_count,
                cast_flags,
            } => {
                if cast_flags & 1 != 0 {
                    continue;
                }
                end_cast(
                    &mut script,
                    &mut units,
                    spells,
                    caster,
                    spell_id,
                    cast_count,
                    "UNIT_SPELLCAST_SUCCEEDED",
                );
            }
            CastBarEdge::Failed {
                caster,
                spell_id,
                cast_count,
                reason,
            } => end_cast(
                &mut script,
                &mut units,
                spells,
                caster,
                spell_id,
                cast_count,
                end_event(reason),
            ),
            CastBarEdge::Cancelled { caster } => {
                let Some(run) = units.casts.get(&caster).copied() else {
                    continue;
                };
                end_cast(
                    &mut script,
                    &mut units,
                    spells,
                    caster,
                    run.spell_id,
                    run.cast_count,
                    "UNIT_SPELLCAST_INTERRUPTED",
                );
            }
            CastBarEdge::Delayed { caster, delay_ms } => {
                let Some(run) = units.casts.get_mut(&caster) else {
                    continue;
                };
                run.end_ms += f64::from(delay_ms);
                let run = *run;
                let cast = unit_cast(&script, spells, run, false);
                script.set_unit_cast(caster, false, Some(cast));
                script.fire_unit_event(
                    caster,
                    "UNIT_SPELLCAST_DELAYED",
                    spell_args(spells, run.spell_id, run.cast_count),
                );
            }
            CastBarEdge::ChannelStart {
                caster,
                spell_id,
                duration_ms,
            } => {
                // No `Spell.dbc` row, no event (`0x801ced`).
                let raises = spells
                    .and_then(|s| s.catalog.get(spell_id))
                    .is_some_and(|d| raises_events(d, is_self(caster), true));
                if !raises {
                    continue;
                }
                let run = Running {
                    spell_id,
                    cast_count: 0,
                    start_ms: now_ms,
                    end_ms: now_ms + f64::from(duration_ms),
                };
                units.channels.insert(caster, run);
                let cast = unit_cast(&script, spells, run, true);
                script.set_unit_cast(caster, true, Some(cast));
                script.fire_unit_event(
                    caster,
                    "UNIT_SPELLCAST_CHANNEL_START",
                    spell_args(spells, spell_id, 0),
                );
            }
            // `0x801e85`: nonzero re-times the channel, zero ends it.
            CastBarEdge::ChannelUpdate {
                caster,
                remaining_ms,
            } => {
                let Some(run) = units.channels.get(&caster).copied() else {
                    continue;
                };
                if remaining_ms == 0 {
                    units.channels.remove(&caster);
                    script.set_unit_cast(caster, true, None);
                    script.fire_unit_event(
                        caster,
                        "UNIT_SPELLCAST_CHANNEL_STOP",
                        spell_args(spells, run.spell_id, 0),
                    );
                } else {
                    let run = Running {
                        end_ms: now_ms + f64::from(remaining_ms),
                        ..run
                    };
                    units.channels.insert(caster, run);
                    let cast = unit_cast(&script, spells, run, true);
                    script.set_unit_cast(caster, true, Some(cast));
                    script.fire_unit_event(
                        caster,
                        "UNIT_SPELLCAST_CHANNEL_UPDATE",
                        spell_args(spells, run.spell_id, 0),
                    );
                }
            }
        }
    }
}

/// `0x7fecc0`: the ending event for any cast, then `STOP` when it was the unit's running one.
fn end_cast(
    script: &mut UiScript,
    units: &mut UnitCasts,
    spells: Option<&Spells>,
    caster: u64,
    spell_id: u32,
    cast_count: u8,
    event: &str,
) {
    script.fire_unit_event(caster, event, spell_args(spells, spell_id, cast_count));
    let running = units
        .casts
        .get(&caster)
        .is_some_and(|r| r.spell_id == spell_id && r.cast_count == cast_count);
    if running {
        units.casts.remove(&caster);
        script.set_unit_cast(caster, false, None);
        script.fire_unit_event(
            caster,
            "UNIT_SPELLCAST_STOP",
            spell_args(spells, spell_id, cast_count),
        );
    }
}

/// The unit-cast feed, drained before the VM ticks so an edge and its first `OnUpdate` share a
/// frame, and after the local cancel so a move or Esc ends the bar on the next tick.
pub(crate) struct UiCastPlugin;

impl Plugin for UiCastPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CastBarFeed>()
            .init_resource::<UnitCasts>()
            .add_systems(
                Update,
                // After the spell's local cancel, whose edges this drains the same frame.
                feed_cast_bar
                    .in_set(UnitFeed)
                    .after(crate::spell::LocalCancel),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(attributes: u32, attributes_ex3: u32, attributes_ex4: u32) -> SpellDisplay {
        SpellDisplay {
            attributes,
            attributes_ex3,
            attributes_ex4,
            ..Default::default()
        }
    }

    #[test]
    fn the_event_gate_follows_the_start_and_channel_handlers() {
        let plain = display(0, 0, 0);
        assert!(raises_events(&plain, false, false));
        assert!(raises_events(&plain, true, true));
        // A ranged-slot spell (Throw, the shots) never, unless Ex4's override.
        assert!(!raises_events(&display(0x2, 0, 0), true, false));
        assert!(raises_events(&display(0x2, 0, 0x0800_0000), false, false));
        // Hidden attributes stop an observer's event, never the player's.
        assert!(!raises_events(&display(0x100, 0, 0), false, false));
        assert!(raises_events(&display(0x100, 0, 0), true, false));
        assert!(!raises_events(&display(0, 0, 0x1), false, false));
        // `AttributesEx3 & 0x2000` (Blood Siphon) gates a channel only.
        assert!(!raises_events(&display(0, 0x2000, 0), false, true));
        assert!(raises_events(&display(0, 0x2000, 0), false, false));
    }

    const ME: u64 = 10;
    const HEAL: u32 = 2050;

    /// A world with the feed's inputs, us as guid 10 and `player`, and Lesser Heal cataloged.
    fn feed_app() -> App {
        let mut app = App::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx))
            .init_resource::<CastBarFeed>()
            .init_resource::<UnitCasts>()
            .init_resource::<PendingCast>()
            .init_resource::<QueuedMeleeSpell>()
            .init_resource::<crate::spell::AutoRepeatActive>()
            .init_resource::<SelfGuid>()
            .init_resource::<GuidIndex>()
            .init_resource::<crate::names::NameCache>()
            .insert_resource(Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(
                        HEAL,
                        SpellDisplay {
                            name: "Lesser Heal".into(),
                            rank: Some("Rank 1".into()),
                            ..Default::default()
                        },
                    )]
                    .into_iter()
                    .collect(),
                ),
                ..Spells::empty_for_tests()
            });
        app.world_mut().resource_mut::<SelfGuid>().0 = Some(ME);
        let mut script = UiScript::new().unwrap();
        script.set_unit_guids(&benilla_ui::script::UnitGuids {
            player: ME,
            ..Default::default()
        });
        script.set_now(100.0);
        script
            .run(
                "SEEN = {} local f = CreateFrame('Frame')
                 for _, e in ipairs({'START', 'STOP', 'FAILED', 'INTERRUPTED', 'SUCCEEDED',
                                     'DELAYED', 'FAILED_QUIET'}) do
                     f:RegisterEvent('UNIT_SPELLCAST_' .. e)
                 end
                 f:SetScript('OnEvent', function(self, e, u, n, r, c)
                     table.insert(SEEN, e:sub(16) .. ':' .. u .. ':' .. n .. ':' .. c) end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app
    }

    fn run(app: &mut App, edges: Vec<CastBarEdge>) -> String {
        use bevy::ecs::system::RunSystemOnce;
        app.world_mut().resource_mut::<CastBarFeed>().0 = edges;
        app.world_mut().run_system_once(feed_cast_bar).unwrap();
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval::<String>("local s = table.concat(SEEN, ' ') SEEN = {} return s")
            .unwrap()
    }

    /// A cast opens on START, reads back through `UnitCastingInfo`, and its launch fires
    /// `SUCCEEDED` then `STOP`; an interrupt fires `INTERRUPTED` then `STOP`; a refusal of a cast
    /// not running fires `FAILED` alone, so the bar (keyed on `castID`) ignores it.
    #[test]
    fn the_feed_raises_the_reference_sequence() {
        let mut app = feed_app();
        let start = |cast_count| CastBarEdge::Start {
            caster: ME,
            spell_id: HEAL,
            cast_count,
            cast_time_ms: 1500,
        };
        assert_eq!(run(&mut app, vec![start(7)]), "START:player:Lesser Heal:7");
        assert_eq!(
            app.world_mut()
                .non_send_resource::<UiScript>()
                .eval::<String>(
                    r#"local n, r, t, i, a, b, ts, id = UnitCastingInfo("player")
                       return n .. "|" .. r .. "|" .. a .. "|" .. b .. "|" .. id"#
                )
                .unwrap(),
            "Lesser Heal|Rank 1|100000|101500|7"
        );
        let go = CastBarEdge::Go {
            caster: ME,
            spell_id: HEAL,
            cast_count: 7,
            cast_flags: 0,
        };
        assert_eq!(
            run(&mut app, vec![go]),
            "SUCCEEDED:player:Lesser Heal:7 STOP:player:Lesser Heal:7"
        );
        assert_eq!(
            app.world_mut()
                .non_send_resource::<UiScript>()
                .arity(r#"UnitCastingInfo("player")"#)
                .unwrap(),
            0
        );

        let interrupted = crate::ui_action::cast_fail::CAST_FAIL_KEYS
            .iter()
            .position(|k| *k == "SPELL_FAILED_INTERRUPTED")
            .unwrap() as u8;
        let failed = |cast_count| CastBarEdge::Failed {
            caster: ME,
            spell_id: HEAL,
            cast_count,
            reason: interrupted,
        };
        assert_eq!(
            run(&mut app, vec![start(8), failed(9), failed(8)]),
            "START:player:Lesser Heal:8 INTERRUPTED:player:Lesser Heal:9 \
             INTERRUPTED:player:Lesser Heal:8 STOP:player:Lesser Heal:8"
        );
        assert_eq!(
            run(
                &mut app,
                vec![start(10), CastBarEdge::Cancelled { caster: ME }]
            ),
            "START:player:Lesser Heal:10 INTERRUPTED:player:Lesser Heal:10 \
             STOP:player:Lesser Heal:10"
        );
        // An instant opens nothing; its launch only succeeds.
        let instant = CastBarEdge::Start {
            caster: ME,
            spell_id: HEAL,
            cast_count: 11,
            cast_time_ms: 0,
        };
        let go11 = CastBarEdge::Go {
            caster: ME,
            spell_id: HEAL,
            cast_count: 11,
            cast_flags: 0,
        };
        assert_eq!(
            run(&mut app, vec![instant, go11]),
            "SUCCEEDED:player:Lesser Heal:11"
        );
    }

    #[test]
    fn the_reason_picks_the_ending_event() {
        let shared = |key: &str| {
            crate::ui_action::cast_fail::CAST_FAIL_KEYS
                .iter()
                .position(|k| *k == key)
                .unwrap() as u8
        };
        assert_eq!(
            end_event(shared("SPELL_FAILED_INTERRUPTED")),
            "UNIT_SPELLCAST_INTERRUPTED"
        );
        assert_eq!(
            end_event(shared("SPELL_FAILED_DONT_REPORT")),
            "UNIT_SPELLCAST_FAILED_QUIET"
        );
        assert_eq!(
            end_event(shared("SPELL_FAILED_BAD_TARGETS")),
            "UNIT_SPELLCAST_FAILED"
        );
    }
}
