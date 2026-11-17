//! Death and resurrection: the wire-fed stores, the events into the Lua UI and the intents back
//! to the wire. Dead is health 0 and ghost is `PLAYER_FLAGS` bit 0x10, read off the self
//! descriptor each frame; `PLAYER_ALIVE` fires on release and on a pre-release res,
//! `PLAYER_UNGHOST` on ghost to alive.

use bevy::prelude::*;

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_ui::script::{DeathAction, ScriptValue, UiScript};

use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::ui_action::Spells;
use crate::ui_script::live::UiEvents;
use crate::ui_script::{UiDrain, UiFeed};

pub(crate) mod net;

/// Where our corpse is, the `MSG_CORPSE_QUERY` answer, in raw WoW coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CorpsePoint {
    /// The map to route toward; for a corpse inside an instance, the entrance's.
    pub(crate) display_map: i32,
    /// The corpse, or the dungeon entrance standing in for it, in the world: the answer's point,
    /// carried by its transport when it has one ([`resolve_corpse_position`]).
    pub(crate) position: [f32; 3],
    /// The answer's point as sent, transport-local when `transport` is set.
    pub(crate) local: [f32; 3],
    /// The corpse's real map id, never adjusted.
    pub(crate) corpse_map: u32,
    /// The low guid of the transport the corpse lies on, 0 for none.
    pub(crate) transport: u32,
}

/// The graveyard a release goes to (`SMSG_DEATH_RELEASE_LOC`), in raw WoW coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ReleasePoint {
    pub(crate) map: u32,
    pub(crate) position: [f32; 3],
}

/// A pending resurrection offer (`SMSG_RESURRECT_REQUEST`), the RESURRECT popup's data.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResurrectOffer {
    /// Answered in `CMSG_RESURRECT_RESPONSE`.
    pub(crate) caster: u64,
    /// Empty on the wire for a player caster, resolved through the name cache.
    pub(crate) name: String,
    /// Accepting applies resurrection sickness; picks the popup variant.
    pub(crate) sickness: bool,
    /// The reclaim-delay gate still holds: RESURRECT, not RESURRECT_NO_TIMER.
    pub(crate) has_timer: bool,
    /// An unnamed caster whose name query is still out (`0x6dbc60` answers it).
    pub(crate) awaiting_name: bool,
}

/// The death state the descriptor does not carry, filled from the wire and cleared at session
/// teardown; the verbs read [`DeathGlobals`] instead.
#[derive(Resource, Default)]
pub(crate) struct DeathNet {
    /// The last `MSG_CORPSE_QUERY` answer; a not-found drops it.
    pub(crate) corpse: Option<CorpsePoint>,
    /// The corpse-range latch (`0xbd0820`): set while in range, so the events fire on its edges.
    pub(crate) in_range: bool,
    /// Our streamed corpse object's guid, which `CMSG_RECLAIM_CORPSE` carries like the 1.12 client.
    pub(crate) corpse_guid: Option<u64>,
    /// Water-walking is granted on our mover; mirrored, not applied: walking on the water surface
    /// is not built.
    #[allow(dead_code)]
    pub(crate) water_walk: bool,
    /// The release graveyard, kept while not a ghost at arrival (`0x6e2e90`): the world map's
    /// `GetDeathReleasePosition` and the minimap's spirit-healer POI (`0x51f690`).
    pub(crate) release: Option<ReleasePoint>,
    /// The corpse's transport in the world, `SMSG_CORPSE_MAP_POSITION_QUERY_RESPONSE`'s position
    /// and facing (`0x512bd0`), read while the transport is not streamed.
    pub(crate) corpse_transport_frame: Option<([f32; 3], f32)>,
    /// When `CMSG_CORPSE_MAP_POSITION_QUERY` may go again, on the real clock: 30 s after a send,
    /// cleared while the transport resolves (`0x51f430`).
    pub(crate) corpse_map_query_at: Option<f64>,
}

/// The UI globals the death handlers write and the death verbs read at call time, written only by
/// handlers and the client actions that run as exclusive commands.
#[derive(Resource, Default)]
pub(crate) struct DeathGlobals {
    /// The release deadline on the real clock (`0xbd0848`).
    release_at: Option<f64>,
    /// The no-timer flag (`0xbd084c`): `GetReleaseTimeRemaining` answers -1.
    no_release_timer: bool,
    /// When the corpse becomes reclaimable, on the real clock (`0xbd0850`).
    pub(crate) reclaim_at: Option<f64>,
    /// The last resurrect offer (`0xc9eab8`-`0xc9eac4`); an answer zeroes its caster alone.
    pub(crate) resurrect: Option<ResurrectOffer>,
    /// The spirit healer asked about (`0xc9eb10`), which `AcceptXPLoss` answers.
    pub(crate) spirit_healer: Option<u64>,
}

impl DeathGlobals {
    /// A UI load zeroes the release deadline, its flag and the reclaim deadline (`0x52a9f6`,
    /// `0x52a9fc`, `0x52aa02`); the resurrect offer and the healer asked about stay.
    pub(crate) fn reset_at_ui_load(&mut self) {
        self.release_at = None;
        self.no_release_timer = false;
        self.reclaim_at = None;
    }

    /// The release timer's derivation (`0x6dc070`): no release window reads 0 at once, the timer
    /// bit arms 360 s unless out of bounds, anything else sets the no-timer flag (`0x513a60`).
    fn update_repop_timer(&mut self, fields: &benilla_protocol::ObjectFields, now: f64) {
        let (at, no_timer) = if fields.player_no_release_window() {
            (Some(now), false)
        } else if fields.player_release_timer_running()
            && fields.player_flags() & PLAYER_FLAGS_OUT_OF_BOUNDS == 0
        {
            (Some(now + RELEASE_WINDOW_SECS), false)
        } else {
            (None, true)
        };
        self.release_at = at;
        self.no_release_timer = no_timer;
    }

    /// `GetReleaseTimeRemaining`'s milliseconds (`0x516210`): `None` under the no-timer flag.
    fn release_left_ms(&self, now: f64) -> Option<u32> {
        (!self.no_release_timer).then(|| left_ms(self.release_at, now))
    }
}

/// Milliseconds from `now` to `at`, 0 when unset or passed.
fn left_ms(at: Option<f64>, now: f64) -> u32 {
    at.map_or(0.0, |at| {
        ((at - now) * 1000.0).clamp(0.0, f64::from(u32::MAX))
    }) as u32
}

/// Set by our own fresh create, taken at the next `PLAYER_ENTERING_WORLD`: the active player's
/// init enters the world and then refreshes the dead or alive signal (`0x6e818b`), which a UI
/// load's own entry does not (`0x52ae84`).
#[derive(Resource, Default)]
pub(crate) struct ActivePlayerInit(pub(crate) bool);

/// The forced-release window, 360000 ms (`0x6dc0b6`); never on the wire.
const RELEASE_WINDOW_SECS: f64 = 360.0;

/// What the death verbs read at call time, part of the VM's live view: only state no system
/// holding the VM writes, so every holder can lend it.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct DeathView<'w, 's> {
    globals: Option<Res<'w, DeathGlobals>>,
    time: Option<Res<'w, Time<Real>>>,
    me: Query<
        'w,
        's,
        (
            &'static ObjectStore,
            Option<&'static crate::unit_auras::UnitAuras>,
        ),
        With<SelfPlayer>,
    >,
    /// Where we stand, which the spirit-healer reach measures from.
    me_at: Query<'w, 's, &'static Transform, (With<SelfPlayer>, With<ObjectStore>)>,
    /// Where each held object stands, which the spirit-healer reach measures to.
    places: Query<'w, 's, (&'static ObjectStore, &'static Transform)>,
    index: Option<Res<'w, crate::net::GuidIndex>>,
    /// Our mover, whose flags `IsFalling` tests.
    player: Option<Res<'w, crate::player::Player>>,
    objects: Option<Objects<'w, 's>>,
    items: Option<Res<'w, crate::items::Items>>,
    spells: Option<Res<'w, Spells>>,
    net: Option<Res<'w, NetCommands>>,
    arena: crate::ui_arena::ArenaInstance<'w>,
}

