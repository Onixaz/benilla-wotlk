//! The guild tabard designer, the app half of the stock `TabardFrame.xml`. Only
//! `MSG_TABARDVENDOR_ACTIVATE` opens it (`0x6cc576` -> `0x598e60`); no Lua verb does. Its verbs read
//! the vendor, the save latch and our guild's cached record at the call, and its events fire from
//! the handlers and the verbs' calls. Our own body wears the design over a forced tabard geoset
//! (`[cc+0xc]`).

use bevy::prelude::*;

use benilla_formats::GuildEmblem;
use benilla_ui::script::{UiScript, TABARD_COUNTS, TABARD_CREATION_COST};

use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::portrait::PaperDollBooth;
use crate::ui_guild::GuildCache;
use crate::ui_script::live::UiEvents;
use crate::ui_script::UiFeed;
use crate::ui_session::NpcSession;

/// The designer's globals: the vendor (`0xc0e518`) and the save latch (`0xc9eacc`).
#[derive(Resource, Default)]
pub(crate) struct TabardOpen {
    vendor: Option<u64>,
    save_pending: bool,
}

impl TabardOpen {
    /// The close's core (`0x598ec0`): the vendor cleared; whether one was held.
    fn close(&mut self) -> bool {
        self.vendor.take().is_some()
    }
}

impl NpcSession for TabardOpen {
    fn npc(&self) -> Option<u64> {
        self.vendor
    }
    fn close(&mut self) {
        TabardOpen::close(self);
    }
}

/// The character component's guild colours while the designer holds our body: the open turns the purchase on (`0x598e98`), the seed and each cycle apply the five (`0x5993b0`), the close turns it off (`0x598eee`).
#[derive(Resource, Default)]
pub(crate) struct TabardDesign {
    purchase: bool,
    colors: Option<[i32; 5]>,
}

impl TabardDesign {
    pub(crate) fn preview(&self) -> Option<GuildEmblem> {
        let d = self.colors.filter(|_| self.purchase)?;
        Some(GuildEmblem {
            emblem_style: d[0],
            emblem_color: d[1],
            border_style: d[2],
            border_color: d[3],
            background_color: d[4],
        })
    }
}

/// The tabard save's checks in their order (`0x6db320`), each a message key: the ten range checks share one, a missing record is no guild, and the money check is the client's own `ERR_NOT_ENOUGH_MONEY` (`0x6db447`, message 0x28), not the server's.
pub(crate) fn preflight(
    design: [i32; 5],
    record: Option<[i32; 5]>,
    guild_rank: u32,
    purse: u32,
) -> Result<[u32; 5], &'static str> {
    if design.iter().any(|v| *v < 0) {
        return Err("ERR_GUILDEMBLEM_INVALID_TABARD_COLORS");
    }
    if design
        .iter()
        .zip(TABARD_COUNTS)
        .any(|(v, count)| *v >= count)
    {
        return Err("ERR_GUILDEMBLEM_INVALID_TABARD_COLORS");
    }
    let Some(record) = record else {
        return Err("ERR_GUILDEMBLEM_NOGUILD");
    };
    if record == design {
        return Err("ERR_GUILDEMBLEM_SAME");
    }
    if guild_rank != 0 {
        return Err("ERR_GUILDEMBLEM_NOTGUILDMASTER");
    }
    if purse < TABARD_CREATION_COST {
        return Err("ERR_NOT_ENOUGH_MONEY");
    }
    Ok(design.map(|v| v as u32))
}

/// What the designer's verbs read and write at the call, part of the VM's live view.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct TabardView<'w, 's> {
    open: Option<ResMut<'w, TabardOpen>>,
    design: Option<ResMut<'w, TabardDesign>>,
    cache: Option<Res<'w, GuildCache>>,
    /// The interact target the save names (`0x599438`) and the close clears (`0x598efa`).
    target: Option<ResMut<'w, crate::ui_session::InteractTarget>>,
    commands: Option<Res<'w, NetCommands>>,
    me: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
}

