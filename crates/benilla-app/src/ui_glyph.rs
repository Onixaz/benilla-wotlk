//! The glyph feed: `GlyphProperties.dbc` and `GlyphSlot.dbc`, the player's glyph fields and the
//! targeting cast's glyph effects into the VM, and the field watchers' events. 12340 registers
//! `OnUpdatePlayerGlyph` (`0x6daa80`) on `PLAYER_FIELD_GLYPHS_1..6` and
//! `OnUpdatePlayerGlyphSlotsEnabled` (`0x6dab80`) on `PLAYER_GLYPHS_ENABLED`; each is called with
//! the old value while the field already holds the new, and a create block notifies nothing.

use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_formats::{GlyphCatalog, SPELL_EFFECT_APPLY_GLYPH};
use benilla_protocol::field::{FIELD_PLAYER_GLYPHS_1, FIELD_PLAYER_GLYPHS_ENABLED};
use benilla_ui::script::{
    GlyphCatalogView, GlyphSlotView, GlyphSockets, GlyphTargeting, GlyphView, ScriptValue,
    UiScript, NUM_GLYPH_SOCKETS,
};

use crate::net::{ClientCommand, FieldChanged, NetCommands, ObjectStore, SelfPlayer};
use crate::spell::targeting::{SpellTargeting, TargetingWants};
use crate::ui_action::Spells;
use crate::ui_script::{UiInput, VmMemo};
use crate::ui_unit::UnitFeed;

/// The glyph tables, absent when the client data failed to load.
#[derive(Resource)]
pub(crate) struct Glyphs {
    pub(crate) catalog: GlyphCatalog,
}

pub(crate) struct UiGlyphPlugin;

impl Plugin for UiGlyphPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_glyphs.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    // After the talent pages, so a watcher's handler reads the groups the
                    // `SMSG_TALENTS_INFO` ahead of the field update named.
                    feed_glyphs
                        .in_set(UnitFeed)
                        .after(crate::ui_talent::TalentFeed),
                    drain_glyph_removes.after(UiInput),
                ),
            );
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

/// Both tables as the VM reads them, each glyph with its spell's name.
pub(crate) fn catalog_view(
    catalog: &GlyphCatalog,
    spells: &benilla_formats::SpellCatalog,
) -> GlyphCatalogView {
    GlyphCatalogView {
        glyphs: catalog
            .glyphs()
            .map(|g| {
                let view = GlyphView {
                    spell: g.spell,
                    slot_type: g.slot_type,
                    icon: g.icon.clone(),
                    name: spells.get(g.spell).map(|d| d.name.clone()),
                };
                (g.id, view)
            })
            .collect(),
        slots: catalog
            .slots()
            .map(|s| {
                let view = GlyphSlotView {
                    slot_type: s.slot_type,
                    tooltip: s.tooltip,
                };
                (s.id, view)
            })
            .collect(),
    }
}

/// The player's three glyph fields.
fn sockets_of(store: &ObjectStore) -> GlyphSockets {
    let mut sockets = GlyphSockets {
        enabled: store.0.player_glyphs_enabled(),
        ..Default::default()
    };
    for i in 0..NUM_GLYPH_SOCKETS {
        sockets.slots[i] = store.0.player_glyph_slot(i as u8);
        sockets.glyphs[i] = store.0.player_glyph(i as u8);
    }
    sockets
}

