//! The barber shop, the app half of the stock `Blizzard_BarbershopUI`, which `UIParent` loads on
//! `BARBER_SHOP_OPEN`. The chair's `SMSG_ENABLE_BARBER_SHOP` opens it (`0x52f9b0`): our own body
//! takes off its helm, shoulders and shield, wears the barber's cape and previews the Lua dials,
//! and the camera turns to face it. `CancelBarberShop()`, leaving the chair, a display change, a
//! world change and an active battleground close it (`0x52e9f0`), which puts all of it back. The
//! verbs read the shop through [`BarberLive`] at their call and write the dials, the send and the
//! close inside it; the events fire from the handlers and watches that cause them.

use benilla_assets::ModelAnimations;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_formats::{BarberShopStyle, GameTableId};
use benilla_ui::script::{BarberRead, BarberStyle, GlueCustomizeGrid};

use crate::creature_anim::AnimDriver;
use crate::net::{
    ClientCommand, EnteredWorldMessage, NetCommands, ObjectStore, SelfPlayer, WorldportMessage,
};
use crate::player::camera_view::BarberView;
use crate::player::camera_zoom::ZoomLimit;
use crate::player::{Player, StandStateRequest};
use crate::ui_script::live::UiEvents;

/// The barber's cape, the `ItemDisplayInfo` the chair puts in the tabard slot (`0x52fb2a`).
pub(crate) const BARBER_CAPE_DISPLAY: u32 = 54559;

/// The stand states of a chair, `UNIT_SITTINGCHAIRLOW` to `HIGH`: sitting in one keeps the shop
/// open, any other state closes it (`0x6e2b30`).
const CHAIR_STAND_STATES: std::ops::RangeInclusive<u8> = 4..=6;

/// `BarberShopStyle.dbc`, every row in file order; absent, the shop opens with no styles.
#[derive(Resource)]
pub(crate) struct BarberStyles(pub(crate) Vec<BarberShopStyle>);

/// What our own body wears for the shop: the open flag and the previewed look, in dial order.
#[derive(Resource, Default, PartialEq)]
pub(crate) struct BarberBody {
    pub(crate) open: bool,
    pub(crate) preview: Option<[u8; 5]>,
}

/// The shop's open flag (`[0xbd19b8]`), our race and sex's rows and grid, and what the close watches.
#[derive(Resource, Default)]
pub(crate) struct BarberShop {
    open: bool,
    /// The skin style's look value (`[0xbd19b4]`): the open's real skin, then a skin step's; a reset keeps it (`0x52f7d0`).
    skin_style: u8,
    /// The rows of our race and sex in file order and the grid the dials walk, set by the open.
    styles: Vec<BarberStyle>,
    grid: Option<GlueCustomizeGrid>,
    /// The stand state the shop last saw, which leaving the chair moves.
    stand: Option<u8>,
    /// A close from `CancelBarberShop()` owes the view, the zoom floor and the stand-up, which [`watch_close`] runs.
    owed_effects: bool,
}

impl BarberShop {
    /// The close (`0x52e9f0`): only an open shop closes. True when it closed.
    fn close(&mut self) -> bool {
        std::mem::take(&mut self.open)
    }

    /// Whether a new stand state closes the shop: leaving the chair for any other state (`0x6e2b30`). The first sight after the open only records it.
    fn watch(&mut self, stand: u8) -> bool {
        if !self.open {
            return false;
        }
        self.stand
            .replace(stand)
            .is_some_and(|old| old != stand && !CHAIR_STAND_STATES.contains(&stand))
    }
}

/// The close's side effects outside the shop: the view the chair left, the zoom floor, and the
/// stand-up (`0x52ea2e`, `0x52ea4b`).
fn close_effects(
    view: &mut MessageWriter<BarberView>,
    zoom: &mut ZoomLimit,
    stand: &mut MessageWriter<StandStateRequest>,
) {
    view.write(BarberView::Leave);
    zoom.barber_floor = None;
    stand.write(StandStateRequest { state: 0 });
}