impl TabardView<'_, '_> {
    /// The interact target, for a verb whose close clears it.
    pub(crate) fn target_mut(&mut self) -> Option<&mut crate::ui_session::InteractTarget> {
        self.target.as_deref_mut()
    }

    /// The interact target (`0xbd07a8`).
    pub(crate) fn target(&self) -> Option<u64> {
        self.target.as_ref().and_then(|t| t.0)
    }

    /// The vendor held, read by a system between its calls into Lua.
    pub(crate) fn vendor(&self) -> Option<u64> {
        self.open.as_ref().and_then(|o| o.vendor)
    }

    /// Our guild's cached record, asking nothing (`0x6de20c`); `None` with no player object (`0x6de1e3`).
    pub(crate) fn guild_record(&self) -> Option<[i32; 5]> {
        let me = self.me.single().ok()?;
        self.cache.as_ref()?.emblem_record(me.0.player_guild_id())
    }

    /// The save latch (`0xc9eacc`).
    pub(crate) fn save_pending(&self) -> bool {
        self.open.as_ref().is_some_and(|o| o.save_pending)
    }

    /// The tabard save (`0x6db320`) with the player object held: the first failing check's key, else `MSG_SAVE_GUILD_EMBLEM` to the interact target, then the latch (`0x6db4cc`); `None` with no player object (`0x599430`).
    pub(crate) fn save(&mut self, design: [i32; 5]) -> Option<Result<(), &'static str>> {
        let me = self.me.single().ok()?;
        let record = self
            .cache
            .as_ref()
            .and_then(|c| c.emblem_record(me.0.player_guild_id()));
        let rank = me.0.player_guild_rank();
        let purse = me.0.player_money().unwrap_or(0);
        let design = match preflight(design, record, rank, purse) {
            Ok(d) => d,
            Err(key) => return Some(Err(key)),
        };
        let vendor = self.target.as_ref().and_then(|t| t.0).unwrap_or(0);
        if let Some(c) = &self.commands {
            let _ = c.0.send(ClientCommand::SaveGuildEmblem { vendor, design });
        }
        if let Some(open) = self.open.as_mut() {
            open.save_pending = true;
        }
        Some(Ok(()))
    }

    /// `CloseTabardCreation` (`0x598f30` -> `0x598ec0`): with a vendor held, the purchase off and the vendor cleared, and the count of `CLOSE_TABARD_FRAME`s: an interact target on the vendor is cleared and re-enters the close through its tabard leg first (`0x598efa` -> `0x512e60` -> `0x51300e`), then its own; one with the target elsewhere (`0x518d69`); none with no vendor (`0x598ecb`).
    pub(crate) fn close(&mut self) -> u32 {
        let Some(vendor) = self.open.as_ref().and_then(|o| o.vendor) else {
            return 0;
        };
        if let Some(o) = self.open.as_mut() {
            o.close();
        }
        if let Some(d) = self.design.as_mut() {
            d.purchase = false;
        }
        if self
            .target
            .as_mut()
            .is_some_and(|t| t.clear_in_call(vendor))
        {
            2
        } else {
            1
        }
    }

    /// The seed's and each cycle's colours onto our body (`0x5993b0`), with our player object held.
    pub(crate) fn apply_guild_color(&mut self, design: [i32; 5]) {
        if self.me.single().is_err() {
            return;
        }
        if let Some(d) = self.design.as_mut() {
            d.colors = Some(design);
        }
    }
}

/// The world leave's interaction close (`0x528dce` -> `0x512e60`): a held vendor closes through the tabard leg.
pub(crate) fn leave_world(world: &mut World) {
    close_by_interaction(world);
}

/// The interaction close's tabard leg (`0x51300e` -> `0x598ec0`), its target already zeroed: a held vendor closes, the purchase off and `CLOSE_TABARD_FRAME` fired once.
pub(crate) fn close_by_interaction(world: &mut World) {
    let held = world
        .get_resource_mut::<TabardOpen>()
        .is_some_and(|mut o| o.close());
    if held {
        if let Some(mut d) = world.get_resource_mut::<TabardDesign>() {
            d.purchase = false;
        }
        if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
            ui.fire("CLOSE_TABARD_FRAME", vec![]);
        }
    }
}

