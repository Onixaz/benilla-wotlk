//! Missile motion scripts (`SpellVisual` column 21 → `SpellMissileMotion.dbc`). Each frame a
//! scripted missile calls its row's Lua function (`0x700350`) with its progress, age, copy index,
//! distances, three per-copy randoms and the spell id; the results offset the model from its path
//! (`0x700550`), turn and scale it (`0x6feb20`) and may set its speed. The functions run in a
//! sandbox holding `math` and the degree-based aliases `0x758c50` installs, nothing else.
//! Deviation: a Lua state of their own, not the interface's, so its VM stays off the missile path.

use std::collections::HashMap;

use bevy::prelude::*;
use mlua::{Function, Lua, Table, Value};

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_formats::{MissileMotionRec, SpellVisualCatalog};

use super::{Missile, MotionPose};

/// The sandbox's math aliases, as `0x758c50` runs them: trigonometry in degrees.
const MATH_ALIASES: &str = "local math = math
abs = math.abs
acos = function (x) return math.deg(math.acos(x)) end
asin = function (x) return math.deg(math.asin(x)) end
atan = function (x) return math.deg(math.atan(x)) end
atan2 = function (x,y) return math.deg(math.atan2(x,y)) end
ceil = math.ceil
cos = function (x) return math.cos(math.rad(x)) end
deg = math.deg
exp = math.exp
floor = math.floor
frexp = math.frexp
ldexp = math.ldexp
log = math.log
log10 = math.log10
max = math.max
min = math.min
mod = math.fmod
PI = math.pi
rad = math.rad
random = math.random
randomseed = math.randomseed
sin = function (x) return math.sin(math.rad(x)) end
sqrt = math.sqrt
tan = function (x) return math.tan(math.rad(x)) end
";

/// A model this far (squared) off its path takes the hit kit at the model (`0x7006c0`).
const OFF_PATH_SQ: f32 = 0.2;

/// The script state and each row's function; `None` flies unscripted (`0x75856f`).
#[derive(Default)]
pub(crate) struct MotionScripts {
    lua: Option<(Lua, Table)>,
    funcs: HashMap<u32, Option<Function>>,
}

impl MotionScripts {
    /// The sandbox: a fresh environment with `math` and the aliases (`0x758c50`).
    fn sandbox() -> mlua::Result<(Lua, Table)> {
        let lua = Lua::new();
        let env = lua.create_table()?;
        env.set("math", lua.globals().get::<Value>("math")?)?;
        lua.load(MATH_ALIASES)
            .set_name("MathAliases")
            .set_environment(env.clone())
            .exec()?;
        Ok((lua, env))
    }

    /// Motion `id`'s function, compiled on the first ask (`0x7587d0`).
    fn function(&mut self, id: u32, visuals: &SpellVisualCatalog) -> Option<&Function> {
        if !self.funcs.contains_key(&id) {
            let compiled = visuals
                .missile_motion(id)
                .and_then(|rec| self.compile(id, rec));
            self.funcs.insert(id, compiled);
        }
        self.funcs.get(&id)?.as_ref()
    }

    /// Compile a row in the sandbox, building the state on first use.
    fn compile(&mut self, id: u32, rec: &MissileMotionRec) -> Option<Function> {
        if self.lua.is_none() {
            self.lua = Self::sandbox()
                .map_err(|e| warn!("missile motion: the script state failed: {e}"))
                .ok();
        }
        let (lua, env) = self.lua.as_ref()?;
        lua.load(rec.chunk())
            .set_name(rec.name.as_str())
            .set_environment(env.clone())
            .eval::<Function>()
            .map_err(|e| warn!("missile motion {id} ({}): {e}", rec.name))
            .ok()
    }

    /// Drop a function whose call failed: every missile on it flies unscripted after (`0x75856f`).
    fn fail(&mut self, id: u32) {
        self.funcs.insert(id, None);
    }
}

/// A scripted missile copy, the reference's motion handle with its per-copy inputs.
#[derive(Component)]
pub(crate) struct MissileMotion {
    /// The `SpellMissileMotion` row.
    pub(crate) id: u32,
    pub(crate) index: u32,
    pub(crate) count: u32,
    /// `rand1`-`rand3`, drawn per copy at launch in `[0, 1)` (`0x702480`).
    pub(crate) rand: [f32; 3],
    /// `Time::elapsed_secs` at launch, the `time` argument's zero.
    pub(crate) fired_at: f32,
    /// The launch-to-aim distance at launch, `startDistance`.
    pub(crate) start_distance: f32,
}

