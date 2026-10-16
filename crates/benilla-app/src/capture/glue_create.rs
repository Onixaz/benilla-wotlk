//! `glue-charcreate`: the stock create screen with the scenario's body on its stage. The screen is
//! set as character select's Create New Character sets it, then the body is pressed in
//! ([`super::glue_press::press_body`]); `WOW_CHARCREATE_PICK="race,sex,class"` overrides the
//! scenario's pick, so one scenario photographs every race's stage.

use benilla_protocol::CharCreateReq;
use bevy::prelude::*;

use super::glue_press;
use super::CaptureCtx;

/// The scenario's name in [`super::scenarios::GLUE_SCENARIOS`].
pub(super) const NAME: &str = "glue-charcreate";

/// The pick: `WOW_CHARCREATE_PICK`, else the scenario's.
fn pick(scenario: (u8, u8, u8)) -> (u8, u8, u8) {
    let Ok(spec) = std::env::var("WOW_CHARCREATE_PICK") else {
        return scenario;
    };
    let mut it = spec.split(',').map(|s| s.trim().parse::<u8>().ok());
    let mut next = |default| it.next().flatten().unwrap_or(default);
    (next(scenario.0), next(scenario.1), next(scenario.2))
}

/// Set the create screen once the glue VM stands, then press the body in once it stands.
pub(super) fn seed_char_create(
    ctx: Res<CaptureCtx>,
    mut step: Local<u8>,
    script: Option<NonSend<benilla_ui::script::UiScript>>,
    catalog: Option<Res<crate::entities::CharCreate>>,
    classes: Option<Res<crate::chr_classes::ChrClassTable>>,
) {
    if ctx.name != NAME || *step > 1 {
        return;
    }
    let Some(s) = glue_press::glue(script.as_deref()) else {
        return;
    };
    match *step {
        0 => {
            glue_press::press_create_screen(s);
            *step = 1;
        }
        _ if glue_press::create_up(s) => {
            let scenario = super::scenarios::GLUE_SCENARIOS
                .iter()
                .find(|g| g.name == NAME)
                .and_then(|g| g.pick)
                .unwrap_or((1, 0, 1));
            let (race, gender, class) = pick(scenario);
            let files = (
                catalog.as_deref().and_then(|c| c.0.race_file(race)),
                classes.as_deref().and_then(|c| c.0.file(u32::from(class))),
            );
            let (Some(race_file), Some(class_file)) = files else {
                return; // the tables are not read yet
            };
            let req = CharCreateReq {
                name: String::new(),
                race,
                class,
                gender,
                skin: 0,
                face: 0,
                hair_style: 0,
                hair_color: 0,
                facial_hair: 0,
            };
            if let Err(why) = glue_press::press_body(s, &req, race_file, class_file) {
                warn!("glue-charcreate: {why}");
            }
            *step = 2;
        }
        _ => {}
    }
}
