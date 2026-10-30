//! The animation system: `AnimationGroup` (methods `0xac1ab8`) and its `Animation`s (`0xac18e0`;
//! `Alpha` `0xac1aa0`, `Translation` `0xac19dc`, `Rotation` `0xac19f0`, `Scale` `0xac1a24`, `Path`
//! `0xac1a48`), owned by a frame or a region. A group plays its animations order by order, each one
//! through its start delay, duration and end delay, and ticks in its owner frame's layer update
//! (`0x490770`) while the frame is shown. The state machine is the reference's; [`lua`] is its Lua
//! face.
//!
//! Alpha and Translation draw: an alpha animation adds to the owner's alpha byte from the value
//! saved when the first alpha group began (`0x48f8e0`, `0x487ce0`), restored each tick
//! (`0x48eba0`) and when the last alpha group ends; a translation offsets the owner's regions'
//! drawn rects, a frame's reaching every region below it (`0x491c60`). Rotation, Scale and Path
//! keep their values and time but draw nothing: no stock 3.3.5a file declares one.

pub(crate) mod lua;
pub(crate) mod run;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::widget::{FrameHandle, RegionHandle};

/// `ANIM_LOOPSTATE_*`.
pub(crate) const NONE: u8 = 0;
pub(crate) const FORWARD: u8 = 1;
pub(crate) const REVERSE: u8 = 2;
/// `ANIM_LOOPTYPE_*`.
const REPEAT: u8 = 1;
const BOUNCE: u8 = 2;

/// Group flags (`+0x88`).
const PENDING_FINISH: u16 = 0x2;
const PAUSED: u16 = 0x4;
const OVERRUN: u16 = 0x8;
const FINISHED: u16 = 0x10;
const STOPPING: u16 = 0x20;

/// What a group belongs to: `GetRegionParent`'s answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Owner {
    Frame(FrameHandle),
    Region(RegionHandle),
}

/// An animation's kind and its own values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Kind {
    Plain,
    /// The alpha step in bytes, `change × 255` truncated (`0x4982e0`).
    Alpha(i16),
    /// The offset in UI units.
    Translation(f32, f32),
    /// Radians, and the origin point with its offset.
    Rotation(f32, OriginPoint),
    /// The x and y scale, and the origin.
    Scale(f32, f32, OriginPoint),
    Path,
}

/// `SetOrigin(point, x, y)`'s values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct OriginPoint {
    pub(crate) point: &'static str,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

impl Default for OriginPoint {
    fn default() -> Self {
        Self {
            point: "CENTER",
            x: 0.0,
            y: 0.0,
        }
    }
}