impl DeathView<'_, '_> {
    fn now(&self) -> f64 {
        self.time.as_ref().map_or(0.0, |t| t.elapsed_secs_f64())
    }

    pub(crate) fn release_time_left_ms(&self) -> Option<u32> {
        self.globals
            .as_ref()
            .map_or(Some(0), |g| g.release_left_ms(self.now()))
    }

    pub(crate) fn corpse_recovery_delay_ms(&self) -> u32 {
        left_ms(self.globals.as_ref().and_then(|g| g.reclaim_at), self.now())
    }

    pub(crate) fn resurrect_sickness(&self) -> bool {
        self.offer().is_some_and(|o| o.sickness)
    }

    /// Outside an arena alone (`0x515a00`, the battlefield word `[0xbea570] == 4`).
    pub(crate) fn resurrect_has_timer(&self) -> bool {
        !self.arena.active() && self.offer().is_some_and(|o| o.has_timer)
    }

    fn offer(&self) -> Option<&ResurrectOffer> {
        self.globals.as_ref()?.resurrect.as_ref()
    }

    /// `CheckSpiritHealerDist` (`0x6d1fc0`): the healer asked about, held, within its reach of us.
    pub(crate) fn spirit_healer_in_range(&self) -> bool {
        let reach = self
            .globals
            .as_ref()
            .and_then(|g| g.spirit_healer)
            .zip(self.index.as_deref())
            .and_then(|(guid, index)| healer_reach(guid, index, &self.places));
        self.me_at
            .single()
            .is_ok_and(|at| within_spirit_healer_reach(at.translation, reach))
    }

    /// The minutes a spirit-healer res would apply at our level ([`sickness_minutes`]).
    pub(crate) fn res_sickness_minutes(&self) -> Option<u32> {
        let (store, _) = self.me.single().ok()?;
        sickness_minutes(store.0.unit_level().unwrap_or(0))
    }

    /// `HasSoulstone` (`0x51acd0`); the dead gate is first, so the inventory walk runs only while
    /// dead.
    pub(crate) fn self_res_label(&self) -> Option<String> {
        let (store, _) = self.me.single().ok()?;
        resolve_self_res(
            &store.0,
            self.objects.as_ref()?,
            self.items.as_deref()?,
            self.spells.as_deref(),
            self.net.as_deref()?,
        )
        .map(|r| r.label().to_owned())
    }

    /// `IsFalling` (`0x612430`): our mover's flags word.
    pub(crate) fn falling(&self) -> bool {
        self.player
            .as_ref()
            .is_some_and(|p| is_falling(p.move_flags()))
    }

    pub(crate) fn out_of_bounds(&self) -> bool {
        self.me
            .single()
            .is_ok_and(|(s, _)| s.0.player_flags() & PLAYER_FLAGS_OUT_OF_BOUNDS != 0)
    }

    /// `CannotBeResurrected` (`0x802270`): a live aura effect of ours.
    pub(crate) fn cannot_be_resurrected(&self) -> bool {
        let Ok((_, Some(auras))) = self.me.single() else {
            return false;
        };
        prevents_resurrection(auras, |id| {
            self.spells
                .as_deref()
                .and_then(|s| s.catalog.get(id))
                .map(|d| d.effect_apply_aura)
        })
    }
}

/// The active-player refresh (`0x520f70`): `PLAYER_DEAD` at or below 0 health, with a cursor clear
/// (`0x520fab`), else `PLAYER_ALIVE`.
fn active_player_refresh(health: u32) -> (&'static str, bool) {
    if health == 0 {
        ("PLAYER_DEAD", true)
    } else {
        ("PLAYER_ALIVE", false)
    }
}

/// The corpse-range dialog radius, 40 yd inclusive: 12340 compares d² ≤ 1600.0 against the
/// corpse-query position (`0x51f663`). The server's reclaim gate, `CORPSE_RECLAIM_RADIUS` 39
/// (`Corpse.h:40`), sits 1 yd inside it.
const CORPSE_RANGE_SQ: f32 = 40.0 * 40.0;

/// The spirit-healer reach past the healer's combat reach (`0x6d1fc0`, `0x6db010`).
const SPIRIT_HEALER_REACH: f32 = 4.0;

/// `PLAYER_FLAGS_IS_OUT_OF_BOUNDS`, the bit `IsOutOfBounds` tests (`0x6123eb`).
const PLAYER_FLAGS_OUT_OF_BOUNDS: u32 = 0x4000;

/// `SPELL_AURA_PREVENT_RESURRECTION`, the aura-effect bit `CannotBeResurrected` tests
/// (`0x802292`: byte `+0xf47` bit 2 is effect 314).
const AURA_PREVENT_RESURRECTION: u32 = 314;

/// `IsFalling`'s test (`0x61245b`): falling and not rooted.
fn is_falling(move_flags: u32) -> bool {
    use crate::creature_anim::move_flags::{FALLING, ROOT};
    move_flags & FALLING != 0 && move_flags & ROOT == 0
}

/// Whether a live effect of the unit's auras is [`AURA_PREVENT_RESURRECTION`]: each aura's effect
/// bits name which of its spell's three effects are applied.
fn prevents_resurrection(
    auras: &crate::unit_auras::UnitAuras,
    effects: impl Fn(u32) -> Option<[u32; 3]>,
) -> bool {
    auras.iter().any(|a| {
        effects(a.spell_id).is_some_and(|types| {
            (0..3).any(|i| a.flags & (1 << i) != 0 && types[i] == AURA_PREVENT_RESURRECTION)
        })
    })
}

/// The self-resurrect check (`0x727860`) before `CMSG_SELF_RES`: with a live
/// [`AURA_PREVENT_RESURRECTION`] effect, a self-res spell without `AttributesEx7 & 0x8000000` is
/// refused with the custom line named by the first such aura's effect misc value; its walk reads
/// all three effects, not the live bits.
fn self_res_refusal(
    auras: &crate::unit_auras::UnitAuras,
    self_res_spell: u32,
    spell: impl Fn(u32) -> Option<(u32, [u32; 3], [i32; 3])>,
) -> Option<i32> {
    if !prevents_resurrection(auras, |id| spell(id).map(|(_, types, _)| types)) {
        return None;
    }
    if spell(self_res_spell).is_some_and(|(ex7, ..)| ex7 & 0x800_0000 != 0) {
        return None;
    }
    auras.iter().find_map(|a| {
        let (_, types, misc) = spell(a.spell_id)?;
        (0..3)
            .find(|&i| types[i] == AURA_PREVENT_RESURRECTION)
            .map(|i| misc[i])
    })
}

/// The state the death handlers write, lent from each handler's parameters.
pub(crate) struct DeathCx<'a> {
    pub(crate) globals: &'a mut DeathGlobals,
    pub(crate) death_net: &'a mut DeathNet,
    pub(crate) ui: &'a mut UiEvents,
    pub(crate) net: &'a NetCommands,
    /// Real-clock seconds.
    pub(crate) now: f64,
    /// The active battlefield is an arena (`[0xbea570] == 4`).
    pub(crate) arena: bool,
}

/// Our own descriptor's death fields, as their mirror handlers take them once the whole update has
/// applied (`0x4d5550`), in field order: health (`0x73f330`), the dynamic dead bit (`0x740570`)
/// and the ghost bit (`0x6df710`). `edges` are `(index, old, new)`.
pub(crate) fn own_fields_changed(
    edges: &[(u16, u32, u32)],
    fields: &benilla_protocol::ObjectFields,
    cx: &mut DeathCx,
) {
    use benilla_protocol::field::{
        FIELD_PLAYER_FLAGS, FIELD_UNIT_DYNAMIC_FLAGS, FIELD_UNIT_HEALTH,
    };
    for &(index, old, new) in edges {
        match index {
            // Health crossing zero: the death or, from zero, the life (`0x73f330`).
            FIELD_UNIT_HEALTH => {
                if (old as i32) > 0 && (new as i32) < 1 {
                    let dead_flag = fields.unit_dynamic_flags() & UNIT_DYNFLAG_DEAD != 0;
                    on_death(fields, dead_flag, cx);
                } else if (old as i32) < 1 && (new as i32) > 0 {
                    // `0x73d5bc`, for our own unit alone.
                    cx.ui.fire("PLAYER_ALIVE", Vec::new());
                }
            }
            // The dead bit set is a death; cleared, a life that fires nothing (`0x740570`).
            FIELD_UNIT_DYNAMIC_FLAGS if (old ^ new) & UNIT_DYNFLAG_DEAD != 0 => {
                if new & UNIT_DYNFLAG_DEAD != 0 {
                    on_death(fields, true, cx);
                }
            }
            FIELD_PLAYER_FLAGS if (old ^ new) & PLAYER_FLAGS_GHOST != 0 => {
                update_ghost_status(fields.player_is_ghost(), cx);
            }
            _ => {}
        }
    }
}

