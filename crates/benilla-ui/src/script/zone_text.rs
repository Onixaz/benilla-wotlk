//! The zone-text family, `GetZoneText`, `GetRealZoneText`, `GetSubZoneText`,
//! `GetMinimapZoneText` and `GetZonePVPInfo`: readers of the zone caches the host pushes on an
//! area change, before it fires `MINIMAP_ZONE_CHANGED` or the `ZONE_CHANGED` family. The caches
//! live in the [`Model`], out of an addon's reach, as the reference's live in the client's BSS.

use mlua::{Lua, MultiValue, Value};

/// The zone caches, as the host resolves them from the area tables.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ZoneTexts {
    /// `GetZoneText`: the zone's name, replaced by the WMO interior's name indoors.
    pub zone: String,
    /// `GetRealZoneText`: the zone's name before any WMO override.
    pub real_zone: String,
    /// `GetSubZoneText`: the leaf area's name, `""` when the leaf is the zone, and indoors.
    pub subzone: String,
    /// `GetMinimapZoneText`: the subzone, else the zone.
    pub minimap: String,
    /// `GetZonePVPInfo`'s type: `"sanctuary"`, `"arena"` or `"combat"` off the area's flags, else
    /// `"friendly"`, `"hostile"` or `"contested"`; `None` when it answers nothing.
    pub pvp_type: Option<String>,
    /// `GetZonePVPInfo`'s faction, `FactionGroup.dbc`'s name for the zone's mask bit, for the
    /// territory types; `None` for none.
    pub pvp_faction: Option<String>,
    /// `GetZonePVPInfo`'s `isSubZonePvP`: a sanctuary, arena or combat type read off the subzone.
    pub pvp_subzone: bool,
}

impl super::UiScript {
    /// Push the zone caches; the caller fires the zone events after.
    pub fn set_zone_texts(&mut self, texts: ZoneTexts) {
        set_field!(self, zone, texts);
    }
}

/// Register the five zone getters.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    // The four text getters (`0x48a0a0`, `0x48a0c0`, `0x48a0e0`, `0x48a100`) each push one string,
    // their cache slot or the shared empty string `0x882748` when the slot is unset, never nil.
    let text = |pick: fn(&ZoneTexts) -> &str| {
        move |lua: &Lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            lua.create_string(pick(&model.zone))
        }
    };
    g.set("GetZoneText", lua.create_function(text(|z| &z.zone))?)?;
    g.set(
        "GetRealZoneText",
        lua.create_function(text(|z| &z.real_zone))?,
    )?;
    g.set("GetSubZoneText", lua.create_function(text(|z| &z.subzone))?)?;
    g.set(
        "GetMinimapZoneText",
        lua.create_function(text(|z| &z.minimap))?,
    )?;
    // GetZonePVPInfo() (`0x51ba50`): `pvpType, isSubZonePvP` for a sanctuary, arena or combat
    // area, `pvpType, nil, factionName` for a territory, nothing without a type. `isSubZonePvP` is
    // 1 or nil, and the faction an empty string for no name.
    g.set(
        "GetZonePVPInfo",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let z = &model.zone;
            let Some(ty) = z.pvp_type.as_deref() else {
                return Ok(MultiValue::new());
            };
            let ty = Value::String(lua.create_string(ty)?);
            if matches!(
                z.pvp_type.as_deref(),
                Some("sanctuary" | "arena" | "combat")
            ) {
                let sub = if z.pvp_subzone {
                    Value::Number(1.0)
                } else {
                    Value::Nil
                };
                return Ok(MultiValue::from_vec(vec![ty, sub]));
            }
            let faction = z.pvp_faction.as_deref().unwrap_or("");
            Ok(MultiValue::from_vec(vec![
                ty,
                Value::Nil,
                Value::String(lua.create_string(faction)?),
            ]))
        })?,
    )?;
    Ok(())
}
