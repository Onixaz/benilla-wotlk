//! The charter globals: the guild registrar (`0x599f00`..), the arena registrar's petition vendor
//! (`0x5a1f70`..) and the petition window (`0x5ceac0`..). The snapshot keeps the signer list and
//! the cached petition record apart, as the reference does, since the bindings read them
//! independently. Predicates are `1` or nil and an unresolved name is nil. `PETITION_SHOW` waits
//! for every signer name and the record, a deferral `crate::ui_petition` owns.

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

/// The cached petition record, as `GetPetitionInfo` reads it; `None` in
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
    /// A vendor is open (`0xc0f7c8`).
    pub open: bool,
    pub items: Vec<PetitionVendorItem>,
    /// `HasFilledPetition()` (`0x5a2820`): the bags hold an arena charter whose record is cached.
    pub has_filled_petition: bool,
}

/// The item on the petition vendor's button: its guid (`0xc0f7d8`) and the bag and 1-based slot
/// the cursor carried it from (`0xc0f7e0`, `0xc0f7d4`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PetitionButtonItem {
    pub item: u64,
    pub bag: i64,
    pub slot: u32,
}

/// An item template's petition flag, the one item the button accepts (`0x5a21af`).
const ITEM_FLAG_PETITION: u32 = 0x2000;

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
    /// latched, not the game UI's interaction pair.
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
    /// `OfferPetition()`: the app resolves the current target (the game UI's selection pair, not
    /// its interaction pair) and runs the eight guards.
    Offer,
    /// `RenamePetition(name)`, after [`validate_guild_name`].
    Rename(String),
    /// `ClosePetition()`: the reference sends `MSG_PETITION_DECLINE` when a petition was open, no
    /// sign is in flight, a record is cached and we do not own it; the app decides.
    ClosePetition,
    /// A name [`validate_guild_name`] refused, with the GlobalStrings key to show; no packet.
    NameRefused(&'static str),
    /// `RenamePetition`'s refused name: shown only with a petition open (`0x5cf491`).
    RenameRefused(&'static str),
    /// `HasFilledPetition()`: the arena charter walk without a turn-in (`0x5a2820`).
    HasFilled,
    /// `BuyPetition(index, name)` at a petition vendor, the row 0-based, after the validator.
    VendorBuy { index: u32, name: String },
    /// `TurnInPetition()`: the item the petition button holds, with an all-ones banner.
    VendorTurnIn { item: u64 },
    /// `TurnInArenaPetition(...)`: the bags' arena charter of that size, with its banner.
    VendorTurnInArena(ArenaBannerChoice),
    /// `ClosePetitionVendor()`: the vendor's interaction ends.
    CloseVendor,
}

/// The event the registrar's close fires, from `CloseGuildRegistrar` and `GetTabardInfo` inside their call (`0x599f00`, `0x59a1f0`).
pub const GUILD_REGISTRAR_CLOSED_EVENT: &str = "GUILD_REGISTRAR_CLOSED";

/// The event the vendor's close fires, from `ClosePetitionVendor` inside its call (`0x5a24f0`).
pub const PETITION_VENDOR_CLOSED_EVENT: &str = "PETITION_VENDOR_CLOSED";

/// A charter verb's step inside its call, in the order 12340 takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PetitionStep {
    /// An event fired inside the call.
    Fire(&'static str),
    /// A message line shown inside the call, with its argument.
    Line(&'static str, PetitionArg),
}

/// A charter line's one argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PetitionArg {
    None,
    S(String),
    D(i64),
}

/// What a charter verb did inside its call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PetitionOutcome {
    pub steps: Vec<PetitionStep>,
    /// The arena charter walk's answer (`0x6dd680`), `HasFilledPetition`'s.
    pub filled: bool,
}

impl super::UiScript {
    /// A test's charter window, which the verbs read where the lent view does not answer it.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn set_petition(&mut self, state: PetitionState) {
        set_field!(self, petition, state);
    }

    /// A test's petition vendor, which the verbs read where the lent view does not answer it.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn set_petition_vendor(&mut self, state: PetitionVendorState) {
        set_field!(self, petition_vendor, state);
    }

    /// The petition list's show and the world entry (`0x5a22cb`, `0x5a1f57`): the button forgets
    /// its item without unlocking it; the app keeps the returned item locked until its update.
    pub fn clear_petition_button(&mut self) -> Option<PetitionButtonItem> {
        let held = self.model_ref().petition_button?;
        // The app keeps the slot's lock at once, read by the bag verbs at the call.
        self.model_mut().petition_button = None;
        Some(held)
    }
}

