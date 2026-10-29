//! The glyph bindings, 12340's `GlyphInfo` (registrar table `0xacff54`) and `GameTooltip:SetGlyph`
//! (`0x625d00` → `0x622ba0`). Two sources meet in them, as in the reference: a talent group's
//! glyphs from `SMSG_TALENTS_INFO` ([`super::TalentGroupView::glyphs`], group `+0x3c`), which the
//! window and the tooltip read, and the player's own fields, the sockets, the inscribed glyphs and
//! the unlocked mask ([`GlyphSockets`]), which the placement gate reads.
//!
//! A socket index is the Lua argument less one, compared unsigned against 6, so 0 and below wrap
//! past every socket. `PlaceGlyphInSocket` judges the targeting cast here, as
//! `Spell_C_HandleGlyphClick` (`0x80c3e0`) does, and queues [`super::ScriptCall::PlaceGlyph`]
//! with the verdict for the app's cursor to bind or fail.

use std::collections::HashMap;

use mlua::{Lua, MultiValue, Table, Value};

use super::binding_abi::{flag, number_arg};
use super::object::frame_handle_of;
use super::tooltip::{append_line, clear_content, fire_cleared};
use super::Model;

/// The glyph sockets, `GetNumGlyphSockets` (`0x5b71e0`).
pub const NUM_GLYPH_SOCKETS: usize = 6;

/// `SPELL_FAILED_INVALID_GLYPH`, `SPELL_FAILED_UNIQUE_GLYPH` and `SPELL_FAILED_GLYPH_SOCKET_LOCKED`
/// as 12340 numbers them (`0x80c510`, `0x80c5be`, `0x80c462`).
pub const GLYPH_FAILED_INVALID: u8 = 0xaf;
pub const GLYPH_FAILED_UNIQUE: u8 = 0xb0;
pub const GLYPH_FAILED_LOCKED: u8 = 0xb1;

/// One `GlyphProperties.dbc` row, with its spell's name for the link and the tooltip.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlyphView {
    pub spell: u32,
    pub slot_type: u32,
    pub icon: Option<String>,
    /// The spell's `Spell.dbc` name; `None` when the spell has no row, which voids the link and
    /// the tooltip's name (`0x61e6c8`, `0x622c2b`).
    pub name: Option<String>,
}

/// One `GlyphSlot.dbc` row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlyphSlotView {
    /// 0 major, 1 minor.
    pub slot_type: u32,
    /// The `GLYPH_SLOT_TOOLTIP<n>` a locked socket's tooltip reads.
    pub tooltip: u32,
}

/// Both glyph tables, pushed once.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlyphCatalogView {
    pub glyphs: HashMap<u32, GlyphView>,
    pub slots: HashMap<u32, GlyphSlotView>,
}

/// The player's glyph fields: `PLAYER_FIELD_GLYPH_SLOTS_1..6` (`+0x1230`),
/// `PLAYER_FIELD_GLYPHS_1..6` (`+0x1248`) and `PLAYER_GLYPHS_ENABLED` (`+0x1260`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlyphSockets {
    pub slots: [u32; NUM_GLYPH_SOCKETS],
    pub glyphs: [u32; NUM_GLYPH_SOCKETS],
    pub enabled: u32,
}

/// The targeting cast as the glyph verbs read it (`Spell_C_GetTargetingSpellCast` `0x7fd640` and
/// its `Spell.dbc` row): each effect slot's `EffectMiscValue` where the effect is 74
/// (`APPLY_GLYPH`), and whether the standing word still wants a socket (`& 0x20000`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlyphTargeting {
    pub glyphs: [Option<u32>; 3],
    pub wants_socket: bool,
}

/// `Spell_C_HandleGlyphClick`'s verdict on a socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlyphPlacement {
    /// The socket binds: the cast takes it as its glyph index and the socket bit clears.
    Bind,
    /// `Spell_C_SpellFailed` with this 12340 reason, which ends the cast.
    Fail(u8),
}

impl GlyphCatalogView {
    fn glyph(&self, id: u32) -> Option<&GlyphView> {
        self.glyphs.get(&id)
    }

