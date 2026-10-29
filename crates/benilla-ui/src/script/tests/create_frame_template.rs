//! The runtime template path, `CreateFrame`'s fourth argument ([`crate::loader::apply_template`]),
//! and `<ScrollChild>`. The caller's name wins: a template made as `Mine` publishes `MineTexture`.

use super::common::script;
use crate::script::UiScript;

/// Registers templates by loading a FrameXML document, the path an addon's own `.xml` takes.
fn register(s: &UiScript, xml: &str) {
    let doc = crate::framexml::parse(xml).expect("valid FrameXML");
    let report = crate::loader::load(s, &doc, &|_| None);
    assert!(
        report.errors.is_empty(),
        "fixture document failed to load: {:?}",
        report.errors
    );
}

#[test]
fn a_runtime_template_brings_size_anchors_regions_and_a_fired_onload() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    register(
        &s,
        r#"<Ui>
             <Frame name="Host"/>
             <Frame name="ProbeTemplate" virtual="true">
               <Size><AbsDimension x="160" y="40"/></Size>
               <Anchors>
                 <Anchor point="TOPLEFT" relativeTo="$parent" relativePoint="BOTTOMLEFT">
                   <Offset><AbsDimension x="7" y="-3"/></Offset>
                 </Anchor>
               </Anchors>
               <Layers>
                 <Layer level="ARTWORK">
                   <Texture name="$parentTexture" file="Interface\Probe\Art"/>
                 </Layer>
               </Layers>
               <Scripts>
                 <OnLoad>ProbeLoadedAs = this:GetName(); ProbeLoadWidth = this:GetWidth()</OnLoad>
               </Scripts>
             </Frame>
           </Ui>"#,
    );

    s.run(r#"Mine = CreateFrame("Frame", "Mine", Host, "ProbeTemplate")"#)
        .unwrap();

    assert_eq!(
        s.eval::<(f32, f32)>("return Mine:GetWidth(), Mine:GetHeight()")
            .unwrap(),
        (160.0, 40.0),
        "the template's <Size>"
    );

    // A template's `$parent` in `<Anchors>` is the frame's first named ancestor, not the frame.
    let (point, rel, rel_point, x, y): (String, String, String, f32, f32) = s
        .eval(
            "local p, r, rp, ox, oy = Mine:GetPoint(1) \
             return p, r:GetName(), rp, ox, oy",
        )
        .unwrap();
    assert_eq!(
        (point.as_str(), rel.as_str(), rel_point.as_str(), x, y),
        ("TOPLEFT", "Host", "BOTTOMLEFT", 7.0, -3.0)
    );

    assert!(
        s.eval::<bool>(r#"return getglobal("MineTexture") ~= nil"#)
            .unwrap(),
        "the template's $parentTexture resolved against the caller's name"
    );
    assert!(
        s.eval::<bool>(r#"return getglobal("ProbeTemplateTexture") == nil"#)
            .unwrap(),
        "nothing may be named after the template"
    );

    // The reference fires OnLoad inside `CreateFrame`, after the template is applied.
    assert_eq!(s.eval::<String>("return ProbeLoadedAs").unwrap(), "Mine");
    assert_eq!(s.eval::<f32>("return ProbeLoadWidth").unwrap(), 160.0);

    assert!(s.take_errors().is_empty());
    assert!(s.take_warnings().is_empty());
}

/// A nested child is parented to the new frame, and `$parent` composes as it does through XML.
#[test]
fn a_nested_frames_child_is_named_against_the_caller() {
    let mut s = script();
    register(
        &s,
        r#"<Ui>
             <Frame name="NestTemplate" virtual="true">
               <Frames>
                 <Frame name="$parentInner">
                   <Size><AbsDimension x="11" y="12"/></Size>
                   <Frames>
                     <Frame name="$parentDeep"/>
                   </Frames>
                 </Frame>
               </Frames>
             </Frame>
           </Ui>"#,
    );
    s.run(r#"Outer = CreateFrame("Frame", "Outer", nil, "NestTemplate")"#)
        .unwrap();

    assert_eq!(
        s.eval::<String>("return OuterInner:GetParent():GetName()")
            .unwrap(),
        "Outer"
    );
    assert_eq!(
        s.eval::<(f32, f32)>("return OuterInner:GetWidth(), OuterInner:GetHeight()")
            .unwrap(),
        (11.0, 12.0)
    );
    // Two levels deep, so the chain is composing rather than substituting once.
    assert_eq!(
        s.eval::<String>("return OuterInnerDeep:GetParent():GetName()")
            .unwrap(),
        "OuterInner"
    );
    assert!(s
        .eval::<bool>(r#"return getglobal("NestTemplateInner") == nil"#)
        .unwrap());
    assert!(s.take_errors().is_empty());
    assert!(s.take_warnings().is_empty());
}

/// The template argument is a comma list, as an XML `inherits=` is: each name trimmed and applied
/// left to right, a single name chaining through its own `inherits`.
#[test]
fn a_comma_list_applies_each_name_and_a_single_name_still_chains() {
    let mut s = script();
    register(
        &s,
        r#"<Ui>
             <Frame name="TemplRoot" virtual="true" alpha="0.25">
               <Size><AbsDimension x="50" y="60"/></Size>
             </Frame>
             <Frame name="TemplA" virtual="true" inherits="TemplRoot"/>
             <Frame name="TemplB" virtual="true">
               <Layers>
                 <Layer level="ARTWORK">
                   <Texture name="$parentBTex" file="Interface\Probe\B"/>
                 </Layer>
               </Layers>
             </Frame>
           </Ui>"#,
    );

    s.run(r#"One = CreateFrame("Frame", "One", nil, "TemplA")"#)
        .unwrap();
    assert_eq!(
        s.eval::<(f32, f32)>("return One:GetWidth(), One:GetHeight()")
            .unwrap(),
        (50.0, 60.0)
    );
    assert_eq!(s.eval::<f32>("return One:GetAlpha()").unwrap(), 0.25);
    assert!(s.take_errors().is_empty());
    assert!(s.take_warnings().is_empty());

    // A comma list applies each template in turn, each name trimmed.
    s.run(r#"Both = CreateFrame("Frame", "Both", nil, "TemplA, TemplB")"#)
        .unwrap();
    assert_eq!(s.eval::<f32>("return Both:GetAlpha()").unwrap(), 0.25);
    assert!(s
        .eval::<bool>(r#"return getglobal("BothBTex") ~= nil"#)
        .unwrap());
}

/// The lookup folds ASCII case: the name hash uppercases before hashing, so a mis-cased name lands
/// in the same bucket, and the compare is case-insensitive.
#[test]
fn a_template_name_is_matched_case_insensitively() {
    let mut s = script();
    register(
        &s,
        r#"<Ui>
             <Frame name="CT_RACheckButtonTemplate" virtual="true" alpha="0.25">
               <Size><AbsDimension x="50" y="60"/></Size>
             </Frame>
           </Ui>"#,
    );
    // The mis-casing `CT_RaidAssist/CT_RAOptions.xml` ships: `RA` written `Ra`.
    s.run(r#"Cased = CreateFrame("Frame", "Cased", nil, "CT_RaCheckButtonTemplate")"#)
        .unwrap();
    assert_eq!(
        s.eval::<(f32, f32)>("return Cased:GetWidth(), Cased:GetHeight()")
            .unwrap(),
        (50.0, 60.0)
    );
    assert!(s.take_errors().is_empty());
    assert!(s.take_warnings().is_empty());
}

/// A template name that misses the lookup raises, before the frame is built or its name published.
/// The registry holds only `virtual="true"` elements, so a non-virtual name misses like an
/// undeclared one.
#[test]
fn an_unresolvable_template_raises_and_creates_nothing() {
    let s = script();
    register(&s, r#"<Ui><Frame name="PlainFrame"/></Ui>"#);

    for (frame, template) in [("Orphan", "NoSuchTemplate"), ("Plainer", "PlainFrame")] {
        let err = s
            .run(&format!(
                r#"{frame} = CreateFrame("Button", "{frame}", nil, "{template}")"#
            ))
            .expect_err("an unresolvable template must raise");
        let text = err.to_string();
        assert!(
            text.contains("Couldn't find inherited node") && text.contains(template),
            "the raise must carry the reference's message and the name: {text}"
        );

        assert!(
            s.eval::<bool>(&format!(r#"return getglobal("{frame}") == nil"#))
                .unwrap(),
            "{frame} was published despite the miss"
        );
    }

    // An ordinary Lua error: `pcall` catches it, as the widget dispatcher's `lua_pcall` does.
    assert!(
        !s.eval::<bool>(
            r#"return (pcall(CreateFrame, "Button", "Caught", nil, "NoSuchTemplate"))"#
        )
        .unwrap(),
        "the raise must be pcall-catchable"
    );
}

/// A region template on a frame applies its frame-shaped content and warns; a template whose tag
/// disagrees with the kind keeps the kind, and its tag-only parts are skipped, not attempted.
#[test]
fn a_region_template_or_a_mismatched_kind_is_named_never_fatal() {
    let mut s = script();
    register(
        &s,
        r#"<Ui>
             <Texture name="RegionTemplate" virtual="true" file="Interface\Probe\Art">
               <Size><AbsDimension x="33" y="44"/></Size>
             </Texture>
             <Button name="ProbeButtonTemplate" virtual="true">
               <Size><AbsDimension x="90" y="22"/></Size>
               <NormalTexture file="Interface\Probe\Normal"/>
               <ButtonText name="$parentText"/>
             </Button>
           </Ui>"#,
    );

    // 1 · a region template on a frame.
    s.run(r#"FromRegion = CreateFrame("Frame", "FromRegion", nil, "RegionTemplate")"#)
        .unwrap();
    assert_eq!(
        s.eval::<(f32, f32)>("return FromRegion:GetWidth(), FromRegion:GetHeight()")
            .unwrap(),
        (33.0, 44.0),
        "the frame-shaped half of a region template still applies"
    );
    let warnings = s.take_warnings();
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("RegionTemplate") && w.contains("REGION template")),
        "the region/frame mismatch must be named: {warnings:?}"
    );
    assert!(s.take_errors().is_empty());

    // 2 · a Button template asked for as a Frame.
    s.run(r#"AsFrame = CreateFrame("Frame", "AsFrame", nil, "ProbeButtonTemplate")"#)
        .unwrap();
    assert_eq!(
        s.eval::<f32>("return AsFrame:GetWidth()").unwrap(),
        90.0,
        "the generic half of the template still applies"
    );
    assert!(
        s.eval::<bool>(r#"return getglobal("AsFrameText") == nil"#)
            .unwrap(),
        "a Frame has no ButtonText slot, so nothing was invented for it"
    );
    let warnings = s.take_warnings();
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("ProbeButtonTemplate") && w.contains("<Button>")),
        "the kind mismatch must be named: {warnings:?}"
    );
    assert!(
        s.take_errors().is_empty(),
        "the Button-only steps are SKIPPED, not attempted and failed"
    );

    // 3 · the control: the same template, asked for as the kind it was written as.
    s.run(r#"AsButton = CreateFrame("Button", "AsButton", nil, "ProbeButtonTemplate")"#)
        .unwrap();
    assert_eq!(s.eval::<f32>("return AsButton:GetWidth()").unwrap(), 90.0);
    assert!(
        s.eval::<bool>(r#"return getglobal("AsButtonText") ~= nil"#)
            .unwrap(),
        "the <ButtonText name=\"$parentText\"> label, named against the caller"
    );
    assert!(s.take_errors().is_empty());
    assert!(
        s.take_warnings().is_empty(),
        "a matching kind has nothing to report"
    );
}

/// `<ScrollChild>` builds its one child through the `<Frames>` path and stores it as the scroll
/// child, which gives the frame its range.
#[test]
fn a_scroll_child_element_gives_the_frame_a_real_scroll_range() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    register(
        &s,
        r#"<Ui>
            <ScrollFrame name="Roller">
              <Size><AbsDimension x="200" y="100"/></Size>
              <Anchors><Anchor point="TOPLEFT"/></Anchors>
              <ScrollChild>
                <Frame name="$parentChild">
                  <Size><AbsDimension x="200" y="400"/></Size>
                </Frame>
              </ScrollChild>
            </ScrollFrame>
          </Ui>"#,
    );
    s.resolve();

    // `$parent` names the child against the scroll frame.
    assert!(s.eval::<bool>("return RollerChild ~= nil").unwrap());
    assert!(s
        .eval::<bool>("return Roller:GetScrollChild() == RollerChild")
        .unwrap());
    // The range is the overhang: a 400-tall child in a 100-tall window scrolls 300.
    assert_eq!(
        s.eval::<f64>("return Roller:GetVerticalScrollRange()")
            .unwrap(),
        300.0,
        "no child means range 0, which is the silent failure this element fixes"
    );
    s.run("Roller:SetVerticalScroll(120)").unwrap();
    assert_eq!(
        s.eval::<f64>("return Roller:GetVerticalScroll()").unwrap(),
        120.0
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The scroll range is the child's whole subtree, not its own height: the reference seeds a box and
/// then walks the child's regions and child frames. The shape is
/// `ItemTextPageScrollChild`, 10×10 around a 270×304 page (`ItemTextFrame.xml:198`).
#[test]
fn the_scroll_range_is_the_childs_whole_subtree() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    register(
        &s,
        r#"<Ui>
            <ScrollFrame name="Reader">
              <Size><AbsDimension x="280" y="100"/></Size>
              <Anchors><Anchor point="TOPLEFT"/></Anchors>
              <ScrollChild>
                <Frame name="$parentChild">
                  <Size><AbsDimension x="10" y="10"/></Size>
                  <Frames>
                    <Frame name="Page">
                      <Size><AbsDimension x="270" y="304"/></Size>
                      <Anchors><Anchor point="TOPLEFT"/></Anchors>
                    </Frame>
                  </Frames>
                </Frame>
              </ScrollChild>
            </ScrollFrame>
          </Ui>"#,
    );
    s.resolve();

    assert_eq!(
        s.eval::<f64>("return Reader:GetVerticalScrollRange()")
            .unwrap(),
        204.0,
        "the 304-tall page inside the 10-tall child, less the 100-tall window"
    );
    assert_eq!(
        s.eval::<f64>("return ReaderChild:GetHeight()").unwrap(),
        10.0,
        "and the child itself really is the reference's 10 — the range does not come from it"
    );

    // A hidden branch adds nothing: the reference guards both list walks on visibility.
    s.run("Page:Hide()").unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<f64>("return Reader:GetVerticalScrollRange()")
            .unwrap(),
        0.0,
        "hidden subtree, no range"
    );
}

/// An empty `<ScrollChild>` warns, in the reference's words, and keeps the frame (`0x96b920`).
#[test]
fn an_empty_scroll_child_is_reported() {
    let s = script();
    let doc = crate::framexml::parse(
        r#"<Ui><ScrollFrame name="Hollow"><ScrollChild/></ScrollFrame></Ui>"#,
    )
    .expect("valid FrameXML");
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w == "Frame Hollow: Scroll frame created without scroll child"),
        "warnings: {:?}",
        report.warnings
    );
    assert!(s.eval::<bool>("return Hollow ~= nil").unwrap());
}

/// The ScrollFrame's XML load (`0x96b920`) builds the `<ScrollChild>`, its `OnLoad` included,
/// before the post-load pass builds `<Frames>` (`0x48f400`) and fires the frame's own `OnLoad`
/// (`0x490410`): a nested scrollbar's `OnLoad` already sees the scroll child.
#[test]
fn a_scroll_child_loads_before_nested_frames_and_the_onload() {
    let s = script();
    s.run("order = {}").unwrap();
    let doc = crate::framexml::parse(
        r#"<Ui>
            <ScrollFrame name="Direct">
              <Frames>
                <Slider name="$parentBar">
                  <Scripts><OnLoad>
                    table.insert(order, "bar:" .. tostring(self:GetParent():GetScrollChild() and self:GetParent():GetScrollChild():GetName()))
                  </OnLoad></Scripts>
                </Slider>
              </Frames>
              <ScrollChild>
                <Frame name="$parentChild">
                  <Scripts><OnLoad>table.insert(order, "child")</OnLoad></Scripts>
                </Frame>
              </ScrollChild>
              <Scripts><OnLoad>
                table.insert(order, "frame:" .. self:GetScrollChild():GetName())
              </OnLoad></Scripts>
            </ScrollFrame>
          </Ui>"#,
    )
    .expect("valid FrameXML");
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
    let order: Vec<String> = s.eval("return order").unwrap();
    assert_eq!(
        order,
        ["child", "bar:DirectChild", "frame:DirectChild"],
        "scroll child, then nested frames, then the frame's own OnLoad"
    );
}

/// A template's `<ScrollChild>` and `<Frames>` follow the same order on an instance: the
/// template's scroll child is built by the template's XML load pass, before any `<Frames>`.
#[test]
fn an_inherited_scroll_child_loads_before_the_templates_frames() {
    let s = script();
    s.run("order = {}").unwrap();
    let doc = crate::framexml::parse(
        r#"<Ui>
            <ScrollFrame name="ListTemplate" virtual="true">
              <Frames>
                <Slider name="$parentBar">
                  <Scripts><OnLoad>
                    table.insert(order, "bar:" .. self:GetParent():GetScrollChild():GetName())
                  </OnLoad></Scripts>
                </Slider>
              </Frames>
              <ScrollChild>
                <Frame name="$parentChild">
                  <Scripts><OnLoad>table.insert(order, "child")</OnLoad></Scripts>
                </Frame>
              </ScrollChild>
            </ScrollFrame>
            <ScrollFrame name="Inherited" inherits="ListTemplate"/>
          </Ui>"#,
    )
    .expect("valid FrameXML");
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
    let order: Vec<String> = s.eval("return order").unwrap();
    assert_eq!(order, ["child", "bar:InheritedChild"]);
}

/// With a scroll child in both the template and the instance, the template's is built first and
/// the instance's last, so the instance's is the scroll child; both load before the template's
/// `<Frames>`.
#[test]
fn an_instance_scroll_child_wins_over_the_templates() {
    let s = script();
    s.run("order = {}").unwrap();
    let doc = crate::framexml::parse(
        r#"<Ui>
            <ScrollFrame name="PaneTemplate" virtual="true">
              <Frames>
                <Slider name="$parentBar">
                  <Scripts><OnLoad>
                    table.insert(order, "bar:" .. self:GetParent():GetScrollChild():GetName())
                  </OnLoad></Scripts>
                </Slider>
              </Frames>
              <ScrollChild>
                <Frame name="$parentShared">
                  <Scripts><OnLoad>table.insert(order, "shared")</OnLoad></Scripts>
                </Frame>
              </ScrollChild>
            </ScrollFrame>
            <ScrollFrame name="Pane" inherits="PaneTemplate">
              <ScrollChild>
                <Frame name="$parentOwn">
                  <Scripts><OnLoad>table.insert(order, "own")</OnLoad></Scripts>
                </Frame>
              </ScrollChild>
            </ScrollFrame>
          </Ui>"#,
    )
    .expect("valid FrameXML");
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
    let order: Vec<String> = s.eval("return order").unwrap();
    assert_eq!(order, ["shared", "own", "bar:PaneOwn"]);
    assert_eq!(
        s.eval::<String>("return Pane:GetScrollChild():GetName()")
            .unwrap(),
        "PaneOwn"
    );
    assert!(s.eval::<bool>("return PaneShared ~= nil").unwrap());
}