/// The UI world entry's tabard reset (`0x528010` -> `0x598dc0`): the vendor zeroed.
pub(crate) fn enter_world(world: &mut World) {
    if let Some(mut o) = world.get_resource_mut::<TabardOpen>() {
        o.vendor = None;
    }
}

/// The pane's yaw onto the booth while the designer shows: rendering, no event, as the model frame reads its facing on every frame render.
fn sync_tabard_booth(script: Option<NonSend<UiScript>>, mut booth: ResMut<PaperDollBooth>) {
    let Some(script) = script else {
        return;
    };
    if script.frame_visible("TabardFrame") {
        let yaw = script.model_pane_facing("TabardModel");
        if booth.yaw != yaw {
            booth.yaw = yaw;
        }
    }
}

/// The tabard vendor's packet handlers.
mod net {
    use benilla_protocol::messages::GUILD_EMBLEM_RESULT_MESSAGES;
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{TabardDesign, TabardOpen};
    use crate::net::{NetHandlerApp, ObjectStore, SelfPlayer};
    use crate::ui_guild::GuildCache;
    use crate::ui_script::live::UiEvents;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::TabardVendorActivate, on_activate)
            .net_handler(K::SaveGuildEmblemResult, on_save_result)
            .net_handler(K::Disconnected, on_session_end);
    }

    /// `MSG_TABARDVENDOR_ACTIVATE` (`0x6cc576` -> `0x598e60`): the vendor stored (`0x598e69`), the purchase on with our player object (`0x598e98`), the interact target set to it (`0x598e9e`), then `OPEN_TABARD_FRAME` (`0x598eaa`).
    fn on_activate(In(ev): In<SessionEvent>, world: &mut World) {
        let SessionEvent::TabardVendorActivate(vendor) = ev else {
            return;
        };
        world.resource_mut::<TabardOpen>().vendor = Some(vendor);
        let me = world
            .query_filtered::<(), With<SelfPlayer>>()
            .single(world)
            .is_ok();
        if me {
            world.resource_mut::<TabardDesign>().purchase = true;
        }
        crate::ui_session::set_interact_target(world, vendor);
        world
            .resource_mut::<UiEvents>()
            .fire("OPEN_TABARD_FRAME", vec![]);
    }

    /// `MSG_SAVE_GUILD_EMBLEM` (`0x6d9230`): a code past 5 is dropped (`0x6d9246`); else its line unless the table's sentinel (`0x6d924f`), the latch cleared (`0x6d9264`), then success drops our guild's record with no ask (`0x6d929b`) and fires `GUILDTABARD_UPDATE` (`0x6d92a7`), a failure `TABARD_SAVE_PENDING` (`0x6d92bf`).
    pub(super) fn on_save_result(
        In(ev): In<SessionEvent>,
        mut tabard: ResMut<TabardOpen>,
        mut cache: ResMut<GuildCache>,
        me: Query<&ObjectStore, With<SelfPlayer>>,
        mut ui: ResMut<UiEvents>,
    ) {
        let SessionEvent::SaveGuildEmblemResult(result) = ev else {
            return;
        };
        let Some(row) = usize::try_from(result)
            .ok()
            .and_then(|i| GUILD_EMBLEM_RESULT_MESSAGES.get(i))
        else {
            return;
        };
        let guild = me.single().ok().map(|me| me.0.player_guild_id());
        let Some(key) = *row else {
            saved(&mut tabard, &mut cache, &mut ui, result, guild);
            return;
        };
        crate::ui_action::show_in_turn(&mut ui, "ui_tabard", crate::ui_action::UiError::key(key));
        // The latch, the cache and the event after the line's listeners (`0x6d9257` before `0x6d9264`).
        ui.world(move |world| {
            let ran = world.run_system_cached_with(
                |In((result, guild)): In<(u32, Option<u32>)>,
                 mut tabard: ResMut<TabardOpen>,
                 mut cache: ResMut<GuildCache>,
                 mut ui: ResMut<UiEvents>| {
                    saved(&mut tabard, &mut cache, &mut ui, result, guild);
                },
                (result, guild),
            );
            crate::ui_cast::step_ran(ran, "a tabard save's result");
        });
    }

    /// The save's latch cleared, the guild's record evicted on a success, then its event (`0x6d9264`, `0x6d929b`, `0x6d92a7`, `0x6d92bf`).
    fn saved(
        tabard: &mut TabardOpen,
        cache: &mut GuildCache,
        ui: &mut UiEvents,
        result: u32,
        guild: Option<u32>,
    ) {
        tabard.save_pending = false;
        if result == 0 {
            if let Some(guild) = guild {
                cache.evict(guild);
            }
            ui.fire("GUILDTABARD_UPDATE", vec![]);
        } else {
            ui.fire("TABARD_SAVE_PENDING", vec![]);
        }
    }

    /// The game's teardown clears the save latch (`0x406510` -> `0x6e483b`).
    fn on_session_end(In(_): In<SessionEvent>, mut tabard: ResMut<TabardOpen>) {
        tabard.save_pending = false;
    }
}

