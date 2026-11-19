//! The innkeeper's bind question: the binder talk (`0x6d2120`) latches the innkeeper and fires `CONFIRM_BINDER` from `SMSG_BINDER_CONFIRM`'s handler (`0x6e3db9`) or the innkeeper click (`0x6ddee6`), and `ConfirmBinder` and `CheckBinderDist` read the latch live at the call.

use benilla_ui::script::{ScriptValue, UiScript};
use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::net::{ClientCommand, GuidIndex, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_script::live::UiEvents;

/// The latched innkeeper (`0xc9eb20`), set by the binder talk and never cleared.
#[derive(Resource, Default)]
pub(crate) struct BinderState {
    npc: Option<u64>,
}

impl BinderState {
    /// The latched innkeeper.
    pub(crate) fn npc(&self) -> Option<u64> {
        self.npc
    }
}

/// A held unit within its combat reach plus 4 yd of us, the talks' and the distance checks' test (`0x6d2196`-`0x6d21bb`).
pub(crate) fn in_talk_reach(
    guid: u64,
    me: Option<&Transform>,
    index: Option<&GuidIndex>,
    units: &Query<(&ObjectStore, &Transform)>,
) -> bool {
    let reach = index.and_then(|index| crate::death::healer_reach(guid, index, units));
    me.is_some_and(|me| crate::death::within_spirit_healer_reach(me.translation, reach))
}

/// What the binder and talent master verbs read at the call: the two latches and the reach test's inputs.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct TalkView<'w, 's> {
    binder: Option<Res<'w, BinderState>>,
    wipe: Option<Res<'w, crate::ui_talent_wipe::TalentWipeState>>,
    index: Option<Res<'w, GuidIndex>>,
    units: Query<'w, 's, (&'static ObjectStore, &'static Transform)>,
    me: Query<'w, 's, (&'static ObjectStore, &'static Transform), With<SelfPlayer>>,
    net: Option<Res<'w, NetCommands>>,
}

impl TalkView<'_, '_> {
    fn reach(&self, guid: Option<u64>) -> bool {
        guid.is_some_and(|guid| {
            in_talk_reach(
                guid,
                self.me.single().ok().map(|(_, t)| t),
                self.index.as_deref(),
                &self.units,
            )
        })
    }

    /// `CheckBinderDist` (`0x6d2290`).
    pub(crate) fn binder_in_reach(&self) -> bool {
        self.reach(self.binder.as_ref().and_then(|b| b.npc))
    }

    /// The binder talk with no guid (`0x6d2120`): in reach, `CMSG_BINDER_ACTIVATE` names the latch (`0x6d2257`).
    pub(crate) fn confirm_binder(&self) {
        let Some(npc) = self.binder.as_ref().and_then(|b| b.npc) else {
            return;
        };
        if self.reach(Some(npc)) {
            if let Some(net) = &self.net {
                let _ = net.0.send(ClientCommand::BinderActivate { binder: npc });
            }
        }
    }

    /// `CheckTalentMasterDist` (`0x6d2070`).
    pub(crate) fn talent_master_in_reach(&self) -> bool {
        self.reach(self.wipe.as_ref().and_then(|w| w.npc()))
    }

    /// The talent master talk with no guid (`0x6db1c0`): in reach, the wipe goes out when our money covers the latched cost (`0x6db2a6`), else `ERR_NOT_ENOUGH_MONEY` (`0x6db2ae`).
    pub(crate) fn confirm_talent_wipe(&self) -> Option<&'static str> {
        let wipe = self.wipe.as_deref()?;
        let npc = wipe.npc()?;
        if !self.reach(Some(npc)) {
            return None;
        }
        let money = self
            .me
            .single()
            .ok()
            .and_then(|(s, _)| s.0.player_money())
            .unwrap_or(0);
        if wipe.cost() > money {
            return Some("ERR_NOT_ENOUGH_MONEY");
        }
        if let Some(net) = &self.net {
            let _ = net
                .0
                .send(ClientCommand::TalentWipeConfirm { trainer: npc });
        }
        None
    }
}

