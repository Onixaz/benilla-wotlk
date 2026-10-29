//! Global-sequence bone channels: loops on a free clock (the eyelid blink, fidget pulses, an
//! effect's twinkles), which the per-sequence reader drops. The reference's cursor is
//! `(sceneClock − attachTime) % duration`, one ms clock per scene (`[scene+0xc]`) snapshotted once
//! per model instance at attach (model `+0x68`); a spell effect is a fresh instance per play (no
//! pooling).

use bevy::prelude::*;

use benilla_assets::GlobalBone;

/// A joint entity (doodad, effect, booth) or a bone of the host's [`super::RigPose`].
enum SeqTarget {
    Joint(Entity),
    Bone(u16),
}

/// A model instance's global-sequence channels and clock anchor.
#[derive(Component)]
pub struct GlobalSeqDrive {
    bones: Vec<(SeqTarget, GlobalBone)>,
    /// The scene clock at attach (secs), stamped once, like the reference's `+0x68`.
    anchor: Option<f64>,
    /// The owning scene's clock (secs) when it is not the world's: the reference reads
    /// `[[model+0x2c]+0xc]`, and a `<Model>` widget owns a private scene (widget `+0x314`)
    /// advanced only by its `OnUpdate`, so a hidden pane's clock stops.
    clock: Option<f64>,
    /// Skip the writes: the doodad lane pauses culled instances, as the reference evaluates a model
    /// only when drawn. The cursor is absolute, so a resume needs no seek.
    paused: bool,
}

impl GlobalSeqDrive {
    /// Map the model's global-sequence bones to this instance's joint entities.
    pub fn new(global_bones: &[GlobalBone], joints: &[Entity]) -> Option<Self> {
        let bones: Vec<_> = global_bones
            .iter()
            .filter_map(|g| {
                joints
                    .get(g.bone as usize)
                    .map(|&e| (SeqTarget::Joint(e), g.clone()))
            })
            .collect();
        (!bones.is_empty()).then_some(Self {
            bones,
            anchor: None,
            clock: None,
            paused: false,
        })
    }

    /// The collapsed-rig lane: channels write the host's [`super::RigPose`] locals by bone index.
    pub fn new_rig(global_bones: &[GlobalBone], nbones: usize) -> Option<Self> {
        let bones: Vec<_> = global_bones
            .iter()
            .filter(|g| (g.bone as usize) < nbones)
            .map(|g| (SeqTarget::Bone(g.bone), g.clone()))
            .collect();
        (!bones.is_empty()).then_some(Self {
            bones,
            anchor: None,
            clock: None,
            paused: false,
        })
    }

    /// Hold only the writes, for the doodad draw gate; [`super::AnimParked`] also holds the pose.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Set this instance's scene clock (secs), each frame, for a scene that is not the world's.
    pub fn set_clock(&mut self, secs: f64) {
        self.clock = Some(secs);
    }
}

/// Sample each drive at `sceneNow − anchor`, wrapped in f64 for a long uptime, and write only the
/// driven components, so the eyelid blinks over any gait.
fn apply_global_sequences(
    time: Res<Time>,
    mut drives: Query<(
        &mut GlobalSeqDrive,
        Option<&mut super::RigPose>,
        Has<super::AnimParked>,
    )>,
    mut joints: Query<&mut Transform>,
) {
    let now = time.elapsed_secs_f64();
    // A collapsed rig's channels write only its own pose, so those drives (the drawn swaying
    // doodads) run in parallel; a joint drive writes other entities and runs after, serially.
    drives.par_iter_mut().for_each(|(mut drive, rig, parked)| {
        let Some(t) = drive_cursor(&mut drive, now, parked) else {
            return;
        };
        let Some(mut rig) = rig else { return };
        for (target, bone) in &drive.bones {
            let SeqTarget::Bone(b) = target else { continue };
            rig.pose_dirty = true;
            if let Some(tf) = rig.locals.get_mut(*b as usize) {
                sample_into(tf, bone, t);
            }
        }
    });
    for (drive, _, parked) in &drives {
        let Some(t) = drive.anchor.map(|a| drive.clock.unwrap_or(now) - a) else {
            continue;
        };
        if drive.paused || parked {
            continue;
        }
        for (target, bone) in &drive.bones {
            let SeqTarget::Joint(joint) = target else {
                continue;
            };
            if let Ok(mut tf) = joints.get_mut(*joint) {
                sample_into(&mut tf, bone, t);
            }
        }
    }
}

/// Stamp the drive's anchor and return its cursor, or `None` while it is held. Stamped even while
/// parked: the reference stamps `+0x68` at attach, not at first draw.
fn drive_cursor(drive: &mut GlobalSeqDrive, now: f64, parked: bool) -> Option<f64> {
    let scene_now = drive.clock.unwrap_or(now);
    let t = scene_now - *drive.anchor.get_or_insert(scene_now);
    (!drive.paused && !parked).then_some(t)
}