/// The guild-name check `BuyGuildCharter` (`0x59a130`) and `RenamePetition` (`0x5cf450`) share:
/// [`crate::names::check_guild_name`], whose refusal shows through the name-error display
/// (`0x5218c0`).
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

/// The charter window through the lent view at the call; with none, a test's, else none.
fn with_petition<R>(lua: &Lua, f: impl FnOnce(&PetitionState) -> R) -> R {
    if let Some(Some(view)) = super::live::read_live(|l| l.petition_view()) {
        return f(&view);
    }
    #[cfg(any(test, feature = "fixtures"))]
    {
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model app_data");
        f(&model.petition)
    }
    #[cfg(not(any(test, feature = "fixtures")))]
    {
        let _ = lua;
        f(&PetitionState::default())
    }
}

/// Whether a petition vendor is set (`0xc0f7c8`), through the lent view; with none, a test's, else none.
fn vendor_open(lua: &Lua) -> bool {
    if let Some(Some(open)) = super::live::read_live(|l| l.petition_vendor_open()) {
        return open;
    }
    test_vendor(lua, |v| v.open).unwrap_or(false)
}

/// A test's petition vendor, where the lent view does not answer it.
#[cfg(any(test, feature = "fixtures"))]
fn test_vendor<R>(lua: &Lua, f: impl FnOnce(&PetitionVendorState) -> R) -> Option<R> {
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    Some(f(&model.petition_vendor))
}

/// No test vendor outside a test.
#[cfg(not(any(test, feature = "fixtures")))]
fn test_vendor<R>(_: &Lua, _: impl FnOnce(&PetitionVendorState) -> R) -> Option<R> {
    None
}

/// Run `act` inside the call through the lent view, its steps after; with none lent there is no charter state to act on.
fn petition_act(lua: &Lua, act: PetitionRequest) -> PetitionOutcome {
    let Some(Some(out)) = super::live::write_live(|l| l.petition_act(act)) else {
        return PetitionOutcome::default();
    };
    for step in &out.steps {
        match step {
            PetitionStep::Fire(event) => {
                super::tick::fire_event_into(lua, event, Vec::new());
            }
            PetitionStep::Line(key, arg) => super::ui_errors::show_now_arg(lua, key, arg),
        }
    }
    out
}