/// The targeting cast's effect-74 glyphs, `None` without a cast or its `Spell.dbc` row.
fn targeting_of(targeting: &SpellTargeting, spells: Option<&Spells>) -> Option<GlyphTargeting> {
    let def = spells?.catalog.get(targeting.spell()?)?;
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

/// `OnUpdatePlayerGlyph`'s and `OnUpdatePlayerGlyphSlotsEnabled`'s events for one field edge, the
/// socket 1-based. A glyph is one with a `GlyphProperties.dbc` row: an old one is `GLYPH_UPDATED`
/// when a new one replaces it and `GLYPH_REMOVED` when none does, and a new one in an empty socket
/// is `GLYPH_ADDED` (`0x6dab0c`, `0x6dab2c`, `0x6dab69`). Each mask bit set is `GLYPH_ENABLED`,
/// each cleared `GLYPH_DISABLED`.
fn watcher_events(catalog: &GlyphCatalog, edge: &FieldChanged) -> Vec<(&'static str, i64)> {
    let glyphs = FIELD_PLAYER_GLYPHS_1..FIELD_PLAYER_GLYPHS_1 + NUM_GLYPH_SOCKETS as u16;
    if glyphs.contains(&edge.index) {
        let socket = i64::from(edge.index - FIELD_PLAYER_GLYPHS_1) + 1;
        let is_glyph = |id: u32| catalog.glyph(id).is_some();
        let event = match (is_glyph(edge.old), is_glyph(edge.new)) {
            (true, true) => "GLYPH_UPDATED",
            (true, false) => "GLYPH_REMOVED",
            (false, true) => "GLYPH_ADDED",
            (false, false) => return Vec::new(),
        };
        return vec![(event, socket)];
    }
    if edge.index != FIELD_PLAYER_GLYPHS_ENABLED {
        return Vec::new();
    }
    (0..NUM_GLYPH_SOCKETS)
        .filter_map(|i| {
            let bit = 1u32 << i;
            match (edge.old & bit != 0, edge.new & bit != 0) {
                (false, true) => Some(("GLYPH_ENABLED", i as i64 + 1)),
                (true, false) => Some(("GLYPH_DISABLED", i as i64 + 1)),
                _ => None,
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn feed_glyphs(
    script: Option<NonSendMut<UiScript>>,
    glyphs: Option<Res<Glyphs>>,
    spells: Option<Res<Spells>>,
    targeting: Res<SpellTargeting>,
    self_q: Query<(Entity, &ObjectStore), With<SelfPlayer>>,
    changed_self: Query<(), (With<SelfPlayer>, Changed<ObjectStore>)>,
    mut edges: MessageReader<FieldChanged>,
    mut memory: Local<VmMemo<()>>,
) {
    let Some(mut script) = script else {
        edges.clear();
        return;
    };
    let (_, vm_reset) = memory.get_reset(&script);
    let spells_changed = spells.as_ref().is_some_and(|r| r.is_changed());
    let glyphs_changed = glyphs.as_ref().is_some_and(|r| r.is_changed());
    if let (Some(glyphs), Some(spells)) = (glyphs.as_deref(), spells.as_deref()) {
        if vm_reset || spells_changed || glyphs_changed {
            script.set_glyph_catalog(catalog_view(&glyphs.catalog, &spells.catalog));
        }
    }
    let me = self_q.iter().next();
    if vm_reset || !changed_self.is_empty() {
        script.set_glyph_sockets(me.map(|(_, store)| sockets_of(store)).unwrap_or_default());
    }
    if vm_reset || spells_changed || targeting.is_changed() {
        script.set_glyph_targeting(targeting_of(&targeting, spells.as_deref()));
    }
    let (Some(glyphs), Some((me, _))) = (glyphs.as_deref(), me) else {
        edges.clear();
        return;
    };
    for edge in edges.read().filter(|e| e.entity == me) {
        for (event, socket) in watcher_events(&glyphs.catalog, edge) {
            debug!("ui_glyph: {event} {socket}");
            script.fire_event(event, vec![ScriptValue::Int(socket)]);
        }
    }
}

/// `RemoveGlyphFromSocket`'s sends, `CMSG_REMOVE_GLYPH` per socket; the server answers with
/// `SMSG_TALENTS_INFO` and the glyph field.
fn drain_glyph_removes(script: Option<NonSendMut<UiScript>>, commands: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for socket in script.take_glyph_removes() {
        debug!("ui_glyph: remove glyph from socket {socket}");
        let _ = commands.0.send(ClientCommand::RemoveGlyph { socket });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::{GlyphProperty, GlyphSlot};
    use benilla_protocol::messages::ObjectType;

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

    fn edge(index: u16, old: u32, new: u32) -> FieldChanged {
        FieldChanged {
            entity: Entity::PLACEHOLDER,
            guid: 1,
            kind: ObjectType::Player,
            index,
            old,
            new,
        }
    }

    /// The watcher's three arms by the old and new value's rows, and an id with no row as empty.
    #[test]
    fn a_glyph_edge_is_added_removed_or_updated_by_its_old_and_new_rows() {
        let c = catalog();
        let third = FIELD_PLAYER_GLYPHS_1 + 2;
        assert_eq!(
            watcher_events(&c, &edge(third, 0, 181)),
            [("GLYPH_ADDED", 3)]
        );
        assert_eq!(
            watcher_events(&c, &edge(third, 181, 0)),
            [("GLYPH_REMOVED", 3)]
        );
        assert_eq!(
            watcher_events(&c, &edge(third, 181, 182)),
            [("GLYPH_UPDATED", 3)]
        );
        assert_eq!(
            watcher_events(&c, &edge(third, 999, 181)),
            [("GLYPH_ADDED", 3)]
        );
        assert!(watcher_events(&c, &edge(third, 0, 999)).is_empty());
        assert!(watcher_events(&c, &edge(FIELD_PLAYER_GLYPHS_1 - 1, 0, 181)).is_empty());
    }

    /// Each mask bit that turns on enables its socket and each that turns off disables it.
    #[test]
    fn a_mask_edge_enables_and_disables_by_bit() {
        let c = catalog();
        assert_eq!(
            watcher_events(
                &c,
                &edge(FIELD_PLAYER_GLYPHS_ENABLED, 0b0000_0011, 0b0000_0110)
            ),
            [("GLYPH_DISABLED", 1), ("GLYPH_ENABLED", 3)]
        );
    }
}
