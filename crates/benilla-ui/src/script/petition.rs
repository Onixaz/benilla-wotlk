//! The charter globals: the guild registrar (`CGGuildRegistrar`, `0x599f00`..), the arena
//! registrar's petition vendor (`CGPetitionVendorInfo`, `0x5a1f70`..) and the petition window
//! (`CGPetitionInfo`, `0x5ceac0`..). The snapshot keeps the signer list and the cached `CGPetition`
//! record apart, as the reference does, since the bindings read them independently. Predicates
//! are `1` or nil and an unresolved name is nil. `PETITION_SHOW` waits for every signer name and
//! the record, a deferral `crate::ui_petition` owns.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi;

/// `GetPetitionInfo`'s first return for a record's `static_type` (`0x5ced0f`): 0 a guild charter,
/// 1 an arena one, anything else another petition.
pub fn petition_type_name(static_type: u32) -> &'static str {
    match static_type {
        0 => "guild",
        1 => "arena",
        _ => "other",
    }
}

/// The cached `CGPetition` record, as `GetPetitionInfo` reads it; `None` in
/// [`PetitionState::record`] is the no-record leg.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionRecordView {
    /// [`petition_type_name`] of the record's `static_type`.
    pub petition_type: String,
    /// The proposed guild or team's name, the record's `char[0x100]` at `+0x10`.
    pub title: String,
    /// The record's `char[0x1000]` body at `+0x110`.
    pub body_text: String,
    /// The record's signature cap (`+0x1114`), which `CanSignPetition` tests, not the name rows.
    pub max_signatures: i32,
    /// The owner's name from the `NameCache`, nil while uncached, never `""`.
    pub originator: Option<String>,
    /// Whether the active player owns the record.
    pub is_originator: bool,
    /// The signatures required (`+0x1110`), `GetPetitionInfo`'s seventh; an arena charter's is
    /// its team size less one.
    pub min_signatures: i32,
}

/// What the two charter windows read, mirroring the reference module's `.data` state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionState {
    /// `GetGuildCharterCost()` (`0x599f40`): the registrar's charter row's cost in copper,
    /// unsigned; 0 before any showlist.
    pub charter_cost: u32,
    /// The signers in wire order, `None` until the `NameCache` resolves one; filled by the packet,
    /// apart from [`Self::record`]. `GetNumPetitionNames()` counts these, never the owner.
    pub signers: Vec<Option<String>>,
    /// The cached record; `None` is the no-record leg.
    pub record: Option<PetitionRecordView>,
    /// `CanSignPetition()`, computed app-side as three of its four refusals need state the engine
    /// lacks; it is `1` with nothing open, as in the reference.
    pub can_sign: bool,
}

/// The arena registrar's charter rows, `GetPetitionItemInfo`'s three (`0x5a1fb0`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionVendorItem {
    /// The item's name and icon path, `None` until the item cache has the row; the binding then
    /// answers `nil, nil, 0` and the cache's answer fires `PETITION_VENDOR_UPDATE`.
    pub name: Option<String>,
    pub texture: Option<String>,
    pub cost: u32,
}

/// The petition vendor's session as the app pushes it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionVendorState {
    /// A vendor is open (`m_vendor`, `0xc0f7c8`).
    pub open: bool,
    pub items: Vec<PetitionVendorItem>,
    /// `HasFilledPetition()` (`0x5a2820`): the bags hold an arena charter whose record is cached.
    pub has_filled_petition: bool,
}

/// An arena charter turn-in's banner, as `TurnInArenaPetition` packs it (`0x5a26d0`): three
/// `0xAARRGGBB` colours and two style indices, all `0xFFFFFFFF` without the eleven numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArenaBannerChoice {
    pub team_size: u32,
    pub background_color: u32,
    pub emblem_style: u32,
    pub emblem_color: u32,
    pub border_style: u32,
    pub border_color: u32,
}

