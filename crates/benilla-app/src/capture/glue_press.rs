//! The stock character screens as an instrument presses them: the create screen's race, sex,
//! class and five customizations set through its own handlers and verbs, a name typed and Okay
//! pressed, a row deleted or picked. Each is what a player's clicks call, so the glue manager's
//! states, dialogs and roster asks run as they do for a player.

use benilla_protocol::CharCreateReq;
use benilla_ui::script::{UiScript, VmKind};

/// The glue VM, while the world holds one.
pub(super) fn glue(script: Option<&UiScript>) -> Option<&UiScript> {
    script.filter(|s| s.vm_kind() == VmKind::Glue)
}

fn visible(s: &UiScript, frame: &str) -> bool {
    s.eval::<bool>(&format!(
        "return {frame} ~= nil and {frame}:IsVisible() == 1"
    ))
    .unwrap_or(false)
}

/// Run what a click runs; a raise is the instrument's to report.
fn press(s: &UiScript, chunk: &str) {
    if let Err(e) = s.run(chunk) {
        bevy::log::warn!("glue press: {chunk:?} raised: {e}");
    }
}

/// A dialog stands over the screens: the glue manager's status dialog or a failure's `OKAY`.
pub(super) fn dialog_up(s: &UiScript) -> bool {
    visible(s, "GlueDialog")
}

/// The select screen stands with nothing over it.
pub(super) fn select_up(s: &UiScript) -> bool {
    visible(s, "CharacterSelect") && !dialog_up(s)
}

/// The create screen stands with nothing over it.
pub(super) fn create_up(s: &UiScript) -> bool {
    visible(s, "CharacterCreate") && !dialog_up(s)
}

/// The select screen's rows, name and level, in row order.
pub(super) fn roster(s: &UiScript) -> Vec<(String, u8)> {
    let names = s
        .eval::<Vec<String>>(
            "local t = {} for i = 1, GetNumCharacters() do t[i] = (GetCharacterInfo(i)) end \
             return t",
        )
        .unwrap_or_default();
    let levels = s
        .eval::<Vec<f64>>(
            "local t = {} for i = 1, GetNumCharacters() do t[i] = select(4, GetCharacterInfo(i)) \
             end return t",
        )
        .unwrap_or_default();
    names
        .into_iter()
        .zip(levels.into_iter().chain(std::iter::repeat(0.0)))
        .map(|(n, l)| (n, l as u8))
        .collect()
}

/// Character select's Create New Character (`CharacterSelect_SelectCharacter` at the create
/// index, which sets the `charcreate` screen).
pub(super) fn press_create_screen(s: &UiScript) {
    press(s, "SetGlueScreen(\"charcreate\")");
}

/// The glue VM has its create catalog: `GetAvailableRaces` lists a race.
pub(super) fn create_catalog_up(s: &UiScript) -> bool {
    s.eval::<bool>("return select('#', GetAvailableRaces()) > 0")
        .unwrap_or(false)
}

/// The 1-based position of `file` among a list verb's (name, file, available) triples.
fn list_index(s: &UiScript, verb: &str, file: &str) -> Option<u32> {
    s.eval::<Option<f64>>(&format!(
        "local t = {{{verb}()}} local i = 2 while t[i] do \
         if strupper(t[i]) == strupper({file:?}) then return (i + 1) / 3 end i = i + 3 end"
    ))
    .ok()
    .flatten()
    .map(|n| n as u32)
}

