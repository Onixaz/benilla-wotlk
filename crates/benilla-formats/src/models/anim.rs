//! M2 skeleton and animation parsing: the rest skeleton and each sequence's per-bone keyframes.

use std::collections::HashMap;

use benilla_m2::{M2Model, M2Track};

use crate::BoneSpin;

/// A rest-skeleton bone, raw WoW model space. An M2 has no inverse bind matrices: the rest pose
/// is identity and the pivot is the bind position.
#[derive(Debug, Clone, Copy)]
pub struct SkeletonBone {
    pub parent: i16,
    pub pivot: [f32; 3],
    /// `KeyBoneID` (`-1` none; 0/1 left/right arm, 2/3 left/right shoulder, …): how the per-arm
    /// masks find roots.
    pub key_bone: i16,
    /// The billboard arm (flags `0x08/0x10/0x20/0x40`); children inherit it.
    pub billboard: Option<crate::BillboardKind>,
    /// Bone flags `0x1/0x2/0x4`: how the parent matrix is rebuilt ([`crate::ParentArm`]).
    pub parent_arm: Option<crate::ParentArm>,
}

/// A model's bones in M2 file order, which a vertex's `joints` index.
#[derive(Debug, Clone, Default)]
pub struct Skeleton {
    pub bones: Vec<SkeletonBone>,
}

impl Skeleton {
    /// The nearest bone at or above `bone` with a billboard arm. Its rows become the camera basis
    /// and children multiply onto it, so every descendant is camera-dependent, emitter positions
    /// too; a nearer host discards what an outer one wrote.
    pub fn billboard_host(&self, bone: u16) -> Option<usize> {
        let mut i = usize::from(bone);
        // Bounded by the bone count: the guard against a malformed parent cycle.
        for _ in 0..=self.bones.len() {
            let b = self.bones.get(i)?;
            if b.billboard.is_some() {
                return Some(i);
            }
            i = usize::try_from(b.parent).ok()?;
        }
        None
    }
}

/// The bone hierarchy: parents and pivots.
pub fn parse_m2_skeleton(model: &M2Model) -> Skeleton {
    let bones = model
        .bones
        .iter()
        .map(|b| SkeletonBone {
            parent: b.parent,
            pivot: [b.pivot.x, b.pivot.y, b.pivot.z],
            key_bone: b.key_bone,
            billboard: crate::BillboardKind::from_bone_flags(b.flags.bits()),
            parent_arm: crate::ParentArm::from_bone_flags(b.flags.bits()),
        })
        .collect();
    Skeleton { bones }
}

/// One attachment point (`0` shield, `1`/`2` right/left hand), position in raw WoW model space.
#[derive(Debug, Clone, Copy)]
pub struct M2Attachment {
    pub id: u16,
    pub bone: u16,
    pub position: [f32; 3],
}

/// One attachment point per id the AttachLookup resolves, in id order: of several records under
/// one id, only the one the lookup names is reachable.
pub fn parse_m2_attachments(model: &M2Model) -> Vec<M2Attachment> {
    (0..model.attach_lookup.len())
        .filter_map(|id| {
            let a = model.attachment(id as u16)?;
            Some(M2Attachment {
                // The lookup's id: the reference never re-reads the record's own id field.
                id: id as u16,
                bone: a.bone,
                position: a.position,
            })
        })
        .collect()
}

/// An event record's position half, looked up by 4CC, first match: the cast-release points
/// `$CSL`/`$CSR`/`$CST` and the ranged release `$BWR`.
#[derive(Debug, Clone, Copy)]
pub struct EventMarker {
    /// The identifier 4CC, stored forward (`*b"$CSL"`).
    pub ident: [u8; 4],
    pub bone: u16,
    pub position: [f32; 3],
}

/// The event table's positional markers, in file order since lookups take the first match.
pub fn parse_m2_event_markers(model: &M2Model) -> Vec<EventMarker> {
    model
        .event_markers
        .iter()
        .map(|m| EventMarker {
            ident: m.ident,
            bone: m.bone,
            position: m.position,
        })
        .collect()
}

/// A bow's `$WTT`/`$WTB` event records, `(bone, position)`, which the string drawer spans top to
/// bottom, each posed by its limb-tip bone.
#[derive(Clone, Copy)]
pub struct StringAnchors {
    pub top: (u16, [f32; 3]),
    pub bottom: (u16, [f32; 3]),
}

/// The first event record named `ident`, `(bone, position)`.
fn first_event(model: &M2Model, ident: &[u8; 4]) -> Option<(u16, [f32; 3])> {
    model
        .events
        .iter()
        .find(|e| &e.ident == ident)
        .map(|e| (e.bone, e.position))
}