/// A charter intent queued from Lua for the app to send. Most get no answer of their own (a
/// purchase shows only as the item, an offer answers the target, a refusal comes on the guild error
/// channel), so none may update local state optimistically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PetitionRequest {
    /// `BuyGuildCharter(name)`, after [`validate_guild_name`]; the app adds the registrar NPC
    /// latched, not `CGGameUI`'s interaction pair.
    Buy(String),
    /// `TurnInGuildCharter()`: the app scans the bags on every call, as the reference does.
    TurnIn,
    /// `CloseGuildRegistrar()`: sends nothing.
    CloseRegistrar,
    /// `GetTabardInfo()`, not a getter: with a player, `MSG_TABARDVENDOR_ACTIVATE` for the
    /// registrar's guid unless the designer is already open on that guid, or the
    /// `ERR_EMBLEMERROR_NOTABARDGEOSET` refusal while the player's display id is not its native
    /// one; then the registrar closes.
    TabardInfo,
    /// `SignPetition([n])`: a wire byte defaulting to 1, not 0; vmangos skips it.
    Sign(i8),
    /// `OfferPetition()`: the app resolves the current target (`CGGameUI`'s selection pair, not its
    /// interaction pair) and runs the eight guards.
    Offer,
    /// `RenamePetition(name)`, after [`validate_guild_name`].
    Rename(String),
    /// `ClosePetition()`: the reference sends `MSG_PETITION_DECLINE` when a petition was open, no
    /// sign is in flight, a record is cached and we do not own it; the app decides.
    ClosePetition,
    /// A name [`validate_guild_name`] refused, with the GlobalStrings key to show; no packet.
    NameRefused(&'static str),
    /// `BuyPetition(index, name)` at a petition vendor, the row 0-based, after the validator.
    VendorBuy { index: u32, name: String },
    /// `TurnInPetition()`: the item the petition button holds, with an empty banner.
    VendorTurnIn,
    /// `TurnInArenaPetition(...)`: the bags' arena charter of that size, with its banner.
    VendorTurnInArena(ArenaBannerChoice),
    /// `ClickPetitionButton()`: the cursor's item onto the vendor's petition button.
    VendorClickButton,
    /// `ClosePetitionVendor()`: the vendor's interaction ends.
    CloseVendor,
}

impl super::UiScript {
    /// Replace the charter snapshot; the app fires the four events and defers `PETITION_SHOW`.
    pub fn set_petition(&mut self, state: PetitionState) {
        set_field!(self, petition, state);
    }

    /// Replace the petition vendor's snapshot; the app fires its three events.
    pub fn set_petition_vendor(&mut self, state: PetitionVendorState) {
        set_field!(self, petition_vendor, state);
    }

    /// Take the charter intents queued since the last drain.
    pub fn take_petition_requests(&mut self) -> Vec<PetitionRequest> {
        drain_field!(self, petition_requests)
    }

    /// Drop the queued close intents, returning how many. Firing `PETITION_CLOSED` runs `OnHide`,
    /// whose `ClosePetition()` queues a close; on a switch to a newly offered charter that close
    /// would shut the new window and decline it on the wire. A user's close survives, as the feed
    /// runs `before(UiInput)` and a click's `OnHide` queues after this.
    pub fn drop_petition_close_intents(&mut self) -> usize {
        let requests = &mut self.model_mut().petition_requests;
        let before = requests.len();
        requests.retain(|r| {
            !matches!(
                r,
                PetitionRequest::CloseRegistrar | PetitionRequest::ClosePetition
            )
        });
        before - requests.len()
    }

    /// Queue one charter intent directly: the test seam.
    #[cfg(test)]
    pub fn queue_petition_request(&mut self, request: PetitionRequest) {
        self.model_mut().petition_requests.push(request);
    }
}

/// The guild-name check `BuyGuildCharter` (`0x59a130`) and `RenamePetition` (`0x5cf450`) share:
/// [`crate::names::check_guild_name`], whose refusal shows through `DisplayNameError 0x5218c0`.
/// `Err` carries that message's key, and no packet is built.
pub fn validate_guild_name(name: &str) -> Result<(), &'static str> {
    crate::names::check_guild_name(name).map_err(crate::names::NameError::key)
}

/// A cached name as a string, an uncached one as nil.
fn name_value(lua: &Lua, name: Option<&String>) -> mlua::Result<Value> {
    match name {
        Some(n) => Ok(Value::String(lua.create_string(n)?)),
        None => Ok(Value::Nil),
    }
}