    fn slot(&self, id: u32) -> Option<&GlyphSlotView> {
        self.slots.get(&id)
    }

    /// `CGTooltip::GetGlyphLink` (`0x61e650`): "" unless the slot, the glyph and its spell have
    /// rows.
    fn link(&self, slot_id: u32, glyph_id: u32) -> String {
        let name = self
            .slot(slot_id)
            .and(self.glyph(glyph_id))
            .and_then(|g| g.name.as_deref());
        match name {
            Some(name) => format!("|cff66bbff|Hglyph:{slot_id}:{glyph_id}|h[{name}]|h|r"),
            None => String::new(),
        }
    }
}

/// The verdict for `socket` (0-based, below 6), `None` without a targeting cast: the socket must be
/// unlocked; then the first effect 74 whose glyph and socket both have rows decides, a type
/// mismatch (`0x80c53f`) or a glyph already in the fields (`0x80c557`) failing it; no such effect
/// fails it as invalid (`0x80c510`).
pub(crate) fn placement(model: &Model, socket: usize) -> Option<GlyphPlacement> {
    let targeting = model.glyph_targeting?;
    let sockets = &model.glyph_sockets;
    if sockets.enabled & (1 << socket) == 0 {
        return Some(GlyphPlacement::Fail(GLYPH_FAILED_LOCKED));
    }
    let catalog = &model.glyph_catalog;
    for misc in targeting.glyphs.into_iter().flatten() {
        let (Some(glyph), Some(slot)) = (catalog.glyph(misc), catalog.slot(sockets.slots[socket]))
        else {
            continue;
        };
        if slot.slot_type != glyph.slot_type {
            return Some(GlyphPlacement::Fail(GLYPH_FAILED_INVALID));
        }
        if sockets.glyphs.contains(&misc) {
            return Some(GlyphPlacement::Fail(GLYPH_FAILED_UNIQUE));
        }
        return Some(GlyphPlacement::Bind);
    }
    Some(GlyphPlacement::Fail(GLYPH_FAILED_INVALID))
}

impl super::UiScript {
    /// Push `GlyphProperties.dbc` and `GlyphSlot.dbc`.
    pub fn set_glyph_catalog(&mut self, catalog: GlyphCatalogView) {
        set_field!(self, glyph_catalog, catalog);
    }

    /// Push the player's glyph fields.
    pub fn set_glyph_sockets(&mut self, sockets: GlyphSockets) {
        set_field!(self, glyph_sockets, sockets);
    }

    /// Push the targeting cast's glyph reading, `None` while no cast is targeting.
    pub fn set_glyph_targeting(&mut self, targeting: Option<GlyphTargeting>) {
        set_field!(self, glyph_targeting, targeting);
    }

    /// Take the `RemoveGlyphFromSocket` sends, 0-based sockets in call order.
    pub fn take_glyph_removes(&mut self) -> Vec<u32> {
        drain_field!(self, glyph_removes)
    }
}

/// The spells of every glyph the talent groups hold, whose descriptions the tooltip reads.
pub(super) fn glyph_spells(model: &Model) -> impl Iterator<Item = u32> + '_ {
    model
        .talents
        .groups
        .iter()
        .flat_map(|g| g.glyphs)
        .filter_map(|id| model.glyph_catalog.glyph(id))
        .map(|g| g.spell)
}

/// The player object exists, as every glyph verb but the count first asks.
fn has_player(model: &Model) -> bool {
    model.unit("player").is_some_and(|u| u.exists)
}

/// `index - 1` as the reference's unsigned compare reads it.
fn socket_of(index: i32) -> usize {
    (index as u32).wrapping_sub(1) as usize
}

/// The group a verb's `talentGroup` names (`TalentInfoGetScriptGroupIndex` `0x5c57d0`), its glyph
/// at `socket` (`CGTalentGroup_C::GetGlyph` `0x5c5770`), `None` for a group not there
/// (`TalentInfoGetTalentGroup` `0x5c6080`).
fn group_glyph(model: &Model, group: &Value, socket: usize) -> Option<u32> {
    let group = super::talent::group_arg(model, group);
    let g = model.talents.groups.get(group)?;
    Some(
        g.glyphs
            .get(socket)
            .copied()
            .unwrap_or(NUM_GLYPH_SOCKETS as u32),
    )
}

