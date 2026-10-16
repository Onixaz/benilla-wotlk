//! The GM ticket globals the stock Help window calls (`0x5ad070`-`0x5ad230`). The verbs share one
//! ordered queue, because call order is the reference's wire order, and two calls are two sends:
//! the ticket toast re-polls `GetGMTicket()` every 10 minutes. `GetGMTicket()` is a send, answered
//! by the `UPDATE_TICKET` event; the client's ticket and response ids live in the app, which gates
//! each write on them as the senders do.

use mlua::{Lua, MultiValue, Value};

/// One ticket verb the window called, drained in call order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GmTicketIntent {
    /// `GetGMTicket()` (`0x5ad070`): `CMSG_GMTICKET_GETTICKET`.
    Ask,
    /// `GetGMStatus()` (`0x5ad1c0`): `CMSG_GMTICKET_SYSTEMSTATUS`.
    AskStatus,
    /// `DeleteGMTicket()` (`0x5ad140`): `CMSG_GMTICKET_DELETETICKET`.
    Delete,
    /// `NewGMTicket(text, needResponse)` (`0x5ad080`), or `GMResponseNeedMoreHelp(text)`
    /// (`0x5ad150`), which files the follow-up with a reply wanted: `CMSG_GMTICKET_CREATE`.
    Create {
        text: String,
        need_response: bool,
        more_help: bool,
    },
    /// `UpdateGMTicket(text)` (`0x5ad0f0`): `CMSG_GMTICKET_UPDATETEXT`.
    Update { text: String },
    /// `GMResponseResolve()` (`0x5ad1b0`): `CMSG_GMRESPONSE_RESOLVE`.
    Resolve,
}

impl super::UiScript {
    /// The `GMTicketCategory.dbc` rows in file order, pushed once.
    pub fn set_gm_ticket_categories(&mut self, categories: Vec<(u32, String)>) {
        set_field!(self, gm_ticket_categories, categories);
    }

    /// Drain the ticket verbs in call order, one packet each.
    pub fn take_gm_ticket_intents(&mut self) -> Vec<GmTicketIntent> {
        drain_field!(self, gm_ticket_intents)
    }

    /// Drain the `Stuck()` calls, each one cast of spell 7355.
    pub fn take_stuck_casts(&mut self) -> u32 {
        drain_field!(self, stuck_casts)
    }
}

/// `lua_isstring` then `lua_tolstring`: a string, or a number in its string form.
fn ticket_text(lua: &Lua, v: Option<&Value>) -> mlua::Result<Option<String>> {
    match v {
        Some(v @ (Value::String(_) | Value::Integer(_) | Value::Number(_))) => {
            Ok(lua.coerce_string(v.clone())?.map(|s| s.to_string_lossy()))
        }
        _ => Ok(None),
    }
}

fn push(lua: &Lua, intent: GmTicketIntent) {
    crate::script::model_mut_of(lua)
        .gm_ticket_intents
        .push(intent);
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // A flat `id, name, ...` vararg list; empty until the app pushes the DBC.
    g.set(
        "GetGMTicketCategories",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let mut out = Vec::with_capacity(model.gm_ticket_categories.len() * 2);
            for (id, name) in &model.gm_ticket_categories {
                out.push(Value::Integer(i64::from(*id)));
                out.push(Value::String(lua.create_string(name)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // The argument-free sends.
    for (name, intent) in [
        ("GetGMTicket", GmTicketIntent::Ask),
        ("GetGMStatus", GmTicketIntent::AskStatus),
        ("DeleteGMTicket", GmTicketIntent::Delete),
        ("GMResponseResolve", GmTicketIntent::Resolve),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                push(lua, intent.clone());
                Ok(())
            })?,
        )?;
    }

    // NewGMTicket(ticketDescription, needResponse): a string and a real boolean (`0x5ad0a1`),
    // else the usage error.
    g.set(
        "NewGMTicket",
        lua.create_function(|lua, args: MultiValue| {
            let text = ticket_text(lua, args.front())?;
            let need = args.get(1);
            let (Some(text), Some(Value::Boolean(need_response))) = (text, need) else {
                return Err(mlua::Error::runtime(
                    "Usage: NewGMTicket(ticketDescription, needResponse)",
                ));
            };
            push(
                lua,
                GmTicketIntent::Create {
                    text,
                    need_response: *need_response,
                    more_help: false,
                },
            );
            Ok(())
        })?,
    )?;

    // UpdateGMTicket(ticketDescription).
    g.set(
        "UpdateGMTicket",
        lua.create_function(|lua, args: MultiValue| {
            let Some(text) = ticket_text(lua, args.front())? else {
                return Err(mlua::Error::runtime(
                    "Usage: UpdateGMTicket(ticketDescription)",
                ));
            };
            push(lua, GmTicketIntent::Update { text });
            Ok(())
        })?,
    )?;

    // GMResponseNeedMoreHelp(newTicketDescription): a create with a reply wanted and the
    // need-more-help byte set (`0x5ad191`).
    g.set(
        "GMResponseNeedMoreHelp",
        lua.create_function(|lua, args: MultiValue| {
            let Some(text) = ticket_text(lua, args.front())? else {
                return Err(mlua::Error::runtime(
                    "Usage: GMResponseNeedMoreHelp(newTicketDescription)",
                ));
            };
            push(
                lua,
                GmTicketIntent::Create {
                    text,
                    need_response: true,
                    more_help: true,
                },
            );
            Ok(())
        })?,
    )?;

    // The Help window's Auto-Unstuck, not a ticket verb: it casts spell 7355, whose
    // `SPELL_EFFECT_STUCK` moves the player to a safe position server-side.
    g.set(
        "Stuck",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua).stuck_casts += 1;
            Ok(())
        })?,
    )?;

    Ok(())
}
