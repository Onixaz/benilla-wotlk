//! Show and Hide fire OnShow/OnHide only on effective transitions; OnUpdate runs only when visible.

use super::common::script;

#[test]
fn show_hide_fires_only_on_effective_transitions() {
    let s = script();
    s.run(
        r#"
        shows, hides = 0, 0
        local f = CreateFrame("Frame", "Vis")
        f:SetScript("OnShow", function() shows = shows + 1 end)
        f:SetScript("OnHide", function() hides = hides + 1 end)
        f:Hide()   -- true→false: OnHide (1)
        f:Hide()   -- no transition: nothing
        f:Show()   -- false→true: OnShow (1)
        f:Show()   -- no transition: nothing
    "#,
    )
    .unwrap();
    let (shows, hides): (i64, i64) = s.eval("return shows, hides").unwrap();
    assert_eq!((shows, hides), (1, 1));
    assert!(s.errors().is_empty(), "no script errors: {:?}", s.errors());
}

#[test]
fn mid_tree_hide_fires_onhide_for_the_subtree() {
    let s = script();
    s.run(
        r#"
        childhides = 0
        local p = CreateFrame("Frame", "PH")
        local c = CreateFrame("Frame", "CH", p)
        c:SetScript("OnHide", function() childhides = childhides + 1 end)
        p:Hide()   -- child loses effective visibility though its own shown stays true
    "#,
    )
    .unwrap();
    let hides: i64 = s.eval("return childhides").unwrap();
    assert_eq!(hides, 1);
}

#[test]
fn tick_runs_onupdate_only_when_effectively_visible() {
    let mut s = script();
    s.run(
        r#"
        ticks, last = 0, 0
        local f = CreateFrame("Frame", "UF")
        f:SetScript("OnUpdate", function(self, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) local self, elapsed = this, arg1 ticks = ticks + 1; last = elapsed end)
    "#,
    )
    .unwrap();

    s.tick(0.25);
    assert_eq!(s.eval::<i64>("return ticks").unwrap(), 1);
    assert!((s.eval::<f64>("return last").unwrap() - 0.25).abs() < 1e-6);

    s.run("UF:Hide()").unwrap();
    s.tick(0.25); // hidden → no fire
    assert_eq!(s.eval::<i64>("return ticks").unwrap(), 1);

    s.run("UF:Show()").unwrap();
    s.tick(0.25); // visible again → fires
    assert_eq!(s.eval::<i64>("return ticks").unwrap(), 2);
}

