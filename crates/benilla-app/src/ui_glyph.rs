//! The glyphs: `GlyphProperties.dbc` and `GlyphSlot.dbc`, which the glyph verbs read at the call
//! with the player's glyph fields and the targeting cast's glyph effects; and the field watches'
//! events. 12340 registers the glyph watch (`0x6daa80`) on `PLAYER_FIELD_GLYPHS_1..6` and the
//! enabled-mask watch (`0x6dab80`) on `PLAYER_GLYPHS_ENABLED` (`0x6e58ea`, `0x6e591b`); each is
//! called from the values handler with the old value while the field already holds the new, and a
//! create block notifies nothing.

use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_formats::{GlyphCatalog, SPELL_EFFECT_APPLY_GLYPH};
use benilla_protocol::field::{FIELD_PLAYER_GLYPHS_1, FIELD_PLAYER_GLYPHS_ENABLED};
use benilla_protocol::ObjectFields;
use benilla_ui::script::{
    GlyphSlotView, GlyphSockets, GlyphTargeting, GlyphView, NUM_GLYPH_SOCKETS,
};

use crate::spell::targeting::{SpellTargeting, TargetingWants};

/// The glyph tables, absent when the client data failed to load.
#[derive(Resource)]
pub(crate) struct Glyphs {
    pub(crate) catalog: GlyphCatalog,
}

pub(crate) struct UiGlyphPlugin;

impl Plugin for UiGlyphPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_glyphs.after(AssetSet::Open));
    }
}

fn load_glyphs(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_glyph_catalog(&mut chain)
    };
    match loaded {
        Ok(catalog) => {
            info!("ui_glyph: {} glyph(s) in the catalog", catalog.len());
            commands.insert_resource(Glyphs { catalog });
        }
        Err(e) => warn!("ui_glyph: GlyphProperties.dbc failed to load — no glyphs: {e:#}"),
    }
}

/// The `GlyphProperties.dbc` row `id` with its spell's name, which voids the link and the tooltip's
/// name without a `Spell.dbc` row (`0x61e6c8`, `0x622c2b`).
pub(crate) fn glyph_view(
    catalog: &GlyphCatalog,
    spells: Option<&benilla_formats::SpellCatalog>,
    id: u32,
) -> Option<GlyphView> {
    let g = catalog.glyph(id)?;
    Some(GlyphView {
        spell: g.spell,
        slot_type: g.slot_type,
        icon: g.icon.clone(),
        name: spells.and_then(|s| s.get(g.spell)).map(|d| d.name.clone()),
    })
}

/// The `GlyphSlot.dbc` row `id`.
pub(crate) fn slot_view(catalog: &GlyphCatalog, id: u32) -> Option<GlyphSlotView> {
    let s = catalog.slot(id)?;
    Some(GlyphSlotView {
        slot_type: s.slot_type,
        tooltip: s.tooltip,
    })
}

/// Both tables whole, as a test view lends them.
#[cfg(test)]
pub(crate) fn catalog_view(
    catalog: &GlyphCatalog,
    spells: &benilla_formats::SpellCatalog,
) -> benilla_ui::script::GlyphCatalogView {
    benilla_ui::script::GlyphCatalogView {
        glyphs: catalog
            .glyphs()
            .filter_map(|g| Some((g.id, glyph_view(catalog, Some(spells), g.id)?)))
            .collect(),
        slots: catalog
            .slots()
            .filter_map(|s| Some((s.id, slot_view(catalog, s.id)?)))
            .collect(),
    }
}

/// The player's three glyph fields as they stand.
pub(crate) fn sockets_of(fields: &ObjectFields) -> GlyphSockets {
    let mut sockets = GlyphSockets {
        enabled: fields.player_glyphs_enabled(),
        ..Default::default()
    };
    for i in 0..NUM_GLYPH_SOCKETS {
        sockets.slots[i] = fields.player_glyph_slot(i as u8);
        sockets.glyphs[i] = fields.player_glyph(i as u8);
    }
    sockets
}

/// The targeting cast's effect-74 glyphs (`0x5b74ab`), `None` without a cast or its `Spell.dbc` row.
pub(crate) fn targeting_of(
    targeting: &SpellTargeting,
    spells: &benilla_formats::SpellCatalog,
) -> Option<GlyphTargeting> {
    let def = spells.get(targeting.spell()?)?;
    let mut glyphs = [None; 3];
    for (slot, (&effect, &misc)) in glyphs
        .iter_mut()
        .zip(def.effects.iter().zip(&def.effect_misc_value))
    {
        *slot = (effect == SPELL_EFFECT_APPLY_GLYPH).then_some(misc as u32);
    }
    Some(GlyphTargeting {
        glyphs,
        wants_socket: targeting.wants(TargetingWants::Glyph),
    })
}