/// `UNIT_DYNFLAG_DEAD`.
const UNIT_DYNFLAG_DEAD: u32 = 0x20;
/// `PLAYER_FLAGS_GHOST`.
const PLAYER_FLAGS_GHOST: u32 = 0x10;

/// Our death (`0x729220`): the player's half derives the release timer unless the dead bit is set
/// (`0x6dc0f0`), then the death anim refreshes the dead or alive signal (`0x7561e0`).
fn on_death(fields: &benilla_protocol::ObjectFields, dead_flag: bool, cx: &mut DeathCx) {
    if !dead_flag {
        cx.globals.update_repop_timer(fields, cx.now);
    }
    refresh_active_player(fields.unit_health().unwrap_or(0), cx.ui);
}

/// The active-player refresh (`0x520f70`): `PLAYER_DEAD` with a cursor clear at or below 0
/// health (`0x520fa2`), else `PLAYER_ALIVE` (`0x520fb9`).
pub(crate) fn refresh_active_player(health: u32, ui: &mut UiEvents) {
    let (event, clear_cursor) = active_player_refresh(health);
    ui.fire(event, Vec::new());
    if clear_cursor {
        ui.clear_cursor();
    }
}

/// The ghost bit's watcher (`0x6df710`): leaving it drops the release point and fires
/// `PLAYER_UNGHOST` (`0x6df75c`); either way the corpse location updates.
fn update_ghost_status(ghost: bool, cx: &mut DeathCx) {
    if !ghost {
        cx.death_net.release = None;
        cx.ui.fire("PLAYER_UNGHOST", Vec::new());
    }
    update_corpse_location(ghost, cx);
}

/// `0x524a30`: drop the corpse point and the range latch, firing `CORPSE_OUT_OF_RANGE` on the
/// latch outside an arena (`0x524a72`), and ask where the corpse is while a ghost outside one.
pub(crate) fn update_corpse_location(ghost: bool, cx: &mut DeathCx) {
    cx.death_net.corpse = None;
    if std::mem::take(&mut cx.death_net.in_range) && !cx.arena {
        cx.ui.fire("CORPSE_OUT_OF_RANGE", Vec::new());
    }
    if ghost && !cx.arena {
        let _ = cx.net.0.send(ClientCommand::CorpseQuery);
    }
}

/// The world-enter cascade's death part, after `PLAYER_ENTERING_WORLD` (`0x528010`): the corpse
/// location, the release point and, dead, the automatic release request (`0x6d2950` with 1);
/// then our own create's init refreshes the dead or alive signal (`0x6e818b`).
pub(crate) fn enter_world_cascade(
    mut init: ResMut<ActivePlayerInit>,
    self_q: Query<(&ObjectStore, Option<&crate::unit_auras::UnitAuras>), With<SelfPlayer>>,
    mut globals: ResMut<DeathGlobals>,
    mut death_net: ResMut<DeathNet>,
    mut ui: ResMut<UiEvents>,
    net: Res<NetCommands>,
    arena: crate::ui_arena::ArenaInstance,
    time: Res<Time<Real>>,
    spells: Option<Res<Spells>>,
) {
    let created = std::mem::take(&mut init.0);
    let Ok((store, auras)) = self_q.single() else {
        return;
    };
    let mut cx = DeathCx {
        globals: &mut globals,
        death_net: &mut death_net,
        ui: &mut ui,
        net: &net,
        now: time.elapsed_secs_f64(),
        arena: arena.active(),
    };
    update_corpse_location(store.0.player_is_ghost(), &mut cx);
    cx.death_net.release = None;
    let health = store.0.unit_health().unwrap_or(0);
    if health == 0 && may_release(&store.0, auras, spells.as_deref()) {
        let _ = net.0.send(ClientCommand::RepopRequest { automatic: true });
    }
    if created {
        refresh_active_player(health, cx.ui);
    }
}

/// The release request's gate (`0x6d2950`): dead or a ghost, and the self-res check passing
/// (`0x727860`).
pub(crate) fn may_release(
    fields: &benilla_protocol::ObjectFields,
    auras: Option<&crate::unit_auras::UnitAuras>,
    spells: Option<&Spells>,
) -> bool {
    fields.is_dead_or_ghost()
        && auras.is_none_or(|a| {
            self_res_refusal(a, fields.player_self_res_spell().unwrap_or(0), |id| {
                let d = spells?.catalog.get(id)?;
                Some((d.attributes_ex7, d.effect_apply_aura, d.effect_misc_value))
            })
            .is_none()
        })
}

/// Queue the world-enter cascade's death part to run once the calling system's commands apply,
/// its events fired with the `World` lent.
pub(crate) fn queue_enter_world(commands: &mut Commands) {
    commands.queue(|world: &mut World| {
        if let Err(e) = world.run_system_cached(enter_world_cascade) {
            warn!("death: the world-enter cascade did not run: {e}");
        }
        crate::ui_script::live::fire_handler_events(world);
    });
}

/// Talking to a spirit healer (`0x6db010`), a ghost alone: a healer within its combat reach
/// plus 4 yd is stored and asked about with `CONFIRM_XP_LOSS` (`0x6db103`); `npc` 0 answers the
/// stored one with `CMSG_SPIRIT_HEALER_ACTIVATE` instead.
pub(crate) fn talk_to_spirit_healer(
    npc: u64,
    me: Option<(&ObjectStore, &Transform)>,
    healer_of: impl Fn(u64) -> Option<(f32, Vec3)>,
    globals: &mut DeathGlobals,
    ui: &mut UiEvents,
    net: &NetCommands,
) {
    let Some((store, at)) = me.filter(|(s, _)| s.0.player_is_ghost()) else {
        return;
    };
    let guid = if npc == 0 {
        globals.spirit_healer.unwrap_or(0)
    } else {
        npc
    };
    if !within_spirit_healer_reach(at.translation, healer_of(guid)) {
        return;
    }
    if npc == 0 {
        let _ = net
            .0
            .send(ClientCommand::SpiritHealerActivate { npc: guid });
        return;
    }
    globals.spirit_healer = Some(npc);
    // The XP the res costs: `PLAYER_NEXT_LEVEL_XP` x 0.05, truncated; no stock FrameXML reads it.
    let xp_cost = store
        .0
        .player_next_level_xp()
        .map_or(0, |next| (f64::from(next) * 0.05) as i64);
    ui.fire("CONFIRM_XP_LOSS", vec![ScriptValue::Int(xp_cost)]);
}

/// `0x6d1fc0`'s test: the held healer's combat reach (`0x6d2023`) plus 4 yd, squared, inclusive.
pub(crate) fn within_spirit_healer_reach(me: Vec3, healer: Option<(f32, Vec3)>) -> bool {
    healer.is_some_and(|(radius, at)| {
        let reach = radius + SPIRIT_HEALER_REACH;
        at.distance_squared(me) <= reach * reach
    })
}

/// `0x6d1d30`: answer the held offer's caster with `CMSG_RESURRECT_RESPONSE`, then zero the caster
/// (`0x6d1da6`), so a second answer sends nothing (`0x6d1d41`); the sickness and timer bytes stay.
pub(crate) fn answer_resurrect(world: &mut World, accept: bool) {
    let Some(caster) = world
        .get_resource_mut::<DeathGlobals>()
        .and_then(|mut g| g.resurrect.as_mut().map(|o| std::mem::take(&mut o.caster)))
        .filter(|&caster| caster != 0)
    else {
        return;
    };
    if let Some(net) = world.get_resource::<NetCommands>() {
        let _ = net
            .0
            .send(ClientCommand::ResurrectResponse { caster, accept });
    }
}

