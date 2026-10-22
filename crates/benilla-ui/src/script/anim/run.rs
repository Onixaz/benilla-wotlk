//! The animation state machine's moves, each the reference's method: state changes under a short
//! model borrow, with the scripts fired between them so a handler sees the state as it stands.

use mlua::{Lua, Value};

use super::lua::fire;
use super::{
    is_delaying, update_progress, AnimStore, Kind, Owner, BOUNCE, FINISHED, FORWARD, LOOP_STATES,
    NONE, OVERRUN, PAUSED, PENDING_FINISH, REVERSE, STOPPING,
};
use crate::script::Model;

fn with<R>(lua: &Lua, f: impl FnOnce(&mut Model) -> R) -> R {
    let mut model = crate::script::model_mut_of(lua);
    f(&mut model)
}

fn with_store<R>(lua: &Lua, f: impl FnOnce(&mut AnimStore) -> R) -> R {
    with(lua, |m| f(&mut m.anims))
}

/// `CSimpleAnimGroup::Play` (`0x49a8f0`).
pub(crate) fn group_play(lua: &Lua, g: u32) -> bool {
    let start = with_store(lua, |s| {
        let group = s.groups.get_mut(&g)?;
        if group.anims.is_empty() {
            return Some(None);
        }
        let mut notify = true;
        if group.flags & FINISHED != 0 {
            group.elapsed = 0.0;
            group.flags = 0;
            group.progress = 0.0;
            group.loop_state = NONE;
            group.current = -1;
            notify = false;
        }
        if group.loop_state != NONE && group.flags & PAUSED == 0 {
            return Some(Some((Vec::new(), false, true)));
        }
        group.flags &= !PAUSED;
        let count = group.lists.len() as i32;
        if count == 0 {
            return Some(Some((Vec::new(), false, false)));
        }
        let reset = if group.current < 0 || group.current >= count {
            if group.current < 0 {
                group.current = 0;
                group.loop_state = FORWARD;
            } else {
                group.current = count - 1;
                group.loop_state = REVERSE;
            }
            group.anims.clone()
        } else {
            Vec::new()
        };
        let state = group.loop_state;
        let mut list = group.lists[group.current as usize].clone();
        if state == REVERSE {
            list.reverse();
        }
        for a in reset {
            if let Some(a) = s.anims.get_mut(&a) {
                a.loop_state = state;
                a.elapsed = 0.0;
                a.play_state = 0;
                a.progress_with_delay = 0.0;
                a.progress = 0.0;
            }
        }
        Some(Some((list, notify, false)))
    });
    let Some(start) = start else { return false };
    let Some((list, notify, already)) = start else {
        return false;
    };
    if already {
        return true;
    }
    let played = !list.is_empty();
    for a in list {
        anim_play(lua, a);
    }
    if notify && played {
        notify_begin(lua, g);
    }
    fire(lua, g, "OnPlay", Vec::new());
    true
}

/// `CSimpleAnim::Play` (`0x49ad80`).
pub(crate) fn anim_play(lua: &Lua, a: u32) -> bool {
    let Some(g) = with_store(lua, |s| s.anims.get(&a).map(|a| a.group)) else {
        return false;
    };
    if !group_play(lua, g) {
        return false;
    }
    let started = with_store(lua, |s| {
        let a = s.anims.get_mut(&a)?;
        if a.play_state == 1 {
            return Some(false);
        }
        a.play_state = 1;
        if a.loop_state == NONE {
            a.loop_state = FORWARD;
        }
        Some(true)
    });
    if started == Some(true) {
        fire(lua, a, "OnPlay", Vec::new());
    }
    true
}

/// `CSimpleAnimGroup::Pause` (`0x49aab0`).
pub(crate) fn group_pause(lua: &Lua, g: u32) {
    let list = with_store(lua, |s| {
        let list = s.list(g);
        let group = s.groups.get_mut(&g)?;
        if group.loop_state == NONE || group.flags & PAUSED != 0 {
            return None;
        }
        group.flags = (group.flags & !0xc) | PAUSED;
        Some(list)
    });
    let Some(list) = list else { return };
    for a in list {
        anim_pause(lua, a);
    }
    fire(lua, g, "OnPause", Vec::new());
}

