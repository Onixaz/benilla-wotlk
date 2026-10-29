//! The `MovieFrame` methods (method table `0xb2cd30`): `StartMovie` (`0x970660`), `StopMovie`
//! (`0x970730`) and `EnableSubtitles` (`0x970770`).
//!
//! Deviation: benilla plays no pre-rendered movie (#130), so `StartMovie` answers nil, the
//! reference's "could not start" when the file does not parse or open (`0x95eb30`, `0x9706e8`).
//! The stock glue's `MovieFrame_PlayMovie` then walks its list to the end and hides the frame,
//! whose `OnHide` returns to the login screen; the in-game one calls `GameMovieFinished()`. With
//! no movie running `StopMovie` has nothing to stop: `0x95eba0` returns while the playing flag
//! `+0x2a0` is clear, and only past that test fires `OnMovieFinished`. The three `OnMovie*`
//! scripts stay unbuilt, as nothing would fire them.

use mlua::{Lua, MultiValue, Table, Value};

use super::binding_abi::bool_or_default;
use super::object::frame_handle_of;
use crate::widget::FrameKind;

pub(super) const REG_MOVIEFRAME_METHODS: &str = "__benilla_movieframe_methods";

/// Raises unless `this` is a live MovieFrame, since a caller can lift the method table onto
/// another frame.
fn check_movie_frame(lua: &Lua, this: &Table) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let is_movie = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data")
        .arena
        .frame(h)
        .is_some_and(|f| f.kind == FrameKind::MovieFrame);
    if is_movie {
        Ok(())
    } else {
        Err(mlua::Error::runtime(
            "Wrong object type for member function",
        ))
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // `lua_isstring` on the file and `lua_isnumber` on the volume, else the usage error
    // (`0xaa1218`); one value back, 1 for a started movie (`0x9706ce`) and nil for one that is not.
    m.set(
        "StartMovie",
        lua.create_function(|lua, (this, file, volume): (Table, Value, Value)| {
            check_movie_frame(lua, &this)?;
            let is_string = matches!(
                file,
                Value::String(_) | Value::Integer(_) | Value::Number(_)
            );
            let is_number = lua.coerce_number(volume).ok().flatten().is_some();
            if !(is_string && is_number) {
                let who: Option<String> =
                    this.get::<mlua::Function>("GetName")?.call(this.clone())?;
                return Err(mlua::Error::runtime(format!(
                    "Usage: {}:StartMovie(\"filename\", volume_0_to_255)",
                    who.as_deref()
                        .unwrap_or(super::object::anchor_args::UNNAMED)
                )));
            }
            Ok(Value::Nil)
        })?,
    )?;
    // No arguments, no returns (`0x970762`).
    m.set(
        "StopMovie",
        lua.create_function(|lua, this: Table| check_movie_frame(lua, &this))?,
    )?;
    // `Lua_GetBoolOrDefault(2, true)` into `+0x2a8`, read only by the subtitle events no movie
    // here raises; no returns.
    m.set(
        "EnableSubtitles",
        lua.create_function(|lua, (this, args): (Table, MultiValue)| {
            check_movie_frame(lua, &this)?;
            let _enabled = bool_or_default(args.front(), true);
            Ok(())
        })?,
    )?;

    lua.set_named_registry_value(REG_MOVIEFRAME_METHODS, m)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// `StartMovie` answers nil for a file and a volume and raises its usage on anything less;
    /// `StopMovie` and `EnableSubtitles` answer nothing; a plain frame has none of the three.
    #[test]
    fn the_movie_frame_takes_its_methods_and_plays_nothing() {
        let s = UiScript::new_glue().unwrap();
        s.run(r#"M = CreateFrame("MovieFrame", "MovieProbe")"#)
            .unwrap();
        assert!(s
            .eval::<bool>(r##"return select("#", M:StartMovie("a\\b", 255)) == 1 and M:StartMovie("a\\b", 255) == nil"##)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return M:StartMovie(7, "150") == nil"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r##"return select("#", M:StopMovie()) == 0"##)
            .unwrap());
        assert!(s
            .eval::<bool>(r##"return select("#", M:EnableSubtitles(1)) + select("#", M:EnableSubtitles()) == 0"##)
            .unwrap());
        for bad in [
            "M:StartMovie()",
            r#"M:StartMovie("a")"#,
            r#"M:StartMovie("a", "loud")"#,
            "M:StartMovie({}, 1)",
            "M:StartMovie(true, 1)",
        ] {
            let err = s.run(bad).unwrap_err().to_string();
            assert!(
                err.contains(r#"Usage: MovieProbe:StartMovie("filename", volume_0_to_255)"#),
                "{bad}: {err}"
            );
        }
        s.run(r#"P = CreateFrame("Frame", "NotAMovie")"#).unwrap();
        assert!(s.eval::<bool>("return P.StartMovie == nil").unwrap());
        let err = s.run("M.StopMovie(P)").unwrap_err().to_string();
        assert!(
            err.contains("Wrong object type for member function"),
            "{err}"
        );
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }
}