/// The spirit-healer talk as a command, for a click: run with the `World` and its event fired.
pub(crate) fn queue_spirit_healer_talk(commands: &mut Commands, npc: u64) {
    commands.queue(move |world: &mut World| {
        if let Err(e) = world.run_system_cached_with(spirit_healer_talk_system, npc) {
            warn!("death: the spirit healer talk did not run: {e}");
        }
        crate::ui_script::live::fire_handler_events(world);
    });
}

/// [`talk_to_spirit_healer`] over the `World`.
pub(crate) fn spirit_healer_talk_system(
    In(npc): In<u64>,
    self_q: Query<(&ObjectStore, &Transform), With<SelfPlayer>>,
    index: Res<crate::net::GuidIndex>,
    units: Query<(&ObjectStore, &Transform)>,
    mut globals: ResMut<DeathGlobals>,
    mut ui: ResMut<UiEvents>,
    net: Res<NetCommands>,
) {
    talk_to_spirit_healer(
        npc,
        self_q.single().ok(),
        |guid| healer_reach(guid, &index, &units),
        &mut globals,
        &mut ui,
        &net,
    );
}

/// A held unit's `UNIT_FIELD_COMBATREACH` and place.
pub(crate) fn healer_reach(
    guid: u64,
    index: &crate::net::GuidIndex,
    units: &Query<(&ObjectStore, &Transform)>,
) -> Option<(f32, Vec3)> {
    let (store, t) = units.get(*index.0.get(&guid)?).ok()?;
    store
        .0
        .is_unit()
        .then(|| (store.0.unit_combat_reach(), t.translation))
}

/// The corpse distance each world-frame update (`0x4fa040` -> `0x51f5c0`): the corpse POI, the
/// 40 yd range test against the corpse point (`0x51f663`, inclusive) and its latch's events
/// (`0x512c40`), and the release POI as its point changes (`0x51f690`), named from the VM.
fn update_corpse_distance(
    (script, _live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    mut death_net: ResMut<DeathNet>,
    self_q: Query<&Transform, With<SelfPlayer>>,
    arena: crate::ui_arena::ArenaInstance,
    mut corpse_poi: ResMut<CorpsePoi>,
    mut release_poi: ResMut<ReleasePoi>,
) {
    let Some(mut script) = script else {
        return;
    };
    if release_poi.from != death_net.release {
        let name = global_text(&script, "SPIRIT_HEALER_RELEASE_RED");
        release_poi.from = death_net.release;
        release_poi.poi = death_net
            .release
            .map(|r| static_poi(name, r.map, r.position));
    }
    let corpse = death_net.corpse.filter(|cp| cp.display_map >= 0);
    if corpse_poi.from != corpse {
        corpse_poi.from = corpse;
        corpse_poi.poi = corpse.map(|cp| {
            let name = global_text(&script, "CORPSE_RED");
            static_poi(name, cp.display_map as u32, cp.position)
        });
    }
    let in_range = corpse.is_some_and(|cp| {
        self_q.single().is_ok_and(|t| {
            wow_to_bevy(cp.position).distance_squared(t.translation) <= CORPSE_RANGE_SQ
        })
    });
    if in_range != death_net.in_range {
        death_net.in_range = in_range;
        if !arena.active() {
            let event = match corpse {
                Some(cp) if in_range => corpse_range_event(cp),
                _ => "CORPSE_OUT_OF_RANGE",
            };
            script.fire_event(event, Vec::new());
        }
    }
}

/// The in-range event: `CORPSE_IN_INSTANCE` when the display map is not the corpse's
/// (`0x512c40`), at a dungeon's entrance.
pub(crate) fn corpse_range_event(cp: CorpsePoint) -> &'static str {
    if u32::try_from(cp.display_map) == Ok(cp.corpse_map) {
        "CORPSE_IN_RANGE"
    } else {
        "CORPSE_IN_INSTANCE"
    }
}
/// A corpse answer's transport guid: the low word under the MO_TRANSPORT high word, its sign bit
/// spread as `0x523db0` spreads it.
pub(crate) fn corpse_transport_guid(transport: u32) -> u64 {
    let high = ((transport as i32) >> 31) as u32 | 0x1FC0_0000;
    (u64::from(high) << 32) | u64::from(transport)
}

/// `0x51f430`: a corpse on a transport rides it. A streamed transport carries the local point by
/// its own pose; else `CMSG_CORPSE_MAP_POSITION_QUERY` asks for the transport's place, at most
/// every 30 s, and the last answer's position and facing carry the point.
fn resolve_corpse_position(
    mut death_net: ResMut<DeathNet>,
    index: Res<crate::net::GuidIndex>,
    transforms: Query<&Transform>,
    net: Res<NetCommands>,
    time: Res<Time<Real>>,
) {
    let Some(cp) = death_net.corpse.filter(|cp| cp.transport != 0) else {
        return;
    };
    let pose = index
        .0
        .get(&corpse_transport_guid(cp.transport))
        .and_then(|&e| transforms.get(e).ok());
    let world = if let Some(t) = pose {
        if death_net.corpse_map_query_at.is_some() {
            death_net.corpse_map_query_at = None;
        }
        bevy_to_wow(t.translation + t.rotation * wow_to_bevy(cp.local))
    } else {
        let now = time.elapsed_secs_f64();
        if death_net.corpse_map_query_at.is_none_or(|at| now >= at) {
            let _ = net.0.send(ClientCommand::CorpseMapPositionQuery {
                transport: cp.transport,
            });
            death_net.corpse_map_query_at = Some(now + 30.0);
        }
        transport_frame_point(cp.local, death_net.corpse_transport_frame)
    };
    if cp.position != world {
        if let Some(cp) = death_net.corpse.as_mut() {
            cp.position = world;
        }
    }
}

/// A transport-local point through the transport's world position and facing, `0x512bd0`'s
/// matrix: a turn about Z, then the move. No answer yet is the identity.
fn transport_frame_point(local: [f32; 3], frame: Option<([f32; 3], f32)>) -> [f32; 3] {
    let ([ox, oy, oz], facing) = frame.unwrap_or(([0.0; 3], 0.0));
    let (s, c) = facing.sin_cos();
    let [x, y, z] = local;
    [x * c - y * s + ox, x * s + y * c + oy, z + oz]
}

/// The release graveyard's minimap POI (`0x7f4a00`): `POIIcons` cell 8 at importance -1 with the
/// in-range icon bit, named by `SPIRIT_HEALER_RELEASE_RED`. `0x51f690` places it only for the
/// current map; here it is kept for the release map and drawn on it.
#[derive(Resource, Default)]
pub(crate) struct ReleasePoi {
    from: Option<ReleasePoint>,
    poi: Option<benilla_formats::AreaPoi>,
}

impl ReleasePoi {
    /// The POI if it lies on `map_id`.
    pub(crate) fn on_map(&self, map_id: u32) -> Option<&benilla_formats::AreaPoi> {
        self.poi.as_ref().filter(|p| p.continent_id == map_id)
    }
}

/// The corpse's minimap POI (`0x7f4990`): `POIIcons` cell 8 at importance -1 with the in-range
/// icon bit, named by `CORPSE_RED`; the one slot exempt from the 694.444-yd cut (`0x7f46b1`).
#[derive(Resource, Default)]
pub(crate) struct CorpsePoi {
    from: Option<CorpsePoint>,
    poi: Option<benilla_formats::AreaPoi>,
}

impl CorpsePoi {
    /// The POI if it lies on `map_id`.
    pub(crate) fn on_map(&self, map_id: u32) -> Option<&benilla_formats::AreaPoi> {
        self.poi.as_ref().filter(|p| p.continent_id == map_id)
    }
}

/// A global string from the stock UI, empty when it is missing.
fn global_text(script: &UiScript, key: &str) -> String {
    script
        .lua()
        .globals()
        .get::<String>(key)
        .unwrap_or_default()
}