/// `CSimpleAnim::Pause` (`0x497e60`).
pub(crate) fn anim_pause(lua: &Lua, a: u32) {
    let Some(g) = with_store(lua, |s| s.anims.get(&a).map(|a| a.group)) else {
        return;
    };
    group_pause(lua, g);
    let paused = with_store(lua, |s| {
        let a = s.anims.get_mut(&a)?;
        (a.play_state == 1).then(|| a.play_state = 2)
    });
    if paused.is_some() {
        fire(lua, a, "OnPause", Vec::new());
    }
}

/// The reset a stop and a finish end with.
fn reset_group(s: &mut AnimStore, g: u32) {
    if let Some(group) = s.groups.get_mut(&g) {
        group.elapsed = 0.0;
        group.progress = 0.0;
        group.flags = 0;
        group.loop_state = NONE;
        group.current = -1;
    }
}

/// `CSimpleAnimGroup::Stop` (`0x49b0f0`).
pub(crate) fn group_stop(lua: &Lua, g: u32, requested: bool) {
    let list = with_store(lua, |s| {
        let list = s.list(g);
        let group = s.groups.get_mut(&g)?;
        if group.loop_state == NONE || group.flags & STOPPING != 0 {
            return None;
        }
        group.flags = (group.flags & !0x28) | STOPPING;
        Some(list)
    });
    let Some(list) = list else { return };
    for a in list {
        anim_stop(lua, a, requested);
    }
    fire(lua, g, "OnStop", vec![Value::Boolean(requested)]);
    notify_end(lua, g);
    with_store(lua, |s| reset_group(s, g));
}

/// `CSimpleAnim::Stop` (`0x49adc0`).
pub(crate) fn anim_stop(lua: &Lua, a: u32, requested: bool) {
    let g = with_store(lua, |s| {
        let anim = s.anims.get_mut(&a)?;
        if anim.play_state == 0 {
            return None;
        }
        anim.elapsed = 0.0;
        anim.progress_with_delay = 0.0;
        anim.progress = 0.0;
        anim.loop_state = NONE;
        anim.play_state = 0;
        Some(anim.group)
    });
    let Some(g) = g else { return };
    fire(lua, a, "OnStop", vec![Value::Boolean(requested)]);
    notify_anim_stopped(lua, g, a);
}

/// `CSimpleAnimGroup::NotifyAnimStopped` (`0x49ab60`): the last animation stopping stops the
/// group, unrequested.
fn notify_anim_stopped(lua: &Lua, g: u32, a: u32) {
    let others = with_store(lua, |s| {
        let list = s.list(g);
        let group = s.groups.get(&g)?;
        if group.loop_state == NONE || group.flags & STOPPING != 0 {
            return None;
        }
        let busy = group
            .anims
            .iter()
            .any(|&x| x != a && s.anims.get(&x).is_some_and(|x| x.play_state != 0));
        if busy {
            return None;
        }
        let group = s.groups.get_mut(&g)?;
        group.flags = (group.flags & !0x28) | STOPPING;
        Some(list)
    });
    let Some(others) = others else { return };
    for x in others.into_iter().filter(|&x| x != a) {
        anim_stop(lua, x, false);
    }
    fire(lua, g, "OnStop", vec![Value::Boolean(false)]);
    notify_end(lua, g);
    with_store(lua, |s| reset_group(s, g));
}

/// `CSimpleAnimGroup::Finish` (`0x498580`): end at the current pass's end.
pub(crate) fn group_finish(lua: &Lua, g: u32) {
    with_store(lua, |s| {
        let done = s.is_done(g);
        if let Some(group) = s.groups.get_mut(&g) {
            if !done && group.loop_state != NONE {
                group.flags |= PENDING_FINISH;
            }
        }
    });
}

/// The region chain a playing group affects: the owner, then its frame and the frame's parents
/// (`CScriptRegion::m_parent`).
fn chain(model: &Model, owner: Owner) -> Vec<Owner> {
    let mut out = vec![owner];
    let mut frame = match owner {
        Owner::Frame(h) => model.arena.frame(h).and_then(|f| f.parent),
        Owner::Region(r) => model.arena.region(r).map(|r| r.owner),
    };
    while let Some(h) = frame {
        out.push(Owner::Frame(h));
        frame = model.arena.frame(h).and_then(|f| f.parent);
    }
    out
}

