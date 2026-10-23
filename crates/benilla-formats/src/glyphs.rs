//! `GlyphProperties.dbc` and `GlyphSlot.dbc`: the glyphs a glyph spell inscribes (its effect 74's
//! `EffectMiscValue`) and the sockets `PLAYER_FIELD_GLYPH_SLOTS_1` names.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const GLYPH_PROPERTIES: &str = "DBFilesClient\\GlyphProperties.dbc";
const GLYPH_SLOT: &str = "DBFilesClient\\GlyphSlot.dbc";

/// One `GlyphProperties.dbc` row: the spell it grants (`+4`, `0x5b7392`), the socket type it fits
/// (`+8`, compared to the slot's `+4` at `0x80c53c`) and its `SpellIcon.dbc` icon (`+0xc`,
/// `0x5b73a1`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GlyphProperty {
    pub id: u32,
    pub spell: u32,
    pub slot_type: u32,
    /// The icon's texture path; `None` when the row names no `SpellIcon.dbc` row.
    pub icon: Option<String>,
}

/// One `GlyphSlot.dbc` row: its type (`+4`, 0 major, 1 minor, `0x5b7352`) and the
/// `GLYPH_SLOT_TOOLTIP<n>` key a locked socket's tooltip reads (`+8`, `0x622d6c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GlyphSlot {
    pub id: u32,
    pub slot_type: u32,
    pub tooltip: u32,
}

/// Both tables, by id.
#[derive(Debug, Clone, Default)]
pub struct GlyphCatalog {
    glyphs: HashMap<u32, GlyphProperty>,
    slots: HashMap<u32, GlyphSlot>,
}

impl GlyphCatalog {
    /// Build from rows, for tests and fixtures.
    pub fn from_rows(
        glyphs: impl IntoIterator<Item = GlyphProperty>,
        slots: impl IntoIterator<Item = GlyphSlot>,
    ) -> Self {
        Self {
            glyphs: glyphs.into_iter().map(|g| (g.id, g)).collect(),
            slots: slots.into_iter().map(|s| (s.id, s)).collect(),
        }
    }

    pub fn glyph(&self, id: u32) -> Option<&GlyphProperty> {
        self.glyphs.get(&id)
    }

    pub fn slot(&self, id: u32) -> Option<&GlyphSlot> {
        self.slots.get(&id)
    }

    pub fn glyphs(&self) -> impl Iterator<Item = &GlyphProperty> {
        self.glyphs.values()
    }

    pub fn slots(&self) -> impl Iterator<Item = &GlyphSlot> {
        self.slots.values()
    }

    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }
}

pub(crate) fn glyph_properties_schema() -> Schema {
    let mut s = Schema::new("GlyphProperties");
    for name in ["ID", "SpellID", "GlyphSlotFlags", "SpellIconID"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

pub(crate) fn glyph_slot_schema() -> Schema {
    let mut s = Schema::new("GlyphSlot");
    for name in ["ID", "Type", "Tooltip"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// Load both tables off the chain, icons resolved through `SpellIcon.dbc`.
pub fn load_glyph_catalog(chain: &mut Chain) -> Result<GlyphCatalog> {
    let icons = crate::dbc::load_spell_icon_map(chain)?;
    let bytes = chain
        .read_file(GLYPH_PROPERTIES)
        .with_context(|| format!("reading {GLYPH_PROPERTIES}"))?;
    let rs = parse(&bytes, glyph_properties_schema(), "GlyphProperties")?;
    let glyphs: Vec<_> = rs
        .records()
        .iter()
        .filter_map(|r| {
            Some(GlyphProperty {
                id: u32_at(r, 0)?,
                spell: u32_at(r, 1)?,
                slot_type: u32_at(r, 2)?,
                icon: icons.get(&u32_at(r, 3)?).cloned(),
            })
        })
        .collect();
    let bytes = chain
        .read_file(GLYPH_SLOT)
        .with_context(|| format!("reading {GLYPH_SLOT}"))?;
    let rs = parse(&bytes, glyph_slot_schema(), "GlyphSlot")?;
    let slots: Vec<_> = rs
        .records()
        .iter()
        .filter_map(|r| {
            Some(GlyphSlot {
                id: u32_at(r, 0)?,
                slot_type: u32_at(r, 1)?,
                tooltip: u32_at(r, 2)?,
            })
        })
        .collect();
    Ok(GlyphCatalog::from_rows(glyphs, slots))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every glyph spell's effect 74 names a `GlyphProperties.dbc` row that grants a spell, the
    /// glyph socket bit is the only `Targets` bit past `0xffff` and stands alone, and the six
    /// sockets are three majors and three minors, each with its own tooltip key.
    #[test]
    fn the_glyph_spells_name_their_rows_and_the_sockets_split_three_and_three() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let catalog = load_glyph_catalog(&mut chain).expect("glyph tables");
        let spells = crate::load_spell_catalog(&mut chain).expect("Spell.dbc");
        assert!(!catalog.is_empty());
        let mut inscriptions = 0;
        for (id, def) in spells.iter() {
            if def.targets > 0xffff {
                assert_eq!(def.targets, 0x2_0000, "spell {id}: the socket bit alone");
                assert!(
                    def.effects.contains(&crate::SPELL_EFFECT_APPLY_GLYPH),
                    "spell {id}"
                );
            }
            for (i, &effect) in def.effects.iter().enumerate() {
                if effect != crate::SPELL_EFFECT_APPLY_GLYPH {
                    continue;
                }
                let Some(g) = catalog.glyph(def.effect_misc_value[i] as u32) else {
                    continue;
                };
                inscriptions += 1;
                assert!(g.spell != 0, "glyph {} grants a spell", g.id);
                assert!(
                    g.slot_type <= 1,
                    "glyph {} fits a major or minor socket",
                    g.id
                );
            }
        }
        assert!(inscriptions > 100, "{inscriptions} glyph inscriptions");
        let mut kinds: Vec<_> = catalog
            .slots()
            .filter(|s| s.tooltip != 0)
            .map(|s| (s.tooltip, s.slot_type))
            .collect();
        kinds.sort();
        let types: Vec<u32> = kinds.iter().map(|k| k.1).collect();
        assert_eq!(kinds.len(), 6);
        assert_eq!(types.iter().filter(|&&t| t == 0).count(), 3);
        assert_eq!(types.iter().filter(|&&t| t == 1).count(), 3);
    }
}