/// The corpse and spirit-healer static POI slots' shared shape (`0x7f4990`, `0x7f4a00`): icon 8,
/// importance -1, flags 2, on `map` at `pos`.
fn static_poi(name: String, map: u32, pos: [f32; 3]) -> benilla_formats::AreaPoi {
    benilla_formats::AreaPoi {
        importance: u32::MAX, // -1, ahead of every landmark band
        icon: 8,
        icons: [8; 9],
        faction_id: 0,
        pos: [pos[0], pos[1], 0.0],
        continent_id: map,
        flags: 0x2,
        area_id: 0,
        name,
        description: String::new(),
        world_state_id: 0,
        map_link: 0,
    }
}

/// `SPELL_EFFECT_SELF_RESURRECT`, the `Spell.dbc` effect the item leg scans for.
const SPELL_EFFECT_SELF_RESURRECT: u32 = 94;

/// What `UseSoulstone()` would spend now; resolved app-side, where the spell catalog and item cache
/// live.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SelfRes {
    /// `PLAYER_SELF_RES_SPELL` is non-zero: send `CMSG_SELF_RES` and the server casts it.
    Spell { spell: u32, label: String },
    /// The field is zero but a carried item's on-use spell self-resurrects: use the item, named
    /// for the item.
    Item {
        bag_index: u8,
        slot: u8,
        guid: u64,
        entry: u32,
        label: String,
    },
}

impl SelfRes {
    /// What `HasSoulstone()` returns.
    fn label(&self) -> &str {
        match self {
            Self::Spell { label, .. } | Self::Item { label, .. } => label,
        }
    }
}

/// `HasSoulstone()`'s answer and `UseSoulstone()`'s routing, one predicate in the 1.12 client, with
/// its gates in its order:
///
/// 1. Dead: nil while `UNIT_FIELD_HEALTH` is positive; a released ghost has health 1.
/// 2. The field: non-zero is the spell leg, named from `Spell.dbc`, `"UNKNOWN"` when unresolved.
/// 3. On a zero field, the carried inventory at the walker's default mask (`0x47`) for an item
///    whose on-use spell carries [`SPELL_EFFECT_SELF_RESURRECT`].
///
/// An item template still in flight reads as no self-res until it lands.
fn resolve_self_res(
    store: &benilla_protocol::ObjectFields,
    objects: &Objects,
    items: &crate::items::Items,
    spells: Option<&Spells>,
    commands: &NetCommands,
) -> Option<SelfRes> {
    if !store.unit_is_dead() {
        return None;
    }
    if let Some(spell) = store.player_self_res_spell() {
        let label = spells
            .and_then(|s| s.catalog.get(spell))
            .map_or_else(|| "UNKNOWN".to_owned(), |d| d.name.clone());
        return Some(SelfRes::Spell { spell, label });
    }
    let slots = crate::ui_items::collect_inventory(
        store,
        objects,
        crate::ui_items::InventoryScope::DEFAULT,
    );
    slots.into_iter().find_map(|(bag_index, slot, guid)| {
        let entry = objects.object(guid)?.object_entry()?;
        let t = items.template(entry, guid, commands)?;
        let self_res = t.spells.iter().any(|sp| {
            sp.trigger == 0
                && spells.is_some_and(|c| {
                    c.catalog
                        .get(sp.spell_id)
                        .is_some_and(|d| d.effects.contains(&SPELL_EFFECT_SELF_RESURRECT))
                })
        });
        self_res.then(|| SelfRes::Item {
            bag_index,
            slot,
            guid,
            entry,
            label: t.name.clone(),
        })
    })
}

/// The minutes of sickness a spirit-healer res applies at `level`, per vmangos
/// `Player::ResurrectPlayer`: none below 11, `level - 10` through 19, 10 from 20.
///
/// Deviation: the server's table, not spell 15007's `Spell.dbc` duration, because the aura the
/// server applies is what the dialog must warn about.
fn sickness_minutes(level: u32) -> Option<u32> {
    match level {
        0..=10 => None,
        11..=19 => Some(level - 10),
        _ => Some(10),
    }
}

/// Drive the FFXDeath screen pass off `PLAYER_FLAGS_GHOST`, instant both ways; in the reference the
/// flag's watcher also drives the death light and ambience.
fn drive_death_look(
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut ffx: ResMut<benilla_world::ffx_glow::FfxDeathFade>,
) {
    let ghost = ghost_probe()
        .unwrap_or_else(|| self_q.single().is_ok_and(|store| store.0.player_is_ghost()));
    let target = if ghost { 1.0 } else { 0.0 };
    if ffx.0 != target {
        ffx.0 = target;
    }
}

/// `WOW_GHOST_PROBE=1|0`: pin the ghost-world look on or off without dying: the screen pass,
/// the death light (`LightParams` slot 4) and the DeathClouds sky. The death events still follow
/// the wire.
pub(crate) fn ghost_probe() -> Option<bool> {
    static PROBE: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    *PROBE.get_or_init(|| match std::env::var("WOW_GHOST_PROBE").ok()?.trim() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    })
}