/// The `$WTT`/`$WTB` anchors, the last record of each; `None` unless both exist.
pub fn parse_m2_string_anchors(model: &M2Model) -> Option<StringAnchors> {
    let last = |ident: &[u8; 4]| {
        model
            .events
            .iter()
            .rfind(|e| &e.ident == ident)
            .map(|e| (e.bone, e.position))
    };
    Some(StringAnchors {
        top: last(b"$WTT")?,
        bottom: last(b"$WTB")?,
    })
}

/// The first `$CCH` marker, `(bone, position)`: the fishing line's near end. The line reads only
/// the pole's, though the bobber authors one; among weapons only the fishing pole does, so a held
/// item keys on its presence.
pub fn parse_m2_cch_marker(model: &M2Model) -> Option<(u16, [f32; 3])> {
    first_event(model, b"$CCH")
}

/// A substitute for a requested animation id and its direction code, the row of a 1.12.1 model's
/// PlayableAnimationLookup. A 3.3.5a model carries no such table, so a model's rows are empty.
#[derive(Debug, Clone, Copy)]
pub struct PlayableAnim {
    /// The `AnimationData.dbc` id this model plays for the row's requested id.
    pub resolved_id: u16,
    /// Direction/variant playback code; carried, not applied.
    pub dir_flags: u16,
}

/// The AnimationLookup (header `0x24`): `AnimationData.dbc` id to first sequence slot, `0xffff`
/// for none, as the ownership test reads it (`M2Model::owns_animation`).
pub fn parse_m2_animation_lookup(model: &M2Model) -> Vec<u16> {
    model.animation_lookup.clone()
}

/// The bones that spin rigidly ([`BoneSpin`]) in anim 0, which the load arms play. How a WMO
/// skybox is armed is untraced; both shipped ones have one clip.
pub fn m2_bone_spins(model: &M2Model) -> HashMap<u16, BoneSpin> {
    let mut out = HashMap::new();
    let skeleton = parse_m2_skeleton(model);
    let anims = parse_m2_animations(model);
    let Some(seq) = anims.iter().find(|a| a.anim_id == 0) else {
        return out;
    };
    for keys in &seq.bones {
        let idx = keys.bone as usize;
        // Rotation only and at least two keys: one key is a pose, not a spin.
        if keys.rotation.len() < 2 || !keys.translation.is_empty() || !keys.scale.is_empty() {
            continue;
        }
        // Parentless: nothing above composes another motion in.
        let Some(bone) = skeleton.bones.get(idx).filter(|b| b.parent < 0) else {
            continue;
        };
        out.insert(
            keys.bone,
            BoneSpin {
                pivot: bone.pivot,
                duration: seq.duration,
                interp: model.bones.get(idx).is_some_and(|b| b.rotation.interp != 0),
                keys: keys.rotation.clone(),
            },
        );
    }
    out
}

/// One bone's keys for a sequence, raw WoW model space, seconds from its start, rotations
/// `[x, y, z, w]`; a bone absent from [`ModelAnimation::bones`] holds its bind pose.
#[derive(Debug, Clone)]
pub struct BoneKeys {
    pub bone: u16,
    pub translation: Vec<(f32, [f32; 3])>,
    pub rotation: Vec<(f32, [f32; 4])>,
    pub scale: Vec<(f32, [f32; 3])>,
}

/// A bone channel on a global sequence, sampled at the scene clock less the instance's attach
/// time, mod `period_ms`. Keys are absolute ms, ascending.
#[derive(Debug, Clone)]
pub struct GlobalSeqChannel<T> {
    pub period_ms: u32,
    pub keys: Vec<(u32, T)>,
}

/// A bone's global-sequence channels, composed over the sequence pose at runtime.
#[derive(Debug, Clone)]
pub struct GlobalSeqBone {
    pub bone: u16,
    pub translation: Option<GlobalSeqChannel<[f32; 3]>>,
    pub rotation: Option<GlobalSeqChannel<[f32; 4]>>,
    pub scale: Option<GlobalSeqChannel<[f32; 3]>>,
}

/// One animation event key: a 4CC-tagged trigger on the sequence timeline.
#[derive(Debug, Clone, Copy)]
pub struct AnimEvent {
    /// Seconds from the sequence start, as the bone keys.
    pub time: f32,
    /// The identifier 4CC, stored forward (`*b"$FL0"`).
    pub ident: [u8; 4],
    /// The payload: a SoundEntries id for `$SND`/`$DSL`/`$DSO`, else 0.
    pub data: u32,
    /// The record's bone, carried on the key because a 4CC repeats: every player model authors
    /// `$CSD` six times, so a lookup by 4CC finds the wrong point.
    pub bone: u16,
    /// The record's point; the event kernel snapshots `placement · boneMatrix[bone] · position`
    /// into the callback record.
    pub position: [f32; 3],
}

