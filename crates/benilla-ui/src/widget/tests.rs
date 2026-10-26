//! Tests of [`super::WidgetArena`]'s mutators and registries.

use super::*;

fn arena() -> WidgetArena {
    WidgetArena::new()
}

// ── Effective visibility ─────────────────────────────────────────────────────────────────────

#[test]
fn hide_show_toggles_effective_visibility_and_reports_changes() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    assert!(a.frame(root).unwrap().effective_visible);

    let changed = a.set_shown(root, false);
    assert_eq!(changed, vec![root]);
    assert!(!a.frame(root).unwrap().effective_visible);

    assert!(a.set_shown(root, false).is_empty());

    let changed = a.set_shown(root, true);
    assert_eq!(changed, vec![root]);
    assert!(a.frame(root).unwrap().effective_visible);
}

#[test]
fn the_glance_table_follows_visibility_destroy_and_slot_reuse() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Cooldown, None, Some(root));
    let glance = |a: &WidgetArena, h| a.glance(h).map(|g| (g.visible, g.kind));
    assert_eq!(glance(&a, child), Some((true, FrameKind::Cooldown)));

    a.set_shown(root, false);
    assert_eq!(glance(&a, child), Some((false, FrameKind::Cooldown)));
    assert!(!a.is_visible(child));
    a.set_shown(root, true);
    assert!(a.is_visible(child));

    a.destroy(child);
    assert_eq!(a.glance(child), None);
    // The freed slot's next tenant answers for its own handle only.
    let next = a.create(FrameKind::MessageFrame, None, None);
    assert_eq!(a.glance(child), None);
    assert_eq!(glance(&a, next), Some((true, FrameKind::MessageFrame)));
    for (h, f) in a.iter_frames() {
        assert_eq!(
            a.is_visible(h),
            f.effective_visible,
            "glance agrees with the node"
        );
    }
}

/// Every node's visibility and walk key, as its glance mirrors them.
fn assert_glances_agree(a: &WidgetArena) {
    for (h, f) in a.iter_frames() {
        let g = a.glance(h).expect("a live frame has a glance");
        assert_eq!(
            (g.visible, g.walk_key()),
            (f.effective_visible, (f.strata, f.level, f.insertion_seq)),
            "glance of {h:?}"
        );
    }
}

#[test]
fn the_glance_mirrors_the_walk_key_through_every_mutator() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    let grand = a.create(FrameKind::Frame, None, Some(child));
    let other = a.create(FrameKind::Frame, None, None);
    assert_glances_agree(&a);
    a.set_frame_level(root, 5, true);
    assert_glances_agree(&a);
    a.set_frame_strata(child, Strata::Dialog);
    assert_glances_agree(&a);
    a.set_shown(root, false);
    a.set_shown(root, true);
    assert_glances_agree(&a);
    a.set_parent(grand, Some(other));
    assert_glances_agree(&a);
    a.compress_strata(Strata::Medium);
    assert_glances_agree(&a);
    let epoch = a.link_epoch();
    a.set_shown(other, false);
    assert!(a.link_epoch() > epoch, "an unlink moves the link epoch");
    let epoch = a.link_epoch();
    a.set_frame_level(other, 9, false);
    assert_eq!(
        a.link_epoch(),
        epoch,
        "a hidden frame's level change links nothing"
    );
    a.set_shown(other, true);
    assert!(a.link_epoch() > epoch, "a show links");
    assert_glances_agree(&a);
}

#[test]
fn mid_tree_hide_blocks_a_shown_grandchild() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    let grand = a.create(FrameKind::Frame, None, Some(child));
    assert!(a.frame(grand).unwrap().effective_visible);

    let changed = a.set_shown(child, false);
    assert_eq!(
        changed,
        vec![grand, child],
        "each frame after its descendants"
    );
    assert!(a.frame(root).unwrap().effective_visible);
    assert!(!a.frame(child).unwrap().effective_visible);
    assert!(!a.frame(grand).unwrap().effective_visible);
    assert!(
        a.frame(grand).unwrap().shown,
        "grandchild's own shown bit is untouched"
    );

    let changed = a.set_shown(child, true);
    assert_eq!(changed, vec![grand, child]);
    assert!(a.frame(grand).unwrap().effective_visible);
}

