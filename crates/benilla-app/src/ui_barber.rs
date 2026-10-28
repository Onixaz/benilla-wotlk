//! The barber shop, the app half of the stock `Blizzard_BarbershopUI`, which `UIParent` loads on
//! `BARBER_SHOP_OPEN`. The chair's `SMSG_ENABLE_BARBER_SHOP` opens it (`0x52f9b0`): our own body
//! takes off its helm, shoulders and shield, wears the barber's cape and previews the Lua dials,
//! and the camera turns to face it. `CancelBarberShop()`, leaving the chair, a display change, a
//! world change and an active battleground close it (`0x52e9f0`), which puts all of it back.

use benilla_assets::ModelAnimations;
use bevy::prelude::*;

use benilla_formats::{BarberShopStyle, GameTableId};
use benilla_ui::script::{BarberHost, BarberIntent, BarberStyle, GlueCustomizeGrid, UiScript};

use crate::creature_anim::AnimDriver;
use crate::net::{
    ClientCommand, EnteredWorldMessage, NetCommands, ObjectStore, SelfPlayer, WorldportMessage,
};
use crate::player::camera_view::BarberView;
use crate::player::camera_zoom::ZoomLimit;
use crate::player::{Player, StandStateRequest};
use crate::ui_script::{UiFeed, UiInput};

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

/// The shop's open flag (`[0xbd19b8]`), the events and lines owed to the next feed, and what the
/// close watches: the stand state and display the shop opened on.
#[derive(Resource, Default)]
pub(crate) struct BarberShop {
    open: bool,
    events: Vec<&'static str>,
    lines: Vec<&'static str>,
    stand: Option<u8>,
    display: Option<i32>,
}

impl BarberShop {
    /// `SMSG_ENABLE_BARBER_SHOP` in (`0x52f9b0`).
    fn open(&mut self) {
        self.open = true;
        self.stand = None;
        self.display = None;
        self.events.push("BARBER_SHOP_OPEN");
    }

    /// The close (`0x52e9f0`): only an open shop closes, and fires `BARBER_SHOP_CLOSE`. True when
    /// it closed.
    fn close(&mut self) -> bool {
        if !std::mem::take(&mut self.open) {
            return false;
        }
        self.events.push("BARBER_SHOP_CLOSE");
        true
    }

    /// `SMSG_BARBER_SHOP_RESULT` in (`0x52e5b0`).
    fn result(&mut self, result: u32) {
        match result {
            0 => self.events.push("BARBER_SHOP_SUCCESS"),
            1 | 3 => self.lines.push("ERR_NOT_ENOUGH_MONEY"),
            2 => self.lines.push("ERR_NOT_BARBER_SITTING"),
            _ => {}
        }
    }