/// `HasActiveAnimGroup` (`0x488740`) and `HasActiveAnimOfType(Alpha)` (`0x488770`).
fn active(model: &Model, owner: Owner, alpha_only: bool) -> bool {
    chain(model, owner).into_iter().any(|o| {
        model.anims.owners.get(&o).is_some_and(|st| {
            st.playing
                .iter()
                .any(|&g| !alpha_only || model.anims.has_alpha(g))
        })
    })
}

/// The owner's alpha byte: a frame's own alpha, a region's.
fn alpha_byte(model: &Model, owner: Owner) -> u8 {
    let a = match owner {
        Owner::Frame(h) => model.arena.frame(h).map_or(1.0, |f| f.alpha),
        Owner::Region(r) => model
            .region_data
            .get(&r)
            .and_then(|d| d.alpha)
            .unwrap_or(1.0),
    };
    (a.clamp(0.0, 1.0) * 255.0) as u8
}

fn set_alpha_byte(model: &mut Model, owner: Owner, byte: u8) {
    if alpha_byte(model, owner) == byte {
        return;
    }
    let a = f32::from(byte) / 255.0;
    match owner {
        Owner::Frame(h) => model.arena.set_alpha(h, a),
        Owner::Region(r) => model.region_data.entry(r).or_default().alpha = Some(a),
    }
}

/// `NotifyAnimBegin` (`0x4889c0`) → `AnimActivated`: the first alpha group saves the owner's alpha.
fn notify_begin(lua: &Lua, g: u32) {
    with(lua, |m| {
        let Some(owner) = m.anims.groups.get(&g).map(|g| g.owner) else {
            return;
        };
        let has_alpha = m.anims.has_alpha(g);
        let saved = alpha_byte(m, owner);
        let st = m.anims.owners.entry(owner).or_default();
        if !st.anim_alpha && has_alpha {
            st.anim_alpha = true;
            st.saved_alpha = saved;
        }
        if !st.playing.contains(&g) {
            st.playing.push(g);
        }
    });
}

/// `NotifyAnimEnd` (`0x488980`) → `AnimDeactivated`: with no alpha group left playing, the saved
/// alpha comes back.
fn notify_end(lua: &Lua, g: u32) {
    with(lua, |m| {
        let Some(owner) = m.anims.groups.get(&g).map(|g| g.owner) else {
            return;
        };
        if let Some(st) = m.anims.owners.get_mut(&owner) {
            st.playing.retain(|&x| x != g);
        }
        let anim_alpha = m.anims.owners.get(&owner).is_some_and(|s| s.anim_alpha);
        if anim_alpha {
            let still = active(m, owner, true);
            let saved = m.anims.owners.get(&owner).map_or(255, |s| s.saved_alpha);
            if let Some(st) = m.anims.owners.get_mut(&owner) {
                st.anim_alpha = still;
            }
            if !still {
                set_alpha_byte(m, owner, saved);
            }
        }
    });
}

/// `AddAnimAlpha` (`0x48f8e0` frame, `0x487ce0` region): the step on the current byte, clamped.
fn add_alpha(model: &mut Model, owner: Owner, delta: i16) {
    let v = (i32::from(delta) + i32::from(alpha_byte(model, owner))).clamp(0, 255) as u8;
    set_alpha_byte(model, owner, v);
}

/// `AddAnimTranslation`: a region's own rect, a frame's every region below it (`0x491c60`).
fn add_translation(model: &mut Model, owner: Owner, d: (f32, f32)) {
    if d == (0.0, 0.0) {
        return;
    }
    let mut regions = Vec::new();
    match owner {
        Owner::Region(r) => regions.push(r),
        Owner::Frame(h) => {
            let mut stack = vec![h];
            while let Some(f) = stack.pop() {
                if let Some(frame) = model.arena.frame(f) {
                    regions.extend(frame.regions.iter().copied());
                    stack.extend(frame.children.iter().copied());
                }
            }
        }
    }
    for r in regions {
        let o = model.anims.offsets.entry(r).or_insert((0.0, 0.0));
        o.0 += d.0;
        o.1 += d.1;
    }
}