#[test]
fn hidden_grandchild_stays_hidden_when_ancestor_reshows() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    let grand = a.create(FrameKind::Frame, None, Some(child));

    a.set_shown(grand, false);
    let changed = a.set_shown(root, false);
    // grand is already invisible, so the hide does not report it.
    assert_eq!(changed, vec![child, root]);

    let changed = a.set_shown(root, true);
    assert_eq!(changed, vec![child, root]);
    assert!(a.frame(child).unwrap().effective_visible);
    assert!(!a.frame(grand).unwrap().effective_visible);
}

// ── Strata ───────────────────────────────────────────────────────────────────────────────────

#[test]
fn set_strata_forces_whole_subtree() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    let grand = a.create(FrameKind::Frame, None, Some(child));

    a.set_frame_strata(root, Strata::Dialog);
    assert_eq!(a.frame(root).unwrap().strata, Strata::Dialog);
    assert_eq!(a.frame(child).unwrap().strata, Strata::Dialog);
    assert_eq!(a.frame(grand).unwrap().strata, Strata::Dialog);
}

// ── Level ────────────────────────────────────────────────────────────────────────────────────

#[test]
fn set_level_delta_shifts_same_strata_children_only() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let same = a.create(FrameKind::Frame, None, Some(root));
    let cross = a.create(FrameKind::Frame, None, Some(root));
    // Distinct child levels show the delta is kept, not the absolute level.
    a.set_frame_level(same, 5, true);
    a.set_frame_level(cross, 2, true);
    a.set_frame_strata(cross, Strata::High);

    a.set_frame_level(root, 10, true);
    assert_eq!(a.frame(root).unwrap().level, 10);
    assert_eq!(
        a.frame(same).unwrap().level,
        15,
        "same-strata child shifted by +10"
    );
    assert_eq!(
        a.frame(cross).unwrap().level,
        2,
        "cross-strata child untouched"
    );
}

#[test]
fn level_shift_saturates_at_zero() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    a.set_frame_level(root, 10, true);
    a.set_frame_level(child, 3, true);
    // Root drops by 10, which would take the child to -7: it stops at 0.
    a.set_frame_level(root, 0, true);
    assert_eq!(a.frame(child).unwrap().level, 0);
}

// ── Level compression ────────────────────────────────────────────────────────────────────────

/// Top-level frames at `levels`, created in order, each shown so it links and lifts the bound.
fn at_levels(a: &mut WidgetArena, levels: &[u16]) -> Vec<FrameHandle> {
    levels
        .iter()
        .map(|&level| {
            let h = a.create(FrameKind::Frame, None, None);
            a.set_frame_level(h, level, false);
            h
        })
        .collect()
}

fn levels_of(a: &WidgetArena, frames: &[FrameHandle]) -> Vec<u16> {
    frames.iter().map(|&h| a.frame(h).unwrap().level).collect()
}

/// `0x495060` resumes at the used level after a drop, so the second run of empties inside the
/// dropped span survives: `{0, 4, 6}` under a bound of 7 becomes `{0, 1, 3}` under 4.
#[test]
fn compression_leaves_the_references_hole_past_a_wide_gap() {
    let mut a = arena();
    let frames = at_levels(&mut a, &[0, 4, 6]);
    assert_eq!(a.top_level(Strata::Medium), 7);
    assert_eq!(a.compress_strata(Strata::Medium), 4);
    assert_eq!(levels_of(&a, &frames), vec![0, 1, 3]);
    assert_eq!(a.top_level(Strata::Medium), 4, "the bound is written back");
}