/// The binder talk (`0x6d2120`): a zero guid stands for the latch (`0x6d2137`-`0x6d2139`); a held innkeeper in reach with a guid is latched (`0x6d21cc`) and `CONFIRM_BINDER` fires with the current subzone's name, else the zone's, else `HOME_INN` (`0x6d21d7`-`0x6d2201`); with no guid it sends `CMSG_BINDER_ACTIVATE` naming the latch (`0x6d21ca` -> `0x6d2257`).
pub(crate) fn talk_to_binder(
    In(npc): In<u64>,
    mut binder: ResMut<BinderState>,
    me: Query<&Transform, With<SelfPlayer>>,
    index: Option<Res<GuidIndex>>,
    units: Query<(&ObjectStore, &Transform)>,
    ids: Option<Res<crate::area::ZoneIds>>,
    areas: Option<Res<AreaTableRes>>,
    net: Option<Res<NetCommands>>,
    mut ui: ResMut<UiEvents>,
) {
    let held = if npc != 0 {
        npc
    } else {
        binder.npc.unwrap_or(0)
    };
    if held == 0 || !in_talk_reach(held, me.single().ok(), index.as_deref(), &units) {
        debug!("ui_binder: innkeeper {held:#x} not held in reach, no question");
        return;
    }
    if npc == 0 {
        if let Some(net) = &net {
            let _ = net.0.send(ClientCommand::BinderActivate { binder: held });
        }
        return;
    }
    binder.npc = Some(npc);
    let name = area_row_name(
        ids.as_deref().copied().unwrap_or_default(),
        areas.as_deref(),
    );
    ui.write(move |script| {
        let name = name.unwrap_or_else(|| home_inn(script));
        script.fire_event("CONFIRM_BINDER", vec![ScriptValue::Str(name)]);
    });
}

/// The talk queued from a click, its event fired in turn.
pub(crate) fn queue_binder_talk(commands: &mut Commands, npc: u64) {
    commands.queue(move |world: &mut World| {
        if let Err(e) = world.run_system_cached_with(talk_to_binder, npc) {
            warn!("ui_binder: the binder talk did not run: {e}");
        }
        crate::ui_script::live::fire_handler_events(world);
    });
}

/// `HOME_INN` from the interface's strings (`0x6d21ee`).
fn home_inn(script: &UiScript) -> String {
    script
        .lua()
        .globals()
        .get::<String>("HOME_INN")
        .unwrap_or_default()
}

/// The client's area row, else its zone's row when the area has none (`0x519ca0`), by name, an empty one included (`0x6d21e0`); `None` for neither.
fn area_row_name(ids: crate::area::ZoneIds, areas: Option<&AreaTableRes>) -> Option<String> {
    let areas = areas?;
    areas
        .0
        .get(ids.area)
        .or_else(|| areas.0.get(ids.zone))
        .map(|row| row.name.clone())
}

/// `SoundEntries.dbc` 1141, played on `SMSG_PLAYERBOUND` before the area is resolved, so an area
/// the catalog cannot name gets the sound and no line.
const SOUND_PLAYERBOUND: u32 = 1141;

/// The innkeeper's packet handlers.
pub(crate) mod net {
    use super::*;

    use bevy::ecs::message::MessageWriter;