/// Write only the driven components at cursor `t`, each channel wrapped on its own period.
fn sample_into(tf: &mut Transform, bone: &GlobalBone, t: f64) {
    let at = |period: f32| (t % f64::from(period.max(1e-3))) as f32;
    if let Some(c) = &bone.translation {
        tf.translation = c.sample(at(c.period));
    }
    if let Some(c) = &bone.rotation {
        tf.rotation = c.sample(at(c.period));
    }
    if let Some(c) = &bone.scale {
        tf.scale = c.sample(at(c.period));
    }
}

/// Register [`apply_global_sequences`] in the pose post-pass window (beside the body twist).
pub fn plugin(app: &mut App) {
    app.add_systems(PostUpdate, apply_global_sequences.in_set(super::PosePost));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig_anim::RigPose;
    use benilla_assets::GlobalSeqChannel;

    fn eyelid_bone() -> GlobalBone {
        GlobalBone {
            bone: 75,
            translation: None,
            rotation: None,
            // The eyelid's shape: open (0), shut (1) for the blink, open again.
            scale: Some(GlobalSeqChannel {
                period: 6.633,
                keys: vec![
                    (0.0, Vec3::ZERO),
                    (0.033, Vec3::ONE),
                    (0.100, Vec3::ONE),
                    (0.133, Vec3::ZERO),
                ],
            }),
        }
    }

    /// A linear ramp (`scale = t`, long period), so a phase difference shows at any elapsed time.
    fn ramp() -> GlobalBone {
        GlobalBone {
            bone: 0,
            translation: None,
            rotation: None,
            scale: Some(GlobalSeqChannel {
                period: 100.0,
                keys: vec![(0.0, Vec3::ZERO), (100.0, Vec3::splat(100.0))],
            }),
        }
    }

    /// A drive spawned later reads a smaller cursor: the phase is per instance.
    #[test]
    fn fresh_drives_anchor_at_attach() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_millis(60),
        ));
        app.add_systems(Update, apply_global_sequences);

        let early_joint = app.world_mut().spawn(Transform::default()).id();
        app.world_mut()
            .spawn(GlobalSeqDrive::new(&[ramp()], &[early_joint]).expect("a keyed channel maps"));
        app.update();
        app.update();
        let late_joint = app.world_mut().spawn(Transform::default()).id();
        app.world_mut()
            .spawn(GlobalSeqDrive::new(&[ramp()], &[late_joint]).expect("a keyed channel maps"));
        app.update();
        app.update();

        let s = |e: Entity| app.world().entity(e).get::<Transform>().unwrap().scale.x;
        assert!(
            s(early_joint) > s(late_joint),
            "the later attach reads a smaller cursor (per-instance anchor): early {} vs late {}",
            s(early_joint),
            s(late_joint)
        );
        assert!(
            s(late_joint) > 0.0,
            "the late drive did tick from its own anchor (got {})",
            s(late_joint)
        );
    }

    #[test]
    fn eyelid_channel_samples_by_clock_value() {
        let bone = eyelid_bone();
        let c = bone.scale.as_ref().unwrap();
        assert!(
            c.sample(0.06).abs_diff_eq(Vec3::ONE, 1e-3),
            "shut mid-blink"
        );
        assert!(
            c.sample(3.0).abs_diff_eq(Vec3::ZERO, 1e-3),
            "open in the tail"
        );
        assert!(
            c.sample(3.0 + 2.0 * 6.633).abs_diff_eq(Vec3::ZERO, 1e-3),
            "wraps on its period"
        );
    }

    /// Both drives spawn on the same frame, so their anchors coincide.
    #[test]
    fn joint_and_rig_targets_write_the_same_pose() {
        let full = GlobalBone {
            bone: 0,
            translation: Some(benilla_assets::GlobalSeqChannel {
                period: 2.5,
                keys: vec![(0.0, Vec3::ZERO), (1.2, Vec3::X), (2.5, Vec3::NEG_Z)],
            }),
            rotation: Some(benilla_assets::GlobalSeqChannel {
                period: 1.7,
                keys: vec![(0.0, Quat::IDENTITY), (1.7, Quat::from_rotation_y(0.9))],
            }),
            scale: Some(benilla_assets::GlobalSeqChannel {
                period: 100.0,
                keys: vec![(0.0, Vec3::ONE), (100.0, Vec3::splat(3.0))],
            }),
        };
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_millis(47),
        ));
        app.add_systems(Update, apply_global_sequences);

        let joint = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().spawn(
            GlobalSeqDrive::new(std::slice::from_ref(&full), &[joint]).expect("keyed channels map"),
        );
        let skeleton = benilla_assets::ModelSkeleton {
            joints: vec![benilla_assets::ModelJoint {
                parent: -1,
                local_translation: Vec3::ZERO,
                billboard: None,
                parent_arm: None,
            }],
            spine_bone: None,
            head_bone: None,
        };
        let host = app.world_mut().spawn_empty().id();
        app.world_mut().entity_mut(host).insert((
            RigPose::new(host, &skeleton),
            GlobalSeqDrive::new_rig(&[full], 1).expect("keyed channels map"),
        ));

        for frame in 0..6 {
            app.update();
            let jt = *app.world().entity(joint).get::<Transform>().unwrap();
            let rl = app.world().entity(host).get::<RigPose>().unwrap().locals[0];
            assert_eq!(jt, rl, "frame {frame}: joint target vs rig target diverged");
        }
    }
}