/// Three `[0, 1)` draws for one copy, the mantissa fill of `0x702480`.
pub(crate) fn draw_randoms() -> [f32; 3] {
    use std::sync::atomic::{AtomicU64, Ordering};
    static STATE: AtomicU64 = AtomicU64::new(0);
    let next = || {
        // SplitMix64 over a shared counter.
        let mut z = STATE
            .fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed)
            .wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        let bits = ((z ^ (z >> 31)) as u32 & 0x7f_ffff) | 0x3f80_0000;
        f32::from_bits(bits) - 1.0
    };
    [next(), next(), next()]
}

/// A script's twelve results, non-finite ones zeroed (`0x7004ae`).
type Outputs = [f64; 12];

fn number(v: &Value) -> f64 {
    let n = match v {
        Value::Number(n) => *n,
        Value::Integer(i) => *i as f64,
        // `lua_tonumber`: a numeric string reads as its number, anything else 0.
        Value::String(s) => s
            .to_str()
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0.0),
        _ => 0.0,
    };
    if n.is_finite() {
        n
    } else {
        0.0
    }
}

/// Run one copy's script; `None` when it fails.
fn call(func: &Function, args: [f64; 12]) -> Option<Outputs> {
    let r = func
        .call::<(
            Value,
            Value,
            Value,
            Value,
            Value,
            Value,
            Value,
            Value,
            Value,
            Value,
            Value,
            Value,
        )>((
            args[0], args[1], args[2], args[3], args[4], args[5], args[6], args[7], args[8],
            args[9], args[10], args[11],
        ))
        .ok()?;
    Some([
        number(&r.0),
        number(&r.1),
        number(&r.2),
        number(&r.3),
        number(&r.4),
        number(&r.5),
        number(&r.6),
        number(&r.7),
        number(&r.8),
        number(&r.9),
        number(&r.10),
        number(&r.11),
    ])
}

/// The model's offset off its path, in Bevy space (`0x700550`).
fn translation(out: &Outputs, launch: Vec3, impact: Vec3) -> Vec3 {
    let [dx, dy, _] = bevy_to_wow(impact - launch);
    let len = (dx * dx + dy * dy).sqrt();
    let (dx, dy) = if len * len > 2.384_185_8e-7 {
        (dx / len, dy / len)
    } else {
        (dx, dy)
    };
    let side = [-dy, dx];
    let mut o = [0.0f32; 3];
    if out[1] != 0.0 {
        let angle = (out[0] as f32).to_radians();
        let mag = out[1] as f32;
        o[0] += angle.sin() * side[0] * mag;
        o[1] += angle.sin() * side[1] * mag;
        o[2] += angle.cos() * mag;
    }
    let (right, front, up) = (out[2] as f32, out[3] as f32, out[4] as f32);
    o[0] += side[0] * right + dx * front;
    o[1] += side[1] * right + dy * front;
    o[2] += up;
    wow_to_bevy(o)
}

/// Yaw, pitch and roll in degrees about the model's up, left and forward (`0x6feb20`).
fn turn(out: &Outputs) -> Quat {
    let r = |v: f64| (v as f32).to_radians();
    Quat::from_axis_angle(Vec3::Y, r(out[5]))
        * Quat::from_axis_angle(Vec3::NEG_X, r(out[6]))
        * Quat::from_axis_angle(Vec3::NEG_Z, r(out[7]))
}

