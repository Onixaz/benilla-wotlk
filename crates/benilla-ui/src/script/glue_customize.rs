//! The create screen's appearance dials as `CCharacterComponent` walks them: each dial holds a
//! `CharSections` variation or colour index, and a step or a draw only lands on rows the class's
//! flag selection admits (`0x4f39a0`), the face keyed by the skin, the hair style and colour by
//! each other, the facial hair by the hair colour.

/// `CharSections` `BaseSection` values, `COMPONENT_VARIATIONS`' order.
const SKIN: usize = 0;
const FACE: usize = 1;
const FACIAL_HAIR: usize = 2;
const HAIR: usize = 3;
const UNDERWEAR: usize = 4;

/// One (race, sex)'s `CharSections` grid as `BuildComponentArray` (`0x4f3dd0`) lays it out: per
/// base section, `[variation][color]`, each slot the `Flags` of the row there or `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlueCustomizeGrid {
    pub sections: [Vec<Vec<Option<u32>>>; 5],
    /// `CharacterFacialHairStyles.dbc`'s rows for the race and sex (`0x4f41b0`).
    pub facial_hair_styles: u32,
}

/// `ComponentFlagsMatch` (`0x4f39a0`) at the create screen's selections, 0 and, for class 6, 1
/// (`0x4f3a40` with context 0): a creatable row, and for a Death Knight one flagged `0x4` or
/// `0x10`; never an NPC's (`0x8`), and `0x4` only for a Death Knight.
fn flags_match(flags: u32, death_knight: bool) -> bool {
    if death_knight {
        flags & 0x1 != 0 && flags & 0x14 != 0 && flags & 0x8 == 0
    } else {
        flags & 0x1 != 0 && flags & 0xC == 0
    }
}

/// The five dials, `CharacterPreferences`' section indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Prefs {
    skin: i32,
    face: i32,
    hair_style: i32,
    hair_color: i32,
    facial_hair: i32,
}

impl Prefs {
    fn from_dials(d: [u8; 5]) -> Self {
        Prefs {
            skin: d[0].into(),
            face: d[1].into(),
            hair_style: d[2].into(),
            hair_color: d[3].into(),
            facial_hair: d[4].into(),
        }
    }

    fn dials(self) -> [u8; 5] {
        [
            self.skin,
            self.face,
            self.hair_style,
            self.hair_color,
            self.facial_hair,
        ]
        .map(|v| v.clamp(0, 255) as u8)
    }
}

/// The dials of one character of a (race, sex) and class, stepped as the component steps them.
pub(super) struct Customizer<'a> {
    grid: &'a GlueCustomizeGrid,
    death_knight: bool,
    p: Prefs,
}

impl<'a> Customizer<'a> {
    pub(super) fn new(grid: &'a GlueCustomizeGrid, class: u8, dials: [u8; 5]) -> Self {
        Customizer {
            grid,
            death_knight: class == 6,
            p: Prefs::from_dials(dials),
        }
    }

    pub(super) fn dials(&self) -> [u8; 5] {
        self.p.dials()
    }

    /// `ComponentGetNumVariations` (`0x4f3ae0`).
    fn num_variations(&self, ty: usize) -> i32 {
        self.grid.sections[ty].len() as i32
    }

    /// `ComponentGetNumColors` (`0x4f3b10`).
    fn num_colors(&self, ty: usize, var: i32) -> i32 {
        usize::try_from(var)
            .ok()
            .and_then(|v| self.grid.sections[ty].get(v))
            .map_or(0, |c| c.len() as i32)
    }

    /// `ComponentValidateBase` (`0x4f3b50`): the slot is inside the grid, filled or not.
    fn in_grid(&self, ty: usize, var: i32, color: i32) -> bool {
        var >= 0 && color >= 0 && color < self.num_colors(ty, var)
    }

    /// `ComponentGetSectionsRecord` (`0x4f3ba0`): the row's flags.
    fn row(&self, ty: usize, var: i32, color: i32) -> Option<u32> {
        if !self.in_grid(ty, var, color) {
            return None;
        }
        self.grid.sections[ty][var as usize][color as usize]
    }

    fn admits(&self, ty: usize, var: i32, color: i32) -> bool {
        self.row(ty, var, color)
            .is_some_and(|f| flags_match(f, self.death_knight))
    }

    /// The skin, face and underwear rows a (face, skin) pair needs, as the skin and face walkers
    /// test them.
    fn face_on_skin(&self, face: i32, skin: i32) -> bool {
        self.admits(FACE, face, skin)
            && self.admits(SKIN, 0, skin)
            && self.admits(UNDERWEAR, 0, skin)
    }

