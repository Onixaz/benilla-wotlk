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
use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::player::camera_view::BarberView;
use crate::player::camera_zoom::ZoomLimit;
use crate::player::StandStateRequest;
use crate::ui_script::live::UiEvents;

/// The barber's cape, the `ItemDisplayInfo` the chair puts in the tabard slot (`0x52fb2a`).
pub(crate) const BARBER_CAPE_DISPLAY: u32 = 54559;

/// The stand states of a chair, `UNIT_SITTINGCHAIRLOW` to `HIGH`: setting one keeps the shop
/// open, any other state closes it (`0x6e2ba3`).
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
    /// A close inside a call or the stand-state setter owes the view and the zoom floor, which [`close_effects`] runs.
    owed_view: bool,
    /// A close from `CancelBarberShop()` owes the stand-up too; the setter stands up itself.
    owed_stand: bool,
}

impl BarberShop {
    /// The close (`0x52e9f0`): only an open shop closes. True when it closed.
    fn close(&mut self) -> bool {
        std::mem::take(&mut self.open)
    }
}

/// The stand-state setter's barber leg (`0x6e2b30`), which the client's own change (`0x6dcb40`)
/// and `SMSG_STANDSTATE_UPDATE` (`0x73f57a`) reach in their turn; our own field echo never does
/// (`0x73f474`).
#[derive(SystemParam)]
pub(crate) struct BarberStand<'w> {
    shop: Option<ResMut<'w, BarberShop>>,
    body: Option<ResMut<'w, BarberBody>>,
    ui: Option<ResMut<'w, UiEvents>>,
}

impl BarberStand<'_> {
    /// A new client stand state (`0x6e2ba3`): any but a chair's closes an open shop (`0x6e2bb6`) and fires `BARBER_SHOP_CLOSE`; the close's stand-up (`0x52ea5f`) is the caller's. True when it closed.
    pub(crate) fn stand_state_set(&mut self, state: u8) -> bool {
        if CHAIR_STAND_STATES.contains(&state) {
            return false;
        }
        let Some(shop) = self.shop.as_mut() else {
            return false;
        };
        if !shop.close() {
            return false;
        }
        shop.owed_view = true;
        if let Some(ui) = self.ui.as_mut() {
            ui.fire("BARBER_SHOP_CLOSE", vec![]);
        }
        if let Some(b) = self.body.as_mut() {
            **b = BarberBody::default();
        }
        true
    }
}

/// The view the chair left and the zoom floor (`0x52ea2f`), owed by a close that had no camera at
/// hand, and a cancel's stand-up (`0x52ea5f`), run before the camera seats the view.
fn close_effects(
    mut shop: ResMut<BarberShop>,
    mut view: MessageWriter<BarberView>,
    mut zoom: ResMut<ZoomLimit>,
    mut stand: MessageWriter<StandStateRequest>,
) {
    if std::mem::take(&mut shop.owed_view) {
        view.write(BarberView::Leave);
        zoom.barber_floor = None;
    }
    if std::mem::take(&mut shop.owed_stand) {
        stand.write(StandStateRequest { state: 0 });
    }
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

    /// `CancelBarberShop()`'s close (`0x52e9f0`): the flag and our body put back in the call, the camera's and the stand-up's effects owed to [`close_effects`].
    pub(crate) fn close(&mut self) -> bool {
        let Some(shop) = self.shop.as_mut() else {
            return false;
        };
        if !shop.close() {
            return false;
        }
        shop.owed_view = true;
        shop.owed_stand = true;
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

/// The world change's close, ahead of the object manager's teardown (`0x403ba2`).
pub(crate) fn world_left(world: &mut World) {
    close_in_turn(world);
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
                close_effects
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
    fn a_chair_keeps_it_open_and_any_other_stand_state_closes_it() {
        let mut world = World::new();
        world.insert_resource(BarberShop {
            open: true,
            ..Default::default()
        });
        world.insert_resource(BarberBody {
            open: true,
            preview: Some([1; 5]),
        });
        world.init_resource::<UiEvents>();
        let set = |world: &mut World, state: u8| {
            world
                .run_system_cached_with(
                    |In(state): In<u8>, mut b: BarberStand| b.stand_state_set(state),
                    state,
                )
                .unwrap()
        };
        for chair in 4..=6 {
            assert!(!set(&mut world, chair), "chair height {chair}");
        }
        assert!(world.resource::<UiEvents>().names().is_empty());
        assert!(set(&mut world, 1), "sitting on the ground");
        assert_eq!(
            world.resource::<UiEvents>().names(),
            vec!["BARBER_SHOP_CLOSE"]
        );
        assert!(world.resource::<BarberBody>() == &BarberBody::default());
        let shop = world.resource::<BarberShop>();
        assert!(!shop.open && shop.owed_view && !shop.owed_stand);
        assert!(!set(&mut world, 0), "a closed shop closes once");
        assert_eq!(world.resource::<UiEvents>().names().len(), 1);
    }
}
