//! The `ItemTextFrame` Lua surface (letters, books, plaques) over the reader's globals the app owns, read at the call. The app fires `ITEM_TEXT_BEGIN`, `ITEM_TEXT_READY` and `ITEM_TEXT_CLOSED` from its handlers; a page turn fires `ITEM_TEXT_READY` and `CloseItemText` fires `ITEM_TEXT_CLOSED` inside their calls.

use mlua::{Lua, Value};

/// The reader at the call.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemTextRead {
    /// `ItemTextGetItem()` (`0x589c40`): the read object's name, `None` with the object gone.
    pub item: Option<String>,
    /// `ItemTextGetText()` (`0x589b90`): the text the last draw left (`0xbfaa00`).
    pub text: String,
    /// `ItemTextGetPage()` (`0x589b50`): the page index plus one (`0xbfc940`).
    pub page: u32,
    /// `ItemTextHasNextPage()` (`0x589d70`): no letter, and the next page id is set.
    pub has_next: bool,
    /// `ItemTextGetMaterial()` (`0x589c90`): the object's `PageTextMaterial.dbc` name, `None` for nil.
    pub material: Option<String>,
}

/// `f` over the reader at the call; `None` with no view lent.
fn read<R>(f: impl FnOnce(&ItemTextRead) -> R) -> Option<R> {
    super::read_live(|l| l.item_text().map(|r| f(&r))).flatten()
}

/// A string or nil.
fn string_or_nil(lua: &Lua, s: Option<String>) -> mlua::Result<Value> {
    Ok(match s {
        Some(s) => Value::String(lua.create_string(&s)?),
        None => Value::Nil,
    })
}

/// Register the item-text globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "ItemTextGetItem",
        lua.create_function(|lua, ()| string_or_nil(lua, read(|r| r.item.clone()).flatten()))?,
    )?;

    // `0x58a480`: the creator's cached name, a miss asked with the reader's callback on it.
    g.set(
        "ItemTextGetCreator",
        lua.create_function(|lua, ()| {
            string_or_nil(lua, super::write_live(|l| l.item_text_creator()).flatten())
        })?,
    )?;

    g.set(
        "ItemTextGetText",
        lua.create_function(|_, ()| Ok(read(|r| r.text.clone()).unwrap_or_default()))?,
    )?;

    g.set(
        "ItemTextGetPage",
        lua.create_function(|_, ()| Ok(read(|r| i64::from(r.page)).unwrap_or(1)))?,
    )?;

    g.set(
        "ItemTextHasNextPage",
        lua.create_function(|_, ()| {
            Ok(match read(|r| r.has_next).unwrap_or(false) {
                true => Value::Number(1.0),
                false => Value::Nil,
            })
        })?,
    )?;

    g.set(
        "ItemTextGetMaterial",
        lua.create_function(|lua, ()| string_or_nil(lua, read(|r| r.material.clone()).flatten()))?,
    )?;

    // `0x58a400` -> `0x58a1a0` with no object: `ITEM_TEXT_CLOSED` inside the call, twice while the reader holds the interact target, whose close re-enters it (`0x58a207` -> `0x512e60`).
    g.set(
        "CloseItemText",
        lua.create_function(|lua, ()| {
            let closes = super::write_live(|l| l.close_item_text()).unwrap_or(0);
            for _ in 0..closes {
                super::tick::fire_event_into(lua, "ITEM_TEXT_CLOSED", Vec::new());
            }
            Ok(())
        })?,
    )?;

    // `0x58a110`, `0x58a150`: the page steps and redraws inside the call (`0x589e90`), `ITEM_TEXT_READY` with it when the page is cached.
    for (name, next) in [("ItemTextPrevPage", false), ("ItemTextNextPage", true)] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                if super::write_live(|l| l.item_text_turn(next)) == Some(true) {
                    super::tick::fire_event_into(lua, "ITEM_TEXT_READY", Vec::new());
                }
                Ok(())
            })?,
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::live::{TestItemText, TestView};
    use crate::script::UiScript;

    fn view() -> TestView {
        TestView {
            item_text: Some(TestItemText {
                read: super::ItemTextRead {
                    item: Some("Plain Letter".into()),
                    text: "asd".into(),
                    page: 1,
                    has_next: false,
                    material: None,
                },
                creator: Some("One".into()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn getters_read_the_reader_at_the_call() {
        let mut s = UiScript::new().unwrap();
        // No view: the defaults the frame never reads while hidden.
        assert!(s.eval::<bool>("return ItemTextGetItem() == nil").unwrap());
        assert_eq!(s.eval::<i64>("return ItemTextGetPage()").unwrap(), 1);

        let mut v = view();
        s.with_live(&mut v, |s| {
            assert_eq!(
                s.eval::<String>("return ItemTextGetItem()").unwrap(),
                "Plain Letter"
            );
            assert_eq!(
                s.eval::<String>("return ItemTextGetCreator()").unwrap(),
                "One"
            );
            assert_eq!(s.eval::<String>("return ItemTextGetText()").unwrap(), "asd");
            assert_eq!(s.eval::<i64>("return ItemTextGetPage()").unwrap(), 1);
            assert!(s
                .eval::<bool>("return ItemTextHasNextPage() == nil")
                .unwrap());
            assert!(s
                .eval::<bool>("return ItemTextGetMaterial() == nil")
                .unwrap());
        });
    }

    /// The close fires its events inside the call, the page turn its redraw's.
    #[test]
    fn close_and_turns_fire_inside_their_calls() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "log = {} local f = CreateFrame('Frame') \
             f:RegisterEvent('ITEM_TEXT_CLOSED') f:RegisterEvent('ITEM_TEXT_READY') \
             f:SetScript('OnEvent', function(_, e) table.insert(log, e) end)",
        )
        .unwrap();
        let mut v = view();
        if let Some(t) = v.item_text.as_mut() {
            t.closes = 2;
            t.turn_ready = true;
        }
        let log = s.with_live(&mut v, |s| {
            s.run("ItemTextNextPage() table.insert(log, 'after next') CloseItemText() table.insert(log, 'after close') CloseItemText()").unwrap();
            s.eval::<Vec<String>>("return log").unwrap()
        });
        assert_eq!(
            log,
            [
                "ITEM_TEXT_READY",
                "after next",
                "ITEM_TEXT_CLOSED",
                "ITEM_TEXT_CLOSED",
                "after close"
            ]
        );
        assert_eq!(v.item_text.as_ref().unwrap().turns, [true]);
    }
}