    /// The `n`th admitted slot: a colour of variation `fixed`, or with `by_var` a variation at
    /// colour `fixed` (`0x4e7e90`, `0x4e7f20`, `0x4e7fb0`, `0x4e8050`); -1 past the last.
    fn nth(&self, ty: usize, fixed: i32, by_var: bool, n: i32) -> i32 {
        let count = if by_var {
            self.num_variations(ty)
        } else {
            self.num_colors(ty, fixed)
        };
        let mut found = 0;
        for i in 0..count {
            let (var, color) = if by_var { (i, fixed) } else { (fixed, i) };
            if self.admits(ty, var, color) {
                if found == n {
                    return if by_var { var } else { color };
                }
                found += 1;
            }
        }
        -1
    }

    fn count(&self, ty: usize, fixed: i32, by_var: bool) -> i32 {
        let count = if by_var {
            self.num_variations(ty)
        } else {
            self.num_colors(ty, fixed)
        };
        (0..count)
            .filter(|&i| {
                let (var, color) = if by_var { (i, fixed) } else { (fixed, i) };
                self.admits(ty, var, color)
            })
            .count() as i32
    }

    /// `GetNumSkins` (`0x4e7b80`).
    fn num_skins(&self) -> i32 {
        self.count(SKIN, 0, false)
    }

    /// `GetNumFacesForSkin` (`0x4e7d50`).
    fn num_faces(&self, skin: i32) -> i32 {
        self.count(FACE, skin, true)
    }

    /// `GetNumHairStylesForColor` (`0x4e7c10`).
    fn num_hair_styles(&self, color: i32) -> i32 {
        self.count(HAIR, color, true)
    }

    /// `GetNumHairColorsForStyle` (`0x4e7cb0`).
    fn num_hair_colors(&self, style: i32) -> i32 {
        self.count(HAIR, style, false)
    }

    /// `GetNumFacialFeaturesForHairColor` (`0x4e7df0`): the styles table's count where the race
    /// keeps no facial-hair rows.
    fn num_facial(&self, color: i32) -> i32 {
        if self.num_variations(FACIAL_HAIR) == 0 {
            return self.grid.facial_hair_styles as i32;
        }
        self.count(FACIAL_HAIR, color, true)
    }

    /// `GetNthFacialFeatureIndex` (`0x4e80e0`), whose scan runs to the admitted count, not the
    /// grid's width.
    fn nth_facial(&self, color: i32, current: i32, n: i32) -> i32 {
        if !self.in_grid(FACIAL_HAIR, current, color) {
            return if n < self.grid.facial_hair_styles as i32 {
                n
            } else {
                -1
            };
        }
        let mut found = 0;
        for i in 0..self.num_facial(color) {
            if self.admits(FACIAL_HAIR, i, color) {
                if found == n {
                    return i;
                }
                found += 1;
            }
        }
        -1
    }

    /// `SetSkinColor` (`0x4ea6b0`) takes any index.
    fn set_skin(&mut self, skin: i32) {
        self.p.skin = skin;
    }

    /// `SetFace` (`0x4ea490`): refused on an NPC skin (`0x8`) or a slot off the grid.
    fn set_face(&mut self, face: i32) {
        if self.row(SKIN, 0, self.p.skin).is_some_and(|f| f & 0x8 != 0) {
            return;
        }
        if self.in_grid(FACE, face, self.p.skin) {
            self.p.face = face;
        }
    }

    /// `SetHairStyle` (`0x4ea3e0`): the style must have the current colour's slot.
    fn set_hair_style(&mut self, style: i32) {
        if self.in_grid(HAIR, style, self.p.hair_color) {
            self.p.hair_style = style;
        }
    }

    /// `SetHairColor` (`0x4ea2f0`): the current style must have the colour's slot.
    fn set_hair_color(&mut self, color: i32) {
        if self.in_grid(HAIR, self.p.hair_style, color) {
            self.p.hair_color = color;
        }
    }

    /// `SetBeardStyle` (`0x4ea590`): `0..=` the styles table's count.
    fn set_beard(&mut self, style: i32) {
        if (0..=self.grid.facial_hair_styles as i32).contains(&style) {
            self.p.facial_hair = style;
        }
    }

    /// One step of `n` slots from `from`, wrapping.
    fn wrap(from: i32, step: i32, n: i32) -> i32 {
        (from + step).rem_euclid(n)
    }

    /// `SetNextSkin` (`0x4eb150`) and `SetPrevSkin` (`0x4eb290`): the next skin whose row, the
    /// current face's row on it and its underwear are all admitted.
    pub(super) fn step_skin(&mut self, step: i32) -> bool {
        let n = self.num_colors(SKIN, 0);
        if n <= 0 {
            return false;
        }
        let mut skin = Self::wrap(self.p.skin, step, n);
        if skin == self.p.skin {
            return false;
        }
        while !self.face_on_skin(self.p.face, skin) {
            skin = Self::wrap(skin, step, n);
            if skin == self.p.skin {
                return false;
            }
        }
        self.set_skin(skin);
        true
    }