/// Put `req`'s body up on the create screen: its race button's click (`CharacterRace_OnClick`,
/// whose button is checked first, as the click would), its sex (`SetCharacterGender`), its class
/// button's click (`CharacterClass_OnClick`), then each customization's arrow pressed once per
/// step to `req`'s value. `race_file` and `class_file` are the race's and class's file strings.
/// Returns why it could not: a race or class the screen does not list.
pub(super) fn press_body(
    s: &UiScript,
    req: &CharCreateReq,
    race_file: &str,
    class_file: &str,
) -> Result<(), String> {
    let race = list_index(s, "GetAvailableRaces", race_file)
        .ok_or(format!("the create screen lists no {race_file}"))?;
    press(
        s,
        &format!(
            "CharacterCreateRaceButton{race}:SetChecked(1) \
             CharacterRace_OnClick(CharacterCreateRaceButton{race}, {race}) \
             SetCharacterGender({})",
            if req.gender == 1 {
                "SEX_FEMALE"
            } else {
                "SEX_MALE"
            }
        ),
    );
    let class = list_index(s, "GetAvailableClasses", class_file)
        .ok_or(format!("the create screen lists no {class_file}"))?;
    if s.eval::<Option<f64>>(&format!(
        "return IsRaceClassValid(GetSelectedRace(), {class})"
    ))
    .ok()
    .flatten()
    .is_none()
    {
        return Err(format!("{race_file} cannot be a {class_file}"));
    }
    press(s, &format!("CharacterClass_OnClick({class})"));
    let look = s
        .glue_create_look()
        .ok_or("the create screen built no character")?;
    let want = [
        req.skin,
        req.face,
        req.hair_style,
        req.hair_color,
        req.facial_hair,
    ];
    let mut chunk = String::new();
    for (i, (&have, &want)) in look.character.dials.iter().zip(&want).enumerate() {
        let delta = i32::from(want) - i32::from(have);
        for _ in 0..delta.unsigned_abs() {
            chunk.push_str(&format!(
                "CycleCharCustomization({}, {}) ",
                i + 1,
                delta.signum()
            ));
        }
    }
    if !chunk.is_empty() {
        press(s, &chunk);
    }
    Ok(())
}

/// Type `name` and press Okay (`CharacterCreate_Okay`), whose `CreateCharacter` reaches the glue
/// manager's create.
pub(super) fn press_okay(s: &UiScript, name: &str) {
    press(
        s,
        &format!("CharacterCreateNameEdit:SetText({name:?}) CharacterCreate_Okay()"),
    );
}

/// The standing dialog's button, then the create screen's Back (`CharacterCreate_Back`), which
/// returns to character select and its roster ask.
pub(super) fn press_dialog_and_back(s: &UiScript) {
    press(s, "GlueDialogButton1:Click() CharacterCreate_Back()");
}

/// Delete `row` (0-based): the row selected, then the delete dialog's verb.
pub(super) fn press_delete(s: &UiScript, row: usize) {
    let n = row + 1;
    press(
        s,
        &format!(
            "CharacterSelect_SelectCharacter({n}) DeleteCharacter(CharacterSelect.selectedIndex)"
        ),
    );
}

/// Pick `row` (0-based) and enter the world, as a click on the row and on Enter World.
pub(super) fn press_enter(s: &UiScript, row: usize) {
    press(
        s,
        &format!(
            "CharacterSelect_SelectCharacter({}) CharacterSelect_EnterWorld()",
            row + 1
        ),
    );
}

/// The stock realm list stands, over whatever screen it opened on.
pub(super) fn realm_list_up(s: &UiScript) -> bool {
    visible(s, "RealmList")
}

/// The select screen stands, whatever is over it.
pub(super) fn select_shown(s: &UiScript) -> bool {
    visible(s, "CharacterSelect")
}

/// Character select's Change Realm (`CharacterSelect_ChangeRealm`, a `RequestRealmList(1)`).
pub(super) fn press_change_realm(s: &UiScript) {
    press(s, "CharacterSelect_ChangeRealm()");
}

/// The realm list's Okay (`RealmList_OnOk`) on the realm it highlighted; returns that realm's
/// index, 0 when none was, in which case the press enters nothing.
pub(super) fn press_realm_okay(s: &UiScript) -> u32 {
    let current = s
        .eval::<f64>("return RealmList.currentRealm or 0")
        .unwrap_or(0.0) as u32;
    press(s, "RealmList_OnOk()");
    current
}

/// The realm list's Cancel (`RealmList_OnCancel`).
pub(super) fn press_realm_cancel(s: &UiScript) {
    press(s, "RealmList_OnCancel()");
}
