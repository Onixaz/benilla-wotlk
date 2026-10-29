//! The update walk (`0x495320`): shown frames by stratum, level and link order, each frame's kind
//! update in its own slot, the layout drained around it, and the model paint after it.

use super::common::script;
use crate::script::UiScript;
use crate::widget::KindState;

/// The `LOG` table, joined, and cleared.
fn take_log(s: &UiScript) -> String {
    s.eval::<String>("local t = table.concat(LOG, ' ') LOG = {} return t")
        .unwrap()
}

/// A frame named `name` at `strata`/`level` that logs its name on each `OnUpdate`.
fn logger(name: &str, strata: &str, level: u16) -> String {
    format!(
        r#"{name} = CreateFrame("Frame", "{name}")
        {name}:SetFrameStrata("{strata}") {name}:SetFrameLevel({level})
        {name}:SetScript("OnUpdate", function() table.insert(LOG, "{name}") end)
        "#
    )
}

/// `0x495320` walks strata low to high, then levels low to high (`0x494a10` per level), then each
/// level's list oldest link first: creation order is not walk order.
#[test]
fn the_walk_runs_by_strata_then_level_then_link_order() {
    let mut s = script();
    s.run(&format!(
        "LOG = {{}} {} {} {} {} {}",
        logger("HighZero", "HIGH", 0),
        logger("MediumFive", "MEDIUM", 5),
        logger("LowNine", "LOW", 9),
        logger("MediumOld", "MEDIUM", 1),
        logger("MediumNew", "MEDIUM", 1),
    ))
    .unwrap();
    // A hide and show relinks at the level's tail.
    s.run("MediumOld:Hide() MediumOld:Show()").unwrap();
    s.tick(0.01);
    assert_eq!(
        take_log(&s),
        "LowNine MediumNew MediumOld MediumFive HighZero"
    );
}

/// The level list saves the next link before each frame runs (`0x494a10`), and an unlink moves
/// that cursor on (`0x493e90`): a frame an earlier handler hid does not run.
#[test]
fn a_frame_hidden_by_an_earlier_handler_does_not_run() {
    let mut s = script();
    s.run(&format!(
        "LOG = {{}} {} {}",
        logger("First", "MEDIUM", 0),
        logger("Second", "MEDIUM", 0)
    ))
    .unwrap();
    s.run("First:SetScript('OnUpdate', function() table.insert(LOG, 'First') Second:Hide() end)")
        .unwrap();
    s.tick(0.01);
    assert_eq!(take_log(&s), "First");
}

/// A frame linked into the level being walked joins its pending list and waits for the next
/// pass (`0x495de0`, `0x494a10`); one linked into a later level runs there this pass.
#[test]
fn a_link_into_the_walked_level_waits_and_one_into_a_later_level_runs() {
    let mut s = script();
    s.run(&format!(
        "LOG = {{}} {} {} {}",
        logger("Shower", "MEDIUM", 0),
        logger("SameLevel", "MEDIUM", 0),
        logger("LaterLevel", "MEDIUM", 3)
    ))
    .unwrap();
    s.run(
        r#"
        SameLevel:Hide() LaterLevel:Hide()
        Shower:SetScript("OnUpdate", function()
            table.insert(LOG, "Shower") SameLevel:Show() LaterLevel:Show()
            Shower:SetScript("OnUpdate", nil)
        end)
        "#,
    )
    .unwrap();
    s.tick(0.01);
    assert_eq!(take_log(&s), "Shower LaterLevel");
    s.tick(0.01);
    assert_eq!(take_log(&s), "SameLevel LaterLevel");
}

/// A frame its own handler lifts to a later level relinks there and runs again this pass; one
/// a handler drops below the walk waits for the next.
#[test]
fn a_level_change_mid_walk_moves_the_frames_slot() {
    let mut s = script();
    s.run(&format!(
        "LOG = {{}} {} {}",
        logger("Climber", "MEDIUM", 0),
        logger("Sinker", "MEDIUM", 4)
    ))
    .unwrap();
    s.run(
        r#"
        Climber:SetScript("OnUpdate", function()
            table.insert(LOG, "Climber" .. Climber:GetFrameLevel())
            if Climber:GetFrameLevel() == 0 then Climber:SetFrameLevel(6) end
        end)
        Sinker:SetScript("OnUpdate", function()
            table.insert(LOG, "Sinker" .. Sinker:GetFrameLevel())
            Sinker:SetFrameLevel(0)
        end)
        "#,
    )
    .unwrap();
    s.tick(0.01);
    assert_eq!(take_log(&s), "Climber0 Sinker4 Climber6");
    s.tick(0.01);
    assert_eq!(take_log(&s), "Sinker0 Climber6");
}