fn arg(args: &MultiValue, i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Nil)
}

fn string_or_nil(lua: &Lua, s: Option<&str>) -> mlua::Result<Value> {
    Ok(match s {
        Some(s) => Value::String(lua.create_string(s)?),
        None => Value::Nil,
    })
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumGlyphSockets() (`0x5b71e0`, shared with GetMultiCastBarOffset): 6.
    g.set(
        "GetNumGlyphSockets",
        lua.create_function(|_, ()| Ok(NUM_GLYPH_SOCKETS as i64))?,
    )?;

    // GetGlyphSocketInfo(index[, talentGroup]) (`0x5b7260`): enabled (1/nil), the socket's type + 1
    // (1 major, 2 minor), the group's glyph spell and its icon; nothing for a socket past 6, a
    // group not there or no player.
    g.set(
        "GetGlyphSocketInfo",
        lua.create_function(|lua, args: MultiValue| {
            let index = number_arg(
                lua,
                arg(&args, 0),
                "Usage: GetGlyphSocketInfo(index[, talentGroup])",
            )?;
            let socket = socket_of(index);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let glyph_id = group_glyph(&model, &arg(&args, 1), socket);
            let (true, Some(glyph_id), true) =
                (socket < NUM_GLYPH_SOCKETS, glyph_id, has_player(&model))
            else {
                return Ok(MultiValue::new());
            };
            let sockets = &model.glyph_sockets;
            let catalog = &model.glyph_catalog;
            let slot_type = catalog
                .slot(sockets.slots[socket])
                .map_or(Value::Nil, |s| Value::Integer(i64::from(s.slot_type) + 1));
            let (spell, icon) = match catalog.glyph(glyph_id) {
                Some(g) => (
                    Value::Integer(i64::from(g.spell)),
                    string_or_nil(lua, g.icon.as_deref())?,
                ),
                None => (Value::Nil, Value::Nil),
            };
            Ok(MultiValue::from_vec(vec![
                flag(sockets.enabled & (1 << socket) != 0),
                slot_type,
                spell,
                icon,
            ]))
        })?,
    )?;

    // GlyphMatchesSocket(socketIndex) (`0x5b7410`): nil without the player; nothing without a
    // targeting cast; else the first effect 74 whose glyph and socket have rows answers 1 on a
    // type match and nothing on a mismatch. The index is not bounded there: one past the six reads
    // past the slots, which here is no slot.
    g.set(
        "GlyphMatchesSocket",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GlyphMatchesSocket(socketIndex)")?;
            let socket = socket_of(index);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            if !has_player(&model) {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            }
            let Some(targeting) = model.glyph_targeting else {
                return Ok(MultiValue::new());
            };
            let catalog = &model.glyph_catalog;
            let slot = model
                .glyph_sockets
                .slots
                .get(socket)
                .and_then(|&id| catalog.slot(id));
            for misc in targeting.glyphs.into_iter().flatten() {
                let (Some(glyph), Some(slot)) = (catalog.glyph(misc), slot) else {
                    continue;
                };
                if slot.slot_type != glyph.slot_type {
                    break;
                }
                return Ok(MultiValue::from_vec(vec![Value::Integer(1)]));
            }
            Ok(MultiValue::new())
        })?,
    )?;

    // PlaceGlyphInSocket(index) (`0x5b75a0`): nothing past the six or without the player; else 1
    // when the targeting cast takes the socket and its word wanted one, nil otherwise.
    g.set(
        "PlaceGlyphInSocket",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: PlaceGlyphInSocket(index)")?;
            let socket = socket_of(index);
            // Read first: the mutable borrow is taken only to queue a verdict.
            let (verdict, wants) = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                if socket >= NUM_GLYPH_SOCKETS || !has_player(&model) {
                    return Ok(MultiValue::new());
                }
                let wants = model.glyph_targeting.is_some_and(|t| t.wants_socket);
                (placement(&model, socket), wants)
            };
            let Some(verdict) = verdict else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            crate::script::model_mut_of(lua)
                .script_calls
                .push(super::ScriptCall::PlaceGlyph {
                    socket: socket as u32,
                    verdict,
                });
            Ok(MultiValue::from_vec(vec![flag(
                verdict == GlyphPlacement::Bind && wants,
            )]))
        })?,
    )?;

    // RemoveGlyphFromSocket(index) (`0x5b7660`): `CMSG_REMOVE_GLYPH` for a socket below 6 that
    // the player's fields name, inscribed or not; no return.
    g.set(
        "RemoveGlyphFromSocket",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: RemoveGlyphFromSocket(index)")?;
            let socket = socket_of(index);
            let sends = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                socket < NUM_GLYPH_SOCKETS
                    && has_player(&model)
                    && model.glyph_sockets.slots[socket] != 0
            };
            if sends {
                crate::script::model_mut_of(lua)
                    .glyph_removes
                    .push(socket as u32);
            }
            Ok(())
        })?,
    )?;

    // GetGlyphLink(index[, talentGroup]) (`0x5b7740`): the socket's slot and the group's glyph as
    // a `glyph:` link, "" when either has no row; nothing past the six, without the player or for
    // a group not there.
    g.set(
        "GetGlyphLink",
        lua.create_function(|lua, args: MultiValue| {
            let index = number_arg(
                lua,
                arg(&args, 0),
                "Usage: GetGlyphLink(index[, talentGroup])",
            )?;
            let socket = socket_of(index);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let glyph_id = group_glyph(&model, &arg(&args, 1), socket);
            let (true, true, Some(glyph_id)) =
                (socket < NUM_GLYPH_SOCKETS, has_player(&model), glyph_id)
            else {
                return Ok(MultiValue::new());
            };
            let link = model
                .glyph_catalog
                .link(model.glyph_sockets.slots[socket], glyph_id);
            Ok(MultiValue::from_vec(vec![Value::String(
                lua.create_string(&link)?,
            )]))
        })?,
    )?;

    // SpellCanTargetGlyph() (`0x7fdd60`): 1 while the targeting word wants a socket, else nil.
    g.set(
        "SpellCanTargetGlyph",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(
                model.spell_targeting && model.glyph_targeting.is_some_and(|t| t.wants_socket),
            ))
        })?,
    )?;

    Ok(())
}