    use crate::net::{ServerSoundKind, ServerSoundMessage};
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::BinderConfirm, on_confirm)
            .net_handler(K::PlayerBound, on_bound);
    }

    /// `SMSG_BINDER_CONFIRM`: the binder talk with the packet's guid, a zero one included (`0x6e3db9`).
    fn on_confirm(In(ev): In<SessionEvent>, world: &mut World) {
        if let SessionEvent::BinderConfirm { binder: npc } = ev {
            if let Err(e) = world.run_system_cached_with(talk_to_binder, npc) {
                warn!("ui_binder: the binder talk did not run: {e}");
            }
        }
    }

    fn on_bound(
        In(ev): In<SessionEvent>,
        mut errors: ResMut<crate::ui_action::UiErrorKeys>,
        areas: Option<Res<AreaTableRes>>,
        mut sounds: MessageWriter<ServerSoundMessage>,
    ) {
        if let SessionEvent::PlayerBound { binder: npc, area } = ev {
            debug!("net: bound to area {area} by {npc:#x}");
            bound(area, &mut errors, areas.as_deref(), &mut sounds);
        }
    }

    /// `SMSG_PLAYERBOUND`, in the handler's order: play the handler's own sound (the error row has
    /// none), then queue `ERR_DEATHBIND_SUCCESS_S` (error id `0x138`, a system chat line) named by
    /// the area. The latch stays; the hearthstone moves on `SMSG_BINDPOINTUPDATE`.
    pub(crate) fn bound(
        area: u32,
        errors: &mut crate::ui_action::UiErrorKeys,
        areas: Option<&AreaTableRes>,
        sounds: &mut MessageWriter<ServerSoundMessage>,
    ) {
        sounds.write(ServerSoundMessage {
            kind: ServerSoundKind::Sound2d,
            sound_id: SOUND_PLAYERBOUND,
            source: None,
        });
        let Some(name) = areas.and_then(|a| a.0.name(area)).filter(|n| !n.is_empty()) else {
            debug!("ui_binder: bound to area {area}, which AreaTable does not name — no line");
            return;
        };
        errors.0.push(crate::ui_action::UiError::s(
            "ERR_DEATHBIND_SUCCESS_S",
            name,
        ));
    }
}

/// The innkeeper bind flow: its handlers and the latch the verbs read.
pub(crate) struct UiBinderPlugin;

impl Plugin for UiBinderPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<BinderState>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0x519ca0` falls back to the zone id's row only when the area id has none (`0x519cc9`), and
    /// `0x6d21e0` passes the row's name as it is. Area 186 is Dolanaar in Teldrassil (141).
    #[test]
    fn the_dialogs_name_is_the_area_row_else_the_zone_row() {
        use crate::area::ZoneIds;
        let row = |id, zone_id, name: &str| {
            (
                id,
                benilla_formats::AreaTableRow {
                    map_id: 1,
                    zone_id,
                    explore_flag: 0,
                    flags: 0,
                    faction_group_mask: 0,
                    exploration_level: 0,
                    name: name.to_string(),
                    ambient_multiplier: 0.0,
                },
            )
        };
        let areas = AreaTableRes(benilla_formats::AreaTableCatalog::from_rows(vec![
            row(141, 0, "Teldrassil"),
            row(186, 141, "Dolanaar"),
            row(187, 141, ""),
        ]));
        let ids = |zone, area| ZoneIds { zone, area };
        assert_eq!(
            area_row_name(ids(141, 186), Some(&areas)).as_deref(),
            Some("Dolanaar")
        );
        assert_eq!(
            area_row_name(ids(141, 0xffff), Some(&areas)).as_deref(),
            Some("Teldrassil"),
            "an area id with no row takes the zone's row"
        );
        assert_eq!(
            area_row_name(ids(141, 187), Some(&areas)).as_deref(),
            Some(""),
            "a row with an empty name is still the area's row"
        );
        // No row for either, no catalog, no ids: the caller takes `HOME_INN`.
        assert_eq!(area_row_name(ids(0xfffe, 0xffff), Some(&areas)), None);
        assert_eq!(area_row_name(ids(141, 186), None), None);
        assert_eq!(area_row_name(ZoneIds::default(), Some(&areas)), None);
    }

    /// Resolved against the install's table: a mistyped key would silence the line, not garble it.
    #[test]
    fn the_bind_queues_the_deathbind_success_line_named_by_the_area() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let vm = UiScript::new().expect("VM");
        vm.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| vm.lua().globals().get::<String>(key).ok();

        let msg = crate::ui_action::UiError::s("ERR_DEATHBIND_SUCCESS_S", "Dolanaar");
        assert_eq!(msg.key, "ERR_DEATHBIND_SUCCESS_S");
        assert_eq!(
            crate::ui_action::ui_error_text(&msg, &g).as_deref(),
            Some("Dolanaar is now your home.")
        );
    }
}