/// A raise inside the walk compresses the stratum and relinks what it moves (`0x4951a0`,
/// `0x495060`): the raised frame runs again at its new level, and a frame compressed into the level
/// being walked waits for the next pass.
#[test]
fn a_raise_during_the_walk_follows_the_relinked_frames() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        LOG = {}
        local function window(name, level)
            local f = CreateFrame("Frame", name)
            f:SetPoint("BOTTOMLEFT", 100, 100) f:SetWidth(200) f:SetHeight(200)
            f:SetFrameLevel(level) f:SetToplevel(true)
            f:SetScript("OnUpdate", function() table.insert(LOG, name .. f:GetFrameLevel()) end)
            return f
        end
        Back = window("Back", 2)
        Front = window("Front", 4)
        Back:SetScript("OnUpdate", function()
            table.insert(LOG, "Back" .. Back:GetFrameLevel())
            if not raised then raised = true Back:Raise() end
        end)
        "#,
    )
    .unwrap();
    s.resolve();
    s.tick(0.01);
    // Compression drops Back 2 -> 0 and Front 4 -> 2 under a bound of 3, then Back lands on 3.
    assert_eq!(take_log(&s), "Back2 Back3");
    assert_eq!(s.eval::<i64>("return Front:GetFrameLevel()").unwrap(), 2);
    s.tick(0.01);
    assert_eq!(take_log(&s), "Front2 Back3");
}

/// A kind's own update runs in its frame's slot (`CGCooldown::OnLayerUpdate` `0x5ec8f0`): a
/// cooldown whose flash ends hides between the frames walked before it and those after.
#[test]
fn a_cooldowns_self_hide_lands_in_its_walk_slot() {
    let mut s = script();
    s.run(
        r#"
        LOG = {}
        CD = CreateFrame("Cooldown", "CD")
        CD:SetFrameLevel(5)
        local function watcher(name, level)
            local f = CreateFrame("Frame", name)
            f:SetFrameLevel(level)
            f:SetScript("OnUpdate", function()
                table.insert(LOG, name .. ":" .. (CD:IsShown() and "shown" or "hidden"))
            end)
        end
        watcher("Before", 0)
        watcher("After", 9)
        "#,
    )
    .unwrap();
    s.tick(1.0);
    s.run("CD:SetCooldown(GetTime() - 10, 1)").unwrap();
    s.tick(0.01); // the elapsed cooldown starts its flash
    take_log(&s);
    s.tick(1.5); // the flash ends inside the walk
    assert_eq!(take_log(&s), "Before:shown After:hidden");
}

/// The message kinds fade only in their walk slot (`0x969e60`): a hidden frame's lines keep their
/// alpha until it shows again.
#[test]
fn a_hidden_scrolling_message_frame_does_not_fade() {
    let mut s = script();
    s.run(
        r#"
        Chat = CreateFrame("ScrollingMessageFrame", "Chat")
        Chat:SetPoint("BOTTOMLEFT", 0, 0) Chat:SetWidth(300) Chat:SetHeight(100)
        Chat:SetTimeVisible(1) Chat:SetFadeDuration(1)
        Chat:AddMessage("hello")
        Chat:Hide()
        "#,
    )
    .unwrap();
    s.resolve();
    let alpha = |s: &UiScript| {
        let model = s.model_ref();
        let h = model.arena.lookup("Chat").unwrap();
        match &model.arena.frame(h).unwrap().kind_state {
            KindState::ScrollingMessage(smf) => smf.lines[0].alpha,
            _ => unreachable!(),
        }
    };
    for _ in 0..5 {
        s.tick(1.0);
    }
    assert_eq!(alpha(&s), 1.0, "no fade while hidden");
    s.run("Chat:Show()").unwrap();
    for _ in 0..3 {
        s.tick(1.0);
    }
    assert_eq!(alpha(&s), 0.0, "the shown frame fades and retires the line");
}