pub(crate) struct TabardUiPlugin;

impl Plugin for TabardUiPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<TabardOpen>()
            .init_resource::<TabardDesign>()
            .add_systems(Update, sync_tabard_booth.in_set(UiFeed));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `MSG_SAVE_GUILD_EMBLEM`: a shown line's listeners read the save still pending; the latch clears and the event fires after them (`0x6d9257` before `0x6d9264`, `0x6d92bf`).
    #[test]
    fn a_save_results_line_runs_before_the_latch_clears() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.insert_resource(TabardOpen {
            vendor: Some(0x42),
            save_pending: true,
        });
        world.init_resource::<crate::ui_guild::GuildCache>();
        world.init_resource::<UiEvents>();
        world
            .run_system_once_with(
                net::on_save_result,
                benilla_protocol::SessionEvent::SaveGuildEmblemResult(3),
            )
            .unwrap();
        assert!(world.resource::<TabardOpen>().save_pending);
        assert_eq!(world.resource::<UiEvents>().names(), ["<world>", "<world>"]);
        crate::ui_script::live::fire_handler_events(&mut world);
        assert!(!world.resource::<TabardOpen>().save_pending);
    }

    #[test]
    fn the_fourteen_checks_run_in_the_references_order() {
        let rec = Some([1, 2, 3, 4, 5]);
        assert_eq!(
            preflight([-1, 0, 0, 0, 0], rec, 0, 200_000),
            Err("ERR_GUILDEMBLEM_INVALID_TABARD_COLORS")
        );
        assert_eq!(
            preflight([170, 0, 0, 0, 0], rec, 0, 200_000),
            Err("ERR_GUILDEMBLEM_INVALID_TABARD_COLORS")
        );
        assert_eq!(
            preflight([0, 0, 0, 0, 51], rec, 0, 200_000),
            Err("ERR_GUILDEMBLEM_INVALID_TABARD_COLORS")
        );
        assert_eq!(
            preflight([0; 5], None, 0, 200_000),
            Err("ERR_GUILDEMBLEM_NOGUILD")
        );
        assert_eq!(
            preflight([1, 2, 3, 4, 5], rec, 0, 200_000),
            Err("ERR_GUILDEMBLEM_SAME")
        );
        assert_eq!(
            preflight([0; 5], rec, 1, 200_000),
            Err("ERR_GUILDEMBLEM_NOTGUILDMASTER")
        );
        assert_eq!(
            preflight([0; 5], rec, 0, 99_999),
            Err("ERR_NOT_ENOUGH_MONEY")
        );
        assert_eq!(
            preflight([169, 16, 5, 16, 50], rec, 0, 100_000),
            Ok([169, 16, 5, 16, 50])
        );
        // An undesigned record (-1s) is a cached record: the same check compares and passes.
        assert_eq!(preflight([0; 5], Some([-1; 5]), 0, 100_000), Ok([0; 5]));
    }
}