/// The tooltip's white name, `0xffffffff` (`0xad2d30`).
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// The empty socket's and the remove hint's gray, `0xff808080` (`0xad2d54`).
const GRAY: [f32; 4] = [128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0];
/// The locked socket's red, `0xffff0000` (`0xad2d48`).
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
/// The glyph type's blue, `0xff66bbff` (`0xad2da4`).
const GLYPH_BLUE: [f32; 4] = [102.0 / 255.0, 187.0 / 255.0, 1.0, 1.0];
/// The description gold, `0xffffd200` (`0xad2d2c`).
const GOLD: [f32; 4] = [1.0, 210.0 / 255.0, 0.0, 1.0];

/// `CGTooltip::SetGlyph` (`0x622ba0`): the glyph's spell name white, else "Empty" gray for an
/// unlocked socket or "Locked" red; the type line blue; then, with no glyph spell, the empty
/// socket's hint or a locked one's unlock level, else the spell's description gold and, unless
/// `hide_remove`, the gray remove hint.
fn set_glyph(
    lua: &Lua,
    this: &Table,
    slot_id: u32,
    glyph_id: u32,
    enabled: bool,
    hide_remove: bool,
) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let (slot, glyph) = {
        let mut model = crate::script::model_mut_of(lua);
        clear_content(&mut model, h);
        let catalog = &model.glyph_catalog;
        (
            catalog.slot(slot_id).copied(),
            catalog.glyph(glyph_id).cloned(),
        )
    };
    fire_cleared(lua, h);
    let text = |key: &str| crate::strings::global(lua, key);
    let spell = glyph
        .as_ref()
        .and_then(|g| Some((g.spell, g.name.clone()?)));
    match &spell {
        Some((_, name)) => append_line(lua, this, (name.clone(), WHITE), None, false)?,
        None if glyph.is_some() => {}
        None => {
            let (key, colour) = if enabled {
                ("GLYPH_INACTIVE", GRAY)
            } else {
                ("GLYPH_LOCKED", RED)
            };
            if let Some(t) = text(key) {
                append_line(lua, this, (t, colour), None, false)?;
            }
        }
    }
    let kind = if slot.is_some_and(|s| s.slot_type != 0) {
        "MINOR_GLYPH"
    } else {
        "MAJOR_GLYPH"
    };
    if let Some(t) = text(kind) {
        append_line(lua, this, (t, GLYPH_BLUE), None, false)?;
    }
    match spell {
        Some((spell, _)) => {
            let desc = super::tooltip_spell::spell_view_of(lua, spell, false)
                .map(|v| v.description)
                .filter(|d| !d.is_empty());
            if let Some(desc) = desc {
                append_line(lua, this, (desc, GOLD), None, true)?;
                if !hide_remove {
                    if let Some(t) = text("GLYPH_SLOT_REMOVE_TOOLTIP") {
                        append_line(lua, this, (t, GRAY), None, false)?;
                    }
                }
            }
        }
        None => {
            let hint = if enabled {
                text("GLYPH_EMPTY_DESC")
            } else {
                slot.and_then(|s| text(&format!("GLYPH_SLOT_TOOLTIP{}", s.tooltip)))
            };
            if let Some(t) = hint {
                append_line(lua, this, (t, GOLD), None, true)?;
            }
        }
    }
    super::tooltip::show_or_hide_empty(lua, h);
    Ok(())
}