/// A frame at or past the bound is outside the scan: hidden at level 10 under a bound of 2, it
/// keeps its level rather than being pulled down under the frames a raise lands above.
#[test]
fn compression_leaves_a_hidden_frame_past_the_bound_where_it_is() {
    let mut a = arena();
    let frames = at_levels(&mut a, &[0, 1]);
    let high = a.create(FrameKind::Frame, None, None);
    a.set_shown(high, false);
    a.set_frame_level(high, 10, false);
    assert_eq!(
        a.top_level(Strata::Medium),
        2,
        "a hidden frame links nowhere"
    );
    assert_eq!(a.compress_strata(Strata::Medium), 2);
    assert_eq!(levels_of(&a, &frames), vec![0, 1]);
    assert_eq!(a.frame(high).unwrap().level, 10);
    // Showing it links it at 10, which lifts the bound past it (`0x495de0`).
    a.set_shown(high, true);
    assert_eq!(a.top_level(Strata::Medium), 11);
    assert_eq!(a.compress_strata(Strata::Medium), 3);
    assert_eq!(a.frame(high).unwrap().level, 2);
}

/// A hidden frame below the bound holds its level open, as the scan reads the frame registry.
#[test]
fn a_hidden_frame_below_the_bound_keeps_its_level_in_use() {
    let mut a = arena();
    let frames = at_levels(&mut a, &[0, 2, 3]);
    a.set_shown(frames[1], false);
    assert_eq!(a.compress_strata(Strata::Medium), 3);
    assert_eq!(levels_of(&a, &frames), vec![0, 1, 2]);
}

/// The moves go through the level setter, which relinks a visible frame at its bucket's tail, one
/// level at a time and in creation order: a frame relinked late at level 4 ends up behind an
/// older one moved after it.
#[test]
fn compression_relinks_moved_frames_in_creation_order() {
    let mut a = arena();
    let frames = at_levels(&mut a, &[0, 4, 4, 5]);
    let (older, newer, above) = (frames[1], frames[2], frames[3]);
    // Relink `older` after `newer`: its stamp is now the later one.
    a.set_shown(older, false);
    a.set_shown(older, true);
    let seq = |a: &WidgetArena, h| a.frame(h).unwrap().insertion_seq;
    assert!(seq(&a, older) > seq(&a, newer));

    assert_eq!(a.compress_strata(Strata::Medium), 3);
    assert_eq!(levels_of(&a, &frames), vec![0, 1, 1, 2]);
    assert!(
        seq(&a, older) < seq(&a, newer),
        "creation order relinks the older frame first"
    );
    assert!(
        seq(&a, newer) < seq(&a, above),
        "level 4 moves before level 5"
    );
}

// ── Scale ────────────────────────────────────────────────────────────────────────────────────

#[test]
fn scale_propagates_multiplicatively() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    let grand = a.create(FrameKind::Frame, None, Some(child));

    a.set_scale(root, 2.0);
    a.set_scale(child, 3.0);
    assert_eq!(a.frame(root).unwrap().effective_scale, 2.0);
    assert_eq!(a.frame(child).unwrap().effective_scale, 6.0); // 2 * 3
    assert_eq!(a.frame(grand).unwrap().effective_scale, 6.0); // 2 * 3 * 1

    a.set_scale(grand, 0.5);
    assert_eq!(a.frame(grand).unwrap().effective_scale, 3.0); // 6 * 0.5
}

#[test]
fn scale_epsilon_gate_skips_subthreshold_change() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    a.set_scale(child, 4.0);
    assert_eq!(a.frame(child).unwrap().effective_scale, 4.0);

    // A sub-ε change to the root prunes the recursion, so the child keeps its exact value.
    let tiny = 1.0 + (SCALE_EPS as f32) / 4.0;
    a.set_scale(root, tiny);
    assert_eq!(
        a.frame(child).unwrap().effective_scale,
        4.0,
        "sub-ε parent change must not disturb the subtree"
    );
}

// ── Reparenting ──────────────────────────────────────────────────────────────────────────────