impl Kind {
    pub(crate) fn type_name(&self) -> &'static str {
        match self {
            Kind::Plain => "Animation",
            Kind::Alpha(_) => "Alpha",
            Kind::Translation(..) => "Translation",
            Kind::Rotation(..) => "Rotation",
            Kind::Scale(..) => "Scale",
            Kind::Path => "Path",
        }
    }

    pub(crate) fn from_name(name: &str) -> Kind {
        match name.to_ascii_lowercase().as_str() {
            "alpha" => Kind::Alpha(0),
            "translation" => Kind::Translation(0.0, 0.0),
            "rotation" => Kind::Rotation(0.0, OriginPoint::default()),
            "scale" => Kind::Scale(1.0, 1.0, OriginPoint::default()),
            "path" => Kind::Path,
            _ => Kind::Plain,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Group {
    pub(crate) owner: Owner,
    pub(crate) name: Option<String>,
    /// Every animation, in link order.
    pub(crate) anims: Vec<u32>,
    /// The animations by order, ascending.
    lists: Vec<Vec<u32>>,
    pub(crate) flags: u16,
    pub(crate) loop_type: u8,
    pub(crate) loop_state: u8,
    current: i32,
    pub(crate) elapsed: f32,
    pub(crate) duration: f32,
    progress: f32,
    pub(crate) initial_offset: (f32, f32),
}

#[derive(Clone, Debug)]
pub(crate) struct Anim {
    pub(crate) group: u32,
    pub(crate) kind: Kind,
    pub(crate) name: Option<String>,
    /// 0-based, -1 unset until the group sorts it.
    pub(crate) order: i32,
    pub(crate) play_state: u8,
    loop_state: u8,
    pub(crate) start_delay: f32,
    pub(crate) end_delay: f32,
    pub(crate) duration: f32,
    pub(crate) max_framerate: f32,
    frame_interval: f32,
    frame_elapsed: f32,
    pub(crate) elapsed: f32,
    pub(crate) progress_with_delay: f32,
    pub(crate) progress: f32,
    pub(crate) smooth_progress: f32,
    /// `(in, out)` weights, `None` before `SetSmoothing`.
    pub(crate) smoothing: Option<(f32, f32)>,
}

/// An owner's playing groups, its animated alpha and its saved alpha.
#[derive(Clone, Debug, Default)]
struct OwnerState {
    groups: Vec<u32>,
    playing: Vec<u32>,
    anim_alpha: bool,
    saved_alpha: u8,
}

/// Every group and animation, by id, and the per-owner lists.
#[derive(Debug, Default)]
pub(crate) struct AnimStore {
    next: u32,
    pub(crate) groups: HashMap<u32, Group>,
    pub(crate) anims: HashMap<u32, Anim>,
    owners: HashMap<Owner, OwnerState>,
    /// The translation each region draws at this tick.
    pub(crate) offsets: HashMap<RegionHandle, (f32, f32)>,
}

impl AnimStore {
    fn id(&mut self) -> u32 {
        self.next += 1;
        self.next
    }

    /// The group constructor (`0x499eb0`), registered on its owner.
    pub(crate) fn new_group(&mut self, owner: Owner, name: Option<String>) -> u32 {
        let id = self.id();
        self.groups.insert(
            id,
            Group {
                owner,
                name,
                anims: Vec::new(),
                lists: Vec::new(),
                flags: 0,
                loop_type: NONE,
                loop_state: NONE,
                current: -1,
                elapsed: 0.0,
                duration: 0.0,
                progress: 0.0,
                initial_offset: (0.0, 0.0),
            },
        );
        self.owners.entry(owner).or_default().groups.push(id);
        id
    }

    /// The animation constructor (`0x499bb0`), linked to its group's tail, not yet in an order
    /// list.
    pub(crate) fn new_anim(&mut self, group: u32, kind: Kind, name: Option<String>) -> u32 {
        let id = self.id();
        self.anims.insert(
            id,
            Anim {
                group,
                kind,
                name,
                order: -1,
                play_state: 0,
                loop_state: NONE,
                start_delay: 0.0,
                end_delay: 0.0,
                duration: 0.0,
                max_framerate: 0.0,
                frame_interval: 0.0,
                frame_elapsed: 0.0,
                elapsed: 0.0,
                progress_with_delay: 0.0,
                progress: 0.0,
                smooth_progress: 0.0,
                smoothing: None,
            },
        );
        if let Some(g) = self.groups.get_mut(&group) {
            g.anims.push(id);
        }
        id
    }

    /// The groups an owner holds, in creation order (`GetAnimationGroups`).
    pub(crate) fn groups_of(&self, owner: Owner) -> Vec<u32> {
        self.owners
            .get(&owner)
            .map(|o| o.groups.clone())
            .unwrap_or_default()
    }

    /// The owners with a group that is playing or paused, for the tick.
    pub(crate) fn active_owners(&self) -> Vec<Owner> {
        self.owners
            .iter()
            .filter(|(_, o)| {
                o.groups
                    .iter()
                    .any(|g| self.groups.get(g).is_some_and(|g| g.loop_state != NONE))
            })
            .map(|(&owner, _)| owner)
            .collect()
    }

    fn group(&self, g: u32) -> Option<&Group> {
        self.groups.get(&g)
    }

    /// `IsDone` (`0x497920`).
    pub(crate) fn is_done(&self, g: u32) -> bool {
        let Some(g) = self.group(g) else { return true };
        if g.duration <= 0.0001 {
            return true;
        }
        if g.loop_type == REPEAT || g.loop_type == BOUNCE {
            return false;
        }
        if g.loop_state == FORWARD || g.loop_state == REVERSE {
            return false;
        }
        g.progress >= 1.0
    }

    /// The highest animation order (`0x498a40`), -1 with none.
    pub(crate) fn max_order(&self, g: u32) -> i32 {
        self.group(g)
            .and_then(|g| g.lists.last())
            .and_then(|l| l.first())
            .and_then(|a| self.anims.get(a))
            .map_or(-1, |a| a.order)
    }

    /// The duration compute (`0x49ac80`): the longest delay-padded animation of each order, summed.
    fn compute_duration(&mut self, g: u32) {
        let Some(group) = self.groups.get(&g) else {
            return;
        };
        let total = group
            .lists
            .iter()
            .map(|l| {
                l.iter()
                    .filter_map(|a| self.anims.get(a))
                    .map(|a| a.duration + a.end_delay + a.start_delay)
                    .fold(0.0f32, f32::max)
            })
            .sum();
        if let Some(group) = self.groups.get_mut(&g) {
            group.duration = total;
        }
    }

    /// The order-list register and node add (`0x499a20`, `0x49a580`): into its order's list, a new
    /// list inserted in order.
    fn add_to_list(&mut self, g: u32, a: u32) {
        let order = self.anims.get(&a).map_or(-1, |a| a.order);
        if order == -1 {
            return;
        }
        let orders: Vec<i32> = self
            .group(g)
            .map(|g| {
                g.lists
                    .iter()
                    .map(|l| {
                        l.first()
                            .and_then(|x| self.anims.get(x))
                            .map_or(i32::MIN, |x| x.order)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let Some(group) = self.groups.get_mut(&g) else {
            return;
        };
        match orders.iter().position(|&o| o >= order) {
            Some(i) if orders[i] == order => group.lists[i].push(a),
            Some(i) => group.lists.insert(i, vec![a]),
            None => group.lists.push(vec![a]),
        }
    }

    /// The animation-added notice (`0x49acf0`).
    pub(crate) fn notify_added(&mut self, g: u32, a: u32) {
        self.add_to_list(g, a);
        self.compute_duration(g);
    }

    /// The order-changed notice (`0x49ad30`): out of its old list into its new one. True when the
    /// old list emptied, which the node removal (`0x49b7b0`) answers with a full re-sort, the
    /// caller's to run since it stops the group.
    pub(crate) fn relist(&mut self, g: u32, a: u32) -> bool {
        let emptied = {
            let Some(group) = self.groups.get_mut(&g) else {
                return false;
            };
            for l in &mut group.lists {
                l.retain(|&x| x != a);
            }
            let before = group.lists.len();
            group.lists.retain(|l| !l.is_empty());
            before != group.lists.len()
        };
        if !emptied {
            self.add_to_list(g, a);
            self.compute_duration(g);
        }
        emptied
    }

    /// [`Self::compute_duration`] for the numeric setters (`0x49ae20`..`0x49aea0`).
    pub(crate) fn compute_duration_pub(&mut self, g: u32) {
        self.compute_duration(g);
    }

    /// The animation sort's list rebuild (`0x49b1c0`), after its stop: unordered animations take
    /// the order after the previous one's.
    pub(crate) fn sort(&mut self, g: u32) {
        let anims = self.group(g).map(|g| g.anims.clone()).unwrap_or_default();
        if let Some(group) = self.groups.get_mut(&g) {
            group.lists.clear();
        }
        let mut next = 0;
        for a in anims {
            let Some(anim) = self.anims.get_mut(&a) else {
                continue;
            };
            if anim.order == -1 {
                anim.order = next.max(0);
                next += 1;
            } else {
                next = anim.order + 1;
            }
            if next > 99 {
                next = 0;
            }
            self.add_to_list(g, a);
        }
        self.compute_duration(g);
    }

    fn list(&self, g: u32) -> Vec<u32> {
        self.group(g)
            .and_then(|gr| {
                usize::try_from(gr.current)
                    .ok()
                    .and_then(|i| gr.lists.get(i))
            })
            .cloned()
            .unwrap_or_default()
    }

    /// `HasAnimOfType(Alpha)` (`0x499720`).
    fn has_alpha(&self, g: u32) -> bool {
        self.group(g).is_some_and(|g| {
            g.anims
                .iter()
                .any(|a| matches!(self.anims.get(a).map(|a| a.kind), Some(Kind::Alpha(_))))
        })
    }
}

/// The S-curve smoother's progress (`0x497ba0`).
fn smooth(weights: (f32, f32), p: f32) -> f32 {
    let (i, o) = (weights.0.abs() >= 0.001, weights.1.abs() >= 0.001);
    match (i, o) {
        (true, true) => 0.5 - (p * std::f32::consts::PI).cos() * 0.5,
        (true, false) => 1.0 - (p * std::f32::consts::FRAC_PI_2).cos(),
        (false, true) => -((p + 1.0) * std::f32::consts::FRAC_PI_2).cos(),
        (false, false) => p,
    }
}

/// The animation's progress update (`0x4985f0`): the time consumed.
fn update_progress(a: &mut Anim, elapsed: f32) -> f32 {
    let prev = a.elapsed;
    a.elapsed += elapsed;
    let total = a.start_delay + a.end_delay + a.duration;
    let at = if total > a.elapsed {
        a.progress_with_delay = (a.elapsed / total).min(1.0);
        if a.duration.abs() < 0.000_000_24 {
            a.progress = 1.0;
        } else {
            a.frame_elapsed += elapsed;
            if a.frame_elapsed >= a.frame_interval {
                let delay = if a.loop_state == REVERSE {
                    a.end_delay
                } else {
                    a.start_delay
                };
                a.progress = ((a.elapsed - delay) / a.duration).clamp(0.0, 1.0);
                a.frame_elapsed = 0.0;
            }
        }
        a.elapsed
    } else {
        a.progress_with_delay = 1.0;
        a.progress = 1.0;
        total
    };
    let p = if a.loop_state == REVERSE {
        1.0 - a.progress
    } else {
        a.progress
    };
    a.smooth_progress = a.smoothing.map_or(p, |w| smooth(w, p));
    at - prev
}

/// `IsDelaying` (`0x497e90`).
pub(crate) fn is_delaying(a: &Anim) -> bool {
    if a.progress_with_delay <= 0.0 || a.progress_with_delay >= 1.0 {
        return false;
    }
    let total = a.end_delay + a.duration + a.start_delay;
    if total.abs() < 0.000_000_24 {
        return false;
    }
    let (lead, run) = if a.loop_state == REVERSE {
        (a.end_delay, a.end_delay + a.duration)
    } else {
        (a.start_delay, a.start_delay + a.duration)
    };
    if lead > a.elapsed {
        return true;
    }
    if run > a.elapsed {
        return false;
    }
    total > a.elapsed
}

/// `SetMaxFramerate` (`0x497680`).
pub(crate) fn set_max_framerate(a: &mut Anim, fps: f32) {
    if fps > 0.0001 {
        a.max_framerate = fps;
        a.frame_interval = 1.0 / fps;
    } else {
        a.max_framerate = 0.0;
        a.frame_interval = 0.0;
    }
}

/// `SetSmoothing` (`0x497f30`): only a weight in `(0, 1]` creates or sets the smoother.
pub(crate) fn set_smoothing(a: &mut Anim, w: (f32, f32)) {
    if (w.0 > 0.0 && w.0 <= 1.0) || (w.1 > 0.0 && w.1 <= 1.0) {
        a.smoothing = Some((w.0.clamp(0.0, 1.0), w.1.clamp(0.0, 1.0)));
    }
}

/// The smoothing weights table (`0x815240`, `0x815590`).
pub(crate) const SMOOTHING: [(&str, f32, f32); 5] = [
    ("NONE", 0.0, 0.0),
    ("IN", 1.0, 0.0),
    ("OUT", 0.0, 1.0),
    ("IN_OUT", 1.0, 1.0),
    ("OUT_IN", 1.0, 1.0),
];

pub(crate) const LOOP_TYPES: [&str; 3] = ["NONE", "REPEAT", "BOUNCE"];
pub(crate) const LOOP_STATES: [&str; 3] = ["NONE", "FORWARD", "REVERSE"];
