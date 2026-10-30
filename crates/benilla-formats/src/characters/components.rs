//! The character component check (`0x4e9d50`): before a look builds its component, each of skin,
//! face, hair and facial hair is tested against its `CharSections` row's flags for the context,
//! and a choice that fails becomes the `(value % count)`-th choice that passes.
//!
//! The grid is the reference's (`0x4f3dd0`): per race and sex, the base sections 0 to 4 as
//! variations of colours, sized by the largest index seen, holes empty, the last row of a key
//! winning.

use std::collections::HashMap;

use super::CharSections;

/// The grid's base sections: skin, face, facial hair, hair; underwear (4) is held but never tested.
const SECTION_COUNT: u32 = 5;
const SKIN: u8 = 0;
const FACE: u8 = 1;
const FACIAL_HAIR: u8 = 2;
const HAIR: u8 = 3;

/// The Death Knight, whose context selects its own row test (`0x4f3a40`).
const CLASS_DEATH_KNIGHT: u8 = 6;

/// Who a look is checked for, the reference's component context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComponentContext {
    /// Character creation (`0x4e16c6`, `0x4e17c2`, `0x4e2280`): offered rows only.
    Create,
    /// A unit wearing a player's look: a player (`0x71d19c`) or a mirror image (`0x73052b`).
    World,
    /// A `CreatureDisplayInfoExtra` look (`0x71d1a7`): every row passes.
    Npc,
}

/// `0x4f39a0`'s row tests, as `0x4f3a40` picks them from a context and class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Selection {
    Create,
    CreateDeathKnight,
    World,
    WorldDeathKnight,
    Any,
}

impl ComponentContext {
    fn selection(self, class: u8) -> Selection {
        let dk = class == CLASS_DEATH_KNIGHT;
        match (self, dk) {
            (Self::Create, false) => Selection::Create,
            (Self::Create, true) => Selection::CreateDeathKnight,
            (Self::World, false) => Selection::World,
            (Self::World, true) => Selection::WorldDeathKnight,
            (Self::Npc, _) => Selection::Any,
        }
    }
}

impl Selection {
    /// `0x4f39a0`: whether a row's `Flags` pass.
    fn admits(self, flags: u32) -> bool {
        match self {
            Self::Create => flags & 0x1 != 0 && flags & 0xc == 0,
            Self::CreateDeathKnight => flags & 0x1 != 0 && flags & 0x14 != 0 && flags & 0x8 == 0,
            Self::World => flags & 0x3 != 0 && flags & 0xc == 0,
            Self::WorldDeathKnight => flags & 0x3 != 0 && flags & 0x14 != 0 && flags & 0x8 == 0,
            Self::Any => true,
        }
    }
}

/// A look as the component check reads it; `-1` is a picker's miss, which no row answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComponentLook {
    pub race: u8,
    pub sex: u8,
    pub class: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
}

/// One `(race, sex, section)`'s variations of colours, each slot a row's `Flags` or a hole.
type Variations = Vec<Vec<Option<u32>>>;

/// The `CharSections` flags grid ([`CharSections::validate_look`]'s input).
#[derive(Clone, Debug, Default)]
pub(crate) struct ComponentGrid(HashMap<(u8, u8, u8), Variations>);

impl ComponentGrid {
    /// File a row; a section past 4 is skipped (`0x4f3dd0`).
    pub(crate) fn insert(
        &mut self,
        race: u8,
        sex: u8,
        section: u32,
        var: u8,
        color: u8,
        flags: u32,
    ) {
        if section >= SECTION_COUNT {
            return;
        }
        let vars = self.0.entry((race, sex, section as u8)).or_default();
        let (var, color) = (usize::from(var), usize::from(color));
        if vars.len() <= var {
            vars.resize(var + 1, Vec::new());
        }
        let colors = &mut vars[var];
        if colors.len() <= color {
            colors.resize(color + 1, None);
        }
        colors[color] = Some(flags);
    }

    fn variations(&self, race: u8, sex: u8, section: u8) -> &[Vec<Option<u32>>] {
        self.0.get(&(race, sex, section)).map_or(&[], Vec::as_slice)
    }

    /// `0x4f3ba0`: `None` when the indices fall outside the grid (its `found` false), else the
    /// slot, a hole or a row's flags.
    fn slot(&self, look: &Look, section: u8, var: i32, color: i32) -> Option<Option<u32>> {
        let vars = self.variations(look.race, look.sex, section);
        let colors = vars.get(usize::try_from(var).ok()?)?;
        colors.get(usize::try_from(color).ok()?).copied()
    }