/// Our look in dial order off `PLAYER_BYTES` and `PLAYER_BYTES_2`.
fn self_look(s: &ObjectStore) -> [u8; 5] {
    let f = &s.0;
    [
        f.player_skin().unwrap_or(0),
        f.player_face().unwrap_or(0),
        f.player_hair_style().unwrap_or(0),
        f.player_hair_color().unwrap_or(0),
        f.player_facial_hair().unwrap_or(0),
    ]
}

/// The barber shop the verbs read and write at their call.
#[derive(SystemParam)]
pub(crate) struct BarberLive<'w, 's> {
    shop: Option<ResMut<'w, BarberShop>>,
    body: Option<ResMut<'w, BarberBody>>,
    sheet: Option<Res<'w, crate::ui_char::SheetTables>>,
    catalog: Option<Res<'w, crate::entities::CharCreate>>,
    commands: Option<Res<'w, NetCommands>>,
    me: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
}

impl BarberLive<'_, '_> {
    /// The shop at the call; `None` with no player object.
    pub(crate) fn read(&self) -> Option<BarberRead<'_>> {
        let me = self.me.single().ok()?;
        let s = &me.0;
        let shop = self.shop.as_deref()?;
        let look = self_look(me);
        let level = s.unit_level().unwrap_or(0) as i32;
        Some(BarberRead {
            open: shop.open,
            class: s.unit_class().unwrap_or(0),
            base_cost: self.sheet.as_deref().map_or(0.0, |t| {
                t.gt.lookup(GameTableId::BarberShopCostBase, level - 1, 0)
            }),
            look,
            dials: self.body.as_deref().and_then(|b| b.preview).unwrap_or(look),
            skin_style: shop.skin_style,
            styles: &shop.styles,
            grid: shop.grid.as_ref(),
            money: s.player_money().unwrap_or(0),
        })
    }

    /// A step's or a reset's dials onto our body (`0x52fc80`, `0x52f7d0`).
    pub(crate) fn set_dials(&mut self, dials: [u8; 5]) {
        if let Some(b) = self.body.as_mut().filter(|b| b.open) {
            b.preview = Some(dials);
        }
    }

    /// A skin step's style pointer (`0x52fd94`).
    pub(crate) fn set_skin_style(&mut self, data: u8) {
        if let Some(s) = self.shop.as_mut().filter(|s| s.open) {
            s.skin_style = data;
        }
    }

    /// `CMSG_ALTER_APPEARANCE` (`0x52eda0`).
    pub(crate) fn send(&self, styles: [u32; 4]) {
        if let Some(c) = &self.commands {
            let _ = c.0.send(ClientCommand::AlterAppearance { styles });
        }
    }

    /// `CancelBarberShop()`'s close (`0x52e9f0`): the flag and our body put back in the call, the camera's and the stand-up's effects owed to [`watch_close`].
    pub(crate) fn close(&mut self) -> bool {
        let Some(shop) = self.shop.as_mut() else {
            return false;
        };
        if !shop.close() {
            return false;
        }
        shop.owed_effects = true;
        if let Some(b) = self.body.as_mut() {
            **b = BarberBody::default();
        }
        true
    }

    /// Our race row's hair token (`0x52e6d0`).
    pub(crate) fn hair_customization(&self) -> Option<&str> {
        let s = &self.me.single().ok()?.0;
        self.catalog
            .as_deref()?
            .0
            .hair_customization(s.unit_race().unwrap_or(0))
    }

    /// Our race row's facial-hair token for the sex in `PLAYER_BYTES_3` (`0x52e730`).
    pub(crate) fn facial_hair_customization(&self) -> Option<&str> {
        let s = &self.me.single().ok()?.0;
        self.catalog.as_deref()?.0.facial_hair_customization(
            s.unit_race().unwrap_or(0),
            s.player_gender_byte().unwrap_or(0),
        )
    }
}