/// `Apply(progress)`: Alpha (`0x498330`) and Translation (`0x498040`) draw; the rest do nothing.
fn apply(lua: &Lua, a: u32, progress: f32) {
    with(lua, |m| {
        let Some(anim) = m.anims.anims.get(&a) else {
            return;
        };
        let Some(owner) = m.anims.groups.get(&anim.group).map(|g| g.owner) else {
            return;
        };
        match anim.kind {
            Kind::Alpha(change) => add_alpha(m, owner, (f32::from(change) * progress) as i16),
            Kind::Translation(x, y) => add_translation(m, owner, (x * progress, y * progress)),
            _ => {}
        }
    });
}

/// `CSimpleAnim::OnUpdate` (`0x498d50`): done, and the time consumed.
fn anim_on_update(lua: &Lua, a: u32, elapsed: f32) -> (bool, f32) {
    let step = with_store(lua, |s| {
        let anim = s.anims.get_mut(&a)?;
        if anim.progress_with_delay >= 1.0 {
            return Some((Some(anim.smooth_progress), true, 0.0, false));
        }
        match anim.play_state {
            0 => Some((None, true, 0.0, false)),
            2 => Some((Some(anim.smooth_progress), false, 0.0, false)),
            _ => {
                let consumed = update_progress(anim, elapsed);
                Some((None, false, consumed, true))
            }
        }
    });
    let Some((applied, done, consumed, ran)) = step else {
        return (true, 0.0);
    };
    if !ran {
        if let Some(p) = applied {
            apply(lua, a, p);
        }
        return (done, consumed);
    }
    fire(lua, a, "OnUpdate", vec![Value::Number(f64::from(elapsed))]);
    let (smooth, pwd) = with_store(lua, |s| {
        s.anims
            .get(&a)
            .map_or((0.0, 1.0), |x| (x.smooth_progress, x.progress_with_delay))
    });
    apply(lua, a, smooth);
    if pwd < 1.0 {
        return (false, consumed);
    }
    fire(lua, a, "OnFinished", Vec::new());
    (true, consumed)
}

/// `AdvanceLoopState` (`0x49b470`): the next order, or the end of a pass: finish, or loop.
fn advance_loop_state(lua: &Lua, g: u32, elapsed: f32, remaining: f32) {
    enum Next {
        Play(Vec<u32>),
        Finish(bool),
        Loop(Vec<u32>, u8, bool),
    }
    let next = with_store(lua, |s| {
        let group = s.groups.get_mut(&g)?;
        let count = group.lists.len() as i32;
        let reverse = group.loop_state == REVERSE;
        let (step, first, end) = if reverse {
            (-1, count - 1, -1)
        } else {
            (1, 0, count)
        };
        group.current += step;
        if group.current != end {
            let list = group
                .lists
                .get(group.current as usize)
                .cloned()
                .unwrap_or_default();
            return Some(Next::Play(list));
        }
        let finishing = group.flags & PENDING_FINISH != 0;
        if group.loop_type == NONE || finishing {
            group.flags |= FINISHED;
            return Some(Next::Finish(finishing));
        }
        if !reverse && remaining > 0.0001 {
            group.flags |= OVERRUN;
        }
        if group.loop_type == BOUNCE {
            group.loop_state = if reverse { FORWARD } else { REVERSE };
            group.current = end - step;
        } else {
            group.current = first;
        }
        group.elapsed = 0.0;
        group.progress = 0.0;
        let state = group.loop_state;
        let current = group.current;
        let lists = group.lists.clone();
        let overrun = group.flags & OVERRUN != 0;
        for (i, l) in lists.iter().enumerate() {
            for a in l {
                if let Some(a) = s.anims.get_mut(a) {
                    a.loop_state = state;
                    a.elapsed = 0.0;
                    a.progress_with_delay = 0.0;
                    a.play_state = 0;
                    a.progress = 0.0;
                }
            }
            let _ = i;
        }
        let play = lists.get(current as usize).cloned().unwrap_or_default();
        Some(Next::Loop(play, state, overrun))
    });
    match next {
        None => {}
        Some(Next::Play(list)) => {
            for a in list {
                anim_play(lua, a);
            }
        }
        Some(Next::Finish(finishing)) => {
            fire(lua, g, "OnUpdate", vec![Value::Number(f64::from(elapsed))]);
            fire(lua, g, "OnFinished", vec![Value::Boolean(finishing)]);
            // A handler that replayed the group cleared the flag (`0x49b6fd`).
            let still = with_store(lua, |s| {
                s.groups.get(&g).is_some_and(|x| x.flags & FINISHED != 0)
            });
            if still {
                notify_end(lua, g);
                with_store(lua, |s| reset_group(s, g));
            }
        }
        Some(Next::Loop(play, state, overrun)) => {
            for a in &play {
                anim_play(lua, *a);
            }
            let name = LOOP_STATES[usize::from(state).min(2)];
            fire(
                lua,
                g,
                "OnLoop",
                vec![Value::String(lua.create_string(name).expect("lua string"))],
            );
            if overrun {
                for a in play {
                    apply(lua, a, -1.0);
                }
                with_store(lua, |s| {
                    if let Some(x) = s.groups.get_mut(&g) {
                        x.flags &= !OVERRUN;
                    }
                });
            }
        }
    }
}