/// One sequence's per-bone keyframes. Stand is `anim_id` 0 but not always record 0, so consumers
/// select by `anim_id`.
#[derive(Debug, Clone)]
pub struct ModelAnimation {
    /// `AnimationData.dbc` id, the selection key (0 Stand, 4 Walk, 5 Run, 13 WalkBackwards, …).
    pub anim_id: u16,
    /// The file slot, which indexes every track's key ranges; zero-length sequences are dropped.
    pub seq_index: usize,
    /// The sequence's band on the model's timeline, ms, which its tracks are cut to.
    pub start_ms: u32,
    pub end_ms: u32,
    pub duration: f32,
    pub looping: bool,
    /// `moveSpeed` (yd/s): a locomotion clip plays at `unitSpeed / (moveSpeed · scale)`; `0.0`
    /// plays at 1×.
    pub move_speed: f32,
    /// `blendTime` (seconds): the cross-fade in from the live pose.
    pub blend_time: f32,
    /// The box centre: the pick broad phase tests this sphere for the current animation, placed
    /// and scaled, no pad.
    pub bounds_center: [f32; 3],
    /// The sphere radius; `0.0` falls back to the header sphere.
    pub bounds_radius: f32,
    /// The box's min corner. The blob shadow sizes from the Stand sequence's box, clamped to ±5
    /// per axis.
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    /// `frequency`: this variation's weight in the per-play roll.
    pub frequency: u16,
    /// The replay range: each arm rolls a play count `R = max(1, min + ⌊rand·(max−min)/32768⌋)`
    /// into the play window; a looping sequence ignores it.
    pub min_replay: u32,
    pub max_replay: u32,
    pub bones: Vec<BoneKeys>,
    /// Event keys in this sequence's band, seconds from its start.
    pub events: Vec<AnimEvent>,
}

impl ModelAnimation {
    /// Does every bone hold the identity for the whole band? Then the doodad gate builds no rig,
    /// though the reference still arms the sequence and its other bakes still need it.
    pub fn is_rest_pose(&self) -> bool {
        const EPS: f32 = 1e-4;
        !self.bones.iter().any(|b| {
            b.translation.len() > 1
                || b.rotation.len() > 1
                || b.scale.len() > 1
                || b.translation
                    .iter()
                    .any(|(_, v)| v.iter().any(|c| c.abs() > EPS))
                || b.rotation
                    .iter()
                    .any(|(_, q)| (q[3].abs() - 1.0).abs() > EPS)
                || b.scale
                    .iter()
                    .any(|(_, s)| s.iter().any(|c| (c - 1.0).abs() > EPS))
        })
    }
}

/// A bone channel's keys for file slot `slot` over the band `[start, end]` (absolute ms), in
/// seconds from its start. In-band keys go by timestamp; `ranges[slot]` is a bracket (its `hi` can
/// sit in a later band) that decides the edges, where the reference keeps sampling: an empty band
/// takes `keys[lo]` when `lo >= hi`, else the bracket's lerp, and past the last key the lerp runs
/// toward `k0 + 1`, bounded by the key count, not the window. An edge key is added only where it
/// differs from the held one. On a global sequence a lone key is a constant in every clip; a
/// multi-key one belongs to [`parse_m2_global_sequence_bones`].
fn band<T: super::key_anim::Lerp + PartialEq + Copy>(
    track: &M2Track<T>,
    slot: usize,
    start: u32,
    end: u32,
) -> Vec<(f32, T)> {
    let keys = &track.keys;
    if keys.is_empty() {
        return Vec::new();
    }
    if track.gseq != 0xffff {
        return match keys.len() {
            1 => vec![(0.0, keys[0].1)],
            _ => Vec::new(),
        };
    }
    let step = track.interp == 0;
    let last = keys.len() - 1;
    let (lo, hi) = track
        .ranges
        .get(slot)
        .map_or((0, last), |&(lo, hi)| (lo as usize, hi as usize));
    let at = |t| super::key_anim::sample_window(keys, step, lo, hi, t);
    let rebase = |ts: u32| (ts.saturating_sub(start)) as f32 / 1000.0;
    let in_band: Vec<(u32, T)> = keys
        .iter()
        .copied()
        .filter(|&(ts, _)| ts >= start && ts <= end)
        .collect();
    let Some(&(first_ms, first_v)) = in_band.first() else {
        // An empty band holds the window's value; two keys only when the bracket lerp moves.
        let head = match at(start) {
            Some(v) => v,
            None => return Vec::new(),
        };
        return match at(end) {
            Some(tail) if tail != head => vec![(0.0, head), (rebase(end), tail)],
            _ => vec![(0.0, head)],
        };
    };
    let (last_ms, last_v) = in_band[in_band.len() - 1];
    let mut out = Vec::with_capacity(in_band.len() + 2);
    if first_ms > start && at(start).is_some_and(|h| h != first_v) {
        out.push((0.0, at(start).unwrap()));
    }
    out.extend(in_band.iter().map(|&(ts, v)| (rebase(ts), v)));
    if last_ms < end && at(end).is_some_and(|t| t != last_v) {
        out.push((rebase(end), at(end).unwrap()));
    }
    out
}