/// Run each scripted copy (`0x701230`): its pose draws the model, its speed retimes (`0x7004cd`).
pub(crate) fn run_missile_motion(
    mut scripts: NonSendMut<MotionScripts>,
    visuals: Option<Res<crate::creature_anim::SpellVisuals>>,
    time: Res<Time>,
    mut commands: Commands,
    mut missiles: Query<(Entity, &mut Missile, &MissileMotion)>,
) {
    let Some(visuals) = visuals.as_deref() else {
        return;
    };
    let now = time.elapsed_secs();
    // Deviation: before the step, on last frame's progress, so this writes no `Transform`.
    for (entity, mut missile, motion) in &mut missiles {
        let Some(func) = scripts.function(motion.id, &visuals.0) else {
            commands.entity(entity).remove::<MissileMotion>();
            missile.pose = None;
            continue;
        };
        let (pos, launch, impact) = (missile.pos, missile.launch, missile.impact);
        let args = [
            f64::from(missile.progress),
            f64::from(now - motion.fired_at),
            f64::from(motion.index),
            f64::from(motion.count),
            f64::from(launch.distance(pos)),
            f64::from(impact.distance(pos)),
            f64::from(motion.start_distance),
            f64::from(impact.distance(launch)),
            f64::from(motion.rand[0]),
            f64::from(motion.rand[1]),
            f64::from(motion.rand[2]),
            f64::from(missile.spell_id),
        ];
        let Some(out) = call(func, args) else {
            scripts.fail(motion.id);
            commands.entity(entity).remove::<MissileMotion>();
            missile.pose = None;
            continue;
        };
        let base = missile.base_speed;
        if out[8] > 0.0 {
            missile.speed_override = Some(out[8] as f32);
        }
        if out[9] > 0.0 {
            missile.speed_override = Some(out[9] as f32 * base);
        }
        if out[10] != 0.0 {
            missile.speed_override = Some((out[10] as f32 + base).max(0.01));
        }
        let offset = translation(&out, launch, impact);
        missile.off_path = offset.length_squared() > OFF_PATH_SQ;
        missile.pose = Some(MotionPose {
            offset,
            turn: turn(&out),
            scale: if out[11] > 0.0 {
                (out[11] as f32).max(0.01)
            } else {
                1.0
            },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(script: &str) -> MissileMotionRec {
        MissileMotionRec {
            name: "test".into(),
            script: script.into(),
            flags: 0,
            missile_count: 1,
        }
    }

    /// The sandbox has `math` and the degree aliases, and nothing of the base library.
    #[test]
    fn the_sandbox_runs_degree_math_and_hides_the_base_library() {
        let mut scripts = MotionScripts::default();
        let f = scripts
            .compile(
                1,
                &rec("transUp = sin(90) + PI * 0\nif print then transRight = 1 end\nscale = time"),
            )
            .expect("compiles");
        let out = call(
            &f,
            [0.0, 2.5, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 133.0],
        )
        .expect("runs");
        assert!((out[4] - 1.0).abs() < 1e-9, "sin takes degrees");
        assert_eq!(out[2], 0.0, "no `print` in the sandbox");
        assert_eq!(out[11], 2.5, "arguments arrive in order");
    }

    /// A script that errors drops out, and a shipped row's shape runs: Parabola lifts the middle of
    /// the flight by a share of its start distance.
    #[test]
    fn a_failing_script_drops_and_parabola_lifts_the_middle() {
        let mut scripts = MotionScripts::default();
        let bad = scripts.compile(2, &rec("error('x')")).expect("compiles");
        assert!(call(&bad, [0.0; 12]).is_none());
        let parabola = scripts
            .compile(
                13,
                &rec("local maxMagnitude = startDistance * .15\n\
                      transAngle = 0\n\
                      transMag = (progress * 2) - 1\n\
                      transMag = (1 - (transMag * transMag)) * maxMagnitude"),
            )
            .expect("compiles");
        let mut args = [0.0; 12];
        args[0] = 0.5;
        args[6] = 20.0;
        let out = call(&parabola, args).expect("runs");
        assert!((out[1] - 3.0).abs() < 1e-9, "{}", out[1]);
        let up = translation(&out, Vec3::ZERO, Vec3::new(0.0, 0.0, -20.0));
        assert!(
            (up - Vec3::new(0.0, 3.0, 0.0)).length() < 1e-5,
            "angle 0 is straight up: {up}"
        );
    }

    /// `transRight` moves along the heading's left in the reference's frame.
    #[test]
    fn trans_right_moves_along_the_left_of_the_heading() {
        let mut out = [0.0; 12];
        out[2] = 1.0;
        out[3] = 2.0;
        // Heading Bevy −Z, the reference's +X: its left is the reference's +Y, Bevy −X.
        let o = translation(&out, Vec3::ZERO, Vec3::new(0.0, 0.0, -10.0));
        assert!((o - Vec3::new(-1.0, 0.0, -2.0)).length() < 1e-5, "{o}");
    }
}