/// A `glyph:<slot>:<glyph>` hyperlink, `CGTooltip::SetHyperlink`'s glyph arm: the slot and glyph
/// ids, `None` without the second field.
fn parse_glyph_link(link: &str) -> Option<(u32, u32)> {
    let rest = &link[link.find("glyph:")? + "glyph:".len()..];
    let (slot, glyph) = rest.split_once(':')?;
    Some((leading_int(slot), leading_int(glyph)))
}

/// `SStrToInt`: the leading decimal digits, 0 for none.
fn leading_int(s: &str) -> u32 {
    s.bytes().take_while(u8::is_ascii_digit).fold(0u32, |n, d| {
        n.wrapping_mul(10).wrapping_add(u32::from(d - b'0'))
    })
}

/// `GameTooltip:SetHyperlink`'s glyph arm: the linked glyph as an unlocked socket's, with no
/// remove hint. Answers whether the link was a glyph link.
pub(super) fn set_glyph_link(lua: &Lua, this: &Table, link: &str) -> mlua::Result<bool> {
    if !link.contains("glyph:") {
        return Ok(false);
    }
    if let Some((slot, glyph)) = parse_glyph_link(link) {
        set_glyph(lua, this, slot, glyph, true, true)?;
    }
    Ok(true)
}

/// Register `GameTooltip:SetGlyph(glyphSlot[, groupIndex])` (`0x625d00`): the socket's slot, the
/// group's glyph and the socket's unlocked bit, for a group that is there and a present player.
pub(super) fn install_tooltip_method(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.set(
        "SetGlyph",
        lua.create_function(|lua, (this, args): (Table, MultiValue)| {
            let index = number_arg(
                lua,
                arg(&args, 0),
                "Usage: GameTooltip:SetGlyph(glyphSlot[, groupIndex])",
            )?;
            let socket = socket_of(index);
            let (slot_id, glyph_id, enabled) = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let Some(glyph_id) = group_glyph(&model, &arg(&args, 1), socket) else {
                    return Ok(());
                };
                if !has_player(&model) {
                    return Ok(());
                }
                let sockets = &model.glyph_sockets;
                (
                    sockets.slots.get(socket).copied().unwrap_or(0),
                    glyph_id,
                    socket < 32 && sockets.enabled & (1 << socket) != 0,
                )
            };
            set_glyph(lua, &this, slot_id, glyph_id, enabled, false)
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{TalentGroupView, TalentUiState, UiScript, UnitState};

    const ICON: &str = r"Interface\Icons\INV_Glyph_MajorHunter";

    /// A present player with sockets 21-26 unlocked but the sixth, glyph 181 (major) in socket 1
    /// of group 1, and glyph 182 a minor.
    fn script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit(
            "player",
            Some(UnitState {
                exists: true,
                ..Default::default()
            }),
        );
        s.set_talents(TalentUiState {
            groups: vec![TalentGroupView {
                glyphs: [181, 0, 0, 0, 0, 0],
                ..Default::default()
            }],
            ..Default::default()
        });
        let glyph = |slot_type, name: &str| GlyphView {
            spell: 1000 + slot_type,
            slot_type,
            icon: Some(ICON.into()),
            name: Some(name.into()),
        };
        s.set_glyph_catalog(GlyphCatalogView {
            glyphs: HashMap::from([(181, glyph(0, "Glyph of A")), (182, glyph(1, "Glyph of B"))]),
            slots: (21..=26)
                .map(|id| {
                    let slot_type = u32::from(matches!(id, 22 | 23 | 25));
                    (
                        id,
                        GlyphSlotView {
                            slot_type,
                            tooltip: id - 20,
                        },
                    )
                })
                .collect(),
        });
        s.set_glyph_sockets(GlyphSockets {
            slots: [21, 22, 23, 24, 25, 26],
            glyphs: [181, 0, 0, 0, 0, 0],
            enabled: 0x1f,
        });
        s
    }

    #[test]
    fn socket_info_reads_the_group_glyph_and_the_fields() {
        let s = script();
        let first: (i64, i64, i64, String) = s.eval("return GetGlyphSocketInfo(1)").unwrap();
        assert_eq!(first, (1, 1, 1000, ICON.into()));
        let locked: (Option<i64>, i64, Option<i64>, Option<String>) =
            s.eval("return GetGlyphSocketInfo(6)").unwrap();
        assert_eq!(locked, (None, 1, None, None), "socket 6 is a locked major");
        assert_eq!(
            s.eval::<i64>("return select('#', GetGlyphSocketInfo(0))")
                .unwrap(),
            0,
            "index 0 wraps past the six"
        );
        assert_eq!(
            s.eval::<i64>("return select('#', GetGlyphSocketInfo(1, 2))")
                .unwrap(),
            0,
            "no group 2"
        );
        assert!(s.run("GetGlyphSocketInfo()").is_err(), "usage");
    }

    #[test]
    fn without_the_player_the_verbs_answer_nothing() {
        let mut s = script();
        s.set_unit("player", None);
        let count = |s: &UiScript, call: &str| {
            s.eval::<i64>(&format!("return select('#', {call})"))
                .unwrap()
        };
        assert_eq!(count(&s, "GetGlyphSocketInfo(1)"), 0);
        assert_eq!(count(&s, "GetGlyphLink(1)"), 0);
        assert_eq!(count(&s, "PlaceGlyphInSocket(1)"), 0);
        assert_eq!(count(&s, "GlyphMatchesSocket(1)"), 1, "an explicit nil");
        s.run("RemoveGlyphFromSocket(1)").unwrap();
        assert!(s.take_glyph_removes().is_empty());
    }

    /// A socket the fields name sends, inscribed or not; past the six, nothing.
    #[test]
    fn remove_sends_for_any_named_socket() {
        let mut s = script();
        s.run("RemoveGlyphFromSocket(1) RemoveGlyphFromSocket(3) RemoveGlyphFromSocket(7)")
            .unwrap();
        assert_eq!(s.take_glyph_removes(), vec![0, 2]);
        s.set_glyph_sockets(GlyphSockets::default());
        s.run("RemoveGlyphFromSocket(1)").unwrap();
        assert!(s.take_glyph_removes().is_empty(), "no socket in the fields");
    }

    #[test]
    fn the_link_names_the_slot_and_the_glyph() {
        let s = script();
        assert_eq!(
            s.eval::<String>("return GetGlyphLink(1)").unwrap(),
            "|cff66bbff|Hglyph:21:181|h[Glyph of A]|h|r"
        );
        assert_eq!(s.eval::<String>("return GetGlyphLink(2)").unwrap(), "");
        assert_eq!(parse_glyph_link("|Hglyph:21:181|h[A]|h"), Some((21, 181)));
        assert_eq!(parse_glyph_link("glyph:21"), None);
    }

    /// Without a targeting cast nothing matches or places; with one, the verdict is queued and
    /// the answer is 1 only for a bind the word wanted.
    #[test]
    fn placement_needs_a_targeting_glyph_cast() {
        let mut s = script();
        assert_eq!(
            s.eval::<i64>("return select('#', GlyphMatchesSocket(4))")
                .unwrap(),
            0
        );
        assert_eq!(
            s.eval::<Option<i64>>("return PlaceGlyphInSocket(4)")
                .unwrap(),
            None
        );
        assert!(s.take_script_calls().is_empty());
        let arm = |s: &mut UiScript, glyph, wants_socket| {
            s.set_glyph_targeting(Some(GlyphTargeting {
                glyphs: [None, Some(glyph), None],
                wants_socket,
            }));
        };
        arm(&mut s, 182, true);
        assert_eq!(
            s.eval::<Option<i64>>("return GlyphMatchesSocket(2)")
                .unwrap(),
            Some(1)
        );
        assert_eq!(
            s.eval::<Option<i64>>("return GlyphMatchesSocket(4)")
                .unwrap(),
            None
        );
        assert_eq!(
            s.eval::<Option<i64>>("return PlaceGlyphInSocket(2)")
                .unwrap(),
            Some(1)
        );
        arm(&mut s, 182, false);
        assert_eq!(
            s.eval::<Option<i64>>("return PlaceGlyphInSocket(3)")
                .unwrap(),
            None
        );
        assert_eq!(
            s.eval::<Option<i64>>("return PlaceGlyphInSocket(6)")
                .unwrap(),
            None
        );
        assert_eq!(
            s.take_script_calls(),
            vec![
                crate::script::ScriptCall::PlaceGlyph {
                    socket: 1,
                    verdict: GlyphPlacement::Bind
                },
                crate::script::ScriptCall::PlaceGlyph {
                    socket: 2,
                    verdict: GlyphPlacement::Bind
                },
                crate::script::ScriptCall::PlaceGlyph {
                    socket: 5,
                    verdict: GlyphPlacement::Fail(GLYPH_FAILED_LOCKED)
                },
            ]
        );
        assert_eq!(
            s.eval::<Option<i64>>("return SpellCanTargetGlyph()")
                .unwrap(),
            None
        );
        s.set_spell_targeting(true);
        arm(&mut s, 182, true);
        assert_eq!(
            s.eval::<Option<i64>>("return SpellCanTargetGlyph()")
                .unwrap(),
            Some(1)
        );
    }

    /// The glyph frame polls `GlyphMatchesSocket` and `GetGlyphSocketInfo` every frame and the
    /// app drains the removes every frame: none of it moves the mutation count, so the UI extract
    /// keeps skipping.
    #[test]
    fn the_per_frame_glyph_reads_leave_the_mutation_count_standing() {
        let mut s = script();
        s.set_spell_targeting(true);
        s.set_glyph_targeting(Some(GlyphTargeting {
            glyphs: [Some(182), None, None],
            wants_socket: true,
        }));
        let poll = "for i = 1, 7 do GlyphMatchesSocket(i) GetGlyphSocketInfo(i) GetGlyphLink(i) end                     SpellCanTargetGlyph() PlaceGlyphInSocket(9) RemoveGlyphFromSocket(9)                     RemoveGlyphFromSocket(6 + 1)";
        s.run(poll).unwrap();
        let settled = s.visual_epoch();
        s.run(poll).unwrap();
        assert!(s.take_glyph_removes().is_empty());
        s.set_glyph_sockets(GlyphSockets {
            slots: [21, 22, 23, 24, 25, 26],
            glyphs: [181, 0, 0, 0, 0, 0],
            enabled: 0x1f,
        });
        s.set_glyph_targeting(Some(GlyphTargeting {
            glyphs: [Some(182), None, None],
            wants_socket: true,
        }));
        assert_eq!(
            s.visual_epoch(),
            settled,
            "an idle glyph frame writes nothing"
        );
    }
}