    /// Whether a change of the watched stand state or display closes the shop: leaving the chair
    /// for any other state (`0x6e2b30`), or a new display (`0x716860`). The first sight of each
    /// after the open only records it.
    fn watch(&mut self, stand: u8, display: i32) -> bool {
        if !self.open {
            return false;
        }
        let left_chair = self
            .stand
            .replace(stand)
            .is_some_and(|old| old != stand && !CHAIR_STAND_STATES.contains(&stand));
        let new_display = self.display.replace(display).is_some_and(|d| d != display);
        left_chair || new_display
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

/// The self facts the host is built from; a change rebuilds it.
#[derive(Clone, Copy, PartialEq)]
struct HostKey {
    open: bool,
    race: u8,
    sex: u8,
    class: u8,
    level: u32,
    look: [u8; 5],
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

#[allow(clippy::too_many_arguments)]
fn feed_barber(
    script: Option<NonSendMut<UiScript>>,
    mut shop: ResMut<BarberShop>,
    mut body: ResMut<BarberBody>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    styles: Option<Res<BarberStyles>>,
    catalog: Option<Res<crate::entities::CharCreate>>,
    sheet: Option<Res<crate::ui_char::SheetTables>>,
    mut last_host: Local<crate::ui_script::VmMemo<Option<HostKey>>>,
    mut last_look: Local<crate::ui_script::VmMemo<Option<[u8; 5]>>>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    let store = self_q.iter().next();
    if let Some(s) = store {
        let f = &s.0;
        let key = HostKey {
            open: shop.open,
            race: f.unit_race().unwrap_or(0),
            sex: f.unit_gender().unwrap_or(0),
            class: f.unit_class().unwrap_or(0),
            level: f.unit_level().unwrap_or(0),
            look: self_look(s),
        };
        let last = last_host.get(&script);
        if *last != Some(key) {
            *last = Some(key);
            let grid = catalog
                .as_deref()
                .and_then(|c| c.0.section_grid(key.race, key.sex))
                .map(|g| GlueCustomizeGrid {
                    sections: g.sections.clone(),
                    facial_hair_styles: g.facial_hair_styles,
                });
            let tokens = catalog.as_deref().map(|c| {
                (
                    c.0.hair_customization(key.race).unwrap_or_default(),
                    c.0.facial_hair_customization(key.race, key.sex)
                        .unwrap_or_default(),
                )
            });
            script.set_barber_host(BarberHost {
                open: key.open,
                class: key.class,
                base_cost: sheet.as_deref().map_or(0.0, |t| {
                    t.gt.lookup(GameTableId::BarberShopCostBase, key.level as i32 - 1, 0)
                }),
                look: key.look,
                styles: styles
                    .as_deref()
                    .map(|s| {
                        s.0.iter()
                            .filter(|r| r.fits(key.race, key.sex))
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
                    .unwrap_or_default(),
                grid,
                hair_customization: tokens.map(|t| t.0.to_string()).unwrap_or_default(),
                facial_hair_customization: tokens.map(|t| t.1.to_string()).unwrap_or_default(),
            });
        }
        // A server change of our hair, its colour, the facial hair or the skin is the applied
        // appearance (`0x6da7f0`, `0x6da870`, `0x6df530`, `0x6df620`), open or not; the face has
        // no such handler.
        let look = self_look(s);
        let seen = last_look.get(&script);
        let applied = seen.is_some_and(|old| [0, 2, 3, 4].iter().any(|&i| old[i] != look[i]));
        *seen = Some(look);
        if applied {
            script.fire_event("BARBER_SHOP_APPEARANCE_APPLIED", vec![]);
        }
    }

    for event in std::mem::take(&mut shop.events) {
        script.fire_event(event, vec![]);
    }
    let lines: Vec<_> = shop
        .lines
        .drain(..)
        .filter_map(|key| crate::ui_action::keyed_line(&script, key))
        .collect();
    crate::ui_action::show_messages(&mut script, &mut sink, "ui_barber", lines);

    let next = BarberBody {
        open: shop.open,
        preview: script.barber_preview(),
    };
    if *body != next {
        *body = next;
    }
}

#[allow(clippy::too_many_arguments)]
fn drain_barber(
    script: Option<NonSendMut<UiScript>>,
    mut shop: ResMut<BarberShop>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
    mut view: MessageWriter<BarberView>,
    mut zoom: ResMut<ZoomLimit>,
    mut stand: MessageWriter<StandStateRequest>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    for intent in script.take_barber_intents() {
        match intent {
            BarberIntent::Apply { styles, cost } => {
                let purse = self_q
                    .iter()
                    .next()
                    .and_then(|s| s.0.player_money())
                    .unwrap_or(0);
                // The client's own check, with no packet (`0x52ee6e`).
                if cost > purse {
                    if let Some(line) =
                        crate::ui_action::keyed_line(&script, "ERR_NOT_ENOUGH_MONEY")
                    {
                        crate::ui_action::show_messages(
                            &mut script,
                            &mut sink,
                            "ui_barber",
                            [line],
                        );
                    }
                    continue;
                }
                let _ = commands.0.send(ClientCommand::AlterAppearance { styles });
            }
            BarberIntent::Cancel => {
                if shop.close() {
                    close_effects(&mut view, &mut zoom, &mut stand);
                }
            }
        }
    }
}

/// The close's other triggers: leaving the chair, a display change, a world change.
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
) {
    // A world change closes it (`0x403b70`).
    let world_changed = entered.read().count() + worldports.read().count() > 0;
    let watched = self_q.iter().next().is_some_and(|s| {
        let f = &s.0;
        let state = player.committed_stand_state(f.unit_stand_state());
        shop.watch(state, f.unit_displayid().unwrap_or(0))
    });
    if (world_changed || watched) && shop.close() {
        close_effects(&mut view, &mut zoom, &mut stand);
    }
}

/// The drain of the verbs' intents, which queues the close's view before the camera seats it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BarberDrain;

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

    /// The open needs our own body (`0x52f9c4`). The view's distance is the body's playing
    /// sequence's bounds radius, twice over, three times for a Gnome (`0x52fbf1`); the radius
    /// floors the zoom while the shop is open.
    #[allow(clippy::type_complexity)]
    fn on_enable(
        In(ev): In<SessionEvent>,
        mut shop: ResMut<BarberShop>,
        self_q: Query<
            (&ObjectStore, Option<&ModelAnimations>, Option<&AnimDriver>),
            With<SelfPlayer>,
        >,
        mut view: MessageWriter<BarberView>,
        mut zoom: ResMut<ZoomLimit>,
    ) {
        if !matches!(ev, SessionEvent::EnableBarberShop) {
            return;
        }
        let Some((store, anims, driver)) = self_q.iter().next() else {
            return;
        };
        shop.open();
        let radius = match (anims, driver) {
            (Some(a), Some(d)) => d
                .active_anim()
                .and_then(|id| a.find(id))
                .map_or(1.0, |c| c.bounds_radius),
            _ => 1.0,
        };
        let scale = if store.0.unit_race() == Some(7) {
            3.0
        } else {
            2.0
        };
        zoom.barber_floor = Some(radius);
        view.write(BarberView::Enter {
            distance: radius * scale,
        });
    }

    fn on_result(In(ev): In<SessionEvent>, mut shop: ResMut<BarberShop>) {
        if let SessionEvent::BarberShopResult(result) = ev {
            shop.result(result);
        }
    }

    /// A slot turning active closes the shop (`0x54b04c`).
    fn on_battlefield_status(
        In(ev): In<SessionEvent>,
        mut shop: ResMut<BarberShop>,
        mut view: MessageWriter<BarberView>,
        mut zoom: ResMut<ZoomLimit>,
        mut stand: MessageWriter<StandStateRequest>,
    ) {
        if let SessionEvent::BattlefieldStatus(status) = ev {
            if status.status == 3 && shop.close() {
                close_effects(&mut view, &mut zoom, &mut stand);
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
                (
                    watch_close
                        .after(crate::net::apply_net_updates)
                        .before(feed_barber),
                    feed_barber
                        .in_set(UiFeed)
                        .run_if(crate::ui_script::ingame_ui_up),
                    drain_barber.after(UiInput).in_set(BarberDrain),
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_close_fires_once_and_only_on_an_open_shop() {
        let mut s = BarberShop::default();
        assert!(!s.close());
        assert!(s.events.is_empty());
        s.open();
        assert!(s.close());
        assert_eq!(s.events, vec!["BARBER_SHOP_OPEN", "BARBER_SHOP_CLOSE"]);
        assert!(!s.close(), "a second close is silent");
    }

    #[test]
    fn the_result_fires_success_or_shows_its_error() {
        let mut s = BarberShop::default();
        s.result(0);
        s.result(1);
        s.result(2);
        s.result(3);
        s.result(4);
        assert_eq!(s.events, vec!["BARBER_SHOP_SUCCESS"]);
        assert_eq!(
            s.lines,
            vec![
                "ERR_NOT_ENOUGH_MONEY",
                "ERR_NOT_BARBER_SITTING",
                "ERR_NOT_ENOUGH_MONEY"
            ]
        );
    }

    #[test]
    fn sitting_down_in_the_chair_keeps_it_open_and_standing_up_closes_it() {
        let mut s = BarberShop::default();
        s.open();
        // The open lands before the server seats us: standing, then the chair.
        assert!(!s.watch(0, 49));
        assert!(!s.watch(5, 49));
        assert!(!s.watch(5, 49));
        assert!(!s.watch(4, 49), "another chair height");
        assert!(s.watch(0, 49), "standing up");
        let mut s = BarberShop::default();
        s.open();
        assert!(!s.watch(5, 49));
        assert!(s.watch(1, 49), "sitting on the ground");
        let mut s = BarberShop::default();
        s.open();
        assert!(!s.watch(5, 49));
        assert!(s.watch(5, 50), "a new display");
        let mut s = BarberShop::default();
        assert!(
            !s.watch(5, 49) && !s.watch(0, 50),
            "a closed shop watches nothing"
        );
    }
}
