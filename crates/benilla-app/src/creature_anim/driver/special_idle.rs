//! The special idle latch (`[unit+0xfa4]`): a stage-0 kit's anim, held as the unit's idle while
//! it plays.

/// Whether the latch on `latched` survives (`0x73bc81`): the reference clears it at an animation
/// end once neither the key bone nor bone 0 plays the latched id, or when that id itself ends.
/// Each bone is `(armed id, still playing)`; a finished clamp clip has ended.
pub(super) fn holds(
    latched: u16,
    key_bone: Option<(u16, bool)>,
    base: Option<(u16, bool)>,
) -> bool {
    let plays = |bone: Option<(u16, bool)>| bone.is_some_and(|(id, live)| live && id == latched);
    plays(key_bone) || plays(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An eat loop on bone 0 holds the latch; a finished one-shot, another clip or no clip drops
    /// it, and the key bone counts as bone 0 does.
    #[test]
    fn the_latch_holds_while_either_bone_plays_its_anim() {
        assert!(holds(61, None, Some((61, true))));
        assert!(holds(61, Some((61, true)), Some((0, true))));
        assert!(
            !holds(61, None, Some((61, false))),
            "the latched anim ended"
        );
        assert!(
            !holds(61, Some((5, true)), Some((0, true))),
            "neither bone plays it"
        );
        assert!(!holds(61, None, None));
    }
}