/// Register the charter globals, which read the lent view at the call.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ── The petition window ──────────────────────────────────────────────────────────────────
    // GetPetitionInfo() (`0x5cece0`): seven values on both legs, in the order `PetitionFrame.lua`
    // destructures them; with no record held (`0xc22b40`), `nil, nil, nil, 0, nil, nil, 0`.
    g.set(
        "GetPetitionInfo",
        lua.create_function(|lua, ()| {
            let Some(r) = with_petition(lua, |p| p.record.clone()) else {
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
        lua.create_function(|lua, ()| Ok(with_petition(lua, |p| p.signers.len() as i64)))?,
    )?;

    // GetPetitionNameInfo(index) (`0x5cee30`): 1-based, one value on every leg. The bound test is
    // unsigned, so 0 or a negative index wraps and answers nil.
    g.set(
        "GetPetitionNameInfo",
        lua.create_function(|lua, index: i64| {
            let name = with_petition(lua, |p| {
                usize::try_from(index)
                    .ok()
                    .and_then(|i| i.checked_sub(1))
                    .and_then(|i| p.signers.get(i).cloned())
            });
            match name {
                Some(name) => name_value(lua, name.as_ref()),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // CanSignPetition() (`0x5cf040`), the Sign button's only gate, is `1` with no petition open:
    // it skips the record refusals into a scan of the empty signer list. The window's
    // `isOriginator` branch is what hides the button.
    g.set(
        "CanSignPetition",
        lua.create_function(|lua, ()| Ok(binding_abi::flag(with_petition(lua, |p| p.can_sign))))?,
    )?;

    // ── The registrar window ─────────────────────────────────────────────────────────────────
    // GetGuildCharterCost() (`0x599f40`): the registrar row's copper, unsigned, kept past the close
    // until the world entry (`0x599dd0`).
    g.set(
        "GetGuildCharterCost",
        lua.create_function(|lua, ()| Ok(with_petition(lua, |p| i64::from(p.charter_cost))))?,
    )?;

    // ── The verbs, each acting inside its call ───────────────────────────────────────────────
    // `GetTabardInfo`'s stock caller is commented out (`GuildRegistrarFrame.xml:190`).
    for (global, request) in [
        ("TurnInGuildCharter", PetitionRequest::TurnIn),
        ("CloseGuildRegistrar", PetitionRequest::CloseRegistrar),
        ("GetTabardInfo", PetitionRequest::TabardInfo),
        ("OfferPetition", PetitionRequest::Offer),
        ("ClosePetition", PetitionRequest::ClosePetition),
        ("ClosePetitionVendor", PetitionRequest::CloseVendor),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, ()| {
                petition_act(lua, request.clone());
                Ok(())
            })?,
        )?;
    }

    // SignPetition([n]) (`0x5cf140`): the optional argument rides the wire as a byte defaulting to
    // 1, not 0; the server skips it.
    g.set(
        "SignPetition",
        lua.create_function(|lua, n: Option<f64>| {
            // `trunc` toward zero, then narrowed the way the `Put8` does.
            let byte = n.map_or(1i8, |v| v.trunc() as i64 as i8);
            petition_act(lua, PetitionRequest::Sign(byte));
            Ok(())
        })?,
    )?;

    // BuyGuildCharter(guildName) (`0x59a130`) returns 1 or nil for name validity, not for a send:
    // it runs the shared validator, and the action behind it (`0x59a050`) refuses on its own.
    g.set(
        "BuyGuildCharter",
        lua.create_function(|lua, name: String| match validate_guild_name(&name) {
            Ok(()) => {
                petition_act(lua, PetitionRequest::Buy(name));
                Ok(Value::Integer(1))
            }
            Err(key) => {
                petition_act(lua, PetitionRequest::NameRefused(key));
                Ok(Value::Nil)
            }
        })?,
    )?;

    // RenamePetition(name) (`0x5cf450`): zero values; nothing with no petition open, else the same
    // validator, silent beyond its message. A bad argument raises mlua's coercion error; the
    // reference's usage string is `Usage(RenamePetition("name")`, unbalanced.
    g.set(
        "RenamePetition",
        lua.create_function(|lua, name: String| {
            let act = match validate_guild_name(&name) {
                Ok(()) => PetitionRequest::Rename(name),
                Err(key) => PetitionRequest::RenameRefused(key),
            };
            petition_act(lua, act);
            Ok(())
        })?,
    )?;

    // ── The petition vendor ──────────────────────────────────────────────────────────────────
    // GetNumPetitionItems() (`0x5a1f70`): the last list's count, kept past the close.
    g.set(
        "GetNumPetitionItems",
        lua.create_function(|lua, ()| {
            if let Some(Some(n)) = super::live::read_live(|l| l.petition_vendor_count()) {
                return Ok(n as i64);
            }
            Ok(test_vendor(lua, |v| v.items.len()).unwrap_or(0) as i64)
        })?,
    )?;

    // GetPetitionItemInfo(index) (`0x5a1fb0`): name, texture, cost; `nil, nil, 0` off the rows or
    // while the item is uncached, whose ask leaves the vendor's record callback (`0x5a1f30`).
    g.set(
        "GetPetitionItemInfo",
        lua.create_function(|lua, index: Value| {
            let index = (binding_abi::number_arg(lua, index, "Usage: GetPetitionItemInfo(index)")?
                as u32)
                .wrapping_sub(1);
            let item = match super::live::write_live(|l| l.petition_item(index)) {
                Some(Some(item)) => item,
                _ => test_vendor(lua, |v| v.items.get(index as usize).cloned()).flatten(),
            };
            let item = item.and_then(|i| Some((i.name?, i.texture?, i.cost)));
            let Some((name, texture, cost)) = item else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Number(0.0),
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&name)?),
                Value::String(lua.create_string(&texture)?),
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
            match validate_guild_name(&name) {
                Ok(()) => {
                    petition_act(lua, PetitionRequest::VendorBuy { index, name });
                    Ok(Value::Integer(1))
                }
                Err(key) => {
                    petition_act(lua, PetitionRequest::NameRefused(key));
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
            if super::live::read_live(|_| ()).is_none() && !vendor_open(lua) {
                return Ok(());
            }
            petition_act(lua, PetitionRequest::VendorTurnInArena(banner));
            Ok(())
        })?,
    )?;

    // HasFilledPetition() (`0x5a2820`): the arena charter walk without a turn-in, 1 or nil, with
    // or without a vendor.
    g.set(
        "HasFilledPetition",
        lua.create_function(|lua, ()| {
            if let Some(Some(out)) =
                super::live::write_live(|l| l.petition_act(PetitionRequest::HasFilled))
            {
                return Ok(binding_abi::flag(out.filled));
            }
            Ok(binding_abi::flag(
                test_vendor(lua, |v| v.has_filled_petition).unwrap_or(false),
            ))
        })?,
    )?;

    // TurnInPetition() (`0x5a2600`): with a vendor open, the button's item with five all-ones
    // banner words; nothing with an empty button.
    g.set(
        "TurnInPetition",
        lua.create_function(|lua, ()| {
            let held = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data")
                .petition_button;
            if let Some(held) = held {
                if super::live::read_live(|_| ()).is_none() && !vendor_open(lua) {
                    return Ok(());
                }
                petition_act(lua, PetitionRequest::VendorTurnIn { item: held.item });
            }
            Ok(())
        })?,
    )?;

    // ClickPetitionButton() (`0x5a2200`): the cursor's petition onto the button, the cursor
    // cleared and any previous item handed back onto it.
    g.set(
        "ClickPetitionButton",
        lua.create_function(|lua, ()| {
            let open = vendor_open(lua);
            click_petition_button(&mut crate::script::model_mut_of(lua), open);
            Ok(())
        })?,
    )?;

    Ok(())
}

/// Whether the button holds the item in `(bag, slot)`, which stays locked as the pickup left it.
pub(super) fn holds_slot(model: &super::Model, bag: i64, slot: u32) -> bool {
    model.petition_button.is_some_and(|b| {
        b.bag == bag
            && b.slot == slot
            && super::container::live_slot(model, bag, slot).is_some_and(|s| s.guid == b.item)
    })
}

/// The button's click (`0x5a2140`): no vendor, or a cursor item without the petition flag, does
/// nothing. An empty cursor empties the button; either way the cursor clears without unlocking
/// its item (`ClearCursor(0, 1)`) and the previous item goes back onto it.
fn click_petition_button(model: &mut super::Model, vendor_open: bool) {
    use super::cursor::{CursorItem, CursorPayload};
    if !vendor_open {
        return;
    }
    let previous = model.petition_button;
    let held = match &model.cursor {
        Some(CursorPayload::Item(item)) => Some(item.clone()),
        _ => None,
    };
    match &held {
        None => model.petition_button = None,
        Some(item) => {
            let Some(guid) = super::container::live_slot(model, item.bag, item.slot)
                .filter(|s| s.guid != 0 && s.item_id == item.item_id)
                .filter(|s| {
                    crate::script::item_stats::item_record(model, s.item_id)
                        .is_some_and(|t| t.flags & ITEM_FLAG_PETITION != 0)
                })
                .map(|s| s.guid)
            else {
                return;
            };
            model.petition_button = Some(PetitionButtonItem {
                item: guid,
                bag: item.bag,
                slot: item.slot,
            });
        }
    }
    model
        .pending_events
        .push(("PETITION_VENDOR_UPDATE".to_string(), Vec::new()));
    if held.is_some() {
        model.cursor = None;
        super::cursor::queue_cursor_update(model);
    } else {
        super::cursor::clear_cursor(model);
    }
    // The previous item back on the cursor, still locked (`0x5a2281`).
    let Some(prev) = previous else {
        return;
    };
    let back = super::container::live_slot(model, prev.bag, prev.slot)
        .filter(|s| s.guid == prev.item)
        .map(|s| CursorItem {
            bag: prev.bag,
            slot: prev.slot,
            item_id: s.item_id,
            texture: s.texture.clone(),
            link: s.link.clone(),
            count: None,
            quality: s.quality,
            equip_slots: s.equip_slots.clone(),
            bar_placeable: s.bar_placeable,
        });
    if let Some(item) = back {
        model.cursor = Some(CursorPayload::Item(item));
        super::cursor::queue_cursor_update(model);
    }
}

/// The script colour read into the packed colour (`0x5a2793`): each channel clamped to
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
    use crate::script::live::TestView;
    use crate::script::UiScript;

    /// A view that records the charter acts the verbs make inside their calls.
    fn acts() -> TestView {
        TestView::default()
    }

    /// The acts the lent view recorded since the last take.
    fn take() -> Vec<PetitionRequest> {
        crate::script::live::test_view(|v| std::mem::take(&mut v.petition_acts))
    }

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
        let s = UiScript::new().unwrap();
        let mut loan_v = acts();
        crate::script::live::lending(&mut loan_v, || {
            s.run("SignPetition(); SignPetition(7); SignPetition(3.9)")
                .unwrap();
            assert_eq!(
                take(),
                vec![
                    PetitionRequest::Sign(1),
                    PetitionRequest::Sign(7),
                    PetitionRequest::Sign(3),
                ],
                "absent = 1, present = truncated toward zero"
            );
        });
    }

    /// A refused name queues its message instead of a packet.
    #[test]
    fn the_name_validator_gates_both_verbs_and_only_buy_reports_it() {
        let s = UiScript::new().unwrap();
        let mut loan_v = acts();
        crate::script::live::lending(&mut loan_v, || {
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
                take(),
                vec![
                    PetitionRequest::Buy("Legacy".into()),
                    PetitionRequest::NameRefused("ERR_NAME_NO_NAME"),
                    PetitionRequest::Rename("Legacy".into()),
                    PetitionRequest::RenameRefused("ERR_NAME_INVALID_SPACE"),
                ]
            );
        });
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
        let s = UiScript::new().unwrap();
        let mut loan_v = acts();
        crate::script::live::lending(&mut loan_v, || {
            assert_eq!(
                s.eval::<i64>("local n = function(...) return arg.n end return n(GetTabardInfo())")
                    .unwrap(),
                0
            );
            assert_eq!(take(), vec![PetitionRequest::TabardInfo]);
        });
    }

    #[test]
    fn the_vendor_answers_its_rows_and_packs_the_banner() {
        let mut s = UiScript::new().unwrap();
        let mut loan_v = acts();
        crate::script::live::lending(&mut loan_v, || {
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
                take(),
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
            // Each close reaches the view inside its call; the vendor's own zero gates the second there (`0x5a24f0`).
            s.run("ClosePetitionVendor() ClosePetitionVendor()")
                .unwrap();
            assert_eq!(
                take(),
                vec![PetitionRequest::CloseVendor, PetitionRequest::CloseVendor]
            );
        });
    }

    /// A backpack with two charters (slots 1 and 2) and a plain item (slot 3), every template
    /// cached, and the vendor open.
    fn vendor_with_charters() -> UiScript {
        use crate::script::{ContainerSlot, ContainerState, ItemTemplateView};
        let mut s = UiScript::new().unwrap();
        let mut slots = std::collections::HashMap::new();
        for (slot, guid, item_id) in [(1, 0x71, 23560), (2, 0x72, 23561), (3, 0x73, 117)] {
            slots.insert(
                slot,
                ContainerSlot {
                    guid,
                    item_id,
                    count: 1,
                    ..Default::default()
                },
            );
        }
        s.set_container(
            0,
            Some(ContainerState {
                num_slots: 16,
                slots,
                ..Default::default()
            }),
        );
        for (item_id, flags) in [(23560, 0x2000), (23561, 0x2000), (117, 0)] {
            s.set_item_template(
                item_id,
                ItemTemplateView {
                    flags,
                    ..Default::default()
                },
            );
        }
        s.set_petition_vendor(PetitionVendorState {
            open: true,
            ..Default::default()
        });
        s
    }

    fn locked(s: &UiScript, slot: u32) -> bool {
        s.eval::<bool>(&format!(
            "local _, _, locked = GetContainerItemInfo(0, {slot}) return locked ~= nil"
        ))
        .unwrap()
    }

    fn held_slot(s: &UiScript) -> Option<u32> {
        s.cursor_item().map(|c| c.slot)
    }

    /// The button keeps the cursor's charter, still locked in its slot, and `TurnInPetition`
    /// sends it with no banner (`0x5a2140`, `0x5a2600`).
    #[test]
    fn the_petition_button_takes_a_charter_and_turns_it_in() {
        let s = vendor_with_charters();
        let mut loan_v = acts();
        crate::script::live::lending(&mut loan_v, || {
            s.run("TurnInPetition()").unwrap();
            assert!(take().is_empty(), "an empty button sends nothing");

            s.run("PickupContainerItem(0, 1)").unwrap();
            s.model_mut().pending_events.clear();
            s.run("ClickPetitionButton()").unwrap();
            let fired: Vec<String> = s
                .model_ref()
                .pending_events
                .iter()
                .map(|(e, _)| e.clone())
                .collect();
            assert!(
                fired.contains(&"PETITION_VENDOR_UPDATE".to_string()),
                "{fired:?}"
            );
            assert!(fired.contains(&"CURSOR_UPDATE".to_string()), "{fired:?}");
            assert!(
                !fired.contains(&"ITEM_LOCK_CHANGED".to_string()),
                "no unlock: {fired:?}"
            );
            assert_eq!(held_slot(&s), None, "the cursor cleared");
            assert!(locked(&s, 1), "the charter stays locked");
            s.run("PickupContainerItem(0, 1)").unwrap();
            assert_eq!(held_slot(&s), None, "a locked charter does not pick up");

            s.run("TurnInPetition()").unwrap();
            assert_eq!(take(), vec![PetitionRequest::VendorTurnIn { item: 0x71 }]);
        });
    }

    /// A second charter swaps the first back onto the cursor; an empty cursor takes the button's
    /// item back; a non-charter item, or no vendor, leaves everything as it was.
    #[test]
    fn the_petition_button_swaps_and_refuses_as_the_reference() {
        let mut s = vendor_with_charters();
        let mut loan_v = acts();
        crate::script::live::lending(&mut loan_v, || {
            s.run("PickupContainerItem(0, 1); ClickPetitionButton()")
                .unwrap();
            s.run("PickupContainerItem(0, 2); ClickPetitionButton()")
                .unwrap();
            assert_eq!(
                held_slot(&s),
                Some(1),
                "the previous charter is handed back"
            );
            assert!(locked(&s, 1) && locked(&s, 2));
            s.run("TurnInPetition()").unwrap();
            assert_eq!(take(), vec![PetitionRequest::VendorTurnIn { item: 0x72 }]);

            // Slot 1 goes back to its bag, then a plain item is refused and stays held.
            s.run("PickupContainerItem(0, 1)").unwrap();
            assert!(!locked(&s, 1));
            s.run("PickupContainerItem(0, 3); ClickPetitionButton()")
                .unwrap();
            assert_eq!(held_slot(&s), Some(3), "a plain item stays on the cursor");
            s.run("ClearCursor()").unwrap();

            // An empty cursor empties the button and takes its charter.
            s.run("ClickPetitionButton()").unwrap();
            assert_eq!(held_slot(&s), Some(2));
            s.run("TurnInPetition()").unwrap();
            assert!(take().is_empty());
            s.run("ClearCursor()").unwrap();
            assert!(!locked(&s, 2));

            // With the vendor closed the button ignores the click.
            s.set_petition_vendor(PetitionVendorState::default());
            s.run("PickupContainerItem(0, 1); ClickPetitionButton()")
                .unwrap();
            assert_eq!(held_slot(&s), Some(1));
        });
    }

    /// The list's show forgets the button's item with no lock event (`0x5a22cb`), the app keeping its lock; an empty button forgets nothing.
    #[test]
    fn a_petition_list_show_empties_the_button_and_keeps_the_lock() {
        let mut s = vendor_with_charters();
        let mut loan_v = acts();
        crate::script::live::lending(&mut loan_v, || {
            assert_eq!(s.clear_petition_button(), None);
            s.run("PickupContainerItem(0, 1); ClickPetitionButton()")
                .unwrap();
            assert!(locked(&s, 1));
            s.model_mut().pending_events.clear();
            assert_eq!(
                s.clear_petition_button(),
                Some(PetitionButtonItem {
                    item: 0x71,
                    bag: 0,
                    slot: 1
                })
            );
            assert!(
                !s.model_ref()
                    .pending_events
                    .iter()
                    .any(|(e, _)| e == "ITEM_LOCK_CHANGED"),
                "no unlock"
            );
            // The app keeps the slot's send lock (`keep_locked`), which the bag verbs read at the call.
            if let Some(slot) = s
                .model_mut()
                .containers
                .get_mut(&0)
                .and_then(|c| c.slots.get_mut(&1))
            {
                slot.locked = true;
            }
            assert!(locked(&s, 1), "the forgotten charter stays locked");
            s.run("PickupContainerItem(0, 1)").unwrap();
            assert_eq!(held_slot(&s), None, "a locked charter does not pick up");
            s.run("TurnInPetition()").unwrap();
            assert!(take().is_empty());
        });
    }
}
