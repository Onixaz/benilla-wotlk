//! The knowledge base's system message of the day, the one part of the help frame's knowledge base
//! the server feeds.

use mlua::Lua;

impl super::UiScript {
    /// The message of the day, its lines joined by `\n`, as `SMSG_MOTD` last set it (`0x551660`).
    pub fn set_kb_motd(&mut self, motd: &str) {
        if self.model_ref().kb_motd != motd {
            self.model_mut().kb_motd = motd.to_string();
        }
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // KBSystem_GetMOTD() (`0x551fb0`): the stored message, "" before any.
    lua.globals().set(
        "KBSystem_GetMOTD",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            lua.create_string(&model.kb_motd)
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    #[test]
    fn the_motd_reads_what_was_set() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<String>("return KBSystem_GetMOTD()").unwrap(), "");
        s.set_kb_motd("one\ntwo");
        assert_eq!(
            s.eval::<String>("return KBSystem_GetMOTD()").unwrap(),
            "one\ntwo"
        );
    }
}