/// Map the queued Lua death intents onto the wire, after `UiInput` so a click drains that frame.
fn drain_death(
    (script, _live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    death_net: Res<DeathNet>,
    mut commands: Commands,
    net: Res<NetCommands>,
    targeting: crate::spell::cast_target::CastTargeting,
    mut ladder: crate::spell::CastLadder,
    mut ui_errors: ResMut<crate::ui_action::UiErrorKeys>,
    self_auras: Query<&crate::unit_auras::UnitAuras, With<SelfPlayer>>,
    // The soulstone's item leg is an item use, so it carries the bind gate.
    mut gate: crate::ui_bind_confirm::BindGate,
) {
    let Some(mut script) = script else {
        return;
    };
    for action in script.take_death_actions() {
        let _ = match action {
            DeathAction::Repop => net.0.send(ClientCommand::RepopRequest { automatic: false }),
            // Else 0: vmangos resolves the corpse through the player and never reads the guid
            // (`MiscHandler.cpp:573-603`).
            DeathAction::RetrieveCorpse => net.0.send(ClientCommand::ReclaimCorpse {
                corpse: death_net.corpse_guid.unwrap_or(0),
            }),
            DeathAction::AcceptResurrect | DeathAction::DeclineResurrect => {
                let accept = action == DeathAction::AcceptResurrect;
                commands.queue(move |world: &mut World| answer_resurrect(world, accept));
                Ok(())
            }
            // The stored healer, re-checked for reach (`0x51b6a0` -> `0x6db010` with 0).
            DeathAction::AcceptXpLoss => {
                queue_spirit_healer_talk(&mut commands, 0);
                Ok(())
            }
            // Re-resolved at click time, as the reference's `OnCancel` calls `HasSoulstone()`
            // again; nothing is cleared here, the server zeroes `PLAYER_SELF_RES_SPELL`.
            DeathAction::UseSoulstone => {
                let store = targeting.self_store.single().ok().map(|s| s.0.clone());
                match store.as_ref().and_then(|store| {
                    resolve_self_res(
                        store,
                        &ladder.objects,
                        &ladder.items,
                        ladder.spells.as_deref(),
                        &ladder.commands,
                    )
                }) {
                    Some(SelfRes::Spell { spell, .. }) => {
                        let refusal = self_auras.single().ok().and_then(|auras| {
                            self_res_refusal(auras, spell, |id| {
                                let d = ladder.spells.as_deref()?.catalog.get(id)?;
                                Some((d.attributes_ex7, d.effect_apply_aura, d.effect_misc_value))
                            })
                        });
                        match refusal {
                            // The check's custom line; nothing goes out.
                            Some(id) => {
                                ladder.cast_errors.push_local_arg(
                                    spell,
                                    crate::ui_action::cast_fail::CUSTOM_ERROR,
                                    id as u32,
                                );
                                Ok(())
                            }
                            None => net.0.send(ClientCommand::SelfRes),
                        }
                    }
                    // An ordinary item use, like every bag click.
                    Some(SelfRes::Item {
                        bag_index,
                        slot,
                        guid,
                        entry,
                        ..
                    }) => {
                        let t = ladder
                            .items
                            .template(entry, guid, &ladder.commands)
                            .cloned();
                        let it = crate::ui_items::ItemUse {
                            guid: Some(guid),
                            start_quest: t.as_ref().map_or(0, |t| t.start_quest),
                            bag_index,
                            slot,
                            entry,
                            use_spell: t.as_ref().and_then(|t| t.use_spell).map(|u| u.spell_id),
                            on_object: None,
                            is_charter: false,
                        };
                        crate::ui_items::send_item_use(
                            it,
                            &targeting.context(),
                            &mut ladder,
                            &mut script,
                            &mut gate,
                            false,
                            &mut ui_errors,
                        );
                        Ok(())
                    }
                    // Nothing to spend: the reference returns silently.
                    None => Ok(()),
                }
            }
        };
    }
}

/// The death stores, the feed into the UI, the intent drain and the root and water-walk messages.
pub(crate) struct DeathPlugin;

impl Plugin for DeathPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<DeathNet>()
            .init_resource::<DeathGlobals>()
            .init_resource::<ActivePlayerInit>()
            .init_resource::<ReleasePoi>()
            .init_resource::<CorpsePoi>()
            .add_systems(
                Update,
                (
                    drain_death.in_set(UiDrain),
                    drive_death_look,
                    // After the transport chain, so a corpse rides this frame's deck pose.
                    resolve_corpse_position
                        .in_set(UiFeed)
                        .after(crate::transport::ground_deck_riders)
                        .after(crate::net::enter_world_on_self_create),
                    update_corpse_distance
                        .in_set(UiFeed)
                        .after(resolve_corpse_position)
                        .run_if(crate::ui_script::ingame_ui_up),
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::field::{
        FIELD_PLAYER_FLAGS, FIELD_UNIT_DYNAMIC_FLAGS, FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH,
    };
    use benilla_protocol::messages::ObjectFields;

    struct Rig {
        globals: DeathGlobals,
        death_net: DeathNet,
        ui: UiEvents,
        net: NetCommands,
        rx: crossbeam_channel::Receiver<ClientCommand>,
        arena: bool,
    }

    impl Rig {
        fn new() -> Self {
            let (tx, rx) = crossbeam_channel::unbounded();
            Self {
                globals: DeathGlobals::default(),
                death_net: DeathNet::default(),
                ui: UiEvents::default(),
                net: NetCommands(tx),
                rx,
                arena: false,
            }
        }

        /// Our descriptor now `pairs`, the edges `(index, old, new)`.
        fn changed(&mut self, pairs: &[(u16, u32)], edges: &[(u16, u32, u32)]) -> Vec<&str> {
            let fields = ObjectFields::from_pairs(pairs);
            let mut cx = DeathCx {
                globals: &mut self.globals,
                death_net: &mut self.death_net,
                ui: &mut self.ui,
                net: &self.net,
                now: 10.0,
                arena: self.arena,
            };
            own_fields_changed(edges, &fields, &mut cx);
            let names = self.ui.names();
            self.ui = UiEvents::default();
            names
        }

        fn sent(&self) -> Vec<ClientCommand> {
            self.rx.try_iter().collect()
        }
    }

    /// The release timer's three arms (`0x6dc070`): no window reads 0 at once, the timer bit arms
    /// 360 s unless out of bounds, anything else sets the no-timer flag; a UI load zeroes both,
    /// and the reclaim deadline with them.
    #[test]
    fn the_release_timer_derives_from_the_window_bits_and_the_bounds_flag() {
        // `PLAYER_FIELD_BYTES` and `PLAYER_FLAGS`.
        let player = |bytes: u32, flags: u32| {
            ObjectFields::from_pairs(&[
                (benilla_protocol::field::FIELD_PLAYER_FIELD_BYTES, bytes),
                (benilla_protocol::field::FIELD_PLAYER_FLAGS, flags),
            ])
        };
        let mut g = DeathGlobals::default();
        assert_eq!(g.release_left_ms(5.0), Some(0), "a fresh UI reads 0");
        g.update_repop_timer(&player(0x10 | 0x08, 0), 5.0);
        assert_eq!(g.release_left_ms(5.0), Some(0));
        g.update_repop_timer(&player(0x08, 0), 5.0);
        assert_eq!(g.release_left_ms(65.0), Some(300_000));
        g.update_repop_timer(&player(0x08, PLAYER_FLAGS_OUT_OF_BOUNDS), 5.0);
        assert_eq!(g.release_left_ms(5.0), None);
        g.update_repop_timer(&player(0, 0), 5.0);
        assert_eq!(g.release_left_ms(5.0), None, "the no-timer flag");
        g.reclaim_at = Some(35.0);
        g.reset_at_ui_load();
        assert_eq!(g.release_left_ms(5.0), Some(0), "a UI load zeroes both");
        assert_eq!(g.reclaim_at, None, "and the reclaim deadline");
        assert_eq!(active_player_refresh(0), ("PLAYER_DEAD", true));
        assert_eq!(active_player_refresh(1), ("PLAYER_ALIVE", false));
    }

    /// Health crossing zero is the death (`0x73f330` -> `0x729220`): the timer first, then
    /// `PLAYER_DEAD` with the cursor clear (`0x520fa2`); the release's health 1 is the life
    /// (`0x73d5bc`); the ghost bit set asks where the corpse is and cleared fires
    /// `PLAYER_UNGHOST` (`0x6df710`).
    #[test]
    fn our_death_fields_fire_from_their_watchers_in_field_order() {
        let mut rig = Rig::new();
        let dead = [(FIELD_UNIT_MAXHEALTH, 100), (FIELD_UNIT_HEALTH, 0)];
        assert_eq!(
            rig.changed(&dead, &[(FIELD_UNIT_HEALTH, 100, 0)]),
            ["PLAYER_DEAD", "<clear cursor>"]
        );
        assert_eq!(
            rig.globals.release_left_ms(10.0),
            None,
            "derived: no timer bit"
        );

        // The release: health and the ghost bit in one update, health's watcher first.
        rig.death_net.release = Some(ReleasePoint {
            map: 0,
            position: [0.0; 3],
        });
        let ghost = [
            (FIELD_UNIT_MAXHEALTH, 100),
            (FIELD_UNIT_HEALTH, 1),
            (FIELD_PLAYER_FLAGS, 0x10),
        ];
        assert_eq!(
            rig.changed(
                &ghost,
                &[(FIELD_UNIT_HEALTH, 0, 1), (FIELD_PLAYER_FLAGS, 0, 0x10)]
            ),
            ["PLAYER_ALIVE"]
        );
        assert!(matches!(
            rig.sent().as_slice(),
            [ClientCommand::CorpseQuery]
        ));
        assert!(
            rig.death_net.release.is_some(),
            "a ghost keeps the release point"
        );

        // The reclaim: the ghost bit drops, health stays above zero.
        let alive = [(FIELD_UNIT_MAXHEALTH, 100), (FIELD_UNIT_HEALTH, 50)];
        assert_eq!(
            rig.changed(
                &alive,
                &[(FIELD_UNIT_HEALTH, 1, 50), (FIELD_PLAYER_FLAGS, 0x10, 0)]
            ),
            ["PLAYER_UNGHOST"]
        );
        assert!(rig.death_net.release.is_none());
        assert!(rig.sent().is_empty(), "no ask while alive");
    }

    /// The dynamic dead bit is a death that leaves the release timer alone (`0x6dc0f0`), and its
    /// clear fires nothing (`0x740570`); a health still above zero answers `PLAYER_ALIVE`.
    #[test]
    fn the_dynamic_dead_bit_is_a_death_without_the_timer() {
        let mut rig = Rig::new();
        let flagged = [
            (FIELD_UNIT_MAXHEALTH, 100),
            (FIELD_UNIT_HEALTH, 0),
            (FIELD_UNIT_DYNAMIC_FLAGS, 0x20),
        ];
        assert_eq!(
            rig.changed(&flagged, &[(FIELD_UNIT_DYNAMIC_FLAGS, 0, 0x20)]),
            ["PLAYER_DEAD", "<clear cursor>"]
        );
        assert_eq!(rig.globals.release_left_ms(10.0), Some(0), "never derived");
        assert!(rig
            .changed(&flagged, &[(FIELD_UNIT_DYNAMIC_FLAGS, 0x20, 0)])
            .is_empty());
        let feigning = [(FIELD_UNIT_MAXHEALTH, 100), (FIELD_UNIT_HEALTH, 40)];
        assert_eq!(
            rig.changed(&feigning, &[(FIELD_UNIT_DYNAMIC_FLAGS, 0, 0x20)]),
            ["PLAYER_ALIVE"]
        );
    }

    /// `0x524a30`: the range latch's drop fires `CORPSE_OUT_OF_RANGE` and the ghost asks, both
    /// outside an arena only.
    #[test]
    fn the_corpse_update_unlatches_and_asks_outside_an_arena() {
        let mut rig = Rig::new();
        rig.death_net.in_range = true;
        let ghost = [
            (FIELD_UNIT_MAXHEALTH, 100),
            (FIELD_UNIT_HEALTH, 1),
            (FIELD_PLAYER_FLAGS, 0x10),
        ];
        assert_eq!(
            rig.changed(&ghost, &[(FIELD_PLAYER_FLAGS, 0, 0x10)]),
            ["CORPSE_OUT_OF_RANGE"]
        );
        assert!(!rig.death_net.in_range);
        assert_eq!(rig.sent().len(), 1);
        rig.arena = true;
        rig.death_net.in_range = true;
        assert!(rig
            .changed(&ghost, &[(FIELD_PLAYER_FLAGS, 0, 0x10)])
            .is_empty());
        assert!(!rig.death_net.in_range, "the latch drops silently");
        assert!(rig.sent().is_empty(), "no ask inside an arena");
    }

    /// `0x6d1fc0`: the healer's combat reach plus 4 yd, inclusive; an unheld healer is out.
    #[test]
    fn the_spirit_healer_reach_is_its_combat_reach_plus_four_yards() {
        let at = |x: f32| Some((1.5, Vec3::new(x, 0.0, 0.0)));
        assert!(within_spirit_healer_reach(Vec3::ZERO, at(5.5)));
        assert!(!within_spirit_healer_reach(Vec3::ZERO, at(5.6)));
        assert!(!within_spirit_healer_reach(Vec3::ZERO, None));
    }

    /// `0x6d1d30`: an answer goes out once for the offer's caster, then zeroes it; a second answer
    /// sends nothing, and the sickness and timer bytes stay.
    #[test]
    fn a_resurrect_answer_goes_out_once_and_keeps_the_offer_bytes() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut w = World::new();
        w.insert_resource(NetCommands(tx));
        w.insert_resource(DeathGlobals {
            resurrect: Some(ResurrectOffer {
                caster: 0x77,
                name: "Priest".into(),
                sickness: true,
                has_timer: true,
                awaiting_name: false,
            }),
            ..Default::default()
        });
        answer_resurrect(&mut w, true);
        answer_resurrect(&mut w, false);
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(matches!(
            sent.as_slice(),
            [ClientCommand::ResurrectResponse {
                caster: 0x77,
                accept: true
            }]
        ));
        let offer = w.resource::<DeathGlobals>().resurrect.clone().unwrap();
        assert_eq!(offer.caster, 0);
        assert!(offer.sickness && offer.has_timer);
    }

    /// `0x512c40`: the instance event when the display map is not the corpse's.
    #[test]
    fn the_in_range_event_names_an_instance_entrance() {
        let cp = |display_map: i32, corpse_map: u32| CorpsePoint {
            display_map,
            position: [0.0; 3],
            local: [0.0; 3],
            corpse_map,
            transport: 0,
        };
        assert_eq!(corpse_range_event(cp(0, 0)), "CORPSE_IN_RANGE");
        assert_eq!(corpse_range_event(cp(0, 36)), "CORPSE_IN_INSTANCE");
    }

    /// The server's sickness table: none through 10, `level - 10` minutes through 19, then 10.
    #[test]
    fn the_sickness_minutes_follow_the_servers_table() {
        assert_eq!(sickness_minutes(10), None);
        assert_eq!(sickness_minutes(11), Some(1));
        assert_eq!(sickness_minutes(19), Some(9));
        assert_eq!(sickness_minutes(80), Some(10));
    }
}

#[cfg(test)]
mod corpse_tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    const BOAT: u32 = 0x10;

    fn corpse_on(transport: u32, local: [f32; 3]) -> CorpsePoint {
        CorpsePoint {
            display_map: 0,
            position: local,
            local,
            corpse_map: 0,
            transport,
        }
    }

    fn app(cp: CorpsePoint) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .insert_resource(DeathNet {
                corpse: Some(cp),
                ..default()
            })
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Time<Real>>();
        (app, rx)
    }

    fn resolved(app: &App) -> [f32; 3] {
        app.world().resource::<DeathNet>().corpse.unwrap().position
    }

    /// `0x523db0` puts the low word under the MO_TRANSPORT high word.
    #[test]
    fn the_transport_guid_is_the_mo_transport_high_word() {
        assert_eq!(corpse_transport_guid(0x10), 0x1FC0_0000_0000_0010);
        assert_eq!(corpse_transport_guid(0x8000_0001), 0xFFFF_FFFF_8000_0001);
    }

    /// `0x512bd0`'s matrix turns the local point about Z by the facing, then moves it.
    #[test]
    fn the_answer_frame_turns_then_moves_the_local_point() {
        assert_eq!(
            transport_frame_point([1.0, 2.0, 3.0], None),
            [1.0, 2.0, 3.0]
        );
        let p = transport_frame_point(
            [1.0, 0.0, 2.0],
            Some(([100.0, 200.0, 10.0], std::f32::consts::FRAC_PI_2)),
        );
        assert!((p[0] - 100.0).abs() < 1e-4 && (p[1] - 201.0).abs() < 1e-4);
        assert_eq!(p[2], 12.0);
    }

    /// An unstreamed transport is asked for once per 30 s, and the answer carries the point.
    #[test]
    fn an_unstreamed_transport_is_asked_for_and_its_answer_carries_the_corpse() {
        let (mut app, rx) = app(corpse_on(BOAT, [1.0, 0.0, 0.0]));
        app.world_mut()
            .run_system_once(resolve_corpse_position)
            .unwrap();
        app.world_mut()
            .run_system_once(resolve_corpse_position)
            .unwrap();
        let asks: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                asks.as_slice(),
                [ClientCommand::CorpseMapPositionQuery { transport: BOAT }]
            ),
            "one ask in the throttle window: {asks:?}"
        );
        app.world_mut()
            .resource_mut::<DeathNet>()
            .corpse_transport_frame = Some(([50.0, 60.0, 5.0], 0.0));
        app.world_mut()
            .run_system_once(resolve_corpse_position)
            .unwrap();
        assert_eq!(resolved(&app), [51.0, 60.0, 5.0]);
    }

    /// A streamed transport carries the local point by its own pose, and asks nothing.
    #[test]
    fn a_streamed_transport_carries_the_corpse_by_its_pose() {
        let (mut app, rx) = app(corpse_on(BOAT, [1.0, 2.0, 3.0]));
        let boat = app
            .world_mut()
            .spawn(Transform::from_translation(wow_to_bevy([
                100.0, 200.0, 10.0,
            ])))
            .id();
        app.world_mut()
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(corpse_transport_guid(BOAT), boat);
        app.world_mut()
            .run_system_once(resolve_corpse_position)
            .unwrap();
        let p = resolved(&app);
        assert!((p[0] - 101.0).abs() < 1e-3 && (p[1] - 202.0).abs() < 1e-3);
        assert!((p[2] - 13.0).abs() < 1e-3);
        assert!(rx.try_iter().next().is_none());
    }
}