/// The close from a watch or a handler (`0x52e9f0`): an open shop closes, puts our body back and fires `BARBER_SHOP_CLOSE` in turn; true when it closed.
fn close_in_turn(world: &mut World) -> bool {
    let closed = world
        .get_resource_mut::<BarberShop>()
        .is_some_and(|mut s| s.close());
    if !closed {
        return false;
    }
    if let Some(mut b) = world.get_resource_mut::<BarberBody>() {
        *b = BarberBody::default();
    }
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.fire("BARBER_SHOP_CLOSE", vec![]);
    }
    world.write_message(BarberView::Leave);
    if let Some(mut z) = world.get_resource_mut::<ZoomLimit>() {
        z.barber_floor = None;
    }
    world.write_message(StandStateRequest { state: 0 });
    true
}

/// Our display id's watch (`0x716860`): a new display closes an open shop.
pub(crate) fn display_changed(world: &mut World) {
    close_in_turn(world);
}

/// Our look byte's watch (`0x6da7f0` hair colour, `0x6da870` skin, `0x6df530` hair style, `0x6df620` facial hair): the body takes the new value on its dial, `dial` in dial order; the watch then queues `BARBER_SHOP_APPEARANCE_APPLIED` for the frame's flush (`0x52e4d0`).
pub(crate) fn look_changed(world: &mut World, dial: usize, value: u8) {
    if let Some(mut b) = world.get_resource_mut::<BarberBody>() {
        if let Some(p) = b.preview.as_mut() {
            p[dial] = value;
        }
    }
}

/// The close's other triggers: leaving the chair, a world change, and a cancel's owed effects.
#[allow(clippy::too_many_arguments)]
fn watch_close(
    mut shop: ResMut<BarberShop>,
    player: Res<Player>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut entered: MessageReader<EnteredWorldMessage>,
    mut worldports: MessageReader<WorldportMessage>,
    mut view: MessageWriter<BarberView>,
    mut zoom: ResMut<ZoomLimit>,
    mut stand: MessageWriter<StandStateRequest>,
    mut queue: Commands,
) {
    if std::mem::take(&mut shop.owed_effects) {
        close_effects(&mut view, &mut zoom, &mut stand);
    }
    // A world change closes it (`0x403b70`).
    let world_changed = entered.read().count() + worldports.read().count() > 0;
    let left = self_q.iter().next().is_some_and(|s| {
        let state = player.committed_stand_state(s.0.unit_stand_state());
        shop.watch(state)
    });
    if (world_changed || left) && shop.open {
        queue.queue(|world: &mut World| {
            close_in_turn(world);
            crate::ui_script::live::fire_handler_events(world);
        });
    }
}

