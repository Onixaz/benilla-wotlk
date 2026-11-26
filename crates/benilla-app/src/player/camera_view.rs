//! The eight camera views (`SetView`, `SaveView`, `ResetView`, `NextView`, `PrevView`) and
//! `FlipCameraYaw`: the engine half of [`benilla_ui::script::CameraViewRequest`].
//!
//! 12340 registers eight views (`0x5fdcb3`, names at `0xad1b54`: `""`, `A`..`E`, `Com`,
//! `Barber Shop`) × three fields (`0xad1b74`: `Distance`, `Pitch`, `Yaw`), each per-account and
//! hidden from Lua (`0x50`), with defaults at `0xad1ba8`; `cameraView` (`0x5fdc3c`, `0x10`)
//! defaults to `"2"` (`0xad1b90`, view `B`). Lua `SetView`/`SaveView`/`ResetView` accept `1..=5`
//! and pass the index straight through (`A`..`E`, where `1`/`A` is first person at `0.0` yd).
//! Pitch is in degrees down-positive, the opposite of [`FlyCam::pitch`], converted only by
//! [`super::camera_saved::pitch_from_file`].
//!
//! The views persist in the account's `config-cache.wtf` (slot 0). [`CameraViews`] is seeded from
//! the CVar registry at startup and reloaded once the account-data CVar slots finish loading
//! (`0x518c80` → `0x5fe510`). Every save or view switch writes back to the registry. The live view
//! is remembered, not applied at startup: [`super::camera_saved`]'s pose lands over it, as the
//! reference's saved pose lands over `cameraView`'s row at UI load.
//!
//! Deviation: `ResetView` writes the CVars back to their defaults, because the CVar store is our
//! persistence; the reference touches no CVar, so its reset reverts at next launch.
//!
//! `SetView` glides the distance and snaps pitch and yaw: the reference arms three glide channels,
//! the rig has only the distance one.

use bevy::prelude::*;

use benilla_ui::script::{
    CameraViewRequest, CAMERA_USER_VIEW_MAX, CAMERA_USER_VIEW_MIN, CAMERA_VIEW_COUNT,
};
use benilla_world::view::WorldCamera;

use crate::creature_anim::wrap_pi;

use super::camera::{CameraControl, FlyCam, CAM_PITCH_LIMIT};
use super::camera_saved::{pitch_from_file, pitch_to_file};
use super::camera_zoom::{CAM_DIST_MAX, CAM_DIST_MIN};
use super::Player;

/// The reference's eight views (`0xad1b54`), through the UI crate's constant so both sides agree.
const VIEW_COUNT: usize = CAMERA_VIEW_COUNT as usize;

/// The user-selectable views (`A`..`E`) that `NextView` (`0x604ce0`) and `PrevView` (`0x604d10`)
/// walk across.
const USER_VIEW_MIN: usize = CAMERA_USER_VIEW_MIN as usize;
const USER_VIEW_MAX: usize = CAMERA_USER_VIEW_MAX as usize;

/// The live view's CVar (`0x5fdc3c`, flags `0x10`), holding the internal index as `"%d"`.
pub(crate) const CVAR_ACTIVE_VIEW: &str = "cameraView";

/// The twenty-four saved-view CVars, `[view][field]` (distance, pitch, yaw), as `0x5fdcb3` composes
/// them from `0xad1b74` and `0xad1b54`; [`crate::cvars::REGISTERED`] must hold the same names
/// (tested).
pub(crate) const VIEW_CVARS: [[&str; 3]; VIEW_COUNT] = [
    ["cameraDistance", "cameraPitch", "cameraYaw"],
    ["cameraDistanceA", "cameraPitchA", "cameraYawA"],
    ["cameraDistanceB", "cameraPitchB", "cameraYawB"],
    ["cameraDistanceC", "cameraPitchC", "cameraYawC"],
    ["cameraDistanceD", "cameraPitchD", "cameraYawD"],
    ["cameraDistanceE", "cameraPitchE", "cameraYawE"],
    ["cameraDistanceCom", "cameraPitchCom", "cameraYawCom"],
    [
        "cameraDistanceBarber Shop",
        "cameraPitchBarber Shop",
        "cameraYawBarber Shop",
    ],
];