    /// Whether the slot holds a row the selection admits.
    fn admits(&self, look: &Look, section: u8, var: i32, color: i32, sel: Selection) -> bool {
        self.slot(look, section, var, color)
            .flatten()
            .is_some_and(|f| sel.admits(f))
    }

    /// The indices `0..n` whose slot admits, varying the variation (`colour` fixed) or the colour.
    fn admitted<'a>(
        &'a self,
        look: &Look,
        section: u8,
        n: usize,
        slot_of: impl Fn(i32) -> (i32, i32) + 'a,
        sel: Selection,
    ) -> impl Iterator<Item = i32> + 'a {
        let look = *look;
        (0..n as i32).filter(move |&i| {
            let (var, color) = slot_of(i);
            self.admits(&look, section, var, color, sel)
        })
    }
}

/// The look in the reference's signed dwords.
#[derive(Clone, Copy, Debug)]
struct Look {
    race: u8,
    sex: u8,
    skin: i32,
    face: i32,
    hair_style: i32,
    hair_color: i32,
    facial_hair: i32,
}

/// The `n`-th admitted index, `-1` past the last (the pickers `0x4e7e90` … `0x4e8050`).
fn nth(mut admitted: impl Iterator<Item = i32>, n: i32) -> i32 {
    usize::try_from(n)
        .ok()
        .and_then(|n| admitted.nth(n))
        .unwrap_or(-1)
}

impl CharSections {
    /// Run the component check (`0x4e9d50`) on a look for `context`. `facial_styles` is the race
    /// and sex's `CharacterFacialHairStyles` row count (`0x4f41b0`), the facial-hair range when
    /// `CharSections` has no facial rows.
    pub fn validate_look(
        &self,
        look: ComponentLook,
        context: ComponentContext,
        facial_styles: u32,
    ) -> ComponentLook {
        self.components.validate(look, context, facial_styles)
    }
}

impl ComponentGrid {
    fn validate(
        &self,
        look: ComponentLook,
        context: ComponentContext,
        facial_styles: u32,
    ) -> ComponentLook {
        let sel = context.selection(look.class);
        let g = self;
        let mut l = Look {
            race: look.race,
            sex: look.sex,
            skin: i32::from(look.skin),
            face: i32::from(look.face),
            hair_style: i32::from(look.hair_style),
            hair_color: i32::from(look.hair_color),
            facial_hair: i32::from(look.facial_hair),
        };

        // Skin (`0x4f3c10`): variation 0's colours.
        let skins = g
            .variations(l.race, l.sex, SKIN)
            .first()
            .map_or(0, Vec::len);
        if !g.admits(&l, SKIN, 0, l.skin, sel) {
            let n = g.admitted(&l, SKIN, skins, |i| (0, i), sel).count() as i32;
            l.skin = if n <= 0 {
                0
            } else {
                nth(g.admitted(&l, SKIN, skins, |i| (0, i), sel), l.skin % n)
            };
        }

        // Face (`0x4f3ce0`): the variations at this skin colour.
        let faces = g.variations(l.race, l.sex, FACE).len();
        if !g.admits(&l, FACE, l.face, l.skin, sel) {
            let skin = l.skin;
            let n = g.admitted(&l, FACE, faces, |i| (i, skin), sel).count() as i32;
            l.face = if n <= 0 {
                0
            } else {
                nth(g.admitted(&l, FACE, faces, |i| (i, skin), sel), l.face % n)
            };
        }

        // Hair (`0x4f3c70`): the styles at this colour, else a style with any admitted colour.
        let styles = g.variations(l.race, l.sex, HAIR);
        if !g.admits(&l, HAIR, l.hair_style, l.hair_color, sel) {
            let color = l.hair_color;
            let n = g
                .admitted(&l, HAIR, styles.len(), |i| (i, color), sel)
                .count() as i32;
            if n > 0 {
                l.hair_style = nth(
                    g.admitted(&l, HAIR, styles.len(), |i| (i, color), sel),
                    l.hair_style % n,
                );
            } else {
                // `0x4e9ea3`: each style's admitted colour count.
                let colors: Vec<i32> = styles
                    .iter()
                    .enumerate()
                    .map(|(s, c)| {
                        let s = s as i32;
                        g.admitted(&l, HAIR, c.len(), |i| (s, i), sel).count() as i32
                    })
                    .collect();
                let valid = colors.iter().filter(|&&c| c > 0).count() as i32;
                if valid <= 0 {
                    l.hair_color = 0;
                    l.hair_style = 0;
                } else {
                    // The remainder is compared with the raw style index, not the n-th valid one.
                    let r = l.hair_style % valid;
                    if let Some(s) =
                        (0..colors.len() as i32).find(|&s| colors[s as usize] > 0 && s == r)
                    {
                        l.hair_style = s;
                    }
                    // The reference divides by the kept style's count even when no style matched;
                    // a zero or out-of-range count keeps the colour here instead of faulting.
                    if let Some(&n) = usize::try_from(l.hair_style)
                        .ok()
                        .and_then(|s| colors.get(s))
                        .filter(|&&n| n > 0)
                    {
                        let style = l.hair_style;
                        let len = styles[style as usize].len();
                        l.hair_color = nth(
                            g.admitted(&l, HAIR, len, |i| (style, i), sel),
                            l.hair_color % n,
                        );
                    }
                }
            }
        }

        // Facial hair (`0x4f3d50`), at the hair colour.
        if !facial_valid(g, &l, facial_styles, sel) {
            let n = facial_count(g, &l, facial_styles, sel);
            l.facial_hair = if n <= 0 {
                0
            } else {
                nth_facial(g, &l, facial_styles, l.facial_hair % n, sel)
            };
        }

        let byte = |v: i32| v as u8;
        ComponentLook {
            skin: byte(l.skin),
            face: byte(l.face),
            hair_style: byte(l.hair_style),
            hair_color: byte(l.hair_color),
            facial_hair: byte(l.facial_hair),
            ..look
        }
    }
}