    /// `SetNextFace` (`0x4eb710`) and `SetPrevFace` (`0x4eb990`): the next face with an admitted
    /// skin at or above `skin_from` (`m_prevSkinIndex`), the scan not wrapping (`0x4eb7a6`); the
    /// skin moves to the first such where the face is not admitted on the current one.
    pub(super) fn step_face(&mut self, step: i32, skin_from: i32) -> bool {
        let n = self.num_variations(FACE);
        if n <= 0 {
            return false;
        }
        let mut face = Self::wrap(self.p.face, step, n);
        if face == self.p.face {
            return false;
        }
        let skin = loop {
            let colors = self.num_colors(FACE, face);
            let mut i = 0;
            let mut hit = None;
            while i < colors {
                let skin = (i + skin_from).rem_euclid(colors);
                if self.face_on_skin(face, skin) {
                    hit = Some(skin);
                    break;
                }
                i = skin + 1;
            }
            if let Some(skin) = hit {
                break skin;
            }
            face = Self::wrap(face, step, n);
            if face == self.p.face {
                return false;
            }
        };
        if !self.face_on_skin(face, self.p.skin) {
            self.set_skin(skin);
        }
        self.set_face(face);
        true
    }

    /// `SetNextHairStyle` (`0x4f0490`) and `SetPrevHairStyle` (`0x4f0630`): the next style with an
    /// admitted colour, keeping the current colour where the style has it, else taking the
    /// style's first and the first facial hair that colour admits.
    pub(super) fn step_hair_style(&mut self, step: i32) -> bool {
        let n = self.num_variations(HAIR);
        if n <= 0 {
            return false;
        }
        let mut style = Self::wrap(self.p.hair_style, step, n);
        if style == self.p.hair_style {
            return false;
        }
        let color = loop {
            let first = (0..self.num_colors(HAIR, style)).find(|&c| self.admits(HAIR, style, c));
            if let Some(c) = first {
                break c;
            }
            style = Self::wrap(style, step, n);
            if style == self.p.hair_style {
                return false;
            }
        };
        if self.admits(HAIR, style, self.p.hair_color) {
            self.set_hair_style(style);
            return true;
        }
        self.set_hair_color(color);
        self.set_hair_style(style);
        let facial = self.nth_facial(self.p.hair_color, self.p.facial_hair, 0);
        if facial >= 0 {
            self.set_beard(facial);
        }
        true
    }

    /// `SetNextHairColor` (`0x4eb500`) and `SetPrevHairColor` (`0x4eb5c0`): the style's next
    /// admitted colour.
    pub(super) fn step_hair_color(&mut self, step: i32) -> bool {
        let n = self.num_colors(HAIR, self.p.hair_style);
        if n <= 0 {
            return false;
        }
        let mut color = Self::wrap(self.p.hair_color, step, n);
        if color == self.p.hair_color {
            return false;
        }
        while !self.admits(HAIR, self.p.hair_style, color) {
            color = Self::wrap(color, step, n);
            if color == self.p.hair_color {
                return false;
            }
        }
        self.set_hair_color(color);
        true
    }

    /// `SetNextFacialFeature` (`0x4ebca0`) and `SetPrevFacialFeature` (`0x4ebe80`): off the
    /// facial-hair rows, a plain step through the styles table; on them, the next style with an
    /// admitted colour, keeping the hair colour where it has it.
    pub(super) fn step_facial(&mut self, step: i32) -> bool {
        if !self.in_grid(FACIAL_HAIR, self.p.facial_hair, self.p.hair_color) {
            let count = self.grid.facial_hair_styles as i32;
            let next = self.p.facial_hair + step;
            self.set_beard(if next >= count {
                0
            } else if next < 0 {
                count - 1
            } else {
                next
            });
            return true;
        }
        let n = self.num_variations(FACIAL_HAIR);
        if n <= 0 {
            return false;
        }
        let mut style = Self::wrap(self.p.facial_hair, step, n);
        if style == self.p.facial_hair {
            return false;
        }
        let color = loop {
            let first = (0..self.num_colors(FACIAL_HAIR, style))
                .find(|&c| self.admits(FACIAL_HAIR, style, c));
            if let Some(c) = first {
                break c;
            }
            style = Self::wrap(style, step, n);
            if style == self.p.facial_hair {
                return false;
            }
        };
        if !self.admits(FACIAL_HAIR, style, self.p.hair_color) {
            self.set_hair_color(color);
        }
        self.set_beard(style);
        true
    }