/// Each bone's rotation, `[x, y, z, w]`, as the reference samples it at the HandsClosed frame
/// (`AnimationData` 15), through the key window: the grip overlay that keeps the weapon hand closed
/// while the body animates. HandsClosed keys no finger bone; its window brackets it.
pub fn hand_grip_finger_poses(model: &M2Model, bones: &[u16]) -> Vec<(u16, [f32; 4])> {
    // HandsClosed's file slot, which indexes the key windows, and its start ms.
    let Some((slot, frame)) = model
        .sequences
        .iter()
        .enumerate()
        .find(|(_, s)| s.id == 15)
        .map(|(i, s)| (i, s.start))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for &bone in bones {
        let Some(tr) = model.bones.get(usize::from(bone)).map(|b| &b.rotation) else {
            continue;
        };
        if tr.gseq != 0xffff || tr.keys.is_empty() {
            continue; // a global-sequence track runs on its own clock
        }
        let last = tr.keys.len() - 1;
        let (lo, hi) = tr
            .ranges
            .get(slot)
            .map_or((0, last), |&(lo, hi)| (lo as usize, hi as usize));
        let Some(v) = super::key_anim::sample_window(&tr.keys, tr.interp == 0, lo, hi, frame)
        else {
            continue;
        };
        out.push((bone, v));
    }
    out
}

/// Every sequence's per-bone keyframes and events; zero-length sequences are skipped.
pub fn parse_m2_animations(model: &M2Model) -> Vec<ModelAnimation> {
    let mut out = Vec::new();
    for (seq_index, s) in model.sequences.iter().enumerate() {
        let (start, end) = (s.start, s.end);
        let duration = end.saturating_sub(start) as f32 / 1000.0;
        if duration <= 0.0 {
            continue;
        }
        let mut bones = Vec::new();
        for (i, b) in model.bones.iter().enumerate() {
            let translation = band(&b.translation, seq_index, start, end);
            let rotation = band(&b.rotation, seq_index, start, end);
            let scale = band(&b.scale, seq_index, start, end);
            if !(translation.is_empty() && rotation.is_empty() && scale.is_empty()) {
                bones.push(BoneKeys {
                    bone: i as u16,
                    translation,
                    rotation,
                    scale,
                });
            }
        }
        let mut events = Vec::new();
        for r in &model.events {
            for &ts in &r.times {
                if ts >= start && ts <= end {
                    events.push(AnimEvent {
                        time: (ts - start) as f32 / 1000.0,
                        ident: r.ident,
                        data: r.data,
                        bone: r.bone,
                        position: r.position,
                    });
                }
            }
        }
        events.sort_by(|a, b| a.time.total_cmp(&b.time));
        let (bmin, bmax) = (s.bounds_min, s.bounds_max);
        out.push(ModelAnimation {
            anim_id: s.id,
            seq_index,
            start_ms: start,
            end_ms: end,
            duration,
            looping: s.flags & 1 == 0, // bit 0 clear loops, set clamps
            move_speed: s.move_speed,
            blend_time: s.blend_time as f32 / 1000.0,
            bounds_center: std::array::from_fn(|k| (bmin[k] + bmax[k]) * 0.5),
            bounds_radius: s.bounds_radius,
            bounds_min: bmin,
            bounds_max: bmax,
            frequency: s.frequency,
            min_replay: s.min_replay,
            max_replay: s.max_replay,
            bones,
            events,
        });
    }
    out
}

/// One bone track as a global-sequence channel: `None` without a global sequence, a non-zero
/// period and two keys (a lone key is a constant [`band`] folds into every clip).
fn global_channel<T: Copy>(
    track: &M2Track<T>,
    global_sequences: &[u32],
) -> Option<GlobalSeqChannel<T>> {
    if track.gseq == 0xffff || track.keys.len() <= 1 {
        return None;
    }
    let period_ms = *global_sequences
        .get(usize::from(track.gseq))
        .filter(|&&d| d > 0)?;
    Some(GlobalSeqChannel {
        period_ms,
        keys: track.keys.clone(),
    })
}