#[test]
fn reparent_reinherits_visibility_and_scale() {
    let mut a = arena();
    let hidden = a.create(FrameKind::Frame, None, None);
    a.set_shown(hidden, false);
    a.set_scale(hidden, 2.0);
    let visible = a.create(FrameKind::Frame, None, None);
    a.set_scale(visible, 3.0);

    let child = a.create(FrameKind::Frame, None, Some(visible));
    assert!(a.frame(child).unwrap().effective_visible);
    assert_eq!(a.frame(child).unwrap().effective_scale, 3.0);

    let changed = a.set_parent(child, Some(hidden));
    assert_eq!(changed, vec![child]);
    assert!(!a.frame(child).unwrap().effective_visible);
    assert_eq!(a.frame(child).unwrap().effective_scale, 2.0);
    assert_eq!(
        a.frame(visible).unwrap().children,
        Vec::<FrameHandle>::new()
    );
    assert_eq!(a.frame(hidden).unwrap().children, vec![child]);
}

#[test]
fn reparent_cycle_is_rejected() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    let changed = a.set_parent(root, Some(child));
    assert!(changed.is_empty());
    assert_eq!(a.frame(root).unwrap().parent, None);
    assert_eq!(a.frame(child).unwrap().parent, Some(root));
}

// ── Named registry ──────────────────────────────────────────────────────────────────────────

#[test]
fn named_registry_is_non_overwriting() {
    let mut a = arena();
    let first = a.create(FrameKind::Frame, Some("MyFrame".into()), None);
    let second = a.create(FrameKind::Frame, Some("MyFrame".into()), None);
    assert_ne!(first, second);
    assert_eq!(a.lookup("MyFrame"), Some(first));
    assert_eq!(a.frame(second).unwrap().name.as_deref(), Some("MyFrame"));
}

#[test]
fn destroy_unpublishes_and_recurses() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, Some("Root".into()), None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    let region = a
        .create_region(child, RegionKind::Texture, DrawLayer::Artwork, 0)
        .unwrap();
    assert_eq!(a.lookup("Root"), Some(root));

    a.destroy(root);
    assert!(a.frame(root).is_none());
    assert!(a.frame(child).is_none(), "subtree destroyed");
    assert!(a.region(region).is_none(), "regions destroyed");
    assert_eq!(a.lookup("Root"), None, "name unpublished");
}

#[test]
fn generational_handle_detects_reuse() {
    let mut a = arena();
    let h = a.create(FrameKind::Frame, None, None);
    a.destroy(h);
    let h2 = a.create(FrameKind::Frame, None, None);
    assert!(a.frame(h).is_none());
    assert!(a.frame(h2).is_some());
}

// ── Alpha ────────────────────────────────────────────────────────────────────────────────────

#[test]
fn set_alpha_overwrites_the_subtree() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    let grandchild = a.create(FrameKind::Frame, None, Some(child));
    a.set_alpha(root, 0.5);
    for h in [root, child, grandchild] {
        assert_eq!(a.frame(h).unwrap().alpha, 0.5);
        assert_eq!(a.frame(h).unwrap().effective_alpha, 0.5);
    }
    // Last write wins: the child's own SetAlpha holds until the parent sets again.
    a.set_alpha(child, 1.0);
    assert_eq!(a.frame(root).unwrap().effective_alpha, 0.5);
    assert_eq!(a.frame(child).unwrap().effective_alpha, 1.0);
    assert_eq!(a.frame(grandchild).unwrap().effective_alpha, 1.0);
    // A frame created under a dimmed parent starts at 1.0.
    let late = a.create(FrameKind::Frame, None, Some(root));
    assert_eq!(a.frame(late).unwrap().effective_alpha, 1.0);
}

// ── The per-kind registries ──────────────────────────────────────────────────────────────────

/// Includes a tooltip destroyed as another frame's child.
#[test]
fn the_tooltip_registry_tracks_live_gametooltips() {
    let mut a = arena();
    assert!(a.tooltip_kinds().is_empty());

    let holder = a.create(FrameKind::Frame, None, None);
    let loose = a.create(FrameKind::GameTooltip, None, None);
    let child = a.create(FrameKind::GameTooltip, None, Some(holder));
    let _plain = a.create(FrameKind::Button, None, None);
    assert_eq!(a.tooltip_kinds(), &[loose, child]);

    a.destroy(holder);
    assert_eq!(a.tooltip_kinds(), &[loose]);

    a.destroy(loose);
    assert!(a.tooltip_kinds().is_empty());
}