/// The reference's default strings (`0xad1ba8`), kept as text: [`crate::cvars`]'s file is a diff
/// against the registered default string, so a reset strips the key only by writing this text.
pub(crate) const VIEW_DEFAULTS: [[&str; 3]; VIEW_COUNT] = [
    ["0.0", "0.0", "0.0"],
    ["0.0", "0.0", "0.0"],
    ["5.55", "10.0", "0.0"],
    ["5.55", "20.0", "0.0"],
    ["13.88", "30.0", "0.0"],
    ["13.88", "10.0", "0.0"],
    ["0.0", "0.0", "0.0"],
    ["5.0", "10.0", "0.0"],
];

/// [`CVAR_ACTIVE_VIEW`]'s default (`0xad1b90`): internal index 2 (view `B`).
pub(crate) const ACTIVE_VIEW_DEFAULT: &str = "2";

/// One view's pose: yards, radians with [`FlyCam::pitch`]'s up-positive sign, and `yaw` as an
/// offset from the subject's facing, which the reference re-adds every frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ViewPose {
    pub(crate) distance: f32,
    pub(crate) pitch: f32,
    pub(crate) yaw: f32,
}

impl ViewPose {
    /// From the CVar units (yards, degrees down-positive, degrees), clamped to the rig's zoom range
    /// and ±89° pitch (reference validators: `[0, 50]`). Yaw is wrapped, as the reference validates
    /// `[0, 360]` but `SaveView` writes an offset.
    fn from_reference(distance: f32, pitch_deg: f32, yaw_deg: f32) -> Self {
        Self {
            distance: distance.clamp(CAM_DIST_MIN, CAM_DIST_MAX),
            pitch: pitch_from_file(pitch_deg),
            yaw: wrap_pi(yaw_deg.to_radians()),
        }
    }

    /// The three CVar strings, in the reference's `"%f"`.
    fn to_reference(self) -> [String; 3] {
        [
            format!("{:.6}", self.distance),
            format!("{:.6}", pitch_to_file(self.pitch)),
            format!("{:.6}", self.yaw.to_degrees()),
        ]
    }

    fn shipped(view: usize) -> Self {
        let row = VIEW_DEFAULTS[view];
        let parse = |s: &str| {
            s.parse::<f32>()
                .expect("the reference's default strings are floats")
        };
        Self::from_reference(parse(row[0]), parse(row[1]), parse(row[2]))
    }
}

/// The five views and the live one: the reference's `[cam+0xb0..+0xdc]` and `[cam+0xac]`.
#[derive(Resource, Debug, PartialEq)]
pub(crate) struct CameraViews {
    views: [ViewPose; VIEW_COUNT],
    /// The live view, persisted as [`CVAR_ACTIVE_VIEW`].
    current: usize,
    /// The view the barber chair left, restored by its close (`[0xacc784]`).
    barber_left: Option<usize>,
}

impl Default for CameraViews {
    fn default() -> Self {
        Self {
            views: std::array::from_fn(ViewPose::shipped),
            current: ACTIVE_VIEW_DEFAULT
                .parse::<usize>()
                .expect("the registrar default is an index"),
            barber_left: None,
        }
    }
}

impl CameraViews {
    fn pose(&self, view: usize) -> ViewPose {
        self.views[view]
    }

    /// `SetView`, and whether the index moved: the gate on writing the CVar.
    fn set(&mut self, view: usize) -> (ViewPose, bool) {
        let moved = self.current != view;
        self.current = view;
        (self.views[view], moved)
    }

    /// `SaveView`: the reference stores the camera's target fields, not the live ones.
    fn save(&mut self, view: usize, pose: ViewPose) {
        self.views[view] = pose;
    }

    /// `ResetView`; `true` when `view` is live, which the reference re-applies.
    fn reset(&mut self, view: usize) -> bool {
        self.views[view] = ViewPose::shipped(view);
        self.current == view
    }