/// Registers a FrameXML document, asserting it loads clean.
fn load_xml(s: &crate::script::UiScript, xml: &str) {
    let doc = crate::framexml::parse(xml).expect("valid FrameXML");
    let report = crate::loader::load(s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
}

/// `ShowThis` (`0x4918f0`) and `HideThis` (`0x491830`) step each child before notifying their own
/// frame, so a subtree hears of a show or hide bottom-up, siblings in link order.
#[test]
fn a_subtree_hears_its_show_and_hide_children_first() {
    let s = script();
    s.run(
        r#"
        log = {}
        local function watch(f, name)
            f:SetScript("OnShow", function() table.insert(log, "show " .. name) end)
            f:SetScript("OnHide", function() table.insert(log, "hide " .. name) end)
        end
        P = CreateFrame("Frame", "OrderP")
        A = CreateFrame("Frame", "OrderA", P)
        A1 = CreateFrame("Frame", "OrderA1", A)
        B = CreateFrame("Frame", "OrderB", P)
        watch(P, "P"); watch(A, "A"); watch(A1, "A1"); watch(B, "B")
        P:Hide()
        P:Show()
        "#,
    )
    .unwrap();
    let log: Vec<String> = s.eval("return log").unwrap();
    assert_eq!(
        log,
        vec!["hide A1", "hide A", "hide B", "hide P", "show A1", "show A", "show B", "show P"]
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The cascade re-reads each step's gate: a sibling an earlier `OnShow` hid is never shown and
/// hears nothing, where a snapshot taken before the handlers would still announce it.
#[test]
fn a_sibling_hidden_by_an_earlier_onshow_is_not_shown() {
    let s = script();
    s.run(
        r#"
        log = {}
        P = CreateFrame("Frame", "GateP")
        A = CreateFrame("Frame", "GateA", P)
        B = CreateFrame("Frame", "GateB", P)
        P:Hide()
        A:SetScript("OnShow", function() table.insert(log, "show A"); B:Hide() end)
        B:SetScript("OnShow", function() table.insert(log, "show B") end)
        B:SetScript("OnHide", function() table.insert(log, "hide B") end)
        P:SetScript("OnShow", function() table.insert(log, "show P") end)
        P:Show()
        "#,
    )
    .unwrap();
    let log: Vec<String> = s.eval("return log").unwrap();
    assert_eq!(log, vec!["show A", "show P"]);
    assert!(!s.eval::<bool>("return GateB:IsVisible()").unwrap());
    assert!(s.eval::<bool>("return GateA:IsVisible()").unwrap());
}

/// `PostLoadXML` (`0x490410`) fires `OnLoad`, then `OnShow` for a frame still visible, children
/// before their parent; a frame hidden by its XML, or by its own `OnLoad`, hears no `OnShow`.
#[test]
fn a_frame_visible_at_the_end_of_its_load_hears_onshow_after_onload() {
    let s = script();
    s.run("log = {}").unwrap();
    load_xml(
        &s,
        r#"<Ui>
             <Frame name="LoadP">
               <Frames>
                 <Frame name="LoadKid">
                   <Scripts>
                     <OnLoad>table.insert(log, "load Kid")</OnLoad>
                     <OnShow>table.insert(log, "show Kid")</OnShow>
                   </Scripts>
                 </Frame>
                 <Frame name="LoadHidden" hidden="true">
                   <Scripts>
                     <OnShow>table.insert(log, "show Hidden")</OnShow>
                   </Scripts>
                 </Frame>
                 <Frame name="LoadSelfHide">
                   <Scripts>
                     <OnLoad>self:Hide()</OnLoad>
                     <OnShow>table.insert(log, "show SelfHide")</OnShow>
                     <OnHide>table.insert(log, "hide SelfHide")</OnHide>
                   </Scripts>
                 </Frame>
               </Frames>
               <Scripts>
                 <OnLoad>table.insert(log, "load P")</OnLoad>
                 <OnShow>table.insert(log, "show P")</OnShow>
               </Scripts>
             </Frame>
           </Ui>"#,
    );
    let log: Vec<String> = s.eval("return log").unwrap();
    assert_eq!(
        log,
        vec!["load Kid", "show Kid", "hide SelfHide", "load P", "show P"]
    );
}

/// `OnLayerHide` (`0x490300`) is silent while the frame is in its XML load: a child's `OnLoad`
/// hiding its still-loading parent fires no `OnHide` on the parent, which then ends its load
/// hidden and hears no `OnShow` either.
#[test]
fn a_hide_during_the_frames_own_load_is_silent() {
    let s = script();
    s.run("log = {}").unwrap();
    load_xml(
        &s,
        r#"<Ui>
             <Frame name="SilentP">
               <Scripts>
                 <OnShow>table.insert(log, "show P")</OnShow>
                 <OnHide>table.insert(log, "hide P")</OnHide>
               </Scripts>
               <Frames>
                 <Frame name="SilentKid">
                   <Scripts>
                     <OnLoad>self:GetParent():Hide()</OnLoad>
                     <OnHide>table.insert(log, "hide Kid")</OnHide>
                   </Scripts>
                 </Frame>
               </Frames>
             </Frame>
           </Ui>"#,
    );
    let log: Vec<String> = s.eval("return log").unwrap();
    assert_eq!(log, vec!["hide Kid"], "the loaded kid hears its hide");
    assert!(!s.eval::<bool>("return SilentP:IsShown()").unwrap());
}

/// `CreateFrame` with a template loads it the same way, so the template's `OnShow` runs after its
/// `OnLoad` on a frame born visible.
#[test]
fn a_templated_create_frame_hears_onshow_after_onload() {
    let s = script();
    s.run("log = {}").unwrap();
    load_xml(
        &s,
        r#"<Ui>
             <Frame name="ShowTemplate" virtual="true">
               <Scripts>
                 <OnLoad>table.insert(log, "load " .. self:GetName())</OnLoad>
                 <OnShow>table.insert(log, "show " .. self:GetName())</OnShow>
               </Scripts>
             </Frame>
           </Ui>"#,
    );
    s.run(
        r#"
        CreateFrame("Frame", "Shown", nil, "ShowTemplate")
        local holder = CreateFrame("Frame", "Holder")
        holder:Hide()
        CreateFrame("Frame", "UnderHidden", holder, "ShowTemplate")
        "#,
    )
    .unwrap();
    let log: Vec<String> = s.eval("return log").unwrap();
    assert_eq!(log, vec!["load Shown", "show Shown", "load UnderHidden"]);
}