#[cfg(test)]
mod self_res_tests {
    use std::collections::HashMap;

    use benilla_formats::{SpellCatalog, SpellDisplay};
    use benilla_protocol::messages::{ItemInfo, ItemSpellEntry};
    use benilla_protocol::ObjectFields;

    use super::{resolve_self_res, SelfRes, SPELL_EFFECT_SELF_RESURRECT};
    use crate::items::Items;
    use crate::net::{ClientCommand, NetCommands};
    use crate::ui_action::Spells;

    /// Reincarnation's effect spell, what the server writes into the field.
    const REINCARNATION: u32 = 21169;
    /// `PLAYER_SELF_RES_SPELL`.
    const F_SELF_RES: u16 = benilla_protocol::field::FIELD_PLAYER_SELF_RES_SPELL;
    /// `UNIT_FIELD_HEALTH` and `UNIT_FIELD_MAXHEALTH`: `unit_is_dead()` also needs a non-zero max.
    const F_HEALTH: u16 = benilla_protocol::field::FIELD_UNIT_HEALTH;
    const F_MAXHEALTH: u16 = benilla_protocol::field::FIELD_UNIT_MAXHEALTH;
    /// `PLAYER_FIELD_PACK_SLOT_1`: the backpack's 16 slots, two dwords per guid.
    const F_PACK_1: u16 = benilla_protocol::field::FIELD_PLAYER_PACK_SLOT_1;