    /// `NextView` (`0x604ce0`) and `PrevView` (`0x604d10`): step within `1..=5` (`A`..`E`), with
    /// no wrap at either end.
    fn step(&self, forward: bool) -> Option<usize> {
        if forward {
            (self.current < USER_VIEW_MAX).then(|| self.current + 1)
        } else {
            (self.current > USER_VIEW_MIN).then(|| self.current - 1)
        }
    }
}

/// Seed all eight views and the live index from the CVar registry (`0x5fe510`); an absent or
/// unparseable key falls back to its shipped default.
fn apply_cvars(cvars: &crate::cvars::Cvars, views: &mut CameraViews) {
    for (view, slot) in views.views.iter_mut().enumerate() {
        // Each field falls back on its own: a bad value costs that number, not the view.
        let field = |i: usize| {
            cvars
                .get(VIEW_CVARS[view][i])
                .and_then(|s| s.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite())
        };
        let shipped_row = VIEW_DEFAULTS[view];
        let raw = |i: usize| {
            field(i).unwrap_or_else(|| {
                shipped_row[i]
                    .parse()
                    .expect("the reference's default strings are floats")
            })
        };
        *slot = ViewPose::from_reference(raw(0), raw(1), raw(2));
    }
    views.current = cvars
        .get(CVAR_ACTIVE_VIEW)
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|v| *v < VIEW_COUNT)
        .unwrap_or_else(|| {
            ACTIVE_VIEW_DEFAULT
                .parse()
                .expect("the registrar default is an index")
        });
}

/// Startup: seed the views and the live index from the CVar registry before entering the world.
fn load_saved_views(cvars: Res<crate::cvars::Cvars>, mut views: ResMut<CameraViews>) {
    apply_cvars(&cvars, &mut views);
}

/// Reload the views from the CVar registry once the account-data CVar slots have loaded
/// (`0x518c80` → `0x5fe510`).
pub(crate) fn load_from_cvars(world: &mut World) {
    if !world.contains_resource::<crate::cvars::Cvars>()
        || !world.contains_resource::<CameraViews>()
    {
        return;
    }
    world.resource_scope(|world, cvars: Mut<crate::cvars::Cvars>| {
        let mut views = world.resource_mut::<CameraViews>();
        apply_cvars(&cvars, &mut views);
    });
}

/// The barber chair's view (view 7): level, facing the character from `distance`; the close goes
/// back to the view the chair left (`0x52ea2e`). Not one of the five saved views, so it writes no
/// CVar.
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub(crate) enum BarberView {
    Enter { distance: f32 },
    Leave,
}

/// Everything one drained request may write.
#[derive(bevy::ecs::system::SystemParam)]
struct ViewTargets<'w, 's> {
    views: ResMut<'w, CameraViews>,
    rig: ResMut<'w, CameraControl>,
    cam: Query<'w, 's, &'static mut FlyCam, With<WorldCamera>>,
    player: Res<'w, Player>,
}