/// The model paint (`0x95fc30`) runs in draw order, not creation order, and only for a pane whose
/// rect meets the screen.
#[test]
fn on_update_model_fires_in_draw_order_for_panes_on_screen() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        LOG = {}
        local function pane(name, level, x)
            local m = CreateFrame("Model", name)
            m:SetPoint("BOTTOMLEFT", x, 100) m:SetWidth(64) m:SetHeight(64)
            m:SetFrameLevel(level)
            m:SetModel("Interface\\Paint.m2")
            m:SetScript("OnUpdateModel", function() table.insert(LOG, name) end)
        end
        pane("Upper", 7, 100)
        pane("Lower", 2, 200)
        pane("Offscreen", 1, 5000)
        "#,
    )
    .unwrap();
    s.resolve();
    s.tick(0.01);
    assert_eq!(take_log(&s), "Lower Upper");
}

/// An ancestor's hide reaches the tooltip's `HideThis` override (`0x61eb20`), which drops its
/// owner, so the ancestor's next show finds it ownerless and its `ShowThis` override (`0x61cff0`)
/// hides it for good.
#[test]
fn an_ancestor_hide_clears_the_tooltips_owner() {
    let s = script();
    s.run(
        r#"
        Holder = CreateFrame("Frame", "Holder")
        Owner = CreateFrame("Button", "TipOwner")
        Tip = CreateFrame("GameTooltip", "HeldTip", Holder)
        Tip:SetOwner(Owner, "ANCHOR_RIGHT")
        Tip:AddLine("line")
        Tip:Show()
        Tip:FadeOut()
        "#,
    )
    .unwrap();
    assert!(s
        .eval::<bool>("return Tip:IsOwned(Owner) and true or false")
        .unwrap());
    s.run("Holder:Hide()").unwrap();
    assert!(s.eval::<bool>("return Tip:GetOwner() == nil").unwrap());
    assert_eq!(s.eval::<i64>("return Tip:NumLines()").unwrap(), 0);
    s.run("Holder:Show()").unwrap();
    assert!(!s
        .eval::<bool>("return Tip:IsShown() and true or false")
        .unwrap());
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The pump drains the layout before the walk (`CLayoutFrame::ResizePending` in `0x495320`): a
/// resize an event handler made fires its `OnSizeChanged` ahead of the walk's `OnUpdate`s.
#[test]
fn an_event_resize_fires_on_size_changed_before_the_walk() {
    let mut s = script();
    s.run(
        r#"
        LOG = {}
        Box = CreateFrame("Frame", "Box")
        Box:SetPoint("BOTTOMLEFT", 0, 0) Box:SetWidth(10) Box:SetHeight(10)
        Box:RegisterEvent("PLAYER_TARGET_CHANGED")
        Box:SetScript("OnEvent", function() table.insert(LOG, "event") Box:SetWidth(20) end)
        Box:SetScript("OnSizeChanged", function() table.insert(LOG, "size") end)
        Box:SetScript("OnUpdate", function() table.insert(LOG, "update") end)
        "#,
    )
    .unwrap();
    s.resolve();
    s.tick(0.01);
    take_log(&s);
    s.queue_event("PLAYER_TARGET_CHANGED", Vec::new());
    s.tick(0.01);
    assert_eq!(take_log(&s), "event size update");
}

/// A fade step moves only line alphas: a paint write of the frame, which the extract patches
/// alone, never a full mutation that sends it back to its whole walk.
#[test]
fn a_fading_message_line_is_a_paint_write() {
    let mut s = script();
    s.run(
        r#"
        Chat = CreateFrame("ScrollingMessageFrame", "Chat")
        Chat:SetPoint("BOTTOMLEFT", 0, 0) Chat:SetWidth(300) Chat:SetHeight(100)
        Chat:SetTimeVisible(1) Chat:SetFadeDuration(4)
        Chat:AddMessage("hello")
        "#,
    )
    .unwrap();
    s.resolve();
    s.tick(1.5); // into the fade
    let (epoch, paint) = (s.visual_epoch(), s.paint_epoch());
    s.tick(0.5);
    assert_eq!(s.visual_epoch(), epoch, "the step is no full mutation");
    assert_ne!(
        s.paint_epoch(),
        paint,
        "the step marks the frame for its paint"
    );
}