/// `0x4f3d50`: past the style count fails; a variation or colour outside the grid passes; a hole
/// fails; a row answers by its flags.
fn facial_valid(g: &ComponentGrid, l: &Look, styles: u32, sel: Selection) -> bool {
    let var = l.facial_hair;
    if var < 0 || var as u32 > styles {
        return false;
    }
    match g.slot(l, FACIAL_HAIR, var, l.hair_color) {
        None => true,
        Some(slot) => slot.is_some_and(|f| sel.admits(f)),
    }
}

/// `0x4e7df0`: the admitted variations at the hair colour, or the style count without facial rows.
fn facial_count(g: &ComponentGrid, l: &Look, styles: u32, sel: Selection) -> i32 {
    let vars = g.variations(l.race, l.sex, FACIAL_HAIR).len();
    if vars == 0 {
        return styles as i32;
    }
    let color = l.hair_color;
    g.admitted(l, FACIAL_HAIR, vars, |i| (i, color), sel)
        .count() as i32
}

/// `0x4e80e0`: with the current choice outside the grid the index stands if under the style
/// count; else the n-th admitted variation, scanning only the first `count` variations.
fn nth_facial(g: &ComponentGrid, l: &Look, styles: u32, index: i32, sel: Selection) -> i32 {
    if g.slot(l, FACIAL_HAIR, l.facial_hair, l.hair_color)
        .is_none()
    {
        return if index < styles as i32 { index } else { -1 };
    }
    let count = facial_count(g, l, styles, sel);
    if count <= 0 {
        return -1;
    }
    let color = l.hair_color;
    nth(
        g.admitted(l, FACIAL_HAIR, count as usize, |i| (i, color), sel),
        index,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const NORMAL: u32 = 0x1;
    const DK_ONLY: u32 = 0x5;
    const NPC_ONLY: u32 = 0x8;

    fn look(class: u8, skin: u8, face: u8, hair_style: u8, hair_color: u8) -> ComponentLook {
        ComponentLook {
            race: 1,
            sex: 0,
            class,
            skin,
            face,
            hair_style,
            hair_color,
            facial_hair: 0,
        }
    }

    /// Two skins, the second NPC-only; three faces at skin 0, the middle one Death Knight only;
    /// two hair styles at colour 0, the first DK-only.
    fn sections() -> ComponentGrid {
        let mut g = ComponentGrid::default();
        g.insert(1, 0, 0, 0, 0, NORMAL);
        g.insert(1, 0, 0, 0, 1, NPC_ONLY);
        for (face, flags) in [(0, NORMAL), (1, DK_ONLY), (2, NORMAL)] {
            g.insert(1, 0, 1, face, 0, flags);
        }
        g.insert(1, 0, 3, 0, 0, DK_ONLY);
        g.insert(1, 0, 3, 1, 0, NORMAL);
        g
    }

    #[test]
    fn the_selections_follow_the_context_and_class() {
        assert!(Selection::World.admits(0x1) && Selection::World.admits(0x2));
        assert!(!Selection::World.admits(DK_ONLY), "0x4 fails a non-DK");
        assert!(!Selection::World.admits(NPC_ONLY));
        assert!(Selection::WorldDeathKnight.admits(DK_ONLY));
        assert!(
            !Selection::WorldDeathKnight.admits(0x1),
            "a DK needs 0x4 or 0x10"
        );
        assert!(Selection::WorldDeathKnight.admits(0x11));
        assert!(!Selection::Create.admits(0x2), "creation wants 0x1");
        assert!(Selection::Any.admits(0xff));
        assert_eq!(
            ComponentContext::World.selection(6),
            Selection::WorldDeathKnight
        );
        assert_eq!(ComponentContext::World.selection(1), Selection::World);
        assert_eq!(ComponentContext::Npc.selection(6), Selection::Any);
        assert_eq!(
            ComponentContext::Create.selection(6),
            Selection::CreateDeathKnight
        );
    }

    /// A non-DK on the DK-only face 1 takes the `(1 % 2)`-th admitted face, face 2.
    #[test]
    fn a_dk_only_face_on_a_non_dk_is_the_nth_valid_face() {
        let cs = sections();
        let out = cs.validate(look(1, 0, 1, 1, 0), ComponentContext::World, 0);
        assert_eq!(out.face, 2);
        assert_eq!((out.skin, out.hair_style), (0, 1), "valid choices stand");
    }

    /// A Death Knight's DK face stands; context 2 accepts every row, the NPC-only skin included,
    /// though a face still needs a row at its skin.
    #[test]
    fn a_dk_and_an_npc_keep_their_choices() {
        let cs = sections();
        let dk = look(6, 0, 1, 0, 0);
        assert_eq!(cs.validate(dk, ComponentContext::World, 0).face, 1);
        let npc = look(1, 1, 0, 0, 0);
        assert_eq!(cs.validate(npc, ComponentContext::Npc, 0).skin, 1);
        let dk_face = look(1, 0, 1, 0, 0);
        assert_eq!(cs.validate(dk_face, ComponentContext::Npc, 0), dk_face);
    }

    /// The NPC-only skin 1 falls to skin 0; the DK-only style 0 to style 1.
    #[test]
    fn a_failing_skin_and_hair_style_are_replaced() {
        let cs = sections();
        let out = cs.validate(look(1, 1, 0, 0, 0), ComponentContext::World, 0);
        assert_eq!(out.skin, 0);
        assert_eq!(out.hair_style, 1);
    }

    /// A colour no style admits falls back to a style with admitted colours, then its colour.
    #[test]
    fn a_hair_colour_with_no_style_takes_a_valid_style_and_colour() {
        let cs = sections();
        let out = cs.validate(look(1, 0, 0, 1, 7), ComponentContext::World, 0);
        assert_eq!((out.hair_style, out.hair_color), (1, 0));
    }

    /// Without facial rows, the facial-hair range is the style count: past it falls to
    /// `value % count`.
    #[test]
    fn facial_hair_without_rows_wraps_by_the_style_count() {
        let cs = sections();
        let mut l = look(1, 0, 0, 1, 0);
        l.facial_hair = 3;
        assert_eq!(cs.validate(l, ComponentContext::World, 4).facial_hair, 3);
        l.facial_hair = 6;
        assert_eq!(cs.validate(l, ComponentContext::World, 4).facial_hair, 2);
    }

    /// On the shipped file: the rows a non-DK player refuses per section, and a DK-only face
    /// (flags 0x5) that becomes the n-th valid face with no eye glow, kept on a Death Knight.
    #[test]
    fn dk_only_faces_on_the_shipped_file() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cs = CharSections::load(&mut chain).expect("CharSections");
        let refused = |section: u8| {
            cs.components
                .0
                .iter()
                .filter(|((_, _, s), _)| *s == section)
                .flat_map(|(_, vars)| vars.iter().flatten().flatten())
                .filter(|&&f| !Selection::World.admits(f))
                .count()
        };
        eprintln!(
            "refused: skin {}, face {}, hair {}",
            refused(SKIN),
            refused(FACE),
            refused(HAIR)
        );
        assert!(refused(FACE) > 1000, "most DK faces are refused");

        // The first DK-only face of a Human male at skin 0.
        let faces = cs.components.variations(1, 0, FACE);
        let dk_face = (0..faces.len())
            .find(|&f| faces[f].first().copied().flatten() == Some(DK_ONLY))
            .expect("a DK-only Human male face at skin 0") as u8;
        let mut l = look(1, 0, dk_face, 0, 0);
        let out = cs.validate_look(l, ComponentContext::World, 9);
        assert_ne!(out.face, dk_face, "a non-DK never wears it");
        assert!(!cs.eye_glow(1, 0, 1, out.face, out.skin));
        l.class = 6;
        let dk = cs.validate_look(l, ComponentContext::World, 9);
        assert_eq!(dk.face, dk_face, "a Death Knight keeps it");
        assert!(cs.eye_glow(1, 0, 6, dk.face, dk.skin));
    }
}