/// Drain the Lua queue and the barber chair's view into the rig, before [`super::control`] seats
/// this frame's camera.
fn drain_view_requests(
    mut script: crate::ui_script::InTurn,
    mut cvars: ResMut<crate::cvars::Cvars>,
    mut targets: ViewTargets,
    mut barber: MessageReader<BarberView>,
) {
    for request in barber.read() {
        let Ok(mut cam) = targets.cam.single_mut() else {
            continue;
        };
        let face_yaw = targets.player.facing();
        match *request {
            BarberView::Enter { distance } => {
                let current = targets.views.current;
                targets.views.barber_left.get_or_insert(current);
                targets.rig.target_distance = distance.clamp(CAM_DIST_MIN, CAM_DIST_MAX);
                cam.pitch = 0.0;
                cam.yaw = wrap_pi(face_yaw + std::f32::consts::PI);
            }
            BarberView::Leave => {
                if let Some(view) = targets.views.barber_left.take() {
                    apply(
                        &mut targets.views,
                        &mut targets.rig,
                        &mut cam,
                        face_yaw,
                        view,
                    );
                }
            }
        }
    }
    let Some(mut script) = script.turn() else {
        return;
    };
    let requests = script.take_camera_view_requests();
    if requests.is_empty() {
        return;
    }
    let Ok(mut cam) = targets.cam.single_mut() else {
        return; // no camera: a silent drop, as on the reference's camera-lookup miss
    };
    let face_yaw = targets.player.facing();
    // Written to the CVars after the batch: the slots a save or reset moved, and the live index.
    let mut write_slots: Vec<usize> = Vec::new();
    let mut write_active = false;

    for request in requests {
        match request {
            CameraViewRequest::Set(n) => {
                write_active |= apply(
                    &mut targets.views,
                    &mut targets.rig,
                    &mut cam,
                    face_yaw,
                    usize::from(n),
                );
            }
            CameraViewRequest::Save(n) => {
                let view = usize::from(n);
                targets.views.save(
                    view,
                    ViewPose {
                        // The target zoom, not the collision-pulled arm (`cam+0x198`).
                        distance: targets.rig.target_distance,
                        pitch: cam.pitch,
                        yaw: wrap_pi(cam.yaw - face_yaw),
                    },
                );
                write_slots.push(view);
            }
            CameraViewRequest::Reset(n) => {
                let view = usize::from(n);
                if targets.views.reset(view) {
                    apply(
                        &mut targets.views,
                        &mut targets.rig,
                        &mut cam,
                        face_yaw,
                        view,
                    );
                }
                write_slots.push(view);
            }
            CameraViewRequest::Next | CameraViewRequest::Prev => {
                let forward = request == CameraViewRequest::Next;
                if let Some(view) = targets.views.step(forward) {
                    write_active |= apply(
                        &mut targets.views,
                        &mut targets.rig,
                        &mut cam,
                        face_yaw,
                        view,
                    );
                }
            }
            CameraViewRequest::FlipYaw(degrees) => {
                // `cam[+0x100] += arg * pi/180`. The reference keeps the flip in its own yaw field,
                // added outside the follow channel, so it survives re-centring; the rig has one
                // yaw, so auto-follow reels the flip back in on Smart and Always.
                cam.yaw = wrap_pi(cam.yaw + degrees.to_radians());
            }
        }
    }

    // Mirror into the registry so the change reaches the account's `config-cache.wtf`.
    for view in write_slots {
        // A reset writes the default string verbatim, so the diff-shaped file drops the key.
        let reset_to_default = targets.views.pose(view) == ViewPose::shipped(view);
        let rendered = targets.views.pose(view).to_reference();
        for field in 0..3 {
            let value = if reset_to_default {
                VIEW_DEFAULTS[view][field]
            } else {
                rendered[field].as_str()
            };
            cvars.set(VIEW_CVARS[view][field], value);
        }
    }
    if write_active {
        cvars.set(CVAR_ACTIVE_VIEW, &targets.views.current.to_string());
    }
}