/// Every bone's global-sequence channels, the free-clock loops [`parse_m2_animations`] leaves out
/// (the eye blink's eyelid scale).
pub fn parse_m2_global_sequence_bones(model: &M2Model) -> Vec<GlobalSeqBone> {
    let g = &model.global_sequences;
    model
        .bones
        .iter()
        .enumerate()
        .filter_map(|(i, b)| {
            let translation = global_channel(&b.translation, g);
            let rotation = global_channel(&b.rotation, g);
            let scale = global_channel(&b.scale, g);
            (translation.is_some() || rotation.is_some() || scale.is_some()).then_some(
                GlobalSeqBone {
                    bone: i as u16,
                    translation,
                    rotation,
                    scale,
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use benilla_m2::fixture::{self, floats, header as h, sequence, Model};

    /// One bone keyed in sequence 0 only, and an event in each band, through the typed model.
    #[test]
    fn clips_cut_bone_keys_and_events_to_their_bands() {
        let mut m = Model::new();
        m.array(
            h::SEQUENCES,
            2,
            &[sequence(0, 1000, 0x20), sequence(4, 500, 0x20)].concat(),
        );
        let t = m.track(
            1,
            0xffff,
            &[
                &[(0, floats(&[0.0; 3])), (1000, floats(&[1.0, 0.0, 0.0]))],
                &[],
            ],
        );
        let (r, s) = (m.no_track(), m.no_track());
        let bone = fixture::bone(-1, 0, [0.0; 3], [&t, &r, &s]);
        m.array(h::BONES, 1, &bone);
        let mut ev = b"$SND".to_vec();
        ev.extend_from_slice(&42u32.to_le_bytes());
        ev.extend_from_slice(&[0; 4]);
        ev.extend(floats(&[0.0; 3]));
        ev.extend(m.track_base(0xffff, &[&[250], &[100]]));
        m.array(h::EVENTS, 1, &ev);
        let model = m.parse(&fixture::empty_skin()).unwrap();
        let clips = parse_m2_animations(&model);
        assert_eq!(clips.len(), 2);
        assert_eq!((clips[1].start_ms, clips[1].end_ms), (1001, 1501));
        let walk = &clips[1];
        // Sequence 1 has no keys: its band takes the channel default, one clamp key.
        assert_eq!(walk.bones[0].translation, vec![(0.0, [0.0; 3])]);
        assert_eq!(clips[0].bones[0].translation.len(), 2);
        assert_eq!(walk.events.len(), 1);
        assert!((walk.events[0].time - 0.1).abs() < 1e-6);
        assert_eq!(walk.events[0].data, 42);
    }

    /// `HumanMale.m2`'s one global-sequence bone, the eyelid (75), scales 0 (open) to 1 (shut).
    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn human_male_eyelid_blink_global_sequence() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        let bytes = crate::read_m2(&chain, "Character\\Human\\Male\\HumanMale.m2")
            .expect("read HumanMale.m2");
        let gs = parse_m2_global_sequence_bones(&bytes);

        assert_eq!(
            gs.len(),
            1,
            "HumanMale has one global-seq bone (the eyelid)"
        );
        let eyelid = &gs[0];
        assert_eq!(eyelid.bone, 75);
        assert!(
            eyelid.translation.is_none() && eyelid.rotation.is_none(),
            "the eyelid blink is scale-only"
        );
        let scale = eyelid.scale.as_ref().expect("eyelid has a scale channel");
        assert!(
            scale.period_ms > 1000,
            "a real multi-second loop, not the empty 0/1 ms table (got {} ms)",
            scale.period_ms
        );
        assert!(scale.keys.len() >= 4);
        assert_eq!(scale.keys[0].0, 0);
        assert!(
            scale.keys[0].1.iter().all(|&c| c.abs() < 1e-3),
            "loop starts eye-open (scale 0), got {:?}",
            scale.keys[0].1
        );
        assert!(
            scale
                .keys
                .iter()
                .any(|(_, v)| v.iter().all(|&c| (c - 1.0).abs() < 1e-3)),
            "the loop has a shut frame (scale 1) — the blink"
        );
    }

    /// `UI_Human` parks a constant −16.5° yaw on its stage bone to face its off-axis camera; stage
    /// scales are 0.97 on `UI_Human`, 1.04 and 0.7324 (pet) on `UI_Tauren`.
    #[test]
    fn the_glue_stage_bones_carry_the_scenes_authored_rotation_and_scale() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        // (scene token, stage yaw °, character-stage scale, pet-stage scale)
        let scenes = [
            ("Human", -16.5_f32, 0.97_f32, 0.97_f32),
            ("Orc", 0.0, 1.0, 1.0),
            ("Dwarf", 0.0, 1.0, 1.0),
            ("NightElf", 0.0, 1.0, 1.0),
            ("Scourge", 0.0, 1.0, 1.0),
            ("Tauren", 0.0, 1.04, 0.7324),
        ];
        for (token, want_deg, want_scale, want_pet_scale) in scenes {
            let bytes = crate::read_m2(
                &chain,
                &format!("Interface\\Glues\\Models\\UI_{token}\\UI_{token}.m2"),
            )
            .unwrap_or_else(|e| panic!("read UI_{token}.m2: {e}"));
            let model = &bytes;
            let bone_of = |id: u16| {
                model
                    .attachments
                    .iter()
                    .find(|a| a.id == id)
                    .unwrap_or_else(|| panic!("UI_{token} has attachment {id}"))
                    .bone
            };
            let gs_all = parse_m2_global_sequence_bones(&bytes);
            // The reference composes the whole bone matrix (`T(att.pos) · parentBone[att.bone]`),
            // so a parked scale resizes what stands on it.
            let scale_of = |bone: u16| -> f32 {
                gs_all
                    .iter()
                    .find(|g| g.bone == bone)
                    .and_then(|g| g.scale.as_ref())
                    .map_or(1.0, |c| c.keys[0].1[0])
            };
            assert!(
                (scale_of(bone_of(0)) - want_scale).abs() < 5e-4,
                "UI_{token} character-stage scale: want {want_scale}, got {}",
                scale_of(bone_of(0))
            );
            assert!(
                (scale_of(bone_of(1)) - want_pet_scale).abs() < 5e-4,
                "UI_{token} pet-stage scale: want {want_pet_scale}, got {}",
                scale_of(bone_of(1))
            );

            let stage = model
                .attachments
                .iter()
                .find(|a| a.id == 0)
                .unwrap_or_else(|| panic!("UI_{token} has a stage attachment"));

            // Every shipped stage bone is a root; the walk covers a nested one.
            let gs = parse_m2_global_sequence_bones(&bytes);
            let mut yaw = 0.0_f32;
            let mut bone = stage.bone as i16;
            while bone >= 0 {
                if let Some(rot) = gs
                    .iter()
                    .find(|g| g.bone as i16 == bone)
                    .and_then(|g| g.rotation.as_ref())
                {
                    let first = rot.keys[0].1;
                    assert!(
                        rot.keys
                            .iter()
                            .all(|(_, q)| q.iter().zip(first).all(|(a, b)| (a - b).abs() < 1e-6)),
                        "UI_{token}'s stage rotation is a PARKED key, not an animation — a \
                         moving stage would have to be read off the live joint instead"
                    );
                    let [x, y, z, w] = first;
                    assert!(
                        x.abs() < 1e-3 && y.abs() < 1e-3,
                        "UI_{token}'s stage rotation is a pure YAW about +Z — a stage tipped in \
                         pitch or roll would need composing, not summing (got {first:?})"
                    );
                    yaw += 2.0 * f32::atan2(z, w).to_degrees();
                }
                bone = model.bones[bone as usize].parent;
            }
            assert!(
                (yaw - want_deg).abs() < 0.1,
                "UI_{token} stage yaw: want {want_deg}°, got {yaw}°"
            );
        }
    }

    /// HumanMale's waist (bone 27) keys rotation only outside Run's band, which `ranges[Run]`
    /// brackets as `(2, 3)`, so Run carries that value. A bone keyed anywhere is keyed in all.
    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn empty_band_poses_the_bone_and_never_drops_it() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        let bytes = crate::read_m2(&chain, "Character\\Human\\Male\\HumanMale.m2")
            .expect("read HumanMale.m2");
        let seqs = parse_m2_animations(&bytes);

        let run = seqs
            .iter()
            .find(|s| s.anim_id == 5)
            .expect("HumanMale has Run");
        let waist = run
            .bones
            .iter()
            .find(|b| b.bone == 27)
            .expect("bone 27 carried into the Run clip");
        assert_eq!(
            waist.rotation.len(),
            1,
            "an empty band emits exactly the one clamp key"
        );
        assert_eq!(waist.rotation[0].0, 0.0);

        let mut keyed = [
            std::collections::HashSet::new(),
            std::collections::HashSet::new(),
        ];
        for s in &seqs {
            for b in &s.bones {
                if !b.translation.is_empty() {
                    keyed[0].insert(b.bone);
                }
                if !b.rotation.is_empty() {
                    keyed[1].insert(b.bone);
                }
            }
        }
        for (i, name) in ["translation", "rotation"].iter().enumerate() {
            for s in &seqs {
                for &bone in &keyed[i] {
                    let present = s.bones.iter().any(|b| {
                        b.bone == bone
                            && if i == 0 {
                                !b.translation.is_empty()
                            } else {
                                !b.rotation.is_empty()
                            }
                    });
                    assert!(
                        present,
                        "seq idx anim {} drops bone {bone}'s {name} — a stale-pose freeze",
                        s.anim_id
                    );
                }
            }
        }
    }

    /// `Zombie.m2`'s root translation steps from `(0,0,0)` (3333 ms) to `(0.0047,0,0)` (30000 ms);
    /// Walk's empty band `[26667, 27333]` sits nearer the second, but `ranges` holds key 0.
    #[test]
    fn empty_band_takes_the_window_key_not_a_later_one() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        let bytes = crate::read_m2(&chain, "Creature\\Zombie\\Zombie.m2").expect("read Zombie.m2");
        let seqs = parse_m2_animations(&bytes);
        for anim in [4, 5] {
            let s = seqs
                .iter()
                .find(|s| s.anim_id == anim)
                .unwrap_or_else(|| panic!("Zombie has anim {anim}"));
            let root = s
                .bones
                .iter()
                .find(|b| b.bone == 0)
                .expect("the root is carried into the clip");
            assert_eq!(
                root.translation.len(),
                1,
                "a step track's empty band holds one value across the band"
            );
            assert_eq!(
                root.translation[0].1,
                [0.0, 0.0, 0.0],
                "anim {anim} must hold key 0, not step early to the 0.0047 key at 30000 ms"
            );
        }
    }

    /// HandsClosed on `HumanMale.m2` is the 33 ms band `[60000, 60033]`, which keys nothing on
    /// finger bone 102; its window brackets it with the same curl at 43333 and 70000 ms.
    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn hand_grip_reads_the_curled_pose_through_the_window() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        let bytes = crate::read_m2(&chain, "Character\\Human\\Male\\HumanMale.m2")
            .expect("read HumanMale.m2");
        let poses = hand_grip_finger_poses(&bytes, &[102]);
        let (bone, q) = *poses.first().expect("bone 102 carries a grip pose");
        assert_eq!(bone, 102);
        let want = [0.2536, -0.016, 0.0577, 0.9654];
        for (i, (&got, &w)) in q.iter().zip(want.iter()).enumerate() {
            assert!(
                (got - w).abs() < 1e-3,
                "grip quat component {i}: got {got}, want ~{w} (the 43333/70000 ms curl)"
            );
        }
    }

    /// A key window's far bracket can sit 14–64 s away (`Bear`) and must not enter the clip.
    #[test]
    fn no_clip_carries_a_key_from_another_sequences_band() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        for model in [
            "Creature\\Bear\\Bear.m2",
            "Creature\\Chicken\\Chicken.m2",
            "Character\\Human\\Male\\HumanMale.m2",
        ] {
            let bytes = crate::read_m2(&chain, model).expect("read model");
            for s in parse_m2_animations(&bytes) {
                for b in &s.bones {
                    let times = b
                        .translation
                        .iter()
                        .map(|&(t, _)| ("translation", t))
                        .chain(b.rotation.iter().map(|&(t, _)| ("rotation", t)))
                        .chain(b.scale.iter().map(|&(t, _)| ("scale", t)));
                    for (ch, t) in times {
                        assert!(
                            (0.0..=s.duration + 1e-3).contains(&t),
                            "{model} anim {} bone {} {ch}: key at {t}s is outside the \
                             {}s clip — a bracket key leaked in as a playable key",
                            s.anim_id,
                            b.bone,
                            s.duration
                        );
                    }
                }
            }
        }
    }

    /// `LShoulder_Mail_PVPAlliance_C_01.m2`: two sparkle emitters on bones 2 and 3 under billboard
    /// bone 1, each at its own pivot ~0.24 yd from the host's; the held torch is bone 10 under 2.
    #[test]
    fn real_pvp_shoulder_emitters_ride_a_billboard_bone() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        let bytes = crate::read_m2(
            &chain,
            "Item\\ObjectComponents\\Shoulder\\LShoulder_Mail_PVPAlliance_C_01.m2",
        )
        .expect("read the R14 mail shoulder");
        let skel = parse_m2_skeleton(&bytes);
        assert_eq!(skel.bones.len(), 4);
        assert_eq!(
            skel.bones.iter().filter(|b| b.billboard.is_some()).count(),
            1,
            "one billboard bone in the chain"
        );
        assert_eq!(
            skel.bones[1].billboard,
            Some(crate::BillboardKind::Spherical),
            "bone 1 authors flag 0x08"
        );
        let emitters = crate::parse_m2_particle_emitters(&bytes).expect("emitters");
        assert_eq!(emitters.len(), 2);
        for (i, em) in emitters.iter().enumerate() {
            let bone = usize::from(em.bone);
            assert_eq!(bone, 2 + i, "the sparkle pair rides bones 2 and 3");
            assert!(
                skel.bones[bone].billboard.is_none(),
                "…neither of which carries the flag itself"
            );
            assert_eq!(
                skel.billboard_host(em.bone),
                Some(1),
                "so the host is their parent"
            );
            for c in 0..3 {
                assert!(
                    (em.position[c] - skel.bones[bone].pivot[c]).abs() < 1e-3,
                    "emitter {i} position {:?} vs bone pivot {:?}",
                    em.position,
                    skel.bones[bone].pivot
                );
            }
            let d: f32 = (0..3)
                .map(|c| (em.position[c] - skel.bones[1].pivot[c]).powi(2))
                .sum();
            assert!(
                (0.23..0.26).contains(&d.sqrt()),
                "chain offset {} yd",
                d.sqrt()
            );
        }
        let torch = crate::read_m2(
            &chain,
            "Item\\ObjectComponents\\Weapon\\Club_1H_Torch_A_01.m2",
        )
        .expect("read the torch");
        let tskel = parse_m2_skeleton(&torch);
        let tem = crate::parse_m2_particle_emitters(&torch).expect("emitters");
        assert_eq!(tem.len(), 1);
        assert_eq!(tem[0].bone, 10);
        assert_eq!(tskel.billboard_host(10), Some(2));
        assert_eq!(
            tskel.bones[2].billboard,
            Some(crate::BillboardKind::Spherical)
        );
        // Bone 0, the root, has no billboard above it; an out-of-range index is `None`.
        assert_eq!(skel.billboard_host(0), None);
        assert_eq!(skel.billboard_host(999), None);
    }

    /// `Sparkle_A.m2` (`ItemVisuals` 28) is one spherical-billboard bone and no emitters or
    /// ribbons: a preview keeping only emitters draws nothing for it.
    #[test]
    fn a_real_item_glow_model_is_pure_billboard_geometry_with_no_emitters() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        let bytes = crate::read_m2(&chain, "Spells\\Enchantments\\Sparkle_A.m2")
            .expect("read the Sparkle_A item glow");
        let skel = parse_m2_skeleton(&bytes);
        assert_eq!(skel.bones.len(), 1, "one bone — the quad's own");
        assert_eq!(
            skel.bones[0].billboard,
            Some(crate::BillboardKind::Spherical),
            "…and it is a spherical billboard, so its geometry is a card"
        );
        assert!(
            crate::parse_m2_particle_emitters(&bytes)
                .expect("emitters")
                .is_empty(),
            "no particle emitters: carrying only emitters carries nothing"
        );
        assert!(
            crate::parse_m2_ribbon_emitters(&bytes)
                .expect("ribbons")
                .is_empty(),
            "nor ribbons"
        );
    }
}