/// The finished orders before the current one, drawn at their end, and the initial offset.
fn apply_done_orders(lua: &Lua, g: u32, reverse: bool) {
    let (done, owner, offset, current) = with_store(lua, |s| {
        s.groups
            .get(&g)
            .map_or((Vec::new(), None, (0.0, 0.0), -1), |x| {
                let n = usize::try_from(x.current).unwrap_or(0).min(x.lists.len());
                let mut done: Vec<u32> = Vec::new();
                for l in &x.lists[..n] {
                    if reverse {
                        done.extend(l.iter().rev());
                    } else {
                        done.extend(l.iter());
                    }
                }
                (done, Some(x.owner), x.initial_offset, x.current)
            })
    });
    for a in done {
        apply(lua, a, 1.0);
    }
    if current >= 0 {
        if let Some(owner) = owner {
            with(lua, |m| add_translation(m, owner, offset));
        }
    }
}

/// `DoGroupUpdateWork` (`0x49c350`).
fn do_group_update_work(lua: &Lua, g: u32, elapsed: f32) {
    let Some((reverse, paused)) = with_store(lua, |s| {
        s.groups
            .get(&g)
            .map(|x| (x.loop_state == REVERSE, x.flags & PAUSED != 0))
    }) else {
        return;
    };
    if !reverse {
        apply_done_orders(lua, g, false);
    }
    if paused {
        let list = with_store(lua, |s| s.list(g));
        for a in list {
            let p = with_store(lua, |s| s.anims.get(&a).map_or(0.0, |x| x.smooth_progress));
            apply(lua, a, p);
        }
        let (owner, offset) = with_store(lua, |s| {
            s.groups
                .get(&g)
                .map_or((None, (0.0, 0.0)), |x| (Some(x.owner), x.initial_offset))
        });
        if let Some(owner) = owner {
            with(lua, |m| add_translation(m, owner, offset));
        }
    } else {
        let mut remaining = elapsed.clamp(0.0, 60.0);
        // Each pass consumes time or ends an order; the bound only guards a malformed group.
        for _ in 0..1000 {
            let list = with_store(lua, |s| {
                let mut list = s.list(g);
                let group = s.groups.get_mut(&g)?;
                group.elapsed += remaining;
                group.progress = if group.elapsed < group.duration {
                    group.elapsed / group.duration
                } else {
                    1.0
                };
                if group.loop_state == REVERSE {
                    list.reverse();
                }
                Some(list)
            });
            let Some(list) = list else { return };
            let mut max_consumed = 0.0f32;
            let mut finished = true;
            let mut stopped = false;
            for a in list {
                let (done, consumed) = anim_on_update(lua, a, remaining);
                if with_store(lua, |s| {
                    s.groups.get(&g).is_none_or(|x| x.loop_state == NONE)
                }) {
                    stopped = true;
                    break;
                }
                max_consumed = max_consumed.max(consumed);
                finished &= done;
            }
            if stopped {
                break;
            }
            remaining -= max_consumed;
            if finished {
                advance_loop_state(lua, g, elapsed, remaining);
            }
            let stop = with_store(lua, |s| {
                s.is_done(g)
                    || s.groups
                        .get(&g)
                        .is_none_or(|x| x.loop_state == NONE || x.flags & PAUSED != 0)
            });
            if remaining <= 0.0001 || stop {
                break;
            }
        }
    }
    let playing = with_store(lua, |s| {
        s.groups.get(&g).is_some_and(|x| x.loop_state != NONE)
    });
    if reverse && playing {
        apply_done_orders(lua, g, true);
    }
}

