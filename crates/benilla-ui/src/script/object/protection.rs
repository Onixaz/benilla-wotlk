//! Region protection: explicit XML protection and the parent/anchor dependencies it protects.

use std::collections::HashSet;

use mlua::{Lua, Table};

use crate::script::binding_abi::flag;
use crate::script::region_map::{set_shared, Side};
use crate::script::ModelCell;

/// Cached over model mutations, so repeated handle checks do not walk the dependency graph.
#[derive(Default)]
pub(crate) struct Cache {
    epoch: Option<u64>,
    protected: HashSet<u32>,
}

impl Cache {
    fn contains(&mut self, model: &ModelCell, id: u32) -> bool {
        if self.epoch != Some(model.epoch()) {
            self.protected.clear();
            let mut pending: Vec<u32> = model
                .protected_frames
                .iter()
                .filter(|h| model.arena.frame(**h).is_some())
                .filter_map(|h| model.frame_to_id.get(h).copied())
                .collect();
            // 0x489bb0 follows protection dependencies, including parents and anchor targets.
            while let Some(id) = pending.pop() {
                if !self.protected.insert(id) {
                    continue;
                }
                if let Some(h) = model.id_to_frame.get(&id) {
                    if let Some(parent) = model.arena.frame(*h).and_then(|f| f.parent) {
                        pending.extend(model.frame_to_id.get(&parent).copied());
                    }
                    if let Some(input) = model.layout_inputs.get(h) {
                        pending.extend(input.anchors.iter().map(|a| a.relative_to));
                    }
                } else if let Some(rh) = model.id_to_region.get(&id) {
                    if let Some(region) = model.arena.region(*rh) {
                        pending.extend(model.frame_to_id.get(&region.owner).copied());
                    }
                    if let Some(data) = model.region_data.get(rh) {
                        pending.extend(data.anchors.iter().map(|a| a.relative_to));
                    }
                }
            }
            self.epoch = Some(model.epoch());
        }
        self.protected.contains(&id)
    }
}

pub(in crate::script) fn install(lua: &Lua, methods: &Table, side: Side) -> mlua::Result<()> {
    // IsProtected (0x49cad0): effective and explicit protection, each 1 or nil.
    set_shared(lua, methods, side, "IsProtected", |lua, this: Table| {
        let id = super::decode_id(&this)?;
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model");
        let explicit = model
            .id_to_frame
            .get(&id)
            .is_some_and(|h| model.protected_frames.contains(h));
        let effective = model.protection_cache.borrow_mut().contains(&model, id);
        Ok((flag(effective), flag(explicit)))
    })?;
    set_shared(
        lua,
        methods,
        side,
        "CanChangeProtectedState",
        |lua, this: Table| {
            let id = super::decode_id(&this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            let combat = model.unit("player").is_some_and(|u| u.in_combat);
            let protected = model.protection_cache.borrow_mut().contains(&model, id);
            Ok(flag(!combat || !protected))
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::{UiScript, UnitState};

    fn script() -> UiScript {
        let s = UiScript::new().unwrap();
        let doc = crate::framexml::parse(
            r#"<Ui><Frame name="ProtectedTemplate" virtual="true" protected="true"/></Ui>"#,
        )
        .unwrap();
        let report = crate::loader::load(&s, &doc, &|_| None);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        s
    }

    #[test]
    fn protection_distinguishes_explicit_frames_from_their_dependencies() {
        let s = script();
        s.run(
            r#"
            Parent = CreateFrame("Frame")
            Target = CreateFrame("Frame")
            Header = CreateFrame("Frame", nil, Parent, "ProtectedTemplate")
            Header:SetPoint("CENTER", Target)
            Child = CreateFrame("Frame", nil, Header)
            local a,b = Header:IsProtected(); assert(a == 1 and b == 1)
            local a,b = Parent:IsProtected(); assert(a == 1 and b == nil)
            local a,b = Target:IsProtected(); assert(a == 1 and b == nil)
            local a,b = Child:IsProtected(); assert(a == nil and b == nil)
            -- RestrictedFrames.lua queries a surrogate carrying the same engine identity.
            local surrogate = setmetatable({[0]=Header[0]}, getmetatable(Header))
            local a,b = surrogate:IsProtected(); assert(a == 1 and b == 1)
            Header:ClearAllPoints()
            assert(Target:IsProtected() == nil)
            Header:SetParent(nil)
            assert(Parent:IsProtected() == nil)
            assert(Header:IsProtected() == 1)
            "#,
        )
        .unwrap();
        let epoch = s.visual_epoch();
        s.run("for i=1,20 do Header:IsProtected(); Target:IsProtected() end")
            .unwrap();
        assert_eq!(s.visual_epoch(), epoch);
    }

    #[test]
    fn a_region_anchor_transmits_protection_and_clearing_it_releases_the_target() {
        let s = script();
        s.run(
            r#"
            local owner = CreateFrame("Frame")
            local target = owner:CreateTexture()
            local header = CreateFrame("Frame", nil, nil, "ProtectedTemplate")
            header:SetPoint("CENTER", target)
            local a,b = target:IsProtected(); assert(a == 1 and b == nil)
            assert(owner:IsProtected() == 1)
            assert(target.IsProtected == owner.IsProtected)
            header:ClearAllPoints()
            assert(target:IsProtected() == nil and owner:IsProtected() == nil)
            "#,
        )
        .unwrap();
    }

    #[test]
    fn combat_gates_protected_state_changes_but_not_ordinary_frames() {
        let mut s = script();
        s.run(
            r#"
            Header = CreateFrame("Frame", nil, nil, "ProtectedTemplate")
            Plain = CreateFrame("Frame")
            assert(Header:CanChangeProtectedState() == 1)
            "#,
        )
        .unwrap();
        s.set_unit(
            "player",
            Some(UnitState {
                exists: true,
                in_combat: true,
                ..Default::default()
            }),
        );
        s.run(
            r#"
            assert(Header:CanChangeProtectedState() == nil)
            assert(Plain:CanChangeProtectedState() == 1)
            "#,
        )
        .unwrap();
        s.set_unit("player", None);
        assert_eq!(
            s.eval::<i64>("return Header:CanChangeProtectedState()")
                .unwrap(),
            1
        );
    }
}