/// Register the charter globals against the snapshot store.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ── The petition window ──────────────────────────────────────────────────────────────────
    // GetPetitionInfo() (`0x5cece0`): seven values on both legs, in the order `PetitionFrame.lua`
    // destructures them; with no record, `nil, nil, nil, 0, nil, nil, 0`.
    g.set(
        "GetPetitionInfo",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let Some(r) = model.petition.record.as_ref() else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Integer(0),
                    Value::Nil,
                    Value::Nil,
                    Value::Integer(0),
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&r.petition_type)?),
                Value::String(lua.create_string(&r.title)?),
                Value::String(lua.create_string(&r.body_text)?),
                Value::Integer(i64::from(r.max_signatures)),
                name_value(lua, r.originator.as_ref())?,
                binding_abi::flag(r.is_originator),
                Value::Integer(i64::from(r.min_signatures)),
            ]))
        })?,
    )?;

    // GetNumPetitionNames() (`0x5ceac0`): signatures only, never the owner.
    g.set(
        "GetNumPetitionNames",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.petition.signers.len() as i64)
        })?,
    )?;

    // GetPetitionNameInfo(index) (`0x5cee30`): 1-based, one value on every leg. The bound test is
    // unsigned, so 0 or a negative index wraps and answers nil.
    g.set(
        "GetPetitionNameInfo",
        lua.create_function(|lua, index: i64| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let Some(name) = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| model.petition.signers.get(i))
            else {
                return Ok(Value::Nil);
            };
            name_value(lua, name.as_ref())
        })?,
    )?;

    // CanSignPetition() (`0x5cf040`), the Sign button's only gate, is `1` with no petition open:
    // it skips the record refusals into a scan of the empty signer list. The window's
    // `isOriginator` branch is what hides the button.
    g.set(
        "CanSignPetition",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(binding_abi::flag(model.petition.can_sign))
        })?,
    )?;

    // ── The registrar window ─────────────────────────────────────────────────────────────────
    // GetGuildCharterCost() (`0x599f40`): copper, unsigned, 0 before any showlist.
    g.set(
        "GetGuildCharterCost",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(model.petition.charter_cost))
        })?,
    )?;

    // ── The verbs ────────────────────────────────────────────────────────────────────────────
    // The five that carry and push nothing; none may touch the snapshot, which the server owns.
    // `GetTabardInfo`'s stock caller is commented out (`GuildRegistrarFrame.xml:190`).
    for (global, request) in [
        ("TurnInGuildCharter", PetitionRequest::TurnIn),
        ("CloseGuildRegistrar", PetitionRequest::CloseRegistrar),
        ("GetTabardInfo", PetitionRequest::TabardInfo),
        ("OfferPetition", PetitionRequest::Offer),
        ("ClosePetition", PetitionRequest::ClosePetition),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, ()| {
                let mut model = crate::script::model_mut_of(lua);
                model.petition_requests.push(request.clone());
                Ok(())
            })?,
        )?;
    }

    // SignPetition([n]) (`0x5cf140`): the optional argument rides the wire as a byte defaulting to
    // 1, not 0; the server skips it.
    g.set(
        "SignPetition",
        lua.create_function(|lua, n: Option<f64>| {
            let mut model = crate::script::model_mut_of(lua);
            // `trunc` toward zero, then narrowed the way the `Put8` does.
            let byte = n.map_or(1i8, |v| v.trunc() as i64 as i8);
            model.petition_requests.push(PetitionRequest::Sign(byte));
            Ok(())
        })?,
    )?;

    // BuyGuildCharter(guildName) (`0x59a130`) returns 1 or nil for name validity, not for a send:
    // it runs the shared validator, and the action behind it (`0x59a050`) refuses silently.
    g.set(
        "BuyGuildCharter",
        lua.create_function(|lua, name: String| {
            let mut model = crate::script::model_mut_of(lua);
            match validate_guild_name(&name) {
                Ok(()) => {
                    model.petition_requests.push(PetitionRequest::Buy(name));
                    Ok(Value::Integer(1))
                }
                Err(key) => {
                    model
                        .petition_requests
                        .push(PetitionRequest::NameRefused(key));
                    Ok(Value::Nil)
                }
            }
        })?,
    )?;

    // RenamePetition(name) (`0x5cf450`): zero values, the same validator, silent beyond its
    // message. A bad argument raises mlua's coercion error; the reference's usage string is
    // `Usage(RenamePetition("name")`, unbalanced.
    g.set(
        "RenamePetition",
        lua.create_function(|lua, name: String| {
            let mut model = crate::script::model_mut_of(lua);
            match validate_guild_name(&name) {
                Ok(()) => model.petition_requests.push(PetitionRequest::Rename(name)),
                Err(key) => model
                    .petition_requests
                    .push(PetitionRequest::NameRefused(key)),
            }
            Ok(())
        })?,
    )?;

    // ── The petition vendor ──────────────────────────────────────────────────────────────────
    g.set(
        "GetNumPetitionItems",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.petition_vendor.items.len() as i64)
        })?,
    )?;

    // GetPetitionItemInfo(index) (`0x5a1fb0`): name, texture, cost; `nil, nil, 0` off the rows or
    // while the item is uncached.
    g.set(
        "GetPetitionItemInfo",
        lua.create_function(|lua, index: Value| {
            let index = (binding_abi::number_arg(lua, index, "Usage: GetPetitionItemInfo(index)")?
                as u32)
                .wrapping_sub(1);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let item = model
                .petition_vendor
                .items
                .get(index as usize)
                .and_then(|i| Some((i.name.as_ref()?, i.texture.as_ref()?, i.cost)));
            let Some((name, texture, cost)) = item else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Number(0.0),
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(name)?),
                Value::String(lua.create_string(texture)?),
                Value::Number(f64::from(cost)),
            ]))
        })?,
    )?;

    // BuyPetition(index, name) (`0x5a2530`): the guild validator, then 1 for a valid name.
    g.set(
        "BuyPetition",
        lua.create_function(|lua, (index, name): (Value, Value)| {
            let usage = "Usage: BuyPetition(index, name)";
            let index = (binding_abi::number_arg(lua, index, usage)? as u32).wrapping_sub(1);
            let name = binding_abi::string_arg(lua, name, usage)?;
            let mut model = crate::script::model_mut_of(lua);
            match validate_guild_name(&name) {
                Ok(()) => {
                    model
                        .petition_requests
                        .push(PetitionRequest::VendorBuy { index, name });
                    Ok(Value::Integer(1))
                }
                Err(key) => {
                    model
                        .petition_requests
                        .push(PetitionRequest::NameRefused(key));
                    Ok(Value::Nil)
                }
            }
        })?,
    )?;

    // TurnInArenaPetition(teamSize [, r, g, b, emblem, r, g, b, border, r, g, b]) (`0x5a26d0`):
    // with all eleven numbers the colours pack as opaque `0xAARRGGBB`, else every word is
    // `0xFFFFFFFF`; nothing without a vendor.
    g.set(
        "TurnInArenaPetition",
        lua.create_function(|lua, args: MultiValue| {
            let mut args: Vec<Value> = args.into_iter().collect();
            args.resize(12, Value::Nil);
            let team_size = binding_abi::number_arg(
                lua,
                args[0].clone(),
                "Usage: TurnInArenaPetition(teamSize)",
            )? as u32;
            let numbers: Vec<Option<f64>> = args[1..12]
                .iter()
                .map(|v| lua.coerce_number(v.clone()).ok().flatten())
                .collect();
            let mut model = crate::script::model_mut_of(lua);
            if !model.petition_vendor.open {
                return Ok(());
            }
            let banner = if numbers.iter().all(Option::is_some) {
                let v: Vec<f64> = numbers.into_iter().flatten().collect();
                let color = |at: usize| pack_color(v[at], v[at + 1], v[at + 2]);
                ArenaBannerChoice {
                    team_size,
                    background_color: color(0),
                    emblem_style: binding_abi::ftol(v[3]) as u32,
                    emblem_color: color(4),
                    border_style: binding_abi::ftol(v[7]) as u32,
                    border_color: color(8),
                }
            } else {
                ArenaBannerChoice {
                    team_size,
                    background_color: u32::MAX,
                    emblem_style: u32::MAX,
                    emblem_color: u32::MAX,
                    border_style: u32::MAX,
                    border_color: u32::MAX,
                }
            };
            model
                .petition_requests
                .push(PetitionRequest::VendorTurnInArena(banner));
            Ok(())
        })?,
    )?;

    // HasFilledPetition() (`0x5a2820`): 1 or nil.
    g.set(
        "HasFilledPetition",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(binding_abi::flag(model.petition_vendor.has_filled_petition))
        })?,
    )?;

    // ClosePetitionVendor() (`0x5a24f0`): with a vendor open, it closes and fires
    // `PETITION_VENDOR_CLOSED`.
    g.set(
        "ClosePetitionVendor",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            if !model.petition_vendor.open {
                return Ok(());
            }
            model.petition_vendor.open = false;
            model
                .pending_events
                .push(("PETITION_VENDOR_CLOSED".to_string(), Vec::new()));
            model.petition_requests.push(PetitionRequest::CloseVendor);
            Ok(())
        })?,
    )?;

    for (global, request) in [
        ("TurnInPetition", PetitionRequest::VendorTurnIn),
        ("ClickPetitionButton", PetitionRequest::VendorClickButton),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, ()| {
                let mut model = crate::script::model_mut_of(lua);
                if model.petition_vendor.open {
                    model.petition_requests.push(request.clone());
                }
                Ok(())
            })?,
        )?;
    }

    Ok(())
}