    /// `SetRandomSkin` (`0x4eb3e0`).
    pub(super) fn random_skin(&mut self, roll: &mut impl FnMut(i32) -> i32) {
        let n = self.num_skins();
        let skin = self.nth(SKIN, 0, false, if n > 0 { roll(n) } else { 0 });
        if skin >= 0 {
            self.set_skin(skin);
        }
    }

    /// `SetRandomFace` (`0x4ebc10`).
    pub(super) fn random_face(&mut self, roll: &mut impl FnMut(i32) -> i32) {
        let n = self.num_faces(self.p.skin);
        let face = self.nth(FACE, self.p.skin, true, if n > 0 { roll(n) } else { 0 });
        if face >= 0 {
            self.set_face(face);
        }
    }

    /// `SetRandomHairStyle` (`0x4eb470`).
    pub(super) fn random_hair_style(&mut self, roll: &mut impl FnMut(i32) -> i32) {
        let n = self.num_hair_styles(self.p.hair_color);
        let style = self.nth(
            HAIR,
            self.p.hair_color,
            true,
            if n > 0 { roll(n) } else { 0 },
        );
        if style >= 0 {
            self.set_hair_style(style);
        }
    }

    /// `SetRandomHairColor` (`0x4eb680`).
    pub(super) fn random_hair_color(&mut self, roll: &mut impl FnMut(i32) -> i32) {
        let n = self.num_hair_colors(self.p.hair_style);
        let color = self.nth(
            HAIR,
            self.p.hair_style,
            false,
            if n > 0 { roll(n) } else { 0 },
        );
        if color >= 0 {
            self.set_hair_color(color);
        }
    }

    /// `SetRandomFacialFeature` (`0x4ec050`).
    pub(super) fn random_facial(&mut self, roll: &mut impl FnMut(i32) -> i32) {
        let n = self.num_facial(self.p.hair_color);
        let pick = if n > 0 { roll(n) } else { 0 };
        let style = self.nth_facial(self.p.hair_color, self.p.facial_hair, pick);
        if style >= 0 {
            self.set_beard(style);
        }
    }

    /// `ValidateComponentData` (`0x4e9d50`): each dial the class does not admit becomes the
    /// admitted one its old index picks, modulo the count.
    pub(super) fn validate(&mut self) {
        let v = self;
        if !v.admits(SKIN, 0, v.p.skin) {
            let n = v.num_skins();
            v.p.skin = if n <= 0 {
                0
            } else {
                v.nth(SKIN, 0, false, v.p.skin % n)
            };
        }
        if !v.admits(FACE, v.p.face, v.p.skin) {
            let n = v.num_faces(v.p.skin);
            v.p.face = if n <= 0 {
                0
            } else {
                v.nth(FACE, v.p.skin, true, v.p.face % n)
            };
        }
        if !v.admits(HAIR, v.p.hair_style, v.p.hair_color) {
            let n = v.num_hair_styles(v.p.hair_color);
            if n > 0 {
                v.p.hair_style = v.nth(HAIR, v.p.hair_color, true, v.p.hair_style % n);
            } else {
                let colors: Vec<i32> = (0..v.num_variations(HAIR))
                    .map(|s| v.num_hair_colors(s))
                    .collect();
                let valid = colors.iter().filter(|&&c| c > 0).count() as i32;
                if valid <= 0 {
                    v.p.hair_color = 0;
                    v.p.hair_style = 0;
                } else {
                    if let Some(s) = (0..colors.len() as i32)
                        .find(|&s| colors[s as usize] > 0 && v.p.hair_style % valid == s)
                    {
                        v.p.hair_style = s;
                    }
                    let n = colors.get(v.p.hair_style as usize).copied().unwrap_or(0);
                    if n > 0 {
                        v.p.hair_color = v.nth(HAIR, v.p.hair_style, false, v.p.hair_color % n);
                    }
                }
            }
        }
        if !v.facial_valid() {
            let n = v.num_facial(v.p.hair_color);
            v.p.facial_hair = if n <= 0 {
                0
            } else {
                v.nth_facial(v.p.hair_color, v.p.facial_hair, v.p.facial_hair % n)
            };
        }
    }

    /// `ComponentValidateFacialFeature` (`0x4f3d50`): inside the styles table, and admitted where
    /// the race keeps a facial-hair row for the style and colour.
    fn facial_valid(&self) -> bool {
        let (style, color) = (self.p.facial_hair, self.p.hair_color);
        if style < 0 || style > self.grid.facial_hair_styles as i32 {
            return false;
        }
        if !self.in_grid(FACIAL_HAIR, style, color) {
            return true;
        }
        self.admits(FACIAL_HAIR, style, color)
    }
}