#[cfg(test)]
mod doodad_sound_tests {
    /// `KalidarStreetLamp01.m2`: one looping Stand keying no bone, so no rig, and one `$DSL` at
    /// t = 0 (SoundEntries 3378): the event track must not follow the rig gate.
    #[test]
    fn the_lamps_hum_is_one_rest_posed_looping_dsl_key() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        let bytes = crate::read_m2(
            &chain,
            "World\\Generic\\NightElf\\Passive Doodads\\Lamps\\KalidarStreetLamp01.m2",
        )
        .expect("the lamp is in the chain");
        let seqs = super::parse_m2_animations(&bytes);
        assert_eq!(seqs.len(), 1, "one authored sequence");

        let stand = &seqs[0];
        assert_eq!(stand.anim_id, 0, "Stand");
        assert!(stand.looping, "the hum's carrier loops");
        assert!(
            stand.is_rest_pose(),
            "keys no bone — the rig gate skips it, and that must not silence it"
        );

        assert_eq!(stand.events.len(), 1, "exactly one event key");
        let ev = &stand.events[0];
        assert_eq!(&ev.ident, b"$DSL", "a doodad sound LOOP tag");
        assert_eq!(ev.data, 3378, "SoundEntries NightElfStreetLampLoop");
        assert!(ev.time.abs() < 1e-6, "keyed at t = 0");
    }

    /// `bellows.m2` keys two `$DSL` at t = 0 and 1.1 on one 2 s loop. Each loops, but a doodad
    /// holds one registration (`[CMapDoodadDef+0x168]`) and a marker with another id releases it
    /// first, so the pair alternates (`sound::anim_events`).
    #[test]
    fn a_dsl_pair_on_one_cycle_alternates_through_one_slot() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        let bytes = crate::read_m2(
            &chain,
            "World\\Generic\\Human\\Passive Doodads\\Bellows\\bellows.m2",
        )
        .expect("the bellows is in the chain");
        let seqs = super::parse_m2_animations(&bytes);
        let carrier = seqs
            .iter()
            .find(|s| s.events.iter().any(|e| &e.ident == b"$DSL"))
            .expect("a sequence carries the pair");
        assert!(carrier.looping, "and it loops");

        let mut keys: Vec<_> = carrier
            .events
            .iter()
            .filter(|e| &e.ident == b"$DSL")
            .map(|e| (e.data, e.time))
            .collect();
        keys.sort_by(|a, b| a.1.total_cmp(&b.1));
        assert_eq!(keys.len(), 2, "a pair sharing one registration slot");
        assert!(keys[0].1.abs() < 1e-6, "the first is keyed at t = 0");
        assert!(
            keys[1].1 > 1.0,
            "the second is keyed a second later ({:.3}s) — the eviction is what makes it a pump",
            keys[1].1
        );
        assert_ne!(keys[0].0, keys[1].0, "and they are different kits");
    }
}