/// `FrameScript_GetColorNoAlpha` into `CImVector::Set` (`0x5a2793`): each channel clamped to
/// `[0, 1]` and rounded as `(int)(f * 255 + 0.5)` in single precision, alpha opaque.
fn pack_color(r: f64, g: f64, b: f64) -> u32 {
    let byte = |c: f64| {
        let c = (c as f32).clamp(0.0, 1.0);
        (c * 255.0 + 0.5) as u32 & 0xFF
    };
    0xFF00_0000 | byte(r) << 16 | byte(g) << 8 | byte(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn record() -> PetitionRecordView {
        PetitionRecordView {
            petition_type: petition_type_name(0).into(),
            title: "Legacy".into(),
            body_text: String::new(),
            max_signatures: 9,
            originator: Some("Founder".into()),
            is_originator: false,
            min_signatures: 4,
        }
    }

    fn open(script: &mut UiScript, r: PetitionRecordView, signers: Vec<Option<String>>) {
        script.set_petition(PetitionState {
            charter_cost: 1000,
            signers,
            record: Some(r),
            can_sign: true,
        });
    }

    /// The seven returns, in the reference's own destructuring order.
    #[test]
    fn get_petition_info_returns_the_reference_seven_in_order() {
        let mut s = UiScript::new().unwrap();
        open(&mut s, record(), vec![]);
        let (kind, title, body, max, originator, is_originator, min) = s
            .eval::<(String, String, String, i32, String, Option<u32>, i32)>(
                "return GetPetitionInfo()",
            )
            .unwrap();
        assert_eq!(kind, "guild");
        assert_eq!(min, 4);
        assert_eq!(title, "Legacy");
        assert_eq!(body, "");
        assert_eq!(max, 9);
        assert_eq!(originator, "Founder");
        assert_eq!(is_originator, None, "era nil, not false");

        open(
            &mut s,
            PetitionRecordView {
                is_originator: true,
                ..record()
            },
            vec![],
        );
        assert_eq!(
            s.eval::<Option<u32>>("local _,_,_,_,_,o = GetPetitionInfo(); return o")
                .unwrap(),
            Some(1),
            "era 1, not true"
        );
    }

    #[test]
    fn the_no_record_leg_is_seven_values_with_numeric_zeros() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.arity("GetPetitionInfo()").unwrap(),
            7,
            "seven values even with nothing open"
        );
        assert_eq!(
            s.eval::<i64>("local _,_,_,m = GetPetitionInfo(); return m")
                .unwrap(),
            0,
            "the fourth is the number 0, not nil"
        );
        assert_eq!(
            s.eval::<Option<String>>("return (GetPetitionInfo())")
                .unwrap(),
            None,
            "…and the first really is nil"
        );
    }

    /// The zero and negative indices matter because the reference's bound test is unsigned.
    #[test]
    fn petition_names_are_one_based_and_uncached_reads_nil() {
        let mut s = UiScript::new().unwrap();
        open(
            &mut s,
            record(),
            vec![Some("Aaa".into()), None, Some("Ccc".into())],
        );
        assert_eq!(
            s.eval::<i64>("return GetNumPetitionNames()").unwrap(),
            3,
            "three signers — the owner is not one of them"
        );
        assert_eq!(
            s.eval::<String>("return GetPetitionNameInfo(1)").unwrap(),
            "Aaa"
        );
        assert_eq!(
            s.eval::<Option<String>>("return GetPetitionNameInfo(2)")
                .unwrap(),
            None,
            "an unresolved name is nil, never an empty string"
        );
        assert_eq!(
            s.eval::<String>("return GetPetitionNameInfo(3)").unwrap(),
            "Ccc",
            "and it keeps its ROW — the list does not close up around it"
        );
        for out_of_range in ["0", "4", "-1"] {
            assert_eq!(
                s.eval::<Option<String>>(&format!("return GetPetitionNameInfo({out_of_range})"))
                    .unwrap(),
                None,
                "index {out_of_range} answers nil"
            );
        }
    }

    /// The `1` with nothing open is the reference's own asymmetry, not a bug to fix.
    #[test]
    fn can_sign_petition_answers_one_with_nothing_open() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<Option<u32>>("return CanSignPetition()").unwrap(),
            None,
            "the app's own default is a refusal…"
        );

        let mut s = UiScript::new().unwrap();
        s.set_petition(PetitionState {
            can_sign: true,
            ..PetitionState::default()
        });
        assert_eq!(
            s.eval::<Option<u32>>("return CanSignPetition()").unwrap(),
            Some(1),
            "…but the binding reports whatever the app computed, including the no-record 1"
        );
    }

    #[test]
    fn charter_cost_is_unsigned_copper_and_the_registrars() {
        let mut s = UiScript::new().unwrap();
        s.set_petition(PetitionState {
            charter_cost: 1000,
            ..PetitionState::default()
        });
        assert_eq!(s.eval::<i64>("return GetGuildCharterCost()").unwrap(), 1000);

        // A negative wire cost reads ~4.29e9: the binding zero-extends it (`fild QWORD`).
        s.set_petition(PetitionState {
            charter_cost: (-1i32) as u32,
            ..PetitionState::default()
        });
        assert_eq!(
            s.eval::<i64>("return GetGuildCharterCost()").unwrap(),
            4_294_967_295
        );

        s.set_petition(PetitionState::default());
        assert_eq!(s.eval::<i64>("return GetGuildCharterCost()").unwrap(), 0);
    }

    #[test]
    fn sign_petition_defaults_its_wire_byte_to_one() {
        let mut s = UiScript::new().unwrap();
        s.run("SignPetition(); SignPetition(7); SignPetition(3.9)")
            .unwrap();
        assert_eq!(
            s.take_petition_requests(),
            vec![
                PetitionRequest::Sign(1),
                PetitionRequest::Sign(7),
                PetitionRequest::Sign(3),
            ],
            "absent = 1, present = truncated toward zero"
        );
    }

    /// A refused name queues its message instead of a packet.
    #[test]
    fn the_name_validator_gates_both_verbs_and_only_buy_reports_it() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<Option<u32>>("return BuyGuildCharter(\"Legacy\")")
                .unwrap(),
            Some(1)
        );
        assert_eq!(
            s.eval::<Option<u32>>("return BuyGuildCharter(\"\")")
                .unwrap(),
            None,
            "an empty name is refused locally — the server answers it with silence"
        );
        assert_eq!(
            s.arity("RenamePetition(\"Legacy\")").unwrap(),
            0,
            "rename pushes nothing at all"
        );
        s.run("RenamePetition(\"  spaced  \")").unwrap();
        assert_eq!(
            s.take_petition_requests(),
            vec![
                PetitionRequest::Buy("Legacy".into()),
                PetitionRequest::NameRefused("ERR_NAME_NO_NAME"),
                PetitionRequest::Rename("Legacy".into()),
                PetitionRequest::NameRefused("ERR_NAME_INVALID_SPACE"),
            ]
        );
    }

    #[test]
    fn a_refused_guild_name_shows_its_name_error() {
        assert_eq!(validate_guild_name(""), Err("ERR_NAME_NO_NAME"));
        assert_eq!(validate_guild_name("A"), Err("ERR_NAME_TOO_SHORT"));
        assert_eq!(
            validate_guild_name(" Legacy"),
            Err("ERR_NAME_INVALID_SPACE")
        );
        assert_eq!(
            validate_guild_name("Legacy  of Steel"),
            Err("ERR_NAME_CONSECUTIVE_SPACES")
        );
        assert_eq!(validate_guild_name("Legacy's"), Err("ERR_NAME_INVALID"));
        for ok in ["Legacy of Steel", "Ab", "Éclair", "x y z"] {
            assert_eq!(validate_guild_name(ok), Ok(()), "{ok:?}");
        }
    }

    /// `GetTabardInfo()` pushes nothing and queues the designer's opener.
    #[test]
    fn get_tabard_info_is_an_intent_with_no_returns() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<i64>("local n = function(...) return arg.n end return n(GetTabardInfo())")
                .unwrap(),
            0
        );
        assert_eq!(
            s.take_petition_requests(),
            vec![PetitionRequest::TabardInfo]
        );
    }

    #[test]
    fn the_vendor_answers_its_rows_and_packs_the_banner() {
        let mut s = UiScript::new().unwrap();
        s.set_petition_vendor(PetitionVendorState {
            open: true,
            items: vec![
                PetitionVendorItem {
                    name: Some("Arena Team Charter".into()),
                    texture: Some("Interface\\Icons\\INV_Misc_Note_01".into()),
                    cost: 800_000,
                },
                PetitionVendorItem::default(),
            ],
            has_filled_petition: true,
        });
        assert_eq!(s.eval::<i64>("return GetNumPetitionItems()").unwrap(), 2);
        let got = s
            .eval::<String>("local n, t, c = GetPetitionItemInfo(1) return n .. '|' .. c")
            .unwrap();
        assert_eq!(got, "Arena Team Charter|800000");
        assert!(s
            .eval::<bool>("local n, t, c = GetPetitionItemInfo(2) return n == nil and c == 0")
            .unwrap());
        assert_eq!(s.eval::<i64>("return HasFilledPetition()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return BuyPetition(1, 'Probes')").unwrap(), 1);
        s.run("TurnInArenaPetition(2, 1, 0.5, 0, 7, 0, 0, 1, 3, 0.2, 0.2, 0.2) TurnInArenaPetition(3)")
            .unwrap();
        assert_eq!(
            s.take_petition_requests(),
            vec![
                PetitionRequest::VendorBuy {
                    index: 0,
                    name: "Probes".into()
                },
                PetitionRequest::VendorTurnInArena(ArenaBannerChoice {
                    team_size: 2,
                    background_color: 0xFF_FF_80_00,
                    emblem_style: 7,
                    emblem_color: 0xFF_00_00_FF,
                    border_style: 3,
                    border_color: 0xFF_33_33_33,
                }),
                PetitionRequest::VendorTurnInArena(ArenaBannerChoice {
                    team_size: 3,
                    background_color: u32::MAX,
                    emblem_style: u32::MAX,
                    emblem_color: u32::MAX,
                    border_style: u32::MAX,
                    border_color: u32::MAX,
                }),
            ]
        );
        s.run("ClosePetitionVendor() ClosePetitionVendor()")
            .unwrap();
        assert_eq!(
            s.take_petition_requests(),
            vec![PetitionRequest::CloseVendor]
        );
    }

    #[test]
    fn dropping_close_intents_leaves_the_other_verbs_untouched() {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"
            SignPetition()
            ClosePetition()
            BuyGuildCharter("Legacy")
            CloseGuildRegistrar()
            OfferPetition()
        "#,
        )
        .unwrap();
        assert_eq!(s.drop_petition_close_intents(), 2);
        assert_eq!(
            s.take_petition_requests(),
            vec![
                PetitionRequest::Sign(1),
                PetitionRequest::Buy("Legacy".into()),
                PetitionRequest::Offer,
            ],
        );
        assert_eq!(s.drop_petition_close_intents(), 0);
    }
}
