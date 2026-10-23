use crate::script::UiScript;

fn script() -> UiScript {
    let s = UiScript::new().unwrap();
    s.run(r#"F = CreateFrame("Frame", "AnimF", UIParent) F:SetSize(10, 10) F:SetPoint("CENTER")"#)
        .unwrap();
    s
}

fn eval(s: &UiScript, code: &str) -> f64 {
    s.eval::<f64>(code).unwrap()
}

#[test]
fn an_alpha_fades_the_frame_and_the_saved_alpha_returns_at_the_end() {
    let mut s = script();
    s.run(
        r#"log = {}
        G = F:CreateAnimationGroup("AnimFGroup")
        A = G:CreateAnimation("Alpha")
        A:SetChange(-1) A:SetDuration(1)
        A:SetScript("OnFinished", function() log[#log + 1] = "anim" end)
        G:SetScript("OnFinished", function(_, r) log[#log + 1] = "group:" .. tostring(r) end)
        G:Play()"#,
    )
    .unwrap();
    assert!(s.eval::<bool>("return G:IsPlaying()").unwrap());
    assert_eq!(
        s.eval::<String>("return G:GetName()").unwrap(),
        "AnimFGroup"
    );
    s.tick(0.5);
    let a = eval(&s, "return F:GetAlpha()");
    assert!((a - 0.5).abs() < 0.01, "half way: {a}");
    s.tick(0.6);
    assert_eq!(
        eval(&s, "return F:GetAlpha()"),
        1.0,
        "restored after the group"
    );
    assert_eq!(
        s.eval::<String>("return table.concat(log, ',')").unwrap(),
        "anim,group:false"
    );
    assert!(!s.eval::<bool>("return G:IsPlaying()").unwrap());
}

#[test]
fn orders_play_in_turn_with_their_delays() {
    let mut s = script();
    s.run(
        r#"G = F:CreateAnimationGroup()
        A1 = G:CreateAnimation("Alpha") A1:SetChange(-1) A1:SetDuration(0) A1:SetOrder(1)
        A2 = G:CreateAnimation("Alpha") A2:SetChange(1) A2:SetDuration(1) A2:SetOrder(2)
        A2:SetStartDelay(1)
        G:Play()"#,
    )
    .unwrap();
    assert_eq!(eval(&s, "return G:GetDuration()"), 2.0);
    assert_eq!(eval(&s, "return G:GetMaxOrder()"), 2.0);
    s.tick(0.5);
    assert_eq!(
        eval(&s, "return F:GetAlpha()"),
        0.0,
        "the first order took it to 0"
    );
    assert!(s.eval::<bool>("return A2:IsDelaying()").unwrap());
    s.tick(1.0);
    let a = eval(&s, "return F:GetAlpha()");
    assert!((a - 0.5).abs() < 0.01, "the second half way in: {a}");
}

#[test]
fn a_repeating_group_loops_and_stop_ends_it_requested() {
    let mut s = script();
    s.run(
        r#"log = {}
        G = F:CreateAnimationGroup() G:SetLooping("REPEAT")
        A = G:CreateAnimation("Animation") A:SetDuration(1)
        G:SetScript("OnLoop", function(_, st) log[#log + 1] = st end)
        G:SetScript("OnStop", function(_, r) log[#log + 1] = "stop:" .. tostring(r) end)
        G:Play()"#,
    )
    .unwrap();
    s.tick(1.1);
    s.tick(1.0);
    s.run("G:Stop()").unwrap();
    assert_eq!(
        s.eval::<String>("return table.concat(log, ',')").unwrap(),
        "FORWARD,FORWARD,stop:true"
    );
    assert_eq!(s.eval::<String>("return G:GetLoopState()").unwrap(), "NONE");
}

#[test]
fn a_hidden_frame_does_not_tick_its_groups() {
    let mut s = script();
    s.run(
        r#"G = F:CreateAnimationGroup() A = G:CreateAnimation() A:SetDuration(1) G:Play()
        F:Hide()"#,
    )
    .unwrap();
    s.tick(2.0);
    assert!(s.eval::<bool>("return G:IsPlaying()").unwrap());
    s.run("F:Show()").unwrap();
    s.tick(1.1);
    assert!(!s.eval::<bool>("return G:IsPlaying()").unwrap());
}

#[test]
fn a_translation_moves_the_drawn_quad_not_the_layout() {
    let mut s = script();
    s.run(
        r#"T = F:CreateTexture() T:SetAllPoints() T:SetTexture(1, 1, 1)
        G = T:CreateAnimationGroup()
        M = G:CreateAnimation("Translation") M:SetOffset(100, 0) M:SetDuration(1)
        G:Play()"#,
    )
    .unwrap();
    s.resolve();
    let left = eval(&s, "return T:GetLeft()");
    s.tick(0.5);
    s.resolve();
    assert_eq!(eval(&s, "return T:GetLeft()"), left, "the layout stands");
    let quads = s.extract();
    let drawn = quads
        .iter()
        .filter_map(|q| q.rect)
        .any(|r| (f64::from(r.left) - (left + 50.0)).abs() < 0.5);
    assert!(drawn, "no quad moved by half the offset");
}

#[test]
fn xml_groups_bind_their_keys_and_play_from_their_onload() {
    let mut s = UiScript::new().unwrap();
    let doc = crate::framexml::parse(
        r#"<Ui>
        <Frame name="XmlAnimFrame" parent="UIParent">
            <Size x="10" y="10"/>
            <Anchors><Anchor point="CENTER"/></Anchors>
            <Animations>
                <AnimationGroup name="$parentTimer" parentKey="timer">
                    <Animation name="$parentWait" duration="1" order="1" parentKey="wait">
                        <Scripts><OnFinished>done = (done or 0) + 1</OnFinished></Scripts>
                    </Animation>
                    <Scripts><OnLoad>self:Play()</OnLoad></Scripts>
                </AnimationGroup>
                <AnimationGroup parentKey="fade">
                    <Alpha change="-1" duration="0.5" order="1"/>
                </AnimationGroup>
            </Animations>
        </Frame>
        </Ui>"#,
    )
    .unwrap();
    let report = crate::loader::load_in(&s, &doc, "test.xml", &|_| None);
    assert!(report.errors.is_empty(), "{:#?}", report.errors);
    assert!(s
        .eval::<bool>("return XmlAnimFrame.timer == XmlAnimFrameTimer and XmlAnimFrame.timer.wait == XmlAnimFrameTimerWait")
        .unwrap());
    assert!(s
        .eval::<bool>("return XmlAnimFrame.timer:IsPlaying()")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return XmlAnimFrame.fade:GetAnimations():GetObjectType()")
            .unwrap(),
        "Alpha"
    );
    s.tick(1.1);
    assert_eq!(eval(&s, "return done"), 1.0);
    assert_eq!(
        s.eval::<String>("return XmlAnimFrameTimerWait:GetRegionParent():GetName()")
            .unwrap(),
        "XmlAnimFrame"
    );
}

/// `CreateTexture(name, layer, template)` loads the virtual texture into the new region, its
/// `<Animations>` included, as `Blizzard_GlyphUI`'s sparkles are made (`GlyphSparkleTexture`).
#[test]
fn a_texture_made_from_a_template_gets_its_animations() {
    let s = UiScript::new().unwrap();
    let doc = crate::framexml::parse(
        r#"<Ui>
        <Texture name="SparkleTemplate" alphaMode="ADD" hidden="true" virtual="true">
            <Size x="13" y="13"/>
            <Animations>
                <AnimationGroup name="$parentAnimGroup" parentKey="animGroup">
                    <Translation name="$parentTranslate" parentKey="translate"/>
                </AnimationGroup>
            </Animations>
        </Texture>
        <Frame name="SparkleHost" parent="UIParent"><Size x="100" y="100"/></Frame>
        </Ui>"#,
    )
    .unwrap();
    let report = crate::loader::load_in(&s, &doc, "test.xml", &|_| None);
    assert!(report.errors.is_empty(), "{:#?}", report.errors);
    s.run(
        r#"
        local t = SparkleHost:CreateTexture("SparkleHostSparkle1", "OVERLAY", "SparkleTemplate")
        assert(t.animGroup == SparkleHostSparkle1AnimGroup, "the group binds its key")
        assert(t.animGroup.translate:GetObjectType() == "Translation")
        assert(not t:IsShown(), "hidden as the template says")
        assert(t:GetWidth() == 13, "sized as the template says")
        t.animGroup.translate:SetOffset(10, 20)
        t.animGroup.translate:SetDuration(1)
        t.animGroup.translate:Play()
        assert(t.animGroup:IsPlaying())
        "#,
    )
    .unwrap();
}