/// The barber shop's packet handlers.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use super::*;
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::EnableBarberShop, on_enable)
            .net_handler(K::BarberShopResult, on_result)
            .net_handler(K::BattlefieldStatus, on_battlefield_status);
    }

    /// `SMSG_ENABLE_BARBER_SHOP` (`0x52f9b0`): needs our own body (`0x52f9ca`); the styles of our race and the sex in `PLAYER_BYTES_3` (`0x52fb45`), the flag set (`0x52fba5`), then `BARBER_SHOP_OPEN` (`0x52fbac`). The view's distance is the body's playing sequence's bounds radius, twice over, three times for a Gnome (`0x52fc47`); the radius floors the zoom while the shop is open.
    #[allow(clippy::type_complexity, clippy::too_many_arguments)]
    fn on_enable(
        In(ev): In<SessionEvent>,
        mut shop: ResMut<BarberShop>,
        mut body: ResMut<BarberBody>,
        self_q: Query<
            (&ObjectStore, Option<&ModelAnimations>, Option<&AnimDriver>),
            With<SelfPlayer>,
        >,
        styles: Option<Res<BarberStyles>>,
        catalog: Option<Res<crate::entities::CharCreate>>,
        mut view: MessageWriter<BarberView>,
        mut zoom: ResMut<ZoomLimit>,
        mut ui: ResMut<UiEvents>,
    ) {
        if !matches!(ev, SessionEvent::EnableBarberShop) {
            return;
        }
        let Some((store, anims, driver)) = self_q.iter().next() else {
            return;
        };
        let f = &store.0;
        let race = f.unit_race().unwrap_or(0);
        let sex = f.player_gender_byte().unwrap_or(0);
        shop.styles = styles
            .as_deref()
            .map(|s| {
                s.0.iter()
                    .filter(|r| r.fits(race, sex))
                    .map(|r| BarberStyle {
                        id: r.id,
                        style_type: r.style_type,
                        name: r.name.clone(),
                        description: r.description.clone(),
                        cost_modifier: r.cost_modifier,
                        data: r.data,
                    })
                    .collect()
            })
            .unwrap_or_default();
        shop.grid = catalog
            .as_deref()
            .and_then(|c| c.0.section_grid(race, f.unit_gender().unwrap_or(0)))
            .map(|g| GlueCustomizeGrid {
                sections: g.sections.clone(),
                facial_hair_styles: g.facial_hair_styles,
            });
        shop.skin_style = f.player_skin().unwrap_or(0);
        shop.open = true;
        shop.stand = None;
        *body = BarberBody {
            open: true,
            preview: Some(self_look(store)),
        };
        ui.fire("BARBER_SHOP_OPEN", vec![]);
        let radius = match (anims, driver) {
            (Some(a), Some(d)) => d
                .active_anim()
                .and_then(|id| a.find(id))
                .map_or(1.0, |c| c.bounds_radius),
            _ => 1.0,
        };
        let scale = if f.unit_race() == Some(7) { 3.0 } else { 2.0 };
        zoom.barber_floor = Some(radius);
        view.write(BarberView::Enter {
            distance: radius * scale,
        });
    }

    /// `SMSG_BARBER_SHOP_RESULT` (`0x52e5b0`): 0 fires `BARBER_SHOP_SUCCESS`, 1 and 3 show ERR_NOT_ENOUGH_MONEY, 2 ERR_NOT_BARBER_SITTING, anything else nothing.
    fn on_result(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
        let SessionEvent::BarberShopResult(result) = ev else {
            return;
        };
        let key = match result {
            0 => {
                ui.fire("BARBER_SHOP_SUCCESS", vec![]);
                return;
            }
            1 | 3 => "ERR_NOT_ENOUGH_MONEY",
            2 => "ERR_NOT_BARBER_SITTING",
            _ => return,
        };
        crate::ui_action::show_in_turn(&mut ui, "ui_barber", crate::ui_action::UiError::key(key));
    }

    /// A slot turning active closes the shop (`0x54b04c`).
    fn on_battlefield_status(In(ev): In<SessionEvent>, world: &mut World) {
        if let SessionEvent::BattlefieldStatus(status) = ev {
            if status.status == 3 {
                close_in_turn(world);
            }
        }
    }
}

pub(crate) struct BarberUiPlugin;

impl Plugin for BarberUiPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<BarberShop>()
            .init_resource::<BarberBody>()
            .add_systems(
                Update,
                watch_close
                    .after(crate::net::apply_net_updates)
                    .in_set(BarberDrain),
            );
    }
}

/// The close's owed effects, run before the camera seats the view.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BarberDrain;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_open_shop_closes() {
        let mut s = BarberShop::default();
        assert!(!s.close());
        s.open = true;
        assert!(s.close());
        assert!(!s.close(), "a second close is silent");
    }

    #[test]
    fn sitting_down_in_the_chair_keeps_it_open_and_standing_up_closes_it() {
        let mut s = BarberShop {
            open: true,
            ..Default::default()
        };
        // The open lands before the server seats us: standing, then the chair.
        assert!(!s.watch(0));
        assert!(!s.watch(5));
        assert!(!s.watch(5));
        assert!(!s.watch(4), "another chair height");
        assert!(s.watch(0), "standing up");
        let mut s = BarberShop {
            open: true,
            ..Default::default()
        };
        assert!(!s.watch(5));
        assert!(s.watch(1), "sitting on the ground");
        let mut s = BarberShop::default();
        assert!(!s.watch(5) && !s.watch(0), "a closed shop watches nothing");
    }
}