    fn commands() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (NetCommands(tx), rx)
    }

    fn catalog(pairs: impl IntoIterator<Item = (u32, SpellDisplay)>) -> Spells {
        Spells {
            catalog: SpellCatalog::from_displays(pairs.into_iter().collect::<HashMap<_, _>>()),
            ..Spells::empty_for_tests()
        }
    }

    fn named(name: &str, effects: [u32; 3]) -> SpellDisplay {
        SpellDisplay {
            name: name.to_string(),
            effects,
            ..Default::default()
        }
    }

    /// Nil while health is positive, including a released ghost's 1 (`BuildPlayerRepop`).
    #[test]
    fn the_dead_gate_precedes_the_field() {
        let (net, _rx) = commands();
        let items = Items::default();
        // Nothing streamed: the spell leg answers before the inventory walk.
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let spells = catalog([(REINCARNATION, named("Reincarnation", [94, 0, 0]))]);

        let alive = ObjectFields::from_pairs(&[
            (F_MAXHEALTH, 4000),
            (F_HEALTH, 4000),
            (F_SELF_RES, REINCARNATION),
        ]);
        assert_eq!(
            resolve_self_res(&alive, &objects, &items, Some(&spells), &net),
            None,
            "alive with a self-res owed: nil"
        );

        let ghost = ObjectFields::from_pairs(&[
            (F_MAXHEALTH, 4000),
            (F_HEALTH, 1),
            (F_SELF_RES, REINCARNATION),
        ]);
        assert_eq!(
            resolve_self_res(&ghost, &objects, &items, Some(&spells), &net),
            None,
            "a released ghost still holds the field, and still answers nil"
        );

        let dead = ObjectFields::from_pairs(&[
            (F_MAXHEALTH, 4000),
            (F_HEALTH, 0),
            (F_SELF_RES, REINCARNATION),
        ]);
        assert_eq!(
            resolve_self_res(&dead, &objects, &items, Some(&spells), &net),
            Some(SelfRes::Spell {
                spell: REINCARNATION,
                label: "Reincarnation".into(),
            })
        );
    }

    /// The reference pushes the literal.
    #[test]
    fn an_unresolvable_spell_id_reads_unknown_not_nil() {
        let (net, _rx) = commands();
        let items = Items::default();
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let dead =
            ObjectFields::from_pairs(&[(F_MAXHEALTH, 4000), (F_HEALTH, 0), (F_SELF_RES, 999_999)]);
        for spells in [None, Some(catalog([]))] {
            assert_eq!(
                resolve_self_res(&dead, &objects, &items, spells.as_ref(), &net),
                Some(SelfRes::Spell {
                    spell: 999_999,
                    label: "UNKNOWN".into(),
                }),
                "no catalog and an empty catalog are the same miss"
            );
        }
    }

    /// The item leg; the backpack holds a self-res on an equip trigger, an unrelated on-use spell,
    /// and the real one.
    #[test]
    fn a_zero_field_falls_through_to_a_carried_item() {
        let (net, _rx) = commands();
        let mut items = Items::default();
        let mut objs = crate::ui_items::TestObjects::new();

        let item = |name: &str, spells: Vec<ItemSpellEntry>| ItemInfo {
            spells,
            ..crate::items::test_template(name)
        };
        let block = |spell_id: u32, trigger: u32| ItemSpellEntry {
            index: 0,
            spell_id,
            trigger,
            charges: 0,
            cooldown_ms: -1,
            category: 0,
            category_cooldown_ms: -1,
        };
        // 3026 "Use Soulstone" is the real effect-94 spell; 439 "Healing Potion" is not.
        items.insert_template(10, Some(item("Healthstone", vec![block(439, 0)])));
        items.insert_template(11, Some(item("Worn Trinket", vec![block(3026, 1)])));
        items.insert_template(
            12,
            Some(item("Ankh of Reincarnation", vec![block(3026, 0)])),
        );
        for (guid, entry) in [(0xA0_u64, 10_u32), (0xA1, 11), (0xA2, 12)] {
            objs.spawn(
                guid,
                ObjectFields::from_pairs(&[(benilla_protocol::field::FIELD_OBJECT_ENTRY, entry)]),
            );
        }
        let objects = objs.get();
        let spells = catalog([
            (
                3026,
                named("Use Soulstone", [SPELL_EFFECT_SELF_RESURRECT, 0, 0]),
            ),
            (439, named("Healing Potion", [6, 0, 0])),
        ]);

        // Backpack slots 23, 24 and 25 hold the three, in that order.
        let mut pairs = vec![(F_MAXHEALTH, 4000), (F_HEALTH, 0)];
        for (i, guid) in [0xA0_u64, 0xA1, 0xA2].iter().enumerate() {
            let f = F_PACK_1 + 2 * i as u16;
            pairs.push((f, (*guid & 0xffff_ffff) as u32));
            pairs.push((f + 1, (*guid >> 32) as u32));
        }
        let dead = ObjectFields::from_pairs(&pairs);

        match resolve_self_res(&dead, &objects, &items, Some(&spells), &net) {
            Some(SelfRes::Item { entry, label, .. }) => {
                assert_eq!(
                    entry, 12,
                    "the EQUIP-trigger copy and the potion are skipped"
                );
                assert_eq!(label, "Ankh of Reincarnation", "the ITEM names the button");
            }
            other => panic!("expected the item leg, got {other:?}"),
        }

        // Without it the answer is nil, not the spell leg's "UNKNOWN".
        let bare = ObjectFields::from_pairs(&[(F_MAXHEALTH, 4000), (F_HEALTH, 0)]);
        assert_eq!(
            resolve_self_res(&bare, &objects, &items, Some(&spells), &net),
            None
        );
    }

    /// The self-resurrect check (`0x727860`): a live prevent-resurrection effect refuses the spell
    /// leg with the custom line its misc value names, unless the self-res spell carries
    /// `AttributesEx7 & 0x8000000`; the walk reads every effect, not only the live ones.
    #[test]
    fn a_prevent_resurrection_aura_refuses_the_self_res_with_its_custom_id() {
        use benilla_protocol::messages::{AuraRecord, AuraUpdate};

        const DEBUFF: u32 = 1000;
        const PLAIN: u32 = 1001;
        let auras = |slots: &[(u32, u8)]| {
            let mut t = crate::unit_auras::UnitAuras::default();
            let slots = slots
                .iter()
                .enumerate()
                .map(|(i, &(spell_id, flags))| {
                    let record = AuraRecord {
                        spell_id,
                        flags,
                        level: 1,
                        stacks: 1,
                        caster: None,
                        duration_ms: None,
                    };
                    (i as u8, Some(record))
                })
                .collect();
            t.apply(
                &AuraUpdate {
                    unit: 1,
                    all: false,
                    slots,
                },
                0.0,
            );
            t
        };
        let spell = |ex7: u32| {
            move |id| match id {
                REINCARNATION => Some((ex7, [0; 3], [0; 3])),
                DEBUFF => Some((0, [0, super::AURA_PREVENT_RESURRECTION, 0], [0, 77, 0])),
                PLAIN => Some((0, [0; 3], [0; 3])),
                _ => None,
            }
        };
        let refusal = |slots: &[(u32, u8)], ex7| {
            super::self_res_refusal(&auras(slots), REINCARNATION, spell(ex7))
        };
        // Effect bit 1 is the aura effect's own.
        assert_eq!(refusal(&[(PLAIN, 1), (DEBUFF, 2)], 0), Some(77));
        assert_eq!(refusal(&[(PLAIN, 1)], 0), None, "no such aura");
        assert_eq!(refusal(&[(DEBUFF, 1)], 0), None, "the effect is not live");
        assert_eq!(
            refusal(&[(DEBUFF, 2)], 0x800_0000),
            None,
            "the spell bypasses"
        );
    }
}