/// Whether our field at `index` has a glyph watch: a glyph or the enabled mask.
pub(crate) fn is_glyph_watch(index: u16) -> bool {
    (FIELD_PLAYER_GLYPHS_1..=FIELD_PLAYER_GLYPHS_ENABLED).contains(&index)
}

/// The glyph watch's and the enabled-mask watch's events for one field edge, the socket 1-based. A
/// glyph is one with a `GlyphProperties.dbc` row: an old one is `GLYPH_UPDATED` when a new one
/// replaces it and `GLYPH_REMOVED` when none does, and a new one in an empty socket is
/// `GLYPH_ADDED` (`0x6dab0c`, `0x6dab2c`, `0x6dab69`). Each mask bit set is `GLYPH_ENABLED`, each
/// cleared `GLYPH_DISABLED`, in socket order (`0x6dabed`).
pub(crate) fn watcher_events(
    catalog: &GlyphCatalog,
    index: u16,
    old: u32,
    new: u32,
) -> Vec<(&'static str, i64)> {
    let glyphs = FIELD_PLAYER_GLYPHS_1..FIELD_PLAYER_GLYPHS_1 + NUM_GLYPH_SOCKETS as u16;
    if glyphs.contains(&index) {
        let socket = i64::from(index - FIELD_PLAYER_GLYPHS_1) + 1;
        let is_glyph = |id: u32| catalog.glyph(id).is_some();
        let event = match (is_glyph(old), is_glyph(new)) {
            (true, true) => "GLYPH_UPDATED",
            (true, false) => "GLYPH_REMOVED",
            (false, true) => "GLYPH_ADDED",
            (false, false) => return Vec::new(),
        };
        return vec![(event, socket)];
    }
    if index != FIELD_PLAYER_GLYPHS_ENABLED {
        return Vec::new();
    }
    (0..NUM_GLYPH_SOCKETS)
        .filter_map(|i| {
            let bit = 1u32 << i;
            match (old & bit != 0, new & bit != 0) {
                (false, true) => Some(("GLYPH_ENABLED", i as i64 + 1)),
                (true, false) => Some(("GLYPH_DISABLED", i as i64 + 1)),
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::{GlyphProperty, GlyphSlot};

    fn catalog() -> GlyphCatalog {
        GlyphCatalog::from_rows(
            [181, 182].map(|id| GlyphProperty {
                id,
                spell: 50000 + id,
                ..Default::default()
            }),
            [GlyphSlot {
                id: 21,
                ..Default::default()
            }],
        )
    }

    fn edge(index: u16, old: u32, new: u32) -> (u16, u32, u32) {
        (index, old, new)
    }

    fn events(c: &GlyphCatalog, (index, old, new): (u16, u32, u32)) -> Vec<(&'static str, i64)> {
        watcher_events(c, index, old, new)
    }

    /// The watcher's three arms by the old and new value's rows, and an id with no row as empty.
    #[test]
    fn a_glyph_edge_is_added_removed_or_updated_by_its_old_and_new_rows() {
        let c = catalog();
        let third = FIELD_PLAYER_GLYPHS_1 + 2;
        assert_eq!(events(&c, edge(third, 0, 181)), [("GLYPH_ADDED", 3)]);
        assert_eq!(events(&c, edge(third, 181, 0)), [("GLYPH_REMOVED", 3)]);
        assert_eq!(events(&c, edge(third, 181, 182)), [("GLYPH_UPDATED", 3)]);
        assert_eq!(events(&c, edge(third, 999, 181)), [("GLYPH_ADDED", 3)]);
        assert!(events(&c, edge(third, 0, 999)).is_empty());
        assert!(events(&c, edge(FIELD_PLAYER_GLYPHS_1 - 1, 0, 181)).is_empty());
    }

    /// Each mask bit that turns on enables its socket and each that turns off disables it.
    #[test]
    fn a_mask_edge_enables_and_disables_by_bit() {
        let c = catalog();
        assert_eq!(
            events(
                &c,
                edge(FIELD_PLAYER_GLYPHS_ENABLED, 0b0000_0011, 0b0000_0110)
            ),
            [("GLYPH_DISABLED", 1), ("GLYPH_ENABLED", 3)]
        );
    }

    /// With no rows loaded a glyph edge fires nothing, and the mask still fires: `0x6dab80` reads
    /// no row.
    #[test]
    fn without_rows_the_mask_still_fires() {
        let c = GlyphCatalog::default();
        assert!(events(&c, edge(FIELD_PLAYER_GLYPHS_1, 0, 181)).is_empty());
        assert_eq!(
            events(&c, edge(FIELD_PLAYER_GLYPHS_ENABLED, 0, 1)),
            [("GLYPH_ENABLED", 1)]
        );
    }
}