/// Seat one view; the distance glides through `target_distance`. `true` when the live index moved.
fn apply(
    views: &mut CameraViews,
    rig: &mut CameraControl,
    cam: &mut FlyCam,
    face_yaw: f32,
    view: usize,
) -> bool {
    let (pose, moved) = views.set(view);
    rig.target_distance = pose.distance;
    cam.pitch = pose.pitch.clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT);
    cam.yaw = wrap_pi(face_yaw + pose.yaw);
    moved
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<CameraViews>()
        .add_message::<BarberView>()
        // After the config load, so the persisted views are in before anything selects one.
        .add_systems(Startup, load_saved_views.after(crate::cvars::CvarLoad))
        .add_systems(
            Update,
            crate::ui_script::raises::then_fire(drain_view_requests)
                // After the Lua tick that queues them, before the control pass seats the pose.
                .in_set(crate::ui_script::UiDrain)
                .after(crate::ui_barber::BarberDrain)
                .before(super::control)
                // Capture parks the camera itself; a queued view must not move it.
                .run_if(not(resource_exists::<crate::run_mode::CaptureMode>)),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_shipped_view_lands_its_recorded_pose() {
        // (internal index, distance yd, pitch in degrees down-positive) from `0xad1ba8`.
        let table = [
            (0, 0.0_f32, 0.0_f32),
            (1, 0.0, 0.0),
            (2, 5.55, 10.0),
            (3, 5.55, 20.0),
            (4, 13.88, 30.0),
            (5, 13.88, 10.0),
            (6, 0.0, 0.0),
            (7, 5.0, 10.0),
        ];
        let views = CameraViews::default();
        for (index, distance, pitch_deg) in table {
            let pose = views.pose(index);
            assert_eq!(pose.distance, distance, "view {index} distance");
            assert!(
                (pose.pitch - pitch_from_file(pitch_deg)).abs() < 1e-6,
                "view {index} pitch: {} vs {}",
                pose.pitch,
                pitch_from_file(pitch_deg)
            );
            assert_eq!(pose.yaw, 0.0, "view {index} yaw is behind the character");
            assert!(
                pose.pitch <= 0.0,
                "a positive saved pitch looks DOWN for us"
            );
        }
    }

    #[test]
    fn view_one_is_real_first_person_not_a_near_stop() {
        assert_eq!(CAM_DIST_MIN, 0.0);
        // Lua `SetView(1)` selects internal index 1 (view `A`), which is first person in 12340.
        assert_eq!(CameraViews::default().pose(1).distance, 0.0);
    }

    #[test]
    fn the_shipped_view_is_third_person_b() {
        assert_eq!(CameraViews::default().current, 2);
        assert_eq!(CameraViews::default().pose(2).distance, 5.55);
    }

    #[test]
    fn a_saved_view_round_trips_and_reset_restores_the_default() {
        let mut views = CameraViews::default();
        let saved = ViewPose {
            distance: 12.25,
            pitch: -0.42,
            yaw: 0.75,
        };
        views.save(2, saved);
        views.save(
            3,
            ViewPose {
                distance: 1.0,
                pitch: 0.1,
                yaw: -2.0,
            },
        );
        assert_eq!(views.pose(2), saved);

        let [d, p, y] = saved.to_reference();
        let reloaded =
            ViewPose::from_reference(d.parse().unwrap(), p.parse().unwrap(), y.parse().unwrap());
        assert!((reloaded.distance - saved.distance).abs() < 1e-4);
        assert!((reloaded.pitch - saved.pitch).abs() < 1e-4);
        assert!((reloaded.yaw - saved.yaw).abs() < 1e-4);

        assert!(views.reset(2), "view 2 is the default live one");
        assert_eq!(views.pose(2), ViewPose::shipped(2));
        assert!(!views.reset(3), "view 3 was not the live one");
        views.set(3);
        assert!(views.reset(3));
    }

    /// Lua `SaveView(1)` and `ResetView(1)` act on internal view 1 (`A`).
    #[test]
    fn view_one_is_saveable_and_resettable() {
        let mut views = CameraViews::default();
        let pose = ViewPose {
            distance: 3.0,
            pitch: -0.2,
            yaw: 0.0,
        };
        views.save(1, pose);
        assert_eq!(views.pose(1), pose);
        views.reset(1);
        assert_eq!(views.pose(1), ViewPose::shipped(1));
        assert_eq!(views.pose(1).distance, 0.0);
    }

    #[test]
    fn next_and_prev_stop_at_the_ends() {
        let mut views = CameraViews::default();
        views.set(USER_VIEW_MIN);
        assert_eq!(views.step(false), None, "PrevView at view 1 (A) is a no-op");
        for expected in (USER_VIEW_MIN + 1)..=USER_VIEW_MAX {
            let next = views.step(true).expect("a view above this one");
            assert_eq!(next, expected);
            views.set(next);
        }
        assert_eq!(views.step(true), None, "NextView at view 5 (E) is a no-op");
        for expected in (USER_VIEW_MIN..USER_VIEW_MAX).rev() {
            let prev = views.step(false).expect("a view below this one");
            assert_eq!(prev, expected);
            views.set(prev);
        }
        assert_eq!(views.step(false), None);
    }

    /// 180 is the stock binding's argument (`Bindings.xml:796`).
    #[test]
    fn flipping_the_yaw_twice_returns_the_original() {
        let flip = |yaw: f32, degrees: f32| wrap_pi(yaw + degrees.to_radians());
        for start in [0.0_f32, 1.3, -2.9, std::f32::consts::PI] {
            let once = flip(start, 180.0);
            assert!(
                (wrap_pi(once - start).abs() - std::f32::consts::PI).abs() < 1e-5,
                "one flip is half a turn"
            );
            let twice = flip(once, 180.0);
            assert!(
                wrap_pi(twice - start).abs() < 1e-5,
                "two flips return: {twice} vs {start}"
            );
        }
    }

    #[test]
    fn a_hand_edited_view_cannot_land_an_illegal_pose() {
        let wild = ViewPose::from_reference(999.0, 400.0, 720.0 + 90.0);
        assert_eq!(wild.distance, CAM_DIST_MAX);
        assert_eq!(wild.pitch, -CAM_PITCH_LIMIT);
        assert!((wild.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert_eq!(
            ViewPose::from_reference(-5.0, -400.0, 0.0).distance,
            CAM_DIST_MIN
        );
        assert_eq!(
            ViewPose::from_reference(-5.0, -400.0, 0.0).pitch,
            CAM_PITCH_LIMIT
        );
    }

    #[test]
    fn the_view_cvars_are_registered_with_the_references_defaults_and_flags() {
        let registered = |name: &str| {
            crate::cvars::REGISTERED
                .iter()
                .find(|r| r.name.eq_ignore_ascii_case(name))
        };
        for view in 0..VIEW_COUNT {
            for field in 0..3 {
                let name = VIEW_CVARS[view][field];
                let row = registered(name).unwrap_or_else(|| panic!("{name} is not registered"));
                assert_eq!(row.default, VIEW_DEFAULTS[view][field], "{name} default");
                assert_eq!(row.scope, crate::cvars::Scope::Account, "{name} scope");
                assert!(row.hidden, "{name} must be hidden from Lua (flag 0x40)");
            }
        }
        let active = registered(CVAR_ACTIVE_VIEW).expect("cameraView is registered");
        assert_eq!(active.default, ACTIVE_VIEW_DEFAULT);
        assert_eq!(active.scope, crate::cvars::Scope::Account);
        assert!(!active.hidden, "cameraView is visible to Lua (flag 0x10)");

        // Hidden rows never reach the Lua VM mirror; `cameraView` does.
        let cvars = crate::cvars::Cvars::default();
        let seeded = cvars.vm_seed();
        assert!(
            seeded
                .iter()
                .any(|s| s.name == CVAR_ACTIVE_VIEW && s.default == "2" && s.per_account),
            "cameraView must be seeded into Lua at default 2"
        );
        for row in &VIEW_CVARS {
            for &name in row {
                assert!(
                    !seeded.iter().any(|s| s.name.eq_ignore_ascii_case(name)),
                    "{name} must not be seeded into Lua"
                );
            }
        }
    }

    #[test]
    fn reference_config_cache_restores_the_same_views() {
        let mut cvars = crate::cvars::Cvars::default();
        cvars.load_scope(
            crate::cvars::Scope::Account,
            concat!(
                "SET cameraView \"4\"\n",
                "SET cameraDistanceB \"9.250000\"\n",
                "SET cameraPitchB \"22.500000\"\n",
                "SET cameraYawB \"45.000000\"\n",
                "SET cameraDistanceE \"20.000000\"\n",
            ),
        );
        let mut views = CameraViews::default();
        apply_cvars(&cvars, &mut views);

        assert_eq!(views.current, 4);
        assert_eq!(views.pose(2).distance, 9.25);
        assert!((views.pose(2).pitch - pitch_from_file(22.5)).abs() < 1e-6);
        assert!((views.pose(2).yaw - 45.0_f32.to_radians()).abs() < 1e-6);
        assert_eq!(views.pose(5).distance, 20.0);
        // Unmodified views keep their 12340 shipped defaults.
        assert_eq!(views.pose(1), ViewPose::shipped(1));
        assert_eq!(views.pose(3), ViewPose::shipped(3));

        // Account config-cache.wtf carries the modified rows back out.
        let saved = cvars.compose_scope(crate::cvars::Scope::Account);
        assert!(saved.contains("SET cameraView \"4\"\n"), "{saved}");
        assert!(
            saved.contains("SET cameraDistanceB \"9.250000\"\n"),
            "{saved}"
        );
        assert!(
            saved.contains("SET cameraDistanceE \"20.000000\"\n"),
            "{saved}"
        );
    }
}