/// `CSimpleAnimGroup::OnUpdate` (`0x49c760`).
fn group_on_update(lua: &Lua, g: u32, elapsed: f32) {
    let idle = with_store(lua, |s| {
        s.is_done(g) || s.groups.get(&g).is_none_or(|x| x.loop_state == NONE)
    });
    if idle {
        return;
    }
    do_group_update_work(lua, g, elapsed);
    if with_store(lua, |s| s.is_done(g)) {
        return;
    }
    fire(lua, g, "OnUpdate", vec![Value::Number(f64::from(elapsed))]);
}

/// `StopAnimating` (`0x4888f0` region, `0x4919f0` frame): every playing group on the owner and,
/// for a frame, below it, stopped as requested.
pub(crate) fn stop_animating(lua: &Lua, owner: Owner) {
    let groups = with(lua, |m| {
        let mut owners = vec![owner];
        if let Owner::Frame(h) = owner {
            let mut stack = vec![h];
            while let Some(f) = stack.pop() {
                if let Some(frame) = m.arena.frame(f) {
                    owners.extend(frame.regions.iter().map(|&r| Owner::Region(r)));
                    owners.extend(frame.children.iter().map(|&c| Owner::Frame(c)));
                    stack.extend(frame.children.iter().copied());
                }
            }
        }
        owners
            .into_iter()
            .flat_map(|o| {
                m.anims
                    .owners
                    .get(&o)
                    .map(|s| s.playing.clone())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
    });
    for g in groups {
        group_stop(lua, g, true);
    }
}

/// The animation pass of every shown owner's layer update (`0x490770`): each owner's alpha restored
/// to its saved byte (`PreOnAnimUpdate` `0x48eba0`), then its groups ticked. Translations rebuild
/// from zero each pass.
pub(crate) fn tick(lua: &Lua, elapsed: f32) {
    // Nothing playing and no offset to clear: the pass would write nothing, so it takes no
    // mutable borrow, which would move the mutation count.
    {
        let m = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model app_data");
        if m.anims.offsets.is_empty() && m.anims.active_owners().is_empty() {
            return;
        }
    }
    let owners = with(lua, |m| {
        m.anims.offsets.clear();
        let mut owners: Vec<((u32, u32), Owner)> = m
            .anims
            .active_owners()
            .into_iter()
            .filter_map(|o| {
                let (frame, sub) = match o {
                    Owner::Frame(h) => (h, 0),
                    Owner::Region(r) => {
                        let reg = m.arena.region(r)?;
                        (reg.owner, reg.decl_seq + 1)
                    }
                };
                m.arena
                    .frame(frame)
                    .is_some_and(|f| f.effective_visible)
                    .then(|| ((m.frame_id(frame), sub), o))
            })
            .collect();
        owners.sort_by_key(|(k, _)| *k);
        owners
    });
    for (_, owner) in owners {
        with(lua, |m| {
            let restore = m
                .anims
                .owners
                .get(&owner)
                .filter(|s| s.anim_alpha)
                .map(|s| s.saved_alpha);
            if let Some(saved) = restore {
                set_alpha_byte(m, owner, saved);
            }
        });
        let groups = with_store(lua, |s| s.groups_of(owner));
        for g in groups {
            group_on_update(lua, g, elapsed);
        }
    }
}

/// `IsDelaying` for the binding.
pub(crate) fn delaying(lua: &Lua, a: u32) -> bool {
    with_store(lua, |s| s.anims.get(&a).is_some_and(is_delaying))
}
